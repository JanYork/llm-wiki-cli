#![cfg(windows)]

use serde_json::{Value, json};
use std::{fs, process::Command};

#[test]
fn cg_lifecycle_never_forwards_a_windows_verbatim_project_path() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    let fake = temp.path().join("codegraph.cmd");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::write(&fake, "@echo off\r\nexit /b 0\r\n").unwrap();

    let canonical = project.canonicalize().unwrap();
    assert!(
        canonical.to_string_lossy().starts_with(r"\\?\"),
        "Windows canonicalize should reproduce the reported verbatim path: {canonical:?}"
    );

    let initialized = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&project)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .arg("init")
        .output()
        .unwrap();
    assert!(initialized.status.success());

    let cases: &[(&[&str], Value)] = &[
        (&["cg", "init"], json!(["init", ".", "--force"])),
        (&["cg", "index"], json!(["index", ".", "--force"])),
        (&["cg", "sync"], json!(["sync", "."])),
        (&["cg", "unlock"], json!(["unlock", "."])),
        (&["cg", "uninit"], json!(["uninit", ".", "--force"])),
    ];

    for (args, expected) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&project)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("LWC_CODEGRAPH_BINARY", &fake)
            .args(*args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(&receipt["command"], expected, "wrong argv for {args:?}");
    }
}
