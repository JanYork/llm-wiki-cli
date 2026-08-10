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
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_TRANS_BYTES: u64 = 64 * 1024 * 1024;
const STDERR_PREVIEW_BYTES: usize = 8192;

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
    let temp_output = TempOutput::create(output_path.parent().expect("validated parent exists"))?;

    let result = run_engine(
        engine,
        &adapter_args,
        &input_path,
        temp_output.path(),
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
    let mut validated = Vec::with_capacity(args.len());
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" || is_output_flag(arg) {
            return Err(AppError::new(
                "trans_unsafe_args",
                format!("trans adapter arg is not allowed: {arg}"),
            ));
        }
        if let Some(value) = arg.strip_prefix("--format=") {
            if engine != "anydoc" || value.is_empty() {
                return Err(AppError::new(
                    "trans_unsafe_args",
                    format!("unsupported trans adapter arg for {engine}: {arg}"),
                ));
            }
            validated.push(arg.clone());
            index += 1;
            continue;
        }
        match (engine, arg.as_str()) {
            ("anydoc", "--format") => {
                let value = args.get(index + 1).ok_or_else(|| {
                    AppError::new("trans_unsafe_args", "anydoc --format requires a value")
                })?;
                if value.starts_with('-') {
                    return Err(AppError::new(
                        "trans_unsafe_args",
                        "anydoc --format value must be literal text",
                    ));
                }
                validated.push(arg.clone());
                validated.push(value.clone());
                index += 2;
            }
            ("markitdown", "--use-plugins" | "--list-plugins") => {
                validated.push(arg.clone());
                index += 1;
            }
            _ if arg.starts_with('-') => {
                return Err(AppError::new(
                    "trans_unsafe_args",
                    format!("unsupported trans adapter arg for {engine}: {arg}"),
                ));
            }
            _ => {
                return Err(AppError::new(
                    "trans_unsafe_args",
                    format!("trans adapter args must not add extra inputs: {arg}"),
                ));
            }
        }
    }
    Ok(validated)
}

fn is_output_flag(arg: &str) -> bool {
    matches!(arg, "-o" | "--output") || arg.starts_with("--output=") || arg.starts_with("-o=")
}

fn run_engine(
    engine: &str,
    adapter_args: &[String],
    input: &Path,
    temp_output: &Path,
    timeout_seconds: u16,
) -> Result<()> {
    let mut command = Command::new(engine);
    command.stdout(Stdio::null()).stderr(Stdio::piped());
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

    let stderr = spawn_stderr_reader(&mut child)?;
    wait_with_timeout(engine, &mut child, timeout_seconds, stderr)
}

fn spawn_stderr_reader(child: &mut Child) -> Result<mpsc::Receiver<io::Result<BoundedBytes>>> {
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::new("trans_failed", "stderr pipe was not available"))?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(read_bounded(stderr));
    });
    Ok(receiver)
}

fn wait_with_timeout(
    engine: &str,
    child: &mut Child,
    timeout_seconds: u16,
    stderr: mpsc::Receiver<io::Result<BoundedBytes>>,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(u64::from(timeout_seconds));
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            let bounded = stderr
                .recv()
                .unwrap_or_else(|_| Ok(BoundedBytes::default()))
                .map_err(AppError::from)?;
            return Err(AppError::new(
                "trans_timeout",
                format!("{engine} exceeded the configured timeout of {timeout_seconds} seconds"),
            )
            .with_details(stderr_details(&bounded)));
        }
        thread::sleep(Duration::from_millis(25));
    };

    let bounded = stderr
        .recv()
        .unwrap_or_else(|_| Ok(BoundedBytes::default()))
        .map_err(AppError::from)?;
    if status.success() {
        return Ok(());
    }
    Err(AppError::new(
        "trans_failed",
        format!("{engine} exited with {}", render_status(status)),
    )
    .with_details(stderr_details(&bounded)))
}

fn validate_output(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::metadata(path).map_err(|error| {
        AppError::new(
            "trans_failed",
            format!("trans engine did not produce output: {error}"),
        )
    })?;
    if !metadata.is_file() {
        return Err(AppError::new(
            "trans_failed",
            format!(
                "trans engine output is not a regular file: {}",
                path.display()
            ),
        ));
    }
    if metadata.len() == 0 {
        return Err(AppError::new(
            "trans_empty_output",
            "trans engine produced an empty output file",
        ));
    }
    if metadata.len() > MAX_TRANS_BYTES {
        return Err(AppError::new(
            "trans_output_too_large",
            format!(
                "trans output is {} bytes; maximum supported output is {MAX_TRANS_BYTES} bytes",
                metadata.len()
            ),
        ));
    }
    let bytes = fs::read(path)?;
    if bytes.is_empty() {
        return Err(AppError::new(
            "trans_empty_output",
            "trans engine produced an empty output file",
        ));
    }
    if bytes.len() as u64 > MAX_TRANS_BYTES {
        return Err(AppError::new(
            "trans_output_too_large",
            format!(
                "trans output is {} bytes; maximum supported output is {MAX_TRANS_BYTES} bytes",
                bytes.len()
            ),
        ));
    }
    std::str::from_utf8(&bytes)
        .map_err(|_| AppError::new("trans_invalid_utf8", "trans output is not valid UTF-8"))?;
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
        "stderr_preview": stderr.preview(),
        "stderr_truncated": stderr.truncated,
    })
}

fn read_bounded(mut reader: impl Read) -> io::Result<BoundedBytes> {
    let mut bounded = BoundedBytes::default();
    let mut buffer = [0u8; 4096];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bounded.push(&buffer[..read]);
    }
    Ok(bounded)
}

#[derive(Default)]
struct BoundedBytes {
    bytes: Vec<u8>,
    truncated: bool,
}

impl BoundedBytes {
    fn push(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        if self.bytes.len() + chunk.len() <= STDERR_PREVIEW_BYTES {
            self.bytes.extend_from_slice(chunk);
            return;
        }

        self.truncated = true;
        let keep = STDERR_PREVIEW_BYTES.saturating_sub(chunk.len());
        if keep == 0 {
            self.bytes.clear();
            self.bytes
                .extend_from_slice(&chunk[chunk.len() - STDERR_PREVIEW_BYTES..]);
            return;
        }
        if self.bytes.len() > keep {
            let drop = self.bytes.len() - keep;
            self.bytes.drain(..drop);
        }
        self.bytes.extend_from_slice(chunk);
    }

    fn preview(&self) -> String {
        String::from_utf8_lossy(&self.bytes).trim().to_string()
    }
}

struct TempOutput {
    path: PathBuf,
}

impl TempOutput {
    fn create(parent: &Path) -> Result<Self> {
        let path = parent.join(format!(
            ".lwc-trans-{}-{}.tmp",
            std::process::id(),
            unique_suffix()
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        options.open(&path)?;
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

impl Drop for TempOutput {
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
