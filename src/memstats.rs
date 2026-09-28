//! Reporting what the daemon's own memory is doing.
//!
//! The numbers themselves live in [`ananicy_platform::memstats`], because the
//! BPF backend needs to report one too and it is a different crate. This is the
//! part that decides *when*: a one-off report at the points where the footprint
//! changes shape, and an optional line a minute for as long as the daemon runs.

use {
    ananicy_core::spawn_named_thread,
    ananicy_platform::memstats::{REPORT_INTERVAL, Snapshot},
    std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering::SeqCst},
        },
        thread::sleep,
    },
    tracing::{debug, info, warn},
};

/// Logs the footprint once, and says what it means.
///
/// Called at the three points where the number changes by an order of magnitude
/// rather than incrementally: after the rule set is built, after the BPF program
/// is loaded, and after the initial `/proc` walk. Which of them is responsible
/// for the heap is otherwise a guess, because `/proc/self/status` only shows the
/// total and the pieces come from three different subsystems.
pub fn report(stage: &str) -> Snapshot {
    let snapshot = Snapshot::read();
    debug!("Memory after {stage}: {}", snapshot.summary());
    for finding in snapshot.findings() {
        warn!("Memory after {stage}: {finding}");
    }
    snapshot
}

/// Starts the periodic report, if `--memory-stats` asked for it.
///
/// A snapshot every minute, not every second: this is a slow-moving picture of a
/// daemon that is idle between events, and a per-second line would bury the
/// tuning output it is meant to sit alongside. The spawn failure is a warning
/// rather than a fatal error, because losing a diagnostic must not stop a daemon
/// that is otherwise working.
pub fn start_periodic(shutdown_flag: Arc<AtomicBool>) {
    if let Err(e) = spawn_named_thread!("ananicy-memstats", move || {
        // Reported before the first wait so the starting point is in the log
        // even if the daemon is stopped inside the first minute.
        report("startup");

        let mut previous = Snapshot::read();
        while !shutdown_flag.load(SeqCst) {
            sleep(REPORT_INTERVAL);
            if shutdown_flag.load(SeqCst) {
                break;
            }

            let current = Snapshot::read();
            // The per-minute deltas are the part that shows a steady rate, where
            // the totals so far only show a total. A daemon sitting in a reclaim
            // loop moves thousands of these a minute; a healthy one moves very
            // few.
            info!(
                "Memory: {}\n  last {}s: +{} faults ({} major), +{} refaults, \
                 +{} pages out / +{} in, +{} read / +{} written",
                current.summary(),
                REPORT_INTERVAL.as_secs(),
                delta(current.pgfault, previous.pgfault),
                delta(current.pgmajfault, previous.pgmajfault),
                delta(current.refault_file, previous.refault_file)
                    + delta(current.refault_anon, previous.refault_anon),
                delta(current.pswpout, previous.pswpout),
                delta(current.pswpin, previous.pswpin),
                delta(current.read_bytes, previous.read_bytes),
                delta(current.written_bytes, previous.written_bytes),
            );
            for finding in current.findings() {
                warn!("Memory: {finding}");
            }

            previous = current;
        }
    }) {
        warn!("Failed to start the memory reporter: {e}");
    }
}

/// A counter's movement since the last report, as a signed number.
///
/// A cgroup counter is cumulative, so the delta is the only readable form — and
/// a counter that appeared or went missing between the two reads is reported as
/// no movement rather than as a wrong one.
fn delta(current: Option<u64>, previous: Option<u64>) -> i64 {
    match (current, previous) {
        (Some(current), Some(previous)) => current as i64 - previous as i64,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delta_is_the_movement_between_two_readings() {
        assert_eq!(delta(Some(100), Some(40)), 60);
        assert_eq!(delta(Some(40), Some(100)), -60);
        assert_eq!(delta(Some(40), Some(40)), 0);
    }

    /// A counter that was unreadable on one of the two reads has no movement we
    /// can claim, and a huge number invented from a missing reading would be
    /// worse than silence.
    #[test]
    fn a_counter_that_appeared_or_vanished_reports_no_movement() {
        assert_eq!(delta(None, Some(100)), 0);
        assert_eq!(delta(Some(100), None), 0);
        assert_eq!(delta(None, None), 0);
    }

    /// Reading the live process is the only way to know the reader does not
    /// panic on a host that has none of these files.
    #[test]
    fn a_report_of_this_process_succeeds() {
        let snapshot = report("a test");
        assert!(snapshot.vm_data.is_some());
    }
}
