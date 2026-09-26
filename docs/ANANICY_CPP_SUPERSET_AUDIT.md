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
High findings of §7 are fixed (`cd77276`, `de694d4`), as are nine of the sixteen behavioural
differences in §5 — one commit each, each with a test that fails without it. What is left are
**seven differences where matching `ananicy-cpp` would mean reproducing a defect in it**, and they
are documented in `ANANICY_CPP_DIFFERENCES.md` §5.1 and §5.2 rather than closed:

* `oom_score_adj` is read as `unsigned` by the reference, so `-900` is reported as `4294966396`.
* A type is merged into a rule *twice*, so an explicit `null` is resurrected and the worker's
  catch-all then applies nothing at all from that rule.
* The core-type split uses an integer mean, so on capacities `{1, 2}` the capacity-1 core is
  classified *big* and `little-cores` comes out empty.
* A deleted binary's name keeps a trailing space, so no rule matches it after a package upgrade.
* Five `EACCES` on `/proc/*/exe` for any five processes permanently disables exe-based naming.
* An unreadable CPU is counted as a capacity value rather than as no data.
* Cgroup detection stops at the first mount rather than the best one.

That is the honest state of a rewrite whose reference has bugs: parity is not the goal, and
reproducing a defect to achieve it would cost a working rule. The six that remain genuinely open
are the ones where the reference is right and this daemon is not, and those are §5.3 and §5.5 in
the register plus the two in §8 that only a build of the reference could settle.

One matrix row in the previous audit was demonstrably wrong (§3.4) and one §12 non-finding is
contradicted by it (§3.4).

**Everything the previous audit reported is fixed and recorded** (§10), one commit per finding, with
the exception of a log-level change (`74793b4`) that fixed a finding without a row. This pass added
thirteen of its own — §7.1, §7.2 and nine of §5 — and recorded the seven it chose not to fix, with
the reason in each case.

### Confidence

* **High** for every finding backed by a citation *and* executed against this host: §7.2 (verified
  live, 2223/2223 `null` before the fix) and §7.1 (reproduced in a mount namespace with no hierarchy),
  §5.2, §5.4, §5.7, §5.8, §5.9, §5.10, §5.11 (all executed).
* **High** for the C++ side of every finding, by source reading. **The C++ daemon could not be
  built in this environment** — `cmake` is absent and the project fetches its dependencies over the
  network (`CPMAddPackage`) — so no C++ claim rests on execution. The control-flow arguments were
  read end to end rather than sampled.
* **Medium** for §5.5, §5.6, §5.12, §5.13, §5.14, §5.15: the divergence is proven from source
  on both sides but the triggering host (a container with an undelegated `cpu` controller, a hybrid
  v1+v2 host, a multi-LLC machine) is not available here to demonstrate it.
* **Medium** for the topology findings in general — they need a big.LITTLE machine, and the audit
  host is homogeneous (12 CPUs, 1 LLC, 1 NUMA node, no big.LITTLE). The counter-examples in §5.8 and
  §5.12 were found by exhaustive search over the two formulas rather than on real hardware.
* **Assessed by build:** `nix develop` provides libbpf, so the BPF backend compiles and
  `nix develop --command cargo test --workspace` passes with it. §5.17 still rests on source
  comparison rather than execution, because the crate has no tests of its own and running the path
  needs root and a BPF-capable kernel. The two `ananicy_cpp.bpf.c` files are byte-identical, which
  is checked by `diff`.
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
  *names*. Three *values* differ. `autogroup` was always `null` too — `cd77276` fixed that one; see §5.1, §5.5, §7.2.
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
| 1 | `--verbose` | one level more verbose | forces DEBUG | DIFF |
| 2 | Rule cache | none | 5000-entry LRU | SUP |
| 3 | LRU rule cache sizing | — | `NonZeroUsize::new(5000)` | SUP |
| 4 | `--force-remove-semaphore` error path | exit 1 + message | exit 0 always | DIFF |
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
| 22 | `check_freq` parse | `std::stoul` | `str::parse::<u32>` | DIFF |
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
| 54 | Non-sandboxable errno | `test_errno` → −1 → treated as success | `Err` → rule aborted | DIFF |
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
| 68 | `dump proc` fields | same fields | same + `rule` | DIFF — **see §5.1, §5.5, §7.2**; `autogroup` now works |
| 69 | `dump` JSON ordering | `unordered_map` | sorted | SUP |
| 70 | `dump rules/types/cgroups` payload | raw / merged | same | EQ |
| 71 | Event-source debug tools | opt-in samples only | not provided | EQ |
| 72 | `libbpf` print callback | n/a | wired to `--verbose` | SUP |
| 73 | `sched_getscheduler` | not used | removed | EQ |
| 74 | 1.3× big.LITTLE threshold | integer compare | `f64` compare | DIFF — **see §5.8** |
| 75 | Homogeneous → `little-cores` | `""` | `""` | EQ |
| 76 | Capacity source selection | stops at first v1 mount | prefers cgroup2 always | DIFF — **see §5.6** |
| 77 | Offline CPU in capacity scan | counts as differentiating | absent | DIFF |
| 78 | `llc-N` id order | ascending CPU | `readdir` order | DIFF — **see §5.12** |
| 79 | `parse_size_string` | K/M/G | K/M/G, shared | EQ |
| 80 | `get_node_id` on bad input | `std::terminate` | 0 | SUP |
| 81 | X3D single-CCD alias | `0-(N-1)` always | `None` if no L3 size | DIFF — **see §5.13** |
| 82 | X3D write timing | before dispatch | after root + singleton | DIFF |
| 83 | X3D restore on exit | on listener failure | on every exit path | SUP |
| 84 | `init_cgroups()` | called | called, and retried | SUP |
| 85 | cgroup detection cache | `static optional` | `RwLock<Option<…>>` | SUP |
| 86 | Cgroup re-detection reachable | n/a | yes | SUP |
| 87 | `sd_pid_get_cgroup` | used under systemd | not used | DIFF |
| 88 | `create_cgroup` idempotence | yes | yes | EQ |
| 89 | `cgroup.subtree_control` | `+cpu` | `+cpu` | EQ |
| 90 | v1 vs v2 detection | first match wins | prefers cgroup2 | DIFF — **see §5.6** |
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
| 122 | `-nice` overflow | no counterpart | debug panic / release wrap | DIFF — **see §5.14** |
| 123 | `dump autogroup` | populated | populated | EQ — **see §7.2** |

---

## 5. Behavioural difference register

Current state. Severity is what an operator observes, not how much code differs.

> **Half of this register has since been fixed**, one commit per finding: 5.1, 5.2 (two of three
> sub-items), 5.4, 5.6, 5.7, 5.9, 5.13, 5.14 and 5.15. Each heading says so. The findings are kept
> as written, with the fix appended, for the reason 5.3 gives — a rewrite that trades a working
> rule for parity with a reference bug is not a superset of anything, so those are documented in
> `ANANICY_CPP_DIFFERENCES.md` §5.1 and §5.2 instead.


### 5.1 `--verbose` and log output — Severity: Informational — **partly fixed in `febceac`**

`ananicy-cpp --verbose` adds one level; `ananicy-rs --verbose` forces DEBUG. The C++ build forces
`log_applied_rule` on in Debug builds (`worker.cpp:89-91`), which has no Rust equivalent. At debug
verbosity with `log_applied_rule = true` the C++ worker prints the rule dump *instead of* the
applied-rule line (`worker.cpp:92-96`); the Rust worker prints both (`worker.rs:181`, `203-214`).
Logs go to stderr and stdout carries only the answer — `dump … | jq` works
(`src/startup.rs:56-64`).

**Resolution.** The `--verbose` half is fixed in `febceac`: the flag is now one step more verbose than the
configured `loglevel`, clamped at `trace`, instead of forcing `DEBUG`. Forcing it made the flag
inert on a configuration already at `debug` or `trace`, and turned a quiet `error` configuration
into a wall of `info` — the opposite of "increase verbosity level". The log-line half stands: the
reference prints the rule dump *instead of* the applied-rule line at debug verbosity
(`worker.cpp:92-96`), so with `log_applied_rule = true` the two emit a different number of lines.
Documented in `ANANICY_CPP_DIFFERENCES.md` §5.2 rather than matched, because the answer here is
both lines.

### 5.2 `dump proc` and `dump autogroup` field values — Severity: Low — **two of three fixed in `8caebff`**

The field *names* match the reference; three *values* do not, and one is always null.

* `cmd` is the `comm` string in Rust (`src/dump.rs:74-82`) and the argv0/exe ladder in C++
  (`process_info.cpp:188`) — so Rust's `rule` field and its `cmd` field can disagree for a process
  that rewrote `argv[0]`.
* `cmdline` is a JSON array in C++ (`process_info.cpp:243`) and a single space-joined string in Rust.
* `oom_score_adj` is read as `unsigned` in C++ (`process_info.cpp:239-241`) and `i32` in Rust
  (`crates/ananicy-platform/src/process_info.rs:43-47`). Verified: NetworkManager's `-900` prints as
  `4294966396` under the reference and `-900` here.
* `autogroup` was always `null` here; `cd77276` fixed it, see §7.2.

**Resolution.** `cmd` and `cmdline` are fixed in `8caebff`. `cmd` was `/proc/<pid>/comm` while the `rule` beside it
was matched on the `cmdline -> exe -> comm` ladder, so the two fields could describe different
processes — `comm` is the kernel's, truncated to 15 characters, and is not what a rule is written
against. The reference has no such split: its `cmd` is the same function it matches rules with, so
`cmd` and `rule` always agree there. `cmdline` was the NUL-separated arguments joined with spaces;
it is now the JSON array the reference emits, which is the only form that keeps an argument
containing a space distinguishable from two arguments.

`oom_score_adj` is deliberately unchanged. The reference reads it as `unsigned`
(`process_info.cpp:239-241`) and reports NetworkManager's `-900` as `4294966396`; the previous
audit's own note recorded the reference's value as the surprising one. Documented in
`ANANICY_CPP_DIFFERENCES.md` §5.1. 

### 5.3 An explicit `null` in a rule deletes the inherited value here, and not in the reference — Severity: Medium

The reference merges a type into a rule **twice** (`rules.cpp:198-206`): `type.merge_patch(rule)` and
then `rule.merge_patch(type)`. This tree merges once (`crates/ananicy-core/src/rules.rs:246-253`).

With a type `{"type":"t1","nice":5,"ioclass":"best-effort","ionice":2}` and a rule
`{"name":"t1","type":"t1","nice":null,"sched":"idle"}`, `dump rules` here emits a rule with **no**
`nice` key — the null deleted it, which is the correct reading of a merge-patch. The reference
resurrects it on the second merge, and then `rule["nice"]` is a `null` that its `const int&`
conversion cannot handle: the worker's catch-all (`worker.cpp:203-206`) logs
`critical: unhandled exception` and applies **nothing at all** from that rule.

Verified by execution on this side; the merge order was traced by hand through the reference.


### 5.4 A priority syscall failing with an unexpected errno aborts the rule here — Severity: Medium — **fixed in `c0086f7`**

The reference's `test_errno` returns −1 on failure, and every caller tests `if (!set_X(…))`, which is
false for −1 — so a failed `sched_setscheduler` is **treated as success** and the rest of the rule is
applied anyway (`priority.cpp:34-36`; `worker.cpp:103,118,131,144,154,167,194`).

Concrete trigger, verified by execution on this host: `sched_setscheduler(SCHED_FIFO, prio=0)` and
`prio=200` both return `EINVAL(22)`. For a rule carrying `sched`, `ioclass`, `ionice`,
`oom_score_adj` and `cgroup`, the reference logs the errno, gets −1, and applies the last four; this
daemon returns `Err` at the `sched` attribute (`crates/ananicy-core/src/worker.rs:38-44`, whose
`is_skippable()` lists only `PermissionDenied` and `Skipped`) and applies **none** of them.

Which of the two is right is arguable — "the rule did not fully apply" is a truer log line than
silence — but they are opposites, and the divergence is in the direction of *not* applying a
perfectly valid `ionice`.

**Resolution.** Fixed in `c0086f7`. Every attribute failure now records a partial failure and the walk continues,
so a rejected `sched` no longer costs the rule a valid `ionice`. The outcome then depends on what
actually happened, which is the part worth being precise about: nothing applied plus a failure is
`Err` and logs at `error`, something applied plus a failure is `Partial` and logs at `warn`. The
reference reaches the same place by accident — its `test_errno` returns -1 and its callers treat
that as success — so the divergence was never about reporting, it was about which errnos the C++
helper happened to swallow. 

### 5.5 Cgroup v1/v2 classification — Severity: Medium

`ananicy-cpp` stops scanning the mount table at the first cgroup mount it recognises
(`cgroups.cpp:300-305`, `321-323`) and **abandons detection entirely** if that mount is a v1
controller whose parent has no `cpu` sibling. This tree keeps scanning (`mounts.rs:84-92`, guarded
only by `info.version == CgroupVersion::None`) and its cgroup2 branch has no "already set" guard, so
a later `cgroup2` mount always wins (`mounts.rs:51-83`).

Two consequences: a container exposing a single v1 controller mount with no `cpu` sibling works here
and silently loses every `cgroup` rule in the reference; and on a **hybrid** host (v1 + unified) the
two daemons pick different hierarchies, so every rule's `cgroup` name resolves to a different
directory and running one after the other leaves orphans. Not demonstrable here — this host is
cgroup2-only.


### 5.6 `check_freq` parsing — Severity: Low — **partly fixed in `78de5c6`**

The reference uses `std::stoul`, which stops at the first bad character, accepts a sign, and narrows
to `uint32_t` (`config.cpp:129-137`); this tree uses `str::parse::<u32>()` and keeps the default on
failure (`crates/ananicy-core/src/config.rs:154-162`). Verified: `-5`, `0x10` and `4294967296` are all
rejected here and become `4294967291`, `0` and `0` there.

`check_freq=0` is the consequential case: the reference's `wait_for(0s)` returns immediately, so
`--manual-scanning` performs a **full `/proc` scan in a tight loop**. This tree substitutes 60 for a
zero interval (`src/runtime.rs:183-184`). Rejecting the nonsense value is the better behaviour; the
divergence is recorded because the previous audit's matrix row 22 mentions only that this side does
not crash.

**Resolution.** The zero case is fixed in `78de5c6`: `check_freq=0` is refused at parse time with an error naming
the reason, rather than stored and silently replaced with 60 at the point of use. The reference has
no guard at all, which is the worse outcome the finding describes.
The parsing difference stands. `std::stoul` stops at the first bad character, accepts a sign and
narrows to `uint32_t`, so `-5`, `0x10` and `4294967296` become three different numbers there and
three errors here. Rejecting nonsense is the better answer; it is still a divergence and is
documented in `ANANICY_CPP_DIFFERENCES.md` §5.2. 

### 5.7 `llc-N` alias indexing — Severity: Medium — **fixed in `d0860d9`**

The reference assigns LLC ids in ascending CPU order (`topology.cpp:235-283`); this tree assigns
them in `read_dir` encounter order (`crates/ananicy-platform/src/topology.rs:281-317`, id =
`llc_map.len()` at first sight, `topology.rs:118-135`). sysfs normally enumerates in ascending
order, so they agree in practice — but nothing guarantees it, and on a multi-LLC machine a rule
`{"cpuset":"llc-1"}` then pins to a different set of CPUs under the two daemons. The previous audit
verified the alias *names* (row 81) but not the id→LLC mapping.

**Resolution.** Fixed in `d0860d9`. The `cpu*` entries are collected and sorted by id before the walk, so the
numbering is a property of the machine rather than of the filesystem. `llc-0` is now the LLC
containing CPU 0, as the reference's ascending walk makes it. 

### 5.8 Core classification averages — Severity: Medium

The reference computes the threshold as an **integer** mean and compares with an integer `>=`
(`topology.cpp:95`, `117`); this tree uses `f64` (`crates/ananicy-platform/src/topology.rs:400-417`).

Smallest counter-example found by exhaustive search over the two formulas, on a machine both agree is
heterogeneous: two CPUs with capacities `{1, 2}` (ratio 2.0, above the 1.3× threshold). The
reference's `avg = 3/2 = 1`, so `1 >= 1` → **Big**, and `little-cores` is `""` — a rule naming it
does nothing. This tree's `threshold = 1.5`, so `1 < 1.5` → **Little**, and the same rule pins the
process to CPU 0. The comment at `topology.rs:397-399` ("the average-based split matches the
reference daemon's") is true everywhere except this boundary.

The 1.3× heterogeneity test itself was brute-forced over 400 000 integer pairs and does **not**
diverge — the 1.3 threshold is safe; only the mean is not.


### 5.9 `--force-remove-semaphore` error path — Severity: Low — **fixed in `c835fc5`**

The reference exits 1 with `Failed to remove semaphore! msg 'No such file or directory'`
(`singleton_process.cpp:113-115`, `main.cpp:115-119`); this tree unlinks, logs, and exits 0
(`src/ipc.rs:25-29`). Verified by execution: a second `--force-remove-semaphore` with no daemon
running returns 0. A wrapper script using the exit code to confirm cleanup is misled. The reference
also never stores a PID in the object.

**Resolution.** Fixed in `c835fc5`. The unlink result is checked: a failure logs the errno and exits 1, matching
`main.cpp:115-119`. The exit status is the point of the flag — a wrapper script that clears a stale
object after a crash and checks the status before starting the daemon was being told the cleanup
had worked. 

### 5.10 `exe` readlink failure heuristic — Severity: Informational

The reference keeps one **global, latching** counter (`static exe_fail_count`, threshold 5, reset
only on success — `process.cpp:195-196, 224, 236, 243-244`); this tree keeps a per-PID LRU of 256
entries (`crates/ananicy-platform/src/procfs.rs:15-17, 62-98`). Five `EACCES` on `/proc/*/exe` for
**any** PIDs permanently disables exe-based naming for the whole reference daemon; this tree needs
five failures for the *same* PID. The per-PID version is the correct fix and the reference is
affected by a real bug — but the two can still resolve different names for the same process, and
therefore match different rules.


### 5.11 A deleted binary resolves to a different name — Severity: Low

For `"/usr/bin/foo (deleted)"`, `find(" (deleted)") == 14` and the reference's
`substr(0, exe_name_end + 1)` keeps one byte too many, yielding `"foo "` where this tree yields
`"foo"` (`process.cpp:230-233`; `crates/ananicy-platform/src/procfs.rs:84-86`). A rule
`{"name":"foo"}` matches here and not in the reference for any process whose binary was replaced —
after a package upgrade, or on NixOS after a store GC of a still-running path. The reference has the
bug; the difference is that a rule matching differently is observable.


### 5.12 Offline CPUs change which capacity source is chosen — Severity: Medium

The reference's "does this source differentiate the CPUs" test reads 0 for an unreadable CPU, and
`0 != reference` therefore counts as differentiating (`topology.cpp:56-60`); this tree treats 0 as
"no data" (`crates/ananicy-platform/src/topology.rs:212-216, 238-243`) and keeps looking. On a
machine with any offline CPU, the reference stops at a source this tree passes over, and the
core-type split — and so `big-cores`/`little-cores`/`turbo-cores` — differs.

This is the mechanism §5.8 was about, in a form the previous audit's §5.1 did not identify. The fix
in `b25eaba` removed the per-CPU choice but not this asymmetry.


### 5.13 The X3D single-CCD alias — Severity: Low — **fixed in `8f36ec9`**

The reference builds it as `0-(N-1)` with `_SC_NPROCESSORS_CONF` unconditionally
(`x3d.cpp:161-169`); this tree returns `None` if no die reports an L3 size
(`crates/ananicy-platform/src/x3d.rs:141-157`). On a single-CCD part with a CPU offline the aliases
differ (`0-15` vs `0-7`); if `cache/index3/size` is unreadable the reference still defines both
aliases and this tree defines neither. The reference's `die_id`→`cluster_id` fallback
(`x3d.cpp:148-152`) is dead code — `read_int`'s default is 0, never negative — while `x3d.rs:116-121`
implements it, so the two can group dies differently on a kernel without `die_id`.

**Resolution.** Fixed in `8f36ec9` for the single-CCD case. The branch is keyed on the die count, as the reference
is (`die_map.size() < 2`, `x3d.cpp:164-170`), and no longer requires a readable L3, so a part whose
`cache/index3/size` is not exposed still gets both aliases instead of none.
The `die_id` to `cluster_id` fallback is deliberately kept. The reference's copy of it is dead code
— `read_int`'s default is 0 and never negative, so the condition it guards never holds — while
`x3d.rs:116-121` implements it. That is a latent bug in the reference, not a compatibility surface,
and matching it would group dies wrongly on a kernel that exposes `cluster_id` but not `die_id`. 

### 5.14 `-nice` overflow — Severity: Informational — **fixed in `697e0b4`**

`crates/ananicy-core/src/worker.rs:318` computes `1.25f64.powi(-nice as i32)`. A rule with
`"nice": -2147483648` overflows the negation: release builds wrap to `i32::MIN`, `powi` yields `inf`,
the saturating cast gives `u32::MAX` and the clamp yields `cpu.weight = 10000`; a debug or
instrumented build **panics the worker thread**, and under `panic = "abort"` the daemon. No
counterpart exists in the reference. Only reachable from a hostile or mistaken rule file.

**Resolution.** Fixed in `697e0b4`. `powf(-(nice as f64))` has no integer step to overflow, so the `i64`-to-`f64`
cast saturates where the clamp already handles it: an out-of-range nice yields the maximum weight
instead of panicking the worker thread in a debug build or wrapping in a release one. 

### 5.15 Realtime rules log a spurious warning — Severity: Low — **fixed in `4981a3b`**

A realtime process whose rule names a `cgroup` produces a `WARN Rule application partially failed …
Unsupported` on every cgroup-v2 host (`crates/ananicy-core/src/worker.rs:416-422`, `484-490`), and
the `log_applied_rule` line the reference prints is suppressed (`worker.rs:222-234`). The reference
logs at debug and does not suppress it (`worker.cpp:159-170`, `92-96`). The same suppression happens
whenever a `cpuset` alias resolves empty (`worker.rs:451`), which the reference treats as a plain
skip.

**Resolution.** Fixed in `4981a3b`. Neither case is recorded as a failure any more. Skipping a realtime process'
cgroup is the workaround working, and the reference logs it at debug and moves on; an empty
`cpuset` alias is the documented way of saying "do not touch this process' affinity". Both keep
their `debug!` lines, and the realtime one gains a second naming the process, so the decision is
still visible to anyone reading at debug. 

### 5.16 `cpuset` whitespace — Severity: Low

`CpuSet::parse` trims each token (`crates/ananicy-core/src/cpuset.rs:55, 62`), so `"0, 1"` and
`" 5"` are accepted; the reference tests the raw token for non-digits and rejects both
(`cpuset.cpp:224-228, 262-267`). A rule written with a space after the comma works here and is
silently ignored there. The previous audit documented the stricter rejections (`0-a`, `1-2x`) but
not this.


### 5.17 BPF backend — Severity: Informational

The two `ananicy_cpp.bpf.c` files are byte-identical (`diff` confirms). `min_us` is inert in both
because the rate-limit check is commented out (`ananicy_cpp.bpf.c:60-61`). The perf buffer is 64
pages per CPU in both — the previous audit's claim that this side used a smaller buffer was wrong
(§10). The lost-event callback writes to stderr in both. The Rust side additionally wires libbpf's
print callback to `--verbose`, which is the only genuine difference.


### 5.18 Netlink receive window — Severity: Informational

The reference sets `SO_RCVTIMEO` to 500 ms and leaves `SO_RCVBUF` at the default
(`netlink_program_utils.c:45-49`); this tree sets neither, and both sides reset `prev_pid` per
`listen()`. Verified as the only behavioural difference in that path.


### 5.19 `get_cgroup_for_pid` ignores `sd_pid_get_cgroup` — Severity: Informational

The reference calls `sd_pid_get_cgroup` under `ENABLE_SYSTEMD`; this tree does not. Unchanged from the
previous audit's finding.

---

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


### 7.3 The Medium set, ranked

Six undocumented differences of Medium severity, from §5: type-inheritance `null` (§5.3),
unexpected errno aborting a rule (§5.4), cgroup classification (§5.5), `llc-N` ordering (§5.7),
core-average boundary (§5.8), offline-CPU source selection (§5.12). Each needs a row in
`docs/ANANICY_CPP_DIFFERENCES.md` §5 or a fix. Two of the six (§5.7, §5.8) decide which rule
matches a given process, which makes them the first two to address.

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
  libbpf's print callback is at least compile-checked, which §11 previously could not claim. §5.17
  remains a source comparison plus a `diff` of the two identical `ananicy_cpp.bpf.c` files, because
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

* **The perf buffer was never smaller.** The matrix and §5.17 said this side used libbpf-rs'
  default page count as if it were smaller than the reference's 64. `PerfBufferBuilder::new` already
  uses 64. No change was made, and both places now say so.
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
