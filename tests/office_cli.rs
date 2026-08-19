use serde_json::Value;
use std::{fs, process::Command};

const OFFICECLI_VERSION: &str = "v1.0.144";

fn officecli_target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "mac-arm64",
        ("macos", "x86_64") => "mac-x64",
        ("linux", "aarch64") => "linux-arm64",
        ("linux", "x86_64") => "linux-x64",
        ("windows", "aarch64") => "win-arm64",
        ("windows", "x86_64") => "win-x64",
        pair => panic!("unsupported test platform {pair:?}"),
    }
}

fn officecli_asset() -> String {
    format!(
        "officecli-{}{}",
        officecli_target(),
        if cfg!(windows) { ".exe" } else { "" }
    )
}

fn officecli_sha256() -> &'static str {
    match officecli_target() {
        "mac-arm64" => "04757163428c5bde8d91e8f838517818e74722157722ca5f3877b6716b77bd45",
        "mac-x64" => "366100643d757b0da24829422897ca74768a894b5ecd1a471a1336f8e2a0787d",
        "linux-arm64" => "56ec2c3114b66f6490888b6778cbb8413a65911a26cacc7207f29e13424966da",
        "linux-x64" => "32ef7a21a54a4ca6c9806bf5e9f3d32bfb1291017329c55044cb2aac71822eb8",
        "win-arm64" => "0adb928d118e237b108077dadca9e272c236cd378c699712a41adda697047860",
        "win-x64" => "e780cc6a5385f84b4d54d71b0c179904ed534125ec33fe39b1a8711fa80e387e",
        target => panic!("unsupported OfficeCLI target {target}"),
    }
}

fn officecli_runtime(home: &std::path::Path) -> std::path::PathBuf {
    home.join(".lwc/runtime/officecli")
        .join(OFFICECLI_VERSION)
        .join(officecli_target())
}

#[test]
fn office_is_disabled_without_a_project_or_runtime_side_effects() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&cwd)
        .env("HOME", &home)
        .args(["office", "view", "report.docx", "text"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "office_disabled");
    assert_eq!(
        error["error"]["details"]["configure"],
        "lwc --scope global config set --office officecli"
    );
    assert!(!home.join(".lwc/runtime/officecli").exists());
    assert!(!cwd.join(".lwc").exists());
}

#[test]
fn office_configuration_is_global_only_and_does_not_download() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&home).unwrap();

    let init_global = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&project)
        .env("HOME", &home)
        .args(["--scope", "global", "init"])
        .output()
        .unwrap();
    assert!(init_global.status.success());

    let enabled = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&project)
        .env("HOME", &home)
        .args([
            "--scope",
            "global",
            "config",
            "set",
            "--office",
            "officecli",
        ])
        .output()
        .unwrap();
    assert!(
        enabled.status.success(),
        "{}",
        String::from_utf8_lossy(&enabled.stderr)
    );
    let config: Value = serde_json::from_slice(&enabled.stdout).unwrap();
    assert_eq!(config["office"]["setting"], "officecli");
    assert_eq!(config["office"]["origin"], "global");
    assert!(!home.join(".lwc/runtime/officecli").exists());

    let init_project = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&project)
        .env("HOME", &home)
        .arg("init")
        .output()
        .unwrap();
    assert!(init_project.status.success());
    let rejected = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&project)
        .env("HOME", &home)
        .args(["config", "set", "--office", "disabled"])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    let error: Value = serde_json::from_slice(&rejected.stderr).unwrap();
    assert_eq!(error["error"]["code"], "office_config_global_only");
}

#[test]
fn office_rejects_write_commands_before_installing_a_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(home.join(".lwc")).unwrap();
    fs::write(
        home.join(".lwc/config.json"),
        r#"{"version":4,"office":"officecli"}"#,
    )
    .unwrap();

    for verb in [
        "create",
        "set",
        "add",
        "remove",
        "move",
        "swap",
        "refresh",
        "batch",
        "merge",
        "raw-set",
        "add-part",
        "import",
        "save",
        "open",
        "close",
        "watch",
        "unwatch",
        "goto",
        "load_skill",
        "plugins",
        "mcp",
        "install",
        "config",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&cwd)
            .env("HOME", &home)
            .args(["office", verb])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{verb} unexpectedly succeeded");
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(
            error["error"]["code"], "office_command_not_read_only",
            "unexpected error for {verb}: {error}"
        );
    }
    assert!(!home.join(".lwc/runtime/officecli").exists());
}

#[cfg(unix)]
#[test]
fn office_forwards_read_commands_without_a_wiki_or_output_wrapping() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(home.join(".lwc")).unwrap();
    fs::write(
        home.join(".lwc/config.json"),
        r#"{"version":4,"office":"officecli"}"#,
    )
    .unwrap();
    let runtime = officecli_runtime(&home);
    fs::create_dir_all(&runtime).unwrap();
    let binary = runtime.join("officecli");
    fs::write(
        &binary,
        "#!/bin/sh\nprintf 'stdout:%s|%s|%s|%s\\n' \"$*\" \"$OFFICECLI_SKIP_UPDATE\" \"$OFFICECLI_NO_AUTO_RESIDENT\" \"$PWD\"\nprintf 'stderr:%s\\n' \"$1\" >&2\n[ \"$1\" != query ]\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&binary).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&binary, permissions).unwrap();
    fs::write(
        runtime.join("runtime.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": OFFICECLI_VERSION,
            "target": officecli_target(),
            "asset": officecli_asset(),
            "sha256": officecli_sha256(),
            "binary": "officecli",
        }))
        .unwrap(),
    )
    .unwrap();

    let success = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&cwd)
        .env("HOME", &home)
        .args(["office", "view", "report.docx", "text", "--max-lines", "5"])
        .output()
        .unwrap();
    assert!(success.status.success());
    assert_eq!(
        String::from_utf8(success.stdout).unwrap(),
        format!(
            "stdout:view report.docx text --max-lines 5|1|1|{}\n",
            cwd.canonicalize().unwrap().display()
        )
    );
    assert_eq!(String::from_utf8(success.stderr).unwrap(), "stderr:view\n");

    let failure = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&cwd)
        .env("HOME", &home)
        .args(["office", "query", "report.docx", "paragraph"])
        .output()
        .unwrap();
    assert_eq!(failure.status.code(), Some(1));
    assert!(
        String::from_utf8(failure.stdout)
            .unwrap()
            .starts_with("stdout:query ")
    );
    assert_eq!(String::from_utf8(failure.stderr).unwrap(), "stderr:query\n");
}

#[cfg(unix)]
#[test]
fn office_lazily_installs_once_and_reuses_the_global_runtime() {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    let fake_bin = temp.path().join("bin");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(home.join(".lwc")).unwrap();
    fs::create_dir_all(&fake_bin).unwrap();
    fs::write(
        home.join(".lwc/config.json"),
        r#"{"version":4,"office":"officecli"}"#,
    )
    .unwrap();

    let asset = temp.path().join(officecli_asset());
    fs::write(&asset, "#!/bin/sh\nprintf 'downloaded:%s\\n' \"$*\"\n").unwrap();
    let sha256 = Sha256::digest(fs::read(&asset).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let count = temp.path().join("downloads");
    let curl = fake_bin.join("curl");
    fs::write(
        &curl,
        "#!/bin/sh\nout=''\nwhile [ \"$#\" -gt 0 ]; do\n  [ \"$1\" = --output ] && { shift; out=$1; }\n  shift\ndone\ncp \"$LWC_TEST_OFFICECLI_ASSET\" \"$out\"\nprintf 'download\\n' >> \"$LWC_TEST_OFFICECLI_COUNT\"\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&curl).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&curl, permissions).unwrap();
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let run = || {
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&cwd)
            .env("HOME", &home)
            .env("PATH", &path)
            .env("LWC_TEST_OFFICECLI_RELEASE_ROOT", "https://example.invalid")
            .env("LWC_TEST_OFFICECLI_SHA256", &sha256)
            .env("LWC_TEST_OFFICECLI_ASSET", &asset)
            .env("LWC_TEST_OFFICECLI_COUNT", &count)
            .args(["office", "view", "report.docx", "text"])
            .output()
            .unwrap()
    };

    for _ in 0..2 {
        let output = run();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "downloaded:view report.docx text\n"
        );
    }
    assert_eq!(fs::read_to_string(&count).unwrap(), "download\n");
    assert!(officecli_runtime(&home).join("runtime.json").is_file());
    assert!(officecli_runtime(&home).join("officecli").is_file());

    let bad_home = temp.path().join("bad-home");
    fs::create_dir_all(bad_home.join(".lwc")).unwrap();
    fs::write(
        bad_home.join(".lwc/config.json"),
        r#"{"version":4,"office":"officecli"}"#,
    )
    .unwrap();
    let rejected = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&cwd)
        .env("HOME", &bad_home)
        .env("PATH", &path)
        .env("LWC_TEST_OFFICECLI_RELEASE_ROOT", "https://example.invalid")
        .env("LWC_TEST_OFFICECLI_SHA256", "0".repeat(64))
        .env("LWC_TEST_OFFICECLI_ASSET", &asset)
        .env("LWC_TEST_OFFICECLI_COUNT", &count)
        .args(["office", "view", "report.docx", "text"])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    let error: Value = serde_json::from_slice(&rejected.stderr).unwrap();
    assert_eq!(error["error"]["code"], "office_checksum_mismatch");
    assert!(!officecli_runtime(&bad_home).exists());
}
