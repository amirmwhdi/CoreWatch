# Debian 13 builds the source tarball, the .deb and the AppImage. Debian 13
# is the floor: a binary built against its glibc runs on every newer system.
FROM docker.io/library/debian:trixie

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl git xz-utils file librsvg2-bin \
      build-essential debhelper meson blueprint-compiler \
      libgtk-4-dev libadwaita-1-dev desktop-file-utils appstream \
 && rm -rf /var/lib/apt/lists/*

# gtk4-rs needs a newer Rust than Debian ships: stable from rustup.
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
