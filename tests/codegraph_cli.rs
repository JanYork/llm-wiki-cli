use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const CODEGRAPH_VERSION: &str = "v1.5.0-lwc.1";

fn codegraph_target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-x64",
        ("linux", "aarch64") => "linux-arm64",
        ("linux", "x86_64") => "linux-x64",
        ("windows", "aarch64") => "win32-arm64",
        ("windows", "x86_64") => "win32-x64",
        pair => panic!("unsupported test platform {pair:?}"),
    }
}

fn codegraph_runtime(home: &Path) -> PathBuf {
    home.join(".lwc")
        .join("runtime")
        .join("codegraph")
        .join(CODEGRAPH_VERSION)
        .join(codegraph_target())
}

#[test]
fn cg_status_reports_global_versioned_runtime_without_downloading() {
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
    let runtime = codegraph_runtime(&home);
    assert_eq!(json["runtime"], runtime.to_string_lossy().as_ref());
    assert!(!home.join(".lwc/runtime/codegraph").exists());
    assert!(!project.join(".lwc/runtime/codegraph").exists());
    assert!(!project.join(".codegraph").exists());
}

#[test]
fn cg_status_requires_a_matching_runtime_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    assert!(
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&project)
            .env("HOME", &home)
            .arg("init")
            .status()
            .unwrap()
            .success()
    );

    let runtime = codegraph_runtime(&home);
    let binary = runtime.join(if cfg!(windows) {
        "bin/codegraph.cmd"
    } else {
        "bin/codegraph"
    });
    std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
    std::fs::write(&binary, b"fake").unwrap();

    let status = cg_status(&project, &home);
    assert_eq!(status["installed"], false);
    assert_eq!(status["runtime_health"], "invalid");

    let asset = format!(
        "codegraph-{}.{}",
        codegraph_target(),
        if cfg!(windows) { "zip" } else { "tar.gz" }
    );
    std::fs::write(
        runtime.join("runtime.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": CODEGRAPH_VERSION,
            "target": codegraph_target(),
            "asset": asset,
            "archive_sha256": "0".repeat(64),
            "binary": binary.strip_prefix(&runtime).unwrap(),
        }))
        .unwrap(),
    )
    .unwrap();

    let status = cg_status(&project, &home);
    assert_eq!(status["installed"], true);
    assert_eq!(status["runtime_health"], "ready");
}

#[test]
fn cg_status_reports_but_does_not_remove_a_legacy_project_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    assert!(
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&project)
            .env("HOME", &home)
            .arg("init")
            .status()
            .unwrap()
            .success()
    );
    let legacy = project.join(".lwc/runtime/codegraph");
    std::fs::create_dir_all(legacy.join("bin")).unwrap();
    std::fs::write(legacy.join("bin/codegraph"), b"legacy").unwrap();

    let status = cg_status(&project, &home);
    assert_eq!(status["legacy_project_runtime"], true);
    assert_eq!(
        Path::new(status["legacy_runtime"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        legacy.canonicalize().unwrap()
    );
    assert!(legacy.join("bin/codegraph").is_file());
}

fn cg_status(project: &Path, home: &Path) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(project)
        .env("HOME", home)
        .args(["cg", "status"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[cfg(unix)]
struct FakeRelease {
    asset: PathBuf,
    sums: PathBuf,
    count: PathBuf,
    path: String,
}

#[cfg(unix)]
fn fake_release(root: &Path) -> FakeRelease {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;

    let fixture = root.join("release-fixture");
    let package_bin = fixture.join("package/bin");
    let fake_bin = root.join("release-bin");
    std::fs::create_dir_all(&package_bin).unwrap();
    std::fs::create_dir_all(&fake_bin).unwrap();
    let executable = package_bin.join("codegraph");
    std::fs::write(
        &executable,
        "#!/bin/sh\nmkdir -p \"$CODEGRAPH_DIR\"\ntouch \"$CODEGRAPH_DIR/codegraph.db\"\nprintf 'ok\\n'\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();

    let asset_name = format!("codegraph-{}.tar.gz", codegraph_target());
    let asset = root.join(&asset_name);
    assert!(
        Command::new("tar")
            .args(["-czf"])
            .arg(&asset)
            .arg("-C")
            .arg(&fixture)
            .arg("package")
            .status()
            .unwrap()
            .success()
    );
    let digest = Sha256::digest(std::fs::read(&asset).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let sums = root.join("release-SHA256SUMS");
    std::fs::write(&sums, format!("{digest}  {asset_name}\n")).unwrap();
    let count = root.join("release-download-count");
    let curl = fake_bin.join("curl");
    std::fs::write(
        &curl,
        "#!/bin/sh\nout=''\nurl=''\nwhile [ \"$#\" -gt 0 ]; do\n  case \"$1\" in\n    --output) shift; out=$1 ;;\n    http*) url=$1 ;;\n  esac\n  shift\ndone\ncase \"$url\" in\n  *SHA256SUMS) cp \"$FAKE_CODEGRAPH_SUMS\" \"$out\" ;;\n  *) cp \"$FAKE_CODEGRAPH_ASSET\" \"$out\" ;;\nesac\nprintf 'download\\n' >> \"$FAKE_CODEGRAPH_COUNT\"\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&curl).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&curl, permissions).unwrap();
    FakeRelease {
        asset,
        sums,
        count,
        path: format!(
            "{}:{}",
            fake_bin.display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    }
}

#[cfg(unix)]
fn configure_fake_release(command: &mut Command, release: &FakeRelease) {
    command
        .env("PATH", &release.path)
        .env("FAKE_CODEGRAPH_ASSET", &release.asset)
        .env("FAKE_CODEGRAPH_SUMS", &release.sums)
        .env("FAKE_CODEGRAPH_COUNT", &release.count);
}

#[cfg(unix)]
#[test]
fn invalid_global_runtime_is_quarantined_before_reinstall() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    assert!(
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&project)
            .env("HOME", &home)
            .arg("init")
            .status()
            .unwrap()
            .success()
    );
    let runtime = codegraph_runtime(&home);
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::write(runtime.join("broken.txt"), b"preserve me").unwrap();
    let release = fake_release(temp.path());
    let mut command = Command::new(env!("CARGO_BIN_EXE_lwc"));
    command
        .current_dir(&project)
        .env("HOME", &home)
        .args(["cg", "init"]);
    configure_fake_release(&mut command, &release);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(cg_status(&project, &home)["runtime_health"], "ready");
    let prefix = format!("{}.invalid-", codegraph_target());
    let quarantined = runtime
        .parent()
        .unwrap()
        .read_dir()
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .expect("invalid runtime should be quarantined");
    assert_eq!(
        std::fs::read(quarantined.join("broken.txt")).unwrap(),
        b"preserve me"
    );
}

#[cfg(unix)]
#[test]
fn concurrent_projects_publish_one_global_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project_a = temp.path().join("project-a");
    let project_b = temp.path().join("project-b");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&project_a).unwrap();
    std::fs::create_dir_all(&project_b).unwrap();

    for project in [&project_a, &project_b] {
        assert!(
            Command::new(env!("CARGO_BIN_EXE_lwc"))
                .current_dir(project)
                .env("HOME", &home)
                .arg("init")
                .status()
                .unwrap()
                .success()
        );
    }

    let release = fake_release(temp.path());

    let spawn = |project: &Path| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lwc"));
        command
            .current_dir(project)
            .env("HOME", &home)
            .args(["cg", "init"]);
        configure_fake_release(&mut command, &release);
        command.spawn().unwrap()
    };
    let first = spawn(&project_a);
    let second = spawn(&project_b);
    let first = first.wait_with_output().unwrap();
    let second = second.wait_with_output().unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&release.count)
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert!(project_a.join(".lwc/codegraph/codegraph.db").is_file());
    assert!(project_b.join(".lwc/codegraph/codegraph.db").is_file());
    assert_eq!(cg_status(&project_a, &home)["runtime_health"], "ready");
    assert_eq!(cg_status(&project_b, &home)["runtime_health"], "ready");
}

#[cfg(unix)]
#[test]
fn cg_serve_mcp_is_a_raw_project_scoped_stdio_bridge() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    let fake = temp.path().join("codegraph");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        &fake,
        "#!/bin/sh\nprintf '%s|%s|%s\\n' \"$PWD\" \"$CODEGRAPH_DIR\" \"$*\"\n",
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
        .args(["cg", "serve", "--mcp"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let expected = format!(
        "{}|.lwc/codegraph|serve --mcp\n",
        project.canonicalize().unwrap().display()
    );
    assert_eq!(stdout, expected);
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

    let initialized = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&project)
        .env("HOME", &home)
        .env("LWC_CODEGRAPH_BINARY", &fake)
        .args(["cg", "init"])
        .output()
        .unwrap();
    assert!(initialized.status.success());
    let json: Value = serde_json::from_slice(&initialized.stdout).unwrap();
    assert!(
        json["stdout"]
            .as_str()
            .unwrap()
            .ends_with("|init . --force")
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
    let expected = "|index . --force";
    let line = json["stdout"].as_str().unwrap();
    assert!(
        line.ends_with(&expected),
        "expected {expected:?}, got {line:?}"
    );
    assert!(String::from_utf8_lossy(&streamed.stderr).contains(expected));

    let uninit = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&project)
        .env("HOME", &home)
        .env("LWC_CODEGRAPH_BINARY", &fake)
        .args(["cg", "uninit"])
        .output()
        .unwrap();
    assert!(uninit.status.success());
    let json: Value = serde_json::from_slice(&uninit.stdout).unwrap();
    let expected = "|uninit . --force";
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

    for args in [
        vec!["cg", "daemons"],
        vec!["cg", "serve"],
        vec!["cg", "serve", "--mcp", "unexpected"],
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
        assert_eq!(
            json["error"]["code"],
            "codegraph_command_not_project_scoped"
        );
    }
}
