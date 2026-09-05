use {
    std::{
        fs::{self, OpenOptions},
        io::Write,
        path::{Path, PathBuf},
    },
    tracing::{debug, error, warn},
};

use crate::cgroup::{
    CgroupInfo, CgroupVersion,
    ownership::{CgroupOwnership, discover_delegated_root},
};

pub trait CgroupController {
    fn ensure_child(&self, name: &str) -> Option<PathBuf>;
    fn move_pid(&self, pid: i32, target: &Path) -> bool;
    fn set_cpu_max(&self, target: &Path, quota: u32) -> bool;
    fn set_cpu_weight(&self, target: &Path, weight: u32) -> bool;
}

#[derive(Debug, Clone)]
pub struct CgroupManager {
    info: CgroupInfo,
    delegated_root: Option<PathBuf>,
}

impl CgroupManager {
    pub fn new(info: CgroupInfo) -> Self {
        let delegated_root = if info.version == CgroupVersion::V2 {
            discover_delegated_root(&info.mount_point)
        } else {
            None
        };

        if info.version == CgroupVersion::V2
            && let Some(ref root) = delegated_root
        {
            debug!("Cgroup v2: Discovered delegated root at {:?}", root);
        } else if info.version == CgroupVersion::V2 {
            warn!(
                "Cgroup v2: No writable delegated root discovered. Cgroup modifications will be disabled. Please run ananicy-rs as a systemd service with `Delegate=yes`."
            );
        }

        Self {
            info,
            delegated_root,
        }
    }

    /// Creates a CgroupManager with a specific delegated root, useful for testing.
    #[doc(hidden)]
    pub fn new_with_root(info: CgroupInfo, delegated_root: Option<PathBuf>) -> Self {
        Self {
            info,
            delegated_root,
        }
    }

    pub fn info(&self) -> &CgroupInfo {
        &self.info
    }

    /// Helper to resolve the target directory based on the cgroup name and version.
    fn resolve_target_dir(&self, name: &str) -> Option<PathBuf> {
        // Prevent basic path traversal attacks
        if name.contains("..") {
            warn!(
                "Security: Rejecting cgroup name with traversal sequence '..': {}",
                name
            );
            return None;
        }

        let is_absolute = name.starts_with('/');
        let relative_name = name.strip_prefix('/').unwrap_or(name);

        match self.info.version {
            CgroupVersion::None => None,
            CgroupVersion::V1 => {
                let base = self.info.mount_point.join("cpu");
                if relative_name.is_empty() {
                    Some(base)
                } else {
                    Some(base.join(relative_name))
                }
            }
            CgroupVersion::V2 => {
                if is_absolute {
                    // Absolute path from global cgroup mount
                    let base = self.info.mount_point.clone();
                    if relative_name.is_empty() {
                        Some(base)
                    } else {
                        Some(base.join(relative_name))
                    }
                } else if let Some(ref root) = self.delegated_root {
                    // Relative path from delegated root
                    if relative_name.is_empty() {
                        Some(root.clone())
                    } else {
                        Some(root.join(relative_name))
                    }
                } else {
                    // In V2, without a delegated root, relative paths fall back to the base mount point.
                    let base = self.info.mount_point.clone();
                    if relative_name.is_empty() {
                        Some(base)
                    } else {
                        Some(base.join(relative_name))
                    }
                }
            }
        }
    }

    pub fn cgroup_exists(&self, name: &str) -> bool {
        if let Some(target) = self.resolve_target_dir(name) {
            target.exists()
        } else {
            false
        }
    }
}

impl CgroupController for CgroupManager {
    fn ensure_child(&self, name: &str) -> Option<PathBuf> {
        let target_dir = self.resolve_target_dir(name)?;

        let ownership = CgroupOwnership::classify(
            &target_dir,
            self.delegated_root.as_deref(),
            self.info.version == CgroupVersion::V2,
        );

        match ownership {
            CgroupOwnership::Legacy => {
                // Legacy v1 behavior: just create the directory
                if !target_dir.exists() {
                    if let Err(e) = fs::create_dir_all(&target_dir) {
                        error!("ensure_child(V1): Failed to create cgroup {}: {}", name, e);
                        return None;
                    }
                }
                Some(target_dir)
            }
            CgroupOwnership::Owned => {
                // Owned v2 behavior: ensure subtree control on parent before creating child
                if let Some(parent) = target_dir.parent() {
                    // Try to enable cpu controller on parent
                    let subtree_control = parent.join("cgroup.subtree_control");
                    if subtree_control.exists() {
                        if let Ok(mut f) = OpenOptions::new().write(true).open(&subtree_control) {
                            // It's okay if this fails (e.g. if cpu is not in cgroup.controllers)
                            // but we must attempt it to adhere to hierarchy rules.
                            let _ = f.write_all(b"+cpu\n");
                        }
                    }
                }

                if !target_dir.exists() {
                    if let Err(e) = fs::create_dir_all(&target_dir) {
                        error!("ensure_child(V2): Failed to create cgroup {}: {}", name, e);
                        return None;
                    }
                }
                Some(target_dir)
            }
            CgroupOwnership::Foreign => {
                debug!(
                    "ensure_child(V2): Path {:?} is Foreign (not in delegated root). Refusing to create.",
                    target_dir
                );
                None
            }
        }
    }

    fn move_pid(&self, pid: i32, target: &Path) -> bool {
        let ownership = CgroupOwnership::classify(
            target,
            self.delegated_root.as_deref(),
            self.info.version == CgroupVersion::V2,
        );

        if ownership == CgroupOwnership::Foreign {
            warn!(
                "move_pid: Target {:?} is Foreign. Refusing to write.",
                target
            );
            return false;
        }

        if !target.exists() {
            error!("move_pid: Cgroup {:?} does not exist.", target);
            return false;
        }

        let procs_file = if self.info.version == CgroupVersion::V2 {
            "cgroup.procs"
        } else {
            "tasks"
        };
        let procs_path = target.join(procs_file);

        let start_time_before = crate::procfs::get_start_time(pid);

        if start_time_before.is_none() {
            // Process doesn't exist or we can't read its stat.
            return false;
        }

        // If in CgroupV2, we must write the TGID (process ID), not the TID, to cgroup.procs
        // Writing a TID that is not a thread group leader to cgroup.procs fails with EINVAL (os error 22)
        let pid_to_write = if self.info.version == CgroupVersion::V2 {
            crate::procfs::get_tgid(pid).unwrap_or(pid)
        } else {
            pid
        };

        match OpenOptions::new().write(true).open(&procs_path) {
            Ok(mut file) => {
                // CRITICAL: Do NOT use writeln!() here. On an unbuffered File,
                // writeln!(f, "{}", pid) performs two separate write() syscalls:
                //   write(fd, "1234", 4)  → succeeds (kernel moves the PID)
                //   write(fd, "\n", 1)    → EINVAL (kernel tries to parse "\n" as a PID)
                // Instead, format the PID + newline into a single buffer so that
                // write_all() issues one atomic write() syscall.
                let pid_buf = format!("{}\n", pid_to_write);
                if let Err(e) = file.write_all(pid_buf.as_bytes()) {
                    error!("move_pid: Failed to write to {:?}: {}", procs_path, e);
                    return false;
                }

                let start_time_after = crate::procfs::get_start_time(pid);
                match (start_time_before, start_time_after) {
                    (Some(before), Some(after)) if before == after => {} // Safe
                    (Some(_), None) => {
                        debug!("move_pid: PID {} died during move operation", pid);
                        // It died, so the move succeeded but the process is gone.
                        // Returning true is fine, or false. We'll return false to stop further rules.
                        return false;
                    }
                    (Some(before), Some(after)) => {
                        warn!(
                            "move_pid: PID {} was reused during move operation! ({} -> {})",
                            pid, before, after
                        );
                        return false;
                    }
                    _ => return false,
                }

                debug!("move_pid: Successfully added {} to {:?}", pid, target);
                true
            }
            Err(e) => {
                error!("move_pid: Failed to open {:?}: {}", procs_path, e);
                false
            }
        }
    }

    fn set_cpu_max(&self, target: &Path, quota: u32) -> bool {
        let ownership = CgroupOwnership::classify(
            target,
            self.delegated_root.as_deref(),
            self.info.version == CgroupVersion::V2,
        );

        if ownership == CgroupOwnership::Foreign {
            warn!(
                "set_cpu_max: Target {:?} is Foreign. Refusing to write.",
                target
            );
            return false;
        }

        let logical_cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1) as u32;
        let clamped_quota = quota.clamp(0, 100);

        if self.info.version == CgroupVersion::V2 {
            let period = 100_000u32;
            let quota_val = period * logical_cores * clamped_quota / 100;
            let max_file = target.join("cpu.max");
            if let Ok(mut f) = OpenOptions::new().write(true).open(&max_file) {
                let buf = format!("{} {}\n", quota_val, period);
                let _ = f.write_all(buf.as_bytes());
            }
        } else {
            let period = 1_000_000u32;
            let quota_val = period * logical_cores * clamped_quota / 100;
            let period_file = target.join("cpu.cfs_period_us");
            if let Ok(mut f) = OpenOptions::new().write(true).open(&period_file) {
                let buf = format!("{}\n", period);
                let _ = f.write_all(buf.as_bytes());
            }
            let quota_file = target.join("cpu.cfs_quota_us");
            if let Ok(mut f) = OpenOptions::new().write(true).open(&quota_file) {
                let buf = format!("{}\n", quota_val);
                let _ = f.write_all(buf.as_bytes());
            }
        }
        true
    }

    fn set_cpu_weight(&self, target: &Path, weight: u32) -> bool {
        let ownership = CgroupOwnership::classify(
            target,
            self.delegated_root.as_deref(),
            self.info.version == CgroupVersion::V2,
        );

        if ownership == CgroupOwnership::Foreign {
            warn!(
                "set_cpu_weight: Target {:?} is Foreign. Refusing to write.",
                target
            );
            return false;
        }

        if self.info.version == CgroupVersion::V2 {
            // Valid values are 1-10000. Default is 100.
            let weight_val = weight.clamp(1, 10000);
            let weight_file = target.join("cpu.weight");
            if let Ok(mut f) = OpenOptions::new().write(true).open(&weight_file) {
                let buf = format!("{}\n", weight_val);
                let _ = f.write_all(buf.as_bytes());
            }
        } else {
            // v1 doesn't have cpu.weight exactly, it has cpu.shares (default 1024, range 2-262144)
            // 100 weight ~ 1024 shares.
            let shares = (weight as f32 / 100.0 * 1024.0) as u32;
            let shares = shares.clamp(2, 262144);
            let shares_file = target.join("cpu.shares");
            if let Ok(mut f) = OpenOptions::new().write(true).open(&shares_file) {
                let buf = format!("{}\n", shares);
                let _ = f.write_all(buf.as_bytes());
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::mounts::{CgroupInfo, CgroupVersion},
        std::path::PathBuf,
    };

    #[test]
    fn test_resolve_target_dir_traversal() {
        let manager = CgroupManager {
            info: CgroupInfo {
                mount_point: PathBuf::from("/sys/fs/cgroup"),
                version: CgroupVersion::V2,
            },
            delegated_root: Some(PathBuf::from("/sys/fs/cgroup/system.slice/ananicy.service")),
        };

        // Traversal attempt should be rejected
        let res = manager.resolve_target_dir("../../other.slice");
        assert!(res.is_none());

        let res = manager.resolve_target_dir("some/../../path");
        assert!(res.is_none());

        let res = manager.resolve_target_dir("/../../etc/passwd");
        assert!(res.is_none());
    }
}
