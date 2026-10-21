#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Bendix Hüsing
#
# Build Stijl for Windows and create an installer.
#
# Run in the MSYS2 “UCRT64” shell (https://www.msys2.org) inside the Stijl folder:
#
#     bash packaging/windows/build.sh
#
# Result:
#   target/windows/Stijl/                        program folder (also runs without installing)
#   target/windows/Stijl-<version>-Setup.exe     installer (if Inno Setup 6 is installed)
#
# SKIP_DEPS=1 skips installing the MSYS2 packages (e.g. in GitHub Actions).
set -euo pipefail

cd "$(dirname "$0")/../.."
ROOT=$PWD

if [[ "${MSYSTEM:-}" != "UCRT64" ]]; then
    echo "Please run this in the MSYS2 “UCRT64” shell (Start menu: MSYS2 UCRT64)." >&2
    exit 1
fi
PREFIX=${MINGW_PREFIX:-/ucrt64}
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
echo "==> Stijl $VERSION for Windows"

# ───── 1. Tools and libraries
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
    echo "==> Installing packages (takes a while the first time)"
    pacman -S --needed --noconfirm "${PKGS[@]}"
fi

# ───── 2. Compile
echo "==> Compiling (cargo build --release)"
cargo build --release

# ───── 3. Assemble the program folder
DIST=$ROOT/target/windows/Stijl
echo "==> Program folder $DIST"
rm -rf "$DIST"
mkdir -p "$DIST/bin" "$DIST/lib" "$DIST/share"

cp target/release/stijl.exe "$DIST/bin/"
# GLib helpers (spawning processes, D-Bus for opening files in the running window)
for f in gdbus.exe gspawn-win64-helper.exe gspawn-win64-helper-console.exe; do
    if [[ -f "$PREFIX/bin/$f" ]]; then
        cp "$PREFIX/bin/$f" "$DIST/bin/"
    fi
done

# gdk-pixbuf image loaders (including SVG for the icons)
PIXREL=lib/gdk-pixbuf-2.0/2.10.0
mkdir -p "$DIST/$PIXREL/loaders"
cp "$PREFIX/$PIXREL/loaders/"*.dll "$DIST/$PIXREL/loaders/"

# collect all required DLLs from MSYS2 (including those of the image loaders)
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
# second round for dependencies of the DLLs just copied
copy_deps "$DIST/bin/"*.dll
echo "    $(ls "$DIST/bin/"*.dll | wc -l) DLLs"

# Image loader list with paths relative to the program folder. MSYS2 already ships it with
# relative paths; otherwise regenerate it and shorten the absolute paths.
if ! grep -qE '^"[A-Za-z]:' "$PREFIX/$PIXREL/loaders.cache"; then
    cp "$PREFIX/$PIXREL/loaders.cache" "$DIST/$PIXREL/loaders.cache"
else
    GDK_PIXBUF_MODULEDIR="$(cygpath -w "$DIST/$PIXREL/loaders")" \
        "$PREFIX/bin/gdk-pixbuf-query-loaders.exe" \
        | sed -E 's#^"[^"]*(lib[/\\]+gdk-pixbuf-2\.0)#"\1#' >"$DIST/$PIXREL/loaders.cache"
fi

# GTK settings schemas (e.g. for the file dialog)
mkdir -p "$DIST/share/glib-2.0/schemas"
cp "$PREFIX/share/glib-2.0/schemas/gschemas.compiled" "$DIST/share/glib-2.0/schemas/"

# Icons
mkdir -p "$DIST/share/icons"
cp -r "$PREFIX/share/icons/Adwaita" "$DIST/share/icons/"
mkdir -p "$DIST/share/icons/hicolor"
cp "$PREFIX/share/icons/hicolor/index.theme" "$DIST/share/icons/hicolor/"
if command -v gtk4-update-icon-cache >/dev/null; then
    gtk4-update-icon-cache -q -t -f "$DIST/share/icons/Adwaita" || true
fi

# Translations of GTK and libadwaita (file dialog, shortcuts …): German and British
# English – Stijl’s own texts are built into the program
for lang in de en_GB; do
    for mo in gtk40 gtk40-properties libadwaita glib20; do
        src="$PREFIX/share/locale/$lang/LC_MESSAGES/$mo.mo"
        if [[ -f "$src" ]]; then
            mkdir -p "$DIST/share/locale/$lang/LC_MESSAGES"
            cp "$src" "$DIST/share/locale/$lang/LC_MESSAGES/"
        fi
    done
done

cp LICENSE "$DIST/LICENSE.txt"

# Licences of the bundled libraries (GTK, libadwaita, Cairo, Pango … and Rust crates)
echo "==> THIRD-PARTY.txt"
{
    echo "Stijl $VERSION"
    echo "Copyright © 2026 Bendix Hüsing"
    echo "Licence: GNU General Public License, version 3 or later (LICENSE.txt)"
    echo
    echo "This Windows build contains libraries from other projects under their own licences"
    echo "(mostly LGPL, MIT, BSD, Zlib). They are taken unmodified from MSYS2; source code and"
    echo "build recipes:"
    echo "  https://packages.msys2.org"
    echo "  https://github.com/msys2/MINGW-packages"
    echo
    echo "─── MSYS2 packages"
    for f in "$DIST/bin/"*.dll "$DIST/bin/"*.exe "$DIST/$PIXREL/loaders/"*.dll; do
        [[ "$(basename "$f")" == stijl.exe ]] && continue
        src="$PREFIX/${f#"$DIST/"}"
        pacman -Qqo "$src" 2>/dev/null || true
    done | sort -u | while read -r pkg; do
        info=$(LC_ALL=C pacman -Qi "$pkg")
        ver=$(sed -n 's/^Version *: //p' <<<"$info")
        lic=$(sed -n 's/^Licenses *: //p' <<<"$info")
        url=$(sed -n 's/^URL *: //p' <<<"$info")
        printf '%s %s\n    Licence: %s\n    %s\n' "$pkg" "$ver" "$lic" "$url"
    done
    for t in Adwaita hicolor; do
        pkg=$(pacman -Qqo "$PREFIX/share/icons/$t/index.theme" 2>/dev/null || true)
        if [[ -n "$pkg" ]]; then
            info=$(LC_ALL=C pacman -Qi "$pkg")
            printf '%s (icons)\n    Licence: %s\n' "$pkg" "$(sed -n 's/^Licenses *: //p' <<<"$info")"
        fi
    done
    echo
    echo "─── Rust crates"
    cargo tree -e normal --prefix none --format "{p}  –  {l}" 2>/dev/null \
        | sed 's/ (\*)$//' | grep -v '^stijl ' | sort -u || true
} >"$DIST/THIRD-PARTY.txt"
cp data/stijl.ico "$DIST/stijl.ico"
echo "    Size: $(du -sh "$DIST" | cut -f1)"

# ───── 4. Installer with Inno Setup
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
    echo "Inno Setup 6 not found – only the program folder was created:"
    echo "    $DIST/bin/stijl.exe"
    echo "To build the installer, install Inno Setup (https://jrsoftware.org/isdl.php)"
    echo "and run this script again."
    exit 0
fi
echo "==> Creating the installer"
# otherwise MSYS2 would treat /D… and /O… as paths and rewrite them
MSYS2_ARG_CONV_EXCL="*" "$ISCC" /Q \
    "/DAppVersion=$VERSION" \
    "/DSourceDir=$(cygpath -w "$DIST")" \
    "/DIconFile=$(cygpath -w "$ROOT/data/stijl.ico")" \
    "/O$(cygpath -w "$ROOT/target/windows")" \
    "$(cygpath -w "$ROOT/packaging/windows/stijl.iss")"
echo
echo "Done: target/windows/Stijl-$VERSION-Setup.exe"
