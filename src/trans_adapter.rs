use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{
    fs,
    io::{self, Read},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

const STDERR_PREVIEW_BYTES: u64 = 8192;
#[cfg(unix)]
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

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    pub(crate) engine: String,
    #[serde(default)]
    pub(crate) args: Vec<String>,
    pub(crate) timeout_seconds: u16,
}

pub(crate) struct Error {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) details: Value,
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self {
            code: "trans_failed",
            message: error.to_string(),
            details: Value::Null,
        }
    }
}

pub(crate) fn run(config: &Config, input: &Path, output: &Path) -> Result<(), Error> {
    if !(1..=900).contains(&config.timeout_seconds) {
        return Err(error(
            "invalid_trans_timeout",
            "converter timeout must be within 1..=900 seconds",
        ));
    }
    let args = validate_adapter_args(&config.engine, &config.args)?;
    let stderr = output.with_extension("stderr");
    let stderr_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stderr)?;
    let mut command = engine_command(&config.engine);
    command.stdin(Stdio::null()).stdout(Stdio::null());
    command.stderr(Stdio::from(stderr_file));
    #[cfg(unix)]
    command.process_group(0);
    command.args(args).arg(input).arg("-o").arg(output);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(start_error) => {
            let _ = fs::remove_file(&stderr);
            return Err(match start_error.kind() {
                io::ErrorKind::NotFound => error(
                    "trans_executable_missing",
                    format!("{} is not installed or not on PATH", config.engine),
                ),
                _ => error(
                    "trans_failed",
                    format!("failed to start {}: {start_error}", config.engine),
                ),
            });
        }
    };
    let result = wait_with_timeout(&config.engine, &mut child, config.timeout_seconds, &stderr);
    let _ = fs::remove_file(stderr);
    result
}

fn validate_adapter_args(engine: &str, args: &[String]) -> Result<Vec<String>, Error> {
    let specs = match engine {
        "anydoc" => ANYDOC_OPTIONS,
        "markitdown" => MARKITDOWN_OPTIONS,
        _ => {
            return Err(error(
                "trans_disabled",
                "no supported converter is configured",
            ));
        }
    };
    let mut validated = Vec::with_capacity(args.len());
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" || !arg.starts_with('-') || matches!(arg.as_str(), "-o" | "--output") {
            return Err(error(
                "trans_unsafe_args",
                "converter arguments cannot add inputs or outputs",
            ));
        }
        if let Some((name, value)) = arg.strip_prefix("--").and_then(|_| arg.split_once('=')) {
            let spec = specs.iter().find(|spec| spec.names.contains(&name));
            if !matches!(spec, Some(spec) if spec.takes_value) || value.is_empty() {
                return Err(error(
                    "trans_unsafe_args",
                    format!("unsupported converter argument: {arg}"),
                ));
            }
            validated.push(arg.clone());
            index += 1;
            continue;
        }
        let Some(spec) = specs.iter().find(|spec| spec.names.contains(&arg.as_str())) else {
            return Err(error(
                "trans_unsafe_args",
                format!("unsupported converter argument: {arg}"),
            ));
        };
        validated.push(arg.clone());
        if spec.takes_value {
            let Some(value) = args.get(index + 1) else {
                return Err(error(
                    "trans_unsafe_args",
                    format!("{arg} requires a value"),
                ));
            };
            if value == "--" {
                return Err(error(
                    "trans_unsafe_args",
                    format!("{arg} has an unsafe value"),
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

#[cfg(windows)]
fn engine_command(engine: &str) -> Command {
    let executable = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .flat_map(|directory| {
                ["exe", "cmd", "bat"]
                    .map(move |extension| directory.join(format!("{engine}.{extension}")))
            })
            .find(|candidate| candidate.is_file())
    });
    Command::new(executable.unwrap_or_else(|| std::path::PathBuf::from(engine)))
}

#[cfg(not(windows))]
fn engine_command(engine: &str) -> Command {
    Command::new(engine)
}

fn wait_with_timeout(
    engine: &str,
    child: &mut Child,
    timeout_seconds: u16,
    stderr_path: &Path,
) -> Result<(), Error> {
    let deadline = Instant::now() + Duration::from_secs(u64::from(timeout_seconds));
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            terminate_process_tree(child)?;
            child.wait()?;
            let stderr = read_bounded(stderr_path)?;
            return Err(Error {
                code: "trans_timeout",
                message: format!(
                    "{engine} exceeded the configured timeout of {timeout_seconds} seconds"
                ),
                details: stderr_details(&stderr),
            });
        }
        thread::sleep(Duration::from_millis(25));
    };
    let stderr = read_bounded(stderr_path)?;
    if status.success() {
        Ok(())
    } else {
        Err(Error {
            code: "trans_failed",
            message: format!("{engine} exited with {}", render_status(status)),
            details: stderr_details(&stderr),
        })
    }
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
    let status = Command::new("taskkill.exe")
        .args(["/PID", &child.id().to_string(), "/T", "/F"])
        .status();
    if matches!(status, Ok(status) if status.success()) {
        Ok(())
    } else {
        child.kill()
    }
}

#[cfg(all(not(unix), not(windows)))]
fn terminate_process_tree(child: &mut Child) -> io::Result<()> {
    child.kill()
}

#[cfg(unix)]
fn kill_process_group(pid: u32) -> io::Result<()> {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    let result = unsafe { kill(-(pid as i32), 9) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

fn read_bounded(path: &Path) -> Result<BoundedBytes, Error> {
    let file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(STDERR_PREVIEW_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() as u64 > STDERR_PREVIEW_BYTES;
    bytes.truncate(STDERR_PREVIEW_BYTES as usize);
    Ok(BoundedBytes { bytes, truncated })
}

fn render_status(status: ExitStatus) -> String {
    status
        .code()
        .map(|code| format!("exit status {code}"))
        .unwrap_or_else(|| "a terminating signal".to_owned())
}

fn stderr_details(stderr: &BoundedBytes) -> Value {
    json!({"stderr_present":!stderr.bytes.is_empty(),"stderr_truncated":stderr.truncated})
}

fn error(code: &'static str, message: impl Into<String>) -> Error {
    Error {
        code,
        message: message.into(),
        details: Value::Null,
    }
}

struct BoundedBytes {
    bytes: Vec<u8>,
    truncated: bool,
}
