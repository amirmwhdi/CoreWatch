#!/bin/sh
# Build corewatch-<version>.tar.xz: the source tree plus every crate from
# Cargo.lock in vendor/, so distribution packages build without network.
# Usage: scripts/vendored-tarball.sh <version> <output-dir>
set -eu

version=$1
out=$(realpath "$2")
name="corewatch-$version"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

git archive --format=tar --prefix="$name/" HEAD | tar -x -C "$work"
(cd "$work/$name" && cargo vendor --locked --versioned-dirs vendor >/dev/null)
mkdir -p "$out"
tar -C "$work" -cJf "$out/$name.tar.xz" "$name"
echo "$out/$name.tar.xz"
