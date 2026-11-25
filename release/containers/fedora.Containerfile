# Fedora builds the .rpm with Fedora's own Rust, from packaging/rpm/corewatch.spec.
FROM registry.fedoraproject.org/fedora:latest

# The spec's BuildRequires, installed ahead so a build only downloads crates;
# release/steps/rpm.sh still runs `dnf builddep` in case the spec changed.
RUN dnf install -y --setopt=install_weak_deps=False \
      rpm-build rpmdevtools dnf-plugins-core git \
      cargo rust gcc meson blueprint-compiler gtk4-devel libadwaita-devel \
      desktop-file-utils appstream \
 && dnf clean all
