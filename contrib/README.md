# Distribution packaging recipes

Native build and packaging recipes for `ananicy-rs`, one directory per
distribution:

| Distribution | Recipe | Build system |
| --- | --- | --- |
| Fedora | [`fedora/`](fedora/) | RPM, `rpmbuild` |
| Debian and derivatives | [`debian/`](debian/) | `debhelper`, `dpkg-buildpackage` |
| Arch Linux | [`archlinux/`](archlinux/) | `makepkg` |
| Nix and NixOS | [`nixos/`](nixos/) | `buildRustPackage`, `nix` |

**These are community-maintained recipes.** They have not been submitted to, or
reviewed by, any distribution's package maintainers. A recipe existing here does
not mean Fedora, Debian, Arch Linux or NixOS support `ananicy-rs`; it means the
packaging work has been done and is available for someone who wants to build,
test, patch or submit it. Each directory's README says the same thing about its
own ecosystem in more detail.

Each recipe has its own README. Start there for build instructions, dependency
rationale and known limitations.

## What every recipe installs

All four produce the same set of files, from the same sources, so a machine
running any of them behaves identically:

| | |
| --- | --- |
| Executable | `/usr/bin/ananicy-rs` |
| systemd unit | `/usr/lib/systemd/system/ananicy-rs.service` |
| Config directory | `/etc/ananicy.d`, empty — Nix has no such thing, see below |
| Completions | bash, fish, zsh |
| Documentation | `README.md`, `CONTRIBUTING.md`, `docs/*.md` |
| Licence | `LICENSE`, installed as licence metadata |

Nix is the one deliberate exception on two points, both consequences of how Nix
works rather than choices: the paths are under the store path instead of `/usr`,
and there is no `/etc/ananicy.d` at all — a Nix derivation may not write outside
its own output, so configuration is the module's job through
`environment.etc`. See [`nixos/README.md`](nixos/README.md).

Every recipe installs the binary and the unit by running the project's own
`make install`, rather than copying files itself. The unit is generated from
`data/ananicy-rs.service.in` in all four cases, so there is one copy of the
daemon's systemd hardening — its `Nice=-5`, `OOMScoreAdjust=-999`,
`Delegate=yes` and `CapabilityBoundingSet` — and no recipe edits it. The only
thing any recipe adds to the unit is how it gets registered with systemd.

**No recipe starts the service on install, and none enables it.** Installing a
package should not begin reprioritising a machine's processes:

```bash
sudo systemctl enable --now ananicy-rs.service
```

## Shared facts, verified rather than assumed

These were read out of the build, not inferred from the source tree:

- The default feature set links exactly one system library, `libsystemd.so.0`. The
  `systemd` feature is on by default and declares `#[link(name = "systemd")]`, so
  libsystemd is a link-time dependency of any default build. The regex engine is
  pure Rust and links nothing, so there is no second system dependency to declare
  and no pkg-config probe whose failure would silently substitute a differently
  configured engine. Every recipe therefore declares libsystemd, and nothing
  else, deliberately.
- `make install` with `PREFIX=/usr` writes exactly two files: the `0755` binary
  at `/usr/bin/ananicy-rs` and the `0644` unit at
  `/usr/lib/systemd/system/ananicy-rs.service`.
- The version is `[workspace.package] version` in `Cargo.toml`. No recipe
  invents its own: the Fedora and Debian recipes read the manifest and fail the
  build on a mismatch, the Arch recipe derives it from the release tag, and
  `flake.nix` reads the same manifest.
- The project ships **no rules and no man pages**. Rules are read at run time
  from `/etc/ananicy.d`, and the daemon writes a default `ananicy.conf` there on
  its first start. No recipe packages a configuration file, so none of them has to
  reconcile one on upgrade.
- Shell completions come from the binary itself
  (`ananicy-rs completions bash|fish|zsh`), which needs no configuration and no
  privileges. The daemon also supports `elvish`, which no recipe installs.

## Features: one deliberate difference

Fedora, Debian and Arch build the **default** feature set — `netlink` plus
`systemd`. The Nix derivation builds `netlink`, `systemd` and `bpf`.

`bpf` is off everywhere else because it is not upstream's default, it needs
`clang`, `libbpf` and `rustfmt` at build time, and it needs `CAP_BPF` and
`CAP_PERFMON` at run time, which the unit in `data/ananicy-rs.service.in` does
not grant. Nix is the exception because nixpkgs makes that toolchain hermetic
and cheap, and because the expression already took `withBpf` as an argument
before this work. Turning it on elsewhere is a real, separate decision, not a
default.

## Where Cargo dependencies come from

The four workspace crates are not published on crates.io, so none of the
distributions' "package every crate as a library" Rust machinery applies. What
each recipe does instead:

| Recipe | Mechanism | Network during build |
| --- | --- | --- |
| Fedora | `Source1` vendor tarball from `make-vendor-archive.sh`, `--offline` | none; Koji has none |
| Debian | `debian/vendor.tar.xz` from `make-vendor.sh`, `--offline`, `CARGO_HOME` in the build tree | none |
| Arch | `cargo fetch --locked` in `prepare()`, then `--frozen` | only in `prepare()` |
| Nix | `cargoLock.lockFile` plus `importCargoLock` | none |

The vendored trees are 197 crates, about 135 MB unpacked and 15 MB compressed.
They are generated on demand, gitignored, and never committed here.

## Continuous integration

[`.github/workflows/packaging.yml`](../.github/workflows/packaging.yml) builds
each recipe in its distribution's own container image, with the distribution's
own packaging tools, so the recipes are checked by actually building the
packages rather than by asserting the files exist. See that workflow for what
each job installs and what it does.

## Adding a recipe

- Keep it self-contained under `contrib/<distribution>/`.
- Do not copy `data/ananicy-rs.service.in`; run `make install` and ship what it
  produces.
- Do not ship a configuration file. Upstream has none, and adding one changes
  the daemon's behaviour.
- Do not touch the unit's hardening. It is upstream's decision, and relaxing it
  to make packaging easier would be a silent change to a privileged service.
- Do not add a dependency on a rule set unless the target distribution actually
  packages one. Neither Fedora nor Debian ships `ananicy-cpp` or the Ananicy
  rule set today, so every recipe here documents where to get rules instead of
  naming a package that cannot be installed.
