use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn view_help_exposes_loopback_read_only_controls() {
    let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .args(["view", "--help"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--port"));
    assert!(help.contains("--no-open"));
    assert!(help.contains("127.0.0.1"));
    assert!(help.contains("read-only"));
}

#[test]
fn view_serves_get_only_without_mutating_the_project() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&home).unwrap();
    let init = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&project)
        .env("HOME", &home)
        .arg("init")
        .output()
        .unwrap();
    assert!(init.status.success());
    let before = snapshot(&project.join(".lwc"));

    let mut child = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&project)
        .env("HOME", &home)
        .args(["view", "--port", "0", "--no-open"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let startup: Value = serde_json::from_str(&line).unwrap();
    let address = startup["address"].as_str().unwrap();

    let index = request(address, "GET", "/");
    assert!(index.starts_with("HTTP/1.1 200"));
    assert!(index.contains("/assets/app.js"));
    assert!(!index.contains("unsafe-inline"));
    let script = request(address, "GET", "/assets/app.js");
    assert!(script.starts_with("HTTP/1.1 200"));
    assert!(script.contains("content-type: application/javascript"));
    assert!(request(address, "GET", "/api/status").starts_with("HTTP/1.1 200"));
    assert!(request(address, "POST", "/api/status").starts_with("HTTP/1.1 405"));
    child.kill().unwrap();
    child.wait().unwrap();

    assert_eq!(snapshot(&project.join(".lwc")), before);
}

fn request(address: &str, method: &str, path: &str) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn snapshot(root: &Path) -> BTreeMap<String, (usize, String)> {
    fn visit(root: &Path, current: &Path, files: &mut BTreeMap<String, (usize, String)>) {
        for entry in fs::read_dir(current).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, files);
            } else if path.extension().and_then(|value| value.to_str()) == Some("db-shm") {
                // SQLite updates shared-memory read-lock bytes even for read-only connections.
            } else {
                let content = fs::read(&path).unwrap();
                let digest = Sha256::digest(&content)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                files.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    (content.len(), digest),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}
