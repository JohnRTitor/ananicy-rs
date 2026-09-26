# Contributing to ananicy-rs

First of all, thank you for considering contributing to `ananicy-rs`! This project is a Rust rewrite of the original `ananicy-cpp`, aiming for memory safety, better architecture, and lower resource consumption while staying behaviorally compatible with it.

## Development Environment

`docs/BUILD.md` is the canonical description of what to install and how to build:
the native dependencies and their per-distribution package names, the feature
flags, the `ananicy-bpf` workspace trap, the release profile, and what each
build error means. In short: Linux, Rust 1.88 or newer via
[rustup](https://rustup.rs/), and — if you want the `bpf` feature or intend to
run `--workspace` — `clang`, `libbpf`, PCRE2 and `rustfmt`.

The short version, which is enough for most changes:

```bash
cargo build            # default features, no eBPF toolchain needed
cargo test             # see "Running Tests" below
```

## Building the Project

The project is a Cargo workspace of `ananicy-core`, `ananicy-platform`,
`ananicy-bpf` and the `ananicy-rs` binary. `ananicy-bpf` is a member but not a
default member, so a default build never compiles it and never needs the eBPF
toolchain; `cargo build --workspace` and `--features bpf` do.

```bash
cargo build                        # netlink + systemd
cargo build --features bpf         # eBPF backend
cargo build --release              # optimised, see docs/BUILD.md
```

`cargo build --workspace` builds `ananicy-bpf` and therefore requires `clang`,
libbpf, PCRE2 and `rustfmt` even with no features enabled. See
[docs/BUILD.md](docs/BUILD.md).

## Running Tests

`ananicy-rs` has its own authoritative test suite; `cargo test` validates it
without needing a checkout of the C++ daemon, an installed configuration, a
running service or (in almost all cases) root.

```bash
cargo test --workspace
```

The suite is layered, and each layer is expected to stay independent:

- **Unit tests** live in `#[cfg(test)] mod tests` next to the code they cover.
  Prefer this for private helpers and parsers.
- **Component tests** live in `crates/*/tests/` and are named after the
  behaviour they verify (`rules.rs`, `config.rs`, `cgroup_manager.rs`, …).
  Use dependency injection and temporary directories: `ananicy-core`'s
  `tests/common/mod.rs` provides a recording `PlatformActions` implementation so
  the worker can be driven without touching a process.
- **Property tests** (`crates/ananicy-core/tests/property_tests.rs`) use
  `proptest` for parser invariants.
- **System tests** read the live cgroup hierarchy or call real syscalls. They say
  so in their module documentation, and the ones needing root print a
  `skipping …` line instead of passing silently.
- **Fuzz targets** live in `crates/ananicy-platform/fuzz` (its own workspace)
  and need nightly plus `cargo-fuzz`:
  `cargo +nightly fuzz run parse_mounts`.

`docs/TESTING.md` documents the layout in detail, including which behaviours are
kept compatible with `ananicy-cpp` on purpose.

*Note: some cgroup tests and the BPF build need `root` or `libbpf`. In a normal
development cycle, `cargo test` runs the user-space logic validations.*

## Code Quality and Linting

Before submitting a Pull Request, please ensure your code follows the project's formatting and linting rules.

**1. Formatting**
Ensure your code is formatted using `rustfmt`:
```bash
cargo fmt
```

**2. Linting**
Check for common mistakes and idiomatic Rust using `clippy`:
```bash
cargo clippy --all-targets --all-features -- -D warnings
```
(Please fix any warnings that arise).

## Adding a Feature or Rule

*   **Rules**: If you're contributing new community rules, add them under `ananicy.d/`. (Currently rules are often pulled from the upstream `ananicy` repository, but local testing is encouraged).
*   **Linux Features**: If you are modifying scheduler operations, CPU affinity parsing, or cgroups, ensure that your changes fall back gracefully when privileges are missing or when running on unsupported kernel versions.

## Adding a Test

Put a test where the behaviour it verifies lives:

| Verifying | Put it in |
| --- | --- |
| a private helper or parser | `#[cfg(test)] mod tests` in the same module |
| a public component's behaviour | `crates/<crate>/tests/<component>.rs` |
| the command line | `tests/cli.rs` |
| an invariant over arbitrary input | `tests/property_tests.rs` |
| behaviour against a real kernel or cgroup hierarchy | the system-test file of that component, documented as such |

Name the test after the behaviour, not after its origin ("a rule with a
trailing comment is accepted", not "cpp parity"). A test whose expected value
came from another implementation should say so in a comment and explain why the
behaviour is required; otherwise assert the intended behaviour directly. New
tests must not depend on `/etc/ananicy.d`, on a running daemon, on another test
having run first, or on root unless they are marked as system tests.

## Submitting Changes

1.  Fork the repository and create a new branch for your feature or bug fix.
2.  Write clear, concise commit messages.
3.  Ensure your code builds (`cargo check`), formats correctly (`cargo fmt`), passes linting (`cargo clippy`), and passes tests (`cargo test`).
4.  Open a Pull Request describing your changes and why they are needed.

Thank you for your contributions!
