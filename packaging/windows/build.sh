#!/usr/bin/env bash
# Mondrian für Windows bauen und einen Installer erstellen.
#
# Ausführen in der MSYS2-Shell „UCRT64“ (https://www.msys2.org) im Mondrian-Ordner:
#
#     bash packaging/windows/build.sh
#
# Ergebnis:
#   target/windows/Mondrian/                     fertiger Programmordner (läuft auch ohne Installation)
#   target/windows/Mondrian-<Version>-Setup.exe  Installer (wenn Inno Setup 6 installiert ist)
#
# SKIP_DEPS=1 überspringt die Installation der MSYS2-Pakete (z. B. in GitHub Actions).
set -euo pipefail

cd "$(dirname "$0")/../.."
ROOT=$PWD

if [[ "${MSYSTEM:-}" != "UCRT64" ]]; then
    echo "Bitte in der MSYS2-Shell „UCRT64“ ausführen (Startmenü: MSYS2 UCRT64)." >&2
    exit 1
fi
PREFIX=${MINGW_PREFIX:-/ucrt64}
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
echo "==> Mondrian $VERSION für Windows"

# ───── 1. Werkzeuge und Bibliotheken
PKGS=(
    mingw-w64-ucrt-x86_64-gtk4
    mingw-w64-ucrt-x86_64-libadwaita
    mingw-w64-ucrt-x86_64-librsvg
    mingw-w64-ucrt-x86_64-adwaita-icon-theme
    mingw-w64-ucrt-x86_64-hicolor-icon-theme
    mingw-w64-ucrt-x86_64-rust
    mingw-w64-ucrt-x86_64-gcc
    mingw-w64-ucrt-x86_64-pkgconf
    mingw-w64-ucrt-x86_64-ntldd
)
if [[ "${SKIP_DEPS:-}" != 1 ]]; then
    echo "==> Pakete installieren (einmalig, dauert beim ersten Mal etwas)"
    pacman -S --needed --noconfirm "${PKGS[@]}"
fi

# ───── 2. Übersetzen
echo "==> Übersetzen (cargo build --release)"
cargo build --release

# ───── 3. Programmordner zusammenstellen
DIST=$ROOT/target/windows/Mondrian
echo "==> Programmordner $DIST"
rm -rf "$DIST"
mkdir -p "$DIST/bin" "$DIST/lib" "$DIST/share"

cp target/release/mondrian.exe "$DIST/bin/"
# Hilfsprogramme von GLib (Prozesse starten, D-Bus für „Datei im offenen Fenster öffnen“)
for f in gdbus.exe gspawn-win64-helper.exe gspawn-win64-helper-console.exe; do
    if [[ -f "$PREFIX/bin/$f" ]]; then
        cp "$PREFIX/bin/$f" "$DIST/bin/"
    fi
done

# Bildlader von gdk-pixbuf (u. a. SVG für die Symbole)
PIXREL=lib/gdk-pixbuf-2.0/2.10.0
mkdir -p "$DIST/$PIXREL/loaders"
cp "$PREFIX/$PIXREL/loaders/"*.dll "$DIST/$PIXREL/loaders/"

# alle benötigten DLLs aus MSYS2 einsammeln (auch die der Bildlader)
copy_deps() {
    local f dll
    for f in "$@"; do
        ntldd -R "$f" 2>/dev/null | sed -n 's/.*=> \(.*\) (0x.*/\1/p' | tr '\\' '/'
    done | sort -u | while read -r dll; do
        dll=$(cygpath -u "$dll")
        case "${dll,,}" in
            "${PREFIX,,}"/bin/*.dll)
                if [[ ! -f "$DIST/bin/$(basename "$dll")" ]]; then
                    cp "$dll" "$DIST/bin/"
                fi
                ;;
        esac
    done
}
copy_deps "$DIST/bin/"*.exe "$DIST/$PIXREL/loaders/"*.dll
# zweite Runde für Abhängigkeiten der eben kopierten DLLs
copy_deps "$DIST/bin/"*.dll
echo "    $(ls "$DIST/bin/"*.dll | wc -l) DLLs"

# Verzeichnis der Bildlader mit Pfaden relativ zum Programmordner. MSYS2 liefert es schon
# relativ; sonst neu erzeugen und die absoluten Pfade kürzen.
if ! grep -qE '^"[A-Za-z]:' "$PREFIX/$PIXREL/loaders.cache"; then
    cp "$PREFIX/$PIXREL/loaders.cache" "$DIST/$PIXREL/loaders.cache"
else
    GDK_PIXBUF_MODULEDIR="$(cygpath -w "$DIST/$PIXREL/loaders")" \
        "$PREFIX/bin/gdk-pixbuf-query-loaders.exe" \
        | sed -E 's#^"[^"]*(lib[/\\]+gdk-pixbuf-2\.0)#"\1#' >"$DIST/$PIXREL/loaders.cache"
fi

# Einstellungsschemas von GTK (z. B. für den Dateidialog)
mkdir -p "$DIST/share/glib-2.0/schemas"
cp "$PREFIX/share/glib-2.0/schemas/gschemas.compiled" "$DIST/share/glib-2.0/schemas/"

# Symbole
mkdir -p "$DIST/share/icons"
cp -r "$PREFIX/share/icons/Adwaita" "$DIST/share/icons/"
mkdir -p "$DIST/share/icons/hicolor"
cp "$PREFIX/share/icons/hicolor/index.theme" "$DIST/share/icons/hicolor/"
if command -v gtk4-update-icon-cache >/dev/null; then
    gtk4-update-icon-cache -q -t -f "$DIST/share/icons/Adwaita" || true
fi

# deutsche Übersetzungen von GTK und libadwaita (Dateidialog, Tastenkürzel …)
for mo in gtk40 gtk40-properties libadwaita glib20; do
    src="$PREFIX/share/locale/de/LC_MESSAGES/$mo.mo"
    if [[ -f "$src" ]]; then
        mkdir -p "$DIST/share/locale/de/LC_MESSAGES"
        cp "$src" "$DIST/share/locale/de/LC_MESSAGES/"
    fi
done

cp LICENSE "$DIST/LICENSE.txt"
cp data/mondrian.ico "$DIST/mondrian.ico"
echo "    Größe: $(du -sh "$DIST" | cut -f1)"

# ───── 4. Installer mit Inno Setup
ISCC=""
for c in "/c/Program Files (x86)/Inno Setup 6/ISCC.exe" "/c/Program Files/Inno Setup 6/ISCC.exe"; do
    if [[ -f "$c" ]]; then
        ISCC=$c
    fi
done
if [[ -z "$ISCC" ]] && command -v iscc >/dev/null; then
    ISCC=iscc
fi
if [[ -z "$ISCC" ]]; then
    echo
    echo "Inno Setup 6 nicht gefunden – es gibt nur den Programmordner:"
    echo "    $DIST/bin/mondrian.exe"
    echo "Für den Installer Inno Setup installieren (https://jrsoftware.org/isdl.php)"
    echo "und dieses Skript noch einmal starten."
    exit 0
fi
echo "==> Installer erstellen"
# MSYS2 würde /D… und /O… sonst für Pfade halten und umschreiben
MSYS2_ARG_CONV_EXCL="*" "$ISCC" /Q \
    "/DAppVersion=$VERSION" \
    "/DSourceDir=$(cygpath -w "$DIST")" \
    "/DIconFile=$(cygpath -w "$ROOT/data/mondrian.ico")" \
    "/O$(cygpath -w "$ROOT/target/windows")" \
    "$(cygpath -w "$ROOT/packaging/windows/mondrian.iss")"
echo
echo "Fertig: target/windows/Mondrian-$VERSION-Setup.exe"
