//! Process discovery through `/proc`.
//!
//! These are system tests: they read the live `/proc` of the machine running
//! them and of the test process itself, so they stay independent of any
//! particular daemon, configuration file or CPU topology. The "disappearing
//! process" cases use a PID that is guaranteed not to exist, which is the
//! situation the daemon has to survive when an event arrives after the process
//! has already exited.

use {
    ananicy_core::worker::{PlatformActions, PlatformError},
    ananicy_platform::{LinuxPlatform, procfs},
};

/// A PID above `/proc/sys/kernel/pid_max` can never refer to a live process.
const DEAD_PID: i32 = i32::MAX;

fn self_pid() -> i32 {
    std::process::id() as i32
}

#[test]
fn a_process_name_is_resolved_for_a_live_process() {
    let name = procfs::get_command_from_pid(self_pid());
    assert!(!name.is_empty());
    assert_ne!(
        name, "<unknown>",
        "the test process itself is always resolvable"
    );
    assert!(
        !name.contains('/'),
        "only the basename may be returned, got {name:?}"
    );
}

#[test]
fn an_unknown_process_has_no_name() {
    assert_eq!(procfs::get_command_from_pid(DEAD_PID), "<unknown>");
    assert_eq!(procfs::get_command_from_pid(0), "<unknown>");
    assert_eq!(procfs::get_command_from_pid(-1), "<unknown>");
}

#[test]
fn the_start_time_of_a_live_process_is_reported() {
    let start_time =
        procfs::get_start_time(self_pid()).expect("the test process is alive and has a start time");
    assert!(
        start_time > 0,
        "a live process cannot have a zero start time"
    );

    // The start time is the PID-reuse guard, so it must be stable across calls.
    assert_eq!(procfs::get_start_time(self_pid()), Some(start_time));
}

#[test]
fn an_unknown_process_has_no_start_time() {
    assert_eq!(procfs::get_start_time(DEAD_PID), None);
    assert_eq!(procfs::get_start_time(0), None);
}

#[test]
fn the_thread_group_id_of_a_live_process_is_itself() {
    assert_eq!(procfs::get_tgid(self_pid()), Some(self_pid()));
    assert_eq!(procfs::get_tgid(DEAD_PID), None);
}

#[test]
fn thread_enumeration_lists_the_leader_of_a_live_process() {
    let tids = procfs::get_tids(self_pid()).expect("the test process is alive");
    assert!(
        tids.contains(&self_pid()),
        "a thread group always contains its leader, got {tids:?}"
    );
}

#[test]
fn thread_enumeration_of_a_dead_process_is_reported_as_not_found() {
    assert!(matches!(
        procfs::get_tids(DEAD_PID),
        Err(PlatformError::NotFound)
    ));
}

#[test]
fn the_platform_reports_a_usable_core_count() {
    // `get_max_cores` is the width of the `cpu_set_t` mask the daemon builds, so
    // it must be able to address every CPU the kernel reports *and* the whole
    // fixed-size mask, otherwise `sched_setaffinity` would truncate it.
    let max_cores = LinuxPlatform::new().get_max_cores();
    assert!(
        max_cores > 0,
        "a CPU count of zero would break every cpuset"
    );
    assert!(max_cores >= 1024, "the mask must span a full cpu_set_t");
    assert_eq!(max_cores % 8, 0, "the mask is addressed in whole bytes");

    let configured = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_CONF) };
    if configured > 0 {
        assert!(
            max_cores >= configured as u32,
            "get_max_cores ({max_cores}) is below the configured CPUs ({configured})"
        );
    }
}

#[test]
fn a_live_process_is_never_reported_as_realtime() {
    // The test harness is not a realtime task; this documents the contract of
    // the check that gates the realtime cgroup workaround.
    assert!(!LinuxPlatform::new().is_realtime(self_pid()));
}

#[test]
fn the_platform_exposes_the_process_identity_it_was_built_from() {
    // The comparison cannot be an exact list: the test binary runs its tests in
    // parallel, so the thread list may legitimately change between the two
    // enumerations. What must hold is that both paths see the same process.
    let platform = LinuxPlatform::new();
    let name = platform.get_process_name(self_pid());
    assert!(!name.is_empty());
    assert_eq!(
        platform.get_start_time(self_pid()),
        procfs::get_start_time(self_pid())
    );

    for tids in [platform.get_tids(self_pid()), procfs::get_tids(self_pid())] {
        let tids = tids.expect("the test process is alive");
        assert!(tids.contains(&self_pid()));
    }
}
