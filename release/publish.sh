#!/bin/sh
# Publish the packages in dist/<version>/ as a GitHub release, as the
# Release workflow's last job did. Needs the GitHub CLI, signed in:
# `gh auth login`.
#
#   release/publish.sh           publish the version in Cargo.toml
#   release/publish.sh --draft   as a draft, to check on GitHub before anyone sees it
#   release/publish.sh --yes     do not ask before publishing
#
# Before it publishes it checks that the tag v<version> is the commit the
# packages were built from and is pushed, and that only installable
# packages are uploaded. See release/README.md.
set -eu

cd "$(dirname "$0")/.." || exit 1

usage() {
    sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
}

draft="" yes=false
for arg in "$@"; do
    case $arg in
        --draft) draft=--draft ;;
        --yes) yes=true ;;
        -h | --help) usage; exit 0 ;;
        *) echo "publish.sh: unknown argument: $arg" >&2; usage >&2; exit 2 ;;
    esac
done

die() {
    echo "publish.sh: $*" >&2
    exit 1
}

command -v gh >/dev/null 2>&1 || die "install the GitHub CLI: https://cli.github.com"
gh auth status >/dev/null 2>&1 || die "sign in first: gh auth login"

version=$(scripts/version.sh check)
tag="v$version"
dist="dist/$version"

# The packages were built from HEAD: the tag must name that very commit.
[ -z "$(git status --porcelain --untracked-files=no)" ] ||
    die "there are uncommitted changes; commit them, tag, and build again"
git rev-parse -q --verify "refs/tags/$tag" >/dev/null ||
    die "no tag $tag. Commit 'release: $version', then: git tag $tag && git push origin $tag"
[ "$(git rev-parse "$tag^{commit}")" = "$(git rev-parse HEAD)" ] ||
    die "$tag is not the current commit, which the packages were built from"
# The tag on GitHub must be this one too ("^{}" is the commit of an annotated tag).
remote=$(git ls-remote --tags origin "refs/tags/$tag" "refs/tags/$tag^{}" 2>/dev/null |
    sort -k2 | tail -n 1 | cut -f1)
[ -n "$remote" ] || die "$tag is not on GitHub yet: git push origin $tag"
[ "$remote" = "$(git rev-parse HEAD)" ] ||
    die "$tag on GitHub is another commit than yours; push the right tag first"

[ -d "$dist" ] || die "no $dist/: run release/build.sh first"
files=""
for f in "$dist"/*; do
    case $f in
        *.deb | *.rpm | *.pkg.tar.zst | *.AppImage | *.flatpak) files="$files $f" ;;
        */SHA256SUMS) ;;
        *) die "unexpected file in $dist: ${f##*/} (only packages are published)" ;;
    esac
done
[ -n "$files" ] || die "no packages in $dist/: run release/build.sh first"

# shellcheck disable=SC2046,SC2086 # $files holds our own file names, no spaces
(cd "$dist" && sha256sum $(for f in $files; do echo "${f##*/}"; done) >SHA256SUMS)

# The packages the Release workflow made; say which are missing.
missing=""
for name in linux-amd64.deb linux-arm64.deb linux-x86_64.rpm linux-aarch64.rpm \
    linux-x86_64.pkg.tar.zst linux-x86_64.AppImage linux-aarch64.AppImage \
    x86_64.flatpak aarch64.flatpak; do
    [ -e "$dist/corewatch-$version-$name" ] || missing="$missing corewatch-$version-$name"
done

prerelease=""
case $version in *-*) prerelease=--prerelease ;; esac

echo "Release $tag${draft:+ (draft)}${prerelease:+ (pre-release)} with:"
for f in $files "$dist/SHA256SUMS"; do echo "  ${f##*/}"; done
if [ -n "$missing" ]; then
    echo "Not built (the ARM ones come from release/build.sh --arch aarch64):"
    for f in $missing; do echo "  $f"; done
fi

if ! $yes; then
    printf 'Publish? [y/N] '
    read -r answer
    case $answer in y | Y | yes) ;; *) echo "Nothing published."; exit 1 ;; esac
fi

# shellcheck disable=SC2086 # empty flags drop out, file names have no spaces
gh release create "$tag" --verify-tag --title "Corewatch $version" --generate-notes \
    $draft $prerelease $files "$dist/SHA256SUMS"
