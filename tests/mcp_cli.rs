use serde_json::{Value, json};
#[cfg(unix)]
use std::time::{Duration, Instant};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

fn run_mcp(messages: &[Value]) -> std::process::Output {
    let cwd = std::env::current_dir().unwrap();
    run_mcp_in(&cwd, None, messages)
}

fn run_mcp_in(cwd: &Path, home: Option<&Path>, messages: &[Value]) -> std::process::Output {
    run_mcp_in_with_root(cwd, home, None, messages)
}

fn run_mcp_in_with_root(
    cwd: &Path,
    home: Option<&Path>,
    project_root: Option<&Path>,
    messages: &[Value],
) -> std::process::Output {
    run_mcp_in_with_env(cwd, home, project_root, None, None, messages)
}

fn run_mcp_in_with_env(
    cwd: &Path,
    home: Option<&Path>,
    project_root: Option<&Path>,
    codegraph_binary: Option<&Path>,
    fake_log: Option<&Path>,
    messages: &[Value],
) -> std::process::Output {
    run_mcp_in_with_extra_env(
        cwd,
        home,
        project_root,
        codegraph_binary,
        fake_log,
        &[],
        messages,
    )
}

fn run_mcp_in_with_extra_env(
    cwd: &Path,
    home: Option<&Path>,
    project_root: Option<&Path>,
    codegraph_binary: Option<&Path>,
    fake_log: Option<&Path>,
    extra_env: &[(&str, &str)],
    messages: &[Value],
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lwc"));
    command
        .args(["serve", "--mcp"])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(home) = home {
        command.env("HOME", home);
    }
    if let Some(project_root) = project_root {
        command.env("LWC_PROJECT_ROOT", project_root);
    }
    if let Some(codegraph_binary) = codegraph_binary {
        command.env("LWC_CODEGRAPH_BINARY", codegraph_binary);
    }
    if let Some(fake_log) = fake_log {
        command.env("FAKE_CODEGRAPH_LOG", fake_log);
    }
    command.envs(extra_env.iter().copied());
    let mut child = command.spawn().unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        for message in messages {
            serde_json::to_writer(&mut *stdin, message).unwrap();
            stdin.write_all(b"\n").unwrap();
        }
    }
    drop(child.stdin.take());
    child.wait_with_output().unwrap()
}

fn run_mcp_raw(input: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .args(["serve", "--mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    drop(child.stdin.take());
    child.wait_with_output().unwrap()
}

fn run_lwc(cwd: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(cwd)
        .env("HOME", home)
        .args(args)
        .output()
        .unwrap()
}

fn responses(output: &std::process::Output) -> Vec<Value> {
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect()
}

fn file_snapshot(root: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    fn walk(
        root: &Path,
        current: &Path,
        files: &mut std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
    ) {
        for entry in fs::read_dir(current).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, files);
            } else if path.to_string_lossy().ends_with("-shm") {
                // SQLite readers coordinate through this transient shared-memory file.
                continue;
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut files = std::collections::BTreeMap::new();
    walk(root, root, &mut files);
    files
}

#[test]
fn mcp_server_negotiates_and_lists_read_only_tools() {
    let output = run_mcp(&[
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "lwc-test", "version": "1"}
            }
        }),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "ping"}),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let responses = responses(&output);
    assert_eq!(
        responses.len(),
        3,
        "notifications must not receive responses"
    );

    assert_eq!(responses[0]["id"], 1);
    assert_eq!(responses[0]["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(responses[0]["result"]["serverInfo"]["name"], "lwc");
    let instructions = responses[0]["result"]["instructions"].as_str().unwrap();
    assert!(instructions.contains("using-lwc Skill"));
    assert!(instructions.contains("LWC_READINESS"));
    assert!(instructions.contains("explicit user consent"));
    assert!(instructions.contains("never downloads, initializes, or mutates"));
    assert_eq!(
        responses[0]["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );
    assert!(responses[0]["result"]["capabilities"]["tools"].is_object());

    assert_eq!(responses[1]["id"], 2);
    let tools = responses[1]["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2);
    let tool = tools
        .iter()
        .find(|tool| tool["name"] == "lwc_explore")
        .unwrap();
    assert_eq!(tool["name"], "lwc_explore");
    assert_eq!(tool["inputSchema"]["additionalProperties"], false);
    assert_eq!(
        tool["inputSchema"]["required"],
        json!(["query", "projectPath"])
    );
    assert_eq!(tool["annotations"]["readOnlyHint"], true);
    assert_eq!(tool["annotations"]["destructiveHint"], false);
    assert_eq!(tool["annotations"]["idempotentHint"], true);
    assert_eq!(tool["annotations"]["openWorldHint"], false);
    let codegraph = tools
        .iter()
        .find(|tool| tool["name"] == "lwc_codegraph")
        .unwrap();
    assert_eq!(codegraph["inputSchema"]["additionalProperties"], false);
    assert_eq!(
        codegraph["inputSchema"]["required"],
        json!(["command", "arguments", "projectPath"])
    );
    assert_eq!(codegraph["annotations"]["readOnlyHint"], true);
    assert_eq!(codegraph["annotations"]["destructiveHint"], false);
    assert_eq!(codegraph["annotations"]["idempotentHint"], true);
    assert_eq!(codegraph["annotations"]["openWorldHint"], false);

    assert_eq!(
        responses[2],
        json!({"jsonrpc": "2.0", "id": 3, "result": {}})
    );
}

#[test]
fn mcp_rejects_relative_project_path_without_creating_state() {
    let output = run_mcp(&[json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "lwc_explore",
            "arguments": {"query": "release policy", "projectPath": "relative/project"}
        }
    })]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let responses = responses(&output);
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0]["id"], 1);
    assert_eq!(responses[0]["result"]["isError"], true);
    let error: Value = serde_json::from_str(
        responses[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(error["error"]["code"], "invalid_project_path");
    assert!(!std::path::Path::new("relative/project/.lwc").exists());
}

#[test]
fn mcp_enforces_declared_mode_scope_and_count_bounds() {
    let project = tempfile::tempdir().unwrap();
    for arguments in [
        json!({"query": "x", "projectPath": project.path(), "mode": "guess"}),
        json!({"query": "x", "projectPath": project.path(), "scope": "nearby"}),
        json!({"query": "x", "projectPath": project.path(), "maxDocuments": 0}),
        json!({"query": "x", "projectPath": project.path(), "maxFiles": 21}),
    ] {
        let output = run_mcp(&[json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "lwc_explore", "arguments": arguments}
        })]);
        assert!(output.status.success());
        let response = responses(&output).remove(0);
        assert_eq!(response["result"]["isError"], true);
        let error: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(error["error"]["code"], "invalid_arguments");
    }
}

#[test]
fn mcp_rejects_oversized_frames_and_sensitive_roots_then_continues() {
    let oversized = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"{}\"}}\n{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}}\n",
        "x".repeat(65_536)
    );
    let output = run_mcp_raw(&oversized);
    assert!(output.status.success());
    let frames = responses(&output);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["error"]["code"], -32600);
    assert_eq!(frames[1]["result"], json!({}));

    let output = run_mcp(&[json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "lwc_explore",
            "arguments": {"query": "x", "projectPath": std::path::MAIN_SEPARATOR.to_string()}
        }
    })]);
    let response = responses(&output).remove(0);
    let error: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(error["error"]["code"], "invalid_project_path");
}

#[cfg(unix)]
#[test]
fn mcp_rejects_a_symlinked_project_boundary() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let alias = temp.path().join("alias");
    fs::create_dir_all(&project).unwrap();
    symlink(&project, &alias).unwrap();
    let output = run_mcp(&[json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "lwc_explore", "arguments": {
            "query": "x",
            "projectPath": alias
        }}
    })]);
    let response = responses(&output).remove(0);
    let error: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(error["error"]["code"], "invalid_project_path");
}

#[test]
fn mcp_explicit_project_path_ignores_conflicting_ambient_root() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let selected = temp.path().join("selected");
    let ambient = temp.path().join("ambient");
    fs::create_dir_all(&home).unwrap();
    for project in [&selected, &ambient] {
        fs::create_dir_all(project).unwrap();
        assert!(run_lwc(project, &home, &["init"]).status.success());
    }
    for (project, slug) in [(&selected, "selected-page"), (&ambient, "ambient-page")] {
        let body = project.join("body.md");
        fs::write(&body, "unique boundary token").unwrap();
        assert!(
            run_lwc(
                project,
                &home,
                &[
                    "page",
                    "put",
                    slug,
                    "--title",
                    slug,
                    "--file",
                    body.to_str().unwrap(),
                    "--provenance",
                    "agent-observed",
                ],
            )
            .status
            .success()
        );
    }

    let output = run_mcp_in_with_root(
        &selected,
        Some(&home),
        Some(&ambient),
        &[json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "lwc_explore", "arguments": {
                "query": "unique boundary token",
                "projectPath": selected,
                "scope": "project"
            }}
        })],
    );
    let response = responses(&output).remove(0);
    let result: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(result["memory"]["pages"][0]["slug"], "selected-page");
    assert!(
        result["memory"]["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["identifier"] != "ambient-page")
    );
}

#[test]
fn mcp_rejects_project_paths_outside_the_host_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let sibling = temp.path().join("sibling");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&sibling).unwrap();

    let output = run_mcp_in(
        &workspace,
        None,
        &[json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "lwc_explore", "arguments": {
                "query": "private sibling",
                "projectPath": sibling
            }}
        })],
    );
    let response = responses(&output).remove(0);
    let error: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(error["error"]["code"], "project_path_outside_workspace");
}

#[test]
fn mcp_explicit_host_workspace_accepts_that_project_from_another_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let launcher = temp.path().join("launcher");
    let project = temp.path().join("project");
    fs::create_dir_all(&launcher).unwrap();
    fs::create_dir_all(&project).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .args(["serve", "--mcp", "--path", project.to_str().unwrap()])
        .current_dir(&launcher)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    serde_json::to_writer(
        child.stdin.as_mut().unwrap(),
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "lwc_explore", "arguments": {
                "query": "anything",
                "projectPath": project,
                "scope": "project"
            }}
        }),
    )
    .unwrap();
    child.stdin.as_mut().unwrap().write_all(b"\n").unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response = responses(&output).remove(0);
    let result: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_ne!(result["error"]["code"], "project_path_outside_workspace");
}

#[test]
fn mcp_missing_memory_is_explicit_and_side_effect_free() {
    let project = tempfile::tempdir().unwrap();
    let output = run_mcp_in(
        project.path(),
        None,
        &[json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "lwc_explore", "arguments": {
                "query": "anything",
                "projectPath": project.path(),
                "mode": "memory",
                "scope": "project"
            }}
        })],
    );
    let response = responses(&output).remove(0);
    assert_eq!(response["result"]["isError"], true);
    let result: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(result["memory"]["state"], "unavailable");
    assert_eq!(result["memory"]["error"]["code"], "store_not_found");
    assert!(!project.path().join(".lwc").exists());
}

#[test]
fn mcp_all_scope_qualifies_duplicate_pages_and_keeps_source_bodies_implicit() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&home).unwrap();
    assert!(run_lwc(&project, &home, &["init"]).status.success());
    assert!(
        run_lwc(&project, &home, &["--scope", "global", "init"])
            .status
            .success()
    );
    for (scope, body) in [
        ("project", "sharedscope project body"),
        ("global", "sharedscope global body"),
    ] {
        let file = project.join(format!("{scope}.md"));
        fs::write(&file, body).unwrap();
        let mut args = vec![];
        if scope == "global" {
            args.extend(["--scope", "global"]);
        }
        args.extend([
            "page",
            "put",
            "shared-page",
            "--title",
            "Shared page",
            "--file",
            file.to_str().unwrap(),
            "--provenance",
            "agent-observed",
        ]);
        assert!(run_lwc(&project, &home, &args).status.success());
    }
    let source = project.join("source.txt");
    fs::write(
        &source,
        format!(
            "sourceonlyneedle {} NEVER_RETURN_FULL_SOURCE_TAIL",
            "private ".repeat(4_000)
        ),
    )
    .unwrap();
    assert!(
        run_lwc(
            &project,
            &home,
            &["source", "add", source.to_str().unwrap()]
        )
        .status
        .success()
    );

    let output = run_mcp_in(
        &project,
        Some(&home),
        &[
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": "lwc_explore", "arguments": {
                    "query": "sharedscope",
                    "projectPath": project,
                    "scope": "all",
                    "maxDocuments": 20
                }}
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "lwc_explore", "arguments": {
                    "query": "sourceonlyneedle",
                    "projectPath": project,
                    "scope": "project"
                }}
            }),
        ],
    );
    let responses = responses(&output);
    let shared: Value = serde_json::from_str(
        responses[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let scopes = shared["memory"]["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|page| page["scope"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(scopes, ["global", "project"].into_iter().collect());
    let source_text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let source_result: Value = serde_json::from_str(source_text).unwrap();
    assert!(
        source_result["memory"]["pages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(!source_text.contains("NEVER_RETURN_FULL_SOURCE_TAIL"));
}

#[test]
fn mcp_code_mode_reports_missing_index_without_initializing_or_downloading() {
    let project = tempfile::tempdir().unwrap();
    let output = run_mcp_in(
        project.path(),
        None,
        &[json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "lwc_explore", "arguments": {
                "query": "x",
                "projectPath": project.path(),
                "mode": "code"
            }}
        })],
    );
    let response = responses(&output).remove(0);
    let result: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(result["codeGraph"]["state"], "unavailable");
    assert_eq!(
        result["codeGraph"]["error"]["code"],
        "codegraph_index_missing"
    );
    assert!(!project.path().join(".lwc").exists());
}

#[cfg(unix)]
#[test]
fn mcp_code_mode_rejects_a_symlinked_index() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let outside = temp.path().join("outside-index");
    fs::create_dir_all(project.join(".lwc")).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("codegraph.db"), b"outside").unwrap();
    symlink(&outside, project.join(".lwc/codegraph")).unwrap();
    let output = run_mcp_in(
        &project,
        None,
        &[json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "lwc_explore", "arguments": {
                "query": "x",
                "projectPath": project,
                "mode": "code"
            }}
        })],
    );
    let response = responses(&output).remove(0);
    let result: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        result["codeGraph"]["error"]["code"],
        "codegraph_index_invalid"
    );
    assert_eq!(fs::read(outside.join("codegraph.db")).unwrap(), b"outside");
}

#[test]
fn mcp_memory_mode_returns_bounded_page_content_without_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&home).unwrap();
    assert!(run_lwc(&project, &home, &["init"]).status.success());
    let body = "Release policy requires a clean affected test run.";
    let body_path = project.join("release-policy.md");
    fs::write(&body_path, body).unwrap();
    let put = run_lwc(
        &project,
        &home,
        &[
            "page",
            "put",
            "release-policy",
            "--title",
            "Release policy",
            "--file",
            body_path.to_str().unwrap(),
            "--provenance",
            "agent-observed",
        ],
    );
    assert!(
        put.status.success(),
        "{}",
        String::from_utf8_lossy(&put.stderr)
    );
    let before = file_snapshot(&project.join(".lwc"));

    let output = run_mcp_in(
        &project,
        Some(&home),
        &[json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "lwc_explore",
                "arguments": {
                    "query": "release policy",
                    "projectPath": project,
                    "mode": "memory",
                    "scope": "project",
                    "maxDocuments": 8
                }
            }
        })],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response = responses(&output).remove(0);
    assert_ne!(response["result"]["isError"], true);
    let result: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(result["mode"], "memory");
    assert_eq!(result["scope"], "project");
    assert_eq!(result["memory"]["state"], "ready");
    assert_eq!(result["memory"]["pages"][0]["slug"], "release-policy");
    assert_eq!(result["memory"]["pages"][0]["body"], body);
    assert_eq!(result["memory"]["graph"][0]["status"]["status"], "disabled");
    assert!(result["memory"]["graph"][0].get("neighborhood").is_none());
    assert_eq!(result["codeGraph"]["state"], "not_requested");
    let after = file_snapshot(&project.join(".lwc"));
    let changed = before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .collect::<std::collections::BTreeSet<_>>();
    assert!(changed.is_empty(), "durable files changed: {changed:?}");
    assert!(!project.join(".lwc/codegraph").exists());
}

#[test]
fn mcp_memory_mode_caps_long_unicode_pages_and_total_output() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&home).unwrap();
    assert!(run_lwc(&project, &home, &["init"]).status.success());
    for index in 0..5 {
        let slug = format!("capacity-{index}");
        let body = project.join(format!("{slug}.md"));
        fs::write(
            &body,
            format!("第{index}篇 容量 capacity {}", "内存边界 ".repeat(4_000)),
        )
        .unwrap();
        assert!(
            run_lwc(
                &project,
                &home,
                &[
                    "page",
                    "put",
                    &slug,
                    "--title",
                    &format!("Capacity {index}"),
                    "--file",
                    body.to_str().unwrap(),
                    "--provenance",
                    "agent-observed",
                ],
            )
            .status
            .success()
        );
    }
    let output = run_mcp_in(
        &project,
        Some(&home),
        &[json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "lwc_explore", "arguments": {
                "query": "capacity",
                "projectPath": project,
                "mode": "memory",
                "scope": "project",
                "maxDocuments": 20
            }}
        })],
    );
    let response = responses(&output).remove(0);
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.chars().count() <= 80_000, "{}", text.chars().count());
    let result: Value = serde_json::from_str(text).unwrap();
    let pages = result["memory"]["pages"].as_array().unwrap();
    assert!(!pages.is_empty());
    assert!(
        pages
            .iter()
            .all(|page| page["body"].as_str().unwrap().chars().count() <= 15_000)
    );
    assert!(
        pages
            .iter()
            .map(|page| page["body"].as_str().unwrap().chars().count())
            .sum::<usize>()
            <= 60_000
    );
    assert!(pages.iter().any(|page| page["truncated"] == true));
}

#[test]
fn mcp_memory_mode_expands_only_one_ready_physical_graph_seed() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&home).unwrap();
    assert!(run_lwc(&project, &home, &["init"]).status.success());
    for (slug, body) in [
        ("graph-a", "graphseed links to [[graph-b]]."),
        ("graph-b", "graphseed related page."),
    ] {
        let file = project.join(format!("{slug}.md"));
        fs::write(&file, body).unwrap();
        assert!(
            run_lwc(
                &project,
                &home,
                &[
                    "page",
                    "put",
                    slug,
                    "--title",
                    slug,
                    "--file",
                    file.to_str().unwrap(),
                    "--provenance",
                    "agent-observed",
                ],
            )
            .status
            .success()
        );
    }
    let enabled = run_lwc(&project, &home, &["config", "set", "--graph", "grafeo"]);
    assert!(enabled.status.success());
    let enabled: Value = serde_json::from_slice(&enabled.stdout).unwrap();
    let work_id = enabled["work"]["id"].as_str().unwrap();
    assert!(
        run_lwc(&project, &home, &["work", "watch", work_id])
            .status
            .success()
    );
    let before = file_snapshot(&project.join(".lwc"));

    let output = run_mcp_in(
        &project,
        Some(&home),
        &[json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "lwc_explore", "arguments": {
                "query": "graphseed",
                "projectPath": project,
                "scope": "project"
            }}
        })],
    );
    let response = responses(&output).remove(0);
    let result: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let graph = &result["memory"]["graph"][0];
    assert_eq!(graph["status"]["status"], "ready", "{graph:#}");
    assert!(matches!(
        graph["seed"].as_str(),
        Some("graph-a" | "graph-b")
    ));
    assert!(graph["neighborhood"]["nodes"].as_array().unwrap().len() <= 12);
    assert!(
        graph["neighborhood"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["depth"].as_u64().unwrap_or(0) <= 1)
    );
    let after = file_snapshot(&project.join(".lwc"));
    let changed = before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .collect::<std::collections::BTreeSet<_>>();
    assert!(changed.is_empty(), "durable files changed: {changed:?}");
}

#[cfg(unix)]
#[test]
fn mcp_code_mode_reuses_one_project_scoped_codegraph_child() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    fs::create_dir_all(project.join(".lwc/codegraph")).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::write(project.join(".lwc/codegraph/codegraph.db"), b"indexed").unwrap();
    let fake = temp.path().join("codegraph");
    let log = temp.path().join("codegraph.log");
    fs::write(
        &fake,
        r#"#!/bin/sh
printf 'start|%s|%s|%s\n' "$PWD" "$CODEGRAPH_DIR" "$*" >> "$FAKE_CODEGRAPH_LOG"
next=1
while IFS= read -r line; do
  printf 'request|%s\n' "$line" >> "$FAKE_CODEGRAPH_LOG"
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"codegraph","version":"test"}}}\n'
      ;;
    *'"method":"tools/call"'*)
      next=$((next + 1))
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"fake code context"}]}}\n' "$next"
      ;;
  esac
done
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&fake).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake, permissions).unwrap();

    let calls = [1, 2].map(|id| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": "lwc_explore", "arguments": {
                "query": "Store search_with_options",
                "projectPath": project,
                "mode": "code",
                "maxFiles": 4
            }}
        })
    });
    let output = run_mcp_in_with_env(&project, Some(&home), None, Some(&fake), Some(&log), &calls);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let responses = responses(&output);
    assert_eq!(responses.len(), 2);
    for response in responses {
        let result: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(result["memory"]["state"], "not_requested");
        assert_eq!(result["codeGraph"]["state"], "ready");
        assert_eq!(
            result["codeGraph"]["result"]["content"][0]["text"],
            "fake code context"
        );
    }
    let log = fs::read_to_string(log).unwrap();
    assert_eq!(
        log.lines()
            .filter(|line| line.starts_with("start|"))
            .count(),
        1
    );
    assert_eq!(
        log.lines()
            .filter(|line| line.starts_with("request|"))
            .count(),
        4
    );
    assert!(log.contains("\"method\":\"notifications/initialized\""));
    assert!(log.contains("|.lwc/codegraph|serve --mcp"));
    assert!(log.contains("\"name\":\"codegraph_explore\""));
    assert!(log.contains("\"maxFiles\":4"));
}

#[cfg(unix)]
#[test]
fn mcp_code_mode_ignores_codegraph_notifications_before_the_response() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    fs::create_dir_all(project.join(".lwc/codegraph")).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::write(project.join(".lwc/codegraph/codegraph.db"), b"indexed").unwrap();
    let fake = temp.path().join("codegraph");
    fs::write(
        &fake,
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"codegraph","version":"test"}}}\n'
      ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info","data":"working"}}\n'
      printf '{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"after notification"}]}}\n'
      ;;
  esac
done
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&fake).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake, permissions).unwrap();

    let output = run_mcp_in_with_env(
        &project,
        Some(&home),
        None,
        Some(&fake),
        None,
        &[json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "lwc_explore", "arguments": {
                "query": "notification",
                "projectPath": project,
                "mode": "code"
            }}
        })],
    );
    let response = responses(&output).remove(0);
    let result: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(result["codeGraph"]["state"], "ready");
    assert_eq!(
        result["codeGraph"]["result"]["content"][0]["text"],
        "after notification"
    );
}

#[cfg(unix)]
#[test]
fn mcp_code_mode_restarts_child_after_protocol_fault() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    fs::create_dir_all(project.join(".lwc/codegraph")).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::write(project.join(".lwc/codegraph/codegraph.db"), b"indexed").unwrap();
    let fake = temp.path().join("codegraph");
    let log = temp.path().join("codegraph.log");
    fs::write(
        &fake,
        r#"#!/bin/sh
printf 'start\n' >> "$FAKE_CODEGRAPH_LOG"
starts=$(grep -c '^start' "$FAKE_CODEGRAPH_LOG")
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"codegraph","version":"test"}}}\n'
      ;;
    *'"method":"tools/call"'*)
      if [ "$starts" -eq 1 ]; then id=999; else id=2; fi
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"recovered"}]}}\n' "$id"
      ;;
  esac
done
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&fake).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake, permissions).unwrap();
    let calls = [1, 2].map(|id| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": "lwc_explore", "arguments": {
                "query": "recovery",
                "projectPath": project,
                "mode": "code"
            }}
        })
    });
    let output = run_mcp_in_with_env(&project, Some(&home), None, Some(&fake), Some(&log), &calls);
    let responses = responses(&output);
    let first: Value = serde_json::from_str(
        responses[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let second: Value = serde_json::from_str(
        responses[1]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(first["codeGraph"]["state"], "error");
    assert_eq!(
        first["codeGraph"]["error"]["code"],
        "codegraph_mcp_wrong_id"
    );
    assert_eq!(second["codeGraph"]["state"], "ready");
    assert_eq!(fs::read_to_string(log).unwrap().lines().count(), 2);
}

#[cfg(unix)]
#[test]
fn mcp_codegraph_gateway_forwards_read_only_aliases_with_canonical_path_and_raw_result() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    fs::create_dir_all(project.join(".lwc/codegraph")).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::write(project.join(".lwc/codegraph/codegraph.db"), b"indexed").unwrap();
    let fake = temp.path().join("codegraph");
    let log = temp.path().join("codegraph.log");
    fs::write(
        &fake,
        r#"#!/bin/sh
printf 'tools|%s\n' "$CODEGRAPH_MCP_TOOLS" >> "$FAKE_CODEGRAPH_LOG"
next=1
while IFS= read -r line; do
  printf 'request|%s\n' "$line" >> "$FAKE_CODEGRAPH_LOG"
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"codegraph","version":"test"}}}\n'
      ;;
    *'"method":"tools/call"'*)
      next=$((next + 1))
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"raw codegraph result"}],"isError":false,"annotations":{"audience":["assistant"]},"futureField":{"preserved":true}}}\n' "$next"
      ;;
  esac
done
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&fake).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake, permissions).unwrap();

    let calls = [
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "lwc_codegraph", "arguments": {
                "command": "node",
                "projectPath": project,
                "arguments": {
                    "symbol": "GetScanBatch",
                    "includeCode": true,
                    "projectPath": "/tmp/forged-project",
                    "futureArgument": {"preserved": true}
                }
            }}
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "lwc_codegraph", "arguments": {
                "command": "codegraph_callers",
                "projectPath": project,
                "arguments": {"symbol": "GetScanBatch"}
            }}
        }),
    ];
    let output = run_mcp_in_with_env(&project, Some(&home), None, Some(&fake), Some(&log), &calls);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected = json!({
        "content": [{"type": "text", "text": "raw codegraph result"}],
        "isError": false,
        "annotations": {"audience": ["assistant"]},
        "futureField": {"preserved": true}
    });
    for response in responses(&output) {
        assert_eq!(response["result"], expected);
    }

    let log = fs::read_to_string(log).unwrap();
    let enabled = log
        .lines()
        .find_map(|line| line.strip_prefix("tools|"))
        .unwrap()
        .split(',')
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        enabled,
        [
            "search", "callers", "callees", "impact", "node", "explore", "status", "files"
        ]
        .into_iter()
        .collect()
    );
    let calls = log
        .lines()
        .filter_map(|line| line.strip_prefix("request|"))
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|request| request["method"] == "tools/call")
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["params"]["name"], "codegraph_node");
    assert_eq!(calls[1]["params"]["name"], "codegraph_callers");
    let canonical = project.canonicalize().unwrap();
    for call in &calls {
        assert_eq!(call["params"]["arguments"]["projectPath"], json!(canonical));
    }
    assert_eq!(
        calls[0]["params"]["arguments"]["futureArgument"],
        json!({"preserved": true})
    );
    assert!(!log.contains("/tmp/forged-project"));
}

#[test]
fn mcp_codegraph_gateway_rejects_non_read_only_commands_and_non_object_arguments() {
    let project = tempfile::tempdir().unwrap();
    let calls = [
        ("sync", json!({})),
        ("codegraph_index", json!({})),
        ("unknown", json!({})),
        ("node", json!(["not", "an", "object"])),
    ]
    .map(|(command, arguments)| {
        json!({
            "jsonrpc": "2.0",
            "id": command,
            "method": "tools/call",
            "params": {"name": "lwc_codegraph", "arguments": {
                "command": command,
                "projectPath": project.path(),
                "arguments": arguments
            }}
        })
    });
    let output = run_mcp_in(project.path(), None, &calls);
    assert!(output.status.success());
    for response in responses(&output) {
        assert!(response.get("error").is_none(), "{response:#}");
        assert_eq!(response["result"]["isError"], true, "{response:#}");
    }
}

#[cfg(unix)]
#[test]
fn mcp_code_mode_routes_exact_identifiers_to_node_and_natural_language_to_explore() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    fs::create_dir_all(project.join(".lwc/codegraph")).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::write(project.join(".lwc/codegraph/codegraph.db"), b"indexed").unwrap();
    let fake = temp.path().join("codegraph");
    let log = temp.path().join("codegraph.log");
    fs::write(
        &fake,
        r#"#!/bin/sh
next=1
while IFS= read -r line; do
  printf 'request|%s\n' "$line" >> "$FAKE_CODEGRAPH_LOG"
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"codegraph","version":"test"}}}\n'
      ;;
    *'"method":"tools/call"'*)
      next=$((next + 1))
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"routed"}]}}\n' "$next"
      ;;
  esac
done
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&fake).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake, permissions).unwrap();

    let calls = [
        (1, "GetScanBatch", 12),
        (2, "batch cancellation flow", 4),
        (3, "取消流程", 2),
    ]
    .map(|(id, query, max_files)| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": "lwc_explore", "arguments": {
                "query": query,
                "projectPath": project,
                "mode": "code",
                "maxFiles": max_files
            }}
        })
    });
    let output = run_mcp_in_with_env(&project, Some(&home), None, Some(&fake), Some(&log), &calls);
    assert!(output.status.success());
    let log = fs::read_to_string(log).unwrap();
    let calls = log
        .lines()
        .filter_map(|line| line.strip_prefix("request|"))
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|request| request["method"] == "tools/call")
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0]["params"]["name"], "codegraph_node");
    assert_eq!(calls[0]["params"]["arguments"]["symbol"], "GetScanBatch");
    assert_eq!(calls[0]["params"]["arguments"]["includeCode"], true);
    assert_eq!(calls[1]["params"]["name"], "codegraph_explore");
    assert_eq!(
        calls[1]["params"]["arguments"]["query"],
        "batch cancellation flow"
    );
    assert_eq!(calls[1]["params"]["arguments"]["maxFiles"], 4);
    assert_eq!(calls[2]["params"]["name"], "codegraph_explore");
    assert_eq!(calls[2]["params"]["arguments"]["query"], "取消流程");
}

#[cfg(unix)]
#[test]
fn mcp_code_mode_waits_past_codegraph_busy_queue_deadline() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    fs::create_dir_all(project.join(".lwc/codegraph")).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::write(project.join(".lwc/codegraph/codegraph.db"), b"indexed").unwrap();
    let fake = temp.path().join("codegraph");
    fs::write(
        &fake,
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"codegraph","version":"test"}}}\n'
      ;;
    *'"method":"tools/call"'*)
      trap 'kill "$sleeper" 2>/dev/null; wait "$sleeper" 2>/dev/null; exit' TERM INT EXIT
      sleep 46 & sleeper=$!
      wait "$sleeper"
      trap - TERM INT EXIT
      printf '{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"after busy deadline"}]}}\n'
      ;;
  esac
done
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&fake).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake, permissions).unwrap();

    let output = run_mcp_in_with_env(
        &project,
        Some(&home),
        None,
        Some(&fake),
        None,
        &[json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "lwc_explore", "arguments": {
                "query": "slow broad query",
                "projectPath": project,
                "mode": "code"
            }}
        })],
    );
    let response = responses(&output).remove(0);
    let result: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        result["codeGraph"]["result"]["content"][0]["text"],
        "after busy deadline"
    );
}

#[cfg(unix)]
#[test]
fn mcp_code_mode_times_out_and_reaps_a_hung_process_tree() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    fs::create_dir_all(project.join(".lwc/codegraph")).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::write(project.join(".lwc/codegraph/codegraph.db"), b"indexed").unwrap();
    let fake = temp.path().join("codegraph");
    let log = temp.path().join("pid");
    fs::write(
        &fake,
        r#"#!/bin/sh
printf 'parent=%s\n' "$$" > "$FAKE_CODEGRAPH_LOG"
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"codegraph","version":"test"}}}\n'
      ;;
    *'"method":"tools/call"'*)
      sleep 30 & descendant=$!
      printf 'descendant=%s\n' "$descendant" >> "$FAKE_CODEGRAPH_LOG"
      wait "$descendant"
      ;;
  esac
done
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&fake).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake, permissions).unwrap();
    let started = Instant::now();
    let output = run_mcp_in_with_extra_env(
        &project,
        Some(&home),
        None,
        Some(&fake),
        Some(&log),
        &[("LWC_TEST_CODEGRAPH_MCP_TIMEOUT_MS", "5000")],
        &[json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "lwc_explore", "arguments": {
                "query": "hang",
                "projectPath": project,
                "mode": "code"
            }}
        })],
    );
    let response = responses(&output).remove(0);
    let result: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        result["codeGraph"]["error"]["code"],
        "codegraph_mcp_timeout"
    );
    let pids = fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| {
            let (name, pid) = line.split_once('=').unwrap();
            (name.to_owned(), pid.to_owned())
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert!(
        !Command::new("kill")
            .args(["-0", &pids["parent"]])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    let descendant_alive = Command::new("kill")
        .args(["-0", &pids["descendant"]])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success();
    if descendant_alive {
        let _ = Command::new("kill")
            .args(["-9", &pids["descendant"]])
            .status();
    }
    assert!(!descendant_alive, "CodeGraph descendant survived timeout");
    assert!(started.elapsed() <= Duration::from_secs(10));
}
