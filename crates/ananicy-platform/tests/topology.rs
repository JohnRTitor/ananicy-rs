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

/// The same machine, with a `acpi_cppc/highest_perf` that reports the same
/// number for every CPU. It sits above `cpu_capacity` in the probing order, so a
/// per-CPU "first readable source wins" would measure the whole machine with it
/// and call the machine homogeneous.
fn uniform_higher_source() -> CpuTopology {
    fixture("topology/uniform-higher-source")
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
fn a_capacity_source_that_tells_no_cpus_apart_is_skipped() {
    // `acpi_cppc/highest_perf` reads 2400 on every CPU here, so it cannot
    // classify anything. The next source that does differentiate — `cpu_capacity`
    // — is the one the machine is measured with, so the big.LITTLE split is the
    // same as on the fixture without the uniform file.
    let topo = uniform_higher_source();

    assert!(topo.has_big_little);
    assert_eq!(topo.little_cores_str, "0-1");
    assert_eq!(topo.big_cores_str, "2-3");
    assert_eq!(topo.turbo_cores_str, "2-3");
    assert_eq!(
        topo.big_cores_str,
        big_little().big_cores_str,
        "a uniform higher-priority file must not change the classification"
    );
}

#[test]
fn a_machine_with_no_differentiating_capacity_is_homogeneous() {
    // Every CPU reports the same capacity through every source: there is
    // nothing to classify, so all of them are big cores and there is neither a
    // little nor a turbo subset.
    let root = tempfile::tempdir().unwrap();
    let cpu_dir = root.path().join("sys/devices/system/cpu");
    for cpu in 0..4 {
        let base = cpu_dir.join(format!("cpu{cpu}"));
        std::fs::create_dir_all(base.join("acpi_cppc")).unwrap();
        std::fs::write(base.join("cpu_capacity"), "1024\n").unwrap();
        std::fs::write(base.join("acpi_cppc").join("highest_perf"), "2400\n").unwrap();
    }
    std::fs::create_dir_all(cpu_dir.join("smt")).unwrap();
    std::fs::write(cpu_dir.join("smt").join("active"), "0\n").unwrap();

    let topo = detect_topology_impl(&root.path().join("sys"));
    assert_eq!(topo.all_cores_str, "0-3", "the fixture is read");
    assert!(!topo.has_big_little);
    assert_eq!(topo.big_cores_str, topo.all_cores_str);
    assert_eq!(topo.little_cores_str, "");
    assert_eq!(topo.turbo_cores_str, "");
}

/// A machine whose CPUs report distinct capacities, but not distinct enough:
/// 166 against 186 is a 1.12x spread, below the 1.3x the classifier requires.
fn near_uniform_capacities() -> CpuTopology {
    let root = tempfile::tempdir().unwrap();
    let cpu_dir = root.path().join("sys/devices/system/cpu");
    for (cpu, capacity) in [(0, 166), (1, 181), (2, 166), (3, 186)] {
        let base = cpu_dir.join(format!("cpu{cpu}"));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("cpu_capacity"), format!("{capacity}\n")).unwrap();
    }
    std::fs::create_dir_all(cpu_dir.join("smt")).unwrap();
    std::fs::write(cpu_dir.join("smt").join("active"), "0\n").unwrap();

    detect_topology_impl(&root.path().join("sys"))
}

#[test]
fn a_capacity_spread_below_the_threshold_is_homogeneous() {
    // Capacities are read and they do differ, but not by the 1.3x the
    // classifier needs. That still counts as homogeneous, and the two
    // homogeneous paths have to agree: no little cores, no turbo subset.
    let topo = near_uniform_capacities();

    assert_eq!(topo.all_cores_str, "0-3", "the fixture is read");
    assert!(!topo.has_big_little);
    assert_eq!(topo.big_cores_str, topo.all_cores_str);
    assert_eq!(
        topo.little_cores_str, "",
        "a machine without little cores must not answer for little-cores"
    );
    assert_eq!(topo.turbo_cores_str, "");

    let aliases = topo.generate_cpuset_aliases();
    assert_eq!(aliases["efficiency-cores"], "");
    assert_eq!(aliases["turbo-cores"], "");
    assert_eq!(
        aliases["performance-cores"], topo.all_cores_str,
        "with no turbo subset, performance cores are all the big cores"
    );
    assert_eq!(aliases["big-cores"], topo.all_cores_str);
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

/// Cache sizes are compared to pick the largest last-level cache and the
/// V-Cache CCD, so the units only have to be consistent — but they do have to
/// be, and the two readers used to disagree about them.
#[test]
fn cache_sizes_are_read_in_bytes() {
    use ananicy_platform::topology::parse_size_string;

    assert_eq!(parse_size_string("32K"), 32 * 1024);
    assert_eq!(parse_size_string("32k"), 32 * 1024);
    assert_eq!(parse_size_string("1536K\n"), 1536 * 1024);
    assert_eq!(parse_size_string("16M"), 16 * 1024 * 1024);
    assert_eq!(parse_size_string("2G"), 2 * 1024 * 1024 * 1024);
    assert_eq!(parse_size_string("  16M  "), 16 * 1024 * 1024);
    assert_eq!(parse_size_string("4096"), 4096, "no suffix means bytes");

    // Anything unreadable is 0, which every caller reads as "not reported".
    assert_eq!(parse_size_string(""), 0);
    assert_eq!(parse_size_string("unknown"), 0);
    assert_eq!(parse_size_string("16X"), 0);
}

/// `llc-N` is numbered by the order the LLCs are first met, so the order the
/// `cpu*` directories are walked in decides what the alias names.
///
/// `read_dir` returns whatever order the filesystem hands back, which is
/// ascending on a plain sysfs and is not promised anywhere. The reference walks
/// CPU ids ascending, so on a machine where the two disagreed a rule naming
/// `llc-1` would pin to a different set of CPUs under each daemon. Ascending CPU
/// order is the order that makes the two agree, and it is also the only one that
/// is reproducible.
#[test]
fn llc_aliases_are_numbered_in_ascending_cpu_order() {
    let topo = big_little();

    // The fixture's two LLCs are {0,1} and {2,3}, so the first one met must be
    // the one holding CPU 0.
    let llc0 = topo
        .llcs
        .iter()
        .find(|llc| llc.id == 0)
        .expect("an llc-0 alias");
    assert_eq!(
        llc0.cpu_ids.iter().copied().collect::<BTreeSet<u32>>(),
        BTreeSet::from([0, 1]),
        "llc-0 must be the LLC containing CPU 0"
    );
    assert_eq!(llc0.cpuset_str, "0-1");

    let llc1 = topo
        .llcs
        .iter()
        .find(|llc| llc.id == 1)
        .expect("an llc-1 alias");
    assert_eq!(
        llc1.cpu_ids.iter().copied().collect::<BTreeSet<u32>>(),
        BTreeSet::from([2, 3]),
        "llc-1 must be the LLC containing CPU 2"
    );
    assert_eq!(llc1.cpuset_str, "2-3");
}
/// The numbering must come from the CPU ids, not from the order the filesystem
/// happened to list the `cpu*` directories in.
///
/// `read_dir` order is not ascending in general — it is whatever the filesystem
/// returns, which is hash order on tmpfs and on several of the overlay and bind
/// mount arrangements a container sees. The directory is built with enough CPUs
/// that the raw order is very unlikely to be ascending, and the test says so
/// when it is, rather than passing quietly on a filesystem that happens to
/// agree.
#[test]
fn llc_numbering_comes_from_cpu_ids_not_directory_order() {
    use std::fs;

    // Eight CPUs in two LLCs, {0-3} and {4-7}. Whichever LLC is met first must
    // be the one holding CPU 0.
    const CPUS: u32 = 8;
    let tmp = tempfile::tempdir().expect("a temporary sysfs");
    let cpu_root = tmp.path().join("devices/system/cpu");
    fs::create_dir_all(&cpu_root).expect("a cpu directory");
    for id in 0..CPUS {
        let base = cpu_root.join(format!("cpu{id}"));
        fs::create_dir_all(base.join("cache/index3")).expect("a cache directory");
        fs::write(
            base.join("cache/index3/shared_cpu_list"),
            format!("{}-{}", id / 4 * 4, id / 4 * 4 + 3),
        )
        .expect("a shared_cpu_list");
        fs::write(base.join("cache/index3/size"), "8192K").expect("a cache size");
    }

    let raw: Vec<String> = fs::read_dir(&cpu_root)
        .expect("a readable directory")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let ascending = {
        let mut sorted = raw.clone();
        sorted.sort();
        sorted == raw
    };
    if ascending {
        eprintln!(
            "note: this filesystem listed the cpu directories in ascending order, \
             so the sort is not exercised by this run ({raw:?})"
        );
    }

    let topo = detect_topology_impl(tmp.path());
    let by_id: std::collections::HashMap<i32, String> = topo
        .llcs
        .iter()
        .map(|llc| (llc.id, llc.cpuset_str.clone()))
        .collect();

    assert_eq!(
        by_id.get(&0).map(String::as_str),
        Some("0-3"),
        "llc-0 must be the LLC containing CPU 0, whatever order the directory \
         was read in; got {by_id:?}"
    );
    assert_eq!(
        by_id.get(&1).map(String::as_str),
        Some("4-7"),
        "llc-1 must be the LLC containing CPU 4; got {by_id:?}"
    );
}
