use crate::{
    error::{AppError, Result},
    scope::{StorePath, global_lwc_root},
};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const VERSION: &str = "v1.5.0-lwc.1";
const RELEASE_ROOT: &str = "https://github.com/JanYork/codegraph/releases/download";
const RUNTIME_MANIFEST: &str = "runtime.json";
const INSTALL_LOCK_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeManifest {
    version: String,
    target: String,
    asset: String,
    archive_sha256: String,
    binary: PathBuf,
}

pub fn status(store: &StorePath) -> Result<Value> {
    let paths = Paths::new(store)?;
    let runtime = runtime_state(&paths);
    Ok(json!({
        "scope": "project",
        "version": VERSION,
        "installed": runtime.binary.is_some(),
        "runtime_health": runtime.health,
        "initialized": paths.index.join("codegraph.db").is_file(),
        "runtime": paths.runtime,
        "legacy_project_runtime": paths.legacy_runtime.exists(),
        "legacy_runtime": paths.legacy_runtime,
        "index": paths.index,
        "telemetry": false,
    }))
}

pub fn init(store: &StorePath, verbose: bool) -> Result<Value> {
    let paths = Paths::new(store)?;
    install(&paths)?;
    let mut args = vec![
        OsString::from("init"),
        OsString::from("."),
        OsString::from("--force"),
    ];
    if verbose {
        args.push(OsString::from("--verbose"));
    }
    execute(&paths, &args, true)
}

pub fn run(store: &StorePath, args: &[OsString]) -> Result<Value> {
    let paths = Paths::new(store)?;
    let Some(name) = args.first().and_then(|value| value.to_str()) else {
        return Err(AppError::new(
            "invalid_codegraph_command",
            "missing CodeGraph command",
        ));
    };
    if name == "serve" {
        if args.len() == 2 && args[1] == "--mcp" {
            return serve_mcp(&paths, args);
        }
        return Err(AppError::new(
            "codegraph_command_not_project_scoped",
            "LWC only permits the exact project-scoped `lwc cg serve --mcp` command",
        ));
    }
    if matches!(
        name,
        "install" | "uninstall" | "upgrade" | "telemetry" | "daemon" | "daemons"
    ) {
        return Err(AppError::new(
            "codegraph_command_not_project_scoped",
            format!(
                "`lwc cg {name}` is unavailable because LWC only permits project-local CodeGraph state"
            ),
        ));
    }
    if binary(&paths).is_none() {
        return Err(AppError::new(
            "codegraph_runtime_missing",
            "run `lwc cg init` to install the pinned global CodeGraph runtime",
        ));
    }
    validate_project_arguments(&paths, args)?;

    let mut forwarded = args.to_vec();
    if matches!(name, "index" | "sync" | "uninit" | "unlock") {
        if forwarded
            .iter()
            .skip(1)
            .any(|arg| !arg.to_string_lossy().starts_with('-'))
        {
            return Err(AppError::new(
                "codegraph_external_path_forbidden",
                "LWC chooses the current project path; do not pass another project path",
            ));
        }
        forwarded.insert(1, OsString::from("."));
        if matches!(name, "index" | "uninit")
            && !forwarded.iter().any(|arg| arg == "--force" || arg == "-f")
        {
            forwarded.push(OsString::from("--force"));
        }
    }
    execute(
        &paths,
        &forwarded,
        matches!(name, "index" | "sync" | "uninit" | "unlock"),
    )
}

fn serve_mcp(paths: &Paths, args: &[OsString]) -> Result<Value> {
    let status = configured_command(paths, args)?.status()?;
    if !status.success() {
        return Err(AppError::new(
            "codegraph_command_failed",
            format!("CodeGraph MCP server exited with {status}"),
        ));
    }
    Ok(Value::Null)
}

fn validate_project_arguments(paths: &Paths, args: &[OsString]) -> Result<()> {
    let mut options = true;
    for arg in args.iter().skip(1) {
        if options && arg == "--" {
            options = false;
        } else if options
            && (arg == "-p" || arg == "--path" || arg.to_string_lossy().starts_with("--path="))
        {
            return Err(external_path_error());
        }
    }

    match args.first().and_then(|arg| arg.to_str()) {
        Some("node") => {
            let mut values = args.iter().skip(1);
            while let Some(arg) = values.next() {
                if arg == "-f" || arg == "--file" {
                    let file = values.next().ok_or_else(external_path_error)?;
                    ensure_project_path(paths, file)?;
                } else if let Some(file) = arg.to_string_lossy().strip_prefix("--file=") {
                    ensure_project_path(paths, OsStr::new(file))?;
                }
            }
        }
        Some("affected") => validate_affected_paths(paths, args)?,
        _ => {}
    }
    Ok(())
}

fn validate_affected_paths(paths: &Paths, args: &[OsString]) -> Result<()> {
    let mut values = args.iter().skip(1);
    let mut positional = false;
    while let Some(arg) = values.next() {
        if arg == "--stdin" {
            return Err(external_path_error());
        }
        if !positional && arg == "--" {
            positional = true;
            continue;
        }
        if !positional && matches!(arg.to_str(), Some("-d" | "--depth" | "-f" | "--filter")) {
            values.next();
            continue;
        }
        if !positional && arg.to_string_lossy().starts_with('-') {
            continue;
        }
        ensure_project_path(paths, arg)?;
    }
    Ok(())
}

fn ensure_project_path(paths: &Paths, value: &OsStr) -> Result<()> {
    let path = Path::new(value);
    let relative = if path.is_absolute() {
        path.strip_prefix(&paths.project)
            .map_err(|_| external_path_error())?
    } else {
        path
    };
    let mut depth = 0_usize;
    for component in relative.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::ParentDir if depth > 0 => depth -= 1,
            Component::CurDir => {}
            _ => return Err(external_path_error()),
        }
    }
    let candidate = paths.project.join(relative);
    if candidate.exists() && !fs::canonicalize(candidate)?.starts_with(&paths.project) {
        return Err(external_path_error());
    }
    Ok(())
}

fn external_path_error() -> AppError {
    AppError::new(
        "codegraph_external_path_forbidden",
        "LWC only permits CodeGraph paths inside the current project",
    )
}

pub fn graph(store: &StorePath) -> Value {
    let paths = match Paths::new(store) {
        Ok(paths) => paths,
        Err(error) => {
            return json!({
                "available": false,
                "nodes": [],
                "edges": [],
                "message": error.message,
            });
        }
    };
    let database = paths.index.join("codegraph.db");
    if !database.is_file() {
        return json!({
            "available": false,
            "nodes": [],
            "edges": [],
            "message": "Run `lwc cg init` to build the project-local code index."
        });
    }
    match read_graph(&database) {
        Ok(value) => value,
        Err(error) => json!({
            "available": false,
            "nodes": [],
            "edges": [],
            "message": error.message,
        }),
    }
}

fn read_graph(database: &Path) -> Result<Value> {
    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let mut node_statement = connection.prepare(
        "SELECT id, name, kind, file_path FROM nodes ORDER BY file_path, start_line, id LIMIT 1000",
    )?;
    let nodes = node_statement
        .query_map([], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "label": row.get::<_, String>(1)?,
                "kind": row.get::<_, String>(2)?,
                "file": row.get::<_, String>(3)?,
            }))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut edge_statement = connection.prepare(
        "WITH selected AS (
           SELECT id FROM nodes ORDER BY file_path, start_line, id LIMIT 1000
         )
         SELECT e.id, e.source, e.target, e.kind
         FROM edges e
         JOIN selected source ON source.id = e.source
         JOIN selected target ON target.id = e.target
         ORDER BY e.id LIMIT 5000",
    )?;
    let edges = edge_statement
        .query_map([], |row| {
            Ok(json!({
                "id": row.get::<_, i64>(0)?.to_string(),
                "source": row.get::<_, String>(1)?,
                "target": row.get::<_, String>(2)?,
                "type": row.get::<_, String>(3)?,
            }))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(json!({
        "available": true,
        "nodes": nodes,
        "edges": edges,
        "limits": {"nodes": 1000, "edges": 5000},
    }))
}

struct Paths {
    project: PathBuf,
    runtime: PathBuf,
    legacy_runtime: PathBuf,
    index: PathBuf,
    target: &'static str,
    asset: String,
}

impl Paths {
    fn new(store: &StorePath) -> Result<Self> {
        let lwc = store
            .path
            .parent()
            .expect("Wiki database has an .lwc parent");
        Self::from_project(
            lwc.parent()
                .expect("project .lwc has a parent")
                .to_path_buf(),
        )
    }

    fn from_project(project: PathBuf) -> Result<Self> {
        let target = target_name()?;
        let extension = if cfg!(windows) { "zip" } else { "tar.gz" };
        let project_lwc = project.join(".lwc");
        Ok(Self {
            legacy_runtime: project_lwc.join("runtime").join("codegraph"),
            index: project_lwc.join("codegraph"),
            project,
            runtime: global_lwc_root()?
                .join("runtime")
                .join("codegraph")
                .join(VERSION)
                .join(target),
            target,
            asset: format!("codegraph-{target}.{extension}"),
        })
    }
}

pub fn prompt_hook(project: &Path, prompt: &str) -> Result<String> {
    const MAX_OUTPUT_BYTES: u64 = 20 * 1024;
    const TIMEOUT: Duration = Duration::from_secs(2);

    let project = fs::canonicalize(project)?;
    let paths = Paths::from_project(project.clone())?;
    let mut command = configured_command(&paths, &[OsString::from("prompt-hook")])?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().expect("piped CodeGraph stdout");
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(MAX_OUTPUT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    if let Some(mut stdin) = child.stdin.take() {
        serde_json::to_writer(&mut stdin, &json!({"prompt": prompt, "cwd": project})).map_err(
            |error| {
                AppError::new(
                    "codegraph_prompt_hook_failed",
                    format!("failed to write CodeGraph prompt input: {error}"),
                )
            },
        )?;
    }
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AppError::new(
                "codegraph_prompt_hook_timeout",
                "CodeGraph prompt hook exceeded 2 seconds",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let bytes = reader
        .join()
        .map_err(|_| AppError::new("codegraph_prompt_hook_failed", "output reader failed"))??;
    if !status.success() || bytes.len() > MAX_OUTPUT_BYTES as usize {
        return Err(AppError::new(
            "codegraph_prompt_hook_failed",
            "CodeGraph prompt hook failed or exceeded its output budget",
        ));
    }
    String::from_utf8(bytes).map_err(|_| {
        AppError::new(
            "codegraph_prompt_hook_failed",
            "CodeGraph prompt hook returned invalid UTF-8",
        )
    })
}

fn execute(paths: &Paths, args: &[OsString], stream: bool) -> Result<Value> {
    let mut command = configured_command(paths, args)?;
    let (status, stdout, stderr) = if stream {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let stdout = stream_lines(child.stdout.take().expect("piped stdout"));
        let stderr = stream_lines(child.stderr.take().expect("piped stderr"));
        let status = child.wait()?;
        (
            status,
            stdout.join().unwrap_or_default(),
            stderr.join().unwrap_or_default(),
        )
    } else {
        let output = command.output()?;
        (
            output.status,
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        )
    };
    if !status.success() {
        return Err(AppError::new(
            "codegraph_command_failed",
            format!("CodeGraph exited with {status}"),
        )
        .with_details(json!({"stdout": stdout, "stderr": stderr})));
    }
    Ok(json!({
        "scope": "project",
        "command": args.iter().map(|arg| arg.to_string_lossy()).collect::<Vec<_>>(),
        "stdout": stdout,
        "stderr": stderr,
        "telemetry": false,
    }))
}

fn configured_command(paths: &Paths, args: &[OsString]) -> Result<Command> {
    let executable = binary(paths)
        .ok_or_else(|| AppError::new("codegraph_runtime_missing", "run `lwc cg init` first"))?;
    let home = paths.runtime.join("home");
    fs::create_dir_all(&home)?;
    let mut command = Command::new(&executable);
    command
        .args(args)
        .current_dir(&paths.project)
        .env("CODEGRAPH_DIR", ".lwc/codegraph")
        .env("CODEGRAPH_TELEMETRY", "0")
        .env("DO_NOT_TRACK", "1")
        .env("NO_COLOR", "1")
        .env("HOME", &home)
        .env("USERPROFILE", &home);
    Ok(command)
}

pub(crate) fn mcp_command(project: &Path) -> Result<Command> {
    let paths = Paths::from_project(fs::canonicalize(project)?)?;
    for directory in [paths.project.join(".lwc"), paths.index.clone()] {
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(AppError::new(
                    "codegraph_index_invalid",
                    "CodeGraph index directories must be real project-local directories",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(AppError::new(
                    "codegraph_index_missing",
                    "run `lwc cg init` to build the project code index",
                ));
            }
            Err(error) => return Err(error.into()),
        }
    }
    let database = paths.index.join("codegraph.db");
    match fs::symlink_metadata(&database) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(AppError::new(
                "codegraph_index_invalid",
                "CodeGraph database must be a regular project-local file",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(AppError::new(
                "codegraph_index_missing",
                "run `lwc cg init` to build the project code index",
            ));
        }
        Err(error) => return Err(error.into()),
    }
    let executable = binary(&paths).ok_or_else(|| {
        AppError::new(
            "codegraph_runtime_missing",
            "run `lwc cg init` to install the pinned CodeGraph runtime",
        )
    })?;
    let home = paths.runtime.join("home");
    let mut command = Command::new(executable);
    command
        .args(["serve", "--mcp"])
        .current_dir(&paths.project)
        .env("CODEGRAPH_DIR", ".lwc/codegraph")
        .env("CODEGRAPH_TELEMETRY", "0")
        .env("DO_NOT_TRACK", "1")
        .env("NO_COLOR", "1")
        .env("HOME", &home)
        .env("USERPROFILE", &home);
    Ok(command)
}

fn stream_lines(pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut captured = String::new();
        for line in BufReader::new(pipe)
            .lines()
            .map_while(std::result::Result::ok)
        {
            eprintln!("{line}");
            if !captured.is_empty() {
                captured.push('\n');
            }
            captured.push_str(&line);
        }
        captured
    })
}

fn install(paths: &Paths) -> Result<()> {
    if binary(paths).is_some() {
        return Ok(());
    }
    let Some(_lock) = acquire_install_lock(paths)? else {
        return Ok(());
    };
    if binary(paths).is_some() {
        return Ok(());
    }
    quarantine_invalid_runtime(paths)?;
    let parent = paths.runtime.parent().expect("runtime has a parent");
    fs::create_dir_all(parent)?;
    let staging = parent.join(format!("codegraph.download-{}", std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir(&staging)?;
    let result = (|| {
        let archive = staging.join(&paths.asset);
        let sums = staging.join("SHA256SUMS");
        download(
            &format!("{RELEASE_ROOT}/{VERSION}/{}", paths.asset),
            &archive,
        )?;
        download(&format!("{RELEASE_ROOT}/{VERSION}/SHA256SUMS"), &sums)?;
        let archive_sha256 = verify(&archive, &sums, &paths.asset)?;
        unpack(&archive, &staging)?;
        fs::remove_file(&archive)?;
        fs::remove_file(&sums)?;
        let executable = find_binary(&staging, 0).ok_or_else(|| {
            AppError::new(
                "codegraph_runtime_invalid",
                "downloaded CodeGraph runtime has no executable",
            )
        })?;
        let relative = executable
            .strip_prefix(&staging)
            .expect("staging executable is inside staging")
            .to_path_buf();
        let manifest = RuntimeManifest {
            version: VERSION.to_owned(),
            target: paths.target.to_owned(),
            asset: paths.asset.clone(),
            archive_sha256,
            binary: relative,
        };
        let manifest = serde_json::to_vec_pretty(&manifest).map_err(|error| {
            AppError::new(
                "codegraph_runtime_invalid",
                format!("failed to encode CodeGraph runtime manifest: {error}"),
            )
        })?;
        fs::write(staging.join(RUNTIME_MANIFEST), manifest)?;
        if fs::symlink_metadata(&paths.runtime).is_ok() {
            return Err(AppError::new(
                "codegraph_runtime_publish_conflict",
                "CodeGraph runtime path appeared while installation was in progress",
            ));
        }
        fs::rename(&staging, &paths.runtime)?;
        Ok(())
    })();
    if result.is_err() && staging.exists() {
        let _ = fs::remove_dir_all(staging);
    }
    result
}

fn quarantine_invalid_runtime(paths: &Paths) -> Result<Option<PathBuf>> {
    if fs::symlink_metadata(&paths.runtime).is_err() || runtime_state(paths).binary.is_some() {
        return Ok(None);
    }
    let parent = paths.runtime.parent().expect("runtime has a parent");
    for attempt in 0..1000_u16 {
        let suffix = if attempt == 0 {
            format!("{}.invalid-{}", paths.target, std::process::id())
        } else {
            format!("{}.invalid-{}-{attempt}", paths.target, std::process::id())
        };
        let destination = parent.join(suffix);
        if fs::symlink_metadata(&destination).is_ok() {
            continue;
        }
        fs::rename(&paths.runtime, &destination)?;
        return Ok(Some(destination));
    }
    Err(AppError::new(
        "codegraph_runtime_quarantine_failed",
        "could not reserve a quarantine path for the invalid CodeGraph runtime",
    ))
}

struct InstallLock {
    path: PathBuf,
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn acquire_install_lock(paths: &Paths) -> Result<Option<InstallLock>> {
    let parent = paths.runtime.parent().expect("runtime has a parent");
    fs::create_dir_all(parent)?;
    let path = parent.join(format!(".{}.install.lock", paths.target));
    let started = Instant::now();
    loop {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => {
                writeln!(file, "pid={}", std::process::id())?;
                file.sync_all()?;
                return Ok(Some(InstallLock { path }));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if binary(paths).is_some() {
                    return Ok(None);
                }
                if started.elapsed() >= INSTALL_LOCK_TIMEOUT {
                    return Err(AppError::new(
                        "codegraph_install_busy",
                        "another CodeGraph runtime installation is still in progress",
                    ));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn download(url: &str, destination: &Path) -> Result<()> {
    let status = Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--output",
        ])
        .arg(destination)
        .arg(url)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(AppError::new(
            "codegraph_download_failed",
            format!("failed to download {url}"),
        ))
    }
}

fn verify(archive: &Path, sums: &Path, asset: &str) -> Result<String> {
    let expected = fs::read_to_string(sums)?
        .lines()
        .find_map(|line| {
            let (hash, name) = line.split_once(char::is_whitespace)?;
            (name.trim_start_matches([' ', '*']) == asset).then(|| hash.to_ascii_lowercase())
        })
        .ok_or_else(|| {
            AppError::new(
                "codegraph_checksum_missing",
                format!("{asset} is absent from SHA256SUMS"),
            )
        })?;
    let mut file = fs::File::open(archive)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let actual = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual == expected {
        Ok(expected)
    } else {
        Err(AppError::new(
            "codegraph_checksum_mismatch",
            format!("checksum mismatch for {asset}"),
        ))
    }
}

fn unpack(archive: &Path, destination: &Path) -> Result<()> {
    let status = if cfg!(windows) {
        Command::new("powershell")
            .args(["-NoProfile", "-Command", "Expand-Archive", "-LiteralPath"])
            .arg(archive)
            .args(["-DestinationPath"])
            .arg(destination)
            .status()?
    } else {
        Command::new("tar")
            .arg("-xzf")
            .arg(archive)
            .arg("-C")
            .arg(destination)
            .status()?
    };
    if status.success() {
        Ok(())
    } else {
        Err(AppError::new(
            "codegraph_unpack_failed",
            "failed to unpack CodeGraph runtime",
        ))
    }
}

fn binary(paths: &Paths) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("LWC_CODEGRAPH_BINARY") {
        return Some(PathBuf::from(path));
    }
    runtime_state(paths).binary
}

struct RuntimeState {
    health: &'static str,
    binary: Option<PathBuf>,
}

fn runtime_state(paths: &Paths) -> RuntimeState {
    let Ok(runtime_metadata) = fs::symlink_metadata(&paths.runtime) else {
        return RuntimeState {
            health: "missing",
            binary: None,
        };
    };
    if !runtime_metadata.is_dir() || runtime_metadata.file_type().is_symlink() {
        return RuntimeState {
            health: "invalid",
            binary: None,
        };
    }
    let manifest_path = paths.runtime.join(RUNTIME_MANIFEST);
    let Ok(metadata) = fs::symlink_metadata(&manifest_path) else {
        return RuntimeState {
            health: "invalid",
            binary: None,
        };
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return RuntimeState {
            health: "invalid",
            binary: None,
        };
    }
    let Ok(bytes) = fs::read(&manifest_path) else {
        return RuntimeState {
            health: "invalid",
            binary: None,
        };
    };
    let Ok(manifest) = serde_json::from_slice::<RuntimeManifest>(&bytes) else {
        return RuntimeState {
            health: "invalid",
            binary: None,
        };
    };
    if manifest.version != VERSION
        || manifest.target != paths.target
        || manifest.asset != paths.asset
        || manifest.archive_sha256.len() != 64
        || !manifest
            .archive_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || manifest.binary.is_absolute()
        || manifest
            .binary
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return RuntimeState {
            health: "invalid",
            binary: None,
        };
    }
    let executable = paths.runtime.join(&manifest.binary);
    let Ok(metadata) = fs::symlink_metadata(&executable) else {
        return RuntimeState {
            health: "invalid",
            binary: None,
        };
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return RuntimeState {
            health: "invalid",
            binary: None,
        };
    }
    RuntimeState {
        health: "ready",
        binary: Some(executable),
    }
}

fn find_binary(directory: &Path, depth: usize) -> Option<PathBuf> {
    if depth > 3 {
        return None;
    }
    for entry in fs::read_dir(directory).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_binary(&path, depth + 1) {
                return Some(found);
            }
        } else if path.file_name()
            == Some(OsStr::new(if cfg!(windows) {
                "codegraph.cmd"
            } else {
                "codegraph"
            }))
            && path.parent()?.file_name() == Some(OsStr::new("bin"))
        {
            return Some(path);
        }
    }
    None
}

fn target_name() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("darwin-arm64"),
        ("macos", "x86_64") => Ok("darwin-x64"),
        ("linux", "aarch64") => Ok("linux-arm64"),
        ("linux", "x86_64") => Ok("linux-x64"),
        ("windows", "aarch64") => Ok("win32-arm64"),
        ("windows", "x86_64") => Ok("win32-x64"),
        (os, arch) => Err(AppError::new(
            "unsupported_codegraph_platform",
            format!("unsupported platform {os}-{arch}"),
        )),
    }
}
