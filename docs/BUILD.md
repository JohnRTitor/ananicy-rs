# Building ananicy-rs

This is the canonical description of how to build `ananicy-rs`: what has to be
installed, which feature pulls in which native library, how the release and Nix
builds differ from a local one, and what each build failure actually means.

For how the *test suite* is organised, see [TESTING.md](./TESTING.md). For
running the daemon, see [CLI.md](./CLI.md).

## Requirements

### Host

Linux only. The daemon is built on cgroup v1/v2, netlink, procfs and BPF
syscalls, and nothing in the tree compiles on another operating system.

### Toolchain

| | |
| --- | --- |
| Minimum | **Rust 1.85** |
| Edition | 2024 (all crates) |
| Install | [rustup](https://rustup.rs/) |

1.85 is the first release that supports edition 2024, which every crate here
uses; there is no `rust-version` field in any manifest, so cargo will not warn
you if you are too old — the error is a parse failure on `edition = "2024"`.

### Native libraries

Three crates in `Cargo.lock` link a system library: `pcre2-sys`, `libbpf-sys`,
and `ananicy-platform` itself, which declares `#[link(name = "systemd")]` in
`src/service.rs` under the `systemd` feature (`linux-raw-sys`,
`core-foundation-sys`, `js-sys`, `web-sys` and `windows-sys` are raw syscall
bindings and non-Linux targets, and link nothing).

The systemd one is easy to miss. `sd-notify` and `tracing-journald` are pure Rust
and talk to the daemon over the `$NOTIFY_SOCKET` and journald stream sockets —
but `systemd` is **on by default**, and it is `ananicy-platform`, not those two
crates, that links `libsystemd.so`. A default build without libsystemd's
development files will not link. You can see it in the result:

```console
$ readelf -d target/release/ananicy-rs | grep NEEDED
 0x0000000000000001 (NEEDED)  Shared library: [libsystemd.so.0]
 0x0000000000000001 (NEEDED)  Shared library: [libpcre2-8.so.0]
 0x0000000000000001 (NEEDED)  Shared library: [libgcc_s.so.1]
 0x0000000000000001 (NEEDED)  Shared library: [libm.so.6]
 0x0000000000000001 (NEEDED)  Shared library: [libc.so.6]
```

Only the `bpf` feature's `libbpf-sys` is avoidable, and only by dropping
`systemd` as well: `--no-default-features --features netlink` needs neither
`libsystemd` nor the eBPF toolchain, at the cost of `sd_notify` readiness,
journald logging and the unit-name detection in `src/systemd.rs`.

`pcre2` is worth separating out, because it is the one that is easy to get
wrong. `pcre2-sys` first asks `pkg-config` for `libpcre2-8`, and only builds its
own vendored copy if that fails. Both paths work, but they are not the same
build: the vendored copy is compiled with `SUPPORT_JIT=1` forced and linked
statically. `contrib/nixos/package.nix` puts `pcre2` in `buildInputs`, so `nix build`
links the shared library, and a host without `libpcre2-8` silently produces a
binary with a different regex engine than the one that ships. Install the
development package so that a local build matches the Nix package.

#### Packages

Package names are not mechanical translations of each other, and the difference
is where mistakes happen: Debian ends development packages in `-dev` and keeps
the library's `lib` prefix, while Fedora ends them in `-devel` and moves the
prefix to the front of the base name — so `libpcre2-dev` is `pcre2-devel`, not
`libpcre2-devel`. Arch uses the upstream names unchanged, except for
`systemd-libs`, which is the package carrying `libsystemd.so.0`.

| Needed for | Debian / Ubuntu | Fedora / RHEL | Arch | Nix |
| --- | --- | --- | --- | --- |
| libsystemd (default features) | `libsystemd-dev` | `systemd-devel` | `systemd-libs` | `systemdLibs` |
| PCRE2 (recommended) | `libpcre2-dev` | `pcre2-devel` | `pcre2` | `pcre2` |
| libbpf (`bpf` feature) | `libbpf-dev` | `libbpf-devel` | `libbpf` | `libbpf` |
| BPF toolchain (`bpf` feature) | `clang` | `clang` | `clang` | `llvmPackages.clang` |
| `pkg-config` probe | `pkg-config` | `pkgconf-pkg-config` | `pkgconf` | `pkg-config` |
| `rustfmt`, if `bpf` is on | `rustfmt` | `rustfmt` | *in `rust`* | — |

`libbpf-dev` / `libbpf-devel` pulls in `libelf` and `zlib`, which `libbpf-sys`
links by name (`-lbpf -lelf -lz`); on Nix those are separate, so the package
expression lists `elfutils`, `zlib` and `zstd` itself. Arch has no separate
`rustfmt` package — its `rust` package carries the whole toolchain.

Install everything a default build needs:

```bash
# Debian / Ubuntu
sudo apt-get install pkg-config libpcre2-dev libsystemd-dev

# Fedora
sudo dnf install pkgconf-pkg-config pcre2-devel systemd-devel

# Arch
sudo pacman -S pkgconf pcre2 systemd-libs
```

And everything on top of that for the `bpf` feature:

```bash
# Debian / Ubuntu
sudo apt-get install clang libbpf-dev libelf-dev zlib1g-dev

# Fedora
sudo dnf install clang libbpf-devel elfutils-libelf-devel zlib-devel

# Arch
sudo pacman -S clang libbpf
```

## Features

| Feature | Default | Effect |
| --- | --- | --- |
| `netlink` | yes | Process events from the netlink connector |
| `systemd` | yes | `sd_notify` readiness and journald logging |
| `bpf` | no | eBPF process events, via the `ananicy-bpf` crate |

`netlink` and `bpf` are the two event sources, and the binary cannot be built
without one of them: `src/main.rs` opens with

```rust
#[cfg(not(any(feature = "bpf", feature = "netlink")))]
compile_error!("At least one event source feature ('bpf' or 'netlink') must be enabled.");
```

so `--no-default-features` on its own is not a valid build. They are
alternatives rather than layers — the daemon subscribes to one, not both — and
enabling both is supported if you want to compare them.

`netlink` gates the monitor in the binary (`src/monitor.rs`); inside
`ananicy-platform` it is only a marker feature, since that crate always
compiles the netlink code. `systemd` gates the unit-detection code in
`ananicy-platform/src/service.rs`. Neither costs anything at build time: they
select code paths and pull in pure-Rust dependencies.

`bpf` is the only feature with native consequences, because it is the only one
that compiles the `ananicy-bpf` crate, and that crate's build script shells out
to clang, pkg-config and rustfmt.

## Workspace layout, and the one trap in it

```
ananicy-rs            the binary (default member)
crates/ananicy-core   rules engine, config, worker
crates/ananicy-platform  the Linux layer
crates/ananicy-bpf    eBPF backend        <- member, but NOT a default member
crates/ananicy-platform/fuzz   fuzz targets (a separate workspace)
```

`ananicy-bpf` is in `members` but not in `default-members`. That is deliberate:
its build script links libbpf, so including it in a default build would make
`cargo build` and `cargo test` fail on a host that has the daemon's
dependencies but not the eBPF toolchain — even with the default feature set,
which does not use the crate at all. The reason is recorded in `Cargo.toml`
next to the list.

The consequence to internalise:

- `cargo build`, `cargo test` — builds the three default members. No eBPF
  toolchain needed.
- `cargo build --workspace`, `cargo test --workspace` — also builds
  `ananicy-bpf`. **Needs everything in the table above**, even with no features
  enabled, because the crate's own build script runs whether or not anything
  uses it.
- `--features bpf` or `--all-features` — same requirement.

## Building

```bash
# Default: netlink + systemd
cargo build --release

# With the eBPF backend
cargo build --release --features bpf

# The whole workspace, including ananicy-bpf
cargo build --workspace --all-features

# Reproducible: fail rather than update Cargo.lock
cargo build --locked
```

`Cargo.lock` is committed and CI builds with `--locked`, so use it locally too;
otherwise a build that quietly resolves a newer dependency is not the build
anyone else is testing.

### The release profile

`[profile.release]` is tuned, and the tuning is visible in the output:

| Setting | Value | Why it matters |
| --- | --- | --- |
| `lto` | `"fat"` | Whole-program optimisation across crates |
| `codegen-units` | `1` | Required for LTO to be effective |
| `panic` | `"abort"` | No unwinding tables |
| `strip` | `true` | Symbols already stripped by cargo |
| `opt-level` | `3` | |

Fat LTO with one codegen unit makes `--release` markedly slower than a debug
build. `strip = true` means cargo already strips the release binary, so the
`strip` step in `release.yml` is redundant but harmless; the Makefile does not
strip at all.

Note that `panic = "abort"` applies to the binary that ships. Cargo ignores the
setting for the `test` and `bench` profiles, which need unwinding for the test
harness, so `cargo test --release` still works.

## Installing

```bash
sudo make install
```

installs the binary to `/usr/bin/ananicy-rs` and the systemd unit to
`/usr/lib/systemd/system/ananicy-rs.service`. The unit is *generated*: the
Makefile substitutes `@bindir@` in `data/ananicy-rs.service.in`. To install
somewhere else:

```bash
make install PREFIX=/usr/local
make install DESTDIR=/tmp/staging          # packaging
make install DESTDIR=/tmp/staging PREFIX=/usr
```

`DESTDIR` and `PREFIX` compose, so a staged install for a package lands in
`$DESTDIR$PREFIX/bin`. `make install` builds nothing — run `make build` first,
or let `cargo` produce the binary some other way, because the Makefile only
copies `target/<profile>/ananicy-rs`. `DEBUG=1` selects the debug profile and
makes the source path match.

`make install` also writes the generated unit to `data/ananicy-rs.service` in
the source tree before installing it. That file is a build product, so it is in
`.gitignore` now; a `make install` from before that change may still have left
one behind.

`contrib/nixos/package.nix` installs the same way, via `make install DESTDIR= PREFIX=$out`,
and additionally generates shell completions with `installShellCompletion`.

## Nix

The flake builds the same package from `contrib/nixos/package.nix`.

```bash
nix build                # ./result
nix run .                # run the daemon
nix build .#ananicy-rs   # same package, explicit attribute
nix flake check          # build the package as a check
nix develop              # shell with cargo, rustfmt, clippy, clang, libbpf, pcre2
```

`nix build` runs the test suite, skipping one test that cannot pass in the
sandbox:

```
--skip=test_set_affinity_on_current_process
```

A system test that calls `sched_setaffinity` on the test process itself cannot
have its mask changed under the sandbox's restrictions, so it is excluded
rather than made to pass vacuously.

The Nix build differs from a local one in three ways worth knowing:

- **Features.** `buildNoDefaultFeatures = true`, then `netlink`, plus `bpf` and
  `systemd` where available. The feature set is explicit rather than inherited.
- **PCRE2.** `pcre2` is in `buildInputs`, so pkg-config finds `libpcre2-8` and
  the shared library is linked. See the note above — this is what a local build
  should match.
- **Hardening.** `hardeningDisable = [ "zerocallusedregs" ]`. Rust 1.85 started
  emitting `zero_caller_used_regs` in function attributes, which trips the
  kernel hardening check in the Nix sandbox.

### NixOS module

`contrib/nixos/module.nix` provides `services.ananicy-rs`:

```nix
{
  inputs.ananicy-rs.url = "github:JohnRTitor/ananicy-rs";

  outputs = { nixpkgs, ananicy-rs, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        ananicy-rs.nixosModules.default
        { services.ananicy-rs = { enable = true; }; }
      ];
    };
  };
}
```

`ananicy-rs` does not ship its own rules, so the module takes them from
`rulesProvider`, which defaults to `pkgs.ananicy-cpp` — set it if you keep rules
somewhere else. `environment.etc."ananicy.d"` is assembled from that provider
plus `settings`, `extraRules`, `extraTypes` and `extraCgroups`.

The module does not re-declare the unit. It adds the package to
`systemd.packages`, so `Delegate=yes`, the hardening, `ExecReload=` and the
restart policy are all read from `data/ananicy-rs.service.in` by systemd, and the
module forces only `ExecStart=` so that `extraArgs` can be injected. There is
one copy of those decisions in the tree. See [SYSTEMD.md](./SYSTEMD.md) for
what the unit does and why.

## Distribution packaging

`contrib/` holds one native packaging recipe per distribution:

| Distribution | Recipe | Artefact |
| --- | --- | --- |
| Fedora | `contrib/fedora/` | RPM, via `rpmbuild` |
| Debian and derivatives | `contrib/debian/` | `.deb`, via `dpkg-buildpackage` |
| Arch Linux | `contrib/archlinux/` | `.pkg.tar.zst`, via `makepkg` |
| Nix and NixOS | `contrib/nixos/` | store path, via `nix build` |

They are **community-maintained recipes**, not packages carried by any of those
distributions; see [contrib/README.md](../contrib/README.md) for what that does
and does not claim, and each directory's README for how to build and validate it.

All of them install the binary and the unit by running `make install`, so the
files a user gets are the ones this project produces, in the same place, with the
same modes. None of them installs a configuration file, because upstream has none
and the daemon writes `/etc/ananicy.d/ananicy.conf` itself on first start. None
of them enables or starts the service on install.

Two things a local build does not have to think about, and the recipes do:

- **The version lives in one place.** `[workspace.package] version` in
  `Cargo.toml`. The Fedora and Debian recipes read the manifest and fail the
  build on a mismatch; the Arch recipe derives it from the release tag.
- **Dependencies must be reachable without a repository.** None of the four
  workspace crates is on crates.io, so the Fedora and Debian recipes ship a
  vendored copy of `Cargo.lock` as part of the source package and build
  `--offline`. The Arch recipe runs `cargo fetch` in `prepare()`; Nix uses
  `cargoLock.lockFile`.

`contrib/debian/` is the one directory that is not usable in place: Debian
requires `debian/` to be the top-level directory of a source package, so
`cp -r contrib/debian debian` before building. See
[contrib/debian/README.md](../contrib/debian/README.md) for the full flow,
including the vendoring step.

## Continuous integration

Five workflows, all under `.github/workflows/`:

| Workflow | Trigger | What it does |
| --- | --- | --- |
| `ci.yml` | `.rs`, `Cargo.toml`, `Cargo.lock` | Builds and tests on stable and nightly; repeats both on Fedora and Arch |
| `lint.yml` | `.rs`, `Cargo.toml` | `cargo fmt --check` on nightly, `cargo clippy -D warnings` on stable |
| `nixos.yml` | Rust files, `*.nix`, `Makefile`, `contrib/**` | `nix flake check` and `nix build`, which is also what builds `contrib/nixos` |
| `packaging.yml` | the above plus `data/**`, `contrib/**` | Builds the RPM, the `.deb` and the Arch package in their own container images |
| `release.yml` | `v*` tags | Builds, strips, tars, and publishes a release, plus a reproducible source tarball |

Every action is pinned to a commit SHA rather than a tag, and Rust setup is
funnelled through one composite action, `.github/actions/setup-rust`, which
installs sccache, then the toolchain, then the native dependencies. sccache is
configured per job rather than per workflow, because `RUSTC_WRAPPER` is only
valid where sccache is actually on `PATH`; the `matrix-distro` jobs run in
containers that do not have it. CI also requests the `rustfmt` component
explicitly — see below for why that is not optional.

The distro matrix builds `--all-features` in `fedora:latest` and
`archlinux:latest`, so it needs clang, pcre2, libbpf and pkg-config in the
image, under that distribution's own package names.

`packaging.yml` installs only the `BuildRequires` each recipe declares, so a
recipe that grew an unnecessary dependency, or dropped a necessary one, fails
the job. The Debian job builds as an unprivileged user, which is what
`Rules-Requires-Root: no` in `debian/control` claims. The Arch job publishes the
checkout as a local git remote carrying the release tag and points makepkg at
that, so it builds the tree under test rather than whatever is published; only
the source URL differs from the `PKGBUILD` on disk.

Each of those three jobs installs `git` **before** `actions/checkout`, and the
order is load-bearing. `actions/checkout` looks for git 2.18 or newer on `PATH`
and, not finding it, logs "The repository will be downloaded using the GitHub
REST API" and extracts a plain working tree with no `.git` at all — silently,
because the checkout itself succeeds. The Fedora and Debian jobs then fail on
`git archive` and the Arch job on `git tag`, all with the same unhelpful "not a
git repository" and exit 128. The dependency step has to come second because
the jobs install `git` in the image they are already running; putting it in the
`BuildRequires` list is not enough. The step that uses git re-checks with
`git rev-parse --git-dir` and names the cause, so a regression says what is
wrong instead of leaving a 128 to interpret.

Note that the path filters of `ci.yml` and `lint.yml` do not include
`.github/**`, so a change to one of those two workflows alone will not trigger a
run. `packaging.yml`, `nixos.yml` and `release.yml` list themselves.

## Troubleshooting

| Error | Cause |
| --- | --- |
| `error: 'rustfmt' is not installed for the toolchain 'nightly-…'` | `rustfmt` missing. See below; almost always a build of `ananicy-bpf`. |
| `Failed to build BPF skeleton. Needs clang, a pkg-config …` | One of the three tools the `bpf` crate needs is missing or unusable. The cause line above the panic says which step failed. |
| `failed to generate skeleton … Caused by: Failed to rustfmt` | rustfmt is missing or unusable for the active toolchain. |
| `error: failed to run custom build command for 'libbpf-sys'` | libbpf's development files are missing, or `pkg-config` cannot find them. |
| `No match for argument: libpcre2-devel` | Fedora's package is `pcre2-devel`. Debian's is `libpcre2-dev`. |
| `failed to run custom build command for 'ananicy-bpf'` | clang missing, or it cannot target BPF. |
| `the crate 'ananicy-bpf' … requires libbpf` | `--workspace` or `--all-features` on a host without the eBPF toolchain. Build the default members instead. |
| `error: '…' requires rustc 1.85 or newer` | Edition 2024. Upgrade the toolchain. |
| `At least one event source feature ('bpf' or 'netlink') must be enabled.` | `--no-default-features` with no replacement. Add `--features netlink` or `--features bpf`. |

### Why `rustfmt` is a build requirement

This one surprises people, because rustfmt is a formatter and not a compiler, and
it is easy to install a toolchain without it.

`ananicy-bpf`'s build script calls libbpf-cargo's `SkeletonBuilder`, which
does three things: compile the BPF C to an object file, generate a Rust skeleton
from it, and **format the generated skeleton**. The last step is not optional.
In `libbpf-cargo`, `gen::gen_single` ends with

```rust
file.write_all(&try_rustfmt(&contents, rustfmt_path)?)?;
```

and `try_rustfmt` spawns `rustfmt`, pipes the generated source through it, and
turns a non-zero exit into a hard error. `SkeletonBuilder::rustfmt()` only
accepts a path to a *different* binary; there is no way to skip the step.

So `rustfmt` is required exactly when `ananicy-bpf` is compiled, which is when
`--features bpf` or `--all-features` is used, or when `--workspace` is passed
— the crate is a workspace member, so `--workspace` builds it even with no
features enabled. It is a build-time requirement only; the daemon does not need
rustfmt at runtime.

The failure is easy to misread, because rustup installs toolchains with a
*minimal* profile, which omits rustfmt, so a freshly installed toolchain does
not have it. A toolchain that came with a distribution or a runner image often
does, which is why this reproduces on `nightly` and not on `stable` — and why
`stable` appearing to work is not evidence that it does. Ask for the component
explicitly:

```bash
rustup component add rustfmt                      # the default toolchain
rustup component add rustfmt --toolchain nightly  # a specific one
```

`cargo` has no `component` subcommand; the `+toolchain` syntax that works for
`cargo build` does not work here.

`bpftool` is **not** one of the three tools. libbpf-cargo generates the skeleton
through libbpf's own in-process BTF API rather than by shelling out to the
`bpftool` binary, so a machine with clang, pkg-config, libbpf and rustfmt can
build the crate without it.

## Benchmarks

`benches/rules.rs` uses `criterion` with `harness = false`:

```bash
cargo bench
```

## Fuzzing

The fuzz targets are a separate workspace under
`crates/ananicy-platform/fuzz/` and need nightly plus `cargo-fuzz`:

```bash
cargo install cargo-fuzz
cargo +nightly fuzz run parse_mounts
```

See [TESTING.md](./TESTING.md) for what each target covers and why the
workspace is split.
