#!/bin/sh
# Everything the CI workflow ran on GitHub, on this machine:
#
#   1. tests/container.sh: versions, formatting, Clippy, tests, desktop entry
#      and metainfo, and a release build, on Debian 13, Fedora and Arch;
#   2. tests/flatpak.sh: cargo-sources.json matches Cargo.lock, and the
#      Flatpak builds.
#
#   tests/ci.sh                 all of it (Podman or Docker; the first run takes a while)
#   tests/ci.sh --no-flatpak    without the Flatpak build
#   tests/ci.sh --quick         only tests/run.sh ci on this machine, no containers
set -u

cd "$(dirname "$0")/.." || exit 1

usage() {
    sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
}

flatpak=true
for arg in "$@"; do
    case $arg in
        --no-flatpak) flatpak=false ;;
        --quick) exec tests/run.sh ci ;;
        -h | --help) usage; exit 0 ;;
        *) echo "ci.sh: unknown argument: $arg" >&2; usage >&2; exit 2 ;;
    esac
done

status=0
tests/container.sh || status=1
if $flatpak; then
    printf '\n\033[1m=== flatpak ===\033[0m\n'
    if tests/flatpak.sh; then
        flatpak_result=passed
    else
        flatpak_result=FAILED
        status=1
    fi
    printf '\n\033[1mFlatpak\033[0m\n  %s\n' "$flatpak_result"
fi

if [ $status = 0 ]; then
    printf '\n\033[32mCI passed.\033[0m\n'
else
    printf '\n\033[31mCI FAILED.\033[0m\n'
fi
exit $status
