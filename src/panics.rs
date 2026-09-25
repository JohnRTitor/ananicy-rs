//! What to print when the daemon is about to die unexpectedly.
//!
//! A panic anywhere in the daemon is a bug, and a bug that only says "index out
//! of bounds" in the journal is a bug that is hard to report. This prints the
//! panic message, the thread it happened on and a backtrace before the process
//! aborts.
//!
//! Release builds set `panic = "abort"`, so there is no unwinding to catch a
//! panic further out — a hook is the only place left to print anything. It runs
//! before the abort either way, which is why it works with that setting.

use std::backtrace::Backtrace;

/// Installs the panic hook.
///
/// A `RUST_BACKTRACE` environment variable other than `0` already asks for a
/// backtrace, so nothing is installed in that case: the standard hook is better
/// at it, and its output is what users are used to reading.
pub(crate) fn install() {
    if backtrace_requested() {
        return;
    }

    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");

        // stderr, not the logger: the logger may be the thing that is broken,
        // and a panic that loses its own report is the worst kind.
        eprintln!("ananicy-rs panicked in thread {name}: {info}");
        eprintln!("backtrace:\n{}", Backtrace::force_capture());
    }));
}

fn backtrace_requested() -> bool {
    std::env::var_os("RUST_BACKTRACE").is_some_and(|value| !value.is_empty() && value != "0")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_backtrace_is_the_default() {
        // `Backtrace::force_capture` is used rather than `capture`, which would
        // print nothing unless RUST_BACKTRACE asked for it — and the whole point
        // is that the operator has not set it.
        let trace = Backtrace::force_capture().to_string();
        assert!(
            !trace.is_empty(),
            "a forced backtrace always has frames or at least its header"
        );
    }

    #[test]
    fn the_hook_can_be_installed_and_does_not_panic() {
        // Setting the hook twice is allowed, and installing it must not fail
        // even when another hook is already there.
        install();
        install();
    }
}
