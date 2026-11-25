Name:           corewatch
Version:        0.5.0
Release:        1%{?dist}
Summary:        A modern system monitor for Linux
License:        GPL-3.0-or-later
URL:            https://github.com/amirmwhdi/CoreWatch
# The tag tarball from GitHub. Cargo downloads the crates in %prep, so on
# COPR enable "Internet access during builds" for this package.
Source0:        %{url}/archive/v%{version}/CoreWatch-%{version}.tar.gz

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
%autosetup -n CoreWatch-%{version}
cargo fetch --locked

%build
export CARGO_NET_OFFLINE=true
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
%{_datadir}/icons/hicolor/symbolic/apps/io.github.amirmwhdi.Corewatch-symbolic.svg

%changelog
* Wed Nov 04 2026 Amirmahdi <amirmwhdi@users.noreply.github.com> - 0.5.0-1
- Group by App: one row per app, with the total CPU, memory, graphics and network use of its processes
- Graphics card use of each process, for amdgpu, Intel, NVIDIA and other drivers
- Process details: command line, program file, start time, disk reads and writes, and network connections, with Suspend, Resume and Lower Priority
- Pressure: how long tasks waited for the processor, memory and disks, on the Overview and on each page
- Performance and efficient cores are shown apart on hybrid processors
- Alerts for high temperatures, full memory or disks, and a process that keeps a core busy, with an option to keep watching in the background
- Graph history of 1, 5 or 15 minutes, or an hour, and the battery's charge over the last day
- A faster process scan
- Corewatch is now in English only

* Tue Nov 03 2026 Amirmahdi <amirmwhdi@users.noreply.github.com> - 0.4.0-1
- Persian translation, with a right-to-left layout and Persian digits
- Keyboard shortcuts: Ctrl+1 to Ctrl+9 open the pages, Ctrl+F searches the processes, Delete ends the selected one, and Ctrl+? lists them all
- Search processes by name or process ID
- Users from systemd-homed, LDAP and other user databases are shown by name
- A laptop's NVIDIA graphics card can go back to sleep while Corewatch runs
- Ending a process never reaches another process that took over its ID
- A failing drive or a slow user database no longer holds up the other pages
- Smooth graphs pause while the system's Power Saver mode is on
- Drive temperatures from UDisks come back when UDisks starts late or restarts

* Sun Oct 25 2026 Amirmahdi <amirmwhdi@users.noreply.github.com> - 0.3.0-1
- Overview, Processes, Network and Sensors pages

* Sat Oct 24 2026 Amirmahdi <amirmwhdi@users.noreply.github.com> - 0.2.0-1
- Graphics, Disks and Battery pages, preferences, Meson install, Flatpak
