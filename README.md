# Corewatch

*Everything your system is doing, in one place.*

Corewatch is a native Linux system monitor built with Rust, GTK 4 and libadwaita.

**Status: 0.1, early preview.** It shows live CPU and memory usage. Disks,
network, processes and GPU follow in later versions; see the roadmap.

## Features in 0.1

- CPU: model, cores and threads, total usage graph, one graph per logical core, current frequency
- Memory: used, available, cache and buffers, swap, usage graph
- Follows the system light/dark style and accent color
- Adaptive layout down to 360 px wide

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
