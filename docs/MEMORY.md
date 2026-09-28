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

A rule is 176 bytes of fixed-size fields, and the names it shares with other
rules — its `type`, `ioclass`, `sched`, `cgroup` and `cpuset` — are one
allocation between all the rules naming the same thing rather than one each. The
rules live in one contiguous `Vec`, and the map that finds them holds a `u32`
index rather than the rule, so a bucket is 28 bytes.

That sharing is not free and not automatic: it holds because the loader resolves
each type once and every rule that inherits it takes a reference to the same
string. The next section says what happens when it does not.

Measured on a 12-core host, same machine and same build, as peak RSS:

| Rules | Each rule a parsed JSON document | Rules in the map | Rules in a `Vec` |
|------:|---------------------------------:|-----------------:|----------------:|
| 15,831 | 24.0 MB | 15.6 MB | **10.4 MB** |
| 30,000 | — | 26.7 MB | **15.4 MB** |
| 60,000 | — | 49.3 MB | **26.7 MB** |
| 120,000 | — | — | **49.4 MB** |

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

## What the names cost, which is not what 176 bytes says

176 bytes is the `Rule` struct. It is not what a rule set keeps, because a rule
also points at its type name, and the four name fields an inherited rule carries
point at strings every other rule with the same type points at too.

That is only true if they point at the *same* string. They do — the loader
resolves each type once and `inherit` copies references, not contents — but this
was not always so, and the difference is worth recording because it is invisible
from the struct size and from peak RSS.

Resolving the type inside the per-rule loop, which is what the loader used to do,
made every rule's `ioclass` and `sched` its own private allocation: the string
was parsed afresh per rule, so the `Arc` each rule ended up holding was
referenced by exactly one rule. A 15,831-rule set drew on two distinct values and
made 31,662 allocations for them.

Counting live heap across a full load, before and after:

| | Rules | Live bytes | Live allocations | Per rule |
|---|---:|---:|---:|---:|
| Type resolved per rule | 15,831 | 5,243,345 | 63,479 | 4.01 allocations |
| Type resolved once | 15,831 | 4,483,505 | 31,819 | 2.01 allocations |
| **Difference** | | **−759,840 (−14.5%)** | **−31,660** | **−2** |
| Type resolved per rule | 120,000 | 40,391,580 | 480,136 | 4.00 allocations |
| Type resolved once | 120,000 | 34,631,628 | 240,138 | 2.00 allocations |
| **Difference** | | **−5,759,952 (−14.3%)** | **−239,998** | **−2** |

Two allocations and 48 bytes per rule, at every scale, for strings a handful of
distinct values cover.

**Peak RSS does not show this.** It moved by 260 kB on the 15,831-rule set, not
760 kB, because the peak is set by the transient work of loading — the 712 kB
file read, a parsed JSON document per line — and the allocator hands those freed
chunks straight back to the live allocations. A freed-then-reused page was
already resident. If you are looking for this class of change, count live bytes
with a `GlobalAlloc`; RSS answers a different question, and the one it answers
well is the fixed cost above.

`rules_inheriting_one_type_share_its_names` and
`a_cgroup_name_inherited_from_a_type_is_shared_too` assert the `Arc` reference
counts directly, so a return to per-rule resolution fails the suite rather than
quietly costing 48 bytes a rule.

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
