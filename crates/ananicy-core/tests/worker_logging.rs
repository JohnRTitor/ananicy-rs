use {
    ananicy_core::{
        config::{Config, ConfigSnapshot, LogLevel},
        cpuset::CpuSet,
        process::Process,
        rules::Rules,
        types::Pid,
        worker::{PlatformActions, PlatformError, Worker},
    },
    std::{
        collections::HashMap,
        fmt, fs,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc,
        },
    },
    std::{thread, time::Duration},
    tracing::{Event, Level, Subscriber, info, warn},
    tracing_subscriber::{
        filter::LevelFilter,
        layer::{Context, Layer},
        prelude::*,
    },
};

#[derive(Clone, Default)]
struct CapturedEvents(Arc<Mutex<Vec<(Level, String)>>>);

struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{:?}", value);
        }
    }
}

impl<S> Layer<S> for CapturedEvents
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        self.0
            .lock()
            .unwrap()
            .push((*event.metadata().level(), visitor.0));
    }
}

impl CapturedEvents {
    fn contains(&self, level: Level, text: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .iter()
            .any(|(event_level, message)| *event_level == level && message.contains(text))
    }
}

struct FakePlatform {
    priority_result: Mutex<Option<Result<(), PlatformError>>>,
    priority_calls: Arc<AtomicUsize>,
    tids: Option<Vec<i32>>,
    cgroup_v2: bool,
    cpu_weight_result: Mutex<Option<Result<(), PlatformError>>>,
}

impl FakePlatform {
    fn new(priority_result: Option<Result<(), PlatformError>>) -> Self {
        Self {
            priority_result: Mutex::new(priority_result),
            priority_calls: Arc::new(AtomicUsize::new(0)),
            tids: None,
            cgroup_v2: false,
            cpu_weight_result: Mutex::new(None),
        }
    }

    fn with_empty_tids() -> Self {
        Self {
            tids: Some(Vec::new()),
            ..Self::new(None)
        }
    }

    fn with_cgroup_v2_cpu_weight_failure() -> Self {
        Self {
            cgroup_v2: true,
            cpu_weight_result: Mutex::new(Some(Err(PlatformError::Unsupported))),
            ..Self::new(None)
        }
    }
}

impl PlatformActions for FakePlatform {
    fn is_realtime(&self, _pid: i32) -> bool {
        false
    }

    fn get_start_time(&self, _pid: i32) -> Option<u64> {
        Some(1)
    }

    fn get_process_name(&self, _pid: i32) -> String {
        "worker-test".to_string()
    }

    fn is_cgroup_v2(&self) -> bool {
        self.cgroup_v2
    }

    fn get_max_cores(&self) -> u32 {
        1
    }

    fn get_tids(&self, pid: i32) -> Result<Vec<i32>, PlatformError> {
        Ok(self.tids.clone().unwrap_or_else(|| vec![pid]))
    }

    fn set_priority(&self, _pid: i32, _tids: &[i32], _nice: i32) -> Result<(), PlatformError> {
        self.priority_calls.fetch_add(1, Ordering::SeqCst);
        self.priority_result
            .lock()
            .unwrap()
            .take()
            .unwrap_or(Ok(()))
    }

    fn set_latency_nice(
        &self,
        _pid: i32,
        _tids: &[i32],
        _lat_nice: i32,
    ) -> Result<(), PlatformError> {
        Ok(())
    }

    fn set_sched(&self, _pid: i32, _sched: &str, _rtprio: u32) -> Result<(), PlatformError> {
        Ok(())
    }

    fn set_io_priority(
        &self,
        _pid: i32,
        _ioclass: &str,
        _ionice: i32,
    ) -> Result<(), PlatformError> {
        Ok(())
    }

    fn set_oom_score_adj(&self, _pid: i32, _oom_score_adj: i32) -> Result<(), PlatformError> {
        Ok(())
    }

    fn add_pid_to_cgroup(&self, _pid: i32, _cgroup: &str) -> Result<(), PlatformError> {
        Ok(())
    }

    fn set_cpu_weight(&self, _pid: i32, _weight: u32) -> Result<(), PlatformError> {
        self.cpu_weight_result
            .lock()
            .unwrap()
            .take()
            .unwrap_or(Ok(()))
    }

    fn set_affinity(
        &self,
        _pid: i32,
        _tids: &[i32],
        _cpuset: &CpuSet,
    ) -> Result<(), PlatformError> {
        Ok(())
    }
}

fn run_worker(
    snapshot: ConfigSnapshot,
    rule: &str,
    platform: FakePlatform,
    level: Level,
) -> CapturedEvents {
    let config = Arc::new(Config::new(snapshot));
    let mut rules = Rules::new(config.clone());
    assert!(rules.load_rule_from_string(rule));

    let (tx, rx) = mpsc::channel();
    tx.send(Process::new(Pid(42), "worker-test".to_string()).with_authoritative_name())
        .unwrap();
    drop(tx);

    let captured = CapturedEvents::default();
    let subscriber = tracing_subscriber::registry()
        .with(LevelFilter::from_level(level))
        .with(captured.clone());

    tracing::subscriber::with_default(subscriber, || {
        Worker::new(
            config,
            Arc::new(rules),
            Arc::new(platform),
            HashMap::new(),
            rx,
            None,
            Arc::new(AtomicBool::new(false)),
        )
        .work_loop();
    });

    captured
}

fn run_worker_at_configured_level(
    snapshot: ConfigSnapshot,
    rule: &str,
    platform: FakePlatform,
) -> CapturedEvents {
    let level = Level::from(&snapshot.loglevel);
    run_worker(snapshot, rule, platform, level)
}

fn snapshot(log_applied_rule: bool) -> ConfigSnapshot {
    ConfigSnapshot {
        log_applied_rule,
        ..ConfigSnapshot::default()
    }
}

#[test]
fn successful_application_logs_when_enabled() {
    let events = run_worker(
        snapshot(true),
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::new(None),
        Level::INFO,
    );

    assert!(events.contains(Level::INFO, "worker-test(42)"));
}

#[test]
fn missing_optional_cpu_weight_does_not_hide_successful_nice() {
    let events = run_worker(
        snapshot(true),
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::with_cgroup_v2_cpu_weight_failure(),
        Level::INFO,
    );

    assert!(events.contains(Level::INFO, "worker-test(42)"));
    assert!(!events.contains(Level::WARN, "partially failed"));
}

#[test]
fn successful_application_is_silent_when_disabled() {
    let events = run_worker(
        snapshot(false),
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::new(None),
        Level::INFO,
    );

    assert!(!events.contains(Level::INFO, "worker-test(42)"));
}

#[test]
fn configured_warn_level_suppresses_applied_rule_event() {
    let mut config = snapshot(true);
    config.loglevel = LogLevel::Warn;
    let events = run_worker_at_configured_level(
        config,
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::new(None),
    );

    assert!(!events.contains(Level::INFO, "worker-test(42)"));
}

#[test]
fn configured_error_level_allows_failure_diagnostics() {
    let mut config = snapshot(true);
    config.loglevel = LogLevel::Error;
    let events = run_worker_at_configured_level(
        config,
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::new(Some(Err(PlatformError::Unsupported))),
    );

    assert!(!events.contains(Level::INFO, "worker-test(42)"));
    assert!(events.contains(Level::ERROR, "Failed to apply rule"));
}

#[test]
fn config_reload_changes_applied_rule_logging_for_next_event() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ananicy.conf");
    fs::write(&path, "log_applied_rule = false\nloglevel = info\n").unwrap();

    let config = Arc::new(Config::load_file(&path, true).unwrap());
    let mut rules = Rules::new(config.clone());
    assert!(rules.load_rule_from_string(r#"{"name":"before-reload","nice":5}"#));
    assert!(rules.load_rule_from_string(r#"{"name":"after-reload","nice":5}"#));

    let platform = FakePlatform::new(None);
    let calls = platform.priority_calls.clone();
    let (tx, rx) = mpsc::channel();
    tx.send(Process::new(Pid(41), "before-reload".to_string()).with_authoritative_name())
        .unwrap();
    let reload_tx = tx.clone();
    drop(tx);

    let reload_path = path.clone();
    let reload_config = config.clone();
    let reload_thread = thread::spawn(move || {
        while calls.load(Ordering::SeqCst) == 0 {
            thread::sleep(Duration::from_millis(1));
        }
        fs::write(&reload_path, "log_applied_rule = true\nloglevel = info\n").unwrap();
        reload_config.reload_file(&reload_path, true).unwrap();
        reload_tx
            .send(Process::new(Pid(42), "after-reload".to_string()).with_authoritative_name())
            .unwrap();
    });

    let captured = CapturedEvents::default();
    let subscriber = tracing_subscriber::registry()
        .with(LevelFilter::INFO)
        .with(captured.clone());
    tracing::subscriber::with_default(subscriber, || {
        Worker::new(
            config,
            Arc::new(rules),
            Arc::new(platform),
            HashMap::new(),
            rx,
            None,
            Arc::new(AtomicBool::new(false)),
        )
        .work_loop();
    });
    reload_thread.join().unwrap();

    assert!(!captured.contains(Level::INFO, "before-reload(41)"));
    assert!(captured.contains(Level::INFO, "after-reload(42)"));
}

#[test]
fn failed_application_does_not_log_success() {
    let events = run_worker(
        snapshot(true),
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::new(Some(Err(PlatformError::Unsupported))),
        Level::INFO,
    );

    assert!(!events.contains(Level::INFO, "worker-test(42)"));
    assert!(events.contains(Level::ERROR, "Failed to apply rule"));
}

#[test]
fn skipped_attribute_does_not_log_success() {
    let events = run_worker(
        snapshot(true),
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::new(Some(Err(PlatformError::PermissionDenied))),
        Level::INFO,
    );

    assert!(!events.contains(Level::INFO, "worker-test(42)"));
    assert!(events.contains(Level::WARN, "partially failed"));
}

#[test]
fn skipped_platform_operation_does_not_log_success() {
    let events = run_worker(
        snapshot(true),
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::new(Some(Err(PlatformError::Skipped(
            "deadline scheduler is unavailable".to_string(),
        )))),
        Level::INFO,
    );

    assert!(!events.contains(Level::INFO, "worker-test(42)"));
    assert!(events.contains(Level::WARN, "partially failed"));
}

#[test]
fn empty_thread_list_does_not_log_success() {
    let events = run_worker(
        snapshot(true),
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::with_empty_tids(),
        Level::INFO,
    );

    assert!(!events.contains(Level::INFO, "worker-test(42)"));
    assert!(events.contains(Level::WARN, "Failed to enumerate threads"));
}

#[test]
fn rule_without_enabled_attributes_does_not_log_success() {
    let events = run_worker(
        snapshot(true),
        r#"{"name":"worker-test"}"#,
        FakePlatform::new(None),
        Level::INFO,
    );

    assert!(!events.contains(Level::INFO, "worker-test(42)"));
}

#[test]
fn debug_logging_does_not_suppress_applied_rule_events() {
    let events = run_worker(
        snapshot(true),
        r#"{"name":"worker-test","nice":5}"#,
        FakePlatform::new(None),
        Level::DEBUG,
    );

    assert!(events.contains(Level::DEBUG, "Found rule"));
    assert!(events.contains(Level::INFO, "worker-test(42)"));
}

#[test]
fn level_filter_suppresses_info_but_allows_warn() {
    let captured = CapturedEvents::default();
    let subscriber = tracing_subscriber::registry()
        .with(LevelFilter::WARN)
        .with(captured.clone());

    tracing::subscriber::with_default(subscriber, || {
        info!("lower severity");
        warn!("higher severity");
    });

    assert!(!captured.contains(Level::INFO, "lower severity"));
    assert!(captured.contains(Level::WARN, "higher severity"));
}
