use crate::{
    config::{self, OfficeSetting},
    error::{AppError, Result},
    scope::global_lwc_root,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::Command,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const VERSION: &str = "v1.0.144";
const RELEASE_ROOT: &str = "https://github.com/iOfficeAI/OfficeCLI/releases/download";
const RUNTIME_MANIFEST: &str = "runtime.json";
const INSTALL_LOCK_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeManifest {
    version: String,
    target: String,
    asset: String,
    sha256: String,
    binary: PathBuf,
}

struct TargetSpec {
    target: &'static str,
    asset: &'static str,
    sha256: &'static str,
}

struct Paths {
    runtime: PathBuf,
    spec: TargetSpec,
}

impl Paths {
    fn new() -> Result<Self> {
        let spec = target_spec()?;
        let runtime = global_lwc_root()?
            .join("runtime")
            .join("officecli")
            .join(VERSION)
            .join(spec.target);
        Ok(Self { runtime, spec })
    }
}

pub(crate) fn status() -> Result<Value> {
    let paths = Paths::new()?;
    let binary = runtime_binary(&paths);
    let health = if binary.is_some() {
        "ready"
    } else if fs::symlink_metadata(&paths.runtime).is_ok() {
        "invalid"
    } else {
        "missing"
    };
    Ok(json!({
        "version": VERSION,
        "runtime": paths.runtime,
        "runtime_health": health,
        "installed": binary.is_some(),
    }))
}

pub fn run(cwd: &Path, args: &[OsString]) -> Result<Value> {
    if config::resolve_office()?.setting != OfficeSetting::Officecli {
        return Err(
            AppError::new("office_disabled", "Office capability is disabled").with_details(json!({
                "configure": "lwc --scope global config set --office officecli",
            })),
        );
    }
    let verb = args
        .first()
        .and_then(|arg| arg.to_str())
        .ok_or_else(|| AppError::new("invalid_office_command", "missing OfficeCLI command"))?;
    if !matches!(
        verb,
        "view"
            | "get"
            | "query"
            | "validate"
            | "dump"
            | "raw"
            | "help"
            | "--help"
            | "-h"
            | "--version"
    ) {
        return Err(AppError::new(
            "office_command_not_read_only",
            format!("`lwc office {verb}` is unavailable because LWC only permits read-only OfficeCLI commands"),
        )
        .with_details(json!({
            "allowed": ["view", "get", "query", "validate", "dump", "raw", "help"],
        })));
    }

    let paths = Paths::new()?;
    install(&paths)?;
    let binary = runtime_binary(&paths).ok_or_else(|| {
        AppError::new(
            "office_runtime_invalid",
            "the pinned OfficeCLI runtime is invalid",
        )
    })?;
    let status = Command::new(binary)
        .args(args)
        .current_dir(cwd)
        .env("OFFICECLI_SKIP_UPDATE", "1")
        .env("OFFICECLI_NO_AUTO_RESIDENT", "1")
        .status()?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(Value::Null)
}

fn runtime_binary(paths: &Paths) -> Option<PathBuf> {
    let runtime = fs::symlink_metadata(&paths.runtime).ok()?;
    if !runtime.is_dir() || runtime.file_type().is_symlink() {
        return None;
    }
    let manifest_path = paths.runtime.join(RUNTIME_MANIFEST);
    let metadata = fs::symlink_metadata(&manifest_path).ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    let manifest: RuntimeManifest = serde_json::from_slice(&fs::read(manifest_path).ok()?).ok()?;
    if manifest.version != VERSION
        || manifest.target != paths.spec.target
        || manifest.asset != paths.spec.asset
        || manifest.sha256 != expected_sha256(&paths.spec)
        || manifest.binary.is_absolute()
        || manifest
            .binary
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    let binary = paths.runtime.join(manifest.binary);
    let metadata = fs::symlink_metadata(&binary).ok()?;
    (metadata.is_file() && !metadata.file_type().is_symlink()).then_some(binary)
}

fn install(paths: &Paths) -> Result<()> {
    if runtime_binary(paths).is_some() {
        return Ok(());
    }
    let Some(_lock) = acquire_install_lock(paths)? else {
        return Ok(());
    };
    if runtime_binary(paths).is_some() {
        return Ok(());
    }
    quarantine_invalid_runtime(paths)?;
    let parent = paths.runtime.parent().expect("runtime has a parent");
    fs::create_dir_all(parent)?;
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let staging = parent.join(format!(
        ".{}.download-{}-{suffix}",
        paths.spec.target,
        std::process::id()
    ));
    fs::create_dir(&staging)?;
    let result = (|| {
        let binary_name = if cfg!(windows) {
            "officecli.exe"
        } else {
            "officecli"
        };
        let binary = staging.join(binary_name);
        download(
            &format!("{}/{VERSION}/{}", release_root(), paths.spec.asset),
            &binary,
        )?;
        let sha256 = file_sha256(&binary)?;
        let expected = expected_sha256(&paths.spec);
        if sha256 != expected {
            return Err(AppError::new(
                "office_checksum_mismatch",
                format!("OfficeCLI checksum mismatch for {}", paths.spec.asset),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&binary)?.permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&binary, permissions)?;
        }
        let manifest = RuntimeManifest {
            version: VERSION.to_owned(),
            target: paths.spec.target.to_owned(),
            asset: paths.spec.asset.to_owned(),
            sha256,
            binary: PathBuf::from(binary_name),
        };
        fs::write(
            staging.join(RUNTIME_MANIFEST),
            serde_json::to_vec_pretty(&manifest)
                .map_err(|error| AppError::new("json_error", error.to_string()))?,
        )?;
        if fs::symlink_metadata(&paths.runtime).is_ok() {
            return Err(AppError::new(
                "office_runtime_publish_conflict",
                "OfficeCLI runtime path appeared while installation was in progress",
            ));
        }
        fs::rename(&staging, &paths.runtime)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
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
                "office_download_failed",
                format!("failed to start OfficeCLI download: {error}"),
            )
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(AppError::new(
            "office_download_failed",
            format!("failed to download {url}"),
        ))
    }
}

fn file_sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
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

fn quarantine_invalid_runtime(paths: &Paths) -> Result<()> {
    if fs::symlink_metadata(&paths.runtime).is_err() || runtime_binary(paths).is_some() {
        return Ok(());
    }
    let parent = paths.runtime.parent().expect("runtime has a parent");
    let destination = parent.join(format!(
        "{}.invalid-{}",
        paths.spec.target,
        std::process::id()
    ));
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(AppError::new(
            "office_runtime_quarantine_failed",
            "could not reserve a quarantine path for the invalid OfficeCLI runtime",
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

fn acquire_install_lock(paths: &Paths) -> Result<Option<InstallLock>> {
    let parent = paths.runtime.parent().expect("runtime has a parent");
    fs::create_dir_all(parent)?;
    let lock = parent.join(format!(".{}.install.lock", paths.spec.target));
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
                if runtime_binary(paths).is_some() {
                    return Ok(None);
                }
                if started.elapsed() >= INSTALL_LOCK_TIMEOUT {
                    return Err(AppError::new(
                        "office_install_busy",
                        "another OfficeCLI runtime installation is still in progress",
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
    if let Ok(value) = std::env::var("LWC_TEST_OFFICECLI_RELEASE_ROOT") {
        return value;
    }
    RELEASE_ROOT.to_owned()
}

fn expected_sha256(spec: &TargetSpec) -> String {
    #[cfg(debug_assertions)]
    if let Ok(value) = std::env::var("LWC_TEST_OFFICECLI_SHA256") {
        return value;
    }
    spec.sha256.to_owned()
}

fn target_spec() -> Result<TargetSpec> {
    let spec = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => TargetSpec {
            target: "mac-arm64",
            asset: "officecli-mac-arm64",
            sha256: "04757163428c5bde8d91e8f838517818e74722157722ca5f3877b6716b77bd45",
        },
        ("macos", "x86_64") => TargetSpec {
            target: "mac-x64",
            asset: "officecli-mac-x64",
            sha256: "366100643d757b0da24829422897ca74768a894b5ecd1a471a1336f8e2a0787d",
        },
        ("linux", "aarch64") => TargetSpec {
            target: "linux-arm64",
            asset: "officecli-linux-arm64",
            sha256: "56ec2c3114b66f6490888b6778cbb8413a65911a26cacc7207f29e13424966da",
        },
        ("linux", "x86_64") => TargetSpec {
            target: "linux-x64",
            asset: "officecli-linux-x64",
            sha256: "32ef7a21a54a4ca6c9806bf5e9f3d32bfb1291017329c55044cb2aac71822eb8",
        },
        ("windows", "aarch64") => TargetSpec {
            target: "win-arm64",
            asset: "officecli-win-arm64.exe",
            sha256: "0adb928d118e237b108077dadca9e272c236cd378c699712a41adda697047860",
        },
        ("windows", "x86_64") => TargetSpec {
            target: "win-x64",
            asset: "officecli-win-x64.exe",
            sha256: "e780cc6a5385f84b4d54d71b0c179904ed534125ec33fe39b1a8711fa80e387e",
        },
        (os, arch) => {
            return Err(AppError::new(
                "unsupported_office_platform",
                format!("unsupported platform {os}-{arch}"),
            ));
        }
    };
    Ok(spec)
}
