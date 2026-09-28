//! What the daemon's own memory is doing, read from procfs and cgroupfs.
//!
//! This exists because the daemon had a failure mode that nothing else could
//! see. The shipped unit carried `MemoryHigh=16M` against a working set of about
//! 40M, and exceeding `MemoryHigh` does not fail anything — it makes the kernel
//! reclaim. This cgroup has almost nothing to reclaim but the daemon's own page
//! cache and its own heap, so every page the daemon touched was discarded and
//! fetched again, continuously. The result was 3.66 GB read from the root
//! filesystem in twenty minutes, 343 MB cycled through zram, 90% of all page
//! faults going to disk, and a daemon whose start-up took minutes because it was
//! busy refaulting its own executable.
//!
//! None of that is visible from inside the process. `/proc/self/status` says
//! `VmData: 24424 kB` and the log looks healthy. The counters that give it away
//! — `memory.events:high`, `pgmajfault`, `workingset_refault_file` — are in
//! cgroupfs, one directory above whatever the daemon can see of itself. This
//! module reads them so the daemon can report its own steady state, and a
//! regression of this shape shows up in the journal rather than needing
//! `cgroupfs` attached by hand.
//!
//! Every read here is optional. A missing or unreadable file leaves its field
//! `None` and is not an error: a cgroup v1 host, a container whose cgroup
//! namespace hides the path, and a `ProcSubset=pid` unit that cannot see
//! `/sys/fs/cgroup` at all all have to keep working, and none of them can be
//! allowed to take the daemon down for wanting to print a diagnostic.

use std::{
    fs::read_to_string,
    path::{Path, PathBuf},
    time::Duration,
};

/// One read of the daemon's own footprint.
///
/// The fields are the ones that distinguish "uses memory" from "is being made to
/// re-fetch it", because those look identical from `/proc/self/status` alone:
/// `VmRSS` and `VmSwap` together are the heap, and the difference between the
/// whole is what is resident.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The heap, from `VmData` in `/proc/self/status`. This is what the rule set
    /// is made of, and it is the number that decides whether the shipped
    /// `MemoryMax` is honest.
    pub vm_data: Option<u64>,
    /// Resident set, `VmRSS`.
    pub vm_rss: Option<u64>,
    /// Heap that has been pushed to swap, `VmSwap`. Non-zero with a large
    /// `vm_data` is the signature of a daemon being throttled rather than one
    /// that is busy.
    pub vm_swap: Option<u64>,
    /// The daemon's own mapped text, `VmExe`. Recoverable, so reclaim throws it
    /// away first.
    pub vm_exe: Option<u64>,
    /// Its shared libraries, `VmLib`. Also recoverable, also re-read from disk.
    pub vm_lib: Option<u64>,
    /// Total cgroup usage, `memory.current`.
    pub current: Option<u64>,
    /// `memory.high`. Exceeding it causes reclaim rather than failure, which is
    /// why it is reported next to `current` and not on its own.
    pub high: Option<u64>,
    /// `memory.max`. The hard cap.
    pub max: Option<u64>,
    /// Anonymous memory in the cgroup, from `memory.stat`.
    pub anon: Option<u64>,
    /// The cgroup's page cache — our own text, the rule files we read at
    /// start-up. What reclaim reaches for when `high` is exceeded.
    pub file: Option<u64>,
    /// Kernel memory charged to us: the BPF maps and the perf buffers. Not
    /// reclaimable, so it raises the floor under everything else.
    pub kernel: Option<u64>,
    /// Slab specifically, so the kernel figure above can be attributed.
    pub slab: Option<u64>,
    /// Page faults that needed a disk read, `pgmajfault`. The number that
    /// actually mattered: a healthy daemon is nearly zero, and a high ratio
    /// against `pgfault` means it is being made to re-read itself.
    pub pgmajfault: Option<u64>,
    /// All page faults, `pgfault`.
    pub pgfault: Option<u64>,
    /// File pages re-faulted, `workingset_refault_file` — a page evicted and
    /// then read again rather than kept.
    pub refault_file: Option<u64>,
    /// Anonymous pages re-faulted, `workingset_refault_anon`.
    pub refault_anon: Option<u64>,
    /// Pages swapped in, `pswpin`.
    pub pswpin: Option<u64>,
    /// Pages swapped out, `pswpout`.
    pub pswpout: Option<u64>,
    /// `memory.events:high` — how many times the cgroup has been told to
    /// reclaim. The single clearest indicator that `MemoryHigh` is set below the
    /// working set; it climbs without bound and means nothing on its own.
    pub high_events: Option<u64>,
    /// `memory.events:max` — how many times the hard cap was hit.
    pub max_events: Option<u64>,
    /// Bytes read by the cgroup, from `io.stat`. Sums every device the cgroup
    /// touched, so a nested device manager and the disk under it are both
    /// counted; the magnitude is the point, not the exact figure.
    pub read_bytes: Option<u64>,
    /// Bytes written by the cgroup, from `io.stat`.
    pub written_bytes: Option<u64>,
    /// The cgroup directory these came from, or `None` where there is none.
    pub cgroup: Option<PathBuf>,
}

/// Reads one `Name:  1234 kB` line out of `/proc/<pid>/status`.
///
/// `VmData`, `VmRSS` and the rest are reported in kibibytes by every kernel that
/// has them, and a field this kernel does not have is simply absent — so a
/// missing one is `None` rather than a zero, because "not reported" and "empty"
/// are different answers and only one of them is a problem.
fn status_kb(status: &str, field: &str) -> Option<u64> {
    status.lines().find_map(|line| {
        let rest = line.strip_prefix(field)?.strip_prefix(':')?;
        rest.split_whitespace().next()?.parse().ok()
    })
}

fn read_status_field(field: &str) -> Option<u64> {
    let status = read_to_string("/proc/self/status").ok()?;
    status_kb(&status, field)
}

/// One `key value` line from a cgroup `memory.stat`.
fn stat_field(contents: &str, key: &str) -> Option<u64> {
    contents.lines().find_map(|line| {
        let (name, value) = line.split_once(' ')?;
        (name == key).then(|| value.parse().ok())?
    })
}

fn read_stat_field(cgroup: &Path, key: &str) -> Option<u64> {
    stat_field(&read_to_string(cgroup.join("memory.stat")).ok()?, key)
}

/// One `key value` line from a cgroup `memory.events`.
///
/// `memory.events` is a flat `low`, `high`, `max`, `oom` set, so the key is the
/// whole first field. A `None` here means this kernel has no such event type,
/// which for the ones read here means a cgroup v1 host — where the equivalent
/// counter is `memory.failcnt` under a different name and a different meaning.
fn event_field(contents: &str, key: &str) -> Option<u64> {
    contents.lines().find_map(|line| {
        let (name, value) = line.split_once(' ')?;
        (name == key).then(|| value.trim().parse().ok())?
    })
}

/// One `key=value` counter from a cgroup `io.stat`, summed over every device.
///
/// `io.stat` has one line per block device and the key appears on each, so
/// summing is the only way to get the cgroup's total rather than one device's
/// share. A cgroup reading through a device mapper layer is charged to both the
/// mapper and the disk beneath it, so the total is an upper bound on the traffic
/// rather than an exact count of it — which is fine, because the magnitude is
/// what distinguishes a daemon reading its own pages from one idling, and an
/// over-count that never under-counts is the safe direction to be wrong in.
fn io_sum(contents: &str, key: &str) -> Option<u64> {
    let total: u64 = contents
        .lines()
        .map(|line| {
            line.split_whitespace()
                .find_map(|token| {
                    let (name, value) = token.split_once('=')?;
                    (name == key).then(|| value.parse::<u64>().ok())?
                })
                .unwrap_or(0)
        })
        .sum();
    (total > 0).then_some(total)
}

/// The `0::` line of `/proc/self/cgroup`, which is where a unified hierarchy
/// publishes our own path.
///
/// Returns `None` on a cgroup v1 host, where no such line exists — the v1 layout
/// puts the path on a per-controller line and the memory controller's is a
/// different directory, which is not worth reconstructing for a diagnostic.
fn own_cgroup_path() -> Option<PathBuf> {
    let cgroup = read_to_string("/proc/self/cgroup").ok()?;
    let path = cgroup.lines().find_map(|line| line.strip_prefix("0::"))?;
    let path = path.trim();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from("/sys/fs/cgroup").join(path.trim_start_matches('/')))
    }
}

/// A kibibyte figure from `/proc/self/status`, in the unit a reader wants.
///
/// `None` is "this kernel did not report it", which is not the same as zero and
/// is rendered so it cannot be mistaken for either.
fn kib(value: Option<u64>) -> String {
    match value {
        Some(kb) => format!("{:.1}M", kb as f64 / 1024.0),
        None => "n/a".to_string(),
    }
}

/// A byte figure from cgroupfs, in MiB.
fn mib(value: Option<u64>) -> String {
    match value {
        Some(bytes) => format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0)),
        None => "n/a".to_string(),
    }
}

/// A counter, in the unit the kernel counted it in.
fn count(value: Option<u64>) -> String {
    value.map(|value| value.to_string()).unwrap_or("n/a".into())
}

impl Snapshot {
    /// Reads the current footprint.
    ///
    /// Reads about a dozen small files. Called at start-up and then at most
    /// once a minute, so the cost is not worth optimising away and each read is
    /// treated as optional — see the module comment.
    pub fn read() -> Self {
        let cgroup = own_cgroup_path();
        let io = cgroup
            .as_ref()
            .and_then(|dir| read_to_string(dir.join("io.stat")).ok())
            .unwrap_or_default();
        let events = cgroup
            .as_ref()
            .and_then(|dir| read_to_string(dir.join("memory.events")).ok())
            .unwrap_or_default();

        Snapshot {
            vm_data: read_status_field("VmData"),
            vm_rss: read_status_field("VmRSS"),
            vm_swap: read_status_field("VmSwap"),
            vm_exe: read_status_field("VmExe"),
            vm_lib: read_status_field("VmLib"),
            current: cgroup
                .as_ref()
                .and_then(|dir| read_to_string(dir.join("memory.current")).ok())
                .and_then(|value| value.trim().parse().ok()),
            high: cgroup
                .as_ref()
                .and_then(|dir| read_to_string(dir.join("memory.high")).ok())
                .and_then(|value| value.trim().parse().ok()),
            max: cgroup
                .as_ref()
                .and_then(|dir| read_to_string(dir.join("memory.max")).ok())
                .and_then(|value| value.trim().parse().ok()),
            anon: cgroup.as_ref().and_then(|dir| read_stat_field(dir, "anon")),
            file: cgroup.as_ref().and_then(|dir| read_stat_field(dir, "file")),
            kernel: cgroup
                .as_ref()
                .and_then(|dir| read_stat_field(dir, "kernel")),
            slab: cgroup.as_ref().and_then(|dir| read_stat_field(dir, "slab")),
            pgmajfault: cgroup
                .as_ref()
                .and_then(|dir| read_stat_field(dir, "pgmajfault")),
            pgfault: cgroup
                .as_ref()
                .and_then(|dir| read_stat_field(dir, "pgfault")),
            refault_file: cgroup
                .as_ref()
                .and_then(|dir| read_stat_field(dir, "workingset_refault_file")),
            refault_anon: cgroup
                .as_ref()
                .and_then(|dir| read_stat_field(dir, "workingset_refault_anon")),
            pswpin: cgroup
                .as_ref()
                .and_then(|dir| read_stat_field(dir, "pswpin")),
            pswpout: cgroup
                .as_ref()
                .and_then(|dir| read_stat_field(dir, "pswpout")),
            high_events: event_field(&events, "high"),
            max_events: event_field(&events, "max"),
            read_bytes: io_sum(&io, "rbytes"),
            written_bytes: io_sum(&io, "wbytes"),
            cgroup,
        }
    }

    /// One line for a log, in the units a reader wants.
    ///
    /// Bytes become MiB and counters stay counts: the byte figures are compared
    /// against a memory limit, and the counters are compared against each other
    /// and against their own previous line.
    pub fn summary(&self) -> String {
        format!(
            "heap={} rss={} swap={} text={} libs={} | cgroup={} high={} max={} \
             anon={} file={} kernel={} slab={} | faults={}/{} refault=file:{}/anon:{} \
             swapio=out:{}/in:{} | throttled={} capped={} | io=read:{}/written:{}",
            kib(self.vm_data),
            kib(self.vm_rss),
            kib(self.vm_swap),
            kib(self.vm_exe),
            kib(self.vm_lib),
            mib(self.current),
            mib(self.high),
            mib(self.max),
            mib(self.anon),
            mib(self.file),
            mib(self.kernel),
            mib(self.slab),
            count(self.pgfault),
            count(self.pgmajfault),
            count(self.refault_file),
            count(self.refault_anon),
            count(self.pswpout),
            count(self.pswpin),
            count(self.high_events),
            count(self.max_events),
            mib(self.read_bytes),
            mib(self.written_bytes),
        )
    }

    /// What the numbers mean, when they mean something.
    ///
    /// Kept separate from the numbers so that adding a counter does not also
    /// mean rewriting prose, and so the prose can be tested. Only conditions
    /// worth a line in the log at all are reported: a diagnostic that fires
    /// routinely teaches the reader to skip it.
    pub fn findings(&self) -> Vec<String> {
        let mut findings = Vec::new();

        if let (Some(current), Some(high)) = (self.current, self.high)
            && high > 0
            && current > high
        {
            findings.push(format!(
                "cgroup usage {:.1}M is above memory.high {:.1}M, so the kernel is \
                 reclaiming this daemon's own pages continuously -- memory.high is a \
                 throttle point, not a limit, and the only thing this cgroup can reclaim \
                 is the daemon itself. Raise or remove memory.high.",
                mib(Some(current)),
                mib(Some(high))
            ));
        }

        if let (Some(total), Some(major)) = (self.pgfault, self.pgmajfault)
            && total > 0
            && major * 10 > total
        {
            findings.push(format!(
                "{} of {} page faults went to disk ({:.0}%), so the daemon is re-reading \
                 its own mapped files rather than keeping them",
                major,
                total,
                major as f64 * 100.0 / total as f64
            ));
        }

        if let Some(swapped) = self.vm_swap
            && swapped > 1024
            && let Some(heap) = self.vm_data
            && heap > 0
        {
            findings.push(format!(
                "{:.1}M of a {:.1}M heap is in swap; a daemon that tunes processes should \
                 not be swapped out from under itself",
                kib(Some(swapped)),
                kib(Some(heap))
            ));
        }

        if let Some(throttled) = self.high_events
            && throttled > 0
        {
            findings.push(format!(
                "memory.events reports {throttled} reclaim events and {} at the hard cap",
                self.max_events.unwrap_or(0)
            ));
        }

        findings
    }
}

impl std::fmt::Display for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.summary())
    }
}

/// How often the periodic report is written.
pub const REPORT_INTERVAL: Duration = Duration::from_secs(60);

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `/proc/self/status`, in the `Name:\t1234 kB` shape the kernel
    /// writes. Only the fields this module reads are present, which is also the
    /// point: an absent field has to be `None` and not zero.
    const STATUS: &str = "\
Name:\tananicy-rs
State:\tS (sleeping)
VmPeak:\t  233624 kB
VmSize:\t  233612 kB
VmRSS:\t    7036 kB
RssAnon:\t    1708 kB
RssFile:\t    5328 kB
VmData:\t   24424 kB
VmStk:\t      132 kB
VmExe:\t     1616 kB
VmLib:\t     5356 kB
VmSwap:\t    16512 kB
Threads:\t\t4
";

    #[test]
    fn a_kibibyte_field_is_read_out_of_status() {
        assert_eq!(status_kb(STATUS, "VmData"), Some(24_424));
        assert_eq!(status_kb(STATUS, "VmRSS"), Some(7_036));
        assert_eq!(status_kb(STATUS, "VmSwap"), Some(16_512));
        assert_eq!(status_kb(STATUS, "VmExe"), Some(1_616));
    }

    /// A kernel that does not report a field leaves it out entirely, and zero
    /// would be a claim about a value the kernel never gave.
    #[test]
    fn a_field_the_kernel_does_not_report_is_absent_not_zero() {
        assert_eq!(status_kb(STATUS, "VmPTE"), None);
        assert_eq!(status_kb("", "VmData"), None);
    }

    /// `VmData` must not match `VmDataFoo`, and neither must `RssAnon` match
    /// `VmRSS` — the prefix is a real hazard here because the kernel's own field
    /// names share it.
    #[test]
    fn a_field_name_is_matched_exactly() {
        let status = "VmData:\t100 kB\nVmDataExtra:\t999 kB\n";
        assert_eq!(status_kb(status, "VmData"), Some(100));
    }

    #[test]
    fn a_memory_stat_field_is_read_out() {
        let stat = "anon 1748992\nfile 2031616\nkernel 5169152\nslab 3926568\n";
        assert_eq!(stat_field(stat, "anon"), Some(1_748_992));
        assert_eq!(stat_field(stat, "slab"), Some(3_926_568));
        assert_eq!(stat_field(stat, "nonesuch"), None);
    }

    #[test]
    fn a_memory_events_field_is_read_out() {
        let events = "low 0\nhigh 126903\nmax 0\noom 0\n";
        assert_eq!(event_field(events, "high"), Some(126_903));
        assert_eq!(event_field(events, "max"), Some(0));
        assert_eq!(event_field(events, "oom"), Some(0));
        assert_eq!(event_field(events, "nonesuch"), None);
    }

    /// `io.stat` is per-device, and this daemon's reads arrive on several: a
    /// zram device, a device mapper layer and the disk under it. Summing them is
    /// deliberate — a nested device manager and the disk beneath it are both
    /// charged, so the total is an upper bound on the traffic, and the
    /// magnitude is what the diagnostic is for.
    #[test]
    fn io_stat_is_summed_across_devices() {
        // The figures from the reported incident: zram first, then the disk
        // under a device mapper layer, then the bare device.
        let io = "252:0 rbytes=332382208 wbytes=351453184 rios=81148 wios=85804\n\
                  259:0 rbytes=3393539072 wbytes=0 rios=54642 wios=0\n\
                  253:0 rbytes=3663162368 wbytes=0 rios=59278 wios=0\n";
        assert_eq!(
            io_sum(io, "rbytes"),
            Some(332_382_208 + 3_393_539_072 + 3_663_162_368)
        );
        assert_eq!(io_sum(io, "wbytes"), Some(351_453_184));
        assert_eq!(io_sum(io, "dbytes"), None);
    }

    /// A cgroup that has done no I/O reports zero on every device, which is not
    /// the same as having no `io.stat` to read. The distinction is carried by
    /// `None` for the second and `Some(0)` for the first, and it matters only
    /// for the line's honesty — no branch turns on it.
    #[test]
    fn an_empty_io_stat_is_reported_as_absent() {
        assert_eq!(io_sum("", "rbytes"), None);
    }

    /// The whole reason the module exists: a cgroup sitting above its own
    /// `memory.high` is being made to re-fetch its pages, and that has to be
    /// reported rather than left in cgroupfs.
    #[test]
    fn usage_above_memory_high_is_reported() {
        let snapshot = Snapshot {
            current: Some(18_612_224),
            high: Some(16_777_216),
            ..Snapshot::default()
        };
        let findings = snapshot.findings();
        assert!(
            findings.iter().any(|f| f.contains("memory.high")),
            "a daemon being throttled must say so: {findings:?}"
        );
    }

    /// The exact numbers from the reported incident, so the report is known to
    /// recognise them.
    #[test]
    fn the_thrashing_daemon_is_diagnosed() {
        let snapshot = Snapshot {
            vm_data: Some(24_424),
            vm_swap: Some(16_512),
            pgfault: Some(96_048),
            pgmajfault: Some(86_413),
            high_events: Some(126_903),
            ..Snapshot::default()
        };
        let findings = snapshot.findings();
        assert!(
            findings.iter().any(|f| f.contains("went to disk")),
            "a 90% major-fault rate is the signature: {findings:?}"
        );
        assert!(
            findings.iter().any(|f| f.contains("in swap")),
            "a swapped-out heap is the other half: {findings:?}"
        );
        assert!(
            findings.iter().any(|f| f.contains("reclaim events")),
            "the throttle counter is the clearest single indicator: {findings:?}"
        );
    }

    /// A daemon that is merely using memory is not a finding. A diagnostic that
    /// fires routinely teaches the reader to skip it.
    #[test]
    fn an_idle_daemon_reports_nothing() {
        let snapshot = Snapshot {
            vm_data: Some(24_424),
            vm_rss: Some(24_424),
            vm_swap: None,
            pgfault: Some(96_048),
            pgmajfault: Some(12),
            high_events: Some(0),
            current: Some(30_000_000),
            high: Some(134_217_728),
            ..Snapshot::default()
        };
        assert_eq!(snapshot.findings(), Vec::<String>::new());
    }

    /// `memory.high` reads `max` when no limit is set, so "above the limit" has
    /// to mean "above a real number".
    #[test]
    fn an_unlimited_memory_high_is_not_treated_as_a_limit() {
        let snapshot = Snapshot {
            current: Some(18_612_224),
            high: Some(u64::MAX),
            ..Snapshot::default()
        };
        assert!(snapshot.findings().is_empty());
    }

    /// Every field missing — a cgroup v1 host, a sandbox with no cgroupfs at
    /// all — must produce a readable line and no findings, not a panic and not a
    /// wall of `n/a` on top of a panic.
    #[test]
    fn an_empty_snapshot_is_still_reportable() {
        let snapshot = Snapshot::default();
        let summary = snapshot.summary();
        assert!(summary.contains("n/a"));
        assert!(snapshot.findings().is_empty());
    }

    /// The reading of the live daemon has to be safe to call on a host that has
    /// none of this, which is the only way to know it does not panic there.
    #[test]
    fn reading_this_process_works() {
        let snapshot = Snapshot::read();
        // `/proc/self/status` is always there, so the heap always is too.
        assert!(snapshot.vm_data.is_some());
        assert!(!snapshot.summary().is_empty());
    }
}
