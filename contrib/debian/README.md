# Debian packaging recipe

`debian/` here is the packaging directory for `ananicy-rs`. It is kept under
`contrib/` rather than at the top of the repository, because Debian requires
`debian/` to be the top-level directory of a source package and duplicating it in
two places would guarantee they drift. Copy it into place when building.

This is a **community-maintained recipe**. It has not been uploaded, ITP'd or
reviewed by Debian, and its presence here does not mean Debian supports the
package.

## Build it

```bash
# 1. Put the packaging where dpkg expects it, from a checkout whose directory is
#    named <source>-<version>.
version=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n1)
cp -r contrib/debian debian

# 2. Vendor the Cargo dependencies. Needs cargo, tar and xz. Needs network:
#    this is the only step in the whole flow that does. It has to run after step
#    1, because the archive belongs in debian/ -- see the note below.
./debian/make-vendor.sh             # -> debian/vendor.tar.xz

# 3. Build. The test suite runs as part of this.
dpkg-buildpackage -us -uc -b
```

A `.deb` build alone does not need the `.orig` tarball. To build a source
package as well — which is what an upload needs, and what
[`.github/workflows/packaging.yml`](../../.github/workflows/packaging.yml) does
— create it first:

```bash
git archive --format=tar --prefix="ananicy-rs-$version/" HEAD \
  | gzip -n -9 > "../ananicy-rs_$version.orig.tar.gz"
dpkg-buildpackage -us -uc --no-sign
```

`debian/vendor.tar.xz` is generated, gitignored, and has to be regenerated
whenever `Cargo.lock` changes. It ships inside the source package; set
`SOURCE_DATE_EPOCH` to the release date for byte-identical archives across
regenerations.

It lives in `debian/`, and `debian/source/include-binaries` lists it, for two
reasons that `dpkg-source` states between them. A binary file in a source
package has to be declared there, and a 15 MB xz of third-party source is a
binary file by that definition even though it is source. And because
`dpkg-source` builds the source package by diffing the working tree against the
orig tarball, the archive must not be left in `contrib/debian/` — there it would
be a modification to a file the orig tarball already has, which cannot be
represented. That is why `make-vendor.sh` refuses to run until the packaging has
been copied into place.

The more usual home for bundled dependencies is the orig tarball, and that was
rejected deliberately: the build would then depend on the orig tarball being
present, and `dpkg-buildpackage -b` does not require one, because it builds in the
working tree. As shipped, the build tree is self-contained and `-b` works from a
checkout with no orig tarball at all.

`dpkg-buildpackage` never reaches the network: `debian/rules` sets
`CARGO_NET_OFFLINE`, points `CARGO_HOME` inside the build directory, and passes
`--locked --offline` to every cargo invocation.

## Static validation

```bash
dpkg-buildpackage -us -uc -b
lintian --tag-display-limit 0 ../ananicy-rs_*.changes
```

What *was* run locally while this recipe was written, and what was not:

- **Run locally:** every step of `debian/rules` was executed against a real
  `git archive` of the tree. `make-vendor.sh` produced a working
  `debian/vendor.tar.xz`; `override_dh_auto_configure`'s extraction,
  absolute-path substitution and version check were run as written, in the
  matching and the mismatching direction and against three changelog version
  shapes; `cargo build --release --locked --offline` succeeded with the
  `RUSTFLAGS` `debian/rules` sets and produced a binary with `BIND_NOW` and a
  `GNU_RELRO` segment; `cargo test --locked --offline` passed, 20 test binaries,
  0 failures; `override_dh_auto_install`'s `make install` and the three
  completions produced exactly the file list `debian/ananicy-rs.install`
  declares, and every path in `debian/ananicy-rs.docs` resolved.
- **Not run locally:** `dpkg-buildpackage` and `lintian` do not exist in the
  environment this was written in, so debhelper's own behaviour
  (`dh_installsystemd`, `dh_installdeb`'s handling of `.dirs` and `.docs`) was
  argued from its documentation rather than observed at the time.
- **Run in CI on every change:** a real `dpkg-buildpackage` on `debian:sid`,
  producing both the source package and a `.deb` whose contents are exactly the
  table above, then `dpkg-deb --contents`, then `lintian`. That is what found
  the build-depends on `make`, the obsolete `pkg-config`, the maintainer scripts
  without an interpreter, and the tags now declared in
  `debian/ananicy-rs.lintian-overrides`. All four were findings about this
  recipe and are fixed.

[`.github/workflows/packaging.yml`](../../.github/workflows/packaging.yml) runs
the real thing on every change — `dpkg-buildpackage` including `dh_auto_test`,
then `dpkg-deb --contents`, then `lintian` — in a `debian:sid` container, as an
unprivileged user.

## Why not `dh-cargo`

Debian's Rust packaging policy asks for `dh-cargo`, and normally that is the
right answer. It is not available here:

- `dh-cargo` installs crates into `/usr/share/cargo/registry` and expects every
  dependency to exist as its own Debian source package, generated by `debcargo`
  from the crate's crates.io metadata. Debian's own description of `dh-cargo`
  says that for a program with private crates — "such as firefox or librsvg" —
  you should use Debian's cargo wrapper from the `cargo` package instead.
- None of the four workspace crates (`ananicy-rs`, `ananicy-core`,
  `ananicy-platform`, `ananicy-bpf`) is published on crates.io, so `debcargo`
  has nothing to work from.
- Packaging the roughly two hundred transitive crates as `librust-*-dev` source
  packages is a project in itself, and not one this recipe should pretend to
  have done.

So this is a plain `debhelper` package that `Build-Depends: cargo` and vendors
the crate sources into the source package, which is the other route the policy
allows and the one that keeps the build offline.

## What goes in the package

| Path | From |
| --- | --- |
| `/usr/bin/ananicy-rs` | `make install` |
| `/usr/lib/systemd/system/ananicy-rs.service` | `make install`, from `data/ananicy-rs.service.in` |
| `/etc/ananicy.d/` | `debian/ananicy-rs.dirs`, packaged empty |
| `/usr/share/bash-completion/completions/ananicy-rs` | `ananicy-rs completions bash` |
| `/usr/share/fish/vendor_completions.d/ananicy-rs.fish` | `ananicy-rs completions fish` |
| `/usr/share/zsh/site-functions/_ananicy-rs` | `ananicy-rs completions zsh` |
| `/usr/share/doc/ananicy-rs/` | `README.md`, `CONTRIBUTING.md`, `docs/*.md`, `README.Debian` |
| `/usr/share/doc/ananicy-rs/copyright` | `debian/copyright` |

The `docs/*.md` are **flattened** into `/usr/share/doc/ananicy-rs/`, not into a
`docs/` subdirectory: `dh_installdeb` drops the leading directories of a path
listed in `debian/ananicy-rs.docs`, so the tree is `usr/share/doc/ananicy-rs/`
plus one file per entry, and `SYSTEMD.md` from `docs/SYSTEMD.md` collides with
nothing because there is no `SYSTEMD.md` at the top level. Everything is
gzipped by `dh_compress` except `TOPOLOGY.md`, which is left alone because it is
mostly tables and does not compress past `dh_compress`'s ratio threshold.

There is no `debian/ananicy-rs.service`. The unit is generated by
`make install` into `debian/tmp`, listed in `debian/ananicy-rs.install`, and
picked up from there by `dh_installsystemd` — which is also what generates the
postinst/postrm snippets in `debian/ananicy-rs.postinst` and
`debian/ananicy-rs.postrm`.

There are no man pages to install: the project has none. `docs/CLI.md` and
`docs/CONFIGURATION.md` are installed as documentation instead, and
`debian/ananicy-rs.lintian-overrides` declares the resulting `no-manual-page`
tag, with the reason.

## Metadata

| Field | Value | Why |
| --- | --- | --- |
| `Architecture` | `linux-any` | the daemon is built on netlink, cgroups and procfs; nothing in the tree compiles elsewhere |
| `Priority` | `optional` | |
| `Section` | `admin` | it administers process priorities |
| `Standards-Version` | 4.7.0 | |
| `Rules-Requires-Root` | `no` | the build only runs `cargo build` and `cargo test`; the tests that need root skip themselves |
| `Depends` | `${shlibs:Depends}, ${misc:Depends}` | `dpkg-shlibdeps` turns the linker-recorded `libsystemd.so.0` and `libpcre2-8.so.0` into a real dependency |
| `Recommends`/`Suggests` | none | see below |

No dependency on systemd-as-init is declared, and none should be: the daemon
detects whether it is supervised and also runs under OpenRC, runit, s6 or no
init at all. Only the packaged unit asks for systemd.

No dependency on a rule set is declared either. `ananicy-rs` ships no rules, and
no Debian derivative packages `ananicy-cpp` or the Ananicy rule set, so naming
one would be a dependency on a package that cannot exist. When a rule set does
get packaged, the right relationship is a `Recommends` or a separate
`ananicy-rs-rules` package, not a `Requires`.

## Build dependencies

`cargo`, `rustc (>= 1.88)`, `pkgconf`, `libpcre2-dev`, `libsystemd-dev`, plus
`debhelper-compat (= 13)`.

`make` is deliberately **not** in the list even though `make install` is what
places the binary and the unit. It is part of `build-essential`, which dpkg
treats as an implicit build dependency and checks all the same, and lintian
objects to naming it: `build-depends-on-build-essential-package-without-using-
version`.

`build-essential` therefore cannot be listed either — Policy says not to — but it
is genuinely required, because linking needs a C compiler and libc headers, and
`dpkg-checkbuilddeps` aborts without it. The CI job installs it explicitly.

`pkgconf` rather than `pkg-config`: the latter has been a transitional package
since bookworm. It provides the same `pkg-config` binary, which is what
`pcre2-sys` invokes.

`rustc (>= 1.88)` is the floor, and it is higher than the edition requires.
Edition 2024 needs 1.85, but the workspace uses let chains — 30 `&& let`
expressions — and those stabilised in 1.88, so a 1.85 toolchain parses every
manifest and then fails with `error[E0658]: let expressions in this position are
unstable`. No manifest carries a `rust-version`, so cargo cannot warn you first.

**Which means this cannot be built against Debian stable.** trixie (13) ships
rustc 1.85.1, one minor release short. A submission would therefore target
trixie-backports, which carries 1.88.0, or sid. The CI job uses `debian:sid` for
the same reason.

`clang`, `libbpf-dev` and `rustfmt` are **not** build dependencies, because the
`bpf` feature is not enabled. See the feature section below.

`libpcre2-dev` is required rather than optional: `pcre2-sys` probes for
`libpcre2-8` with pkg-config and only falls back to building its own vendored
copy when the probe fails, and that copy is a different build (`SUPPORT_JIT=1`
forced, linked statically).

## Configuration and upgrades

Nothing under `/etc` is a conffile and nothing is added on upgrade.
`debian/ananicy-rs.dirs` creates `/etc/ananicy.d` empty; the daemon writes
`ananicy.conf` into it on first start, and that file is not owned by the package,
so `dpkg` never touches it. Shipping a default `ananicy.conf` would shadow the
one the daemon writes for itself and turn an administrator's file into something
dpkg has to reconcile on every upgrade.

## systemd

The unit is upstream's, unchanged. `dh_installsystemd --no-enable --no-start`
means installing the package registers the unit but neither enables nor starts
it, which matches the Fedora recipe's `systemctl preset` and the Arch package's
plain unit install. Enabling a privileged daemon that has no rules configured is
a decision for the administrator:

```bash
sudo systemctl enable --now ananicy-rs.service
```

With `--no-start`, an upgrade stops and restarts the service, so a running
daemon picks up the new executable.

## Features

Default features only: `netlink` plus `systemd`. The `bpf` feature is off
because it is not upstream's default, it needs `clang`, `libbpf` and `rustfmt`
at build time, and it needs `CAP_BPF` and `CAP_PERFMON` at run time, which the
shipped unit's `CapabilityBoundingSet` does not grant. A `-bpf` variant would be
a runtime opt-in, not a replacement: the daemon already falls back from the BPF
monitor to netlink if the former fails to start.

## Known limitations

- `Maintainer:` and the `debian/changelog` trailer are placeholders
  (`ananicy-rs-packaging@example.invalid`). They must be replaced with a real
  address before any upload.
- `Standards-Version: 4.7.0` is a stated value, not one resolved against the
  target suite. Check it against the release you are targeting.
- The `changelog` distribution is `unstable`. Change it if you are targeting
  `experimental`.
- `debian/ananicy-rs.lintian-overrides` declares three tags: `no-manual-page`
  and `initial-upload-closes-no-bugs` because both are inherent to a new
  package, and `maintainer-script-ignores-errors` because the `|| true` calls
  are inside the block `dh_installsystemd` generates. The reasons are in the
  file. A submission would be expected to keep the first two and argue the third
  with the release team rather than override it.
- Not uploaded, not ITP'd, not reviewed. A real ITP would need a
  `librust-*-dev`-style dependency story, or a clear statement that the
  maintainer is prepared to carry the crate source themselves.
