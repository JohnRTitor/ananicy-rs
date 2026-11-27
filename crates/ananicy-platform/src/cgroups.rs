use std::{
    fmt, fs, io,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, RwLock},
    thread,
    time::Duration,
};

use {
    ananicy_core::worker::{
        PlatformError,
        PlatformError::{NotFound, Unsupported},
    },
    lru::LruCache,
    tracing::{debug, warn},
};

use crate::{
    cgroup::{
        CgroupError,
        manager::{CgroupController, CgroupManager},
    },
    mounts::{CgroupInfo, get_cgroup_info},
};

/// The manager for the currently detected hierarchy.
///
/// It is built on first use and then reused, so a running daemon does not
/// re-read the mount table for every process it tunes. Caching it is only safe
/// while [`reset_cgroup_detection`] exists to drop it again: the
/// `cgroup_realtime_workaround` re-creates the cgroups after a short delay
/// because systemd may only just have created the delegated subtree, and a
/// cached manager would keep resolving names against the hierarchy as it looked
/// before that.
static MANAGER: RwLock<Option<Arc<CgroupManager>>> = RwLock::new(None);

/// The cgroup manager, building it from the current detection on first use.
fn get_manager() -> Option<Arc<CgroupManager>> {
    if let Some(cached) = MANAGER.read().ok().and_then(|guard| guard.clone()) {
        return Some(cached);
    }

    let mut guard = MANAGER.write().ok()?;
    Some(
        guard
            .get_or_insert_with(|| Arc::new(CgroupManager::new(get_cgroup_info())))
            .clone(),
    )
}

/// Forgets the detected hierarchy and everything derived from it.
///
/// The next cgroup operation re-detects the mount table and rebuilds the
/// manager, which is what makes a re-detection actually reach the cgroup code
/// paths instead of only the read-only accessors.
pub fn reset_cgroup_detection() {
    crate::mounts::reset_cgroup_info();
    if let Ok(mut guard) = MANAGER.write() {
        *guard = None;
    }
}

/// Whether a cgroup hierarchy is available, without caching the answer.
///
/// Used to decide whether waiting can help at all; deliberately does not build
/// the manager, so that a later detection is still free to change its mind.
pub fn has_cgroup_hierarchy() -> bool {
    get_cgroup_info().version != crate::cgroup::CgroupVersion::None
}

/// What a `.cgroups` rule's `CPUQuota` asks the kernel for.
///
/// `Unlimited` is the kernel default (`max` on v2, `-1` on v1). A rule that
/// omits `CPUQuota` leaves the field unset, which also means "do not write the
/// control file": the kernel default stays in place without this module
/// touching it at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuQuota {
    /// No bandwidth limit.
    Unlimited,
    /// A percentage of the machine's CPUs; the manager clamps it to `1..=100`.
    Percentage(u32),
}

/// The resource settings a `.cgroups` rule can ask a cgroup to be created with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CgroupSettings {
    /// A bandwidth limit, written to `cpu.max` on cgroup v2 and to
    /// `cpu.cfs_quota_us` on cgroup v1. `None` means no write at all.
    pub cpu_quota: Option<CpuQuota>,
    /// A relative weight, written to `cpu.weight` on cgroup v2 and to
    /// `cpu.shares` on cgroup v1. The kernel default is 100, the range is
    /// `1..=10000`; anything outside it is clamped.
    pub cpu_weight: Option<u32>,
}

/// What happened to a cgroup a `.cgroups` rule asked to create.
///
/// This is deliberately small: the caller only needs to know whether the
/// directory was created and whether all of the requested settings were
/// applied, so it can warn once per cgroup and carry on with the other rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CgroupApplyStatus {
    /// The cgroup was created (or adopted) and every requested setting applied.
    Applied { created: bool },
    /// The cgroup was created (or adopted), but at least one setting failed.
    SettingsFailed { created: bool },
    /// The cgroup could not be created or resolved; nothing was configured.
    Failed,
}

impl CgroupApplyStatus {
    /// Whether the cgroup directory was created by this call. Existing cgroups
    /// still get their settings re-applied, so this says nothing about success.
    pub fn created(self) -> bool {
        matches!(
            self,
            Self::Applied { created: true } | Self::SettingsFailed { created: true }
        )
    }

    /// Whether every requested setting was applied.
    pub fn is_ok(self) -> bool {
        matches!(self, Self::Applied { .. })
    }
}

impl fmt::Display for CgroupApplyStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Applied { created } => {
                if *created {
                    f.write_str("cgroup created and settings applied")
                } else {
                    f.write_str("existing cgroup settings re-applied")
                }
            }
            Self::SettingsFailed { created } => {
                if *created {
                    f.write_str("cgroup created, but at least one setting failed")
                } else {
                    f.write_str("existing cgroup, but at least one setting failed")
                }
            }
            Self::Failed => f.write_str("cgroup could not be created"),
        }
    }
}

/// Creates the cgroup a rule asks for and applies the rule's settings to it.
///
/// The returned status says both whether the cgroup was created and whether
/// every setting took effect. An existing cgroup is reported as not created but
/// still has the settings applied, which is the point: a cgroup can already be
/// there because a previous run made it, because another rule names it, or
/// because it predates the rule being edited, and in none of those cases is its
/// current configuration necessarily the one this rule asks for. Skipping the
/// settings for an existing cgroup meant editing a `.cgroups` file had no effect
/// until the cgroup was deleted by hand. The reference re-applies them
/// unconditionally for the same reason.
pub fn create_cgroup(cgroup_name: &str, settings: CgroupSettings) -> CgroupApplyStatus {
    let Some(manager) = get_manager() else {
        debug!(
            "cgroup {} not created: no hierarchy to create it in",
            cgroup_name
        );
        return CgroupApplyStatus::Failed;
    };

    // Resolved once. `cgroup_exists` used to answer this by resolving the target
    // and dropping the result, and then the branch that acted on the answer
    // resolved the same name again — two identical `PathBuf`s built to ask one
    // question, on a path that runs for every `.cgroups` rule at every reload.
    let (target, created) = match manager.resolve_target_dir(cgroup_name) {
        Some(target) if target.exists() => (target, false),
        // Either there is no hierarchy to name a path in, or the cgroup is not
        // there yet. Both are `ensure_child`'s to answer, and it resolves the
        // name again because creating it is where the path is actually needed.
        _ => match manager.ensure_child(cgroup_name) {
            Ok(target) => (target, true),
            Err(e) => {
                debug!("cgroup {cgroup_name} not created: {e}");
                return CgroupApplyStatus::Failed;
            }
        },
    };

    // Applied either way. The manager knows the difference between "the
    // controller is not enabled here" and a real write error, and warns once
    // per cgroup; the status here lets the caller report the rule that asked
    // for it and continue with the remaining rules.
    let mut failed = false;
    if let Some(quota) = settings.cpu_quota {
        failed |= manager.set_cpu_max(&target, quota).is_err();
    }
    if let Some(weight) = settings.cpu_weight {
        failed |= manager.set_cpu_weight(&target, weight).is_err();
    }
    let status = if failed {
        CgroupApplyStatus::SettingsFailed { created }
    } else {
        CgroupApplyStatus::Applied { created }
    };

    // Only cgroups *this run* created are eligible for cleanup later. A cgroup
    // that already existed is adopted for settings re-apply (above), but nobody
    // can prove it was ours to delete — it may be left by a previous run or by
    // another tool. `is_removable_owned` also keeps a sync of the delegation
    // boundary itself out of the registry.
    // A cgroup is registered even if its settings failed: the directory exists
    // and belongs to this run, so leaving it out of the registry would leak it
    // at the next reload or at shutdown.
    if status.created() && manager.is_removable_owned(&target) {
        record_created(cgroup_name, &target);
    }

    status
}

/// The cgroups this run actually created, keyed by the rule name that asked
/// for them. The value is the path that was created: ownership is re-checked
/// against the current manager at cleanup time, because delegation can be
/// re-detected between creation and a reload.
///
/// Behind the same bounded, clone-shared pattern as the manager's
/// `reported_weightless` cache: the registry lives in a process-global, so the
/// `Arc` makes a shared handle possible, and a bounded LRU stops a daemon that
/// churns rule files from holding every name it has ever seen.
type CreatedRegistry = Arc<Mutex<LruCache<String, PathBuf>>>;
const CREATED_REGISTRY_CAPACITY: usize = 1024;

fn created_registry() -> &'static CreatedRegistry {
    static REGISTRY: OnceLock<CreatedRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let capacity = NonZeroUsize::new(CREATED_REGISTRY_CAPACITY).unwrap_or(NonZeroUsize::MIN);
        Arc::new(Mutex::new(LruCache::new(capacity)))
    })
}

fn record_created(name: &str, path: &Path) {
    if let Ok(mut registry) = created_registry().lock() {
        registry.put(name.to_string(), path.to_path_buf());
    }
}

fn forget_created(name: &str) {
    if let Ok(mut registry) = created_registry().lock() {
        registry.pop(name);
    }
}

/// The rule names of cgroups created by this run that are still remembered.
pub fn created_cgroup_names() -> Vec<String> {
    let Ok(registry) = created_registry().lock() else {
        return Vec::new();
    };
    registry.iter().map(|(name, _)| name.clone()).collect()
}

/// Forgets every recorded cgroup. Intended for tests, which replace the
/// hierarchy between scenarios and must not inherit one another's registry.
pub fn clear_created_registry() {
    if let Ok(mut registry) = created_registry().lock() {
        registry.clear();
    }
}

/// Installs a manager directly, for hermetic tests that run against a
/// temporary hierarchy instead of the machine's cgroup mount.
#[doc(hidden)]
pub fn set_cgroup_manager_for_tests(info: CgroupInfo, delegated_root: Option<PathBuf>) {
    if let Ok(mut guard) = MANAGER.write() {
        *guard = Some(Arc::new(CgroupManager::new_with_root(info, delegated_root)));
    }
}

/// Adds an entry to the created-cgroup registry, for tests that need to model
/// a cgroup created earlier in this run without going through `create_cgroup`.
#[doc(hidden)]
pub fn record_created_cgroup_for_tests(name: &str, path: &Path) {
    record_created(name, path);
}

/// How many times an `EBUSY` rmdir is retried, and how long to wait between
/// attempts. Matches the bounded `EBUSY` retry RUM uses when destroying a
/// cgroup: long enough for the kernel to finish moving a last process out,
/// short enough that shutdown cannot hang on a cgroup nobody will ever empty.
const REMOVE_ATTEMPTS: u8 = 20;
const RETRY_DELAY: Duration = Duration::from_millis(50);

/// The injected rmdir operation. Separated out because a `Fn` over a borrowed
/// path is awkward to read at every use site.
pub type RemoveDirFn = Arc<dyn Fn(&Path) -> io::Result<()> + Send + Sync>;

/// The injected parts of a reconciliation. The defaults act on the real
/// filesystem; tests pass recording closures so they can observe that settings
/// are reset before the delete and drive the `EBUSY` retry without sleeping.
pub struct ReconcileOptions {
    pub sleep: Arc<dyn Fn(Duration) + Send + Sync>,
    pub remove_dir: RemoveDirFn,
}

impl Default for ReconcileOptions {
    fn default() -> Self {
        Self {
            sleep: Arc::new(thread::sleep as fn(Duration)),
            remove_dir: Arc::new(|path: &Path| fs::remove_dir(path)),
        }
    }
}

/// What one reconciliation attempt did. Counts are enough for both the log
/// line at reload/shutdown and the hermetic lifecycle tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconcileSummary {
    /// Registered cgroups whose rule is still active; left alone.
    pub active: usize,
    /// Registered cgroups that were successfully removed.
    pub removed: usize,
    /// Registered cgroups that had already disappeared.
    pub missing: usize,
    /// Cgroups kept because they still hold processes (or have children).
    pub retained_populated: usize,
    /// Entries refused because the path is no longer inside the delegated root.
    pub skipped_foreign: usize,
    /// `EBUSY` rmdir retries that succeeded on a later attempt.
    pub busy_retries: usize,
    /// Settings resets or rmdirs that failed; the daemon carries on regardless.
    pub failures: usize,
}

/// Removes the cgroups this run created that are no longer named by an active
/// `.cgroups` rule.
///
/// Nothing here is fatal: a failed reset or rmdir is logged and the next entry
/// is still processed. A cgroup whose path is no longer Owned (for example
/// because delegation was re-detected) and the delegated root itself are
/// refused before any write. A cgroup that is populated is kept untouched, with
/// one warning, since deleting it would also evict the processes in it; its
/// settings are not reset either, because the rule that owns them is still
/// doing its job for those processes.
///
/// `active_names` is the *new* rule set: a directory is removed only when no
/// currently installed rule still asks for that name.
pub fn reconcile_cgroups(active_names: &[String]) -> ReconcileSummary {
    reconcile_cgroups_with(active_names, &ReconcileOptions::default())
}

/// The injectable form of [`reconcile_cgroups`], used by hermetic tests.
pub fn reconcile_cgroups_with(
    active_names: &[String],
    options: &ReconcileOptions,
) -> ReconcileSummary {
    let mut summary = ReconcileSummary::default();
    let Some(manager) = get_manager() else {
        return summary;
    };

    // Snapshot before touching the filesystem: the lock protects the map, not
    // the cgroups, and holding it across I/O would stall a concurrent reload.
    let entries: Vec<(String, PathBuf)> = {
        let Ok(registry) = created_registry().lock() else {
            return summary;
        };
        registry
            .iter()
            .map(|(name, path)| (name.clone(), path.clone()))
            .collect()
    };

    for (name, path) in entries {
        if active_names.iter().any(|active| active == &name) {
            summary.active += 1;
            continue;
        }
        if !manager.is_removable_owned(&path) {
            summary.skipped_foreign += 1;
            warn!(
                "Cgroup '{}' at {} is no longer inside our delegated cgroup; refusing to clean it up",
                name,
                path.display()
            );
            continue;
        }
        if !path.exists() {
            summary.missing += 1;
            forget_created(&name);
            continue;
        }

        // Check emptiness first: a cgroup that still holds processes is kept,
        // and keeping it includes keeping the settings the rule applied.
        // Resetting here would silently undo those settings while the warning
        // claimed they were preserved. Only entries that will actually be
        // removed have their defaults restored.
        if !cgroup_is_empty(&path) {
            summary.retained_populated += 1;
            warn!(
                "Cgroup '{}' at {} still holds processes; keeping it with its settings untouched rather than removing it",
                name,
                path.display()
            );
            continue;
        }

        if let Err(error) = manager.reset_cpu_settings(&path) {
            summary.failures += 1;
            warn!(
                "Could not reset cpu settings of cgroup '{}' at {}: {}; continuing with cleanup",
                name,
                path.display(),
                error
            );
        }

        match manager.remove_cgroup(
            &path,
            REMOVE_ATTEMPTS,
            RETRY_DELAY,
            &*options.sleep,
            &*options.remove_dir,
        ) {
            Ok(retries) => {
                summary.removed += 1;
                summary.busy_retries += retries;
                forget_created(&name);
            }
            Err(error) => {
                summary.failures += 1;
                warn!(
                    "Could not remove empty cgroup '{}' at {}: {}; leaving it in place",
                    name,
                    path.display(),
                    error
                );
            }
        }
    }

    summary
}

/// Whether a cgroup has no processes, including processes in its children.
///
/// `cgroup.events` is the authoritative answer when the kernel exposes it: an
/// empty `cgroup.procs` can still have tasks in a child cgroup, and `rmdir`
/// would fail with `ENOTEMPTY`. Older or synthetic hierarchies may only have
/// `cgroup.procs`; there, an unreadable file means "keep it", never "delete".
fn cgroup_is_empty(target: &Path) -> bool {
    if let Ok(events) = fs::read_to_string(target.join("cgroup.events"))
        && let Some(populated) = events
            .lines()
            .find_map(|line| line.strip_prefix("populated "))
    {
        return populated.trim() == "0";
    }
    match fs::read_to_string(target.join("cgroup.procs")) {
        Ok(procs) => procs.trim().is_empty(),
        Err(_) => false,
    }
}

pub fn add_pid_to_cgroup(pid: i32, cgroup_name: &str) -> Result<(), PlatformError> {
    let manager = get_manager().ok_or(Unsupported)?;

    // A cgroup is expected to be created first, either by `create_cgroup` or by
    // a `.cgroups` rule that declares it. Moving a process into a target that
    // does not exist is reported as `NotFound` instead of creating it here, so
    // a typo in a rule cannot silently create a cgroup nobody configured.
    let target = manager.resolve_target_dir(cgroup_name).ok_or(NotFound)?;
    if !target.exists() {
        return Err(NotFound);
    }
    match manager.move_pid(pid, &target) {
        Ok(()) => Ok(()),
        Err(e) => {
            // A process that exited during the move is not a rule failure: say
            // what actually happened instead of collapsing every typed cgroup
            // error into `Unsupported`.
            debug!("cgroup {cgroup_name}: could not add pid {pid}: {e}");
            if matches!(e, CgroupError::Gone) {
                Err(NotFound)
            } else {
                Err(Unsupported)
            }
        }
    }
}

pub fn set_cpu_weight_for_cgroup(cgroup_name: &str, weight: u32) -> Result<(), PlatformError> {
    let manager = get_manager().ok_or(Unsupported)?;
    let target = manager.resolve_target_dir(cgroup_name).ok_or(NotFound)?;
    match manager.set_cpu_weight(&target, weight) {
        Ok(()) => Ok(()),
        Err(e) => {
            debug!("cgroup {cgroup_name}: could not set cpu weight: {e}");
            Err(Unsupported)
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{cgroup::CgroupVersion, mounts::CgroupInfo},
        std::{fs, path::PathBuf},
    };

    /// The manager currently cached, and the hierarchy it was built from.
    fn cached_info() -> Option<CgroupInfo> {
        MANAGER
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().map(|manager| manager.info().clone()))
    }

    /// `MANAGER` is process-global and these tests replace it wholesale, so they
    /// have to take turns. Every test that injects a hierarchy holds this.
    static MANAGER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The regression: a `.cgroups` rule edited while its cgroup already exists
    /// had no effect, because the early return on "it exists" happened before the
    /// settings were written. The reference re-applies them, which is what makes
    /// editing the file work.
    ///
    /// The cgroup is created here, the controller files are put inside it by hand
    /// -- they would be created by the kernel in a real hierarchy -- and the rule
    /// is then applied again with a different value.
    #[test]
    fn a_rule_is_reapplied_to_a_cgroup_that_already_exists() {
        let _guard = MANAGER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let hierarchy = tempfile::tempdir().expect("a temporary hierarchy");
        let mount = hierarchy.path().to_path_buf();

        *MANAGER.write().expect("an uncontended lock") =
            Some(Arc::new(CgroupManager::new(CgroupInfo {
                version: CgroupVersion::V1,
                mount_point: mount.clone(),
            })));

        assert!(
            create_cgroup(
                "reapply",
                CgroupSettings {
                    cpu_quota: Some(CpuQuota::Percentage(90)),
                    ..Default::default()
                }
            )
            .created(),
            "the first call creates it"
        );

        let dir = mount.join("cpu/reapply");
        let period_file = dir.join("cpu.cfs_period_us");
        let quota_file = dir.join("cpu.cfs_quota_us");
        let shares_file = dir.join("cpu.shares");
        // All three, as the kernel would provide them. The v1 path writes the
        // period before the quota and gives up if it cannot, so a partial set
        // here would test nothing.
        fs::write(&period_file, "1000000\n").expect("a period file");
        fs::write(&quota_file, "0\n").expect("a quota file");
        fs::write(&shares_file, "1024\n").expect("a shares file");

        // Second call: not created, but the settings must be written.
        let status = create_cgroup(
            "reapply",
            CgroupSettings {
                cpu_quota: Some(CpuQuota::Percentage(50)),
                cpu_weight: Some(256),
            },
        );
        assert!(
            !status.created(),
            "an existing cgroup is reported as not created"
        );
        assert!(status.is_ok(), "the settings were re-applied");

        let quota = fs::read_to_string(&quota_file).expect("read back the quota");
        let shares = fs::read_to_string(&shares_file).expect("read back the shares");
        assert_ne!(
            quota.trim(),
            "0",
            "the quota must be re-applied to an existing cgroup, not left at what it was"
        );
        assert_ne!(
            shares.trim(),
            "1024",
            "the weight must be re-applied to an existing cgroup too"
        );
    }

    /// A single test drives the cache: `MANAGER` is process-global, so two tests
    /// mutating it would race.
    #[test]
    fn a_re_detection_reaches_the_cgroup_operations() {
        let _guard = MANAGER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let hierarchy = tempfile::tempdir().expect("a temporary hierarchy");

        // Pretend the daemon detected this cgroup v1 hierarchy.
        *MANAGER.write().expect("an uncontended lock") =
            Some(Arc::new(CgroupManager::new(CgroupInfo {
                version: CgroupVersion::V1,
                mount_point: PathBuf::from(hierarchy.path()),
            })));

        // An operation uses the cached detection, and creates in it.
        assert!(create_cgroup("cached", CgroupSettings::default()).created());
        assert!(
            hierarchy.path().join("cpu/cached").is_dir(),
            "the cgroup is created under the detected hierarchy"
        );
        assert_eq!(
            cached_info().map(|info| info.mount_point),
            Some(hierarchy.path().into())
        );

        // Re-detecting drops the cached manager along with the mount-table
        // cache, so the next operation has to rebuild from the live hierarchy
        // instead of reusing the injected one.
        reset_cgroup_detection();
        let _ = create_cgroup("after-reset", CgroupSettings::default());

        let rebuilt = cached_info().expect("the next operation rebuilds the manager");
        assert_ne!(
            rebuilt.mount_point,
            hierarchy.path(),
            "the manager must be rebuilt from the live detection, not the cached one"
        );
        assert!(
            !hierarchy.path().join("cpu/after-reset").exists(),
            "nothing is created in the hierarchy that was cached before the reset"
        );
    }

    /// `cgroup.events` accounts for processes in descendant cgroups too, so it
    /// wins over an empty `cgroup.procs`; a missing events file falls back to
    /// the procs list, and an unreadable procs file means "keep it".
    #[test]
    fn emptiness_checks_events_first_and_is_conservative_without_them() {
        let hierarchy = tempfile::tempdir().expect("a temporary hierarchy");
        let cgroup = hierarchy.path().join("cgroup.scope");
        fs::create_dir(&cgroup).expect("a cgroup");
        fs::write(cgroup.join("cgroup.procs"), "").expect("an empty procs file");

        assert!(cgroup_is_empty(&cgroup), "empty procs means empty");
        fs::write(cgroup.join("cgroup.events"), "populated 1\n").expect("events");
        assert!(
            !cgroup_is_empty(&cgroup),
            "a populated child is reported by events, not by the parent's procs"
        );
        fs::write(cgroup.join("cgroup.events"), "populated 0\n").expect("events");
        fs::write(cgroup.join("cgroup.procs"), "42\n").expect("procs");
        assert!(
            cgroup_is_empty(&cgroup),
            "events is authoritative even when procs disagrees"
        );
        fs::remove_file(cgroup.join("cgroup.events")).expect("remove events");
        assert!(!cgroup_is_empty(&cgroup));
        fs::remove_file(cgroup.join("cgroup.procs")).expect("remove procs");
        assert!(
            !cgroup_is_empty(&cgroup),
            "an unreadable procs file must never be mistaken for an empty cgroup"
        );
    }

    /// The registry is a bounded LRU, shared behind an `Arc`, and can be
    /// emptied between scenarios.
    #[test]
    fn the_created_registry_is_bounded_and_can_be_cleared() {
        clear_created_registry();
        for i in 0..(CREATED_REGISTRY_CAPACITY * 2) {
            record_created(
                &format!("cgroup-{i}"),
                Path::new(&format!("/tmp/cgroup-{i}")),
            );
        }
        assert_eq!(
            created_cgroup_names().len(),
            CREATED_REGISTRY_CAPACITY,
            "the registry must not grow without bound"
        );
        clear_created_registry();
        assert!(created_cgroup_names().is_empty());
    }
}
