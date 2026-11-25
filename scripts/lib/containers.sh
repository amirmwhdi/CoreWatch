# Shared by tests/*.sh and release/*.sh, which source this file. It picks
# Podman or Docker, builds the images under tests/containers and
# release/containers, and passes a proxy through to them.
#
# Environment:
#   COREWATCH_ENGINE=podman|docker   use this engine instead of guessing
#   HTTPS_PROXY, HTTP_PROXY, ALL_PROXY, NO_PROXY (or lower case)
#                                    passed to every build and container,
#                                    with host networking so a proxy on
#                                    127.0.0.1 works too

# Sets $engine, or exits.
cw_engine() {
    engine=${COREWATCH_ENGINE:-}
    if [ -z "$engine" ]; then
        if command -v podman >/dev/null 2>&1; then
            engine=podman
        elif command -v docker >/dev/null 2>&1; then
            engine=docker
        else
            echo "Install Podman (recommended) or Docker: https://podman.io" >&2
            exit 1
        fi
    fi
    command -v "$engine" >/dev/null 2>&1 || {
        echo "COREWATCH_ENGINE=$engine, but $engine is not installed." >&2
        exit 1
    }
}

# Whether files a container's root writes end up owned by root on this
# machine (Docker, or Podman run as root); rootless Podman maps them to you.
cw_root_owns_files() {
    case $engine in
        docker) return 0 ;;
        *) [ "$(podman info --format '{{.Host.Security.Rootless}}' 2>/dev/null)" != true ] ;;
    esac
}

# The proxy variables that are set, as `<flag> NAME=value` words for
# `run` (flag -e) or `build` (flag --build-arg). Proxy URLs hold no spaces.
cw_proxy_args() {
    _flag=$1
    _args=""
    for _var in HTTPS_PROXY https_proxy HTTP_PROXY http_proxy ALL_PROXY all_proxy NO_PROXY no_proxy; do
        eval "_value=\${$_var:-}"
        # shellcheck disable=SC2154 # set by the eval
        [ -n "$_value" ] && _args="$_args $_flag $_var=$_value"
    done
    [ -n "$_args" ] && _args="--network host$_args"
    echo "$_args"
}

# cw_image <image> <containerfile> <rebuild: true|false> [platform]
# Build the image unless it exists already. An image for another
# architecture needs a name of its own.
cw_image() {
    _platform=${4:+--platform $4}
    if ! $3 && $engine image inspect "$1" >/dev/null 2>&1; then
        return 0
    fi
    echo "Building the image $1 (once; --rebuild makes it again)"
    # shellcheck disable=SC2046,SC2086 # word splitting wanted
    $engine build --pull $_platform $(cw_proxy_args --build-arg) \
        -t "$1" -f "$2" "$(dirname "$2")"
}

# git refuses a repository that seems to belong to another user, which a
# mounted folder does inside a container.
# shellcheck disable=SC2034 # used by the scripts that source this file
cw_git_env="-e GIT_CONFIG_COUNT=1 -e GIT_CONFIG_KEY_0=safe.directory -e GIT_CONFIG_VALUE_0=*"

# The architecture of this machine, as x86_64 or aarch64.
cw_host_arch() {
    case $(uname -m) in
        arm64 | aarch64) echo aarch64 ;;
        *) uname -m ;;
    esac
}

# The container platform of an architecture.
cw_platform() {
    case $1 in
        x86_64) echo linux/amd64 ;;
        aarch64) echo linux/arm64 ;;
        *) echo "unsupported architecture: $1" >&2; return 1 ;;
    esac
}
