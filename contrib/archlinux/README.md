# Arch Linux packaging recipe

`PKGBUILD` is a **community-maintained recipe**. It has not been submitted to or
reviewed by the Arch Linux package maintainers, and its presence here does not
mean Arch Linux supports the package.

- recipe: [`PKGBUILD`](PKGBUILD)
- install message: [`ananicy-rs.install`](ananicy-rs.install)

## Build it

```bash
cp contrib/archlinux/{PKGBUILD,ananicy-rs.install} .
makepkg -f
```

`makepkg` clones the `v<pkgver>` tag into `src/ananicy-rs`, fetches every crate
in `Cargo.lock` in `prepare()`, and builds, tests and installs offline from
there. Needs `base-devel` plus the `makedepends` in the recipe.

To bump to a new release, change `pkgver` and rebuild; `pkgver()` re-derives it
from the tag, so the two cannot disagree silently.

## Static validation

```bash
bash -n PKGBUILD                                     # syntax
makepkg --printsrcinfo                              # metadata
namcap PKGBUILD                                     # if namcap is installed
makepkg -f                                           # the real thing
```

`namcap` 3.6.0 reports nothing about the `PKGBUILD` itself. It did find two real
problems while this recipe was written, and both are fixed:

- `VCS source PKGBUILD needs additional makedepends 'git'` — a VCS checkout
  needs `git` in `makedepends`, so it is there now.
- `Make dependency (systemd-libs) already included as dependency` —
  `makepkg` installs `depends` before `build()` runs, so repeating it in
  `makedepends` is noise. It is only in `depends`.

Every function in the recipe was run against a real checkout of this tree, and
the recipe is also built end to end on every change by
[`.github/workflows/packaging.yml`](../../.github/workflows/packaging.yml) in an
`archlinux:latest` container, with `namcap` re-run there. `pkgver()` returns
`0.1.0` both on the tag and one commit past it; `cargo fetch --locked`,
`cargo build --release --frozen` and `cargo test --frozen` (20 test binaries, 0
failures) all pass as the unprivileged build user; and `package()` produces
exactly the file list above, plus the automatic `ananicy-rs-debug` package that
Arch's default `makepkg.conf` asks for.

`namcap`'s checks on an installed package tarball need `libalpm`, which nixpkgs
does not package, so only its `PKGBUILD` checks were run locally. In the Arch
container both are available.

## What goes in the package

| Path | From |
| --- | --- |
| `/usr/bin/ananicy-rs` | `make install` |
| `/usr/lib/systemd/system/ananicy-rs.service` | `make install`, from `data/ananicy-rs.service.in` |
| `/etc/ananicy.d/` | packaged empty; the daemon writes `ananicy.conf` into it on first start |
| `/usr/share/bash-completion/completions/ananicy-rs` | `ananicy-rs completions bash` |
| `/usr/share/fish/vendor_completions.d/ananicy-rs.fish` | `ananicy-rs completions fish` |
| `/usr/share/zsh/site-functions/_ananicy-rs` | `ananicy-rs completions zsh` |
| `/usr/share/doc/ananicy-rs/` | `README.md`, `CONTRIBUTING.md`, `docs/*.md` |
| `/usr/share/licenses/ananicy-rs/LICENSE` | `LICENSE` |

There are no man pages to install: the project has none.

`make install` is called rather than a hand-rolled install, so the binary, its
mode and the unit are byte-for-byte the same as every other recipe under
`contrib/`. The unit's `@bindir@` placeholder resolves to `/usr/bin`.

## Dependencies

`depends`:

| Package | Why |
| --- | --- |
| `glibc` | `libc.so.6` and `libm.so.6` |
| `libgcc` | `libgcc_s.so.1` |
| `systemd-libs` | `libsystemd.so.0`, recorded by the linker |

`makedepends`:

| Package | Why |
| --- | --- |
| `cargo` | the build |
| `git` | the source is a repository checkout, not an archive |
| `make` | `make install` |
| `pkgconf` | `libbpf-sys` probes for `libbpf` with `pkg-config` |

`systemd-libs` is only in `depends`, which is enough: `makepkg` installs a
package's `depends` before `build()` runs, so it is already present at link time.
It needs no `-dev` package — `ananicy-rs` declares `#[link(name = "systemd")]`
by hand — so `systemd-libs`, not `systemd`, is the right one: it is the package
that carries `/usr/lib/libsystemd.so.0`, and it is a fraction of the size.

The daemon's `#[link(name = "systemd")]` needs the link-time `.so` and nothing
else, which is why no distribution's `libsystemd-dev`/`systemd-devel` headers
are involved either.

`clang`, `libbpf` and `rustfmt` are **not** makedepends, because the `bpf`
feature is not enabled. See the features section below.

Nothing depends on `systemd` as the init system. The daemon detects whether it
is supervised and also runs under OpenRC, runit, s6 or no init at all.

`rust` is not in `makedepends` because Arch ships one rolling toolchain and
`cargo` pulls it in. Nothing needs pinning here, but the floor is real: the
workspace uses let chains, so **rustc 1.88 or newer** is required. Arch has been
well past that for some time, which is why there is no version constraint.

## Source, version and integrity

The source is the release tag,
`git+https://github.com/JohnRTitor/ananicy-rs.git#tag=v$pkgver`, with
`b2sums=('SKIP')`. That is a deliberate choice, not a shortcut:

- no release has been tagged yet, so the tag is the only stable source there is.
  `release.yml` now attaches a reproducible source archive, and this is the
  recipe to switch the moment one exists;
- upstream does not sign its commits or its tags, so there is nothing to verify
  a checkout against.

This is the same shape the official `bat` and `fd` packages use. The trade-off is
real and worth stating: nothing pins the bytes that get built. When a release
exists, switch `source` to

```bash
source=("$pkgname-$pkgver.tar.gz::$url/archive/v$pkgver/$pkgname-$pkgver.tar.gz")
sha256sums=('...')
```

and drop `pkgver()`. `contrib/fedora/` already consumes exactly that tarball.

`pkgver()` reports the release tag, dropping the git-describe suffix and any
dirty marker, because neither is valid in an Arch package version. The
consequence is that two different commits after the same tag report the same
`pkgver`; a real submission would bump `pkgrel` for the second one.

## Features

Default features only: `netlink` plus `systemd` — the netlink connector for
process events, and `sd_notify`/journald integration.

The `bpf` feature is off because it is not upstream's default, it needs `clang`,
`libbpf` and `rustfmt` to build, and it needs `CAP_BPF` and `CAP_PERFMON` at run
time, which the shipped unit does not grant. A `-bpf` split package would be a
runtime opt-in rather than a replacement: the daemon already falls back from the
BPF monitor to netlink if the former fails to start.

## systemd

The unit is the one upstream ships, unchanged. Its hardening, `Nice=-5`,
`OOMScoreAdjust=-999`, `Delegate=yes` and `CapabilityBoundingSet` are upstream's
decisions and none of them is relaxed here.

The unit is installed and nothing more. A package install does not enable or
start a privileged daemon, which matches the Fedora recipe's `systemctl preset`
and the Debian recipe's `dh_installsystemd --no-enable --no-start`:

```bash
sudo systemctl enable --now ananicy-rs.service
```

Upgrading the package replaces the binary; restarting the service is left to the
administrator, as it is for any Arch package with a running unit.

## Rules

`ananicy-rs` ships no rules. It reads `*.rules`, `*.types` and `*.cgroups` from
`/etc/ananicy.d` (recursively, sorted by path) and writes a default
`ananicy.conf` there on its first start. Copy a rule set in before enabling the
service, from <https://gitlab.com/ananicy-cpp/ananicy-cpp> or
<https://github.com/Nefelim4ag/Ananicy>.

`/etc/ananicy.d` is packaged empty. Shipping a default `ananicy.conf` would
shadow the one the daemon writes for itself, and would turn a file the
administrator is expected to own into one pacman has to reconcile on upgrade.

There is no `optdepends` entry for a rule set: `ananicy-cpp` is not in the
official repositories, and naming a package that cannot be installed would be
worse than saying nothing. `ananicy-rs.install` prints the two upstream URLs at
install time instead, which is the Arch idiom for "here is an extra step".

## Known limitations

- The `Maintainer:` line is a placeholder address. A submission to the official
  repositories or to the AUR needs a real one.
- `pkgver` is a literal in the recipe. It must be bumped by hand; `pkgver()`
  re-derives it during a build but does not update the file.
- `arch=('x86_64')` because that is the only architecture Arch Linux runs. The
  daemon itself is portable to aarch64, riscv64 and loongarch64 — those are the
  architectures the `bpf` build script supports — but claiming them here would
  be a claim about repos this recipe is not in.
- Not submitted, not reviewed. This is a starting point for a submission.
