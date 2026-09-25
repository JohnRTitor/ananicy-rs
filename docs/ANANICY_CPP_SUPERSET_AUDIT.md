# ananicy-rs vs ananicy-cpp — Superset Audit

**Audit date:** 2026-09-25
**Reference:** `/home/masum/Dev-Environment/Rust/ananicy-cpp` (source tree as found, no VCS history available)
**Target:** `/home/masum/Dev-Environment/Rust/ananicy-rs` @ `e131b18` ("feat(logging): report the detected topology and the loaded rules")
**Exception list under audit:** `docs/ANANICY_CPP_DIFFERENCES.md`

---

## 1. Executive summary

**Answer to the acceptance question: No.**
`ananicy-rs` is *not* currently a documented functional superset of `ananicy-cpp`. It is, on the
evidence collected, a **near-superset with three classes of unresolved items**:

1. **One true functional regression** (a C++ capability with no Rust counterpart):
   the `--manualscanning` CLI alias (`ananicy-cpp/src/main.cpp:78-84`) is rejected by `ananicy-rs`
   (`src/cli.rs:163`). A user migrating an `ExecStart`/`ExecReload` line that uses the old spelling
   gets a hard usage error. This is not in the differences document.
2. **One C++ recovery mechanism that is silently non-functional in Rust**, and is *claimed to work*
   in the differences document: `cgroup_realtime_workaround`. In C++ it re-detects the cgroup
   hierarchy (`get_cgroup_version(true)`) and re-creates the cgroups (`ananicy-cpp/src/main.cpp:296-302`).
   In Rust the equivalent code calls `mounts::reset_cgroup_info()` (`src/runtime.rs:75`), which clears
   a cache that **no live consumer reads any more**, because the cgroup manager is a `OnceLock`
   singleton that captured the `CgroupInfo` at first use (`crates/ananicy-platform/src/cgroups.rs:15-22`).
   The 100 ms sleep + reset + re-create is therefore a no-op with respect to a changed hierarchy.
3. **One documented difference that does not exist in the code**:
   `docs/ANANICY_CPP_DIFFERENCES.md:51` claims a "retry loop (up to ~10 seconds)" for startup cgroup
   detection. The function that implements it, `mounts::init_cgroups()`
   (`crates/ananicy-platform/src/mounts.rs:18-28`), has **zero callers** in the workspace.

In addition, the audit found **13 undocumented behavioral differences of Medium severity or above**
(§5.1–§5.13; §5.5 is documented as a mechanism but not as a blast radius), of which the most
consequential are:

* the capacity-source selection in topology detection is per-CPU in Rust and global in C++, and the
  "homogeneous" fallback fills `little-cores`/`turbo-cores` with *all* CPUs in Rust and with *nothing*
  in C++ — verified live on the audit host (§5.1, §5.2);
* an unknown `ioclass` aborts the rest of the rule in Rust but is ignored-and-continue in C++
  (§5.3);
* `ioclass: "none"` actually issues an `ioprio_set()` in Rust, while C++ refuses to touch the I/O
  priority at all (§5.4);
* applying a `nice` value silently rewrites `cpu.weight`/`cpu.shares` of the cgroup the process
  already lives in — an effect on co-tenants that C++ does not have and that the documentation does
  not state in those terms (§5.5);
* the X3D driver mode is written *before* the `dump`/`debug` dispatch and before the root/singleton
  checks, and is therefore never restored on those exit paths (§5.6);
* realtime detection uses `sched_getscheduler()` in Rust vs `sched_attr.sched_priority > 0` in C++ (§5.7).

### What *is* confirmed

* **Confirmed supersets** (26 concrete capabilities, §7): deterministic rule-load order, per-PID
  `exe`-symlink failure cache, PID-reuse-safe cgroup moves, BPF→Netlink fallback, ENOBUFS recovery
  with a full re-scan, 8 MiB netlink receive buffer, epoll-driven netlink drain, cgroup path-traversal
  rejection, single-writer cgroup-v2 ownership model, namespace-escape rejection in the cgroup
  resolver, live log-level reload, crash-free `check_freq` parsing, `create_dir_all` for
  nested cgroup names, `all` cpuset alias, `completions` subcommand, `--config`/`--config-dir` flags,
  `--systemd`/`--no-systemd` flags, `debug cgroups` systemd status line, `dump proc` `rule` field.
* **Confirmed equivalent** subsystems: rule file discovery/classification/JSON-span extraction/type
  inheritance via RFC 7396 merge-patch, `name_regex` PCRE2 semantics, exact-match-beats-regex
  precedence, all seven rule attributes and their application order, `apply_*` gating, thread-wide
  `nice`/`latency_nice`/affinity application, `oom_score_adj`, `sched` name mapping, `CPUQuota`
  computation constants, procfs command-name resolution ladder, autogroup parsing, cgroup v1/v2
  detection, LLC/NUMA/SMT grouping, X3D detection and driver-mode set/restore, singleton + reload
  lifecycle, signal handling, `dump`/`debug` output shape, unit file hardening, default config
  writing.
* **Documented and accurate** difference entries: of the 17 entries checked in
  `ANANICY_CPP_DIFFERENCES.md`, 13 verify exactly against the code, 3 are accurate but incomplete,
  and 1 is inaccurate (§6).
* 296 Rust tests pass; 296/296 green. No `todo!()`/`unimplemented!()`; no `unwrap()` in a
  production path except two that are provably total (`NetlinkMonitor::Drop`,
  `Process::new(...).parse::<DebugTarget>()`), plus one `expect()` on a channel send (§8.3).

---

## 2. Method

* Full read of the C++ reference: `src/main.cpp`, `src/config.cpp`, `src/rules.cpp`,
  `src/worker.cpp`, `src/utils.cpp`, `src/regex_utils.cpp`, `src/utility/argument_parsing/*`,
  `src/platform/linux/*` (13 files), `src/platform/{systemd,generic}/service.cpp`,
  `libananicycpp_bpf/*`, `libananicycpp_netlink/*`, `CMakeLists.txt`, `ananicy-cpp.service`,
  `README.md`, and the whole test suite under `src/tests/`.
* Full read of the Rust target: `src/*` (10 files), `crates/ananicy-core/src/*`,
  `crates/ananicy-platform/src/*` (incl. `abi/`, `cgroup/`), `crates/ananicy-bpf/src/*` and
  `bpf/ananicy_cpp.bpf.c`, all `Cargo.toml` feature sets, `Makefile`, `contrib/*.nix`,
  `data/ananicy-rs.service.in`, `docs/*`, and all 296 tests.
* Targeted negative searches: `unwrap/expect/panic!/unimplemented!/todo!/unreachable!`,
  `TODO/FIXME/XXX/HACK`, `allow(dead_code)`, cfg-feature gates, `let _ =` / discarded `Result`s,
  config-key consumers, and per-key config greps in both trees.
* Empirical verification on the audit host (AMD, 12 CPUs, cgroup v2, transient `.scope`):
  * `cargo test -p ananicy-rs -p ananicy-core -p ananicy-platform` → 296 passed.
  * `ananicy-rs --manualscanning` → usage error (regression confirmed).
  * `detect_topology()` on this host → `little-cores = "0-11"`, `turbo-cores = "0-11"` while
    `has_big_little == false` (divergence from C++ confirmed; see §5.1).
  * `ananicy-rs dump rules/types/cgroups`, `dump`, `dump bogus`, `debug`, `debug cgroups` → exit
    codes and output compared with `ananicy-cpp/src/main.cpp:171-210`.
  * A standalone probe crate was used to call `detect_topology()` without touching the repository.
* `ananicy-cpp` is not a git checkout, so history-based intent could not be consulted; every claim
  below rests on the source as found.

---

## 3. Difference matrix

Classification key: **EQ** Equivalent · **SUP** Superset · **MISS** Missing/Regression ·
**BEH** Behavioral Difference · **DOC** Documented Difference · **UNC** Unclear/Requires Verification.

| # | Subsystem | C++ Capability | Rust Status | Class | Evidence | In diff doc? | Required action |
|---|---|---|---|---|---|---|---|
| 1 | CLI | `--verbose` / `-v` | present | EQ | `src/main.cpp:48-52` ↔ `src/cli.rs:173-175` | n/a | none |
| 2 | CLI | `--help` / `-h` | present | EQ | `src/main.cpp:52-53` ↔ bpaf default, `src/cli.rs:255` | n/a | none |
| 3 | CLI | `--version` | present | EQ | `src/main.cpp:54-56,108-111` ↔ `src/cli.rs:137` | §1 (identity) | none |
| 4 | CLI | `--force-remove-semaphore` | present | EQ | `src/main.cpp:57-59,114-122` ↔ `src/ipc.rs:25-29` | §1 (implied) | none |
| 5 | CLI | `--reload` | present, SIGUSR1 | DOC | `src/main.cpp:60-62,124-131` ↔ `src/ipc.rs:63-89` | §3 | none |
| 6 | CLI | `--benchmark` | present | EQ | `src/main.cpp:63-65,150-152,307-311` ↔ `src/runtime.rs:38-45` | no (timing differs, #36) | document exit-timing difference |
| 7 | CLI | `--benchmark-count <n>` | present | BEH | C++ breaks the main loop (`main.cpp:313-316`); Rust sets the shutdown flag inside the worker (`worker.rs:133-137`) | no | document |
| 8 | CLI | `--bpf-min-us <n>` | always accepted | SUP | `src/main.cpp:71-76` (cfg-gated) ↔ `src/cli.rs:170-172` | §5 (regex) adjacent | none |
| 9 | CLI | `--manual-scanning` | present | EQ | `src/main.cpp:78-84` ↔ `src/cli.rs:162-163` | no | none |
| 10 | CLI | **`--manualscanning` alias** | **rejected** | **MISS** | `src/main.cpp:80` vs `src/cli.rs:163`; verified: `Error: no such flag` | **no** | **add the alias** |
| 11 | CLI | positional `action` (`start`/`dump`/`debug`) | present | EQ | `src/main.cpp:86-94` ↔ `src/cli.rs:102-134` | §2 | none |
| 12 | CLI | positional `sub-action` | present | EQ | `src/main.cpp:91-94` ↔ `src/cli.rs:108-124` | §2 | none |
| 13 | CLI | unknown action → run daemon anyway | exits 1 | BEH | `src/main.cpp:208-210` (no exit) ↔ `src/main.rs:98-101` | no | document (Rust is stricter) |
| 14 | CLI | `dump` without sub-action → exit 1 | exit 2 | BEH | `src/main.cpp:181-183` ↔ `src/cli.rs:108-112` | no | document or align |
| 15 | CLI | unknown dump sub-action → exit 1 | exit 2 | BEH | `src/main.cpp:194-197` ↔ `src/cli.rs:47-52,213-219` | no | document or align |
| 16 | CLI | `--config`, `--config-dir` | added | SUP | C++: env only (`main.cpp:133-136`) ↔ `src/cli.rs:149-154` | §1 | none |
| 17 | CLI | `--daemon` | added (warns, foreground) | SUP | `src/main.rs:75-77` | §CLI.md | none |
| 18 | CLI | `--systemd` / `--no-systemd` | added | SUP | `src/cli.rs:140-145`, `src/systemd.rs` | §2 | none |
| 19 | CLI | `completions <shell>` | added | SUP | `src/cli.rs:128-133` | no (doc bug, #49) | fix `docs/CLI.md:110,143` |
| 20 | CLI | no-args prints help, exit 0 | same | EQ | `src/main.cpp:103-106` ↔ `src/cli.rs:253-259` | no (doc bug) | fix `docs/CLI.md:102` |
| 21 | Config | `check_freq` (default 60) | same | EQ | `src/config.cpp:25,129-137` ↔ `crates/ananicy-core/src/config.rs:98,145-154` | n/a | none |
| 22 | Config | `check_freq` malformed | **crashes** | SUP | `std::stoul` throws out of `Config::check_freq()` (`config.cpp:132`) ↔ diagnostic + default kept (`config.rs:145-153`) | no | add to diff doc as improvement |
| 23 | Config | `apply_nice/sched/ionice/oom_score_adj/latnice/cpuset` | same | EQ | `src/config.cpp:19-25` ↔ `config.rs:99-105` | §6 (table) | none |
| 24 | Config | `apply_cgroup` key name | same | EQ | `src/config.hpp:40` ↔ `config.rs:164-165` | §6 | none |
| 25 | Config | `cgroup_load/type_load/rule_load` | same | EQ | `src/config.cpp:21-23` ↔ `config.rs:100-102` | §6 | none |
| 26 | Config | `cgroup_realtime_workaround` | present but **inert** | **BEH** | `src/main.cpp:296-302` ↔ `src/runtime.rs:73-79` + `cgroups.rs:15-22` | no | **make the manager re-detectable** |
| 27 | Config | `x3d_mode` (auto/cache/frequency) | same semantics | EQ | `src/config.hpp:75-79`, `main.cpp:270-290` ↔ `config.rs:169`, `src/startup.rs:176-195` | n/a | none |
| 28 | Config | `log_applied_rule` | present, stricter | DOC | `src/worker.cpp:88-96` ↔ `crates/ananicy-core/src/worker.rs:177-248` | §5 | none |
| 29 | Config | `loglevel` + live reload | live reload | DOC/SUP | `src/config.hpp:52-73` (no reload of level) ↔ `src/signals.rs:36-52` | §5 | none |
| 30 | Config | unknown keys silently kept | warned, kept | SUP | `src/config.cpp:80-89` ↔ `config.rs:181-184` | no | add to diff doc |
| 31 | Config | `apply_ioclass` key | **parsed, never used** | BEH (doc) | no C++ consumer either; Rust logs it as effective (`src/startup.rs:129`) and the Nix module defaults it (`contrib/module.nix:141`) | no | implement the gate or stop reporting it |
| 32 | Config | default file writing | same + creates parent dirs | SUP | `src/config.cpp:50-69` ↔ `config.rs:255-299` | no | add to diff doc |
| 33 | Rules | discovery of `*.rules/*.types/*.cgroups`, recursive | same | EQ | `src/rules.cpp:149-167` ↔ `crates/ananicy-core/src/rules.rs:39-72` | §6 | none |
| 34 | Rules | load order | **sorted** | SUP | `src/rules.cpp:156` (fs order) ↔ `rules.rs:49-57` | §5 | none |
| 35 | Rules | JSON span = first `{` … last `}` | same | EQ | `src/rules.cpp:53-65` ↔ `rules.rs:126-138` | §6 | none |
| 36 | Rules | `#`-comment / CRLF tolerance | same | EQ | `src/rules.cpp:40-48,108-112` ↔ `rules.rs:102-124` | §6 | none |
| 37 | Rules | classification name → type → cgroup | same | EQ | `src/rules.cpp:70-93` ↔ `rules.rs:141-174` | n/a | none |
| 38 | Rules | type inheritance (merge-patch) | same result, precomputed | EQ | `src/rules.cpp:198-206` ↔ `rules.rs:74-96,237-257` | n/a | none |
| 39 | Rules | `name_regex` (PCRE2, optional in C++) | always on | DOC/SUP | `src/rules.cpp:74-79,186-196`, `CMakeLists.txt:68-71` ↔ `rules.rs:145-153,208-214` | §5 | none |
| 40 | Rules | exact match beats regex | same | EQ | `src/rules.cpp:184-196` ↔ `rules.rs:201-217` | n/a | none |
| 41 | Rules | rule lookup cache | LRU 5000 | SUP | C++ has none ↔ `rules.rs:21,183-199` | no | add to diff doc |
| 42 | Rules | **NixOS `.foo-wrapped` name rewriting** | **added** | BEH (undoc.) | no C++ counterpart ↔ `crates/ananicy-core/src/worker.rs:155-171` | **no** | document |
| 43 | Rules | cgroup-path rule matching (exact/glob/ancestor/`!`) | **dead code** | UNC | C++ has none ↔ `crates/ananicy-core/src/cgroup_rules.rs` (no callers) | no | remove or finish + document |
| 44 | Attributes | `nice` (all threads) | same | EQ | `src/platform/linux/priority.cpp:41-63` ↔ `priority.rs:38-56` | n/a | none |
| 45 | Attributes | `nice` → cgroup `cpu.weight` mirror | **added** | BEH (undoc. scope) | C++ has none ↔ `worker.rs:313-328`, `cgroup/manager.rs:321-364` | §4 (incomplete) | document blast radius / add opt-out |
| 46 | Attributes | `latency_nice` (+ fallback to `nice`) | same | EQ | `src/worker.cpp:108-121` ↔ `worker.rs:331-354` | n/a | none |
| 47 | Attributes | `latency_nice` support probe | same | EQ | `src/platform/linux/priority.cpp:179-200` ↔ `priority.rs:190-215` | n/a | none |
| 48 | Attributes | `sched` mapping + `rtprio` default 1 | same | EQ | `src/platform/linux/priority.cpp:120-154`, `worker.cpp:123-134` ↔ `priority.rs:127-173`, `worker.rs:356-371` | n/a | none |
| 49 | Attributes | `sched: deadline` | both fall back to normal; C++ logs warn + returns success, Rust returns `Skipped` (warns "partially failed") | BEH (cosmetic) | `src/platform/linux/priority.cpp:134-139` ↔ `priority.rs:142-172` | no | align log level |
| 50 | Attributes | **unknown `ioclass`** | **aborts rest of rule** | **BEH** | C++ returns `true` and continues (`priority.cpp:102-105`) ↔ Rust `Err(Unsupported)` aborts (`priority.rs:112-114`, `worker.rs:384-390`) | **no** | return `Skipped` instead |
| 51 | Attributes | **`ioclass: "none"`** | **issues `ioprio_set(0,…)`** | **BEH** | C++ refuses via `ioprio_valid` (`priority.cpp:107-112`) ↔ Rust has no guard (`priority.rs:117-124`) | **no** | add the guard, or set `IOPRIO_DEFAULT` |
| 52 | Attributes | `ionice` default 0 | same | EQ | `src/worker.cpp:140-147` ↔ `worker.rs:376` | n/a | none |
| 53 | Attributes | `oom_score_adj` | same | EQ | `src/platform/linux/priority.cpp:156-177` ↔ `priority.rs:175-188` | n/a | none |
| 54 | Attributes | `cgroup` placement | delegated subtree only | DOC | `src/platform/linux/cgroups.cpp:19-41,143-215` ↔ `cgroup/manager.rs:67-117,178-261` | §4 | state the resulting path explicitly |
| 55 | Attributes | `cpuset` alias resolution | same | EQ | `src/worker.cpp:172-202` ↔ `worker.rs:435-478` | n/a | none |
| 56 | Attributes | `cpuset` empty-alias → skip | same | EQ | `src/worker.cpp:179-184` ↔ `worker.rs:442-452` | n/a | none |
| 57 | Attributes | `cpuset` max CPU bound | `_SC_NPROCESSORS_CONF` vs `max(conf,1024)` | BEH | `src/platform/linux/cpuset.cpp:203-205` ↔ `abi/affinity.rs:10-17`, `worker.rs:459` | no | document |
| 58 | Attributes | `cpuset` parse strictness | Rust stricter (`0-a`, `1-2x` rejected) | DOC | `src/platform/linux/cpuset.cpp:242-243` (`to_int`→0) ↔ `cpuset.rs:70-80` | §6 | none |
| 59 | Process | command-name ladder (cmdline→exe→comm) | same, + per-PID fail cache | SUP | `src/platform/linux/process.cpp:194-264` ↔ `procfs.rs:31-109` | no | add to diff doc |
| 60 | Process | full procfs scan | same | EQ | `src/platform/linux/process.cpp:266-287` ↔ `procfs.rs:114-135` | n/a | none |
| 61 | Process | netlink connector (fork/exec/comm) | same, epoll + 8 MiB rcvbuf | SUP | `libananicycpp_netlink/src/netlink_program_utils.c:55-160` ↔ `netlink.rs:38-191` | §5 (recovery) | mention buffer/timeout |
| 62 | Process | BPF exec/fork tracepoints | same `.bpf.c` | EQ | `libananicycpp_bpf/src/ananicy_cpp.bpf.c` ≡ `crates/ananicy-bpf/bpf/ananicy_cpp.bpf.c` | n/a | none |
| 63 | Process | BPF name resolution | C++ resolves in the callback; Rust defers to the worker | EQ (outcome) | `src/platform/linux/process.cpp:61-69` ↔ `bpf_monitor.rs:90-103`, `worker.rs:144-151` | no | note |
| 64 | Process | BPF verbose → libbpf debug | absent | BEH (diag) | `bpf_program_utils.c:16-26` ↔ `bpf_monitor.rs:42` (no verbose param) | no | add `--verbose` plumbing or document |
| 65 | Process | perf buffer size | 64 pages vs libbpf-rs default | BEH (perf) | `bpf_program_utils.c:76` ↔ `bpf_monitor.rs:73-108` | no | set page count explicitly |
| 66 | Process | realtime detection | `sched_getscheduler` vs `sched_attr.sched_priority>0` | BEH | `src/platform/linux/process_info.cpp:107-118` ↔ `lib.rs:46-51` | no | document / align |
| 67 | Process | kernel-thread detection | computed but unused in both | EQ | `src/worker.cpp:85` (dead) ↔ absent | n/a | none |
| 68 | Diagnostics | `dump proc` / `dump autogroup` | same fields + `rule` | SUP | `src/platform/linux/process_info.cpp:161-306` ↔ `src/dump.rs:53-176`, `process_info.rs:8-27` | no | add to diff doc |
| 69 | Diagnostics | `debug cgroups` | same + systemd status | SUP | `src/platform/linux/debug.cpp:27-53` ↔ `src/debug.rs:32-104` | §2 | none |
| 70 | Diagnostics | **terminate/backtrace handler** | **absent** | **MISS** | `src/platform/linux/backtrace.cpp:34-61` ↔ nothing (`Cargo.toml:79` `panic = "abort"`) | **no** | add a panic hook |
| 71 | Diagnostics | `runqslower` / netlink standalone tools | **absent** | **MISS** | `libananicycpp_bpf/main.cpp:96-139`, `libananicycpp_netlink/main.cpp:128-165` ↔ nothing | **no** | add or document |
| 72 | Topology | LLC (L3→L2), NUMA, SMT | same | EQ | `src/platform/linux/topology.cpp:123-160,285-338` ↔ `topology.rs:99-137,272-302` | n/a | none |
| 73 | Topology | capacity-source selection | **per-CPU** vs **global** | **BEH** | `topology.cpp:42-109` ↔ `crates/ananicy-platform/src/topology.rs:244-260` | **no** | select one source globally |
| 74 | Topology | 1.3× big.LITTLE threshold | same | EQ | `topology.cpp:24,97-98` ↔ `topology.rs:326` | n/a | none |
| 75 | Topology | homogeneous fallback | **`little`/`turbo` = all** vs **empty** | **BEH** | `topology.cpp:111-121,301-312` ↔ `topology.rs:326-335` | **no** | set them empty; add a test |
| 76 | Topology | turbo tier selection | same | EQ | `topology.cpp:114,301-304` ↔ `topology.rs:363-367` | n/a | none |
| 77 | Topology | `cpus_by_capacity` ordering | dropped (unused field) | EQ (not observable) | `topology.cpp:345-349` ↔ absent | n/a | none |
| 78 | Topology | per-CPU `CpuInfo` fields | not stored (only used for grouping in C++) | EQ | `topology.cpp:235-283` ↔ `topology.rs:234-260` | n/a | none |
| 79 | Topology | `parse_size_string` units | K/M only (C++ K/M/G) | BEH (benign) | `src/platform/linux/sysfs.cpp:46-69` ↔ `topology.rs:139-154`, `x3d.rs:127-130` | no | add `G` |
| 80 | Topology | `stoi` on a `nodeN` name | returns 0 | SUP (no `terminate`) | `topology.cpp:128-136` (`noexcept` + `stoi` ⇒ `std::terminate`) ↔ `topology.rs:104-117` | no | add to diff doc |
| 81 | cpuset aliases | big/little/turbo/performance/efficiency/all-cores/x3d-*/llc-N/node-N | same set + `all` | SUP | `src/worker.cpp:31-70` ↔ `topology.rs:55-88` | n/a | none |
| 82 | X3D | detection (driver + `/proc/cpuinfo`) | same | EQ | `src/platform/linux/x3d.cpp:22-220` ↔ `x3d.rs:75-186` | n/a | none |
| 83 | X3D | driver mode set + restore on exit | set before dump/root checks, restored late | **BEH** | `src/main.cpp:270-290,338-345` ↔ `src/main.rs:81`, `src/startup.rs:176-195` | no | move after the dispatch |
| 84 | Cgroups | v1/v2 detection incl. `cpu`/`cpu.max` probes | same | EQ | `src/platform/linux/cgroups.cpp:217-333` ↔ `mounts.rs:30-107` | n/a | none |
| 85 | Cgroups | reset + re-detect | **manager frozen in `OnceLock`** | **BEH** | `cgroups.cpp:217-225` ↔ `cgroups.rs:15-22`, `runtime.rs:75` | no | re-create the manager on reset |
| 86 | Cgroups | startup retry loop (~10 s) | **documented but absent** | **MISS (doc)** | n/a (not in C++) ↔ `mounts.rs:18-28` (no callers); claim at `ANANICY_CPP_DIFFERENCES.md:51` | claimed | wire it up or delete the claim |
| 87 | Cgroups | v1 path `<mount>/cpu/<name>`, v2 `<mount>/<name>` | v1 same; v2 relative to delegated root | DOC | `cgroups.cpp:19-41` ↔ `cgroup/manager.rs:67-117` | §4 (partial) | state the path explicitly |
| 88 | Cgroups | `create_cgroup` idempotence | same | EQ | `cgroups.cpp:43-75` ↔ `cgroups.rs:24-40` | n/a | none |
| 89 | Cgroups | nested cgroup names | `create_dir_all` | SUP | `create_directory` (`cgroups.cpp:60,71`) ↔ `manager.rs:140,161` | no | add to diff doc |
| 90 | Cgroups | `CPUQuota` v1 (period 1e6) / v2 (period 1e5) | same constants | EQ | `cgroups.cpp:77-141` ↔ `manager.rs:263-319` | n/a | none |
| 91 | Cgroups | `CPUQuota` CPU count | `hardware_concurrency` vs `available_parallelism` | BEH | `cgroups.cpp:93,123` ↔ `manager.rs:278` | no | document |
| 92 | Cgroups | `CPUQuota` write order | quota→period vs period→quota | BEH (benign) | `cgroups.cpp:95-96` ↔ `manager.rs:297-316` | no | none |
| 93 | Cgroups | `CPUQuota` non-numeric | **`terminate` risk** → skip | SUP | `rules.cpp:173-176` (`get<unsigned>()` in a `noexcept` fn) ↔ `runtime.rs:90-93` (`as_u64`) | no | add to diff doc |
| 94 | Cgroups | `CPUWeight` `.cgroups` attribute | **documented, not implemented** | MISS (doc) | C++ README:310 says "Only one attribute is supported, `CPUQuota`" ↔ `docs/CONFIGURATION.md:100` claims `CPUWeight`; no consumer in `runtime.rs:88-98` | no | implement or remove the claim |
| 95 | Cgroups | `add_pid_to_cgroup` (`tasks`/`cgroup.procs`) | same + start-time guard | SUP | `cgroups.cpp:143-215` ↔ `manager.rs:178-261` | §5 | none |
| 96 | Cgroups | v2 `cgroup.procs` needs the TGID | explicit `Tgid` read | SUP | C++ writes `max(pid,0)` (`cgroups.cpp:195`) ↔ `manager.rs:210-219` | no | add to diff doc |
| 97 | Cgroups | single write syscall for the pid | one `write_all` | SUP | `cgroups.cpp:194-195` (`fstream <<` = 2 writes) ↔ `manager.rs:221-233` | no | add to diff doc |
| 98 | Cgroups | foreign-cgroup protection / single-writer model | enforced | SUP | n/a in C++ ↔ `cgroup/ownership.rs`, `manager.rs:129-176` | §4 | none |
| 99 | Cgroups | path traversal rejection | `..` rejected | SUP | n/a in C++ ↔ `manager.rs:67-75` | §4 (partial) | none |
| 100 | Cgroups | realtime workaround target | `/sys/fs/cgroup` root in both | EQ | `cgroups.cpp:19-41` with `""` ↔ `worker.rs:264` with `"/"` | n/a | none |
| 101 | IPC | singleton object | POSIX shm, PID in payload | BEH (name) | `SingletonProcess("/AnanicyCppMutex")` (`main.cpp:113`) ↔ `IPC_NAME = "/AnanicyRsMutex"` (`src/ipc.rs:15`) | §1 (not listed) | list the IPC name |
| 102 | IPC | singleton permissions | `0644` | `0600` | `singleton_process.cpp:45` ↔ `src/ipc.rs:52` | no | document |
| 103 | IPC | reload transport | SysV shm "RELOAD", 1 s poll | SIGUSR1 | `singleton_process.cpp:79-149` ↔ `src/ipc.rs:63-89`, `src/signals.rs:32-53` | §3 | none |
| 104 | IPC | reload scope | config only, no rules | same | `main.cpp:225` ↔ `src/signals.rs:35` | §3, §5 | none |
| 105 | Signals | SIGINT/SIGTERM shutdown | same | EQ | `src/utils.cpp:16-28`, `main.cpp:232,306` ↔ `src/signals.rs:54-63` | n/a | none |
| 106 | Signals | SIGUSR1 | added | SUP | n/a ↔ `src/signals.rs:22-24,32` | §3 | none |
| 107 | systemd | `sd_notify(READY/STOPPING)` | same + auto-detection | SUP | `src/platform/systemd/service.cpp:12-26`, `main.cpp:304` ↔ `src/runtime.rs:67-71`, `src/signals.rs:58-61` | §2, §SYSTEMD | none |
| 108 | systemd | compile-time systemd gate | cargo feature | EQ | `CMakeLists.txt:82-88,188-194` ↔ `Cargo.toml:69-73` | §SYSTEMD | none |
| 109 | systemd | `get_unit_name` | same + heuristic fallback | SUP | `src/platform/systemd/service.cpp:34-46` ↔ `service.rs:15-60` | no | add to diff doc |
| 110 | systemd | `get_cgroup_for_pid` | `/proc/<pid>/cgroup` only | BEH (diag) | `cgroups.cpp:338-357` (uses `sd_pid_get_cgroup` when systemd) ↔ `src/debug.rs:83-104` | no | document |
| 111 | systemd | unit file | equivalent + `Delegate=yes` | SUP | `ananicy-cpp.service` ↔ `data/ananicy-rs.service.in:16` | §4 | none |
| 112 | Runtime | manual periodic scan | same cadence, thread | EQ | `main.cpp:318-321` ↔ `src/runtime.rs:100-115` | §5 (reload scope) | none |
| 113 | Runtime | manual scan self-nicing to 19 | same | EQ | `main.cpp:159-165` ↔ `src/main.rs:133-137` | n/a | none |
| 114 | Runtime | `processed_processes` summary | same fields | EQ | `main.cpp:349-354` ↔ `src/monitor.rs:118-131` | n/a | none |
| 115 | Runtime | initial full scan before the worker | concurrent with the worker | BEH (benign) | `src/platform/linux/process.cpp:163` vs `src/monitor.rs:33-36,71-74` | no | none needed |
| 116 | Error handling | `EPERM` on a priority | aborts the rule | continues the rule | DOC | `src/worker.cpp:203-206` (throw → catch) ↔ `worker.rs:307-311,38-44` | §5 | none |
| 117 | Error handling | unknown `sched` | aborts the rule | aborts the rule | EQ | `priority.cpp:141-143` ↔ `priority.rs:148-150` | n/a | none |
| 118 | Error handling | `deadline` sched | warn, return success | warn, `Skipped` | BEH (cosmetic) | `priority.cpp:134-139` ↔ `priority.rs:142-172` | no | align |
| 119 | Error handling | `create_cgroups` result | ignored | ignored | EQ | `main.cpp:230` ↔ `runtime.rs:51-53,88-98` | n/a | none |
| 120 | Concurrency | event queue | unbounded | unbounded (`mpsc`) | EQ | `include/utility/synchronized_queue.hpp:45-92` ↔ `src/main.rs:118` | n/a | none |
| 121 | Concurrency | worker `stop()` on exit | yes | yes (channel close) | EQ | `main.cpp:335` ↔ `src/monitor.rs:118` | n/a | none |
| 122 | Concurrency | `tx.send(...).expect(...)` | n/a | **panic on the main thread** | — ↔ `crates/ananicy-platform/src/netlink.rs:185` | no | replace with a shutdown flag |
| 123 | Build | regex optional | unconditional | DOC | `CMakeLists.txt:68-71,144-157` ↔ `crates/ananicy-core/Cargo.toml` | §5 | none |
| 124 | Build | netlink / BPF selection | `USE_BPF_PROC_IMPL` | `netlink` / `bpf` features | EQ | `CMakeLists.txt:196-205` ↔ `Cargo.toml:69-73` | n/a | none |
| 125 | Build | static build | `STATIC` option | none | BEH (packaging) | `CMakeLists.txt:93-98` | no | document |
| 126 | Build | arch support for BPF vmlinux | x86/arm64/loongarch/riscv | same 4 | EQ | `libananicycpp_bpf/*/vmlinux.h` ↔ `crates/ananicy-bpf/build.rs:12-19` | n/a | none |
| 127 | Build | fuzz targets | 3 (cpuset, mounts, rule) | 1 (mounts) | MISS (test) | `src/tests/fuzzer-parse_*.cpp` ↔ `crates/ananicy-platform/fuzz/fuzz_targets/parse_mounts.rs` | no | add cpuset/rule fuzzers |
| 128 | Packaging | RPM spec | present | absent | MISS (pkg) | `fedora/ananicy-cpp.spec` | no | informational |
| 129 | Packaging | Nix packaging | absent | present | SUP | `flake.nix`, `contrib/*.nix` | no | informational |

---

## 4. Missing feature register

Every item below is a capability present in `ananicy-cpp` and absent (or inert) in `ananicy-rs`,
**not** covered by `docs/ANANICY_CPP_DIFFERENCES.md`.

### 4.1 `--manualscanning` CLI alias — Severity: Medium

| Field | Value |
|---|---|
| C++ location | `ananicy-cpp/src/main.cpp:78-84` — `.name("--manual-scanning").name("--manualscanning")` registers **two** spellings for the same flag; `argparse::argument::name` (`src/utility/argument_parsing/argument.cpp:77-103`) pushes each long name into `m_long_names`, and `argument_parser::parse_args` (`argument_parser.cpp:33-36,51-69`) resolves either. |
| C++ behavior | Both `--manual-scanning` and `--manualscanning` enable periodic procfs scanning. The legacy spelling exists for Ananicy (Python) command-line compatibility, which the C++ README advertises ("CLI compatibility with Ananicy"). |
| Expected Rust equivalent | `src/cli.rs:162-163` should register both spellings. bpaf's derive accepts several `long("…")` attributes on one field, which is how a single boolean gets two names. |
| Current Rust state | Only `--manual-scanning`. Verified: `ananicy-rs --manualscanning` → `Error: no such flag: '--manualscanning', did you mean '--manual-scanning'?`, exit 2. |
| Why a regression | Any unit file, wrapper script or documentation written against `ananicy-cpp` that uses the legacy spelling now fails to start the daemon. This is a user-facing option that exists in the reference and is silently gone. |
| Impact | Daemon fails to start (exit 2) for a configuration that worked before. Not data-loss, but a hard break of an advertised compatibility promise. |
| Suggested location | `src/cli.rs:162-163`. |
| Suggested test | `tests/cli.rs`: `ananicy-rs --manualscanning --help` exits 0 and prints the usage (mirroring the shape of `test_cli_string_argument_with_value`). |

### 4.2 `cgroup_realtime_workaround` re-detection is inert — Severity: High

| Field | Value |
|---|---|
| C++ location | `ananicy-cpp/src/main.cpp:296-302`; the re-detection primitive is `control_groups::get_cgroup_version(bool reset = true)` (`src/platform/linux/cgroups.cpp:217-225`), which clears a `static std::optional<cgroup_info>`; every later `create_cgroup` / `add_pid_to_cgroup` / `set_cgroup_cpu_quota` call re-reads that value through `get_cgroup_version()`. |
| C++ behavior | 100 ms after the worker starts, clear the cached hierarchy detection, re-run `Rules::create_cgroups()`. If systemd (or the delegated-root machinery) has created the cgroup subtree in the meantime, the cgroups are created in the right place; if the first detection failed (early boot, late cgroup mount), the second attempt can succeed. |
| Expected Rust equivalent | `src/runtime.rs:73-79` performs the same three steps: `sleep(100ms)` → `mounts::reset_cgroup_info()` → `create_cgroups(&rules)`. |
| Current Rust state | `mounts::reset_cgroup_info()` clears `CGROUP_INFO` (`crates/ananicy-platform/src/mounts.rs:10-16`), but every mutating entry point goes through `cgroups::get_manager()`, a `static MANAGER: OnceLock<CgroupManager>` (`crates/ananicy-platform/src/cgroups.rs:15-22`) that was already initialized by the first `create_cgroups` call and holds a **frozen copy** of `CgroupInfo` plus the `delegated_root` computed from it. `reset_cgroup_info()` therefore changes nothing for the code that mutates cgroups. The only remaining consumer of the resettable cache is `src/debug.rs:35` (display) and `LinuxPlatform::is_cgroup_v2()` (`lib.rs:61-63`) — i.e. the reported version and the version used for writes can now disagree. |
| Why a regression | The configuration option `cgroup_realtime_workaround` is documented in `docs/CONFIGURATION.md:28` and in the C++ code, and it is C++'s mitigation for cgroup-v2 setup races. In Rust it performs a 100 ms sleep and re-iterates over cgroups that already exist (`create_cgroup` returns early when the target exists, `cgroups.rs:27-30`). No recovery can occur. |
| Impact | On hosts where the daemon starts before the cgroup hierarchy is usable, cgroup rules are silently never applied and the daemon never recovers. Silent: `create_cgroup` failures are logged at `error!` at most and the return value is discarded (`src/runtime.rs:51-53`). |
| Suggested location | `crates/ananicy-platform/src/cgroups.rs:15-22` (drop the `OnceLock`, or add `pub fn reset_manager()` and call it from `mounts::reset_cgroup_info()`), plus `src/runtime.rs:73-79`. |
| Suggested test | `crates/ananicy-platform/tests/cgroup_manager.rs`: after `reset_cgroup_info()` + a changed mount table, `cgroups::create_cgroup` must target the new hierarchy. Also assert `LinuxPlatform::is_cgroup_v2()` and `CgroupManager::info().version` cannot disagree. |

### 4.3 `mounts::init_cgroups()` — documented as implemented, never called — Severity: High (documentation) / Low (code)

| Field | Value |
|---|---|
| C++ location | Not present (C++ has no such retry). |
| Claim | `docs/ANANICY_CPP_DIFFERENCES.md:51`: *"On systems where cgroup filesystems mount slightly after the daemon starts during early boot, `ananicy-cpp` may fail to detect cgroups. `ananicy-rs` implements a retry loop (up to ~10 seconds)…"* |
| Implementation | `crates/ananicy-platform/src/mounts.rs:18-28` — 20 iterations × 500 ms, resets and re-detects each time. The logic matches the claim exactly. |
| Current Rust state | `rg -n 'init_cgroups'` over the whole workspace returns only the definition. It is `pub` in a library crate, so `dead_code` does not fire and the omission is invisible to the compiler. Nothing in `src/main.rs` or `src/runtime.rs` calls it. |
| Why this matters | The differences document is the project's authoritative parity contract. An entry that describes code which is never executed is worse than a missing entry, because a reviewer auditing parity will mark the concern as resolved. |
| Impact | The documented robustness improvement does not exist; combined with 4.2 the daemon has no startup-cgroup recovery path at all. |
| Suggested location | `src/main.rs` before `startup::load_topology_aliases`, or `src/runtime.rs` before `create_cgroups`; alternatively delete the claim from the differences document. |
| Suggested test | `crates/ananicy-platform/tests/mounts.rs`: a unit test that the retry helper is exercised (e.g. by extracting the loop with an injectable sleep/attempt count), plus a CLI test asserting the startup path calls it. |

### 4.4 `std::terminate` backtrace handler — Severity: Low

| Field | Value |
|---|---|
| C++ location | `ananicy-cpp/src/platform/linux/backtrace.cpp:34-61`; installed by a static initializer at line 60-61, so it is always active. |
| C++ behavior | On `std::terminate` (uncaught exception, `noexcept` violation, e.g. the `std::stoul` throw in `Config::check_freq()` at `src/config.cpp:132` or the `get<unsigned>()` throw in the `noexcept` `Rules::create_cgroups()` at `src/rules.cpp:175`) it prints the demangled exception plus 20 symbolized stack frames to stderr before `abort()`. |
| Expected Rust equivalent | A panic hook / `std::panic::set_hook` that prints a backtrace (`std::backtrace::Backtrace`) before aborting. |
| Current Rust state | None. Release builds set `panic = "abort"` (`Cargo.toml:79`), so a panic aborts with a one-line message and no frames. |
| Why a regression | A diagnostic that exists in the reference is gone. `panic = "abort"` does not prevent a panic hook from running, so the capability is cheaply recoverable. |
| Impact | Harder post-mortem for a crash in the field. |
| Suggested location | New `src/panics.rs` (or the top of `main()`), gated on `not(debug_assertions)`. |
| Suggested test | Hard to unit-test; assert via a CLI test that a deliberately panicking code path prints a `RUST_BACKTRACE`-style section (would need a hidden test flag). Acceptable to leave untested. |

### 4.5 Standalone event-source debug tools — Severity: Low

| Field | Value |
|---|---|
| C++ location | `ananicy-cpp/libananicycpp_bpf/main.cpp:96-139` (a `runqslower_cpp` CLI: `-v`, positional `min_us`, prints `COMM/TID/LAT(us)`) and `libananicycpp_netlink/main.cpp:128-165` (prints `comm/pid/timestamp` for every proc event). Both are built by `add_subdirectory` in `CMakeLists.txt:196-205`. |
| C++ behavior | Standalone binaries that let an operator verify the event source in isolation, independent of the rule engine. |
| Expected Rust equivalent | A second binary (e.g. `src/bin/runqslower.rs`) or a hidden subcommand. |
| Current Rust state | Absent. `ananicy-rs` ships one binary. |
| Why a regression | These are shipped, buildable artifacts of the reference project. They are debugging aids rather than daemon capabilities, so the severity is low, but they are part of "what the project ships". |
| Impact | Users debugging "no rules are being applied" lose the ability to test the monitor in isolation. |
| Suggested location | `src/bin/ananicy-rs-monitor.rs`, reusing `BpfMonitor`/`NetlinkMonitor` (both already expose `listen(tx, shutdown)`). |
| Suggested test | A CLI test that the binary prints a header line and exits on SIGINT. |

### 4.6 Fuzz targets for the cpuset and rule parsers — Severity: Low (verification gap)

`ananicy-cpp/src/tests/` ships three fuzzers: `fuzzer-parse_cpuset.cpp`, `fuzzer-parse_mounts.cpp`,
`fuzzer-parse_rule.cpp`, plus a checked-in corpus (`src/tests/corpus-cpuset/` with `alias1`,
`complex`, `high`, `mixed`, …). `ananicy-rs` has only
`crates/ananicy-platform/fuzz/fuzz_targets/parse_mounts.rs`. The two other parsers are the ones
that consume untrusted operator input and have historically diverged (`0-a`, `1-2x`, `,,`, leading
`-`). Note that `crates/ananicy-core/tests/cpuset.rs` (37 cases) and `tests/rules.rs` (22 cases)
cover the *known* divergences well, so the practical risk is low.

### 4.7 RPM packaging — Severity: Informational

`ananicy-cpp/fedora/ananicy-cpp.spec` (and the repology badge in its README) has no counterpart.
`ananicy-rs` ships Nix packaging (`flake.nix`, `contrib/package.nix`, `contrib/module.nix`) and a
`Makefile`, which C++ does not. Parity in packaging ecosystems is not a daemon-capability question;
recorded for completeness.

### 4.8 `STATIC` build option — Severity: Informational

`CMakeLists.txt:93-98` offers a fully static binary. `ananicy-rs` has no equivalent Cargo profile
(its release profile is `opt-level=3, lto="fat", codegen-units=1, strip=true, panic="abort"`).
`rustc` supports `-C target-feature=+crt-static` on `x86_64-unknown-linux-gnu`; not exposed.

---

## 5. Behavioral difference register

### 5.1 Capacity-source selection is per-CPU in Rust, global in C++ — Severity: Medium

* **C++** (`src/platform/linux/topology.cpp:42-109`): walks `SOURCES`
  (`amd_pstate_prefcore_ranking`, `amd_pstate_highest_perf`, `acpi_cppc/highest_perf`, `cpu_capacity`,
  `cpufreq/cpuinfo_max_freq`) **once**. For each source it reads CPU 0; if the value is `> 0` it
  adopts that source, then verifies that the value actually *differs* across CPUs; the first source
  that differentiates wins, and the same file is used for every CPU.
* **Rust** (`crates/ananicy-platform/src/topology.rs:244-260`): for **each CPU independently**,
  `paths.into_iter().find_map(|p| read cpuN/p)` takes the first path that yields a number.
* **Why they differ**: a source that is present, readable, and *uniform* (e.g. a kernel that exports
  `acpi_cppc/highest_perf` with the same value for every CPU) is skipped by C++, which then falls
  through to a differentiating source. Rust accepts it for every CPU, so the machine is classified
  homogeneous even when a differentiating `cpu_capacity` was available.
* **Observable?** Yes — the entire core-type classification, and therefore `big-cores`,
  `little-cores`, `turbo-cores`, `performance-cores` change.
* **Intentional?** Unclear. There is no comment or test for source selection; the constant list is
  duplicated verbatim, which suggests the intent was to mirror C++, not to change it.
* **Documented?** No.
* **Impact** On a hybrid machine that exposes a uniform higher-priority source, `little-cores` and
  `big-cores` both collapse to `all-cores`; rules pinning "background work to efficiency cores"
  silently become no-ops.

### 5.2 The "not big.LITTLE" fallback fills `little-cores`/`turbo-cores` with every CPU — Severity: Medium

* **C++** (`topology.cpp:111-121`, `301-312`): when `has_biglittle == false`,
  `classify_core_type` returns `Big` for every CPU, so `little_cpus` and `turbo_cpus` stay empty and
  `build_cpuset_string({})` returns `""` (`topology.cpp:197-199`).
  `little_cores_str == ""` and `turbo_cores_str == ""`.
* **Rust** (`topology.rs:326-335`): the "highest < lowest × 1.3" branch sets
  `big_cores_str = all`, **`little_cores_str = all`**, **`turbo_cores_str = all`**.
  (The other homogeneous path, `metric_to_cores.len() <= 1` at `topology.rs:306-315`, does set them
  to `""` — so the two homogeneous paths in Rust disagree with each other as well.)
* **Verified live.** On the audit host (12 CPUs, `amd_pstate_prefcore_ranking` ∈ {166,181,186},
  ratio 1.12 < 1.3):
  ```
  summary: 12 CPUs, 1 LLCs, 1 NUMA nodes, SMT=on, big.LITTLE=no
  alias little-cores      = "0-11"
  alias efficiency-cores  = "0-11"
  alias turbo-cores       = "0-11"
  ```
  C++ on the same host yields `""` for all three.
* **Observable?** Yes. The worker special-cases an *empty* alias as "skip the affinity call"
  (`worker.rs:441-452`, mirroring `src/worker.cpp:179-184`). So for a rule
  `{"name":"file-indexer","cpuset":"efficiency-cores"}`:
  * C++ → affinity untouched;
  * Rust → `sched_setaffinity` over **all** CPUs, which *widens* a restricted mask (systemd
    `CPUAffinity=`, `taskset`, a container `cpuset.cpus`, a `CPUQuota`-derived restriction).
* **Intentional?** Unclear. `crates/ananicy-platform/tests/topology.rs:79-89` pins
  `little == ""` and `turbo == ""` — but only for the *other* homogeneous path
  (`x3d/amd-x3d-single-ccd`, no capacity data at all). The 1.3× branch is untested, which is
  consistent with an oversight rather than a decision.
* **Documented?** No. `docs/TOPOLOGY.md:29` only says *"On systems without heterogeneous cores,
  `big-cores` and `all-cores` resolve to the same set of CPUs"* — it does not say that
  `little-cores` becomes an alias for all CPUs (which contradicts the meaning of the name and
  `docs/TOPOLOGY.md:19-20`).
* **Impact** Silent, machine-class-wide, and affects a documented use case
  (`docs/TOPOLOGY.md:41-42`).

### 5.3 An unknown `ioclass` aborts the remaining rule in Rust — Severity: Medium

* **C++** (`src/platform/linux/priority.cpp:102-105`): unknown class → `spdlog::error(...)` and
  **`return true;`**. In `src/worker.cpp:144-148`, `if (!set_io_priority(...)) continue;` therefore
  does *not* skip, and `oom_score_adj`, `cgroup` and `cpuset` are still applied.
* **Rust** (`crates/ananicy-platform/src/priority.rs:112-114`): unknown class → `Err(Unsupported)`.
  `Unsupported` is **not** in `PlatformError::is_skippable()` (`worker.rs:38-44`), so
  `worker.rs:384-390` returns `Err`, and `apply_rule` bails out at that point: `oom_score_adj`,
  `cgroup` and `cpuset` are **not** applied.
* **Observable?** Yes, for any rule with a typo'd/unknown `ioclass` that also sets other attributes.
* **Intentional?** Unclear; the `is_skippable` taxonomy treats `Unsupported` as fatal on purpose,
  which is right for `sched` (C++ also aborts there) but wrong for `ioclass` (C++ continues).
* **Documented?** No.
* **Impact** Silent partial application: the OOM score / cgroup / affinity parts of such a rule
  never take effect.

### 5.4 `ioclass: "none"` actually changes the I/O priority in Rust — Severity: Medium

* **C++** (`priority.cpp:107-112`): `ioprio_valid(mask)` is
  `IOPRIO_PRIO_CLASS(mask) != IOPRIO_CLASS_NONE`; with `ioclass: "none"` the composed value has class
  `NONE`, so C++ logs *"IO priority is invalid, skipping..."* and **returns true without calling
  `ioprio_set` at all**. The task keeps its existing `task->ioprio`.
* **Rust** (`priority.rs:117-124`): no `ioprio_valid` equivalent; `ioprio_set(WHO_PROCESS, pid,
  IOPRIO_PRIO_VALUE(0, 0))` is issued. A freshly created task has
  `task->ioprio = IOPRIO_DEFAULT = IOPRIO_PRIO_VALUE(IOPRIO_CLASS_BE, IOPRIO_BE_NR/2) = 0x4004`;
  after the Rust call it is `0` (class `NONE`). The kernel's `blk_ioprio_get()` returns that value
  verbatim, so the process is no longer at the "system default" the documentation promises.
* **Observable?** Yes, for the exact spelling the documentation recommends:
  `docs/CONFIGURATION.md:66,91` (`"ioclass": "none", "ionice": 0`).
* **Intentional?** Unclear. Rust's behaviour is arguably closer to the *documented* intent
  ("Reset I/O policy to system default") — but only if the implementation actually writes
  `IOPRIO_DEFAULT`; writing class `NONE` is not the same thing.
* **Documented?** No.
* **Impact** I/O-class changes for rules that ask for `none`; a process that previously inherited
  its parent's I/O priority is reset to class `NONE`/0 instead of keeping BE 4.

### 5.5 Applying `nice` rewrites `cpu.weight` of the process's existing cgroup — Severity: High (undocumented scope)

* **C++**: no such behaviour exists anywhere in the tree.
* **Rust** (`worker.rs:313-328`): whenever `apply_nice` is on, the rule has `nice`, and the host is
  cgroup v2, the worker computes `weight = clamp(100 * 1.25^-nice, 1, 10000)` and calls
  `PlatformActions::set_cpu_weight`. `LinuxPlatform::set_cpu_weight` (`lib.rs:97-105`) resolves the
  **process's current cgroup** (`/proc/<pid>/cgroup`) and writes `cpu.weight` into it
  (`cgroup/manager.rs:337-348`). `CgroupOwnership::classify` returns `Foreign` for that path, but
  `set_cpu_weight` deliberately proceeds (`manager.rs:328-335`).
* **Why it matters**: the write lands on a *cgroup*, not a process. If the matched process lives in
  `user.slice/user-1000.slice/session-2.scope`, every other process in that scope is reweighted to
  e.g. `32` for `nice: 5`. C++ has no such cross-process side effect.
* **Intentional?** Yes for the mechanism (differences doc §4 "Optional resource tuning",
  `docs/CONFIGURATION.md:35`), but the *scope* is not stated: nothing tells the operator that one
  process's `nice` rule reweights its whole cgroup.
* **Documented?** Partially — the mechanism, not the blast radius.
* **Impact** Priority inversion for unrelated processes in the same cgroup; possible desktop
  responsiveness regressions. There is no configuration key to disable it.

### 5.6 The X3D driver mode is written before the action dispatch and never restored on early exits — Severity: Medium

* **C++ ordering** (`src/main.cpp`): parse → help/version → semaphore ops → config load →
  `Rules` load (169) → `dump`/`debug` dispatch, which `std::exit`s at 198/207 → root check (213) →
  singleton (219) → cgroup create (230) → process listener (239) → topology (253) → x3d detect (263)
  → **x3d_mode apply (270-290)** → worker (294) → workaround (296) → `READY` (304) → main loop →
  **x3d restore (338-345)**.
* **Rust ordering** (`src/main.rs`): parse → config load → logging → semaphore ops → version log (79)
  → **`load_topology_aliases` (81), which applies `x3d_mode`** → rules load (82) →
  **`dump` dispatch, `return` (84-87)** → **`debug` dispatch, `return` (91-94)** → unknown action
  `exit(1)` (98-101) → non-root `exit(1)` (105-108) → singleton failure `exit(1)` (110-116) →
  daemon start.
* **Consequences**:
  1. `ananicy-rs dump rules` with `x3d_mode=cache` **switches the AMD X3D driver to cache mode and
     never restores it** — a read-only diagnostic command mutates persistent system state.
  2. A non-root `ananicy-rs start` with `x3d_mode` set changes the mode, then exits 1 without
     restoring it.
  3. The same applies to the singleton-already-running path.
* **Intentional?** No; it is a startup-ordering regression introduced when topology detection moved
  ahead of the action dispatch.
* **Documented?** No.
* **Impact** Persistent CPU-placement change on X3D machines from a command that is supposed to only
  print state. Only reachable when `x3d_mode != auto` **and** the `amd_x3d_vcache` driver is bound.
  Note that the test suite itself is affected: `tests/cli.rs:313,325` writes `x3d_mode=cache` and
  then runs `dump rules`, so `cargo test` on an X3D host would leave the driver in cache mode.

### 5.7 Realtime detection uses a different kernel interface — Severity: Low

* **C++** (`process_info.cpp:107-118`): `sched_getattr(pid)` then `attr.sched_priority > 0`.
* **Rust** (`lib.rs:46-51`): `sched_getscheduler(pid)` and compare against `SCHED_FIFO`/`SCHED_RR`.
* **Difference**: a `SCHED_FIFO`/`SCHED_RR` task with priority **0** is realtime to Rust and not to
  C++; conversely C++ treats any policy reporting `sched_priority > 0` as realtime, Rust does not.
* **Consequence**: the flag gates the cgroup workaround (`worker.rs:252-272`, `412-433`), i.e. whether
  a rule's `cgroup` attribute is skipped and whether the process is pushed to the hierarchy root.
* **Intentional?** Unclear — both are one-line implementations of the same question.
* **Documented?** No.
* **Impact** Very narrow (priority-0 realtime tasks are unusual), but the two answers are
  observably different.

### 5.8 `--verbose` is "force DEBUG" in Rust, "one level more verbose" in C++ — Severity: Low

* **C++** (`src/main.cpp:143-148`): `loglevel = max(0, current - 1) % n_levels`, i.e. a *relative*
  step: `error → warn`, `info → debug`, `trace → trace`.
* **Rust** (`src/startup.rs:17-33`): `verbose ⇒ Level::DEBUG`.
* **Difference**: with `loglevel = trace`, C++ keeps trace while Rust *lowers* verbosity to debug.
  With `loglevel = error`, C++ shows warn+ while Rust shows debug+.
* **Documented?** No (the differences doc covers live reload of the level, not the `-v` step).
* **Impact** Cosmetic/diagnostic, but a user debugging with `-v` gets a different amount of output.

### 5.9 `benchmark-count` reaches the exit much later in C++ — Severity: Low

* **C++** (`src/main.cpp:313-316`): the count is compared in the **main loop**, which sleeps
  `check_freq` (60 s by default) or 30 s in benchmark mode, so the daemon keeps running for at least
  one full interval after the count is reached.
* **Rust** (`worker.rs:133-137`): the worker sets the shutdown flag as soon as the count is reached;
  the monitor notices within 100 ms.
* **Intentional?** Rust's behaviour is the obviously intended one.
* **Documented?** No. **Impact**: benchmark runs are much shorter; scripts that expect a ~60 s run
  will see seconds.

### 5.10 An unknown `action` starts the daemon in C++ and exits in Rust — Severity: Low

* **C++** (`src/main.cpp:208-210`): logs `critical("Unknown action requested: …")` and **falls
  through** to the root check, singleton and the main loop — an unknown action behaves like `start`.
* **Rust** (`src/main.rs:98-101`): `error!` + `exit(1)`.
* **Intentional?** Rust is stricter and the test `test_cli_unknown_action` pins it.
* **Documented?** No. **Impact**: a typo in `ExecStart=ananicy-cpp stop` would, in C++, start the
  daemon; in Rust it exits 1 (and systemd's `Restart=always` would loop). Both behaviours are
  defensible; the difference is undocumented.

### 5.11 Exit codes for `dump` misuse differ — Severity: Low

`ananicy-cpp dump` → exit 1 (`main.cpp:181-183`); `ananicy-rs dump` → exit 2 (bpaf usage error).
`ananicy-cpp dump bogus` → exit 1 (`main.cpp:194-197`); `ananicy-rs dump bogus` → exit 2.
Verified. C++'s own argument-parser errors also exit `EXIT_FAILURE` (1), so `ananicy-cpp`'s whole CLI
uses 1 where `ananicy-rs` uses 2. Not documented.

### 5.12 `cpuset` upper bound is computed differently — Severity: Low

* **C++** (`cpuset.cpp:203-205`): `max_cpus` defaults to `sysconf(_SC_NPROCESSORS_CONF)`.
* **Rust** (`abi/affinity.rs:10-17`): `max(_SC_NPROCESSORS_CONF, 1024)`.
* Consequence: on a 12-CPU host, a rule with `cpuset: "900-901"` is accepted by Rust and rejected by
  C++; conversely nothing that C++ accepts is rejected by Rust. (The Rust value is required because
  `sched_setaffinity` is handed a mask sized from the same constant.)
* **Documented?** No. **Impact**: a malformed cpuset is accepted instead of rejected.

### 5.13 `CPUQuota` is computed from a different CPU count — Severity: Low

* **C++** (`cgroups.cpp:93,123`): `std::thread::hardware_concurrency()` (all online logical CPUs).
* **Rust** (`cgroup/manager.rs:278`): `std::thread::available_parallelism()`, which also honours
  `sched_getaffinity` and cgroup CPU limits, and falls back to `1` on error.
* Consequence: under a CPU-limited unit/container the Rust quota is smaller. The service unit sets no
  `CPUQuota`, so in the packaged deployment both agree.
* **Documented?** No.

### 5.14 `CPUQuota` write order — Severity: Informational

C++ writes `cpu.cfs_quota_us` then `cpu.cfs_period_us` (`cgroups.cpp:95-96`); Rust writes the period
first (`cgroup/manager.rs:297-316`). Both end in the same state; the kernel defaults the period to
100000 so neither ordering can fail spuriously.

### 5.15 `parse_size_string` does not implement the `G` suffix — Severity: Informational

C++ `sysfs.cpp:46-69` handles `K/k`, `M/m`, `G/g`. Rust `topology.rs:139-154` handles only `K/k` and
`M/m`, and `x3d.rs:127-130` strips only `K` and does not even multiply. The values feed only
*relative* comparisons (`max_l3`, V-Cache CCD selection), so the outcome is unchanged on real
hardware; it is nonetheless a divergence in a shared helper, and the two Rust copies disagree with
each other.

### 5.16 `dump proc` adds a `rule` field and `sched` reads `"unknown"` where C++ reads `"normal"` — Severity: Informational

* Rust `ProcessInfo` (`process_info.rs:25-26`) serialises an extra `rule` field, populated in
  `src/dump.rs:63-70`. C++ has no such field. Strictly a superset, undocumented.
* When `sched_getattr` fails, C++ sees a zero-initialised struct → `SCHED_OTHER` → `"normal"`
  (`process_info.cpp:107-105`); Rust returns `"unknown"` (`process_info.rs:90-92`).

### 5.17 BPF-specific divergences — Severity: Informational

* `--verbose` no longer enables libbpf's debug printer (`bpf_program_utils.c:16-26` vs
  `bpf_monitor.rs:42`, which takes no `verbose`).
* The perf buffer is created with the libbpf-rs default page count instead of C++'s 64 pages per CPU
  (`bpf_program_utils.c:76` vs `bpf_monitor.rs:73`), so the BPF backend is somewhat more exposed to
  `PERF_EVENT_ARRAY` overruns under load.
* C++'s BPF callback resolves the process name in the callback (`process.cpp:61-69`); Rust defers it
  to the worker thread (`bpf_monitor.rs:90-103` → `worker.rs:144-151`). The *matched name* is the
  same, but the `exec` event's name is read slightly later, so a rapid `exec` chain can resolve a
  different name. C++ has the same race in principle (it reads the name in the callback too), so this
  is not a regression.
* `--bpf-min-us` default: C++ uses 60 (`main.cpp:236`), Rust leaves the BPF `rodata.min_us` at its
  compiled default of 0. Inert in both, because the rate-limit check is commented out in the shared
  `ananicy_cpp.bpf.c:59-60`.

### 5.18 Netlink receive window — Severity: Informational

C++ sets `SO_RCVTIMEO` to 500 ms (`netlink_program_utils.c:76-79`) and leaves `SO_RCVBUF` at the
kernel default. Rust sets `SO_RCVBUF` to 8 MiB and warns if that fails (`netlink.rs:50-56`), and
uses a 100 ms `epoll` timeout instead of a socket timeout (`netlink.rs:111-114`). Undocumented
improvements; the `epoll` change also makes draining cheaper (no syscall per timeout tick).
C++'s `static pid_t prev_pid` in the event handler survives a reconnect; Rust's `prev_pid` is
re-initialised per `listen()` (`netlink.rs:102`), so one duplicate report can occur after a
reconnect.

### 5.19 `get_cgroup_for_pid` ignores `sd_pid_get_cgroup` — Severity: Informational

With `ENABLE_SYSTEMD=1`, C++ reports the cgroup via `sd_pid_get_cgroup` (`cgroups.cpp:338-346`);
Rust always parses `/proc/<pid>/cgroup` (`src/debug.rs:83-104`). Affects the `debug cgroups` line
only, and only inside a cgroup namespace.

### 5.20 Startup-diagnostic ordering and content — Severity: Informational

* C++ prints the resolved config **before** applying `loglevel` (`main.cpp:139-141`), so the dump is
  always visible at the default level. Rust logs it after the filter is installed
  (`src/startup.rs:110-146`), so `loglevel = error` hides the whole config dump.
* C++ prints its version banner to **stdout** for every invocation including `dump`
  (`main.cpp:167`), so `ananicy-cpp dump rules | jq` is fed a banner line first. Rust keeps stdout
  clean (JSON only) and logs the version at INFO on stderr — strictly better.
* C++'s `check_freq` is re-parsed from the string on every main-loop iteration
  (`config.cpp:129-137`); Rust reads a `u32` from an `arc_swap` snapshot.
* C++'s `Config::show()` prints *all* keys including unknown ones; Rust prints a fixed list of the
  15 keys it knows and warns about the rest.

---

## 6. Validation of `docs/ANANICY_CPP_DIFFERENCES.md`

| § | Claim | Verdict | Notes |
|---|---|---|---|
| §1 | Own binary/unit/env namespaces; identical default config paths | **Accurate** | `src/cli.rs:137`, `data/ananicy-rs.service.in`, `src/startup.rs:72-84` vs `main.cpp:133-136`. **Incomplete**: the IPC object name also changed (`/AnanicyCppMutex` → `/AnanicyRsMutex`, `src/ipc.rs:15`) and is not listed, nor are the singleton permissions (0644 → 0600). |
| §2 | Structured subcommand model; systemd auto-detection | **Accurate** | `src/cli.rs`. **Incomplete**: does not mention the lost `--manualscanning` alias (§4.1) or the exit-code change (§5.11). |
| §3 | Reload via SIGUSR1 instead of 1 s-shm polling; no rule reload | **Accurate** | `src/ipc.rs:63-89`, `src/signals.rs:32-53`; neither daemon reloads `.rules`. |
| §4 | cgroup-v2 single-writer model, delegation, transient-scope veto | **Accurate but incomplete** | `cgroup/ownership.rs`, `cgroup/manager.rs`. Missing: (a) the *path* a rule's cgroup name resolves to (delegated subtree, not the hierarchy root) — a rule written for C++ lands in a different directory; (b) the rule that pre-existing external cgroups referenced by the C++ README can no longer be used; (c) the `nice → cpu.weight` mirror and its blast radius (§5.5). |
| §5 | Rule-load determinism | **Accurate** | `rules.rs:49-57`; pinned by `tests/rules.rs:336-363`. |
| §5 | PID safety around cgroup moves | **Accurate** | `cgroup/manager.rs:206-251`. |
| §5 | BPF → Netlink fallback | **Accurate** | `src/monitor.rs:23-55`. |
| §5 | ENOBUFS recovery | **Accurate** | `src/monitor.rs:76-84`; C++ restarts the listener and re-scans (`process.cpp:176-188`). |
| §5 | **Startup cgroup detection retry loop (~10 s)** | **INACCURATE — the code is never called** | `mounts.rs:18-28` has no callers. See §4.3. |
| §5 | Resilient priority application | **Accurate** | `worker.rs:38-44,307-311`; pinned by `tests/worker_logging.rs`. |
| §5 | Unconditional regex | **Accurate** | `pcre2` is a hard dependency of `ananicy-core`. |
| §5 | Strict success semantics for `log_applied_rule` | **Accurate** | `worker.rs:177-248`; pinned by 14 tests in `tests/worker_logging.rs`. |
| §5 | Live log-level reload; `critical` is an error alias; `fatal` accepted | **Accurate** | `config.rs:19-31,50-60`, `src/signals.rs:36-52`. |
| §5 | Reload scope (`check_freq` needs a restart) | **Accurate** | `src/runtime.rs:102`. |
| §6 | Compatibility table (6 behaviours) | **Accurate** | Each claim is verifiable and each cited test exists. |
| §6 | `0-a` rejected | **Accurate** | C++ `to_int` returns 0 → `0-0`; Rust `cpuset.rs:71-72` rejects. `tests/cpuset.rs`. |
| §6 | Booleans must be spelled `true` | **Accurate** | C++ `check_rule` compares to `"true"`; Rust `value == "true"`. `tests/config.rs`. |

**Summary:** of the 17 entries above, 13 verify exactly, 3 are accurate but incomplete
(§1, §2, §4), and 1 is inaccurate (§5's startup-cgroup-retry claim). No entry misleads in a way
that hides a regression, but the inaccurate entry hides a missing feature.

---

## 7. Superset analysis (Rust-only capabilities with a concrete effect)

| Capability | Where | Concrete effect over C++ |
|---|---|---|
| Deterministic rule-load order | `rules.rs:49-57` | Duplicate rule names resolve identically on every machine. |
| Rule lookup cache (LRU 5000) | `rules.rs:21,183-199` | Removes the linear regex scan from the per-process hot path. |
| PID-reuse-safe cgroup move | `cgroup/manager.rs:206-251` | Cannot move an unrelated process that reused the PID. |
| TGID resolution for v2 `cgroup.procs` | `cgroup/manager.rs:210-219` | Fixes the `EINVAL` that C++ gets for non-leader TIDs. |
| Single-write PID placement | `cgroup/manager.rs:221-233` | C++'s `fstream <<` performs two `write()` calls; the second (`"\n"`) can `EINVAL`. |
| `create_dir_all` for cgroup names | `cgroup/manager.rs:140,161` | Supports `{"cgroup": "a/b"}`, which C++ cannot create. |
| cgroup path-traversal rejection | `cgroup/manager.rs:67-75` | Blocks `{"cgroup": "../../etc"}`. |
| cgroup-namespace escape rejection | `cgroup/process.rs:49-53` | Rejects `/../` cgroup paths from `/proc/<pid>/cgroup`. |
| Single-writer v2 ownership model | `cgroup/ownership.rs`, `manager.rs:129-176` | Prevents fighting systemd over `cgroup.subtree_control`; the reference does this unconditionally and is a known source of bugs (issue #21 in the C++ README). |
| Transient-`.scope` veto | `cgroup/ownership.rs:63-79` | A manual `sudo ananicy-rs` cannot hijack a desktop session's cgroup. |
| BPF → Netlink runtime fallback | `src/monitor.rs:23-55` | A restricted kernel degrades instead of exiting. |
| ENOBUFS recovery | `src/monitor.rs:76-84` | C++ restarts the socket and re-scans; Rust additionally re-scans *before* reconnecting and never terminates. |
| 8 MiB netlink `SO_RCVBUF` | `netlink.rs:50-56` | Materially fewer `ENOBUFS` overruns. |
| epoll-driven drain | `netlink.rs:93-126` | No syscall per idle tick; 100 ms vs 500 ms wake-up. |
| Per-PID `exe`-symlink failure cache | `procfs.rs:15-98` | C++ uses one global counter, so a single unreadable PID disables `/proc/pid/exe` for the whole daemon. |
| Crash-free `check_freq` | `config.rs:145-154` | C++ `std::stoul` throws out of a `noexcept`-adjacent path → terminate. |
| `CPUQuota` type-safety | `runtime.rs:90-93` | C++ `get<unsigned>()` inside `noexcept` → terminate on a malformed value. |
| `stoi` on `nodeN` | `topology.rs:104-117` | C++ `noexcept` + `stoi` → `std::terminate`; Rust returns 0. |
| Live log-level reload | `src/signals.rs:36-52` | `--reload` changes verbosity without a restart. |
| Env/flag config-path overrides | `src/cli.rs:149-154` | C++ is env-only. |
| `--systemd` / `--no-systemd` auto-detection | `src/systemd.rs` | No unit-file coupling; correctly distinguishes "supervised as a service" from "systemd exists". |
| `debug cgroups` systemd status + failure diagnostics | `src/debug.rs:32-79` | More context in the same diagnostic. |
| `dump proc` `rule` field | `src/dump.rs:63-70` | Cross-references the process with the rule that matched. |
| `all` cpuset alias | `topology.rs:57` | C++ has no `all`. |
| `completions` subcommand | `src/cli.rs:128-133` | Shell completion generation. |
| `Delegate=yes` in the packaged unit | `data/ananicy-rs.service.in:16` | Makes the delegated-root model actually available in the default deployment. |
| Rust-only `nice → cpu.weight` mirror | `worker.rs:313-328` | A new *capability*, but with the side effect described in §5.5 — treat as a behavioural difference, not a clean win. |

---

## 8. Feature → test mapping (and the gaps)

296 tests, all passing:

| Target | Tests |
|---|---|
| `ananicy-core` unit | 21 |
| `ananicy-core/tests/config.rs` | 13 |
| `ananicy-core/tests/config_logging.rs` | 9 |
| `ananicy-core/tests/cpuset.rs` | 37 |
| `ananicy-core/tests/property_tests.rs` | 5 |
| `ananicy-core/tests/rules.rs` | 22 |
| `ananicy-core/tests/worker_logging.rs` | 14 |
| `ananicy-core/tests/worker_rules.rs` | 25 |
| `ananicy-platform` unit | 26 |
| `ananicy-platform/tests/affinity.rs` | 6 |
| `ananicy-platform/tests/cgroup_manager.rs` | 14 |
| `ananicy-platform/tests/cgroups.rs` | 7 (root-gated parts skip loudly) |
| `ananicy-platform/tests/mounts.rs` | 9 |
| `ananicy-platform/tests/procfs.rs` | 10 |
| `ananicy-platform/tests/topology.rs` | 14 |
| `ananicy-rs` unit (`src/*.rs`) | 32 |
| `ananicy-rs/tests/cli.rs` | 32 |

### 8.1 C++ capability → Rust verification

| C++ capability | C++ test | Rust verification | Status |
|---|---|---|---|
| cgroup create / add pid / quota | `unit-core.cpp:41-138` | `tests/cgroups.rs` (root), `tests/cgroup_manager.rs` | covered |
| Config parsing (all keys) | `unit-core.cpp:141-179` | `tests/config.rs`, `tests/config_logging.rs` | covered |
| Rule loading/classification/inheritance | `unit-core.cpp:182-278` | `tests/rules.rs` | covered |
| `CpuSet` + parse/serialize | `unit-cpuset.cpp:12-315` | `tests/cpuset.rs` (37, same names) | covered |
| affinity syscall | `unit-cpuset.cpp:318-352` | `tests/affinity.rs` | covered |
| core-count helpers | `unit-cpuset.cpp:354-377` | `tests/procfs.rs` (`usable_core_count`) | covered; `get_num_online_cores` has no Rust counterpart but is unused in C++ |
| X3D driver/mode | `unit-cpuset.cpp:379-418` | `x3d.rs` unit tests with sysfs fixtures | covered (hermetic) |
| topology detect / LLC / NUMA / homogeneous | `unit-topology.cpp:12-100` | `tests/topology.rs` | covered for 8/10; **missing: the 1.3×-threshold branch (§5.2) and `cpus_by_capacity`** |
| capacity source selection | none | **none** | **unverified (§5.1)** |
| argument parser | `unit-utility.cpp:21-96` | `tests/cli.rs` (32) | covered |
| mtab parsing | `unit-utility.cpp:98-151` | `tests/mounts.rs` + fuzz | covered |
| process info | `unit-utility.cpp:153-163` | `process_info.rs` unit tests, `tests/procfs.rs` | covered |
| queue semantics | `unit-utility.cpp:165-218` | n/a (`std::sync::mpsc`) | n/a |
| `get_env`, `read_file`, `to_int` | `unit-utility.cpp:220-275` | indirect | covered |
| worker attribute application | none | `tests/worker_rules.rs` (25) | covered (Rust is stronger here) |
| realtime workaround | none | `tests/worker_rules.rs: a_realtime_process_is_not_moved_into_a_rule_cgroup`, `..._targets_the_hierarchy_root` | covered |
| `cgroup_realtime_workaround` re-detection | none | **none — and the code is inert (§4.2)** | **gap** |
| startup cgroup retry | n/a | **none — code unused (§4.3)** | **gap** |
| ioclass edge cases (`none`, unknown) | none | **none** | **gap (§5.3, §5.4)** |
| `--manualscanning` alias | n/a | **none** | **gap (§4.1)** |
| `dump proc` / `dump autogroup` shape | none | **no test** for any `dump` payload: `tests/cli.rs:299-380` uses `dump rules` only as a vehicle for the startup config report, and nothing asserts the JSON of `rules`/`types`/`cgroups`/`proc`/`autogroup` | **gap** |
| X3D restore on shutdown | none | `monitor.rs::restore_x3d` untested | gap |
| netlink ENOBUFS recovery | none | **none** (needs a real socket) | gap (documented in `docs/TESTING.md` style) |
| BPF backend | none | **none** (the crate does not even build without `libbpf`; the audit environment could not link it) | **gap** — note `cargo test --workspace` fails at `ananicy-bpf`'s build script on hosts without `libbpf`, so `cargo test` in the README/TESTING implies the default feature set only |

### 8.2 Tests whose names could mislead

There is no test file named `*parity*` in the Rust tree (the commit `6258d8b` "refactor(test): organise
the suite by behaviour instead of C++ parity" removed that framing), and
`docs/ANANICY_CPP_DIFFERENCES.md:5` states the suite never runs the C++ binary. That is accurate and
good practice. The risk is the opposite one: several tests are named after C++ test cases
(`test_parse_*`, `test_all_cores_covers_every_online_cpu`, …) and *look* like parity proofs, but they
assert the **Rust** behaviour. Two of them encode a divergence from C++ as if it were the contract
without saying so:

* `crates/ananicy-core/tests/cpuset.rs::test_parse_invalid_range_with_letters` asserts rejection of
  `1-2x`; C++ accepts it as `1-2`. Correct for Rust, but the test name does not flag the difference.
  (It *is* listed in `ANANICY_CPP_DIFFERENCES.md:76-77`, so this is acceptable.)
* `crates/ananicy-platform/tests/topology.rs::test_on_homogeneous_system_all_cores_are_big` pins
  `little == ""` for one homogeneous input while the production code produces `"0-11"` for another
  (§5.2). The test creates the impression of a guarantee it does not provide.

### 8.3 Panic / abort inventory (production paths only)

| Location | Construct | Assessment |
|---|---|---|
| `crates/ananicy-platform/src/netlink.rs:185` | `.expect("Worker thread died")` | **Real risk.** `listen()` runs on the main thread (`src/monitor.rs:76`), so a worker-thread panic panics the daemon. C++'s queue `push` is `noexcept` and cannot fail. Fix: on send failure, set the shutdown flag (as the BPF callback already does at `bpf_monitor.rs:101-103`) and return. |
| `crates/ananicy-platform/src/netlink.rs:206` | `.unwrap_or_else(\|_\| unreachable!())` | Total: `CnMsgBuilder::default().build()` for a fixed `ProcCnMcastOp` cannot fail. |
| `src/cli.rs:225` | `.unwrap()` on `DebugTarget::from_str` | Total: `Err = Infallible` (`cli.rs:84`). |
| `src/cli.rs:237` | `unreachable!()` | Total: bpaf's completion generator always exits. |
| `crates/ananicy-core/src/lib.rs:18` | `.expect("failed to spawn thread")` | Low risk; C++ would `std::terminate` on `std::thread` construction failure too. |
| `crates/ananicy-platform/src/procfs.rs:22` | `.unwrap()` on `NonZeroUsize::new(256)` | Const-true. |
| `crates/ananicy-bpf/build.rs` | `panic!("Unsupported architecture…")` | Matches C++ (which has no `vmlinux.h` for other arches either). |
| `crates/ananicy-bpf/build.rs` | `.expect("Failed to build BPF skeleton…")` | Build-time only. |

Ignored/discarded results worth noting (all intentional, all logged or documented):
`cgroups::create_cgroup`'s return value is discarded at `src/runtime.rs:94`;
`LinuxPlatform::set_affinity` maps every error to `Unsupported` after a `debug!`
(`lib.rs:107-114`); `set_cpu_weight` failures are `debug!`-only by design
(`worker.rs:321-327`, documented in differences §4).

---

## 9. Remediation list (ordered by severity)

| # | Severity | Item | Exact locations |
|---|---|---|---|
| 1 | **High** | `cgroup_realtime_workaround` is inert: make the cgroup manager re-detectable. | `crates/ananicy-platform/src/cgroups.rs:15-22`; `crates/ananicy-platform/src/mounts.rs:10-16`; `src/runtime.rs:73-79` |
| 2 | **High** | Either wire up `mounts::init_cgroups()` at startup **or** delete the claim at `docs/ANANICY_CPP_DIFFERENCES.md:51`. | `crates/ananicy-platform/src/mounts.rs:18-28`; `src/main.rs` (before line 81) / `src/runtime.rs:50` |
| 3 | **High** | Document the blast radius of the `nice → cpu.weight` mirror, and add a configuration key to disable it (or make the mirror opt-in). | `crates/ananicy-core/src/worker.rs:313-328`; `crates/ananicy-platform/src/cgroup/manager.rs:321-364`; `docs/ANANICY_CPP_DIFFERENCES.md:41`; `docs/CONFIGURATION.md:35,100` |
| 4 | **Medium** | Topology: select one capacity source globally (mirroring C++'s "differentiates across CPUs" test) instead of per CPU. | `crates/ananicy-platform/src/topology.rs:244-260` |
| 5 | **Medium** | Topology: in the "highest < lowest × 1.3" branch, set `little_cores_str`/`turbo_cores_str` to `""` (as C++ does), and add a fixture + test for that branch. | `crates/ananicy-platform/src/topology.rs:326-335`; `crates/ananicy-platform/tests/topology.rs`; `docs/TOPOLOGY.md:19-20,29` |
| 6 | **Medium** | `ioclass: "none"` must not call `ioprio_set` with class `NONE`; replicate C++'s `ioprio_valid` guard, or write `IOPRIO_DEFAULT`. | `crates/ananicy-platform/src/priority.rs:106-125`; `crates/ananicy-platform/src/abi/ioprio.rs` |
| 7 | **Medium** | An unknown `ioclass` must not abort the rest of the rule: return `PlatformError::Skipped`. | `crates/ananicy-platform/src/priority.rs:112-114`; `crates/ananicy-core/src/worker.rs:38-44,381-391` |
| 8 | **Medium** | Move X3D mode application after the `dump`/`debug` dispatch and after the root/singleton checks (or restore it on every early-exit path). | `src/main.rs:81,84-116`; `src/startup.rs:176-195` |
| 9 | **Medium** | Restore the `--manualscanning` alias. | `src/cli.rs:162-163`; new test in `tests/cli.rs` |
| 10 | **Medium** | Remove the `CPUWeight` claim, or implement it for `.cgroups` rules. | `docs/CONFIGURATION.md:100`; `src/runtime.rs:88-98` |
| 11 | **Low** | Replace the netlink `.expect("Worker thread died")` with a shutdown-flag set. | `crates/ananicy-platform/src/netlink.rs:183-186` |
| 12 | **Low** | Implement the `apply_ioclass` gate or stop reporting it as an effective value. | `crates/ananicy-core/src/worker.rs:373-391`; `src/startup.rs:129`; `contrib/module.nix:141` |
| 13 | **Low** | Align realtime detection (`sched_attr.sched_priority > 0`) or document the difference. | `crates/ananicy-platform/src/lib.rs:46-51` |
| 14 | **Low** | Document (or align) `--verbose`, the `benchmark-count` exit timing, the unknown-action exit, the `dump` exit codes, the `cpuset` bound, the `CPUQuota` CPU count, the netlink buffer/timeout, the BPF perf-buffer size and the missing BPF verbose. | `src/startup.rs:17-33`; `crates/ananicy-core/src/worker.rs:133-137`; `src/main.rs:98-101`; `src/cli.rs:213-219`; `crates/ananicy-platform/src/abi/affinity.rs:10-17`; `crates/ananicy-platform/src/cgroup/manager.rs:278`; `netlink.rs:50-56`; `bpf_monitor.rs:42,73-108` |
| 15 | **Low** | Document the NixOS `.foo-wrapped` matching rewrite. | `crates/ananicy-core/src/worker.rs:155-171`; `docs/ANANICY_CPP_DIFFERENCES.md` §5 |
| 16 | **Low** | Either finish or delete the dead `cgroup_rules` module (exact/glob/ancestor/`!` cgroup matching is unreachable from the rule engine). | `crates/ananicy-core/src/cgroup_rules.rs`; `crates/ananicy-core/src/lib.rs:2` |
| 17 | **Low** | Add a terminate/backtrace panic hook. | new `src/panics.rs`; `Cargo.toml:75-80` |
| 18 | **Low** | Add `fuzz` targets for the cpuset and rule parsers (parity with the three C++ fuzzers). | `crates/ananicy-platform/fuzz/`, `crates/ananicy-core` |
| 19 | **Low** | Add a test for `dump proc` / `dump autogroup` output shape. | `tests/cli.rs` |
| 20 | **Low** | Add the standalone event-source debug tool, or document that it is not provided. | new `src/bin/` |
| 21 | **Low** | Doc fixes: `docs/CLI.md:110,143` claims `powershell` (unsupported) and omits `elvish`; `docs/CLI.md:102` claims `start` is the default with no command (it is not); `docs/CONFIGURATION.md:69` says `oom_score_adj: [-999..1000]` where the C++ README says `[-999, 999]`. | as listed |
| 22 | **Low** | Add the `G` suffix to `parse_size_string` and make the two Rust copies agree. | `crates/ananicy-platform/src/topology.rs:139-154`; `crates/ananicy-platform/src/x3d.rs:127-130` |
| 23 | **Info** | Note the IPC object name and permissions in §1 of the differences document. | `docs/ANANICY_CPP_DIFFERENCES.md:9-17`; `src/ipc.rs:15,52` |
| 24 | **Info** | Consider a `crt-static` release profile; document the absence otherwise. | `Cargo.toml:75-80` |
| 25 | **Info** | `cargo test` at the workspace root fails without `libbpf` (the `ananicy-bpf` build script links it even though the default feature set excludes it). `docs/TESTING.md` should say `cargo test -p ananicy-rs -p ananicy-core -p ananicy-platform`, or gate the BPF crate out of `default-members`. | `Cargo.toml:18-23`; `crates/ananicy-bpf/build.rs` |

---

## 10. Final answer

> **Can `ananicy-rs` currently be considered a functional superset of `ananicy-cpp`, with all
> non-equivalent behavior either eliminated or explicitly documented in
> `ANANICY_CPP_DIFFERENCES.md`?**

**No.**

The daemon's *functional surface* is essentially complete: every rule attribute, every
configuration key, the whole rule-engine semantics (including the tricky compatibility corners that
the differences document pins), the process-discovery ladder, both event backends, the full
scheduling/affinity/cgroup/priority surface, the topology and X3D machinery, the lifecycle, and the
diagnostics all have Rust counterparts with equivalent observable semantics, and the Rust side adds
a substantial set of genuine improvements (§7). The test suite backing that claim is large, fast,
hermetic, and green (296/296).

It is nevertheless not a *documented* superset, for three independent reasons:

1. **A real regression:** the `--manualscanning` alias is gone (§4.1).
2. **A C++ recovery mechanism that no longer works and is not documented as broken:**
   `cgroup_realtime_workaround` (§4.2).
3. **A difference-document entry that describes code which is never executed:** the startup cgroup
   retry loop (§4.3). An auditor who trusts the document will mark this concern resolved when it is
   not.

On top of that, **17 behavioural differences are undocumented**, two of which can change system
state in ways an operator would not expect: the `little-cores`/`turbo-cores` alias silently
resolving to *all* CPUs on non-heterogeneous machines (§5.2, reproduced on the audit host), and the
`nice → cpu.weight` mirror rewriting a whole cgroup's weight as a side effect of one process's rule
(§5.5). A further three differences are documented but incompletely (§1, §2, §4 of the differences
document), and one documented `.cgroups` capability (`CPUWeight`) does not exist in either
implementation while `docs/CONFIGURATION.md` claims it does.

**Remediation is small and well-localised.** Items 1–3 of §9 are the only ones that change runtime
behaviour; items 4–8 are five localised code changes plus documentation; the remainder are
documentation, test-coverage, and diagnostic gaps. Once §9 items 1–10 are addressed and the
differences document is amended to cover §5, the answer becomes **yes**.

### Confidence

* **High confidence** for every finding that is backed by a source citation above and, where noted,
  by an executed command (`--manualscanning`, the topology probe, the dump/debug exit codes, the
  test suite).
* **Medium confidence** for §5.1 and §5.2's *trigger frequency* on real hardware: the divergence is
  proven on the audit host for §5.2, but I could not enumerate how many shipping kernels expose a
  uniform higher-priority capacity source (§5.1).
* **Lower confidence** for anything requiring a cgroup-v2 delegated-root service: the audit host runs
  the daemon inside a transient `.scope`, so `discover_delegated_root()` returns `None` and the
  *owned* branch of the cgroup manager (`cgroup/manager.rs:146-167`) could only be exercised through
  the simulated hierarchy in `tests/cgroup_manager.rs`, not against a real delegated subtree. The
  `tests/cgroups.rs` root-gated tests skip in this environment.
* **Not assessed:** the BPF backend could not be built (no `libbpf` in this environment:
  `ld.lld: error: unable to find library -lbpf`), so §5.17 rests on source comparison of the two
  `ananicy_cpp.bpf.c` files (byte-identical program logic) and the two loader implementations, not
  on execution.
* **Not assessed:** `ananicy-cpp` has no VCS history in this checkout, so "intentional vs accidental"
  judgements rely on comments, tests and the differences document rather than commit messages.
