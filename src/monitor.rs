use {
    ananicy_core::process::Process,
    ananicy_platform::procfs::ProcfsScanner,
    std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc::Sender,
        },
        thread,
        time::Duration,
    },
    tracing::{error, info, warn},
};

pub(crate) fn run(
    tx: Sender<Process>,
    shutdown_flag: Arc<AtomicBool>,
    worker_handle: std::thread::JoinHandle<(usize, Duration)>,
    saved_x3d_mode: Option<ananicy_platform::x3d::X3DMode>,
    bpf_min_us: Option<u32>,
) {
    #[cfg(feature = "ananicy-bpf")]
    {
        use ananicy_bpf::BpfMonitor;
        info!("Attempting to start BPF monitor...");
        loop {
            match BpfMonitor::new(bpf_min_us) {
                Ok(mut bpf) => {
                    info!("BPF monitor successfully started.");
                    let tx_clone = tx.clone();
                    let tx_scan = tx.clone();
                    info!("Running initial procfs full scan");
                    ananicy_core::spawn_named_thread!("ananicy-init", move || {
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
                        ananicy_core::spawn_named_thread!("ananicy-init", move || {
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
                    std::process::exit(1);
                }
            }
        }
    }

    #[cfg(not(feature = "netlink"))]
    {
        error!("No event monitor available. Exiting.");
        restore_x3d(saved_x3d_mode, "on startup failure");
        std::process::exit(1);
    }
}

fn finish(
    tx: Sender<Process>,
    worker_handle: std::thread::JoinHandle<(usize, Duration)>,
    saved_x3d_mode: Option<ananicy_platform::x3d::X3DMode>,
) {
    drop(tx);
    finish_join(worker_handle, saved_x3d_mode);
}

fn finish_join(
    worker_handle: std::thread::JoinHandle<(usize, Duration)>,
    saved_x3d_mode: Option<ananicy_platform::x3d::X3DMode>,
) {
    match worker_handle.join() {
        Ok((count, duration)) => info!("Worker processed {} events in {:?}", count, duration),
        Err(e) => error!("Worker thread panicked: {:?}", e),
    }
    restore_x3d(saved_x3d_mode, "on shutdown");
}

fn restore_x3d(saved_x3d_mode: Option<ananicy_platform::x3d::X3DMode>, reason: &str) {
    if let Some(mode) = saved_x3d_mode
        && ananicy_platform::x3d::set_driver_mode(mode)
    {
        info!("Restored X3D mode {}", reason);
    }
}
