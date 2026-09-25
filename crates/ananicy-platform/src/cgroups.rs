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

pub fn create_cgroup(cgroup_name: &str, settings: CgroupSettings) -> bool {
    let Some(manager) = get_manager() else {
        debug!(
            "cgroup {} not created: no hierarchy to create it in",
            cgroup_name
        );
        return false;
    };

    if manager.cgroup_exists(cgroup_name) {
        debug!("cgroup {} already exists, ignoring", cgroup_name);
        return false;
    }

    let Some(target) = manager.ensure_child(cgroup_name) else {
        return false;
    };

    if let Some(quota) = settings.cpu_quota {
        manager.set_cpu_max(&target, quota);
    }
    if let Some(weight) = settings.cpu_weight {
        manager.set_cpu_weight(&target, weight);
    }
    true
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
        std::path::PathBuf,
    };

    /// The manager currently cached, and the hierarchy it was built from.
    fn cached_info() -> Option<CgroupInfo> {
        MANAGER
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().map(|manager| manager.info().clone()))
    }

    /// A single test drives the cache: `MANAGER` is process-global, so two tests
    /// mutating it would race.
    #[test]
    fn a_re_detection_reaches_the_cgroup_operations() {
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
