#!/bin/bash
# Build a self-contained Waroxide.app (and a .dmg) for Apple Silicon Macs.
#
# The app contains the Rust launcher and extractor from this repository,
# the Stratagus engine with the libraries it needs, and the Wargus game
# scripts. Users only need their own Warcraft II CD or CD image.
#
# Requirements (build machine only): Xcode command line tools, Rust, and
#   brew install cmake sdl2_mixer sdl2_image dylibbundler
#
# Usage: macos/build-app.sh            -> target/macos/Waroxide.app, Waroxide.dmg
set -euo pipefail

STRATAGUS_REPO=https://github.com/Wargus/stratagus.git
STRATAGUS_COMMIT=3d87c93f7fd8c0b62ee1be5df0a6d9efc72ca6cc
WARGUS_REPO=https://github.com/Wargus/wargus.git
WARGUS_COMMIT=57f9d4d2ee557c9d725cb830752e394f4fb00c51

ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
WORK="$ROOT/target/macos"
APP="$WORK/Waroxide.app"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"

step() { printf '\n==> %s\n' "$*"; }

for tool in cmake dylibbundler cargo; do
    command -v "$tool" >/dev/null || { echo "missing $tool (see the requirements at the top of $0)"; exit 1; }
done
for formula in sdl2_mixer sdl2_image sdl3; do
    brew list --versions "$formula" >/dev/null || { echo "missing Homebrew package $formula"; exit 1; }
done

# checkout <repo> <commit> <dir>
checkout() {
    if [ ! -d "$3/.git" ]; then
        git clone --quiet "$1" "$3"
    fi
    git -C "$3" fetch --quiet origin "$2" 2>/dev/null || git -C "$3" fetch --quiet origin
    git -C "$3" checkout --quiet "$2"
    git -C "$3" submodule update --quiet --init --recursive
}

mkdir -p "$WORK"

step "Fetching Stratagus ${STRATAGUS_COMMIT:0:10} and Wargus ${WARGUS_COMMIT:0:10}"
checkout "$STRATAGUS_REPO" "$STRATAGUS_COMMIT" "$WORK/stratagus"
checkout "$WARGUS_REPO" "$WARGUS_COMMIT" "$WORK/wargus"

step "Building Stratagus"
# CMake caches absolute paths: start over if the checkout was moved.
CACHE="$WORK/build-stratagus/CMakeCache.txt"
if [ -f "$CACHE" ] && ! grep -q "^CMAKE_HOME_DIRECTORY:INTERNAL=$WORK/stratagus$" "$CACHE"; then
    rm -rf "$WORK/build-stratagus"
fi
cmake -S "$WORK/stratagus" -B "$WORK/build-stratagus" \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_FIND_FRAMEWORK=LAST \
    -DBUILD_VENDORED_LUA=ON \
    -DBUILD_VENDORED_SDL=OFF \
    -DBUILD_VENDORED_MEDIA_LIBS=OFF >/dev/null
cmake --build "$WORK/build-stratagus" -j "$(sysctl -n hw.ncpu)" >/dev/null

step "Building the launcher and extractor"
cargo build --release --manifest-path "$ROOT/Cargo.toml" --bin waroxide --bin wartool

step "Assembling Waroxide.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/libs"
cp "$ROOT/target/release/waroxide" "$ROOT/target/release/wartool" "$APP/Contents/MacOS/"
cp "$WORK/build-stratagus/stratagus" "$APP/Contents/MacOS/"
# The scripts, maps, campaigns, shaders and extra graphics the launcher
# copies next to the extracted data.
for dir in campaigns contrib maps scripts shaders; do
    cp -R "$WORK/wargus/$dir" "$APP/Contents/Resources/"
done
sed "s/@VERSION@/$VERSION/" "$ROOT/macos/Info.plist" > "$APP/Contents/Info.plist"

ICONSET="$WORK/wargus.iconset"
rm -rf "$ICONSET" && mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
    sips -z $size $size "$WORK/wargus/wargus.png" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    sips -z $((size * 2)) $((size * 2)) "$WORK/wargus/wargus.png" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/wargus.icns"
rm -rf "$ICONSET"

step "Bundling libraries"
dylibbundler -of -cd -b -x "$APP/Contents/MacOS/stratagus" -d "$APP/Contents/libs/" -p @executable_path/../libs/ >/dev/null 2>&1
# Homebrew's SDL2 is sdl2-compat, which dlopen()s SDL3 from next to itself;
# dylibbundler cannot see that dependency.
cp "$(brew --prefix sdl3)/lib/libSDL3.0.dylib" "$APP/Contents/libs/libSDL3.dylib"
chmod u+w "$APP/Contents/libs/libSDL3.dylib"
install_name_tool -id @loader_path/libSDL3.dylib "$APP/Contents/libs/libSDL3.dylib" 2>/dev/null

# Nothing may still point into Homebrew.
if otool -L "$APP"/Contents/MacOS/* "$APP"/Contents/libs/* | grep -q /opt/homebrew; then
    otool -L "$APP"/Contents/MacOS/* "$APP"/Contents/libs/* | grep /opt/homebrew
    echo "error: the app still references Homebrew libraries"
    exit 1
fi

step "Signing (ad hoc)"
# Sign inside out: libraries (Contents/libs is not a standard location, so
# --deep would skip them and macOS kills the process when one with a broken
# signature is loaded), then executables, then the bundle.
for f in "$APP"/Contents/libs/*.dylib "$APP"/Contents/MacOS/*; do
    codesign --force --sign - "$f" 2>/dev/null
done
codesign --force --sign - "$APP" 2>/dev/null
for f in "$APP"/Contents/libs/*.dylib "$APP"/Contents/MacOS/*; do
    codesign --verify "$f" || { echo "error: bad signature on $f"; exit 1; }
done

step "Creating Waroxide.dmg"
DMG_DIR="$WORK/dmg"
rm -rf "$DMG_DIR" "$WORK/Waroxide.dmg" && mkdir -p "$DMG_DIR"
cp -R "$APP" "$DMG_DIR/"
ln -s /Applications "$DMG_DIR/Applications"
hdiutil create -quiet -volname Waroxide -srcfolder "$DMG_DIR" -format UDZO "$WORK/Waroxide.dmg"
rm -rf "$DMG_DIR"

MIN_OS="$(otool -l "$APP/Contents/libs/libSDL3.dylib" | awk '/LC_BUILD_VERSION/{f=1} f&&/minos/{print $2; exit}')"
step "Done: $APP ($(du -sh "$APP" | cut -f1)), $WORK/Waroxide.dmg (needs macOS $MIN_OS or later)"
