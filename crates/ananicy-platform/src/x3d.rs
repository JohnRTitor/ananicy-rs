use {
    std::{
        collections::{BTreeMap, BTreeSet},
        fs,
        path::{Path, PathBuf},
    },
    tracing::{debug, error, warn},
};

pub enum X3DMode {
    Cache,
    Frequency,
}

impl X3DMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            X3DMode::Cache => "cache",
            X3DMode::Frequency => "frequency",
        }
    }
}

fn get_x3d_mode_path_in_sysfs(sysfs_root: &Path) -> Option<PathBuf> {
    let driver_dir = sysfs_root.join("bus/platform/drivers/amd_x3d_vcache");
    let entries = fs::read_dir(driver_dir).ok()?;
    entries
        .flatten()
        .map(|entry| entry.path().join("amd_x3d_mode"))
        .find(|path| path.exists())
}

pub fn get_driver_mode() -> Option<X3DMode> {
    get_driver_mode_in(Path::new("/sys"))
}

pub fn get_driver_mode_in(sysfs_root: &Path) -> Option<X3DMode> {
    let mode_path = get_x3d_mode_path_in_sysfs(sysfs_root)?;
    let content = fs::read_to_string(&mode_path).ok()?;
    match content.trim() {
        "cache" => Some(X3DMode::Cache),
        "frequency" => Some(X3DMode::Frequency),
        _ => None,
    }
}

pub fn set_driver_mode(mode: X3DMode) -> bool {
    set_driver_mode_in(Path::new("/sys"), mode)
}

pub fn set_driver_mode_in(sysfs_root: &Path, mode: X3DMode) -> bool {
    let Some(mode_path) = get_x3d_mode_path_in_sysfs(sysfs_root) else {
        warn!("x3d: driver sysfs path not found, cannot set mode");
        return false;
    };

    match fs::write(&mode_path, mode.as_str()) {
        Ok(_) => {
            debug!("x3d: set driver mode to {}", mode.as_str());
            true
        }
        Err(e) => {
            error!(
                "x3d: failed to write mode to {}: {}",
                mode_path.display(),
                e
            );
            false
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct X3DTopology {
    pub cache_cores_str: String,
    pub frequency_cores_str: String,
}

fn is_x3d_from_cpuinfo(proc_root: &Path) -> bool {
    let cpuinfo_path = proc_root.join("cpuinfo");
    fs::read_to_string(&cpuinfo_path).is_ok_and(|content| {
        content.contains("X3D") || content.contains("x3d") || content.contains("3D V-Cache")
    })
}

pub fn detect_x3d_topology() -> Option<X3DTopology> {
    detect_x3d_topology_impl(Path::new("/sys"), Path::new("/proc"))
}

fn detect_x3d_topology_impl(sys_root: &Path, proc_root: &Path) -> Option<X3DTopology> {
    // Check if X3D driver is actually bound to a device (not just loaded), or fallback to cpuinfo
    let driver_present = get_x3d_mode_path_in_sysfs(sys_root).is_some();
    if !driver_present && !is_x3d_from_cpuinfo(proc_root) {
        debug!("detect_x3d_topology: amd_x3d_vcache bound device not found and cpuinfo lacks X3D");
        return None;
    }

    let mut die_to_cores: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    let mut die_to_cache: BTreeMap<u32, u64> = BTreeMap::new();

    let sys_cpu_dir = sys_root.join("devices/system/cpu");
    if let Ok(entries) = fs::read_dir(&sys_cpu_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("cpu")
                && name.len() > 3
                && let Ok(cpu_id) = name[3..].parse::<u32>()
            {
                let mut die_id = None;

                let die_path = entry.path().join("topology/die_id");
                if let Ok(die_str) = fs::read_to_string(&die_path) {
                    die_id = die_str.trim().parse::<i32>().ok();
                }

                if die_id.is_none_or(|id| id == -1) {
                    let cluster_path = entry.path().join("topology/cluster_id");
                    if let Ok(cluster_str) = fs::read_to_string(&cluster_path) {
                        die_id = cluster_str.trim().parse::<i32>().ok();
                    }
                }

                let die_id = die_id.unwrap_or(0) as u32;

                die_to_cores.entry(die_id).or_default().insert(cpu_id);

                // Read the L3 cache size (index3 is usually L3), in the same
                // units the topology detector uses so the two agree on which
                // CCD holds the V-Cache.
                let cache_path = entry.path().join("cache/index3/size");
                if let Ok(cache_str) = fs::read_to_string(&cache_path) {
                    let cache_size = crate::topology::parse_size_string(&cache_str);
                    if cache_size > 0 {
                        die_to_cache.insert(die_id, cache_size);
                    }
                }
            }
        }
    }

    // Single-CCD part, like a 7800X3D or 9800X3D: both aliases are every core.
    //
    // The reference decides this on the number of *dies*, not on how many of them
    // reported a cache size, and it does not require a readable L3 to call a part
    // single-CCD — it just maps both aliases to `0-(N-1)`
    // (`x3d.cpp:164-170`, `die_map.size() < 2`). Bailing out when no cache size
    // could be read therefore left a single-CCD machine with no `x3d-*` alias at
    // all, where the reference still had both, and a two-die machine with only
    // one readable L3 was treated as single-CCD here and as multi-CCD there.
    if die_to_cores.len() < 2 {
        let all_cores: BTreeSet<u32> = die_to_cores.values().flatten().copied().collect();
        if all_cores.is_empty() {
            warn!("detect_x3d_topology: no CPU topology to build an alias from");
            return None;
        }
        if die_to_cache.is_empty() {
            debug!(
                "detect_x3d_topology: single-CCD part with no readable L3, \
                 mapping both aliases to all {} cores",
                all_cores.len()
            );
        }
        let all_cores_str = format_cpuset(&all_cores);
        return Some(X3DTopology {
            cache_cores_str: all_cores_str.clone(),
            frequency_cores_str: all_cores_str,
        });
    }

    // More than one die, so the V-Cache CCD has to be identified by its L3, and
    // without one there is nothing to go on. The reference reaches the same
    // conclusion by a different route: it picks the die with the largest
    // readable L3 and returns nothing if there is none (`x3d.cpp:188-192`).
    if die_to_cache.is_empty() {
        warn!("detect_x3d_topology: Could not find any cache sizes");
        return None;
    }

    // Find the die with the largest L3 cache
    let mut max_cache = 0;
    let mut cache_die = 0;

    for (&die_id, &size) in &die_to_cache {
        if size > max_cache {
            max_cache = size;
            cache_die = die_id;
        }
    }

    let mut cache_cores = BTreeSet::new();
    let mut freq_cores = BTreeSet::new();

    for (&die, cores) in &die_to_cores {
        if die == cache_die {
            cache_cores.extend(cores.iter().copied());
        } else {
            freq_cores.extend(cores.iter().copied());
        }
    }

    let top = X3DTopology {
        cache_cores_str: format_cpuset(&cache_cores),
        frequency_cores_str: format_cpuset(&freq_cores),
    };

    debug!(
        "X3D Topology detected: cache_cores={}, frequency_cores={}",
        top.cache_cores_str, top.frequency_cores_str
    );
    Some(top)
}

fn format_cpuset(cores: &BTreeSet<u32>) -> String {
    if cores.is_empty() {
        return String::new();
    }

    let mut sorted: Vec<u32> = cores.iter().copied().collect();
    sorted.sort_unstable();

    let mut result = String::new();
    let mut start = sorted[0];
    let mut prev = sorted[0];

    for &cpu in &sorted[1..] {
        if cpu == prev + 1 {
            prev = cpu;
        } else {
            if start == prev {
                result.push_str(&format!("{},", start));
            } else {
                result.push_str(&format!("{}-{},", start, prev));
            }
            start = cpu;
            prev = cpu;
        }
    }

    if start == prev {
        result.push_str(&format!("{}", start));
    } else {
        result.push_str(&format!("{}-{}", start, prev));
    }

    result
}

#[cfg(test)]
mod tests {
    use {super::*, std::path::Path};

    #[test]
    fn test_nonx3d_single_ccd() {
        let root = Path::new("tests/fixtures/x3d/amd-nonx3d-single-ccd");
        let sys_root = root.join("sys");
        let proc_root = root.join("proc");

        let result = detect_x3d_topology_impl(&sys_root, &proc_root);
        assert!(result.is_none());
    }

    #[test]
    fn test_x3d_single_ccd() {
        let root = Path::new("tests/fixtures/x3d/amd-x3d-single-ccd");
        let sys_root = root.join("sys");
        let proc_root = root.join("proc");

        let result = detect_x3d_topology_impl(&sys_root, &proc_root);
        assert!(result.is_some());
        let top = result.unwrap();
        assert_eq!(top.cache_cores_str, "0");
        assert_eq!(top.frequency_cores_str, "0");
    }

    #[test]
    fn test_x3d_multi_ccd() {
        let root = Path::new("tests/fixtures/x3d/amd-x3d-multi-ccd");
        let sys_root = root.join("sys");
        let proc_root = root.join("proc");

        let result = detect_x3d_topology_impl(&sys_root, &proc_root);
        assert!(result.is_some());
        let top = result.unwrap();
        assert_eq!(top.cache_cores_str, "0");
        assert_eq!(top.frequency_cores_str, "1");
    }

    /// A single die with no readable L3 still gets both aliases, covering every
    /// core.
    ///
    /// The reference does not need a cache size to recognise a single-CCD part:
    /// it counts dies and maps both aliases to `0-(N-1)` regardless
    /// (`x3d.cpp:164-170`). Requiring a readable L3 meant a part whose
    /// `cache/index3/size` was not exposed had no `x3d-cache` or `x3d-frequency`
    /// alias at all, so a rule naming one silently matched nothing.
    #[test]
    fn a_single_die_with_no_readable_l3_still_gets_both_aliases() {
        let root = Path::new("tests/fixtures/x3d/amd-x3d-single-ccd-no-l3");
        let result = detect_x3d_topology_impl(&root.join("sys"), &root.join("proc"));

        let top = result.expect("a single die needs no L3 to be recognised");
        assert_eq!(top.cache_cores_str, "0-3");
        assert_eq!(top.frequency_cores_str, "0-3");
    }

    /// Two dies with no readable L3 between them cannot be split, because there
    /// is nothing to identify the V-Cache CCD by.
    ///
    /// This is the one multi-CCD case where the answer is "no", and the
    /// reference reaches it the same way: it picks the die with the largest
    /// readable L3 and returns nothing if there is none (`x3d.cpp:188-192`). What
    /// it must not do is quietly treat two dies as one and hand every core to
    /// both aliases, which is what counting only the dies that reported a
    /// readable cache size would have done.
    #[test]
    fn two_dies_with_no_readable_l3_cannot_be_split() {
        use std::fs;

        let root = Path::new("tests/fixtures/x3d/amd-x3d-multi-ccd");
        let tmp = tempfile::tempdir().expect("a temporary fixture");
        copy_dir(&root.join("sys"), &tmp.path().join("sys"));

        for cpu in ["cpu0", "cpu1"] {
            fs::remove_file(
                tmp.path()
                    .join(format!("sys/devices/system/cpu/{cpu}/cache/index3/size")),
            )
            .expect("the fixture has an L3 to remove");
        }

        let result = detect_x3d_topology_impl(&tmp.path().join("sys"), &root.join("proc"));

        assert!(
            result.is_none(),
            "two dies and no L3 to compare identifies no V-Cache CCD: {:?}",
            result
        );
    }

    /// With one die's L3 missing, the readable one is taken as the V-Cache CCD,
    /// which is what the reference does — it only gives up when *no* die reports
    /// a size.
    #[test]
    fn two_dies_with_one_readable_l3_split_on_the_readable_one() {
        use std::fs;

        let root = Path::new("tests/fixtures/x3d/amd-x3d-multi-ccd");
        let tmp = tempfile::tempdir().expect("a temporary fixture");
        copy_dir(&root.join("sys"), &tmp.path().join("sys"));
        fs::remove_file(
            tmp.path()
                .join("sys/devices/system/cpu/cpu1/cache/index3/size"),
        )
        .expect("the fixture has an L3 to remove");

        let result = detect_x3d_topology_impl(&tmp.path().join("sys"), &root.join("proc"));

        let top = result.expect("one readable L3 is enough to name a V-Cache CCD");
        assert_eq!(
            top.cache_cores_str, "0",
            "die 0 is the only one whose L3 could be read"
        );
        assert_eq!(top.frequency_cores_str, "1");
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).expect("a directory");
        for entry in std::fs::read_dir(from)
            .expect("a readable directory")
            .flatten()
        {
            let target = to.join(entry.file_name());
            if entry.file_type().expect("a file type").is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).expect("a copied file");
            }
        }
    }
}
