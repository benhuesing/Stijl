//! Druckfertige PDF-Dateien (ohne GTK): Cairo schreibt das PDF in RGB, diese Stufe rechnet
//! alle Farben nach CMYK um und macht auf Wunsch eine PDF/X-4-Datei daraus
//! (Ausgabebedingung mit eingebettetem Profil, Endformat- und Anschnittrahmen, XMP-Daten).
//!
//! Umgerechnet werden Farben in Seiteninhalten, Formularen und Mustern (`rg`, `RG`,
//! `sc`/`scn` in RGB), Verläufe (Funktionen vom Typ 0, 2 und 3), RGB-Bilder (8 Bit) und
//! Transparenzgruppen. Graustufen (z. B. Masken) bleiben Grau.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::icc::CmykProfile;
use crate::pdf::{Dict, Lex, Obj, Pdf, Strm, Tok};

// ───────────────────────────── Deflate ─────────────────────────────

struct BitOut {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitOut {
    fn put(&mut self, v: u32, bits: u32) {
        self.acc |= (v as u64) << self.n;
        self.n += bits;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }
    /// Huffman-Codes werden mit dem höchsten Bit zuerst geschrieben.
    fn put_rev(&mut self, code: u32, bits: u32) {
        let mut r = 0;
        for i in 0..bits {
            r |= ((code >> i) & 1) << (bits - 1 - i);
        }
        self.put(r, bits);
    }
    fn flush(&mut self) {
        if self.n > 0 {
            self.out.push(self.acc as u8);
            self.acc = 0;
            self.n = 0;
        }
    }
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195,
    227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073,
    4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13,
];

fn put_lit(b: &mut BitOut, v: u16) {
    match v {
        0..=143 => b.put_rev(0x30 + v as u32, 8),
        144..=255 => b.put_rev(0x190 + (v as u32 - 144), 9),
        256..=279 => b.put_rev(v as u32 - 256, 7),
        _ => b.put_rev(0xC0 + (v as u32 - 280), 8),
    }
}

/// Daten mit Deflate (feste Huffman-Codes, LZ77 mit Hash-Ketten) im zlib-Format packen.
pub fn zlib_encode(data: &[u8]) -> Vec<u8> {
    let mut b = BitOut {
        out: vec![0x78, 0x9C],
        acc: 0,
        n: 0,
    };
    b.put(1, 1); // letzter Block
    b.put(1, 2); // feste Codes
    const WIN: usize = 32768;
    const HBITS: u32 = 15;
    let mut head = vec![usize::MAX; 1 << HBITS];
    let mut prev = vec![usize::MAX; WIN];
    let hash = |i: usize| -> usize {
        let v = (data[i] as u32) << 16 | (data[i + 1] as u32) << 8 | data[i + 2] as u32;
        (v.wrapping_mul(2654435761) >> (32 - HBITS)) as usize
    };
    let n = data.len();
    let mut i = 0;
    let insert = |i: usize, head: &mut Vec<usize>, prev: &mut Vec<usize>| {
        if i + 2 < n {
            let h = hash(i);
            prev[i % WIN] = head[h];
            head[h] = i;
        }
    };
    while i < n {
        let mut best_len = 0;
        let mut best_dist = 0;
        if i + 2 < n {
            let h = hash(i);
            let mut cand = head[h];
            let mut chain = 64;
            while cand != usize::MAX && chain > 0 && i - cand <= WIN - 1 && cand < i {
                let max = (n - i).min(258);
                let mut l = 0;
                while l < max && data[cand + l] == data[i + l] {
                    l += 1;
                }
                if l > best_len {
                    best_len = l;
                    best_dist = i - cand;
                    if l == max {
                        break;
                    }
                }
                let nx = prev[cand % WIN];
                if nx == usize::MAX || nx >= cand {
                    break;
                }
                cand = nx;
                chain -= 1;
            }
        }
        if best_len >= 3 {
            let li = LEN_BASE
                .iter()
                .rposition(|&x| x as usize <= best_len)
                .unwrap_or(0);
            put_lit(&mut b, 257 + li as u16);
            b.put((best_len - LEN_BASE[li] as usize) as u32, LEN_EXTRA[li] as u32);
            let di = DIST_BASE
                .iter()
                .rposition(|&x| x as usize <= best_dist)
                .unwrap_or(0);
            b.put_rev(di as u32, 5);
            b.put((best_dist - DIST_BASE[di] as usize) as u32, DIST_EXTRA[di] as u32);
            for k in 0..best_len {
                insert(i + k, &mut head, &mut prev);
            }
            i += best_len;
        } else {
            put_lit(&mut b, data[i] as u16);
            insert(i, &mut head, &mut prev);
            i += 1;
        }
    }
    put_lit(&mut b, 256);
    b.flush();
    // Adler-32
    let (mut s1, mut s2) = (1u32, 0u32);
    for &x in data {
        s1 = (s1 + x as u32) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    b.out.extend_from_slice(&((s2 << 16) | s1).to_be_bytes());
    b.out
}

// ───────────────────────────── Schreiben ─────────────────────────────

fn num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v:.5}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        if s == "-0" {
            "0".into()
        } else {
            s.to_string()
        }
    }
}

fn name(n: &str) -> String {
    let mut s = String::from("/");
    for b in n.bytes() {
        if b <= b' ' || b >= 0x7F || b"()<>[]{}/%#".contains(&b) {
            s.push_str(&format!("#{b:02X}"));
        } else {
            s.push(b as char);
        }
    }
    s
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2 + 2);
    s.push('<');
    for b in bytes {
        s.push_str(&format!("{b:02X}"));
    }
    s.push('>');
    s
}

/// Objekt als PDF-Text (Datenströme getrennt, siehe `write_object`).
fn ser(o: &Obj, out: &mut String) {
    match o {
        Obj::Null => out.push_str("null"),
        Obj::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Obj::Num(n) => out.push_str(&num(*n)),
        Obj::Str(s) => out.push_str(&hex(s)),
        Obj::Name(n) => out.push_str(&name(n)),
        Obj::Arr(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                ser(x, out);
            }
            out.push(']');
        }
        Obj::Dict(d) => ser_dict(d, out),
        Obj::Stream(s) => ser_dict(&s.dict, out),
        Obj::Ref(n) => out.push_str(&format!("{n} 0 R")),
    }
}

fn ser_dict(d: &Dict, out: &mut String) {
    out.push_str("<<");
    let mut keys: Vec<&String> = d.keys().collect();
    keys.sort();
    for k in keys {
        out.push_str(&name(k));
        out.push(' ');
        ser(&d[k], out);
        out.push(' ');
    }
    out.push_str(">>");
}

fn write_object(id: u32, o: &Obj, out: &mut Vec<u8>) {
    out.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
    match o {
        Obj::Stream(s) => {
            let mut d = s.dict.clone();
            d.insert("Length".into(), Obj::Num(s.data.len() as f64));
            let mut t = String::new();
            ser_dict(&d, &mut t);
            out.extend_from_slice(t.as_bytes());
            out.extend_from_slice(b"\nstream\n");
            out.extend_from_slice(&s.data);
            out.extend_from_slice(b"\nendstream");
        }
        _ => {
            let mut t = String::new();
            ser(o, &mut t);
            out.extend_from_slice(t.as_bytes());
        }
    }
    out.extend_from_slice(b"\nendobj\n");
}

// ───────────────────────────── Farben ─────────────────────────────

/// Umrechnung RGB → CMYK (Profil oder einfache Formel), mit Zwischenspeicher für Bilder.
struct Conv<'a> {
    profile: Option<&'a CmykProfile>,
    cache: HashMap<u32, [u8; 4]>,
}

impl Conv<'_> {
    fn cmyk(&self, r: f64, g: f64, b: f64) -> [f64; 4] {
        match self.profile {
            Some(p) => p.convert(r, g, b),
            None => crate::icc::naive_cmyk(r, g, b),
        }
    }
    fn cmyk8(&mut self, r: u8, g: u8, b: u8) -> [u8; 4] {
        let key = (r as u32) << 16 | (g as u32) << 8 | b as u32;
        if let Some(v) = self.cache.get(&key) {
            return *v;
        }
        let c = self.cmyk(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
        let v = c.map(|x| (x * 255.0).round().clamp(0.0, 255.0) as u8);
        if self.cache.len() < 1 << 20 {
            self.cache.insert(key, v);
        }
        v
    }
}

fn nums(o: &[Obj]) -> Option<Vec<f64>> {
    o.iter().map(|x| x.num()).collect()
}

/// Inhaltsstrom umschreiben: RGB-Farboperatoren werden zu CMYK.
fn convert_content(data: &[u8], conv: &Conv) -> Result<Vec<u8>, String> {
    let mut lex = Lex::new(data, false);
    let mut out = String::with_capacity(data.len() + data.len() / 4);
    let mut ops: Vec<Obj> = Vec::new();
    // aktueller Farbraum (Füllung, Kontur) als RGB? – mit q/Q gesichert
    let mut rgb_cs = (false, false);
    let mut stack: Vec<(bool, bool)> = Vec::new();
    let flush = |ops: &mut Vec<Obj>, out: &mut String| {
        for o in ops.drain(..) {
            ser(&o, out);
            out.push(' ');
        }
    };
    loop {
        match lex.next() {
            Tok::O(o) => ops.push(o),
            Tok::Kw(k) => {
                let n = ops.len();
                let conv4 = |ops: &mut Vec<Obj>| -> bool {
                    if ops.len() < 3 {
                        return false;
                    }
                    let Some(v) = nums(&ops[ops.len() - 3..]) else {
                        return false;
                    };
                    let c = conv.cmyk(v[0], v[1], v[2]);
                    ops.truncate(ops.len() - 3);
                    ops.extend(c.iter().map(|x| Obj::Num((x * 10000.0).round() / 10000.0)));
                    true
                };
                match k.as_str() {
                    "BI" => return Err("eingebettete Bilder im Inhalt werden nicht unterstützt".into()),
                    "q" => stack.push(rgb_cs),
                    "Q" => rgb_cs = stack.pop().unwrap_or((false, false)),
                    "rg" | "RG" if n >= 3 => {
                        if conv4(&mut ops) {
                            flush(&mut ops, &mut out);
                            out.push_str(if k == "rg" { "k\n" } else { "K\n" });
                            continue;
                        }
                    }
                    "cs" | "CS" if n >= 1 => {
                        let is_rgb = matches!(&ops[n - 1], Obj::Name(x) if x == "DeviceRGB" || x == "CalRGB");
                        if is_rgb {
                            ops[n - 1] = Obj::Name("DeviceCMYK".into());
                        }
                        if k == "cs" {
                            rgb_cs.0 = is_rgb;
                        } else {
                            rgb_cs.1 = is_rgb;
                        }
                    }
                    "sc" | "scn" if rgb_cs.0 && n >= 3 => {
                        conv4(&mut ops);
                    }
                    "SC" | "SCN" if rgb_cs.1 && n >= 3 => {
                        conv4(&mut ops);
                    }
                    _ => {}
                }
                flush(&mut ops, &mut out);
                out.push_str(&k);
                out.push('\n');
            }
            Tok::ArrEnd | Tok::DictEnd => {}
            Tok::Eof => break,
        }
    }
    flush(&mut ops, &mut out);
    Ok(out.into_bytes())
}

/// Ausgabewerte einer Funktion (Verlauf) von RGB nach CMYK.
fn convert_function(pdf: &mut Pdf, f: &Obj, conv: &Conv, done: &mut HashSet<u32>) {
    let (id, mut obj) = match f {
        Obj::Ref(n) => {
            if !done.insert(*n) {
                return;
            }
            match pdf.objs.get(n) {
                Some(o) => (Some(*n), o.clone()),
                None => return,
            }
        }
        o => (None, o.clone()),
    };
    let dict = match &obj {
        Obj::Dict(d) => d.clone(),
        Obj::Stream(s) => s.dict.clone(),
        _ => return,
    };
    let ftype = dict.get("FunctionType").and_then(Obj::num).unwrap_or(-1.0) as i32;
    let mut nd = dict.clone();
    match ftype {
        2 => {
            for key in ["C0", "C1"] {
                let v = match dict.get(key).map(|x| pdf.res(x).clone()) {
                    Some(Obj::Arr(a)) => nums(&a).unwrap_or_default(),
                    _ if key == "C0" => vec![0.0, 0.0, 0.0],
                    _ => vec![1.0, 1.0, 1.0],
                };
                if v.len() == 3 {
                    let c = conv.cmyk(v[0], v[1], v[2]);
                    nd.insert(key.into(), Obj::Arr(c.iter().map(|x| Obj::Num(*x)).collect()));
                }
            }
            if nd.contains_key("Range") {
                nd.insert(
                    "Range".into(),
                    Obj::Arr((0..4).flat_map(|_| [Obj::Num(0.0), Obj::Num(1.0)]).collect()),
                );
            }
        }
        3 => {
            if let Some(Obj::Arr(fs)) = dict.get("Functions").map(|x| pdf.res(x).clone()) {
                let mut new = Vec::new();
                for sub in &fs {
                    match sub {
                        Obj::Ref(_) => {
                            convert_function(pdf, sub, conv, done);
                            new.push(sub.clone());
                        }
                        other => {
                            // eingebettete Funktion: als eigenes Objekt umrechnen
                            let nid = pdf.objs.keys().max().copied().unwrap_or(0) + 1;
                            pdf.objs.insert(nid, other.clone());
                            convert_function(pdf, &Obj::Ref(nid), conv, done);
                            new.push(Obj::Ref(nid));
                        }
                    }
                }
                nd.insert("Functions".into(), Obj::Arr(new));
            }
            if nd.contains_key("Range") {
                nd.insert(
                    "Range".into(),
                    Obj::Arr((0..4).flat_map(|_| [Obj::Num(0.0), Obj::Num(1.0)]).collect()),
                );
            }
        }
        0 => {
            // abgetastete Funktion: Werte entpacken und umrechnen
            if let Obj::Stream(s) = &obj {
                let bps = dict.get("BitsPerSample").and_then(Obj::num).unwrap_or(8.0) as usize;
                let range = dict.get("Range").and_then(|r| nums(r.arr()?)).unwrap_or_default();
                if bps == 8 && range.len() == 6 {
                    let (data, _) = pdf.stream_data(s);
                    let mut out = Vec::with_capacity(data.len() / 3 * 4);
                    for px in data.chunks_exact(3) {
                        let v: Vec<f64> = (0..3)
                            .map(|i| range[2 * i] + px[i] as f64 / 255.0 * (range[2 * i + 1] - range[2 * i]))
                            .collect();
                        let c = conv.cmyk(v[0], v[1], v[2]);
                        out.extend(c.iter().map(|x| (x * 255.0).round() as u8));
                    }
                    nd.insert(
                        "Range".into(),
                        Obj::Arr((0..4).flat_map(|_| [Obj::Num(0.0), Obj::Num(1.0)]).collect()),
                    );
                    nd.insert("Filter".into(), Obj::Name("FlateDecode".into()));
                    nd.remove("DecodeParms");
                    nd.remove("Decode");
                    obj = Obj::Stream(Rc::new(Strm {
                        dict: nd,
                        data: zlib_encode(&out),
                    }));
                    if let Some(id) = id {
                        pdf.objs.insert(id, obj);
                    }
                    return;
                }
            }
        }
        _ => {}
    }
    let new = match obj {
        Obj::Stream(s) => Obj::Stream(Rc::new(Strm {
            dict: nd,
            data: s.data.clone(),
        })),
        _ => Obj::Dict(nd),
    };
    if let Some(id) = id {
        pdf.objs.insert(id, new);
    }
}

/// RGB-Bild (8 Bit) nach CMYK.
fn convert_image(pdf: &Pdf, s: &Strm, conv: &mut Conv) -> Option<Obj> {
    let bpc = s.dict.get("BitsPerComponent").and_then(Obj::num).unwrap_or(8.0) as usize;
    if bpc != 8 {
        return None;
    }
    let (data, special) = pdf.stream_data(s);
    if special.is_some() {
        return None;
    }
    let mut out = Vec::with_capacity(data.len() / 3 * 4);
    for px in data.chunks_exact(3) {
        out.extend_from_slice(&conv.cmyk8(px[0], px[1], px[2]));
    }
    let mut d = s.dict.clone();
    d.insert("ColorSpace".into(), Obj::Name("DeviceCMYK".into()));
    d.insert("Filter".into(), Obj::Name("FlateDecode".into()));
    d.remove("DecodeParms");
    d.remove("Decode");
    Some(Obj::Stream(Rc::new(Strm {
        dict: d,
        data: zlib_encode(&out),
    })))
}

fn is_rgb_name(o: &Obj) -> bool {
    matches!(o, Obj::Name(n) if n == "DeviceRGB" || n == "CalRGB" || n == "RGB")
}

/// Farbräume und Gruppen in einem Wörterbuch (rekursiv, ohne Verweise) umstellen;
/// Verläufe merken, deren Funktionen umgerechnet werden müssen.
fn fix_dict(d: &mut Dict, functions: &mut Vec<Obj>, pdfx: bool) {
    if pdfx {
        // PDF/X erlaubt keine Bildglättung
        d.remove("Interpolate");
    }
    let shading = d.contains_key("ShadingType");
    if d.get("ColorSpace").is_some_and(is_rgb_name) && !d.contains_key("Width") {
        d.insert("ColorSpace".into(), Obj::Name("DeviceCMYK".into()));
        if shading {
            if let Some(f) = d.get("Function") {
                match f {
                    Obj::Arr(a) => functions.extend(a.iter().cloned()),
                    other => functions.push(other.clone()),
                }
            }
            if let Some(Obj::Arr(bg)) = d.get("Background") {
                if bg.len() == 3 {
                    d.remove("Background");
                }
            }
        }
    }
    // Transparenzgruppen
    if d.get("S").and_then(Obj::name) == Some("Transparency") && d.get("CS").is_some_and(is_rgb_name) {
        d.insert("CS".into(), Obj::Name("DeviceCMYK".into()));
    }
    // Hintergrund einer Luminanzmaske mit drei Werten
    if let Some(Obj::Arr(bc)) = d.get("BC") {
        if bc.len() == 3 {
            d.remove("BC");
        }
    }
    let keys: Vec<String> = d.keys().cloned().collect();
    for k in keys {
        if let Some(v) = d.get_mut(&k) {
            fix_value(v, functions, pdfx);
        }
    }
}

fn fix_value(v: &mut Obj, functions: &mut Vec<Obj>, pdfx: bool) {
    match v {
        Obj::Dict(d) => fix_dict(d, functions, pdfx),
        Obj::Arr(a) => {
            for x in a.iter_mut() {
                fix_value(x, functions, pdfx);
            }
        }
        _ => {}
    }
}

/// Optionen für `convert`.
pub struct Options<'a> {
    /// Farbprofil der Druckbedingung (ICC-Daten); ohne Profil einfache Umrechnung
    pub profile: Option<(&'a CmykProfile, &'a [u8])>,
    /// PDF/X-4 erzeugen (verlangt ein Profil)
    pub pdfx: bool,
    /// Anschnitt in Punkt, der bereits um jede Seite herum gezeichnet ist
    pub bleed: f64,
    pub title: String,
    /// Zeitpunkt (Sekunden seit 1970)
    pub time: u64,
}

fn pdf_date(t: u64) -> (String, String) {
    // Kalenderdatum aus Sekunden (UTC)
    let days = (t / 86400) as i64;
    let secs = t % 86400;
    let (hh, mm, ss) = (secs / 3600, secs / 60 % 60, secs % 60);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (
        format!("D:{y:04}{m:02}{d:02}{hh:02}{mm:02}{ss:02}Z"),
        format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z"),
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Text als PDF-Zeichenkette (UTF-16 mit BOM, falls nötig).
fn pdf_text(s: &str) -> Obj {
    if s.is_ascii() {
        Obj::Str(s.as_bytes().to_vec())
    } else {
        let mut v = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            v.extend_from_slice(&u.to_be_bytes());
        }
        Obj::Str(v)
    }
}

/// Cairo-PDF in CMYK (und auf Wunsch PDF/X-4) umwandeln.
pub fn convert(input: &[u8], opt: &Options) -> Result<Vec<u8>, String> {
    if opt.pdfx && opt.profile.is_none() {
        return Err("Für PDF/X wird ein Farbprofil der Druckbedingung gebraucht.".into());
    }
    let mut pdf = Pdf::load(input)?;
    // Hilfsobjekte der Datei (Querverweise, Objektströme) werden neu geschrieben
    pdf.objs.retain(|_, o| {
        !matches!(o, Obj::Stream(s) if matches!(s.dict.get("Type").and_then(Obj::name), Some("XRef") | Some("ObjStm")))
    });
    let mut conv = Conv {
        profile: opt.profile.map(|p| p.0),
        cache: HashMap::new(),
    };
    let ids: Vec<u32> = {
        let mut v: Vec<u32> = pdf.objs.keys().copied().collect();
        v.sort();
        v
    };
    // Seiteninhalte und Zeichenvorschriften von Type-3-Schriften sind ebenfalls Inhalt
    let mut contents: HashSet<u32> = HashSet::new();
    for id in &ids {
        let Some(d) = pdf.objs.get(id).and_then(|o| o.dict()) else {
            continue;
        };
        match d.get("Type").and_then(Obj::name) {
            Some("Page") => match d.get("Contents") {
                Some(Obj::Ref(n)) => {
                    contents.insert(*n);
                }
                Some(Obj::Arr(a)) => {
                    for x in a {
                        if let Obj::Ref(n) = x {
                            contents.insert(*n);
                        }
                    }
                }
                _ => {}
            },
            Some("Font") if d.get("Subtype").and_then(Obj::name) == Some("Type3") => {
                if let Some(cp) = d.get("CharProcs").map(|c| pdf.res(c).clone()) {
                    if let Some(cp) = cp.dict() {
                        for v in cp.values() {
                            if let Obj::Ref(n) = v {
                                contents.insert(*n);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let mut functions: Vec<Obj> = Vec::new();
    for id in &ids {
        let Some(obj) = pdf.objs.get(id).cloned() else {
            continue;
        };
        let new = match obj {
            Obj::Stream(s) => {
                let d = &s.dict;
                let sub = d.get("Subtype").and_then(Obj::name);
                let is_image = sub == Some("Image");
                if is_image && d.get("ColorSpace").map(|c| pdf.res(c)).is_some_and(is_rgb_name) {
                    let mut img = convert_image(&pdf, &s, &mut conv)
                        .ok_or_else(|| "Ein Bild ließ sich nicht nach CMYK umrechnen.".to_string())?;
                    if let Obj::Stream(st) = &mut img {
                        let mut dd = st.dict.clone();
                        fix_dict(&mut dd, &mut functions, opt.pdfx);
                        img = Obj::Stream(Rc::new(Strm {
                            dict: dd,
                            data: st.data.clone(),
                        }));
                    }
                    img
                } else {
                    let content = contents.contains(id)
                        || sub == Some("Form")
                        || d.get("PatternType").and_then(Obj::num) == Some(1.0);
                    let mut dd = d.clone();
                    fix_dict(&mut dd, &mut functions, opt.pdfx);
                    if content {
                        let (data, special) = pdf.stream_data(&s);
                        if special.is_some() {
                            return Err("Ein Inhalt der Seite ließ sich nicht lesen.".into());
                        }
                        let conv_data = convert_content(&data, &conv)?;
                        dd.insert("Filter".into(), Obj::Name("FlateDecode".into()));
                        dd.remove("DecodeParms");
                        Obj::Stream(Rc::new(Strm {
                            dict: dd,
                            data: zlib_encode(&conv_data),
                        }))
                    } else {
                        Obj::Stream(Rc::new(Strm {
                            dict: dd,
                            data: s.data.clone(),
                        }))
                    }
                }
            }
            mut other => {
                fix_value(&mut other, &mut functions, opt.pdfx);
                other
            }
        };
        pdf.objs.insert(*id, new);
    }
    let mut done = HashSet::new();
    for f in functions {
        convert_function(&mut pdf, &f, &conv, &mut done);
    }

    // Katalog, Seitenrahmen, Ausgabebedingung, Metadaten
    let catalog_id = pdf
        .objs
        .iter()
        .filter(|(_, o)| o.dict().and_then(|d| d.get("Type")).and_then(Obj::name) == Some("Catalog"))
        .map(|(k, _)| *k)
        .min()
        .ok_or("Die PDF-Datei hat keinen Katalog.")?;
    let mut next = pdf.objs.keys().max().copied().unwrap_or(0) + 1;
    let mut alloc = || {
        let n = next;
        next += 1;
        n
    };
    let page_ids: Vec<u32> = pdf
        .objs
        .iter()
        .filter(|(_, o)| o.dict().and_then(|d| d.get("Type")).and_then(Obj::name) == Some("Page"))
        .map(|(k, _)| *k)
        .collect();
    for pid in page_ids {
        let Some(Obj::Dict(mut d)) = pdf.objs.get(&pid).cloned() else {
            continue;
        };
        let mb = d
            .get("MediaBox")
            .map(|m| pdf.res(m).clone())
            .and_then(|m| nums(m.arr()?))
            .filter(|v| v.len() == 4);
        if let Some(mb) = mb {
            let b = opt.bleed.max(0.0);
            let trim = [mb[0] + b, mb[1] + b, mb[2] - b, mb[3] - b];
            d.insert(
                "TrimBox".into(),
                Obj::Arr(trim.iter().map(|x| Obj::Num(*x)).collect()),
            );
            if b > 0.0 {
                d.insert(
                    "BleedBox".into(),
                    Obj::Arr(mb.iter().map(|x| Obj::Num(*x)).collect()),
                );
            }
        }
        pdf.objs.insert(pid, Obj::Dict(d));
    }
    let (pdf_t, xmp_t) = pdf_date(opt.time);
    let mut info = Dict::new();
    info.insert("Title".into(), pdf_text(&opt.title));
    info.insert("Creator".into(), pdf_text("Mondrian"));
    info.insert("Producer".into(), pdf_text("Mondrian (Cairo)"));
    info.insert("CreationDate".into(), Obj::Str(pdf_t.clone().into_bytes()));
    info.insert("ModDate".into(), Obj::Str(pdf_t.into_bytes()));
    info.insert("Trapped".into(), Obj::Name("False".into()));
    let mut catalog = match pdf.objs.get(&catalog_id) {
        Some(Obj::Dict(d)) => d.clone(),
        _ => return Err("Die PDF-Datei hat einen ungültigen Katalog.".into()),
    };
    if let Some((profile, icc)) = opt.profile {
        let desc = if profile.description.is_empty() {
            "CMYK".to_string()
        } else {
            profile.description.clone()
        };
        let icc_id = alloc();
        let mut icc_d = Dict::new();
        icc_d.insert("N".into(), Obj::Num(4.0));
        icc_d.insert("Filter".into(), Obj::Name("FlateDecode".into()));
        pdf.objs.insert(
            icc_id,
            Obj::Stream(Rc::new(Strm {
                dict: icc_d,
                data: zlib_encode(icc),
            })),
        );
        let mut oi = Dict::new();
        oi.insert("Type".into(), Obj::Name("OutputIntent".into()));
        oi.insert("S".into(), Obj::Name("GTS_PDFX".into()));
        oi.insert(
            "OutputConditionIdentifier".into(),
            pdf_text(match crate::icc::output_condition(&desc) {
                "Custom" => desc.as_str(),
                known => known,
            }),
        );
        oi.insert("OutputCondition".into(), pdf_text(&desc));
        oi.insert("Info".into(), pdf_text(&desc));
        oi.insert("RegistryName".into(), pdf_text("http://www.color.org"));
        oi.insert("DestOutputProfile".into(), Obj::Ref(icc_id));
        let oi_id = alloc();
        pdf.objs.insert(oi_id, Obj::Dict(oi));
        catalog.insert("OutputIntents".into(), Obj::Arr(vec![Obj::Ref(oi_id)]));
    }
    // Kennung der Datei
    let id_bytes: Vec<u8> = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        input.len().hash(&mut h);
        opt.time.hash(&mut h);
        opt.title.hash(&mut h);
        input[..input.len().min(4096)].hash(&mut h);
        let a = h.finish();
        a.hash(&mut h);
        let b = h.finish();
        [a.to_be_bytes(), b.to_be_bytes()].concat()
    };
    let uuid = {
        let h: String = id_bytes.iter().map(|b| format!("{b:02x}")).collect();
        format!(
            "uuid:{}-{}-{}-{}-{}",
            &h[0..8],
            &h[8..12],
            &h[12..16],
            &h[16..20],
            &h[20..32]
        )
    };
    if opt.pdfx {
        info.insert("GTS_PDFXVersion".into(), pdf_text("PDF/X-4"));
        let title = xml_escape(&opt.title);
        let xmp = format!(
            r#"<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/"
    xmlns:pdf="http://ns.adobe.com/pdf/1.3/"
    xmlns:pdfxid="http://www.npes.org/pdfx/ns/id/">
   <dc:format>application/pdf</dc:format>
   <dc:title><rdf:Alt><rdf:li xml:lang="x-default">{title}</rdf:li></rdf:Alt></dc:title>
   <xmp:CreateDate>{xmp_t}</xmp:CreateDate>
   <xmp:ModifyDate>{xmp_t}</xmp:ModifyDate>
   <xmp:MetadataDate>{xmp_t}</xmp:MetadataDate>
   <xmp:CreatorTool>Mondrian</xmp:CreatorTool>
   <xmpMM:DocumentID>{uuid}</xmpMM:DocumentID>
   <xmpMM:InstanceID>{uuid}</xmpMM:InstanceID>
   <xmpMM:VersionID>1</xmpMM:VersionID>
   <xmpMM:RenditionClass>default</xmpMM:RenditionClass>
   <pdf:Producer>Mondrian (Cairo)</pdf:Producer>
   <pdf:Trapped>False</pdf:Trapped>
   <pdfxid:GTS_PDFXVersion>PDF/X-4</pdfxid:GTS_PDFXVersion>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#
        );
        let mut md = Dict::new();
        md.insert("Type".into(), Obj::Name("Metadata".into()));
        md.insert("Subtype".into(), Obj::Name("XML".into()));
        let md_id = alloc();
        pdf.objs.insert(
            md_id,
            Obj::Stream(Rc::new(Strm {
                dict: md,
                data: xmp.into_bytes(),
            })),
        );
        catalog.insert("Metadata".into(), Obj::Ref(md_id));
    }
    pdf.objs.insert(catalog_id, Obj::Dict(catalog));
    let info_id = alloc();
    pdf.objs.insert(info_id, Obj::Dict(info));

    // Schreiben: PDF 1.6, klassische Querverweistabelle
    let mut out: Vec<u8> = Vec::with_capacity(input.len() * 2);
    out.extend_from_slice(b"%PDF-1.6\n%\xE2\xE3\xCF\xD3\n");
    let max = pdf.objs.keys().max().copied().unwrap_or(0);
    let mut offsets = vec![None; max as usize + 1];
    let mut keys: Vec<u32> = pdf.objs.keys().copied().collect();
    keys.sort();
    for k in keys {
        offsets[k as usize] = Some(out.len());
        write_object(k, &pdf.objs[&k], &mut out);
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", max + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for o in offsets.iter().skip(1) {
        match o {
            Some(off) => out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes()),
            None => out.extend_from_slice(b"0000000000 65535 f \n"),
        }
    }
    let idh = hex(&id_bytes);
    out.extend_from_slice(
        format!(
            "trailer\n<</Size {} /Root {catalog_id} 0 R /Info {info_id} 0 R /ID [{idh} {idh}]>>\nstartxref\n{xref}\n%%EOF\n",
            max + 1
        )
        .as_bytes(),
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deflate_round_trip() {
        let mut data = Vec::new();
        for i in 0..20000u32 {
            data.push((i % 251) as u8);
            if i % 7 == 0 {
                data.extend_from_slice(b"Mondrian Mondrian ");
            }
        }
        for sample in [&b""[..], &b"a"[..], &b"abcabcabcabcabcabcabc"[..], &data[..]] {
            let z = zlib_encode(sample);
            assert_eq!(crate::pdf::zlib_decode(&z), sample, "Länge {}", sample.len());
        }
        // es wird tatsächlich gepackt
        assert!(zlib_encode(&data).len() < data.len() / 3);
    }

    #[test]
    fn content_colors_become_cmyk() {
        let conv = Conv {
            profile: None,
            cache: HashMap::new(),
        };
        let src = b"q 1 0 0 rg /a0 gs 10 10 50 50 re f 0 0 0 RG 3 w [] 0 d 0.5 0.5 0.5 g\n\
            BT /f-0-0 1 Tf (Hallo) Tj [(A) -20 (B)] TJ ET /DeviceRGB cs 0 0 1 sc Q /Pattern cs /p1 scn";
        let out = String::from_utf8(convert_content(src, &conv).unwrap()).unwrap();
        assert!(out.contains("0 1 1 0 k"), "{out}");
        assert!(out.contains("0 0 0 1 K"), "{out}");
        assert!(out.contains("/DeviceCMYK cs"), "{out}");
        assert!(out.contains("1 1 0 0 sc"), "{out}");
        assert!(out.contains("0.5 0.5 0.5 g"), "{out}");
        assert!(out.contains("<48616C6C6F> Tj"), "{out}");
        assert!(out.contains("/Pattern cs") && out.contains("/p1 scn"), "{out}");
        assert!(!out.contains(" rg") && !out.contains(" RG"), "{out}");
    }

    #[test]
    fn dates() {
        let (p, x) = pdf_date(1_760_558_400); // 15.10.2025 20:00 UTC
        assert_eq!(p, "D:20251015200000Z");
        assert_eq!(x, "2025-10-15T20:00:00Z");
    }

    #[test]
    fn whole_file_conversion() {
        // kleine PDF-Datei: eine Seite mit roter Fläche und einem 2×1-RGB-Bild
        let content = b"1 0 0 rg 0 0 10 10 re f q 2 0 0 1 0 0 cm /Im1 Do Q";
        let objs: Vec<Vec<u8>> = vec![
            b"<</Type /Catalog /Pages 2 0 R>>".to_vec(),
            b"<</Type /Pages /Kids [3 0 R] /Count 1>>".to_vec(),
            b"<</Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources <</XObject <</Im1 5 0 R>>>> /Group <</S /Transparency /CS /DeviceRGB>>>>".to_vec(),
            [format!("<</Length {}>>\nstream\n", content.len()).as_bytes(), &content[..], b"\nendstream"].concat(),
            [&b"<</Type /XObject /Subtype /Image /Width 2 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceRGB /Interpolate true /Length 6>>\nstream\n"[..], &[255, 0, 0, 0, 0, 0][..], b"\nendstream"].concat(),
        ];
        let mut pdf = b"%PDF-1.7\n".to_vec();
        for (i, o) in objs.iter().enumerate() {
            pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            pdf.extend_from_slice(o);
            pdf.extend_from_slice(b"\nendobj\n");
        }
        pdf.extend_from_slice(b"trailer\n<</Root 1 0 R /Size 6>>\n%%EOF\n");
        let out = convert(
            &pdf,
            &Options {
                profile: None,
                pdfx: false,
                bleed: 3.0,
                title: "Probe".into(),
                time: 0,
            },
        )
        .unwrap();
        let back = Pdf::load(&out).unwrap();
        let page = back.objs[&3].dict().unwrap();
        assert_eq!(
            nums(page["TrimBox"].arr().unwrap()).unwrap(),
            vec![3.0, 3.0, 97.0, 97.0]
        );
        assert_eq!(page["Group"].dict().unwrap()["CS"].name(), Some("DeviceCMYK"));
        let Obj::Stream(c) = &back.objs[&4] else { panic!() };
        let text = String::from_utf8(back.stream_data(c).0).unwrap();
        assert!(text.starts_with("0 1 1 0 k"), "{text}");
        let Obj::Stream(img) = &back.objs[&5] else {
            panic!()
        };
        assert_eq!(img.dict["ColorSpace"].name(), Some("DeviceCMYK"));
        assert_eq!(back.stream_data(img).0, vec![0, 255, 255, 0, 0, 0, 0, 255]);
        // ohne PDF/X bleibt die Bildglättung
        assert!(img.dict.contains_key("Interpolate"));
        // PDF/X ohne Profil geht nicht
        assert!(convert(
            &pdf,
            &Options {
                profile: None,
                pdfx: true,
                bleed: 0.0,
                title: String::new(),
                time: 0
            }
        )
        .is_err());
    }
}
