#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Bendix Hüsing
#
# Remove Stijl again. Your drawings and libraries (Documents/Stijl) and the settings
# (~/.config/stijl) are kept.
ID="io.github.benhuesing.Stijl"
rm -f "$HOME/.local/bin/stijl" \
      "$HOME/.local/share/applications/$ID.desktop" \
      "$HOME/.local/share/icons/hicolor/scalable/apps/$ID.svg" \
      "$HOME/.local/share/metainfo/$ID.metainfo.xml"
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
HICOLOR="$HOME/.local/share/icons/hicolor"
gtk4-update-icon-cache -q -f -t "$HICOLOR" 2>/dev/null ||
    gtk-update-icon-cache -q -f -t "$HICOLOR" 2>/dev/null ||
    rm -f "$HICOLOR/icon-theme.cache"
echo "Stijl has been removed."
