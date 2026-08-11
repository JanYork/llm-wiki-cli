#![cfg(unix)]

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Output, Stdio};
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
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("PATH", &self.bin)
            .args(args)
            .output()
            .unwrap()
    }

    fn spawn(&self, args: &[&str]) -> Child {
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("PATH", &self.bin)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
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

    fn install_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.bin.join(name);
        fs::write(&path, body).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }
}

fn as_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn output_sha256(content: &[u8]) -> String {
    let digest = Sha256::digest(content);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn args_log(script: &Path) -> Vec<String> {
    let log = PathBuf::from(format!("{}.args", script.display()));
    fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(ToOwned::to_owned)
        .collect()
}

fn success_script() -> &'static str {
    r#"#!/bin/sh
set -eu
log="$0.args"
: > "$log"
for arg in "$@"; do
  printf '%s\n' "$arg" >> "$log"
done
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then
    out="$arg"
    break
  fi
  prev="$arg"
done
[ -n "$out" ]
printf '# converted by %s\n' "${0##*/}" > "$out"
"#
}

#[test]
fn trans_uses_the_resolved_engine_literal_argv_and_returns_a_receipt() {
    for (engine, args) in [
        ("anydoc", vec!["--format", "pdf"]),
        ("markitdown", vec!["--use-plugins"]),
    ] {
        let world = TransWorld::new();
        world.init();
        let selected = world.install_script(engine, success_script());
        let other = world.install_script(
            if engine == "anydoc" {
                "markitdown"
            } else {
                "anydoc"
            },
            "#!/bin/sh\nexit 97\n",
        );
        let input = world.write_project("docs/source.docx", "stub");
        let output = world.project.join("out/converted.md");
        let mut config_args = vec!["config", "set", "--trans", engine];
        for value in &args {
            config_args.push("--trans-arg");
            config_args.push(value);
        }
        world.ok(&config_args);

        let receipt = world.ok(&["trans", as_str(&input), "--output", as_str(&output)]);

        let final_content = fs::read(&output).unwrap();
        let logged = args_log(&selected);
        let temp_output = PathBuf::from(logged.last().unwrap());
        let expected_work_root = world
            .project
            .join(".lwc/work/trans")
            .canonicalize()
            .unwrap();
        assert_eq!(&logged[..args.len()], args);
        assert_eq!(
            logged[args.len()],
            input.canonicalize().unwrap().display().to_string()
        );
        assert_eq!(logged[args.len() + 1], "-o");
        assert_ne!(temp_output, output);
        assert_eq!(
            temp_output.parent().unwrap().canonicalize().unwrap(),
            expected_work_root
        );
        assert!(!temp_output.exists(), "temporary output should be cleaned");
        assert!(!PathBuf::from(format!("{}.args", other.display())).exists());

        assert_eq!(receipt["engine"], engine);
        assert_eq!(
            receipt["input_path"],
            input.canonicalize().unwrap().display().to_string()
        );
        assert_eq!(
            receipt["output_path"],
            output.canonicalize().unwrap().display().to_string()
        );
        assert_eq!(receipt["output_bytes"], final_content.len() as u64);
        assert_eq!(receipt["output_sha256"], output_sha256(&final_content));
        assert_eq!(
            String::from_utf8(final_content).unwrap(),
            format!("# converted by {engine}\n")
        );
    }
}

#[test]
fn trans_uses_the_scope_specific_private_work_root() {
    let world = TransWorld::new();
    world.init();
    world.ok(&["--scope", "global", "init"]);
    let selected = world.install_script("anydoc", success_script());
    let input = world.write_project("docs/source.docx", "stub");
    let output = world.project.join("out/global.md");

    world.ok(&[
        "--scope",
        "global",
        "config",
        "set",
        "--trans",
        "anydoc",
        "--trans-arg",
        "--format",
        "--trans-arg",
        "docx",
    ]);
    world.ok(&[
        "--scope",
        "global",
        "trans",
        as_str(&input),
        "--output",
        as_str(&output),
    ]);

    let logged = args_log(&selected);
    let temp_output = PathBuf::from(logged.last().unwrap());
    assert!(
        temp_output.starts_with(world.home.join(".lwc/work/trans")),
        "{temp_output:?}"
    );
    assert!(!temp_output.exists(), "temporary output should be cleaned");
}

#[test]
fn trans_requires_an_enabled_engine_and_a_present_executable() {
    let world = TransWorld::new();
    world.init();
    let input = world.write_project("docs/source.docx", "stub");
    let output = world.project.join("out/converted.md");

    let disabled = world.err(&["trans", as_str(&input), "--output", as_str(&output)]);
    assert_eq!(disabled["error"]["code"], "trans_disabled");

    world.ok(&["config", "set", "--trans", "anydoc"]);
    let missing = world.err(&["trans", as_str(&input), "--output", as_str(&output)]);
    assert_eq!(missing["error"]["code"], "trans_executable_missing");
}

#[test]
fn trans_reuses_existing_source_authorization_and_rejects_unsafe_input_or_output() {
    let world = TransWorld::new();
    world.init();
    world.install_script("anydoc", success_script());
    world.ok(&["config", "set", "--trans", "anydoc"]);

    let external = world.home.join("outside.docx");
    world.write_path(&external, b"stub");
    let blocked = world.err(&[
        "trans",
        as_str(&external),
        "--output",
        as_str(&world.project.join("out/external.md")),
    ]);
    assert_eq!(
        blocked["error"]["code"],
        "external_source_requires_acknowledgement"
    );

    let allowed = world.ok(&[
        "trans",
        as_str(&external),
        "--output",
        as_str(&world.project.join("out/external.md")),
        "--allow-external-source",
    ]);
    assert_eq!(allowed["engine"], "anydoc");

    let directory = world.project.join("docs");
    fs::create_dir_all(&directory).unwrap();
    let unsafe_input = world.err(&[
        "trans",
        as_str(&directory),
        "--output",
        as_str(&world.project.join("out/from-dir.md")),
    ]);
    assert_eq!(unsafe_input["error"]["code"], "trans_unsafe_input");

    let input = world.write_project("docs/source.docx", "stub");
    let existing = world.write_project("out/existing.md", "already here");
    let unsafe_output = world.err(&["trans", as_str(&input), "--output", as_str(&existing)]);
    assert_eq!(unsafe_output["error"]["code"], "trans_unsafe_output");

    let same_path = world.err(&["trans", as_str(&input), "--output", as_str(&input)]);
    assert_eq!(same_path["error"]["code"], "trans_unsafe_output");
}

#[test]
fn trans_rejects_adapter_args_that_override_io_or_add_extra_inputs() {
    let world = TransWorld::new();
    world.init();
    let anydoc = world.install_script("anydoc", success_script());
    let input = world.write_project("docs/source.docx", "stub");
    let output = world.project.join("out/converted.md");

    for args in [
        vec![
            "config",
            "set",
            "--trans",
            "anydoc",
            "--trans-arg",
            "-o",
            "--trans-arg",
            "evil.md",
        ],
        vec![
            "config",
            "set",
            "--trans",
            "anydoc",
            "--trans-arg",
            "--format",
            "--trans-arg",
            "pdf",
            "--trans-arg",
            "rogue-input.docx",
        ],
        vec![
            "config",
            "set",
            "--trans",
            "markitdown",
            "--trans-arg",
            "--use-plugins",
            "--trans-arg",
            "rogue-input.docx",
        ],
        vec![
            "config",
            "set",
            "--trans",
            "markitdown",
            "--trans-arg",
            "--output=evil.md",
        ],
    ] {
        world.ok(&args);
        let error = world.err(&["trans", as_str(&input), "--output", as_str(&output)]);
        assert_eq!(error["error"]["code"], "trans_unsafe_args");
        assert!(!PathBuf::from(format!("{}.args", anydoc.display())).exists());
    }
}

#[test]
fn trans_allows_documented_markitdown_flags_as_literal_passthrough() {
    let world = TransWorld::new();
    world.init();
    let selected = world.install_script("markitdown", success_script());
    let input = world.write_project("docs/source.pdf", "stub");
    let output = world.project.join("out/markitdown.md");
    let args = vec![
        "--use-cu",
        "--cu-endpoint",
        "https://example.invalid/cu",
        "--cu-analyzer",
        "invoice",
        "--cu-file-types",
        "pdf,jpeg",
        "-x",
        "pdf",
        "-m",
        "application/pdf",
        "-c",
        "utf-8",
        "-p",
        "--use-plugins",
        "--keep-data-uris",
    ];
    let mut config_args = vec!["config", "set", "--trans", "markitdown"];
    for value in &args {
        config_args.push("--trans-arg");
        config_args.push(value);
    }
    world.ok(&config_args);

    world.ok(&["trans", as_str(&input), "--output", as_str(&output)]);

    let logged = args_log(&selected);
    assert_eq!(&logged[..args.len()], args);
}

#[test]
fn trans_allows_documented_markitdown_docintel_flags() {
    let world = TransWorld::new();
    world.init();
    let selected = world.install_script("markitdown", success_script());
    let input = world.write_project("docs/source.pdf", "stub");
    let output = world.project.join("out/docintel.md");
    let args = vec!["-d", "-e", "https://example.invalid/docintel"];
    let mut config_args = vec!["config", "set", "--trans", "markitdown"];
    for value in &args {
        config_args.push("--trans-arg");
        config_args.push(value);
    }
    world.ok(&config_args);

    world.ok(&["trans", as_str(&input), "--output", as_str(&output)]);

    let logged = args_log(&selected);
    assert_eq!(&logged[..args.len()], args);
}

#[test]
fn trans_times_out_kills_the_child_and_leaves_no_destination() {
    let world = TransWorld::new();
    world.init();
    world.install_script(
        "markitdown",
        r#"#!/bin/sh
set -eu
/bin/sleep 5
"#,
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
    let output = world.project.join("out/timed.md");

    let error = world.err(&["trans", as_str(&input), "--output", as_str(&output)]);
    assert_eq!(error["error"]["code"], "trans_timeout");
    assert!(!output.exists());
}

#[test]
fn trans_errors_never_echo_raw_stderr_secrets() {
    let world = TransWorld::new();
    world.init();
    let input = world.write_project("docs/source.pdf", "stub");
    let output = world.project.join("out/secret.md");
    let secret = "Authorization: Bearer super-secret-token";

    world.install_script(
        "markitdown",
        &format!(
            r#"#!/bin/sh
set -eu
printf '%s\n' '{}' >&2
exit 7
"#,
            secret
        ),
    );
    world.ok(&["config", "set", "--trans", "markitdown"]);
    let failed = world.err(&["trans", as_str(&input), "--output", as_str(&output)]);
    let failed_json = serde_json::to_string(&failed).unwrap();
    assert_eq!(failed["error"]["code"], "trans_failed");
    assert!(!failed_json.contains(secret), "{failed_json}");
    assert_eq!(failed["error"]["details"]["stderr_present"], true);
    assert_eq!(failed["error"]["details"]["stderr_truncated"], false);
    assert!(failed["error"]["details"].get("stderr_preview").is_none());

    world.install_script(
        "markitdown",
        &format!(
            r#"#!/bin/sh
set -eu
printf '%s\n' '{}' >&2
/bin/sleep 5
"#,
            secret
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
    let timed_out = world.err(&["trans", as_str(&input), "--output", as_str(&output)]);
    let timeout_json = serde_json::to_string(&timed_out).unwrap();
    assert_eq!(timed_out["error"]["code"], "trans_timeout");
    assert!(!timeout_json.contains(secret), "{timeout_json}");
    assert_eq!(timed_out["error"]["details"]["stderr_truncated"], false);
    assert!(
        timed_out["error"]["details"]
            .get("stderr_preview")
            .is_none()
    );
}

#[test]
fn trans_kills_descendants_that_keep_stderr_open() {
    let world = TransWorld::new();
    world.init();
    let marker = world.bin.join("markitdown.desc.survived");
    world.install_script(
        "markitdown",
        &format!(
            r#"#!/bin/sh
set -eu
(
  /bin/sleep 2
  echo survived > "{}"
) &
exec /bin/sleep 8
"#,
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
    thread::sleep(Duration::from_millis(2500));
    assert!(
        !marker.exists(),
        "descendant should not survive to write its marker"
    );
}

#[test]
fn trans_classifies_nonzero_empty_oversized_and_invalid_utf8_outputs() {
    let world = TransWorld::new();
    world.init();
    let input = world.write_project("docs/source.docx", "stub");
    let output = world.project.join("out/result.md");

    for (name, script, code) in [
        (
            "anydoc",
            r#"#!/bin/sh
set -eu
i=0
while [ "$i" -lt 20000 ]; do
  printf 'E' >&2
  i=$((i + 1))
done
exit 7
"#,
            "trans_failed",
        ),
        (
            "markitdown",
            r#"#!/bin/sh
set -eu
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then
    out="$arg"
    break
  fi
  prev="$arg"
done
: > "$out"
"#,
            "trans_empty_output",
        ),
    ] {
        world.install_script(name, script);
        world.ok(&["config", "set", "--trans", name]);
        let error = world.err(&["trans", as_str(&input), "--output", as_str(&output)]);
        assert_eq!(error["error"]["code"], code);
        assert!(!output.exists());
    }

    world.install_script(
        "markitdown",
        r#"#!/bin/sh
set -eu
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then
    out="$arg"
    break
  fi
  prev="$arg"
done
/bin/dd if=/dev/zero of="$out" bs=1 count=0 seek=67108865 2>/dev/null
"#,
    );
    world.ok(&["config", "set", "--trans", "markitdown"]);
    let oversized = world.err(&["trans", as_str(&input), "--output", as_str(&output)]);
    assert_eq!(oversized["error"]["code"], "trans_output_too_large");
    assert!(!output.exists());

    world.install_script(
        "markitdown",
        r#"#!/bin/sh
set -eu
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then
    out="$arg"
    break
  fi
  prev="$arg"
done
printf '\377' > "$out"
"#,
    );
    world.ok(&["config", "set", "--trans", "markitdown"]);
    let invalid_utf8 = world.err(&["trans", as_str(&input), "--output", as_str(&output)]);
    assert_eq!(invalid_utf8["error"]["code"], "trans_invalid_utf8");
    assert!(!output.exists());
}

#[test]
fn trans_rejects_non_regular_engine_output_without_blocking() {
    let world = TransWorld::new();
    world.init();
    world.install_script(
        "markitdown",
        r#"#!/bin/sh
set -eu
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then
    out="$arg"
    break
  fi
  prev="$arg"
done
/bin/rm -f "$out"
/usr/bin/mkfifo "$out"
"#,
    );
    world.ok(&["config", "set", "--trans", "markitdown"]);
    let input = world.write_project("docs/source.pdf", "stub");
    let output = world.project.join("out/fifo.md");

    let error = world.err(&["trans", as_str(&input), "--output", as_str(&output)]);
    assert_eq!(error["error"]["code"], "trans_unsafe_output");
    assert!(!output.exists());
}

#[test]
fn trans_detects_publish_races_without_overwriting_the_destination() {
    let world = TransWorld::new();
    world.init();
    world.install_script(
        "anydoc",
        r#"#!/bin/sh
set -eu
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then
    out="$arg"
    break
  fi
  prev="$arg"
done
printf 'winner from child\n' > "$out"
/bin/sleep 1
"#,
    );
    world.ok(&["config", "set", "--trans", "anydoc"]);
    let input = world.write_project("docs/source.docx", "stub");
    let output = world.project.join("out/raced.md");

    let child = world.spawn(&["trans", as_str(&input), "--output", as_str(&output)]);
    thread::sleep(Duration::from_millis(100));
    world.write_path(&output, b"racer content");

    let waited = child.wait_with_output().unwrap();
    assert!(!waited.status.success());
    let error: Value = serde_json::from_slice(&waited.stderr).unwrap();
    assert_eq!(error["error"]["code"], "trans_publish_race");
    assert_eq!(fs::read_to_string(&output).unwrap(), "racer content");
}
