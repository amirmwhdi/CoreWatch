# Testing Corewatch

Nothing runs on GitHub any more: everything the CI workflow did runs on
your own machine, with the scripts in this folder. Building and publishing
releases is in [`release/`](../release/README.md).

| Command | Replaces | Needs |
| --- | --- | --- |
| `tests/run.sh` | the checks and tests, on your own system | Rust, GTK 4.18, libadwaita 1.7 |
| `tests/container.sh` | the CI job on Debian 13, Fedora and Arch | Podman or Docker |
| `tests/flatpak.sh` | the CI Flatpak job | Podman or Docker (or `flatpak-builder` with `--host`) |
| `tests/ci.sh` | the whole CI workflow: both of the above | Podman or Docker |
| `tests/install-hook.sh` | CI running on every push | git |

## Every day: `tests/run.sh`

```sh
tests/run.sh
```

This runs these steps and ends with a summary; one failed step does not
stop the others.

| Step | What it checks |
| --- | --- |
| `versions` | Cargo, Meson, the metainfo and every packaging recipe name the same version |
| `fmt` | `cargo fmt --check` |
| `lint` | `cargo clippy`, warnings are errors |
| `test` | `cargo test`: the parsers' unit tests, the collectors against the fake `/proc` and `/sys` trees, and the template test that builds the real window |
| `data` | `desktop-file-validate` and `appstreamcli validate` on the files in `data/` |
| `build` | `cargo build --release`; only when named, or with `ci` |

Useful variations:

```sh
tests/run.sh test             # only the tests
tests/run.sh lint test        # several steps, in this order
tests/run.sh ci               # every step, the release build too
tests/run.sh --core test      # only corewatch-core: fast, needs no GTK
tests/run.sh --fix fmt        # format the code instead of checking it
cargo test -p corewatch-core cpu::   # plain cargo works too, with a filter
```

Besides the build dependencies in the main README, install a virtual
display and the two validators, so that no step is skipped:

| Distribution | Packages |
| --- | --- |
| Arch | `sudo pacman -S --needed xorg-server-xvfb xorg-xauth desktop-file-utils appstream` |
| Debian, Ubuntu | `sudo apt install xvfb xauth desktop-file-utils appstream` |
| Fedora | `sudo dnf install xorg-x11-server-Xvfb xauth desktop-file-utils appstream` |

Without Xvfb the template test uses your desktop's display, or skips itself
when there is none.

### Before every push, by itself

```sh
tests/install-hook.sh           # each git push runs tests/run.sh first
tests/install-hook.sh --full    # each git push runs tests/ci.sh first (slow)
tests/install-hook.sh --remove
```

A push whose checks fail is stopped; `git push --no-verify` pushes anyway,
once.

## Like CI: `tests/ci.sh`

`tests/run.sh` only tests the GTK and libadwaita your distribution has.
`tests/ci.sh` does everything the CI workflow did, in containers:

```sh
tests/ci.sh                  # all of it
tests/ci.sh --no-flatpak     # without the Flatpak build
tests/ci.sh --quick          # just tests/run.sh ci here, no containers
```

Run it before a release, and after changes to the build or to code that
uses newer GTK or libadwaita: a change that works with your GTK can still
break on Debian's older one. It needs [Podman](https://podman.io)
(recommended) or Docker. The first run builds the images and downloads a
lot (about 5 GB); later runs reuse them.

Its two halves also run on their own:

```sh
tests/container.sh                      # Debian 13, Fedora and Arch: every step, build too
tests/container.sh debian               # Debian 13: the oldest GTK (4.18) and libadwaita (1.7)
tests/container.sh arch -- test lint    # arguments after -- go to tests/run.sh
tests/container.sh --rebuild fedora     # rebuild the image: newer Rust and packages

tests/flatpak.sh                        # cargo-sources.json is current, and the Flatpak builds
tests/flatpak.sh --sources              # only the cargo-sources.json check
tests/flatpak.sh --fix                  # regenerate cargo-sources.json after Cargo.lock changed
tests/flatpak.sh --host                 # with flatpak-builder on this machine, no container
```

`tests/flatpak.sh` uses the image CI used
(`ghcr.io/flathub-infra/flatpak-github-actions:gnome-49`). The SDKs it
downloads the first time are kept in the volume `corewatch-flatpak`.

### Disk space

Everything these scripts build stays in `target/` and in container images,
apart from your own build. To take the space back:

```sh
rm -rf target/containers target/flatpak
podman volume rm corewatch-flatpak                 # or docker volume rm
podman rmi localhost/corewatch-test-debian localhost/corewatch-test-fedora \
    localhost/corewatch-test-arch ghcr.io/flathub-infra/flatpak-github-actions:gnome-49
```

### Behind a proxy

Set `HTTPS_PROXY` (and `HTTP_PROXY`, `NO_PROXY` if you need them) as
usual. The scripts pass them to every image build and container and give
the containers the host's network, so a proxy on `127.0.0.1` works too.
`COREWATCH_ENGINE=docker` picks Docker when both engines are installed.

## Where the tests are

Cargo looks for tests in fixed places, so they stay next to the code:

| Where | What |
| --- | --- |
| `crates/core/src/**` (`#[cfg(test)] mod tests`) | Unit tests of the parsers, process signals and error handling |
| `crates/core/tests/collectors.rs` | Each collector against a fake machine |
| `crates/core/tests/fixtures/<machine>/` | The fake `/proc` and `/sys` trees: `basic`, `desktop`, `laptop`, `gpus`, `hybrid`, `arm`, ... |
| `crates/app/src/main.rs` | The template test: every Blueprint file loads and every page builds |
| `crates/app/src/**` | Unit tests of formatting, settings, alerts, graphs, app icons and the process search |

This folder holds what runs them: the scripts above and the images in
`containers/`.

A new fixture machine is a folder under `crates/core/tests/fixtures/` with
only the files the test needs, copied from a real `/proc` and `/sys`. Point
the app at one to see it in the real window:

```sh
COREWATCH_SYSROOT=crates/core/tests/fixtures/laptop cargo run -p corewatch
```
