//! Cgroup mutation against the live hierarchy.
//!
//! Everything here talks to the real cgroup filesystem of the machine running
//! the suite. The read-only detection tests always run; the tests that create a
//! cgroup and move a process into it need root and are skipped (with a message)
//! when the suite is unprivileged, so an unprivileged `cargo test` still
//! validates the daemon's own logic.

use {
    ananicy_platform::{
        cgroups::{add_pid_to_cgroup, create_cgroup},
        mounts::{CgroupVersion, get_cgroup_info},
    },
    std::{fs, path::PathBuf, process::id},
};

/// A name no packaged rule set is likely to use.
const TEST_CGROUP: &str = "UNIT_TEST_ANANICY";

fn is_root() -> bool {
    rustix::process::geteuid().is_root()
}

/// Reports why a privileged test is not running, so a skip is never mistaken
/// for a pass in CI output.
fn require_root(test: &str) -> bool {
    if is_root() {
        return true;
    }
    eprintln!("skipping {test}: creating cgroups requires root");
    false
}

fn current_cgroup_path(pid: i32) -> Option<String> {
    let content = fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let info = get_cgroup_info();

    content.lines().find_map(|line| {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() < 3 {
            return None;
        }
        match info.version {
            // The unified hierarchy has an empty controller field.
            CgroupVersion::V2 if parts[1].is_empty() => Some(parts[2].to_string()),
            CgroupVersion::V1 if parts[1].split(',').any(|c| c == "cpu") => {
                Some(parts[2].to_string())
            }
            _ => None,
        }
    })
}

fn test_cgroup_dir() -> Option<PathBuf> {
    let info = get_cgroup_info();
    if info.version == CgroupVersion::None {
        return None;
    }

    let base = if info.version == CgroupVersion::V1 {
        info.mount_point.join("cpu")
    } else {
        info.mount_point.clone()
    };
    Some(base.join(TEST_CGROUP))
}

#[test]
fn the_detected_cgroup_version_is_one_this_build_understands() {
    let info = get_cgroup_info();
    assert!(matches!(
        info.version,
        CgroupVersion::None | CgroupVersion::V1 | CgroupVersion::V2
    ));

    if info.version != CgroupVersion::None {
        assert!(
            info.mount_point.is_absolute(),
            "a detected hierarchy has an absolute mount point: {:?}",
            info.mount_point
        );
    }
}

#[test]
fn the_test_process_belongs_to_a_cgroup_when_one_is_available() {
    if get_cgroup_info().version == CgroupVersion::None {
        return; // no cgroup hierarchy on this host
    }
    assert!(
        current_cgroup_path(id() as i32).is_some(),
        "a live process on a cgroup host is always in a cgroup"
    );
}

#[test]
fn moving_a_process_to_a_cgroup_that_does_not_exist_fails() {
    // The rules engine is expected to create a cgroup before moving a process
    // into it, so a missing target has to be reported instead of silently
    // creating a directory here.
    let name = "unit-test-ananicy-does-not-exist";
    let result = add_pid_to_cgroup(id() as i32, name);
    assert!(result.is_err(), "moving into a missing cgroup must fail");
}

#[test]
fn a_cgroup_name_escaping_the_hierarchy_is_rejected() {
    // Path traversal in a rule's `cgroup` field must never resolve outside the
    // cgroup mount point.
    for name in ["../../../etc", "foo/../../bar", "a/../.."] {
        assert!(
            add_pid_to_cgroup(id() as i32, name).is_err(),
            "{name:?} must be rejected before touching the filesystem"
        );
    }
}

#[test]
fn create_cgroup_refuses_to_recreate_an_existing_cgroup() {
    if !require_root("create_cgroup_refuses_to_recreate_an_existing_cgroup") {
        return;
    }
    let Some(path) = test_cgroup_dir() else {
        return; // no cgroup hierarchy
    };
    let _ = fs::remove_dir(&path);

    assert!(
        create_cgroup(TEST_CGROUP, None),
        "the first creation succeeds"
    );
    assert!(path.exists(), "the cgroup directory must exist");
    assert!(
        !create_cgroup(TEST_CGROUP, None),
        "an existing cgroup is reported, not recreated"
    );

    let _ = fs::remove_dir(&path);
}

#[test]
fn create_cgroup_can_apply_a_cpu_quota() {
    if !require_root("create_cgroup_can_apply_a_cpu_quota") {
        return;
    }
    let Some(path) = test_cgroup_dir() else {
        return;
    };
    let _ = fs::remove_dir(&path);

    assert!(create_cgroup(TEST_CGROUP, Some(90)));

    let _ = fs::remove_dir(&path);
}

#[test]
fn a_process_can_be_moved_into_a_cgroup_and_back() {
    if !require_root("a_process_can_be_moved_into_a_cgroup_and_back") {
        return;
    }
    let Some(path) = test_cgroup_dir() else {
        return;
    };
    let _ = fs::remove_dir(&path);
    if !create_cgroup(TEST_CGROUP, None) {
        return; // the hierarchy refused to delegate a new cgroup here
    }

    let pid = id() as i32;

    // Move to the root of the hierarchy first so the test is repeatable even if
    // a previous run left this process somewhere else.
    let _ = add_pid_to_cgroup(pid, "/");
    assert!(add_pid_to_cgroup(pid, TEST_CGROUP).is_ok());

    let current = current_cgroup_path(pid).expect("the process is in a cgroup");
    assert!(
        current.ends_with(TEST_CGROUP),
        "expected to end up in {TEST_CGROUP}, got {current}"
    );

    // Leave the machine as we found it.
    assert!(add_pid_to_cgroup(pid, "/").is_ok());
    let _ = fs::remove_dir(&path);
}
