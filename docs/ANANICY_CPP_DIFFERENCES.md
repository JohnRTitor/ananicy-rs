# Behavioral Differences: ananicy-rs vs ananicy-cpp

This document outlines intentional behavioral differences and improvements in `ananicy-rs` compared to the reference `ananicy-cpp` implementation.

## BPF → Netlink Runtime Fallback

- **C++**: The process discovery mechanism is compile-time only (`#if defined(USE_BPF_PROC_IMPL)`). If BPF fails to load at runtime on a system compiled with it, the daemon fails.
- **Rust**: The daemon utilizes a runtime fallback. It attempts to load the eBPF program, and if the kernel lacks support or restricts it, `ananicy-rs` gracefully degrades to the Netlink-based process monitor. It also includes an auto-restart capability if the Netlink connector dies, running a procfs recovery scan.

## Cgroup v2 Delegation and Ownership Model

- **C++**: Performs raw writes to `cgroup.procs` and controller limits without verifying ownership of the subtree. This violates the cgroups v2 single-writer rule if modifying `system.slice` subtrees managed by systemd.
- **Rust**: Implements a strict ownership model. It classifies target cgroup paths as `Legacy` (v1), `Owned` (v2 delegated root), or `Foreign` (v2 systemd-managed). 
  - Structural modifications (like writing to `cpu.max` or `cgroup.subtree_control`) are **refused** on `Foreign` cgroups to prevent breaking systemd's state tracking.
  - Rust also explicitly enables controllers via `cgroup.subtree_control` (`+cpu`) before creating child groups in V2.

## TGID Resolution and PID-Reuse Guard

- **C++**: Writes the raw process ID (PID) to `cgroup.procs`.
- **Rust**: Resolves the Thread Group ID (TGID) via `/proc/[pid]/status` for V2 writes. Furthermore, it reads the process start time (`/proc/[pid]/stat`) before and after the move operation to ensure the PID has not been recycled by the kernel during the operation, preventing accidental moves of innocent processes.

## Cpuset Trailing-Comma Acceptance

- **C++**: Strict parsing. A trailing comma in a cpuset definition (e.g., `0,1,`) causes the entire string to be rejected.
- **Rust**: Lenient parsing. Trailing commas are explicitly ignored, improving user experience when manually writing rules.

## Rule-Directory Load Order Determinism

- **C++**: Relies on the OS filesystem iteration order (`readdir`), which is non-deterministic. If multiple rules share the same name, the "winner" depends on file system layout.
- **Rust**: Explicitly sorts files alphabetically before loading. This guarantees deterministic rule application across different machines and deployments.

## Worker Error-Path Control Flow (Realtime Workaround)

- **C++**: If `apply_rule` fails for any reason (including Permission Denied), the worker `continue`s immediately and skips the rest of the loop.
- **Rust**: If `apply_rule` fails with a `PermissionDenied` error, the worker logs the failure but *falls through* to execute the realtime cgroup workaround. This ensures that even if standard priority changes are rejected by the kernel, the fallback logic is still evaluated.

## Execution Inside Transient Scopes (e.g., from a terminal)

When `ananicy-rs` is executed manually from a terminal using `sudo`, systemd places the shell (and therefore the daemon) inside a transient `.scope` cgroup (like `session-2.scope`).
- **Rust**: The daemon detects this by parsing `/proc/self/cgroup` and explicitly rejects using a `.scope` as a delegated root, because scopes are strictly managed by `systemd-logind`. As a result, **all cgroup structural mutations are silently disabled** when running manually.
- **Note for testing**: To properly test cgroup v2 functionality, `ananicy-rs` MUST be run as a systemd `.service` with `Delegate=yes` configured. Running from a terminal will intentionally not work for cgroup management.
