use {
    crate::{
        cgroup::manager::{CgroupController, CgroupManager},
        mounts::get_cgroup_info,
    },
    std::sync::OnceLock,
};

static MANAGER: OnceLock<CgroupManager> = OnceLock::new();

fn get_manager() -> &'static CgroupManager {
    MANAGER.get_or_init(|| {
        let info = get_cgroup_info();
        CgroupManager::new(info)
    })
}

pub fn create_cgroup(cgroup_name: &str, cpu_quota: Option<u32>) -> bool {
    let manager = get_manager();

    if manager.cgroup_exists(cgroup_name) {
        tracing::warn!("cgroup {} already exists, ignoring", cgroup_name);
        return false;
    }

    if let Some(target) = manager.ensure_child(cgroup_name) {
        if let Some(quota) = cpu_quota {
            manager.set_cpu_max(&target, quota);
        }
        true
    } else {
        false
    }
}

pub fn add_pid_to_cgroup(pid: i32, cgroup_name: &str) -> Result<(), ananicy_core::worker::PlatformError> {
    let manager = get_manager();

    // We assume the caller or `create_cgroup` has already ensured the target directory exists,
    // but just in case, we could call ensure_child or just attempt to move.
    // The previous implementation required the directory to exist already.
    // Let's resolve the path by trying to ensure it, or just use internal resolve if possible.
    // For now, ensuring it is fine since it's idempotent.
    if let Some(target) = manager.ensure_child(cgroup_name) {
        if manager.move_pid(pid, &target) {
            Ok(())
        } else {
            Err(ananicy_core::worker::PlatformError::Unsupported)
        }
    } else {
        Err(ananicy_core::worker::PlatformError::NotFound)
    }
}
