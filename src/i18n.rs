// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! Translations. The program's source language is (British) English; the translations
//! are in `po/de.po`, `po/es.po` and `po/fr.po` and are built into the program
//! (`po/stijl.pot` is the template for new languages).
//!
//! `tr("Text")` returns the text in the chosen language, `trf("{} files", &[&n])` also
//! inserts values (`{}` in order, in translations also `{0}`, `{1}` …). If a translation
//! is missing, the English text is shown.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Language of the user interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    De,
    Es,
    Fr,
}

impl Lang {
    /// All languages with their setting name and their own name (shown in that language).
    pub const ALL: [(Lang, &'static str, &'static str); 4] = [
        (Lang::En, "en", "English"),
        (Lang::De, "de", "Deutsch"),
        (Lang::Es, "es", "Español"),
        (Lang::Fr, "fr", "Français"),
    ];

    /// Value for the `LANGUAGE` environment variable (GTK's own texts).
    pub fn env(self) -> &'static str {
        match self {
            Lang::En => "en_GB:en",
            Lang::De => "de_DE:de",
            Lang::Es => "es_ES:es",
            Lang::Fr => "fr_FR:fr",
        }
    }

    fn po(self) -> Option<&'static str> {
        match self {
            Lang::En => None,
            Lang::De => Some(include_str!("../po/de.po")),
            Lang::Es => Some(include_str!("../po/es.po")),
            Lang::Fr => Some(include_str!("../po/fr.po")),
        }
    }
}

static LANG: OnceLock<Lang> = OnceLock::new();
static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Set the language (once at start-up, before the user interface is built).
pub fn init(lang: Lang) {
    let _ = LANG.set(lang);
    if let Some(po) = lang.po() {
        let _ = TABLE.set(parse_po(po));
    }
}

/// Language from the setting (`system`, `en`, `de`, `es`, `fr`; earlier versions also
/// stored `en_GB`) and the system languages (e.g. `["de_DE.UTF-8", "de_DE", "de", "C"]`):
/// the first system language that is available, otherwise English.
pub fn choose(setting: &str, system: &[String]) -> Lang {
    let pick = |code: &str| {
        Lang::ALL
            .iter()
            .find(|(_, c, _)| {
                code == *c || code.starts_with(&format!("{c}_")) || code.starts_with(&format!("{c}."))
            })
            .map(|(l, _, _)| *l)
    };
    if setting == "en_GB" {
        return Lang::En;
    }
    if let Some(l) = pick(setting) {
        return l;
    }
    system
        .iter()
        .map(|s| s.as_str())
        .filter(|s| !s.is_empty() && *s != "C" && *s != "POSIX")
        .find_map(pick)
        .unwrap_or(Lang::En)
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
        } else if (line.is_empty() || line.starts_with('#')) && state == 2 {
            flush(&mut id, &mut st, &mut map);
            state = 0;
        }
    }
    flush(&mut id, &mut st, &mut map);
    map
}

/// Alle msgids einer .po/.pot-Datei (auch ohne Übersetzung), ohne den Kopf.
#[cfg(test)]
fn parse_po_all(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut id: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("msgid ") {
            if let Some(i) = id.take() {
                ids.push(i);
            }
            id = Some(unquote(rest));
        } else if line.starts_with('"') {
            if let Some(i) = id.as_mut() {
                i.push_str(&unquote(line));
            }
        } else if line.starts_with("msgstr") {
            if let Some(i) = id.take() {
                ids.push(i);
            }
        }
    }
    ids.retain(|i| !i.is_empty());
    ids
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
        assert_eq!(choose("system", &sys(&["en_US.UTF-8", "en", "C"])), Lang::En);
        assert_eq!(choose("system", &sys(&["fr_FR", "fr", "C"])), Lang::Fr);
        assert_eq!(choose("system", &sys(&["es_MX.UTF-8", "es", "C"])), Lang::Es);
        // nicht vorhanden (Italienisch), dann die nächste Sprache der Liste
        assert_eq!(choose("system", &sys(&["it_IT", "fr", "C"])), Lang::Fr);
        assert_eq!(choose("system", &sys(&["it_IT", "C"])), Lang::En);
        assert_eq!(choose("system", &sys(&["C"])), Lang::En);
        assert_eq!(choose("en_GB", &sys(&["de", "C"])), Lang::En);
        assert_eq!(choose("en", &sys(&["de", "C"])), Lang::En);
        assert_eq!(choose("de", &sys(&["en", "C"])), Lang::De);
        assert_eq!(choose("es", &sys(&["en", "C"])), Lang::Es);
        // „deutsch“ o. ä. ist keine Sprachkennung
        assert_eq!(choose("system", &sys(&["dev", "C"])), Lang::En);
    }

    #[test]
    fn translations_are_complete() {
        // jede Übersetzung hat dieselben Platzhalter und Zeilenumbrüche wie das Englische
        let pot = parse_po_all(include_str!("../po/stijl.pot"));
        for lang in [Lang::De, Lang::Es, Lang::Fr] {
            let m = parse_po(lang.po().unwrap());
            for id in &pot {
                let t = m.get(id).unwrap_or_else(|| panic!("{lang:?}: {id:?} fehlt"));
                let ph = |s: &str| {
                    let mut v: Vec<String> = s
                        .match_indices('{')
                        .map(|(i, _)| s[i..].chars().take_while(|c| *c != '}').collect())
                        .collect();
                    v.sort();
                    v
                };
                assert_eq!(ph(id), ph(t), "{lang:?}: {id:?} → {t:?}");
                assert_eq!(
                    id.matches('\n').count(),
                    t.matches('\n').count(),
                    "{lang:?}: {id:?}"
                );
            }
        }
    }

    #[test]
    fn every_text_in_the_code_is_in_the_template() {
        // alle `tr("…")` und `trf("…")` im Quelltext stehen in po/stijl.pot (sonst bliebe
        // der Text in allen Sprachen englisch)
        let pot: std::collections::HashSet<String> = parse_po_all(include_str!("../po/stijl.pot"))
            .into_iter()
            .collect();
        let dir = std::path::Path::new(file!()).parent().unwrap().to_path_buf();
        let mut missing = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") || path.ends_with("i18n.rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            let b = src.as_bytes();
            for start in ["tr(", "trf("] {
                for (i, _) in src.match_indices(start) {
                    // nur der Aufruf selbst (nicht z. B. `attr(`)
                    if i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_') {
                        continue;
                    }
                    let mut j = i + start.len();
                    while j < b.len() && b[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if j >= b.len() || b[j] != b'"' {
                        continue;
                    }
                    // Zeichenkette mit Escapes lesen
                    let mut text = String::new();
                    let mut k = j + 1;
                    let chars: Vec<char> = src[k..].chars().take(2000).collect();
                    let mut ci = 0;
                    while ci < chars.len() {
                        let c = chars[ci];
                        if c == '"' {
                            break;
                        }
                        if c == '\\' && ci + 1 < chars.len() {
                            match chars[ci + 1] {
                                'n' => text.push('\n'),
                                't' => text.push('\t'),
                                '"' => text.push('"'),
                                '\\' => text.push('\\'),
                                '\n' => {
                                    ci += 2;
                                    while ci < chars.len() && chars[ci].is_whitespace() {
                                        ci += 1;
                                    }
                                    continue;
                                }
                                o => {
                                    text.push('\\');
                                    text.push(o);
                                }
                            }
                            ci += 2;
                            continue;
                        }
                        text.push(c);
                        ci += 1;
                    }
                    k += ci;
                    let _ = k;
                    if !pot.contains(&text) {
                        missing.push(format!(
                            "{}: {text:?}",
                            path.file_name().unwrap().to_string_lossy()
                        ));
                    }
                }
            }
        }
        assert!(
            missing.is_empty(),
            "nicht in po/stijl.pot:\n{}",
            missing.join("\n")
        );
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
        // ohne init: Ausgangstext, Werte der Reihe nach
        assert_eq!(trf("Seite {} von {}", &[&2, &5]), "Seite 2 von 5");
        assert_eq!(trf("{{x}} {1}{0}", &[&"a", &"b"]), "{x} ba");
        // alle Übersetzungen lassen sich lesen
        for lang in [Lang::De, Lang::Es, Lang::Fr] {
            assert!(parse_po(lang.po().unwrap()).len() > 500, "{lang:?}");
        }
    }
}
