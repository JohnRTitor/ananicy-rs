//! Cgroup manager behaviour against a simulated cgroup-v2 hierarchy.
//!
//! The manager is constructed with an explicit delegated root, so these tests
//! run against a temporary directory instead of the real `/sys/fs/cgroup`. That
//! is what makes the ownership rules testable: a systemd-managed subtree must be
//! refused structurally while the delegated subtree may be modified.

use {
    ananicy_platform::cgroup::{
        CgroupError, CgroupInfo, CgroupVersion,
        manager::{CgroupController, CgroupManager, resolve_rule_name},
    },
    common::{Failure, FakeController},
    std::{
        fs,
        path::{Path, PathBuf},
        process::id,
    },
    tempfile::TempDir,
};

mod common;

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
        manager.ensure_child(foreign_target).is_err(),
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
        manager.set_cpu_max(&child_path, 50).is_ok(),
        "Should successfully set cpu.max in Owned subtree"
    );
    let cores = std::thread::available_parallelism()
        .map(|n| n.get() as u64)
        .unwrap_or(1);
    let expected_quota = 100_000u64.saturating_mul(cores).saturating_mul(50) / 100;
    let content = fs::read_to_string(child_path.join("cpu.max")).unwrap();
    assert_eq!(
        content,
        format!("{expected_quota} 100000\n"),
        "both the quota and the period, separated by a space, must be written"
    );

    assert!(
        manager.set_cpu_weight(&child_path, 200).is_ok(),
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
        manager.set_cpu_max(&foreign, 50).is_err(),
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
        manager.set_cpu_weight(&foreign, 300).is_ok(),
        "an existing foreign cpu.weight may be written"
    );
    assert_eq!(
        fs::read_to_string(foreign.join("cpu.weight")).unwrap(),
        "300\n"
    );

    // Without the controller file the write is an expected, reported skip.
    let without_controller = info.mount_point.join("system.slice");
    assert!(manager.set_cpu_weight(&without_controller, 300).is_err());
    assert!(!without_controller.join("cpu.weight").exists());
}

#[test]
fn a_process_is_not_moved_into_a_foreign_cgroup() {
    let (_temp, info, ananicy_service) = delegated_v2_hierarchy();
    let manager = manager(ananicy_service, info.mount_point.clone());

    let foreign = info.mount_point.join("user.slice");
    fs::write(foreign.join("cgroup.procs"), "").unwrap();

    assert!(
        manager.move_pid(id() as i32, &foreign).is_err(),
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
    assert!(manager.move_pid(id() as i32, &missing).is_err());
}

#[test]
fn a_dead_process_is_not_moved_anywhere() {
    let (_temp, info, ananicy_service) = delegated_v2_hierarchy();
    let manager = manager(ananicy_service.clone(), info.mount_point);

    let target = manager.ensure_child("recycle.scope").unwrap();
    fs::File::create(target.join("cgroup.procs")).unwrap();

    assert!(
        manager.move_pid(i32::MAX, &target).is_err(),
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
        manager.ensure_child("child").is_err(),
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
fn a_successful_move_writes_the_pid_and_newline_in_one_piece() {
    let (_temp, info, ananicy_service) = delegated_v2_hierarchy();
    let manager = manager(ananicy_service, info.mount_point);
    let target = manager.ensure_child("move-test").unwrap();
    fs::File::create(target.join("cgroup.procs")).unwrap();

    let pid = id() as i32;
    assert!(
        manager.move_pid(pid, &target).is_ok(),
        "moving into an owned cgroup must succeed"
    );
    assert_eq!(
        fs::read_to_string(target.join("cgroup.procs")).unwrap(),
        format!("{pid}\n"),
        "the kernel accepts exactly one write of \"pid\\n\"; anything else is EINVAL"
    );
}

#[test]
fn all_v1_control_files_receive_their_exact_values() {
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
    let child = manager.ensure_child("ananicy-test").unwrap();
    for file in ["cpu.cfs_period_us", "cpu.cfs_quota_us", "cpu.shares"] {
        fs::File::create(child.join(file)).unwrap();
    }

    assert!(manager.set_cpu_max(&child, 50).is_ok());
    assert!(manager.set_cpu_weight(&child, 200).is_ok());

    let cores = std::thread::available_parallelism()
        .map(|n| n.get() as u64)
        .unwrap_or(1);
    let expected_quota = 1_000_000u64.saturating_mul(cores).saturating_mul(50) / 100;
    assert_eq!(
        fs::read_to_string(child.join("cpu.cfs_period_us")).unwrap(),
        "1000000\n"
    );
    assert_eq!(
        fs::read_to_string(child.join("cpu.cfs_quota_us")).unwrap(),
        format!("{expected_quota}\n")
    );
    assert_eq!(
        fs::read_to_string(child.join("cpu.shares")).unwrap(),
        "2048\n"
    );
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
    assert!(manager.ensure_child("anything").is_err());
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

#[test]
fn typed_errors_reach_the_caller() {
    let fake = FakeController::new();

    // A structural write into systemd's subtree is Foreign, not a generic I/O
    // failure or a missing file.
    assert!(matches!(
        fake.ensure_child("/user.slice/other.service"),
        Err(CgroupError::Foreign)
    ));

    // A target that does not exist is NotFound.
    let missing = fake.hierarchy().delegated_root.join("absent.scope");
    assert!(matches!(
        fake.move_pid(id() as i32, &missing),
        Err(CgroupError::NotFound)
    ));

    // A raw EBUSY injected by the harness is converted exactly as the real
    // manager converts it, so the caller sees Busy rather than Io.
    let busy = FakeController::new().failing("set_cpu_weight", Failure::Errno(libc::EBUSY));
    let target = busy.hierarchy().owned_cgroup("busy.scope", &["cpu.weight"]);
    assert!(matches!(
        busy.set_cpu_weight(&target, 200),
        Err(CgroupError::Busy)
    ));

    // Other injected kinds stay distinct too.
    let gone = FakeController::new().failing("move_pid", Failure::Gone);
    let target = gone.hierarchy().owned_cgroup("gone.scope", &[]);
    assert!(matches!(
        gone.move_pid(id() as i32, &target),
        Err(CgroupError::Gone)
    ));
}

#[test]
fn the_recording_controller_keeps_every_control_line() {
    let fake = FakeController::new();
    let target = fake
        .hierarchy()
        .owned_cgroup("recorded.scope", &["cpu.max", "cpu.weight"]);
    assert!(fake.set_cpu_max(&target, 25).is_ok());
    assert!(fake.set_cpu_weight(&target, 300).is_ok());
    assert_eq!(fake.lines_for("cpu.max"), ["25 100000\n"]);
    assert_eq!(fake.lines_for("cpu.weight"), ["300\n"]);
    assert_eq!(fake.writes().len(), 2);
}

#[test]
fn rule_names_are_validated_component_wise_without_breaking_nested_names() {
    let (_temp, info, ananicy_service) = delegated_v2_hierarchy();
    let manager = manager(ananicy_service.clone(), info.mount_point.clone());

    for name in [
        "../escape",
        "a/../../b",
        "..",
        "legit/../..",
        "a//b",
        "trailing/",
        "a/./b",
        "bad\u{0}name",
        "bad\u{1}name",
    ] {
        assert!(
            manager.resolve_target_dir(name).is_none(),
            "{name:?} must not resolve to a path"
        );
        assert!(
            matches!(resolve_rule_name(name), Err(CgroupError::InvalidName(_))),
            "{name:?} must report why it was rejected"
        );
    }

    // Nested systemd-style names, absolute-from-root names and dots inside a
    // component were all valid before and must stay valid.
    assert_eq!(
        resolve_rule_name("my-app.slice/my-app.scope").unwrap(),
        "my-app.slice/my-app.scope"
    );
    assert_eq!(
        resolve_rule_name("/my-app.slice/my-app.scope").unwrap(),
        "my-app.slice/my-app.scope"
    );
    assert_eq!(
        manager.resolve_target_dir("my-app.slice/my-app.scope"),
        Some(ananicy_service.join("my-app.slice/my-app.scope"))
    );
    assert_eq!(
        manager.resolve_target_dir("app..name.scope"),
        Some(ananicy_service.join("app..name.scope"))
    );

    // Paths read from /proc are absolute from the mount root, not from the
    // delegated unit; they must not be able to escape it.
    assert_eq!(
        manager
            .resolve_process_path(Path::new("/user.slice/session.scope"))
            .unwrap(),
        info.mount_point.join("user.slice/session.scope")
    );
    assert!(
        manager
            .resolve_process_path(Path::new("../escape"))
            .is_err()
    );
    assert!(
        manager
            .resolve_process_path(Path::new("/../../escape"))
            .is_err()
    );
}
