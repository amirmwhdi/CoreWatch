#!/bin/sh
# In the Debian image: the source tarball with every crate in vendor/, which
# the .deb and the AppImage build from without network. Not published.
# Usage: source.sh <version>   (run from the repository, writes to /work)
set -eu
scripts/vendored-tarball.sh "$1" /work
