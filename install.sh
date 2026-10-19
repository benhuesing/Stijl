#!/bin/sh
# Mondrian für den aktuellen Benutzer installieren (ohne Root-Rechte).
#   Programm:     ~/.local/bin/mondrian
#   Starter:      ~/.local/share/applications/io.github.mondrian.Mondrian.desktop
#   Symbol:       ~/.local/share/icons/hicolor/scalable/apps/io.github.mondrian.Mondrian.svg
#                 (nur wenn data/io.github.mondrian.Mondrian.svg vorhanden ist)
set -e
cd "$(dirname "$0")"

echo "Mondrian wird gebaut …"
cargo build --release

BIN="$HOME/.local/bin/mondrian"
APPS="$HOME/.local/share/applications"
ICONS="$HOME/.local/share/icons/hicolor/scalable/apps"
ID="io.github.mondrian.Mondrian"

install -Dm755 target/release/mondrian "$BIN"
mkdir -p "$APPS"

ICON="applications-graphics"
if [ -f "data/$ID.svg" ]; then
    install -Dm644 "data/$ID.svg" "$ICONS/$ID.svg"
    ICON="$ID"
fi

# Starter mit vollem Pfad (funktioniert auch, wenn ~/.local/bin nicht im PATH steht)
sed -e "s|^Exec=.*|Exec=$BIN %F|" -e "s|^Icon=.*|Icon=$ICON|" "data/$ID.desktop" > "$APPS/$ID.desktop"

update-desktop-database "$APPS" 2>/dev/null || true
gtk-update-icon-cache -q "$HOME/.local/share/icons/hicolor" 2>/dev/null || true

echo "Fertig. Mondrian steht jetzt in der App-Übersicht."
