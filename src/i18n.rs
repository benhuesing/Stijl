// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! Übersetzungen. Ausgangssprache des Programms ist Deutsch; die englische Fassung
//! (britisches Englisch) steht in `po/en_GB.po` und wird ins Programm eingebaut.
//!
//! `tr("Text")` liefert den Text in der gewählten Sprache, `trf("{} Dateien", &[&n])` setzt
//! zusätzlich Werte ein (`{}` der Reihe nach, in Übersetzungen auch `{0}`, `{1}` …).
//! Fehlt eine Übersetzung, bleibt der deutsche Text stehen.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Sprache der Oberfläche.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    De,
    EnGb,
}

static LANG: OnceLock<Lang> = OnceLock::new();
static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();

const EN_GB: &str = include_str!("../po/en_GB.po");

/// Sprache festlegen (einmal beim Start, vor dem Aufbau der Oberfläche).
pub fn init(lang: Lang) {
    let _ = LANG.set(lang);
    if lang == Lang::EnGb {
        let _ = TABLE.set(parse_po(EN_GB));
    }
}

/// Sprache aus der Einstellung (`system`, `de`, `en_GB`) und den Sprachen des Systems
/// (z. B. `["de_DE.UTF-8", "de_DE", "de", "C"]`): Deutsch für deutschsprachige Systeme,
/// sonst (auch ohne Angabe) britisches Englisch.
pub fn choose(setting: &str, system: &[String]) -> Lang {
    match setting {
        "de" => Lang::De,
        "en_GB" | "en" => Lang::EnGb,
        _ => {
            let first = system
                .iter()
                .map(|s| s.as_str())
                .find(|s| !s.is_empty() && *s != "C" && *s != "POSIX");
            match first {
                Some(s) if s.starts_with("de") => Lang::De,
                // andere Sprachen und nur „C“ (ohne Angabe): Englisch
                _ => Lang::EnGb,
            }
        }
    }
}

/// Text in der gewählten Sprache.
pub fn tr(s: &str) -> &str {
    match TABLE.get().and_then(|t| t.get(s)) {
        Some(t) => t.as_str(),
        None => s,
    }
}

/// Text mit eingesetzten Werten: `{}` der Reihe nach, `{0}`, `{1}` … nach Nummer;
/// `{{` und `}}` stehen für geschweifte Klammern.
pub fn trf(fmt: &str, args: &[&dyn std::fmt::Display]) -> String {
    let f = tr(fmt);
    let mut out = String::with_capacity(f.len() + 16);
    let mut next = 0;
    let mut chars = f.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut inner = String::new();
                for d in chars.by_ref() {
                    if d == '}' {
                        break;
                    }
                    inner.push(d);
                }
                let i = if inner.is_empty() {
                    next += 1;
                    next - 1
                } else {
                    inner.parse().unwrap_or(usize::MAX)
                };
                if let Some(a) = args.get(i) {
                    out.push_str(&a.to_string());
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// Einfache .po-Datei lesen (msgid/msgstr, auch über mehrere Zeilen).
fn parse_po(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let (mut id, mut st) = (String::new(), String::new());
    // 0 = nichts, 1 = in msgid, 2 = in msgstr
    let mut state = 0;
    let flush = |id: &mut String, st: &mut String, map: &mut HashMap<String, String>| {
        if !id.is_empty() && !st.is_empty() {
            map.insert(std::mem::take(id), std::mem::take(st));
        }
        id.clear();
        st.clear();
    };
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("msgid ") {
            flush(&mut id, &mut st, &mut map);
            id = unquote(rest);
            state = 1;
        } else if let Some(rest) = line.strip_prefix("msgstr ") {
            st = unquote(rest);
            state = 2;
        } else if line.starts_with('"') {
            match state {
                1 => id.push_str(&unquote(line)),
                2 => st.push_str(&unquote(line)),
                _ => {}
            }
        } else if line.is_empty() || line.starts_with('#') {
            if state == 2 {
                flush(&mut id, &mut st, &mut map);
                state = 0;
            }
        }
    }
    flush(&mut id, &mut st, &mut map);
    map
}

/// `"…"` einer .po-Zeile ohne Anführungszeichen, mit aufgelösten Escapes.
fn unquote(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix('"').unwrap_or(s);
    let s = s.strip_suffix('"').unwrap_or(s);
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(o) => {
                    out.push('\\');
                    out.push(o);
                }
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_choice() {
        let sys = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            choose("system", &sys(&["de_AT.UTF-8", "de_AT", "de", "C"])),
            Lang::De
        );
        assert_eq!(choose("system", &sys(&["en_US.UTF-8", "en", "C"])), Lang::EnGb);
        assert_eq!(choose("system", &sys(&["fr_FR", "fr", "C"])), Lang::EnGb);
        assert_eq!(choose("system", &sys(&["C"])), Lang::EnGb);
        assert_eq!(choose("en_GB", &sys(&["de", "C"])), Lang::EnGb);
        assert_eq!(choose("de", &sys(&["en", "C"])), Lang::De);
    }

    #[test]
    fn po_and_format() {
        let m = parse_po(
            "# Kommentar\nmsgid \"\"\nmsgstr \"\"\n\"Content-Type: text/plain\\n\"\n\nmsgid \"Seite {} von {}\"\nmsgstr \"Page {} of {}\"\n\nmsgid \"Zwei\\nZeilen\"\nmsgstr \"\"\n\"Two\\n\"\n\"lines\"\n",
        );
        assert_eq!(
            m.get("Seite {} von {}").map(|s| s.as_str()),
            Some("Page {} of {}")
        );
        assert_eq!(m.get("Zwei\nZeilen").map(|s| s.as_str()), Some("Two\nlines"));
        assert!(!m.contains_key(""));
        // ohne init: Deutsch, Werte der Reihe nach
        assert_eq!(trf("Seite {} von {}", &[&2, &5]), "Seite 2 von 5");
        assert_eq!(trf("{{x}} {1}{0}", &[&"a", &"b"]), "{x} ba");
        // alle Übersetzungen der Datei lassen sich lesen und haben gleich viele Platzhalter
        let en = parse_po(EN_GB);
        assert!(en.len() > 500, "{}", en.len());
        let count = |s: &str| s.replace("{{", "").matches('{').count();
        for (k, v) in &en {
            assert_eq!(count(k), count(v), "{k:?} → {v:?}");
        }
    }
}
