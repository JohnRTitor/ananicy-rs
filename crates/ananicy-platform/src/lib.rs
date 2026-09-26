#![allow(clippy::io_other_error)]
use {
    ananicy_core::worker::{PlatformError, PlatformError::Unsupported},
    std::time::Duration,
    tracing::debug,
};

pub mod abi;
pub mod cgroup;
pub mod cgroups;
pub mod mounts;
pub mod netlink;
pub mod priority;
pub mod process_info;
pub mod procfs;
pub mod service;
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
                Duration::from_secs(1),
            ),
        }
    }
}

impl Default for LinuxPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformActions for LinuxPlatform {
    fn is_realtime(&self, pid: i32) -> bool {
        // A task counts as realtime when it holds a static priority above zero,
        // which is what the policy and the priority together mean. Asking for
        // the policy instead (`sched_getscheduler`) would call a priority-zero
        // FIFO or RR task realtime, and miss a task that reports a priority
        // under any other policy; the reference asks for the priority, so the
        // two agree on the same machine.
        let size = std::mem::size_of::<crate::abi::sched_attr::sched_attr>() as u32;
        let mut attr = crate::abi::sched_attr::sched_attr {
            size,
            ..Default::default()
        };

        match crate::abi::sched_attr::sched_getattr(pid, &mut attr, size, 0) {
            // `sched_getattr` can report a partial struct on a kernel that grew
            // it, so a successful call may still leave the priority at zero —
            // which is the answer, not a failure to read it.
            Ok(()) => attr.sched_priority > 0,
            Err(_) => false,
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
        abi::affinity::get_max_number_of_cpus()
    }

    fn get_tids(&self, pid: i32) -> Result<Vec<i32>, PlatformError> {
        crate::procfs::get_tids(pid)
    }

    fn set_priority(&self, pid: i32, tids: &[i32], nice: i32) -> Result<(), PlatformError> {
        priority::set_priority(pid, tids, nice)
    }

    fn set_latency_nice(&self, pid: i32, tids: &[i32], lat_nice: i32) -> Result<(), PlatformError> {
        priority::set_latency_nice(pid, tids, lat_nice)
    }

    fn set_sched(&self, pid: i32, sched: &str, rtprio: u32) -> Result<(), PlatformError> {
        priority::set_sched(pid, sched, rtprio)
    }

    fn set_io_priority(&self, pid: i32, ioclass: &str, ionice: i32) -> Result<(), PlatformError> {
        priority::set_io_priority(pid, ioclass, ionice)
    }

    fn set_oom_score_adj(&self, pid: i32, oom_score_adj: i32) -> Result<(), PlatformError> {
        priority::set_oom_score_adjust(pid, oom_score_adj)
    }

    fn add_pid_to_cgroup(&self, pid: i32, cgroup: &str) -> Result<(), PlatformError> {
        cgroups::add_pid_to_cgroup(pid, cgroup)
    }

    fn set_cpu_weight(&self, pid: i32, weight: u32) -> Result<(), PlatformError> {
        let identity = self.process_cgroup(pid).ok_or(PlatformError::NotFound)?;
        let path_str = identity
            .path
            .as_path()
            .to_str()
            .ok_or(PlatformError::NotFound)?;
        cgroups::set_cpu_weight_for_cgroup(path_str, weight)
    }

    fn set_affinity(&self, pid: i32, tids: &[i32], cpuset: &CpuSet) -> Result<(), PlatformError> {
        if let Err(e) = abi::affinity::set_affinity(pid, tids, cpuset) {
            debug!("set_affinity failed for pid {}: {}", pid, e);
            Err(Unsupported)
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
