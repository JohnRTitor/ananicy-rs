# CLI and Usage

`ananicy-rs` is a daemon that runs in the background. It modifies process properties and therefore requires root privileges.

## Running Manually

You can start `ananicy-rs` manually for testing:

```bash
sudo ananicy-rs start
```

> [!WARNING]
> When running manually from a terminal, Cgroup v2 modifications may be disabled if the daemon detects it is running inside a transient desktop scope (e.g., `app-*.scope`). This is a safety mechanism to prevent hijacking the terminal's cgroup. To fully utilize cgroups safely, you must run the daemon as a systemd service (see below).

## Systemd Service (Recommended)

For production use, run `ananicy-rs` as a systemd service. The unit enables `Delegate=yes` for the service's own cgroup subtree and other parameters; it does not delegate user/session scopes or arbitrary systemd units.

Enable and start the daemon:

```bash
sudo systemctl enable --now ananicy-rs.service
```

Reload the global configuration without a full restart:

```bash
sudo systemctl reload ananicy-rs.service
# OR
sudo ananicy-rs --reload
```

## Systemd Integration and Auto-Detection

`ananicy-rs` decides at start-up whether to integrate with a service manager
(native journald logging, plus `READY=1`/`STOPPING=1` status notifications).
No flag is required: the decision is derived from what a service manager handed
to *this* process, never from the fact that the host runs systemd.

| Invocation | Result |
|------------|--------|
| `ananicy-rs start` | **Auto** — enabled when the process is supervised by a systemd *service*, disabled otherwise |
| `ananicy-rs --systemd start` | Always enabled (never auto-disable) |
| `ananicy-rs --no-systemd start` | Always disabled, even when detected |

Auto-detection enables the mode when one of these environment variables is set
and non-empty, and the process' own cgroup is not a transient `*.scope`:

| Evidence | Set by | Notes |
|----------|--------|-------|
| `$INVOCATION_ID` | systemd ≥ 232 for every process of an active unit | Primary signal; covers all unit types and all `Type=` values, including `Type=simple` |
| `$NOTIFY_SOCKET` | systemd ≥ 229, only when `NotifyAccess=` ≠ `none` | Covers `Type=notify`/watchdog units and systemd 229–231 |
| `$JOURNAL_STREAM` | systemd ≥ 231, when stdout/stderr are wired to journald | Fallback |

Consequences of that design:

* **As a service** (system or user instance, including inside a container that
  runs its own systemd) the mode is enabled automatically, also when the unit
  runs the daemon through a wrapper such as `ExecStart=/bin/sh -c 'ananicy-rs
  start'` or `sudo`.
* **Manual invocations are not affected by the host using systemd.** A terminal
  started by a desktop is itself a systemd scope, so it inherits that scope's
  `$INVOCATION_ID`; the daemon therefore treats "member of a transient
  `.scope`" as *not* a service and keeps logging to stderr.
* **Non-systemd hosts** (OpenRC, runit, s6, BusyBox init, plain containers) and
  **containers on a systemd host** have no service-manager evidence and stay in
  non-systemd mode.
* **systemd < 232** does not export `$INVOCATION_ID`; use `--systemd` there.
* Being a service only controls logging and notifications. Cgroup delegation is
  still derived from the process' own cgroup (see
  [Cgroups v2 Delegation and Ownership](./CONFIGURATION.md#cgroups-v2-delegation-and-ownership)).

The decision is reported at `debug` level and by the `debug cgroups`
diagnostic, e.g. `Systemd integration: enabled (auto-detected from
$INVOCATION_ID)`. See
[SYSTEMD_AUTO_DETECTION_RESEARCH.md](./SYSTEMD_AUTO_DETECTION_RESEARCH.md) for
the full rationale.

## Command Line Arguments

`ananicy-rs` supports the following CLI arguments and subcommands:

### Options

- `--systemd`: Always run as a systemd service; skips auto-detection and sets up `sd_notify`.
- `--no-systemd`: Never use systemd integration, even when auto-detected. Mutually exclusive with `--systemd`.
- `--daemon`: Run in daemon mode (currently warns and runs in the foreground).
- `--config <CONFIG>`: Override the default config file path (default: `/etc/ananicy.d/ananicy.conf`).
- `--config-dir <CONFIG_DIR>`: Override the rules directory (default: `/etc/ananicy.d`).
- `--reload`: Send a signal to the running `ananicy-rs` instance (via an IPC semaphore) to reload global configuration and the active log level. Rule, type, and cgroup files require a daemon restart.
- `--force-remove-semaphore`: Force remove the IPC semaphore (use only if the daemon crashed and left a stale semaphore).
- `--manual-scanning`: Enable periodic manual procfs scanning (useful if event listeners miss events).
- `--benchmark`: Run the daemon in benchmark mode for performance profiling.
- `--benchmark-count <BENCHMARK_COUNT>`: Number of iterations to run in benchmark mode.
- `--bpf-min-us <BPF_MIN_US>`: Minimum microseconds for BPF intervals.
- `-v, --verbose`: Enable verbose output.

The active `loglevel` and per-event application flags are reloaded live. `check_freq` is captured when the manual scanner starts, so changing it requires a daemon restart; rule, type, and cgroup files also require a restart.

### Commands

- `start`: Start the daemon (this is the default behavior if no command is specified, but explicitly using `start` is supported).
- `dump <sub_action>`: Dump internal state. Sub-actions include:
  - `rules`: Dump parsed rules.
  - `types`: Dump parsed types.
  - `cgroups`: Dump parsed cgroups.
  - `proc`: Dump process information cache.
  - `autogroup`: Dump autogroup status.
- `debug cgroups`: Dump diagnostic information about the system's cgroup mounts.
- `completions <shell>`: Generate shell completions (supported shells: `bash`, `zsh`, `fish`, `powershell`).

Example:

```bash
sudo ananicy-rs dump rules
```

## Shell Completions

Generate shell completions and output them to stdout so you can redirect them into the appropriate completion path for your shell.

### Bash

```bash
ananicy-rs completions bash > ~/.local/share/bash-completion/completions/ananicy-rs
```

### Zsh

```zsh
ananicy-rs completions zsh > ~/.zfunc/_ananicy-rs
```

### Fish

```fish
ananicy-rs completions fish > ~/.config/fish/completions/ananicy-rs.fish
```

### PowerShell

```powershell
ananicy-rs completions powershell > ananicy-rs.ps1
```
