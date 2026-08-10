use crate::config::{self, EffectiveTransConfig, TransSetting};
use crate::error::{AppError, Result};
use crate::scope::{Scope, StorePath};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_TRANS_BYTES: u64 = 64 * 1024 * 1024;
const STDERR_PREVIEW_BYTES: u64 = 8192;
const ESRCH: i32 = 3;

struct OptionSpec {
    names: &'static [&'static str],
    takes_value: bool,
}

const ANYDOC_OPTIONS: &[OptionSpec] = &[OptionSpec {
    names: &["--format"],
    takes_value: true,
}];

const MARKITDOWN_OPTIONS: &[OptionSpec] = &[
    OptionSpec {
        names: &["-v", "--version"],
        takes_value: false,
    },
    OptionSpec {
        names: &["-x", "--extension"],
        takes_value: true,
    },
    OptionSpec {
        names: &["-m", "--mime-type"],
        takes_value: true,
    },
    OptionSpec {
        names: &["-c", "--charset"],
        takes_value: true,
    },
    OptionSpec {
        names: &["-d", "--use-docintel"],
        takes_value: false,
    },
    OptionSpec {
        names: &["--use-cu", "--use-content-understanding"],
        takes_value: false,
    },
    OptionSpec {
        names: &["-e", "--endpoint"],
        takes_value: true,
    },
    OptionSpec {
        names: &["--cu-endpoint"],
        takes_value: true,
    },
    OptionSpec {
        names: &["--cu-analyzer"],
        takes_value: true,
    },
    OptionSpec {
        names: &["--cu-file-types"],
        takes_value: true,
    },
    OptionSpec {
        names: &["-p", "--use-plugins"],
        takes_value: false,
    },
    OptionSpec {
        names: &["--list-plugins"],
        takes_value: false,
    },
    OptionSpec {
        names: &["--keep-data-uris"],
        takes_value: false,
    },
];

pub fn run(
    store_path: &StorePath,
    cwd: &Path,
    input: &Path,
    output: &Path,
    allow_external_source: bool,
) -> Result<Value> {
    let effective = config::resolve_trans(scope_name(store_path.scope), &store_path.path)?;
    let engine = selected_engine(&effective)?;
    let adapter_args = validate_adapter_args(engine, args_for_engine(&effective))?;
    let input_path = validate_input_path(store_path, input, allow_external_source)?;
    let output_path = validate_output_path(cwd, output, &input_path)?;
    let work_root = trans_work_root(&store_path.path)?;
    let temp_output = TempArtifact::create(&work_root, "output", "tmp")?;
    let temp_stderr = TempArtifact::create(&work_root, "stderr", "log")?;

    let result = run_engine(
        engine,
        &adapter_args,
        &input_path,
        temp_output.path(),
        temp_stderr.path(),
        effective.timeout_seconds,
    );
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

fn validate_adapter_args(engine: &str, args: &[String]) -> Result<Vec<String>> {
    let specs = option_specs(engine);
    let mut validated = Vec::with_capacity(args.len());
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            return Err(AppError::new(
                "trans_unsafe_args",
                "trans adapter args must not inject a positional separator",
            ));
        }
        if !arg.starts_with('-') {
            return Err(AppError::new(
                "trans_unsafe_args",
                format!("trans adapter args must not add extra inputs: {arg}"),
            ));
        }
        if let Some((name, value)) = split_inline_value(arg) {
            if is_output_flag(name) {
                return Err(AppError::new(
                    "trans_unsafe_args",
                    format!("trans adapter arg is not allowed: {arg}"),
                ));
            }
            let spec = lookup_option(specs, name).ok_or_else(|| {
                AppError::new(
                    "trans_unsafe_args",
                    format!("unsupported trans adapter arg for {engine}: {name}"),
                )
            })?;
            if !spec.takes_value || value.is_empty() {
                return Err(AppError::new(
                    "trans_unsafe_args",
                    format!("unsupported trans adapter arg for {engine}: {arg}"),
                ));
            }
            validated.push(arg.clone());
            index += 1;
            continue;
        }

        if is_output_flag(arg) {
            return Err(AppError::new(
                "trans_unsafe_args",
                format!("trans adapter arg is not allowed: {arg}"),
            ));
        }

        let spec = lookup_option(specs, arg).ok_or_else(|| {
            AppError::new(
                "trans_unsafe_args",
                format!("unsupported trans adapter arg for {engine}: {arg}"),
            )
        })?;
        validated.push(arg.clone());
        if spec.takes_value {
            let value = args.get(index + 1).ok_or_else(|| {
                AppError::new(
                    "trans_unsafe_args",
                    format!("{arg} requires a value for {engine}"),
                )
            })?;
            if value == "--" {
                return Err(AppError::new(
                    "trans_unsafe_args",
                    format!("{arg} must not inject a positional separator"),
                ));
            }
            validated.push(value.clone());
            index += 2;
        } else {
            index += 1;
        }
    }
    Ok(validated)
}

fn option_specs(engine: &str) -> &'static [OptionSpec] {
    match engine {
        "anydoc" => ANYDOC_OPTIONS,
        "markitdown" => MARKITDOWN_OPTIONS,
        _ => &[],
    }
}

fn lookup_option<'a>(options: &'a [OptionSpec], name: &str) -> Option<&'a OptionSpec> {
    options.iter().find(|spec| spec.names.contains(&name))
}

fn split_inline_value(arg: &str) -> Option<(&str, &str)> {
    if !arg.starts_with("--") {
        return None;
    }
    arg.split_once('=')
}

fn is_output_flag(arg: &str) -> bool {
    matches!(arg, "-o" | "--output")
}

fn run_engine(
    engine: &str,
    adapter_args: &[String],
    input: &Path,
    temp_output: &Path,
    temp_stderr: &Path,
    timeout_seconds: u16,
) -> Result<()> {
    let stderr_file = open_existing_private_file(temp_stderr)?;
    let mut command = Command::new(engine);
    command.stdin(Stdio::null()).stdout(Stdio::null());
    command.stderr(Stdio::from(stderr_file));
    #[cfg(unix)]
    command.process_group(0);
    command.args(adapter_args);
    command.arg(input);
    command.arg("-o");
    command.arg(temp_output);
    let mut child = command.spawn().map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => AppError::new(
            "trans_executable_missing",
            format!("{engine} is not installed or not on PATH"),
        ),
        _ => AppError::new("trans_failed", format!("failed to start {engine}: {error}")),
    })?;

    wait_with_timeout(engine, &mut child, timeout_seconds, temp_stderr)
}

fn wait_with_timeout(
    engine: &str,
    child: &mut Child,
    timeout_seconds: u16,
    stderr_path: &Path,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(u64::from(timeout_seconds));
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            terminate_process_tree(child)?;
            child.wait()?;
            let stderr = read_bounded_file(stderr_path, STDERR_PREVIEW_BYTES)?;
            return Err(AppError::new(
                "trans_timeout",
                format!("{engine} exceeded the configured timeout of {timeout_seconds} seconds"),
            )
            .with_details(stderr_details(&stderr)));
        }
        thread::sleep(Duration::from_millis(25));
    };

    let stderr = read_bounded_file(stderr_path, STDERR_PREVIEW_BYTES)?;
    if status.success() {
        return Ok(());
    }
    Err(AppError::new(
        "trans_failed",
        format!("{engine} exited with {}", render_status(status)),
    )
    .with_details(stderr_details(&stderr)))
}

#[cfg(unix)]
fn terminate_process_tree(child: &mut Child) -> io::Result<()> {
    if let Err(error) = kill_process_group(child.id())
        && error.raw_os_error() != Some(ESRCH)
    {
        let _ = child.kill();
    }
    Ok(())
}

#[cfg(windows)]
fn terminate_process_tree(child: &mut Child) -> io::Result<()> {
    let pid = child.id().to_string();
    let taskkill_path = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|root| root.join("System32").join("taskkill.exe"))
        .unwrap_or_else(|| PathBuf::from("taskkill.exe"));
    let mut taskkill = Command::new(taskkill_path);
    let taskkill = taskkill
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .args(["/PID", pid.as_str(), "/T", "/F"])
        .status();
    if matches!(taskkill, Ok(status) if status.success()) {
        return Ok(());
    }

    match child.kill() {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => Ok(()),
        Err(error) => {
            if child.try_wait()?.is_some() {
                Ok(())
            } else {
                Err(error)
            }
        }
    }
}

#[cfg(all(not(unix), not(windows)))]
fn terminate_process_tree(child: &mut Child) -> io::Result<()> {
    match child.kill() {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => Ok(()),
        Err(error) => {
            if child.try_wait()?.is_some() {
                Ok(())
            } else {
                Err(error)
            }
        }
    }
}

#[cfg(unix)]
fn kill_process_group(pid: u32) -> io::Result<()> {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }

    const SIGKILL: i32 = 9;
    let result = unsafe { kill(-(pid as i32), SIGKILL) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ESRCH) {
        return Ok(());
    }
    Err(error)
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

fn set_directory_mode(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
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

fn open_existing_private_file(path: &Path) -> Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(|error| {
            AppError::new(
                "trans_unsafe_output",
                format!("failed to open trans work file {}: {error}", path.display()),
            )
        })
}

fn read_bounded_file(path: &Path, limit: u64) -> Result<BoundedBytes> {
    match fs::File::open(path) {
        Ok(file) => {
            let mut limited = file.take(limit + 1);
            let mut bytes = Vec::new();
            limited.read_to_end(&mut bytes).map_err(AppError::from)?;
            let truncated = bytes.len() as u64 > limit;
            if truncated {
                bytes.truncate(limit as usize);
            }
            Ok(BoundedBytes { bytes, truncated })
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(BoundedBytes::default()),
        Err(error) => Err(error.into()),
    }
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

fn render_status(status: ExitStatus) -> String {
    status
        .code()
        .map(|code| format!("exit status {code}"))
        .unwrap_or_else(|| "a terminating signal".to_string())
}

fn stderr_details(stderr: &BoundedBytes) -> Value {
    json!({
        "stderr_present": !stderr.bytes.is_empty(),
        "stderr_truncated": stderr.truncated,
    })
}

#[derive(Default)]
struct BoundedBytes {
    bytes: Vec<u8>,
    truncated: bool,
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
