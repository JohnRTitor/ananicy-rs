#![allow(clippy::collapsible_if)]
use {
    ananicy_core::{process::Process, worker::Worker},
    ananicy_platform::procfs::ProcfsScanner,
    cli::{Args, Commands},
    std::{
        sync::{Arc, RwLock, mpsc},
        thread,
    },
    tracing::{error, info, warn},
};

mod cli;

struct IpcSingletonGuard;

impl Drop for IpcSingletonGuard {
    fn drop(&mut self) {
        let _ = rustix::shm::unlink("/AnanicyRsMutex");
    }
}

fn check_ipc_singleton(force_remove: bool) -> Result<IpcSingletonGuard, String> {
    use {
        rustix::{
            fs::Mode,
            shm::{self, OFlags as ShmOFlags},
        },
        std::io::{Read, Write},
    };

    let name = "/AnanicyRsMutex";

    if force_remove {
        let _ = shm::unlink(name);
    }

    match shm::open(name, ShmOFlags::RDWR, Mode::empty()) {
        Ok(fd) => {
            let mut file = std::fs::File::from(fd);
            let mut buf = String::new();
            if file.read_to_string(&mut buf).is_ok() {
                if let Ok(old_pid) = buf.trim().parse::<i32>() {
                    return Err(format!("Another instance is running (PID: {})", old_pid));
                }
            }
            return Err("Another instance is running (PID: unknown)".to_string());
        }
        Err(rustix::io::Errno::NOENT) => {
            let fd = shm::open(
                name,
                ShmOFlags::CREATE | ShmOFlags::EXCL | ShmOFlags::RDWR,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(|e| e.to_string())?;
            let mut file = std::fs::File::from(fd);
            let _ = write!(file, "{}", std::process::id());
        }
        Err(e) => return Err(format!("Failed to open shm: {}", e)),
    }
    Ok(IpcSingletonGuard)
}

fn main() {
    let args = Args::parse();

    let log_level = if args.verbose {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };

    let is_systemd = args.systemd || std::env::var("NOTIFY_SOCKET").is_ok();

    if is_systemd && tracing_journald::layer().is_ok() {
        use tracing_subscriber::layer::SubscriberExt;
        let layer = tracing_journald::layer().unwrap();
        let subscriber = tracing_subscriber::Registry::default()
            .with(tracing_subscriber::filter::LevelFilter::from_level(
                log_level,
            ))
            .with(layer);
        let _ = tracing::subscriber::set_global_default(subscriber);
    } else {
        let subscriber = tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(log_level)
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
    }

    let config_path = std::env::var("ANANICY_RS_CONF").unwrap_or_else(|_| {
        args.config
            .clone()
            .unwrap_or_else(|| "/etc/ananicy.d/ananicy.conf".to_string())
    });

    let config_dir_path = std::env::var("ANANICY_RS_CONFDIR").unwrap_or_else(|_| {
        args.config_dir
            .clone()
            .unwrap_or_else(|| "/etc/ananicy.d".to_string())
    });

    if args.reload {
        use {
            rustix::{
                fs::Mode,
                shm::{self, OFlags as ShmOFlags},
            },
            std::io::Read,
        };

        match shm::open("/AnanicyRsMutex", ShmOFlags::RDONLY, Mode::empty()) {
            Ok(fd) => {
                let mut file = std::fs::File::from(fd);
                let mut buf = String::new();
                if file.read_to_string(&mut buf).is_ok() {
                    if let Ok(old_pid) = buf.trim().parse::<i32>() {
                        if let Some(pid) = rustix::process::Pid::from_raw(old_pid) {
                            if let Err(e) =
                                rustix::process::kill_process(pid, rustix::process::Signal::USR1)
                            {
                                eprintln!("Failed to send reload signal: {}", e);
                                std::process::exit(1);
                            } else {
                                println!("Reload signal sent to PID {}", old_pid);
                                std::process::exit(0);
                            }
                        }
                    }
                }
                eprintln!("Unable to read PID from IPC singleton");
                std::process::exit(1);
            }
            Err(_) => {
                eprintln!("Unable to reload. Ananicy is not running!");
                std::process::exit(1);
            }
        }
    }

    // Defer IPC guard and root check to later.

    if args.daemon && !is_systemd {
        // Basic daemonize logic would go here if we used a crate like `daemonize`
        // Since we didn't add it, we just warn for now.
        warn!("Daemon mode requested but not fully implemented. Running in foreground.");
    }

    // Create the central event channel
    let (tx, rx) = mpsc::channel::<Process>();

    // Load configuration and rules
    let latnice_supported = ananicy_platform::test_latnice_support();
    let config = match ananicy_core::config::Config::load_file(&config_path, latnice_supported) {
        Ok(cfg) => {
            let snap = cfg.get();
            info!("Config apply_nice: {}", snap.apply_nice);
            info!("Config apply_sched: {}", snap.apply_sched);
            info!("Config cgroup_load: {}", snap.cgroup_load);
            info!("Config apply_oom_score_adj: {}", snap.apply_oom_score_adj);
            info!("Config apply_latnice: {}", snap.apply_latnice);
            info!("Config log_applied_rule: {}", snap.log_applied_rule);
            info!("Config type_load: {}", snap.type_load);
            info!("Config rule_load: {}", snap.rule_load);
            info!("Config cgroup_realtime_workaround: {}", snap.cgroup_realtime_workaround);
            info!("Config check_freq: {}", snap.check_freq);
            info!("Config apply_cpuset: {}", snap.apply_cpuset);
            info!("Config apply_ionice: {}", snap.apply_ionice);
            info!("Config x3d_mode: {}", snap.x3d_mode);
            info!("Config loglevel: {}", snap.loglevel);
            Arc::new(cfg)
        }
        Err(e) => {
            error!(
                "Failed to load config from {}: {}. Using default.",
                config_path, e
            );
            let mut snapshot = ananicy_core::config::ConfigSnapshot::default();
            if !latnice_supported {
                snapshot.apply_latnice = false;
                warn!("latency_nice is not supported by the kernel, disabling it");
            }
            Arc::new(ananicy_core::config::Config::new(snapshot))
        }
    };

    // Setup aliases with topology and X3D
    let mut aliases = ananicy_platform::topology::detect_topology().generate_cpuset_aliases();

    // Detect AMD X3D topology
    let mut saved_x3d_mode = None;
    if let Some(x3d_top) = ananicy_platform::x3d::detect_x3d_topology() {
        info!(
            "AMD X3D detected: cache cores={}, frequency cores={}",
            x3d_top.cache_cores_str, x3d_top.frequency_cores_str
        );
        aliases.insert("x3d-cache".to_string(), x3d_top.cache_cores_str);
        aliases.insert("x3d-frequency".to_string(), x3d_top.frequency_cores_str);

        // Apply X3D driver mode if configured
        let x3d_mode_str = config.get().x3d_mode.clone();
        if x3d_mode_str != "auto" {
            saved_x3d_mode = ananicy_platform::x3d::get_driver_mode();
            if saved_x3d_mode.is_some() {
                let target = if x3d_mode_str == "cache" {
                    ananicy_platform::x3d::X3DMode::Cache
                } else {
                    ananicy_platform::x3d::X3DMode::Frequency
                };
                if ananicy_platform::x3d::set_driver_mode(target) {
                    info!("Set X3D mode to '{}'", x3d_mode_str);
                } else {
                    warn!("Failed to set X3D mode to '{}'", x3d_mode_str);
                    saved_x3d_mode = None;
                }
            } else {
                info!("X3D driver not present, x3d_mode config ignored");
            }
        }
    }

    let mut rules_obj = ananicy_core::rules::Rules::new(config.clone());
    rules_obj.load_directory(&config_dir_path);

    // Process dump commands
    if let Some(Commands::Dump { sub_action }) = &args.command {
        match sub_action {
            cli::DumpTarget::Rules => {
                println!("Loaded Rules: {:?}", rules_obj.get_rules());
            }
            cli::DumpTarget::Types => {
                println!("Loaded Types: {:?}", rules_obj.get_types());
            }
            cli::DumpTarget::Cgroups => {
                println!("Loaded Cgroups: {:?}", rules_obj.get_cgroups());
            }
            cli::DumpTarget::Proc => {
                let (tx_dump, rx_dump) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    ProcfsScanner::full_scan(tx_dump);
                });
                println!("{:<10} {:<10} {:<20} {:<20}", "PID", "TID", "NAME", "RULES");
                while let Ok(p) = rx_dump.recv() {
                    let pid = p.identity.pid.0;
                    let rule = rules_obj.get_rule(&p.name);
                    let rule_name = rule
                        .as_ref()
                        .and_then(|r| r.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    
                    if let Ok(entries) = std::fs::read_dir(format!("/proc/{}/task", pid)) {
                        for entry in entries.flatten() {
                            if let Ok(file_name) = entry.file_name().into_string() {
                                if let Ok(tid) = file_name.parse::<i32>() {
                                    println!("{:<10} {:<10} {:<20} {}", pid, tid, p.name, rule_name);
                                }
                            }
                        }
                    } else {
                        // fallback if tasks can't be read
                        println!("{:<10} {:<10} {:<20} {}", pid, pid, p.name, rule_name);
                    }
                }
            }
            cli::DumpTarget::Autogroup => {
                if let Ok(content) =
                    std::fs::read_to_string("/proc/sys/kernel/sched_autogroup_enabled")
                {
                    println!("Autogroup enabled: {}", content.trim());
                } else {
                    println!(
                        "Autogroup status unknown (failed to read /proc/sys/kernel/sched_autogroup_enabled)"
                    );
                }
            }
        }
        return;
    }

    match &args.command {
        Some(Commands::Start) => {
            info!("Starting ananicy-rs daemon");
        }
        Some(Commands::Unknown(action)) => {
            error!("Unknown action requested: {}", action);
        }
        _ => return,
    }

    if rustix::process::geteuid().as_raw() != 0 {
        error!("This program must be run as root");
        std::process::exit(1);
    }

    let _ipc_guard = match check_ipc_singleton(args.force_remove_semaphore) {
        Ok(guard) => guard,
        Err(e) => {
            error!("IPC Singleton check failed: {}", e);
            std::process::exit(1);
        }
    };

    if args.force_remove_semaphore {
        info!("Force removed IPC semaphore. Exiting.");
        std::process::exit(0);
    }

    let rules = Arc::new(RwLock::new(rules_obj));
    let platform = Arc::new(ananicy_platform::LinuxPlatform::new());

    // Setup signal handling
    let config_clone_sig = config.clone();
    let config_path_sig = config_path.clone();
    let is_systemd_sig = is_systemd;
    let shutdown_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let shutdown_flag_clone = shutdown_flag.clone();
    let tx_clone_sig = tx.clone();

    if let Ok(mut signals) = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGUSR1,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
    ]) {
        thread::spawn(move || {
            for sig in signals.forever() {
                if sig == signal_hook::consts::SIGUSR1 {
                    info!("Received SIGUSR1, reloading config...");
                    let latnice_supported = ananicy_platform::test_latnice_support();
                    if let Err(e) =
                        config_clone_sig.reload_file(&config_path_sig, latnice_supported)
                    {
                        error!("Failed to reload config: {}", e);
                    }
                    info!("Config reloaded");
                } else if sig == signal_hook::consts::SIGINT || sig == signal_hook::consts::SIGTERM
                {
                    info!("Received termination signal. Shutting down...");
                    shutdown_flag_clone.store(true, std::sync::atomic::Ordering::SeqCst);

                    // We must drop our local tx_clone_sig so that the channel can be fully closed
                    // when the main thread also drops its tx senders.
                    drop(tx_clone_sig);

                    if is_systemd_sig {
                        let _ = sd_notify::notify(&[sd_notify::NotifyState::Stopping]);
                    }
                    break;
                }
            }
        });
    }

    // No startup retry loop for cgroups, to match reference zero-wait behavior
    // just let cgroup initialization proceed.

    // Create cgroups
    info!("Initializing cgroups based on rules");
    for (name, value) in rules.read().unwrap().get_cgroups() {
        let quota = value
            .get("CPUQuota")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        ananicy_platform::cgroups::create_cgroup(&name.0, quota);
    }

    // Spawn the Worker thread
    info!("Spawning worker thread");
    let worker = Worker::new(config.clone(), rules.clone(), platform, aliases, rx);
    let worker_handle = worker.start();

    if is_systemd {
        info!("Notifying systemd of readiness...");
        let _ = sd_notify::notify(&[sd_notify::NotifyState::Ready]);
    }

    if config.get().cgroup_realtime_workaround {
        thread::sleep(std::time::Duration::from_millis(100));
        // Force redetect version and recreate
        ananicy_platform::mounts::reset_cgroup_info();
        
        for (name, value) in rules.read().unwrap().get_cgroups() {
            let quota = value
                .get("CPUQuota")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
            ananicy_platform::cgroups::create_cgroup(&name.0, quota);
        }
    }

    if args.manual_scanning {
        let tx_scan = tx.clone();
        let shutdown_scan = shutdown_flag.clone();
        let config_clone = config.clone();
        thread::spawn(move || {
            let freq = config_clone.get().check_freq;
            let check_freq = if freq > 0 { freq } else { 60 };
            let mut last_scan = std::time::Instant::now();
            
            while !shutdown_scan.load(std::sync::atomic::Ordering::SeqCst) {
                thread::sleep(std::time::Duration::from_secs(1));
                if last_scan.elapsed().as_secs() >= check_freq as u64 {
                    info!("Running periodic manual procfs scan");
                    ProcfsScanner::full_scan(tx_scan.clone());
                    last_scan = std::time::Instant::now();
                }
            }
        });
    }

    // Start event monitor (BPF or Netlink)
    // We try BPF first if the feature is enabled
    #[cfg(feature = "ananicy-bpf")]
    {
        use ananicy_bpf::BpfMonitor;
        info!("Attempting to start BPF monitor...");
        let bpf_min_us = args.bpf_min_us;
        loop {
            match BpfMonitor::new(bpf_min_us) {
                Ok(mut bpf) => {
                    info!("BPF monitor successfully started.");
                    let tx_clone = tx.clone();
                    let tx_scan = tx.clone();

                    info!("Running initial procfs full scan");
                    thread::spawn(move || {
                        ProcfsScanner::full_scan(tx_scan);
                    });

                    // Block the main thread with BPF listening
                    bpf.listen(tx_clone, shutdown_flag.clone());

                    if shutdown_flag.load(std::sync::atomic::Ordering::SeqCst) {
                        // Clean shutdown
                        drop(tx);
                        match worker_handle.join() {
                            Ok((count, duration)) => {
                                info!("Worker processed {} events in {:?}", count, duration)
                            }
                            Err(e) => error!("Worker thread panicked: {:?}", e),
                        }
                        if let Some(mode) = saved_x3d_mode {
                            if ananicy_platform::x3d::set_driver_mode(mode) {
                                info!("Restored X3D mode on shutdown");
                            }
                        }
                        return;
                    } else {
                        warn!("BPF monitor exited unexpectedly, restarting in 1 second...");
                        thread::sleep(std::time::Duration::from_secs(1));
                        continue;
                    }
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

    // Fallback to Netlink
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
                        thread::spawn(move || {
                            ProcfsScanner::full_scan(tx_scan);
                        });
                    }

                    if let Err(e) = nl.listen(tx_clone.clone(), shutdown_flag.clone()) {
                        warn!(
                            "Netlink error: {}. Triggering full scan recovery and reconnect...", e
                        );
                        ProcfsScanner::full_scan(tx_clone.clone());
                        std::thread::sleep(std::time::Duration::from_secs(1));
                        continue; // Re-create monitor and listen
                    }
                    // After netlink loop breaks, drop tx and wait for worker
                    drop(tx);
                    drop(tx_clone);
                    match worker_handle.join() {
                        Ok((count, duration)) => {
                            info!("Worker processed {} events in {:?}", count, duration)
                        }
                        Err(e) => error!("Worker thread panicked: {:?}", e),
                    }
                    if let Some(mode) = saved_x3d_mode {
                        if ananicy_platform::x3d::set_driver_mode(mode) {
                            info!("Restored X3D mode on shutdown");
                        }
                    }
                    break;
                }
                Err(e) => {
                    error!("Failed to start Netlink monitor: {}. Exiting.", e);
                    if let Some(mode) = saved_x3d_mode {
                        if ananicy_platform::x3d::set_driver_mode(mode) {
                            info!("Restored X3D mode on netlink monitor failure");
                        }
                    }
                    std::process::exit(1);
                }
            }
        }
    }

    #[cfg(not(feature = "netlink"))]
    {
        error!("No event monitor available. Exiting.");
        if let Some(mode) = saved_x3d_mode {
            if ananicy_platform::x3d::set_driver_mode(mode) {
                info!("Restored X3D mode on startup failure");
            }
        }
        std::process::exit(1);
    }
}
