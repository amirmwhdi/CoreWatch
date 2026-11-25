#!/bin/sh
# The version of Corewatch is written in many files. This script keeps them
# in step.
#
#   scripts/version.sh                 print the current version
#   scripts/version.sh check           fail when a file names another version
#   scripts/version.sh list            print the version each file names
#   scripts/version.sh bump <version> <notes-file> [<date>]
#                                      move every file to <version>
#
# <notes-file> holds the release notes, one change per line (empty lines and
# lines starting with # are skipped). They become the metainfo release, the
# Debian changelog entry and the RPM changelog entry. <date> is YYYY-MM-DD,
# today by default.
#
# After a bump: review `git diff`, commit as "release: <version>", tag
# v<version>, push the tag, build and publish the packages
# (release/build.sh, release/publish.sh), then fill in the commit of the
# Flathub manifest (packaging/flathub/README.md). See release/README.md.
set -eu

cd "$(dirname "$0")/.."

cargo_toml=Cargo.toml
cargo_lock=Cargo.lock
meson=meson.build
metainfo=data/io.github.amirmwhdi.Corewatch.metainfo.xml
pkgbuild=packaging/arch/PKGBUILD
deb_changelog=packaging/debian/changelog
spec=packaging/rpm/corewatch.spec
flathub=packaging/flathub/io.github.amirmwhdi.Corewatch.json
readme=README.md

maintainer='Amirmahdi <amirmwhdi@users.noreply.github.com>'

die() {
    echo "version.sh: $*" >&2
    exit 1
}

current() {
    sed -n 's/^version = "\(.*\)"$/\1/p' "$cargo_toml" | head -n 1
}

# Every place a version is written, one "file: version" line each.
found() {
    printf '%s: %s\n' "$meson" "$(sed -n "s/^  version: '\(.*\)',$/\1/p" "$meson")"
    for crate in corewatch corewatch-core; do
        printf '%s (%s): %s\n' "$cargo_lock" "$crate" "$(awk -v name="$crate" '
            $0 == "name = \"" name "\"" { getline; gsub(/version = |"/, ""); print; exit }
        ' "$cargo_lock")"
    done
    printf '%s (newest release): %s\n' "$metainfo" \
        "$(sed -n 's/^ *<release version="\([^"]*\)".*/\1/p' "$metainfo" | head -n 1)"
    sed -n 's|.*raw.githubusercontent.com/[^/]*/[^/]*/v\([^/]*\)/.*|\1|p' "$metainfo" |
        sort -u | sed "s|^|$metainfo (screenshots): |"
    printf '%s: %s\n' "$pkgbuild" "$(sed -n 's/^pkgver=//p' "$pkgbuild")"
    printf '%s: %s\n' "$deb_changelog" "$(sed -n '1s/^corewatch (\(.*\)-[0-9]*).*/\1/p' "$deb_changelog")"
    printf '%s: %s\n' "$spec" "$(sed -n 's/^Version: *//p' "$spec")"
    printf '%s (changelog): %s\n' "$spec" \
        "$(sed -n 's/^\* .* - \(.*\)-[0-9]*$/\1/p' "$spec" | head -n 1)"
    printf '%s: %s\n' "$flathub" "$(flathub_tag)"
    sed -n 's/.*corewatch[-_]\([0-9][0-9.]*[0-9]\)[-_].*/\1/p' "$readme" |
        sort -u | sed "s|^|$readme (install commands): |"
}

# The tag of Corewatch's own source in the Flathub manifest: the "tag" line
# right after its url (the manifest also builds blueprint-compiler from a tag).
flathub_tag() {
    awk '/"url": "https:\/\/github.com\/amirmwhdi\/CoreWatch.git"/ { getline; gsub(/.*"v|",$/, ""); print; exit }' "$flathub"
}

check() {
    version=$(current)
    [ -n "$version" ] || die "no version in $cargo_toml"
    bad=$(found | grep -v ": $version\$" || true)
    if [ -n "$bad" ]; then
        echo "$cargo_toml says $version, but:" >&2
        echo "$bad" >&2
        echo "Run scripts/version.sh bump to move every file to one version." >&2
        exit 1
    fi
    echo "$version"
}

# XML text: & < > escaped.
xml_escape() {
    sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'
}

bump() {
    new=$1
    notes_file=$2
    day=${3:-$(date +%Y-%m-%d)}
    old=$(current)

    echo "$new" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || die "'$new' is not a version like 1.2.3"
    echo "$day" | grep -Eq '^[0-9]{4}-[0-9]{2}-[0-9]{2}$' || die "'$day' is not a date like 2026-11-03"
    [ -r "$notes_file" ] || die "cannot read $notes_file"
    [ "$new" != "$old" ] || die "the version is already $new"
    if grep -q "<release version=\"$new\"" "$metainfo"; then
        die "$metainfo already has a release $new"
    fi

    notes=$(grep -v '^[[:space:]]*#' "$notes_file" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' |
        grep -v '^$' || true)
    [ -n "$notes" ] || die "$notes_file has no release notes"

    deb_date=$(LC_ALL=C date -u -d "$day 12:00" '+%a, %d %b %Y %H:%M:%S +0000')
    rpm_date=$(LC_ALL=C date -u -d "$day 12:00" '+%a %b %d %Y')

    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT

    # Cargo.toml: only the workspace version, the first `version =` line.
    awk -v v="$new" '!done && /^version = "/ { $0 = "version = \"" v "\""; done = 1 } 1' \
        "$cargo_toml" >"$tmp/f" && cat "$tmp/f" >"$cargo_toml"

    # Cargo.lock: the two workspace crates, not a dependency of that version.
    awk -v v="$new" '
        /^name = "corewatch(-core)?"$/ { print; getline; $0 = "version = \"" v "\"" }
        1' "$cargo_lock" >"$tmp/f" && cat "$tmp/f" >"$cargo_lock"

    sed -i "s/^  version: '.*',$/  version: '$new',/" "$meson"

    # metainfo: screenshots from the new tag, and the new release on top.
    sed -i "s|\(raw.githubusercontent.com/[^/]*/[^/]*/\)v[^/]*/|\1v$new/|" "$metainfo"
    {
        printf '    <release version="%s" date="%s">\n' "$new" "$day"
        printf '      <description>\n        <ul>\n'
        echo "$notes" | xml_escape | sed 's|.*|          <li>&</li>|'
        printf '        </ul>\n      </description>\n    </release>\n'
    } >"$tmp/release"
    awk -v file="$tmp/release" '
        { print }
        /^  <releases>$/ { while ((getline line < file) > 0) print line }
    ' "$metainfo" >"$tmp/f" && cat "$tmp/f" >"$metainfo"

    sed -i -e "s/^pkgver=.*/pkgver=$new/" -e "s/^pkgrel=.*/pkgrel=1/" "$pkgbuild"

    {
        echo "corewatch ($new-1) unstable; urgency=medium"
        echo
        echo "$notes" | sed 's/^/  * /'
        echo
        echo " -- $maintainer  $deb_date"
        echo
        cat "$deb_changelog"
    } >"$tmp/f" && cat "$tmp/f" >"$deb_changelog"

    {
        echo "* $rpm_date $maintainer - $new-1"
        echo "$notes" | sed 's/^/- /'
        echo
    } >"$tmp/rpm"
    awk -v file="$tmp/rpm" '
        { print }
        /^%changelog$/ { while ((getline line < file) > 0) print line }
    ' "$spec" >"$tmp/f" && cat "$tmp/f" >"$spec"
    sed -i -e "s/^\(Version: *\).*/\1$new/" -e "s/^\(Release: *\)[0-9]*/\11/" "$spec"

    # The commit is known only after tagging.
    awk -v v="$new" '
        /"url": "https:\/\/github.com\/amirmwhdi\/CoreWatch.git"/ { own = 1 }
        own && /"tag": / { sub(/"v[^"]*"/, "\"v" v "\"") }
        own && /"commit": / { sub(/: "[^"]*"/, ": \"FILL-IN-AFTER-TAGGING\""); own = 0 }
        1' "$flathub" >"$tmp/f" && cat "$tmp/f" >"$flathub"

    major_minor=$(echo "$new" | cut -d. -f1-2)
    sed -i -e "s/\(corewatch[-_]\)$old\([-_]\)/\1$new\2/g" \
        -e "s/^> \*\*Status: [0-9.]*, preview\.\*\*/> **Status: $major_minor, preview.**/" "$readme"

    check >/dev/null
    echo "$old -> $new"
    echo "Review with git diff, then commit \"release: $new\" and tag v$new."
}

case ${1:-} in
"") current ;;
check) check ;;
list) found ;;
bump)
    [ $# -ge 3 ] || die "usage: scripts/version.sh bump <version> <notes-file> [<date>]"
    bump "$2" "$3" "${4:-}"
    ;;
*) die "unknown command '$1' (use check or bump)" ;;
esac
