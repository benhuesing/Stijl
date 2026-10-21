#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Bendix Hüsing
#
# Stijl für den aktuellen Benutzer installieren (ohne Root-Rechte).
#   Programm:     ~/.local/bin/stijl
#   Starter:      ~/.local/share/applications/io.github.benhuesing.Stijl.desktop
#   Symbol:       ~/.local/share/icons/hicolor/scalable/apps/io.github.benhuesing.Stijl.svg
# Eine frühere Installation unter dem alten Namen „Mondrian“ wird dabei entfernt; Einstellungen
# und Bibliotheken übernimmt Stijl beim ersten Start.
set -e
cd "$(dirname "$0")"

echo "Stijl wird gebaut …"
cargo build --release

# Symbolverzeichnis auffrischen. Ein veralteter Zwischenspeicher (icon-theme.cache) würde
# neue Symbole verstecken; lässt er sich nicht neu schreiben, wird er gelöscht – dann liest
# das System die Ordner direkt.
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

# Starter mit vollem Pfad (funktioniert auch, wenn ~/.local/bin nicht im PATH steht)
sed -e "s|^Exec=.*|Exec=$BIN %F|" -e "s|^Icon=.*|Icon=$ICON|" "data/$ID.desktop" > "$APPS/$ID.desktop"

# Beschreibung für Software-Zentren
install -Dm644 "data/$ID.metainfo.xml" "$HOME/.local/share/metainfo/$ID.metainfo.xml"

# alte Installation unter dem früheren Namen entfernen
OLD="io.github.mondrian.Mondrian"
rm -f "$HOME/.local/bin/mondrian" "$APPS/$OLD.desktop" "$ICONS/$OLD.svg"

update-desktop-database "$APPS" 2>/dev/null || true
refresh_icons


echo "Fertig. Stijl steht jetzt in der App-Übersicht."
