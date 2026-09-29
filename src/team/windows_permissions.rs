use std::{io, os::windows::ffi::OsStrExt, path::Path, ptr};
use windows_sys::Win32::{
    Security::{
        ACL, ACL_REVISION, AddAccessAllowedAceEx,
        Authorization::{SE_FILE_OBJECT, SetNamedSecurityInfoW},
        CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, GetTokenInformation, InitializeAcl,
        OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::FILE_ALL_ACCESS,
};

/// Replace inherited/explicit grants with current-user-only inheritable access.
/// Runs on every access: do not cache away checks after external ACL changes.
pub(super) fn restrict(path: &Path) -> io::Result<()> {
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if path[..path.len() - 1].contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"));
    }
    // Aligned storage exceeds TOKEN_USER + SECURITY_MAX_SID_SIZE (68 bytes),
    // and an ACL containing one ACCESS_ALLOWED_ACE with that SID.
    let mut user = [0usize; 32];
    let mut acl = [0usize; 32];
    let mut needed = 0;
    // SAFETY: buffers are aligned and sized for all Windows SIDs; token data
    // stays alive until SetNamedSecurityInfoW returns. -4 is the SDK's
    // GetCurrentProcessToken() pseudo-handle, which must not be closed.
    unsafe {
        if GetTokenInformation(
            -4isize as _,
            TokenUser,
            user.as_mut_ptr().cast(),
            size_of_val(&user) as u32,
            &mut needed,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let sid = (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid;
        let dacl = acl.as_mut_ptr().cast::<ACL>();
        if InitializeAcl(dacl, size_of_val(&acl) as u32, ACL_REVISION) == 0
            || AddAccessAllowedAceEx(
                dacl,
                ACL_REVISION,
                CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE,
                FILE_ALL_ACCESS,
                sid,
            ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let error = SetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            sid,
            ptr::null_mut(),
            dacl,
            ptr::null(),
        );
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
    }
    Ok(())
}
