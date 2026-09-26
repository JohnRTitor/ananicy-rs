# Fedora packaging recipe for ananicy-rs.
#
# This is a community-maintained recipe. It is not a package carried by Fedora
# and carries no Fedora review. See contrib/fedora/README.md for how to build it
# and for the reasoning behind the dependency and install decisions.
#
# Version: keep in step with the version in Cargo.toml's [workspace.package]
# table. The prep step refuses to build when the two have drifted, because the
# daemon reports the version cargo reads, so a stale Version: would produce a
# package whose own version is wrong.

Name:           ananicy-rs
Version:        0.1.0
Release:        1%{?dist}
Summary:        Ananicy daemon rewrite in Rust for lower CPU and memory usage
# No Packager: tag. Fedora's build system sets it, and a hardcoded one is
# overwritten there anyway.

License:        GPL-3.0-only
URL:            https://github.com/JohnRTitor/ananicy-rs
# The `git archive` of the release tag, attached to the GitHub release by
# .github/workflows/release.yml. Its top-level directory is ananicy-rs-<version>,
# which is what the prep step below asks for, and also the shape GitHub's own
# /archive/v<version>.tar.gz has, so this spec can be pointed at either.
Source0:        %{url}/releases/download/v%{version}/%{name}-%{version}.tar.gz
# Every crate in Cargo.lock, vendored. Fedora builds have no network, so the build
# cannot fetch them; regenerate with contrib/fedora/make-vendor-archive.sh.
Source1:        %{name}-%{version}-vendor.tar.xz

# The build is the project's own: `cargo build --release` with its default
# feature set. Nothing here needs the eBPF toolchain, because the `bpf` feature
# is not part of that default and is not enabled below.
BuildRequires:  cargo
# Edition 2024, which every crate in the workspace uses, needs 1.85. There is no
# `rust-version` in any manifest, so cargo cannot report this itself.
BuildRequires:  rust >= 1.85
# `make install` is what places the binary and the unit.
BuildRequires:  make
# pcre2-sys asks pkg-config for libpcre2-8 and only falls back to building its
# own vendored copy when the probe fails. The fallback is a different build
# (SUPPORT_JIT=1 forced, linked statically), so the system library is required,
# not optional.
BuildRequires:  pkgconf-pkg-config
BuildRequires:  pcre2-devel
# The `systemd` feature is on by default and declares `#[link(name = "systemd")]`
# in ananicy-platform, so libsystemd.so is a link-time dependency of the
# default build. `systemd-devel` also pulls in the compiler.
BuildRequires:  systemd-devel
# For the systemd_post, systemd_preun and systemd_postun_with_restart macros.
BuildRequires:  systemd-rpm-macros

# No manual Requires: the linker records libsystemd.so.0 and libpcre2-8.so.0 and
# rpm turns those SONAMEs into requires against systemd-libs and pcre2. The
# daemon itself needs no particular init system, so nothing here depends on
# systemd-as-init: the unit is what asks for it, and `ananicy-rs start` also runs
# under OpenRC, runit, s6, or no init at all.
#
# No rules are packaged either. ananicy-rs ships none -- it reads whatever is in
# /etc/ananicy.d at run time and writes a default ananicy.conf on first start --
# and Fedora does not carry ananicy-cpp or the Ananicy rule set, so there is
# nothing to depend on. See contrib/fedora/README.md.

%description
ananicy-rs is a rewrite of the ananicy daemon in Rust. It watches process
creation and automatically applies the nice value, I/O priority, scheduling
policy, CPU affinity, OOM score adjustment and cgroup placement that a matching
rule asks for.

ananicy-rs ships no rules of its own. It reads them from /etc/ananicy.d, so
install a rule set there before enabling the service; the daemon writes a
default ananicy.conf there on its first start.

This build uses the default features: the netlink process-event source and
systemd integration. The optional eBPF event source is not built -- it needs
clang, libbpf and rustfmt at build time, and CAP_BPF plus CAP_PERFMON at run
time, which the shipped unit does not grant.

%prep
%autosetup -n %{name}-%{version}

# The daemon prints the version cargo reads out of [workspace.package], so a
# spec that has drifted from Cargo.toml would build a package mislabelled with
# its own version. Fail here instead.
if ! grep -qx 'version = "%{version}"' Cargo.toml; then
    : 'Version: does not match [workspace.package] version in Cargo.toml'
    grep -n '^version = ' Cargo.toml || :
    exit 1
fi

# Source1 has been unpacked at the top of the build directory. Point cargo's
# source replacement at it, and keep CARGO_HOME inside the build directory so
# nothing is read from or written to the build user's home directory. The
# directory is substituted as an absolute path, which is what a cargo
# `directory` source requires.
install -d %{_builddir}/cargo-home
sed -e 's|@VENDOR_DIR@|%{_builddir}/vendor|' \
    %{_builddir}/vendor-config.toml > %{_builddir}/cargo-home/config.toml

%build
# Default features, which is `netlink` plus `systemd`. `--offline` and the
# source replacement above mean a Fedora build never touches the network, and
# `--locked` means it never rewrites Cargo.lock.
CARGO_HOME=%{_builddir}/cargo-home \
    cargo build --release --locked --offline --verbose

%install
# The project's own install rule, so the binary, its mode and the unit are
# exactly what `make install` produces for every other packaging recipe here.
# The unit's @bindir@ placeholder resolves to /usr/bin and the unit lands in
# /usr/lib/systemd/system.
make install DESTDIR=%{buildroot} PREFIX=%{_prefix}

# The daemon looks for its configuration here. The directory is packaged empty
# on purpose: the daemon creates ananicy.conf in it on first start, and shipping
# one would both shadow that and become a file rpm then has to treat as
# configuration on every upgrade.
install -d -m 0755 %{buildroot}%{_sysconfdir}/ananicy.d

# `completions` is handled before any configuration is read and needs no
# privileges, so generating it here is safe.
install -d -m 0755 %{buildroot}%{_datadir}/bash-completion/completions
install -d -m 0755 %{buildroot}%{_datadir}/fish/vendor_completions.d
install -d -m 0755 %{buildroot}%{_datadir}/zsh/site-functions
%{buildroot}%{_bindir}/ananicy-rs completions bash \
    > %{buildroot}%{_datadir}/bash-completion/completions/ananicy-rs
%{buildroot}%{_bindir}/ananicy-rs completions fish \
    > %{buildroot}%{_datadir}/fish/vendor_completions.d/ananicy-rs.fish
%{buildroot}%{_bindir}/ananicy-rs completions zsh \
    > %{buildroot}%{_datadir}/zsh/site-functions/_ananicy-rs

# No strip step: `[profile.release] strip = true` in Cargo.toml means cargo has
# already stripped the binary, and there are no debug symbols left to remove.

%check
# The check step runs as the build user in mock, so the cgroup tests in
# ananicy-platform detect that they are unprivileged and skip themselves, with a
# message, rather than failing. The dev profile is used on purpose: the release
# profile is fat-LTO with a single codegen unit, which would multiply the runtime
# of a suite that exercises the same code.
CARGO_HOME=%{_builddir}/cargo-home \
    cargo test --locked --offline

%files
%license LICENSE
%doc README.md CONTRIBUTING.md
%doc docs/BUILD.md docs/CLI.md docs/COMPATIBILITY.md
%doc docs/CONFIGURATION.md docs/SYSTEMD.md docs/TESTING.md docs/TOPOLOGY.md

%{_bindir}/ananicy-rs
# Not a ghost unit and not disabled: the systemd_post macro below makes rpm's
# file triggers reload systemd when the unit appears, changes or disappears, and
# systemd_preun stops the service before the binary is taken away.
%{_unitdir}/ananicy-rs.service

%dir %{_sysconfdir}/ananicy.d

%{_datadir}/bash-completion/completions/ananicy-rs
%{_datadir}/fish/vendor_completions.d/ananicy-rs.fish
%{_datadir}/zsh/site-functions/_ananicy-rs

# The unit is registered and preset, never started: installing a package must not
# begin reprioritising the machine's processes. Enabling is the admin's call.
%post
%systemd_post ananicy-rs.service

%preun
%systemd_preun ananicy-rs.service

# An upgrade replaces a running daemon's executable, so restart it -- but only if
# it was running. Removing the package still stops it.
%postun
%systemd_postun_with_restart ananicy-rs.service

%changelog
* Sat Sep 26 2026 ananicy-rs packagers <ananicy-rs-packaging@example.invalid> - 0.1.0-1
- Initial community packaging recipe, built from the v0.1.0 release tarball with
  Cargo dependencies vendored for a network-isolated build.
