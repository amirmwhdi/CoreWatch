<p align="center">
  <img src="data/icons/io.github.amirmwhdi.Corewatch.svg" alt="Corewatch Logo" width="128" height="128">
</p>

<h1 align="center">Corewatch</h1>

<p align="center">
  <em>Everything your system is doing, in one place.</em>
</p>

<p align="center">
  <a href="#features">Features</a> •
  <a href="#build-and-run">Build & Run</a> •
  <a href="#license">License</a>
</p>

---

Corewatch is a native Linux system monitor built with Rust, GTK 4 and libadwaita.

**Status: 0.2, preview.** It shows live CPU, GPU, memory, disk and battery usage.

## Features in 0.2

- CPU: model, cores and threads, total usage graph, one graph per logical core, current frequency
- GPU: model, driver, core usage graph, clock speed, VRAM, and temperature
- Memory: used, available, cache and buffers, swap, usage graph (binary IEC units)
- Disks: activity graph, read/write speeds, and mounted filesystem space (binary IEC units)
- Battery: charge level graph, charging status, time remaining, health, power rate, and cycles
- Follows the system light/dark style and accent color
- Configurable update speeds (0.5s, 1s, 2s) and preferences dialog
- Adaptive layout down to 360 px wide
- A banner tells you when a data source stops answering

## Install

Every [release](https://github.com/amirmwhdi/CoreWatch/releases) has a Flatpak
bundle, `.deb` (Debian 13 and newer, Ubuntu 26.04), `.rpm` (Fedora), an Arch
package, an AppImage and a portable tarball, for x86_64 and aarch64 (the Arch
package is x86_64 only).

The AppImage and the portable tarball carry their own GTK, libadwaita, Mesa
and glibc, so they run on any distribution, old or new, without installing
anything. They are larger (about 110 MB) for the same reason. Unpack the
tarball anywhere and run `./corewatch` inside it.

```sh
flatpak install --user corewatch-0.2.1-x86_64.flatpak
sudo apt install ./corewatch-0.2.1-linux-amd64.deb
sudo dnf install ./corewatch-0.2.1-linux-x86_64.rpm
sudo pacman -U corewatch-0.2.1-linux-x86_64.pkg.tar.zst
chmod +x corewatch-0.2.1-linux-x86_64.AppImage && ./corewatch-0.2.1-linux-x86_64.AppImage
```

Releases contain only installable packages: the program, its desktop entry,
icon and metainfo. Recipes for distribution repositories are in `packaging/`:
the AUR PKGBUILD and the Fedora COPR spec build from the tag tarball GitHub
serves, and the Debian package builds offline from a tarball with vendored
crates (`scripts/vendored-tarball.sh`).

## Build and run

Corewatch needs GTK 4.18 or newer and libadwaita 1.7 or newer.

| Distribution | Install build dependencies |
| --- | --- |
| Arch | `sudo pacman -S --needed gtk4 libadwaita blueprint-compiler pkgconf base-devel rustup && rustup default stable` |
| Debian 13 | `sudo apt install libgtk-4-dev libadwaita-1-dev blueprint-compiler pkg-config build-essential`, then Rust from [rustup.rs](https://rustup.rs) |
| Fedora 42+ | `sudo dnf install gtk4-devel libadwaita-devel blueprint-compiler gcc`, then Rust from [rustup.rs](https://rustup.rs) |
| Ubuntu 26.04 | same as Debian |

```sh
cargo run -p corewatch --release
```

To install it with its desktop entry, icon and AppStream metadata, use Meson
(1.7 or newer):

```sh
meson setup build --prefix=/usr/local
meson compile -C build
sudo meson install -C build
```

`meson test -C build` validates the desktop entry and the metainfo when
`desktop-file-validate` and `appstreamcli` are installed.

Useful while developing:

```sh
cargo run -p corewatch -- --verbose                   # detailed logs in the terminal
COREWATCH_LOG=corewatch_core=debug cargo run -p corewatch   # custom log filter
cargo run -p corewatch-core --example print           # CPU and memory in the terminal, no GUI
COREWATCH_SYSROOT=crates/core/tests/fixtures/basic \
    cargo run -p corewatch                            # read a fake /proc and /sys
cargo test --workspace                                # tests
```

## Logs and bug reports

Corewatch writes a log file to `~/.local/state/corewatch/logs/` (one file per
day, the last 7 kept). Inside Flatpak the folder is under
`~/.var/app/io.github.amirmwhdi.Corewatch/`. The terminal only shows warnings
unless you pass `--verbose`.

When reporting a bug, attach today's log file. Its first lines say which
versions of Corewatch, GTK, libadwaita, the kernel and the distribution were
running, and which modules were enabled or disabled and why. Logs never leave
your computer unless you attach them yourself.

## Project layout

```
crates/core       corewatch-core: reads /proc and /sys, no GTK
crates/app        corewatch: the GTK app
data/ui/stable    Blueprint files for the UI, compiled into the binary
data              desktop entry, metainfo, icon (installed by Meson)
po                translation template; no translations ship yet
packaging         Flatpak manifest, Arch PKGBUILD, Debian and RPM packaging
```

The UI layout is written in [Blueprint](https://gnome.pages.gitlab.gnome.org/blueprint-compiler/).
`crates/app/build.rs` compiles it with `blueprint-compiler` and embeds it in the
binary together with `data/style.css`, so `cargo build` alone produces a
complete program.

Every resource is a **module**: one `Collector` in `corewatch-core` that fills one
field of `Snapshot`, and one `ResourcePage` in the app that shows it. Modules are
registered in one place, `crates/app/src/modules/mod.rs`.

### Adding a module

1. `crates/core/src/<name>/`: parser with unit tests, `<Name>Sample`, `<Name>Collector`
2. `crates/core/src/lib.rs` and `snapshot.rs`: `pub mod <name>;` and one `Snapshot` field
3. `crates/app/src/modules/<name>.rs`: the page and `pub const MODULE`
4. `crates/app/src/modules/mod.rs`: `pub mod <name>;` and `<name>::MODULE` in `all()`

The window and the sampler never change when a module is added.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).

Copyright © 2026 Amirmahdi ([@amirmwhdi](https://github.com/amirmwhdi))
