#![allow(clippy::collapsible_if)]

use {
    ananicy_core::process::Process,
    cli::{Args, Commands},
    std::sync::{Arc, RwLock, atomic::AtomicBool, mpsc},
    tracing::{error, warn},
};

mod cli;
mod dump;
mod ipc;
mod monitor;
mod runtime;
mod signals;
mod startup;

fn main() {
    let args = Args::parse();
    let is_systemd = args.systemd || std::env::var("NOTIFY_SOCKET").is_ok();
    startup::init_logging(args.verbose, is_systemd);

    let (config_path, config_dir_path) = startup::resolve_config_paths(&args);

    if args.reload {
        ipc::request_reload();
    }

    if args.daemon && !is_systemd {
        warn!("Daemon mode requested but not fully implemented. Running in foreground.");
    }

    let config = startup::load_config(&config_path);
    let (aliases, saved_x3d_mode) = startup::load_topology_aliases(&config);
    let rules_obj = startup::load_rules(config.clone(), &config_dir_path);

    if let Some(Commands::Dump { sub_action }) = &args.command {
        dump::run(sub_action, &rules_obj);
        return;
    }

    match &args.command {
        Some(Commands::Start) => tracing::info!("Starting ananicy-rs daemon"),
        Some(Commands::Unknown(action)) => {
            error!("Unknown action requested: {}", action);
        }
        _ => return,
    }

    if rustix::process::geteuid().as_raw() != 0 {
        error!("This program must be run as root");
        std::process::exit(1);
    }

    let _ipc_guard = match ipc::check_singleton(args.force_remove_semaphore) {
        Ok(guard) => guard,
        Err(e) => {
            error!("IPC Singleton check failed: {}", e);
            std::process::exit(1);
        }
    };

    let (tx, rx) = mpsc::channel::<Process>();
    let rules = Arc::new(RwLock::new(rules_obj));
    let platform = Arc::new(ananicy_platform::LinuxPlatform::new());
    let shutdown_flag = Arc::new(AtomicBool::new(false));

    signals::install(
        config.clone(),
        config_path,
        is_systemd,
        shutdown_flag.clone(),
        tx.clone(),
    );

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
    );
}
