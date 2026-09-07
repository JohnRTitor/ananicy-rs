use {
    crate::monitor,
    ananicy_core::{config::Config, process::Process, rules::Rules, worker::Worker},
    std::{
        collections::HashMap,
        sync::{Arc, atomic::AtomicBool, mpsc::Receiver},
        thread,
        time::{Duration, Instant},
    },
    tracing::info,
};

pub(crate) fn run(
    config: Arc<Config>,
    rules: Arc<Rules>,
    platform: Arc<ananicy_platform::LinuxPlatform>,
    aliases: HashMap<String, String>,
    rx: Receiver<Process>,
    tx: std::sync::mpsc::Sender<Process>,
    shutdown_flag: Arc<AtomicBool>,
    manual_scanning: bool,
    cgroup_realtime_workaround: bool,
    bpf_min_us: Option<u32>,
    is_systemd: bool,
    saved_x3d_mode: Option<ananicy_platform::x3d::X3DMode>,
    benchmark: bool,
    benchmark_count: Option<u32>,
) {
    if benchmark || benchmark_count.is_some() {
        if benchmark {
            tracing::warn!("Benchmark enabled!");
            let shutdown = shutdown_flag.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(30));
                shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
            });
        }
        if let Some(count) = benchmark_count {
            tracing::warn!("Benchmark count: {}", count);
        }
    }

    info!("Initializing cgroups based on rules");
    if !create_cgroups(&rules) {
        return;
    }

    info!("Spawning worker thread");
    let worker = Worker::new(
        config.clone(),
        rules.clone(),
        platform,
        aliases,
        rx,
        benchmark_count,
        shutdown_flag.clone(),
    );
    let worker_handle = worker.start();

    #[cfg(feature = "systemd")]
    if is_systemd {
        info!("Notifying systemd of readiness...");
        let _ = sd_notify::notify(&[sd_notify::NotifyState::Ready]);
    }

    if cgroup_realtime_workaround {
        thread::sleep(Duration::from_millis(100));
        ananicy_platform::mounts::reset_cgroup_info();
        if !create_cgroups(&rules) {
            return;
        }
    }

    if manual_scanning {
        start_manual_scanner(config.clone(), tx.clone(), shutdown_flag.clone());
    }

    monitor::run(tx, shutdown_flag, worker_handle, saved_x3d_mode, bpf_min_us);
}

fn create_cgroups(rules: &Arc<Rules>) -> bool {
    for (name, value) in rules.get_cgroups() {
        let quota = value
            .get("CPUQuota")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        ananicy_platform::cgroups::create_cgroup(&name.0, quota);
    }
    true
}

fn start_manual_scanner(
    config: Arc<Config>,
    tx: std::sync::mpsc::Sender<Process>,
    shutdown_flag: Arc<AtomicBool>,
) {
    ananicy_core::spawn_named_thread!("ananicy-scan", move || {
        let freq = config.get().check_freq;
        let check_freq = if freq > 0 { freq } else { 60 };
        let mut last_scan = Instant::now();

        while !shutdown_flag.load(std::sync::atomic::Ordering::SeqCst) {
            thread::sleep(Duration::from_secs(1));
            if last_scan.elapsed().as_secs() >= check_freq as u64 {
                tracing::info!("Running periodic manual procfs scan");
                ananicy_platform::procfs::ProcfsScanner::full_scan(tx.clone());
                last_scan = Instant::now();
            }
        }
    });
}
