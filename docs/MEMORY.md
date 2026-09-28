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
