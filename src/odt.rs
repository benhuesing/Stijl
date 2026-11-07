// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! Öffnen von OpenDocument-Texten (.odt) und -Vorlagen (.ott).
//!
//! Der Text wird zum Fließtext des Notizhefts: Schrift, Größe, Auszeichnungen und Farbe je
//! Zeichen, Ausrichtung, Zeilenabstand und Abstände je Absatz, Aufzählungen und Einzüge,
//! Seitenumbrüche, Seitengröße und Ränder, Spalten, Kopf- und Fußzeile (mit Seitenzahl,
//! Seitenanzahl und Datum). Bilder und Tabellen werden zu Objekten, die beim Öffnen an ihren
//! Absatz gesetzt werden (`Floating`) und den Text wegdrängen. Gespeichert wird nie als ODT.
//!
//! Nicht übernommen (mit Hinweis): Fußnoten stehen am Ende des Textes, Kommentare,
//! Zeichnungen und Textrahmen fehlen, eigene Tabulatoren gelten nicht.

use crate::geom::{Affine, Rect};
use crate::i18n::{tr, trf};
use crate::model::{
    Align, Color, Document, Layer, ListStyle, Object, ParaFmt, RichText, RunStyle, Shape, Style, TextFormat,
    TextFrame, TextRun, BAND_FOOTER, BAND_HEADER, FIELD_DATE, FIELD_PAGE, FIELD_PAGES, PAGE_BREAK,
};
use crate::xml::{Element, XNode};
use std::collections::HashMap;
use std::rc::Rc;

/// Größte Datei im Archiv, die entpackt wird.
const MAX_ENTRY: usize = 200 << 20;

/// Objekt, das an einer Stelle im Text steht (Bild oder Tabelle) und beim Öffnen dorthin
/// gesetzt wird, sobald der Text auf die Seiten verteilt ist.
#[derive(Clone, Debug)]
pub struct Floating {
    /// Stelle im ganzen Fließtext (Byte), an der es steht
    pub offset: usize,
    /// an eine Seite gebunden (0 = erste): dann gilt `x`/`y` ab der Seitenecke
    pub page: Option<usize>,
    /// im Text: Abstand zur Zeile (bzw. zum Textbereich waagerecht); `None`: an der Stelle
    /// des Zeichens (im Text wie ein Buchstabe)
    pub x: Option<f64>,
    pub y: f64,
    /// waagerecht mittig bzw. rechts im Textbereich
    pub halign: Option<Align>,
    pub w: f64,
    pub h: f64,
    pub kind: FloatKind,
}

#[derive(Clone, Debug)]
pub enum FloatKind {
    Image(Rc<Vec<u8>>),
    Table(Box<crate::smart::Table>),
}

/// Ergebnis des Einlesens.
pub struct OdtImport {
    pub doc: Document,
    pub floats: Vec<Floating>,
    pub notes: Vec<String>,
}

/// Sieht die Datei wie ein OpenDocument-Text aus (ZIP mit passendem Typ)?
pub fn looks_like_odt(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
        && unzip(bytes, "mimetype")
            .map(|m| m.starts_with(b"application/vnd.oasis.opendocument.text"))
            .unwrap_or(false)
}

// ───────────────────────────── ZIP ─────────────────────────────

fn u16le(b: &[u8], i: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?) as usize)
}

fn u32le(b: &[u8], i: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?) as usize)
}

/// Datei `name` aus einem ZIP-Archiv (gespeichert oder „deflate“).
pub fn unzip(bytes: &[u8], name: &str) -> Option<Vec<u8>> {
    // Ende des Inhaltsverzeichnisses: von hinten suchen (dahinter höchstens ein Kommentar)
    let min = bytes.len().saturating_sub(22 + 65535);
    let eocd = (min..bytes.len().saturating_sub(21))
        .rev()
        .find(|&i| bytes[i..].starts_with(b"PK\x05\x06"))?;
    let count = u16le(bytes, eocd + 10)?;
    let mut p = u32le(bytes, eocd + 16)?;
    for _ in 0..count {
        if !bytes.get(p..)?.starts_with(b"PK\x01\x02") {
            return None;
        }
        let method = u16le(bytes, p + 10)?;
        let csize = u32le(bytes, p + 20)?;
        let usize_ = u32le(bytes, p + 24)?;
        let nlen = u16le(bytes, p + 28)?;
        let elen = u16le(bytes, p + 30)?;
        let clen = u16le(bytes, p + 32)?;
        let local = u32le(bytes, p + 42)?;
        let n = bytes.get(p + 46..p + 46 + nlen)?;
        if n == name.as_bytes() {
            if usize_ > MAX_ENTRY {
                return None;
            }
            if !bytes.get(local..)?.starts_with(b"PK\x03\x04") {
                return None;
            }
            let start = local + 30 + u16le(bytes, local + 26)? + u16le(bytes, local + 28)?;
            let data = bytes.get(start..start.checked_add(csize)?)?;
            return match method {
                0 => Some(data.to_vec()),
                8 => {
                    let out = crate::pdf::inflate(data);
                    (out.len() <= MAX_ENTRY).then_some(out)
                }
                _ => None,
            };
        }
        p += 46 + nlen + elen + clen;
    }
    None
}

// ───────────────────────────── Maße ─────────────────────────────

/// Länge („2cm“, „12pt“, „0.5in“, „3mm“, „10px“) in Pixeln (96 je Zoll).
fn length(v: &str) -> Option<f64> {
    let v = v.trim();
    let split = v
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(v.len());
    let n: f64 = v[..split].parse().ok()?;
    let px = match v[split..].trim() {
        "cm" => n * 96.0 / 2.54,
        "mm" => n * 96.0 / 25.4,
        "in" | "inch" => n * 96.0,
        "pt" => n * 96.0 / 72.0,
        "pc" => n * 16.0,
        "px" | "" => n,
        _ => return None,
    };
    px.is_finite().then_some(px)
}

/// Prozentangabe („115%“) als Faktor.
fn percent(v: &str) -> Option<f64> {
    let n: f64 = v.trim().strip_suffix('%')?.trim().parse().ok()?;
    n.is_finite().then_some(n / 100.0)
}

// ───────────────────────────── Formate ─────────────────────────────

/// Zeichenformat (alles optional: was fehlt, kommt von weiter oben).
#[derive(Clone, Debug, Default, PartialEq)]
struct CharProps {
    family: Option<String>,
    /// Pixel, oder Prozent der übergeordneten Größe
    size: Option<f64>,
    size_pct: Option<f64>,
    bold: Option<bool>,
    weight: Option<u16>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    color: Option<Color>,
}

impl CharProps {
    fn merge(&mut self, o: &CharProps) {
        if o.family.is_some() {
            self.family.clone_from(&o.family);
        }
        if let Some(s) = o.size {
            self.size = Some(s);
        } else if let Some(p) = o.size_pct {
            self.size = Some(self.size.unwrap_or(16.0) * p);
        }
        macro_rules! take {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f; } )* };
        }
        take!(bold, weight, italic, underline, strike, color);
    }
}

/// Absatzformat.
#[derive(Clone, Debug, Default)]
struct ParaProps {
    align: Option<Align>,
    /// Zeilenhöhe: Faktor (Prozent), feste Höhe in px, oder „normal“/mindestens (automatisch)
    line_pct: Option<f64>,
    line_px: Option<f64>,
    line_auto: Option<bool>,
    before: Option<f64>,
    after: Option<f64>,
    indent: Option<f64>,
    first: Option<f64>,
    break_before: bool,
    break_after: bool,
}

impl ParaProps {
    fn merge(&mut self, o: &ParaProps) {
        macro_rules! take {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f; } )* };
        }
        take!(align, before, after, indent, first);
        if o.line_pct.is_some() || o.line_px.is_some() || o.line_auto.is_some() {
            self.line_pct = o.line_pct;
            self.line_px = o.line_px;
            self.line_auto = o.line_auto;
        }
        // Umbrüche gelten nur für den Absatz selbst (nicht vererbt über Vorlagen hinaus)
        self.break_before = o.break_before;
        self.break_after = o.break_after;
    }
}

#[derive(Clone, Debug, Default)]
struct StyleDef {
    parent: Option<String>,
    chars: CharProps,
    para: ParaProps,
    list_style: Option<String>,
    master: Option<String>,
}

/// Alle Formatvorlagen (Absatz und Zeichen), Listen, Schriften und Seiten.
#[derive(Default)]
struct Styles {
    para: HashMap<String, StyleDef>,
    text: HashMap<String, StyleDef>,
    default_para: StyleDef,
    /// Liste → je Stufe eine Nummerierung?
    lists: HashMap<String, Vec<bool>>,
    /// Rahmenvorlage → waagerechte Lage (mittig, rechts)
    graphic: HashMap<String, Option<Align>>,
    fonts: HashMap<String, String>,
    /// Seitenlayout → (Breite, Höhe, Ränder oben/rechts/unten/links, Spalten, Hintergrund,
    /// Kopf- und Fußzeilenhöhe samt Abstand)
    layouts: HashMap<String, PageLayout>,
    masters: Vec<Master>,
}

#[derive(Clone, Debug)]
struct PageLayout {
    w: f64,
    h: f64,
    margins: [f64; 4],
    columns: u32,
    col_gap: f64,
    background: Option<Color>,
    header_h: f64,
    footer_h: f64,
}

impl Default for PageLayout {
    fn default() -> Self {
        // A4 mit 2 cm Rand
        let cm = 96.0 / 2.54;
        PageLayout {
            w: 21.0 * cm,
            h: 29.7 * cm,
            margins: [2.0 * cm; 4],
            columns: 1,
            col_gap: 0.0,
            background: None,
            header_h: 0.0,
            footer_h: 0.0,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Master {
    name: String,
    layout: String,
    header: Option<Element>,
    footer: Option<Element>,
    /// eigene (leere) Kopf-/Fußzeile der ersten Seite
    first_empty: bool,
}

fn attr<'a>(e: &'a Element, q: &str) -> Option<&'a str> {
    e.attr(q)
        .or_else(|| e.attr_local(q.rsplit(':').next().unwrap_or(q)))
}

fn color_of(v: &str) -> Option<Color> {
    if v == "transparent" {
        return None;
    }
    Color::parse(v)
}

fn read_chars(e: &Element, fonts: &HashMap<String, String>) -> CharProps {
    let mut c = CharProps::default();
    if let Some(f) = e.attr("style:font-name").or_else(|| e.attr("fo:font-family")) {
        let f = fonts.get(f).cloned().unwrap_or_else(|| f.to_string());
        c.family = Some(f.trim_matches(|c| c == '\'' || c == '"').to_string());
    }
    if let Some(s) = e.attr("fo:font-size") {
        if let Some(p) = percent(s) {
            c.size_pct = Some(p);
        } else {
            c.size = length(s).filter(|x| *x > 0.0);
        }
    }
    if let Some(w) = e.attr("fo:font-weight") {
        match w {
            "bold" => c.bold = Some(true),
            "normal" => c.bold = Some(false),
            n => {
                if let Ok(n) = n.parse::<u16>() {
                    c.bold = Some(n >= 600);
                    if n != 400 && n != 700 {
                        c.weight = Some(n);
                    }
                }
            }
        }
    }
    if let Some(s) = e.attr("fo:font-style") {
        c.italic = Some(s == "italic" || s == "oblique");
    }
    if let Some(u) = e.attr("style:text-underline-style") {
        c.underline = Some(u != "none");
    }
    if let Some(u) = e.attr("style:text-line-through-style") {
        c.strike = Some(u != "none");
    }
    if let Some(col) = e.attr("fo:color").and_then(color_of) {
        c.color = Some(col);
    }
    c
}

fn read_para(e: &Element) -> ParaProps {
    let mut p = ParaProps::default();
    if let Some(a) = e.attr("fo:text-align") {
        p.align = match a {
            "center" => Some(Align::Center),
            "end" | "right" => Some(Align::Right),
            "justify" => Some(Align::Justify),
            "start" | "left" => Some(Align::Left),
            _ => None,
        };
    }
    if let Some(l) = e.attr("fo:line-height") {
        if l == "normal" {
            p.line_auto = Some(true);
        } else if let Some(f) = percent(l) {
            p.line_pct = Some(f);
        } else if let Some(px) = length(l) {
            p.line_px = Some(px);
        }
    }
    if e.attr("style:line-height-at-least").is_some() || e.attr("style:line-spacing").is_some() {
        p.line_auto = Some(true);
        p.line_pct = None;
        p.line_px = None;
    }
    p.before = e.attr("fo:margin-top").and_then(length);
    p.after = e.attr("fo:margin-bottom").and_then(length);
    p.indent = e.attr("fo:margin-left").and_then(length);
    p.first = e.attr("fo:text-indent").and_then(length);
    p.break_before = e.attr("fo:break-before") == Some("page");
    p.break_after = e.attr("fo:break-after") == Some("page");
    p
}

impl Styles {
    fn read(&mut self, root: &Element) {
        for e in root.elements() {
            match e.local() {
                "font-face-decls" => {
                    for f in e.elements().filter(|f| f.local() == "font-face") {
                        if let (Some(n), Some(fam)) = (attr(f, "style:name"), f.attr("svg:font-family")) {
                            self.fonts.insert(
                                n.to_string(),
                                fam.trim_matches(|c| c == '\'' || c == '"').to_string(),
                            );
                        }
                    }
                }
                "styles" | "automatic-styles" => self.read_styles(e),
                "master-styles" => {
                    for m in e.elements().filter(|m| m.local() == "master-page") {
                        let first = m
                            .elements()
                            // eigene, aber leere Kopf-/Fußzeile der ersten Seite (mit Inhalt: wie
                            // die übrigen Seiten – eine abweichende gibt es in Stijl nicht)
                            .any(|x| {
                                matches!(x.local(), "header-first" | "footer-first")
                                    && (x.attr("style:display") == Some("false")
                                        || x.text().trim().is_empty())
                            });
                        self.masters.push(Master {
                            name: attr(m, "style:name").unwrap_or("").to_string(),
                            layout: attr(m, "style:page-layout-name").unwrap_or("").to_string(),
                            header: m
                                .elements()
                                .find(|x| x.local() == "header" && x.attr("style:display") != Some("false"))
                                .cloned(),
                            footer: m
                                .elements()
                                .find(|x| x.local() == "footer" && x.attr("style:display") != Some("false"))
                                .cloned(),
                            first_empty: first,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    fn read_styles(&mut self, root: &Element) {
        for e in root.elements() {
            match e.local() {
                "style" | "default-style" => {
                    let mut d = StyleDef {
                        parent: e.attr("style:parent-style-name").map(String::from),
                        list_style: e.attr("style:list-style-name").map(String::from),
                        master: e
                            .attr("style:master-page-name")
                            .filter(|m| !m.is_empty())
                            .map(String::from),
                        ..StyleDef::default()
                    };
                    for p in e.elements() {
                        match p.local() {
                            "text-properties" => d.chars = read_chars(p, &self.fonts),
                            "paragraph-properties" => d.para = read_para(p),
                            _ => {}
                        }
                    }
                    let fam = e.attr("style:family").unwrap_or("");
                    if fam == "graphic" {
                        if let Some(name) = attr(e, "style:name") {
                            let pos = e
                                .elements()
                                .find(|p| p.local() == "graphic-properties")
                                .and_then(|p| p.attr("style:horizontal-pos"));
                            self.graphic.insert(
                                name.to_string(),
                                match pos {
                                    Some("center") => Some(Align::Center),
                                    Some("right") | Some("outside") => Some(Align::Right),
                                    _ => None,
                                },
                            );
                        }
                        continue;
                    }
                    if e.local() == "default-style" {
                        if fam == "paragraph" {
                            self.default_para = d;
                        }
                        continue;
                    }
                    let Some(name) = attr(e, "style:name") else {
                        continue;
                    };
                    match fam {
                        "paragraph" => {
                            self.para.insert(name.to_string(), d);
                        }
                        "text" => {
                            self.text.insert(name.to_string(), d);
                        }
                        _ => {}
                    }
                }
                "list-style" => {
                    let Some(name) = attr(e, "style:name") else {
                        continue;
                    };
                    let mut levels = vec![false; 10];
                    for l in e.elements() {
                        let lvl: usize = attr(l, "text:level").and_then(|v| v.parse().ok()).unwrap_or(1);
                        if (1..=10).contains(&lvl) {
                            levels[lvl - 1] = l.local() == "list-level-style-number";
                        }
                    }
                    self.lists.insert(name.to_string(), levels);
                }
                "page-layout" => {
                    let Some(name) = attr(e, "style:name") else {
                        continue;
                    };
                    let mut pl = PageLayout::default();
                    for p in e.elements() {
                        match p.local() {
                            "page-layout-properties" => {
                                if let Some(w) = p.attr("fo:page-width").and_then(length) {
                                    pl.w = w;
                                }
                                if let Some(h) = p.attr("fo:page-height").and_then(length) {
                                    pl.h = h;
                                }
                                for (i, k) in [
                                    "fo:margin-top",
                                    "fo:margin-right",
                                    "fo:margin-bottom",
                                    "fo:margin-left",
                                ]
                                .iter()
                                .enumerate()
                                {
                                    if let Some(v) = p.attr(k).and_then(length) {
                                        pl.margins[i] = v.max(0.0);
                                    }
                                }
                                pl.background = p.attr("fo:background-color").and_then(color_of);
                                if let Some(c) = p.elements().find(|c| c.local() == "columns") {
                                    pl.columns = attr(c, "fo:column-count")
                                        .and_then(|v| v.parse().ok())
                                        .unwrap_or(1)
                                        .clamp(1, crate::model::MAX_COLUMNS);
                                    pl.col_gap = attr(c, "fo:column-gap").and_then(length).unwrap_or(0.0);
                                }
                            }
                            "header-style" | "footer-style" => {
                                if let Some(hp) =
                                    p.elements().find(|x| x.local() == "header-footer-properties")
                                {
                                    let h = hp
                                        .attr("fo:min-height")
                                        .or_else(|| hp.attr("svg:height"))
                                        .and_then(length)
                                        .unwrap_or(0.0);
                                    let gap = hp
                                        .attr(if p.local() == "header-style" {
                                            "fo:margin-bottom"
                                        } else {
                                            "fo:margin-top"
                                        })
                                        .and_then(length)
                                        .unwrap_or(0.0);
                                    if p.local() == "header-style" {
                                        pl.header_h = h + gap;
                                    } else {
                                        pl.footer_h = h + gap;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    self.layouts.insert(name.to_string(), pl);
                }
                _ => {}
            }
        }
    }

    /// Wirksames Absatzformat einer Vorlage (mit allen übergeordneten).
    fn para_style(&self, name: Option<&str>) -> StyleDef {
        let mut chain = Vec::new();
        let mut cur = name.map(String::from);
        while let Some(n) = cur {
            if chain.len() > 20 || chain.contains(&n) {
                break;
            }
            let next = self.para.get(&n).and_then(|d| d.parent.clone());
            chain.push(n);
            cur = next;
        }
        let mut out = self.default_para.clone();
        out.para.break_before = false;
        out.para.break_after = false;
        let mut list = None;
        let mut master = None;
        for n in chain.iter().rev() {
            if let Some(d) = self.para.get(n) {
                out.chars.merge(&d.chars);
                out.para.merge(&d.para);
                if d.list_style.is_some() {
                    list.clone_from(&d.list_style);
                }
                if d.master.is_some() {
                    master.clone_from(&d.master);
                }
            }
        }
        // Seitenumbrüche: nur die eigene Vorlage (bzw. ihre automatische Kopie)
        let own = name.and_then(|n| self.para.get(n));
        out.para.break_before = own.is_some_and(|d| d.para.break_before);
        out.para.break_after = own.is_some_and(|d| d.para.break_after);
        out.list_style = list;
        out.master = master;
        out
    }

    /// Zeichenformat einer Zeichenvorlage (mit übergeordneten).
    fn text_style(&self, name: &str) -> CharProps {
        let mut chain = Vec::new();
        let mut cur = Some(name.to_string());
        while let Some(n) = cur {
            if chain.len() > 20 || chain.contains(&n) {
                break;
            }
            let next = self.text.get(&n).and_then(|d| d.parent.clone());
            chain.push(n);
            cur = next;
        }
        let mut out = CharProps::default();
        for n in chain.iter().rev() {
            if let Some(d) = self.text.get(n) {
                out.merge(&d.chars);
            }
        }
        out
    }
}

// ───────────────────────────── Text ─────────────────────────────

/// Ein Absatz beim Einlesen.
#[derive(Clone, Debug, Default)]
struct Para {
    text: String,
    /// (Anfang, Ende, wirksames Zeichenformat) im Absatz
    runs: Vec<(usize, usize, CharProps)>,
    props: ParaProps,
    base: CharProps,
    list: ListStyle,
    level: u8,
}

/// Sammelt Absätze, Bilder und Tabellen.
struct Builder<'a> {
    st: &'a Styles,
    zip: &'a [u8],
    paras: Vec<Para>,
    floats: Vec<(usize, Floating)>,
    notes: Vec<String>,
    footnotes: Vec<String>,
    /// erster Absatz: welche Musterseite
    first_master: Option<String>,
    left_out: HashMap<&'static str, usize>,
    /// in einer Kopf-/Fußzeile: Felder als Platzhalter
    fields: bool,
}

impl Builder<'_> {
    fn skip(&mut self, what: &'static str) {
        *self.left_out.entry(what).or_insert(0) += 1;
    }

    /// Blockelemente (Absätze, Überschriften, Listen, Tabellen, Abschnitte).
    fn blocks(&mut self, e: &Element, list: Option<(&str, u8)>) {
        for c in e.elements() {
            match c.local() {
                "p" | "h" => self.paragraph(c, list),
                "list" => {
                    let name = c
                        .attr("text:style-name")
                        .map(String::from)
                        .or_else(|| list.map(|l| l.0.to_string()));
                    let level = list.map(|l| l.1 + 1).unwrap_or(0);
                    for item in c.elements() {
                        if matches!(item.local(), "list-item" | "list-header") {
                            self.blocks(item, Some((name.as_deref().unwrap_or(""), level)));
                        }
                    }
                }
                "section" | "index-body" | "table-of-content" | "alphabetical-index"
                | "illustration-index" | "table-index" | "bibliography" | "user-index" | "object-index" => {
                    if c.local() != "section" && c.local() != "index-body" {
                        // Verzeichnisse: nur ihr Inhalt (ohne Kopf)
                        if let Some(b) = c.elements().find(|x| x.local() == "index-body") {
                            self.blocks(b, list);
                        }
                    } else {
                        self.blocks(c, list);
                    }
                }
                "table" => self.table(c),
                "tracked-changes" | "sequence-decls" | "variable-decls" | "user-field-decls" | "forms"
                | "soft-page-break" => {}
                _ => {}
            }
        }
    }

    fn paragraph(&mut self, e: &Element, list: Option<(&str, u8)>) {
        let sname = e.attr("text:style-name");
        let def = self.st.para_style(sname);
        if self.paras.is_empty() && self.first_master.is_none() {
            self.first_master.clone_from(&def.master);
        }
        let mut p = Para {
            props: def.para.clone(),
            base: def.chars.clone(),
            ..Para::default()
        };
        // Aufzählung: Liste des Elements, sonst der Absatzvorlage
        if let Some((lname, level)) = list {
            let lname = if lname.is_empty() {
                def.list_style.as_deref().unwrap_or("")
            } else {
                lname
            };
            let numbered = self
                .st
                .lists
                .get(lname)
                .and_then(|l| l.get(level as usize).copied())
                .unwrap_or(false);
            p.list = if numbered {
                ListStyle::Number
            } else {
                ListStyle::Dot
            };
            p.level = level.min(crate::model::MAX_INDENT);
        }
        let base = def.chars.clone();
        self.inline(e, &base, &mut p);
        // Leerraum am Anfang und Ende des Absatzes zählt nicht
        trim_para(&mut p);
        self.paras.push(p);
    }

    /// Inhalt eines Absatzes (Text, Teile mit eigener Vorlage, Leerzeichen, Felder …).
    fn inline(&mut self, e: &Element, fmt: &CharProps, p: &mut Para) {
        for c in &e.children {
            match c {
                XNode::Text(t) => push_text(p, t, fmt),
                XNode::Elem(x) => match x.local() {
                    "span" | "a" | "meta" | "ruby" | "ruby-base" => {
                        let mut f = fmt.clone();
                        if let Some(n) = x.attr("text:style-name") {
                            f.merge(&self.st.text_style(n));
                        }
                        self.inline(x, &f, p);
                    }
                    "s" => {
                        let n: usize = x.attr("text:c").and_then(|v| v.parse().ok()).unwrap_or(1);
                        push_raw(p, &" ".repeat(n.min(1000)), fmt);
                    }
                    "tab" => push_raw(p, "\t", fmt),
                    "line-break" => push_raw(p, "\u{2028}", fmt),
                    "page-number" if self.fields => push_raw(p, &FIELD_PAGE.to_string(), fmt),
                    "page-count" if self.fields => push_raw(p, &FIELD_PAGES.to_string(), fmt),
                    "date" if self.fields => push_raw(p, &FIELD_DATE.to_string(), fmt),
                    "note" => {
                        // Fußnote: Nummer im Text, der Text am Ende
                        let cit = x
                            .elements()
                            .find(|n| n.local() == "note-citation")
                            .map(|n| n.text())
                            .unwrap_or_default();
                        let body = x
                            .elements()
                            .find(|n| n.local() == "note-body")
                            .map(|b| {
                                b.elements()
                                    .map(|q| collapse(&q.text()))
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            })
                            .unwrap_or_default();
                        let n = if cit.trim().is_empty() {
                            (self.footnotes.len() + 1).to_string()
                        } else {
                            cit.trim().to_string()
                        };
                        push_raw(p, &format!("[{n}]"), fmt);
                        self.footnotes.push(format!("[{n}] {}", body.trim()));
                    }
                    "frame" => self.frame(x, p),
                    "annotation" | "annotation-end" => self.skip("comments"),
                    "bookmark"
                    | "bookmark-start"
                    | "bookmark-end"
                    | "reference-mark"
                    | "reference-mark-start"
                    | "reference-mark-end"
                    | "soft-page-break"
                    | "change-start"
                    | "change-end"
                    | "change"
                    | "toc-mark"
                    | "alphabetical-index-mark" => {}
                    "custom-shape" | "rect" | "line" | "ellipse" | "polygon" | "polyline" | "path"
                    | "connector" | "g" => self.skip("drawings"),
                    // Felder und alles Übrige: ihr Text, wie er im Dokument steht
                    _ => self.inline(x, fmt, p),
                },
            }
        }
    }

    /// Bild (oder Textrahmen) im Absatz.
    fn frame(&mut self, f: &Element, p: &mut Para) {
        let w = f.attr("svg:width").and_then(length).unwrap_or(0.0);
        let h = f.attr("svg:height").and_then(length).unwrap_or(0.0);
        let anchor = f.attr("text:anchor-type").unwrap_or("paragraph");
        let x = f.attr("svg:x").and_then(length);
        let y = f.attr("svg:y").and_then(length).unwrap_or(0.0);
        let page = (anchor == "page").then(|| {
            f.attr("text:anchor-page-number")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(1)
                .saturating_sub(1)
        });
        let halign = f
            .attr("draw:style-name")
            .and_then(|n| self.st.graphic.get(n))
            .copied()
            .flatten();
        if let Some(img) = f.elements().find(|e| e.local() == "image") {
            let href = img.attr("xlink:href").unwrap_or("");
            let data = unzip(self.zip, href.trim_start_matches("./"));
            match data.filter(|d| crate::svg::looks_like_image(d)) {
                Some(d) if w > 0.0 && h > 0.0 => {
                    let at = self.offset_here(p);
                    self.floats.push((
                        at,
                        Floating {
                            offset: 0,
                            page,
                            x: if anchor == "as-char" || anchor == "char" {
                                None
                            } else {
                                x
                            },
                            y: if anchor == "as-char" { 0.0 } else { y },
                            halign,
                            w,
                            h,
                            kind: FloatKind::Image(Rc::new(d)),
                        },
                    ));
                }
                _ => self.skip("pictures"),
            }
        } else if f.elements().any(|e| e.local() == "text-box") {
            self.skip("text frames");
        } else {
            self.skip("drawings");
        }
    }

    /// Stelle im Text, an der das gerade gelesene steht (Absatz `p`, noch nicht abgelegt):
    /// vorläufig als (Absatznummer, Byte im Absatz) verschlüsselt.
    fn offset_here(&self, p: &Para) -> usize {
        (self.paras.len() << 24) | p.text.len().min((1 << 24) - 1)
    }

    fn table(&mut self, t: &Element) {
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut header = false;
        let collect = |rows: &mut Vec<Vec<String>>, r: &Element| {
            let mut row = Vec::new();
            for c in r.elements() {
                match c.local() {
                    "table-cell" => {
                        let rep: usize = c
                            .attr("table:number-columns-repeated")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(1)
                            .min(64);
                        let text = c
                            .elements()
                            .filter(|q| matches!(q.local(), "p" | "h" | "list"))
                            .map(|q| collapse(&q.text()).trim().to_string())
                            .collect::<Vec<_>>()
                            .join("\n");
                        for _ in 0..rep {
                            row.push(text.clone());
                        }
                    }
                    "covered-table-cell" => row.push(String::new()),
                    _ => {}
                }
            }
            rows.push(row);
        };
        for c in t.elements() {
            match c.local() {
                "table-header-rows" => {
                    header = true;
                    for r in c.elements().filter(|r| r.local() == "table-row") {
                        collect(&mut rows, r);
                    }
                }
                "table-row" => collect(&mut rows, c),
                "table-rows" | "table-row-group" => {
                    for r in c.elements().filter(|r| r.local() == "table-row") {
                        collect(&mut rows, r);
                    }
                }
                _ => {}
            }
        }
        if rows.is_empty() || rows.iter().all(|r| r.is_empty()) {
            return;
        }
        let mut table = crate::smart::Table::with_cells(rows);
        table.header = header;
        table.normalize();
        // Platz für die Tabelle: ein leerer Absatz, an dessen Stelle sie steht
        let p = Para::default();
        let at = self.offset_here(&p);
        let n = table.n_rows() as f64;
        self.floats.push((
            at,
            Floating {
                offset: 0,
                page: None,
                x: Some(0.0),
                y: 0.0,
                halign: None,
                // Breite: der ganze Textbereich (wird beim Setzen angepasst); Höhe je Zeile
                w: 0.0,
                h: n,
                kind: FloatKind::Table(Box::new(table)),
            },
        ));
        self.paras.push(p);
    }
}

/// Leerraum wie in ODF: Tabulatoren und Umbrüche sind Leerzeichen, mehrere zählen als eins.
fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(c);
            space = false;
        }
    }
    out
}

fn push_text(p: &mut Para, t: &str, fmt: &CharProps) {
    let mut s = collapse(t);
    // nach einem Leerzeichen kein weiteres
    if s.starts_with(' ') && (p.text.is_empty() || p.text.ends_with(' ')) {
        s.remove(0);
    }
    push_raw(p, &s, fmt);
}

fn push_raw(p: &mut Para, s: &str, fmt: &CharProps) {
    if s.is_empty() {
        return;
    }
    let a = p.text.len();
    p.text.push_str(s);
    match p.runs.last_mut() {
        Some(r) if r.1 == a && r.2 == *fmt => r.1 = p.text.len(),
        _ => p.runs.push((a, p.text.len(), fmt.clone())),
    }
}

fn trim_para(p: &mut Para) {
    let end = p.text.trim_end_matches(' ').len();
    p.text.truncate(end);
    for r in &mut p.runs {
        r.0 = r.0.min(end);
        r.1 = r.1.min(end);
    }
    p.runs.retain(|r| r.1 > r.0);
}

/// Grundformat (Schrift, Größe, Farbe) als Zeichenformat.
fn base_char(c: &CharProps) -> (String, f64, bool, bool, Option<Color>) {
    (
        c.family.clone().unwrap_or_else(|| "Liberation Serif".into()),
        c.size.unwrap_or(16.0),
        c.bold.unwrap_or(false),
        c.italic.unwrap_or(false),
        c.color,
    )
}

/// Absätze zu Text, Formatierung und Absatzformaten zusammenfügen (`base`: Grundschrift
/// des Feldes, `size`: Grundgröße).
fn assemble(paras: &[Para], base: &CharProps, size: f64) -> (String, RichText, Vec<usize>) {
    let mut text = String::new();
    let mut rich = RichText::default();
    let mut starts = Vec::new();
    let (bfam, bsize, bbold, bital, bcol) = base_char(base);
    for (i, p) in paras.iter().enumerate() {
        if i > 0 {
            text.push('\n');
        }
        starts.push(text.len());
        let off = text.len();
        text.push_str(&p.text);
        for (a, b, f) in &p.runs {
            let fam = f.family.clone().unwrap_or_else(|| bfam.clone());
            let sz = f.size.unwrap_or(bsize);
            let style = RunStyle {
                family: (fam != bfam).then_some(fam),
                size: ((sz - bsize).abs() > 0.01).then_some(sz),
                bold: (f.bold.unwrap_or(false) != bbold).then(|| f.bold.unwrap_or(false)),
                weight: f.weight,
                italic: (f.italic.unwrap_or(false) != bital).then(|| f.italic.unwrap_or(false)),
                underline: f.underline.filter(|u| *u),
                strike: f.strike.filter(|s| *s),
                color: f.color.filter(|c| Some(*c) != bcol),
            };
            if !style.is_empty() {
                rich.runs.push(TextRun {
                    start: off + a,
                    end: off + b,
                    style,
                });
            }
        }
        // Absatzformat: Abstände als Vielfache der Grundgröße
        let pp = &p.props;
        let psize = p.base.size.unwrap_or(bsize);
        let line = if pp.line_auto == Some(true) {
            Some(0.0)
        } else if let Some(f) = pp.line_pct {
            // Prozent der Schrift des Absatzes; bei abweichender Größe lieber automatisch
            if (f - 1.0).abs() < 0.01 || (psize - bsize).abs() > 0.5 {
                Some(0.0)
            } else {
                Some(f * crate::model::AUTO_LINE)
            }
        } else if let Some(px) = pp.line_px {
            (px >= psize).then_some(px / bsize)
        } else {
            None
        };
        let indent = pp.indent.unwrap_or(0.0);
        let step = size * 1.5;
        let mut level = p.level;
        if p.list == ListStyle::None && indent > step * 0.5 {
            level = ((indent / step).round() as u8).min(crate::model::MAX_INDENT);
        }
        rich.paras.push(ParaFmt {
            list: p.list,
            indent: level,
            align: pp.align,
            line,
            before: pp.before.map(|v| v / size),
            after: pp.after.map(|v| v / size),
        });
    }
    (text, rich, starts)
}

/// OpenDocument-Text einlesen. `known`: ist eine Schrift installiert?
pub fn import(bytes: &[u8], known: &dyn Fn(&str) -> bool) -> Result<OdtImport, String> {
    if !looks_like_odt(bytes) {
        return Err(tr("This is not an OpenDocument text").into());
    }
    let read = |name: &str| -> Option<Element> {
        let raw = unzip(bytes, name)?;
        crate::xml::parse(&String::from_utf8_lossy(&raw)).ok()
    };
    let content = read("content.xml").ok_or_else(|| tr("The document has no content").to_string())?;
    let mut st = Styles::default();
    if let Some(s) = read("styles.xml") {
        st.read(&s);
    }
    st.read(&content);

    let mut b = Builder {
        st: &st,
        zip: bytes,
        paras: Vec::new(),
        floats: Vec::new(),
        notes: Vec::new(),
        footnotes: Vec::new(),
        first_master: None,
        left_out: HashMap::new(),
        fields: false,
    };
    let body = content
        .elements()
        .find(|e| e.local() == "body")
        .and_then(|b| b.elements().find(|e| e.local() == "text"))
        .ok_or_else(|| tr("The document has no content").to_string())?;
    b.blocks(body, None);
    // Fußnoten am Ende
    if !b.footnotes.is_empty() {
        let notes = std::mem::take(&mut b.footnotes);
        let n = notes.len();
        for f in notes {
            let mut p = Para::default();
            push_raw(&mut p, &f, &CharProps::default());
            p.props.before = Some(4.0);
            b.paras.push(p);
        }
        b.notes
            .push(trf("{} footnotes are at the end of the text", &[&n]));
    }
    if b.paras.is_empty() {
        b.paras.push(Para::default());
    }
    // Seitenumbrüche: vor bzw. nach Absätzen
    let mut breaks = vec![false; b.paras.len()];
    for i in 0..b.paras.len() {
        if b.paras[i].props.break_before && i > 0 {
            breaks[i - 1] = true;
        }
        if b.paras[i].props.break_after && i + 1 < b.paras.len() {
            breaks[i] = true;
        }
    }
    for (i, br) in breaks.iter().enumerate() {
        if *br {
            let p = &mut b.paras[i];
            let at = p.text.len();
            p.text.push(PAGE_BREAK);
            if let Some(r) = p.runs.last_mut().filter(|r| r.1 == at) {
                r.1 = p.text.len();
            }
        }
    }

    // Seite: Musterseite des ersten Absatzes bzw. die Standardseite – die mit Kopf-/Fußzeile
    let main = st
        .masters
        .iter()
        .find(|m| m.name == "Standard")
        .or_else(|| st.masters.first())
        .cloned()
        .unwrap_or_default();
    let first = b
        .first_master
        .as_ref()
        .and_then(|n| st.masters.iter().find(|m| &m.name == n))
        .cloned();
    let with_bands = if main.header.is_some() || main.footer.is_some() {
        Some(main.clone())
    } else {
        st.masters
            .iter()
            .find(|m| m.header.is_some() || m.footer.is_some())
            .cloned()
    };
    // Seitengröße und Ränder: die der Seiten mit Kopf-/Fußzeile (eine abweichende Titelseite
    // gibt es in Stijl nur ohne Kopf-/Fußzeile), sonst die der ersten Seite
    let page_master = with_bands
        .clone()
        .or_else(|| first.clone())
        .unwrap_or_else(|| main.clone());
    let layout = st.layouts.get(&page_master.layout).cloned().unwrap_or_default();
    // Titelseite ohne Kopf-/Fußzeile: eigene Musterseite ohne sie, oder leere erste
    let skip_first = with_bands.as_ref().is_some_and(|wb| {
        wb.first_empty
            || first
                .as_ref()
                .is_some_and(|f| f.header.is_none() && f.footer.is_none() && f.name != wb.name)
    });

    // Grundschrift: die Schrift, in der der meiste Text steht (Überschriften zählen nicht)
    let base = {
        let mut count: Vec<(CharProps, usize)> = Vec::new();
        for p in &b.paras {
            let mut k = p.base.clone();
            k.bold = None;
            k.italic = None;
            k.weight = None;
            k.underline = None;
            k.strike = None;
            match count.iter_mut().find(|c| c.0 == k) {
                Some(c) => c.1 += p.text.len(),
                None => count.push((k, p.text.len())),
            }
        }
        count
            .into_iter()
            .max_by_key(|c| c.1)
            .map(|c| c.0)
            .unwrap_or_else(|| st.default_para.chars.clone())
    };
    let (family, size, bold, italic, color) = base_char(&base);
    let (text, rich, starts) = assemble(&b.paras, &base, size);

    let mut doc = Document::new(layout.w, layout.h, None);
    doc.unit = "mm".into();
    doc.view = crate::model::ViewMode::Notebook;
    let bands = with_bands.as_ref().map(|m| layout_of(&st, m, &layout));
    let bl = bands.clone().unwrap_or(layout.clone());
    let mut margins = layout.margins;
    // Ränder bis zum Text: Kopf-/Fußzeile liegen im Rand
    if with_bands.as_ref().is_some_and(|m| m.header.is_some()) {
        margins[0] += bl.header_h;
    }
    if with_bands.as_ref().is_some_and(|m| m.footer.is_some()) {
        margins[2] += bl.footer_h;
    }
    doc.margins = margins;
    doc.bands_skip_first = skip_first;
    // Datum des letzten Speicherns (für das Datumsfeld)
    if let Some(m) = read("meta.xml") {
        fn find(e: &Element) -> Option<String> {
            for c in e.elements() {
                if c.name == "dc:date" {
                    return Some(c.text());
                }
                if let Some(d) = find(c) {
                    return Some(d);
                }
            }
            None
        }
        if let Some(d) =
            find(&m).filter(|d| d.len() >= 10 && d.as_bytes()[4] == b'-' && d.as_bytes()[7] == b'-')
        {
            doc.date = d[..10].to_string();
        }
    }
    if let Some(bg) = layout.background {
        doc.background = Some(bg);
    }
    let first_indent = b
        .paras
        .iter()
        .filter(|p| p.list == ListStyle::None && !p.text.is_empty())
        .filter_map(|p| p.props.first)
        .find(|f| *f > 0.0)
        .unwrap_or(0.0);
    let fmt = TextFormat {
        line_auto: true,
        para: 0.0,
        columns: layout.columns,
        col_gap: layout.col_gap,
        first: first_indent,
        ..TextFormat::default()
    };
    let at = Rect {
        x0: margins[3],
        y0: margins[0],
        x1: (layout.w - margins[1]).max(margins[3] + 20.0),
        y1: (layout.h - margins[2]).max(margins[0] + 20.0),
    };
    let body_id = doc.alloc_id();
    let fill = Some(color.unwrap_or(Color::rgb(0.0, 0.0, 0.0)));
    let mut body_layer = Layer {
        id: doc.alloc_id(),
        name: tr("Body Text").into(),
        visible: true,
        locked: true,
        color: 1,
        objects: vec![text_object(
            body_id,
            tr("Body Text"),
            at,
            text,
            rich,
            (&family, size, bold, italic),
            fmt,
            TextFrame {
                body: true,
                ..TextFrame::default()
            },
            fill,
        )],
    };
    // Kopf- und Fußzeile (ein Feld auf der ersten Seite; der Editor setzt sie auf alle)
    if let Some(m) = &with_bands {
        for (kind, el) in [(BAND_HEADER, &m.header), (BAND_FOOTER, &m.footer)] {
            let Some(el) = el else { continue };
            let mut hb = Builder {
                st: &st,
                zip: bytes,
                paras: Vec::new(),
                floats: Vec::new(),
                notes: Vec::new(),
                footnotes: Vec::new(),
                first_master: None,
                left_out: HashMap::new(),
                fields: true,
            };
            hb.blocks(el, None);
            if hb.paras.iter().all(|p| p.text.trim().is_empty()) {
                continue;
            }
            let hbase = hb.paras.first().map(|p| p.base.clone()).unwrap_or_default();
            let (hfam, hsize, hbold, hital, hcol) = base_char(&hbase);
            let (t, r, _) = assemble(&hb.paras, &hbase, hsize);
            let bx = if kind == BAND_HEADER {
                Rect {
                    x0: at.x0,
                    y0: layout.margins[0],
                    x1: at.x1,
                    y1: margins[0],
                }
            } else {
                Rect {
                    x0: at.x0,
                    y0: layout.h - margins[2],
                    x1: at.x1,
                    y1: layout.h - layout.margins[2],
                }
            };
            let id = doc.alloc_id();
            body_layer.objects.push(text_object(
                id,
                if kind == BAND_HEADER {
                    tr("Header")
                } else {
                    tr("Footer")
                },
                bx,
                t,
                r,
                (&hfam, hsize, hbold, hital),
                TextFormat {
                    line_auto: true,
                    valign: crate::model::VAlign::Middle,
                    ..TextFormat::default()
                },
                TextFrame {
                    band: kind,
                    no_flow: true,
                    ..TextFrame::default()
                },
                Some(hcol.or(color).unwrap_or(Color::rgb(0.0, 0.0, 0.0))),
            ));
            if hb.paras.iter().any(|p| p.text.contains('\t')) {
                b.notes
                    .push(tr("Tab stops in the header or footer are not kept").into());
            }
        }
    }
    doc.layers.insert(0, body_layer);

    // Stellen der Bilder und Tabellen im ganzen Text
    let floats = b
        .floats
        .iter()
        .map(|(code, f)| {
            let (pi, inner) = (code >> 24, code & ((1 << 24) - 1));
            let mut f = f.clone();
            f.offset = starts.get(pi).copied().unwrap_or(0) + inner;
            f
        })
        .collect::<Vec<_>>();

    // fehlende Schriften
    let mut fams: Vec<String> = Vec::new();
    for f in std::iter::once(family.clone()).chain(
        b.paras
            .iter()
            .flat_map(|p| p.runs.iter().filter_map(|r| r.2.family.clone())),
    ) {
        if !known(&f) && !fams.contains(&f) {
            fams.push(f);
        }
    }
    if !fams.is_empty() {
        b.notes.push(trf("Fonts not installed: {}", &[&fams.join(", ")]));
    }
    for (what, n) in &b.left_out {
        let msg = match *what {
            "comments" => trf("{} comments left out", &[n]),
            "drawings" => trf("{} drawings left out", &[n]),
            "text frames" => trf("{} text frames left out", &[n]),
            _ => trf("{} pictures left out (format not supported)", &[n]),
        };
        b.notes.push(msg);
    }
    if !floats.is_empty() {
        b.notes.push(
            tr("Pictures and tables were placed next to their paragraph – please check their position")
                .into(),
        );
    }
    Ok(OdtImport {
        doc,
        floats,
        notes: b.notes,
    })
}

/// Seitenlayout einer Musterseite.
fn layout_of(st: &Styles, m: &Master, fallback: &PageLayout) -> PageLayout {
    st.layouts
        .get(&m.layout)
        .cloned()
        .unwrap_or_else(|| fallback.clone())
}

fn text_object(
    id: u64,
    name: &str,
    at: Rect,
    text: String,
    rich: RichText,
    font: (&str, f64, bool, bool),
    fmt: TextFormat,
    frame: TextFrame,
    fill: Option<Color>,
) -> Object {
    Object {
        group: 0,
        sub: Vec::new(),
        mask: false,
        locked: false,
        smart: None,
        name: name.to_string(),
        id,
        shape: Shape::TextArea {
            xf: Affine::translate(at.x0, at.y0),
            w: at.width().max(1.0),
            h: at.height().max(1.0),
            text,
            family: font.0.to_string(),
            size: font.1,
            bold: font.2,
            italic: font.3,
            fmt,
            frame,
            rich,
        },
        style: Style {
            fill,
            stroke: None,
            ..Style::default()
        },
        angle: 0.0,
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// ZIP ohne Kompression (für Tests).
    pub fn zip_stored(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let off = out.len() as u32;
            let crc = 0u32;
            let mut h = Vec::new();
            h.extend_from_slice(b"PK\x03\x04");
            h.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            h.extend_from_slice(&crc.to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            h.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&h);
            out.extend_from_slice(data);
            let mut c = Vec::new();
            c.extend_from_slice(b"PK\x01\x02");
            c.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            c.extend_from_slice(&crc.to_le_bytes());
            c.extend_from_slice(&(data.len() as u32).to_le_bytes());
            c.extend_from_slice(&(data.len() as u32).to_le_bytes());
            c.extend_from_slice(&(name.len() as u16).to_le_bytes());
            c.extend_from_slice(&[0; 12]);
            c.extend_from_slice(&off.to_le_bytes());
            c.extend_from_slice(name.as_bytes());
            central.extend_from_slice(&c);
        }
        let cd = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    const STYLES: &str = r##"<office:document-styles xmlns:office="o" xmlns:style="s" xmlns:fo="f" xmlns:text="t" xmlns:svg="v">
<office:font-face-decls><style:font-face style:name="Liberation Serif" svg:font-family="'Liberation Serif'"/></office:font-face-decls>
<office:styles>
<style:default-style style:family="paragraph"><style:text-properties style:font-name="Liberation Serif" fo:font-size="12pt"/></style:default-style>
<style:style style:name="Standard" style:family="paragraph"/>
<style:style style:name="Text_20_body" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties fo:margin-top="0cm" fo:margin-bottom="0.247cm" fo:line-height="115%"/></style:style>
<style:style style:name="Heading_20_1" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties fo:text-align="center" fo:margin-top="0.423cm"/><style:text-properties fo:font-size="130%" fo:font-weight="bold"/></style:style>
<style:style style:name="Strong" style:family="text"><style:text-properties fo:font-weight="bold"/></style:style>
<text:list-style style:name="Numbering"><text:list-level-style-number text:level="1"/><text:list-level-style-bullet text:level="2"/></text:list-style>
</office:styles>
<office:automatic-styles><style:page-layout style:name="pm1"><style:page-layout-properties fo:page-width="21cm" fo:page-height="29.7cm" fo:margin-top="2cm" fo:margin-bottom="2cm" fo:margin-left="2.5cm" fo:margin-right="2cm"/><style:header-style><style:header-footer-properties fo:min-height="0.5cm" fo:margin-bottom="0.5cm"/></style:header-style></style:page-layout></office:automatic-styles>
<office:master-styles><style:master-page style:name="Standard" style:page-layout-name="pm1"><style:header><text:p>Seite <text:page-number>1</text:page-number> von <text:page-count>3</text:page-count></text:p></style:header></style:master-page></office:master-styles>
</office:document-styles>"##;

    const CONTENT: &str = r##"<office:document-content xmlns:office="o" xmlns:style="s" xmlns:fo="f" xmlns:text="t" xmlns:table="b" xmlns:draw="d" xmlns:svg="v" xmlns:xlink="x">
<office:automatic-styles><style:style style:name="P1" style:family="paragraph" style:parent-style-name="Text_20_body"><style:paragraph-properties fo:break-before="page"/></style:style>
<style:style style:name="T1" style:family="text"><style:text-properties fo:font-style="italic" fo:color="#c00000"/></style:style></office:automatic-styles>
<office:body><office:text>
<text:h text:style-name="Heading_20_1" text:outline-level="1">Überschrift</text:h>
<text:p text:style-name="Text_20_body">Ein <text:span text:style-name="Strong">fetter</text:span> und <text:span text:style-name="T1">roter</text:span>   Text<text:s text:c="2"/>mit<text:tab/>Tab.</text:p>
<text:list text:style-name="Numbering"><text:list-item><text:p>Eins</text:p><text:list><text:list-item><text:p>Unterpunkt</text:p></text:list-item></text:list></text:list-item></text:list>
<table:table><table:table-column table:number-columns-repeated="2"/><table:table-row><table:table-cell><text:p>A1</text:p></table:table-cell><table:table-cell><text:p>B1</text:p></table:table-cell></table:table-row></table:table>
<text:p text:style-name="P1">Neue Seite<text:note text:note-class="footnote"><text:note-citation>1</text:note-citation><text:note-body><text:p>Fußnote</text:p></text:note-body></text:note></text:p>
</office:text></office:body></office:document-content>"##;

    pub fn sample_odt() -> Vec<u8> {
        zip_stored(&[
            ("mimetype", b"application/vnd.oasis.opendocument.text"),
            ("content.xml", CONTENT.as_bytes()),
            ("styles.xml", STYLES.as_bytes()),
        ])
    }

    #[test]
    fn reads_text_formats_lists_tables_and_header() {
        let bytes = sample_odt();
        assert!(looks_like_odt(&bytes));
        let imp = import(&bytes, &|f| f == "Liberation Serif").unwrap();
        let d = &imp.doc;
        // A4, Ränder: oben mit Kopfzeile
        let cm = 96.0 / 2.54;
        assert!((d.width - 21.0 * cm).abs() < 0.1);
        assert!((d.margins[3] - 2.5 * cm).abs() < 0.1);
        assert!((d.margins[0] - 3.0 * cm).abs() < 0.1);
        let body = d.layers[0]
            .objects
            .iter()
            .find(|o| crate::editor::is_body(&o.shape))
            .unwrap();
        let Shape::TextArea {
            text,
            rich,
            size,
            family,
            ..
        } = &body.shape
        else {
            unreachable!()
        };
        assert_eq!(family, "Liberation Serif");
        assert!((size - 16.0).abs() < 1e-6);
        let paras: Vec<&str> = text.split('\n').collect();
        assert_eq!(paras[0], "Überschrift");
        assert_eq!(paras[1], "Ein fetter und roter Text  mit\tTab.");
        assert_eq!(paras[2], "Eins");
        assert_eq!(paras[3], "Unterpunkt");
        // Tabelle: Platzhalter-Absatz, davor ein Seitenumbruch
        assert_eq!(paras[4], format!("{PAGE_BREAK}"));
        assert!(paras[5].starts_with("Neue Seite[1]"));
        assert_eq!(paras[6], "[1] Fußnote");
        // Überschrift zentriert und größer, fett
        let p0 = rich.para(0, ListStyle::None);
        assert_eq!(p0.align, Some(Align::Center));
        let h = rich.runs.iter().find(|r| r.start == 0).unwrap();
        assert_eq!(h.style.bold, Some(true));
        assert!((h.style.size.unwrap() - 16.0 * 1.3).abs() < 0.01);
        // fett und kursiv-rot je Teil
        let fett = text.find("fetter").unwrap();
        assert!(rich
            .runs
            .iter()
            .any(|r| r.start == fett && r.style.bold == Some(true)));
        let rot = text.find("roter").unwrap();
        assert!(rich
            .runs
            .iter()
            .any(|r| r.start == rot && r.style.italic == Some(true) && r.style.color.is_some()));
        // Liste: nummeriert, Unterpunkt mit Punkt eine Stufe tiefer
        assert_eq!(rich.para(2, ListStyle::None).list, ListStyle::Number);
        let sub = rich.para(3, ListStyle::None);
        assert_eq!((sub.list, sub.indent), (ListStyle::Dot, 1));
        // Abstand nach „Textkörper“
        assert!((rich.para(1, ListStyle::None).after.unwrap() * 16.0 - 0.247 * cm).abs() < 0.1);
        // Kopfzeile mit Feldern
        let head = d.layers[0]
            .objects
            .iter()
            .find(|o| crate::editor::is_band(&o.shape))
            .unwrap();
        let ht = head.shape.text_fields().unwrap().0;
        assert_eq!(ht, format!("Seite {FIELD_PAGE} von {FIELD_PAGES}"));
        // Tabelle an ihrem Absatz
        assert_eq!(imp.floats.len(), 1);
        let at = text.split('\n').take(4).map(|p| p.len() + 1).sum::<usize>();
        assert_eq!(imp.floats[0].offset, at);
        assert!(matches!(&imp.floats[0].kind, FloatKind::Table(t) if t.cells[0][1] == "B1"));
        assert!(imp.notes.iter().any(|n| n.contains("footnote")));
    }

    #[test]
    fn not_an_odt() {
        assert!(!looks_like_odt(b"%PDF-1.4"));
        let other = zip_stored(&[("mimetype", b"application/zip")]);
        assert!(import(&other, &|_| true).is_err());
    }
}
