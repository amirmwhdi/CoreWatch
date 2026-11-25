# Releasing Corewatch

Releases are built and published from your own machine; GitHub only hosts
them. Each package is built in a container of the distribution it is for,
the same way the Release workflow built it. You need
[Podman](https://podman.io) (recommended) or Docker, and the
[GitHub CLI](https://cli.github.com) for the last step.

## A release, step by step

```sh
# 1. New version in every file (release notes: one change per line)
scripts/version.sh bump 0.6.0 notes.txt
git diff                                   # review

# 2. Everything CI used to check, on every distribution
tests/ci.sh

# 3. Commit and tag
git commit -am "release: 0.6.0"
git tag v0.6.0
git push && git push origin v0.6.0

# 4. Build every package into dist/0.6.0/
release/build.sh

# 5. Put them on GitHub
release/publish.sh --draft                 # check the draft on GitHub, then publish it there
release/publish.sh                         # or straight away
```

Then update the Flathub manifest (`packaging/flathub/README.md`).

## `release/build.sh`

```sh
release/build.sh                    # every package for this machine's architecture
release/build.sh deb rpm            # only some: deb rpm arch appimage flatpak
release/build.sh --arch aarch64     # the ARM packages, see below
release/build.sh --rebuild          # rebuild the images (newer Rust, newer packages)
```

| Package | Built in | How |
| --- | --- | --- |
| `.deb` | Debian 13 | `dpkg-buildpackage`, from a tarball with every crate vendored (`scripts/vendored-tarball.sh`) |
| `.rpm` | Fedora | `rpmbuild` with `packaging/rpm/corewatch.spec`, Fedora's own Rust |
| `.pkg.tar.zst` | Arch | `makepkg` with `packaging/arch/PKGBUILD`; x86_64 only |
| `.AppImage` | Debian 13 | Meson install into an AppDir, then `appimagetool` |
| `.flatpak` | the CI Flatpak image | `tests/flatpak.sh --bundle` |

The packages are built from the **last commit**: uncommitted changes are
not in them, and the script says so. The result, with a `SHA256SUMS`, is in
`dist/<version>/`; intermediate files are in `target/packaging/<version>/`.

The images are built once from `containers/` and named
`localhost/corewatch-release-<distro>-<arch>`. `steps/` holds what runs in
each container, one script per package.

### ARM (aarch64)

GitHub had ARM machines; here an ARM build runs under QEMU, which works
but takes much longer. Install the emulator once, then build:

| Distribution | Install |
| --- | --- |
| Arch | `sudo pacman -S qemu-user-static qemu-user-static-binfmt` |
| Debian, Ubuntu | `sudo apt install qemu-user-static binfmt-support` |
| Fedora | `sudo dnf install qemu-user-static` |

```sh
release/build.sh --arch aarch64     # deb, rpm, AppImage and Flatpak for ARM, into the same folder
```

On an ARM machine it is the other way round: `--arch x86_64`.

## `release/publish.sh`

```sh
release/publish.sh            # asks before it publishes
release/publish.sh --draft    # a draft only you see on GitHub
release/publish.sh --yes      # no question
```

Before anything is uploaded it checks:

- the working tree has no uncommitted changes;
- the tag `v<version>` exists, is the current commit (the one the packages
  were built from) and is pushed to GitHub;
- `dist/<version>/` holds only installable packages.

It lists any package the Release workflow used to make that was not built,
such as the ARM ones, writes `SHA256SUMS`, and creates the release with
`gh release create`: titled "Corewatch <version>", notes generated from the
commits since the last release, marked as a pre-release when the version
has a `-` (`0.6.0-beta.1`).

## Behind a proxy

Set `HTTPS_PROXY` as usual: it is passed to every image build and
container, with the host's network, so a proxy on `127.0.0.1` works too.
`gh` reads it by itself.
