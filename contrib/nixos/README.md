# Nix / NixOS packaging

Two separate things live here, and it is worth keeping them apart:

- **[`package.nix`](package.nix)** is the *software*: a `rustPlatform.buildRustPackage`
  derivation for the `ananicy-rs` daemon. It is what `nix build` and `nix flake check`
  in this repository build, and it is what a nixpkgs-style overlay or a
  `pkgs.callPackage` would consume.
- **[`module.nix`](module.nix)** is the *NixOS integration*: a
  `services.ananicy-rs` module that declares the unit, wires up
  `environment.etc."ananicy.d"`, and makes the package easy to configure. It is
  not part of the package and nothing in `package.nix` depends on it.

`module.nix` already existed in this repository before the recipes were added; it
was moved here from `contrib/module.nix` when `contrib/` was organised by
distribution. Only the location changed. **No new NixOS module was written as
part of the packaging work** — the repository's existing functionality already
justifies the one that is here, and inventing a second one would have been
redundant.

```nix
# the package, consumed directly
pkgs.callPackage ./contrib/nixos/package.nix { }

# or, from a flake
nix build github:JohnRTitor/ananicy-rs
nix run github:JohnRTitor/ananicy-rs
```

## Build and check it

```bash
nix build              # -> ./result
nix flake check        # builds the package as a check
nix build .#ananicy-rs # same derivation, explicit attribute
nix develop            # shell with cargo, rustfmt, clippy, clang, libbpf
nixfmt --check contrib/nixos/*.nix flake.nix default.nix
```

`nix build` and `nix flake check` were both run while this recipe was moved and
the documentation installed, and both pass. They were already wired into
[`.github/workflows/nixos.yml`](../../.github/workflows/nixos.yml); that
workflow's path filters now include `contrib/**`, so a change here triggers it.

`nixfmt --check` reports `contrib/nixos/module.nix` and
`contrib/nixos/package.nix` as unformatted. Both were already unformatted before
the move — the deviations are in `buildFeatures` and the `postInstall`
concatenation, neither of which this work touched — and reformatting them would
have put unrelated churn in the diff. `nix flake check` is what enforces the
expression, and it passes.

## What goes in the derivation

| Path | From |
| --- | --- |
| `$out/bin/ananicy-rs` | `make install` |
| `$out/lib/systemd/system/ananicy-rs.service` | `make install`, from `data/ananicy-rs.service.in` |
| `$out/share/bash-completion/completions/ananicy-rs.bash` | `installShellCompletion` |
| `$out/share/fish/vendor_completions.d/ananicy-rs.fish` | `installShellCompletion` |
| `$out/share/zsh/site-functions/_ananicy-rs` | `installShellCompletion` |

The binary and the unit come from `make install DESTDIR= PREFIX=$out`, the same
rule the Fedora, Debian and Arch recipes use, so all four ship the same bytes.
`CARGO_TARGET_DIR` is passed explicitly because the Makefile's
`target/<profile>` path does not account for a cross-compilation `--target`
subdirectory, which `buildRustPackage` uses.

`meta.license = lib.licenses.gpl3Only` is the licence metadata; the store path
does not carry a copy of `LICENSE`.

## Dependencies

| Input | Why |
| --- | --- |
| `elfutils`, `zlib`, `zstd` | `libbpf`'s own link-time dependencies, which nixpkgs keeps separate |
| `llvmPackages.clang` | compiles the BPF C |
| `pkg-config` | the `pkg-config` probe |
| `libbpf` | `bpf` feature |
| `systemdLibs` | the `systemd` feature, conditional on it being available |
| `installShellFiles` | the completions |

`strictDeps = true` and `__structuredAttrs = true` are set, so a missing input is
a build failure rather than a link against whatever happens to be in the store.

`rustPlatform.bindgenHook` is in `nativeBuildInputs`. Nothing in the dependency
graph actually needs bindgen — `libbpf-rs` is used with
`default-features = false`, so `libbpf-sys` uses its pre-generated bindings — but
removing it was out of scope for a relocation and it is harmless.

## Source and reproducibility

`src` is a `lib.fileset.toSource` of the repository root with every `.nix` file
dropped, so the derivation does not vendor the nix files that describe it.
`cargoLock.lockFile` points at the committed `Cargo.lock`, which is what
`importCargoLock` uses to fetch the dependencies at a pinned hash. The build
therefore does not need a lockfile-derived `cargoHash`, and there is no
hand-maintained dependency list to drift.

`version` is not hard-coded: `flake.nix` reads `[workspace.package] version` out
of `Cargo.toml` and appends the short git revision, which is what produces store
paths like `ananicy-rs-0.1.0+78ac5b4`. `package.nix` defaults `version` to
`unstable` for callers that do not pass one, and takes `withBpf` and
`withSystemd` as arguments.

## Features

`buildNoDefaultFeatures = true`, then `netlink`, plus `bpf` and `systemd` where
available. The feature set is stated explicitly rather than inherited, so a
change to the default set upstream is a visible diff here.

This is the one recipe under `contrib/` that does build the `bpf` feature, and
that is a deliberate difference rather than an oversight:

- nixpkgs makes the eBPF toolchain hermetic and cheap, so there is no reason not
  to;
- `withBpf` and `withSystemd` are already parameters, so a downstream packager
  can turn either off.

The other three recipes build the default feature set only. See
[contrib/README.md](../README.md) for why.

## The test suite

`cargo test` runs as part of the build, with one test skipped:

```
--skip=test_set_affinity_on_current_process
```

A test that calls `sched_setaffinity` on the test process itself cannot have its
mask changed under the Nix sandbox's restrictions, so it is excluded rather than
made to pass vacuously. The other three recipes run the same test, because a
Fedora mock, an sbuild chroot and a makepkg chroot do not impose that sandbox.

`hardeningDisable = [ "zerocallusedregs" ]` is also a sandbox artefact: Rust
1.85 began emitting `zero_caller_used_regs` in function attributes, which trips
the kernel hardening check in the Nix sandbox.

## systemd

`make install` puts the unit at `$out/lib/systemd/system/ananicy-rs.service`.
That path is the store path, not the system, so on NixOS the module is what
actually creates the system unit.

The module does that by adding the package to `systemd.packages`, which puts
`$out/lib/systemd/system` on systemd's unit search path, and then defining
`systemd.services."ananicy-rs"` with a single forced `ExecStart` so that
`extraArgs` can be injected. Everything else — `Delegate=yes`, the hardening,
`ExecReload=`, the restart policy — is read from the packaged unit itself, so
there is exactly one copy of those decisions in the tree. The module does not
weaken any of it.

Because a NixOS system can end up with both a package-provided and a
module-generated unit, `docs/SYSTEMD.md` says to check which one is in effect
before debugging:

```bash
systemctl cat ananicy-rs.service | grep -E 'Delegate|ExecReload|ExecStart'
```

For a non-NixOS Nix system, enabling the unit means symlinking it into
`/etc/systemd/system`, which is a manual step. Nix deliberately does not patch
store paths, so there is no `systemctl preset` equivalent.

## Configuration

`/etc/ananicy.d` is not a thing a Nix package can create. The module assembles it
with `environment.etc."ananicy.d"`, and it takes its default rules from
`services.ananicy-rs.rulesProvider`, which defaults to `pkgs.ananicy-cpp`
because `ananicy-rs` ships no rules of its own.

For the derivation outside NixOS, the daemon reads `/etc/ananicy.d` at run time
and the paths are overridden with `--config` / `--config-dir` or the
`ANANICY_RS_CONF` / `ANANICY_RS_CONFDIR` environment variables. There is no
`postInstall` that writes to `/etc`: that would be a build writing outside its
own output, which no Nix derivation may do.
