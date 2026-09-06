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

For production use, you should run `ananicy-rs` as a systemd service. The service file implements strict hardening and proper Cgroup v2 delegation.

Enable and start the daemon:

```bash
sudo systemctl enable --now ananicy-rs.service
```

Reload rules and configuration without a full restart:

```bash
sudo systemctl reload ananicy-rs.service
# OR
sudo ananicy-rs --reload
```

## Command Line Arguments

`ananicy-rs` supports the following CLI arguments and subcommands:

### Options

- `--systemd`: Run as a systemd service (automatically sets up `sd_notify`).
- `--daemon`: Run in daemon mode (currently warns and runs in the foreground).
- `--config <CONFIG>`: Override the default config file path (default: `/etc/ananicy.d/ananicy.conf`).
- `--config-dir <CONFIG_DIR>`: Override the rules directory (default: `/etc/ananicy.d`).
- `--reload`: Send a signal to the running `ananicy-rs` instance (via an IPC semaphore) to reload rules and configuration.
- `--force-remove-semaphore`: Force remove the IPC semaphore (use only if the daemon crashed and left a stale semaphore).
- `--manual-scanning`: Enable periodic manual procfs scanning (useful if event listeners miss events).
- `--benchmark`: Run the daemon in benchmark mode for performance profiling.
- `--benchmark-count <BENCHMARK_COUNT>`: Number of iterations to run in benchmark mode.
- `--bpf-min-us <BPF_MIN_US>`: Minimum microseconds for BPF intervals.
- `-v, --verbose`: Enable verbose output.

### Commands

- `start`: Start the daemon (this is the default behavior if no command is specified, but explicitly using `start` is supported).
- `dump <sub_action>`: Dump internal state. Sub-actions include:
  - `rules`: Dump parsed rules.
  - `types`: Dump parsed types.
  - `cgroups`: Dump parsed cgroups.
  - `proc`: Dump process information cache.
  - `autogroup`: Dump autogroup status.
- `debug cgroups`: Dump diagnostic information about the system's cgroup mounts.

Example:

```bash
sudo ananicy-rs dump rules
```
