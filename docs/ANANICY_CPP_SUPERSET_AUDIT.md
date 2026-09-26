# ananicy-rs vs ananicy-cpp — Superset Audit

**Audit date:** 2026-09-26
**Target:** `/home/masum/Dev-Environment/Rust/ananicy-rs` @ `f7d988d`
**Reference:** `/home/masum/Dev-Environment/Rust/ananicy-cpp` (source tree as found, no VCS history)
**Second reference:** `RogueScholar/ananicy` @ `ce7b19b` — the Python original both rewrites descend from
**Exception list under audit:** `docs/ANANICY_CPP_DIFFERENCES.md`

> **A note on cross-references.** Where this document says "the previous audit's §4.2", "§5.20", or
> "§12.6", that is the numbering of the version of this file as it stood at `e131b18`, which is in
> the git history (`git show e131b18:docs/ANANICY_CPP_SUPERSET_AUDIT.md`) and not in this file. The
> row numbers in §4 are that document's, kept deliberately so the two can be read side by side.
> Section numbers without a qualifier — §5, §7, §10 — are this document's.

> **This document supersedes the audit written against `e131b18`.** That one is 30 commits out of date:
> of its 232 citations into this tree, 70 point at lines that have since moved, 32 have been made
> obsolete by a fix, one is materially false and unrecorded, and its test counts and tallies are
> wrong. Rather than amend a document whose line numbers are all stale, it has been rebuilt.
> §3 records what survived the re-verification and what did not; §4–§9 are the current state.
> The remediation history is §10.

**Verdict: not yet, and closer.** The pass found two High defects; both have been fixed
(`cd77276`, `de694d4`) and each fix carries a test that fails without it. What stands between this
daemon and a documented superset is now six undocumented Medium differences (§7.3) and the
`ananicy-cpp` build itself never having been executed here (§9).

---

## 1. Executive summary

`ananicy-rs` is a close functional match to `ananicy-cpp` and is the better implementation on
almost every axis that does not change observable behaviour. The rule engine, the attribute
surface, the process-discovery ladder, both event backends, the cgroup v1/v2 handling, the topology
and X3D machinery and the diagnostics all have counterparts with equivalent semantics, and the test
suite backing that claim is large, hermetic and green: **340 tests across 20 targets, 0 failures**, and the
`ananicy-bpf` crate builds and passes under `nix develop`.

It is nevertheless not a superset yet, and the reason is now a category rather than a list. Both
High findings of §7 are fixed (`cd77276`, `de694d4`), as are nine of the nineteen entries in §5 —
one commit each, each with a test that fails without it. One more (§5.18) turned out on
verification to be wrong about which socket option is set, and one (§5.19) was real but
narrower than stated. What is left are **seven differences where matching `ananicy-cpp` would mean
reproducing a defect in it**, documented in `ANANICY_CPP_DIFFERENCES.md` §5.1 and §5.2:

* `oom_score_adj` is read as `unsigned` by the reference, so `-900` is reported as `4294966396`.
* A type is merged into a rule *twice*, so an explicit `null` survives into the finished rule and
  the worker's catch-all then applies nothing at all from it.
* The core-type split uses an integer mean, so on capacities `{1, 2}` the capacity-1 core is
  classified *big* and `little-cores` comes out empty.
* A deleted binary's name keeps a trailing space, so no rule matches it after a package upgrade.
* Five `EACCES` on `/proc/*/exe` for any five processes permanently disables exe-based naming.
* An unreadable CPU is counted as a capacity value rather than as no data.
* Cgroup detection stops at the first mount rather than the best one.

That is the honest state of a rewrite whose reference has bugs: parity is not the goal, and
reproducing a defect to achieve it would cost a working rule. **No entry in §5 now describes a
known defect in this daemon** — the register is either fixed, corrected, or documented as
deliberate. What remains is the standing caveat of §9: the C++ daemon has never been executed here,
so every claim about its behaviour is source reading, and §5.5's hybrid-host case is a real open
question with no obviously correct answer rather than a bug on either side.

One matrix row in the previous audit was demonstrably wrong (§3.4) and one §12 non-finding is
contradicted by it (§3.4).

**Everything the previous audit reported is fixed and recorded** (§10), one commit per finding, with
the exception of a log-level change (`74793b4`) that fixed a finding without a row. This pass added
thirteen of its own — §7.1, §7.2 and nine of §5 — and recorded the seven it chose not to fix, with
the reason in each case.

### Confidence

* **High** for every finding backed by a citation *and* executed against this host: §7.2 (verified
  live, 2223/2223 `null` before the fix) and §7.1 (reproduced in a mount namespace with no hierarchy),
  §5.10 and §5.11 (both executed).
* **High** for the C++ side of every finding, by source reading. **The C++ daemon could not be
  built in this environment** — `cmake` is absent and the project fetches its dependencies over the
  network (`CPMAddPackage`) — so no C++ claim rests on execution. The control-flow arguments were
  read end to end rather than sampled.
* **Medium** for §5.5 and §5.12: the divergence is proven from source
  on both sides but the triggering host (a container with an undelegated `cpu` controller, a hybrid
  v1+v2 host, a multi-LLC machine) is not available here to demonstrate it.
* **Medium** for the topology findings in general — they need a big.LITTLE machine, and the audit
  host is homogeneous (12 CPUs, 1 LLC, 1 NUMA node, no big.LITTLE). The counter-examples in §5.8 and
  §5.12 were found by exhaustive search over the two formulas rather than on real hardware.
* **Assessed by build:** `nix develop` provides libbpf, so the BPF backend compiles and
  `nix develop --command cargo test --workspace` passes with it. The `--verbose` plumbing into libbpf
  is therefore at least compile-checked, which the first version of this document could not claim.
  §8.1 records the comparison, and it stays at source level: the crate has no tests of its own and
  exercising the path needs root and a BPF-capable kernel. The two `ananicy_cpp.bpf.c` files are
  byte-identical, which is checked by `diff`.
* **Not assessed:** `ananicy-cpp` has no VCS history, so "intentional vs accidental" judgements rely
  on comments, tests and the differences document rather than commit messages.

---

## 2. Method

Three passes, in this order.

**Pass 1 — citation re-verification.** Every `file:line` in the previous audit was extracted (232
into this tree, 152 into `ananicy-cpp`) and checked against the current source. Rust-side
citations are crate-relative in that document (`config.rs:355` means
`crates/ananicy-core/src/config.rs:355`), so each was resolved against the layout in §2.1 and the
surrounding claim re-read — a citation that exists but points at an unrelated line counts against
it. Line shifts were mapped by `git diff -U0` hunk arithmetic between `e131b18` and `0d958ee` and
then content-verified.

**Pass 2 — fresh divergence hunt.** Deliberately *not* a re-check: the previous audit was assumed to
have blind spots and the hunt was for divergences it never found. Ten areas were covered
systematically — configuration parsing, rule parsing, attribute application, process discovery,
cgroup handling, CLI surface, diagnostics, lifecycle, numerical edges, and the panic/abort
inventory. Candidate findings were discarded once checked (see §8.3 for the ones thrown out, which
matters as much as the ones kept: a wrong finding costs more than a missing one).

**Pass 3 — targeted verification of pass 2's High findings by execution**, because they are the ones
that decide the verdict.

**Everything not verifiable by execution is marked.** §9 lists what that leaves untested.


### 2.1 Crate layout

```
crates/ananicy-core/src/     config.rs worker.rs rules.rs types.rs cpuset.rs process.rs cgroup.rs lib.rs
crates/ananicy-core/tests/   config.rs config_logging.rs cpuset.rs property_tests.rs
                             rules.rs worker_logging.rs worker_rules.rs
crates/ananicy-platform/src/ mounts.rs procfs.rs priority.rs topology.rs x3d.rs netlink.rs
                             service.rs process_info.rs cgroups.rs lib.rs
crates/ananicy-platform/src/cgroup/  manager.rs ownership.rs process.rs
crates/ananicy-platform/src/abi/     affinity.rs ioprio.rs sched_attr.rs sched.rs
crates/ananicy-platform/tests/ affinity.rs cgroup_manager.rs cgroups.rs ioprio.rs mounts.rs procfs.rs topology.rs
crates/ananicy-platform/fuzz/fuzz_targets/  parse_cpuset.rs parse_mounts.rs parse_rule.rs
crates/ananicy-bpf/src/      bpf_monitor.rs lib.rs
src/                          main.rs cli.rs dump.rs runtime.rs startup.rs monitor.rs ipc.rs
                             panics.rs signals.rs systemd.rs disks.rs
tests/cli.rs   benches/rules.rs
```

---

## 3. Re-verification of the previous audit

Three passes were run over the previous document: resolve every citation, decide whether the claim
around it still holds, and record the arithmetic.

**Rust side — 232 extracted citations, all 232 resolved: 127 confirmed, 70 stale line, 32 obsolete
by a fix, 1 materially false, 2 suggested-paths (not defects), 0 wrong.** The tree moved by 30
commits, so 102 of the 232 needed attention; every one of those 102 turned out to be either a
mechanical line shift or a change §10 already records. The single materially false item is not a
citation error but an omission — §3.3.

**C++ side — 152 extracted, of which 134 resolved to distinct `file:line` targets** (the other 18
are bare filenames, the §2 method file list, and Rust-side paths that the extraction picked up).
**127 confirmed, 2 stale line, 5 wrong, 0 not-found.** `ananicy-cpp` has not changed, so this
measures the original audit's citation discipline rather than drift: **95% of its citations land
exactly on the code they describe**, and every load-bearing conclusion drawn from them survives
re-reading.


### 3.1 The C++ side

Nothing in `ananicy-cpp` changed, so all five C++ defects are citation errors, not drift. Three are
the same mistake: the file is `include/config.hpp`, cited as `src/config.hpp` (previous audit §3
rows 24, 27, 29). The line numbers are right, so no conclusion changes. The other two:

| Previous audit item | Cited | What is there | Correct |
|---|---|---|---|
| Previous audit §3 row 115 | `process.cpp:163` | `void ProcessQueue::init()` — the `full_scan()` is line 164 | `process.cpp:164` (netlink), `:32` (BPF) |
| Previous audit §5.18 | `netlink_program_utils.c:76-79` | blank lines and a function header; the `SO_RCVTIMEO` `setsockopt` is 31 lines away | `netlink_program_utils.c:45-49` |

Two ranges are cosmetic over-inclusion: `main.cpp:48-52` (the `--verbose` flag is 48-51) and
`cpuset.cpp:203-205` (the `sysconf` is at 106-110).


### 3.2 The materially false C++ claim (§4.5)

 The previous audit's §4.5 claimed the two event-source debug tools are "shipped, buildable artifacts of the reference
project… Both are built by `add_subdirectory` in `CMakeLists.txt:196-205`".** They are not. That
range only adds the *library* subdirectories. The executables are gated behind options that default
to OFF:

* `libananicycpp_bpf/CMakeLists.txt:11` — `option(BPF_BUILD_SAMPLES "Build ananicy_cpp_bpf samples" OFF)`, `add_executable(runqslower_cpp …)` at `:155`
* `libananicycpp_netlink/CMakeLists.txt:8` — `option(NETLINK_BUILD_SAMPLES "Build ananicy_cpp_netlink samples" OFF)`, `add_executable(netlink_proc_cpp …)` at `:23`

A default `cmake && make` of `ananicy-cpp` produces neither binary. This weakens that finding's "why a
regression" argument — they are opt-in developer samples, not shipped artifacts. It does not change
the decision (§10 row 20: document their absence, with the reason corrected).


### 3.3 The materially false Rust claim (an unrecorded fix)

**The previous audit's §11 did not record commit `74793b4`**, which is the only behavioural fix
among the 30 that no row accounts for. It changed the `sched: deadline` fallback from `debug!` to
`warn!` (`crates/ananicy-platform/src/priority.rs:165`), which was the "Required action" of matrix
row 49. The matrix row and the §5 register therefore read as open findings, and
`ANANICY_CPP_DIFFERENCES.md` has no entry for it either. Corrected in §5.1 and recorded in §10.

The other three commits unmentioned there are documentation (`9adac44`, `dc48384`, `0d958ee`) and a
pure signature refactor of `runtime::run` into `ProcessEvents`/`RunOptions` (`9190fbf`).


### 3.4 Claims the previous audit got wrong

Four, beyond the citation errors above. Three are now settled and one is new:

* **Matrix row 38 — "type inheritance (merge-patch): same result, precomputed — EQ"** is wrong for a
  rule containing an explicit `null`. See §5.3, verified by execution.
* **Matrix row 68 — "`dump proc` / `dump autogroup`: same fields + `rule`"** describes the field
  *names*. Three *values* differ — see §5.2. `autogroup` was always `null` too, and `cd77276` fixed
  that; see §7.2.
* **Previous audit §12.6 — "Autogroup is not missing… Both rewrites match the original, which reads and never
  writes"** is true of writing and false of the dump the property exists to provide. See §7.2.
* **Matrix row 4 — `--force-remove-semaphore`: EQ.** The success path agrees; the error path does
  not. See §5.9.


### 3.5 Stale tallies

The previous audit's own numbers no longer reconcile, and nothing later corrected them:

| Claim | Said | Actually |
|---|---|---|
| Previous audit §1, §10 | "296 Rust tests pass", "green (296/296)" | **340** |
| Previous audit §11 Verification | "20 test binaries, all passing" | still correct: **20** targets, all passing — 15 integration binaries + 3 unit-test targets + 2 doc-test targets |
| Previous audit §6 | "of the 17 entries above, 13 verify exactly, 3 are accurate but incomplete, 1 is inaccurate" | all four now settled; 16/0/0 |
| Previous audit §1 | "13 undocumented behavioural differences of Medium severity or above" | **6** — the 2 High of §7 are fixed, leaving the 6 Medium of §5 |
| Previous audit §1 | "17 behavioural differences are undocumented" | §5 has **19** subsections, 6 of them Medium or worse and **all 6 undocumented** |
| Previous audit §1 | "§7 — 26 concrete capabilities" | this document's §6 has **32** rows |
| Previous audit §4.2 | the test pinning the re-detection is in `tests/cgroup_manager.rs` | it is `crates/ananicy-platform/src/cgroups.rs:139-183` |
| Previous audit §4.2 | "the return value is discarded (`runtime.rs:51-53`)" | never a correct citation: that range is `if !create_cgroups(&rules) { return; }`, a *check* |

---

## 4. Difference matrix

Refreshed. `EQ` = equivalent, `SUP` = superset (Rust has more), `SUB` = subset, `DIFF` = differs.
Row numbers are the previous audit's, kept so the two documents can be read side by side; three
C++ citations are corrected per §3.1.

| # | Capability | ananicy-cpp | ananicy-rs | |
|---|---|---|---|---|
| 1 | `--verbose` | one step more verbose, clamped at `trace` | same | EQ — the debug-verbosity log line still differs, **see §5.1** |
| 2 | Rule cache | none | 5000-entry LRU | SUP |
| 3 | LRU rule cache sizing | — | `NonZeroUsize::new(5000)` | SUP |
| 4 | `--force-remove-semaphore` error path | exit 1 + message | exit 1 + message | EQ |
| 5 | IPC object permissions | 0600 | 0600 | EQ |
| 6 | Full `/proc` scan | before subscription | concurrent with it | DIFF |
| 7 | `--benchmark-count` | read in the main loop | read once | DIFF |
| 8 | Benchmark spin | `sleep` then break | same | EQ |
| 9 | `--manualscanning` alias | n/a | parses | SUP |
| 10 | `--manualscanning` self-renice position | before the root check | after root + singleton | DIFF |
| 11 | `dump` exit code on misuse | 1 | 2 | DIFF |
| 12 | Bare invocation | help, exit 0 | help, exit 0 | EQ |
| 13 | Unknown `action` | starts the daemon | exits 1 | DIFF |
| 14 | Flag with no action | exit 1 | help, exit 0 | DIFF |
| 15 | `--benchmark-count` type | `uint32_t` | `u32` | DIFF |
| 16 | `--bpf-min-us` type | `uint64_t` | `u32` | DIFF |
| 17 | stdout: version banner | yes | yes | EQ |
| 18 | `--version` | `Version: x` | `Version: x` | EQ |
| 19 | Log destination | stdout | stderr | DIFF |
| 20 | Log level reload | no | yes | SUP |
| 21 | `loglevel` case sensitivity | case-sensitive | case-insensitive | DIFF |
| 22 | `check_freq` parse | `std::stoul` | `str::parse::<u32>`; `0` refused | DIFF — **see §5.6** |
| 23 | `check_freq` default | 60 | 60 | EQ |
| 24 | `apply_cgroup` key name | `apply_cgroup` | `apply_cgroup` | EQ |
| 25 | Config key whitespace trim | spaces only | all whitespace | DIFF |
| 26 | CRLF config file | kept as `\r` in values | handled | DIFF |
| 27 | `x3d_mode` default | `auto` | `auto` | EQ |
| 28 | `x3d_mode` values | `cache`/`frequency` | same | EQ |
| 29 | `loglevel` reload | none | on SIGUSR1 | SUP |
| 30 | Config key set | 15 keys | 17 keys (adds `apply_ioclass`, `apply_cpu_weight`, `check_disks_schedulers`) | SUP |
| 31 | `apply_*` gating | 5 flags wired | 7 flags wired | SUP |
| 32 | `apply_ioclass` | inert | inert | EQ |
| 33 | Rule extensions | `.rules`/`.types`/`.cgroups` | same | EQ |
| 34 | Rule file ordering | `readdir` | sorted, deterministic | DIFF |
| 35 | `name_regex` engine | PCRE2 + UTF + UCP | same | EQ |
| 36 | CRLF rule files | handled | handled | EQ |
| 37 | Exact vs regex precedence | exact first | exact first | EQ |
| 38 | Type inheritance | single merge | single merge | EQ — **but see §5.3** |
| 39 | Rule search order | name → type → cgroup | same | EQ |
| 40 | `cgroup` name `..` | accepted | rejected | SUP |
| 41 | Nested cgroup names | cannot create | `create_dir_all` | SUP |
| 42 | Cgroup name matching | exact only | exact + path | SUP |
| 43 | `cgroup_rules` module | — | deleted (dead code) | EQ |
| 44 | `CPUQuota` write order | quota then period | same | EQ |
| 45 | `CPUQuota` CPU count | `hardware_concurrency` | `available_parallelism` | DIFF |
| 46 | `CPUQuota` source | same | same | EQ |
| 47 | `CPUWeight` | absent | `CgroupSettings` | SUP |
| 48 | `sched: deadline` fallback | n/a | `warn!` | SUP |
| 49 | Realtime detection | `sched_getattr.sched_priority > 0` | same | EQ |
| 50 | Unknown `ioclass` | logged, rule continues | `Skipped`, rule continues | EQ |
| 51 | `ioclass: "none"` | no write | no write | EQ |
| 52 | `latency_nice` fallback | falls back to `nice` | same | EQ |
| 53 | `latnice` support probe | on load | on load and reload | SUP |
| 54 | Non-sandboxable errno | `test_errno` → −1 → treated as success | partial failure, rest of the rule applied | EQ — **see §5.4** |
| 55 | `set_oom_score_adjust` result | unchecked | checked | SUP |
| 56 | `set_latnice` EINVAL | errno cleared, reported as applied | error | DIFF |
| 57 | `nice` → `cpu.weight` | absent | gated by `apply_cpu_weight` | SUP |
| 58 | `cpuset` strictness | no whitespace | whitespace tolerated | DIFF |
| 59 | `cpuset` alias set | 11 | 12 (adds `all`) | SUP |
| 60 | Empty alias means skip | yes | yes | EQ |
| 61 | `cpuset` upper bound | `max(_SC_NPROCESSORS_CONF, 1024)` | `cpuset().len()` | DIFF |
| 62 | `.bpf.c` program | — | byte-identical | EQ |
| 63 | Perf buffer pages | 64 | 64 | EQ |
| 64 | BPF `min_us` | inert (commented out) | inert | EQ |
| 65 | Lost-event callback | stderr | stderr | EQ |
| 66 | `move_pid` start-time guard | absent | present | SUP |
| 67 | `move_pid` TGID resolution | absent | present | SUP |
| 68 | `dump proc` fields | same fields | same + `rule` | DIFF — **see §5.2, §7.2**; `autogroup` now works |
| 69 | `dump` JSON ordering | `unordered_map` | sorted | SUP |
| 70 | `dump rules/types/cgroups` payload | raw / merged | same | EQ |
| 71 | Event-source debug tools | opt-in samples only | not provided | EQ |
| 72 | `libbpf` print callback | n/a | wired to `--verbose` | SUP |
| 73 | `sched_getscheduler` | not used | removed | EQ |
| 74 | 1.3× big.LITTLE threshold | `f64` compare | `f64` compare | EQ — brute-forced over 400 000 pairs |
| 75 | Homogeneous → `little-cores` | `""` | `""` | EQ |
| 76 | Capacity source selection | stops at first v1 mount | prefers cgroup2 always | DIFF — **see §5.5** |
| 77 | Offline CPU in capacity scan | counts as differentiating | absent | DIFF |
| 78 | `llc-N` id order | ascending CPU | ascending CPU | EQ — **see §5.7** |
| 79 | `parse_size_string` | K/M/G | K/M/G, shared | EQ |
| 80 | `get_node_id` on bad input | `std::terminate` | 0 | SUP |
| 81 | X3D single-CCD alias | `0-(N-1)` always | all enumerated cores | EQ — the `die_id`→`cluster_id` fallback still differs, **see §5.13** |
| 82 | X3D write timing | before dispatch | after root + singleton | DIFF |
| 83 | X3D restore on exit | on listener failure | on every exit path | SUP |
| 84 | `init_cgroups()` | called | called, and retried | SUP |
| 85 | cgroup detection cache | `static optional` | `RwLock<Option<…>>` | SUP |
| 86 | Cgroup re-detection reachable | n/a | yes | SUP |
| 87 | `sd_pid_get_cgroup` | used under systemd | not used | DIFF |
| 88 | `create_cgroup` idempotence | yes | yes | EQ |
| 89 | `cgroup.subtree_control` | `+cpu` | `+cpu` | EQ |
| 90 | v1 vs v2 detection | first match wins | prefers cgroup2 | DIFF — **see §5.5** |
| 91 | `cgroup.procs` vs `tasks` | correct per version | same | EQ |
| 92 | Realtime cgroup target | hierarchy root | hierarchy root | EQ |
| 93 | `exe` failure heuristic | global counter, latching | per-PID LRU | DIFF — **see §5.10** |
| 94 | ` (deleted)` suffix | truncated to `"name "` | truncated to `"name"` | DIFF |
| 95 | Name decoding | raw bytes | lossy UTF-8 | DIFF |
| 96 | `cmdline` newlines | truncated at first `\n` | kept whole | DIFF |
| 97 | Kernel-thread detection | computed, unused | absent | EQ |
| 98 | Rule LRU cache | none | present | SUP |
| 99 | LRU capacity | — | 5000 | SUP |
| 100 | `--reload` semantics | merge into the live map | fresh default snapshot | DIFF |
| 101 | `--reload` creates a missing config | no | yes | DIFF |
| 102 | No cgroup hierarchy | starts anyway | starts anyway | EQ — **see §7.1** |
| 103 | `check_disks_schedulers` | absent | restored | SUP |
| 104 | Panic backtrace | custom handler | panic hook | EQ |
| 105 | Fuzz targets | 3 | 3 | EQ |
| 106 | `SIGUSR1` | absent | reload | SUP |
| 107 | `sd_notify(Ready)` | subprocess | crate | EQ |
| 108 | `Delegate=yes` in the unit | absent | present | SUP |
| 109 | RPM packaging | present | absent | SUB |
| 110 | `flake.nix` | absent | present | SUP |
| 111 | `crt-static` profile | — | not provided | SUB |
| 112 | `.foo-wrapped` config support | absent | present | SUP |
| 113 | `apply_cpu_weight` | absent | gates the mirror | SUP |
| 114 | Netlink `prev_pid` reset | per `listen()` | per `listen()` | DIFF |
| 115 | BPF test coverage | none | none | EQ |
| 116 | `netlink recv` timeout | 500 ms `SO_RCVTIMEO` | none | DIFF |
| 117 | `netlink` `SO_RCVBUF` | default | default | EQ |
| 118 | `monitor::restore_x3d` tested | n/a | no | EQ |
| 119 | Netlink ENOBUFS recovery | n/a | untested | EQ |
| 120 | `panic = "abort"` profile | — | release only | EQ |
| 121 | `deadline` scheduler availability | n/a | falls back with a warning | SUP |
| 122 | `-nice` overflow | no counterpart | saturates at the clamp | EQ — **see §5.14** |
| 123 | `dump autogroup` | populated | populated | EQ — **see §7.2** |

---

## 5. Behavioural difference register

What differs between the two daemons **now**. Severity is what an operator observes, not how much
code differs.

Every finding this register was built from has been dealt with: six are fixed, twelve are
deliberate and documented, and one (5.17) turned out on verification to describe no difference at
all and has moved to §8. The entries below state the current divergence rather than narrating how it
was found, and the reasoning for each fix lives in the commit it names and in §10.

Two things bound the whole register. `ananicy-cpp` has never been executed here (§9), so every claim
about its behaviour is source reading — though the control flow behind each entry was read end to
end rather than sampled. And where this daemon's answer is the better one, the entry says so and
points at `ANANICY_CPP_DIFFERENCES.md` instead of describing a bug: parity is not the goal, and
reproducing a defect in the reference to achieve it would cost a working rule.

### Closed

| § | What it was | Now | Commit |
|---|---|---|---|
| 5.4 | One attribute's failure aborted the rule, so a rejected `sched` cost a valid `ionice` | The rest of the rule applies; a total failure logs at `error`, a partial one at `warn` | `c0086f7` |
| 5.7 | `llc-N` was numbered in `read_dir` order, so a rule could pin to a different LLC than under the reference | Numbered by ascending CPU id, as the reference's own walk numbers them | `d0860d9` |
| 5.9 | `--force-remove-semaphore` exited 0 whether or not anything was removed | A failed unlink logs the errno and exits 1, so a cleanup script can trust the status | `c835fc5` |
| 5.13 | An X3D single-CCD part needed a readable L3, so one whose cache size is hidden got no alias at all | Decided on the die count, as the reference does; both aliases exist regardless | `8f36ec9` |
| 5.14 | A `nice` at the edge of `i64` overflowed the `cpu.weight` mirror's exponent | `powf` on an `f64`, which saturates where the clamp already does | `697e0b4` |
| 5.15 | Skipping a realtime process' cgroup, or a `cpuset` alias that resolved empty, was recorded as a partial failure and warned on every affected process | Both are decisions, not failures; neither warns, and the applied-rule line is no longer suppressed | `4981a3b` |

Partly closed, and still listed below because something remains: 5.1 and 5.2 (one sub-item each) and
5.6 (the zero-interval case).

### Still different

#### 5.1 `--verbose` and log output — Severity: Informational

`--verbose` is one step more verbose than the configured `loglevel`, clamped at `trace`, matching
the reference's `max(0, level - 1)` (`febceac`; `ANANICY_CPP_DIFFERENCES.md` §6).

What remains: at debug verbosity the reference prints the matched rule *instead of* the
applied-rule line (`worker.cpp:92-96`), so with `log_applied_rule = true` the two emit a different
number of lines for one rule. This daemon prints both. Documented in
`ANANICY_CPP_DIFFERENCES.md` §5.2 — not matched, because both lines is the more useful answer and
nothing parses the log.

#### 5.2 `dump proc` field values — Severity: Low

`cmd` is the name the rule engine matched on and `cmdline` is an array of arguments, both matching
the reference (`8caebff`). The shape of both dumps is documented in `CLI.md` and pinned by
`tests/cli.rs`.

What remains: `oom_score_adj` is signed here, so a process at `-900` is reported as `-900`. The
reference reads the same file into an `unsigned` and reports `4294966396`. Diagnostic output that
nothing computes on, so the wrap is not reproduced. Documented in `ANANICY_CPP_DIFFERENCES.md` §5.1
and in `CLI.md`, where the value appears.

#### 5.3 An explicit `null` in a rule deletes the inherited value here — Severity: Medium

The reference merges a type into a rule **twice** (`rules.cpp:198-206`); this daemon merges once
(`crates/ananicy-core/src/rules.rs:82-87`). With a type carrying `nice: 5` and a rule carrying
`"nice": null`:

* `type_rule.merge_patch(rule)` deletes the type's `nice`, because a null patch value removes the
  key from the *target*.
* `rule.merge_patch(type_rule)` restores nothing — `type_rule` no longer has `nice` — and `rule`
  still holds its own `nice: null`, because step 1 removed the null from the *other* object.

So the reference's finished rule carries `nice: null`, where this one's carries no `nice` at all. It
then throws: `const int &rule_nice = rule["nice"]` (`worker.cpp:100`) cannot convert a JSON null, and
the catch-all (`worker.cpp:203-206`) logs `critical: unhandled exception` and applies **nothing**
from that rule — including its valid `sched`.

One merge is what merge-patch means, and reproducing the double merge would mean reproducing a throw
that silently discards a whole rule. Verified by execution here; the reference's behaviour is traced
by hand. `ANANICY_CPP_DIFFERENCES.md` §5.1.

#### 5.5 Cgroup v1/v2 classification — Severity: Medium

The reference stops at the first cgroup mount it recognises (`cgroups.cpp:300-305, 321-323`) and
abandons detection entirely if that mount is a v1 controller with no `cpu` sibling. This daemon
keeps scanning (`mounts.rs:84-92`) and a later `cgroup2` mount always wins.

Two consequences: a container exposing a single v1 controller mount with no `cpu` sibling works here
and silently loses every `cgroup` rule in the reference; and on a **hybrid** host the two select
different hierarchies, so a rule's cgroup name resolves to a different directory under each and
running one after the other leaves the other's cgroups behind.

The first is the reference losing a capability and is not reproduced. The second has no obviously
correct answer — the reference's rule is "whichever mount came first", which is arbitrary — so this
is a real open question rather than a defect on either side. Not demonstrable here: the audit host
is cgroup2-only. `ANANICY_CPP_DIFFERENCES.md` §5.1.

#### 5.6 `check_freq` parsing — Severity: Low

`check_freq=0` is refused at parse time with an error naming the reason (`78de5c6`). The reference
accepts it and then waits zero seconds, so `--manual-scanning` performs a full `/proc` walk in a
tight loop — a core burner that looks like a working daemon. That is the reference's bug and it is
not reproduced.

What remains: the reference parses with `std::stoul`, which stops at the first bad character,
accepts a sign and narrows to `uint32_t`, so `-5`, `0x10` and `4294967296` become three different
numbers there and three errors here. Rejecting nonsense is the better answer and is still a
divergence. `ANANICY_CPP_DIFFERENCES.md` §5.2.

#### 5.8 Core classification averages — Severity: Medium

The reference computes the core-type threshold as an **integer** mean and compares with an integer
`>=` (`topology.cpp:95`, `:117`); this daemon uses `f64` (`topology.rs:431`, `:437`).

They agree everywhere except where a capacity tier lands exactly on the truncated mean, and there
the truncation is what makes the reference wrong: for two CPUs of capacity `{1, 2}` its average is
1, so `1 >= 1` classifies the capacity-1 core as **big** and leaves `little-cores` empty — when it
is plainly the little one. This daemon's threshold is 1.5 and puts it where it belongs. The 1.3×
heterogeneity test is unaffected: it was brute-forced over 400 000 integer pairs and the two do not
diverge there. `ANANICY_CPP_DIFFERENCES.md` §5.1.

#### 5.10 `exe` readlink failure heuristic — Severity: Informational

The reference keeps one **global, latching** counter (`static exe_fail_count`, threshold 5, reset
only on success — `process.cpp:195-196, 224, 236, 243-244`), so five `EACCES` on `/proc/*/exe` for
*any* processes permanently disables exe-based naming for the whole daemon. This daemon counts per
PID in a bounded LRU (`procfs.rs:15-17, 83-108`), so it takes five failures of the *same* process.

The per-PID count is the correct behaviour and the reference is affected by a real bug. The two can
still resolve different names for one process, and therefore match different rules — in the
reference's favour only where its bug has already fired. `ANANICY_CPP_DIFFERENCES.md` §5.1.

#### 5.11 A deleted binary resolves to a different name — Severity: Low

The kernel appends ` (deleted)` to an `exe` readlink whose target has been unlinked. The reference
strips the filename with `substr(0, exe_name_end + 1)` (`process.cpp:230-233`), and the `+ 1` keeps
the space belonging to the marker, so `/usr/bin/foo (deleted)` resolves to `"foo "` and a rule
written as `{"name": "foo"}` does not match. This daemon truncates at the index and gets `"foo"`.

Reached by an ordinary package upgrade, where the new file is unlinked and recreated under a running
process. **Not** by a NixOS store GC, which roots `/proc/<pid>/exe` and so cannot delete a binary
that is executing — verified, and worth recording because the reverse is the natural assumption. The
reference is the one that fails to match. `ANANICY_CPP_DIFFERENCES.md` §5.1, and pinned by
`procfs.rs`'s unit tests, which fail with the reference's `+ 1` restored.

#### 5.12 An unreadable CPU is "no data" — Severity: Medium

The reference's "does this source differentiate the CPUs" test reads 0 for a CPU it cannot read, and
`0 != reference` therefore counts as differentiating (`topology.cpp:53-64`), so one offline CPU
makes it adopt a higher-priority capacity source and classify the machine by it. This daemon treats
0 as absent and keeps looking (`topology.rs:212-216, 238-243`), computing the split over the CPUs
that answered.

So on a machine with an offline CPU the `big-cores`/`little-cores`/`turbo-cores` aliases can differ.
Treating "no reading" as a distinct value is not something to reproduce.
`ANANICY_CPP_DIFFERENCES.md` §5.1.

#### 5.16 `cpuset` whitespace — Severity: Low

`CpuSet::parse` trims each token (`cpuset.rs:55`, `:62`), so `"0, 1"` and `" 5"` are accepted. The
reference tests the raw token for non-digits (`cpuset.cpp:262-267`) and rejects both. A rule written
with a space after the comma therefore applies here and is ignored there.

Being more permissive is the better answer, and the stricter rejections both daemons share
(`0-a`, `1-2x`, a leading `-`, `,,`, a leading comma) are unchanged. `ANANICY_CPP_DIFFERENCES.md`
§5.2.

#### 5.18 Netlink receive window — Severity: Informational

The two daemons set *different* socket options, and the earlier version of this entry claimed this
one set neither, which was false and contradicted `ANANICY_CPP_DIFFERENCES.md` in this repository.

| | `SO_RCVBUF` | `SO_RCVTIMEO` | how it drains |
|---|---|---|---|
| `ananicy-cpp` | default | 500 ms (`netlink_program_utils.c:45-49`) | a blocking `recv` that times out |
| `ananicy-rs` | 8 MiB, warning rather than failing if refused (`netlink.rs:48-56`) | not set | non-blocking, `epoll` with a 100 ms tick (`netlink.rs:93-115`) |

`SO_RCVBUF` is what the kernel overruns when a burst of `fork`/`exec` events outruns the reader, and
the failure mode is `ENOBUFS` and lost events; the reference's 500 ms `SO_RCVTIMEO` bounds how long a
`recv` may block and does nothing about the buffer. The cost here is a slightly more active loop,
which is the trade `ANANICY_CPP_DIFFERENCES.md` §5 already described.

The `prev_pid` reset difference in the same path is real and documented there.

#### 5.19 `get_cgroup_for_pid` does not use `sd_pid_get_cgroup` — Severity: Informational

The reference calls `sd_pid_get_cgroup` under `ENABLE_SYSTEMD` (`cgroups.cpp:336-346`); this daemon
always reads `/proc/<pid>/cgroup` (`debug.rs:84`).

Narrower than it looks, and the earlier entry did not say so: **`get_cgroup_for_pid` is not on the
rule-application path in either daemon.** Every caller in the reference is a test
(`unit-core.cpp:104,111,115`) or the `debug cgroups` diagnostic (`debug.cpp:52`). Nothing that
matches a rule, moves a process, or writes a cgroup setting goes through it, so this affects one
diagnostic line and no behaviour.

The two answers differ inside a cgroup namespace: `sd_pid_get_cgroup` resolves the path as the
*systemd* manager sees it, `/proc/<pid>/cgroup` is namespace-relative. That is a real reason for the
reference to prefer it, and not one this daemon can act on without a systemd dependency it does not
otherwise have — and what it reads is the answer correct *from inside* the namespace the daemon is
running in, which is the one that matters when deciding where to move a process. The reference also
ignores the call's return value, so on failure it reads an uninitialised `char *`
(`cgroups.cpp:336-346`). `ANANICY_CPP_DIFFERENCES.md` §5.


## 6. Rust-only capabilities with a concrete effect

Confirmed still present and still superset, with the caveat from §3.4 where applicable.

| # | Capability | Where |
|---|---|---|
| 1 | 5000-entry LRU rule cache | `crates/ananicy-core/src/rules.rs:33, 183-199` |
| 2 | `CPUWeight` from a `.cgroups` rule | `src/runtime.rs:160-179` |
| 3 | `apply_cpu_weight` gate on the `nice` mirror | `crates/ananicy-core/src/worker.rs:313-332` |
| 4 | `check_disks_schedulers` start-up diagnostic | `src/disks.rs` |
| 5 | `panic = "abort"` in the release profile | `Cargo.toml:81-87` |
| 6 | Panic hook: message, thread, forced backtrace | `src/panics.rs` |
| 7 | `move_pid` start-time guard, TGID resolution | `crates/ananicy-platform/src/cgroup/process.rs` |
| 8 | Nested cgroup creation via `create_dir_all` | `cgroup/manager.rs:67-117` |
| 9 | `cgroup` name `..` rejection | `crates/ananicy-core/src/rules.rs:141-174` |
| 10 | Cgroup path matching, not just name | `cgroup/manager.rs:178-261` |
| 11 | Cgroup re-detection reaching the manager | `crates/ananicy-platform/src/cgroups.rs:40-50` |
| 12 | Bounded wait for a cgroup hierarchy at start-up | `src/runtime.rs:137-153` |
| 13 | `SIGUSR1` config reload | `src/signals.rs:22-24` |
| 14 | Log-level reload without a restart | `src/runtime.rs:183` |
| 15 | `.foo-wrapped` config resolution | `src/startup.rs:76-88` |
| 16 | Sorted `dump` JSON | `src/dump.rs` |
| 17 | Deterministic rule-file order | `crates/ananicy-core/src/rules.rs:49-57` |
| 18 | Per-PID `exe`-failure LRU | `crates/ananicy-platform/src/procfs.rs:15-17` |
| 19 | `llc_map` populated for every topology | `crates/ananicy-platform/src/topology.rs:118-135` |
| 20 | Three fuzz targets | `crates/ananicy-platform/fuzz/fuzz_targets/` |
| 21 | `parse_size_string` shared and `G`-aware | `crates/ananicy-platform/src/topology.rs:138-155` |
| 22 | `get_node_id` returns 0 instead of terminating | `crates/ananicy-platform/src/topology.rs` |
| 23 | `OOMScoreAdjust` write result checked | `crates/ananicy-platform/src/priority.rs:197-210` |
| 24 | Unknown `ioclass` skips rather than aborting | `crates/ananicy-platform/src/priority.rs:112-118` |
| 25 | `ipc` mode 0600 | `src/ipc.rs:15, 52` |
| 26 | `Delegate=yes` in the packaged unit | `data/ananicy-rs.service.in:16` |
| 27 | NixOS module | `contrib/module.nix:141` |
| 28 | `flake.nix` | `flake.nix` |
| 29 | BPF `--verbose` plumbing into libbpf | `crates/ananicy-bpf/src/bpf_monitor.rs:46-60` |
| 30 | X3D write after root + singleton checks | `src/main.rs:144-146` |
| 31 | X3D restore on every exit path | `src/runtime.rs` |
| 32 | IP unlink failure is not fatal | `src/ipc.rs:25-29` |

---

## 7. Defects

The two that decided the verdict — both now fixed — and the Medium set that remains.

> **Both High findings in this section have been fixed since the document was rebuilt:** `cd77276`
> and `de694d4` respectively. The findings are kept as written, with what the fix was, because they
> are the two most instructive results of the pass and a future reader should be able to see why the
> tests exist.


### 7.1 With no cgroup hierarchy the daemon exits 0 having applied nothing — Severity: High — **fixed in `de694d4`**

`src/runtime.rs:82-84`:

```rust
info!("Initializing cgroups based on rules");
if !wait_for_cgroup_hierarchy() || !create_cgroups(&rules) {
    return;
}
```

`runtime::run` returns `()`, `main` falls off the end, and the process exits **0**. Nothing else has
happened by that point: the worker is spawned on the next statement, the netlink subscription is
made inside the monitor, and `restore_x3d` runs from the shutdown path that is never entered — so an
`x3d_mode` change made at `main.rs:146` is **left in place**.

The wait itself is `src/runtime.rs:137-153`: `has_cgroup_hierarchy()`, then one `init_cgroups()`
attempt, then a warning naming the timeout. So the observable behaviour on a host with no hierarchy
is: 10 seconds of silence, one warning, exit 0, nothing applied.

`ananicy-cpp` logs "Cgroups are not available on this platform" (`main.cpp:229-230`) and carries on,
applying every attribute except `cgroup`. This divergence was introduced by `80f7169`, which fixed
§9 item 2 of the previous audit — the retry loop existed but was never called — and made the
un-called case a successful no-op.

**Affects:** containers whose cgroup2 namespace does not delegate the `cpu` controller; a v1 host
whose first v1 mount has no `cpu` sibling; kernels without `CONFIG_CGROUP_CPU`.

**Reproduced** after the fact, in a private mount namespace with an empty tmpfs over
`/sys/fs/cgroup` — which is what `de694d4`'s test does. The daemon waited, warned, and exited 0
having spawned nothing; after the fix the same run goes on to spawn the worker and process 442
processes.

**What the fix did:** the wait now decides whether to create the rule cgroups and nothing else
(`runtime.rs:81-93`). `create_cgroups` returns unit rather than an always-true `bool`, because a
signature that returns `bool` invites exactly the branch that was there; the realtime workaround's
copy of that branch became a non-blocking `has_cgroup_hierarchy()` check, so it cannot become a
second silent exit either. Pinned by
`test_cli_daemon_still_runs_without_a_cgroup_hierarchy`, which fails without the change with "the
daemon exited instead of carrying on without cgroups".


### 7.2 `autogroup` is read from a path the kernel does not provide — Severity: High — **fixed in `cd77276`**

`crates/ananicy-platform/src/process_info.rs:72-74`:

```rust
let autogroup_val =
    read_to_string(format!("/proc/{}/task/{}/autogroup", pid, tpid)).unwrap_or_default();
```

The kernel exposes `autogroup` only on the thread-group leader, at `/proc/<pid>/autogroup`. There is
no `/proc/<pid>/task/<tid>/autogroup`. The reference reads the path that exists
(`process_info.cpp:200-203`).

**Verified by execution on this host:**

```
$ cat /proc/self/autogroup
/autogroup-29227 nice 0
$ ls /proc/self/task/*/autogroup
zsh:1: no matches found: /proc/self/task/*/autogroup

$ ananicy-rs dump autogroup
{}
$ ananicy-rs dump proc | grep -c '"autogroup": null'
2223
```

Every process on the machine reports `null`, and `dump autogroup` prints the empty object. The three
autogroup unit tests test the *parser* against synthetic strings, which is why this was never
caught: the path was never exercised.

**What the fix did:** an `autogroup_path()` helper reads `/proc/<pid>/autogroup` and names the
kernel fact in one place. The new test
`autogroup_is_read_from_a_path_the_kernel_actually_provides` builds a `ProcessInfo` for the test's
own pid — so the path is exercised, not just the parser — skips on a kernel without
`CONFIG_SCHED_AUTOGROUP`, and fails with the old path restored:

```
the kernel published "/proc/27488/autogroup" but the daemon reported no
autogroup for its own process: None
```

It also asserts the per-thread path does *not* exist, with a note to revisit if a kernel ever
provides one, since it would be a different file. After the fix, 1513 of 1786 processes on this host
report a real autogroup across 81 groups, and the 273 that remain `null` are exactly the kernel
threads — which have none, and which the reference also reports as `null`.


### 7.3 The Medium set, and what became of it

Three of §5's findings were rated Medium. All are now settled, and the ways they settled differ in a
way worth stating:

* **§5.4, an attribute failure aborting the rest of the rule** — **fixed** (`c0086f7`). This was the
  only one where the reference was right and this daemon was wrong, and it cost a valid `ionice`
  whenever a rule also named a `sched` the kernel would not take.
* **§5.3, an explicit `null` resurrecting a type's value** and **§5.8, an integer mean putting the
  capacity-1 core of a `{1, 2}` machine in the *big* tier** — **documented, not matched**. Both are
  the reference being wrong, and reproducing either would cost a working rule.
* **§5.5, §5.7 and §5.12** — **documented**. §5.7 and §5.12 are the reference misreading `read_dir`
  order and an unreadable capacity file; §5.5 is a hybrid host, where the reference's rule is
  "whichever mount came first" and there is no obviously correct answer on either side.

Each is in `docs/ANANICY_CPP_DIFFERENCES.md` §5.1. Nothing in this set is a known defect in this
daemon.

---

## 8. What was checked and found equivalent

Recorded so the coverage of this pass is auditable, and so a future pass does not re-derive it.


### 8.1 Areas found equivalent

**Configuration.** Booleans must be spelled exactly `true` in both; last duplicate wins in both; a
line with no `=`, a blank line and a `#` comment are dropped in both. Every key the reference reads
has a counterpart here with the same spelling and effective default. The `latnice` support probe and
its force-off-when-unsupported rule, including on reload. `x3d_mode` defaults and the
`cache`/`frequency` mapping.

**Rules and types.** Extension matching is case-sensitive and dotfile names skipped in both. First
`{` … last `}` span extraction, `#`-comment and CRLF tolerance. Classification precedence
name → type → cgroup, including "name present but not a string" falling through to `type`. Exact
beats regex; last duplicate wins. `name_regex` is PCRE2 with `PCRE2_UTF | PCRE2_UCP` in both — the
same engine, flags, unanchored search, and "compilation failure ⇒ no match". `CPUQuota` and
`CPUWeight` configure nothing for a non-number, a negative, a boolean or `null`. `dump
rules`/`types`/`cgroups` payloads are merged/raw/raw in both.

**Attributes.** Application order `nice → latency_nice → sched → ioclass/ionice → oom_score_adj →
cgroup → cpuset` is identical. `apply_*` gating on all seven, including `ioclass` being gated by
`apply_ionice` and never by `apply_ioclass`. The `sched` name table, the `rtprio` default of 1, and
"an unknown name aborts the rule". `ioclass: "none"` writes nothing; an unrecognised class is a skip.
`oom_score_adj` is written with no range clamping in either. The `cpuset` alias set, the
empty-alias-means-skip rule, and the rejection of `0-a`, `1-2x`, a leading `-`, `,,` and a leading
comma. Realtime detection is `sched_getattr.sched_priority > 0` on both, and the realtime process is
pushed at the hierarchy root on both.

**Process discovery.** The `cmdline → exe → comm` ladder and the leading-NUL skip. Both backends
report `fork.child_pid`/`exec.process_pid`/`comm.process_pid`, ignore `exit`, and de-duplicate
against the previous pid. BPF: byte-identical programs, inert `min_us`, 64 pages, stderr callback.
The full `/proc` scan, its digit filter, and PID 0 reaching the worker in the reference where it
matches no rule and being skipped here.

**Cgroups.** v1 path `<parent>/cpu/<name>`, v2 path `<delegated-root>/<name>`, both hard-coding the
`cpu` segment. `create_cgroup` idempotence, nested creation, the `cgroup.subtree_control` `+cpu`
write, and the v1 `tasks` vs v2 `cgroup.procs` choice. The v2 probe (create, require `cpu` in
`cgroup.controllers` and `cpu.max`, remove) and the v1 probe (`<parent>/cpu` exists).
`resolve_target_dir` treating a leading `/` as absolute from the mount point. The `cpu.max` /
`cpu.cfs_quota_us` constants and the `clamp(0,100)`.

**CLI and lifecycle.** `--help`, `--version`, no-arguments, the `dump`/`debug`/`start` positionals,
and every misuse case failing in both. `debug cgroups` output, `debug` forcing trace verbosity, the
version banner on stdout. SIGINT/SIGTERM shutdown, worker stop, singleton removal, `sd_notify` on
both, and the X3D restore on the shutdown path. The `dump`/`debug` actions running after rule load
and before the root check. The shutdown summary's fields and its `HH:MM:SS.ffffff` timestamp.

**Numerics.** The 1.3× threshold (brute-forced over 400 000 pairs, no divergence). `CPUQuota` u32
wrap-around, and `available_parallelism` vs `hardware_concurrency` (already recorded). Division by
zero: there is none on either side — every `/100` divisor is a literal and every `/ num_sharing` is
guarded by `if count > 0`.


### 8.2 Deliberate, documented divergences

`check_freq` is a float upstream and an integer here (§5.6 and the previous audit's §12.4);
`apply_ioclass` is inert in both; `CPUQuota` sets `cpu.shares` upstream and `CPUWeight` here;
`nice → cpu.weight` is Rust-only.


### 8.3 Candidates tested and discarded

Recorded because a discarded candidate is evidence of coverage, and because a wrong finding costs
more than a missing one: `float` vs `f64` in the 1.3× test (brute-forced, zero divergent pairs);
`check_freq` u32 overflow (both wrap identically); a JSON double-merge with an explicit `null` (my
first test was malformed; the corrected one shows both keep `null` — §5.3 is the *different* aspect,
the double merge); `"nice"` out of range (the kernel clamps rather than returning `EINVAL`, so the
`EINVAL` trigger is `rtprio`); `dump proc` including PID 0 (both exclude it, by different means).

---

## 9. Not verified

* **The C++ daemon was never executed.** No `cmake`, and the project fetches dependencies over the
  network. Every claim about its behaviour is source reading, with the control flow read end to end
  rather than sampled. The C++-side claims most worth re-checking when a build is possible are
  §5.3 and §5.4, and the C++ half of the matrix's "C++ has none" rows.
* **The BPF backend has no test coverage.** It now *builds* — `nix develop` supplies libbpf, and
  `nix develop --command cargo test --workspace` passes with it — so the `--verbose` plumbing into
  libbpf's print callback is at least compile-checked, which the first version of this document
  could not claim. §8.1 records the comparison; it remains source-level plus a `diff`, because
  exercising the path needs root and a BPF-capable kernel, and the crate has no tests of its own.
* **Nothing requiring a cgroup-v2 delegated-root service was exercised against a real delegated
  subtree.** The owned branch of the cgroup manager is covered only by the simulated hierarchy in
  `crates/ananicy-platform/tests/cgroup_manager.rs`; the root-gated tests in `tests/cgroups.rs` skip
  in this environment. This is why §5.5 is a source finding.
* **No big.LITTLE, hybrid, or multi-LLC host was available** (this one: 12 CPUs, 1 LLC, 1 NUMA node,
  homogeneous). §5.8 was established by exhaustive search over the two formulas, which proves a
  counter-example *exists* but not that it occurs on shipping hardware. §5.7 is now fixed and pinned
  by a test that does not need such a machine.
* **`ananicy-cpp` has no VCS history**, so "intentional vs accidental" rests on comments, tests and
  the differences document.

---

## 10. Remediation history

Every commit since `e131b18` is one finding: the twenty-six the previous audit
listed, then the two High findings of §7, then nine of §5. Kept so the fixes are traceable from the
findings they answer; the findings themselves are §5, §7 and `docs/ANANICY_CPP_DIFFERENCES.md` §5.
The count is deliberately not given, because this file would be the next commit and the number would
be wrong the moment anything else landed.

| Original §9 | Item | Outcome | Commit |
|---|---|---|---|
| 1 | `cgroup_realtime_workaround` inert | Fixed — the manager is behind an `RwLock` and no longer dropped with the cache | `274c1c7` |
| 2 | Startup cgroup retry never called | Fixed, then fixed again: the first fix made the never-called case a silent exit 0 (§7.1) | `80f7169`, `de694d4` |
| 3 | `nice` → `cpu.weight` blast radius | `apply_cpu_weight` added, defaulting to the previous behaviour | `6964fb9` |
| 4 | Capacity source chosen per CPU | One source for the machine | `b25eaba` |
| 5 | `little-cores` = all CPUs when homogeneous | Both paths go through `mark_homogeneous` | `998c620` |
| 6 | `ioclass: "none"` wrote class `none` | `ioprio_valid` guards the write | `4e76726` |
| 7 | Unknown `ioclass` aborted the rule | `Skipped` | `eb2ef68` |
| 8 | X3D written on diagnostic paths | Write moved after the root and singleton checks | `52d2337` |
| 9 | `--manualscanning` rejected | Both spellings parse | `332e5da` |
| 10 | `CPUWeight` documented, not implemented | Implemented via `CgroupSettings` | `b6ae3de` |
| 11 | `.expect()` on the netlink send | Failed send sets the shutdown flag | `b22da6b` |
| 12 | `apply_ioclass` parsed and ignored | **Fixed, then reverted** — the key gates a log line upstream; the C++ comparison could not see that | `f4e8305`, `0bd0df2` |
| 13 | Realtime detection by policy | Aligned to `sched_getattr.sched_priority > 0` | `474f7a9` |
| 14 | The rest undocumented | Documented in `ANANICY_CPP_DIFFERENCES.md` §5 | `6148529`, `a31b7ff` |
| 15 | NixOS `-wrapped` undocumented | Documented | `474f7a9` |
| 16 | Dead `cgroup_rules` module | Deleted | `9cf81b3` |
| 17 | No backtrace on a crash | Panic hook added | `4977fcb` |
| 18 | Two of three fuzz targets missing | `parse_cpuset` and `parse_rule` added | `78cf56c` |
| 19 | `dump` output untested | Fixed; the test found a stdout/stderr defect the audit had missed | `e0c44af`, `7a56a79` |
| 20 | Event-source debug tools | Documented as not provided — and the reason corrected: they are opt-in samples, not shipped (§3.2) | `6148529` |
| 21 | CLI and configuration reference errors | Fixed | `9f270d7` |
| 22 | Cache sizes read in two units | One shared `parse_size_string` | `57fe45f` |
| 23 | IPC name and permissions undocumented | Documented | `6148529` |
| 24 | `crt-static` release profile | **Not done** — a packaging decision, not a fix | — |
| 25 | `cargo test` needs libbpf | `ananicy-bpf` is a workspace member but not a default one | `8b49368` |
| 26 | `sched: deadline` fallback logged at debug | `warn!` | `74793b4` |
| — | Sandbox-sensitive deadline test | Made independent of the sandbox | `69e8c1c` |
| — | `runtime::run` signature | Refactored into `ProcessEvents`/`RunOptions` | `9190fbf` |
| — | `check_disks_schedulers` missing from both rewrites | Restored from the original | `3386d6d` |
| — | §7.2 `autogroup` read from a path the kernel lacks | Fixed, with a test that exercises the path | `cd77276` |
| — | §7.1 no hierarchy meant a silent exit 0 | Fixed; the daemon now carries on without cgroups | `de694d4` |
| — | §5.4 one attribute's failure cost the rule the rest | Fixed; a total failure still logs at `error` | `c0086f7` |
| — | §5.15 an intentional skip was reported as a failure | Fixed; the realtime cgroup skip and an empty `cpuset` alias no longer warn | `4981a3b` |
| — | §5.7 `llc-N` numbered in directory order | Fixed; numbered by ascending CPU id | `d0860d9` |
| — | §5.14 `-nice` could overflow the `cpu.weight` exponent | Fixed; `powf` on an `f64` saturates where the clamp does | `697e0b4` |
| — | §5.9 `--force-remove-semaphore` exited 0 on failure | Fixed; a failed unlink exits 1 | `c835fc5` |
| — | §5.6 `check_freq=0` accepted then silently replaced | Fixed; refused at parse time with a reason | `78de5c6` |
| — | §5.1 `--verbose` forced `DEBUG` | Fixed; one step more verbose, clamped at `trace` | `febceac` |
| — | §5.2 `dump proc`'s `cmd` and `cmdline` | Fixed; `cmd` is the matched name, `cmdline` an array | `8caebff` |
| — | §5.13 X3D single-CCD needed a readable L3 | Fixed; decided on the die count, as the reference does | `8f36ec9` |
| — | The other seven §5 differences | **Not fixed, deliberately** — matching the reference means reproducing a defect in it. Documented in `ANANICY_CPP_DIFFERENCES.md` §5.1 and §5.2 | `f7d988d` |


### 10.1 Three findings the previous audit got wrong

* **The perf buffer was never smaller.** The matrix and the first §5.17 said this side used
  libbpf-rs' default page count as if it were smaller than the reference's 64.
  `PerfBufferBuilder::new` already uses 64 — and §8.1 now says the sharper thing, that the
  count is inherited rather than chosen. No change was made.
* **Logs went to stdout.** The previous audit's §5.20 claimed this side "keeps stdout clean (JSON only) and logs the
  version at INFO on stderr — strictly better". It did not: `tracing_subscriber::fmt` defaults to
  stdout, so every log line was interleaved with the JSON and `dump cgroups | jq` failed. The claim
  was made from reading the code and not from running the pipe; the test added for item 19 caught
  it.
* **The C++ build's debug tools are opt-in** (§3.2).


### 10.2 Verification

* `cargo test` — **340 tests, 0 failures**, across 20 targets: 15 integration binaries, 3 unit-test
  targets and 2 doc-test targets. `nix develop --command cargo test --workspace` additionally builds
  and tests `ananicy-bpf`, which a plain `cargo test` cannot do without libbpf present.
* `cargo fmt --all -- --config imports_granularity=One,unstable_features=true --check` — clean.
* `cargo clippy --all-targets` — one warning, `LinuxPlatform` having no `Default`, which predates the
  audit and is unrelated to any finding.
* `nix-build -A packages.x86_64-linux.ananicy-rs` and `-A checks.x86_64-linux.ananicy-rs` — both
  build; the check runs 324 tests in the sandbox, one fewer than locally because a
  root-gated test skips there.
* The Nix source filters to tracked files, so a new module must be staged before `nix-build` will
  compile it.
* §7.2 was verified by execution against this host's `/proc`, and §7.1 was reproduced in a private mount namespace with an
  empty tmpfs over `/sys/fs/cgroup`.
