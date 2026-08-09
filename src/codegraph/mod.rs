use crate::{
    error::{AppError, Result},
    scope::StorePath,
};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    ffi::{OsStr, OsString},
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const VERSION: &str = "v1.5.0-lwc.1";
const RELEASE_ROOT: &str = "https://github.com/JanYork/codegraph/releases/download";

pub fn status(store: &StorePath) -> Value {
    let paths = Paths::new(store);
    json!({
        "scope": "project",
        "version": VERSION,
        "installed": binary(&paths).is_some(),
        "initialized": paths.index.join("codegraph.db").is_file(),
        "runtime": paths.runtime,
        "index": paths.index,
        "telemetry": false,
    })
}

pub fn init(store: &StorePath, verbose: bool) -> Result<Value> {
    let paths = Paths::new(store);
    install(&paths)?;
    let mut args = vec![
        OsString::from("init"),
        paths.project.as_os_str().to_owned(),
        OsString::from("--force"),
    ];
    if verbose {
        args.push(OsString::from("--verbose"));
    }
    execute(&paths, &args, true)
}

pub fn run(store: &StorePath, args: &[OsString]) -> Result<Value> {
    let paths = Paths::new(store);
    let Some(name) = args.first().and_then(|value| value.to_str()) else {
        return Err(AppError::new(
            "invalid_codegraph_command",
            "missing CodeGraph command",
        ));
    };
    if matches!(
        name,
        "install" | "uninstall" | "upgrade" | "telemetry" | "daemon" | "serve"
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
            "run `lwc cg init` to download the pinned project-local CodeGraph runtime",
        ));
    }

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
        forwarded.insert(1, paths.project.as_os_str().to_owned());
    }
    execute(
        &paths,
        &forwarded,
        matches!(name, "index" | "sync" | "uninit" | "unlock"),
    )
}

pub fn graph(store: &StorePath) -> Value {
    let paths = Paths::new(store);
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
    index: PathBuf,
}

impl Paths {
    fn new(store: &StorePath) -> Self {
        let lwc = store
            .path
            .parent()
            .expect("Wiki database has an .lwc parent");
        Self {
            project: lwc
                .parent()
                .expect("project .lwc has a parent")
                .to_path_buf(),
            runtime: lwc.join("runtime/codegraph"),
            index: lwc.join("codegraph"),
        }
    }
}

fn execute(paths: &Paths, args: &[OsString], stream: bool) -> Result<Value> {
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
    let target = target_name()?;
    let extension = if cfg!(windows) { "zip" } else { "tar.gz" };
    let asset = format!("codegraph-{target}.{extension}");
    let parent = paths.runtime.parent().expect("runtime has a parent");
    fs::create_dir_all(parent)?;
    let staging = parent.join(format!("codegraph.download-{}", std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir(&staging)?;
    let result = (|| {
        let archive = staging.join(&asset);
        let sums = staging.join("SHA256SUMS");
        download(&format!("{RELEASE_ROOT}/{VERSION}/{asset}"), &archive)?;
        download(&format!("{RELEASE_ROOT}/{VERSION}/SHA256SUMS"), &sums)?;
        verify(&archive, &sums, &asset)?;
        unpack(&archive, &staging)?;
        fs::remove_file(&archive)?;
        fs::remove_file(&sums)?;
        if paths.runtime.exists() {
            fs::remove_dir_all(&paths.runtime)?;
        }
        fs::rename(&staging, &paths.runtime)?;
        Ok(())
    })();
    if result.is_err() && staging.exists() {
        let _ = fs::remove_dir_all(staging);
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

fn verify(archive: &Path, sums: &Path, asset: &str) -> Result<()> {
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
        Ok(())
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
    find_binary(&paths.runtime, 0)
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
