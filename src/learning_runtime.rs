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
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::Command,
};

const RUNTIME_MANIFEST: &str = "runtime.json";

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
            format!("{}_disabled", plugin.id()),
            format!("{} capability is disabled", plugin.id()),
        )
        .with_details(json!({
            "configure": format!("lwc --scope global config set --{} enabled", plugin.id()),
        })));
    }
    let paths = Paths::new(plugin)?;
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
    Ok(format!("{:x}", hasher.finalize()))
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
