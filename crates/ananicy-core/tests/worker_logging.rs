//! How the worker reports the outcome of a rule application.
//!
//! The rule-application contract itself is covered by `worker_rules.rs`; this
//! file pins what the daemon *tells the operator*, which is the part users
//! debug with. The rules engine is driven through a recording platform, so no
//! process is ever touched.

mod common;

use {
    ananicy_core::{
        config::{ConfigSnapshot, LogLevel},
        worker::PlatformError,
    },
    common::{FakePlatform, capture_events, run_worker, snapshot},
    std::{sync::atomic::Ordering, thread},
    tracing::{Level, info, warn},
    tracing_subscriber::filter::LevelFilter,
};

#[test]
fn successful_application_logs_when_enabled() {
    let run = run_worker(
        snapshot(true),
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::new(),
    );

    assert!(run.events.contains(Level::INFO, "worker-test(42)"));
}

#[test]
fn missing_optional_cpu_weight_does_not_hide_successful_nice() {
    // The cgroup-v2 cpu.weight mirroring is a best-effort extra: if the kernel
    // refuses it, the successful `nice` must still be reported as a success.
    let run = run_worker(
        snapshot(true),
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::cgroup_v2().failing("set_cpu_weight", PlatformError::Unsupported),
    );

    assert!(run.events.contains(Level::INFO, "worker-test(42)"));
    assert!(!run.events.contains(Level::WARN, "partially failed"));
}

#[test]
fn successful_application_is_silent_when_disabled() {
    let run = run_worker(
        snapshot(false),
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::new(),
    );

    assert!(!run.events.contains(Level::INFO, "worker-test(42)"));
}

#[test]
fn configured_warn_level_suppresses_applied_rule_event() {
    let mut config = snapshot(true);
    config.loglevel = LogLevel::Warn;
    let run = run_worker(
        config,
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::new(),
    );

    assert!(!run.events.contains(Level::INFO, "worker-test(42)"));
}

#[test]
fn configured_error_level_allows_failure_diagnostics() {
    let mut config = snapshot(true);
    config.loglevel = LogLevel::Error;
    let run = run_worker(
        config,
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::new().failing("set_priority", PlatformError::Unsupported),
    );

    assert!(!run.events.contains(Level::INFO, "worker-test(42)"));
    assert!(run.events.contains(Level::ERROR, "Failed to apply rule"));
}

#[test]
fn config_reload_changes_applied_rule_logging_for_next_event() {
    // The reload happens on another thread while the worker is running, so this
    // also covers the work_loop reading a fresh snapshot per process.
    use {
        ananicy_core::{process::Process, rules::Rules, types::Pid, worker::Worker},
        std::{
            collections::HashMap,
            fs,
            sync::{Arc, mpsc},
        },
    };

    let (directory, config) = common::load_config("log_applied_rule = false\nloglevel = info\n");
    let config_path = directory.path().join("ananicy.conf");

    let mut rules = Rules::new(config.clone());
    assert!(rules.load_rule_from_string(r#"{"name":"before-reload","nice":5}"#));
    assert!(rules.load_rule_from_string(r#"{"name":"after-reload","nice":5}"#));

    let platform = FakePlatform::new();
    let calls = platform.priority_call_counter();

    let (tx, rx) = mpsc::channel();
    tx.send(Process::new(Pid(41), "before-reload".to_string()).with_authoritative_name())
        .unwrap();
    let reload_tx = tx.clone();
    drop(tx);

    let reload_config = config.clone();
    let reload_thread = thread::spawn(move || {
        // Wait for the worker to have applied the first rule, so the reload is
        // guaranteed to happen between the two processes. `wait_until` gives up
        // instead of hanging the suite if that never happens.
        common::wait_until(|| calls.load(Ordering::SeqCst) > 0);
        fs::write(&config_path, "log_applied_rule = true\nloglevel = info\n").unwrap();
        reload_config.reload_file(&config_path, true).unwrap();
        reload_tx
            .send(Process::new(Pid(42), "after-reload".to_string()).with_authoritative_name())
            .unwrap();
    });

    let events = capture_events(LevelFilter::INFO, || {
        Worker::new(
            config,
            Arc::new(rules),
            Arc::new(platform),
            HashMap::new(),
            rx,
            None,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .work_loop();
    });
    reload_thread.join().unwrap();

    assert!(!events.contains(Level::INFO, "before-reload(41)"));
    assert!(events.contains(Level::INFO, "after-reload(42)"));
}

#[test]
fn failed_application_does_not_log_success() {
    let run = run_worker(
        snapshot(true),
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::new().failing("set_priority", PlatformError::Unsupported),
    );

    assert!(!run.events.contains(Level::INFO, "worker-test(42)"));
    assert!(run.events.contains(Level::ERROR, "Failed to apply rule"));
}

#[test]
fn skipped_attribute_does_not_log_success() {
    let run = run_worker(
        snapshot(true),
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::new().failing("set_priority", PlatformError::PermissionDenied),
    );

    assert!(!run.events.contains(Level::INFO, "worker-test(42)"));
    assert!(run.events.contains(Level::WARN, "partially failed"));
}

#[test]
fn skipped_platform_operation_does_not_log_success() {
    let run = run_worker(
        snapshot(true),
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::new().failing(
            "set_priority",
            PlatformError::Skipped("deadline scheduler is unavailable".to_string()),
        ),
    );

    assert!(!run.events.contains(Level::INFO, "worker-test(42)"));
    assert!(run.events.contains(Level::WARN, "partially failed"));
}

#[test]
fn empty_thread_list_does_not_log_success() {
    // Without a thread list the rule can only be applied to the main thread, so
    // the daemon must not claim success.
    let run = run_worker(
        snapshot(true),
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::with_thread_list(Vec::new()),
    );

    assert!(!run.events.contains(Level::INFO, "worker-test(42)"));
    assert!(
        run.events
            .contains(Level::WARN, "Failed to enumerate threads")
    );
}

#[test]
fn rule_without_enabled_attributes_does_not_log_success() {
    let run = run_worker(
        snapshot(true),
        r#"{"name":"worker-test"}"#,
        FakePlatform::new(),
    );

    assert!(!run.events.contains(Level::INFO, "worker-test(42)"));
}

#[test]
fn debug_logging_does_not_suppress_applied_rule_events() {
    let mut config = snapshot(true);
    config.loglevel = LogLevel::Debug;
    let run = run_worker(
        config,
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::new(),
    );

    assert!(run.events.contains(Level::DEBUG, "Found rule"));
    assert!(run.events.contains(Level::INFO, "worker-test(42)"));
}

#[test]
fn level_filter_suppresses_info_but_allows_warn() {
    let captured = capture_events(LevelFilter::WARN, || {
        info!("lower severity");
        warn!("higher severity");
    });

    assert!(!captured.contains(Level::INFO, "lower severity"));
    assert!(captured.contains(Level::WARN, "higher severity"));
}

/// The configured level has to reach the subscriber the worker logs through.
#[test]
fn the_configured_level_drives_the_subscriber() {
    let mut config: ConfigSnapshot = snapshot(true);
    config.loglevel = LogLevel::Error;

    let run = run_worker(
        config,
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::new().failing("set_priority", PlatformError::Unsupported),
    );

    assert!(
        run.events.contains(Level::ERROR, "Failed to apply rule"),
        "an error must still be reported at the error threshold"
    );
}
