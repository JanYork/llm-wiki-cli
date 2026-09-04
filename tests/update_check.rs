use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::Barrier,
    thread,
    time::{Duration, Instant},
};

const NEW_VERSION: &str = "99.0.0";

struct World {
    _temp: tempfile::TempDir,
    project: PathBuf,
    home: PathBuf,
    curl: PathBuf,
    curl_log: PathBuf,
}

impl World {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let home = temp.path().join("home");
        let curl_log = temp.path().join("curl.log");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&home).unwrap();
        let curl = write_fake_curl(temp.path());
        let world = Self {
            _temp: temp,
            project,
            home,
            curl,
            curl_log,
        };
        let initialized = Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&world.project)
            .env("HOME", &world.home)
            .env("USERPROFILE", &world.home)
            .args(["init"])
            .output()
            .unwrap();
        assert!(
            initialized.status.success(),
            "failed to initialize update-check fixture: {}",
            String::from_utf8_lossy(&initialized.stderr)
        );
        world
    }

    fn hook(&self, event: &str, payload: &Value, environment: &[(&str, &str)]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env_remove("LWC_PROJECT_ROOT")
            .env("LWC_TEST_UPDATE_CURL", &self.curl)
            .env("LWC_TEST_UPDATE_CURL_LOG", &self.curl_log)
            .env("LWC_TEST_UPDATE_LATEST_VERSION", NEW_VERSION)
            .envs(environment.iter().copied())
            .args(["agent", "hook", "--agent", "claude", "--event", event])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn session_hook(&self, environment: &[(&str, &str)]) -> Output {
        self.hook(
            "SessionStart",
            &serde_json::json!({
                "source":"startup",
                "session_id":"update-test-session",
                "cwd":self.project,
            }),
            environment,
        )
    }

    fn state_path(&self) -> PathBuf {
        self.home.join(".lwc/update-check.json")
    }

    fn marker_path(&self) -> PathBuf {
        self.home.join(".lwc/update-check.lock")
    }

    fn marker_owner_path(&self) -> PathBuf {
        self.marker_path().join("owner")
    }

    fn curl_calls(&self) -> usize {
        fs::read_to_string(&self.curl_log)
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    fn curl_arguments(&self) -> String {
        fs::read_to_string(&self.curl_log).unwrap_or_default()
    }

    fn wait_for_state(&self) -> Value {
        wait_until(Duration::from_secs(5), || {
            fs::read(self.state_path())
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .is_some()
                && !self.marker_path().exists()
        });
        serde_json::from_slice(&fs::read(self.state_path()).unwrap()).unwrap()
    }

    fn wait_for_latest(&self, version: &str) -> Value {
        wait_until(Duration::from_secs(5), || {
            fs::read(self.state_path())
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .is_some_and(|state| state["latest_version"] == version)
                && !self.marker_path().exists()
        });
        serde_json::from_slice(&fs::read(self.state_path()).unwrap()).unwrap()
    }
}

fn wait_until(timeout: Duration, predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + timeout;
    while !predicate() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(predicate(), "condition was not met within {timeout:?}");
}

fn hook_value(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "update checks must not write stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn readiness(value: &Value) -> Value {
    let context = value
        .pointer("/hookSpecificOutput/additionalContext")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("Claude lifecycle Hook must return additionalContext: {value}"));
    let line = context
        .lines()
        .find_map(|line| line.strip_prefix("LWC_READINESS "))
        .expect("lifecycle context must contain LWC_READINESS");
    serde_json::from_str(line).unwrap()
}

fn assert_no_update(output: &Output) {
    assert!(readiness(&hook_value(output)).get("update").is_none());
}

#[cfg(unix)]
fn write_fake_curl(root: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = root.join("fake-curl");
    fs::write(
        &path,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$LWC_TEST_UPDATE_CURL_LOG"
case "${LWC_TEST_UPDATE_CURL_MODE:-latest}" in
  slow) sleep 2 ;;
  fail) exit 22 ;;
  malformed) printf '%s' 'https://github.com/JanYork/llm-wiki-cli/releases/tag/v1.2.3-beta'; exit 0 ;;
esac
printf 'noise on stderr\n' >&2
printf '%s' "https://github.com/JanYork/llm-wiki-cli/releases/tag/v${LWC_TEST_UPDATE_LATEST_VERSION}"
"#,
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(windows)]
fn write_fake_curl(root: &Path) -> PathBuf {
    let path = root.join("fake-curl.cmd");
    fs::write(
        &path,
        "@echo off\r\n\
         echo %*>>\"%LWC_TEST_UPDATE_CURL_LOG%\"\r\n\
         if \"%LWC_TEST_UPDATE_CURL_MODE%\"==\"fail\" exit /b 22\r\n\
         if \"%LWC_TEST_UPDATE_CURL_MODE%\"==\"malformed\" (echo https://github.com/JanYork/llm-wiki-cli/releases/tag/v1.2.3-beta& exit /b 0)\r\n\
         echo noise on stderr 1>&2\r\n\
         <nul set /p=\"https://github.com/JanYork/llm-wiki-cli/releases/tag/v%LWC_TEST_UPDATE_LATEST_VERSION%\"\r\n",
    )
    .unwrap();
    path
}

#[cfg(unix)]
#[test]
fn discovery_is_detached_and_not_visible_until_the_next_hook() {
    let world = World::new();
    let started = Instant::now();
    let first = world.session_hook(&[("LWC_TEST_UPDATE_CURL_MODE", "slow")]);
    let elapsed = started.elapsed();

    assert_no_update(&first);
    assert!(
        elapsed < Duration::from_millis(1_500),
        "Hook waited for the two-second fake network call: {elapsed:?}"
    );

    let state = world.wait_for_latest(NEW_VERSION);
    assert_eq!(state["latest_version"], NEW_VERSION);
    let keys = state
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        vec![
            "last_attempt".to_owned(),
            "latest_version".to_owned(),
            "notified_version".to_owned(),
            "schema".to_owned(),
        ]
    );
    let serialized = state.to_string();
    assert!(!serialized.contains(world.project.to_string_lossy().as_ref()));
    assert!(!serialized.contains("update-test-session"));
    let curl_arguments = world.curl_arguments();
    assert!(curl_arguments.contains("--connect-timeout"));
    assert!(curl_arguments.contains("--max-time"));
    assert!(curl_arguments.contains("https://github.com/JanYork/llm-wiki-cli/releases/latest"));

    let second = hook_value(&world.session_hook(&[]));
    let second_readiness = readiness(&second);
    let update = &second_readiness["update"];
    assert_eq!(update["available"], true);
    assert_eq!(update["current_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(update["latest_version"], NEW_VERSION);
    let instruction = update["instruction"].as_str().unwrap();
    assert!(instruction.contains("Ask the user for explicit consent"));
    assert!(instruction.contains("Without explicit consent, skip"));
    assert!(instruction.contains("Never install automatically"));
    let claimed_state: Value =
        serde_json::from_slice(&fs::read(world.state_path()).unwrap()).unwrap();
    assert_eq!(claimed_state["notified_version"], NEW_VERSION);

    assert_no_update(&world.session_hook(&[]));
    assert_eq!(
        world.curl_calls(),
        1,
        "one-hour throttle must suppress curl"
    );
}

#[test]
fn concurrent_hooks_claim_a_pending_notice_exactly_once() {
    let world = World::new();
    assert_no_update(&world.session_hook(&[]));
    world.wait_for_latest(NEW_VERSION);

    let barrier = Barrier::new(4);
    let outputs = thread::scope(|scope| {
        let handles = (0..4)
            .map(|index| {
                let barrier = &barrier;
                let world = &world;
                scope.spawn(move || {
                    barrier.wait();
                    world.hook(
                        "SessionStart",
                        &serde_json::json!({
                            "source":"startup",
                            "session_id":format!("concurrent-update-{index}"),
                            "cwd":world.project,
                        }),
                        &[],
                    )
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });

    let notices = outputs
        .iter()
        .map(hook_value)
        .filter(|output| {
            output
                .pointer("/hookSpecificOutput/additionalContext")
                .is_some()
        })
        .map(|output| readiness(&output))
        .filter(|readiness| readiness["update"]["available"] == true)
        .count();
    assert_eq!(notices, 1, "a pending version must be claimed exactly once");
}

#[test]
fn throttle_covers_the_full_hour_after_every_attempt() {
    let world = World::new();
    assert_no_update(&world.session_hook(&[("LWC_TEST_UPDATE_NOW_UNIX_SECS", "1000000")]));
    world.wait_for_latest(NEW_VERSION);
    assert_eq!(world.curl_calls(), 1);

    hook_value(&world.session_hook(&[("LWC_TEST_UPDATE_NOW_UNIX_SECS", "1003599")]));
    thread::sleep(Duration::from_millis(100));
    assert_eq!(world.curl_calls(), 1);

    assert_no_update(&world.session_hook(&[("LWC_TEST_UPDATE_NOW_UNIX_SECS", "1003600")]));
    wait_until(Duration::from_secs(3), || world.curl_calls() == 2);
}

#[test]
fn only_context_producing_lifecycle_events_start_checks() {
    for (event, payload) in [
        (
            "SessionStart",
            serde_json::json!({"source":"startup","session_id":"session-start"}),
        ),
        (
            "SessionStart",
            serde_json::json!({"source":"resume","session_id":"session-resume"}),
        ),
        (
            "SessionStart",
            serde_json::json!({"source":"clear","session_id":"session-clear"}),
        ),
        (
            "SessionStart",
            serde_json::json!({"source":"compact","session_id":"compact-after"}),
        ),
        (
            "SubagentStart",
            serde_json::json!({"session_id":"root","agent_id":"child"}),
        ),
    ] {
        let world = World::new();
        assert_no_update(&world.hook(event, &payload, &[]));
        world.wait_for_latest(NEW_VERSION);
    }

    let world = World::new();
    let prompt = world.hook(
        "UserPromptSubmit",
        &serde_json::json!({
            "hook_event_name":"UserPromptSubmit",
            "prompt":"check this code",
            "session_id":"prompt-only"
        }),
        &[],
    );
    hook_value(&prompt);
    thread::sleep(Duration::from_millis(250));
    assert_eq!(world.curl_calls(), 0);
}

#[test]
fn current_malformed_and_failed_checks_stay_silent_and_are_throttled() {
    for (mode, latest) in [
        ("latest", env!("CARGO_PKG_VERSION")),
        ("latest", "0.1.0"),
        ("malformed", NEW_VERSION),
        ("fail", NEW_VERSION),
    ] {
        let world = World::new();
        let latest_env = [
            ("LWC_TEST_UPDATE_CURL_MODE", mode),
            ("LWC_TEST_UPDATE_LATEST_VERSION", latest),
        ];
        assert_no_update(&world.session_hook(&latest_env));
        wait_until(Duration::from_secs(3), || world.curl_calls() == 1);
        world.wait_for_state();
        assert_no_update(&world.session_hook(&latest_env));
        assert_eq!(
            world.curl_calls(),
            1,
            "failed attempts must also be throttled"
        );
    }
}

#[test]
fn unreadable_state_never_breaks_or_annotates_hooks() {
    let state = World::new();
    fs::create_dir_all(state.state_path()).unwrap();
    assert_no_update(&state.session_hook(&[]));
    assert!(state.state_path().is_dir());
}

#[cfg(unix)]
#[test]
fn stale_takeover_does_not_move_the_live_successor() {
    let world = World::new();
    fs::create_dir_all(world.marker_path()).unwrap();
    fs::write(world.marker_owner_path(), "stale-owner").unwrap();
    let touched = Command::new("/usr/bin/touch")
        .args(["-t", "200001010000"])
        .arg(world.marker_path())
        .status()
        .unwrap();
    assert!(touched.success());

    assert_no_update(&world.session_hook(&[("LWC_TEST_UPDATE_CURL_MODE", "slow")]));
    wait_until(Duration::from_secs(3), || world.curl_calls() == 1);
    let successor = fs::read_to_string(world.marker_owner_path()).unwrap();
    assert_ne!(successor, "stale-owner");

    assert_no_update(&world.session_hook(&[]));
    assert_eq!(
        fs::read_to_string(world.marker_owner_path()).unwrap(),
        successor
    );
    wait_until(Duration::from_secs(5), || !world.marker_path().exists());
}
