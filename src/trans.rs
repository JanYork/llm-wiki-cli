use crate::config::{self, EffectiveTransConfig, TransSetting};
use crate::error::{AppError, Result};
use crate::scope::{Scope, StorePath};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_TRANS_BYTES: u64 = 64 * 1024 * 1024;

pub fn run(
    store_path: &StorePath,
    cwd: &Path,
    input: &Path,
    output: &Path,
    allow_external_source: bool,
) -> Result<Value> {
    let effective = config::resolve_trans(scope_name(store_path.scope), &store_path.path)?;
    let engine = selected_engine(&effective)?;
    let input_path = validate_input_path(store_path, input, allow_external_source)?;
    let output_path = validate_output_path(cwd, output, &input_path)?;
    let work_root = trans_work_root(&store_path.path)?;
    let temp_output = TempArtifact::create(&work_root, "output", "tmp")?;
    let result = crate::trans_adapter::run(
        &crate::trans_adapter::Config {
            engine: engine.to_owned(),
            args: args_for_engine(&effective).to_vec(),
            timeout_seconds: effective.timeout_seconds,
        },
        &input_path,
        temp_output.path(),
    )
    .map_err(|error| AppError::new(error.code, error.message).with_details(error.details));
    temp_output.finish(result)?;
    let bytes = validate_output(temp_output.path())?;
    publish_output(&output_path, &bytes)?;

    Ok(json!({
        "engine": engine,
        "input_path": input_path.display().to_string(),
        "output_path": output_path.display().to_string(),
        "output_bytes": bytes.len(),
        "output_sha256": sha256_hex(&bytes),
    }))
}

fn selected_engine(config: &EffectiveTransConfig) -> Result<&'static str> {
    match config.setting {
        TransSetting::Disabled | TransSetting::Inherit => Err(AppError::new(
            "trans_disabled",
            "trans is disabled; configure --trans anydoc or --trans markitdown first",
        )),
        TransSetting::Anydoc => Ok("anydoc"),
        TransSetting::Markitdown => Ok("markitdown"),
    }
}

fn args_for_engine(config: &EffectiveTransConfig) -> &[String] {
    match config.setting {
        TransSetting::Anydoc => &config.anydoc_args,
        TransSetting::Markitdown => &config.markitdown_args,
        TransSetting::Disabled | TransSetting::Inherit => &[],
    }
}

fn validate_input_path(
    store_path: &StorePath,
    input: &Path,
    allow_external_source: bool,
) -> Result<PathBuf> {
    let resolved = fs::canonicalize(input)?;
    if store_path.scope == Scope::Project && !allow_external_source {
        let project_root = project_root(store_path)?;
        if !resolved.starts_with(&project_root) {
            return Err(AppError::new(
                "external_source_requires_acknowledgement",
                format!(
                    "source {} resolves outside project root {}; retry with --allow-external-source only after confirming it belongs in this Wiki",
                    resolved.display(),
                    project_root.display()
                ),
            ));
        }
    }

    let metadata = fs::metadata(&resolved)?;
    if !metadata.is_file() {
        return Err(AppError::new(
            "trans_unsafe_input",
            format!("trans input must be a regular file: {}", resolved.display()),
        ));
    }
    if metadata.len() > MAX_TRANS_BYTES {
        return Err(AppError::new(
            "trans_input_too_large",
            format!(
                "trans input is {} bytes; maximum supported input is {MAX_TRANS_BYTES} bytes",
                metadata.len()
            ),
        ));
    }
    Ok(resolved)
}

fn validate_output_path(cwd: &Path, output: &Path, input: &Path) -> Result<PathBuf> {
    let absolute = if output.is_absolute() {
        output.to_path_buf()
    } else {
        cwd.join(output)
    };
    let name = absolute.file_name().ok_or_else(|| {
        AppError::new(
            "trans_unsafe_output",
            format!("trans output must name a file: {}", absolute.display()),
        )
    })?;
    let parent = absolute.parent().ok_or_else(|| {
        AppError::new(
            "trans_unsafe_output",
            format!(
                "trans output has no parent directory: {}",
                absolute.display()
            ),
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        AppError::new(
            "trans_unsafe_output",
            format!(
                "failed to prepare trans output parent for {}: {error}",
                absolute.display()
            ),
        )
    })?;
    let parent = fs::canonicalize(parent).map_err(|error| {
        AppError::new(
            "trans_unsafe_output",
            format!(
                "trans output parent is unavailable for {}: {error}",
                absolute.display()
            ),
        )
    })?;
    let resolved = parent.join(name);
    if resolved == input {
        return Err(AppError::new(
            "trans_unsafe_output",
            "trans output must not overwrite the input file",
        ));
    }
    if resolved.exists() {
        return Err(AppError::new(
            "trans_unsafe_output",
            format!("trans output already exists: {}", resolved.display()),
        ));
    }
    Ok(resolved)
}

fn validate_output(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        AppError::new(
            "trans_failed",
            format!("trans engine did not produce output: {error}"),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AppError::new(
            "trans_unsafe_output",
            format!(
                "trans engine output is not a regular file: {}",
                path.display()
            ),
        ));
    }

    let bytes = read_limited_utf8_candidate(path, MAX_TRANS_BYTES)?;
    if bytes.is_empty() {
        return Err(AppError::new(
            "trans_empty_output",
            "trans engine produced an empty output file",
        ));
    }
    std::str::from_utf8(&bytes)
        .map_err(|_| AppError::new("trans_invalid_utf8", "trans output is not valid UTF-8"))?;
    Ok(bytes)
}

fn read_limited_utf8_candidate(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    let file = fs::File::open(path).map_err(|error| {
        AppError::new(
            "trans_unsafe_output",
            format!("cannot read trans output {}: {error}", path.display()),
        )
    })?;
    let mut limited = file.take(max_bytes + 1);
    let mut bytes = Vec::new();
    limited.read_to_end(&mut bytes).map_err(|error| {
        AppError::new(
            "trans_unsafe_output",
            format!("cannot read trans output {}: {error}", path.display()),
        )
    })?;
    if bytes.len() as u64 > max_bytes {
        return Err(AppError::new(
            "trans_output_too_large",
            format!("trans output exceeds the maximum supported output of {max_bytes} bytes"),
        ));
    }
    Ok(bytes)
}

fn publish_output(destination: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(destination)
        .map_err(|error| match error.kind() {
            io::ErrorKind::AlreadyExists => AppError::new(
                "trans_publish_race",
                format!(
                    "trans output was created concurrently: {}",
                    destination.display()
                ),
            ),
            _ => AppError::new(
                "trans_unsafe_output",
                format!(
                    "failed to create trans output {}: {error}",
                    destination.display()
                ),
            ),
        })?;

    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(destination);
        return Err(AppError::new(
            "trans_unsafe_output",
            format!(
                "failed to publish trans output {}: {error}",
                destination.display()
            ),
        ));
    }
    Ok(())
}

fn trans_work_root(database: &Path) -> Result<PathBuf> {
    let lwc = database.parent().ok_or_else(|| {
        AppError::new(
            "trans_unsafe_output",
            "wiki database has no LWC directory for trans work files",
        )
    })?;
    ensure_private_directory(lwc)?;
    let work = lwc.join("work");
    ensure_private_directory(&work)?;
    let trans = work.join("trans");
    ensure_private_directory(&trans)?;
    Ok(trans)
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(AppError::new(
                "trans_unsafe_output",
                format!(
                    "trans work path is not a real directory: {}",
                    path.display()
                ),
            ))
        }
        Ok(_) => {
            set_directory_mode(path)?;
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                ensure_private_directory(parent)?;
            }
            fs::create_dir(path).map_err(|create_error| {
                AppError::new(
                    "trans_unsafe_output",
                    format!(
                        "failed to create trans work directory {}: {create_error}",
                        path.display()
                    ),
                )
            })?;
            set_directory_mode(path)?;
            Ok(())
        }
        Err(error) => Err(AppError::new(
            "trans_unsafe_output",
            format!(
                "cannot inspect trans work directory {}: {error}",
                path.display()
            ),
        )),
    }
}

fn set_directory_mode(_path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(_path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn create_private_file(path: &Path) -> Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path).map_err(|error| {
        AppError::new(
            "trans_unsafe_output",
            format!(
                "failed to create trans work file {}: {error}",
                path.display()
            ),
        )
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn project_root(store_path: &StorePath) -> Result<PathBuf> {
    let root = store_path
        .authority_path()
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| {
            AppError::new(
                "invalid_store_path",
                "project Wiki path has no project root",
            )
        })?;
    Ok(fs::canonicalize(root)?)
}

fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::Project => "project",
        Scope::Global => "global",
        Scope::All => "all",
    }
}

struct TempArtifact {
    path: PathBuf,
}

impl TempArtifact {
    fn create(root: &Path, kind: &str, extension: &str) -> Result<Self> {
        let path = root.join(format!(
            "{kind}-{}-{}.{}",
            std::process::id(),
            unique_suffix(),
            extension
        ));
        create_private_file(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn finish<T>(&self, result: Result<T>) -> Result<T> {
        let output = result;
        if output.is_err() {
            let _ = fs::remove_file(&self.path);
        }
        output
    }
}

impl Drop for TempArtifact {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn unique_suffix() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string()
}
