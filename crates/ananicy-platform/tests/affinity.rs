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
    ananicy_platform::{
        LinuxPlatform,
        abi::affinity::{get_max_number_of_cpus, set_affinity},
    },
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

/// A process that has gone is reported as gone, at the surface the worker calls.
///
/// This is the test that distinguishes the fix from the code before it. The
/// `abi::affinity` layer always produced `ESRCH` correctly; `LinuxPlatform`
/// wrapped it in `PlatformError::Unsupported` for *every* failure, and
/// `Unsupported` is the value the worker treats as "abort the whole rule" and
/// renders as a rule failure. So a process that exited between the
/// `/proc/<pid>/task` snapshot and the syscall — an entirely ordinary event — was
/// logged as a rule the daemon could not apply.
///
/// Against the pre-fix code this test fails with `Err(Unsupported)`.
#[test]
fn a_vanished_process_is_reported_as_gone_not_as_an_unsupported_kernel() {
    use ananicy_core::worker::{PlatformActions, PlatformError};

    let platform = LinuxPlatform::new();
    let mut any = CpuSet::new(get_max_number_of_cpus());
    any.set_cpu(0);

    match platform.set_affinity(DEAD_PID, &[DEAD_PID], &any) {
        Err(PlatformError::NotFound) => {}
        other => panic!("a pid that cannot exist must read as NotFound, got {other:?}"),
    }
}

/// A mask the kernel considers impossible is reported as the kernel's own
/// `EINVAL`, not as an unsupported platform.
///
/// Same wrapper, different errno. This is the case where a rule names a CPU the
/// machine does not have — the `CpuSet` parser only enforces a syntactic bound —
/// and the operator deserves to be told that, rather than to be told the kernel
/// has no affinity support.
///
/// `Skipped` is the other wrong answer: it reads "no CPU in the cpuset a rule
/// asked for is available to *this pid*", which is a claim about the process,
/// where a CPU the machine does not have is a claim about the machine.
///
/// Against the pre-fix code this test fails with `Err(Unsupported)`.
#[test]
fn a_mask_the_kernel_cannot_accept_is_reported_as_such() {
    use ananicy_core::worker::{PlatformActions, PlatformError};

    let _guard = affinity_lock();
    let configured = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_CONF) };
    if configured <= 0 {
        return;
    }
    let beyond = configured as u32 + 64;
    if beyond >= get_max_number_of_cpus() {
        return; // this machine has more CPUs than the mask can address
    }

    let before = current_affinity();
    let mut mask = CpuSet::new(get_max_number_of_cpus());
    mask.set_cpu(beyond);

    let platform = LinuxPlatform::new();
    let outcome = platform.set_affinity(self_pid(), &[self_pid()], &mask);

    assert_eq!(current_affinity(), before, "a refused mask changes nothing");
    assert!(
        matches!(
            outcome,
            Err(PlatformError::Io(_)) | Err(PlatformError::InvalidCpuset(_))
        ),
        "the kernel's own refusal must survive the wrapper, got {outcome:?}"
    );
}

/// The narrowing retry is defensive.
///
/// A mask with no CPU in common with the process's **cpuset** is rejected
/// outright by the kernel — `cpumask_and(new_mask, ctx->new_mask,
/// cpuset_cpus_allowed(p, ...))` in `__sched_setaffinity()`, and an empty
/// intersection is `EINVAL`. That is the case the retry exists for: a rule
/// naming `big-cores` for a process systemd has confined to `little-cores` via
/// `AllowedCPUs=`, or one inside a container cpuset.
///
/// It is *not* reproducible by narrowing the process's affinity, which is what
/// this suite can do: `cpuset_cpus_allowed` is the cpuset, not the current
/// affinity, so asking for a CPU outside a `taskset` restriction but inside the
/// cpuset simply succeeds and widens the mask. So this test pins the invariant
/// rather than the failure — a narrowed mask must be non-empty and must leave the
/// process on a CPU it is allowed to use — and the EINVAL path itself is covered
/// by the two tests above.
#[test]
fn a_narrowed_mask_never_leaves_the_process_where_it_could_not_run() {
    let _guard = affinity_lock();

    let before = current_affinity();
    let allowed = before.get_cores();
    let Some(&first) = allowed.first() else {
        return;
    };
    let second = allowed.iter().copied().find(|&c| c != first);
    let Some(second) = second else { return };

    let mut only_first = CpuSet::new(get_max_number_of_cpus());
    only_first.set_cpu(first);
    set_affinity(self_pid(), &[self_pid()], &only_first).expect("restricting is allowed");

    let mut only_second = CpuSet::new(get_max_number_of_cpus());
    only_second.set_cpu(second);

    // The kernel's answer here is "yes": the cpuset allows it, so this *widens*
    // the mask. What must not happen is a reported success with an empty mask, or
    // a mask containing a CPU the process is not allowed to run on.
    let outcome = set_affinity(self_pid(), &[self_pid()], &only_second);
    let after = current_affinity().get_cores();
    set_affinity(self_pid(), &[self_pid()], &before).expect("restoring is allowed");

    assert!(
        outcome.is_ok(),
        "a CPU inside the cpuset is always acceptable: {outcome:?}"
    );
    assert!(
        !after.is_empty(),
        "an affinity must never end up empty; that is EINVAL at best and a task \
         that cannot run at all at worst"
    );
    assert!(
        after.iter().all(|c| allowed.contains(c)),
        "the process must not be left on a CPU outside what it was allowed: {after:?}"
    );
}
