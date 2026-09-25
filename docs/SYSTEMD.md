# systemd Reference

`ananicy-rs` is normally run as a systemd service. This document is a reference
for the systemd behaviour the daemon depends on: what a service manager hands to
the processes it supervises, how cgroup v2 delegation works, why each hardening
directive in the shipped unit is there, and how the daemon decides whether it is
being supervised at all.

It is a description of current behaviour, not a change log. For day-to-day
operation see [CLI & Usage](./CLI.md); for the tuning configuration see
[Configuration and Rules](./CONFIGURATION.md); for the cgroup mutation policy see
[Cgroups v2 Delegation and Ownership](./CONFIGURATION.md#cgroups-v2-delegation-and-ownership).

## Unit Basics

A **unit** is a configuration object systemd activates; each has a name whose
suffix encodes its type.

| Type | Meaning | Started by | `ananicy-rs` relevance |
|------|---------|-------------|------------------------|
| `.service` | A long-running process, supervised for the whole runtime | `systemctl start`, socket/timer activation, `WantedBy=` | The daemon's own unit; the only unit type allowed to manage cgroups |
| `.scope` | An *existing* process group that systemd merely tracks | `systemd-run --scope`, desktop sessions, `logind` login sessions | Membership means "someone else's session"; cgroup mutation is refused |
| `.slice` | A grouping of units for resource accounting | `Slice=` in other units | Resource sharing happens *between* siblings in a slice, not within one unit |

The three facts that matter most for this daemon:

* **`Type=` defaults to `simple`.** systemd considers the service started as
  soon as `ExecStart=` is forked; it waits for nothing. The shipped unit does not
  set `Type=`, so it is `Type=simple` and readiness notifications are ignored
  (see [Status Notifications](#status-notifications-sd_notify)).
* **The `[Install]` section only affects enabling.** `systemctl enable` reads
  `WantedBy=multi-user.target` and creates a symlink; it does not start anything.
  The shipped unit uses the same target.
* **`systemctl reload` runs `ExecReload=`, it does not restart.** What can
  actually be reloaded depends on the daemon (see
  [Reload Semantics](#reload-semantics)).

## What systemd Hands a Supervised Process

`systemd.exec(5)` documents the environment systemd exports to the processes it
spawns. This is the only *documented, versioned* contract between a service
manager and a service, which makes it the right basis for detection. Version
numbers are the "Added in version" annotations from the manual.

| Variable | Since | Exported when | Usable as "this process is supervised"? |
|----------|-------|---------------|----------------------------------------|
| `$INVOCATION_ID` | 232 | For every process of an *active* unit, all unit types and all `Type=` values. A random 128-bit ID (32 hex chars) per runtime cycle, the same for all processes of the unit | **Yes** — the broadest signal; this is what the daemon uses first |
| `$NOTIFY_SOCKET` | 229 | Only when `NotifyAccess=` is not `none`. `Type=simple` defaults to `none`, so a plain service has no `$NOTIFY_SOCKET` | Yes, but incomplete |
| `$JOURNAL_STREAM` | 231 | When stdout/stderr are connected to the journal (`StandardOutput=journal` or the default `journal-or-kmsg`) | Weak hint (see [Journal Integration](#journal-integration)) |
| `$SYSTEMD_EXEC_PID` | 248 | PID of the process systemd itself `exec`ed | Precise "am I the main process", but only on very new systemd |
| `$MANAGERPID` | 208 | PID of the *user* service manager, for its children | Redundant; implies `$INVOCATION_ID` is present too |
| `$MAINPID` | 209 | Only for control processes such as `ExecReload=` | Useless for the main process |
| `$LISTEN_FDS` / `$LISTEN_PID` | 208 | Socket-activated services only | Useless here |

Two properties of this contract shape the whole design:

* **It is generic by intent.** `$INVOCATION_ID` is a public, documented
  interface: any process may set it, and any wrapper may export it. It is
  evidence, not proof.
* **The environment is inherited.** A terminal launched by a desktop is itself a
  `.scope` unit, and every shell, `sudo` call, and manual daemon run started
  from it inherits that scope's `$INVOCATION_ID`. Presence of the variable
  therefore proves "a systemd unit started an ancestor of me", not "systemd
  started *me*".

## Detecting Supervision

### Signals that do not work

| Signal | Why it is not usable |
|--------|----------------------|
| `/run/systemd/system` exists (`sd_booted()`) | `sd_booted(3)` "checks whether the system was booted up using the systemd init system", implemented by testing whether that directory exists — true for *every* process on a systemd host, manual runs included |
| PID 1 is systemd | PID 1 is not necessarily systemd (containers, `runit`, `s6`, BusyBox init), and it is true for the whole host anyway |
| Parent PID / parent process name | `systemd → sudo → ananicy-rs` and `systemd → sh -c 'ananicy-rs …'` both hide the supervisor; `/proc` exposes only the direct parent, and matching names is racy |
| Own cgroup path ends in `.service` | The unit ⇄ cgroup mapping is explicitly not public API (systemd's own `CGROUP_DELEGATION` document says so). Units can live in any slice, `Slice=` can be customised, `DelegateSubgroup=` puts the process *below* the unit cgroup, and containers may expose a delegated subtree with no unit names at all |
| `sd_pid_get_unit()` | Authoritative but answers the wrong question — see below |

### `sd_pid_get_unit()` returns the *enclosing* unit

`sd_pid_get_unit()` (added in 236) is the closest thing to an API for this
question, and it is still the wrong one. The manual says it

> may be used to determine the systemd **system unit (i.e. system service or
> scope unit)** identifier of a process.

Its implementation matches: read `/proc/<pid>/cgroup`, convert the cgroup path to
a unit name (skipping slices, taking the next segment). It therefore answers
*"which unit's cgroup subtree is this process in"*, not *"which unit is this
process"*, and the manual's own parenthetical — *service **or scope** unit* —
rules out using the `.service` suffix as a test. For a process in

```text
/user.slice/user@1000.service/app.slice/kitty-133224-0.scope
```

it returns `user@1000.service`, the first non-slice segment, so a terminal
session looks like a service. It also has no view of *user* units, which need
the separate `sd_pid_get_user_unit()`.

The daemon's own cgroup leaf (`/proc/self/cgroup`) is the more precise signal,
so that is what it uses — and it uses it only as a *negative* check.

### The rule the daemon implements

Resolution lives in `src/systemd.rs` as a pure function over an injectable
environment (`SystemdEnvironment`), so it is unit-testable without systemd:

1. **Evidence** — `$INVOCATION_ID`, else `$NOTIFY_SOCKET`, else
   `$JOURNAL_STREAM`. An *empty* value counts as unset, matching `sd_notify()` and
   `sd_listen_fds()`, so `Environment=INVOCATION_ID=` cannot switch the mode on.
2. **Scope veto** — if that evidence exists but the process' own cgroup leaf ends
   in `.scope`, the process is a member of an interactive or delegated-to-else
   session, not the service systemd was asked to start, so the mode stays off.
   Only the leaf is examined, because `DelegateSubgroup=` places the main process
   one level below the unit cgroup — `ananicy-rs.service/child` is still a
   service. The cgroup is only read when evidence exists, so a manual run on a
   non-systemd host pays no `/proc` cost.
3. **Nothing else** — no host probing, no parent inspection, no libsystemd call,
   so the `systemd`-less build (`withSystemd = false` in
   `contrib/package.nix`) behaves identically apart from the feature gate.

Precedence, with the observed result:

| Invocation | Result |
|------------|--------|
| `ananicy-rs start` | **Auto** — enabled iff evidence *and* not a transient scope |
| `ananicy-rs --systemd start` | Forced on, even inside a scope |
| `ananicy-rs --no-systemd start` | Forced off |
| `ananicy-rs --systemd --no-systemd` | Parse error, exit code 2 |

`--systemd` stays a valueless flag so existing command lines such as
`ExecStart=ananicy-rs --systemd start` keep parsing; an optional-value form like
`--systemd=auto` would swallow the `start` argument.

The decision is reported at `debug` level and by the `debug cgroups`
diagnostic, e.g. `Systemd integration: enabled (auto-detected from
$INVOCATION_ID)`. Without the `systemd` cargo feature the mode is always off and
the status reads `disabled (built without the systemd feature)`.

### What the mode does and does not control

| Effect | Where |
|--------|-------|
| Logs through the journald *native* protocol instead of stderr; silently falls back to stderr if the journal socket is unreachable | `startup::init_logging` (`src/startup.rs`) |
| Sends `READY=1` after cgroup creation and worker start-up | `runtime.rs` |
| Sends `STOPPING=1` on `SIGINT`/`SIGTERM` | `signals.rs` |
| Suppresses the "daemon mode not implemented" warning for foreground service runs | `main.rs` |

It does **not** influence cgroup handling. Delegation is discovered
independently from the process' own cgroup (see below), and cgroup writes are
gated by `CgroupOwnership`, not by this mode. Conversely, running as a service
does not by itself grant delegation: a unit without `Delegate=yes` is refused
just like a manual run.

## Journal Integration

There are two ways to talk to journald, and systemd makes both available:

* **The stream** — stdout/stderr are connected to the journal (the default
  `StandardOutput=journal-or-kmsg`). The daemon's `fmt` tracing layer writes
  here; the output looks like ordinary stderr output in the journal.
* **The native protocol** — a datagram socket carrying structured key/value
  fields. `$JOURNAL_STREAM` (since 231) tells a process that its output *is*
  connected to the journal, so it can upgrade to the native protocol and attach
  structured metadata.

Two caveats from `systemd.exec(5)` that matter in practice:

* `$JOURNAL_STREAM` holds the *device and inode* of the connection file
  descriptor, and the manual explicitly warns that it is "generally not
  sufficient to only check whether `$JOURNAL_STREAM` is set at all", because a
  service may `exec` a child that replaces stdout without unsetting it. The
  daemon only checks presence, which is safe here because the native-protocol
  layer degrades to stderr on any failure.
* Journald's socket is an `AF_UNIX` datagram socket and the process connector is
  `AF_NETLINK`, which is why the unit allows exactly those two address families
  (see [Hardening](#hardening-in-the-shipped-unit)).

The daemon builds a `tracing-journald` layer when the mode is on; if the layer
cannot be created it keeps the stderr layer, so a host without a reachable
journal socket degrades instead of failing.

## Status Notifications (`sd_notify`)

`sd_notify(3)` sends a datagram to the socket named in `$NOTIFY_SOCKET`. The
unit controls who may talk to it with `NotifyAccess=`:

| `NotifyAccess=` | Accepts notifications from |
|-----------------|----------------------------|
| `none` (default) | Nobody — all messages are ignored |
| `main` | The main process only; implied for `Type=notify`, `Type=notify-reload` and `WatchdogSec=` |
| `exec` | Main and control processes spawned from any `Exec*=` |
| `all` | Any member of the service's control group |

Consequences:

* **The shipped unit has `Type=simple` and no `NotifyAccess=`, so
  `NotifyAccess=none`.** The daemon still sends `READY=1` and `STOPPING=1`; both
  are discarded. This is intentional: switching to `Type=notify` would make
  systemd *wait* for `READY=1` before considering the service started, which
  changes start-up semantics for no functional gain here.
* `$NOTIFY_SOCKET` is therefore *not* exported for the shipped unit, which is
  why `$INVOCATION_ID` (since 232) is the primary detection signal and why
  hosts older than 232 need an explicit `--systemd`.
* `sd_notify()` returns immediately if `$NOTIFY_SOCKET` is unset or empty, which
  is why an empty variable is treated as "not set" by the detector.
* Attribution requires the sending process to still exist when PID 1 processes
  the message, or to be runtime-tracked by the manager. The daemon notifies from
  its main process, so this holds.

Watchdogs (`WatchdogSec=` / `WATCHDOG=1`) are not used; `ananicy-rs` is an
event-driven daemon with no natural periodic tick.

## cgroup v2 and Delegation

### systemd owns the cgroup tree

On cgroup v2 the kernel enforces a single-writer model per subtree, and a domain
cgroup may not have member processes *and* enabled domain controllers in
`cgroup.subtree_control` at the same time (the "no internal process" rule).
That is why a resource manager cannot both keep applying limits to a service's
own processes and hand the same subtree to the service to organise itself.
systemd's own wording for `DelegateSubgroup=`:

> Since no processes should live in inner nodes of the control group tree it is
> almost always necessary to run the main ("supervising") process of a unit that
> has delegation turned on in a subgroup.

`Delegate=` (`systemd.resource-control(5)`, since **218**) is the hand-off:

> …the service manager will refrain from manipulating control groups or moving
> processes below the unit's control group, so that a clear concept of ownership
> is established: the control group tree at the level of the unit's control
> group and above is owned and managed by the service manager, while the control
> group tree below the unit's control group is owned and managed by the unit
> itself.

Three details that are easy to get wrong:

* `Delegate=yes` does not only skip interference, it **enables all supported
  controllers for the unit**, making them available for the unit to manage.
* Because the hierarchy is hierarchical, delegated controllers are enabled for
  the **parent and sibling units** too. Availability propagates *up*; the
  parent's `cgroup.subtree_control` decides whether a *given leaf* actually
  gets a `cpu.weight` file. This is the usual reason a delegated service can
  see `cpu` in `../cgroup.controllers` yet find no `cpu.weight` in a leaf: the
  controller has not been enabled on the parent.
* `DelegateSubgroup=` (since **254**) places the main process in a subgroup of
  the unit cgroup. Control processes from `ExecReload=` and similar always go
  into a subgroup named `.control`.

Systemd's resource-control settings map to kernel attributes as follows; the
left column is what a rule uses, the right column is what ends up in cgroupfs:

| Unit setting | cgroup v2 attribute | Notes |
|--------------|---------------------|-------|
| `CPUWeight=` | `cpu.weight` | Range 1–10000, kernel default 100 |
| `CPUQuota=` | `cpu.max` | `<quota> <period>` |
| `MemoryMax=` | `memory.max` | |

Writing cgroupfs at all requires the mount to stay writable, hence
`ProtectControlGroups=no` in the shipped unit.

### What the daemon does with delegation

`ananicy-platform`'s `cgroup::ownership` module discovers the delegated root
from the process' own cgroup (`discover_delegated_root`):

* The unified (`0::`) line of `/proc/self/cgroup` gives the path; the mount point
  is joined onto it and writability of `cgroup.procs` is used as a heuristic that
  the subtree is actually handed over.
* **A `.scope` leaf is refused outright.** Running `sudo ananicy-rs` from a
  desktop terminal puts the daemon in the terminal's scope, and root can write to
  *any* `cgroup.procs` — so writability is not evidence of delegation there.
  Adopting the scope would make the daemon move PIDs into cgroups systemd owns.
* On a cgroup **v1** host there is no unified line, so no delegated root is
  discovered and cgroup mutations stay off. Process-level tuning (`nice`,
  `ionice`, `oom_score_adj`, `latency_nice`) is unaffected by cgroup support.
* In a container with its own cgroup namespace the process' path is `/`, so a
  writable container cgroup root becomes the delegated root — which is how the
  daemon manages cgroups there. A read-only container root leaves cgroup
  management off, and process-level tuning continues to work.

`CgroupOwnership` then gates every write; the full allow/deny matrix is in
[Configuration and Rules](./CONFIGURATION.md#cgroups-v2-delegation-and-ownership).

## Hardening in the Shipped Unit

`data/ananicy-rs.service.in` hardens the daemon aggressively. The directives that
are *not* obvious, and why they are set the way they are:

| Directive | Value | Why |
|-----------|-------|-----|
| `ProtectControlGroups` | `no` | Set to `yes` it mounts the cgroup hierarchies read-only. The daemon needs cgroupfs writable, so the default `no` is spelled out explicitly in the unit |
| `PrivateUsers` | `no` | With user-namespace remapping, other processes appear as `nobody`/`overflowuid` and `/proc` ownership checks break |
| `ProtectProc` | `default` | `default` means *no* restrictions. `invisible` hides other users' processes, and `systemd.exec(5)` states it "cannot be used for services that need to access metainformation about other users' processes" — which is this daemon's entire purpose. (Root also bypasses this option regardless.) |
| `ProcSubset` | `pid` | Hides `/proc` files unrelated to process management, e.g. `/proc/sys`. The daemon reads per-PID files, and probes `latency_nice` support by calling `sched_setattr(2)` on its own thread and restoring the previous value, not by reading `/proc/sys` |
| `PrivateNetwork` | `no` | The netlink process connector (`NETLINK_CONNECTOR`) is scoped to a network namespace; in a private one it would only see the daemon's own processes. The eBPF backend is host-wide, and `--manual-scanning` polls procfs, so either can substitute — but the default netlink backend cannot run in a private netns |
| `RestrictAddressFamilies` | `AF_UNIX`, `AF_NETLINK` | `AF_UNIX` for the journal socket, `AF_NETLINK` for the process connector |
| `RestrictNamespaces` | `cgroup` | Blocks cgroup-namespace creation, which the daemon never needs. `Delegate=yes` does not require it: managing cgroupfs means `mkdir` + file writes, not `unshare(CLONE_NEWCGROUP)` |
| `NoNewPrivileges` | `yes` | Compatible with the capabilities below; nothing in the daemon needs privilege escalation |
| `MemoryDenyWriteExecute` | `yes` | The daemon is plain Rust with no JIT |
| `CapabilityBoundingSet` | `CAP_SYS_NICE`, `CAP_SYS_RESOURCE`, `CAP_DAC_READ_SEARCH`, `CAP_SYS_ADMIN`, `CAP_DAC_OVERRIDE` | What setting `nice`/`ionice`/`oom_score_adj`/`latency_nice` and cgroupfs actually needs; the bounding set drops everything else |
| `Nice` / `OOMScoreAdjust` | `-5` / `-999` | Protects the daemon itself from starvation and the OOM killer |
| `MemoryHigh` / `MemoryMax` | `16M` / `64M` | The daemon is idle most of the time; a runaway allocation should not take the system with it |
| `SuccessExitStatus` | `143` | `128 + SIGTERM`, so a deliberate `systemctl stop` is not recorded as a failure |
| `StartLimitIntervalSec` / `StartLimitBurst` | `60` / `5` | Break restart loops instead of thrashing (both belong in `[Unit]` on systemd ≥ 230, which is where the unit has them) |

None of these hide `/proc/self/cgroup` or the journald socket, so detection and
delegation discovery work identically under the shipped hardening. Verify on
your own host with:

```bash
sudo ananicy-rs debug cgroups
```

## Reload Semantics

`systemctl reload` runs the unit's `ExecReload=`, which is
`ananicy-rs --reload`. That command does not do the work itself: it signals the
running daemon (via the IPC semaphore) and exits, so the *running* process
re-reads its configuration.

| Change | Applied by |
|--------|-----------|
| `loglevel`, `log_applied_rule`, other `apply_*` flags | Live, on reload |
| `ananicy.conf` values that are re-read | Live, on reload |
| `.rules`, `.types`, `.cgroups` | Restart required |
| `check_freq` | Restart required — it is captured when the manual scanner starts |

If a reload fails to parse, the previous configuration stays active and the error
is logged; the daemon does not fall back to defaults.

## NixOS Packaging

`contrib/module.nix` follows the same rule: the packaged unit
(`data/ananicy-rs.service.in`, shipped by the flake) is the single place where
`Delegate=yes`, hardening, `ExecReload=` and the restart policy are defined, and
the module makes it available with `systemd.packages`. The module only forces
`ExecStart=` so that `services.ananicy-rs.extraArgs` can be injected in front of
`start`; it does not redefine delegation or hardening.

`extraArgs` defaults to `[]` because auto-detection makes `--systemd`
unnecessary; use `[ "--no-systemd" ]` to force the mode off. Because a NixOS
system can have both a package-provided unit and a module-generated one, confirm
which unit is in effect before debugging:

```bash
systemctl cat ananicy-rs.service | grep -E 'Delegate|ExecReload|ExecStart'
```

## Verifying a Deployment

```bash
# Which unit is active, and with which settings
systemctl status ananicy-rs.service
systemctl cat ananicy-rs.service

# Was the unit delegated, and where does the process sit?
systemctl show ananicy-rs.service -p Delegate -p Type -p NotifyAccess
cat /proc/$(pidof ananicy-rs)/cgroup

# The daemon's own view: integration mode, unit name, cgroup
sudo ananicy-rs debug cgroups

# Logs (only meaningful when the native protocol is in use)
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

Common symptoms:

| Symptom | Cause | Fix |
|---------|-------|-----|
| `disabled (no systemd service manager detected)` on a real unit | Host is older than systemd 232, so no `$INVOCATION_ID` is exported | Pass `--systemd` via `extraArgs` |
| `disabled (member of a transient .scope, not a service)` when started from a terminal | Manual run inherits the session scope's environment | Run it as a service |
| `WARN Cgroup v2: Detected manual execution inside a transient .scope` | Manual run; delegation would hijack the session's cgroup | Run it as a service with `Delegate=yes` |
| Rule logged as applied but CPU weight unchanged | The parent cgroup has not enabled the `cpu` controller, so no `cpu.weight` file exists in the leaf | Enable the controller on the parent, or accept the DEBUG-level optional skip |
| Daemon output never reaches `journalctl` | The systemd mode was off, so the logs went to stderr — the journal only captures that if the unit's stdio is connected to it (the default) | Check the reported mode with `ananicy-rs debug cgroups`; force it with `--systemd` |
| Journal entries lack structured fields | The stderr stream layer was used instead of the native protocol, because the mode was off or `$JOURNAL_STREAM` was unset | Same as above; then `journalctl -u … -o verbose` shows the native-protocol fields |
| `READY=1` never appears in the journal | It is a notification, not a log line, and is discarded with `NotifyAccess=none` | Expected; see [Status Notifications](#status-notifications-sd_notify) |

## Hosts Without systemd

Nothing requires systemd. On OpenRC, `runit`, `s6`, BusyBox `init` or a plain
container, auto-detection finds no evidence and the daemon logs to stderr.

The only thing lost is delegation *provided by systemd*. A writable cgroup root
in `/proc/self/cgroup` — which is what most containers and most non-systemd
inits give the process — is still used as the delegated root, so cgroup rules
keep working there. On a cgroup v1 host, or where that root is read-only, cgroup
rules are skipped while process-level tuning continues. `--no-systemd` makes the
logging choice explicit.

## Testing

`src/systemd.rs` unit-tests the resolver without a service manager: service
supervision, user service, systemd inside a container, manual invocation, each
evidence source in isolation, evidence ordering, partial and empty environments,
the scope veto, explicit precedence, and `/proc/self/cgroup` parsing for the
unified, hybrid and legacy hierarchies. `tests/cli.rs` covers flag acceptance,
mutual exclusion and that the *environment* (not the host) drives the decision —
the ambient environment is neutralised so the suite does not depend on the
developer's init system.

Not covered automatically, and why: a root (system) service instance and the
packaged unit under `Type=simple` need root and a real `systemctl`; `Type=notify`
units are not used by the shipped unit; hosts older than systemd 232 are covered
by the documented `--systemd` override.

## Further Reading

* `systemd.exec(5)` — the environment variables and all `Protect*=` options
* `systemd.service(5)` — `Type=`, `NotifyAccess=`, `ExecStart=`, `ExecReload=`
* `systemd.resource-control(5)` — `Delegate=`, `DelegateSubgroup=`, `CPUWeight=`, `CPUQuota=`
* `systemd.unit(5)` — unit types, `[Install]`, `StartLimit*`
* `sd_notify(3)`, `sd_pid_get_unit(3)`, `sd_listen_fds(3)`
* `cgroup-v2(7)` — the unified hierarchy, the no-internal-process rule
* <https://systemd.io/CGROUP_DELEGATION> — the delegation model, and the
  explicit note that the unit ⇄ cgroup path mapping is not public API
