# Configuration and Rules

`ananicy-rs` dynamically applies performance tweaks based on rule files. The configuration behavior mirrors the upstream `ananicy-cpp` and `ananicy` projects.

By default, `ananicy-rs` looks for configuration and rules in `/etc/ananicy.d/`. This path can be overridden with the `--config-dir` CLI option or the `ANANICY_RS_CONFDIR` environment variable.

*(Note: `ananicy-rs` does not ship with rules by default. You should copy community rules from the original `ananicy` or `ananicy-cpp` project into `/etc/ananicy.d/`)*.

## Global Configuration (`ananicy.conf`)

Global settings are defined in `/etc/ananicy.d/ananicy.conf`. The path can be explicitly set via the `--config` CLI option or the `ANANICY_RS_CONF` environment variable.

The format is `key=value`, one per line.

| Option | Default | Description |
|--------|---------|-------------|
| `check_freq` | `60` | Full process scan interval in seconds (used during manual scanning) |
| `apply_nice` | `true` | Apply nice values from rules |
| `apply_sched` | `true` | Apply scheduling policy from rules |
| `apply_ionice` | `true` | Apply I/O nice values from rules |
| `apply_oom_score_adj` | `true` | Apply OOM score adjustments from rules |
| `apply_latnice` | `true` | Apply latency nice values from rules |
| `apply_cpuset` | `true` | Apply CPU affinity (cpuset) from rules |
| `apply_cgroup` | `true` | Apply cgroup membership from rules |
| `cgroup_load` | `true` | Load cgroup definitions (`.cgroups` files) |
| `type_load` | `true` | Load type definitions (`.types` files) |
| `rule_load` | `true` | Load rule definitions (`.rules` files) |
| `cgroup_realtime_workaround` | `true` | Enable cgroup realtime workaround |
| `log_applied_rule` | `false` | Emit an INFO event after a matching rule is applied successfully |
| `loglevel` | `info` | Minimum log level (`trace`, `debug`, `info`, `warn`, `error`, `critical`) |
| `x3d_mode` | `auto` | AMD X3D driver mode: `auto` (don't touch), `cache`, or `frequency` |

### Applied-rule logging

`log_applied_rule` is opt-in. When it is `true`, the daemon emits one `INFO` event for each task whose matching rule has at least one enabled attribute and whose application completes without an error or a skipped attribute. The event includes the task name, PID, and matched rule. On cgroup v2, mirroring `nice` to the optional `cpu.weight` controller is best-effort; if that controller is unavailable, a successful `nice` application is still reported and the mirror failure is available at DEBUG level.

The event is intentionally not emitted for a rule match with no enabled applicable attributes, a partial application, or a failed application. A process can produce more than one event when it is observed through multiple monitor events or repeated procfs scans; the daemon does not deduplicate those observations. `loglevel` still applies normally, so `warn`, `error`, and `critical` suppress the `INFO` event. Use `loglevel = info` (or a more verbose level) when enabling this option.

Rust uses the `tracing` severity set, which has no separate `critical` event level. The configuration value `critical` is therefore accepted as an error-threshold alias; it does not create a distinct output severity. The legacy spelling `fatal` is also accepted as an input alias and is serialized as `critical`. An unknown `loglevel` value falls back to `info` and produces a warning through the configured logger.

## Rules (`*.rules`)

Rules are defined in files ending with `.rules` in the configuration directory.

For instance, to add a rule for GCC, you could do the following:

1. Create the `/etc/ananicy.d/10-compilers` folder.
2. Create the `/etc/ananicy.d/10-compilers/gcc.rules` file
3. Add `{"name": "gcc", "nice": 19, "latency_nice": 19, "sched": "batch", "ioclass": "idle"}` to the file.

### Supported Attributes

- `name`: The process name or wildcard to match (e.g., `gcc`, `kworker/*`).
- `nice: [-20..19]`: Set the nice value of the process. A process with a higher nice value will be more "polite", and will get less CPU time than processes with a lower nice value.
- `latency_nice: [-20..19]`: Set the latency_nice value of the process. A process with a lower latency_nice value indicates the task needs lower latency. *(Note: Requires specific kernel patches or newer kernels that support latency_nice)*.
- `sched: {"fifo", "rr", "normal", "batch", "idle"}`: Set the scheduling policy.
  - `fifo` and `rr` (round-robin) are realtime scheduling policies, and must only be used for latency critical programs (e.g., `Xorg`, `pulseaudio`). Nice values are ignored, `rtprio` should be used instead.
  - `deadline`: Special realtime scheduling policy which *can't* be set by ananicy, but can be reported.
  - `normal`: The default behavior for the OS. Useful to force a child of a realtime process back to normal scheduling.
  - `batch`: Very useful for compilers or other CPU-hungry, non-interactive programs. Improves their performance with almost no cost to the rest of the system.
  - `idle`: Very, very low priority, even lower than a nice value of `19`. Useful for background, low priority tasks like file indexers.
- `rtprio: [0, 99]`: Sets the static priority of a process. Only relevant if the actual scheduling policy of a process is a realtime one (`fifo` or `rr`). A higher value means a higher priority.
- `ioclass: {"best-effort", "realtime", "idle", "none"}`: Define the IO scheduling policy. By default, it is `best-effort`. **Only the BFQ/CFQ I/O schedulers fully support `ioclass` and `ionice`**.
  - `realtime`: Absolute priority above all `best-effort` processes. Can starve other processes.
  - `idle`: Process gets I/O resources after all other processes. Can starve this process. (`ionice` is ignored).
  - `none`: Reset I/O policy to system default, `ionice` must be `0`.
  - `best-effort`: Try to fairly share I/O resources between processes.
- `ionice: [0..7]`: I/O priority for `realtime` and `best-effort` classes. A lower value is a higher priority.
- `oom_score_adj: [-999..1000]`: Adjust the Out Of Memory killer score. Negative values decrease the score, making it *less* likely to be killed. Use for critical programs.
- `cpuset`: Pin the process to the specified CPU cores using Linux cpuset notation. Accepts ranges (`0-7`), comma-separated lists (`0,2,4`), mixed (`0-3,8-11`), or [Named Aliases (Topology)](./TOPOLOGY.md).
- `cgroup`: Put the process in the specified cgroup.
- `type`: Set the type of the rule. All options defined in the type will be used as if written explicitly in the rule, although you can override each option if needed.

## Types (`*.types`)

To avoid repeating yourself, you can add types in `.types` files.

The syntax is the following:
```json
{"type": "my_type", "nice": 19, "other_parameter": "value"}
```

It can then be used in any rule by adding the `type` property:
```json
{"name": "gcc", "type": "compiler"}
```

Parameters can be overridden in the rule:
```json
{"type": "compiler", "nice": 19, "sched": "batch", "ioclass": "idle"}
{"name": "gcc", "type": "compiler", "ioclass": "none", "ionice": 0}
```

## Cgroups (`*.cgroups`)

Cgroup parameters are defined in `.cgroups` files.

Currently, the following attributes are supported:
- `CPUQuota`: Maps to `cpu.max` in Cgroups v2 or `cpu.cfs_quota_us` in v1 (expressed as a percentage, e.g., 80 = 80%).
- `CPUWeight`: Maps to `cpu.weight` in Cgroups v2 or `cpu.shares` in v1.

Example:
```json
{"cgroup": "cpu80", "CPUQuota": 80}
```

### Cgroups v2 Delegation and Ownership

`ananicy-rs` respects the kernel's cgroup-v2 single-writer model. The systemd unit uses `Delegate=yes`, which delegates the cgroup subtree assigned to `ananicy-rs.service` (normally `/sys/fs/cgroup/system.slice/ananicy-rs.service`). It does not delegate `user.slice`, desktop session scopes, or other systemd units.

Within the delegated subtree, `ananicy-rs` may create and configure cgroups, enable supported controllers, move processes, and apply `CPUQuota`/`CPUWeight`.

Outside that subtree, structural changes are refused. In particular, it will not create foreign cgroup directories, enable foreign `cgroup.subtree_control`, write foreign `cpu.max`, or move processes into foreign cgroups.

There is one limited resource-tuning exception: for a foreign cgroup, `ananicy-rs` may attempt to write an already-existing `cpu.weight` or `cpu.shares` file. It does not create that file or enable its controller. If the controller is not enabled, the file is absent, or it is not writable, the optional mirror is skipped at DEBUG level. Process-level `nice` application can still succeed in that case.

Do not enable controllers in `user.slice` or session scopes merely to satisfy Ananicy; those scopes are managed by systemd. Use a dedicated cgroup under the delegated Ananicy subtree when testing cgroup CPU weighting. See [CLI & Usage](./CLI.md) for the systemd and NixOS service setup.
