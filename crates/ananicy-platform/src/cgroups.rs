use {
    ananicy_core::worker::{
        PlatformError,
        PlatformError::{NotFound, Unsupported},
    },
    std::sync::OnceLock,
    tracing::warn,
};

use crate::{
    cgroup::manager::{CgroupController, CgroupManager},
    mounts::get_cgroup_info,
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
        warn!("cgroup {} already exists, ignoring", cgroup_name);
        return false;
    }

    let Some(target) = manager.ensure_child(cgroup_name) else {
        return false;
    };

    if let Some(quota) = cpu_quota {
        manager.set_cpu_max(&target, quota);
    }
    true
}

pub fn add_pid_to_cgroup(pid: i32, cgroup_name: &str) -> Result<(), PlatformError> {
    let manager = get_manager();

    // In C++, the cgroup must have been created already by `.cgroups` rules
    // (i.e., `create_cgroup`). If it doesn't exist, we error out to match parity.
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
    let manager = get_manager();
    let target = manager.resolve_target_dir(cgroup_name).ok_or(NotFound)?;
    if manager.set_cpu_weight(&target, weight) {
        Ok(())
    } else {
        Err(Unsupported)
    }
}
