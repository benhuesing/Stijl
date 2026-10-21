# Stijl

**A vector graphics editor for GNOME** – draw shapes, paths and text, arrange them on
artboards and export to SVG, PDF, PNG or JPG. Runs on Linux, Windows and macOS.

Stijl is named after **De Stijl** (“The Style”), the Dutch art movement that Piet Mondrian
belonged to. Until version 0.9.38 the app was called “Mondrian”; settings, shortcuts and
libraries from that time are taken over automatically on first start.

Version 0.9.42 – still in testing. Feedback and bug reports are very welcome.

> **Note:** Stijl was written with the help of AI (Claude by Anthropic). Ideas, requirements
> and testing come from a human; most of the program code was written by the AI.

## Highlights

- **Drawing tools:** shapes (rectangle, ellipse, polygon, star), pen and curvature pen,
  freehand, node editing across several objects, rounded corners on any shape, eraser,
  scissors and a **Shape Builder** that turns any area enclosed by outlines into a new shape
- **Path operations:** unite, subtract front, intersect, exclude overlap, join paths,
  convert stroke to fill, blend between two shapes
- **Text:** text boxes with columns, lists, indents and word-processor-style formatting,
  text on a path, text flowing inside shapes, small caps and ligatures, variable fonts
- **Colour:** gradients, patterns, eyedropper, colour libraries with styles and reusable
  graphics
- **Effects:** shadow, reflection, frosted glass, brushed metal, gloss, 3D relief,
  watercolour, grain, pixelate, image filters – combinable and non-destructive
- **Layout:** multiple artboards, layers, smart guides, equal-spacing snapping, align and
  distribute, a “notebook” view that shows one page at a time
- **Import:** SVG from other apps, PDF and Adobe Illustrator (.ai) files
- **Export:** PNG, JPG, SVG and PDF – including print-ready **PDF/X-4 with CMYK** and bleed
- **Safety nets:** atomic saving, automatic recovery after a crash, unlimited undo
- **Languages:** English (British) and German

See the **[User Guide](docs/USER_GUIDE.md)** for a full description of every feature and
all keyboard shortcuts.

## Download

Ready-made installers for **Windows 10/11** and **macOS 15+ (Apple silicon)** are on the
[Releases page](https://github.com/benhuesing/Stijl/releases).

The apps are not yet signed by Microsoft or Apple, so you will see a warning on first start:

- **Windows:** “Windows protected your PC” → *More info* → *Run anyway*
- **macOS:** “Stijl” Not Opened → open *System Settings › Privacy & Security* and click
  *Open Anyway* at the bottom

This is only needed once.

On **Linux**, build from source (see below). Packages for Linux app stores are planned.

## Building from source (Linux)

Requires GTK ≥ 4.12, libadwaita ≥ 1.5 and a recent Rust toolchain (e.g. via
[rustup](https://rustup.rs)).

```bash
# Ubuntu 24.04+ / Debian 13
sudo apt install build-essential libgtk-4-dev libadwaita-1-dev
# Fedora 40+
sudo dnf install gcc gtk4-devel libadwaita-devel
# Arch
sudo pacman -S base-devel gtk4 libadwaita

git clone https://github.com/benhuesing/Stijl.git
cd Stijl
cargo run --release
```

### Installing for the current user

```sh
./install.sh
```

Builds Stijl and installs it for the current user without root rights: the program goes to
`~/.local/bin/stijl` and a launcher appears in the app overview. Run it again after an
update; remove Stijl with `./uninstall.sh`.

### Windows and macOS

Stijl is a GNOME app, but it also runs on Windows 10/11 (64-bit) and on macOS 15 or later
on Apple silicon Macs (M1 and newer) – with the GNOME look.

- **With GitHub Actions (recommended):** run *Actions › Release › Run workflow*. After
  about 30 minutes, `Stijl-<version>-Setup.exe` and `Stijl-<version>-macOS.dmg` can be
  downloaded from the run (under *Artifacts*).
- **Windows, locally:** install [MSYS2](https://www.msys2.org) and
  [Inno Setup 6](https://jrsoftware.org/isdl.php), open *MSYS2 UCRT64* from the Start menu,
  change to the Stijl folder (e.g. `cd /c/Users/Name/Downloads/Stijl`) and run
  `bash packaging/windows/build.sh`. Result: `target/windows/Stijl-<version>-Setup.exe` and
  a portable folder `target/windows/Stijl` (`bin\stijl.exe`).
- **macOS, locally:** install [Homebrew](https://brew.sh) and [Rust](https://rustup.rs),
  then run `bash packaging/macos/build.sh` in the Stijl folder. Result:
  `target/macos/Stijl.app` and `target/macos/Stijl-<version>-macOS.dmg`.

The Windows installer adds Stijl to the Start menu (optionally to the desktop), registers it
under *Open with* for SVG, AI and PDF (your default apps stay unchanged) and can be removed
in Windows Settings. On a Mac, drag Stijl from the disk image into *Applications*.

Differences from Linux: fonts cannot be embedded in SVG exports and the weight slider for
variable fonts is missing (both need Fontconfig). On the Mac, ⌘ replaces Ctrl.
More details: [PORTING.md](PORTING.md).

### Publishing a new version

1. Raise the version in `Cargo.toml` and add an entry to
   `data/io.github.benhuesing.Stijl.metainfo.xml` (`<releases>`)
2. Push and create a tag, e.g. `git tag v0.9.42 && git push --tags`
3. The *Release* workflow creates a **draft release** with the Windows installer and the
   macOS disk image. Check the text and click *Publish release* – only then do installed
   copies see the new version.

## Privacy

Stijl does not collect any data. It only connects to the internet in two cases:

- **Update check (Windows and macOS):** once a day Stijl asks GitHub for the number of the
  latest release. Like any website, GitHub sees your IP address. You can turn this off or
  check manually under *Preferences › View › Updates*. On Linux it is off by default and not
  available in Flatpak or Snap builds, where the package manager handles updates.
- **Reporting a problem:** *Main menu › Report a Problem…* and the crash dialog open a
  pre-filled GitHub issue in your browser. Nothing is sent until you submit it there; you can
  read and edit everything first. Crash reports contain the version, the operating system
  and where the error happened – never your drawings.

## Where things are stored

| What | Location |
|---|---|
| Libraries | `Documents/Stijl/Libraries/` (or `Bibliotheken` in German) – one SVG file per library; copy them to share or sync the folder with a cloud service |
| Settings | `~/.config/stijl/settings.ini` |
| Recently opened | `~/.config/stijl/recent` |
| Recovery files | `~/.local/state/stijl/recovery/` |
| Keyboard shortcuts | `~/.config/stijl/shortcuts.ini` |

On Windows, settings and recovery files are in `%LOCALAPPDATA%\stijl`; on macOS in
`~/.config/stijl` and `~/.local/share/stijl`.

## Contributing

Bug reports and ideas are welcome as [issues](https://github.com/benhuesing/Stijl/issues).

- **Tests:** `cargo test`. The *Check* workflow builds Stijl on Linux and runs the tests on
  every push.
- **Translations:** the English text lives in `po/en_GB.po` (editable with e.g.
  [Poedit](https://poedit.net)); `po/stijl.pot` is the template for new languages. The
  original strings in the source code are German.
- **File format:** drawings are plain SVG. Stijl’s own information (effects, text boxes,
  layers …) is stored in the `mondrian:` namespace – the old name stays so that older files
  remain readable.

### Code overview

| File | Contents |
|---|---|
| `geom.rs` | points, rectangles, matrices, Bézier helpers |
| `model.rs` | document, artboards, layers, objects, shapes, styles |
| `editor.rs` | tools, selection, undo (no GTK, tested) |
| `boolean.rs` | path operations (tested) |
| `shapeops.rs` | Shape Builder, eraser, scissors (tested) |
| `library.rs` | libraries (tested) |
| `fsutil.rs` | atomic saving, recovery files (tested) |
| `icc.rs`, `pdfx.rs` | colour profiles (sRGB → CMYK) and PDF conversion to CMYK/PDF/X-4 with its own Deflate (tested) |
| `keys.rs` | keyboard shortcuts, custom key bindings, conflict checks (tested) |
| `xml.rs`, `svg.rs` | reading and writing SVG (tested) |
| `imgfx.rs`, `trace.rs` | image filters, the maths behind 3D effects and textures, image tracing (tested) |
| `pdf.rs`, `ai.rs` | PDF reader (Inflate, functions, colour spaces) and Illustrator import (tested) |
| `render.rs` | drawing with Cairo/Pango, effects |
| `export.rs` | PNG, JPG, PDF, SVG |
| `window.rs` | user interface (libadwaita) |
| `i18n.rs`, `po/` | translations (tested) |
| `icons.rs` | custom icons |
| `update.rs` | update check (tested) |
| `packaging/`, `.github/` | Windows installer, macOS bundle, automated builds |

## Licence

Copyright © 2026 Bendix Hüsing

Stijl is free software: you can redistribute it and/or modify it under the terms of the
GNU General Public License, version 3 or (at your option) any later version – see
[LICENSE](LICENSE). It is distributed without any warranty, to the extent permitted by law.

The Windows and macOS builds contain libraries from other projects (including GTK,
libadwaita, Cairo, Pango and GLib) under their own licences; the list is in
`THIRD-PARTY.txt` inside the program folder.
