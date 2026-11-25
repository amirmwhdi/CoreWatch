# Arch builds the .pkg.tar.zst with makepkg, from packaging/arch/PKGBUILD.
FROM docker.io/library/archlinux:latest

RUN pacman -Syu --noconfirm --needed \
      base-devel git cargo meson blueprint-compiler \
      gtk4 libadwaita appstream desktop-file-utils \
 && pacman -Scc --noconfirm \
 && useradd -m builder
