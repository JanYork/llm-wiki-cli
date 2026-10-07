use super::spaces::*;
use super::*;
use std::process::{Command, Stdio};

fn paced_delay_ms(interval: u64, elapsed: Duration, failures: u32, entropy: u64) -> u64 {
    if failures == 0 {
        // Leave breathing room after expensive batches. Local durable state is the
        // coalescing queue; edits during this pause belong to the next fixed window.
        let work = elapsed.as_millis().min(60_000) as u64;
        let delay = interval.max(work);
        delay.saturating_add(entropy % ((delay / 10).min(5_000) + 1))
    } else {
        let delay = interval
            .saturating_mul(1u64 << failures.min(7))
            .min(60_000)
            .max(interval);
        delay
            .saturating_sub(delay / 4)
            .saturating_add(entropy % (delay / 4 + 1))
            .max(interval)
    }
}

fn entropy() -> Result<u64> {
    let mut random = [0u8; 8];
    getrandom::fill(&mut random)
        .map_err(|_| AppError::new("random_failed", "sync pacing jitter unavailable"))?;
    Ok(u64::from_le_bytes(random))
}

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
    let mut last_status = None;
    let mut last_report = Instant::now();
    // Aggregate startup edits, but resume an uncertain in-flight batch immediately.
    // This window never restarts on each edit, so continuous editing cannot starve it.
    if !directory.join("active.json").exists() {
        std::thread::sleep(Duration::from_millis(paced_delay_ms(
            record.interval_ms,
            Duration::ZERO,
            0,
            entropy()?,
        )));
    }
    loop {
        let record = read_record(&directory.join("replica.json"))?;
        if !record.automatic {
            return Ok(json!({"status":"stopped"}));
        }
        let started = Instant::now();
        let result = sync_space(&reference);
        let status = match result {
            Ok(value) => {
                // A completed local rebase is progress, not a failing transport.
                failures = if value["status"] == "retry" && value["reason"] != "local_changed" {
                    failures.saturating_add(1)
                } else {
                    0
                };
                value
            }
            Err(error) => {
                failures = failures.saturating_add(1);
                json!({"status":"retry","error":error.code,"local_memory_preserved":true})
            }
        };
        let delay = paced_delay_ms(record.interval_ms, started.elapsed(), failures, entropy()?);
        // Reports are telemetry, not queue durability. Avoid two fsyncs every idle
        // poll; record status changes immediately and otherwise heartbeat once a minute.
        if last_status.as_ref() != Some(&status) || last_report.elapsed() >= Duration::from_secs(60)
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| AppError::new("clock_error", "clock predates Unix epoch"))?
                .as_millis();
            save_credentials(
                &directory.join("worker.json"),
                &json!({"pid":std::process::id(),"checked_at_ms":now,"next_delay_ms":delay,"result":status}),
            )?;
            last_status = Some(status);
            last_report = Instant::now();
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pacing_coalesces_slow_batches_and_bounds_retries() {
        assert_eq!(paced_delay_ms(2_000, Duration::ZERO, 0, 0), 2_000);
        assert_eq!(paced_delay_ms(2_000, Duration::from_secs(34), 0, 0), 34_000);
        assert_eq!(
            paced_delay_ms(2_000, Duration::from_secs(600), 0, 0),
            60_000
        );
        assert_eq!(paced_delay_ms(2_000, Duration::ZERO, 1, 0), 3_000);
        assert_eq!(paced_delay_ms(2_000, Duration::ZERO, u32::MAX, 0), 45_000);
        for failures in [0, 1, 7, u32::MAX] {
            let delay = paced_delay_ms(300_000, Duration::from_secs(600), failures, u64::MAX);
            assert!((300_000..=305_000).contains(&delay));
        }
        for entropy in [0, 1, u64::MAX] {
            assert!((34_000..=37_400).contains(&paced_delay_ms(
                2_000,
                Duration::from_secs(34),
                0,
                entropy,
            )));
        }
    }
}
