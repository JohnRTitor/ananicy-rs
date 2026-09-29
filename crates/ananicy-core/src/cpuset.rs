use std::fmt;

/// A set of CPUs, as a bitset.
///
/// The set used to be a `Vec<bool>` — one byte per CPU, of which
/// `get_max_number_of_cpus()` is never fewer than 1024, so every parse
/// allocated at least a kilobyte to hold a handful of bits, and then walked all
/// of it. A `Box<[u64]>` holds the same information in a sixteenth of the
/// space, and `is_empty` and `cores()` read it a word at a time rather than a
/// byte at a time.
///
/// The kernel's own mask is built here rather than at the `sched_setaffinity`
/// call, because it is a pure function of the set and the machine's CPU count,
/// and this is on the per-process path: a rule with a `cpuset` used to derive
/// the same bytes from the same set for every process it touched. The cost is
/// that the mask is fixed at the width the set was built for, so a set parsed
/// for 8192 CPUs cannot be applied to a 16384-CPU machine — which is not a
/// situation that arises, because both come from `get_max_number_of_cpus`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuSet {
    max_cores: u32,
    cores: Box<[u64]>,
}

impl CpuSet {
    pub fn new(max_cores: u32) -> Self {
        Self {
            max_cores,
            cores: vec![0u64; words_for(max_cores)].into_boxed_slice(),
        }
    }

    /// The kernel's CPU mask for this set: one bit per CPU, little-endian
    /// within each byte, which is the layout `sched_setaffinity(2)` wants.
    ///
    /// `sched_setaffinity` requires `len` to be at least the size of the kernel's
    /// own `cpumask`, which is why this is derived from `max_cores` rather than
    /// from the highest set CPU — a set naming only CPU 0 still has to hand over
    /// a full-width mask.
    ///
    /// The width is `ceil(max_cores / 8)`, and the ceiling is the point rather
    /// than an off-by-one to be tidied away. The mask used to be `max_cores / 8`
    /// bytes with its bits set from a `cpu < max_cores` guard, so a machine
    /// whose CPU count is not a multiple of eight had a mask one byte too short
    /// for its own highest CPU, and setting that CPU indexed past the end of
    /// the buffer. It was unreachable in practice for an unrelated reason —
    /// `get_max_number_of_cpus` floors at 1024, and 1024 is a multiple of eight —
    /// which is the only reason it had not already bitten on a machine with,
    /// say, 1026 CPUs. Rounding up makes every CPU in the set representable, and
    /// a mask one byte longer than the kernel's own is exactly what
    /// `sched_setaffinity` wants anyway.
    pub fn kernel_mask(&self) -> Vec<u8> {
        let mask_bytes = (self.max_cores as usize).div_ceil(8);
        let mut mask = vec![0u8; mask_bytes];
        for (cpu_index, word) in self.cores.iter().enumerate() {
            let base = cpu_index * 64;
            for bit in 0..64 {
                if word & (1u64 << bit) != 0 {
                    let cpu = base + bit;
                    if cpu / 8 < mask_bytes {
                        mask[cpu / 8] |= 1 << (cpu % 8);
                    }
                }
            }
        }
        mask
    }

    pub fn set_cpu(&mut self, cpu: u32) {
        if cpu < self.max_cores {
            self.set_bit(cpu as usize);
        }
    }

    pub fn clear_cpu(&mut self, cpu: u32) {
        if cpu < self.max_cores {
            self.clear_bit(cpu as usize);
        }
    }

    pub fn zero(&mut self) {
        self.cores.fill(0);
    }

    pub fn valid(&self) -> bool {
        true
    }

    pub fn has_cpu(&self, cpu: u32) -> bool {
        cpu < self.max_cores && self.bit(cpu as usize)
    }

    /// Whether the set names no CPU at all.
    ///
    /// A word-at-a-time scan of the bitset rather than a byte-at-a-time one over
    /// at least 1024 bytes, and this is asked once per process with a `cpuset`
    /// rule to decide whether there is anything to do at all.
    pub fn is_empty(&self) -> bool {
        self.cores.iter().all(|&word| word == 0)
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
            .flat_map(|(word_index, &word)| {
                // A set bit is a CPU. Sparse words are the common case — a rule
                // names a handful of CPUs out of a thousand — so the inner loop
                // skips whole empty words without an inner branch per bit.
                (0..64).filter_map(move |bit| {
                    (word & (1u64 << bit) != 0).then_some((word_index * 64 + bit) as u32)
                })
            })
    }

    fn bit(&self, cpu: usize) -> bool {
        self.cores
            .get(cpu / 64)
            .is_some_and(|&word| word & (1u64 << (cpu % 64)) != 0)
    }

    fn set_bit(&mut self, cpu: usize) {
        if let Some(word) = self.cores.get_mut(cpu / 64) {
            *word |= 1u64 << (cpu % 64);
        }
    }

    fn clear_bit(&mut self, cpu: usize) {
        if let Some(word) = self.cores.get_mut(cpu / 64) {
            *word &= !(1u64 << (cpu % 64));
        }
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

/// The `u64` words a set of `max_cores` CPUs needs.
fn words_for(max_cores: u32) -> usize {
    // At least one word, so a zero-CPU machine still has somewhere to look for
    // a bit in rather than indexing an empty slice on every `has_cpu`.
    (max_cores as usize).div_ceil(64).max(1)
}

impl fmt::Display for CpuSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ranges = Vec::new();
        let mut in_range = false;
        let mut range_start = 0;

        for i in 0..self.max_cores {
            let active = self.has_cpu(i);
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
