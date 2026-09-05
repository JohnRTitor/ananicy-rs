use {
    std::{fs, io::Write, path::PathBuf},
    tempfile::TempDir,
};

use ananicy_platform::cgroup::{
    CgroupInfo, CgroupVersion,
    manager::{CgroupController, CgroupManager},
};

fn setup_mock_v2_delegated_root() -> (TempDir, CgroupInfo, PathBuf) {
    let temp_dir = tempfile::tempdir().unwrap();
    let mount_point = temp_dir.path().to_path_buf();

    // Simulate systemd delegated hierarchy
    // Root -> system.slice -> ananicy.service (delegated)
    let system_slice = mount_point.join("system.slice");
    let ananicy_service = system_slice.join("ananicy.service");

    fs::create_dir_all(&ananicy_service).unwrap();

    // Create cgroup.controllers and cgroup.subtree_control
    let mut file = fs::File::create(ananicy_service.join("cgroup.controllers")).unwrap();
    file.write_all(b"cpu memory pids").unwrap();

    let mut file = fs::File::create(ananicy_service.join("cgroup.procs")).unwrap();
    // Simulate our process being in this cgroup
    let pid = std::process::id();
    file.write_all(pid.to_string().as_bytes()).unwrap();

    let info = CgroupInfo {
        version: CgroupVersion::V2,
        mount_point,
    };

    (temp_dir, info, ananicy_service)
}

#[test]
fn test_cgroup_manager_v2_owned_subtree() {
    let (_temp, info, ananicy_service) = setup_mock_v2_delegated_root();

    let manager = CgroupManager::new_with_root(info, Some(ananicy_service.clone()));

    // Request a child cgroup under our delegated root
    // In ananicy rules, cgroup targets starting without / are relative to the delegated root
    let target = "my-app.slice/my-app.scope";

    // Ensure child should succeed because it's under our Owned subtree
    let child_path = manager.ensure_child(target);
    assert!(
        child_path.is_some(),
        "Should allow creating child in Owned subtree"
    );

    let child_path = child_path.unwrap();
    assert!(child_path.starts_with(&ananicy_service));
    assert!(child_path.ends_with(target));
    assert!(child_path.exists(), "Directory should have been created");
}

#[test]
fn test_cgroup_manager_v2_foreign_rejection() {
    let (_temp, info, ananicy_service) = setup_mock_v2_delegated_root();

    let manager = CgroupManager::new_with_root(info, Some(ananicy_service.clone()));

    // Request a cgroup starting with / which means absolute path from cgroup root
    let foreign_target = "/user.slice/user-1000.slice";

    // Ensure child should fail and return None because it's Foreign
    let child_path = manager.ensure_child(foreign_target);
    assert!(
        child_path.is_none(),
        "Should reject structural modifications in Foreign subtree"
    );
}

#[test]
fn test_cgroup_manager_v2_write_files() {
    let (_temp, info, ananicy_service) = setup_mock_v2_delegated_root();

    let manager = CgroupManager::new_with_root(info, Some(ananicy_service.clone()));
    let target = "test-limits.scope";
    let child_path = manager.ensure_child(target).unwrap();

    // Create dummy cpu.max and cpu.weight files so we can test writing to them
    fs::File::create(child_path.join("cpu.max")).unwrap();
    fs::File::create(child_path.join("cpu.weight")).unwrap();

    let success = manager.set_cpu_max(&child_path, 50);
    assert!(success, "Should successfully set cpu.max in Owned subtree");

    let content = fs::read_to_string(child_path.join("cpu.max")).unwrap();
    assert!(content.ends_with("1000000\n"));

    let success = manager.set_cpu_weight(&child_path, 200);
    assert!(
        success,
        "Should successfully set cpu.weight in Owned subtree"
    );

    let content = fs::read_to_string(child_path.join("cpu.weight")).unwrap();
    assert_eq!(content, "200\n");
}
