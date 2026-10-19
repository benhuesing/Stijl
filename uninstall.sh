#!/bin/sh
# Mondrian wieder entfernen. Eigene Zeichnungen und Bibliotheken
# (Dokumente/Mondrian) und die Einstellungen (~/.config/mondrian) bleiben erhalten.
ID="io.github.mondrian.Mondrian"
rm -f "$HOME/.local/bin/mondrian" \
      "$HOME/.local/share/applications/$ID.desktop" \
      "$HOME/.local/share/icons/hicolor/scalable/apps/$ID.svg"
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
echo "Mondrian wurde entfernt."
