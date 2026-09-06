# ananicy-rs

ANother Auto NICe daemon rewrite in Rust for lower CPU and memory usage.

[![License: GPL v3](https://img.shields.io/badge/License-GPL_v3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)

## Overview

`ananicy-rs` is a Rust rewrite of the [ananicy-cpp](https://gitlab.com/ananicy-cpp/ananicy-cpp) daemon, originally based on [Ananicy](https://github.com/Nefelim4ag/Ananicy). It aims to provide the exact same core functionality—automatically managing process priorities, IO priorities, scheduling classes, and CPU affinity—with improved memory safety, predictability, and efficiency enabled by Rust.

It operates by loading rules for known applications and listening to process creation events (via Netlink or eBPF). When a matching process is spawned, `ananicy-rs` dynamically applies the specified performance tweaks without requiring manual intervention.

## Documentation

Comprehensive documentation is available in the `docs/` directory:

- **[Configuration and Rules](docs/CONFIGURATION.md)**: How to configure the daemon, write rules, types, and cgroup specifications.
- **[CLI and Usage](docs/CLI.md)**: How to run the daemon, command-line arguments, and systemd integration.
- **[CPU Topology and Affinity](docs/TOPOLOGY.md)**: Details on CPU pinning, `big.LITTLE` detection, and AMD X3D support.
- **[Differences from the Reference Implementation](docs/ANANICY_CPP_DIFFERENCES.md)**: Explicit behavioral and implementation differences between `ananicy-rs` and the C++ reference implementation.

## Status

**Alpha / Experimental**

The project is currently under active development. While it supports loading rules, cgroups v1/v2, CPU topology detection, and both Netlink and BPF event backends, it is continuously being stabilized to reach parity with the mature reference version.

## Requirements

### Build-time Requirements

- **Linux** (The daemon heavily relies on Linux-specific APIs).
- **Rust Toolchain**: 1.70 or newer (Edition 2024).
- _Optional (for BPF)_: `clang`, `libbpf`, `elfutils`, `zlib`.

### Runtime Requirements

- **Root privileges**: Required for modifying process attributes, cgroups, and mounting BPF programs.
- **systemd**: Optional, but recommended for service management.
- **cgroup v2** (or v1): Required for the cgroup functionalities.

## Installation

### 1. Build from Source

Clone the repository and build the project using Cargo:

```bash
git clone https://github.com/JohnRTitor/ananicy-rs.git
cd ananicy-rs

# Build with the default Netlink monitor
cargo build --release

# OR build with the optional BPF monitor
cargo build --release --features bpf
```

Install the binary and systemd service:

```bash
sudo make install
```

This will place the binary in `/usr/bin/ananicy-rs` and the systemd unit in `/usr/lib/systemd/system/ananicy-rs.service`.

### 2. Nix / NixOS

A `flake.nix` is provided for Nix and NixOS users. You can run the package directly:

```bash
nix run github:JohnRTitor/ananicy-rs
```

To use it as a NixOS module, import `contrib/module.nix` in your configuration, or add the flake to your inputs and enable the service:

```nix
services.ananicy-rs = {
  enable = true;
};
```

## License

This project is licensed under the **GNU General Public License v3.0 (GPL-3.0)**. See the [LICENSE](LICENSE) file for the complete text.
