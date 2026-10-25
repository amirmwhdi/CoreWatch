# Flatpak

```sh
flatpak install --user flathub org.gnome.Sdk//49 org.gnome.Platform//49 \
    org.freedesktop.Sdk.Extension.rust-stable//25.08
flatpak-builder --user --install --force-clean build-dir \
    packaging/flatpak/io.github.amirmwhdi.Corewatch.json
flatpak run io.github.amirmwhdi.Corewatch
```

`cargo-sources.json` lists every crate in `Cargo.lock`, so the build runs
offline. Regenerate it whenever `Cargo.lock` changes:

```sh
python3 flatpak-cargo-generator.py Cargo.lock -o packaging/flatpak/cargo-sources.json
```

(`flatpak-cargo-generator.py` is in
[flatpak-builder-tools](https://github.com/flatpak/flatpak-builder-tools/tree/master/cargo).)
CI fails when the file is out of date.

Inside the sandbox `/proc/self/mountinfo` lists the sandbox's own mounts, so
the Filesystems group on the Disks page is not meaningful there yet. CPU,
memory, disks, graphics, sensors and battery read global kernel files and
work as on the host.

Two permissions are there for single pages:

- `--share=network`: without it the sandbox has its own network namespace
  with only `lo`, and the Network page would show nothing. Corewatch itself
  never connects anywhere.
- `--talk-name=org.freedesktop.Flatpak`: the sandbox has its own process
  namespace, so the Processes page runs a small shell script on the host
  with `flatpak-spawn --host` (see `crates/core/src/process/source.rs`). The
  script only reads `/proc` and `/etc/passwd`, using shell built-ins, and
  "End Process" runs the host's `kill`.
