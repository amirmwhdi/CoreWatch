# Fedora: the RPM world and recent GNOME.
FROM registry.fedoraproject.org/fedora:latest

RUN dnf install -y --setopt=install_weak_deps=False \
      gtk4-devel libadwaita-devel blueprint-compiler gcc pkgconf-pkg-config \
      git curl which meson desktop-file-utils appstream \
      xorg-x11-server-Xvfb xauth \
 && dnf clean all

ENV RUSTUP_HOME=/usr/local/rustup PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | CARGO_HOME=/usr/local/cargo \
      sh -s -- -y --profile minimal --component rustfmt,clippy --no-modify-path \
 && chmod -R a+rX /usr/local/rustup /usr/local/cargo
