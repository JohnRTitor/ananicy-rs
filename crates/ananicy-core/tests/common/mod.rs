//! Shared test harness for the `ananicy-core` worker tests.
//!
//! [`FakePlatform`] implements [`PlatformActions`] without touching the kernel:
//! it records every call the worker makes and can be told to fail a specific
//! operation. That makes the rule-application contract observable, so the tests
//! can assert *what* the daemon would do to a process rather than how it logged
//! it.

#![allow(dead_code)]

use {
    ananicy_core::{
        config::{Config, ConfigSnapshot},
        cpuset::CpuSet,
        process::Process,
        rules::{Rules, SharedRules},
        types::Pid,
        worker::{PlatformActions, PlatformError, Tids, Worker},
    },
    smallvec::{SmallVec, smallvec},
    std::{
        collections::HashMap,
        fmt, fs,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc,
        },
        thread,
        time::Duration,
    },
    tracing::{Event, Level, Subscriber},
    tracing_subscriber::{
        filter::LevelFilter,
        layer::{Context, Layer},
        prelude::*,
    },
};

// ---------------------------------------------------------
// Captured tracing events
// ---------------------------------------------------------

#[derive(Clone, Default)]
pub struct CapturedEvents(Arc<Mutex<Vec<(Level, String)>>>);

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
    pub fn contains(&self, level: Level, text: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .iter()
            .any(|(event_level, message)| *event_level == level && message.contains(text))
    }

    /// Everything captured, for putting in a failure message.
    ///
    /// An assertion that reports *what was logged* is the difference between a
    /// test that can be debugged from its own output and one that has to be
    /// reproduced by hand to see what the code actually emitted.
    pub fn all(&self) -> Vec<(Level, String)> {
        self.0.lock().unwrap().clone()
    }
}

/// Installs a subscriber filtered at `filter` and runs `body` with it as the
/// default subscriber, returning the events it emitted.
pub fn capture_events(filter: LevelFilter, body: impl FnOnce()) -> CapturedEvents {
    let captured = CapturedEvents::default();
    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(captured.clone());

    tracing::subscriber::with_default(subscriber, body);
    captured
}

// ---------------------------------------------------------
// Recording platform
// ---------------------------------------------------------

/// One recorded interaction with the platform layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    SetPriority { nice: i32 },
    SetLatencyNice { value: i32 },
    SetSched { sched: String, rtprio: u32 },
    SetIoPriority { ioclass: String, ionice: i32 },
    SetOomScoreAdj { value: i32 },
    AddPidToCgroup { cgroup: String },
    SetCpuWeight { weight: u32 },
    SetAffinity { cpuset: String },
}

pub struct FakePlatform {
    cgroup_v2: bool,
    realtime: bool,
    /// Whether a successful realtime policy change should flip `is_realtime`.
    becomes_realtime_on_set_sched: bool,
    /// The observed state, initially CFS and flipped by `set_sched` when armed.
    /// An `Arc` keeps the builder's `self`-consuming API while the flag stays
    /// writable from `&self` methods.
    realtime_after_set_sched: Arc<AtomicBool>,
    tids: Option<Vec<i32>>,
    max_cores: u32,
    failures: Mutex<HashMap<&'static str, PlatformError>>,
    calls: Mutex<Vec<Call>>,
    priority_calls: Arc<AtomicUsize>,
    /// The worker only asks for the core count when it has to parse a cpuset,
    /// so this distinguishes a cache hit from a miss.
    max_cores_calls: Arc<AtomicUsize>,
}

impl Default for FakePlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl FakePlatform {
    pub fn new() -> Self {
        Self {
            cgroup_v2: false,
            realtime: false,
            becomes_realtime_on_set_sched: false,
            realtime_after_set_sched: Arc::new(AtomicBool::new(false)),
            tids: None,
            max_cores: 8,
            failures: Mutex::new(HashMap::new()),
            calls: Mutex::new(Vec::new()),
            priority_calls: Arc::new(AtomicUsize::new(0)),
            max_cores_calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// A platform on a cgroup-v2 host, where `nice` is mirrored into
    /// `cpu.weight` and realtime processes hit the cgroup kernel limitation.
    pub fn cgroup_v2() -> Self {
        Self {
            cgroup_v2: true,
            ..Self::new()
        }
    }

    /// A cgroup-v2 platform on which the test process reports as a realtime
    /// task, which is the combination the cgroup kernel limitation applies to.
    pub fn realtime_on_cgroup_v2() -> Self {
        Self {
            cgroup_v2: true,
            realtime: true,
            ..Self::new()
        }
    }

    /// A cgroup-v1 platform on which the test process reports as a realtime
    /// task.
    pub fn realtime_on_cgroup_v1() -> Self {
        Self {
            realtime: true,
            ..Self::new()
        }
    }

    /// Declares that this pid becomes realtime after a successful FIFO/RR
    /// `set_sched`. Until then it reports as CFS, which is the stale-sample
    /// situation a rule that sets a realtime policy creates.
    pub fn becomes_realtime_after_set_sched(mut self) -> Self {
        self.becomes_realtime_on_set_sched = true;
        self
    }

    /// A platform that reports a fixed thread list, including the empty one.
    pub fn with_thread_list(tids: Vec<i32>) -> Self {
        Self {
            tids: Some(tids),
            ..Self::new()
        }
    }

    pub fn with_max_cores(mut self, max_cores: u32) -> Self {
        self.max_cores = max_cores;
        self
    }

    /// Makes the named operation fail with `error` on every call.
    pub fn failing(self, operation: &'static str, error: PlatformError) -> Self {
        self.failures.lock().unwrap().insert(operation, error);
        self
    }

    /// Every mutation the worker attempted, in order.
    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    /// How many times the worker had to look up the CPU count, which happens
    /// only on a cpuset-cache miss.
    pub fn max_cores_calls(&self) -> usize {
        self.max_cores_calls.load(Ordering::SeqCst)
    }

    /// Only the mutations of the given kind, in order.
    pub fn calls_of(&self, operation: &str) -> Vec<Call> {
        self.calls()
            .into_iter()
            .filter(|call| operation_of(call) == operation)
            .collect()
    }

    /// A counter that can be polled from another thread to observe that the
    /// worker has started applying rules.
    pub fn priority_call_counter(&self) -> Arc<AtomicUsize> {
        self.priority_calls.clone()
    }

    fn record(&self, operation: &'static str, call: Call) -> Result<(), PlatformError> {
        self.calls.lock().unwrap().push(call);
        match self.failures.lock().unwrap().get(operation) {
            Some(error) => Err(error.clone_error()),
            None => Ok(()),
        }
    }
}

/// `PlatformError` is not `Clone`, so the fake keeps the ability to report the
/// same failure more than once by rebuilding it from its variant.
trait CloneError {
    fn clone_error(&self) -> PlatformError;
}

impl CloneError for PlatformError {
    fn clone_error(&self) -> PlatformError {
        match self {
            PlatformError::NotFound => PlatformError::NotFound,
            PlatformError::PermissionDenied => PlatformError::PermissionDenied,
            PlatformError::Skipped(message) => PlatformError::Skipped(message.clone()),
            PlatformError::Unsupported => PlatformError::Unsupported,
            PlatformError::InvalidCpuset(message) => PlatformError::InvalidCpuset(message.clone()),
            PlatformError::Io(error) => {
                PlatformError::Io(std::io::Error::new(error.kind(), error.to_string()))
            }
        }
    }
}

fn operation_of(call: &Call) -> &'static str {
    match call {
        Call::SetPriority { .. } => "set_priority",
        Call::SetLatencyNice { .. } => "set_latency_nice",
        Call::SetSched { .. } => "set_sched",
        Call::SetIoPriority { .. } => "set_io_priority",
        Call::SetOomScoreAdj { .. } => "set_oom_score_adj",
        Call::AddPidToCgroup { .. } => "add_pid_to_cgroup",
        Call::SetCpuWeight { .. } => "set_cpu_weight",
        Call::SetAffinity { .. } => "set_affinity",
    }
}

impl PlatformActions for FakePlatform {
    fn is_realtime(&self, _pid: i32) -> bool {
        self.realtime || self.realtime_after_set_sched.load(Ordering::SeqCst)
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
        self.max_cores_calls.fetch_add(1, Ordering::SeqCst);
        self.max_cores
    }

    fn get_tids(&self, pid: i32) -> Result<Tids, PlatformError> {
        Ok(self
            .tids
            .clone()
            .map(SmallVec::from_vec)
            .unwrap_or_else(|| smallvec![pid]))
    }

    fn set_priority(&self, _pid: i32, _tids: &[i32], nice: i32) -> Result<(), PlatformError> {
        self.priority_calls.fetch_add(1, Ordering::SeqCst);
        self.record("set_priority", Call::SetPriority { nice })
    }

    fn set_latency_nice(
        &self,
        _pid: i32,
        _tids: &[i32],
        lat_nice: i32,
    ) -> Result<(), PlatformError> {
        self.record("set_latency_nice", Call::SetLatencyNice { value: lat_nice })
    }

    fn set_sched(
        &self,
        _pid: i32,
        _tids: &[i32],
        sched: &str,
        rtprio: u32,
    ) -> Result<(), PlatformError> {
        let result = self.record(
            "set_sched",
            Call::SetSched {
                sched: sched.to_string(),
                rtprio,
            },
        );
        // A kernel only changes policy when the call succeeds, and an RT
        // policy with priority zero is not a realtime task by the daemon's
        // own definition (static priority > 0).
        if result.is_ok()
            && self.becomes_realtime_on_set_sched
            && matches!(sched, "fifo" | "rr")
            && rtprio > 0
        {
            self.realtime_after_set_sched.store(true, Ordering::SeqCst);
        }
        result
    }

    fn set_io_priority(&self, _pid: i32, ioclass: &str, ionice: i32) -> Result<(), PlatformError> {
        self.record(
            "set_io_priority",
            Call::SetIoPriority {
                ioclass: ioclass.to_string(),
                ionice,
            },
        )
    }

    fn set_oom_score_adj(&self, _pid: i32, oom_score_adj: i32) -> Result<(), PlatformError> {
        self.record(
            "set_oom_score_adj",
            Call::SetOomScoreAdj {
                value: oom_score_adj,
            },
        )
    }

    fn add_pid_to_cgroup(&self, _pid: i32, cgroup: &str) -> Result<(), PlatformError> {
        self.record(
            "add_pid_to_cgroup",
            Call::AddPidToCgroup {
                cgroup: cgroup.to_string(),
            },
        )
    }

    fn set_cpu_weight(&self, _pid: i32, weight: u32) -> Result<(), PlatformError> {
        self.record("set_cpu_weight", Call::SetCpuWeight { weight })
    }

    fn set_affinity(&self, _pid: i32, _tids: &[i32], cpuset: &CpuSet) -> Result<(), PlatformError> {
        self.record(
            "set_affinity",
            Call::SetAffinity {
                cpuset: cpuset.to_string(),
            },
        )
    }
}

// ---------------------------------------------------------
// Worker driver
// ---------------------------------------------------------

/// What a worker run produced: the log events and the platform calls.
pub struct WorkerRun {
    pub events: CapturedEvents,
    pub platform: Arc<FakePlatform>,
}

/// Feeds one process matching `rule` to a worker and returns what happened.
pub fn run_worker(snapshot: ConfigSnapshot, rule: &str, platform: FakePlatform) -> WorkerRun {
    run_worker_with(
        snapshot,
        rule,
        platform,
        HashMap::new(),
        Pid(42),
        "worker-test",
    )
}

/// Full-control variant: explicit cpuset aliases, PID and process name.
pub fn run_worker_with(
    snapshot: ConfigSnapshot,
    rule: &str,
    platform: FakePlatform,
    cpuset_aliases: HashMap<String, String>,
    pid: Pid,
    name: &str,
) -> WorkerRun {
    let filter = LevelFilter::from_level(Level::from(&snapshot.loglevel));
    let config = Arc::new(Config::new(snapshot));
    let mut rules = Rules::new(config.clone());
    assert!(
        rules.load_rule_from_string(rule),
        "the test rule must be valid: {rule}"
    );

    let (tx, rx) = mpsc::channel();
    tx.send(Process::new(pid, name.to_string()).with_authoritative_name())
        .unwrap();
    drop(tx);

    let platform = Arc::new(platform);
    let events = capture_events(filter, || {
        Worker::new(
            config,
            SharedRules::new(rules),
            platform.clone(),
            cpuset_aliases,
            rx,
            None,
            Arc::new(AtomicBool::new(false)),
        )
        .work_loop();
    });

    WorkerRun { events, platform }
}

/// The default configuration for tests that assert on rule application rather
/// than on logging, with the `log_applied_rule` flag under test.
pub fn snapshot(log_applied_rule: bool) -> ConfigSnapshot {
    ConfigSnapshot {
        log_applied_rule,
        ..ConfigSnapshot::default()
    }
}

/// Writes `contents` to a configuration file in a fresh temporary directory and
/// loads it, so no test depends on `/etc/ananicy.d`.
pub fn load_config(contents: &str) -> (tempfile::TempDir, Arc<Config>) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ananicy.conf");
    fs::write(&path, contents).unwrap();
    let config = Arc::new(Config::load_file(&path, true).unwrap());
    (directory, config)
}

/// Polls `condition` until it holds, so a test can wait for the worker instead
/// of sleeping for a fixed duration.
pub fn wait_until(mut condition: impl FnMut() -> bool) {
    for _ in 0..2_000 {
        if condition() {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("condition was not met in time");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_successful_realtime_policy_flips_is_realtime() {
        let platform = FakePlatform::cgroup_v2().becomes_realtime_after_set_sched();
        assert!(!platform.is_realtime(42), "the process starts as CFS");
        platform.set_sched(42, &[], "fifo", 1).unwrap();
        assert!(
            platform.is_realtime(42),
            "the policy change makes the process realtime"
        );
        assert_eq!(
            platform.calls_of("set_sched").len(),
            1,
            "the fake still records the call for rule tests"
        );
    }

    #[test]
    fn a_failed_realtime_policy_does_not_flip_is_realtime() {
        let platform = FakePlatform::new()
            .becomes_realtime_after_set_sched()
            .failing("set_sched", PlatformError::Unsupported);
        assert!(platform.set_sched(42, &[], "rr", 1).is_err());
        assert!(
            !platform.is_realtime(42),
            "a policy the kernel refused cannot make the process realtime"
        );
    }
}
