// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! Stijl – ein Vektorzeichenprogramm für GNOME (GTK 4 + libadwaita).
// Windows: als Fensterprogramm starten (ohne zusätzliches Konsolenfenster)
#![cfg_attr(windows, windows_subsystem = "windows")]

mod ai;
mod boolean;
mod editor;
mod export;
mod fsutil;
mod geom;
mod i18n;
mod icc;
mod icons;
mod imgfx;
mod keys;
mod library;
mod model;
mod pdf;
mod pdfx;
mod render;
mod shapeops;
mod stroker;
mod svg;
mod trace;
mod update;
mod window;
mod xml;

use adw::prelude::*;
use gtk::{gio, glib};

const APP_ID: &str = "io.github.benhuesing.Stijl";

fn main() -> glib::ExitCode {
    // macOS-App-Bundle: GTK die mitgelieferten Daten zeigen (vor jedem GTK-Aufruf)
    #[cfg(target_os = "macos")]
    mac_bundle_env();
    // Einstellungen, Bibliotheken usw. aus der Zeit, als das Programm „Mondrian“ hieß
    window::migrate_from_mondrian();
    // Sprache: Einstellung (System/Deutsch/Englisch) oder die des Systems
    let setting = window::language_setting();
    let system: Vec<String> = glib::language_names().iter().map(|s| s.to_string()).collect();
    let lang = i18n::choose(&setting, &system);
    // Weicht die gewählte Sprache vom System ab, auch GTKs eigene Texte (Dateidialog …)
    // umstellen – das muss vor dem Start von GTK geschehen.
    if lang != i18n::choose("system", &system) {
        std::env::set_var(
            "LANGUAGE",
            match lang {
                i18n::Lang::De => "de_DE:de",
                i18n::Lang::EnGb => "en_GB:en",
            },
        );
    }
    i18n::init(lang);

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    app.connect_startup(|app| {
        icons::install();
        window::set_accels(app);
        app_actions(app);
    });

    // Erneuter Start (z. B. über das Dock): vorhandenes Fenster zeigen
    app.connect_activate(|app| match window::existing_ui() {
        Some(ui) => ui.present(),
        None => window::Ui::new(app).present(),
    });

    // Dateien über „Öffnen mit“, den Desktop oder die Kommandozeile: als Tabs im vorhandenen
    // Fenster (sonst in einem neuen)
    app.connect_open(|app, files, _| {
        let ui = window::existing_ui().unwrap_or_else(|| window::Ui::new(app));
        for file in files {
            if let Some(path) = file.path() {
                window::open_path(&ui, &path);
            }
        }
        ui.present();
    });

    app.run()
}

/// Programmaktionen, die macOS im Programmmenü erwartet (Über, Einstellungen, Beenden).
/// Beenden schließt die Fenster einzeln, damit nach ungespeicherten Änderungen gefragt wird.
fn app_actions(app: &adw::Application) {
    let weak = app.downgrade();
    let quit = gio::SimpleAction::new("quit", None);
    quit.connect_activate(move |_, _| {
        if let Some(app) = weak.upgrade() {
            for w in app.windows() {
                w.close();
            }
        }
    });
    app.add_action(&quit);
    for name in ["about", "preferences"] {
        let weak = app.downgrade();
        let act = gio::SimpleAction::new(name, None);
        act.connect_activate(move |_, _| {
            if let Some(w) = weak.upgrade().and_then(|a| a.active_window()) {
                let _ = WidgetExt::activate_action(&w, &format!("win.{name}"), None);
            }
        });
        app.add_action(&act);
    }
}

/// Läuft das Programm aus einem App-Bundle (`Stijl.app/Contents/MacOS/stijl`), liegen
/// Symbole, Schemas, Übersetzungen und Bildlader unter `Contents/Resources`. Homebrew-GTK
/// sucht sie sonst unter `/opt/homebrew`.
#[cfg(target_os = "macos")]
fn mac_bundle_env() {
    let Some(res) = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .and_then(|p| p.parent()?.parent().map(|c| c.join("Resources")))
        .filter(|r| r.join("share").is_dir())
    else {
        return;
    };
    let share = res.join("share");
    std::env::set_var("XDG_DATA_DIRS", &share);
    std::env::set_var("GSETTINGS_SCHEMA_DIR", share.join("glib-2.0").join("schemas"));
    // Bildlader: die Liste enthält feste Pfade und wird deshalb beim Start aus der Vorlage
    // (mit Platzhalter für den Ort des Bundles) erzeugt
    let lib = res.join("lib").join("gdk-pixbuf-2.0").join("2.10.0");
    if let Ok(tpl) = std::fs::read_to_string(lib.join("loaders.cache.in")) {
        let text = tpl.replace("@RESOURCES@", &res.to_string_lossy());
        let dir = glib::user_cache_dir().join("stijl");
        let file = dir.join("loaders.cache");
        if std::fs::read_to_string(&file).ok().as_deref() != Some(text.as_str()) {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&file, &text);
        }
        std::env::set_var("GDK_PIXBUF_MODULE_FILE", &file);
        std::env::set_var("GDK_PIXBUF_MODULEDIR", lib.join("loaders"));
    }
}
