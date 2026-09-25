//! CPU topology detection from a synthetic `sysfs` tree.
//!
//! `detect_topology_impl` takes the sysfs root as an argument, so these tests
//! never look at the host's real hardware: they are hermetic and therefore
//! behave identically on a phone, in a container and on a workstation.
//!
//! The `homogeneous` case is not a compatibility requirement but a property of
//! the algorithm: a machine whose CPUs all report the same capacity has no
//! little cores and no turbo subset, so every CPU counts as a big core.

use {
    ananicy_core::cpuset::CpuSet,
    ananicy_platform::topology::{CpuTopology, detect_topology_impl},
    std::{collections::BTreeSet, path::Path},
};

fn fixture(name: &str) -> CpuTopology {
    detect_topology_impl(&Path::new("tests/fixtures").join(name).join("sys"))
}

fn big_little() -> CpuTopology {
    fixture("topology/big-little")
}

#[test]
fn test_detect_produces_valid_topology() {
    let topo = big_little();

    // 4 CPUs in our fixture
    assert_eq!(topo.all_cores_str, "0-3");

    // We mocked a big.LITTLE system where CPUs 0,1 are 500 capacity, and CPUs 2,3 are 1024.
    assert!(topo.has_big_little);
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
    // Capacity is what separates big from little, so a fixture without
    // capacity differences must not be reported as a heterogeneous machine.
    let topo = big_little();
    assert_ne!(
        topo.big_cores_str, topo.little_cores_str,
        "Should detect capacity differences"
    );
}

#[test]
fn test_cpuset_strings_are_parseable() {
    let topo = big_little();

    assert!(CpuSet::parse(&topo.big_cores_str, 32).is_some());
    assert!(CpuSet::parse(&topo.little_cores_str, 32).is_some());
    assert!(CpuSet::parse(&topo.all_cores_str, 32).is_some());
    assert!(CpuSet::parse(&topo.turbo_cores_str, 32).is_some());
}

#[test]
fn test_all_cores_covers_every_online_cpu() {
    let topo = big_little();
    let parsed = CpuSet::parse(&topo.all_cores_str, 32).unwrap();

    assert!(parsed.has_cpu(0));
    assert!(parsed.has_cpu(1));
    assert!(parsed.has_cpu(2));
    assert!(parsed.has_cpu(3));
    assert!(!parsed.has_cpu(4));
}

#[test]
fn test_on_homogeneous_system_all_cores_are_big() {
    // The x3d-single-ccd fixture has no capacity data, so the machine is
    // homogeneous: every CPU is a big core and there is neither a little nor a
    // turbo subset.
    let topo = fixture("x3d/amd-x3d-single-ccd");

    assert!(!topo.has_big_little);
    assert_eq!(topo.big_cores_str, topo.all_cores_str);
    assert_eq!(topo.little_cores_str, "");
    assert_eq!(topo.turbo_cores_str, "");
}

#[test]
fn test_llc_grouping_covers_all_online_cpus() {
    let topo = big_little();

    let mut covered = BTreeSet::new();
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
    let topo = big_little();

    let mut covered = BTreeSet::new();
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

#[test]
fn every_llc_cpuset_string_describes_its_own_cpus() {
    let topo = big_little();

    for llc in &topo.llcs {
        let parsed = CpuSet::parse(&llc.cpuset_str, 32)
            .unwrap_or_else(|| panic!("unparseable llc cpuset {:?}", llc.cpuset_str));
        let expected: Vec<u32> = llc.cpu_ids.to_vec();
        assert_eq!(parsed.get_cores(), expected, "llc {} disagrees", llc.id);
    }
}

#[test]
fn every_node_cpuset_string_describes_its_own_cpus() {
    let topo = big_little();

    for node in &topo.nodes {
        let parsed = CpuSet::parse(&node.cpuset_str, 32)
            .unwrap_or_else(|| panic!("unparseable node cpuset {:?}", node.cpuset_str));
        let expected: Vec<u32> = node.cpu_ids.to_vec();
        assert_eq!(parsed.get_cores(), expected, "node {} disagrees", node.id);
    }
}

#[test]
fn cpuset_aliases_expose_the_topology_under_its_documented_names() {
    let topo = big_little();
    let aliases = topo.generate_cpuset_aliases();

    assert_eq!(aliases["all"], topo.all_cores_str);
    assert_eq!(aliases["all-cores"], topo.all_cores_str);
    assert_eq!(aliases["big-cores"], topo.big_cores_str);
    assert_eq!(aliases["little-cores"], topo.little_cores_str);
    assert_eq!(aliases["turbo-cores"], topo.turbo_cores_str);

    // `performance-cores` prefers the turbo subset while it is a strict
    // subset, and falls back to the big cores once it covers everything.
    assert_eq!(aliases["performance-cores"], topo.turbo_cores_str);
    let homogeneous = fixture("x3d/amd-x3d-single-ccd");
    assert_eq!(
        homogeneous.generate_cpuset_aliases()["performance-cores"],
        homogeneous.big_cores_str
    );
}

#[test]
fn cpuset_aliases_cover_every_llc_and_node() {
    let topo = big_little();
    let aliases = topo.generate_cpuset_aliases();

    for llc in &topo.llcs {
        assert_eq!(aliases[&format!("llc-{}", llc.id)], llc.cpuset_str);
    }
    for node in &topo.nodes {
        assert_eq!(aliases[&format!("node-{}", node.id)], node.cpuset_str);
    }
    // The x3d alias is only published when a cache group was detected, and then
    // it must be exactly the group the topology picked.
    if topo.biggest_llc_cores_str.is_empty() {
        assert!(!aliases.contains_key("x3d-cache"));
    } else {
        assert_eq!(aliases["x3d-cache"], topo.biggest_llc_cores_str);
    }
}

#[test]
fn every_alias_resolves_to_a_usable_cpuset() {
    let topo = big_little();

    for (name, value) in topo.generate_cpuset_aliases() {
        if value.is_empty() {
            continue; // an alias may legitimately map to "no CPUs"
        }
        assert!(
            CpuSet::parse(&value, 32).is_some(),
            "alias {name} is not a valid cpuset: {value:?}"
        );
    }
}

#[test]
fn summary_reports_cpu_llc_node_and_feature_counts() {
    let topo = big_little();

    assert_eq!(
        topo.summary(),
        format!(
            "{} CPUs, {} LLCs, {} NUMA nodes, SMT=off, big.LITTLE=yes",
            topo.cpu_count,
            topo.llcs.len(),
            topo.nodes.len()
        )
    );
    assert_eq!(topo.cpu_count, 4);
}

#[test]
fn summary_reports_a_homogeneous_machine_without_smt_or_big_little() {
    let topo = fixture("x3d/amd-x3d-single-ccd");

    assert!(
        topo.summary().ends_with("SMT=off, big.LITTLE=no"),
        "unexpected summary: {}",
        topo.summary()
    );
}
