use {
    ananicy_platform::{
        cgroups::{add_pid_to_cgroup, create_cgroup},
        mounts::{CgroupVersion, get_cgroup_info},
    },
    std::{fs, path::PathBuf},
};

fn get_cgroup_for_pid(pid: i32) -> Option<String> {
    if let Ok(content) = fs::read_to_string(format!("/proc/{}/cgroup", pid)) {
        let info = get_cgroup_info();
        for line in content.lines() {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() < 3 {
                continue;
            }
            if info.version == CgroupVersion::V2 && parts[1].is_empty() {
                return Some(parts[2].to_string());
            }
            if info.version == CgroupVersion::V1 && parts[1].split(',').any(|c| c == "cpu") {
                return Some(parts[2].to_string());
            }
        }
    }
    None
}

fn get_cgroup_path(cgroup_name: &str) -> Option<PathBuf> {
    let info = get_cgroup_info();
    if info.version == CgroupVersion::None {
        return None;
    }

    let base = if info.version == CgroupVersion::V1 {
        info.mount_point.join("cpu")
    } else {
        info.mount_point.clone()
    };

    let relative = cgroup_name.strip_prefix('/').unwrap_or(cgroup_name);
    Some(base.join(relative))
}

#[test]
fn test_cgroup_is_valid() {
    let info = get_cgroup_info();
    let is_known = info.version == CgroupVersion::None
        || info.version == CgroupVersion::V1
        || info.version == CgroupVersion::V2;
    assert!(is_known);
}

// These tests require root privileges to create cgroups and write to tasks/cgroup.procs
// So they will be skipped if not root, matching C++ `doctest::skip(amiuser)`
fn is_root() -> bool {
    rustix::process::geteuid().is_root()
}

#[test]
fn test_create_cgroup() {
    if !is_root() {
        println!("Skipping test_create_cgroup because not root");
        return;
    }

    let name = "UNIT_TEST_ANANICY";
    if let Some(path) = get_cgroup_path(name) {
        let _ = fs::remove_dir(&path); // Cleanup before

        assert!(
            create_cgroup(name, None),
            "Should successfully create cgroup"
        );
        assert!(path.exists(), "Cgroup directory should exist");

        let _ = fs::remove_dir(&path);
    }
}

#[test]
fn test_add_pid_to_cgroup() {
    if !is_root() {
        println!("Skipping test_add_pid_to_cgroup because not root");
        return;
    }

    let name = "UNIT_TEST_ANANICY";
    if let Some(path) = get_cgroup_path(name) {
        let _ = fs::remove_dir(&path);

        assert!(create_cgroup(name, None));

        let current_pid = std::process::id() as i32;
        let cgroup = get_cgroup_for_pid(current_pid);
        assert!(cgroup.is_some(), "Current proc should be in a cgroup");

        // Move to root
        assert!(add_pid_to_cgroup(current_pid, "/").is_ok());
        assert_eq!(get_cgroup_for_pid(current_pid).unwrap(), "/");

        // Move to created
        assert!(add_pid_to_cgroup(current_pid, name).is_ok());

        // Depending on V1 or V2, the reported cgroup might have a leading slash
        let current = get_cgroup_for_pid(current_pid).unwrap();
        assert!(current.ends_with(name) || current == format!("/{}", name));

        // Cleanup: move back to root and remove
        let _ = add_pid_to_cgroup(current_pid, "/");
        let _ = fs::remove_dir(&path);
    }
}

#[test]
fn test_add_pid_to_cgroup_with_quota() {
    if !is_root() {
        println!("Skipping test_add_pid_to_cgroup_with_quota because not root");
        return;
    }

    let name = "UNIT_TEST_ANANICY";
    if let Some(path) = get_cgroup_path(name) {
        let _ = fs::remove_dir(&path);

        assert!(create_cgroup(name, Some(90)));

        let _ = fs::remove_dir(&path);
    }
}
