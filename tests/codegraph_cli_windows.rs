#![cfg(windows)]

use std::{fs, process::Command};

#[test]
fn cg_lifecycle_never_forwards_a_windows_verbatim_project_path() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    let fake = temp.path().join("codegraph.cmd");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::write(&fake, "@echo off\r\necho %*\r\nexit /b 0\r\n").unwrap();

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

    let cases: &[(&[&str], &str)] = &[
        (&["cg", "init"], "init . --force"),
        (&["cg", "index"], "index . --force"),
        (&["cg", "sync"], "sync ."),
        (&["cg", "unlock"], "unlock ."),
        (&["cg", "uninit"], "uninit . --force"),
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
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim_end(),
            *expected,
            "wrong argv for {args:?}"
        );
    }
    fs::write(
        &fake,
        "@echo off\r\necho raw\r\necho diagnostic 1>&2\r\nexit /b 23\r\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&project)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("LWC_CODEGRAPH_BINARY", &fake)
        .args(["cg", "query", "Symbol"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(23));
    let native = Command::new(&fake)
        .current_dir(&project)
        .args(["query", "Symbol"])
        .output()
        .unwrap();
    assert_eq!(output.stdout, native.stdout);
    assert_eq!(output.stderr, native.stderr);
}
