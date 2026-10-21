#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Bendix Hüsing
#
# Build Stijl for macOS: app bundle and disk image (.dmg).
#
# Requirements: macOS on Apple silicon, Homebrew (https://brew.sh) and Rust
# (https://rustup.rs). Run inside the Stijl folder:
#
#     bash packaging/macos/build.sh
#
# Result:
#   target/macos/Stijl.app                         the finished app
#   target/macos/Stijl-<version>-macOS.dmg         for distribution
#
# SKIP_DEPS=1 skips installing the Homebrew packages (e.g. in GitHub Actions).
#
# The app is only signed “ad hoc”. Without an Apple developer account (Developer ID and
# notarisation), users have to allow it on first start under System Settings › Privacy &
# Security › “Open Anyway”.
set -euo pipefail

cd "$(dirname "$0")/../.."
ROOT=$PWD

if [[ "$(uname)" != Darwin ]]; then
    echo "This script only runs on macOS." >&2
    exit 1
fi
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
# Homebrew libraries are built for the macOS version they were installed on, so older
# systems cannot run the app
MINOS=${MINOS:-$(sw_vers -productVersion | cut -d. -f1).0}
export MACOSX_DEPLOYMENT_TARGET=$MINOS
echo "==> Stijl $VERSION for macOS $MINOS+ ($(uname -m))"

# ───── 1. Tools and libraries
if [[ "${SKIP_DEPS:-}" != 1 ]]; then
    echo "==> Installing Homebrew packages"
    brew install gtk4 libadwaita librsvg adwaita-icon-theme hicolor-icon-theme pkgconf dylibbundler
fi
BREW=$(brew --prefix)

# ───── 2. Compile
echo "==> Compiling (cargo build --release)"
cargo build --release

# ───── 3. Assemble the app bundle
OUT=$ROOT/target/macos
APP=$OUT/Stijl.app
C=$APP/Contents
RES=$C/Resources
echo "==> $APP"
rm -rf "$APP"
mkdir -p "$C/MacOS" "$C/Frameworks" "$RES/share"
cp target/release/stijl "$C/MacOS/stijl"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@MINOS@/$MINOS/g" packaging/macos/Info.plist >"$C/Info.plist"
printf 'APPL????' >"$C/PkgInfo"

# app icon from the SVG
ICONSET=$OUT/stijl.iconset
rm -rf "$ICONSET"
mkdir -p "$ICONSET"
SVG=data/io.github.benhuesing.Stijl.svg
for s in 16 32 128 256 512; do
    rsvg-convert -w $s -h $s "$SVG" -o "$ICONSET/icon_${s}x${s}.png"
    rsvg-convert -w $((s * 2)) -h $((s * 2)) "$SVG" -o "$ICONSET/icon_${s}x${s}@2x.png"
done
iconutil -c icns "$ICONSET" -o "$RES/stijl.icns"
rm -rf "$ICONSET"

# gdk-pixbuf image loaders (including SVG for the icons). The loader list contains absolute
# paths; in the bundle they hold a placeholder that Stijl replaces on start (main.rs).
PIXREL=lib/gdk-pixbuf-2.0/2.10.0
CACHE=$BREW/$PIXREL/loaders.cache
mkdir -p "$RES/$PIXREL/loaders"
LOADERS=()
while read -r so; do
    [[ -f "$so" ]] || continue
    cp "$so" "$RES/$PIXREL/loaders/"
    LOADERS+=("$RES/$PIXREL/loaders/$(basename "$so")")
done < <(sed -nE 's/^"(\/.*\.(so|dylib))"$/\1/p' "$CACHE")
sed -E "s#^\"/.*/([^/]+\\.(so|dylib))\"\$#\"@RESOURCES@/$PIXREL/loaders/\\1\"#" "$CACHE" >"$RES/$PIXREL/loaders.cache.in"
echo "    ${#LOADERS[@]} image loaders"

# copy the Homebrew libraries into the bundle and rewrite the references
echo "==> Collecting libraries (dylibbundler)"
FIX=(-x "$C/MacOS/stijl")
for l in "${LOADERS[@]}"; do
    FIX+=(-x "$l")
done
dylibbundler -od -b "${FIX[@]}" -d "$C/Frameworks" -p @executable_path/../Frameworks/ \
    -s "$BREW/lib" >/dev/null
echo "    $(ls "$C/Frameworks" | wc -l | tr -d ' ') libraries"

# settings schemas, icons, translations of GTK/libadwaita
mkdir -p "$RES/share/glib-2.0/schemas"
cp "$BREW/share/glib-2.0/schemas/gschemas.compiled" "$RES/share/glib-2.0/schemas/"
mkdir -p "$RES/share/icons/hicolor"
cp -R "$BREW/share/icons/Adwaita" "$RES/share/icons/"
cp "$BREW/share/icons/hicolor/index.theme" "$RES/share/icons/hicolor/"
if command -v gtk4-update-icon-cache >/dev/null; then
    gtk4-update-icon-cache -q -t -f "$RES/share/icons/Adwaita" || true
fi
for lang in de en_GB; do
    for mo in gtk40 libadwaita glib20; do
        src="$BREW/share/locale/$lang/LC_MESSAGES/$mo.mo"
        if [[ -f "$src" ]]; then
            mkdir -p "$RES/share/locale/$lang/LC_MESSAGES"
            cp "$src" "$RES/share/locale/$lang/LC_MESSAGES/"
        fi
    done
done

# licences
cp LICENSE "$RES/LICENSE.txt"
echo "==> THIRD-PARTY.txt"
{
    echo "Stijl $VERSION"
    echo "Copyright © 2026 Bendix Hüsing"
    echo "Licence: GNU General Public License, version 3 or later (LICENSE.txt)"
    echo
    echo "This macOS build contains libraries from other projects under their own licences"
    echo "(mostly LGPL, MIT, BSD, Zlib). They are taken unmodified from Homebrew; source code"
    echo "and build recipes: https://formulae.brew.sh"
    echo
    echo "─── Homebrew formulae"
    FORMULAE=$( (echo gtk4 libadwaita librsvg adwaita-icon-theme hicolor-icon-theme;
        brew deps --installed --union gtk4 libadwaita librsvg) | tr ' ' '\n' | sort -u)
    # shellcheck disable=SC2086
    brew info --json=v2 $FORMULAE | python3 -c '
import json, sys
for f in json.load(sys.stdin)["formulae"]:
    lic = f.get("license") or "?"
    ver = (f.get("installed") or [{}])[0].get("version", "")
    print("%s %s\n    Licence: %s\n    %s" % (f["name"], ver, lic, f.get("homepage", "")))
'
    echo
    echo "─── Rust crates"
    cargo tree -e normal --prefix none --format "{p}  –  {l}" 2>/dev/null |
        sed 's/ (\*)$//' | grep -v '^stijl ' | sort -u || true
} >"$RES/THIRD-PARTY.txt"

# sign ad hoc (without a valid signature the app does not start on Apple silicon)
echo "==> Signing (ad hoc)"
find "$C/Frameworks" "$RES/lib" -type f \( -name "*.dylib" -o -name "*.so" \) -print0 |
    xargs -0 codesign --force --sign -
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"
echo "    Size: $(du -sh "$APP" | cut -f1)"

# ───── 4. Disk image
echo "==> Disk image"
DMG=$OUT/Stijl-$VERSION-macOS.dmg
STAGE=$OUT/dmg
rm -rf "$STAGE" "$DMG"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -quiet -volname "Stijl $VERSION" -srcfolder "$STAGE" -ov -format UDZO "$DMG"
rm -rf "$STAGE"
echo
echo "Done: target/macos/Stijl-$VERSION-macOS.dmg"
