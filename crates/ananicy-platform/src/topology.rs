use {
    std::{
        collections::{BTreeMap, BTreeSet, HashMap},
        fs,
        path::{Path, PathBuf},
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
    pub has_big_little: bool,
    pub big_cores_str: String,
    pub little_cores_str: String,
    pub turbo_cores_str: String,
    pub all_cores_str: String,
    pub cpu_count: usize,
    pub smt_enabled: bool,
    pub nodes: Vec<NodeInfo>,
    pub llcs: Vec<LlcInfo>,
    pub biggest_llc_cores_str: String,
}

impl CpuTopology {
    /// One-line description of the detected machine, logged at startup so the
    /// topology a run is using can be read back from the journal.
    ///
    /// The phrasing and the naive pluralisation mirror `ananicy-cpp` so both
    /// daemons can be compared line by line.
    pub fn summary(&self) -> String {
        format!(
            "{} CPUs, {} LLCs, {} NUMA nodes, SMT={}, big.LITTLE={}",
            self.cpu_count,
            self.llcs.len(),
            self.nodes.len(),
            on_off(self.smt_enabled),
            yes_no(self.has_big_little),
        )
    }

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

fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn detect_smt(sys_root: &Path) -> bool {
    let path = sys_root.join("devices/system/cpu/smt/active");
    fs::read_to_string(&path).is_ok_and(|content| content.trim() == "1")
}

fn get_node_id(base: &Path) -> i32 {
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

fn get_llc_id(base: &Path, llc_map: &mut HashMap<String, i32>) -> i32 {
    for level in (2..=3).rev() {
        let path = base.join(format!("cache/index{}/shared_cpu_list", level));
        let Ok(key) = fs::read_to_string(&path) else {
            continue;
        };
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
    0
}

/// Parses a cache size as the kernel writes it — `32K`, `1536K`, `16M` — into
/// bytes. A `G` suffix is understood as well, since the kernel is free to use
/// it for a large last-level cache; an unparsable or zero value reads as 0,
/// which the callers treat as "no size reported".
pub fn parse_size_string(s: &str) -> u64 {
    let s = s.trim();
    let (digits, multiplier) = match s.as_bytes().last() {
        Some(b'K' | b'k') => (&s[..s.len() - 1], 1024u64),
        Some(b'M' | b'm') => (&s[..s.len() - 1], 1024 * 1024),
        Some(b'G' | b'g') => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        _ => (s, 1),
    };

    digits
        .parse::<u64>()
        .unwrap_or(0)
        .saturating_mul(multiplier)
}

#[allow(dead_code)]
fn get_cache_size(base: &Path) -> u64 {
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
    detect_topology_impl(Path::new("/sys"))
}

/// The sysfs files that report a per-CPU capacity or maximum-frequency figure,
/// ordered from the most to the least precise. They are only comparable within
/// one source, so exactly one of them is chosen for the whole machine.
const CAPACITY_SOURCES: [&str; 5] = [
    "cpufreq/amd_pstate_prefcore_ranking",
    "cpufreq/amd_pstate_highest_perf",
    "acpi_cppc/highest_perf",
    "cpu_capacity",
    "cpufreq/cpuinfo_max_freq",
];

/// Reads a capacity figure for one CPU, or `None` when it is not available.
///
/// A reported zero means "offline or not filled in", so it is treated as absent
/// rather than as the smallest capacity there is.
fn read_capacity(bases: &BTreeMap<u32, PathBuf>, cpu_id: u32, source: &str) -> Option<u64> {
    let raw = fs::read_to_string(bases.get(&cpu_id)?.join(source)).ok()?;
    let value = raw.trim().parse::<u64>().ok()?;
    (value > 0).then_some(value)
}

/// Picks the single capacity source the whole machine is measured with.
///
/// Taking the first readable source per CPU is not enough: a kernel that
/// exports a higher-priority source with the same number for every CPU — a
/// uniform `acpi_cppc/highest_perf`, say — would be taken at face value and
/// every heterogeneous machine would look homogeneous. So a candidate is only
/// accepted once it has been seen to tell two CPUs apart, and the least
/// precise source is the fallback when none of them does.
fn pick_capacity_source(bases: &BTreeMap<u32, PathBuf>, cpus: &BTreeSet<u32>) -> &'static str {
    let Some(&first) = cpus.iter().next() else {
        return CAPACITY_SOURCES[CAPACITY_SOURCES.len() - 1];
    };

    let mut chosen = None;
    for source in CAPACITY_SOURCES {
        let Some(reference) = read_capacity(bases, first, source) else {
            continue;
        };
        chosen = Some(source);

        if cpus
            .iter()
            .any(|&cpu| read_capacity(bases, cpu, source).is_some_and(|value| value != reference))
        {
            break;
        }
    }

    // With no source that tells the CPUs apart, the least precise one is the
    // fallback, exactly as for a machine with no CPUs at all.
    chosen.unwrap_or(CAPACITY_SOURCES[CAPACITY_SOURCES.len() - 1])
}

/// Records a machine with no usable core-type split: every online CPU counts as
/// a big core, and there is no little and no turbo subset.
///
/// `little-cores` and `turbo-cores` are left empty on purpose. An empty alias
/// means "no CPUs of that class", and the worker then leaves the affinity of a
/// rule that used it alone. Filling them with every CPU would instead pin
/// whatever asked for efficiency cores to all of them, widening an existing
/// restriction instead of honouring the alias.
fn mark_homogeneous(top: &mut CpuTopology) {
    top.has_big_little = false;
    top.big_cores_str = top.all_cores_str.clone();
    top.little_cores_str = String::new();
    top.turbo_cores_str = String::new();
}

pub fn detect_topology_impl(sys_root: &Path) -> CpuTopology {
    let mut top = CpuTopology {
        smt_enabled: detect_smt(sys_root),
        ..Default::default()
    };

    let mut all_cores = BTreeSet::new();
    let mut bases: BTreeMap<u32, PathBuf> = BTreeMap::new();
    let mut metric_to_cores: BTreeMap<u64, BTreeSet<u32>> = BTreeMap::new();
    let mut llc_map: HashMap<String, i32> = HashMap::new();

    let mut llc_groups: HashMap<i32, BTreeSet<u32>> = HashMap::new();
    let mut node_groups: HashMap<i32, BTreeSet<u32>> = HashMap::new();
    let mut llc_l3_size: HashMap<i32, u64> = HashMap::new();

    if let Ok(entries) = fs::read_dir(sys_root.join("devices/system/cpu")) {
        // The alias ids are handed out in the order the LLCs are first seen, so
        // the order of this walk decides what `llc-0` names. `read_dir` yields
        // whatever order the filesystem feels like — ascending on a plain sysfs,
        // but nothing guarantees it, and an overlay, a bind-mounted subset or a
        // different kernel can all change it. The reference walks CPU ids in
        // ascending order, so sorting here is what makes `llc-N` name the same
        // physical LLC under both daemons.
        let mut cpus: Vec<(u32, PathBuf)> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let id = name.strip_prefix("cpu")?.parse::<u32>().ok()?;
                Some((id, entry.path()))
            })
            .collect();
        cpus.sort_unstable_by_key(|(id, _)| *id);

        for (cpu_id, base) in cpus {
            let online = if cpu_id == 0 {
                true
            } else {
                match fs::read_to_string(base.join("online")) {
                    Ok(s) => s.trim() == "1",
                    Err(_) => true,
                }
            };

            if !online {
                continue;
            }

            all_cores.insert(cpu_id);
            bases.insert(cpu_id, base.clone());

            let node_id = get_node_id(&base);
            let llc_id = get_llc_id(&base, &mut llc_map);

            node_groups.entry(node_id).or_default().insert(cpu_id);
            llc_groups.entry(llc_id).or_default().insert(cpu_id);

            if let Ok(l3_str) = fs::read_to_string(base.join("cache/index3/size")) {
                llc_l3_size.insert(llc_id, parse_size_string(&l3_str));
            }
        }
    }

    if all_cores.is_empty() {
        warn!("detect_topology: Could not detect any CPUs in /sys");
        return top;
    }

    // One source for every CPU, so that all capacities are on one scale and can
    // be compared with each other.
    let source = pick_capacity_source(&bases, &all_cores);
    debug!("detect_topology: measuring capacities with '{source}'");
    for &cpu_id in &all_cores {
        if let Some(value) = read_capacity(&bases, cpu_id, source) {
            metric_to_cores.entry(value).or_default().insert(cpu_id);
        }
    }

    top.all_cores_str = format_cpuset(&all_cores);
    top.cpu_count = all_cores.len();

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
        mark_homogeneous(&mut top);
        return top;
    }

    let metrics: Vec<_> = metric_to_cores.keys().copied().collect();
    let (Some(&lowest_metric), Some(&highest_metric)) = (metrics.first(), metrics.last()) else {
        return top;
    };

    // A machine needs at least a 1.3x capacity difference before it is
    // considered heterogeneous; below that the noise is larger than the
    // difference. The same 1.3x threshold is used by the reference daemon, so
    // the same machine is classified the same way by both.
    if (highest_metric as f64) < (lowest_metric as f64) * 1.3 {
        debug!(
            "detect_topology: Max capacity ({}) is not >= 1.3x min capacity ({}). Assuming homogeneous.",
            highest_metric, lowest_metric
        );
        mark_homogeneous(&mut top);
        return top;
    }

    // Cores below the mean capacity form the little tier, the rest the big one,
    // and the highest-capacity tier is the turbo one.
    //
    // The mean is an `f64` here and an integer in the reference
    // (`topology.cpp:95` — `sum_rcap / nr_cpus` into a `std::size_t`, compared
    // with `>=` at `:117`). The two agree everywhere except where a capacity tier
    // lands exactly on the truncated mean, and there the truncation is what makes
    // the reference wrong: for two CPUs of capacity {1, 2} its `avg` is 1, so
    // `1 >= 1` classifies the capacity-1 core as *big* and leaves `little-cores`
    // empty, when it is plainly the little one. The f64 threshold is 1.5 and puts
    // it where it belongs.
    //
    // The 1.3x heterogeneity test above is unaffected: it was brute-forced over
    // the integer range and the two do not diverge there.
    //
    // The turbo tier matches the reference's `BigTurbo` — the highest-capacity
    // cores, but only when the machine is heterogeneous and the mean is below the
    // maximum (`topology.cpp:110-112`). A homogeneous machine leaves
    // `turbo-cores` empty for the same reason it leaves `little-cores` empty.
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
    top.has_big_little = true;

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
