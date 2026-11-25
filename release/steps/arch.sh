#!/bin/sh
# In the Arch image: the .pkg.tar.zst with makepkg, which refuses to run as
# root, so as the image's `builder` user.
# Usage: arch.sh <version>
set -eu
v=$1
pkg=/home/builder/pkg

mkdir -p "$pkg"
cp packaging/arch/PKGBUILD "$pkg/"
# The commit being released, under the name the PKGBUILD downloads, so
# makepkg uses it instead of the tag tarball on GitHub.
git archive --prefix="CoreWatch-$v/" -o "$pkg/corewatch-$v.tar.gz" HEAD
chown -R builder "$pkg"
su builder -c "cd $pkg && makepkg --noconfirm"
cp "$pkg"/corewatch-[0-9]*.pkg.tar.zst "/out/corewatch-$v-linux-x86_64.pkg.tar.zst"
