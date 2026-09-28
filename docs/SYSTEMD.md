# systemd in ananicy-rs

This document is about how `ananicy-rs` uses systemd: which parts of the service
manager we consume, which flags control that behaviour, what the shipped unit
has to do for us, and what the operator has to know. It is not a guide to writing
systemd units — for that see `systemd.service(5)` and friends, listed at the
end.

Related: [CLI & Usage](./CLI.md) · [Configuration and Rules](./CONFIGURATION.md)
· [Cgroups v2 Delegation and Ownership](./CONFIGURATION.md#cgroups-v2-delegation-and-ownership)

## What we use systemd for

Three things, all of them optional:

1. **A journald destination.** When we are confident a service manager started
   us, we log over the journald *native* protocol so entries carry structured
   fields instead of being plain stderr text.
2. **Status notifications.** We send `READY=1` after the worker is up and
   `STOPPING=1` on `SIGTERM`.
3. **A delegated cgroup subtree.** A unit with `Delegate=yes` delegates the
   subtree *above* the unit's own cgroup, which is what lets us create cgroups and
   write `cpu.max`/`cpu.weight`. This is the only thing we *need* systemd for — and
   it is governed by the unit, not by our flag. The unit also sets
   `DelegateSubgroup=` and needs to, for bandwidth rules to have any effect; see
   the delegation section below for why the obvious alternative does not work.


Everything else we do — applying `nice`, `ionice`, `oom_score_adj`,
`latency_nice` — is plain process manipulation and does not involve systemd.

## The environment we read

`src/systemd.rs` resolves one question: *was this process started by a service
manager as a service?* We answer it from the environment systemd exports, which
is the only versioned contract between the two, and never from the fact that the
host runs systemd.

| Variable | Since | Why we read it |
|----------|-------|----------------|
| `$INVOCATION_ID` | 232 | Our primary signal. Set for every process of an active unit, for all unit types and all `Type=` values, so it covers the plain `Type=simple` unit we ship |
| `$NOTIFY_SOCKET` | 229 | Secondary. Only set when `NotifyAccess=` ≠ `none`. The shipped unit sets `NotifyAccess=main`, so it *is* set for our own unit; it is checked only after `$INVOCATION_ID`, so this is a fallback for `Type=notify` units and for systemd 229–231 |
| `$JOURNAL_STREAM` | 231 | Weakest. Set when our stdio is wired to the journal, which is the precondition for the native protocol we want |

Rules we apply to them:

* **An empty value counts as unset**, matching `sd_notify()` and
  `sd_listen_fds()`. `Environment=INVOCATION_ID=` in a unit therefore cannot
  switch the mode on.
* **The environment is inherited**, so a manual `sudo ananicy-rs` from a desktop
  terminal inherits the terminal's `$INVOCATION_ID` — the terminal is a
  `.scope`. Presence proves "a unit started an ancestor of me", not "a unit
  started me".
* **Therefore the scope veto:** if evidence exists but our own cgroup leaf ends in
  `.scope`, we are a guest in someone else's session, not the service systemd was
  asked to start, and the mode stays off. Only the leaf is checked, so
  `DelegateSubgroup=` putting us one level below the unit cgroup does not break
  detection; and the cgroup is only read when there is evidence, so a manual run
  on a non-systemd host pays nothing.
* **We read nothing else.** No `/run/systemd/system`, no PID 1, no parent
  process, no `sd_pid_get_unit()`. Those are host-wide or racy: `/run/systemd/system`
  exists for every process on a systemd host including manual runs, and
  `sd_pid_get_unit()` reports the *enclosing* unit — for a shell in
  `…/user@1000.service/app.slice/kitty-133224-0.scope` it answers
  `user@1000.service`, which would look like a service but is a login session.

## Flags

| Invocation | Result |
|------------|--------|
| `ananicy-rs start` | **Auto** — enabled iff environment evidence *and* not a transient scope |
| `ananicy-rs --systemd start` | Forced on, even inside a scope |
| `ananicy-rs --no-systemd start` | Forced off |
| `ananicy-rs --systemd --no-systemd` | Parse error, exit code 2 |

`--systemd` stays a valueless flag so existing command lines like
`ExecStart=ananicy-rs --systemd start` keep parsing; an optional-value form such
as `--systemd=auto` would consume the `start` argument.

There is deliberately **no `ananicy.conf` key** for this. It describes how the
process was launched, not how it should tune processes, and the flags already
give explicit control.

### What the mode changes

| Enabled | Disabled |
|---------|----------|
| `tracing-journald` layer, so log entries get structured fields; silently falls back to stderr if the journal socket is unreachable | `fmt` layer on stderr |
| `READY=1` sent after cgroup creation and worker start-up | — |
| `STOPPING=1` sent on `SIGINT`/`SIGTERM` | — |
| The "daemon mode not implemented" warning is suppressed, since foreground service runs are expected | The warning appears for `--daemon` |

What it does **not** change:

* **Cgroup handling.** Delegation is discovered from our own cgroup
  independently, and cgroup writes are gated by `CgroupOwnership`. Being a
  service does not grant delegation — a unit without `Delegate=yes` is refused
  exactly like a manual run.
* **Process tuning.** `nice`, `ionice`, `oom_score_adj` and `latency_nice` work
  identically in both modes.
* **Build configuration.** Without the `systemd` cargo feature the mode is
  always off and the status reads `disabled (built without the systemd feature)`.
  `contrib/nixos/package.nix` exposes this as `withSystemd`.

The resolved mode is logged at `debug` level and shown by `debug cgroups`, e.g.
`Systemd integration: enabled (auto-detected from $INVOCATION_ID)`.

## Our unit, and why each setting is there

`data/ananicy-rs.service.in` is the packaged unit. Most of it is hardening that
exists for us rather than for systemd, so this is the reasoning:

| Setting | Value | What it buys us |
|---------|-------|-----------------|
| `Delegate` | `yes` | Delegates the subtree *above* this unit's cgroup, which is what lets us create cgroups and write `cpu.max`/`cpu.weight`. Scoped to the unit — it says nothing about `user.slice` or session scopes. The consequence for `cpu` is in the delegation section below |
| `DelegateSubgroup` | `delegated` | Starts this process in a sub-cgroup of the unit, which leaves the unit's own cgroup with no processes in it — the precondition for enabling `cpu` there. Without it every cgroup we create has no `cpu.max` and no `cpu.weight`. Takes a cgroup name, not a boolean. See the delegation section below |
| `NotifyAccess` | `main` | Without it the unit is `Type=simple`, whose default `NotifyAccess=` is `none`, and the manager discards every `sd_notify` we send. With it, `READY=1` and `STOPPING=1` reach the manager. It changes no lifecycle semantics — the unit stays `Type=simple` — it only stops the messages being dropped |
| `ReadWritePaths` | `/sys/bus/platform/drivers/amd_x3d_vcache` | `ProtectKernelTunables=yes` mounts `/sys` read-only, which includes the AMD X3D driver's `amd_x3d_mode`, so `x3d_mode` could not be applied. systemd applies `ReadWritePaths` before the read-only remount and `MS_RDONLY` is per-mount, so the subdirectory bind survives. Harmless where the driver is absent |
| `ProtectControlGroups` | `no` | With `yes` the cgroup hierarchies are mounted read-only, which would disable cgroup management entirely |
| `ProtectProc` | `default` | `default` means *no* restrictions. `invisible` would hide other users' processes, and we exist to tune them |
| `PrivateUsers` | `no` | User-namespace remapping would make other processes appear as `nobody`/`overflowuid` and break our `/proc` ownership checks |
| `PrivateNetwork` | `no` | The netlink process connector is per network namespace; in a private one it would only report our own processes. The eBPF backend and `--manual-scanning` are alternatives, but the default netlink backend is not |
| `ProcSubset` | `pid` | Drops `/proc` files unrelated to processes, e.g. `/proc/sys`. We read per-PID files, and detect `latency_nice` support with `sched_setattr(2)` on our own thread, not through `/proc/sys` |
| `RestrictAddressFamilies` | `AF_UNIX`, `AF_NETLINK` | Journal socket and process connector, nothing else |
| `RestrictNamespaces` | `cgroup` | We never create a cgroup namespace; `Delegate=yes` does not require one, since managing cgroups is `mkdir` plus file writes |
| `CapabilityBoundingSet` | `CAP_SYS_NICE`, `CAP_SYS_RESOURCE`, `CAP_DAC_READ_SEARCH`, `CAP_SYS_ADMIN`, `CAP_DAC_OVERRIDE` | Exactly what setting `nice`/`ionice`/`oom_score_adj`/`latency_nice` and writing cgroupfs needs |
| `Nice`, `OOMScoreAdjust` | `-5`, `-999` | Keeps the daemon itself from being starved or OOM-killed while it manages everyone else |
| `MemoryHigh`, `MemoryMax` | `48M`, `96M` | A soft line above the working set — 1.40× the 34.4M peak with a default rule set — so reclaim is a response to a leak rather than a permanent state, and a hard cap twice it. `MemoryHigh` reads like a limit and is not one, and the reference's 16M was below the working set; see [COMPATIBILITY § 1](./COMPATIBILITY.md#1-project-identity-and-configuration) and [Memory](./MEMORY.md) |
| `ExecReload` | `ananicy-rs --reload` | Configuration reload without dropping events (see below) |
| `Restart`, `RestartSec` | `always`, `10` | Survives crashes; `SuccessExitStatus=143` (`128 + SIGTERM`) keeps a deliberate stop from being logged as a failure |
| `StartLimitIntervalSec`, `StartLimitBurst` | `60`, `5` | Stops a restart loop from thrashing the machine |
| `After`, `WantedBy` | `local-fs.target`, `multi-user.target` | Config lives on disk; normal boot-time start |

None of these hide `/proc/self/cgroup` or the journald socket, so detection and
delegation discovery behave the same under the shipped hardening as they do
outside it. `ananicy-rs debug cgroups` prints the resolved mode, the enclosing
unit name and our cgroup path.

The settings not in the table — `PrivateTmp`, `PrivateDevices`, `ProtectHome`,
`ProtectSystem`, `ProtectClock`, `ProtectHostname`, `ProtectKernelLogs`,
`ProtectKernelModules`, `NoNewPrivileges`, `MemoryDenyWriteExecute`,
`LockPersonality`, `RestrictRealtime`, `RestrictSUIDSGID` — are stock hardening with
no daemon-specific reasoning behind it. They are listed in the unit file itself.

## Delegation, from the operator's side


`Delegate=yes` in our unit is what enables cgroup support. The daemon then
discovers its delegated root from `/proc/self/cgroup` and refuses cgroup
mutation outside it. What that means in practice:

* **Run it as a service.** `sudo ananicy-rs` from a terminal lands in that
  terminal's `.scope`, which we refuse with a warning — root can write to any
  `cgroup.procs`, so writability is not evidence of delegation there. Adopting
  the session's cgroup would mean moving PIDs into cgroups systemd owns.
* **cgroup v1 hosts have no delegated root** for us (no unified `0::` line), so
  cgroup rules are skipped and process tuning continues.
* **Controllers must be enabled on the parent, and the shipped unit does not do
  it.** `Delegate=yes` hands over the subtree above the unit's own cgroup, but the
  *parent's* `cgroup.subtree_control` decides whether a given leaf gets a `cpu.weight`
  file at all, and systemd only enables a controller in a unit's own cgroup when the
  unit has a resource setting that needs one. The shipped unit sets `Nice=-5` and
  nothing CPU-bandwidth-related, so its own `cgroup.subtree_control` does not contain
  `cpu` on its own account, and the leaves the daemon creates have no `cpu.weight` and
  no `cpu.max`. Every `CPUWeight` and `CPUQuota` in a `.cgroups` rule, and the `nice` →
  `cpu.weight` mirroring into cgroups we create, are therefore no-ops unless the unit
  arranges for the controller to be available. The daemon warns at start-up when it
  cannot apply a weight.

  **`DelegateSubgroup=delegated` is what arranges it.** systemd starts this process in
  a sub-cgroup of the unit, which leaves the unit's own cgroup with no processes in
  it, and an empty cgroup is what the kernel requires before a domain controller can
  be enabled underneath it (`cgroup_vet_subtree_control_enable()` returns `-EBUSY` when
  `cgroup_has_tasks()`). It takes a cgroup name, not a boolean — `DelegateSubgroup=yes`
  would create a cgroup called `yes`.

  **Both halves are needed, and the daemon does the other one.** Because systemd puts
  us in the sub-cgroup, the cgroup we would naturally manage from is the sub-cgroup —
  and that one holds this process, so it can never take a controller. The daemon
  therefore checks, at start-up, whether the cgroup it discovered still holds
  processes and whether its parent is empty, and if so manages from the parent instead.
  The step up is bounded to one level and requires the parent to be genuinely empty,
  which is what keeps it inside the delegated subtree: the hierarchy root always holds
  PID 1, so it can never qualify, and a container's root has no parent inside its own
  mount.

  `CPUWeight=` on the unit would *not* fix it, and it is worth saying why because it is
  the obvious thing to try. A unit's `cgroup.subtree_control` is enabled from what the
  unit's children need, and a leaf service has no children, so it stays empty however
  the unit's own weight is set. `CPUWeight=` writes a value into the unit's own
  `cpu.weight`; it does not enable anything underneath. What is needed is an empty
  cgroup to be the parent, not a weight on the populated one.

  We never enable controllers on someone else's cgroup to work around it.

  `ananicy-cpp` is not affected, and the reason is worth knowing before copying its
  approach: its unit carries no `Delegate=` at all. It creates each cgroup directly
  under the cgroup2 mount point (`/sys/fs/cgroup/<name>`), which works because the
  hierarchy root already has `cpu` in its `cgroup.subtree_control` and the daemon runs
  as root with no ownership checks. `ananicy-rs` confines itself to a subtree it has
  been delegated, and refuses the global root — which is the safer behaviour, and also
  what puts it behind the no-internal-process rule.
* **Containers** work: with their own cgroup namespace our path is `/` and a
  writable container root becomes the delegated root.

The full allow/refuse matrix is in
[Configuration and Rules](./CONFIGURATION.md#cgroups-v2-delegation-and-ownership).

## Reload

`systemctl reload` runs `ExecReload=`, which is `ananicy-rs --reload`. That
command only signals the running daemon through the IPC semaphore and exits, so
the reload happens in the running process without dropping events.

| Change | Applied by |
|--------|-----------|
| `loglevel`, `log_applied_rule`, other `apply_*` flags | Reload |
| `ananicy.conf` values that are re-read | Reload |
| `.rules`, `.types`, `.cgroups` | Reload |
| `check_freq` | Restart — captured when the manual scanner starts |

A reload that fails to parse keeps the previous configuration and logs the
error; the daemon does not silently fall back to defaults. The rules are reloaded
independently of that outcome, because a configuration that will not parse has not
changed `rule_load` either.

`rule_load = false` is honoured as the deliberate choice it is: the rules already
loaded stay in force and the reload says so.

Two things about what a reload can and cannot reach:

* **It applies to processes as they are seen.** A process already tuned keeps the
  values it was given until the next time it produces an event, which for an
  event-driven backend means the next exec. Nothing is re-tuned retroactively,
  because the daemon is not keeping a list of what it has already done.
* **It creates cgroups for newly added `.cgroups` entries.** A rule naming a
  cgroup that does not exist yet has no directory until one is made, and a restart
  would have made it, so a reload does too. Settings are applied whether or not
  the cgroup already existed, so editing an existing entry works as well.

The rule set is swapped in atomically. A process being matched at the moment of the
swap is matched against one consistent set rather than a mixture of the old and the
new, and the write lock is held only for the swap itself.

## NixOS

`contrib/nixos/module.nix` keeps the packaged unit as the single source of truth:
`Delegate=yes`, hardening, `ExecReload=` and the restart policy live in
`data/ananicy-rs.service.in`, which the module exposes through
`systemd.packages`. The module only forces `ExecStart=` so that
`services.ananicy-rs.extraArgs` can be injected in front of `start`.

`extraArgs` defaults to `[]` because auto-detection makes `--systemd`
unnecessary; use `[ "--no-systemd" ]` to force the mode off. Since a NixOS system
can end up with both a package-provided and a module-generated unit, check which
one is in effect before debugging:

```bash
systemctl cat ananicy-rs.service | grep -E 'Delegate|ExecReload|ExecStart'
```

## Checking a deployment

```bash
# Is the unit delegated, and where does the process sit?
systemctl show ananicy-rs.service -p Delegate -p Type -p NotifyAccess
cat /proc/$(pidof ananicy-rs)/cgroup

# Our own view: integration mode, unit name, cgroup
sudo ananicy-rs debug cgroups

# Logs
journalctl -u ananicy-rs.service -p debug
```

Reproducing the detection matrix by hand:

```bash
# As a service: enabled
systemd-run --user --wait --pipe --unit=ananicy-detect -- \
  ./target/debug/ananicy-rs debug cgroups | grep 'Systemd integration'
# → Systemd integration: enabled (auto-detected from $INVOCATION_ID)

# As a scope: vetoed
systemd-run --user --scope --unit=ananicy-detect-scope -- \
  ./target/debug/ananicy-rs debug cgroups 2>/dev/null | grep 'Systemd integration'
# → Systemd integration: disabled (member of a transient .scope, not a service)

# From a desktop terminal, which inherits the scope's $INVOCATION_ID
./target/debug/ananicy-rs debug cgroups | grep 'Systemd integration'
# → Systemd integration: disabled (member of a transient .scope, not a service)
```

| Symptom | Cause | Fix |
|---------|-------|-----|
| `disabled (no systemd service manager detected)` in a real unit | Host older than systemd 232, so no `$INVOCATION_ID` is exported | Pass `--systemd` through `extraArgs` |
| `disabled (member of a transient .scope, not a service)` from a terminal | Manual run inherits the session scope's environment | Run it as a service |
| `WARN Cgroup v2: Detected manual execution inside a transient .scope` | Manual run; the session cgroup is not delegated to us | Run it as a service with `Delegate=yes` |
| `Rule applied, CPU weight unchanged` | The `cpu` controller is not in the leaf's parent `cgroup.subtree_control`, so no `cpu.weight` file exists. This is the expected state of the shipped unit — see the delegation section below | `DelegateSubgroup=<name>` on the unit. `CPUWeight=` will not do it; the delegation section explains why. The daemon warns at `warn` level whenever it hits this |
| Output never reaches `journalctl` | The mode was off, so we logged to stderr, which the journal only captures if the unit's stdio is connected to it | Check the reported mode; force with `--systemd` |
| Journal entries have no structured fields | The stderr layer was used because the mode was off or `$JOURNAL_STREAM` was unset | Same as above; then `journalctl -o verbose` shows native-protocol fields |
| `READY=1` never appears in the journal | It is a notification, not a log line, so it reaches the manager and is not journal output by design. The shipped unit sets `NotifyAccess=main`, so the manager accepts it; `READY=1` is still not something to look for in `journalctl` | Expected; use `systemctl show -p ActiveState` for readiness |

## Without systemd

Nothing requires systemd. On OpenRC, `runit`, `s6`, BusyBox `init` or a plain
container, auto-detection finds no evidence and we log to stderr. The only thing
lost is delegation that systemd would have provided: a writable cgroup root in
`/proc/self/cgroup` — what most containers and most non-systemd inits give us —
is still used as the delegated root, so cgroup rules keep working. On cgroup v1,
or where that root is read-only, cgroup rules are skipped and process-level
tuning continues.

## Testing

`src/systemd.rs` unit-tests the resolver without a service manager: service
supervision, user service, systemd in a container, manual invocation, each
evidence source in isolation, evidence ordering, partial and empty environments,
the scope veto, explicit precedence, and `/proc/self/cgroup` parsing for the
unified, hybrid and legacy hierarchies. `tests/cli.rs` covers flag acceptance,
mutual exclusion, and that the *environment* rather than the host drives the
decision — the ambient environment is neutralised so the suite does not depend on
the developer's init system.

Not covered automatically: a root (system) service instance and our packaged unit
need root and a real `systemctl`; `Type=notify` is not what we ship; hosts older
than systemd 232 rely on the documented `--systemd` override.

## Upstream references

`systemd.exec(5)` for the environment variables and the `Protect*=` options ·
`systemd.service(5)` for `Type=`, `NotifyAccess=`, `ExecStart=`, `ExecReload=` ·
`systemd.resource-control(5)` for `Delegate=`, `DelegateSubgroup=`, `CPUWeight=`,
`CPUQuota=` · `sd_notify(3)`, `sd_pid_get_unit(3)` · `cgroup-v2(7)` for the
no-internal-process rule · <https://systemd.io/CGROUP_DELEGATION> for the
delegation model.
