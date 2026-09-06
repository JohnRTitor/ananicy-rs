use std::{fs, path::PathBuf, sync::RwLock};

pub use crate::cgroup::{CgroupInfo, CgroupVersion};

static CGROUP_INFO: RwLock<Option<CgroupInfo>> = RwLock::new(None);

pub fn reset_cgroup_info() {
    if let Ok(mut info) = CGROUP_INFO.write() {
        *info = None;
    }
}

pub fn init_cgroups() -> bool {
    for _ in 0..20 {
        reset_cgroup_info();
        let info = get_cgroup_info();
        if info.version != CgroupVersion::None {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
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
                    tracing::trace!(
                        "get_cgroup_version: Found cpu controller in {}",
                        controllers_path.display()
                    );
                    has_cpu_controller = true;
                }

                let has_cpu_max = test_cgroup.join("cpu.max").exists();

                let _ = fs::remove_dir(&test_cgroup);

                if has_cpu_controller && has_cpu_max {
                    tracing::trace!("Found cgroup v2 at {}", mount_point.display());
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
            tracing::trace!("Found cgroup v1 at {}", parent.display());
            info.version = CgroupVersion::V1;
            info.mount_point = parent.to_path_buf();
        }
    }
}

pub fn get_cgroup_info() -> CgroupInfo {
    if let Ok(info) = CGROUP_INFO.read() {
        if let Some(i) = &*info {
            return i.clone();
        }
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
