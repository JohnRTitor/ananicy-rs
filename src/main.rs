#[cfg(not(any(feature = "bpf", feature = "netlink")))]
compile_error!("At least one event source feature ('bpf' or 'netlink') must be enabled.");

use {
    ananicy_core::process::Process,
    ananicy_platform::LinuxPlatform,
    std::process::{exit, id},
    tracing::{debug, info},
};

use {
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
mod systemd;

fn main() {
    let args = Args::parse();
    let systemd_mode = systemd::resolve(args.systemd, systemd::SystemdEnvironment::from_process());
    // The systemd integration (sd_notify, journald logging) is only linked in
    // when the `systemd` cargo feature is enabled.
    let systemd_supported = cfg!(feature = "systemd");
    let is_systemd = systemd_supported && systemd_mode.is_enabled();
    let systemd_status = if systemd_supported {
        systemd_mode.description()
    } else {
        "disabled (built without the `systemd` feature)"
    };
    // Force trace-level logging for the whole `debug` action before
    // dispatching to a sub-action so the debug module's diagnostics
    // are actually emitted.
    let force_trace = matches!(args.command, Some(Commands::Debug { .. }));

    let (config_path, config_dir_path) = startup::resolve_config_paths(&args);

    let (config, config_err, config_diagnostics, latnice_supported) =
        startup::load_config(&config_path);

    let log_reload_handle = match startup::init_logging(
        config.get().loglevel.clone(),
        args.verbose,
        force_trace,
        is_systemd,
    ) {
        Ok(handle) => handle,
        Err(e) => {
            eprintln!("Failed to initialize logging: {}", e);
            exit(1);
        }
    };
    let log_level_override = startup::log_level_override(args.verbose, force_trace);

    debug!("Systemd integration: {}", systemd_status);

    startup::log_config(&config, config_err, &config_diagnostics, latnice_supported);

    if args.force_remove_semaphore {
        ipc::force_remove_semaphore();
    }

    if args.reload {
        ipc::request_reload();
    }

    if args.daemon && !is_systemd {
        warn!("Daemon mode requested but not fully implemented. Running in foreground.");
    }

    info!("Ananicy Rs {}", env!("CARGO_PKG_VERSION"));

    let (aliases, saved_x3d_mode) = startup::load_topology_aliases(&config);
    let rules_obj = startup::load_rules(config.clone(), &config_dir_path);

    if let Some(Commands::Dump { sub_action }) = &args.command {
        dump::run(sub_action, &rules_obj);
        return;
    }

    // Like `dump`, the `debug` action runs after config/rules
    // initialization but exits before the root check and daemon startup.
    if let Some(Commands::Debug { sub_action }) = &args.command {
        debug::run(sub_action, systemd_status);
        return;
    }

    match &args.command {
        Some(Commands::Start) => info!("Starting ananicy-rs daemon"),
        Some(Commands::Unknown(action)) => {
            error!("Unknown action requested: {}", action);
            exit(1);
        }
        _ => return,
    }

    if rustix::process::getuid().as_raw() != 0 {
        error!("This program must be run as root");
        exit(1);
    }

    let _ipc_guard = match ipc::check_singleton() {
        Ok(guard) => guard,
        Err(e) => {
            error!("IPC Singleton check failed: {}", e);
            exit(1);
        }
    };

    let (tx, rx) = mpsc::channel::<Process>();
    let rules = Arc::new(rules_obj);
    let platform = Arc::new(LinuxPlatform::new());
    let shutdown_flag = Arc::new(AtomicBool::new(false));

    signals::install(
        config.clone(),
        config_path,
        is_systemd,
        shutdown_flag.clone(),
        tx.clone(),
        log_reload_handle,
        log_level_override,
    );

    if args.manual_scanning {
        info!("Manual scanning enabled! Increasing Ananicy Nice value to prevent lag.");
        let _ = ananicy_platform::priority::set_priority(id() as i32, &[], 19);
        info!("Checking frequency set to {}", config.get().check_freq);
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
