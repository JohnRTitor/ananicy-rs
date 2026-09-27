use {
    std::fs::{OpenOptions, read_to_string},
    tracing::{debug, warn},
};

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

/// Moves the delegated root up one level when that is the only level at which a
/// controller can be enabled.
///
/// The root is the parent of wherever this process sits. That is the right
/// answer for a plain `Delegate=yes` unit, where the daemon is the only thing in
/// the unit's cgroup and the unit's cgroup is therefore the boundary systemd
/// delegated. It is the wrong answer when the unit also sets `DelegateSubgroup=`,
/// because then the daemon sits one level lower and the delegated boundary is the
/// unit's cgroup, not the sub-cgroup.
///
/// The distinction is not something the daemon can read from its environment, so
/// it is discovered from the shape of the tree instead: a cgroup that still holds
/// processes cannot have a domain controller enabled underneath it
/// (`cgroup_vet_subtree_control_enable()` returns `-EBUSY` when
/// `cgroup_has_tasks()`), so a root we hold processes in is a root we cannot use,
/// while an empty parent is exactly what we need.
///
/// The step up is bounded to one level and requires the parent to be empty, which
/// is what keeps this from walking out of the delegated subtree: the hierarchy
/// root always holds PID 1, so it can never qualify, and a container's root
/// usually does not exist inside its own mount. A root we cannot improve is
/// returned unchanged, and the caller reports the `-EBUSY` it gets.
fn usable_root(root: PathBuf) -> PathBuf {
    let Some(parent) = root.parent() else {
        return root;
    };
    if !has_tasks(&root) {
        // Already empty: this is the unit's own cgroup and it is usable as it is.
        return root;
    }
    if parent.join("cgroup.procs").exists() && !has_tasks(parent) {
        debug!(
            "Cgroup v2: Delegated root {} still holds processes, so a controller cannot \
             be enabled in it; using its parent {} instead.",
            root.display(),
            parent.display()
        );
        return parent.to_path_buf();
    }
    root
}

/// Whether a cgroup currently holds any process.
///
/// `cgroup.procs` lists one pid per line, so an empty file is a cgroup with no
/// processes in it. A file that cannot be read is reported as holding tasks, so an
/// unreadable cgroup is never mistaken for an empty one.
fn has_tasks(cgroup: &Path) -> bool {
    match read_to_string(cgroup.join("cgroup.procs")) {
        Ok(list) => !list.trim().is_empty(),
        Err(_) => true,
    }
}

/// Helper to determine the delegated root by inspecting our own process's cgroup.
/// For systemd services, this is typically something like `/sys/fs/cgroup/system.slice/ananicy.service`.
pub fn discover_delegated_root(mount_point: &Path) -> Option<PathBuf> {
    // We read /proc/self/cgroup and find the unified hierarchy path
    let content = read_to_string("/proc/self/cgroup").ok()?;
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
                warn!(
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
                return Some(usable_root(full_path));
            }
        }
    }
    None
}

fn is_writable(path: &Path) -> bool {
    // Basic write access check by attempting to open for append/write
    OpenOptions::new().append(true).open(path).is_ok()
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

#[cfg(test)]
mod usable_root_tests {
    use super::usable_root;
    use std::path::Path;

    fn cgroup(dir: &Path, pids: &[u32]) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).expect("a cgroup directory");
        std::fs::write(
            dir.join("cgroup.procs"),
            pids.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .expect("cgroup.procs");
        dir.to_path_buf()
    }

    /// The case that made CPU bandwidth rules inert. The unit sets
    /// DelegateSubgroup=, so the daemon sits one level below the cgroup systemd
    /// actually delegated, and the sub-cgroup it sits in cannot have `cpu` enabled
    /// underneath it because it holds the daemon. The parent is empty, so it is
    /// the usable root.
    #[test]
    fn a_populated_root_with_an_empty_parent_steps_up_one_level() {
        let dir = tempfile::tempdir().expect("a temporary hierarchy");
        let unit = cgroup(&dir.path().join("ananicy-rs.service"), &[]);
        let sub = cgroup(&unit.join("delegated"), &[4242]);

        assert_eq!(
            usable_root(sub),
            unit,
            "the sub-cgroup holds the daemon, so the unit cgroup is the boundary"
        );
    }

    /// A plain Delegate=yes unit: the daemon is the only thing in the unit's own
    /// cgroup, and that cgroup is empty as far as *other* processes go -- but it
    /// holds us, so it cannot take a controller. Its parent is the slice, which is
    /// full of processes, so there is nowhere to step up to and the root stands.
    #[test]
    fn a_populated_root_whose_parent_is_also_populated_stays_put() {
        let dir = tempfile::tempdir().expect("a temporary hierarchy");
        let slice = cgroup(&dir.path().join("system.slice"), &[1, 2, 3]);
        let unit = cgroup(&slice.join("ananicy-rs.service"), &[4242]);

        assert_eq!(
            usable_root(unit),
            slice.join("ananicy-rs.service"),
            "system.slice is not empty, so stepping up would leave the delegation"
        );
    }

    /// A root with nothing in it is already what a controller needs, and is used
    /// unchanged -- stepping up would needlessly widen what we may write to.
    #[test]
    fn an_empty_root_is_left_alone() {
        let dir = tempfile::tempdir().expect("a temporary hierarchy");
        let unit = cgroup(&dir.path().join("ananicy-rs.service"), &[]);
        cgroup(&unit.join("..").join("system.slice"), &[1]);

        assert_eq!(usable_root(unit.clone()), unit);
    }

    /// A container's root is the mount point, so it has no parent to step up to
    /// inside the mount, and the hierarchy root always holds PID 1. Neither can be
    /// mistaken for an empty delegated parent.
    #[test]
    fn a_root_at_the_top_of_its_mount_cannot_step_up() {
        let dir = tempfile::tempdir().expect("a temporary hierarchy");
        let root = cgroup(dir.path(), &[1]);

        assert_eq!(usable_root(root.clone()), root);
    }

    /// A cgroup whose cgroup.procs cannot be read is treated as populated. That
    /// matters for the *candidate parent*: the cost of treating an unreadable
    /// parent as empty is adopting a cgroup as ours that we know nothing about,
    /// which is the mistake the whole ownership check exists to prevent. The cost
    /// of the opposite is one refused write.
    #[test]
    fn an_unreadable_parent_is_never_treated_as_an_empty_one() {
        let dir = tempfile::tempdir().expect("a temporary hierarchy");
        let unit = dir.path().join("unit");
        std::fs::create_dir_all(&unit).expect("a cgroup directory");
        // A parent that exists but whose cgroup.procs we cannot read: the
        // directory is there, the file is not.
        let sub = unit.join("delegated");
        std::fs::create_dir_all(&sub).expect("a cgroup directory");
        std::fs::write(sub.join("cgroup.procs"), "4242\n").expect("cgroup.procs");

        assert_eq!(
            usable_root(sub.clone()),
            sub,
            "an unreadable parent must not qualify as an empty one to step up into"
        );
    }
}
