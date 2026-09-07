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
    if let Ok(entries) = fs::read_dir(driver_dir) {
        for entry in entries.flatten() {
            let path = entry.path().join("amd_x3d_mode");
            if path.exists() {
                return Some(path);
            }
        }
    }
    None
}

fn get_x3d_mode_path() -> Option<PathBuf> {
    get_x3d_mode_path_in_sysfs(Path::new("/sys"))
}

pub fn get_driver_mode() -> Option<X3DMode> {
    if let Some(mode_path) = get_x3d_mode_path()
        && let Ok(content) = fs::read_to_string(&mode_path)
    {
        let trimmed = content.trim();
        if trimmed == "cache" {
            return Some(X3DMode::Cache);
        } else if trimmed == "frequency" {
            return Some(X3DMode::Frequency);
        }
    }
    None
}

pub fn set_driver_mode(mode: X3DMode) -> bool {
    if let Some(mode_path) = get_x3d_mode_path() {
        match fs::write(&mode_path, mode.as_str()) {
            Ok(_) => {
                debug!("x3d: set driver mode to {}", mode.as_str());
                return true;
            }
            Err(e) => {
                error!(
                    "x3d: failed to write mode to {}: {}",
                    mode_path.display(),
                    e
                );
                return false;
            }
        }
    }
    warn!("x3d: driver sysfs path not found, cannot set mode");
    false
}

#[derive(Debug, PartialEq, Eq)]
pub struct X3DTopology {
    pub cache_cores_str: String,
    pub frequency_cores_str: String,
}

fn is_x3d_from_cpuinfo(proc_root: &Path) -> bool {
    let cpuinfo_path = proc_root.join("cpuinfo");
    if let Ok(content) = fs::read_to_string(&cpuinfo_path) {
        return content.contains("X3D")
            || content.contains("x3d")
            || content.contains("3D V-Cache");
    }
    false
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

                if die_id.is_none() || die_id == Some(-1) {
                    let cluster_path = entry.path().join("topology/cluster_id");
                    if let Ok(cluster_str) = fs::read_to_string(&cluster_path) {
                        die_id = cluster_str.trim().parse::<i32>().ok();
                    }
                }

                let die_id = die_id.unwrap_or(0) as u32;

                die_to_cores.entry(die_id).or_default().insert(cpu_id);

                // Read L3 cache size (index3 is usually L3)
                let cache_path = entry.path().join("cache/index3/size");
                if let Ok(cache_str) = fs::read_to_string(&cache_path) {
                    // "98304K" -> parse out K
                    let val_str = cache_str.trim().trim_end_matches('K');
                    if let Ok(cache_size) = val_str.parse::<u64>() {
                        die_to_cache.insert(die_id, cache_size);
                    }
                }
            }
        }
    }

    if die_to_cache.is_empty() {
        warn!("detect_x3d_topology: Could not find any cache sizes");
        return None;
    }

    if die_to_cache.len() == 1 {
        // Single-CCD X3D part (like 7800X3D)
        let Some(only_die) = die_to_cores.values().next() else {
            warn!("detect_x3d_topology: cache information found without any CPU topology");
            return None;
        };
        let all_cores_str = format_cpuset(only_die);
        return Some(X3DTopology {
            cache_cores_str: all_cores_str.clone(),
            frequency_cores_str: all_cores_str,
        });
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
}
