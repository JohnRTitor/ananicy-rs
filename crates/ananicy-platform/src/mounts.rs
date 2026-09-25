use {
    std::{thread::sleep, time::Duration},
    tracing::trace,
};

use std::{fs, path::PathBuf, sync::RwLock};

pub use crate::cgroup::{CgroupInfo, CgroupVersion};

static CGROUP_INFO: RwLock<Option<CgroupInfo>> = RwLock::new(None);

/// How many times [`init_cgroups`] re-detects before giving up, and how long it
/// waits in between. Together they bound the startup delay at ten seconds.
pub const CGROUP_INIT_ATTEMPTS: usize = 20;
pub const CGROUP_INIT_INTERVAL: Duration = Duration::from_millis(500);

/// The total time [`init_cgroups`] can spend waiting, for reporting.
pub const CGROUP_INIT_TIMEOUT: Duration =
    Duration::from_millis(CGROUP_INIT_ATTEMPTS as u64 * CGROUP_INIT_INTERVAL.as_millis() as u64);

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

pub fn parse_cgroups_from_str(content: &str, info: &mut CgroupInfo) {
    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }

        let fs_type = parts[2];
        let mount_point = PathBuf::from(parts[1]);

        if fs_type == "cgroup2" {
            let test_cgroup = mount_point.join("ananicy_test_cgroup2");

            // Clean up left-over test cgroup if it exists
            if test_cgroup.exists() {
                let _ = fs::remove_dir(&test_cgroup);
            }

            if fs::create_dir(&test_cgroup).is_ok() {
                let controllers_path = test_cgroup.join("cgroup.controllers");
                let mut has_cpu_controller = false;

                if let Ok(controllers) = fs::read_to_string(&controllers_path)
                    && controllers.split_whitespace().any(|c| c == "cpu")
                {
                    trace!(
                        "get_cgroup_version: Found cpu controller in {}",
                        controllers_path.display()
                    );
                    has_cpu_controller = true;
                }

                let has_cpu_max = test_cgroup.join("cpu.max").exists();

                let _ = fs::remove_dir(&test_cgroup);

                if has_cpu_controller && has_cpu_max {
                    trace!("Found cgroup v2 at {}", mount_point.display());
                    info.version = CgroupVersion::V2;
                    info.mount_point = mount_point.clone();
                    break;
                }
            }
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

    let mut info = CgroupInfo {
        version: CgroupVersion::None,
        mount_point: PathBuf::new(),
    };

    // `/proc/self/mounts` is a pseudo-file; do not rely on metadata/file size before reading it.
    if let Ok(content) = fs::read_to_string("/proc/self/mounts") {
        parse_cgroups_from_str(&content, &mut info);
    }

    if let Ok(mut lock) = CGROUP_INFO.write() {
        *lock = Some(info.clone());
    }

    info
}
