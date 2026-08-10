use serde_json::Value;
use std::process::Command;

#[test]
fn cg_status_is_project_local_and_does_not_download() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&home).unwrap();

    let init = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&project)
        .env("HOME", &home)
        .arg("init")
        .output()
        .unwrap();
    assert!(init.status.success());

    let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&project)
        .env("HOME", &home)
        .args(["cg", "status"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["installed"], false);
    assert_eq!(json["initialized"], false);
    assert!(!project.join(".lwc/runtime/codegraph").exists());
    assert!(!project.join(".codegraph").exists());
}

#[cfg(unix)]
#[test]
fn cg_forwards_only_project_local_state_with_telemetry_disabled() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    let fake = temp.path().join("codegraph");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        &fake,
        "#!/bin/sh\nprintf '%s\\n' \"$PWD|$CODEGRAPH_DIR|$DO_NOT_TRACK|$HOME|$*\"\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&fake, permissions).unwrap();

    assert!(
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&project)
            .env("HOME", &home)
            .arg("init")
            .status()
            .unwrap()
            .success()
    );
    let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&project)
        .env("HOME", &home)
        .env("LWC_CODEGRAPH_BINARY", &fake)
        .args(["cg", "query", "Widget"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    let line = json["stdout"].as_str().unwrap();
    assert!(line.contains("/project|.lwc/codegraph|1|"), "{line}");
    assert!(line.contains("|.lwc/codegraph|1|"));
    assert!(line.ends_with("|query Widget"));
    assert!(!project.join(".codegraph").exists());
    assert!(!home.join(".codegraph").exists());

    let help = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&project)
        .env("HOME", &home)
        .env("LWC_CODEGRAPH_BINARY", &fake)
        .args(["cg", "help", "query"])
        .output()
        .unwrap();
    assert!(
        help.status.success(),
        "{}",
        String::from_utf8_lossy(&help.stderr)
    );
    let json: Value = serde_json::from_slice(&help.stdout).unwrap();
    assert!(json["stdout"].as_str().unwrap().ends_with("|help query"));

    let streamed = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&project)
        .env("HOME", &home)
        .env("LWC_CODEGRAPH_BINARY", &fake)
        .args(["cg", "index"])
        .output()
        .unwrap();
    assert!(streamed.status.success());
    let json: Value = serde_json::from_slice(&streamed.stdout).unwrap();
    let expected = format!(
        "|index {} --force",
        project.canonicalize().unwrap().display()
    );
    let line = json["stdout"].as_str().unwrap();
    assert!(
        line.ends_with(&expected),
        "expected {expected:?}, got {line:?}"
    );
    assert!(String::from_utf8_lossy(&streamed.stderr).contains(&expected));

    let uninit = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&project)
        .env("HOME", &home)
        .env("LWC_CODEGRAPH_BINARY", &fake)
        .args(["cg", "uninit"])
        .output()
        .unwrap();
    assert!(uninit.status.success());
    let json: Value = serde_json::from_slice(&uninit.stdout).unwrap();
    let expected = format!(
        "|uninit {} --force",
        project.canonicalize().unwrap().display()
    );
    assert!(json["stdout"].as_str().unwrap().ends_with(&expected));
}

#[cfg(unix)]
#[test]
fn cg_rejects_global_lifecycle_and_external_paths() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    let fake = temp.path().join("codegraph");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(&fake, "#!/bin/sh\nprintf '%s\\n' \"$*\"\n").unwrap();
    let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&fake, permissions).unwrap();
    assert!(
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&project)
            .env("HOME", &home)
            .arg("init")
            .status()
            .unwrap()
            .success()
    );

    for args in [
        vec!["cg", "query", "Widget", "--path", "/tmp"],
        vec!["cg", "node", "--file", "/etc/hosts"],
        vec!["cg", "affected", "/etc/hosts"],
        vec!["cg", "affected", "--stdin"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&project)
            .env("HOME", &home)
            .env("LWC_CODEGRAPH_BINARY", &fake)
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let json: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(json["error"]["code"], "codegraph_external_path_forbidden");
    }

    let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&project)
        .env("HOME", &home)
        .env("LWC_CODEGRAPH_BINARY", &fake)
        .args(["cg", "daemons"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(
        json["error"]["code"],
        "codegraph_command_not_project_scoped"
    );
}
