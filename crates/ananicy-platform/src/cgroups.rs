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

pub fn add_pid_to_cgroup(
    pid: i32,
    cgroup_name: &str,
) -> Result<(), ananicy_core::worker::PlatformError> {
    let manager = get_manager();

    // In C++, the cgroup must have been created already by `.cgroups` rules
    // (i.e., `create_cgroup`). If it doesn't exist, we error out to match parity.
    if let Some(target) = manager.resolve_target_dir(cgroup_name) {
        if !target.exists() {
            return Err(ananicy_core::worker::PlatformError::NotFound);
        }
        if manager.move_pid(pid, &target) {
            Ok(())
        } else {
            Err(ananicy_core::worker::PlatformError::Unsupported)
        }
    } else {
        Err(ananicy_core::worker::PlatformError::NotFound)
    }
}
