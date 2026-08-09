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

    let streamed = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&project)
        .env("HOME", &home)
        .env("LWC_CODEGRAPH_BINARY", &fake)
        .args(["cg", "index"])
        .output()
        .unwrap();
    assert!(streamed.status.success());
    let json: Value = serde_json::from_slice(&streamed.stdout).unwrap();
    assert!(json["stdout"].as_str().unwrap().contains("|index "));
    assert!(String::from_utf8_lossy(&streamed.stderr).contains("|index "));
}
