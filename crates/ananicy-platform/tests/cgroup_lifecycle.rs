//! Hermetic tests for cgroup lifecycle reconciliation.
//!
//! These run against a temporary cgroup-v2 hierarchy installed through the
//! library's test seam, so nothing here touches the machine's cgroups. Each
//! test takes a lock because the manager and the created-cgroup registry are
//! process-global.

mod common;

use {
    ananicy_platform::{
        cgroup::{CgroupInfo, CgroupVersion},
        cgroups::{
            CgroupApplyStatus, CgroupSettings, CpuQuota, ReconcileOptions, clear_created_registry,
            create_cgroup, created_cgroup_names, reconcile_cgroups, reconcile_cgroups_with,
            record_created_cgroup_for_tests, reset_cgroup_detection, set_cgroup_manager_for_tests,
        },
    },
    common::SimHierarchy,
    std::{
        fmt, fs, io,
        path::Path,
        sync::{Arc, Mutex, MutexGuard},
        time::Duration,
    },
    tracing::{
        Event, Level, Metadata, Subscriber,
        field::{Field, Visit},
        span::Id,
        subscriber,
    },
};

static HIERARCHY_LOCK: Mutex<()> = Mutex::new(());

fn install(hierarchy: &SimHierarchy) -> MutexGuard<'static, ()> {
    let guard = HIERARCHY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    clear_created_registry();
    set_cgroup_manager_for_tests(
        CgroupInfo {
            version: CgroupVersion::V2,
            mount_point: hierarchy.mount_point.clone(),
        },
        Some(hierarchy.delegated_root.clone()),
    );
    guard
}

fn teardown() {
    clear_created_registry();
    // `reset_cgroup_detection` clears the cached manager, so a test that
    // replaced it (e.g. to model a re-detected foreign root) must not leave
    // the next test rebuilding from the host's real cgroup hierarchy.
    reset_cgroup_detection();
}

/// Creates a cgroup through the public API (which registers it) and then seeds
/// the control files a real kernel would have created with non-default values,
/// so a reset can be observed afterwards.
fn create_registered(hierarchy: &SimHierarchy, name: &str) -> std::path::PathBuf {
    let status = create_cgroup(name, CgroupSettings::default());
    assert!(status.created() && status.is_ok(), "{status}");
    let path = hierarchy.delegated_root.join(name);
    assert!(path.is_dir());
    // Keep the quota line shorter than the default we expect to see written:
    // a real kernel control file replaces its contents on write, while a
    // regular file in a temp hierarchy would retain the tail of a longer line.
    fs::write(path.join("cpu.max"), "0\n").unwrap();
    fs::write(path.join("cpu.weight"), "500\n").unwrap();
    fs::write(path.join("cgroup.procs"), "").unwrap();
    path
}

/// A temp hierarchy's control files are regular files, not kernel pseudo-files:
/// they have to be removed before `rmdir` can succeed, the way the kernel
/// removes its own control files when a cgroup is deleted.
fn clean_remove(path: &Path) -> io::Result<()> {
    for file in ["cpu.max", "cpu.weight", "cgroup.procs", "cgroup.events"] {
        match fs::remove_file(path.join(file)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    fs::remove_dir(path)
}

fn no_sleep() -> Arc<dyn Fn(Duration) + Send + Sync> {
    Arc::new(|_| panic!("no EBUSY was injected, so there must be no retry sleep"))
}

/// The smallest subscriber that can answer "was a warning emitted for this
/// cgroup?". `tracing-subscriber` is not a dependency of this crate, and the
/// summary count alone cannot distinguish a warning from a silent skip.
#[derive(Default)]
struct WarningCapture(Arc<Mutex<Vec<String>>>);

impl Subscriber for WarningCapture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        if *event.metadata().level() == Level::WARN {
            let mut visitor = MessageVisitor(String::new());
            event.record(&mut visitor);
            self.0.lock().unwrap().push(visitor.0);
        }
    }
}

struct MessageVisitor(String);

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

#[test]
fn a_removed_rule_has_its_settings_reset_and_its_empty_cgroup_deleted() {
    let hierarchy = SimHierarchy::new();
    let _guard = install(&hierarchy);
    let path = create_registered(&hierarchy, "removed.scope");

    // Observe the control files at the moment of rmdir: that proves the reset
    // happens before the delete, rather than only afterwards in a test.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_by_rmdir = Arc::clone(&seen);
    let options = ReconcileOptions {
        sleep: no_sleep(),
        remove_dir: Arc::new(move |path: &Path| {
            seen_by_rmdir.lock().unwrap().push((
                fs::read_to_string(path.join("cpu.max")).unwrap(),
                fs::read_to_string(path.join("cpu.weight")).unwrap(),
            ));
            clean_remove(path)
        }),
    };

    let summary = reconcile_cgroups_with(&[], &options);
    assert_eq!(summary.removed, 1);
    assert_eq!(summary.failures, 0);
    assert!(!path.exists(), "the empty cgroup must be deleted");
    assert_eq!(
        *seen.lock().unwrap(),
        vec![("max 100000\n".to_string(), "100\n".to_string())],
        "cpu.max and cpu.weight must be back at their kernel defaults before rmdir"
    );
    assert!(
        !created_cgroup_names().contains(&"removed.scope".to_string()),
        "a deleted cgroup must leave the registry"
    );
    teardown();
}

#[test]
fn a_new_cgroup_whose_settings_failed_is_still_registered() {
    let hierarchy = SimHierarchy::new();
    let _guard = install(&hierarchy);
    // The simulated hierarchy has no cpu.max file yet, so the quota write
    // fails while the directory itself is created.
    let status = create_cgroup(
        "failed-settings.scope",
        CgroupSettings {
            cpu_quota: Some(CpuQuota::Percentage(50)),
            ..Default::default()
        },
    );
    assert_eq!(status, CgroupApplyStatus::SettingsFailed { created: true });
    assert!(
        created_cgroup_names().contains(&"failed-settings.scope".to_string()),
        "a created cgroup must be registered even when its settings failed"
    );

    // Registration is what lets reconciliation clean it up later. Seed the
    // procs file the kernel would expose so the cgroup is known to be empty.
    let path = hierarchy.delegated_root.join("failed-settings.scope");
    fs::write(path.join("cgroup.procs"), "").unwrap();
    let options = ReconcileOptions {
        sleep: no_sleep(),
        remove_dir: Arc::new(clean_remove),
    };
    let summary = reconcile_cgroups_with(&[], &options);
    assert_eq!(
        summary.removed, 1,
        "the failed cgroup must still be cleaned up"
    );
    assert!(!path.exists());
    teardown();
}

#[test]
fn a_still_active_rule_keeps_its_cgroup() {
    let hierarchy = SimHierarchy::new();
    let _guard = install(&hierarchy);
    let path = create_registered(&hierarchy, "kept.scope");

    let summary = reconcile_cgroups(&["kept.scope".to_string()]);
    assert_eq!(summary.active, 1);
    assert_eq!(summary.removed, 0);
    assert!(path.exists());
    assert_eq!(
        fs::read_to_string(path.join("cpu.max")).unwrap(),
        "0\n",
        "an active rule's settings must not be reset"
    );
    teardown();
}

#[test]
fn a_populated_cgroup_is_retained_with_a_warning() {
    let hierarchy = SimHierarchy::new();
    let _guard = install(&hierarchy);
    let path = create_registered(&hierarchy, "populated.scope");
    fs::write(path.join("cgroup.procs"), "4242\n").unwrap();

    let capture = WarningCapture::default();
    let messages = Arc::clone(&capture.0);
    let summary = subscriber::with_default(capture, || reconcile_cgroups(&[]));

    assert_eq!(summary.retained_populated, 1);
    assert_eq!(summary.removed, 0);
    assert!(path.exists(), "a populated cgroup must never be deleted");
    assert_eq!(
        fs::read_to_string(path.join("cpu.max")).unwrap(),
        "0\n",
        "a retained cgroup's settings must be left untouched"
    );
    assert_eq!(
        fs::read_to_string(path.join("cpu.weight")).unwrap(),
        "500\n",
        "both settings must survive retention"
    );
    assert!(
        messages
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains("populated.scope") && line.contains("keeping it")),
        "the retained cgroup must be reported at warn level: {messages:?}"
    );
    teardown();
}

#[test]
fn a_foreign_path_is_never_deleted() {
    let hierarchy = SimHierarchy::new();
    let _guard = install(&hierarchy);
    let path = create_registered(&hierarchy, "changed-ownership.scope");

    // Model delegation being re-detected elsewhere: the recorded path is now
    // outside the new delegated root, so it must be refused even though it is
    // still in the registry.
    let other_root = hierarchy.mount_point.join("system.slice/other.service");
    fs::create_dir_all(&other_root).unwrap();
    set_cgroup_manager_for_tests(
        CgroupInfo {
            version: CgroupVersion::V2,
            mount_point: hierarchy.mount_point.clone(),
        },
        Some(other_root),
    );

    let summary = reconcile_cgroups(&[]);
    assert_eq!(summary.skipped_foreign, 1);
    assert_eq!(summary.removed, 0);
    assert!(path.exists());
    assert_eq!(
        fs::read_to_string(path.join("cpu.max")).unwrap(),
        "0\n",
        "a foreign path must not even have its settings reset"
    );

    // A foreign cgroup that was never registered is equally untouched.
    let foreign = hierarchy.foreign_root.join("untouched.scope");
    fs::create_dir_all(&foreign).unwrap();
    fs::write(foreign.join("cgroup.procs"), "").unwrap();
    assert!(reconcile_cgroups(&[]).removed == 0);
    assert!(foreign.exists());
    // Restore this test's own hierarchy before releasing the lock: a later
    // test must not inherit a manager pointed at the foreign root.
    set_cgroup_manager_for_tests(
        CgroupInfo {
            version: CgroupVersion::V2,
            mount_point: hierarchy.mount_point.clone(),
        },
        Some(hierarchy.delegated_root.clone()),
    );
    teardown();
}

#[test]
fn shutdown_reconciliation_does_not_delete_the_delegated_root() {
    let hierarchy = SimHierarchy::new();
    let _guard = install(&hierarchy);
    let child = create_registered(&hierarchy, "shutdown.scope");

    // A defensive registry entry for the root itself: even if one slipped in,
    // the delegation boundary is not a cgroup this daemon may remove.
    record_created_cgroup_for_tests("delegated-root", &hierarchy.delegated_root);

    let options = ReconcileOptions {
        sleep: no_sleep(),
        remove_dir: Arc::new(clean_remove),
    };
    let summary = reconcile_cgroups_with(&[], &options);
    assert_eq!(summary.removed, 1, "the registered child is cleaned up");
    assert_eq!(summary.skipped_foreign, 1, "the root is refused");
    assert!(!child.exists());
    assert!(hierarchy.delegated_root.is_dir(), "the root must survive");
    teardown();
}

#[test]
fn a_cgroup_left_by_an_earlier_run_is_not_deleted() {
    let hierarchy = SimHierarchy::new();
    let _guard = install(&hierarchy);
    let leftover = hierarchy.delegated_root.join("from-previous-run.scope");
    fs::create_dir_all(&leftover).unwrap();
    fs::write(leftover.join("cgroup.procs"), "").unwrap();
    fs::write(leftover.join("cpu.max"), "50000 100000\n").unwrap();

    assert!(!created_cgroup_names().contains(&"from-previous-run.scope".to_string()));
    let summary = reconcile_cgroups(&[]);
    assert_eq!(summary.removed, 0);
    assert!(leftover.exists());
    assert_eq!(
        fs::read_to_string(leftover.join("cpu.max")).unwrap(),
        "50000 100000\n",
        "an unregistered cgroup is not ours to reset or delete"
    );
    teardown();
}

#[test]
fn an_ebusy_rmdir_is_retried_within_a_bounded_backoff() {
    let hierarchy = SimHierarchy::new();
    let _guard = install(&hierarchy);
    let path = create_registered(&hierarchy, "busy.scope");

    let failures = Arc::new(Mutex::new(2usize));
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let remaining = Arc::clone(&failures);
    let slept = Arc::clone(&sleeps);
    let options = ReconcileOptions {
        sleep: Arc::new(move |delay: Duration| slept.lock().unwrap().push(delay)),
        remove_dir: Arc::new(move |path: &Path| {
            let mut remaining = remaining.lock().unwrap();
            if *remaining > 0 {
                *remaining -= 1;
                return Err(io::Error::from_raw_os_error(libc::EBUSY));
            }
            drop(remaining);
            clean_remove(path)
        }),
    };

    let summary = reconcile_cgroups_with(&[], &options);
    assert_eq!(summary.busy_retries, 2);
    assert_eq!(summary.removed, 1);
    assert!(!path.exists());
    assert_eq!(*sleeps.lock().unwrap(), vec![Duration::from_millis(50); 2]);
    teardown();
}

#[test]
fn a_persistently_busy_cgroup_is_left_in_place_after_the_retry_bound() {
    let hierarchy = SimHierarchy::new();
    let _guard = install(&hierarchy);
    let path = create_registered(&hierarchy, "always-busy.scope");

    let sleeps = Arc::new(Mutex::new(0usize));
    let slept = Arc::clone(&sleeps);
    let options = ReconcileOptions {
        sleep: Arc::new(move |_: Duration| *slept.lock().unwrap() += 1),
        remove_dir: Arc::new(|_: &Path| Err(io::Error::from_raw_os_error(libc::EBUSY))),
    };

    let summary = reconcile_cgroups_with(&[], &options);
    assert_eq!(summary.removed, 0);
    assert_eq!(summary.failures, 1);
    assert!(path.exists());
    assert_eq!(*sleeps.lock().unwrap(), 19, "20 attempts means 19 backoffs");
    teardown();
}

/// The symlink guard must also run when a rule names a cgroup that already
/// exists: `Path::exists()` follows the link, so the "adopt existing" branch
/// is otherwise a bypass around `ensure_child`'s check.
#[test]
fn an_existing_symlinked_cgroup_is_refused_on_the_adoption_path() {
    let hierarchy = SimHierarchy::new();
    let _guard = install(&hierarchy);
    let foreign = hierarchy.mount_point.join("user.slice");
    fs::create_dir_all(&foreign).unwrap();
    fs::write(foreign.join("cgroup.procs"), "").unwrap();

    #[cfg(unix)]
    std::os::unix::fs::symlink(&foreign, hierarchy.delegated_root.join("escape.scope")).unwrap();

    let status = create_cgroup("escape.scope", CgroupSettings::default());
    assert_eq!(
        status,
        CgroupApplyStatus::Failed,
        "an adopted cgroup behind a symlink must be refused"
    );
    teardown();
}
