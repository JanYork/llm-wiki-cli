#![cfg(windows)]

use serde_json::Value;
use std::{
    collections::BTreeMap,
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

fn project_command(project: &Path, project_root: &Path, home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lwc"));
    command
        .current_dir(project)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("LWC_PROJECT_ROOT", project_root);
    command
}

fn project_json(project: &Path, project_root: &Path, home: &Path, args: &[&str]) -> Value {
    let output = project_command(project, project_root, home)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn project_hook(
    project: &Path,
    project_root: &Path,
    home: &Path,
    event: &str,
    input: &Value,
) -> Value {
    let mut child = project_command(project, project_root, home)
        .args([
            "--scope", "all", "agent", "hook", "--agent", "claude", "--event", event,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{event}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn tree_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, snapshot: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let relative = path.strip_prefix(root).unwrap().to_path_buf();
        if metadata.file_type().is_symlink() {
            snapshot.insert(
                relative,
                fs::read_link(path)
                    .unwrap()
                    .to_string_lossy()
                    .as_bytes()
                    .to_vec(),
            );
        } else if metadata.is_dir() {
            snapshot.insert(relative, b"directory".to_vec());
            let mut entries = fs::read_dir(path)
                .unwrap()
                .map(|entry| entry.unwrap())
                .collect::<Vec<_>>();
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                visit(root, &entry.path(), snapshot);
            }
        } else {
            snapshot.insert(relative, fs::read(path).unwrap());
        }
    }

    let mut snapshot = BTreeMap::new();
    visit(root, root, &mut snapshot);
    snapshot
}

fn signal_batch(context: &str) -> Value {
    let line = context
        .lines()
        .find_map(|line| line.strip_prefix("LWC_SIGNAL "))
        .expect("Hook context must contain an LWC_SIGNAL batch");
    serde_json::from_str(line).unwrap()
}

fn readiness(context: &str) -> Value {
    let line = context
        .lines()
        .find_map(|line| line.strip_prefix("LWC_READINESS "))
        .expect("Hook context must contain LWC_READINESS");
    serde_json::from_str(line).unwrap()
}

#[test]
fn npm_cmd_shim_passes_agent_install_preflight() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    let bin = home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&project).unwrap();
    fs::write(
        bin.join("lwc.cmd"),
        "@echo off\r\nif \"%1\"==\"serve\" if \"%2\"==\"--help\" echo Usage: lwc serve --mcp\r\nexit /b 0\r\n",
    )
    .unwrap();
    let path = env::join_paths(
        std::iter::once(bin.clone()).chain(env::split_paths(&env::var_os("PATH").unwrap())),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&project)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("PATH", path)
        .env("PATHEXT", ".COM;.EXE;.BAT;.CMD")
        .env_remove("CODEX_HOME")
        .args([
            "agent",
            "install",
            "--target",
            "codex",
            "--location",
            "global",
            "--yes",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["targets"][0]["status"], "installed");
    let config = fs::read_to_string(home.join(".codex/config.toml")).unwrap();
    assert!(config.contains("command = \"lwc\""));
    assert!(config.contains("args = [\"serve\", \"--mcp\"]"));
}

#[test]
fn canonical_project_root_hooks_read_the_active_plan_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project space 中文");
    let home = temp.path().join("home");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&home).unwrap();
    let canonical_project = project.canonicalize().unwrap();
    assert!(
        canonical_project.to_string_lossy().starts_with(r"\\?\"),
        "Windows canonicalize must exercise the verbatim-drive path: {canonical_project:?}"
    );

    project_json(&project, &canonical_project, &home, &["init"]);
    project_json(
        &project,
        &canonical_project,
        &home,
        &["config", "set", "--plan", "enabled"],
    );
    let created = project_json(
        &project,
        &canonical_project,
        &home,
        &[
            "plan",
            "create",
            "WINDOWS_VERBATIM_PLAN",
            "--objective",
            "continue safely",
            "--done-when",
            "the Hook sees this Plan",
            "--step",
            "verify the current step",
        ],
    );
    let plan_id = created["plan"]["id"].as_str().unwrap();
    let session_id = "WINDOWS_VERBATIM_SESSION";
    let discovery = project_hook(
        &project,
        &canonical_project,
        &home,
        "SessionStart",
        &serde_json::json!({
            "hook_event_name":"SessionStart",
            "source":"startup",
            "session_id":session_id,
        }),
    );
    let discovery_context = discovery
        .pointer("/hookSpecificOutput/additionalContext")
        .and_then(Value::as_str)
        .expect("SessionStart must expose the resolved Agent context");
    let context_id = readiness(discovery_context)["agent_context"]["context_id"]
        .as_str()
        .unwrap()
        .to_owned();
    project_json(
        &project,
        &canonical_project,
        &home,
        &["plan", "track", plan_id, "--context", &context_id],
    );
    let before = tree_snapshot(temp.path());

    let session = project_hook(
        &project,
        &canonical_project,
        &home,
        "SessionStart",
        &serde_json::json!({
            "hook_event_name":"SessionStart",
            "source":"startup",
            "session_id":session_id,
        }),
    );
    let session_context = session
        .pointer("/hookSpecificOutput/additionalContext")
        .and_then(Value::as_str)
        .expect("SessionStart must preserve the native Claude context envelope");
    let session_batch = signal_batch(session_context);
    assert_eq!(session_batch["event"], "session_start");
    assert_eq!(session_batch["signals"][0]["kind"], "plan.resume");
    assert_eq!(session_batch["signals"][0]["state"]["id"], plan_id);
    assert_eq!(tree_snapshot(temp.path()), before);

    let stop = project_hook(
        &project,
        &canonical_project,
        &home,
        "Stop",
        &serde_json::json!({
            "hook_event_name":"Stop",
            "stop_hook_active":false,
            "session_id":session_id,
        }),
    );
    assert_eq!(stop["decision"], "block", "{stop}");
    let stop_batch = signal_batch(stop["reason"].as_str().unwrap());
    assert_eq!(stop_batch["event"], "stop");
    assert_eq!(stop_batch["signals"][0]["kind"], "plan.continue");
    assert_eq!(stop_batch["signals"][0]["state"]["id"], plan_id);
    assert_eq!(tree_snapshot(temp.path()), before);
}
