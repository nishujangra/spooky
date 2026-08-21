#[cfg(unix)]
#[allow(dead_code)]
pub fn drop_privileges(user: &str, group: &str) -> Result<(), String> {
    use std::{ffi::CString, io};

    const DEFAULT_LOOKUP_BUF_LEN: usize = 16 * 1024;
    const MAX_LOOKUP_BUF_LEN: usize = 1024 * 1024;

    fn initial_lookup_buf_len(selector: libc::c_int) -> usize {
        let size = unsafe {
            // SAFETY: sysconf is thread-safe and does not require additional invariants.
            libc::sysconf(selector)
        };
        if size > 0 {
            size as usize
        } else {
            DEFAULT_LOOKUP_BUF_LEN
        }
    }

    fn lookup_group_gid(c_group: &CString, group: &str) -> Result<libc::gid_t, String> {
        let mut buf_len = initial_lookup_buf_len(libc::_SC_GETGR_R_SIZE_MAX);
        loop {
            let mut entry = std::mem::MaybeUninit::<libc::group>::uninit();
            let mut result: *mut libc::group = std::ptr::null_mut();
            let mut buf = vec![0 as libc::c_char; buf_len];
            let rc = unsafe {
                // SAFETY: pointers are valid for the provided buffer and output slots.
                libc::getgrnam_r(
                    c_group.as_ptr(),
                    entry.as_mut_ptr(),
                    buf.as_mut_ptr(),
                    buf.len(),
                    &mut result,
                )
            };
            if rc == 0 {
                if result.is_null() {
                    return Err(format!("group '{}' not found", group));
                }
                let entry = unsafe {
                    // SAFETY: successful lookup initializes `entry`.
                    entry.assume_init()
                };
                return Ok(entry.gr_gid);
            }
            if rc == libc::ERANGE && buf_len < MAX_LOOKUP_BUF_LEN {
                buf_len = (buf_len * 2).min(MAX_LOOKUP_BUF_LEN);
                continue;
            }
            return Err(format!(
                "failed to resolve group '{}': {}",
                group,
                io::Error::from_raw_os_error(rc)
            ));
        }
    }

    fn lookup_user_uid(c_user: &CString, user: &str) -> Result<libc::uid_t, String> {
        let mut buf_len = initial_lookup_buf_len(libc::_SC_GETPW_R_SIZE_MAX);
        loop {
            let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
            let mut result: *mut libc::passwd = std::ptr::null_mut();
            let mut buf = vec![0 as libc::c_char; buf_len];
            let rc = unsafe {
                // SAFETY: pointers are valid for the provided buffer and output slots.
                libc::getpwnam_r(
                    c_user.as_ptr(),
                    entry.as_mut_ptr(),
                    buf.as_mut_ptr(),
                    buf.len(),
                    &mut result,
                )
            };
            if rc == 0 {
                if result.is_null() {
                    return Err(format!("user '{}' not found", user));
                }
                let entry = unsafe {
                    // SAFETY: successful lookup initializes `entry`.
                    entry.assume_init()
                };
                return Ok(entry.pw_uid);
            }
            if rc == libc::ERANGE && buf_len < MAX_LOOKUP_BUF_LEN {
                buf_len = (buf_len * 2).min(MAX_LOOKUP_BUF_LEN);
                continue;
            }
            return Err(format!(
                "failed to resolve user '{}': {}",
                user,
                io::Error::from_raw_os_error(rc)
            ));
        }
    }

    let c_group = CString::new(group).map_err(|_| "group contains NUL byte".to_string())?;
    let c_user = CString::new(user).map_err(|_| "user contains NUL byte".to_string())?;

    let gid = lookup_group_gid(&c_group, group)?;
    let uid = lookup_user_uid(&c_user, user)?;

    let clear_groups_rc = unsafe {
        // SAFETY: passing null pointer with length 0 clears supplementary groups.
        libc::setgroups(0, std::ptr::null())
    };
    if clear_groups_rc != 0 {
        return Err("failed to clear supplementary groups".to_string());
    }

    let setgid_rc = unsafe {
        // SAFETY: gid resolved from getgrnam_r above.
        libc::setgid(gid)
    };
    if setgid_rc != 0 {
        return Err(format!("failed to drop group privileges to '{}'", group));
    }

    let setuid_rc = unsafe {
        // SAFETY: uid resolved from getpwnam_r above.
        libc::setuid(uid)
    };
    if setuid_rc != 0 {
        return Err(format!("failed to drop user privileges to '{}'", user));
    }

    let effective_uid = unsafe {
        // SAFETY: simple libc getter.
        libc::geteuid()
    };
    if effective_uid == 0 {
        return Err("privilege drop verification failed: still running as root".to_string());
    }

    Ok(())
}

#[cfg(not(unix))]
#[allow(dead_code)]
pub fn drop_privileges(_user: &str, _group: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::drop_privileges;

    #[cfg(unix)]
    #[test]
    fn rejects_unknown_group_or_user_before_system_calls() {
        let missing_group = format!("missing-group-{}", std::process::id());
        let result = drop_privileges("nobody", &missing_group);
        assert!(result.is_err());

        let missing_user = format!("missing-user-{}", std::process::id());
        let result = drop_privileges(&missing_user, "nogroup");
        assert!(result.is_err());
    }
}
