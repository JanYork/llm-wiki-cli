use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    env, fmt,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SCHEMA: u8 = 1;
const CHECK_INTERVAL: u64 = 60 * 60;
const STALE_MARKER: Duration = Duration::from_secs(5 * 60);
const MAX_STATE_BYTES: u64 = 4 * 1024;
const LATEST_URL: &str = "https://github.com/JanYork/llm-wiki-cli/releases/latest";
static MARKER_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct UpdateState {
    schema: u8,
    last_attempt: Option<u64>,
    latest_version: Option<String>,
    notified_version: Option<String>,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            last_attempt: None,
            latest_version: None,
            notified_version: None,
        }
    }
}

pub(crate) struct LifecycleUpdate {
    pub(crate) notice: Option<Value>,
    pub(crate) notice_version: Option<String>,
    pub(crate) check_due: bool,
}

pub(crate) fn prepare_lifecycle() -> io::Result<LifecycleUpdate> {
    let paths = Paths::resolve()?;
    with_marker(&paths.marker, || {
        let mut state = read_state(&paths.state)?;
        let notice = pending_notice(&state);
        let now = now_unix_seconds()?;
        let check_due = state
            .last_attempt
            .is_none_or(|attempt| now.saturating_sub(attempt) >= CHECK_INTERVAL);
        if check_due {
            state.last_attempt = Some(now);
            write_state(&paths.state, &state)?;
        }
        let notice_version = notice.as_ref().map(|(_, version)| version.clone());
        Ok(LifecycleUpdate {
            notice: notice.map(|(current, latest)| {
                json!({
                    "available": true,
                    "current_version": current,
                    "latest_version": latest,
                    "instruction": format!(
                        "Ask the user for explicit consent before updating LWC. Without explicit consent, skip version {latest}. Never install automatically."
                    ),
                })
            }),
            notice_version,
            check_due,
        })
    })
}

pub(crate) fn mark_notified(version: &str) -> io::Result<bool> {
    let paths = Paths::resolve()?;
    with_marker(&paths.marker, || {
        let mut state = read_state(&paths.state)?;
        let pending = pending_notice(&state).is_some_and(|(_, latest)| latest == version);
        if pending {
            state.notified_version = Some(version.to_owned());
            write_state(&paths.state, &state)?;
        }
        Ok(pending)
    })
}

pub(crate) fn spawn_checker() -> io::Result<()> {
    // Test-only: complete the fake transport without a second LWC process.
    if env::var_os("LWC_TEST_UPDATE_CURL").is_some()
        && env::var("LWC_TEST_UPDATE_WAIT_FOR_CHECKER").as_deref() == Ok("1")
    {
        run_checker();
        return Ok(());
    }
    let mut command = Command::new(env::current_exe()?);
    command
        .arg("__update-check")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command.spawn().map(|_| ())
}

pub(crate) fn run_checker() {
    let _ = check_latest();
}

fn check_latest() -> io::Result<()> {
    let paths = Paths::resolve()?;
    with_marker_retry(&paths.marker, || {
        let latest = fetch_latest_version()?;
        let mut state = read_state(&paths.state)?;
        state.latest_version = Some(latest);
        write_state(&paths.state, &state)
    })
}

fn pending_notice(state: &UpdateState) -> Option<(String, String)> {
    let current = parse_version(env!("CARGO_PKG_VERSION"))?;
    let latest_text = state.latest_version.as_ref()?;
    let latest = parse_version(latest_text)?;
    (latest > current && state.notified_version.as_deref() != Some(latest_text.as_str()))
        .then(|| (current.to_string(), latest.to_string()))
}

fn fetch_latest_version() -> io::Result<String> {
    let curl = env::var_os("LWC_TEST_UPDATE_CURL").unwrap_or_else(|| "curl".into());
    let null_output = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let output = Command::new(curl)
        .args([
            "-fsSLI",
            "--connect-timeout",
            "2",
            "--max-time",
            "5",
            "--output",
            null_output,
            "--write-out",
            "%{url_effective}",
            LATEST_URL,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("release check failed"));
    }
    let effective = std::str::from_utf8(&output.stdout)
        .map_err(|_| io::Error::other("release URL is not UTF-8"))?
        .trim();
    let tag = effective
        .strip_prefix("https://github.com/JanYork/llm-wiki-cli/releases/tag/v")
        .ok_or_else(|| io::Error::other("unexpected release URL"))?;
    if tag.contains('/') || tag.contains('?') || tag.contains('#') {
        return Err(io::Error::other("unexpected release tag"));
    }
    parse_version(tag)
        .map(|version| version.to_string())
        .ok_or_else(|| io::Error::other("invalid release version"))
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version(u64, u64, u64);

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.0, self.1, self.2)
    }
}

fn parse_version(value: &str) -> Option<Version> {
    let mut parts = value.split('.');
    let version = Version(
        parse_component(parts.next()?)?,
        parse_component(parts.next()?)?,
        parse_component(parts.next()?)?,
    );
    parts.next().is_none().then_some(version)
}

fn parse_component(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

struct Paths {
    state: PathBuf,
    marker: PathBuf,
}

impl Paths {
    fn resolve() -> io::Result<Self> {
        let home = env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .filter(|value| !value.is_empty())
            .ok_or_else(|| io::Error::other("home directory is unavailable"))?;
        let root = PathBuf::from(home).join(".lwc");
        fs::create_dir_all(&root)?;
        let metadata = fs::symlink_metadata(&root)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(io::Error::other("update state directory is unsafe"));
        }
        Ok(Self {
            state: root.join("update-check.json"),
            marker: root.join("update-check.lock"),
        })
    }
}

fn read_state(path: &Path) -> io::Result<UpdateState> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(UpdateState::default()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > MAX_STATE_BYTES
    {
        return Err(io::Error::other("update state file is unsafe"));
    }
    let state: UpdateState = serde_json::from_slice(&fs::read(path)?)
        .map_err(|_| io::Error::other("invalid update state"))?;
    if state.schema != SCHEMA {
        return Err(io::Error::other("unsupported update state schema"));
    }
    Ok(state)
}

fn write_state(path: &Path, state: &UpdateState) -> io::Result<()> {
    let bytes = serde_json::to_vec(state).map_err(io::Error::other)?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(io::Error::other("update state is too large"));
    }
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&bytes)?;
        replace_file(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn with_marker<T>(path: &Path, operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let marker = acquire_marker(path)?;
    finish_with_marker(path, marker, operation)
}

fn with_marker_retry<T>(path: &Path, operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let deadline = Instant::now() + Duration::from_millis(250);
    let marker = loop {
        match acquire_marker(path) {
            Ok(marker) => break marker,
            Err(error)
                if error.kind() == io::ErrorKind::AlreadyExists && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    };
    finish_with_marker(path, marker, operation)
}

fn acquire_marker(path: &Path) -> io::Result<Marker> {
    let token = marker_token();
    match create_marker(path, &token) {
        Ok(marker) => Ok(marker),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            reclaim_stale(path, &token)?.ok_or(error)
        }
        Err(error) => Err(error),
    }
}

fn finish_with_marker<T>(
    path: &Path,
    marker: Marker,
    operation: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    let result = operation();
    if marker.acquired.elapsed() < STALE_MARKER {
        remove_owned_marker(path, &marker.token);
    }
    result
}

struct Marker {
    token: String,
    acquired: Instant,
}

fn create_marker(path: &Path, token: &str) -> io::Result<Marker> {
    let staging = path.with_extension(format!("lock.owner-{token}"));
    fs::create_dir(&staging)?;
    let result = (|| {
        write_token(&staging.join("owner"), token)?;
        fs::rename(&staging, path).map_err(|error| {
            if path.exists() {
                io::Error::from(io::ErrorKind::AlreadyExists)
            } else {
                error
            }
        })?;
        Ok(Marker {
            token: token.to_owned(),
            acquired: Instant::now(),
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(staging.join("owner"));
        let _ = fs::remove_dir(staging);
    }
    result
}

fn write_token(path: &Path, token: &str) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(token.as_bytes())
}

fn reclaim_stale(path: &Path, reaper: &str) -> io::Result<Option<Marker>> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(None);
    }
    if !metadata
        .modified()?
        .elapsed()
        .is_ok_and(|elapsed| elapsed >= STALE_MARKER)
    {
        return Ok(None);
    }
    let observed = read_marker(path)?;
    let reaping = path.join("reaping");
    if let Err(error) = write_token(&reaping, reaper) {
        return match error.kind() {
            io::ErrorKind::AlreadyExists => Ok(None),
            _ => Err(error),
        };
    }
    if !read_marker(path).is_ok_and(|token| token == observed) {
        remove_reaping_token(path, reaper);
        return Ok(None);
    }
    let quarantine = path.with_extension("lock.stale");
    cleanup_stale_quarantine(&quarantine);
    if let Err(error) = fs::rename(path, &quarantine) {
        remove_reaping(path, &observed, reaper);
        return Err(if quarantine.exists() {
            io::Error::from(io::ErrorKind::AlreadyExists)
        } else {
            error
        });
    }
    let successor = create_marker(path, reaper);
    remove_quarantine(&quarantine, &observed, reaper);
    successor.map(Some)
}

fn read_marker(path: &Path) -> io::Result<String> {
    let owner = path.join("owner");
    let metadata = fs::symlink_metadata(&owner)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 128 {
        return Err(io::Error::other("update marker is unsafe"));
    }
    String::from_utf8(fs::read(owner)?).map_err(|_| io::Error::other("invalid update marker"))
}

fn remove_owned_marker(path: &Path, owner: &str) {
    if read_marker(path).is_ok_and(|token| token == owner) {
        let _ = fs::remove_file(path.join("owner"));
        let _ = fs::remove_dir(path);
    }
}

fn remove_quarantine(path: &Path, owner: &str, reaper: &str) {
    if read_marker(path).is_ok_and(|token| token == owner)
        && fs::read_to_string(path.join("reaping")).is_ok_and(|token| token == reaper)
    {
        let _ = fs::remove_file(path.join("reaping"));
        let _ = fs::remove_file(path.join("owner"));
        let _ = fs::remove_dir(path);
    }
}

fn remove_reaping(path: &Path, owner: &str, reaper: &str) {
    if read_marker(path).is_ok_and(|token| token == owner) {
        remove_reaping_token(path, reaper);
    }
}

fn remove_reaping_token(path: &Path, reaper: &str) {
    let reaping = path.join("reaping");
    if fs::read_to_string(&reaping).is_ok_and(|token| token == reaper) {
        let _ = fs::remove_file(reaping);
    }
}

fn cleanup_stale_quarantine(path: &Path) {
    let stale = fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && metadata
                .modified()
                .and_then(|modified| modified.elapsed().map_err(io::Error::other))
                .is_ok_and(|elapsed| elapsed >= STALE_MARKER)
    });
    if stale {
        let _ = fs::remove_file(path.join("reaping"));
        let _ = fs::remove_file(path.join("owner"));
        let _ = fs::remove_dir(path);
    }
}

fn marker_token() -> String {
    let sequence = MARKER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("{}-{nanos}-{sequence}", std::process::id())
}

fn now_unix_seconds() -> io::Result<u64> {
    if let Ok(value) = env::var("LWC_TEST_UPDATE_NOW_UNIX_SECS") {
        return value
            .parse()
            .map_err(|_| io::Error::other("invalid test clock"));
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(io::Error::other)
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    (result != 0)
        .then_some(())
        .ok_or_else(io::Error::last_os_error)
}
