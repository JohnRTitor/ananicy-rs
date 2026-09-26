# Testing ananicy-rs

`ananicy-rs` has its own authoritative test suite. `cargo test` validates
`ananicy-rs` on its own: no checkout of the C++ daemon, no reference binary, no
rules installed in `/etc/ananicy.d`, no running service and (with two documented
exceptions) no root.

`cargo test` covers the three packages that make up a default build: the binary,
`ananicy-core` and `ananicy-platform`. The `bpf` feature pulls in `ananicy-bpf`,
whose build needs `clang` and `libbpf`; it is a workspace member but not a
default one, so a host without the eBPF toolchain can still run the suite. To
include it, use `cargo test --workspace`.

## Test layers

| Layer | Where | Needs |
| --- | --- | --- |
| Unit | `#[cfg(test)] mod tests` next to the code | nothing |
| Component (integration) | `crates/*/tests/*.rs` | nothing beyond Linux `/proc` |
| Property | `crates/ananicy-core/tests/property_tests.rs` | nothing |
| Fuzz | `crates/ananicy-platform/fuzz/` | nightly + `cargo-fuzz` |
| System | tests that read the live cgroup hierarchy or the real syscalls | Linux, sometimes root |
| Command line | `tests/cli.rs` | builds the binary |

Component tests use dependency injection and temporary directories: the rules
engine is driven through a recording `PlatformActions` implementation
(`crates/ananicy-core/tests/common/mod.rs`), cgroup hierarchies are simulated in
a `tempfile` directory, and CPU topology is read from a checked-in `sysfs`
fixture instead of the host.

## What lives where

### `ananicy-core` — the rules engine, configuration and worker

| File | Verifies |
| --- | --- |
| `src/config.rs` | defaults, log level parsing, writing a default config, live reload |
| `src/cgroup.rs` | the cgroup path type a process is resolved into |
| `tests/config.rs` | `ananicy.conf` syntax, unknown keys, malformed values, missing files |
| `tests/config_logging.rs` | `loglevel` / `log_applied_rule` handling and reload |
| `tests/rules.rs` | rule, type and cgroup loading, matching, directory loading, inheritance, and the `name_regex` contract: the whole corpus of patterns any ananicy rule has ever used, the constructs the engine refuses, and the size and nesting limits |
| `tests/cpuset.rs` | the `cpuset` field: parsing, serialization, bounds |
| `tests/worker_rules.rs` | which platform operations a matched rule triggers |
| `tests/worker_logging.rs` | how the outcome of a rule application is reported |
| `tests/property_tests.rs` | parser invariants under arbitrary input (`proptest`) |

### `ananicy-platform` — the Linux layer

| File | Verifies |
| --- | --- |
| `src/process_info.rs` | `/proc/<pid>/stat` and autogroup parsing |
| `src/service.rs` | deriving the systemd unit name from a cgroup path |
| `src/cgroup/manager.rs`, `src/cgroup/ownership.rs`, `src/cgroup/process.rs` | delegated-root resolution and the ownership rules |
| `tests/procfs.rs` | process discovery: names, start times, thread lists, vanished processes |
| `tests/mounts.rs` | cgroup v1/v2 detection from a mount table |
| `tests/topology.rs` | big/little, LLC and NUMA detection from a `sysfs` fixture |
| `tests/cgroup_manager.rs` | owned vs. foreign cgroup mutations against a simulated hierarchy |
| `tests/cgroups.rs` | **system test**: the cgroup hierarchy of the host |
| `tests/affinity.rs` | **system test**: `sched_setaffinity` on the test process itself |

### `ananicy-rs` — the binary

| File | Verifies |
| --- | --- |
| `src/cli.rs`, `src/startup.rs`, `src/systemd.rs`, `src/debug.rs` | argument parsing, log level resolution, systemd detection, `debug` output |
| `tests/cli.rs` | the process as a user sees it: exit status, stdout, stderr |

## System tests

A handful of tests genuinely need the machine they run on. They are marked in
their module documentation, and the ones that need privileges print a
`skipping …` line so a skip is never mistaken for a pass:

- `crates/ananicy-platform/tests/cgroups.rs` — creating a cgroup and moving a
  process into it needs root. The read-only detection tests always run.
- `crates/ananicy-platform/tests/affinity.rs` — calls the real syscall, but only
  ever on the test process itself, and restores the previous mask.
- `crates/ananicy-platform/tests/mounts.rs` and `tests/procfs.rs` — read the
  host's `/proc`; the synthetic tables and fixtures cover everything that can be
  covered without a host.

## Fuzzing

```bash
cargo +nightly fuzz run parse_mounts
cargo +nightly fuzz run parse_cpuset
cargo +nightly fuzz run parse_rule
```

There is one target per parser that sees operator-written text — the mount table, the cpuset
notation and a rule line. They need nightly and `cargo-fuzz`, and they are the only thing in the
tree that cannot be built with the toolchain the rest of it uses; the invariants they check are
also asserted by `crates/ananicy-core/tests/property_tests.rs`, which do run in CI, so a target
that has never been run does not leave anything unverified.

`crates/ananicy-platform/fuzz` is its own workspace (see its `Cargo.toml`)
because the targets are compiled with sanitizer instrumentation. The fuzzed
parsers have property-based counterparts in
`crates/ananicy-core/tests/property_tests.rs` that run as part of `cargo test`.

## Relationship with `ananicy-cpp`

`ananicy-rs` was started as a rewrite of the C++ daemon, so parts of the test
suite were originally written to check that the two implementations agreed.
Those tests have been re-classified by what they actually verify:

- A test that feeds input to the `ananicy-rs` parser and checks the result is an
  ordinary `ananicy-rs` test, and lives in the component it exercises. This is
  the case for essentially everything that used to be called a "parity" test:
  no `ananicy-cpp` build is needed to run it.
- Behaviour that `ananicy-rs` deliberately inherits from the C++ daemon to stay
  compatible with existing configuration and rule files is still tested, but the
  test states *why* the behaviour is expected. The list of such requirements is
  kept in [COMPATIBILITY.md](./COMPATIBILITY.md).
- Behaviour that `ananicy-rs` intentionally does differently is pinned by an
  ordinary test asserting the `ananicy-rs` behaviour, not by "whatever the other
  implementation does".

There is currently **no** test that executes `ananicy-cpp` as an oracle: the
expected values in the suite are written out as contracts, so the tests stay
meaningful even when the C++ project is unavailable. If a future change needs a
live comparison, it belongs in a separate `reference_tests.rs` target that is not
part of `cargo test`.
