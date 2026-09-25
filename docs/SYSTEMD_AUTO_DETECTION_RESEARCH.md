# systemd Integration: Research and Auto-Detection Report

This document records how `--systemd` was investigated, which detection
mechanisms were evaluated, why the selected mechanism was chosen, and what
the resulting semantics are.

## 1. Current behaviour: what `--systemd` actually does

`--systemd` is *not* a cgroup switch and it does *not* mean "systemd is PID 1".
It is a single boolean that gates three things in the binary:

| Location | Effect when true |
|----------|------------------|
| `src/startup.rs:47` | Logging goes through `tracing_journald` (native journal protocol) instead of the `fmt` layer on stderr. Falls back silently to stderr if `/run/systemd/journal/socket` is unreachable. |
| `src/runtime.rs:68` | `sd_notify(READY=1)` is sent after cgroup creation and worker start-up. A no-op when `$NOTIFY_SOCKET` is unset. |
| `src/signals.rs:59` | `sd_notify(STOPPING=1)` is sent on `SIGINT`/`SIGTERM`. |
| `src/main.rs:69` | The "daemon mode not implemented" warning is suppressed (a foreground run under a service manager is expected). |

Everything else is independent of the flag:

* cgroup delegation is discovered from the process' own cgroup in
  `ananicy_platform::cgroup::ownership::discover_delegated_root`, and refuses
  transient scopes;
* cgroup writes are gated by `CgroupOwnership`, not by the flag;
* the only libsystemd call, `sd_pid_get_unit()` in
  `ananicy_platform::service::get_unit_name()`, is used solely by the
  `debug cgroups` diagnostic.

So the semantic meaning of the flag is: **"this process is supervised by a
service manager; talk to it (journald + status notifications)"**. That is the
question auto-detection has to answer, and it is deliberately *not* the
question "is systemd installed/running on this host".

Before this change the binary also treated a bare `$NOTIFY_SOCKET` as evidence
(`src/main.rs`: `args.systemd || var("NOTIFY_SOCKET").is_ok()`). That heuristic
is **wrong for the common case**: systemd only exports `$NOTIFY_SOCKET` when
`NotifyAccess=` is not `none` (see `service_exec_needs_notify_socket()` in
`src/core/service.c`), and the default for `Type=simple` is `none`. Both
shipped unit files (`data/ananicy-rs.service.in`, and the NixOS module) run
`Type=simple`, so the previous auto-detection never fired for them.

## 2. Detection candidates

### 2.1 systemd environment variables

`systemd.exec(5)` documents the variables a service manager exports to the
processes it spawns. Their preconditions matter:

| Variable | Exported when | Usable as "supervised"? |
|----------|---------------|-------------------------|
| `$INVOCATION_ID` | every process of an *active* unit, all unit types and all `Type=` values, systemd ≥ 232 ("The same ID is passed to all processes run as part of the unit") | Yes — the broadest signal |
| `$NOTIFY_SOCKET` | only when `NotifyAccess=` ≠ `none`; `Type=simple` defaults to `none`; systemd ≥ 229 | Yes, but incomplete |
| `$JOURNAL_STREAM` | only when stdout/stderr are connected to journald, systemd ≥ 231 | Weak hint |
| `$SYSTEMD_EXEC_PID` | systemd ≥ 248, the PID of the directly exec'd process | Redundant (always set together with `$INVOCATION_ID`) |
| `$MANAGERPID` | processes spawned by a *user* manager, systemd ≥ 208 | Redundant |
| `$LISTEN_FDS` / `$LISTEN_PID` | socket-activated services only | Useless here |
| `$MAINPID` | control processes only (`ExecReload=` and similar) | Useless here |

Caveats taken into account:

* `$INVOCATION_ID` is a *generic* interface by design — upstream systemd
  discussion (#11660) notes anyone may set it, and the commit that introduced
  it calls it generic. Practically, only systemd sets it in the wild, but it
  can be forged or leaked.
* Environment variables are **inherited**. A terminal started by the desktop
  (`kitty`, `gnome-terminal`, …) is itself a systemd unit, so its
  `$INVOCATION_ID` is inherited by the shell, by `sudo`-less root shells, and
  by the daemon. Inherited evidence therefore does not imply "this process is
  the unit's main process". This is not hypothetical: it reproduced on the
  development host (a `kitty-…scope`) and initially broke the project's own
  tests, which assert on stderr output.

### 2.2 systemd runtime presence

`/run/systemd/system` (what `sd_booted()` checks) only proves that a systemd
instance booted the kernel. It is true for every process on the host,
including manual runs. Upstream is explicit that `sd_booted()` "only checks
that systemd is pid1" and "says nothing about the program itself". Rejected
as a positive signal; it is only useful to *disqualify* evidence, which the
chosen design does not need.

### 2.3 Process ancestry

`/proc/<pid>/stat`/`status` can only see the direct parent. `systemd → ananicy-rs`
gives `PPID == 1`, while `systemd → sh/sudo → ananicy-rs` gives a shell or
`sudo` PID, and a manual run gives a terminal. PID 1 is also not necessarily
systemd (containers, `runit`, `s6`, BusyBox init), and matching a *process
name* is inherently racy. No systemd-stable API offers "am I the exec'd
process" other than `$SYSTEMD_EXEC_PID` (systemd ≥ 248 only). Rejected.

### 2.4 cgroup v2 path

`/proc/self/cgroup` is authoritative for *where* a process sits and, for a
systemd-managed service, its leaf is `<name>.service`. Reliability caveats:

* the unit ⇄ cgroup path mapping is explicitly *not* public API
  (`systemd.io/CGROUP_DELEGATION`, "⚡ Currently, the algorithm for mapping
  between slice/scope/service unit naming and their cgroup paths is not
  considered public API");
* nothing may be assumed about `/system.slice/`: units can be placed in any
  slice, `Slice=` may be customised, and user services live under
  `/user.slice/user-$UID.slice/…`;
* containers may expose a delegated subtree with no unit names at all;
* `Delegate=yes` (used by this project) plus systemd ≥ 254 `DelegateSubgroup=`
  place the process *below* the unit cgroup;
* the kernel guarantees the process is in *a* cgroup, never that this cgroup
  is a service.

Conclusion: a cgroup path is not a usable *positive* signal. It is, however, a
precise *negative* signal for one specific case — the process is a member of a
transient `.scope` (interactive session, desktop app scope, `systemd-run
--scope`), which is a systemd unit that nobody asked to start the daemon in.
The project already relies on exactly this notion in
`cgroup/ownership.rs:71`, which refuses cgroup mutation for `*.scope` paths.

### 2.5 libsystemd APIs

`sd_pid_get_unit()` exists and the project already links libsystemd (feature
`systemd`). It is authoritative but *not* sufficient: its implementation
(`src/basic/cgroup-util.c`) is `cg_pid_get_unit()` → read `/proc/<pid>/cgroup`
→ `cg_path_get_unit()` → skip slices → take the next segment and validate it
as a unit name. It therefore answers "which unit's cgroup subtree am I in",
**not** "which unit am I". For a process in
`…/user@1000.service/app.slice/kitty-133224-0.scope` it returns
`user@1000.service` (verified on the development host), so a `.service`
suffix test on its output still misclassifies interactive sessions. It also
does not see user units (`sd_pid_get_user_unit()` is a separate call) and
cannot distinguish a service from a scope.

No new dependency was added for detection: `$INVOCATION_ID` needs nothing but
`std::env`, which keeps the logic testable on hosts without libsystemd and
keeps the non-`systemd` builds (`contrib/package.nix` with
`withSystemd = false`) unchanged.

## 3. Reliability analysis

| Method | Detects systemd host | Detects systemd **service** | Works through wrapper (`sh`, `sudo`) | Works in container | False-positive risk |
|---|---|---|---|---|---|
| `/run/systemd/system` (`sd_booted()`) | yes | **no** | no | yes | high: true for every process on a systemd host |
| `INVOCATION_ID` | yes | yes, all `Type=` | yes (inherited) | yes, incl. systemd-in-container | medium: inherited from interactive scopes, forgeable by any process |
| `NOTIFY_SOCKET` | yes | only when `NotifyAccess=` ≠ `none` (not for `Type=simple`) | yes (inherited) | yes | low, but incomplete |
| `JOURNAL_STREAM` | yes | only when stdio is journald | yes (inherited) | yes | low, weak signal |
| cgroup `.service` leaf | yes | yes, but mapping is private API; breaks in delegated subtrees, custom slices, containers | yes (inherited cgroup) | only for systemd-in-container | medium; no positive evidence at all on non-systemd hosts, and useless for a service moved into a sub-cgroup |
| parent PID / comm | yes | no (`sh`, `sudo`, `machinectl shell`, …) | no | no | high, racy |
| `sd_pid_get_unit()` | yes | approximates "enclosing unit" (scope member → `user@N.service`), misses user units | yes | yes | medium, plus link-time dependency |

Combined rule used by this implementation:

1. **Evidence** — `$INVOCATION_ID`, else `$NOTIFY_SOCKET`, else
   `$JOURNAL_STREAM`. Empty values count as unset (matching `sd_notify()` and
   `sd_listen_fds()`), so `Environment=INVOCATION_ID=` cannot force the mode on.
2. **Scope veto** — if that evidence exists but the process' own cgroup leaf is
   a `*.scope`, the process is an interactive/session member, not a service, so
   the mode stays off. The cgroup is only read when evidence exists, so manual
   runs pay no `/proc` cost.
3. **Nothing else is consulted** — no host probing, no parent inspection, no
   libsystemd call.

Residual risks and their consequences:

* *Forged/stale* `$INVOCATION_ID` (e.g. `env INVOCATION_ID=x ananicy-rs
  start`, or an `ExecStart=` wrapper that exports it) can enable the mode. The
  only consequences are journald logging and a `READY=1`/`STOPPING=1` datagram
  that a non-`notify` unit ignores; cgroup safety is unaffected because that
  logic is driven by `CgroupOwnership`, not by this mode.
* systemd < 232 does not export `$INVOCATION_ID`; such hosts can be handled
  with `--systemd` (still supported).
* A deliberate `systemd-run --scope -p Delegate=yes -- ananicy-rs start`
  deployment is classified as "not a service" (scopes are not services);
  `--systemd` overrides it. The scope also has no cgroup delegation support in
  `ownership.rs`, so this is consistent with the existing safety model.
* `--systemd` inside a scope also forces the mode on (explicit override wins),
  which preserves the pre-change behaviour of the flag for anyone who relied on
  it in such setups.

## 4. Chosen design

`src/systemd.rs` — a dependency-free, testable module:

* `SystemdRequest` = `Auto | Enabled | Disabled` (from `--systemd`/`--no-systemd`).
* `SystemdEnvironment` = the injectable inputs
  (`invocation_id`, `notify_socket`, `journal_stream`, `in_transient_scope`),
  filled by `SystemdEnvironment::from_process()`.
* `resolve(request, env) -> SystemdMode` = pure function, fully unit-tested.
* `SystemdMode` = `Forced | Enabled(evidence) | Disabled(reason)` with a
  `description()` used for the `debug` log line and the `debug cgroups` output.

Why this matches `ananicy-rs`: the flag's entire effect is service-manager
integration, the environment variables are exactly what a service manager hands
to the processes it supervises (systemd's documented, versioned contract), and
the scope veto removes the one systematic false positive that inheritance
creates. The result works identically in system services, user services,
containers with their own systemd, containers without systemd, and on non-systemd
hosts, and it never depends on `/system.slice` conventions.

Configuration file support (`systemd = auto|true|false` in `ananicy.conf`) was
considered and **rejected**: the mode is a property of how the process was
launched, not of the tuning configuration, and the CLI already provides an
explicit override for unusual environments. Adding a key would also create a
second precedence layer to reason about.

## 5. CLI / precedence rules

| Invocation | Result |
|------------|--------|
| `ananicy-rs [start]` | auto: enabled iff service-manager evidence and not a transient scope |
| `ananicy-rs --systemd [start]` | always enabled (unchanged behaviour) |
| `ananicy-rs --no-systemd [start]` | always disabled |
| `ananicy-rs --systemd --no-systemd` | parse error, exit code 2 |

`--systemd` deliberately stays a valueless flag so that existing command lines
such as `ExecStart=ananicy-rs --systemd start` keep parsing; an optional-value
form such as `--systemd=auto` would have consumed the `start` argument.

Feature gating is unchanged: without the `systemd` cargo feature the mode is
always off, and the reported status says
`disabled (built without the systemd feature)`.

## 6. NixOS impact

`contrib/module.nix`:

* `extraArgs` default changed from `[ "--systemd" ]` to `[ ]`;
* `ExecStart` is now built with `lib.concatStringsSep " "`, so the rendered
  command is `/nix/store/…-ananicy-rs/bin/ananicy-rs start` instead of
  `… ananicy-rs --systemd start`. The leading `""` entry that clears the
  packaged unit's `ExecStart=` is preserved, and `extraArgs` overrides (e.g.
  `[ "--no-systemd" ]`) still land in front of `start`.

Verified by evaluating the module:

```
execStart = [ "" "/nix/store/…-ananicy-rs/bin/ananicy-rs start" ]
extraArgs = [ ]
Delegate  = unset (comes from the packaged unit via systemd.packages)
Type      = unset (⇒ Type=simple)
NotifyAccess = unset (⇒ none)
```

No other module behaviour was touched. Note that `Type=simple` means systemd
does not export `$NOTIFY_SOCKET`; detection therefore relies on
`$INVOCATION_ID` (systemd ≥ 232). `Delegate=`, `ProtectSystem=`,
`PrivateUsers=`, `PrivateMounts=`, `PrivateDevices=` and
`RestrictAddressFamilies=` are unchanged and none of them hide
`/proc/self/cgroup` or the journald socket from the service. `Type=notify` was
deliberately *not* added: that would change start-up semantics (systemd would
wait for `READY=1`, which is currently sent before the event monitor is
running) and is out of scope here.

## 7. Compatibility

* `ananicy-rs --systemd` keeps working exactly as before, including on
  systemd < 232 hosts, in scopes, and in containers.
* `ananicy-rs` (no flags) now auto-detects. The observable change is limited to
  logging destination and service notifications; cgroup behaviour is untouched.
* Existing unit files (`data/ananicy-rs.service.in`, distro packaging, custom
  units) keep working: they never needed `--systemd`, and they may keep passing
  it.
* Manual runs are *less* likely to be misclassified than with the old
  `$NOTIFY_SOCKET` heuristic, because the scope veto covers the interactive
  session case that `$INVOCATION_ID` alone would have caught.
* `services.ananicy-rs.extraArgs` keeps its type and meaning; only its default
  changed. Users who set `extraArgs = [ "--systemd" ]` explicitly are
  unaffected.

## 8. Testing

Automated (no systemd required):

* `src/systemd.rs` unit tests: service supervision, user service, systemd in a
  container, manual invocation, container without systemd, each evidence source
  in isolation, partial environments and evidence ordering, scope veto,
  explicit enable/disable precedence, `/proc/self/cgroup` parsing for unified,
  hybrid and legacy hierarchies, malformed input, and scope recognition for
  services, delegated subtrees and the root cgroup.
* `tests/cli.rs`: CLI flag acceptance, mutual exclusion (exit 2), the reported
  mode for forced modes, empty-variable handling, and that the environment —
  not the host — drives auto-detection. The ambient environment is neutralised
  so the suite does not depend on the developer's init system.

Manual procedure used on a real systemd host (also reproducible by reviewers):

```bash
# As a service: detected
systemd-run --user --wait --pipe --unit=ananicy-detect -- \
  ./target/debug/ananicy-rs debug cgroups | grep 'Systemd integration'
# → Systemd integration: enabled (auto-detected from $INVOCATION_ID)

# As a scope: vetoed
systemd-run --user --scope --unit=ananicy-detect-scope -- \
  ./target/debug/ananicy-rs debug cgroups 2>/dev/null | grep 'Systemd integration'
# → Systemd integration: disabled (member of a transient .scope, not a service)

# From an interactive terminal (inherits the scope's $INVOCATION_ID)
./target/debug/ananicy-rs debug cgroups | grep 'Systemd integration'
# → Systemd integration: disabled (member of a transient .scope, not a service)

# Explicit overrides
env -u INVOCATION_ID ./target/debug/ananicy-rs --systemd debug cgroups | grep 'Systemd integration'
# → Systemd integration: enabled (forced by --systemd)
INVOCATION_ID=x ./target/debug/ananicy-rs --no-systemd debug cgroups | grep 'Systemd integration'
# → Systemd integration: disabled (forced by --no-systemd)
```

Not covered by automated tests, and why:

* a *system* (root) service instance and the packaged unit under
  `Type=simple` — needs root and a real `systemctl`; the user-manager
  equivalent above exercises the same code path;
* `Type=notify` units — the shipped unit is `Type=simple`, so `READY=1` is
  accepted and dropped by systemd in both the old and the new code path;
* hosts with systemd < 232 (no `$INVOCATION_ID`) — the fallback is the
  documented `--systemd` override.
