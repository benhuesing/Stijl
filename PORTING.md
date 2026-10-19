# Mondrian auf Windows und macOS – Merkzettel

Mondrian ist und bleibt ein GNOME-Programm. Auf Windows und macOS darf es nach GNOME
aussehen (Kopfleiste, Adwaita-Stil, Knöpfe rechts oben).

## Stand

**Windows (seit 0.9.35):** Bau und Installer über MSYS2 und Inno Setup
(`packaging/windows/`, GitHub Actions: `.github/workflows/windows.yml`; Anleitung im README).
Schon angepasst:
- kein Konsolenfenster (`windows_subsystem`), Programmsymbol in der .exe (`build.rs`, `data/mondrian.ico`)
- keine Aufrufe von `fc-list`/`fc-match` unter Windows (Strichstärke-Regler und Schrifteinbettung in SVG entfallen dort)
- Wiederherstellungsdateien unter `%LOCALAPPDATA%\mondrian\recovery`
- Farbprofile auch aus `C:\Windows\System32\spool\drivers\color` (und den macOS-Ordnern)
- Grundschrift Segoe UI (macOS: Helvetica Neue)
- „Öffnen mit“ für SVG/AI/PDF über den Installer; `gdbus.exe` liegt bei, damit Dateien möglichst im offenen Fenster landen

Noch offen für Windows: Schrifteinbettung und variable Schriften ohne Fontconfig (z. B. über
Pango ≥ 1.44 `FontFamily::is_variable()`), Signatur des Installers (sonst warnt SmartScreen).

**macOS:** noch nicht umgesetzt; die Tabelle unten gilt weiter.

## Was schon passt

- **GTK 4, libadwaita, Cairo und Pango** laufen auf beiden Systemen (Windows über MSYS2
  oder gvsbuild, macOS über Homebrew). Der Rust-Code nutzt dieselben Crates.
- **Ordner für Einstellungen, Bibliotheken und zuletzt geöffnete Dateien** kommen über
  `glib::user_config_dir()` / `glib::user_data_dir()`. Das liefert unter Windows `%LOCALAPPDATA%`
  und unter macOS `~/Library/Application Support`.
- **Sicheres Speichern** (`fsutil::write_atomic`) nutzt nur `std::fs`. `rename` ersetzt
  auch unter Windows eine vorhandene Datei. Es scheitert aber, wenn ein anderes Programm
  die Datei gerade offen hält. Dann kommt eine Fehlermeldung, und die alte Datei bleibt
  heil.
- **SVG, PDF/X, ICC und Effekte** sind reiner Rust- bzw. Cairo-Code. Hier ist nichts zu
  tun.

## Was angepasst werden muss

| Stelle | Problem | Lösung |
|---|---|---|
| `window.rs`: `font_is_variable` | ruft `fc-list` auf; unter Windows und macOS gibt es Fontconfig meist nicht, dann fehlt nur der Strichstärke-Regler | Pango ≥ 1.44 kennt `FontFamily::is_variable()` (Crate-Feature `v1_44`). Das ist besser für alle Systeme |
| `export.rs`: `font_file` | `fc-match` sucht die Schriftdatei zum Einbetten in SVG | Pango-Fontmap fragen (`pangocairo`/`pango_fc_font` gibt es nur mit Fontconfig) oder eine Rust-Bibliothek wie `font-kit` nutzen |
| `std::process::Command` (fc-list, fc-match) | unter Windows blitzt bei GUI-Programmen ein Konsolenfenster auf | `CREATE_NO_WINDOW` setzen (`std::os::windows::process::CommandExt`) oder die Aufrufe ganz ersetzen (siehe oben) |
| Wiederherstellungsordner (`window.rs`, `$XDG_STATE_HOME` bzw. `~/.local/state`) | unter Windows und macOS unüblich | dort `glib::user_data_dir()/mondrian/recovery` nehmen |
| `export.rs`: `find_profiles` | sucht ICC-Profile nur in Linux-Ordnern | Windows: `C:\Windows\System32\spool\drivers\color`; macOS: `/Library/ColorSync/Profiles`, `~/Library/ColorSync/Profiles` |
| Grundschrift „Cantarell“ (`editor.rs`, `text_defaults`) | fehlt außerhalb von GNOME, Pango nimmt dann eine Ersatzschrift | je System eine Vorgabe (Windows „Segoe UI“, macOS „Helvetica Neue“) oder Cantarell mitliefern |
| Tastenkürzel (`keys.rs`) | `<Primary>` wird zu `<Control>` normalisiert; auf dem Mac erwartet man ⌘ | auf macOS `<Primary>` als `<Meta>` behandeln und die Beschriftung „Strg“ durch „⌘“ ersetzen. Eigene Kürzel in `shortcuts.ini` bleiben gültig |
| Nur eine Instanz (Dateien als Tab im offenen Fenster) | `GApplication` erkennt eine laufende Instanz über D-Bus, das gibt es unter Windows/macOS nicht | Windows: GLib ≥ 2.74 kann das teilweise selbst, sonst eine benannte Pipe. macOS: Dateien kommen über `open`-Ereignisse an dieselbe Instanz (GTK-macOS-Backend prüfen) |
| `install.sh` / `.desktop`-Datei | nur Linux | Windows: Installer (z. B. Inno Setup, MSIX) mit Dateizuordnung `.svg`/`.ai`. macOS: App-Bundle mit `Info.plist` (Dokumenttypen), ggf. `gtk-mac-bundler` |
| Symbole | eigene Symbole werden zur Laufzeit erzeugt (`icons.rs`), die übrigen kommen aus dem Adwaita-Symbolthema | das Adwaita-Icon-Theme (und `hicolor`) mitliefern |
| Dunkles Design | libadwaita folgt unter Linux dem System über das Portal | unter Windows/macOS testen. Notfalls die Einstellung „Hell/Dunkel“ nutzen, die es schon gibt |
| Ordner öffnen (Bibliothek ⋮ › Ordner öffnen) | `gtk::FileLauncher` | sollte funktionieren, testen |
| Drucken | Mondrian druckt nicht selbst (Export als PDF) | nichts nötig |

## Bauen und Verteilen (Stichworte)

- **Windows:** MSYS2 (UCRT64) mit `mingw-w64-ucrt-x86_64-gtk4`, `…-libadwaita`, `…-rust`.
  Zum Verteilen alle DLLs, `share/glib-2.0/schemas`, Icons und `gdk-pixbuf`-Loader
  einsammeln (z. B. mit einem `ldd`-Skript). Ohne Signatur warnt SmartScreen.
- **macOS:** Homebrew `gtk4 libadwaita`. Für ein verteilbares Bundle die dylibs mit
  `dylibbundler` einbetten. Ohne Apple-Entwicklerkonto (Signatur und Notarisierung)
  muss man die App beim ersten Start per Rechtsklick › Öffnen freigeben.
- **CI:** GitHub Actions hat Windows- und macOS-Runner. Ein Job je System, der
  `cargo test` und den Bau ausführt, deckt die meisten Probleme früh auf.

## Beim Weiterentwickeln beachten

- Keine neuen Aufrufe externer Linux-Programme (`fc-*`, `xdg-*`). Lieber GLib/GIO
  (`gio::AppInfo`, `gtk::FileLauncher`, `glib::user_*_dir`) oder reines Rust nutzen.
- Pfade immer mit `Path::join` bauen, nie mit `/` zusammensetzen.
- Linux-Ordner (`/usr/share/…`) nur als zusätzliche Suchorte, nie als einzige.
- Neue Tastenkürzel mit `<Control>` schreiben. Die Mac-Umsetzung passiert dann an einer
  Stelle (`keys.rs`).
