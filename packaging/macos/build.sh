#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Bendix Hüsing
#
# Stijl für macOS bauen: App-Bundle und Disk-Image (.dmg).
#
# Voraussetzungen: macOS auf Apple Silicon, Homebrew (https://brew.sh) und Rust
# (https://rustup.rs). Ausführen im Stijl-Ordner:
#
#     bash packaging/macos/build.sh
#
# Ergebnis:
#   target/macos/Stijl.app                         fertiges Programm
#   target/macos/Stijl-<Version>-macOS.dmg         zum Weitergeben
#
# SKIP_DEPS=1 überspringt die Installation der Homebrew-Pakete (z. B. in GitHub Actions).
#
# Die App ist nur „ad hoc“ signiert. Ohne Apple-Entwicklerkonto (Developer ID und
# Notarisierung) muss man sie beim ersten Start unter Systemeinstellungen › Datenschutz &
# Sicherheit › „Dennoch öffnen“ freigeben.
set -euo pipefail

cd "$(dirname "$0")/../.."
ROOT=$PWD

if [[ "$(uname)" != Darwin ]]; then
    echo "Dieses Skript läuft nur unter macOS." >&2
    exit 1
fi
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
# Homebrew-Bibliotheken sind für die macOS-Version gebaut, auf der sie installiert wurden;
# ältere Systeme können die App deshalb nicht starten
MINOS=${MINOS:-$(sw_vers -productVersion | cut -d. -f1).0}
export MACOSX_DEPLOYMENT_TARGET=$MINOS
echo "==> Stijl $VERSION für macOS $MINOS+ ($(uname -m))"

# ───── 1. Werkzeuge und Bibliotheken
if [[ "${SKIP_DEPS:-}" != 1 ]]; then
    echo "==> Homebrew-Pakete installieren"
    brew install gtk4 libadwaita librsvg adwaita-icon-theme hicolor-icon-theme pkgconf dylibbundler
fi
BREW=$(brew --prefix)

# ───── 2. Übersetzen
echo "==> Übersetzen (cargo build --release)"
cargo build --release

# ───── 3. App-Bundle zusammenstellen
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

# Programmsymbol aus dem SVG
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

# Bildlader von gdk-pixbuf (u. a. SVG für die Symbole). Die Liste der Lader enthält feste
# Pfade; im Bundle steht dort ein Platzhalter, den Stijl beim Start ersetzt (main.rs).
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
echo "    ${#LOADERS[@]} Bildlader"

# Bibliotheken aus Homebrew ins Bundle holen und die Verweise umschreiben
echo "==> Bibliotheken einsammeln (dylibbundler)"
FIX=(-x "$C/MacOS/stijl")
for l in "${LOADERS[@]}"; do
    FIX+=(-x "$l")
done
dylibbundler -od -b "${FIX[@]}" -d "$C/Frameworks" -p @executable_path/../Frameworks/ \
    -s "$BREW/lib" >/dev/null
echo "    $(ls "$C/Frameworks" | wc -l | tr -d ' ') Bibliotheken"

# Einstellungsschemas, Symbole, Übersetzungen von GTK/libadwaita
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

# Lizenzen
cp LICENSE "$RES/LICENSE.txt"
echo "==> THIRD-PARTY.txt"
{
    echo "Stijl $VERSION"
    echo "Copyright © 2026 Bendix Hüsing"
    echo "Lizenz: GNU General Public License, Version 3 oder später (LICENSE.txt)"
    echo
    echo "Diese Ausgabe für macOS enthält Bibliotheken anderer Projekte. Sie stehen unter"
    echo "ihren eigenen Lizenzen (überwiegend LGPL, MIT, BSD, Zlib) und stammen unverändert"
    echo "aus Homebrew; Quelltexte und Bauanleitungen: https://formulae.brew.sh"
    echo
    echo "This macOS build contains libraries from other projects under their own licences"
    echo "(mostly LGPL, MIT, BSD, Zlib), taken unmodified from Homebrew (sources: see above)."
    echo
    echo "─── Homebrew-Pakete / Homebrew formulae"
    FORMULAE=$( (echo gtk4 libadwaita librsvg adwaita-icon-theme hicolor-icon-theme;
        brew deps --installed --union gtk4 libadwaita librsvg) | tr ' ' '\n' | sort -u)
    # shellcheck disable=SC2086
    brew info --json=v2 $FORMULAE | python3 -c '
import json, sys
for f in json.load(sys.stdin)["formulae"]:
    lic = f.get("license") or "?"
    ver = (f.get("installed") or [{}])[0].get("version", "")
    print("%s %s\n    Lizenz: %s\n    %s" % (f["name"], ver, lic, f.get("homepage", "")))
'
    echo
    echo "─── Rust-Crates"
    cargo tree -e normal --prefix none --format "{p}  –  {l}" 2>/dev/null |
        sed 's/ (\*)$//' | grep -v '^stijl ' | sort -u || true
} >"$RES/THIRD-PARTY.txt"

# ad hoc signieren (ohne gültige Signatur startet die App auf Apple Silicon nicht)
echo "==> Signieren (ad hoc)"
find "$C/Frameworks" "$RES/lib" -type f \( -name "*.dylib" -o -name "*.so" \) -print0 |
    xargs -0 codesign --force --sign -
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"
echo "    Größe: $(du -sh "$APP" | cut -f1)"

# ───── 4. Disk-Image
echo "==> Disk-Image"
DMG=$OUT/Stijl-$VERSION-macOS.dmg
STAGE=$OUT/dmg
rm -rf "$STAGE" "$DMG"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -quiet -volname "Stijl $VERSION" -srcfolder "$STAGE" -ov -format UDZO "$DMG"
rm -rf "$STAGE"
echo
echo "Fertig: target/macos/Stijl-$VERSION-macOS.dmg"
