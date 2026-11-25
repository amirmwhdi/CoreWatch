#!/bin/sh
# In the Debian image: the AppImage, from the vendored source tarball.
# GTK and libadwaita come from the system it runs on (4.18 / 1.7 or newer).
# Usage: appimage.sh <version> <arch: x86_64|aarch64>
set -eu
v=$1 arch=$2
app=io.github.amirmwhdi.Corewatch

mkdir -p /tmp/appimage && cd /tmp/appimage
tar -xf "/work/corewatch-$v.tar.xz"
(
    cd "corewatch-$v"
    # shellcheck disable=SC1091 # in the unpacked tarball
    . scripts/cargo-offline.sh
    meson setup build --prefix=/usr
    meson compile -C build
    DESTDIR=/tmp/appimage/stage meson install -C build
)

mkdir -p AppDir
cp -r stage/usr AppDir/usr
cp "AppDir/usr/share/applications/$app.desktop" AppDir/
# appimagetool wants a PNG for .DirIcon (it will refuse an SVG one).
rsvg-convert -w 256 -h 256 -o "AppDir/$app.png" \
    "AppDir/usr/share/icons/hicolor/scalable/apps/$app.svg"
ln -s "$app.png" AppDir/.DirIcon
cat >AppDir/AppRun <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/corewatch" "$@"
EOF
chmod +x AppDir/AppRun

# The tool and the runtime it puts in front of the AppImage. Downloaded with
# curl, which honours HTTPS_PROXY: appimagetool's own download does not.
tools=/work/appimage-tools
mkdir -p "$tools"
tool="$tools/appimagetool-$arch.AppImage"
runtime="$tools/runtime-$arch"
[ -s "$tool" ] || curl -fL -o "$tool" \
    "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$arch.AppImage"
[ -s "$runtime" ] || curl -fL -o "$runtime" \
    "https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-$arch"
chmod +x "$tool"
ARCH=$arch "$tool" --appimage-extract-and-run --runtime-file "$runtime" \
    AppDir "/out/corewatch-$v-linux-$arch.AppImage"
