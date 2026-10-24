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
memory, disks, graphics and battery read global kernel files and work as
on the host.
