// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! Minimaler PDF-Leser für den Import von Illustrator-Dateien (.ai sind PDF-Dateien).
//!
//! Enthalten sind: Objektleser, eigenes Inflate (Flate), die üblichen Filter, Funktionen
//! (Typ 0, 2, 3, 4) und Farbräume. Der Inhalt der Seiten wird in `ai.rs` ausgewertet.
//! Verschlüsselte Dateien werden nicht unterstützt.

use crate::i18n::tr;
use std::collections::HashMap;
use std::rc::Rc;

pub type Dict = HashMap<String, Obj>;

#[derive(Clone, Debug)]
pub enum Obj {
    Null,
    Bool(bool),
    Num(f64),
    Str(Vec<u8>),
    Name(String),
    Arr(Vec<Obj>),
    Dict(Dict),
    Stream(Rc<Strm>),
    Ref(u32),
}

#[derive(Debug)]
pub struct Strm {
    pub dict: Dict,
    pub data: Vec<u8>,
}

impl Obj {
    pub fn num(&self) -> Option<f64> {
        match self {
            Obj::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn name(&self) -> Option<&str> {
        match self {
            Obj::Name(n) => Some(n),
            _ => None,
        }
    }
    pub fn arr(&self) -> Option<&Vec<Obj>> {
        match self {
            Obj::Arr(a) => Some(a),
            _ => None,
        }
    }
    pub fn dict(&self) -> Option<&Dict> {
        match self {
            Obj::Dict(d) => Some(d),
            Obj::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }
    pub fn string(&self) -> Option<&[u8]> {
        match self {
            Obj::Str(s) => Some(s),
            _ => None,
        }
    }
}

// ───────────────────────────── Inflate ─────────────────────────────

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    buf: u64,
    cnt: u32,
}

impl<'a> Bits<'a> {
    fn bits(&mut self, n: u32) -> Option<u32> {
        while self.cnt < n {
            let b = *self.d.get(self.pos)?;
            self.pos += 1;
            self.buf |= (b as u64) << self.cnt;
            self.cnt += 8;
        }
        let v = (self.buf & ((1u64 << n) - 1)) as u32;
        self.buf >>= n;
        self.cnt -= n;
        Some(v)
    }
}

struct Huff {
    count: [u16; 16],
    symbol: Vec<u16>,
}

fn huff(lengths: &[u8]) -> Huff {
    let mut count = [0u16; 16];
    for &l in lengths {
        count[l as usize] += 1;
    }
    let mut offs = [0u16; 16];
    for l in 1..15 {
        offs[l + 1] = offs[l] + count[l];
    }
    let mut symbol = vec![0u16; lengths.len()];
    for (s, &l) in lengths.iter().enumerate() {
        if l != 0 {
            symbol[offs[l as usize] as usize] = s as u16;
            offs[l as usize] += 1;
        }
    }
    Huff { count, symbol }
}

fn decode_sym(b: &mut Bits, h: &Huff) -> Option<u16> {
    let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
    for len in 1..16 {
        code |= b.bits(1)? as i32;
        let count = h.count[len] as i32;
        if code - count < first {
            return h.symbol.get((index + (code - first)) as usize).copied();
        }
        index += count;
        first += count;
        first <<= 1;
        code <<= 1;
    }
    None
}

const LBASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195,
    227, 258,
];
const LEXT: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DBASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073,
    4097, 6145, 8193, 12289, 16385, 24577,
];
const DEXT: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13,
];

const MAX_OUT: usize = 1 << 29;

fn inflate_codes(b: &mut Bits, out: &mut Vec<u8>, lit: &Huff, dist: &Huff) -> Option<()> {
    loop {
        let sym = decode_sym(b, lit)? as usize;
        if sym < 256 {
            out.push(sym as u8);
        } else if sym == 256 {
            return Some(());
        } else {
            let s = sym - 257;
            if s >= 29 {
                return None;
            }
            let len = LBASE[s] as usize + b.bits(LEXT[s] as u32)? as usize;
            let ds = decode_sym(b, dist)? as usize;
            if ds >= 30 {
                return None;
            }
            let d = DBASE[ds] as usize + b.bits(DEXT[ds] as u32)? as usize;
            if d > out.len() {
                return None;
            }
            let start = out.len() - d;
            for i in 0..len {
                let v = out[start + i];
                out.push(v);
            }
        }
        if out.len() > MAX_OUT {
            return None;
        }
    }
}

/// Rohes Deflate. Bei beschädigten Daten wird das bis dahin Entpackte geliefert.
pub fn inflate(data: &[u8]) -> Vec<u8> {
    let mut b = Bits {
        d: data,
        pos: 0,
        buf: 0,
        cnt: 0,
    };
    let mut out = Vec::with_capacity(data.len() * 3);
    loop {
        let Some(last) = b.bits(1) else { break };
        let Some(typ) = b.bits(2) else { break };
        let ok = match typ {
            0 => {
                b.buf = 0;
                b.cnt = 0;
                if b.pos + 4 > data.len() {
                    break;
                }
                let len = data[b.pos] as usize | (data[b.pos + 1] as usize) << 8;
                b.pos += 4;
                let end = (b.pos + len).min(data.len());
                out.extend_from_slice(&data[b.pos..end]);
                b.pos = end;
                true
            }
            1 => {
                let mut l = [0u8; 288];
                for (i, v) in l.iter_mut().enumerate() {
                    *v = match i {
                        0..=143 => 8,
                        144..=255 => 9,
                        256..=279 => 7,
                        _ => 8,
                    };
                }
                let lit = huff(&l);
                let dist = huff(&[5u8; 30]);
                inflate_codes(&mut b, &mut out, &lit, &dist).is_some()
            }
            2 => (|| {
                let nlen = b.bits(5)? as usize + 257;
                let ndist = b.bits(5)? as usize + 1;
                let ncode = b.bits(4)? as usize + 4;
                const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
                let mut lengths = [0u8; 320];
                for &o in ORDER.iter().take(ncode) {
                    lengths[o] = b.bits(3)? as u8;
                }
                let lencode = huff(&lengths[..19]);
                let mut lens = [0u8; 320];
                let mut idx = 0;
                while idx < nlen + ndist {
                    let sym = decode_sym(&mut b, &lencode)?;
                    if sym < 16 {
                        lens[idx] = sym as u8;
                        idx += 1;
                    } else {
                        let (prev, rep) = match sym {
                            16 => {
                                if idx == 0 {
                                    return None;
                                }
                                (lens[idx - 1], 3 + b.bits(2)? as usize)
                            }
                            17 => (0, 3 + b.bits(3)? as usize),
                            _ => (0, 11 + b.bits(7)? as usize),
                        };
                        if idx + rep > nlen + ndist {
                            return None;
                        }
                        for _ in 0..rep {
                            lens[idx] = prev;
                            idx += 1;
                        }
                    }
                }
                let lit = huff(&lens[..nlen]);
                let dist = huff(&lens[nlen..nlen + ndist]);
                inflate_codes(&mut b, &mut out, &lit, &dist)
            })()
            .is_some(),
            _ => false,
        };
        if !ok || last == 1 {
            break;
        }
    }
    out
}

/// zlib-Datenstrom (Kopf wird übersprungen, Prüfsumme ignoriert).
pub fn zlib_decode(data: &[u8]) -> Vec<u8> {
    if data.len() >= 2 && data[0] & 0x0f == 8 && ((data[0] as u32) << 8 | data[1] as u32) % 31 == 0 {
        inflate(&data[2..])
    } else {
        inflate(data)
    }
}

fn lzw_decode(data: &[u8], early: usize) -> Vec<u8> {
    let mut table: Vec<Vec<u8>> = (0..256).map(|i| vec![i as u8]).collect();
    table.push(Vec::new());
    table.push(Vec::new());
    let mut out = Vec::new();
    let mut bits = 9usize;
    let (mut buf, mut cnt) = (0u32, 0usize);
    let mut prev: Option<Vec<u8>> = None;
    let mut pos = 0;
    loop {
        while cnt < bits {
            let Some(&b) = data.get(pos) else { return out };
            pos += 1;
            buf = (buf << 8) | b as u32;
            cnt += 8;
        }
        let code = ((buf >> (cnt - bits)) & ((1 << bits) - 1)) as usize;
        cnt -= bits;
        buf &= (1u32 << cnt) - 1;
        if code == 256 {
            table.truncate(258);
            bits = 9;
            prev = None;
            continue;
        }
        if code == 257 {
            break;
        }
        let entry = if code < table.len() {
            table[code].clone()
        } else if let Some(p) = &prev {
            let mut e = p.clone();
            e.push(p[0]);
            e
        } else {
            break;
        };
        out.extend_from_slice(&entry);
        if let Some(p) = prev.take() {
            let mut n = p;
            n.push(entry[0]);
            if table.len() < 4096 {
                table.push(n);
            }
        }
        prev = Some(entry);
        let n = table.len() + early;
        bits = if n >= 2048 {
            12
        } else if n >= 1024 {
            11
        } else if n >= 512 {
            10
        } else {
            9
        };
        if out.len() > MAX_OUT {
            break;
        }
    }
    out
}

fn ascii85(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut grp = [0u32; 5];
    let mut n = 0;
    let mut i = 0;
    if data.starts_with(b"<~") {
        i = 2;
    }
    while i < data.len() {
        let c = data[i];
        i += 1;
        match c {
            b'~' => break,
            b'z' if n == 0 => out.extend_from_slice(&[0, 0, 0, 0]),
            b'!'..=b'u' => {
                grp[n] = (c - b'!') as u32;
                n += 1;
                if n == 5 {
                    let v = grp.iter().fold(0u32, |a, &d| a.wrapping_mul(85).wrapping_add(d));
                    out.extend_from_slice(&v.to_be_bytes());
                    n = 0;
                }
            }
            _ => {}
        }
    }
    if n > 1 {
        for g in grp.iter_mut().skip(n) {
            *g = 84;
        }
        let v = grp.iter().fold(0u32, |a, &d| a.wrapping_mul(85).wrapping_add(d));
        out.extend_from_slice(&v.to_be_bytes()[..n - 1]);
    }
    out
}

fn asciihex(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut hi: Option<u8> = None;
    for &c in data {
        if c == b'>' {
            break;
        }
        let v = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => continue,
        };
        match hi.take() {
            Some(h) => out.push(h << 4 | v),
            None => hi = Some(v),
        }
    }
    if let Some(h) = hi {
        out.push(h << 4);
    }
    out
}

fn runlength(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let l = data[i] as usize;
        i += 1;
        if l == 128 {
            break;
        }
        if l < 128 {
            let end = (i + l + 1).min(data.len());
            out.extend_from_slice(&data[i..end]);
            i = end;
        } else if let Some(&b) = data.get(i) {
            out.extend(std::iter::repeat(b).take(257 - l));
            i += 1;
        }
    }
    out
}

fn predictor(data: Vec<u8>, parms: &Dict) -> Vec<u8> {
    let pred = parms.get("Predictor").and_then(Obj::num).unwrap_or(1.0) as i32;
    if pred < 2 {
        return data;
    }
    let colors = parms.get("Colors").and_then(Obj::num).unwrap_or(1.0) as usize;
    let bpc = parms.get("BitsPerComponent").and_then(Obj::num).unwrap_or(8.0) as usize;
    let cols = parms.get("Columns").and_then(Obj::num).unwrap_or(1.0) as usize;
    let bpp = (colors * bpc).div_ceil(8).max(1);
    let row = (colors * bpc * cols).div_ceil(8);
    if row == 0 {
        return data;
    }
    if pred == 2 {
        if bpc != 8 {
            return data;
        }
        let mut out = data;
        for r in out.chunks_mut(row) {
            for i in bpp..r.len() {
                r[i] = r[i].wrapping_add(r[i - bpp]);
            }
        }
        return out;
    }
    let n = data.len().div_ceil(row + 1);
    let mut out = vec![0u8; n * row];
    for y in 0..n {
        let start = y * (row + 1);
        let Some(&ft) = data.get(start) else { break };
        for x in 0..row {
            let cur = data.get(start + 1 + x).copied().unwrap_or(0);
            let a = if x >= bpp { out[y * row + x - bpp] } else { 0 };
            let b = if y > 0 { out[(y - 1) * row + x] } else { 0 };
            let c = if x >= bpp && y > 0 {
                out[(y - 1) * row + x - bpp]
            } else {
                0
            };
            let v = match ft {
                1 => cur.wrapping_add(a),
                2 => cur.wrapping_add(b),
                3 => cur.wrapping_add(((a as u16 + b as u16) / 2) as u8),
                4 => {
                    let p = a as i32 + b as i32 - c as i32;
                    let (pa, pb, pc) = ((p - a as i32).abs(), (p - b as i32).abs(), (p - c as i32).abs());
                    let pr = if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    };
                    cur.wrapping_add(pr)
                }
                _ => cur,
            };
            out[y * row + x] = v;
        }
    }
    out
}

// ───────────────────────────── Lexer ─────────────────────────────

fn is_ws(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}

fn is_delim(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

pub enum Tok {
    O(Obj),
    Kw(String),
    ArrEnd,
    DictEnd,
    Eof,
}

pub struct Lex<'a> {
    pub d: &'a [u8],
    pub pos: usize,
    refs: bool,
    /// Verschachtelungstiefe von Arrays und Dictionaries
    depth: u32,
}

/// Tiefer verschachtelte Arrays/Dictionaries werden nicht gelesen (sonst könnte eine
/// präparierte Datei den Stapel überlaufen lassen).
const MAX_NEST: u32 = 100;

fn parse_number(t: &[u8]) -> Option<f64> {
    if t.is_empty() || !t.iter().all(|c| matches!(c, b'0'..=b'9' | b'+' | b'-' | b'.')) {
        return None;
    }
    if !t.iter().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut s = std::str::from_utf8(t).ok()?;
    while s.starts_with("--") || s.starts_with("+-") {
        s = &s[1..];
    }
    s.parse::<f64>().ok()
}

impl<'a> Lex<'a> {
    pub fn new(d: &'a [u8], refs: bool) -> Lex<'a> {
        Lex {
            d,
            pos: 0,
            refs,
            depth: 0,
        }
    }

    pub fn skip_ws(&mut self) {
        loop {
            while self.pos < self.d.len() && is_ws(self.d[self.pos]) {
                self.pos += 1;
            }
            if self.pos < self.d.len() && self.d[self.pos] == b'%' {
                while self.pos < self.d.len() && self.d[self.pos] != b'\n' && self.d[self.pos] != b'\r' {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    fn token_end(&self) -> usize {
        let mut e = self.pos;
        while e < self.d.len() && !is_ws(self.d[e]) && !is_delim(self.d[e]) {
            e += 1;
        }
        e
    }

    pub fn next(&mut self) -> Tok {
        // einzelne „)“ und „>“ ohne Gegenstück überspringen (in einer Schleife, nicht rekursiv)
        loop {
            self.skip_ws();
            match (self.d.get(self.pos), self.d.get(self.pos + 1)) {
                (Some(b')'), _) => self.pos += 1,
                (Some(b'>'), n) if n != Some(&b'>') => self.pos += 1,
                _ => break,
            }
        }
        let Some(&c) = self.d.get(self.pos) else {
            return Tok::Eof;
        };
        match c {
            b'/' => {
                self.pos += 1;
                let e = self.token_end();
                let raw = &self.d[self.pos..e];
                self.pos = e;
                let mut name = Vec::with_capacity(raw.len());
                let mut i = 0;
                while i < raw.len() {
                    if raw[i] == b'#' && i + 2 < raw.len() {
                        let h = std::str::from_utf8(&raw[i + 1..i + 3])
                            .ok()
                            .and_then(|s| u8::from_str_radix(s, 16).ok());
                        if let Some(v) = h {
                            name.push(v);
                            i += 3;
                            continue;
                        }
                    }
                    name.push(raw[i]);
                    i += 1;
                }
                Tok::O(Obj::Name(String::from_utf8_lossy(&name).into_owned()))
            }
            b'(' => {
                self.pos += 1;
                let mut depth = 1;
                let mut s = Vec::new();
                while self.pos < self.d.len() {
                    let b = self.d[self.pos];
                    self.pos += 1;
                    match b {
                        b'\\' => {
                            let Some(&n) = self.d.get(self.pos) else { break };
                            self.pos += 1;
                            match n {
                                b'n' => s.push(b'\n'),
                                b'r' => s.push(b'\r'),
                                b't' => s.push(b'\t'),
                                b'b' => s.push(8),
                                b'f' => s.push(12),
                                b'0'..=b'7' => {
                                    let mut v = (n - b'0') as u32;
                                    for _ in 0..2 {
                                        match self.d.get(self.pos) {
                                            Some(&o @ b'0'..=b'7') => {
                                                v = v * 8 + (o - b'0') as u32;
                                                self.pos += 1;
                                            }
                                            _ => break,
                                        }
                                    }
                                    s.push(v as u8);
                                }
                                b'\r' => {
                                    if self.d.get(self.pos) == Some(&b'\n') {
                                        self.pos += 1;
                                    }
                                }
                                b'\n' => {}
                                other => s.push(other),
                            }
                        }
                        b'(' => {
                            depth += 1;
                            s.push(b);
                        }
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                            s.push(b);
                        }
                        _ => s.push(b),
                    }
                }
                Tok::O(Obj::Str(s))
            }
            b'<' => {
                if self.d.get(self.pos + 1) == Some(&b'<') {
                    self.pos += 2;
                    if self.depth >= MAX_NEST {
                        return Tok::O(Obj::Null);
                    }
                    self.depth += 1;
                    let mut dict = Dict::new();
                    loop {
                        match self.next() {
                            Tok::O(Obj::Name(k)) => match self.next() {
                                Tok::O(v) => {
                                    dict.insert(k, v);
                                }
                                Tok::Kw(_) => {
                                    dict.insert(k, Obj::Null);
                                }
                                _ => break,
                            },
                            Tok::DictEnd | Tok::Eof => break,
                            _ => {}
                        }
                    }
                    self.depth -= 1;
                    Tok::O(Obj::Dict(dict))
                } else {
                    self.pos += 1;
                    let mut s = Vec::new();
                    let mut hi: Option<u8> = None;
                    while self.pos < self.d.len() {
                        let b = self.d[self.pos];
                        self.pos += 1;
                        if b == b'>' {
                            break;
                        }
                        let v = match b {
                            b'0'..=b'9' => b - b'0',
                            b'a'..=b'f' => b - b'a' + 10,
                            b'A'..=b'F' => b - b'A' + 10,
                            _ => continue,
                        };
                        match hi.take() {
                            Some(h) => s.push(h << 4 | v),
                            None => hi = Some(v),
                        }
                    }
                    if let Some(h) = hi {
                        s.push(h << 4);
                    }
                    Tok::O(Obj::Str(s))
                }
            }
            b'>' => {
                if self.d.get(self.pos + 1) == Some(&b'>') {
                    self.pos += 2;
                    Tok::DictEnd
                } else {
                    self.pos += 1;
                    self.next()
                }
            }
            b'[' => {
                self.pos += 1;
                if self.depth >= MAX_NEST {
                    return Tok::O(Obj::Null);
                }
                self.depth += 1;
                let mut a = Vec::new();
                loop {
                    match self.next() {
                        Tok::O(o) => a.push(o),
                        Tok::ArrEnd | Tok::Eof => break,
                        Tok::DictEnd => {}
                        Tok::Kw(_) => {}
                    }
                }
                self.depth -= 1;
                Tok::O(Obj::Arr(a))
            }
            b']' => {
                self.pos += 1;
                Tok::ArrEnd
            }
            b'{' | b'}' => {
                self.pos += 1;
                Tok::Kw((c as char).to_string())
            }
            b')' => {
                self.pos += 1;
                self.next()
            }
            _ => {
                let e = self.token_end().max(self.pos + 1);
                let t = &self.d[self.pos..e];
                self.pos = e;
                if let Some(n) = parse_number(t) {
                    if self.refs && !t.contains(&b'.') && n >= 0.0 {
                        let save = self.pos;
                        self.skip_ws();
                        let e2 = self.token_end();
                        let gen = &self.d[self.pos..e2];
                        if !gen.is_empty() && gen.iter().all(|c| c.is_ascii_digit()) {
                            self.pos = e2;
                            self.skip_ws();
                            if self.d.get(self.pos) == Some(&b'R')
                                && self
                                    .d
                                    .get(self.pos + 1)
                                    .map(|&b| is_ws(b) || is_delim(b))
                                    .unwrap_or(true)
                            {
                                self.pos += 1;
                                return Tok::O(Obj::Ref(n as u32));
                            }
                        }
                        self.pos = save;
                    }
                    return Tok::O(Obj::Num(n));
                }
                match t {
                    b"true" => Tok::O(Obj::Bool(true)),
                    b"false" => Tok::O(Obj::Bool(false)),
                    b"null" => Tok::O(Obj::Null),
                    _ => Tok::Kw(String::from_utf8_lossy(t).into_owned()),
                }
            }
        }
    }
}

// ───────────────────────────── Dokument ─────────────────────────────

pub struct Pdf {
    pub objs: HashMap<u32, Obj>,
    pub root: Dict,
    null: Obj,
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= hay.len() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

impl Pdf {
    pub fn res<'a>(&'a self, o: &'a Obj) -> &'a Obj {
        let mut o = o;
        for _ in 0..16 {
            match o {
                Obj::Ref(n) => match self.objs.get(n) {
                    Some(x) => o = x,
                    None => return &self.null,
                },
                _ => return o,
            }
        }
        &self.null
    }

    pub fn get<'a>(&'a self, d: &'a Dict, key: &str) -> &'a Obj {
        match d.get(key) {
            Some(o) => self.res(o),
            None => &self.null,
        }
    }

    pub fn num(&self, d: &Dict, key: &str) -> Option<f64> {
        self.get(d, key).num()
    }

    pub fn nums(&self, o: &Obj) -> Vec<f64> {
        match self.res(o) {
            Obj::Arr(a) => a.iter().filter_map(|x| self.res(x).num()).collect(),
            _ => Vec::new(),
        }
    }

    /// Dekodierte Daten eines Datenstroms; zweiter Wert: nicht entpackter Bildfilter.
    pub fn stream_data(&self, s: &Strm) -> (Vec<u8>, Option<String>) {
        let filters: Vec<String> = match self.get(&s.dict, "Filter").clone() {
            Obj::Name(n) => vec![n],
            Obj::Arr(a) => a
                .iter()
                .filter_map(|x| self.res(x).name().map(str::to_string))
                .collect(),
            _ => match self.get(&s.dict, "F").clone() {
                Obj::Name(n) => vec![n],
                Obj::Arr(a) => a
                    .iter()
                    .filter_map(|x| self.res(x).name().map(str::to_string))
                    .collect(),
                _ => Vec::new(),
            },
        };
        let parms: Vec<Dict> = match self.get(&s.dict, "DecodeParms").clone() {
            Obj::Dict(d) => vec![d],
            Obj::Arr(a) => a
                .iter()
                .map(|x| self.res(x).dict().cloned().unwrap_or_default())
                .collect(),
            _ => match self.get(&s.dict, "DP").clone() {
                Obj::Dict(d) => vec![d],
                Obj::Arr(a) => a
                    .iter()
                    .map(|x| self.res(x).dict().cloned().unwrap_or_default())
                    .collect(),
                _ => Vec::new(),
            },
        };
        let mut data = s.data.clone();
        for (i, f) in filters.iter().enumerate() {
            let p = parms.get(i).cloned().unwrap_or_default();
            let p: Dict = p.into_iter().map(|(k, v)| (k, self.res(&v).clone())).collect();
            data = match f.as_str() {
                "FlateDecode" | "Fl" => predictor(zlib_decode(&data), &p),
                "LZWDecode" | "LZW" => {
                    let early = p.get("EarlyChange").and_then(Obj::num).unwrap_or(1.0) as usize;
                    predictor(lzw_decode(&data, early), &p)
                }
                "ASCII85Decode" | "A85" => ascii85(&data),
                "ASCIIHexDecode" | "AHx" => asciihex(&data),
                "RunLengthDecode" | "RL" => runlength(&data),
                "Crypt" => data,
                other => return (data, Some(other.to_string())),
            };
        }
        (data, None)
    }

    pub fn load(file: &[u8]) -> Result<Pdf, String> {
        let start = find(file, b"%PDF-", 0).ok_or_else(|| {
            tr("Diese Datei enthält keine PDF-Daten. Die Illustrator-Datei muss mit der Option „PDF-kompatible Datei erstellen“ gespeichert sein.")
                .to_string()
        })?;
        let data = &file[start..];
        let mut pdf = Pdf {
            objs: HashMap::new(),
            root: Dict::new(),
            null: Obj::Null,
        };
        let mut trailers: Vec<Dict> = Vec::new();
        let mut i = 0usize;
        while let Some(j) = find(data, b"obj", i) {
            i = j + 3;
            let ok_before = j > 0 && is_ws(data[j - 1]);
            let ok_after = data.get(j + 3).map(|&b| is_ws(b) || is_delim(b)).unwrap_or(true);
            if !ok_before || !ok_after {
                continue;
            }
            // „Nummer Generation obj“ rückwärts lesen
            let mut k = j;
            while k > 0 && is_ws(data[k - 1]) {
                k -= 1;
            }
            let ge = k;
            while k > 0 && data[k - 1].is_ascii_digit() {
                k -= 1;
            }
            if k == ge {
                continue;
            }
            while k > 0 && is_ws(data[k - 1]) {
                k -= 1;
            }
            let ne = k;
            while k > 0 && data[k - 1].is_ascii_digit() {
                k -= 1;
            }
            if k == ne {
                continue;
            }
            let Some(num) = std::str::from_utf8(&data[k..ne])
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            let mut lex = Lex::new(data, true);
            lex.pos = j + 3;
            let Tok::O(obj) = lex.next() else { continue };
            let mut obj = obj;
            if let Obj::Dict(dict) = &obj {
                lex.skip_ws();
                if data[lex.pos..].starts_with(b"stream") {
                    let mut s = lex.pos + 6;
                    if data.get(s) == Some(&b'\r') {
                        s += 1;
                    }
                    if data.get(s) == Some(&b'\n') {
                        s += 1;
                    }
                    let mut end = None;
                    if let Some(len) = dict.get("Length").and_then(Obj::num) {
                        let e = s + len as usize;
                        if e <= data.len() {
                            let mut t = e;
                            while t < data.len() && is_ws(data[t]) {
                                t += 1;
                            }
                            if data[t..].starts_with(b"endstream") {
                                end = Some(e);
                            }
                        }
                    }
                    let end = match end {
                        Some(e) => e,
                        None => {
                            let mut e = find(data, b"endstream", s).unwrap_or(data.len());
                            if e > s && data[e - 1] == b'\n' {
                                e -= 1;
                            }
                            if e > s && data[e - 1] == b'\r' {
                                e -= 1;
                            }
                            e
                        }
                    };
                    obj = Obj::Stream(Rc::new(Strm {
                        dict: dict.clone(),
                        data: data[s..end.max(s)].to_vec(),
                    }));
                    i = end;
                } else {
                    i = lex.pos;
                }
            } else {
                i = lex.pos;
            }
            if let Obj::Stream(s) = &obj {
                match s.dict.get("Type").and_then(Obj::name) {
                    Some("XRef") => trailers.push(s.dict.clone()),
                    Some("ObjStm") => {
                        pdf.objs.insert(num, obj.clone());
                        pdf.read_objstm(s);
                        continue;
                    }
                    _ => {}
                }
            }
            pdf.objs.insert(num, obj);
        }
        let mut t = 0;
        while let Some(p) = find(data, b"trailer", t) {
            t = p + 7;
            let mut lex = Lex::new(data, true);
            lex.pos = t;
            if let Tok::O(Obj::Dict(d)) = lex.next() {
                trailers.push(d);
            }
        }
        if trailers.iter().any(|t| t.contains_key("Encrypt")) {
            return Err(
                tr("Die Datei ist verschlüsselt (passwortgeschützt) und lässt sich nicht öffnen.").into(),
            );
        }
        let root = trailers
            .iter()
            .rev()
            .find_map(|t| t.get("Root"))
            .and_then(|r| pdf.res(r).dict().cloned())
            .or_else(|| {
                pdf.objs
                    .values()
                    .filter_map(|o| o.dict())
                    .find(|d| d.get("Type").and_then(Obj::name) == Some("Catalog"))
                    .cloned()
            })
            .ok_or_else(|| {
                tr("Die PDF-Struktur der Datei ist beschädigt (kein Dokumentkatalog).").to_string()
            })?;
        pdf.root = root;
        Ok(pdf)
    }

    fn read_objstm(&mut self, s: &Rc<Strm>) {
        let (data, _) = self.stream_data(s);
        let n = s.dict.get("N").and_then(Obj::num).unwrap_or(0.0) as usize;
        let first = s.dict.get("First").and_then(Obj::num).unwrap_or(0.0) as usize;
        let mut head = Lex::new(&data, false);
        let mut pairs = Vec::new();
        for _ in 0..n {
            let (Tok::O(Obj::Num(a)), Tok::O(Obj::Num(b))) = (head.next(), head.next()) else {
                break;
            };
            pairs.push((a as u32, b as usize));
        }
        for (num, off) in pairs {
            let mut lex = Lex::new(&data, true);
            lex.pos = first + off;
            if lex.pos >= data.len() {
                continue;
            }
            if let Tok::O(o) = lex.next() {
                self.objs.insert(num, o);
            }
        }
    }

    /// Alle Seiten mit vererbten Eigenschaften.
    pub fn pages(&self) -> Vec<Dict> {
        let mut out = Vec::new();
        let Some(pages) = self.root.get("Pages") else {
            return out;
        };
        self.walk(self.res(pages), &Dict::new(), &mut out, 0);
        out
    }

    fn walk(&self, node: &Obj, inh: &Dict, out: &mut Vec<Dict>, depth: usize) {
        let Some(d) = node.dict() else { return };
        if depth > 24 || out.len() > 200 {
            return;
        }
        let mut inh = inh.clone();
        for k in ["Resources", "MediaBox", "CropBox", "Rotate"] {
            if let Some(v) = d.get(k) {
                inh.insert(k.to_string(), v.clone());
            }
        }
        match self.get(d, "Kids") {
            Obj::Arr(kids) => {
                for k in kids {
                    self.walk(self.res(k), &inh, out, depth + 1);
                }
            }
            _ => {
                let mut page = d.clone();
                for (k, v) in inh {
                    page.entry(k).or_insert(v);
                }
                out.push(page);
            }
        }
    }

    /// Inhalt einer Seite (alle Teile hintereinander).
    pub fn page_content(&self, page: &Dict) -> Vec<u8> {
        let mut out = Vec::new();
        let mut add = |o: &Obj| {
            if let Obj::Stream(s) = self.res(o) {
                out.extend(self.stream_data(s).0);
                out.push(b'\n');
            }
        };
        match page.get("Contents").map(|c| self.res(c)) {
            Some(Obj::Arr(a)) => {
                for x in a {
                    add(x);
                }
            }
            Some(o @ Obj::Stream(_)) => add(o),
            _ => {}
        }
        out
    }
}

// ───────────────────────────── Funktionen ─────────────────────────────

#[derive(Clone, Debug)]
enum Ps {
    Num(f64),
    Op(String),
    Block(Vec<Ps>),
    If(Vec<Ps>),
    IfElse(Vec<Ps>, Vec<Ps>),
}

fn ps_block(lex: &mut Lex) -> Vec<Ps> {
    let mut items: Vec<Ps> = Vec::new();
    loop {
        match lex.next() {
            Tok::O(Obj::Num(n)) => items.push(Ps::Num(n)),
            Tok::O(Obj::Bool(b)) => items.push(Ps::Num(b as i32 as f64)),
            Tok::Kw(k) => match k.as_str() {
                "{" => {
                    let b = ps_block(lex);
                    items.push(Ps::Block(b));
                }
                "}" => break,
                "if" => {
                    if let Some(Ps::Block(b)) = items.pop() {
                        items.push(Ps::If(b));
                    }
                }
                "ifelse" => {
                    let b2 = items.pop();
                    let b1 = items.pop();
                    if let (Some(Ps::Block(b1)), Some(Ps::Block(b2))) = (b1, b2) {
                        items.push(Ps::IfElse(b1, b2));
                    }
                }
                other => items.push(Ps::Op(other.to_string())),
            },
            Tok::Eof => break,
            _ => {}
        }
    }
    items
}

fn ps_run(code: &[Ps], st: &mut Vec<f64>, depth: usize) {
    if depth > 16 {
        return;
    }
    for it in code {
        match it {
            Ps::Num(n) => st.push(*n),
            Ps::Block(_) => {}
            Ps::If(b) => {
                if st.pop().unwrap_or(0.0) != 0.0 {
                    ps_run(b, st, depth + 1);
                }
            }
            Ps::IfElse(a, b) => {
                if st.pop().unwrap_or(0.0) != 0.0 {
                    ps_run(a, st, depth + 1);
                } else {
                    ps_run(b, st, depth + 1);
                }
            }
            Ps::Op(op) => ps_op(op, st),
        }
        if st.len() > 200 {
            st.truncate(200);
        }
    }
}

fn ps_op(op: &str, st: &mut Vec<f64>) {
    macro_rules! pop {
        () => {
            st.pop().unwrap_or(0.0)
        };
    }
    macro_rules! un {
        ($f:expr) => {{
            let a = pop!();
            st.push($f(a));
        }};
    }
    macro_rules! bin {
        ($f:expr) => {{
            let b = pop!();
            let a = pop!();
            st.push($f(a, b));
        }};
    }
    let bo = |v: bool| if v { 1.0 } else { 0.0 };
    match op {
        "abs" => un!(|a: f64| a.abs()),
        "add" => bin!(|a, b| a + b),
        "sub" => bin!(|a, b| a - b),
        "mul" => bin!(|a, b| a * b),
        "div" => bin!(|a: f64, b: f64| if b == 0.0 { 0.0 } else { a / b }),
        "idiv" => bin!(|a: f64, b: f64| if b as i64 == 0 {
            0.0
        } else {
            ((a as i64) / (b as i64)) as f64
        }),
        "mod" => bin!(|a: f64, b: f64| if b as i64 == 0 {
            0.0
        } else {
            ((a as i64) % (b as i64)) as f64
        }),
        "neg" => un!(|a: f64| -a),
        "atan" => bin!(|a: f64, b: f64| {
            let d = a.atan2(b).to_degrees();
            if d < 0.0 {
                d + 360.0
            } else {
                d
            }
        }),
        "ceiling" => un!(|a: f64| a.ceil()),
        "floor" => un!(|a: f64| a.floor()),
        "round" => un!(|a: f64| (a + 0.5).floor()),
        "truncate" | "cvi" => un!(|a: f64| a.trunc()),
        "cvr" => {}
        "cos" => un!(|a: f64| a.to_radians().cos()),
        "sin" => un!(|a: f64| a.to_radians().sin()),
        "exp" => bin!(|a: f64, b: f64| a.powf(b)),
        "ln" => un!(|a: f64| if a > 0.0 { a.ln() } else { 0.0 }),
        "log" => un!(|a: f64| if a > 0.0 { a.log10() } else { 0.0 }),
        "sqrt" => un!(|a: f64| a.max(0.0).sqrt()),
        "eq" => bin!(|a: f64, b: f64| bo(a == b)),
        "ne" => bin!(|a: f64, b: f64| bo(a != b)),
        "ge" => bin!(|a: f64, b: f64| bo(a >= b)),
        "gt" => bin!(|a: f64, b: f64| bo(a > b)),
        "le" => bin!(|a: f64, b: f64| bo(a <= b)),
        "lt" => bin!(|a: f64, b: f64| bo(a < b)),
        "and" => bin!(|a: f64, b: f64| ((a as i64) & (b as i64)) as f64),
        "or" => bin!(|a: f64, b: f64| ((a as i64) | (b as i64)) as f64),
        "xor" => bin!(|a: f64, b: f64| ((a as i64) ^ (b as i64)) as f64),
        "not" => un!(|a: f64| if a == 0.0 {
            1.0
        } else if a == 1.0 {
            0.0
        } else {
            !(a as i64) as f64
        }),
        "bitshift" => bin!(|a: f64, b: f64| {
            let (a, b) = (a as i64, b as i64);
            (if b >= 0 { a << b.min(40) } else { a >> (-b).min(40) }) as f64
        }),
        "true" => st.push(1.0),
        "false" => st.push(0.0),
        "pop" => {
            st.pop();
        }
        "exch" => {
            let n = st.len();
            if n >= 2 {
                st.swap(n - 1, n - 2);
            }
        }
        "dup" => {
            if let Some(&v) = st.last() {
                st.push(v);
            }
        }
        "copy" => {
            let n = pop!() as usize;
            if n <= st.len() {
                let s = st.len() - n;
                let part: Vec<f64> = st[s..].to_vec();
                st.extend(part);
            }
        }
        "index" => {
            let n = pop!() as usize;
            if n < st.len() {
                st.push(st[st.len() - 1 - n]);
            }
        }
        "roll" => {
            let j = pop!() as i64;
            let n = pop!() as usize;
            if n > 0 && n <= st.len() {
                let s = st.len() - n;
                let k = j.rem_euclid(n as i64) as usize;
                st[s..].rotate_right(k);
            }
        }
        _ => {}
    }
}

#[derive(Clone, Debug)]
enum FKind {
    Ident,
    Sampled {
        size: Vec<usize>,
        bps: usize,
        encode: Vec<f64>,
        decode: Vec<f64>,
        samples: Vec<f64>,
        n_out: usize,
    },
    Exp {
        c0: Vec<f64>,
        c1: Vec<f64>,
        n: f64,
    },
    Stitch {
        funcs: Vec<Func>,
        bounds: Vec<f64>,
        encode: Vec<f64>,
    },
    Calc(Vec<Ps>),
}

#[derive(Clone, Debug)]
pub struct Func {
    kind: FKind,
    domain: Vec<f64>,
    range: Vec<f64>,
}

impl Func {
    pub fn parse(pdf: &Pdf, o: &Obj) -> Option<Func> {
        Self::parse_depth(pdf, o, 0)
    }

    fn parse_depth(pdf: &Pdf, o: &Obj, depth: usize) -> Option<Func> {
        if depth > 8 {
            return None;
        }
        let o = pdf.res(o);
        if o.name() == Some("Identity") {
            return Some(Func {
                kind: FKind::Ident,
                domain: vec![0.0, 1.0],
                range: vec![],
            });
        }
        let d = o.dict()?;
        let domain = pdf.nums(pdf.get(d, "Domain"));
        let domain = if domain.len() >= 2 { domain } else { vec![0.0, 1.0] };
        let range = pdf.nums(pdf.get(d, "Range"));
        let kind = match pdf.num(d, "FunctionType")? as i32 {
            0 => {
                let Obj::Stream(s) = o else { return None };
                let size: Vec<usize> = pdf
                    .nums(pdf.get(d, "Size"))
                    .iter()
                    .map(|&v| v.max(1.0) as usize)
                    .collect();
                let bps = pdf.num(d, "BitsPerSample")? as usize;
                let n_out = range.len() / 2;
                if n_out == 0 || size.is_empty() || !matches!(bps, 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32) {
                    return None;
                }
                let total = size.iter().product::<usize>().checked_mul(n_out)?;
                if total > 8_000_000 {
                    return None;
                }
                let mut encode = pdf.nums(pdf.get(d, "Encode"));
                if encode.len() < size.len() * 2 {
                    encode = size.iter().flat_map(|&s| [0.0, (s - 1) as f64]).collect();
                }
                let mut decode = pdf.nums(pdf.get(d, "Decode"));
                if decode.len() < range.len() {
                    decode = range.clone();
                }
                let (data, _) = pdf.stream_data(s);
                let mut samples = Vec::with_capacity(total);
                let mut bitpos = 0usize;
                for _ in 0..total {
                    let mut v: u64 = 0;
                    for _ in 0..bps {
                        let byte = data.get(bitpos / 8).copied().unwrap_or(0);
                        v = (v << 1) | ((byte >> (7 - bitpos % 8)) & 1) as u64;
                        bitpos += 1;
                    }
                    samples.push(v as f64);
                }
                FKind::Sampled {
                    size,
                    bps,
                    encode,
                    decode,
                    samples,
                    n_out,
                }
            }
            2 => {
                let c0 = pdf.nums(pdf.get(d, "C0"));
                let c1 = pdf.nums(pdf.get(d, "C1"));
                FKind::Exp {
                    c0: if c0.is_empty() { vec![0.0] } else { c0 },
                    c1: if c1.is_empty() { vec![1.0] } else { c1 },
                    n: pdf.num(d, "N").unwrap_or(1.0),
                }
            }
            3 => {
                let funcs: Vec<Func> = match pdf.get(d, "Functions") {
                    Obj::Arr(a) => a
                        .iter()
                        .filter_map(|f| Func::parse_depth(pdf, f, depth + 1))
                        .collect(),
                    _ => return None,
                };
                if funcs.is_empty() {
                    return None;
                }
                let bounds = pdf.nums(pdf.get(d, "Bounds"));
                let mut encode = pdf.nums(pdf.get(d, "Encode"));
                while encode.len() < funcs.len() * 2 {
                    encode.extend([0.0, 1.0]);
                }
                FKind::Stitch {
                    funcs,
                    bounds,
                    encode,
                }
            }
            4 => {
                let Obj::Stream(s) = o else { return None };
                let (data, _) = pdf.stream_data(s);
                let mut lex = Lex::new(&data, false);
                // äußere geschweifte Klammer
                loop {
                    match lex.next() {
                        Tok::Kw(k) if k == "{" => break,
                        Tok::Eof => return None,
                        _ => {}
                    }
                }
                FKind::Calc(ps_block(&mut lex))
            }
            _ => return None,
        };
        Some(Func { kind, domain, range })
    }

    pub fn eval(&self, x: &[f64]) -> Vec<f64> {
        let mut xin: Vec<f64> = x.to_vec();
        for (i, v) in xin.iter_mut().enumerate() {
            if let (Some(&lo), Some(&hi)) = (self.domain.get(2 * i), self.domain.get(2 * i + 1)) {
                *v = v.clamp(lo.min(hi), lo.max(hi));
            }
        }
        let mut out = match &self.kind {
            FKind::Ident => xin.clone(),
            FKind::Exp { c0, c1, n } => {
                let t = xin.first().copied().unwrap_or(0.0);
                let p = if *n == 1.0 { t } else { t.max(0.0).powf(*n) };
                c0.iter()
                    .zip(c1.iter().chain(std::iter::repeat(&1.0)))
                    .map(|(a, b)| a + p * (b - a))
                    .collect()
            }
            FKind::Stitch {
                funcs,
                bounds,
                encode,
            } => {
                let t = xin.first().copied().unwrap_or(0.0);
                let (d0, d1) = (self.domain[0], self.domain[1]);
                let k = funcs.len();
                let mut i = 0;
                while i < k - 1 && i < bounds.len() && t >= bounds[i] {
                    i += 1;
                }
                let lo = if i == 0 { d0 } else { bounds[i - 1] };
                let hi = if i == k - 1 || i >= bounds.len() {
                    d1
                } else {
                    bounds[i]
                };
                let (e0, e1) = (encode[2 * i], encode[2 * i + 1]);
                let u = if (hi - lo).abs() < 1e-12 {
                    e0
                } else {
                    e0 + (t - lo) * (e1 - e0) / (hi - lo)
                };
                funcs[i].eval(&[u])
            }
            FKind::Calc(code) => {
                let mut st = xin.clone();
                ps_run(code, &mut st, 0);
                let n = self.range.len() / 2;
                if st.len() >= n {
                    st.split_off(st.len() - n)
                } else {
                    st.resize(n, 0.0);
                    st
                }
            }
            FKind::Sampled {
                size,
                bps,
                encode,
                decode,
                samples,
                n_out,
            } => {
                let m = size.len().min(xin.len()).min(8);
                let maxv = ((1u64 << *bps) - 1) as f64;
                let mut idx0 = vec![0usize; m];
                let mut frac = vec![0f64; m];
                for i in 0..m {
                    let (d0, d1) = (self.domain[2 * i], self.domain[2 * i + 1]);
                    let e = if (d1 - d0).abs() < 1e-12 {
                        encode[2 * i]
                    } else {
                        encode[2 * i] + (xin[i] - d0) * (encode[2 * i + 1] - encode[2 * i]) / (d1 - d0)
                    };
                    let e = e.clamp(0.0, (size[i] - 1) as f64);
                    idx0[i] = (e.floor() as usize).min(size[i] - 1);
                    frac[i] = e - idx0[i] as f64;
                }
                let mut res = vec![0f64; *n_out];
                for corner in 0..(1usize << m) {
                    let mut w = 1.0;
                    let mut index = 0usize;
                    let mut stride = 1usize;
                    for i in 0..m {
                        let hi = corner >> i & 1 == 1;
                        let ii = if hi {
                            (idx0[i] + 1).min(size[i] - 1)
                        } else {
                            idx0[i]
                        };
                        w *= if hi { frac[i] } else { 1.0 - frac[i] };
                        index += ii * stride;
                        stride *= size[i];
                    }
                    if w == 0.0 {
                        continue;
                    }
                    for j in 0..*n_out {
                        res[j] += w * samples.get(index * n_out + j).copied().unwrap_or(0.0);
                    }
                }
                for (j, r) in res.iter_mut().enumerate() {
                    let (d0, d1) = (decode[2 * j], decode[2 * j + 1]);
                    *r = d0 + *r * (d1 - d0) / maxv;
                }
                res
            }
        };
        for (j, v) in out.iter_mut().enumerate() {
            if let (Some(&lo), Some(&hi)) = (self.range.get(2 * j), self.range.get(2 * j + 1)) {
                *v = v.clamp(lo.min(hi), lo.max(hi));
            }
        }
        out
    }

    /// Stellen, an denen sich der Verlauf der Funktion knickt (für Farbverlaufsstopps).
    pub fn breaks(&self, lo: f64, hi: f64, out: &mut Vec<f64>) {
        out.push(lo);
        out.push(hi);
        match &self.kind {
            FKind::Exp { n, .. } if (*n - 1.0).abs() > 1e-9 => {
                for i in 1..8 {
                    out.push(lo + (hi - lo) * i as f64 / 8.0);
                }
            }
            FKind::Stitch {
                funcs,
                bounds,
                encode,
            } => {
                let k = funcs.len();
                let (d0, d1) = (self.domain[0], self.domain[1]);
                for i in 0..k {
                    let a = if i == 0 {
                        d0
                    } else {
                        bounds.get(i - 1).copied().unwrap_or(d1)
                    };
                    let b = if i == k - 1 {
                        d1
                    } else {
                        bounds.get(i).copied().unwrap_or(d1)
                    };
                    if a >= lo - 1e-9 && a <= hi + 1e-9 {
                        out.push(a);
                    }
                    if b >= lo - 1e-9 && b <= hi + 1e-9 {
                        out.push(b);
                    }
                    // Unterfunktion: Knickstellen zurückrechnen
                    let (e0, e1) = (encode[2 * i], encode[2 * i + 1]);
                    let mut sub = Vec::new();
                    funcs[i].breaks(funcs[i].domain[0], funcs[i].domain[1], &mut sub);
                    if (e1 - e0).abs() > 1e-12 {
                        for u in sub {
                            let t = a + (u - e0) * (b - a) / (e1 - e0);
                            if t >= a - 1e-9 && t <= b + 1e-9 && t >= lo - 1e-9 && t <= hi + 1e-9 {
                                out.push(t);
                            }
                        }
                    }
                }
            }
            FKind::Sampled { .. } | FKind::Calc(_) => {
                for i in 1..16 {
                    out.push(lo + (hi - lo) * i as f64 / 16.0);
                }
            }
            _ => {}
        }
    }
}

// ───────────────────────────── Farbräume ─────────────────────────────

#[derive(Clone, Debug)]
pub enum Cs {
    Gray,
    Rgb,
    Cmyk,
    Lab {
        white: [f64; 3],
        range: [f64; 4],
    },
    Indexed {
        base: Rc<Cs>,
        hival: usize,
        table: Rc<Vec<u8>>,
    },
    Tint {
        n: usize,
        alt: Rc<Cs>,
        funcs: Rc<Vec<Func>>,
    },
    Pattern,
}

fn cl(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

/// CMYK → sRGB (Näherung wie in pdf.js, ähnelt dem Adobe-Standardprofil).
pub fn cmyk_to_rgb(c: f64, m: f64, y: f64, k: f64) -> [f64; 3] {
    let (c, m, y, k) = (cl(c), cl(m), cl(y), cl(k));
    let r = 255.0
        + c * (-4.387332384609988 * c
            + 54.48615194189176 * m
            + 18.82290502165302 * y
            + 212.25662451639585 * k
            - 285.2331026137004)
        + m * (1.7149763477362134 * m - 5.6096736904047315 * y - 17.873870861415444 * k - 5.497006427196366)
        + y * (-2.5217340131683033 * y - 21.248923337353073 * k + 17.5119270841813)
        + k * (-21.86122147463605 * k - 189.48180835922747);
    let g = 255.0
        + c * (8.841041422036149 * c
            + 60.118027045597366 * m
            + 6.871425592049007 * y
            + 31.159100130055922 * k
            - 79.2970844816548)
        + m * (-15.310361306967817 * m + 17.575251261109482 * y + 131.35250912493976 * k - 190.9453302588951)
        + y * (4.444339102852739 * y + 9.8632861493405 * k - 24.86741582555878)
        + k * (-20.737325471181034 * k - 187.80453709719578);
    let b = 255.0
        + c * (0.8842522430003296 * c + 8.078677503112928 * m + 30.89978309703729 * y
            - 0.23883238689178934 * k
            - 14.183576799673286)
        + m * (10.49593273432072 * m + 63.02378494754052 * y + 50.606957656360734 * k - 112.23884253719248)
        + y * (0.03296041114873217 * y + 115.60384449646641 * k - 193.58209356861505)
        + k * (-22.33816807309886 * k - 180.12613974708367);
    [cl(r / 255.0), cl(g / 255.0), cl(b / 255.0)]
}

fn srgb_gamma(v: f64) -> f64 {
    let v = cl(v);
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

impl Cs {
    pub fn n(&self) -> usize {
        match self {
            Cs::Gray => 1,
            Cs::Rgb => 3,
            Cs::Cmyk => 4,
            Cs::Lab { .. } => 3,
            Cs::Indexed { .. } => 1,
            Cs::Tint { n, .. } => *n,
            Cs::Pattern => 0,
        }
    }

    /// Anfangsfarbe nach `cs`/`CS`.
    pub fn initial(&self) -> Vec<f64> {
        match self {
            Cs::Cmyk => vec![0.0, 0.0, 0.0, 1.0],
            Cs::Tint { n, .. } => vec![1.0; *n],
            other => vec![0.0; other.n()],
        }
    }

    pub fn to_rgb(&self, c: &[f64]) -> [f64; 3] {
        let g = |i: usize| c.get(i).copied().unwrap_or(0.0);
        match self {
            Cs::Gray => {
                let v = cl(g(0));
                [v, v, v]
            }
            Cs::Rgb => [cl(g(0)), cl(g(1)), cl(g(2))],
            Cs::Cmyk => cmyk_to_rgb(g(0), g(1), g(2), g(3)),
            Cs::Lab { white, range } => {
                let l = g(0).clamp(0.0, 100.0);
                let a = g(1).clamp(range[0], range[1]);
                let b = g(2).clamp(range[2], range[3]);
                let m = (l + 16.0) / 116.0;
                let (ll, mm, nn) = (m + a / 500.0, m, m - b / 200.0);
                let f = |x: f64| {
                    if x >= 6.0 / 29.0 {
                        x * x * x
                    } else {
                        108.0 / 841.0 * (x - 4.0 / 29.0)
                    }
                };
                // auf D65 bezogen (grobe Anpassung der Weißpunkte)
                let x = f(ll) * white[0] / white[0] * 0.95047;
                let y = f(mm) * white[1] / white[1];
                let z = f(nn) * white[2] / white[2] * 1.08883;
                let r = 3.2406 * x - 1.5372 * y - 0.4986 * z;
                let gg = -0.9689 * x + 1.8758 * y + 0.0415 * z;
                let bb = 0.0557 * x - 0.2040 * y + 1.0570 * z;
                [srgb_gamma(r), srgb_gamma(gg), srgb_gamma(bb)]
            }
            Cs::Indexed { base, hival, table } => {
                let i = (g(0).round().max(0.0) as usize).min(*hival);
                let n = base.n();
                let comps: Vec<f64> = (0..n)
                    .map(|k| table.get(i * n + k).copied().unwrap_or(0) as f64 / 255.0)
                    .collect();
                if let Cs::Lab { range, .. } = &**base {
                    let l = comps[0] * 100.0;
                    let a = range[0] + comps[1] * (range[1] - range[0]);
                    let b = range[2] + comps[2] * (range[3] - range[2]);
                    return base.to_rgb(&[l, a, b]);
                }
                base.to_rgb(&comps)
            }
            Cs::Tint { alt, funcs, .. } => {
                let mut out = Vec::new();
                for f in funcs.iter() {
                    out.extend(f.eval(c));
                }
                alt.to_rgb(&out)
            }
            Cs::Pattern => [0.0, 0.0, 0.0],
        }
    }

    pub fn parse(pdf: &Pdf, o: &Obj, res: &Dict, depth: usize) -> Cs {
        if depth > 6 {
            return Cs::Gray;
        }
        let o = pdf.res(o);
        match o {
            Obj::Name(n) => match n.as_str() {
                "DeviceGray" | "G" | "CalGray" => Cs::Gray,
                "DeviceRGB" | "RGB" | "CalRGB" => Cs::Rgb,
                "DeviceCMYK" | "CMYK" => Cs::Cmyk,
                "Pattern" => Cs::Pattern,
                other => {
                    if let Some(r) = pdf.get(res, "ColorSpace").dict().and_then(|d| d.get(other)) {
                        if r.name() != Some(other) {
                            return Cs::parse(pdf, r, res, depth + 1);
                        }
                    }
                    Cs::Gray
                }
            },
            Obj::Arr(a) if !a.is_empty() => {
                let Some(kind) = pdf.res(&a[0]).name() else {
                    return Cs::Gray;
                };
                let arg = |i: usize| a.get(i).map(|x| pdf.res(x)).unwrap_or(&pdf.null);
                match kind {
                    "DeviceGray" | "G" | "CalGray" => Cs::Gray,
                    "DeviceRGB" | "RGB" | "CalRGB" => Cs::Rgb,
                    "DeviceCMYK" | "CMYK" => Cs::Cmyk,
                    "ICCBased" => {
                        let d = arg(1).dict();
                        match d.and_then(|d| pdf.num(d, "N")).map(|n| n as i32) {
                            Some(1) => Cs::Gray,
                            Some(4) => Cs::Cmyk,
                            Some(3) => Cs::Rgb,
                            _ => match d.map(|d| pdf.get(d, "Alternate")) {
                                Some(alt) if !matches!(alt, Obj::Null) => Cs::parse(pdf, alt, res, depth + 1),
                                _ => Cs::Rgb,
                            },
                        }
                    }
                    "Lab" => {
                        let d = arg(1).dict();
                        let w = d.map(|d| pdf.nums(pdf.get(d, "WhitePoint"))).unwrap_or_default();
                        let r = d.map(|d| pdf.nums(pdf.get(d, "Range"))).unwrap_or_default();
                        Cs::Lab {
                            white: [
                                w.first().copied().unwrap_or(0.9505),
                                w.get(1).copied().unwrap_or(1.0),
                                w.get(2).copied().unwrap_or(1.089),
                            ],
                            range: [
                                r.first().copied().unwrap_or(-100.0),
                                r.get(1).copied().unwrap_or(100.0),
                                r.get(2).copied().unwrap_or(-100.0),
                                r.get(3).copied().unwrap_or(100.0),
                            ],
                        }
                    }
                    "Indexed" | "I" => {
                        let base = Cs::parse(pdf, arg(1), res, depth + 1);
                        let hival = arg(2).num().unwrap_or(255.0).clamp(0.0, 255.0) as usize;
                        let table = match arg(3) {
                            Obj::Str(s) => s.clone(),
                            Obj::Stream(s) => pdf.stream_data(s).0,
                            _ => Vec::new(),
                        };
                        Cs::Indexed {
                            base: Rc::new(base),
                            hival,
                            table: Rc::new(table),
                        }
                    }
                    "Separation" | "DeviceN" => {
                        let n = if kind == "Separation" {
                            1
                        } else {
                            arg(1).arr().map(|v| v.len()).unwrap_or(1)
                        };
                        let alt = Cs::parse(pdf, arg(2), res, depth + 1);
                        let f = Func::parse(pdf, arg(3));
                        match f {
                            Some(f) => Cs::Tint {
                                n,
                                alt: Rc::new(alt),
                                funcs: Rc::new(vec![f]),
                            },
                            None => Cs::Gray,
                        }
                    }
                    "Pattern" => Cs::Pattern,
                    _ => Cs::Gray,
                }
            }
            _ => Cs::Gray,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_or_stray_tokens_do_not_overflow() {
        let n = 200_000;
        for data in ["[".repeat(n), "<< /A ".repeat(n), ")".repeat(n), "> ) ".repeat(n)] {
            let mut lex = Lex::new(data.as_bytes(), true);
            while !matches!(lex.next(), Tok::Eof) {}
        }
    }
}
