#!/bin/sh
# Run tests/run.sh inside a container of each distribution Corewatch
# supports, as the CI workflow did: Debian 13 (the oldest GTK and
# libadwaita), Fedora and Arch (the newest). Needs Podman or Docker.
#
#   tests/container.sh                    all three, every step of the old CI
#   tests/container.sh debian             one of debian, fedora, arch
#   tests/container.sh arch -- test lint  arguments after -- go to run.sh
#   tests/container.sh --rebuild debian   rebuild the image (new Rust, new packages)
#
# Images are built once from tests/containers/ and reused. Build output
# and downloaded crates stay in target/containers/<distro>/, apart from
# the build on this machine. A proxy in HTTPS_PROXY is passed through.
set -u

cd "$(dirname "$0")/.." || exit 1
repo=$(pwd)
. scripts/lib/containers.sh

usage() {
    sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'
}

distros=""
rebuild=false
while [ $# -gt 0 ]; do
    case $1 in
        debian | fedora | arch) distros="$distros $1" ;;
        all) distros="debian fedora arch" ;;
        --rebuild) rebuild=true ;;
        --) shift; break ;;
        -h | --help) usage; exit 0 ;;
        *) echo "container.sh: unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done
[ -n "$distros" ] || distros="debian fedora arch"
# Without arguments for run.sh: everything the CI job ran, the release build too.
[ $# -gt 0 ] || set -- versions fmt lint test data build

cw_engine
if [ "$engine" = podman ]; then
    # Files written in the container belong to you, not to root.
    user_args="--userns=keep-id"
else
    user_args="--user $(id -u):$(id -g)"
fi

summary=""
failed=false
for distro in $distros; do
    image="localhost/corewatch-test-$distro"
    printf '\n\033[1m=== %s ===\033[0m\n' "$distro"

    if ! cw_image "$image" "tests/containers/$distro.Containerfile" "$rebuild"; then
        summary="$summary
  $(printf '%-7s' "$distro") image build FAILED"
        failed=true
        continue
    fi

    work="target/containers/$distro"
    mkdir -p "$work/cargo-home" "$work/home"
    # :z lets the container read the folder on SELinux hosts (Fedora).
    # shellcheck disable=SC2046,SC2086 # word splitting wanted
    if $engine run --rm --init $user_args $(cw_proxy_args -e) $cw_git_env \
        -v "$repo:/src:z" -w /src \
        -e CARGO_TARGET_DIR="/src/$work/target" \
        -e CARGO_HOME="/src/$work/cargo-home" \
        -e HOME="/src/$work/home" \
        -e TERM="${TERM:-dumb}" \
        "$image" tests/run.sh "$@"; then
        summary="$summary
  $(printf '%-7s' "$distro") passed"
    else
        summary="$summary
  $(printf '%-7s' "$distro") FAILED"
        failed=true
    fi
done

printf '\n\033[1mDistributions\033[0m%s\n' "$summary"
if $failed; then
    exit 1
fi
