#![cfg(windows)]

use serde_json::Value;
use std::{env, fs, process::Command};

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
