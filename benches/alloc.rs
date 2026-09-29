//! Live heap accounting for a rule load.
//!
//! The number this exists to produce is the one `docs/MEMORY.md` keeps in a
//! table: **live bytes and live allocations per rule** for a fully loaded rule
//! set. Peak RSS cannot produce it. `MEMORY.md`'s own note says so directly —
//! the peak is set by the transient work of loading, and the allocator hands
//! those freed chunks straight to the live allocations, so a freed-then-reused
//! page was already resident and the change never shows up. A 15,831-rule set
//! moved 260 kB of peak RSS for the 760 kB that a real fix removed.
//!
//! So this wraps the system allocator and counts the two things that are
//! actually being asked about:
//!
//! * **live bytes** — every byte currently allocated and not yet freed, tracked
//!   by a counter incremented on `alloc` and decremented on `dealloc`. This is
//!   a delta measured around the load, not an absolute: whatever criterion, the
//!   formatter and the tracing subscriber allocated before the measurement
//!   opened is already in the counter and cancels out.
//! * **live allocations** — the same, counted in pieces rather than bytes. This
//!   is the column that catches the bug `MEMORY.md:91-96` documents, where a
//!   fix was invisible in the struct size and in peak RSS and was worth 14.5% of
//!   the rule set on its own.
//!
//! Both are read through [`AllocationSnapshot`], which `reset`s the counters
//! before the measured region so nothing outside it leaks in.
//!
//! The counters are relaxed atomics updated on every allocation in the
//! process. That is not free, and it is deliberate: a benchmark that perturbs
//! the thing it measures less is worth more than a fast one. The rule load
//! being measured does ~15,800 allocations, so the counter updates are noise
//! next to the parse work they sit between, and this bench measures *bytes*,
//! not time — `benches/rules.rs` owns timing.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
};

/// Bytes handed out and not yet returned, across every thread.
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
/// Allocations handed out and not yet freed, across every thread.
static LIVE_ALLOCS: AtomicUsize = AtomicUsize::new(0);

/// A counting wrapper around the system allocator.
///
/// The counters are process-global because a `GlobalAlloc` is: there is no way
/// to scope one, and a per-thread counter would need a TLS shim that itself
/// allocates. Since the measured region is single-threaded — a rule load
/// happens on one thread — process-global is exact for it, and this bench
/// measures one.
struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `layout` is forwarded unmodified, and the counters below
        // touch no memory of their own. `System::alloc` is the allocator this
        // replaces, so the pointer it returns is one it will also free.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            LIVE_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
            LIVE_ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` and `layout` are exactly what `alloc` was given, which
        // is the contract `GlobalAlloc::dealloc` requires.
        unsafe { System.dealloc(ptr, layout) };
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        LIVE_ALLOCS.fetch_sub(1, Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarded unchanged, per `GlobalAlloc::realloc`'s contract.
        // A realloc is a free of the old block and an allocation of the new
        // one, so the byte delta is `new_size - layout.size()` and the count is
        // unchanged. Charging it as a fresh allocation instead would report a
        // realloc as a leak, which is the failure mode that would make this
        // bench quietly wrong rather than obviously wrong.
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            LIVE_BYTES.fetch_add(new_size, Ordering::Relaxed);
            LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        new_ptr
    }
}

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

/// A reading of the two counters, taken as a difference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AllocationSnapshot {
    bytes: usize,
    allocs: usize,
}

impl AllocationSnapshot {
    fn reset() -> Self {
        Self {
            bytes: LIVE_BYTES.load(Ordering::Relaxed),
            allocs: LIVE_ALLOCS.load(Ordering::Relaxed),
        }
    }

    /// The change since `self`, which is what a measured region cost.
    fn since(&self) -> Self {
        Self {
            bytes: LIVE_BYTES
                .load(Ordering::Relaxed)
                .saturating_sub(self.bytes),
            allocs: LIVE_ALLOCS
                .load(Ordering::Relaxed)
                .saturating_sub(self.allocs),
        }
    }
}

/// One line of the `docs/MEMORY.md` table.
struct Footprint {
    bytes_per_rule: f64,
    allocs_per_rule: f64,
}

impl Footprint {
    fn of(snapshot: AllocationSnapshot, rules: usize) -> Self {
        Self {
            bytes_per_rule: snapshot.bytes as f64 / rules as f64,
            allocs_per_rule: snapshot.allocs as f64 / rules as f64,
        }
    }

    fn report(&self, label: &str, rules: usize) {
        println!(
            "{label:<34} {rules:>7} rules  {:>10.0} B/rule  {:>6.2} allocs/rule",
            self.bytes_per_rule, self.allocs_per_rule
        );
    }
}

/// A rule set shaped like the shipped one.
///
/// Generated rather than read from `/etc/ananicy.d`, because a benchmark that
/// depends on what happens to be installed on the machine running it measures
/// the machine rather than the code, and stops being comparable the day the
/// upstream rules change. The shape is what matters here: a couple of dozen
/// distinct type names shared across every rule, a handful of distinct
/// `sched`/`ioclass`/`cgroup` values, and a distribution of which attributes
/// each rule actually carries — a real rule set is overwhelmingly `type` plus
/// `nice`, which is precisely the case the interning work in
/// `rules.rs` (`NAME_TABLE`) exists to make cheap.
fn shipped_shape(rule_count: usize) -> Vec<String> {
    const TYPES: [&str; 14] = [
        "BG_CPUIO",
        "Chat",
        "Doc-View",
        "Game",
        "Heavy_CPU",
        "Image-View",
        "IN_DIFF",
        "Launcher",
        "LowLatency_RT",
        "OOM_NO_KILL",
        "Player-Audio",
        "Player-Video",
        "Service",
        "TODO",
    ];

    (0..rule_count)
        .map(|i| {
            let type_name = TYPES[i % TYPES.len()];
            match i % 10 {
                // The common shape: a type and a nice, and nothing else.
                0..=6 => format!(r#"{{"name":"proc{i}","type":"{type_name}","nice":{}}}"#, i % 20),
                7 | 8 => format!(
                    r#"{{"name":"svc{i}","type":"{type_name}","sched":"other","ionice":4}}"#
                ),
                _ => format!(
                    r#"{{"name":"app{i}","type":"{type_name}","ioclass":"best-effort","cgroup":"ananicy.slice"}}"#
                ),
            }
        })
        .collect()
}

/// Loads `rule_count` rules and reports what they cost to hold.
///
/// The rule set is built in an inner scope so that the only thing the delta
/// measures is the rule set itself and not the `Vec<String>` the source lines
/// are held in. Those are dropped before the snapshot is read.
fn measure(rule_count: usize, label: &str) -> Footprint {
    let source = shipped_shape(rule_count);
    let config = std::sync::Arc::new(ananicy_core::config::Config::new(
        ananicy_core::config::ConfigSnapshot::default(),
    ));

    // Read the starting point only after everything above has been built, so
    // the JSON strings above are not counted.
    let before = AllocationSnapshot::reset();
    let rules = {
        let mut rules = ananicy_core::rules::Rules::new(config);
        for line in &source {
            rules.load_rule_from_string(line);
        }
        rules
    };
    let after = before.since();

    assert_eq!(
        rules.size(),
        rule_count,
        "every generated line has to have loaded, or the per-rule figure is \
         silently computed over the wrong number of rules"
    );

    // Holding `rules` to here rather than dropping it is the point: the
    // footprint of a rule set is what it costs to *keep*, not to load.
    let footprint = Footprint::of(after, rule_count);
    footprint.report(label, rule_count);
    drop(rules);
    footprint
}

fn main() {
    println!(
        "\nlive heap for a loaded rule set, as {} sees it\n\
         (docs/MEMORY.md keeps the same two columns for its own layouts)\n",
        std::env::args()
            .next()
            .unwrap_or_else(|| "the allocator".into())
    );

    let small = measure(15_831, "shipped default set");
    let large = measure(120_000, "large ruleset (MEMORY.md row)");

    println!();
    // Not a gate. A regression in bytes-per-rule is caught by review and by
    // reading the table above; asserting on it here would make a legitimate
    // change to the rule format look like a broken build, and a test that
    // fails for the wrong reason trains people to re-run it rather than read
    // it. The one thing worth asserting is that the per-rule figure does not
    // grow between a small and a large set — the per-rule cost is supposed to
    // be constant, and a structure that pays a per-rule price only past some
    // threshold is exactly the failure this bench exists to see.
    assert!(
        large.bytes_per_rule < small.bytes_per_rule * 1.5,
        "bytes per rule grew from {:.0} to {:.0} between a 15,831-rule and a \
         120,000-rule set; per-rule cost is meant to be constant, so something \
         is paying a per-rule price only past a size threshold",
        small.bytes_per_rule,
        large.bytes_per_rule
    );
}
