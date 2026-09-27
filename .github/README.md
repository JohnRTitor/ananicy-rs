# Continuous integration

Four workflows under [`workflows/`](workflows) and one composite action under
[`actions/setup-rust`](actions/setup-rust). This is what each of them runs and
why the jobs are shaped the way they are: the container images, the unprivileged
test runs and the git-before-checkout step are all load-bearing, and each is
explained where it comes up rather than left as a habit.

For how to build the project at all, see [docs/BUILD.md](../docs/BUILD.md); for
what the suite verifies, see [docs/TESTING.md](../docs/TESTING.md).

## The workflows

| Workflow | Trigger | What it does |
| --- | --- | --- |
| `ci.yml` | `.rs`, `Cargo.toml`, `Cargo.lock`, `*.nix`, `flake.lock` | Builds and tests on stable and nightly; repeats both on Fedora, Debian and Arch, and once more in the Nix devShell |
| `lint.yml` | `.rs`, `Cargo.toml` | `cargo fmt --check` on nightly, `cargo clippy -D warnings` on stable |
| `packaging.yml` | `.rs`, `Cargo.toml`, `Cargo.lock`, `*.nix`, `Makefile`, `data/**`, `contrib/**`, `flake.lock` | Builds the RPM, the `.deb` and the Arch package in their own container images, and `contrib/nixos` with `nix flake check` and `nix build` |
| `release.yml` | `v*` tags | Builds, strips, tars, and publishes a release, plus a reproducible source tarball |

## Pins, Rust setup and superseded runs

Every action is pinned to a commit SHA rather than a tag, and Rust setup is
funnelled through one composite action, `.github/actions/setup-rust`, which
installs sccache, then the toolchain, then the native dependencies. sccache is
configured per job rather than per workflow, because `RUSTC_WRAPPER` is only
valid where sccache is actually on `PATH`; the `matrix-distro` jobs run in
containers that do not have it. CI also requests the `rustfmt` component
explicitly — see [why that is not
optional](../docs/BUILD.md#why-rustfmt-is-a-build-requirement).

`ci.yml`, `lint.yml` and `packaging.yml` each cancel an in-progress run when a
newer push lands on the same ref, so a superseded commit does not keep paying
for a full test matrix. The group is `<workflow>-${{ github.ref }}`, and the
prefix is load-bearing: two runs share a group whenever the name matches, so an
unprefixed `${{ github.ref }}` would have the three workflows cancel each other
rather than their own predecessors. A pull request and the push of the same
commit are two different refs and so do not cancel each other, which is what you
want — the branch result and the PR result are both reported. `release.yml` has
no group: it runs on a tag, where there is nothing to supersede.

## `ci.yml`: the test matrix

The distro matrix builds `--all-features` in `fedora:latest`, `debian:sid` and
`archlinux:latest`, so it needs clang, libbpf and pkg-config in the
image, under that distribution's own package names. `debian:sid` is sid rather
than trixie for the same reason `packaging.yml` uses it: the workspace uses let
chains, which need rustc 1.88, and trixie ships 1.85.1. It is also the only one
of the three images that needs an `apt-get update` before installing, and the
only one whose cargo cannot reach crates.io without `ca-certificates`, which
the image does not carry.

The `nix` job runs the same two commands through `nix develop --command`, and
installs no Rust at all: the toolchain under test is the one the flake pins, so
using the runner's would test something else. The devShell names cargo, rustc,
rustfmt, clippy, libbpf and clang itself and takes `inputsFrom` the package, so
`--all-features` has what its build scripts need. Its test step is
unprivileged, like the packaging jobs and unlike the `test` job above, because
the cgroup tests skip themselves rather than race as root — and `sudo` is not
an option inside a devShell, as it would replace the shell's `PATH` and with it
the cargo being tested. The root case is what the `test` job is for.

## `packaging.yml`: one job per artefact

`packaging.yml` installs only the `BuildRequires` each recipe declares, so a
recipe that grew an unnecessary dependency, or dropped a necessary one, fails
the job. The Debian job builds as an unprivileged user, which is what
`Rules-Requires-Root: no` in `debian/control` claims. The Arch job publishes the
checkout as a local git remote carrying the release tag and points makepkg at
that, so it builds the tree under test rather than whatever is published; only
the source URL differs from the `PKGBUILD` on disk.

`packaging.yml`'s Nix job is the exception to all of that: it runs on the runner
rather than in a container, because the pinned `determinate-nix-action` installs
Nix itself rather than needing a distribution image. `nix flake check` evaluates
the package and the NixOS module together, so that one command covers what
`contrib/nixos` declares.

### git before the checkout

Each of those three container jobs runs an `Install git and trust the workspace`
step **before** `actions/checkout`, and both halves of it are load-bearing:

- **git has to exist when the checkout runs.** `actions/checkout` looks for git
  2.18 or newer on `PATH` and, not finding it, logs "The repository will be
  downloaded using the GitHub REST API" — which extracts a plain working tree
  with no `.git` at all, and reports success doing it. All three images lack
  git, and listing it among the build dependencies is not enough, because that
  step runs after the checkout.
- **The workspace has to be a trusted git directory afterwards.** `/__w` is
  bind-mounted from the host and owned by the runner user while the container
  runs as root, so git refuses it as dubious ownership. `actions/checkout` adds
  its own `safe.directory` entry to a *temporary* `HOME` and then restores
  `HOME`, so nothing downstream inherits it. The same rule applies to the local
  mirror the Arch job clones from, which is why that is handed over to the build
  user rather than just made world-readable.

Both failures look identical from the failing step — "not a git repository" and
exit 128 — so every step that uses git re-checks with `git rev-parse --git-dir`
and names the cause in a `::error::` annotation, with git's own message left on
stderr.

The `matrix-distro` job in `ci.yml` runs `actions/checkout` in those same images
without git installed, and gets the same `.git`-less working tree. Nothing there
calls git afterwards, so it costs that job nothing; it is only a problem for a
job that has to use git, which is why the Nix job runs on the runner instead.

### Tests run unprivileged

All three container jobs in `packaging.yml` run the test suite unprivileged,
which is what their build systems do by construction — `makepkg` refuses to run
as root, and the Debian package declares `Rules-Requires-Root: no`. The Fedora
job has to arrange it, because a bare `rpmbuild -ba` runs as root, and the
cgroup tests in `ananicy-platform` are unreliable as root: three of them share
one cgroup name and each removes it on the way out, so they race each other. It
builds with `--nocheck` and then runs the same `cargo test --locked --offline`
against the same source and vendor tree as an unprivileged user.

## Path filters

`lint.yml`'s path filter does not include `.github/**`, so a change to that
workflow alone will not trigger a run. `ci.yml`, `packaging.yml` and
`release.yml` list themselves.
