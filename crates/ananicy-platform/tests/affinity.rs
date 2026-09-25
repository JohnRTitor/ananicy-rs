//! CPU affinity through the `sched_setaffinity` ABI.
//!
//! These are system tests: they call the real syscall. They only ever restrict
//! the *test process* to the affinity it already has, so they never disturb the
//! machine and they do not need root — a process may always change its own
//! affinity.
//!
//! The test binary runs these tests in parallel threads of one process, and
//! affinity is per-thread, so every test takes [`affinity_lock`] before touching
//! the mask. Without it, one test restoring the original mask could race with
//! another test asserting a restricted one.

use {
    ananicy_core::cpuset::CpuSet,
    ananicy_platform::abi::affinity::{get_max_number_of_cpus, set_affinity},
    rustix::process::Pid,
    std::{process, sync::MutexGuard},
};

/// A PID above any plausible `pid_max` can never refer to a live process.
const DEAD_PID: i32 = i32::MAX;

static AFFINITY: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn affinity_lock() -> MutexGuard<'static, ()> {
    AFFINITY.lock().unwrap_or_else(|e| e.into_inner())
}

fn self_pid() -> i32 {
    process::id() as i32
}

/// The affinity mask the test process currently has, as a `CpuSet` wide enough
/// to cover the whole `cpu_set_t`.
fn current_affinity() -> CpuSet {
    let max_cores = get_max_number_of_cpus();
    let mut set = CpuSet::new(max_cores);
    let pid = Pid::from_raw(self_pid()).expect("the test process is alive");

    let mask =
        rustix::thread::sched_getaffinity(Some(pid)).expect("a live process has an affinity");
    for cpu in 0..max_cores {
        if mask.is_set(cpu as usize) {
            set.set_cpu(cpu);
        }
    }
    set
}

#[test]
fn test_set_affinity_on_current_process() {
    let _guard = affinity_lock();

    let before = current_affinity();
    assert!(
        !before.is_empty(),
        "the calling process must have at least one CPU in its mask"
    );

    let result = set_affinity(self_pid(), &[self_pid()], &before);
    assert!(
        result.is_ok(),
        "set_affinity on current process should succeed"
    );

    assert_eq!(
        current_affinity(),
        before,
        "re-applying the same mask must not change the affinity"
    );
}

#[test]
fn a_single_cpu_mask_is_applied_and_restored() {
    let _guard = affinity_lock();

    let before = current_affinity();
    let Some(first) = before.get_cores().first().copied() else {
        return;
    };

    let mut single = CpuSet::new(get_max_number_of_cpus());
    single.set_cpu(first);

    set_affinity(self_pid(), &[self_pid()], &single).expect("restricting to one CPU is allowed");
    assert_eq!(current_affinity().get_cores(), vec![first]);

    // Leaving the machine as we found it is mandatory: the test binary runs its
    // remaining tests on this process.
    set_affinity(self_pid(), &[self_pid()], &before).expect("restoring must be allowed");
    assert_eq!(current_affinity(), before);
}

#[test]
fn an_empty_cpuset_is_a_no_op() {
    // An empty set means "do not touch the affinity" rather than "no CPUs at
    // all", which is the only safe interpretation for a `cpuset` rule field.
    let _guard = affinity_lock();

    let before = current_affinity();
    let empty = CpuSet::new(get_max_number_of_cpus());

    assert!(set_affinity(self_pid(), &[], &empty).is_ok());
    assert_eq!(current_affinity(), before);
}

#[test]
fn test_set_affinity_on_nonexistent_process() {
    let mut cs = CpuSet::new(get_max_number_of_cpus());
    cs.set_cpu(0);

    let result = set_affinity(DEAD_PID, &[DEAD_PID], &cs);
    assert!(
        result.is_err(),
        "set_affinity on nonexistent process should fail"
    );
}

#[test]
fn test_set_affinity_with_zero_ncpus_cpuset() {
    // A cpuset with no CPU slots at all must fail gracefully, not panic.
    let cs = CpuSet::new(0);
    let _ = set_affinity(self_pid(), &[], &cs);
}

#[test]
fn a_cpu_beyond_the_configured_range_is_rejected() {
    // The cpuset parser only enforces a syntactic bound, so a rule may name a
    // CPU the machine does not have. The syscall has to report that instead of
    // silently widening the mask.
    let _guard = affinity_lock();

    let configured = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_CONF) };
    let beyond = configured as u32;
    if configured <= 0 || beyond >= get_max_number_of_cpus() {
        return; // this machine has more CPUs than the mask can address
    }

    let before = current_affinity();
    let mut mask = CpuSet::new(get_max_number_of_cpus());
    mask.set_cpu(beyond);

    assert!(
        set_affinity(self_pid(), &[self_pid()], &mask).is_err(),
        "CPU {beyond} is outside the configured range and must be refused"
    );
    assert_eq!(current_affinity(), before, "a refused mask changes nothing");
}
