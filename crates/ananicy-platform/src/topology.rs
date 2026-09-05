#![allow(clippy::collapsible_if)]
use {
    std::{
        collections::{BTreeMap, BTreeSet, HashMap},
        fs,
    },
    tracing::{debug, warn},
};

pub struct NodeInfo {
    pub id: i32,
    pub cpu_ids: Vec<u32>,
    pub cpuset_str: String,
}

pub struct LlcInfo {
    pub id: i32,
    pub cpu_ids: Vec<u32>,
    pub cpuset_str: String,
    pub l3_size: u64,
}

#[derive(Default)]
pub struct CpuTopology {
    pub big_cores_str: String,
    pub little_cores_str: String,
    pub turbo_cores_str: String,
    pub all_cores_str: String,
    pub smt_enabled: bool,
    pub nodes: Vec<NodeInfo>,
    pub llcs: Vec<LlcInfo>,
    pub biggest_llc_cores_str: String,
}

impl CpuTopology {
    pub fn generate_cpuset_aliases(&self) -> HashMap<String, String> {
        let mut aliases = HashMap::new();
        aliases.insert("all".to_string(), self.all_cores_str.clone());
        aliases.insert("all-cores".to_string(), self.all_cores_str.clone());
        aliases.insert("big-cores".to_string(), self.big_cores_str.clone());
        aliases.insert("little-cores".to_string(), self.little_cores_str.clone());

        let perf_cores =
            if !self.turbo_cores_str.is_empty() && self.turbo_cores_str != self.all_cores_str {
                self.turbo_cores_str.clone()
            } else {
                self.big_cores_str.clone()
            };

        aliases.insert("performance-cores".to_string(), perf_cores);
        aliases.insert(
            "efficiency-cores".to_string(),
            self.little_cores_str.clone(),
        );
        aliases.insert("turbo-cores".to_string(), self.turbo_cores_str.clone());

        if !self.biggest_llc_cores_str.is_empty() {
            aliases.insert("x3d-cache".to_string(), self.biggest_llc_cores_str.clone());
        }

        for llc in &self.llcs {
            aliases.insert(format!("llc-{}", llc.id), llc.cpuset_str.clone());
        }
        for node in &self.nodes {
            aliases.insert(format!("node-{}", node.id), node.cpuset_str.clone());
        }

        aliases
    }
}

fn detect_smt(sys_root: &std::path::Path) -> bool {
    let path = sys_root.join("devices/system/cpu/smt/active");
    if let Ok(content) = fs::read_to_string(&path) {
        return content.trim() == "1";
    }
    false
}

fn get_node_id(base: &std::path::Path) -> i32 {
    if let Ok(entries) = fs::read_dir(base) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("node")
                && name.len() > 4
                && let Ok(id) = name[4..].parse::<i32>()
            {
                return id;
            }
        }
    }
    0
}

fn get_llc_id(base: &std::path::Path, llc_map: &mut HashMap<String, i32>) -> i32 {
    for level in (2..=3).rev() {
        let path = base.join(format!("cache/index{}/shared_cpu_list", level));
        if let Ok(key) = fs::read_to_string(&path) {
            let key = key.trim().to_string();
            if key.is_empty() {
                continue;
            }
            if let Some(&id) = llc_map.get(&key) {
                return id;
            }
            let new_id = llc_map.len() as i32;
            llc_map.insert(key, new_id);
            return new_id;
        }
    }
    0
}

fn parse_size_string(s: &str) -> u64 {
    let s = s.trim();
    if s.is_empty() {
        return 0;
    }
    let mut num_str = s.to_string();
    let mut mult = 1;
    if s.ends_with('K') || s.ends_with('k') {
        num_str.pop();
        mult = 1024;
    } else if s.ends_with('M') || s.ends_with('m') {
        num_str.pop();
        mult = 1024 * 1024;
    }
    num_str.parse::<u64>().unwrap_or(0) * mult
}

#[allow(dead_code)]
fn get_cache_size(base: &std::path::Path) -> u64 {
    let mut total = 0;
    for idx in 0..8 {
        let index_base = base.join(format!("cache/index{}", idx));
        if let Ok(size_str) = fs::read_to_string(index_base.join("size")) {
            if size_str.trim().is_empty() {
                break;
            }
            let mut num_sharing = 1;
            if let Ok(cpulist_str) = fs::read_to_string(index_base.join("shared_cpu_list")) {
                let cpulist_str = cpulist_str.trim();
                if !cpulist_str.is_empty() {
                    let mut count = 0;
                    for part in cpulist_str.split(',') {
                        if let Some((start, end)) = part.split_once('-') {
                            if let (Ok(s), Ok(e)) = (start.parse::<u32>(), end.parse::<u32>()) {
                                count += e.saturating_sub(s) + 1;
                            }
                        } else {
                            count += 1;
                        }
                    }
                    if count > 0 {
                        num_sharing = count;
                    }
                }
            }
            total += parse_size_string(&size_str) / (num_sharing as u64);
        } else {
            break;
        }
    }
    total
}

pub fn detect_topology() -> CpuTopology {
    detect_topology_impl(std::path::Path::new("/sys"))
}

pub fn detect_topology_impl(sys_root: &std::path::Path) -> CpuTopology {
    let mut top = CpuTopology {
        smt_enabled: detect_smt(sys_root),
        ..Default::default()
    };

    let mut all_cores = BTreeSet::new();
    let mut metric_to_cores: BTreeMap<u64, BTreeSet<u32>> = BTreeMap::new();
    let mut llc_map: HashMap<String, i32> = HashMap::new();

    let mut llc_groups: HashMap<i32, BTreeSet<u32>> = HashMap::new();
    let mut node_groups: HashMap<i32, BTreeSet<u32>> = HashMap::new();
    let mut llc_l3_size: HashMap<i32, u64> = HashMap::new();

    if let Ok(entries) = fs::read_dir(sys_root.join("devices/system/cpu")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("cpu")
                && name.len() > 3
                && let Ok(cpu_id) = name[3..].parse::<u32>()
            {
                let base = entry.path();

                all_cores.insert(cpu_id);

                let node_id = get_node_id(&base);
                let llc_id = get_llc_id(&base, &mut llc_map);

                node_groups.entry(node_id).or_default().insert(cpu_id);
                llc_groups.entry(llc_id).or_default().insert(cpu_id);

                if let Ok(l3_str) = fs::read_to_string(base.join("cache/index3/size")) {
                    llc_l3_size.insert(llc_id, parse_size_string(&l3_str));
                }

                let paths = [
                    "cpufreq/amd_pstate_prefcore_ranking",
                    "cpufreq/amd_pstate_highest_perf",
                    "acpi_cppc/highest_perf",
                    "cpu_capacity",
                    "cpufreq/cpuinfo_max_freq",
                ];

                let mut metric = None;
                for p in paths {
                    let cap_path = base.join(p);
                    if let Ok(cap_str) = fs::read_to_string(&cap_path)
                        && let Ok(cap) = cap_str.trim().parse::<u64>()
                    {
                        metric = Some(cap);
                        break;
                    }
                }

                if let Some(val) = metric {
                    metric_to_cores.entry(val).or_default().insert(cpu_id);
                }
            }
        }
    }

    if all_cores.is_empty() {
        warn!("detect_topology: Could not detect any CPUs in /sys");
        return top;
    }

    top.all_cores_str = format_cpuset(&all_cores);

    for (node_id, cores) in node_groups {
        top.nodes.push(NodeInfo {
            id: node_id,
            cpu_ids: cores.iter().copied().collect(),
            cpuset_str: format_cpuset(&cores),
        });
    }

    let mut max_l3 = 0;
    for (llc_id, cores) in &llc_groups {
        let l3 = llc_l3_size.get(llc_id).copied().unwrap_or(0);
        if l3 > max_l3 {
            max_l3 = l3;
        }
        top.llcs.push(LlcInfo {
            id: *llc_id,
            cpu_ids: cores.iter().copied().collect(),
            cpuset_str: format_cpuset(cores),
            l3_size: l3,
        });
    }

    if max_l3 > 0 {
        let mut biggest_cores = BTreeSet::new();
        for llc in &top.llcs {
            if llc.l3_size == max_l3 {
                biggest_cores.extend(llc.cpu_ids.iter().copied());
            }
        }
        top.biggest_llc_cores_str = format_cpuset(&biggest_cores);
    }

    // If we couldn't read frequencies/capacities, or all CPUs have the same max,
    // we can't differentiate big/little.
    if metric_to_cores.len() <= 1 {
        debug!(
            "detect_topology: All cores have the same metric or no info. Cannot determine big/little."
        );
        top.big_cores_str = top.all_cores_str.clone();
        top.little_cores_str = top.all_cores_str.clone();
        top.turbo_cores_str = top.all_cores_str.clone();
        return top;
    }

    let metrics: Vec<_> = metric_to_cores.keys().copied().collect();
    let (Some(&lowest_metric), Some(&highest_metric)) = (metrics.first(), metrics.last()) else {
        return top;
    };

    // C++ BIG_LITTLE_RATIO requires at least 1.3x difference
    if (highest_metric as f64) < (lowest_metric as f64) * 1.3 {
        debug!(
            "detect_topology: Max capacity ({}) is not >= 1.3x min capacity ({}). Assuming homogeneous.",
            highest_metric, lowest_metric
        );
        top.big_cores_str = top.all_cores_str.clone();
        top.little_cores_str = top.all_cores_str.clone();
        top.turbo_cores_str = top.all_cores_str.clone();
        return top;
    }

    // C++ average-based threshold for middle tiers
    let mut sum = 0.0;
    let mut count = 0;
    for (&metric, cores) in &metric_to_cores {
        sum += metric as f64 * cores.len() as f64;
        count += cores.len();
    }
    let threshold = sum / count as f64;

    let mut little_cores = BTreeSet::new();
    let mut big_cores = BTreeSet::new();

    for (&metric, cores) in &metric_to_cores {
        if (metric as f64) < threshold {
            little_cores.extend(cores.iter().copied());
        } else {
            big_cores.extend(cores.iter().copied());
        }
    }

    top.little_cores_str = format_cpuset(&little_cores);
    top.big_cores_str = format_cpuset(&big_cores);

    let Some(turbo_cores) = metric_to_cores.get(&highest_metric) else {
        warn!("detect_topology: highest metric disappeared before turbo-core selection");
        return top;
    };
    top.turbo_cores_str = format_cpuset(turbo_cores);

    debug!(
        "Topology detected: all={}, big={}, little={}, turbo={}",
        top.all_cores_str, top.big_cores_str, top.little_cores_str, top.turbo_cores_str
    );

    top
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
