//! Mondrian – ein einfacher Vektorzeichner für GNOME (GTK 4 + libadwaita).
// Windows: als Fensterprogramm starten (ohne zusätzliches Konsolenfenster)
#![cfg_attr(windows, windows_subsystem = "windows")]

mod ai;
mod boolean;
mod editor;
mod export;
mod fsutil;
mod geom;
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
mod svg;
mod trace;
mod window;
mod xml;

use adw::prelude::*;
use gtk::{gio, glib};

const APP_ID: &str = "io.github.mondrian.Mondrian";

fn main() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    app.connect_startup(|app| {
        icons::install();
        window::set_accels(app);
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
