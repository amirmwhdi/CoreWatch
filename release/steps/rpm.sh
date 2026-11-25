#!/bin/sh
# In the Fedora image: the .rpm, from the same tarball GitHub serves for
# the tag (git archive of the commit being released).
# Usage: rpm.sh <version> <arch: x86_64|aarch64>
set -eu
v=$1 arch=$2
spec=packaging/rpm/corewatch.spec

dnf builddep -y "$spec"
rpmdev-setuptree
git archive --prefix="CoreWatch-$v/" -o ~/rpmbuild/SOURCES/CoreWatch-"$v".tar.gz HEAD
rpmbuild -bb "$spec"
cp ~/rpmbuild/RPMS/*/corewatch-*.rpm "/out/corewatch-$v-linux-$arch.rpm"
