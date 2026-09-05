use {
    ananicy_core::{cpuset::CpuSet, rules::Rules},
    criterion::{Criterion, black_box, criterion_group, criterion_main},
};

fn bench_cpuset_parse(c: &mut Criterion) {
    c.bench_function("cpuset_parse", |b| {
        b.iter(|| CpuSet::parse(black_box("0-3,8-11"), black_box(8192)))
    });
}

fn bench_rules_match(c: &mut Criterion) {
    let rules = Rules::new(std::sync::Arc::new(ananicy_core::config::Config::new(
        ananicy_core::config::ConfigSnapshot::default(),
    )));

    // Test cache miss performance
    c.bench_function("rules_get_cache_miss", |b| {
        b.iter(|| rules.get_rule(black_box("nonexistent_process")))
    });
}

criterion_group!(benches, bench_cpuset_parse, bench_rules_match);
criterion_main!(benches);
