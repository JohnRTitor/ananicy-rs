# Contributing to ananicy-rs

First of all, thank you for considering contributing to `ananicy-rs`! This project is a Rust rewrite of the original `ananicy-cpp`, aiming for memory safety, better architecture, and lower resource consumption while providing feature parity.

## Development Environment

To start developing `ananicy-rs`, you'll need the following installed on your Linux system:

- **Rust toolchain**: 1.70 or newer (Edition 2024 is used). Install via [rustup](https://rustup.rs/).
- **Linux**: This project heavily relies on Linux-specific APIs (cgroups, netlink, procfs, bpf) and will not build on other operating systems.
- **Optional (for BPF features)**: `clang`, `libbpf`, `elfutils`, `zlib`.

## Building the Project

The project is structured as a Cargo workspace with several crates (`ananicy-core`, `ananicy-platform`, `ananicy-bpf`, and the main `ananicy-rs` bin).

To build the project with the default features (Netlink monitor):
```bash
cargo build
```

To build with BPF support enabled:
```bash
cargo build --features bpf
```

To create a production-ready release build:
```bash
cargo build --release
```

## Running Tests

`ananicy-rs` contains multiple test suites:

- **Unit tests**: Can be run normally as they don't require special privileges.
- **Linux/System tests**: Some tests in `ananicy-platform` interact with cgroups and process namespaces.

To run all basic tests across the workspace:
```bash
cargo test
# or explicitly
cargo test --workspace
```

*Note: Some cgroup-related tests, BPF tests, or affinity tests (e.g., `test_set_affinity_on_current_process`) might require `root` privileges or specific capabilities to succeed. In a normal development cycle, `cargo test` runs the user-space logic validations.*

## Code Quality and Linting

Before submitting a Pull Request, please ensure your code follows the project's formatting and linting rules.

**1. Formatting**
Ensure your code is formatted using `rustfmt`:
```bash
cargo fmt
```

**2. Linting**
Check for common mistakes and idiomatic Rust using `clippy`:
```bash
cargo clippy --all-targets --all-features -- -D warnings
```
(Please fix any warnings that arise).

## Adding a Feature or Rule

*   **Rules**: If you're contributing new community rules, add them under `ananicy.d/`. (Currently rules are often pulled from the upstream `ananicy` repository, but local testing is encouraged).
*   **Linux Features**: If you are modifying scheduler operations, CPU affinity parsing, or cgroups, ensure that your changes fall back gracefully when privileges are missing or when running on unsupported kernel versions.

## Submitting Changes

1.  Fork the repository and create a new branch for your feature or bug fix.
2.  Write clear, concise commit messages.
3.  Ensure your code builds (`cargo check`), formats correctly (`cargo fmt`), passes linting (`cargo clippy`), and passes tests (`cargo test`).
4.  Open a Pull Request describing your changes and why they are needed.

Thank you for your contributions!
