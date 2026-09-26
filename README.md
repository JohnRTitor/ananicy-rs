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
- **[Building](docs/BUILD.md)**: Build requirements and native dependencies, features, the release profile, installing, distribution packaging, Nix, and what each build error means.
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

Building needs Linux and Rust 1.88 or newer, plus the development files for
libsystemd and PCRE2 — the default feature set links both. The `bpf` feature
additionally needs clang, libbpf and rustfmt. See
**[Building](docs/BUILD.md)** for the full dependency list and
per-distribution package names.

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

### 2. Distribution packages

Packaging recipes are available under `contrib/`:

| Distribution | Recipe | Artefact |
| --- | --- | --- |
| Fedora | [`contrib/fedora/`](contrib/fedora/) | RPM |
| Debian, Ubuntu and other derivatives | [`contrib/debian/`](contrib/debian/) | `.deb` |
| Arch Linux | [`contrib/archlinux/`](contrib/archlinux/) | `.pkg.tar.zst` |
| Nix, NixOS | [`contrib/nixos/`](contrib/nixos/) | store path, plus a `services.ananicy-rs` module |

These are **community-maintained recipes**, not packages carried by any of those
distributions. None of them has been submitted to, or reviewed by, a distribution
package maintainer, so a recipe existing here does not mean the distribution
supports `ananicy-rs`; it means the packaging work has been done and is
available to build, test, patch or submit. See
[contrib/README.md](contrib/README.md) for the details, and each directory's
README for how to build and validate its recipe.

All of them install the same files in the same places, and none of them enables
or starts the service on install:

```bash
sudo systemctl enable --now ananicy-rs.service
```

`ananicy-rs` ships no rules, so install a rule set into `/etc/ananicy.d` before
enabling the service; each recipe's README says where to get one. The daemon
creates `/etc/ananicy.d` and a default `ananicy.conf` on its first start if they
are missing, and no package owns them, so upgrades never touch your changes.

### 3. Nix / NixOS

A `flake.nix` is provided for Nix and NixOS users. You can run the package directly:

```bash
nix run github:JohnRTitor/ananicy-rs
```

To use it as a NixOS module, import `contrib/nixos/module.nix` in your configuration, or add the flake to your inputs and enable the service:

```nix
services.ananicy-rs = {
  enable = true;
};
```

See **[Building](docs/BUILD.md)** for the Nix build, the module's options, and
how to install elsewhere with `PREFIX`/`DESTDIR`.

## License

This project is licensed under the **GNU General Public License v3.0 (GPL-3.0)**. See the [LICENSE](LICENSE) file for the complete text.
