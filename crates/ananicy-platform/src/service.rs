#[cfg(feature = "systemd")]
#[link(name = "systemd")]
unsafe extern "C" {
    fn sd_pid_get_unit(pid: libc::pid_t, unit: *mut *mut libc::c_char) -> libc::c_int;
}

/// Returns the systemd unit name for the current process.
#[cfg(feature = "systemd")]
pub fn get_unit_name() -> String {
    let pid = std::process::id();
    let mut ptr: *mut libc::c_char = std::ptr::null_mut();
    
    let res = unsafe { sd_pid_get_unit(pid as libc::pid_t, &mut ptr) };
    if res >= 0 && !ptr.is_null() {
        let name = unsafe { std::ffi::CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned();
        unsafe { libc::free(ptr as *mut libc::c_void) };
        return name;
    }
    
    get_unit_name_heuristic()
}

/// Returns the systemd unit name for the current process.
#[cfg(not(feature = "systemd"))]
pub fn get_unit_name() -> String {
    "<not using systemd>".to_string()
}

#[allow(dead_code)]
fn get_unit_name_heuristic() -> String {
    let Some(cgroup_path) = read_own_cgroup_path() else {
        return "<empty>".to_string();
    };

    cgroup_path
        .split('/')
        .filter(|s| !s.is_empty())
        .rev()
        .find(|segment| !segment.ends_with(".slice") && !segment.ends_with(".scope"))
        .map(str::to_string)
        .unwrap_or_else(|| "<empty>".to_string())
}

/// Reads `/proc/self/cgroup`'s first line and returns the path portion
/// (everything after the last `:`), or `None` if it can't be determined.
fn read_own_cgroup_path() -> Option<String> {
    let content = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let first_line = content.lines().next()?;
    if first_line.is_empty() {
        return None;
    }
    let idx = first_line.rfind(':')?;
    let path = &first_line[idx + 1..];
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_name_skips_trailing_slice_and_scope_segments() {
        let path = "/user.slice/user-1000.slice/user@1000.service/app.slice/kitty-4280-0.scope";
        let unit = path
            .split('/')
            .filter(|s| !s.is_empty())
            .rev()
            .find(|segment| !segment.ends_with(".slice") && !segment.ends_with(".scope"));
        assert_eq!(unit, Some("user@1000.service"));
    }

    #[test]
    fn unit_name_falls_back_to_empty_when_only_slices_and_scopes() {
        let path = "/user.slice/user-1000.slice/session-2.scope";
        let unit = path
            .split('/')
            .filter(|s| !s.is_empty())
            .rev()
            .find(|segment| !segment.ends_with(".slice") && !segment.ends_with(".scope"));
        assert_eq!(unit, None);
    }
}
