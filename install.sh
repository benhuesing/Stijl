#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Bendix Hüsing
#
# Install Stijl for the current user (no root rights needed).
#   Program:   ~/.local/bin/stijl
#   Launcher:  ~/.local/share/applications/io.github.benhuesing.Stijl.desktop
#   Icon:      ~/.local/share/icons/hicolor/scalable/apps/io.github.benhuesing.Stijl.svg
# An earlier installation under the old name “Mondrian” is removed; Stijl takes over its
# settings and libraries on first start.
set -e
cd "$(dirname "$0")"

echo "Building Stijl …"
cargo build --release

# Refresh the icon folder. An outdated icon-theme.cache would hide new icons; if it cannot
# be rewritten it is deleted, and the system then reads the folders directly.
refresh_icons() {
    HICOLOR="$HOME/.local/share/icons/hicolor"
    UPD=""
    for t in gtk4-update-icon-cache gtk-update-icon-cache; do
        if command -v "$t" >/dev/null 2>&1; then
            UPD="$t"
            break
        fi
    done
    if [ -z "$UPD" ] || ! "$UPD" -q -f -t "$HICOLOR" 2>/dev/null; then
        rm -f "$HICOLOR/icon-theme.cache"
    fi
    touch "$HICOLOR"
}

BIN="$HOME/.local/bin/stijl"
APPS="$HOME/.local/share/applications"
ICONS="$HOME/.local/share/icons/hicolor/scalable/apps"
ID="io.github.benhuesing.Stijl"

install -Dm755 target/release/stijl "$BIN"
mkdir -p "$APPS"

ICON="applications-graphics"
if [ -f "data/$ID.svg" ]; then
    install -Dm644 "data/$ID.svg" "$ICONS/$ID.svg"
    ICON="$ID"
fi

# launcher with the full path (works even if ~/.local/bin is not in PATH)
sed -e "s|^Exec=.*|Exec=$BIN %F|" -e "s|^Icon=.*|Icon=$ICON|" "data/$ID.desktop" > "$APPS/$ID.desktop"

# description for software centres
install -Dm644 "data/$ID.metainfo.xml" "$HOME/.local/share/metainfo/$ID.metainfo.xml"

# remove the old installation under the former name
OLD="io.github.mondrian.Mondrian"
rm -f "$HOME/.local/bin/mondrian" "$APPS/$OLD.desktop" "$ICONS/$OLD.svg"

update-desktop-database "$APPS" 2>/dev/null || true
refresh_icons


echo "Done. Stijl is now in the app overview."
