#![allow(clippy::collapsible_if, clippy::io_other_error)]
pub mod abi;
pub mod cgroup;
pub mod cgroups;
pub mod mounts;
pub mod netlink;
pub mod priority;
pub mod procfs;
pub mod topology;
pub mod x3d;

use {
    ananicy_core::{cgroup::CgroupIdentity, cpuset::CpuSet, worker::PlatformActions},
    cgroup::process::{CgroupProcessResolver, LinuxCgroupResolver},
    mounts::{CgroupVersion, get_cgroup_info},
};

pub struct LinuxPlatform {
    cgroup_resolver: cgroup::process::CachingCgroupResolver<LinuxCgroupResolver>,
}

impl LinuxPlatform {
    pub fn new() -> Self {
        let version = get_cgroup_info().version;
        let inner = LinuxCgroupResolver::new(version);
        Self {
            cgroup_resolver: cgroup::process::CachingCgroupResolver::new(
                inner,
                5000,
                std::time::Duration::from_secs(1),
            ),
        }
    }
}

impl PlatformActions for LinuxPlatform {
    fn is_realtime(&self, pid: i32) -> bool {
        // Read /proc/<pid>/stat and check the policy field (policy is field 41)
        // Or simply check if sched_getscheduler returns SCHED_FIFO or SCHED_RR
        if let Ok(sched) = crate::abi::sched::sched_getscheduler(pid) {
            sched == crate::abi::sched::SCHED_FIFO || sched == crate::abi::sched::SCHED_RR
        } else {
            false
        }
    }

    fn get_start_time(&self, pid: i32) -> Option<u64> {
        crate::procfs::get_start_time(pid)
    }

    fn get_process_name(&self, pid: i32) -> String {
        crate::procfs::get_command_from_pid(pid)
    }

    fn is_cgroup_v2(&self) -> bool {
        get_cgroup_info().version == CgroupVersion::V2
    }

    fn get_max_cores(&self) -> u32 {
        unsafe { libc::sysconf(libc::_SC_NPROCESSORS_CONF) as u32 }
    }

    fn set_priority(&self, pid: i32, nice: i32) -> Result<(), ananicy_core::worker::PlatformError> {
        priority::set_priority(pid, nice)
    }

    fn set_latency_nice(
        &self,
        pid: i32,
        lat_nice: i32,
    ) -> Result<(), ananicy_core::worker::PlatformError> {
        priority::set_latency_nice(pid, lat_nice)
    }

    fn set_sched(
        &self,
        pid: i32,
        sched: &str,
        rtprio: u32,
    ) -> Result<(), ananicy_core::worker::PlatformError> {
        priority::set_sched(pid, sched, rtprio)
    }

    fn set_io_priority(
        &self,
        pid: i32,
        ioclass: &str,
        ionice: i32,
    ) -> Result<(), ananicy_core::worker::PlatformError> {
        priority::set_io_priority(pid, ioclass, ionice)
    }

    fn set_oom_score_adj(
        &self,
        pid: i32,
        oom_score_adj: i32,
    ) -> Result<(), ananicy_core::worker::PlatformError> {
        priority::set_oom_score_adjust(pid, oom_score_adj)
    }

    fn add_pid_to_cgroup(
        &self,
        pid: i32,
        cgroup: &str,
    ) -> Result<(), ananicy_core::worker::PlatformError> {
        cgroups::add_pid_to_cgroup(pid, cgroup)
    }

    fn set_affinity(
        &self,
        pid: i32,
        cpuset: &CpuSet,
    ) -> Result<(), ananicy_core::worker::PlatformError> {
        if let Err(e) = abi::affinity::set_affinity(pid, cpuset) {
            tracing::error!("set_affinity failed for pid {}: {}", pid, e);
            Err(ananicy_core::worker::PlatformError::Unsupported)
        } else {
            Ok(())
        }
    }

    fn process_cgroup(&self, pid: i32) -> Option<CgroupIdentity> {
        self.cgroup_resolver.resolve(pid).unwrap_or(None)
    }
}

pub fn test_latnice_support() -> bool {
    priority::test_latnice_support()
}
