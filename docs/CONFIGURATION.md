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
| `cgroup_load` | `true` | Load cgroup definitions (`.cgroups` files) |
| `type_load` | `true` | Load type definitions (`.types` files) |
| `rule_load` | `true` | Load rule definitions (`.rules` files) |
| `cgroup_realtime_workaround` | `true` | Enable cgroup realtime workaround |
| `log_applied_rule` | `false` | Log each applied rule |
| `loglevel` | `info` | Log level (`trace`, `debug`, `info`, `warn`, `error`, `critical`) |
| `x3d_mode` | `auto` | AMD X3D driver mode: `auto` (don't touch), `cache`, or `frequency` |

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

### Cgroups V2 Limitations
While `ananicy-rs` supports Cgroups V2, it does not bypass the kernel's strict "single-writer" rule. To use Cgroups v2 safely, `ananicy-rs` relies on systemd delegation. It will refuse to mutate foreign cgroups to ensure system stability. See [CLI & Usage](./CLI.md) for running `ananicy-rs` under systemd properly.
