use std::path::{Path, PathBuf};

/// Represents the ownership classification of a target cgroup path.
///
/// In cgroups v2, the kernel enforces a "single-writer rule", meaning a single subtree
/// should only be managed by one writer (e.g., systemd). Multiple concurrent writers
/// modifying the same cgroup directory will cause conflicts, race conditions, and
/// undefined behavior in systemd's state tracking.
///
/// `ananicy-rs` strictly adheres to this rule by only performing active cgroup mutations
/// (like creating directories or modifying controllers) in its **Owned** delegated subtree
/// (usually `/sys/fs/cgroup/system.slice/ananicy.service`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CgroupOwnership {
    /// Legacy cgroup v1 handling. We don't track delegation closely here.
    Legacy,
    /// We own this v2 cgroup (it is at or below our delegated root).
    /// It is safe to perform mutations like creating directories and modifying controllers.
    Owned,
    /// Foreign v2 cgroup (managed by systemd or another entity).
    /// We cannot safely write to this directory. `ananicy-rs` will refuse to perform
    /// any structural modifications here.
    Foreign,
}

impl CgroupOwnership {
    /// Classifies a target path based on whether it is a descendant of the delegated root.
    pub fn classify(
        target_path: &Path,
        delegated_root: Option<&Path>,
        is_v2: bool,
    ) -> CgroupOwnership {
        if !is_v2 {
            return CgroupOwnership::Legacy;
        }

        if let Some(root) = delegated_root
            && target_path.starts_with(root)
        {
            // target_path must start with delegated_root to be Owned
            return CgroupOwnership::Owned;
        }

        CgroupOwnership::Foreign
    }
}

/// Helper to determine the delegated root by inspecting our own process's cgroup.
/// For systemd services, this is typically something like `/sys/fs/cgroup/system.slice/ananicy.service`.
pub fn discover_delegated_root(mount_point: &Path) -> Option<PathBuf> {
    // We read /proc/self/cgroup and find the unified hierarchy path
    let content = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    for line in content.lines() {
        if line.starts_with("0::") {
            let path = line.trim_start_matches("0::");
            let path = path.trim_start_matches('/'); // remove leading slash for joining

            // Reject transient systemd scopes (e.g. app-org.chromium.Chromium-1234.scope).
            // When ananicy-rs is launched via `sudo` from a user shell, /proc/self/cgroup
            // inherits the shell's cgroup, which is typically a .scope created by
            // systemd-logind. Root can write to any cgroup.procs file, so the
            // is_writable check below would incorrectly pass, causing ananicy-rs to
            // adopt a foreign scope as its delegated root. This leads to move_pid
            // writing PIDs into cgroups managed by systemd, producing EINVAL errors.
            // Only .service cgroups (our own systemd unit) are valid delegation targets.
            if path.ends_with(".scope") {
                tracing::warn!(
                    "Cgroup v2: Detected manual execution inside a transient .scope ('{}'). \
                     Cgroup mutations are disabled to prevent hijacking. \
                     Please run ananicy-rs as a systemd service with `Delegate=yes`.",
                    path
                );
                return None;
            }

            let full_path = mount_point.join(path);

            // Check if we actually have write access to it.
            // A good heuristic for delegation is if we can write to cgroup.procs
            if is_writable(&full_path.join("cgroup.procs")) {
                return Some(full_path);
            }
        }
    }
    None
}

fn is_writable(path: &Path) -> bool {
    // Basic write access check by attempting to open for append/write
    std::fs::OpenOptions::new()
        .write(true)
        .append(true)
        .open(path)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use {super::*, std::path::Path};

    #[test]
    fn test_classify_v1() {
        assert_eq!(
            CgroupOwnership::classify(Path::new("/foo/bar"), None, false),
            CgroupOwnership::Legacy
        );
    }

    #[test]
    fn test_classify_v2_foreign() {
        assert_eq!(
            CgroupOwnership::classify(
                Path::new("/sys/fs/cgroup/user.slice"),
                Some(Path::new("/sys/fs/cgroup/system.slice/ananicy.service")),
                true
            ),
            CgroupOwnership::Foreign
        );

        assert_eq!(
            CgroupOwnership::classify(
                Path::new("/sys/fs/cgroup/system.slice"),
                Some(Path::new("/sys/fs/cgroup/system.slice/ananicy.service")),
                true
            ),
            CgroupOwnership::Foreign
        );
    }

    #[test]
    fn test_classify_v2_owned() {
        assert_eq!(
            CgroupOwnership::classify(
                Path::new("/sys/fs/cgroup/system.slice/ananicy.service/foo"),
                Some(Path::new("/sys/fs/cgroup/system.slice/ananicy.service")),
                true
            ),
            CgroupOwnership::Owned
        );

        assert_eq!(
            CgroupOwnership::classify(
                Path::new("/sys/fs/cgroup/system.slice/ananicy.service"),
                Some(Path::new("/sys/fs/cgroup/system.slice/ananicy.service")),
                true
            ),
            CgroupOwnership::Owned
        );
    }
}
