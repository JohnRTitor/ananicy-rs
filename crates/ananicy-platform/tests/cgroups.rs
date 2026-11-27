//! Cgroup mutation against the live hierarchy.
//!
//! Everything here talks to the real cgroup filesystem of the machine running
//! the suite. The read-only detection tests always run; the tests that create a
//! cgroup and move a process into it need root *and* a hierarchy that actually
//! accepts writes. Root inside a container is not enough: Docker mounts
//! `/sys/fs/cgroup` read-only, so creating anything there fails with EROFS.
//! Those tests skip with a message written straight to stderr — libtest hides
//! `eprintln!` for passing tests — so an unprivileged or containerised
//! `cargo test` still validates the daemon's own logic without reporting a host
//! limitation as a bug, and the skip is visible in the CI log. On cgroup v2 a
//! transient `.scope` is such an environment: the daemon disables structural
//! mutations there because there is no delegated root.
//!
//! They also share one process-wide filesystem, so every mutating test takes a
//! lock and uses its own cgroup name: two tests removing or creating the same
//! directory in parallel make each other fail intermittently.

use {
    ananicy_platform::{
        cgroups::{
            CgroupSettings, CpuQuota, add_pid_to_cgroup, clear_created_registry, create_cgroup,
            created_cgroup_names,
        },
        mounts::{CgroupVersion, get_cgroup_info},
    },
    std::{
        fs,
        path::{Path, PathBuf},
        process::id,
        sync::{Mutex, MutexGuard},
    },
};

/// A prefix no packaged rule set is likely to use.
const TEST_CGROUP: &str = "UNIT_TEST_ANANICY";

/// Serialises all tests in this binary that mutate the live hierarchy.
static LIVE_HIERARCHY: Mutex<()> = Mutex::new(());

fn is_root() -> bool {
    rustix::process::geteuid().is_root()
}

/// Writes straight to stderr instead of using `eprintln!`: libtest captures the
/// print macros and hides their output for tests that pass, which would make a
/// skip indistinguishable from a real run in plain `cargo test` CI logs.
fn report(message: std::fmt::Arguments<'_>) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{message}");
}

/// Reports why a privileged test is not running, so a skip is never mistaken
/// for a pass in CI output.
fn require_root(test: &str) -> bool {
    if is_root() {
        return true;
    }
    report(format_args!(
        "skipping {test}: creating cgroups requires root"
    ));
    false
}

/// A cgroup name unique to this test run, so parallel runs and leftovers from
/// an interrupted run cannot collide, even if the PID is later reused.
fn unique_name(label: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!("{TEST_CGROUP}_{label}_{}_{nanos}", id())
}

/// Where the library creates a cgroup named by a relative rule name.
fn cgroup_base() -> Option<PathBuf> {
    let info = get_cgroup_info();
    match info.version {
        CgroupVersion::None => None,
        CgroupVersion::V1 => Some(info.mount_point.join("cpu")),
        CgroupVersion::V2 => Some(info.mount_point),
    }
}

/// Finds a cgroup under the hierarchy. The mount point is only the right answer
/// when there is no delegated root; otherwise cgroups live somewhere below it,
/// so search the tree instead of guessing a path. An unreadable directory is
/// simply skipped, and there is no depth limit: a nested delegated root is still
/// a valid place for the library to create.
fn find_cgroup(base: &Path, name: &str) -> Option<PathBuf> {
    let mut pending = vec![base.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let candidate = dir.join(name);
        if candidate.is_dir() {
            return Some(candidate);
        }
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    pending.push(entry.path());
                }
            }
        }
    }
    None
}

/// Drops a cgroup left behind by an interrupted earlier run, if there is one.
fn remove_if_present(base: &Path, name: &str) {
    if let Some(path) = find_cgroup(base, name) {
        let _ = fs::remove_dir(&path);
    }
}

/// The library deliberately disables v2 mutations when it runs in a transient
/// systemd scope (there is no delegated root there). A `sudo` launched from a
/// user session often lands in one, on a host whose cgroup mount is writable.
/// Detect that from `/proc` rather than from the API under test, so these tests
/// skip instead of demanding behaviour the daemon turns off by design.
fn in_transient_scope() -> bool {
    fs::read_to_string("/proc/self/cgroup").is_ok_and(|content| {
        content.lines().any(|line| {
            line.strip_prefix("0::")
                .is_some_and(|path| path.trim().ends_with(".scope"))
        })
    })
}

/// Takes the lock shared by the mutating tests and verifies that this machine
/// really lets us create a cgroup. The probe is a plain filesystem operation:
/// using `create_cgroup` here would turn a regression in the very API under test
/// into a silent skip. A read-only cgroup mount makes these tests meaningless.
fn writable_hierarchy(test: &str) -> Option<(MutexGuard<'static, ()>, PathBuf)> {
    let guard = LIVE_HIERARCHY.lock().unwrap_or_else(|e| e.into_inner());
    if !require_root(test) {
        return None;
    }
    let Some(base) = cgroup_base() else {
        report(format_args!(
            "skipping {test}: this host has no cgroup hierarchy"
        ));
        return None;
    };
    if get_cgroup_info().version == CgroupVersion::V2 && in_transient_scope() {
        report(format_args!(
            "skipping {test}: cgroup mutations are disabled inside a transient .scope"
        ));
        return None;
    }
    let probe = base.join(unique_name("PROBE"));
    let _ = fs::remove_dir(&probe);
    if let Err(e) = fs::create_dir(&probe) {
        report(format_args!(
            "skipping {test}: cannot create a cgroup under {}: {e}",
            base.display()
        ));
        return None;
    }
    let _ = fs::remove_dir(&probe);
    Some((guard, base))
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
    let test = "create_cgroup_refuses_to_recreate_an_existing_cgroup";
    let Some((_guard, base)) = writable_hierarchy(test) else {
        return;
    };
    let name = unique_name("RECREATE");
    remove_if_present(&base, &name);

    assert!(
        create_cgroup(&name, CgroupSettings::default()).created(),
        "the first creation succeeds"
    );
    let path =
        find_cgroup(&base, &name).expect("the created cgroup can be found under the hierarchy");
    assert!(path.is_dir(), "the cgroup directory must exist");
    assert!(
        !create_cgroup(&name, CgroupSettings::default()).created(),
        "an existing cgroup is reported, not recreated"
    );

    let _ = fs::remove_dir(&path);
}

/// Only the creation is recorded. Re-applying settings to an existing cgroup
/// (which is what the previous test checks) must not claim it as ours to
/// clean up later.
#[test]
fn create_cgroup_records_only_the_cgroup_it_created() {
    let test = "create_cgroup_records_only_the_cgroup_it_created";
    let Some((_guard, base)) = writable_hierarchy(test) else {
        return;
    };
    let name = unique_name("REGISTRY");
    remove_if_present(&base, &name);
    clear_created_registry();

    assert!(create_cgroup(&name, CgroupSettings::default()).created());
    assert!(
        created_cgroup_names().contains(&name),
        "the created cgroup must be registered for later reconciliation"
    );

    assert!(!create_cgroup(&name, CgroupSettings::default()).created());
    assert_eq!(
        created_cgroup_names()
            .iter()
            .filter(|recorded| *recorded == &name)
            .count(),
        1,
        "re-applying to an existing cgroup must not add a second entry"
    );

    clear_created_registry();
    if let Some(path) = find_cgroup(&base, &name) {
        let _ = fs::remove_dir(&path);
    }
}

#[test]
fn create_cgroup_can_apply_a_cpu_quota() {
    let test = "create_cgroup_can_apply_a_cpu_quota";
    let Some((_guard, base)) = writable_hierarchy(test) else {
        return;
    };
    let name = unique_name("QUOTA");
    remove_if_present(&base, &name);

    assert!(
        create_cgroup(
            &name,
            CgroupSettings {
                cpu_quota: Some(CpuQuota::Percentage(90)),
                ..Default::default()
            },
        )
        .is_ok()
    );

    remove_if_present(&base, &name);
}

#[test]
fn a_process_can_be_moved_into_a_cgroup_and_back() {
    let test = "a_process_can_be_moved_into_a_cgroup_and_back";
    let Some((_guard, base)) = writable_hierarchy(test) else {
        return;
    };
    let name = unique_name("MOVE");
    remove_if_present(&base, &name);

    assert!(
        create_cgroup(&name, CgroupSettings::default()).created(),
        "the test cgroup is created"
    );
    let path =
        find_cgroup(&base, &name).expect("the created cgroup can be found under the hierarchy");

    let pid = id() as i32;
    let original = current_cgroup_path(pid);

    // Move to the root of the hierarchy first so the test is repeatable even if
    // a previous run left this process somewhere else. Whether the host lets us
    // write to the root is not what this test is about; the move in is.
    let _ = add_pid_to_cgroup(pid, "/");
    assert!(add_pid_to_cgroup(pid, &name).is_ok());

    let current = current_cgroup_path(pid).expect("the process is in a cgroup");
    assert!(
        current.ends_with(&name),
        "expected to end up in {name}, got {current}"
    );

    // Restoring the process is cleanup, not a behaviour under test: ownership
    // rules decide whether we may write back to the original cgroup, and the
    // process exits with this test binary either way. Try the root as a fallback
    // so the cgroup can usually be removed; report any failure instead of
    // quietly leaving the process and its cgroup behind.
    let back = original.as_deref().unwrap_or("/");
    let mut restored = add_pid_to_cgroup(pid, back).is_ok();
    if !restored && back != "/" {
        restored = add_pid_to_cgroup(pid, "/").is_ok();
    }
    if !restored {
        report(format_args!(
            "could not restore the test process to {back}; it exits with the test binary"
        ));
    } else if let Err(e) = fs::remove_dir(&path) {
        report(format_args!(
            "could not remove the test cgroup {}: {e}",
            path.display()
        ));
    }
}
