use crate::{
    config::{self, CapabilitySetting},
    error::{AppError, Result},
    scope::global_lwc_root,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::Command,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const RUNTIME_MANIFEST: &str = "runtime.json";
const RELEASE_ROOT: &str = "https://github.com/JanYork/llm-wiki-cli/releases/download";
const CHECKSUMS_MAX_BYTES: u64 = 1024 * 1024;
const ARCHIVE_MAX_BYTES: u64 = 256 * 1024 * 1024;
const INSTALL_LOCK_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy)]
pub(crate) enum Plugin {
    Tutor,
    Book,
    Practice,
}

impl Plugin {
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Tutor => "tutor",
            Self::Book => "book",
            Self::Practice => "practice",
        }
    }

    fn disabled_code(self) -> &'static str {
        match self {
            Self::Tutor => "tutor_disabled",
            Self::Book => "book_disabled",
            Self::Practice => "practice_disabled",
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeManifest {
    plugin: String,
    version: String,
    target: String,
    asset: String,
    sha256: String,
    binary: PathBuf,
}

struct Paths {
    runtime: PathBuf,
    target: &'static str,
    asset: String,
    binary: &'static str,
}

impl Paths {
    fn new(plugin: Plugin) -> Result<Self> {
        let target = target()?;
        let extension = if cfg!(windows) { "zip" } else { "tar.gz" };
        let asset = format!(
            "lwc-{}-{}-{target}.{extension}",
            plugin.id(),
            env!("CARGO_PKG_VERSION")
        );
        let runtime = global_lwc_root()?
            .join("runtime")
            .join(plugin.id())
            .join(env!("CARGO_PKG_VERSION"))
            .join(target);
        let binary = match plugin {
            Plugin::Tutor => binary_name("lwc-tutor", "lwc-tutor.exe"),
            Plugin::Book => binary_name("lwc-book", "lwc-book.exe"),
            Plugin::Practice => binary_name("lwc-practice", "lwc-practice.exe"),
        };
        Ok(Self {
            runtime,
            target,
            asset,
            binary,
        })
    }
}

pub(crate) fn run(plugin: Plugin, cwd: &Path, args: &[OsString]) -> Result<Value> {
    if config::resolve_learning(plugin.id())?.setting != CapabilitySetting::Enabled {
        return Err(AppError::new(
            plugin.disabled_code(),
            format!("{} capability is disabled", plugin.id()),
        )
        .with_details(json!({
            "configure": format!("lwc --scope global config set --{} enabled", plugin.id()),
        })));
    }
    let paths = Paths::new(plugin)?;
    if runtime_binary(plugin, &paths)?.is_none() {
        install(plugin, &paths)?;
    }
    let binary = runtime_binary(plugin, &paths)?.ok_or_else(|| {
        AppError::new(
            "learning_runtime_missing",
            format!("{} runtime is not installed", plugin.id()),
        )
        .with_details(json!({
            "plugin": plugin.id(),
            "runtime": paths.runtime,
            "recovery": format!("retry `lwc {} ...` to install the fixed runtime", plugin.id()),
        }))
    })?;
    let status = Command::new(binary)
        .args(args)
        .current_dir(cwd)
        .env("LWC_PLUGIN_SKIP_UPDATE", "1")
        .env("LWC_PLUGIN_NO_BACKGROUND", "1")
        .status()?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(Value::Null)
}

pub(crate) fn status(plugin: Plugin) -> Result<Value> {
    let paths = Paths::new(plugin)?;
    let binary = runtime_binary(plugin, &paths)?;
    let health = if binary.is_some() {
        "ready"
    } else if fs::symlink_metadata(&paths.runtime).is_ok() {
        "invalid"
    } else {
        "missing"
    };
    let data = global_lwc_root()?.join("plugins").join(plugin.id());
    Ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "runtime": paths.runtime,
        "runtime_health": health,
        "installed": binary.is_some(),
        "data_present": fs::symlink_metadata(data).is_ok(),
    }))
}

fn install(plugin: Plugin, paths: &Paths) -> Result<()> {
    let Some(_lock) = acquire_install_lock(plugin, paths)? else {
        return Ok(());
    };
    if runtime_binary(plugin, paths)?.is_some() {
        return Ok(());
    }
    quarantine_invalid_runtime(plugin, paths)?;
    let parent = paths.runtime.parent().expect("runtime has a parent");
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let staging = parent.join(format!(
        ".{}.download-{}-{suffix}",
        paths.target,
        std::process::id()
    ));
    fs::create_dir(&staging)?;
    let result = install_staged(plugin, paths, &staging);
    let _ = fs::remove_dir_all(&staging);
    result
}

fn install_staged(plugin: Plugin, paths: &Paths, staging: &Path) -> Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let release = format!("{}/v{version}", release_root());
    let checksums = staging.join("SHA256SUMS");
    download(&format!("{release}/SHA256SUMS"), &checksums)?;
    ensure_regular_bounded(
        &checksums,
        CHECKSUMS_MAX_BYTES,
        "learning_checksums_too_large",
    )?;
    let expected = checksum_for(&fs::read(&checksums)?, &paths.asset)?;

    let archive = staging.join(&paths.asset);
    download(&format!("{release}/{}", paths.asset), &archive)?;
    ensure_regular_bounded(&archive, ARCHIVE_MAX_BYTES, "learning_archive_too_large")?;
    if file_sha256(&archive)? != expected {
        return Err(AppError::new(
            "learning_checksum_mismatch",
            format!("checksum mismatch for {}", paths.asset),
        ));
    }

    let unpack = staging.join("unpack");
    fs::create_dir(&unpack)?;
    let archive_root = paths
        .asset
        .strip_suffix(if cfg!(windows) { ".zip" } else { ".tar.gz" })
        .expect("asset has a fixed extension");
    validate_archive_listing(&archive, archive_root, paths.binary)?;
    let status = Command::new("tar")
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(&unpack)
        .status()
        .map_err(|error| {
            AppError::new(
                "learning_archive_extract_failed",
                format!("failed to start tar: {error}"),
            )
        })?;
    if !status.success() {
        return Err(AppError::new(
            "learning_archive_extract_failed",
            format!("failed to extract {}", paths.asset),
        ));
    }
    let extracted_root = unpack.join(archive_root);
    let extracted_binary = extracted_root.join(paths.binary);
    validate_extracted_archive(&extracted_root, &extracted_binary)?;

    let publish = staging.join("publish");
    fs::create_dir(&publish)?;
    fs::rename(&extracted_binary, publish.join(paths.binary))?;
    let binary_sha = file_sha256(&publish.join(paths.binary))?;
    let manifest = RuntimeManifest {
        plugin: plugin.id().to_owned(),
        version: version.to_owned(),
        target: paths.target.to_owned(),
        asset: paths.asset.clone(),
        sha256: binary_sha,
        binary: PathBuf::from(paths.binary),
    };
    fs::write(
        publish.join(RUNTIME_MANIFEST),
        serde_json::to_vec_pretty(&manifest)
            .map_err(|error| AppError::new("json_error", error.to_string()))?,
    )?;
    if fs::symlink_metadata(&paths.runtime).is_ok() {
        return Err(AppError::new(
            "learning_runtime_publish_conflict",
            format!("{} runtime appeared during installation", plugin.id()),
        ));
    }
    fs::rename(publish, &paths.runtime)?;
    Ok(())
}

fn download(url: &str, destination: &Path) -> Result<()> {
    let status = Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--retry",
            "3",
            "--connect-timeout",
            "15",
            "--max-time",
            "300",
            "--output",
        ])
        .arg(destination)
        .arg(url)
        .status()
        .map_err(|error| {
            AppError::new(
                "learning_download_failed",
                format!("failed to start runtime download: {error}"),
            )
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(AppError::new(
            "learning_download_failed",
            format!("failed to download {url}"),
        ))
    }
}

fn checksum_for(bytes: &[u8], asset: &str) -> Result<String> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        AppError::new(
            "learning_checksums_invalid",
            "SHA256SUMS is not valid UTF-8",
        )
    })?;
    let mut selected = Vec::new();
    for line in text.lines().filter(|line| !line.is_empty()) {
        let Some((hash, name)) = line.split_once("  ") else {
            return Err(AppError::new(
                "learning_checksums_invalid",
                "SHA256SUMS contains a malformed entry",
            ));
        };
        if !valid_sha256(hash) || name.is_empty() || name.contains('/') || name.contains('\\') {
            return Err(AppError::new(
                "learning_checksums_invalid",
                "SHA256SUMS contains an invalid hash or asset name",
            ));
        }
        if name == asset {
            selected.push(hash.to_owned());
        }
    }
    if selected.len() != 1 {
        return Err(AppError::new(
            "learning_checksum_entry_invalid",
            format!("SHA256SUMS must contain exactly one entry for {asset}"),
        ));
    }
    Ok(selected.pop().unwrap())
}

fn validate_archive_listing(archive: &Path, root: &str, binary: &str) -> Result<()> {
    let output = Command::new("tar")
        .arg("-tf")
        .arg(archive)
        .output()
        .map_err(|error| {
            AppError::new(
                "learning_archive_invalid",
                format!("failed to inspect runtime archive: {error}"),
            )
        })?;
    if !output.status.success() || output.stdout.len() > 64 * 1024 {
        return Err(AppError::new(
            "learning_archive_invalid",
            "runtime archive listing failed or is too large",
        ));
    }
    let listing = std::str::from_utf8(&output.stdout).map_err(|_| {
        AppError::new(
            "learning_archive_invalid",
            "runtime archive listing is not UTF-8",
        )
    })?;
    let expected_root = format!("{root}/");
    let expected_binary = format!("{root}/{binary}");
    let entries = listing.lines().collect::<Vec<_>>();
    if entries.len() != 2
        || !entries.contains(&expected_root.as_str())
        || !entries.contains(&expected_binary.as_str())
    {
        return Err(AppError::new(
            "learning_archive_invalid",
            "runtime archive must contain exactly its fixed root and binary",
        ));
    }
    Ok(())
}

fn ensure_regular_bounded(path: &Path, max: u64, code: &'static str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > max {
        return Err(AppError::new(
            code,
            format!("downloaded file is not a regular file at most {max} bytes"),
        ));
    }
    Ok(())
}

fn validate_extracted_archive(root: &Path, binary: &Path) -> Result<()> {
    let root_metadata = fs::symlink_metadata(root).map_err(|_| {
        AppError::new(
            "learning_archive_invalid",
            "runtime archive has an unexpected root",
        )
    })?;
    let binary_metadata = fs::symlink_metadata(binary).map_err(|_| {
        AppError::new(
            "learning_archive_invalid",
            "runtime archive is missing its binary",
        )
    })?;
    if !root_metadata.is_dir()
        || root_metadata.file_type().is_symlink()
        || !binary_metadata.is_file()
        || binary_metadata.file_type().is_symlink()
    {
        return Err(AppError::new(
            "learning_archive_invalid",
            "runtime archive contains an unsafe path",
        ));
    }
    let entries = fs::read_dir(root)?.collect::<std::io::Result<Vec<_>>>()?;
    if entries.len() != 1 || entries[0].path() != binary {
        return Err(AppError::new(
            "learning_archive_invalid",
            "runtime archive must contain only the fixed binary",
        ));
    }
    Ok(())
}

fn quarantine_invalid_runtime(plugin: Plugin, paths: &Paths) -> Result<()> {
    if fs::symlink_metadata(&paths.runtime).is_err() || runtime_binary(plugin, paths)?.is_some() {
        return Ok(());
    }
    let parent = paths.runtime.parent().expect("runtime has a parent");
    let destination = parent.join(format!("{}.invalid-{}", paths.target, std::process::id()));
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(AppError::new(
            "learning_runtime_quarantine_failed",
            format!("cannot quarantine invalid {} runtime", plugin.id()),
        ));
    }
    fs::rename(&paths.runtime, destination)?;
    Ok(())
}

struct InstallLock(PathBuf);

impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn acquire_install_lock(plugin: Plugin, paths: &Paths) -> Result<Option<InstallLock>> {
    let parent = paths.runtime.parent().expect("runtime has a parent");
    fs::create_dir_all(parent)?;
    let lock = parent.join(format!(".{}.install.lock", paths.target));
    let started = Instant::now();
    loop {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&lock) {
            Ok(mut file) => {
                writeln!(file, "pid={}", std::process::id())?;
                file.sync_all()?;
                return Ok(Some(InstallLock(lock)));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if runtime_binary(plugin, paths)?.is_some() {
                    return Ok(None);
                }
                if started.elapsed() >= INSTALL_LOCK_TIMEOUT {
                    return Err(AppError::new(
                        "learning_install_busy",
                        format!("another {} installation is in progress", plugin.id()),
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn release_root() -> String {
    #[cfg(debug_assertions)]
    if let Ok(value) = std::env::var("LWC_TEST_LEARNING_RELEASE_ROOT") {
        return value;
    }
    RELEASE_ROOT.to_owned()
}

fn runtime_binary(plugin: Plugin, paths: &Paths) -> Result<Option<PathBuf>> {
    let Ok(runtime) = fs::symlink_metadata(&paths.runtime) else {
        return Ok(None);
    };
    if !runtime.is_dir() || runtime.file_type().is_symlink() {
        return Ok(None);
    }
    let manifest_path = paths.runtime.join(RUNTIME_MANIFEST);
    let Ok(metadata) = fs::symlink_metadata(&manifest_path) else {
        return Ok(None);
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Ok(None);
    }
    let Ok(manifest) = serde_json::from_slice::<RuntimeManifest>(&fs::read(manifest_path)?) else {
        return Ok(None);
    };
    if manifest.plugin != plugin.id()
        || manifest.version != env!("CARGO_PKG_VERSION")
        || manifest.target != paths.target
        || manifest.asset != paths.asset
        || manifest.binary != Path::new(paths.binary)
        || manifest.binary.is_absolute()
        || manifest
            .binary
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || !valid_sha256(&manifest.sha256)
    {
        return Ok(None);
    }
    let binary = paths.runtime.join(&manifest.binary);
    let Ok(metadata) = fs::symlink_metadata(&binary) else {
        return Ok(None);
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Ok(None);
    }
    Ok((file_sha256(&binary)? == manifest.sha256).then_some(binary))
}

fn file_sha256(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(windows)]
const fn binary_name(_unix: &'static str, windows: &'static str) -> &'static str {
    windows
}

#[cfg(not(windows))]
const fn binary_name(unix: &'static str, _windows: &'static str) -> &'static str {
    unix
}

fn target() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-gnu"),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("windows", "aarch64") => Ok("aarch64-pc-windows-msvc"),
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        (os, arch) => Err(AppError::new(
            "learning_runtime_unsupported_platform",
            format!("learning runtimes do not support {os}/{arch}"),
        )),
    }
}
