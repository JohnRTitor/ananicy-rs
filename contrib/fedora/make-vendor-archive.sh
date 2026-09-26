#!/bin/sh
# Build the Source1 that contrib/fedora/ananicy-rs.spec expects.
#
# ananicy-rs is not published on crates.io -- neither is ananicy-core,
# ananicy-platform or ananicy-bpf -- so Fedora's `%cargo_prep` route, which
# derives the build requirements from a crate's crates.io metadata, does not
# apply. Instead the recipe carries a vendored copy of everything in Cargo.lock
# as a source tarball and builds with `--offline`, because Fedora builds have no
# network access.
#
# Usage:
#   contrib/fedora/make-vendor-archive.sh [OUTPUT]
#
# OUTPUT defaults to contrib/fedora/ananicy-rs-<version>-vendor.tar.xz, which is
# the name ananicy-rs.spec's `Source1:` resolves to. The archive is gitignored;
# regenerate it whenever Cargo.lock changes, and re-upload it with the source
# package.
#
# The archive contains two entries:
#
#   vendor/            one directory per crate, each with the .cargo-checksum.json
#                      that makes a cargo directory source work
#   vendor-config.toml a cargo source-replacement config whose directory is the
#                      literal @VENDOR_DIR@; the spec substitutes the absolute
#                      path at build time
#
# Requirements: cargo, tar, xz.

set -eu

srcdir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$srcdir"

# The version the spec's Source1 name is built from, read the same way the spec
# reads it. `[workspace.package] version` is the only version in the tree.
version=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)
if [ -z "$version" ]; then
    echo "make-vendor-archive: no [workspace.package] version in Cargo.toml" >&2
    exit 1
fi

output=${1:-"$srcdir/contrib/fedora/ananicy-rs-$version-vendor.tar.xz"}

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT INT HUP TERM

# --locked keeps cargo from touching Cargo.lock, so the archive matches the
# lockfile the package was released with. --versioned-dirs keeps the directory
# name carrying the version, which makes the two archives of two releases
# comparable.
cargo vendor --locked --versioned-dirs "$workdir/vendor" >/dev/null

cat >"$workdir/vendor-config.toml" <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "@VENDOR_DIR@"
EOF

# Sorted names, fixed ownership and a fixed timestamp, so regenerating without a
# Cargo.lock change produces the same bytes. Set SOURCE_DATE_EPOCH to the
# release date to tie the archive to it.
mkdir -p "$(dirname -- "$output")"
tar --create --xz --file="$output" \
    --directory="$workdir" \
    --sort=name --owner=0 --group=0 --numeric-owner \
    --mtime="@${SOURCE_DATE_EPOCH:-0}" \
    vendor vendor-config.toml

echo "wrote $output"
