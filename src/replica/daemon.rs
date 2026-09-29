use super::spaces::*;
use super::*;
use std::process::{Command, Stdio};

fn worker_lock(directory: &Path) -> Result<fs::File> {
    let path = directory.join("worker.lock");
    if path.exists() && fs::symlink_metadata(&path)?.file_type().is_symlink() {
        return Err(AppError::new(
            "unsafe_replica_path",
            "worker lock cannot be a symlink",
        ));
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.try_lock()
        .map_err(|_| AppError::new("worker_running", "a sync worker already owns this space"))?;
    Ok(file)
}
/// Native user services restart this supervisor after login or executable upgrades.
/// Worker locks keep command-triggered and supervised startup idempotent.
pub(crate) fn supervise() -> Result<Value> {
    loop {
        for (directory, record) in records()? {
            if record.joined
                && record.automatic
                && let Err(error) = start_worker(&reference(&directory, &record))
            {
                eprintln!("sync supervisor: {}", error.code);
            }
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}
pub(crate) fn start_worker(space: &str) -> Result<()> {
    let (directory, record) = resolve(space)?;
    if !record.automatic || !record.joined {
        return Ok(());
    }
    match worker_lock(&directory) {
        Ok(lock) => drop(lock),
        Err(error) if error.code == "worker_running" => return Ok(()),
        Err(error) => return Err(error),
    }
    #[cfg(windows)]
    crate::work::disable_standard_handle_inheritance()?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["space", "watch", &reference(&directory, &record)])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0200);
    }
    command.spawn().map_err(|_| {
        AppError::new(
            "sync_worker_start_failed",
            "could not start the local sync worker",
        )
    })?;
    Ok(())
}
pub(crate) fn watch_space(space: &str) -> Result<Value> {
    let (directory, record) = resolve(space)?;
    let _worker = match worker_lock(&directory) {
        Ok(lock) => lock,
        Err(error) if error.code == "worker_running" => return Ok(Value::Null),
        Err(error) => return Err(error),
    };
    let reference = reference(&directory, &record);
    let mut failures = 0u32;
    loop {
        let record = read_record(&directory.join("replica.json"))?;
        if !record.automatic {
            return Ok(json!({"status":"stopped"}));
        }
        let result = sync_space(&reference);
        let (status, delay) = match result {
            Ok(value) => {
                failures = 0;
                (value, record.interval_ms)
            }
            Err(error) => {
                failures = failures.saturating_add(1);
                let delay = record
                    .interval_ms
                    .saturating_mul(1u64 << failures.min(7))
                    .min(60_000);
                (
                    json!({"status":"retry","error":error.code,"local_memory_preserved":true}),
                    delay,
                )
            }
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| AppError::new("clock_error", "clock predates Unix epoch"))?
            .as_millis();
        save_credentials(
            &directory.join("worker.json"),
            &json!({"pid":std::process::id(),"checked_at_ms":now,"next_delay_ms":delay,"result":status}),
        )?;
        // Polling is portable and also catches writes by other processes. Store identity
        // avoids exports while idle; filesystem notifications can be added if measured necessary.
        std::thread::sleep(Duration::from_millis(delay));
    }
}
pub(crate) fn configure_space(
    space: &str,
    interval_ms: Option<u64>,
    automatic: Option<bool>,
) -> Result<Value> {
    if interval_ms.is_some_and(|ms| !(250..=300_000).contains(&ms)) {
        return Err(AppError::new(
            "invalid_interval",
            "sync interval must be 250..300000 ms",
        ));
    }
    let (directory, _) = resolve(space)?;
    let record = {
        let _lock = sync_lock(&directory)?;
        let mut record = read_record(&directory.join("replica.json"))?;
        if let Some(interval) = interval_ms {
            record.interval_ms = interval;
        }
        if let Some(automatic) = automatic {
            record.automatic = automatic;
        }
        save_credentials(&directory.join("replica.json"), &record)?;
        record
    };
    if record.automatic {
        start_worker(space)?;
    }
    show_space(space)
}
