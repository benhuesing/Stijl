# Stijl on Windows and macOS – notes for developers

Stijl is and remains a GNOME app. On Windows and macOS it is fine for it to look like GNOME
(header bar, Adwaita style, window buttons at the top right).

## Status

**Windows (since 0.9.35):** build and installer via MSYS2 and Inno Setup
(`packaging/windows/`, GitHub Actions: `.github/workflows/release.yml`; instructions in the
README). Already adapted:
- no console window (`windows_subsystem`), app icon in the .exe (`build.rs`, `data/stijl.ico`)
- no calls to `fc-list`/`fc-match` (the weight slider for variable fonts and font embedding
  in SVG are not available there)
- recovery files in `%LOCALAPPDATA%\stijl\recovery`
- colour profiles also from `C:\Windows\System32\spool\drivers\color` (and the macOS folders)
- default font Segoe UI (macOS: Helvetica Neue)
- *Open with* for SVG/AI/PDF via the installer; `gdbus.exe` is bundled so that files open in
  the running window where possible
- the update check uses the system’s `curl.exe` (`update.rs`)

Still open for Windows: font embedding and variable fonts without Fontconfig (e.g. via
Pango ≥ 1.44 `FontFamily::is_variable()`), code signing (otherwise SmartScreen warns).

**macOS (since 0.9.40):** app bundle and disk image via Homebrew (`packaging/macos/`,
GitHub Actions: `.github/workflows/release.yml`). Already adapted:
- Apple silicon, from the macOS version of the build machine (runner `macos-15` → macOS 15+)
- libraries bundled with `dylibbundler`, ad-hoc signed; runtime paths for icons, schemas and
  image loaders are set in `main.rs` (`mac_bundle_env`)
- keyboard shortcuts: `<Control>` in the key bindings becomes `<Primary>` (⌘), labels are
  shown as ⇧⌘S
- application menu: actions `app.about`, `app.preferences`, `app.quit`
- no calls to `fc-list`/`fc-match`
- *Open with* for SVG, AI and PDF (`Info.plist`)

Still open for macOS: signing with a Developer ID and notarisation (99 USD/year from Apple,
otherwise a warning on first start), Intel Macs, translations of GTK/libadwaita inside the
bundle (their own texts stay English there), testing on real hardware.

## What already works everywhere

- **GTK 4, libadwaita, Cairo and Pango** run on both systems (Windows via MSYS2 or
  gvsbuild, macOS via Homebrew). The Rust code uses the same crates.
- **Folders for settings, libraries and recent files** come from `glib::user_config_dir()` /
  `glib::user_data_dir()`. On Windows that is `%LOCALAPPDATA%`; on macOS GLib follows the
  XDG defaults (`~/.config`, `~/.local/share`).
- **Atomic saving** (`fsutil::write_atomic`) only uses `std::fs`. `rename` replaces an
  existing file on Windows too, but fails while another program holds the file open – then
  an error message appears and the old file stays intact.
- **SVG, PDF/X, ICC and effects** are pure Rust or Cairo code. Nothing to do here.

## Still to do

| Where | Problem | Solution |
|---|---|---|
| `window.rs`: `font_is_variable` | calls `fc-list`; without Fontconfig the weight slider is missing | Pango ≥ 1.44 has `FontFamily::is_variable()` (crate feature `v1_44`), which is better on all systems |
| `export.rs`: `font_file` | `fc-match` finds the font file for embedding in SVG | ask the Pango font map, or use a Rust library such as `font-kit` |
| Single instance (files open as tabs in the running window) | `GApplication` detects a running instance via D-Bus, which Windows/macOS lack | Windows: GLib ≥ 2.74 partly handles this, otherwise a named pipe. macOS: files arrive as `open` events to the same instance (check the GTK macOS backend) |
| Dark style | on Linux libadwaita follows the system via the portal | test on Windows/macOS; the Light/Dark preference exists as a fallback |
| Open folder (Library ⋮ › Open folder) | `gtk::FileLauncher` | should work – test |

## Guidelines for further development

- No new calls to external Linux programs (`fc-*`, `xdg-*`). Prefer GLib/GIO
  (`gio::AppInfo`, `gtk::FileLauncher`, `glib::user_*_dir`) or pure Rust.
- Always build paths with `Path::join`, never by concatenating `/`.
- Linux folders (`/usr/share/…`) only as additional search locations, never as the only one.
- Write new shortcuts with `<Control>`; the Mac mapping happens in one place (`keys.rs`).
