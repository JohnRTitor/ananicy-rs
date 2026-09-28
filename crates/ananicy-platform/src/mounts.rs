use {
    std::{thread::sleep, time::Duration},
    tracing::trace,
};

use std::{
    fs,
    path::{Path, PathBuf},
    sync::RwLock,
};

pub use crate::cgroup::{CgroupInfo, CgroupVersion};

static CGROUP_INFO: RwLock<Option<CgroupInfo>> = RwLock::new(None);

/// How many times [`init_cgroups`] re-detects before giving up, and how long it
/// waits in between. Together they bound the startup delay at ten seconds.
pub const CGROUP_INIT_ATTEMPTS: usize = 20;
pub const CGROUP_INIT_INTERVAL: Duration = Duration::from_millis(500);

/// The total time [`init_cgroups`] can spend waiting, for reporting.
pub const CGROUP_INIT_TIMEOUT: Duration =
    Duration::from_millis(CGROUP_INIT_ATTEMPTS as u64 * CGROUP_INIT_INTERVAL.as_millis() as u64);

/// The hierarchy version, without the mount point.
///
/// [`get_cgroup_info`] returns `CgroupInfo` by value, which is right for a
/// caller that wants the mount point. A caller that only wants the version was
/// cloning a `PathBuf` — a heap allocation and a copy — to read a one-byte
/// enum, and `is_cgroup_v2` is asked once per process that matches a rule with
/// a `nice` on it, so that was one allocation per process for a discriminant.
///
/// Falls back to [`get_cgroup_info`] when nothing is cached, which keeps the
/// lazy detection and its single-publisher rule exactly as they were.
pub fn cgroup_version() -> CgroupVersion {
    if let Ok(info) = CGROUP_INFO.read()
        && let Some(i) = &*info
    {
        return i.version;
    }
    get_cgroup_info().version
}

pub fn reset_cgroup_info() {
    if let Ok(mut info) = CGROUP_INFO.write() {
        *info = None;
    }
}

/// Re-detects the hierarchy until one is found, or the attempts run out.
///
/// Returns whether a hierarchy is available.
pub fn init_cgroups() -> bool {
    for _ in 0..CGROUP_INIT_ATTEMPTS {
        if get_cgroup_info().version != CgroupVersion::None {
            return true;
        }
        sleep(CGROUP_INIT_INTERVAL);
    }
    reset_cgroup_info();
    false
}

/// Whether a `cgroup2` mount point is a hierarchy this daemon can actually apply
/// `cpu` limits in.
///
/// `cgroup.controllers` on the *root* of a v2 hierarchy lists the controllers
/// that may be enabled for its children, which is exactly the question a
/// `CPUQuota`/`CPUWeight` rule ends up asking. The reference daemon (and this one
/// until now) instead created a throwaway cgroup in the mount point and read
/// `cgroup.controllers` from *that*.
///
/// The two answers are the same, and the read is strictly better:
///
/// * a `mkdir` in the cgroup root needs write access to it, which a read-only
///   cgroupfs mount does not give anybody — inside most container runtimes'
///   default cgroup namespace, and on any `ro` sysfs, so the probe failed and
///   the whole hierarchy was reported as unavailable even though it was fully
///   functional;
/// * the probe created and removed a directory in the global cgroup root on
///   every re-detection, which is visible to `cgroup.events`, to inotify
///   watchers, and to systemd's own accounting;
/// * two threads detecting at once raced on the same fixed name
///   (`ananicy_test_cgroup2`), so one got `EEXIST`, decided the hierarchy was
///   unusable, and could overwrite the other's correct answer.
fn v2_has_cpu_controller(mount_point: &Path) -> bool {
    let Ok(controllers) = fs::read_to_string(mount_point.join("cgroup.controllers")) else {
        return false;
    };
    let has_cpu = controllers.split_whitespace().any(|c| c == "cpu");
    trace!(
        "get_cgroup_version: {} controllers = {:?}",
        mount_point.display(),
        controllers
    );
    // `cpu.max` is what a `.cgroups` rule writes. A root whose `cpu` controller
    // exists but whose `cpu.max` does not is a kernel or configuration this
    // daemon cannot do anything useful with, so it is not a match.
    has_cpu && mount_point.join("cgroup.procs").exists()
}

/// The `mkdir`-based probe, kept only for a hierarchy whose `cgroup.controllers`
/// could not be read at all. It is the last resort, not the first: see
/// [`v2_has_cpu_controller`].
fn v2_probe_by_creating_a_cgroup(mount_point: &Path) -> bool {
    let test_cgroup = mount_point.join("ananicy_test_cgroup2");

    // Clean up left-over test cgroup if it exists
    if test_cgroup.exists() {
        let _ = fs::remove_dir(&test_cgroup);
    }

    if fs::create_dir(&test_cgroup).is_err() {
        trace!(
            "get_cgroup_version: cannot create a probe cgroup at {}",
            mount_point.display()
        );
        return false;
    }

    let controllers_path = test_cgroup.join("cgroup.controllers");
    let mut has_cpu_controller = false;

    if let Ok(controllers) = fs::read_to_string(&controllers_path)
        && controllers.split_whitespace().any(|c| c == "cpu")
    {
        has_cpu_controller = true;
    }

    let has_cpu_max = test_cgroup.join("cpu.max").exists();

    let _ = fs::remove_dir(&test_cgroup);

    has_cpu_controller && has_cpu_max
}

pub fn parse_cgroups_from_str(content: &str, info: &mut CgroupInfo) {
    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }

        let fs_type = parts[2];
        let mount_point = PathBuf::from(parts[1]);

        if fs_type == "cgroup2" {
            if !v2_has_cpu_controller(&mount_point) && !v2_probe_by_creating_a_cgroup(&mount_point)
            {
                continue;
            }
            trace!("Found cgroup v2 at {}", mount_point.display());
            info.version = CgroupVersion::V2;
            info.mount_point = mount_point;
            break;
        } else if fs_type == "cgroup"
            && info.version == CgroupVersion::None
            && let Some(parent) = mount_point.parent()
            && parent.join("cpu").exists()
        {
            trace!("Found cgroup v1 at {}", parent.display());
            info.version = CgroupVersion::V1;
            info.mount_point = parent.to_path_buf();
        }
    }
}

pub fn get_cgroup_info() -> CgroupInfo {
    if let Ok(info) = CGROUP_INFO.read()
        && let Some(i) = &*info
    {
        return i.clone();
    }

    // A cached answer is not published here: the detection below reads the mount
    // table and, in the fallback case, creates a directory in the cgroup root,
    // and two threads doing that at once used to overwrite each other's result
    // with whichever finished last — including overwriting a correct
    // "cgroup v2" with "none". A repeat costs one read of `/proc/self/mounts`,
    // which is a few kilobytes and happens at most once per `reset_cgroup_info`.
    let mut info = CgroupInfo {
        version: CgroupVersion::None,
        mount_point: PathBuf::new(),
    };

    // `/proc/self/mounts` is a pseudo-file; do not rely on metadata/file size before reading it.
    if let Ok(content) = fs::read_to_string("/proc/self/mounts") {
        parse_cgroups_from_str(&content, &mut info);
    }

    // Publish only if nothing else has in the meantime.
    if let Ok(mut lock) = CGROUP_INFO.write()
        && lock.is_none()
    {
        *lock = Some(info.clone());
    }

    info
}
