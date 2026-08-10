fn load_state(store: &StorePath, id: &str) -> Result<WorkState> {
    validate_id(id)?;
    let root = work_root(&store.path)?;
    ensure_root(&root)?;
    let directory = root.join(id);
    ensure_work_directory(&directory)?;
    read_json(&directory.join("state.json"))
}

fn active_state(root: &Path) -> Result<Option<WorkState>> {
    let active = root.join("active");
    let id = match fs::read_to_string(&active) {
        Ok(value) => value.trim().to_owned(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    validate_id(&id)?;
    let directory = root.join(&id);
    ensure_work_directory(&directory)?;
    let state: WorkState = read_json(&directory.join("state.json"))?;
    if terminal(&state.state) {
        release_active(root, &id)?;
        return Ok(None);
    }
    Ok(Some(state))
}

fn claim_active(root: &Path, id: &str) -> Result<()> {
    let path = root.join("active");
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path).map_err(|error| {
        AppError::new(
            "work_busy",
            format!("another state-changing work is active: {error}"),
        )
    })?;
    file.write_all(id.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn spawn(root: &Path, id: &str) -> Result<()> {
    #[cfg(windows)]
    disable_standard_handle_inheritance()?;
    let executable = env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .arg("__work-run")
        .arg("--root")
        .arg(root)
        .arg("--id")
        .arg(id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(0x0800_0200);
    command.spawn().map(|_| ()).map_err(|error| {
        AppError::new(
            "work_spawn_failed",
            format!("failed to start work: {error}"),
        )
    })
}

#[cfg(windows)]
fn disable_standard_handle_inheritance() -> Result<()> {
    use std::ffi::c_void;

    const STD_INPUT_HANDLE: u32 = -10_i32 as u32;
    const STD_OUTPUT_HANDLE: u32 = -11_i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12_i32 as u32;
    const HANDLE_FLAG_INHERIT: u32 = 1;
    const INVALID_HANDLE_VALUE: *mut c_void = -1_isize as *mut c_void;

    unsafe extern "system" {
        #[link_name = "GetStdHandle"]
        fn get_std_handle(n_std_handle: u32) -> *mut c_void;
        #[link_name = "SetHandleInformation"]
        fn set_handle_information(handle: *mut c_void, mask: u32, flags: u32) -> i32;
    }

    for standard_handle in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        let handle = unsafe { get_std_handle(standard_handle) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            continue;
        }
        if unsafe { set_handle_information(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(AppError::new(
                "work_spawn_failed",
                format!(
                    "failed to detach Work standard handles: {}",
                    io::Error::last_os_error()
                ),
            ));
        }
    }
    Ok(())
}

fn release_active(root: &Path, id: &str) -> Result<()> {
    let path = root.join("active");
    match fs::read_to_string(&path) {
        Ok(value) if value.trim() == id => remove_file_if_present(&path)?,
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn remove_file_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn work_root(database: &Path) -> Result<PathBuf> {
    let parent = database
        .parent()
        .ok_or_else(|| AppError::new("work_invalid", "wiki database has no parent directory"))?;
    Ok(parent.join("work"))
}

fn ensure_root(root: &Path) -> Result<()> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(AppError::new(
                "work_invalid",
                format!("work root is not a real directory: {}", root.display()),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(root)?;
            set_directory_mode(root)?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn ensure_work_directory(directory: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(directory).map_err(|error| {
        AppError::new(
            "work_not_found",
            format!(
                "cannot inspect work directory {}: {error}",
                directory.display()
            ),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(AppError::new(
            "work_invalid",
            format!("work path is not a real directory: {}", directory.display()),
        ));
    }
    Ok(())
}

fn set_directory_mode(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_bytes(
        path,
        &serde_json::to_vec_pretty(value).map_err(|error| {
            AppError::new("work_invalid", format!("cannot encode work state: {error}"))
        })?,
    )
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        WORK_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    if let Err(error) = replace_with_retry(|| fs::rename(&temporary, path)) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    if let Some(parent) = path.parent()
        && let Ok(directory) = fs::File::open(parent)
    {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn replace_with_retry(mut replace: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    for attempt in 0..=STATE_REPLACE_RETRIES {
        match replace() {
            Ok(()) => return Ok(()),
            Err(error)
                if attempt < STATE_REPLACE_RETRIES
                    && matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted
                            | io::ErrorKind::PermissionDenied
                            | io::ErrorKind::WouldBlock
                    ) =>
            {
                thread::sleep(STATE_REPLACE_RETRY_DELAY);
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("state replacement loop always returns")
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(AppError::new(
            "work_invalid",
            format!("work state is a symlink: {}", path.display()),
        ));
    }
    let bytes = fs::read(path).map_err(|error| {
        AppError::new(
            "work_not_found",
            format!("cannot read work state {}: {error}", path.display()),
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        AppError::new(
            "work_invalid",
            format!("invalid work state {}: {error}", path.display()),
        )
    })
}

fn validate_id(id: &str) -> Result<()> {
    if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::new(
            "work_invalid",
            "work ID must be 64 hex characters",
        ));
    }
    Ok(())
}

fn work_id(database: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(database.as_os_str().as_encoded_bytes());
    hasher.update(std::process::id().to_be_bytes());
    hasher.update(now_ms().to_be_bytes());
    hasher.update(WORK_COUNTER.fetch_add(1, Ordering::Relaxed).to_be_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn terminal(state: &str) -> bool {
    matches!(state, "succeeded" | "failed" | "cancelled")
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::Project => "project",
        Scope::Global => "global",
        Scope::All => "all",
    }
}
