// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! Öffnen von Adobe-Illustrator-Dateien (.ai).
//!
//! Eine .ai-Datei mit „PDF-kompatibler Datei“ ist ein PDF. Die erste Seite wird hier
//! ausgewertet und als SVG-Text ausgegeben, den `svg::read` wie jede andere SVG-Datei
//! liest. Es wird nie als .ai gespeichert.
//!
//! Unterstützt: Pfade (Füllung, Kontur), Verläufe (axial, radial), Beschneidungspfade
//! (über Boolesche Operationen), Gruppen mit Deckkraft/Mischmodus, Ebenen (optionale
//! Inhalte), Text (über die ToUnicode-Tabelle der Schrift), Bilder (JPEG, Flate),
//! Farbräume (Gray, RGB, CMYK, ICC, Lab, Indexed, Separation, DeviceN).
//! Nicht unterstützt: Deckkraftmasken, Strichelungen, Muster, Verlaufsgitter, CMYK-JPEG.

use crate::i18n::tr;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use crate::boolean::{self, Op, Ring};
use crate::geom::{cubic_bbox, Affine, Pt, Rect};
use crate::model::{Color, GradShape, Gradient, GradientKind, Node, Stop, SubPath, PATTERN_CELL};
use crate::pdf::{Cs, Dict, Func, Lex, Obj, Pdf, Strm, Tok};
use crate::svg::{base64_encode, num, path_data};
use crate::xml::escape;

pub struct Import {
    pub svg: String,
    pub notes: Vec<String>,
}

/// Sieht die Datei nach einer Illustrator-/PDF-Datei aus?
pub fn looks_like_ai(data: &[u8]) -> bool {
    let head = &data[..data.len().min(2048)];
    head.windows(5).any(|w| w == b"%PDF-") || head.starts_with(b"%!PS-Adobe")
}

const MAX_PAGES: usize = 24;
const PAGE_GAP: f64 = 40.0;

// ───────────────────────────── Hilfsfunktionen ─────────────────────────────

fn hex(c: [f64; 3]) -> String {
    let b = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}

fn utf16_or_latin(b: &[u8]) -> String {
    if b.len() >= 2 && b[0] == 0xFE && b[1] == 0xFF {
        let units: Vec<u16> = b[2..]
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| (c[0] as u16) << 8 | c[1] as u16)
            .collect();
        String::from_utf16_lossy(&units)
    } else if b.len() >= 3 && b[0] == 0xEF && b[1] == 0xBB && b[2] == 0xBF {
        String::from_utf8_lossy(&b[3..]).into_owned()
    } else {
        b.iter().map(|&c| cp1252(c)).collect()
    }
}

fn cp1252(c: u8) -> char {
    const HI: [char; 32] = [
        '€', ' ', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', ' ', 'Ž', ' ', ' ', '‘', '’', '“',
        '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', ' ', 'ž', 'Ÿ',
    ];
    match c {
        0x80..=0x9F => HI[(c - 0x80) as usize],
        _ => c as char,
    }
}

fn glyph_char(name: &str) -> Option<char> {
    let mut it = name.chars();
    if let (Some(c), None) = (it.next(), it.next()) {
        return Some(c);
    }
    if let Some(h) = name.strip_prefix("uni") {
        if h.len() >= 4 {
            return u32::from_str_radix(&h[..4], 16).ok().and_then(char::from_u32);
        }
    }
    let c = match name {
        "space" => ' ',
        "exclam" => '!',
        "quotedbl" => '"',
        "numbersign" => '#',
        "dollar" => '$',
        "percent" => '%',
        "ampersand" => '&',
        "quotesingle" => '\'',
        "parenleft" => '(',
        "parenright" => ')',
        "asterisk" => '*',
        "plus" => '+',
        "comma" => ',',
        "hyphen" | "minus" => '-',
        "period" => '.',
        "slash" => '/',
        "zero" => '0',
        "one" => '1',
        "two" => '2',
        "three" => '3',
        "four" => '4',
        "five" => '5',
        "six" => '6',
        "seven" => '7',
        "eight" => '8',
        "nine" => '9',
        "colon" => ':',
        "semicolon" => ';',
        "less" => '<',
        "equal" => '=',
        "greater" => '>',
        "question" => '?',
        "at" => '@',
        "bracketleft" => '[',
        "backslash" => '\\',
        "bracketright" => ']',
        "asciicircum" => '^',
        "underscore" => '_',
        "grave" => '`',
        "braceleft" => '{',
        "bar" => '|',
        "braceright" => '}',
        "asciitilde" => '~',
        "Adieresis" => 'Ä',
        "Odieresis" => 'Ö',
        "Udieresis" => 'Ü',
        "adieresis" => 'ä',
        "odieresis" => 'ö',
        "udieresis" => 'ü',
        "germandbls" => 'ß',
        "endash" => '–',
        "emdash" => '—',
        "bullet" => '•',
        "quoteleft" => '‘',
        "quoteright" => '’',
        "quotedblleft" => '“',
        "quotedblright" => '”',
        "quotesinglbase" => '‚',
        "quotedblbase" => '„',
        "guillemotleft" => '«',
        "guillemotright" => '»',
        "Euro" => '€',
        "eacute" => 'é',
        "egrave" => 'è',
        "agrave" => 'à',
        "aacute" => 'á',
        "ccedilla" => 'ç',
        "copyright" => '©',
        "registered" => '®',
        "degree" => '°',
        "section" => '§',
        "fi" => 'ﬁ',
        "fl" => 'ﬂ',
        "ellipsis" => '…',
        "multiply" => '×',
        "divide" => '÷',
        "trademark" => '™',
        _ => return None,
    };
    Some(c)
}

// ───────────────────────────── PNG schreiben ─────────────────────────────

fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (n, v) in t.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
            }
            *v = c;
        }
        t
    });
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = t[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
}

/// PNG mit unkomprimierten Deflate-Blöcken. `ch`: 1 Grau, 3 RGB, 4 RGBA.
pub fn png_encode(w: u32, h: u32, ch: usize, pixels: &[u8]) -> Vec<u8> {
    let color = match ch {
        1 => 0,
        3 => 2,
        _ => 6,
    };
    let row = w as usize * ch;
    let mut raw = Vec::with_capacity((row + 1) * h as usize);
    for y in 0..h as usize {
        raw.push(0);
        raw.extend_from_slice(&pixels[y * row..(y + 1) * row]);
    }
    let mut z = vec![0x78, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    if raw.is_empty() {
        z.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(c) = chunks.next() {
        z.push(if chunks.peek().is_none() { 1 } else { 0 });
        z.extend_from_slice(&(c.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        z.extend_from_slice(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &v in &raw {
        a = (a + v as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, color, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// Anzahl der Farbkomponenten eines JPEG (aus dem SOF-Marker).
fn jpeg_components(d: &[u8]) -> Option<u8> {
    let mut i = 2;
    while i + 9 < d.len() {
        if d[i] != 0xFF {
            i += 1;
            continue;
        }
        let m = d[i + 1];
        if m == 0xFF {
            i += 1;
            continue;
        }
        if (0xC0..=0xCF).contains(&m) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
            return Some(d[i + 9]);
        }
        if m == 0xD8 || (0xD0..=0xD7).contains(&m) || m == 0x01 {
            i += 2;
            continue;
        }
        let len = (d[i + 2] as usize) << 8 | d[i + 3] as usize;
        i += 2 + len.max(2);
    }
    None
}

// ───────────────────────────── Schriften ─────────────────────────────

struct Font {
    family: String,
    bold: bool,
    italic: bool,
    two: bool,
    uni: HashMap<u32, String>,
    enc: Vec<Option<char>>,
    widths: HashMap<u32, f64>,
    dw: f64,
    has_uni: bool,
}

/// Schriftfamilie, Fett, Kursiv aus dem Schriftnamen. Kandidaten für die Familie (beste zuerst).
fn family_of(base: &str) -> (Vec<String>, bool, bool) {
    let mut name = base;
    if name.len() > 7 && name.as_bytes()[6] == b'+' && name[..6].chars().all(|c| c.is_ascii_uppercase()) {
        name = &name[7..];
    }
    let lower = name.to_lowercase();
    let bold = ["bold", "black", "heavy", "semibold", "demi", "extrabold"]
        .iter()
        .any(|k| lower.contains(k));
    let italic = ["italic", "oblique", "ital"].iter().any(|k| lower.contains(k));
    let mut fam = name.split(['-', ',']).next().unwrap_or(name).replace('_', " ");
    for suf in ["PSMT", "MT", "PS"] {
        if fam.len() > suf.len() + 2 && fam.ends_with(suf) {
            fam.truncate(fam.len() - suf.len());
            break;
        }
    }
    let mut split = String::new();
    let mut prev: Option<char> = None;
    for c in fam.chars() {
        if let Some(p) = prev {
            if c.is_uppercase() && p.is_lowercase() {
                split.push(' ');
            }
        }
        split.push(c);
        prev = Some(c);
    }
    // „DejaVu“ gehört zusammen: Großbuchstabe, Kleinbuchstaben, Großbuchstabe, Kleinbuchstaben
    let mut cands = vec![split.clone(), fam.clone()];
    let words: Vec<&str> = split.split(' ').collect();
    for i in 0..words.len().saturating_sub(1) {
        let mut w: Vec<String> = words.iter().map(|x| x.to_string()).collect();
        let merged = format!("{}{}", w[i], w[i + 1]);
        w.splice(i..i + 2, [merged]);
        cands.push(w.join(" "));
    }
    (cands, bold, italic)
}

fn utf16be(b: &[u8]) -> String {
    let units: Vec<u16> = b
        .chunks(2)
        .map(|c| {
            if c.len() == 2 {
                (c[0] as u16) << 8 | c[1] as u16
            } else {
                c[0] as u16
            }
        })
        .collect();
    String::from_utf16_lossy(&units)
}

fn be(b: &[u8]) -> u32 {
    b.iter().fold(0u32, |a, &x| (a << 8) | x as u32)
}

fn parse_tounicode(data: &[u8], uni: &mut HashMap<u32, String>) -> Option<usize> {
    let mut lex = Lex::new(data, false);
    let mut args: Vec<Obj> = Vec::new();
    let mut code_bytes = None;
    loop {
        match lex.next() {
            Tok::Eof => break,
            Tok::O(o) => args.push(o),
            Tok::Kw(k) => {
                match k.as_str() {
                    "endcodespacerange" => {
                        if let Some(Obj::Str(s)) = args.first() {
                            code_bytes = Some(s.len());
                        }
                    }
                    "endbfchar" => {
                        for p in args.chunks(2) {
                            if let [Obj::Str(src), Obj::Str(dst)] = p {
                                uni.insert(be(src), utf16be(dst));
                            }
                        }
                    }
                    "endbfrange" => {
                        for t in args.chunks(3) {
                            let [Obj::Str(lo), Obj::Str(hi), dst] = t else {
                                continue;
                            };
                            let (lo, hi) = (be(lo), be(hi));
                            if hi < lo || hi - lo > 65535 {
                                continue;
                            }
                            match dst {
                                Obj::Str(d) => {
                                    let base = utf16be(d);
                                    let mut chars: Vec<char> = base.chars().collect();
                                    for c in lo..=hi {
                                        uni.insert(c, chars.iter().collect());
                                        if let Some(l) = chars.last_mut() {
                                            *l = char::from_u32(*l as u32 + 1).unwrap_or(*l);
                                        }
                                    }
                                }
                                Obj::Arr(list) => {
                                    for (i, d) in list.iter().enumerate() {
                                        if let Obj::Str(d) = d {
                                            uni.insert(lo + i as u32, utf16be(d));
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
                args.clear();
            }
            _ => {}
        }
    }
    code_bytes
}

fn load_font(pdf: &Pdf, fd: &Dict, exists: &dyn Fn(&str) -> bool) -> Font {
    let subtype = pdf.get(fd, "Subtype").name().unwrap_or("").to_string();
    let mut base = pdf.get(fd, "BaseFont").name().unwrap_or("").to_string();
    let mut f = Font {
        family: String::new(),
        bold: false,
        italic: false,
        two: false,
        uni: HashMap::new(),
        enc: (0..256u32)
            .map(|c| if c >= 32 { Some(cp1252(c as u8)) } else { None })
            .collect(),
        widths: HashMap::new(),
        dw: 1000.0,
        has_uni: false,
    };
    let mut desc_dict: Option<&Dict> = pdf.get(fd, "FontDescriptor").dict();
    if subtype == "Type0" {
        f.two = true;
        if let Obj::Arr(a) = pdf.get(fd, "DescendantFonts") {
            if let Some(df) = a.first().map(|x| pdf.res(x)).and_then(|o| o.dict()) {
                if let Some(b) = pdf.get(df, "BaseFont").name() {
                    base = b.to_string();
                }
                f.dw = pdf.num(df, "DW").unwrap_or(1000.0);
                if let Obj::Arr(w) = pdf.get(df, "W") {
                    let mut i = 0;
                    while i < w.len() {
                        let Some(c0) = pdf.res(&w[i]).num() else { break };
                        match w.get(i + 1).map(|x| pdf.res(x)) {
                            Some(Obj::Arr(list)) => {
                                for (k, v) in list.iter().enumerate() {
                                    if let Some(v) = pdf.res(v).num() {
                                        f.widths.insert(c0 as u32 + k as u32, v);
                                    }
                                }
                                i += 2;
                            }
                            Some(Obj::Num(c1)) => {
                                let wv = w.get(i + 2).and_then(|x| pdf.res(x).num()).unwrap_or(f.dw);
                                let (a, b) = (c0 as u32, (*c1 as u32).min(c0 as u32 + 70000));
                                for c in a..=b {
                                    f.widths.insert(c, wv);
                                }
                                i += 3;
                            }
                            _ => break,
                        }
                    }
                }
                if desc_dict.is_none() {
                    desc_dict = pdf.get(df, "FontDescriptor").dict();
                }
            }
        }
    } else {
        let first = pdf.num(fd, "FirstChar").unwrap_or(0.0) as u32;
        if let Obj::Arr(w) = pdf.get(fd, "Widths") {
            for (i, v) in w.iter().enumerate() {
                if let Some(v) = pdf.res(v).num() {
                    f.widths.insert(first + i as u32, v);
                }
            }
        }
        f.dw = desc_dict
            .and_then(|d| pdf.num(d, "MissingWidth"))
            .filter(|w| *w > 0.0)
            .unwrap_or(if f.widths.is_empty() { 556.0 } else { 0.0 });
        // Kodierung mit Abweichungen
        if let Some(enc) = pdf.get(fd, "Encoding").dict() {
            if let Obj::Arr(diff) = pdf.get(enc, "Differences") {
                let mut code = 0usize;
                for item in diff {
                    match pdf.res(item) {
                        Obj::Num(n) => code = *n as usize,
                        Obj::Name(nm) => {
                            if code < 256 {
                                f.enc[code] = glyph_char(nm);
                            }
                            code += 1;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    if let Obj::Stream(s) = pdf.get(fd, "ToUnicode") {
        let (data, _) = pdf.stream_data(s);
        let cb = parse_tounicode(&data, &mut f.uni);
        f.has_uni = !f.uni.is_empty();
        if f.two && cb == Some(1) {
            f.two = false;
        }
    }
    let (cands, mut bold, mut italic) = family_of(&base);
    let fam = cands
        .iter()
        .find(|c| !c.is_empty() && exists(c))
        .or_else(|| cands.first())
        .cloned()
        .unwrap_or_default();
    if let Some(d) = desc_dict {
        let flags = pdf.num(d, "Flags").unwrap_or(0.0) as u32;
        italic |= flags & 64 != 0 || pdf.num(d, "ItalicAngle").map(|a| a != 0.0).unwrap_or(false);
        bold |= flags & (1 << 18) != 0 || pdf.num(d, "FontWeight").map(|w| w >= 600.0).unwrap_or(false);
        if fam.is_empty() {
            if let Some(n) = pdf.get(d, "FontFamily").string() {
                f.family = utf16_or_latin(n);
            }
        }
    }
    if f.family.is_empty() {
        f.family = if fam.is_empty() { "Sans".into() } else { fam };
    }
    f.bold = bold;
    f.italic = italic;
    f
}

impl Font {
    /// (Code, Text, Breite in 1/1000) je Zeichen.
    fn decode(&self, s: &[u8]) -> Vec<(u32, String, f64)> {
        let mut out = Vec::new();
        if self.two {
            for c in s.chunks(2) {
                let code = be(c);
                let text = self.uni.get(&code).cloned().unwrap_or_default();
                out.push((code, text, self.widths.get(&code).copied().unwrap_or(self.dw)));
            }
        } else {
            for &b in s {
                let code = b as u32;
                let text = match self.uni.get(&code) {
                    Some(t) => t.clone(),
                    None if !self.has_uni => self.enc[b as usize].map(|c| c.to_string()).unwrap_or_default(),
                    None => String::new(),
                };
                out.push((code, text, self.widths.get(&code).copied().unwrap_or(self.dw)));
            }
        }
        out
    }
}

// ───────────────────────────── Zustand ─────────────────────────────

struct Shade {
    radial: bool,
    p0: Pt,
    p1: Pt,
    r1: f64,
    stops: Vec<(f64, [f64; 3])>,
}

#[derive(Clone)]
enum Paint {
    Solid([f64; 3]),
    Grad(Rc<Shade>),
}

impl Paint {
    fn flat(&self) -> [f64; 3] {
        match self {
            Paint::Solid(c) => *c,
            Paint::Grad(s) => s.stops.get(s.stops.len() / 2).map(|x| x.1).unwrap_or([0.5; 3]),
        }
    }
}

struct Clip {
    rings: Vec<Ring>,
    bbox: Rect,
    is_rect: bool,
}

#[derive(Clone)]
struct Gs {
    ctm: Affine,
    fill: Paint,
    stroke: Paint,
    fcs: Cs,
    scs: Cs,
    lw: f64,
    cap: u8,
    join: u8,
    fa: f64,
    sa: f64,
    blend: String,
    clip: Option<Rc<Clip>>,
    tc: f64,
    tw: f64,
    th: f64,
    tl: f64,
    font: Option<Rc<Font>>,
    fs: f64,
    tr: u8,
    rise: f64,
    /// Strichmuster (in Benutzereinheiten)
    dash: Vec<f64>,
}

impl Gs {
    fn new(ctm: Affine) -> Gs {
        Gs {
            ctm,
            fill: Paint::Solid([0.0; 3]),
            stroke: Paint::Solid([0.0; 3]),
            fcs: Cs::Gray,
            scs: Cs::Gray,
            lw: 1.0,
            cap: 0,
            join: 0,
            fa: 1.0,
            sa: 1.0,
            blend: "Normal".into(),
            clip: None,
            tc: 0.0,
            tw: 0.0,
            th: 1.0,
            tl: 0.0,
            font: None,
            fs: 1.0,
            tr: 0,
            rise: 0.0,
            dash: Vec::new(),
        }
    }
}

struct Layer {
    name: String,
    hidden: bool,
    body: String,
}

struct PText {
    sig: String,
    end: Pt,
    head: String,
    text: String,
    layer: usize,
    size: f64,
}

#[derive(PartialEq)]
enum ClipTest {
    Outside,
    Inside,
    Partial,
}

struct Interp<'a> {
    pdf: &'a Pdf,
    exists: &'a dyn Fn(&str) -> bool,
    gs: Gs,
    stack: Vec<Gs>,
    path: Vec<SubPath>,
    start: Pt,
    pend_clip: Option<bool>,
    layers: Vec<Layer>,
    layer_ix: HashMap<String, usize>,
    cur: Option<usize>,
    mc: Vec<bool>,
    oc_depth: u32,
    off: HashSet<u32>,
    tm: Affine,
    tlm: Affine,
    pending: Option<PText>,
    base: Affine,
    page: Rect,
    notes: BTreeSet<String>,
    depth: usize,
    fonts: HashMap<u32, Rc<Font>>,
    ops: usize,
}

fn rect_of(a: Pt, b: Pt) -> Rect {
    Rect {
        x0: a.x.min(b.x),
        y0: a.y.min(b.y),
        x1: a.x.max(b.x),
        y1: a.y.max(b.y),
    }
}

fn subs_bbox(subs: &[SubPath]) -> Option<Rect> {
    let mut r: Option<Rect> = None;
    for sp in subs {
        if sp.nodes.len() == 1 {
            let p = sp.nodes[0].p;
            let b = rect_of(p, p);
            r = Some(r.map(|x| x.union(&b)).unwrap_or(b));
        }
        for (a, c1, c2, b) in sp.segments() {
            let bb = cubic_bbox(a, c1, c2, b);
            r = Some(r.map(|x| x.union(&bb)).unwrap_or(bb));
        }
    }
    r
}

fn transform_subs(subs: &[SubPath], m: &Affine) -> Vec<SubPath> {
    subs.iter()
        .map(|sp| {
            let mut nodes: Vec<Node> = sp
                .nodes
                .iter()
                .map(|n| Node {
                    p: m.apply(n.p),
                    h_in: m.apply(n.h_in),
                    h_out: m.apply(n.h_out),
                    r: 0.0,
                })
                .collect();
            let mut closed = sp.closed;
            // Schlusspunkt gleich Startpunkt: zusammenlegen
            if closed && nodes.len() > 2 && nodes[0].p.dist(nodes[nodes.len() - 1].p) < 1e-6 {
                let last = nodes.pop().unwrap();
                nodes[0].h_in = last.h_in;
            }
            if nodes.len() < 2 {
                closed = false;
            }
            SubPath { nodes, closed }
        })
        .collect()
}

fn is_rect_ring(rings: &[Ring]) -> bool {
    if rings.len() != 1 {
        return false;
    }
    let r = &rings[0];
    let n = if r.len() == 5 && r[0].dist(r[4]) < 1e-6 {
        4
    } else {
        r.len()
    };
    if n != 4 {
        return false;
    }
    (0..4).all(|i| {
        let (a, b) = (r[i], r[(i + 1) % 4]);
        (a.x - b.x).abs() < 1e-6 || (a.y - b.y).abs() < 1e-6
    })
}

fn rings_bbox(rings: &[Ring]) -> Rect {
    let mut r: Option<Rect> = None;
    for ring in rings {
        for p in ring {
            let b = rect_of(*p, *p);
            r = Some(r.map(|x| x.union(&b)).unwrap_or(b));
        }
    }
    r.unwrap_or(Rect {
        x0: 0.0,
        y0: 0.0,
        x1: 0.0,
        y1: 0.0,
    })
}

fn css_blend(pdf_name: &str) -> Option<String> {
    if pdf_name == "Normal" || pdf_name == "Compatible" {
        return None;
    }
    let mut out = String::new();
    for (i, c) in pdf_name.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('-');
        }
        out.push(c.to_ascii_lowercase());
    }
    Some(out)
}

impl<'a> Interp<'a> {
    fn new(pdf: &'a Pdf, exists: &'a dyn Fn(&str) -> bool) -> Interp<'a> {
        Interp {
            pdf,
            exists,
            gs: Gs::new(Affine::IDENTITY),
            stack: Vec::new(),
            path: Vec::new(),
            start: Pt::new(0.0, 0.0),
            pend_clip: None,
            layers: Vec::new(),
            layer_ix: HashMap::new(),
            cur: None,
            mc: Vec::new(),
            oc_depth: 0,
            off: HashSet::new(),
            tm: Affine::IDENTITY,
            tlm: Affine::IDENTITY,
            pending: None,
            base: Affine::IDENTITY,
            page: rect_of(Pt::new(0.0, 0.0), Pt::new(1.0, 1.0)),
            notes: BTreeSet::new(),
            depth: 0,
            fonts: HashMap::new(),
            ops: 0,
        }
    }

    fn layer_for(&mut self, name: &str, hidden: bool) -> usize {
        if let Some(&i) = self.layer_ix.get(name) {
            return i;
        }
        self.layers.push(Layer {
            name: name.to_string(),
            hidden,
            body: String::new(),
        });
        let i = self.layers.len() - 1;
        self.layer_ix.insert(name.to_string(), i);
        i
    }

    fn target(&mut self) -> usize {
        match self.cur {
            Some(i) => i,
            None => self.layer_for("Ebene 1", false),
        }
    }

    fn emit(&mut self, s: &str) {
        let i = self.target();
        let b = &mut self.layers[i].body;
        b.push_str("    ");
        b.push_str(s);
        b.push('\n');
    }

    fn note(&mut self, s: &str) {
        self.notes.insert(s.to_string());
    }

    // ───── Auswertung ─────

    fn exec(&mut self, content: &[u8], res: &Dict) {
        let mut lex = Lex::new(content, false);
        let mut args: Vec<Obj> = Vec::new();
        loop {
            match lex.next() {
                Tok::Eof => break,
                Tok::O(o) => {
                    args.push(o);
                    if args.len() > 96 {
                        args.remove(0);
                    }
                }
                Tok::Kw(k) => {
                    self.ops += 1;
                    if self.ops > 20_000_000 {
                        self.note(tr("Die Datei ist sehr groß; es wurde nur ein Teil gelesen"));
                        break;
                    }
                    if k == "BI" {
                        self.inline_image(&mut lex, res);
                    } else {
                        self.op(&k, &args, res);
                    }
                    args.clear();
                }
                _ => {}
            }
        }
    }

    fn op(&mut self, op: &str, a: &[Obj], res: &Dict) {
        let n = |i: usize| a.get(i).and_then(Obj::num).unwrap_or(0.0);
        let pdf = self.pdf;
        match op {
            "q" => {
                if self.stack.len() < 256 {
                    self.stack.push(self.gs.clone());
                }
            }
            "Q" => {
                if let Some(g) = self.stack.pop() {
                    self.flush_text();
                    self.gs = g;
                }
            }
            "cm" => {
                let m = Affine {
                    a: n(0),
                    b: n(1),
                    c: n(2),
                    d: n(3),
                    e: n(4),
                    f: n(5),
                };
                self.gs.ctm = self.gs.ctm.then_before(&m);
            }
            "w" => self.gs.lw = n(0),
            "J" => self.gs.cap = n(0) as u8,
            "j" => self.gs.join = n(0) as u8,
            "d" => {
                self.gs.dash = match a.first() {
                    Some(Obj::Arr(d)) => {
                        let v: Vec<f64> = d.iter().map(|x| x.num().unwrap_or(0.0).max(0.0)).collect();
                        if v.iter().any(|x| *x > 0.0) {
                            v
                        } else {
                            Vec::new()
                        }
                    }
                    _ => Vec::new(),
                };
            }
            "gs" => {
                let name = a.first().and_then(Obj::name).unwrap_or("");
                if let Some(d) = pdf
                    .get(res, "ExtGState")
                    .dict()
                    .and_then(|d| d.get(name))
                    .and_then(|o| pdf.res(o).dict())
                {
                    if let Some(v) = pdf.num(d, "LW") {
                        self.gs.lw = v;
                    }
                    if let Some(v) = pdf.num(d, "LC") {
                        self.gs.cap = v as u8;
                    }
                    if let Some(v) = pdf.num(d, "LJ") {
                        self.gs.join = v as u8;
                    }
                    if let Some(v) = pdf.num(d, "ca") {
                        self.gs.fa = v.clamp(0.0, 1.0);
                    }
                    if let Some(v) = pdf.num(d, "CA") {
                        self.gs.sa = v.clamp(0.0, 1.0);
                    }
                    match pdf.get(d, "BM") {
                        Obj::Name(b) => self.gs.blend = b.clone(),
                        Obj::Arr(l) => {
                            if let Some(b) = l.first().and_then(|x| pdf.res(x).name()) {
                                self.gs.blend = b.to_string();
                            }
                        }
                        _ => {}
                    }
                    if let Some(sm) = d.get("SMask") {
                        if pdf.res(sm).dict().is_some() {
                            self.note(tr("Deckkraftmasken wurden nicht übernommen"));
                        }
                    }
                    if let Obj::Arr(f) = pdf.get(d, "Font") {
                        if let Some(fo) = f.first() {
                            self.set_font_obj(fo);
                            self.gs.fs = f.get(1).and_then(|x| pdf.res(x).num()).unwrap_or(1.0);
                        }
                    }
                }
            }
            // Pfade
            "m" => {
                let p = Pt::new(n(0), n(1));
                self.path.push(SubPath {
                    nodes: vec![Node::corner(p)],
                    closed: false,
                });
                self.start = p;
            }
            "l" => {
                let p = Pt::new(n(0), n(1));
                self.ensure_open();
                self.path.last_mut().unwrap().nodes.push(Node::corner(p));
            }
            "c" | "v" | "y" => {
                self.ensure_open();
                let sp = self.path.last_mut().unwrap();
                let cur = sp.nodes.last().unwrap().p;
                let (c1, c2, p3) = match op {
                    "c" => (Pt::new(n(0), n(1)), Pt::new(n(2), n(3)), Pt::new(n(4), n(5))),
                    "v" => (cur, Pt::new(n(0), n(1)), Pt::new(n(2), n(3))),
                    _ => {
                        let p3 = Pt::new(n(2), n(3));
                        (Pt::new(n(0), n(1)), p3, p3)
                    }
                };
                sp.nodes.last_mut().unwrap().h_out = c1;
                sp.nodes.push(Node {
                    p: p3,
                    h_in: c2,
                    h_out: p3,
                    r: 0.0,
                });
            }
            "h" => {
                if let Some(sp) = self.path.last_mut() {
                    sp.closed = true;
                }
            }
            "re" => {
                let (x, y, w, h) = (n(0), n(1), n(2), n(3));
                let pts = [
                    Pt::new(x, y),
                    Pt::new(x + w, y),
                    Pt::new(x + w, y + h),
                    Pt::new(x, y + h),
                ];
                self.path.push(SubPath {
                    nodes: pts.iter().map(|p| Node::corner(*p)).collect(),
                    closed: true,
                });
                self.start = pts[0];
            }
            "S" => self.paint_path(false, true, false, false),
            "s" => self.paint_path(false, true, false, true),
            "f" | "F" => self.paint_path(true, false, false, false),
            "f*" => self.paint_path(true, false, true, false),
            "B" => self.paint_path(true, true, false, false),
            "B*" => self.paint_path(true, true, true, false),
            "b" => self.paint_path(true, true, false, true),
            "b*" => self.paint_path(true, true, true, true),
            "n" => self.paint_path(false, false, false, false),
            "W" => self.pend_clip = Some(false),
            "W*" => self.pend_clip = Some(true),
            // Farben
            "g" => {
                self.gs.fcs = Cs::Gray;
                self.gs.fill = Paint::Solid(Cs::Gray.to_rgb(&[n(0)]));
            }
            "G" => {
                self.gs.scs = Cs::Gray;
                self.gs.stroke = Paint::Solid(Cs::Gray.to_rgb(&[n(0)]));
            }
            "rg" => {
                self.gs.fcs = Cs::Rgb;
                self.gs.fill = Paint::Solid(Cs::Rgb.to_rgb(&[n(0), n(1), n(2)]));
            }
            "RG" => {
                self.gs.scs = Cs::Rgb;
                self.gs.stroke = Paint::Solid(Cs::Rgb.to_rgb(&[n(0), n(1), n(2)]));
            }
            "k" => {
                self.gs.fcs = Cs::Cmyk;
                self.gs.fill = Paint::Solid(Cs::Cmyk.to_rgb(&[n(0), n(1), n(2), n(3)]));
            }
            "K" => {
                self.gs.scs = Cs::Cmyk;
                self.gs.stroke = Paint::Solid(Cs::Cmyk.to_rgb(&[n(0), n(1), n(2), n(3)]));
            }
            "cs" | "CS" => {
                let cs = match a.first() {
                    Some(o) => Cs::parse(pdf, o, res, 0),
                    None => Cs::Gray,
                };
                let paint = Paint::Solid(cs.to_rgb(&cs.initial()));
                if op == "cs" {
                    self.gs.fcs = cs;
                    self.gs.fill = paint;
                } else {
                    self.gs.scs = cs;
                    self.gs.stroke = paint;
                }
            }
            "sc" | "scn" => self.set_color(false, a, res),
            "SC" | "SCN" => self.set_color(true, a, res),
            "sh" => self.shading_fill(a.first().and_then(Obj::name).unwrap_or(""), res),
            "Do" => self.do_xobject(a.first().and_then(Obj::name).unwrap_or(""), res),
            // Text
            "BT" => {
                self.tm = Affine::IDENTITY;
                self.tlm = Affine::IDENTITY;
            }
            "ET" => self.flush_text(),
            "Tc" => self.gs.tc = n(0),
            "Tw" => self.gs.tw = n(0),
            "Tz" => self.gs.th = n(0) / 100.0,
            "TL" => self.gs.tl = n(0),
            "Tr" => self.gs.tr = n(0) as u8,
            "Ts" => self.gs.rise = n(0),
            "Tf" => {
                let name = a.first().and_then(Obj::name).unwrap_or("");
                if let Some(fo) = pdf.get(res, "Font").dict().and_then(|d| d.get(name)) {
                    self.set_font_obj(fo);
                }
                self.gs.fs = n(1);
            }
            "Td" | "TD" => {
                if op == "TD" {
                    self.gs.tl = -n(1);
                }
                self.tlm = self.tlm.then_before(&Affine::translate(n(0), n(1)));
                self.tm = self.tlm;
            }
            "Tm" => {
                self.tlm = Affine {
                    a: n(0),
                    b: n(1),
                    c: n(2),
                    d: n(3),
                    e: n(4),
                    f: n(5),
                };
                self.tm = self.tlm;
            }
            "T*" => self.next_line(),
            "Tj" => {
                if let Some(Obj::Str(s)) = a.first() {
                    let s = s.clone();
                    self.show(&[TextItem::Str(s)]);
                }
            }
            "'" | "\"" => {
                if op == "\"" {
                    self.gs.tw = n(0);
                    self.gs.tc = n(1);
                }
                self.next_line();
                if let Some(Obj::Str(s)) = a.last() {
                    let s = s.clone();
                    self.show(&[TextItem::Str(s)]);
                }
            }
            "TJ" => {
                if let Some(Obj::Arr(items)) = a.first() {
                    let list: Vec<TextItem> = items
                        .iter()
                        .filter_map(|o| match o {
                            Obj::Str(s) => Some(TextItem::Str(s.clone())),
                            Obj::Num(v) => Some(TextItem::Adj(*v)),
                            _ => None,
                        })
                        .collect();
                    self.show(&list);
                }
            }
            // Markierter Inhalt (Ebenen)
            "BMC" | "BDC" => {
                self.flush_text();
                let mut pushed = false;
                if op == "BDC" && a.first().and_then(Obj::name) == Some("OC") && self.oc_depth == 0 {
                    let (dict, num) = match a.get(1) {
                        Some(Obj::Name(nm)) => {
                            let raw = pdf.get(res, "Properties").dict().and_then(|d| d.get(nm));
                            let num = match raw {
                                Some(Obj::Ref(r)) => Some(*r),
                                _ => None,
                            };
                            (raw.and_then(|r| pdf.res(r).dict()), num)
                        }
                        Some(Obj::Dict(d)) => (Some(d), None),
                        _ => (None, None),
                    };
                    if let Some(d) = dict {
                        let mut d = d;
                        let mut num = num;
                        // Mitgliedschaft: erste Gruppe nehmen
                        if pdf.get(d, "Type").name() == Some("OCMD") {
                            let first = match d.get("OCGs") {
                                Some(Obj::Arr(l)) => l.first(),
                                other => other,
                            };
                            if let Some(g) = first {
                                num = if let Obj::Ref(r) = g { Some(*r) } else { None };
                                if let Some(gd) = pdf.res(g).dict() {
                                    d = gd;
                                }
                            }
                        }
                        let name = pdf
                            .get(d, "Name")
                            .string()
                            .map(utf16_or_latin)
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| "Ebene".to_string());
                        let hidden = num.map(|r| self.off.contains(&r)).unwrap_or(false);
                        self.cur = Some(self.layer_for(&name, hidden));
                        self.oc_depth = 1;
                        pushed = true;
                    }
                }
                self.mc.push(pushed);
            }
            "EMC" => {
                self.flush_text();
                if let Some(true) = self.mc.pop() {
                    self.cur = None;
                    self.oc_depth = 0;
                }
            }
            _ => {}
        }
    }

    fn ensure_open(&mut self) {
        let need_new = match self.path.last() {
            None => true,
            Some(sp) => sp.closed,
        };
        if need_new {
            self.path.push(SubPath {
                nodes: vec![Node::corner(self.start)],
                closed: false,
            });
        }
    }

    // ───── Farben und Muster ─────

    fn set_color(&mut self, stroke: bool, a: &[Obj], res: &Dict) {
        let pdf = self.pdf;
        let cs = if stroke {
            self.gs.scs.clone()
        } else {
            self.gs.fcs.clone()
        };
        let paint = if let Some(Obj::Name(pn)) = a.last() {
            let pat = pdf
                .get(res, "Pattern")
                .dict()
                .and_then(|d| d.get(pn))
                .map(|o| pdf.res(o));
            self.pattern_paint(pat, res)
        } else {
            let comps: Vec<f64> = a.iter().filter_map(|o| o.num()).collect();
            Paint::Solid(cs.to_rgb(&comps))
        };
        if stroke {
            self.gs.stroke = paint;
        } else {
            self.gs.fill = paint;
        }
    }

    fn pattern_paint(&mut self, pat: Option<&Obj>, res: &Dict) -> Paint {
        let pdf = self.pdf;
        let Some(d) = pat.and_then(|p| p.dict()) else {
            return Paint::Solid([0.5; 3]);
        };
        if pdf.num(d, "PatternType") == Some(2.0) {
            let m = pdf.nums(pdf.get(d, "Matrix"));
            let pm = if m.len() >= 6 {
                Affine {
                    a: m[0],
                    b: m[1],
                    c: m[2],
                    d: m[3],
                    e: m[4],
                    f: m[5],
                }
            } else {
                Affine::IDENTITY
            };
            let total = self.base.then_before(&pm);
            if let Some(sh) = self.make_shade(pdf.get(d, "Shading"), &total, res) {
                return Paint::Grad(Rc::new(sh));
            }
        }
        self.note(tr("Muster- und Netzfüllungen wurden vereinfacht"));
        Paint::Solid([0.6; 3])
    }

    fn make_shade(&mut self, sh: &Obj, m: &Affine, res: &Dict) -> Option<Shade> {
        let pdf = self.pdf;
        let d = sh.dict()?;
        let kind = pdf.num(d, "ShadingType")? as i32;
        if kind != 2 && kind != 3 {
            self.note(tr("Muster- und Netzfüllungen wurden vereinfacht"));
            return None;
        }
        let coords = pdf.nums(pdf.get(d, "Coords"));
        if (kind == 2 && coords.len() < 4) || (kind == 3 && coords.len() < 6) {
            return None;
        }
        let cs = Cs::parse(pdf, pdf.get(d, "ColorSpace"), res, 0);
        let funcs: Vec<Func> = match pdf.get(d, "Function") {
            Obj::Arr(l) => l.iter().filter_map(|f| Func::parse(pdf, f)).collect(),
            other => Func::parse(pdf, other).into_iter().collect(),
        };
        if funcs.is_empty() {
            return None;
        }
        let dom = pdf.nums(pdf.get(d, "Domain"));
        let (t0, t1) = if dom.len() >= 2 {
            (dom[0], dom[1])
        } else {
            (0.0, 1.0)
        };
        let mut ts = Vec::new();
        for f in &funcs {
            f.breaks(t0, t1, &mut ts);
        }
        ts.retain(|t| *t >= t0 - 1e-9 && *t <= t1 + 1e-9);
        ts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        ts.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        if ts.len() > 40 {
            let step = ts.len() as f64 / 40.0;
            ts = (0..40)
                .map(|i| ts[((i as f64 * step) as usize).min(ts.len() - 1)])
                .collect();
            if let Some(l) = ts.last_mut() {
                *l = t1;
            }
        }
        let span = if (t1 - t0).abs() < 1e-12 { 1.0 } else { t1 - t0 };
        let mut stops: Vec<(f64, [f64; 3])> = ts
            .iter()
            .map(|&t| {
                let mut comps = Vec::new();
                for f in &funcs {
                    comps.extend(f.eval(&[t]));
                }
                ((t - t0) / span, cs.to_rgb(&comps))
            })
            .collect();
        if stops.len() < 2 {
            let c = stops.first().map(|s| s.1).unwrap_or([0.5; 3]);
            stops = vec![(0.0, c), (1.0, c)];
        }
        if kind == 2 {
            Some(Shade {
                radial: false,
                p0: m.apply(Pt::new(coords[0], coords[1])),
                p1: m.apply(Pt::new(coords[2], coords[3])),
                r1: 0.0,
                stops,
            })
        } else {
            let (r0, r1) = (coords[2].abs(), coords[5].abs());
            if r1 < 1e-9 {
                return None;
            }
            if r0 > 1e-9 {
                // Innenradius: Verlauf beginnt erst bei r0/r1
                let k = (r0 / r1).min(0.99);
                let first = stops[0].1;
                for s in stops.iter_mut() {
                    s.0 = k + s.0 * (1.0 - k);
                }
                stops.insert(0, (0.0, first));
            }
            Some(Shade {
                radial: true,
                p0: m.apply(Pt::new(coords[0], coords[1])),
                p1: m.apply(Pt::new(coords[3], coords[4])),
                r1: r1 * m.mean_scale(),
                stops,
            })
        }
    }

    fn gradient_attr(&self, sh: &Shade, bbox: &Rect) -> Option<String> {
        if bbox.width() < 1e-6 || bbox.height() < 1e-6 || sh.stops.len() < 2 {
            return None;
        }
        let stops: Vec<Stop> = sh
            .stops
            .iter()
            .map(|(t, c)| Stop {
                pos: t.clamp(0.0, 1.0),
                color: Color::rgb(c[0], c[1], c[2]),
            })
            .collect();
        let mut g = Gradient {
            stops,
            p0: Pt::new(0.0, 0.0),
            p1: Pt::new(1.0, 0.0),
            kind: GradientKind::Smooth,
            shape: if sh.radial {
                GradShape::Radial
            } else {
                GradShape::Linear
            },
            cell: PATTERN_CELL,
        };
        if sh.radial {
            g.set_line(sh.p0, Pt::new(sh.p0.x + sh.r1, sh.p0.y), bbox);
        } else {
            g.set_line(sh.p0, sh.p1, bbox);
        }
        Some(g.to_attr())
    }

    // ───── Zeichnen ─────

    fn paint_path(&mut self, fill: bool, stroke: bool, evenodd: bool, close: bool) {
        self.flush_text();
        let mut subs = std::mem::take(&mut self.path);
        if close {
            if let Some(l) = subs.last_mut() {
                l.closed = true;
            }
        }
        if !subs.is_empty() && (fill || stroke) {
            self.emit_path(&subs, fill, stroke, evenodd);
        }
        if let Some(eo) = self.pend_clip.take() {
            self.apply_clip(&subs, eo);
        }
    }

    fn apply_clip(&mut self, subs: &[SubPath], _evenodd: bool) {
        let t = transform_subs(subs, &self.gs.ctm);
        if t.is_empty() {
            return;
        }
        let mut rings = boolean::to_rings(&t, 0.1);
        let mut rect = is_rect_ring(&rings);
        if let Some(old) = &self.gs.clip {
            rings = boolean::rings_op(&old.rings, &rings, Op::Intersection);
            rect = is_rect_ring(&rings);
        } else if rect {
            // Beschnitt auf die Seite selbst: Inhalt darüber hinaus soll erhalten bleiben
            let b = rings_bbox(&rings);
            let pg = &self.page;
            let tol = 1.0;
            if b.x0 <= pg.x0 + tol && b.y0 <= pg.y0 + tol && b.x1 >= pg.x1 - tol && b.y1 >= pg.y1 - tol {
                return;
            }
        }
        let bbox = rings_bbox(&rings);
        self.gs.clip = Some(Rc::new(Clip {
            rings,
            bbox,
            is_rect: rect,
        }));
    }

    fn clip_test(&self, bbox: &Rect) -> ClipTest {
        let Some(c) = &self.gs.clip else {
            return ClipTest::Inside;
        };
        if c.rings.is_empty() || !bbox.intersects(&c.bbox) {
            return ClipTest::Outside;
        }
        if c.is_rect && c.bbox.contains_rect(bbox) {
            return ClipTest::Inside;
        }
        ClipTest::Partial
    }

    fn blend_attr(&self) -> String {
        match css_blend(&self.gs.blend) {
            Some(b) => format!(" mix-blend-mode=\"{b}\""),
            None => String::new(),
        }
    }

    fn stroke_attrs(&self, bbox: &Rect) -> String {
        let g = &self.gs;
        let w = if g.lw <= 0.0 {
            0.25
        } else {
            g.lw * g.ctm.mean_scale()
        };
        let mut s = format!(" stroke=\"{}\" stroke-width=\"{}\"", hex(g.stroke.flat()), num(w));
        if let Paint::Grad(sh) = &g.stroke {
            if let Some(a) = self.gradient_attr(sh, bbox) {
                s.push_str(&format!(" mondrian:stroke-gradient=\"{a}\""));
            }
        }
        if g.sa < 0.999 {
            s.push_str(&format!(" stroke-opacity=\"{}\"", num(g.sa)));
        }
        s.push_str(match g.cap {
            1 => " stroke-linecap=\"round\"",
            2 => " stroke-linecap=\"square\"",
            _ => "",
        });
        s.push_str(match g.join {
            1 => " stroke-linejoin=\"round\"",
            2 => " stroke-linejoin=\"bevel\"",
            _ => "",
        });
        if !g.dash.is_empty() {
            let k = g.ctm.mean_scale();
            let v: Vec<String> = g.dash.iter().map(|x| num(x * k)).collect();
            s.push_str(&format!(" stroke-dasharray=\"{}\"", v.join(" ")));
        }
        s
    }

    fn fill_attrs(&self, bbox: &Rect, evenodd: bool) -> String {
        let g = &self.gs;
        let mut s = format!(" fill=\"{}\"", hex(g.fill.flat()));
        if let Paint::Grad(sh) = &g.fill {
            if let Some(a) = self.gradient_attr(sh, bbox) {
                s.push_str(&format!(" mondrian:fill-gradient=\"{a}\""));
            }
        }
        if g.fa < 0.999 {
            s.push_str(&format!(" fill-opacity=\"{}\"", num(g.fa)));
        }
        if evenodd {
            s.push_str(" fill-rule=\"evenodd\"");
        }
        s
    }

    fn emit_path(&mut self, subs: &[SubPath], fill: bool, stroke: bool, evenodd: bool) {
        let t = transform_subs(subs, &self.gs.ctm);
        let t: Vec<SubPath> = t.into_iter().filter(|s| s.nodes.len() >= 2).collect();
        let Some(bbox) = subs_bbox(&t) else { return };
        let blend = self.blend_attr();
        let mut fill = fill;
        let mut shape = t.clone();
        let mut split_stroke = false;
        match self.clip_test(&bbox) {
            ClipTest::Outside => return,
            ClipTest::Inside => {}
            ClipTest::Partial => {
                if fill {
                    let clip = self.gs.clip.clone().unwrap();
                    let rings = boolean::to_rings(&t, 0.05);
                    let cut = boolean::rings_op(&rings, &clip.rings, Op::Intersection);
                    if cut.is_empty() {
                        fill = false;
                    } else {
                        shape = boolean::rings_to_paths(cut);
                        split_stroke = stroke;
                    }
                }
            }
        }
        let evenodd = if shape.len() != t.len() { false } else { evenodd };
        if fill && split_stroke {
            let fa = self.fill_attrs(&bbox, evenodd);
            let d = path_data(&shape);
            self.emit(&format!("<path d=\"{d}\"{fa} stroke=\"none\"{blend}/>"));
            let sa = self.stroke_attrs(&bbox);
            let d = path_data(&t);
            self.emit(&format!("<path d=\"{d}\" fill=\"none\"{sa}{blend}/>"));
            return;
        }
        if !fill && !stroke {
            return;
        }
        let mut attrs = if fill {
            self.fill_attrs(&bbox, evenodd)
        } else {
            " fill=\"none\"".to_string()
        };
        if stroke {
            attrs.push_str(&self.stroke_attrs(&bbox));
        }
        let d = path_data(&shape);
        self.emit(&format!("<path d=\"{d}\"{attrs}{blend}/>"));
    }

    fn shading_fill(&mut self, name: &str, res: &Dict) {
        self.flush_text();
        let pdf = self.pdf;
        let Some(sh) = pdf
            .get(res, "Shading")
            .dict()
            .and_then(|d| d.get(name))
            .map(|o| pdf.res(o))
        else {
            return;
        };
        let ctm = self.gs.ctm;
        let Some(shade) = self.make_shade(sh, &ctm, res) else {
            return;
        };
        // Fläche: aktueller Beschnitt oder die ganze Seite
        let paths: Vec<SubPath> = match &self.gs.clip {
            Some(c) => boolean::rings_to_paths(c.rings.clone()),
            None => {
                let p = self.page;
                vec![SubPath {
                    nodes: [
                        Pt::new(p.x0, p.y0),
                        Pt::new(p.x1, p.y0),
                        Pt::new(p.x1, p.y1),
                        Pt::new(p.x0, p.y1),
                    ]
                    .iter()
                    .map(|q| Node::corner(*q))
                    .collect(),
                    closed: true,
                }]
            }
        };
        let Some(bbox) = subs_bbox(&paths) else { return };
        let saved = self.gs.fill.clone();
        self.gs.fill = Paint::Grad(Rc::new(shade));
        let attrs = self.fill_attrs(&bbox, false);
        self.gs.fill = saved;
        let blend = self.blend_attr();
        self.emit(&format!(
            "<path d=\"{}\"{attrs} stroke=\"none\"{blend}/>",
            path_data(&paths)
        ));
    }

    // ───── XObjects ─────

    fn do_xobject(&mut self, name: &str, res: &Dict) {
        let pdf = self.pdf;
        let Some(xo) = pdf
            .get(res, "XObject")
            .dict()
            .and_then(|d| d.get(name))
            .map(|o| pdf.res(o))
        else {
            return;
        };
        let Obj::Stream(s) = xo else { return };
        match pdf.get(&s.dict, "Subtype").name() {
            Some("Image") => {
                self.flush_text();
                let s = s.clone();
                self.draw_image(&s.dict, &s.data, res, true);
            }
            Some("Form") => {
                if self.depth >= 12 {
                    return;
                }
                self.flush_text();
                let s = s.clone();
                let m = pdf.nums(pdf.get(&s.dict, "Matrix"));
                let saved_gs = self.gs.clone();
                let saved_stack = self.stack.len();
                let saved_path = std::mem::take(&mut self.path);
                if m.len() >= 6 {
                    let fm = Affine {
                        a: m[0],
                        b: m[1],
                        c: m[2],
                        d: m[3],
                        e: m[4],
                        f: m[5],
                    };
                    self.gs.ctm = self.gs.ctm.then_before(&fm);
                }
                let is_group = pdf.get(&s.dict, "Group").dict().is_some();
                let blend = css_blend(&self.gs.blend);
                let wrap = is_group && (self.gs.fa < 0.999 || blend.is_some());
                let open_layer = self.target();
                if wrap {
                    let mut tag = String::from("<g");
                    if self.gs.fa < 0.999 {
                        tag.push_str(&format!(" opacity=\"{}\"", num(self.gs.fa)));
                    }
                    if let Some(b) = &blend {
                        tag.push_str(&format!(" mix-blend-mode=\"{b}\""));
                    }
                    tag.push('>');
                    self.emit(&tag);
                    self.gs.fa = 1.0;
                    self.gs.sa = 1.0;
                    self.gs.blend = "Normal".into();
                }
                let fres: Dict = pdf
                    .get(&s.dict, "Resources")
                    .dict()
                    .cloned()
                    .unwrap_or_else(|| res.clone());
                let (content, _) = pdf.stream_data(&s);
                self.depth += 1;
                self.exec(&content, &fres);
                self.flush_text();
                self.depth -= 1;
                if wrap {
                    let b = &mut self.layers[open_layer].body;
                    b.push_str("    </g>\n");
                }
                self.stack.truncate(saved_stack);
                self.gs = saved_gs;
                self.path = saved_path;
            }
            _ => {}
        }
    }

    fn inline_image(&mut self, lex: &mut Lex, res: &Dict) {
        let mut dict = Dict::new();
        loop {
            match lex.next() {
                Tok::O(Obj::Name(k)) => match lex.next() {
                    Tok::O(v) => {
                        dict.insert(k, v);
                    }
                    Tok::Kw(k2) if k2 == "ID" => break,
                    _ => {}
                },
                Tok::Kw(k) if k == "ID" => break,
                Tok::Eof => return,
                _ => {}
            }
        }
        for (short, long) in [
            ("BPC", "BitsPerComponent"),
            ("CS", "ColorSpace"),
            ("D", "Decode"),
            ("DP", "DecodeParms"),
            ("F", "Filter"),
            ("H", "Height"),
            ("IM", "ImageMask"),
            ("W", "Width"),
        ] {
            if let Some(v) = dict.remove(short) {
                dict.entry(long.to_string()).or_insert(v);
            }
        }
        let d = lex.d;
        let start = (lex.pos + 1).min(d.len());
        let mut end = d.len();
        let mut i = start;
        while i + 1 < d.len() {
            if d[i] == b'E'
                && d[i + 1] == b'I'
                && (i == start || matches!(d[i - 1], 0 | 9 | 10 | 12 | 13 | 32))
                && d.get(i + 2)
                    .map(|&b| matches!(b, 0 | 9 | 10 | 12 | 13 | 32))
                    .unwrap_or(true)
            {
                end = i;
                break;
            }
            i += 1;
        }
        lex.pos = (end + 2).min(d.len());
        let data = d[start..end.max(start)].to_vec();
        self.flush_text();
        self.draw_image(&dict, &data, res, false);
    }

    fn draw_image(&mut self, dict: &Dict, raw: &[u8], res: &Dict, _xobj: bool) {
        let pdf = self.pdf;
        let strm = Strm {
            dict: dict.clone(),
            data: raw.to_vec(),
        };
        let w = pdf.num(dict, "Width").unwrap_or(0.0) as u32;
        let h = pdf.num(dict, "Height").unwrap_or(0.0) as u32;
        if w == 0 || h == 0 || (w as u64) * (h as u64) > 80_000_000 {
            return;
        }
        // Lage: Einheitsquadrat mit y nach oben → Bildpixel
        let m = self.gs.ctm.then_before(&Affine {
            a: 1.0 / w as f64,
            b: 0.0,
            c: 0.0,
            d: -1.0 / h as f64,
            e: 0.0,
            f: 1.0,
        });
        let unit = [
            Pt::new(0.0, 0.0),
            Pt::new(1.0, 0.0),
            Pt::new(1.0, 1.0),
            Pt::new(0.0, 1.0),
        ];
        let corners: Vec<Pt> = unit.iter().map(|p| self.gs.ctm.apply(*p)).collect();
        let bbox = corners
            .iter()
            .fold(rect_of(corners[0], corners[0]), |r, p| r.union(&rect_of(*p, *p)));
        let clip_attr = match self.clip_test(&bbox) {
            ClipTest::Outside => return,
            ClipTest::Inside => String::new(),
            ClipTest::Partial => {
                let c = self.gs.clip.clone().unwrap();
                format!(
                    " mondrian:clip=\"{}\"",
                    path_data(&boolean::rings_to_paths(c.rings.clone()))
                )
            }
        };
        let is_mask = matches!(pdf.get(dict, "ImageMask"), Obj::Bool(true));
        let (data, rest) = pdf.stream_data(&strm);
        let (mime, bytes): (&str, Vec<u8>) = if let Some(f) = rest {
            if (f == "DCTDecode" || f == "DCT") && !is_mask {
                match jpeg_components(&data) {
                    Some(1) | Some(3) => ("image/jpeg", data),
                    _ => {
                        self.note(tr("CMYK-JPEG-Bilder wurden übersprungen"));
                        return;
                    }
                }
            } else {
                self.note(tr(
                    "Bilder mit besonderer Kompression (JPEG 2000, JBIG2, CCITT) wurden übersprungen",
                ));
                return;
            }
        } else {
            let fill_rgb = self.gs.fill.flat();
            match self.decode_pixels(dict, &data, w, h, res, is_mask, fill_rgb) {
                Some(png) => ("image/png", png),
                None => return,
            }
        };
        if mime == "image/jpeg" && dict.contains_key("SMask") {
            self.note(tr("Transparenz von JPEG-Bildern wurde nicht übernommen"));
        }
        let blend = self.blend_attr();
        let op = if self.gs.fa < 0.999 {
            format!(" opacity=\"{}\"", num(self.gs.fa))
        } else {
            String::new()
        };
        let b64 = base64_encode(&bytes);
        let geom = if m.b.abs() < 1e-9 && m.c.abs() < 1e-9 && m.a > 0.0 && m.d > 0.0 {
            format!(
                "x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"",
                num(m.e),
                num(m.f),
                num(m.a * w as f64),
                num(m.d * h as f64)
            )
        } else {
            format!(
                "x=\"0\" y=\"0\" width=\"{w}\" height=\"{h}\" transform=\"matrix({} {} {} {} {} {})\"",
                num(m.a),
                num(m.b),
                num(m.c),
                num(m.d),
                num(m.e),
                num(m.f)
            )
        };
        self.emit(&format!(
            "<image {geom} preserveAspectRatio=\"none\"{op}{blend}{clip_attr} href=\"data:{mime};base64,{b64}\"/>"
        ));
    }

    #[allow(clippy::too_many_arguments)]
    fn decode_pixels(
        &mut self,
        dict: &Dict,
        data: &[u8],
        w: u32,
        h: u32,
        res: &Dict,
        is_mask: bool,
        fill_rgb: [f64; 3],
    ) -> Option<Vec<u8>> {
        let pdf = self.pdf;
        let (wu, hu) = (w as usize, h as usize);
        let decode = pdf.nums(pdf.get(dict, "Decode"));
        if is_mask {
            let stride = wu.div_ceil(8);
            let invert = decode.first().copied().unwrap_or(0.0) >= 0.5;
            let c = [
                (fill_rgb[0] * 255.0).round() as u8,
                (fill_rgb[1] * 255.0).round() as u8,
                (fill_rgb[2] * 255.0).round() as u8,
            ];
            let mut px = Vec::with_capacity(wu * hu * 4);
            for y in 0..hu {
                for x in 0..wu {
                    let bit = data
                        .get(y * stride + x / 8)
                        .map(|b| (b >> (7 - x % 8)) & 1)
                        .unwrap_or(1);
                    let paint = (bit == 0) != invert;
                    px.extend_from_slice(&[c[0], c[1], c[2], if paint { 255 } else { 0 }]);
                }
            }
            return Some(png_encode(w, h, 4, &px));
        }
        let bpc_raw = pdf.num(dict, "BitsPerComponent").unwrap_or(8.0) as usize;
        let cs = match dict.get("ColorSpace") {
            Some(o) => Cs::parse(pdf, o, res, 0),
            None => Cs::Gray,
        };
        let nc = cs.n().max(1);
        let (bpc, step) = if bpc_raw == 16 {
            (8, 2)
        } else {
            (bpc_raw.max(1), 1)
        };
        let rowbits = wu * nc * bpc_raw.max(1);
        let stride = rowbits.div_ceil(8);
        let maxv = ((1u32 << bpc) - 1) as f64;
        let indexed = matches!(cs, Cs::Indexed { .. });
        let sample = |row: &[u8], i: usize| -> u32 {
            match bpc_raw {
                8 => row.get(i).copied().unwrap_or(0) as u32,
                16 => row.get(i * 2).copied().unwrap_or(0) as u32,
                b => {
                    let bit = i * b;
                    let byte = row.get(bit / 8).copied().unwrap_or(0) as u32;
                    (byte >> (8 - b - bit % 8)) & ((1 << b) - 1)
                }
            }
        };
        let _ = step;
        let mut rgb = Vec::with_capacity(wu * hu * 3);
        let mut cache: HashMap<u32, [u8; 3]> = HashMap::new();
        let quick = bpc_raw == 8 && decode.is_empty();
        for y in 0..hu {
            let row = data
                .get(y * stride..)
                .map(|r| &r[..r.len().min(stride)])
                .unwrap_or(&[]);
            for x in 0..wu {
                if quick && matches!(cs, Cs::Rgb) {
                    rgb.extend_from_slice(&[
                        row.get(x * 3).copied().unwrap_or(0),
                        row.get(x * 3 + 1).copied().unwrap_or(0),
                        row.get(x * 3 + 2).copied().unwrap_or(0),
                    ]);
                    continue;
                }
                if quick && matches!(cs, Cs::Gray) {
                    let g = row.get(x).copied().unwrap_or(0);
                    rgb.extend_from_slice(&[g, g, g]);
                    continue;
                }
                let vals: Vec<u32> = (0..nc).map(|k| sample(row, x * nc + k)).collect();
                let key = if nc <= 4 {
                    vals.iter().fold(0u32, |a, &v| (a << 8) | (v & 0xFF))
                } else {
                    u32::MAX
                };
                if nc <= 4 && bpc <= 8 {
                    if let Some(c) = cache.get(&key) {
                        rgb.extend_from_slice(c);
                        continue;
                    }
                }
                let comps: Vec<f64> = vals
                    .iter()
                    .enumerate()
                    .map(|(k, &v)| {
                        if indexed {
                            v as f64
                        } else if decode.len() >= 2 * (k + 1) {
                            decode[2 * k] + v as f64 * (decode[2 * k + 1] - decode[2 * k]) / maxv
                        } else if let Cs::Lab { range, .. } = &cs {
                            let u = v as f64 / maxv;
                            match k {
                                0 => u * 100.0,
                                1 => range[0] + u * (range[1] - range[0]),
                                _ => range[2] + u * (range[3] - range[2]),
                            }
                        } else {
                            v as f64 / maxv
                        }
                    })
                    .collect();
                let c = cs.to_rgb(&comps);
                let px = [
                    (c[0] * 255.0).round() as u8,
                    (c[1] * 255.0).round() as u8,
                    (c[2] * 255.0).round() as u8,
                ];
                if nc <= 4 && bpc <= 8 && cache.len() < 70_000 {
                    cache.insert(key, px);
                }
                rgb.extend_from_slice(&px);
            }
        }
        // Transparenz: weiche Maske oder Stanzmaske
        let mut alpha: Option<Vec<u8>> = None;
        if let Obj::Stream(sm) = pdf.get(dict, "SMask") {
            let (sd, rest) = pdf.stream_data(sm);
            let (mw, mh) = (
                pdf.num(&sm.dict, "Width").unwrap_or(0.0) as usize,
                pdf.num(&sm.dict, "Height").unwrap_or(0.0) as usize,
            );
            if rest.is_none() && mw > 0 && mh > 0 {
                let mb = pdf.num(&sm.dict, "BitsPerComponent").unwrap_or(8.0) as usize;
                let mstride = (mw * mb.max(1)).div_ceil(8);
                let md = pdf.nums(pdf.get(&sm.dict, "Decode"));
                let inv = md.first().copied().unwrap_or(0.0) > md.get(1).copied().unwrap_or(1.0);
                let mut a = Vec::with_capacity(wu * hu);
                for y in 0..hu {
                    let my = (y * mh / hu).min(mh - 1);
                    let row = sd
                        .get(my * mstride..)
                        .map(|r| &r[..r.len().min(mstride)])
                        .unwrap_or(&[]);
                    for x in 0..wu {
                        let mx = (x * mw / wu).min(mw - 1);
                        let v = match mb {
                            8 => row.get(mx).copied().unwrap_or(255) as u32,
                            16 => row.get(mx * 2).copied().unwrap_or(255) as u32,
                            b => {
                                let bit = mx * b;
                                let byte = row.get(bit / 8).copied().unwrap_or(0) as u32;
                                let raw = (byte >> (8 - b - bit % 8)) & ((1 << b) - 1);
                                raw * 255 / ((1 << b) - 1)
                            }
                        } as u8;
                        a.push(if inv { 255 - v } else { v });
                    }
                }
                alpha = Some(a);
            }
        } else if let Obj::Stream(mk) = pdf.get(dict, "Mask") {
            let (sd, rest) = pdf.stream_data(mk);
            let (mw, mh) = (
                pdf.num(&mk.dict, "Width").unwrap_or(0.0) as usize,
                pdf.num(&mk.dict, "Height").unwrap_or(0.0) as usize,
            );
            if rest.is_none() && mw > 0 && mh > 0 {
                let mstride = mw.div_ceil(8);
                let md = pdf.nums(pdf.get(&mk.dict, "Decode"));
                let inv = md.first().copied().unwrap_or(0.0) >= 0.5;
                let mut a = Vec::with_capacity(wu * hu);
                for y in 0..hu {
                    let my = (y * mh / hu).min(mh - 1);
                    for x in 0..wu {
                        let mx = (x * mw / wu).min(mw - 1);
                        let bit = sd
                            .get(my * mstride + mx / 8)
                            .map(|b| (b >> (7 - mx % 8)) & 1)
                            .unwrap_or(0);
                        let painted = (bit == 0) != inv;
                        a.push(if painted { 255 } else { 0 });
                    }
                }
                alpha = Some(a);
            }
        }
        Some(match alpha {
            Some(a) => {
                let mut px = Vec::with_capacity(wu * hu * 4);
                for (c, al) in rgb.chunks(3).zip(a) {
                    px.extend_from_slice(&[c[0], c[1], c[2], al]);
                }
                png_encode(w, h, 4, &px)
            }
            None => png_encode(w, h, 3, &rgb),
        })
    }

    // ───── Text ─────

    fn set_font_obj(&mut self, fo: &Obj) {
        let pdf = self.pdf;
        let key = if let Obj::Ref(r) = fo { Some(*r) } else { None };
        if let Some(k) = key {
            if let Some(f) = self.fonts.get(&k) {
                self.gs.font = Some(f.clone());
                return;
            }
        }
        let Some(d) = pdf.res(fo).dict() else { return };
        let f = Rc::new(load_font(pdf, d, self.exists));
        if let Some(k) = key {
            self.fonts.insert(k, f.clone());
        }
        self.gs.font = Some(f);
    }

    fn next_line(&mut self) {
        self.tlm = self.tlm.then_before(&Affine::translate(0.0, -self.gs.tl));
        self.tm = self.tlm;
    }

    fn glyph_matrix(&self, tm: &Affine) -> Affine {
        let g = &self.gs;
        g.ctm
            .then_before(tm)
            .then_before(&Affine {
                a: g.fs * g.th,
                b: 0.0,
                c: 0.0,
                d: g.fs,
                e: 0.0,
                f: g.rise,
            })
            .then_before(&Affine::scale(1.0, -1.0))
    }

    fn show(&mut self, items: &[TextItem]) {
        let Some(font) = self.gs.font.clone() else { return };
        let start_tm = self.tm;
        let mut text = String::new();
        let mut undecodable = false;
        for it in items {
            match it {
                TextItem::Adj(v) => {
                    let tx = -v / 1000.0 * self.gs.fs * self.gs.th;
                    if -v > 250.0 && !text.ends_with(' ') && !text.is_empty() {
                        text.push(' ');
                    }
                    self.tm = self.tm.then_before(&Affine::translate(tx, 0.0));
                }
                TextItem::Str(s) => {
                    for (code, t, w) in font.decode(s) {
                        if t.is_empty() && code != 32 {
                            undecodable = true;
                        }
                        text.push_str(&t);
                        let mut tx = w / 1000.0 * self.gs.fs + self.gs.tc;
                        if code == 32 && !font.two {
                            tx += self.gs.tw;
                        }
                        self.tm = self.tm.then_before(&Affine::translate(tx * self.gs.th, 0.0));
                    }
                }
            }
        }
        if undecodable {
            self.note(tr(
                "Text mit unbekannter Zeichenkodierung wurde teilweise übersprungen",
            ));
        }
        if self.gs.tr == 3 || text.trim().is_empty() || self.gs.fs.abs() < 1e-9 {
            return;
        }
        if self.gs.tr >= 4 {
            return;
        }
        self.note(tr("Schriften werden durch installierte Schriften ersetzt; Zeilenumbrüche bleiben einzelne Textzeilen"));
        let gm = self.glyph_matrix(&start_tm);
        let s = (gm.a * gm.a + gm.b * gm.b).sqrt();
        if s < 0.05 {
            return;
        }
        let end = self.glyph_matrix(&self.tm).apply(Pt::new(0.0, 0.0));
        let origin = gm.apply(Pt::new(0.0, 0.0));
        let n = Affine {
            a: gm.a / s,
            b: gm.b / s,
            c: gm.c / s,
            d: gm.d / s,
            e: gm.e,
            f: gm.f,
        };
        let plain =
            (n.a - 1.0).abs() < 1e-3 && n.b.abs() < 1e-3 && n.c.abs() < 1e-3 && (n.d - 1.0).abs() < 1e-3;
        let fill_only = self.gs.tr == 0;
        let color = if self.gs.tr == 1 {
            self.gs.stroke.flat()
        } else {
            self.gs.fill.flat()
        };
        let mut head = String::from("<text");
        if plain {
            head.push_str(&format!(" x=\"{}\" y=\"{}\"", num(n.e), num(n.f)));
        } else {
            head.push_str(&format!(
                " x=\"0\" y=\"0\" transform=\"matrix({} {} {} {} {} {})\"",
                num(n.a),
                num(n.b),
                num(n.c),
                num(n.d),
                num(n.e),
                num(n.f)
            ));
        }
        head.push_str(&format!(
            " font-family=\"{}\" font-size=\"{}\"",
            escape(&font.family),
            num(s)
        ));
        if font.bold {
            head.push_str(" font-weight=\"bold\"");
        }
        if font.italic {
            head.push_str(" font-style=\"italic\"");
        }
        if fill_only || self.gs.tr == 2 {
            head.push_str(&format!(" fill=\"{}\"", hex(self.gs.fill.flat())));
            if self.gs.fa < 0.999 {
                head.push_str(&format!(" fill-opacity=\"{}\"", num(self.gs.fa)));
            }
        } else {
            head.push_str(" fill=\"none\"");
        }
        if self.gs.tr == 1 || self.gs.tr == 2 {
            head.push_str(&format!(
                " stroke=\"{}\" stroke-width=\"{}\"",
                hex(color),
                num((self.gs.lw * self.gs.ctm.mean_scale()).max(0.25))
            ));
        }
        let ratio = s / self.gs.fs.abs().max(1e-9);
        if self.gs.tc.abs() > 1e-6 {
            head.push_str(&format!(" letter-spacing=\"{}\"", num(self.gs.tc * ratio)));
        }
        head.push_str(&self.blend_attr());
        head.push_str(" xml:space=\"preserve\"");
        let sig = format!(
            "{}|{}|{}|{}|{:.3}|{:.3}|{:.3}|{:.3}|{:.3}|{}",
            font.family,
            font.bold,
            font.italic,
            hex(color),
            s,
            n.a,
            n.b,
            n.c,
            n.d,
            self.gs.tc
        );
        let layer = self.target();
        if let Some(p) = &mut self.pending {
            if p.sig == sig && p.layer == layer && p.end.dist(origin) < p.size * 0.35 {
                p.text.push_str(&text);
                p.end = end;
                return;
            }
        }
        self.flush_text();
        self.pending = Some(PText {
            sig,
            end,
            head,
            text,
            layer,
            size: s,
        });
    }

    fn flush_text(&mut self) {
        if let Some(p) = self.pending.take() {
            let line = format!("    {}>{}</text>\n", p.head, escape(&p.text));
            self.layers[p.layer].body.push_str(&line);
        }
    }
}

enum TextItem {
    Str(Vec<u8>),
    Adj(f64),
}

// ───────────────────────────── Einstieg ─────────────────────────────

fn box_of(pdf: &Pdf, page: &Dict) -> Rect {
    for key in ["CropBox", "MediaBox"] {
        let v = pdf.nums(pdf.get(page, key));
        if v.len() >= 4 && (v[2] - v[0]).abs() > 0.5 && (v[3] - v[1]).abs() > 0.5 {
            return rect_of(Pt::new(v[0], v[1]), Pt::new(v[2], v[3]));
        }
    }
    rect_of(Pt::new(0.0, 0.0), Pt::new(612.0, 792.0))
}

/// Liest eine Illustrator-Datei (PDF-kompatibel) und liefert SVG-Text.
/// `exists` fragt, ob eine Schriftfamilie installiert ist (Groß-/Kleinschreibung egal).
pub fn import(data: &[u8], exists: &dyn Fn(&str) -> bool) -> Result<Import, String> {
    let pdf = Pdf::load(data)?;
    let pages = pdf.pages();
    if pages.is_empty() {
        return Err(tr("Die Datei enthält keine Seiten").into());
    }
    let mut ix = Interp::new(&pdf, exists);
    // ausgeblendete Ebenen
    if let Some(ocp) = pdf.get(&pdf.root, "OCProperties").dict() {
        if let Some(d) = pdf.get(ocp, "D").dict() {
            if let Some(Obj::Arr(list)) = d.get("OFF").map(|o| pdf.res(o)) {
                for o in list {
                    if let Obj::Ref(r) = o {
                        ix.off.insert(*r);
                    }
                }
            }
        }
    }
    let mut x_off = 0.0;
    let mut max_h: f64 = 0.0;
    let mut boards: Vec<(f64, f64, f64, f64)> = Vec::new();
    for page in pages.iter().take(MAX_PAGES) {
        let b = box_of(&pdf, page);
        let (w0, h0) = (b.width(), b.height());
        let rot = pdf.num(page, "Rotate").unwrap_or(0.0) as i32;
        let rot = rot.rem_euclid(360);
        let flip = Affine {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: -1.0,
            e: -b.x0,
            f: b.y1,
        };
        let (rm, w, h) = match rot {
            90 => (
                Affine {
                    a: 0.0,
                    b: 1.0,
                    c: -1.0,
                    d: 0.0,
                    e: h0,
                    f: 0.0,
                },
                h0,
                w0,
            ),
            180 => (
                Affine {
                    a: -1.0,
                    b: 0.0,
                    c: 0.0,
                    d: -1.0,
                    e: w0,
                    f: h0,
                },
                w0,
                h0,
            ),
            270 => (
                Affine {
                    a: 0.0,
                    b: -1.0,
                    c: 1.0,
                    d: 0.0,
                    e: 0.0,
                    f: w0,
                },
                h0,
                w0,
            ),
            _ => (Affine::IDENTITY, w0, h0),
        };
        let base = Affine::translate(x_off, 0.0).then_before(&rm).then_before(&flip);
        ix.base = base;
        ix.gs = Gs::new(base);
        ix.stack.clear();
        ix.path.clear();
        ix.page = rect_of(Pt::new(x_off, 0.0), Pt::new(x_off + w, h));
        ix.cur = None;
        ix.mc.clear();
        ix.oc_depth = 0;
        let res: Dict = pdf.get(page, "Resources").dict().cloned().unwrap_or_default();
        let content = pdf.page_content(page);
        ix.exec(&content, &res);
        ix.flush_text();
        boards.push((x_off, 0.0, w, h));
        x_off += w + PAGE_GAP;
        max_h = max_h.max(h);
    }
    let total_w = (x_off - PAGE_GAP).max(1.0);
    let mut o = String::new();
    o.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    o.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:inkscape=\"http://www.inkscape.org/namespaces/inkscape\" \
xmlns:sodipodi=\"http://sodipodi.sourceforge.net/DTD/sodipodi-0.dtd\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
xmlns:mondrian=\"https://mondrian.invalid/ns\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\" mondrian:unit=\"px\">\n",
        w = num(total_w),
        h = num(max_h.max(1.0))
    ));
    if boards.len() > 1 {
        for (i, (x, y, w, h)) in boards.iter().enumerate() {
            o.push_str(&format!(
                "  <mondrian:artboard x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" name=\"Zeichenfläche {}\"/>\n",
                num(*x),
                num(*y),
                num(*w),
                num(*h),
                i + 1
            ));
        }
    }
    let mut count = 0;
    for l in ix.layers.iter().filter(|l| !l.body.is_empty()) {
        count += 1;
        o.push_str(&format!(
            "  <g id=\"layer{count}\" inkscape:groupmode=\"layer\" inkscape:label=\"{}\"{}>\n",
            escape(&l.name),
            if l.hidden { " style=\"display:none\"" } else { "" }
        ));
        o.push_str(&l.body);
        o.push_str("  </g>\n");
    }
    o.push_str("</svg>\n");
    if count == 0 {
        ix.notes
            .insert(tr("Auf der ersten Seite wurden keine darstellbaren Inhalte gefunden").into());
    }
    if pages.len() > MAX_PAGES {
        ix.notes
            .insert(format!("Es wurden nur die ersten {MAX_PAGES} Seiten gelesen."));
    }
    Ok(Import {
        svg: o,
        notes: ix.notes.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }

    /// Einfaches PDF aus Seiteninhalt und zusätzlichen Objekten.
    fn make_pdf(content: &str, resources: &str, extra: &str) -> Vec<u8> {
        format!(
            "%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R {extra} >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << {resources} >> >>\nendobj\n\
4 0 obj\n<< /Length {} >>\nstream\n{content}\nendstream\nendobj\n\
trailer\n<< /Root 1 0 R >>\n%%EOF\n",
            content.len()
        )
        .into_bytes()
    }

    fn import_str(pdf: &[u8]) -> Import {
        import(pdf, &|_| false).expect("Import")
    }

    #[test]
    fn inflate_dynamic_block() {
        let packed = unhex("78da1592010ec5200843afe2d5c415252a2620f1fa9f9f6cd99615db3e9df79d0633ebd13a35008a450121501c3ac0c319f4a8f625ba29e520a0e2afb87a047a177deaf14e882b642874790ef6d9beda2e300a24d771faae09225f75fe355835d7920ef0471f57e8c63bb63a32d102ef709e8757112be8c4198d5c5438e6a5fe8fa13ce83c90a5cdf665ebeff3daf441b25d4bbf6094aa6bab123752c2b0fd1053d3bd8bf30bfd8630f78d6a45ab293f43dffe455f6688058b1dd9b7d0a80443fbc600daf813b020b300cffc02dc5eef451a53c406bec052816fa1e3102f7c8c0f93e28e72b2a3500bcd2ae733bdb759529afc163513bdb022f4e249186b1a677851c5aedb831eaea43ae66e4350a572322cb97b8eace8781bd793b5e63d96b929f2ca9e558c7ced6a12f814215325516dd39e3e33918aad4b7676f6264e134b19345035ff36b9f9c87ce8b9192747e1b39f828f23274e955d21c5290f8eeab044725323ed818910d7a04d446ca470196e22cef397f1d3466fd28a3c450732dbaa12df198c0c926c6d563f355b64c4fb205b37ff00a28b1e85");
        let out = crate::pdf::zlib_decode(&packed);
        let letters: Vec<char> = "eeeeeenniiisrartdhulcgmo bbfwk".chars().collect();
        let mut s: u64 = 7;
        let mut expect = String::new();
        for _ in 0..700 {
            s = (s * 1103515245 + 12345) & 0x7fff_ffff;
            expect.push(letters[((s >> 8) as usize) % letters.len()]);
        }
        assert_eq!(String::from_utf8(out).unwrap(), expect);
    }

    #[test]
    fn inflate_fixed_and_stored() {
        let fixed = unhex("78dacb48cdc9c957c8402775147cf3f3528a3213f3143125c9520200d69d2560");
        let want = "hello hello hello hello, Mondrian! ".repeat(3);
        assert_eq!(crate::pdf::zlib_decode(&fixed), want.as_bytes());
        let stored = png_encode(2, 1, 3, &[1, 2, 3, 4, 5, 6]);
        // IDAT enthält unkomprimierte Blöcke
        let pos = stored.windows(4).position(|w| w == b"IDAT").unwrap();
        let len = u32::from_be_bytes(stored[pos - 4..pos].try_into().unwrap()) as usize;
        let z = &stored[pos + 4..pos + 4 + len];
        assert_eq!(crate::pdf::zlib_decode(z), vec![0, 1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn cmyk_colors() {
        let white = crate::pdf::cmyk_to_rgb(0.0, 0.0, 0.0, 0.0);
        assert!(white.iter().all(|v| *v > 0.99));
        let black = crate::pdf::cmyk_to_rgb(0.0, 0.0, 0.0, 1.0);
        assert!(black.iter().all(|v| *v < 0.25));
        let red = crate::pdf::cmyk_to_rgb(0.0, 1.0, 1.0, 0.0);
        assert!(red[0] > 0.9 && red[1] < 0.3 && red[2] < 0.3);
    }

    #[test]
    fn rectangle_flips_y_and_reads_back() {
        let pdf = make_pdf("1 0 0 rg 10 20 50 30 re f", "", "");
        let imp = import_str(&pdf);
        assert!(
            imp.svg.contains("M 10 80 L 60 80 L 60 50 L 10 50 Z"),
            "{}",
            imp.svg
        );
        let doc = crate::svg::read(&imp.svg).unwrap();
        assert_eq!((doc.width, doc.height), (200.0, 100.0));
        let objs: Vec<_> = doc.layers.iter().flat_map(|l| l.objects.iter()).collect();
        assert_eq!(objs.len(), 1);
        assert_eq!(objs[0].style.fill.unwrap().to_hex(), "#ff0000");
    }

    #[test]
    fn stroke_width_follows_matrix() {
        let pdf = make_pdf("2 0 0 2 0 0 cm 3 w 0 0 1 RG 5 5 m 20 5 l S", "", "");
        let svg = import_str(&pdf).svg;
        assert!(svg.contains("stroke-width=\"6\""), "{svg}");
        assert!(svg.contains("fill=\"none\""));
    }

    #[test]
    fn clip_cuts_fill() {
        // Kreisförmiger Ausschnitt: ein großes Rechteck wird auf die Form zurechtgeschnitten
        let pdf = make_pdf(
            "q 100 50 m 140 50 140 90 100 90 c 60 90 60 50 100 50 c W n 0 g 0 0 200 100 re f Q 1 0 0 rg 0 0 5 5 re f",
            "",
            "",
        );
        let doc = crate::svg::read(&import_str(&pdf).svg).unwrap();
        let objs: Vec<_> = doc.layers.iter().flat_map(|l| l.objects.iter()).collect();
        assert_eq!(objs.len(), 2);
        // zugeschnitten: nicht mehr das ganze Blatt
        let b = crate::model::Shape::geom_bbox(&objs[0].shape).unwrap();
        assert!(b.width() < 100.0 && b.height() < 100.0, "{:?}", b);
    }

    #[test]
    fn page_sized_clip_is_ignored() {
        let pdf = make_pdf("0 0 200 100 re W n -20 -20 300 140 re f", "", "");
        let doc = crate::svg::read(&import_str(&pdf).svg).unwrap();
        let objs: Vec<_> = doc.layers.iter().flat_map(|l| l.objects.iter()).collect();
        let b = crate::model::Shape::geom_bbox(&objs[0].shape).unwrap();
        assert!((b.width() - 300.0).abs() < 1e-6);
    }

    #[test]
    fn gradient_from_pattern() {
        let res = "/Pattern << /P0 5 0 R >>";
        let extra = "";
        let mut pdf = make_pdf("/Pattern cs /P0 scn 0 0 200 100 re f", res, extra);
        let tail = b"5 0 obj\n<< /PatternType 2 /Shading << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 200 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> >> >>\nendobj\n";
        let at = pdf.windows(7).position(|w| w == b"trailer").unwrap();
        pdf.splice(at..at, tail.iter().copied());
        let doc = crate::svg::read(&import_str(&pdf).svg).unwrap();
        let o = doc.layers.iter().flat_map(|l| l.objects.iter()).next().unwrap();
        let g = o.style.gradient.as_ref().expect("Verlauf");
        assert_eq!(g.stops.len(), 2);
        assert_eq!(g.stops[0].color.to_hex(), "#ff0000");
        assert_eq!(g.stops[1].color.to_hex(), "#0000ff");
    }

    #[test]
    fn layers_and_hidden_layers() {
        let res = "/Properties << /MC0 5 0 R /MC1 6 0 R >>";
        let mut pdf = make_pdf(
            "/OC /MC0 BDC 0 g 0 0 10 10 re f EMC /OC /MC1 BDC 1 g 20 0 10 10 re f EMC",
            res,
            "/OCProperties << /OCGs [5 0 R 6 0 R] /D << /OFF [6 0 R] >> >>",
        );
        let tail = b"5 0 obj\n<< /Type /OCG /Name (Unten) >>\nendobj\n6 0 obj\n<< /Type /OCG /Name (Oben) >>\nendobj\n";
        let at = pdf.windows(7).position(|w| w == b"trailer").unwrap();
        pdf.splice(at..at, tail.iter().copied());
        let doc = crate::svg::read(&import_str(&pdf).svg).unwrap();
        assert_eq!(doc.layers.len(), 2);
        assert_eq!(doc.layers[0].name, "Unten");
        assert!(doc.layers[0].visible);
        assert_eq!(doc.layers[1].name, "Oben");
        assert!(!doc.layers[1].visible);
    }

    #[test]
    fn text_uses_tounicode_and_position() {
        let res = "/Font << /F0 5 0 R >>";
        let mut pdf = make_pdf("BT /F0 20 Tf 10 30 Td <0102> Tj ET", res, "");
        let cmap = "1 begincodespacerange <00> <FF> endcodespacerange 2 beginbfchar <01> <0048> <02> <0069> endbfchar";
        let tail = format!(
            "5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+Arial-BoldMT /FirstChar 1 /LastChar 2 /Widths [700 300] /ToUnicode 6 0 R >>\nendobj\n6 0 obj\n<< /Length {} >>\nstream\n{cmap}\nendstream\nendobj\n",
            cmap.len()
        );
        let at = pdf.windows(7).position(|w| w == b"trailer").unwrap();
        pdf.splice(at..at, tail.bytes());
        let imp = import_str(&pdf);
        assert!(imp.svg.contains(">Hi</text>"), "{}", imp.svg);
        assert!(imp.svg.contains("font-family=\"Arial\""));
        assert!(imp.svg.contains("font-weight=\"bold\""));
        assert!(imp.svg.contains("x=\"10\" y=\"70\""));
        let doc = crate::svg::read(&imp.svg).unwrap();
        assert_eq!(doc.layers.iter().map(|l| l.objects.len()).sum::<usize>(), 1);
    }

    #[test]
    fn transparency_group_and_blend() {
        let res = "/ExtGState << /G0 << /ca 0.5 /BM /Multiply >> >> /XObject << /X0 5 0 R >>";
        let mut pdf = make_pdf("/G0 gs /X0 Do", res, "");
        let form = "0 g 0 0 10 10 re f";
        let tail = format!(
            "5 0 obj\n<< /Type /XObject /Subtype /Form /BBox [0 0 50 50] /Group << /S /Transparency >> /Length {} >>\nstream\n{form}\nendstream\nendobj\n",
            form.len()
        );
        let at = pdf.windows(7).position(|w| w == b"trailer").unwrap();
        pdf.splice(at..at, tail.bytes());
        let svg = import_str(&pdf).svg;
        assert!(
            svg.contains("<g opacity=\"0.5\" mix-blend-mode=\"multiply\">"),
            "{svg}"
        );
        assert!(svg.contains("</g>"));
        assert!(crate::svg::read(&svg).is_ok());
    }

    #[test]
    fn broken_input_is_an_error_not_a_panic() {
        assert!(import(b"hallo", &|_| false).is_err());
        assert!(import(b"%PDF-1.4\ngarbage", &|_| false).is_err());
        let enc =
            b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<< /Root 1 0 R /Encrypt 9 0 R >>\n";
        let e = import(enc, &|_| false).err().unwrap();
        assert!(e.contains("verschlüsselt"));
    }

    #[test]
    fn looks_like_ai_detects_pdf_header() {
        assert!(looks_like_ai(b"%PDF-1.6\n..."));
        assert!(looks_like_ai(b"%!PS-Adobe-3.0 \n%PDF-1.5"));
        assert!(!looks_like_ai(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"));
    }
}
