// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! Zeichnen mit Cairo/Pango: Dokument, Zeichenfläche und Bearbeitungshilfen.

use crate::i18n::{tr, trf};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::f64::consts::TAU;
use std::rc::{Rc, Weak};

use gtk::gdk_pixbuf::prelude::*;
use gtk::{cairo, gdk_pixbuf, pango};

use crate::editor::{Drag, Editor, Metrics, Tool, HANDLE_RADIUS};
use crate::geom::{Affine, Pt, Rect};
use crate::model::{
    Align, Art, Blend, Color, Document, Effects, Gradient, ImagePart, Layer, ListStyle, Marker, Node, Object,
    RichText, Shape, SubPath, TextFormat, VAlign,
};

const ACCENT: (f64, f64, f64) = (0.208, 0.518, 0.894);

fn set_color(cr: &cairo::Context, c: Color) {
    cr.set_source_rgba(c.r, c.g, c.b, c.a);
}

thread_local! {
    /// Aktuelle Markierungsfarbe (Farbe der Ebene des gerade gezeichneten Objekts).
    static ACC: std::cell::Cell<(f64, f64, f64)> = const { std::cell::Cell::new(ACCENT) };
}

thread_local! {
    /// Akzentfarbe des Systems (setzt das Fenster vor dem Zeichnen)
    static UI_ACCENT: std::cell::Cell<(f64, f64, f64)> = const { std::cell::Cell::new(ACCENT) };
}

/// Akzentfarbe des Systems setzen.
pub fn set_ui_accent(c: (f64, f64, f64)) {
    UI_ACCENT.with(|a| a.set(c));
}

/// Akzentfarbe des Systems (für Markierungen).
pub fn ui_accent() -> (f64, f64, f64) {
    UI_ACCENT.with(|a| a.get())
}

fn ui_accent_color() -> Color {
    let a = ui_accent();
    Color::rgb(a.0, a.1, a.2)
}

fn accent(cr: &cairo::Context, a: f64) {
    let c = ACC.with(|c| c.get());
    cr.set_source_rgba(c.0, c.1, c.2, a);
}

fn use_color(c: Color) {
    ACC.with(|a| a.set((c.r, c.g, c.b)));
}

/// Markierungsfarbe auf die Ebene des Objekts `id` setzen.
fn use_layer_color(ed: &Editor, id: u64) {
    let c = ed.doc.layer_of(id).map(|l| crate::model::layer_color(l.color));
    use_color(c.unwrap_or(ui_accent_color()));
}

/// Markierungsfarbe der aktiven Ebene.
fn use_active_layer_color(ed: &Editor) {
    let c = ed
        .doc
        .layer(ed.active_layer)
        .map(|l| crate::model::layer_color(l.color));
    use_color(c.unwrap_or(ui_accent_color()));
}

pub fn font_desc(family: &str, size: f64, bold: bool, italic: bool) -> pango::FontDescription {
    let mut fd = pango::FontDescription::new();
    fd.set_family(family);
    fd.set_absolute_size(size.max(0.5) * pango::SCALE as f64);
    if bold {
        fd.set_weight(pango::Weight::Bold);
    }
    if italic {
        fd.set_style(pango::Style::Italic);
    }
    fd
}

fn invertible(a: &Affine) -> bool {
    (a.a * a.d - a.b * a.c).abs() > 1e-12
}

fn map_subpaths(subs: &[SubPath], m: &Affine) -> Vec<SubPath> {
    subs.iter()
        .map(|sp| SubPath {
            nodes: sp
                .nodes
                .iter()
                .map(|n| Node {
                    r: n.r,
                    p: m.apply(n.p),
                    h_in: m.apply(n.h_in),
                    h_out: m.apply(n.h_out),
                })
                .collect(),
            closed: sp.closed,
        })
        .collect()
}

fn path_subpaths(cr: &cairo::Context, subs: &[SubPath]) {
    for sp in subs {
        let Some(first) = sp.nodes.first() else { continue };
        cr.move_to(first.p.x, first.p.y);
        for (_, c1, c2, p) in sp.segments() {
            cr.curve_to(c1.x, c1.y, c2.x, c2.y, p.x, p.y);
        }
        if sp.closed {
            cr.close_path();
        }
    }
}

/// Aktuellen Cairo-Pfad in Teilpfade umwandeln.
fn cairo_to_subpaths(path: &cairo::Path) -> Vec<SubPath> {
    let mut subs: Vec<SubPath> = Vec::new();
    let mut cur: Vec<Node> = Vec::new();
    let finish = |subs: &mut Vec<SubPath>, cur: &mut Vec<Node>, closed: bool| {
        let mut nodes = std::mem::take(cur);
        if closed && nodes.len() > 1 {
            let l = nodes.len() - 1;
            if nodes[l].p.dist(nodes[0].p) < 1e-6 {
                nodes[0].h_in = nodes[l].h_in;
                nodes.pop();
            }
        }
        if nodes.len() >= 2 {
            subs.push(SubPath { nodes, closed });
        }
    };
    for seg in path.iter() {
        match seg {
            cairo::PathSegment::MoveTo((x, y)) => {
                finish(&mut subs, &mut cur, false);
                cur.push(Node::corner(Pt::new(x, y)));
            }
            cairo::PathSegment::LineTo((x, y)) => cur.push(Node::corner(Pt::new(x, y))),
            cairo::PathSegment::CurveTo((x1, y1), (x2, y2), (x3, y3)) => {
                if let Some(l) = cur.last_mut() {
                    l.h_out = Pt::new(x1, y1);
                }
                let p = Pt::new(x3, y3);
                cur.push(Node {
                    r: 0.0,
                    p,
                    h_in: Pt::new(x2, y2),
                    h_out: p,
                });
            }
            cairo::PathSegment::ClosePath => finish(&mut subs, &mut cur, true),
        }
    }
    finish(&mut subs, &mut cur, false);
    subs
}

// ───────────────────────────── Textgeometrie ─────────────────────────────

/// Umriss und Maße eines Textes im eigenen Koordinatensystem
/// (Grundlinie der ersten Zeile bei y = 0). Ohne Hinting berechnet, damit der Text
/// bei jeder Zoomstufe und Skalierung exakt gleich aussieht.
pub struct TextGeom {
    pub outline: Vec<SubPath>,
    pub bbox: Rect,
    pub advance: f64,
}

type TextKey = (String, String, u64, bool, bool, u64, (u8, u64, u64));

thread_local! {
    static SCRATCH: cairo::Context = {
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 1, 1).expect("Cairo-Fläche");
        cairo::Context::new(&surface).expect("Cairo-Kontext")
    };
    static TEXT_CACHE: RefCell<HashMap<TextKey, Rc<TextGeom>>> = RefCell::new(HashMap::new());
    static IMAGE_CACHE: RefCell<HashMap<usize, (Weak<Vec<u8>>, Option<cairo::ImageSurface>)>> =
        RefCell::new(HashMap::new());
    static NOISE: RefCell<Option<cairo::ImageSurface>> = const { RefCell::new(None) };
    static FLAT_CACHE: RefCell<HashMap<usize, (Weak<Vec<u8>>, bool)>> = RefCell::new(HashMap::new());
    /// Bilder mit Kunst- oder Farbfilter: (Bild, Filterschlüssel, Ergebnis)
    static IMAGE_FX_CACHE: RefCell<HashMap<usize, (Weak<Vec<u8>>, String, Option<cairo::ImageSurface>)>> =
        RefCell::new(HashMap::new());
}

fn fmt_key(f: &TextFormat) -> (u8, u64, u64) {
    let a = match f.align {
        Align::Left => 0,
        Align::Center => 1,
        Align::Right => 2,
        Align::Justify => 3,
    };
    let l = match f.list {
        crate::model::ListStyle::None => 0,
        crate::model::ListStyle::Dot => 1,
        crate::model::ListStyle::Square => 2,
        crate::model::ListStyle::Dash => 3,
        crate::model::ListStyle::Number => 4,
    };
    (
        a | (f.underline as u8) << 2 | (f.strike as u8) << 3 | l << 4,
        f.line.to_bits(),
        f.tracking.to_bits(),
    )
}

/// Pango-Kontext ohne Hinting (wie für die Textgeometrie).
fn plain_context() -> pango::Context {
    SCRATCH.with(|cr| {
        let pctx = pangocairo::functions::create_context(cr);
        if let Ok(mut fo) = cairo::FontOptions::new() {
            fo.set_hint_metrics(cairo::HintMetrics::Off);
            fo.set_hint_style(cairo::HintStyle::None);
            pangocairo::functions::context_set_font_options(&pctx, Some(&fo));
        }
        pctx
    })
}

/// Tabstopps: ein Tabulator springt zum nächsten Vielfachen von 2,4 Schriftgrößen.
fn set_default_tabs(l: &pango::Layout, size: f64) {
    let mut tabs = pango::TabArray::new(1, false);
    tabs.set_tab(
        0,
        pango::TabAlign::Left,
        (size * 2.4 * pango::SCALE as f64).round().max(1.0) as i32,
    );
    l.set_tabs(Some(&tabs));
}

/// Schrift mit den Formatwünschen des Absatzes (Kapitälchen).
/// Pango-Strichstärke zu einer Zahl (1–1000).
pub fn pango_weight(w: u16) -> pango::Weight {
    match w {
        100 => pango::Weight::Thin,
        200 => pango::Weight::Ultralight,
        300 => pango::Weight::Light,
        350 => pango::Weight::Semilight,
        380 => pango::Weight::Book,
        400 => pango::Weight::Normal,
        500 => pango::Weight::Medium,
        600 => pango::Weight::Semibold,
        700 => pango::Weight::Bold,
        800 => pango::Weight::Ultrabold,
        900 => pango::Weight::Heavy,
        1000 => pango::Weight::Ultraheavy,
        other => pango::Weight::__Unknown(other.clamp(1, 1000) as i32),
    }
}

fn styled_font(fd: &pango::FontDescription, fmt: &TextFormat) -> pango::FontDescription {
    let mut fd = fd.clone();
    // Zwischenstufen der Strichstärke (Medium, Extra Bold …, variable Schriften)
    if fmt.weight > 0 {
        fd.set_weight(pango_weight(fmt.weight));
    }
    if fmt.small_caps {
        fd.set_variant(pango::Variant::SmallCaps);
    }
    fd
}

/// Zeichenabstand, Unter- und Durchstreichen als Pango-Attribute setzen.
fn apply_tracking(l: &pango::Layout, fmt: &TextFormat, size: f64) {
    let t = fmt.tracking_px(size);
    if t.abs() < 1e-9 && !fmt.underline && !fmt.strike && !fmt.no_ligatures {
        return;
    }
    let attrs = pango::AttrList::new();
    if fmt.no_ligatures {
        attrs.insert(pango::AttrFontFeatures::new("liga=0, clig=0"));
    }
    if t.abs() > 1e-9 {
        attrs.insert(pango::AttrInt::new_letter_spacing(
            (t * pango::SCALE as f64).round() as i32,
        ));
    }
    if fmt.underline {
        attrs.insert(pango::AttrInt::new_underline(pango::Underline::Single));
    }
    if fmt.strike {
        attrs.insert(pango::AttrInt::new_strikethrough(true));
    }
    l.set_attributes(Some(&attrs));
}

/// Abweichend formatierte Textteile als Pango-Attribute ins Layout eintragen.
/// `off`/`len`: Bereich des Layout-Textes im Gesamttext; `map`: Aufzählungszeichen davor.
fn add_runs(l: &pango::Layout, rich: &RichText, off: usize, len: usize, map: &[(usize, usize, usize)]) {
    if rich.runs.is_empty() {
        return;
    }
    let k = pango::SCALE as f64;
    let attrs = l.attributes().unwrap_or_else(pango::AttrList::new);
    let conv = |i: usize| {
        (if map.is_empty() {
            i
        } else {
            crate::model::list_to_display(map, i)
        }) as u32
    };
    for r in &rich.runs {
        let (a, b) = (r.start.clamp(off, off + len), r.end.clamp(off, off + len));
        if b <= a {
            continue;
        }
        let (da, db) = (conv(a - off), conv(b - off));
        let st = &r.style;
        let mut list: Vec<pango::Attribute> = Vec::new();
        if let Some(f) = &st.family {
            list.push(pango::AttrString::new_family(f).into());
        }
        if let Some(z) = st.size {
            list.push(pango::AttrSize::new_size_absolute((z.max(0.5) * k).round() as i32).into());
        }
        match (st.weight, st.bold) {
            (Some(w), _) => list.push(pango::AttrInt::new_weight(pango_weight(w)).into()),
            (None, Some(v)) => list.push(
                pango::AttrInt::new_weight(if v {
                    pango::Weight::Bold
                } else {
                    pango::Weight::Normal
                })
                .into(),
            ),
            _ => {}
        }
        if let Some(v) = st.italic {
            list.push(
                pango::AttrInt::new_style(if v {
                    pango::Style::Italic
                } else {
                    pango::Style::Normal
                })
                .into(),
            );
        }
        if let Some(v) = st.underline {
            list.push(
                pango::AttrInt::new_underline(if v {
                    pango::Underline::Single
                } else {
                    pango::Underline::None
                })
                .into(),
            );
        }
        if let Some(v) = st.strike {
            list.push(pango::AttrInt::new_strikethrough(v).into());
        }
        for mut at in list {
            at.set_start_index(da);
            at.set_end_index(db);
            attrs.insert(at);
        }
    }
    l.set_attributes(Some(&attrs));
}

/// Schriftgröße am Anfang des Absatzes, der bei Byte `start` beginnt.
fn para_size(rich: &RichText, start: usize, base: f64) -> f64 {
    rich.runs
        .iter()
        .filter(|r| r.start <= start && (start < r.end || r.start == r.end && r.start == start))
        .filter_map(|r| r.style.size)
        .last()
        .unwrap_or(base)
        .max(0.5)
}

/// Aufzählungszeichen (die ersten `len` Bytes des Layouts) in der Größe des Absatzes.
fn size_marker(l: &pango::Layout, len: usize, psize: f64, base: f64) {
    if len == 0 || (psize - base).abs() < 1e-9 {
        return;
    }
    let attrs = l.attributes().unwrap_or_else(pango::AttrList::new);
    let mut at: pango::Attribute =
        pango::AttrSize::new_size_absolute((psize * pango::SCALE as f64).round() as i32).into();
    at.set_start_index(0);
    at.set_end_index(len as u32);
    attrs.insert(at);
    l.set_attributes(Some(&attrs));
}

/// Absätze eines rechteckigen Textfeldes als eigene Layouts untereinander, je mit
/// Aufzählungszeichen und Einzug (Ursprung = linke obere Ecke des Textbereichs).
/// Bei mehreren Spalten läuft der Text von Spalte zu Spalte, sobald die Höhe `limit`
/// erreicht ist; Absätze werden dabei zeilenweise geteilt. Liefert die Layouts und die
/// Gesamthöhe (bei Spalten die der längsten Spalte; läuft der Text aus der letzten Spalte
/// hinaus, wächst sie über `limit`).
#[allow(clippy::too_many_arguments)]
fn para_lines(
    pctx: &pango::Context,
    text: &str,
    family: &str,
    size: f64,
    bold: bool,
    italic: bool,
    width: f64,
    limit: f64,
    fmt: &TextFormat,
    rich: &RichText,
) -> (EditLines, f64) {
    let k = pango::SCALE as f64;
    let fd = font_desc(family, size, bold, italic);
    // Abstand zwischen den Zeilen wie innerhalb eines Absatzes, dazu der Absatzabstand
    let extra = size * fmt.line - natural_line(pctx, &fd);
    let gap = size * fmt.para.max(0.0);
    let cols = fmt.cols();
    let gutter = size * 1.5;
    let col_w = if cols > 1 {
        ((width - gutter * (cols - 1) as f64) / cols as f64).max(1.0)
    } else {
        width
    };
    let mut lines: EditLines = Vec::new();
    let mut y = 0.0;
    let mut max_y: f64 = 0.0;
    let mut col = 0usize;
    let mut start = 0usize;
    let mut counters = [0usize; crate::model::MAX_INDENT as usize + 1];
    for (i, para) in text.split('\n').enumerate() {
        let pf = rich.para(i, fmt.list);
        let lvl = (pf.indent.min(crate::model::MAX_INDENT)) as usize;
        let marker = match pf.list {
            ListStyle::None => {
                counters = [0; crate::model::MAX_INDENT as usize + 1];
                String::new()
            }
            ListStyle::Number => {
                counters[lvl] += 1;
                for c in counters[lvl + 1..].iter_mut() {
                    *c = 0;
                }
                ListStyle::Number.marker(counters[lvl] - 1)
            }
            other => {
                for c in counters[lvl..].iter_mut() {
                    *c = 0;
                }
                other.marker(0)
            }
        };
        // Aufzählungszeichen, Einzug und Abstand richten sich nach der Schriftgröße am Anfang
        // des Absatzes (nicht nach der Grundgröße des Feldes)
        let psize = para_size(rich, start, size);
        let left = lvl as f64 * indent_step(psize);
        let (shown, map) = crate::model::para_display(para, &marker);
        let prefix = shown.len() - para.len();
        let l = area_layout(
            pctx,
            &shown,
            family,
            size,
            bold,
            italic,
            (col_w - left).max(1.0),
            fmt,
        );
        let hang = psize * if pf.list == ListStyle::Number { 1.8 } else { 1.2 };
        if !marker.is_empty() {
            // Platz für das Zeichen, Folgezeilen hängen darunter ein
            let mut tabs = pango::TabArray::new(1, false);
            tabs.set_tab(0, pango::TabAlign::Left, (hang * k).round() as i32);
            l.set_tabs(Some(&tabs));
            l.set_indent(-(hang * k).round() as i32);
        }
        add_runs(&l, rich, start, para.len(), &map);
        size_marker(&l, prefix, psize, size);
        if y > 0.0 {
            y += extra + gap;
        }
        // Zeilen des Absatzes: (Byte-Beginn, Oberkante, Unterkante) im Layout
        let mut spans: Vec<(usize, f64, f64)> = Vec::new();
        if cols > 1 {
            let mut it = l.iter();
            let mut idx = 0;
            loop {
                let (y0, y1) = it.line_yrange();
                let st = l
                    .line_readonly(idx)
                    .map(|ln| ln.start_index().max(0) as usize)
                    .unwrap_or(0);
                spans.push((st, y0 as f64 / k, y1 as f64 / k));
                idx += 1;
                if !it.next_line() {
                    break;
                }
            }
        }
        let col_x = |c: usize| c as f64 * (col_w + gutter);
        if spans.len() <= 1 {
            // ein Absatz ohne Teilung (bei Spalten: eventuell erst in die nächste Spalte)
            let (_, logical) = l.extents();
            let hgt = logical.height() as f64 / k;
            if cols > 1 && col + 1 < cols && y > 0.0 && y + hgt > limit {
                max_y = max_y.max(y);
                col += 1;
                y = 0.0;
            }
            lines.push(EditLine {
                layout: l,
                x: left + col_x(col),
                top: y,
                start,
                first_only: false,
                map,
            });
            y += hgt;
        } else {
            // Absatz zeilenweise auf die Spalten verteilen
            let n = spans.len();
            let mut first = 0usize;
            let mut li = 0usize;
            let mut base = y;
            let emit = |a: usize, b: usize, col: usize, top: f64, lines: &mut EditLines| {
                let s0 = spans[a].0;
                let e0 = if b < n { spans[b].0 } else { shown.len() };
                if a == 0 && b == n {
                    lines.push(EditLine {
                        layout: l.clone(),
                        x: left + col_x(col),
                        top,
                        start,
                        first_only: false,
                        map: map.clone(),
                    });
                    return;
                }
                let chunk = shown[s0..e0].trim_end_matches('\u{2028}');
                let hang_in = if a > 0 && !marker.is_empty() { hang } else { 0.0 };
                let lay = area_layout(
                    pctx,
                    chunk,
                    family,
                    size,
                    bold,
                    italic,
                    (col_w - left - hang_in).max(1.0),
                    fmt,
                );
                if a == 0 {
                    if !marker.is_empty() {
                        let mut tabs = pango::TabArray::new(1, false);
                        tabs.set_tab(0, pango::TabAlign::Left, (hang * k).round() as i32);
                        lay.set_tabs(Some(&tabs));
                        lay.set_indent(-(hang * k).round() as i32);
                    }
                    add_runs(&lay, rich, start, e0.saturating_sub(prefix), &map);
                    size_marker(&lay, prefix, psize, size);
                    lines.push(EditLine {
                        layout: lay,
                        x: left + col_x(col),
                        top,
                        start,
                        first_only: false,
                        map: map.clone(),
                    });
                } else {
                    let real = s0 - prefix;
                    add_runs(&lay, rich, start + real, e0 - s0, &[]);
                    lines.push(EditLine {
                        layout: lay,
                        x: left + hang_in + col_x(col),
                        top,
                        start: start + real,
                        first_only: false,
                        map: Vec::new(),
                    });
                }
            };
            while li < n {
                let bottom = base + spans[li].2 - spans[first].1;
                if bottom > limit && col + 1 < cols && (li > first || base > 0.0) {
                    if li > first {
                        emit(first, li, col, base, &mut lines);
                        max_y = max_y.max(base + spans[li - 1].2 - spans[first].1);
                    }
                    col += 1;
                    base = 0.0;
                    first = li;
                }
                li += 1;
            }
            emit(first, n, col, base, &mut lines);
            y = base + spans[n - 1].2 - spans[first].1;
        }
        max_y = max_y.max(y);
        start += para.len() + 1;
    }
    (lines, if cols > 1 { max_y.max(y) } else { y })
}

/// Einzug je Stufe.
fn indent_step(size: f64) -> f64 {
    size * 1.5
}

/// Einzelne Zeile eines Punkttextes.
fn line_layout(
    pctx: &pango::Context,
    line: &str,
    fd: &pango::FontDescription,
    fmt: &TextFormat,
    size: f64,
) -> pango::Layout {
    let l = pango::Layout::new(pctx);
    l.set_font_description(Some(&styled_font(fd, fmt)));
    set_default_tabs(&l, size);
    apply_tracking(&l, fmt, size);
    l.set_text(line);
    l
}

/// Waagerechte Verschiebung einer Zeile von Punkttext (Ausrichtung am Ankerpunkt).
fn line_shift(l: &pango::Layout, fmt: &TextFormat) -> f64 {
    let (_, logical) = l.extents();
    let w = logical.width() as f64 / pango::SCALE as f64;
    match fmt.align {
        Align::Center => -w / 2.0,
        Align::Right => -w,
        _ => 0.0,
    }
}

/// Natürliche Zeilenhöhe einer Schrift (ohne zusätzlichen Abstand).
fn natural_line(pctx: &pango::Context, fd: &pango::FontDescription) -> f64 {
    let l = pango::Layout::new(pctx);
    l.set_font_description(Some(fd));
    l.set_text("Ag");
    let (_, logical) = l.extents();
    logical.height() as f64 / pango::SCALE as f64
}

/// Umbrochenes Layout eines Textfeldes (Ursprung = linke obere Ecke).
#[allow(clippy::too_many_arguments)]
fn area_layout(
    pctx: &pango::Context,
    text: &str,
    family: &str,
    size: f64,
    bold: bool,
    italic: bool,
    width: f64,
    fmt: &TextFormat,
) -> pango::Layout {
    let k = pango::SCALE as f64;
    let fd = font_desc(family, size, bold, italic);
    let l = pango::Layout::new(pctx);
    l.set_font_description(Some(&styled_font(&fd, fmt)));
    set_default_tabs(&l, size);
    l.set_width((width.max(1.0) * k) as i32);
    l.set_wrap(pango::WrapMode::WordChar);
    l.set_alignment(match fmt.align {
        Align::Center => pango::Alignment::Center,
        Align::Right => pango::Alignment::Right,
        _ => pango::Alignment::Left,
    });
    l.set_justify(fmt.align == Align::Justify);
    // Zeilenabstand: Grundlinienabstand = Schriftgröße × Faktor
    let extra = size * fmt.line - natural_line(pctx, &fd);
    l.set_spacing((extra * k).round() as i32);
    apply_tracking(&l, fmt, size);
    l.set_text(text);
    l
}

/// Höhe eines umbrochenen Textes (für das Füllen von Textfeldern).
#[allow(clippy::too_many_arguments)]
fn area_height(
    text: &str,
    family: &str,
    size: f64,
    bold: bool,
    italic: bool,
    width: f64,
    limit: f64,
    fmt: &TextFormat,
) -> f64 {
    para_lines(
        &plain_context(),
        text,
        family,
        size,
        bold,
        italic,
        width,
        limit,
        fmt,
        &RichText::default(),
    )
    .1
}

/// Senkrechte Verschiebung des Textes in einem Textfeld (oben, Mitte, unten).
fn area_dy(shape: &Shape) -> f64 {
    let Shape::TextArea { h, fmt, frame, .. } = shape else {
        return 0.0;
    };
    let i = frame.inset.max(0.0);
    let th = rect_geom(shape).map(|g| g.bbox.y1).unwrap_or(0.0);
    let free = (h - 2.0 * i - th).max(0.0);
    i + match fmt.valign {
        VAlign::Top => 0.0,
        VAlign::Middle => free / 2.0,
        VAlign::Bottom => free,
    }
}

/// Absätze eines rechteckigen Textfeldes (ohne senkrechte Ausrichtung) und Texthöhe.
fn rect_lines(shape: &Shape) -> Option<(EditLines, f64)> {
    let Shape::TextArea {
        w,
        h,
        text,
        family,
        size,
        bold,
        italic,
        fmt,
        frame,
        rich,
        ..
    } = shape
    else {
        return None;
    };
    if frame.path.is_some() {
        return None;
    }
    let i = frame.inset.max(0.0);
    Some(para_lines(
        &plain_context(),
        text,
        family,
        *size,
        *bold,
        *italic,
        (w - 2.0 * i).max(1.0),
        (h - 2.0 * i).max(1.0),
        fmt,
        rich,
    ))
}

thread_local! {
    static RECT_CACHE: RefCell<HashMap<u64, Rc<TextGeom>>> = RefCell::new(HashMap::new());
}

/// Umriss eines rechteckigen Textfeldes im Textbereich (oben links = 0,0) – zwischengespeichert.
fn rect_geom(shape: &Shape) -> Option<Rc<TextGeom>> {
    use std::hash::{Hash, Hasher};
    let Shape::TextArea {
        w,
        h,
        text,
        family,
        size,
        bold,
        italic,
        fmt,
        frame,
        rich,
        ..
    } = shape
    else {
        return None;
    };
    if frame.path.is_some() {
        return None;
    }
    let mut hs = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hs);
    family.hash(&mut hs);
    size.to_bits().hash(&mut hs);
    bold.hash(&mut hs);
    italic.hash(&mut hs);
    format!("{fmt:?}").hash(&mut hs);
    w.to_bits().hash(&mut hs);
    if fmt.cols() > 1 {
        h.to_bits().hash(&mut hs);
    }
    frame.inset.to_bits().hash(&mut hs);
    if !rich.is_plain() {
        format!("{rich:?}").hash(&mut hs);
    }
    let key = hs.finish();
    if let Some(g) = RECT_CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return Some(g);
    }
    let (lines, height) = rect_lines(shape)?;
    let width = (w - 2.0 * frame.inset.max(0.0)).max(1.0);
    let geom = SCRATCH.with(|cr| {
        cr.new_path();
        for l in &lines {
            cr.move_to(l.x, l.top);
            pangocairo::functions::layout_path(cr, &l.layout);
        }
        let outline = cr.copy_path().map(|p| cairo_to_subpaths(&p)).unwrap_or_default();
        cr.new_path();
        TextGeom {
            outline,
            bbox: Rect::from_xywh(0.0, 0.0, width, height.max(1.0)),
            advance: width,
        }
    });
    let g = Rc::new(geom);
    RECT_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 500 {
            c.clear();
        }
        c.insert(key, g.clone());
    });
    Some(g)
}

// ───────────── Text in einer Form ─────────────

/// Teilpfade als geschlossene Polygone (für Zeilenbreiten in Formen).
fn flatten_polys(subs: &[SubPath]) -> Vec<Vec<Pt>> {
    subs.iter()
        .filter(|sp| sp.nodes.len() >= 2)
        .map(|sp| {
            let mut v = vec![sp.nodes[0].p];
            let mut segs = sp.segments();
            if !sp.closed {
                // offene Form wie geschlossen behandeln
                let (a, b) = (sp.nodes[sp.nodes.len() - 1], sp.nodes[0]);
                segs.push((a.p, a.p, b.p, b.p));
            }
            for (p0, c1, c2, p3) in segs {
                for k in 1..=12 {
                    v.push(crate::geom::cubic_point(p0, c1, c2, p3, k as f64 / 12.0));
                }
            }
            v
        })
        .collect()
}

/// Innere Abschnitte (x von–bis) auf Höhe `y`.
fn spans_at(polys: &[Vec<Pt>], y: f64) -> Vec<(f64, f64)> {
    let mut xs = Vec::new();
    for poly in polys {
        let n = poly.len();
        for i in 0..n {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            if (a.y <= y && b.y > y) || (b.y <= y && a.y > y) {
                let t = (y - a.y) / (b.y - a.y);
                xs.push(a.x + t * (b.x - a.x));
            }
        }
    }
    xs.sort_by(|a, b| a.total_cmp(b));
    xs.chunks_exact(2).map(|c| (c[0], c[1])).collect()
}

fn intersect_spans(a: &[(f64, f64)], b: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for &(a0, a1) in a {
        for &(b0, b1) in b {
            let (lo, hi) = (a0.max(b0), a1.min(b1));
            if hi > lo {
                out.push((lo, hi));
            }
        }
    }
    out
}

/// Breitester Abschnitt, der über die ganze Höhe `ya`…`yb` innen liegt (minus Abstand).
fn band_span(polys: &[Vec<Pt>], ya: f64, yb: f64, inset: f64) -> Option<(f64, f64)> {
    let n = 6;
    let mut cur = spans_at(polys, ya);
    for i in 1..=n {
        let y = ya + (yb - ya) * i as f64 / n as f64;
        cur = intersect_spans(&cur, &spans_at(polys, y));
        if cur.is_empty() {
            return None;
        }
    }
    cur.into_iter()
        .map(|(a, b)| (a + inset, b - inset))
        .filter(|(a, b)| b > a)
        .max_by(|x, y| (x.1 - x.0).total_cmp(&(y.1 - y.0)))
}

/// Zeilen eines Textfeldes mit eigener Form: jede Zeile so breit, wie die Form an dieser
/// Stelle ist. Liefert die Zeilen und ob Text nicht mehr hineinpasst.
fn shape_lines(shape: &Shape) -> Option<(EditLines, bool)> {
    let Shape::TextArea {
        h,
        text,
        family,
        size,
        bold,
        italic,
        fmt,
        frame,
        rich,
        ..
    } = shape
    else {
        return None;
    };
    let path = frame.path.as_ref()?;
    let gap = size * fmt.para.max(0.0);
    let subs = crate::model::effective_subpaths(path);
    let polys = flatten_polys(&subs);
    let bb = subpaths_bbox(&subs)?;
    let pctx = plain_context();
    let pitch = (size * fmt.line).max(1.0);
    let inset = frame.inset.max(0.0);
    let top0 = bb.y0.max(0.0) + inset;
    let bottom = bb.y1.min(*h) - inset;
    let min_w = size * 1.2;
    let run = |y0: f64| -> (EditLines, bool, f64) {
        let mut lines: EditLines = Vec::new();
        let mut y = y0;
        let mut pos = 0usize;
        let mut guard = 0;
        let mut overflow = false;
        loop {
            guard += 1;
            if guard > 4000 {
                break;
            }
            let done = pos >= text.len();
            // nach dem Text nur noch eine leere Zeile, falls er mit einem Umbruch endet
            if done && !(lines.is_empty() || text.ends_with('\n')) {
                break;
            }
            if y + pitch > bottom + 1e-6 {
                overflow = !done;
                break;
            }
            let Some((a, b)) =
                band_span(&polys, y - inset, y + pitch + inset, inset).filter(|(a, b)| b - a >= min_w)
            else {
                y += pitch * 0.25;
                continue;
            };
            if done || text[pos..].starts_with('\n') {
                let layout = area_layout(&pctx, "", family, *size, *bold, *italic, b - a, fmt);
                lines.push(EditLine {
                    layout,
                    x: a,
                    top: y,
                    start: pos,
                    first_only: true,
                    map: Vec::new(),
                });
                y += pitch;
                if done {
                    break;
                }
                pos += 1;
                y += gap;
                continue;
            }
            let rest = &text[pos..];
            let layout = area_layout(&pctx, rest, family, *size, *bold, *italic, b - a, fmt);
            add_runs(&layout, rich, pos, rest.len(), &[]);
            let len = layout
                .line_readonly(0)
                .map(|l| l.length().max(0) as usize)
                .unwrap_or(0)
                .min(rest.len());
            if len == 0 {
                overflow = true;
                break;
            }
            lines.push(EditLine {
                layout,
                x: a,
                top: y,
                start: pos,
                first_only: true,
                map: Vec::new(),
            });
            pos += len;
            y += pitch;
            if text[pos..].starts_with('\n') {
                pos += 1;
                y += gap;
            }
        }
        let used = y - y0;
        (lines, overflow, used)
    };
    let (lines, ov, used) = run(top0);
    if fmt.valign == VAlign::Top || ov {
        return Some((lines, ov));
    }
    let free = (bottom - top0 - used).max(0.0);
    let dy = if fmt.valign == VAlign::Middle {
        free / 2.0
    } else {
        free
    };
    if dy < 0.5 {
        return Some((lines, ov));
    }
    let (l2, ov2, _) = run(top0 + dy);
    if ov2 {
        Some((lines, ov))
    } else {
        Some((l2, ov2))
    }
}

type ShapeKey = u64;

thread_local! {
    static SHAPE_CACHE: RefCell<HashMap<ShapeKey, Rc<(TextGeom, bool)>>> = RefCell::new(HashMap::new());
}

/// Umriss (lokal) und Überlauf eines Textes in einer Form – zwischengespeichert.
fn shape_geom(shape: &Shape) -> Option<Rc<(TextGeom, bool)>> {
    use std::hash::{Hash, Hasher};
    let Shape::TextArea {
        w,
        h,
        text,
        family,
        size,
        bold,
        italic,
        fmt,
        frame,
        ..
    } = shape
    else {
        return None;
    };
    frame.path.as_ref()?;
    let mut hs = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hs);
    family.hash(&mut hs);
    size.to_bits().hash(&mut hs);
    bold.hash(&mut hs);
    italic.hash(&mut hs);
    fmt_key(fmt).hash(&mut hs);
    (fmt.weight, fmt.small_caps, fmt.no_ligatures).hash(&mut hs);
    fmt.valign.name().hash(&mut hs);
    w.to_bits().hash(&mut hs);
    h.to_bits().hash(&mut hs);
    format!("{:?}{}", frame.path, frame.inset).hash(&mut hs);
    fmt.para.to_bits().hash(&mut hs);
    if let Shape::TextArea { rich, .. } = shape {
        if !rich.is_plain() {
            format!("{rich:?}").hash(&mut hs);
        }
    }
    let key = hs.finish();
    if let Some(g) = SHAPE_CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return Some(g);
    }
    let (lines, overflow) = shape_lines(shape)?;
    let k = pango::SCALE as f64;
    let geom = SCRATCH.with(|cr| {
        cr.new_path();
        let mut bbox: Option<Rect> = None;
        for l in &lines {
            let Some(line0) = l.layout.line_readonly(0) else {
                continue;
            };
            let mut it = l.layout.iter();
            let (_, lr) = it.line_extents();
            let x = l.x + lr.x() as f64 / k;
            let base = l.top + l.layout.baseline() as f64 / k;
            cr.move_to(x, base);
            pangocairo::functions::layout_line_path(cr, &line0);
            let r = Rect::from_xywh(
                x,
                l.top + lr.y() as f64 / k,
                (lr.width() as f64 / k).max(1.0),
                (lr.height() as f64 / k).max(1.0),
            );
            bbox = Some(match bbox {
                Some(b) => b.union(&r),
                None => r,
            });
        }
        let outline = cr.copy_path().map(|p| cairo_to_subpaths(&p)).unwrap_or_default();
        cr.new_path();
        TextGeom {
            outline,
            bbox: bbox.unwrap_or(Rect::from_xywh(0.0, 0.0, 1.0, 1.0)),
            advance: 0.0,
        }
    });
    let g = Rc::new((geom, overflow));
    SHAPE_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 500 {
            c.clear();
        }
        c.insert(key, g.clone());
    });
    Some(g)
}

/// Rahmen eines Textfeldes (Füllung, Kontur) zeichnen.
fn draw_text_frame(cr: &cairo::Context, xf: &Affine, w: f64, h: f64, frame: &crate::model::TextFrame) {
    if frame.fill.is_none() && frame.stroke.is_none() {
        return;
    }
    cr.new_path();
    match &frame.path {
        Some(p) => path_subpaths(cr, &map_subpaths(&crate::model::effective_subpaths(p), xf)),
        None => box_path(cr, xf, w, h),
    }
    if let Some(f) = frame.fill {
        set_color(cr, f);
        let _ = cr.fill_preserve();
    }
    if let Some(s) = frame.stroke.filter(|_| frame.width > 0.0) {
        set_color(cr, s);
        cr.set_line_width(frame.width);
        cr.set_line_join(cairo::LineJoin::Miter);
        let _ = cr.stroke_preserve();
    }
    cr.new_path();
}

/// Kasten hinter Punkttext bzw. Text auf Pfad: Matrix, Breite, Höhe (Kasten von (0,0)
/// bis (w,h) im System der Matrix).
fn text_box_geom(shape: &Shape) -> Option<(Affine, f64, f64)> {
    match shape {
        Shape::Text {
            xf,
            text,
            family,
            size,
            bold,
            italic,
            fmt,
        } if fmt.has_box() => {
            let b = text_geom(text, family, *size, *bold, *italic, fmt).bbox;
            let i = fmt.box_inset.max(0.0);
            Some((
                xf.then_before(&Affine::translate(b.x0 - i, b.y0 - i)),
                b.width() + 2.0 * i,
                b.height() + 2.0 * i,
            ))
        }
        Shape::TextPath { fmt, .. } if fmt.has_box() => {
            let b = subpaths_bbox(&textpath_outline(shape))?;
            let i = fmt.box_inset.max(0.0);
            Some((
                Affine::translate(b.x0 - i, b.y0 - i),
                b.width() + 2.0 * i,
                b.height() + 2.0 * i,
            ))
        }
        _ => None,
    }
}

/// Kasten eines Textes als Teilpfad in Dokumentkoordinaten (z. B. für den Export).
pub fn text_box_outline(shape: &Shape) -> Option<Vec<SubPath>> {
    let (m, w, h) = text_box_geom(shape)?;
    let c = [
        m.apply(Pt::new(0.0, 0.0)),
        m.apply(Pt::new(w, 0.0)),
        m.apply(Pt::new(w, h)),
        m.apply(Pt::new(0.0, h)),
    ];
    Some(vec![SubPath {
        nodes: c.iter().map(|p| Node::corner(*p)).collect(),
        closed: true,
    }])
}

/// Lage-Matrix eines bearbeitbaren Textes.
fn text_xf(shape: &Shape) -> Option<Affine> {
    match shape {
        Shape::Text { xf, .. } | Shape::TextArea { xf, .. } => Some(*xf),
        _ => None,
    }
}

/// Zeile bzw. Layout zum Bearbeiten im Textkoordinatensystem.
struct EditLine {
    layout: pango::Layout,
    /// Ursprung des Layouts
    x: f64,
    top: f64,
    /// Byte-Start des Layout-Textes im Gesamttext
    start: usize,
    /// Nur die erste Zeile des Layouts gilt (Text in einer Form)
    first_only: bool,
    /// Aufzählung: Zuordnung Original ↔ angezeigter Text (leer = gleich)
    map: Vec<(usize, usize, usize)>,
}

type EditLines = Vec<EditLine>;

fn edit_layouts(shape: &Shape) -> Option<(String, EditLines)> {
    let k = pango::SCALE as f64;
    let pctx = plain_context();
    match shape {
        Shape::Text {
            text,
            family,
            size,
            bold,
            italic,
            fmt,
            ..
        } => {
            let fd = font_desc(family, *size, *bold, *italic);
            let mut out = Vec::new();
            let mut start = 0;
            for (i, line) in text.split('\n').enumerate() {
                let l = line_layout(&pctx, line, &fd, fmt, *size);
                let top = i as f64 * size * fmt.line - l.baseline() as f64 / k;
                let x = line_shift(&l, fmt);
                out.push(EditLine {
                    layout: l,
                    x,
                    top,
                    start,
                    first_only: false,
                    map: Vec::new(),
                });
                start += line.len() + 1;
            }
            Some((text.clone(), out))
        }
        Shape::TextArea { text, frame, .. } => {
            if frame.path.is_some() {
                return shape_lines(shape).map(|(l, _)| (text.clone(), l));
            }
            let i = frame.inset.max(0.0);
            let dy = area_dy(shape);
            let (mut lines, _) = rect_lines(shape)?;
            for l in lines.iter_mut() {
                l.x += i;
                l.top += dy;
            }
            Some((text.clone(), lines))
        }
        _ => None,
    }
}

/// Layout, das Byte-Index `idx` enthält: (Layout, x-Versatz, Oberkante, lokaler Index).
fn layout_for(layouts: &EditLines, text: &str, idx: usize) -> Option<(pango::Layout, f64, f64, usize)> {
    for (i, l) in layouts.iter().enumerate() {
        let inside = idx >= l.start
            && match layouts.get(i + 1) {
                Some(n) => idx < n.start,
                None => idx <= text.len(),
            };
        if inside {
            let li = idx - l.start;
            let li = if l.map.is_empty() {
                li
            } else {
                crate::model::list_to_display(&l.map, li)
            };
            return Some((l.layout.clone(), l.x, l.top, li));
        }
    }
    None
}

/// Textcursor als Linie im Dokument.
pub fn caret_line(shape: &Shape, idx: usize) -> Option<(Pt, Pt)> {
    if matches!(shape, Shape::TextPath { .. }) {
        return textpath_caret(shape, idx);
    }
    let xf = text_xf(shape)?;
    let (text, layouts) = edit_layouts(shape)?;
    let (l, x0, top, li) = layout_for(&layouts, &text, idx)?;
    let k = pango::SCALE as f64;
    let r = l.index_to_pos(li as i32);
    let x = x0 + r.x() as f64 / k;
    Some((
        xf.apply(Pt::new(x, top + r.y() as f64 / k)),
        xf.apply(Pt::new(x, top + (r.y() + r.height()) as f64 / k)),
    ))
}

/// Markierung als Vierecke im Dokument.
pub fn selection_quads(shape: &Shape, a: usize, b: usize) -> Vec<[Pt; 4]> {
    if matches!(shape, Shape::TextPath { .. }) {
        return textpath_quads(shape, a, b);
    }
    let (Some(xf), Some((text, layouts))) = (text_xf(shape), edit_layouts(shape)) else {
        return Vec::new();
    };
    let k = pango::SCALE as f64;
    let mut out = Vec::new();
    for (i, c) in text.char_indices() {
        if i < a || i >= b || c == '\n' {
            continue;
        }
        let Some((l, x0, top, li)) = layout_for(&layouts, &text, i) else {
            continue;
        };
        let r = l.index_to_pos(li as i32);
        let (x1, x2) = {
            let x = x0 + r.x() as f64 / k;
            let w = r.width() as f64 / k;
            (x.min(x + w), x.max(x + w))
        };
        let (y0, y1) = (top + r.y() as f64 / k, top + (r.y() + r.height()) as f64 / k);
        out.push([
            xf.apply(Pt::new(x1, y0)),
            xf.apply(Pt::new(x2, y0)),
            xf.apply(Pt::new(x2, y1)),
            xf.apply(Pt::new(x1, y1)),
        ]);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn compute_text_geom(
    text: &str,
    family: &str,
    size: f64,
    bold: bool,
    italic: bool,
    fmt: &TextFormat,
) -> TextGeom {
    SCRATCH.with(|cr| {
        cr.new_path();
        let pctx = pangocairo::functions::create_context(cr);
        if let Ok(mut fo) = cairo::FontOptions::new() {
            fo.set_hint_metrics(cairo::HintMetrics::Off);
            fo.set_hint_style(cairo::HintStyle::None);
            pangocairo::functions::context_set_font_options(&pctx, Some(&fo));
        }
        let fd = font_desc(family, size, bold, italic);
        let mut bbox: Option<Rect> = None;
        let mut advance: f64 = 0.0;
        let k = pango::SCALE as f64;
        for (i, line) in text.split('\n').enumerate() {
            let layout = line_layout(&pctx, line, &fd, fmt, size);
            let baseline = layout.baseline() as f64 / k;
            let top = i as f64 * size * fmt.line - baseline;
            let x = line_shift(&layout, fmt);
            cr.move_to(x, top);
            pangocairo::functions::layout_path(cr, &layout);
            let (_, logical) = layout.extents();
            let lr = Rect::from_xywh(
                x + logical.x() as f64 / k,
                top + logical.y() as f64 / k,
                (logical.width() as f64 / k).max(1.0),
                logical.height() as f64 / k,
            );
            advance = advance.max(logical.width() as f64 / k);
            bbox = Some(match bbox {
                Some(b) => b.union(&lr),
                None => lr,
            });
        }
        let outline = cr.copy_path().map(|p| cairo_to_subpaths(&p)).unwrap_or_default();
        cr.new_path();
        TextGeom {
            outline,
            bbox: bbox.unwrap_or(Rect::from_xywh(0.0, -size, 1.0, size)),
            advance,
        }
    })
}

pub fn text_geom(
    text: &str,
    family: &str,
    size: f64,
    bold: bool,
    italic: bool,
    fmt: &TextFormat,
) -> Rc<TextGeom> {
    cached_geom(text, family, size, bold, italic, fmt)
}

fn cached_geom(
    text: &str,
    family: &str,
    size: f64,
    bold: bool,
    italic: bool,
    fmt: &TextFormat,
) -> Rc<TextGeom> {
    let key: TextKey = (
        text.to_string(),
        family.to_string(),
        size.to_bits(),
        bold,
        italic,
        // Strichstärke, Kapitälchen und Ligaturen ändern die Glyphen
        fmt.weight as u64 | (fmt.small_caps as u64) << 16 | (fmt.no_ligatures as u64) << 17,
        fmt_key(fmt),
    );
    if let Some(g) = TEXT_CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return g;
    }
    let g = Rc::new(compute_text_geom(text, family, size, bold, italic, fmt));
    TEXT_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 2000 {
            c.clear();
        }
        c.insert(key, g.clone());
    });
    g
}

/// Grundlinie eines Pfades als Polylinie mit Bogenlängen.
use crate::model::point_at_length as point_at;

/// Zeichen von Text auf Pfad: (Byte-Index, Zeichen, Anfang auf dem Pfad, Glyphenbreite,
/// Vorschub inkl. Zeichenabstand) und die Grundlinie.
struct PathGlyphs {
    items: Vec<(usize, char, f64, f64, f64)>,
    pts: Vec<Pt>,
    len: Vec<f64>,
    size: f64,
    /// geschlossene Grundlinie: Positionen laufen um den Anfang herum weiter
    closed: bool,
    /// Text unter der Linie: Abstand der Grundlinie nach unten
    drop: f64,
    /// Bogenlänge hinter dem letzten Zeichen
    end: f64,
}

fn textpath_glyphs(shape: &Shape) -> Option<PathGlyphs> {
    let Shape::TextPath {
        path,
        text,
        family,
        size,
        bold,
        italic,
        offset,
        fmt,
    } = shape
    else {
        return None;
    };
    let (pts, len) = crate::model::textpath_line(path, fmt.path_reverse);
    let total = len.last().copied().unwrap_or(0.0);
    let track = fmt.tracking_px(*size);
    // jedes Zeichen einzeln, aber mit der Strichstärke des Textes
    let plain = TextFormat {
        weight: fmt.weight,
        small_caps: fmt.small_caps,
        ..TextFormat::default()
    };
    let mut items = Vec::new();
    let mut width = 0.0;
    for (i, ch) in text.char_indices() {
        let ch = if ch == '\n' { ' ' } else { ch };
        let w = text_geom(&ch.to_string(), family, *size, *bold, *italic, &plain).advance;
        items.push((i, ch, 0.0, w, w + track));
        width += w + track;
    }
    if !items.is_empty() {
        width -= track;
    }
    let anchor = crate::model::textpath_anchor(total, *offset, fmt.align);
    let mut pos = match fmt.align {
        Align::Center => anchor - width / 2.0,
        Align::Right => anchor - width,
        _ => anchor,
    };
    for it in items.iter_mut() {
        it.2 = pos;
        pos += it.4;
    }
    let end = if items.is_empty() { pos } else { pos - track };
    Some(PathGlyphs {
        items,
        pts,
        len,
        size: *size,
        end,
        closed: crate::model::path_is_closed(path),
        drop: if fmt.path_below { *size * 0.78 } else { 0.0 },
    })
}

impl PathGlyphs {
    /// Punkt und Winkel bei Bogenlänge `d` (bei geschlossenen Pfaden umlaufend), bereits
    /// um den Abstand für „unter der Linie“ verschoben.
    fn at(&self, d: f64) -> Option<(Pt, f64)> {
        let total = self.len.last().copied().unwrap_or(0.0);
        let d = if self.closed && total > 0.0 {
            d.rem_euclid(total)
        } else {
            d.clamp(0.0, total)
        };
        let (p, a) = point_at(&self.pts, &self.len, d)?;
        Some((p - up_vec(a) * self.drop, a))
    }
}

/// Glyphenumrisse für Text auf Pfad im Dokument.
pub fn textpath_outline(shape: &Shape) -> Vec<SubPath> {
    let Shape::TextPath {
        family,
        size,
        bold,
        italic,
        fmt,
        ..
    } = shape
    else {
        return Vec::new();
    };
    let Some(pg) = textpath_glyphs(shape) else {
        return Vec::new();
    };
    let total = pg.len.last().copied().unwrap_or(0.0);
    // jedes Zeichen einzeln, aber mit der Strichstärke des Textes
    let plain = TextFormat {
        weight: fmt.weight,
        small_caps: fmt.small_caps,
        ..TextFormat::default()
    };
    let mut out = Vec::new();
    for (_, ch, pos, w, _) in &pg.items {
        let mid = pos + w / 2.0;
        // offene Linie: Text endet an den Enden; geschlossene: läuft herum
        if !pg.closed {
            if mid > total {
                break;
            }
            if mid < 0.0 {
                continue;
            }
        }
        let g = text_geom(&ch.to_string(), family, *size, *bold, *italic, &plain);
        if let Some((p, ang)) = pg.at(mid) {
            let m = Affine::translate(p.x, p.y)
                .then_before(&Affine::rotate_deg(ang.to_degrees()))
                .then_before(&Affine::translate(-w / 2.0, 0.0));
            out.extend(map_subpaths(&g.outline, &m));
        }
    }
    out
}

/// Richtung „nach oben“ (senkrecht zur Grundlinie) bei Winkel `ang`.
fn up_vec(ang: f64) -> Pt {
    Pt::new(ang.sin(), -ang.cos())
}

fn textpath_caret(shape: &Shape, idx: usize) -> Option<(Pt, Pt)> {
    let pg = textpath_glyphs(shape)?;
    let at = pg
        .items
        .iter()
        .find(|it| it.0 >= idx)
        .map(|it| it.2)
        .unwrap_or(pg.end);
    let (p, ang) = pg.at(at)?;
    let up = up_vec(ang);
    Some((p + up * (pg.size * 0.95), p - up * (pg.size * 0.25)))
}

fn textpath_quads(shape: &Shape, a: usize, b: usize) -> Vec<[Pt; 4]> {
    let Some(pg) = textpath_glyphs(shape) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (i, _, pos, _, adv) in &pg.items {
        if *i < a || *i >= b {
            continue;
        }
        let (Some((pa, aa)), Some((pb, ab))) = (pg.at(*pos), pg.at(pos + adv)) else {
            continue;
        };
        let (ua, ub) = (up_vec(aa), up_vec(ab));
        let (hi, lo) = (pg.size * 0.95, pg.size * 0.25);
        out.push([pa + ua * hi, pb + ub * hi, pb - ub * lo, pa - ua * lo]);
    }
    out
}

fn textpath_index(shape: &Shape, p: Pt) -> Option<usize> {
    let Shape::TextPath { text, .. } = shape else {
        return None;
    };
    let pg = textpath_glyphs(shape)?;
    let l = crate::model::nearest_length(&pg.pts, &pg.len, p);
    for (i, _, pos, _, adv) in &pg.items {
        if l < pos + adv / 2.0 {
            return Some(*i);
        }
    }
    Some(text.len())
}

/// Läuft der Text eines Textfeldes über die Feldhöhe hinaus?
pub fn area_overflows(shape: &Shape) -> bool {
    match shape {
        Shape::TextArea { h, frame, .. } => {
            if frame.path.is_some() {
                return shape_geom(shape).map(|g| g.1).unwrap_or(false);
            }
            let i = frame.inset.max(0.0);
            rect_geom(shape).map(|g| g.bbox.y1).unwrap_or(0.0) > *h - 2.0 * i + 0.5
        }
        _ => false,
    }
}

/// Textumriss im Dokument (Text, Text auf Pfad, Textfeld).
fn text_doc_outline(shape: &Shape) -> Vec<SubPath> {
    match shape {
        Shape::TextArea { xf, frame, .. } => {
            if frame.path.is_some() {
                return shape_geom(shape)
                    .map(|g| map_subpaths(&g.0.outline, xf))
                    .unwrap_or_default();
            }
            let i = frame.inset.max(0.0);
            let Some(g) = rect_geom(shape) else {
                return Vec::new();
            };
            let m = xf.then_before(&Affine::translate(i, area_dy(shape)));
            map_subpaths(&g.outline, &m)
        }
        Shape::Text {
            xf,
            text,
            family,
            size,
            bold,
            italic,
            fmt,
        } => {
            let g = text_geom(text, family, *size, *bold, *italic, fmt);
            map_subpaths(&g.outline, xf)
        }
        Shape::TextPath { .. } => textpath_outline(shape),
        _ => Vec::new(),
    }
}

// ───────────── Echte Schrift (PDF) und Zeilen für den SVG-Export ─────────────

thread_local! {
    /// Text als echte Schrift statt als Umrisse zeichnen (PDF mit durchsuchbarem Text)
    static TEXT_GLYPHS: Cell<bool> = const { Cell::new(false) };
}

/// Text als echte Schrift zeichnen (für den PDF-Export); liefert den bisherigen Wert.
pub fn set_text_as_glyphs(on: bool) -> bool {
    TEXT_GLYPHS.with(|c| c.replace(on))
}

/// Gezeichnete Zeilen eines Textobjekts: Matrix und je Layout (Layout, x, y, nur erste
/// Zeile). Bei „nur erste Zeile“ ist (x, y) der Anfang der Grundlinie, sonst die linke obere
/// Ecke des Layouts – genau wie bei den Umrissen. Text auf Pfad: `None`.
fn text_runs(shape: &Shape) -> Option<(Affine, Vec<(pango::Layout, f64, f64, bool)>)> {
    let k = pango::SCALE as f64;
    match shape {
        Shape::Text {
            xf,
            text,
            family,
            size,
            bold,
            italic,
            fmt,
        } => {
            let pctx = plain_context();
            let fd = font_desc(family, *size, *bold, *italic);
            let mut v = Vec::new();
            for (i, line) in text.split('\n').enumerate() {
                let layout = line_layout(&pctx, line, &fd, fmt, *size);
                let baseline = layout.baseline() as f64 / k;
                let top = i as f64 * size * fmt.line - baseline;
                let x = line_shift(&layout, fmt);
                v.push((layout, x, top, false));
            }
            Some((*xf, v))
        }
        Shape::TextArea { xf, frame, .. } if frame.path.is_none() => {
            let (lines, _) = rect_lines(shape)?;
            let i = frame.inset.max(0.0);
            let m = xf.then_before(&Affine::translate(i, area_dy(shape)));
            Some((
                m,
                lines.into_iter().map(|l| (l.layout, l.x, l.top, false)).collect(),
            ))
        }
        Shape::TextArea { xf, .. } => {
            let (lines, _) = shape_lines(shape)?;
            let mut v = Vec::new();
            for l in lines {
                let mut it = l.layout.iter();
                let (_, lr) = it.line_extents();
                let x = l.x + lr.x() as f64 / k;
                let base = l.top + l.layout.baseline() as f64 / k;
                v.push((l.layout, x, base, true));
            }
            Some((*xf, v))
        }
        _ => None,
    }
}

/// Text mit der aktuellen Quelle als echte Schrift zeichnen (der Pfad bleibt erhalten).
fn show_text_runs(cr: &cairo::Context, m: &Affine, runs: &[(pango::Layout, f64, f64, bool)]) {
    let path = cr.copy_path();
    cr.new_path();
    let _ = cr.save();
    cr.transform(cairo::Matrix::new(m.a, m.b, m.c, m.d, m.e, m.f));
    for (layout, x, y, first) in runs {
        cr.move_to(*x, *y);
        if *first {
            if let Some(line) = layout.line_readonly(0) {
                pangocairo::functions::show_layout_line(cr, &line);
            }
        } else {
            pangocairo::functions::show_layout(cr, layout);
        }
    }
    let _ = cr.restore();
    cr.new_path();
    if let Ok(p) = &path {
        cr.append_path(p);
    }
}

/// Zeilen eines Textfeldes, wie sie gezeichnet werden (Koordinaten des Textfeldes, vor `xf`),
/// für den SVG-Export: so umbrechen auch Programme, die Textfelder nicht kennen, gleich.
pub fn area_export_lines(shape: &Shape) -> Option<Vec<crate::svg::TextLine>> {
    let k = pango::SCALE as f64;
    let Shape::TextArea { frame, fmt, text, .. } = shape else {
        return None;
    };
    let (lines, dx, dy) = if frame.path.is_none() {
        let (l, _) = rect_lines(shape)?;
        (l, frame.inset.max(0.0), area_dy(shape))
    } else {
        let (l, _) = shape_lines(shape)?;
        (l, 0.0, 0.0)
    };
    let mut out = Vec::new();
    for l in &lines {
        let shown = l.layout.text().to_string();
        let prefix = l.map.first().map(|m| m.2).unwrap_or(0).min(shown.len());
        let count = if l.first_only {
            1
        } else {
            l.layout.line_count().max(0) as usize
        };
        let mut it = l.layout.iter();
        for li in 0..count {
            if li > 0 && !it.next_line() {
                break;
            }
            let Some(line) = l.layout.line_readonly(li as i32) else {
                break;
            };
            let ds = (line.start_index().max(0) as usize).min(shown.len());
            let de = (ds + line.length().max(0) as usize).min(shown.len());
            let (_, lr) = it.line_extents();
            let baseline = l.top + it.baseline() as f64 / k;
            let lx = l.x + lr.x() as f64 / k;
            // Aufzählungszeichen (vor dem Tabulator) getrennt vom Text
            let (marker, ts) = if ds < prefix {
                let end = prefix.min(de);
                (shown[ds..end].trim_end_matches('\t').to_string(), end)
            } else {
                (String::new(), ds)
            };
            let text_x = if ts > ds {
                l.x + l.layout.index_to_pos(ts as i32).x() as f64 / k
            } else {
                lx
            };
            let to_orig = |d: usize| (l.start + d.saturating_sub(prefix)).min(text.len());
            let (a, mut b) = (to_orig(ts), to_orig(de));
            while b > a && text.is_char_boundary(b) && text[..b].ends_with(['\u{2028}', '\n']) {
                b -= text[..b].chars().next_back().map(char::len_utf8).unwrap_or(1);
            }
            let right = lx + lr.width() as f64 / k;
            let last_of_para = li + 1 >= l.layout.line_count().max(1) as usize;
            out.push(crate::svg::TextLine {
                start: a,
                end: b.max(a),
                marker,
                x: lx + dx,
                text_x: text_x + dx,
                baseline: baseline + dy,
                width: (right - text_x).max(0.0),
                justify: fmt.align == Align::Justify && !last_of_para,
            });
        }
    }
    Some(out)
}

/// Ein Objekt samt Effekten (bei Glas mit dem gebrochenen Hintergrund) als PNG, für
/// Programme, die die Effekte von Stijl nicht kennen. `res`: Bildpixel je Dokumenteinheit.
/// Liefert die PNG-Daten und die Lage im Dokument.
pub fn rasterize_object(doc: &Document, id: u64, res: f64) -> Option<(Vec<u8>, Rect)> {
    let obj = doc
        .layers
        .iter()
        .flat_map(|l| l.objects.iter())
        .find(|o| o.id == id)?;
    let mut b = object_box(obj)?;
    let grow = obj.style.width.abs()
        + obj
            .style
            .shadow
            .map(|s| s.blur * 3.0 + s.dx.abs().max(s.dy.abs()))
            .unwrap_or(0.0)
        + 4.0;
    b = b.inflate(grow);
    if obj.style.fx.mirror.is_some() {
        b.y1 += b.height();
    }
    // nur der Teil auf der Seite
    let b = Rect {
        x0: b.x0.max(0.0),
        y0: b.y0.max(0.0),
        x1: b.x1.min(doc.width),
        y1: b.y1.min(doc.height),
    };
    if b.width() <= 0.0 || b.height() <= 0.0 {
        return None;
    }
    let mut res = res.max(0.1);
    while b.width() * res * b.height() * res > 36_000_000.0 && res > 0.2 {
        res *= 0.8;
    }
    let (w, h) = ((b.width() * res).ceil() as i32, (b.height() * res).ceil() as i32);
    let surf = cairo::ImageSurface::create(cairo::Format::ARgb32, w.max(1), h.max(1)).ok()?;
    {
        let cr = cairo::Context::new(&surf).ok()?;
        cr.scale(res, res);
        cr.translate(-b.x0, -b.y0);
        let prev = set_effect_resolution(1.0);
        if obj.style.fx.needs_glass() {
            draw_glass(&cr, doc, obj);
        } else {
            draw_object(&cr, obj);
        }
        set_effect_resolution(prev);
    }
    let mut surf = surf;
    surf.flush();
    // auf den sichtbaren Teil zuschneiden
    let (wu, hu) = (surf.width() as usize, surf.height() as usize);
    let stride = surf.stride() as usize;
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    {
        let d = surf.data().ok()?;
        for y in 0..hu {
            for x in 0..wu {
                let v = u32::from_ne_bytes([
                    d[y * stride + x * 4],
                    d[y * stride + x * 4 + 1],
                    d[y * stride + x * 4 + 2],
                    d[y * stride + x * 4 + 3],
                ]);
                if v >> 24 != 0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
    }
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    let crop = cairo::ImageSurface::create(cairo::Format::ARgb32, (x1 - x0) as i32, (y1 - y0) as i32).ok()?;
    {
        let cr = cairo::Context::new(&crop).ok()?;
        let _ = cr.set_source_surface(&surf, -(x0 as f64), -(y0 as f64));
        let _ = cr.paint();
    }
    let mut png: Vec<u8> = Vec::new();
    crop.write_to_png(&mut png).ok()?;
    Some((
        png,
        Rect {
            x0: b.x0 + x0 as f64 / res,
            y0: b.y0 + y0 as f64 / res,
            x1: b.x0 + x1 as f64 / res,
            y1: b.y0 + y1 as f64 / res,
        },
    ))
}

/// Pipette: sichtbare Farbe an der Bildschirmstelle (sx, sy) der Zeichenfläche, wie sie auf
/// der Seite erscheint (mit Effekten und Bildern). Außerhalb der Seiten: `None`.
pub fn sample_color(ed: &Editor, sx: f64, sy: f64) -> Option<Color> {
    let v = ed.view;
    let doc = &ed.doc;
    let p = v.to_doc(Pt::new(sx, sy));
    let rects = doc.artboard_rects();
    if !rects
        .iter()
        .any(|(_, r)| p.x >= r.x0 && p.x <= r.x1 && p.y >= r.y0 && p.y <= r.y1)
    {
        return None;
    }
    let mut surf = cairo::ImageSurface::create(cairo::Format::ARgb32, 1, 1).ok()?;
    {
        let cr = cairo::Context::new(&surf).ok()?;
        cr.translate(-sx.floor(), -sy.floor());
        cr.translate(v.ox, v.oy);
        cr.scale(v.zoom, v.zoom);
        // Seite: Hintergrund des Dokuments, sonst Weiß
        for (_, r) in &rects {
            cr.rectangle(r.x0, r.y0, r.width(), r.height());
        }
        cr.set_source_rgb(1.0, 1.0, 1.0);
        let _ = cr.fill_preserve();
        match doc.background {
            Some(bg) => {
                set_color(&cr, bg);
                let _ = cr.fill();
            }
            None => cr.new_path(),
        }
        draw_document(&cr, doc);
    }
    surf.flush();
    let d = surf.data().ok()?;
    let px = u32::from_ne_bytes([d[0], d[1], d[2], d[3]]);
    let a = (px >> 24) as f64 / 255.0;
    if a <= 0.0 {
        return None;
    }
    let ch = |k: u32| (((px >> k) & 255) as f64 / 255.0 / a).clamp(0.0, 1.0);
    Some(Color {
        r: ch(16),
        g: ch(8),
        b: ch(0),
        a: 1.0,
    })
}

/// Lupe der Pipette: Ring in der aufgenommenen Farbe um den Zeiger.
pub fn draw_pipette(cr: &cairo::Context, x: f64, y: f64, c: Option<Color>) {
    let _ = cr.save();
    let (r_out, r_in) = (26.0, 12.0);
    cr.new_path();
    cr.arc(x, y, r_out, 0.0, TAU);
    cr.arc_negative(x, y, r_in, TAU, 0.0);
    match c {
        Some(c) => cr.set_source_rgb(c.r, c.g, c.b),
        None => cr.set_source_rgba(0.5, 0.5, 0.5, 0.35),
    }
    let _ = cr.fill();
    for (r, w, col) in [
        (r_out + 1.0, 2.0, 1.0),
        (r_out + 2.5, 1.0, 0.0),
        (r_in - 1.0, 2.0, 1.0),
        (r_in - 2.5, 1.0, 0.0),
    ] {
        cr.new_path();
        cr.arc(x, y, r, 0.0, TAU);
        cr.set_source_rgba(col, col, col, if col > 0.5 { 0.95 } else { 0.45 });
        cr.set_line_width(w);
        let _ = cr.stroke();
    }
    let _ = cr.restore();
}

/// Rahmen eines Objekts (ohne Strichbreite) – Grundlage für Auswahl und Farbverläufe.
pub fn object_box(obj: &Object) -> Option<Rect> {
    match &obj.shape {
        Shape::Text {
            xf,
            text,
            family,
            size,
            bold,
            italic,
            fmt,
        } => {
            let b = text_geom(text, family, *size, *bold, *italic, fmt).bbox;
            // mit Kasten zählt der Kasten
            let b = if fmt.has_box() {
                b.inflate(fmt.box_inset.max(0.0))
            } else {
                b
            };
            let mut r = Rect::from_points(xf.apply(Pt::new(b.x0, b.y0)), xf.apply(Pt::new(b.x1, b.y0)));
            r.include(xf.apply(Pt::new(b.x1, b.y1)));
            r.include(xf.apply(Pt::new(b.x0, b.y1)));
            Some(r)
        }
        Shape::TextPath { path, fmt, .. } => {
            let o = subpaths_bbox(&textpath_outline(&obj.shape)).map(|b| {
                if fmt.has_box() {
                    b.inflate(fmt.box_inset.max(0.0))
                } else {
                    b
                }
            });
            let p = subpaths_bbox(path);
            match (o, p) {
                (Some(a), Some(b)) => Some(a.union(&b)),
                (a, b) => a.or(b),
            }
        }
        s => s.geom_bbox(),
    }
}

// ───────────────────────────── Bilder ─────────────────────────────

fn decode_image(data: &[u8]) -> Option<cairo::ImageSurface> {
    let loader = gdk_pixbuf::PixbufLoader::new();
    loader.write(data).ok()?;
    loader.close().ok()?;
    let pb = loader.pixbuf()?;
    let (w, h) = (pb.width(), pb.height());
    let n = pb.n_channels() as usize;
    let alpha = pb.has_alpha();
    let rs = pb.rowstride() as usize;
    let bytes = pb.read_pixel_bytes();
    let src: &[u8] = &bytes;
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).ok()?;
    let stride = surface.stride() as usize;
    {
        let mut dst = surface.data().ok()?;
        for y in 0..h as usize {
            for x in 0..w as usize {
                let i = y * rs + x * n;
                if i + n > src.len() {
                    continue;
                }
                let a = if alpha { src[i + 3] as u32 } else { 255 };
                let pm = |v: u8| (v as u32 * a + 127) / 255;
                let v = (a << 24) | (pm(src[i]) << 16) | (pm(src[i + 1]) << 8) | pm(src[i + 2]);
                let o = y * stride + x * 4;
                dst[o..o + 4].copy_from_slice(&v.to_ne_bytes());
            }
        }
    }
    surface.mark_dirty();
    Some(surface)
}

fn image_surface(data: &Rc<Vec<u8>>) -> Option<cairo::ImageSurface> {
    let key = Rc::as_ptr(data) as usize;
    let cached = IMAGE_CACHE.with(|c| {
        c.borrow()
            .get(&key)
            .and_then(|(w, s)| w.upgrade().filter(|rc| Rc::ptr_eq(rc, data)).map(|_| s.clone()))
    });
    if let Some(s) = cached {
        return s;
    }
    let s = decode_image(data);
    IMAGE_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.retain(|_, (w, _)| w.strong_count() > 0);
        c.insert(key, (Rc::downgrade(data), s.clone()));
    });
    s
}

/// Pixelgröße einer Bilddatei (zum Einfügen).
pub fn image_size(data: &[u8]) -> Option<(f64, f64)> {
    let s = decode_image(data)?;
    Some((s.width() as f64, s.height() as f64))
}

/// Längste Kante, auf die Bilder für Filter verkleinert werden (Rechenzeit).
const IMAGE_FX_MAX: i32 = 2400;

/// Bild mit Kunsteffekt und Farbfilter. Deckkraft bleibt unverändert. Ohne Filter das Original.
fn image_surface_fx(data: &Rc<Vec<u8>>, fx: &Effects) -> Option<cairo::ImageSurface> {
    if fx.art.is_none() && fx.tint.is_none() && fx.temp.abs() < 1e-9 {
        return image_surface(data);
    }
    let key = format!(
        "{:?}|{:?}|{}",
        fx.art.map(|(a, k)| (a, k.to_bits())),
        fx.tint.map(|(f, k)| (f, k.to_bits())),
        fx.temp.to_bits()
    );
    let id = Rc::as_ptr(data) as usize;
    let cached = IMAGE_FX_CACHE.with(|c| {
        c.borrow().get(&id).and_then(|(w, k, s)| {
            (k == &key && w.upgrade().is_some_and(|rc| Rc::ptr_eq(&rc, data))).then(|| s.clone())
        })
    });
    if let Some(s) = cached {
        return s;
    }
    let result = (|| {
        let src = image_surface(data)?;
        let (sw, sh) = (src.width(), src.height());
        if sw <= 0 || sh <= 0 {
            return None;
        }
        let f = (IMAGE_FX_MAX as f64 / sw.max(sh) as f64).min(1.0);
        let (w, h) = (
            ((sw as f64 * f).round() as i32).max(1),
            ((sh as f64 * f).round() as i32).max(1),
        );
        let mut out = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).ok()?;
        {
            let cx = cairo::Context::new(&out).ok()?;
            cx.scale(f, f);
            let p = cairo::SurfacePattern::create(&src);
            p.set_filter(cairo::Filter::Good);
            cx.set_source(&p).ok()?;
            cx.paint().ok()?;
        }
        out.flush();
        let stride = out.stride() as usize;
        {
            let mut d = out.data().ok()?;
            let mut buf = crate::imgfx::Buf {
                data: &mut d,
                w: w as usize,
                h: h as usize,
                stride,
            };
            if let Some((art, k)) = fx.art {
                match art {
                    Art::Pixel => {
                        let cell = (w.max(h) as f64 * (0.012 + 0.04 * k)).round() as usize;
                        crate::imgfx::pixelate(&mut buf, cell.max(2));
                    }
                    Art::Grain => crate::imgfx::grain(&mut buf, k),
                    Art::Watercolor => crate::imgfx::watercolor(&mut buf, k),
                }
            }
            crate::imgfx::temperature(&mut buf, fx.temp);
            if let Some((t, k)) = fx.tint {
                crate::imgfx::tint(&mut buf, t, k);
            }
        }
        out.mark_dirty();
        Some(out)
    })();
    IMAGE_FX_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.retain(|_, (w, _, _)| w.strong_count() > 0);
        c.insert(id, (Rc::downgrade(data), key, result.clone()));
    });
    result
}

/// Bild als unvormultiplizierte RGBA-Bytes, auf höchstens `max_dim` Pixel Kantenlänge verkleinert.
pub fn image_rgba(data: &Rc<Vec<u8>>, max_dim: i32) -> Option<(Vec<u8>, usize, usize)> {
    let src = image_surface(data)?;
    let (sw, sh) = (src.width(), src.height());
    if sw <= 0 || sh <= 0 {
        return None;
    }
    let f = (max_dim as f64 / sw.max(sh) as f64).min(1.0);
    let (w, h) = (
        ((sw as f64 * f).round() as i32).max(1),
        ((sh as f64 * f).round() as i32).max(1),
    );
    let mut out = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).ok()?;
    {
        let cx = cairo::Context::new(&out).ok()?;
        cx.scale(f, f);
        let p = cairo::SurfacePattern::create(&src);
        p.set_filter(cairo::Filter::Good);
        cx.set_source(&p).ok()?;
        cx.paint().ok()?;
    }
    out.flush();
    let stride = out.stride() as usize;
    let d = out.data().ok()?;
    Some((
        crate::imgfx::to_rgba(&d, w as usize, h as usize, stride),
        w as usize,
        h as usize,
    ))
}

/// Zeigt das Bild flache Farbflächen (Logo, Illustration)? Das Ergebnis wird je Bild gemerkt.
pub fn image_is_flat(data: &Rc<Vec<u8>>) -> bool {
    let id = Rc::as_ptr(data) as usize;
    let cached = FLAT_CACHE.with(|c| {
        c.borrow()
            .get(&id)
            .and_then(|(w, v)| w.upgrade().is_some_and(|rc| Rc::ptr_eq(&rc, data)).then_some(*v))
    });
    if let Some(v) = cached {
        return v;
    }
    let v = image_rgba(data, 400)
        .map(|(px, w, h)| crate::imgfx::is_flat(&px, w, h))
        .unwrap_or(false);
    FLAT_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.retain(|_, (w, _)| w.strong_count() > 0);
        c.insert(id, (Rc::downgrade(data), v));
    });
    v
}

fn draw_image(cr: &cairo::Context, xf: &Affine, w: f64, h: f64, data: &Rc<Vec<u8>>, fx: &Effects) {
    if !invertible(xf) {
        return;
    }
    let _ = cr.save();
    cr.transform(cairo::Matrix::new(xf.a, xf.b, xf.c, xf.d, xf.e, xf.f));
    cr.rectangle(0.0, 0.0, w, h);
    cr.clip();
    match image_surface_fx(data, fx) {
        Some(surf) if surf.width() > 0 && surf.height() > 0 => {
            cr.scale(w / surf.width() as f64, h / surf.height() as f64);
            // Randpixel fortsetzen: sonst mischt die Glättung beim Vergrößern die Ränder mit
            // „nichts“ und es entsteht ein heller, halbdurchsichtiger Streifen am Bildrand
            let pat = cairo::SurfacePattern::create(&surf);
            // (nicht im PDF: dort zeichnen die Anzeigeprogramme selbst und Cairo würde das
            // Bild sonst neu rastern)
            if cr.target().type_() != cairo::SurfaceType::Pdf {
                pat.set_extend(cairo::Extend::Pad);
            }
            let _ = cr.set_source(&pat);
            let _ = cr.paint();
        }
        _ => {
            // Nicht lesbares Bild: graues Platzhalterfeld
            cr.set_source_rgb(0.75, 0.75, 0.78);
            let _ = cr.paint();
        }
    }
    let _ = cr.restore();
}

// ───────────────────────────── Formen ─────────────────────────────

/// Legt die Kontur einer Form als aktuellen Cairo-Pfad an (Dokumentkoordinaten).
pub fn build_path(cr: &cairo::Context, shape: &Shape) {
    match shape {
        Shape::Rect { x, y, w, h, r } if *r <= 0.0 => cr.rectangle(*x, *y, *w, *h),
        Shape::Line { a, b } => {
            cr.move_to(a.x, a.y);
            cr.line_to(b.x, b.y);
        }
        Shape::Path { subpaths, .. } => path_subpaths(cr, &crate::model::effective_subpaths(subpaths)),
        Shape::Text { .. } | Shape::TextPath { .. } | Shape::TextArea { .. } => {
            path_subpaths(cr, &text_doc_outline(shape))
        }
        Shape::Image { clip: Some(c), .. } => path_subpaths(cr, &crate::model::effective_subpaths(c)),
        Shape::Image { xf, w, h, .. } => {
            let c = [
                xf.apply(Pt::new(0.0, 0.0)),
                xf.apply(Pt::new(*w, 0.0)),
                xf.apply(Pt::new(*w, *h)),
                xf.apply(Pt::new(0.0, *h)),
            ];
            cr.move_to(c[0].x, c[0].y);
            for p in &c[1..] {
                cr.line_to(p.x, p.y);
            }
            cr.close_path();
        }
        other => {
            if let Some(p) = other.to_path() {
                build_path(cr, &p);
            }
        }
    }
}

/// Cairo-Operator für einen Mischmodus.
///
/// „Aufhellen“ ist additiv (wie überlagerte Lichtspots): Farben addieren sich und werden
/// erst bei vielen Überlagerungen weiß. Mit `Screen` wurde dagegen schon bei wenigen
/// gesättigten Flächen alles weiß. „Abdunkeln“ bleibt multiplikativ, also wie
/// übereinandergelegtes farbiges Glas, das Richtung Schwarz läuft.
fn blend_operator(b: Blend) -> cairo::Operator {
    match b {
        Blend::Normal => cairo::Operator::Over,
        Blend::Multiply => cairo::Operator::Multiply,
        Blend::Screen => cairo::Operator::Add,
        Blend::Overlay => cairo::Operator::Overlay,
        Blend::Darken => cairo::Operator::Darken,
        Blend::Lighten => cairo::Operator::Lighten,
        Blend::ColorDodge => cairo::Operator::ColorDodge,
        Blend::ColorBurn => cairo::Operator::ColorBurn,
        Blend::HardLight => cairo::Operator::HardLight,
        Blend::SoftLight => cairo::Operator::SoftLight,
        Blend::Difference => cairo::Operator::Difference,
        Blend::Exclusion => cairo::Operator::Exclusion,
        Blend::Hue => cairo::Operator::HslHue,
        Blend::Saturation => cairo::Operator::HslSaturation,
        Blend::Color => cairo::Operator::HslColor,
        Blend::Luminosity => cairo::Operator::HslLuminosity,
    }
}

pub fn draw_object(cr: &cairo::Context, obj: &Object) {
    if obj.style.opacity <= 0.0 {
        return;
    }
    draw_extras(cr, obj);
    draw_object_body(cr, obj);
}

/// Extras unter dem Objekt: Schatten oder Spiegelung.
fn draw_extras(cr: &cairo::Context, obj: &Object) {
    draw_shadow(cr, obj);
    draw_mirror(cr, obj);
}

/// Spiegelung: das Objekt kopfüber unter sich, nach unten ausblendend.
fn draw_mirror(cr: &cairo::Context, obj: &Object) {
    let Some(k) = obj.style.fx.mirror else { return };
    let Some(b) = object_box(obj) else { return };
    if k <= 0.0 || b.height() <= 0.0 {
        return;
    }
    let gap = (b.height() * 0.02).min(4.0);
    let _ = cr.save();
    cr.push_group();
    // an der Unterkante spiegeln
    cr.translate(0.0, 2.0 * b.y1 + gap);
    cr.scale(1.0, -1.0);
    let mut o = obj.clone();
    o.style.fx.mirror = None;
    o.style.shadow = None;
    draw_object_body(cr, &o);
    let Ok(pat) = cr.pop_group() else {
        let _ = cr.restore();
        return;
    };
    let fade = cairo::LinearGradient::new(0.0, b.y1 + gap, 0.0, b.y1 + gap + b.height() * 0.75);
    fade.add_color_stop_rgba(0.0, 0.0, 0.0, 0.0, (0.15 + 0.6 * k).min(0.9));
    fade.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, 0.0);
    let _ = cr.set_source(&pat);
    let _ = cr.mask(&fade);
    let _ = cr.restore();
}

/// Schlagschatten: Form des Objekts deckend in einen Zwischenspeicher zeichnen,
/// verschieben, weichzeichnen und in der Schattenfarbe auftragen.
fn draw_shadow(cr: &cairo::Context, obj: &Object) {
    let Some(sh) = obj.style.shadow else { return };
    let alpha = sh.opacity * obj.style.opacity;
    if alpha <= 0.0 {
        return;
    }
    let _ = (|| -> Option<()> {
        let b = object_box(obj)?.inflate(obj.style.width.max(0.0));
        let b = Rect {
            x0: b.x0 + sh.dx,
            y0: b.y0 + sh.dy,
            x1: b.x1 + sh.dx,
            y1: b.y1 + sh.dy,
        };
        let (ctm, area) = raster_area(cr, &b, &|scale| sh.blur * scale * 2.0 + 2.0)?;
        let key = fx_key(obj, &ctm, area, 0);
        let (surf, m) = match fx_lookup(obj, SLOT_SHADOW, key) {
            Some((e, map)) => (e.surf?, map),
            None => {
                let (ix0, iy0, w, h) = area;
                let scale = (ctm.xx() * ctm.yy() - ctm.xy() * ctm.yx()).abs().sqrt().max(1e-6);
                let radius = sh.blur * scale;
                let mut surf = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).ok()?;
                {
                    let c2 = cairo::Context::new(&surf).ok()?;
                    c2.translate(-ix0, -iy0);
                    c2.transform(ctm);
                    c2.translate(sh.dx, sh.dy);
                    let mut solid = obj.clone();
                    solid.style.fill = solid.style.fill.map(|_| Color::BLACK);
                    solid.style.stroke = solid.style.stroke.map(|_| Color::BLACK);
                    solid.style.gradient = None;
                    solid.style.opacity = 1.0;
                    solid.style.blend = Blend::Normal;
                    solid.style.fx = crate::model::Effects::default();
                    solid.style.shadow = None;
                    draw_object_body(&c2, &solid);
                }
                surf.flush();
                if radius >= 0.5 {
                    blur_surface(&mut surf, radius);
                }
                let m = to_image(&ctm, ix0, iy0);
                fx_store(obj, SLOT_SHADOW, key, Some(surf.clone()), None, m);
                (surf, m)
            }
        };
        let sp = cairo::SurfacePattern::create(&surf);
        sp.set_matrix(m);
        let _ = cr.save();
        match (obj.style.grad(crate::model::PaintTarget::Shadow), object_box(obj)) {
            (Some(g), Some(r)) => {
                // Schatten mit Verlauf: Verlauf durch die Schattenform auftragen
                let r = Rect {
                    x0: r.x0 + sh.dx,
                    y0: r.y0 + sh.dy,
                    x1: r.x1 + sh.dx,
                    y1: r.y1 + sh.dy,
                };
                cr.push_group();
                set_gradient(cr, g, &r);
                let _ = cr.mask(&sp);
                let _ = cr.pop_group_to_source();
                let _ = cr.paint_with_alpha(alpha.clamp(0.0, 1.0));
            }
            _ => {
                cr.set_source_rgba(sh.color.r, sh.color.g, sh.color.b, alpha.clamp(0.0, 1.0));
                let _ = cr.mask(&sp);
            }
        }
        let _ = cr.restore();
        Some(())
    })();
}

/// Objekt ohne Schatten zeichnen (Deckkraft und Mischmodus berücksichtigt).
fn draw_object_body(cr: &cairo::Context, obj: &Object) {
    let op = obj.style.opacity.clamp(0.0, 1.0);
    if op <= 0.0 {
        return;
    }
    let blend = obj.style.blend;
    if op < 0.999 || blend != Blend::Normal {
        // Objekt einzeln zeichnen und dann mit Deckkraft und Mischmodus auftragen
        cr.push_group();
        draw_shape(cr, obj);
        let _ = cr.pop_group_to_source();
        cr.set_operator(blend_operator(blend));
        let _ = cr.paint_with_alpha(op);
        cr.set_operator(cairo::Operator::Over);
        return;
    }
    draw_shape(cr, obj);
}

/// Farbverlauf bzw. Muster als Cairo-Quelle setzen.
fn set_gradient(cr: &cairo::Context, g: &Gradient, r: &Rect) {
    use crate::model::GradShape;
    match g.shape {
        GradShape::Linear => {
            let (a, b) = g.line(r);
            let lg = cairo::LinearGradient::new(a.x, a.y, b.x, b.y);
            for (pos, c) in g.render_stops() {
                lg.add_color_stop_rgba(pos.clamp(0.0, 1.0), c.r, c.g, c.b, c.a);
            }
            lg.set_extend(cairo::Extend::Pad);
            let _ = cr.set_source(&lg);
        }
        GradShape::Radial => {
            // im Rahmen des Objekts gerechnet (Kreis bei quadratischem Rahmen, sonst Ellipse)
            let rad = g.p0.dist(g.p1).max(1e-4);
            let rg = cairo::RadialGradient::new(g.p0.x, g.p0.y, 0.0, g.p0.x, g.p0.y, rad);
            for (pos, c) in g.render_stops() {
                rg.add_color_stop_rgba(pos.clamp(0.0, 1.0), c.r, c.g, c.b, c.a);
            }
            rg.set_extend(cairo::Extend::Pad);
            let (w, h) = (r.width().max(1e-6), r.height().max(1e-6));
            // Dokument → Einheitsrahmen
            rg.set_matrix(cairo::Matrix::new(
                1.0 / w,
                0.0,
                0.0,
                1.0 / h,
                -r.x0 / w,
                -r.y0 / h,
            ));
            let _ = cr.set_source(&rg);
        }
        _ => set_pattern(cr, g, r),
    }
}

/// Zweifarbiges Muster (Schachbrett, Punkte, Streifen) als Vektor-Kachel; Lage und Winkel
/// folgen der Verlaufslinie, die Größe `g.cell`.
fn set_pattern(cr: &cairo::Context, g: &Gradient, r: &Rect) {
    use crate::model::GradShape;
    let c1 = g.stops.first().map(|s| s.color).unwrap_or(Color::WHITE);
    let c2 = g.stops.get(1).map(|s| s.color).unwrap_or(Color::BLACK);
    let k = g.cell.max(0.5);
    let tile = if g.shape == GradShape::Stripes { k } else { 2.0 * k };
    let Ok(rec) = cairo::RecordingSurface::create(
        cairo::Content::ColorAlpha,
        Some(cairo::Rectangle::new(0.0, 0.0, tile, tile)),
    ) else {
        set_color(cr, c1);
        return;
    };
    if let Ok(t) = cairo::Context::new(&rec) {
        t.rectangle(0.0, 0.0, tile, tile);
        set_color(&t, c1);
        let _ = t.fill();
        set_color(&t, c2);
        match g.shape {
            GradShape::Checker => {
                t.rectangle(0.0, 0.0, k, k);
                t.rectangle(k, k, k, k);
            }
            GradShape::Dots => {
                t.arc(k * 0.5, k * 0.5, k * 0.3, 0.0, TAU);
                t.new_sub_path();
                t.arc(k * 1.5, k * 1.5, k * 0.3, 0.0, TAU);
            }
            _ => t.rectangle(0.0, 0.0, k * 0.5, k),
        }
        let _ = t.fill();
    }
    let pat = cairo::SurfacePattern::create(&rec);
    pat.set_extend(cairo::Extend::Repeat);
    // Kachel → Dokument: verschieben an den Anfang der Linie, drehen in ihre Richtung
    let (a, b) = g.line(r);
    let th = (b.y - a.y).atan2(b.x - a.x);
    let (cs, sn) = (th.cos(), th.sin());
    pat.set_matrix(cairo::Matrix::new(
        cs,
        -sn,
        sn,
        cs,
        -(cs * a.x + sn * a.y),
        sn * a.x - cs * a.y,
    ));
    let _ = cr.set_source(&pat);
}

/// Graues Rauschen (einmal erzeugt) für gekörnte Verläufe.
fn noise_surface() -> Option<cairo::ImageSurface> {
    if let Some(s) = NOISE.with(|n| n.borrow().clone()) {
        return Some(s);
    }
    let n = 256;
    let mut surf = cairo::ImageSurface::create(cairo::Format::Rgb24, n, n).ok()?;
    let stride = surf.stride() as usize;
    {
        let mut data = surf.data().ok()?;
        let mut x: u32 = 0x9E37_79B9;
        for y in 0..n as usize {
            for i in 0..n as usize {
                // xorshift32
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let v = 128 + ((x >> 24) as i32 - 128) * 3 / 4;
                let v = v.clamp(0, 255) as u32;
                let px = (v << 16) | (v << 8) | v;
                let o = y * stride + i * 4;
                data[o..o + 4].copy_from_slice(&px.to_ne_bytes());
            }
        }
    }
    surf.mark_dirty();
    NOISE.with(|c| *c.borrow_mut() = Some(surf.clone()));
    Some(surf)
}

/// Körnung über die aktuelle Füllfläche legen (Pfad bleibt erhalten).
fn draw_grain(cr: &cairo::Context, k: f64) {
    let Some(surf) = noise_surface() else { return };
    let p = cairo::SurfacePattern::create(&surf);
    p.set_extend(cairo::Extend::Repeat);
    let _ = cr.save();
    cr.clip_preserve();
    let _ = cr.set_source(&p);
    cr.set_operator(cairo::Operator::Overlay);
    let _ = cr.paint_with_alpha((0.1 + 0.9 * k).clamp(0.0, 1.0));
    let _ = cr.restore();
}

fn draw_shape(cr: &cairo::Context, obj: &Object) {
    if let Shape::Image { xf, w, h, data, clip } = &obj.shape {
        match clip {
            Some(c) => {
                // Bild im Rahmen: nur innerhalb sichtbar, Rahmenlinie darüber
                let _ = cr.save();
                cr.new_path();
                path_subpaths(cr, &crate::model::effective_subpaths(c));
                cr.clip();
                draw_image(cr, xf, *w, *h, data, &obj.style.fx);
                let _ = cr.restore();
                if let Some(sc) = obj.style.stroke.filter(|_| obj.style.width > 0.0) {
                    cr.new_path();
                    path_subpaths(cr, &crate::model::effective_subpaths(c));
                    set_color(cr, sc);
                    cr.set_line_width(obj.style.width);
                    cr.set_line_join(cairo_join(obj.style.join));
                    let _ = cr.stroke();
                }
            }
            None => draw_image(cr, xf, *w, *h, data, &obj.style.fx),
        }
        return;
    }
    // Textfelder: Rahmen zeichnen, rechteckige Felder auf ihre Fläche beschneiden
    let clipped = if let Shape::TextArea { xf, w, h, frame, .. } = &obj.shape {
        draw_text_frame(cr, xf, *w, *h, frame);
        if frame.path.is_none() {
            let _ = cr.save();
            cr.new_path();
            box_path(cr, xf, *w, *h);
            cr.clip();
            true
        } else {
            false
        }
    } else {
        false
    };
    // Kasten hinter Punkttext und Text auf Pfad
    if let (Some((m, w, h)), Some(fmt)) = (text_box_geom(&obj.shape), obj.shape.text_format()) {
        draw_text_frame(cr, &m, w, h, &fmt.text_box());
    }
    draw_filled(cr, obj);
    if clipped {
        let _ = cr.restore();
    }
}

fn box_path(cr: &cairo::Context, xf: &Affine, w: f64, h: f64) {
    let c = [
        xf.apply(Pt::new(0.0, 0.0)),
        xf.apply(Pt::new(w, 0.0)),
        xf.apply(Pt::new(w, h)),
        xf.apply(Pt::new(0.0, h)),
    ];
    cr.move_to(c[0].x, c[0].y);
    for p in &c[1..] {
        cr.line_to(p.x, p.y);
    }
    cr.close_path();
}

fn draw_filled(cr: &cairo::Context, obj: &Object) {
    cr.new_path();
    build_path(cr, &obj.shape);
    let evenodd = matches!(obj.shape, Shape::Path { evenodd: true, .. });
    cr.set_fill_rule(if evenodd {
        cairo::FillRule::EvenOdd
    } else {
        cairo::FillRule::Winding
    });
    let stroke = obj.style.stroke.filter(|_| obj.style.width > 0.0);
    if let Some(f) = obj.style.fill {
        let grad = obj
            .style
            .gradient
            .as_ref()
            .and_then(|g| object_box(obj).map(|r| (g, r)));
        // Kunsteffekte wirken auf die Füllung (Farbe, Verlauf oder Muster)
        let art = obj.style.fx.art;
        // PDF mit durchsuchbarem Text: echte Schrift statt Umrissen (nicht bei Kunsteffekten)
        let glyphs = if art.is_none() && TEXT_GLYPHS.with(|c| c.get()) {
            text_runs(&obj.shape)
        } else {
            None
        };
        match art {
            Some((Art::Pixel, k)) => {
                let cell = PIXEL_CELL * (0.4 + 2.2 * k);
                match &grad {
                    Some((g, r)) => draw_pixel_gradient(cr, g, r, cell),
                    None => match object_box(obj) {
                        Some(r) => draw_pixel_gradient(cr, &Gradient::new(f, f), &r, cell),
                        None => {
                            set_color(cr, f);
                            let _ = cr.fill_preserve();
                        }
                    },
                }
            }
            Some((Art::Watercolor, k)) => {
                let paint = |c: &cairo::Context| match &grad {
                    Some((g, r)) => set_gradient(c, g, r),
                    None => set_color(c, f),
                };
                if !draw_watercolor(cr, obj, &paint, k) {
                    paint(cr);
                    let _ = cr.fill_preserve();
                }
            }
            _ => {
                match &grad {
                    Some((g, r)) => set_gradient(cr, g, r),
                    None => set_color(cr, f),
                }
                match &glyphs {
                    Some((m, runs)) => show_text_runs(cr, m, runs),
                    None => {
                        let _ = cr.fill_preserve();
                    }
                }
            }
        }
        if let Some((Art::Grain, k)) = art {
            draw_grain(cr, k);
        }
        if stroke.is_none() {
            cr.new_path();
        }
    }
    if let Some(s) = stroke {
        // Kontur mit Verlauf
        match (obj.style.grad(crate::model::PaintTarget::Stroke), object_box(obj)) {
            (Some(g), Some(r)) => set_gradient(cr, g, &r),
            _ => set_color(cr, s),
        }
        cr.set_line_width(obj.style.width);
        cr.set_line_join(cairo_join(obj.style.join));
        cr.set_line_cap(cairo_cap(obj.style.cap));
        // Strichmuster (Punkte brauchen runde Enden)
        let dash = obj.style.dash.pattern(obj.style.width);
        if !dash.is_empty() {
            cr.set_dash(&dash, 0.0);
            if obj.style.dash.round_caps() {
                cr.set_line_cap(cairo::LineCap::Round);
            }
        }
        let mk = obj.style.marker;
        match open_subpaths(&obj.shape).filter(|_| mk != Marker::None) {
            Some(subs) => {
                // Linie an den Enden kürzen, damit sie nicht durch die Spitze ragt
                let w = obj.style.width;
                let ends = obj.style.marker_ends;
                let size = obj.style.marker_size;
                let trim = mk.trim(w, size);
                let trimmed: Vec<SubPath> = subs
                    .iter()
                    .map(|sp| {
                        crate::model::trim_subpath(
                            sp,
                            if ends.start() { trim } else { 0.0 },
                            if ends.end() { trim } else { 0.0 },
                        )
                    })
                    .collect();
                cr.new_path();
                path_subpaths(cr, &trimmed);
                let _ = cr.stroke();
                // Pfeilspitzen & Co. immer durchgezogen
                cr.set_dash(&[], 0.0);
                for sp in &subs {
                    if let Some((a, b)) = crate::model::subpath_ends(sp) {
                        if ends.start() {
                            draw_marker(cr, a.0, a.1, w, size, mk, s);
                        }
                        if ends.end() {
                            draw_marker(cr, b.0, b.1, w, size, mk, s);
                        }
                    }
                }
            }
            None => match obj.style.stroke_align {
                // innen: doppelt breit, auf die Fläche beschnitten
                crate::model::StrokeAlign::Inside if !obj.shape.is_open() => {
                    let path = cr.copy_path();
                    let _ = cr.save();
                    cr.clip_preserve();
                    cr.set_line_width(obj.style.width * 2.0);
                    let _ = cr.stroke();
                    let _ = cr.restore();
                    drop(path);
                }
                // außen: doppelt breit, nur außerhalb der Fläche
                crate::model::StrokeAlign::Outside if !obj.shape.is_open() => {
                    let path = cr.copy_path();
                    let _ = cr.save();
                    let w = obj.style.width * 2.0;
                    if let Ok((x0, y0, x1, y1)) = cr.path_extents() {
                        cr.new_path();
                        cr.rectangle(x0 - w * 2.0, y0 - w * 2.0, x1 - x0 + w * 4.0, y1 - y0 + w * 4.0);
                        if let Ok(p) = &path {
                            cr.append_path(p);
                        }
                        cr.set_fill_rule(cairo::FillRule::EvenOdd);
                        cr.clip();
                    }
                    cr.new_path();
                    if let Ok(p) = &path {
                        cr.append_path(p);
                    }
                    cr.set_line_width(w);
                    let _ = cr.stroke();
                    let _ = cr.restore();
                }
                _ => {
                    let _ = cr.stroke();
                }
            },
        }
        cr.set_dash(&[], 0.0);
    }
    cr.new_path();
}

/// Glatter Wert-Rauschen (0–1) an der Stelle (x, y) in Zellen der Größe `scale`.
fn value_noise(x: f64, y: f64, scale: f64, salt: u32) -> f64 {
    let (fx, fy) = (x / scale, y / scale);
    let (i, j) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - i, fy - j);
    let s = |t: f64| t * t * (3.0 - 2.0 * t);
    let (sx, sy) = (s(tx), s(ty));
    let (i, j) = (i as i64, j as i64);
    let a = cell_noise(i, j, salt);
    let b = cell_noise(i + 1, j, salt);
    let c = cell_noise(i, j + 1, salt);
    let d = cell_noise(i + 1, j + 1, salt);
    let top = a + (b - a) * sx;
    let bot = c + (d - c) * sx;
    top + (bot - top) * sy
}

/// Wasserfarben: die Füllung (aktueller Pfad, Farbe über `paint`) wird in einen
/// Zwischenspeicher gemalt und wie mit Aquarell aufgetragen – unregelmäßige Ränder,
/// dunklere Farbränder, wolkige Farbdichte und etwas Papierstruktur. `k` = Stärke.
/// Gibt `false` zurück, wenn nichts gezeichnet wurde.
fn draw_watercolor(cr: &cairo::Context, obj: &Object, paint: &dyn Fn(&cairo::Context), k: f64) -> bool {
    let Ok(path) = cr.copy_path() else { return false };
    let Ok((x0, y0, x1, y1)) = cr.fill_extents() else {
        return false;
    };
    let b = Rect { x0, y0, x1, y1 };
    let res = FX_RES.with(|c| c.get());
    let edge_for = |scale: f64| (6.0 * scale).clamp(1.5, 40.0 * res);
    let Some((ctm, area)) = raster_area(cr, &b, &|scale| edge_for(scale) * 2.0 + 2.0) else {
        return false;
    };
    // gespiegelt (Spiegelung unter dem Objekt) eigener Platz im Zwischenspeicher
    let slot = if ctm.yy() < 0.0 {
        SLOT_WATER_MIRROR
    } else {
        SLOT_WATER
    };
    let key = fx_key(obj, &ctm, area, 0);
    if let Some((e, map)) = fx_lookup(obj, slot, key) {
        if let Some(surf) = &e.surf {
            let _ = cr.save();
            cr.new_path();
            image_source(cr, surf, map);
            let _ = cr.paint();
            let _ = cr.restore();
            cr.new_path();
            cr.append_path(&path);
            return true;
        }
    }
    let (ix0, iy0, w, h) = area;
    let scale = (ctm.xx() * ctm.yy() - ctm.xy() * ctm.yx()).abs().sqrt().max(1e-6);
    let edge = edge_for(scale);
    let Ok(mut surf) = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h) else {
        return false;
    };
    {
        let Ok(c2) = cairo::Context::new(&surf) else {
            return false;
        };
        c2.translate(-ix0, -iy0);
        c2.transform(ctm);
        c2.append_path(&path);
        c2.set_fill_rule(cr.fill_rule());
        paint(&c2);
        let _ = c2.fill();
    }
    surf.flush();
    let (wu, hu) = (w as usize, h as usize);
    let stride = surf.stride() as usize;
    // Abstand zum Rand (weich): 0,5 am Rand, 1 tief innen
    let mut field = vec![0f32; wu * hu];
    {
        let Ok(d) = surf.data() else { return false };
        for y in 0..hu {
            for x in 0..wu {
                let o = y * stride + x * 4;
                field[y * wu + x] = d[o + 3] as f32 / 255.0;
            }
        }
    }
    blur_field(&mut field, wu, hu, edge);
    // Gerät → Dokument (damit die Struktur beim Verschieben der Ansicht stehen bleibt)
    let inv = ctm.try_invert().unwrap_or(ctm);
    let k = k.clamp(0.0, 1.0) as f32;
    {
        let Ok(mut d) = surf.data() else { return false };
        for y in 0..hu {
            for x in 0..wu {
                let o = y * stride + x * 4;
                let a0 = d[o + 3] as f32;
                if a0 <= 0.0 {
                    continue;
                }
                let (dx, dy) = inv.transform_point(x as f64 + ix0 + 0.5, y as f64 + iy0 + 0.5);
                let low =
                    value_noise(dx, dy, 60.0, 11) as f32 * 0.6 + value_noise(dx, dy, 23.0, 12) as f32 * 0.4;
                let fine = value_noise(dx, dy, 3.5, 13) as f32;
                let f = field[y * wu + x];
                // ausgefranster Rand
                let rag = ((f - 0.5 + (fine - 0.5) * 0.45 * k + (low - 0.5) * 0.2 * k) / 0.12 + 0.5)
                    .clamp(0.0, 1.0);
                let rag = 1.0 - k + k * rag;
                // dunklerer Farbrand, wolkige Dichte, Papier
                let rim = ((0.85 - f) / 0.35).clamp(0.0, 1.0) * 0.4 * k;
                let cloud = (low - 0.5) * 0.45 * k;
                let paper = (fine - 0.5) * 0.12 * k;
                let dens = (1.0 - 0.25 * k + cloud + rim * 0.6 + paper).clamp(0.2, 1.0);
                let dark = 1.0 - rim * 0.55;
                // Deckkraft bleibt erhalten (nur der ausgefranste Rand verändert die Form):
                // die wolkige Dichte hellt die Farbe auf, statt sie durchsichtig zu machen
                let lighten = 1.0 - dens;
                for c in 0..3 {
                    let v = d[o + c] as f32 * dark;
                    let v = v + (a0 - v) * lighten;
                    d[o + c] = (v * rag).round().clamp(0.0, 255.0) as u8;
                }
                d[o + 3] = (a0 * rag).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    surf.mark_dirty();
    let to_img = to_image(&ctm, ix0, iy0);
    fx_store(obj, slot, key, Some(surf.clone()), None, to_img);
    let _ = cr.save();
    cr.new_path();
    image_source(cr, &surf, to_img);
    let _ = cr.paint();
    let _ = cr.restore();
    cr.new_path();
    cr.append_path(&path);
    true
}

/// Teilpfade einer offenen Linie (für Linienenden), sonst None.
fn open_subpaths(shape: &Shape) -> Option<Vec<SubPath>> {
    if !shape.is_open() {
        return None;
    }
    match shape {
        Shape::Path { subpaths, .. } => Some(crate::model::effective_subpaths(subpaths)),
        s => match s.to_path()? {
            Shape::Path { subpaths, .. } => Some(subpaths),
            _ => None,
        },
    }
}

/// Linienende an `tip` zeichnen; `dir` zeigt vom Pfad nach außen.
fn draw_marker(cr: &cairo::Context, tip: Pt, dir: Pt, w: f64, size: f64, mk: Marker, color: Color) {
    let l = mk.length(w, size);
    let half = l * 0.4;
    let perp = Pt::new(-dir.y, dir.x);
    set_color(cr, color);
    cr.new_path();
    match mk {
        Marker::Arrow => {
            let base = tip - dir * l;
            let (a, b) = (base + perp * half, base - perp * half);
            cr.move_to(tip.x, tip.y);
            cr.line_to(a.x, a.y);
            cr.line_to(b.x, b.y);
            cr.close_path();
            let _ = cr.fill();
        }
        Marker::OpenArrow => {
            let base = tip - dir * (l * 0.8);
            let (a, b) = (base + perp * half, base - perp * half);
            cr.move_to(a.x, a.y);
            cr.line_to(tip.x, tip.y);
            cr.line_to(b.x, b.y);
            cr.set_line_width(w);
            cr.set_line_join(cairo::LineJoin::Round);
            cr.set_line_cap(cairo::LineCap::Round);
            let _ = cr.stroke();
            cr.set_line_cap(cairo::LineCap::Butt);
            cr.set_line_join(cairo::LineJoin::Miter);
        }
        Marker::Dot => {
            cr.arc(tip.x, tip.y, (w * 1.6).max(2.5) * size.clamp(0.1, 20.0), 0.0, TAU);
            let _ = cr.fill();
        }
        Marker::Bar => {
            let (a, b) = (tip + perp * half, tip - perp * half);
            cr.move_to(a.x, a.y);
            cr.line_to(b.x, b.y);
            cr.set_line_width(w);
            let _ = cr.stroke();
        }
        Marker::None => {}
    }
    cr.new_path();
}

/// Alle sichtbaren Ebenen zeichnen (ohne Hintergrund).
pub fn draw_document(cr: &cairo::Context, doc: &Document) {
    draw_document_until(cr, doc, None, true);
}

/// Dokument zeichnen, optional nur bis (ausschließlich) zum Objekt `stop`.
/// `frost`: Milchglas-Objekte mit weichgezeichnetem Hintergrund (sonst wie normal).
fn draw_document_until(cr: &cairo::Context, doc: &Document, stop: Option<u64>, frost: bool) {
    // Aufhellen wirkt wie Licht: Aufhellen-Objekte, die in der Reihenfolge direkt
    // aufeinander folgen, werden gesammelt und gemeinsam aufgetragen (ein einzelnes
    // Feld zeigt seine Farbe, Überlagerungen laufen Richtung Weiß).
    let mut run: Vec<&Object> = Vec::new();
    'objects: for layer in doc.layers.iter().filter(|l| l.visible) {
        for obj in &layer.objects {
            if Some(obj.id) == stop {
                break 'objects;
            }
            let glass = frost && obj.style.fx.needs_glass() && obj.style.opacity > 0.0;
            if !glass && obj.style.blend == Blend::Screen && obj.style.opacity > 0.0 {
                run.push(obj);
                continue;
            }
            flush_lights(cr, &mut run);
            if glass {
                draw_glass(cr, doc, obj);
            } else {
                draw_object(cr, obj);
            }
        }
    }
    flush_lights(cr, &mut run);
}

/// Eine Folge von Aufhellen-Objekten wie überlagerte Lichtspots auftragen.
///
/// Das Licht `L` ist die Summe der Farben (jede mit ihrer Deckkraft gewichtet, höchstens
/// Weiß), die Abdeckung `A` die vereinigte Deckkraft aller Felder. Ergebnis:
/// `Hintergrund · (1 − A) + L`. Ein einzelnes Feld sieht damit aus wie ein normales
/// (Rot auf Weiß ist rot), Überlagerungen addieren ihr Licht: Rot und Grün werden Gelb,
/// Rot, Grün und Blau Weiß. Der Hintergrund zählt dabei nicht als Licht, sonst wäre auf
/// weißem Grund nichts zu sehen. Ohne die Abdeckung würden sich Felder mit weniger als
/// 100 % Deckkraft in der Überlappung dunkler statt heller mischen.
fn flush_lights(cr: &cairo::Context, run: &mut Vec<&Object>) {
    if run.is_empty() {
        return;
    }
    let _ = (|| -> Option<()> {
        // Licht: Farben addieren (Aufhellen-Objekte nutzen dafür den Operator „Add“)
        cr.push_group();
        for o in run.iter() {
            draw_object(cr, o);
        }
        let light = cr.pop_group().ok()?;
        // Abdeckung: dieselben Felder wie normal gezeichnet, ergibt die vereinigte Deckkraft
        cr.push_group();
        for o in run.iter() {
            let mut n = (*o).clone();
            n.style.blend = Blend::Normal;
            draw_object(cr, &n);
        }
        let cover = cr.pop_group().ok()?;
        let _ = cr.save();
        cr.set_operator(cairo::Operator::DestOut);
        let _ = cr.set_source(&cover);
        let _ = cr.paint();
        cr.set_operator(cairo::Operator::Add);
        let _ = cr.set_source(&light);
        let _ = cr.paint();
        let _ = cr.restore();
        Some(())
    })();
    run.clear();
}

/// Weichzeichnung des Milchglases in Dokumenteinheiten (bei mittlerer Stärke).
const FROST_BLUR: f64 = 18.0;

/// Kantenlänge der groben Pixel in Dokumenteinheiten. Fest und am Dokumentursprung
/// ausgerichtet, damit nebeneinanderliegende Objekte dasselbe Raster haben.
const PIXEL_CELL: f64 = 6.0;

/// Pseudozufall je Rasterzelle (gleich für alle Objekte an derselben Stelle).
fn cell_noise(i: i64, j: i64, salt: u32) -> f64 {
    let mut h = (i as u32).wrapping_mul(0x9E37_79B1) ^ (j as u32).wrapping_mul(0x85EB_CA77) ^ salt;
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h & 0x00FF_FFFF) as f64 / 0x0100_0000 as f64
}

/// Verlauf aus groben Pixeln: eine Rasterzelle = ein Bildpunkt eines kleinen Bildes,
/// das ohne Glättung vergrößert wird (keine hellen Fugen). Jede Zelle weicht zufällig
/// etwas vom Verlauf ab und ist etwas heller oder dunkler.
fn draw_pixel_gradient(cr: &cairo::Context, g: &Gradient, r: &Rect, cell: f64) {
    let path = cr.copy_path();
    let (a, b) = g.line(r);
    let ab = b - a;
    let l2 = (ab.x * ab.x + ab.y * ab.y).max(1e-9);
    let radial = g.shape == crate::model::GradShape::Radial;
    let rad = g.p0.dist(g.p1).max(1e-6);
    let (rw, rh) = (r.width().max(1e-6), r.height().max(1e-6));
    let cell = cell.max(0.5);
    let (i0, j0) = ((r.x0 / cell).floor() as i64, (r.y0 / cell).floor() as i64);
    let (i1, j1) = ((r.x1 / cell).ceil() as i64, (r.y1 / cell).ceil() as i64);
    let (w, h) = ((i1 - i0).max(1), (j1 - j0).max(1));
    if w * h > 4_000_000 {
        return;
    }
    let Ok(mut img) = cairo::ImageSurface::create(cairo::Format::ARgb32, w as i32, h as i32) else {
        return;
    };
    let stride = img.stride() as usize;
    if let Ok(mut d) = img.data() {
        for jj in 0..h {
            for ii in 0..w {
                let (ci, cj) = (i0 + ii, j0 + jj);
                let q = Pt::new((ci as f64 + 0.5) * cell, (cj as f64 + 0.5) * cell);
                let t = if radial {
                    // im Einheitsrahmen: Abstand zur Mitte / Radius
                    let u = Pt::new((q.x - r.x0) / rw, (q.y - r.y0) / rh);
                    u.dist(g.p0) / rad
                } else {
                    ((q - a).x * ab.x + (q - a).y * ab.y) / l2
                };
                // Lage im Verlauf zufällig verschieben, Helligkeit leicht streuen
                let jitter = (cell_noise(ci, cj, 1) - 0.5) * 0.45;
                let c = g.color_at((t + jitter).clamp(0.0, 1.0));
                let lum = 1.0 + (cell_noise(ci, cj, 2) - 0.5) * 0.22;
                let ch = |v: f64| ((v * lum).clamp(0.0, 1.0) * c.a * 255.0).round() as u32;
                let px = ((c.a * 255.0).round() as u32) << 24 | ch(c.r) << 16 | ch(c.g) << 8 | ch(c.b);
                let o = jj as usize * stride + ii as usize * 4;
                d[o..o + 4].copy_from_slice(&px.to_ne_bytes());
            }
        }
    }
    img.mark_dirty();
    let pat = cairo::SurfacePattern::create(&img);
    pat.set_filter(cairo::Filter::Nearest);
    // Dokument → Bild: (x / cell − i0, y / cell − j0)
    let m = cairo::Matrix::new(1.0 / cell, 0.0, 0.0, 1.0 / cell, -(i0 as f64), -(j0 as f64));
    pat.set_matrix(m);
    let _ = cr.save();
    cr.clip_preserve();
    let _ = cr.set_source(&pat);
    let _ = cr.paint();
    let _ = cr.restore();
    cr.new_path();
    if let Ok(p) = path {
        cr.append_path(&p);
    }
}

/// Gleitender Mittelwert auf einem einkanaligen Feld (drei Durchgänge ≈ Gauß).
fn blur_field(f: &mut [f32], w: usize, h: usize, radius: f64) {
    let r = ((radius / 3f64.sqrt()).round() as usize).max(1);
    let mut tmp = vec![0f32; w.max(h)];
    let mut line = vec![0f32; w.max(h)];
    let pass = |line: &mut [f32], tmp: &mut [f32], n: usize| {
        let px = |line: &[f32], i: isize| line[i.clamp(0, n as isize - 1) as usize];
        let div = (2 * r + 1) as f32;
        let mut sum = 0f32;
        for i in -(r as isize)..=(r as isize) {
            sum += px(line, i);
        }
        for i in 0..n {
            tmp[i] = sum / div;
            sum += px(line, i as isize + r as isize + 1);
            sum -= px(line, i as isize - r as isize);
        }
        line[..n].copy_from_slice(&tmp[..n]);
    };
    for _ in 0..3 {
        for y in 0..h {
            line[..w].copy_from_slice(&f[y * w..y * w + w]);
            pass(&mut line, &mut tmp, w);
            f[y * w..y * w + w].copy_from_slice(&line[..w]);
        }
        for x in 0..w {
            for y in 0..h {
                line[y] = f[y * w + x];
            }
            pass(&mut line, &mut tmp, h);
            for y in 0..h {
                f[y * w + x] = line[y];
            }
        }
    }
}

// ───────────────────────────── Effekt-Zwischenspeicher ─────────────────────────────
//
// Schatten, Wasserfarben, Milchglas, 3D-Effekte und Texturen werden als Rasterbilder
// berechnet. Auf der Zeichenfläche werden diese Bilder zwischengespeichert und nur neu
// berechnet, wenn sich das Objekt, die Ansicht oder (bei Glas) das Dokument geändert hat.
// Während des Verschiebens, Skalierens, Verschiebens der Ansicht und Zoomens wird gar nichts
// neu berechnet: die vorhandenen Bilder wandern mit dem Objekt mit; erst danach wird gerechnet.

thread_local! {
    /// Auflösungsfaktor der Rasterbilder (Bildschirm: Skalierungsfaktor, Export: Qualität)
    static FX_RES: Cell<f64> = const { Cell::new(1.0) };
    /// Es wird die Zeichenfläche gezeichnet (nur dann wird zwischengespeichert)
    static FX_CANVAS: Cell<bool> = const { Cell::new(false) };
    /// Verschieben, Skalieren, Zoomen läuft: vorhandene Bilder verwenden statt neu zu rechnen
    static FX_INTERACTIVE: Cell<bool> = const { Cell::new(false) };
    /// Änderungsstand des Dokuments (Glas-Effekte hängen vom Hintergrund ab)
    static FX_EPOCH: Cell<u64> = const { Cell::new(0) };
    static FX_FRAME: Cell<u64> = const { Cell::new(0) };
    /// Wurde beim letzten Zeichnen ein veraltetes Bild gezeigt?
    static FX_STALE: Cell<bool> = const { Cell::new(false) };
    static FX_CACHE: RefCell<HashMap<(u64, u8), FxEntry>> = RefCell::new(HashMap::new());
}

/// Größte Pixelzahl eines Effekt-Rasterbildes; größere Bilder werden mit geringerer Auflösung
/// gerechnet (betrifft nur sehr große Objekte bei hoher Exportqualität).
const FX_MAX_PIXELS: f64 = 24_000_000.0;

const SLOT_SHADOW: u8 = 1;
const SLOT_WATER: u8 = 2;
const SLOT_WATER_MIRROR: u8 = 3;
const SLOT_GLASS: u8 = 4;

#[derive(Clone)]
struct FxEntry {
    key: u64,
    /// Hauptbild (Schatten, Wasserfarben, bei Glas: was unter dem Objekt erscheint)
    surf: Option<cairo::ImageSurface>,
    /// zweites Bild (Glas: Licht, Schatten, Texturen über dem Objekt)
    aux: Option<cairo::ImageSurface>,
    /// Dokument → Bildpixel beim Erzeugen
    to_img: cairo::Matrix,
    /// Rahmen des Objekts (Dokument) beim Erzeugen
    anchor: Rect,
    used: u64,
}

/// Auflösungsfaktor für Effekte setzen; liefert den bisherigen Wert.
pub fn set_effect_resolution(r: f64) -> f64 {
    FX_RES.with(|c| c.replace(r.clamp(0.25, 16.0)))
}

/// Zustand für das nächste Zeichnen der Zeichenfläche: Änderungsstand des Dokuments und ob
/// gerade verschoben/gezoomt wird.
pub fn set_canvas_state(epoch: u64, interactive: bool) {
    FX_EPOCH.with(|c| c.set(epoch));
    FX_INTERACTIVE.with(|c| c.set(interactive));
}

/// Wurde beim letzten Zeichnen der Zeichenfläche ein veraltetes Effektbild gezeigt? (Dann
/// sollte nach dem Verschieben noch einmal gezeichnet werden.)
pub fn effects_stale() -> bool {
    FX_STALE.with(|c| c.get())
}

/// Text in Hash-Werte schreiben, ohne ihn zusammenzusetzen.
struct HashWriter(std::collections::hash_map::DefaultHasher);

impl std::fmt::Write for HashWriter {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        use std::hash::Hasher;
        self.0.write(s.as_bytes());
        Ok(())
    }
}

/// Fingerabdruck von Form und Stil eines Objekts (Bilddaten nur über ihre Adresse).
fn fingerprint(obj: &Object) -> u64 {
    use std::fmt::Write;
    use std::hash::Hasher;
    let mut h = HashWriter(std::collections::hash_map::DefaultHasher::new());
    match &obj.shape {
        Shape::Image {
            xf,
            w,
            h: ih,
            data,
            clip,
        } => {
            let _ = write!(h, "img {xf:?} {w} {ih} {:p} {clip:?}", Rc::as_ptr(data));
        }
        s => {
            let _ = write!(h, "{s:?}");
        }
    }
    let _ = write!(h, "{:?}", obj.style);
    h.0.finish()
}

fn fx_key(obj: &Object, m: &cairo::Matrix, area: (f64, f64, i32, i32), extra: u64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    fingerprint(obj).hash(&mut h);
    for v in [m.xx(), m.yx(), m.xy(), m.yy(), m.x0(), m.y0(), area.0, area.1] {
        ((v * 1000.0).round() as i64).hash(&mut h);
    }
    (area.2, area.3, extra).hash(&mut h);
    h.finish()
}

/// Gespeichertes Bild verwenden? Liefert es samt passender Matrix, wenn es aktuell ist oder
/// gerade verschoben/gezoomt wird.
fn fx_lookup(obj: &Object, slot: u8, key: u64) -> Option<(FxEntry, cairo::Matrix)> {
    if !FX_CANVAS.with(|c| c.get()) {
        return None;
    }
    let frame = FX_FRAME.with(|c| c.get());
    let interactive = FX_INTERACTIVE.with(|c| c.get());
    FX_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        let e = c.get_mut(&(obj.id, slot))?;
        let fresh = e.key == key;
        if !fresh && !interactive {
            return None;
        }
        e.used = frame;
        if !fresh {
            FX_STALE.with(|s| s.set(true));
        }
        let now = object_box(obj).unwrap_or(e.anchor);
        Some((e.clone(), fx_map(e, &now)))
    })
}

/// Matrix, die ein Bild vom alten Rahmen des Objekts auf den neuen überträgt.
fn fx_map(e: &FxEntry, now: &Rect) -> cairo::Matrix {
    let a = &e.anchor;
    let ratio = |n: f64, o: f64| {
        if o.abs() > 1e-9 && n.abs() > 1e-9 {
            n / o
        } else {
            1.0
        }
    };
    let sx = ratio(now.width(), a.width());
    let sy = ratio(now.height(), a.height());
    let ainv = cairo::Matrix::new(
        1.0 / sx,
        0.0,
        0.0,
        1.0 / sy,
        a.x0 - now.x0 / sx,
        a.y0 - now.y0 / sy,
    );
    cairo::Matrix::multiply(&ainv, &e.to_img)
}

fn fx_store(
    obj: &Object,
    slot: u8,
    key: u64,
    surf: Option<cairo::ImageSurface>,
    aux: Option<cairo::ImageSurface>,
    to_img: cairo::Matrix,
) {
    if !FX_CANVAS.with(|c| c.get()) {
        return;
    }
    let Some(anchor) = object_box(obj) else { return };
    let used = FX_FRAME.with(|c| c.get());
    FX_CACHE.with(|c| {
        c.borrow_mut().insert(
            (obj.id, slot),
            FxEntry {
                key,
                surf,
                aux,
                to_img,
                anchor,
                used,
            },
        )
    });
}

/// Zeichnen der Zeichenfläche beginnt bzw. endet (nicht mehr gebrauchte Bilder verwerfen).
fn fx_canvas_begin() {
    FX_CANVAS.with(|c| c.set(true));
    FX_STALE.with(|c| c.set(false));
    FX_FRAME.with(|c| c.set(c.get() + 1));
}

fn fx_canvas_end() {
    FX_CANVAS.with(|c| c.set(false));
    let frame = FX_FRAME.with(|c| c.get());
    FX_CACHE.with(|c| c.borrow_mut().retain(|_, e| e.used == frame));
}

/// Bild mit Matrix als Quelle setzen.
fn image_source(cr: &cairo::Context, surf: &cairo::ImageSurface, m: cairo::Matrix) {
    let sp = cairo::SurfacePattern::create(surf);
    sp.set_matrix(m);
    let _ = cr.set_source(&sp);
}

/// Rasterbereich für einen Effekt: Matrix Dokument → Rasterpixel (Gerätematrix mal
/// Auflösungsfaktor) und Lage (x0, y0, Breite, Höhe) im Raster. `pad` bekommt den Maßstab
/// (Rasterpixel je Dokumenteinheit) und liefert den nötigen Rand in Rasterpixeln.
/// Ist das Bild zu groß, wird die Auflösung verringert.
fn raster_area(
    cr: &cairo::Context,
    b: &Rect,
    pad: &dyn Fn(f64) -> f64,
) -> Option<(cairo::Matrix, (f64, f64, i32, i32))> {
    let base = cr.matrix();
    let mut r = FX_RES.with(|c| c.get());
    for _ in 0..6 {
        let m = cairo::Matrix::multiply(&base, &cairo::Matrix::new(r, 0.0, 0.0, r, 0.0, 0.0));
        let scale = (m.xx() * m.yy() - m.xy() * m.yx()).abs().sqrt().max(1e-6);
        let area = device_rect_m(cr, b, pad(scale), &m)?;
        if (area.2 as f64) * (area.3 as f64) <= FX_MAX_PIXELS && area.2 <= 16000 && area.3 <= 16000 {
            return Some((m, area));
        }
        r *= 0.6;
    }
    None
}

/// Rahmen in Rasterkoordinaten (Matrix `m`) mit Rand, auf den sichtbaren Bereich beschränkt.
fn device_rect_m(cr: &cairo::Context, b: &Rect, pad: f64, m: &cairo::Matrix) -> Option<(f64, f64, i32, i32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y) in [(b.x0, b.y0), (b.x1, b.y0), (b.x1, b.y1), (b.x0, b.y1)] {
        let (dx, dy) = m.transform_point(x, y);
        x0 = x0.min(dx);
        y0 = y0.min(dy);
        x1 = x1.max(dx);
        y1 = y1.max(dy);
    }
    let (mut x0, mut y0, mut x1, mut y1) = (x0 - pad, y0 - pad, x1 + pad, y1 + pad);
    if let Ok((cx0, cy0, cx1, cy1)) = cr.clip_extents() {
        let mut ex = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for (x, y) in [(cx0, cy0), (cx1, cy0), (cx1, cy1), (cx0, cy1)] {
            let (dx, dy) = m.transform_point(x, y);
            ex = (ex.0.min(dx), ex.1.min(dy), ex.2.max(dx), ex.3.max(dy));
        }
        x0 = x0.max(ex.0 - pad);
        y0 = y0.max(ex.1 - pad);
        x1 = x1.min(ex.2 + pad);
        y1 = y1.min(ex.3 + pad);
    }
    let (ix0, iy0) = (x0.floor(), y0.floor());
    let w = (x1.ceil() - ix0) as i32;
    let h = (y1.ceil() - iy0) as i32;
    if w <= 0 || h <= 0 {
        return None;
    }
    Some((ix0, iy0, w, h))
}

/// Matrix Dokument → Bildpixel für ein Rasterbild ab (x0, y0).
fn to_image(m: &cairo::Matrix, ix0: f64, iy0: f64) -> cairo::Matrix {
    cairo::Matrix::multiply(m, &cairo::Matrix::new(1.0, 0.0, 0.0, 1.0, -ix0, -iy0))
}

/// Kleinster innerer Radius des Objekts in Rasterpixeln (größter Abstand zum Rand), aus
/// einem verkleinerten Abbild. Er hängt nicht vom sichtbaren Ausschnitt ab, damit das
/// Relief beim Verschieben und Zoomen stabil bleibt.
fn inscribed_radius(obj: &Object, ctm: &cairo::Matrix, dev: (f64, f64, f64, f64)) -> Option<f32> {
    let (ox0, oy0, ox1, oy1) = dev;
    let big = (ox1 - ox0).max(oy1 - oy0).max(1.0);
    let sc = (160.0 / big).min(1.0);
    let sw = ((ox1 - ox0) * sc).ceil() as i32 + 4;
    let sh = ((oy1 - oy0) * sc).ceil() as i32 + 4;
    let mut surf = cairo::ImageSurface::create(cairo::Format::ARgb32, sw.max(4), sh.max(4)).ok()?;
    {
        let c = cairo::Context::new(&surf).ok()?;
        c.translate(2.0, 2.0);
        c.scale(sc, sc);
        c.translate(-ox0, -oy0);
        c.transform(*ctm);
        draw_object_body(&c, &solid_copy(obj));
    }
    surf.flush();
    let (w, h) = (surf.width() as usize, surf.height() as usize);
    let stride = surf.stride() as usize;
    let d = surf.data().ok()?;
    let mut a = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            a[y * w + x] = d[y * stride + x * 4 + 3] as f32 / 255.0;
        }
    }
    drop(d);
    let dist = crate::imgfx::inside_distance(&a, w, h);
    let m = dist.iter().cloned().fold(0f32, f32::max);
    Some((m / sc as f32).max(1.0))
}

/// Das Objekt einfarbig und deckend (als Maske für Effekte).
fn solid_copy(obj: &Object) -> Object {
    let mut solid = obj.clone();
    solid.style.fill = Some(Color::BLACK);
    solid.style.stroke = None;
    solid.style.gradient = None;
    solid.style.opacity = 1.0;
    solid.style.blend = Blend::Normal;
    solid.style.fx = crate::model::Effects::default();
    solid
}

/// Ergebnis der Glas-Berechnung: Bild unter dem Objekt (bei Milchglas ohne 3D-Effekt noch
/// mit der Form zu maskieren) und Bild über dem Objekt.
struct GlassOut {
    under: Option<cairo::ImageSurface>,
    over: Option<cairo::ImageSurface>,
    to_img: cairo::Matrix,
}

/// Milchglas, 3D-Effekte und Texturen.
///
/// Unter dem Objekt: was dahinter liegt, wird in einen Zwischenspeicher gezeichnet, bei
/// Milchglas weichgezeichnet und bei einem 3D-Effekt wie durch ein Relief gebrochen
/// (herausgedrückt: am Rand nach außen verschoben und in der Mitte vergrößert,
/// eingedrückt: umgekehrt). Darüber das Objekt selbst, und darüber Licht und Schatten des
/// Reliefs sowie die Texturen gebürstetes Metall und Hochglanz.
fn draw_glass(cr: &cairo::Context, doc: &Document, obj: &Object) {
    draw_extras(cr, obj);
    let fx = obj.style.fx;
    let frost = fx.frost();
    let lens = fx.lens.filter(|(_, k)| k.abs() > 0.004);
    let out: Option<(GlassOut, bool)> = (|| {
        let b = object_box(obj)?;
        let base = cr.matrix();
        let base_scale = (base.xx() * base.yy() - base.xy() * base.yx())
            .abs()
            .sqrt()
            .max(1e-6);
        // Rand: Weichzeichnung des Milchglases und Reichweite der Brechung
        let (bw0, bh0) = (b.width() * base_scale, b.height() * base_scale);
        let pad = |scale: f64| {
            let blur = frost
                .map(|k| (FROST_BLUR * (0.15 + 0.85 * k) * scale).clamp(0.5, 600.0))
                .unwrap_or(0.0);
            let small = bw0.min(bh0).max(1.0) * scale / base_scale;
            let reach = if lens.is_some() {
                (small * 0.5).max(2.0)
            } else {
                0.0
            };
            reach * 2.2 + blur * 2.0 + 2.0
        };
        let (m, area) = raster_area(cr, &b, &pad)?;
        // Glas hängt vom Hintergrund ab, daher zählt der Änderungsstand des Dokuments mit
        let epoch = FX_EPOCH.with(|c| c.get());
        let key = fx_key(obj, &m, area, epoch);
        if let Some((e, map)) = fx_lookup(obj, SLOT_GLASS, key) {
            let to_img = map;
            return Some((
                GlassOut {
                    under: e.surf,
                    over: e.aux,
                    to_img,
                },
                true,
            ));
        }
        let g = glass_compute(doc, obj, &m, area)?;
        fx_store(obj, SLOT_GLASS, key, g.under.clone(), g.over.clone(), g.to_img);
        Some((g, false))
    })();
    // Form des Objekts als Maske (für Milchglas ohne 3D-Effekt und den Schleier)
    let shape_mask = {
        cr.push_group();
        draw_shape(cr, obj);
        cr.pop_group().ok()
    };
    if let Some((g, _)) = &out {
        if let Some(u) = &g.under {
            let _ = cr.save();
            image_source(cr, u, g.to_img);
            match (&lens, &shape_mask) {
                (None, Some(mask)) => {
                    let _ = cr.mask(mask);
                }
                _ => {
                    let _ = cr.paint();
                }
            }
            let _ = cr.restore();
        }
    }
    // leichter Schleier über Milchglas
    if let (Some(k), Some(mask)) = (frost, &shape_mask) {
        let _ = cr.save();
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.1 + 0.15 * k);
        let _ = cr.mask(mask);
        let _ = cr.restore();
    }
    // darüber das Objekt selbst als Tönung
    draw_object_body(cr, obj);
    if let Some((g, _)) = &out {
        if let Some(o) = &g.over {
            let _ = cr.save();
            image_source(cr, o, g.to_img);
            let _ = cr.paint();
            let _ = cr.restore();
        }
    }
}

/// Glas-Bilder berechnen (Matrix `ctm`: Dokument → Rasterpixel, `area`: Lage im Raster).
fn glass_compute(
    doc: &Document,
    obj: &Object,
    ctm: &cairo::Matrix,
    area: (f64, f64, i32, i32),
) -> Option<GlassOut> {
    use crate::model::{Lens, Texture};
    let ctm = *ctm;
    let (ix0, iy0, w, h) = area;
    let b = object_box(obj)?;
    let scale = (ctm.xx() * ctm.yy() - ctm.xy() * ctm.yx()).abs().sqrt().max(1e-6);
    let fx = obj.style.fx;
    let frost = fx.frost();
    let lens = fx.lens.filter(|(_, k)| k.abs() > 0.004);
    let finish = match fx.tex {
        Some((t @ (Texture::Brushed | Texture::Gloss), k)) => Some((t, k)),
        _ => None,
    };
    // Milchglas-Skala: 100 % entsprechen dem früheren 50 % (daher halbiert)
    let blur = frost
        .map(|k| (FROST_BLUR * (0.15 + 0.85 * k) * scale).clamp(0.5, 600.0))
        .unwrap_or(0.0);
    // Objektrahmen in Rasterkoordinaten
    let (mut ox0, mut oy0, mut ox1, mut oy1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y) in [(b.x0, b.y0), (b.x1, b.y0), (b.x1, b.y1), (b.x0, b.y1)] {
        let (dx, dy) = ctm.transform_point(x, y);
        ox0 = ox0.min(dx);
        oy0 = oy0.min(dy);
        ox1 = ox1.max(dx);
        oy1 = oy1.max(dy);
    }
    let (wu, hu) = (w as usize, h as usize);
    let to_img = to_image(&ctm, ix0, iy0);
    let mut out = GlassOut {
        under: None,
        over: None,
        to_img,
    };

    // Hintergrund (nur für Milchglas und 3D-Effekt); dabei nichts zwischenspeichern
    let mut back = None;
    if frost.is_some() || lens.is_some() {
        let mut surf = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).ok()?;
        {
            let c2 = cairo::Context::new(&surf).ok()?;
            c2.translate(-ix0, -iy0);
            c2.transform(ctm);
            if let Some(bg) = doc.background {
                for (_, r) in doc.artboard_rects() {
                    c2.rectangle(r.x0, r.y0, r.width(), r.height());
                }
                set_color(&c2, bg);
                let _ = c2.fill();
            }
            let was = FX_CANVAS.with(|c| c.replace(false));
            draw_document_until(&c2, doc, Some(obj.id), false);
            FX_CANVAS.with(|c| c.set(was));
        }
        surf.flush();
        if blur >= 0.5 {
            blur_surface(&mut surf, blur);
        }
        back = Some(surf);
    }
    if lens.is_none() {
        // nur Milchglas: weichgezeichneter Hintergrund, wird mit der Form maskiert
        out.under = back.clone();
    }
    if lens.is_none() && finish.is_none() {
        return Some(out);
    }

    // Form als deckende Maske
    let mut mask = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).ok()?;
    {
        let c3 = cairo::Context::new(&mask).ok()?;
        c3.translate(-ix0, -iy0);
        c3.transform(ctm);
        draw_object_body(&c3, &solid_copy(obj));
    }
    mask.flush();
    let ms = mask.stride() as usize;
    let alpha: Vec<f32> = {
        let d = mask.data().ok()?;
        let mut a = vec![0f32; wu * hu];
        for y in 0..hu {
            for x in 0..wu {
                a[y * wu + x] = d[y * ms + x * 4 + 3] as f32 / 255.0;
            }
        }
        a
    };
    let dist = crate::imgfx::inside_distance(&alpha, wu, hu);
    // Licht (weiß) und Schatten (schwarz) über dem Objekt, je Pixel 0…1
    let mut lit = vec![0f32; wu * hu];
    let mut shade = vec![0f32; wu * hu];
    // Objektrahmen im Zwischenbild
    let (bx0, by0) = (ox0 - ix0, oy0 - iy0);
    let (bw, bh) = ((ox1 - ox0).max(1.0), (oy1 - oy0).max(1.0));
    let (ocx, ocy) = ((bx0 + bw / 2.0) as f32, (by0 + bh / 2.0) as f32);

    if let (Some((kind, k)), Some(back)) = (lens, back.as_mut()) {
        let dm = inscribed_radius(obj, &ctm, (ox0, oy0, ox1, oy1))?;
        let sign: f32 = if k < 0.0 { -1.0 } else { 1.0 };
        let amt = k.abs() as f32;
        // Stärke der Beleuchtung: 50 % ist die Grundeinstellung
        let lc = (fx.light * 2.0) as f32;
        // Randzone des Regentropfens höchstens 130 Dokumenteinheiten, unabhängig von Zoom und Auflösung
        let mut rel = crate::imgfx::relief(kind, &dist, dm, sign, (130.0 * scale) as f32);
        // Treppenstufen am Rand glätten, damit Licht und Brechung ruhig bleiben
        blur_field(&mut rel.height, wu, hu, (1.5 + rel.zone as f64 * 0.04).min(8.0));
        let zone = rel.zone;
        let bs = back.stride() as usize;
        let src: Vec<u32> = {
            let d = back.data().ok()?;
            let mut v = vec![0u32; wu * hu];
            for y in 0..hu {
                for x in 0..wu {
                    let o = y * bs + x * 4;
                    v[y * wu + x] = u32::from_ne_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
                }
            }
            v
        };
        let sample = |x: f64, y: f64| -> [f32; 4] {
            let x = x.clamp(0.0, (wu - 1) as f64);
            let y = y.clamp(0.0, (hu - 1) as f64);
            let (x0, y0) = (x.floor() as usize, y.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(wu - 1), (y0 + 1).min(hu - 1));
            let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
            let ch = |v: u32, k: u32| ((v >> k) & 255) as f32;
            let mut out = [0f32; 4];
            for (i, k) in [24u32, 16, 8, 0].iter().enumerate() {
                let a = ch(src[y0 * wu + x0], *k) * (1.0 - fx) + ch(src[y0 * wu + x1], *k) * fx;
                let b = ch(src[y1 * wu + x0], *k) * (1.0 - fx) + ch(src[y1 * wu + x1], *k) * fx;
                out[i] = a * (1.0 - fy) + b * fy;
            }
            out
        };
        let mut outs = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).ok()?;
        let os = outs.stride() as usize;
        {
            let mut d = outs.data().ok()?;
            let (ohw, ohh) = ((bw / 2.0).max(1.0) as f32, (bh / 2.0).max(1.0) as f32);
            for y in 0..hu {
                for x in 0..wu {
                    let m = alpha[y * wu + x];
                    if m <= 0.0 {
                        continue;
                    }
                    let i = y * wu + x;
                    let (gx, gy) = crate::imgfx::slope(&rel.height, wu, hu, x, y);
                    let sl = (gx * gx + gy * gy).sqrt();
                    let (nx, ny) = if sl > 1e-6 { (gx / sl, gy / sl) } else { (0.0, 0.0) };
                    let pp = rel.p[i];
                    let (px_, py_) = (x as f32, y as f32);
                    // Brechung: in der Mitte vergrößert (herausgedrückt) bzw. verkleinert,
                    // an den Flanken zur Seite verschoben
                    let mag = 0.14 * amt * sign * pp;
                    let sx = ocx + (px_ - ocx) * (1.0 - mag) - gx * zone * 0.8 * amt;
                    let sy = ocy + (py_ - ocy) * (1.0 - mag) - gy * zone * 0.8 * amt;
                    let c = sample(sx as f64, sy as f64);
                    // Umgebungsspiegelung zum Rand hin
                    let e = 1.0 - pp;
                    let rw = (0.06 + 0.2 * e * e) * (2.0 * amt).min(1.0);
                    let rx = ocx - (px_ - ocx) * 1.3 - nx * sign * zone * 0.6;
                    let ry = ocy - (py_ - ocy) * 1.3 - ny * sign * zone * 0.6;
                    let refl = sample(rx as f64, ry as f64);
                    let mut col = [0f32; 4];
                    for q in 0..4 {
                        col[q] = c[q] * (1.0 - rw) + refl[q] * rw;
                    }
                    let px = |v: f32| (v * m).round().clamp(0.0, 255.0) as u32;
                    let val = (px(col[0]) << 24)
                        | (px(col[1].min(col[0])) << 16)
                        | (px(col[2].min(col[0])) << 8)
                        | px(col[3].min(col[0]));
                    let o = y * os + x * 4;
                    d[o..o + 4].copy_from_slice(&val.to_ne_bytes());

                    // Licht und Schatten auf dem Objekt
                    let delta = crate::imgfx::light_delta(gx, gy);
                    let mut wv = (delta * 2.0 * amt * lc).clamp(0.0, 0.7);
                    let dv = (-delta * 1.1 * amt * lc).clamp(0.0, 0.5);
                    // Glanzlicht oben links auf runden Formen
                    if sign > 0.0 && matches!(kind, Lens::Drop | Lens::Sphere) {
                        let sx = (px_ - (ocx - ohw * 0.38)) / (ohw * 0.32);
                        let sy = (py_ - (ocy - ohh * 0.45)) / (ohh * 0.22);
                        let spot = (-(sx * sx + sy * sy)).exp() * 0.4 * (2.0 * amt).min(1.0) * lc.min(1.5);
                        wv = 1.0 - (1.0 - wv) * (1.0 - spot);
                    }
                    lit[i] = wv;
                    shade[i] = dv;
                }
            }
        }
        outs.mark_dirty();
        out.under = Some(outs);
    }

    // Texturen: gebürstetes Metall und Hochglanz
    if let Some((tex, k)) = finish {
        let k = k as f32;
        for y in 0..hu {
            for x in 0..wu {
                let i = y * wu + x;
                if alpha[i] <= 0.0 {
                    continue;
                }
                let lx = (x as f64 + 0.5 - bx0) as f32;
                let ly = (y as f64 + 0.5 - by0) as f32;
                match tex {
                    Texture::Brushed => {
                        let v = crate::imgfx::brushed(lx, ly, bw as f32, scale as f32) * k * 0.5;
                        if v > 0.0 {
                            lit[i] = 1.0 - (1.0 - lit[i]) * (1.0 - v);
                        } else {
                            shade[i] = 1.0 - (1.0 - shade[i]) * (1.0 - (-v * 0.8));
                        }
                    }
                    _ => {
                        let g = crate::imgfx::gloss(lx, ly, bw as f32, bh as f32, dist[i], scale as f32) * k;
                        lit[i] = 1.0 - (1.0 - lit[i]) * (1.0 - g);
                    }
                }
            }
        }
    }

    // Schicht zusammensetzen: erst Schatten, darauf das Licht (vormultipliziert)
    let mut ov = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).ok()?;
    let os = ov.stride() as usize;
    {
        let mut d = ov.data().ok()?;
        for y in 0..hu {
            for x in 0..wu {
                let i = y * wu + x;
                let m = alpha[i];
                if m <= 0.0 || (lit[i] <= 0.0 && shade[i] <= 0.0) {
                    continue;
                }
                let a = lit[i] + (1.0 - lit[i]) * shade[i];
                let px = |v: f32| (v * m * 255.0).round().clamp(0.0, 255.0) as u32;
                let val = (px(a) << 24) | (px(lit[i]) << 16) | (px(lit[i]) << 8) | px(lit[i]);
                let o = y * os + x * 4;
                d[o..o + 4].copy_from_slice(&val.to_ne_bytes());
            }
        }
    }
    ov.mark_dirty();
    out.over = Some(ov);
    Some(out)
}

/// Dreifacher Kastenweichzeichner (annähernd gaußförmig) auf einer ARGB-Fläche.
fn blur_surface(surf: &mut cairo::ImageSurface, radius: f64) {
    let w = surf.width() as usize;
    let h = surf.height() as usize;
    let stride = surf.stride() as usize;
    // drei Durchgänge mit etwa radius/√3 ergeben ungefähr die gewünschte Unschärfe
    let r = ((radius / 3f64.sqrt()).round() as usize).max(1);
    let Ok(mut data) = surf.data() else { return };
    let mut buf = vec![0u8; w.max(h) * 4];
    for _ in 0..3 {
        for y in 0..h {
            let row = &mut data[y * stride..y * stride + w * 4];
            box_line(row, &mut buf[..w * 4], w, 4, r);
        }
        let mut col = vec![0u8; h * 4];
        for x in 0..w {
            for y in 0..h {
                let o = y * stride + x * 4;
                col[y * 4..y * 4 + 4].copy_from_slice(&data[o..o + 4]);
            }
            box_line(&mut col, &mut buf[..h * 4], h, 4, r);
            for y in 0..h {
                let o = y * stride + x * 4;
                data[o..o + 4].copy_from_slice(&col[y * 4..y * 4 + 4]);
            }
        }
    }
    drop(data);
    surf.mark_dirty();
}

/// Gleitender Mittelwert über `n` Pixel (je 4 Kanäle), Ränder werden fortgesetzt.
fn box_line(line: &mut [u8], tmp: &mut [u8], n: usize, ch: usize, r: usize) {
    if n == 0 {
        return;
    }
    let div = (2 * r + 1) as u32;
    for c in 0..ch {
        let px = |i: isize| -> u32 { line[(i.clamp(0, n as isize - 1) as usize) * ch + c] as u32 };
        let mut sum: u32 = 0;
        for i in -(r as isize)..=(r as isize) {
            sum += px(i);
        }
        for i in 0..n {
            tmp[i * ch + c] = ((sum + div / 2) / div) as u8;
            sum += px(i as isize + r as isize + 1);
            sum -= px(i as isize - r as isize);
        }
    }
    line[..n * ch].copy_from_slice(&tmp[..n * ch]);
}

pub fn draw_layer(cr: &cairo::Context, layer: &Layer) {
    for obj in &layer.objects {
        draw_object(cr, obj);
    }
}

// ───────────────────────────── Metriken ─────────────────────────────

/// Schriftmetriken und Trefferprüfung über einen kleinen Hilfskontext.
pub struct CairoMetrics {
    cr: cairo::Context,
}

impl CairoMetrics {
    pub fn new() -> CairoMetrics {
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 1, 1).expect("Cairo-Fläche");
        let cr = cairo::Context::new(&surface).expect("Cairo-Kontext");
        CairoMetrics { cr }
    }
}

fn subpaths_bbox(subs: &[SubPath]) -> Option<Rect> {
    Shape::Path {
        subpaths: subs.to_vec(),
        evenodd: false,
    }
    .geom_bbox()
}

impl Metrics for CairoMetrics {
    fn bbox(&self, obj: &Object) -> Option<Rect> {
        object_box(obj)
    }

    fn hit(&self, obj: &Object, p: Pt, tol: f64) -> bool {
        match &obj.shape {
            Shape::Text {
                xf,
                text,
                family,
                size,
                bold,
                italic,
                fmt,
            } => {
                if !invertible(xf) {
                    return false;
                }
                let local = xf.invert().apply(p);
                let g = text_geom(text, family, *size, *bold, *italic, fmt);
                return g.bbox.inflate(tol / xf.mean_scale().max(1e-6)).contains(local);
            }
            Shape::Image { clip: Some(c), .. } => {
                let cr = &self.cr;
                cr.new_path();
                path_subpaths(cr, &crate::model::effective_subpaths(c));
                let mut hit = cr.in_fill(p.x, p.y).unwrap_or(false);
                if !hit {
                    cr.set_line_width(obj.style.width + 2.0 * tol);
                    hit = cr.in_stroke(p.x, p.y).unwrap_or(false);
                }
                cr.new_path();
                return hit;
            }
            Shape::Image { xf, w, h, .. } | Shape::TextArea { xf, w, h, .. } => {
                if !invertible(xf) {
                    return false;
                }
                let local = xf.invert().apply(p);
                let t = tol / xf.mean_scale().max(1e-6);
                return Rect::from_xywh(0.0, 0.0, *w, *h).inflate(t).contains(local);
            }
            _ => {}
        }
        if let Some(b) = self.bbox(obj) {
            if !b.inflate(tol + obj.style.width).contains(p) {
                return false;
            }
        }
        let cr = &self.cr;
        cr.new_path();
        if let Shape::TextPath { path, size, .. } = &obj.shape {
            // Glyphen oder die Grundlinie (mit halber Schrifthöhe Spielraum)
            path_subpaths(cr, &textpath_outline(&obj.shape));
            let mut hit = cr.in_fill(p.x, p.y).unwrap_or(false);
            cr.new_path();
            if !hit {
                path_subpaths(cr, path);
                cr.set_line_width(size * 0.6 + 2.0 * tol);
                hit = cr.in_stroke(p.x, p.y).unwrap_or(false);
                cr.new_path();
            }
            return hit;
        }
        build_path(cr, &obj.shape);
        let evenodd = matches!(obj.shape, Shape::Path { evenodd: true, .. });
        cr.set_fill_rule(if evenodd {
            cairo::FillRule::EvenOdd
        } else {
            cairo::FillRule::Winding
        });
        let mut hit = obj.style.fill.is_some() && cr.in_fill(p.x, p.y).unwrap_or(false);
        if !hit {
            let w = if obj.style.stroke.is_some() {
                obj.style.width
            } else {
                0.0
            };
            cr.set_line_width(w + 2.0 * tol);
            hit = cr.in_stroke(p.x, p.y).unwrap_or(false);
        }
        cr.new_path();
        hit
    }

    fn text_box(&self, obj: &Object) -> Option<[Pt; 4]> {
        match &obj.shape {
            Shape::Text {
                xf,
                text,
                family,
                size,
                bold,
                italic,
                fmt,
            } => {
                let b = text_geom(text, family, *size, *bold, *italic, fmt).bbox;
                Some([
                    xf.apply(Pt::new(b.x0, b.y0)),
                    xf.apply(Pt::new(b.x1, b.y0)),
                    xf.apply(Pt::new(b.x1, b.y1)),
                    xf.apply(Pt::new(b.x0, b.y1)),
                ])
            }
            Shape::TextArea { xf, w, h, .. } => Some([
                xf.apply(Pt::new(0.0, 0.0)),
                xf.apply(Pt::new(*w, 0.0)),
                xf.apply(Pt::new(*w, *h)),
                xf.apply(Pt::new(0.0, *h)),
            ]),
            _ => {
                let b = self.bbox(obj)?;
                Some([
                    Pt::new(b.x0, b.y0),
                    Pt::new(b.x1, b.y0),
                    Pt::new(b.x1, b.y1),
                    Pt::new(b.x0, b.y1),
                ])
            }
        }
    }

    fn fill_area(&self, obj: &Object) -> Option<String> {
        let Shape::TextArea {
            w,
            h,
            family,
            size,
            bold,
            italic,
            fmt,
            frame,
            ..
        } = &obj.shape
        else {
            return None;
        };
        let inset = frame.inset.max(0.0);
        let shaped = frame.path.is_some();
        let words: Vec<&str> = crate::editor::LOREM.split(' ').collect();
        let make = |n: usize| {
            words
                .iter()
                .cycle()
                .take(n.max(1))
                .copied()
                .collect::<Vec<_>>()
                .join(" ")
        };
        let fits = |n: usize| {
            if shaped {
                // Text in Form: passt, solange nichts überläuft
                let mut s2 = obj.shape.clone();
                if let Some((t, ..)) = s2.text_fields_mut() {
                    *t = make(n);
                }
                return !shape_lines(&s2).map(|x| x.1).unwrap_or(true);
            }
            area_height(
                &make(n),
                family,
                *size,
                *bold,
                *italic,
                (w - 2.0 * inset).max(1.0),
                (*h - 2.0 * inset).max(1.0),
                fmt,
            ) <= *h - 2.0 * inset + 0.5
        };
        // so viele Wörter wie hineinpassen (mindestens eines)
        let (mut lo, mut hi) = (1usize, 2usize);
        while fits(hi) && hi < 20000 {
            lo = hi;
            hi *= 2;
        }
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if fits(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Some(make(lo))
    }

    fn text_index(&self, obj: &Object, p: Pt) -> Option<usize> {
        if matches!(obj.shape, Shape::TextPath { .. }) {
            return textpath_index(&obj.shape, p);
        }
        let xf = text_xf(&obj.shape)?;
        if !invertible(&xf) {
            return None;
        }
        let local = xf.invert().apply(p);
        let (text, layouts) = edit_layouts(&obj.shape)?;
        let k = pango::SCALE as f64;
        // Zeile anhand der Höhe wählen
        let mut best = 0;
        for (i, l) in layouts.iter().enumerate() {
            let (_, logical) = l.layout.extents();
            let y0 = l.top + logical.y() as f64 / k;
            if local.y >= y0 {
                best = i;
            }
        }
        let line = layouts.get(best)?;
        // Text in Form: nur die erste Zeile des Layouts gilt
        let ly = if line.first_only {
            line.layout.baseline() as f64 / k * 0.5
        } else {
            local.y - line.top
        };
        let (_, idx, trailing) = line
            .layout
            .xy_to_index(((local.x - line.x) * k) as i32, (ly * k) as i32);
        let start = line.start;
        let idx = if line.map.is_empty() {
            idx.max(0) as usize
        } else {
            crate::model::list_from_display(&line.map, idx.max(0) as usize)
        };
        let mut i = start + idx;
        for _ in 0..trailing.max(0) {
            if let Some(c) = text[i.min(text.len())..].chars().next() {
                i += c.len_utf8();
            }
        }
        Some(i.min(text.len()))
    }

    fn text_outline(&self, obj: &Object) -> Option<Vec<SubPath>> {
        let subs = text_doc_outline(&obj.shape);
        (!subs.is_empty()).then_some(subs)
    }
}

/// Schachbrettmuster für transparente Flächen.
fn checker(light: (f64, f64), size: f64) -> Option<cairo::SurfacePattern> {
    let n = (size * 2.0).round().max(2.0) as i32;
    let s = cairo::ImageSurface::create(cairo::Format::Rgb24, n, n).ok()?;
    {
        let cr = cairo::Context::new(&s).ok()?;
        cr.set_source_rgb(light.0, light.0, light.0);
        cr.paint().ok()?;
        cr.set_source_rgb(light.1, light.1, light.1);
        let h = n as f64 / 2.0;
        cr.rectangle(0.0, 0.0, h, h);
        cr.rectangle(h, h, h, h);
        cr.fill().ok()?;
    }
    let p = cairo::SurfacePattern::create(&s);
    p.set_extend(cairo::Extend::Repeat);
    Some(p)
}

fn checkerboard() -> Option<cairo::SurfacePattern> {
    checker((0.97, 0.86), 8.0)
}

/// Pfadumriss eines Objekts in Bildschirmkoordinaten anlegen.
fn outline(cr: &cairo::Context, ed: &Editor, obj: &Object, m: &dyn Metrics) {
    let v = ed.view;
    let _ = cr.save();
    cr.translate(v.ox, v.oy);
    cr.scale(v.zoom, v.zoom);
    cr.new_path();
    if obj.shape.is_text() {
        if let Some(c) = m.text_box(obj) {
            cr.move_to(c[0].x, c[0].y);
            for p in &c[1..] {
                cr.line_to(p.x, p.y);
            }
            cr.close_path();
        }
    } else {
        build_path(cr, &obj.shape);
    }
    let _ = cr.restore(); // Pfad bleibt in Gerätekoordinaten erhalten
}

fn square(cr: &cairo::Context, p: Pt, size: f64, filled: bool) {
    let h = size / 2.0;
    cr.rectangle((p.x - h).round() + 0.5, (p.y - h).round() + 0.5, size, size);
    if filled {
        accent(cr, 1.0);
    } else {
        cr.set_source_rgb(1.0, 1.0, 1.0);
    }
    let _ = cr.fill_preserve();
    accent(cr, 1.0);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
}

fn dot(cr: &cairo::Context, p: Pt, r: f64) {
    cr.new_path();
    cr.arc(p.x, p.y, r, 0.0, TAU);
    accent(cr, 1.0);
    let _ = cr.fill();
}

/// Hintergrund um die Zeichenflächen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backdrop {
    /// helles bzw. dunkles Grau je nach Farbschema
    Default,
    /// immer dunkles Grau
    Dark,
    /// Schachbrett (außerhalb der Seite wird nichts exportiert)
    Checker,
}

impl Backdrop {
    pub const ALL: [Backdrop; 3] = [Backdrop::Default, Backdrop::Dark, Backdrop::Checker];
    pub fn name(self) -> &'static str {
        match self {
            Backdrop::Default => "default",
            Backdrop::Dark => "dark",
            Backdrop::Checker => "checker",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Backdrop::Default => tr("Standard"),
            Backdrop::Dark => tr("Dunkel"),
            Backdrop::Checker => tr("Schachbrett"),
        }
    }
    pub fn from_name(s: &str) -> Backdrop {
        Backdrop::ALL
            .into_iter()
            .find(|b| b.name() == s)
            .unwrap_or(Backdrop::Default)
    }
}

/// Gesamte Zeichenfläche: Arbeitsbereich, Seite, Objekte und Bearbeitungshilfen.
pub fn draw_canvas(
    cr: &cairo::Context,
    w: f64,
    h: f64,
    ed: &Editor,
    m: &dyn Metrics,
    dark: bool,
    unit: Unit,
    backdrop: Backdrop,
    show_rulers: bool,
    res: f64,
) {
    // „Dunkel“ gilt auch für Lineale, Raster und Hilfen
    let dark = dark || backdrop == Backdrop::Dark;
    let v = ed.view;
    let doc = &ed.doc;

    // Arbeitsbereich: Schachbrett (außerhalb der Seite wird nichts exportiert)
    let pb = match backdrop {
        Backdrop::Checker if dark => checker((0.22, 0.17), 10.0),
        Backdrop::Checker => checker((0.84, 0.76), 10.0),
        _ => None,
    };
    cr.rectangle(0.0, 0.0, w, h);
    match pb {
        Some(p) => {
            let _ = cr.set_source(&p);
        }
        None => {
            if dark {
                cr.set_source_rgb(0.14, 0.14, 0.15);
            } else {
                cr.set_source_rgb(0.87, 0.87, 0.89);
            }
        }
    }
    let _ = cr.fill();

    // Zeichenflächen (ohne Schatten); transparente Flächen als helleres Schachbrett
    // Notizheft: nur die aktuelle Seite
    let only = ed.page_index();
    let pages: Vec<(String, (f64, f64, f64, f64))> = doc
        .artboard_rects()
        .into_iter()
        .enumerate()
        .filter(|(i, _)| only.map_or(true, |o| o == *i))
        .map(|(_, (n, r))| {
            let a = v.to_screen(Pt::new(r.x0, r.y0));
            (n, (a.x, a.y, r.width() * v.zoom, r.height() * v.zoom))
        })
        .collect();
    for (_, (px, py, pw, ph)) in &pages {
        cr.rectangle(*px, *py, *pw, *ph);
    }
    match doc.background {
        Some(bg) => {
            if bg.a < 1.0 {
                if let Some(p) = checkerboard() {
                    let _ = cr.set_source(&p);
                    let _ = cr.fill_preserve();
                }
            }
            set_color(cr, bg);
            let _ = cr.fill();
        }
        None => {
            if let Some(p) = checkerboard() {
                let _ = cr.set_source(&p);
            } else {
                cr.set_source_rgb(1.0, 1.0, 1.0);
            }
            let _ = cr.fill();
        }
    }

    // Raster auf den Zeichenflächen (nur wenn am Raster eingerastet wird)
    if ed.grid_snap && ed.grid_size > 0.0 && ed.grid_size * v.zoom >= 6.0 {
        let step = ed.grid_size * v.zoom;
        for &(_, (px, py, pw, ph)) in pages.iter() {
            let _ = cr.save();
            cr.rectangle(px, py, pw, ph);
            cr.clip();
            cr.set_line_width(1.0);
            if dark {
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.12);
            } else {
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.09);
            }
            // Raster am Dokumentursprung ausrichten
            let mut x = v.ox + ((px - v.ox) / step).ceil() * step;
            while x < px + pw {
                cr.move_to(x.round() + 0.5, py);
                cr.line_to(x.round() + 0.5, py + ph);
                x += step;
            }
            let mut y = v.oy + ((py - v.oy) / step).ceil() * step;
            while y < py + ph {
                cr.move_to(px, y.round() + 0.5);
                cr.line_to(px + pw, y.round() + 0.5);
                y += step;
            }
            let _ = cr.stroke();
            let _ = cr.restore();
        }
    }

    // Objekte (Effektbilder in der Auflösung des Bildschirms, zwischengespeichert)
    let _ = cr.save();
    cr.translate(v.ox, v.oy);
    cr.scale(v.zoom, v.zoom);
    let prev_res = set_effect_resolution(res);
    fx_canvas_begin();
    if let Some(r) = ed.page_clip() {
        // Inhalte außerhalb der Seite ausblenden
        cr.rectangle(r.x0, r.y0, r.width(), r.height());
        cr.clip();
    }
    draw_document(cr, doc);
    fx_canvas_end();
    set_effect_resolution(prev_res);
    let _ = cr.restore();

    // Ränder der Zeichenflächen dezent nachziehen, bei mehreren mit Namen darüber
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.35);
    cr.set_line_width(1.0);
    for (_, (px, py, pw, ph)) in &pages {
        cr.rectangle(
            px.round() + 0.5,
            py.round() + 0.5,
            pw.round() - 1.0,
            ph.round() - 1.0,
        );
    }
    let _ = cr.stroke();
    if pages.len() > 1 {
        let _ = cr.save();
        cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
        cr.set_font_size(11.0);
        if dark {
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.75);
        } else {
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.65);
        }
        for (name, (px, py, _, _)) in &pages {
            cr.move_to(px.round(), (py - 7.0).round());
            let _ = cr.show_text(name);
        }
        let _ = cr.restore();
    }

    // Zeichenflächen-Modus (Ebenen-/Zeichenflächenleiste offen): Ränder hervorheben und
    // Anfasser zum Ändern der Größe
    let handles = ed.artboard_handles();
    if !handles.is_empty() {
        use_color(ui_accent_color());
        accent(cr, 0.9);
        cr.set_line_width(1.5);
        for (_, (px, py, pw, ph)) in &pages {
            cr.rectangle(
                px.round() + 0.5,
                py.round() + 0.5,
                pw.round() - 1.0,
                ph.round() - 1.0,
            );
        }
        let _ = cr.stroke();
        for (_, _, p) in handles {
            square(cr, p, 8.0, false);
        }
    }

    draw_guides(cr, ed, w, h);
    draw_overlay(cr, ed, m);
    draw_snap_hints(cr, ed);
    if show_rulers {
        draw_rulers(cr, w, h, ed, unit, dark);
    }
    // Wert neben dem Zeiger (Eckenradius, Drehwinkel) über allem
    draw_badge(cr, ed, unit, w, h);
}

/// Intelligente Hilfslinien: woran gerade eingerastet wird (Linie, Punkt, Beschriftung).
fn draw_snap_hints(cr: &cairo::Context, ed: &Editor) {
    let hints = ed.hints.borrow();
    if hints.is_empty() {
        return;
    }
    let v = ed.view;
    let _ = cr.save();
    let col = (0.85, 0.12, 0.6);
    cr.set_line_width(1.0);
    for hnt in hints.iter() {
        let a = v.to_screen(hnt.a);
        let b = v.to_screen(hnt.b);
        cr.set_source_rgb(col.0, col.1, col.2);
        if a.dist(b) > 0.5 {
            cr.new_path();
            cr.move_to(a.x.round() + 0.5, a.y.round() + 0.5);
            cr.line_to(b.x.round() + 0.5, b.y.round() + 0.5);
            let _ = cr.stroke();
            // kleines Kreuz am Bezugspunkt
            for q in [a, b] {
                cr.move_to(q.x - 3.0, q.y - 3.0);
                cr.line_to(q.x + 3.0, q.y + 3.0);
                cr.move_to(q.x + 3.0, q.y - 3.0);
                cr.line_to(q.x - 3.0, q.y + 3.0);
            }
            let _ = cr.stroke();
        } else {
            cr.new_path();
            cr.arc(b.x, b.y, 3.5, 0.0, TAU);
            cr.set_source_rgb(1.0, 1.0, 1.0);
            let _ = cr.fill_preserve();
            cr.set_source_rgb(col.0, col.1, col.2);
            cr.set_line_width(1.5);
            let _ = cr.stroke();
            cr.set_line_width(1.0);
        }
        if let Some(text) = hnt.label {
            cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
            cr.set_font_size(10.0);
            if let Ok(ext) = cr.text_extents(text) {
                let (tx, ty) = (b.x + 8.0, b.y - 8.0);
                cr.rectangle(
                    tx - 3.0,
                    ty - ext.height() - 3.0,
                    ext.x_advance() + 6.0,
                    ext.height() + 6.0,
                );
                cr.set_source_rgb(col.0, col.1, col.2);
                let _ = cr.fill();
                cr.move_to(tx, ty);
                cr.set_source_rgb(1.0, 1.0, 1.0);
                let _ = cr.show_text(text);
            }
        }
    }
    let _ = cr.restore();
}

/// Hilfslinien über die ganze Zeichenfläche.
fn draw_guides(cr: &cairo::Context, ed: &Editor, w: f64, h: f64) {
    if !ed.show_guides || ed.doc.guides.is_empty() {
        return;
    }
    let v = ed.view;
    let far = (w + h) * 2.0 / v.zoom.max(1e-6);
    for (i, g) in ed.doc.guides.iter().enumerate() {
        // Hilfslinien liegen auf Ebenen: ausgeblendete Ebene → unsichtbar
        if !ed.doc.guide_visible(g) {
            continue;
        }
        let d = g.dir();
        let a = v.to_screen(g.pos - d * far);
        let b = v.to_screen(g.pos + d * far);
        cr.new_path();
        if g.is_horizontal() {
            let y = a.y.round() + 0.5;
            cr.move_to(0.0, y);
            cr.line_to(w, y);
        } else if g.is_vertical() {
            let x = a.x.round() + 0.5;
            cr.move_to(x, 0.0);
            cr.line_to(x, h);
        } else {
            cr.move_to(a.x, a.y);
            cr.line_to(b.x, b.y);
        }
        let selected = ed.guide_sel == Some(i);
        if selected {
            cr.set_source_rgba(0.87, 0.18, 0.6, 1.0);
            cr.set_line_width(1.5);
        } else {
            cr.set_source_rgba(0.0, 0.68, 0.94, 0.9);
            cr.set_line_width(1.0);
        }
        let _ = cr.stroke();
    }
}

/// Verlaufslinie mit Endpunkten und Farbstopps zum direkten Bearbeiten.
fn draw_gradient_line(cr: &cairo::Context, ed: &Editor, m: &dyn Metrics) {
    let Some((id, a, b, _)) = ed.gradient_line(m) else {
        return;
    };
    let Some(g) = ed.doc.object(id).and_then(|o| o.style.gradient.as_ref()) else {
        return;
    };
    let v = ed.view;
    let (sa, sb) = (v.to_screen(a), v.to_screen(b));
    // Linie: weiß unterlegt, damit sie auf jedem Hintergrund sichtbar ist
    cr.new_path();
    cr.move_to(sa.x, sa.y);
    cr.line_to(sb.x, sb.y);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.9);
    cr.set_line_width(3.0);
    let _ = cr.stroke_preserve();
    cr.set_source_rgba(0.1, 0.1, 0.1, 0.9);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
    // Farbstopps als Kreise in ihrer Farbe
    for (i, st) in g.stops.iter().enumerate() {
        let q = sa.lerp(sb, st.pos);
        let sel = i == ed.grad_stop;
        let r = if sel { 6.5 } else { 5.0 };
        cr.new_path();
        cr.arc(q.x, q.y, r, 0.0, TAU);
        if let Some(p) = checkerboard() {
            let _ = cr.set_source(&p);
            let _ = cr.fill_preserve();
        }
        set_color(cr, st.color);
        let _ = cr.fill_preserve();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.set_line_width(2.0);
        let _ = cr.stroke_preserve();
        if sel {
            accent(cr, 1.0);
        } else {
            cr.set_source_rgba(0.1, 0.1, 0.1, 0.8);
        }
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    }
    // Endpunkte: Anfang als Quadrat, Ende als Raute
    for (k, p) in [(0, sa), (1, sb)] {
        cr.new_path();
        if k == 0 {
            cr.rectangle(p.x - 4.5, p.y - 4.5, 9.0, 9.0);
        } else {
            cr.move_to(p.x, p.y - 6.0);
            cr.line_to(p.x + 6.0, p.y);
            cr.line_to(p.x, p.y + 6.0);
            cr.line_to(p.x - 6.0, p.y);
            cr.close_path();
        }
        cr.set_source_rgb(1.0, 1.0, 1.0);
        let _ = cr.fill_preserve();
        cr.set_source_rgba(0.1, 0.1, 0.1, 0.9);
        cr.set_line_width(1.2);
        let _ = cr.stroke();
    }
}

fn draw_overlay(cr: &cairo::Context, ed: &Editor, m: &dyn Metrics) {
    let v = ed.view;
    let idle = matches!(ed.drag, Drag::None);

    // Hervorhebung unter dem Zeiger
    if let Some(id) = ed.hover.filter(|id| idle && !ed.selection.contains(id)) {
        if let Some(o) = ed.doc.object(id) {
            use_layer_color(ed, id);
            outline(cr, ed, o, m);
            accent(cr, 0.9);
            cr.set_line_width(1.5);
            let _ = cr.stroke();
        }
    }

    // Grundlinie von Text auf Pfad
    for o in ed.selected_objects() {
        use_layer_color(ed, o.id);
        if let Shape::TextPath { path, .. } = &o.shape {
            let _ = cr.save();
            cr.translate(v.ox, v.oy);
            cr.scale(v.zoom, v.zoom);
            cr.new_path();
            path_subpaths(cr, path);
            let _ = cr.restore();
            accent(cr, 0.7);
            cr.set_line_width(1.0);
            cr.set_dash(&[4.0, 3.0], 0.0);
            let _ = cr.stroke();
            cr.set_dash(&[], 0.0);
        }
    }

    // Umrisse der Auswahl (in der Farbe ihrer Ebene)
    for o in ed.selected_objects() {
        use_layer_color(ed, o.id);
        outline(cr, ed, o, m);
        accent(cr, 0.9);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
        // Textfeld mit verstecktem Text: rotes Plus unten rechts
        if let Shape::TextArea { xf, w, h, .. } = &o.shape {
            if area_overflows(&o.shape) {
                let c = v.to_screen(xf.apply(Pt::new(*w, *h)));
                cr.rectangle(c.x - 12.5, c.y - 12.5, 10.0, 10.0);
                cr.set_source_rgb(1.0, 1.0, 1.0);
                let _ = cr.fill_preserve();
                cr.set_source_rgb(0.88, 0.11, 0.14);
                cr.set_line_width(1.0);
                let _ = cr.stroke();
                cr.move_to(c.x - 7.5, c.y - 10.5);
                cr.line_to(c.x - 7.5, c.y - 4.5);
                cr.move_to(c.x - 10.5, c.y - 7.5);
                cr.line_to(c.x - 4.5, c.y - 7.5);
                let _ = cr.stroke();
            }
        }
    }
    // Gerahmtes Bild im Inhaltsmodus: ganzes Bild blass zeigen
    if let [id] = ed.selection[..] {
        if ed.image_part(id) == ImagePart::Content {
            if let Some((Shape::Image { xf, w, h, data, .. }, fx)) =
                ed.doc.object(id).map(|o| (&o.shape, &o.style.fx))
            {
                let _ = cr.save();
                cr.translate(v.ox, v.oy);
                cr.scale(v.zoom, v.zoom);
                cr.push_group();
                draw_image(cr, xf, *w, *h, data, fx);
                let _ = cr.pop_group_to_source();
                let _ = cr.paint_with_alpha(0.35);
                let _ = cr.restore();
            }
        }
    }

    // Bild schwebt über einer Form: Umriss gestrichelt, nach der Wartezeit eine
    // halbtransparente Vorschau, wie das Bild in der Form sitzen wird.
    if let Some((t, prog)) = ed.frame_progress() {
        if let Some(o) = ed.doc.object(t) {
            use_layer_color(ed, t);
            if prog >= 1.0 {
                // Vorschau: so sitzt das Bild bzw. der Text nach dem Loslassen in der Form
                let preview = match ed.selection[..] {
                    [id] => ed.frame_preview(id, t, m),
                    _ => None,
                };
                if let Some(po) = preview {
                    let _ = cr.save();
                    cr.translate(v.ox, v.oy);
                    cr.scale(v.zoom, v.zoom);
                    cr.push_group();
                    draw_object(cr, &po);
                    let _ = cr.pop_group_to_source();
                    let _ = cr.paint_with_alpha(0.6);
                    let _ = cr.restore();
                }
                outline(cr, ed, o, m);
                accent(cr, 1.0);
                cr.set_line_width(2.0);
                let _ = cr.stroke();
            } else {
                outline(cr, ed, o, m);
                accent(cr, 0.9);
                cr.set_line_width(1.5);
                cr.set_dash(&[6.0, 4.0], 0.0);
                let _ = cr.stroke();
                cr.set_dash(&[], 0.0);
                if let Some((_, _, anchor)) = ed.frame_hover {
                    wait_ring(cr, anchor, prog);
                }
            }
        }
    }

    // Formerstellung: Netz der Teilflächen, getroffene Flächen hervorgehoben
    if ed.tool == Tool::Builder {
        let ring_path = |cr: &cairo::Context, rings: &[Vec<Pt>]| {
            for r in rings {
                for (i, p) in r.iter().enumerate() {
                    let q = v.to_screen(*p);
                    if i == 0 {
                        cr.move_to(q.x, q.y);
                    } else {
                        cr.line_to(q.x, q.y);
                    }
                }
                cr.close_path();
            }
        };
        cr.new_path();
        for rings in ed.builder_all() {
            ring_path(cr, &rings);
        }
        cr.set_source_rgba(0.5, 0.5, 0.5, 0.8);
        cr.set_line_width(1.0);
        cr.set_dash(&[3.0, 3.0], 0.0);
        let _ = cr.stroke();
        cr.set_dash(&[], 0.0);
        let (marks, del) = ed.builder_marks();
        for rings in &marks {
            cr.new_path();
            ring_path(cr, rings);
            // Fläche mit Löchern
            cr.set_fill_rule(cairo::FillRule::EvenOdd);
            if del {
                cr.set_source_rgba(0.88, 0.11, 0.14, 0.3);
            } else {
                accent(cr, 0.3);
            }
            let _ = cr.fill_preserve();
            cr.set_fill_rule(cairo::FillRule::Winding);
            if del {
                cr.set_source_rgba(0.88, 0.11, 0.14, 0.9);
            } else {
                accent(cr, 0.9);
            }
            cr.set_line_width(1.5);
            let _ = cr.stroke();
        }
        if let Drag::Build { pts, alt, .. } = &ed.drag {
            cr.new_path();
            for (i, p) in pts.iter().enumerate() {
                let q = v.to_screen(*p);
                if i == 0 {
                    cr.move_to(q.x, q.y);
                } else {
                    cr.line_to(q.x, q.y);
                }
            }
            if *alt {
                cr.set_source_rgba(0.88, 0.11, 0.14, 0.9);
            } else {
                accent(cr, 0.9);
            }
            cr.set_line_width(2.0);
            cr.set_line_cap(cairo::LineCap::Round);
            cr.set_line_join(cairo::LineJoin::Round);
            let _ = cr.stroke();
            cr.set_line_cap(cairo::LineCap::Butt);
            cr.set_line_join(cairo::LineJoin::Miter);
        }
    }

    // Radierer: Spur und Pinselkreis
    if ed.tool == Tool::Eraser {
        let rad = ed.eraser_size / 2.0 * v.zoom;
        if let Drag::Erase { pts } = &ed.drag {
            cr.new_path();
            for (i, p) in pts.iter().enumerate() {
                let q = v.to_screen(*p);
                if i == 0 {
                    cr.move_to(q.x, q.y);
                } else {
                    cr.line_to(q.x, q.y);
                }
            }
            if pts.len() == 1 {
                let q = v.to_screen(pts[0]);
                cr.line_to(q.x + 0.01, q.y);
            }
            cr.set_source_rgba(0.88, 0.11, 0.14, 0.25);
            cr.set_line_width(rad * 2.0);
            cr.set_line_cap(cairo::LineCap::Round);
            cr.set_line_join(cairo::LineJoin::Round);
            let _ = cr.stroke();
            cr.set_line_cap(cairo::LineCap::Butt);
            cr.set_line_join(cairo::LineJoin::Miter);
        }
        if let Some(p) = ed.pointer {
            let q = v.to_screen(p);
            cr.new_path();
            cr.arc(q.x, q.y, rad.max(1.0), 0.0, TAU);
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.9);
            cr.set_line_width(3.0);
            let _ = cr.stroke_preserve();
            cr.set_source_rgba(0.1, 0.1, 0.1, 0.9);
            cr.set_line_width(1.0);
            let _ = cr.stroke();
        }
    }

    // Schere: gestrichelte Schnittlinie
    if ed.tool == Tool::Scissors {
        if let Drag::Erase { pts } = &ed.drag {
            cr.new_path();
            for (i, p) in pts.iter().enumerate() {
                let q = v.to_screen(*p);
                if i == 0 {
                    cr.move_to(q.x, q.y);
                } else {
                    cr.line_to(q.x, q.y);
                }
            }
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.9);
            cr.set_line_width(3.0);
            let _ = cr.stroke_preserve();
            cr.set_source_rgba(0.88, 0.11, 0.14, 1.0);
            cr.set_line_width(1.5);
            cr.set_dash(&[6.0, 4.0], 0.0);
            let _ = cr.stroke();
            cr.set_dash(&[], 0.0);
        }
    }

    // Aus der Bibliothek über ein Objekt gezogen: erst gestrichelter Umriss, nach der
    // Wartezeit eine halbtransparente Vorschau mit dem neuen Stil
    if let Some((t, prog)) = ed.lib_drag_progress() {
        if let Some(o) = ed.doc.object(t) {
            use_layer_color(ed, t);
            if prog >= 1.0 {
                if let Some(po) = ed.lib_drag_preview() {
                    let _ = cr.save();
                    cr.translate(v.ox, v.oy);
                    cr.scale(v.zoom, v.zoom);
                    cr.push_group();
                    draw_object(cr, &po);
                    let _ = cr.pop_group_to_source();
                    let _ = cr.paint_with_alpha(0.75);
                    let _ = cr.restore();
                }
                outline(cr, ed, o, m);
                accent(cr, 1.0);
                cr.set_line_width(2.0);
                let _ = cr.stroke();
            } else {
                outline(cr, ed, o, m);
                accent(cr, 0.9);
                cr.set_line_width(1.5);
                cr.set_dash(&[6.0, 4.0], 0.0);
                let _ = cr.stroke();
                cr.set_dash(&[], 0.0);
                if let Some(p) = ed.pointer {
                    wait_ring(cr, v.to_screen(p), prog);
                }
            }
        }
    }

    if let Some(&first) = ed.selection.first() {
        use_layer_color(ed, first);
    } else {
        use_active_layer_color(ed);
    }

    match ed.tool {
        Tool::Select => {
            if !matches!(ed.drag, Drag::Rubber { .. }) {
                if let Some(hs) = ed.handle_positions(m) {
                    // Rahmen durch die Eckanfasser (bei gedrehter Auswahl gedreht)
                    let straight = (hs[0].y - hs[2].y).abs() < 0.01 && (hs[0].x - hs[6].x).abs() < 0.01;
                    let px = |q: Pt| {
                        if straight {
                            Pt::new(q.x.round() + 0.5, q.y.round() + 0.5)
                        } else {
                            q
                        }
                    };
                    cr.move_to(px(hs[0]).x, px(hs[0]).y);
                    for k in [2usize, 4, 6] {
                        cr.line_to(px(hs[k]).x, px(hs[k]).y);
                    }
                    cr.close_path();
                    accent(cr, 1.0);
                    cr.set_line_width(1.0);
                    let _ = cr.stroke();
                    for hp in hs {
                        square(cr, hp, 7.0, false);
                    }
                    // Drehgriff
                    if let Some(rh) = ed.rotate_handle(m) {
                        let dir = (rh - hs[1]).normalized();
                        let (s0, s1) = (hs[1] + dir * 4.0, rh - dir * 4.5);
                        cr.move_to(s0.x, s0.y);
                        cr.line_to(s1.x, s1.y);
                        accent(cr, 1.0);
                        cr.set_line_width(1.0);
                        let _ = cr.stroke();
                        cr.new_path();
                        cr.arc(rh.x, rh.y, 4.5, 0.0, TAU);
                        cr.set_source_rgb(1.0, 1.0, 1.0);
                        let _ = cr.fill_preserve();
                        accent(cr, 1.0);
                        let _ = cr.stroke();
                    }
                }
            }
        }
        Tool::Node => draw_nodes(cr, ed),
        _ => {}
    }
    draw_gradient_line(cr, ed, m);

    match &ed.drag {
        Drag::Rubber { start, cur, .. } => {
            let a = v.to_screen(*start);
            let b = v.to_screen(*cur);
            let r = Rect::from_points(a, b);
            cr.rectangle(
                r.x0.round() + 0.5,
                r.y0.round() + 0.5,
                r.width().round(),
                r.height().round(),
            );
            accent(cr, 0.15);
            let _ = cr.fill_preserve();
            accent(cr, 0.95);
            cr.set_line_width(1.0);
            let _ = cr.stroke();
        }
        // Rahmen beim Auswählen von Knoten
        Drag::NodeRubber { start, cur, .. } => {
            let a = v.to_screen(*start);
            let b = v.to_screen(*cur);
            let r = Rect::from_points(a, b);
            cr.rectangle(
                r.x0.round() + 0.5,
                r.y0.round() + 0.5,
                r.width().round(),
                r.height().round(),
            );
            accent(cr, 0.15);
            let _ = cr.fill_preserve();
            accent(cr, 0.95);
            cr.set_line_width(1.0);
            let _ = cr.stroke();
        }
        Drag::Rotate { .. } => {
            if let (Some(p), Some(o)) = (ed.pointer, ed.selected_objects().first()) {
                let sp = v.to_screen(p);
                label(cr, Pt::new(sp.x + 14.0, sp.y + 22.0), &format!("{:.0}°", o.angle));
            }
        }
        Drag::Pencil { pts } => {
            cr.new_path();
            for (i, p) in pts.iter().enumerate() {
                let s = v.to_screen(*p);
                if i == 0 {
                    cr.move_to(s.x, s.y);
                } else {
                    cr.line_to(s.x, s.y);
                }
            }
            let c = ed.defaults.stroke.or(ed.defaults.fill).unwrap_or(Color::BLACK);
            set_color(cr, c);
            cr.set_line_width((ed.defaults.width * v.zoom).max(1.0));
            cr.set_line_cap(cairo::LineCap::Round);
            cr.set_line_join(cairo::LineJoin::Round);
            let _ = cr.stroke();
        }
        _ => {}
    }

    // Textbearbeitung: Markierung und Cursor
    if let Some(te) = ed.text_edit {
        if let Some(o) = ed.doc.object(te.id) {
            use_layer_color(ed, o.id);
            let (a, b) = te.range();
            for q in selection_quads(&o.shape, a, b) {
                let s: Vec<Pt> = q.iter().map(|p| v.to_screen(*p)).collect();
                cr.move_to(s[0].x, s[0].y);
                for p in &s[1..] {
                    cr.line_to(p.x, p.y);
                }
                cr.close_path();
            }
            accent(cr, 0.3);
            let _ = cr.fill();
            if let Some((p0, p1)) = caret_line(&o.shape, te.caret) {
                let (a, b) = (v.to_screen(p0), v.to_screen(p1));
                cr.move_to(a.x, a.y);
                cr.line_to(b.x, b.y);
                accent(cr, 1.0);
                cr.set_line_width(1.5);
                let _ = cr.stroke();
            }
        }
    }

    // Griff am Anfang von Text auf Pfad
    if let Some((id, g, ang)) = ed.textpath_grip() {
        use_layer_color(ed, id);
        let s = v.to_screen(g);
        let n = Pt::new(-ang.sin(), ang.cos());
        cr.move_to(s.x - n.x * 12.0, s.y - n.y * 12.0);
        cr.line_to(s.x + n.x * 6.0, s.y + n.y * 6.0);
        accent(cr, 1.0);
        cr.set_line_width(2.0);
        let _ = cr.stroke();
        cr.new_path();
        cr.arc(s.x, s.y, 4.5, 0.0, TAU);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        let _ = cr.fill_preserve();
        accent(cr, 1.0);
        cr.set_line_width(1.2);
        let _ = cr.stroke();
    }

    use_active_layer_color(ed);
    if let Some(nodes) = &ed.pen {
        draw_pen(cr, ed, nodes);
    }
    if let Some((nodes, closed)) = ed.curvature_preview() {
        draw_curvature(cr, ed, &nodes, closed);
    }
}

fn draw_curvature(cr: &cairo::Context, ed: &Editor, nodes: &[Node], closed: bool) {
    let v = ed.view;
    let Some(first) = nodes.first() else { return };
    cr.new_path();
    let f = v.to_screen(first.p);
    cr.move_to(f.x, f.y);
    for w in nodes.windows(2) {
        let c1 = v.to_screen(w[0].h_out);
        let c2 = v.to_screen(w[1].h_in);
        let p = v.to_screen(w[1].p);
        cr.curve_to(c1.x, c1.y, c2.x, c2.y, p.x, p.y);
    }
    if closed {
        // Zeiger über dem Startpunkt: die Linie schließt sich so, wie der Klick sie schließt
        if let Some(last) = nodes.last() {
            let c1 = v.to_screen(last.h_out);
            let c2 = v.to_screen(first.h_in);
            cr.curve_to(c1.x, c1.y, c2.x, c2.y, f.x, f.y);
        }
        cr.close_path();
    }
    let c = ed.defaults.stroke.unwrap_or(Color::BLACK);
    set_color(cr, c);
    cr.set_line_width((ed.defaults.width * v.zoom).clamp(1.0, 4.0));
    let _ = cr.stroke_preserve();
    accent(cr, 0.9);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
    // gesetzte Punkte (ohne Zeigerpunkt); Eckpunkte als Quadrat, sonst als Kreis
    if let Some(pts) = &ed.curv {
        let near_first = ed.curvature_near_start();
        for (i, (p, corner)) in pts.iter().enumerate() {
            let s = v.to_screen(*p);
            let big = i == 0 && near_first;
            if *corner {
                square(cr, s, if big { 10.0 } else { 7.0 }, false);
            } else {
                cr.new_path();
                cr.arc(s.x, s.y, if big { 5.5 } else { 4.0 }, 0.0, TAU);
                cr.set_source_rgb(1.0, 1.0, 1.0);
                let _ = cr.fill_preserve();
                accent(cr, 1.0);
                cr.set_line_width(1.0);
                let _ = cr.stroke();
            }
        }
    }
}

/// Wartezeit bis zur Vorschau (Einrasten): Ring, der sich im Uhrzeigersinn füllt –
/// einheitlich für Kurvenzeichner, Bilder/Text in Formen und Bibliothek.
fn wait_ring(cr: &cairo::Context, c: Pt, prog: f64) {
    let r = 11.0;
    cr.new_path();
    cr.arc(c.x, c.y, r, 0.0, TAU);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.75);
    cr.set_line_width(4.5);
    let _ = cr.stroke();
    cr.new_path();
    cr.arc(c.x, c.y, r, 0.0, TAU);
    accent(cr, 0.25);
    cr.set_line_width(2.5);
    let _ = cr.stroke();
    if prog > 0.0 {
        cr.new_path();
        cr.arc(c.x, c.y, r, -TAU / 4.0, -TAU / 4.0 + TAU * prog.min(1.0));
        accent(cr, 1.0);
        cr.set_line_width(2.5);
        let _ = cr.stroke();
    }
}

fn draw_nodes(cr: &cairo::Context, ed: &Editor) {
    let v = ed.view;
    // Knoten aller ausgewählten Objekte (auch mehrerer zugleich)
    let views = ed.node_views();
    for (id, subpaths) in &views {
        use_layer_color(ed, *id);
        // Pfadumriss (bei Formen, die noch keine Pfade sind, sonst unsichtbar)
        let _ = cr.save();
        cr.translate(v.ox, v.oy);
        cr.scale(v.zoom, v.zoom);
        cr.new_path();
        path_subpaths(cr, &crate::model::effective_subpaths(subpaths));
        let _ = cr.restore();
        accent(cr, 0.6);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    }
    // Anfasserlinien
    for (id, si, ni, _part, hp) in ed.visible_handles() {
        let Some(n) = views
            .iter()
            .find(|w| w.0 == id)
            .and_then(|w| w.1.get(si))
            .and_then(|sp| sp.nodes.get(ni))
        else {
            continue;
        };
        use_layer_color(ed, id);
        let a = v.to_screen(n.p);
        let b = v.to_screen(hp);
        cr.move_to(a.x, a.y);
        cr.line_to(b.x, b.y);
        accent(cr, 0.8);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
        dot(cr, b, 3.5);
    }
    for (id, subpaths) in &views {
        use_layer_color(ed, *id);
        for (si, sp) in subpaths.iter().enumerate() {
            for (ni, n) in sp.nodes.iter().enumerate() {
                let sel = ed.node_sel.contains(&(*id, si, ni));
                square(cr, v.to_screen(n.p), if sel { 8.0 } else { 7.0 }, sel);
            }
        }
    }
    // Ziehpunkte für den Eckenradius (kleiner Kreis mit Punkt)
    for (w, _, _) in ed.corner_widgets() {
        let s = v.to_screen(w);
        cr.new_path();
        cr.arc(s.x, s.y, 5.0, 0.0, TAU);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        let _ = cr.fill_preserve();
        accent(cr, 1.0);
        cr.set_line_width(1.2);
        let _ = cr.stroke();
        dot(cr, s, 1.6);
    }
}

fn draw_pen(cr: &cairo::Context, ed: &Editor, nodes: &[crate::model::Node]) {
    let v = ed.view;
    let Some(first) = nodes.first() else { return };
    cr.new_path();
    let f = v.to_screen(first.p);
    cr.move_to(f.x, f.y);
    for w in nodes.windows(2) {
        let (a, b) = (w[0], w[1]);
        let c1 = v.to_screen(a.h_out);
        let c2 = v.to_screen(b.h_in);
        let p = v.to_screen(b.p);
        cr.curve_to(c1.x, c1.y, c2.x, c2.y, p.x, p.y);
    }
    // Vorschau zum Zeiger
    let last = nodes[nodes.len() - 1];
    let ptr = ed.pointer.filter(|_| !matches!(ed.drag, Drag::PenHandle));
    if let Some(ptr) = ptr {
        let c1 = v.to_screen(last.h_out);
        let p = v.to_screen(ptr);
        cr.curve_to(c1.x, c1.y, p.x, p.y, p.x, p.y);
    }
    let c = ed.defaults.stroke.unwrap_or(Color::BLACK);
    set_color(cr, c);
    cr.set_line_width((ed.defaults.width * v.zoom).clamp(1.0, 4.0));
    let _ = cr.stroke_preserve();
    accent(cr, 0.9);
    cr.set_line_width(1.0);
    let _ = cr.stroke();

    // Anfasser des letzten Punktes
    if last.h_out.dist(last.p) > 1e-9 {
        let a = v.to_screen(last.h_in);
        let b = v.to_screen(last.h_out);
        cr.move_to(a.x, a.y);
        cr.line_to(b.x, b.y);
        accent(cr, 0.8);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
        dot(cr, a, 3.0);
        dot(cr, b, 3.0);
    }
    // Punkte; der erste wird größer, wenn ein Klick den Pfad schließen würde
    let near_first = ptr
        .map(|p| nodes.len() >= 2 && v.to_screen(p).dist(f) <= HANDLE_RADIUS + 1.0)
        .unwrap_or(false);
    for (i, n) in nodes.iter().enumerate() {
        let big = i == 0 && near_first;
        square(
            cr,
            v.to_screen(n.p),
            if big { 10.0 } else { 7.0 },
            i == nodes.len() - 1,
        );
    }
}

/// Kleine Beschriftung mit Hintergrund (z. B. Drehwinkel).
fn label(cr: &cairo::Context, at: Pt, text: &str) {
    cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
    cr.set_font_size(11.0);
    let w = cr.text_extents(text).map(|e| e.x_advance()).unwrap_or(30.0);
    cr.new_path();
    cr.rectangle(at.x - 5.0, at.y - 13.0, w + 10.0, 18.0);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.75);
    let _ = cr.fill();
    cr.move_to(at.x, at.y);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    let _ = cr.show_text(text);
}

// ───────────────────────────── Lineale ─────────────────────────────

pub const RULER: f64 = 20.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Px,
    Mm,
    Cm,
    /// Zoll (imperial)
    In,
    /// Punkt, 1/72 Zoll (imperial/typografisch)
    Pt,
}

impl Unit {
    pub const ALL: [Unit; 5] = [Unit::Px, Unit::Mm, Unit::Cm, Unit::In, Unit::Pt];

    /// Name für Auswahllisten, z. B. „Zoll (in)“.
    pub fn label(self) -> &'static str {
        match self {
            Unit::Px => tr("Pixel (px)"),
            Unit::Mm => tr("Millimeter (mm)"),
            Unit::Cm => tr("Zentimeter (cm)"),
            Unit::In => tr("Zoll (in)"),
            Unit::Pt => tr("Punkt (pt)"),
        }
    }

    /// Pixel pro Einheit (96 dpi).
    pub fn px(self) -> f64 {
        match self {
            Unit::Px => 1.0,
            Unit::Mm => 96.0 / 25.4,
            Unit::Cm => 96.0 / 2.54,
            Unit::In => 96.0,
            Unit::Pt => 96.0 / 72.0,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Unit::Px => "px",
            Unit::Mm => "mm",
            Unit::Cm => "cm",
            Unit::In => "in",
            Unit::Pt => "pt",
        }
    }
    pub fn from_name(s: &str) -> Option<Unit> {
        match s {
            "px" => Some(Unit::Px),
            "mm" => Some(Unit::Mm),
            "cm" => Some(Unit::Cm),
            "in" => Some(Unit::In),
            "pt" => Some(Unit::Pt),
            _ => None,
        }
    }
    pub fn next(self) -> Unit {
        match self {
            Unit::Px => Unit::Mm,
            Unit::Mm => Unit::Cm,
            Unit::Cm => Unit::In,
            Unit::In => Unit::Pt,
            Unit::Pt => Unit::Px,
        }
    }
}

/// Schöne Schrittweite (1, 2, 5 × 10ⁿ) ≥ `v`.
fn nice_step(v: f64) -> (f64, u32) {
    let e = v.max(1e-9).log10().floor();
    let base = 10f64.powf(e);
    for (m, minor) in [(1.0, 5), (2.0, 4), (5.0, 5), (10.0, 5)] {
        if base * m >= v {
            return (base * m, minor);
        }
    }
    (base * 10.0, 5)
}

fn fmt_tick(v: f64, step: f64) -> String {
    if step >= 1.0 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{:.1}", v);
        s.replace('.', ",")
    }
}

/// Kleines Schild neben dem Zeiger: Eckenradius oder Drehwinkel beim Ziehen.
fn draw_badge(cr: &cairo::Context, ed: &Editor, unit: Unit, w: f64, h: f64) {
    let Some((at, badge)) = ed.badge else { return };
    let text = match badge {
        crate::editor::Badge::Radius(r) => {
            let v = r / unit.px();
            let v = if unit.px() > 1.5 {
                (v * 10.0).round() / 10.0
            } else {
                v.round()
            };
            trf("Radius {} {}", &[&(v), &(unit.name())])
        }
        crate::editor::Badge::Angle(a) => {
            let a = (a * 10.0).round() / 10.0;
            format!("{a}°")
        }
    };
    let _ = cr.save();
    cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
    cr.set_font_size(12.0);
    let Ok(ext) = cr.text_extents(&text) else {
        let _ = cr.restore();
        return;
    };
    let (pw, ph) = (ext.x_advance() + 16.0, 24.0);
    let x = (at.x + 18.0).min(w - pw - 4.0).max(4.0);
    let y = (at.y + 18.0).min(h - ph - 4.0).max(4.0);
    let r = 6.0;
    cr.new_sub_path();
    cr.arc(x + pw - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + pw - r, y + ph - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(
        x + r,
        y + ph - r,
        r,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    cr.arc(x + r, y + r, r, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
    cr.close_path();
    cr.set_source_rgba(0.14, 0.14, 0.16, 0.9);
    let _ = cr.fill();
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.move_to(x + 8.0, y + ph / 2.0 - (ext.y_bearing() + ext.height() / 2.0));
    let _ = cr.show_text(&text);
    let _ = cr.restore();
}

fn draw_rulers(cr: &cairo::Context, w: f64, h: f64, ed: &Editor, unit: Unit, dark: bool) {
    let v = ed.view;
    let k = v.zoom * unit.px(); // Bildschirmpixel pro Einheit
    let (step, minors) = nice_step(64.0 / k);
    let minor = step / minors as f64;
    let (bg, fg, line) = if dark {
        ((0.17, 0.17, 0.18), (0.75, 0.75, 0.77), (0.3, 0.3, 0.32))
    } else {
        ((0.97, 0.97, 0.98), (0.35, 0.35, 0.38), (0.78, 0.78, 0.8))
    };
    let pointer = ed.pointer.map(|p| v.to_screen(p));

    let _ = cr.save();
    cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(9.0);

    // Hintergründe
    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(0.0, 0.0, w, RULER);
    cr.rectangle(0.0, 0.0, RULER, h);
    let _ = cr.fill();
    cr.set_source_rgb(line.0, line.1, line.2);
    cr.set_line_width(1.0);
    cr.move_to(RULER, RULER - 0.5);
    cr.line_to(w, RULER - 0.5);
    cr.move_to(RULER - 0.5, RULER);
    cr.line_to(RULER - 0.5, h);
    let _ = cr.stroke();

    cr.set_source_rgb(fg.0, fg.1, fg.2);
    // oben
    let first = ((RULER - v.ox) / k / minor).floor() as i64;
    let last = ((w - v.ox) / k / minor).ceil() as i64;
    for i in first..=last {
        let u = i as f64 * minor;
        let x = (v.ox + u * k).round() + 0.5;
        if x < RULER {
            continue;
        }
        let major = i.rem_euclid(minors as i64) == 0;
        let len = if major { 9.0 } else { 4.0 };
        cr.move_to(x, RULER);
        cr.line_to(x, RULER - len);
        if major {
            cr.move_to(x + 3.0, 9.0);
            let _ = cr.show_text(&fmt_tick(u, step));
        }
    }
    let _ = cr.stroke();
    // links (Beschriftung gedreht)
    let first = ((RULER - v.oy) / k / minor).floor() as i64;
    let last = ((h - v.oy) / k / minor).ceil() as i64;
    for i in first..=last {
        let u = i as f64 * minor;
        let y = (v.oy + u * k).round() + 0.5;
        if y < RULER {
            continue;
        }
        let major = i.rem_euclid(minors as i64) == 0;
        let len = if major { 9.0 } else { 4.0 };
        cr.move_to(RULER, y);
        cr.line_to(RULER - len, y);
        let _ = cr.stroke();
        if major {
            let _ = cr.save();
            cr.translate(9.0, y - 3.0);
            cr.rotate(-std::f64::consts::FRAC_PI_2);
            cr.move_to(0.0, 0.0);
            let _ = cr.show_text(&fmt_tick(u, step));
            let _ = cr.restore();
        }
    }

    // Zeigerposition
    if let Some(p) = pointer {
        accent(cr, 1.0);
        if p.x > RULER {
            cr.move_to(p.x.round() + 0.5, 0.0);
            cr.line_to(p.x.round() + 0.5, RULER);
        }
        if p.y > RULER {
            cr.move_to(0.0, p.y.round() + 0.5);
            cr.line_to(RULER, p.y.round() + 0.5);
        }
        let _ = cr.stroke();
    }

    // Ecke mit Einheit (anklickbar)
    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(0.0, 0.0, RULER, RULER);
    let _ = cr.fill();
    cr.set_source_rgb(fg.0, fg.1, fg.2);
    let name = unit.name();
    let tw = cr.text_extents(name).map(|e| e.x_advance()).unwrap_or(12.0);
    cr.move_to(((RULER - tw) / 2.0).round(), 13.0);
    let _ = cr.show_text(name);
    let _ = cr.restore();
}

// ───────────────────────────── Vorschaubilder ─────────────────────────────

/// Kleines Vorschaubild einer Ebene.
pub fn draw_thumbnail(cr: &cairo::Context, w: f64, h: f64, doc: &Document, layer: &Layer, dark: bool) {
    thumbnail_frame(cr, w, h, doc, dark, |cr| draw_layer(cr, layer));
}

/// Vorschaubild einer Ebene nur mit ihren Objekten (ohne Zeichenfläche dahinter);
/// Lage wie auf der Seite, was darüber hinausragt, wird am Bildrand abgeschnitten.
pub fn draw_layer_objects(
    cr: &cairo::Context,
    w: f64,
    h: f64,
    frame: Rect,
    boards: &[Rect],
    dark: bool,
    layer: &Layer,
) {
    // `frame`: gezeigter Ausschnitt in Dokumentkoordinaten – für alle Ebenen derselbe
    // (alle Zeichenflächen und aller Inhalt), damit die Vorschaubilder vergleichbar bleiben
    let pad = 6.0;
    let (fw, fh) = (frame.width().max(1e-6), frame.height().max(1e-6));
    let s = ((w - 2.0 * pad) / fw).min((h - 2.0 * pad) / fh).max(1e-9);
    let (x, y) = ((w - fw * s) / 2.0, (h - fh * s) / 2.0);
    let _ = cr.save();
    cr.rectangle(0.0, 0.0, w, h);
    cr.clip();
    // Zeichenflächen nur andeuten (bei mehreren sieht man, wo was liegt)
    if boards.len() > 1 {
        for b in boards {
            cr.rectangle(
                (x + (b.x0 - frame.x0) * s).round() + 0.5,
                (y + (b.y0 - frame.y0) * s).round() + 0.5,
                (b.width() * s).round(),
                (b.height() * s).round(),
            );
        }
        let a = if dark { 0.18 } else { 0.14 };
        let c = if dark { 1.0 } else { 0.0 };
        cr.set_source_rgba(c, c, c, a);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    }
    cr.translate(x - frame.x0 * s, y - frame.y0 * s);
    cr.scale(s, s);
    draw_layer(cr, layer);
    let _ = cr.restore();
}

/// Vorschaubild des ganzen Dokuments (alle sichtbaren Ebenen).
pub fn draw_document_thumbnail(cr: &cairo::Context, w: f64, h: f64, doc: &Document, dark: bool) {
    thumbnail_frame(cr, w, h, doc, dark, |cr| draw_document(cr, doc));
}

/// Lage der Seite in einem Vorschaubild der Größe `w`×`h` (x, y, Breite, Höhe).
pub fn thumb_page_rect(w: f64, h: f64, doc: &Document) -> (f64, f64, f64, f64) {
    let s = ((w - 2.0) / doc.width).min((h - 2.0) / doc.height);
    let (pw, ph) = (doc.width * s, doc.height * s);
    (((w - pw) / 2.0).round(), ((h - ph) / 2.0).round(), pw, ph)
}

fn thumbnail_frame(
    cr: &cairo::Context,
    w: f64,
    h: f64,
    doc: &Document,
    dark: bool,
    content: impl FnOnce(&cairo::Context),
) {
    let (x, y, pw, ph) = thumb_page_rect(w, h, doc);
    let s = pw / doc.width;
    cr.rectangle(x, y, pw, ph);
    match doc.background {
        Some(bg) => set_color(cr, bg),
        None => {
            if let Some(p) = checkerboard() {
                let _ = cr.set_source(&p);
            }
        }
    }
    let _ = cr.fill();
    let _ = cr.save();
    cr.rectangle(x, y, pw, ph);
    cr.clip();
    cr.translate(x, y);
    cr.scale(s, s);
    content(cr);
    let _ = cr.restore();
    if dark {
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.25);
    } else {
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.25);
    }
    cr.set_line_width(1.0);
    cr.rectangle(x + 0.5, y + 0.5, pw - 1.0, ph - 1.0);
    let _ = cr.stroke();
}

/// Vorschaubild der Zeichenfläche (nur Hintergrund und Format).
pub fn draw_artboard_thumbnail(cr: &cairo::Context, w: f64, h: f64, doc: &Document, dark: bool) {
    let empty = Layer {
        color: 0,
        id: 0,
        name: String::new(),
        visible: true,
        locked: false,
        objects: Vec::new(),
    };
    draw_thumbnail(cr, w, h, doc, &empty, dark);
}

/// Vorschau eines Bibliotheksstils: Rechteck (bzw. Linie) oder „Aa“ in der Schrift.
pub fn draw_style_preview(cr: &cairo::Context, w: f64, h: f64, s: &crate::library::LibStyle) {
    let shape = match &s.text {
        Some(t) => Shape::Text {
            xf: Affine::IDENTITY,
            text: "Aa".into(),
            family: t.family.clone(),
            size: t.size.max(1.0),
            bold: t.bold,
            italic: t.italic,
            fmt: t.fmt,
        },
        None if s.style.marker != crate::model::Marker::None => Shape::Path {
            subpaths: vec![SubPath {
                nodes: vec![
                    Node::corner(Pt::new(0.0, 20.0)),
                    Node::corner(Pt::new(60.0, 20.0)),
                ],
                closed: false,
            }],
            evenodd: false,
        },
        // Stile als Kreis (wie die Farben)
        None => Shape::Ellipse {
            cx: 20.0,
            cy: 20.0,
            rx: 20.0,
            ry: 20.0,
        },
    };
    let obj = Object {
        group: 0,
        locked: false,
        name: String::new(),
        id: 0,
        shape,
        style: s.style.clone(),
        angle: 0.0,
    };
    draw_objects_preview(cr, w, h, std::slice::from_ref(&obj), 4.0);
}

/// Objekte eingepasst in ein Feld zeichnen (Vorschau in der Bibliothek). Gezeichnet wird
/// in ein eigenes Bild (doppelte Auflösung), damit Schatten und Effekte vollständig
/// und unabhängig vom Widget-Zuschnitt erscheinen.
pub fn draw_objects_preview(cr: &cairo::Context, w: f64, h: f64, objs: &[Object], pad: f64) {
    // Rahmen inkl. Kontur und Schatten
    let mut bb: Option<Rect> = None;
    for o in objs {
        let Some(b) = object_box(o) else { continue };
        let mut b = b.inflate(if o.style.stroke.is_some() {
            o.style.width / 2.0
        } else {
            0.0
        });
        if let Some(sh) = o.style.shadow {
            let mut sb = b.inflate(sh.blur);
            sb.x0 += sh.dx;
            sb.x1 += sh.dx;
            sb.y0 += sh.dy;
            sb.y1 += sh.dy;
            b = b.union(&sb);
        }
        bb = Some(match bb {
            Some(x) => x.union(&b),
            None => b,
        });
    }
    let Some(b) = bb else { return };
    let (bw, bh) = (b.width(), b.height());
    if bw <= 0.0 || bh <= 0.0 || w <= 0.0 || h <= 0.0 {
        return;
    }
    let k = ((w - 2.0 * pad) / bw).min((h - 2.0 * pad) / bh);
    let q = 2.0;
    let Ok(surf) = cairo::ImageSurface::create(
        cairo::Format::ARgb32,
        (w * q).ceil() as i32,
        (h * q).ceil() as i32,
    ) else {
        return;
    };
    if let Ok(c2) = cairo::Context::new(&surf) {
        c2.scale(q, q);
        c2.translate(w / 2.0, h / 2.0);
        c2.scale(k, k);
        c2.translate(-(b.x0 + b.x1) / 2.0, -(b.y0 + b.y1) / 2.0);
        for o in objs {
            draw_object(&c2, o);
        }
    }
    surf.flush();
    let _ = cr.save();
    cr.scale(1.0 / q, 1.0 / q);
    let _ = cr.set_source_surface(&surf, 0.0, 0.0);
    let _ = cr.paint();
    let _ = cr.restore();
}

fn cairo_join(j: crate::model::LineJoin) -> cairo::LineJoin {
    match j {
        crate::model::LineJoin::Miter => cairo::LineJoin::Miter,
        crate::model::LineJoin::Round => cairo::LineJoin::Round,
        crate::model::LineJoin::Bevel => cairo::LineJoin::Bevel,
    }
}

fn cairo_cap(c: crate::model::LineCap) -> cairo::LineCap {
    match c {
        crate::model::LineCap::Butt => cairo::LineCap::Butt,
        crate::model::LineCap::Round => cairo::LineCap::Round,
        crate::model::LineCap::Square => cairo::LineCap::Square,
    }
}
