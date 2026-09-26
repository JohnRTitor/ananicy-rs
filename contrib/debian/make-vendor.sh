#!/bin/sh
# Build the vendored Cargo dependencies that debian/rules expects.
#
# Debian's Rust packaging policy requires that a package build not access the
# network: crate sources have to come from the source package, not from
# crates.io at build time. ananicy-rs is not published on crates.io, and neither
# are ananicy-core, ananicy-platform or ananicy-bpf, so the `dh-cargo` route
# that expresses every dependency as a librust-<crate>-dev source package is not
# available either. So the crate sources are vendored once, here, and shipped
# inside the source package as debian/vendor.tar.xz.
#
# Usage:
#   cp -r contrib/debian debian      # the packaging has to be in place first
#   debian/make-vendor.sh [OUTPUT]
#
# OUTPUT defaults to debian/vendor.tar.xz, the path debian/rules looks for and
# the path debian/source/include-binaries declares. It has to be inside the
# installed debian/ directory and not inside contrib/debian/: dpkg-source builds
# the source package by diffing the working tree against the orig tarball, so an
# archive sitting in contrib/debian/ is a modification to a file the orig tarball
# already has, which it cannot represent. Inside debian/ it is simply a new file
# in the packaging directory, which is the ordinary case.
#
# The archive is generated, not committed; regenerate it whenever Cargo.lock
# changes and ship the result with the source package.
#
# The archive contains two entries:
#
#   vendor/            one directory per crate, each with the .cargo-checksum.json
#                      that makes a cargo directory source work
#   vendor-config.toml a cargo source-replacement config whose directory is the
#                      literal @VENDOR_DIR@; debian/rules substitutes the absolute
#                      path at build time
#
# Requirements: cargo, tar, xz.

set -eu

srcdir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$srcdir"

if [ ! -d debian ]; then
    echo "make-vendor.sh: $srcdir/debian does not exist." >&2
    echo "Copy the packaging into place first: cp -r contrib/debian debian" >&2
    exit 1
fi

output=${1:-"$srcdir/debian/vendor.tar.xz"}

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT INT HUP TERM

# --locked keeps cargo from touching Cargo.lock, so the archive matches the
# lockfile the .deb is built from. --versioned-dirs keeps the directory name
# carrying the version, so two archives of two releases stay comparable.
cargo vendor --locked --versioned-dirs "$workdir/vendor" >/dev/null

cat >"$workdir/vendor-config.toml" <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "@VENDOR_DIR@"
EOF

# Sorted names, fixed ownership and a fixed timestamp, so regenerating without a
# Cargo.lock change produces the same bytes. Set SOURCE_DATE_EPOCH to the release
# date to tie the archive to it.
mkdir -p "$(dirname -- "$output")"
tar --create --xz --file="$output" \
    --directory="$workdir" \
    --sort=name --owner=0 --group=0 --numeric-owner \
    --mtime="@${SOURCE_DATE_EPOCH:-0}" \
    vendor vendor-config.toml

echo "wrote $output"
