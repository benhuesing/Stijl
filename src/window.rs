// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! Hauptfenster: Kopfleiste (links Seitenleiste, Rückgängig/Wiederholen; in der Mitte die
//! Werkzeuge; rechts Eigenschaften, Neu, Menü), Tabs, links Ebenen und Bibliothek,
//! Zeichenfläche mit Linealen, rechts die Eigenschaften.

use crate::i18n::{tr, trf};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib, pango};

use crate::ai;
use crate::boolean::Op;
use crate::editor::{Drag, Editor, Mods, Move, Tool};
use crate::export::{self, Format};
use crate::geom::Pt;
use crate::library::{self, Library};
use crate::model::{
    Align, Blend, Color, Document, Gradient, GradientKind, ImagePart, LineCap, LineJoin, ListStyle, Marker,
    MarkerEnds, PaintTarget, Shape, ShapeKind, VAlign,
};
use crate::render::{self, CairoMetrics, Unit, RULER};
use crate::svg;

const SHAPE_KINDS: [(ShapeKind, &str); 5] = [
    (ShapeKind::Rect, "Rechteck"),
    (ShapeKind::Ellipse, "Ellipse"),
    (ShapeKind::Line, "Linie"),
    (ShapeKind::Polygon, "Vieleck"),
    (ShapeKind::Star, "Stern"),
];

const PANEL_WIDTH: i32 = 300;
/// Schmalste Seitenleiste
const PANEL_MIN: i32 = 260;

type Then = Box<dyn FnOnce(&Rc<Ui>)>;
type Done = Box<dyn FnOnce(&Rc<Ui>, bool)>;

#[derive(Clone)]
struct ExportSettings {
    format: u32,
    background: bool,
    scale: u32,
    quality: f64,
    embed_fonts: bool,
    /// Qualität der Effekte: 0 klein, 1 mittel, 2 hoch
    effect_quality: u32,
    /// PDF: Text durchsuchbar (sonst Umrisse)
    pdf_text: bool,
    /// PDF: 0 RGB, 1 CMYK, 2 PDF/X-4
    color: u32,
    /// PDF: Farbprofil (Pfad, Beschreibung)
    profile: Option<(PathBuf, String)>,
    /// PDF: Beschnittzugabe in mm
    bleed: f64,
}

/// Auflösung der Effekte je Qualitätsstufe (dpi).
const EFFECT_DPI: [f64; 3] = [150.0, 300.0, 600.0];

/// Tab mit der Willkommensseite (Neu, Öffnen, Formate, zuletzt geöffnet).
struct StartTab {
    page: adw::TabPage,
    /// eine Reihe Karten der zuletzt geöffneten Dokumente
    recent_list: gtk::FlowBox,
    recent_box: gtk::Box,
}

/// Ein geöffnetes Dokument (ein Tab).
pub struct DocTab {
    ed: RefCell<Editor>,
    file: RefCell<Option<PathBuf>>,
    /// Name eines importierten Dokuments (z. B. aus einer .ai-Datei), das noch keine eigene Datei hat
    title: RefCell<Option<String>>,
    canvas: gtk::DrawingArea,
    page: RefCell<Option<adw::TabPage>>,
    /// Tastatur der Zeichenfläche; die Eingabemethode hängt nur beim Textbearbeiten daran.
    keys: gtk::EventControllerKey,
    im: gtk::IMMulticontext,
    im_on: Cell<bool>,
    /// Läuft gerade ein Zeitgeber, der die Zeichenfläche neu zeichnet (Wartekreis)?
    ticking: Cell<bool>,
    /// Lage der Zeichenfläche im Fenster beim letzten Zeichnen: Öffnet sich links eine
    /// Leiste, wird die Ansicht um dieselbe Strecke verschoben, damit das Dokument stehen bleibt.
    origin: Cell<Option<(f64, f64)>>,
    /// Zeichenfläche mit der schwebenden Leiste darüber (Inhalt des Tabs)
    holder: gtk::Overlay,
    float: FloatBar,
    /// Kennung der Wiederherstellungsdatei und Änderungsstand, der zuletzt gesichert wurde
    recovery_id: String,
    recovery_edits: Cell<Option<u64>>,
    /// zuletzt gezeichnete Ansicht (Zoom, Verschiebung) und wann sie sich zuletzt geändert hat
    view_seen: Cell<(f64, f64, f64)>,
    view_changed: Cell<Option<std::time::Instant>>,
    /// Neuzeichnen nach dem Verschieben/Zoomen ist vorgemerkt
    settle_pending: Cell<bool>,
}

/// So lange nach der letzten Änderung der Ansicht gilt das Zoomen/Verschieben als laufend
/// (Effekte werden erst danach neu berechnet).
const SETTLE_MS: u64 = 300;

/// HSV (Farbton 0–360, Sättigung und Helligkeit 0–1) in RGB.
fn hsv_to_rgb(h: f64, s: f64, v: f64) -> (f64, f64, f64) {
    let h = (h.rem_euclid(360.0)) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    (r + m, g + m, b + m)
}

fn rgb_to_hsv(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d < 1e-9 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max < 1e-9 { 0.0 } else { d / max };
    (h, s, max)
}

/// Abgerundetes Rechteck als Pfad.
fn round_rect(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(
        x + r,
        y + h - r,
        r,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    cr.arc(x + r, y + r, r, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
    cr.close_path();
}

/// Ziehpunkt der Farbwahl: weißer Ring mit dunklem Rand.
fn picker_knob(cr: &gtk::cairo::Context, x: f64, y: f64, fill: Option<(f64, f64, f64)>) {
    cr.new_path();
    cr.arc(x, y, 7.0, 0.0, std::f64::consts::TAU);
    if let Some((r, g, b)) = fill {
        cr.set_source_rgb(r, g, b);
        let _ = cr.fill_preserve();
    }
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.set_line_width(3.0);
    let _ = cr.stroke_preserve();
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.35);
    cr.set_line_width(1.0);
    cr.new_path();
    cr.arc(x, y, 8.5, 0.0, std::f64::consts::TAU);
    let _ = cr.stroke();
}

/// Schachbrett für durchsichtige Farben.
fn checker(cr: &gtk::cairo::Context, w: f64, h: f64, size: f64) {
    cr.set_source_rgb(1.0, 1.0, 1.0);
    let _ = cr.paint();
    cr.set_source_rgb(0.8, 0.8, 0.8);
    let (nx, ny) = ((w / size).ceil() as i32, (h / size).ceil() as i32);
    for j in 0..ny {
        for i in 0..nx {
            if (i + j) % 2 == 0 {
                cr.rectangle(i as f64 * size, j as f64 * size, size, size);
            }
        }
    }
    let _ = cr.fill();
}

/// Eingebettete Farbwahl: Feld für Sättigung/Helligkeit, Farbton- und Deckkraftleiste,
/// Hex-Wert. Jede Änderung wird sofort gemeldet.
struct ColorPicker {
    root: gtk::Box,
    sv: gtk::DrawingArea,
    hue: gtk::DrawingArea,
    alpha: gtk::DrawingArea,
    /// Feld neben der Leiste zum Eintippen der Deckkraft in Prozent
    alpha_spin: gtk::SpinButton,
    dot: gtk::DrawingArea,
    hex: gtk::Entry,
    hex_row: gtk::Box,
    /// Farbton, Sättigung, Helligkeit
    hsv: Cell<(f64, f64, f64)>,
    a: Cell<f64>,
    on_change: RefCell<Option<Rc<dyn Fn(Color)>>>,
    /// nur die Deckkraftleiste bewegt (Farbe bleibt bei jedem Objekt erhalten)
    on_alpha: RefCell<Option<Rc<dyn Fn(f64)>>>,
}

impl ColorPicker {
    fn new() -> Rc<ColorPicker> {
        let root = vbox(8);
        let sv = gtk::DrawingArea::new();
        sv.set_content_height(120);
        sv.set_hexpand(true);
        sv.set_cursor_from_name(Some("crosshair"));
        let bar = || {
            let b = gtk::DrawingArea::new();
            b.set_content_height(18);
            b.set_hexpand(true);
            b.set_cursor_from_name(Some("pointer"));
            b
        };
        let hue = bar();
        let alpha = bar();
        alpha.set_tooltip_text(Some(tr("Deckkraft der Farbe")));
        let dot = gtk::DrawingArea::new();
        dot.set_content_width(22);
        dot.set_content_height(22);
        dot.set_valign(gtk::Align::Center);
        let hex = gtk::Entry::builder().hexpand(true).max_length(9).build();
        hex.set_tooltip_text(Some(tr("Farbwert, z. B. #3584E4")));
        let hex_row = hbox(8);
        hex_row.add_css_class("mondrian-hex");
        hex_row.append(&dot);
        hex_row.append(&hex);
        root.append(&sv);
        root.append(&hue);
        let alpha_spin = gtk::SpinButton::with_range(0.0, 100.0, 1.0);
        alpha_spin.set_width_chars(4);
        alpha_spin.set_numeric(true);
        alpha_spin.set_valign(gtk::Align::Center);
        alpha_spin.set_tooltip_text(Some(tr("Deckkraft in Prozent – Wert eintippen")));
        let alpha_unit = gtk::Label::new(Some("%"));
        alpha_unit.add_css_class("dim-label");
        let alpha_row = hbox(6);
        // die Leiste behält ihre Höhe, auch wenn das Zahlenfeld höher ist
        alpha.set_valign(gtk::Align::Center);
        alpha_row.append(&alpha);
        alpha_row.append(&alpha_spin);
        alpha_row.append(&alpha_unit);
        root.append(&alpha_row);
        root.append(&hex_row);
        let hex_row = hex_row.clone();
        let p = Rc::new(ColorPicker {
            root,
            sv,
            hue,
            alpha,
            alpha_spin,
            dot,
            hex,
            hex_row,
            hsv: Cell::new((0.0, 0.0, 0.0)),
            a: Cell::new(1.0),
            on_change: RefCell::new(None),
            on_alpha: RefCell::new(None),
        });
        // Zeichnen
        {
            let w = Rc::downgrade(&p);
            p.sv.set_draw_func(move |_, cr, w_, h_| {
                let Some(p) = w.upgrade() else { return };
                let (w_, h_) = (w_ as f64, h_ as f64);
                let (h, s, v) = p.hsv.get();
                round_rect(cr, 0.0, 0.0, w_, h_, 8.0);
                cr.clip();
                let (r, g, b) = hsv_to_rgb(h, 1.0, 1.0);
                cr.set_source_rgb(r, g, b);
                let _ = cr.paint();
                let white = gtk::cairo::LinearGradient::new(0.0, 0.0, w_, 0.0);
                white.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 1.0);
                white.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.0);
                let _ = cr.set_source(&white);
                let _ = cr.paint();
                let black = gtk::cairo::LinearGradient::new(0.0, 0.0, 0.0, h_);
                black.add_color_stop_rgba(0.0, 0.0, 0.0, 0.0, 0.0);
                black.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, 1.0);
                let _ = cr.set_source(&black);
                let _ = cr.paint();
                cr.reset_clip();
                picker_knob(cr, s * w_, (1.0 - v) * h_, Some(hsv_to_rgb(h, s, v)));
            });
        }
        {
            let w = Rc::downgrade(&p);
            p.hue.set_draw_func(move |_, cr, w_, h_| {
                let Some(p) = w.upgrade() else { return };
                let (w_, h_) = (w_ as f64, h_ as f64);
                let (h, _, _) = p.hsv.get();
                round_rect(cr, 0.0, 2.0, w_, h_ - 4.0, h_ / 2.0);
                let g = gtk::cairo::LinearGradient::new(0.0, 0.0, w_, 0.0);
                for k in 0..=6 {
                    let (r, gg, b) = hsv_to_rgb(k as f64 * 60.0, 1.0, 1.0);
                    g.add_color_stop_rgb(k as f64 / 6.0, r, gg, b);
                }
                let _ = cr.set_source(&g);
                let _ = cr.fill();
                let x = (h / 360.0 * (w_ - h_) + h_ / 2.0).clamp(h_ / 2.0, w_ - h_ / 2.0);
                picker_knob(cr, x, h_ / 2.0, Some(hsv_to_rgb(h, 1.0, 1.0)));
            });
        }
        {
            let w = Rc::downgrade(&p);
            p.alpha.set_draw_func(move |_, cr, w_, h_| {
                let Some(p) = w.upgrade() else { return };
                let (w_, h_) = (w_ as f64, h_ as f64);
                let (h, s, v) = p.hsv.get();
                let (r, g, b) = hsv_to_rgb(h, s, v);
                let _ = cr.save();
                round_rect(cr, 0.0, 2.0, w_, h_ - 4.0, h_ / 2.0);
                cr.clip();
                checker(cr, w_, h_, 5.0);
                let lg = gtk::cairo::LinearGradient::new(0.0, 0.0, w_, 0.0);
                lg.add_color_stop_rgba(0.0, r, g, b, 0.0);
                lg.add_color_stop_rgba(1.0, r, g, b, 1.0);
                let _ = cr.set_source(&lg);
                let _ = cr.paint();
                let _ = cr.restore();
                let x = (p.a.get() * (w_ - h_) + h_ / 2.0).clamp(h_ / 2.0, w_ - h_ / 2.0);
                picker_knob(cr, x, h_ / 2.0, None);
            });
        }
        {
            let w = Rc::downgrade(&p);
            p.dot.set_draw_func(move |_, cr, w_, h_| {
                let Some(p) = w.upgrade() else { return };
                draw_swatch(cr, w_ as f64, h_ as f64, Some(p.color()), None, false);
            });
        }
        // Ziehen im Feld und auf den Leisten
        let drag_on = |area: &gtk::DrawingArea, f: Rc<dyn Fn(f64, f64, f64, f64)>| {
            let g = gtk::GestureDrag::new();
            {
                let (a, f) = (area.clone(), f.clone());
                g.connect_drag_begin(move |_, x, y| f(x, y, a.width() as f64, a.height() as f64));
            }
            {
                let (a, f) = (area.clone(), f.clone());
                g.connect_drag_update(move |g, dx, dy| {
                    if let Some((x, y)) = g.start_point() {
                        f(x + dx, y + dy, a.width() as f64, a.height() as f64);
                    }
                });
            }
            area.add_controller(g);
        };
        {
            let w = Rc::downgrade(&p);
            drag_on(
                &p.sv,
                Rc::new(move |x: f64, y: f64, aw: f64, ah: f64| {
                    if let Some(p) = w.upgrade() {
                        let (h, _, _) = p.hsv.get();
                        let s = (x / aw.max(1.0)).clamp(0.0, 1.0);
                        let v = (1.0 - y / ah.max(1.0)).clamp(0.0, 1.0);
                        p.hsv.set((h, s, v));
                        p.changed();
                    }
                }),
            );
        }
        {
            let w = Rc::downgrade(&p);
            drag_on(
                &p.hue,
                Rc::new(move |x: f64, _y: f64, aw: f64, ah: f64| {
                    if let Some(p) = w.upgrade() {
                        let (_, s, v) = p.hsv.get();
                        let t = ((x - ah / 2.0) / (aw - ah).max(1.0)).clamp(0.0, 1.0);
                        p.hsv.set((t * 359.999, s, v));
                        p.changed();
                    }
                }),
            );
        }
        {
            let w = Rc::downgrade(&p);
            drag_on(
                &p.alpha,
                Rc::new(move |x: f64, _y: f64, aw: f64, ah: f64| {
                    if let Some(p) = w.upgrade() {
                        let t = ((x - ah / 2.0) / (aw - ah).max(1.0)).clamp(0.0, 1.0);
                        p.a.set((t * 100.0).round() / 100.0);
                        let f = p.on_alpha.borrow().clone();
                        match f {
                            Some(f) => {
                                p.redraw();
                                f(p.a.get());
                            }
                            None => p.changed(),
                        }
                    }
                }),
            );
        }
        {
            // Deckkraft eintippen
            let w = Rc::downgrade(&p);
            p.alpha_spin.connect_value_changed(move |sp| {
                let Some(p) = w.upgrade() else { return };
                let a = (sp.value() / 100.0).clamp(0.0, 1.0);
                if (a - p.a.get()).abs() < 0.004 {
                    return;
                }
                p.a.set(a);
                let f = p.on_alpha.borrow().clone();
                match f {
                    Some(f) => {
                        p.redraw();
                        f(p.a.get());
                    }
                    None => p.changed(),
                }
            });
        }
        {
            let w = Rc::downgrade(&p);
            p.hex.connect_activate(move |e| {
                let Some(p) = w.upgrade() else { return };
                let t = e.text().trim().to_string();
                let t = if t.starts_with('#') { t } else { format!("#{t}") };
                match Color::parse(&t) {
                    Some(c) => {
                        p.show(c);
                        p.changed();
                    }
                    None => p.show(p.color()),
                }
            });
        }
        p
    }

    /// Nur die Deckkraftleiste zeigen (z. B. bei Bildern).
    fn set_alpha_only(&self, v: bool) {
        self.sv.set_visible(!v);
        self.hue.set_visible(!v);
        self.hex_row.set_visible(!v);
    }

    fn color(&self) -> Color {
        let (h, s, v) = self.hsv.get();
        let (r, g, b) = hsv_to_rgb(h, s, v);
        Color {
            r,
            g,
            b,
            a: self.a.get(),
        }
    }

    /// Farbe anzeigen (ohne Meldung). Der Farbton bleibt bei Grau erhalten.
    fn show(&self, c: Color) {
        if !same_rgba(self.color(), c) {
            let (h0, _, _) = self.hsv.get();
            let (h, s, v) = rgb_to_hsv(c.r, c.g, c.b);
            let h = if s < 1e-6 || v < 1e-6 { h0 } else { h };
            self.hsv.set((h, s, v));
            self.a.set(c.a);
        }
        self.redraw();
    }

    fn redraw(&self) {
        self.sv.queue_draw();
        self.hue.queue_draw();
        self.alpha.queue_draw();
        self.dot.queue_draw();
        let c = self.color();
        let pct = (c.a * 100.0).round();
        if (self.alpha_spin.value() - pct).abs() > 0.5 {
            self.alpha_spin.set_value(pct);
        }
        let mut t = c.to_hex().to_uppercase();
        if c.a < 0.999 {
            t.push_str(&format!("{:02X}", (c.a * 255.0).round() as u8));
        }
        if !self.hex.has_focus() && self.hex.text().as_str() != t {
            self.hex.set_text(&t);
        }
    }

    fn changed(&self) {
        self.redraw();
        let f = self.on_change.borrow().clone();
        if let Some(f) = f {
            f(self.color());
        }
    }
}

/// Wofür eine Farbwahl in der Seitenleiste gilt.
#[derive(Clone, Copy, Debug, PartialEq)]
enum SwTarget {
    /// Füllung, Kontur oder Schatten eines Objekts
    Paint(PaintTarget),
    /// Füllung und Kontur des Rahmens eines Textfeldes (ohne Verlauf)
    FrameFill,
    FrameStroke,
    /// Deckkraft eines Bildes (nur die Deckkraftleiste)
    ImageOpacity,
}

thread_local! {
    /// Ausklappbereiche der Seitenleiste: (Inhalt, Pfeil, offen). Immer nur einer offen.
    static FOLDS: RefCell<Vec<(gtk::Box, gtk::Button, Rc<Cell<bool>>)>> = const { RefCell::new(Vec::new()) };
}

fn fold_register(detail: &gtk::Box, arrow: &gtk::Button, open: &Rc<Cell<bool>>) -> usize {
    FOLDS.with(|f| {
        let mut f = f.borrow_mut();
        f.push((detail.clone(), arrow.clone(), open.clone()));
        f.len() - 1
    })
}

/// Bereich auf- oder zuklappen; beim Aufklappen schließen sich alle anderen.
fn fold_set(idx: usize, v: bool) {
    let all: Vec<(gtk::Box, gtk::Button, Rc<Cell<bool>>)> = FOLDS.with(|f| f.borrow().clone());
    for (i, (d, a, o)) in all.iter().enumerate() {
        let open = if i == idx {
            v
        } else if v {
            false
        } else {
            o.get()
        };
        if o.get() != open || i == idx {
            o.set(open);
            d.set_visible(open);
            a.set_icon_name(if open {
                "pan-down-symbolic"
            } else {
                "pan-end-symbolic"
            });
        }
    }
}

/// Ausklappbereich eines Effekts: Auswahl (falls mehrere), Stärke, Entfernen.
struct FxFold {
    root: gtk::Box,
    hint: gtk::Label,
    /// Auswahlliste der Arten (bleibt unsichtbar, wenn es nur eine Art gibt)
    choice: gtk::DropDown,
    /// Hauptregler (0–100 %, beim 3D-Effekt −100…100 %)
    strength: gtk::Scale,
    /// zweiter Regler (3D-Effekt: Beleuchtung)
    strength2: gtk::Scale,
}

/// Einheitlicher Regler: runde Leiste wie bei der Deckkraft der Farbe, daneben ein Feld, in
/// das man die Zahl eintippen kann. Leiste und Feld teilen sich dieselben Werte. Mit
/// Beschriftung steht sie über dem Regler.
fn slider_row(label: &str, scale: &gtk::Scale) -> gtk::Box {
    slider_row_unit(label, scale, "%")
}

/// Wie `slider_row`, mit eigener Einheit (leer: keine).
fn slider_row_unit(label: &str, scale: &gtk::Scale, unit: &str) -> gtk::Box {
    scale.add_css_class("mondrian-slider");
    scale.set_draw_value(false);
    scale.set_digits(0);
    scale.set_hexpand(true);
    let spin = gtk::SpinButton::new(Some(&scale.adjustment()), 1.0, 0);
    spin.set_width_chars(4);
    spin.set_numeric(true);
    spin.set_valign(gtk::Align::Center);
    spin.set_tooltip_text(Some(tr("Wert eintippen")));
    let line = hbox(6);
    line.append(scale);
    line.append(&spin);
    if !unit.is_empty() {
        let unit = gtk::Label::new(Some(unit));
        unit.add_css_class("dim-label");
        line.append(&unit);
    }
    if label.is_empty() {
        return line;
    }
    let l = gtk::Label::builder().label(label).xalign(0.0).build();
    let b = vbox(4);
    b.add_css_class("mondrian-prow");
    b.add_css_class("mondrian-prow-wide");
    b.append(&l);
    b.append(&line);
    b
}

fn new_slider(min: f64, max: f64) -> gtk::Scale {
    gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, 5.0)
}

/// Ausklappbereich eines Effekts. `strength` und `second` sind die Beschriftungen der
/// Regler (ohne Angabe gibt es den Regler nicht).
fn fx_fold(
    title: &str,
    choices: &[&str],
    strength: Option<&str>,
    second: Option<&str>,
    remove_label: &str,
    remove_action: &str,
) -> FxFold {
    let t = gtk::Label::new(Some(title));
    t.add_css_class("heading");
    let hint = gtk::Label::new(None);
    let arrow = gtk::Button::from_icon_name("pan-end-symbolic");
    let head = fold_head(&t, &hint, None, &arrow);
    // alles in einer Gruppe: Art, Stärke und Entfernen ohne Trennlinien
    let detail = group_box();
    let choice = gtk::DropDown::from_strings(choices);
    if !choices.is_empty() {
        detail.append(&prow(tr("Art"), &choice));
    }
    let scale = new_slider(0.0, 100.0);
    scale.set_tooltip_text(Some(tr("Stärke")));
    if let Some(l) = strength {
        detail.append(&slider_row(l, &scale));
    }
    let scale2 = new_slider(0.0, 100.0);
    if let Some(l) = second {
        detail.append(&slider_row(l, &scale2));
    }
    let remove = gtk::Button::with_label(remove_label);
    remove.add_css_class("mondrian-layer-delete");
    remove.set_action_name(Some(remove_action));
    detail.append(&wide(&remove));
    detail.set_visible(false);
    let open = Rc::new(Cell::new(false));
    let idx = fold_register(&detail, &arrow, &open);
    {
        let open = open.clone();
        arrow.connect_clicked(move |_| fold_set(idx, !open.get()));
    }
    {
        let open = open.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| fold_set(idx, !open.get()));
        t.add_controller(click);
    }
    let root = rows_box();
    root.add_css_class("mondrian-fold");
    root.append(&head);
    root.append(&detail);
    root.set_visible(false);
    FxFold {
        root,
        hint,
        choice,
        strength: scale,
        strength2: scale2,
    }
}

/// Kopfzeile eines Ausklappbereichs: Titel, Hinweis (z. B. „80 %“), optional Farbkreis, Pfeil.
fn fold_head(
    title: &gtk::Label,
    hint: &gtk::Label,
    chip: Option<&gtk::DrawingArea>,
    arrow: &gtk::Button,
) -> gtk::Box {
    title.set_hexpand(true);
    title.set_xalign(0.0);
    hint.add_css_class("dim-label");
    hint.add_css_class("caption");
    arrow.add_css_class("flat");
    arrow.add_css_class("circular");
    arrow.set_valign(gtk::Align::Center);
    arrow.set_tooltip_text(Some(tr("Auf- oder zuklappen")));
    let head = hbox(6);
    head.add_css_class("mondrian-fold-head");
    head.append(title);
    head.append(hint);
    if let Some(c) = chip {
        head.append(c);
    }
    head.append(arrow);
    head
}

/// Was ein Farbkreis darstellt.
#[derive(Clone)]
enum SwItem {
    None,
    Lib(Color, Option<Gradient>),
    /// die eigene Farbe des Objekts (nicht in der Bibliothek)
    Custom,
}

/// Farbwahl für Füllung, Kontur oder Schatten: Reiter Farbe/Verlauf, Verlaufsleiste,
/// Farbwahl und darunter die Farben der Bibliothek („keine“, Bibliothek, eigene Farbe,
/// „+“ = aktuelle Farbe in die Bibliothek).
struct Swatches {
    target: SwTarget,
    /// Hinweis in der Kopfzeile (Deckkraft, bei der Kontur auch die Stärke)
    hint: gtk::Label,
    /// zusätzliche Einstellungen am Ende des aufgeklappten Bereichs (z. B. Linienstärke)
    extra: gtk::Box,
    /// Nummer im Ausklapp-Verbund und offen/zu
    fold: Cell<usize>,
    ui: RefCell<std::rc::Weak<Ui>>,
    root: gtk::Box,
    title: gtk::Label,
    chip: gtk::DrawingArea,
    arrow: gtk::Button,
    expanded: Rc<Cell<bool>>,
    tabs: (gtk::ToggleButton, gtk::ToggleButton),
    /// Reiter „Muster“ (nur Füllung)
    pattern_tab: gtk::ToggleButton,
    grad_area: gtk::Box,
    /// linear / kreisförmig
    shape_btns: (gtk::ToggleButton, gtk::ToggleButton),
    /// Muster: Vorschaubilder, zwei Farben, Größe, Winkel
    pattern_area: gtk::Box,
    pattern_btns: Vec<(crate::model::GradShape, gtk::ToggleButton)>,
    pattern_chips: [gtk::DrawingArea; 2],
    pattern_cell: gtk::SpinButton,
    pattern_angle: gtk::SpinButton,
    /// aktuelle Form des Verlaufs
    shape: Cell<crate::model::GradShape>,
    bar: gtk::DrawingArea,
    angle: gtk::SpinButton,
    picker: Rc<ColorPicker>,
    grid: gtk::Grid,
    /// Farbe des Objekts (None = keine) und ihr Verlauf
    current: Cell<Option<Color>>,
    grad: RefCell<Option<Gradient>>,
    /// gewählter Farbstopp
    stop: Cell<usize>,
    /// Farbe, die die Farbwahl gerade zeigt
    shown: Cell<Option<Color>>,
    /// Farben der Bibliothek, wie sie zuletzt aufgebaut wurden
    lib: RefCell<Vec<(Color, Option<Gradient>)>>,
    key: RefCell<String>,
    syncing: Cell<bool>,
    /// Pipette: Farbe aus dem Dokument aufnehmen
    pipette_btn: gtk::ToggleButton,
}

/// Pipette beenden (Knopf zurücksetzen, Lupe entfernen).
fn end_pipette(u: &Rc<Ui>) {
    let sw = u.pipette.borrow_mut().take().and_then(|w| w.upgrade());
    u.pipette_at.set(None);
    if let Some(sw) = sw {
        if sw.pipette_btn.is_active() {
            sw.pipette_btn.set_active(false);
        }
    }
    if let Some(d) = cur(u) {
        d.canvas.set_cursor_from_name(Some("default"));
        d.canvas.queue_draw();
    }
}

/// Aufgenommene Farbe übernehmen (die Deckkraft der Farbwahl bleibt).
fn pipette_pick(u: &Rc<Ui>, c: Color) {
    let sw = u.pipette.borrow().as_ref().and_then(|w| w.upgrade());
    if let Some(sw) = sw {
        let a = sw.picker.color().a;
        sw.picker.show(Color { a, ..c });
        sw.picker.changed();
    }
    end_pipette(u);
}

fn pipette_active(u: &Ui) -> bool {
    u.pipette.borrow().as_ref().is_some_and(|w| w.upgrade().is_some())
}

/// Durchmesser der Farbkreise in der Seitenleiste
const SWATCH: i32 = 30;

fn same_rgba(a: Color, b: Color) -> bool {
    (a.r - b.r).abs() < 2e-3
        && (a.g - b.g).abs() < 2e-3
        && (a.b - b.b).abs() < 2e-3
        && (a.a - b.a).abs() < 2e-3
}

/// Vorschaubild eines Musters (abgerundet, mit den beiden Farben).
fn draw_pattern_preview(
    cr: &gtk::cairo::Context,
    w: f64,
    h: f64,
    shape: crate::model::GradShape,
    c1: Color,
    c2: Color,
) {
    use crate::model::GradShape;
    round_rect(cr, 1.0, 1.0, w - 2.0, h - 2.0, 6.0);
    cr.clip();
    checker(cr, w, h, 5.0);
    cr.set_source_rgba(c1.r, c1.g, c1.b, c1.a);
    let _ = cr.paint();
    cr.set_source_rgba(c2.r, c2.g, c2.b, c2.a);
    let k = (w.min(h) / 4.0).max(2.0);
    match shape {
        GradShape::Checker => {
            let mut y = 0.0;
            let mut row_ = 0;
            while y < h {
                let mut x = if row_ % 2 == 0 { 0.0 } else { k };
                while x < w {
                    cr.rectangle(x, y, k, k);
                    x += 2.0 * k;
                }
                y += k;
                row_ += 1;
            }
        }
        GradShape::Dots => {
            let mut y = k * 0.5;
            let mut row_ = 0;
            while y < h + k {
                let mut x = if row_ % 2 == 0 { k * 0.5 } else { k * 1.5 };
                while x < w + k {
                    cr.new_sub_path();
                    cr.arc(x, y, k * 0.3, 0.0, std::f64::consts::TAU);
                    x += 2.0 * k;
                }
                y += k;
                row_ += 1;
            }
        }
        _ => {
            let mut x = 0.0;
            while x < w {
                cr.rectangle(x, 0.0, k * 0.5, h);
                x += k;
            }
        }
    }
    let _ = cr.fill();
    cr.reset_clip();
    round_rect(cr, 1.0, 1.0, w - 2.0, h - 2.0, 6.0);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.2);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
}

/// Farbkreis zeichnen; `None` = durchgestrichen (keine Farbe), gewählt = mit Ring.
/// Mit Verlauf wird der Kreis von links nach rechts mit dem Verlauf gefüllt.
fn draw_swatch(
    cr: &gtk::cairo::Context,
    w: f64,
    h: f64,
    c: Option<Color>,
    g: Option<&Gradient>,
    selected: bool,
) {
    let (cx, cy) = (w / 2.0, h / 2.0);
    let outer = w.min(h) / 2.0 - 1.0;
    let r = if selected { outer - 3.5 } else { outer - 0.5 };
    if selected {
        cr.arc(cx, cy, outer - 0.75, 0.0, std::f64::consts::TAU);
        let a = render::ui_accent();
        cr.set_source_rgb(a.0, a.1, a.2);
        cr.set_line_width(1.5);
        let _ = cr.stroke();
    }
    cr.new_path();
    cr.arc(cx, cy, r, 0.0, std::f64::consts::TAU);
    match c {
        Some(c) => {
            let transparent = c.a < 1.0
                || g.map(|g| g.stops.iter().any(|s| s.color.a < 1.0))
                    .unwrap_or(false);
            if transparent {
                cr.set_source_rgb(0.85, 0.85, 0.85);
                let _ = cr.fill_preserve();
            }
            match g {
                Some(g) => {
                    let lg = gtk::cairo::LinearGradient::new(cx - r, 0.0, cx + r, 0.0);
                    for (pos, col) in g.render_stops() {
                        lg.add_color_stop_rgba(pos.clamp(0.0, 1.0), col.r, col.g, col.b, col.a);
                    }
                    let _ = cr.set_source(&lg);
                }
                None => cr.set_source_rgba(c.r, c.g, c.b, c.a),
            }
            let _ = cr.fill_preserve();
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.25);
            cr.set_line_width(1.0);
            let _ = cr.stroke();
        }
        None => {
            cr.set_source_rgb(1.0, 1.0, 1.0);
            let _ = cr.fill_preserve();
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.3);
            cr.set_line_width(1.0);
            let _ = cr.stroke();
            let d = r * std::f64::consts::FRAC_1_SQRT_2;
            cr.move_to(cx - d, cy + d);
            cr.line_to(cx + d, cy - d);
            cr.set_source_rgb(0.88, 0.11, 0.14);
            cr.set_line_width(2.0);
            let _ = cr.stroke();
        }
    }
}

impl Swatches {
    fn new(title: &str, target: SwTarget) -> Rc<Swatches> {
        let root = rows_box();
        let title = gtk::Label::builder().label(title).xalign(0.0).build();
        title.add_css_class("heading");
        // Reiter Farbe / Verlauf
        let solid = gtk::ToggleButton::with_label(tr("Farbe"));
        let grad_tab = gtk::ToggleButton::with_label(tr("Verlauf"));
        grad_tab.set_group(Some(&solid));
        solid.set_active(true);
        for b in [&solid, &grad_tab] {
            b.set_hexpand(true);
            b.add_css_class("flat");
            b.add_css_class("mondrian-tab");
        }
        let pattern_tab = gtk::ToggleButton::with_label(tr("Muster"));
        pattern_tab.set_group(Some(&solid));
        pattern_tab.set_hexpand(true);
        pattern_tab.add_css_class("flat");
        pattern_tab.add_css_class("mondrian-tab");
        // Muster gibt es nur für die Füllung
        pattern_tab.set_visible(target == SwTarget::Paint(PaintTarget::Fill));
        let tab_row = hbox(0);
        tab_row.set_homogeneous(true);
        tab_row.set_hexpand(true);
        tab_row.append(&solid);
        tab_row.append(&grad_tab);
        tab_row.append(&pattern_tab);
        // Pipette rechts neben den Reitern
        let pipette_btn = gtk::ToggleButton::new();
        pipette_btn.set_icon_name("color-select-symbolic");
        pipette_btn.add_css_class("flat");
        pipette_btn.set_valign(gtk::Align::Center);
        pipette_btn.set_tooltip_text(Some(tr(
            "Pipette: Farbe aus dem Dokument oder einem Foto aufnehmen (Esc bricht ab)",
        )));
        let tabs_line = hbox(4);
        tabs_line.append(&tab_row);
        tabs_line.append(&pipette_btn);
        // Verlaufsleiste (wie die Farbleisten: runde Enden, Farbstopps als Kreise)
        let bar = gtk::DrawingArea::new();
        // etwas höher als die Leiste selbst, damit der Ring um den gewählten Stopp Platz hat
        bar.set_content_height(26);
        bar.set_hexpand(true);
        bar.set_focusable(true);
        bar.set_cursor_from_name(Some("pointer"));
        bar.set_tooltip_text(Some(tr(
            "Klick auf die Leiste: Farbstopp hinzufügen · Farbstopp ziehen: verschieben · Entf: entfernen",
        )));
        let angle = spin(-180.0, 180.0, 1.0, 0, tr("Winkel des Verlaufs in Grad"));
        angle.set_wrap(true);
        // linear oder kreisförmig
        let lin_btn = gtk::ToggleButton::with_label(tr("Linear"));
        let rad_btn = gtk::ToggleButton::with_label(tr("Kreisförmig"));
        rad_btn.set_group(Some(&lin_btn));
        lin_btn.set_active(true);
        let shape_row = hbox(0);
        shape_row.add_css_class("linked");
        for b in [&lin_btn, &rad_btn] {
            b.set_hexpand(true);
            shape_row.append(b);
        }
        let angle_row = row(tr("Winkel (°)"), &angle);
        let grad_area = vbox(8);
        grad_area.append(&shape_row);
        grad_area.append(&bar);
        grad_area.append(&angle_row);
        grad_area.set_visible(false);
        // Muster: Vorschaubilder zur Auswahl
        let pattern_row = hbox(6);
        pattern_row.set_homogeneous(true);
        let mut pattern_btns: Vec<(crate::model::GradShape, gtk::ToggleButton)> = Vec::new();
        for shp in crate::model::GradShape::PATTERNS {
            let da = gtk::DrawingArea::new();
            da.set_content_width(40);
            da.set_content_height(40);
            let b = gtk::ToggleButton::new();
            b.set_child(Some(&da));
            b.add_css_class("flat");
            b.add_css_class("mondrian-pattern-btn");
            b.set_tooltip_text(Some(shp.label()));
            if let Some((_, f)) = pattern_btns.first() {
                b.set_group(Some(f));
            }
            pattern_row.append(&b);
            pattern_btns.push((shp, b));
        }
        // zwei Farben: Klick wählt, welche die Farbwahl ändert
        let chip_row = hbox(8);
        let mk_chip = |tip: &str| {
            let c = gtk::DrawingArea::new();
            c.set_content_width(30);
            c.set_content_height(30);
            c.set_cursor_from_name(Some("pointer"));
            c.set_tooltip_text(Some(tip));
            c
        };
        let pattern_chips = [mk_chip(tr("Grundfarbe")), mk_chip(tr("Musterfarbe"))];
        let chip_label = gtk::Label::builder()
            .label(tr("Farben"))
            .xalign(0.0)
            .hexpand(true)
            .build();
        chip_row.append(&chip_label);
        chip_row.append(&pattern_chips[0]);
        chip_row.append(&pattern_chips[1]);
        let (cell_box, pattern_cell) = unit_spin(2.0, 500.0, 1.0, 0, "px", tr("Größe des Musters"));
        let pattern_angle = spin(-180.0, 180.0, 1.0, 0, tr("Winkel des Musters in Grad"));
        pattern_angle.set_wrap(true);
        let pattern_area = vbox(8);
        pattern_area.append(&pattern_row);
        pattern_area.append(&chip_row);
        pattern_area.append(&row(tr("Größe"), &cell_box));
        pattern_area.append(&row(tr("Winkel (°)"), &pattern_angle));
        pattern_area.set_visible(false);
        let picker = ColorPicker::new();
        let grid = gtk::Grid::builder()
            .column_spacing(6)
            .row_spacing(6)
            .halign(gtk::Align::Start)
            .build();
        // Kopfzeile: Überschrift, aktuelle Farbe, Pfeil zum Aufklappen
        let chip = gtk::DrawingArea::new();
        chip.set_content_width(24);
        chip.set_content_height(24);
        chip.set_valign(gtk::Align::Center);
        chip.set_cursor_from_name(Some("pointer"));
        chip.set_tooltip_text(Some(tr("Farbwahl auf- oder zuklappen")));
        let arrow = gtk::Button::from_icon_name("pan-end-symbolic");
        arrow.add_css_class("flat");
        arrow.add_css_class("circular");
        arrow.set_valign(gtk::Align::Center);
        arrow.set_tooltip_text(Some(tr("Farbwahl auf- oder zuklappen")));
        let hint = gtk::Label::new(None);
        let head = fold_head(&title, &hint, Some(&chip), &arrow);
        // aufgeklappt: Farbwahl (mit Innenabstand), darunter weitere Einstellungen als Zeilen
        let extra = rows_box();
        let pick = vbox(8);
        pick.add_css_class("mondrian-pad");
        pick.append(&tabs_line);
        pick.append(&grad_area);
        pick.append(&pattern_area);
        pick.append(&picker.root);
        pick.append(&grid);
        let detail = rows_box();
        detail.append(&pick);
        detail.append(&extra);
        detail.set_visible(false);
        // ohne Verlauf (Textrahmen) bzw. nur Deckkraft (Bilder)
        tab_row.set_visible(matches!(target, SwTarget::Paint(_)));
        if target == SwTarget::ImageOpacity {
            pipette_btn.set_visible(false);
            picker.set_alpha_only(true);
            grid.set_visible(false);
            chip.set_visible(false);
        }
        let expanded = Rc::new(Cell::new(false));
        let fold = fold_register(&detail, &arrow, &expanded);
        root.append(&head);
        root.append(&detail);
        root.add_css_class("mondrian-fold");
        let sw = Rc::new(Swatches {
            target,
            hint,
            extra,
            fold: Cell::new(fold),
            ui: RefCell::new(std::rc::Weak::new()),
            root,
            title,
            chip,
            arrow,
            expanded,
            tabs: (solid, grad_tab),
            pattern_tab,
            grad_area,
            shape_btns: (lin_btn, rad_btn),
            pattern_area,
            pattern_btns,
            pattern_chips,
            pattern_cell,
            pattern_angle,
            shape: Cell::new(crate::model::GradShape::Linear),
            bar,
            angle,
            picker,
            grid,
            current: Cell::new(None),
            grad: RefCell::new(None),
            stop: Cell::new(0),
            shown: Cell::new(None),
            lib: RefCell::new(Vec::new()),
            key: RefCell::new(String::new()),
            syncing: Cell::new(false),
            pipette_btn,
        });
        // Pipette ein- und ausschalten
        {
            let w = Rc::downgrade(&sw);
            sw.pipette_btn.connect_toggled(move |b| {
                let Some(sw) = w.upgrade() else { return };
                let Some(u) = sw.ui.borrow().upgrade() else { return };
                if b.is_active() {
                    // nur eine Pipette zugleich
                    let old = u.pipette.borrow_mut().replace(Rc::downgrade(&sw));
                    if let Some(o) = old.and_then(|o| o.upgrade()) {
                        if !Rc::ptr_eq(&o, &sw) {
                            o.pipette_btn.set_active(false);
                        }
                    }
                    if let Some(d) = cur(&u) {
                        d.canvas.set_cursor_from_name(Some("crosshair"));
                    }
                } else {
                    let mine = u
                        .pipette
                        .borrow()
                        .as_ref()
                        .and_then(|o| o.upgrade())
                        .is_some_and(|o| Rc::ptr_eq(&o, &sw));
                    if mine {
                        end_pipette(&u);
                    }
                }
            });
        }
        // Farbwahl: einfarbig oder den gewählten Farbstopp sofort ändern
        {
            let w = Rc::downgrade(&sw);
            *sw.picker.on_change.borrow_mut() = Some(Rc::new(move |c: Color| {
                let Some(sw) = w.upgrade() else { return };
                sw.shown.set(Some(c));
                let t = sw.target;
                if sw.grad.borrow().is_some() {
                    let i = sw.stop.get();
                    sw.edit(move |e, _| {
                        e.set_stop_color(i, c);
                    });
                } else {
                    sw.edit(move |e, m| apply_paint(e, m, t, Some(c), None));
                }
            }));
        }
        // nur die Deckkraft: jedes Objekt behält seine Farbe
        {
            let w = Rc::downgrade(&sw);
            *sw.picker.on_alpha.borrow_mut() = Some(Rc::new(move |a: f64| {
                let Some(sw) = w.upgrade() else { return };
                let t = sw.target;
                if sw.grad.borrow().is_some() {
                    // Verlauf: Deckkraft des gewählten Farbstopps
                    let c = sw.picker.color();
                    sw.shown.set(Some(c));
                    let i = sw.stop.get();
                    sw.edit(move |e, _| {
                        e.set_stop_color(i, c);
                    });
                    return;
                }
                sw.edit(move |e, m| apply_alpha(e, m, t, a));
            }));
        }
        // Reiter
        {
            use crate::model::GradShape;
            // 0 = Farbe, 1 = Verlauf, 2 = Muster
            for (b, mode) in [(&sw.tabs.0, 0), (&sw.tabs.1, 1), (&sw.pattern_tab, 2)] {
                let w = Rc::downgrade(&sw);
                b.connect_toggled(move |b| {
                    let Some(sw) = w.upgrade() else { return };
                    if !b.is_active() || sw.syncing.get() {
                        return;
                    }
                    let cur = sw.shape.get();
                    sw.edit_later(move |e, _| match mode {
                        0 => {
                            e.remove_gradient();
                        }
                        1 => {
                            let to = if cur == GradShape::Radial {
                                GradShape::Radial
                            } else {
                                GradShape::Linear
                            };
                            e.set_gradient_shape(to);
                        }
                        _ => {
                            let to = if cur.is_pattern() { cur } else { GradShape::Checker };
                            e.set_gradient_shape(to);
                        }
                    });
                });
            }
            for (b, shp) in [
                (&sw.shape_btns.0, GradShape::Linear),
                (&sw.shape_btns.1, GradShape::Radial),
            ] {
                let w = Rc::downgrade(&sw);
                b.connect_toggled(move |b| {
                    let Some(sw) = w.upgrade() else { return };
                    if !b.is_active() || sw.syncing.get() {
                        return;
                    }
                    sw.edit_later(move |e, _| {
                        e.set_gradient_shape(shp);
                    });
                });
            }
            for (shp, b) in &sw.pattern_btns {
                let shp = *shp;
                let w = Rc::downgrade(&sw);
                b.connect_toggled(move |b| {
                    let Some(sw) = w.upgrade() else { return };
                    if !b.is_active() || sw.syncing.get() {
                        return;
                    }
                    sw.edit_later(move |e, _| {
                        e.set_gradient_shape(shp);
                    });
                });
                // Vorschau mit den aktuellen Farben
                if let Some(da) = b.child().and_downcast::<gtk::DrawingArea>() {
                    let w = Rc::downgrade(&sw);
                    da.set_draw_func(move |_, cr, cw, ch| {
                        let Some(sw) = w.upgrade() else { return };
                        let g = sw.grad.borrow().clone();
                        let c1 = g
                            .as_ref()
                            .and_then(|g| g.stops.first().map(|s| s.color))
                            .unwrap_or(Color::WHITE);
                        let c2 = g
                            .as_ref()
                            .and_then(|g| g.stops.get(1).map(|s| s.color))
                            .unwrap_or(Color::BLACK);
                        draw_pattern_preview(cr, cw as f64, ch as f64, shp, c1, c2);
                    });
                }
            }
            for (i, chip) in sw.pattern_chips.iter().enumerate() {
                let w = Rc::downgrade(&sw);
                chip.set_draw_func(move |_, cr, cw, ch| {
                    let Some(sw) = w.upgrade() else { return };
                    let c = sw
                        .grad
                        .borrow()
                        .as_ref()
                        .and_then(|g| g.stops.get(i).map(|s| s.color));
                    draw_swatch(cr, cw as f64, ch as f64, c, None, sw.stop.get() == i);
                });
                let w = Rc::downgrade(&sw);
                let click = gtk::GestureClick::new();
                click.connect_released(move |_, _, _, _| {
                    let Some(sw) = w.upgrade() else { return };
                    sw.stop.set(i);
                    let col = sw
                        .grad
                        .borrow()
                        .as_ref()
                        .and_then(|g| g.stops.get(i).map(|s| s.color));
                    if let Some(c) = col {
                        sw.shown.set(Some(c));
                        sw.picker.show(c);
                    }
                    for ch in &sw.pattern_chips {
                        ch.queue_draw();
                    }
                    sw.edit(move |e, _| e.grad_stop = i);
                });
                chip.add_controller(click);
            }
            {
                let w = Rc::downgrade(&sw);
                sw.pattern_cell.connect_value_changed(move |sp| {
                    let Some(sw) = w.upgrade() else { return };
                    if sw.syncing.get() {
                        return;
                    }
                    let v = sp.value();
                    sw.edit(move |e, _| {
                        e.set_pattern_cell(v);
                    });
                });
            }
            {
                let w = Rc::downgrade(&sw);
                sw.pattern_angle.connect_value_changed(move |sp| {
                    let Some(sw) = w.upgrade() else { return };
                    if sw.syncing.get() {
                        return;
                    }
                    let v = sp.value();
                    sw.edit(move |e, m| {
                        e.set_gradient_angle(v, m);
                    });
                });
            }
        }
        // Winkel
        {
            let w = Rc::downgrade(&sw);
            sw.angle.connect_value_changed(move |s| {
                let Some(sw) = w.upgrade() else { return };
                if sw.syncing.get() {
                    return;
                }
                let v = s.value();
                sw.edit(move |e, m| {
                    e.set_gradient_angle(v, m);
                });
            });
        }
        Swatches::setup_bar(&sw);
        // Kopfzeile: Farbkreis zeigt die aktuelle Farbe; Klick oder Pfeil klappt auf
        {
            let w = Rc::downgrade(&sw);
            sw.chip.set_draw_func(move |_, cr, cw, ch| {
                let Some(sw) = w.upgrade() else { return };
                let g = sw.grad.borrow().clone();
                let (cw, ch) = (cw as f64, ch as f64);
                match g.as_ref().filter(|g| g.shape.is_pattern()) {
                    // Muster: rund ausgeschnitten
                    Some(g) => {
                        cr.arc(
                            cw / 2.0,
                            ch / 2.0,
                            cw.min(ch) / 2.0 - 1.0,
                            0.0,
                            std::f64::consts::TAU,
                        );
                        cr.clip();
                        let c1 = g.stops.first().map(|s| s.color).unwrap_or(Color::WHITE);
                        let c2 = g.stops.get(1).map(|s| s.color).unwrap_or(Color::BLACK);
                        draw_pattern_preview(cr, cw, ch, g.shape, c1, c2);
                    }
                    None => draw_swatch(cr, cw, ch, sw.current.get(), g.as_ref(), false),
                }
            });
        }
        {
            let w = Rc::downgrade(&sw);
            let click = gtk::GestureClick::new();
            click.connect_released(move |_, _, _, _| {
                if let Some(sw) = w.upgrade() {
                    fold_set(sw.fold.get(), !sw.expanded.get());
                }
            });
            sw.chip.add_controller(click);
        }
        {
            let w = Rc::downgrade(&sw);
            sw.arrow.connect_clicked(move |_| {
                if let Some(sw) = w.upgrade() {
                    fold_set(sw.fold.get(), !sw.expanded.get());
                }
            });
        }
        sw
    }

    /// Änderung am Editor für diese Farbe (Füllung, Kontur oder Schatten).
    fn edit(&self, f: impl FnOnce(&mut Editor, &CairoMetrics)) {
        let Some(u) = self.ui.borrow().upgrade() else {
            return;
        };
        let t = self.target;
        with_ed_m(&u, |e, m| {
            if let SwTarget::Paint(t) = t {
                e.grad_target = t;
            }
            f(e, m);
        });
    }

    /// Wie `edit`, aber erst nach dem laufenden Ereignis (die Kreise werden evtl. neu aufgebaut).
    fn edit_later(&self, f: impl FnOnce(&mut Editor, &CairoMetrics) + 'static) {
        let Some(u) = self.ui.borrow().upgrade() else {
            return;
        };
        let t = self.target;
        glib::idle_add_local_once(move || {
            with_ed_m(&u, |e, m| {
                if let SwTarget::Paint(t) = t {
                    e.grad_target = t;
                }
                f(e, m);
            });
        });
    }

    /// Lage eines Farbstopps auf der Leiste (Leiste 18 px hoch, 4 px Rand rundum).
    fn stop_x(w: f64, _h: f64, pos: f64) -> f64 {
        13.0 + pos.clamp(0.0, 1.0) * (w - 26.0).max(1.0)
    }

    fn setup_bar(sw: &Rc<Swatches>) {
        {
            let w = Rc::downgrade(sw);
            sw.bar.set_draw_func(move |_, cr, w_, h_| {
                let Some(sw) = w.upgrade() else { return };
                let Some(g) = sw.grad.borrow().clone() else { return };
                let (w_, h_) = (w_ as f64, h_ as f64);
                let cy = h_ / 2.0;
                let _ = cr.save();
                round_rect(cr, 4.0, cy - 9.0, w_ - 8.0, 18.0, 9.0);
                cr.clip();
                checker(cr, w_, h_, 5.0);
                let lg = gtk::cairo::LinearGradient::new(13.0, 0.0, w_ - 13.0, 0.0);
                for (pos, c) in g.render_stops() {
                    lg.add_color_stop_rgba(pos.clamp(0.0, 1.0), c.r, c.g, c.b, c.a);
                }
                let _ = cr.set_source(&lg);
                let _ = cr.paint();
                let _ = cr.restore();
                let sel = sw.stop.get();
                for (i, st) in g.stops.iter().enumerate() {
                    let x = Swatches::stop_x(w_, h_, st.pos);
                    if i == sel {
                        cr.new_path();
                        cr.arc(x, cy, 11.0, 0.0, std::f64::consts::TAU);
                        let a = render::ui_accent();
                        cr.set_source_rgb(a.0, a.1, a.2);
                        cr.set_line_width(2.0);
                        let _ = cr.stroke();
                    }
                    picker_knob(cr, x, cy, Some((st.color.r, st.color.g, st.color.b)));
                }
            });
        }
        // Klicken, Ziehen: Farbstopp wählen/verschieben oder neuen einfügen
        let drag = gtk::GestureDrag::new();
        let dragging: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));
        {
            let (w, dr) = (Rc::downgrade(sw), dragging.clone());
            drag.connect_drag_begin(move |_, x, _| {
                let Some(sw) = w.upgrade() else { return };
                sw.bar.grab_focus();
                let Some(g) = sw.grad.borrow().clone() else { return };
                let (bw, bh) = (sw.bar.width() as f64, sw.bar.height() as f64);
                let near = g
                    .stops
                    .iter()
                    .enumerate()
                    .map(|(i, st)| (i, (Swatches::stop_x(bw, bh, st.pos) - x).abs()))
                    .filter(|(_, d)| *d <= 9.0)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(i, _)| i);
                match near {
                    Some(i) => {
                        dr.set(Some(i));
                        sw.stop.set(i);
                        if let Some(st) = g.stops.get(i) {
                            sw.shown.set(Some(st.color));
                            sw.picker.show(st.color);
                        }
                        sw.bar.queue_draw();
                        sw.edit(move |e, _| e.grad_stop = i);
                    }
                    None => {
                        let _ = bh;
                        let t = ((x - 13.0) / (bw - 26.0).max(1.0)).clamp(0.0, 1.0);
                        let res: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));
                        let r2 = res.clone();
                        sw.edit(move |e, _| {
                            if e.add_stop(t) {
                                r2.set(Some(e.grad_stop));
                            }
                        });
                        dr.set(res.get());
                    }
                }
            });
        }
        {
            let (w, dr) = (Rc::downgrade(sw), dragging.clone());
            drag.connect_drag_update(move |g, dx, _| {
                let Some(sw) = w.upgrade() else { return };
                let (Some(i), Some((sx, _))) = (dr.get(), g.start_point()) else {
                    return;
                };
                let bw = sw.bar.width() as f64;
                let t = ((sx + dx - 13.0) / (bw - 26.0).max(1.0)).clamp(0.0, 1.0);
                sw.edit(move |e, _| {
                    e.set_stop_pos(i, t);
                });
            });
        }
        {
            let dr = dragging.clone();
            drag.connect_drag_end(move |_, _, _| dr.set(None));
        }
        sw.bar.add_controller(drag);
        // Entf entfernt den gewählten Farbstopp
        let keys = gtk::EventControllerKey::new();
        {
            let w = Rc::downgrade(sw);
            keys.connect_key_pressed(move |_, key, _, _| {
                let Some(sw) = w.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if matches!(key, gdk::Key::Delete | gdk::Key::BackSpace | gdk::Key::KP_Delete) {
                    let i = sw.stop.get();
                    sw.edit(move |e, _| {
                        e.grad_stop = i;
                        e.remove_stop();
                    });
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        sw.bar.add_controller(keys);
    }

    fn redraw_cells(&self) {
        let mut c = self.grid.first_child();
        while let Some(w) = c {
            w.queue_draw();
            c = w.next_sibling();
        }
    }

    /// Passt ein Bibliothekseintrag zur Farbe des Objekts?
    fn matches(&self, c: Color, g: Option<&Gradient>) -> bool {
        let cur = self.current.get();
        let grad = self.grad.borrow();
        match (g, grad.as_ref()) {
            (Some(a), Some(b)) => a.same_colors(b),
            (None, None) => cur.map(|x| same_rgba(x, c)).unwrap_or(false),
            _ => false,
        }
    }

    /// Ist die Farbe des Objekts eine eigene (nicht aus der Bibliothek)?
    fn has_custom(&self) -> bool {
        self.current.get().is_some()
            && !self
                .lib
                .borrow()
                .iter()
                .any(|(c, g)| self.matches(*c, g.as_ref()))
    }

    /// Zustand vom Objekt übernehmen: Farbe, Verlauf, gewählter Farbstopp, Winkel und der
    /// Hinweis für die Kopfzeile.
    fn sync(&self, current: Option<Color>, g: Option<Gradient>, stop: usize, angle: f64, hint: &str) {
        self.current.set(current);
        self.hint.set_label(hint);
        let stop = g
            .as_ref()
            .map(|g| stop.min(g.stops.len().saturating_sub(1)))
            .unwrap_or(0);
        self.stop.set(stop);
        let edit = match &g {
            Some(g) => g.stops.get(stop).map(|s| s.color),
            None => current,
        };
        let has_grad = g.is_some();
        let shape = g.as_ref().map(|g| g.shape).unwrap_or_default();
        let cell = g.as_ref().map(|g| g.cell).unwrap_or(crate::model::PATTERN_CELL);
        let pattern = has_grad && shape.is_pattern();
        self.shape.set(shape);
        *self.grad.borrow_mut() = g;
        self.syncing.set(true);
        if pattern {
            self.pattern_tab.set_active(true);
        } else if has_grad {
            self.tabs.1.set_active(true);
        } else {
            self.tabs.0.set_active(true);
        }
        if shape == crate::model::GradShape::Radial {
            self.shape_btns.1.set_active(true);
        } else {
            self.shape_btns.0.set_active(true);
        }
        for (shp, b) in &self.pattern_btns {
            if *shp == shape {
                b.set_active(true);
            }
            if let Some(c) = b.child() {
                c.queue_draw();
            }
        }
        for ch in &self.pattern_chips {
            ch.queue_draw();
        }
        if (self.pattern_cell.value() - cell).abs() > 0.01 {
            self.pattern_cell.set_value(cell);
        }
        if (self.angle.value() - angle.round()).abs() > 0.5 {
            self.angle.set_value(angle.round());
        }
        if (self.pattern_angle.value() - angle.round()).abs() > 0.5 {
            self.pattern_angle.set_value(angle.round());
        }
        self.syncing.set(false);
        self.grad_area.set_visible(has_grad && !pattern);
        self.pattern_area.set_visible(pattern);
        // kreisförmig: der Winkel spielt keine Rolle
        if let Some(r) = self.angle.parent() {
            r.set_visible(shape != crate::model::GradShape::Radial);
        }
        if let Some(c) = edit {
            if self.shown.get().map(|s| !same_rgba(s, c)).unwrap_or(true) {
                self.shown.set(Some(c));
                self.picker.show(c);
            }
        }
        self.bar.queue_draw();
        self.chip.queue_draw();
        self.redraw_cells();
    }

    /// Kreise neu aufbauen, wenn sich Farben, die eigene Farbe oder die Breite geändert haben.
    fn update(self: &Rc<Self>, colors: &[(Color, Option<Gradient>, String)], cols: i32) {
        *self.lib.borrow_mut() = colors.iter().map(|(c, g, _)| (*c, g.clone())).collect();
        let custom = self.has_custom();
        let key = format!("{colors:?}|{custom}|{cols}");
        if *self.key.borrow() == key {
            self.redraw_cells();
            return;
        }
        *self.key.borrow_mut() = key;
        while let Some(c) = self.grid.first_child() {
            self.grid.remove(&c);
        }
        let mut cells: Vec<gtk::Widget> = Vec::new();
        let mut items: Vec<(SwItem, String)> = vec![(SwItem::None, tr("Keine").into())];
        items.extend(
            colors
                .iter()
                .map(|(c, g, n)| (SwItem::Lib(*c, g.clone()), n.clone())),
        );
        if custom {
            items.push((SwItem::Custom, String::new()));
        }
        for (item, name) in items {
            let area = gtk::DrawingArea::new();
            area.set_content_width(SWATCH);
            area.set_content_height(SWATCH);
            area.set_cursor_from_name(Some("pointer"));
            let tip = match &item {
                SwItem::None => tr("Keine").to_string(),
                SwItem::Lib(_, Some(_)) if name.is_empty() => tr("Verlauf").to_string(),
                SwItem::Lib(col, None) if name.is_empty() => col.to_hex(),
                SwItem::Lib(col, _) => format!("{name} · {}", col.to_hex()),
                SwItem::Custom => tr("Eigene Farbe – „+“ nimmt sie in die Bibliothek auf").to_string(),
            };
            area.set_tooltip_text(Some(&tip));
            {
                let w = Rc::downgrade(self);
                let item = item.clone();
                area.set_draw_func(move |_, cr, aw, ah| {
                    let Some(sw) = w.upgrade() else { return };
                    let (aw, ah) = (aw as f64, ah as f64);
                    match &item {
                        SwItem::None => draw_swatch(cr, aw, ah, None, None, sw.current.get().is_none()),
                        SwItem::Lib(c, g) => {
                            let sel = sw.matches(*c, g.as_ref());
                            draw_swatch(cr, aw, ah, Some(*c), g.as_ref(), sel)
                        }
                        SwItem::Custom => {
                            let g = sw.grad.borrow().clone();
                            draw_swatch(cr, aw, ah, sw.current.get(), g.as_ref(), true)
                        }
                    }
                });
            }
            let click = gtk::GestureClick::new();
            {
                let w = Rc::downgrade(self);
                click.connect_released(move |_, _, _, _| {
                    let Some(sw) = w.upgrade() else { return };
                    let t = sw.target;
                    match &item {
                        SwItem::None => sw.edit_later(move |e, m| apply_paint(e, m, t, None, None)),
                        // Verlauf in Arbeit und eine einfache Farbe gewählt: nur der gewählte
                        // Farbstopp bekommt die Farbe
                        SwItem::Lib(c, None) if sw.grad.borrow().is_some() => {
                            let (c, i) = (*c, sw.stop.get());
                            sw.shown.set(Some(c));
                            sw.picker.show(c);
                            sw.edit_later(move |e, _| {
                                e.set_stop_color(i, c);
                            });
                        }
                        SwItem::Lib(c, g) => {
                            // die Farbwahl springt mit
                            let show = g
                                .as_ref()
                                .and_then(|g| g.stops.first().map(|s| s.color))
                                .unwrap_or(*c);
                            sw.shown.set(Some(show));
                            sw.picker.show(show);
                            let (c, g) = (*c, g.clone());
                            sw.edit_later(move |e, m| apply_paint(e, m, t, Some(c), g));
                        }
                        SwItem::Custom => {}
                    }
                });
            }
            area.add_controller(click);
            cells.push(area.upcast());
        }
        // „+“: aktuelle Farbe (oder Verlauf) in die Bibliothek aufnehmen
        let add = gtk::Button::from_icon_name("list-add-symbolic");
        add.add_css_class("mondrian-swatch-add");
        add.set_tooltip_text(Some(tr("Aktuelle Farbe zur Bibliothek hinzufügen")));
        add.set_size_request(SWATCH, SWATCH);
        add.set_valign(gtk::Align::Center);
        add.set_halign(gtk::Align::Center);
        {
            let w = Rc::downgrade(self);
            add.connect_clicked(move |_| {
                let Some(sw) = w.upgrade() else { return };
                let Some(u) = sw.ui.borrow().upgrade() else { return };
                let Some(c) = sw.current.get() else {
                    u.toast(tr("Keine Farbe gewählt"));
                    return;
                };
                let g = sw.grad.borrow().clone();
                lib_add_paint_dialog(&u, c, g);
            });
        }
        cells.push(add.upcast());
        let n = cols.max(1);
        for (i, w) in cells.iter().enumerate() {
            let i = i as i32;
            self.grid.attach(w, i % n, i / n, 1, 1);
        }
    }
}

/// Farbe oder Verlauf in eine Bibliothek aufnehmen: Bibliothek wählen, optional benennen.
fn lib_add_paint_dialog(ui: &Rc<Ui>, c: Color, g: Option<Gradient>) {
    lib_paint_dialog(ui, c, g, false);
}

/// Neue Farbe für die Bibliothek mit derselben Farbwahl wie in den Eigenschaften.
fn lib_new_color_dialog(ui: &Rc<Ui>, c: Color) {
    lib_paint_dialog(ui, c, None, true);
}

/// Farbe/Verlauf in eine Bibliothek aufnehmen; mit `editable` lässt sich die Farbe vorher
/// in der eingebetteten Farbwahl einstellen.
fn lib_paint_dialog(ui: &Rc<Ui>, c: Color, g: Option<Gradient>, editable: bool) {
    // nur eigene Bibliotheken (die Standardbibliothek ist nicht änderbar)
    let own: Vec<(usize, String)> = ui
        .libraries
        .borrow()
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.locked)
        .map(|(i, l)| (i, l.name.clone()))
        .collect();
    let mut names: Vec<String> = own.iter().map(|(_, n)| n.clone()).collect();
    if names.is_empty() {
        names.push(tr("Meine Bibliothek (neu)").into());
    }
    let refs: Vec<&str> = names.iter().map(|n| n.as_str()).collect();
    let dd = gtk::DropDown::from_strings(&refs);
    dd.set_valign(gtk::Align::Center);
    if let Some(p) = own.iter().position(|(i, _)| *i == ui.lib_current.get()) {
        dd.set_selected(p as u32);
    }
    let entry = gtk::Entry::builder()
        .placeholder_text(tr("Name (freiwillig)"))
        .activates_default(true)
        .build();
    let preview = gtk::DrawingArea::new();
    preview.set_content_width(44);
    preview.set_content_height(44);
    preview.set_halign(gtk::Align::Center);
    {
        let g = g.clone();
        preview.set_draw_func(move |_, cr, w, h| {
            draw_swatch(cr, w as f64, h as f64, Some(c), g.as_ref(), false);
        });
    }
    let form = vbox(10);
    let picker = editable.then(ColorPicker::new);
    match &picker {
        Some(p) => {
            p.show(c);
            p.root.set_size_request(260, -1);
            form.append(&p.root);
        }
        None => form.append(&preview),
    }
    form.append(&form_row(tr("Bibliothek"), &dd));
    form.append(&entry);
    let what = if g.is_some() { tr("Verlauf") } else { tr("Farbe") };
    let dlg = adw::AlertDialog::new(
        Some(&if editable {
            tr("Neue Farbe für die Bibliothek").to_string()
        } else {
            trf("{} zur Bibliothek hinzufügen", &[&(what)])
        }),
        None,
    );
    dlg.set_extra_child(Some(&form));
    dlg.add_response("cancel", tr("_Abbrechen"));
    dlg.add_response("add", tr("_Hinzufügen"));
    dlg.set_response_appearance("add", adw::ResponseAppearance::Suggested);
    dlg.set_default_response(Some("add"));
    dlg.set_close_response("cancel");
    let u = ui.clone();
    dlg.connect_response(None, move |_, r| {
        if r != "add" {
            return;
        }
        if let Some((idx, _)) = own.get(dd.selected() as usize) {
            if *idx != u.lib_current.get() {
                u.lib_current.set(*idx);
                save_settings(&u);
                rebuild_library(&u);
            }
        }
        let name = entry.text().trim().to_string();
        let g = g.clone();
        let c = picker.as_ref().map(|p| p.color()).unwrap_or(c);
        let added = with_library_add(&u, move |l| {
            let added = l.add_paint(c, g);
            if added && !name.is_empty() {
                let i = l.colors.len() - 1;
                l.set_color_name(i, &name);
            }
            added
        });
        lib_added(&u, added, what);
    });
    label_icon_buttons(dlg.upcast_ref());
    dlg.present(Some(&ui.window));
}

/// Farbe setzen – je nachdem, wofür die Farbwahl gilt.
fn apply_paint(e: &mut Editor, m: &CairoMetrics, t: SwTarget, c: Option<Color>, g: Option<Gradient>) {
    match t {
        SwTarget::Paint(p) => {
            e.set_paint(p, c, g);
        }
        SwTarget::FrameFill => {
            e.set_text_frame("frame-fill", move |f| f.fill = c, m);
        }
        SwTarget::FrameStroke => {
            e.set_text_frame(
                "frame-stroke",
                move |f| {
                    f.stroke = c;
                    if f.width <= 0.0 {
                        f.width = 1.0;
                    }
                },
                m,
            );
        }
        SwTarget::ImageOpacity => {
            if let Some(c) = c {
                set_image_opacity(e, c.a);
            }
        }
    }
}

/// Nur die Deckkraft ändern (Farben der einzelnen Objekte bleiben).
fn apply_alpha(e: &mut Editor, m: &CairoMetrics, t: SwTarget, a: f64) {
    match t {
        SwTarget::Paint(p) => {
            e.set_paint_alpha(p, a);
        }
        SwTarget::FrameFill => {
            e.set_text_frame(
                "frame-fill-alpha",
                move |f| {
                    if let Some(c) = f.fill.as_mut() {
                        c.a = a;
                    }
                },
                m,
            );
        }
        SwTarget::FrameStroke => {
            e.set_text_frame(
                "frame-stroke-alpha",
                move |f| {
                    if let Some(c) = f.stroke.as_mut() {
                        c.a = a;
                    }
                },
                m,
            );
        }
        SwTarget::ImageOpacity => {
            set_image_opacity(e, a);
        }
    }
}

/// Deckkraft nur der ausgewählten Bilder (nicht als Vorgabe für neue Formen merken – sonst
/// wären alle neuen Formen unsichtbar halbdurchsichtig).
fn set_image_opacity(e: &mut Editor, a: f64) {
    e.update_objects(
        "opacity",
        |o| matches!(o.shape, crate::model::Shape::Image { .. }),
        move |s| s.opacity = a,
    );
}

/// Farbkreise der Seitenleiste mit der gewählten Bibliothek und der Leistenbreite abgleichen.
fn update_swatches(ui: &Rc<Ui>) {
    let colors: Vec<(Color, Option<Gradient>, String)> = {
        let libs = ui.libraries.borrow();
        libs.get(ui.lib_current.get())
            .map(|l| {
                l.colors
                    .iter()
                    .enumerate()
                    .map(|(i, c)| (*c, l.gradient(i).cloned(), l.color_name(i).to_string()))
                    .collect()
            })
            .unwrap_or_default()
    };
    let width = props_panel_width(ui);
    // Ränder der Eigenschaften abziehen
    let avail = (width - 24 - 12).max(SWATCH);
    let cols = ((avail + 6) / (SWATCH + 6)).max(1);
    for sw in [
        &ui.panel.fill_sw,
        &ui.panel.stroke_sw,
        &ui.panel.shadow_sw,
        &ui.panel.frame_fill_sw,
        &ui.panel.frame_stroke_sw,
        &ui.panel.image_sw,
    ] {
        sw.update(&colors, cols);
    }
}

/// Menü als Popover an der Zeichenfläche öffnen – das normale GNOME-Menü (GtkPopoverMenu):
/// gleiche Abstände wie in anderen Programmen, Pfeiltasten und Zugriffstasten. Untermenüs
/// erscheinen als eigene Popover daneben. Das Popover wird bei jedem Öffnen neu gebaut und
/// danach entfernt.
fn popup_menu(d: &DocTab, menu: &gio::Menu, rect: gdk::Rectangle, class: Option<&str>) {
    let pop = gtk::PopoverMenu::from_model_full(menu, gtk::PopoverMenuFlags::NESTED);
    if let Some(c) = class {
        pop.add_css_class(c);
    }
    pop.set_parent(&d.canvas);
    pop.set_has_arrow(false);
    pop.set_halign(gtk::Align::Start);
    pop.set_pointing_to(Some(&rect));
    pop.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    pop.popup();
}

fn is_image(o: &crate::model::Object) -> bool {
    matches!(o.shape, Shape::Image { .. })
}

fn not_image(o: &crate::model::Object) -> bool {
    !is_image(o)
}

/// Effektmenü der Schwebeleiste: wirkt auf alle ausgewählten Objekte.
fn show_effect_menu(d: &DocTab, rect: gdk::Rectangle) {
    let item = |label: &str, action: &str, target: &str| {
        let it = gio::MenuItem::new(Some(label), None);
        it.set_action_and_target_value(Some(action), Some(&target.to_variant()));
        it
    };
    // je Kategorie eine Unterseite; aus jeder Kategorie ist ein Effekt möglich
    let menu = gio::Menu::new();
    let color = gio::Menu::new();
    for b in &Blend::SIMPLE[1..] {
        color.append_item(&item(b.label(), "win.effect-blend", b.css()));
    }
    menu.append_submenu(Some(tr("Farbüberlagerung")), &color);
    let tex = gio::Menu::new();
    for t in crate::model::Texture::ALL {
        tex.append_item(&item(t.label(), "win.effect-tex", t.name()));
    }
    menu.append_submenu(Some(tr("Textur")), &tex);
    let (any_image, any_other) = d.ed.borrow().selection_image_kinds();
    let art = gio::Menu::new();
    for a in crate::model::Art::ALL {
        art.append_item(&item(a.label(), "win.effect-art", a.name()));
    }
    menu.append_submenu(Some(tr("Kunsteffekte")), &art);
    let lens = gio::Menu::new();
    for l in crate::model::Lens::ALL {
        lens.append_item(&item(l.label(), "win.effect-lens", l.name()));
    }
    if any_other {
        menu.append_submenu(Some(tr("3D-Effekte")), &lens);
    }
    if any_image {
        let tint = gio::Menu::new();
        for f in crate::model::ImgFilter::ALL {
            tint.append_item(&item(f.label(), "win.effect-tint", f.name()));
        }
        menu.append_submenu(Some(tr("Bildfilter")), &tint);
    }
    let extras = gio::Menu::new();
    extras.append(Some(tr("Schatten")), Some("win.effect-shadow"));
    extras.append(Some(tr("Spiegelung")), Some("win.effect-mirror"));
    menu.append_submenu(Some(tr("Extras")), &extras);
    popup_menu(d, &menu, rect, Some("mondrian-fxmenu"));
}

/// Schwebende Leiste knapp über oder unter der Auswahl.
struct FloatBar {
    bar: gtk::Box,
    group: gtk::Button,
    ungroup: gtk::Button,
    sep: gtk::Separator,
    lock: gtk::Button,
    /// Effekte für alle ausgewählten Objekte
    effects: gtk::Button,
    /// Bild in Vektorflächen umwandeln (nur bei einem Bild mit flachen Farbflächen)
    trace: gtk::MenuButton,
    trace_colors: gtk::SpinButton,
    trace_make: gtk::Button,
    /// mehrere Objekte aneinander ausrichten, ab drei auch gleichmäßig verteilen
    arrange: gtk::MenuButton,
    /// Menü des Knopfs: nur Ausrichten bzw. Ausrichten und Verteilen
    arrange_menus: (gio::Menu, gio::Menu),
    /// Pfadoperationen (ab zwei Objekten)
    bools: gtk::Box,
    /// weitere Möglichkeiten (dasselbe Menü wie beim Rechtsklick)
    more: gtk::Button,
    /// zuletzt gesetzte Lage (x, y) und Zustand der Knöpfe
    shown: Cell<Option<(i32, i32, u8)>>,
    /// Aktualisierung ist bereits vorgemerkt
    pending: Cell<bool>,
}

fn float_button(icon: &str, tip: &str, action: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.set_tooltip_text(Some(tip));
    b.set_action_name(Some(action));
    b.add_css_class("flat");
    b.set_focusable(false);
    b
}

fn float_label_button(icon: &str, label: &str, tip: &str, action: &str) -> gtk::Button {
    let content = hbox(6);
    content.append(&gtk::Image::from_icon_name(icon));
    content.append(&gtk::Label::new(Some(label)));
    let b = gtk::Button::new();
    b.set_child(Some(&content));
    b.set_tooltip_text(Some(tip));
    b.set_action_name(Some(action));
    b.add_css_class("flat");
    b.set_focusable(false);
    b
}

fn build_float_bar() -> FloatBar {
    let bar = hbox(0);
    bar.add_css_class("mondrian-float");
    bar.set_halign(gtk::Align::Start);
    bar.set_valign(gtk::Align::Start);
    bar.set_visible(false);
    let group = float_label_button(
        "mondrian-group-symbolic",
        tr("Gruppieren"),
        &key_tip(tr("Gruppieren"), "group"),
        "win.group",
    );
    let ungroup = float_label_button(
        "mondrian-ungroup-symbolic",
        tr("Gruppierung auflösen"),
        &key_tip(tr("Gruppierung auflösen"), "ungroup"),
        "win.ungroup",
    );
    let sep = gtk::Separator::new(gtk::Orientation::Vertical);
    sep.set_margin_top(6);
    sep.set_margin_bottom(6);
    let lock = float_button("changes-allow-symbolic", tr("Sperren"), "win.lock-toggle");
    // „fx“ als Schrift (wie in vielen Grafikprogrammen) statt eines Symbols
    let effects = gtk::Button::with_label("fx");
    effects.set_tooltip_text(Some(tr("Effekte – für alle ausgewählten Objekte")));
    effects.add_css_class("flat");
    effects.add_css_class("mondrian-fx");
    effects.update_property(&[gtk::accessible::Property::Label(tr("Effekte"))]);
    effects.set_focusable(false);
    // Knopf mit Symbol und Text, der ein Fenster öffnet
    let label_menu_button = |icon: &str, label: &str, tip: &str| {
        let content = hbox(6);
        content.append(&gtk::Image::from_icon_name(icon));
        content.append(&gtk::Label::new(Some(label)));
        let b = gtk::MenuButton::new();
        b.set_child(Some(&content));
        b.set_has_frame(false);
        b.set_focusable(false);
        b.set_tooltip_text(Some(tip));
        b.set_visible(false);
        b
    };
    let trace = label_menu_button(
        "insert-image-symbolic",
        tr("Nachzeichnen"),
        tr("Bild in bearbeitbare Vektorflächen umwandeln"),
    );
    let trace_colors = spin(2.0, 24.0, 1.0, 0, tr("Anzahl der Farben im Bild"));
    trace_colors.set_value(8.0);
    let trace_make = gtk::Button::with_label(tr("Nachzeichnen"));
    trace_make.add_css_class("suggested-action");
    let trace_box = vbox(10);
    trace_box.set_margin_top(10);
    trace_box.set_margin_bottom(10);
    trace_box.set_margin_start(10);
    trace_box.set_margin_end(10);
    trace_box.append(&row(tr("Farben"), &trace_colors));
    trace_box.append(&trace_make);
    let trace_pop = gtk::Popover::new();
    trace_pop.set_child(Some(&trace_box));
    trace.set_popover(Some(&trace_pop));
    // Ausrichten aneinander (ab zwei Objekten), Verteilen (ab drei)
    let arrange = label_menu_button(
        "mondrian-page-hcenter-symbolic",
        tr("Ausrichten"),
        tr("Objekte aneinander ausrichten"),
    );
    let align_section = |items: [(&str, &str); 3]| {
        let m = gio::Menu::new();
        for (label, action) in items {
            m.append(Some(label), Some(action));
        }
        m
    };
    let mk_arrange_menu = |dist: bool| {
        let m = gio::Menu::new();
        m.append_section(
            None,
            &align_section([
                (tr("Links ausrichten"), "win.sel-align-left"),
                (tr("Waagerecht mittig ausrichten"), "win.sel-align-hcenter"),
                (tr("Rechts ausrichten"), "win.sel-align-right"),
            ]),
        );
        m.append_section(
            None,
            &align_section([
                (tr("Oben ausrichten"), "win.sel-align-top"),
                (tr("Senkrecht mittig ausrichten"), "win.sel-align-vcenter"),
                (tr("Unten ausrichten"), "win.sel-align-bottom"),
            ]),
        );
        if dist {
            let d = gio::Menu::new();
            d.append(Some(tr("Horizontal verteilen")), Some("win.distribute-h"));
            d.append(Some(tr("Vertikal verteilen")), Some("win.distribute-v"));
            m.append_section(None, &d);
        }
        m
    };
    let arrange_menus = (mk_arrange_menu(false), mk_arrange_menu(true));
    arrange.set_menu_model(Some(&arrange_menus.0));
    // Pfadoperationen direkt als Symbole
    let bools = hbox(0);
    for (icon, tip, action) in [
        ("mondrian-union-symbolic", tr("Vereinigen"), "win.bool-union"),
        (
            "mondrian-subtract-symbolic",
            tr("Vorderes abziehen"),
            "win.bool-subtract",
        ),
        (
            "mondrian-intersect-symbolic",
            tr("Schnittmenge"),
            "win.bool-intersect",
        ),
        (
            "mondrian-exclude-symbolic",
            tr("Überlappung entfernen"),
            "win.bool-exclude",
        ),
    ] {
        bools.append(&float_button(icon, tip, action));
    }
    bools.set_visible(false);
    bar.append(&group);
    bar.append(&ungroup);
    bar.append(&arrange);
    bar.append(&bools);
    bar.append(&trace);
    bar.append(&sep);
    bar.append(&lock);
    bar.append(&effects);
    let more = gtk::Button::from_icon_name("view-more-symbolic");
    more.add_css_class("flat");
    more.set_tooltip_text(Some(tr("Weitere Möglichkeiten")));
    more.set_focusable(false);
    bar.append(&more);
    FloatBar {
        bar,
        group,
        ungroup,
        sep,
        lock,
        effects,
        trace,
        trace_colors,
        trace_make,
        arrange,
        arrange_menus,
        bools,
        more,
        shown: Cell::new(None),
        pending: Cell::new(false),
    }
}

thread_local! {
    /// zuletzt gewählte Zahl der Zwischenschritte eines Übergangs
    static BLEND_STEPS: Cell<f64> = const { Cell::new(5.0) };
}

/// Übergang zwischen den beiden ausgewählten Formen: Zahl der Zwischenschritte erfragen.
fn blend_dialog(ui: &Rc<Ui>) {
    let Some(d) = cur(ui) else { return };
    if !d.ed.borrow().can_blend() {
        return;
    }
    let dlg = adw::AlertDialog::new(
        Some(tr("Übergang")),
        Some(tr("Übergang zwischen den beiden Formen erzeugen")),
    );
    let steps = spin(1.0, 100.0, 1.0, 0, tr("Anzahl der Zwischenschritte"));
    steps.set_value(BLEND_STEPS.with(|c| c.get()));
    let r = hbox(12);
    let l = gtk::Label::builder()
        .label(tr("Schritte"))
        .xalign(0.0)
        .hexpand(true)
        .build();
    r.append(&l);
    r.append(&steps);
    dlg.set_extra_child(Some(&r));
    dlg.add_response("cancel", tr("_Abbrechen"));
    dlg.add_response("create", tr("_Erstellen"));
    dlg.set_response_appearance("create", adw::ResponseAppearance::Suggested);
    dlg.set_default_response(Some("create"));
    dlg.set_close_response("cancel");
    let u = ui.clone();
    dlg.connect_response(None, move |_, r| {
        if r != "create" {
            return;
        }
        steps.update();
        let n = steps.value().round().max(1.0);
        BLEND_STEPS.with(|c| c.set(n));
        if let Some(d) = cur(&u) {
            d.ed.borrow_mut().blend_selected(n as usize);
            refresh(&u);
        }
    });
    dlg.present(Some(&ui.window));
}

/// Nachfragen, was aus der Auswahl in die Bibliothek soll: Farbe, Stil oder abbrechen.
fn lib_add_dialog(ui: &Rc<Ui>) {
    let Some(d) = cur(ui) else { return };
    let (fill, stroke, style, text) = {
        let ed = d.ed.borrow();
        (
            ed.selected_color(false).is_some(),
            ed.selected_color(true).is_some(),
            ed.selected_lib_style(false).is_some(),
            ed.selected_lib_style(true).is_some(),
        )
    };
    let dlg = adw::AlertDialog::new(
        Some(tr("Zur Bibliothek hinzufügen")),
        Some(tr(
            "Was soll aus der Auswahl in die Bibliothek übernommen werden?",
        )),
    );
    dlg.add_response("cancel", tr("_Abbrechen"));
    dlg.add_response("color", tr("_Farbe hinzufügen"));
    dlg.add_response(
        "style",
        if text && !style {
            tr("_Textstil hinzufügen")
        } else {
            tr("_Stil hinzufügen")
        },
    );
    dlg.add_response("graphic", tr("Als _Grafik"));
    dlg.set_response_enabled("color", fill || stroke);
    dlg.set_response_enabled("style", style || text);
    dlg.set_response_appearance("style", adw::ResponseAppearance::Suggested);
    dlg.set_default_response(Some(if style || text { "style" } else { "color" }));
    dlg.set_close_response("cancel");
    let u = ui.clone();
    dlg.connect_response(None, move |_, r| {
        let action = match r {
            "color" if fill => "lib-add-fill",
            "color" => "lib-add-stroke",
            "style" if style => "lib-add-style",
            "style" => "lib-add-textstyle",
            "graphic" => "lib-add-graphic",
            _ => return,
        };
        if let Some(a) = u.window.lookup_action(action) {
            a.activate(None);
        }
    });
    label_icon_buttons(dlg.upcast_ref());
    dlg.present(Some(&ui.window));
}

/// Leiste nach dem nächsten Zeichnen an die Auswahl setzen (nicht während des Zeichnens,
/// damit sich das Layout nicht mitten im Zeichnen ändert).
fn schedule_float(ui: &Rc<Ui>, d: &Rc<DocTab>) {
    if d.float.pending.replace(true) {
        return;
    }
    let (u, d) = (ui.clone(), d.clone());
    glib::idle_add_local_once(move || {
        d.float.pending.set(false);
        update_float(&u, &d);
    });
}

fn update_float(ui: &Ui, d: &DocTab) {
    let fb = &d.float;
    let hide = || {
        if fb.bar.is_visible() {
            fb.bar.set_visible(false);
        }
        fb.shown.set(None);
    };
    let (w, h) = canvas_size(d);
    let state = {
        let ed = d.ed.borrow();
        if ed.tool != Tool::Select || !matches!(ed.drag, Drag::None) || ed.space_down {
            None
        } else {
            ed.selection_actions().and_then(|st| {
                let b = ed.selection_bbox(&ui.metrics)?;
                let a = ed.view.to_screen(Pt::new(b.x0, b.y0));
                let c = ed.view.to_screen(Pt::new(b.x1, b.y1));
                let can_trace = ed
                    .traceable_image()
                    .is_some_and(|(_, data)| crate::render::image_is_flat(&data));
                Some((st, a, c, ed.selection_counts(), can_trace))
            })
        }
    };
    let Some(((locked, can_group, can_ungroup), a, c, (n_obj, n_unit), traceable)) = state else {
        hide();
        return;
    };
    // Auswahl ganz außerhalb des Sichtbereichs: keine Leiste
    if c.x < 0.0 || c.y < 0.0 || a.x > w || a.y > h {
        hide();
        return;
    }
    // Ausrichten aneinander ab zwei Objekten bzw. Gruppen, Verteilen ab drei;
    // Pfadoperationen ab zwei Objekten
    let can_align = n_unit >= 2 && !locked;
    let can_dist = n_unit >= 3 && !locked;
    let can_bool = n_obj >= 2 && !locked;
    let can_trace = traceable && !locked;
    let flags = locked as u8
        | (can_group as u8) << 1
        | (can_ungroup as u8) << 2
        | (can_align as u8) << 3
        | (can_dist as u8) << 5
        | (can_trace as u8) << 6
        | (can_bool as u8) << 7;
    if fb.shown.get().map(|s| s.2) != Some(flags) {
        fb.group.set_visible(can_group);
        fb.ungroup.set_visible(can_ungroup);
        fb.trace.set_visible(can_trace);
        fb.arrange.set_visible(can_align);
        fb.arrange.set_menu_model(Some(if can_dist {
            &fb.arrange_menus.1
        } else {
            &fb.arrange_menus.0
        }));
        fb.bools.set_visible(can_bool);
        fb.sep
            .set_visible(can_group || can_ungroup || can_align || can_bool || can_trace);
        fb.lock.set_icon_name(if locked {
            "changes-prevent-symbolic"
        } else {
            "changes-allow-symbolic"
        });
        fb.lock
            .set_tooltip_text(Some(if locked { tr("Entsperren") } else { tr("Sperren") }));
        fb.group.set_sensitive(!locked);
    }
    fb.bar.set_visible(true);
    // gemessene Größe enthält die Ränder, mit denen die Leiste positioniert wird
    let (_, bw, _, _) = fb.bar.measure(gtk::Orientation::Horizontal, -1);
    let bw = bw - fb.bar.margin_start();
    let (_, bh, _, _) = fb.bar.measure(gtk::Orientation::Vertical, -1);
    let bh = bh - fb.bar.margin_top();
    let (bw, bh) = (bw.max(1) as f64, bh.max(1) as f64);
    let edge = if ui.show_rulers.get() { RULER } else { 0.0 } + 6.0;
    // über der Auswahl (oberhalb des Drehgriffs), sonst darunter
    let lift = if locked {
        10.0
    } else {
        crate::editor::ROTATE_OFFSET + crate::editor::HANDLE_RADIUS + 8.0
    };
    let above = a.y - lift - bh;
    let below = c.y + crate::editor::HANDLE_RADIUS + 10.0;
    let y = if above >= edge {
        above
    } else if below + bh <= h - 6.0 {
        below
    } else {
        edge
    };
    let x = ((a.x + c.x) / 2.0 - bw / 2.0).clamp(edge, (w - bw - 6.0).max(edge));
    let (xi, yi) = (x.round() as i32, y.round() as i32);
    if fb.shown.get() != Some((xi, yi, flags)) {
        fb.bar.set_margin_start(xi);
        fb.bar.set_margin_top(yi);
        fb.shown.set(Some((xi, yi, flags)));
    }
}

impl DocTab {
    fn name(&self) -> String {
        self.file
            .borrow()
            .as_ref()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
            .or_else(|| self.title.borrow().clone())
            .unwrap_or_else(|| tr("Unbenannt").into())
    }

    /// Leeres, unverändertes Dokument ohne Datei (kann beim Öffnen ersetzt werden).
    fn pristine(&self) -> bool {
        let ed = self.ed.borrow();
        self.file.borrow().is_none()
            && !ed.dirty
            && !ed.can_undo()
            && ed.doc.layers.iter().all(|l| l.objects.is_empty())
    }
}

/// Kurzname aus dem Tooltip („Auswahl (V)“ → „Auswahl“).
fn short_label(l: &str) -> &str {
    l.split(" (").next().unwrap_or(l).split(" –").next().unwrap_or(l)
}

/// Eine Werkzeugvariante (z. B. „Ellipse“ in der Formen-Gruppe).
#[derive(Clone, Copy)]
struct Variant {
    tool: Tool,
    kind: Option<ShapeKind>,
    icon: &'static str,
    label: &'static str,
}

/// Ein Platz der Werkzeugleiste: ein Werkzeug oder eine Gruppe (z. B. die Formen). Ist
/// ein Werkzeug der Gruppe aktiv, klappt sie auf und zeigt alle Varianten; sonst steht nur
/// die zuletzt benutzte da.
struct Slot {
    /// je Variante ein Knopf, jeder in einem Revealer (zum Auf- und Zuklappen)
    buttons: Vec<gtk::ToggleButton>,
    revealers: Vec<gtk::Revealer>,
    /// kleiner Pfeil rechts im Knopf: zugeklappt gibt es hier weitere Werkzeuge
    arrows: Vec<gtk::Image>,
    /// Rahmen der Gruppe (aufgeklappt mit eigenem Hintergrund)
    group: gtk::Box,
    variants: Vec<Variant>,
    current: Cell<usize>,
}

/// Einstellungen des aktiven Werkzeugs: zweite schwebende Leiste unter den Werkzeugen
/// (nur bei Werkzeugen, die etwas einzustellen haben).
struct ToolOpts {
    bar: gtk::Box,
    corners_box: gtk::Box,
    corners: gtk::SpinButton,
    eraser_box: gtk::Box,
    eraser: gtk::SpinButton,
    pencil_box: gtk::Box,
    smooth: gtk::SpinButton,
    close: gtk::SpinButton,
}

/// Beschriftete Zahl für die Werkzeugeinstellungen: „Ecken [ 5 ]“, „Größe [ 20 ] px“.
fn opt_spin(
    label: &str,
    min: f64,
    max: f64,
    step: f64,
    unit: &str,
    tip: &str,
) -> (gtk::Box, gtk::SpinButton) {
    let b = hbox(6);
    let l = gtk::Label::new(Some(label));
    l.set_margin_start(6);
    b.append(&l);
    let sp = spin(min, max, step, 0, tip);
    sp.set_width_chars(4);
    b.append(&sp);
    if !unit.is_empty() {
        let u = gtk::Label::new(Some(unit));
        u.add_css_class("dim-label");
        b.append(&u);
    }
    (b, sp)
}

fn build_tool_opts() -> ToolOpts {
    let bar = osd_bar();
    bar.set_margin_top(6);
    bar.set_margin_bottom(0);
    bar.set_margin_start(0);
    bar.set_margin_end(0);
    bar.set_halign(gtk::Align::Start);
    bar.set_spacing(12);
    let (corners_box, corners) = opt_spin(
        tr("Ecken"),
        3.0,
        64.0,
        1.0,
        "",
        tr("Anzahl der Ecken neuer Vielecke und Sterne (und der ausgewählten)"),
    );
    let (eraser_box, eraser) = opt_spin(
        tr("Größe"),
        1.0,
        2000.0,
        1.0,
        "px",
        tr("Durchmesser des Radierers ( [ und ] ändern ihn )"),
    );
    let pencil_box = hbox(12);
    let (sb, smooth) = opt_spin(
        tr("Glättung"),
        0.0,
        100.0,
        5.0,
        "%",
        tr("Wie stark zittrige Striche geglättet werden"),
    );
    let (cb, close) = opt_spin(
        tr("Schließen bis"),
        0.0,
        200.0,
        1.0,
        "px",
        tr("Liegen Anfang und Ende näher beieinander, wird die Form geschlossen (0 = nie)"),
    );
    pencil_box.append(&sb);
    pencil_box.append(&cb);
    for b in [&corners_box, &eraser_box, &pencil_box] {
        b.set_visible(false);
        bar.append(b);
    }
    bar.set_visible(false);
    ToolOpts {
        bar,
        corners_box,
        corners,
        eraser_box,
        eraser,
        pencil_box,
        smooth,
        close,
    }
}

fn slot_definitions() -> Vec<Vec<Variant>> {
    let v = |tool, kind, icon, label| Variant {
        tool,
        kind,
        icon,
        label,
    };
    vec![
        vec![v(Tool::Select, None, "mondrian-select-symbolic", tr("Auswahl (V)"))],
        vec![v(
            Tool::Node,
            None,
            "mondrian-node-symbolic",
            tr("Knoten bearbeiten (A)"),
        )],
        vec![
            v(
                Tool::Shape,
                Some(ShapeKind::Rect),
                "mondrian-rect-symbolic",
                tr("Rechteck (R)"),
            ),
            v(
                Tool::Shape,
                Some(ShapeKind::Ellipse),
                "mondrian-ellipse-symbolic",
                tr("Ellipse"),
            ),
            v(
                Tool::Shape,
                Some(ShapeKind::Polygon),
                "mondrian-polygon-symbolic",
                tr("Vieleck"),
            ),
            v(
                Tool::Shape,
                Some(ShapeKind::Star),
                "mondrian-star-symbolic",
                tr("Stern"),
            ),
        ],
        vec![v(
            Tool::Builder,
            None,
            "mondrian-builder-symbolic",
            tr("Formerstellung (Umschalt M oder K) – Klick in eine Fläche macht sie zur Form, Ziehen über Flächen vereinigt sie, mit Alt löschen"),
        )],
        vec![
            v(Tool::Pen, None, "mondrian-pen-symbolic", tr("Zeichenstift (P)")),
            v(
                Tool::Curvature,
                None,
                "mondrian-curvature-symbolic",
                tr("Kurvenzeichner (Umschalt P)"),
            ),
        ],
        vec![v(Tool::Pencil, None, "mondrian-pencil-symbolic", tr("Freihand (N)"))],
        vec![
            v(
                Tool::Eraser,
                None,
                "mondrian-eraser-symbolic",
                tr("Radierer (Umschalt E) – Größe mit [ und ]"),
            ),
            v(
                Tool::Scissors,
                None,
                "edit-cut-symbolic",
                tr("Schere (C) – eine Linie ziehen zerschneidet Formen und Pfade"),
            ),
        ],
        vec![
            v(Tool::Text, None, "mondrian-text-symbolic", tr("Text (T)")),
            v(
                Tool::TextPath,
                None,
                "mondrian-textpath-symbolic",
                tr("Text auf Pfad (Umschalt T)"),
            ),
            v(Tool::TextArea, None, "mondrian-textarea-symbolic", tr("Textfeld")),
        ],
        vec![v(
            Tool::Guide,
            None,
            "mondrian-guide-symbolic",
            tr("Hilfslinien (G) – klicken und ziehen, Umschalt rastet den Winkel ein"),
        )],
    ]
}

/// Bedienelemente der Eigenschaftenleiste.
struct Panel {
    heading: gtk::Label,
    empty_hint: gtk::Box,
    shape_group: gtk::Box,
    shape_kind: gtk::DropDown,
    corners_row: gtk::Box,
    corners: gtk::SpinButton,
    radius_row: gtk::Box,
    radius: gtk::SpinButton,
    text_group: gtk::Box,
    /// Kopfzeile des Text-Ausklappbereichs: Schrift und Größe
    text_hint: gtk::Label,
    font: gtk::DropDown,
    size: gtk::SpinButton,
    /// Schnitt der Schrift (Light, Regular, Bold Italic …)
    face: gtk::DropDown,
    faces: RefCell<Vec<FontFace>>,
    faces_family: RefCell<String>,
    /// stufenlose Strichstärke (nur bei variablen Schriften sichtbar)
    weight: gtk::Scale,
    weight_row: gtk::Box,
    underline: gtk::ToggleButton,
    strike: gtk::ToggleButton,
    list_box: gtk::Box,
    list_on: gtk::ToggleButton,
    indent_less: gtk::Button,
    indent_more: gtk::Button,
    para_row: gtk::Box,
    para_spacing: gtk::SpinButton,
    columns_row: gtk::Box,
    columns: gtk::SpinButton,
    small_caps: gtk::ToggleButton,
    ligatures: gtk::ToggleButton,
    /// Textstile der aktuellen Bibliothek (Klick übernimmt)
    text_styles_box: gtk::Box,
    text_styles: gtk::FlowBox,
    list_style: Rc<Cell<ListStyle>>,
    path_box: gtk::Box,
    path_below: gtk::ToggleButton,
    path_reverse: gtk::ToggleButton,
    text_frame: gtk::Frame,
    text: gtk::TextView,
    offset_row: gtk::Box,
    offset: gtk::SpinButton,
    align: [gtk::ToggleButton; 4],
    valign_box: gtk::Box,
    valign: [gtk::ToggleButton; 3],
    line_spacing: gtk::SpinButton,
    tracking: gtk::SpinButton,
    node_group: gtk::Box,
    node_radius: gtk::SpinButton,
    node_corner: gtk::Button,
    node_smooth: gtk::Button,
    node_delete: gtk::Button,
    style_group: gtk::Box,
    /// Farbkreise für Füllung, Kontur und Schatten
    fill_sw: Rc<Swatches>,
    stroke_sw: Rc<Swatches>,
    shadow_sw: Rc<Swatches>,
    shadow_box: gtk::Box,
    /// Effekte, nur sichtbar, wenn angewendet
    shadow_section: gtk::Box,
    /// je Effektkategorie ein Ausklappbereich
    blend_fold: FxFold,
    tex_fold: FxFold,
    art_fold: FxFold,
    lens_fold: FxFold,
    tint_fold: FxFold,
    mirror_fold: FxFold,
    image_sw: Rc<Swatches>,
    temp: gtk::Scale,
    stroke_align: [gtk::ToggleButton; 3],
    frame_fill_sw: Rc<Swatches>,
    frame_stroke_sw: Rc<Swatches>,
    inset_row: gtk::Box,
    shadow_opacity: gtk::SpinButton,
    shadow_blur: gtk::SpinButton,
    shadow_dx: gtk::SpinButton,
    shadow_dy: gtk::SpinButton,
    width: gtk::SpinButton,
    opacity: gtk::SpinButton,
    grad_add: gtk::Button,
    grad_box: gtk::Box,
    grad_bar: gtk::DrawingArea,
    grad_color: gtk::ColorDialogButton,
    grad_remove_stop: gtk::Button,
    grad_angle: gtk::SpinButton,
    grad_kind: gtk::DropDown,
    fill_cmyk: gtk::Box,
    fill_cmyk_scales: [gtk::Scale; 4],
    stroke_cmyk: gtk::Box,
    stroke_cmyk_scales: [gtk::Scale; 4],
    arrange_group: gtk::Box,
    /// fester Bereich unten: Trennlinie + Position
    pinned: gtk::Box,
    angle: gtk::SpinButton,
    marker_row: gtk::Box,
    /// Konturecken und Linienenden
    join_row: gtk::Box,
    dash_row: gtk::Box,
    dash: gtk::DropDown,
    join: [gtk::ToggleButton; 3],
    cap_row: gtk::Box,
    cap: [gtk::ToggleButton; 3],
    marker: gtk::DropDown,
    marker_ends_box: gtk::Box,
    marker_ends: [gtk::ToggleButton; 3],
    marker_size_row: gtk::Box,
    marker_size: gtk::SpinButton,
    tframe_group: gtk::Box,
    tframe_fill_on: gtk::CheckButton,
    tframe_fill: gtk::ColorDialogButton,
    tframe_stroke_on: gtk::CheckButton,
    tframe_stroke: gtk::ColorDialogButton,
    tframe_width_row: gtk::Box,
    tframe_width: gtk::SpinButton,
    tframe_inset: gtk::SpinButton,
    tframe_release: gtk::Button,
    image_group: gtk::Box,
    image_crop: gtk::Button,
    image_modes_box: gtk::Box,
    image_modes: [gtk::ToggleButton; 3],
    image_release: gtk::Button,
    convert: gtk::Button,
    guide_group: gtk::Box,
    guide_angle_row: gtk::Box,
    guide_angle: gtk::SpinButton,
    guide_pos_row: gtk::Box,
    guide_pos_label: gtk::Label,
    guide_pos: gtk::SpinButton,
    guide_show: gtk::Switch,
    guide_snap: gtk::Switch,
    guide_delete: gtk::Button,
}

pub struct Ui {
    pub window: adw::ApplicationWindow,
    tab_view: adw::TabView,
    docs: RefCell<Vec<Rc<DocTab>>>,
    current: RefCell<Option<Rc<DocTab>>>,
    metrics: CairoMetrics,
    toasts: adw::ToastOverlay,
    title: adw::WindowTitle,
    slots: Vec<Rc<Slot>>,
    /// schwebende Leisten über der Zeichenfläche: Werkzeuge (oben links), Ansicht und Seiten
    /// (unten links), Zoom (unten rechts)
    tools_bar: gtk::Box,
    /// Einstellungen des aktiven Werkzeugs (unter den Werkzeugen)
    tool_opts: ToolOpts,
    view_bar: gtk::Box,
    zoom_bar: gtk::Box,
    /// Ebene über den Dokumenten (schwebende Leisten; in schmalen Fenstern auch die
    /// Seitenleisten)
    canvas_overlay: gtk::Overlay,
    /// schmales Fenster: Seitenleisten schweben über der Zeichenfläche, nur eine zugleich
    narrow: Cell<bool>,
    /// rechte ausklappbare Leiste (Eigenschaften), ihr Inhalt und die Trennung zur
    /// Zeichenfläche
    dock: gtk::Box,
    dock_stack: gtk::Stack,
    props_paned: gtk::Paned,
    /// linke ausklappbare Leiste (Ebenen oder Bibliothek), ihr Inhalt und die Trennung
    side_dock: gtk::Box,
    side_stack: gtk::Stack,
    side_paned: gtk::Paned,
    /// Breite der linken Leiste
    side_width: Cell<i32>,
    /// Knöpfe: Eigenschaften (oben rechts), linke Leiste (oben links), Reiter „Ebenen“
    props_toggle: gtk::ToggleButton,
    /// Informationen zum Dokument (rechte Leiste, statt der Eigenschaften)
    info_toggle: gtk::ToggleButton,
    info: InfoWidgets,
    side_toggle: gtk::ToggleButton,
    layers_toggle: gtk::ToggleButton,
    /// zuletzt gezeigter Inhalt der linken Seitenleiste
    side_last: Cell<&'static str>,
    snap_corners: Cell<bool>,
    snap_mids: Cell<bool>,
    snap_anchors: Cell<bool>,
    snap_page: Cell<bool>,
    snap_paths: Cell<bool>,
    snap_spacing: Cell<bool>,
    grid_snap: Cell<bool>,
    grid_size: Cell<f64>,
    frame_dwell: Cell<f64>,
    panel: Panel,
    layers_list: gtk::ListBox,
    /// Zeichenflächen (eigener Bereich unter den Ebenen)
    artboards_list: gtk::ListBox,
    /// Ebenenzeilen: (Ebene, Zeile)
    layer_rows: RefCell<Vec<(u64, gtk::ListBoxRow)>>,
    /// Breite der rechten Seitenleiste (Pixel, wird gespeichert)
    panel_width: Cell<i32>,
    /// Bibliotheken (Farben, Stile) und die gerade gewählte
    libraries: RefCell<Vec<Library>>,
    lib_current: Cell<usize>,
    lib: LibWidgets,
    /// Breite der Eigenschaften
    props_width: Cell<i32>,
    /// Speichern der Einstellungen ist bereits vorgemerkt
    save_pending: Cell<bool>,
    /// gemeinsame Zwischenablage aller Tabs: (Quelldokument, Objekte)
    clipboard: RefCell<(usize, Vec<crate::model::Object>)>,
    thumbs: RefCell<Vec<gtk::DrawingArea>>,
    /// Ausschnitt der Ebenen-Vorschaubilder (alle Zeichenflächen und aller Inhalt), wird
    /// beim nächsten Zeichnen neu bestimmt
    thumb_frame: Cell<Option<crate::geom::Rect>>,
    /// (Dokument, Ebenenversion), für die die Liste zuletzt aufgebaut wurde.
    layers_shown: RefCell<(usize, u64, Vec<u64>)>,
    layers_shown_active: Cell<u64>,
    /// gerade gezogene Ebene (0 = keine)
    layer_drag: Cell<u64>,
    /// Auswahl wird auf diese Ebene gezogen (leeres Kästchen im Vorschaubild)
    sel_target: Cell<u64>,
    /// Markierung beim Ziehen über die Ebenenliste
    layer_mark: RefCell<Option<LayerMark>>,
    zoom_label: gtk::Button,
    /// Notizheft: „Seite n von m“ mit Blättern
    page_box: gtk::Box,
    page_label: gtk::Label,
    updating: Cell<bool>,
    pointer: Cell<(f64, f64)>,
    export: RefCell<ExportSettings>,
    scheme: Cell<Scheme>,
    unit: Cell<Unit>,
    cmyk: Cell<bool>,
    backdrop: Cell<render::Backdrop>,
    show_rulers: Cell<bool>,
    /// Notizheft-Ansicht (eine Seite nach der anderen) statt Zeichenfläche
    notebook: Cell<bool>,
    /// gewählte Sprache (system, de, en_GB)
    language: RefCell<String>,
    /// nach neuen Versionen suchen; Zeitpunkt der letzten Suche
    updates: Cell<bool>,
    update_checked: Cell<i64>,
    show_guides: Cell<bool>,
    snap_guides: Cell<bool>,
    split: Cell<i32>,
    tool: Cell<Tool>,
    closing: Cell<bool>,
    /// Willkommensseite oder Zeichenfläche
    /// Gerade ist ein Start-Tab (Willkommensseite) gewählt.
    welcome: Cell<bool>,
    /// Offene Start-Tabs mit ihrer Liste „Zuletzt geöffnet“.
    starts: RefCell<Vec<StartTab>>,
    /// Pipette aktiv: Farbbereich, der die Farbe bekommt, und letzte Probe (Ort, Farbe)
    pipette: RefCell<Option<std::rc::Weak<Swatches>>>,
    pipette_at: Cell<Option<(f64, f64, Option<Color>)>>,
    /// Nur im Editor sichtbare Kopfleistenknöpfe
    editor_only: Vec<gtk::Widget>,
}

// ───────────────────────────── Hilfsfunktionen ─────────────────────────────

/// Haupt-Modifikator: Strg, auf dem Mac ⌘ (Befehlstaste).
fn primary_mask() -> gdk::ModifierType {
    if cfg!(target_os = "macos") {
        gdk::ModifierType::META_MASK
    } else {
        gdk::ModifierType::CONTROL_MASK
    }
}

/// Modifikatoren eines Tastendrucks in der Schreibweise der Belegung (`<Control>` steht
/// auf dem Mac für ⌘).
fn accel_mods(st: gdk::ModifierType) -> gdk::ModifierType {
    let mut m = st & (gdk::ModifierType::SHIFT_MASK | gdk::ModifierType::ALT_MASK);
    if st.contains(primary_mask()) {
        m |= gdk::ModifierType::CONTROL_MASK;
    }
    m
}

/// Kurzinfo mit dem aktuellen Kürzel der Belegung: „Gruppieren (Strg+G)“.
fn key_tip(text: &str, id: &str) -> String {
    match KEYMAP.with(|k| k.borrow().keys(id).into_iter().next()) {
        Some(k) => format!("{text} ({})", crate::keys::label(&k)),
        None => text.to_string(),
    }
}

fn mods(st: gdk::ModifierType) -> Mods {
    Mods {
        shift: st.contains(gdk::ModifierType::SHIFT_MASK),
        ctrl: st.contains(primary_mask()),
        alt: st.contains(gdk::ModifierType::ALT_MASK),
    }
}

fn to_rgba(c: Color) -> gdk::RGBA {
    gdk::RGBA::new(c.r as f32, c.g as f32, c.b as f32, c.a as f32)
}

fn from_rgba(c: &gdk::RGBA) -> Color {
    Color {
        r: c.red() as f64,
        g: c.green() as f64,
        b: c.blue() as f64,
        a: c.alpha() as f64,
    }
}

fn hbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Horizontal, spacing)
}

fn vbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, spacing)
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.set_tooltip_text(Some(tooltip));
    b.add_css_class("flat");
    b
}

fn action_button(icon: &str, tooltip: &str, action: &str) -> gtk::Button {
    let b = icon_button(icon, tooltip);
    b.set_action_name(Some(action));
    b
}

fn spin(min: f64, max: f64, step: f64, digits: u32, tooltip: &str) -> gtk::SpinButton {
    let s = gtk::SpinButton::with_range(min, max, step);
    s.set_digits(digits);
    s.set_tooltip_text(Some(tooltip));
    s.set_valign(gtk::Align::Center);
    s
}

/// Schriftauswahl wie in einer Textverarbeitung: Ausklappliste aller Schriftfamilien,
/// jede in ihrer eigenen Schrift dargestellt, mit Suchfeld.
fn font_dropdown() -> gtk::DropDown {
    use pango::prelude::FontFamilyExt as _;
    let ctx = gtk::Label::new(None).pango_context();
    let mut names: Vec<String> = ctx.list_families().iter().map(|f| f.name().to_string()).collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    let list = gtk::StringList::new(&refs);
    let expr = gtk::PropertyExpression::new(
        gtk::StringObject::static_type(),
        None::<gtk::Expression>,
        "string",
    );
    let dd = gtk::DropDown::new(Some(list), Some(expr));
    dd.set_enable_search(true);
    dd.set_search_match_mode(gtk::StringFilterMatchMode::Substring);
    dd.set_tooltip_text(Some(tr("Schriftart")));
    dd.set_valign(gtk::Align::Center);
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let l = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::End)
            .max_width_chars(22)
            .build();
        item.set_child(Some(&l));
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let (Some(l), Some(so)) = (
            item.child().and_downcast::<gtk::Label>(),
            item.item().and_downcast::<gtk::StringObject>(),
        ) else {
            return;
        };
        let name = so.string();
        l.set_label(&name);
        l.set_tooltip_text(Some(&name));
        let attrs = pango::AttrList::new();
        attrs.insert(pango::AttrString::new_family(&name));
        l.set_attributes(Some(&attrs));
    });
    dd.set_factory(Some(&factory));
    dd
}

/// Gewählte Schriftfamilie der Ausklappliste.
fn font_selected(dd: &gtk::DropDown) -> Option<String> {
    dd.selected_item()
        .and_downcast::<gtk::StringObject>()
        .map(|s| s.string().to_string())
}

/// Schriftfamilie in der Ausklappliste wählen (unbekannte werden angehängt).
fn font_select(dd: &gtk::DropDown, family: &str) {
    if font_selected(dd).as_deref() == Some(family) {
        return;
    }
    let Some(list) = dd.model().and_downcast::<gtk::StringList>() else {
        return;
    };
    let n = list.n_items();
    let pos = (0..n)
        .find(|i| {
            list.string(*i)
                .map(|s| s.eq_ignore_ascii_case(family))
                .unwrap_or(false)
        })
        .unwrap_or_else(|| {
            list.append(family);
            n
        });
    dd.set_selected(pos);
}

/// Ein Schnitt einer Schriftfamilie (Light, Regular, Medium Italic …).
#[derive(Clone, Debug, PartialEq)]
struct FontFace {
    name: String,
    weight: u16,
    italic: bool,
}

/// Alle Schnitte einer installierten Schriftfamilie, nach Strichstärke geordnet.
fn font_faces(family: &str) -> Vec<FontFace> {
    use glib::translate::IntoGlib as _;
    use pango::prelude::{FontFaceExt as _, FontFamilyExt as _};
    let ctx = gtk::Label::new(None).pango_context();
    let mut out: Vec<FontFace> = Vec::new();
    let fams = ctx.list_families();
    if let Some(fam) = fams.iter().find(|f| f.name().eq_ignore_ascii_case(family)) {
        let mut faces = fam.list_faces();
        // echte Schnitte vor den von Pango nachgebildeten
        faces.sort_by_key(|f| f.is_synthesized());
        for face in faces {
            let d = face.describe();
            let w = d.weight().into_glib().clamp(1, 1000) as u16;
            let it = matches!(d.style(), pango::Style::Italic | pango::Style::Oblique);
            if out.iter().any(|f| f.weight == w && f.italic == it) {
                continue;
            }
            out.push(FontFace {
                name: face.face_name().to_string(),
                weight: w,
                italic: it,
            });
        }
    }
    if out.is_empty() {
        for (name, weight, italic) in [
            ("Regular", 400, false),
            ("Italic", 400, true),
            ("Bold", 700, false),
            ("Bold Italic", 700, true),
        ] {
            out.push(FontFace {
                name: name.into(),
                weight,
                italic,
            });
        }
    }
    out.sort_by_key(|f| (f.weight, f.italic));
    out
}

/// Variable Schrift (stufenlose Strichstärke)? Fragt Fontconfig, Ergebnis zwischengespeichert.
fn font_is_variable(family: &str) -> bool {
    thread_local! {
        static VARIABLE: RefCell<std::collections::HashMap<String, bool>> =
            RefCell::new(std::collections::HashMap::new());
    }
    // Fontconfig gibt es unter Windows und macOS nicht (unter Windows öffnete ein Aufruf
    // zudem ein Konsolenfenster)
    if cfg!(any(windows, target_os = "macos")) {
        return false;
    }
    let key = family.to_lowercase();
    if let Some(v) = VARIABLE.with(|m| m.borrow().get(&key).copied()) {
        return v;
    }
    let mut pat = String::new();
    for ch in family.chars() {
        if matches!(ch, '\\' | '-' | ':' | ',') {
            pat.push('\\');
        }
        pat.push(ch);
    }
    let v = std::process::Command::new("fc-list")
        .arg(format!("{pat}:variable=true"))
        .arg("family")
        .output()
        .map(|o| o.status.success() && o.stdout.iter().any(|b| !b.is_ascii_whitespace()))
        .unwrap_or(false);
    VARIABLE.with(|m| m.borrow_mut().insert(key, v));
    v
}

/// Auswahlliste mit den Schnitten füllen.
fn faces_model(faces: &[FontFace]) -> gtk::StringList {
    let names: Vec<&str> = faces.iter().map(|f| f.name.as_str()).collect();
    gtk::StringList::new(&names)
}

/// Passendster Schnitt zu Strichstärke und Kursiv (gleiche Lage bevorzugt).
fn face_index(faces: &[FontFace], weight: u16, italic: bool) -> u32 {
    faces
        .iter()
        .enumerate()
        .min_by_key(|(_, f)| {
            (f.weight as i32 - weight as i32).abs() + if f.italic == italic { 0 } else { 10_000 }
        })
        .map(|(i, _)| i as u32)
        .unwrap_or(gtk::INVALID_LIST_POSITION)
}

/// Breite der Bedienelemente rechts in den Zeilen der Eigenschaften: alle gleich breit,
/// damit sie untereinander bündig stehen.
const CTRL_W: i32 = 150;

/// Liste von Einstellungszeilen mit Trennlinien dazwischen (wie eine GNOME-Liste).
fn rows_box() -> gtk::Box {
    let b = vbox(0);
    b.add_css_class("mondrian-rows");
    b
}

/// Karte: Liste von Zeilen mit abgerundetem Rahmen („boxed list“ wie in den
/// GNOME-Einstellungen).
fn card() -> gtk::Box {
    let c = rows_box();
    c.add_css_class("card");
    c
}

/// Abschnitt der Eigenschaftenleiste: Überschrift über einer Karte. Liefert (Abschnitt,
/// Karte); die Überschrift verschwindet mit der Karte.
fn section(title: &str) -> (gtk::Box, gtk::Box) {
    let g = vbox(8);
    let c = card();
    if !title.is_empty() {
        let l = gtk::Label::builder().label(title).xalign(0.0).build();
        l.add_css_class("heading");
        c.bind_property("visible", &l, "visible").sync_create().build();
        g.append(&l);
    }
    g.append(&c);
    (g, c)
}

/// Gruppe zusammengehöriger Zeilen innerhalb einer Karte: ohne Trennlinien dazwischen,
/// zu den Nachbargruppen durch eine Linie getrennt.
fn group_box() -> gtk::Box {
    let g = vbox(0);
    g.add_css_class("mondrian-group");
    g
}

/// Zeile einer Karte: Beschriftung links, Bedienelement rechts (einheitlich breit).
/// Die Zeile ist sichtbar, solange das Bedienelement sichtbar ist.
fn prow(label: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    prow_with(&gtk::Label::new(Some(label)), widget)
}

/// Wie `prow`, mit eigener Beschriftung (deren Text sich ändern kann).
fn prow_with(label: &gtk::Label, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_wrap(true);
    label.set_wrap_mode(pango::WrapMode::WordChar);
    let w: &gtk::Widget = widget.upcast_ref();
    w.set_valign(gtk::Align::Center);
    w.set_hexpand(false);
    if !w.is::<gtk::Switch>() {
        w.set_size_request(CTRL_W, -1);
    }
    let r = hbox(12);
    r.add_css_class("mondrian-prow");
    r.append(label);
    r.append(w);
    w.bind_property("visible", &r, "visible").sync_create().build();
    r
}

/// Zeile einer Karte über die ganze Breite (Umschaltgruppen, Auswahllisten, Knöpfe).
/// Die Zeile ist sichtbar, solange ihr Inhalt sichtbar ist.
fn wide(widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let w: &gtk::Widget = widget.upcast_ref();
    w.set_hexpand(true);
    w.set_valign(gtk::Align::Center);
    let r = hbox(0);
    r.add_css_class("mondrian-prow");
    r.add_css_class("mondrian-prow-wide");
    r.append(w);
    w.bind_property("visible", &r, "visible").sync_create().build();
    r
}

/// Verbundene Knöpfe, alle gleich breit.
fn linked_box<W: IsA<gtk::Widget>>(items: &[&W]) -> gtk::Box {
    let b = hbox(0);
    b.add_css_class("linked");
    b.set_homogeneous(true);
    for w in items {
        w.set_hexpand(true);
        b.append(*w);
    }
    b
}

/// `target` ist sichtbar, solange eines von `items` sichtbar ist.
fn show_if_any(target: &impl IsA<gtk::Widget>, items: &[gtk::Widget]) {
    let t = target.upcast_ref::<gtk::Widget>().downgrade();
    let weak: Vec<glib::WeakRef<gtk::Widget>> = items.iter().map(|w| w.downgrade()).collect();
    let update = Rc::new(move || {
        if let Some(t) = t.upgrade() {
            // eigener Schalter, nicht `is_visible` (das prüft auch die Eltern: eine gerade
            // verborgene Leiste würde ihre Karte sonst dauerhaft ausblenden)
            t.set_visible(
                weak.iter()
                    .any(|w| w.upgrade().is_some_and(|w| w.property::<bool>("visible"))),
            );
        }
    });
    (*update)();
    for w in items {
        let u = update.clone();
        w.connect_notify_local(Some("visible"), move |_, _| (*u)());
    }
}

/// Karte oder Zeilenliste ausblenden, solange keine ihrer Zeilen sichtbar ist (sonst
/// bliebe ein leerer Rahmen bzw. eine doppelte Trennlinie stehen).
fn auto_hide(b: &gtk::Box) {
    let mut items = Vec::new();
    let mut c = b.first_child();
    while let Some(w) = c {
        c = w.next_sibling();
        items.push(w);
    }
    show_if_any(b, &items);
}

/// Zeile „Beschriftung …… Bedienelement“.
fn row(label: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let r = hbox(8);
    let l = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .hexpand(true)
        .build();
    r.append(&l);
    r.append(widget);
    r
}

/// Zahlenfeld mit Einheit dahinter (z. B. „45 %“, „90 °“), eintippbar, mit − und +.
/// Liefert (Zeile, Zahlenfeld).
fn unit_spin(
    min: f64,
    max: f64,
    step: f64,
    digits: u32,
    suffix: &str,
    tooltip: &str,
) -> (gtk::Box, gtk::SpinButton) {
    let s = spin(min, max, step, digits, tooltip);
    s.set_hexpand(true);
    s.set_width_chars(5);
    let unit = gtk::Label::new(Some(suffix));
    unit.add_css_class("dim-label");
    unit.set_width_chars(2);
    unit.set_xalign(0.0);
    let b = hbox(6);
    b.append(&s);
    b.append(&unit);
    (b, s)
}

/// Überschrift eines Unterabschnitts (gleiche Schrift wie die Abschnittstitel).
fn sub_heading(title: &str) -> gtk::Label {
    let l = gtk::Label::builder().label(title).xalign(0.0).build();
    l.add_css_class("caption-heading");
    l.add_css_class("dim-label");
    l.set_margin_top(6);
    l
}

/// Vier Schieberegler für C, M, Y, K (0–100 %).
fn cmyk_block() -> (gtk::Box, [gtk::Scale; 4]) {
    let b = vbox(0);
    b.add_css_class("mondrian-pad");
    let mk = |name: &str| {
        let r = hbox(6);
        let l = gtk::Label::builder()
            .label(name)
            .width_chars(2)
            .xalign(0.0)
            .build();
        l.add_css_class("dim-label");
        let sc = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        r.append(&l);
        r.append(&slider_row("", &sc));
        b.append(&r);
        sc
    };
    let scales = [mk("C"), mk("M"), mk("Y"), mk("K")];
    (b, scales)
}

fn settings_path() -> PathBuf {
    glib::user_config_dir().join("stijl").join("settings.ini")
}

/// Farbschema des Programms.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scheme {
    /// Systemeinstellung übernehmen
    System,
    Light,
    Dark,
}

impl Scheme {
    const ALL: [Scheme; 3] = [Scheme::System, Scheme::Light, Scheme::Dark];
    fn name(self) -> &'static str {
        match self {
            Scheme::System => "system",
            Scheme::Light => "light",
            Scheme::Dark => "dark",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Scheme::System => tr("System"),
            Scheme::Light => tr("Hell"),
            Scheme::Dark => tr("Dunkel"),
        }
    }
    fn from_name(s: &str) -> Scheme {
        Scheme::ALL
            .into_iter()
            .find(|x| x.name() == s)
            .unwrap_or(Scheme::System)
    }
    fn apply(self) {
        adw::StyleManager::default().set_color_scheme(match self {
            Scheme::System => adw::ColorScheme::Default,
            Scheme::Light => adw::ColorScheme::ForceLight,
            Scheme::Dark => adw::ColorScheme::ForceDark,
        });
    }
}

struct Settings {
    scheme: Scheme,
    unit: Unit,
    cmyk: bool,
    backdrop: render::Backdrop,
    rulers: bool,
    /// Ansicht: Notizheft statt Zeichenfläche
    notebook: bool,
    /// Sprache: system, de oder en_GB (wirkt nach einem Neustart)
    language: String,
    guides: bool,
    snap: bool,
    /// Ziele beim Einrasten an Objekten und der Seite
    snap_corners: bool,
    snap_mids: bool,
    snap_anchors: bool,
    snap_page: bool,
    snap_paths: bool,
    /// Einrasten bei gleichen Abständen zwischen Objekten
    spacing: bool,
    grid: bool,
    grid_size: f64,
    /// Wartezeit in Sekunden, bis ein Bild/Textfeld in eine Form gesetzt wird
    dwell: f64,
    /// Lage der Trennlinie zwischen Eigenschaften und Ebenen (Pixel von oben)
    split: i32,
    /// Breite der rechten Seitenleiste
    panel_width: i32,
    /// zuletzt gewählte Bibliothek
    library: String,
    library_width: i32,
    /// Breite der linken Leiste (Ebenen, Bibliothek)
    side_width: i32,
    /// einmal am Tag nach neuen Versionen suchen
    updates: bool,
    /// Zeitpunkt der letzten Suche (Sekunden seit 1970)
    update_checked: i64,
}

/// Einstellungen laden (Schachbrett und Lineale sind standardmäßig aus).
fn load_settings() -> Settings {
    // Fallback: Einstellungen aus der Zeit, als das Programm noch „Linea“ hieß.
    let text = std::fs::read_to_string(settings_path())
        .or_else(|_| {
            std::fs::read_to_string(glib::user_config_dir().join("silhouettes").join("settings.ini"))
        })
        .or_else(|_| std::fs::read_to_string(glib::user_config_dir().join("linea").join("settings.ini")))
        .unwrap_or_default();
    let mut st = Settings {
        scheme: Scheme::System,
        unit: Unit::Px,
        cmyk: false,
        backdrop: render::Backdrop::Default,
        rulers: false,
        notebook: false,
        language: "system".into(),
        guides: true,
        snap: true,
        snap_corners: true,
        snap_mids: true,
        snap_anchors: true,
        snap_page: true,
        snap_paths: true,
        spacing: true,
        grid: false,
        grid_size: 20.0,
        dwell: crate::editor::FRAME_DWELL,
        split: 460,
        panel_width: PANEL_WIDTH,
        library: String::new(),
        library_width: 248,
        side_width: 280,
        updates: crate::update::enabled_by_default(),
        update_checked: 0,
    };
    for line in text.lines() {
        match line.split_once('=').map(|(k, v)| (k.trim(), v.trim())) {
            Some(("unit", v)) => st.unit = Unit::from_name(v).unwrap_or(Unit::Px),
            Some(("scheme", v)) => st.scheme = Scheme::from_name(v),
            // Farbmodell gibt es nicht mehr (immer RGB)
            Some(("color", _)) => {}
            // bis 0.9.24: Schachbrett an/aus
            Some(("checker", v)) => {
                if v == "true" {
                    st.backdrop = render::Backdrop::Checker
                }
            }
            Some(("background", v)) => st.backdrop = render::Backdrop::from_name(v),
            Some(("rulers", v)) => st.rulers = v == "true",
            Some(("view", v)) => st.notebook = v == "notebook",
            Some(("language", v)) => st.language = v.to_string(),
            Some(("guides", v)) => st.guides = v != "false",
            // früher „snap“ – alte Werte werden bewusst nicht übernommen (Einrasten an)
            Some(("guide_snap", v)) => st.snap = v != "false",
            // bis 0.9.24: ein Schalter für alle Ziele an Objekten und der Seite
            Some(("smart", v)) => {
                let on = v != "false";
                st.snap_corners = on;
                st.snap_mids = on;
                st.snap_anchors = on;
                st.snap_page = on;
                st.snap_paths = on;
            }
            Some(("snap_corners", v)) => st.snap_corners = v != "false",
            Some(("snap_mids", v)) => st.snap_mids = v != "false",
            Some(("snap_anchors", v)) => st.snap_anchors = v != "false",
            Some(("snap_page", v)) => st.snap_page = v != "false",
            Some(("snap_paths", v)) => st.snap_paths = v != "false",
            Some(("spacing", v)) => st.spacing = v != "false",
            Some(("grid", v)) => st.grid = v == "true",
            Some(("grid_size", v)) => st.grid_size = v.parse().unwrap_or(20.0f64).clamp(1.0, 1000.0),
            Some(("dwell", v)) => st.dwell = v.parse().unwrap_or(crate::editor::FRAME_DWELL).clamp(0.1, 5.0),
            Some(("panel_width", v)) => {
                st.panel_width = v.parse().unwrap_or(PANEL_WIDTH).clamp(PANEL_MIN, 900)
            }
            Some(("library", v)) => st.library = v.to_string(),
            Some(("library_width", v)) => st.library_width = v.parse().unwrap_or(248).clamp(200, 700),
            Some(("split", v)) => st.split = v.parse().unwrap_or(460).max(100),
            Some(("side_width", v)) => st.side_width = v.parse().unwrap_or(280).clamp(200, 700),
            Some(("updates", v)) => st.updates = v == "true",
            Some(("update_checked", v)) => st.update_checked = v.parse().unwrap_or(0),
            _ => {}
        }
    }
    st
}

/// Gewählte Sprache aus den Einstellungen (vor dem Start von GTK gelesen).
pub fn language_setting() -> String {
    load_settings().language
}

fn save_settings(ui: &Ui) {
    let path = settings_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text = format!(
        "scheme={}\nlanguage={}\nunit={}\ncolor={}\nbackground={}\nrulers={}\nview={}\nguides={}\nguide_snap={}\nsnap_corners={}\nsnap_mids={}\nsnap_anchors={}\nsnap_page={}\nsnap_paths={}\nspacing={}\ngrid={}\ngrid_size={}\ndwell={}\nsplit={}\npanel_width={}\nlibrary={}\nlibrary_width={}\nside_width={}\nupdates={}\nupdate_checked={}\n",
        ui.scheme.get().name(),
        ui.language.borrow(),
        ui.unit.get().name(),
        if ui.cmyk.get() { "cmyk" } else { "rgb" },
        ui.backdrop.get().name(),
        ui.show_rulers.get(),
        if ui.notebook.get() { "notebook" } else { "canvas" },
        ui.show_guides.get(),
        ui.snap_guides.get(),
        ui.snap_corners.get(),
        ui.snap_mids.get(),
        ui.snap_anchors.get(),
        ui.snap_page.get(),
        ui.snap_paths.get(),
        ui.snap_spacing.get(),
        ui.grid_snap.get(),
        ui.grid_size.get(),
        ui.frame_dwell.get(),
        ui.split.get(),
        ui.panel_width.get(),
        ui.libraries
            .borrow()
            .get(ui.lib_current.get())
            .map(|l| l.name.clone())
            .unwrap_or_default(),
        ui.props_width.get(),
        ui.side_width.get(),
        ui.updates.get(),
        ui.update_checked.get(),
    );
    let _ = crate::fsutil::write_atomic(&path, text.as_bytes());
}

fn unit_digits(u: Unit) -> u32 {
    match u {
        Unit::Px => 0,
        Unit::Mm => 1,
        Unit::Cm => 2,
        Unit::In => 2,
        Unit::Pt => 1,
    }
}

fn menu_item(label: &str, action: &str, accel: Option<&str>) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), Some(action));
    // angezeigtes Kürzel: aus der eigenen Belegung, sonst das feste (Strg, auf dem Mac ⌘)
    let acc = KEYMAP
        .with(|k| k.borrow().accel_for_action(action))
        .or_else(|| accel.map(str::to_string));
    if let Some(a) = acc {
        item.set_attribute_value("accel", Some(&crate::keys::to_gtk(&a).to_variant()));
    }
    item
}

/// Kontextmenü: nur, was sonst nirgends erreichbar ist (Zwischenablage, Skalieren,
/// Anordnen, Pfadoperationen); alles andere steht in der Schwebeleiste oder der Seitenleiste.
fn edit_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let s1 = gio::Menu::new();
    s1.append_item(&menu_item(tr("Ausschneiden"), "win.cut", Some("<Control>x")));
    s1.append_item(&menu_item(tr("Kopieren"), "win.copy", Some("<Control>c")));
    s1.append_item(&menu_item(tr("Als Bild kopieren"), "win.copy-image", None));
    s1.append_item(&menu_item(tr("Einfügen"), "win.paste", Some("<Control>v")));
    s1.append_item(&menu_item(tr("Duplizieren"), "win.duplicate", Some("<Control>d")));
    menu.append_section(None, &s1);
    let s2 = gio::Menu::new();
    s2.append_item(&menu_item(tr("Skalieren …"), "win.scale-dialog", None));
    menu.append_section(None, &s2);
    let s3 = gio::Menu::new();
    let order = gio::Menu::new();
    order.append_item(&menu_item(
        tr("In den Vordergrund"),
        "win.raise-top",
        Some("<Control><Shift>bracketright"),
    ));
    order.append_item(&menu_item(
        tr("Schritt nach vorne"),
        "win.raise",
        Some("<Control>bracketright"),
    ));
    order.append_item(&menu_item(
        tr("Schritt nach hinten"),
        "win.lower",
        Some("<Control>bracketleft"),
    ));
    order.append_item(&menu_item(
        tr("In den Hintergrund"),
        "win.lower-bottom",
        Some("<Control><Shift>bracketleft"),
    ));
    s3.append_submenu(Some(tr("Anordnen")), &order);
    menu.append_section(None, &s3);
    let s4 = gio::Menu::new();
    let bools = gio::Menu::new();
    bools.append(Some(tr("Vereinigen")), Some("win.bool-union"));
    bools.append(Some(tr("Vorderes abziehen")), Some("win.bool-subtract"));
    bools.append(Some(tr("Schnittmenge")), Some("win.bool-intersect"));
    bools.append(Some(tr("Überlappung entfernen")), Some("win.bool-exclude"));
    s4.append_submenu(Some(tr("Pfadoperationen")), &bools);
    s4.append_item(&menu_item(
        tr("Pfade verbinden"),
        "win.join-paths",
        Some("<Control>j"),
    ));
    menu.append_section(None, &s4);
    menu
}

/// Letzte Abschnitte des Kontextmenüs: Bibliothek, Löschen.
fn context_menu_end(menu: &gio::Menu) {
    let lib = gio::Menu::new();
    lib.append_item(&menu_item(
        tr("Zur Bibliothek hinzufügen …"),
        "win.lib-add-ask",
        None,
    ));
    menu.append_section(None, &lib);
    let del = gio::Menu::new();
    del.append_item(&menu_item(tr("Löschen"), "win.delete", Some("Delete")));
    menu.append_section(None, &del);
}

/// Kontextmenü der Zeichenfläche inkl. „In Ebene verschieben“.
fn context_menu(d: &DocTab) -> gio::Menu {
    let menu = edit_menu();
    let ed = d.ed.borrow();
    if ed.selection.is_empty() {
        return menu;
    }
    // Übergang nur bei genau zwei Formen
    if ed.can_blend() && !ed.selected_objects().iter().any(|o| o.locked) {
        let sec = gio::Menu::new();
        sec.append_item(&menu_item(tr("Übergang …"), "win.blend-dialog", None));
        menu.append_section(None, &sec);
    }
    if ed.doc.layers.len() < 2 {
        context_menu_end(&menu);
        return menu;
    }
    let here = ed.selection_layers();
    let layers = gio::Menu::new();
    for l in ed.doc.layers.iter().rev() {
        if here.len() == 1 && here[0] == l.id {
            continue;
        }
        let item = gio::MenuItem::new(Some(&l.name), None);
        item.set_action_and_target_value(Some("win.move-to-layer"), Some(&l.id.to_variant()));
        layers.append_item(&item);
    }
    let sec = gio::Menu::new();
    sec.append_submenu(Some(tr("Auf Ebene verschieben")), &layers);
    menu.append_section(None, &sec);
    context_menu_end(&menu);
    menu
}

fn main_menu() -> gio::Menu {
    // Unterstrich: Zugriffstaste (in jeder Sprache eigene, innerhalb des Menüs eindeutig)
    let menu = gio::Menu::new();
    let s1 = gio::Menu::new();
    s1.append(Some(tr("_Neu")), Some("win.new"));
    s1.append(Some(tr("_Öffnen…")), Some("win.open"));
    menu.append_section(None, &s1);
    let s2 = gio::Menu::new();
    s2.append(Some(tr("_Speichern")), Some("win.save"));
    s2.append(Some(tr("Speichern _unter…")), Some("win.save-as"));
    s2.append(Some(tr("_Exportieren…")), Some("win.export"));
    s2.append(Some(tr("_Auswahl exportieren…")), Some("win.export-selection"));
    menu.append_section(None, &s2);
    let s3 = gio::Menu::new();
    s3.append_item(&menu_item(
        tr("_Bild einfügen…"),
        "win.insert-image",
        Some("<Control>i"),
    ));
    menu.append_section(None, &s3);
    // Standardeinträge am Ende (GNOME): Einstellungen, Tastenkürzel, Hilfe, Info
    let s4 = gio::Menu::new();
    s4.append_item(&menu_item(
        tr("E_instellungen"),
        "win.preferences",
        Some("<Control>comma"),
    ));
    s4.append_item(&menu_item(
        tr("_Tastenkürzel"),
        "win.shortcuts",
        Some("<Control>question"),
    ));
    s4.append(Some(tr("_Hilfe")), Some("win.help"));
    s4.append(Some(tr("_Fehler melden…")), Some("win.report-bug"));
    s4.append(Some(tr("_Über Stijl")), Some("win.about"));
    menu.append_section(None, &s4);
    menu
}

// ───────────────────────────── Aufbau ─────────────────────────────

fn build_panel() -> (gtk::Box, Panel) {
    let content = vbox(18);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    // Karten und Zeilenlisten, die sich ausblenden, wenn keine ihrer Zeilen sichtbar ist
    let mut cards: Vec<gtk::Box> = Vec::new();

    let heading = gtk::Label::builder().label(tr("Auswahl")).xalign(0.0).build();
    heading.add_css_class("mondrian-panel-title");
    content.append(&heading);
    // nichts ausgewählt: kurzer Hinweis in der Mitte
    let empty_hint = vbox(6);
    let empty_title = gtk::Label::new(Some(tr("Nichts ausgewählt")));
    empty_title.add_css_class("title-4");
    let empty_text = gtk::Label::builder()
        .label(tr("Klicke ein Objekt an, um es zu bearbeiten"))
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();
    empty_hint.append(&empty_title);
    empty_hint.append(&empty_text);
    empty_hint.add_css_class("dim-label");
    empty_hint.set_vexpand(true);
    empty_hint.set_valign(gtk::Align::Center);
    empty_hint.set_margin_top(60);
    empty_hint.set_margin_bottom(60);
    content.append(&empty_hint);

    // Form
    let (shape_group, shape_card) = section(tr("Form"));
    cards.push(shape_card.clone());
    let names: Vec<&str> = SHAPE_KINDS.iter().map(|&(_, n)| tr(n)).collect();
    let shape_kind = gtk::DropDown::from_strings(&names);
    shape_card.append(&prow(tr("Art"), &shape_kind));
    let (corners_box, corners) = unit_spin(3.0, 64.0, 1.0, 0, "", tr("Anzahl der Ecken"));
    let corners_row = prow(tr("Ecken"), &corners_box);
    shape_card.append(&corners_row);
    let (radius_box, radius) = unit_spin(0.0, 5000.0, 1.0, 0, "px", tr("Eckenradius in px"));
    let radius_row = prow(tr("Eckenradius"), &radius_box);
    shape_card.append(&radius_row);

    // Text: eigene Karte zum Aufklappen; in der Kopfzeile Schrift und Größe
    let text_group = card();
    let text_title = gtk::Label::new(Some(tr("Text")));
    text_title.add_css_class("heading");
    let text_hint = gtk::Label::new(None);
    text_hint.set_ellipsize(pango::EllipsizeMode::End);
    text_hint.set_max_width_chars(16);
    let text_arrow = gtk::Button::from_icon_name("pan-end-symbolic");
    let text_head = fold_head(&text_title, &text_hint, None, &text_arrow);
    // (Name bleibt: alle Textoptionen landen im aufklappbaren Inhalt)
    let text_body = rows_box();
    text_body.set_visible(false);
    let text_open = Rc::new(Cell::new(false));
    let text_fold = fold_register(&text_body, &text_arrow, &text_open);
    {
        let open = text_open.clone();
        text_arrow.connect_clicked(move |_| fold_set(text_fold, !open.get()));
    }
    {
        // auch ein Klick auf die Kopfzeile klappt auf
        let open = text_open.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| fold_set(text_fold, !open.get()));
        text_title.add_controller(click);
    }
    text_group.append(&text_head);
    text_group.append(&text_body);
    let text_group_outer = text_group;
    let text_group = text_body;

    // Textstile der aktuellen Bibliothek zum schnellen Übernehmen
    let text_styles = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .max_children_per_line(3)
        .min_children_per_line(2)
        .column_spacing(6)
        .row_spacing(6)
        .build();
    // (steht wie die Bibliotheksfarben unten im Ausklappbereich, ohne Überschrift)
    let text_styles_box = vbox(4);
    text_styles_box.add_css_class("mondrian-pad");
    text_styles_box.append(&text_styles);

    // ── Schrift: Familie, Schnitt, Größe, bei variablen Schriften die Strichstärke
    // Zusammengehöriges steht ohne Trennlinien in einer Gruppe, Gruppen sind durch
    // Linien getrennt (statt Zwischenüberschriften)
    let g_font = group_box();
    let g_char = group_box();
    let g_para = group_box();
    let g_box = group_box();
    let g_path = group_box();
    for g in [&g_font, &g_char, &g_para, &g_box, &g_path] {
        text_group.append(g);
        cards.push(g.clone());
    }
    let font = font_dropdown();
    g_font.append(&wide(&font));
    // Schnitt statt Fett/Kursiv: alle Schnitte der Familie, bei variablen Schriften ein Regler
    let face = gtk::DropDown::from_strings(&[]);
    face.set_tooltip_text(Some(tr("Schnitt")));
    g_font.append(&wide(&face));
    let (size_box, size) = unit_spin(1.0, 2000.0, 1.0, 0, "px", tr("Schriftgröße in px"));
    g_font.append(&prow(tr("Größe"), &size_box));
    let weight = new_slider(100.0, 900.0);
    weight.adjustment().set_step_increment(10.0);
    weight.set_tooltip_text(Some(tr("Strichstärke (variable Schrift)")));
    let weight_row = slider_row_unit(tr("Strichstärke"), &weight, "");
    weight_row.set_visible(false);
    g_font.append(&weight_row);

    // ── Zeichen: Auszeichnungen und Zeichenabstand
    let fmt_toggle = |icon: &str, tip: &str| {
        gtk::ToggleButton::builder()
            .icon_name(icon)
            .tooltip_text(tip)
            .build()
    };
    let underline = fmt_toggle("format-text-underline-symbolic", tr("Unterstrichen"));
    let strike = fmt_toggle("format-text-strikethrough-symbolic", tr("Durchgestrichen"));
    // OpenType-Funktionen der Schrift: Kapitälchen und Ligaturen (fi, fl …, standardmäßig an)
    let small_caps = fmt_toggle(
        "mondrian-smallcaps-symbolic",
        tr("Kapitälchen: Kleinbuchstaben als kleine Großbuchstaben"),
    );
    let ligatures = fmt_toggle(
        "mondrian-ligature-symbolic",
        tr("Ligaturen (fi, fl …) verwenden, soweit die Schrift sie hat"),
    );
    ligatures.set_active(true);
    let styles = linked_box(&[&underline, &strike, &small_caps, &ligatures]);
    g_char.append(&wide(&styles));
    let (tracking_box, tracking) = unit_spin(
        -300.0,
        2000.0,
        10.0,
        0,
        "",
        tr("Zeichenabstand in 1/1000 Geviert"),
    );
    g_char.append(&prow(tr("Zeichenabstand"), &tracking_box));

    // ── Absatz: Ausrichtung, Aufzählung, Abstände
    let mk_toggle = |icon: &str, tip: &str| {
        gtk::ToggleButton::builder()
            .icon_name(icon)
            .tooltip_text(tip)
            .build()
    };
    let align = [
        mk_toggle("format-justify-left-symbolic", tr("Linksbündig")),
        mk_toggle("format-justify-center-symbolic", tr("Zentriert")),
        mk_toggle("format-justify-right-symbolic", tr("Rechtsbündig")),
        mk_toggle("format-justify-fill-symbolic", tr("Blocksatz")),
    ];
    for b in &align[1..] {
        b.set_group(Some(&align[0]));
    }
    let align_box = linked_box(&[&align[0], &align[1], &align[2], &align[3]]);
    g_para.append(&wide(&align_box));
    // Aufzählung: Knopf schaltet ein/aus, der kleine Pfeil wählt den Stil; davor der Einzug
    let list_style = Rc::new(Cell::new(ListStyle::Dot));
    let list_on = fmt_toggle("view-list-bullet-symbolic", tr("Aufzählung"));
    let list_menu = gio::Menu::new();
    for ls in ListStyle::ALL {
        list_menu.append(
            Some(ls.label()),
            Some(format!("win.text-list::{}", ls.name()).as_str()),
        );
    }
    let list_arrow = gtk::MenuButton::builder()
        .icon_name("pan-down-symbolic")
        .tooltip_text(tr("Aufzählungszeichen wählen"))
        .menu_model(&list_menu)
        .build();
    list_arrow.add_css_class("mondrian-narrow");
    let list_links = hbox(0);
    list_links.add_css_class("linked");
    list_links.append(&list_on);
    list_links.append(&list_arrow);
    // Einzug verkleinern/vergrößern (Absätze der Markierung bzw. alle)
    let indent_less = gtk::Button::builder()
        .icon_name("format-indent-less-symbolic")
        .tooltip_text(tr("Einzug verkleinern"))
        .build();
    let indent_more = gtk::Button::builder()
        .icon_name("format-indent-more-symbolic")
        .tooltip_text(tr("Einzug vergrößern"))
        .build();
    let indent_links = linked_box(&[&indent_less, &indent_more]);
    let list_controls = hbox(6);
    list_controls.append(&indent_links);
    list_controls.append(&list_links);
    let list_box = prow(tr("Aufzählung"), &list_controls);
    g_para.append(&list_box);
    let (ls_box, line_spacing) = unit_spin(
        50.0,
        500.0,
        5.0,
        0,
        "%",
        tr("Zeilenabstand in % der Schriftgröße"),
    );
    g_para.append(&prow(tr("Zeilenabstand"), &ls_box));
    let (para_box, para_spacing) = unit_spin(
        0.0,
        500.0,
        10.0,
        0,
        "%",
        tr("Abstand zwischen Absätzen in % der Schriftgröße"),
    );
    let para_row = prow(tr("Absatzabstand"), &para_box);
    g_para.append(&para_row);
    // Der Text wird direkt im Dokument bearbeitet; das Feld bleibt nur intern bestehen.
    let text = gtk::TextView::new();
    text.set_wrap_mode(gtk::WrapMode::WordChar);
    let text_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(64)
        .max_content_height(160)
        .propagate_natural_height(true)
        .child(&text)
        .build();
    let text_frame = gtk::Frame::new(None);
    text_frame.set_child(Some(&text_scroll));
    text_frame.set_visible(false);
    text_group.append(&text_frame);

    // ── Textfeld: Lage im Feld, Spalten, Abstand zum Rahmen
    let valign = [
        mk_toggle("mondrian-valign-top-symbolic", tr("Im Feld oben")),
        mk_toggle("mondrian-valign-middle-symbolic", tr("Im Feld mittig")),
        mk_toggle("mondrian-valign-bottom-symbolic", tr("Im Feld unten")),
    ];
    for b in &valign[1..] {
        b.set_group(Some(&valign[0]));
    }
    let valign_box = prow(tr("Vertikal"), &linked_box(&[&valign[0], &valign[1], &valign[2]]));
    g_box.append(&valign_box);
    let (columns_box, columns) = unit_spin(
        1.0,
        crate::model::MAX_COLUMNS as f64,
        1.0,
        0,
        "",
        tr("Spaltenzahl – der Text läuft von Spalte zu Spalte"),
    );
    let columns_row = prow(tr("Spalten"), &columns_box);
    g_box.append(&columns_row);
    let (inset_box, tframe_inset) = unit_spin(0.0, 1000.0, 1.0, 0, "px", tr("Abstand des Textes zum Rahmen"));
    let inset_row = prow(tr("Abstand zum Rahmen"), &inset_box);
    g_box.append(&inset_row);

    // ── Text auf Pfad: Versatz, unter der Linie und/oder umgekehrte Laufrichtung
    let (offset_box, offset) = unit_spin(
        -10000.0,
        10000.0,
        1.0,
        0,
        "px",
        tr("Abstand vom Pfadanfang in px"),
    );
    let offset_row = prow(tr("Versatz"), &offset_box);
    g_path.append(&offset_row);
    let path_below = gtk::ToggleButton::builder()
        .icon_name("mondrian-path-below-symbolic")
        .tooltip_text(tr("Text unter dem Pfad statt darüber"))
        .build();
    let path_reverse = gtk::ToggleButton::builder()
        .icon_name("mondrian-path-reverse-symbolic")
        .tooltip_text(tr(
            "Laufrichtung umkehren (Text andersherum, z. B. unten am Kreis lesbar)",
        ))
        .build();
    let path_box = wide(&linked_box(&[&path_below, &path_reverse]));
    g_path.append(&path_box);
    text_group.append(&text_styles_box);
    // gehört zu den Textoptionen, nicht zur Position
    let convert = gtk::Button::with_label(tr("Text in Pfade umwandeln"));
    convert.set_action_name(Some("win.to-path"));
    text_group.append(&wide(&convert));

    // Knoten
    let (node_group, node_card) = section(tr("Knoten"));
    cards.push(node_card.clone());
    let node_corner = gtk::Button::with_label(tr("Eckig"));
    node_corner.set_action_name(Some("win.node-corner"));
    let node_smooth = gtk::Button::with_label(tr("Glatt"));
    node_smooth.set_action_name(Some("win.node-smooth"));
    let node_delete = gtk::Button::from_icon_name("user-trash-symbolic");
    node_delete.set_tooltip_text(Some(tr("Knoten löschen (Entf)")));
    node_delete.set_action_name(Some("win.node-delete"));
    node_card.append(&wide(&linked_box(&[&node_corner, &node_smooth, &node_delete])));
    let node_radius = spin(
        0.0,
        5000.0,
        1.0,
        0,
        tr("Radius für die gewählten Ecken (ohne Auswahl: alle Ecken)"),
    );
    // Eckenradius nur durch Ziehen der runden Punkte an den Ecken
    let _ = &node_radius;

    // Darstellung: Füllung, Kontur, Schatten und Effekte als aufklappbare Zeilen einer Karte
    let style_group = vbox(18);
    let appear = card();
    cards.push(appear.clone());
    let fill_sw = Swatches::new(tr("Füllung"), SwTarget::Paint(PaintTarget::Fill));
    // Bilder: „Füllung“ ist nur ihre Deckkraft
    let image_sw = Swatches::new(tr("Füllung"), SwTarget::ImageOpacity);
    // Farbtemperatur des Bildes: −100 % kalt, 0 % Original, 100 % warm
    let temp = gtk::Scale::with_range(gtk::Orientation::Horizontal, -100.0, 100.0, 5.0);
    temp.set_tooltip_text(Some(tr("Temperatur: −100 % kalt, 0 % Original, 100 % warm")));
    image_sw.extra.append(&slider_row(tr("Temperatur"), &temp));
    let (fill_cmyk, fill_cmyk_scales) = cmyk_block();
    fill_sw.extra.append(&fill_cmyk);
    // Farbverlauf: kommt über den Reiter „Verlauf“ der Farbwahl; die alten Bedienelemente
    // bleiben nur intern bestehen
    let grad_add = gtk::Button::with_label(tr("Farbverlauf hinzufügen"));
    grad_add.set_visible(false);
    let grad_box = vbox(6);
    let grad_bar = gtk::DrawingArea::new();
    grad_bar.set_content_height(38);
    grad_box.append(&grad_bar);
    let grad_color = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::builder().with_alpha(true).build()));
    let grad_remove_stop = gtk::Button::from_icon_name("list-remove-symbolic");
    grad_remove_stop.set_action_name(Some("win.gradient-remove-stop"));
    let grad_angle = spin(-180.0, 180.0, 1.0, 0, tr("Winkel des Verlaufs in Grad"));
    grad_angle.set_wrap(true);
    let kind_names: Vec<&str> = GradientKind::ALL.iter().map(|k| k.label()).collect();
    let grad_kind = gtk::DropDown::from_strings(&kind_names);

    let stroke_sw = Swatches::new(tr("Kontur"), SwTarget::Paint(PaintTarget::Stroke));
    let (stroke_cmyk, stroke_cmyk_scales) = cmyk_block();
    stroke_sw.extra.append(&stroke_cmyk);
    // Stärke, Lage, Linie, Ecken, Enden und Linienenden im aufgeklappten Bereich der Kontur
    let (width_box, width) = unit_spin(0.0, 500.0, 0.5, 1, "px", tr("Linienstärke in px"));
    let g_line = group_box();
    stroke_sw.extra.append(&g_line);
    cards.push(g_line.clone());
    g_line.append(&prow(tr("Linienstärke"), &width_box));
    let stroke_align = [
        (tr("Mitte"), tr("Kontur mittig auf der Linie")),
        (tr("Innen"), tr("Kontur innen")),
        (tr("Außen"), tr("Kontur außen")),
    ]
    .map(|(l, tip)| gtk::ToggleButton::builder().label(l).tooltip_text(tip).build());
    for b in &stroke_align[1..] {
        b.set_group(Some(&stroke_align[0]));
    }
    g_line.append(&wide(&linked_box(&[
        &stroke_align[0],
        &stroke_align[1],
        &stroke_align[2],
    ])));
    // Strichmuster
    let dash_names: Vec<&str> = crate::model::Dash::ALL.iter().map(|d| d.label()).collect();
    let dash = gtk::DropDown::from_strings(&dash_names);
    dash.set_tooltip_text(Some(tr("Durchgezogen, gestrichelt oder gepunktet")));
    let dash_row = prow(tr("Linie"), &dash);
    g_line.append(&dash_row);
    // Ecken und Enden der Kontur
    let toggles3 = |items: [(&str, &str); 3]| {
        let bs = items.map(|(icon, tip)| {
            gtk::ToggleButton::builder()
                .icon_name(icon)
                .tooltip_text(tip)
                .build()
        });
        for b in &bs[1..] {
            b.set_group(Some(&bs[0]));
        }
        bs
    };
    let join = toggles3([
        ("mondrian-join-miter-symbolic", tr("Spitze Ecken")),
        ("mondrian-join-round-symbolic", tr("Runde Ecken")),
        ("mondrian-join-bevel-symbolic", tr("Abgeschrägte Ecken")),
    ]);
    let join_row = prow(tr("Ecken"), &linked_box(&[&join[0], &join[1], &join[2]]));
    g_line.append(&join_row);
    let cap = toggles3([
        ("mondrian-cap-butt-symbolic", tr("Gerade Enden")),
        ("mondrian-cap-round-symbolic", tr("Runde Enden")),
        ("mondrian-cap-square-symbolic", tr("Überstehende Enden")),
    ]);
    let cap_row = prow(tr("Enden"), &linked_box(&[&cap[0], &cap[1], &cap[2]]));
    g_line.append(&cap_row);
    // Linienenden (nur offene Linien)
    let marker_row = group_box();
    let marker_names: Vec<&str> = Marker::ALL.iter().map(|m| m.label()).collect();
    let marker = gtk::DropDown::from_strings(&marker_names);
    marker.set_tooltip_text(Some(tr("Form der Linienenden, z. B. Pfeil")));
    marker_row.append(&prow(tr("Linienende"), &marker));
    let mk_end = |label: &str| gtk::ToggleButton::builder().label(label).build();
    let marker_ends = [mk_end(tr("Anfang")), mk_end(tr("Ende")), mk_end(tr("Beide"))];
    for b in &marker_ends[1..] {
        b.set_group(Some(&marker_ends[0]));
    }
    let marker_ends_box = wide(&linked_box(&[&marker_ends[0], &marker_ends[1], &marker_ends[2]]));
    marker_row.append(&marker_ends_box);
    let (msize_box, marker_size) = unit_spin(10.0, 1000.0, 10.0, 0, "%", tr("Größe der Linienenden"));
    let marker_size_row = prow(tr("Größe"), &msize_box);
    marker_row.append(&marker_size_row);
    stroke_sw.extra.append(&marker_row);
    let outline_button = gtk::Button::with_label(tr("Kontur in Fläche umwandeln"));
    outline_button.set_action_name(Some("win.outline-stroke"));
    outline_button.set_tooltip_text(Some(tr(
        "Die Kontur wird ein eigenes Objekt – skalierbar, mit eigener Füllung und Kontur",
    )));
    stroke_sw.extra.append(&wide(&outline_button));

    // Rahmen eines Textfeldes: Füllung und Kontur stehen unter denen des Textes
    let frame_fill_sw = Swatches::new(tr("Rahmenfüllung"), SwTarget::FrameFill);
    let frame_stroke_sw = Swatches::new(tr("Rahmenkontur"), SwTarget::FrameStroke);
    let (tframe_width_box, tframe_width) =
        unit_spin(0.0, 500.0, 0.5, 1, "px", tr("Linienstärke des Rahmens in px"));
    let tframe_width_row = prow(tr("Linienstärke"), &tframe_width_box);
    frame_stroke_sw.extra.append(&tframe_width_row);
    // (frühere Bedienelemente des Rahmens, nur noch intern)
    let fdlg = gtk::ColorDialog::builder().with_alpha(true).build();
    let tframe_fill_on = gtk::CheckButton::with_label(tr("Rahmenfüllung"));
    let tframe_fill = gtk::ColorDialogButton::new(Some(fdlg.clone()));
    let tframe_stroke_on = gtk::CheckButton::with_label(tr("Rahmenkontur"));
    let tframe_stroke = gtk::ColorDialogButton::new(Some(fdlg));

    // Effekte: jeder angewendete Effekt ist eine eigene aufklappbare Zeile; sie erscheinen
    // erst, wenn man sie über „Effekt anwenden“ hinzufügt
    let remove_button = |label: &str, action: &str| {
        let b = gtk::Button::with_label(label);
        b.add_css_class("mondrian-layer-delete");
        b.set_action_name(Some(action));
        wide(&b)
    };
    let shadow_sw = Swatches::new(tr("Schatten"), SwTarget::Paint(PaintTarget::Shadow));
    let shadow_box = group_box();
    // Deckkraft des Schattens = Deckkraftleiste seiner Farbe
    let (_sop_row, shadow_opacity) = unit_spin(0.0, 100.0, 5.0, 0, "%", tr("Deckkraft des Schattens"));
    let (sbl_box, shadow_blur) = unit_spin(0.0, 500.0, 1.0, 0, "px", tr("Unschärfe des Schattens"));
    shadow_box.append(&prow(tr("Unschärfe"), &sbl_box));
    let (sdx_box, shadow_dx) = unit_spin(-1000.0, 1000.0, 1.0, 0, "px", tr("Versatz nach rechts in px"));
    shadow_box.append(&prow(tr("Versatz X"), &sdx_box));
    let (sdy_box, shadow_dy) = unit_spin(-1000.0, 1000.0, 1.0, 0, "px", tr("Versatz nach unten in px"));
    shadow_box.append(&prow(tr("Versatz Y"), &sdy_box));
    shadow_sw.extra.append(&shadow_box);
    shadow_sw.extra.append(&remove_button(
        tr("Schatten entfernen"),
        "win.effect-remove-shadow",
    ));
    let shadow_section = shadow_sw.root.clone();

    // Deckkraft steht an jeder Farbe (Leiste in der Farbwahl); der Wert bleibt intern
    let (_opacity_row, opacity) = unit_spin(0.0, 100.0, 5.0, 0, "%", tr("Deckkraft in Prozent"));
    // Weitere Effekte je Kategorie (nur sichtbar, wenn angewendet)
    let blend_fold = fx_fold(
        tr("Farbüberlagerung"),
        &[Blend::Screen.label(), Blend::Multiply.label()],
        None,
        None,
        tr("Farbüberlagerung entfernen"),
        "win.effect-remove-blend",
    );
    let tex_labels: Vec<&str> = crate::model::Texture::ALL.iter().map(|a| a.label()).collect();
    let tex_fold = fx_fold(
        tr("Textur"),
        &tex_labels,
        Some(tr("Stärke")),
        None,
        tr("Textur entfernen"),
        "win.effect-remove-tex",
    );
    let art_labels: Vec<&str> = crate::model::Art::ALL.iter().map(|a| a.label()).collect();
    let art_fold = fx_fold(
        tr("Kunsteffekt"),
        &art_labels,
        Some(tr("Stärke")),
        None,
        tr("Kunsteffekt entfernen"),
        "win.effect-remove-art",
    );
    let lens_labels: Vec<&str> = crate::model::Lens::ALL.iter().map(|a| a.label()).collect();
    let lens_fold = fx_fold(
        tr("3D-Effekt"),
        &lens_labels,
        Some(tr("Tiefe")),
        Some(tr("Beleuchtung")),
        tr("3D-Effekt entfernen"),
        "win.effect-remove-lens",
    );
    // Tiefe: −100 % eingedrückt, 0 % Ausgangszustand, 100 % herausgedrückt
    lens_fold.strength.set_range(-100.0, 100.0);
    lens_fold
        .strength
        .set_tooltip_text(Some(tr("Tiefe: −100 % eingedrückt, 100 % herausgedrückt")));
    lens_fold
        .strength2
        .set_tooltip_text(Some(tr("Stärke von Licht und Schatten")));
    let mirror_fold = fx_fold(
        tr("Spiegelung"),
        &[],
        Some(tr("Stärke")),
        None,
        tr("Spiegelung entfernen"),
        "win.effect-remove-mirror",
    );
    let tint_labels: Vec<&str> = crate::model::ImgFilter::ALL.iter().map(|a| a.label()).collect();
    let tint_fold = fx_fold(
        tr("Bildfilter"),
        &tint_labels,
        None,
        None,
        tr("Bildfilter entfernen"),
        "win.effect-remove-tint",
    );
    for r in [
        &fill_sw.root,
        &image_sw.root,
        &stroke_sw.root,
        &frame_fill_sw.root,
        &frame_stroke_sw.root,
        &shadow_section,
        &blend_fold.root,
        &tex_fold.root,
        &art_fold.root,
        &lens_fold.root,
        &tint_fold.root,
        &mirror_fold.root,
    ] {
        appear.append(r);
    }
    // Text zuerst, darunter die Darstellung
    style_group.append(&text_group_outer);
    style_group.append(&appear);

    // Position: Drehwinkel, Drehen und Spiegeln, Ausrichten (steht fest unten in der Leiste)
    let (arrange_group, arrange_card) = section(tr("Position"));
    cards.push(arrange_card.clone());
    let pinned = vbox(0);
    let (angle_row, angle) = unit_spin(
        -180.0,
        180.0,
        1.0,
        1,
        "°",
        tr("Drehwinkel in Grad (gegen den Uhrzeigersinn) – eintippen oder mit − und + ändern"),
    );
    angle.set_wrap(true);
    arrange_card.append(&prow(tr("Drehung"), &angle_row));
    let arr: Vec<gtk::Button> = [
        (
            "object-rotate-left-symbolic",
            tr("90° nach links drehen"),
            "win.rotate-left",
        ),
        (
            "object-rotate-right-symbolic",
            tr("90° nach rechts drehen"),
            "win.rotate-right",
        ),
        (
            "object-flip-horizontal-symbolic",
            tr("Horizontal spiegeln (Umschalt H)"),
            "win.flip-h",
        ),
        (
            "object-flip-vertical-symbolic",
            tr("Vertikal spiegeln (Umschalt V)"),
            "win.flip-v",
        ),
    ]
    .into_iter()
    .map(|(icon, tip, action)| {
        let b = gtk::Button::from_icon_name(icon);
        b.set_tooltip_text(Some(tip));
        b.set_action_name(Some(action));
        b
    })
    .collect();
    arrange_card.append(&wide(&linked_box(&arr.iter().collect::<Vec<_>>())));
    // Ausrichten hier immer an der Seite (aneinander: Schwebeleiste)
    let page_rows = hbox(6);
    page_rows.set_homogeneous(true);
    for items in [
        [
            (
                "mondrian-page-left-symbolic",
                tr("Links ausrichten"),
                "win.page-left",
            ),
            (
                "mondrian-page-hcenter-symbolic",
                tr("Waagerecht mittig ausrichten"),
                "win.page-hcenter",
            ),
            (
                "mondrian-page-right-symbolic",
                tr("Rechts ausrichten"),
                "win.page-right",
            ),
        ],
        [
            (
                "mondrian-page-top-symbolic",
                tr("Oben ausrichten"),
                "win.page-top",
            ),
            (
                "mondrian-page-vcenter-symbolic",
                tr("Senkrecht mittig ausrichten"),
                "win.page-vcenter",
            ),
            (
                "mondrian-page-bottom-symbolic",
                tr("Unten ausrichten"),
                "win.page-bottom",
            ),
        ],
    ] {
        let bs: Vec<gtk::Button> = items
            .into_iter()
            .map(|(icon, tip, action)| {
                let b = gtk::Button::from_icon_name(icon);
                b.set_tooltip_text(Some(tip));
                b.set_action_name(Some(action));
                b
            })
            .collect();
        page_rows.append(&linked_box(&bs.iter().collect::<Vec<_>>()));
    }
    arrange_card.append(&wide(&page_rows));

    // Bild: zuschneiden, in Rahmen setzen
    let (image_group, image_card) = section(tr("Bild"));
    cards.push(image_card.clone());
    let image_crop = gtk::Button::with_label(tr("Zuschneiden"));
    image_crop.set_action_name(Some("win.image-crop"));
    image_crop.set_tooltip_text(Some(tr("Rahmen um das Bild legen und anpassen")));
    image_card.append(&wide(&image_crop));
    let mk_mode = |label: &str, tip: &str| {
        gtk::ToggleButton::builder()
            .label(label)
            .tooltip_text(tip)
            .build()
    };
    let image_modes = [
        mk_mode(tr("Alles"), tr("Rahmen und Bild gemeinsam bearbeiten")),
        mk_mode(tr("Rahmen"), tr("Nur den Rahmen ändern (zuschneiden)")),
        mk_mode(
            tr("Bild"),
            tr("Nur das Bild im Rahmen verschieben oder skalieren"),
        ),
    ];
    for b in &image_modes[1..] {
        b.set_group(Some(&image_modes[0]));
    }
    let image_modes_box = linked_box(&[&image_modes[0], &image_modes[1], &image_modes[2]]);
    image_card.append(&wide(&image_modes_box));
    let image_release = gtk::Button::with_label(tr("Rahmen lösen"));
    image_release.set_action_name(Some("win.image-release"));
    image_card.append(&wide(&image_release));

    // Textfeld in einer Form: Form wieder lösen
    let (tframe_group, tframe_card) = section("");
    cards.push(tframe_card.clone());
    let tframe_release = gtk::Button::with_label(tr("Form lösen"));
    tframe_release.set_action_name(Some("win.text-release"));
    tframe_release.set_tooltip_text(Some(tr(
        "Text wieder rechteckig umbrechen; die Form bleibt als eigenes Objekt",
    )));
    tframe_card.append(&wide(&tframe_release));

    // Hilfslinien
    let (guide_group, guide_card) = section(tr("Hilfslinien"));
    cards.push(guide_card.clone());
    let (guide_angle_box, guide_angle) =
        unit_spin(-180.0, 180.0, 1.0, 1, "°", tr("Winkel der Hilfslinie in Grad"));
    guide_angle.set_wrap(true);
    let guide_angle_row = prow(tr("Winkel"), &guide_angle_box);
    guide_card.append(&guide_angle_row);
    let (guide_pos_box, guide_pos) = unit_spin(
        -100000.0,
        100000.0,
        1.0,
        1,
        "",
        tr("Lage der gewählten Hilfslinie"),
    );
    let guide_pos_label = gtk::Label::new(Some(tr("Position")));
    let guide_pos_row = prow_with(&guide_pos_label, &guide_pos_box);
    guide_card.append(&guide_pos_row);
    let guide_show = gtk::Switch::new();
    // Ein-/Ausblenden geschieht über die Zeile „Hilfslinien“ in der Ebenenliste.
    let show_row = prow(tr("Anzeigen"), &guide_show);
    show_row.set_visible(false);
    guide_card.append(&show_row);
    let guide_snap = gtk::Switch::new();
    guide_card.append(&prow(tr("Einrasten"), &guide_snap));
    let guide_delete = gtk::Button::with_label(tr("Löschen"));
    guide_delete.set_action_name(Some("win.guide-delete"));
    let guide_clear = gtk::Button::with_label(tr("Alle löschen"));
    guide_clear.set_action_name(Some("win.guide-clear"));
    let gb = hbox(6);
    gb.set_homogeneous(true);
    gb.append(&guide_delete);
    gb.append(&guide_clear);
    guide_card.append(&wide(&gb));

    for g in [
        &shape_group,
        &node_group,
        &style_group,
        &tframe_group,
        &image_group,
        &guide_group,
    ] {
        content.append(g);
    }
    // Karten und Listen ohne sichtbare Zeile (samt Überschrift) ausblenden
    for sw in [
        &fill_sw,
        &image_sw,
        &stroke_sw,
        &frame_fill_sw,
        &frame_stroke_sw,
        &shadow_sw,
    ] {
        cards.push(sw.extra.clone());
    }
    for c in &cards {
        auto_hide(c);
    }

    let panel = Panel {
        heading,
        empty_hint,
        shape_group,
        shape_kind,
        corners_row,
        corners,
        radius_row,
        radius,
        text_group: text_group_outer,
        text_hint,
        font,
        size,
        face,
        faces: RefCell::new(Vec::new()),
        faces_family: RefCell::new(String::new()),
        weight,
        weight_row,
        underline,
        strike,
        list_box,
        list_on,
        indent_less,
        indent_more,
        para_row,
        para_spacing,
        columns_row,
        columns,
        small_caps,
        ligatures,
        text_styles_box,
        text_styles,
        list_style,
        path_box,
        path_below,
        path_reverse,
        text_frame,
        text,
        offset_row,
        offset,
        align,
        valign_box,
        valign,
        line_spacing,
        tracking,
        node_group,
        node_radius,
        node_corner,
        node_smooth,
        node_delete,
        style_group,
        fill_sw,
        stroke_sw,
        shadow_sw,
        shadow_box,
        shadow_section,
        blend_fold,
        tex_fold,
        art_fold,
        lens_fold,
        tint_fold,
        mirror_fold,
        image_sw,
        temp,
        stroke_align,
        frame_fill_sw,
        frame_stroke_sw,
        inset_row,
        shadow_opacity,
        shadow_blur,
        shadow_dx,
        shadow_dy,
        width,
        opacity,
        grad_add,
        grad_box,
        grad_bar,
        grad_color,
        grad_remove_stop,
        grad_angle,
        grad_kind,
        fill_cmyk,
        fill_cmyk_scales,
        stroke_cmyk,
        stroke_cmyk_scales,
        arrange_group,
        pinned,
        angle,
        marker_row,
        join_row,
        dash_row,
        dash,
        join,
        cap_row,
        cap,
        marker,
        marker_ends_box,
        marker_ends,
        marker_size_row,
        marker_size,
        tframe_group,
        tframe_fill_on,
        tframe_fill,
        tframe_stroke_on,
        tframe_stroke,
        tframe_width_row,
        tframe_width,
        tframe_inset,
        tframe_release,
        image_group,
        image_crop,
        image_modes_box,
        image_modes,
        image_release,
        convert,
        guide_group,
        guide_angle_row,
        guide_angle,
        guide_pos_row,
        guide_pos_label,
        guide_pos,
        guide_show,
        guide_snap,
        guide_delete,
    };
    (content, panel)
}

impl Ui {
    pub fn new(app: &adw::Application) -> Rc<Ui> {
        // Kopfleiste: [↶][↷]   Titel   [+][☰]
        let header = adw::HeaderBar::new();
        let undo_button = action_button(
            "edit-undo-symbolic",
            &key_tip(tr("Rückgängig"), "undo"),
            "win.undo",
        );
        let redo_button = action_button(
            "edit-redo-symbolic",
            &key_tip(tr("Wiederholen"), "redo"),
            "win.redo",
        );
        // oben links: Ebenen und Bibliothek – je ein Knopf öffnet die linke Seitenleiste mit
        // diesem Inhalt (ein zweiter Klick schließt sie)
        let lib = build_library_panel();
        let layers_toggle = gtk::ToggleButton::builder()
            .icon_name("mondrian-layers-symbolic")
            .tooltip_text(key_tip(tr("Ebenen und Zeichenflächen"), "toggle-sidebar"))
            .build();
        lib.toggle.remove_css_class("flat");
        lib.toggle.set_margin_start(0);
        lib.toggle.set_margin_end(0);
        lib.toggle.set_margin_bottom(0);
        lib.toggle.set_tooltip_text(Some(tr("Bibliothek")));
        // Zustand „linke Seitenleiste offen“ (unsichtbar; F8 und schmale Fenster nutzen ihn)
        let side_toggle = gtk::ToggleButton::new();
        header.pack_start(&layers_toggle);
        header.pack_start(&lib.toggle);
        let gap = gtk::Separator::new(gtk::Orientation::Vertical);
        gap.add_css_class("spacer");
        header.pack_start(&gap);
        header.pack_start(&undo_button);
        header.pack_start(&redo_button);

        // Mitte: Name des Dokuments, darunter Speicherort und Format
        let title = adw::WindowTitle::new("Stijl", "");
        header.set_title_widget(Some(&title));

        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text(tr("Hauptmenü"))
            .menu_model(&main_menu())
            .primary(true)
            .build();
        header.pack_end(&menu_button);
        header.pack_end(&action_button(
            "tab-new-symbolic",
            &key_tip(tr("Neues Dokument"), "new"),
            "win.new",
        ));
        // oben rechts: Eigenschaften ein-/ausblenden
        let props_toggle = gtk::ToggleButton::builder()
            .icon_name("mondrian-props-symbolic")
            .tooltip_text(key_tip(tr("Eigenschaften"), "toggle-panel"))
            .active(true)
            .build();
        header.pack_end(&props_toggle);
        // daneben: Informationen zum Dokument (wie in der Bildbetrachtung)
        let info_toggle = gtk::ToggleButton::builder()
            .icon_name("help-about-symbolic")
            .tooltip_text(tr("Informationen zum Dokument"))
            .build();
        header.pack_end(&info_toggle);
        let info = build_info_panel();

        // Tabs (nur sichtbar, wenn mehr als ein Dokument offen ist)
        let tab_view = adw::TabView::new();
        let tab_bar = adw::TabBar::new();
        tab_bar.set_view(Some(&tab_view));
        tab_bar.set_autohide(true);

        // Werkzeuge: schwebende Leiste oben links über der Zeichenfläche
        let tools_box = osd_bar();
        tools_box.set_halign(gtk::Align::Start);
        tools_box.set_valign(gtk::Align::Start);
        let mut slots: Vec<Rc<Slot>> = Vec::new();
        let mut first: Option<gtk::ToggleButton> = None;
        for variants in slot_definitions() {
            // Gruppe: zugeklappt ist nur die zuletzt benutzte Variante zu sehen
            let group = hbox(0);
            group.add_css_class("mondrian-tool-group");
            let mut buttons = Vec::new();
            let mut revealers = Vec::new();
            let mut arrows = Vec::new();
            for (i, v) in variants.iter().enumerate() {
                let b = gtk::ToggleButton::new();
                if variants.len() > 1 {
                    // Symbol und kleiner Pfeil nach rechts: Klick klappt die Gruppe auf
                    let content = hbox(2);
                    content.set_halign(gtk::Align::Center);
                    content.append(&gtk::Image::from_icon_name(v.icon));
                    let arrow = gtk::Image::from_icon_name("pan-end-symbolic");
                    arrow.set_pixel_size(10);
                    arrow.add_css_class("dim-label");
                    content.append(&arrow);
                    b.set_child(Some(&content));
                    arrows.push(arrow);
                } else {
                    b.set_icon_name(v.icon);
                }
                b.add_css_class("flat");
                b.set_tooltip_text(Some(v.label));
                b.set_active(v.tool == Tool::Select);
                b.set_margin_start(1);
                b.set_margin_end(1);
                if let Some(f) = &first {
                    b.set_group(Some(f));
                } else {
                    first = Some(b.clone());
                }
                let r = gtk::Revealer::builder()
                    .transition_type(gtk::RevealerTransitionType::SlideRight)
                    .transition_duration(150)
                    .reveal_child(i == 0)
                    .child(&b)
                    .build();
                group.append(&r);
                buttons.push(b);
                revealers.push(r);
            }
            tools_box.append(&group);
            slots.push(Rc::new(Slot {
                buttons,
                revealers,
                arrows,
                group,
                variants,
                current: Cell::new(0),
            }));
        }
        // Werkzeuge und darunter ihre Einstellungen in einer Spalte oben links
        tools_box.set_spacing(0);
        tools_box.set_margin_top(0);
        tools_box.set_margin_bottom(0);
        tools_box.set_margin_start(0);
        tools_box.set_margin_end(0);
        let tool_opts = build_tool_opts();
        let tools_col = vbox(0);
        tools_col.set_halign(gtk::Align::Start);
        tools_col.set_valign(gtk::Align::Start);
        tools_col.set_margin_top(12);
        tools_col.set_margin_start(12);
        tools_col.append(&tools_box);
        tools_col.append(&tool_opts.bar);
        let tools_box = tools_col;

        // Zoom: schwebende Leiste unten rechts (wie in der Bildbetrachtung)
        let zoom_label = gtk::Button::with_label("100 %");
        zoom_label.add_css_class("flat");
        zoom_label.add_css_class("numeric");
        zoom_label.set_tooltip_text(Some(&key_tip(tr("Originalgröße"), "zoom-100")));
        zoom_label.set_action_name(Some("win.zoom-100"));
        {
            let css = gtk::CssProvider::new();
            css.load_from_string(
                ".mondrian-osd { border-radius: 12px; padding: 6px; }\n\
                 .mondrian-osd separator { margin: 6px 4px; }\n\
                 .mondrian-tool-group { border-radius: 9px; transition: background-color 150ms; }\n\
                 .mondrian-tool-group-open { background-color: alpha(currentColor, 0.1); }\n\
                 flowbox.mondrian-cards > flowboxchild, flowbox.mondrian-cards > flowboxchild:hover, \
                   flowbox.mondrian-cards > flowboxchild:focus, flowbox.mondrian-cards > flowboxchild:selected \
                   { padding: 0; background: none; outline: none; box-shadow: none; }\n\
                 .mondrian-overlay-sidebar { background-color: @window_bg_color; \
                   box-shadow: 0 0 0 1px alpha(black, 0.07), 0 2px 10px alpha(black, 0.22); }\n\
                 .mondrian-layer-delete { background-color: alpha(@destructive_bg_color, 0.18); color: @destructive_color; }\n\
                 .mondrian-float { background-color: @popover_bg_color; color: @popover_fg_color; border-radius: 10px; padding: 3px; \
                   box-shadow: 0 0 0 1px alpha(black, 0.07), 0 1px 3px alpha(black, 0.18), 0 3px 12px alpha(black, 0.12); }\n\
                 popover.mondrian-fxmenu > contents { min-width: 220px; }\n\
                 button.mondrian-fx label { font-family: serif; font-style: italic; font-weight: 700; font-size: 1.15em; }\n\
                 scale.mondrian-slider trough { min-height: 14px; border-radius: 7px; background-color: alpha(currentColor, 0.15); }\n\
                 scale.mondrian-slider trough highlight, scale.mondrian-slider trough fill { background: none; }\n\
                 scale.mondrian-slider slider { background-color: @accent_bg_color; background-image: none; \
                   box-shadow: inset 0 0 0 3px white, 0 0 0 1px alpha(black, 0.35); border: none; }\n\
                 .mondrian-rows > *:not(:first-child) { border-top: 1px solid alpha(currentColor, 0.1); }\n\
                 .mondrian-prow { min-height: 42px; padding: 2px 12px; }\n\
                 .mondrian-prow-wide { padding-top: 6px; padding-bottom: 6px; }\n\
                 .mondrian-group { padding: 5px 0; }\n\
                 .mondrian-group > .mondrian-prow { min-height: 36px; padding-top: 1px; padding-bottom: 1px; }\n\
                 .mondrian-group > .mondrian-prow-wide { padding-top: 4px; padding-bottom: 4px; }\n\
                 .mondrian-pad { padding: 10px 12px; }\n\
                 .mondrian-fold-head { min-height: 46px; padding: 0 8px 0 12px; }\n\
                 image.mondrian-empty-hint { opacity: 0.35; }\n\
                 button.mondrian-pattern-btn { padding: 3px; border-radius: 9px; }\n\
                 button.mondrian-pattern-btn:checked { box-shadow: inset 0 0 0 2px @accent_bg_color; background: none; }\n\
                 label.mondrian-panel-title { font-size: 1.15em; font-weight: 800; }\n\
                 .mondrian-narrow > button { padding-left: 3px; padding-right: 3px; min-width: 0; }\n\
                 row.mondrian-layer-row { transition: margin 180ms ease-in-out, opacity 180ms; }\n\
                 row.mondrian-dragging { opacity: 0.35; }\n\
                 row.mondrian-drop-above { margin-top: 48px; border-radius: 0; box-shadow: inset 0 3px 0 0 @accent_bg_color; }\n\
                 row.mondrian-drop-below { margin-bottom: 48px; border-radius: 0; box-shadow: inset 0 -3px 0 0 @accent_bg_color; }\n\
                 row.mondrian-drop-into { background-color: alpha(@accent_bg_color, 0.08); }\n\
                 .mondrian-drag-card { border-radius: 12px; background-color: @card_bg_color; \
                   box-shadow: 0 0 0 1px alpha(black, 0.08), 0 4px 14px alpha(black, 0.25); }\n\
                 button.mondrian-layer-btn { min-height: 24px; min-width: 24px; padding: 0; }\n\
                 button.mondrian-layer-btn:checked { background-color: transparent; }\n\
                 button.mondrian-layer-btn:checked:hover { background-color: alpha(currentColor, 0.08); }\n\
                 button.mondrian-layer-btn.mondrian-layer-off, button.mondrian-layer-btn.mondrian-layer-off:checked { color: alpha(currentColor, 0.45); }\n\
                 button.mondrian-layer-trash:hover { color: @destructive_color; background-color: alpha(@destructive_bg_color, 0.12); }\n\
                 .mondrian-tab { border-radius: 0; box-shadow: inset 0 -1px alpha(currentColor, 0.15); }\n\
                 .mondrian-tab:checked { background: none; box-shadow: inset 0 -3px @accent_bg_color; font-weight: bold; }\n\
                 .mondrian-hex { border: 1px solid alpha(currentColor, 0.15); border-radius: 8px; padding: 2px 2px 2px 8px; }\n\
                 .mondrian-hex entry { background: none; box-shadow: none; outline: none; }\n\
                 button.mondrian-swatch-add { min-width: 0; min-height: 0; padding: 0; background: none; box-shadow: none; \
                   border: 1px dashed alpha(currentColor, 0.35); border-radius: 999px; }\n\
                 button.mondrian-swatch-add:hover { background: alpha(currentColor, 0.07); }\n\
                 .mondrian-lib-add > button { min-width: 0; min-height: 0; padding: 0; background: none; box-shadow: none; \
                   border: 1px dashed alpha(currentColor, 0.35); border-radius: 8px; }\n\
                 .mondrian-lib-add > button:hover { background: alpha(currentColor, 0.07); }\n\
                 .mondrian-lib-add-round > button { border-radius: 999px; }\n\
                 button.mondrian-dash-card { background: none; box-shadow: none; \
                   border: 1px dashed alpha(currentColor, 0.35); border-radius: 12px; }\n\
                 button.mondrian-dash-card:hover { background: alpha(currentColor, 0.07); }\n\
                 .mondrian-float button { min-height: 28px; min-width: 28px; padding: 2px 5px; margin: 0 1px; }",
            );
            if let Some(display) = gtk::gdk::Display::default() {
                gtk::style_context_add_provider_for_display(
                    &display,
                    &css,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
            }
        }
        // Ansicht: schwebende Leiste unten links – Umschalter Zeichenfläche/Notizheft, im
        // Notizheft dazu Blättern mit Seitenanzeige
        let page_label = gtk::Label::new(Some(tr("Seite 1 von 1")));
        page_label.add_css_class("numeric");
        page_label.set_margin_start(4);
        page_label.set_margin_end(4);
        let page_box = hbox(0);
        page_box.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        page_box.append(&action_button(
            "go-previous-symbolic",
            tr("Vorige Seite (Bild ↑)"),
            "win.page-prev",
        ));
        page_box.append(&page_label);
        page_box.append(&action_button(
            "go-next-symbolic",
            tr("Nächste Seite (Bild ↓)"),
            "win.page-next",
        ));
        page_box.set_visible(false);
        let notebook_btn = gtk::ToggleButton::builder()
            .icon_name("mondrian-notebook-symbolic")
            .tooltip_text(key_tip(
                tr("Notizheft: eine Seite nach der anderen"),
                "view-notebook",
            ))
            .action_name("win.notebook")
            .build();
        notebook_btn.add_css_class("flat");
        let view_bar = osd_bar();
        view_bar.set_halign(gtk::Align::Start);
        view_bar.set_valign(gtk::Align::End);
        view_bar.append(&notebook_btn);
        view_bar.append(&page_box);
        let zoom_bar = osd_bar();
        zoom_bar.set_halign(gtk::Align::End);
        zoom_bar.set_valign(gtk::Align::End);
        zoom_bar.append(&action_button(
            "zoom-out-symbolic",
            &key_tip(tr("Verkleinern"), "zoom-out"),
            "win.zoom-out",
        ));
        zoom_bar.append(&zoom_label);
        zoom_bar.append(&action_button(
            "zoom-in-symbolic",
            &key_tip(tr("Vergrößern"), "zoom-in"),
            "win.zoom-in",
        ));
        zoom_bar.append(&action_button(
            "zoom-fit-best-symbolic",
            &key_tip(tr("Ganze Seite"), "zoom-fit"),
            "win.zoom-fit",
        ));
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&tab_view));
        overlay.set_hexpand(true);
        overlay.set_vexpand(true);
        for bar in [&tools_box, &view_bar, &zoom_bar] {
            bar.set_visible(false);
            overlay.add_overlay(bar);
        }

        // Rechte Seitenleiste: Eigenschaften oben, Ebenen unten
        let (panel_content, panel) = build_panel();
        // External: die Breite bestimmt die Leiste, nicht der Inhalt (bleibt beim Wechsel
        // zwischen Objekten gleich breit)
        // Never: die Leiste ist mindestens so breit wie ihr Inhalt – nichts wird abgeschnitten
        let props_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .min_content_height(100)
            .child(&panel_content)
            .build();
        // Mausrad in den Eigenschaften: immer die Leiste rollen, nie versehentlich Zahlen,
        // Schieberegler oder Auswahllisten verstellen.
        {
            let sc = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
            sc.set_propagation_phase(gtk::PropagationPhase::Capture);
            let sw = props_scroll.clone();
            sc.connect_scroll(move |c, _dx, dy| {
                let adj = sw.vadjustment();
                let step = if c.unit() == gdk::ScrollUnit::Wheel {
                    adj.page_size().powf(2.0 / 3.0)
                } else {
                    1.0
                };
                let max = (adj.upper() - adj.page_size()).max(adj.lower());
                adj.set_value((adj.value() + dy * step).clamp(adj.lower(), max));
                glib::Propagation::Stop
            });
            props_scroll.add_controller(sc);
        }

        let layers_list = gtk::ListBox::new();
        layers_list.set_selection_mode(gtk::SelectionMode::Single);
        layers_list.add_css_class("navigation-sidebar");
        let layers_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vexpand(true)
            .min_content_height(120)
            .child(&layers_list)
            .build();
        let artboards_list = gtk::ListBox::new();
        artboards_list.set_selection_mode(gtk::SelectionMode::None);
        artboards_list.add_css_class("navigation-sidebar");
        let artboards_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(220)
            .child(&artboards_list)
            .build();
        let layers_title = gtk::Label::builder().label(tr("Ebenen")).xalign(0.0).build();
        layers_title.add_css_class("mondrian-panel-title");
        layers_title.set_margin_top(12);
        layers_title.set_margin_start(12);
        layers_title.set_margin_bottom(4);
        // „Neue Ebene“ bzw. „Zeichenfläche“ jeweils unter ihrem Bereich
        let add_button = |label: &str, action: &str, tip: &str| {
            let content = hbox(6);
            content.set_halign(gtk::Align::Center);
            content.append(&gtk::Image::from_icon_name("list-add-symbolic"));
            content.append(&gtk::Label::new(Some(label)));
            let b = gtk::Button::builder()
                .child(&content)
                .action_name(action)
                .tooltip_text(tip)
                .build();
            b.set_margin_start(12);
            b.set_margin_end(12);
            b.set_margin_top(6);
            b.set_margin_bottom(10);
            b
        };
        let new_layer_button = add_button(
            tr("Neue Ebene"),
            "win.layer-add",
            tr("Neue Ebene über der aktuellen anlegen"),
        );
        let new_board_button = add_button(
            tr("Zeichenfläche"),
            "win.artboard-add",
            tr("Weitere Zeichenfläche rechts daneben anlegen"),
        );
        let boards_title = gtk::Label::builder()
            .label(tr("Zeichenflächen"))
            .xalign(0.0)
            .tooltip_text(tr("Solange diese Leiste offen ist, lassen sich Zeichenflächen am Namen verschieben und an den Anfassern in der Größe ändern"))
            .build();
        boards_title.add_css_class("mondrian-panel-title");
        boards_title.set_margin_top(12);
        boards_title.set_margin_start(12);
        boards_title.set_margin_bottom(4);

        // Eigenschaften oben, Ebenen unten; die Trennlinie lässt sich mit der Maus verschieben
        // und behält ihre Lage, egal wie viel gerade in den Eigenschaften steht.
        let layers_part = vbox(0);
        layers_part.append(&layers_title);
        layers_part.append(&layers_scroll);
        layers_part.append(&new_layer_button);
        layers_part.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        layers_part.append(&boards_title);
        layers_part.append(&artboards_scroll);
        layers_part.append(&new_board_button);
        layers_part.set_size_request(-1, 140);
        // Linke Leiste: Eigenschaften, Ebenen oder Bibliothek – immer nur eine offen
        let dock_stack = gtk::Stack::new();
        // alle drei Leisten gleich breit (Eigenschaften, Ebenen, Bibliothek)
        dock_stack.set_hhomogeneous(true);
        // Position steht fest unten unter einer Trennlinie (verrutscht nicht mit dem Inhalt)
        let props_page = vbox(0);
        props_page.append(&props_scroll);
        panel
            .pinned
            .append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        panel.arrange_group.set_margin_start(12);
        panel.arrange_group.set_margin_end(12);
        panel.arrange_group.set_margin_top(10);
        panel.arrange_group.set_margin_bottom(12);
        panel.pinned.append(&panel.arrange_group);
        panel.pinned.set_vexpand(false);
        props_page.append(&panel.pinned);
        dock_stack.add_named(&props_page, Some("props"));
        dock_stack.add_named(&info.page, Some("info"));
        dock_stack.set_size_request(PANEL_MIN - 20, -1);
        dock_stack.set_hexpand(true);
        // rechte Leiste: Eigenschaften
        let dock = hbox(0);
        dock.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        dock.append(&dock_stack);
        dock.set_visible(false);
        // linke Leiste: Ebenen oder Bibliothek (Reiter oben)
        let side_stack = gtk::Stack::new();
        side_stack.set_hhomogeneous(true);
        side_stack.add_named(&layers_part, Some("layers"));
        side_stack.add_named(&lib.panel, Some("library"));
        side_stack.set_size_request(PANEL_MIN - 20, -1);
        side_stack.set_hexpand(true);
        side_stack.set_vexpand(true);
        let side_inner = vbox(0);
        side_inner.append(&side_stack);
        let side_dock = hbox(0);
        side_dock.append(&side_inner);
        side_dock.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        side_dock.set_visible(false);

        let middle = hbox(0);
        // Zeichenfläche | Eigenschaften: die Leiste lässt sich verbreitern und behält ihre
        // Breite, wenn sich das Fenster ändert
        let props_paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        props_paned.set_start_child(Some(&overlay));
        props_paned.set_resize_start_child(true);
        props_paned.set_shrink_start_child(false);
        props_paned.set_end_child(Some(&dock));
        props_paned.set_resize_end_child(false);
        props_paned.set_shrink_end_child(false);
        props_paned.set_hexpand(true);
        // Ebenen/Bibliothek | Rest: die Leiste lässt sich verbreitern
        let side_paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        side_paned.set_start_child(Some(&side_dock));
        side_paned.set_resize_start_child(false);
        side_paned.set_shrink_start_child(false);
        side_paned.set_resize_end_child(true);
        side_paned.set_shrink_end_child(false);
        side_paned.set_hexpand(true);
        side_paned.set_end_child(Some(&props_paned));
        middle.append(&side_paned);
        middle.set_vexpand(true);
        let body = vbox(0);
        body.append(&middle);
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&body));
        let editor_only: Vec<gtk::Widget> = vec![
            undo_button.upcast(),
            redo_button.upcast(),
            layers_toggle.clone().upcast(),
            lib.toggle.clone().upcast(),
            gap.clone().upcast(),
            props_toggle.clone().upcast(),
            info_toggle.clone().upcast(),
        ];
        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&header);
        toolbar_view.add_top_bar(&tab_bar);
        // Trennlinie unter Kopf- und Tableiste (wie über der Statusleiste)
        toolbar_view.set_top_bar_style(adw::ToolbarStyle::RaisedBorder);
        toolbar_view.set_content(Some(&toasts));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .default_width(1300)
            .default_height(840)
            .content(&toolbar_view)
            .build();
        window.set_size_request(760, 480);

        let settings = load_settings();
        let libraries = load_libraries();
        let (unit, cmyk) = (settings.unit, settings.cmyk);
        let ui = Rc::new(Ui {
            window,
            tab_view,
            docs: RefCell::new(Vec::new()),
            current: RefCell::new(None),
            metrics: CairoMetrics::new(),
            toasts,
            title,
            slots,
            tools_bar: tools_box.clone(),
            tool_opts,
            view_bar,
            zoom_bar,
            canvas_overlay: overlay.clone(),
            narrow: Cell::new(false),
            dock,
            dock_stack,
            props_paned,
            side_dock,
            side_stack,
            side_paned,
            side_width: Cell::new(settings.side_width),
            props_toggle,
            info_toggle,
            info,
            side_toggle,
            layers_toggle,
            side_last: Cell::new("layers"),
            panel,
            layers_list,
            artboards_list,
            layer_rows: RefCell::new(Vec::new()),
            panel_width: Cell::new(settings.panel_width),
            lib_current: Cell::new(
                libraries
                    .iter()
                    .position(|l| l.name == settings.library)
                    .unwrap_or(0),
            ),
            libraries: RefCell::new(libraries),
            lib,
            props_width: Cell::new(settings.library_width),
            save_pending: Cell::new(false),
            clipboard: RefCell::new((0, Vec::new())),
            thumbs: RefCell::new(Vec::new()),
            thumb_frame: Cell::new(None),
            layers_shown: RefCell::new((0, 0, Vec::new())),
            layers_shown_active: Cell::new(0),
            layer_drag: Cell::new(0),
            sel_target: Cell::new(0),
            layer_mark: RefCell::new(None),
            zoom_label,
            page_box,
            page_label,
            updating: Cell::new(false),
            pointer: Cell::new((0.0, 0.0)),
            export: RefCell::new(ExportSettings {
                format: 0,
                background: true,
                scale: 1,
                quality: 90.0,
                embed_fonts: false,
                effect_quality: 1,
                pdf_text: true,
                color: 0,
                profile: None,
                bleed: 0.0,
            }),
            scheme: Cell::new(settings.scheme),
            unit: Cell::new(unit),
            cmyk: Cell::new(cmyk),
            backdrop: Cell::new(settings.backdrop),
            show_rulers: Cell::new(settings.rulers),
            notebook: Cell::new(settings.notebook),
            language: RefCell::new(settings.language.clone()),
            updates: Cell::new(settings.updates),
            update_checked: Cell::new(settings.update_checked),
            show_guides: Cell::new(settings.guides),
            snap_guides: Cell::new(settings.snap),
            snap_corners: Cell::new(settings.snap_corners),
            snap_mids: Cell::new(settings.snap_mids),
            snap_anchors: Cell::new(settings.snap_anchors),
            snap_page: Cell::new(settings.snap_page),
            snap_paths: Cell::new(settings.snap_paths),
            snap_spacing: Cell::new(settings.spacing),
            grid_snap: Cell::new(settings.grid),
            grid_size: Cell::new(settings.grid_size),
            frame_dwell: Cell::new(settings.dwell),
            split: Cell::new(settings.split),
            tool: Cell::new(Tool::Select),
            closing: Cell::new(false),
            welcome: Cell::new(false),
            starts: RefCell::new(Vec::new()),
            pipette: RefCell::new(None),
            pipette_at: Cell::new(None),
            editor_only,
        });

        setup_actions(&ui);
        setup_tools(&ui);
        setup_panel(&ui);
        setup_library(&ui);
        setup_layers(&ui);
        setup_tabs(&ui);
        setup_window(&ui);
        setup_dock(&ui);
        add_start_tab(&ui);
        setup_recovery(&ui);
        label_icon_buttons(ui.window.upcast_ref());
        ui
    }

    pub fn present(&self) {
        self.window.present();
    }

    fn toast(&self, msg: &str) {
        let t = adw::Toast::new(msg);
        t.set_timeout(3);
        self.toasts.add_toast(t);
    }
}

/// Dokument des gerade gewählten Tabs (Start-Tabs haben keins).
fn cur(ui: &Ui) -> Option<Rc<DocTab>> {
    let page = ui.tab_view.selected_page()?;
    doc_for_page(ui, &page)
}

/// Einrast- und Einsetzeinstellungen auf einen Editor übertragen.
fn apply_snap_settings(ui: &Ui, ed: &mut Editor) {
    ed.snap_corners = ui.snap_corners.get();
    ed.snap_mids = ui.snap_mids.get();
    ed.snap_anchors = ui.snap_anchors.get();
    ed.snap_page = ui.snap_page.get();
    ed.snap_paths = ui.snap_paths.get();
    // die Einrastziele werden nur gesammelt, wenn eines davon oder die Abstände gebraucht werden
    ed.smart_snap = ed.snap_corners
        || ed.snap_mids
        || ed.snap_anchors
        || ed.snap_page
        || ed.snap_paths
        || ui.snap_spacing.get();
    ed.snap_spacing = ui.snap_spacing.get();
    ed.grid_snap = ui.grid_snap.get();
    ed.grid_size = ui.grid_size.get();
    ed.frame_dwell = ui.frame_dwell.get();
}

fn apply_snap_settings_all(ui: &Ui) {
    for d in ui.docs.borrow().iter() {
        apply_snap_settings(ui, &mut d.ed.borrow_mut());
        d.canvas.queue_draw();
    }
}

// ───────────────────────────── Dokumente / Tabs ─────────────────────────────

fn add_doc(ui: &Rc<Ui>, doc: Document, file: Option<PathBuf>) -> Rc<DocTab> {
    let canvas = gtk::DrawingArea::new();
    canvas.set_hexpand(true);
    canvas.set_vexpand(true);
    canvas.set_focusable(true);
    let mut ed = Editor::new(doc);
    ed.set_tool(ui.tool.get());
    ed.show_guides = ui.show_guides.get();
    ed.snap_guides = ui.snap_guides.get();
    ed.artboard_mode = artboard_mode(ui);
    apply_snap_settings(ui, &mut ed);
    let im = gtk::IMMulticontext::new();
    im.set_client_widget(Some(&canvas));
    let float = build_float_bar();
    let holder = gtk::Overlay::new();
    holder.set_child(Some(&canvas));
    holder.add_overlay(&float.bar);
    let d = Rc::new(DocTab {
        ed: RefCell::new(ed),
        file: RefCell::new(file),
        title: RefCell::new(None),
        canvas,
        page: RefCell::new(None),
        keys: gtk::EventControllerKey::new(),
        im,
        im_on: Cell::new(false),
        ticking: Cell::new(false),
        origin: Cell::new(None),
        holder,
        float,
        recovery_id: next_recovery_id(),
        recovery_edits: Cell::new(None),
        view_seen: Cell::new((0.0, 0.0, 0.0)),
        view_changed: Cell::new(None),
        settle_pending: Cell::new(false),
    });
    setup_canvas(ui, &d);
    // Aus einem Start-Tab heraus: das Dokument ersetzt diesen Tab.
    let start_page = ui
        .tab_view
        .selected_page()
        .filter(|p| ui.starts.borrow().iter().any(|st| st.page == *p));
    let page = match &start_page {
        Some(sp) => ui.tab_view.insert(&d.holder, ui.tab_view.page_position(sp)),
        None => ui.tab_view.append(&d.holder),
    };
    page.set_title(&d.name());
    *d.page.borrow_mut() = Some(page.clone());
    ui.docs.borrow_mut().push(d.clone());
    ui.tab_view.set_selected_page(&page);
    // Falls das Signal nicht feuert (erster Tab), selbst umschalten.
    switch_to(ui, &page);
    if let Some(sp) = start_page {
        ui.tab_view.close_page(&sp);
    }
    d
}

fn doc_for_page(ui: &Ui, page: &adw::TabPage) -> Option<Rc<DocTab>> {
    ui.docs
        .borrow()
        .iter()
        .find(|d| d.page.borrow().as_ref() == Some(page))
        .cloned()
}

fn switch_to(ui: &Rc<Ui>, page: &adw::TabPage) {
    let Some(d) = doc_for_page(ui, page) else {
        // Start-Tab: kein Dokument, Liste „Zuletzt geöffnet“ auffrischen
        *ui.current.borrow_mut() = None;
        refresh_starts(ui);
        refresh(ui);
        return;
    };
    let same = ui
        .current
        .borrow()
        .as_ref()
        .map(|c| Rc::ptr_eq(c, &d))
        .unwrap_or(false);
    if !same {
        d.ed.borrow_mut().set_tool(ui.tool.get());
        *ui.current.borrow_mut() = Some(d.clone());
        ui.layers_shown.replace((0, 0, Vec::new()));
    }
    refresh(ui);
    d.canvas.grab_focus();
}

fn setup_tabs(ui: &Rc<Ui>) {
    {
        let u = ui.clone();
        ui.tab_view.connect_selected_page_notify(move |v| {
            if let Some(p) = v.selected_page() {
                switch_to(&u, &p);
            }
        });
    }
    {
        let u = ui.clone();
        ui.tab_view.connect_close_page(move |view, page| {
            let Some(d) = doc_for_page(&u, page) else {
                view.close_page_finish(page, true);
                return glib::Propagation::Stop;
            };
            if !d.ed.borrow().dirty {
                view.close_page_finish(page, true);
                return glib::Propagation::Stop;
            }
            let (view2, page2) = (view.clone(), page.clone());
            confirm_discard(
                &u,
                &d,
                Box::new(move |_: &Rc<Ui>| {
                    view2.close_page_finish(&page2, true);
                }),
                Some(Box::new({
                    let (view3, page3) = (view.clone(), page.clone());
                    move |_: &Rc<Ui>| view3.close_page_finish(&page3, false)
                })),
            );
            glib::Propagation::Stop
        });
    }
    {
        let u = ui.clone();
        ui.tab_view.connect_page_detached(move |view, page, _| {
            let removed = doc_for_page(&u, page);
            // geschlossenes Dokument: Wiederherstellungsdatei wird nicht mehr gebraucht
            if let Some(r) = &removed {
                drop_recovery(r);
            }
            let was_start = u.starts.borrow().iter().any(|st| st.page == *page);
            u.starts.borrow_mut().retain(|st| st.page != *page);
            u.docs
                .borrow_mut()
                .retain(|d| d.page.borrow().as_ref() != Some(page));
            let prev = u.current.borrow().clone();
            let was_current = match (&removed, prev) {
                (Some(r), Some(c)) => Rc::ptr_eq(r, &c),
                _ => false,
            };
            if was_current {
                *u.current.borrow_mut() = None;
            }
            if view.n_pages() == 0 {
                *u.current.borrow_mut() = None;
                if !u.closing.get() {
                    if was_start {
                        // Letzter Tab war die Startseite: Fenster schließen
                        u.closing.set(true);
                        u.window.close();
                    } else {
                        // Letztes Dokument geschlossen: Startseite zeigen
                        add_start_tab(&u);
                    }
                }
            } else if let Some(p) = view.selected_page() {
                switch_to(&u, &p);
            }
        });
    }
}

/// Datei öffnen: ersetzt ein leeres Dokument, sonst neuer Tab.
/// Dateien, die nicht SVG sind, aber geöffnet und wie SVG behandelt werden:
/// Illustrator-Dateien (PDF-kompatibel) und PDF.
fn is_import_file(path: &Path) -> bool {
    path.extension()
        .map(|e| e.eq_ignore_ascii_case("ai") || e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

/// Kleingeschriebene Namen aller installierten Schriftfamilien.
fn installed_families() -> std::collections::HashSet<String> {
    use pango::prelude::FontFamilyExt as _;
    let ctx = gtk::Label::new(None).pango_context();
    ctx.list_families()
        .iter()
        .map(|f| f.name().to_lowercase())
        .collect()
}

/// Liest eine Datei als Dokument. Der zweite Wert nennt Hinweise zum Import (leer bei SVG).
fn read_document(path: &Path) -> Result<(Document, Vec<String>), String> {
    if is_import_file(path) {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        if !ai::looks_like_ai(&bytes) {
            return Err(tr("Das ist keine Illustrator- oder PDF-Datei").into());
        }
        let known = installed_families();
        let imp = ai::import(&bytes, &|f: &str| known.contains(&f.to_lowercase()))?;
        let doc = svg::read(&imp.svg)?;
        Ok((doc, imp.notes))
    } else {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        // auch Dateien, die nicht in UTF-8 gespeichert sind (dann als ISO-8859-1 gelesen)
        let s =
            String::from_utf8(bytes).unwrap_or_else(|e| e.as_bytes().iter().map(|&b| b as char).collect());
        svg::read_with(&s, path.parent())
    }
}

pub fn open_path(ui: &Rc<Ui>, path: &Path) {
    let (doc, notes) = match read_document(path) {
        Ok(r) => r,
        Err(e) => {
            ui.toast(&trf("Öffnen fehlgeschlagen: {}", &[&(e)]));
            if !path.exists() {
                remove_recent(path);
                refresh_starts(ui);
            }
            return;
        }
    };
    // SVG-Datei eines anderen Programms mit Inhalten, die Stijl nicht übernimmt: wie ein
    // Import behandeln, damit das Original beim Speichern nicht überschrieben wird.
    let foreign_svg = !is_import_file(path) && !notes.is_empty();
    let imported = is_import_file(path) || foreign_svg;
    // Importierte Dateien haben keinen eigenen Speicherort: Speichern fragt nach einem SVG-Namen,
    // die Originaldatei bleibt unberührt.
    if imported {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let d = match cur(ui).filter(|d| d.pristine()) {
            Some(d) => {
                d.ed.borrow_mut().set_document(doc);
                ui.layers_shown.replace((0, 0, Vec::new()));
                d
            }
            None => add_doc(ui, doc, None),
        };
        *d.title.borrow_mut() = Some(name);
        d.ed.borrow_mut().dirty = true;
        refresh(ui);
        if foreign_svg {
            let list: String = notes.iter().map(|n| format!("\n• {n}")).collect();
            let dialog = adw::AlertDialog::new(
                Some(tr("Nicht alles wird übernommen")),
                Some(&trf("Diese SVG-Datei enthält Inhalte, die Stijl nicht darstellen oder bearbeiten kann:\n{}\n\nDamit dabei nichts verloren geht, bleibt die Originaldatei unverändert: Speichern legt eine neue SVG-Datei an.", &[&(list)])),
            );
            dialog.add_response("ok", "_OK");
            label_icon_buttons(dialog.upcast_ref());
            dialog.present(Some(&ui.window));
            return;
        }
        let mut msg = String::from(tr(
            "Datei importiert und als SVG geöffnet. Speichern legt eine neue SVG-Datei an.",
        ));
        if let Some(first) = notes.first() {
            msg = trf("Importiert. {}", &[&(first)]);
            if notes.len() > 1 {
                msg.push_str(&trf(" (+{} weitere Hinweise)", &[&(notes.len() - 1)]));
            }
        }
        ui.toast(&msg);
        return;
    }
    // Schon offen? Dann nur dorthin wechseln.
    let existing = ui
        .docs
        .borrow()
        .iter()
        .find(|d| d.file.borrow().as_deref() == Some(path))
        .cloned();
    if let Some(d) = existing {
        if let Some(p) = d.page.borrow().clone() {
            ui.tab_view.set_selected_page(&p);
        }
        return;
    }
    add_recent(path);
    match cur(ui).filter(|d| d.pristine()) {
        Some(d) => {
            d.ed.borrow_mut().set_document(doc);
            *d.file.borrow_mut() = Some(path.to_path_buf());
            ui.layers_shown.replace((0, 0, Vec::new()));
            refresh(ui);
        }
        None => {
            add_doc(ui, doc, Some(path.to_path_buf()));
        }
    }
}

// ───────────────────────────── Aktualisieren ─────────────────────────────

fn guarded(ui: &Ui, f: impl FnOnce()) {
    let prev = ui.updating.replace(true);
    f();
    ui.updating.set(prev);
}

fn set_action_enabled(ui: &Ui, name: &str, enabled: bool) {
    if let Some(a) = ui
        .window
        .lookup_action(name)
        .and_then(|a| a.downcast::<gio::SimpleAction>().ok())
    {
        a.set_enabled(enabled);
    }
}

fn refresh(ui: &Rc<Ui>) {
    let Some(d) = cur(ui) else {
        ui.title.set_title("Stijl");
        ui.title.set_subtitle("");
        ui.window.set_title(Some("Stijl"));
        // Start-Tab: Werkzeuge, Seitenleiste und Statusleiste ausblenden
        set_editor_chrome(ui, false);
        ui.welcome.set(true);
        // Ohne Dokument gibt es nur Neu, Öffnen, Tab schließen, Einstellungen und Info.
        for name in ui.window.list_actions() {
            let n = name.as_str();
            set_action_enabled(
                ui,
                n,
                matches!(
                    n,
                    "new" | "new-dialog" | "open" | "preferences" | "about" | "close-tab"
                ),
            );
        }
        return;
    };
    set_editor_chrome(ui, true);
    if ui.welcome.replace(false) {
        for name in ui.window.list_actions() {
            set_action_enabled(ui, name.as_str(), true);
        }
    }
    let (msg, focus_text, tool, version, active, dirty, can_undo, can_redo, sel_count, zoom, dragging) = {
        let mut ed = d.ed.borrow_mut();
        (
            ed.message.take(),
            std::mem::take(&mut ed.request_text_focus),
            ed.tool,
            // Hilfslinien-Zeile hängt auch an Sichtbarkeit und Sperre
            (ed.layers_version * 64 + ed.doc.artboards.len() as u64) * 4
                + (ed.show_guides as u64) * 2
                + ed.lock_guides as u64,
            ed.active_layer,
            ed.dirty,
            ed.can_undo(),
            ed.can_redo(),
            ed.selection.len(),
            ed.view.zoom,
            !matches!(ed.drag, Drag::None),
        )
    };
    if let Some(m) = msg {
        ui.toast(&m);
    }
    ui.tool.set(tool);
    let kind = d.ed.borrow().shape_kind;
    guarded(ui, || {
        for slot in &ui.slots {
            sync_slot(slot, tool, kind);
        }
        sync_tool_opts(ui, &d, tool, kind);
    });
    set_action_enabled(ui, "undo", can_undo);
    set_action_enabled(ui, "redo", can_redo);
    for a in [
        "cut",
        "copy",
        "duplicate",
        "delete",
        "raise",
        "lower",
        "raise-top",
        "lower-bottom",
        "flip-h",
        "flip-v",
        "rotate-left",
        "rotate-right",
        "to-path",
        "scale-dialog",
        "lock-toggle",
        "group",
        "ungroup",
        "page-left",
        "page-hcenter",
        "page-right",
        "page-top",
        "page-vcenter",
        "page-bottom",
    ] {
        set_action_enabled(ui, a, sel_count > 0);
    }
    for a in ["bool-union", "bool-subtract", "bool-intersect", "bool-exclude"] {
        set_action_enabled(ui, a, sel_count >= 2);
    }
    let joinable = d.ed.borrow().can_join_paths();
    set_action_enabled(ui, "join-paths", joinable);
    set_action_enabled(ui, "close-tab", true);
    refresh_panel(ui, &d);

    let doc_key = Rc::as_ptr(&d) as usize;
    let sel_layers = d.ed.borrow().selection_layers();
    let key = (doc_key, version, sel_layers);
    if *ui.layers_shown.borrow() != key {
        ui.layers_shown.replace(key);
        let u = ui.clone();
        glib::idle_add_local_once(move || rebuild_layers(&u));
    } else if active != ui.layers_shown_active.get() {
        ui.layers_shown_active.set(active);
        let row = ui
            .layer_rows
            .borrow()
            .iter()
            .find(|(id, _)| *id == active)
            .map(|(_, r)| r.clone());
        if let Some(row) = row {
            guarded(ui, || ui.layers_list.select_row(Some(&row)));
        }
    }
    if !dragging {
        ui.thumb_frame.set(None);
        for t in ui.thumbs.borrow().iter() {
            t.queue_draw();
        }
    }

    let name = d.name();
    let shown = format!("{}{}", if dirty { "• " } else { "" }, name);
    ui.title.set_title(&shown);
    // unter dem Titel: Speicherort (oder „Nicht gespeichert“) und Format
    let place = d
        .file
        .borrow()
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .map(|s| s.replace(&glib::home_dir().to_string_lossy().to_string(), "~"));
    let save_state = place.unwrap_or_else(|| tr("Nicht gespeichert").to_string());
    ui.window.set_title(Some(&trf("{} – Stijl", &[&(shown)])));
    if let Some(p) = d.page.borrow().as_ref() {
        p.set_title(&shown);
    }
    ui.zoom_label.set_label(&format!("{:.0} %", zoom * 100.0));
    {
        let ed = d.ed.borrow();
        let n = ed.doc.artboard_rects().len();
        let i = ed.page_index();
        ui.page_box.set_visible(ui.notebook.get());
        ui.page_label
            .set_label(&trf("Seite {} von {}", &[&(i.unwrap_or(0) + 1), &(n)]));
    }
    {
        let ed = d.ed.borrow();
        let u = doc_unit(ui, &ed);
        let k = u.px();
        ui.title.set_subtitle(&format!(
            "{save_state} · {} × {} {}",
            fmt_num((ed.doc.width / k * 10.0).round() / 10.0),
            fmt_num((ed.doc.height / k * 10.0).round() / 10.0),
            u.name()
        ));
    }
    refresh_info(ui);
    d.canvas.queue_draw();
    sync_im(&d);

    // Texte werden im Dokument bearbeitet; die Seitenleiste braucht keinen Fokus mehr.
    let _ = focus_text;
}

fn refresh_panel(ui: &Rc<Ui>, d: &DocTab) {
    let (info, kind, corners, grad_angle, unit, show_guides, snap_guides) = {
        let ed = d.ed.borrow();
        (
            ed.context_info(),
            ed.shape_kind,
            ed.corners,
            ed.gradient_angle(&ui.metrics),
            doc_unit(ui, &ed),
            ed.show_guides,
            ed.snap_guides,
        )
    };
    let c = &ui.panel;
    guarded(ui, || {
        c.heading.set_label(&info.title);

        // nur Vielecke und Sterne haben hier noch etwas einzustellen (Anzahl der Ecken)
        // Ecken: beim Zeichnen von Vielecken/Sternen und bei einem ausgewählten Vieleck/Stern
        // Ecken des ausgewählten Vielecks/Sterns; für neue stehen sie unter den Werkzeugen
        c.shape_group.set_visible(info.poly.is_some());
        let shape_widgets_visible = info.show_shape;
        let _ = shape_widgets_visible;
        // Die Form wird über die Werkzeugleiste gewählt (Rechtsklick auf das Formen-Werkzeug).
        c.shape_kind.parent().inspect(|p| p.set_visible(false));
        if let Some(i) = SHAPE_KINDS.iter().position(|(k, _)| *k == kind) {
            c.shape_kind.set_selected(i as u32);
        }
        c.corners_row.set_visible(info.poly.is_some());
        c.corners
            .set_value(info.poly.map(|p| p.0).unwrap_or(corners) as f64);
        // Eckenradius nur noch durch Ziehen am Objekt (Knotenwerkzeug)
        c.radius_row.set_visible(false);
        c.radius.set_value(info.radius);

        c.text_group.set_visible(info.show_text);
        // uneinheitliche Angaben (verschieden formatierte Textteile): Felder leer
        let mx = info.font_mixed;
        if mx.family {
            c.font.set_selected(gtk::INVALID_LIST_POSITION);
        } else {
            font_select(&c.font, &info.font.family);
        }
        c.text_hint.set_label(&match (mx.family, mx.size) {
            (false, false) => format!("{} · {} px", info.font.family, fmt_num(info.font.size)),
            (false, true) => info.font.family.clone(),
            (true, false) => format!("{} px", fmt_num(info.font.size)),
            (true, true) => "gemischt".to_string(),
        });
        c.size.set_value(info.font.size);
        if mx.size {
            c.size.set_text("");
        } else if c.size.text().is_empty() {
            c.size.set_text(&format!("{:.0}", info.font.size));
        }
        // Schnitte der Familie (neu einlesen, wenn sich die Familie geändert hat)
        if !mx.family && *c.faces_family.borrow() != info.font.family {
            let faces = font_faces(&info.font.family);
            c.face.set_model(Some(&faces_model(&faces)));
            *c.faces.borrow_mut() = faces;
            *c.faces_family.borrow_mut() = info.font.family.clone();
        }
        let w = crate::model::effective_weight(info.font.fmt.weight, info.font.bold);
        if mx.family || mx.weight || mx.bold || mx.italic {
            c.face.set_selected(gtk::INVALID_LIST_POSITION);
        } else {
            c.face
                .set_selected(face_index(&c.faces.borrow(), w, info.font.italic));
        }
        c.weight_row
            .set_visible(!mx.family && font_is_variable(&info.font.family));
        c.weight.set_value(w as f64);
        c.text_frame.set_visible(false);
        let f = info.font.fmt;
        c.underline.set_active(f.underline && !mx.underline);
        c.strike.set_active(f.strike && !mx.strike);
        c.list_box.set_visible(info.show_list);
        let pl = info.para.list;
        if pl != ListStyle::None {
            c.list_style.set(pl);
        }
        c.list_on.set_active(pl != ListStyle::None);
        c.list_on
            .set_tooltip_text(Some(&trf("Aufzählung ({})", &[&(c.list_style.get().label())])));
        c.indent_less.set_sensitive(info.para.indent > 0);
        c.indent_more
            .set_sensitive(info.para.indent < crate::model::MAX_INDENT);
        c.para_row.set_visible(info.show_valign);
        c.para_spacing.set_value((f.para * 100.0).round());
        c.columns_row.set_visible(info.show_list);
        c.columns.set_value(f.cols() as f64);
        c.small_caps.set_active(f.small_caps);
        c.ligatures.set_active(!f.no_ligatures);
        c.path_box.set_visible(info.text_offset.is_some());
        c.path_below.set_active(f.path_below);
        c.path_reverse.set_active(f.path_reverse);
        let ai = Align::ALL.iter().position(|a| *a == f.align).unwrap_or(0);
        c.align[ai].set_active(true);
        c.valign_box.set_visible(info.show_valign);
        let vi = VAlign::ALL.iter().position(|a| *a == f.valign).unwrap_or(0);
        c.valign[vi].set_active(true);
        c.line_spacing.set_value((f.line * 100.0).round());
        c.tracking.set_value(f.tracking.round());
        c.offset_row.set_visible(info.text_offset.is_some());
        if let Some(o) = info.text_offset {
            c.offset.set_value(o);
        }
        if let Some(t) = &info.text {
            let buf = c.text.buffer();
            let (s, e) = buf.bounds();
            if buf.text(&s, &e, false).as_str() != t {
                buf.set_text(t);
            }
        }

        c.node_group.set_visible(info.show_node);
        c.node_radius.set_value(info.node_radius);
        c.node_corner.set_sensitive(info.node_count > 0);
        c.node_smooth.set_sensitive(info.node_count > 0);
        c.node_delete.set_sensitive(info.node_count > 0);

        c.style_group.set_visible(info.show_style);
        {
            // Farbwahl für Füllung, Kontur und Schatten; mit Verlauf bearbeitet sie den
            // gewählten Farbstopp
            let (gstop, gtarget, angles) = {
                let ed = d.ed.borrow();
                (
                    ed.grad_stop,
                    ed.grad_target,
                    [PaintTarget::Fill, PaintTarget::Stroke, PaintTarget::Shadow]
                        .map(|t| ed.gradient_angle_of(t, &ui.metrics)),
                )
            };
            let pct = |c: Option<Color>| {
                c.map(|c| format!("{} %", (c.a * 100.0).round()))
                    .unwrap_or_default()
            };
            let targets = [PaintTarget::Fill, PaintTarget::Stroke, PaintTarget::Shadow];
            for (k, sw) in [&c.fill_sw, &c.stroke_sw, &c.shadow_sw].into_iter().enumerate() {
                let t = targets[k];
                let stop = if t == gtarget { gstop } else { 0 };
                let cur = info.style.paint(t);
                let mut hint = pct(cur);
                if t == PaintTarget::Stroke && cur.is_some() {
                    hint = format!(
                        "{hint} · {} {}",
                        fmt_num((info.style.width / unit.px() * 10.0).round() / 10.0),
                        unit.name()
                    );
                }
                sw.sync(cur, info.style.grad(t).cloned(), stop, angles[k], &hint);
            }
            // Muster machen Text unleserlich: bei reinem Text kein Reiter „Muster“
            // (es sei denn, der Text hat schon eins)
            let has_pattern = info
                .style
                .grad(PaintTarget::Fill)
                .is_some_and(|g| g.shape.is_pattern());
            c.fill_sw.pattern_tab.set_visible(!info.is_text || has_pattern);
            // Bild: nur Deckkraft
            let is_image = info.image.is_some();
            c.image_sw.root.set_visible(is_image);
            if is_image {
                let op = Color {
                    a: info.style.opacity,
                    ..Color::BLACK
                };
                let t = info.style.fx.temp;
                let mut hint = pct(Some(op));
                if t.abs() > 1e-9 {
                    hint = format!("{hint} · {:+.0} °", t * 100.0);
                }
                c.image_sw.sync(Some(op), None, 0, 0.0, &hint);
                c.temp.set_value((t * 100.0).round());
            }
            // Lage der Kontur
            let ai = crate::model::StrokeAlign::ALL
                .iter()
                .position(|a| *a == info.style.stroke_align)
                .unwrap_or(0);
            c.stroke_align[ai].set_active(true);
        }
        c.shadow_box.set_visible(info.style.shadow.is_some());
        c.shadow_section.set_visible(info.style.shadow.is_some());
        {
            // Effekt-Bereiche: sichtbar, wenn angewendet; Kopfzeile nennt Art und Stärke
            let st = &info.style;
            let pct = |k: f64| format!("{} %", (k * 100.0).round());
            // mit Vorzeichen (3D-Effekt: − eingedrückt, + herausgedrückt)
            let spct = |k: f64| {
                let v = (k * 100.0).round() as i32;
                if v > 0 {
                    format!("+{v} %")
                } else {
                    format!("{v} %")
                }
            };
            let b = st.blend;
            c.blend_fold.root.set_visible(b != Blend::Normal);
            c.blend_fold
                .hint
                .set_label(if b != Blend::Normal { b.label() } else { "" });
            if let Some(i) = Blend::SIMPLE[1..].iter().position(|x| *x == b) {
                c.blend_fold.choice.set_selected(i as u32);
            }
            let fx = st.fx;
            c.tex_fold.root.set_visible(fx.tex.is_some());
            if let Some((t, k)) = fx.tex {
                c.tex_fold.hint.set_label(&format!("{} · {}", t.label(), pct(k)));
                c.tex_fold.strength.set_value(k * 100.0);
                if let Some(i) = crate::model::Texture::ALL.iter().position(|x| *x == t) {
                    c.tex_fold.choice.set_selected(i as u32);
                }
            }
            c.art_fold.root.set_visible(fx.art.is_some());
            if let Some((a, k)) = fx.art {
                c.art_fold.hint.set_label(&format!("{} · {}", a.label(), pct(k)));
                c.art_fold.strength.set_value(k * 100.0);
                if let Some(i) = crate::model::Art::ALL.iter().position(|x| *x == a) {
                    c.art_fold.choice.set_selected(i as u32);
                }
            }
            c.lens_fold.root.set_visible(fx.lens.is_some());
            if let Some((l, k)) = fx.lens {
                c.lens_fold
                    .hint
                    .set_label(&format!("{} · {}", l.label(), spct(k)));
                c.lens_fold.strength.set_value(k * 100.0);
                c.lens_fold.strength2.set_value(fx.light * 100.0);
                if let Some(i) = crate::model::Lens::ALL.iter().position(|x| *x == l) {
                    c.lens_fold.choice.set_selected(i as u32);
                }
            }
            c.tint_fold.root.set_visible(fx.tint.is_some());
            if let Some((f, _)) = fx.tint {
                c.tint_fold.hint.set_label(f.label());
                if let Some(i) = crate::model::ImgFilter::ALL.iter().position(|x| *x == f) {
                    c.tint_fold.choice.set_selected(i as u32);
                }
            }
            c.mirror_fold.root.set_visible(fx.mirror.is_some());
            if let Some(k) = fx.mirror {
                c.mirror_fold.hint.set_label(&pct(k));
                c.mirror_fold.strength.set_value(k * 100.0);
            }
        }
        {
            let sh = info.style.shadow.unwrap_or_default();
            c.shadow_opacity.set_value((sh.opacity * 100.0).round());
            c.shadow_blur.set_value(sh.blur.round());
            c.shadow_dx.set_value(sh.dx.round());
            c.shadow_dy.set_value(sh.dy.round());
        }
        c.width.set_value(info.style.width);
        // Füllung/Kontur nur, wo sie Sinn ergeben; Linienstärke nur bei aktiver Kontur
        c.fill_sw.root.set_visible(info.show_fill);
        c.stroke_sw.root.set_visible(info.show_stroke);
        update_swatches(ui);
        c.width
            .parent()
            .inspect(|p| p.set_visible(info.show_stroke && info.style.stroke.is_some()));
        c.opacity.set_value((info.style.opacity * 100.0).round());
        // Farbverlauf
        let grad = info.style.gradient.as_ref().filter(|_| info.style.fill.is_some());
        // Verlauf kommt über den Reiter „Verlauf“ der Farbwahl
        c.grad_add.set_visible(false);
        c.grad_box
            .set_visible(info.show_fill && info.has_selection && grad.is_some());
        if let Some(g) = grad {
            let i = info.grad_stop.min(g.stops.len().saturating_sub(1));
            if let Some(st) = g.stops.get(i) {
                c.grad_color.set_rgba(&to_rgba(st.color));
            }
            c.grad_remove_stop.set_sensitive(g.stops.len() > 2);
            c.grad_angle.set_value(grad_angle.round());
            let ki = GradientKind::ALL.iter().position(|k| *k == g.kind).unwrap_or(0);
            c.grad_kind.set_selected(ki as u32);
        }
        c.grad_bar.queue_draw();
        let cmyk = ui.cmyk.get();
        c.fill_cmyk
            .set_visible(cmyk && info.show_fill && info.style.fill.is_some() && grad.is_none());
        c.stroke_cmyk
            .set_visible(cmyk && info.show_stroke && info.style.stroke.is_some());
        if let Some(f) = info.style.fill {
            let (cc, m, y, k) = f.to_cmyk();
            for (sc, v) in c.fill_cmyk_scales.iter().zip([cc, m, y, k]) {
                sc.set_value((v * 100.0).round());
            }
        }
        if let Some(st) = info.style.stroke {
            let (cc, m, y, k) = st.to_cmyk();
            for (sc, v) in c.stroke_cmyk_scales.iter().zip([cc, m, y, k]) {
                sc.set_value((v * 100.0).round());
            }
        }

        c.arrange_group.set_visible(info.has_selection);
        c.pinned.set_visible(info.has_selection && !info.empty_hint);
        c.angle.set_value((info.angle * 10.0).round() / 10.0);
        // Ecken und Enden der Kontur
        let stroke_shown = info.show_stroke && info.style.stroke.is_some();
        c.join_row.set_visible(stroke_shown);
        c.dash_row.set_visible(stroke_shown);
        c.dash.set_selected(
            crate::model::Dash::ALL
                .iter()
                .position(|d| *d == info.style.dash)
                .unwrap_or(0) as u32,
        );
        c.cap_row.set_visible(stroke_shown && info.show_markers);
        let ji = LineJoin::ALL
            .iter()
            .position(|j| *j == info.style.join)
            .unwrap_or(0);
        c.join[ji].set_active(true);
        let ci = LineCap::ALL
            .iter()
            .position(|j| *j == info.style.cap)
            .unwrap_or(0);
        c.cap[ci].set_active(true);
        // Linienenden
        c.marker_row
            .set_visible(info.show_markers && info.show_stroke && info.style.stroke.is_some());
        let mi = Marker::ALL
            .iter()
            .position(|m| *m == info.style.marker)
            .unwrap_or(0);
        c.marker.set_selected(mi as u32);
        c.marker_ends_box.set_visible(info.style.marker != Marker::None);
        c.marker_size_row.set_visible(info.style.marker != Marker::None);
        c.marker_size.set_value((info.style.marker_size * 100.0).round());
        let ei = MarkerEnds::ALL
            .iter()
            .position(|m| *m == info.style.marker_ends)
            .unwrap_or(1);
        c.marker_ends[ei].set_active(true);
        // Rahmen eines Textfeldes
        c.tframe_group.set_visible(info.text_frame.is_some());
        c.inset_row.set_visible(info.text_frame.is_some());
        c.frame_fill_sw.root.set_visible(info.text_frame.is_some());
        c.frame_stroke_sw.root.set_visible(info.text_frame.is_some());
        if let Some(f) = &info.text_frame {
            let pct = |c: Option<Color>| {
                c.map(|c| format!("{} %", (c.a * 100.0).round()))
                    .unwrap_or_default()
            };
            c.frame_fill_sw.sync(f.fill, None, 0, 0.0, &pct(f.fill));
            let sh = match f.stroke {
                Some(_) => format!(
                    "{} · {} {}",
                    pct(f.stroke),
                    fmt_num((f.width / unit.px() * 10.0).round() / 10.0),
                    unit.name()
                ),
                None => String::new(),
            };
            c.frame_stroke_sw.sync(f.stroke, None, 0, 0.0, &sh);
            c.tframe_fill_on.set_active(f.fill.is_some());
            if let Some(col) = f.fill {
                c.tframe_fill.set_rgba(&to_rgba(col));
            }
            c.tframe_fill.set_sensitive(f.fill.is_some());
            c.tframe_stroke_on.set_active(f.stroke.is_some());
            if let Some(col) = f.stroke {
                c.tframe_stroke.set_rgba(&to_rgba(col));
            }
            c.tframe_stroke.set_sensitive(f.stroke.is_some());
            c.tframe_width_row.set_visible(f.stroke.is_some());
            c.tframe_width.set_value(f.width);
            c.tframe_inset.set_value(f.inset);
            c.tframe_release.set_visible(f.path.is_some());
        }
        c.fill_sw.title.set_label(if info.is_text {
            tr("Textfüllung")
        } else {
            tr("Füllung")
        });
        c.stroke_sw.title.set_label(if info.is_text {
            tr("Textkontur")
        } else {
            tr("Kontur")
        });
        // Freihand
        // Bild
        c.image_group.set_visible(info.image.is_some());
        if let Some((framed, part)) = info.image {
            c.image_crop.set_visible(!framed);
            c.image_modes_box.set_visible(framed);
            c.image_release.set_visible(framed);
            let pi = match part {
                ImagePart::All => 0,
                ImagePart::Frame => 1,
                ImagePart::Content => 2,
            };
            c.image_modes[pi].set_active(true);
        }
        // Hilfslinien
        c.guide_group.set_visible(info.show_guides_panel);
        let ga = info.guide.map(|g| g.angle).unwrap_or(info.guide_angle);
        // Winkel und Lage nur für eine gewählte Hilfslinie; neue Linien bekommen ihren
        // Winkel beim Aufziehen.
        let gi = if crate::model::normalize_angle(ga).abs() < 1e-6
            || (crate::model::normalize_angle(ga).abs() - 180.0).abs() < 1e-6
        {
            0
        } else if (crate::model::normalize_angle(ga).abs() - 90.0).abs() < 1e-6 {
            1
        } else {
            2
        };
        c.guide_angle_row.set_visible(info.guide.is_some());
        c.guide_angle.set_value(ga);
        let k = unit.px();
        match info.guide {
            Some(g) if gi != 2 => {
                c.guide_pos_row.set_visible(true);
                let (axis, v) = if gi == 0 { ("y", g.pos.y) } else { ("x", g.pos.x) };
                c.guide_pos_label
                    .set_label(&trf("Position {} ({})", &[&(axis), &(unit.name())]));
                c.guide_pos.set_digits(unit_digits(unit));
                c.guide_pos.set_value(v / k);
            }
            _ => c.guide_pos_row.set_visible(false),
        }
        c.guide_delete.set_sensitive(info.guide.is_some());
        c.guide_show.set_active(show_guides);
        c.guide_snap.set_active(snap_guides);
        c.convert.set_visible(info.can_convert);
        // Auswahlwerkzeug ohne Auswahl: nur der Hinweis
        c.empty_hint.set_visible(info.empty_hint);
        if info.empty_hint {
            for g in [
                &c.shape_group,
                &c.text_group,
                &c.node_group,
                &c.style_group,
                &c.arrange_group,
                &c.image_group,
                &c.tframe_group,
                &c.guide_group,
            ] {
                g.set_visible(false);
            }
        }
    });
}

// ───────────────────────────── Aktionen ─────────────────────────────

fn add_action(ui: &Rc<Ui>, name: &str, f: impl Fn(&Rc<Ui>) + 'static) {
    let a = gio::SimpleAction::new(name, None);
    let u = ui.clone();
    a.connect_activate(move |_, _| f(&u));
    ui.window.add_action(&a);
}

/// Bearbeitungsbefehl auf das aktuelle Dokument anwenden und neu zeichnen.
fn edit_action(ui: &Rc<Ui>, name: &str, f: impl Fn(&mut Editor, &CairoMetrics) + 'static) {
    add_action(ui, name, move |u| {
        let Some(d) = cur(u) else { return };
        f(&mut d.ed.borrow_mut(), &u.metrics);
        refresh(u);
    });
}

/// Aktion mit Text-Parameter, die den Editor ändert.
fn str_action(ui: &Rc<Ui>, name: &str, f: impl Fn(&mut Editor, &str) + 'static) {
    let a = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
    let u = ui.clone();
    a.connect_activate(move |_, v| {
        let Some(n) = v.and_then(|v| v.get::<String>()) else {
            return;
        };
        if let Some(d) = cur(&u) {
            f(&mut d.ed.borrow_mut(), &n);
            refresh(&u);
        }
    });
    ui.window.add_action(&a);
}

/// Zeichenfläche/Notizheft umschalten, merken und alle Dokumente neu zeichnen.
fn set_notebook_view(ui: &Rc<Ui>, on: bool, action: Option<&gio::SimpleAction>) {
    ui.notebook.set(on);
    let a = action.cloned().or_else(|| {
        ui.window
            .lookup_action("notebook")
            .and_downcast::<gio::SimpleAction>()
    });
    if let Some(a) = a {
        a.set_state(&on.to_variant());
    }
    save_settings(ui);
    redraw_all(ui);
    refresh(ui);
}

fn canvas_size(d: &DocTab) -> (f64, f64) {
    (d.canvas.width() as f64, d.canvas.height() as f64)
}

fn zoom_by(ui: &Rc<Ui>, f: impl Fn(f64) -> f64) {
    let Some(d) = cur(ui) else { return };
    let (w, h) = canvas_size(&d);
    {
        let mut ed = d.ed.borrow_mut();
        let factor = f(ed.view.zoom);
        ed.zoom_at(factor, Pt::new(w / 2.0, h / 2.0));
    }
    refresh(ui);
}

fn redraw_all(ui: &Ui) {
    for d in ui.docs.borrow().iter() {
        d.canvas.queue_draw();
    }
}

fn setup_actions(ui: &Rc<Ui>) {
    edit_action(ui, "gradient-add-stop", |e, _| {
        // In die größte Lücke zwischen zwei Farben
        let pos: Vec<f64> = e
            .selected_objects()
            .into_iter()
            .find_map(|o| {
                o.style
                    .gradient
                    .as_ref()
                    .map(|g| g.sorted().iter().map(|s| s.pos).collect())
            })
            .unwrap_or_default();
        let mut best = (0.5, -1.0);
        for w in pos.windows(2) {
            if w[1] - w[0] > best.1 {
                best = ((w[0] + w[1]) / 2.0, w[1] - w[0]);
            }
        }
        e.add_stop(best.0);
    });
    edit_action(ui, "gradient-remove-stop", |e, _| {
        e.remove_stop();
    });
    edit_action(ui, "gradient-remove", |e, _| {
        e.remove_gradient();
    });
    edit_action(ui, "text-release", |e, _| {
        e.release_text_shape();
    });
    edit_action(ui, "image-crop", |e, _| {
        e.crop_image();
    });
    edit_action(ui, "image-release", |e, _| {
        e.release_frame();
    });
    edit_action(ui, "guide-delete", |e, _| {
        e.delete_guide();
    });
    edit_action(ui, "guide-clear", |e, _| {
        e.clear_guides();
    });
    for (name, h, v) in [
        ("page-left", Some(-1i8), None),
        ("page-hcenter", Some(0), None),
        ("page-right", Some(1), None),
        ("page-top", None, Some(-1i8)),
        ("page-vcenter", None, Some(0)),
        ("page-bottom", None, Some(1)),
    ] {
        let u = ui.clone();
        let a = gio::SimpleAction::new(name, None);
        a.connect_activate(move |_, _| {
            // Eigenschaften: immer an der Seite ausrichten
            with_ed_m(&u, |e, m| {
                let keep = e.align_selection;
                e.align_selection = false;
                e.align(h, v, m);
                e.align_selection = keep;
            });
        });
        ui.window.add_action(&a);
    }
    // Schwebeleiste: immer aneinander ausrichten (unabhängig von der Wahl in den
    // Eigenschaften)
    for (name, h, v) in [
        ("sel-align-left", Some(-1i8), None),
        ("sel-align-hcenter", Some(0), None),
        ("sel-align-right", Some(1), None),
        ("sel-align-top", None, Some(-1i8)),
        ("sel-align-vcenter", None, Some(0)),
        ("sel-align-bottom", None, Some(1)),
    ] {
        let u = ui.clone();
        let a = gio::SimpleAction::new(name, None);
        a.connect_activate(move |_, _| {
            with_ed_m(&u, |e, m| {
                let keep = e.align_selection;
                e.align_selection = true;
                e.align(h, v, m);
                e.align_selection = keep;
            });
        });
        ui.window.add_action(&a);
    }
    edit_action(ui, "distribute-h", |e, m| {
        e.distribute(true, m);
    });
    edit_action(ui, "distribute-v", |e, m| {
        e.distribute(false, m);
    });
    add_action(ui, "new", add_start_tab);
    add_action(ui, "new-dialog", new_document_dialog);
    add_action(ui, "open", open_dialog);
    add_action(ui, "save", |u| {
        if let Some(d) = cur(u) {
            save(u, &d, Box::new(|_: &Rc<Ui>, _: bool| {}));
        }
    });
    add_action(ui, "save-as", |u| {
        if let Some(d) = cur(u) {
            save_as(u, &d, Box::new(|_: &Rc<Ui>, _: bool| {}));
        }
    });
    add_action(ui, "export", export_dialog);
    add_action(ui, "export-selection", |u| export_dialog_with(u, true));
    add_action(ui, "copy-image", copy_as_image);
    add_action(ui, "report-bug", |u| report_bug(u, None));
    add_action(ui, "close-tab", |u| {
        if let Some(p) = u.tab_view.selected_page() {
            u.tab_view.close_page(&p);
        }
    });
    add_action(ui, "preferences", preferences_dialog);
    add_action(ui, "shortcuts", |u| preferences_dialog_at(u, 2));
    add_action(ui, "help", |u| {
        gtk::UriLauncher::new(HELP_URL).launch(Some(&u.window), gio::Cancellable::NONE, |_| {});
    });
    add_action(ui, "insert-image", insert_image_dialog);
    add_action(ui, "artboard", artboard_dialog);
    add_action(ui, "scale-dialog", scale_dialog);
    edit_action(ui, "artboard-add", |e, _| {
        e.add_artboard();
    });
    for (name, f) in [
        (
            "artboard-edit",
            (|u: &Rc<Ui>, i: usize| artboard_dialog_at(u, i)) as fn(&Rc<Ui>, usize),
        ),
        ("artboard-delete", |u: &Rc<Ui>, i: usize| {
            with_ed(u, |e| {
                e.delete_artboard(i);
            })
        }),
        ("artboard-duplicate", |u: &Rc<Ui>, i: usize| {
            with_ed_m(u, |e, m| {
                e.duplicate_artboard(i, m);
            })
        }),
    ] {
        let a = gio::SimpleAction::new(name, Some(glib::VariantTy::UINT32));
        let u = ui.clone();
        a.connect_activate(move |_, v| {
            if let Some(i) = v.and_then(|v| v.get::<u32>()) {
                f(&u, i as usize);
            }
        });
        ui.window.add_action(&a);
    }
    add_action(ui, "about", |u| {
        let about = adw::AboutDialog::builder()
            .application_name("Stijl")
            .version(env!("CARGO_PKG_VERSION"))
            .developer_name("Bendix Hüsing")
            .copyright("© 2026 Bendix Hüsing")
            .website("https://github.com/benhuesing/Stijl")
            .issue_url(ISSUE_URL)
            .comments(tr(
                "Ein Vektorzeichenprogramm, benannt nach der niederländischen Kunstbewegung De Stijl, der auch Piet Mondrian angehörte.\n\nMit Hilfe von KI (Claude von Anthropic) geschrieben.",
            ))
            .license_type(gtk::License::Gpl30)
            .application_icon("io.github.benhuesing.Stijl")
            .build();
        label_icon_buttons(about.upcast_ref());
        about.present(Some(&u.window));
    });

    edit_action(ui, "undo", |e, _| {
        e.undo();
    });
    edit_action(ui, "redo", |e, _| {
        e.redo();
    });
    for (name, kind) in [("cut", 0u8), ("copy", 1), ("paste", 2)] {
        add_action(ui, name, move |u| {
            let Some(d) = cur(u) else { return };
            {
                let mut ed = d.ed.borrow_mut();
                clipboard_op(u, &d, &mut ed, kind);
            }
            refresh(u);
        });
    }
    edit_action(ui, "duplicate", |e, _| {
        e.duplicate();
    });
    edit_action(ui, "delete", |e, _| {
        e.delete_selection();
    });
    edit_action(ui, "select-all", |e, _| e.select_all());
    edit_action(ui, "raise", |e, _| {
        e.restack(1, false);
    });
    edit_action(ui, "lower", |e, _| {
        e.restack(-1, false);
    });
    edit_action(ui, "raise-top", |e, _| {
        e.restack(1, true);
    });
    edit_action(ui, "lower-bottom", |e, _| {
        e.restack(-1, true);
    });
    edit_action(ui, "to-path", |e, m| {
        if e.convert_to_path(m) {
            e.set_tool(Tool::Node);
        }
    });
    edit_action(ui, "rotate-left", |e, m| {
        e.rotate_selection(-90.0, m);
    });
    edit_action(ui, "rotate-right", |e, m| {
        e.rotate_selection(90.0, m);
    });
    edit_action(ui, "flip-h", |e, m| {
        e.flip_selection(true, m);
    });
    edit_action(ui, "flip-v", |e, m| {
        e.flip_selection(false, m);
    });
    edit_action(ui, "join-paths", |e, _| {
        e.join_paths();
    });
    edit_action(ui, "bool-union", |e, m| {
        e.boolean(Op::Union, m);
    });
    edit_action(ui, "bool-subtract", |e, m| {
        e.boolean(Op::Difference, m);
    });
    edit_action(ui, "bool-intersect", |e, m| {
        e.boolean(Op::Intersection, m);
    });
    edit_action(ui, "bool-exclude", |e, m| {
        e.boolean(Op::Xor, m);
    });
    edit_action(ui, "node-corner", |e, _| {
        e.node_make_corner();
    });
    edit_action(ui, "node-smooth", |e, _| {
        e.node_make_smooth();
    });
    edit_action(ui, "node-delete", |e, _| {
        e.node_delete();
    });

    {
        let a = gio::SimpleAction::new("move-to-layer", Some(glib::VariantTy::UINT64));
        let u = ui.clone();
        a.connect_activate(move |_, param| {
            let Some(target) = param.and_then(|v| v.get::<u64>()) else {
                return;
            };
            if let Some(d) = cur(&u) {
                d.ed.borrow_mut().move_selection_to_layer(target);
                refresh(&u);
            }
        });
        ui.window.add_action(&a);
    }
    edit_action(ui, "layer-add", |e, _| e.add_layer());
    edit_action(ui, "layer-up", |e, _| {
        e.move_layer(1);
    });
    edit_action(ui, "layer-down", |e, _| {
        e.move_layer(-1);
    });
    edit_action(ui, "layer-delete", |e, _| {
        e.delete_layer();
    });

    add_action(ui, "zoom-in", |u| zoom_by(u, |_| 1.25));
    add_action(ui, "zoom-out", |u| zoom_by(u, |_| 0.8));
    add_action(ui, "zoom-100", |u| zoom_by(u, |z| 1.0 / z));
    add_action(ui, "zoom-fit", |u| {
        let Some(d) = cur(u) else { return };
        let (w, h) = canvas_size(&d);
        d.ed.borrow_mut().fit(w, h);
        refresh(u);
    });
    {
        // Ansicht: Zeichenfläche oder Notizheft (gilt für alle Dokumente des Fensters)
        let a = gio::SimpleAction::new_stateful("notebook", None, &ui.notebook.get().to_variant());
        let u = ui.clone();
        a.connect_activate(move |a, _| set_notebook_view(&u, !u.notebook.get(), Some(a)));
        ui.window.add_action(&a);
    }
    edit_action(ui, "page-next", |e, _| {
        let i = e.page_index().unwrap_or(0);
        e.go_page(i + 1, false);
    });
    edit_action(ui, "page-prev", |e, _| {
        if let Some(i) = e.page_index().filter(|i| *i > 0) {
            e.go_page(i - 1, false);
        }
    });
    {
        let a = gio::SimpleAction::new("page-go", Some(glib::VariantTy::UINT32));
        let u = ui.clone();
        a.connect_activate(move |_, p| {
            let Some(i) = p.and_then(|v| v.get::<u32>()) else {
                return;
            };
            let Some(d) = cur(&u) else { return };
            {
                let mut ed = d.ed.borrow_mut();
                if ed.notebook.is_some() {
                    ed.go_page(i as usize, false);
                    let (w, h) = ed.viewport;
                    ed.fit(w, h);
                }
            }
            refresh(&u);
        });
        ui.window.add_action(&a);
    }
    add_action(ui, "lib-add-ask", lib_add_dialog);
    add_action(ui, "blend-dialog", blend_dialog);
    edit_action(ui, "lock-toggle", |e, _| {
        let v = !e.selection_locked();
        e.set_selection_locked(v);
    });
    edit_action(ui, "group", |e, _| {
        e.group_selection();
    });
    edit_action(ui, "ungroup", |e, _| {
        e.ungroup_selection();
    });
    add_action(ui, "toggle-sidebar", |u| {
        u.side_toggle.set_active(!u.side_toggle.is_active());
    });
    add_action(ui, "toggle-panel", |u| {
        u.props_toggle.set_active(!u.props_toggle.is_active());
    });
}

thread_local! {
    /// Aktuelle Tastenbelegung (Standard plus eigene Änderungen)
    static KEYMAP: RefCell<crate::keys::Keymap> = RefCell::new(crate::keys::Keymap::default());
}

fn shortcuts_path() -> PathBuf {
    glib::user_config_dir().join("stijl").join("shortcuts.ini")
}

fn save_keymap() {
    let path = shortcuts_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text = KEYMAP.with(|k| k.borrow().to_text());
    if text.is_empty() {
        let _ = std::fs::remove_file(path);
    } else {
        let _ = crate::fsutil::write_atomic(&path, text.as_bytes());
    }
}

/// Fensteraktionen mit ihren aktuellen Kürzeln bei der Anwendung anmelden.
fn apply_accels(app: &gtk::Application) {
    use crate::keys::{Kind, DEFS};
    KEYMAP.with(|k| {
        let k = k.borrow();
        for d in DEFS {
            if let Kind::Action(action) = d.kind {
                let keys = k.keys(d.id);
                // „<Primary>“: Strg, auf dem Mac ⌘
                let keys: Vec<String> = keys.iter().map(|a| crate::keys::to_gtk(a)).collect();
                let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
                app.set_accels_for_action(action, &refs);
            }
        }
    });
}

/// Beim Start: eigene Tastenbelegung laden, Kürzel anmelden, Farbschema setzen.
pub fn set_accels(app: &adw::Application) {
    if let Ok(text) = std::fs::read_to_string(shortcuts_path()) {
        KEYMAP.with(|k| *k.borrow_mut() = crate::keys::Keymap::from_text(&text));
    }
    apply_accels(app.upcast_ref());
    load_settings().scheme.apply();
}

// ───────────────────────────── Zeichenfläche ─────────────────────────────

fn setup_canvas(ui: &Rc<Ui>, d: &Rc<DocTab>) {
    let canvas = &d.canvas;
    {
        let wu = Rc::downgrade(ui);
        let wd = Rc::downgrade(d);
        canvas.set_draw_func(move |da, cr, w, h| {
            // Markierungen in der Akzentfarbe des Systems
            render::set_ui_accent(accent_rgb(da));
            let (Some(u), Some(d)) = (wu.upgrade(), wd.upgrade()) else {
                return;
            };
            let here = d
                .canvas
                .compute_point(&u.window, &gtk::graphene::Point::new(0.0, 0.0))
                .map(|p| (p.x() as f64, p.y() as f64));
            let fitted = {
                let mut ed = d.ed.borrow_mut();
                if let Some((x, y)) = here {
                    if let Some((x0, y0)) = d.origin.replace(Some((x, y))) {
                        if !ed.needs_fit && ((x - x0).abs() > 0.1 || (y - y0).abs() > 0.1) {
                            ed.view.ox -= x - x0;
                            ed.view.oy -= y - y0;
                        }
                    }
                }
                ed.viewport = (w as f64, h as f64);
                // Ansicht (Zeichenfläche/Notizheft) des Fensters übernehmen
                let mut fit = false;
                if u.notebook.get() != ed.notebook.is_some() && w > 0 && h > 0 && !ed.needs_fit {
                    ed.set_notebook(u.notebook.get());
                    fit = true;
                }
                if ed.needs_fit && w > 0 && h > 0 {
                    ed.set_notebook(u.notebook.get());
                    ed.fit(w as f64, h as f64);
                    fit = true;
                }
                fit
            };
            let dark = adw::StyleManager::default().is_dark();
            let unit = doc_unit(&u, &d.ed.borrow());
            // Effekte nicht neu berechnen, solange verschoben, skaliert oder gezoomt wird
            {
                let ed = d.ed.borrow();
                let now = std::time::Instant::now();
                let v = (ed.view.zoom, ed.view.ox, ed.view.oy);
                if d.view_seen.replace(v) != v {
                    d.view_changed.set(Some(now));
                }
                let moving = matches!(
                    ed.drag,
                    Drag::Pan { .. }
                        | Drag::Move { .. }
                        | Drag::Scale { .. }
                        | Drag::Rotate { .. }
                        | Drag::Artboard { .. }
                        | Drag::ArtboardSize { .. }
                );
                let recent = d
                    .view_changed
                    .get()
                    .is_some_and(|t| now.duration_since(t) < std::time::Duration::from_millis(SETTLE_MS));
                render::set_canvas_state(ed.edits(), moving || recent);
            }
            render::draw_canvas(
                cr,
                w as f64,
                h as f64,
                &d.ed.borrow(),
                &u.metrics,
                dark,
                unit,
                u.backdrop.get(),
                u.show_rulers.get(),
                d.canvas.scale_factor() as f64,
            );
            // Pipette: Lupe mit der Farbe unter dem Zeiger
            if let (true, Some((px, py, col))) = (pipette_active(&u), u.pipette_at.get()) {
                render::draw_pipette(cr, px, py, col);
            }
            // veraltete Effektbilder gezeigt: kurz danach noch einmal zeichnen (dann gerechnet)
            if render::effects_stale() && !d.settle_pending.get() {
                d.settle_pending.set(true);
                let d2 = d.clone();
                glib::timeout_add_local_once(std::time::Duration::from_millis(SETTLE_MS + 30), move || {
                    d2.settle_pending.set(false);
                    d2.canvas.queue_draw();
                });
            }
            if fitted {
                let u2 = u.clone();
                glib::idle_add_local_once(move || refresh(&u2));
            }
            schedule_float(&u, &d);
        });
    }
    {
        let c = canvas.clone();
        adw::StyleManager::default().connect_dark_notify(move |_| c.queue_draw());
    }

    // Linke Maustaste: Werkzeuge (Klick in die Linealecke wechselt die Einheit)
    let drag = gtk::GestureDrag::new();
    drag.set_button(gdk::BUTTON_PRIMARY);
    let in_corner = Rc::new(Cell::new(false));
    {
        let (u, d, corner) = (ui.clone(), d.clone(), in_corner.clone());
        drag.connect_drag_begin(move |g, x, y| {
            d.canvas.grab_focus();
            // Pipette: Farbe aufnehmen statt zu bearbeiten (der Rest der Geste wird ignoriert)
            if pipette_active(&u) {
                corner.set(true);
                let c = render::sample_color(&d.ed.borrow(), x, y);
                match c {
                    Some(c) => pipette_pick(&u, c),
                    None => u.toast(tr("Hier ist keine Farbe – auf die Seite klicken")),
                }
                return;
            }
            if u.show_rulers.get() && x < RULER && y < RULER {
                corner.set(true);
                {
                    let mut ed = d.ed.borrow_mut();
                    let next = doc_unit(&u, &ed).next();
                    ed.doc.unit = next.name().into();
                }
                d.canvas.queue_draw();
                return;
            }
            corner.set(false);
            let m = mods(g.current_event_state());
            {
                let mut ed = d.ed.borrow_mut();
                ed.ruler = if u.show_rulers.get() { RULER } else { 0.0 };
                ed.press(Pt::new(x, y), m, false, &u.metrics);
            }
            refresh(&u);
        });
    }
    {
        let (u, d, corner) = (ui.clone(), d.clone(), in_corner.clone());
        drag.connect_drag_update(move |g, dx, dy| {
            if corner.get() {
                return;
            }
            if let Some((sx, sy)) = g.start_point() {
                let m = mods(g.current_event_state());
                d.ed.borrow_mut().motion(Pt::new(sx + dx, sy + dy), m, &u.metrics);
                ensure_ticker(&d);
                refresh(&u);
            }
        });
    }
    {
        let (u, d, corner) = (ui.clone(), d.clone(), in_corner);
        drag.connect_drag_end(move |g, dx, dy| {
            if corner.get() {
                return;
            }
            if let Some((sx, sy)) = g.start_point() {
                let m = mods(g.current_event_state());
                d.ed.borrow_mut()
                    .release(Pt::new(sx + dx, sy + dy), m, &u.metrics);
                refresh(&u);
            }
        });
    }
    canvas.add_controller(drag);

    // Mittlere Maustaste: Ansicht verschieben
    let pan = gtk::GestureDrag::new();
    pan.set_button(gdk::BUTTON_MIDDLE);
    {
        let (u, d) = (ui.clone(), d.clone());
        pan.connect_drag_begin(move |_, x, y| {
            d.ed.borrow_mut()
                .press(Pt::new(x, y), Mods::default(), true, &u.metrics);
            refresh(&u);
        });
    }
    {
        let (u, d) = (ui.clone(), d.clone());
        pan.connect_drag_update(move |g, dx, dy| {
            if let Some((sx, sy)) = g.start_point() {
                d.ed.borrow_mut()
                    .motion(Pt::new(sx + dx, sy + dy), Mods::default(), &u.metrics);
                refresh(&u);
            }
        });
    }
    {
        let (u, d) = (ui.clone(), d.clone());
        pan.connect_drag_end(move |g, dx, dy| {
            if let Some((sx, sy)) = g.start_point() {
                d.ed.borrow_mut()
                    .release(Pt::new(sx + dx, sy + dy), Mods::default(), &u.metrics);
                refresh(&u);
            }
        });
    }
    canvas.add_controller(pan);

    // Doppelklick
    let dbl = gtk::GestureClick::new();
    dbl.set_button(gdk::BUTTON_PRIMARY);
    {
        let (u, d) = (ui.clone(), d.clone());
        dbl.connect_pressed(move |_, n, x, y| {
            if n == 2 && !(u.show_rulers.get() && x < RULER && y < RULER) {
                d.ed.borrow_mut().double_click(Pt::new(x, y), &u.metrics);
                refresh(&u);
            }
        });
    }
    canvas.add_controller(dbl);

    // Rechtsklick: Kontextmenü
    let rc = gtk::GestureClick::new();
    rc.set_button(gdk::BUTTON_SECONDARY);
    {
        let (u, d) = (ui.clone(), d.clone());
        rc.connect_pressed(move |_, _, x, y| {
            {
                let mut ed = d.ed.borrow_mut();
                ed.finish_pen(false);
                let p = ed.view.to_doc(Pt::new(x, y));
                if let Some(id) = ed.hit(p, &u.metrics) {
                    if !ed.selection.contains(&id) {
                        ed.selection = vec![id];
                        ed.node_sel.clear();
                    }
                }
            }
            refresh(&u);
            popup_menu(
                &d,
                &context_menu(&d),
                gdk::Rectangle::new(x as i32, y as i32, 1, 1),
                None,
            );
        });
    }
    canvas.add_controller(rc);
    // „Weitere Möglichkeiten“ in der Schwebeleiste: dasselbe Menü wie beim Rechtsklick
    {
        let d2 = d.clone();
        d.float.more.connect_clicked(move |b| {
            let rect = match b.compute_bounds(&d2.canvas) {
                Some(r) => {
                    gdk::Rectangle::new(r.x() as i32, r.y() as i32, r.width() as i32, r.height() as i32)
                }
                None => gdk::Rectangle::new(0, 0, 1, 1),
            };
            popup_menu(&d2, &context_menu(&d2), rect, None);
        });
    }
    // „Effekte“ in der Schwebeleiste: wirkt auf alle ausgewählten Objekte
    {
        let d2 = d.clone();
        d.float.effects.connect_clicked(move |b| {
            let rect = match b.compute_bounds(&d2.canvas) {
                Some(r) => {
                    gdk::Rectangle::new(r.x() as i32, r.y() as i32, r.width() as i32, r.height() as i32)
                }
                None => gdk::Rectangle::new(0, 0, 1, 1),
            };
            show_effect_menu(&d2, rect);
        });
    }
    // „Nachzeichnen“ in der Schwebeleiste: Bild mit flachen Farbflächen in Vektoren verwandeln
    {
        let (u, d2) = (ui.clone(), d.clone());
        d.float.trace_make.connect_clicked(move |_| {
            d2.float.trace_colors.update();
            let colors = d2.float.trace_colors.value().round().clamp(2.0, 24.0) as usize;
            d2.float.trace.popdown();
            let img = d2.ed.borrow().traceable_image();
            if let Some((id, data)) = img {
                match crate::render::image_rgba(&data, 900) {
                    Some((px, w, h)) => {
                        let layers = crate::trace::trace(&px, w, h, colors);
                        d2.ed.borrow_mut().replace_with_traced(id, layers, w, h);
                    }
                    None => {
                        d2.ed.borrow_mut().message = Some(tr("Das Bild lässt sich nicht lesen").into());
                    }
                }
            }
            refresh(&u);
        });
    }

    // Zeigerbewegung: Hervorhebung, Zeichenstift-Vorschau, Cursor, Lineal-Markierung
    let motion = gtk::EventControllerMotion::new();
    {
        let (u, d) = (ui.clone(), d.clone());
        motion.connect_motion(move |c, x, y| {
            u.pointer.set((x, y));
            if pipette_active(&u) {
                let col = render::sample_color(&d.ed.borrow(), x, y);
                u.pipette_at.set(Some((x, y, col)));
                d.canvas.set_cursor_from_name(Some("crosshair"));
                d.canvas.queue_draw();
                return;
            }
            let s = Pt::new(x, y);
            let cursor = {
                let mut ed = d.ed.borrow_mut();
                if matches!(ed.drag, Drag::None) {
                    ed.motion(s, mods(c.current_event_state()), &u.metrics);
                }
                if u.show_rulers.get() && (x < RULER || y < RULER) {
                    "default"
                } else {
                    ed.cursor_at(s, &u.metrics)
                }
            };
            d.canvas.set_cursor_from_name(Some(cursor));
            ensure_ticker(&d);
            d.canvas.queue_draw();
        });
    }
    {
        let (u, d) = (ui.clone(), d.clone());
        motion.connect_leave(move |_| {
            u.pipette_at.set(None);
            {
                let mut ed = d.ed.borrow_mut();
                ed.hover = None;
                if matches!(ed.drag, Drag::None) {
                    ed.pointer = None;
                    ed.hints.borrow_mut().clear();
                }
            }
            d.canvas.queue_draw();
        });
    }
    canvas.add_controller(motion);

    // Mausrad / Touchpad: verschieben, mit Strg zoomen
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    {
        let (u, d) = (ui.clone(), d.clone());
        scroll.connect_scroll(move |c, dx, dy| {
            let st = c.current_event_state();
            let wheel = c.unit() == gdk::ScrollUnit::Wheel;
            let (px, py) = u.pointer.get();
            {
                let mut ed = d.ed.borrow_mut();
                if st.contains(primary_mask()) {
                    let f = if wheel {
                        1.2f64.powf(-dy)
                    } else {
                        (-dy * 0.01).exp()
                    };
                    ed.zoom_at(f, Pt::new(px, py));
                } else {
                    let k = if wheel { 40.0 } else { 1.0 };
                    let (mut dx, mut dy) = (dx * k, dy * k);
                    if st.contains(gdk::ModifierType::SHIFT_MASK) && dx == 0.0 {
                        std::mem::swap(&mut dx, &mut dy);
                    }
                    if ed.notebook.is_some() {
                        // Notizheft: Seite bleibt im Bild, am Ende wird umgeblättert
                        ed.notebook_scroll(dx, dy);
                    } else {
                        ed.view.ox -= dx;
                        ed.view.oy -= dy;
                    }
                }
            }
            refresh(&u);
            glib::Propagation::Stop
        });
    }
    canvas.add_controller(scroll);

    // Touchpad-Zoom (zwei Finger)
    let zoom = gtk::GestureZoom::new();
    let start = Rc::new(Cell::new((1.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64)));
    {
        let (u, d, start) = (ui.clone(), d.clone(), start.clone());
        zoom.connect_begin(move |g, _| {
            let (cx, cy) = g.bounding_box_center().unwrap_or(u.pointer.get());
            let v = d.ed.borrow().view;
            start.set((v.zoom, v.ox, v.oy, cx, cy));
        });
    }
    {
        let (u, d) = (ui.clone(), d.clone());
        zoom.connect_scale_changed(move |_, scale| {
            let (z0, ox0, oy0, cx, cy) = start.get();
            {
                let mut ed = d.ed.borrow_mut();
                let z = (z0 * scale).clamp(0.02, 64.0);
                ed.view.zoom = z;
                ed.view.ox = cx - (cx - ox0) * z / z0;
                ed.view.oy = cy - (cy - oy0) * z / z0;
            }
            refresh(&u);
        });
    }
    canvas.add_controller(zoom);

    // Aus der Bibliothek auf ein Objekt ziehen: kurz warten, Vorschau, loslassen übernimmt
    let lib_drop = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::COPY);
    lib_drop.set_preload(true);
    {
        let (u, d) = (ui.clone(), d.clone());
        lib_drop.connect_motion(move |t, x, y| {
            let payload = t
                .value()
                .and_then(|v| v.get::<String>().ok())
                .and_then(|s| lib_payload(&u, &s));
            let Some(payload) = payload else {
                return gdk::DragAction::empty();
            };
            d.ed.borrow_mut()
                .lib_drag_motion(Pt::new(x, y), payload, &u.metrics);
            ensure_ticker(&d);
            d.canvas.queue_draw();
            gdk::DragAction::COPY
        });
    }
    {
        let d = d.clone();
        lib_drop.connect_leave(move |_| {
            d.ed.borrow_mut().lib_drag_leave();
            d.canvas.queue_draw();
        });
    }
    {
        let (u, d) = (ui.clone(), d.clone());
        lib_drop.connect_drop(move |_, value, x, y| {
            let Some(payload) = value.get::<String>().ok().and_then(|s| lib_payload(&u, &s)) else {
                return false;
            };
            let ok = d.ed.borrow_mut().lib_drop_at(Pt::new(x, y), &payload, &u.metrics);
            let u2 = u.clone();
            glib::idle_add_local_once(move || refresh(&u2));
            ok
        });
    }
    canvas.add_controller(lib_drop);

    // Dateien hineinziehen: Bilder werden eingefügt, SVG-Dateien als Tab geöffnet.
    let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
    {
        // Über einer Form kurz halten: das Bild kommt in die Form
        let (u, d) = (ui.clone(), d.clone());
        drop.connect_motion(move |_, x, y| {
            d.ed.borrow_mut().drop_hover(Pt::new(x, y), &u.metrics);
            ensure_ticker(&d);
            d.canvas.queue_draw();
            gdk::DragAction::COPY
        });
    }
    {
        let d = d.clone();
        drop.connect_leave(move |_| {
            d.ed.borrow_mut().frame_hover = None;
            d.canvas.queue_draw();
        });
    }
    {
        let (u, d) = (ui.clone(), d.clone());
        drop.connect_drop(move |_, value, x, y| {
            let Ok(list) = value.get::<gdk::FileList>() else {
                return false;
            };
            let frame = {
                let mut ed = d.ed.borrow_mut();
                let f = ed.frame_ready();
                ed.frame_hover = None;
                f
            };
            let at = d.ed.borrow().view.to_doc(Pt::new(x, y));
            let mut any = false;
            let mut framed = false;
            for file in list.files() {
                let Some(path) = file.path() else { continue };
                let is_svg = path
                    .extension()
                    .map(|e| e.eq_ignore_ascii_case("svg"))
                    .unwrap_or(false)
                    || is_import_file(&path);
                if is_svg {
                    open_path(&u, &path);
                    any = true;
                } else if insert_image_file(&u, &d, &path, Some(at)) {
                    any = true;
                    if let (Some(t), false) = (frame, framed) {
                        framed = true;
                        let img = d.ed.borrow().selection.first().copied();
                        if let Some(img) = img {
                            d.ed.borrow_mut().place_in_frame(img, t, &u.metrics);
                            refresh(&u);
                        }
                    }
                }
            }
            any
        });
    }
    canvas.add_controller(drop);

    // Tastatur beim Bearbeiten von Text direkt im Dokument
    {
        let (u, d2) = (ui.clone(), d.clone());
        d.keys
            .connect_key_pressed(move |_, key, _, st| handle_edit_key(&u, &d2, key, st));
    }
    canvas.add_controller(d.keys.clone());
    {
        let (u, d2) = (ui.clone(), d.clone());
        d.im.connect_commit(move |_, text| {
            let changed = d2.ed.borrow_mut().edit_insert(text);
            if changed {
                refresh(&u);
            }
        });
    }
    let focus = gtk::EventControllerFocus::new();
    {
        let d2 = d.clone();
        focus.connect_enter(move |_| {
            if d2.im_on.get() {
                d2.im.focus_in();
            }
        });
    }
    {
        let d2 = d.clone();
        focus.connect_leave(move |_| {
            if d2.im_on.get() {
                d2.im.focus_out();
            }
        });
    }
    canvas.add_controller(focus);
}

/// Solange ein Bild über einer Form wartet: regelmäßig neu zeichnen (Wartekreis läuft).
fn ensure_ticker(d: &Rc<DocTab>) {
    fn waiting(d: &DocTab) -> bool {
        let ed = d.ed.borrow();
        ed.frame_hover.is_some() || ed.lib_drop.is_some()
    }
    if d.ticking.get() || !waiting(d) {
        return;
    }
    d.ticking.set(true);
    let d2 = d.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(60), move || {
        d2.canvas.queue_draw();
        if waiting(&d2) {
            glib::ControlFlow::Continue
        } else {
            d2.ticking.set(false);
            glib::ControlFlow::Break
        }
    });
}

/// Eingabemethode nur anhängen, solange Text im Dokument bearbeitet wird –
/// sonst bleiben die Buchstaben-Kürzel der Werkzeuge erhalten.
fn sync_im(d: &Rc<DocTab>) {
    let editing = d.ed.borrow().text_edit.is_some();
    if editing == d.im_on.get() {
        return;
    }
    d.im_on.set(editing);
    if editing {
        d.keys.set_im_context(Some(&d.im));
        d.im.focus_in();
        if !d.canvas.has_focus() {
            d.canvas.grab_focus();
        }
    } else {
        // Erst nach dem laufenden Tastenereignis lösen.
        let d2 = d.clone();
        glib::idle_add_local_once(move || {
            if !d2.im_on.get() {
                d2.im.focus_out();
                d2.im.reset();
                d2.keys.set_im_context(None::<&gtk::IMContext>);
            }
        });
    }
}

/// Tasten beim Bearbeiten von Text im Dokument (Cursor, Markierung, Zwischenablage).
fn handle_edit_key(ui: &Rc<Ui>, d: &Rc<DocTab>, key: gdk::Key, st: gdk::ModifierType) -> glib::Propagation {
    if d.ed.borrow().text_edit.is_none() {
        return glib::Propagation::Proceed;
    }
    let m = mods(st);
    let handled = {
        let mut ed = d.ed.borrow_mut();
        let mv = |plain: Move, word: Move| if m.ctrl { word } else { plain };
        match key {
            gdk::Key::Escape => {
                ed.escape();
                true
            }
            gdk::Key::BackSpace => {
                ed.edit_backspace();
                true
            }
            gdk::Key::Delete | gdk::Key::KP_Delete => {
                ed.edit_delete();
                true
            }
            gdk::Key::Left | gdk::Key::KP_Left => ed.edit_move(mv(Move::Left, Move::WordLeft), m.shift),
            gdk::Key::Right | gdk::Key::KP_Right => ed.edit_move(mv(Move::Right, Move::WordRight), m.shift),
            gdk::Key::Up | gdk::Key::KP_Up => ed.edit_move(Move::Up, m.shift),
            gdk::Key::Down | gdk::Key::KP_Down => ed.edit_move(Move::Down, m.shift),
            gdk::Key::Home | gdk::Key::KP_Home => ed.edit_move(Move::Home, m.shift),
            gdk::Key::End | gdk::Key::KP_End => ed.edit_move(Move::End, m.shift),
            gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter => {
                // Enter: neuer Absatz; Umschalt+Enter im Textfeld: Zeilenumbruch im Absatz
                let area = ed
                    .text_edit
                    .as_ref()
                    .and_then(|t| ed.doc.object(t.id))
                    .map(|o| matches!(o.shape, crate::model::Shape::TextArea { .. }))
                    .unwrap_or(false);
                ed.edit_insert(if m.shift && area { "\u{2028}" } else { "\n" });
                true
            }
            gdk::Key::Tab | gdk::Key::KP_Tab => ed.edit_insert("\t"),
            gdk::Key::ISO_Left_Tab => true,
            _ if m.ctrl && !m.alt => match key.to_lower().to_unicode() {
                Some('a') => ed.edit_select_all(),
                Some(c @ ('c' | 'x')) => {
                    if let Some(t) = ed.edit_selected_text() {
                        d.canvas.clipboard().set_text(&t);
                        if c == 'x' {
                            ed.edit_delete();
                        }
                    }
                    true
                }
                Some('v') => {
                    let (u, d2) = (ui.clone(), d.clone());
                    d.canvas
                        .clipboard()
                        .read_text_async(gio::Cancellable::NONE, move |res| {
                            if let Ok(Some(t)) = res {
                                d2.ed.borrow_mut().edit_insert(&t);
                                refresh(&u);
                            }
                        });
                    true
                }
                _ => false,
            },
            _ if !m.ctrl && !m.alt => {
                // Ohne Eingabemethode (z. B. Fokus woanders): Zeichen direkt einfügen.
                match key.to_unicode().filter(|c| !c.is_control()) {
                    Some(c) => ed.edit_insert(&c.to_string()),
                    None => false,
                }
            }
            _ => false,
        }
    };
    if handled {
        refresh(ui);
        glib::Propagation::Stop
    } else {
        glib::Propagation::Proceed
    }
}

/// Ausschneiden (0), Kopieren (1), Einfügen (2) – über alle Tabs hinweg: die
/// Zwischenablage gehört dem Fenster, nicht dem einzelnen Dokument.
fn clipboard_op(ui: &Ui, d: &Rc<DocTab>, ed: &mut Editor, kind: u8) -> bool {
    let me = Rc::as_ptr(d) as usize;
    match kind {
        0 | 1 => {
            let ok = if kind == 0 { ed.cut() } else { ed.copy() };
            if ok {
                *ui.clipboard.borrow_mut() = (me, ed.clipboard.clone());
            }
            ok
        }
        _ => {
            let (src, objs) = ui.clipboard.borrow().clone();
            if src != me && !objs.is_empty() {
                ed.paste_objects(objs)
            } else {
                ed.paste()
            }
        }
    }
}

fn handle_key(ui: &Rc<Ui>, key: gdk::Key, st: gdk::ModifierType) -> glib::Propagation {
    // Tasten nicht abfangen, während in ein Textfeld geschrieben wird.
    if let Some(f) = gtk::prelude::RootExt::focus(&ui.window) {
        if f.is::<gtk::Text>() || f.is::<gtk::TextView>() {
            return glib::Propagation::Proceed;
        }
    }
    if key == gdk::Key::Escape && pipette_active(ui) {
        end_pipette(ui);
        return glib::Propagation::Stop;
    }
    let Some(d) = cur(ui) else {
        return glib::Propagation::Proceed;
    };
    if d.ed.borrow().text_edit.is_some() {
        return handle_edit_key(ui, &d, key, st);
    }
    let m = mods(st);
    let step = if m.shift { 10.0 } else { 1.0 };
    let mut handled = true;
    {
        let mut ed = d.ed.borrow_mut();
        let lower = key.to_lower();
        let drawing = ed.pen.is_some() || ed.curv.is_some();
        if key == gdk::Key::BackSpace && drawing {
            ed.remove_last_point();
        } else if key == gdk::Key::Delete || key == gdk::Key::BackSpace || key == gdk::Key::KP_Delete {
            ed.delete_selection();
        } else if key == gdk::Key::Escape {
            ed.escape();
        } else if key == gdk::Key::Return || key == gdk::Key::KP_Enter {
            handled = ed.finish_drawing();
        } else if key == gdk::Key::Left {
            handled = ed.nudge(-step, 0.0);
        } else if key == gdk::Key::Right {
            handled = ed.nudge(step, 0.0);
        } else if key == gdk::Key::Up {
            handled = ed.nudge(0.0, -step);
        } else if key == gdk::Key::Down {
            handled = ed.nudge(0.0, step);
        } else if matches!(key, gdk::Key::Page_Down | gdk::Key::Page_Up) && !m.ctrl && ed.notebook.is_some() {
            let i = ed.page_index().unwrap_or(0);
            handled = if key == gdk::Key::Page_Down {
                ed.go_page(i + 1, false)
            } else {
                i > 0 && ed.go_page(i - 1, false)
            };
        } else if key == gdk::Key::space && !m.ctrl {
            ed.space_down = true;
        } else if m.ctrl {
            match lower.to_unicode() {
                Some('c') => handled = clipboard_op(ui, &d, &mut ed, 1),
                Some('x') => handled = clipboard_op(ui, &d, &mut ed, 0),
                Some('v') => handled = clipboard_op(ui, &d, &mut ed, 2),
                Some('a') => ed.select_all(),
                _ => handled = false,
            }
        } else if ed.tool == Tool::Eraser && (key == gdk::Key::bracketleft || key == gdk::Key::bracketright) {
            let f = if key == gdk::Key::bracketright { 1.25 } else { 0.8 };
            ed.eraser_size = (ed.eraser_size * f).clamp(1.0, 2000.0);
        } else if !m.alt {
            // Werkzeug- und Spiegeltasten laut eingestellter Belegung
            let name = gtk::accelerator_name(lower, accel_mods(st));
            let found = KEYMAP.with(|k| k.borrow().find_canvas(name.as_str()).map(|d| d.kind));
            match found {
                Some(crate::keys::Kind::Tool(t)) => {
                    // nochmals die Taste des aktiven Werkzeugs: nächste Variante der Gruppe
                    let group = (ed.tool == t)
                        .then(|| {
                            ui.slots
                                .iter()
                                .find(|s| s.variants.len() > 1 && s.variants.iter().any(|v| v.tool == t))
                        })
                        .flatten();
                    match group {
                        Some(s) => {
                            let v = s.variants[(s.current.get() + 1) % s.variants.len()];
                            if let Some(k) = v.kind {
                                ed.shape_kind = k;
                            }
                            ed.set_tool(v.tool);
                        }
                        None => ed.set_tool(t),
                    }
                }
                Some(crate::keys::Kind::Flip { horizontal }) => {
                    handled = ed.flip_selection(horizontal, &ui.metrics)
                }
                _ => handled = false,
            }
        } else {
            handled = false;
        }
    }
    if handled {
        refresh(ui);
        d.canvas.grab_focus();
        glib::Propagation::Stop
    } else {
        glib::Propagation::Proceed
    }
}

/// Gruppe an das aktive Werkzeug anpassen: enthält sie es, ist sie aufgeklappt und die
/// Variante gedrückt; sonst zeigt sie nur die zuletzt benutzte Variante.
fn sync_slot(slot: &Slot, tool: Tool, kind: ShapeKind) {
    let found = slot
        .variants
        .iter()
        .position(|v| v.tool == tool && (v.kind.is_none() || v.kind == Some(kind)));
    if let Some(i) = found {
        slot.current.set(i);
        if !slot.buttons[i].is_active() {
            slot.buttons[i].set_active(true);
        }
    }
    let open = found.is_some() && slot.variants.len() > 1;
    for (j, r) in slot.revealers.iter().enumerate() {
        let show = open || j == slot.current.get();
        if r.reveals_child() != show {
            r.set_reveal_child(show);
        }
    }
    // aufgeklappt braucht es den Pfeil nicht mehr
    for a in &slot.arrows {
        a.set_visible(!open);
    }
    if open {
        slot.group.add_css_class("mondrian-tool-group-open");
    } else {
        slot.group.remove_css_class("mondrian-tool-group-open");
    }
}

/// Zweite Leiste: nur, was das aktive Werkzeug einzustellen hat.
fn sync_tool_opts(ui: &Ui, d: &DocTab, tool: Tool, kind: ShapeKind) {
    let o = &ui.tool_opts;
    let ed = d.ed.borrow();
    let poly = tool == Tool::Shape && matches!(kind, ShapeKind::Polygon | ShapeKind::Star);
    o.corners_box.set_visible(poly);
    o.eraser_box.set_visible(tool == Tool::Eraser);
    o.pencil_box.set_visible(tool == Tool::Pencil);
    o.bar
        .set_visible(poly || matches!(tool, Tool::Eraser | Tool::Pencil));
    o.corners.set_value(ed.corners as f64);
    o.eraser.set_value(ed.eraser_size.round());
    o.smooth.set_value((ed.pencil_smooth * 100.0).round());
    o.close.set_value(ed.pencil_close);
}

/// Aktuelle Variante eines Knopfes als Werkzeug setzen.
fn activate_slot(ui: &Rc<Ui>, slot: &Slot) {
    let v = slot.variants[slot.current.get()];
    ui.tool.set(v.tool);
    if let Some(d) = cur(ui) {
        {
            let mut ed = d.ed.borrow_mut();
            if let Some(k) = v.kind {
                ed.shape_kind = k;
            }
            ed.set_tool(v.tool);
        }
        refresh(ui);
        d.canvas.grab_focus();
    }
}

fn setup_tools(ui: &Rc<Ui>) {
    for slot in &ui.slots {
        for (i, b) in slot.buttons.iter().enumerate() {
            let (u, sl) = (ui.clone(), slot.clone());
            b.connect_toggled(move |b| {
                if u.updating.get() || !b.is_active() {
                    return;
                }
                sl.current.set(i);
                activate_slot(&u, &sl);
            });
        }
    }
    // Einstellungen des Werkzeugs
    let o = &ui.tool_opts;
    {
        let u = ui.clone();
        o.corners.connect_value_changed(move |s| {
            let v = s.value() as u32;
            with_ed(&u, |e| {
                e.set_corners(v);
            });
        });
    }
    {
        let u = ui.clone();
        o.eraser.connect_value_changed(move |s| {
            let v = s.value();
            with_ed(&u, |e| e.eraser_size = v.max(1.0));
        });
    }
    {
        let u = ui.clone();
        o.smooth.connect_value_changed(move |s| {
            let v = (s.value() / 100.0).clamp(0.0, 1.0);
            with_ed(&u, |e| e.pencil_smooth = v);
        });
    }
    {
        let u = ui.clone();
        o.close.connect_value_changed(move |s| {
            let v = s.value();
            with_ed(&u, |e| e.pencil_close = v);
        });
    }
}

// ───────────────────────────── Eigenschaftenleiste ─────────────────────────────

/// Änderung aus der Eigenschaftenleiste auf das aktuelle Dokument anwenden.
fn with_ed(ui: &Rc<Ui>, f: impl FnOnce(&mut Editor)) {
    if ui.updating.get() {
        return;
    }
    let Some(d) = cur(ui) else { return };
    f(&mut d.ed.borrow_mut());
    refresh(ui);
}

/// Wie `with_ed`, aber mit Schriftmetriken (für Drehen, Verläufe, Textfelder).
fn with_ed_m(ui: &Rc<Ui>, f: impl FnOnce(&mut Editor, &CairoMetrics)) {
    if ui.updating.get() {
        return;
    }
    let Some(d) = cur(ui) else { return };
    f(&mut d.ed.borrow_mut(), &ui.metrics);
    refresh(ui);
}

/// Waagerechter Innenabstand der Verlaufsleiste.
const GRAD_PAD: f64 = 8.0;
/// Höhe des Farbbalkens der Verlaufsleiste.
const GRAD_BAR_H: f64 = 18.0;

/// Verlauf des ersten gewählten Objekts (für die Verlaufsleiste).
fn panel_gradient(ui: &Ui) -> Option<crate::model::Gradient> {
    let d = cur(ui)?;
    let ed = d.ed.borrow();
    let g = ed
        .selected_objects()
        .into_iter()
        .find_map(|o| o.style.gradient.clone());
    g
}

fn draw_gradient_bar(ui: &Ui, area: &gtk::DrawingArea, cr: &gtk::cairo::Context, w: f64) {
    let Some(g) = panel_gradient(ui) else { return };
    let sel = match cur(ui) {
        Some(d) => {
            let s = d.ed.borrow().grad_stop;
            s
        }
        None => 0,
    };
    let (x0, x1) = (GRAD_PAD, (w - GRAD_PAD).max(GRAD_PAD + 1.0));
    // Schachbrett für transparente Farben
    let sq = 6.0;
    let mut x = x0;
    let mut col = 0;
    while x < x1 {
        for row in 0..3 {
            let light = (col + row) % 2 == 0;
            let v = if light { 0.95 } else { 0.8 };
            cr.set_source_rgb(v, v, v);
            cr.rectangle(x, 2.0 + row as f64 * sq, sq.min(x1 - x), sq);
            let _ = cr.fill();
        }
        x += sq;
        col += 1;
    }
    let lg = gtk::cairo::LinearGradient::new(x0, 0.0, x1, 0.0);
    for (pos, c) in g.render_stops() {
        lg.add_color_stop_rgba(pos.clamp(0.0, 1.0), c.r, c.g, c.b, c.a);
    }
    cr.rectangle(x0, 2.0, x1 - x0, GRAD_BAR_H);
    let _ = cr.set_source(&lg);
    let _ = cr.fill_preserve();
    let fg = area.color();
    cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.4);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
    // Farbstopps als kleine Fähnchen unter dem Balken
    for (i, st) in g.stops.iter().enumerate() {
        let x = x0 + st.pos.clamp(0.0, 1.0) * (x1 - x0);
        let y = 2.0 + GRAD_BAR_H + 1.0;
        cr.new_path();
        cr.move_to(x, y);
        cr.line_to(x + 6.0, y + 6.0);
        cr.line_to(x + 6.0, y + 15.0);
        cr.line_to(x - 6.0, y + 15.0);
        cr.line_to(x - 6.0, y + 6.0);
        cr.close_path();
        cr.set_source_rgba(st.color.r, st.color.g, st.color.b, 1.0);
        let _ = cr.fill_preserve();
        if i == sel {
            let a = render::ui_accent();
            cr.set_source_rgb(a.0, a.1, a.2);
            cr.set_line_width(2.5);
        } else {
            cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.7);
            cr.set_line_width(1.0);
        }
        let _ = cr.stroke();
    }
}

fn setup_gradient_bar(ui: &Rc<Ui>) {
    let bar = &ui.panel.grad_bar;
    {
        let wu = Rc::downgrade(ui);
        bar.set_draw_func(move |area, cr, w, _| {
            if let Some(u) = wu.upgrade() {
                draw_gradient_bar(&u, area, cr, w as f64);
            }
        });
    }
    let drag = gtk::GestureDrag::new();
    let dragging: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));
    let to_t = |area: &gtk::DrawingArea, x: f64| {
        let w = area.width() as f64;
        ((x - GRAD_PAD) / (w - 2.0 * GRAD_PAD).max(1.0)).clamp(0.0, 1.0)
    };
    {
        let (u, dr, b) = (ui.clone(), dragging.clone(), bar.clone());
        drag.connect_drag_begin(move |_, x, _| {
            let Some(g) = panel_gradient(&u) else { return };
            let w = (b.width() as f64 - 2.0 * GRAD_PAD).max(1.0);
            let near = g
                .stops
                .iter()
                .enumerate()
                .map(|(i, st)| (i, (GRAD_PAD + st.pos * w - x).abs()))
                .filter(|(_, d)| *d <= 7.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i);
            let t = to_t(&b, x);
            match near {
                Some(i) => {
                    dr.set(Some(i));
                    with_ed(&u, |e| e.grad_stop = i);
                }
                None => {
                    // Klick auf den Balken: neue Farbe an dieser Stelle
                    let mut idx = None;
                    with_ed(&u, |e| {
                        if e.add_stop(t) {
                            idx = Some(e.grad_stop);
                        }
                    });
                    dr.set(idx);
                }
            }
        });
    }
    {
        let (u, dr, b) = (ui.clone(), dragging.clone(), bar.clone());
        drag.connect_drag_update(move |g, dx, _| {
            let (Some(i), Some((sx, _))) = (dr.get(), g.start_point()) else {
                return;
            };
            let t = to_t(&b, sx + dx);
            with_ed(&u, |e| {
                e.set_stop_pos(i, t);
            });
        });
    }
    {
        let dr = dragging.clone();
        drag.connect_drag_end(move |_, _, _| dr.set(None));
    }
    bar.add_controller(drag);
}

fn setup_panel(ui: &Rc<Ui>) {
    setup_gradient_bar(ui);
    let c = &ui.panel;
    // Textausrichtung
    for (i, b) in c.align.iter().enumerate() {
        let u = ui.clone();
        b.connect_toggled(move |b| {
            if !b.is_active() {
                return;
            }
            let a = Align::ALL[i];
            with_ed_m(&u, |e, m| {
                e.set_text_align(a);
                e.refill_placeholders(m);
            });
        });
    }
    for (i, b) in c.valign.iter().enumerate() {
        let u = ui.clone();
        b.connect_toggled(move |b| {
            if !b.is_active() {
                return;
            }
            let a = VAlign::ALL[i];
            with_ed(&u, |e| {
                e.update_text("valign", move |t| t.fmt.valign = a);
            });
        });
    }
    {
        let u = ui.clone();
        c.line_spacing.connect_value_changed(move |s| {
            let v = s.value() / 100.0;
            with_ed_m(&u, |e, m| {
                e.update_text("line", move |t| t.fmt.line = v);
                e.refill_placeholders(m);
            });
        });
    }
    {
        let u = ui.clone();
        c.tracking.connect_value_changed(move |s| {
            let v = s.value();
            with_ed_m(&u, |e, m| {
                e.update_text("tracking", move |t| t.fmt.tracking = v);
                e.refill_placeholders(m);
            });
        });
    }
    {
        let u = ui.clone();
        c.angle.connect_value_changed(move |s| {
            let v = s.value();
            with_ed_m(&u, |e, m| {
                e.set_rotation(v, m);
            });
        });
    }
    // Effekte: Auswahl innerhalb der Kategorie und Stärke
    {
        let u = ui.clone();
        c.blend_fold.choice.connect_selected_notify(move |dd| {
            let bl = Blend::SIMPLE[1 + (dd.selected() as usize).min(Blend::SIMPLE.len() - 2)];
            with_ed(&u, |e| {
                e.set_blend(bl);
            });
        });
    }
    {
        let u = ui.clone();
        c.art_fold.choice.connect_selected_notify(move |dd| {
            let a = crate::model::Art::ALL[(dd.selected() as usize).min(crate::model::Art::ALL.len() - 1)];
            with_ed(&u, |e| {
                e.update_style("art", move |s| {
                    let k = s.fx.art.map(|x| x.1).unwrap_or(crate::model::FX_DEFAULT);
                    s.fx.art = Some((a, k));
                });
            });
        });
    }
    {
        let u = ui.clone();
        c.tint_fold.choice.connect_selected_notify(move |dd| {
            let f = crate::model::ImgFilter::ALL
                [(dd.selected() as usize).min(crate::model::ImgFilter::ALL.len() - 1)];
            with_ed(&u, |e| {
                e.update_objects("tint", is_image, move |s| {
                    let k = match s.fx.tint {
                        Some((old, k)) if old == f => k,
                        _ => f.default_strength(),
                    };
                    s.fx.tint = Some((f, k));
                });
            });
        });
    }
    {
        let u = ui.clone();
        c.lens_fold.choice.connect_selected_notify(move |dd| {
            let l = crate::model::Lens::ALL[(dd.selected() as usize).min(crate::model::Lens::ALL.len() - 1)];
            with_ed(&u, |e| {
                e.update_objects("lens", not_image, move |s| {
                    let k = s.fx.lens.map(|x| x.1).unwrap_or(crate::model::FX_DEFAULT);
                    s.fx.lens = Some((l, k));
                });
            });
        });
    }
    {
        let u = ui.clone();
        c.lens_fold.strength2.connect_value_changed(move |sc| {
            let v = (sc.value() / 100.0).clamp(0.0, 1.0);
            with_ed(&u, |e| {
                e.update_objects("lens-light", not_image, move |s| {
                    if s.fx.lens.is_some() {
                        s.fx.light = v;
                    }
                });
            });
        });
    }
    {
        let u = ui.clone();
        c.tex_fold.choice.connect_selected_notify(move |dd| {
            let t = crate::model::Texture::ALL
                [(dd.selected() as usize).min(crate::model::Texture::ALL.len() - 1)];
            with_ed(&u, |e| {
                e.update_style("tex", move |s| {
                    let k = s.fx.tex.map(|x| x.1).unwrap_or(t.default_strength());
                    s.fx.tex = Some((t, k));
                });
            });
        });
    }
    {
        let u = ui.clone();
        c.tex_fold.strength.connect_value_changed(move |sc| {
            let k = sc.value() / 100.0;
            with_ed(&u, |e| {
                e.update_style("tex-k", move |s| {
                    if let Some((t, _)) = s.fx.tex {
                        s.fx.tex = Some((t, k));
                    }
                });
            });
        });
    }
    {
        let u = ui.clone();
        c.art_fold.strength.connect_value_changed(move |sc| {
            let k = sc.value() / 100.0;
            with_ed(&u, |e| {
                e.update_style("art-k", move |s| {
                    if let Some((a, _)) = s.fx.art {
                        s.fx.art = Some((a, k));
                    }
                });
            });
        });
    }
    {
        let u = ui.clone();
        c.temp.connect_value_changed(move |sc| {
            let t = (sc.value() / 100.0).clamp(-1.0, 1.0);
            with_ed(&u, |e| {
                e.update_objects("temp", is_image, move |s| s.fx.temp = t);
            });
        });
    }
    {
        let u = ui.clone();
        c.lens_fold.strength.connect_value_changed(move |sc| {
            let k = (sc.value() / 100.0).clamp(-1.0, 1.0);
            with_ed(&u, |e| {
                e.update_objects("lens-k", not_image, move |s| {
                    if let Some((l, _)) = s.fx.lens {
                        s.fx.lens = Some((l, k));
                    }
                });
            });
        });
    }
    {
        let u = ui.clone();
        c.mirror_fold.strength.connect_value_changed(move |sc| {
            let k = sc.value() / 100.0;
            with_ed(&u, |e| {
                e.update_style("mirror-k", move |s| {
                    if s.fx.mirror.is_some() {
                        s.fx.mirror = Some(k);
                    }
                });
            });
        });
    }
    {
        let u = ui.clone();
        c.grad_add.connect_clicked(move |_| {
            with_ed(&u, |e| {
                e.add_gradient();
            });
        });
    }
    {
        let u = ui.clone();
        c.grad_color.connect_rgba_notify(move |b| {
            let col = from_rgba(&b.rgba());
            with_ed(&u, |e| {
                let i = e.grad_stop;
                e.set_stop_color(i, col);
            });
        });
    }
    {
        let u = ui.clone();
        c.grad_angle.connect_value_changed(move |s| {
            let v = s.value();
            with_ed_m(&u, |e, m| {
                e.set_gradient_angle(v, m);
            });
        });
    }
    {
        let u = ui.clone();
        c.grad_kind.connect_selected_notify(move |dd| {
            let k = GradientKind::ALL
                .get(dd.selected() as usize)
                .copied()
                .unwrap_or_default();
            with_ed(&u, |e| {
                e.set_gradient_kind(k);
            });
        });
    }
    {
        let u = ui.clone();
        for (i, b) in c.join.iter().enumerate() {
            let u = u.clone();
            b.connect_toggled(move |b| {
                if !b.is_active() {
                    return;
                }
                let j = LineJoin::ALL[i];
                with_ed(&u, |e| {
                    e.update_style("join", move |st| st.join = j);
                });
            });
        }
        {
            let u = u.clone();
            c.dash.connect_selected_notify(move |dd| {
                let k =
                    crate::model::Dash::ALL[(dd.selected() as usize).min(crate::model::Dash::ALL.len() - 1)];
                with_ed(&u, |e| {
                    e.update_style("dash", move |st| st.dash = k);
                });
            });
        }
        for (i, b) in c.cap.iter().enumerate() {
            let u = u.clone();
            b.connect_toggled(move |b| {
                if !b.is_active() {
                    return;
                }
                let k = LineCap::ALL[i];
                with_ed(&u, |e| {
                    e.update_style("cap", move |st| st.cap = k);
                });
            });
        }
        c.marker.connect_selected_notify(move |dd| {
            let mk = Marker::ALL
                .get(dd.selected() as usize)
                .copied()
                .unwrap_or_default();
            with_ed(&u, |e| {
                e.update_style("marker", move |s| s.marker = mk);
            });
        });
    }
    {
        let u = ui.clone();
        c.marker_size.connect_value_changed(move |s| {
            let v = (s.value() / 100.0).max(0.1);
            with_ed(&u, |e| {
                e.update_style("marker-size", move |st| st.marker_size = v);
            });
        });
    }
    for (i, b) in c.marker_ends.iter().enumerate() {
        let u = ui.clone();
        b.connect_toggled(move |b| {
            if !b.is_active() {
                return;
            }
            let ends = MarkerEnds::ALL[i];
            with_ed(&u, |e| {
                e.update_style("marker-ends", move |s| s.marker_ends = ends);
            });
        });
    }
    // Textrahmen
    let apply_tframe_fill = {
        let u = ui.clone();
        move || {
            let v = u
                .panel
                .tframe_fill_on
                .is_active()
                .then(|| from_rgba(&u.panel.tframe_fill.rgba()));
            with_ed_m(&u, |e, m| {
                e.set_text_frame("frame-fill", move |f| f.fill = v, m);
            });
        }
    };
    let apply_tframe_stroke = {
        let u = ui.clone();
        move || {
            let v = u
                .panel
                .tframe_stroke_on
                .is_active()
                .then(|| from_rgba(&u.panel.tframe_stroke.rgba()));
            with_ed_m(&u, |e, m| {
                e.set_text_frame(
                    "frame-stroke",
                    move |f| {
                        f.stroke = v;
                        if f.width <= 0.0 {
                            f.width = 1.0;
                        }
                    },
                    m,
                );
            });
        }
    };
    {
        let f = apply_tframe_fill.clone();
        c.tframe_fill_on.connect_toggled(move |_| f());
    }
    c.tframe_fill.connect_rgba_notify(move |_| apply_tframe_fill());
    {
        let f = apply_tframe_stroke.clone();
        c.tframe_stroke_on.connect_toggled(move |_| f());
    }
    c.tframe_stroke
        .connect_rgba_notify(move |_| apply_tframe_stroke());
    {
        let u = ui.clone();
        c.tframe_width.connect_value_changed(move |s| {
            let v = s.value();
            with_ed_m(&u, |e, m| {
                e.set_text_frame("frame-width", move |f| f.width = v, m);
            });
        });
    }
    {
        let u = ui.clone();
        c.tframe_inset.connect_value_changed(move |s| {
            let v = s.value();
            with_ed_m(&u, |e, m| {
                e.set_text_frame("frame-inset", move |f| f.inset = v, m);
            });
        });
    }
    for (i, b) in c.image_modes.iter().enumerate() {
        let u = ui.clone();
        b.connect_toggled(move |b| {
            if !b.is_active() {
                return;
            }
            let part = [ImagePart::All, ImagePart::Frame, ImagePart::Content][i];
            with_ed(&u, |e| e.set_frame_mode(part));
        });
    }
    // Hilfslinien
    {
        let u = ui.clone();
        c.guide_angle.connect_value_changed(move |s| {
            let v = s.value();
            with_ed(&u, |e| {
                e.set_guide_angle(v);
            });
        });
    }
    {
        let u = ui.clone();
        c.guide_pos.connect_value_changed(move |s| {
            let v = s.value();
            let Some(d) = cur(&u) else { return };
            let k = doc_unit(&u, &d.ed.borrow()).px();
            with_ed(&u, |e| {
                e.set_guide_position(v * k);
            });
        });
    }
    for snap in [false, true] {
        let u = ui.clone();
        let sw = if snap { &c.guide_snap } else { &c.guide_show };
        sw.connect_active_notify(move |s| {
            if u.updating.get() {
                return;
            }
            let v = s.is_active();
            if snap {
                u.snap_guides.set(v);
            } else {
                u.show_guides.set(v);
            }
            for d in u.docs.borrow().iter() {
                let mut ed = d.ed.borrow_mut();
                if snap {
                    ed.snap_guides = v;
                } else {
                    ed.show_guides = v;
                    if !v {
                        ed.guide_sel = None;
                    }
                }
            }
            save_settings(&u);
            redraw_all(&u);
            refresh(&u);
        });
    }
    {
        let u = ui.clone();
        c.shape_kind.connect_selected_notify(move |dd| {
            let i = dd.selected() as usize;
            with_ed(&u, |e| {
                if let Some((k, _)) = SHAPE_KINDS.get(i) {
                    e.shape_kind = *k;
                }
            });
        });
    }
    {
        let u = ui.clone();
        c.corners.connect_value_changed(move |s| {
            let v = s.value() as u32;
            with_ed(&u, |e| {
                e.set_corners(v);
            });
        });
    }
    {
        let u = ui.clone();
        c.radius.connect_value_changed(move |s| {
            let v = s.value();
            with_ed(&u, |e| {
                e.set_corner_radius(v);
            });
        });
    }
    {
        let u = ui.clone();
        c.font.connect_selected_notify(move |dd| {
            if let Some(fam) = font_selected(dd) {
                with_ed(&u, |e| {
                    e.set_char_fmt("family", move |t| t.family = fam.clone());
                });
            }
        });
    }
    {
        let u = ui.clone();
        c.underline.connect_toggled(move |b| {
            let v = b.is_active();
            with_ed(&u, |e| {
                e.set_char_fmt("underline", move |t| t.underline = v);
            });
        });
    }
    {
        let u = ui.clone();
        c.strike.connect_toggled(move |b| {
            let v = b.is_active();
            with_ed(&u, |e| {
                e.set_char_fmt("strike", move |t| t.strike = v);
            });
        });
    }
    {
        let u = ui.clone();
        c.list_on.connect_toggled(move |b| {
            let v = if b.is_active() {
                u.panel.list_style.get()
            } else {
                ListStyle::None
            };
            with_ed(&u, |e| {
                e.set_para_fmt("list", move |p| p.list = v);
            });
        });
    }
    {
        let u = ui.clone();
        c.indent_less.connect_clicked(move |_| {
            with_ed(&u, |e| {
                e.set_para_fmt("indent", |p| p.indent = p.indent.saturating_sub(1));
            });
        });
    }
    {
        let u = ui.clone();
        c.indent_more.connect_clicked(move |_| {
            with_ed(&u, |e| {
                e.set_para_fmt("indent", |p| {
                    p.indent = (p.indent + 1).min(crate::model::MAX_INDENT)
                });
            });
        });
    }
    {
        let u = ui.clone();
        c.para_spacing.connect_value_changed(move |s| {
            let v = s.value() / 100.0;
            with_ed(&u, |e| {
                e.update_text("para", move |t| t.fmt.para = v);
            });
        });
    }
    {
        let u = ui.clone();
        c.columns.connect_value_changed(move |s| {
            let v = s.value().round().clamp(1.0, crate::model::MAX_COLUMNS as f64) as u32;
            with_ed(&u, |e| {
                e.update_text("columns", move |t| t.fmt.columns = v);
            });
        });
    }
    {
        let u = ui.clone();
        c.small_caps.connect_toggled(move |b| {
            let v = b.is_active();
            with_ed(&u, |e| {
                e.update_text("small-caps", move |t| t.fmt.small_caps = v);
            });
        });
    }
    {
        let u = ui.clone();
        c.ligatures.connect_toggled(move |b| {
            let v = !b.is_active();
            with_ed(&u, |e| {
                e.update_text("ligatures", move |t| t.fmt.no_ligatures = v);
            });
        });
    }
    {
        let u = ui.clone();
        c.path_below.connect_toggled(move |b| {
            let v = b.is_active();
            with_ed(&u, |e| {
                e.update_text("path-below", move |t| t.fmt.path_below = v);
            });
        });
    }
    {
        let u = ui.clone();
        c.path_reverse.connect_toggled(move |b| {
            let v = b.is_active();
            with_ed(&u, |e| {
                e.update_text("path-reverse", move |t| t.fmt.path_reverse = v);
            });
        });
    }
    {
        let u = ui.clone();
        c.size.connect_value_changed(move |s| {
            let v = s.value();
            with_ed(&u, |e| {
                e.set_char_fmt("size", move |t| t.size = v);
            });
        });
    }
    {
        let u = ui.clone();
        c.face.connect_selected_notify(move |dd| {
            let f = { u.panel.faces.borrow().get(dd.selected() as usize).cloned() };
            let Some(f) = f else { return };
            with_ed(&u, |e| {
                e.set_char_fmt("face", move |t| {
                    t.weight = f.weight;
                    t.italic = f.italic;
                });
            });
        });
    }
    {
        let u = ui.clone();
        c.weight.connect_value_changed(move |s| {
            let w = s.value().round() as u16;
            with_ed(&u, |e| {
                e.set_char_fmt("weight", move |t| t.weight = w);
            });
        });
    }
    {
        let u = ui.clone();
        c.text.buffer().connect_changed(move |buf| {
            let (s, e) = buf.bounds();
            let t = buf.text(&s, &e, false).to_string();
            with_ed(&u, |ed| {
                ed.set_text(&t);
            });
        });
    }

    {
        let u = ui.clone();
        c.offset.connect_value_changed(move |s| {
            let v = s.value();
            with_ed(&u, |e| {
                e.set_text_offset(v);
            });
        });
    }
    {
        let u = ui.clone();
        c.node_radius.connect_value_changed(move |s| {
            let v = s.value();
            with_ed(&u, |e| {
                e.set_node_radius(v);
            });
        });
    }
    for (stroke, scales) in [(false, &c.fill_cmyk_scales), (true, &c.stroke_cmyk_scales)] {
        for sc in scales.iter() {
            let u = ui.clone();
            sc.connect_value_changed(move |_| {
                let s = if stroke {
                    &u.panel.stroke_cmyk_scales
                } else {
                    &u.panel.fill_cmyk_scales
                };
                let v: Vec<f64> = s.iter().map(|x| x.value() / 100.0).collect();
                let cur_alpha = if stroke {
                    u.panel.stroke_sw.current.get()
                } else {
                    u.panel.fill_sw.current.get()
                }
                .map(|c| c.a)
                .unwrap_or(1.0);
                let col = Color::from_cmyk(v[0], v[1], v[2], v[3], cur_alpha);
                with_ed(&u, |e| {
                    if stroke {
                        e.update_style("stroke", move |st| st.stroke = Some(col));
                    } else {
                        e.update_style("fill", move |st| st.fill = Some(col));
                    }
                });
            });
        }
    }

    // Farbwahl: Füllung, Kontur und Schatten bearbeiten den Editor selbst
    for sw in [
        &c.fill_sw,
        &c.stroke_sw,
        &c.shadow_sw,
        &c.frame_fill_sw,
        &c.frame_stroke_sw,
        &c.image_sw,
    ] {
        *sw.ui.borrow_mut() = Rc::downgrade(ui);
    }
    // Lage der Kontur
    for (i, b) in c.stroke_align.iter().enumerate() {
        let u = ui.clone();
        b.connect_toggled(move |b| {
            if !b.is_active() {
                return;
            }
            let a = crate::model::StrokeAlign::ALL[i];
            with_ed(&u, |e| {
                e.update_style("stroke-align", move |s| s.stroke_align = a);
            });
        });
    }
    edit_action(ui, "outline-stroke", |e, m| {
        e.outline_stroke(m);
    });
    // Effekte: je Kategorie einer; Schatten und Spiegelung schließen sich aus
    use crate::model::{Art, Lens, Texture, FX_DEFAULT};
    edit_action(ui, "effect-shadow", |e, _| {
        e.update_style("shadow", |s| {
            if s.shadow.is_none() {
                s.shadow = Some(crate::model::Shadow::default());
            }
            s.fx.mirror = None;
        });
    });
    edit_action(ui, "effect-remove-shadow", |e, _| {
        e.update_style("shadow", |s| {
            s.shadow = None;
            s.shadow_gradient = None;
        });
    });
    edit_action(ui, "effect-mirror", |e, _| {
        e.update_style("mirror", |s| {
            if s.fx.mirror.is_none() {
                s.fx.mirror = Some(FX_DEFAULT);
            }
            s.shadow = None;
            s.shadow_gradient = None;
        });
    });
    edit_action(ui, "effect-remove-mirror", |e, _| {
        e.update_style("mirror", |s| s.fx.mirror = None);
    });
    str_action(ui, "effect-tex", |e, n| {
        if let Some(t) = Texture::from_name(n) {
            e.update_style("tex", move |s| {
                // wählt man dieselbe Textur erneut, bleibt die Stärke
                let k = match s.fx.tex {
                    Some((old, k)) if old == t => k,
                    _ => t.default_strength(),
                };
                s.fx.tex = Some((t, k));
            });
        }
    });
    edit_action(ui, "effect-remove-tex", |e, _| {
        e.update_style("tex", |s| s.fx.tex = None);
    });
    edit_action(ui, "effect-remove-blend", |e, _| {
        e.set_blend(Blend::Normal);
    });
    edit_action(ui, "effect-remove-art", |e, _| {
        e.update_style("art", |s| s.fx.art = None);
    });
    edit_action(ui, "effect-remove-lens", |e, _| {
        e.update_style("lens", |s| s.fx.lens = None);
    });
    edit_action(ui, "effect-remove-tint", |e, _| {
        e.update_objects("tint", is_image, |s| s.fx.tint = None);
    });
    str_action(ui, "effect-tint", |e, n| {
        if let Some(f) = crate::model::ImgFilter::from_name(n) {
            e.update_objects("tint", is_image, move |s| {
                let k = match s.fx.tint {
                    Some((old, k)) if old == f => k,
                    _ => f.default_strength(),
                };
                s.fx.tint = Some((f, k));
            });
        }
    });
    str_action(ui, "effect-blend", |e, n| {
        if let Some(b) = Blend::from_css(n) {
            e.set_blend(b);
        }
    });
    str_action(ui, "effect-art", |e, n| {
        if let Some(a) = Art::from_name(n) {
            e.update_style("art", move |s| {
                let k = s.fx.art.map(|x| x.1).unwrap_or(FX_DEFAULT);
                s.fx.art = Some((a, k));
            });
        }
    });
    str_action(ui, "effect-lens", |e, n| {
        if let Some(l) = Lens::from_name(n) {
            e.update_objects("lens", not_image, move |s| {
                let k = s.fx.lens.map(|x| x.1).unwrap_or(FX_DEFAULT);
                s.fx.lens = Some((l, k));
            });
        }
    });
    {
        // Aufzählungszeichen wählen (kleiner Pfeil neben dem Aufzählungsknopf)
        let a = gio::SimpleAction::new("text-list", Some(glib::VariantTy::STRING));
        let u = ui.clone();
        a.connect_activate(move |_, v| {
            let Some(ls) = v
                .and_then(|v| v.get::<String>())
                .and_then(|n| ListStyle::from_name(&n))
            else {
                return;
            };
            u.panel.list_style.set(ls);
            with_ed(&u, |e| {
                e.set_para_fmt("list", move |p| p.list = ls);
            });
        });
        ui.window.add_action(&a);
    }
    {
        let apply_shadow = {
            let u = ui.clone();
            Rc::new(move || {
                let p = &u.panel;
                let v = p.shadow_sw.current.get().map(|color| crate::model::Shadow {
                    color,
                    opacity: p.shadow_opacity.value() / 100.0,
                    blur: p.shadow_blur.value(),
                    dx: p.shadow_dx.value(),
                    dy: p.shadow_dy.value(),
                });
                with_ed(&u, |e| {
                    e.update_style("shadow", move |s| s.shadow = v);
                });
            })
        };
        for sp in [&c.shadow_opacity, &c.shadow_blur, &c.shadow_dx, &c.shadow_dy] {
            let f = apply_shadow.clone();
            sp.connect_value_changed(move |_| f());
        }
    }
    {
        let u = ui.clone();
        c.width.connect_value_changed(move |s| {
            let v = s.value();
            with_ed(&u, |e| {
                e.update_style("width", move |st| st.width = v);
            });
        });
    }
    {
        let u = ui.clone();
        c.opacity.connect_value_changed(move |s| {
            let v = (s.value() / 100.0).clamp(0.0, 1.0);
            with_ed(&u, |e| {
                e.update_style("opacity", move |st| st.opacity = v);
            });
        });
    }
}

// ───────────────────────────── Ebenen ─────────────────────────────

fn setup_layers(ui: &Rc<Ui>) {
    let u = ui.clone();
    ui.layers_list.connect_row_selected(move |_, row| {
        if u.updating.get() {
            return;
        }
        let Some(row) = row else { return };
        let id = u
            .layer_rows
            .borrow()
            .iter()
            .find(|(_, r)| r == row)
            .map(|(id, _)| *id);
        let (Some(id), Some(d)) = (id, cur(&u)) else {
            return;
        };
        d.ed.borrow_mut().active_layer = id;
        u.layers_shown_active.set(id);
        refresh(&u);
    });
    // Ablegen auf der ganzen Liste: Ebenen umsortieren (Strich an der Einfügestelle, die
    // übrigen Zeilen rücken schon beim Ziehen auseinander) oder Auswahl in eine Ebene legen
    let target = gtk::DropTarget::new(glib::Type::INVALID, gdk::DragAction::MOVE);
    target.set_types(&[glib::Type::U64, glib::Type::STRING]);
    {
        let u = ui.clone();
        target.connect_motion(move |_, _, y| {
            update_layer_mark(&u, y);
            gdk::DragAction::MOVE
        });
    }
    {
        let u = ui.clone();
        target.connect_leave(move |_| clear_layer_mark(&u));
    }
    {
        let u = ui.clone();
        target.connect_drop(move |_, value, _, y| {
            update_layer_mark(&u, y);
            let mark = u.layer_mark.borrow().clone();
            clear_layer_mark(&u);
            let Some(mark) = mark else { return false };
            let u = u.clone();
            if let Ok(src_id) = value.get::<u64>() {
                let Some(slot) = mark.slot else { return false };
                glib::idle_add_local_once(move || {
                    if let Some(d) = cur(&u) {
                        d.ed.borrow_mut().move_layer_to_slot(src_id, slot);
                        refresh(&u);
                    }
                });
                return true;
            }
            if value
                .get::<String>()
                .map(|s| s == "mondrian-selection")
                .unwrap_or(false)
            {
                let target_id = mark.layer;
                glib::idle_add_local_once(move || {
                    if let Some(d) = cur(&u) {
                        d.ed.borrow_mut().move_selection_to_layer(target_id);
                        refresh(&u);
                    }
                });
                return true;
            }
            false
        });
    }
    ui.layers_list.add_controller(target);
}

/// Markierung beim Ziehen über die Ebenenliste.
#[derive(Clone)]
struct LayerMark {
    row: gtk::ListBoxRow,
    class: &'static str,
    /// Ebene unter dem Zeiger
    layer: u64,
    /// Einfügeplatz (0 = ganz oben) beim Umsortieren
    slot: Option<usize>,
}

fn clear_layer_mark(ui: &Ui) {
    if let Some(m) = ui.layer_mark.borrow_mut().take() {
        m.row.remove_css_class(m.class);
    }
    set_sel_target(ui, 0);
}

/// Zielebene beim Ziehen der Auswahl: dort erscheint ein leeres Kästchen.
fn set_sel_target(ui: &Ui, id: u64) {
    if ui.sel_target.replace(id) != id {
        for t in ui.thumbs.borrow().iter() {
            t.queue_draw();
        }
    }
}

fn update_layer_mark(ui: &Ui, y: f64) {
    // Zeile unter dem Zeiger; in einer Lücke bleibt die bisherige Markierung stehen
    let Some(row) = ui.layers_list.row_at_y(y as i32) else {
        return;
    };
    let rows: Vec<(u64, gtk::ListBoxRow)> = ui.layer_rows.borrow().clone();
    let Some(j) = rows.iter().position(|(_, r)| *r == row) else {
        return;
    };
    let layer = rows[j].0;
    let new = if ui.layer_drag.get() != 0 {
        let top = row
            .compute_point(&ui.layers_list, &gtk::graphene::Point::new(0.0, 0.0))
            .map(|p| p.y() as f64)
            .unwrap_or(0.0);
        let upper = y - top < row.height() as f64 / 2.0;
        let slot = if upper { j } else { j + 1 };
        // Strich und Lücke über der Zeile am Einfügeplatz, hinter der letzten darunter
        let (row, class) = if slot < rows.len() {
            (rows[slot].1.clone(), "mondrian-drop-above")
        } else {
            (rows[rows.len() - 1].1.clone(), "mondrian-drop-below")
        };
        LayerMark {
            row,
            class,
            layer,
            slot: Some(slot),
        }
    } else {
        set_sel_target(ui, layer);
        LayerMark {
            row,
            class: "mondrian-drop-into",
            layer,
            slot: None,
        }
    };
    let same = ui
        .layer_mark
        .borrow()
        .as_ref()
        .map(|m| m.row == new.row && m.class == new.class)
        .unwrap_or(false);
    if same {
        if let Some(m) = ui.layer_mark.borrow_mut().as_mut() {
            m.slot = new.slot;
            m.layer = new.layer;
        }
        return;
    }
    clear_layer_mark(ui);
    new.row.add_css_class(new.class);
    *ui.layer_mark.borrow_mut() = Some(new);
}

fn rebuild_layers(ui: &Rc<Ui>) {
    let Some(d) = cur(ui) else { return };
    let (rows, active): (Vec<(u64, String, bool, bool)>, u64) = {
        let ed = d.ed.borrow();
        (
            ed.doc
                .layers
                .iter()
                .rev()
                .map(|l| (l.id, l.name.clone(), l.visible, l.locked))
                .collect(),
            ed.active_layer,
        )
    };
    guarded(ui, || {
        ui.layers_list.remove_all();
        ui.thumbs.borrow_mut().clear();
        ui.thumb_frame.set(None);
        ui.layer_rows.borrow_mut().clear();
        let single = rows.len() <= 1;
        for (id, name, visible, locked) in &rows {
            let row = layer_row(ui, *id, name, *visible, *locked, single);
            ui.layers_list.append(&row);
            ui.layer_rows.borrow_mut().push((*id, row.clone()));
            if *id == active {
                ui.layers_list.select_row(Some(&row));
            }
        }
        let n_boards = cur(ui)
            .map(|d| d.ed.borrow().doc.artboards.len() + 1)
            .unwrap_or(1);
        ui.artboards_list.remove_all();
        for i in 0..n_boards {
            ui.artboards_list.append(&artboard_row(ui, i));
        }
    });
    ui.layers_shown_active.set(active);
}

/// Ausschnitt der Ebenen-Vorschaubilder: alle Zeichenflächen und aller Inhalt (auch
/// außerhalb), für alle Ebenen gleich. Wird einmal je Aktualisierung berechnet.
fn layer_thumb_frame(ui: &Ui, doc: &crate::model::Document) -> crate::geom::Rect {
    if let Some(r) = ui.thumb_frame.get() {
        return r;
    }
    let mut r = doc.artboards_bbox();
    for o in doc.layers.iter().flat_map(|l| l.objects.iter()) {
        if let Some(b) = crate::editor::Metrics::bbox(&ui.metrics, o) {
            r = r.union(&b);
        }
    }
    ui.thumb_frame.set(Some(r));
    r
}

/// Größe der Vorschaubilder in der Ebenenliste
const THUMB_W: i32 = 120;
const THUMB_H: i32 = 84;
/// Vorschaubilder der Zeichenflächen
const BOARD_W: i32 = 56;
const BOARD_H: i32 = 44;

/// Auswahl-Kästchen unten rechts im Vorschaubild (wie ein Anfasser in der Ebenenfarbe).
fn sel_badge(cr: &gtk::cairo::Context, w: f64, h: f64, col: Color) {
    let size = 11.0;
    let (x, y) = (w - size - 7.0, h - size - 7.0);
    // weicher Schatten, weißer Rand, Füllung in der Ebenenfarbe
    round_rect(cr, x - 1.0, y, size + 2.0, size + 2.5, 3.5);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.18);
    let _ = cr.fill();
    round_rect(cr, x - 1.5, y - 1.5, size + 3.0, size + 3.0, 4.0);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    let _ = cr.fill();
    round_rect(cr, x, y, size, size, 2.5);
    cr.set_source_rgb(col.r, col.g, col.b);
    let _ = cr.fill();
}

/// Leeres Kästchen: Ziel beim Verschieben der Auswahl auf diese Ebene.
fn sel_badge_outline(cr: &gtk::cairo::Context, w: f64, h: f64, col: Color) {
    let size = 11.0;
    let (x, y) = (w - size - 7.0, h - size - 7.0);
    round_rect(cr, x - 0.5, y - 0.5, size + 1.0, size + 1.0, 3.0);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.9);
    cr.set_line_width(4.0);
    let _ = cr.stroke_preserve();
    cr.set_source_rgb(col.r, col.g, col.b);
    cr.set_line_width(2.0);
    let _ = cr.stroke();
}

/// Liegt (x, y) auf dem Auswahl-Kästchen des Vorschaubilds?
fn near_badge(th: &gtk::DrawingArea, x: f64, y: f64) -> bool {
    let (w, h) = (th.width() as f64, th.height() as f64);
    let (cx, cy) = (w - 7.0 - 5.5, h - 7.0 - 5.5);
    (x - cx).abs() <= 11.0 && (y - cy).abs() <= 11.0
}

/// Knöpfe einer Ebene untereinander: Sperren, Ein-/Ausblenden, Löschen – schlicht und rund
/// wie die übrigen kleinen Knöpfe der Leisten.
fn layer_pill(
    locked: bool,
    visible: bool,
    single: bool,
) -> (gtk::Box, gtk::ToggleButton, gtk::ToggleButton, gtk::Button) {
    let small = |b: &gtk::Widget| {
        b.add_css_class("flat");
        b.add_css_class("circular");
        b.add_css_class("mondrian-layer-btn");
    };
    let lock = gtk::ToggleButton::builder()
        .icon_name(if locked {
            "changes-prevent-symbolic"
        } else {
            "changes-allow-symbolic"
        })
        .active(locked)
        .tooltip_text(tr("Sperren"))
        .build();
    small(lock.upcast_ref());
    lock.connect_toggled(|b| {
        b.set_icon_name(if b.is_active() {
            "changes-prevent-symbolic"
        } else {
            "changes-allow-symbolic"
        });
    });
    let eye = gtk::ToggleButton::builder()
        .icon_name(if visible {
            "view-reveal-symbolic"
        } else {
            "view-conceal-symbolic"
        })
        .tooltip_text(tr("Ein-/Ausblenden"))
        .build();
    small(eye.upcast_ref());
    // Auge: aktiv = sichtbar; hervorgehoben wird nur der Ausnahmezustand „ausgeblendet“
    eye.set_active(visible);
    if !visible {
        eye.add_css_class("mondrian-layer-off");
    }
    eye.connect_toggled(|b| {
        b.set_icon_name(if b.is_active() {
            "view-reveal-symbolic"
        } else {
            "view-conceal-symbolic"
        });
        if b.is_active() {
            b.remove_css_class("mondrian-layer-off");
        } else {
            b.add_css_class("mondrian-layer-off");
        }
    });
    let del = gtk::Button::from_icon_name("user-trash-symbolic");
    del.set_tooltip_text(Some(tr("Ebene löschen")));
    small(del.upcast_ref());
    del.add_css_class("mondrian-layer-trash");
    del.set_sensitive(!single);
    let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
    col.set_valign(gtk::Align::Center);
    col.append(&lock);
    col.append(&eye);
    col.append(&del);
    (col, lock, eye, del)
}

/// Letzte Zeile der Ebenenliste: die Zeichenfläche mit Zahnrad zum Bearbeiten.
/// Zeile einer Zeichenfläche (unter den Ebenen) mit Zahnrad und – bei weiteren
/// Zeichenflächen – Löschen.
fn artboard_row(ui: &Rc<Ui>, idx: usize) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);
    let bx = hbox(8);
    let thumb = gtk::DrawingArea::new();
    thumb.set_content_width(BOARD_W);
    thumb.set_content_height(BOARD_H);
    thumb.set_valign(gtk::Align::Center);
    {
        let wu = Rc::downgrade(ui);
        thumb.set_draw_func(move |_, cr, w, h| {
            let Some(u) = wu.upgrade() else { return };
            let Some(d) = cur(&u) else { return };
            let dark = adw::StyleManager::default().is_dark();
            let doc = d.ed.borrow().doc.artboard_doc(idx);
            render::draw_artboard_thumbnail(cr, w as f64, h as f64, &doc, dark);
        });
    }
    {
        // Notizheft: Klick auf das Vorschaubild blättert zu dieser Seite
        let wu = Rc::downgrade(ui);
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            let Some(u) = wu.upgrade() else { return };
            if u.notebook.get() {
                // die Aktion des Fensters (nicht die gleichnamige Methode der Aktionsgruppe)
                let _ =
                    WidgetExt::activate_action(&u.window, "win.page-go", Some(&(idx as u32).to_variant()));
            }
        });
        thumb.add_controller(click);
    }
    ui.thumbs.borrow_mut().push(thumb.clone());
    let (name, many) = cur(ui)
        .map(|d| {
            let ed = d.ed.borrow();
            let rects = ed.doc.artboard_rects();
            (
                rects.get(idx).map(|r| r.0.clone()).unwrap_or_default(),
                rects.len() > 1,
            )
        })
        .unwrap_or_else(|| (tr("Zeichenfläche").into(), false));
    let label = gtk::Label::builder()
        .label(if many { name.as_str() } else { tr("Zeichenfläche") })
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(pango::EllipsizeMode::End)
        .build();
    let tools = hbox(0);
    tools.add_css_class("linked");
    tools.set_valign(gtk::Align::Center);
    let gear = gtk::Button::from_icon_name("emblem-system-symbolic");
    gear.set_tooltip_text(Some(tr("Zeichenfläche bearbeiten")));
    gear.set_action_name(Some("win.artboard-edit"));
    gear.set_action_target_value(Some(&(idx as u32).to_variant()));
    tools.append(&gear);
    let dup = gtk::Button::from_icon_name("edit-copy-symbolic");
    dup.set_tooltip_text(Some(tr("Zeichenfläche mit Inhalt duplizieren")));
    dup.set_action_name(Some("win.artboard-duplicate"));
    dup.set_action_target_value(Some(&(idx as u32).to_variant()));
    tools.append(&dup);
    // jede Zeichenfläche lässt sich entfernen, solange eine weitere bleibt
    if many {
        let del = gtk::Button::from_icon_name("user-trash-symbolic");
        del.set_tooltip_text(Some(tr("Zeichenfläche entfernen (die Objekte bleiben)")));
        del.add_css_class("mondrian-layer-delete");
        del.set_action_name(Some("win.artboard-delete"));
        del.set_action_target_value(Some(&(idx as u32).to_variant()));
        tools.append(&del);
    }
    bx.append(&thumb);
    bx.append(&label);
    bx.append(&tools);
    row.set_child(Some(&bx));
    label_icon_buttons(row.upcast_ref());
    row
}

fn layer_row(ui: &Rc<Ui>, id: u64, name: &str, visible: bool, locked: bool, single: bool) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_tooltip_text(Some(name));
    let bx = hbox(10);

    // Farbe der Ebene als Strich links; liegt hier etwas Ausgewähltes, steht unten rechts im
    // Vorschaubild ein Punkt – ihn auf eine andere Ebene ziehen verschiebt die Auswahl.
    let (color, has_sel) = cur(ui)
        .map(|d| {
            let ed = d.ed.borrow();
            let c = ed.doc.layer(id).map(|l| l.color).unwrap_or(0);
            (c, ed.selection_layers().contains(&id))
        })
        .unwrap_or((0, false));
    let col = crate::model::layer_color(color);

    let bar = gtk::DrawingArea::new();
    bar.set_content_width(6);
    bar.set_content_height(THUMB_H * 2 / 3);
    bar.set_valign(gtk::Align::Center);
    bar.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (w as f64, h as f64);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.set_line_width(w);
        cr.move_to(w / 2.0, w / 2.0);
        cr.line_to(w / 2.0, h - w / 2.0);
        cr.set_source_rgb(col.r, col.g, col.b);
        let _ = cr.stroke();
    });

    // großes Vorschaubild der Ebene: nur ihre Objekte auf einer dezenten Karte
    let thumb = gtk::DrawingArea::new();
    thumb.set_content_width(THUMB_W);
    thumb.set_content_height(THUMB_H);
    thumb.set_hexpand(true);
    thumb.set_valign(gtk::Align::Center);
    {
        let wu = Rc::downgrade(ui);
        thumb.set_draw_func(move |_, cr, w, h| {
            let (w, h) = (w as f64, h as f64);
            let Some(u) = wu.upgrade() else { return };
            let Some(d) = cur(&u) else { return };
            let ed = d.ed.borrow();
            let Some(layer) = ed.doc.layer(id) else { return };
            let dark = adw::StyleManager::default().is_dark();
            round_rect(cr, 0.5, 0.5, w - 1.0, h - 1.0, 8.0);
            if dark {
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.06);
            } else {
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.045);
            }
            let _ = cr.fill();
            let _ = cr.save();
            round_rect(cr, 0.0, 0.0, w, h, 8.0);
            cr.clip();
            let boards: Vec<crate::geom::Rect> =
                ed.doc.artboard_rects().into_iter().map(|(_, r)| r).collect();
            let frame = layer_thumb_frame(&u, &ed.doc);
            render::draw_layer_objects(cr, w, h, frame, &boards, dark, layer);
            let _ = cr.restore();
            if has_sel {
                sel_badge(cr, w, h, col);
            } else if u.sel_target.get() == id {
                // hierhin würde die Auswahl verschoben
                sel_badge_outline(cr, w, h, col);
            }
        });
    }
    ui.thumbs.borrow_mut().push(thumb.clone());
    if has_sel {
        thumb.set_tooltip_text(Some(&trf(
            "{} – die Auswahl liegt auf dieser Ebene; das Kästchen auf eine andere Ebene ziehen, um sie zu verschieben",
            &[&name],
        )));
        let sel_src = gtk::DragSource::new();
        sel_src.set_actions(gdk::DragAction::MOVE);
        {
            let th = thumb.clone();
            sel_src.connect_prepare(move |_, x, y| {
                if !near_badge(&th, x, y) {
                    return None;
                }
                Some(gdk::ContentProvider::for_value(&"mondrian-selection".to_value()))
            });
        }
        {
            // Am Zeiger hängt nur das kleine Kästchen
            let u = ui.clone();
            sel_src.connect_drag_begin(move |_, drag| {
                u.layer_drag.set(0);
                let icon = gtk::DragIcon::for_drag(drag);
                let da = gtk::DrawingArea::new();
                da.set_content_width(22);
                da.set_content_height(22);
                da.set_draw_func(move |_, cr, w, h| sel_badge(cr, w as f64 + 3.0, h as f64 + 3.0, col));
                icon.set_child(Some(&da));
            });
        }
        {
            let u = ui.clone();
            sel_src.connect_drag_end(move |_, _, _| clear_layer_mark(&u));
        }
        thumb.add_controller(sel_src);
        let motion = gtk::EventControllerMotion::new();
        {
            let th = thumb.clone();
            motion.connect_motion(move |_, x, y| {
                let near = near_badge(&th, x, y);
                th.set_cursor_from_name(Some(if near { "grab" } else { "default" }));
            });
        }
        thumb.add_controller(motion);
    } else {
        thumb.set_tooltip_text(Some(&trf("{} – Doppelklick zum Umbenennen", &[&(name)])));
    }
    // Doppelklick: Namen in einem kleinen Fenster ändern
    {
        let u = ui.clone();
        let th = thumb.clone();
        let name = name.to_string();
        let dc = gtk::GestureClick::new();
        dc.connect_pressed(move |_, n, _, _| {
            if n != 2 {
                return;
            }
            let entry = gtk::Entry::builder().text(&name).width_chars(16).build();
            let pop = gtk::Popover::new();
            pop.set_child(Some(&entry));
            pop.set_parent(&th);
            {
                let (u, pop) = (u.clone(), pop.clone());
                entry.connect_activate(move |e| {
                    let text = e.text().to_string();
                    if !text.trim().is_empty() {
                        with_ed(&u, |ed| ed.rename_layer(id, text.trim()));
                    }
                    pop.popdown();
                });
            }
            pop.connect_closed(|p| {
                let p = p.clone();
                glib::idle_add_local_once(move || p.unparent());
            });
            pop.popup();
            entry.grab_focus();
            entry.select_region(0, -1);
        });
        thumb.add_controller(dc);
    }

    let (pill, lock, eye, del) = layer_pill(locked, visible, single);
    {
        let u = ui.clone();
        eye.connect_toggled(move |b| {
            let v = b.is_active();
            with_ed(&u, |e| e.set_layer_visible(id, v));
        });
    }
    {
        let u = ui.clone();
        lock.connect_toggled(move |b| {
            let v = b.is_active();
            with_ed(&u, |e| e.set_layer_locked(id, v));
        });
    }
    {
        let u = ui.clone();
        del.connect_clicked(move |_| {
            with_ed(&u, |e| {
                e.active_layer = id;
                e.delete_layer();
            });
        });
    }

    bx.append(&bar);
    bx.append(&thumb);
    bx.append(&pill);
    bx.set_margin_top(4);
    bx.set_margin_bottom(4);
    row.set_child(Some(&bx));

    // Ziehen und Ablegen zum Umsortieren (Ablegen: siehe setup_layers)
    row.add_css_class("mondrian-layer-row");
    let src = gtk::DragSource::new();
    src.set_actions(gdk::DragAction::MOVE);
    {
        let (r, th) = (row.clone(), thumb.clone());
        src.connect_prepare(move |_, x, y| {
            // Startet das Ziehen am Auswahl-Kästchen, wird die Auswahl gezogen, nicht die Ebene
            if has_sel {
                let p = r.compute_point(&th, &gtk::graphene::Point::new(x as f32, y as f32));
                if p.map(|p| near_badge(&th, p.x() as f64, p.y() as f64))
                    .unwrap_or(false)
                {
                    return None;
                }
            }
            Some(gdk::ContentProvider::for_value(&id.to_value()))
        });
    }
    {
        let (u, r) = (ui.clone(), row.clone());
        src.connect_drag_begin(move |_, drag| {
            u.layer_drag.set(id);
            // Abbild der Zeile am Zeiger als eigene kleine Karte (rundum abgerundet); die
            // Zeile selbst wird blass
            let img = gtk::WidgetPaintable::new(Some(&r)).current_image();
            let pic = gtk::Picture::for_paintable(&img);
            pic.set_can_shrink(false);
            let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
            card.add_css_class("mondrian-drag-card");
            card.set_overflow(gtk::Overflow::Hidden);
            card.append(&pic);
            // Rand, damit der Schatten der Karte Platz hat
            card.set_margin_start(12);
            card.set_margin_end(12);
            card.set_margin_top(12);
            card.set_margin_bottom(12);
            let icon = gtk::DragIcon::for_drag(drag);
            icon.set_child(Some(&card));
            drag.set_hotspot(36, 28);
            r.add_css_class("mondrian-dragging");
        });
    }
    {
        let (u, r) = (ui.clone(), row.clone());
        src.connect_drag_end(move |_, _, _| {
            u.layer_drag.set(0);
            r.remove_css_class("mondrian-dragging");
            clear_layer_mark(&u);
        });
    }
    row.add_controller(src);
    label_icon_buttons(row.upcast_ref());
    row
}

// ───────────────────────────── Fenster & Dateien ─────────────────────────────

fn setup_window(ui: &Rc<Ui>) {
    let keys = gtk::EventControllerKey::new();
    {
        let u = ui.clone();
        keys.connect_key_pressed(move |_, key, _, st| handle_key(&u, key, st));
    }
    {
        let u = ui.clone();
        keys.connect_key_released(move |_, key, _, _| {
            if key == gdk::Key::space {
                if let Some(d) = cur(&u) {
                    d.ed.borrow_mut().space_down = false;
                    let (x, y) = u.pointer.get();
                    let cursor = d.ed.borrow().cursor_at(Pt::new(x, y), &u.metrics);
                    d.canvas.set_cursor_from_name(Some(cursor));
                }
            }
        });
    }
    ui.window.add_controller(keys);

    // Beim Schließen nacheinander für jedes geänderte Dokument nachfragen.
    let u = ui.clone();
    ui.window.connect_close_request(move |_| {
        if u.closing.get() {
            drop_all_recovery(&u);
            return glib::Propagation::Proceed;
        }
        let dirty = u.docs.borrow().iter().find(|d| d.ed.borrow().dirty).cloned();
        match dirty {
            None => {
                u.closing.set(true);
                drop_all_recovery(&u);
                glib::Propagation::Proceed
            }
            Some(d) => {
                if let Some(p) = d.page.borrow().clone() {
                    u.tab_view.set_selected_page(&p);
                }
                let d2 = d.clone();
                confirm_discard(
                    &u,
                    &d,
                    Box::new(move |u: &Rc<Ui>| {
                        d2.ed.borrow_mut().dirty = false;
                        u.window.close();
                    }),
                    None,
                );
                glib::Propagation::Stop
            }
        }
    });
}

/// Bei ungespeicherten Änderungen nachfragen, dann `then` ausführen (`cancel` bei Abbruch).
fn confirm_discard(ui: &Rc<Ui>, d: &Rc<DocTab>, then: Then, cancel: Option<Then>) {
    if !d.ed.borrow().dirty {
        then(ui);
        return;
    }
    let dialog = adw::AlertDialog::new(
        Some(&trf("Änderungen an „{}“ speichern?", &[&(d.name())])),
        Some(tr("Wenn du nicht speicherst, gehen die Änderungen verloren")),
    );
    dialog.add_response("cancel", tr("_Abbrechen"));
    dialog.add_response("discard", tr("_Verwerfen"));
    dialog.add_response("save", tr("_Speichern"));
    dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");
    let then = Rc::new(RefCell::new(Some(then)));
    let cancel = Rc::new(RefCell::new(cancel));
    let (u, d) = (ui.clone(), d.clone());
    dialog.connect_response(None, move |_, resp| {
        let Some(f) = then.borrow_mut().take() else { return };
        let c = cancel.borrow_mut().take();
        match resp {
            "discard" => f(&u),
            "save" => save(
                &u,
                &d,
                Box::new(move |u: &Rc<Ui>, ok: bool| {
                    if ok {
                        f(u)
                    } else if let Some(c) = c {
                        c(u)
                    }
                }),
            ),
            _ => {
                if let Some(c) = c {
                    c(&u)
                }
            }
        }
    });
    label_icon_buttons(dialog.upcast_ref());
    dialog.present(Some(&ui.window));
}

// ───────────────────────────── Wiederherstellung ─────────────────────────────

thread_local! {
    static RECOVERY_SEQ: Cell<u64> = const { Cell::new(0) };
    /// Beim Start schon nach Dateien eines abgestürzten Laufs gesucht?
    static RECOVERY_CHECKED: Cell<bool> = const { Cell::new(false) };
    /// alle Fenster (für die Notsicherung bei einem Programmfehler)
    static ALL_UIS: RefCell<Vec<std::rc::Weak<Ui>>> = const { RefCell::new(Vec::new()) };
}

/// Zuletzt benutztes noch offenes Fenster (für Dateien, die vom Desktop aus geöffnet werden).
pub fn existing_ui() -> Option<Rc<Ui>> {
    ALL_UIS.with(|l| {
        let mut l = l.borrow_mut();
        l.retain(|w| w.upgrade().is_some_and(|u| !u.closing.get()));
        // das aktive Fenster bevorzugen
        let all: Vec<Rc<Ui>> = l.iter().filter_map(|w| w.upgrade()).collect();
        all.iter()
            .find(|u| u.window.is_active())
            .cloned()
            .or_else(|| all.last().cloned())
    })
}

/// Sekunden zwischen zwei Prüfungen, ob eine Wiederherstellungsdatei geschrieben werden muss.
const AUTOSAVE_SECS: u32 = 20;

fn recovery_dir() -> PathBuf {
    // Windows/macOS: im Datenordner des Programms (%LOCALAPPDATA%\stijl\recovery)
    if !cfg!(target_os = "linux") && std::env::var_os("XDG_STATE_HOME").is_none() {
        return glib::user_data_dir().join("stijl").join("recovery");
    }
    // $XDG_STATE_HOME bzw. ~/.local/state
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".local").join("state"))
        .join("stijl")
        .join("recovery")
}

fn next_recovery_id() -> String {
    RECOVERY_SEQ.with(|c| {
        c.set(c.get() + 1);
        crate::fsutil::recovery_id(c.get())
    })
}

/// Wiederherstellungsdatei eines Dokuments aktualisieren: nur bei ungespeicherten Änderungen,
/// die noch nicht gesichert sind. Die Originaldatei bleibt unberührt.
fn autosave_doc(d: &DocTab) {
    let Ok(ed) = d.ed.try_borrow() else { return };
    if !ed.dirty {
        drop(ed);
        drop_recovery(d);
        return;
    }
    if d.recovery_edits.get() == Some(ed.edits()) {
        return;
    }
    let edits = ed.edits();
    let text = svg::write(&ed.doc, true);
    drop(ed);
    let file = d.file.try_borrow().ok().and_then(|f| f.clone());
    let title = file
        .as_ref()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .or_else(|| d.title.try_borrow().ok().and_then(|t| t.clone()))
        .unwrap_or_else(|| tr("Unbenannt").into());
    if crate::fsutil::save_recovery(&recovery_dir(), &d.recovery_id, file.as_deref(), &title, &text).is_ok() {
        d.recovery_edits.set(Some(edits));
    }
}

/// Wiederherstellungsdatei entfernen (gespeichert, verworfen oder geschlossen).
fn drop_recovery(d: &DocTab) {
    if d.recovery_edits.get().is_some() {
        crate::fsutil::remove_recovery(&recovery_dir(), &d.recovery_id);
        d.recovery_edits.set(None);
    }
}

fn drop_all_recovery(ui: &Ui) {
    if let Ok(docs) = ui.docs.try_borrow() {
        for d in docs.iter() {
            drop_recovery(d);
        }
    }
}

fn autosave_all(ui: &Ui) {
    if let Ok(docs) = ui.docs.try_borrow() {
        for d in docs.iter() {
            autosave_doc(d);
        }
    }
}

/// Regelmäßig sichern, bei einem Programmfehler sofort; beim ersten Fenster nach Dateien eines
/// abgestürzten Laufs suchen.
fn setup_recovery(ui: &Rc<Ui>) {
    ALL_UIS.with(|l| l.borrow_mut().push(Rc::downgrade(ui)));
    {
        let w = Rc::downgrade(ui);
        glib::timeout_add_seconds_local(AUTOSAVE_SECS, move || match w.upgrade() {
            Some(u) => {
                autosave_all(&u);
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
    }
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Absturzbericht für den nächsten Start
            write_crash_report(info);
            // Notsicherung aller ungespeicherten Dokumente, dann die übliche Meldung
            let _ = ALL_UIS.try_with(|l| {
                if let Ok(list) = l.try_borrow() {
                    for w in list.iter() {
                        if let Some(u) = w.upgrade() {
                            autosave_all(&u);
                        }
                    }
                }
            });
            prev(info);
        }));
    });
    let u = ui.clone();
    glib::idle_add_local_once(move || {
        offer_recovery(&u);
        offer_crash_report(&u);
    });
    // nach dem Start in Ruhe nach Updates sehen
    let u = ui.clone();
    glib::timeout_add_seconds_local_once(5, move || check_for_updates(&u, false));
}

/// Nach einer neuen Version suchen (`manual`: auf Knopfdruck, dann auch Rückmeldung,
/// wenn nichts Neues da ist oder keine Verbindung besteht).
fn check_for_updates(ui: &Rc<Ui>, manual: bool) {
    use crate::update;
    if update::managed_externally() {
        return;
    }
    let now = update::now_secs();
    if !manual && (!ui.updates.get() || !update::due(ui.update_checked.get(), now)) {
        return;
    }
    ui.update_checked.set(now);
    save_settings(ui);
    let u = ui.clone();
    glib::spawn_future_local(async move {
        let res = gio::spawn_blocking(update::fetch_latest).await;
        match res {
            Ok(Ok(rel)) if update::is_newer(&rel.version, env!("CARGO_PKG_VERSION")) => {
                let t = adw::Toast::builder()
                    .title(trf("Stijl {} ist verfügbar", &[&rel.version]))
                    .button_label(tr("Herunterladen"))
                    .timeout(0)
                    .build();
                let win = u.window.clone();
                t.connect_button_clicked(move |_| {
                    gtk::UriLauncher::new(&rel.url).launch(Some(&win), gio::Cancellable::NONE, |_| {});
                });
                u.toasts.add_toast(t);
            }
            Ok(Ok(_)) => {
                if manual {
                    u.toast(tr("Stijl ist auf dem neuesten Stand"));
                }
            }
            _ => {
                if manual {
                    u.toast(tr("Keine Verbindung zu GitHub"));
                }
            }
        }
    });
}

/// Anleitung (Hilfe im Hauptmenü).
const HELP_URL: &str = "https://github.com/benhuesing/Stijl/blob/main/docs/USER_GUIDE.md";

/// Wohin „Fehler melden“ führt (neues Issue im Repository).
const ISSUE_URL: &str = "https://github.com/benhuesing/Stijl/issues/new";

/// Prozentkodierung für URL-Parameter.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Kurze Angaben zu Programm und System (für Fehlerberichte; keine persönlichen Daten).
fn system_info() -> String {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    format!(
        "Stijl {} · {} {}{} · GTK {}.{}.{} · libadwaita {}.{}.{}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        if desktop.is_empty() {
            String::new()
        } else {
            format!(" ({desktop})")
        },
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version(),
        adw::major_version(),
        adw::minor_version(),
        adw::micro_version(),
    )
}

/// „Fehler melden“: neues Issue im Browser öffnen, mit Vorlage und Systemangaben;
/// `report`: angehängter Absturzbericht.
fn report_bug(ui: &Rc<Ui>, report: Option<String>) {
    let mut body = String::new();
    match &report {
        Some(r) => {
            body.push_str(tr("**Was habe ich zuletzt gemacht?**\n\n\n"));
            body.push_str(tr("**Absturzbericht**\n```\n"));
            // URLs dürfen nicht beliebig lang werden
            let r: String = r.chars().take(3500).collect();
            body.push_str(&r);
            body.push_str("\n```\n");
        }
        None => {
            body.push_str(tr("**Was ist passiert?**\n\n\n"));
            body.push_str(tr("**Was hätte passieren sollen?**\n\n\n"));
            body.push_str(tr("**Schritte zum Nachstellen**\n1. \n2. \n\n"));
        }
    }
    body.push_str(&format!("---\n{}\n", system_info()));
    let title = if report.is_some() { tr("Absturz: ") } else { "" };
    let url = format!(
        "{ISSUE_URL}?title={}&body={}",
        url_encode(title),
        url_encode(&body)
    );
    gtk::UriLauncher::new(&url).launch(Some(&ui.window), gio::Cancellable::NONE, |_| {});
}

/// Datei für den Bericht nach einem Absturz.
fn crash_file() -> PathBuf {
    recovery_dir()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(recovery_dir)
        .join("crash-report.txt")
}

/// Im Panic-Hook: Bericht schreiben (Version, System, Fehlerstelle, Aufrufkette).
fn write_crash_report(info: &dyn std::fmt::Display) {
    let bt = std::backtrace::Backtrace::force_capture().to_string();
    // nur die ersten Zeilen der Aufrufkette – der Rest ist GTK-Innenleben
    let bt: String = bt.lines().take(60).collect::<Vec<_>>().join("\n");
    let when = glib::DateTime::now_local()
        .ok()
        .and_then(|t| t.format("%Y-%m-%d %H:%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let text = format!("{}\n{when}\n\n{info}\n\n{bt}\n", system_info());
    let path = crash_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, text);
}

/// Nach einem Absturz: fragen, ob der Bericht gesendet werden soll (vorher ansehbar).
fn offer_crash_report(ui: &Rc<Ui>) {
    thread_local! {
        static CRASH_CHECKED: Cell<bool> = const { Cell::new(false) };
    }
    if CRASH_CHECKED.with(|c| c.replace(true)) {
        return;
    }
    let path = crash_file();
    let Ok(report) = std::fs::read_to_string(&path) else {
        return;
    };
    let dialog = adw::AlertDialog::new(
        Some(tr("Stijl ist abgestürzt")),
        Some(
            tr("Beim letzten Mal ist ein Fehler aufgetreten. Möchtest du einen Fehlerbericht senden? Er enthält nur technische Angaben (Version, System, Fehlerstelle) – keine Zeichnungen."),
        ),
    );
    let view = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .build();
    view.buffer().set_text(&report);
    let scroll = gtk::ScrolledWindow::builder()
        .min_content_height(160)
        .max_content_height(260)
        .child(&view)
        .build();
    let expander = gtk::Expander::new(Some(tr("Bericht ansehen")));
    expander.set_child(Some(&scroll));
    dialog.set_extra_child(Some(&expander));
    dialog.add_response("no", tr("_Nicht senden"));
    dialog.add_response("copy", tr("_Kopieren"));
    dialog.add_response("send", tr("_Senden"));
    dialog.set_response_appearance("send", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("send"));
    dialog.set_close_response("no");
    let u = ui.clone();
    dialog.connect_response(None, move |_, resp| {
        match resp {
            "send" => report_bug(&u, Some(report.clone())),
            "copy" => {
                u.window.clipboard().set_text(&report);
                u.toast(tr("Bericht in die Zwischenablage kopiert"));
            }
            _ => {}
        }
        let _ = std::fs::remove_file(&path);
    });
    label_icon_buttons(dialog.upcast_ref());
    dialog.present(Some(&ui.window));
}

/// „Als Bild kopieren“: Auswahl (sonst die Zeichenfläche in der Bildmitte) als PNG in die
/// Zwischenablage – zum Einfügen in Chat, Mail oder Textverarbeitung.
fn copy_as_image(ui: &Rc<Ui>) {
    let Some(d) = cur(ui) else { return };
    let doc = {
        let ed = d.ed.borrow();
        ed.selection_doc(&ui.metrics).unwrap_or_else(|| {
            let (w, h) = ed.viewport;
            let c = ed.view.to_doc(Pt::new(w / 2.0, h / 2.0));
            let i = ed.page_index().unwrap_or_else(|| {
                ed.doc
                    .artboard_rects()
                    .iter()
                    .position(|(_, r)| r.contains(c))
                    .unwrap_or(0)
            });
            ed.doc.artboard_doc(i)
        })
    };
    // doppelte Auflösung, damit es auch auf scharfen Bildschirmen gut aussieht
    let png = match export::png_bytes(&doc, 2.0, doc.background) {
        Ok(p) => p,
        Err(e) => {
            ui.toast(&trf("Kopieren fehlgeschlagen: {}", &[&(e)]));
            return;
        }
    };
    match gdk::Texture::from_bytes(&glib::Bytes::from_owned(png)) {
        Ok(t) => {
            ui.window.clipboard().set_texture(&t);
            ui.toast(tr("Als Bild kopiert"));
        }
        Err(e) => ui.toast(&trf("Kopieren fehlgeschlagen: {}", &[&(e)])),
    }
}

/// Nach einem Absturz: gefundene Wiederherstellungsdateien anbieten.
fn offer_recovery(ui: &Rc<Ui>) {
    if RECOVERY_CHECKED.with(|c| c.replace(true)) {
        return;
    }
    let list = crate::fsutil::foreign_recoveries(&recovery_dir());
    if list.is_empty() {
        return;
    }
    let mut body = String::from(
        tr("Stijl wurde beim letzten Mal nicht richtig beendet. Diese ungespeicherten Dokumente lassen sich wiederherstellen:\n"),
    );
    for r in &list {
        let when = glib::DateTime::from_unix_local(r.time as i64)
            .ok()
            .and_then(|t| t.format("%d.%m.%Y, %H:%M").ok())
            .map(|s| s.to_string())
            .unwrap_or_default();
        body.push_str(&format!("\n• {} ({when})", r.title));
    }
    body.push_str(tr("\n\nDie Originaldateien wurden nicht verändert."));
    let dialog = adw::AlertDialog::new(Some(tr("Dokumente wiederherstellen?")), Some(&body));
    dialog.add_response("discard", tr("_Verwerfen"));
    dialog.add_response("later", tr("_Später"));
    dialog.add_response("restore", tr("_Wiederherstellen"));
    dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
    dialog.set_response_appearance("restore", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("restore"));
    dialog.set_close_response("later");
    let u = ui.clone();
    dialog.connect_response(None, move |_, resp| match resp {
        "restore" => {
            for r in &list {
                restore_recovery(&u, r);
            }
        }
        "discard" => {
            for r in &list {
                crate::fsutil::remove_recovery(&recovery_dir(), &r.id);
            }
        }
        _ => {}
    });
    label_icon_buttons(dialog.upcast_ref());
    dialog.present(Some(&ui.window));
}

/// Ein Dokument aus seiner Wiederherstellungsdatei öffnen. Es gilt als ungespeichert; ein
/// Speichern schreibt erst dann an den ursprünglichen Ort (SVG) bzw. fragt nach einem Namen.
fn restore_recovery(ui: &Rc<Ui>, r: &crate::fsutil::Recovery) {
    let doc = std::fs::read_to_string(&r.svg)
        .map_err(|e| e.to_string())
        .and_then(|t| svg::read(&t));
    let doc = match doc {
        Ok(doc) => doc,
        Err(e) => {
            ui.toast(&trf(
                "„{}“ ließ sich nicht wiederherstellen: {}",
                &[&(r.title), &(e)],
            ));
            return;
        }
    };
    let file = r.original.clone().filter(|p| {
        p.extension()
            .map(|e| e.eq_ignore_ascii_case("svg"))
            .unwrap_or(false)
    });
    let d = add_doc(ui, doc, file);
    if d.file.borrow().is_none() {
        *d.title.borrow_mut() = Some(r.title.clone());
    }
    d.ed.borrow_mut().dirty = true;
    crate::fsutil::remove_recovery(&recovery_dir(), &r.id);
    // gleich unter der neuen Kennung sichern
    autosave_doc(&d);
    refresh(ui);
}

fn write_svg(ui: &Rc<Ui>, d: &DocTab, path: PathBuf) -> bool {
    let data = svg::write(&d.ed.borrow().doc, true);
    match crate::fsutil::write_atomic(&path, data.as_bytes()) {
        Ok(()) => {
            add_recent(&path);
            *d.file.borrow_mut() = Some(path);
            d.ed.borrow_mut().dirty = false;
            drop_recovery(d);
            ui.toast(tr("Gespeichert"));
            refresh(ui);
            true
        }
        Err(e) => {
            ui.toast(&trf("Speichern fehlgeschlagen: {}", &[&(e)]));
            false
        }
    }
}

fn save(ui: &Rc<Ui>, d: &Rc<DocTab>, done: Done) {
    let path = d.file.borrow().clone();
    match path {
        Some(p)
            if p.extension()
                .map(|e| e.eq_ignore_ascii_case("svg"))
                .unwrap_or(false) =>
        {
            let ok = write_svg(ui, d, p);
            done(ui, ok);
        }
        _ => save_as(ui, d, done),
    }
}

fn file_filter(name: &str, patterns: &[&str]) -> gio::ListStore {
    let f = gtk::FileFilter::new();
    f.set_name(Some(name));
    for p in patterns {
        f.add_pattern(p);
    }
    let store = gio::ListStore::new::<gtk::FileFilter>();
    store.append(&f);
    store
}

fn save_as(ui: &Rc<Ui>, d: &Rc<DocTab>, done: Done) {
    let dialog = gtk::FileDialog::builder()
        .title(tr("Speichern unter"))
        .modal(true)
        .initial_name(format!("{}.svg", d.name()))
        .build();
    dialog.set_filters(Some(&file_filter(tr("SVG-Dateien"), &["*.svg"])));
    let (u, d) = (ui.clone(), d.clone());
    dialog.save(Some(&ui.window), gio::Cancellable::NONE, move |res| {
        let path = res.ok().and_then(|f| f.path());
        match path {
            Some(mut p) => {
                if p.extension().is_none() {
                    p.set_extension("svg");
                }
                let ok = write_svg(&u, &d, p);
                done(&u, ok);
            }
            None => done(&u, false),
        }
    });
}

fn open_dialog(ui: &Rc<Ui>) {
    let dialog = gtk::FileDialog::builder().title(tr("Öffnen")).modal(true).build();
    dialog.set_filters(Some(&file_filter(
        tr("SVG, Illustrator (.ai) und PDF"),
        &["*.svg", "*.SVG", "*.ai", "*.AI", "*.pdf", "*.PDF"],
    )));
    let u = ui.clone();
    dialog.open(Some(&ui.window), gio::Cancellable::NONE, move |res| {
        if let Some(p) = res.ok().and_then(|f| f.path()) {
            open_path(&u, &p);
        }
    });
}

fn dialog_shell(title: &str, width: i32) -> (adw::Dialog, gtk::Box) {
    let content = vbox(18);
    content.set_margin_top(12);
    content.set_margin_bottom(18);
    content.set_margin_start(18);
    content.set_margin_end(18);
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(&content));
    let dialog = adw::Dialog::builder()
        .title(title)
        .content_width(width)
        .child(&tv)
        .build();
    (dialog, content)
}

fn pill_button(label: &str) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.add_css_class("suggested-action");
    b.add_css_class("pill");
    b.set_halign(gtk::Align::Center);
    b
}

// ───────────────────────────── Formate / neues Dokument ─────────────────────────────

#[derive(Clone, Copy)]
struct Preset {
    group: &'static str,
    name: &'static str,
    w: f64,
    h: f64,
    unit: Unit,
}

const fn preset(group: &'static str, name: &'static str, w: f64, h: f64, unit: Unit) -> Preset {
    Preset {
        group,
        name,
        w,
        h,
        unit,
    }
}

/// Gängige Formate: Druck in Millimetern, Online und Bildschirm in Pixeln.
const PRESETS: &[Preset] = &[
    preset("Druck", "A4 Hochformat", 210.0, 297.0, Unit::Mm),
    preset("Druck", "A4 Querformat", 297.0, 210.0, Unit::Mm),
    preset("Druck", "A3 Hochformat", 297.0, 420.0, Unit::Mm),
    preset("Druck", "A5 Hochformat", 148.0, 210.0, Unit::Mm),
    preset("Druck", "A6 Hochformat", 105.0, 148.0, Unit::Mm),
    preset("Druck", "US Letter", 216.0, 279.0, Unit::Mm),
    preset("Druck", "Postkarte", 148.0, 105.0, Unit::Mm),
    preset("Druck", "Visitenkarte", 85.0, 55.0, Unit::Mm),
    preset("Druck", "DIN lang (Flyer)", 99.0, 210.0, Unit::Mm),
    preset("Online", "Instagram Post", 1080.0, 1080.0, Unit::Px),
    preset("Online", "Instagram Hochformat", 1080.0, 1350.0, Unit::Px),
    preset("Online", "Instagram Story / Reel", 1080.0, 1920.0, Unit::Px),
    preset("Online", "Facebook Post", 1200.0, 630.0, Unit::Px),
    preset("Online", "Facebook Titelbild", 851.0, 315.0, Unit::Px),
    preset("Online", "X / Twitter Post", 1600.0, 900.0, Unit::Px),
    preset("Online", "LinkedIn Post", 1200.0, 627.0, Unit::Px),
    preset("Online", "YouTube Thumbnail", 1280.0, 720.0, Unit::Px),
    preset("Online", "YouTube Banner", 2560.0, 1440.0, Unit::Px),
    preset("Online", "Pinterest Pin", 1000.0, 1500.0, Unit::Px),
    preset("Online", "TikTok Video", 1080.0, 1920.0, Unit::Px),
    preset("Bildschirm", "Full HD", 1920.0, 1080.0, Unit::Px),
    preset("Bildschirm", "4K UHD", 3840.0, 2160.0, Unit::Px),
];

const UNITS: [Unit; 5] = Unit::ALL;

impl Preset {
    fn px(&self) -> (f64, f64) {
        (self.w * self.unit.px(), self.h * self.unit.px())
    }
    fn size_label(&self) -> String {
        format!("{} × {} {}", fmt_num(self.w), fmt_num(self.h), self.unit.name())
    }
    fn label(&self) -> String {
        format!("{} · {} ({})", tr(self.group), tr(self.name), self.size_label())
    }
}

fn find_preset(name: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.name == name)
}

fn fmt_num(v: f64) -> String {
    if (v - v.round()).abs() < 1e-6 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}").replace('.', ",")
    }
}

/// Maßeinheit eines Dokuments (sonst die Voreinstellung).
fn doc_unit(ui: &Ui, ed: &Editor) -> Unit {
    Unit::from_name(&ed.doc.unit).unwrap_or(ui.unit.get())
}

/// Format, Einheit, Breite und Höhe – gemeinsam für „Neues Dokument“ und „Zeichenfläche“.
struct SizeEditor {
    group: adw::PreferencesGroup,
    format: adw::ComboRow,
    unit_row: adw::ComboRow,
    w: adw::SpinRow,
    h: adw::SpinRow,
    unit: Cell<Unit>,
    lock: Cell<bool>,
}

impl SizeEditor {
    fn new(w_px: f64, h_px: f64, unit: Unit) -> Rc<SizeEditor> {
        let mut names: Vec<String> = PRESETS.iter().map(|p| p.label()).collect();
        names.push(tr("Eigene Größe").into());
        let names: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let format = adw::ComboRow::builder()
            .title(tr("Format"))
            .model(&gtk::StringList::new(&names))
            .build();
        let unit_row = adw::ComboRow::builder()
            .title(tr("Einheit"))
            .model(&gtk::StringList::new(
                &UNITS.iter().map(|u| u.label()).collect::<Vec<_>>(),
            ))
            .build();
        let w = adw::SpinRow::with_range(0.1, 100000.0, 1.0);
        let h = adw::SpinRow::with_range(0.1, 100000.0, 1.0);
        let group = adw::PreferencesGroup::new();
        group.set_title(tr("Größe"));
        group.add(&format);
        group.add(&unit_row);
        group.add(&w);
        group.add(&h);
        let se = Rc::new(SizeEditor {
            group,
            format,
            unit_row,
            w,
            h,
            unit: Cell::new(unit),
            lock: Cell::new(true),
        });
        se.unit_row
            .set_selected(UNITS.iter().position(|u| *u == unit).unwrap_or(0) as u32);
        se.apply_unit_labels();
        se.w.set_value(w_px / unit.px());
        se.h.set_value(h_px / unit.px());
        let preset = PRESETS.iter().position(|p| {
            let (pw, ph) = p.px();
            (pw - w_px).abs() < 0.6 && (ph - h_px).abs() < 0.6
        });
        se.format.set_selected(preset.unwrap_or(PRESETS.len()) as u32);
        se.lock.set(false);

        {
            let s = Rc::downgrade(&se);
            se.format.connect_selected_notify(move |f| {
                let Some(s) = s.upgrade() else { return };
                if s.lock.get() {
                    return;
                }
                if let Some(p) = PRESETS.get(f.selected() as usize) {
                    s.lock.set(true);
                    // Online-Formate wechseln automatisch auf Pixel, Druckformate auf mm.
                    s.unit_row
                        .set_selected(UNITS.iter().position(|u| *u == p.unit).unwrap_or(0) as u32);
                    s.unit.set(p.unit);
                    s.apply_unit_labels();
                    s.w.set_value(p.w);
                    s.h.set_value(p.h);
                    s.lock.set(false);
                }
            });
        }
        {
            let s = Rc::downgrade(&se);
            se.unit_row.connect_selected_notify(move |r| {
                let Some(s) = s.upgrade() else { return };
                let new = UNITS[(r.selected() as usize).min(UNITS.len() - 1)];
                let old = s.unit.replace(new);
                if old == new {
                    return;
                }
                // Werte umrechnen, die Größe bleibt gleich.
                let prev = s.lock.replace(true);
                let (w, h) = (s.w.value() * old.px(), s.h.value() * old.px());
                s.apply_unit_labels();
                s.w.set_value(w / new.px());
                s.h.set_value(h / new.px());
                s.lock.set(prev);
            });
        }
        for row in [&se.w, &se.h] {
            let s = Rc::downgrade(&se);
            row.connect_value_notify(move |_| {
                let Some(s) = s.upgrade() else { return };
                if s.lock.get() {
                    return;
                }
                let (w, h) = s.px();
                let still = PRESETS.get(s.format.selected() as usize).map(|p| {
                    let (pw, ph) = p.px();
                    (pw - w).abs() < 0.6 && (ph - h).abs() < 0.6
                });
                if still != Some(true) {
                    s.lock.set(true);
                    s.format.set_selected(PRESETS.len() as u32);
                    s.lock.set(false);
                }
            });
        }
        se
    }

    fn apply_unit_labels(&self) {
        let u = self.unit.get();
        self.w.set_title(&trf("Breite ({})", &[&(u.name())]));
        self.h.set_title(&trf("Höhe ({})", &[&(u.name())]));
        self.w.set_digits(unit_digits(u));
        self.h.set_digits(unit_digits(u));
    }

    /// Größe in Dokumentpixeln.
    fn px(&self) -> (f64, f64) {
        let k = self.unit.get().px();
        (self.w.value() * k, self.h.value() * k)
    }

    fn select(&self, p: &Preset) {
        if let Some(i) = PRESETS.iter().position(|q| q.name == p.name) {
            self.format.set_selected(i as u32);
        }
    }
}

/// Neues Dokument anlegen: ersetzt ein leeres, unverändertes Dokument, sonst neuer Tab.
fn create_document(ui: &Rc<Ui>, w: f64, h: f64, unit: Unit, bg: Option<Color>) {
    let mut doc = Document::new(w, h, bg);
    doc.unit = unit.name().into();
    match cur(ui).filter(|d| d.pristine()) {
        Some(d) => {
            d.ed.borrow_mut().set_document(doc);
            ui.layers_shown.replace((0, 0, Vec::new()));
            refresh(ui);
        }
        None => {
            add_doc(ui, doc, None);
        }
    }
}

fn new_document_dialog(ui: &Rc<Ui>) {
    new_document_dialog_with(ui, None);
}

fn new_document_dialog_with(ui: &Rc<Ui>, start: Option<&Preset>) {
    let (dialog, content) = dialog_shell(tr("Neues Dokument"), 460);
    let a4 = find_preset("A4 Hochformat").copied().unwrap_or(PRESETS[0]);
    let (w0, h0) = a4.px();
    let size = SizeEditor::new(w0, h0, a4.unit);
    if let Some(p) = start {
        size.select(p);
    }
    let bg = adw::SwitchRow::builder()
        .title(tr("Weißer Hintergrund"))
        .subtitle(tr("Ausgeschaltet ist der Hintergrund transparent"))
        .active(true)
        .build();
    let bg_group = adw::PreferencesGroup::new();
    bg_group.add(&bg);
    content.append(&size.group);
    content.append(&bg_group);
    let create = pill_button(tr("Erstellen"));
    content.append(&create);
    {
        let (u, dlg, size) = (ui.clone(), dialog.clone(), size.clone());
        create.connect_clicked(move |_| {
            let (w, h) = size.px();
            dlg.close();
            create_document(&u, w, h, size.unit.get(), bg.is_active().then_some(Color::WHITE));
        });
    }
    dialog.set_default_widget(Some(&create));
    label_icon_buttons(dialog.upcast_ref());
    dialog.present(Some(&ui.window));
}

// ───────────────────────────── Zuletzt geöffnet ─────────────────────────────

const MAX_RECENT: usize = 12;

fn recent_path() -> PathBuf {
    glib::user_config_dir().join("stijl").join("recent")
}

fn load_recent() -> Vec<PathBuf> {
    std::fs::read_to_string(recent_path())
        .or_else(|_| std::fs::read_to_string(glib::user_config_dir().join("silhouettes").join("recent")))
        .or_else(|_| std::fs::read_to_string(glib::user_config_dir().join("linea").join("recent")))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(PathBuf::from)
        .take(MAX_RECENT)
        .collect()
}

fn store_recent(list: &[PathBuf]) {
    let path = recent_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text: String = list
        .iter()
        .map(|p| format!("{}\n", p.to_string_lossy()))
        .collect();
    let _ = crate::fsutil::write_atomic(&path, text.as_bytes());
}

fn add_recent(path: &Path) {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut list = load_recent();
    list.retain(|p| *p != path);
    list.insert(0, path);
    list.truncate(MAX_RECENT);
    store_recent(&list);
}

fn remove_recent(path: &Path) {
    let mut list = load_recent();
    list.retain(|p| p != path);
    store_recent(&list);
}

fn pretty_dir(p: &Path) -> String {
    let dir = p
        .parent()
        .map(|d| d.to_string_lossy().to_string())
        .unwrap_or_default();
    let home = glib::home_dir().to_string_lossy().to_string();
    match dir.strip_prefix(&home) {
        Some(rest) => format!("~{rest}"),
        None => dir,
    }
}

// ───────────────────────────── Informationen zum Dokument ─────────────────────────────

/// Rechte Leiste „Informationen“: Ordner, Format und Inhalt, Datei.
struct InfoWidgets {
    page: gtk::ScrolledWindow,
    folder: adw::ActionRow,
    folder_open: gtk::Button,
    format: adw::ActionRow,
    size: adw::ActionRow,
    artboards: adw::ActionRow,
    layers: adw::ActionRow,
    objects: adw::ActionRow,
    file_group: adw::PreferencesGroup,
    file_size: adw::ActionRow,
    created: adw::ActionRow,
    modified: adw::ActionRow,
}

fn build_info_panel() -> InfoWidgets {
    // Zeile mit kleinem Titel und dem Wert darunter
    let prop = |title: &str| {
        let r = adw::ActionRow::builder().title(title).subtitle("–").build();
        r.add_css_class("property");
        r.set_subtitle_selectable(true);
        r
    };
    let group = |rows: &[&adw::ActionRow]| {
        let g = adw::PreferencesGroup::new();
        for r in rows {
            g.add(*r);
        }
        g
    };
    let folder = prop(tr("Ordner"));
    let folder_open = gtk::Button::from_icon_name("folder-open-symbolic");
    folder_open.set_tooltip_text(Some(tr("Ordner öffnen")));
    folder_open.set_valign(gtk::Align::Center);
    folder_open.add_css_class("flat");
    folder.add_suffix(&folder_open);
    let format = prop(tr("Format"));
    let size = prop(tr("Größe"));
    let artboards = prop(tr("Zeichenflächen"));
    let layers = prop(tr("Ebenen"));
    let objects = prop(tr("Objekte"));
    let file_size = prop(tr("Dateigröße"));
    let created = prop(tr("Datei erstellt"));
    let modified = prop(tr("Datei geändert"));
    let file_group = group(&[&file_size, &created, &modified]);
    let content = vbox(18);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.append(&group(&[&folder]));
    content.append(&group(&[&format, &size, &artboards]));
    content.append(&group(&[&layers, &objects]));
    content.append(&file_group);
    let page = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&content)
        .build();
    InfoWidgets {
        page,
        folder,
        folder_open,
        format,
        size,
        artboards,
        layers,
        objects,
        file_group,
        file_size,
        created,
        modified,
    }
}

/// Datum und Uhrzeit in der Schreibweise des Systems.
fn local_time(t: std::time::SystemTime) -> String {
    t.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| glib::DateTime::from_unix_local(d.as_secs() as i64).ok())
        .and_then(|d| d.format("%x %X").ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "–".into())
}

/// Informationen zum aktuellen Dokument eintragen (nur, wenn die Leiste sichtbar ist).
fn refresh_info(ui: &Ui) {
    if !ui.info_toggle.is_active() {
        return;
    }
    let Some(d) = cur(ui) else { return };
    let w = &ui.info;
    let file = d.file.borrow().clone();
    match file.as_ref().and_then(|f| f.parent().map(|p| p.to_path_buf())) {
        Some(dir) => {
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| dir.to_string_lossy().to_string());
            w.folder.set_subtitle(&name);
            w.folder.set_tooltip_text(Some(&dir.to_string_lossy()));
            w.folder_open.set_visible(true);
        }
        None => {
            w.folder.set_subtitle(tr("Nicht gespeichert"));
            w.folder.set_tooltip_text(None);
            w.folder_open.set_visible(false);
        }
    }
    {
        let ed = d.ed.borrow();
        let u = doc_unit(ui, &ed);
        let k = u.px();
        let rects = ed.doc.artboard_rects();
        let r = rects[ed.page_index().unwrap_or(0).min(rects.len() - 1)].1;
        w.format.set_subtitle("SVG");
        w.size.set_subtitle(&format!(
            "{} × {} {}",
            fmt_num((r.width() / k * 10.0).round() / 10.0),
            fmt_num((r.height() / k * 10.0).round() / 10.0),
            u.name()
        ));
        w.artboards.set_subtitle(&rects.len().to_string());
        w.layers.set_subtitle(&ed.doc.layers.len().to_string());
        let n: usize = ed.doc.layers.iter().map(|l| l.objects.len()).sum();
        w.objects.set_subtitle(&n.to_string());
    }
    let meta = file.as_ref().and_then(|f| std::fs::metadata(f).ok());
    w.file_group.set_visible(meta.is_some());
    if let Some(m) = meta {
        w.file_size.set_subtitle(&glib::format_size(m.len()));
        w.created
            .set_subtitle(&m.created().map(local_time).unwrap_or_else(|_| "–".into()));
        w.modified
            .set_subtitle(&m.modified().map(local_time).unwrap_or_else(|_| "–".into()));
    }
}

// ───────────────────────────── Willkommensseite ─────────────────────────────

/// Akzentfarbe des Systems (für Markierungen, die selbst gezeichnet werden).
fn accent_rgb(w: &impl IsA<gtk::Widget>) -> (f64, f64, f64) {
    #[allow(deprecated)]
    let c = w.as_ref().style_context().lookup_color("accent_color");
    c.map(|c| (c.red() as f64, c.green() as f64, c.blue() as f64))
        .unwrap_or((0.208, 0.518, 0.894))
}

/// Knöpfe, die nur ein Symbol zeigen, bekommen den Text ihrer Kurzinfo als Namen für
/// Bildschirmleser (ohne Kürzel und Erklärung).
fn label_icon_buttons(root: &gtk::Widget) {
    fn has_text(w: &gtk::Widget) -> bool {
        if let Some(l) = w.downcast_ref::<gtk::Label>() {
            if !l.label().is_empty() {
                return true;
            }
        }
        let mut c = w.first_child();
        while let Some(ch) = c {
            if has_text(&ch) {
                return true;
            }
            c = ch.next_sibling();
        }
        false
    }
    let mut todo = vec![root.clone()];
    while let Some(w) = todo.pop() {
        let mut c = w.first_child();
        while let Some(ch) = c {
            todo.push(ch.clone());
            c = ch.next_sibling();
        }
        let button = w.is::<gtk::Button>() || w.is::<gtk::MenuButton>();
        if !button || has_text(&w) {
            continue;
        }
        if let Some(tip) = w.tooltip_text() {
            let name = short_label(&tip).trim().to_string();
            if !name.is_empty() {
                w.update_property(&[gtk::accessible::Property::Label(&name)]);
            }
        }
    }
}

/// Schwebende Leiste im Stil der Bildbetrachtung (dunkel, abgerundet).
fn osd_bar() -> gtk::Box {
    let b = hbox(2);
    b.add_css_class("osd");
    b.add_css_class("toolbar");
    b.add_css_class("mondrian-osd");
    b.set_margin_top(12);
    b.set_margin_bottom(12);
    b.set_margin_start(12);
    b.set_margin_end(12);
    b
}

/// Schwebende Leisten neben den Linealen halten.
fn update_osd_margins(ui: &Ui) {
    let r = if ui.show_rulers.get() { RULER as i32 } else { 0 };
    ui.tools_bar.set_margin_top(12 + r);
    ui.tools_bar.set_margin_start(12 + r);
    ui.view_bar.set_margin_start(12 + r);
}

/// Werkzeuge, Seitenleisten und schwebende Leisten zeigen (Dokument) oder verbergen
/// (Start-Tab).
fn set_editor_chrome(ui: &Ui, on: bool) {
    for bar in [&ui.tools_bar, &ui.view_bar, &ui.zoom_bar] {
        bar.set_visible(on);
    }
    update_osd_margins(ui);
    let props_was_hidden = !ui.dock.is_visible();
    ui.dock.set_visible(on && right_open(ui));
    if ui.dock.is_visible() && props_was_hidden {
        apply_props_width(ui);
    }
    let side_was_hidden = !ui.side_dock.is_visible();
    ui.side_dock.set_visible(on && ui.side_toggle.is_active());
    if ui.side_dock.is_visible() && side_was_hidden {
        ui.side_paned.set_position(ui.side_width.get());
    }

    for w in &ui.editor_only {
        w.set_visible(on);
    }
}

/// Neuer Tab mit der Willkommensseite.
fn add_start_tab(ui: &Rc<Ui>) {
    let (widget, recent_list, recent_box) = build_welcome(ui);
    {
        // Sobald die Startseite sichtbar wird: Werkzeuge und Seitenleisten ausblenden
        let wu = Rc::downgrade(ui);
        widget.connect_map(move |_| {
            if let Some(u) = wu.upgrade() {
                let u2 = u.clone();
                glib::idle_add_local_once(move || refresh(&u2));
            }
        });
    }
    let page = ui.tab_view.append(&widget);
    page.set_title(tr("Start"));
    ui.starts.borrow_mut().push(StartTab {
        page: page.clone(),
        recent_list,
        recent_box,
    });
    ui.tab_view.set_selected_page(&page);
    switch_to(ui, &page);
}

/// Höchstens so viele zuletzt geöffnete Dokumente (eine Reihe, daneben „Öffnen“).
const RECENT_CARDS: usize = 4;

fn build_welcome(ui: &Rc<Ui>) -> (gtk::Widget, gtk::FlowBox, gtk::Box) {
    let content = vbox(32);

    // Begrüßung (Platz für eine eigene Grafik: vorerst das App-Symbol)
    let welcome = vbox(8);
    welcome.set_margin_top(24);
    welcome.set_margin_bottom(8);
    let art = gtk::Image::builder()
        .icon_name("io.github.benhuesing.Stijl")
        .pixel_size(96)
        .margin_bottom(8)
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    let hello = gtk::Label::new(Some(tr("Willkommen bei Stijl")));
    hello.add_css_class("title-1");
    hello.set_wrap(true);
    hello.set_justify(gtk::Justification::Center);
    let hello_sub = gtk::Label::new(Some(tr(
        "Leg ein neues Dokument an oder mach dort weiter, wo du aufgehört hast.",
    )));
    hello_sub.add_css_class("dim-label");
    hello_sub.set_wrap(true);
    hello_sub.set_justify(gtk::Justification::Center);
    welcome.append(&art);
    welcome.append(&hello);
    welcome.append(&hello_sub);
    content.append(&welcome);

    // Zuletzt geöffnet: eine Reihe Karten, am Ende „Öffnen …“ – im selben Raster wie die
    // Formate darunter, damit alle Karten gleich groß sind
    let recent_title = gtk::Label::builder()
        .label(tr("Zuletzt geöffnet"))
        .xalign(0.0)
        .build();
    recent_title.add_css_class("heading");
    let recent_list = card_grid();
    {
        // bei jeder Breitenänderung neu entscheiden, wie viele Karten in die Reihe passen
        let last = Cell::new(0);
        recent_list.add_tick_callback(move |fb, _| {
            let w = fb.width();
            if w != last.get() {
                last.set(w);
                fit_recent_row(fb);
            }
            glib::ControlFlow::Continue
        });
    }
    let recent_box = vbox(8);
    recent_box.append(&recent_title);
    recent_box.append(&recent_list);
    content.append(&recent_box);

    // Formate: Reiter Print / Online, „Eigenes Format“ und Suche
    let formats_title = gtk::Label::builder()
        .label(tr("Neues Dokument"))
        .xalign(0.0)
        .build();
    formats_title.add_css_class("heading");
    let tab_print = gtk::ToggleButton::with_label(tr("Print"));
    let tab_online = gtk::ToggleButton::with_label(tr("Online"));
    tab_online.set_group(Some(&tab_print));
    tab_print.set_active(true);
    for t in [&tab_print, &tab_online] {
        t.add_css_class("flat");
        t.add_css_class("mondrian-tab");
    }
    let search = gtk::SearchEntry::builder()
        .placeholder_text(tr("Format suchen, z. B. „A5“ oder „Story“"))
        .hexpand(true)
        .margin_start(14)
        .build();
    // Suche steht neben den Reitern (spart eine Zeile), mit etwas Abstand dazwischen
    let bar = hbox(6);
    bar.append(&tab_print);
    bar.append(&tab_online);
    bar.append(&search);
    let cards = card_grid();
    let empty = gtk::Label::new(Some(tr(
        "Kein passendes Format – „Eigenes Format …“ legt jede Größe an",
    )));
    empty.add_css_class("dim-label");
    empty.set_visible(false);
    let fill = {
        let (u, cards, empty, tab_print, search) = (
            ui.clone(),
            cards.clone(),
            empty.clone(),
            tab_print.clone(),
            search.clone(),
        );
        Rc::new(move || {
            while let Some(c) = cards.first_child() {
                cards.remove(&c);
            }
            let q = search.text().to_lowercase();
            let print = tab_print.is_active();
            let mut n = 0;
            for p in PRESETS.iter() {
                let hit = if q.trim().is_empty() {
                    (p.group == "Druck") == print
                } else {
                    format!(
                        "{} {} {} {} {}",
                        p.name,
                        tr(p.name),
                        p.group,
                        tr(p.group),
                        p.size_label()
                    )
                    .to_lowercase()
                    .contains(q.trim())
                };
                if !hit {
                    continue;
                }
                let card = format_card(p);
                let u = u.clone();
                card.connect_clicked(move |_| {
                    let (w, h) = p.px();
                    create_document(&u, w, h, p.unit, Some(Color::WHITE));
                });
                cards.insert(&card, -1);
                n += 1;
            }
            // „Eigenes Format …“ als gestrichelte Kachel am Ende des Rasters
            let custom = dashed_tile(
                "document-page-setup-symbolic",
                tr("Eigenes Format …"),
                Some(tr("Größe frei wählen")),
            );
            custom.set_action_name(Some("win.new-dialog"));
            custom.set_tooltip_text(Some(tr("Dokument in beliebiger Größe anlegen")));
            cards.insert(&custom, -1);
            empty.set_visible(n == 0);
        })
    };
    fill();
    {
        let f = fill.clone();
        tab_print.connect_toggled(move |_| f());
    }
    {
        let f = fill.clone();
        search.connect_search_changed(move |_| f());
    }
    let formats = vbox(12);
    formats.append(&formats_title);
    formats.append(&bar);
    formats.append(&cards);
    formats.append(&empty);
    content.append(&formats);

    let clamp = adw::Clamp::builder()
        .maximum_size(820)
        .tightening_threshold(560)
        .child(&content)
        .build();
    clamp.set_margin_top(36);
    clamp.set_margin_bottom(36);
    clamp.set_margin_start(12);
    clamp.set_margin_end(12);
    let page = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&clamp)
        .build();
    fill_recent(ui, &recent_list, &recent_box);
    (page.upcast(), recent_list, recent_box)
}

/// Raster der Karten auf der Startseite (zuletzt geöffnet und Formate gleich groß).
fn card_grid() -> gtk::FlowBox {
    let g = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(2)
        .max_children_per_line(5)
        .column_spacing(12)
        .row_spacing(12)
        .valign(gtk::Align::Start)
        .build();
    g.add_css_class("mondrian-cards");
    g
}

/// Nur so viele zuletzt geöffnete Dokumente zeigen, wie in eine Reihe passen („Öffnen …“
/// bleibt immer am Ende). Die Spaltenzahl rechnet wie das Raster selbst.
fn fit_recent_row(fb: &gtk::FlowBox) {
    let w = fb.width();
    let mut kids = Vec::new();
    let mut c = fb.first_child();
    while let Some(k) = c {
        c = k.next_sibling();
        kids.push(k);
    }
    let n = kids.len();
    if w <= 0 || n == 0 {
        return;
    }
    // Breite einer Karte: die immer sichtbare Kachel „Öffnen …“
    let (cw, _, _, _) = kids[n - 1].measure(gtk::Orientation::Horizontal, -1);
    let cols = ((w + 12) / (cw.max(1) + 12)).clamp(2, 5) as usize;
    for (i, k) in kids.iter().enumerate() {
        let show = i == n - 1 || i + 1 < cols;
        if k.is_visible() != show {
            k.set_visible(show);
        }
    }
}

/// Größe der Vorschau in den Karten der Startseite
const CARD_PREVIEW_W: i32 = 120;
const CARD_PREVIEW_H: i32 = 90;

/// Inhalt einer Karte: Vorschau, Titel und eine kleine Zeile darunter (immer vorhanden,
/// damit alle Karten gleich hoch sind).
fn card_content(preview: &impl IsA<gtk::Widget>, title: &str, sub: &str) -> gtk::Box {
    let name = gtk::Label::builder()
        .label(title)
        .ellipsize(pango::EllipsizeMode::End)
        .max_width_chars(16)
        .build();
    name.add_css_class("heading");
    let small = gtk::Label::builder()
        .label(if sub.is_empty() { " " } else { sub })
        .ellipsize(pango::EllipsizeMode::End)
        .max_width_chars(18)
        .build();
    small.add_css_class("dim-label");
    small.add_css_class("caption");
    let b = vbox(6);
    b.set_margin_top(14);
    b.set_margin_bottom(12);
    b.set_margin_start(10);
    b.set_margin_end(10);
    b.append(preview);
    b.append(&name);
    b.append(&small);
    b
}

/// Gestrichelte Kachel in der Größe der Karten: Symbol statt Vorschau, darunter Titel
/// und (wie bei den Formatkarten) optional eine kleine Zeile.
fn dashed_tile(icon: &str, title: &str, sub: Option<&str>) -> gtk::Button {
    let symbol = gtk::Image::from_icon_name(icon);
    symbol.set_pixel_size(32);
    symbol.set_opacity(0.7);
    // feste Größe wie die Vorschauen; Symbol mittig (ohne vexpand, sonst wachsen alle
    // Karten der Reihe in die Höhe)
    let preview = gtk::CenterBox::new();
    preview.set_orientation(gtk::Orientation::Vertical);
    preview.set_size_request(CARD_PREVIEW_W, CARD_PREVIEW_H);
    preview.set_center_widget(Some(&symbol));
    let b = card_content(&preview, title, sub.unwrap_or(""));
    let button = gtk::Button::new();
    button.set_child(Some(&b));
    button.add_css_class("mondrian-dash-card");
    button.set_valign(gtk::Align::Start);
    button
}

/// Karte mit maßstäblicher Formatvorschau.
fn format_card(p: &'static Preset) -> gtk::Button {
    let (w, h) = p.px();
    let preview = gtk::DrawingArea::new();
    preview.set_content_width(CARD_PREVIEW_W);
    preview.set_content_height(CARD_PREVIEW_H);
    preview.set_draw_func(move |area, cr, aw, ah| {
        let (aw, ah) = (aw as f64, ah as f64);
        let s = ((aw - 8.0) / w).min((ah - 8.0) / h);
        let (pw, ph) = ((w * s).max(4.0), (h * s).max(4.0));
        let (x, y) = (((aw - pw) / 2.0).round(), ((ah - ph) / 2.0).round());
        let fg = area.color();
        cr.rectangle(x, y, pw, ph);
        cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
        let _ = cr.fill_preserve();
        cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.35);
        cr.set_line_width(1.0);
        cr.rectangle(x + 0.5, y + 0.5, pw - 1.0, ph - 1.0);
        let _ = cr.stroke();
    });
    let b = card_content(&preview, tr(p.name), &p.size_label());
    let button = gtk::Button::new();
    button.set_child(Some(&b));
    button.add_css_class("card");
    button.set_valign(gtk::Align::Start);
    button.set_tooltip_text(Some(&format!("{} – {}", tr(p.group), p.size_label())));
    button
}

fn export_dialog(ui: &Rc<Ui>) {
    export_dialog_with(ui, false);
}

/// Export-Dialog; `selection_only`: nur die Auswahl (vorbelegt).
fn export_dialog_with(ui: &Rc<Ui>, selection_only: bool) {
    let Some(d) = cur(ui) else { return };
    let has_selection = !d.ed.borrow().selection.is_empty();
    if selection_only && !has_selection {
        ui.toast(tr("Erst etwas auswählen"));
        return;
    }
    let s = ui.export.borrow().clone();
    let (w, h, has_bg, boards) = {
        let ed = d.ed.borrow();
        (
            ed.doc.width,
            ed.doc.height,
            ed.doc.background.is_some(),
            ed.doc.artboard_rects(),
        )
    };
    let (dialog, content) = dialog_shell(tr("Exportieren"), 440);
    // Mehrere Zeichenflächen: auswählen, welche exportiert werden – als Vorschaubilder
    let board_docs: Vec<crate::model::Document> = {
        let ed = d.ed.borrow();
        (0..boards.len()).map(|i| ed.doc.artboard_doc(i)).collect()
    };
    let board_checks: Vec<gtk::ToggleButton> = boards
        .iter()
        .zip(board_docs)
        .map(|((name, r), doc)| {
            let (tw, th) = (120, 90);
            let area = gtk::DrawingArea::new();
            area.set_content_width(tw);
            area.set_content_height(th);
            area.set_draw_func(move |_, cr, w, h| {
                let dark = adw::StyleManager::default().is_dark();
                render::draw_artboard_thumbnail(cr, w as f64, h as f64, &doc, dark);
            });
            let title = gtk::Label::builder()
                .label(name.as_str())
                .ellipsize(pango::EllipsizeMode::End)
                .max_width_chars(14)
                .build();
            title.add_css_class("caption");
            let size = gtk::Label::new(Some(&format!(
                "{} × {} px",
                r.width().round(),
                r.height().round()
            )));
            size.add_css_class("caption");
            size.add_css_class("dim-label");
            let card = vbox(4);
            card.append(&area);
            card.append(&title);
            card.append(&size);
            let t = gtk::ToggleButton::builder()
                .child(&card)
                .active(true)
                .tooltip_text(tr("Klicken zum Aus- oder Abwählen"))
                .build();
            t.add_css_class("mondrian-pattern-btn");
            t.add_css_class("flat");
            t
        })
        .collect();
    // nur die Auswahl exportieren (so groß wie die Auswahl)
    let only_sel = adw::SwitchRow::builder()
        .title(tr("Nur die Auswahl"))
        .active(selection_only)
        .build();
    if has_selection {
        let g = adw::PreferencesGroup::new();
        g.add(&only_sel);
        content.append(&g);
    }
    if boards.len() > 1 {
        let g = adw::PreferencesGroup::new();
        g.set_visible(!selection_only);
        {
            let g = g.clone();
            only_sel.connect_active_notify(move |r| g.set_visible(!r.is_active()));
        }
        g.set_title(tr("Zeichenflächen"));
        g.set_description(Some(tr(
            "PDF: eine Seite je Zeichenfläche. Andere Formate: eine Datei je Zeichenfläche.",
        )));
        // Alle / Keine
        let all = gtk::Button::with_label(tr("Alle"));
        let none = gtk::Button::with_label(tr("Keine"));
        let bx = hbox(0);
        bx.add_css_class("linked");
        bx.set_valign(gtk::Align::Center);
        bx.append(&all);
        bx.append(&none);
        g.set_header_suffix(Some(&bx));
        for (btn, on) in [(all, true), (none, false)] {
            let checks = board_checks.clone();
            btn.connect_clicked(move |_| {
                for c in &checks {
                    c.set_active(on);
                }
            });
        }
        let flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .min_children_per_line(2)
            .max_children_per_line(3)
            .column_spacing(8)
            .row_spacing(8)
            .build();
        for c in &board_checks {
            flow.append(c);
        }
        // viele Zeichenflächen: rollen statt den Dialog zu sprengen
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(300)
            .child(&flow)
            .build();
        g.add(&scroll);
        content.append(&g);
    }
    let labels: Vec<&str> = Format::ALL.iter().map(|f| f.label()).collect();
    let format = adw::ComboRow::builder()
        .title(tr("Format"))
        .model(&gtk::StringList::new(&labels))
        .build();
    format.set_selected(s.format);
    let bg = adw::SwitchRow::builder()
        .title(tr("Hintergrund mitspeichern"))
        .active(s.background)
        .build();
    let scale_labels: Vec<String> = (1..=4)
        .map(|k| {
            format!(
                "{k}× ({} × {} px)",
                (w * k as f64).round(),
                (h * k as f64).round()
            )
        })
        .collect();
    let scale_refs: Vec<&str> = scale_labels.iter().map(String::as_str).collect();
    let scale = adw::ComboRow::builder()
        .title(tr("Auflösung"))
        .model(&gtk::StringList::new(&scale_refs))
        .build();
    scale.set_selected(s.scale);
    let quality = adw::SpinRow::with_range(10.0, 100.0, 5.0);
    quality.set_title(tr("JPG-Qualität"));
    quality.set_value(s.quality);
    let embed = adw::SwitchRow::builder()
        .title(tr("Schriften einbetten"))
        .subtitle(tr(
            "Die verwendeten Schriften werden in die SVG-Datei gelegt (größere Datei)",
        ))
        .active(s.embed_fonts)
        .build();
    let fx_quality = adw::ComboRow::builder()
        .title(tr("Qualität der Effekte"))
        .tooltip_text(tr("Auflösung von Schatten, Glas-, 3D- und Kunsteffekten"))
        .model(&gtk::StringList::new(&[
            tr("Klein (150 dpi)"),
            tr("Mittel (300 dpi)"),
            tr("Hoch (600 dpi)"),
        ]))
        .build();
    fx_quality.set_selected(s.effect_quality.min(2));
    let text_mode = adw::ComboRow::builder()
        .title(tr("Text"))
        .model(&gtk::StringList::new(&[
            tr("Durchsuchbar"),
            tr("In Umrisse umwandeln"),
        ]))
        .build();
    text_mode.set_selected(if s.pdf_text { 0 } else { 1 });
    let colors = adw::ComboRow::builder()
        .title(tr("Farben"))
        .model(&gtk::StringList::new(&[
            tr("RGB (Bildschirm)"),
            tr("CMYK (Druck)"),
            tr("PDF/X-4 (Druckerei)"),
        ]))
        .build();
    colors.set_selected(s.color.min(2));
    // Farbprofil: zuletzt gewähltes oder das erste gefundene
    let profile: Rc<RefCell<Option<(PathBuf, String)>>> = Rc::new(RefCell::new(
        s.profile
            .clone()
            .filter(|(p, _)| p.exists())
            .or_else(|| export::find_profiles().into_iter().next()),
    ));
    let profile_row = adw::ActionRow::builder().title(tr("Farbprofil")).build();
    let profile_btn = gtk::Button::with_label(tr("Wählen…"));
    profile_btn.set_valign(gtk::Align::Center);
    profile_row.add_suffix(&profile_btn);
    let show_profile = {
        let (row, profile) = (profile_row.clone(), profile.clone());
        move || {
            row.set_subtitle(&match &*profile.borrow() {
                Some((_, d)) => d.clone(),
                None => tr("Keins gefunden – z. B. „PSO Coated v3“ von eci.org laden und wählen").to_string(),
            });
        }
    };
    show_profile();
    {
        let (u, profile, show) = (ui.clone(), profile.clone(), show_profile.clone());
        profile_btn.connect_clicked(move |b| {
            let fd = gtk::FileDialog::builder()
                .title(tr("Farbprofil wählen"))
                .modal(true)
                .build();
            fd.set_filters(Some(&file_filter(
                tr("ICC-Farbprofile"),
                &["*.icc", "*.ICC", "*.icm", "*.ICM"],
            )));
            if let Some(dir) = ["/usr/share/color/icc", "/usr/local/share/color/icc"]
                .iter()
                .find(|d| Path::new(d).is_dir())
            {
                fd.set_initial_folder(Some(&gio::File::for_path(dir)));
            }
            let (u, profile, show) = (u.clone(), profile.clone(), show.clone());
            let win = b.root().and_then(|r| r.downcast::<gtk::Window>().ok());
            fd.open(win.as_ref(), gio::Cancellable::NONE, move |res| {
                let Some(p) = res.ok().and_then(|f| f.path()) else {
                    return;
                };
                let parsed = std::fs::read(&p)
                    .ok()
                    .and_then(|d| crate::icc::CmykProfile::parse(&d));
                match parsed {
                    Some(prof) => {
                        let desc = if prof.description.is_empty() {
                            p.file_stem()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_default()
                        } else {
                            prof.description.clone()
                        };
                        *profile.borrow_mut() = Some((p, desc));
                        show();
                    }
                    None => u.toast(tr("Das ist kein CMYK-Druckprofil (ICC)")),
                }
            });
        });
    }
    let bleed = adw::SpinRow::with_range(0.0, 10.0, 0.5);
    bleed.set_digits(1);
    bleed.set_title(tr("Beschnittzugabe (mm)"));
    bleed.set_value(s.bleed);
    let group = adw::PreferencesGroup::new();
    group.add(&format);
    group.add(&bg);
    group.add(&embed);
    group.add(&scale);
    group.add(&quality);
    group.add(&fx_quality);
    content.append(&group);
    let pdf_group = adw::PreferencesGroup::new();
    pdf_group.set_title("PDF");
    pdf_group.add(&text_mode);
    pdf_group.add(&colors);
    pdf_group.add(&profile_row);
    pdf_group.add(&bleed);
    content.append(&pdf_group);
    let button = pill_button(tr("Exportieren…"));
    content.append(&button);

    let update = {
        let (format, scale, quality, bg, embed, fx_quality, pdf_group, colors, profile_row) = (
            format.clone(),
            scale.clone(),
            quality.clone(),
            bg.clone(),
            embed.clone(),
            fx_quality.clone(),
            pdf_group.clone(),
            colors.clone(),
            profile_row.clone(),
        );
        move || {
            let f = Format::ALL[(format.selected() as usize).min(3)];
            // Schriften einbetten braucht Fontconfig (unter Windows und macOS nicht vorhanden)
            embed.set_visible(f == Format::Svg && !cfg!(any(windows, target_os = "macos")));
            scale.set_visible(matches!(f, Format::Png | Format::Jpeg));
            quality.set_visible(f == Format::Jpeg);
            fx_quality.set_visible(matches!(f, Format::Pdf | Format::Svg));
            pdf_group.set_visible(f == Format::Pdf);
            profile_row.set_visible(colors.selected() > 0);
            if f == Format::Jpeg && !bg.is_active() {
                bg.set_subtitle(tr("JPG unterstützt keine Transparenz – es wird Weiß verwendet"));
            } else if !has_bg {
                bg.set_subtitle(tr("Dieses Dokument hat einen transparenten Hintergrund"));
            } else {
                bg.set_subtitle("");
            }
        }
    };
    update();
    {
        let up = update.clone();
        format.connect_selected_notify(move |_| up());
    }
    {
        // PDF/X: Druckereien wollen fast immer eine Beschnittzugabe
        let (up, bleed) = (update.clone(), bleed.clone());
        colors.connect_selected_notify(move |c| {
            if c.selected() == 2 && bleed.value() == 0.0 {
                bleed.set_value(3.0);
            }
            up();
        });
    }
    bg.connect_active_notify(move |_| update());

    {
        let (u, dlg) = (ui.clone(), dialog.clone());
        button.connect_clicked(move |_| {
            let settings = ExportSettings {
                format: format.selected().min(3),
                background: bg.is_active(),
                scale: scale.selected().min(3),
                quality: quality.value(),
                embed_fonts: embed.is_active(),
                effect_quality: fx_quality.selected().min(2),
                pdf_text: text_mode.selected() == 0,
                color: colors.selected().min(2),
                profile: profile.borrow().clone(),
                bleed: bleed.value(),
            };
            if settings.format == 2 && settings.color == 2 && settings.profile.is_none() {
                u.toast(tr("Für PDF/X zuerst ein Farbprofil wählen"));
                return;
            }
            *u.export.borrow_mut() = settings.clone();
            if has_selection && only_sel.is_active() {
                dlg.close();
                choose_export_file(&u, &d, settings, None);
                return;
            }
            let picked: Vec<usize> = board_checks
                .iter()
                .enumerate()
                .filter(|(_, c)| c.is_active())
                .map(|(i, _)| i)
                .collect();
            if picked.is_empty() {
                u.toast(tr("Mindestens eine Zeichenfläche auswählen"));
                return;
            }
            dlg.close();
            choose_export_file(&u, &d, settings, Some(picked));
        });
    }
    label_icon_buttons(dialog.upcast_ref());
    dialog.present(Some(&ui.window));
}

/// Exportieren in eine Datei: `boards` = gewählte Zeichenflächen, `None` = nur die Auswahl.
fn choose_export_file(ui: &Rc<Ui>, d: &Rc<DocTab>, s: ExportSettings, boards: Option<Vec<usize>>) {
    let fmt = Format::ALL[s.format as usize];
    let opt = export::Options {
        format: fmt,
        background: s.background,
        scale: (s.scale + 1) as f64,
        quality: s.quality.round() as u8,
        embed_fonts: s.embed_fonts,
        effect_dpi: EFFECT_DPI[(s.effect_quality as usize).min(2)],
        pdf_text: s.pdf_text,
        color: match s.color {
            1 => export::ColorMode::Cmyk,
            2 => export::ColorMode::PdfX,
            _ => export::ColorMode::Rgb,
        },
        profile: s.profile.as_ref().map(|(p, _)| p.clone()),
        bleed_mm: if fmt == Format::Pdf { s.bleed } else { 0.0 },
        title: d.name(),
    };
    let dialog = gtk::FileDialog::builder()
        .title(tr("Exportieren"))
        .modal(true)
        .initial_name(format!("{}.{}", d.name(), fmt.ext()))
        .build();
    dialog.set_filters(Some(&file_filter(fmt.label(), fmt.pattern())));
    let (u, d) = (ui.clone(), d.clone());
    dialog.save(Some(&ui.window), gio::Cancellable::NONE, move |res| {
        let Some(mut path) = res.ok().and_then(|f| f.path()) else {
            return;
        };
        if path.extension().is_none() {
            path.set_extension(fmt.ext());
        }
        let (docs, names): (Vec<crate::model::Document>, Vec<String>) = {
            let ed = d.ed.borrow();
            match &boards {
                Some(boards) => {
                    let rects = ed.doc.artboard_rects();
                    boards
                        .iter()
                        .map(|i| (ed.doc.artboard_doc(*i), rects[*i].0.clone()))
                        .unzip()
                }
                None => match ed.selection_doc(&u.metrics) {
                    Some(doc) => (vec![doc], vec![tr("Auswahl").to_string()]),
                    None => (Vec::new(), Vec::new()),
                },
            }
        };
        if docs.is_empty() {
            u.toast(tr("Erst etwas auswählen"));
            return;
        }
        let result = if docs.len() == 1 {
            export::export(&docs[0], &path, &opt).map(|_| 1)
        } else if fmt == Format::Pdf {
            export::export_pdf_pages(&docs, &path, &opt).map(|_| 1)
        } else {
            // eine Datei je Zeichenfläche: „Name-Zeichenfläche 2.png“
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| tr("Export").into());
            let mut n = 0;
            let mut res = Ok(0);
            for (doc, name) in docs.iter().zip(&names) {
                let clean: String = name
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || c == ' ' || c == '-' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect();
                let p = path.with_file_name(format!("{stem}-{}.{}", clean.trim(), fmt.ext()));
                if let Err(e) = export::export(doc, &p, &opt) {
                    res = Err(e);
                    break;
                }
                n += 1;
            }
            res.map(|_: usize| n)
        };
        match result {
            Ok(1) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                u.toast(&trf("Exportiert: {}", &[&(name)]));
            }
            Ok(n) => u.toast(&trf("{} Dateien exportiert", &[&(n)])),
            Err(e) => u.toast(&trf("Export fehlgeschlagen: {}", &[&(e)])),
        }
    });
}

// ───────────────────────────── Zeichenfläche, Einstellungen, Bilder ─────────────────────────────

/// Auswahl um einen Prozentwert skalieren (gleichmäßig oder Breite/Höhe getrennt).
fn scale_dialog(ui: &Rc<Ui>) {
    let Some(d) = cur(ui) else { return };
    if d.ed.borrow().selection.is_empty() {
        ui.toast(tr("Erst etwas auswählen"));
        return;
    }
    let (dialog, content) = dialog_shell(tr("Skalieren"), 400);
    let group = adw::PreferencesGroup::new();
    let uniform = adw::SpinRow::with_range(1.0, 10000.0, 1.0);
    uniform.set_title(tr("Größe (%)"));
    uniform.set_value(100.0);
    let separate = adw::SwitchRow::builder()
        .title(tr("Breite und Höhe getrennt"))
        .build();
    let sx = adw::SpinRow::with_range(1.0, 10000.0, 1.0);
    sx.set_title(tr("Breite (%)"));
    sx.set_value(100.0);
    let sy = adw::SpinRow::with_range(1.0, 10000.0, 1.0);
    sy.set_title(tr("Höhe (%)"));
    sy.set_value(100.0);
    let strokes = adw::SwitchRow::builder()
        .title(tr("Konturen und Schatten mitskalieren"))
        .active(true)
        .build();
    group.add(&uniform);
    group.add(&separate);
    group.add(&sx);
    group.add(&sy);
    group.add(&strokes);
    content.append(&group);
    let button = pill_button(tr("Skalieren"));
    content.append(&button);
    let sync = {
        let (uniform, sx, sy) = (uniform.clone(), sx.clone(), sy.clone());
        move |sep: bool| {
            uniform.set_visible(!sep);
            sx.set_visible(sep);
            sy.set_visible(sep);
        }
    };
    sync(false);
    separate.connect_active_notify(move |r| sync(r.is_active()));
    {
        let (u, dlg, separate) = (ui.clone(), dialog.clone(), separate.clone());
        button.connect_clicked(move |_| {
            let (fx, fy) = if separate.is_active() {
                (sx.value() / 100.0, sy.value() / 100.0)
            } else {
                (uniform.value() / 100.0, uniform.value() / 100.0)
            };
            let st = strokes.is_active();
            with_ed_m(&u, |e, m| {
                e.scale_selection(fx, fy, st, m);
            });
            dlg.close();
        });
    }
    label_icon_buttons(dialog.upcast_ref());
    dialog.present(Some(&ui.window));
}

fn artboard_dialog(ui: &Rc<Ui>) {
    artboard_dialog_at(ui, 0);
}

fn artboard_dialog_at(ui: &Rc<Ui>, idx: usize) {
    let Some(d) = cur(ui) else { return };
    let (name0, w0, h0, bg0, unit, many) = {
        let ed = d.ed.borrow();
        let rects = ed.doc.artboard_rects();
        let Some((n, r)) = rects.get(idx).cloned() else {
            return;
        };
        (
            n,
            r.width(),
            r.height(),
            ed.doc.background,
            doc_unit(ui, &ed),
            rects.len() > 1,
        )
    };
    let (dialog, content) = dialog_shell(
        if many {
            name0.as_str()
        } else {
            tr("Zeichenfläche")
        },
        460,
    );
    let size = SizeEditor::new(w0, h0, unit);
    // weitere Zeichenflächen lassen sich umbenennen
    let name_entry = adw::EntryRow::builder().title(tr("Name")).text(&name0).build();
    if idx > 0 {
        let g = adw::PreferencesGroup::new();
        g.add(&name_entry);
        content.append(&g);
    }

    let bg_on = adw::SwitchRow::builder()
        .title(tr("Hintergrundfarbe"))
        .subtitle(tr("Ausgeschaltet ist die Zeichenfläche transparent"))
        .active(bg0.is_some())
        .build();
    let color = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::builder().with_alpha(true).build()));
    color.set_rgba(&to_rgba(bg0.unwrap_or(Color::WHITE)));
    color.set_valign(gtk::Align::Center);
    let color_row = adw::ActionRow::builder().title(tr("Farbe")).build();
    color_row.add_suffix(&color);
    bg_on
        .bind_property("active", &color_row, "sensitive")
        .sync_create()
        .build();
    let bg_group = adw::PreferencesGroup::new();
    bg_group.set_title(if many {
        tr("Hintergrund (alle Zeichenflächen)")
    } else {
        tr("Hintergrund")
    });
    bg_group.add(&bg_on);
    bg_group.add(&color_row);

    content.append(&size.group);
    content.append(&bg_group);
    let apply = pill_button(tr("Übernehmen"));
    content.append(&apply);
    {
        let (u, dlg) = (ui.clone(), dialog.clone());
        apply.connect_clicked(move |_| {
            let bg = bg_on.is_active().then(|| from_rgba(&color.rgba()));
            let (w, h) = size.px();
            {
                let mut ed = d.ed.borrow_mut();
                if idx == 0 {
                    ed.set_artboard(w, h, bg);
                } else {
                    ed.set_artboard_size(idx, w, h);
                    let (mw, mh) = (ed.doc.width, ed.doc.height);
                    ed.set_artboard(mw, mh, bg);
                    ed.rename_artboard(idx, &name_entry.text());
                }
                ed.doc.unit = size.unit.get().name().into();
            }
            dlg.close();
            refresh(&u);
        });
    }
    label_icon_buttons(dialog.upcast_ref());
    dialog.present(Some(&ui.window));
}

/// Eintrag der Einstellungen für die Suche: Kategorie, Gruppe, Zeile und Suchtext.
struct PrefEntry {
    cat: usize,
    group: usize,
    widget: gtk::Widget,
    text: String,
}

/// Alle Kürzel der Anwendung vorübergehend abmelden (während eine Taste aufgenommen wird).
fn clear_accels(app: &gtk::Application) {
    use crate::keys::{Kind, DEFS};
    for d in DEFS {
        if let Kind::Action(action) = d.kind {
            app.set_accels_for_action(action, &[]);
        }
    }
}

fn is_modifier_key(k: gdk::Key) -> bool {
    matches!(
        k,
        gdk::Key::Shift_L
            | gdk::Key::Shift_R
            | gdk::Key::Control_L
            | gdk::Key::Control_R
            | gdk::Key::Alt_L
            | gdk::Key::Alt_R
            | gdk::Key::Super_L
            | gdk::Key::Super_R
            | gdk::Key::Meta_L
            | gdk::Key::Meta_R
            | gdk::Key::Caps_Lock
            | gdk::Key::ISO_Level3_Shift
    )
}

/// Dialog „Neue Tastenkombination drücken“. `done` wird nach einer Änderung aufgerufen.
fn capture_shortcut(ui: &Rc<Ui>, parent: &impl IsA<gtk::Widget>, id: &'static str, done: Rc<dyn Fn()>) {
    let Some(def) = crate::keys::def(id) else { return };
    let app = ui.window.application();
    if let Some(app) = &app {
        clear_accels(app);
    }
    let info = gtk::Label::new(Some(tr("Jetzt die neue Tastenkombination drücken …")));
    info.add_css_class("title-3");
    let hint = gtk::Label::builder()
        .label(tr("Esc bricht ab, ⌫ entfernt das Kürzel"))
        .wrap(true)
        .build();
    hint.add_css_class("dim-label");
    let msg = gtk::Label::builder()
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();
    msg.add_css_class("error");
    let bx = vbox(12);
    bx.set_margin_top(24);
    bx.set_margin_bottom(24);
    bx.set_margin_start(24);
    bx.set_margin_end(24);
    bx.append(&info);
    bx.append(&hint);
    bx.append(&msg);
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(&bx));
    let dlg = adw::Dialog::builder()
        .title(tr(def.label))
        .content_width(380)
        .child(&tv)
        .build();
    let ctl = gtk::EventControllerKey::new();
    ctl.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let dlg2 = dlg.clone();
        let msg = msg.clone();
        ctl.connect_key_pressed(move |_, key, _, st| {
            if is_modifier_key(key) {
                return glib::Propagation::Stop;
            }
            let mods = accel_mods(st);
            if mods.is_empty() && key == gdk::Key::Escape {
                dlg2.close();
                return glib::Propagation::Stop;
            }
            let keys: Vec<String> = if mods.is_empty() && key == gdk::Key::BackSpace {
                Vec::new()
            } else {
                let name = gtk::accelerator_name(key.to_lower(), mods).to_string();
                if let Err(why) = crate::keys::allowed(def.kind, &name) {
                    msg.set_label(why);
                    return glib::Propagation::Stop;
                }
                if let Some(other) = KEYMAP.with(|k| k.borrow().owner(&name, def.id)) {
                    msg.set_label(&trf(
                        "{} ist schon vergeben für „{}“. Eine andere Kombination wählen oder Esc drücken.",
                        &[&(crate::keys::label(&name)), &(tr(other.label))],
                    ));
                    return glib::Propagation::Stop;
                }
                vec![name]
            };
            KEYMAP.with(|k| k.borrow_mut().set(def.id, keys));
            save_keymap();
            dlg2.close();
            glib::Propagation::Stop
        });
    }
    dlg.add_controller(ctl);
    {
        let done = done.clone();
        dlg.connect_closed(move |_| {
            if let Some(app) = &app {
                apply_accels(app);
            }
            done();
        });
    }
    label_icon_buttons(dlg.upcast_ref());
    dlg.present(Some(parent));
}

fn preferences_dialog(ui: &Rc<Ui>) {
    preferences_dialog_at(ui, 0);
}

/// Einstellungen mit der Kategorie `start` (0 Ansicht, 1 Ausrichten, 2 Tastenkürzel).
fn preferences_dialog_at(ui: &Rc<Ui>, start: usize) {
    const CATS: [(&str, &str); 3] = [
        ("Ansicht", "preferences-desktop-appearance-symbolic"),
        ("Ausrichten", "mondrian-guide-symbolic"),
        ("Tastenkürzel", "input-keyboard-symbolic"),
    ];
    let entries: Rc<RefCell<Vec<PrefEntry>>> = Rc::new(RefCell::new(Vec::new()));
    let groups: Rc<RefCell<Vec<(usize, adw::PreferencesGroup)>>> = Rc::new(RefCell::new(Vec::new()));
    // legt eine Gruppe in der Kategorie an
    let new_group = {
        let groups = groups.clone();
        move |cat: usize, title: &str, desc: Option<&str>| -> (usize, adw::PreferencesGroup) {
            let g = adw::PreferencesGroup::new();
            if !title.is_empty() {
                g.set_title(title);
            }
            if let Some(d) = desc {
                g.set_description(Some(d));
            }
            let mut gs = groups.borrow_mut();
            gs.push((cat, g.clone()));
            (gs.len() - 1, g)
        }
    };
    // Zeile in die Gruppe setzen und für die Suche merken
    let add_row = {
        let entries = entries.clone();
        move |cat: usize, gi: usize, g: &adw::PreferencesGroup, row: &gtk::Widget, text: &str| {
            g.add(row);
            entries.borrow_mut().push(PrefEntry {
                cat,
                group: gi,
                widget: row.clone(),
                text: text.to_lowercase(),
            });
        }
    };

    // ───── Ansicht
    let (gi, look) = new_group(0, tr("Darstellung"), None);
    let scheme_row = adw::ComboRow::builder()
        .title(tr("Farbschema"))
        .model(&gtk::StringList::new(
            &Scheme::ALL.iter().map(|s| s.label()).collect::<Vec<_>>(),
        ))
        .build();
    scheme_row.set_selected(
        Scheme::ALL
            .iter()
            .position(|s| *s == ui.scheme.get())
            .unwrap_or(0) as u32,
    );
    add_row(
        0,
        gi,
        &look,
        scheme_row.upcast_ref(),
        tr("farbschema system hell dunkel darstellung theme design"),
    );
    // Sprache (Sprachnamen in ihrer eigenen Sprache)
    const LANGS: [&str; 3] = ["system", "de", "en_GB"];
    let lang_row = adw::ComboRow::builder()
        .title(tr("Sprache"))
        .model(&gtk::StringList::new(&[tr("System"), "Deutsch", "English (UK)"]))
        .build();
    lang_row.set_selected(
        LANGS
            .iter()
            .position(|l| *l == ui.language.borrow().as_str())
            .unwrap_or(0) as u32,
    );
    add_row(
        0,
        gi,
        &look,
        lang_row.upcast_ref(),
        "sprache language deutsch english englisch",
    );
    {
        let u = ui.clone();
        lang_row.connect_selected_notify(move |r| {
            let l = LANGS[(r.selected() as usize).min(LANGS.len() - 1)];
            *u.language.borrow_mut() = l.to_string();
            save_settings(&u);
            u.toast(tr("Die Sprache wechselt beim nächsten Start von Stijl"));
        });
    }
    let backdrop_row = adw::ComboRow::builder()
        .title(tr("Hintergrund"))
        .model(&gtk::StringList::new(
            &render::Backdrop::ALL
                .iter()
                .map(|b| b.label())
                .collect::<Vec<_>>(),
        ))
        .build();
    backdrop_row.set_selected(
        render::Backdrop::ALL
            .iter()
            .position(|b| *b == ui.backdrop.get())
            .unwrap_or(0) as u32,
    );
    add_row(
        0,
        gi,
        &look,
        backdrop_row.upcast_ref(),
        tr("hintergrund standard dunkel schachbrett transparent arbeitsbereich"),
    );
    let view_row = adw::ComboRow::builder()
        .title(tr("Ansicht"))
        .tooltip_text(tr("Notizheft: eine Seite nach der anderen, Scrollen blättert um"))
        .model(&gtk::StringList::new(&[tr("Zeichenfläche"), tr("Notizheft")]))
        .build();
    view_row.set_selected(ui.notebook.get() as u32);
    add_row(
        0,
        gi,
        &look,
        view_row.upcast_ref(),
        tr("ansicht zeichenfläche notizheft seiten blättern seite canva"),
    );
    let rulers_row = adw::SwitchRow::builder()
        .title(tr("Lineale"))
        .tooltip_text(tr("Ein Klick in die Ecke der Lineale wechselt die Einheit"))
        .active(ui.show_rulers.get())
        .build();
    add_row(
        0,
        gi,
        &look,
        rulers_row.upcast_ref(),
        tr("lineale oben links einheit"),
    );

    let (gi, units) = new_group(0, tr("Maßeinheit"), None);
    let unit_row = adw::ComboRow::builder()
        .title(tr("Einheit"))
        .tooltip_text(tr(
            "Voreinstellung; jedes Dokument merkt sich seine eigene Einheit",
        ))
        .model(&gtk::StringList::new(
            &Unit::ALL.iter().map(|u| u.label()).collect::<Vec<_>>(),
        ))
        .build();
    unit_row.set_selected(Unit::ALL.iter().position(|u| *u == ui.unit.get()).unwrap_or(0) as u32);
    add_row(
        0,
        gi,
        &units,
        unit_row.upcast_ref(),
        tr("maßeinheit einheit pixel px millimeter mm zentimeter cm zoll inch in punkt pt metrisch imperial"),
    );

    // Updates (nicht unter Flatpak/Snap – dort kümmert sich die Paketverwaltung)
    if !crate::update::managed_externally() {
        let (gi, upd) = new_group(0, tr("Updates"), None);
        let upd_row = adw::SwitchRow::builder()
            .title(tr("Nach neuen Versionen suchen"))
            .tooltip_text(tr("Einmal am Tag bei GitHub nachsehen"))
            .active(ui.updates.get())
            .build();
        let now_btn = gtk::Button::builder()
            .label(tr("Jetzt suchen"))
            .valign(gtk::Align::Center)
            .build();
        now_btn.add_css_class("flat");
        upd_row.add_suffix(&now_btn);
        add_row(
            0,
            gi,
            &upd,
            upd_row.upcast_ref(),
            tr("updates aktualisierung neue version github"),
        );
        let u = ui.clone();
        upd_row.connect_active_notify(move |r| {
            u.updates.set(r.is_active());
            save_settings(&u);
        });
        let u = ui.clone();
        now_btn.connect_clicked(move |_| check_for_updates(&u, true));
    }

    // ───── Ausrichten
    let (gi, snapg) = new_group(1, tr("Einrasten an"), None);
    let switch = |title: &str, on: bool, text: &str| -> adw::SwitchRow {
        let r = adw::SwitchRow::builder().title(title).active(on).build();
        add_row(1, gi, &snapg, r.upcast_ref(), &trf("einrasten {}", &[&(text)]));
        r
    };
    let guide_snap_row = switch(tr("Hilfslinien"), ui.snap_guides.get(), "hilfslinien");
    let corners_row = switch(tr("Ecken"), ui.snap_corners.get(), tr("ecken objekte punkte"));
    let mids_row = switch(
        tr("Mitten"),
        ui.snap_mids.get(),
        tr("mitten mitte kantenmitten objekte punkte"),
    );
    let anchors_row = switch(
        tr("Ankerpunkte"),
        ui.snap_anchors.get(),
        tr("ankerpunkte pfade punkte"),
    );
    let page_row = switch(tr("Seite"), ui.snap_page.get(), tr("seite zeichenfläche rand"));
    let paths_row = switch(tr("Pfade"), ui.snap_paths.get(), tr("pfade konturen linien"));
    let spacing_row = switch(
        tr("Abstände"),
        ui.snap_spacing.get(),
        tr("abstände gleiche lücken reihe"),
    );
    let grid_row = switch(tr("Raster"), ui.grid_snap.get(), tr("raster gitter"));
    let snap_rows = vec![
        guide_snap_row.clone(),
        corners_row.clone(),
        mids_row.clone(),
        anchors_row.clone(),
        page_row.clone(),
        paths_row.clone(),
        spacing_row.clone(),
        grid_row.clone(),
    ];
    {
        let all_on = gtk::Button::with_label(tr("Alle an"));
        let all_off = gtk::Button::with_label(tr("Alle aus"));
        let bx = hbox(0);
        bx.add_css_class("linked");
        bx.append(&all_on);
        bx.append(&all_off);
        snapg.set_header_suffix(Some(&bx));
        for (btn, on) in [(all_on, true), (all_off, false)] {
            let rows = snap_rows.clone();
            btn.connect_clicked(move |_| {
                for r in &rows {
                    r.set_active(on);
                }
            });
        }
    }
    let grid_size_row = adw::SpinRow::with_range(1.0, 1000.0, 1.0);
    grid_size_row.set_title(tr("Rasterweite (px)"));
    grid_size_row.set_value(ui.grid_size.get());
    grid_size_row.set_sensitive(ui.grid_snap.get());
    add_row(
        1,
        gi,
        &snapg,
        grid_size_row.upcast_ref(),
        tr("rasterweite raster gitter px"),
    );
    let (gi, frameg) = new_group(1, tr("Einsetzen in Formen"), None);
    let dwell_row = adw::SpinRow::with_range(0.1, 5.0, 0.1);
    dwell_row.set_digits(1);
    dwell_row.set_title(tr("Wartezeit (s)"));
    dwell_row.set_tooltip_text(Some(tr(
        "So lange ein Bild oder Textfeld über einer Form gehalten werden muss, bis es eingesetzt wird",
    )));
    dwell_row.set_value(ui.frame_dwell.get());
    add_row(
        1,
        gi,
        &frameg,
        dwell_row.upcast_ref(),
        tr("wartezeit einsetzen formen bild textfeld halten rahmen"),
    );

    // ───── Shortcuts
    let (gi, reset_g) = new_group(2, "", None);
    let reset_all = gtk::Button::with_label(tr("Alle zurücksetzen"));
    reset_all.set_valign(gtk::Align::Center);
    let reset_row = adw::ActionRow::builder().title(tr("Standardbelegung")).build();
    reset_row.add_suffix(&reset_all);
    reset_all.set_sensitive(KEYMAP.with(|k| k.borrow().any_custom()));
    add_row(
        2,
        gi,
        &reset_g,
        reset_row.upcast_ref(),
        tr("standardbelegung alle zurücksetzen shortcuts tastenkürzel tastenkombination"),
    );
    // je Zeile: (Kennung, Beschriftung der Kürzel, Zurücksetzen-Knopf)
    type ShortcutRow = (&'static str, gtk::Label, gtk::Button);
    let rows: Rc<RefCell<Vec<ShortcutRow>>> = Rc::new(RefCell::new(Vec::new()));
    let shortcut_groups: Vec<(usize, adw::PreferencesGroup, &'static str)> = crate::keys::GROUPS
        .iter()
        .map(|name| {
            let (gi, g) = new_group(2, tr(name), None);
            (gi, g, *name)
        })
        .collect();
    let dialog = adw::Dialog::new();
    // Beschriftung und Knöpfe aller Kürzel-Zeilen neu setzen
    let refresh_rows: Rc<dyn Fn()> = {
        let rows = rows.clone();
        let reset_all = reset_all.clone();
        Rc::new(move || {
            KEYMAP.with(|k| {
                let k = k.borrow();
                for (id, label, reset) in rows.borrow().iter() {
                    let keys = k.keys(id);
                    label.set_label(&if keys.is_empty() {
                        tr("Nicht belegt").to_string()
                    } else {
                        keys.iter()
                            .map(|a| crate::keys::label(a))
                            .collect::<Vec<_>>()
                            .join(", ")
                    });
                    reset.set_visible(k.is_custom(id));
                }
                reset_all.set_sensitive(k.any_custom());
            });
        })
    };
    for (gi, g, name) in &shortcut_groups {
        for d in crate::keys::DEFS.iter().filter(|d| d.group == *name) {
            let row = adw::ActionRow::builder().title(tr(d.label)).build();
            let label = gtk::Label::new(None);
            label.add_css_class("dim-label");
            label.set_valign(gtk::Align::Center);
            row.add_suffix(&label);
            let reset = gtk::Button::from_icon_name("edit-undo-symbolic");
            reset.add_css_class("flat");
            reset.set_valign(gtk::Align::Center);
            reset.set_tooltip_text(Some(tr("Auf den Standard zurücksetzen")));
            reset.set_visible(false);
            if d.kind != crate::keys::Kind::Fixed {
                let edit = gtk::Button::from_icon_name("document-edit-symbolic");
                edit.add_css_class("flat");
                edit.set_valign(gtk::Align::Center);
                edit.set_tooltip_text(Some(tr("Tastenkombination ändern")));
                row.add_suffix(&reset);
                row.add_suffix(&edit);
                row.set_activatable_widget(Some(&edit));
                {
                    let (u, dlg, done, id) = (ui.clone(), dialog.clone(), refresh_rows.clone(), d.id);
                    edit.connect_clicked(move |_| capture_shortcut(&u, &dlg, id, done.clone()));
                }
                {
                    let (u, done, id) = (ui.clone(), refresh_rows.clone(), d.id);
                    reset.connect_clicked(move |_| {
                        KEYMAP.with(|k| k.borrow_mut().reset(id));
                        save_keymap();
                        if let Some(app) = u.window.application() {
                            apply_accels(&app);
                        }
                        done();
                    });
                }
            }
            rows.borrow_mut().push((d.id, label, reset));
            let keys_text = d
                .keys
                .iter()
                .map(|a| crate::keys::label(a))
                .collect::<Vec<_>>()
                .join(" ");
            add_row(
                2,
                *gi,
                g,
                row.upcast_ref(),
                &format!(
                    "{} {} {} {} {}",
                    d.label,
                    tr(d.label),
                    d.group,
                    tr(d.group),
                    keys_text
                ),
            );
        }
    }
    refresh_rows();
    {
        let (u, done) = (ui.clone(), refresh_rows.clone());
        reset_all.connect_clicked(move |_| {
            KEYMAP.with(|k| k.borrow_mut().reset_all());
            save_keymap();
            if let Some(app) = u.window.application() {
                apply_accels(&app);
            }
            done();
        });
    }

    // ───── Verhalten der Einstellungen
    {
        let u = ui.clone();
        scheme_row.connect_selected_notify(move |r| {
            let s = Scheme::ALL[(r.selected() as usize).min(Scheme::ALL.len() - 1)];
            u.scheme.set(s);
            s.apply();
            save_settings(&u);
            redraw_all(&u);
        });
    }
    {
        let u = ui.clone();
        backdrop_row.connect_selected_notify(move |r| {
            let b = render::Backdrop::ALL[(r.selected() as usize).min(render::Backdrop::ALL.len() - 1)];
            u.backdrop.set(b);
            save_settings(&u);
            redraw_all(&u);
        });
    }
    {
        let u = ui.clone();
        view_row.connect_selected_notify(move |r| {
            set_notebook_view(&u, r.selected() == 1, None);
        });
    }
    {
        let u = ui.clone();
        rulers_row.connect_active_notify(move |r| {
            u.show_rulers.set(r.is_active());
            update_osd_margins(&u);
            save_settings(&u);
            redraw_all(&u);
        });
    }
    {
        let u = ui.clone();
        unit_row.connect_selected_notify(move |r| {
            let unit = Unit::ALL[(r.selected() as usize).min(Unit::ALL.len() - 1)];
            u.unit.set(unit);
            // Auch das aktuelle Dokument umstellen (andere behalten ihre Einheit).
            if let Some(d) = cur(&u) {
                d.ed.borrow_mut().doc.unit = unit.name().into();
            }
            save_settings(&u);
            redraw_all(&u);
        });
    }
    {
        let u = ui.clone();
        guide_snap_row.connect_active_notify(move |r| {
            let v = r.is_active();
            u.snap_guides.set(v);
            for d in u.docs.borrow().iter() {
                d.ed.borrow_mut().snap_guides = v;
            }
            save_settings(&u);
            if cur(&u).is_some() {
                refresh(&u);
            }
        });
    }
    // Ziele an Objekten und Abstände: Zustand merken, Editoren aktualisieren
    let bind_snap = |row: &adw::SwitchRow, set: fn(&Ui, bool)| {
        let u = ui.clone();
        row.connect_active_notify(move |r| {
            set(&u, r.is_active());
            save_settings(&u);
            apply_snap_settings_all(&u);
        });
    };
    bind_snap(&corners_row, |u, v| u.snap_corners.set(v));
    bind_snap(&mids_row, |u, v| u.snap_mids.set(v));
    bind_snap(&anchors_row, |u, v| u.snap_anchors.set(v));
    bind_snap(&page_row, |u, v| u.snap_page.set(v));
    bind_snap(&paths_row, |u, v| u.snap_paths.set(v));
    bind_snap(&spacing_row, |u, v| u.snap_spacing.set(v));
    {
        let u = ui.clone();
        let gs = grid_size_row.clone();
        grid_row.connect_active_notify(move |r| {
            u.grid_snap.set(r.is_active());
            gs.set_sensitive(r.is_active());
            save_settings(&u);
            apply_snap_settings_all(&u);
        });
    }
    {
        let u = ui.clone();
        grid_size_row.connect_value_notify(move |r| {
            u.grid_size.set(r.value().max(1.0));
            save_settings(&u);
            apply_snap_settings_all(&u);
        });
    }
    {
        let u = ui.clone();
        dwell_row.connect_value_notify(move |r| {
            u.frame_dwell.set(r.value().clamp(0.1, 5.0));
            save_settings(&u);
            apply_snap_settings_all(&u);
        });
    }

    // ───── Aufbau: graue Seitenleiste links, Inhalt rechts, Suche in der Kopfzeile
    let page_box = vbox(18);
    for (_, g) in groups.borrow().iter() {
        page_box.append(g);
    }
    let clamp = adw::Clamp::builder().maximum_size(640).child(&page_box).build();
    clamp.set_margin_top(12);
    clamp.set_margin_bottom(24);
    clamp.set_margin_start(12);
    clamp.set_margin_end(12);
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&clamp)
        .build();
    let content_title = adw::WindowTitle::new(tr(CATS[0].0), "");
    let content_header = adw::HeaderBar::new();
    content_header.set_title_widget(Some(&content_title));
    let content_view = adw::ToolbarView::new();
    content_view.add_top_bar(&content_header);
    content_view.set_content(Some(&scroll));
    let content_page = adw::NavigationPage::builder()
        .title(tr(CATS[0].0))
        .child(&content_view)
        .build();

    let side_list = gtk::ListBox::new();
    side_list.add_css_class("navigation-sidebar");
    side_list.set_selection_mode(gtk::SelectionMode::Single);
    for (name, icon) in CATS {
        let r = hbox(10);
        r.set_margin_top(8);
        r.set_margin_bottom(8);
        r.set_margin_start(6);
        r.set_margin_end(6);
        r.append(&gtk::Image::from_icon_name(icon));
        r.append(&gtk::Label::new(Some(tr(name))));
        side_list.append(&r);
    }
    let search_entry = gtk::SearchEntry::new();
    search_entry.set_placeholder_text(Some(tr("Einstellung suchen")));
    search_entry.set_hexpand(true);
    let search_bar = gtk::SearchBar::new();
    search_bar.set_child(Some(&search_entry));
    search_bar.connect_entry(&search_entry);
    let search_toggle = gtk::ToggleButton::new();
    search_toggle.set_icon_name("system-search-symbolic");
    search_toggle.set_tooltip_text(Some(tr("Einstellung suchen")));
    search_toggle
        .bind_property("active", &search_bar, "search-mode-enabled")
        .bidirectional()
        .sync_create()
        .build();
    let side_header = adw::HeaderBar::new();
    side_header.pack_start(&search_toggle);
    side_header.set_title_widget(Some(&adw::WindowTitle::new(tr("Einstellungen"), "")));
    let side_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&side_list)
        .build();
    let side_view = adw::ToolbarView::new();
    side_view.add_top_bar(&side_header);
    side_view.add_top_bar(&search_bar);
    side_view.set_content(Some(&side_scroll));
    let side_page = adw::NavigationPage::builder()
        .title(tr("Einstellungen"))
        .child(&side_view)
        .build();

    let split = adw::NavigationSplitView::new();
    split.set_sidebar(Some(&side_page));
    split.set_content(Some(&content_page));
    split.set_min_sidebar_width(200.0);
    split.set_max_sidebar_width(260.0);
    split.set_sidebar_width_fraction(0.3);

    // zeigt die gewählte Kategorie bzw. beim Suchen alle passenden Einträge
    let cat = Rc::new(Cell::new(0usize));
    let refilter: Rc<dyn Fn()> = {
        let (entries, groups, cat) = (entries.clone(), groups.clone(), cat.clone());
        let (entry, title, page) = (search_entry.clone(), content_title.clone(), content_page.clone());
        Rc::new(move || {
            let q = entry.text().trim().to_lowercase();
            let searching = !q.is_empty();
            let es = entries.borrow();
            for e in es.iter() {
                e.widget.set_visible(if searching {
                    q.split_whitespace().all(|w| e.text.contains(w))
                } else {
                    e.cat == cat.get()
                });
            }
            for (gi, (gcat, g)) in groups.borrow().iter().enumerate() {
                g.set_visible(if searching {
                    es.iter().any(|e| e.group == gi && e.widget.is_visible())
                } else {
                    *gcat == cat.get()
                });
            }
            let name = if searching {
                tr("Suchergebnisse")
            } else {
                tr(CATS[cat.get()].0)
            };
            title.set_title(name);
            page.set_title(name);
        })
    };
    refilter();
    side_list.select_row(side_list.row_at_index(start.min(CATS.len() - 1) as i32).as_ref());
    {
        let (cat, refilter, split, entry, toggle) = (
            cat.clone(),
            refilter.clone(),
            split.clone(),
            search_entry.clone(),
            search_toggle.clone(),
        );
        side_list.connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
            cat.set((row.index() as usize).min(CATS.len() - 1));
            // eine Kategorie zu wählen beendet die Suche
            if !entry.text().is_empty() {
                entry.set_text("");
            }
            toggle.set_active(false);
            refilter();
            split.set_show_content(true);
        });
    }
    {
        let refilter = refilter.clone();
        let split = split.clone();
        search_entry.connect_search_changed(move |e| {
            refilter();
            if !e.text().is_empty() {
                split.set_show_content(true);
            }
        });
    }
    {
        // Esc im Suchfeld schließt die Suche
        let t = search_toggle.clone();
        search_entry.connect_stop_search(move |_| t.set_active(false));
    }

    dialog.set_title(tr("Einstellungen"));
    dialog.set_content_width(860);
    dialog.set_content_height(620);
    dialog.set_child(Some(&split));
    label_icon_buttons(dialog.upcast_ref());
    dialog.present(Some(&ui.window));
}

/// Bilddatei lesen und ins Dokument einfügen.
fn insert_image_file(ui: &Rc<Ui>, d: &Rc<DocTab>, path: &Path, at: Option<Pt>) -> bool {
    let data = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            ui.toast(&trf("Bild konnte nicht gelesen werden: {}", &[&(e)]));
            return false;
        }
    };
    let Some((w, h)) = render::image_size(&data) else {
        ui.toast(tr("Dieses Bildformat wird nicht unterstützt"));
        return false;
    };
    let ok = d.ed.borrow_mut().insert_image(Rc::new(data), w, h, at);
    refresh(ui);
    ok
}

fn insert_image_dialog(ui: &Rc<Ui>) {
    let Some(d) = cur(ui) else { return };
    let dialog = gtk::FileDialog::builder()
        .title(tr("Bild einfügen"))
        .modal(true)
        .build();
    let f = gtk::FileFilter::new();
    f.set_name(Some(tr("Bilder")));
    f.add_pixbuf_formats();
    let store = gio::ListStore::new::<gtk::FileFilter>();
    store.append(&f);
    dialog.set_filters(Some(&store));
    let u = ui.clone();
    dialog.open(Some(&ui.window), gio::Cancellable::NONE, move |res| {
        if let Some(p) = res.ok().and_then(|f| f.path()) {
            insert_image_file(&u, &d, &p, None);
        }
    });
}

/// Liste „Zuletzt geöffnet“ einer Startseite neu aufbauen.
fn fill_recent(ui: &Rc<Ui>, recent_list: &gtk::FlowBox, recent_box: &gtk::Box) {
    while let Some(c) = recent_list.first_child() {
        recent_list.remove(&c);
    }
    let list: Vec<PathBuf> = load_recent()
        .into_iter()
        .filter(|p| p.exists())
        .take(RECENT_CARDS)
        .collect();
    let dark = adw::StyleManager::default().is_dark();
    for path in list {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let doc = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| svg::read(&s).ok())
            .map(Rc::new);
        let thumb = gtk::DrawingArea::new();
        thumb.set_content_width(CARD_PREVIEW_W);
        thumb.set_content_height(CARD_PREVIEW_H);
        {
            let doc = doc.clone();
            thumb.set_draw_func(move |_, cr, w, h| {
                if let Some(doc) = &doc {
                    render::draw_document_thumbnail(cr, w as f64, h as f64, doc, dark);
                }
            });
        }
        // darunter, wann zuletzt geändert (wie bei den Formaten die Größe)
        let when = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|d| glib::DateTime::from_unix_local(d.as_secs() as i64).ok())
            .and_then(|t| t.format("%x").ok())
            .map(|s| s.to_string())
            .unwrap_or_default();
        let b = card_content(&thumb, &name, &when);
        let card = gtk::Button::new();
        card.set_child(Some(&b));
        card.add_css_class("card");
        card.set_tooltip_text(Some(&format!("{name} – {}", pretty_dir(&path))));
        {
            let (u, p) = (ui.clone(), path.clone());
            card.connect_clicked(move |_| open_path(&u, &p));
        }
        // kleines × oben rechts: aus der Liste entfernen
        let remove = gtk::Button::from_icon_name("window-close-symbolic");
        remove.add_css_class("flat");
        remove.add_css_class("circular");
        remove.set_halign(gtk::Align::End);
        remove.set_valign(gtk::Align::Start);
        remove.set_margin_top(4);
        remove.set_margin_end(4);
        remove.set_tooltip_text(Some(tr("Aus der Liste entfernen")));
        {
            let (u, p) = (ui.clone(), path.clone());
            remove.connect_clicked(move |_| {
                remove_recent(&p);
                let u2 = u.clone();
                glib::idle_add_local_once(move || refresh_starts(&u2));
            });
        }
        let ov = gtk::Overlay::new();
        ov.set_child(Some(&card));
        ov.add_overlay(&remove);
        ov.set_valign(gtk::Align::Start);
        recent_list.insert(&ov, -1);
    }
    // „Öffnen …“ als gestrichelte Kachel in Größe der Karten am Ende der Reihe
    let open = dashed_tile("folder-open-symbolic", tr("Öffnen …"), None);
    open.set_action_name(Some("win.open"));
    open.set_tooltip_text(Some(&key_tip(tr("Dokument öffnen"), "open")));
    recent_list.insert(&open, -1);
    fit_recent_row(recent_list);
    recent_box.set_visible(true);
}

/// Alle Startseiten auffrischen.
fn refresh_starts(ui: &Rc<Ui>) {
    let lists: Vec<(gtk::FlowBox, gtk::Box)> = ui
        .starts
        .borrow()
        .iter()
        .map(|st| (st.recent_list.clone(), st.recent_box.clone()))
        .collect();
    for (l, b) in lists {
        fill_recent(ui, &l, &b);
    }
}

// ───────────────────────────── Bibliothek ─────────────────────────────

/// Bedienelemente der Bibliotheksleiste (links neben der Zeichenfläche).
struct LibWidgets {
    toggle: gtk::ToggleButton,
    /// die ganze Leiste (sichtbar, solange der Knopf gedrückt ist)
    panel: gtk::Box,
    dropdown: gtk::DropDown,
    colors: gtk::Grid,
    colors_empty: gtk::Label,
    styles: gtk::Grid,
    styles_empty: gtk::Label,
    texts: gtk::Grid,
    texts_empty: gtk::Label,
    /// Grafiken (gespeicherte Objekte)
    graphics: gtk::Grid,
    graphics_head: gtk::Box,
    /// Felder je Abschnitt (Farben, Stile, Textstile, Grafiken), je nach Breite umbrochen
    cells: RefCell<[Vec<gtk::Widget>; 4]>,
    /// aktuelle Spaltenzahl je Abschnitt
    cols: Cell<[i32; 4]>,
    /// Kopf und Hinweis des Abschnitts „Stile“
    styles_head: gtk::Box,
    /// Hinweis, dass die Standardbibliothek nicht änderbar ist
    locked_note: gtk::Label,
}

/// Bibliotheken liegen sichtbar unter „Dokumente/Stijl/Bibliotheken“ – so lassen sie
/// sich kopieren, sichern oder mit anderen Rechnern abgleichen (z. B. Nextcloud).
fn libs_dir() -> PathBuf {
    let base = glib::user_special_dir(glib::UserDirectory::Documents)
        .unwrap_or_else(glib::home_dir)
        .join("Stijl");
    // vorhandenen Ordner behalten, auch wenn inzwischen die Sprache gewechselt wurde
    for name in ["Bibliotheken", "Libraries"] {
        if base.join(name).is_dir() {
            return base.join(name);
        }
    }
    base.join(tr("Bibliotheken"))
}

/// Einmalig: Bibliotheken aus den früheren (versteckten) Ordnern übernehmen.
fn migrate_libraries() {
    let dir = libs_dir();
    let has_new = std::fs::read_dir(&dir)
        .map(|mut r| r.next().is_some())
        .unwrap_or(false);
    if has_new {
        return;
    }
    for old in [
        glib::user_config_dir().join("stijl").join("bibliotheken"),
        glib::user_config_dir().join("mondrian").join("bibliotheken"),
        glib::user_config_dir().join("silhouettes").join("bibliotheken"),
    ] {
        let Ok(rd) = std::fs::read_dir(&old) else { continue };
        let files: Vec<PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map(|x| x == "svg").unwrap_or(false))
            .collect();
        if files.is_empty() {
            continue;
        }
        let _ = std::fs::create_dir_all(&dir);
        for f in files {
            if let Some(name) = f.file_name() {
                let _ = std::fs::copy(&f, dir.join(name));
            }
        }
        return;
    }
}

/// Einmalig beim Start: Ordner aus der Zeit übernehmen, als das Programm „Mondrian“ hieß
/// (Einstellungen, Tastenkürzel, zuletzt geöffnete Dateien, Wiederherstellung, Bibliotheken).
/// Vorhandene Stijl-Ordner werden nie überschrieben.
pub fn migrate_from_mondrian() {
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| glib::home_dir().join(".local").join("state"));
    for base in [
        glib::user_config_dir(),
        glib::user_data_dir(),
        glib::user_cache_dir(),
        state,
    ] {
        let (old, new) = (base.join("mondrian"), base.join("stijl"));
        if old.is_dir() && !new.exists() {
            let _ = std::fs::rename(&old, &new);
        }
    }
    // sichtbare Bibliotheken: Dokumente/Mondrian/… → Dokumente/Stijl/…
    let docs = glib::user_special_dir(glib::UserDirectory::Documents).unwrap_or_else(glib::home_dir);
    let (old, new) = (docs.join("Mondrian"), docs.join("Stijl"));
    let Ok(rd) = std::fs::read_dir(&old) else { return };
    for entry in rd.flatten() {
        let target = new.join(entry.file_name());
        if target.exists() {
            continue;
        }
        let _ = std::fs::create_dir_all(&new);
        let _ = std::fs::rename(entry.path(), &target);
    }
    // nur entfernen, wenn wirklich leer
    let _ = std::fs::remove_dir(&old);
}

/// Alle gespeicherten Bibliotheken (mindestens eine).
fn load_libraries() -> Vec<Library> {
    migrate_libraries();
    let mut libs: Vec<Library> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(libs_dir()) {
        let mut paths: Vec<PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map(|x| x == "svg").unwrap_or(false))
            .collect();
        paths.sort();
        for p in paths {
            let name = p
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if let Ok(text) = std::fs::read_to_string(&p) {
                if let Ok(l) = Library::from_svg(&name, &text) {
                    libs.push(l);
                }
            }
        }
    }
    for l in libs.iter_mut() {
        // eigene Datei mit dem Namen der Standardbibliothek
        if l.name == library::BUILTIN_NAME {
            l.name = trf("{} (eigene)", &[&(library::BUILTIN_NAME)]);
        }
    }
    if libs.is_empty() {
        libs.push(Library::new(tr("Meine Bibliothek")));
    }
    // Standardbibliothek immer zuerst
    libs.insert(0, library::builtin());
    libs
}

fn save_library(lib: &Library) {
    if lib.locked {
        return;
    }
    let dir = libs_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = crate::fsutil::write_atomic(&dir.join(library::file_name(&lib.name)), lib.to_svg().as_bytes());
}

fn build_library_panel() -> LibWidgets {
    let toggle = gtk::ToggleButton::builder()
        .icon_name("mondrian-library-symbolic")
        .tooltip_text(tr("Bibliothek – Farben und Stile sammeln und übernehmen"))
        .build();
    toggle.add_css_class("flat");
    toggle.set_margin_start(6);
    toggle.set_margin_end(6);
    toggle.set_margin_bottom(8);

    let title = gtk::Label::builder()
        .label(tr("Bibliothek"))
        .xalign(0.0)
        .hexpand(true)
        .build();
    title.add_css_class("mondrian-panel-title");
    let menu = gio::Menu::new();
    let m1 = gio::Menu::new();
    m1.append(Some(tr("Neue Bibliothek")), Some("win.lib-new"));
    m1.append(Some(tr("Umbenennen …")), Some("win.lib-rename"));
    m1.append(Some(tr("Löschen …")), Some("win.lib-delete"));
    menu.append_section(None, &m1);
    let m2 = gio::Menu::new();
    m2.append(Some(tr("Importieren …")), Some("win.lib-import"));
    m2.append(Some(tr("Exportieren …")), Some("win.lib-export"));
    m2.append(Some(tr("Ordner öffnen")), Some("win.lib-folder"));
    m2.append(Some(tr("Neu einlesen")), Some("win.lib-reload"));
    menu.append_section(None, &m2);
    let more = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .menu_model(&menu)
        .tooltip_text(tr("Bibliotheken verwalten"))
        .build();
    more.add_css_class("flat");
    let head = hbox(6);
    head.append(&title);
    head.append(&more);

    let dropdown = gtk::DropDown::from_strings(&[]);
    dropdown.set_tooltip_text(Some(tr("Bibliothek wählen")));

    // Abschnitte als aufklappbare Zeilen einer gemeinsamen Karte (wie Füllung und Kontur in
    // den Eigenschaften); anders als dort können mehrere zugleich offen sein, anfangs alle
    let section = |label: &str| {
        let card = rows_box();
        let title = gtk::Label::new(Some(label));
        title.add_css_class("heading");
        let hint = gtk::Label::new(None);
        let arrow = gtk::Button::from_icon_name("pan-down-symbolic");
        let head = fold_head(&title, &hint, None, &arrow);
        let body = vbox(8);
        body.add_css_class("mondrian-pad");
        card.append(&head);
        card.append(&body);
        let toggle = {
            let (body, arrow) = (body.clone(), arrow.clone());
            move || {
                let open = !body.property::<bool>("visible");
                body.set_visible(open);
                arrow.set_icon_name(if open {
                    "pan-down-symbolic"
                } else {
                    "pan-end-symbolic"
                });
            }
        };
        let t2 = toggle.clone();
        arrow.connect_clicked(move |_| t2());
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| toggle());
        title.add_controller(click);
        (card, body)
    };
    let grid = || {
        gtk::Grid::builder()
            .column_spacing(LIB_SPACING)
            .row_spacing(LIB_SPACING)
            .halign(gtk::Align::Start)
            .build()
    };
    let empty = |text: &str| {
        let l = gtk::Label::builder().label(text).xalign(0.0).wrap(true).build();
        l.add_css_class("dim-label");
        l.add_css_class("caption");
        l
    };
    let colors = grid();
    let colors_empty = empty(tr("Noch keine Farben"));
    let styles = grid();
    let styles_empty = empty(tr("Noch keine Stile"));
    let texts = grid();
    let texts_empty = empty(tr("Noch keine Textstile"));

    let inner = vbox(12);
    inner.set_margin_start(12);
    inner.set_margin_end(12);
    inner.set_margin_top(4);
    inner.set_margin_bottom(12);
    let locked_note = empty(tr(
        "Die Standardbibliothek „Stijl“ lässt sich nicht ändern. Für eigene Farben und \
         Stile über ⋮ eine neue Bibliothek anlegen.",
    ));
    inner.append(&locked_note);
    let lib_card = card();
    inner.append(&lib_card);
    let (colors_card, colors_body) = section(tr("Farben"));
    colors_body.append(&colors);
    colors_body.append(&colors_empty);
    lib_card.append(&colors_card);
    // (Kopf = ganzer Abschnitt: wird bei der Standardbibliothek ohne Stile ausgeblendet)
    let (styles_head, styles_body) = section(tr("Stile"));
    styles_body.append(&styles);
    styles_body.append(&styles_empty);
    lib_card.append(&styles_head);
    let (texts_card, texts_body) = section(tr("Textstile"));
    texts_body.append(&texts);
    texts_body.append(&texts_empty);
    lib_card.append(&texts_card);
    let graphics = grid();
    let (graphics_head, graphics_body) = section(tr("Grafiken"));
    graphics_body.append(&graphics);
    lib_card.append(&graphics_head);
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&inner)
        .build();

    let top = vbox(8);
    top.set_margin_start(12);
    top.set_margin_end(12);
    top.set_margin_top(12);
    top.append(&head);
    top.append(&dropdown);

    let col = vbox(8);
    col.set_size_request(200, -1);
    col.set_hexpand(true);
    col.append(&top);
    col.append(&scroll);
    let panel = hbox(0);
    panel.set_hexpand(true);
    panel.append(&col);
    LibWidgets {
        toggle,
        panel,
        dropdown,
        colors,
        colors_empty,
        styles,
        styles_empty,
        texts,
        texts_empty,
        graphics,
        graphics_head,
        cells: RefCell::new([Vec::new(), Vec::new(), Vec::new(), Vec::new()]),
        cols: Cell::new([0; 4]),
        styles_head,
        locked_note,
    }
}

/// Inhalt eines Zieh-Vorgangs aus der Bibliothek („mondrian-lib-color:3“ …).
fn lib_payload(ui: &Ui, s: &str) -> Option<crate::editor::LibPayload> {
    let libs = ui.libraries.borrow();
    let l = libs.get(ui.lib_current.get())?;
    if let Some(i) = s.strip_prefix("mondrian-lib-color:") {
        let i = i.parse::<usize>().ok()?;
        return l
            .colors
            .get(i)
            .map(|c| crate::editor::LibPayload::Color(*c, l.gradient(i).cloned()));
    }
    if let Some(i) = s.strip_prefix("mondrian-lib-graphic:") {
        return l
            .graphics
            .get(i.parse::<usize>().ok()?)
            .map(|g| crate::editor::LibPayload::Graphic(g.objects.clone()));
    }
    if let Some(i) = s.strip_prefix("mondrian-lib-style:") {
        return l
            .styles
            .get(i.parse::<usize>().ok()?)
            .map(|st| crate::editor::LibPayload::Style(st.clone()));
    }
    None
}

/// Feld der Bibliothek ziehbar machen (auf ein Objekt in der Zeichenfläche).
fn lib_drag_source(w: &gtk::DrawingArea, key: String) {
    let src = gtk::DragSource::new();
    src.set_actions(gdk::DragAction::COPY);
    src.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&key.to_value())));
    {
        let w2 = w.clone();
        src.connect_drag_begin(move |s, _| {
            // kleines Abbild des Feldes am Zeiger
            let paintable = gtk::WidgetPaintable::new(Some(&w2));
            s.set_icon(Some(&paintable), 12, 12);
        });
    }
    w.add_controller(src);
}

/// Kleines Kontextmenü an einem Feld der Bibliothek.
fn lib_popover(parent: &impl IsA<gtk::Widget>, menu: &gio::Menu) {
    let pop = gtk::PopoverMenu::from_model(Some(menu));
    pop.set_parent(parent);
    pop.set_has_arrow(true);
    pop.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    pop.popup();
}

fn lib_item(label: &str, action: &str, idx: u32) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), None);
    item.set_action_and_target_value(Some(action), Some(&idx.to_variant()));
    item
}

/// Bibliotheksleiste neu aufbauen: Auswahlliste der Bibliotheken und Inhalt.
fn rebuild_library(ui: &Rc<Ui>) {
    let names: Vec<String> = ui.libraries.borrow().iter().map(|l| l.name.clone()).collect();
    let cur = ui.lib_current.get().min(names.len().saturating_sub(1));
    ui.lib_current.set(cur);
    guarded(ui, || {
        let names: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        ui.lib.dropdown.set_model(Some(&gtk::StringList::new(&names)));
        ui.lib.dropdown.set_selected(cur as u32);
    });
    rebuild_library_content(ui);
}

/// abgerundete Karte als Hintergrund einer Stilvorschau
fn preview_card(cr: &gtk::cairo::Context, w: f64, h: f64) {
    let dark = adw::StyleManager::default().is_dark();
    let r = 6.0;
    cr.new_sub_path();
    cr.arc(w - r, r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(w - r, h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(r, h - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.arc(r, r, r, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
    cr.close_path();
    if dark {
        cr.set_source_rgb(0.24, 0.24, 0.26);
    } else {
        cr.set_source_rgb(0.96, 0.96, 0.97);
    }
    let _ = cr.fill();
}

/// Größe der Felder in der Bibliothek (Pixel)
const LIB_SWATCH: i32 = 40;
/// Stile als Kreis wie die Farben
const LIB_STYLE_W: i32 = 40;
/// Vorschau einer Grafik
const LIB_GRAPHIC_W: i32 = 64;
const LIB_TEXT_W: i32 = 101;
const LIB_CARD_H: i32 = 48;
const LIB_SPACING: i32 = 6;

/// Feld mit optionalem Namen darunter.
fn lib_cell(widget: &impl IsA<gtk::Widget>, name: &str, width: i32) -> gtk::Widget {
    let cell = vbox(2);
    cell.set_valign(gtk::Align::Start);
    cell.append(widget);
    if !name.is_empty() {
        let l = gtk::Label::new(Some(name));
        l.add_css_class("caption");
        l.add_css_class("dim-label");
        l.set_ellipsize(pango::EllipsizeMode::End);
        l.set_max_width_chars(1);
        l.set_size_request(width, -1);
        l.set_xalign(0.5);
        cell.append(&l);
    }
    cell.upcast()
}

/// „+“ am Ende eines Abschnitts.
fn lib_add_cell(items: &[(&str, &str)], round: bool, w: i32, h: i32) -> gtk::Widget {
    let m = gio::Menu::new();
    for (text, action) in items {
        m.append(Some(text), Some(action));
    }
    let b = gtk::MenuButton::builder()
        .icon_name("list-add-symbolic")
        .menu_model(&m)
        .tooltip_text(tr("Hinzufügen"))
        .build();
    b.add_css_class("mondrian-lib-add");
    if round {
        b.add_css_class("mondrian-lib-add-round");
    }
    b.set_size_request(w, h);
    b.set_valign(gtk::Align::Start);
    b.set_halign(gtk::Align::Start);
    b.upcast()
}

/// Felder je nach Breite der Leiste auf Zeilen verteilen.
fn layout_library(ui: &Ui, force: bool) {
    let width = side_panel_width(ui);
    // Ränder links/rechts, Innenabstand der Karten und Trennlinie abziehen
    let avail = (width - 24 - 24 - 8).max(LIB_SWATCH);
    let w = &ui.lib;
    let cells = w.cells.borrow();
    let mut cols = w.cols.get();
    for (k, (grid, cw)) in [
        (&w.colors, LIB_SWATCH),
        (&w.styles, LIB_STYLE_W),
        (&w.texts, LIB_TEXT_W),
        (&w.graphics, LIB_GRAPHIC_W),
    ]
    .into_iter()
    .enumerate()
    {
        let n = ((avail + LIB_SPACING) / (cw + LIB_SPACING)).max(1);
        if !force && cols[k] == n {
            continue;
        }
        cols[k] = n;
        while let Some(c) = grid.first_child() {
            grid.remove(&c);
        }
        for (i, cell) in cells[k].iter().enumerate() {
            let i = i as i32;
            grid.attach(cell, i % n, i / n, 1, 1);
        }
    }
    w.cols.set(cols);
}

/// Farbfelder, Stile und Textstile der gewählten Bibliothek anzeigen.
/// Textstile der aktuellen Bibliothek in den Textoptionen (Klick übernimmt den Stil –
/// beim Bearbeiten nur für den markierten Text).
fn rebuild_text_styles(ui: &Rc<Ui>, styles: &[crate::library::LibStyle]) {
    let fb = &ui.panel.text_styles;
    while let Some(c) = fb.first_child() {
        fb.remove(&c);
    }
    for st in styles.iter().filter(|s| s.text.is_some()) {
        let area = gtk::DrawingArea::new();
        area.set_content_width(LIB_TEXT_W - 16);
        area.set_content_height(LIB_CARD_H - 8);
        area.set_cursor_from_name(Some("pointer"));
        if let Some(t) = &st.text {
            let name = if st.name.is_empty() {
                String::new()
            } else {
                format!("{}: ", st.name)
            };
            area.set_tooltip_text(Some(&trf(
                "{}{} {} px – Klick übernimmt den Textstil (beim Bearbeiten nur für den markierten Text)",
                &[&(name), &(t.family), &(fmt_num(t.size))],
            )));
        }
        let stc = st.clone();
        area.set_draw_func(move |_, cr, w, h| {
            let (w, h) = (w as f64, h as f64);
            preview_card(cr, w, h);
            render::draw_style_preview(cr, w, h, &stc);
        });
        let click = gtk::GestureClick::new();
        {
            let (u, st) = (ui.clone(), st.clone());
            click.connect_released(move |_, _, _, _| {
                with_ed_m(&u, |e, m| {
                    e.apply_lib_style(&st, m);
                });
            });
        }
        area.add_controller(click);
        fb.insert(&area, -1);
    }
    // „+“ am Ende: Textstil der Auswahl in die Bibliothek (wie bei den Farben)
    let add = gtk::Button::from_icon_name("list-add-symbolic");
    add.set_action_name(Some("win.lib-add-textstyle"));
    add.set_tooltip_text(Some(tr("Textstil der Auswahl zur Bibliothek hinzufügen")));
    add.set_size_request(LIB_TEXT_W - 16, LIB_CARD_H - 8);
    let cell = hbox(0);
    cell.add_css_class("mondrian-lib-add");
    cell.append(&add);
    fb.insert(&cell, -1);
    ui.panel.text_styles_box.set_visible(true);
}

fn rebuild_library_content(ui: &Rc<Ui>) {
    let graphics: Vec<crate::library::LibGraphic> = ui
        .libraries
        .borrow()
        .get(ui.lib_current.get())
        .map(|l| l.graphics.clone())
        .unwrap_or_default();
    let (colors, names, grads, styles, locked) = {
        let libs = ui.libraries.borrow();
        let cur = ui.lib_current.get().min(libs.len().saturating_sub(1));
        match libs.get(cur) {
            Some(l) => (
                l.colors.clone(),
                (0..l.colors.len())
                    .map(|i| l.color_name(i).to_string())
                    .collect::<Vec<_>>(),
                (0..l.colors.len())
                    .map(|i| l.gradient(i).cloned())
                    .collect::<Vec<_>>(),
                l.styles.clone(),
                l.locked,
            ),
            None => (Vec::new(), Vec::new(), Vec::new(), Vec::new(), false),
        }
    };
    rebuild_text_styles(ui, &styles);
    let w = &ui.lib;
    // Hinweise „Noch keine …“ braucht es nicht mehr: das „+“ steht am Ende
    w.colors_empty.set_visible(false);
    w.styles_empty.set_visible(false);
    w.texts_empty.set_visible(false);
    w.locked_note.set_visible(locked);
    // Standardbibliothek ohne Stile: Abschnitt weglassen
    w.styles_head
        .set_visible(!locked || styles.iter().any(|s| s.text.is_none()));
    let mut cells: [Vec<gtk::Widget>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    w.graphics_head.set_visible(!locked || !graphics.is_empty());
    for (i, c) in colors.iter().enumerate() {
        let c = *c;
        let name = names.get(i).cloned().unwrap_or_default();
        let grad: Option<Gradient> = grads.get(i).cloned().flatten();
        let sw = gtk::DrawingArea::new();
        sw.set_content_width(LIB_SWATCH);
        sw.set_content_height(LIB_SWATCH);
        sw.set_halign(gtk::Align::Center);
        sw.set_cursor_from_name(Some("pointer"));
        sw.set_tooltip_text(Some(&trf(
            "{}{} – Klick: Füllung, Umschalt-Klick: Kontur",
            &[
                &(if name.is_empty() {
                    String::new()
                } else {
                    format!("{name} · ")
                }),
                &(if grad.is_some() {
                    tr("Verlauf").to_string()
                } else {
                    c.to_hex()
                }),
            ],
        )));
        {
            let grad = grad.clone();
            sw.set_draw_func(move |_, cr, w, h| {
                draw_swatch(cr, w as f64, h as f64, Some(c), grad.as_ref(), false);
            });
        }
        let click = gtk::GestureClick::new();
        click.set_button(0);
        {
            let (u, swc) = (ui.clone(), sw.clone());
            click.connect_released(move |g, _, _, _| {
                if g.current_button() == gdk::BUTTON_SECONDARY {
                    let menu = gio::Menu::new();
                    menu.append_item(&lib_item(tr("Als Füllung"), "win.lib-color-fill", i as u32));
                    menu.append_item(&lib_item(tr("Als Kontur"), "win.lib-color-stroke", i as u32));
                    if !locked {
                        let sec = gio::Menu::new();
                        sec.append_item(&lib_item(tr("Benennen …"), "win.lib-color-name", i as u32));
                        sec.append_item(&lib_item(
                            tr("Aus der Bibliothek entfernen"),
                            "win.lib-color-remove",
                            i as u32,
                        ));
                        menu.append_section(None, &sec);
                    }
                    lib_popover(&swc, &menu);
                    return;
                }
                let stroke = g.current_event_state().contains(gdk::ModifierType::SHIFT_MASK);
                let grad = grad.clone();
                with_ed(&u, |e| {
                    e.apply_lib_color(c, grad, stroke);
                });
            });
        }
        sw.add_controller(click);
        lib_drag_source(&sw, format!("mondrian-lib-color:{i}"));
        cells[0].push(lib_cell(&sw, &name, LIB_SWATCH));
    }
    for (i, st) in styles.into_iter().enumerate() {
        let is_text = st.text.is_some();
        let name = st.name.clone();
        let cw = if is_text { LIB_TEXT_W } else { LIB_STYLE_W };
        let area = gtk::DrawingArea::new();
        area.set_content_width(cw);
        area.set_content_height(if is_text { LIB_CARD_H } else { LIB_STYLE_W });
        area.set_halign(gtk::Align::Center);
        area.set_cursor_from_name(Some("pointer"));
        let prefix = if name.is_empty() {
            String::new()
        } else {
            format!("{name}: ")
        };
        area.set_tooltip_text(Some(&match &st.text {
            Some(t) => trf(
                "{}{} {} px{}{} – Klick übernimmt den Textstil",
                &[
                    &(prefix),
                    &(t.family),
                    &(fmt_num(t.size)),
                    &(match t.fmt.weight_for(t.bold) {
                        400 => String::new(),
                        w => format!(", {}", crate::model::weight_name(w)),
                    }),
                    &(if t.italic { tr(", kursiv") } else { "" }),
                ],
            ),
            None => trf(
                "{}Stil – Klick übernimmt Füllung, Kontur, Deckkraft …",
                &[&(prefix)],
            ),
        }));
        let stc = st.clone();
        area.set_draw_func(move |_, cr, w, h| {
            let (w, h) = (w as f64, h as f64);
            // Textstile auf einer Karte, Stile als Kreis ohne Hintergrund
            if stc.text.is_some() {
                preview_card(cr, w, h);
            }
            render::draw_style_preview(cr, w, h, &stc);
        });
        let click = gtk::GestureClick::new();
        click.set_button(0);
        {
            let (u, ac) = (ui.clone(), area.clone());
            click.connect_released(move |g, _, _, _| {
                if g.current_button() == gdk::BUTTON_SECONDARY {
                    let menu = gio::Menu::new();
                    menu.append_item(&lib_item(tr("Übernehmen"), "win.lib-style-apply", i as u32));
                    if !locked {
                        let sec = gio::Menu::new();
                        sec.append_item(&lib_item(tr("Benennen …"), "win.lib-style-name", i as u32));
                        sec.append_item(&lib_item(
                            tr("Aus der Bibliothek entfernen"),
                            "win.lib-style-remove",
                            i as u32,
                        ));
                        menu.append_section(None, &sec);
                    }
                    lib_popover(&ac, &menu);
                    return;
                }
                let st = st.clone();
                with_ed_m(&u, |e, m| {
                    e.apply_lib_style(&st, m);
                });
            });
        }
        area.add_controller(click);
        lib_drag_source(&area, format!("mondrian-lib-style:{i}"));
        cells[if is_text { 2 } else { 1 }].push(lib_cell(&area, &name, cw));
    }
    if !locked {
        cells[0].push(lib_add_cell(
            &[
                (tr("Neue Farbe …"), "win.lib-new-color"),
                (tr("Füllfarbe der Auswahl"), "win.lib-add-fill"),
                (tr("Konturfarbe der Auswahl"), "win.lib-add-stroke"),
            ],
            true,
            LIB_SWATCH,
            LIB_SWATCH,
        ));
        cells[1].push(lib_add_cell(
            &[
                (tr("Neuer Stil …"), "win.lib-new-style"),
                (tr("Stil der Auswahl"), "win.lib-add-style"),
            ],
            true,
            LIB_STYLE_W,
            LIB_STYLE_W,
        ));
        cells[2].push(lib_add_cell(
            &[
                (tr("Neuer Textstil …"), "win.lib-new-textstyle"),
                (tr("Textstil der Auswahl"), "win.lib-add-textstyle"),
            ],
            false,
            LIB_TEXT_W,
            LIB_CARD_H,
        ));
    }
    // Grafiken: Vorschau; Klick fügt in der Mitte ein, Ziehen an die gewünschte Stelle
    for (i, g) in graphics.iter().enumerate() {
        let area = gtk::DrawingArea::new();
        area.set_content_width(LIB_GRAPHIC_W);
        area.set_content_height(LIB_GRAPHIC_W);
        area.set_cursor_from_name(Some("pointer"));
        area.set_tooltip_text(Some(&trf(
            "{} – Klick fügt ein, oder ins Dokument ziehen",
            &[&(g.name)],
        )));
        let objs = g.objects.clone();
        area.set_draw_func(move |_, cr, w, h| {
            let (w, h) = (w as f64, h as f64);
            preview_card(cr, w, h);
            render::draw_objects_preview(cr, w, h, &objs, 6.0);
        });
        let click = gtk::GestureClick::new();
        click.set_button(0);
        {
            let (u, ac) = (ui.clone(), area.clone());
            click.connect_released(move |g, _, _, _| {
                if g.current_button() == gdk::BUTTON_SECONDARY {
                    let menu = gio::Menu::new();
                    menu.append_item(&lib_item(tr("Einfügen"), "win.lib-graphic-insert", i as u32));
                    if !locked {
                        let sec = gio::Menu::new();
                        sec.append_item(&lib_item(tr("Benennen …"), "win.lib-graphic-name", i as u32));
                        sec.append_item(&lib_item(
                            tr("Aus der Bibliothek entfernen"),
                            "win.lib-graphic-remove",
                            i as u32,
                        ));
                        menu.append_section(None, &sec);
                    }
                    lib_popover(&ac, &menu);
                    return;
                }
                let _ = u
                    .window
                    .lookup_action("lib-graphic-insert")
                    .map(|a| a.activate(Some(&(i as u32).to_variant())));
            });
        }
        area.add_controller(click);
        lib_drag_source(&area, format!("mondrian-lib-graphic:{i}"));
        cells[3].push(lib_cell(&area, &g.name, LIB_GRAPHIC_W));
    }
    if !locked {
        cells[3].push(lib_add_cell(
            &[(tr("Auswahl als Grafik …"), "win.lib-add-graphic")],
            false,
            LIB_GRAPHIC_W,
            LIB_GRAPHIC_W,
        ));
    }
    *w.cells.borrow_mut() = cells;
    layout_library(ui, true);
    // Farbkreise der Eigenschaften zeigen die gewählte Bibliothek
    update_swatches(ui);
}

/// Aktuelle Bibliothek ändern, speichern und neu anzeigen.
fn with_library(ui: &Rc<Ui>, f: impl FnOnce(&mut Library) -> bool) -> bool {
    if lib_locked(ui) {
        ui.toast(tr("Die Standardbibliothek „Stijl“ lässt sich nicht ändern"));
        return false;
    }
    let changed = {
        let mut libs = ui.libraries.borrow_mut();
        let cur = ui.lib_current.get().min(libs.len().saturating_sub(1));
        let l = &mut libs[cur];
        let changed = f(l);
        if changed {
            save_library(l);
        }
        changed
    };
    if changed {
        rebuild_library_content(ui);
    }
    changed
}

/// Name für eine Farbe oder einen Stil abfragen (leer = ohne Namen).
fn lib_name_dialog(ui: &Rc<Ui>, title: &str, old: &str, set: impl Fn(&mut Library, &str) -> bool + 'static) {
    let entry = gtk::Entry::builder()
        .text(old)
        .placeholder_text(tr("ohne Namen"))
        .activates_default(true)
        .build();
    let dlg = adw::AlertDialog::new(
        Some(title),
        Some(tr("Der Name ist freiwillig und erscheint unter dem Feld")),
    );
    dlg.set_extra_child(Some(&entry));
    dlg.add_response("cancel", tr("_Abbrechen"));
    dlg.add_response("ok", tr("_Übernehmen"));
    dlg.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    dlg.set_default_response(Some("ok"));
    dlg.set_close_response("cancel");
    let u = ui.clone();
    dlg.connect_response(None, move |_, r| {
        if r != "ok" {
            return;
        }
        let name = entry.text().trim().to_string();
        with_library(&u, |l| set(l, &name));
    });
    label_icon_buttons(dlg.upcast_ref());
    dlg.present(Some(&ui.window));
}

/// Ist die gewählte Bibliothek die (unveränderliche) Standardbibliothek?
fn lib_locked(ui: &Ui) -> bool {
    ui.libraries
        .borrow()
        .get(ui.lib_current.get())
        .map(|l| l.locked)
        .unwrap_or(false)
}

/// Etwas zur Bibliothek hinzufügen. Ist gerade die Standardbibliothek gewählt, landet es
/// in der ersten eigenen Bibliothek (die bei Bedarf angelegt wird).
fn with_library_add(ui: &Rc<Ui>, f: impl FnOnce(&mut Library) -> bool) -> bool {
    if lib_locked(ui) {
        let idx = {
            let mut libs = ui.libraries.borrow_mut();
            match libs.iter().position(|l| !l.locked) {
                Some(i) => i,
                None => {
                    libs.push(Library::new(tr("Meine Bibliothek")));
                    libs.len() - 1
                }
            }
        };
        ui.lib_current.set(idx);
        save_settings(ui);
        rebuild_library(ui);
    }
    with_library(ui, f)
}

fn lib_added(ui: &Rc<Ui>, added: bool, what: &str) {
    let name = ui
        .libraries
        .borrow()
        .get(ui.lib_current.get())
        .map(|l| l.name.clone())
        .unwrap_or_default();
    if added {
        ui.toast(&trf("{} zu „{}“ hinzugefügt", &[&(what), &(name)]));
        show_library(&ui);
    } else {
        ui.toast(&trf("{} ist schon in „{}“", &[&(what), &(name)]));
    }
}

/// Zeile „Beschriftung … Bedienelement“ für die Dialoge.
fn form_row(label: &str, w: &impl IsA<gtk::Widget>) -> gtk::Box {
    let r = hbox(12);
    r.append(
        &gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .hexpand(true)
            .build(),
    );
    r.append(w);
    r
}

/// Ausgangswerte für einen neuen Stil: aus der Auswahl, sonst aus den Vorgaben.
fn starting_style(ui: &Rc<Ui>, text: bool) -> library::LibStyle {
    if let Some(d) = cur(ui) {
        let ed = d.ed.borrow();
        if let Some(s) = ed.selected_lib_style(text) {
            return s;
        }
        if text {
            let t = &ed.text_defaults;
            return library::LibStyle {
                style: ed.text_style.clone(),
                text: Some(library::TextStyle {
                    family: t.family.clone(),
                    size: t.size,
                    bold: t.bold,
                    italic: t.italic,
                    fmt: t.fmt,
                }),
                name: String::new(),
            };
        }
        return library::LibStyle {
            style: ed.defaults.clone(),
            text: None,
            name: String::new(),
        };
    }
    library::LibStyle {
        style: crate::model::Style::default(),
        text: if text {
            Some(library::TextStyle {
                family: "Sans".into(),
                size: 24.0,
                bold: false,
                italic: false,
                fmt: crate::model::TextFormat::default(),
            })
        } else {
            None
        },
        name: String::new(),
    }
}

/// Dialog „Neuer Stil“ bzw. „Neuer Textstil“ mit Vorschau.
fn new_style_dialog(ui: &Rc<Ui>, text: bool) {
    let start = starting_style(ui, text);
    let state = Rc::new(RefCell::new(start.clone()));

    let preview = gtk::DrawingArea::new();
    preview.set_content_width(220);
    preview.set_content_height(if text { 70 } else { 90 });
    {
        let st = state.clone();
        preview.set_draw_func(move |_, cr, w, h| {
            let (w, h) = (w as f64, h as f64);
            preview_card(cr, w, h);
            render::draw_style_preview(cr, w, h, &st.borrow());
        });
    }
    let form = vbox(10);
    form.append(&preview);

    // jede Änderung: Zustand anpassen und Vorschau neu zeichnen
    let changed = {
        let (st, pv) = (state.clone(), preview.clone());
        Rc::new(move |f: &dyn Fn(&mut library::LibStyle)| {
            f(&mut st.borrow_mut());
            pv.queue_draw();
        })
    };

    if let Some(t) = &start.text {
        let font = font_dropdown();
        font_select(&font, &t.family);
        {
            let ch = changed.clone();
            font.connect_selected_notify(move |b| {
                if let Some(fam) = font_selected(b) {
                    ch(&|s: &mut library::LibStyle| {
                        if let Some(t) = s.text.as_mut() {
                            t.family = fam.clone();
                        }
                    });
                }
            });
        }
        form.append(&form_row(tr("Schrift"), &font));
        let size = spin(1.0, 2000.0, 1.0, 0, tr("Schriftgröße in px"));
        size.set_value(t.size);
        {
            let ch = changed.clone();
            size.connect_value_changed(move |sp| {
                let v = sp.value();
                ch(&|s: &mut library::LibStyle| {
                    if let Some(t) = s.text.as_mut() {
                        t.size = v;
                    }
                });
            });
        }
        form.append(&form_row(tr("Größe (px)"), &size));
        // Schnitt der Familie (Light, Regular, Bold Italic …)
        let faces = Rc::new(RefCell::new(font_faces(&t.family)));
        let face = gtk::DropDown::new(Some(faces_model(&faces.borrow())), None::<gtk::Expression>);
        face.set_selected(face_index(&faces.borrow(), t.fmt.weight_for(t.bold), t.italic));
        {
            let (ch, faces) = (changed.clone(), faces.clone());
            face.connect_selected_notify(move |dd| {
                let f = { faces.borrow().get(dd.selected() as usize).cloned() };
                let Some(f) = f else { return };
                ch(&|s: &mut library::LibStyle| {
                    if let Some(t) = s.text.as_mut() {
                        t.bold = f.weight >= 600;
                        t.fmt.weight = if f.weight == crate::model::effective_weight(0, t.bold) {
                            0
                        } else {
                            f.weight
                        };
                        t.italic = f.italic;
                    }
                });
            });
        }
        {
            // andere Familie: Schnitte neu einlesen, möglichst gleichen Schnitt behalten
            let (st, faces, face) = (state.clone(), faces.clone(), face.clone());
            font.connect_selected_notify(move |b| {
                let Some(fam) = font_selected(b) else { return };
                let (w, it) = match &st.borrow().text {
                    Some(t) => (t.fmt.weight_for(t.bold), t.italic),
                    None => (400, false),
                };
                let list = font_faces(&fam);
                let idx = face_index(&list, w, it);
                *faces.borrow_mut() = list;
                face.set_model(Some(&faces_model(&faces.borrow())));
                face.set_selected(idx);
            });
        }
        form.append(&form_row(tr("Schnitt"), &face));
        let align =
            gtk::DropDown::from_strings(&[tr("Links"), tr("Zentriert"), tr("Rechts"), tr("Blocksatz")]);
        align.set_selected(match t.fmt.align {
            Align::Left => 0,
            Align::Center => 1,
            Align::Right => 2,
            Align::Justify => 3,
        });
        {
            let ch = changed.clone();
            align.connect_selected_notify(move |d| {
                let a = [Align::Left, Align::Center, Align::Right, Align::Justify]
                    [(d.selected() as usize).min(3)];
                ch(&|s: &mut library::LibStyle| {
                    if let Some(t) = s.text.as_mut() {
                        t.fmt.align = a;
                    }
                });
            });
        }
        form.append(&form_row(tr("Ausrichtung"), &align));
        let line = spin(
            0.5,
            5.0,
            0.1,
            2,
            tr("Zeilenabstand (Vielfaches der Schriftgröße)"),
        );
        line.set_value(t.fmt.line);
        {
            let ch = changed.clone();
            line.connect_value_changed(move |sp| {
                let v = sp.value();
                ch(&|s: &mut library::LibStyle| {
                    if let Some(t) = s.text.as_mut() {
                        t.fmt.line = v;
                    }
                });
            });
        }
        form.append(&form_row(tr("Zeilenabstand"), &line));
        let tracking = spin(-300.0, 2000.0, 10.0, 0, tr("Zeichenabstand in 1/1000 Geviert"));
        tracking.set_value(t.fmt.tracking);
        {
            let ch = changed.clone();
            tracking.connect_value_changed(move |sp| {
                let v = sp.value();
                ch(&|s: &mut library::LibStyle| {
                    if let Some(t) = s.text.as_mut() {
                        t.fmt.tracking = v;
                    }
                });
            });
        }
        form.append(&form_row(tr("Zeichenabstand"), &tracking));
        // Textfarbe mit derselben Farbwahl wie in den Eigenschaften
        form.append(&sub_heading(tr("Textfarbe")));
        let picker = ColorPicker::new();
        picker.show(start.style.fill.unwrap_or(Color::BLACK));
        {
            let ch = changed.clone();
            *picker.on_change.borrow_mut() = Some(Rc::new(move |c: Color| {
                ch(&|s: &mut library::LibStyle| {
                    s.style.fill = Some(c);
                    s.style.gradient = None;
                });
            }));
        }
        form.append(&picker.root);
    } else {
        // Füllung und Kontur: Reiter wie in den Eigenschaften, darunter die Farbwahl
        let fill_tab = gtk::ToggleButton::with_label(tr("Füllung"));
        let stroke_tab = gtk::ToggleButton::with_label(tr("Kontur"));
        stroke_tab.set_group(Some(&fill_tab));
        fill_tab.set_active(true);
        let tabs = hbox(0);
        tabs.set_homogeneous(true);
        for b in [&fill_tab, &stroke_tab] {
            b.add_css_class("flat");
            b.add_css_class("mondrian-tab");
            tabs.append(b);
        }
        form.append(&tabs);
        let none = gtk::CheckButton::with_label(tr("Keine Farbe"));
        let picker = ColorPicker::new();
        let editing_stroke = Rc::new(Cell::new(false));
        let syncing = Rc::new(Cell::new(false));
        // Farbwahl auf die gewählte Seite (Füllung/Kontur) einstellen
        let sync = {
            let (st, picker, none, es, syncing) = (
                state.clone(),
                picker.clone(),
                none.clone(),
                editing_stroke.clone(),
                syncing.clone(),
            );
            Rc::new(move || {
                let cur_c = {
                    let s = st.borrow();
                    if es.get() {
                        s.style.stroke
                    } else {
                        s.style.fill
                    }
                };
                syncing.set(true);
                none.set_active(cur_c.is_none());
                picker.show(cur_c.unwrap_or(if es.get() {
                    Color::BLACK
                } else {
                    Color::rgb(0.6, 0.757, 0.945)
                }));
                syncing.set(false);
            })
        };
        sync();
        for (tab, stroke) in [(fill_tab.clone(), false), (stroke_tab.clone(), true)] {
            let (es, sync) = (editing_stroke.clone(), sync.clone());
            tab.connect_toggled(move |t| {
                if t.is_active() {
                    es.set(stroke);
                    sync();
                }
            });
        }
        let set_color = {
            let (ch, es) = (changed.clone(), editing_stroke.clone());
            Rc::new(move |c: Option<Color>| {
                let stroke = es.get();
                ch(&|s: &mut library::LibStyle| {
                    if stroke {
                        s.style.stroke = c;
                        if c.is_some() && s.style.width <= 0.0 {
                            s.style.width = 1.0;
                        }
                    } else {
                        s.style.fill = c;
                        s.style.gradient = None;
                    }
                });
            })
        };
        {
            let (set_color, none, syncing) = (set_color.clone(), none.clone(), syncing.clone());
            *picker.on_change.borrow_mut() = Some(Rc::new(move |c: Color| {
                if syncing.get() {
                    return;
                }
                syncing.set(true);
                none.set_active(false);
                syncing.set(false);
                set_color(Some(c));
            }));
        }
        {
            let (set_color, picker, syncing) = (set_color.clone(), picker.clone(), syncing.clone());
            none.connect_toggled(move |b| {
                if syncing.get() {
                    return;
                }
                set_color((!b.is_active()).then(|| picker.color()));
            });
        }
        form.append(&none);
        form.append(&picker.root);
        let width = spin(0.0, 500.0, 0.5, 1, tr("Linienstärke in px"));
        width.set_value(start.style.width);
        {
            let ch = changed.clone();
            width.connect_value_changed(move |sp| {
                let v = sp.value();
                ch(&|s: &mut library::LibStyle| s.style.width = v);
            });
        }
        form.append(&form_row(tr("Linienstärke"), &width));
        let joins: Vec<&str> = LineJoin::ALL.iter().map(|j| j.label()).collect();
        let join = gtk::DropDown::from_strings(&joins);
        join.set_selected(
            LineJoin::ALL
                .iter()
                .position(|j| *j == start.style.join)
                .unwrap_or(0) as u32,
        );
        {
            let ch = changed.clone();
            join.connect_selected_notify(move |d| {
                let j = LineJoin::ALL[(d.selected() as usize).min(2)];
                ch(&|s: &mut library::LibStyle| s.style.join = j);
            });
        }
        form.append(&form_row(tr("Ecken"), &join));
        let caps: Vec<&str> = LineCap::ALL.iter().map(|c| c.label()).collect();
        let cap = gtk::DropDown::from_strings(&caps);
        cap.set_selected(
            LineCap::ALL
                .iter()
                .position(|c| *c == start.style.cap)
                .unwrap_or(0) as u32,
        );
        {
            let ch = changed.clone();
            cap.connect_selected_notify(move |d| {
                let c = LineCap::ALL[(d.selected() as usize).min(2)];
                ch(&|s: &mut library::LibStyle| s.style.cap = c);
            });
        }
        form.append(&form_row(tr("Enden"), &cap));
    }
    // Deckkraft und Mischmodus für beide
    let opacity = spin(0.0, 100.0, 5.0, 0, tr("Deckkraft in Prozent"));
    opacity.set_value((start.style.opacity * 100.0).round());
    {
        let ch = changed.clone();
        opacity.connect_value_changed(move |sp| {
            let v = sp.value() / 100.0;
            ch(&|s: &mut library::LibStyle| s.style.opacity = v);
        });
    }
    form.append(&form_row(tr("Deckkraft (%)"), &opacity));
    let blend_names: Vec<&str> = Blend::SIMPLE.iter().map(|b| b.label()).collect();
    let blend = gtk::DropDown::from_strings(&blend_names);
    blend.set_selected(
        Blend::SIMPLE
            .iter()
            .position(|b| *b == start.style.blend)
            .unwrap_or(0) as u32,
    );
    {
        let ch = changed.clone();
        blend.connect_selected_notify(move |d| {
            let b = Blend::SIMPLE
                .get(d.selected() as usize)
                .copied()
                .unwrap_or_default();
            ch(&|s: &mut library::LibStyle| s.style.blend = b);
        });
    }
    form.append(&form_row(tr("Farbüberlagerung"), &blend));

    let dlg = adw::AlertDialog::new(
        Some(if text {
            tr("Neuer Textstil")
        } else {
            tr("Neuer Stil")
        }),
        None,
    );
    form.set_size_request(280, -1);
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(560)
        .child(&form)
        .build();
    dlg.set_extra_child(Some(&scroll));
    dlg.add_response("cancel", tr("_Abbrechen"));
    dlg.add_response("add", tr("_Hinzufügen"));
    dlg.set_response_appearance("add", adw::ResponseAppearance::Suggested);
    dlg.set_default_response(Some("add"));
    dlg.set_close_response("cancel");
    let u = ui.clone();
    dlg.connect_response(None, move |_, r| {
        if r != "add" {
            return;
        }
        let st = state.borrow().clone();
        let added = with_library_add(&u, |l| l.add_style(st));
        lib_added(&u, added, if text { tr("Textstil") } else { tr("Stil") });
    });
    label_icon_buttons(dlg.upcast_ref());
    dlg.present(Some(&ui.window));
}

/// Bibliotheken neu von der Festplatte lesen (z. B. nach einem Abgleich mit anderen
/// Rechnern); die gewählte bleibt gewählt.
fn reload_libraries(ui: &Rc<Ui>) {
    let current = ui
        .libraries
        .borrow()
        .get(ui.lib_current.get())
        .map(|l| l.name.clone());
    let libs = load_libraries();
    let idx = current
        .and_then(|n| libs.iter().position(|l| l.name == n))
        .unwrap_or(0);
    *ui.libraries.borrow_mut() = libs;
    ui.lib_current.set(idx);
    rebuild_library(ui);
}

/// Zeichenflächen bearbeiten, solange die Leiste „Ebenen und Zeichenflächen“ offen ist.
fn artboard_mode(ui: &Ui) -> bool {
    ui.side_toggle.is_active() && ui.layers_toggle.is_active()
}

fn sync_artboard_mode(ui: &Ui) {
    let v = artboard_mode(ui);
    for d in ui.docs.borrow().iter() {
        d.ed.borrow_mut().artboard_mode = v;
    }
    redraw_all(ui);
}

/// Leisten: links Ebenen oder Bibliothek (je ein Knopf oben links), rechts die Eigenschaften
/// (Knopf oben rechts); beide Seiten können zugleich offen sein.
fn setup_dock(ui: &Rc<Ui>) {
    {
        // Informationen: Ordner des Dokuments in der Dateiverwaltung zeigen
        let wu = Rc::downgrade(ui);
        ui.info.folder_open.connect_clicked(move |_| {
            let Some(u) = wu.upgrade() else { return };
            let Some(f) = cur(&u).and_then(|d| d.file.borrow().clone()) else {
                return;
            };
            gtk::FileLauncher::new(Some(&gio::File::for_path(&f))).open_containing_folder(
                Some(&u.window),
                gio::Cancellable::NONE,
                |_| {},
            );
        });
    }
    // rechts: Eigenschaften oder Informationen (immer nur eines)
    for (name, t, other) in [
        ("props", ui.props_toggle.clone(), ui.info_toggle.clone()),
        ("info", ui.info_toggle.clone(), ui.props_toggle.clone()),
    ] {
        let wu = Rc::downgrade(ui);
        t.connect_toggled(move |b| {
            let Some(u) = wu.upgrade() else { return };
            if b.is_active() {
                if other.is_active() {
                    other.set_active(false);
                }
                if u.narrow.get() {
                    u.side_toggle.set_active(false);
                }
                u.dock_stack.set_visible_child_name(name);
                if name == "info" {
                    refresh_info(&u);
                }
            }
            let was_hidden = !u.dock.is_visible();
            u.dock.set_visible(right_open(&u) && cur(&u).is_some());
            if u.dock.is_visible() && was_hidden {
                apply_props_width(&u);
            }
        });
    }
    {
        let wu = Rc::downgrade(ui);
        ui.side_toggle.connect_toggled(move |b| {
            let Some(u) = wu.upgrade() else { return };
            if b.is_active() {
                if u.narrow.get() {
                    u.props_toggle.set_active(false);
                    u.info_toggle.set_active(false);
                }
                // über F8 geöffnet: zuletzt gezeigten Inhalt wählen
                if !u.layers_toggle.is_active() && !u.lib.toggle.is_active() {
                    if u.side_last.get() == "library" {
                        u.lib.toggle.set_active(true);
                    } else {
                        u.layers_toggle.set_active(true);
                    }
                }
            } else {
                u.layers_toggle.set_active(false);
                u.lib.toggle.set_active(false);
            }
            let was_hidden = !u.side_dock.is_visible();
            u.side_dock.set_visible(b.is_active() && cur(&u).is_some());
            if u.side_dock.is_visible() && was_hidden {
                u.side_paned.set_position(u.side_width.get());
                layout_library(&u, false);
            }
            sync_artboard_mode(&u);
        });
    }
    for (name, t, other) in [
        ("layers", ui.layers_toggle.clone(), ui.lib.toggle.clone()),
        ("library", ui.lib.toggle.clone(), ui.layers_toggle.clone()),
    ] {
        let wu = Rc::downgrade(ui);
        t.connect_toggled(move |b| {
            let Some(u) = wu.upgrade() else { return };
            if b.is_active() {
                // immer nur einer der beiden Inhalte
                if other.is_active() {
                    other.set_active(false);
                }
                u.side_last.set(name);
                u.side_stack.set_visible_child_name(name);
                // z. B. „Zur Bibliothek hinzufügen“: Leiste dafür öffnen
                if !u.side_toggle.is_active() {
                    u.side_toggle.set_active(true);
                }
                if name == "library" {
                    layout_library(&u, false);
                }
            } else if !u.layers_toggle.is_active() && !u.lib.toggle.is_active() && u.side_toggle.is_active() {
                u.side_toggle.set_active(false);
            }
            sync_artboard_mode(&u);
        });
    }
    ui.dock_stack.set_visible_child_name("props");
    ui.side_stack.set_visible_child_name("layers");
    ui.side_paned.set_position(ui.side_width.get());

    // Schmale Fenster: Seitenleisten schweben über der Zeichenfläche statt sie zu verkleinern
    if let Ok(cond) = adw::BreakpointCondition::parse("max-width: 1100sp") {
        let bp = adw::Breakpoint::new(cond);
        let wu = Rc::downgrade(ui);
        bp.connect_apply(move |_| {
            if let Some(u) = wu.upgrade() {
                set_narrow(&u, true);
            }
        });
        let wu = Rc::downgrade(ui);
        bp.connect_unapply(move |_| {
            if let Some(u) = wu.upgrade() {
                set_narrow(&u, false);
            }
        });
        ui.window.add_breakpoint(bp);
    }
    // Klick neben eine schwebende Seitenleiste schließt sie (der Klick wirkt sonst nicht)
    {
        let wu = Rc::downgrade(ui);
        let click = gtk::GestureClick::new();
        click.set_button(0);
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        click.connect_pressed(move |g, _, x, y| {
            let Some(u) = wu.upgrade() else { return };
            if !u.narrow.get() {
                return;
            }
            let p = gtk::graphene::Point::new(x as f32, y as f32);
            let inside = |w: &gtk::Box| {
                w.is_visible()
                    && w.compute_bounds(&u.canvas_overlay)
                        .map(|r| r.contains_point(&p))
                        .unwrap_or(false)
            };
            let mut closed = false;
            if u.side_dock.is_visible() && !inside(&u.side_dock) {
                u.side_toggle.set_active(false);
                closed = true;
            }
            if u.dock.is_visible() && !inside(&u.dock) {
                u.props_toggle.set_active(false);
                u.info_toggle.set_active(false);
                closed = true;
            }
            if closed {
                g.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        ui.canvas_overlay.add_controller(click);
    }
}

/// Zwischen nebeneinanderliegenden (breit) und schwebenden (schmal) Seitenleisten wechseln.
fn set_narrow(ui: &Rc<Ui>, on: bool) {
    if ui.narrow.replace(on) == on {
        return;
    }
    if on {
        ui.side_paned.set_start_child(None::<&gtk::Widget>);
        ui.props_paned.set_end_child(None::<&gtk::Widget>);
        for (w, align, width) in [
            (&ui.side_dock, gtk::Align::Start, ui.side_width.get()),
            (&ui.dock, gtk::Align::End, ui.props_width.get()),
        ] {
            w.set_halign(align);
            w.set_valign(gtk::Align::Fill);
            w.set_size_request(width.min(360), -1);
            w.add_css_class("mondrian-overlay-sidebar");
            ui.canvas_overlay.add_overlay(w);
        }
        // nur eine Seitenleiste zugleich
        if ui.side_toggle.is_active() && right_open(ui) {
            ui.side_toggle.set_active(false);
        }
    } else {
        for w in [&ui.side_dock, &ui.dock] {
            ui.canvas_overlay.remove_overlay(w);
            w.set_size_request(-1, -1);
            w.set_halign(gtk::Align::Fill);
            w.remove_css_class("mondrian-overlay-sidebar");
        }
        ui.side_paned.set_start_child(Some(&ui.side_dock));
        ui.props_paned.set_end_child(Some(&ui.dock));
        ui.side_paned.set_position(ui.side_width.get());
        if ui.dock.is_visible() {
            apply_props_width(ui);
        }
    }
    layout_library(ui, false);
    update_swatches(ui);
}

/// Ist die rechte Leiste (Eigenschaften oder Informationen) offen?
fn right_open(ui: &Ui) -> bool {
    ui.props_toggle.is_active() || ui.info_toggle.is_active()
}

/// Linke Leiste mit der Bibliothek zeigen.
fn show_library(ui: &Ui) {
    ui.lib.toggle.set_active(true);
}

/// Eigenschaften auf ihre gespeicherte Breite bringen, sobald die Fläche bekannt ist.
fn apply_props_width(ui: &Ui) {
    let want = ui.props_width.get();
    let _ = ui.props_paned.add_tick_callback(move |p, _| {
        let total = p.width();
        if total <= 0 {
            return glib::ControlFlow::Continue;
        }
        p.set_position((total - want).max(200));
        glib::ControlFlow::Break
    });
}

/// Aktuelle Breite der Eigenschaften (sonst die gespeicherte).
fn props_panel_width(ui: &Ui) -> i32 {
    if ui.narrow.get() {
        return ui.props_width.get().min(360);
    }
    let w = ui.props_paned.width() - ui.props_paned.position();
    if ui.dock.is_visible() && ui.props_paned.width() > 0 && (150..=900).contains(&w) {
        w
    } else {
        ui.props_width.get()
    }
}

/// Aktuelle Breite der linken Leiste (sonst die gespeicherte).
fn side_panel_width(ui: &Ui) -> i32 {
    if ui.narrow.get() {
        return ui.side_width.get().min(360);
    }
    if ui.side_dock.is_visible() && ui.side_paned.position() >= 150 {
        ui.side_paned.position()
    } else {
        ui.side_width.get()
    }
}

fn setup_library(ui: &Rc<Ui>) {
    add_action(ui, "lib-reload", reload_libraries);
    add_action(ui, "lib-folder", |u| {
        let dir = libs_dir();
        let _ = std::fs::create_dir_all(&dir);
        let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&dir)));
        launcher.launch(Some(&u.window), gio::Cancellable::NONE, |_| {});
    });
    add_action(ui, "lib-export", |u| {
        let Some(lib) = u.libraries.borrow().get(u.lib_current.get()).cloned() else {
            return;
        };
        let dialog = gtk::FileDialog::builder()
            .title(tr("Bibliothek exportieren"))
            .modal(true)
            .initial_name(library::file_name(&lib.name))
            .build();
        dialog.set_filters(Some(&file_filter(tr("Stijl-Bibliothek (SVG)"), &["*.svg"])));
        let u2 = u.clone();
        dialog.save(Some(&u.window), gio::Cancellable::NONE, move |res| {
            if let Some(mut p) = res.ok().and_then(|f| f.path()) {
                if p.extension().is_none() {
                    p.set_extension("svg");
                }
                match crate::fsutil::write_atomic(&p, lib.to_svg().as_bytes()) {
                    Ok(()) => u2.toast(&trf("„{}“ exportiert", &[&(lib.name)])),
                    Err(e) => u2.toast(&trf("Exportieren fehlgeschlagen: {}", &[&(e)])),
                }
            }
        });
    });
    add_action(ui, "lib-import", |u| {
        let dialog = gtk::FileDialog::builder()
            .title(tr("Bibliothek importieren"))
            .modal(true)
            .build();
        dialog.set_filters(Some(&file_filter(
            tr("Stijl-Bibliothek (SVG)"),
            &["*.svg", "*.SVG"],
        )));
        let u2 = u.clone();
        dialog.open(Some(&u.window), gio::Cancellable::NONE, move |res| {
            let Some(p) = res.ok().and_then(|f| f.path()) else {
                return;
            };
            let base = p
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| tr("Bibliothek").into());
            let lib = std::fs::read_to_string(&p)
                .map_err(|e| e.to_string())
                .and_then(|t| Library::from_svg(&base, &t));
            let mut lib = match lib {
                Ok(l) if !l.colors.is_empty() || !l.styles.is_empty() => l,
                Ok(_) => {
                    u2.toast(tr("In dieser Datei ist keine Bibliothek"));
                    return;
                }
                Err(e) => {
                    u2.toast(&trf("Importieren fehlgeschlagen: {}", &[&(e)]));
                    return;
                }
            };
            // gleichnamige Bibliothek nicht überschreiben
            {
                let libs = u2.libraries.borrow();
                let mut k = 2;
                while libs.iter().any(|l| l.name == lib.name) {
                    lib.name = format!("{base} {k}");
                    k += 1;
                }
            }
            save_library(&lib);
            let name = lib.name.clone();
            let n = {
                let mut libs = u2.libraries.borrow_mut();
                // leere Standardbibliothek durch die importierte ersetzen
                if libs.len() == 1 && libs[0].colors.is_empty() && libs[0].styles.is_empty() {
                    libs.clear();
                }
                libs.push(lib);
                libs.len()
            };
            u2.lib_current.set(n - 1);
            save_settings(&u2);
            rebuild_library(&u2);
            show_library(&u2);
            u2.toast(&trf("„{}“ importiert", &[&(name)]));
        });
    });
    {
        let wu = Rc::downgrade(ui);
        ui.lib.toggle.connect_toggled(move |b| {
            if let Some(u) = wu.upgrade() {
                if b.is_active() {
                    // Änderungen von anderen Rechnern (abgeglichener Ordner) übernehmen
                    reload_libraries(&u);
                }
            }
        });
    }
    {
        let wu = Rc::downgrade(ui);
        ui.props_paned.connect_position_notify(move |p| {
            let Some(u) = wu.upgrade() else { return };
            let w = p.width() - p.position();
            if u.narrow.get() || !u.dock.is_visible() || p.width() <= 0 || !(150..=900).contains(&w) {
                return;
            }
            update_swatches(&u);
            let w = w.clamp(200, 700);
            if w != u.props_width.get() {
                u.props_width.set(w);
                if !u.save_pending.replace(true) {
                    let u2 = u.clone();
                    glib::timeout_add_local_once(std::time::Duration::from_millis(600), move || {
                        u2.save_pending.set(false);
                        save_settings(&u2);
                    });
                }
            }
        });
    }
    {
        let wu = Rc::downgrade(ui);
        ui.side_paned.connect_position_notify(move |p| {
            let Some(u) = wu.upgrade() else { return };
            if u.narrow.get() || !u.side_dock.is_visible() || p.position() < 150 {
                return;
            }
            layout_library(&u, false);
            let w = p.position().clamp(200, 700);
            if w != u.side_width.get() {
                u.side_width.set(w);
                if !u.save_pending.replace(true) {
                    let u2 = u.clone();
                    glib::timeout_add_local_once(std::time::Duration::from_millis(600), move || {
                        u2.save_pending.set(false);
                        save_settings(&u2);
                    });
                }
            }
        });
    }
    {
        let u = ui.clone();
        ui.lib.dropdown.connect_selected_notify(move |dd| {
            if u.updating.get() {
                return;
            }
            let i = dd.selected() as usize;
            if i >= u.libraries.borrow().len() || i == u.lib_current.get() {
                return;
            }
            u.lib_current.set(i);
            // erst nach dem Schließen der Liste neu aufbauen
            let u2 = u.clone();
            glib::idle_add_local_once(move || {
                save_settings(&u2);
                rebuild_library_content(&u2);
            });
        });
    }
    for stroke in [false, true] {
        add_action(
            ui,
            if stroke { "lib-add-stroke" } else { "lib-add-fill" },
            move |u| {
                let t = if stroke {
                    PaintTarget::Stroke
                } else {
                    PaintTarget::Fill
                };
                let Some((c, g)) = cur(u).and_then(|d| d.ed.borrow().selected_paint(t)) else {
                    u.toast(tr("Erst ein Objekt mit dieser Farbe auswählen"));
                    return;
                };
                let what = if g.is_some() { tr("Verlauf") } else { tr("Farbe") };
                let added = with_library_add(u, |l| l.add_paint(c, g));
                lib_added(u, added, what);
            },
        );
    }
    for text in [false, true] {
        add_action(
            ui,
            if text {
                "lib-add-textstyle"
            } else {
                "lib-add-style"
            },
            move |u| {
                let Some(st) = cur(u).and_then(|d| d.ed.borrow().selected_lib_style(text)) else {
                    u.toast(if text {
                        tr("Erst einen Text auswählen")
                    } else {
                        tr("Erst eine Form oder einen Pfad auswählen")
                    });
                    return;
                };
                let added = with_library_add(u, |l| l.add_style(st));
                lib_added(u, added, if text { tr("Textstil") } else { tr("Stil") });
            },
        );
        add_action(
            ui,
            if text {
                "lib-new-textstyle"
            } else {
                "lib-new-style"
            },
            move |u| {
                new_style_dialog(u, text);
            },
        );
    }
    add_action(ui, "lib-new-color", |u| {
        let initial = cur(u)
            .and_then(|d| d.ed.borrow().selected_color(false))
            .unwrap_or(Color::rgb(0.21, 0.52, 0.89));
        lib_new_color_dialog(u, initial);
    });
    let indexed = |name: &str, f: fn(&Rc<Ui>, usize)| {
        let a = gio::SimpleAction::new(name, Some(glib::VariantTy::UINT32));
        let u = ui.clone();
        a.connect_activate(move |_, v| {
            if let Some(i) = v.and_then(|v| v.get::<u32>()) {
                f(&u, i as usize);
            }
        });
        ui.window.add_action(&a);
    };
    fn color_at(u: &Rc<Ui>, i: usize) -> Option<(Color, Option<Gradient>)> {
        u.libraries
            .borrow()
            .get(u.lib_current.get())
            .and_then(|l| Some((*l.colors.get(i)?, l.gradient(i).cloned())))
    }
    indexed("lib-color-fill", |u, i| {
        if let Some((c, g)) = color_at(u, i) {
            with_ed(u, |e| {
                e.apply_lib_color(c, g, false);
            });
        }
    });
    indexed("lib-color-stroke", |u, i| {
        if let Some((c, g)) = color_at(u, i) {
            with_ed(u, |e| {
                e.apply_lib_color(c, g, true);
            });
        }
    });
    indexed("lib-graphic-insert", |u, i| {
        let objs = u
            .libraries
            .borrow()
            .get(u.lib_current.get())
            .and_then(|l| l.graphics.get(i))
            .map(|g| g.objects.clone());
        let Some(objs) = objs else { return };
        let Some(d) = cur(u) else { return };
        // in der Mitte der sichtbaren Fläche einfügen
        let (w, h) = canvas_size(&d);
        let at = d.ed.borrow().view.to_doc(Pt::new(w / 2.0, h / 2.0));
        with_ed_m(u, |e, m| {
            if e.tool != Tool::Select {
                e.set_tool(Tool::Select);
            }
            e.insert_graphic(&objs, at, m);
        });
    });
    indexed("lib-graphic-name", |u, i| {
        let old = u
            .libraries
            .borrow()
            .get(u.lib_current.get())
            .and_then(|l| l.graphics.get(i))
            .map(|g| g.name.clone())
            .unwrap_or_default();
        lib_name_dialog(u, tr("Grafik benennen"), &old, move |l, name| {
            match l.graphics.get_mut(i) {
                Some(g) if g.name != name && !name.is_empty() => {
                    g.name = name.to_string();
                    true
                }
                _ => false,
            }
        });
    });
    indexed("lib-graphic-remove", |u, i| {
        let u2 = u.clone();
        glib::idle_add_local_once(move || {
            with_library(&u2, |l| {
                if i < l.graphics.len() {
                    l.graphics.remove(i);
                    true
                } else {
                    false
                }
            });
        });
    });
    add_action(ui, "lib-add-graphic", |u| {
        let objs = cur(u)
            .map(|d| d.ed.borrow().selected_graphic())
            .unwrap_or_default();
        if objs.is_empty() {
            u.toast(tr("Erst die Objekte der Grafik auswählen"));
            return;
        }
        let entry = gtk::Entry::builder()
            .placeholder_text(tr("Name, z. B. Logo"))
            .activates_default(true)
            .build();
        let preview = gtk::DrawingArea::new();
        preview.set_content_width(96);
        preview.set_content_height(72);
        preview.set_halign(gtk::Align::Center);
        {
            let objs = objs.clone();
            preview.set_draw_func(move |_, cr, w, h| {
                let (w, h) = (w as f64, h as f64);
                preview_card(cr, w, h);
                render::draw_objects_preview(cr, w, h, &objs, 6.0);
            });
        }
        let form = vbox(10);
        form.append(&preview);
        form.append(&entry);
        let dlg = adw::AlertDialog::new(
            Some(tr("Als Grafik in die Bibliothek")),
            Some(tr(
                "Die ausgewählten Objekte lassen sich später per Ziehen in jedes Dokument kopieren",
            )),
        );
        dlg.set_extra_child(Some(&form));
        dlg.add_response("cancel", tr("_Abbrechen"));
        dlg.add_response("add", tr("_Hinzufügen"));
        dlg.set_response_appearance("add", adw::ResponseAppearance::Suggested);
        dlg.set_default_response(Some("add"));
        dlg.set_close_response("cancel");
        let u2 = u.clone();
        dlg.connect_response(None, move |_, r| {
            if r != "add" {
                return;
            }
            let name = entry.text().to_string();
            let objs = objs.clone();
            let added = with_library_add(&u2, move |l| l.add_graphic(&name, objs));
            lib_added(&u2, added, tr("Grafik"));
        });
        label_icon_buttons(dlg.upcast_ref());
        dlg.present(Some(&u.window));
    });
    indexed("lib-color-name", |u, i| {
        let old = u
            .libraries
            .borrow()
            .get(u.lib_current.get())
            .map(|l| l.color_name(i).to_string())
            .unwrap_or_default();
        lib_name_dialog(u, tr("Farbe benennen"), &old, move |l, name| {
            l.set_color_name(i, name)
        });
    });
    indexed("lib-style-name", |u, i| {
        let (old, text) = u
            .libraries
            .borrow()
            .get(u.lib_current.get())
            .and_then(|l| l.styles.get(i))
            .map(|s| (s.name.clone(), s.text.is_some()))
            .unwrap_or_default();
        let title = if text {
            tr("Textstil benennen")
        } else {
            tr("Stil benennen")
        };
        lib_name_dialog(u, title, &old, move |l, name| l.set_style_name(i, name));
    });
    indexed("lib-color-remove", |u, i| {
        let u2 = u.clone();
        glib::idle_add_local_once(move || {
            with_library(&u2, |l| {
                if i < l.colors.len() {
                    l.remove_color(i)
                } else {
                    false
                }
            });
        });
    });
    indexed("lib-style-apply", |u, i| {
        let st = u
            .libraries
            .borrow()
            .get(u.lib_current.get())
            .and_then(|l| l.styles.get(i).cloned());
        if let Some(st) = st {
            with_ed_m(u, |e, m| {
                e.apply_lib_style(&st, m);
            });
        }
    });
    indexed("lib-style-remove", |u, i| {
        let u2 = u.clone();
        glib::idle_add_local_once(move || {
            with_library(&u2, |l| {
                if i < l.styles.len() {
                    l.styles.remove(i);
                    true
                } else {
                    false
                }
            });
        });
    });
    add_action(ui, "lib-new", |u| {
        let name = {
            let libs = u.libraries.borrow();
            let mut k = libs.len() + 1;
            let mut name = trf("Bibliothek {}", &[&(k)]);
            while libs.iter().any(|l| l.name == name) {
                k += 1;
                name = trf("Bibliothek {}", &[&(k)]);
            }
            name
        };
        let lib = Library::new(&name);
        save_library(&lib);
        let n = {
            let mut libs = u.libraries.borrow_mut();
            libs.push(lib);
            libs.len()
        };
        u.lib_current.set(n - 1);
        save_settings(u);
        rebuild_library(u);
        show_library(&u);
    });
    add_action(ui, "lib-rename", |u| {
        if lib_locked(u) {
            u.toast(tr("Die Standardbibliothek „Stijl“ lässt sich nicht umbenennen"));
            return;
        }
        let old = u
            .libraries
            .borrow()
            .get(u.lib_current.get())
            .map(|l| l.name.clone())
            .unwrap_or_default();
        let entry = gtk::Entry::builder().text(&old).activates_default(true).build();
        let dlg = adw::AlertDialog::new(Some(tr("Bibliothek umbenennen")), None);
        dlg.set_extra_child(Some(&entry));
        dlg.add_response("cancel", tr("_Abbrechen"));
        dlg.add_response("ok", tr("_Umbenennen"));
        dlg.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
        dlg.set_default_response(Some("ok"));
        dlg.set_close_response("cancel");
        let u2 = u.clone();
        dlg.connect_response(None, move |_, r| {
            if r != "ok" {
                return;
            }
            let name = entry.text().trim().to_string();
            if name.is_empty() || name == old {
                return;
            }
            if u2.libraries.borrow().iter().any(|l| l.name == name) {
                u2.toast(tr("Eine Bibliothek mit diesem Namen gibt es schon"));
                return;
            }
            let _ = std::fs::remove_file(libs_dir().join(library::file_name(&old)));
            {
                let mut libs = u2.libraries.borrow_mut();
                if let Some(l) = libs.get_mut(u2.lib_current.get()) {
                    l.name = name.clone();
                    save_library(l);
                }
            }
            save_settings(&u2);
            rebuild_library(&u2);
        });
        label_icon_buttons(dlg.upcast_ref());
        dlg.present(Some(&u.window));
    });
    add_action(ui, "lib-delete", |u| {
        if lib_locked(u) {
            u.toast(tr("Die Standardbibliothek „Stijl“ lässt sich nicht löschen"));
            return;
        }
        let name = u
            .libraries
            .borrow()
            .get(u.lib_current.get())
            .map(|l| l.name.clone())
            .unwrap_or_default();
        let dlg = adw::AlertDialog::new(
            Some(tr("Bibliothek löschen?")),
            Some(&trf("„{}“ mit allen Farben und Stilen wird gelöscht", &[&(name)])),
        );
        dlg.add_response("cancel", tr("_Abbrechen"));
        dlg.add_response("delete", tr("_Löschen"));
        dlg.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dlg.set_close_response("cancel");
        let u2 = u.clone();
        dlg.connect_response(None, move |_, r| {
            if r != "delete" {
                return;
            }
            let _ = std::fs::remove_file(libs_dir().join(library::file_name(&name)));
            {
                let mut libs = u2.libraries.borrow_mut();
                let i = u2.lib_current.get();
                if i < libs.len() {
                    libs.remove(i);
                }
                if libs.is_empty() {
                    libs.push(Library::new(tr("Meine Bibliothek")));
                }
            }
            u2.lib_current.set(0);
            save_settings(&u2);
            rebuild_library(&u2);
        });
        label_icon_buttons(dlg.upcast_ref());
        dlg.present(Some(&u.window));
    });

    rebuild_library(ui);
}
