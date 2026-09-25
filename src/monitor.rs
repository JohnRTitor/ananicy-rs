use {
    ananicy_core::{process::Process, spawn_named_thread},
    ananicy_platform::{procfs::ProcfsScanner, x3d::X3DMode},
    std::{process::exit, thread::JoinHandle},
};

use {
    std::{
        sync::{Arc, atomic::AtomicBool, mpsc::Sender},
        thread,
        time::Duration,
    },
    tracing::{error, info, warn},
};

// `bpf_min_us` and `verbose` are only read by the eBPF backend, so they are
// unused in a netlink build.
#[cfg_attr(not(feature = "bpf"), allow(unused_variables))]
pub(crate) fn run(
    tx: Sender<Process>,
    shutdown_flag: Arc<AtomicBool>,
    worker_handle: JoinHandle<(usize, Duration)>,
    saved_x3d_mode: Option<X3DMode>,
    bpf_min_us: Option<u32>,
    verbose: bool,
) {
    #[cfg(feature = "bpf")]
    {
        use {ananicy_bpf::BpfMonitor, std::sync::atomic::Ordering};
        info!("Attempting to start BPF monitor...");
        loop {
            match BpfMonitor::new(bpf_min_us, verbose) {
                Ok(mut bpf) => {
                    info!("BPF monitor successfully started.");
                    let tx_clone = tx.clone();
                    let tx_scan = tx.clone();
                    info!("Running initial procfs full scan");
                    spawn_named_thread!("ananicy-init", move || {
                        ProcfsScanner::full_scan(tx_scan);
                    });
                    bpf.listen(tx_clone, shutdown_flag.clone());

                    if shutdown_flag.load(Ordering::SeqCst) {
                        return finish(tx, worker_handle, saved_x3d_mode);
                    }

                    warn!("BPF monitor exited unexpectedly, restarting in 1 second...");
                    thread::sleep(Duration::from_secs(1));
                }
                Err(e) => {
                    warn!(
                        "Failed to start BPF monitor: {}. Falling back to Netlink.",
                        e
                    );
                    break;
                }
            }
        }
    }

    #[cfg(feature = "netlink")]
    {
        use ananicy_platform::netlink::NetlinkMonitor;
        info!("Attempting to start Netlink monitor...");
        let mut is_first = true;
        loop {
            match NetlinkMonitor::new() {
                Ok(mut nl) => {
                    info!("Netlink monitor successfully started.");
                    let tx_clone = tx.clone();
                    if is_first {
                        is_first = false;
                        let tx_scan = tx.clone();
                        info!("Running initial procfs full scan");
                        spawn_named_thread!("ananicy-init", move || {
                            ProcfsScanner::full_scan(tx_scan);
                        });
                    }

                    if let Err(e) = nl.listen(tx_clone.clone(), shutdown_flag.clone()) {
                        warn!(
                            "Netlink error: {}. Triggering full scan recovery and reconnect...",
                            e
                        );
                        ProcfsScanner::full_scan(tx_clone.clone());
                        thread::sleep(Duration::from_secs(1));
                        continue;
                    }

                    drop(tx);
                    drop(tx_clone);
                    finish_join(worker_handle, saved_x3d_mode);
                    break;
                }
                Err(e) => {
                    error!("Failed to start Netlink monitor: {}. Exiting.", e);
                    restore_x3d(saved_x3d_mode, "on netlink monitor failure");
                    exit(1);
                }
            }
        }
    }

    #[cfg(not(feature = "netlink"))]
    {
        error!("No event monitor available. Exiting.");
        restore_x3d(saved_x3d_mode, "on startup failure");
        exit(1);
    }
}

#[allow(dead_code)]
fn finish(
    tx: Sender<Process>,
    worker_handle: JoinHandle<(usize, Duration)>,
    saved_x3d_mode: Option<X3DMode>,
) {
    drop(tx);
    finish_join(worker_handle, saved_x3d_mode);
}

fn finish_join(worker_handle: JoinHandle<(usize, Duration)>, saved_x3d_mode: Option<X3DMode>) {
    match worker_handle.join() {
        Ok((count, duration)) => {
            info!("Summary:");
            info!(
                "{} processes processed, ran for {} seconds",
                count,
                format_elapsed(duration)
            );
        }
        Err(e) => error!("Worker thread panicked: {:?}", e),
    }
    restore_x3d(saved_x3d_mode, "on shutdown");
}

/// `HH:MM:SS.ffffff`, the shape `ananicy-cpp` prints its runtime in, so both
/// daemons' shutdown summaries can be compared at a glance.
fn format_elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs();
    format!(
        "{:02}:{:02}:{:02}.{:06}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60,
        duration.subsec_micros()
    )
}

fn restore_x3d(saved_x3d_mode: Option<X3DMode>, reason: &str) {
    if let Some(mode) = saved_x3d_mode
        && ananicy_platform::x3d::set_driver_mode(mode)
    {
        info!("Restored X3D mode {}", reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_is_reported_as_hours_minutes_and_microseconds() {
        assert_eq!(format_elapsed(Duration::ZERO), "00:00:00.000000");
        assert_eq!(
            format_elapsed(Duration::from_micros(45_110_643)),
            "00:00:45.110643"
        );
        assert_eq!(
            format_elapsed(Duration::from_secs(3661) + Duration::from_micros(500_000)),
            "01:01:01.500000"
        );
    }
}
