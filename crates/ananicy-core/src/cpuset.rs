use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuSet {
    max_cores: u32,
    cores: Vec<bool>,
}

impl CpuSet {
    pub fn new(max_cores: u32) -> Self {
        Self {
            max_cores,
            cores: vec![false; max_cores as usize],
        }
    }

    pub fn set_cpu(&mut self, cpu: u32) {
        if cpu < self.max_cores {
            self.cores[cpu as usize] = true;
        }
    }

    pub fn clear_cpu(&mut self, cpu: u32) {
        if cpu < self.max_cores {
            self.cores[cpu as usize] = false;
        }
    }

    pub fn zero(&mut self) {
        self.cores.fill(false);
    }

    pub fn valid(&self) -> bool {
        true
    }

    pub fn has_cpu(&self, cpu: u32) -> bool {
        cpu < self.max_cores && self.cores[cpu as usize]
    }

    pub fn is_empty(&self) -> bool {
        !self.cores.iter().any(|&c| c)
    }

    pub fn get_cores(&self) -> Vec<u32> {
        self.cores().collect()
    }

    /// The CPUs in the set, without materialising a `Vec` of them.
    ///
    /// The same information as [`CpuSet::get_cores`], lent rather than owned.
    /// `get_cores` is what a caller wants when it is going to keep the list; it
    /// is not what `set_affinity` wants, because that walks the mask it is
    /// building anyway and was allocating a vector per process to do it — and a
    /// second one, on top, only to ask whether the set was empty.
    pub fn cores(&self) -> impl Iterator<Item = u32> + '_ {
        self.cores
            .iter()
            .enumerate()
            .filter_map(|(i, &b)| b.then_some(i as u32))
    }

    pub fn parse(s: &str, max_cores: u32) -> Option<Self> {
        let mut cpuset = Self::new(max_cores);
        let s = s.trim();
        if s.is_empty() {
            return None;
        }

        // `peekable` rather than `collect()`: the loop only ever needed to know
        // whether an empty token was the last one, which is what a peek answers,
        // and this is on the per-process path for every rule with a cpuset.
        // The `Vec` of tokens was one allocation per process to hold a list
        // that is walked once.
        let mut tokens = s.split(',').peekable();
        while let Some(token) = tokens.next() {
            let token = token.trim();
            if token.is_empty() {
                if tokens.peek().is_none() {
                    continue; // Allow trailing comma
                }
                return None; // Do not allow double commas or empty tokens like ',,'
            }

            if let Some((start_str, end_str)) = token.split_once('-') {
                let start: u32 = start_str.parse().ok()?;
                let end: u32 = end_str.parse().ok()?;

                if start > end || end >= max_cores {
                    return None;
                }

                for cpu in start..=end {
                    cpuset.set_cpu(cpu);
                }
            } else {
                let cpu: u32 = token.parse().ok()?;
                if cpu >= max_cores {
                    return None;
                }
                cpuset.set_cpu(cpu);
            }
        }

        Some(cpuset)
    }
}

impl fmt::Display for CpuSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ranges = Vec::new();
        let mut in_range = false;
        let mut range_start = 0;

        for i in 0..self.max_cores {
            let active = self.cores[i as usize];
            if active && !in_range {
                in_range = true;
                range_start = i;
            } else if !active && in_range {
                in_range = false;
                if i - 1 == range_start {
                    ranges.push(format!("{}", range_start));
                } else {
                    ranges.push(format!("{}-{}", range_start, i - 1));
                }
            }
        }

        if in_range {
            let i = self.max_cores;
            if i - 1 == range_start {
                ranges.push(format!("{}", range_start));
            } else {
                ranges.push(format!("{}-{}", range_start, i - 1));
            }
        }

        write!(f, "{}", ranges.join(","))
    }
}
