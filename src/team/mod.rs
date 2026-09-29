mod auth;
mod control;
mod delegation;
mod hub;
mod identity;
pub(crate) mod lease;
mod policy;
mod query;
mod server;
#[cfg(windows)]
mod windows_permissions;
pub(crate) use identity::{DeviceProfile, IdentityRegistration};
pub(crate) use query::CloudQuery;

pub(crate) use control::initialize;
pub(crate) use server::run;

pub(crate) fn private_directory(path: &std::path::Path) -> crate::error::Result<()> {
    use crate::error::AppError;
    use std::fs;
    let existed = path.try_exists()?;
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(AppError::new(
            "unsafe_private_directory",
            "private data directory cannot be a symlink",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if !existed {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
        if metadata.permissions().mode() & 0o077 != 0 && existed {
            return Err(AppError::new(
                "unsafe_private_directory",
                "private data directory must have mode 0700",
            ));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(AppError::new(
                "unsafe_private_directory",
                "private data directory cannot be a reparse point",
            ));
        }
        windows_permissions::restrict(path).map_err(|_| {
            AppError::new(
                "private_directory_acl_failed",
                "could not restrict the data directory to the current user",
            )
        })?;
        let _ = existed;
    }
    Ok(())
}

mod access;
pub(crate) use access::rotate as rotate_access_token;
mod admin;
mod recovery;
pub(crate) use recovery::RecoveryQuery;

mod backup;
pub(crate) use backup::snapshot as snapshot_server;

mod keys;
pub(crate) use keys::valid_format as valid_personal_key;
