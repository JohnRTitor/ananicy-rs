# ananicy-rs vs ananicy-cpp: User-Facing Differences

While `ananicy-rs` aims for high behavioral compatibility with the reference `ananicy-cpp` implementation, there are several intentional differences you should be aware of when installing, configuring, or running the daemon. This document highlights the changes that affect end users.

Where a difference here is a compatibility requirement — behaviour `ananicy-rs` keeps on purpose so that existing configuration and rule files keep working — it is pinned by a test in the component that owns the behaviour. See [TESTING.md](./TESTING.md) for the test layout; the test suite itself never runs `ananicy-cpp`.

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
- `--manual-scanning` and `--manualscanning` both enable manual scanning, exactly as in `ananicy-cpp`.
- `ananicy-rs` decides by itself whether to integrate with a service manager
  (journald logging plus `sd_notify`). `--systemd` forces that behaviour on and
  `--no-systemd` forces it off, so unit files and packaging do not need to pass
  anything. See the [systemd Reference](./SYSTEMD.md).


## 3. Reload Mechanism (`--reload`)

- **ananicy-cpp:** Uses a cooperative polling mechanism via shared memory. When you trigger a reload, the daemon checks for the signal every second, which can cause a slight delay.
- **ananicy-rs:** Uses an OS-level real-time signal (`SIGUSR1`). Configuration reloads happen **instantaneously**, independent of the main loop's scheduling. Neither implementation reloads `.rules`, `.types`, or `.cgroups` files during this operation; those files require a restart.

## 4. Cgroup v2 Delegation and Ownership

`ananicy-rs` follows the cgroup-v2 single-writer model and distinguishes structural ownership from optional resource tuning:

- **Delegated structural ownership:** The packaged systemd unit sets `Delegate=yes`. Within the service's delegated subtree, `ananicy-rs` may create cgroups, enable supported controllers, move processes, and apply `CPUQuota`/`CPUWeight`.
- **Foreign structural protection:** It refuses to create foreign cgroups, enable foreign `cgroup.subtree_control`, write foreign `cpu.max`, or move processes into foreign cgroups.
- **Optional resource tuning:** It may attempt an already-existing `cpu.weight` or `cpu.shares` write in a foreign cgroup when the kernel exposes that controller. It does not create the file or enable the controller; an absent file is an expected DEBUG-level skip.
- **`nice` mirror scope:** On cgroup v2, an applied `nice` value is mirrored into the `cpu.weight` of the cgroup the process *already* belongs to. That is a cgroup write, so it also reweights the other tasks sharing that cgroup — for a desktop application typically its whole systemd session scope. `ananicy-cpp` only calls `setpriority(2)` and has no such side effect. Set `apply_cpu_weight = false` in `ananicy.conf` to keep the `nice` value and drop the mirror. See [Configuration and Rules](./CONFIGURATION.md#the-nice--cpuweight-mirror).
- **Scope of `Delegate=yes`:** Delegation applies only to the `ananicy-rs.service` subtree, not `user.slice`, desktop session scopes, or other systemd units.
- **Transient scopes:** Running manually from a terminal leaves the daemon in a systemd-managed transient scope, so it cannot safely perform delegated structural mutations.
- **Where a rule's cgroup lands:** A rule's `cgroup` name is resolved relative to the delegated subtree, so `{"cgroup": "cpu80"}` creates `/sys/fs/cgroup/system.slice/ananicy-rs.service/cpu80` where `ananicy-cpp` creates `/sys/fs/cgroup/cpu80`. A name that starts with `/` is resolved from the hierarchy root instead. The consequence for a rule set written for the reference is that a `cgroup` naming a cgroup somebody else manages — the reference's README suggests that cgroups "can be any cgroup, including those created outside ananicy-cpp" — is refused rather than used.

## 5. Process and Rule Handling Improvements

- **Rule Load Determinism:** If multiple rule files have the same name, `ananicy-cpp` relies on the OS filesystem iteration order (which is non-deterministic). `ananicy-rs` explicitly sorts files alphabetically before loading, guaranteeing deterministic rule application across different machines.
- **Process ID (PID) Safety:** When moving processes into cgroups, `ananicy-rs` checks the process start time before and after the operation. This prevents accidentally moving an innocent process if the kernel has recycled the PID during the operation.
- **BPF to Netlink Fallback:** If `ananicy-rs` is compiled with eBPF support but the kernel restricts it or fails to load it at runtime, the daemon will gracefully degrade to the Netlink-based process monitor instead of crashing.
- **Netlink Overrun Recovery:** If the Netlink listener is overwhelmed and drops events (`ENOBUFS`), `ananicy-cpp` will exit. `ananicy-rs` recovers automatically by falling back to a full `procfs` scan, waiting briefly, and reconnecting without terminating the daemon.
- **Startup Cgroup Detection:** On systems where cgroup filesystems mount slightly after the daemon starts during early boot, `ananicy-cpp` may fail to detect cgroups. `ananicy-rs` implements a retry loop (up to ~10 seconds) to wait for cgroup mounts to become available before giving up.
- **Resilient Priority Application:** If standard process priority adjustments (like `nice` values) are rejected by the kernel (e.g., due to Permission Denied), `ananicy-cpp` aborts applying the rule entirely. `ananicy-rs` will log the failure but continue executing, ensuring that the `cgroup_realtime_workaround` (if enabled in your config) is still applied.
- **Unconditional Regex Support:** While `ananicy-cpp` treats regex support as an optional compile-time dependency, `ananicy-rs` includes it unconditionally by default, as the Rust regex engine is lightweight and avoids the complexities of maintaining a separate feature-gated build variant.
- **NixOS wrapper executables:** A process whose name starts with `.` and ends with `-wrapped` (how NixOS names the real binary of a shell-script wrapper) is looked up without the leading `.` and the `-wrapped` suffix, so the rule written for the program inside the wrapper applies. `ananicy-cpp` matches the literal name and finds nothing.
- **Strict Success Semantics:** When `log_applied_rule` is enabled, `ananicy-cpp` logs the rule application message *before* applying the rule attributes. If the application fails, a false positive success message remains in the log. `ananicy-rs` intentionally emits the message only after all enabled rule attributes complete successfully; partial, skipped, and failed applications are reported separately. Rust also keeps the opt-in applied-rule event independent of its separate debug rule-match event.
- **Live Log-Level Reload:** `ananicy-rs` applies a reloaded `loglevel` to the active filter, while `ananicy-cpp` retains its original process-wide level. Because Rust uses `tracing`, its supported `critical` configuration value is an error-threshold alias rather than a distinct emitted severity; Rust also accepts case-insensitive names and the legacy `fatal` alias.
- **Reload Scope:** Both daemons reload global configuration values, but neither reloads rule files. In `ananicy-rs`, `check_freq` is captured by the manual scanner thread, so changing it also requires a restart; the other per-event apply flags and `log_applied_rule` are read from the current snapshot.
- **`CPUWeight`:** `ananicy-cpp` reads only `CPUQuota` from a `.cgroups` rule and says so; `ananicy-rs` also honours `CPUWeight`, which its configuration reference has documented all along.
- **`apply_cpu_weight`:** the only key in `ananicy.conf` with no counterpart in `ananicy-cpp`. It gates the `nice` → `cpu.weight` mirror described in §4, and defaults to the behaviour the daemon had before it existed, so no existing configuration changes.
- **`check_disks_schedulers`:** `ananicy-cpp` has no start-up check for block devices on a scheduler that cannot honour `ioclass`/`ionice`, although the key is in its own `test-readfile.txt` fixture and it documents the CFQ/BFQ requirement. Restored from the original Ananicy, where it shipped enabled; read-only, defaults to on.
- **`--verbose`:** `ananicy-cpp` makes `--verbose` one step more verbose than the configured `loglevel` (`error` becomes `warn`). `ananicy-rs` treats it as "at least debug", which is the same in every configuration except `trace`, where the reference stays at `trace` and this daemon drops to `debug`.
- **`--benchmark-count`:** `ananicy-cpp` compares the count in its main loop, so it keeps running for a whole `check_freq` interval (a minute by default) after reaching it. `ananicy-rs` stops as soon as the worker reaches it, which is a few milliseconds later.
- **An unknown action:** `ananicy-cpp` logs `Unknown action requested` and then starts the daemon anyway. `ananicy-rs` exits 1.
- **Exit codes for a misused `dump`:** `ananicy-cpp` exits 1 for a missing or unknown sub-action; `ananicy-rs` exits 2, the code it uses for every usage error. `--reload` and `--force-remove-semaphore` exit 1 in both.
- **The cpuset CPU bound:** a rule's `cpuset` is validated against at least 1024 CPUs, so a CPU index above the configured count is accepted as long as it is below 1024. `ananicy-cpp` validates against the configured count alone (`sysconf(_SC_NPROCESSORS_CONF)`), which is also what it parses a cpuset with.
- **`CPUQuota` arithmetic:** both compute the quota as a period times the number of CPUs times the percentage. `ananicy-cpp` uses the total number of logical CPUs, `ananicy-rs` the number the kernel says this process may run on at once, which is smaller under a cgroup CPU limit. The packaged unit sets no CPU limit, so the two agree there.
- **Netlink receive buffer:** `ananicy-rs` asks for 8 MiB and falls back silently if the kernel refuses, and it drains the socket through `epoll` with a 100 ms tick where `ananicy-cpp` uses a 500 ms `SO_RCVTIMEO`. Fewer overruns, at the cost of a slightly more active loop. After a reconnect the reference keeps its "same pid as last time" filter, this daemon starts over, so one process can be reported twice.
- **`dump proc`:** the entries carry one field the reference does not have, `rule`, naming the rule that matched the process. An entry whose policy could not be read says `sched: "unknown"` where the reference reports the zeroed `normal`.
- **`debug cgroups`:** reads the process' cgroup from `/proc/<pid>/cgroup` in both. With systemd support compiled in, `ananicy-cpp` asks `sd_pid_get_cgroup` instead, which answers the same thing except inside a cgroup namespace.
- **The single-instance lock:** `ananicy-cpp` uses the POSIX shared-memory object `/AnanicyCppMutex` and this daemon uses `/AnanicyRsMutex`, so the two can run side by side — as intended — and the object this daemon creates is mode `0600` where the reference uses `0644`. A `--force-remove-semaphore` from one does not release the other.
- **Developer tools:** `ananicy-cpp` builds two extra binaries, `runqslower_cpp` and `netlink_proc_cpp`, which print raw event-source output. They are not installed by its `cmake --install` and `ananicy-rs` does not build them; the equivalent check is `loglevel = trace`, which logs the name and PID of every process a rule matched.

## 6. Compatibility Requirements Kept on Purpose

The following behaviours are *not* differences — `ananicy-rs` reproduces them so
that configuration files and rule sets written for the C++ daemon keep working.
They are part of the contract, not accidents, and each is pinned by a test:

| Behaviour | Test |
| --- | --- |
| A rule line may be followed by a `#` comment, and CRLF files are accepted. | `ananicy-core/tests/rules.rs` |
| A rule is the text between the first `{` and the last `}` of the line. | `ananicy-core/tests/rules.rs` |
| `name_regex` accepts PCRE2 syntax, including lookarounds. | `ananicy-core/tests/rules.rs`, `tests/worker_rules.rs` |
| The configuration key for cgroup application is `apply_cgroup`. | `ananicy-core/tests/config.rs` |
| `loglevel` accepts `critical` and the legacy `fatal` alias, case-insensitively. | `ananicy-core/src/config.rs` |
| Rules are read from `*.rules`, `*.types` and `*.cgroups`; other extensions are ignored. | `ananicy-core/tests/rules.rs` |
| `apply_ioclass` is accepted, reported, and inert: a rule's `ioclass` is gated by `apply_ionice` alone, as in `ananicy-cpp`. | `ananicy-core/tests/worker_rules.rs` |
| One capacity source is chosen for the whole machine, the first that both reports a value and tells two CPUs apart. | `ananicy-platform/tests/topology.rs` |
| `ioclass: "none"` writes nothing: the class is a reading, not a value. | `ananicy-platform/tests/ioprio.rs` |
| An unrecognised `ioclass` is dropped and the rest of the rule still applies. | `ananicy-core/tests/worker_rules.rs` |
| A task counts as realtime by its static priority, not by its policy. | `ananicy-platform/tests/procfs.rs` |
| stdout carries the answer, stderr carries the log, so `dump` output parses. | `tests/cli.rs` |

Two historical leniencies were deliberately *not* reproduced, because accepting
malformed input silently is worse than rejecting it:

- `0-a` is rejected instead of being read as the range `0-0`
  (`ananicy-core/tests/cpuset.rs`).
- Booleans must be spelled `true`; `1`, `yes` and `True` are false
  (`ananicy-core/tests/config.rs`).
