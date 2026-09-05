# ananicy-rs Differences from the Reference Implementation

## 1. Purpose

This document explicitly records meaningful implementation, runtime, and CLI differences between `ananicy-rs` and the `ananicy-cpp` reference implementation.

The goal of `ananicy-rs` is to maintain behavioral parity with the reference implementation while leveraging Rust for memory safety and improved daemon architecture. However, due to language characteristics, safety guarantees, or architectural improvements, a few differences exist.

## 2. Behavioral Differences

| Area | Reference behavior | ananicy-rs behavior | Reason | Impact |
| ---- | ------------------ | ------------------- | ------ | ------ |
| Cgroup V2 limitations | "Limited support" | Retains the exact same limitations as the reference implementation. | Parity with reference. | Cannot arbitrarily mutate foreign Cgroups v2 safely. Requires systemd delegation. |
| Process Events | C++ custom epoll/netlink loop | Uses Netlink or eBPF (via `libbpf-rs`). | Architectural choice for safety and maintainability. | None; should perform identically or better. |

## 3. CLI Differences

The `ananicy-rs` binary has a few differences in its CLI interface compared to the reference implementation.

- **Configuration Flags:** Instead of relying entirely on environment variables, `ananicy-rs` adds explicit `--config <PATH>` and `--config-dir <PATH>` flags for better UX. The environment variables have been renamed to `ANANICY_RS_CONF` and `ANANICY_RS_CONFDIR` (replacing the `ANANICY_CPP_*` prefix).

## 4. Configuration Differences

There are no significant differences in the configuration parsing logic. `ananicy-rs` implements identical logic for `.rules`, `.types`, and `.cgroups` files, including the handling of fallbacks, trailing commas, and default overrides.

All named CPU topology aliases (like `big-cores`, `x3d-cache`, etc.) behave identically.

## 5. Runtime Differences

The core daemon lifecycle is very similar:
- Both implement `sd_notify` for systemd integration.
- Both use an IPC semaphore for `--reload` logic.

## 6. Build Differences

- **Build System:** `ananicy-rs` uses `cargo` instead of `cmake`.
- **BPF Dependencies:** Building with BPF (`--features bpf`) requires `cargo` to invoke `clang` via a `build.rs` script, utilizing `libbpf-rs` and `libbpf-sys`.
- **Packaging:** `ananicy-rs` does not ship with `.spec` files or AUR helpers in the source tree; instead it provides a Nix flake (`flake.nix`) alongside the standard `cargo` workflows.

## 7. Known Non-Parity Areas

Currently, there are no known areas where `ananicy-rs` intentionally lacks parity with the core feature set of the reference implementation. If any deviation is discovered in process matching, rules loading, or topology detection, it is considered a bug.
