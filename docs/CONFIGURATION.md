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
| `apply_cpu_weight_from_nice` | `true` | On cgroup v2, mirror an applied `nice` value into the `cpu.weight` of the cgroup the process already belongs to. See [the mirror](#the-nice--cpuweight-mirror) |
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

`log_applied_rule` is opt-in. When it is `true`, the daemon emits one `INFO` event for each task whose matching rule has at least one enabled attribute and whose application completes without an error or a skipped attribute. The event includes the task name, PID, and matched rule. On cgroup v2, mirroring `nice` to the optional `cpu.weight` controller is best-effort; if that controller is unavailable or a write is refused, a successful `nice` application is still reported. The mirror failure is logged at WARN level, once per cgroup, naming the cgroup and the file — a failed write also includes the kernel's error (with the errno).

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

Set `apply_cpu_weight_from_nice = false` to keep the `nice` value and drop the mirror. The mirror is
also skipped whenever the kernel does not expose a `cpu.weight` (cgroup v2) or
`cpu.shares` (cgroup v1) file in that cgroup — the controller has to be enabled in the
cgroup's *parent* first, and `ananicy-rs` never enables it in a cgroup it does not own.
When that happens to a cgroup the daemon manages, it says so at `warn` level, **once per
cgroup**: the warning means the `nice` value was applied but the bandwidth it implies was
not, and it names the cgroup and the file that is missing.

That covers two different situations, and they have different fixes. A cgroup the daemon
created from a `.cgroups` rule gets a `cpu.weight` once the unit delegates correctly —
see the delegation section below. An ordinary desktop application's scope does not,
because `cpu` has to be enabled in that scope's parent all the way down from the root,
and systemd only does that where a unit under it asks for a weight. On such a host
`app.slice` typically has no `cpu` in its `cgroup.subtree_control`, so no scope beneath it
has a `cpu.weight` file at all. That is the host's unit configuration rather than
anything `ananicy-rs` controls, and the daemon will not enable a controller in someone
else's populated cgroup to work around it — the same reason it refuses to touch
`user.slice`.

### `cgroup_realtime_workaround`

On by default, and relevant only to rules that set a realtime policy (`fifo` or `rr`) on
cgroup v2. For a realtime process the daemon leaves the rule's `cgroup` attribute
unapplied while this option is on: **the task stays in its existing cgroup** rather than
being moved. That skip is unconditional on v2 — `CONFIG_RT_GROUP_SCHED=y` is what makes
moving a realtime task into a bandwidth-limited cgroup a kernel restriction, and it only
governs whether the bandwidth diagnostic below matters. The daemon deliberately does not
reproduce `ananicy-cpp`'s additional move of the process to the hierarchy root: the root
is a foreign cgroup, outside this daemon's delegated subtree, so a cross-boundary move
would be refused before reaching the kernel.

The realtime policy is re-read after `set_sched` succeeds and before the `cgroup` and
mirror decisions. A rule that *makes* a CFS task realtime (`"sched": "fifo"` or `"rr"`,
with `rtprio` defaulting to 1) therefore skips both its own `cgroup` attribute and the
`nice` → `cpu.weight` mirror in the same event. `ananicy-cpp` samples realtime status
before applying the rule and does not recover this case; the re-check here is stronger
than parity.

When a realtime task is found in a cgroup whose own-or-ancestor `cpu.max` has a quota
other than `max`, the daemon emits at most one warning per pid (bounded cache) and keeps
the task where it is. The warning is non-fatal and is not reported as a rule failure; on
kernels without `CONFIG_RT_GROUP_SCHED=y` it is merely informational.

The `nice` → `cpu.weight` mirror described above is also skipped for realtime tasks on
v2, independently of this option: `cpu.weight` affects only fair-class tasks, so it
cannot help the realtime process and can reweight unrelated CFS siblings in the same
cgroup. Setting `cgroup_realtime_workaround` to `false` re-enables applying the rule's
`cgroup` attribute to realtime processes like any other; it does not re-enable the mirror.

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
  than full PCRE2; see the [migration guide](./COMPATIBILITY.md#name_regex)
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
- `type`: Set the type of the rule. All options defined in the type will be used as if written explicitly in the rule, although you can override each option if needed. An attribute written with nothing usable in it — `null`, or a number written as a string — *suppresses* the value its type would otherwise supply rather than inheriting it, so `{"name": "x", "type": "T", "nice": null}` is a rule that does not set `nice`. That is what an RFC 7396 merge of the rule onto `T` gives, and what `ananicy-cpp` does.

A key this daemon does not implement is kept with the rule and reported by `dump rules` rather than dropped, so a rule set that uses one is not reported as though it had never been written. It is not applied, and it is not counted as a reason the rule did nothing.

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
than an arbitrary amount. `CPUQuota: 0` is ignored with a warning: a zero quota would give the
cgroup no CPU time at all, so the kernel's unlimited default stays in place and the control file
is not written. A quota above 100% is clamped to 100% with a warning. Values above `u32::MAX` for
`CPUQuota` or `CPUWeight` are ignored with a warning rather than truncated modulo 2³². Values
that do fit in a `u32` are clamped to the range the kernel accepts — `CPUQuota` to 1–100% and
`CPUWeight` to 1–10000 on cgroup v2 (`cpu.shares` is scaled from the weight on v1).

These settings are applied every time the cgroup is prepared at start-up, whether
the daemon created the cgroup or it was already there. Editing a `.cgroups` file
therefore takes effect on the next start without the cgroup having to be deleted
first.

The daemon tracks only cgroup v2 directories this run created and still remembers.
After a `SIGUSR1` reload (once the new rule cgroups have been prepared) and on a
graceful `SIGINT`/`SIGTERM` stop, a remembered cgroup that no rule names any more
has `cpu.max` and `cpu.weight` reset to their kernel defaults when it is removed;
removal retries a transient `EBUSY` a bounded number of times. A cgroup that still
holds processes is kept with its settings untouched and reported at `warn` level. Foreign
paths, the delegated root itself, and cgroups left behind by a previous run are
never reset or deleted; adopting an existing cgroup for settings re-apply does
not make it ours to remove.

### Cgroups v2 Delegation and Ownership

`ananicy-rs` respects the kernel's cgroup-v2 single-writer model. The systemd unit uses `Delegate=yes`. It does not delegate `user.slice`, desktop session scopes, or other systemd units.

**`Delegate=yes` on its own does not give the daemon a usable `cpu` controller.** It
gives the service ownership of its own cgroup *and* delegation of that cgroup's subtree,
but without `DelegateSubgroup=` the daemon runs in the unit's own cgroup. That cgroup
holds the daemon process, and the kernel's "no internal process" rule means a cgroup with
processes in it cannot have `+cpu` added to its `subtree_control`. The daemon then cannot
enable `cpu` there, cgroups it creates below lack `cpu.max` and `cpu.weight`, and
**`CPUQuota` and `CPUWeight` rules and `nice`→`cpu.weight` mirroring are no-ops**. It
logs a `warn` when enabling `cpu` fails rather than skipping silently.

Two changes fix it, and the shipped unit makes both. `DelegateSubgroup=delegated` on the
unit makes systemd start the daemon in a sub-cgroup of the unit, which leaves the unit's
own cgroup with no processes in it — and an empty cgroup is what the kernel requires
before a domain controller can be enabled underneath it. Note it takes a name, not a
boolean: `DelegateSubgroup=yes` would create a cgroup literally called `yes`.

The daemon does the other half. It discovers its root from `/proc/self/cgroup`: with the
shipped sub-cgroup that path is `…/ananicy-rs.service/delegated`, and the cgroup it
manages from is the parent of that sub-cgroup — the unit's own cgroup — because the
sub-cgroup holds the daemon and cannot take a controller itself. The step up is bounded
to one level and happens only for the shipped unit's `delegated` subgroup with the parent
verified as ours: a readable, empty cgroup strictly inside the mount point. The mount
point itself is never adopted, so an empty path outside the delegated subtree cannot
widen ownership to the rest of the hierarchy.

What does *not* work is `CPUWeight=` on the unit, which is the obvious thing to reach for.
A unit's `cgroup.subtree_control` is populated from what the unit's *children* require,
and a leaf service has no children, so it stays empty however the unit's own weight is
set. `CPUWeight=` writes a value into the unit's own `cpu.weight`; it does not cause
anything to be enabled underneath it. What is needed is an empty cgroup to be the parent,
not a weight on the populated one.

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
