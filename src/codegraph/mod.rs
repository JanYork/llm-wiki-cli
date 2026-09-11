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
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const VERSION: &str = "v1.5.0-lwc.1";
const RELEASE_ROOT: &str = "https://github.com/JanYork/codegraph/releases/download";
const RUNTIME_MANIFEST: &str = "runtime.json";
const INSTALL_LOCK_TIMEOUT: Duration = Duration::from_secs(30);
const PROMPT_HOOK_TIMEOUT: Duration = Duration::from_secs(2);
const PROMPT_HOOK_CLEANUP_RESERVE: Duration = Duration::from_millis(100);
const PROMPT_HOOK_MAX_OUTPUT_BYTES: u64 = 20 * 1024;

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
        "version": if paths.external.is_some() { Value::Null } else { json!(VERSION) },
        "owner": if paths.external.is_some() { "independent" } else { "lwc" },
        "checkout": paths.project,
        "executable": runtime.binary,
        "freshness": "unknown",
        "coverage": "unknown",
        "installed": runtime.binary.is_some(),
        "runtime_health": runtime.health,
        "initialized": validate_index(&paths).is_ok(),
        "runtime": paths.external.as_ref().and_then(|path| path.parent()).unwrap_or(&paths.runtime),
        "legacy_project_runtime": paths.legacy_runtime.exists(),
        "legacy_runtime": paths.legacy_runtime,
        "index": paths.index,
        "telemetry": false,
    }))
}

pub fn init(store: &StorePath, verbose: bool) -> Result<Value> {
    let paths = Paths::new(store)?;
    if paths.external.is_none() {
        install(&paths)?;
    }
    let mut args = vec![
        OsString::from("init"),
        OsString::from("."),
        OsString::from("--force"),
    ];
    if verbose {
        args.push(OsString::from("--verbose"));
    }
    execute(&paths, &args)
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
    let mut forwarded = args.to_vec();
    if name == "affected" && forwarded.iter().any(|arg| arg == "--stdin") {
        let mut input = String::new();
        io::stdin()
            .take(1024 * 1024 + 1)
            .read_to_string(&mut input)?;
        if input.len() > 1024 * 1024 {
            return Err(AppError::new(
                "invalid_input",
                "affected file list exceeds 1 MiB",
            ));
        }
        forwarded.retain(|arg| arg != "--stdin");
        for line in input.lines().filter(|line| !line.is_empty()) {
            if line.starts_with('-') {
                return Err(external_path_error());
            }
            ensure_project_path(&paths, OsStr::new(line))?;
            forwarded.push(OsString::from(line));
        }
    }
    validate_project_arguments(&paths, &forwarded)?;
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
    execute(&paths, &forwarded)
}

fn serve_mcp(paths: &Paths, args: &[OsString]) -> Result<Value> {
    execute(paths, args)
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

fn project_relative_path(paths: &Paths, path: &Path) -> Result<PathBuf> {
    let root = fs::canonicalize(&paths.project)?;
    #[cfg(windows)]
    if let Some(Component::Prefix(prefix)) = path.components().next()
        && !matches!(
            prefix.kind(),
            std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
        )
    {
        return Err(external_path_error());
    }
    let relative = if path.is_absolute() {
        // Resolve through the filesystem so Windows casing and local verbatim paths agree.
        // Missing files retain their suffix beneath the nearest existing ancestor.
        let mut existing = path;
        while !existing.try_exists()? {
            existing = existing.parent().ok_or_else(external_path_error)?;
        }
        let mut resolved = fs::canonicalize(existing)?;
        for component in path
            .strip_prefix(existing)
            .map_err(|_| external_path_error())?
            .components()
        {
            resolved.push(component.as_os_str());
        }
        #[cfg(windows)]
        let boundary = resolved
            .ancestors()
            .find(|ancestor| same_file::is_same_file(ancestor, &root).unwrap_or(false))
            .ok_or_else(external_path_error)?;
        #[cfg(not(windows))]
        let boundary = root.as_path();
        resolved
            .strip_prefix(boundary)
            .map_err(|_| external_path_error())?
            .to_path_buf()
    } else {
        path.to_path_buf()
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
    crate::scope::ensure_project_path(&root.join(&relative), &root)
        .map_err(|_| external_path_error())?;
    Ok(relative)
}

fn ensure_project_path(paths: &Paths, value: &OsStr) -> Result<()> {
    project_relative_path(paths, Path::new(value)).map(|_| ())
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
    external: Option<PathBuf>,
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
        if fs::symlink_metadata(&project_lwc).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(AppError::new(
                "codegraph_index_invalid",
                "project .lwc must not be a symlink",
            ));
        }
        let external = crate::config::load_file(&crate::config::config_path_for_database(
            &project_lwc.join("wiki.db"),
        )?)?
        .codegraph_executable;
        if external.as_ref().is_some_and(|p| !p.is_absolute()) {
            return Err(AppError::new(
                "invalid_codegraph_runtime",
                "configured executable must be an absolute path",
            ));
        }
        Ok(Self {
            legacy_runtime: project_lwc.join("runtime").join("codegraph"),
            index: if external.is_some() {
                project.join(".codegraph")
            } else {
                project_lwc.join("codegraph")
            },
            external,
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

#[allow(dead_code)] // Compatibility entry point; Agent hooks pass the remaining wall budget.
pub fn prompt_hook(project: &Path, prompt: &str) -> Result<String> {
    prompt_hook_with_budget(project, prompt, PROMPT_HOOK_TIMEOUT)
}

pub fn prompt_hook_with_budget(project: &Path, prompt: &str, budget: Duration) -> Result<String> {
    let budget = budget.min(PROMPT_HOOK_TIMEOUT);
    if budget.is_zero() {
        return Err(prompt_hook_timeout(budget));
    }
    let deadline = Instant::now() + budget;
    let project = fs::canonicalize(project)?;
    let paths = Paths::from_project(project.clone())?;
    let command = configured_prompt_hook_command(&paths, &[OsString::from("prompt-hook")])?;
    run_prompt_hook_until(command, &project, prompt, deadline, budget)
}

fn run_prompt_hook_until(
    mut command: Command,
    project: &Path,
    prompt: &str,
    deadline: Instant,
    budget: Duration,
) -> Result<String> {
    if Instant::now() >= deadline {
        return Err(prompt_hook_timeout(budget));
    }
    let payload =
        serde_json::to_vec(&json!({"prompt": prompt, "cwd": project})).map_err(|error| {
            AppError::new(
                "codegraph_prompt_hook_failed",
                format!("failed to encode CodeGraph prompt input: {error}"),
            )
        })?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().expect("piped CodeGraph stdout");
    let (reader_sender, reader_receiver) = std::sync::mpsc::sync_channel(1);
    let _reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(PROMPT_HOOK_MAX_OUTPUT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = reader_sender.send(result);
    });
    let (writer_sender, writer_receiver) = std::sync::mpsc::sync_channel(1);
    let _writer = child.stdin.take().map(|mut stdin| {
        std::thread::spawn(move || {
            let result = stdin.write_all(&payload);
            let _ = writer_sender.send(result);
        })
    });
    let status = wait_for_prompt_hook_exit(&mut child, deadline, budget)?;

    let writer_result =
        receive_prompt_hook_result(&writer_receiver, deadline).ok_or_else(|| {
            terminate_prompt_hook(&mut child, deadline);
            prompt_hook_timeout(budget)
        })?;
    writer_result.map_err(|error| {
        AppError::new(
            "codegraph_prompt_hook_failed",
            format!("failed to write CodeGraph prompt input: {error}"),
        )
    })?;
    let reader_result =
        receive_prompt_hook_result(&reader_receiver, deadline).ok_or_else(|| {
            terminate_prompt_hook(&mut child, deadline);
            prompt_hook_timeout(budget)
        })?;
    let bytes = reader_result
        .map_err(|_| AppError::new("codegraph_prompt_hook_failed", "output reader failed"))?;
    if !status.success() || bytes.len() > PROMPT_HOOK_MAX_OUTPUT_BYTES as usize {
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

fn wait_for_prompt_hook_exit(
    child: &mut Child,
    deadline: Instant,
    budget: Duration,
) -> Result<std::process::ExitStatus> {
    let cleanup_reserve = deadline
        .saturating_duration_since(Instant::now())
        .min(PROMPT_HOOK_CLEANUP_RESERVE);
    let work_deadline = deadline.checked_sub(cleanup_reserve).unwrap_or(deadline);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => {
                terminate_prompt_hook(child, deadline);
                return Err(error.into());
            }
        }
        let remaining = work_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            terminate_prompt_hook(child, deadline);
            return Err(prompt_hook_timeout(budget));
        }
        std::thread::sleep(remaining.min(Duration::from_millis(10)));
    }
}

fn receive_prompt_hook_result<T>(
    receiver: &std::sync::mpsc::Receiver<T>,
    deadline: Instant,
) -> Option<T> {
    receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .ok()
}

fn prompt_hook_timeout(budget: Duration) -> AppError {
    AppError::new(
        "codegraph_prompt_hook_timeout",
        format!(
            "CodeGraph prompt hook exceeded its {} millisecond time budget",
            budget.as_millis()
        ),
    )
}

fn terminate_prompt_hook(child: &mut Child, deadline: Instant) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    let _ = terminate_prompt_hook_process_group(child, deadline);
    let _ = wait_for_child_until(child, deadline);
}

fn wait_for_child_until(
    child: &mut Child,
    deadline: Instant,
) -> io::Result<Option<std::process::ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        std::thread::sleep(remaining.min(Duration::from_millis(5)));
    }
}

#[cfg(unix)]
fn terminate_prompt_hook_process_group(child: &mut Child, _deadline: Instant) -> io::Result<()> {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    if unsafe { kill(-(child.id() as i32), 9) } == 0 {
        return Ok(());
    }
    match child.kill() {
        Ok(()) => Ok(()),
        Err(_error) if child.try_wait()?.is_some() => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn terminate_prompt_hook_process_group(child: &mut Child, deadline: Instant) -> io::Result<()> {
    let started = Instant::now();
    let remaining = deadline.saturating_duration_since(started);
    let taskkill_deadline = started + remaining / 3;
    let taskkill_reap_deadline = started + remaining.saturating_mul(2) / 3;
    let mut taskkill = Command::new("taskkill.exe")
        .args(["/PID", &child.id().to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(taskkill) = taskkill.as_mut() {
        match wait_for_child_until(taskkill, taskkill_deadline) {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                let _ = taskkill.kill();
                let _ = wait_for_child_until(taskkill, taskkill_reap_deadline);
            }
        }
    }
    match child.kill() {
        Ok(()) => Ok(()),
        Err(_error) if child.try_wait()?.is_some() => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(all(not(unix), not(windows)))]
fn terminate_prompt_hook_process_group(child: &mut Child, _deadline: Instant) -> io::Result<()> {
    match child.kill() {
        Ok(()) => Ok(()),
        Err(_error) if child.try_wait()?.is_some() => Ok(()),
        Err(error) => Err(error),
    }
}

fn execute(paths: &Paths, args: &[OsString]) -> Result<Value> {
    let status = configured_command(paths, args)?.status()?;
    if !status.success() {
        return Err(AppError::new("codegraph_exit", "upstream process exited")
            .with_details(json!({"exit_code": status.code().unwrap_or(1)})));
    }
    Ok(Value::Null)
}

fn configured_command(paths: &Paths, args: &[OsString]) -> Result<Command> {
    let executable = binary(paths)
        .ok_or_else(|| AppError::new("codegraph_runtime_missing", "run `lwc cg init` first"))?;
    let home = if paths.external.is_some() {
        crate::scope::global_lwc_root()?
            .parent()
            .unwrap()
            .to_path_buf()
    } else {
        paths.runtime.join("home")
    };
    fs::create_dir_all(&home)?;
    Ok(build_configured_command(paths, args, &executable, &home))
}

fn configured_prompt_hook_command(paths: &Paths, args: &[OsString]) -> Result<Command> {
    if paths.external.is_none() {
        require_prompt_hook_directory(&paths.runtime)?;
    }
    let home = if paths.external.is_some() {
        crate::scope::global_lwc_root()?
            .parent()
            .unwrap()
            .to_path_buf()
    } else {
        paths.runtime.join("home")
    };
    require_prompt_hook_directory(&home)?;
    let executable = binary(paths).ok_or_else(prompt_hook_unavailable)?;
    Ok(build_configured_command(paths, args, &executable, &home))
}

fn require_prompt_hook_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) | Err(_) => Err(prompt_hook_unavailable()),
    }
}

fn prompt_hook_unavailable() -> AppError {
    AppError::new(
        "codegraph_prompt_hook_unavailable",
        "CodeGraph prompt Hook runtime is unavailable",
    )
}

fn build_configured_command(
    paths: &Paths,
    args: &[OsString],
    executable: &Path,
    home: &Path,
) -> Command {
    let mut command = Command::new(executable);
    command
        .args(args)
        .current_dir(&paths.project)
        .env(
            "CODEGRAPH_DIR",
            if paths.external.is_some() {
                ".codegraph"
            } else {
                ".lwc/codegraph"
            },
        )
        .env("CODEGRAPH_TELEMETRY", "0")
        .env("DO_NOT_TRACK", "1")
        .env("NO_COLOR", "1")
        .env("HOME", home)
        .env("USERPROFILE", home);
    command
}

pub(crate) fn mcp_command(project: &Path) -> Result<Command> {
    let paths = Paths::from_project(fs::canonicalize(project)?)?;
    validate_index(&paths)?;
    let executable = binary(&paths).ok_or_else(|| {
        AppError::new(
            "codegraph_runtime_missing",
            "run `lwc cg init` to install the pinned CodeGraph runtime",
        )
    })?;
    let home = if paths.external.is_some() {
        crate::scope::global_lwc_root()?
            .parent()
            .unwrap()
            .to_path_buf()
    } else {
        paths.runtime.join("home")
    };
    let mut command = Command::new(executable);
    command
        .args(["serve", "--mcp"])
        .current_dir(&paths.project)
        .env(
            "CODEGRAPH_DIR",
            if paths.external.is_some() {
                ".codegraph"
            } else {
                ".lwc/codegraph"
            },
        )
        .env(
            "CODEGRAPH_MCP_TOOLS",
            "search,callers,callees,impact,node,explore,status,files",
        )
        .env("CODEGRAPH_TELEMETRY", "0")
        .env("DO_NOT_TRACK", "1")
        .env("NO_COLOR", "1")
        .env("HOME", &home)
        .env("USERPROFILE", &home);
    Ok(command)
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
    if let Some(executable) = &paths.external {
        return RuntimeState {
            health: if executable.is_file() {
                "ready"
            } else {
                "missing"
            },
            binary: executable.is_file().then(|| executable.clone()),
        };
    }
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

pub fn configure(store: &StorePath, executable: Option<&Path>) -> Result<Value> {
    let executable = executable
        .map(|path| {
            if !path.is_absolute() || !path.is_file() {
                return Err(AppError::new(
                    "invalid_codegraph_runtime",
                    "--executable must name an existing absolute executable path",
                ));
            }
            Ok(fs::canonicalize(path)?)
        })
        .transpose()?;
    crate::config::update(
        &store.path,
        crate::config::ConfigPatch {
            codegraph_executable: Some(executable),
            ..Default::default()
        },
    )?;
    status(store)
}

/// This is evidence for named files, not a completeness claim about call edges.
pub fn check_files(store: &StorePath, files: &[PathBuf], require_fresh: bool) -> Result<Value> {
    let paths = Paths::new(store)?;
    let project = fs::canonicalize(&paths.project)?;
    let database = paths.index.join("codegraph.db");
    if files.len() > 1000 {
        return Err(AppError::new(
            "invalid_input",
            "at most 1000 files can be checked per request",
        ));
    }
    let conn = validate_index(&paths).and_then(|_| {
        Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(Into::into)
    });
    let index_state = match &conn {
        Err(error) if error.code == "codegraph_index_missing" => "index_missing",
        Err(_) => "index_unreadable",
        Ok(conn) => match conn.prepare("SELECT path,content_hash FROM files LIMIT 0") {
            Ok(_) => "ready",
            Err(error)
                if matches!(
                    error.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)
                ) =>
            {
                "index_unreadable"
            }
            Err(_) => "unsupported_index",
        },
    };
    let mut checks = Vec::new();
    for file in files {
        let file = project_relative_path(&paths, file)?;
        let mut relative = PathBuf::new();
        for component in file.components() {
            match component {
                Component::Normal(name) => relative.push(name),
                Component::ParentDir => {
                    relative.pop();
                }
                Component::CurDir => {}
                _ => return Err(external_path_error()),
            }
        }
        let name = relative.to_string_lossy().replace('\\', "/");
        let indexed = if index_state == "ready" {
            use rusqlite::OptionalExtension;
            conn.as_ref()
                .unwrap()
                .query_row(
                    "SELECT content_hash FROM files WHERE path=?1",
                    [&name],
                    |row| row.get::<_, String>(0),
                )
                .optional()
        } else {
            Ok(None)
        };
        let current = content_hash(&project.join(&relative));
        let state = match (&indexed, &current) {
            _ if index_state != "ready" => index_state,
            (Err(_), _) => "index_unreadable",
            (Ok(None), _) => "not_indexed",
            (_, Err(error)) if error.kind() == io::ErrorKind::NotFound => "missing_file",
            (_, Err(_)) => "file_unreadable",
            (Ok(Some(indexed)), Ok(current)) if indexed == current => "fresh",
            _ => "stale",
        };
        checks.push(json!({"file":name,"state":state,"indexed_hash":indexed.ok().flatten(),"current_hash":current.ok()}));
    }
    let fresh = !checks.is_empty() && checks.iter().all(|check| check["state"] == "fresh");
    let result = json!({"checkout": project, "index": paths.index, "files": checks, "fresh": fresh,
        "coverage": "explicit_files_only", "relationship_completeness": "unknown", "indexed_commit":null, "hash_algorithm":"sha256",
        "checked_at": chrono::Utc::now().to_rfc3339()});
    if require_fresh && !fresh {
        return Err(AppError::new(
            "codegraph_freshness_unproven",
            "at least one requested file is stale, missing, or not proven indexed",
        )
        .with_details(result));
    }
    Ok(result)
}

fn validate_index(paths: &Paths) -> Result<()> {
    for directory in [
        paths.index.parent().unwrap().to_path_buf(),
        paths.index.clone(),
    ] {
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
    Ok(())
}

fn content_hash(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub(crate) fn query_identity(project: &Path) -> Result<(PathBuf, PathBuf)> {
    let paths = Paths::from_project(fs::canonicalize(project)?)?;
    validate_index(&paths)?;
    let executable = binary(&paths).ok_or_else(|| {
        AppError::new(
            "codegraph_runtime_missing",
            "the selected CodeGraph runtime is unavailable",
        )
    })?;
    Ok((executable, paths.index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn runtime_fixture(temp: &tempfile::TempDir) -> Paths {
        let target = target_name().unwrap();
        let extension = if cfg!(windows) { "zip" } else { "tar.gz" };
        let project = temp.path().join("project");
        let runtime = temp.path().join("global-runtime");
        let executable = runtime.join("bin").join("codegraph");
        fs::create_dir(&project).unwrap();
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, b"fixture").unwrap();
        let asset = format!("codegraph-{target}.{extension}");
        fs::write(
            runtime.join(RUNTIME_MANIFEST),
            serde_json::to_vec(&RuntimeManifest {
                version: VERSION.to_owned(),
                target: target.to_owned(),
                asset: asset.clone(),
                archive_sha256: "0".repeat(64),
                binary: PathBuf::from("bin/codegraph"),
            })
            .unwrap(),
        )
        .unwrap();
        Paths {
            external: None,
            legacy_runtime: project.join(".lwc/runtime/codegraph"),
            index: project.join(".lwc/codegraph"),
            project,
            runtime,
            target,
            asset,
        }
    }

    fn snapshot_tree(root: &Path) -> BTreeSet<PathBuf> {
        fn visit(root: &Path, path: &Path, entries: &mut BTreeSet<PathBuf>) {
            let Ok(metadata) = fs::symlink_metadata(path) else {
                return;
            };
            entries.insert(path.strip_prefix(root).unwrap().to_path_buf());
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                for entry in fs::read_dir(path).unwrap() {
                    visit(root, &entry.unwrap().path(), entries);
                }
            }
        }

        let mut entries = BTreeSet::new();
        visit(root, root, &mut entries);
        entries
    }

    #[test]
    fn prompt_hook_exposes_a_bounded_budget_companion() {
        let _api: fn(&Path, &str, Duration) -> Result<String> = prompt_hook_with_budget;
    }

    #[test]
    fn prompt_hook_command_requires_existing_real_runtime_home_without_writes() {
        let temp = tempfile::tempdir().unwrap();
        let paths = runtime_fixture(&temp);
        let before = snapshot_tree(temp.path());

        let error =
            configured_prompt_hook_command(&paths, &[OsString::from("prompt-hook")]).unwrap_err();

        assert_eq!(error.code, "codegraph_prompt_hook_unavailable");
        assert!(!error.to_string().contains(temp.path().to_str().unwrap()));
        assert_eq!(snapshot_tree(temp.path()), before);
        assert!(!paths.runtime.join("home").exists());

        fs::write(paths.runtime.join("home"), b"not a directory").unwrap();
        let invalid_before = snapshot_tree(temp.path());
        let error =
            configured_prompt_hook_command(&paths, &[OsString::from("prompt-hook")]).unwrap_err();
        assert_eq!(error.code, "codegraph_prompt_hook_unavailable");
        assert!(!error.to_string().contains(temp.path().to_str().unwrap()));
        assert_eq!(snapshot_tree(temp.path()), invalid_before);

        fs::remove_file(paths.runtime.join("home")).unwrap();
        configured_command(&paths, &[OsString::from("status")]).unwrap();
        let metadata = fs::symlink_metadata(paths.runtime.join("home")).unwrap();
        assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
    }

    #[test]
    fn prompt_hook_command_does_not_create_a_missing_runtime_tree() {
        let temp = tempfile::tempdir().unwrap();
        let mut paths = runtime_fixture(&temp);
        paths.runtime = temp.path().join("missing/runtime/codegraph");
        let before = snapshot_tree(temp.path());

        let error =
            configured_prompt_hook_command(&paths, &[OsString::from("prompt-hook")]).unwrap_err();

        assert_eq!(error.code, "codegraph_prompt_hook_unavailable");
        assert!(!error.to_string().contains(temp.path().to_str().unwrap()));
        assert_eq!(snapshot_tree(temp.path()), before);
        assert!(!paths.runtime.exists());
    }

    #[test]
    fn prompt_hook_cleanup_wait_is_deadline_bounded_and_reaps() {
        let temp = tempfile::tempdir().unwrap();
        let ready = temp.path().join("ready");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "codegraph::tests::prompt_hook_cleanup_child_fixture",
            ])
            .env("LWC_PROMPT_HOOK_CLEANUP_FIXTURE", &ready)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().unwrap();
        let ready_by = Instant::now() + Duration::from_secs(5);
        while !ready.is_file() && Instant::now() < ready_by {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ready.is_file(), "cleanup child never became ready");

        let started = Instant::now();
        assert!(wait_for_child_until(&mut child, started).unwrap().is_none());
        assert!(started.elapsed() < Duration::from_millis(100));

        terminate_prompt_hook(&mut child, Instant::now() + Duration::from_millis(500));
        assert!(
            wait_for_child_until(&mut child, Instant::now() + Duration::from_millis(100))
                .unwrap()
                .is_some(),
            "cleanup did not reap the child"
        );
    }

    #[test]
    fn prompt_hook_cleanup_child_fixture() {
        let Some(ready) = std::env::var_os("LWC_PROMPT_HOOK_CLEANUP_FIXTURE") else {
            return;
        };
        fs::write(ready, b"ready").unwrap();
        std::thread::sleep(Duration::from_secs(30));
    }

    #[cfg(unix)]
    #[test]
    fn prompt_hook_budget_times_out_fail_open_and_reaps_the_process_group() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let executable = temp.path().join("fake-codegraph");
        let pid_file = temp.path().join("pids");
        fs::create_dir(&project).unwrap();
        fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s ' \"$$\" > \"$PID_FILE\"\nsleep 30 &\ndescendant=$!\nprintf '%s\\n' \"$descendant\" >> \"$PID_FILE\"\nwait\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

        let mut command = Command::new(&executable);
        command.current_dir(&project).env("PID_FILE", &pid_file);
        use std::os::unix::process::CommandExt;
        command.process_group(0);
        let mut child = command.spawn().unwrap();
        let ready_by = Instant::now() + Duration::from_secs(5);
        while !pid_file.is_file() && Instant::now() < ready_by {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            pid_file.is_file(),
            "fake CodeGraph child never became ready"
        );

        let budget = Duration::from_millis(1_500);
        let started = Instant::now();
        let result = wait_for_prompt_hook_exit(&mut child, started + budget, budget);
        let elapsed = started.elapsed();

        let error = result.unwrap_err();
        assert_eq!(error.code, "codegraph_prompt_hook_timeout");
        assert!(
            elapsed < Duration::from_millis(1_900),
            "1500ms budget took {elapsed:?}"
        );
        let pids = fs::read_to_string(&pid_file)
            .unwrap()
            .split_whitespace()
            .map(|pid| pid.parse::<i32>().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(pids.len(), 2);
        let reaped_by = Instant::now() + Duration::from_millis(500);
        while pids.iter().any(|pid| process_exists(*pid)) && Instant::now() < reaped_by {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            pids.iter().all(|pid| !process_exists(*pid)),
            "timed-out prompt hook leaked child processes: {pids:?}"
        );
    }

    #[cfg(unix)]
    fn process_exists(pid: i32) -> bool {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        unsafe { kill(pid, 0) == 0 }
    }
}
