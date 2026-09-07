#![allow(clippy::collapsible_if)]

#[cfg(not(any(feature = "bpf", feature = "netlink")))]
compile_error!("At least one event source feature ('bpf' or 'netlink') must be enabled.");

use {
    ananicy_core::process::Process,
    cli::{Args, Commands},
    std::sync::{Arc, atomic::AtomicBool, mpsc},
    tracing::{error, warn},
};

mod cli;
mod debug;
mod dump;
mod ipc;
mod monitor;
mod runtime;
mod signals;
mod startup;

fn main() {
    let args = Args::parse();
    #[cfg(feature = "systemd")]
    let is_systemd = args.systemd || std::env::var("NOTIFY_SOCKET").is_ok();
    #[cfg(not(feature = "systemd"))]
    let is_systemd = false;
    // Force trace-level logging for the whole `debug` action before
    // dispatching to a sub-action so the debug module's diagnostics
    // are actually emitted.
    let force_trace = matches!(args.command, Some(Commands::Debug { .. }));
    startup::init_logging(args.verbose, force_trace, is_systemd);

    let (config_path, config_dir_path) = startup::resolve_config_paths(&args);

    if args.force_remove_semaphore {
        ipc::force_remove_semaphore();
    }

    if args.reload {
        ipc::request_reload();
    }

    if args.daemon && !is_systemd {
        warn!("Daemon mode requested but not fully implemented. Running in foreground.");
    }

    let config = startup::load_config(&config_path);
    println!("Ananicy Rs {}", env!("CARGO_PKG_VERSION"));

    let (aliases, saved_x3d_mode) = startup::load_topology_aliases(&config);
    let rules_obj = startup::load_rules(config.clone(), &config_dir_path);

    if let Some(Commands::Dump { sub_action }) = &args.command {
        dump::run(sub_action, &rules_obj);
        return;
    }

    // Like `dump`, the `debug` action runs after config/rules
    // initialization but exits before the root check and daemon startup.
    if let Some(Commands::Debug { sub_action }) = &args.command {
        debug::run(sub_action);
        return;
    }

    match &args.command {
        Some(Commands::Start) => tracing::info!("Starting ananicy-rs daemon"),
        Some(Commands::Unknown(action)) => {
            error!("Unknown action requested: {}", action);
        }
        _ => return,
    }

    if rustix::process::getuid().as_raw() != 0 {
        error!("This program must be run as root");
        std::process::exit(1);
    }

    let _ipc_guard = match ipc::check_singleton() {
        Ok(guard) => guard,
        Err(e) => {
            error!("IPC Singleton check failed: {}", e);
            std::process::exit(1);
        }
    };

    let (tx, rx) = mpsc::channel::<Process>();
    let rules = Arc::new(rules_obj);
    let platform = Arc::new(ananicy_platform::LinuxPlatform::new());
    let shutdown_flag = Arc::new(AtomicBool::new(false));

    signals::install(
        config.clone(),
        config_path,
        is_systemd,
        shutdown_flag.clone(),
        tx.clone(),
    );

    if args.manual_scanning {
        tracing::info!("Manual scanning enabled! Increasing Ananicy Nice value to prevent lag.");
        let _ = ananicy_platform::priority::set_priority(std::process::id() as i32, 19);
        tracing::info!("Checking frequency set to {}", config.get().check_freq);
    }

    runtime::run(
        config.clone(),
        rules,
        platform,
        aliases,
        rx,
        tx,
        shutdown_flag,
        args.manual_scanning,
        config.get().cgroup_realtime_workaround,
        args.bpf_min_us,
        is_systemd,
        saved_x3d_mode,
        args.benchmark,
        args.benchmark_count,
    );
}
