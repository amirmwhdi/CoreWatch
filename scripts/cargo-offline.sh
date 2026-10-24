#!/bin/sh
# Point Cargo at the vendor/ directory of a tarball made by
# vendored-tarball.sh. Packaging recipes source this file:
#   . scripts/cargo-offline.sh
# It sets CARGO_HOME to a private directory, so the user's own Cargo
# configuration is never touched.
set -eu

src=$(pwd)
export CARGO_HOME="$src/.cargo-home"
export CARGO_NET_OFFLINE=true
mkdir -p "$CARGO_HOME"
cat > "$CARGO_HOME/config.toml" <<TOML
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "$src/vendor"
TOML
