#!/bin/sh
# In the Debian image: the .deb, from the vendored source tarball.
# Usage: deb.sh <version> <debian-arch: amd64|arm64>
set -eu
v=$1 debarch=$2

mkdir -p /tmp/deb && cd /tmp/deb
cp "/work/corewatch-$v.tar.xz" "corewatch_$v.orig.tar.xz"
tar -xf "/work/corewatch-$v.tar.xz"
cd "corewatch-$v"
cp -r packaging/debian debian
# -d: cargo and rustc come from rustup in the image, not from apt.
dpkg-buildpackage -us -uc -b -d
cp ../corewatch_*.deb "/out/corewatch-$v-linux-$debarch.deb"
