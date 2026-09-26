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
- **[systemd Reference](docs/SYSTEMD.md)**: How systemd supervises a service, what it hands to a process, cgroup v2 delegation, and what the shipped unit's hardening does.
- **[CPU Topology and Affinity](docs/TOPOLOGY.md)**: Details on CPU pinning, `big.LITTLE` detection, and AMD X3D support.
- **[Building](docs/BUILD.md)**: Build requirements and native dependencies, features, the release profile, installing, Nix, and what each build error means.
- **[Compatibility with the Reference Implementation](docs/COMPATIBILITY.md)**: Every behavioural difference between `ananicy-rs` and the C++ reference, plus what was verified equivalent, what this daemon adds, and the two things it lacks.
- **[Testing](docs/TESTING.md)**: What the test suite verifies, which tests need a live system, and how the tests relate to the C++ daemon.

## Status

**Alpha / Experimental**

The project is currently under active development. While it supports loading rules, cgroups v1/v2, CPU topology detection, and both Netlink and BPF event backends, it is continuously being stabilized towards behavioral compatibility with the mature reference version. The differences that remain, and the evidence around them, are in [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md).

## Requirements

### Runtime Requirements

- **Root privileges**: Required for modifying process attributes, cgroups, and mounting BPF programs.
- **systemd**: Optional, but recommended for service management.
- **cgroup v2** (or v1): Required for the cgroup functionalities.

Building needs Linux and Rust 1.85 or newer; the `bpf` feature additionally
needs clang, libbpf, PCRE2 and rustfmt. See **[Building](docs/BUILD.md)** for the
full dependency list and per-distribution package names.

## Installation

### 1. Build from Source

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

See **[Building](docs/BUILD.md)** for the Nix build, the module's options, and
how to install elsewhere with `PREFIX`/`DESTDIR`.

## License

This project is licensed under the **GNU General Public License v3.0 (GPL-3.0)**. See the [LICENSE](LICENSE) file for the complete text.
