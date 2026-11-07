// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © Bendix Hüsing

//! Zellen mit Formeln wie in einer Tabellenkalkulation: Zahlen, Text und Formeln
//! (`=SUMME(A1:A3)*2`), ausgewertet mit Bezügen auf andere Zellen.
//!
//! Zellen heißen wie üblich A1, B2 … (Spalte als Buchstaben, Zeile ab 1); Bereiche A1:B3.
//! Rechenarten: + − * / ^ %, & (Text verbinden), Vergleiche = <> < > <= >=.
//! Funktionen wie in LibreOffice Calc und Excel (englische Namen und die deutschen,
//! spanischen und französischen): Rechnen, Statistik, Bedingungen (SUMIF …), Logik, Text,
//! Nachschlagen (VLOOKUP, INDEX, MATCH, XLOOKUP …) und Datum – siehe [`FUNCTIONS`].
//!
//! Dezimaltrennzeichen: in Sprachen mit Komma (Deutsch, Spanisch, Französisch) ist das Komma
//! zwischen Ziffern ein Dezimalkomma und Argumente werden mit Semikolon getrennt – wie in den
//! übersetzten Tabellenkalkulationen. Der Punkt gilt immer als Dezimalpunkt.

use std::collections::HashMap;

/// Ergebnis einer Zelle.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Empty,
    Num(f64),
    Text(String),
    Bool(bool),
    /// Datum als fortlaufende Zahl wie in Tabellenkalkulationen (Tage seit dem 30.12.1899)
    Date(f64),
    /// Uhrzeit bzw. Dauer als Bruchteil eines Tages
    Time(f64),
    /// Fehler wie in Tabellenkalkulationen (#DIV/0!, #REF!, #NAME?, #VALUE!, #CYCLE!)
    Err(&'static str),
}

impl Value {
    /// Zahl für Rechnungen (leer = 0, Wahrheitswert = 1/0); Text ist ein Fehler.
    fn num(&self) -> Result<f64, &'static str> {
        match self {
            Value::Empty => Ok(0.0),
            Value::Num(n) | Value::Date(n) | Value::Time(n) => Ok(*n),
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            Value::Text(t) => parse_number(t, true)
                .or_else(|| parse_number(t, false))
                .or_else(|| parse_date(t))
                .or_else(|| parse_time(t))
                .ok_or("#VALUE!"),
            Value::Err(e) => Err(e),
        }
    }

    pub fn as_num(&self) -> Option<f64> {
        match self {
            Value::Num(n) | Value::Date(n) | Value::Time(n) => Some(*n),
            _ => None,
        }
    }

    fn truthy(&self) -> Result<bool, &'static str> {
        match self {
            Value::Bool(b) => Ok(*b),
            Value::Err(e) => Err(e),
            Value::Text(t) => Ok(!t.is_empty()),
            v => Ok(v.num()? != 0.0),
        }
    }

    /// Anzeige: Zahlen gerundet (höchstens 10 gültige Stellen, ohne überflüssige Nullen).
    pub fn display(&self, decimal_comma: bool) -> String {
        match self {
            Value::Empty => String::new(),
            Value::Num(n) => format_number(*n, decimal_comma),
            Value::Date(n) => format_date(*n),
            Value::Time(n) => format_time(*n),
            Value::Text(t) => t.clone(),
            Value::Bool(b) => bool_word(*b).to_string(),
            Value::Err(e) => e.to_string(),
        }
    }
}

/// Ist der Wert eine Zahl (auch ein Datum)? Zahlen stehen in Tabellen rechts.
impl Value {
    pub fn is_number(&self) -> bool {
        matches!(self, Value::Num(_) | Value::Date(_) | Value::Time(_))
    }
}

/// WAHR/FALSCH … in der Sprache der Oberfläche.
fn bool_word(b: bool) -> &'static str {
    use crate::i18n::Lang;
    match (crate::i18n::lang(), b) {
        (Lang::De, true) => "WAHR",
        (Lang::De, false) => "FALSCH",
        (Lang::Es, true) => "VERDADERO",
        (Lang::Es, false) => "FALSO",
        (Lang::Fr, true) => "VRAI",
        (Lang::Fr, false) => "FAUX",
        (_, true) => "TRUE",
        (_, false) => "FALSE",
    }
}

/// Tage seit dem 1.1.1970 → (Jahr, Monat, Tag) (Kalender nach Howard Hinnant).
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// (Jahr, Monat, Tag) → Tage seit dem 1.1.1970; Monate über 12 bzw. unter 1 laufen weiter.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y + (m - 1).div_euclid(12);
    let m = (m - 1).rem_euclid(12) + 1;
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Tage zwischen dem 30.12.1899 (Tag 0 der Tabellenkalkulationen) und dem 1.1.1970.
const EPOCH: i64 = 25_569;

/// Fortlaufende Zahl → (Jahr, Monat, Tag).
pub fn date_parts(serial: f64) -> (i64, u32, u32) {
    civil(serial.floor() as i64 - EPOCH)
}

/// (Jahr, Monat, Tag) → fortlaufende Zahl.
pub fn date_serial(y: i64, m: i64, d: i64) -> f64 {
    (days_from_civil(y, m, d) + EPOCH) as f64
}

/// Datum so, wie man es in der Sprache schreibt (TT.MM.JJJJ bzw. TT/MM/JJJJ), mit Uhrzeit,
/// wenn es eine hat.
fn format_date(serial: f64) -> String {
    let (y, m, d) = date_parts(serial);
    let day = if crate::i18n::lang() == crate::i18n::Lang::De {
        format!("{d:02}.{m:02}.{y}")
    } else {
        format!("{d:02}/{m:02}/{y}")
    };
    if (serial - serial.floor()) * 1440.0 >= 0.5 {
        format!("{day} {}", format_time(serial - serial.floor()))
    } else {
        day
    }
}

/// Uhrzeit (hh:mm, mit Sekunden, wenn es welche gibt); ab einem Tag als Dauer in Stunden.
fn format_time(t: f64) -> String {
    let secs = (t * 86_400.0).round() as i64;
    let (h, m, sec) = (secs / 3600, secs / 60 % 60, secs % 60);
    let h = if t >= 1.0 { h } else { h % 24 };
    if sec != 0 {
        format!("{h:02}:{m:02}:{sec:02}")
    } else {
        format!("{h:02}:{m:02}")
    }
}

/// Eingabe als Uhrzeit: „14:30“, „14:30:15“, „9:05“.
fn parse_time(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if !(2..=3).contains(&parts.len())
        || !parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 2 && p.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let n: Vec<u32> = parts.iter().filter_map(|p| p.parse().ok()).collect();
    let (h, m, sec) = (n[0], n[1], n.get(2).copied().unwrap_or(0));
    if h > 23 || m > 59 || sec > 59 {
        return None;
    }
    Some((h * 3600 + m * 60 + sec) as f64 / 86_400.0)
}

/// Eingabe als Datum: „5.11.2026“, „05/11/2026“ (Tag zuerst), „2026-11-05“.
fn parse_date(s: &str) -> Option<f64> {
    let s = s.trim();
    let parts: Vec<&str> = s.split(['.', '/', '-']).collect();
    if parts.len() != 3
        || !parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let n: Vec<i64> = parts.iter().filter_map(|p| p.parse().ok()).collect();
    let (y, m, d) = if parts[0].len() == 4 && s.contains('-') {
        (n[0], n[1], n[2])
    } else if parts[2].len() == 4 && !s.contains('-') {
        (n[2], n[1], n[0])
    } else {
        return None;
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || !(1900..=9999).contains(&y) {
        return None;
    }
    let serial = date_serial(y, m, d);
    // 31.02. u. Ä. gibt es nicht
    (date_parts(serial) == (y, m as u32, d as u32)).then_some(serial)
}

/// Zahl lesbar ausgeben: höchstens 10 gültige Stellen, ohne Nullen am Ende.
pub fn format_number(n: f64, decimal_comma: bool) -> String {
    if !n.is_finite() {
        return "#NUM!".into();
    }
    if n == 0.0 {
        return "0".into();
    }
    let digits = (10 - (n.abs().log10().floor() as i32 + 1)).clamp(0, 10) as usize;
    let mut s = format!("{:.*}", digits, n);
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
    if decimal_comma {
        s = s.replace('.', ",");
    }
    s
}

/// Zahl aus einer Eingabe: „12“, „-3.5“, „3,5“, „1.234,5“, „1,234.5“, „25 %“. Mit `comma`
/// darf das Komma Dezimaltrennzeichen sein (sonst nur Tausendertrennung).
pub fn parse_number(s: &str, comma: bool) -> Option<f64> {
    let mut t = s.trim().replace([' ', '\u{a0}', '\u{202f}'], "");
    let percent = t.ends_with('%');
    if percent {
        t.pop();
    }
    if t.is_empty()
        || !t
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-' | '+' | 'e' | 'E'))
    {
        return None;
    }
    let (dots, commas) = (t.matches('.').count(), t.matches(',').count());
    let norm = match (dots, commas) {
        (_, 0) => t.clone(),
        (0, 1) if comma => t.replace(',', "."),
        (0, _) => t.replace(',', ""),
        _ => {
            // beide: das letzte ist das Dezimaltrennzeichen
            if t.rfind(',') > t.rfind('.') {
                t.replace('.', "").replace(',', ".")
            } else {
                t.replace(',', "")
            }
        }
    };
    let v: f64 = norm.parse().ok()?;
    Some(if percent { v / 100.0 } else { v })
}

/// Spaltenname: 0 → A, 25 → Z, 26 → AA.
pub fn col_name(mut c: usize) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (c % 26) as u8);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

/// Zellbezug „B3“ → (Zeile 2, Spalte 1); `$` (absolut) wird überlesen.
pub fn parse_ref(s: &str) -> Option<(usize, usize)> {
    let s = s.replace('$', "");
    let split = s.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = s.split_at(split);
    if letters.is_empty() || letters.len() > 3 || !letters.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut col = 0usize;
    for c in letters.chars() {
        col = col * 26 + (c.to_ascii_uppercase() as u8 - b'A' + 1) as usize;
    }
    let row: usize = digits.parse().ok()?;
    if row == 0 || row > 100_000 {
        return None;
    }
    Some((row - 1, col - 1))
}

/// Zellbezüge einer Formel umschreiben: `f` bekommt Zeile, Spalte und ob sie mit `$`
/// festgehalten sind und liefert die neue Zeile und Spalte (`None`: Bezug ungültig →
/// `#REF!`). Text in Anführungszeichen bleibt unberührt.
pub fn map_refs(formula: &str, f: impl Fn(usize, usize, bool, bool) -> Option<(usize, usize)>) -> String {
    map_refs_in(formula, |r, c, ar, ac, _| f(r, c, ar, ac))
}

/// Stellung eines Bezugs: allein, Anfang oder Ende eines Bereichs (`A1:B4`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefRole {
    Single,
    Start,
    End,
}

/// Wie `map_refs`, `f` erfährt zusätzlich, ob der Bezug Anfang oder Ende eines Bereichs ist.
pub fn map_refs_in(
    formula: &str,
    f: impl Fn(usize, usize, bool, bool, RefRole) -> Option<(usize, usize)>,
) -> String {
    let c: Vec<char> = formula.chars().collect();
    let mut out = String::with_capacity(formula.len());
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch == '"' {
            // Text bis zum schließenden Anführungszeichen
            out.push(ch);
            i += 1;
            while i < c.len() {
                out.push(c[i]);
                i += 1;
                if c[i - 1] == '"' {
                    break;
                }
            }
            continue;
        }
        let word_start = i == 0 || !(c[i - 1].is_alphanumeric() || c[i - 1] == '_' || c[i - 1] == '.');
        if word_start && (ch == '$' || ch.is_ascii_alphabetic()) {
            // $?Buchstaben $?Ziffern, danach kein Buchstabe, keine Ziffer, keine Klammer
            let mut j = i;
            let abs_col = c[j] == '$';
            if abs_col {
                j += 1;
            }
            let l0 = j;
            while j < c.len() && c[j].is_ascii_alphabetic() {
                j += 1;
            }
            let letters: String = c[l0..j].iter().collect();
            let abs_row = j < c.len() && c[j] == '$';
            if abs_row {
                j += 1;
            }
            let d0 = j;
            while j < c.len() && c[j].is_ascii_digit() {
                j += 1;
            }
            let digits: String = c[d0..j].iter().collect();
            let ends = j >= c.len() || !(c[j].is_alphanumeric() || c[j] == '_' || c[j] == '(' || c[j] == '.');
            if !letters.is_empty() && letters.len() <= 3 && !digits.is_empty() && ends {
                if let Some((r, col)) = parse_ref(&format!("{letters}{digits}")) {
                    let role = if j < c.len() && c[j] == ':' {
                        RefRole::Start
                    } else if i > 0 && c[i - 1] == ':' {
                        RefRole::End
                    } else {
                        RefRole::Single
                    };
                    match f(r, col, abs_row, abs_col, role) {
                        Some((nr, nc)) => {
                            if abs_col {
                                out.push('$');
                            }
                            out.push_str(&col_name(nc));
                            if abs_row {
                                out.push('$');
                            }
                            out.push_str(&(nr + 1).to_string());
                        }
                        None => out.push_str("#REF!"),
                    }
                    i = j;
                    continue;
                }
            }
            // kein Bezug: das ganze Wort übernehmen
            let mut k = i + 1;
            while k < c.len() && (c[k].is_alphanumeric() || c[k] == '_' || c[k] == '.' || c[k] == '$') {
                k += 1;
            }
            out.extend(&c[i..k]);
            i = k;
            continue;
        }
        out.push(ch);
        i += 1;
    }
    out
}

/// Farben für Bezüge in einer Formel beim Bearbeiten (wie in Tabellenkalkulationen).
pub const REF_COLORS: [(f64, f64, f64); 7] = [
    (0.208, 0.518, 0.894),
    (0.878, 0.106, 0.141),
    (0.569, 0.255, 0.675),
    (0.149, 0.635, 0.412),
    (0.902, 0.380, 0.0),
    (0.0, 0.565, 0.604),
    (0.753, 0.169, 0.537),
];

/// Bezüge in einer Formel: (Byte von, Byte bis, Bereich oben, links, unten, rechts, Farbe).
/// Gleiche Bereiche bekommen dieselbe Farbe. Text in Anführungszeichen zählt nicht.
pub fn formula_refs(formula: &str) -> Vec<(usize, usize, (usize, usize, usize, usize), usize)> {
    let mut out = Vec::new();
    if !formula.trim_start().starts_with('=') {
        return out;
    }
    let b = formula.as_bytes();
    // ein Bezug ab Byte i: $?Buchstaben$?Ziffern → (Ende, Zeile, Spalte)
    let one = |i: usize| -> Option<(usize, usize, usize)> {
        let mut j = i;
        if j < b.len() && b[j] == b'$' {
            j += 1;
        }
        let l0 = j;
        while j < b.len() && b[j].is_ascii_alphabetic() {
            j += 1;
        }
        if j == l0 || j - l0 > 3 {
            return None;
        }
        if j < b.len() && b[j] == b'$' {
            j += 1;
        }
        let d0 = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j == d0 {
            return None;
        }
        if j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b'(' || b[j] == b'.') {
            return None;
        }
        let (r, c) = parse_ref(&formula[i..j])?;
        Some((j, r, c))
    };
    let mut seen: Vec<(usize, usize, usize, usize)> = Vec::new();
    let mut i = 0;
    let mut quoted = false;
    while i < b.len() {
        let ch = b[i];
        if ch == b'"' {
            quoted = !quoted;
            i += 1;
            continue;
        }
        let start_ok = i == 0
            || !(b[i - 1].is_ascii_alphanumeric()
                || b[i - 1] == b'_'
                || b[i - 1] == b'.'
                || b[i - 1] >= 0x80);
        if !quoted && start_ok && (ch == b'$' || ch.is_ascii_alphabetic()) {
            if let Some((j, r, c)) = one(i) {
                let (mut end, mut rg) = (j, (r, c, r, c));
                if j < b.len() && b[j] == b':' {
                    if let Some((k, r2, c2)) = one(j + 1) {
                        end = k;
                        rg = (r.min(r2), c.min(c2), r.max(r2), c.max(c2));
                    }
                }
                let color = match seen.iter().position(|x| *x == rg) {
                    Some(k) => k,
                    None => {
                        seen.push(rg);
                        seen.len() - 1
                    }
                } % REF_COLORS.len();
                out.push((i, end, rg, color));
                i = end;
                continue;
            }
            // ganzes Wort überspringen (Funktionsname)
            while i < b.len()
                && (b[i].is_ascii_alphanumeric()
                    || b[i] == b'_'
                    || b[i] == b'.'
                    || b[i] == b'$'
                    || b[i] >= 0x80)
            {
                i += 1;
            }
            continue;
        }
        i += 1;
    }
    out
}

/// Funktion vorschlagen, deren Name mit dem Wort vor dem Cursor beginnt (in einer Formel,
/// nicht in Anführungszeichen). Liefert (fehlender Rest, ganzer Name); bei einem schon
/// vollständigen Namen ist der Rest leer.
pub fn suggest_function(text: &str, caret: usize) -> Option<(String, String)> {
    if !text.starts_with('=') || caret > text.len() || !text.is_char_boundary(caret) {
        return None;
    }
    // nicht in Text in Anführungszeichen
    if text[..caret].matches('"').count() % 2 == 1 {
        return None;
    }
    // dahinter darf kein Wort weitergehen
    if text[caret..]
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '.' || c == '(' || c == '_')
    {
        return None;
    }
    let before = &text[..caret];
    let start = before
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '.' || *c == '_')
        .last()
        .map(|(i, _)| i)?;
    let word = &before[start..];
    if !word.chars().next().is_some_and(|c| c.is_alphabetic()) {
        return None;
    }
    let up = word.to_uppercase();
    let all = FUNCTIONS
        .iter()
        .map(|(en, _)| *en)
        .chain(FUNCTIONS.iter().flat_map(|(_, other)| other.iter().copied()));
    let mut best: Option<&str> = None;
    for name in all {
        if name == up {
            best = Some(name);
            break;
        }
        if best.is_none() && name.starts_with(&up) {
            best = Some(name);
        }
    }
    let full = best?;
    Some((full[up.len()..].to_string(), full.to_string()))
}

/// Darf an dieser Stelle der Formel ein Bezug stehen (nach =, (, ;, Rechenzeichen …)?
pub fn wants_ref(formula: &str, caret: usize) -> bool {
    if !formula.starts_with('=') || caret == 0 || caret > formula.len() {
        return false;
    }
    let before = formula[..caret].trim_end();
    before.chars().last().is_some_and(|c| "=(;,+-*/^&<>:".contains(c))
}

/// Formel wie beim Ausfüllen versetzen: Bezüge ohne `$` wandern um `dr` Zeilen und `dc`
/// Spalten mit.
pub fn shift_formula(formula: &str, dr: isize, dc: isize) -> String {
    map_refs(formula, |r, c, ar, ac| {
        let nr = if ar { r as isize } else { r as isize + dr };
        let nc = if ac { c as isize } else { c as isize + dc };
        (nr >= 0 && nc >= 0).then_some((nr as usize, nc as usize))
    })
}

// ───────────────────────────── Formeln lesen ─────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Op(&'static str),
    LParen,
    RParen,
    Sep,
    Colon,
}

fn tokenize(s: &str, comma: bool) -> Result<Vec<Tok>, &'static str> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
            continue;
        }
        if ch.is_ascii_digit() || (ch == '.' && c.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = i;
            while i < c.len()
                && (c[i].is_ascii_digit()
                    || c[i] == '.'
                    || (comma && c[i] == ',' && c.get(i + 1).is_some_and(|d| d.is_ascii_digit())))
            {
                i += 1;
            }
            // Exponent
            if i < c.len() && (c[i] == 'e' || c[i] == 'E') {
                let mut j = i + 1;
                if j < c.len() && (c[j] == '+' || c[j] == '-') {
                    j += 1;
                }
                if j < c.len() && c[j].is_ascii_digit() {
                    i = j;
                    while i < c.len() && c[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let t: String = c[start..i].iter().collect::<String>().replace(',', ".");
            out.push(Tok::Num(t.parse().map_err(|_| "#VALUE!")?));
            continue;
        }
        if ch == '"' {
            let mut t = String::new();
            i += 1;
            loop {
                match c.get(i) {
                    None => return Err("#VALUE!"),
                    Some('"') if c.get(i + 1) == Some(&'"') => {
                        t.push('"');
                        i += 2;
                    }
                    Some('"') => {
                        i += 1;
                        break;
                    }
                    Some(&x) => {
                        t.push(x);
                        i += 1;
                    }
                }
            }
            out.push(Tok::Str(t));
            continue;
        }
        if ch.is_alphabetic() || ch == '$' || ch == '_' {
            let start = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '$' || c[i] == '_' || c[i] == '.') {
                i += 1;
            }
            out.push(Tok::Ident(c[start..i].iter().collect()));
            continue;
        }
        let two: String = c[i..(i + 2).min(c.len())].iter().collect();
        if let Some(op) = ["<=", ">=", "<>"].into_iter().find(|o| *o == two) {
            out.push(Tok::Op(op));
            i += 2;
            continue;
        }
        let tok = match ch {
            '+' => Tok::Op("+"),
            '-' | '−' => Tok::Op("-"),
            '*' | '×' => Tok::Op("*"),
            '/' | '÷' => Tok::Op("/"),
            '^' => Tok::Op("^"),
            '%' => Tok::Op("%"),
            '&' => Tok::Op("&"),
            '=' => Tok::Op("="),
            '<' => Tok::Op("<"),
            '>' => Tok::Op(">"),
            '(' => Tok::LParen,
            ')' => Tok::RParen,
            ',' | ';' => Tok::Sep,
            ':' => Tok::Colon,
            _ => return Err("#NAME?"),
        };
        out.push(tok);
        i += 1;
    }
    Ok(out)
}

#[derive(Clone, Debug)]
enum Expr {
    Num(f64),
    Str(String),
    Bool(bool),
    Ref(usize, usize),
    Range((usize, usize), (usize, usize)),
    Neg(Box<Expr>),
    Percent(Box<Expr>),
    Bin(&'static str, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn op(&self, ops: &[&str]) -> Option<&'static str> {
        match self.peek() {
            Some(Tok::Op(o)) if ops.contains(o) => Some(o),
            _ => None,
        }
    }

    fn compare(&mut self) -> Result<Expr, &'static str> {
        let mut a = self.concat()?;
        while let Some(o) = self.op(&["=", "<>", "<", ">", "<=", ">="]) {
            self.pos += 1;
            a = Expr::Bin(o, Box::new(a), Box::new(self.concat()?));
        }
        Ok(a)
    }

    fn concat(&mut self) -> Result<Expr, &'static str> {
        let mut a = self.add()?;
        while self.op(&["&"]).is_some() {
            self.pos += 1;
            a = Expr::Bin("&", Box::new(a), Box::new(self.add()?));
        }
        Ok(a)
    }

    fn add(&mut self) -> Result<Expr, &'static str> {
        let mut a = self.mul()?;
        while let Some(o) = self.op(&["+", "-"]) {
            self.pos += 1;
            a = Expr::Bin(o, Box::new(a), Box::new(self.mul()?));
        }
        Ok(a)
    }

    fn mul(&mut self) -> Result<Expr, &'static str> {
        let mut a = self.pow()?;
        while let Some(o) = self.op(&["*", "/"]) {
            self.pos += 1;
            a = Expr::Bin(o, Box::new(a), Box::new(self.pow()?));
        }
        Ok(a)
    }

    /// Potenz: linksassoziativ, das Vorzeichen bindet stärker (−2^2 = 4, 2^3^2 = 64) – wie
    /// in Tabellenkalkulationen.
    fn pow(&mut self) -> Result<Expr, &'static str> {
        let mut a = self.unary()?;
        while self.op(&["^"]).is_some() {
            self.pos += 1;
            a = Expr::Bin("^", Box::new(a), Box::new(self.unary()?));
        }
        Ok(a)
    }

    fn unary(&mut self) -> Result<Expr, &'static str> {
        match self.op(&["-", "+"]) {
            Some("-") => {
                self.pos += 1;
                Ok(Expr::Neg(Box::new(self.unary()?)))
            }
            Some(_) => {
                self.pos += 1;
                self.unary()
            }
            None => self.postfix(),
        }
    }

    fn postfix(&mut self) -> Result<Expr, &'static str> {
        let mut a = self.primary()?;
        while self.op(&["%"]).is_some() {
            self.pos += 1;
            a = Expr::Percent(Box::new(a));
        }
        Ok(a)
    }

    fn primary(&mut self) -> Result<Expr, &'static str> {
        match self.next() {
            Some(Tok::Num(n)) => Ok(Expr::Num(n)),
            Some(Tok::Str(s)) => Ok(Expr::Str(s)),
            Some(Tok::LParen) => {
                let e = self.compare()?;
                match self.next() {
                    Some(Tok::RParen) => Ok(e),
                    _ => Err("#VALUE!"),
                }
            }
            Some(Tok::Ident(name)) => {
                if self.peek() == Some(&Tok::LParen) {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if self.peek() == Some(&Tok::RParen) {
                        self.pos += 1;
                    } else {
                        loop {
                            args.push(self.compare()?);
                            match self.next() {
                                Some(Tok::Sep) => continue,
                                Some(Tok::RParen) => break,
                                _ => return Err("#VALUE!"),
                            }
                        }
                    }
                    return Ok(Expr::Call(name.to_uppercase(), args));
                }
                let up = name.to_uppercase();
                if matches!(up.as_str(), "TRUE" | "WAHR" | "VERDADERO" | "VRAI") {
                    return Ok(Expr::Bool(true));
                }
                if matches!(up.as_str(), "FALSE" | "FALSCH" | "FALSO" | "FAUX") {
                    return Ok(Expr::Bool(false));
                }
                let a = parse_ref(&name).ok_or("#NAME?")?;
                if self.peek() == Some(&Tok::Colon) {
                    self.pos += 1;
                    let b = match self.next() {
                        Some(Tok::Ident(n)) => parse_ref(&n).ok_or("#REF!")?,
                        _ => return Err("#REF!"),
                    };
                    return Ok(Expr::Range(a, b));
                }
                Ok(Expr::Ref(a.0, a.1))
            }
            _ => Err("#VALUE!"),
        }
    }
}

fn parse_formula(s: &str, comma: bool) -> Result<Expr, &'static str> {
    let toks = tokenize(s, comma)?;
    let mut p = Parser { toks, pos: 0 };
    let e = p.compare()?;
    if p.pos != p.toks.len() {
        return Err("#VALUE!");
    }
    Ok(e)
}

// ───────────────────────────── Auswerten ─────────────────────────────

/// Funktionen: englischer Name und die Namen in LibreOffice Calc bzw. Excel auf Deutsch,
/// Spanisch und Französisch.
pub const FUNCTIONS: &[(&str, &[&str])] = &[
    // Rechnen
    ("SUM", &["SUMME", "SUMA", "SOMME"]),
    ("PRODUCT", &["PRODUKT", "PRODUCTO", "PRODUIT"]),
    ("AVERAGE", &["MITTELWERT", "PROMEDIO", "MOYENNE"]),
    ("MIN", &["MÍN"]),
    ("MAX", &["MÁX"]),
    ("COUNT", &["ANZAHL", "CONTAR", "NB"]),
    ("COUNTA", &["ANZAHL2", "CONTARA", "NBVAL"]),
    ("COUNTBLANK", &["ANZAHLLEEREZELLEN", "CONTAR.BLANCO", "NB.VIDE"]),
    ("ABS", &[]),
    ("SQRT", &["WURZEL", "RAIZ", "RAÍZ", "RACINE"]),
    ("POWER", &["POTENZ", "POTENCIA", "PUISSANCE"]),
    ("MOD", &["REST", "RESIDUO"]),
    ("ROUND", &["RUNDEN", "REDONDEAR", "ARRONDI"]),
    (
        "ROUNDUP",
        &["AUFRUNDEN", "REDONDEAR.MAS", "REDONDEAR.MÁS", "ARRONDI.SUP"],
    ),
    ("ROUNDDOWN", &["ABRUNDEN", "REDONDEAR.MENOS", "ARRONDI.INF"]),
    ("INT", &["GANZZAHL", "ENTERO", "ENT"]),
    ("TRUNC", &["KÜRZEN", "TRUNCAR", "TRONQUE"]),
    (
        "CEILING",
        &["OBERGRENZE", "MULTIPLO.SUPERIOR", "MÚLTIPLO.SUPERIOR", "PLAFOND"],
    ),
    (
        "FLOOR",
        &[
            "UNTERGRENZE",
            "MULTIPLO.INFERIOR",
            "MÚLTIPLO.INFERIOR",
            "PLANCHER",
        ],
    ),
    ("SIGN", &["VORZEICHEN", "SIGNO", "SIGNE"]),
    ("PI", &[]),
    ("EXP", &[]),
    ("LN", &[]),
    ("LOG", &[]),
    ("LOG10", &[]),
    ("FACT", &["FAKULTÄT"]),
    ("SUMPRODUCT", &["SUMMENPRODUKT", "SUMAPRODUCTO", "SOMMEPROD"]),
    ("SUMSQ", &["QUADRATESUMME", "SUMA.CUADRADOS", "SOMME.CARRES"]),
    // Statistik
    ("MEDIAN", &["MEDIANA", "MEDIANE"]),
    (
        "STDEV",
        &[
            "STDEV.S",
            "STABW",
            "STABW.S",
            "DESVEST",
            "DESVEST.M",
            "ECARTYPE",
            "ECARTYPE.STANDARD",
        ],
    ),
    (
        "STDEVP",
        &[
            "STDEV.P",
            "STABWN",
            "STABW.N",
            "DESVESTP",
            "DESVEST.P",
            "ECARTYPEP",
            "ECARTYPE.PEARSON",
        ],
    ),
    ("VAR", &["VAR.S", "VARIANZ", "VARIANCE"]),
    ("VARP", &["VAR.P", "VARIANZEN", "VARIANCE.PEARSON"]),
    ("LARGE", &["KGRÖSSTE", "K.ESIMO.MAYOR", "GRANDE.VALEUR"]),
    ("SMALL", &["KKLEINSTE", "K.ESIMO.MENOR", "PETITE.VALEUR"]),
    (
        "RANK",
        &[
            "RANK.EQ",
            "RANG",
            "RANG.GLEICH",
            "JERARQUIA",
            "JERARQUÍA",
            "JERARQUIA.EQV",
            "EQUATION.RANG",
        ],
    ),
    // Bedingungen
    ("SUMIF", &["SUMMEWENN", "SUMAR.SI", "SOMME.SI"]),
    ("COUNTIF", &["ZÄHLENWENN", "CONTAR.SI", "NB.SI"]),
    ("AVERAGEIF", &["MITTELWERTWENN", "PROMEDIO.SI", "MOYENNE.SI"]),
    ("SUMIFS", &["SUMMEWENNS", "SUMAR.SI.CONJUNTO", "SOMME.SI.ENS"]),
    ("COUNTIFS", &["ZÄHLENWENNS", "CONTAR.SI.CONJUNTO", "NB.SI.ENS"]),
    (
        "AVERAGEIFS",
        &["MITTELWERTWENNS", "PROMEDIO.SI.CONJUNTO", "MOYENNE.SI.ENS"],
    ),
    ("MAXIFS", &["MAXWENNS", "MAX.SI.CONJUNTO", "MAX.SI.ENS"]),
    ("MINIFS", &["MINWENNS", "MIN.SI.CONJUNTO", "MIN.SI.ENS"]),
    // Logik
    ("IF", &["WENN", "SI"]),
    ("IFS", &["WENNS", "SI.CONJUNTO", "SI.CONDITIONS"]),
    ("AND", &["UND", "Y", "ET"]),
    ("OR", &["ODER", "O", "OU"]),
    ("NOT", &["NICHT", "NO", "NON"]),
    ("XOR", &["XODER", "XO", "OUX"]),
    ("IFERROR", &["WENNFEHLER", "SI.ERROR", "SIERREUR"]),
    ("IFNA", &["WENNNV", "SI.ND", "SI.NON.DISP"]),
    ("ISBLANK", &["ISTLEER", "ESBLANCO", "ESTVIDE"]),
    ("ISNUMBER", &["ISTZAHL", "ESNUMERO", "ESNÚMERO", "ESTNUM"]),
    ("ISTEXT", &["ISTTEXT", "ESTEXTO", "ESTTEXTE"]),
    ("ISERROR", &["ISTFEHLER", "ESERROR", "ESTERREUR"]),
    ("SWITCH", &["ERSTERWERT", "SCHALTER", "CAMBIAR", "SI.MULTIPLE"]),
    ("CHOOSE", &["WAHL", "ELEGIR", "CHOISIR"]),
    ("TRUE", &["WAHR", "VERDADERO", "VRAI"]),
    ("FALSE", &["FALSCH", "FALSO", "FAUX"]),
    // Text
    ("CONCATENATE", &["VERKETTEN", "CONCATENAR", "CONCATENER"]),
    ("CONCAT", &["TEXTKETTE"]),
    ("TEXTJOIN", &["TEXTVERKETTEN", "UNIRCADENAS", "JOINDRE.TEXTE"]),
    ("LEFT", &["LINKS", "IZQUIERDA", "GAUCHE"]),
    ("RIGHT", &["RECHTS", "DERECHA", "DROITE"]),
    ("MID", &["TEIL", "EXTRAE", "STXT"]),
    ("LEN", &["LÄNGE", "LARGO", "NBCAR"]),
    ("UPPER", &["GROSS", "MAYUSC", "MAJUSCULE"]),
    ("LOWER", &["KLEIN", "MINUSC", "MINUSCULE"]),
    ("PROPER", &["GROSS2", "NOMPROPIO", "NOMPROPRE"]),
    ("TRIM", &["GLÄTTEN", "ESPACIOS", "SUPPRESPACE"]),
    ("VALUE", &["WERT", "VALOR", "CNUM"]),
    ("REPT", &["WIEDERHOLEN", "REPETIR"]),
    ("SUBSTITUTE", &["WECHSELN", "SUSTITUIR", "SUBSTITUE"]),
    ("REPLACE", &["ERSETZEN", "REEMPLAZAR", "REMPLACER"]),
    ("FIND", &["FINDEN", "ENCONTRAR", "TROUVE"]),
    ("SEARCH", &["SUCHEN", "HALLAR", "CHERCHE"]),
    ("EXACT", &["IDENTISCH", "IGUAL"]),
    ("TEXT", &["TEXTO", "TEXTE"]),
    // Nachschlagen
    ("VLOOKUP", &["SVERWEIS", "BUSCARV", "RECHERCHEV"]),
    ("HLOOKUP", &["WVERWEIS", "BUSCARH", "RECHERCHEH"]),
    ("XLOOKUP", &["XVERWEIS", "BUSCARX", "RECHERCHEX"]),
    ("INDEX", &["INDICE", "ÍNDICE"]),
    ("MATCH", &["VERGLEICH", "COINCIDIR", "EQUIV"]),
    ("ROW", &["ZEILE", "FILA", "LIGNE"]),
    ("COLUMN", &["SPALTE", "COLUMNA", "COLONNE"]),
    ("ROWS", &["ZEILEN", "FILAS", "LIGNES"]),
    ("COLUMNS", &["SPALTEN", "COLUMNAS", "COLONNES"]),
    // Datum
    ("TODAY", &["HEUTE", "HOY", "AUJOURDHUI"]),
    ("NOW", &["JETZT", "AHORA", "MAINTENANT"]),
    ("DATE", &["DATUM", "FECHA"]),
    ("YEAR", &["JAHR", "AÑO", "ANNEE", "ANNÉE"]),
    ("MONTH", &["MONAT", "MES", "MOIS"]),
    ("DAY", &["TAG", "DIA", "DÍA", "JOUR"]),
    ("WEEKDAY", &["WOCHENTAG", "DIASEM", "JOURSEM"]),
    ("DAYS", &["TAGE", "DIAS", "DÍAS", "JOURS"]),
    ("TIME", &["ZEIT", "NSHORA", "TEMPS"]),
    ("HOUR", &["STUNDE", "HORA", "HEURE"]),
    ("MINUTE", &["MINUTO"]),
    ("SECOND", &["SEKUNDE", "SEGUNDO", "SECONDE"]),
];

/// Funktionsnamen der Übersetzungen auf die englischen abbilden.
fn canonical(name: &str) -> &str {
    match name {
        "MEAN" | "AVG" => return "AVERAGE",
        _ => {}
    }
    for (en, other) in FUNCTIONS {
        if *en == name || other.contains(&name) {
            return en;
        }
    }
    name
}

/// Passt ein Wert zu einer Bedingung wie in SUMIF/COUNTIF: Zahl (gleich), „>5“, „<>x“,
/// „=a*“ (mit * und ? als Platzhalter), leer (leere Zelle).
fn criterion(v: &Value, crit: &Value, comma: bool) -> bool {
    let text = match crit {
        Value::Num(n) | Value::Date(n) | Value::Time(n) => return v.as_num() == Some(*n),
        Value::Bool(b) => return *v == Value::Bool(*b),
        Value::Empty => return matches!(v, Value::Empty),
        Value::Err(_) => return false,
        Value::Text(t) => t.as_str(),
    };
    let (op, rest) = ["<=", ">=", "<>", "<", ">", "="]
        .iter()
        .find_map(|o| text.strip_prefix(o).map(|r| (*o, r)))
        .unwrap_or(("=", text));
    if let Some(x) = parse_number(rest, comma).or_else(|| parse_date(rest)) {
        let Some(y) = v.as_num() else {
            return op == "<>";
        };
        return match op {
            "<=" => y <= x,
            ">=" => y >= x,
            "<>" => y != x,
            "<" => y < x,
            ">" => y > x,
            _ => y == x,
        };
    }
    let s = v.display(comma).to_lowercase();
    let pat = rest.to_lowercase();
    match op {
        "=" => {
            if pat.is_empty() {
                matches!(v, Value::Empty)
            } else {
                wildcard(&pat, &s)
            }
        }
        "<>" => {
            if pat.is_empty() {
                !matches!(v, Value::Empty)
            } else {
                !wildcard(&pat, &s)
            }
        }
        "<" => s < pat,
        ">" => s > pat,
        "<=" => s <= pat,
        _ => s >= pat,
    }
}

/// Text mit Platzhaltern * (beliebig viel) und ? (ein Zeichen); ~ hebt sie auf.
fn wildcard(pat: &str, s: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = s.chars().collect();
    fn go(p: &[char], t: &[char]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some('~') if p.len() > 1 => t.first() == Some(&p[1]) && go(&p[2..], &t[1..]),
            Some('*') => (0..=t.len()).any(|i| go(&p[1..], &t[i..])),
            Some('?') => !t.is_empty() && go(&p[1..], &t[1..]),
            Some(c) => t.first() == Some(c) && go(&p[1..], &t[1..]),
        }
    }
    go(&p, &t)
}

/// Zahl nach einem Muster wie in TEXT(): „0“, „0.00“, „#,##0.00“, „0%“, „TT.MM.JJJJ“ …
fn text_format(v: &Value, fmt: &str, comma: bool) -> String {
    let lower = fmt.to_lowercase();
    let Some(n) = v.as_num() else {
        return v.display(comma);
    };
    // Datum: d/t/j (Tag), m (Monat), y/j/a (Jahr)
    if lower.chars().any(|c| matches!(c, 'd' | 't' | 'y' | 'j' | 'a')) && lower.contains('m') {
        let (y, m, d) = date_parts(n);
        let mut out = String::new();
        let c: Vec<char> = lower.chars().collect();
        let mut i = 0;
        while i < c.len() {
            let ch = c[i];
            let mut k = 1;
            while i + k < c.len() && c[i + k] == ch {
                k += 1;
            }
            match ch {
                'd' | 't' => out.push_str(&if k >= 2 { format!("{d:02}") } else { d.to_string() }),
                'm' => out.push_str(&if k >= 2 { format!("{m:02}") } else { m.to_string() }),
                'y' | 'j' | 'a' => out.push_str(&if k <= 2 {
                    format!("{:02}", y.rem_euclid(100))
                } else {
                    y.to_string()
                }),
                _ => {
                    for _ in 0..k {
                        out.push(ch);
                    }
                }
            }
            i += k;
        }
        return out;
    }
    let percent = fmt.contains('%');
    let x = if percent { n * 100.0 } else { n };
    // Dezimalstellen nach dem letzten Trennzeichen vor den Nullen
    let dec_sep = if comma { ',' } else { '.' };
    let digits_part = fmt.trim_end_matches('%');
    let decimals = digits_part
        .rfind(dec_sep)
        .map(|i| {
            digits_part[i + 1..]
                .chars()
                .filter(|c| matches!(c, '0' | '#'))
                .count()
        })
        .unwrap_or(0);
    let group = digits_part.contains(if comma { '.' } else { ',' }) && digits_part.contains('#');
    let s = format!("{:.*}", decimals, x.abs());
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    let mut int_s = String::new();
    for (i, ch) in int.chars().enumerate() {
        if group && i > 0 && (int.len() - i) % 3 == 0 {
            int_s.push(if comma { '.' } else { ',' });
        }
        int_s.push(ch);
    }
    let mut out = String::new();
    if x < 0.0 && s.chars().any(|c| c != '0' && c != '.') {
        out.push('-');
    }
    out.push_str(&int_s);
    if decimals > 0 {
        out.push(dec_sep);
        out.push_str(frac);
    }
    if percent {
        out.push('%');
    }
    out
}

struct Eval<'a> {
    cells: &'a [Vec<String>],
    comma: bool,
    done: HashMap<(usize, usize), Value>,
    busy: Vec<(usize, usize)>,
}

impl Eval<'_> {
    fn cell(&mut self, r: usize, c: usize) -> Value {
        if let Some(v) = self.done.get(&(r, c)) {
            return v.clone();
        }
        if self.busy.contains(&(r, c)) {
            return Value::Err("#CYCLE!");
        }
        let input = self
            .cells
            .get(r)
            .and_then(|row| row.get(c))
            .map(|s| s.as_str())
            .unwrap_or("");
        self.busy.push((r, c));
        let v = self.input(input);
        self.busy.pop();
        self.done.insert((r, c), v.clone());
        v
    }

    fn input(&mut self, input: &str) -> Value {
        let t = input.trim();
        if t.is_empty() {
            return Value::Empty;
        }
        if let Some(f) = t.strip_prefix('=') {
            return match parse_formula(f, self.comma) {
                Ok(e) => match self.expr(&e) {
                    Ok(Value::Empty) => Value::Num(0.0),
                    Ok(v) => v,
                    Err(e) => Value::Err(e),
                },
                Err(e) => Value::Err(e),
            };
        }
        if let Some(n) = parse_number(t, self.comma) {
            return Value::Num(n);
        }
        if let Some(d) = parse_date(t) {
            return Value::Date(d);
        }
        if let Some(x) = parse_time(t) {
            return Value::Time(x);
        }
        // Datum mit Uhrzeit
        if let Some((d, h)) = t.trim().split_once(' ') {
            if let (Some(d), Some(h)) = (parse_date(d), parse_time(h)) {
                return Value::Date(d + h);
            }
        }
        Value::Text(input.to_string())
    }

    /// Werte einer Liste von Argumenten (Bereiche aufgelöst).
    fn values(&mut self, args: &[Expr]) -> Result<Vec<Value>, &'static str> {
        let mut out = Vec::new();
        for a in args {
            match a {
                Expr::Range(p, q) => {
                    let (r0, r1) = (p.0.min(q.0), p.0.max(q.0));
                    let (c0, c1) = (p.1.min(q.1), p.1.max(q.1));
                    if (r1 - r0 + 1) * (c1 - c0 + 1) > 100_000 {
                        return Err("#REF!");
                    }
                    for r in r0..=r1 {
                        for c in c0..=c1 {
                            out.push(self.cell(r, c));
                        }
                    }
                }
                e => out.push(self.expr(e)?),
            }
        }
        Ok(out)
    }

    /// Zahlen der Argumente (Text und leere Zellen in Bereichen zählen nicht).
    fn numbers(&mut self, args: &[Expr]) -> Result<Vec<f64>, &'static str> {
        let mut out = Vec::new();
        for v in self.values(args)? {
            match v {
                Value::Num(n) | Value::Date(n) | Value::Time(n) => out.push(n),
                Value::Bool(b) => out.push(if b { 1.0 } else { 0.0 }),
                Value::Err(e) => return Err(e),
                _ => {}
            }
        }
        Ok(out)
    }

    fn expr(&mut self, e: &Expr) -> Result<Value, &'static str> {
        Ok(match e {
            Expr::Num(n) => Value::Num(*n),
            Expr::Str(s) => Value::Text(s.clone()),
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Ref(r, c) => match self.cell(*r, *c) {
                Value::Err(e) => return Err(e),
                v => v,
            },
            Expr::Range(..) => return Err("#VALUE!"),
            Expr::Neg(a) => Value::Num(-self.expr(a)?.num()?),
            Expr::Percent(a) => Value::Num(self.expr(a)?.num()? / 100.0),
            Expr::Bin(op, a, b) => {
                let (va, vb) = (self.expr(a)?, self.expr(b)?);
                match *op {
                    "&" => Value::Text(va.display(self.comma) + &vb.display(self.comma)),
                    "=" | "<>" | "<" | ">" | "<=" | ">=" => {
                        let ord = match (va.num(), vb.num()) {
                            (Ok(x), Ok(y)) => x.partial_cmp(&y),
                            _ => Some(
                                va.display(false)
                                    .to_lowercase()
                                    .cmp(&vb.display(false).to_lowercase()),
                            ),
                        };
                        let Some(o) = ord else { return Err("#VALUE!") };
                        use std::cmp::Ordering::*;
                        Value::Bool(match *op {
                            "=" => o == Equal,
                            "<>" => o != Equal,
                            "<" => o == Less,
                            ">" => o == Greater,
                            "<=" => o != Greater,
                            _ => o != Less,
                        })
                    }
                    _ => {
                        let (x, y) = (va.num()?, vb.num()?);
                        let v = match *op {
                            "+" => x + y,
                            "-" => x - y,
                            "*" => x * y,
                            "/" => {
                                if y == 0.0 {
                                    return Err("#DIV/0!");
                                }
                                x / y
                            }
                            _ => x.powf(y),
                        };
                        if !v.is_finite() {
                            return Err("#NUM!");
                        }
                        Value::Num(v)
                    }
                }
            }
            Expr::Call(name, args) => self.call(canonical(name), args)?,
        })
    }

    /// Werte eines Bereichs zeilenweise (eine Zelle bzw. ein Wert: 1 × 1).
    fn grid_of(&mut self, e: &Expr) -> Result<Vec<Vec<Value>>, &'static str> {
        match e {
            Expr::Range(p, q) => {
                let (r0, r1) = (p.0.min(q.0), p.0.max(q.0));
                let (c0, c1) = (p.1.min(q.1), p.1.max(q.1));
                if (r1 - r0 + 1) * (c1 - c0 + 1) > 100_000 {
                    return Err("#REF!");
                }
                Ok((r0..=r1)
                    .map(|r| (c0..=c1).map(|c| self.cell(r, c)).collect())
                    .collect())
            }
            Expr::Ref(r, c) => Ok(vec![vec![self.cell(*r, *c)]]),
            e => Ok(vec![vec![self.expr(e)?]]),
        }
    }

    /// Alle Werte eines Bereichs der Reihe nach.
    fn flat(&mut self, e: &Expr) -> Result<Vec<Value>, &'static str> {
        Ok(self.grid_of(e)?.into_iter().flatten().collect())
    }

    fn arg(&mut self, args: &[Expr], i: usize) -> Result<Value, &'static str> {
        match args.get(i) {
            Some(e) => self.expr(e),
            None => Err("#VALUE!"),
        }
    }

    fn num_arg(&mut self, args: &[Expr], i: usize, default: Option<f64>) -> Result<f64, &'static str> {
        match args.get(i) {
            Some(e) => self.expr(e)?.num(),
            None => default.ok_or("#VALUE!"),
        }
    }

    fn text_arg(&mut self, args: &[Expr], i: usize) -> Result<String, &'static str> {
        match self.arg(args, i)? {
            Value::Err(e) => Err(e),
            v => Ok(v.display(self.comma)),
        }
    }

    /// Zellen, die alle Bedingungspaare (Bereich; Bedingung) ab `from` erfüllen: Nummern
    /// innerhalb der Bereiche.
    fn criteria_hits(&mut self, args: &[Expr], from: usize) -> Result<Vec<usize>, &'static str> {
        let mut hits: Option<Vec<usize>> = None;
        let mut i = from;
        while i + 1 < args.len() {
            let range = self.flat(&args[i])?;
            let crit = self.expr(&args[i + 1])?;
            let these: Vec<usize> = (0..range.len())
                .filter(|k| criterion(&range[*k], &crit, self.comma))
                .collect();
            hits = Some(match hits {
                None => these,
                Some(h) => h.into_iter().filter(|k| these.contains(k)).collect(),
            });
            i += 2;
        }
        hits.ok_or("#VALUE!")
    }

    fn call(&mut self, name: &str, args: &[Expr]) -> Result<Value, &'static str> {
        let num = Value::Num;
        let check = |x: f64| {
            if x.is_finite() {
                Ok(Value::Num(x))
            } else {
                Err("#NUM!")
            }
        };
        Ok(match name {
            "SUM" => num(self.numbers(args)?.iter().sum()),
            "PRODUCT" => num(self.numbers(args)?.iter().product()),
            "SUMSQ" => num(self.numbers(args)?.iter().map(|x| x * x).sum()),
            "AVERAGE" => {
                let n = self.numbers(args)?;
                if n.is_empty() {
                    return Err("#DIV/0!");
                }
                num(n.iter().sum::<f64>() / n.len() as f64)
            }
            "MIN" => num(self.numbers(args)?.into_iter().reduce(f64::min).unwrap_or(0.0)),
            "MAX" => num(self.numbers(args)?.into_iter().reduce(f64::max).unwrap_or(0.0)),
            "COUNT" => num(self.numbers(args)?.len() as f64),
            "COUNTA" => {
                let v = self.values(args)?;
                num(v.iter().filter(|v| !matches!(v, Value::Empty)).count() as f64)
            }
            "COUNTBLANK" => {
                let v = self.values(args)?;
                num(v
                    .iter()
                    .filter(|v| matches!(v, Value::Empty) || matches!(v, Value::Text(t) if t.is_empty()))
                    .count() as f64)
            }
            "ABS" => num(self.num_arg(args, 0, None)?.abs()),
            "SQRT" => {
                let x = self.num_arg(args, 0, None)?;
                if x < 0.0 {
                    return Err("#NUM!");
                }
                num(x.sqrt())
            }
            "POWER" => check(self.num_arg(args, 0, None)?.powf(self.num_arg(args, 1, None)?))?,
            "MOD" => {
                let (x, y) = (self.num_arg(args, 0, None)?, self.num_arg(args, 1, None)?);
                if y == 0.0 {
                    return Err("#DIV/0!");
                }
                num(x - y * (x / y).floor())
            }
            "ROUND" | "ROUNDUP" | "ROUNDDOWN" => {
                let (x, d) = (self.num_arg(args, 0, None)?, self.num_arg(args, 1, Some(0.0))?);
                let f = 10f64.powi((d.trunc() as i32).clamp(-15, 15));
                let y = x * f;
                // kleine Rechenfehler (2,675 × 100 = 267,49999…) nicht mitrunden
                let y = (y * 1e9).round() / 1e9;
                num(match name {
                    "ROUND" => y.abs().round().copysign(y),
                    "ROUNDUP" => y.abs().ceil().copysign(y),
                    _ => y.trunc(),
                } / f)
            }
            "INT" => num(self.num_arg(args, 0, None)?.floor()),
            "TRUNC" => {
                let (x, d) = (self.num_arg(args, 0, None)?, self.num_arg(args, 1, Some(0.0))?);
                let f = 10f64.powi((d.trunc() as i32).clamp(-15, 15));
                num((x * f).trunc() / f)
            }
            "CEILING" | "FLOOR" => {
                let x = self.num_arg(args, 0, None)?;
                let s = self
                    .num_arg(args, 1, Some(if x < 0.0 { -1.0 } else { 1.0 }))?
                    .abs();
                if s == 0.0 {
                    return Ok(num(0.0));
                }
                num(if name == "CEILING" {
                    (x / s).ceil() * s
                } else {
                    (x / s).floor() * s
                })
            }
            "SIGN" => {
                let x = self.num_arg(args, 0, None)?;
                num(if x > 0.0 {
                    1.0
                } else if x < 0.0 {
                    -1.0
                } else {
                    0.0
                })
            }
            "PI" => num(std::f64::consts::PI),
            "EXP" => check(self.num_arg(args, 0, None)?.exp())?,
            "LN" | "LOG10" | "LOG" => {
                let x = self.num_arg(args, 0, None)?;
                if x <= 0.0 {
                    return Err("#NUM!");
                }
                match name {
                    "LN" => num(x.ln()),
                    "LOG10" => num(x.log10()),
                    _ => {
                        let b = self.num_arg(args, 1, Some(10.0))?;
                        if b <= 0.0 || b == 1.0 {
                            return Err("#NUM!");
                        }
                        num(x.ln() / b.ln())
                    }
                }
            }
            "FACT" => {
                let n = self.num_arg(args, 0, None)?.trunc();
                if !(0.0..=170.0).contains(&n) {
                    return Err("#NUM!");
                }
                num((1..=n as u64).map(|k| k as f64).product())
            }
            "SUMPRODUCT" => {
                let lists: Vec<Vec<Value>> = args.iter().map(|a| self.flat(a)).collect::<Result<_, _>>()?;
                let n = lists.first().map(|l| l.len()).unwrap_or(0);
                if lists.iter().any(|l| l.len() != n) {
                    return Err("#VALUE!");
                }
                num((0..n)
                    .map(|i| {
                        lists
                            .iter()
                            .map(|l| l[i].as_num().unwrap_or(0.0))
                            .product::<f64>()
                    })
                    .sum())
            }
            "MEDIAN" => {
                let mut v = self.numbers(args)?;
                if v.is_empty() {
                    return Err("#NUM!");
                }
                v.sort_by(|a, b| a.total_cmp(b));
                let m = v.len() / 2;
                num(if v.len() % 2 == 1 {
                    v[m]
                } else {
                    (v[m - 1] + v[m]) / 2.0
                })
            }
            "STDEV" | "STDEVP" | "VAR" | "VARP" => {
                let v = self.numbers(args)?;
                let pop = name.ends_with('P');
                let n = v.len() as f64;
                if v.len() < if pop { 1 } else { 2 } {
                    return Err("#DIV/0!");
                }
                let mean = v.iter().sum::<f64>() / n;
                let ss: f64 = v.iter().map(|x| (x - mean).powi(2)).sum();
                let var = ss / if pop { n } else { n - 1.0 };
                num(if name.starts_with("STDEV") {
                    var.sqrt()
                } else {
                    var
                })
            }
            "LARGE" | "SMALL" => {
                let mut v: Vec<f64> = match args.first() {
                    Some(a) => self.numbers(std::slice::from_ref(a))?,
                    None => return Err("#VALUE!"),
                };
                let k = self.num_arg(args, 1, None)?;
                if k < 1.0 || k as usize > v.len() {
                    return Err("#NUM!");
                }
                v.sort_by(|a, b| a.total_cmp(b));
                if name == "LARGE" {
                    v.reverse();
                }
                num(v[k as usize - 1])
            }
            "RANK" => {
                let x = self.num_arg(args, 0, None)?;
                let v: Vec<f64> = match args.get(1) {
                    Some(a) => self.numbers(std::slice::from_ref(a))?,
                    None => return Err("#VALUE!"),
                };
                let asc = self.num_arg(args, 2, Some(0.0))? != 0.0;
                if !v.contains(&x) {
                    return Err("#N/A");
                }
                num(1.0 + v.iter().filter(|y| if asc { **y < x } else { **y > x }).count() as f64)
            }
            "SUMIF" | "AVERAGEIF" | "COUNTIF" => {
                let range = match args.first() {
                    Some(a) => self.flat(a)?,
                    None => return Err("#VALUE!"),
                };
                let crit = self.arg(args, 1)?;
                let target = match args.get(2) {
                    Some(a) if name != "COUNTIF" => self.flat(a)?,
                    _ => range.clone(),
                };
                let hits: Vec<f64> = (0..range.len())
                    .filter(|k| criterion(&range[*k], &crit, self.comma))
                    .filter_map(|k| target.get(k).and_then(|v| v.as_num()).or(Some(f64::NAN)))
                    .collect();
                match name {
                    "COUNTIF" => num(hits.len() as f64),
                    "SUMIF" => num(hits.iter().filter(|x| x.is_finite()).sum()),
                    _ => {
                        let n: Vec<f64> = hits.into_iter().filter(|x| x.is_finite()).collect();
                        if n.is_empty() {
                            return Err("#DIV/0!");
                        }
                        num(n.iter().sum::<f64>() / n.len() as f64)
                    }
                }
            }
            "COUNTIFS" => num(self.criteria_hits(args, 0)?.len() as f64),
            "SUMIFS" | "AVERAGEIFS" | "MAXIFS" | "MINIFS" => {
                let target = match args.first() {
                    Some(a) => self.flat(a)?,
                    None => return Err("#VALUE!"),
                };
                let v: Vec<f64> = self
                    .criteria_hits(args, 1)?
                    .into_iter()
                    .filter_map(|k| target.get(k).and_then(|v| v.as_num()))
                    .collect();
                match name {
                    "SUMIFS" => num(v.iter().sum()),
                    "MAXIFS" => num(v.into_iter().reduce(f64::max).unwrap_or(0.0)),
                    "MINIFS" => num(v.into_iter().reduce(f64::min).unwrap_or(0.0)),
                    _ => {
                        if v.is_empty() {
                            return Err("#DIV/0!");
                        }
                        num(v.iter().sum::<f64>() / v.len() as f64)
                    }
                }
            }
            "IF" => {
                let c = self.arg(args, 0)?.truthy()?;
                match args.get(if c { 1 } else { 2 }) {
                    Some(e) => self.expr(e)?,
                    None => Value::Bool(c),
                }
            }
            "IFS" => {
                let mut i = 0;
                while i + 1 < args.len() {
                    if self.expr(&args[i])?.truthy()? {
                        return self.expr(&args[i + 1]);
                    }
                    i += 2;
                }
                return Err("#N/A");
            }
            "AND" | "OR" | "XOR" => {
                let v = self.values(args)?;
                let mut b = Vec::new();
                for x in v {
                    match x {
                        Value::Empty => {}
                        Value::Text(_) => {}
                        x => b.push(x.truthy()?),
                    }
                }
                if b.is_empty() {
                    return Err("#VALUE!");
                }
                Value::Bool(match name {
                    "AND" => b.iter().all(|x| *x),
                    "OR" => b.iter().any(|x| *x),
                    _ => b.iter().filter(|x| **x).count() % 2 == 1,
                })
            }
            "NOT" => Value::Bool(!self.arg(args, 0)?.truthy()?),
            "TRUE" => Value::Bool(true),
            "FALSE" => Value::Bool(false),
            "IFERROR" | "IFNA" => {
                let v = match args.first() {
                    Some(e) => self.expr(e),
                    None => return Err("#VALUE!"),
                };
                let failed = match &v {
                    Err(e) | Ok(Value::Err(e)) => name == "IFERROR" || *e == "#N/A",
                    _ => false,
                };
                if failed {
                    self.arg(args, 1)?
                } else {
                    v?
                }
            }
            "ISBLANK" => Value::Bool(matches!(
                args.first().map(|e| self.grid_of(e)),
                Some(Ok(g)) if g.len() == 1 && g[0].len() == 1 && g[0][0] == Value::Empty
            )),
            "ISNUMBER" => {
                Value::Bool(matches!(args.first().map(|e| self.expr(e)), Some(Ok(v)) if v.is_number()))
            }
            "ISTEXT" => Value::Bool(matches!(
                args.first().map(|e| self.expr(e)),
                Some(Ok(Value::Text(_)))
            )),
            "ISERROR" => Value::Bool(matches!(
                args.first().map(|e| self.expr(e)),
                Some(Err(_)) | Some(Ok(Value::Err(_)))
            )),
            "SWITCH" => {
                let v = self.arg(args, 0)?;
                let mut i = 1;
                while i + 1 < args.len() {
                    let w = self.expr(&args[i])?;
                    if v.display(false).to_lowercase() == w.display(false).to_lowercase() {
                        return self.expr(&args[i + 1]);
                    }
                    i += 2;
                }
                if i < args.len() {
                    return self.expr(&args[i]);
                }
                return Err("#N/A");
            }
            "CHOOSE" => {
                let k = self.num_arg(args, 0, None)?.trunc() as usize;
                if k == 0 || k >= args.len() {
                    return Err("#VALUE!");
                }
                self.expr(&args[k])?
            }
            "CONCATENATE" | "CONCAT" => {
                let mut s = String::new();
                for a in args {
                    for v in self.flat(a)? {
                        if let Value::Err(e) = v {
                            return Err(e);
                        }
                        s.push_str(&v.display(self.comma));
                    }
                }
                Value::Text(s)
            }
            "TEXTJOIN" => {
                let sep = self.text_arg(args, 0)?;
                let skip = self.arg(args, 1)?.truthy()?;
                let mut parts = Vec::new();
                for a in args.iter().skip(2) {
                    for v in self.flat(a)? {
                        if let Value::Err(e) = v {
                            return Err(e);
                        }
                        let t = v.display(self.comma);
                        if !(skip && t.is_empty()) {
                            parts.push(t);
                        }
                    }
                }
                Value::Text(parts.join(&sep))
            }
            "LEFT" | "RIGHT" => {
                let t: Vec<char> = self.text_arg(args, 0)?.chars().collect();
                let n = (self.num_arg(args, 1, Some(1.0))?.max(0.0) as usize).min(t.len());
                Value::Text(if name == "LEFT" {
                    t[..n].iter().collect()
                } else {
                    t[t.len() - n..].iter().collect()
                })
            }
            "MID" => {
                let t: Vec<char> = self.text_arg(args, 0)?.chars().collect();
                let start = self.num_arg(args, 1, None)?;
                let n = self.num_arg(args, 2, None)?;
                if start < 1.0 || n < 0.0 {
                    return Err("#VALUE!");
                }
                Value::Text(t.iter().skip(start as usize - 1).take(n as usize).collect())
            }
            "LEN" => num(self.text_arg(args, 0)?.chars().count() as f64),
            "UPPER" => Value::Text(self.text_arg(args, 0)?.to_uppercase()),
            "LOWER" => Value::Text(self.text_arg(args, 0)?.to_lowercase()),
            "PROPER" => {
                let mut out = String::new();
                let mut start = true;
                for ch in self.text_arg(args, 0)?.chars() {
                    if ch.is_alphabetic() {
                        if start {
                            out.extend(ch.to_uppercase());
                        } else {
                            out.extend(ch.to_lowercase());
                        }
                        start = false;
                    } else {
                        out.push(ch);
                        start = true;
                    }
                }
                Value::Text(out)
            }
            "TRIM" => Value::Text(
                self.text_arg(args, 0)?
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            "VALUE" => {
                let t = self.text_arg(args, 0)?;
                match parse_number(&t, self.comma).or_else(|| parse_date(&t)) {
                    Some(n) => num(n),
                    None => return Err("#VALUE!"),
                }
            }
            "REPT" => {
                let t = self.text_arg(args, 0)?;
                let n = self.num_arg(args, 1, None)?;
                if n < 0.0 || t.len() as f64 * n > 32_767.0 {
                    return Err("#VALUE!");
                }
                Value::Text(t.repeat(n as usize))
            }
            "SUBSTITUTE" => {
                let t = self.text_arg(args, 0)?;
                let old = self.text_arg(args, 1)?;
                let new = self.text_arg(args, 2)?;
                if old.is_empty() {
                    return Ok(Value::Text(t));
                }
                match args.get(3) {
                    None => Value::Text(t.replace(&old, &new)),
                    Some(_) => {
                        let k = self.num_arg(args, 3, None)? as usize;
                        match t.match_indices(&old).nth(k.saturating_sub(1)).filter(|_| k >= 1) {
                            Some((i, _)) => Value::Text(format!("{}{}{}", &t[..i], new, &t[i + old.len()..])),
                            None => Value::Text(t),
                        }
                    }
                }
            }
            "REPLACE" => {
                let t: Vec<char> = self.text_arg(args, 0)?.chars().collect();
                let start = self.num_arg(args, 1, None)?;
                let n = self.num_arg(args, 2, None)?.max(0.0) as usize;
                let new = self.text_arg(args, 3)?;
                if start < 1.0 {
                    return Err("#VALUE!");
                }
                let a = (start as usize - 1).min(t.len());
                let b = (a + n).min(t.len());
                Value::Text(t[..a].iter().collect::<String>() + &new + &t[b..].iter().collect::<String>())
            }
            "FIND" | "SEARCH" => {
                let needle = self.text_arg(args, 0)?;
                let hay = self.text_arg(args, 1)?;
                let start = self.num_arg(args, 2, Some(1.0))?;
                if start < 1.0 {
                    return Err("#VALUE!");
                }
                let hay_c: Vec<char> = hay.chars().collect();
                let from = start as usize - 1;
                if from > hay_c.len() {
                    return Err("#VALUE!");
                }
                let found = if name == "FIND" {
                    let rest: String = hay_c[from..].iter().collect();
                    rest.find(&needle).map(|i| rest[..i].chars().count())
                } else {
                    let pat = format!("{}*", needle.to_lowercase());
                    (from..=hay_c.len())
                        .find(|i| wildcard(&pat, &hay_c[*i..].iter().collect::<String>().to_lowercase()))
                        .map(|i| i - from)
                };
                match found {
                    Some(i) => num((from + i + 1) as f64),
                    None => return Err("#VALUE!"),
                }
            }
            "EXACT" => Value::Bool(self.text_arg(args, 0)? == self.text_arg(args, 1)?),
            "TEXT" => {
                let v = self.arg(args, 0)?;
                let f = self.text_arg(args, 1)?;
                Value::Text(text_format(&v, &f, self.comma))
            }
            "VLOOKUP" | "HLOOKUP" => {
                let key = self.arg(args, 0)?;
                let mut g = match args.get(1) {
                    Some(a) => self.grid_of(a)?,
                    None => return Err("#VALUE!"),
                };
                if name == "HLOOKUP" {
                    // Zeilen und Spalten tauschen
                    let w = g.first().map(|r| r.len()).unwrap_or(0);
                    g = (0..w).map(|c| g.iter().map(|r| r[c].clone()).collect()).collect();
                }
                let idx = self.num_arg(args, 2, None)? as usize;
                let approx = match args.get(3) {
                    Some(e) => self.expr(e)?.truthy()?,
                    None => true,
                };
                if idx < 1 || g.first().is_some_and(|r| idx > r.len()) {
                    return Err("#REF!");
                }
                let keys: Vec<Value> = g.iter().map(|r| r[0].clone()).collect();
                let row = lookup(&keys, &key, if approx { 1 } else { 0 }, self.comma).ok_or("#N/A")?;
                g[row][idx - 1].clone()
            }
            "MATCH" => {
                let key = self.arg(args, 0)?;
                let list = match args.get(1) {
                    Some(a) => self.flat(a)?,
                    None => return Err("#VALUE!"),
                };
                let mode = self.num_arg(args, 2, Some(1.0))? as i32;
                num((lookup(&list, &key, mode, self.comma).ok_or("#N/A")? + 1) as f64)
            }
            "XLOOKUP" => {
                let key = self.arg(args, 0)?;
                let keys = match args.get(1) {
                    Some(a) => self.flat(a)?,
                    None => return Err("#VALUE!"),
                };
                let vals = match args.get(2) {
                    Some(a) => self.flat(a)?,
                    None => return Err("#VALUE!"),
                };
                match lookup(&keys, &key, 0, self.comma) {
                    Some(i) => vals.get(i).cloned().ok_or("#VALUE!")?,
                    None => match args.get(3) {
                        Some(e) => self.expr(e)?,
                        None => return Err("#N/A"),
                    },
                }
            }
            "INDEX" => {
                let g = match args.first() {
                    Some(a) => self.grid_of(a)?,
                    None => return Err("#VALUE!"),
                };
                let r = self.num_arg(args, 1, Some(1.0))? as usize;
                let c = self.num_arg(args, 2, Some(1.0))? as usize;
                // eine Zeile: die Zahl meint die Spalte
                let (r, c) = if g.len() == 1 && args.len() == 2 {
                    (1, r)
                } else {
                    (r, c)
                };
                g.get(r.max(1) - 1)
                    .and_then(|row| row.get(c.max(1) - 1))
                    .cloned()
                    .ok_or("#REF!")?
            }
            "ROW" | "COLUMN" | "ROWS" | "COLUMNS" => {
                let (r0, c0, r1, c1) = match args.first() {
                    Some(Expr::Ref(r, c)) => (*r, *c, *r, *c),
                    Some(Expr::Range(p, q)) => (p.0.min(q.0), p.1.min(q.1), p.0.max(q.0), p.1.max(q.1)),
                    None => {
                        let (r, c) = self.busy.last().copied().ok_or("#VALUE!")?;
                        (r, c, r, c)
                    }
                    _ => return Err("#VALUE!"),
                };
                num(match name {
                    "ROW" => r0 + 1,
                    "COLUMN" => c0 + 1,
                    "ROWS" => r1 - r0 + 1,
                    _ => c1 - c0 + 1,
                } as f64)
            }
            "TODAY" | "NOW" => {
                let secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                let days = secs / 86_400.0 + EPOCH as f64;
                if name == "TODAY" {
                    Value::Date(days.floor())
                } else {
                    num(days)
                }
            }
            "DATE" => {
                let y = self.num_arg(args, 0, None)?.trunc() as i64;
                let m = self.num_arg(args, 1, None)?.trunc() as i64;
                let d = self.num_arg(args, 2, None)?.trunc() as i64;
                let y = if (0..1900).contains(&y) { y + 1900 } else { y };
                Value::Date(date_serial(y, m, 1) + (d - 1) as f64)
            }
            "YEAR" | "MONTH" | "DAY" => {
                let (y, m, d) = date_parts(self.num_arg(args, 0, None)?);
                num(match name {
                    "YEAR" => y as f64,
                    "MONTH" => m as f64,
                    _ => d as f64,
                })
            }
            "WEEKDAY" => {
                let n = self.num_arg(args, 0, None)?.floor() as i64;
                let kind = self.num_arg(args, 1, Some(1.0))? as i64;
                // 30.12.1899 war ein Samstag: Sonntag = 1
                let sun1 = (n + 6).rem_euclid(7) + 1;
                num(match kind {
                    2 => ((sun1 + 5) % 7 + 1) as f64,
                    3 => ((sun1 + 5) % 7) as f64,
                    _ => sun1 as f64,
                })
            }
            "TIME" => {
                let h = self.num_arg(args, 0, None)?;
                let m = self.num_arg(args, 1, Some(0.0))?;
                let sec = self.num_arg(args, 2, Some(0.0))?;
                Value::Time(((h * 3600.0 + m * 60.0 + sec) / 86_400.0).rem_euclid(1.0))
            }
            "HOUR" | "MINUTE" | "SECOND" => {
                let t = (self.num_arg(args, 0, None)?.rem_euclid(1.0) * 86_400.0).round() as i64;
                num(match name {
                    "HOUR" => t / 3600 % 24,
                    "MINUTE" => t / 60 % 60,
                    _ => t % 60,
                } as f64)
            }
            "DAYS" => {
                num((self.num_arg(args, 0, None)?.floor() - self.num_arg(args, 1, None)?.floor()).trunc())
            }
            _ => return Err("#NAME?"),
        })
    }
}

/// Stelle von `key` in `list`: 0 = genau gleich (Text ohne Groß/Klein, mit Platzhaltern),
/// 1 = größter Wert ≤ key (aufsteigend sortiert), −1 = kleinster Wert ≥ key.
fn lookup(list: &[Value], key: &Value, mode: i32, comma: bool) -> Option<usize> {
    let same = |v: &Value| match (v.as_num(), key.as_num()) {
        (Some(a), Some(b)) => a == b,
        _ => match key {
            Value::Text(t) => wildcard(&t.to_lowercase(), &v.display(comma).to_lowercase()),
            k => v.display(comma).to_lowercase() == k.display(comma).to_lowercase(),
        },
    };
    let cmp = |v: &Value| -> Option<std::cmp::Ordering> {
        match (v.as_num(), key.as_num()) {
            (Some(a), Some(b)) => a.partial_cmp(&b),
            (None, None) => Some(
                v.display(comma)
                    .to_lowercase()
                    .cmp(&key.display(comma).to_lowercase()),
            ),
            _ => None,
        }
    };
    use std::cmp::Ordering::*;
    match mode {
        0 => list.iter().position(same),
        m if m > 0 => {
            let mut best = None;
            for (i, v) in list.iter().enumerate() {
                match cmp(v) {
                    Some(Less) | Some(Equal) => best = Some(i),
                    Some(Greater) => break,
                    None => {}
                }
            }
            best
        }
        _ => {
            let mut best = None;
            for (i, v) in list.iter().enumerate() {
                match cmp(v) {
                    Some(Greater) | Some(Equal) => best = Some(i),
                    Some(Less) => break,
                    None => {}
                }
            }
            best
        }
    }
}

/// Alle Zellen auswerten (gleiche Form wie die Eingaben).
pub fn evaluate(cells: &[Vec<String>], decimal_comma: bool) -> Vec<Vec<Value>> {
    let mut ev = Eval {
        cells,
        comma: decimal_comma,
        done: HashMap::new(),
        busy: Vec::new(),
    };
    cells
        .iter()
        .enumerate()
        .map(|(r, row)| (0..row.len()).map(|c| ev.cell(r, c)).collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter()
            .map(|r| r.iter().map(|s| s.to_string()).collect())
            .collect()
    }

    fn shown(cells: &[Vec<String>], comma: bool) -> Vec<Vec<String>> {
        evaluate(cells, comma)
            .iter()
            .map(|r| r.iter().map(|v| v.display(comma)).collect())
            .collect()
    }

    #[test]
    fn numbers_and_names() {
        assert_eq!(parse_number("3,5", true), Some(3.5));
        assert_eq!(parse_number("3,5", false), Some(35.0));
        assert_eq!(parse_number("1.234,5", true), Some(1234.5));
        assert_eq!(parse_number("1,234.5", false), Some(1234.5));
        assert_eq!(parse_number("25 %", true), Some(0.25));
        assert_eq!(parse_number("Peter", true), None);
        assert_eq!(format_number(0.1 + 0.2, false), "0.3");
        assert_eq!(format_number(2.5, true), "2,5");
        assert_eq!(format_number(1e6, false), "1000000");
        assert_eq!(col_name(0), "A");
        assert_eq!(col_name(26), "AA");
        assert_eq!(parse_ref("$B$3"), Some((2, 1)));
        assert_eq!(parse_ref("AA10"), Some((9, 26)));
        assert_eq!(parse_ref("A0"), None);
    }

    #[test]
    fn formulas() {
        let g = grid(&[
            &["Miete", "500"],
            &["Essen", "250,5"],
            &["Summe", "=SUMME(B1:B2)"],
            &["Schnitt", "=AVERAGE(B1;B2)"],
            &["Anteil", "=B1/B3*100%"],
            &["Text", "=A1&\" \"&B1"],
            &["Wenn", "=WENN(B3>700;\"viel\";\"wenig\")"],
            &["Runden", "=RUNDEN(B2*1,333;1)"],
            &["Potenz", "=-2^2+2^3^2"],
        ]);
        let v = shown(&g, true);
        assert_eq!(v[2][1], "750,5");
        assert_eq!(v[3][1], "375,25");
        assert_eq!(v[4][1], "0,6662225183");
        assert_eq!(v[5][1], "Miete 500");
        assert_eq!(v[6][1], "viel");
        assert_eq!(v[7][1], "333,9");
        // (−2)² + (2³)²
        assert_eq!(v[8][1], "68");
        // englische Schreibweise
        let e = shown(
            &grid(&[&["1.5", "2", "=SUM(A1,B1)*2", "=MAX(A1:B1)", "=COUNT(A1:D1)"]]),
            false,
        );
        assert_eq!(e[0][2], "7");
        assert_eq!(e[0][3], "2");
        assert_eq!(e[0][4], "4");
    }

    #[test]
    fn spreadsheet_functions() {
        let g = grid(&[
            &["Name", "Team", "Points", "Date"],
            &["Anna", "red", "12", "2026-11-05"],
            &["Ben", "blue", "7", "2026-02-28"],
            &["Cleo", "red", "20", ""],
            &["Dan", "green", "3", ""],
        ]);
        let f = |formula: &str| {
            let mut g2 = g.clone();
            g2.push(vec![formula.to_string()]);
            let v = shown(&g2, false);
            v[5][0].clone()
        };
        assert_eq!(f("=SUMIF(B2:B5,\"red\",C2:C5)"), "32");
        assert_eq!(f("=COUNTIF(C2:C5,\">5\")"), "3");
        assert_eq!(f("=AVERAGEIF(B2:B5,\"<>red\",C2:C5)"), "5");
        assert_eq!(f("=SUMIFS(C2:C5,B2:B5,\"r*\",C2:C5,\">15\")"), "20");
        assert_eq!(f("=COUNTIFS(B2:B5,\"red\",C2:C5,\"<15\")"), "1");
        assert_eq!(f("=VLOOKUP(\"Cleo\",A2:C5,3,FALSE)"), "20");
        assert_eq!(f("=INDEX(A2:C5,MATCH(7,C2:C5,0),1)"), "Ben");
        assert_eq!(f("=XLOOKUP(\"Zoe\",A2:A5,C2:C5,\"none\")"), "none");
        assert_eq!(f("=IFERROR(1/0,\"oops\")"), "oops");
        assert_eq!(f("=IF(AND(C2>10,OR(B2=\"red\",B2=\"x\")),\"yes\",\"no\")"), "yes");
        assert_eq!(f("=IFS(C3>10,\"high\",C3>5,\"mid\",TRUE,\"low\")"), "mid");
        assert_eq!(f("=SWITCH(B5,\"red\",1,\"green\",2,0)"), "2");
        assert_eq!(f("=CHOOSE(2,\"a\",\"b\",\"c\")"), "b");
        assert_eq!(
            f("=CONCAT(LEFT(A2,2),\"-\",UPPER(RIGHT(A3,2)),MID(A4,2,2))"),
            "An-ENle"
        );
        assert_eq!(f("=TEXTJOIN(\", \",TRUE,A2:A5)"), "Anna, Ben, Cleo, Dan");
        assert_eq!(f("=LEN(TRIM(\"  a   b \"))"), "3");
        assert_eq!(f("=PROPER(\"hello wORLD\")"), "Hello World");
        assert_eq!(f("=SUBSTITUTE(\"a-b-c\",\"-\",\"+\",2)"), "a-b+c");
        assert_eq!(f("=FIND(\"b\",\"abcb\",3)"), "4");
        assert_eq!(f("=SEARCH(\"C?\",\"abcd\")"), "3");
        assert_eq!(f("=ROUNDUP(2.341,1)+ROUNDDOWN(-2.36,1)"), "0.1");
        assert_eq!(f("=ROUND(2.675,2)"), "2.68");
        assert_eq!(f("=MOD(-7,3)"), "2");
        assert_eq!(f("=MEDIAN(C2:C5)"), "9.5");
        assert_eq!(f("=LARGE(C2:C5,2)+SMALL(C2:C5,1)"), "15");
        assert_eq!(f("=RANK(12,C2:C5)"), "2");
        assert_eq!(f("=ROUND(STDEV(C2:C5),4)"), "7.3258");
        assert_eq!(f("=SUMPRODUCT(C2:C3,C4:C5)"), "261");
        assert_eq!(f("=CEILING(7,5)+FLOOR(7,5)+INT(-1.5)+TRUNC(-1.5)"), "12");
        assert_eq!(f("=YEAR(D2)*100+MONTH(D2)+DAY(D3)"), "202639");
        assert_eq!(f("=DAYS(D2,D3)"), "250");
        assert_eq!(f("=WEEKDAY(D2)"), "5");
        assert_eq!(f("=DATE(2026,13,1)"), "01/01/2027");
        assert_eq!(f("=TEXT(1234.5,\"#,##0.00\")"), "1,234.50");
        assert_eq!(f("=TEXT(D2,\"yyyy-mm-dd\")"), "2026-11-05");
        assert_eq!(f("=TEXT(0.256,\"0.0%\")"), "25.6%");
        assert_eq!(f("=COUNTA(A1:D5)+COUNTBLANK(D2:D5)"), "20");
        assert_eq!(f("=ROWS(A1:B3)*10+COLUMNS(A1:B3)"), "32");
        assert_eq!(
            f("=ISNUMBER(C2)&ISTEXT(A2)&ISBLANK(D4)&ISERROR(1/0)"),
            "TRUETRUETRUETRUE"
        );
        assert_eq!(f("=NOPE(1)"), "#NAME?");
        // deutsche, spanische und französische Namen
        let de = |formula: &str| {
            let mut g2 = g.clone();
            g2.push(vec![formula.to_string()]);
            shown(&g2, true)[5][0].clone()
        };
        assert_eq!(de("=SUMMEWENN(B2:B5;\"red\";C2:C5)"), "32");
        assert_eq!(de("=SVERWEIS(\"Ben\";A2:C5;3;FALSCH)"), "7");
        assert_eq!(de("=ZÄHLENWENN(C2:C5;\">=12\")"), "2");
        assert_eq!(de("=WENNFEHLER(VERGLEICH(\"x\";A2:A5;0);LÄNGE(\"abc\"))"), "3");
        assert_eq!(
            de("=BUSCARV(\"Ben\";A2:C5;3;FALSO)+SOMME.SI(B2:B5;\"red\";C2:C5)"),
            "39"
        );
        assert_eq!(de("=RUNDEN(MITTELWERT(C2:C5);1)"), "10,5");
    }

    #[test]
    fn functions_are_suggested() {
        assert_eq!(suggest_function("=su", 3), Some(("M".into(), "SUM".into())));
        assert_eq!(
            suggest_function("=A1+vlo", 7),
            Some(("OKUP".into(), "VLOOKUP".into()))
        );
        assert_eq!(suggest_function("=SUMME", 6), Some(("".into(), "SUMME".into())));
        assert_eq!(
            suggest_function("=mittel", 7).map(|x| x.1),
            Some("MITTELWERT".into())
        );
        assert_eq!(suggest_function("=\"su", 4), None);
        assert_eq!(suggest_function("su", 2), None);
        assert_eq!(suggest_function("=A1", 3), None);
        assert_eq!(suggest_function("=su(", 3), None);
    }

    #[test]
    fn times_and_dates_with_times() {
        let v = shown(&grid(&[&["14:30", "2026-11-05 08:15", "=A1+1/24"]]), false);
        assert_eq!(v[0][0], "14:30");
        assert_eq!(v[0][1], "05/11/2026 08:15");
        assert_eq!(v[0][2], "0.6458333333");
        let v = shown(
            &grid(&[&[
                "=TIME(9;5;0)",
                "=HOUR(B2)*100+MINUTE(\"14:30\")",
                "=STUNDE(\"08:15\")",
            ]]),
            true,
        );
        assert_eq!(v[0][0], "09:05");
        assert_eq!(v[0][2], "8");
    }

    #[test]
    fn references_are_found_for_colouring() {
        let f = "=SUM(B2:C3)+b2*$A$1+B2:C3&\"A1\"+SUMME(D4)";
        let r = formula_refs(f);
        let spans: Vec<&str> = r.iter().map(|x| &f[x.0..x.1]).collect();
        assert_eq!(spans, ["B2:C3", "b2", "$A$1", "B2:C3", "D4"]);
        assert_eq!(r[0].2, (1, 1, 2, 2));
        assert_eq!(r[0].3, r[3].3);
        assert_ne!(r[0].3, r[1].3);
        assert!(formula_refs("SUM(A1)").is_empty());
        assert!(wants_ref("=SUM(", 5));
        assert!(wants_ref("=A1+ ", 5));
        assert!(!wants_ref("=A1", 3));
        assert!(!wants_ref("A+", 2));
    }

    #[test]
    fn references_move_along() {
        assert_eq!(
            shift_formula("=SUM(B2:B4)*$C$1+A$2", 1, 0),
            "=SUM(B3:B5)*$C$1+A$2"
        );
        assert_eq!(shift_formula("=B2&\"B2\"", 0, 2), "=D2&\"B2\"");
        assert_eq!(shift_formula("=A1+ROUND(B1;2)", -1, 0), "=#REF!+ROUND(#REF!;2)");
        assert_eq!(shift_formula("=SUMME(A1)", 0, 1), "=SUMME(B1)");
    }

    #[test]
    fn errors() {
        let g = grid(&[&["=B1", "=A1", "=1/0", "=FOO(1)", "=A1+\"x\"", "=(1", "=Z999999"]]);
        let v = shown(&g, false);
        assert_eq!(v[0][0], "#CYCLE!");
        assert_eq!(v[0][1], "#CYCLE!");
        assert_eq!(v[0][2], "#DIV/0!");
        assert_eq!(v[0][3], "#NAME?");
        assert_eq!(v[0][4], "#CYCLE!");
        assert_eq!(v[0][5], "#VALUE!");
        assert_eq!(v[0][6], "#NAME?");
        // Text, der wie eine Zahl aussieht, rechnet mit; leere Zellen zählen als 0
        let w = shown(&grid(&[&["", "=A1+1", "abc", "=C1&\"!\""]]), false);
        assert_eq!(w[0][1], "1");
        assert_eq!(w[0][3], "abc!");
    }
}
