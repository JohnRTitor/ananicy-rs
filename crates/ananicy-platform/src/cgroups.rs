use {
    ananicy_core::worker::{
        PlatformError,
        PlatformError::{NotFound, Unsupported},
    },
    std::sync::{Arc, RwLock},
    tracing::debug,
};

use crate::{
    cgroup::manager::{CgroupController, CgroupManager},
    mounts::get_cgroup_info,
};

/// The manager for the currently detected hierarchy.
///
/// It is built on first use and then reused, so a running daemon does not
/// re-read the mount table for every process it tunes. Caching it is only safe
/// while [`reset_cgroup_detection`] exists to drop it again: the
/// `cgroup_realtime_workaround` re-creates the cgroups after a short delay
/// because systemd may only just have created the delegated subtree, and a
/// cached manager would keep resolving names against the hierarchy as it looked
/// before that.
static MANAGER: RwLock<Option<Arc<CgroupManager>>> = RwLock::new(None);

/// The cgroup manager, building it from the current detection on first use.
fn get_manager() -> Option<Arc<CgroupManager>> {
    if let Some(cached) = MANAGER.read().ok().and_then(|guard| guard.clone()) {
        return Some(cached);
    }

    let mut guard = MANAGER.write().ok()?;
    Some(
        guard
            .get_or_insert_with(|| Arc::new(CgroupManager::new(get_cgroup_info())))
            .clone(),
    )
}

/// Forgets the detected hierarchy and everything derived from it.
///
/// The next cgroup operation re-detects the mount table and rebuilds the
/// manager, which is what makes a re-detection actually reach the cgroup code
/// paths instead of only the read-only accessors.
pub fn reset_cgroup_detection() {
    crate::mounts::reset_cgroup_info();
    if let Ok(mut guard) = MANAGER.write() {
        *guard = None;
    }
}

/// Whether a cgroup hierarchy is available, without caching the answer.
///
/// Used to decide whether waiting can help at all; deliberately does not build
/// the manager, so that a later detection is still free to change its mind.
pub fn has_cgroup_hierarchy() -> bool {
    get_cgroup_info().version != crate::cgroup::CgroupVersion::None
}

/// The resource settings a `.cgroups` rule can ask a cgroup to be created with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CgroupSettings {
    /// A percentage of the machine's CPUs, written to `cpu.max` on cgroup v2 and
    /// to `cpu.cfs_quota_us` on cgroup v1. Clamped to `0..=100`.
    pub cpu_quota: Option<u32>,
    /// A relative weight, written to `cpu.weight` on cgroup v2 and to
    /// `cpu.shares` on cgroup v1. The kernel default is 100, the range is
    /// `1..=10000`; anything outside it is clamped.
    pub cpu_weight: Option<u32>,
}

/// Creates the cgroup a rule asks for and applies the rule's settings to it.
///
/// Returns whether the cgroup was created, not whether the settings took effect.
/// An existing cgroup is reported as not created but still has the settings
/// applied, which is the point: a cgroup can already be there because a previous
/// run made it, because another rule names it, or because it predates the rule
/// being edited, and in none of those cases is its current configuration
/// necessarily the one this rule asks for. Skipping the settings for an existing
/// cgroup meant editing a `.cgroups` file had no effect until the cgroup was
/// deleted by hand. The reference re-applies them unconditionally for the same
/// reason.
pub fn create_cgroup(cgroup_name: &str, settings: CgroupSettings) -> bool {
    let Some(manager) = get_manager() else {
        debug!(
            "cgroup {} not created: no hierarchy to create it in",
            cgroup_name
        );
        return false;
    };

    // Resolved once. `cgroup_exists` used to answer this by resolving the target
    // and dropping the result, and then the branch that acted on the answer
    // resolved the same name again — two identical `PathBuf`s built to ask one
    // question, on a path that runs for every `.cgroups` rule at every reload.
    let (target, created) = match manager.resolve_target_dir(cgroup_name) {
        Some(target) if target.exists() => (target, false),
        // Either there is no hierarchy to name a path in, or the cgroup is not
        // there yet. Both are `ensure_child`'s to answer, and it resolves the
        // name again because creating it is where the path is actually needed.
        _ => match manager.ensure_child(cgroup_name) {
            Some(target) => (target, true),
            None => return false,
        },
    };

    // Applied either way. A failure to write is reported by the manager, which
    // knows the difference between "the controller is not enabled here" and a
    // real write error, and says so once per cgroup rather than once per
    // process.
    if let Some(quota) = settings.cpu_quota {
        manager.set_cpu_max(&target, quota);
    }
    if let Some(weight) = settings.cpu_weight {
        manager.set_cpu_weight(&target, weight);
    }
    created
}

pub fn add_pid_to_cgroup(pid: i32, cgroup_name: &str) -> Result<(), PlatformError> {
    let manager = get_manager().ok_or(Unsupported)?;

    // A cgroup is expected to be created first, either by `create_cgroup` or by
    // a `.cgroups` rule that declares it. Moving a process into a target that
    // does not exist is reported as `NotFound` instead of creating it here, so
    // a typo in a rule cannot silently create a cgroup nobody configured.
    let target = manager.resolve_target_dir(cgroup_name).ok_or(NotFound)?;
    if !target.exists() {
        return Err(NotFound);
    }
    if manager.move_pid(pid, &target) {
        Ok(())
    } else {
        Err(Unsupported)
    }
}

pub fn set_cpu_weight_for_cgroup(cgroup_name: &str, weight: u32) -> Result<(), PlatformError> {
    let manager = get_manager().ok_or(Unsupported)?;
    let target = manager.resolve_target_dir(cgroup_name).ok_or(NotFound)?;
    if manager.set_cpu_weight(&target, weight) {
        Ok(())
    } else {
        Err(Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{cgroup::CgroupVersion, mounts::CgroupInfo},
        std::{fs, path::PathBuf},
    };

    /// The manager currently cached, and the hierarchy it was built from.
    fn cached_info() -> Option<CgroupInfo> {
        MANAGER
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().map(|manager| manager.info().clone()))
    }

    /// `MANAGER` is process-global and these tests replace it wholesale, so they
    /// have to take turns. Every test that injects a hierarchy holds this.
    static MANAGER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The regression: a `.cgroups` rule edited while its cgroup already exists
    /// had no effect, because the early return on "it exists" happened before the
    /// settings were written. The reference re-applies them, which is what makes
    /// editing the file work.
    ///
    /// The cgroup is created here, the controller files are put inside it by hand
    /// -- they would be created by the kernel in a real hierarchy -- and the rule
    /// is then applied again with a different value.
    #[test]
    fn a_rule_is_reapplied_to_a_cgroup_that_already_exists() {
        let _guard = MANAGER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let hierarchy = tempfile::tempdir().expect("a temporary hierarchy");
        let mount = hierarchy.path().to_path_buf();

        *MANAGER.write().expect("an uncontended lock") =
            Some(Arc::new(CgroupManager::new(CgroupInfo {
                version: CgroupVersion::V1,
                mount_point: mount.clone(),
            })));

        assert!(
            create_cgroup(
                "reapply",
                CgroupSettings {
                    cpu_quota: Some(90),
                    ..Default::default()
                }
            ),
            "the first call creates it"
        );

        let dir = mount.join("cpu/reapply");
        let period_file = dir.join("cpu.cfs_period_us");
        let quota_file = dir.join("cpu.cfs_quota_us");
        let shares_file = dir.join("cpu.shares");
        // All three, as the kernel would provide them. The v1 path writes the
        // period before the quota and gives up if it cannot, so a partial set
        // here would test nothing.
        fs::write(&period_file, "1000000\n").expect("a period file");
        fs::write(&quota_file, "0\n").expect("a quota file");
        fs::write(&shares_file, "1024\n").expect("a shares file");

        // Second call: not created, but the settings must be written.
        assert!(
            !create_cgroup(
                "reapply",
                CgroupSettings {
                    cpu_quota: Some(50),
                    cpu_weight: Some(256),
                },
            ),
            "an existing cgroup is reported as not created"
        );

        let quota = fs::read_to_string(&quota_file).expect("read back the quota");
        let shares = fs::read_to_string(&shares_file).expect("read back the shares");
        assert_ne!(
            quota.trim(),
            "0",
            "the quota must be re-applied to an existing cgroup, not left at what it was"
        );
        assert_ne!(
            shares.trim(),
            "1024",
            "the weight must be re-applied to an existing cgroup too"
        );
    }

    /// A single test drives the cache: `MANAGER` is process-global, so two tests
    /// mutating it would race.
    #[test]
    fn a_re_detection_reaches_the_cgroup_operations() {
        let _guard = MANAGER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let hierarchy = tempfile::tempdir().expect("a temporary hierarchy");

        // Pretend the daemon detected this cgroup v1 hierarchy.
        *MANAGER.write().expect("an uncontended lock") =
            Some(Arc::new(CgroupManager::new(CgroupInfo {
                version: CgroupVersion::V1,
                mount_point: PathBuf::from(hierarchy.path()),
            })));

        // An operation uses the cached detection, and creates in it.
        assert!(create_cgroup("cached", CgroupSettings::default()));
        assert!(
            hierarchy.path().join("cpu/cached").is_dir(),
            "the cgroup is created under the detected hierarchy"
        );
        assert_eq!(
            cached_info().map(|info| info.mount_point),
            Some(hierarchy.path().into())
        );

        // Re-detecting drops the cached manager along with the mount-table
        // cache, so the next operation has to rebuild from the live hierarchy
        // instead of reusing the injected one.
        reset_cgroup_detection();
        let _ = create_cgroup("after-reset", CgroupSettings::default());

        let rebuilt = cached_info().expect("the next operation rebuilds the manager");
        assert_ne!(
            rebuilt.mount_point,
            hierarchy.path(),
            "the manager must be rebuilt from the live detection, not the cached one"
        );
        assert!(
            !hierarchy.path().join("cpu/after-reset").exists(),
            "nothing is created in the hierarchy that was cached before the reset"
        );
    }
}
