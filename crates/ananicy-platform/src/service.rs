use std::fs::read_to_string;
#[cfg(feature = "systemd")]
use std::process::id;

#[cfg(feature = "systemd")]
use std::ffi::CStr;
#[cfg(feature = "systemd")]
#[link(name = "systemd")]
unsafe extern "C" {
    fn sd_pid_get_unit(pid: libc::pid_t, unit: *mut *mut libc::c_char) -> libc::c_int;
}

/// Returns the systemd unit name for the current process.
#[cfg(feature = "systemd")]
pub fn get_unit_name() -> String {
    let pid = id();
    let mut ptr: *mut libc::c_char = std::ptr::null_mut();

    let res = unsafe { sd_pid_get_unit(pid as libc::pid_t, &mut ptr) };
    if res >= 0 && !ptr.is_null() {
        let name = unsafe { CStr::from_ptr(ptr) }
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

/// Derives the systemd unit name from a cgroup path.
///
/// The deepest segment that is neither a `.slice` nor a `.scope` is the unit
/// itself: a desktop application usually lives in
/// `…/app.slice/kitty-4280-0.scope`, but the unit that owns it is
/// `user@1000.service`. Returns `None` when the path contains nothing but
/// slices and scopes, i.e. when no unit owns this cgroup.
fn unit_name_from_cgroup_path(cgroup_path: &str) -> Option<String> {
    cgroup_path
        .split('/')
        .filter(|s| !s.is_empty())
        .rev()
        .find(|segment| !segment.ends_with(".slice") && !segment.ends_with(".scope"))
        .map(str::to_string)
}

#[allow(dead_code)]
fn get_unit_name_heuristic() -> String {
    let Some(cgroup_path) = read_own_cgroup_path() else {
        return "<empty>".to_string();
    };

    unit_name_from_cgroup_path(&cgroup_path).unwrap_or_else(|| "<empty>".to_string())
}

/// Reads `/proc/self/cgroup`'s first line and returns the path portion
/// (everything after the last `:`), or `None` if it can't be determined.
fn read_own_cgroup_path() -> Option<String> {
    let content = read_to_string("/proc/self/cgroup").ok()?;
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
        assert_eq!(
            unit_name_from_cgroup_path(
                "/user.slice/user-1000.slice/user@1000.service/app.slice/kitty-4280-0.scope"
            )
            .as_deref(),
            Some("user@1000.service")
        );
    }

    #[test]
    fn unit_name_is_the_deepest_non_slice_segment() {
        assert_eq!(
            unit_name_from_cgroup_path("/system.slice/systemd-journald.service").as_deref(),
            Some("systemd-journald.service")
        );
        assert_eq!(
            unit_name_from_cgroup_path("/user.slice/user-1000.slice/session-2.scope/app-runnable")
                .as_deref(),
            Some("app-runnable")
        );
        assert_eq!(
            unit_name_from_cgroup_path("/user.slice/user-1000.slice/session-2.scope/work.scope"),
            None,
            "every segment is a slice or a scope, so no unit owns this cgroup"
        );
    }

    #[test]
    fn unit_name_falls_back_to_empty_when_only_slices_and_scopes() {
        assert_eq!(
            unit_name_from_cgroup_path("/user.slice/user-1000.slice/session-2.scope"),
            None
        );
        assert_eq!(unit_name_from_cgroup_path(""), None);
        assert_eq!(unit_name_from_cgroup_path("/"), None);
    }

    #[test]
    fn a_cgroup_path_is_read_from_the_procfs_cgroup_file() {
        // System test: whatever this process' cgroup is, it must be an absolute
        // path (or absent, on a host without cgroups).
        if let Some(path) = read_own_cgroup_path() {
            assert!(path.starts_with('/'), "not a cgroup path: {path:?}");
        }
    }

    /// System test: the reported unit name is either a real unit or the
    /// documented placeholder, never empty.
    #[test]
    fn the_reported_unit_name_is_never_empty() {
        let name = get_unit_name();
        assert!(!name.is_empty());
    }
}
