// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! Minimaler XML-Parser, ausreichend für SVG-Dateien.

use crate::i18n::{tr, trf};

#[derive(Debug, Clone)]
pub enum XNode {
    Elem(Element),
    Text(String),
}

#[derive(Debug, Clone, Default)]
pub struct Element {
    /// Vollständiger Name inklusive Präfix, z. B. `inkscape:label`.
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<XNode>,
}

impl Element {
    /// Name ohne Namensraum-Präfix.
    pub fn local(&self) -> &str {
        local_name(&self.name)
    }
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
    /// Attribut über den lokalen Namen suchen (ignoriert Präfixe).
    pub fn attr_local(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| local_name(k) == name)
            .map(|(_, v)| v.as_str())
    }
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|c| match c {
            XNode::Elem(e) => Some(e),
            _ => None,
        })
    }
    /// Gesamter Textinhalt (rekursiv).
    pub fn text(&self) -> String {
        let mut s = String::new();
        for c in &self.children {
            match c {
                XNode::Text(t) => s.push_str(t),
                XNode::Elem(e) => s.push_str(&e.text()),
            }
        }
        s
    }
}

pub fn local_name(n: &str) -> &str {
    n.rsplit(':').next().unwrap_or(n)
}

pub fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            _ => o.push(c),
        }
    }
    o
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        match rest.find(';') {
            Some(j) if j < 12 => {
                let ent = &rest[1..j];
                let rep = match ent {
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "amp" => Some('&'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some('\u{a0}'),
                    _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                        u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32)
                    }
                    _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
                    _ => None,
                };
                match rep {
                    Some(c) => {
                        out.push(c);
                        rest = &rest[j + 1..];
                    }
                    None => {
                        out.push('&');
                        rest = &rest[1..];
                    }
                }
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

struct Parser<'a> {
    s: &'a str,
    i: usize,
    /// Verschachtelungstiefe der Elemente
    depth: u32,
}

/// Tiefer verschachtelte Dateien werden abgelehnt (sonst könnte eine präparierte Datei den
/// Stapel überlaufen lassen; echte Zeichnungen kommen mit wenigen Dutzend Ebenen aus).
const MAX_DEPTH: u32 = 256;

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.i..]
    }
    fn starts(&self, p: &str) -> bool {
        self.rest().starts_with(p)
    }
    fn skip_ws(&mut self) {
        while let Some(c) = self.rest().chars().next() {
            if c.is_whitespace() {
                self.i += c.len_utf8();
            } else {
                break;
            }
        }
    }
    fn skip_past(&mut self, pat: &str) -> Result<(), String> {
        match self.rest().find(pat) {
            Some(j) => {
                self.i += j + pat.len();
                Ok(())
            }
            None => Err(trf("Unerwartetes Dateiende, „{}“ fehlt", &[&(pat)])),
        }
    }
    fn name(&mut self) -> Result<String, String> {
        let start = self.i;
        while let Some(c) = self.rest().chars().next() {
            if c.is_alphanumeric() || matches!(c, ':' | '_' | '-' | '.') {
                self.i += c.len_utf8();
            } else {
                break;
            }
        }
        if self.i == start {
            return Err(format!("Name erwartet bei Position {}", self.i));
        }
        Ok(self.s[start..self.i].to_string())
    }

    /// Überspringt Kommentare, Verarbeitungsanweisungen und DOCTYPE.
    fn skip_misc(&mut self) -> Result<bool, String> {
        if self.starts("<!--") {
            self.skip_past("-->")?;
            Ok(true)
        } else if self.starts("<?") {
            self.skip_past("?>")?;
            Ok(true)
        } else if self.starts("<!DOCTYPE") || self.starts("<!doctype") {
            // Interne Teilmenge in [...] berücksichtigen.
            let mut depth = 0;
            while let Some(c) = self.rest().chars().next() {
                self.i += c.len_utf8();
                match c {
                    '[' => depth += 1,
                    ']' => depth -= 1,
                    '>' if depth <= 0 => break,
                    _ => {}
                }
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn element(&mut self) -> Result<Element, String> {
        if self.depth >= MAX_DEPTH {
            return Err(tr("Die Datei ist zu tief verschachtelt.").into());
        }
        self.depth += 1;
        let r = self.element_inner();
        self.depth -= 1;
        r
    }

    fn element_inner(&mut self) -> Result<Element, String> {
        // Aufrufer garantiert '<'
        self.i += 1;
        let name = self.name()?;
        let mut el = Element {
            name,
            ..Default::default()
        };
        loop {
            self.skip_ws();
            if self.starts("/>") {
                self.i += 2;
                return Ok(el);
            }
            if self.starts(">") {
                self.i += 1;
                break;
            }
            let an = self.name()?;
            self.skip_ws();
            if !self.starts("=") {
                el.attrs.push((an, String::new()));
                continue;
            }
            self.i += 1;
            self.skip_ws();
            let q = self.rest().chars().next().ok_or(tr("Attributwert fehlt"))?;
            if q != '"' && q != '\'' {
                return Err(trf("Anführungszeichen erwartet bei Position {}", &[&(self.i)]));
            }
            self.i += 1;
            let end = self
                .rest()
                .find(q)
                .ok_or(tr("Attributwert nicht abgeschlossen"))?;
            let v = unescape(&self.rest()[..end]);
            self.i += end + 1;
            el.attrs.push((an, v));
        }
        // Kinder
        loop {
            if self.i >= self.s.len() {
                return Err(format!("Element <{}> nicht geschlossen", el.name));
            }
            if self.starts("</") {
                self.i += 2;
                let n = self.name()?;
                if n != el.name {
                    return Err(format!("</{n}> passt nicht zu <{}>", el.name));
                }
                self.skip_ws();
                self.skip_past(">")?;
                return Ok(el);
            }
            if self.starts("<![CDATA[") {
                self.i += 9;
                let end = self.rest().find("]]>").ok_or(tr("CDATA nicht abgeschlossen"))?;
                el.children.push(XNode::Text(self.rest()[..end].to_string()));
                self.i += end + 3;
                continue;
            }
            if self.skip_misc()? {
                continue;
            }
            if self.starts("<") {
                let child = self.element()?;
                el.children.push(XNode::Elem(child));
                continue;
            }
            let end = self.rest().find('<').unwrap_or(self.rest().len());
            let t = unescape(&self.rest()[..end]);
            self.i += end;
            el.children.push(XNode::Text(t));
        }
    }
}

pub fn parse(s: &str) -> Result<Element, String> {
    let s = s.strip_prefix('\u{feff}').unwrap_or(s);
    let mut p = Parser { s, i: 0, depth: 0 };
    loop {
        p.skip_ws();
        if p.i >= s.len() {
            return Err(tr("Kein Wurzelelement gefunden").into());
        }
        if p.skip_misc()? {
            continue;
        }
        if p.starts("<") {
            return p.element();
        }
        return Err(tr("Ungültiger Dateianfang").into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_nesting_is_an_error_not_a_crash() {
        let n = 100_000;
        let s = format!("<a>{}{}</a>", "<b>".repeat(n), "</b>".repeat(n));
        assert!(parse(&s).is_err());
        let ok = format!("<a>{}{}</a>", "<b>".repeat(200), "</b>".repeat(200));
        assert!(parse(&ok).is_ok());
    }

    #[test]
    fn basic() {
        let x = r#"<?xml version="1.0"?>
<!DOCTYPE svg [ <!ENTITY a "b"> ]>
<!-- c -->
<svg xmlns="x" width='10'><g inkscape:label="L &amp; 1"><rect/><text>a&lt;b<tspan>c</tspan></text></g><![CDATA[z]]></svg>"#;
        let e = parse(x).unwrap();
        assert_eq!(e.local(), "svg");
        assert_eq!(e.attr("width"), Some("10"));
        let g = e.elements().next().unwrap();
        assert_eq!(g.attr_local("label"), Some("L & 1"));
        let t = g.elements().nth(1).unwrap();
        assert_eq!(t.text(), "a<bc");
    }
}
