# ananicy-rs vs ananicy-cpp: User-Facing Differences

While `ananicy-rs` aims for high behavioral parity with the reference `ananicy-cpp` implementation, there are several intentional differences you should be aware of when installing, configuring, or running the daemon. This document highlights the changes that affect end users.

## 1. Project Identity and Configuration

To allow both implementations to coexist on the same system without colliding, `ananicy-rs` uses its own namespaces for binaries, services, and environment variables:

- **Binary Name:** `ananicy-rs` (instead of `ananicy-cpp`)
- **Systemd Unit:** `ananicy-rs.service` (instead of `ananicy-cpp.service`)
- **Environment Variables:** 
  - `ANANICY_RS_CONF` overrides the default config file path (defaults to `/etc/ananicy.d/ananicy.conf`).
  - `ANANICY_RS_CONFDIR` overrides the default config directory (defaults to `/etc/ananicy.d`).

*(Note: The default paths for the configuration files themselves remain exactly the same as in `ananicy-cpp`.)*

## 2. Command Line Interface (CLI)

`ananicy-rs` uses a more structured subcommand model for its CLI arguments rather than positional strings:
- Use `ananicy-rs start` instead of `ananicy-cpp start`.
- Use `ananicy-rs dump <rules|types|cgroups|proc|autogroup>` instead of `ananicy-cpp dump <target>`.


## 3. Reload Mechanism (`--reload`)

- **ananicy-cpp:** Uses a cooperative polling mechanism via shared memory. When you trigger a reload, the daemon checks for the signal every second, which can cause a slight delay.
- **ananicy-rs:** Uses an OS-level real-time signal (`SIGUSR1`). Configuration reloads happen **instantaneously**, independent of the main loop's scheduling. Neither implementation reloads `.rules`, `.types`, or `.cgroups` files during this operation; those files require a restart.

## 4. Cgroup v2 Delegation and Ownership

`ananicy-rs` follows the cgroup-v2 single-writer model and distinguishes structural ownership from optional resource tuning:

- **Delegated structural ownership:** The packaged systemd unit sets `Delegate=yes`. Within the service's delegated subtree, `ananicy-rs` may create cgroups, enable supported controllers, move processes, and apply `CPUQuota`/`CPUWeight`.
- **Foreign structural protection:** It refuses to create foreign cgroups, enable foreign `cgroup.subtree_control`, write foreign `cpu.max`, or move processes into foreign cgroups.
- **Optional resource tuning:** It may attempt an already-existing `cpu.weight` or `cpu.shares` write in a foreign cgroup when the kernel exposes that controller. It does not create the file or enable the controller; an absent file is an expected DEBUG-level skip.
- **Scope of `Delegate=yes`:** Delegation applies only to the `ananicy-rs.service` subtree, not `user.slice`, desktop session scopes, or other systemd units.
- **Transient scopes:** Running manually from a terminal leaves the daemon in a systemd-managed transient scope, so it cannot safely perform delegated structural mutations.

## 5. Process and Rule Handling Improvements

- **Rule Load Determinism:** If multiple rule files have the same name, `ananicy-cpp` relies on the OS filesystem iteration order (which is non-deterministic). `ananicy-rs` explicitly sorts files alphabetically before loading, guaranteeing deterministic rule application across different machines.
- **Process ID (PID) Safety:** When moving processes into cgroups, `ananicy-rs` checks the process start time before and after the operation. This prevents accidentally moving an innocent process if the kernel has recycled the PID during the operation.
- **BPF to Netlink Fallback:** If `ananicy-rs` is compiled with eBPF support but the kernel restricts it or fails to load it at runtime, the daemon will gracefully degrade to the Netlink-based process monitor instead of crashing.
- **Netlink Overrun Recovery:** If the Netlink listener is overwhelmed and drops events (`ENOBUFS`), `ananicy-cpp` will exit. `ananicy-rs` recovers automatically by falling back to a full `procfs` scan, waiting briefly, and reconnecting without terminating the daemon.
- **Startup Cgroup Detection:** On systems where cgroup filesystems mount slightly after the daemon starts during early boot, `ananicy-cpp` may fail to detect cgroups. `ananicy-rs` implements a retry loop (up to ~10 seconds) to wait for cgroup mounts to become available before giving up.
- **Resilient Priority Application:** If standard process priority adjustments (like `nice` values) are rejected by the kernel (e.g., due to Permission Denied), `ananicy-cpp` aborts applying the rule entirely. `ananicy-rs` will log the failure but continue executing, ensuring that the `cgroup_realtime_workaround` (if enabled in your config) is still applied.
- **Unconditional Regex Support:** While `ananicy-cpp` treats regex support as an optional compile-time dependency, `ananicy-rs` includes it unconditionally by default, as the Rust regex engine is lightweight and avoids the complexities of maintaining a separate feature-gated build variant.
- **Strict Success Semantics:** When `log_applied_rule` is enabled, `ananicy-cpp` logs the rule application message *before* applying the rule attributes. If the application fails, a false positive success message remains in the log. `ananicy-rs` intentionally emits the message only after all enabled rule attributes complete successfully; partial, skipped, and failed applications are reported separately. Rust also keeps the opt-in applied-rule event independent of its separate debug rule-match event.
- **Live Log-Level Reload:** `ananicy-rs` applies a reloaded `loglevel` to the active filter, while `ananicy-cpp` retains its original process-wide level. Because Rust uses `tracing`, its supported `critical` configuration value is an error-threshold alias rather than a distinct emitted severity; Rust also accepts case-insensitive names and the legacy `fatal` alias.
- **Reload Scope:** Both daemons reload global configuration values, but neither reloads rule files. In `ananicy-rs`, `check_freq` is captured by the manual scanner thread, so changing it also requires a restart; the other per-event apply flags and `log_applied_rule` are read from the current snapshot.
