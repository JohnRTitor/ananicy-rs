# Project Rules — ananicy-rs

`ananicy-rs` is a Linux daemon that applies nice/ionice/scheduling/cgroup/affinity
settings to processes matched against rules. The workspace has a root binary,
`ananicy-core` (rules, worker, types), `ananicy-platform` (procfs, cgroup, netlink,
ABI), and `ananicy-bpf` (optional `bpf` feature; a workspace member but **not** a
default member because its build script needs libbpf). Keep changes inside these
boundaries; core must stay independent of the Linux-specific crate.

## Architecture & threading

- Event sources (`netlink.rs`, `bpf_monitor.rs`) push `Process` values through one
  `mpsc` channel; a single `Worker` thread consumes them (`work_loop` →
  `apply_rule`). There is no cross-thread mutable cache in the worker — state kept
  between events (e.g. the parsed-cpuset map) belongs to `work_loop` as a local.
- `PlatformActions` is the seam for testing the worker. It has two implementors:
  `LinuxPlatform` and the `FakePlatform` in `tests/common/mod.rs`. A signature
  change means updating both plus their tests.
- The per-process path is **syscall-bound**: ~31 µs total, of which four procfs
  reads dominate; rule lookup is ~0.2%. The lever is *fewer reads*, not cheaper
  work around them. Do not spend effort micro-optimising rule matching while a
  redundant `/proc/<pid>/…` read remains. See `docs/MEMORY.md` § "Where the time
  goes".
- PID reuse is a real correctness boundary. Any new cache keyed by pid must also
  carry `get_start_time`; existing guards are in `cgroup/process.rs`,
  `procfs.rs` (`EXE_FAIL_CACHE`), and `netlink.rs` (`ReportedNames`). Never widen
  the recycled-pid window for a speed gain.
- Two `#[repr(C)]` structs are kernel ABIs and must not be reordered or packed:
  `abi/sched_attr.rs::sched_attr` and `bpf_monitor.rs::Event` (mirrored by
  `struct event` in `bpf/ananicy_cpp.bpf.c`).
- `cgroup.procs` requires **one** `write()` of `"pid\n"`; a split newline is
  EINVAL. Keep using `control_line`/`write_control_line` in `cgroup/manager.rs`.
- Release profile uses `panic = "abort"`: no new `unwrap`, `expect`, unchecked
  index, or arithmetic that can overflow on event-driven input. Test and bench
  profiles unwind, so a bug can hide there.

## Memory & allocation discipline

Allocation work is measured, not guessed. `docs/MEMORY.md` holds the published
numbers; re-derive them before claiming an improvement:

- `cargo bench --bench alloc` — counting `#[global_allocator]`; live bytes and
  live allocations per rule. This is the only `GlobalAlloc` in the tree.
- `cargo bench --bench procfs` — the per-process reads (the real budget).
- `cargo bench --bench rules` — lookup, regex, and cpuset parse timing.

**Do not add a custom global allocator, arena, jemalloc or mimalloc.** The
allocation *rate* is the problem, not the allocator; the workload is
syscall-bound and the bench harnesses install their own allocator.

Patterns that apply to this project (from the Cloudflare case studies and the
Rust Performance Book):

- **Immutable data → `Box<str>` / `Box<[T]>`.** Map keys (`RuleName`, `TypeName`,
  `CgroupName`) and `Matcher.name` are already `Box<str>`; `CpuSet.cores` is
  `Box<[u64]>`. A `String` pays a capacity word nothing ever uses.
- **Intern repeated names.** `rules.rs::NAME_TABLE` stores attribute/type names
  as `u16` indices; it is capped at 65,533 entries and deliberately does **not**
  hold rule names. Widening it to `u32` breaks the 72-byte `Rule` pin — treat that
  as a redesign, not a tweak.
- **Precompute derived data.** `CpuSet` maintains its kernel mask incrementally
  so `kernel_mask()` lends `&[u8]`; parsed cpusets are cached per *resolved*
  string in `Worker`. Both fix work that used to repeat per process.
- **Keep small buffers on the stack.** `procfs::proc_path` writes `/proc/<pid>/…`
  into a 64-byte buffer; `cgroup/manager.rs::control_line` renders control values
  into a 48-byte buffer. `Tids = SmallVec<[i32; 8]>` keeps the common thread list
  off the heap and is defined once in `ananicy-core::worker` — use the alias, not
  a bare `SmallVec` type, in new signatures.
- **Read each source once.** `memstats::Snapshot::read()` parses one status
  buffer and one `memory.stat` buffer rather than re-opening them per field.
- **Keep the hot path borrow-based.** `Borrow<str>` on the name newtypes and the
  `Box<str>` cache keys make `HashMap::get(&str)` allocation-free. The resolved
  rule cache is keyed by `Box<str>` and bounded by `MAX_CACHEABLE_NAME`.

Pin layout and invariants with tests: `size_of::<Rule>() == 72`,
`size_of::<Identity>() == 16`, the cpuset mask width/coverage properties, and
the pattern from `docs/MEMORY.md` that universal guards must be *properties*, not
lists of examples. Never weaken or delete an assertion to make a change pass; add
a test that fails on revert.

## Code style

- Use `let-else` and `if let … && let …` chains to flatten control flow; the
  codebase targets a recent Rust (floor 1.88 for let chains).
- `tracing` for all logging; include the reason in `debug!`/`warn!` messages.
  Keep CLI/config/log text stable — `tests/cli.rs` pins it exactly.
- Use `foldhash::quality` for keys an unprivileged process chooses (process
  names); pid-keyed caches use it too because the keys are 4-byte integers. Do
  not switch to `foldhash::fast`.
- Document non-obvious allocation/syscall costs in comments beside the code,
  following the style in `docs/MEMORY.md` and `benches/*`.
- Format with `cargo fmt --all`; the repo is checked with `cargo fmt --all --check`.

## Testing & validation

Real commands (from `docs/TESTING.md`, `docs/BUILD.md`, and CI):

```sh
cargo fmt --all --check
cargo clippy --locked --tests -- -D warnings
cargo test --no-fail-fast
cargo bench --bench alloc
cargo bench --bench procfs
cargo bench --bench rules
# BPF-enabled equivalents (need clang, libbpf, rustfmt):
nix develop --command cargo clippy --locked --workspace --all-features --tests -- -D warnings
nix develop --command cargo test --locked --all-targets --all-features
```

- `cargo test` covers the three default members; only `--workspace` /
  `--all-features` builds `ananicy-bpf`.
- System tests that need root or namespaces skip when unavailable. A skip is not
  a pass; check the skip output before drawing conclusions.
- Changes to memory layout, cgroup writes, or the per-process path need both a
  behavioural test and a re-run of the relevant bench; update `docs/MEMORY.md`
  with the new numbers and keep its existing measured tables as history.

## Environment & tooling

- If `nix` is present, `nix shell` or `nix run` can be used to get the packages needed during agent session. Additionally, `nix develop` provides clang, libbpf, rustfmt and clippy. If there is a build issue due to missing libc headers — that is an environment issue, not a code one, use `nix develop`.