Name:           corewatch
Version:        0.2.0
Release:        1%{?dist}
Summary:        A modern system monitor for Linux
License:        GPL-3.0-or-later
URL:            https://github.com/amirmwhdi/CoreWatch
# The release tarball holds every crate in vendor/, so the build is offline.
Source0:        %{url}/releases/download/v%{version}/%{name}-%{version}.tar.xz

# The release profile already strips the binary.
%global debug_package %{nil}

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  gcc
BuildRequires:  meson
BuildRequires:  blueprint-compiler
BuildRequires:  pkgconfig(gtk4) >= 4.18
BuildRequires:  pkgconfig(libadwaita-1) >= 1.7
BuildRequires:  desktop-file-utils
BuildRequires:  appstream

Requires:       gtk4 >= 4.18
Requires:       libadwaita >= 1.7

%description
Corewatch is a native system monitor for Linux. It shows live graphs for the
processor, memory, graphics, disks and battery, and follows the system light
and dark style.

%prep
%autosetup

%build
. scripts/cargo-offline.sh
%meson
%meson_build

%check
%meson_test

%install
%meson_install

%files
%license LICENSE
%doc README.md
%{_bindir}/corewatch
%{_datadir}/applications/io.github.amirmwhdi.Corewatch.desktop
%{_datadir}/metainfo/io.github.amirmwhdi.Corewatch.metainfo.xml
%{_datadir}/icons/hicolor/scalable/apps/io.github.amirmwhdi.Corewatch.svg

%changelog
* Sat Oct 24 2026 Amirmahdi <amirmwhdi@users.noreply.github.com> - 0.2.0-1
- Graphics, Disks and Battery pages, preferences, Meson install, Flatpak
