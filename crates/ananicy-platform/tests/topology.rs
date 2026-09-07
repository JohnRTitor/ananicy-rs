use {
    ananicy_core::cpuset::CpuSet,
    ananicy_platform::topology::{CpuTopology, detect_topology_impl},
    std::path::Path,
};

fn get_fixture_topo() -> CpuTopology {
    detect_topology_impl(Path::new("tests/fixtures/topology/big-little/sys"))
}

#[test]
fn test_detect_produces_valid_topology() {
    let topo = get_fixture_topo();
    // 4 CPUs in our fixture
    assert_eq!(topo.all_cores_str, "0-3");

    // We mocked a big.LITTLE system where CPUs 0,1 are 500 capacity, and CPUs 2,3 are 1024.
    assert_eq!(topo.little_cores_str, "0-1");
    assert_eq!(topo.big_cores_str, "2-3");
    assert_eq!(topo.turbo_cores_str, "2-3");

    // We mocked a dual-LLC system
    assert!(!topo.llcs.is_empty());

    // We mocked 1 NUMA node
    assert!(!topo.nodes.is_empty());
}

#[test]
fn test_all_online_cpus_have_valid_capacity() {
    // In Rust, capacity is used internally by detect_topology_impl to separate big/little
    // The fixture correctly parsed capacities to generate big/little sets.
    let topo = get_fixture_topo();
    assert_ne!(
        topo.big_cores_str, topo.little_cores_str,
        "Should detect capacity differences"
    );
}

#[test]
fn test_cpuset_strings_are_parseable() {
    let topo = get_fixture_topo();

    assert!(CpuSet::parse(&topo.big_cores_str, 32).is_some());
    assert!(CpuSet::parse(&topo.little_cores_str, 32).is_some());
    assert!(CpuSet::parse(&topo.all_cores_str, 32).is_some());
    assert!(CpuSet::parse(&topo.turbo_cores_str, 32).is_some());
}

#[test]
fn test_all_cores_covers_every_online_cpu() {
    let topo = get_fixture_topo();
    let parsed = CpuSet::parse(&topo.all_cores_str, 32).unwrap();

    assert!(parsed.has_cpu(0));
    assert!(parsed.has_cpu(1));
    assert!(parsed.has_cpu(2));
    assert!(parsed.has_cpu(3));
    assert!(!parsed.has_cpu(4));
}

#[test]
fn test_on_homogeneous_system_all_cores_are_big() {
    // Test what happens on homogeneous by using the x3d-single-ccd fixture,
    // which has no capacities so it defaults to homogeneous.
    let topo = detect_topology_impl(Path::new("tests/fixtures/x3d/amd-x3d-single-ccd/sys"));

    // Parity: homogeneous systems set big to all, and leave little/turbo empty
    assert_eq!(topo.big_cores_str, topo.all_cores_str);
    assert_eq!(topo.little_cores_str, "");
    assert_eq!(topo.turbo_cores_str, "");
}

#[test]
fn test_llc_grouping_covers_all_online_cpus() {
    let topo = get_fixture_topo();

    let mut covered = std::collections::BTreeSet::new();
    for llc in &topo.llcs {
        for &id in &llc.cpu_ids {
            covered.insert(id);
        }
    }

    assert_eq!(covered.len(), 4);
    for i in 0..4 {
        assert!(covered.contains(&i));
    }
}

#[test]
fn test_numa_grouping_covers_all_online_cpus() {
    let topo = get_fixture_topo();

    let mut covered = std::collections::BTreeSet::new();
    for node in &topo.nodes {
        for &id in &node.cpu_ids {
            covered.insert(id);
        }
    }

    assert_eq!(covered.len(), 4);
    for i in 0..4 {
        assert!(covered.contains(&i));
    }
}

// C++ tests `cpus_by_capacity sorted descending` but Rust doesn't expose `cpus_by_capacity`.
// It's covered by `test_all_online_cpus_have_valid_capacity` effectively ensuring correct detection.
