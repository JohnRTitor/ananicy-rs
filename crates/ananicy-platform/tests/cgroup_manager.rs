//! Cgroup manager behaviour against a simulated cgroup-v2 hierarchy.
//!
//! The manager is constructed with an explicit delegated root, so these tests
//! run against a temporary directory instead of the real `/sys/fs/cgroup`. That
//! is what makes the ownership rules testable: a systemd-managed subtree must be
//! refused structurally while the delegated subtree may be modified.

use {
    ananicy_platform::cgroup::{
        CgroupInfo, CgroupVersion,
        manager::{CgroupController, CgroupManager},
    },
    std::{fs, path::PathBuf, process::id},
    tempfile::TempDir,
};

/// Builds the `root -> system.slice -> ananicy.service` layout that a packaged
/// systemd unit with `Delegate=yes` produces, and returns the delegated root.
fn delegated_v2_hierarchy() -> (TempDir, CgroupInfo, PathBuf) {
    let temp_dir = tempfile::tempdir().unwrap();
    let mount_point = temp_dir.path().to_path_buf();

    let system_slice = mount_point.join("system.slice");
    let ananicy_service = system_slice.join("ananicy.service");
    fs::create_dir_all(&ananicy_service).unwrap();

    fs::write(
        ananicy_service.join("cgroup.controllers"),
        "cpu memory pids",
    )
    .unwrap();
    // Simulate our process being in this cgroup.
    fs::write(ananicy_service.join("cgroup.procs"), id().to_string()).unwrap();

    // A sibling subtree owned by systemd, i.e. foreign to us.
    fs::create_dir_all(mount_point.join("user.slice")).unwrap();

    let info = CgroupInfo {
        version: CgroupVersion::V2,
        mount_point,
    };

    (temp_dir, info, ananicy_service)
}

fn manager(delegated_root: PathBuf, mount_point: PathBuf) -> CgroupManager {
    CgroupManager::new_with_root(
        CgroupInfo {
            version: CgroupVersion::V2,
            mount_point,
        },
        Some(delegated_root),
    )
}

#[test]
fn test_cgroup_manager_v2_owned_subtree() {
    let (_temp, info, ananicy_service) = delegated_v2_hierarchy();
    let manager = manager(ananicy_service.clone(), info.mount_point);

    // In ananicy rules, cgroup targets starting without / are relative to the
    // delegated root.
    let target = "my-app.slice/my-app.scope";

    // Creating a child is allowed because it is under our owned subtree.
    let child_path = manager
        .ensure_child(target)
        .expect("creating a child in the owned subtree must succeed");

    assert!(child_path.starts_with(&ananicy_service));
    assert!(child_path.ends_with(target));
    assert!(child_path.exists(), "Directory should have been created");
}

#[test]
fn test_cgroup_manager_v2_foreign_rejection() {
    let (_temp, info, ananicy_service) = delegated_v2_hierarchy();
    let mount_point = info.mount_point.clone();
    let manager = manager(ananicy_service, mount_point.clone());

    // A cgroup target starting with / means an absolute path from the cgroup
    // root, i.e. outside the delegated subtree.
    let foreign_target = "/user.slice/user-1000.slice";

    assert!(
        manager.ensure_child(foreign_target).is_none(),
        "Should reject structural modifications in Foreign subtree"
    );
    assert!(
        !mount_point.join("user.slice/user-1000.slice").exists(),
        "the foreign directory must not have been created"
    );
}

#[test]
fn test_cgroup_manager_v2_write_files() {
    let (_temp, info, ananicy_service) = delegated_v2_hierarchy();
    let manager = manager(ananicy_service, info.mount_point);
    let target = "test-limits.scope";
    let child_path = manager.ensure_child(target).unwrap();

    // Create dummy cpu.max and cpu.weight files so we can test writing to them
    fs::File::create(child_path.join("cpu.max")).unwrap();
    fs::File::create(child_path.join("cpu.weight")).unwrap();

    assert!(
        manager.set_cpu_max(&child_path, 50),
        "Should successfully set cpu.max in Owned subtree"
    );
    let content = fs::read_to_string(child_path.join("cpu.max")).unwrap();
    assert!(content.ends_with("100000\n"), "the period must be 100000");

    assert!(
        manager.set_cpu_weight(&child_path, 200),
        "Should successfully set cpu.weight in Owned subtree"
    );
    assert_eq!(
        fs::read_to_string(child_path.join("cpu.weight")).unwrap(),
        "200\n"
    );
}

#[test]
fn a_foreign_cgroup_never_receives_a_cpu_quota() {
    // Unlike optional resource tuning, creating a `cpu.max` file in a foreign
    // subtree would change how systemd accounts the unit, so it must be refused.
    let (_temp, info, _ananicy_service) = delegated_v2_hierarchy();
    let manager = manager(
        info.mount_point.join("system.slice/ananicy.service"),
        info.mount_point.clone(),
    );

    let foreign = info.mount_point.join("user.slice");
    fs::File::create(foreign.join("cpu.max")).unwrap();

    assert!(
        !manager.set_cpu_max(&foreign, 50),
        "writing cpu.max into a foreign subtree must be refused"
    );
    assert_eq!(
        fs::read_to_string(foreign.join("cpu.max")).unwrap(),
        "",
        "the foreign cpu.max must be left untouched"
    );
}

#[test]
fn a_foreign_cgroup_may_still_be_weight_tuned() {
    // `cpu.weight` on an already-existing controller file is the documented
    // exception to the ownership rule: ananicy-rs may tune a foreign cgroup but
    // never create one.
    let (_temp, info, _ananicy_service) = delegated_v2_hierarchy();
    let manager = manager(
        info.mount_point.join("system.slice/ananicy.service"),
        info.mount_point.clone(),
    );

    let foreign = info.mount_point.join("user.slice");
    fs::write(foreign.join("cpu.weight"), "").unwrap();
    assert!(
        manager.set_cpu_weight(&foreign, 300),
        "an existing foreign cpu.weight may be written"
    );
    assert_eq!(
        fs::read_to_string(foreign.join("cpu.weight")).unwrap(),
        "300\n"
    );

    // Without the controller file the write is an expected, reported skip.
    let without_controller = info.mount_point.join("system.slice");
    assert!(!manager.set_cpu_weight(&without_controller, 300));
    assert!(!without_controller.join("cpu.weight").exists());
}

#[test]
fn a_process_is_not_moved_into_a_foreign_cgroup() {
    let (_temp, info, ananicy_service) = delegated_v2_hierarchy();
    let manager = manager(ananicy_service, info.mount_point.clone());

    let foreign = info.mount_point.join("user.slice");
    fs::write(foreign.join("cgroup.procs"), "").unwrap();

    assert!(
        !manager.move_pid(id() as i32, &foreign),
        "moving a process into a foreign cgroup must be refused"
    );
    assert_eq!(
        fs::read_to_string(foreign.join("cgroup.procs")).unwrap(),
        "",
        "the foreign cgroup.procs must be left untouched"
    );
}

#[test]
fn a_process_is_not_moved_into_a_cgroup_that_does_not_exist() {
    let (_temp, info, ananicy_service) = delegated_v2_hierarchy();
    let manager = manager(ananicy_service, info.mount_point.clone());

    let missing = info
        .mount_point
        .join("system.slice/ananicy.service/absent.scope");
    assert!(!manager.move_pid(id() as i32, &missing));
}

#[test]
fn a_dead_process_is_not_moved_anywhere() {
    let (_temp, info, ananicy_service) = delegated_v2_hierarchy();
    let manager = manager(ananicy_service.clone(), info.mount_point);

    let target = manager.ensure_child("recycle.scope").unwrap();
    fs::File::create(target.join("cgroup.procs")).unwrap();

    assert!(
        !manager.move_pid(i32::MAX, &target),
        "a process that no longer exists must not be written to cgroup.procs"
    );
}

#[test]
fn a_cgroup_name_containing_traversal_is_rejected() {
    let (_temp, info, ananicy_service) = delegated_v2_hierarchy();
    let manager = manager(ananicy_service, info.mount_point.clone());

    for name in ["../escape", "a/../../b", "..", "legit/../.."] {
        assert!(
            manager.resolve_target_dir(name).is_none(),
            "{name:?} must not resolve to a path"
        );
    }
    assert!(manager.cgroup_exists("..").eq(&false));
}

#[test]
fn a_name_is_resolved_relative_to_the_delegated_root() {
    let (_temp, info, ananicy_service) = delegated_v2_hierarchy();
    let manager = manager(ananicy_service.clone(), info.mount_point.clone());

    assert_eq!(
        manager.resolve_target_dir("child.scope"),
        Some(ananicy_service.join("child.scope"))
    );
    assert_eq!(
        manager.resolve_target_dir("/absolute.scope"),
        Some(info.mount_point.join("absolute.scope"))
    );
    assert_eq!(
        manager.resolve_target_dir("/"),
        Some(info.mount_point.clone()),
        "the hierarchy root itself is addressable"
    );
    assert_eq!(manager.resolve_target_dir(""), Some(ananicy_service));
}

#[test]
fn without_a_delegated_root_relative_names_stay_at_the_mount_point() {
    // A daemon started outside a delegated service must not invent a hierarchy:
    // relative names resolve to the mount point, where the ownership check then
    // refuses any structural change.
    let temp = tempfile::tempdir().unwrap();
    let mount_point = temp.path().to_path_buf();
    let manager = CgroupManager::new_with_root(
        CgroupInfo {
            version: CgroupVersion::V2,
            mount_point: mount_point.clone(),
        },
        None,
    );

    assert_eq!(
        manager.resolve_target_dir("child"),
        Some(mount_point.join("child"))
    );
    assert!(
        manager.ensure_child("child").is_none(),
        "nothing may be created"
    );
    assert!(!mount_point.join("child").exists());
}

#[test]
fn a_v1_manager_creates_directories_without_delegation() {
    // cgroup v1 has no single-writer rule, so the legacy path simply creates the
    // directory. It is exercised here against a temporary directory.
    let temp = tempfile::tempdir().unwrap();
    let mount_point = temp.path().to_path_buf();
    fs::create_dir(mount_point.join("cpu")).unwrap();

    let manager = CgroupManager::new_with_root(
        CgroupInfo {
            version: CgroupVersion::V1,
            mount_point: mount_point.clone(),
        },
        None,
    );

    let created = manager
        .ensure_child("ananicy-test")
        .expect("v1 always creates the directory");
    assert_eq!(created, mount_point.join("cpu/ananicy-test"));
    assert!(created.exists());
    assert!(manager.cgroup_exists("ananicy-test"));
}

#[test]
fn a_v1_manager_reports_no_target_without_a_hierarchy() {
    let manager = CgroupManager::new_with_root(
        CgroupInfo {
            version: CgroupVersion::None,
            mount_point: PathBuf::new(),
        },
        None,
    );

    assert_eq!(manager.resolve_target_dir("anything"), None);
    assert!(!manager.cgroup_exists("anything"));
    assert!(manager.ensure_child("anything").is_none());
}

#[test]
fn subtree_control_is_enabled_before_a_child_is_created() {
    // The cgroup v2 hierarchy rules require the parent's controllers to be
    // enabled before a child uses them.
    let (_temp, info, ananicy_service) = delegated_v2_hierarchy();
    let manager = manager(ananicy_service.clone(), info.mount_point);

    let parent = manager.ensure_child("parent.scope").unwrap();
    fs::File::create(parent.join("cgroup.subtree_control")).unwrap();

    let child = manager.ensure_child("parent.scope/child.scope").unwrap();
    assert!(child.exists());
    assert!(child.starts_with(&parent));
}
