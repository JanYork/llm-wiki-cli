#![cfg(windows)]

use serde_json::Value;
use std::process::{Command, Output};
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

struct TransWorld {
    _temp: TempDir,
    project: PathBuf,
    home: PathBuf,
    bin: PathBuf,
}

impl TransWorld {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let home = temp.path().join("home");
        let bin = temp.path().join("bin");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&bin).unwrap();
        Self {
            _temp: temp,
            project,
            home,
            bin,
        }
    }

    fn init(&self) {
        let output = self.command(&["init"]);
        assert!(
            output.status.success(),
            "init failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn command(&self, args: &[&str]) -> Output {
        let system32 = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
        let path = std::env::join_paths([&self.bin, &system32]).unwrap();
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("PATH", path)
            .args(args)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> Value {
        let output = self.command(args);
        assert!(
            output.status.success(),
            "command {args:?} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn err(&self, args: &[&str]) -> Value {
        let output = self.command(args);
        assert!(
            !output.status.success(),
            "command {args:?} unexpectedly succeeded\nstdout: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice(&output.stderr).unwrap()
    }

    fn write_project(&self, relative: &str, content: &str) -> PathBuf {
        self.write_path(&self.project.join(relative), content.as_bytes())
    }

    fn write_path(&self, path: &Path, content: &[u8]) -> PathBuf {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
        path.to_path_buf()
    }

    fn install_cmd_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.bin.join(format!("{name}.cmd"));
        fs::write(&path, body).unwrap();
        path
    }
}

fn as_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn trans_kills_windows_descendants_on_timeout() {
    let world = TransWorld::new();
    world.init();
    let marker = world.bin.join("markitdown.desc.survived");
    world.install_cmd_script(
        "markitdown",
        &format!(
            "@echo off\r\n\
setlocal\r\n\
start \"\" /B cmd /c \"ping -n 4 127.0.0.1 >nul && echo survived>\\\"{}\\\"\"\r\n\
ping -n 9 127.0.0.1 >nul\r\n",
            marker.display()
        ),
    );
    world.ok(&[
        "config",
        "set",
        "--trans",
        "markitdown",
        "--trans-timeout",
        "1",
    ]);
    let input = world.write_project("docs/source.pdf", "stub");
    let output = world.project.join("out/descendant.md");

    let started = Instant::now();
    let error = world.err(&["trans", as_str(&input), "--output", as_str(&output)]);
    assert_eq!(error["error"]["code"], "trans_timeout");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!output.exists());
    thread::sleep(Duration::from_secs(4));
    assert!(
        !marker.exists(),
        "descendant should not survive to write its marker"
    );
}
