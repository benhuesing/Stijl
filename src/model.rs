// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! Dokumentmodell: Ebenen, Objekte, Formen und Stile.

use crate::i18n::{tr, trf};
use std::collections::HashMap;
use std::rc::Rc;

use crate::geom::{cubic_bbox, Affine, Pt, Rect};

/// Faktor, mit dem sich Kreisbögen als kubische Bézierkurven annähern lassen.
pub const KAPPA: f64 = 0.552_284_749_830_793_4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Color {
    pub const fn rgb(r: f64, g: f64, b: f64) -> Color {
        Color { r, g, b, a: 1.0 }
    }
    pub const WHITE: Color = Color::rgb(1.0, 1.0, 1.0);
    pub const BLACK: Color = Color::rgb(0.0, 0.0, 0.0);

    /// Einfache (nicht farbverwaltete) Umrechnung nach CMYK, Werte 0–1.
    pub fn to_cmyk(&self) -> (f64, f64, f64, f64) {
        let k = 1.0 - self.r.max(self.g).max(self.b);
        if k >= 1.0 - 1e-9 {
            return (0.0, 0.0, 0.0, 1.0);
        }
        let d = 1.0 - k;
        (
            (1.0 - self.r - k) / d,
            (1.0 - self.g - k) / d,
            (1.0 - self.b - k) / d,
            k,
        )
    }

    pub fn from_cmyk(c: f64, m: f64, y: f64, k: f64, a: f64) -> Color {
        let f = |v: f64| ((1.0 - v.clamp(0.0, 1.0)) * (1.0 - k.clamp(0.0, 1.0))).clamp(0.0, 1.0);
        Color {
            r: f(c),
            g: f(m),
            b: f(y),
            a,
        }
    }

    pub fn to_hex(&self) -> String {
        let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02x}{:02x}{:02x}", c(self.r), c(self.g), c(self.b))
    }

    /// Liest #rgb, #rrggbb, rgb(…), rgba(…) und gängige Farbnamen.
    pub fn parse(s: &str) -> Option<Color> {
        let s = s.trim();
        let lower = s.to_ascii_lowercase();
        if let Some(hex) = lower.strip_prefix('#') {
            // nur ASCII lässt sich stückweise lesen (sonst läge ein Schnitt mitten in einem Zeichen)
            let v = |i: usize, n: usize| hex.get(i..i + n).and_then(|h| u8::from_str_radix(h, 16).ok());
            return match hex.len() {
                3 | 4 => {
                    let r = v(0, 1)? as f64 * 17.0;
                    let g = v(1, 1)? as f64 * 17.0;
                    let b = v(2, 1)? as f64 * 17.0;
                    let a = if hex.len() == 4 {
                        v(3, 1)? as f64 * 17.0
                    } else {
                        255.0
                    };
                    Some(Color {
                        r: r / 255.0,
                        g: g / 255.0,
                        b: b / 255.0,
                        a: a / 255.0,
                    })
                }
                6 | 8 => {
                    let a = if hex.len() == 8 { v(6, 2)? as f64 } else { 255.0 };
                    Some(Color {
                        r: v(0, 2)? as f64 / 255.0,
                        g: v(2, 2)? as f64 / 255.0,
                        b: v(4, 2)? as f64 / 255.0,
                        a: a / 255.0,
                    })
                }
                _ => None,
            };
        }
        if lower.starts_with("rgb") {
            let open = lower.find('(')?;
            let close = lower.find(')')?;
            let parts: Vec<&str> = lower[open + 1..close]
                .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
                .filter(|p| !p.is_empty())
                .collect();
            if parts.len() < 3 {
                return None;
            }
            let chan = |p: &str| -> Option<f64> {
                if let Some(pc) = p.strip_suffix('%') {
                    Some(pc.parse::<f64>().ok()? / 100.0)
                } else {
                    Some(p.parse::<f64>().ok()? / 255.0)
                }
            };
            let a = match parts.get(3) {
                Some(p) => {
                    if let Some(pc) = p.strip_suffix('%') {
                        pc.parse::<f64>().ok()? / 100.0
                    } else {
                        p.parse::<f64>().ok()?
                    }
                }
                None => 1.0,
            };
            return Some(Color {
                r: chan(parts[0])?,
                g: chan(parts[1])?,
                b: chan(parts[2])?,
                a,
            });
        }
        let named = match lower.as_str() {
            "black" => (0, 0, 0),
            "white" => (255, 255, 255),
            "red" => (255, 0, 0),
            "green" => (0, 128, 0),
            "lime" => (0, 255, 0),
            "blue" => (0, 0, 255),
            "yellow" => (255, 255, 0),
            "cyan" | "aqua" => (0, 255, 255),
            "magenta" | "fuchsia" => (255, 0, 255),
            "gray" | "grey" => (128, 128, 128),
            "silver" => (192, 192, 192),
            "maroon" => (128, 0, 0),
            "olive" => (128, 128, 0),
            "navy" => (0, 0, 128),
            "purple" => (128, 0, 128),
            "teal" => (0, 128, 128),
            "orange" => (255, 165, 0),
            "pink" => (255, 192, 203),
            "brown" => (165, 42, 42),
            _ => return None,
        };
        Some(Color::rgb(
            named.0 as f64 / 255.0,
            named.1 as f64 / 255.0,
            named.2 as f64 / 255.0,
        ))
    }
}

/// Ankerpunkt eines Pfades mit eingehendem und ausgehendem Anfasser (absolute Koordinaten).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Node {
    pub p: Pt,
    pub h_in: Pt,
    pub h_out: Pt,
    /// Eckenradius (lebendige Ecke): wird beim Zeichnen angewendet, der Knoten bleibt spitz
    /// gespeichert und lässt sich jederzeit wieder ändern. 0 = keine Rundung.
    pub r: f64,
}

impl Node {
    pub fn corner(p: Pt) -> Node {
        Node {
            r: 0.0,
            p,
            h_in: p,
            h_out: p,
        }
    }
    pub fn is_corner(&self) -> bool {
        self.h_in.dist(self.p) < 1e-9 && self.h_out.dist(self.p) < 1e-9
    }
    /// Beide Anfasser liegen auf einer Linie durch den Punkt.
    pub fn is_smooth(&self) -> bool {
        let a = self.p - self.h_in;
        let b = self.h_out - self.p;
        if a.len() < 1e-6 || b.len() < 1e-6 {
            return false;
        }
        let cross = a.normalized().x * b.normalized().y - a.normalized().y * b.normalized().x;
        let dot = a.x * b.x + a.y * b.y;
        cross.abs() < 0.02 && dot > 0.0
    }
    fn map(&mut self, m: &Affine) {
        self.p = m.apply(self.p);
        self.h_in = m.apply(self.h_in);
        self.h_out = m.apply(self.h_out);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubPath {
    pub nodes: Vec<Node>,
    pub closed: bool,
}

impl SubPath {
    /// Geometrie mit angewendeten Eckenradien.
    pub fn effective(&self) -> SubPath {
        let items: Vec<(usize, f64)> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.r > 0.0)
            .map(|(i, n)| (i, n.r))
            .collect();
        if items.is_empty() {
            return self.clone();
        }
        fillet_radii(self, &items).0
    }

    pub fn has_radii(&self) -> bool {
        self.nodes.iter().any(|n| n.r > 0.0)
    }

    /// Segmente als (Start, Kontrolle1, Kontrolle2, Ende).
    pub fn segments(&self) -> Vec<(Pt, Pt, Pt, Pt)> {
        let n = self.nodes.len();
        let mut out = Vec::with_capacity(n);
        for i in 1..n {
            let a = &self.nodes[i - 1];
            let b = &self.nodes[i];
            out.push((a.p, a.h_out, b.h_in, b.p));
        }
        if self.closed && n > 1 {
            let a = &self.nodes[n - 1];
            let b = &self.nodes[0];
            out.push((a.p, a.h_out, b.h_in, b.p));
        }
        out
    }
}

/// Kennwerte eines regelmäßigen Vielecks bzw. Sterns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegularPolygon {
    pub center: Pt,
    pub radius: f64,
    /// Stern: Innenradius als Anteil
    pub inner: Option<f64>,
    /// Drehung (0 = Spitze oben)
    pub angle: f64,
    pub corners: u32,
    /// Eckenradius
    pub fillet: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    Rect,
    Ellipse,
    Line,
    Polygon,
    Star,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Rect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        r: f64,
    },
    Ellipse {
        cx: f64,
        cy: f64,
        rx: f64,
        ry: f64,
    },
    Line {
        a: Pt,
        b: Pt,
    },
    Path {
        subpaths: Vec<SubPath>,
        evenodd: bool,
    },
    /// Text im eigenen Koordinatensystem: Grundlinie der ersten Zeile beginnt bei (0, 0),
    /// `xf` bildet es auf das Dokument ab (Position, Drehung, Spiegelung, Verzerrung).
    Text {
        xf: Affine,
        text: String,
        family: String,
        size: f64,
        bold: bool,
        italic: bool,
        fmt: TextFormat,
    },
    /// Text entlang eines Pfades; `path` ist die Grundlinie, `offset` der Abstand vom Pfadanfang.
    TextPath {
        path: Vec<SubPath>,
        text: String,
        family: String,
        size: f64,
        bold: bool,
        italic: bool,
        offset: f64,
        fmt: TextFormat,
    },
    /// Textfeld mit Zeilenumbruch: `xf` bildet die linke obere Ecke (0,0) ab und enthält nur
    /// Lage, Drehung und Spiegelung; `w`/`h` ist die Feldgröße.
    TextArea {
        xf: Affine,
        w: f64,
        h: f64,
        text: String,
        family: String,
        size: f64,
        bold: bool,
        italic: bool,
        fmt: TextFormat,
        /// Rahmen des Feldes (Form, Füllung, Kontur, Abstand des Textes)
        frame: TextFrame,
        /// Formatierung von Textteilen und Absätzen
        rich: RichText,
    },
    /// Eingebettetes Rasterbild (Originaldatei); `xf` bildet (0,0)–(w,h) auf das Dokument ab.
    Image {
        xf: Affine,
        w: f64,
        h: f64,
        data: Rc<Vec<u8>>,
        /// Rahmen (Beschnitt) im Dokument: nur innerhalb ist das Bild sichtbar.
        clip: Option<Vec<SubPath>>,
    },
}

impl Shape {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Shape::Rect { .. } => tr("Rectangle"),
            Shape::Ellipse { .. } => tr("Ellipse"),
            Shape::Line { .. } => tr("Line"),
            Shape::Path { .. } => tr("Path"),
            Shape::Text { .. } => tr("Text"),
            Shape::TextPath { .. } => tr("Text on Path"),
            Shape::TextArea { .. } => tr("Text Box"),
            Shape::Image { .. } => tr("Image"),
        }
    }

    /// Offene Linie (Linie oder Pfad ohne geschlossene Teilpfade): hat Enden, keine Füllung.
    pub fn is_open(&self) -> bool {
        match self {
            Shape::Line { .. } => true,
            Shape::Path { subpaths, .. } => !subpaths.is_empty() && subpaths.iter().all(|s| !s.closed),
            _ => false,
        }
    }

    /// Text oder Text auf Pfad.
    pub fn is_text(&self) -> bool {
        matches!(
            self,
            Shape::Text { .. } | Shape::TextPath { .. } | Shape::TextArea { .. }
        )
    }

    /// Veränderbare Textfelder (Inhalt, Schrift, Größe, fett, kursiv).
    #[allow(clippy::type_complexity)]
    pub fn text_fields_mut(&mut self) -> Option<(&mut String, &mut String, &mut f64, &mut bool, &mut bool)> {
        match self {
            Shape::Text {
                text,
                family,
                size,
                bold,
                italic,
                ..
            }
            | Shape::TextArea {
                text,
                family,
                size,
                bold,
                italic,
                ..
            }
            | Shape::TextPath {
                text,
                family,
                size,
                bold,
                italic,
                ..
            } => Some((text, family, size, bold, italic)),
            _ => None,
        }
    }

    /// Textfelder lesen: (Inhalt, Schrift, Größe, fett, kursiv).
    pub fn text_fields(&self) -> Option<(&str, &str, f64, bool, bool)> {
        match self {
            Shape::Text {
                text,
                family,
                size,
                bold,
                italic,
                ..
            }
            | Shape::TextArea {
                text,
                family,
                size,
                bold,
                italic,
                ..
            }
            | Shape::TextPath {
                text,
                family,
                size,
                bold,
                italic,
                ..
            } => Some((text.as_str(), family.as_str(), *size, *bold, *italic)),
            _ => None,
        }
    }

    /// Neuer Text mit Grundlinienanfang bei `pos`.
    pub fn text_at(pos: Pt, text: &str, family: &str, size: f64, bold: bool, italic: bool) -> Shape {
        Shape::Text {
            xf: Affine::translate(pos.x, pos.y),
            text: text.to_string(),
            family: family.to_string(),
            size,
            bold,
            italic,
            fmt: TextFormat::default(),
        }
    }

    /// Absatzformat eines Textes (Ausrichtung, Abstände).
    pub fn text_format(&self) -> Option<TextFormat> {
        match self {
            Shape::Text { fmt, .. } | Shape::TextPath { fmt, .. } | Shape::TextArea { fmt, .. } => Some(*fmt),
            _ => None,
        }
    }

    pub fn text_format_mut(&mut self) -> Option<&mut TextFormat> {
        match self {
            Shape::Text { fmt, .. } | Shape::TextPath { fmt, .. } | Shape::TextArea { fmt, .. } => Some(fmt),
            _ => None,
        }
    }

    /// Formatierung von Textteilen (nur Textfelder).
    pub fn rich(&self) -> Option<&RichText> {
        match self {
            Shape::TextArea { rich, .. } => Some(rich),
            _ => None,
        }
    }

    pub fn rich_mut(&mut self) -> Option<&mut RichText> {
        match self {
            Shape::TextArea { rich, .. } => Some(rich),
            _ => None,
        }
    }

    /// Grundformat der Zeichen (Schrift, Größe, Auszeichnungen) eines Textes.
    pub fn char_base(&self) -> Option<CharFmt> {
        let (_, family, size, bold, italic) = self.text_fields()?;
        let fmt = self.text_format()?;
        Some(CharFmt {
            family: family.to_string(),
            size,
            bold,
            weight: fmt.weight_for(bold),
            italic,
            underline: fmt.underline,
            strike: fmt.strike,
        })
    }

    pub fn set_char_base(&mut self, c: &CharFmt) {
        if let Some((_, family, size, bold, italic)) = self.text_fields_mut() {
            *family = c.family.clone();
            *size = c.size;
            *bold = c.bold;
            *italic = c.italic;
        }
        if let Some(f) = self.text_format_mut() {
            f.underline = c.underline;
            f.strike = c.strike;
            // Standardstärke von fett/normal wird nicht eigens gespeichert
            f.weight = if c.weight == effective_weight(0, c.bold) {
                0
            } else {
                c.weight
            };
        }
    }

    /// Text ersetzen; die Formatierung von Textteilen und Absätzen wird nachgeführt.
    pub fn replace_text(&mut self, new: String) {
        let old = self.text_fields().map(|t| t.0.to_string()).unwrap_or_default();
        if let Some(r) = self.rich_mut() {
            r.edit(&old, &new);
        }
        if let Some((t, ..)) = self.text_fields_mut() {
            *t = new;
        }
    }

    /// Erzeugt ein regelmäßiges Vieleck oder einen Stern um `center`.
    pub fn polygon(center: Pt, radius: f64, angle: f64, corners: u32, star: bool) -> Shape {
        Shape::polygon_ratio(center, radius, angle, corners, star.then_some(0.5))
    }

    /// Wie `polygon`, mit wählbarem Innenradius des Sterns (Anteil am Außenradius).
    pub fn polygon_ratio(center: Pt, radius: f64, angle: f64, corners: u32, inner: Option<f64>) -> Shape {
        let star = inner.is_some();
        let ratio = inner.unwrap_or(1.0);
        let corners = corners.max(3) as usize;
        let count = if star { corners * 2 } else { corners };
        let nodes = (0..count)
            .map(|i| {
                let r = if star && i % 2 == 1 {
                    radius * ratio
                } else {
                    radius
                };
                let t = angle - std::f64::consts::FRAC_PI_2 + i as f64 * std::f64::consts::TAU / count as f64;
                Node::corner(Pt::new(center.x + r * t.cos(), center.y + r * t.sin()))
            })
            .collect();
        Shape::Path {
            subpaths: vec![SubPath { nodes, closed: true }],
            evenodd: false,
        }
    }

    /// Ist das ein regelmäßiges Vieleck oder ein Stern (wie mit dem Formwerkzeug erzeugt)?
    /// Dann lässt sich die Anzahl der Ecken nachträglich ändern.
    pub fn regular_polygon(&self) -> Option<RegularPolygon> {
        let Shape::Path { subpaths, .. } = self else {
            return None;
        };
        let [sp] = subpaths.as_slice() else { return None };
        let n = sp.nodes.len();
        if !sp.closed || n < 3 {
            return None;
        }
        // nur Eckpunkte ohne Griffe
        if sp
            .nodes
            .iter()
            .any(|nd| nd.h_in.dist(nd.p) > 1e-6 || nd.h_out.dist(nd.p) > 1e-6)
        {
            return None;
        }
        let c = sp.nodes.iter().fold(Pt::new(0.0, 0.0), |a, nd| a + nd.p) * (1.0 / n as f64);
        let rs: Vec<f64> = sp.nodes.iter().map(|nd| nd.p.dist(c)).collect();
        let outer = rs.iter().cloned().fold(0.0, f64::max);
        if outer < 1e-6 {
            return None;
        }
        let tol = outer * 1e-3 + 1e-6;
        // gleiche Winkelabstände
        let a0 = (sp.nodes[0].p.y - c.y).atan2(sp.nodes[0].p.x - c.x);
        let step = std::f64::consts::TAU / n as f64;
        for (i, nd) in sp.nodes.iter().enumerate() {
            let a = (nd.p.y - c.y).atan2(nd.p.x - c.x);
            let want = a0 + step * i as f64;
            let d = (a - want).rem_euclid(std::f64::consts::TAU);
            let d = d.min(std::f64::consts::TAU - d);
            let d2 = {
                let w = a0 - step * i as f64;
                let e = (a - w).rem_euclid(std::f64::consts::TAU);
                e.min(std::f64::consts::TAU - e)
            };
            if d.min(d2) * outer > tol * 4.0 {
                return None;
            }
        }
        let same = |a: f64, b: f64| (a - b).abs() <= tol;
        let radius = sp.nodes[0].r;
        if rs.iter().all(|r| same(*r, rs[0])) {
            return Some(RegularPolygon {
                center: c,
                radius: rs[0],
                inner: None,
                angle: a0 + std::f64::consts::FRAC_PI_2,
                corners: n as u32,
                fillet: radius,
            });
        }
        // Stern: abwechselnd außen und innen
        if n % 2 == 0
            && n >= 6
            && rs.iter().step_by(2).all(|r| same(*r, rs[0]))
            && rs.iter().skip(1).step_by(2).all(|r| same(*r, rs[1]))
            && rs[0] > rs[1]
        {
            return Some(RegularPolygon {
                center: c,
                radius: rs[0],
                inner: Some(rs[1] / rs[0]),
                angle: a0 + std::f64::consts::FRAC_PI_2,
                corners: (n / 2) as u32,
                fillet: radius,
            });
        }
        None
    }

    /// Wandelt jede Form außer Text in einen Bézierpfad um.
    pub fn to_path(&self) -> Option<Shape> {
        let subpaths = match *self {
            Shape::Rect { x, y, w, h, r } => {
                let r = r.max(0.0).min(w / 2.0).min(h / 2.0);
                if r < 1e-9 {
                    vec![SubPath {
                        nodes: vec![
                            Node::corner(Pt::new(x, y)),
                            Node::corner(Pt::new(x + w, y)),
                            Node::corner(Pt::new(x + w, y + h)),
                            Node::corner(Pt::new(x, y + h)),
                        ],
                        closed: true,
                    }]
                } else {
                    let k = r * (1.0 - KAPPA);
                    let n = |p: Pt, hi: Pt, ho: Pt| Node {
                        r: 0.0,
                        p,
                        h_in: hi,
                        h_out: ho,
                    };
                    vec![SubPath {
                        nodes: vec![
                            n(Pt::new(x + r, y), Pt::new(x + k, y), Pt::new(x + r, y)),
                            n(
                                Pt::new(x + w - r, y),
                                Pt::new(x + w - r, y),
                                Pt::new(x + w - k, y),
                            ),
                            n(
                                Pt::new(x + w, y + r),
                                Pt::new(x + w, y + k),
                                Pt::new(x + w, y + r),
                            ),
                            n(
                                Pt::new(x + w, y + h - r),
                                Pt::new(x + w, y + h - r),
                                Pt::new(x + w, y + h - k),
                            ),
                            n(
                                Pt::new(x + w - r, y + h),
                                Pt::new(x + w - k, y + h),
                                Pt::new(x + w - r, y + h),
                            ),
                            n(
                                Pt::new(x + r, y + h),
                                Pt::new(x + r, y + h),
                                Pt::new(x + k, y + h),
                            ),
                            n(
                                Pt::new(x, y + h - r),
                                Pt::new(x, y + h - k),
                                Pt::new(x, y + h - r),
                            ),
                            n(Pt::new(x, y + r), Pt::new(x, y + r), Pt::new(x, y + k)),
                        ],
                        closed: true,
                    }]
                }
            }
            Shape::Ellipse { cx, cy, rx, ry } => {
                let (kx, ky) = (rx * KAPPA, ry * KAPPA);
                let n = |px: f64, py: f64, ix: f64, iy: f64, ox: f64, oy: f64| Node {
                    r: 0.0,
                    p: Pt::new(px, py),
                    h_in: Pt::new(ix, iy),
                    h_out: Pt::new(ox, oy),
                };
                vec![SubPath {
                    nodes: vec![
                        n(cx + rx, cy, cx + rx, cy - ky, cx + rx, cy + ky),
                        n(cx, cy + ry, cx + kx, cy + ry, cx - kx, cy + ry),
                        n(cx - rx, cy, cx - rx, cy + ky, cx - rx, cy - ky),
                        n(cx, cy - ry, cx - kx, cy - ry, cx + kx, cy - ry),
                    ],
                    closed: true,
                }]
            }
            Shape::Line { a, b } => vec![SubPath {
                nodes: vec![Node::corner(a), Node::corner(b)],
                closed: false,
            }],
            Shape::Path {
                ref subpaths,
                evenodd,
            } => {
                return Some(Shape::Path {
                    subpaths: effective_subpaths(subpaths),
                    evenodd,
                })
            }
            Shape::Text { .. } | Shape::TextPath { .. } | Shape::TextArea { .. } | Shape::Image { .. } => {
                return None
            }
        };
        Some(Shape::Path {
            subpaths,
            evenodd: false,
        })
    }

    /// Wendet eine Transformation an. Rechtecke/Ellipsen werden bei Drehung zu Pfaden.
    pub fn transform(&mut self, m: &Affine) {
        match self {
            Shape::Rect { x, y, w, h, r } if m.is_axis_aligned() => {
                let a = m.apply(Pt::new(*x, *y));
                let b = m.apply(Pt::new(*x + *w, *y + *h));
                let rr = Rect::from_points(a, b);
                *r *= m.mean_scale();
                *x = rr.x0;
                *y = rr.y0;
                *w = rr.width();
                *h = rr.height();
            }
            Shape::Ellipse { cx, cy, rx, ry } if m.is_axis_aligned() => {
                let c = m.apply(Pt::new(*cx, *cy));
                *cx = c.x;
                *cy = c.y;
                *rx *= m.a.abs();
                *ry *= m.d.abs();
            }
            Shape::Line { a, b } => {
                *a = m.apply(*a);
                *b = m.apply(*b);
            }
            Shape::Path { subpaths, .. } => {
                let s = m.mean_scale();
                for sp in subpaths.iter_mut() {
                    for n in sp.nodes.iter_mut() {
                        n.map(m);
                        n.r *= s;
                    }
                }
            }
            Shape::Text { xf, size, fmt, .. } => {
                // Skalierung in die Schriftgröße übernehmen, damit die Matrix nur Lage,
                // Drehung und Spiegelung enthält; der Abstand zum Rahmen wächst mit.
                let mut x = m.then_before(xf);
                let s = x.mean_scale();
                if s > 1e-9 && (s - 1.0).abs() > 1e-12 {
                    x.a /= s;
                    x.b /= s;
                    x.c /= s;
                    x.d /= s;
                    *size = (*size * s).max(0.5);
                    fmt.box_inset *= s;
                }
                *xf = x;
            }
            Shape::TextPath {
                path,
                size,
                offset,
                fmt,
                ..
            } => {
                for sp in path.iter_mut() {
                    for n in sp.nodes.iter_mut() {
                        n.map(m);
                    }
                }
                let s = m.mean_scale();
                *size = (*size * s).max(0.5);
                *offset *= s;
                fmt.box_inset *= s;
            }
            Shape::Image { xf, clip, .. } => {
                *xf = m.then_before(xf);
                if let Some(c) = clip {
                    for sp in c.iter_mut() {
                        for n in sp.nodes.iter_mut() {
                            n.map(m);
                        }
                    }
                }
            }
            Shape::TextArea { xf, w, h, frame, .. } => {
                // Feld vergrößern, Schrift bleibt gleich (Text bricht neu um).
                let x = m.then_before(xf);
                let sx = (x.a * x.a + x.b * x.b).sqrt();
                let sy = (x.c * x.c + x.d * x.d).sqrt();
                // Rahmenform (lokale Koordinaten) mitskalieren
                let scaled = (sx - 1.0).abs() > 1e-4 || (sy - 1.0).abs() > 1e-4;
                if let Some(path) = frame.path.as_mut().filter(|_| scaled) {
                    let sm = Affine::scale(sx.max(1e-9), sy.max(1e-9));
                    for sp in path.iter_mut() {
                        for n in sp.nodes.iter_mut() {
                            n.map(&sm);
                            n.r *= (sx * sy).sqrt();
                        }
                    }
                }
                let mut n = x;
                if sx > 1e-9 {
                    n.a /= sx;
                    n.b /= sx;
                }
                if sy > 1e-9 {
                    n.c /= sy;
                    n.d /= sy;
                }
                *w = (*w * sx).max(1.0);
                *h = (*h * sy).max(1.0);
                *xf = n;
            }
            _ => {
                if let Some(mut p) = self.to_path() {
                    p.transform(m);
                    *self = p;
                }
            }
        }
    }

    /// Nur einen Teil eines gerahmten Bildes transformieren (Rahmen oder Inhalt).
    pub fn transform_part(&mut self, m: &Affine, part: ImagePart) {
        match (self, part) {
            (Shape::Image { clip: Some(c), .. }, ImagePart::Frame) => {
                for sp in c.iter_mut() {
                    for n in sp.nodes.iter_mut() {
                        n.map(m);
                    }
                }
            }
            (Shape::Image { xf, .. }, ImagePart::Content) => {
                *xf = m.then_before(xf);
            }
            (s, _) => s.transform(m),
        }
    }

    /// Ecken des ganzen Bildes (ohne Rahmen) im Dokument.
    pub fn image_quad(&self) -> Option<[Pt; 4]> {
        let Shape::Image { xf, w, h, .. } = self else {
            return None;
        };
        Some([
            xf.apply(Pt::new(0.0, 0.0)),
            xf.apply(Pt::new(*w, 0.0)),
            xf.apply(Pt::new(*w, *h)),
            xf.apply(Pt::new(0.0, *h)),
        ])
    }

    pub fn translate(&mut self, d: Pt) {
        self.transform(&Affine::translate(d.x, d.y));
    }

    /// Geometrischer Rahmen ohne Strichbreite. Für Text `None` (braucht Schriftmetriken).
    pub fn geom_bbox(&self) -> Option<Rect> {
        match self {
            Shape::Rect { x, y, w, h, .. } => Some(Rect::from_xywh(*x, *y, *w, *h)),
            Shape::Ellipse { cx, cy, rx, ry } => Some(Rect {
                x0: cx - rx,
                y0: cy - ry,
                x1: cx + rx,
                y1: cy + ry,
            }),
            Shape::Line { a, b } => Some(Rect::from_points(*a, *b)),
            Shape::Path { subpaths, .. } => {
                let mut r: Option<Rect> = None;
                for sp in &effective_subpaths(subpaths) {
                    if sp.nodes.len() == 1 {
                        let p = sp.nodes[0].p;
                        r = Some(match r {
                            Some(mut rr) => {
                                rr.include(p);
                                rr
                            }
                            None => Rect::from_points(p, p),
                        });
                    }
                    for (p0, c1, c2, p1) in sp.segments() {
                        let b = cubic_bbox(p0, c1, c2, p1);
                        r = Some(match r {
                            Some(rr) => rr.union(&b),
                            None => b,
                        });
                    }
                }
                r
            }
            Shape::Image { clip: Some(c), .. } => Shape::Path {
                subpaths: c.clone(),
                evenodd: false,
            }
            .geom_bbox(),
            Shape::Image { xf, w, h, .. } | Shape::TextArea { xf, w, h, .. } => {
                let mut r = Rect::from_points(xf.apply(Pt::new(0.0, 0.0)), xf.apply(Pt::new(*w, 0.0)));
                r.include(xf.apply(Pt::new(*w, *h)));
                r.include(xf.apply(Pt::new(0.0, *h)));
                Some(r)
            }
            Shape::Text { .. } | Shape::TextPath { .. } => None,
        }
    }
}

/// Erster Teilpfad als Polylinie mit Bogenlängen (Grundlinie für Text auf Pfad).
pub fn path_polyline(path: &[SubPath]) -> (Vec<Pt>, Vec<f64>) {
    let Some(sp) = path.first().map(|s| s.effective()) else {
        return (Vec::new(), Vec::new());
    };
    let mut pts = Vec::new();
    if let Some(n) = sp.nodes.first() {
        pts.push(n.p);
    }
    for (p0, c1, c2, p3) in sp.segments() {
        let steps = 24;
        for i in 1..=steps {
            pts.push(crate::geom::cubic_point(p0, c1, c2, p3, i as f64 / steps as f64));
        }
    }
    let mut len = vec![0.0];
    for w in pts.windows(2) {
        let l = len[len.len() - 1] + w[0].dist(w[1]);
        len.push(l);
    }
    (pts, len)
}

/// Punkt und Richtungswinkel bei Bogenlänge `d`.
pub fn point_at_length(pts: &[Pt], len: &[f64], d: f64) -> Option<(Pt, f64)> {
    if pts.len() < 2 {
        return None;
    }
    let i = match len.binary_search_by(|v| v.total_cmp(&d)) {
        Ok(i) => i.max(1),
        Err(i) => i.clamp(1, len.len() - 1),
    };
    let (a, b) = (pts[i - 1], pts[i]);
    let seg = (len[i] - len[i - 1]).max(1e-12);
    let t = ((d - len[i - 1]) / seg).clamp(0.0, 1.0);
    let dir = b - a;
    Some((a.lerp(b, t), dir.y.atan2(dir.x)))
}

/// Bogenlänge des Punktes auf der Polylinie, der `p` am nächsten liegt.
pub fn nearest_length(pts: &[Pt], len: &[f64], p: Pt) -> f64 {
    let mut best = (f64::MAX, 0.0);
    for i in 1..pts.len() {
        let (a, b) = (pts[i - 1], pts[i]);
        let ab = b - a;
        let l2 = ab.x * ab.x + ab.y * ab.y;
        let t = if l2 < 1e-12 {
            0.0
        } else {
            (((p - a).x * ab.x + (p - a).y * ab.y) / l2).clamp(0.0, 1.0)
        };
        let q = a.lerp(b, t);
        let d = q.dist(p);
        if d < best.0 {
            best = (d, len[i - 1] + (len[i] - len[i - 1]) * t);
        }
    }
    best.1
}

/// Geometrie einer Ecke: Punkt, Einlauf- und Auslaufrichtung, Innenwinkel.
#[derive(Clone, Copy, Debug)]
pub struct Corner {
    pub p: Pt,
    pub u_in: Pt,
    pub u_out: Pt,
    pub theta: f64,
}

impl Corner {
    /// Richtung der Winkelhalbierenden ins Innere der Ecke.
    pub fn bisector(&self) -> Pt {
        (self.u_out - self.u_in).normalized()
    }
}

/// Ecke an Knoten `i` (nur wenn ein Abrunden dort sinnvoll ist).
pub fn corner_at(sp: &SubPath, i: usize) -> Option<Corner> {
    let n = sp.nodes.len();
    if n < 3 || i >= n || (!sp.closed && (i == 0 || i == n - 1)) {
        return None;
    }
    let node = sp.nodes[i];
    let prev = sp.nodes[(i + n - 1) % n];
    let next = sp.nodes[(i + 1) % n];
    let p = node.p;
    let a = if node.h_in.dist(p) > 1e-9 {
        node.h_in
    } else if prev.h_out.dist(p) > 1e-9 {
        prev.h_out
    } else {
        prev.p
    };
    let b = if node.h_out.dist(p) > 1e-9 {
        node.h_out
    } else if next.h_in.dist(p) > 1e-9 {
        next.h_in
    } else {
        next.p
    };
    let u_in = (p - a).normalized();
    let u_out = (b - p).normalized();
    if u_in.len() < 0.5 || u_out.len() < 0.5 {
        return None;
    }
    let cos_t = (-(u_in.x * u_out.x + u_in.y * u_out.y)).clamp(-1.0, 1.0);
    let theta = cos_t.acos();
    if !(1e-3..=std::f64::consts::PI - 1e-3).contains(&theta) {
        return None;
    }
    Some(Corner {
        p,
        u_in,
        u_out,
        theta,
    })
}

/// Größter sinnvoller Radius an Knoten `i`: Der Bogen darf eine angrenzende Kante höchstens
/// bis zur Mitte belegen, wenn die Nachbarecke auch gerundet ist (`rounded`), sonst ganz.
pub fn max_corner_radius(sp: &SubPath, i: usize, rounded: &dyn Fn(usize) -> bool) -> Option<f64> {
    let c = corner_at(sp, i)?;
    let n = sp.nodes.len();
    let (ip, inx) = ((i + n - 1) % n, (i + 1) % n);
    let share = |k: usize| {
        if corner_at(sp, k).is_some() && rounded(k) {
            0.5
        } else {
            1.0
        }
    };
    let t = (sp.nodes[ip].p.dist(c.p) * share(ip)).min(sp.nodes[inx].p.dist(c.p) * share(inx));
    Some(t * (c.theta / 2.0).tan())
}

/// Ecken eines Teilpfades abrunden. `idxs` sind die Knoten, `r` der Radius.
/// Liefert den neuen Teilpfad und die Indizes der neu entstandenen Bogenknoten.
#[cfg(test)]
pub fn fillet(sp: &SubPath, idxs: &[usize], r: f64) -> (SubPath, Vec<usize>) {
    if r <= 0.0 {
        return (sp.clone(), idxs.to_vec());
    }
    let items: Vec<(usize, f64)> = idxs.iter().map(|&i| (i, r)).collect();
    fillet_radii(sp, &items)
}

/// Alle Teilpfade mit angewendeten Eckenradien.
pub fn effective_subpaths(subs: &[SubPath]) -> Vec<SubPath> {
    subs.iter().map(|s| s.effective()).collect()
}

/// Ecken mit je eigenem Radius abrunden: `items` = (Knotenindex, Radius).
pub fn fillet_radii(sp: &SubPath, items: &[(usize, f64)]) -> (SubPath, Vec<usize>) {
    let n = sp.nodes.len();
    let mut out = sp.clone();
    if n < 3 {
        return (out, items.iter().map(|x| x.0).collect());
    }
    let mut sorted: Vec<(usize, f64)> = items.iter().copied().filter(|&(i, r)| i < n && r > 0.0).collect();
    sorted.sort_by_key(|a| a.0);
    sorted.dedup_by(|a, b| a.0 == b.0);
    // Ersatzknoten je Index berechnen (auf dem Original), dann von hinten einsetzen.
    let mut replacements: Vec<(usize, [Node; 2])> = Vec::new();
    for &(i, r) in &sorted {
        let Some(Corner {
            p,
            u_in,
            u_out,
            theta,
        }) = corner_at(sp, i)
        else {
            continue;
        };
        let prev = sp.nodes[(i + n - 1) % n];
        let next = sp.nodes[(i + 1) % n];
        let mut t = r / (theta / 2.0).tan();
        // Nachbarecke ebenfalls gerundet: Kante teilen, sonst ganz nutzbar
        let rounded = |k: usize| sorted.iter().any(|&(j, _)| j == k) && corner_at(sp, k).is_some();
        let lp = if rounded((i + n - 1) % n) { 0.5 } else { 1.0 };
        let ln = if rounded((i + 1) % n) { 0.5 } else { 1.0 };
        let limit = (prev.p.dist(p) * lp).min(next.p.dist(p) * ln);
        t = t.min(limit);
        let r_eff = t * (theta / 2.0).tan();
        let phi = std::f64::consts::PI - theta;
        let hl = 4.0 / 3.0 * (phi / 4.0).tan() * r_eff;
        let p1 = p - u_in * t;
        let p2 = p + u_out * t;
        replacements.push((
            i,
            [
                Node {
                    r: 0.0,
                    p: p1,
                    h_in: p1,
                    h_out: p1 + u_in * hl,
                },
                Node {
                    r: 0.0,
                    p: p2,
                    h_in: p2 - u_out * hl,
                    h_out: p2,
                },
            ],
        ));
    }
    for (i, nodes) in replacements.iter().rev() {
        out.nodes.splice(*i..*i + 1, nodes.iter().copied());
    }
    // Neue Indizes: jede vorherige Ersetzung verschiebt um eins.
    let mut new_idx = Vec::new();
    for (k, (i, _)) in replacements.iter().enumerate() {
        new_idx.push(i + k);
        new_idx.push(i + k + 1);
    }
    (out, new_idx)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub fill: Option<Color>,
    pub stroke: Option<Color>,
    pub width: f64,
    /// Deckkraft des ganzen Objekts (0–1).
    pub opacity: f64,
    /// Farbverlauf der Füllung (ersetzt die Füllfarbe, solange eine Füllung aktiv ist).
    pub gradient: Option<Gradient>,
    /// Wie sich das Objekt mit darunterliegenden Objekten mischt.
    pub blend: Blend,
    /// Linienende (Pfeil …) bei offenen Pfaden und Linien.
    pub marker: Marker,
    pub marker_ends: MarkerEnds,
    /// Größe der Linienenden (1 = 100 %).
    pub marker_size: f64,
    /// Ecken der Kontur: spitz, rund oder abgeschrägt
    pub join: LineJoin,
    /// Enden offener Konturen: gerade, rund oder überstehend
    pub cap: LineCap,
    /// Schlagschatten (None = aus)
    pub shadow: Option<Shadow>,
    /// Farbverlauf der Kontur (solange eine Kontur aktiv ist)
    pub stroke_gradient: Option<Gradient>,
    /// Farbverlauf des Schattens (solange ein Schatten aktiv ist)
    pub shadow_gradient: Option<Gradient>,
    /// Kontur mittig, innen oder außen
    pub stroke_align: StrokeAlign,
    /// Strichmuster der Kontur (durchgezogen, gestrichelt …)
    pub dash: Dash,
    /// Effekte (Milchglas, Kunsteffekt, 3D-Effekt, Spiegelung)
    pub fx: Effects,
    /// Globale Farben: Kennung der Bibliotheksfarbe, aus der Füllung bzw. Kontur stammen.
    /// Ändert man die Farbe in der Bibliothek, ändern sich alle so verknüpften Objekte mit.
    pub fill_link: Option<ColorLink>,
    pub stroke_link: Option<ColorLink>,
}

/// Wofür eine Farbe gilt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PaintTarget {
    #[default]
    Fill,
    Stroke,
    Shadow,
}

impl Style {
    /// Grundfarbe für Füllung, Kontur oder Schatten (None = keine).
    pub fn paint(&self, t: PaintTarget) -> Option<Color> {
        match t {
            PaintTarget::Fill => self.fill,
            PaintTarget::Stroke => self.stroke,
            // die Deckkraft des Schattens zählt als Deckkraft seiner Farbe
            PaintTarget::Shadow => self.shadow.map(|s| Color {
                a: s.opacity,
                ..s.color
            }),
        }
    }

    /// Wirksamer Verlauf (nur, solange die Farbe aktiv ist).
    pub fn grad(&self, t: PaintTarget) -> Option<&Gradient> {
        self.paint(t)?;
        match t {
            PaintTarget::Fill => self.gradient.as_ref(),
            PaintTarget::Stroke => self.stroke_gradient.as_ref(),
            PaintTarget::Shadow => self.shadow_gradient.as_ref(),
        }
    }

    pub fn grad_slot(&mut self, t: PaintTarget) -> &mut Option<Gradient> {
        match t {
            PaintTarget::Fill => &mut self.gradient,
            PaintTarget::Stroke => &mut self.stroke_gradient,
            PaintTarget::Shadow => &mut self.shadow_gradient,
        }
    }

    /// Nur die Deckkraft einer Farbe ändern (Farbton bleibt, auch bei Verläufen).
    pub fn set_paint_alpha(&mut self, t: PaintTarget, a: f64) {
        let Some(c) = self.paint(t) else { return };
        let g = self.grad(t).cloned().map(|mut g| {
            for st in g.stops.iter_mut() {
                st.color.a = a;
            }
            g
        });
        // nur die Deckkraft: eine Verknüpfung mit der Bibliothek bleibt
        let links = (self.fill_link.clone(), self.stroke_link.clone());
        self.set_paint(t, Some(Color { a, ..c }), g);
        (self.fill_link, self.stroke_link) = links;
    }

    /// Deckkraft des ganzen Objekts in die Farben übernehmen (die Seitenleiste zeigt nur
    /// noch die Deckkraft der Farben). Danach ist `opacity` = 1.
    pub fn fold_opacity(&mut self) {
        let op = self.opacity.clamp(0.0, 1.0);
        self.opacity = 1.0;
        if op >= 0.999 {
            return;
        }
        for c in [self.fill.as_mut(), self.stroke.as_mut()].into_iter().flatten() {
            c.a *= op;
        }
        for g in [
            self.gradient.as_mut(),
            self.stroke_gradient.as_mut(),
            self.shadow_gradient.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            for st in g.stops.iter_mut() {
                st.color.a *= op;
            }
        }
        if let Some(sh) = self.shadow.as_mut() {
            sh.opacity *= op;
        }
    }

    /// Farbe (und Verlauf) setzen; `None` schaltet Füllung, Kontur bzw. Schatten ab.
    pub fn set_paint(&mut self, t: PaintTarget, c: Option<Color>, g: Option<Gradient>) {
        // eine von Hand gewählte Farbe ist nicht mehr mit der Bibliothek verknüpft
        match t {
            PaintTarget::Fill => self.fill_link = None,
            PaintTarget::Stroke => self.stroke_link = None,
            PaintTarget::Shadow => {}
        }
        match t {
            PaintTarget::Fill => self.fill = c,
            PaintTarget::Stroke => self.stroke = c,
            PaintTarget::Shadow => {
                self.shadow = c.map(|color| Shadow {
                    color: Color { a: 1.0, ..color },
                    opacity: color.a,
                    ..self.shadow.unwrap_or_default()
                })
            }
        }
        *self.grad_slot(t) = if c.is_some() { g } else { None };
    }
}

/// Schlagschatten eines Objekts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub color: Color,
    /// Deckkraft des Schattens (0–1)
    pub opacity: f64,
    /// Unschärfe (Radius in Dokumenteinheiten)
    pub blur: f64,
    /// Versatz (Dokumenteinheiten)
    pub dx: f64,
    pub dy: f64,
}

impl Default for Shadow {
    fn default() -> Self {
        Shadow {
            color: Color::BLACK,
            opacity: 0.35,
            blur: 8.0,
            dx: 4.0,
            dy: 6.0,
        }
    }
}

impl Shadow {
    /// Als Text „dx dy Unschärfe Deckkraft #Farbe“ (eigenes SVG-Attribut).
    pub fn to_attr(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.dx,
            self.dy,
            self.blur,
            self.opacity,
            self.color.to_hex()
        )
    }
    pub fn from_attr(s: &str) -> Option<Shadow> {
        let mut it = s.split_whitespace();
        let dx = it.next()?.parse().ok()?;
        let dy = it.next()?.parse().ok()?;
        let blur: f64 = it.next()?.parse().ok()?;
        let opacity: f64 = it.next()?.parse().ok()?;
        let color = Color::parse(it.next()?)?;
        Some(Shadow {
            color,
            opacity: opacity.clamp(0.0, 1.0),
            blur: blur.max(0.0),
            dx,
            dy,
        })
    }
}

/// Form der Konturecken.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LineJoin {
    #[default]
    Miter,
    Round,
    Bevel,
}

impl LineJoin {
    pub const ALL: [LineJoin; 3] = [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel];
    pub fn css(self) -> &'static str {
        match self {
            LineJoin::Miter => "miter",
            LineJoin::Round => "round",
            LineJoin::Bevel => "bevel",
        }
    }
    pub fn from_css(s: &str) -> Option<LineJoin> {
        LineJoin::ALL.into_iter().find(|j| j.css() == s.trim())
    }
    pub fn label(self) -> &'static str {
        match self {
            LineJoin::Miter => tr("Mitred corners"),
            LineJoin::Round => tr("Round corners"),
            LineJoin::Bevel => tr("Bevelled corners"),
        }
    }
}

/// Form der Linienenden.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LineCap {
    #[default]
    Butt,
    Round,
    Square,
}

impl LineCap {
    pub const ALL: [LineCap; 3] = [LineCap::Butt, LineCap::Round, LineCap::Square];
    pub fn css(self) -> &'static str {
        match self {
            LineCap::Butt => "butt",
            LineCap::Round => "round",
            LineCap::Square => "square",
        }
    }
    pub fn from_css(s: &str) -> Option<LineCap> {
        LineCap::ALL.into_iter().find(|c| c.css() == s.trim())
    }
    pub fn label(self) -> &'static str {
        match self {
            LineCap::Butt => tr("Butt ends"),
            LineCap::Round => tr("Round ends"),
            LineCap::Square => tr("Square ends"),
        }
    }
}

/// Lage der Kontur zur Umrisslinie.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StrokeAlign {
    /// mittig auf der Linie
    #[default]
    Center,
    /// innen
    Inside,
    /// außen
    Outside,
}

impl StrokeAlign {
    pub const ALL: [StrokeAlign; 3] = [StrokeAlign::Center, StrokeAlign::Inside, StrokeAlign::Outside];
    pub fn name(self) -> &'static str {
        match self {
            StrokeAlign::Center => "center",
            StrokeAlign::Inside => "inside",
            StrokeAlign::Outside => "outside",
        }
    }
    pub fn from_name(s: &str) -> Option<StrokeAlign> {
        StrokeAlign::ALL.into_iter().find(|a| a.name() == s.trim())
    }
}

impl Default for Style {
    fn default() -> Self {
        Style {
            fill: Some(Color::rgb(0.6, 0.757, 0.945)),
            stroke: Some(Color::rgb(0.11, 0.11, 0.11)),
            width: 2.0,
            opacity: 1.0,
            gradient: None,
            blend: Blend::Normal,
            marker: Marker::None,
            marker_ends: MarkerEnds::End,
            marker_size: 1.0,
            join: LineJoin::Miter,
            cap: LineCap::Butt,
            shadow: None,
            stroke_gradient: None,
            shadow_gradient: None,
            stroke_align: StrokeAlign::Center,
            dash: Dash::Solid,
            fx: Effects::default(),
            fill_link: None,
            stroke_link: None,
        }
    }
}

/// Strichmuster einer Kontur; die Längen richten sich nach der Konturstärke.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Dash {
    #[default]
    Solid,
    Dashed,
    Dotted,
    DashDot,
    LongDash,
}

impl Dash {
    pub const ALL: [Dash; 5] = [
        Dash::Solid,
        Dash::Dashed,
        Dash::Dotted,
        Dash::DashDot,
        Dash::LongDash,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Dash::Solid => "solid",
            Dash::Dashed => "dashed",
            Dash::Dotted => "dotted",
            Dash::DashDot => "dash-dot",
            Dash::LongDash => "long-dash",
        }
    }
    pub fn from_name(s: &str) -> Option<Dash> {
        Dash::ALL.into_iter().find(|d| d.name() == s.trim())
    }
    pub fn label(self) -> &'static str {
        match self {
            Dash::Solid => tr("Solid"),
            Dash::Dashed => tr("Dashed"),
            Dash::Dotted => tr("Dotted"),
            Dash::DashDot => tr("Dash-dot"),
            Dash::LongDash => tr("Long dashes"),
        }
    }
    /// Strich- und Lückenlängen bei Konturstärke `w` (leer: durchgezogen).
    pub fn pattern(self, w: f64) -> Vec<f64> {
        let w = w.max(0.5);
        match self {
            Dash::Solid => Vec::new(),
            Dash::Dashed => vec![3.0 * w, 2.0 * w],
            // Punkte: Strichlänge 0 mit runden Enden
            Dash::Dotted => vec![0.0, 2.0 * w],
            Dash::DashDot => vec![4.0 * w, 2.0 * w, 0.0, 2.0 * w],
            Dash::LongDash => vec![8.0 * w, 3.0 * w],
        }
    }
    /// Punkte brauchen runde Enden, sonst sind sie unsichtbar.
    pub fn round_caps(self) -> bool {
        matches!(self, Dash::Dotted | Dash::DashDot)
    }
    /// Muster aus `stroke-dasharray` (fremde SVG-Dateien) möglichst gut zuordnen.
    pub fn from_array(v: &[f64], w: f64) -> Dash {
        let w = w.max(0.5);
        match v {
            [] => Dash::Solid,
            [a, ..] if v.iter().all(|x| *x <= 0.0) && *a <= 0.0 => Dash::Solid,
            [a, b] if *a <= 0.25 * w || *a <= 0.5 * b.min(w) => Dash::Dotted,
            [a, _] if *a >= 6.0 * w => Dash::LongDash,
            [_, _] => Dash::Dashed,
            [_] => Dash::Dashed,
            _ if v.iter().any(|x| *x <= 0.25 * w) => Dash::DashDot,
            _ => Dash::Dashed,
        }
    }
}

// ───────────────────────────── Linienenden ─────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Marker {
    #[default]
    None,
    Arrow,
    OpenArrow,
    Dot,
    Bar,
}

impl Marker {
    pub const ALL: [Marker; 5] = [
        Marker::None,
        Marker::Arrow,
        Marker::OpenArrow,
        Marker::Dot,
        Marker::Bar,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Marker::None => tr("None"),
            Marker::Arrow => tr("Arrow"),
            Marker::OpenArrow => tr("Open arrow"),
            Marker::Dot => tr("Dot"),
            Marker::Bar => tr("Bar"),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Marker::None => "none",
            Marker::Arrow => "arrow",
            Marker::OpenArrow => "open-arrow",
            Marker::Dot => "dot",
            Marker::Bar => "bar",
        }
    }
    pub fn from_name(s: &str) -> Option<Marker> {
        Marker::ALL.into_iter().find(|m| m.name() == s)
    }
    /// Länge der Spitze bei Linienstärke `w`.
    pub fn length(self, w: f64, size: f64) -> f64 {
        (w.max(0.5) * 4.0 + 6.0) * size.clamp(0.1, 20.0)
    }
    /// Um so viel wird die Linie gekürzt, damit sie nicht durch die Spitze ragt.
    pub fn trim(self, w: f64, size: f64) -> f64 {
        match self {
            Marker::Arrow => self.length(w, size) * 0.8,
            _ => 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MarkerEnds {
    Start,
    #[default]
    End,
    Both,
}

impl MarkerEnds {
    pub const ALL: [MarkerEnds; 3] = [MarkerEnds::Start, MarkerEnds::End, MarkerEnds::Both];
    pub fn name(self) -> &'static str {
        match self {
            MarkerEnds::Start => "start",
            MarkerEnds::End => "end",
            MarkerEnds::Both => "both",
        }
    }
    pub fn from_name(s: &str) -> Option<MarkerEnds> {
        MarkerEnds::ALL.into_iter().find(|m| m.name() == s)
    }
    pub fn start(self) -> bool {
        matches!(self, MarkerEnds::Start | MarkerEnds::Both)
    }
    pub fn end(self) -> bool {
        matches!(self, MarkerEnds::End | MarkerEnds::Both)
    }
}

fn cubic_len(p0: Pt, c1: Pt, c2: Pt, p3: Pt, t0: f64, t1: f64) -> f64 {
    let steps = 24;
    let mut len = 0.0;
    let mut prev = crate::geom::cubic_point(p0, c1, c2, p3, t0);
    for i in 1..=steps {
        let t = t0 + (t1 - t0) * i as f64 / steps as f64;
        let q = crate::geom::cubic_point(p0, c1, c2, p3, t);
        len += prev.dist(q);
        prev = q;
    }
    len
}

/// Kurvenparameter, bei dem vom Anfang (bzw. vom Ende) aus die Länge `d` erreicht ist.
fn cubic_t_at(p0: Pt, c1: Pt, c2: Pt, p3: Pt, d: f64, from_end: bool) -> f64 {
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..30 {
        let mid = (lo + hi) / 2.0;
        let l = if from_end {
            cubic_len(p0, c1, c2, p3, mid, 1.0)
        } else {
            cubic_len(p0, c1, c2, p3, 0.0, mid)
        };
        // vom Ende: Länge fällt mit wachsendem t
        if (l < d) != from_end {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) / 2.0
}

/// De-Casteljau-Teilung: (linker Teil, rechter Teil).
fn split_cubic(p0: Pt, c1: Pt, c2: Pt, p3: Pt, t: f64) -> ([Pt; 4], [Pt; 4]) {
    let a = p0.lerp(c1, t);
    let b = c1.lerp(c2, t);
    let c = c2.lerp(p3, t);
    let ab = a.lerp(b, t);
    let bc = b.lerp(c, t);
    let m = ab.lerp(bc, t);
    ([p0, a, ab, m], [m, bc, c, p3])
}

/// Offenen Teilpfad an Anfang und Ende um die Längen `start` bzw. `end` kürzen.
pub fn trim_subpath(sp: &SubPath, start: f64, end: f64) -> SubPath {
    let mut out = sp.clone();
    let n = out.nodes.len();
    if sp.closed || n < 2 {
        return out;
    }
    if end > 0.0 {
        let (a, b) = (out.nodes[n - 2], out.nodes[n - 1]);
        let len = cubic_len(a.p, a.h_out, b.h_in, b.p, 0.0, 1.0);
        if len > end * 1.2 {
            let t = cubic_t_at(a.p, a.h_out, b.h_in, b.p, end, true);
            let (l, _) = split_cubic(a.p, a.h_out, b.h_in, b.p, t);
            out.nodes[n - 2].h_out = l[1];
            out.nodes[n - 1].h_in = l[2];
            out.nodes[n - 1].p = l[3];
            out.nodes[n - 1].h_out = l[3];
        }
    }
    if start > 0.0 {
        let (a, b) = (out.nodes[0], out.nodes[1]);
        let len = cubic_len(a.p, a.h_out, b.h_in, b.p, 0.0, 1.0);
        if len > start * 1.2 {
            let t = cubic_t_at(a.p, a.h_out, b.h_in, b.p, start, false);
            let (_, r) = split_cubic(a.p, a.h_out, b.h_in, b.p, t);
            out.nodes[0].p = r[0];
            out.nodes[0].h_in = r[0];
            out.nodes[0].h_out = r[1];
            out.nodes[1].h_in = r[2];
        }
    }
    out
}

/// Enden eines offenen Teilpfades: ((Anfang, Richtung nach außen), (Ende, Richtung nach außen)).
pub fn subpath_ends(sp: &SubPath) -> Option<((Pt, Pt), (Pt, Pt))> {
    let n = sp.nodes.len();
    if sp.closed || n < 2 {
        return None;
    }
    let dir = |from: Pt, cands: [Pt; 3]| -> Pt {
        for c in cands {
            let d = from - c;
            if d.len() > 1e-9 {
                return d.normalized();
            }
        }
        Pt::new(1.0, 0.0)
    };
    let (a, b) = (sp.nodes[0], sp.nodes[1]);
    let start = (a.p, dir(a.p, [a.h_out, b.h_in, b.p]));
    let (y, z) = (sp.nodes[n - 2], sp.nodes[n - 1]);
    let end = (z.p, dir(z.p, [z.h_in, y.h_out, y.p]));
    Some((start, end))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    pub id: u64,
    pub shape: Shape,
    pub style: Style,
    /// Drehwinkel in Grad (gegen den Uhrzeigersinn), wird bei Drehungen mitgeführt.
    pub angle: f64,
    /// Gruppe (0 = keine); alle Objekte mit derselben Nummer werden gemeinsam ausgewählt.
    /// Das ist immer die äußerste Gruppe.
    pub group: u64,
    /// Gruppen innerhalb von `group`, von außen nach innen (leer = direkt in `group`).
    /// Gruppieren legt eine Ebene außen herum, Aufheben nimmt die äußerste weg.
    pub sub: Vec<u64>,
    /// Schnittmaske: das Objekt wird nicht gezeichnet; seine Form begrenzt alle anderen
    /// Objekte seiner innersten Gruppe.
    pub mask: bool,
    /// Gesperrt: auswählbar, aber nicht verschieb-, skalier- oder löschbar.
    pub locked: bool,
    /// Optionaler Name (in SVG als inkscape:label)
    pub name: String,
}

impl Object {
    /// Alle Gruppen des Objekts von außen nach innen (leer = in keiner Gruppe).
    pub fn chain(&self) -> Vec<u64> {
        if self.group == 0 {
            Vec::new()
        } else {
            std::iter::once(self.group)
                .chain(self.sub.iter().copied())
                .collect()
        }
    }

    /// Innerste Gruppe (0 = keine).
    pub fn inner_group(&self) -> u64 {
        self.sub.last().copied().unwrap_or(self.group)
    }

    /// Fläche einer Schnittmaske: die Form gefüllt, ohne Kontur und Effekte.
    pub fn clip_area(&self) -> Object {
        let mut o = self.clone();
        o.style = Style {
            fill: Some(Color::BLACK),
            stroke: None,
            ..Style::default()
        };
        o.mask = false;
        o
    }
}

/// Neue Gruppennummern für eine Kopie (auf allen Ebenen; dieselbe alte Nummer bekommt in
/// einem Durchgang dieselbe neue). `next` ist die zuletzt vergebene Nummer.
pub fn regroup_ids(o: &mut Object, map: &mut Vec<(u64, u64)>, next: &mut u64) {
    let mut renew = |g: u64| -> u64 {
        if g == 0 {
            return 0;
        }
        match map.iter().find(|(old, _)| *old == g) {
            Some((_, n)) => *n,
            None => {
                *next += 1;
                map.push((g, *next));
                *next
            }
        }
    };
    o.group = renew(o.group);
    for g in o.sub.iter_mut() {
        *g = renew(*g);
    }
}

/// Schnittmasken je Gruppe: innerste Gruppe der Maske → Maske (als Fläche).
pub fn masks_of<'a>(objs: impl Iterator<Item = &'a Object>) -> HashMap<u64, Object> {
    let mut m = HashMap::new();
    for o in objs.filter(|o| o.mask && o.group != 0) {
        m.entry(o.inner_group()).or_insert_with(|| o.clip_area());
    }
    m
}

/// Die Masken, die ein Objekt begrenzen (von außen nach innen).
pub fn clips_of<'a>(o: &Object, masks: &'a HashMap<u64, Object>) -> Vec<&'a Object> {
    if o.mask || masks.is_empty() {
        return Vec::new();
    }
    o.chain().iter().filter_map(|g| masks.get(g)).collect()
}

/// Winkel auf den Bereich (−180°, 180°] bringen.
pub fn normalize_angle(a: f64) -> f64 {
    let mut a = a % 360.0;
    if a <= -180.0 {
        a += 360.0;
    } else if a > 180.0 {
        a -= 360.0;
    }
    if a.abs() < 1e-9 {
        0.0
    } else {
        a
    }
}

/// Rahmen eines Textfeldes: eigene Form (lokale Koordinaten 0…w × 0…h, None = Rechteck),
/// Füllung und Kontur des Rahmens sowie Abstand des Textes zum Rand.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TextFrame {
    pub path: Option<Vec<SubPath>>,
    pub fill: Option<Color>,
    pub stroke: Option<Color>,
    pub width: f64,
    pub inset: f64,
}

/// Welcher Teil eines gerahmten Bildes bearbeitet wird.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ImagePart {
    /// Rahmen und Bild gemeinsam
    #[default]
    All,
    /// Nur der Rahmen (Beschnitt)
    Frame,
    /// Nur das Bild im Rahmen
    Content,
}

// ───────────────────────────── Text: Absatzformat ─────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

impl Align {
    pub const ALL: [Align; 4] = [Align::Left, Align::Center, Align::Right, Align::Justify];
    pub fn name(self) -> &'static str {
        match self {
            Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
            Align::Justify => "justify",
        }
    }
    pub fn from_name(s: &str) -> Option<Align> {
        Align::ALL.into_iter().find(|a| a.name() == s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VAlign {
    #[default]
    Top,
    Middle,
    Bottom,
}

impl VAlign {
    pub const ALL: [VAlign; 3] = [VAlign::Top, VAlign::Middle, VAlign::Bottom];
    pub fn name(self) -> &'static str {
        match self {
            VAlign::Top => "top",
            VAlign::Middle => "middle",
            VAlign::Bottom => "bottom",
        }
    }
    pub fn from_name(s: &str) -> Option<VAlign> {
        VAlign::ALL.into_iter().find(|a| a.name() == s)
    }
}

/// Absatzformat: Ausrichtung, Zeilenabstand (Vielfaches der Schriftgröße) und
/// Zeichenabstand (in 1/1000 Geviert).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextFormat {
    pub align: Align,
    pub valign: VAlign,
    pub line: f64,
    pub tracking: f64,
    /// unterstrichen
    pub underline: bool,
    /// durchgestrichen
    pub strike: bool,
    /// Aufzählung (Textfelder)
    pub list: ListStyle,
    /// Text auf Pfad: unter der Linie statt darauf
    pub path_below: bool,
    /// Text auf Pfad: in umgekehrter Richtung (z. B. lesbar unten am Kreis)
    pub path_reverse: bool,
    /// Abstand zwischen Absätzen (Vielfaches der Schriftgröße, Textfelder)
    pub para: f64,
    /// Spaltenzahl (Textfelder; 1 = eine Spalte)
    pub columns: u32,
    /// Kapitälchen (OpenType `smcp`, sonst von Pango nachgebildet)
    pub small_caps: bool,
    /// Standardligaturen (fi, fl …) abschalten
    pub no_ligatures: bool,
    /// Kasten hinter Punkttext und Text auf Pfad (Textfelder haben ihren eigenen Rahmen):
    /// Füllung, Kontur, Linienstärke und Abstand zum Text
    pub box_fill: Option<Color>,
    pub box_stroke: Option<Color>,
    pub box_width: f64,
    pub box_inset: f64,
    /// Strichstärke 1–1000 (400 normal, 700 fett); 0 = aus „fett“ abgeleitet
    pub weight: u16,
}

/// Höchste Spaltenzahl eines Textfeldes.
pub const MAX_COLUMNS: u32 = 8;

impl TextFormat {
    /// Wirksame Strichstärke bei Grundschnitt fett/normal.
    pub fn weight_for(&self, bold: bool) -> u16 {
        effective_weight(self.weight, bold)
    }

    /// Kasten hinter dem Text als Rahmen (Punkttext, Text auf Pfad).
    pub fn text_box(&self) -> TextFrame {
        TextFrame {
            path: None,
            fill: self.box_fill,
            stroke: self.box_stroke,
            width: self.box_width,
            inset: self.box_inset,
        }
    }

    /// Kasten aus einem Rahmen übernehmen.
    pub fn set_text_box(&mut self, f: &TextFrame) {
        self.box_fill = f.fill;
        self.box_stroke = f.stroke;
        self.box_width = f.width;
        self.box_inset = f.inset.max(0.0);
    }

    /// Hat der Text einen sichtbaren Kasten?
    pub fn has_box(&self) -> bool {
        self.box_fill.is_some() || (self.box_stroke.is_some() && self.box_width > 0.0)
    }

    /// Wirksame Spaltenzahl (1 bis `MAX_COLUMNS`).
    pub fn cols(&self) -> usize {
        self.columns.clamp(1, MAX_COLUMNS) as usize
    }
}

impl Default for TextFormat {
    fn default() -> Self {
        TextFormat {
            align: Align::Left,
            valign: VAlign::Top,
            line: 1.2,
            tracking: 0.0,
            underline: false,
            strike: false,
            list: ListStyle::None,
            path_below: false,
            path_reverse: false,
            para: 0.5,
            columns: 1,
            small_caps: false,
            no_ligatures: false,
            box_fill: None,
            box_stroke: None,
            box_width: 1.0,
            box_inset: 0.0,
            weight: 0,
        }
    }
}

/// Art der Aufzählung.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ListStyle {
    #[default]
    None,
    Dot,
    Square,
    Dash,
    Number,
}

impl ListStyle {
    pub const ALL: [ListStyle; 4] = [
        ListStyle::Dot,
        ListStyle::Square,
        ListStyle::Dash,
        ListStyle::Number,
    ];
    pub fn name(self) -> &'static str {
        match self {
            ListStyle::None => "none",
            ListStyle::Dot => "dot",
            ListStyle::Square => "square",
            ListStyle::Dash => "dash",
            ListStyle::Number => "number",
        }
    }
    pub fn from_name(s: &str) -> Option<ListStyle> {
        [
            ListStyle::None,
            ListStyle::Dot,
            ListStyle::Square,
            ListStyle::Dash,
            ListStyle::Number,
        ]
        .into_iter()
        .find(|l| l.name() == s.trim())
    }
    pub fn label(self) -> &'static str {
        match self {
            ListStyle::None => tr("None"),
            ListStyle::Dot => tr("• Bullets"),
            ListStyle::Square => tr("▪ Squares"),
            ListStyle::Dash => tr("– Dashes"),
            ListStyle::Number => tr("1. Numbered"),
        }
    }
    /// Aufzählungszeichen für Absatz `i` (ab 0).
    pub fn marker(self, i: usize) -> String {
        match self {
            ListStyle::None => String::new(),
            ListStyle::Dot => "•".into(),
            ListStyle::Square => "▪".into(),
            ListStyle::Dash => "–".into(),
            ListStyle::Number => format!("{}.", i + 1),
        }
    }
}

/// Index im Original → Index im angezeigten Text.
pub fn list_to_display(map: &[(usize, usize, usize)], idx: usize) -> usize {
    let shift = map
        .iter()
        .rev()
        .find(|(s, _, _)| *s <= idx)
        .map(|(_, sh, _)| *sh)
        .unwrap_or(0);
    idx + shift
}

/// Index im angezeigten Text → Index im Original (im Aufzählungszeichen: Absatzanfang).
pub fn list_from_display(map: &[(usize, usize, usize)], d: usize) -> usize {
    let mut best = d;
    for &(s, sh, plen) in map {
        let para_display = s + sh - plen;
        if para_display > d {
            break;
        }
        best = if d < s + sh { s } else { d - sh };
    }
    best
}

/// Angezeigter Text eines einzelnen Absatzes mit Aufzählungszeichen `marker` davor
/// (Zuordnung wie bei `list_display`).
pub fn para_display(text: &str, marker: &str) -> (String, Vec<(usize, usize, usize)>) {
    if marker.is_empty() {
        return (text.to_string(), Vec::new());
    }
    let prefix = format!("{marker}\t");
    let n = prefix.len();
    (prefix + text, vec![(0, n, n)])
}

// ───────────────────────────── Formatierter Text ─────────────────────────────

/// Wirksames Zeichenformat an einer Textstelle.
#[derive(Clone, Debug, PartialEq)]
pub struct CharFmt {
    pub family: String,
    pub size: f64,
    pub bold: bool,
    /// Strichstärke 1–1000 (400 normal, 700 fett)
    pub weight: u16,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

/// Wirksame Strichstärke: angegeben oder aus fett/normal (700/400).
pub fn effective_weight(weight: u16, bold: bool) -> u16 {
    if weight > 0 {
        weight.clamp(1, 1000)
    } else if bold {
        700
    } else {
        400
    }
}

/// Lesbarer Name einer Strichstärke.
pub fn weight_name(w: u16) -> &'static str {
    match w {
        0..=149 => "Thin",
        150..=249 => "Extra Light",
        250..=349 => "Light",
        350..=449 => "Regular",
        450..=549 => "Medium",
        550..=649 => "Semi Bold",
        650..=749 => "Bold",
        750..=849 => "Extra Bold",
        _ => "Black",
    }
}

impl CharFmt {
    /// Änderung anwenden und „fett“ und Strichstärke stimmig halten: wird nur „fett“
    /// umgeschaltet, folgt die Stärke (700/400); wird die Stärke gesetzt, folgt „fett“.
    pub fn change(&mut self, f: &dyn Fn(&mut CharFmt)) {
        let (bold, weight) = (self.bold, self.weight);
        f(self);
        if self.weight != weight {
            self.weight = self.weight.clamp(1, 1000);
            self.bold = self.weight >= 600;
        } else if self.bold != bold {
            self.weight = effective_weight(0, self.bold);
        }
    }
}

/// Abweichungen eines Textteils vom Grundformat des Objekts.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RunStyle {
    pub family: Option<String>,
    pub size: Option<f64>,
    pub bold: Option<bool>,
    /// Strichstärke (überstimmt `bold`)
    pub weight: Option<u16>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
}

impl RunStyle {
    pub fn is_empty(&self) -> bool {
        *self == RunStyle::default()
    }

    /// Wirksames Format auf Grundlage von `base`.
    pub fn resolve(&self, base: &CharFmt) -> CharFmt {
        CharFmt {
            family: self.family.clone().unwrap_or_else(|| base.family.clone()),
            size: self.size.unwrap_or(base.size),
            bold: self.bold.unwrap_or(base.bold),
            weight: match (self.weight, self.bold) {
                (Some(w), _) => w,
                (None, Some(b)) => effective_weight(0, b),
                (None, None) => base.weight,
            },
            italic: self.italic.unwrap_or(base.italic),
            underline: self.underline.unwrap_or(base.underline),
            strike: self.strike.unwrap_or(base.strike),
        }
    }

    /// Nur was von `base` abweicht.
    pub fn diff(eff: &CharFmt, base: &CharFmt) -> RunStyle {
        RunStyle {
            family: (eff.family != base.family).then(|| eff.family.clone()),
            size: ((eff.size - base.size).abs() > 1e-9).then_some(eff.size),
            bold: (eff.bold != base.bold).then_some(eff.bold),
            weight: (eff.weight != base.weight).then_some(eff.weight),
            italic: (eff.italic != base.italic).then_some(eff.italic),
            underline: (eff.underline != base.underline).then_some(eff.underline),
            strike: (eff.strike != base.strike).then_some(eff.strike),
        }
    }
}

/// Formatierter Textteil (Byte-Bereich `start..end`).
#[derive(Clone, Debug, PartialEq)]
pub struct TextRun {
    pub start: usize,
    pub end: usize,
    pub style: RunStyle,
}

/// Absatzformat eines einzelnen Absatzes.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct ParaFmt {
    pub list: ListStyle,
    /// Einzugsstufe
    pub indent: u8,
}

/// Höchste Einzugsstufe.
pub const MAX_INDENT: u8 = 8;

/// Welche Zeichenformate in einem Bereich uneinheitlich sind.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct FmtMixed {
    pub family: bool,
    pub size: bool,
    pub bold: bool,
    pub weight: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

/// Formatierung von Textteilen und Absätzen eines Textfeldes.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RichText {
    pub runs: Vec<TextRun>,
    /// je Absatz; fehlende Einträge: Aufzählung des Objekts, kein Einzug
    pub paras: Vec<ParaFmt>,
}

/// Gemeinsamer Anfang (in Bytes, an Zeichengrenzen).
fn common_prefix(a: &str, b: &str) -> usize {
    a.char_indices()
        .zip(b.chars())
        .find(|((_, x), y)| x != y)
        .map(|((i, _), _)| i)
        .unwrap_or_else(|| a.len().min(b.len()))
}

/// Gemeinsames Ende (in Bytes, an Zeichengrenzen).
fn common_suffix(a: &str, b: &str) -> usize {
    let mut n = 0;
    for (x, y) in a.chars().rev().zip(b.chars().rev()) {
        if x != y {
            break;
        }
        n += x.len_utf8();
    }
    n
}

impl RichText {
    pub fn is_plain(&self) -> bool {
        self.runs.is_empty() && self.paras.is_empty()
    }

    /// Abschnitte gleichen Formats, die den ganzen Text (Länge `len`) abdecken.
    pub fn segments(&self, len: usize, base: &CharFmt) -> Vec<(usize, usize, CharFmt)> {
        let mut out = Vec::new();
        let mut pos = 0;
        for r in &self.runs {
            let (a, b) = (r.start.min(len), r.end.min(len));
            if a > pos {
                out.push((pos, a, base.clone()));
            }
            if b > a.max(pos) {
                out.push((a.max(pos), b, r.style.resolve(base)));
            }
            pos = pos.max(b);
        }
        if pos < len || out.is_empty() {
            out.push((pos, len, base.clone()));
        }
        out
    }

    /// Textteile aus Abschnitten neu bilden (gleiche Nachbarn zusammengefasst).
    fn set_segments(&mut self, segs: &[(usize, usize, CharFmt)], base: &CharFmt) {
        let mut runs: Vec<TextRun> = Vec::new();
        for (a, b, f) in segs {
            if b <= a {
                continue;
            }
            let st = RunStyle::diff(f, base);
            if st.is_empty() {
                continue;
            }
            match runs.last_mut() {
                Some(l) if l.end == *a && l.style == st => l.end = *b,
                _ => runs.push(TextRun {
                    start: *a,
                    end: *b,
                    style: st,
                }),
            }
        }
        self.runs = runs;
    }

    /// Zeichenformat ändern: im Bereich `range` oder (None) im ganzen Text – dann ändert
    /// sich das Grundformat, das zurückgegeben wird.
    pub fn apply_chars(
        &mut self,
        len: usize,
        base: &CharFmt,
        range: Option<(usize, usize)>,
        f: &dyn Fn(&mut CharFmt),
    ) -> CharFmt {
        let f = |c: &mut CharFmt| c.change(f);
        let mut segs = self.segments(len, base);
        let new_base = match range {
            None => {
                for s in segs.iter_mut() {
                    f(&mut s.2);
                }
                let mut nb = base.clone();
                f(&mut nb);
                nb
            }
            Some((a, b)) => {
                let mut out = Vec::new();
                for (s, e, fm) in segs.drain(..) {
                    // in bis zu drei Stücke teilen
                    let cuts = [s, a.clamp(s, e), b.clamp(s, e), e];
                    for w in cuts.windows(2) {
                        if w[1] > w[0] {
                            let mut g = fm.clone();
                            if w[0] >= a && w[1] <= b {
                                f(&mut g);
                            }
                            out.push((w[0], w[1], g));
                        }
                    }
                }
                segs = out;
                base.clone()
            }
        };
        self.set_segments(&segs, &new_base);
        new_base
    }

    /// Format an einer Stelle bzw. im Bereich und welche Angaben dort uneinheitlich sind.
    pub fn format_in(&self, len: usize, base: &CharFmt, a: usize, b: usize) -> (CharFmt, FmtMixed) {
        let segs = self.segments(len, base);
        let inside: Vec<&CharFmt> = if a == b {
            // Cursor: Format des Zeichens davor (am Anfang: des ersten)
            let at = a.saturating_sub(1);
            segs.iter()
                .find(|(s, e, _)| *s <= at && at < *e)
                .or(segs.first())
                .map(|x| vec![&x.2])
                .unwrap_or_default()
        } else {
            segs.iter()
                .filter(|(s, e, _)| *e > a && *s < b)
                .map(|x| &x.2)
                .collect()
        };
        let Some(first) = inside.first().copied() else {
            return (base.clone(), FmtMixed::default());
        };
        let mixed = FmtMixed {
            family: inside.iter().any(|f| f.family != first.family),
            size: inside.iter().any(|f| (f.size - first.size).abs() > 1e-9),
            bold: inside.iter().any(|f| f.bold != first.bold),
            weight: inside.iter().any(|f| f.weight != first.weight),
            italic: inside.iter().any(|f| f.italic != first.italic),
            underline: inside.iter().any(|f| f.underline != first.underline),
            strike: inside.iter().any(|f| f.strike != first.strike),
        };
        (first.clone(), mixed)
    }

    /// Text wurde von `old` zu `new` geändert: Textteile und Absätze nachführen.
    /// Eingefügter Text übernimmt das Format davor (wie beim Weiterschreiben).
    pub fn edit(&mut self, old: &str, new: &str) {
        if old == new {
            return;
        }
        let p = common_prefix(old, new);
        let s = common_suffix(&old[p..], &new[p..]);
        let old_end = old.len() - s;
        let ins = new.len() - s - p;
        let rem = old_end - p;
        let map_end = |x: usize| x + ins - rem;
        for r in self.runs.iter_mut() {
            let (s0, e0) = (r.start, r.end);
            r.start = if s0 < p || (s0 == 0 && p == 0) {
                s0
            } else if s0 >= old_end && !(s0 == p && rem == 0 && ins > 0) {
                map_end(s0)
            } else {
                p + ins
            };
            r.end = if e0 < p {
                e0
            } else if e0 == p {
                if s0 < p || (s0 == 0 && p == 0) {
                    p + ins
                } else {
                    p
                }
            } else if e0 >= old_end {
                map_end(e0)
            } else {
                p + ins
            };
            if r.end < r.start {
                r.end = r.start;
            }
        }
        self.runs.retain(|r| r.end > r.start);
        // Absätze: entfernte Umbrüche löschen, neue übernehmen das Format des Absatzes
        let pi = old[..p].matches('\n').count();
        let removed = old[p..old_end].matches('\n').count();
        let added = new[p..p + ins].matches('\n').count();
        if pi < self.paras.len() {
            let to = (pi + 1 + removed).min(self.paras.len());
            if pi + 1 < to {
                self.paras.drain(pi + 1..to);
            }
            let cur = self.paras[pi];
            for _ in 0..added {
                self.paras.insert(pi + 1, cur);
            }
        }
    }

    /// Format von Absatz `i`.
    pub fn para(&self, i: usize, default_list: ListStyle) -> ParaFmt {
        self.paras.get(i).copied().unwrap_or(ParaFmt {
            list: default_list,
            indent: 0,
        })
    }

    /// Absätze im Bereich (None = alle) ändern.
    pub fn apply_paras(
        &mut self,
        text: &str,
        range: Option<(usize, usize)>,
        default_list: ListStyle,
        f: &dyn Fn(&mut ParaFmt),
    ) {
        let n = text.matches('\n').count() + 1;
        let (pa, pb) = match range {
            None => (0, n - 1),
            Some((a, b)) => (
                text[..a.min(text.len())].matches('\n').count(),
                text[..b.min(text.len())].matches('\n').count(),
            ),
        };
        while self.paras.len() < n {
            self.paras.push(ParaFmt {
                list: default_list,
                indent: 0,
            });
        }
        self.paras.truncate(n);
        for p in &mut self.paras[pa..=pb.min(n - 1)] {
            f(p);
        }
    }
}

impl TextFormat {
    /// Zusätzlicher Abstand nach jedem Zeichen in Pixeln.
    pub fn tracking_px(&self, size: f64) -> f64 {
        self.tracking / 1000.0 * size
    }
}

// ───────────────────────────── Effekte ─────────────────────────────

/// Kunsteffekt: verändert, wie die Füllung aussieht.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Art {
    Grain,
    Pixel,
    Watercolor,
    /// Wachsmalstifte: Papierstruktur scheint in schrägen Strichen durch
    Crayon,
    /// Ölfarbe: Pinselstriche mit leichtem Relief
    Oil,
    /// Raster: Punkte, deren Größe der Helligkeit folgt (Größe und Winkel einstellbar)
    Halftone,
    /// Risografie: leicht versetzte Farbschichten und Körnung
    Risograph,
}

impl Art {
    pub const ALL: [Art; 7] = [
        Art::Grain,
        Art::Pixel,
        Art::Watercolor,
        Art::Crayon,
        Art::Oil,
        Art::Halftone,
        Art::Risograph,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Art::Grain => "grain",
            Art::Pixel => "pixel",
            Art::Watercolor => "watercolor",
            Art::Crayon => "crayon",
            Art::Oil => "oil",
            Art::Halftone => "halftone",
            Art::Risograph => "riso",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Art::Grain => tr("Grain"),
            Art::Pixel => tr("Pixels"),
            Art::Watercolor => tr("Watercolour"),
            Art::Crayon => tr("Crayons"),
            Art::Oil => tr("Oil paint"),
            Art::Halftone => tr("Halftone"),
            Art::Risograph => tr("Risograph"),
        }
    }
    /// Hat der Effekt eine Größe und einen Winkel (wie ein Muster)?
    pub fn has_pattern(self) -> bool {
        self == Art::Halftone
    }
    pub fn from_name(s: &str) -> Option<Art> {
        Art::ALL.into_iter().find(|a| a.name() == s.trim())
    }
}

/// Einfache Farbfilter, nur für Bilder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImgFilter {
    Sepia,
    Mono,
    Invert,
}

impl ImgFilter {
    pub const ALL: [ImgFilter; 3] = [ImgFilter::Sepia, ImgFilter::Mono, ImgFilter::Invert];
    pub fn name(self) -> &'static str {
        match self {
            ImgFilter::Sepia => "sepia",
            ImgFilter::Mono => "mono",
            ImgFilter::Invert => "invert",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ImgFilter::Sepia => tr("Sepia"),
            ImgFilter::Mono => tr("Black & white"),
            ImgFilter::Invert => tr("Invert colours"),
        }
    }
    pub fn from_name(s: &str) -> Option<ImgFilter> {
        ImgFilter::ALL.into_iter().find(|f| f.name() == s.trim())
    }
    /// Die Filter wirken immer voll.
    pub fn default_strength(self) -> f64 {
        1.0
    }
}

/// 3D-Effekt: Form des Reliefs, das dem Objekt Licht und Schatten gibt und das
/// Darunterliegende wie durch Glas bricht.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lens {
    /// abgeflachte Kugel (wie ein Regentropfen): gerundeter Rand, flache Mitte
    Drop,
    /// komplett rund
    Sphere,
    /// spitz zulaufende Kanten mit flacher Oberfläche
    Crystal,
    /// komplett spitz zulaufend
    Pyramid,
}

impl Lens {
    pub const ALL: [Lens; 4] = [Lens::Drop, Lens::Sphere, Lens::Crystal, Lens::Pyramid];
    pub fn name(self) -> &'static str {
        match self {
            Lens::Drop => "drop",
            Lens::Sphere => "sphere",
            Lens::Crystal => "crystal",
            Lens::Pyramid => "pyramid",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Lens::Drop => tr("Raindrop"),
            Lens::Sphere => tr("Sphere"),
            Lens::Crystal => tr("Crystal"),
            Lens::Pyramid => tr("Pyramid"),
        }
    }
    pub fn from_name(s: &str) -> Option<Lens> {
        Lens::ALL.into_iter().find(|a| a.name() == s.trim())
    }
}

/// Material: wie die Oberfläche eines Objekts wirkt und wie sie das Licht eines 3D-Effekts
/// zurückwirft. Ohne Material ist die Oberfläche matt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Material {
    /// Hochglanz: lackiert oder Kunststoff, kleines helles Glanzlicht
    Gloss,
    /// poliertes Metall: spiegelt Himmel und Boden, Glanz in der Farbe des Objekts
    Metal,
    /// gebürstetes Metall: feine Striche, Glanz entlang der Bürstrichtung
    Brushed,
    /// Samt: dunkel in der Fläche, schimmert an Rändern und Flanken
    Velvet,
}

impl Material {
    pub const ALL: [Material; 4] = [
        Material::Gloss,
        Material::Metal,
        Material::Brushed,
        Material::Velvet,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Material::Gloss => "gloss",
            Material::Metal => "metal",
            Material::Brushed => "brushed",
            Material::Velvet => "velvet",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Material::Gloss => tr("Gloss"),
            Material::Metal => tr("Metal"),
            Material::Brushed => tr("Brushed metal"),
            Material::Velvet => tr("Velvet"),
        }
    }
    pub fn from_name(s: &str) -> Option<Material> {
        Material::ALL.into_iter().find(|a| a.name() == s.trim())
    }
    /// Hat das Material eine Richtung (Bürstrichtung)?
    pub fn has_direction(self) -> bool {
        self == Material::Brushed
    }
}

/// Effekte eines Objekts; je Kategorie höchstens einer, Stärken 0–1. Farbüberlagerung
/// (`Style::blend`) und Schatten (`Style::shadow`) stehen im Stil selbst.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Effects {
    /// Glas: Darunterliegendes erscheint weichgezeichnet (Stärke = Unschärfe, 0–1)
    pub glass: Option<f64>,
    /// Glas: feine Körnung (0–1). Die Füllung tönt das Glas (siehe `GLASS_FILL`).
    pub glass_grain: f64,
    /// Material der Oberfläche (ohne: matt)
    pub mat: Option<(Material, f64)>,
    /// Richtung des Materials in Grad (Bürstrichtung)
    pub mat_angle: f64,
    /// Kunsteffekt auf der Füllung
    pub art: Option<(Art, f64)>,
    /// 3D-Effekt: Form und Stärke von −1 (eingedrückt) über 0 (Ausgangszustand) bis 1
    /// (herausgedrückt)
    pub lens: Option<(Lens, f64)>,
    /// Stärke von Licht und Schatten beim 3D-Effekt, 0–1 (Standard 0,5)
    pub light: f64,
    /// Extra: Spiegelung unter dem Objekt (schließt den Schatten aus)
    pub mirror: Option<f64>,
    /// Farbfilter für Bilder (Sepia, Schwarz-Weiß, Invertieren)
    pub tint: Option<(ImgFilter, f64)>,
    /// Farbtemperatur von Bildern: −1 = kalt, 0 = Original, 1 = warm
    pub temp: f64,
    /// Extra: Leuchten um die Kontur (bzw. die Form ohne Kontur), Stärke 0–1
    pub glow: Option<f64>,
    /// Extra: Schatten nach innen (die Form wirkt eingedrückt), Deckkraft 0–1
    pub inner: Option<f64>,
    /// Innenschatten: Abstand und Weichzeichnung (Dokumenteinheiten), Richtung in Grad
    /// (0° = nach rechts, 90° = nach unten; das Licht kommt von der Gegenseite)
    pub inner_dist: f64,
    pub inner_blur: f64,
    pub inner_angle: f64,
    /// Spiegelung: Länge (Anteil der Objekthöhe) und Abstand zur Unterkante
    pub mirror_len: f64,
    pub mirror_gap: f64,
    /// Abstand der Punkte (Dokumenteinheiten) und Winkel (Grad) der Rasterpunkte
    pub art_size: f64,
    pub art_angle: f64,
}

/// Abstand der Rasterpunkte, mit dem sie beginnen.
pub const ART_SIZE: f64 = 8.0;
/// Winkel der Rasterpunkte, mit dem sie beginnen (wie im Druck).
pub const ART_ANGLE: f64 = 45.0;

/// Vorgaben für Innenschatten und Spiegelung.
pub const INNER_DIST: f64 = 4.0;
pub const INNER_BLUR: f64 = 8.0;
pub const INNER_ANGLE: f64 = 45.0;
pub const MIRROR_LEN: f64 = 0.75;
pub const MIRROR_GAP: f64 = 2.0;
/// Glas: Körnung, mit der es beginnt, und wie stark die Füllung das Glas tönt (Anteil ihrer
/// Deckkraft; ohne Füllung ist das Glas farblos).
pub const GLASS_GRAIN: f64 = 0.2;
pub const GLASS_FILL: f64 = 0.25;

/// Reichweite des Leuchtens (Dokumenteinheiten) bei Stärke `k`.
pub fn glow_radius(k: f64) -> f64 {
    4.0 + 28.0 * k.clamp(0.0, 1.0)
}

/// Stärke der Beleuchtung beim 3D-Effekt, mit der ein neuer Effekt beginnt.
pub const LIGHT_DEFAULT: f64 = 0.5;

impl Default for Effects {
    fn default() -> Effects {
        Effects {
            glass: None,
            glass_grain: GLASS_GRAIN,
            mat: None,
            mat_angle: 0.0,
            art: None,
            lens: None,
            light: LIGHT_DEFAULT,
            mirror: None,
            tint: None,
            temp: 0.0,
            glow: None,
            inner: None,
            inner_dist: INNER_DIST,
            inner_blur: INNER_BLUR,
            inner_angle: INNER_ANGLE,
            mirror_len: MIRROR_LEN,
            mirror_gap: MIRROR_GAP,
            art_size: ART_SIZE,
            art_angle: ART_ANGLE,
        }
    }
}

impl Effects {
    /// Braucht das Objekt den Hintergrund unter sich (Glas, 3D-Effekt)?
    pub fn uses_backdrop(&self) -> bool {
        self.glass.is_some() || self.lens.is_some()
    }

    /// Braucht das Objekt die Glas-Zeichnung (Hintergrund, Relief oder Material über dem Objekt)?
    pub fn needs_glass(&self) -> bool {
        self.uses_backdrop() || self.mat.is_some()
    }

    /// Versatz des Innenschattens (Dokumenteinheiten).
    pub fn inner_offset(&self) -> Pt {
        let a = self.inner_angle.to_radians();
        Pt::new(a.cos(), a.sin()) * self.inner_dist
    }

    /// Unschärfe des Glases, falls Glas angewendet ist.
    pub fn frost(&self) -> Option<f64> {
        self.glass
    }

    /// Kurzform zum Speichern: „frost=0.5 tex=gloss:0.4 art=grain:0.4 lens=drop:-0.6 …“
    /// (Glas steht als „frost=“, das Material als „tex=“ – so lesen ältere Versionen Glas,
    /// Hochglanz und gebürstetes Metall weiter).
    pub fn to_attr(&self) -> String {
        let mut v = Vec::new();
        if let Some(f) = self.glass {
            v.push(format!("frost={f}"));
        }
        if let Some((t, f)) = self.mat {
            v.push(format!("tex={}:{f}", t.name()));
        }
        if let Some((a, k)) = self.art {
            v.push(format!("art={}:{k}", a.name()));
        }
        if let Some((l, k)) = self.lens {
            v.push(format!("lens={}:{k}", l.name()));
        }
        if (self.light - LIGHT_DEFAULT).abs() > 1e-9 {
            v.push(format!("light={}", self.light));
        }
        if let Some(m) = self.mirror {
            v.push(format!("mirror={m}"));
        }
        if let Some((f, k)) = self.tint {
            v.push(format!("tint={}:{k}", f.name()));
        }
        if self.temp.abs() > 1e-9 {
            v.push(format!("temp={}", self.temp));
        }
        if let Some(g) = self.glow {
            v.push(format!("glow={g}"));
        }
        if let Some(g) = self.inner {
            v.push(format!("inner={g}"));
        }
        for (key, val, def) in [
            ("innerdist", self.inner_dist, INNER_DIST),
            ("innerblur", self.inner_blur, INNER_BLUR),
            ("innerangle", self.inner_angle, INNER_ANGLE),
            ("mirrorlen", self.mirror_len, MIRROR_LEN),
            ("mirrorgap", self.mirror_gap, MIRROR_GAP),
            ("glassgrain", self.glass_grain, GLASS_GRAIN),
            ("matangle", self.mat_angle, 0.0),
        ] {
            if (val - def).abs() > 1e-9 {
                v.push(format!("{key}={val}"));
            }
        }
        if (self.art_size - ART_SIZE).abs() > 1e-9 {
            v.push(format!("artsize={}", self.art_size));
        }
        if (self.art_angle - ART_ANGLE).abs() > 1e-9 {
            v.push(format!("artangle={}", self.art_angle));
        }
        v.join(" ")
    }

    pub fn from_attr(s: &str) -> Effects {
        let mut e = Effects::default();
        for part in s.split_whitespace() {
            let Some((k, v)) = part.split_once('=') else {
                continue;
            };
            let pair = || {
                let (a, b) = v.split_once(':')?;
                Some((a, b.parse::<f64>().ok()?.clamp(0.0, 1.0)))
            };
            match k {
                "frost" => e.glass = v.parse::<f64>().ok().map(|x| x.clamp(0.0, 1.0)),
                // Dateien bis 0.9.60: Milchglas war eine Textur; Papier, Holz und Fell
                // entfallen (matt)
                "tex" => match pair() {
                    Some(("frost", k)) => e.glass = Some(k),
                    p => e.mat = p.and_then(|(a, k)| Some((Material::from_name(a)?, k))),
                },
                "art" => e.art = pair().and_then(|(a, k)| Some((Art::from_name(a)?, k))),
                "lens" => {
                    e.lens = v.split_once(':').and_then(|(a, b)| {
                        let k = b.parse::<f64>().ok()?.clamp(-1.0, 1.0);
                        // Dateien bis 0.9.24: Konvex und Konkav (Stärke 0–1)
                        match a {
                            "convex" => Some((Lens::Drop, k)),
                            "concave" => Some((Lens::Drop, -k)),
                            _ => Some((Lens::from_name(a)?, k)),
                        }
                    })
                }
                "light" => e.light = v.parse::<f64>().ok().map_or(LIGHT_DEFAULT, |x| x.clamp(0.0, 1.0)),
                "mirror" => e.mirror = v.parse::<f64>().ok().map(|x| x.clamp(0.0, 1.0)),
                "tint" => match pair() {
                    // Dateien bis 0.9.23: Warm und Kalt waren Filter, jetzt die Temperatur
                    Some(("warm", k)) => e.temp = k,
                    Some(("cool", k)) => e.temp = -k,
                    p => e.tint = p.and_then(|(a, k)| Some((ImgFilter::from_name(a)?, k))),
                },
                "temp" => e.temp = v.parse::<f64>().ok().map_or(0.0, |x| x.clamp(-1.0, 1.0)),
                "glow" => e.glow = v.parse::<f64>().ok().map(|x| x.clamp(0.0, 1.0)),
                "inner" => e.inner = v.parse::<f64>().ok().map(|x| x.clamp(0.0, 1.0)),
                "innerdist" | "innerblur" | "innerangle" | "mirrorlen" | "mirrorgap" | "glassgrain"
                | "matangle" => {
                    if let Some(x) = v.parse::<f64>().ok().filter(|x| x.is_finite()) {
                        match k {
                            "innerdist" => e.inner_dist = x.clamp(0.0, 500.0),
                            "innerblur" => e.inner_blur = x.clamp(0.0, 500.0),
                            "innerangle" => e.inner_angle = normalize_angle(x),
                            "mirrorlen" => e.mirror_len = x.clamp(0.05, 3.0),
                            "glassgrain" => e.glass_grain = x.clamp(0.0, 1.0),
                            "matangle" => e.mat_angle = normalize_angle(x),
                            _ => e.mirror_gap = x.clamp(-1000.0, 1000.0),
                        }
                    }
                }
                "artsize" => {
                    e.art_size = v
                        .parse::<f64>()
                        .ok()
                        .filter(|x| x.is_finite())
                        .map_or(ART_SIZE, |x| x.clamp(1.0, 500.0))
                }
                "artangle" => {
                    e.art_angle = v
                        .parse::<f64>()
                        .ok()
                        .filter(|x| x.is_finite())
                        .map_or(ART_ANGLE, normalize_angle)
                }
                _ => {}
            }
        }
        e
    }
}

/// Stärke, mit der neue Effekte beginnen.
pub const FX_DEFAULT: f64 = 0.5;

/// Stärke, mit der Milchglas beginnt. Die Skala ist seit 0.9.19 verdoppelt:
/// 100 % entsprechen dem früheren 50 %, damit neues Milchglas gleich aussieht.
pub const FROST_DEFAULT: f64 = 1.0;

// ───────────────────────────── Mischmodi ─────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Blend {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl Blend {
    pub const ALL: [Blend; 16] = [
        Blend::Normal,
        Blend::Multiply,
        Blend::Screen,
        Blend::Overlay,
        Blend::Darken,
        Blend::Lighten,
        Blend::ColorDodge,
        Blend::ColorBurn,
        Blend::HardLight,
        Blend::SoftLight,
        Blend::Difference,
        Blend::Exclusion,
        Blend::Hue,
        Blend::Saturation,
        Blend::Color,
        Blend::Luminosity,
    ];
    /// Farbüberlagerung, wie sie die Seitenleiste anbietet: überdecken (Standard), aufhellen
    /// oder abdunkeln (Farben mischen sich wie übereinanderliegende Farbfolien).
    pub const SIMPLE: [Blend; 3] = [Blend::Normal, Blend::Screen, Blend::Multiply];
    /// Bezeichnung wie in gängigen Grafikprogrammen.
    pub fn label(self) -> &'static str {
        match self {
            Blend::Normal => tr("Cover"),
            Blend::Multiply => tr("Darken"),
            Blend::Screen => tr("Lighten"),
            Blend::Overlay => tr("Overlay"),
            Blend::Darken => tr("Darken (darker colour)"),
            Blend::Lighten => tr("Lighten (lighter colour)"),
            Blend::ColorDodge => tr("Colour dodge"),
            Blend::ColorBurn => tr("Colour burn"),
            Blend::HardLight => tr("Hard light"),
            Blend::SoftLight => tr("Soft light"),
            Blend::Difference => tr("Difference"),
            Blend::Exclusion => tr("Exclusion"),
            Blend::Hue => tr("Hue"),
            Blend::Saturation => tr("Saturation"),
            Blend::Color => tr("Colour"),
            Blend::Luminosity => tr("Luminosity"),
        }
    }
    /// Name in CSS/SVG (`mix-blend-mode`).
    pub fn css(self) -> &'static str {
        match self {
            Blend::Normal => "normal",
            Blend::Multiply => "multiply",
            Blend::Screen => "screen",
            Blend::Overlay => "overlay",
            Blend::Darken => "darken",
            Blend::Lighten => "lighten",
            Blend::ColorDodge => "color-dodge",
            Blend::ColorBurn => "color-burn",
            Blend::HardLight => "hard-light",
            Blend::SoftLight => "soft-light",
            Blend::Difference => "difference",
            Blend::Exclusion => "exclusion",
            Blend::Hue => "hue",
            Blend::Saturation => "saturation",
            Blend::Color => "color",
            Blend::Luminosity => "luminosity",
        }
    }
    pub fn from_css(s: &str) -> Option<Blend> {
        Blend::ALL.into_iter().find(|b| b.css() == s.trim())
    }
}

// ───────────────────────────── Farbverläufe ─────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GradientKind {
    /// Weiche Übergänge
    #[default]
    Smooth,
    /// Weich mit Körnung (Rauschen)
    Grain,
    /// Grobe, gerasterte Pixel (geometrisch, gröber als gekörnt)
    Pixel,
}

impl GradientKind {
    pub const ALL: [GradientKind; 3] = [GradientKind::Smooth, GradientKind::Grain, GradientKind::Pixel];
    pub fn name(self) -> &'static str {
        match self {
            GradientKind::Smooth => "smooth",
            GradientKind::Grain => "grain",
            GradientKind::Pixel => "pixel",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            GradientKind::Smooth => tr("Soft"),
            GradientKind::Grain => tr("Grainy"),
            GradientKind::Pixel => tr("Coarse pixels"),
        }
    }
    pub fn from_name(s: &str) -> Option<GradientKind> {
        GradientKind::ALL.into_iter().find(|k| k.name() == s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stop {
    /// Lage auf der Verlaufslinie (0 = Anfang, 1 = Ende).
    pub pos: f64,
    pub color: Color,
}

/// Anteil, um den die Verlaufslinie an beiden Enden eingerückt beginnt.
pub const GRADIENT_INSET: f64 = 0.08;

/// Linearer Farbverlauf. `p0`/`p1` liegen relativ zum Objektrahmen (0–1, wie
/// `objectBoundingBox` in SVG) und wandern so beim Verschieben und Skalieren mit.
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub stops: Vec<Stop>,
    pub p0: Pt,
    pub p1: Pt,
    pub kind: GradientKind,
    /// linear, kreisförmig oder ein zweifarbiges Muster
    pub shape: GradShape,
    /// Mustergröße (Dokumenteinheiten)
    pub cell: f64,
}

/// Form eines Verlaufs bzw. Musters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GradShape {
    #[default]
    Linear,
    /// kreisförmig um `p0`, Radius bis `p1`
    Radial,
    /// Muster: Schachbrett
    Checker,
    /// Muster: Punkte
    Dots,
    /// Muster: Streifen
    Stripes,
    /// Muster: Zebrastreifen (wellig, mit Gabelungen)
    Zebra,
    /// Muster: Kuhflecken
    Cow,
    /// Muster: Gitter aus dünnen Linien
    Grid,
    /// Muster: Vichy-Karo (Streifen kreuzen sich, die Kreuzungen sind dunkler)
    Gingham,
    /// Muster: Zickzack-Streifen
    Chevron,
    /// Muster: Fischgrät
    Herringbone,
    /// Muster: Bienenwaben
    Honeycomb,
    /// Muster: Wellenlinien
    Waves,
    /// Muster: Schuppen (japanische Wellen, Seigaiha)
    Scales,
    /// Muster: Mauerwerk
    Bricks,
    /// Muster: Leopard (offene Ringe und Flecken)
    Leopard,
    /// Muster: Terrazzo (verstreute Splitter)
    Terrazzo,
}

/// Eine Fläche einer Musterkachel: Umrisse (gerade/ungerade gefüllt, so entstehen Ringe),
/// Farbe (`ink` = zweite Farbe, sonst die Grundfarbe) und Deckkraft.
#[derive(Clone, Debug, PartialEq)]
pub struct PatternShape {
    pub contours: Vec<Vec<Pt>>,
    pub ink: bool,
    pub alpha: f64,
}

/// Kachel eines Musters: Breite, Höhe und Flächen in Zeichenreihenfolge.
#[derive(Clone, Debug, PartialEq)]
pub struct PatternTile {
    pub w: f64,
    pub h: f64,
    pub shapes: Vec<PatternShape>,
}

impl GradShape {
    pub const PATTERNS: [GradShape; 15] = [
        GradShape::Checker,
        GradShape::Gingham,
        GradShape::Grid,
        GradShape::Dots,
        GradShape::Stripes,
        GradShape::Waves,
        GradShape::Chevron,
        GradShape::Herringbone,
        GradShape::Bricks,
        GradShape::Honeycomb,
        GradShape::Scales,
        GradShape::Terrazzo,
        GradShape::Zebra,
        GradShape::Leopard,
        GradShape::Cow,
    ];
    pub fn name(self) -> &'static str {
        match self {
            GradShape::Linear => "linear",
            GradShape::Radial => "radial",
            GradShape::Checker => "checker",
            GradShape::Dots => "dots",
            GradShape::Stripes => "stripes",
            GradShape::Zebra => "zebra",
            GradShape::Cow => "cow",
            GradShape::Grid => "grid",
            GradShape::Gingham => "gingham",
            GradShape::Chevron => "chevron",
            GradShape::Herringbone => "herringbone",
            GradShape::Honeycomb => "honeycomb",
            GradShape::Waves => "waves",
            GradShape::Scales => "scales",
            GradShape::Bricks => "bricks",
            GradShape::Leopard => "leopard",
            GradShape::Terrazzo => "terrazzo",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            GradShape::Linear => tr("Linear"),
            GradShape::Radial => tr("Radial"),
            GradShape::Checker => tr("Chequerboard"),
            GradShape::Dots => tr("Dots"),
            GradShape::Stripes => tr("Stripes"),
            GradShape::Zebra => tr("Zebra"),
            GradShape::Cow => tr("Cow print"),
            GradShape::Grid => tr("Grid"),
            GradShape::Gingham => tr("Gingham"),
            GradShape::Chevron => tr("Chevron"),
            GradShape::Herringbone => tr("Herringbone"),
            GradShape::Honeycomb => tr("Honeycomb"),
            GradShape::Waves => tr("Waves"),
            GradShape::Scales => tr("Scales"),
            GradShape::Bricks => tr("Bricks"),
            GradShape::Leopard => tr("Leopard"),
            GradShape::Terrazzo => tr("Terrazzo"),
        }
    }
    pub fn from_name(s: &str) -> Option<GradShape> {
        [GradShape::Linear, GradShape::Radial]
            .into_iter()
            .chain(GradShape::PATTERNS)
            .find(|g| g.name() == s.trim())
    }
    pub fn is_pattern(self) -> bool {
        GradShape::PATTERNS.contains(&self)
    }

    /// Kachel der neueren Muster (alle außer Schachbrett, Punkte und Streifen). Flächen,
    /// die über den Rand ragen, stehen auch auf der Gegenseite, so dass die Kacheln nahtlos
    /// aneinanderpassen (außerhalb wird abgeschnitten). Für die übrigen Muster `None`.
    pub fn pattern_tile(self, cell: f64) -> Option<PatternTile> {
        use std::f64::consts::TAU;
        let k = cell.max(0.5);
        let ink = |pts: Vec<Pt>| PatternShape {
            contours: vec![pts],
            ink: true,
            alpha: 1.0,
        };
        let rect = |x: f64, y: f64, w: f64, h: f64| {
            vec![
                Pt::new(x, y),
                Pt::new(x + w, y),
                Pt::new(x + w, y + h),
                Pt::new(x, y + h),
            ]
        };
        let circle = |cx: f64, cy: f64, r: f64, n: usize| -> Vec<Pt> {
            (0..n)
                .map(|j| {
                    let a = j as f64 / n as f64 * TAU;
                    Pt::new(cx + r * a.cos(), cy + r * a.sin())
                })
                .collect()
        };
        // einfacher Zufall mit festem Startwert (gleiches Muster bei jedem Zeichnen)
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut shapes: Vec<PatternShape> = Vec::new();
        // eigene Abdeckung über den Rand (sonst wird unten gespiegelt ergänzt)
        let mut wrapped = false;
        let (w, h) = match self {
            GradShape::Grid => {
                let lw = (k * 0.08).max(0.05);
                shapes.push(ink(rect(0.0, 0.0, k, lw)));
                shapes.push(ink(rect(0.0, 0.0, lw, k)));
                (k, k)
            }
            GradShape::Gingham => {
                // Kreuzung voll, einzelne Streifen schräg schraffiert – echte zwei Farben,
                // wie gewebtes Vichy-Karo
                shapes.push(ink(rect(0.0, 0.0, k, k)));
                let p = k / 4.0;
                let lw = p * 0.45;
                let l = 3.0 * k;
                for (x0, y0) in [(k, 0.0), (0.0, k)] {
                    let mut c = -2.0 * k;
                    while c < 2.0 * k {
                        let band = vec![
                            Pt::new(c - l, -l),
                            Pt::new(c + l, l),
                            Pt::new(c + lw + l, l),
                            Pt::new(c + lw - l, -l),
                        ];
                        let piece = clip_to_rect(&band, x0, y0, x0 + k, y0 + k);
                        if piece.len() >= 3 {
                            shapes.push(ink(piece));
                        }
                        c += p;
                    }
                }
                (2.0 * k, 2.0 * k)
            }
            GradShape::Chevron => {
                let th = k * 0.5;
                shapes.push(ink(vec![
                    Pt::new(0.0, 0.0),
                    Pt::new(k, th),
                    Pt::new(2.0 * k, 0.0),
                    Pt::new(2.0 * k, th),
                    Pt::new(k, 2.0 * th),
                    Pt::new(0.0, th),
                ]));
                (2.0 * k, k)
            }
            GradShape::Herringbone => {
                // zwei Spalten schräger Bretter, gegeneinander versetzt
                let th = k * 0.45;
                for y in [-k, 0.0, k, 2.0 * k] {
                    shapes.push(ink(vec![
                        Pt::new(0.0, y),
                        Pt::new(k, y - k),
                        Pt::new(k, y - k + th),
                        Pt::new(0.0, y + th),
                    ]));
                    let y2 = y + k * 0.5;
                    shapes.push(ink(vec![
                        Pt::new(k, y2 - k),
                        Pt::new(2.0 * k, y2),
                        Pt::new(2.0 * k, y2 + th),
                        Pt::new(k, y2 - k + th),
                    ]));
                }
                wrapped = true;
                (2.0 * k, k)
            }
            GradShape::Honeycomb => {
                // spitze Waben mit Radius k, Linien als Ringe
                let r = k;
                let (w, h) = (3f64.sqrt() * r, 3.0 * r);
                let lw = k * 0.12;
                let hex = |cx: f64, cy: f64, rr: f64| -> Vec<Pt> {
                    (0..6)
                        .map(|j| {
                            let a = (j as f64 * 60.0 + 30.0).to_radians();
                            Pt::new(cx + rr * a.cos(), cy + rr * a.sin())
                        })
                        .collect()
                };
                let d = lw / 3f64.sqrt();
                for j in -1..=2 {
                    for i in -1..=1 {
                        let cx = i as f64 * w + if j % 2 != 0 { w / 2.0 } else { 0.0 };
                        let cy = j as f64 * 1.5 * r;
                        shapes.push(PatternShape {
                            contours: vec![hex(cx, cy, r + d), hex(cx, cy, r - d)],
                            ink: true,
                            alpha: 1.0,
                        });
                    }
                }
                wrapped = true;
                (w, h)
            }
            GradShape::Waves => {
                let (w, th, amp) = (2.0 * k, k * 0.3, k * 0.22);
                let n = 48;
                let mut pts: Vec<Pt> = (0..=n)
                    .map(|j| {
                        let x = j as f64 * w / n as f64;
                        Pt::new(x, k * 0.3 + amp * (x / w * TAU).sin())
                    })
                    .collect();
                pts.extend((0..=n).rev().map(|j| {
                    let x = j as f64 * w / n as f64;
                    Pt::new(x, k * 0.3 + th + amp * (x / w * TAU).sin())
                }));
                shapes.push(ink(pts));
                (w, k)
            }
            GradShape::Scales => {
                // Halbkreise in versetzten Reihen; jede Reihe verdeckt die untere Hälfte der
                // vorigen (Grundfarbe zuerst, dann die Ringe)
                let r = k;
                for j in -2..=3 {
                    for i in -1..=2 {
                        let cx = i as f64 * 2.0 * r + if j % 2 != 0 { r } else { 0.0 };
                        let cy = j as f64 * 0.5 * r;
                        shapes.push(PatternShape {
                            contours: vec![circle(cx, cy, r, 48)],
                            ink: false,
                            alpha: 1.0,
                        });
                        for (a, b) in [(0.84, 1.0), (0.5, 0.66)] {
                            shapes.push(PatternShape {
                                contours: vec![circle(cx, cy, r * b, 48), circle(cx, cy, r * a, 48)],
                                ink: true,
                                alpha: 1.0,
                            });
                        }
                        shapes.push(ink(circle(cx, cy, r * 0.2, 24)));
                    }
                }
                wrapped = true;
                (2.0 * r, r)
            }
            GradShape::Bricks => {
                let lw = (k * 0.08).max(0.05);
                let (w, h) = (2.0 * k, k);
                shapes.push(ink(rect(0.0, 0.0, w, lw)));
                shapes.push(ink(rect(0.0, h * 0.5, w, lw)));
                for x in [0.0, k] {
                    shapes.push(ink(rect(x, 0.0, lw, h * 0.5)));
                }
                for x in [k * 0.5, k * 1.5] {
                    shapes.push(ink(rect(x, h * 0.5, lw, h * 0.5)));
                }
                (w, h)
            }
            GradShape::Leopard => {
                // offene Ringe aus zwei bis drei Bögen, dazwischen kleine Flecken
                let t = k * 6.0;
                let rosettes = [
                    (0.18, 0.2, 0.15),
                    (0.62, 0.14, 0.13),
                    (0.4, 0.52, 0.16),
                    (0.86, 0.5, 0.13),
                    (0.14, 0.8, 0.14),
                    (0.66, 0.85, 0.15),
                ];
                for (cx, cy, r) in rosettes {
                    let (cx, cy, r) = (cx * t, cy * t, r * t);
                    let start = rnd() * TAU;
                    let pieces = if rnd() < 0.5 { 2 } else { 3 };
                    for p in 0..pieces {
                        let a0 = start + p as f64 * TAU / pieces as f64;
                        let a1 = a0 + TAU / pieces as f64 * (0.72 + 0.18 * rnd());
                        let wob = rnd() * TAU;
                        let n = 24;
                        let rad = |a: f64| r * (1.0 + 0.12 * (3.0 * a + wob).sin());
                        let thick = |f: f64| r * 0.5 * (f * std::f64::consts::PI).sin().powf(0.4).max(0.2);
                        let mut pts: Vec<Pt> = (0..=n)
                            .map(|j| {
                                let f = j as f64 / n as f64;
                                let a = a0 + (a1 - a0) * f;
                                let rr = rad(a) + thick(f) * 0.5;
                                Pt::new(cx + rr * a.cos(), cy + rr * 0.9 * a.sin())
                            })
                            .collect();
                        pts.extend((0..=n).rev().map(|j| {
                            let f = j as f64 / n as f64;
                            let a = a0 + (a1 - a0) * f;
                            let rr = rad(a) - thick(f) * 0.5;
                            Pt::new(cx + rr * a.cos(), cy + rr * 0.9 * a.sin())
                        }));
                        shapes.push(ink(pts));
                    }
                }
                for (sx, sy, r) in [
                    (0.4, 0.03, 0.04),
                    (0.92, 0.22, 0.035),
                    (0.04, 0.5, 0.04),
                    (0.42, 0.97, 0.035),
                ] {
                    shapes.push(ink(circle(sx * t, sy * t, r * t, 16)));
                }
                (t, t)
            }
            GradShape::Terrazzo => {
                let t = k * 6.0;
                for _ in 0..44 {
                    let (cx, cy) = (rnd() * t, rnd() * t);
                    let size = k * (0.18 + 0.55 * rnd() * rnd());
                    let n = 5 + (rnd() * 3.0) as usize;
                    let rot = rnd() * TAU;
                    let pts: Vec<Pt> = (0..n)
                        .map(|j| {
                            let a = rot + j as f64 / n as f64 * TAU;
                            let rr = size * (0.55 + 0.45 * rnd());
                            Pt::new(cx + rr * a.cos(), cy + rr * a.sin())
                        })
                        .collect();
                    shapes.push(ink(pts));
                }
                (t, t)
            }
            GradShape::Zebra => {
                // fünf breite, sanft gewellte Streifen je Kachel, die zu Spitzen auslaufen
                let t = k * 6.0;
                let rows = 5;
                let gap = t / rows as f64;
                let n = 72;
                for i in 0..rows {
                    let fi = i as f64;
                    let base = (fi + 0.2) * gap;
                    // je Streifen eigene Phasen, damit kein gleichmäßiges Wellenmuster entsteht
                    let (p1, p2, p3, p4) = (fi * 2.1 + 0.4, fi * 1.3 + 2.0, fi * 2.9 + 1.1, fi * 0.7 + 0.3);
                    let top = |x: f64| {
                        let u = x / t * TAU;
                        base + gap * 0.42 * (u + p1).sin() + gap * 0.12 * (2.0 * u + p2).sin()
                    };
                    let thick = |x: f64| {
                        let u = x / t * TAU;
                        let w = 0.55 + 0.45 * (u + p3).sin() + 0.12 * (2.0 * u + p4).sin();
                        gap * 0.6 * w.clamp(0.0, 1.0).powf(0.5).max(0.03)
                    };
                    let mut pts: Vec<Pt> = (0..=n)
                        .map(|j| {
                            let x = j as f64 * t / n as f64;
                            Pt::new(x, top(x))
                        })
                        .collect();
                    pts.extend((0..=n).rev().map(|j| {
                        let x = j as f64 * t / n as f64;
                        Pt::new(x, top(x) + thick(x))
                    }));
                    shapes.push(ink(pts));
                }
                (t, t)
            }
            GradShape::Cow => {
                // fünf unregelmäßige Flecken je Kachel
                let t = k * 6.0;
                let spots = [
                    (0.22, 0.25, 0.17, 0.3),
                    (0.7, 0.18, 0.12, 1.4),
                    (0.55, 0.62, 0.2, 2.2),
                    (0.12, 0.78, 0.1, 3.1),
                    (0.9, 0.85, 0.08, 4.4),
                ];
                for (cx, cy, r, ph) in spots {
                    let n = 48;
                    let pts: Vec<Pt> = (0..n)
                        .map(|j| {
                            let a = j as f64 / n as f64 * TAU;
                            let rr = r
                                * t
                                * (1.0
                                    + 0.22 * (3.0 * a + ph).sin()
                                    + 0.14 * (5.0 * a + ph * 1.7).sin()
                                    + 0.1 * (2.0 * a + ph * 0.6).cos());
                            Pt::new(cx * t + rr * a.cos(), cy * t + rr * 0.85 * a.sin())
                        })
                        .collect();
                    shapes.push(ink(pts));
                }
                (t, t)
            }
            _ => return None,
        };
        if wrapped {
            return Some(PatternTile { w, h, shapes });
        }
        // über den Rand ragende Flächen auch auf der Gegenseite
        let mut out = Vec::new();
        for sh in shapes {
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for p in sh.contours.iter().flatten() {
                x0 = x0.min(p.x);
                y0 = y0.min(p.y);
                x1 = x1.max(p.x);
                y1 = y1.max(p.y);
            }
            for dx in [-w, 0.0, w] {
                for dy in [-h, 0.0, h] {
                    if x0 + dx < w && x1 + dx > 0.0 && y0 + dy < h && y1 + dy > 0.0 {
                        out.push(PatternShape {
                            contours: sh
                                .contours
                                .iter()
                                .map(|c| c.iter().map(|p| Pt::new(p.x + dx, p.y + dy)).collect())
                                .collect(),
                            ink: sh.ink,
                            alpha: sh.alpha,
                        });
                    }
                }
            }
        }
        Some(PatternTile { w, h, shapes: out })
    }
}

/// Konvexes Vieleck auf ein Rechteck zuschneiden (Sutherland–Hodgman).
fn clip_to_rect(poly: &[Pt], x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Pt> {
    let mut out: Vec<Pt> = poly.to_vec();
    // (Kante: innen, wenn f(p) >= 0; Schnittpunkt mit der Kante)
    let edges: [(&dyn Fn(Pt) -> f64, bool); 4] = [
        (&|p: Pt| p.x - x0, true),
        (&|p: Pt| x1 - p.x, true),
        (&|p: Pt| p.y - y0, false),
        (&|p: Pt| y1 - p.y, false),
    ];
    for (f, _) in edges {
        if out.is_empty() {
            break;
        }
        let input = std::mem::take(&mut out);
        for i in 0..input.len() {
            let (a, b) = (input[i], input[(i + 1) % input.len()]);
            let (fa, fb) = (f(a), f(b));
            if fa >= 0.0 {
                out.push(a);
            }
            if (fa >= 0.0) != (fb >= 0.0) {
                let t = fa / (fa - fb);
                out.push(Pt::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t));
            }
        }
    }
    out
}

/// Standardgröße eines Musters.
pub const PATTERN_CELL: f64 = 16.0;

fn mix(a: Color, b: Color, t: f64) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

impl Gradient {
    /// Kurzform zum Speichern: „x0 y0 x1 y1|pos:#rrggbb:a …“.
    pub fn to_attr(&self) -> String {
        let stops: Vec<String> = self
            .stops
            .iter()
            .map(|s| format!("{}:{}:{}", s.pos, s.color.to_hex(), s.color.a))
            .collect();
        let mut out = format!(
            "{} {} {} {}|{}",
            self.p0.x,
            self.p0.y,
            self.p1.x,
            self.p1.y,
            stops.join(" ")
        );
        if self.shape != GradShape::Linear {
            out.push_str(&format!("|{} {}", self.shape.name(), self.cell));
        }
        out
    }

    pub fn from_attr(s: &str) -> Option<Gradient> {
        let (line, rest) = s.split_once('|')?;
        let (stops, form) = rest.split_once('|').unwrap_or((rest, ""));
        let mut fit = form.split_whitespace();
        let shape = fit.next().and_then(GradShape::from_name).unwrap_or_default();
        let cell = fit.next().and_then(|c| c.parse().ok()).unwrap_or(PATTERN_CELL);
        let v: Vec<f64> = line.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        if v.len() != 4 {
            return None;
        }
        let stops: Vec<Stop> = stops
            .split_whitespace()
            .filter_map(|it| {
                let mut p = it.split(':');
                let pos: f64 = p.next()?.parse().ok()?;
                let mut color = Color::parse(p.next()?)?;
                color.a = p.next().and_then(|a| a.parse().ok()).unwrap_or(1.0);
                Some(Stop { pos, color })
            })
            .collect();
        if stops.len() < 2 {
            return None;
        }
        Some(Gradient {
            stops,
            p0: Pt::new(v[0], v[1]),
            p1: Pt::new(v[2], v[3]),
            kind: GradientKind::Smooth,
            shape,
            cell,
        })
    }

    /// Gleiche Farben an gleichen Stellen (Lage der Verlaufslinie egal)?
    pub fn same_colors(&self, o: &Gradient) -> bool {
        self.stops.len() == o.stops.len()
            && self.stops.iter().zip(&o.stops).all(|(a, b)| {
                (a.pos - b.pos).abs() < 1e-3
                    && (a.color.r - b.color.r).abs() < 2e-3
                    && (a.color.g - b.color.g).abs() < 2e-3
                    && (a.color.b - b.color.b).abs() < 2e-3
                    && (a.color.a - b.color.a).abs() < 2e-3
            })
    }
    /// Waagerechter Verlauf über die ganze Breite.
    pub fn new(a: Color, b: Color) -> Gradient {
        Gradient {
            stops: vec![Stop { pos: 0.0, color: a }, Stop { pos: 1.0, color: b }],
            // leicht eingerückt, damit die Griffe nicht auf den Skalierungsanfassern liegen
            p0: Pt::new(GRADIENT_INSET, 0.5),
            p1: Pt::new(1.0 - GRADIENT_INSET, 0.5),
            kind: GradientKind::Smooth,
            shape: GradShape::Linear,
            cell: PATTERN_CELL,
        }
    }

    /// Form wechseln; die Verlaufslinie wird passend gesetzt (kreisförmig: von der Mitte
    /// zum Rand, Muster: waagerecht).
    pub fn set_shape(&mut self, shape: GradShape) {
        if self.shape == shape {
            return;
        }
        match shape {
            GradShape::Radial => {
                self.p0 = Pt::new(0.5, 0.5);
                self.p1 = Pt::new(1.0 - GRADIENT_INSET, 0.5);
            }
            _ if self.shape == GradShape::Radial => {
                self.p0 = Pt::new(GRADIENT_INSET, 0.5);
                self.p1 = Pt::new(1.0 - GRADIENT_INSET, 0.5);
            }
            _ => {}
        }
        self.shape = shape;
    }

    /// Farbstopps nach Lage sortiert.
    pub fn sorted(&self) -> Vec<Stop> {
        let mut v = self.stops.clone();
        v.sort_by(|a, b| a.pos.total_cmp(&b.pos));
        v
    }

    /// Farbe an Stelle `t` (weich interpoliert).
    pub fn color_at(&self, t: f64) -> Color {
        let s = self.sorted();
        let Some(first) = s.first() else {
            return Color::BLACK;
        };
        if t <= first.pos {
            return first.color;
        }
        for w in s.windows(2) {
            if t <= w[1].pos {
                let span = (w[1].pos - w[0].pos).max(1e-9);
                return mix(w[0].color, w[1].color, (t - w[0].pos) / span);
            }
        }
        s[s.len() - 1].color
    }

    /// Stopps für die Darstellung (nach Lage sortiert).
    pub fn render_stops(&self) -> Vec<(f64, Color)> {
        self.sorted().iter().map(|x| (x.pos, x.color)).collect()
    }

    fn denorm(p: Pt, r: &Rect) -> Pt {
        Pt::new(r.x0 + p.x * r.width(), r.y0 + p.y * r.height())
    }

    fn norm(p: Pt, r: &Rect) -> Pt {
        let w = if r.width().abs() < 1e-9 { 1.0 } else { r.width() };
        let h = if r.height().abs() < 1e-9 { 1.0 } else { r.height() };
        Pt::new((p.x - r.x0) / w, (p.y - r.y0) / h)
    }

    /// Anfang und Ende der Verlaufslinie im Dokument.
    pub fn line(&self, r: &Rect) -> (Pt, Pt) {
        (Self::denorm(self.p0, r), Self::denorm(self.p1, r))
    }

    /// Verlaufslinie aus Dokumentkoordinaten setzen.
    pub fn set_line(&mut self, a: Pt, b: Pt, r: &Rect) {
        self.p0 = Self::norm(a, r);
        self.p1 = Self::norm(b, r);
    }

    /// Winkel der Verlaufslinie im Dokument (Grad, gegen den Uhrzeigersinn, 0 = nach rechts).
    pub fn angle(&self, r: &Rect) -> f64 {
        let (a, b) = self.line(r);
        let d = b - a;
        if d.len() < 1e-9 {
            return 0.0;
        }
        normalize_angle((-d.y).atan2(d.x).to_degrees())
    }

    /// Winkel setzen: Die Linie läuft durch ihre bisherige Mitte und deckt das Objekt ganz ab.
    pub fn set_angle(&mut self, deg: f64, r: &Rect) {
        let (a, b) = self.line(r);
        let c = Pt::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
        let t = deg.to_radians();
        let dir = Pt::new(t.cos(), -t.sin());
        let len = ((r.width() * t.cos()).abs() + (r.height() * t.sin()).abs()) * (1.0 - 2.0 * GRADIENT_INSET);
        let half = dir * (len.max(1.0) / 2.0);
        self.set_line(c - half, c + half, r);
    }

    /// Verlauf mit dem Objekt transformieren (Rahmen vorher und nachher).
    pub fn transform(&mut self, m: &Affine, before: &Rect, after: &Rect) {
        let (a, b) = self.line(before);
        self.set_line(m.apply(a), m.apply(b), after);
    }

    /// Neuen Farbstopp an Stelle `t` (Farbe wie dort im Verlauf); liefert seinen Index.
    pub fn add_stop(&mut self, t: f64) -> usize {
        let t = t.clamp(0.0, 1.0);
        let color = self.color_at(t);
        self.stops.push(Stop { pos: t, color });
        self.stops.len() - 1
    }
}

/// Grundlinie für Text auf Pfad: wie `path_polyline`, auf Wunsch in umgekehrter Richtung.
pub fn textpath_line(path: &[SubPath], reverse: bool) -> (Vec<Pt>, Vec<f64>) {
    let (mut pts, len) = path_polyline(path);
    if !reverse {
        return (pts, len);
    }
    pts.reverse();
    let mut len = Vec::with_capacity(pts.len());
    len.push(0.0);
    for w in pts.windows(2) {
        let l = len[len.len() - 1] + w[0].dist(w[1]);
        len.push(l);
    }
    (pts, len)
}

/// Ist die Grundlinie geschlossen (dann läuft der Text um den Anfang herum weiter)?
pub fn path_is_closed(path: &[SubPath]) -> bool {
    path.first().map(|s| s.closed).unwrap_or(false)
}

/// Bogenlänge des Ankerpunkts von Text auf Pfad (Griffposition) je nach Ausrichtung.
pub fn textpath_anchor(total: f64, offset: f64, align: Align) -> f64 {
    match align {
        Align::Left | Align::Justify => offset,
        Align::Center => total / 2.0 + offset,
        Align::Right => total - offset,
    }
}

/// Umkehrung von `textpath_anchor`: Versatz aus der Bogenlänge des Ankers.
pub fn textpath_offset(total: f64, anchor: f64, align: Align) -> f64 {
    match align {
        Align::Left | Align::Justify => anchor,
        Align::Center => anchor - total / 2.0,
        Align::Right => total - anchor,
    }
}

// ───────────────────────────── Hilfslinien ─────────────────────────────

/// Unendlich lange Hilfslinie durch `pos` mit Winkel `angle`
/// (Grad gegen den Uhrzeigersinn: 0 = waagerecht, 90 = senkrecht).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guide {
    pub pos: Pt,
    pub angle: f64,
    /// Ebene, auf der die Hilfslinie liegt (0 = keine: immer sichtbar)
    pub layer: u64,
}

impl Guide {
    pub fn dir(&self) -> Pt {
        let t = self.angle.to_radians();
        Pt::new(t.cos(), -t.sin())
    }
    pub fn is_horizontal(&self) -> bool {
        normalize_angle(self.angle).abs() < 1e-6 || (normalize_angle(self.angle).abs() - 180.0).abs() < 1e-6
    }
    pub fn is_vertical(&self) -> bool {
        (normalize_angle(self.angle).abs() - 90.0).abs() < 1e-6
    }
    /// Lotfußpunkt von `p` auf der Linie.
    pub fn project(&self, p: Pt) -> Pt {
        let d = self.dir();
        let v = p - self.pos;
        let t = v.x * d.x + v.y * d.y;
        self.pos + d * t
    }
    pub fn dist(&self, p: Pt) -> f64 {
        self.project(p).dist(p)
    }
    /// Schnittpunkt zweier Hilfslinien (None, wenn parallel).
    pub fn intersect(&self, o: &Guide) -> Option<Pt> {
        let (d1, d2) = (self.dir(), o.dir());
        let den = d1.x * d2.y - d1.y * d2.x;
        if den.abs() < 1e-9 {
            return None;
        }
        let w = o.pos - self.pos;
        let t = (w.x * d2.y - w.y * d2.x) / den;
        Some(self.pos + d1 * t)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub id: u64,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    /// Index in `LAYER_COLORS` (Farbe der Auswahlmarkierung).
    pub color: usize,
    pub objects: Vec<Object>,
}

/// Markierungsfarben der Ebenen (jede Ebene hat ihre eigene Farbe).
pub const LAYER_COLORS: [Color; 9] = [
    Color::rgb(0.208, 0.518, 0.894), // Blau
    Color::rgb(0.878, 0.106, 0.141), // Rot
    Color::rgb(0.180, 0.761, 0.494), // Grün
    Color::rgb(1.000, 0.471, 0.000), // Orange
    Color::rgb(0.569, 0.255, 0.675), // Violett
    Color::rgb(0.898, 0.647, 0.039), // Gelb
    Color::rgb(0.000, 0.627, 0.627), // Türkis
    Color::rgb(1.000, 0.357, 0.678), // Rosa
    Color::rgb(0.596, 0.416, 0.267), // Braun
];

pub fn layer_color(i: usize) -> Color {
    LAYER_COLORS[i % LAYER_COLORS.len()]
}

impl Layer {
    pub fn editable(&self) -> bool {
        self.visible && !self.locked
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub width: f64,
    pub height: f64,
    pub background: Option<Color>,
    /// Index 0 ist die unterste Ebene.
    pub layers: Vec<Layer>,
    pub next_id: u64,
    /// Bevorzugte Maßeinheit für Lineale ("px", "mm", "cm"; leer = Voreinstellung).
    pub unit: String,
    pub guides: Vec<Guide>,
    /// Weitere Zeichenflächen. Die erste (Haupt-)Zeichenfläche liegt immer bei (0, 0)
    /// und hat die Größe `width` × `height`.
    pub artboards: Vec<Artboard>,
}

/// Zusätzliche Zeichenfläche im selben Dokument.
#[derive(Clone, Debug, PartialEq)]
pub struct Artboard {
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// eigener Hintergrund (None = durchsichtig); die erste Zeichenfläche nutzt
    /// `Document::background`
    pub background: Option<Color>,
}

/// Gleiche Farbe (bis auf Rundung beim Speichern als #rrggbb).
/// Verknüpfung einer Farbe mit einer Bibliotheksfarbe (globale Farben): Kennung und die
/// Bibliotheksfarbe zur Zeit der letzten Übernahme. Hat das Objekt inzwischen einen anderen
/// Farbton, gilt die Verknüpfung nicht mehr – so kann keine Änderung auf anderem Weg
/// (Pipette, Umwandeln, eingefügter Stil …) später überschrieben werden.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorLink {
    pub id: String,
    pub color: Color,
}

impl ColorLink {
    pub fn new(id: &str, color: Color) -> ColorLink {
        ColorLink {
            id: id.to_string(),
            color,
        }
    }

    /// Gilt die Verknüpfung noch für die Farbe `paint`? (gleicher Farbton; die Deckkraft
    /// darf das Objekt selbst haben)
    pub fn holds(&self, paint: Option<Color>) -> bool {
        paint.is_some_and(|c| same_rgb(c, self.color))
    }

    /// Der Bibliotheksfarbe `new` folgen: `None`, wenn die Verknüpfung nicht mehr gilt;
    /// `Some(None)`, wenn sich nichts ändert; sonst die neue Farbe des Objekts. Eine eigene
    /// Deckkraft des Objekts bleibt erhalten.
    pub fn follow(&mut self, paint: Option<Color>, new: Color) -> Option<Option<Color>> {
        let c = paint.filter(|c| same_rgb(*c, self.color))?;
        let old = std::mem::replace(&mut self.color, new);
        if same_color(old, new) {
            return Some(None);
        }
        let a = if (c.a - old.a).abs() < 0.003 { new.a } else { c.a };
        Some(Some(Color { a, ..new }))
    }
}

fn same_rgb(a: Color, b: Color) -> bool {
    (a.r - b.r).abs() < 0.003 && (a.g - b.g).abs() < 0.003 && (a.b - b.b).abs() < 0.003
}

pub fn same_color(a: Color, b: Color) -> bool {
    (a.r - b.r).abs() < 0.003
        && (a.g - b.g).abs() < 0.003
        && (a.b - b.b).abs() < 0.003
        && (a.a - b.a).abs() < 0.003
}

/// Größte Koordinate, die ein Objekt haben darf (Dokumenteinheiten). Größere Werte (oder
/// NaN/unendlich) kommen nur aus beschädigten oder präparierten Dateien und brächten das
/// Zeichnen durcheinander.
pub const MAX_COORD: f64 = 1e7;

fn ok_num(v: f64) -> bool {
    v.is_finite() && v.abs() <= MAX_COORD
}

fn ok_pt(p: &Pt) -> bool {
    ok_num(p.x) && ok_num(p.y)
}

fn ok_affine(m: &crate::geom::Affine) -> bool {
    [m.a, m.b, m.c, m.d]
        .iter()
        .all(|v| v.is_finite() && v.abs() <= 1e6)
        && ok_num(m.e)
        && ok_num(m.f)
}

fn ok_subpaths(sp: &[SubPath]) -> bool {
    sp.iter().all(|s| {
        s.nodes
            .iter()
            .all(|n| ok_pt(&n.p) && ok_pt(&n.h_in) && ok_pt(&n.h_out) && ok_num(n.r))
    })
}

/// Zahl auf einen Bereich bringen; NaN wird zu `def`.
fn tame(v: f64, lo: f64, hi: f64, def: f64) -> f64 {
    if v.is_nan() {
        def
    } else {
        v.clamp(lo, hi)
    }
}

fn tame_color(c: &mut Color) {
    for v in [&mut c.r, &mut c.g, &mut c.b, &mut c.a] {
        *v = tame(*v, 0.0, 1.0, 1.0);
    }
}

fn tame_format(f: &mut TextFormat) {
    f.line = tame(f.line, 0.1, 100.0, 1.2);
    f.tracking = tame(f.tracking, -10_000.0, 10_000.0, 0.0);
    f.para = tame(f.para, 0.0, 100.0, 0.5);
    f.box_width = tame(f.box_width, 0.0, 1e5, 1.0);
    f.box_inset = tame(f.box_inset, 0.0, 1e5, 0.0);
    for c in [&mut f.box_fill, &mut f.box_stroke].into_iter().flatten() {
        tame_color(c);
    }
}

/// Gradient brauchbar machen; `false`, wenn er sich nicht retten lässt.
fn tame_gradient(g: &mut Gradient) -> bool {
    if !ok_pt(&g.p0) || !ok_pt(&g.p1) || g.stops.is_empty() {
        return false;
    }
    g.cell = tame(g.cell, 0.5, 1e5, 16.0);
    for st in &mut g.stops {
        st.pos = tame(st.pos, 0.0, 1.0, 0.0);
        tame_color(&mut st.color);
    }
    true
}

/// Lage und Größe eines Objekts sind brauchbar (endlich und nicht riesig)?
fn shape_is_sane(s: &Shape) -> bool {
    let size = |v: f64| v.is_finite() && v > 0.0 && v <= 1e5;
    match s {
        Shape::Rect { x, y, w, h, r } => [*x, *y, *w, *h, *r].iter().all(|v| ok_num(*v)),
        Shape::Ellipse { cx, cy, rx, ry } => [*cx, *cy, *rx, *ry].iter().all(|v| ok_num(*v)),
        Shape::Line { a, b } => ok_pt(a) && ok_pt(b),
        Shape::Path { subpaths, .. } => ok_subpaths(subpaths),
        Shape::Text { xf, size: sz, .. } => ok_affine(xf) && size(*sz),
        Shape::TextPath {
            path,
            size: sz,
            offset,
            ..
        } => ok_subpaths(path) && size(*sz) && ok_num(*offset),
        Shape::TextArea {
            xf,
            w,
            h,
            size: sz,
            frame,
            ..
        } => {
            ok_affine(xf)
                && ok_num(*w)
                && ok_num(*h)
                && size(*sz)
                && frame.path.as_ref().map_or(true, |p| ok_subpaths(p))
        }
        Shape::Image { xf, w, h, clip, .. } => {
            ok_affine(xf) && size(*w) && size(*h) && clip.as_ref().map_or(true, |c| ok_subpaths(c))
        }
    }
}

impl Document {
    /// Werte aus beschädigten oder präparierten Dateien unschädlich machen: unbrauchbare
    /// Größen der Seiten ersetzen, Zahlen begrenzen und Objekte mit unbrauchbarer Lage oder
    /// Größe entfernen. Liefert die Zahl der entfernten Objekte.
    pub fn sanitize(&mut self) -> usize {
        let page = |v: f64| {
            if v.is_finite() && v > 0.0 {
                v.clamp(1.0, 1e6)
            } else {
                1000.0
            }
        };
        self.width = page(self.width);
        self.height = page(self.height);
        if let Some(c) = self.background.as_mut() {
            tame_color(c);
        }
        for a in &mut self.artboards {
            a.x = if ok_num(a.x) { a.x } else { 0.0 };
            a.y = if ok_num(a.y) { a.y } else { 0.0 };
            a.w = page(a.w);
            a.h = page(a.h);
            if let Some(c) = a.background.as_mut() {
                tame_color(c);
            }
        }
        self.guides.retain(|g| ok_pt(&g.pos) && g.angle.is_finite());
        let mut removed = 0;
        for layer in &mut self.layers {
            let before = layer.objects.len();
            layer.objects.retain(|o| shape_is_sane(&o.shape));
            removed += before - layer.objects.len();
            for o in &mut layer.objects {
                if !o.angle.is_finite() {
                    o.angle = 0.0;
                }
                match &mut o.shape {
                    Shape::Text { fmt, .. } | Shape::TextPath { fmt, .. } => tame_format(fmt),
                    Shape::TextArea { fmt, frame, rich, .. } => {
                        tame_format(fmt);
                        frame.width = tame(frame.width, 0.0, 1e5, 1.0);
                        frame.inset = tame(frame.inset, 0.0, 1e5, 0.0);
                        for c in [&mut frame.fill, &mut frame.stroke].into_iter().flatten() {
                            tame_color(c);
                        }
                        for run in &mut rich.runs {
                            if let Some(sz) = run.style.size.as_mut() {
                                *sz = tame(*sz, 0.1, 1e5, 12.0);
                            }
                        }
                    }
                    _ => {}
                }
                let st = &mut o.style;
                st.width = tame(st.width, 0.0, 1e5, 1.0);
                st.opacity = tame(st.opacity, 0.0, 1.0, 1.0);
                st.marker_size = tame(st.marker_size, 0.1, 20.0, 1.0);
                for c in [&mut st.fill, &mut st.stroke].into_iter().flatten() {
                    tame_color(c);
                }
                if let Some(sh) = st.shadow.as_mut() {
                    tame_color(&mut sh.color);
                    sh.opacity = tame(sh.opacity, 0.0, 1.0, 0.35);
                    sh.blur = tame(sh.blur, 0.0, 1e4, 8.0);
                    sh.dx = tame(sh.dx, -1e5, 1e5, 0.0);
                    sh.dy = tame(sh.dy, -1e5, 1e5, 0.0);
                }
                for g in [&mut st.gradient, &mut st.stroke_gradient, &mut st.shadow_gradient] {
                    if g.as_mut().is_some_and(|g| !tame_gradient(g)) {
                        *g = None;
                    }
                }
            }
        }
        removed
    }
}

impl Document {
    /// Alle Objekte der Gruppe von `id` (oder nur `id`, wenn es zu keiner gehört).
    pub fn group_members(&self, id: u64) -> Vec<u64> {
        match self.object(id).map(|o| o.group) {
            Some(g) if g != 0 => self
                .layers
                .iter()
                .flat_map(|l| l.objects.iter())
                .filter(|o| o.group == g)
                .map(|o| o.id)
                .collect(),
            _ => vec![id],
        }
    }

    /// Größte vergebene Gruppennummer (auf allen Ebenen).
    pub fn max_group(&self) -> u64 {
        self.layers
            .iter()
            .flat_map(|l| l.objects.iter())
            .flat_map(|o| std::iter::once(o.group).chain(o.sub.iter().copied()))
            .max()
            .unwrap_or(0)
    }

    /// Verknüpfte Farben auf den Stand der Bibliotheken bringen (`lookup`: Kennung →
    /// aktuelle Farbe; unbekannte Kennungen bleiben, wie sie sind). Liefert die Zahl
    /// geänderter Farben.
    pub fn sync_linked_colors(&mut self, lookup: &dyn Fn(&str) -> Option<Color>) -> usize {
        let mut n = 0;
        for o in self.layers.iter_mut().flat_map(|l| l.objects.iter_mut()) {
            let st = &mut o.style;
            for (paint, lk) in [
                (&mut st.fill, &mut st.fill_link),
                (&mut st.stroke, &mut st.stroke_link),
            ] {
                let Some(l) = lk.as_mut() else { continue };
                let Some(new) = lookup(&l.id) else { continue };
                match l.follow(*paint, new) {
                    Some(Some(c)) => {
                        *paint = Some(c);
                        n += 1;
                    }
                    Some(None) => {}
                    None => *lk = None,
                }
            }
        }
        n
    }

    /// Schnittmasken des Dokuments (innerste Gruppe → Maske als Fläche).
    pub fn masks(&self) -> HashMap<u64, Object> {
        masks_of(self.layers.iter().flat_map(|l| l.objects.iter()))
    }

    /// Gruppen aufräumen: Gruppen mit nur einem Objekt entfallen (auf jeder Ebene), eine
    /// Schnittmaske ohne andere Objekte in ihrer Gruppe wird ein gewöhnliches Objekt.
    pub fn tidy_groups(&mut self) {
        let mut count: HashMap<u64, usize> = HashMap::new();
        for o in self.layers.iter().flat_map(|l| l.objects.iter()) {
            for g in o.chain() {
                *count.entry(g).or_default() += 1;
            }
        }
        let n = |g: &u64| count.get(g).copied().unwrap_or(0);
        for o in self.layers.iter_mut().flat_map(|l| l.objects.iter_mut()) {
            if o.mask && n(&o.inner_group()) < 2 {
                o.mask = false;
            }
            o.sub.retain(|g| n(g) > 1);
            if o.group != 0 && n(&o.group) < 2 {
                o.group = 0;
                o.sub.clear();
            }
        }
    }

    /// Eine Gruppenebene entfernen (wo sie auch liegt): ihre Objekte und Untergruppen
    /// gehören danach zur Gruppe darüber.
    pub fn remove_group_level(&mut self, gid: u64) {
        if gid == 0 {
            return;
        }
        for o in self.layers.iter_mut().flat_map(|l| l.objects.iter_mut()) {
            if o.group == gid {
                o.group = if o.sub.is_empty() { 0 } else { o.sub.remove(0) };
            } else {
                o.sub.retain(|g| *g != gid);
            }
        }
        self.tidy_groups();
    }

    /// Alle Zeichenflächen: (Name, Rahmen); Index 0 ist die Hauptzeichenfläche.
    pub fn artboard_rects(&self) -> Vec<(String, Rect)> {
        let mut v = vec![(
            tr("Artboard 1").to_string(),
            Rect::from_xywh(0.0, 0.0, self.width, self.height),
        )];
        for a in &self.artboards {
            v.push((a.name.clone(), Rect::from_xywh(a.x, a.y, a.w, a.h)));
        }
        v
    }

    /// Hintergründe der Zeichenflächen (in derselben Reihenfolge wie `artboard_rects`).
    pub fn artboard_backgrounds(&self) -> Vec<Option<Color>> {
        std::iter::once(self.background)
            .chain(self.artboards.iter().map(|a| a.background))
            .collect()
    }

    /// Hintergrund einer Zeichenfläche setzen (0 = erste).
    pub fn set_artboard_background(&mut self, i: usize, bg: Option<Color>) {
        if i == 0 {
            self.background = bg;
        } else if let Some(a) = self.artboards.get_mut(i - 1) {
            a.background = bg;
        }
    }

    /// Rahmen um alle Zeichenflächen.
    pub fn artboards_bbox(&self) -> Rect {
        self.artboard_rects()
            .into_iter()
            .map(|(_, r)| r)
            .reduce(|a, b| a.union(&b))
            .unwrap_or(Rect::from_xywh(0.0, 0.0, self.width, self.height))
    }

    /// Freier Name „Zeichenfläche n“.
    pub fn next_artboard_name(&self) -> String {
        let names: Vec<String> = self.artboard_rects().into_iter().map(|(n, _)| n).collect();
        let mut k = names.len() + 1;
        loop {
            let n = trf("Artboard {}", &[&(k)]);
            if !names.contains(&n) {
                return n;
            }
            k += 1;
        }
    }

    /// Dokument für eine einzelne Zeichenfläche (zum Exportieren): alles so verschoben,
    /// dass sie bei (0, 0) liegt, ohne weitere Zeichenflächen.
    pub fn artboard_doc(&self, i: usize) -> Document {
        let rects = self.artboard_rects();
        let Some((_, r)) = rects.get(i) else {
            return self.clone();
        };
        let mut d = self.clone();
        d.background = self.artboard_backgrounds().get(i).copied().flatten();
        d.artboards.clear();
        d.width = r.width();
        d.height = r.height();
        if r.x0 != 0.0 || r.y0 != 0.0 {
            let t = Pt::new(-r.x0, -r.y0);
            for l in &mut d.layers {
                for o in &mut l.objects {
                    o.shape.translate(t);
                }
            }
            for g in &mut d.guides {
                g.pos = g.pos + t;
            }
        }
        d
    }

    pub fn new(width: f64, height: f64, background: Option<Color>) -> Document {
        let mut d = Document {
            width,
            height,
            background,
            layers: Vec::new(),
            next_id: 1,
            unit: String::new(),
            guides: Vec::new(),
            artboards: Vec::new(),
        };
        let id = d.alloc_id();
        d.layers.push(Layer {
            color: 0,
            id,
            name: tr("Layer 1").into(),
            visible: true,
            locked: false,
            objects: Vec::new(),
        });
        d
    }

    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Stellt sicher, dass `next_id` größer als jede vergebene ID ist.
    pub fn fix_ids(&mut self) {
        let mut max = 0;
        for l in &self.layers {
            max = max.max(l.id);
            for o in &l.objects {
                max = max.max(o.id);
            }
        }
        self.next_id = self.next_id.max(max + 1);
    }

    /// Hilfslinie sichtbar (ihre Ebene ist eingeblendet)?
    pub fn guide_visible(&self, g: &Guide) -> bool {
        g.layer == 0 || self.layer(g.layer).map(|l| l.visible).unwrap_or(true)
    }

    /// Hilfslinie gesperrt (ihre Ebene ist gesperrt)?
    pub fn guide_locked(&self, g: &Guide) -> bool {
        g.layer != 0 && self.layer(g.layer).map(|l| l.locked).unwrap_or(false)
    }

    pub fn layer(&self, id: u64) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }
    pub fn layer_mut(&mut self, id: u64) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }
    pub fn layer_index(&self, id: u64) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }

    /// (Ebenenindex, Objektindex) eines Objekts.
    pub fn locate(&self, id: u64) -> Option<(usize, usize)> {
        for (li, l) in self.layers.iter().enumerate() {
            if let Some(oi) = l.objects.iter().position(|o| o.id == id) {
                return Some((li, oi));
            }
        }
        None
    }
    pub fn object(&self, id: u64) -> Option<&Object> {
        let (li, oi) = self.locate(id)?;
        Some(&self.layers[li].objects[oi])
    }
    pub fn object_mut(&mut self, id: u64) -> Option<&mut Object> {
        let (li, oi) = self.locate(id)?;
        Some(&mut self.layers[li].objects[oi])
    }
    pub fn layer_of(&self, id: u64) -> Option<&Layer> {
        let (li, _) = self.locate(id)?;
        Some(&self.layers[li])
    }
    pub fn remove_object(&mut self, id: u64) -> Option<Object> {
        let (li, oi) = self.locate(id)?;
        Some(self.layers[li].objects.remove(oi))
    }

    /// Verschiebt ein Objekt innerhalb seiner Ebene. `delta` > 0 nach vorne.
    /// `to_end` setzt es ganz nach vorne bzw. ganz nach hinten.
    pub fn restack(&mut self, id: u64, delta: i32, to_end: bool) {
        let Some((li, oi)) = self.locate(id) else { return };
        let objs = &mut self.layers[li].objects;
        let len = objs.len();
        let target = if to_end {
            if delta > 0 {
                len - 1
            } else {
                0
            }
        } else {
            (oi as i64 + delta as i64).clamp(0, len as i64 - 1) as usize
        };
        if target != oi {
            let o = objs.remove(oi);
            objs.insert(target, o);
        }
    }

    /// Ebene verschieben: `delta` > 0 nach oben (vorne).
    pub fn move_layer(&mut self, id: u64, delta: i32) {
        let Some(i) = self.layer_index(id) else { return };
        let t = (i as i64 + delta as i64).clamp(0, self.layers.len() as i64 - 1) as usize;
        if t != i {
            let l = self.layers.remove(i);
            self.layers.insert(t, l);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_patterns_tile_seamlessly() {
        for shape in GradShape::PATTERNS {
            assert!(shape.is_pattern());
            assert_eq!(GradShape::from_name(shape.name()), Some(shape));
            let Some(t) = shape.pattern_tile(16.0) else {
                assert!(matches!(
                    shape,
                    GradShape::Checker | GradShape::Dots | GradShape::Stripes
                ));
                continue;
            };
            assert!(t.w >= 16.0 && t.h >= 16.0 && !t.shapes.is_empty(), "{shape:?}");
            // jede Fläche berührt die Kachel
            for sh in &t.shapes {
                let pts: Vec<&Pt> = sh.contours.iter().flatten().collect();
                assert!(pts.len() >= 3);
                assert!(pts
                    .iter()
                    .any(|q| q.x > -2.0 * t.w && q.x < 3.0 * t.w && q.y > -2.0 * t.h && q.y < 3.0 * t.h));
            }
        }
        // Zebrastreifen: linker und rechter Rand passen zusammen
        let t = GradShape::Zebra.pattern_tile(10.0).unwrap();
        let p = &t.shapes[0].contours[0];
        let n = p.len() / 2;
        assert!((p[0].y - p[n - 1].y).abs() < 1e-9 && (p[n - 1].x - t.w).abs() < 1e-9);
        // Vichy-Karo: nur zwei Farben (alles voll deckend), Schraffur nur in den Streifen
        let g = GradShape::Gingham.pattern_tile(10.0).unwrap();
        assert!(g.shapes.iter().all(|s| s.alpha == 1.0 && s.ink));
        assert!(g.shapes.len() > 8);
        for sh in &g.shapes[1..] {
            for p in &sh.contours[0] {
                // nie im leeren Feld rechts unten
                assert!(!(p.x > 10.0 + 1e-9 && p.y > 10.0 + 1e-9), "{p:?}");
            }
        }
        assert!(GradShape::Checker.pattern_tile(10.0).is_none());
    }

    #[test]
    fn new_effects_round_trip() {
        let e = Effects {
            mat: Some((Material::Brushed, 0.6)),
            mat_angle: 30.0,
            art: Some((Art::Halftone, 0.3)),
            glow: Some(0.7),
            inner: Some(0.4),
            inner_dist: 7.0,
            inner_angle: -30.0,
            mirror_len: 0.4,
            mirror_gap: 10.0,
            art_size: 12.0,
            art_angle: 15.0,
            ..Effects::default()
        };
        let back = Effects::from_attr(&e.to_attr());
        assert_eq!(back, e);
        assert!(!back.uses_backdrop() && back.needs_glass());
        let g = Effects {
            glass: Some(0.8),
            glass_grain: 0.0,
            ..Effects::default()
        };
        assert_eq!(Effects::from_attr(&g.to_attr()), g);
        // ältere Dateien: Milchglas als Textur wird Glas, Papier entfällt (matt)
        assert_eq!(Effects::from_attr("tex=frost:0.4").glass, Some(0.4));
        assert_eq!(Effects::from_attr("frost=0.7").glass, Some(0.7));
        assert_eq!(Effects::from_attr("tex=paper:0.5"), Effects::default());
        assert_eq!(Effects::from_attr("tex=wood:0.5 tex=fur:0.3"), Effects::default());
        assert_eq!(
            Effects::from_attr("tex=brushed:0.5").mat,
            Some((Material::Brushed, 0.5))
        );
        assert_eq!(
            Effects::from_attr("art=riso:0.5").art,
            Some((Art::Risograph, 0.5))
        );
        assert_eq!(Effects::from_attr("art=crayon:0.5").art, Some((Art::Crayon, 0.5)));
        // Vorgaben werden nicht gespeichert
        assert_eq!(Effects::default().to_attr(), "");
    }

    #[test]
    fn text_frame_padding_scales_with_text() {
        let mut fmt = TextFormat::default();
        fmt.box_inset = 10.0;
        let mut t = Shape::Text {
            xf: Affine::IDENTITY,
            text: "Hallo".into(),
            family: "Sans".into(),
            size: 20.0,
            bold: false,
            italic: false,
            fmt,
        };
        t.transform(&Affine::scale(2.0, 2.0));
        match &t {
            Shape::Text { size, fmt, .. } => {
                assert!((*size - 40.0).abs() < 1e-9);
                assert!((fmt.box_inset - 20.0).abs() < 1e-9);
            }
            _ => panic!(),
        }
        // Verschieben ändert nichts
        t.transform(&Affine::translate(5.0, 5.0));
        assert!((t.text_fields().unwrap().2 - 40.0).abs() < 1e-9);
    }

    #[test]
    fn rich_runs() {
        let base = CharFmt {
            family: "Sans".into(),
            size: 12.0,
            bold: false,
            weight: 400,
            italic: false,
            underline: false,
            strike: false,
        };
        let mut r = RichText::default();
        // „Hallo Welt“: „Welt“ fett
        r.apply_chars(10, &base, Some((6, 10)), &|f| f.bold = true);
        assert_eq!(r.runs.len(), 1);
        assert_eq!((r.runs[0].start, r.runs[0].end), (6, 10));
        let (_, mixed) = r.format_in(10, &base, 0, 10);
        assert!(mixed.bold && !mixed.family);
        // am Ende weiterschreiben: fett geht weiter
        r.edit("Hallo Welt", "Hallo Welt!");
        assert_eq!(r.runs[0].end, 11);
        // vorne einfügen verschiebt
        r.edit("Hallo Welt!", "Ja Hallo Welt!");
        assert_eq!((r.runs[0].start, r.runs[0].end), (9, 14));
        // ganzes Feld fett: Grundformat ändert sich, Teile verschwinden
        let nb = r.apply_chars(14, &base, None, &|f| f.bold = true);
        assert!(nb.bold);
        assert!(r.runs.is_empty());
        // Schrift für das ganze Feld gleicht abweichende Teile an
        let mut r2 = RichText::default();
        r2.apply_chars(10, &base, Some((0, 4)), &|f| f.family = "Serif".into());
        r2.apply_chars(10, &base, None, &|f| f.family = "Mono".into());
        assert!(r2.runs.is_empty());
        // Teil löschen
        let mut r3 = RichText::default();
        r3.apply_chars(10, &base, Some((2, 6)), &|f| f.italic = true);
        r3.edit("0123456789", "0189");
        assert!(r3.runs.is_empty());
    }

    #[test]
    fn rich_paras() {
        let mut r = RichText::default();
        let t = "eins\nzwei\ndrei";
        r.apply_paras(t, Some((6, 6)), ListStyle::None, &|p| p.list = ListStyle::Number);
        assert_eq!(r.para(1, ListStyle::None).list, ListStyle::Number);
        assert_eq!(r.para(0, ListStyle::None).list, ListStyle::None);
        // Enter im zweiten Absatz: neuer Absatz übernimmt die Aufzählung
        r.edit(t, "eins\nzw\nei\ndrei");
        assert_eq!(r.paras.len(), 4);
        assert_eq!(r.para(2, ListStyle::None).list, ListStyle::Number);
        assert_eq!(r.para(3, ListStyle::None).list, ListStyle::None);
        // Umbruch löschen
        r.edit("eins\nzw\nei\ndrei", "einszw\nei\ndrei");
        assert_eq!(r.paras.len(), 3);
        assert_eq!(r.para(0, ListStyle::None).list, ListStyle::None);
        let (d, map) = para_display("ab", "1.");
        assert_eq!(d, "1.\tab");
        assert_eq!(list_to_display(&map, 1), 4);
        assert_eq!(list_from_display(&map, 1), 0);
    }

    #[test]
    fn colors() {
        assert_eq!(Color::parse("#fff"), Some(Color::WHITE));
        assert_eq!(Color::parse("#3584e4").unwrap().to_hex(), "#3584e4");
        // nicht-ASCII nach dem #: kein Absturz
        for s in ["#\u{fffd}a", "#a\u{fffd}", "#\u{e9}\u{e9}\u{e9}", "#ab\u{1f600}c"] {
            assert_eq!(Color::parse(s), None, "{s}");
        }
        let c = Color::parse("rgba(255, 0, 0, 0.5)").unwrap();
        assert!((c.r - 1.0).abs() < 1e-9 && (c.a - 0.5).abs() < 1e-9);
        assert_eq!(Color::parse("navy").unwrap().to_hex(), "#000080");
    }

    #[test]
    fn cmyk_roundtrip() {
        let c = Color::rgb(0.2, 0.5, 0.8);
        let (cc, m, y, k) = c.to_cmyk();
        let back = Color::from_cmyk(cc, m, y, k, 1.0);
        assert!((back.r - 0.2).abs() < 1e-9 && (back.g - 0.5).abs() < 1e-9 && (back.b - 0.8).abs() < 1e-9);
        assert_eq!(Color::BLACK.to_cmyk(), (0.0, 0.0, 0.0, 1.0));
    }

    #[test]
    fn fillet_square() {
        let sq = match (Shape::Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
            r: 0.0,
        })
        .to_path()
        .unwrap()
        {
            Shape::Path { subpaths, .. } => subpaths,
            _ => unreachable!(),
        };
        let (f, idx) = fillet(&sq[0], &[0, 1, 2, 3], 10.0);
        assert_eq!(f.nodes.len(), 8);
        assert_eq!(idx, vec![0, 1, 2, 3, 4, 5, 6, 7]);
        let area = crate::boolean::total_area(&[f.clone()]);
        let expect = 10000.0 - (4.0 - std::f64::consts::PI) * 100.0;
        assert!((area - expect).abs() < 1.0, "{area} vs {expect}");
        // Radius größer als halbe Kante wird begrenzt
        let (g, _) = fillet(&sq[0], &[0], 500.0);
        assert!(g.nodes.iter().all(|n| n.p.x >= -1e-9 && n.p.y >= -1e-9));
    }

    #[test]
    fn text_scale_goes_into_size() {
        let mut t = Shape::text_at(Pt::new(10.0, 10.0), "A", "Sans", 20.0, false, false);
        t.transform(&Affine::scale_about(2.0, 2.0, Pt::new(0.0, 0.0)));
        match t {
            Shape::Text { xf, size, .. } => {
                assert!((size - 40.0).abs() < 1e-9);
                assert!((xf.a - 1.0).abs() < 1e-9 && (xf.e - 20.0).abs() < 1e-9);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn rect_rotate_becomes_path() {
        let mut s = Shape::Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            r: 0.0,
        };
        s.transform(&Affine::rotate_deg(45.0));
        assert!(matches!(s, Shape::Path { .. }));
    }

    #[test]
    fn ellipse_bbox_via_path() {
        let e = Shape::Ellipse {
            cx: 50.0,
            cy: 50.0,
            rx: 20.0,
            ry: 10.0,
        };
        let p = e.to_path().unwrap();
        let b = p.geom_bbox().unwrap();
        assert!((b.x0 - 30.0).abs() < 1e-6 && (b.y1 - 60.0).abs() < 1e-6);
    }

    #[test]
    fn restacking() {
        let mut d = Document::new(100.0, 100.0, None);
        for _ in 0..3 {
            let id = d.alloc_id();
            d.layers[0].objects.push(Object {
                group: 0,
                sub: Vec::new(),
                mask: false,
                locked: false,
                name: String::new(),
                id,
                shape: Shape::Line {
                    a: Pt::new(0.0, 0.0),
                    b: Pt::new(1.0, 1.0),
                },
                style: Style::default(),
                angle: 0.0,
            });
        }
        let first = d.layers[0].objects[0].id;
        d.restack(first, 1, true);
        assert_eq!(d.layers[0].objects[2].id, first);
        d.restack(first, -1, false);
        assert_eq!(d.layers[0].objects[1].id, first);
    }
    #[test]
    fn trim_and_ends() {
        let sp = SubPath {
            nodes: vec![Node::corner(Pt::new(0.0, 0.0)), Node::corner(Pt::new(100.0, 0.0))],
            closed: false,
        };
        let t = trim_subpath(&sp, 10.0, 20.0);
        assert!((t.nodes[0].p.x - 10.0).abs() < 0.1, "{:?}", t.nodes[0].p);
        assert!((t.nodes[1].p.x - 80.0).abs() < 0.1, "{:?}", t.nodes[1].p);
        let ((a, da), (b, db)) = subpath_ends(&sp).unwrap();
        assert_eq!(a, Pt::new(0.0, 0.0));
        assert!((da.x + 1.0).abs() < 1e-9 && (db.x - 1.0).abs() < 1e-9);
        assert_eq!(b, Pt::new(100.0, 0.0));
    }
}
