# The floor: the oldest GTK (4.18) and libadwaita (1.7) Corewatch supports.
FROM docker.io/library/debian:trixie

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl git build-essential pkg-config \
      libgtk-4-dev libadwaita-1-dev blueprint-compiler \
      meson desktop-file-utils appstream \
      xvfb xauth \
 && rm -rf /var/lib/apt/lists/*

# gtk4-rs needs a newer Rust than Debian ships: stable from rustup, as CI used.
ENV RUSTUP_HOME=/usr/local/rustup PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | CARGO_HOME=/usr/local/cargo \
      sh -s -- -y --profile minimal --component rustfmt,clippy --no-modify-path \
 && chmod -R a+rX /usr/local/rustup /usr/local/cargo
