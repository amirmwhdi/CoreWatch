#!/bin/sh
# Run Corewatch's checks and tests on this machine.
#
#   tests/run.sh                  every step below but build, in order
#   tests/run.sh test lint        only the steps named
#   tests/run.sh ci               every step, build too: what CI ran
#   tests/run.sh --core test      only corewatch-core (no GTK needed)
#   tests/run.sh --fix fmt        format the code instead of checking it
#
# Steps:
#   versions  every file names the same version (scripts/version.sh check)
#   fmt       cargo fmt --check
#   lint      cargo clippy, warnings are errors
#   test      cargo test, under a virtual display when Xvfb is installed so
#             the template test builds the real window
#   data      validate the desktop entry and the metainfo
#   build     cargo build --release (only when named, or with ci)
#
# A failed step does not stop the others; the summary at the end lists
# every step, and the exit status is 1 when one failed. See tests/README.md.
set -u

cd "$(dirname "$0")/.." || exit 1

usage() {
    sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'
}

all_steps="versions fmt lint test data"
steps=""
core_only=false
fix=false

for arg in "$@"; do
    case $arg in
        --core) core_only=true ;;
        --fix) fix=true ;;
        -h | --help) usage; exit 0 ;;
        versions | fmt | lint | test | data | build) steps="$steps $arg" ;;
        ci) steps="$steps $all_steps build" ;;
        *) echo "run.sh: unknown argument: $arg" >&2; usage >&2; exit 2 ;;
    esac
done
[ -n "$steps" ] || steps=$all_steps

if $core_only; then
    packages="-p corewatch-core"
else
    packages="--workspace"
fi

if [ -t 1 ]; then
    bold=$(printf '\033[1m') red=$(printf '\033[31m') green=$(printf '\033[32m')
    yellow=$(printf '\033[33m') reset=$(printf '\033[0m')
else
    bold="" red="" green="" yellow="" reset=""
fi

summary=""
failed=false

record() { # <step> <passed|failed|skipped> [note]
    case $2 in
        passed) mark="${green}passed${reset}" ;;
        failed) mark="${red}FAILED${reset}"; failed=true ;;
        *) mark="${yellow}skipped${reset}" ;;
    esac
    summary="$summary
  $(printf '%-9s' "$1") $mark${3:+  ($3)}"
}

need() { # <command> <hint>
    command -v "$1" >/dev/null 2>&1 && return 0
    echo "${red}$1 is not installed.${reset} $2" >&2
    return 1
}

run_step() {
    printf '\n%s==> %s%s\n' "$bold" "$1" "$reset"
    case $1 in
        versions)
            scripts/version.sh check >/dev/null && echo "all files name $(scripts/version.sh)"
            ;;
        fmt)
            need cargo "Install Rust from https://rustup.rs" || return 1
            if $fix; then
                cargo fmt --all
            else
                cargo fmt --all --check
            fi
            ;;
        lint)
            need cargo "Install Rust from https://rustup.rs" || return 1
            # shellcheck disable=SC2086 # $packages is two words on purpose
            cargo clippy $packages --all-targets -- -D warnings
            ;;
        test)
            need cargo "Install Rust from https://rustup.rs" || return 1
            if ! $core_only && ! command -v xvfb-run >/dev/null 2>&1 &&
                ! command -v Xvfb >/dev/null 2>&1 && [ -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
                echo "${yellow}No display and no Xvfb: the template test will skip itself.${reset}"
            fi
            # shellcheck disable=SC2086
            scripts/run-with-display.sh cargo test $packages
            ;;
        build)
            need cargo "Install Rust from https://rustup.rs" || return 1
            # shellcheck disable=SC2086
            cargo build $packages --release
            ;;
        data)
            ok=0 ran=0
            if command -v desktop-file-validate >/dev/null 2>&1; then
                ran=1
                desktop-file-validate data/io.github.amirmwhdi.Corewatch.desktop && echo "desktop entry: valid" || ok=1
            fi
            if command -v appstreamcli >/dev/null 2>&1; then
                ran=1
                appstreamcli validate --no-net --explain data/io.github.amirmwhdi.Corewatch.metainfo.xml || ok=1
            fi
            if [ $ran = 0 ]; then
                echo "Neither desktop-file-validate nor appstreamcli is installed."
                return 3
            fi
            return $ok
            ;;
    esac
}

for step in $steps; do
    run_step "$step"
    case $? in
        0) record "$step" passed ;;
        3) record "$step" skipped "install desktop-file-utils and appstream" ;;
        *) record "$step" failed ;;
    esac
done

printf '\n%sSummary%s%s\n' "$bold" "$reset" "$summary"
if $failed; then
    exit 1
fi
