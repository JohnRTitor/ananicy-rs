# Memory

What the daemon uses memory for, how much of it is the rule set, and how to
find out on your own machine. The two unit settings that bound it are
`MemoryHigh=48M` and `MemoryMax=96M`, and why they differ from the reference's
16M/64M is in
[COMPATIBILITY § 1](./COMPATIBILITY.md#1-project-identity-and-configuration).

## What it is made of

Two things, and only one of them is worth watching.

**The daemon's own cost is fixed.** Roughly 29 MB, of a 39.4 MB peak, before a
single rule is loaded: the binary and its libraries in page cache, the eBPF maps
and perf buffers charged as kernel memory, the dentry and slab the procfs reads
leave behind, page tables. It does not move with your rule set, and it is the
floor a `MemoryMax` has to clear before rule count is even relevant.

**The rule set scales with the number of rules**, and that is the part to watch,
because it is not proportional. 2.1 MB of rule files is 15,831 rules and 10.4 MB
of memory; the files themselves are page cache the kernel will reclaim under any
pressure at all, so the count is what matters and the disk size is not.

## Why the count matters more than the bytes

A rule is 72 bytes of fixed-size fields, and the five names it carries — its
`type`, `ioclass`, `sched`, `cgroup` and `cpuset` — are two bytes each, so a
rule set pays for a couple of dozen distinct names once rather than per rule. The
rules live in one contiguous `Vec`, and the map that finds them holds a `u32`
index rather than the rule, so a bucket is 28 bytes.

Measured on a 12-core host, same machine and same build, as peak RSS:

| Rules | Each rule a parsed JSON document | Rules in the map | Rules in a `Vec` |
|------:|---------------------------------:|-----------------:|----------------:|
| 15,831 | 24.0 MB | 15.6 MB | **10.4 MB** |
| 30,000 | — | 26.7 MB | **15.4 MB** |
| 60,000 | — | 49.3 MB | **26.7 MB** |
| 120,000 | — | — | **49.4 MB** |

Those figures predate the changes below and are left as they were measured. The
rule set's own cost has since come down by half; see § What the names cost.

The index matters more than its size suggests, because a hash map's bucket count
is the next power of two above `count * 8/7`. With the rules in the map, every
bucket carries 200 bytes and the set's cost becomes a staircase:

| | |
|---|---|
| 28,670 rules | 16,080 kB |
| 28,673 rules | 26,844 kB — **one extra rule, 10.7 MB** |
| 57,342 rules | 27,924 kB |
| 57,345 rules | 49,096 kB — **one extra rule, 21.2 MB** |

With the rules in a `Vec` the same three rules cost 1.2 MB and 3.1 MB. Nothing
in a rule file indicates a boundary is near, so a user crossing one sees memory
double for no visible reason — which is the failure mode worth knowing about and
the reason the current layout is the one it is.

The current numbers are worth putting next to a cap. Against the unit's
`MemoryHigh=48M`, the default rule set peaks at 34.4 MB and the soft line is
crossed at roughly **57,000 rules**, about 3.6× the default set. Game rule sets
grow, and `extraRules` can cross that without anyone noticing.

## What the names cost, which is not what the struct size says

A `Rule` is 72 bytes. That number was 176, and the difference is 96 bytes of
names that a rule set of any size shares: four `Option<Option<Arc<str>>>` at 24
bytes each, holding a few dozen distinct values across fifteen thousand rules.
Twenty-four rather than sixteen because the inner `Option` spends the
null-pointer niche and the outer one needs a discriminant of its own.

They are two-byte indices into a process-wide table now, `type_name` included.
Counting live heap across a full load:

| | Rules | Live bytes | Live allocations | Per rule |
|---|---:|---:|---:|---:|
| 176-byte rule, type resolved per rule | 15,831 | 5,243,345 | 63,479 | 331 B, 4.01 allocs |
| 176-byte rule, type resolved once | 15,831 | 4,483,505 | 31,819 | 283 B, 2.01 allocs |
| 168-byte rule (`extras` boxed) | 15,831 | 4,352,433 | 31,819 | 275 B, 2.01 allocs |
| **72-byte rule (names interned)** | **15,831** | **2,654,357** | **16,007** | **168 B, 1.01 allocs** |
| | | **−49.4%** | **−74.8%** | |
| 176-byte rule, type resolved per rule | 120,000 | 40,391,580 | 480,136 | 337 B, 4.00 allocs |
| **72-byte rule (names interned)** | **120,000** | **20,041,576** | **120,157** | **167 B, 1.00 allocs** |
| | | **−50.4%** | **−75.0%** | |

The remaining allocation per rule is the rule's own name, which is the key of
the map that finds it and has to be stored. The remaining bytes are the `Vec` of
rules and that map's buckets.

Two of those rows were the same bug found twice. The middle one is a separate
defect from the interning: the loader resolved each type *inside* the per-rule
loop, and `from_json` allocates a fresh `Arc<str>` for every name it reads, so
every rule inherited a private copy of a string fifteen thousand rules share. It
was invisible from the struct size and invisible in peak RSS, and it was worth
14.5% of the rule set on its own.

## Those numbers are measured now, and how

`cargo bench --bench alloc` prints the last two rows of that table from a
counting `GlobalAlloc`: live bytes and live allocation count, taken as a delta
around a full load, over a rule set generated to the shape of the shipped one.
It reproduces the 72-byte row from scratch — 160 B/rule and 1.00 allocs/rule
against the table's 168 and 1.01 — so those figures are re-derivable rather
than asserted. The generator is used in preference to reading `/etc/ananicy.d`
because a benchmark that measures whatever is installed measures the machine
rather than the code, and stops being comparable the day upstream changes.

`cargo bench --bench rules` owns timing. The two that matter for a change here
are `rules_get_cache_{1,100}_rules/{hit,miss}`; the older
`rules_get_cache_miss` measures a lookup against an *empty* rule set repeating
one name, so it never misses the cache and never reaches the regex loop.

Two lessons from building these, both of which cost a wrong conclusion first:

- **A measurement taken against the wrong tree is worse than none.** The
  instrument read 3.96 allocs/rule and appeared to contradict the table above.
  The table was right; the branch was not — it predated the interning work, and
  `docs/MEMORY.md` on that branch does not have these rows at all. The only
  reason it was caught is that a published number existed to contradict it.
- **A benchmark's first case in a group pays setup.** A combined run once
  reported the 1-rule cache *hit* at 14.7 ns while the 100-rule hit measured
  8.2 ns, which is not something a hasher can do. Re-run alone, both are 8.2
  ns. A real regression there would have looked identical.

## Two invariants that are properties, not lists

Two of the optimisations in this tree are only correct if a small function is
right about *every* input, and both got it wrong once. They are worth writing
down together because the failure is the same and it is quiet.

The `name_regex` prefilter decides, before entering the engine, that a name
cannot match a pattern. Getting that wrong does not produce a wrong answer —
it produces a rule that silently stops applying to some process names, which
looks identical to a rule that has stopped working at all. The first version
prefilted `^m?\w+ode$` on `m`, and `?` allows zero occurrences, so `node`
matches that pattern and was rejected.

The CPU mask's width was `max_cores / 8` with its bits set behind a
`cpu < max_cores` guard, so any machine whose CPU count is not a multiple of
eight had a mask one byte too short for its own highest CPU, and naming that
CPU indexed past the end of the buffer — a panic inside rule application.
Unreachable only because `get_max_number_of_cpus` floors at 1024.

Both were caught by tests, which is the good part. But both guards were
*lists* — eleven patterns that should and should not be prefilterable, ten CPU
widths — and a list is the wrong shape for a function whose job is universal
correctness. Writing the prefilter is exactly how `^m?` got through: eleven
shapes considered, quantifiers not among them. A second such bug is caught by
a different list, and the third by neither.

So both are now stated as properties, and each was checked by reintroducing the
bug and watching it fail:

- `a_prefilter_only_ever_rejects_a_name_the_pattern_does_not_match` generates
  patterns from the constructs that make a required first byte hard — anchors,
  quantifiers, classes, groups, inline flag groups, multi-byte literals —
  crossed with names from an alphabet containing the patterns' own literals,
  and asserts the one-sided invariant `!may_match(name) => !is_match(name)`.
  With the quantifier check removed it fails on `pattern = "^a?"`,
  `subject = "b"`.
- `the_kernel_mask_is_whole_bytes_and_holds_every_cpu` covers every width in
  `0..600` rather than ten chosen ones, asserting both that the mask is
  `ceil(max_cores / 8)` bytes and that it agrees with the set on every CPU. With
  the width reverted to `/ 8` it panics, which is the original defect.

The rule this establishes: a guard for a universal invariant is a property, and
a property that has never been seen failing is not known to work. Both of
these were run against a deliberately reintroduced bug before being accepted.


## Hashing

`std`'s `RandomState` is SipHash-1-3, which is a deliberate choice for keys an
adversary picks and a slow one for keys that are 3-15 bytes. This daemon has
both kinds of key, and they get different answers.

The rule map, the name table, and the resolved-rule cache are keyed on a process
name, which is the basename of `argv[0]` — a `MAX_ARG_STRLEN` string any local
process writes itself. That is the one key here an adversary chooses, so these
use `foldhash::quality`: faster than SipHash on short keys, and not the cheapest
option available. `foldhash::fast` would be faster still and is the one to
avoid, because degrading a lookup into a bucket-chain walk costs a local
process a few microseconds and costs the daemon a rule set's worth of
comparisons. The middle option is worth the few nanoseconds it gives up.

The caches keyed by pid — `ReportedNames`, the cgroup resolver, the
`/proc/<pid>/exe` failure cache — use it too, for the opposite reason: their
keys are 4-byte integers, so there is no string to collide and nothing to
defend.

Nothing new enters the build for any of this. `lru` pulls in `hashbrown`, which
pulls in `foldhash`, so the crate was already compiled before it was named.

**Peak RSS shows almost none of this.** The 15,831-rule set moved 260 kB of peak
RSS for the 760 kB that the type fix removed, and the interning is similarly
invisible there, because the peak is set by the transient work of loading — the
712 kB file read, a parsed JSON document per line — and the allocator hands those
freed chunks straight to the live allocations. A freed-then-reused page was
already resident. If you are looking for this class of change, count live bytes
with a `GlobalAlloc`; RSS answers a different question, and the one it answers
well is the fixed cost above.

`a_rule_is_the_size_the_memory_budget_assumes` pins the struct size, and
`rules_inheriting_one_type_share_its_names` and
`an_attribute_of_the_wrong_type_is_declared_but_not_usable` hold the two
properties the interning has to keep: names the rules share resolve to the same
index, and a name declared with nothing usable in it stays distinct from one the
rule said nothing about, or `"ioclass": null` stops suppressing the type's value.

## Where the time goes, which is not where the memory is

The table above is about a rule set held in memory. The per-process path is a
different budget and a different ranking, and the two do not agree.

| | |
|---|---:|
| `get_command_from_pid` | 10.3 µs |
| `rules_get_cache_100_rules/miss` | 1.19 µs |
| `rules_get_cache_1_rules/miss` | 60 ns |
| `rules_get_cache_1_rules/hit` | 8.2 ns |
| `cpuset_parse` | 51 ns |

Resolving a process name costs **170x an entire uncached rule lookup** and
**1300x a cached one**. It is the dominant cost in the daemon by a wide
margin, and it is `cargo bench --bench procfs`.

That function is syscall-bound, not allocation-bound: four `fs` calls on
procfs are open/read/close each, and those are the 10 µs. The lever is
therefore *fewer syscalls*, not cheaper bookkeeping around them — which is
why the fix that mattered there was to stop reading `/proc` for a `Fork`
event that cannot have a name yet, rather than the one that stopped
rebuilding the same `/proc/<pid>` path four times. The latter removed four
allocations and bought 3%; it was kept because those allocations are real
under a `MemoryHigh`, not because it was fast.

The rule-matching numbers are only large for a rule set with many
`name_regex` rules. The shipped set has exactly one, which is why the
100-rule row is three orders of magnitude above the 1-rule row and why the
two have moved independently throughout: hashing and the `Vec`→bitset
changes show up in the 1-rule row, the regex scan and its prefilter show up
in the 100-rule row, and the shipped daemon only ever runs the first.

## Finding out on your own machine

```sh
ananicy-rs debug memory          # one snapshot: heap, cgroup breakdown, faults
ananicy-rs start --memory-stats  # the same, with per-minute deltas, while running
```

`debug memory` prints the heap from `/proc/self/status` and the cgroup's own
figures — `memory.current`, `memory.high`, `memory.max`, the `memory.stat`
breakdown, the page-fault and refault counters, swap in and out, and the
block-device totals. It then says whether the numbers mean anything, and is
silent when they are healthy.

Two of those numbers answer "is the kernel making the daemon re-fetch its own
pages", which is the question that matters when I/O appears out of nowhere:

- **`memory.events` `high`** — how many times the cgroup has been told to
  reclaim. It should be flat. A number climbing without bound is the daemon
  above its `MemoryHigh`.
- **`pgmajfault` against `pgfault`** — the ratio of faults that had to go to
  disk. A healthy daemon is near zero. A high ratio means it is re-reading its
  own mapped files, and the reads will average tens of kilobytes, because that
  is readahead against its executable.

Both are in cgroupfs, one level above anything a process can see about itself,
which is why they are reported from inside rather than left to `cgroupfs`.
