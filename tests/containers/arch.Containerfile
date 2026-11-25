# The edge: the newest GTK and libadwaita.
FROM docker.io/library/archlinux:latest

RUN pacman -Syu --noconfirm --needed \
      gtk4 libadwaita blueprint-compiler pkgconf base-devel git curl \
      meson desktop-file-utils appstream \
      xorg-server-xvfb xorg-xauth \
 && pacman -Scc --noconfirm

ENV RUSTUP_HOME=/usr/local/rustup PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | CARGO_HOME=/usr/local/cargo \
      sh -s -- -y --profile minimal --component rustfmt,clippy --no-modify-path \
 && chmod -R a+rX /usr/local/rustup /usr/local/cargo
