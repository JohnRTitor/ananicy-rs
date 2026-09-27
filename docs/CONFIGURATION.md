# Configuration and Rules

`ananicy-rs` dynamically applies performance tweaks based on rule files. The configuration behavior mirrors the upstream `ananicy-cpp` and `ananicy` projects.

By default, `ananicy-rs` looks for configuration and rules in `/etc/ananicy.d/`. This path can be overridden with the `--config-dir` CLI option or the `ANANICY_RS_CONFDIR` environment variable.

*(Note: `ananicy-rs` does not ship with rules by default. You should copy community rules from the original `ananicy` or `ananicy-cpp` project into `/etc/ananicy.d/`)*.

## Global Configuration (`ananicy.conf`)

Global settings are defined in `/etc/ananicy.d/ananicy.conf`. The path can be explicitly set via the `--config` CLI option or the `ANANICY_RS_CONF` environment variable.

The format is `key=value`, one per line.

| Option | Default | Description |
|--------|---------|-------------|
| `check_freq` | `60` | Full process scan interval in seconds (used during manual scanning); must be at least 1 |
| `check_disks_schedulers` | `true` | Report the block devices whose I/O scheduler will not honour `ioclass` and `ionice` |
| `apply_nice` | `true` | Apply nice values from rules |
| `apply_sched` | `true` | Apply scheduling policy from rules |
| `apply_ionice` | `true` | Apply I/O nice values from rules |
| `apply_ioclass` | `true` | Accepted and reported, but it has no effect — see [below](#apply_ioclass-has-no-effect) |
| `apply_oom_score_adj` | `true` | Apply OOM score adjustments from rules |
| `apply_latnice` | `true` | Apply latency nice values from rules |
| `apply_cpuset` | `true` | Apply CPU affinity (cpuset) from rules |
| `apply_cgroup` | `true` | Apply cgroup membership from rules |
| `apply_cpu_weight` | `true` | On cgroup v2, mirror an applied `nice` value into the `cpu.weight` of the cgroup the process already belongs to. See [Applied-rule logging](#applied-rule-logging) |
| `cgroup_load` | `true` | Load cgroup definitions (`.cgroups` files) |
| `type_load` | `true` | Load type definitions (`.types` files) |
| `rule_load` | `true` | Load rule definitions (`.rules` files) |
| `cgroup_realtime_workaround` | `true` | Enable cgroup realtime workaround |
| `log_applied_rule` | `false` | Emit an INFO event after a matching rule is applied successfully. See [Applied-rule logging](#applied-rule-logging) |
| `loglevel` | `info` | Minimum log level (`trace`, `debug`, `info`, `warn`, `error`, `critical`) |
| `x3d_mode` | `auto` | AMD X3D driver mode: `auto` (don't touch), `cache`, or `frequency` |

### `check_disks_schedulers`

At start-up the daemon reads the I/O scheduler of every block device and reports the ones that will
not honour the `ioclass` and `ionice` of a rule:

```
WARN Disk sda is on a scheduler that does not honour ioprio (it is using "none"), so ioclass and ionice will not work for it
```

Support for I/O priorities is scheduler-dependent. A device is treated as able to honour
`ioclass`/`ionice` when its active scheduler is `mq-deadline`, `bfq`, `bfq-mq` or `cfq`;
any other scheduler accepts the value from `ioprio_set(2)` and ignores it, so the daemon
reports the rule as applied while the I/O priority is unchanged. `none` and `kyber` are
the two that do not honour it. (`none` is the current name for what older kernels call
`noop`, so a log line naming either means the same thing. `cfq` survives only on kernels
old enough to still have it and is kept for those.) Loop, `ram` and `sr` devices are
skipped, as are devices with no scheduler file at all.

The check is read-only, and `dump` and `debug` skip it — only the running daemon has use
for it. Set it to `false` to silence it.

The key comes from the original Ananicy, which shipped it with this meaning; the C++ and
Rust rewrites both dropped it, and it is back here because without it a rule can be
reported as applied while the disk ignores it.

### `apply_ioclass` has no effect

The key is accepted, defaulted to `true` and reported at start-up, and it changes nothing. A
rule's `ioclass` is gated by `apply_ionice` alone.

**If you are carrying an `ananicy.conf` from the original Ananicy, this matters:** there,
`apply_ioclass=false` meant "do not log the I/O class I just set", not "do not set an I/O
class". Here it does not disable I/O classes either — `apply_ionice=false` does. To stop
`ioclass` being applied, turn off `apply_ionice`, which also stops `ionice`.

That is worth knowing about the whole family, not just this key: in the original, *none*
of the `apply_*` flags gated anything. They were arguments to a logging helper, so
`apply_nice=false` did not stop `nice` from being applied either. The C++ rewrite wired
them to the syscalls, which is why they mostly do something now — but a single
`ioprio_set(2)` sets the class and the priority together, leaving `apply_ioclass` with
nothing left to gate, and the key has been inert ever since.

`ananicy-core/tests/worker_rules.rs` pins the behaviour in both directions.

### Applied-rule logging

`log_applied_rule` is opt-in. When it is `true`, the daemon emits one `INFO` event for each task whose matching rule has at least one enabled attribute and whose application completes without an error or a skipped attribute. The event includes the task name, PID, and matched rule. On cgroup v2, mirroring `nice` to the optional `cpu.weight` controller is best-effort; if that controller is unavailable, a successful `nice` application is still reported and the mirror failure is available at DEBUG level.

The event is intentionally not emitted for a rule match with no enabled applicable attributes, a partial application, or a failed application. A process can produce more than one event when it is observed through multiple monitor events or repeated procfs scans; the daemon does not deduplicate those observations. `loglevel` still applies normally, so `warn`, `error`, and `critical` suppress the `INFO` event. Use `loglevel = info` (or a more verbose level) when enabling this option.

Rust uses the `tracing` severity set, which has no separate `critical` event level. The configuration value `critical` is therefore accepted as an error-threshold alias; it does not create a distinct output severity. The legacy spelling `fatal` is also accepted as an input alias and is serialized as `critical`. An unknown `loglevel` value falls back to `info` and produces a warning through the configured logger.

### The `nice` → `cpu.weight` mirror

On cgroup v2 the kernel does not use `nice` for bandwidth control, so a rule that sets
`nice` is additionally mirrored into the `cpu.weight` of **the cgroup the process already belongs
to** — the one it was in before the rule was applied, which for a desktop session is typically a
systemd scope shared with every other application in that session.

That write is a cgroup write, not a process write: it changes the weight of the whole cgroup, and
therefore of the other tasks in it, not only of the process that matched the rule. The value is
`100 × 1.25⁻ⁿⁱᶜᵉ`, clamped to the kernel's `1..10000` range, so a rule with `nice: 5` sets a weight
of about `32` for the entire cgroup.

Set `apply_cpu_weight = false` to keep the `nice` value and drop the mirror. The mirror is
also skipped whenever the kernel does not expose a `cpu.weight` (cgroup v2) or
`cpu.shares` (cgroup v1) file in that cgroup — the controller has to be enabled there
first, and `ananicy-rs` never enables it in a cgroup it does not own. When that happens
to a cgroup the daemon manages, it says so at `warn` level, **once per cgroup**: the
warning means the `nice` value was applied but the bandwidth it implies was not, and it
names the cgroup and the file that is missing. `ananicy-cpp` has no equivalent behaviour
— it only ever calls `setpriority(2)`.

### `cgroup_realtime_workaround`

On by default, and relevant only to rules that set a realtime policy (`fifo` or `rr`) on
cgroup v2. Bandwidth control and realtime scheduling do not currently coexist, so for such
a process the daemon leaves the rule's `cgroup` attribute unapplied: the task stays in
the cgroup it is already in rather than being moved into a bandwidth-limited one it could
not be served from. That is the whole of what this option does.

It is not reported as a rule failure, and nothing is logged per process. `ananicy-cpp`
carries the same option with the same default, but there it additionally moves the process
to the hierarchy root; that move is not possible here, because a cgroup this daemon may
write to is never the hierarchy root, so an attempt would be refused before reaching the
kernel. Setting this to `false` applies the rule's `cgroup` attribute to realtime processes
like any other.

### `x3d_mode`

See [AMD X3D Support](./TOPOLOGY.md#amd-x3d-support). `auto` leaves the driver alone;
`cache` and `frequency` choose which CCD the `amd_x3d_vcache` driver prefers for
scheduling. It is a no-op on hardware without that driver, and the daemon restores the
previous mode on `SIGTERM`, so a deliberate stop does not leave the driver switched.

## Rules (`*.rules`)
Rules are defined in files ending with `.rules` in the configuration directory.

For instance, to add a rule for GCC, you could do the following:

1. Create the `/etc/ananicy.d/10-compilers` folder.
2. Create the `/etc/ananicy.d/10-compilers/gcc.rules` file
3. Add `{"name": "gcc", "nice": 19, "latency_nice": 19, "sched": "batch", "ioclass": "idle"}` to the file.

### How a process is matched, and what happens when two rules collide

Files are loaded in sorted order by path, across the whole directory tree, and a name may
be defined more than once. Three rules govern the result:

* **A later definition replaces an earlier one outright.** Two rules with the same `name`
  do not merge: the one loaded last wins in full, and any attribute the earlier rule set
  and the later one omits is not applied. To split one process's attributes across two
  files, write them in one place.
* **An exact `name` always beats a `name_regex`**, whatever the file order.
* **Between several matching regular expressions, the first in load order wins** — that
  is, the first by sorted path. This makes the result independent of the order the
  filesystem happens to return.

`name` is matched literally. `kworker/*` is not a glob: it matches a process whose name is
the literal text `kworker/*`, which is none. To match by pattern, give the rule a
`name_regex` (see [Supported Attributes](#supported-attributes)); it is used only when the
exact name does not match.

A rule is only useful if it has a `name`, a `type` or a `cgroup`; anything else is
rejected at load with an `error` and skipped, and so is a line that is not valid JSON.
Loading continues past both, so one bad line does not cost you the rest of the file.

### Supported Attributes

- `name`: The process name to match, exactly (e.g., `gcc`, `Xorg`). Matched literally —
  it is not a glob or a pattern. Use `name_regex` for anything else.
- `name_regex`: An optional regular expression, used when no `name` matches. The `name` is
  still required and is what identifies the rule, so `{"name": "kworkers", "name_regex":
  "kworker/.*"}` matches every kernel worker. Patterns are a PCRE-compatible subset rather
  than full PCRE2; see [COMPATIBILITY §5.4](./COMPATIBILITY.md#54-the-name_regex-engine-is-not-pcre2)
  for the constructs the engine refuses. A pattern the engine cannot compile is logged as
  an error and skipped, and the rule still works by its exact `name`.
- `nice: [-20..19]`: Set the nice value of the process. A process with a higher nice value will be more "polite", and will get less CPU time than processes with a lower nice value.
- `latency_nice: [-20..19]`: Set the latency_nice value of the process. A process with a lower latency_nice value indicates the task needs lower latency. *(Note: Requires specific kernel patches or newer kernels that support latency_nice)*.
- `sched: {"fifo", "rr", "normal", "batch", "idle"}`: Set the scheduling policy.
  - `fifo` and `rr` (round-robin) are realtime scheduling policies, and must only be used for latency critical programs (e.g., `Xorg`, `pulseaudio`). Nice values are ignored, `rtprio` should be used instead.
  - `deadline`: Special realtime scheduling policy which *can't* be set by ananicy, but can be reported.
  - `normal`: The default behavior for the OS. Useful to force a child of a realtime process back to normal scheduling.
  - `batch`: Very useful for compilers or other CPU-hungry, non-interactive programs. Improves their performance with almost no cost to the rest of the system.
  - `idle`: Very, very low priority, even lower than a nice value of `19`. Useful for background, low priority tasks like file indexers.
- `rtprio: [0, 99]`: Sets the static priority of a process. Only relevant if the actual scheduling policy of a process is a realtime one (`fifo` or `rr`). A higher value means a higher priority.
- `ioclass: {"best-effort", "realtime", "idle", "none"}`: Define the IO scheduling policy. By default, it is `best-effort`. **Only the `bfq` and `mq-deadline` I/O schedulers support `ioclass` and `ionice`**; any other scheduler accepts the value and ignores it.
  - `realtime`: Absolute priority above all `best-effort` processes. Can starve other processes.
  - `idle`: Process gets I/O resources after all other processes. Can starve this process. (`ionice` is ignored).
  - `none`: Reset I/O policy to system default, `ionice` must be `0`. `none` is the reading a process
    reports when no I/O priority was ever set for it, not a priority that can be written, so a rule
    asking for it leaves the process' I/O priority as it is — the same thing `ananicy-cpp` does.
  - `best-effort`: Try to fairly share I/O resources between processes.
- `ionice: [0..7]`: I/O priority for `realtime` and `best-effort` classes. A lower value is a higher priority.
- `oom_score_adj: [-1000..1000]`: Adjust the Out Of Memory killer score. Negative values decrease the score, making it *less* likely to be killed. Use for critical programs. (`ananicy-cpp` documents the range as `[-999, 999]`; the kernel accepts `-1000`, and the daemon writes the value to `/proc/<pid>/oom_score_adj` unchanged, so the kernel is the one that rejects anything outside its own range.)
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
- `CPUWeight`: Maps to `cpu.weight` in Cgroups v2 or `cpu.shares` in v1. 100 is the kernel's default weight, and the useful range is `1`–`10000`.

Example:
```json
{"cgroup": "cpu80", "CPUQuota": 80}
{"cgroup": "light", "CPUWeight": 50}
```

An attribute whose value is not a number is ignored, so a malformed rule configures nothing rather
than an arbitrary amount. Values outside the kernel's range are clamped to it.

### Cgroups v2 Delegation and Ownership

`ananicy-rs` respects the kernel's cgroup-v2 single-writer model. The systemd unit uses `Delegate=yes`. It does not delegate `user.slice`, desktop session scopes, or other systemd units.

**What `Delegate=yes` actually hands over is one level wider than it looks.** `Delegate=yes` gives the service ownership of its own cgroup *and* delegation of that cgroup's subtree, but the daemon discovers its root by taking the parent of `/proc/self/cgroup` — which lands on the unit's parent (`…/system.slice`), not on the unit's own cgroup. The unit's own cgroup contains the daemon process, and the kernel's "no internal process" rule means a cgroup with processes in it cannot have `+cpu` added to its `subtree_control`. So under the shipped unit the daemon is unable to enable the `cpu` controller on the parent, every cgroup it creates below lacks `cpu.max` and `cpu.weight`, and **`CPUQuota` and `CPUWeight` rules and `nice`→`cpu.weight` mirroring are no-ops**. The daemon logs a `warn` for this at start-up rather than skipping silently.

Two changes fix it, either of which is sufficient: add a CPU-bandwidth setting (`CPUWeight=`) to the unit, which makes systemd enable `cpu` in the unit's own `cgroup.subtree_control`; or set `DelegateSubgroup=yes`, which delegates the unit's own cgroup so the daemon's parent is the unit's cgroup and the "no internal process" rule no longer blocks it. Neither is done in the shipped unit.

Within the delegated subtree, `ananicy-rs` may create and configure cgroups, enable supported controllers, move processes, and apply `CPUQuota`/`CPUWeight`.

Outside that subtree, structural changes are refused. In particular, it will not create foreign cgroup directories, enable foreign `cgroup.subtree_control`, write foreign `cpu.max`, or move processes into foreign cgroups.

There is one limited resource-tuning exception: for a foreign cgroup, `ananicy-rs` may attempt to write an already-existing `cpu.weight` or `cpu.shares` file. It does not create that file or enable its controller. If the controller is not enabled, the file is absent, or it is not writable, the optional mirror is skipped and — when the reason is a missing `cpu.max` or `cpu.weight` on a cgroup the daemon manages — the reason is logged at `warn` level. Process-level `nice` application can still succeed in that case.

Do not enable controllers in `user.slice` or session scopes merely to satisfy Ananicy; those scopes are managed by systemd. Use a dedicated cgroup under the delegated Ananicy subtree when testing cgroup CPU weighting. See [CLI & Usage](./CLI.md) for the systemd and NixOS service setup.

Whether the daemon logs to journald and sends `sd_notify` status messages is a
separate concern from delegation: it is auto-detected from the process'
environment (`$INVOCATION_ID`, `$NOTIFY_SOCKET`, `$JOURNAL_STREAM`) and can be
forced with `--systemd`/`--no-systemd`. It is deliberately not a configuration
file key, since it describes how the process was launched rather than how it
should tune processes. See
[systemd Reference](./SYSTEMD.md).
