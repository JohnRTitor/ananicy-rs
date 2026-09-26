use {
    ananicy_core::config::{Config, ConfigSnapshot},
    std::sync::Arc,
};

use {
    ananicy_core::{cpuset::CpuSet, rules::Rules},
    criterion::{Criterion, black_box, criterion_group, criterion_main},
};

/// The `name_regex` shapes that appear in ananicy rules, plus the ones a
/// community ruleset plausibly adds. Kept here rather than in the daemon so
/// the benchmark and the compatibility tests can be compared directly.
const NAME_REGEX_RULES: &[&str] = &[
    r"^java[0-9.]*$",
    r"^(java|javaw)[0-9.]*$",
    r"^bash(?!_script)",
    r"^app.*$",
    r"gcc-.*",
    r"^Steam.*$",
    r"^wine.*$",
    r"^.*-wrapped$",
    r"^\.?gnome-keyring-d$",
    r"^kworker/[0-9]+-[0-9]+$",
    r"^(?:gimp|inkscape|blender)$",
    r"^firefox(-bin|-esr)?$",
    r"^systemd-(?:journald|udevd|logind|resolved|timesyncd)$",
    r"^.*\.exe$",
    r"^VirtualBox(Headless)?VM$",
    r"^sshd(-session)?$",
    r"^m?\w+ode$",
];

/// Process-name-shaped subjects. A real haystack is 3-15 bytes in almost every
/// case — the kernel truncates `/proc/<pid>/comm` to 15 — with a long tail from
/// `argv[0]` basenames, so the corpus is weighted towards short names and
/// carries one name per rule's first literal so matches actually land.
fn subjects() -> Vec<String> {
    let mut names: Vec<String> = [
        "init",
        "systemd",
        "bash",
        "sshd",
        "java",
        "java17",
        "javaw",
        "app",
        "app-helper",
        "gcc-aarch64",
        "Steam",
        "wine",
        "gcc",
        "gnome-keyring-d",
        "kworker/0-1",
        "gimp",
        "firefox",
        "firefox-esr",
        "systemd-journald",
        "setup.exe",
        "VirtualBoxVM",
        "VirtualBoxHeadlessVM",
        "m",
        "mode",
        "node",
        "code",
        "python3",
        "pipewire",
        "Xorg",
        "kworker",
        "rustc",
        "cc1",
        "ananicy-rs",
        "dbus-daemon",
        "polkitd",
        "rtkit-daemon",
        "chrome",
        "pulseaudio",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();

    // The worst case for a literal prefilter: a name sharing no byte with the
    // pattern, short and long.
    names.push("x".repeat(255));
    names.push("q".repeat(64));
    names
}

/// A pool larger than the 5 000-entry lookup cache, so cycling through it
/// forces a cache *miss* on almost every iteration. `get_rule` memoises its
/// answer, so a benchmark that reuses a handful of names measures hash
/// bookkeeping and never reaches the regex loop at all — which is what
/// `rules_get_cache_miss` below does, and why it is not the interesting number.
fn miss_pool() -> Vec<String> {
    let mut pool: Vec<String> = subjects();
    let base = pool.len();
    for i in 0..8_000 {
        pool.push(format!("{}-{}", pool[i % base], i));
    }
    pool
}

/// Loads `count` rules that each carry a `name_regex`, all with distinct
/// `name`s so the exact-name map cannot short-circuit the regex fallback.
fn rules_with(count: usize) -> Rules {
    let mut rules = Rules::new(Arc::new(Config::new(ConfigSnapshot::default())));
    for i in 0..count {
        let pattern = NAME_REGEX_RULES[i % NAME_REGEX_RULES.len()];
        assert!(rules.load_rule_from_string(&format!(
            r#"{{ "name": "rule-{i}", "name_regex": {pattern:?}, "nice": {i} }}"#
        )));
    }
    rules
}

fn bench_cpuset_parse(c: &mut Criterion) {
    c.bench_function("cpuset_parse", |b| {
        b.iter(|| CpuSet::parse(black_box("0-3,8-11"), black_box(8192)))
    });
}

fn bench_rules_match(c: &mut Criterion) {
    let rules = Rules::new(Arc::new(Config::new(ConfigSnapshot::default())));

    // Test cache miss performance
    c.bench_function("rules_get_cache_miss", |b| {
        b.iter(|| rules.get_rule(black_box("nonexistent_process")))
    });
}

/// The regex path, which `rules_get_cache_miss` never reaches because it runs
/// against an empty rule set.
fn bench_rules_regex(c: &mut Criterion) {
    let pool = miss_pool();

    for count in [1usize, 10, 100] {
        let rules = rules_with(count);
        let mut group = c.benchmark_group(format!("rules_regex_{count}_rules"));

        // One uncached lookup: exact-map probe, then every `name_regex` in
        // load order. This is the netlink per-exec cost and the manual-scan
        // per-process cost.
        group.bench_function("lookup_miss", |b| {
            let mut i = 0usize;
            b.iter(|| {
                let name = &pool[i % pool.len()];
                i += 1;
                rules.get_rule(black_box(name))
            })
        });

        group.finish();
    }
}

/// A pattern is compiled exactly once, when its rule is loaded, so a compile
/// regression shows up here and on `--reload` rather than in matching.
fn bench_rules_regex_compile(c: &mut Criterion) {
    let mut group = c.benchmark_group("rules_regex_compile");
    for pattern in [
        r"^java[0-9.]*$",
        r"^(java|javaw)[0-9.]*$",
        r"^bash(?!_script)",
        r"gcc-.*",
    ] {
        group.bench_function(pattern, |b| {
            b.iter(|| {
                let mut rules = Rules::new(Arc::new(Config::new(ConfigSnapshot::default())));
                rules.load_rule_from_string(&format!(
                    r#"{{ "name": "x", "name_regex": {pattern:?}, "nice": 1 }}"#
                ));
                black_box(rules.size())
            })
        });
    }
    group.finish();
}

/// The cost of a *cold* `ProcfsScanner::full_scan`: every process on the host
/// looked up with nothing memoised. This is the worst case — the first scan
/// after start-up or a `--reload` — and the only one where the regex loop runs
/// for every process. `iter_batched` builds the rule set in setup so the
/// measurement is lookups only; every scan after this one hits the cache, which
/// is why `lookup_miss` above exists as a separate figure.
fn bench_rules_regex_full_scan(c: &mut Criterion) {
    let names = subjects();

    for count in [1usize, 10, 100] {
        let mut group = c.benchmark_group(format!("rules_regex_{count}_rules"));
        group.bench_function("cold_full_scan", |b| {
            b.iter_batched(
                || rules_with(count),
                |rules| {
                    for name in &names {
                        rules.get_rule(black_box(name));
                    }
                },
                criterion::BatchSize::SmallInput,
            )
        });
        group.finish();
    }
}

criterion_group!(
    benches,
    bench_cpuset_parse,
    bench_rules_match,
    bench_rules_regex,
    bench_rules_regex_full_scan,
    bench_rules_regex_compile
);
criterion_main!(benches);
