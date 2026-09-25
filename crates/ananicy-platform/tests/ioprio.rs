//! The `ioprio_set(2)` ABI.
//!
//! These are system tests: they call the real syscall, and only ever against the
//! test process itself, which may always change its own I/O priority without
//! privileges. Every test takes [`ioprio_lock`] and restores the priority it
//! found, because the test binary is one process shared by all its threads.

use {
    ananicy_platform::{
        abi::ioprio::{IOPRIO_CLASS_BE, IOPRIO_CLASS_IDLE, IOPRIO_WHO_PROCESS, ioprio_get},
        priority::set_io_priority,
    },
    std::process,
};

static IOPRIO: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn ioprio_lock() -> std::sync::MutexGuard<'static, ()> {
    IOPRIO.lock().unwrap_or_else(|e| e.into_inner())
}

fn self_pid() -> i32 {
    process::id() as i32
}

fn class_and_data(value: i32) -> (i32, i32) {
    (value >> 13, value & ((1 << 13) - 1))
}

fn current() -> i32 {
    ioprio_get(IOPRIO_WHO_PROCESS, self_pid()).expect("a live process has an ioprio")
}

/// The kernel may not implement `ioprio_set` at all (an unsupported filesystem
/// answers `EINVAL`/`ENOSYS`). Skipping loudly beats asserting on a platform
/// that never had the feature.
fn require_ioprio_support() -> bool {
    if set_io_priority(self_pid(), "best-effort", 4).is_ok() {
        true
    } else {
        eprintln!("skipping: ioprio_set is not available on this kernel");
        false
    }
}

#[test]
fn a_class_the_task_can_hold_is_written_through() {
    let _guard = ioprio_lock();
    if !require_ioprio_support() {
        return;
    }
    let before = current();

    assert!(
        set_io_priority(self_pid(), "best-effort", 3).is_ok(),
        "best-effort is a class the kernel accepts"
    );
    assert_eq!(class_and_data(current()), (IOPRIO_CLASS_BE, 3));

    assert!(set_io_priority(self_pid(), "idle", 0).is_ok());
    assert_eq!(class_and_data(current()), (IOPRIO_CLASS_IDLE, 0));

    // `realtime` is deliberately not exercised: the kernel checks CAP_SYS_ADMIN
    // for that class, and these tests stay unprivileged.

    let (class, data) = class_and_data(before);
    let _ = set_io_priority(
        self_pid(),
        match class {
            IOPRIO_CLASS_IDLE => "idle",
            _ => "best-effort",
        },
        data,
    );
}

#[test]
fn the_none_class_leaves_the_io_priority_untouched() {
    let _guard = ioprio_lock();
    if !require_ioprio_support() {
        return;
    }

    // Put the process in a class we can recognise, then ask for `none`.
    assert!(set_io_priority(self_pid(), "best-effort", 5).is_ok());
    let before = current();

    assert!(
        set_io_priority(self_pid(), "none", 0).is_ok(),
        "an unapplicable class is not an error"
    );
    assert_eq!(
        current(),
        before,
        "`none` is the 'nothing was set' reading, so writing it would replace \
         the process' I/O priority instead of restoring the default"
    );
    assert_ne!(
        class_and_data(current()).0,
        0,
        "the process must not be left in class none"
    );

    let _ = set_io_priority(self_pid(), "best-effort", 4);
}
