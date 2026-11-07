// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! Eigene symbolische Symbole (in Stijl gezeichnet). Sie werden beim Start in den Cache
//! geschrieben und als Suchpfad im Symbolthema registriert, damit GTK sie wie
//! Systemsymbole einfärbt (hell/dunkel).

use gtk::{gdk, glib};

/// Eigene Symbole (16 × 16, nur Füllungen in einer Farbe) aus `data/icons/`. Ein Symbol
/// lässt sich ersetzen, indem man die Datei dort überschreibt und neu baut.
macro_rules! icon {
    ($name:literal) => {
        ($name, include_str!(concat!("../data/icons/", $name, ".svg")))
    };
}

const ICONS: &[(&str, &str)] = &[
    icon!("mondrian-align-artboard-symbolic"),
    icon!("mondrian-blend-symbolic"),
    icon!("mondrian-board-symbolic"),
    icon!("mondrian-builder-symbolic"),
    icon!("mondrian-cap-butt-symbolic"),
    icon!("mondrian-cap-round-symbolic"),
    icon!("mondrian-cap-square-symbolic"),
    icon!("mondrian-cell-merge-symbolic"),
    icon!("mondrian-cell-split-symbolic"),
    icon!("mondrian-chart-symbolic"),
    icon!("mondrian-clip-symbolic"),
    icon!("mondrian-columns-symbolic"),
    icon!("mondrian-curvature-symbolic"),
    icon!("mondrian-distribute-symbolic"),
    icon!("mondrian-distribute-vertical-symbolic"),
    icon!("mondrian-ellipse-symbolic"),
    icon!("mondrian-eraser-symbolic"),
    icon!("mondrian-exclude-symbolic"),
    icon!("mondrian-first-line-symbolic"),
    icon!("mondrian-formatting-marks-symbolic"),
    icon!("mondrian-fx-symbolic"),
    icon!("mondrian-group-symbolic"),
    icon!("mondrian-guide-symbolic"),
    icon!("mondrian-hand-symbolic"),
    icon!("mondrian-highlighter-symbolic"),
    icon!("mondrian-intersect-symbolic"),
    icon!("mondrian-join-bevel-symbolic"),
    icon!("mondrian-join-miter-symbolic"),
    icon!("mondrian-join-round-symbolic"),
    icon!("mondrian-laser-symbolic"),
    icon!("mondrian-layers-symbolic"),
    icon!("mondrian-left-turn-symbolic"),
    icon!("mondrian-letter-spacing-symbolic"),
    icon!("mondrian-library-symbolic"),
    icon!("mondrian-ligature-symbolic"),
    icon!("mondrian-line-spacing-symbolic"),
    icon!("mondrian-margin-bottom-symbolic"),
    icon!("mondrian-margin-left-symbolic"),
    icon!("mondrian-margin-right-symbolic"),
    icon!("mondrian-margin-top-symbolic"),
    icon!("mondrian-mirror-horizontal-symbolic"),
    icon!("mondrian-mirror-vertical-symbolic"),
    icon!("mondrian-node-symbolic"),
    icon!("mondrian-notebook-symbolic"),
    icon!("mondrian-page-bottom-symbolic"),
    icon!("mondrian-page-break-symbolic"),
    icon!("mondrian-page-hcenter-symbolic"),
    icon!("mondrian-page-left-symbolic"),
    icon!("mondrian-page-right-symbolic"),
    icon!("mondrian-page-top-symbolic"),
    icon!("mondrian-page-vcenter-symbolic"),
    icon!("mondrian-pages-symbolic"),
    icon!("mondrian-path-below-symbolic"),
    icon!("mondrian-path-reverse-symbolic"),
    icon!("mondrian-pen-symbolic"),
    icon!("mondrian-pencil-symbolic"),
    icon!("mondrian-polygon-symbolic"),
    icon!("mondrian-presentation-symbolic"),
    icon!("mondrian-props-symbolic"),
    icon!("mondrian-rect-symbolic"),
    icon!("mondrian-right-turn-symbolic"),
    icon!("mondrian-select-symbolic"),
    icon!("mondrian-smallcaps-symbolic"),
    icon!("mondrian-space-after-symbolic"),
    icon!("mondrian-space-before-symbolic"),
    icon!("mondrian-star-symbolic"),
    icon!("mondrian-subtract-symbolic"),
    icon!("mondrian-table-add-row-symbolic"),
    icon!("mondrian-table-symbolic"),
    icon!("mondrian-text-size-symbolic"),
    icon!("mondrian-text-symbolic"),
    icon!("mondrian-text-weight-symbolic"),
    icon!("mondrian-textarea-symbolic"),
    icon!("mondrian-textpath-symbolic"),
    icon!("mondrian-ungroup-symbolic"),
    icon!("mondrian-union-symbolic"),
    icon!("mondrian-valign-bottom-symbolic"),
    icon!("mondrian-valign-middle-symbolic"),
    icon!("mondrian-valign-top-symbolic"),
];

pub fn install() {
    let dir = glib::user_cache_dir().join("stijl").join("icons");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    for (name, content) in ICONS {
        let path = dir.join(format!("{name}.svg"));
        if std::fs::read_to_string(&path).ok().as_deref() != Some(*content) {
            let _ = std::fs::write(&path, content);
        }
    }
    // farbiges App-Symbol (Startseite, Info-Dialog, Fenster)
    let app = dir.join("io.github.benhuesing.Stijl.svg");
    let content = include_str!("../data/io.github.benhuesing.Stijl.svg");
    if std::fs::read_to_string(&app).ok().as_deref() != Some(content) {
        let _ = std::fs::write(&app, content);
    }
    if let Some(display) = gdk::Display::default() {
        gtk::IconTheme::for_display(&display).add_search_path(&dir);
        gtk::Window::set_default_icon_name("io.github.benhuesing.Stijl");
    }
}

#[cfg(test)]
mod tests {
    use super::ICONS;

    /// Jedes Symbol, das die Oberfläche nennt, gibt es auch, und jedes ist ein sauberes
    /// symbolisches Symbol (16 × 16, nur Füllungen, keine Konturen, Bilder oder Texte).
    #[test]
    fn icons_exist_and_are_symbolic() {
        let ui = include_str!("window.rs");
        let mut rest = ui;
        while let Some(i) = rest.find("\"mondrian-") {
            let s = &rest[i + 1..];
            let end = s.find('"').unwrap();
            let name = &s[..end];
            if name.ends_with("-symbolic") {
                assert!(ICONS.iter().any(|(n, _)| *n == name), "missing icon {name}");
            }
            rest = &s[end..];
        }
        for (name, svg) in ICONS {
            assert!(svg.contains("viewBox=\"0 0 16 16\""), "{name}: not 16 × 16");
            for bad in ["stroke=\"#", "<image", "<text", "gradient", "filter", "opacity"] {
                assert!(!svg.contains(bad), "{name}: contains {bad}");
            }
        }
    }
}
