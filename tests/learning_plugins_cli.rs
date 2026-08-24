use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};

const PLUGINS: [&str; 3] = ["tutor", "book", "practice"];

fn target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        pair => panic!("unsupported test platform {pair:?}"),
    }
}

fn asset(plugin: &str) -> String {
    let extension = if cfg!(windows) { "zip" } else { "tar.gz" };
    format!(
        "lwc-{plugin}-{}-{}.{}",
        env!("CARGO_PKG_VERSION"),
        target(),
        extension
    )
}

fn runtime(home: &std::path::Path, plugin: &str) -> std::path::PathBuf {
    home.join(".lwc/runtime")
        .join(plugin)
        .join(env!("CARGO_PKG_VERSION"))
        .join(target())
}

fn run(cwd: &std::path::Path, home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(cwd)
        .env("HOME", home)
        .args(args)
        .output()
        .unwrap()
}

fn error(output: &std::process::Output) -> Value {
    assert!(!output.status.success());
    serde_json::from_slice(&output.stderr).unwrap_or_else(|parse| {
        panic!(
            "stderr was not a JSON error ({parse}): {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn learning_plugins_are_disabled_without_runtime_data_or_project_side_effects() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();

    for plugin in PLUGINS {
        let output = run(&cwd, &home, &[plugin, "status"]);
        let error = error(&output);
        assert_eq!(error["error"]["code"], format!("{plugin}_disabled"));
        assert_eq!(
            error["error"]["details"]["configure"],
            format!("lwc --scope global config set --{plugin} enabled")
        );
    }

    assert!(!home.join(".lwc/runtime").exists());
    assert!(!home.join(".lwc/plugins").exists());
    assert!(!cwd.join(".lwc").exists());
}

#[test]
fn learning_plugin_settings_are_global_only_independent_and_do_not_download() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("project");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();

    assert!(
        run(&cwd, &home, &["--scope", "global", "init"])
            .status
            .success()
    );
    let configured = run(
        &cwd,
        &home,
        &[
            "--scope",
            "global",
            "config",
            "set",
            "--tutor",
            "enabled",
            "--book",
            "disabled",
            "--practice",
            "enabled",
        ],
    );
    assert!(
        configured.status.success(),
        "{}",
        String::from_utf8_lossy(&configured.stderr)
    );
    let response: Value = serde_json::from_slice(&configured.stdout).unwrap();
    for (plugin, setting, origin) in [
        ("tutor", "enabled", "global"),
        ("book", "disabled", "built-in"),
        ("practice", "enabled", "global"),
    ] {
        assert_eq!(response[plugin]["setting"], setting);
        assert_eq!(response[plugin]["origin"], origin);
    }
    assert!(!home.join(".lwc/runtime").exists());
    assert!(!home.join(".lwc/plugins").exists());

    assert!(run(&cwd, &home, &["init"]).status.success());
    let rejected = run(&cwd, &home, &["config", "set", "--book", "enabled"]);
    assert_eq!(
        error(&rejected)["error"]["code"],
        "learning_plugin_config_global_only"
    );
}

#[cfg(unix)]
fn install_fake_runtime(home: &std::path::Path, plugin: &str) {
    use std::os::unix::fs::PermissionsExt;

    let directory = runtime(home, plugin);
    fs::create_dir_all(&directory).unwrap();
    let binary_name = format!("lwc-{plugin}");
    let binary = directory.join(&binary_name);
    fs::write(
        &binary,
        "#!/bin/sh\nprintf 'stdout:%s|%s|%s|%s\\n' \"$*\" \"$LWC_PLUGIN_SKIP_UPDATE\" \"$LWC_PLUGIN_NO_BACKGROUND\" \"$PWD\"\nprintf 'stderr:%s\\n' \"$1\" >&2\nread input\nprintf 'stdin:%s\\n' \"$input\"\n[ \"$1\" != fail ]\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&binary).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&binary, permissions).unwrap();
    let sha256 = Sha256::digest(fs::read(&binary).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    fs::write(
        directory.join("runtime.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "plugin": plugin,
            "version": env!("CARGO_PKG_VERSION"),
            "target": target(),
            "asset": asset(plugin),
            "sha256": sha256,
            "binary": binary_name,
        }))
        .unwrap(),
    )
    .unwrap();
}

#[cfg(unix)]
#[test]
fn learning_plugin_passthrough_preserves_args_cwd_stdin_stdout_stderr_and_exit() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(home.join(".lwc")).unwrap();
    fs::write(
        home.join(".lwc/config.json"),
        r#"{"version":7,"tutor":"enabled","book":"disabled","practice":"disabled"}"#,
    )
    .unwrap();
    install_fake_runtime(&home, "tutor");

    let mut child = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&cwd)
        .env("HOME", &home)
        .args(["tutor", "turn", "begin", "--json", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"visible learner input\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!(
            "stdout:turn begin --json -|1|1|{}\nstdin:visible learner input\n",
            cwd.canonicalize().unwrap().display()
        )
    );
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "stderr:turn\n");

    let failed = run(&cwd, &home, &["tutor", "fail"]);
    assert_eq!(failed.status.code(), Some(1));
    assert!(
        String::from_utf8(failed.stdout)
            .unwrap()
            .starts_with("stdout:fail")
    );
    assert_eq!(String::from_utf8(failed.stderr).unwrap(), "stderr:fail\n");
}

#[cfg(unix)]
#[test]
fn disabling_or_replacing_a_runtime_never_removes_plugin_data() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    assert!(
        run(&cwd, &home, &["--scope", "global", "init"])
            .status
            .success()
    );
    fs::create_dir_all(home.join(".lwc/plugins/book")).unwrap();
    fs::write(home.join(".lwc/plugins/book/data.sqlite3"), b"canonical").unwrap();
    fs::write(
        home.join(".lwc/config.json"),
        r#"{"version":7,"tutor":"disabled","book":"enabled","practice":"disabled"}"#,
    )
    .unwrap();
    install_fake_runtime(&home, "book");

    let disabled = run(
        &cwd,
        &home,
        &["--scope", "global", "config", "set", "--book", "disabled"],
    );
    assert!(disabled.status.success());
    assert_eq!(
        fs::read(home.join(".lwc/plugins/book/data.sqlite3")).unwrap(),
        b"canonical"
    );
}

#[test]
fn learning_plugin_commands_reject_changesets_before_runtime_installation() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();

    let rejected = run(&cwd, &home, &["--changeset", "draft", "practice", "status"]);
    assert_eq!(
        error(&rejected)["error"]["code"],
        "changeset_command_unsupported"
    );
    assert!(!home.join(".lwc/runtime/practice").exists());
}
