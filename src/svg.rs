// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! SVG lesen und schreiben. Ebenen werden als Gruppen mit dem verbreiteten
//! `inkscape:groupmode="layer"` gespeichert, damit andere Programme sie erkennen.

use crate::i18n::{tr, trf};
use std::collections::HashMap;
use std::rc::Rc;

use crate::geom::{arc_to_cubics, parse_numbers, Affine, NumLexer, Pt};
use crate::model::{
    Align, Blend, Color, Document, Gradient, GradientKind, Guide, Layer, LineCap, LineJoin, ListStyle,
    Marker, MarkerEnds, Node, Object, ParaFmt, RichText, RunStyle, Shape, Stop, Style, SubPath, TextFormat,
    TextFrame, TextRun, VAlign,
};
use crate::xml::{self, escape, Element, XNode};

/// Abstand Feldoberkante → erste Grundlinie bei Textfeldern (in Schriftgrößen).
pub const AREA_ASCENT: f64 = 0.9;
const BG_ID: &str = "mondrian-background";

/// Zahl kompakt formatieren (höchstens 3 Nachkommastellen).
pub fn num(v: f64) -> String {
    let mut s = format!("{:.3}", v);
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" {
        s = "0".into();
    }
    s
}

// ───────────────────────────── Schreiben ─────────────────────────────

/// Alle im Dokument verwendeten Schriften: (Familie, fett, kursiv).
pub fn used_fonts(doc: &Document) -> Vec<(String, bool, bool)> {
    let mut set = std::collections::BTreeSet::new();
    for l in &doc.layers {
        for o in &l.objects {
            let Some((_, family, _, bold, italic)) = o.shape.text_fields() else {
                continue;
            };
            set.insert((family.to_string(), bold, italic));
            if let Some(r) = o.shape.rich() {
                for run in &r.runs {
                    let st = &run.style;
                    if st.family.is_some() || st.bold.is_some() || st.italic.is_some() {
                        set.insert((
                            st.family.clone().unwrap_or_else(|| family.to_string()),
                            st.bold.unwrap_or(bold),
                            st.italic.unwrap_or(italic),
                        ));
                    }
                }
            }
        }
    }
    set.into_iter().collect()
}

/// Wie `write`, mit den verwendeten Schriften als eingebettete `@font-face`-Regeln.
/// `font` liefert zu (Familie, fett, kursiv) die Schriftdatei und ihren Typ (`ttf`/`otf`).
pub fn write_with_fonts(
    doc: &Document,
    include_background: bool,
    font: &dyn Fn(&str, bool, bool) -> Option<(Vec<u8>, &'static str)>,
) -> String {
    let out = write(doc, include_background);
    let mut css = String::new();
    for (family, bold, italic) in used_fonts(doc) {
        let Some((data, kind)) = font(&family, bold, italic) else {
            continue;
        };
        css.push_str(&format!(
            "@font-face{{font-family:\"{}\";font-weight:{};font-style:{};src:url(data:font/{kind};base64,{}) format(\"{}\");}}\n",
            family.replace(['"', '\\', '<', '>', '&'], ""),
            if bold { "bold" } else { "normal" },
            if italic { "italic" } else { "normal" },
            base64_encode(&data),
            if kind == "otf" { "opentype" } else { "truetype" }
        ));
    }
    if css.is_empty() {
        return out;
    }
    // hinter dem öffnenden <svg>-Element einfügen
    match out
        .find("<svg")
        .and_then(|i| out[i..].find(">\n").map(|j| i + j + 2))
    {
        Some(at) => format!(
            "{}  <defs><style type=\"text/css\"><![CDATA[\n{css}]]></style></defs>\n{}",
            &out[..at],
            &out[at..]
        ),
        None => out,
    }
}

/// Eine gesetzte Zeile eines Textfeldes (für den Export in Programme ohne Textfelder).
/// Koordinaten im System des Textfeldes (vor seiner Matrix).
#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    /// Bereich im Text (Bytes)
    pub start: usize,
    pub end: usize,
    /// Aufzählungszeichen vor der Zeile (sonst leer)
    pub marker: String,
    /// Anfang der Zeile (Aufzählungszeichen) und des Textes
    pub x: f64,
    pub text_x: f64,
    pub baseline: f64,
    /// Breite des Textes (für Blocksatz)
    pub width: f64,
    /// Blocksatz: auf die Breite strecken
    pub justify: bool,
}

thread_local! {
    /// Beim Export: Zeilen eines Textfeldes (gesetzt mit Pango, siehe `render.rs`)
    static LINES_FN: std::cell::Cell<Option<fn(&Shape) -> Option<Vec<TextLine>>>> = const { std::cell::Cell::new(None) };
}

/// SVG für andere Programme: Textfelder werden als einzelne, fest gesetzte Zeilen
/// geschrieben (Umbruch, Spalten und Text in Formen bleiben so erhalten).
pub fn write_export(
    doc: &Document,
    include_background: bool,
    font: Option<&dyn Fn(&str, bool, bool) -> Option<(Vec<u8>, &'static str)>>,
    lines: fn(&Shape) -> Option<Vec<TextLine>>,
) -> String {
    LINES_FN.with(|c| c.set(Some(lines)));
    let out = match font {
        Some(f) => write_with_fonts(doc, include_background, f),
        None => write(doc, include_background),
    };
    LINES_FN.with(|c| c.set(None));
    out
}

pub fn write(doc: &Document, include_background: bool) -> String {
    let mut o = String::new();
    o.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    o.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" \
xmlns:inkscape=\"http://www.inkscape.org/namespaces/inkscape\" \
xmlns:sodipodi=\"http://sodipodi.sourceforge.net/DTD/sodipodi-0.dtd\" \
xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
xmlns:mondrian=\"https://mondrian.invalid/ns\" \
width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\"{u}>\n",
        w = num(doc.width),
        h = num(doc.height),
        u = if doc.unit.is_empty() {
            String::new()
        } else {
            format!(" mondrian:unit=\"{}\"", xml::escape(&doc.unit))
        }
    ));
    if include_background {
        if let Some(bg) = doc.background {
            o.push_str(&format!(
                "  <rect id=\"{BG_ID}\" x=\"0\" y=\"0\" width=\"{}\" height=\"{}\"{} sodipodi:insensitive=\"true\"/>\n",
                num(doc.width),
                num(doc.height),
                paint_attrs("fill", Some(bg))
            ));
        }
    }
    for layer in &doc.layers {
        o.push_str(&format!(
            "  <g id=\"layer{}\" inkscape:groupmode=\"layer\" mondrian:color=\"{}\" inkscape:label=\"{}\"{}{}>\n",
            layer.id,
            layer.color,
            escape(&layer.name),
            if layer.visible {
                ""
            } else {
                " style=\"display:none\""
            },
            if layer.locked {
                " sodipodi:insensitive=\"true\""
            } else {
                ""
            }
        ));
        // Gruppen: aufeinanderfolgende Objekte derselben Gruppe in ein <g>
        let mut open = 0u64;
        for obj in &layer.objects {
            if obj.group != open {
                if open != 0 {
                    o.push_str("    </g>\n");
                }
                if obj.group != 0 {
                    o.push_str(&format!("    <g mondrian:group=\"{}\">\n", obj.group));
                }
                open = obj.group;
            }
            o.push_str(if open != 0 { "      " } else { "    " });
            o.push_str(&object_to_svg(obj));
            o.push('\n');
        }
        if open != 0 {
            o.push_str("    </g>\n");
        }
        o.push_str("  </g>\n");
    }
    for g in &doc.guides {
        let layer = doc
            .layer_index(g.layer)
            .map(|i| format!(" layer=\"{i}\""))
            .unwrap_or_default();
        o.push_str(&format!(
            "  <mondrian:guide x=\"{}\" y=\"{}\" angle=\"{}\"{layer}/>\n",
            num(g.pos.x),
            num(g.pos.y),
            num(g.angle)
        ));
    }
    for a in &doc.artboards {
        o.push_str(&format!(
            "  <mondrian:artboard x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" name=\"{}\"/>\n",
            num(a.x),
            num(a.y),
            num(a.w),
            num(a.h),
            xml::escape(&a.name)
        ));
    }
    o.push_str("</svg>\n");
    o
}

fn paint_attrs(name: &str, c: Option<Color>) -> String {
    match c {
        None => format!(" {name}=\"none\""),
        Some(c) if c.a < 0.999 => {
            format!(" {name}=\"{}\" {name}-opacity=\"{}\"", c.to_hex(), num(c.a))
        }
        Some(c) => format!(" {name}=\"{}\"", c.to_hex()),
    }
}

fn style_attrs(s: &Style) -> String {
    let mut a = paint_attrs("fill", s.fill);
    a.push_str(&paint_attrs("stroke", s.stroke));
    if s.stroke.is_some() {
        a.push_str(&format!(" stroke-width=\"{}\"", num(s.width)));
        if s.join != LineJoin::Miter {
            a.push_str(&format!(" stroke-linejoin=\"{}\"", s.join.css()));
        }
        // Punkte brauchen runde Enden, auch in anderen Programmen
        let cap = if s.dash.round_caps() {
            LineCap::Round
        } else {
            s.cap
        };
        if cap != LineCap::Butt {
            a.push_str(&format!(" stroke-linecap=\"{}\"", cap.css()));
        }
        let pat = s.dash.pattern(s.width);
        if !pat.is_empty() {
            let v: Vec<String> = pat.iter().map(|x| num(*x)).collect();
            a.push_str(&format!(" stroke-dasharray=\"{}\"", v.join(" ")));
        }
    }
    if s.opacity < 0.999 {
        a.push_str(&format!(" opacity=\"{}\"", num(s.opacity.clamp(0.0, 1.0))));
    }
    a
}

/// Zahl mit höherer Genauigkeit (für Matrizen).
fn num6(v: f64) -> String {
    let mut s = format!("{:.6}", v);
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    if s == "-0" {
        s = "0".into();
    }
    s
}

pub fn path_data(subpaths: &[SubPath]) -> String {
    let mut d = String::new();
    for sp in subpaths {
        let Some(first) = sp.nodes.first() else { continue };
        if !d.is_empty() {
            d.push(' ');
        }
        d.push_str(&format!("M {} {}", num(first.p.x), num(first.p.y)));
        let segs = sp.segments();
        let count = segs.len();
        for (i, (p0, c1, c2, p1)) in segs.into_iter().enumerate() {
            let straight = c1.dist(p0) < 1e-9 && c2.dist(p1) < 1e-9;
            let closing = sp.closed && i == count - 1;
            if closing && straight {
                break; // Z zeichnet die gerade Schlusslinie
            }
            if straight {
                d.push_str(&format!(" L {} {}", num(p1.x), num(p1.y)));
            } else {
                d.push_str(&format!(
                    " C {} {} {} {} {} {}",
                    num(c1.x),
                    num(c1.y),
                    num(c2.x),
                    num(c2.y),
                    num(p1.x),
                    num(p1.y)
                ));
            }
        }
        if sp.closed {
            d.push_str(" Z");
        }
    }
    d
}

/// Eigene Attribute für das Absatzformat (nur Abweichungen vom Standard).
fn format_attrs(f: &TextFormat) -> String {
    let d = TextFormat::default();
    let mut s = String::new();
    if f.align != d.align {
        s.push_str(&format!(" mondrian:align=\"{}\"", f.align.name()));
    }
    if f.valign != d.valign {
        s.push_str(&format!(" mondrian:valign=\"{}\"", f.valign.name()));
    }
    if (f.line - d.line).abs() > 1e-9 {
        s.push_str(&format!(" mondrian:line=\"{}\"", num(f.line)));
    }
    if f.tracking.abs() > 1e-9 {
        s.push_str(&format!(" mondrian:tracking=\"{}\"", num(f.tracking)));
    }
    if f.underline || f.strike {
        let mut deco = Vec::new();
        if f.underline {
            deco.push("underline");
        }
        if f.strike {
            deco.push("line-through");
        }
        s.push_str(&format!(" text-decoration=\"{}\"", deco.join(" ")));
    }
    if f.list != crate::model::ListStyle::None {
        s.push_str(&format!(" mondrian:list=\"{}\"", f.list.name()));
    }
    if f.path_below {
        s.push_str(" mondrian:path-below=\"true\"");
    }
    if f.path_reverse {
        s.push_str(" mondrian:path-reverse=\"true\"");
    }
    if (f.para - d.para).abs() > 1e-9 {
        s.push_str(&format!(" mondrian:para=\"{}\"", num(f.para)));
    }
    if f.cols() > 1 {
        s.push_str(&format!(" mondrian:columns=\"{}\"", f.cols()));
    }
    if f.small_caps {
        s.push_str(" font-variant=\"small-caps\"");
    }
    if f.no_ligatures {
        s.push_str(" mondrian:no-liga=\"true\"");
    }
    // Kasten hinter Punkttext und Text auf Pfad
    let col = |c: &Color| format!("{}:{}", c.to_hex(), num(c.a));
    if let Some(c) = &f.box_fill {
        s.push_str(&format!(" mondrian:box-fill=\"{}\"", col(c)));
    }
    if let Some(c) = &f.box_stroke {
        s.push_str(&format!(
            " mondrian:box-stroke=\"{}\" mondrian:box-width=\"{}\"",
            col(c),
            num(f.box_width)
        ));
    }
    if f.box_inset > 0.0 {
        s.push_str(&format!(" mondrian:box-inset=\"{}\"", num(f.box_inset)));
    }
    s
}

/// `font-weight` eines Textes: Zwischenstufen als Zahl, sonst „bold“ bzw. nichts.
fn weight_attr(bold: bool, fmt: &TextFormat) -> String {
    match fmt.weight {
        0 if bold => " font-weight=\"bold\"".into(),
        0 => String::new(),
        w => format!(" font-weight=\"{w}\""),
    }
}

/// Inhalt eines Textfeldes: abweichend formatierte Teile als `<tspan>`.
fn rich_content(text: &str, rich: &RichText, fmt: &TextFormat) -> String {
    rich_range(text, rich, fmt, 0, text.len())
}

/// Wie `rich_content`, aber nur für den Bereich `from..to` des Textes.
fn rich_range(text: &str, rich: &RichText, fmt: &TextFormat, from: usize, to: usize) -> String {
    let to = to.min(text.len());
    if from >= to || !text.is_char_boundary(from) || !text.is_char_boundary(to) {
        return String::new();
    }
    let text_all = text;
    let text = &text_all[..to];
    let mut out = String::new();
    let mut pos = from;
    for r in &rich.runs {
        let (a, b) = (r.start.max(from).min(text.len()), r.end.min(text.len()));
        if a < pos || b <= a || !text.is_char_boundary(a) || !text.is_char_boundary(b) {
            continue;
        }
        out.push_str(&escape(&text[pos..a]));
        let st = &r.style;
        let mut at = String::new();
        if let Some(f) = &st.family {
            at.push_str(&format!(" font-family=\"{}\"", escape(f)));
        }
        if let Some(z) = st.size {
            at.push_str(&format!(" font-size=\"{}\"", num(z)));
        }
        match (st.weight, st.bold) {
            (Some(w), _) => at.push_str(&format!(" font-weight=\"{w}\"")),
            (None, Some(v)) => at.push_str(if v {
                " font-weight=\"bold\""
            } else {
                " font-weight=\"normal\""
            }),
            _ => {}
        }
        if let Some(v) = st.italic {
            at.push_str(if v {
                " font-style=\"italic\""
            } else {
                " font-style=\"normal\""
            });
        }
        if st.underline.is_some() || st.strike.is_some() {
            let mut deco = Vec::new();
            if st.underline.unwrap_or(fmt.underline) {
                deco.push("underline");
            }
            if st.strike.unwrap_or(fmt.strike) {
                deco.push("line-through");
            }
            let d = if deco.is_empty() {
                "none".to_string()
            } else {
                deco.join(" ")
            };
            at.push_str(&format!(" text-decoration=\"{d}\""));
        }
        out.push_str(&format!("<tspan{at}>{}</tspan>", escape(&text[a..b])));
        pos = b;
    }
    out.push_str(&escape(&text[pos..]));
    out
}

/// Absatzformate als Attribut („Aufzählung:Einzug;…“).
fn paras_attr(rich: &RichText) -> String {
    if rich.paras.is_empty() {
        return String::new();
    }
    let v: Vec<String> = rich
        .paras
        .iter()
        .map(|p| format!("{}:{}", p.list.name(), p.indent))
        .collect();
    format!(" mondrian:paras=\"{}\"", v.join(";"))
}

/// Text und Formatierung eines Textfeldes lesen (Teile in `<tspan>`).
fn read_rich(el: &Element) -> (String, RichText) {
    let mut text = String::new();
    let mut rich = RichText::default();
    for c in &el.children {
        match c {
            XNode::Text(t) => text.push_str(t),
            XNode::Elem(e) => {
                let start = text.len();
                text.push_str(&e.text());
                if e.local() != "tspan" {
                    continue;
                }
                let deco = e.attr("text-decoration");
                let style = RunStyle {
                    family: e
                        .attr("font-family")
                        .map(|f| f.trim_matches(|c| c == '\'' || c == '"').to_string()),
                    size: e
                        .attr("font-size")
                        .and_then(|v| v.trim_end_matches("px").parse().ok()),
                    bold: e
                        .attr("font-weight")
                        .map(|w| w == "bold" || w.parse::<u32>().map(|n| n >= 600).unwrap_or(false)),
                    // Zwischenstufen (z. B. 500, 800) als eigene Strichstärke
                    weight: e
                        .attr("font-weight")
                        .and_then(|w| w.parse::<u16>().ok())
                        .filter(|n| *n != 400 && *n != 700),
                    italic: e.attr("font-style").map(|v| v == "italic" || v == "oblique"),
                    underline: deco.map(|d| d.contains("underline")),
                    strike: deco.map(|d| d.contains("line-through")),
                };
                if !style.is_empty() && text.len() > start {
                    rich.runs.push(TextRun {
                        start,
                        end: text.len(),
                        style,
                    });
                }
            }
        }
    }
    if let Some(v) = own(el, "paras") {
        rich.paras = v
            .split(';')
            .map(|p| {
                let mut it = p.split(':');
                ParaFmt {
                    list: it.next().and_then(ListStyle::from_name).unwrap_or_default(),
                    indent: it.next().and_then(|n| n.trim().parse().ok()).unwrap_or(0),
                }
            })
            .collect();
    }
    (text, rich)
}

/// `text-anchor` für Punkttext und Text auf Pfad.
fn anchor_attr(a: Align) -> &'static str {
    match a {
        Align::Center => " text-anchor=\"middle\"",
        Align::Right => " text-anchor=\"end\"",
        _ => "",
    }
}

fn letter_spacing_attr(f: &TextFormat, size: f64) -> String {
    if f.tracking.abs() > 1e-9 {
        format!(" letter-spacing=\"{}\"", num(f.tracking_px(size)))
    } else {
        String::new()
    }
}

/// Verlauf als SVG-Definition (objectBoundingBox wie im Modell). Kreisförmig als
/// `radialGradient`, Muster als `pattern` (ausgerichtet am Objekt `bbox`).
fn gradient_def(id: &str, g: &Gradient, bbox: Option<crate::geom::Rect>) -> String {
    use crate::model::GradShape;
    if g.shape.is_pattern() {
        let (c1, c2) = (
            g.stops.first().map(|s| s.color).unwrap_or(Color::WHITE),
            g.stops.get(1).map(|s| s.color).unwrap_or(Color::BLACK),
        );
        let fillc = |c: Color| {
            if c.a < 0.999 {
                format!("fill=\"{}\" fill-opacity=\"{}\"", c.to_hex(), num(c.a))
            } else {
                format!("fill=\"{}\"", c.to_hex())
            }
        };
        let k = g.cell.max(0.5);
        let (a, deg) = match bbox {
            Some(r) => {
                let (a, b) = g.line(&r);
                (a, (b.y - a.y).atan2(b.x - a.x).to_degrees())
            }
            None => (Pt::new(0.0, 0.0), 0.0),
        };
        let (tile, motif) = match g.shape {
            GradShape::Checker => (
                2.0 * k,
                format!(
                    "<rect width=\"{k}\" height=\"{k}\" {f}/><rect x=\"{k}\" y=\"{k}\" width=\"{k}\" height=\"{k}\" {f}/>",
                    k = num(k),
                    f = fillc(c2)
                ),
            ),
            GradShape::Dots => (
                2.0 * k,
                format!(
                    "<circle cx=\"{a}\" cy=\"{a}\" r=\"{r}\" {f}/><circle cx=\"{b}\" cy=\"{b}\" r=\"{r}\" {f}/>",
                    a = num(k * 0.5),
                    b = num(k * 1.5),
                    r = num(k * 0.3),
                    f = fillc(c2)
                ),
            ),
            _ => (
                k,
                format!(
                    "<rect width=\"{}\" height=\"{}\" {}/>",
                    num(k * 0.5),
                    num(k),
                    fillc(c2)
                ),
            ),
        };
        return format!(
            "<defs><pattern id=\"{id}\" patternUnits=\"userSpaceOnUse\" width=\"{t}\" height=\"{t}\" patternTransform=\"translate({} {}) rotate({})\"><rect width=\"{t}\" height=\"{t}\" {}/>{motif}</pattern></defs>",
            num(a.x),
            num(a.y),
            num(deg),
            fillc(c1),
            t = num(tile)
        );
    }
    let own: Vec<String> = g
        .stops
        .iter()
        .map(|s| format!("{}:{}:{}", num(s.pos), s.color.to_hex(), num(s.color.a)))
        .collect();
    let radial = g.shape == GradShape::Radial;
    let mut s = if radial {
        format!(
            "<defs><radialGradient id=\"{id}\" gradientUnits=\"objectBoundingBox\" cx=\"{}\" cy=\"{}\" r=\"{}\" mondrian:kind=\"{}\" mondrian:stops=\"{}\">",
            num6(g.p0.x),
            num6(g.p0.y),
            num6(g.p0.dist(g.p1)),
            g.kind.name(),
            own.join(" ")
        )
    } else {
        format!(
            "<defs><linearGradient id=\"{id}\" gradientUnits=\"objectBoundingBox\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" mondrian:kind=\"{}\" mondrian:stops=\"{}\">",
            num6(g.p0.x),
            num6(g.p0.y),
            num6(g.p1.x),
            num6(g.p1.y),
            g.kind.name(),
            own.join(" ")
        )
    };
    for (pos, c) in g.render_stops() {
        let op = if c.a < 0.999 {
            format!(" stop-opacity=\"{}\"", num(c.a))
        } else {
            String::new()
        };
        s.push_str(&format!(
            "<stop offset=\"{}\" stop-color=\"{}\"{op}/>",
            num6(pos.clamp(0.0, 1.0)),
            c.to_hex()
        ));
    }
    s.push_str(if radial {
        "</radialGradient></defs>"
    } else {
        "</linearGradient></defs>"
    });
    s
}

/// Eckenradien als „Teilpfad:Knoten:Radius …“.
fn radii_list(subpaths: &[SubPath]) -> String {
    let v: Vec<String> = subpaths
        .iter()
        .enumerate()
        .flat_map(|(si, sp)| {
            sp.nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| n.r > 0.0)
                .map(move |(ni, n)| format!("{si}:{ni}:{}", num(n.r)))
        })
        .collect();
    v.join(" ")
}

fn apply_radii(subpaths: &mut [SubPath], list: &str) {
    for item in list.split_whitespace() {
        let v: Vec<&str> = item.split(':').collect();
        if let [si, ni, r] = v[..] {
            if let (Ok(si), Ok(ni), Ok(r)) = (si.parse::<usize>(), ni.parse::<usize>(), r.parse::<f64>()) {
                if let Some(n) = subpaths.get_mut(si).and_then(|s| s.nodes.get_mut(ni)) {
                    n.r = r;
                }
            }
        }
    }
}

pub fn object_to_svg(obj: &Object) -> String {
    let angle = if obj.angle.abs() > 1e-9 {
        format!(" mondrian:angle=\"{}\"", num(obj.angle))
    } else {
        String::new()
    };
    let locked = if obj.locked {
        " mondrian:locked=\"true\""
    } else {
        ""
    };
    let label = if obj.name.is_empty() {
        String::new()
    } else {
        format!(" inkscape:label=\"{}\"", escape(&obj.name))
    };
    let id = format!("id=\"obj{}\"{angle}{locked}{label}", obj.id);
    // Farbverlauf: Definition vor dem Element, Füllung verweist darauf.
    let grad = obj.style.gradient.as_ref().filter(|_| obj.style.fill.is_some());
    let gid = format!("grad{}", obj.id);
    let defs = grad
        .map(|g| gradient_def(&gid, g, obj.shape.geom_bbox()))
        .unwrap_or_default();
    let mut base = style_attrs(&obj.style);
    if grad.is_some() {
        // fill="#…" durch den Verlauf ersetzen
        let fill = paint_attrs("fill", obj.style.fill);
        base = base.replacen(&fill, &format!(" fill=\"url(#{gid})\""), 1);
    }
    let blend_css = if obj.style.blend != Blend::Normal {
        format!("mix-blend-mode:{}", obj.style.blend.css())
    } else {
        String::new()
    };
    // Linienenden als SVG-Marker (plus eigene Angaben zum Wiederherstellen)
    let mut marker_defs = String::new();
    let mut marker_attr = String::new();
    if obj.style.marker != Marker::None && obj.shape.is_open() {
        let mk = obj.style.marker;
        let ends = obj.style.marker_ends;
        let col = obj.style.stroke.unwrap_or(Color::BLACK).to_hex();
        let mid = format!("mk{}", obj.id);
        let (body, refx, size) = match mk {
            Marker::Arrow => (format!("<path d=\"M0 0L10 5L0 10z\" fill=\"{col}\"/>"), 8.0, 4.0),
            Marker::OpenArrow => (
                format!("<path d=\"M1 1L9 5L1 9\" fill=\"none\" stroke=\"{col}\" stroke-width=\"1.5\"/>"),
                9.0,
                4.0,
            ),
            Marker::Dot => (
                format!("<circle cx=\"5\" cy=\"5\" r=\"4\" fill=\"{col}\"/>"),
                5.0,
                3.2,
            ),
            Marker::Bar => (
                format!("<rect x=\"4\" y=\"0\" width=\"2\" height=\"10\" fill=\"{col}\"/>"),
                5.0,
                4.0,
            ),
            Marker::None => (String::new(), 0.0, 0.0),
        };
        marker_defs = format!(
            "<defs><marker id=\"{mid}\" viewBox=\"0 0 10 10\" refX=\"{}\" refY=\"5\" markerWidth=\"{}\" markerHeight=\"{}\" orient=\"auto-start-reverse\">{body}</marker></defs>",
            num(refx),
            num(size * obj.style.marker_size),
            num(size * obj.style.marker_size)
        );
        if ends.start() {
            marker_attr.push_str(&format!(" marker-start=\"url(#{mid})\""));
        }
        if ends.end() {
            marker_attr.push_str(&format!(" marker-end=\"url(#{mid})\""));
        }
        marker_attr.push_str(&format!(
            " mondrian:marker=\"{}\" mondrian:marker-ends=\"{}\" mondrian:marker-size=\"{}\"",
            mk.name(),
            ends.name(),
            num(obj.style.marker_size)
        ));
    }
    if obj.style.fx != crate::model::Effects::default() {
        marker_attr.push_str(&format!(" mondrian:fx=\"{}\"", obj.style.fx.to_attr()));
    }
    // Muster und kreisförmige Verläufe: eigene Angabe (vollständig)
    if let Some(g) = obj
        .style
        .gradient
        .as_ref()
        .filter(|g| g.shape != crate::model::GradShape::Linear && obj.style.fill.is_some())
    {
        marker_attr.push_str(&format!(" mondrian:fill-gradient=\"{}\"", g.to_attr()));
    }
    if obj.style.dash != crate::model::Dash::Solid && obj.style.stroke.is_some() {
        marker_attr.push_str(&format!(" mondrian:dash=\"{}\"", obj.style.dash.name()));
    }
    if obj.style.stroke_align != crate::model::StrokeAlign::Center {
        marker_attr.push_str(&format!(
            " mondrian:stroke-align=\"{}\"",
            obj.style.stroke_align.name()
        ));
    }
    // Verläufe von Kontur und Schatten (eigene Angabe; andere Programme sehen die Grundfarbe)
    if let Some(g) = obj.style.grad(crate::model::PaintTarget::Stroke) {
        marker_attr.push_str(&format!(" mondrian:stroke-gradient=\"{}\"", g.to_attr()));
    }
    if let Some(g) = obj.style.grad(crate::model::PaintTarget::Shadow) {
        marker_attr.push_str(&format!(" mondrian:shadow-gradient=\"{}\"", g.to_attr()));
    }
    // Schlagschatten: eigene Angabe plus SVG-Filter für andere Programme
    if let Some(sh) = obj.style.shadow {
        let fid = format!("shadow{}", obj.id);
        marker_attr.push_str(&format!(
            " mondrian:shadow=\"{}\" filter=\"url(#{fid})\"",
            sh.to_attr()
        ));
        marker_defs.push_str(&format!(
            "<defs><filter id=\"{fid}\" x=\"-50%\" y=\"-50%\" width=\"200%\" height=\"200%\" color-interpolation-filters=\"sRGB\">\
<feGaussianBlur in=\"SourceAlpha\" stdDeviation=\"{}\"/><feOffset dx=\"{}\" dy=\"{}\" result=\"o\"/>\
<feFlood flood-color=\"{}\" flood-opacity=\"{}\"/><feComposite in2=\"o\" operator=\"in\"/>\
<feMerge><feMergeNode/><feMergeNode in=\"SourceGraphic\"/></feMerge></filter></defs>",
            num(sh.blur / 2.0),
            num(sh.dx),
            num(sh.dy),
            sh.color.to_hex(),
            num(sh.opacity)
        ));
    }
    let base = format!("{base}{marker_attr}");
    let st = if blend_css.is_empty() {
        base.clone()
    } else {
        format!("{base} style=\"{blend_css}\"")
    };
    let body = match &obj.shape {
        Shape::Rect { x, y, w, h, r } => {
            let rx = if *r > 0.0 {
                format!(" rx=\"{}\"", num(*r))
            } else {
                String::new()
            };
            format!(
                "<rect {id} x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{rx}{st}/>",
                num(*x),
                num(*y),
                num(*w),
                num(*h)
            )
        }
        Shape::Ellipse { cx, cy, rx, ry } => format!(
            "<ellipse {id} cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\"{st}/>",
            num(*cx),
            num(*cy),
            num(*rx),
            num(*ry)
        ),
        Shape::Line { a, b } => format!(
            "<line {id} x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"{st}/>",
            num(a.x),
            num(a.y),
            num(b.x),
            num(b.y)
        ),
        Shape::Path { subpaths, evenodd } => {
            // Lebendige Ecken: gerundete Form in d, Grundform und Radien zusätzlich für Stijl
            let live = if subpaths.iter().any(|s| s.has_radii()) {
                let radii: Vec<String> = subpaths
                    .iter()
                    .enumerate()
                    .flat_map(|(si, sp)| {
                        sp.nodes
                            .iter()
                            .enumerate()
                            .filter(|(_, n)| n.r > 0.0)
                            .map(move |(ni, n)| format!("{si}:{ni}:{}", num(n.r)))
                    })
                    .collect();
                format!(
                    " mondrian:d=\"{}\" mondrian:radii=\"{}\"",
                    path_data(subpaths),
                    radii.join(" ")
                )
            } else {
                String::new()
            };
            format!(
                "<path {id} d=\"{}\"{live}{}{st}/>",
                path_data(&crate::model::effective_subpaths(subpaths)),
                if *evenodd { " fill-rule=\"evenodd\"" } else { "" }
            )
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
            // Reine Verschiebung als x/y schreiben, sonst als Transformationsmatrix.
            let plain = (xf.a - 1.0).abs() < 1e-9
                && xf.b.abs() < 1e-9
                && xf.c.abs() < 1e-9
                && (xf.d - 1.0).abs() < 1e-9;
            let (ox, oy, tf) = if plain {
                (xf.e, xf.f, String::new())
            } else {
                (
                    0.0,
                    0.0,
                    format!(
                        " transform=\"matrix({} {} {} {} {} {})\"",
                        num6(xf.a),
                        num6(xf.b),
                        num6(xf.c),
                        num6(xf.d),
                        num6(xf.e),
                        num6(xf.f)
                    ),
                )
            };
            let pos = Pt::new(ox, oy);
            let mut s = format!(
                "<text {id} xml:space=\"preserve\"{tf} x=\"{}\" y=\"{}\" font-family=\"{}\" font-size=\"{}\"{}{}{}{}{}{st}>",
                num(pos.x),
                num(pos.y),
                escape(family),
                num(*size),
                weight_attr(*bold, fmt),
                if *italic { " font-style=\"italic\"" } else { "" },
                anchor_attr(fmt.align),
                letter_spacing_attr(fmt, *size),
                format_attrs(fmt),
            );
            for (i, line) in text.split('\n').enumerate() {
                s.push_str(&format!(
                    "<tspan x=\"{}\" y=\"{}\">{}</tspan>",
                    num(pos.x),
                    num(pos.y + i as f64 * size * fmt.line),
                    escape(line)
                ));
            }
            s.push_str("</text>");
            s
        }
        Shape::TextPath {
            path,
            text,
            family,
            size,
            bold,
            italic,
            offset,
            fmt,
        } => {
            let pid = format!("tp{}", obj.id);
            // startOffset ist in SVG der Ankerpunkt (je nach text-anchor)
            let (_, len) = crate::model::path_polyline(path);
            let total = len.last().copied().unwrap_or(0.0);
            let start = crate::model::textpath_anchor(total, *offset, fmt.align);
            format!(
                "<defs><path id=\"{pid}\" d=\"{}\"/></defs><text {id} xml:space=\"preserve\" font-family=\"{}\" font-size=\"{}\"{}{}{}{}{}{st}><textPath href=\"#{pid}\" xlink:href=\"#{pid}\" startOffset=\"{}\">{}</textPath></text>",
                path_data(path),
                escape(family),
                num(*size),
                weight_attr(*bold, fmt),
                if *italic { " font-style=\"italic\"" } else { "" },
                anchor_attr(fmt.align),
                letter_spacing_attr(fmt, *size),
                format_attrs(fmt),
                num(start),
                escape(&text.replace('\n', " "))
            )
        }
        Shape::TextArea {
            xf,
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
        } => {
            // Rahmen des Textfeldes: eigene Angaben + sichtbarer Pfad für andere Programme
            let col = |c: &Color| format!("{}:{}", c.to_hex(), num(c.a));
            let mut fattr = String::new();
            if let Some(p) = &frame.path {
                fattr.push_str(&format!(" mondrian:frame-d=\"{}\"", path_data(p)));
                let r = radii_list(p);
                if !r.is_empty() {
                    fattr.push_str(&format!(" mondrian:frame-radii=\"{r}\""));
                }
            }
            if let Some(c) = &frame.fill {
                fattr.push_str(&format!(" mondrian:frame-fill=\"{}\"", col(c)));
            }
            if let Some(c) = &frame.stroke {
                fattr.push_str(&format!(
                    " mondrian:frame-stroke=\"{}\" mondrian:frame-width=\"{}\"",
                    col(c),
                    num(frame.width)
                ));
            }
            if frame.inset > 0.0 {
                fattr.push_str(&format!(" mondrian:inset=\"{}\"", num(frame.inset)));
            }
            let frame_el = if frame.fill.is_some() || frame.stroke.is_some() {
                let local = match &frame.path {
                    Some(p) => crate::model::effective_subpaths(p),
                    None => vec![SubPath {
                        nodes: vec![
                            Node::corner(Pt::new(0.0, 0.0)),
                            Node::corner(Pt::new(*w, 0.0)),
                            Node::corner(Pt::new(*w, *h)),
                            Node::corner(Pt::new(0.0, *h)),
                        ],
                        closed: true,
                    }],
                };
                let mut sh = Shape::Path {
                    subpaths: local,
                    evenodd: false,
                };
                sh.transform(xf);
                let d = match &sh {
                    Shape::Path { subpaths, .. } => path_data(subpaths),
                    _ => String::new(),
                };
                let mut a = paint_attrs("fill", frame.fill);
                a.push_str(&paint_attrs("stroke", frame.stroke));
                if frame.stroke.is_some() {
                    a.push_str(&format!(" stroke-width=\"{}\"", num(frame.width)));
                }
                format!("<path d=\"{d}\"{a} mondrian:role=\"frame\"/>")
            } else {
                String::new()
            };
            // Export für andere Programme: fest gesetzte Zeilen
            if let Some(lines) = LINES_FN.with(|c| c.get()).and_then(|f| f(&obj.shape)) {
                let mut css = String::new();
                if fmt.tracking.abs() > 1e-9 {
                    css.push_str(&format!("letter-spacing:{}px;", num(fmt.tracking_px(*size))));
                }
                if fmt.no_ligatures {
                    css.push_str("font-variant-ligatures:none;");
                }
                if fmt.small_caps {
                    css.push_str("font-variant-caps:small-caps;");
                }
                css.push_str(&blend_css);
                let mut body = String::new();
                for l in &lines {
                    body.push_str(&format!(
                        "<text xml:space=\"preserve\" x=\"{}\" y=\"{}\">",
                        num(l.x),
                        num(l.baseline)
                    ));
                    if !l.marker.is_empty() {
                        body.push_str(&escape(&l.marker));
                    }
                    let fit = if l.justify && l.width > 0.0 {
                        format!(" textLength=\"{}\" lengthAdjust=\"spacing\"", num(l.width))
                    } else {
                        String::new()
                    };
                    body.push_str(&format!(
                        "<tspan x=\"{}\"{fit}>{}</tspan></text>",
                        num(l.text_x),
                        rich_range(text, rich, fmt, l.start, l.end)
                    ));
                }
                let deco = if fmt.underline || fmt.strike {
                    let mut d = Vec::new();
                    if fmt.underline {
                        d.push("underline");
                    }
                    if fmt.strike {
                        d.push("line-through");
                    }
                    format!(" text-decoration=\"{}\"", d.join(" "))
                } else {
                    String::new()
                };
                return format!(
                    "{defs}{marker_defs}{frame_el}<g {id} transform=\"matrix({} {} {} {} {} {})\" font-family=\"{}\" font-size=\"{}\"{}{}{deco}{base}{}>{body}</g>",
                    num6(xf.a),
                    num6(xf.b),
                    num6(xf.c),
                    num6(xf.d),
                    num6(xf.e),
                    num6(xf.f),
                    escape(family),
                    num(*size),
                    weight_attr(*bold, fmt),
                    if *italic { " font-style=\"italic\"" } else { "" },
                    if css.is_empty() {
                        String::new()
                    } else {
                        format!(" style=\"{css}\"")
                    }
                );
            }
            let mut css = format!(
                "inline-size:{}px;white-space:pre-wrap;text-align:{};line-height:{}",
                num(*w),
                fmt.align.name(),
                num(fmt.line)
            );
            if fmt.tracking.abs() > 1e-9 {
                css.push_str(&format!(";letter-spacing:{}px", num(fmt.tracking_px(*size))));
            }
            if fmt.cols() > 1 {
                css.push_str(&format!(";column-count:{}", fmt.cols()));
            }
            if fmt.no_ligatures {
                css.push_str(";font-variant-ligatures:none");
            }
            if !blend_css.is_empty() {
                css.push(';');
                css.push_str(&blend_css);
            }
            format!(
                "{frame_el}<text {id} xml:space=\"preserve\" transform=\"matrix({} {} {} {} {} {})\" x=\"0\" y=\"{}\" mondrian:area=\"{} {}\" style=\"{css}\" font-family=\"{}\" font-size=\"{}\"{}{}{}{fattr}{base}>{}</text>",
                num6(xf.a),
                num6(xf.b),
                num6(xf.c),
                num6(xf.d),
                num6(xf.e),
                num6(xf.f),
                num(size * AREA_ASCENT),
                num(*w),
                num(*h),
                escape(family),
                num(*size),
                weight_attr(*bold, fmt),
                if *italic { " font-style=\"italic\"" } else { "" },
                format!("{}{}", format_attrs(fmt), paras_attr(rich)),
                rich_content(text, rich, fmt)
            )
        }
        Shape::Image { xf, w, h, data, clip } => {
            let uri = format!("data:{};base64,{}", sniff_mime(data), base64_encode(data));
            // Rahmen: clipPath im Bildkoordinatensystem (für andere Programme) und
            // eigene Kopie in Dokumentkoordinaten; die Rahmenlinie als eigener Pfad.
            let (cdef, cattr, frame) = match clip {
                Some(c) if xf.invert().apply(Pt::new(0.0, 0.0)).x.is_finite() => {
                    let cid = format!("clip{}", obj.id);
                    let eff = crate::model::effective_subpaths(c);
                    let radii = radii_list(c);
                    let radii_attr = if radii.is_empty() {
                        String::new()
                    } else {
                        format!(" mondrian:clip-radii=\"{radii}\"")
                    };
                    let mut local = Shape::Path {
                        subpaths: eff.clone(),
                        evenodd: false,
                    };
                    local.transform(&xf.invert());
                    let local_d = match &local {
                        Shape::Path { subpaths, .. } => path_data(subpaths),
                        _ => String::new(),
                    };
                    let stroke = obj.style.stroke.filter(|_| obj.style.width > 0.0);
                    let sattr = match stroke {
                        Some(sc) => format!(
                            "{} stroke-width=\"{}\"",
                            paint_attrs("stroke", Some(sc)),
                            num(obj.style.width)
                        ),
                        None => String::new(),
                    };
                    let frame = if stroke.is_some() {
                        format!(
                            "<path d=\"{}\" fill=\"none\"{sattr} mondrian:role=\"frame\"/>",
                            path_data(&eff)
                        )
                    } else {
                        String::new()
                    };
                    (
                        format!("<defs><clipPath id=\"{cid}\"><path d=\"{local_d}\"/></clipPath></defs>"),
                        format!(
                            " clip-path=\"url(#{cid})\" mondrian:clip=\"{}\"{radii_attr}{sattr}",
                            path_data(c)
                        ),
                        frame,
                    )
                }
                _ => (String::new(), String::new(), String::new()),
            };
            let op = if obj.style.opacity < 0.999 {
                format!(" opacity=\"{}\"", num(obj.style.opacity))
            } else {
                String::new()
            };
            let bl = if blend_css.is_empty() {
                String::new()
            } else {
                format!(" style=\"{blend_css}\"")
            };
            format!(
                "{cdef}<image {id} x=\"0\" y=\"0\" width=\"{}\" height=\"{}\" preserveAspectRatio=\"none\" transform=\"matrix({} {} {} {} {} {})\"{op}{bl}{cattr} href=\"{uri}\" xlink:href=\"{uri}\"/>{frame}",
                num(*w),
                num(*h),
                num6(xf.a),
                num6(xf.b),
                num6(xf.c),
                num6(xf.d),
                num6(xf.e),
                num6(xf.f)
            )
        }
    };
    format!("{defs}{marker_defs}{body}")
}

// ───────────────────────────── Lesen ─────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
enum Paint {
    None,
    Color(Color),
}

#[derive(Clone, Debug)]
struct Ctx {
    m: Affine,
    fill: Paint,
    stroke: Paint,
    width: f64,
    fill_op: f64,
    stroke_op: f64,
    opacity: f64,
    evenodd: bool,
    family: String,
    size: f64,
    bold: bool,
    /// Strichstärke als Zahl (Zwischenstufen; 0 = aus „bold“)
    weight: u16,
    italic: bool,
    hidden: bool,
    /// Füllung verweist auf einen Verlauf (ID).
    fill_url: Option<String>,
    blend: Blend,
    join: LineJoin,
    cap: LineCap,
    /// stroke-dasharray (leer: durchgezogen)
    dash: Vec<f64>,
    align: Align,
    /// Zeichenabstand in px
    letter_spacing: f64,
    /// Zeilenhöhe als Faktor bzw. in px
    line_factor: Option<f64>,
    line_px: Option<f64>,
}

impl Default for Ctx {
    fn default() -> Self {
        Ctx {
            m: Affine::IDENTITY,
            fill: Paint::Color(Color::BLACK),
            stroke: Paint::None,
            width: 1.0,
            fill_op: 1.0,
            stroke_op: 1.0,
            opacity: 1.0,
            evenodd: false,
            family: "sans-serif".into(),
            size: 16.0,
            bold: false,
            weight: 0,
            italic: false,
            hidden: false,
            fill_url: None,
            blend: Blend::Normal,
            join: LineJoin::Miter,
            cap: LineCap::Butt,
            dash: Vec::new(),
            align: Align::Left,
            letter_spacing: 0.0,
            line_factor: None,
            line_px: None,
        }
    }
}

// ───────────── Farbverläufe lesen ─────────────

#[derive(Clone, Debug, Default)]
struct RawGrad {
    x1: Option<f64>,
    y1: Option<f64>,
    x2: Option<f64>,
    y2: Option<f64>,
    user: Option<bool>,
    stops: Vec<Stop>,
    own: Option<Vec<Stop>>,
    kind: Option<GradientKind>,
    href: Option<String>,
}

thread_local! {
    /// Verläufe des gerade gelesenen Dokuments (ID → Rohdaten).
    static GRADS: std::cell::RefCell<HashMap<String, RawGrad>> = std::cell::RefCell::new(HashMap::new());
    /// CSS-Regeln aus `<style>`-Blöcken des gerade gelesenen Dokuments.
    static CSS: std::cell::RefCell<Vec<CssRule>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Ordner der gelesenen Datei (für verlinkte Bilder mit relativem Pfad).
    static BASE_DIR: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
    /// Hinweise beim Lesen (Inhalte, die nicht übernommen werden konnten).
    static NOTES: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Hinweis beim Lesen merken (jeder nur einmal).
fn note(msg: String) {
    NOTES.with(|n| {
        let mut n = n.borrow_mut();
        if !n.contains(&msg) {
            n.push(msg);
        }
    });
}

// ───────────────────────────── CSS ─────────────────────────────

/// Eine CSS-Regel mit einfachem Selektor: Element, Klassen und/oder ID (z. B. `.cls-1`,
/// `path.st0`, `#logo`, `rect`). Verschachtelte Selektoren werden nicht ausgewertet.
#[derive(Clone, Debug, PartialEq)]
struct CssRule {
    tag: Option<String>,
    classes: Vec<String>,
    id: Option<String>,
    decls: Vec<(String, String)>,
    /// Gewicht: ID 100, Klasse 10, Element 1
    spec: u32,
    order: usize,
}

/// Deklarationen „a: b; c: d“ (ohne `!important`).
fn css_decls(text: &str) -> Vec<(String, String)> {
    text.split(';')
        .filter_map(|d| {
            let (k, v) = d.split_once(':')?;
            let v = v.trim().trim_end_matches("!important").trim();
            let k = k.trim();
            (!k.is_empty() && !v.is_empty()).then(|| (k.to_string(), v.to_string()))
        })
        .collect()
}

/// Einfachen Selektor zerlegen; `None` bei Selektoren, die nicht unterstützt werden.
fn css_selector(sel: &str) -> Option<(Option<String>, Vec<String>, Option<String>)> {
    let sel = sel.trim();
    if sel.is_empty() || sel.contains(|c: char| c.is_whitespace() || "[]>+~:()".contains(c)) {
        return None;
    }
    let mut tag = None;
    let mut classes = Vec::new();
    let mut id = None;
    let mut cur = String::new();
    let mut kind = 't';
    let mut flush = |kind: char, cur: &mut String| {
        if !cur.is_empty() {
            match kind {
                '.' => classes.push(std::mem::take(cur)),
                '#' => id = Some(std::mem::take(cur)),
                _ => {
                    let t = std::mem::take(cur);
                    if t != "*" {
                        tag = Some(t);
                    }
                }
            }
        }
    };
    for ch in sel.chars() {
        if ch == '.' || ch == '#' {
            flush(kind, &mut cur);
            kind = ch;
        } else {
            cur.push(ch);
        }
    }
    flush(kind, &mut cur);
    Some((tag, classes, id))
}

/// Regeln aus einem Stylesheet lesen. Liefert `false`, wenn etwas nicht unterstützt wird.
fn parse_css(text: &str, rules: &mut Vec<CssRule>) -> bool {
    // Kommentare entfernen
    let mut t = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("/*") {
        t.push_str(&rest[..i]);
        rest = match rest[i + 2..].find("*/") {
            Some(j) => &rest[i + 2 + j + 2..],
            None => "",
        };
    }
    t.push_str(rest);
    let mut ok = true;
    let mut rest = t.as_str();
    while let Some(open) = rest.find('{') {
        let sel = rest[..open].trim();
        // Block bis zur passenden Klammer (auch verschachtelt bei @media)
        let mut depth = 0;
        let mut close = rest.len();
        for (i, ch) in rest[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = open + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        let body = &rest[open + 1..close.min(rest.len())];
        rest = if close < rest.len() {
            &rest[close + 1..]
        } else {
            ""
        };
        if sel.starts_with('@') {
            // @font-face und Ähnliches betreffen die Darstellung der Formen nicht
            if !sel.starts_with("@font-face") && !sel.starts_with("@import") {
                ok = false;
            }
            continue;
        }
        let decls = css_decls(body);
        for one in sel.split(',') {
            match css_selector(one) {
                Some((tag, classes, id)) => {
                    let spec = id.as_ref().map_or(0, |_| 100)
                        + classes.len() as u32 * 10
                        + tag.as_ref().map_or(0, |_| 1);
                    let order = rules.len();
                    rules.push(CssRule {
                        tag,
                        classes,
                        id,
                        decls: decls.clone(),
                        spec,
                        order,
                    });
                }
                None => ok = false,
            }
        }
    }
    ok
}

/// Alle `<style>`-Blöcke eines Dokuments.
fn gather_css(root: &Element) -> (Vec<CssRule>, bool) {
    let mut rules = Vec::new();
    let mut ok = true;
    let mut stack = vec![root];
    while let Some(el) = stack.pop() {
        if el.local() == "style" {
            ok &= parse_css(&el.text(), &mut rules);
            continue;
        }
        // in Dokumentreihenfolge abarbeiten
        let kids: Vec<&Element> = el.elements().collect();
        for k in kids.into_iter().rev() {
            stack.push(k);
        }
    }
    (rules, ok)
}

/// CSS-Deklarationen, die auf ein Element zutreffen, in der Reihenfolge ihres Gewichts.
fn css_for(el: &Element) -> Vec<(String, String)> {
    CSS.with(|css| {
        let css = css.borrow();
        if css.is_empty() {
            return Vec::new();
        }
        let classes: Vec<&str> = el
            .attr("class")
            .map(|c| c.split_whitespace().collect())
            .unwrap_or_default();
        let id = el.attr("id");
        let mut hits: Vec<&CssRule> = css
            .iter()
            .filter(|r| {
                r.tag.as_deref().map_or(true, |t| t == el.local())
                    && r.id.as_deref().map_or(true, |i| Some(i) == id)
                    && r.classes.iter().all(|c| classes.contains(&c.as_str()))
            })
            .collect();
        hits.sort_by_key(|r| (r.spec, r.order));
        hits.iter().flat_map(|r| r.decls.iter().cloned()).collect()
    })
}

/// Koordinate eines Verlaufs: Zahl oder Prozentangabe.
fn grad_coord(v: &str) -> Option<f64> {
    let v = v.trim();
    match v.strip_suffix('%') {
        Some(p) => p.trim().parse::<f64>().ok().map(|x| x / 100.0),
        None => v.parse::<f64>().ok().or_else(|| parse_length(v)),
    }
}

/// Wert aus `style`, CSS-Regeln oder Attribut eines Elements (in dieser Rangfolge).
fn prop(el: &Element, name: &str) -> Option<String> {
    if let Some(st) = el.attr("style") {
        for decl in st.split(';') {
            if let Some((k, v)) = decl.split_once(':') {
                if k.trim() == name {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    if let Some((_, v)) = css_for(el).into_iter().rev().find(|(k, _)| k == name) {
        return Some(v);
    }
    el.attr(name).map(str::to_string)
}

fn gather_gradients(el: &Element, map: &mut HashMap<String, RawGrad>) {
    for c in el.elements() {
        if c.local() == "linearGradient" {
            if let Some(id) = c.attr("id") {
                let mut g = RawGrad {
                    x1: c.attr("x1").and_then(grad_coord),
                    y1: c.attr("y1").and_then(grad_coord),
                    x2: c.attr("x2").and_then(grad_coord),
                    y2: c.attr("y2").and_then(grad_coord),
                    user: c.attr("gradientUnits").map(|u| u == "userSpaceOnUse"),
                    kind: own(c, "kind").and_then(GradientKind::from_name),
                    href: c
                        .attr("href")
                        .or_else(|| c.attr_local("href"))
                        .map(|h| h.trim_start_matches('#').to_string()),
                    ..RawGrad::default()
                };
                if let Some(list) = own(c, "stops") {
                    let stops: Vec<Stop> = list
                        .split_whitespace()
                        .filter_map(|item| {
                            let mut it = item.split(':');
                            let pos = it.next()?.parse::<f64>().ok()?;
                            let mut color = Color::parse(it.next()?)?;
                            if let Some(a) = it.next().and_then(|a| a.parse::<f64>().ok()) {
                                color.a = a;
                            }
                            Some(Stop { pos, color })
                        })
                        .collect();
                    if stops.len() >= 2 {
                        g.own = Some(stops);
                    }
                }
                for st in c.elements().filter(|e| e.local() == "stop") {
                    let pos = prop(st, "offset").and_then(|v| grad_coord(&v)).unwrap_or(0.0);
                    let mut color = prop(st, "stop-color")
                        .and_then(|v| Color::parse(&v))
                        .unwrap_or(Color::BLACK);
                    if let Some(op) = prop(st, "stop-opacity").and_then(|v| v.parse::<f64>().ok()) {
                        color.a *= op;
                    }
                    g.stops.push(Stop {
                        pos: pos.clamp(0.0, 1.0),
                        color,
                    });
                }
                map.insert(id.to_string(), g);
            }
        }
        gather_gradients(c, map);
    }
}

/// Verlauf mit Verweisen (href) auflösen.
fn resolve_gradient(id: &str) -> Option<RawGrad> {
    GRADS.with(|m| {
        let m = m.borrow();
        let mut g = m.get(id)?.clone();
        let mut next = g.href.clone();
        let mut depth = 0;
        while let (Some(h), true) = (next.clone(), depth < 8) {
            let Some(p) = m.get(&h) else { break };
            g.x1 = g.x1.or(p.x1);
            g.y1 = g.y1.or(p.y1);
            g.x2 = g.x2.or(p.x2);
            g.y2 = g.y2.or(p.y2);
            g.user = g.user.or(p.user);
            g.kind = g.kind.or(p.kind);
            if g.stops.is_empty() {
                g.stops = p.stops.clone();
            }
            if g.own.is_none() {
                g.own = p.own.clone();
            }
            next = p.href.clone();
            depth += 1;
        }
        Some(g)
    })
}

/// Verlauf eines gelesenen Objekts (Form bereits in Dokumentkoordinaten).
fn object_gradient(c: &Ctx, shape: &Shape) -> Option<Gradient> {
    let raw = resolve_gradient(c.fill_url.as_deref()?)?;
    let mut stops = raw.own.clone().unwrap_or_else(|| raw.stops.clone());
    if stops.is_empty() {
        return None;
    }
    if stops.len() == 1 {
        let mut s = stops[0];
        s.pos = 1.0;
        stops.push(s);
    }
    let kind = raw.kind.unwrap_or_default();
    let (x1, y1, x2, y2) = (
        raw.x1.unwrap_or(0.0),
        raw.y1.unwrap_or(0.0),
        raw.x2.unwrap_or(1.0),
        raw.y2.unwrap_or(0.0),
    );
    let (p0, p1) = if raw.user.unwrap_or(false) {
        let r = shape.geom_bbox()?;
        let a = c.m.apply(Pt::new(x1, y1));
        let b = c.m.apply(Pt::new(x2, y2));
        let mut g = Gradient::new(Color::BLACK, Color::WHITE);
        g.set_line(a, b, &r);
        (g.p0, g.p1)
    } else {
        (Pt::new(x1, y1), Pt::new(x2, y2))
    };
    Some(Gradient {
        stops,
        p0,
        p1,
        kind,
        shape: crate::model::GradShape::Linear,
        cell: crate::model::PATTERN_CELL,
    })
}

/// Eigenes Attribut (auch aus Dateien, die noch unter dem alten Namen „Linea“ gespeichert wurden).
fn own<'a>(el: &'a Element, name: &str) -> Option<&'a str> {
    el.attr(&format!("mondrian:{name}"))
        // ältere Dateien (Silhouettes, Linea)
        .or_else(|| el.attr(&format!("silhouettes:{name}")))
        .or_else(|| el.attr(&format!("linea:{name}")))
}

fn parse_length(s: &str) -> Option<f64> {
    let s = s.trim();
    // Zahl (auch mit Exponent wie „1e3“ oder „2.5E-1“), danach die Einheit
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
        i += 1;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    let (numpart, unit) = (&s[..i], s[i..].trim());
    let v: f64 = numpart.trim().parse().ok()?;
    let f = match unit {
        "" | "px" => 1.0,
        "mm" => 96.0 / 25.4,
        "cm" => 96.0 / 2.54,
        "in" => 96.0,
        "pt" => 96.0 / 72.0,
        "pc" => 16.0,
        "em" => 16.0,
        _ => return None,
    };
    Some(v * f)
}

fn parse_paint(v: &str) -> Option<Paint> {
    let v = v.trim();
    if v == "none" {
        return Some(Paint::None);
    }
    if v == "inherit" {
        return None;
    }
    if v.starts_with("url(") {
        // Verläufe und Muster werden noch nicht unterstützt: Ersatzfarbe.
        // Eine eventuell angegebene Ausweichfarbe nach der URL verwenden.
        if let Some(close) = v.find(')') {
            if let Some(c) = Color::parse(v[close + 1..].trim()) {
                return Some(Paint::Color(c));
            }
        }
        return Some(Paint::Color(Color::rgb(0.5, 0.5, 0.5)));
    }
    if v == "currentColor" {
        return Some(Paint::Color(Color::BLACK));
    }
    Color::parse(v).map(Paint::Color)
}

fn apply_prop(ctx: &mut Ctx, key: &str, val: &str) {
    let val = val.trim();
    match key {
        "fill" => {
            if let Some(p) = parse_paint(val) {
                ctx.fill = p;
                ctx.fill_url = None;
                if let Some(rest) = val.strip_prefix("url(") {
                    let id = rest
                        .split(')')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .trim_matches(|c| c == '\'' || c == '"')
                        .trim_start_matches('#')
                        .to_string();
                    if let Some(g) = resolve_gradient(&id) {
                        let first = g
                            .own
                            .as_ref()
                            .and_then(|v| v.first().copied())
                            .or_else(|| g.stops.first().copied());
                        if let Some(st) = first {
                            ctx.fill = Paint::Color(st.color);
                        }
                        ctx.fill_url = Some(id);
                    }
                }
            }
        }
        "mix-blend-mode" => {
            if let Some(b) = Blend::from_css(val) {
                ctx.blend = b;
            }
        }
        "text-anchor" => {
            ctx.align = match val {
                "middle" => Align::Center,
                "end" => Align::Right,
                _ => Align::Left,
            }
        }
        "text-align" => {
            ctx.align = match val {
                "center" => Align::Center,
                "right" | "end" => Align::Right,
                "justify" => Align::Justify,
                _ => Align::Left,
            }
        }
        "letter-spacing" => {
            ctx.letter_spacing = if val == "normal" {
                0.0
            } else {
                parse_length(val).unwrap_or(0.0)
            };
        }
        "line-height" => {
            if let Ok(f) = val.parse::<f64>() {
                ctx.line_factor = Some(f);
                ctx.line_px = None;
            } else if let Some(pc) = val.strip_suffix('%').and_then(|p| p.trim().parse::<f64>().ok()) {
                ctx.line_factor = Some(pc / 100.0);
                ctx.line_px = None;
            } else if let Some(px) = parse_length(val) {
                ctx.line_px = Some(px);
                ctx.line_factor = None;
            }
        }
        "stroke" => {
            if let Some(p) = parse_paint(val) {
                ctx.stroke = p;
            }
        }
        "stroke-width" => {
            if let Some(w) = parse_length(val) {
                ctx.width = w;
            }
        }
        "stroke-linejoin" => {
            if let Some(j) = LineJoin::from_css(val) {
                ctx.join = j;
            }
        }
        "stroke-linecap" => {
            if let Some(c) = LineCap::from_css(val) {
                ctx.cap = c;
            }
        }
        "stroke-dasharray" => {
            ctx.dash = if val.trim() == "none" {
                Vec::new()
            } else {
                val.split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|t| !t.is_empty())
                    .filter_map(parse_length)
                    .collect()
            };
        }
        "fill-opacity" => {
            if let Ok(v) = val.parse() {
                ctx.fill_op = v;
            }
        }
        "stroke-opacity" => {
            if let Ok(v) = val.parse() {
                ctx.stroke_op = v;
            }
        }
        "opacity" => {
            if let Ok(v) = val.parse::<f64>() {
                ctx.opacity *= v;
            }
        }
        "fill-rule" => ctx.evenodd = val == "evenodd",
        "font-family" => {
            let first = val.split(',').next().unwrap_or(val).trim();
            let first = first.trim_matches(|c| c == '\'' || c == '"');
            if !first.is_empty() {
                ctx.family = first.to_string();
            }
        }
        "font-size" => {
            if let Some(v) = parse_length(val) {
                ctx.size = v;
            }
        }
        "font-weight" => {
            let n = val.trim().parse::<u16>().ok();
            ctx.bold = val == "bold" || val == "bolder" || n.is_some_and(|w| w >= 600);
            ctx.weight = n.filter(|w| *w != 400 && *w != 700).unwrap_or(0);
        }
        "font-style" => ctx.italic = val == "italic" || val == "oblique",
        "display" => {
            if val == "none" {
                ctx.hidden = true;
            }
        }
        "visibility" => {
            if val == "hidden" || val == "collapse" {
                ctx.hidden = true;
            }
        }
        _ => {}
    }
}

/// Neuen Kontext für ein Element aus dem Elternkontext ableiten.
fn derive_ctx(parent: &Ctx, el: &Element) -> Ctx {
    let mut c = parent.clone();
    if let Some(t) = el.attr("transform") {
        c.m = parent.m.then_before(&Affine::parse_svg(t));
    }
    for (k, v) in &el.attrs {
        apply_prop(&mut c, k, v);
    }
    // CSS-Regeln stehen über den Attributen, aber unter `style`
    for (k, v) in css_for(el) {
        apply_prop(&mut c, &k, &v);
    }
    if let Some(style) = el.attr("style") {
        for decl in style.split(';') {
            if let Some((k, v)) = decl.split_once(':') {
                apply_prop(&mut c, k.trim(), v);
            }
        }
    }
    c
}

fn style_from_ctx(c: &Ctx) -> Style {
    let col = |p: Paint, op: f64| match p {
        Paint::None => None,
        Paint::Color(mut col) => {
            col.a = (col.a * op).clamp(0.0, 1.0);
            Some(col)
        }
    };
    Style {
        fill: col(c.fill, c.fill_op),
        stroke: col(c.stroke, c.stroke_op),
        width: (c.width * c.m.mean_scale()).max(0.0),
        opacity: c.opacity.clamp(0.0, 1.0),
        gradient: None,
        blend: c.blend,
        marker: Marker::None,
        marker_ends: MarkerEnds::End,
        marker_size: 1.0,
        join: c.join,
        cap: c.cap,
        shadow: None,
        stroke_gradient: None,
        fx: crate::model::Effects::default(),
        shadow_gradient: None,
        stroke_align: crate::model::StrokeAlign::Center,
        dash: crate::model::Dash::from_array(&c.dash, c.width),
    }
}

fn attr_num(el: &Element, name: &str) -> f64 {
    el.attr(name).and_then(parse_length).unwrap_or(0.0)
}

fn first_num(el: &Element, name: &str) -> Option<f64> {
    el.attr(name).and_then(|v| parse_numbers(v).first().copied())
}

/// SVG-Pfaddaten in Teilpfade umwandeln.
pub fn parse_path(d: &str) -> Vec<SubPath> {
    let mut subs: Vec<SubPath> = Vec::new();
    let mut cur: Vec<Node> = Vec::new();
    let mut pos = Pt::default();
    let mut start = Pt::default();
    let mut last_ctrl: Option<Pt> = None; // für S
    let mut last_quad: Option<Pt> = None; // für T
    let mut cmd = b'M';
    let mut lex = NumLexer::new(d);

    fn finish(subs: &mut Vec<SubPath>, cur: &mut Vec<Node>, closed: bool) {
        if cur.is_empty() {
            return;
        }
        let mut nodes = std::mem::take(cur);
        if closed && nodes.len() > 1 {
            let l = nodes.len() - 1;
            if nodes[l].p.dist(nodes[0].p) < 1e-6 {
                nodes[0].h_in = nodes[l].h_in;
                nodes.pop();
            }
        }
        subs.push(SubPath { nodes, closed });
    }

    let ensure_start = |cur: &mut Vec<Node>, pos: Pt| {
        if cur.is_empty() {
            cur.push(Node::corner(pos));
        }
    };

    loop {
        let Some(c) = lex.peek() else { break };
        if c.is_ascii_alphabetic() {
            cmd = c;
            lex.i += 1;
            if cmd == b'Z' || cmd == b'z' {
                finish(&mut subs, &mut cur, true);
                pos = start;
                last_ctrl = None;
                last_quad = None;
                continue;
            }
        } else if cmd == b'Z' || cmd == b'z' {
            // Zahlen nach Z ohne neuen Befehl: ungültig, abbrechen.
            break;
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { pos } else { Pt::default() };
        macro_rules! num {
            () => {
                match lex.next_number() {
                    Some(v) => v,
                    None => break,
                }
            };
        }
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let p = Pt::new(num!(), num!()) + base;
                finish(&mut subs, &mut cur, false);
                cur.push(Node::corner(p));
                pos = p;
                start = p;
                // weitere Koordinatenpaare gelten als L
                cmd = if rel { b'l' } else { b'L' };
                last_ctrl = None;
                last_quad = None;
            }
            b'L' => {
                let p = Pt::new(num!(), num!()) + base;
                ensure_start(&mut cur, pos);
                cur.push(Node::corner(p));
                pos = p;
                last_ctrl = None;
                last_quad = None;
            }
            b'H' => {
                let x = num!() + if rel { pos.x } else { 0.0 };
                let p = Pt::new(x, pos.y);
                ensure_start(&mut cur, pos);
                cur.push(Node::corner(p));
                pos = p;
                last_ctrl = None;
                last_quad = None;
            }
            b'V' => {
                let y = num!() + if rel { pos.y } else { 0.0 };
                let p = Pt::new(pos.x, y);
                ensure_start(&mut cur, pos);
                cur.push(Node::corner(p));
                pos = p;
                last_ctrl = None;
                last_quad = None;
            }
            b'C' | b'S' => {
                let c1 = if cmd.to_ascii_uppercase() == b'C' {
                    Pt::new(num!(), num!()) + base
                } else {
                    match last_ctrl {
                        Some(lc) => pos + (pos - lc),
                        None => pos,
                    }
                };
                let c2 = Pt::new(num!(), num!()) + base;
                let p = Pt::new(num!(), num!()) + base;
                ensure_start(&mut cur, pos);
                if let Some(l) = cur.last_mut() {
                    l.h_out = c1;
                }
                cur.push(Node {
                    r: 0.0,
                    p,
                    h_in: c2,
                    h_out: p,
                });
                pos = p;
                last_ctrl = Some(c2);
                last_quad = None;
            }
            b'Q' | b'T' => {
                let q = if cmd.to_ascii_uppercase() == b'Q' {
                    Pt::new(num!(), num!()) + base
                } else {
                    match last_quad {
                        Some(lq) => pos + (pos - lq),
                        None => pos,
                    }
                };
                let p = Pt::new(num!(), num!()) + base;
                let c1 = pos + (q - pos) * (2.0 / 3.0);
                let c2 = p + (q - p) * (2.0 / 3.0);
                ensure_start(&mut cur, pos);
                if let Some(l) = cur.last_mut() {
                    l.h_out = c1;
                }
                cur.push(Node {
                    r: 0.0,
                    p,
                    h_in: c2,
                    h_out: p,
                });
                pos = p;
                last_quad = Some(q);
                last_ctrl = None;
            }
            b'A' => {
                let rx = num!();
                let ry = num!();
                let rot = num!();
                let Some(large) = lex.next_flag() else { break };
                let Some(sweep) = lex.next_flag() else { break };
                let p = Pt::new(num!(), num!()) + base;
                ensure_start(&mut cur, pos);
                for (c1, c2, e) in arc_to_cubics(pos, rx, ry, rot, large, sweep, p) {
                    if let Some(l) = cur.last_mut() {
                        l.h_out = c1;
                    }
                    cur.push(Node {
                        r: 0.0,
                        p: e,
                        h_in: c2,
                        h_out: e,
                    });
                }
                pos = p;
                last_ctrl = None;
                last_quad = None;
            }
            _ => break,
        }
    }
    finish(&mut subs, &mut cur, false);
    subs.retain(|s| !s.nodes.is_empty());
    subs
}

fn points_attr(el: &Element, closed: bool) -> Option<Shape> {
    let nums = parse_numbers(el.attr("points")?);
    let nodes: Vec<Node> = nums
        .chunks_exact(2)
        .map(|c| Node::corner(Pt::new(c[0], c[1])))
        .collect();
    if nodes.len() < 2 {
        return None;
    }
    Some(Shape::Path {
        subpaths: vec![SubPath { nodes, closed }],
        evenodd: false,
    })
}

/// Absatzformat eines gelesenen Textes (eigene Attribute haben Vorrang).
fn text_format(el: &Element, ctx: &Ctx) -> TextFormat {
    let mut f = TextFormat {
        align: own(el, "align").and_then(Align::from_name).unwrap_or(ctx.align),
        valign: own(el, "valign").and_then(VAlign::from_name).unwrap_or_default(),
        weight: ctx.weight,
        ..TextFormat::default()
    };
    if let Some(l) = own(el, "line").and_then(|v| v.parse::<f64>().ok()) {
        f.line = l;
    } else if let Some(l) = ctx.line_factor {
        f.line = l;
    } else if let Some(px) = ctx.line_px {
        f.line = px / ctx.size.max(0.1);
    }
    if let Some(t) = own(el, "tracking").and_then(|v| v.parse::<f64>().ok()) {
        f.tracking = t;
    } else if ctx.letter_spacing.abs() > 1e-9 {
        f.tracking = ctx.letter_spacing / ctx.size.max(0.1) * 1000.0;
    }
    if let Some(d) = el.attr("text-decoration") {
        f.underline = d.contains("underline");
        f.strike = d.contains("line-through");
    }
    f.list = own(el, "list")
        .and_then(crate::model::ListStyle::from_name)
        .unwrap_or_default();
    f.path_below = own(el, "path-below") == Some("true");
    f.path_reverse = own(el, "path-reverse") == Some("true");
    if let Some(v) = own(el, "para").and_then(|v| v.parse::<f64>().ok()) {
        f.para = v;
    }
    if let Some(v) = own(el, "columns").and_then(|v| v.parse::<u32>().ok()) {
        f.columns = v.clamp(1, crate::model::MAX_COLUMNS);
    }
    f.small_caps = el.attr("font-variant").is_some_and(|v| v.contains("small-caps"));
    f.no_ligatures = own(el, "no-liga") == Some("true");
    let col = |v: &str| {
        let mut it = v.split(':');
        let mut c = Color::parse(it.next()?)?;
        if let Some(a) = it.next().and_then(|a| a.parse::<f64>().ok()) {
            c.a = a;
        }
        Some(c)
    };
    f.box_fill = own(el, "box-fill").and_then(col);
    f.box_stroke = own(el, "box-stroke").and_then(col);
    if let Some(v) = own(el, "box-width").and_then(|v| v.parse::<f64>().ok()) {
        f.box_width = v.max(0.0);
    }
    if let Some(v) = own(el, "box-inset").and_then(|v| v.parse::<f64>().ok()) {
        f.box_inset = v.max(0.0);
    }
    f
}

/// Rahmen eines Textfeldes aus den eigenen Attributen.
fn read_text_frame(el: &Element) -> TextFrame {
    let col = |v: &str| {
        let mut it = v.split(':');
        let mut c = Color::parse(it.next()?)?;
        if let Some(a) = it.next().and_then(|a| a.parse::<f64>().ok()) {
            c.a = a;
        }
        Some(c)
    };
    TextFrame {
        path: own(el, "frame-d")
            .map(parse_path)
            .filter(|p| !p.is_empty())
            .map(|mut p| {
                apply_radii(&mut p, own(el, "frame-radii").unwrap_or(""));
                p
            }),
        fill: own(el, "frame-fill").and_then(col),
        stroke: own(el, "frame-stroke").and_then(col),
        width: own(el, "frame-width").and_then(|v| v.parse().ok()).unwrap_or(1.0),
        inset: own(el, "inset").and_then(|v| v.parse().ok()).unwrap_or(0.0),
    }
}

fn read_text(el: &Element, ctx: &Ctx, ids: &PathIds) -> Option<Shape> {
    let fmt = text_format(el, ctx);
    // Textfeld (von Stijl geschrieben)
    if let Some(area) = el.attr_local("area") {
        let v = parse_numbers(area);
        if v.len() >= 2 {
            let x = first_num(el, "x").unwrap_or(0.0);
            let y = first_num(el, "y").unwrap_or(0.0);
            let (text, rich) = read_rich(el);
            let mut shape = Shape::TextArea {
                xf: Affine::translate(x, y - ctx.size * AREA_ASCENT),
                w: v[0],
                h: v[1],
                text,
                rich,
                family: ctx.family.clone(),
                size: ctx.size,
                bold: ctx.bold,
                italic: ctx.italic,
                fmt,
                frame: read_text_frame(el),
            };
            if !ctx.m.is_identity() {
                shape.transform(&ctx.m);
            }
            return Some(shape);
        }
    }
    // Text auf Pfad?
    if let Some(tp) = el.elements().find(|e| e.local() == "textPath") {
        let href = tp.attr("href").or_else(|| tp.attr_local("href"))?;
        let (d, tf) = ids.get(href.trim_start_matches('#'))?;
        let mut path = parse_path(d);
        if path.is_empty() {
            return None;
        }
        if let Some(t) = tf {
            let m = Affine::parse_svg(t);
            let mut sh = Shape::Path {
                subpaths: path,
                evenodd: false,
            };
            sh.transform(&m);
            let Shape::Path { subpaths, .. } = sh else {
                return None;
            };
            path = subpaths;
        }
        let (_, len) = crate::model::path_polyline(&path);
        let total = len.last().copied().unwrap_or(0.0);
        let start = match tp.attr("startOffset").map(str::trim) {
            Some(v) if v.ends_with('%') => grad_coord(v).unwrap_or(0.0) * total,
            Some(v) => parse_length(v).unwrap_or(0.0),
            None => 0.0,
        };
        let offset = crate::model::textpath_offset(total, start, fmt.align);
        let text = tp.text().replace('\n', " ");
        let mut shape = Shape::TextPath {
            path,
            text: text.trim().to_string(),
            family: ctx.family.clone(),
            size: ctx.size,
            bold: ctx.bold,
            italic: ctx.italic,
            offset,
            fmt,
        };
        if !ctx.m.is_identity() {
            shape.transform(&ctx.m);
        }
        return Some(shape);
    }
    let x = first_num(el, "x").unwrap_or(0.0);
    let y = first_num(el, "y").unwrap_or(0.0);
    let mut lines: Vec<String> = vec![String::new()];
    let mut started = false;
    let mut first_y: Option<f64> = None;
    let mut cur_y = y;
    for child in &el.children {
        match child {
            XNode::Text(t) => {
                if t.trim().is_empty() {
                    continue;
                }
                lines.last_mut().unwrap().push_str(&t.replace('\n', " "));
                started = true;
            }
            XNode::Elem(e) if e.local() == "tspan" => {
                let ty = first_num(e, "y");
                let role_line = e.attr_local("role") == Some("line");
                let new_line =
                    started && (role_line || ty.map(|v| (v - cur_y).abs() > 1e-6).unwrap_or(false));
                if new_line {
                    lines.push(String::new());
                }
                if let Some(v) = ty {
                    cur_y = v;
                    if first_y.is_none() {
                        first_y = Some(v);
                    }
                }
                lines.last_mut().unwrap().push_str(&e.text().replace('\n', " "));
                started = true;
            }
            _ => {}
        }
    }
    let text = lines.join("\n");
    if text.trim().is_empty() {
        return None;
    }
    let base_y = if el.attr("y").is_some() {
        y
    } else {
        first_y.unwrap_or(y)
    };
    let x = if el.attr("x").is_some() {
        x
    } else {
        el.elements().find_map(|e| first_num(e, "x")).unwrap_or(0.0)
    };
    let mut shape = Shape::Text {
        xf: Affine::translate(x, base_y),
        text,
        family: ctx.family.clone(),
        size: ctx.size,
        bold: ctx.bold,
        italic: ctx.italic,
        fmt,
    };
    if !ctx.m.is_identity() {
        shape.transform(&ctx.m);
    }
    Some(shape)
}

fn read_shape(el: &Element, ctx: &Ctx, ids: &PathIds) -> Option<Shape> {
    let shape = match el.local() {
        "rect" => {
            let w = attr_num(el, "width");
            let h = attr_num(el, "height");
            if w <= 0.0 || h <= 0.0 {
                return None;
            }
            let rx = el.attr("rx").and_then(parse_length);
            let ry = el.attr("ry").and_then(parse_length);
            let r = rx.or(ry).unwrap_or(0.0);
            Shape::Rect {
                x: attr_num(el, "x"),
                y: attr_num(el, "y"),
                w,
                h,
                r,
            }
        }
        "circle" => {
            let r = attr_num(el, "r");
            if r <= 0.0 {
                return None;
            }
            Shape::Ellipse {
                cx: attr_num(el, "cx"),
                cy: attr_num(el, "cy"),
                rx: r,
                ry: r,
            }
        }
        "ellipse" => {
            let rx = attr_num(el, "rx");
            let ry = attr_num(el, "ry");
            if rx <= 0.0 || ry <= 0.0 {
                return None;
            }
            Shape::Ellipse {
                cx: attr_num(el, "cx"),
                cy: attr_num(el, "cy"),
                rx,
                ry,
            }
        }
        "line" => Shape::Line {
            a: Pt::new(attr_num(el, "x1"), attr_num(el, "y1")),
            b: Pt::new(attr_num(el, "x2"), attr_num(el, "y2")),
        },
        "polyline" => points_attr(el, false)?,
        "polygon" => points_attr(el, true)?,
        "path" => {
            let mut subpaths = match own(el, "d") {
                Some(base) => parse_path(base),
                None => parse_path(el.attr("d")?),
            };
            if subpaths.is_empty() {
                return None;
            }
            if own(el, "d").is_some() {
                for item in own(el, "radii").unwrap_or("").split_whitespace() {
                    let v: Vec<&str> = item.split(':').collect();
                    if let [si, ni, r] = v[..] {
                        if let (Ok(si), Ok(ni), Ok(r)) =
                            (si.parse::<usize>(), ni.parse::<usize>(), r.parse::<f64>())
                        {
                            if let Some(n) = subpaths.get_mut(si).and_then(|s| s.nodes.get_mut(ni)) {
                                n.r = r;
                            }
                        }
                    }
                }
            }
            Shape::Path {
                subpaths,
                evenodd: ctx.evenodd,
            }
        }
        "text" => return read_text(el, ctx, ids),
        "image" => return read_image(el, ctx),
        _ => return None,
    };
    let mut shape = shape;
    if !ctx.m.is_identity() {
        shape.transform(&ctx.m);
    }
    Some(shape)
}

const SKIP: &[&str] = &[
    "defs",
    "metadata",
    "title",
    "desc",
    "namedview",
    "style",
    "script",
    "clipPath",
    "mask",
    "symbol",
    "marker",
    "pattern",
    "linearGradient",
    "radialGradient",
    "filter",
    "foreignObject",
    "use",
    "guide",
    "artboard",
];

/// Sammelt alle Formen unterhalb eines Elements (Gruppen werden aufgelöst).
/// Gruppennummern beim Lesen: Nummer aus der Datei (oder anonyme Gruppe) → neue Nummer.
type Groups = HashMap<String, u64>;

fn new_group(groups: &mut Groups, key: Option<&str>) -> u64 {
    let key = match key {
        Some(k) => k.to_string(),
        None => format!("#{}", groups.len()),
    };
    let next = groups.len() as u64 + 1;
    *groups.entry(key).or_insert(next)
}

fn collect(
    el: &Element,
    ctx: &Ctx,
    ids: &PathIds,
    doc: &mut Document,
    out: &mut Vec<Object>,
    group: u64,
    groups: &mut Groups,
) {
    for child in el.elements() {
        let name = child.local();
        if SKIP.contains(&name) {
            continue;
        }
        // Rahmenlinie eines gerahmten Bildes (gehört zum Bild)
        if own(child, "role") == Some("frame") {
            continue;
        }
        let c = derive_ctx(ctx, child);
        if c.hidden {
            continue;
        }
        if matches!(name, "g" | "a" | "switch" | "svg") {
            // verschachtelte Gruppen werden zu einer (flachen) Gruppe
            let g = if name == "g" && group == 0 {
                new_group(groups, own(child, "group"))
            } else {
                group
            };
            collect(child, &c, ids, doc, out, g, groups);
        } else if let Some(shape) = read_shape(child, &c, ids) {
            let id = doc.alloc_id();
            let mut style = style_from_ctx(&c);
            style.gradient = object_gradient(&c, &shape);
            style.shadow = own(child, "shadow").and_then(crate::model::Shadow::from_attr);
            style.stroke_gradient = own(child, "stroke-gradient").and_then(Gradient::from_attr);
            style.shadow_gradient = own(child, "shadow-gradient").and_then(Gradient::from_attr);
            if let Some(d) = own(child, "dash").and_then(crate::model::Dash::from_name) {
                style.dash = d;
            }
            style.stroke_align = own(child, "stroke-align")
                .and_then(crate::model::StrokeAlign::from_name)
                .unwrap_or_default();
            // Gesamtdeckkraft in die Farben übernehmen (Bilder behalten sie)
            if !matches!(shape, Shape::Image { .. }) {
                style.fold_opacity();
            }
            if let Some(g) = own(child, "fill-gradient").and_then(Gradient::from_attr) {
                style.gradient = Some(g);
            }
            style.fx = own(child, "fx")
                .map(crate::model::Effects::from_attr)
                .unwrap_or_default();
            // ältere Dateien: Texturen als Mischmodus
            use crate::model::{Art, Lens, Texture, FROST_DEFAULT, FX_DEFAULT};
            match own(child, "blend") {
                Some("frost") => style.fx.tex = Some((Texture::Frost, FROST_DEFAULT)),
                Some("liquid") => style.fx.lens = Some((Lens::Drop, FX_DEFAULT)),
                Some("grain") => style.fx.art = Some((Art::Grain, FX_DEFAULT)),
                Some("pixel") => style.fx.art = Some((Art::Pixel, FX_DEFAULT)),
                _ => {}
            }
            // noch ältere: gekörnter/gepixelter Verlauf
            if let Some(g) = style.gradient.as_mut() {
                match g.kind {
                    GradientKind::Grain if style.fx.art.is_none() => {
                        style.fx.art = Some((Art::Grain, FX_DEFAULT))
                    }
                    GradientKind::Pixel if style.fx.art.is_none() => {
                        style.fx.art = Some((Art::Pixel, FX_DEFAULT))
                    }
                    _ => {}
                }
                g.kind = GradientKind::Smooth;
            }
            if let Some(mk) = own(child, "marker").and_then(Marker::from_name) {
                style.marker = mk;
                style.marker_ends = own(child, "marker-ends")
                    .and_then(MarkerEnds::from_name)
                    .unwrap_or_default();
                style.marker_size = own(child, "marker-size")
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(1.0);
            }
            if matches!(shape, Shape::Image { .. }) {
                // Rahmenlinie: Breite steht in Dokumenteinheiten (ohne die Bildmatrix)
                style.width = c.width;
            }
            out.push(Object {
                group,
                locked: own(child, "locked") == Some("true")
                    || child.attr_local("insensitive") == Some("true"),
                name: child.attr_local("label").unwrap_or("").to_string(),
                id,
                shape,
                style,
                angle: own(child, "angle")
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.0),
            });
        }
    }
}

pub fn read(text: &str) -> Result<Document, String> {
    read_with(text, None).map(|(d, _)| d)
}

/// Wie `read`; `base` ist der Ordner der Datei (für verlinkte Bilder mit relativem Pfad).
/// Liefert zusätzlich Hinweise auf Inhalte, die Stijl nicht übernehmen kann (nur bei
/// Dateien anderer Programme; eigene Dateien enthalten nichts davon).
pub fn read_with(text: &str, base: Option<&std::path::Path>) -> Result<(Document, Vec<String>), String> {
    BASE_DIR.with(|b| *b.borrow_mut() = base.map(|p| p.to_path_buf()));
    NOTES.with(|n| n.borrow_mut().clear());
    let r = read_inner(text);
    BASE_DIR.with(|b| *b.borrow_mut() = None);
    CSS.with(|c| c.borrow_mut().clear());
    let notes = NOTES.with(|n| std::mem::take(&mut *n.borrow_mut()));
    r.map(|d| (d, notes))
}

/// Inhalte suchen, die Stijl nicht übernimmt (und die beim Speichern verloren gingen).
fn scan_unsupported(root: &Element) {
    let mut patterns: Vec<String> = Vec::new();
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        if e.local() == "pattern" {
            if let Some(id) = e.attr("id") {
                patterns.push(format!("url(#{id})"));
            }
        }
        stack.extend(e.elements());
    }
    fn walk(el: &Element, gdepth: usize, patterns: &[String]) {
        for c in el.elements() {
            let name = c.local();
            match name {
                "defs" | "metadata" | "style" | "title" | "desc" | "namedview" | "clipPath" | "mask"
                | "symbol" | "marker" | "pattern" | "linearGradient" | "radialGradient" | "filter"
                | "script" => continue,
                "use" => note(tr("wiederverwendete Elemente (<use>)").into()),
                "foreignObject" => note(tr("eingebettete HTML-Inhalte").into()),
                _ => {}
            }
            let style = c.attr("style").unwrap_or("");
            let has = |k: &str| {
                c.attr(k).is_some_and(|v| v != "none")
                    || style.split(';').any(|d| {
                        d.split_once(':')
                            .is_some_and(|(a, v)| a.trim() == k && v.trim() != "none")
                    })
            };
            if has("clip-path") {
                note(tr("Schnittpfade (clip-path)").into());
            }
            if has("mask") {
                note(tr("Masken").into());
            }
            if has("filter") {
                note(tr("Filter (z. B. Weichzeichner oder Schatten anderer Programme)").into());
            }
            if has("marker-start") || has("marker-mid") || has("marker-end") {
                note(tr("Linienenden anderer Programme (Marker)").into());
            }
            let paint = format!(
                "{} {} {style}",
                c.attr("fill").unwrap_or(""),
                c.attr("stroke").unwrap_or("")
            );
            if patterns.iter().any(|p| paint.contains(p.as_str())) {
                note(tr("Muster anderer Programme").into());
            }
            match name {
                "g" => {
                    if gdepth >= 2 {
                        note(tr("verschachtelte Gruppen (werden zu einer Gruppe zusammengefasst)").into());
                    }
                    walk(c, gdepth + 1, patterns);
                }
                "a" | "switch" | "svg" => walk(c, gdepth, patterns),
                _ => {}
            }
        }
    }
    walk(root, 0, &patterns);
}

fn read_inner(text: &str) -> Result<Document, String> {
    let root = xml::parse(text)?;
    // CSS zuerst: Verläufe und Formen greifen darauf zurück
    let (rules, css_ok) = gather_css(&root);
    CSS.with(|c| *c.borrow_mut() = rules);
    let own = root
        .attrs
        .iter()
        .any(|(k, _)| matches!(k.as_str(), "xmlns:mondrian" | "xmlns:silhouettes" | "xmlns:linea"));
    if !own {
        if !css_ok {
            note(tr("Teile der CSS-Formatierung").into());
        }
        scan_unsupported(&root);
    }
    let mut ids = PathIds::new();
    gather_paths(&root, &mut ids);
    let mut grads = HashMap::new();
    gather_gradients(&root, &mut grads);
    GRADS.with(|g| *g.borrow_mut() = grads);
    if root.local() != "svg" {
        return Err(tr("Die Datei ist kein SVG-Dokument.").into());
    }
    let vb: Vec<f64> = root.attr("viewBox").map(parse_numbers).unwrap_or_default();
    let vb = (vb.len() == 4 && vb[2] > 0.0 && vb[3] > 0.0).then(|| (vb[0], vb[1], vb[2], vb[3]));
    let w = root.attr("width").and_then(parse_length);
    let h = root.attr("height").and_then(parse_length);
    let (width, height, root_m) = match (vb, w, h) {
        (Some((vx, vy, vw, vh)), Some(w), Some(h)) => {
            let s = (w / vw).min(h / vh);
            let tx = (w - vw * s) / 2.0 - vx * s;
            let ty = (h - vh * s) / 2.0 - vy * s;
            (
                w,
                h,
                Affine {
                    a: s,
                    d: s,
                    e: tx,
                    f: ty,
                    ..Affine::IDENTITY
                },
            )
        }
        (Some((vx, vy, vw, vh)), _, _) => (vw, vh, Affine::translate(-vx, -vy)),
        (None, Some(w), Some(h)) => (w, h, Affine::IDENTITY),
        _ => (800.0, 600.0, Affine::IDENTITY),
    };

    let mut doc = Document {
        width,
        height,
        background: None,
        layers: Vec::new(),
        next_id: 1,
        unit: root
            .attr("mondrian:unit")
            .or_else(|| root.attr("silhouettes:unit"))
            .or_else(|| root.attr("linea:unit"))
            .filter(|u| matches!(*u, "px" | "mm" | "cm" | "in" | "pt"))
            .unwrap_or("")
            .to_string(),
        guides: root
            .elements()
            .filter(|e| e.local() == "guide")
            .filter_map(|e| {
                Some(Guide {
                    pos: Pt::new(e.attr("x")?.parse().ok()?, e.attr("y")?.parse().ok()?),
                    angle: e.attr("angle").and_then(|a| a.parse().ok()).unwrap_or(0.0),
                    // vorerst Ebenen-Nummer + 1; nach dem Lesen der Ebenen in die ID umgesetzt
                    layer: e
                        .attr("layer")
                        .and_then(|a| a.parse::<u64>().ok())
                        .map(|i| i + 1)
                        .unwrap_or(0),
                })
            })
            .collect(),
        artboards: root
            .elements()
            .filter(|e| e.local() == "artboard")
            .filter_map(|e| {
                let num = |k: &str| e.attr(k).and_then(|v| v.parse::<f64>().ok());
                Some(crate::model::Artboard {
                    name: e.attr("name").unwrap_or(tr("Zeichenfläche")).to_string(),
                    x: num("x")?,
                    y: num("y")?,
                    w: num("width")?.max(1.0),
                    h: num("height")?.max(1.0),
                })
            })
            .collect(),
    };
    let mut base = Ctx {
        m: root_m,
        ..Ctx::default()
    };
    for (k, v) in &root.attrs {
        if k != "width" && k != "height" && k != "transform" {
            apply_prop(&mut base, k, v);
        }
    }
    base.hidden = false;

    let mut loose_layer: Option<usize> = None;
    let mut counter = 0;
    let mut groups = Groups::new();
    for child in root.elements() {
        let name = child.local();
        if SKIP.contains(&name) {
            continue;
        }
        if name == "rect"
            && matches!(
                child.attr("id"),
                Some(BG_ID) | Some("silhouettes-background") | Some("linea-background")
            )
        {
            let c = derive_ctx(&base, child);
            if let Paint::Color(col) = c.fill {
                doc.background = Some(col);
            }
            continue;
        }
        if name == "g" {
            counter += 1;
            let mut c = derive_ctx(&base, child);
            let visible = !c.hidden;
            c.hidden = false;
            let locked = child.attr_local("insensitive") == Some("true");
            let name = child
                .attr_local("label")
                .or(child.attr("id"))
                .map(str::to_string)
                .unwrap_or_else(|| format!("Ebene {counter}"));
            let color = child
                .attr_local("color")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(counter - 1);
            let mut objects = Vec::new();
            collect(child, &c, &ids, &mut doc, &mut objects, 0, &mut groups);
            let id = doc.alloc_id();
            doc.layers.push(Layer {
                id,
                color,
                name,
                visible,
                locked,
                objects,
            });
            loose_layer = None;
        } else {
            let li = match loose_layer {
                Some(i) => i,
                None => {
                    counter += 1;
                    let id = doc.alloc_id();
                    doc.layers.push(Layer {
                        id,
                        color: counter - 1,
                        name: format!("Ebene {counter}"),
                        visible: true,
                        locked: false,
                        objects: Vec::new(),
                    });
                    loose_layer = Some(doc.layers.len() - 1);
                    doc.layers.len() - 1
                }
            };
            // Einzelnes Element wie eine Gruppe mit einem Kind behandeln.
            let wrapper = Element {
                name: "g".into(),
                attrs: Vec::new(),
                children: vec![XNode::Elem(child.clone())],
            };
            let mut objects = Vec::new();
            collect(&wrapper, &base, &ids, &mut doc, &mut objects, 0, &mut groups);
            doc.layers[li].objects.extend(objects);
        }
    }
    if doc.layers.is_empty() {
        let id = doc.alloc_id();
        doc.layers.push(Layer {
            id,
            color: 0,
            name: tr("Ebene 1").into(),
            visible: true,
            locked: false,
            objects: Vec::new(),
        });
    }
    // Gruppen mit nur einem Objekt auflösen
    let mut count: HashMap<u64, usize> = HashMap::new();
    for o in doc.layers.iter().flat_map(|l| l.objects.iter()) {
        *count.entry(o.group).or_default() += 1;
    }
    for o in doc.layers.iter_mut().flat_map(|l| l.objects.iter_mut()) {
        if count.get(&o.group).copied().unwrap_or(0) < 2 {
            o.group = 0;
        }
    }
    doc.fix_ids();
    // Hilfslinien ihren Ebenen zuordnen
    let layer_ids: Vec<u64> = doc.layers.iter().map(|l| l.id).collect();
    for g in doc.guides.iter_mut() {
        g.layer = if g.layer == 0 {
            0
        } else {
            layer_ids.get(g.layer as usize - 1).copied().unwrap_or(0)
        };
    }
    GRADS.with(|g| g.borrow_mut().clear());
    Ok(doc)
}

/// Pfade mit ID (für textPath): ID → (d, transform)
type PathIds = HashMap<String, (String, Option<String>)>;

fn gather_paths(el: &Element, ids: &mut PathIds) {
    for c in el.elements() {
        if c.local() == "path" {
            if let (Some(id), Some(d)) = (c.attr("id"), c.attr("d")) {
                ids.insert(
                    id.to_string(),
                    (d.to_string(), c.attr("transform").map(str::to_string)),
                );
            }
        }
        gather_paths(c, ids);
    }
}

fn read_image(el: &Element, ctx: &Ctx) -> Option<Shape> {
    let href = el.attr("href").or_else(|| el.attr_local("href"))?;
    let data = if let Some(rest) = href.strip_prefix("data:") {
        let (head, payload) = rest.split_once(',')?;
        if head.ends_with(";base64") {
            base64_decode(payload)?
        } else {
            percent_decode(payload)
        }
    } else {
        // verlinktes Bild: relativ zum Ordner der SVG-Datei
        let raw = href.strip_prefix("file://").unwrap_or(href);
        let path = std::path::PathBuf::from(String::from_utf8_lossy(&percent_decode(raw)).to_string());
        let path = if path.is_absolute() {
            path
        } else {
            BASE_DIR
                .with(|b| b.borrow().as_ref().map(|d| d.join(&path)))
                .unwrap_or(path)
        };
        // nur gewöhnliche Dateien, die wirklich Bilder sind: eine präparierte Datei soll weder
        // fremde Dateien (z. B. Schlüssel) ins Dokument holen noch an /dev/zero hängen bleiben
        let usable = std::fs::metadata(&path)
            .map(|m| m.is_file() && m.len() <= MAX_LINKED_IMAGE)
            .unwrap_or(false);
        match usable.then(|| std::fs::read(&path).ok()).flatten() {
            Some(d) if looks_like_image(&d) => d,
            Some(_) => {
                note(trf("Bild „{}“ (keine Bilddatei)", &[&(href)]));
                return None;
            }
            None => {
                note(trf("Bild „{}“ (Datei nicht gefunden)", &[&(href)]));
                return None;
            }
        }
    };
    let w = attr_num(el, "width");
    let h = attr_num(el, "height");
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let xf = ctx
        .m
        .then_before(&Affine::translate(attr_num(el, "x"), attr_num(el, "y")));
    // eigener Rahmen in Dokumentkoordinaten (ctx.m enthält bereits die Bildmatrix)
    let clip = own(el, "clip")
        .map(parse_path)
        .filter(|c| !c.is_empty())
        .map(|mut c| {
            apply_radii(&mut c, own(el, "clip-radii").unwrap_or(""));
            c
        });
    Some(Shape::Image {
        xf,
        w,
        h,
        data: Rc::new(data),
        clip,
    })
}

/// Größte verlinkte Bilddatei, die beim Öffnen eingelesen wird.
const MAX_LINKED_IMAGE: u64 = 200 << 20;

/// Sind die Daten ein Bild (Rastergrafik oder SVG)?
fn looks_like_image(data: &[u8]) -> bool {
    if sniff_mime(data) != "image/svg+xml" {
        return true;
    }
    let head = &data[..data.len().min(4096)];
    String::from_utf8_lossy(head).contains("<svg")
}

/// MIME-Typ anhand der ersten Bytes erkennen.
pub fn sniff_mime(data: &[u8]) -> &'static str {
    if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if data.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if data.starts_with(b"GIF8") {
        "image/gif"
    } else if data.len() > 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        "image/webp"
    } else if data.starts_with(b"BM") {
        "image/bmp"
    } else if data.starts_with(b"II*\0") || data.starts_with(b"MM\0*") {
        "image/tiff"
    } else {
        "image/svg+xml"
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// „%20“ usw. in Adressen auflösen.
fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() && s.is_char_boundary(i + 3) {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            c if c.is_ascii_whitespace() => continue,
            _ => return None,
        } as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Shape;

    fn sample() -> Document {
        let mut d = Document::new(400.0, 300.0, Some(Color::WHITE));
        let id = d.alloc_id();
        d.layers[0].objects.push(Object {
            group: 0,
            locked: false,
            name: String::new(),
            angle: 0.0,
            id,
            shape: Shape::Rect {
                x: 10.0,
                y: 20.0,
                w: 100.0,
                h: 50.0,
                r: 5.0,
            },
            style: Style::default(),
        });
        let id = d.alloc_id();
        d.layers[0].objects.push(Object {
            group: 0,
            locked: false,
            name: String::new(),
            angle: 0.0,
            id,
            shape: Shape::text_at(
                Pt::new(10.0, 200.0),
                "Hallo <Welt>\nZeile & zwei",
                "Cantarell",
                24.0,
                true,
                false,
            ),
            style: Style {
                fill: Some(Color::BLACK),
                stroke: None,
                width: 1.0,
                opacity: 0.5,
                ..Style::default()
            },
        });
        let id = d.alloc_id();
        let mut rotated = Shape::text_at(Pt::new(50.0, 50.0), "Gedreht", "Sans", 12.0, false, true);
        rotated.transform(&Affine::rotate_about(30.0, Pt::new(0.0, 0.0)));
        rotated.transform(&Affine::scale(-1.0, 1.0));
        d.layers[0].objects.push(Object {
            group: 0,
            locked: false,
            name: String::new(),
            angle: 0.0,
            id,
            shape: rotated,
            style: Style::default(),
        });
        let lid = d.alloc_id();
        let id = d.alloc_id();
        d.layers.push(Layer {
            id: lid,
            color: 3,
            name: "Oben".into(),
            visible: false,
            locked: true,
            objects: vec![Object {
                group: 0,
                locked: false,
                name: String::new(),
                angle: 0.0,
                id,
                shape: Shape::Ellipse {
                    cx: 200.0,
                    cy: 150.0,
                    rx: 40.0,
                    ry: 20.0,
                },
                style: Style {
                    fill: Some(Color {
                        r: 1.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.5,
                    }),
                    stroke: None,
                    width: 1.0,
                    opacity: 1.0,
                    ..Style::default()
                },
            }],
        });
        d
    }

    #[test]
    fn roundtrip() {
        let d = sample();
        let s = write(&d, true);
        let r = read(&s).unwrap();
        assert_eq!(r.width, 400.0);
        assert_eq!(r.background, Some(Color::WHITE));
        assert_eq!(r.layers.len(), 2);
        assert_eq!(r.layers[1].name, "Oben");
        assert!(!r.layers[1].visible && r.layers[1].locked);
        assert_eq!(r.layers[0].objects[0].shape, d.layers[0].objects[0].shape);
        assert_eq!(r.layers[0].objects[1].shape, d.layers[0].objects[1].shape);
        // Deckkraft steckt nach dem Laden in den Farben
        assert!((r.layers[0].objects[1].style.opacity - 1.0).abs() < 1e-9);
        match (&r.layers[0].objects[2].shape, &d.layers[0].objects[2].shape) {
            (
                Shape::Text {
                    xf: a, italic: true, ..
                },
                Shape::Text { xf: b, .. },
            ) => {
                for (u, v) in [
                    (a.a, b.a),
                    (a.b, b.b),
                    (a.c, b.c),
                    (a.d, b.d),
                    (a.e, b.e),
                    (a.f, b.f),
                ] {
                    assert!((u - v).abs() < 1e-4, "{u} != {v}");
                }
            }
            other => panic!("{other:?}"),
        }
        let st = &r.layers[1].objects[0].style;
        assert!((st.fill.unwrap().a - 0.5).abs() < 1e-9);
        assert!(st.stroke.is_none());
    }

    #[test]
    fn unit_roundtrip() {
        let mut d = Document::new(1080.0, 1920.0, None);
        d.unit = "px".into();
        let back = read(&write(&d, true)).unwrap();
        assert_eq!(back.unit, "px");
        let plain = read(&write(&Document::new(10.0, 10.0, None), true)).unwrap();
        assert_eq!(plain.unit, "");
    }

    #[test]
    fn textarea_and_layer_color() {
        let mut d = Document::new(300.0, 300.0, None);
        d.layers[0].color = 5;
        let id = d.alloc_id();
        let mut shape = Shape::TextArea {
            xf: Affine::translate(20.0, 30.0),
            w: 200.0,
            h: 120.0,
            text: "Lorem ipsum\nZweite Zeile".into(),
            family: "Sans".into(),
            size: 14.0,
            bold: false,
            italic: true,
            fmt: TextFormat::default(),
            frame: TextFrame {
                path: Some(vec![SubPath {
                    nodes: vec![
                        Node::corner(Pt::new(100.0, 0.0)),
                        Node::corner(Pt::new(200.0, 120.0)),
                        Node::corner(Pt::new(0.0, 120.0)),
                    ],
                    closed: true,
                }]),
                fill: Some(Color::rgb(1.0, 1.0, 0.0)),
                stroke: Some(Color::rgb(0.0, 0.0, 1.0)),
                width: 2.5,
                inset: 6.0,
            },
            rich: RichText {
                runs: vec![TextRun {
                    start: 6,
                    end: 11,
                    style: RunStyle {
                        family: Some("Serif".into()),
                        bold: Some(true),
                        underline: Some(true),
                        ..RunStyle::default()
                    },
                }],
                paras: vec![
                    ParaFmt {
                        list: ListStyle::Dot,
                        indent: 1,
                    },
                    ParaFmt::default(),
                ],
            },
        };
        shape.transform(&Affine::rotate_about(10.0, Pt::new(0.0, 0.0)));
        d.layers[0].objects.push(Object {
            group: 0,
            locked: false,
            name: String::new(),
            angle: 0.0,
            id,
            shape: shape.clone(),
            style: Style::default(),
        });
        let r = read(&write(&d, true)).unwrap();
        assert_eq!(r.layers[0].color, 5);
        match (&r.layers[0].objects[0].shape, &shape) {
            (
                Shape::TextArea {
                    frame: a, rich: ra, ..
                },
                Shape::TextArea {
                    frame: b, rich: rb, ..
                },
            ) => {
                assert_eq!(a, b);
                assert_eq!(ra.paras, rb.paras);
                assert_eq!(ra.runs.len(), 1);
                assert_eq!((ra.runs[0].start, ra.runs[0].end), (6, 11));
                assert_eq!(ra.runs[0].style.family.as_deref(), Some("Serif"));
                assert_eq!(ra.runs[0].style.bold, Some(true));
                assert_eq!(ra.runs[0].style.underline, Some(true));
            }
            _ => panic!(),
        }
        assert_eq!(
            r.layers[0].objects.len(),
            1,
            "Rahmenpfad wird nicht als eigenes Objekt gelesen"
        );
        match (&r.layers[0].objects[0].shape, &shape) {
            (
                Shape::TextArea {
                    xf,
                    w,
                    h,
                    text,
                    italic,
                    ..
                },
                Shape::TextArea {
                    xf: x2, w: w2, h: h2, ..
                },
            ) => {
                assert_eq!(text, "Lorem ipsum\nZweite Zeile");
                assert!(*italic);
                assert!((w - w2).abs() < 1e-3 && (h - h2).abs() < 1e-3);
                assert!(
                    (xf.e - x2.e).abs() < 1e-3 && (xf.f - x2.f).abs() < 1e-3 && (xf.b - x2.b).abs() < 1e-5
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn live_corners_roundtrip() {
        let mut d = Document::new(300.0, 300.0, None);
        let mut subs = match (Shape::Rect {
            x: 10.0,
            y: 10.0,
            w: 100.0,
            h: 80.0,
            r: 0.0,
        })
        .to_path()
        .unwrap()
        {
            Shape::Path { subpaths, .. } => subpaths,
            _ => unreachable!(),
        };
        subs[0].nodes[1].r = 12.0;
        subs[0].nodes[3].r = 4.5;
        let id = d.alloc_id();
        d.layers[0].objects.push(Object {
            group: 0,
            locked: false,
            name: String::new(),
            angle: 0.0,
            id,
            shape: Shape::Path {
                subpaths: subs,
                evenodd: false,
            },
            style: Style::default(),
        });
        let text = write(&d, true);
        assert!(text.contains("mondrian:radii"));
        let r = read(&text).unwrap();
        match &r.layers[0].objects[0].shape {
            Shape::Path { subpaths, .. } => {
                assert_eq!(subpaths[0].nodes.len(), 4);
                assert_eq!(subpaths[0].nodes[1].r, 12.0);
                assert_eq!(subpaths[0].nodes[3].r, 4.5);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn fonts_can_be_embedded() {
        let mut d = Document::new(200.0, 100.0, None);
        let id = d.alloc_id();
        d.layers[0].objects.push(Object {
            id,
            shape: Shape::text_at(Pt::new(5.0, 20.0), "Hallo", "Beispiel Serif", 14.0, true, false),
            style: Style::default(),
            angle: 0.0,
            group: 0,
            locked: false,
            name: String::new(),
        });
        assert_eq!(used_fonts(&d), vec![("Beispiel Serif".to_string(), true, false)]);
        let t = write_with_fonts(&d, true, &|f, b, _| {
            (f == "Beispiel Serif" && b).then(|| (vec![0u8, 1, 2, 3], "ttf"))
        });
        assert!(
            t.contains("@font-face{font-family:\"Beispiel Serif\";font-weight:bold"),
            "{t}"
        );
        assert!(t.contains("AAECAw=="), "{t}");
        // ohne Schriftdatei bleibt alles wie zuvor; das Lesen ignoriert die Regeln
        assert_eq!(write_with_fonts(&d, true, &|_, _, _| None), write(&d, true));
        let r = read(&t).unwrap();
        assert_eq!(r.layers[0].objects.len(), 1);
    }

    #[test]
    fn base64() {
        for data in [&b""[..], b"a", b"ab", b"abc", b"\x89PNG\r\n\x1a\n\x00\xff"] {
            assert_eq!(base64_decode(&base64_encode(data)).unwrap(), data);
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
    }

    #[test]
    fn textpath_and_image_roundtrip() {
        let mut d = Document::new(200.0, 200.0, None);
        let circle = match (Shape::Ellipse {
            cx: 100.0,
            cy: 100.0,
            rx: 50.0,
            ry: 50.0,
        })
        .to_path()
        .unwrap()
        {
            Shape::Path { subpaths, .. } => subpaths,
            _ => unreachable!(),
        };
        let id = d.alloc_id();
        d.layers[0].objects.push(Object {
            group: 0,
            locked: false,
            name: String::new(),
            angle: 0.0,
            id,
            shape: Shape::TextPath {
                path: circle,
                text: "Rund & bunt".into(),
                family: "Sans".into(),
                size: 18.0,
                bold: false,
                italic: false,
                offset: 5.0,
                fmt: TextFormat::default(),
            },
            style: Style::default(),
        });
        let id = d.alloc_id();
        let png = vec![0x89, b'P', b'N', b'G', 1, 2, 3, 250];
        d.layers[0].objects.push(Object {
            group: 0,
            locked: false,
            name: String::new(),
            angle: 0.0,
            id,
            shape: Shape::Image {
                xf: Affine::translate(10.0, 20.0).then_before(&Affine::scale(0.5, 0.5)),
                w: 64.0,
                h: 32.0,
                data: Rc::new(png.clone()),
                clip: Some(vec![SubPath {
                    nodes: vec![
                        Node::corner(Pt::new(12.0, 22.0)),
                        Node::corner(Pt::new(30.0, 22.0)),
                        Node::corner(Pt::new(30.0, 34.0)),
                    ],
                    closed: true,
                }]),
            },
            style: Style {
                opacity: 0.5,
                stroke: Some(Color::rgb(1.0, 0.0, 0.0)),
                width: 3.0,
                ..Style::default()
            },
        });
        let text = write(&d, true);
        let r = read(&text).unwrap();
        assert_eq!(r.layers[0].objects.len(), 2, "{text}");
        match &r.layers[0].objects[0].shape {
            Shape::TextPath {
                text, offset, path, ..
            } => {
                assert_eq!(text, "Rund & bunt");
                assert_eq!(*offset, 5.0);
                assert_eq!(path[0].nodes.len(), 4);
            }
            other => panic!("{other:?}"),
        }
        match &r.layers[0].objects[1].shape {
            Shape::Image {
                xf, w, data, clip, ..
            } => {
                let c = clip.as_ref().expect("Rahmen");
                assert_eq!(c[0].nodes.len(), 3);
                assert!((c[0].nodes[1].p.x - 30.0).abs() < 1e-9);
                assert_eq!(*w, 64.0);
                assert!((xf.a - 0.5).abs() < 1e-9 && (xf.e - 10.0).abs() < 1e-9);
                assert_eq!(**data, png);
            }
            other => panic!("{other:?}"),
        }
        // Bilder behalten ihre Deckkraft
        assert!((r.layers[0].objects[1].style.opacity - 0.5).abs() < 1e-9);
        assert!((r.layers[0].objects[1].style.width - 3.0).abs() < 1e-9);
        assert!(r.layers[0].objects[1].style.stroke.is_some());
    }

    #[test]
    fn path_roundtrip() {
        let shape = Shape::Ellipse {
            cx: 50.0,
            cy: 50.0,
            rx: 30.0,
            ry: 20.0,
        }
        .to_path()
        .unwrap();
        let Shape::Path { subpaths, .. } = &shape else {
            panic!()
        };
        let d = path_data(subpaths);
        let back = parse_path(&d);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].nodes.len(), 4);
        assert!(back[0].closed);
        for (a, b) in back[0].nodes.iter().zip(&subpaths[0].nodes) {
            assert!(a.p.dist(b.p) < 1e-3 && a.h_in.dist(b.h_in) < 1e-3 && a.h_out.dist(b.h_out) < 1e-3);
        }
    }

    #[test]
    fn path_commands() {
        let sp = parse_path("m10 10 h20v20 h-20z M0 0 q 10 10 20 0 t 20 0 a5 5 0 0 1 10 0");
        assert_eq!(sp.len(), 2);
        assert_eq!(sp[0].nodes.len(), 4);
        assert!(sp[0].closed);
        assert_eq!(sp[1].nodes.last().unwrap().p, Pt::new(50.0, 0.0));
    }

    #[test]
    fn foreign_svg() {
        let s = r##"<svg width="100mm" height="50mm" viewBox="0 0 100 50">
          <defs><linearGradient id="g"/></defs>
          <rect x="0" y="0" width="10" height="10" style="fill:#ff0000;stroke:none"/>
          <g transform="translate(10 0) rotate(45)" fill="blue">
            <circle r="5"/>
            <polygon points="0,0 10,0 5,8"/>
          </g>
          <text x="5" y="40" font-size="12pt" font-family="'DejaVu Sans', sans-serif">Hi</text>
        </svg>"##;
        let d = read(s).unwrap();
        assert!((d.width - 377.952).abs() < 0.01);
        // erst lose Ebene, dann Gruppe, dann neue lose Ebene
        assert_eq!(d.layers.len(), 3);
        let o = &d.layers[0].objects[0];
        assert!(matches!(o.shape, Shape::Rect { .. }));
        assert_eq!(o.style.fill.unwrap().to_hex(), "#ff0000");
        assert!(matches!(d.layers[1].objects[1].shape, Shape::Path { .. }));
        match &d.layers[2].objects[0].shape {
            Shape::Text { family, text, .. } => {
                assert_eq!(family, "DejaVu Sans");
                assert_eq!(text, "Hi");
            }
            _ => panic!(),
        }
    }
    #[test]
    fn gradient_blend_format_guides_roundtrip() {
        let mut d = Document::new(300.0, 300.0, None);
        d.guides.push(Guide {
            pos: Pt::new(0.0, 120.0),
            angle: 0.0,
            layer: 0,
        });
        d.guides.push(Guide {
            pos: Pt::new(40.0, 0.0),
            angle: 90.0,
            layer: d.layers[0].id,
        });
        let mut g = Gradient::new(Color::rgb(1.0, 0.0, 0.0), Color::rgb(0.0, 0.0, 1.0));
        g.add_stop(0.3);
        g.kind = GradientKind::Grain;
        g.p0 = Pt::new(0.1, 0.2);
        let id = d.alloc_id();
        d.layers[0].objects.push(Object {
            group: 0,
            locked: false,
            name: String::new(),
            id,
            angle: 25.0,
            shape: Shape::Rect {
                x: 10.0,
                y: 10.0,
                w: 100.0,
                h: 50.0,
                r: 0.0,
            },
            style: Style {
                gradient: Some(g.clone()),
                blend: Blend::Multiply,
                ..Style::default()
            },
        });
        let id = d.alloc_id();
        let mut t = Shape::text_at(
            Pt::new(50.0, 200.0),
            "Mitte\tTab\nZeile",
            "Sans",
            20.0,
            false,
            false,
        );
        *t.text_format_mut().unwrap() = TextFormat {
            align: Align::Center,
            valign: VAlign::Top,
            line: 1.6,
            tracking: 50.0,
            underline: true,
            list: crate::model::ListStyle::Number,
            ..TextFormat::default()
        };
        d.layers[0].objects.push(Object {
            group: 0,
            locked: false,
            name: String::new(),
            id,
            angle: 0.0,
            shape: t,
            style: Style::default(),
        });
        let id = d.alloc_id();
        d.layers[0].objects.push(Object {
            group: 0,
            locked: false,
            name: String::new(),
            id,
            angle: 0.0,
            shape: Shape::TextArea {
                xf: Affine::translate(0.0, 0.0),
                w: 100.0,
                h: 80.0,
                text: "Block".into(),
                family: "Sans".into(),
                size: 12.0,
                bold: false,
                italic: false,
                fmt: TextFormat {
                    align: Align::Justify,
                    valign: VAlign::Bottom,
                    line: 1.4,
                    tracking: 0.0,
                    columns: 3,
                    small_caps: true,
                    no_ligatures: true,
                    ..TextFormat::default()
                },
                frame: TextFrame::default(),
                rich: RichText::default(),
            },
            style: Style {
                blend: Blend::Screen,
                ..Style::default()
            },
        });
        let text = write(&d, true);
        assert!(text.contains("mix-blend-mode:multiply"), "{text}");
        let r = read(&text).unwrap();
        assert_eq!(r.guides.len(), 2);
        assert_eq!(r.guides[0], d.guides[0]);
        assert_eq!(r.guides[1].pos, d.guides[1].pos);
        assert_eq!(r.guides[1].layer, r.layers[0].id);
        let o = &r.layers[0].objects;
        assert_eq!(o.len(), 3, "{text}");
        assert!((o[0].angle - 25.0).abs() < 1e-9);
        assert_eq!(o[0].style.blend, Blend::Multiply);
        let rg = o[0].style.gradient.as_ref().expect("Verlauf");
        // gekörnter Verlauf wird zum Kunsteffekt „Körnung“
        assert_eq!(rg.kind, GradientKind::Smooth);
        assert_eq!(o[0].style.fx.art.map(|a| a.0), Some(crate::model::Art::Grain));
        assert_eq!(rg.stops.len(), 3);
        assert!((rg.p0.x - 0.1).abs() < 1e-6 && (rg.p0.y - 0.2).abs() < 1e-6);
        assert_eq!(
            o[1].shape.text_fields().map(|t| t.0.to_string()).as_deref(),
            Some("Mitte\tTab\nZeile"),
            "{text}"
        );
        let f = o[1].shape.text_format().unwrap();
        assert_eq!(f.align, Align::Center);
        assert!((f.line - 1.6).abs() < 1e-9 && (f.tracking - 50.0).abs() < 1e-9);
        assert!(f.underline && !f.strike && f.list == crate::model::ListStyle::Number);
        let f = o[2].shape.text_format().unwrap();
        assert_eq!((f.align, f.valign), (Align::Justify, VAlign::Bottom));
        assert_eq!((f.columns, f.small_caps, f.no_ligatures), (3, true, true));
        assert!(text.contains("column-count:3"), "{text}");
        assert_eq!(o[2].style.blend, Blend::Screen);
        // Effekte als eigenes Attribut, Muster und kreisförmige Verläufe
        use crate::model::{Art, Effects, GradShape, Lens, Texture};
        let mut r2 = r.clone();
        r2.layers[0].objects[0].style.fx = Effects {
            tex: Some((Texture::Frost, 0.7)),
            lens: Some((Lens::Drop, -0.3)),
            ..Effects::default()
        };
        r2.layers[0].objects[1].style.fx.art = Some((Art::Watercolor, 0.6));
        r2.layers[0].objects[2].style.fx.mirror = Some(0.4);
        r2.layers[0].objects[2].style.fx.tint = Some((crate::model::ImgFilter::Sepia, 1.0));
        r2.layers[0].objects[2].style.fx.temp = -0.35;
        r2.layers[0].objects[1].style.fx.tex = Some((Texture::Brushed, 0.4));
        r2.layers[0].objects[1].style.fx.lens = Some((Lens::Pyramid, 0.8));
        r2.layers[0].objects[1].style.fx.light = 0.2;
        if let Some(g) = r2.layers[0].objects[0].style.gradient.as_mut() {
            g.set_shape(GradShape::Dots);
            g.cell = 22.0;
        }
        let t2 = write(&r2, true);
        assert!(t2.contains("<pattern"), "{t2}");
        let r3 = read(&t2).unwrap();
        assert_eq!(r3.layers[0].objects[0].style.fx, r2.layers[0].objects[0].style.fx);
        assert_eq!(r3.layers[0].objects[1].style.fx.art, Some((Art::Watercolor, 0.6)));
        assert_eq!(
            r3.layers[0].objects[1].style.fx.tex,
            Some((Texture::Brushed, 0.4))
        );
        assert_eq!(r3.layers[0].objects[1].style.fx.lens, Some((Lens::Pyramid, 0.8)));
        assert!((r3.layers[0].objects[1].style.fx.light - 0.2).abs() < 1e-9);
        // ohne Angabe gilt die Grundstärke der Beleuchtung
        assert_eq!(
            crate::model::Effects::from_attr("lens=drop:0.5").light,
            crate::model::LIGHT_DEFAULT
        );
        assert!(!crate::model::Effects::default().to_attr().contains("light"));
        assert_eq!(r3.layers[0].objects[2].style.fx.mirror, Some(0.4));
        assert_eq!(
            r3.layers[0].objects[2].style.fx.tint,
            Some((crate::model::ImgFilter::Sepia, 1.0))
        );
        assert_eq!(r3.layers[0].objects[2].style.fx.temp, -0.35);
        // alte Dateien: Konvex und Konkav sind jetzt der Regentropfen mit Vorzeichen
        let old = crate::model::Effects::from_attr("lens=convex:0.6");
        assert_eq!(old.lens, Some((Lens::Drop, 0.6)));
        let old = crate::model::Effects::from_attr("lens=concave:0.3 frost=0.5");
        assert_eq!(old.lens, Some((Lens::Drop, -0.3)));
        assert_eq!(old.tex, Some((Texture::Frost, 0.5)));
        // alte Dateien: Warm und Kalt werden zur Temperatur
        let old = crate::model::Effects::from_attr("tint=warm:0.4");
        assert!(old.tint.is_none() && (old.temp - 0.4).abs() < 1e-9);
        assert!((crate::model::Effects::from_attr("tint=cool:0.5").temp + 0.5).abs() < 1e-9);
        let g3 = r3.layers[0].objects[0].style.gradient.clone().unwrap();
        assert_eq!((g3.shape, g3.cell), (GradShape::Dots, 22.0));
        let mut r4 = r3.clone();
        r4.layers[0].objects[0].style.shadow = Some(crate::model::Shadow {
            color: Color::rgb(1.0, 0.0, 0.0),
            opacity: 0.5,
            blur: 12.0,
            dx: -3.0,
            dy: 7.5,
        });
        let t4 = write(&r4, true);
        assert!(t4.contains("feGaussianBlur"), "{t4}");
        let r5 = read(&t4).unwrap();
        assert_eq!(
            r5.layers[0].objects[0].style.shadow,
            r4.layers[0].objects[0].style.shadow
        );
        assert_eq!(r5.layers[0].objects[1].style.shadow, None);
    }

    #[test]
    fn foreign_gradient() {
        let s = r##"<svg width="100" height="100" viewBox="0 0 100 100">
          <defs>
            <linearGradient id="a"><stop offset="0" stop-color="#ff0000"/><stop offset="100%" style="stop-color:#0000ff;stop-opacity:0.5"/></linearGradient>
            <linearGradient id="b" href="#a" x1="0" y1="0" x2="0" y2="1"/>
            <linearGradient id="u" gradientUnits="userSpaceOnUse" x1="10" y1="0" x2="60" y2="0"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient>
          </defs>
          <rect x="0" y="0" width="50" height="50" fill="url(#b)"/>
          <rect x="10" y="60" width="50" height="20" style="fill:url('#u')"/>
        </svg>"##;
        let d = read(s).unwrap();
        let o = &d.layers[0].objects;
        let g = o[0].style.gradient.as_ref().unwrap();
        assert_eq!(g.stops.len(), 2);
        assert!((g.stops[1].color.a - 0.5).abs() < 1e-9);
        assert!((g.p1.y - 1.0).abs() < 1e-9 && g.p1.x.abs() < 1e-9);
        let u = o[1].style.gradient.as_ref().unwrap();
        assert!(u.p0.x.abs() < 1e-9 && (u.p1.x - 1.0).abs() < 1e-9);
    }
    #[test]
    fn marker_roundtrip() {
        let mut d = Document::new(100.0, 100.0, None);
        let id = d.alloc_id();
        d.layers[0].objects.push(Object {
            group: 0,
            locked: false,
            name: String::new(),
            id,
            angle: 0.0,
            shape: Shape::Line {
                a: Pt::new(0.0, 0.0),
                b: Pt::new(50.0, 50.0),
            },
            style: Style {
                fill: None,
                marker: Marker::Arrow,
                marker_ends: MarkerEnds::Both,
                ..Style::default()
            },
        });
        let text = write(&d, true);
        assert!(text.contains("marker-start") && text.contains("marker-end"));
        let r = read(&text).unwrap();
        let st = &r.layers[0].objects[0].style;
        assert_eq!((st.marker, st.marker_ends), (Marker::Arrow, MarkerEnds::Both));
    }

    #[test]
    fn reads_old_silhouettes_files() {
        let text = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:silhouettes="https://silhouettes.invalid/ns" width="100" height="100" silhouettes:unit="mm">
          <rect x="1" y="2" width="30" height="40" fill="#ff0000" silhouettes:blend="frost"/>
        </svg>"##;
        let d = read(text).unwrap();
        assert_eq!(d.unit, "mm");
        let o = &d.layers[0].objects[0];
        assert_eq!(
            o.style.fx.tex,
            Some((crate::model::Texture::Frost, crate::model::FROST_DEFAULT))
        );
    }

    #[test]
    fn artboards_roundtrip() {
        let mut d = Document::new(200.0, 100.0, None);
        d.artboards.push(crate::model::Artboard {
            name: "Rückseite & Co".into(),
            x: 260.0,
            y: 0.0,
            w: 150.0,
            h: 90.0,
        });
        let t = write(&d, true);
        let r = read(&t).unwrap();
        assert_eq!(r.artboards, d.artboards);
        let rects = r.artboard_rects();
        assert_eq!(rects.len(), 2);
        let one = r.artboard_doc(1);
        assert_eq!((one.width, one.height), (150.0, 90.0));
        assert!(one.artboards.is_empty());
    }

    #[test]
    fn css_classes_from_other_programs() {
        // so schreibt z. B. Illustrator („Exportieren als SVG“, interne CSS)
        let t = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
          <defs><style>/* Farben */ .cls-1{fill:#e30613;} .cls-2, rect.big {fill:#00ff00;stroke:#000;stroke-width:2px}
          #spezial { fill: #0000ff !important } @media print { .cls-1 { fill: #000 } }</style></defs>
          <path class="cls-1" d="M0 0L10 0L10 10Z"/>
          <rect class="cls-2" x="20" y="0" width="10" height="10"/>
          <rect class="big" x="40" y="0" width="10" height="10" fill="#123456"/>
          <circle id="spezial" class="cls-1" cx="70" cy="5" r="5"/>
          <rect class="cls-1" x="80" y="0" width="5" height="5" style="fill:#ffff00"/>
        </svg>"##;
        let (d, notes) = read_with(t, None).unwrap();
        let o = &d.layers[0].objects;
        let fill = |i: usize| o[i].style.fill.map(|c| c.to_hex());
        assert_eq!(fill(0).as_deref(), Some("#e30613"));
        assert_eq!(fill(1).as_deref(), Some("#00ff00"));
        assert!((o[1].style.width - 2.0).abs() < 1e-9);
        // CSS steht über dem Attribut …
        assert_eq!(fill(2).as_deref(), Some("#00ff00"));
        // … die ID über der Klasse, `style` über allem
        assert_eq!(fill(3).as_deref(), Some("#0000ff"));
        assert_eq!(fill(4).as_deref(), Some("#ffff00"));
        // @media wird nicht ausgewertet – das wird gemeldet
        assert!(notes.iter().any(|n| n.contains("CSS")), "{notes:?}");
    }

    #[test]
    fn unsupported_content_is_reported_but_not_for_own_files() {
        let t = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
          <defs><clipPath id="c"><rect width="5" height="5"/></clipPath>
            <pattern id="p" width="4" height="4"><rect width="2" height="2"/></pattern></defs>
          <g><g><g><rect width="3" height="3"/></g></g></g>
          <rect width="5" height="5" clip-path="url(#c)"/>
          <rect width="5" height="5" style="fill:url(#p)"/>
          <use href="#c"/>
          <image href="fehlt.png" width="4" height="4"/>
        </svg>"##;
        let (_, notes) = read_with(t, None).unwrap();
        let all = notes.join(" | ");
        for word in ["Schnittpfade", "Muster", "<use>", "verschachtelte", "fehlt.png"] {
            assert!(all.contains(word), "{word} fehlt in: {all}");
        }
        // eigene Dateien: nie Hinweise
        let mut d = Document::new(100.0, 100.0, None);
        d.layers[0].objects.push(Object {
            id: 1,
            shape: Shape::Rect {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
                r: 0.0,
            },
            style: Style::default(),
            angle: 0.0,
            group: 0,
            locked: false,
            name: String::new(),
        });
        let (_, notes) = read_with(&write(&d, true), None).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn linked_images_and_lengths() {
        assert_eq!(parse_length("1e3"), Some(1000.0));
        assert_eq!(parse_length("2.5E-1mm"), Some(0.25 * 96.0 / 25.4));
        assert_eq!(parse_length("10 px"), Some(10.0));
        assert_eq!(parse_length("3em"), Some(48.0));
        assert_eq!(percent_decode("mein%20bild.png"), b"mein bild.png".to_vec());
        // Bild neben der SVG-Datei, Name mit Leerzeichen
        let dir = std::env::temp_dir().join(format!("mondrian-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mein bild.png"), b"\x89PNG-Daten").unwrap();
        let t = r#"<svg xmlns="http://www.w3.org/2000/svg" width="1e2" height="50"><image href="mein%20bild.png" x="1" y="2" width="10" height="20"/></svg>"#;
        // verlinkte Nicht-Bilder und Geräte werden nicht eingelesen
        let evil = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><image href="/etc/hostname" width="4" height="4"/><image href="/dev/zero" width="4" height="4"/></svg>"#;
        let (d, notes) = read_with(evil, None).unwrap();
        assert!(d.layers.iter().all(|l| l.objects.is_empty()), "{notes:?}");
        assert_eq!(notes.len(), 2, "{notes:?}");
        let (d, notes) = read_with(t, Some(&dir)).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(d.width, 100.0);
        match &d.layers[0].objects[0].shape {
            Shape::Image { data, .. } => assert_eq!(data.as_slice(), b"\x89PNG-Daten"),
            s => panic!("{s:?}"),
        }
        // ohne Ordner nicht gefunden → Hinweis
        let (_, notes) = read_with(t, None).unwrap();
        assert!(notes.iter().any(|n| n.contains("nicht gefunden")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_writes_text_areas_as_lines() {
        fn fake(shape: &Shape) -> Option<Vec<TextLine>> {
            let Shape::TextArea { text, .. } = shape else {
                return None;
            };
            let mid = text.find(' ').unwrap_or(text.len());
            Some(vec![
                TextLine {
                    start: 0,
                    end: mid,
                    marker: "•".into(),
                    x: 0.0,
                    text_x: 12.0,
                    baseline: 10.0,
                    width: 50.0,
                    justify: true,
                },
                TextLine {
                    start: mid + 1,
                    end: text.len(),
                    marker: String::new(),
                    x: 12.0,
                    text_x: 12.0,
                    baseline: 24.0,
                    width: 30.0,
                    justify: false,
                },
            ])
        }
        let t = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:mondrian="https://mondrian.invalid/ns" width="200" height="100">
          <text x="0" y="10" mondrian:area="100 50" font-family="Sans" font-size="10" fill="#ff0000">Hallo Welt</text></svg>"##;
        let d = read(t).unwrap();
        assert!(matches!(d.layers[0].objects[0].shape, Shape::TextArea { .. }));
        let out = write_export(&d, false, None, fake);
        assert!(out.contains(r#"<text xml:space="preserve" x="0" y="10">•<tspan x="12" textLength="50" lengthAdjust="spacing">Hallo</tspan></text>"#), "{out}");
        assert!(out.contains(r#"<tspan x="12">Welt</tspan>"#), "{out}");
        assert!(!out.contains("inline-size"), "{out}");
        // normales Speichern bleibt beim Textfeld
        assert!(write(&d, false).contains("inline-size"));
        // Teilbereiche mit Formatierung
        let mut rich = RichText::default();
        rich.runs.push(crate::model::TextRun {
            start: 2,
            end: 8,
            style: crate::model::RunStyle {
                bold: Some(true),
                ..Default::default()
            },
        });
        let r = rich_range("Hallo Welt", &rich, &TextFormat::default(), 6, 10);
        assert_eq!(r, r#"<tspan font-weight="bold">We</tspan>lt"#);
        let r = rich_range("Hallo Welt", &rich, &TextFormat::default(), 0, 5);
        assert_eq!(r, r#"Ha<tspan font-weight="bold">llo</tspan>"#);
    }

    #[test]
    fn dashes_round_trip_and_foreign() {
        let mut d = Document::new(200.0, 100.0, None);
        for (i, dash) in crate::model::Dash::ALL.into_iter().enumerate() {
            d.layers[0].objects.push(Object {
                id: i as u64 + 1,
                shape: Shape::Rect {
                    x: 10.0 * i as f64,
                    y: 0.0,
                    w: 5.0,
                    h: 5.0,
                    r: 0.0,
                },
                style: Style {
                    dash,
                    width: 3.0,
                    ..Style::default()
                },
                angle: 0.0,
                group: 0,
                locked: false,
                name: String::new(),
            });
        }
        let text = write(&d, false);
        assert!(text.contains("stroke-dasharray=\"9 6\""), "{text}");
        let back = read(&text).unwrap();
        let got: Vec<_> = back.layers[0].objects.iter().map(|o| o.style.dash).collect();
        assert_eq!(got, crate::model::Dash::ALL.to_vec());
        // fremde Datei ohne Stijl-Angaben: Muster wird zugeordnet
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
            <line x1="0" y1="0" x2="10" y2="0" stroke="black" stroke-width="2" stroke-dasharray="6,4"/>
            <line x1="0" y1="5" x2="10" y2="5" stroke="black" stroke-width="2" stroke-dasharray="0 4" stroke-linecap="round"/>
            </svg>"#;
        let doc = read(svg).unwrap();
        let ds: Vec<_> = doc.layers[0].objects.iter().map(|o| o.style.dash).collect();
        assert_eq!(ds, vec![crate::model::Dash::Dashed, crate::model::Dash::Dotted]);
    }

    #[test]
    fn text_box_round_trip() {
        let mut d = Document::new(200.0, 100.0, None);
        let mut fmt = TextFormat::default();
        fmt.box_fill = Some(Color::rgb(1.0, 1.0, 0.0));
        fmt.box_stroke = Some(Color::rgb(0.0, 0.0, 1.0));
        fmt.box_width = 2.5;
        fmt.box_inset = 6.0;
        d.layers[0].objects.push(Object {
            id: 1,
            shape: Shape::Text {
                xf: Affine::translate(10.0, 50.0),
                text: "Hallo".into(),
                family: "Sans".into(),
                size: 20.0,
                bold: false,
                italic: false,
                fmt,
            },
            style: Style::default(),
            angle: 0.0,
            group: 0,
            locked: false,
            name: String::new(),
        });
        let back = read(&write(&d, false)).unwrap();
        let f = back.layers[0].objects[0].shape.text_format().unwrap();
        assert_eq!((f.box_fill, f.box_stroke), (fmt.box_fill, fmt.box_stroke));
        assert_eq!((f.box_width, f.box_inset), (2.5, 6.0));
        assert!(f.has_box());
        assert!(!TextFormat::default().has_box());
    }

    #[test]
    fn font_weights_round_trip() {
        let t = r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
          <text x="1" y="20" font-family="Inter" font-size="12" font-weight="500">Medium</text>
          <text x="1" y="40" font-family="Inter" font-size="12" font-weight="bold">Fett</text>
          <text x="1" y="60" font-family="Inter" font-size="12" font-weight="800">Extra</text></svg>"#;
        let d = read(t).unwrap();
        let w: Vec<(u16, bool)> = d.layers[0]
            .objects
            .iter()
            .map(|o| match &o.shape {
                Shape::Text { bold, fmt, .. } => (fmt.weight, *bold),
                _ => (0, false),
            })
            .collect();
        assert_eq!(w, vec![(500, false), (0, true), (800, true)]);
        let out = write(&d, false);
        assert!(
            out.contains(r#"font-weight="500""#) && out.contains(r#"font-weight="800""#),
            "{out}"
        );
        let back = read(&out).unwrap();
        assert_eq!(back.layers[0].objects[0].shape.text_format().unwrap().weight, 500);
        // Grundformat: fett umschalten setzt die Stärke, Stärke setzt „fett“
        let mut c = back.layers[0].objects[0].shape.char_base().unwrap();
        assert_eq!((c.weight, c.bold), (500, false));
        c.change(&|f| f.weight = 650);
        assert!(c.bold);
        c.change(&|f| f.bold = false);
        assert_eq!(c.weight, 400);
    }
}
