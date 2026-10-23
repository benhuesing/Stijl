// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! Editorzustand und Werkzeuglogik, unabhängig von GTK (dadurch testbar).
//!
//! Die Oberfläche übergibt Zeigerereignisse in Bildschirmkoordinaten; Schriftmetriken
//! und Trefferprüfung kommen über das `Metrics`-Trait aus dem Cairo-Renderer.

use crate::i18n::{tr, trf};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::boolean::{self, Op};
use crate::geom::{simplify, Affine, Pt, Rect};
use crate::model::{
    corner_at, normalize_angle, Align, Blend, CharFmt, Color, Corner, Document, FmtMixed, Gradient,
    GradientKind, Guide, ImagePart, Layer, Node, Object, PaintTarget, ParaFmt, RichText, Shape, ShapeKind,
    Style, SubPath, TextFormat, TextFrame,
};

const MAX_UNDO: usize = 200;
/// Fangradius für Anfasser und Knoten in Bildschirmpixeln.
pub const HANDLE_RADIUS: f64 = 7.0;

pub trait Metrics {
    /// Geometrischer Rahmen eines Objekts (ohne Strichbreite).
    fn bbox(&self, obj: &Object) -> Option<Rect>;
    /// Trifft der Punkt `p` (Dokumentkoordinaten) das Objekt? `tol` in Dokumenteinheiten.
    fn hit(&self, obj: &Object, p: Pt, tol: f64) -> bool;
    /// Umriss eines Textes als Pfad (Glyphenkonturen).
    fn text_outline(&self, obj: &Object) -> Option<Vec<SubPath>>;
    /// Text, der ein Textfeld genau füllt (Blindtext, so oft wiederholt wie nötig).
    fn fill_area(&self, _obj: &Object) -> Option<String> {
        None
    }
    /// Zeichenposition (Byte-Index) im Text unter dem Punkt `p`.
    fn text_index(&self, _obj: &Object, _p: Pt) -> Option<usize> {
        None
    }
    /// Die vier Ecken des (ggf. gedrehten) Rahmens eines Textes.
    fn text_box(&self, obj: &Object) -> Option<[Pt; 4]> {
        let b = self.bbox(obj)?;
        Some([
            Pt::new(b.x0, b.y0),
            Pt::new(b.x1, b.y0),
            Pt::new(b.x1, b.y1),
            Pt::new(b.x0, b.y1),
        ])
    }
}

/// So lange (Sekunden) muss ein Bild über einer Form ruhen, bis es in sie hineinspringt.
/// Voreinstellung: so lange (Sekunden) muss ein Bild über einer Form ruhen.
pub const FRAME_DWELL: f64 = 0.6;
/// Zeichenstift: so weit (Bildschirmpixel) darf man beim Klicken ziehen, ohne dass der
/// Punkt abgerundet wird.
pub const PEN_DEADZONE: f64 = 8.0;
/// So weit (Bildschirmpixel) darf der Zeiger beim Klicken wackeln, ohne dass daraus ein
/// Ziehen wird (Verschieben, Anfasser, Knoten, Aufziehen …).
pub const CLICK_SLOP: f64 = 5.0;
/// Fangradius der Hilfslinien beim Verschieben (Bildschirmpixel); etwas größer als für
/// andere Objekte, damit die Auswahl spürbar „hängen bleibt“.
pub const GUIDE_SNAP: f64 = 10.0;

/// Liegt `p` im (konvexen) Viereck `q`?
fn in_quad(q: &[Pt; 4], p: Pt) -> bool {
    let mut sign = 0.0;
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        let c = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        if c.abs() < 1e-12 {
            continue;
        }
        if sign == 0.0 {
            sign = c.signum();
        } else if c.signum() != sign {
            return false;
        }
    }
    true
}

/// Einrasthilfe zum Anzeigen: Linie von `a` nach `b`, optional mit Beschriftung bei `b`.
#[derive(Clone, Debug)]
pub struct SnapHint {
    pub a: Pt,
    pub b: Pt,
    pub label: Option<&'static str>,
}

/// Einrastziele: markante Punkte, Kanten-/Mittelkoordinaten und Pfade anderer Objekte.
#[derive(Clone, Debug, Default)]
struct SnapCtx {
    points: Vec<(Pt, &'static str)>,
    xs: Vec<(f64, Pt)>,
    ys: Vec<(f64, Pt)>,
    paths: Vec<Vec<Pt>>,
    /// Rahmen der anderen Objekte (für gleiche Abstände), ohne Seiten
    boxes: Vec<Rect>,
}

/// Lage eines Bildes (Pixelgröße `w`×`h`), das den Rahmen `fb` proportional ausfüllt.
pub fn frame_fit(w: f64, h: f64, fb: &Rect) -> Affine {
    let sc = (fb.width() / w).max(fb.height() / h).max(1e-6);
    let cx = (fb.x0 + fb.x1) / 2.0;
    let cy = (fb.y0 + fb.y1) / 2.0;
    Affine::translate(cx - w * sc / 2.0, cy - h * sc / 2.0).then_before(&Affine::scale(sc, sc))
}

/// Abstand des Drehgriffs über der Auswahl in Bildschirmpixeln.
pub const ROTATE_OFFSET: f64 = 26.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Node,
    Shape,
    Pen,
    Pencil,
    Text,
    TextPath,
    TextArea,
    Curvature,
    Guide,
    /// Radierer (Umschalt E)
    Eraser,
    /// Schere (C): eine gezogene Linie zerschneidet Formen und Pfade
    Scissors,
    /// Formerstellung (Umschalt M)
    Builder,
}

impl Tool {
    #[allow(dead_code)]
    pub fn from_key(c: char) -> Option<Tool> {
        Some(match c.to_ascii_lowercase() {
            'v' => Tool::Select,
            'a' => Tool::Node,
            'r' => Tool::Shape,
            'p' => Tool::Pen,
            'n' => Tool::Pencil,
            't' => Tool::Text,
            'g' => Tool::Guide,
            'k' => Tool::Builder,
            'c' => Tool::Scissors,
            _ => return None,
        })
    }
    pub fn cursor(self) -> &'static str {
        match self {
            Tool::Select | Tool::Node => "default",
            Tool::Text | Tool::TextPath | Tool::TextArea => "text",
            _ => "crosshair",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// Fläche eines Linienendes (wie `render::draw_marker`); `dir` zeigt vom Pfad nach außen.
fn marker_area(tip: Pt, dir: Pt, w: f64, size: f64, mk: crate::model::Marker) -> Vec<boolean::Ring> {
    use crate::model::{LineCap, LineJoin, Marker, Node};
    use crate::stroker::{stroke_area, StrokeStyle};
    let l = mk.length(w, size);
    let half = l * 0.4;
    let perp = Pt::new(-dir.y, dir.x);
    let line = |pts: &[Pt], join: LineJoin, cap: LineCap| {
        let sp = SubPath {
            nodes: pts.iter().map(|p| Node::corner(*p)).collect(),
            closed: false,
        };
        stroke_area(
            &[sp],
            &StrokeStyle {
                width: w,
                join,
                cap,
                dash: Vec::new(),
            },
        )
    };
    match mk {
        Marker::Arrow => {
            let base = tip - dir * l;
            boolean::rings_op(
                &[vec![tip, base + perp * half, base - perp * half]],
                &[],
                Op::Union,
            )
        }
        Marker::OpenArrow => {
            let base = tip - dir * (l * 0.8);
            line(
                &[base + perp * half, tip, base - perp * half],
                LineJoin::Round,
                LineCap::Round,
            )
        }
        Marker::Dot => {
            let r = (w * 1.6).max(2.5) * size.clamp(0.1, 20.0);
            let n = 64;
            let ring: Vec<Pt> = (0..n)
                .map(|i| {
                    let t = i as f64 / n as f64 * std::f64::consts::TAU;
                    Pt::new(tip.x + r * t.cos(), tip.y + r * t.sin())
                })
                .collect();
            boolean::rings_op(&[ring], &[], Op::Union)
        }
        Marker::Bar => line(
            &[tip + perp * half, tip - perp * half],
            LineJoin::Miter,
            LineCap::Butt,
        ),
        Marker::None => Vec::new(),
    }
}

/// Teilpfad als Punktfolge (Kurven in `n` Stücke geteilt; geschlossene enden am Anfang).
pub fn flatten_subpath(sp: &SubPath, n: usize) -> Vec<Pt> {
    let k = sp.nodes.len();
    if k == 0 {
        return Vec::new();
    }
    let mut out = vec![sp.nodes[0].p];
    let segs = if sp.closed { k } else { k - 1 };
    for i in 0..segs {
        let a = sp.nodes[i];
        let b = sp.nodes[(i + 1) % k];
        let straight = a.h_out.dist(a.p) < 1e-9 && b.h_in.dist(b.p) < 1e-9;
        if straight {
            out.push(b.p);
            continue;
        }
        for j in 1..=n {
            let t = j as f64 / n as f64;
            let u = 1.0 - t;
            let p = a.p * (u * u * u)
                + a.h_out * (3.0 * u * u * t)
                + b.h_in * (3.0 * u * t * t)
                + b.p * (t * t * t);
            out.push(p);
        }
    }
    out
}

/// Punkt im Vieleck (gerade-ungerade-Regel)?
fn point_in_poly(p: Pt, poly: &[Pt]) -> bool {
    let mut inside = false;
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn poly_area(poly: &[Pt]) -> f64 {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum::<f64>()
        .abs()
        / 2.0
}

/// Umriss eines Textes in einzelne Buchstaben aufteilen: Löcher (o, a …) bleiben bei
/// ihrem Buchstaben, ebenso übereinanderliegende Teile wie i-Punkte, Umlaute, „:“ oder „=“.
/// `frame`: Lage des Textes (dort läuft er waagerecht), `em`: Schriftgröße.
pub fn glyph_parts(subs: Vec<SubPath>, frame: Option<Affine>, em: f64) -> Vec<Vec<SubPath>> {
    let back = frame.map(|f| f.invert()).unwrap_or(Affine::IDENTITY);
    let polys: Vec<Vec<Pt>> = subs
        .iter()
        .map(|sp| {
            flatten_subpath(sp, 8)
                .into_iter()
                .map(|p| back.apply(p))
                .collect()
        })
        .collect();
    let areas: Vec<f64> = polys.iter().map(|p| poly_area(p)).collect();
    let n = subs.len();
    // jeder Teilpfad gehört zum kleinsten größeren Teilpfad, in dem er liegt (Löcher, ®)
    let parent: Vec<Option<usize>> = (0..n)
        .map(|i| {
            let p0 = *polys[i].first()?;
            (0..n)
                .filter(|&j| j != i && areas[j] > areas[i] && point_in_poly(p0, &polys[j]))
                .min_by(|&a, &b| areas[a].total_cmp(&areas[b]))
        })
        .collect();
    let root = |mut i: usize| {
        let mut guard = 0;
        while let Some(p) = parent[i] {
            i = p;
            guard += 1;
            if guard > n {
                break;
            }
        }
        i
    };
    // Inseln: ein äußerer Umriss mit allem, was darin liegt
    let mut island_of: Vec<usize> = (0..n).map(root).collect();
    let bbox = |idx: &[usize]| -> Option<Rect> {
        idx.iter()
            .flat_map(|&i| polys[i].iter().copied())
            .fold(None, |r: Option<Rect>, p| {
                Some(match r {
                    Some(r) => r.union(&Rect::from_points(p, p)),
                    None => Rect::from_points(p, p),
                })
            })
    };
    let mut roots: Vec<usize> = island_of.clone();
    roots.sort_unstable();
    roots.dedup();
    let boxes: Vec<Option<Rect>> = roots
        .iter()
        .map(|&r| bbox(&(0..n).filter(|&i| island_of[i] == r).collect::<Vec<_>>()))
        .collect();
    // Teile desselben Buchstabens zusammenfassen
    let mut merged: Vec<usize> = (0..roots.len()).collect();
    fn find(m: &mut [usize], i: usize) -> usize {
        let mut i = i;
        while m[i] != i {
            m[i] = m[m[i]];
            i = m[i];
        }
        i
    }
    let overlap = |a0: f64, a1: f64, b0: f64, b1: f64| (a1.min(b1) - a0.max(b0)).max(0.0);
    for a in 0..roots.len() {
        for b in a + 1..roots.len() {
            let (Some(ra), Some(rb)) = (boxes[a], boxes[b]) else {
                continue;
            };
            let join = if frame.is_some() {
                // übereinander (i-Punkt, Umlaut, Doppelpunkt), mit kleinem Abstand
                let xo = overlap(ra.x0, ra.x1, rb.x0, rb.x1);
                let yo = overlap(ra.y0, ra.y1, rb.y0, rb.y1);
                let minw = ra.width().min(rb.width());
                let minh = ra.height().min(rb.height());
                let gap = (rb.y0 - ra.y1).max(ra.y0 - rb.y1).max(0.0);
                xo >= 0.5 * minw && yo <= 0.2 * minh && gap < 0.3 * em
            } else {
                // Text auf Pfad: kleine Teile zum nahen Buchstaben
                let small = |r: Rect| r.width().max(r.height()) < 0.25 * em;
                let (ca, cb) = (
                    Pt::new((ra.x0 + ra.x1) / 2.0, (ra.y0 + ra.y1) / 2.0),
                    Pt::new((rb.x0 + rb.x1) / 2.0, (rb.y0 + rb.y1) / 2.0),
                );
                (small(ra) != small(rb)) && ca.dist(cb) < 0.45 * em
            };
            if join {
                let (fa, fb) = (find(&mut merged, a), find(&mut merged, b));
                if fa != fb {
                    merged[fb.max(fa)] = fb.min(fa);
                }
            }
        }
    }
    for i in 0..n {
        let k = roots.iter().position(|&r| r == island_of[i]).unwrap_or(0);
        island_of[i] = find(&mut merged, k);
    }
    // Reihenfolge wie im Text (erster Teilpfad jedes Buchstabens)
    let mut order: Vec<usize> = island_of.clone();
    order.dedup();
    let mut seen = Vec::new();
    for g in order {
        if !seen.contains(&g) {
            seen.push(g);
        }
    }
    let mut out: Vec<Vec<SubPath>> = vec![Vec::new(); seen.len()];
    for (i, sp) in subs.into_iter().enumerate() {
        let k = seen.iter().position(|&g| g == island_of[i]).unwrap_or(0);
        out[k].push(sp);
    }
    out.retain(|v| !v.is_empty());
    out
}

/// Wert, der beim Ziehen neben dem Zeiger erscheint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Badge {
    /// Eckenradius (Dokumenteinheiten)
    Radius(f64),
    /// Drehwinkel in Grad
    Angle(f64),
}

/// Abstand der Seite zum Fensterrand in der Notizheft-Ansicht (Bildschirmpixel).
const PAGE_MARGIN: f64 = 24.0;
/// Scrollweg über das Seitenende hinaus, ab dem umgeblättert wird.
const PAGE_FLIP: f64 = 90.0;

#[derive(Clone, Copy, Debug)]
pub struct View {
    pub zoom: f64,
    pub ox: f64,
    pub oy: f64,
}

impl View {
    pub fn to_doc(&self, s: Pt) -> Pt {
        Pt::new((s.x - self.ox) / self.zoom, (s.y - self.oy) / self.zoom)
    }
    pub fn to_screen(&self, p: Pt) -> Pt {
        Pt::new(p.x * self.zoom + self.ox, p.y * self.zoom + self.oy)
    }
}

/// Ein Knoten im Knotenwerkzeug: (Objekt, Teilpfad, Knoten).
pub type NodeRef = (u64, usize, usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Anchor,
    In,
    Out,
}

#[derive(Clone, Debug)]
pub enum Drag {
    None,
    Pan {
        s0: Pt,
        ox: f64,
        oy: f64,
    },
    Move {
        start: Pt,
        orig: Vec<(u64, Object)>,
        /// Rahmen der Auswahl beim Start (zum Einrasten an Hilfslinien).
        bbox: Option<Rect>,
        /// mit Alt: an der alten Stelle zurückgelassene Kopien
        copies: Vec<u64>,
    },
    Scale {
        handle: usize,
        /// Rahmen im (ggf. gedrehten) System des Auswahlrahmens
        bbox: Rect,
        orig: Vec<(u64, Object)>,
        uniform: bool,
        /// Dokument → System des Rahmens (Identität bei ungedrehter Auswahl)
        frame: Affine,
    },
    Rotate {
        center: Pt,
        start_angle: f64,
        orig: Vec<(u64, Object)>,
    },
    /// Zeichenfläche am Namen verschieben (mit allem, was ganz auf ihr liegt)
    Artboard {
        idx: usize,
        start: Pt,
        orig: Box<Document>,
        view: View,
        inside: Vec<u64>,
    },
    /// Zeichenfläche an einem Anfasser vergrößern/verkleinern (nur im Zeichenflächen-Modus)
    ArtboardSize {
        idx: usize,
        handle: usize,
        rect: Rect,
        orig: Box<Document>,
        view: View,
    },
    /// Radierer: Spur des Pinsels
    Erase {
        pts: Vec<Pt>,
    },
    /// Formerstellung: Spur, getroffene Teilflächen, Alt = löschen
    /// Formerstellung: überstrichene Flächen (Nummern im ebenen Graphen)
    Build {
        pts: Vec<Pt>,
        hits: Vec<usize>,
        alt: bool,
    },
    Rubber {
        start: Pt,
        cur: Pt,
        base: Vec<u64>,
    },
    Create {
        id: u64,
        start: Pt,
        was_dirty: bool,
    },
    Pencil {
        pts: Vec<Pt>,
    },
    PenHandle,
    /// Knoten oder Anfasser ziehen. `handle` = None: alle gewählten Knoten verschieben
    /// (auch in mehreren Objekten). `orig`: Ausgangsform jedes beteiligten Objekts.
    NodeEdit {
        start: Pt,
        orig: Vec<(u64, Vec<SubPath>)>,
        handle: Option<(u64, usize, usize, Part)>,
        mirror: bool,
        active: bool,
    },
    /// Eckenradius über den runden Ziehpunkt einstellen.
    Fillet {
        corner: Corner,
        off: f64,
    },
    /// Textfeld aufziehen.
    AreaCreate {
        id: u64,
        start: Pt,
        was_dirty: bool,
    },
    /// Startpunkt von Text auf Pfad verschieben.
    TextOffset {
        id: u64,
    },
    /// Text mit der Maus markieren.
    TextSelect {
        id: u64,
    },
    /// Rahmen zum Auswählen von Knoten (auch verschiedener Objekte).
    NodeRubber {
        start: Pt,
        cur: Pt,
        base: Vec<NodeRef>,
        /// Objektauswahl beim Start
        base_objs: Vec<u64>,
        /// mit Umschalt: zur Auswahl hinzufügen
        add: bool,
        had_nodes: bool,
    },
    /// Anfang (`end` = 0) oder Ende (1) der Verlaufslinie ziehen; `r` = Objektrahmen.
    GradientEnd {
        id: u64,
        end: usize,
        r: Rect,
    },
    /// Farbstopp auf der Verlaufslinie verschieben.
    GradientStop {
        id: u64,
        idx: usize,
        r: Rect,
    },
    /// Hilfslinie verschieben.
    Guide {
        idx: usize,
        start: Pt,
        orig: Pt,
    },
    /// Neue Hilfslinie: Ziehen legt den Winkel fest (Strg: 45°-Schritte).
    GuideNew {
        idx: usize,
        origin: Pt,
    },
}

/// Bearbeitungszustand: Cursor und Markierungsanfang als Byte-Index im Text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub id: u64,
    pub caret: usize,
    pub anchor: usize,
}

impl TextEdit {
    pub fn range(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }
}

/// Cursorbewegungen beim Bearbeiten.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    WordLeft,
    WordRight,
}

fn prev_boundary(t: &str, i: usize) -> usize {
    t[..i].chars().next_back().map(|c| i - c.len_utf8()).unwrap_or(0)
}

fn next_boundary(t: &str, i: usize) -> usize {
    t[i..].chars().next().map(|c| i + c.len_utf8()).unwrap_or(t.len())
}

fn line_start(t: &str, i: usize) -> usize {
    t[..i].rfind('\n').map(|p| p + 1).unwrap_or(0)
}

fn line_end(t: &str, i: usize) -> usize {
    t[i..].find('\n').map(|p| i + p).unwrap_or(t.len())
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Wortgrenzen um `i` (für Doppelklick).
pub fn word_at(t: &str, i: usize) -> (usize, usize) {
    let mut a = i.min(t.len());
    while a > 0 {
        let p = prev_boundary(t, a);
        if !t[p..a].chars().all(is_word) {
            break;
        }
        a = p;
    }
    let mut b = i.min(t.len());
    while b < t.len() {
        let n = next_boundary(t, b);
        if !t[b..n].chars().all(is_word) {
            break;
        }
        b = n;
    }
    (a, b)
}

/// Blindtext für neue Textfelder.
pub const LOREM: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.";

/// Glatte Kurve durch Punkte wie beim Kurvenzeichner; `true` markiert Eckpunkte.
pub fn curvature_nodes(pts: &[(Pt, bool)], closed: bool) -> Vec<Node> {
    let n = pts.len();
    (0..n)
        .map(|i| {
            let (p, corner) = pts[i];
            let prev = if i > 0 {
                Some(pts[i - 1].0)
            } else if closed && n > 2 {
                Some(pts[n - 1].0)
            } else {
                None
            };
            let next = if i + 1 < n {
                Some(pts[i + 1].0)
            } else if closed && n > 2 {
                Some(pts[0].0)
            } else {
                None
            };
            if corner {
                return Node::corner(p);
            }
            let dir = match (prev, next) {
                (Some(a), Some(b)) => (b - a).normalized(),
                (None, Some(b)) => (b - p).normalized(),
                (Some(a), None) => (p - a).normalized(),
                (None, None) => return Node::corner(p),
            };
            Node {
                r: 0.0,
                p,
                h_in: prev.map(|a| p - dir * (a.dist(p) / 3.0)).unwrap_or(p),
                h_out: next.map(|b| p + dir * (b.dist(p) / 3.0)).unwrap_or(p),
            }
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextDefaults {
    pub family: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub fmt: TextFormat,
}

/// Was die Kontextleiste anzeigen soll.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextInfo {
    pub show_shape: bool,
    /// einzelnes ausgewähltes Vieleck/Stern: (Ecken, Stern?)
    pub poly: Option<(u32, bool)>,
    pub show_radius: bool,
    pub radius: f64,
    pub show_text: bool,
    pub text: Option<String>,
    pub font: TextDefaults,
    /// uneinheitliche Zeichenformate (Felder bleiben dann leer)
    pub font_mixed: FmtMixed,
    /// Absatzformat an Markierung/Cursor
    pub para: ParaFmt,
    pub show_node: bool,
    pub node_count: usize,
    /// Aktueller Eckenradius der laufenden Abrundung.
    pub node_radius: f64,
    /// Abstand des Textes vom Pfadanfang (nur Text auf Pfad).
    pub text_offset: Option<f64>,
    pub can_convert: bool,
    pub show_style: bool,
    /// Füllung bzw. Kontur anbieten (bei Bildern ohne bzw. mit Rahmen eingeschränkt).
    pub show_fill: bool,
    pub show_stroke: bool,
    /// Auswahlwerkzeug ohne Auswahl: nur ein Hinweis.
    pub empty_hint: bool,
    /// Linienenden anbieten (offene Pfade, Zeichenwerkzeuge ohne Auswahl).
    pub show_markers: bool,
    /// Freihand: Glättung (0–1) und Abstand zum automatischen Schließen (Bildschirmpixel).
    pub show_pencil: bool,
    pub show_eraser: bool,
    pub eraser_size: f64,
    pub pencil_smooth: f64,
    pub pencil_close: f64,
    pub style: Style,
    pub has_selection: bool,
    pub can_boolean: bool,
    /// Überschrift der Eigenschaftenleiste (z. B. „Rechteck“, „3 Objekte“).
    pub title: String,
    /// Vertikale Ausrichtung anbieten (Textfelder).
    pub show_valign: bool,
    /// Aufzählungen möglich (rechteckige Textfelder)
    pub show_list: bool,
    /// Drehwinkel des ersten gewählten Objekts.
    pub angle: f64,
    /// Ausgewählter Farbstopp in der Verlaufsleiste.
    pub grad_stop: usize,
    pub show_guides_panel: bool,
    /// Gewählte Hilfslinie (Winkel, Position) bzw. Winkel für neue Hilfslinien.
    pub guide: Option<Guide>,
    pub guide_angle: f64,
    /// Einzelnes Bild gewählt: (hat Rahmen, bearbeiteter Teil).
    pub image: Option<(bool, ImagePart)>,
    /// Rahmen des ersten gewählten Textfeldes.
    pub text_frame: Option<TextFrame>,
    /// Auswahl besteht aus Text (Füllung/Kontur heißen dann Text…).
    pub is_text: bool,
}

pub struct Editor {
    pub doc: Document,
    pub selection: Vec<u64>,
    pub active_layer: u64,
    undo: Vec<Document>,
    redo: Vec<Document>,
    pending: Option<Document>,
    last_key: Option<(String, Instant, Vec<u64>)>,
    pub dirty: bool,
    /// Wird erhöht, wenn sich die Ebenenliste ändert (Oberfläche baut sie dann neu auf).
    pub layers_version: u64,

    pub tool: Tool,
    pub shape_kind: ShapeKind,
    pub corners: u32,
    pub corner_radius: f64,
    pub defaults: Style,
    pub text_style: Style,
    pub text_defaults: TextDefaults,

    pub view: View,
    pub needs_fit: bool,
    /// Notizheft-Ansicht: nur diese Zeichenfläche wird gezeigt und bearbeitet
    pub notebook: Option<usize>,
    /// Ausrichten mehrerer Objekte aneinander (sonst an der Seite)
    pub align_selection: bool,
    /// Scrollweg über das Seitenende hinaus (blättert ab einer Schwelle um)
    notebook_acc: f64,
    pub drag: Drag,
    pub pen: Option<Vec<Node>>,
    /// Gewählte Knoten (Objekt, Teilpfad, Knoten) – auch über mehrere Objekte.
    pub node_sel: Vec<NodeRef>,
    /// Punkte des Kurvenzeichners (Punkt, Eckpunkt?).
    pub curv: Option<Vec<(Pt, bool)>>,
    /// Ort des letzten Drückens (Bildschirm); `None`, sobald der Zeiger weiter als
    /// `CLICK_SLOP` davon weg war – erst dann wirkt das Ziehen.
    press_at: Option<Pt>,
    /// Zeichenflächen bearbeiten (Ebenen-/Zeichenflächenleiste offen): verschieben am
    /// Namen und Größe an den Anfassern ändern
    pub artboard_mode: bool,
    pub hover: Option<u64>,
    pub pointer: Option<Pt>,
    pub space_down: bool,
    pub clipboard: Vec<Object>,
    paste_count: u32,
    /// Die Oberfläche soll das Texteingabefeld fokussieren.
    pub request_text_focus: bool,
    /// Text wird direkt im Dokument bearbeitet.
    pub text_edit: Option<TextEdit>,
    /// Format fürs Weiterschreiben am Cursor (Objekt, Cursor, Format)
    pending_fmt: Option<(u64, usize, CharFmt)>,
    /// Textfelder, die noch Blindtext enthalten (füllen sich beim Größenändern neu).
    placeholders: Vec<u64>,
    /// Meldung für die Oberfläche (z. B. „Ebene ist gesperrt“).
    pub message: Option<String>,
    /// Ausgewählter Farbstopp des Verlaufs (Index in `stops`).
    pub grad_stop: usize,
    /// Welcher Verlauf bearbeitet wird (Füllung, Kontur, Schatten)
    pub grad_target: PaintTarget,
    /// Wert zum Anzeigen neben dem Zeiger beim Ziehen (Eckenradius, Drehwinkel)
    pub badge: Option<(Pt, Badge)>,
    /// Ausgewählte Hilfslinie.
    pub guide_sel: Option<usize>,
    /// Winkel für neue Hilfslinien (0 = waagerecht, 90 = senkrecht).
    pub guide_angle: f64,
    pub show_guides: bool,
    pub snap_guides: bool,
    /// Breite der Lineale in Bildschirmpixeln (0 = ausgeblendet); aus ihnen lassen sich Hilfslinien ziehen.
    pub ruler: f64,
    /// Hilfslinien gesperrt (nicht verschieb- oder löschbar).
    pub lock_guides: bool,
    /// Gerahmtes Bild: welcher Teil bearbeitet wird (Bild-ID, Teil).
    pub frame_mode: Option<(u64, ImagePart)>,
    /// Bild schwebt über einer Form: (Form, seit wann, Bildschirmpunkt beim Start).
    pub frame_hover: Option<(u64, Instant, Pt)>,
    /// Aus der Bibliothek wird etwas über ein Objekt gezogen: (Ziel, seit, was)
    pub lib_drop: Option<(u64, Instant, LibPayload)>,
    /// Wartezeit (s), bis ein Bild/Textfeld in eine Form springt.
    pub frame_dwell: f64,
    /// Einrasten an Objekten und der Seite überhaupt (Hauptschalter; die Fenster-Einstellung
    /// schaltet ihn ein, sobald eines der Ziele darunter oder die Abstände aktiv ist).
    pub smart_snap: bool,
    /// Ziele beim Einrasten an Objekten: Ecken, Mitten (Mitte und Kantenmitten),
    /// Ankerpunkte, die Seite und die Konturen (Pfade).
    pub snap_corners: bool,
    pub snap_mids: bool,
    pub snap_anchors: bool,
    pub snap_page: bool,
    pub snap_paths: bool,
    /// Beim Verschieben einrasten, wenn Lücken zu Nachbarn gleich groß sind wie andere
    /// Lücken in derselben Reihe (setzt `smart_snap` voraus).
    pub snap_spacing: bool,
    /// Am Raster einrasten; Rasterweite in Dokumenteinheiten.
    pub grid_snap: bool,
    pub grid_size: f64,
    /// Einrastziele, beim Drücken gesammelt.
    snap_ctx: Option<SnapCtx>,
    /// Einrastziele für die Vorschau ohne gedrückte Taste (gilt bis zur nächsten Änderung)
    hover_ctx: RefCell<Option<(u64, SnapCtx)>>,
    /// zählt Änderungen am Dokument (für zwischengespeicherte Einrastziele)
    rev: u64,
    /// zählt jede Änderung (auch zusammengefasste), z. B. für die Wiederherstellungsdatei
    edits: u64,
    /// Anzeige der Einrasthilfen (Linien und Beschriftung).
    pub hints: RefCell<Vec<SnapHint>>,
    /// Freihand: Glättung 0–1.
    pub pencil_smooth: f64,
    /// Freihand: Liegen Anfang und Ende näher als so viele Bildschirmpixel, wird die Form geschlossen.
    pub pencil_close: f64,
    /// Durchmesser des Radierers (Dokumenteinheiten)
    pub eraser_size: f64,
    /// beim Zeigen hervorgehobene Fläche der Formerstellung
    builder_hover: RefCell<Option<usize>>,
    /// Größe der Zeichenfläche auf dem Bildschirm (für die Formerstellung)
    pub viewport: (f64, f64),
    /// Formerstellung: ebener Graph aller Umrisse (zwischengespeichert)
    bucket: RefCell<Option<BucketCache>>,
}

impl Editor {
    pub fn new(doc: Document) -> Editor {
        let active = doc.layers.last().map(|l| l.id).unwrap_or(0);
        Editor {
            doc,
            selection: Vec::new(),
            active_layer: active,
            undo: Vec::new(),
            redo: Vec::new(),
            pending: None,
            last_key: None,
            dirty: false,
            layers_version: 1,
            tool: Tool::Select,
            shape_kind: ShapeKind::Rect,
            corners: 5,
            corner_radius: 0.0,
            defaults: Style::default(),
            text_style: Style {
                fill: Some(Color::rgb(0.11, 0.11, 0.11)),
                stroke: None,
                width: 1.0,
                opacity: 1.0,
                ..Style::default()
            },
            text_defaults: TextDefaults {
                // Grundschrift des Systems (Cantarell gibt es nur unter GNOME)
                family: if cfg!(windows) {
                    "Segoe UI"
                } else if cfg!(target_os = "macos") {
                    "Helvetica Neue"
                } else {
                    "Cantarell"
                }
                .into(),
                size: 36.0,
                bold: false,
                italic: false,
                fmt: TextFormat::default(),
            },
            view: View {
                zoom: 1.0,
                ox: 0.0,
                oy: 0.0,
            },
            needs_fit: true,
            notebook: None,
            notebook_acc: 0.0,
            align_selection: true,
            drag: Drag::None,
            pen: None,
            node_sel: Vec::new(),
            curv: None,
            press_at: None,
            artboard_mode: false,
            hover: None,
            pointer: None,
            space_down: false,
            clipboard: Vec::new(),
            paste_count: 0,
            request_text_focus: false,
            text_edit: None,
            pending_fmt: None,
            placeholders: Vec::new(),
            message: None,
            grad_stop: 0,
            grad_target: PaintTarget::Fill,
            badge: None,
            guide_sel: None,
            guide_angle: 0.0,
            show_guides: true,
            snap_guides: true,
            ruler: 0.0,
            lock_guides: false,
            frame_mode: None,
            frame_hover: None,
            lib_drop: None,
            frame_dwell: FRAME_DWELL,
            smart_snap: true,
            snap_corners: true,
            snap_mids: true,
            snap_anchors: true,
            snap_page: true,
            snap_paths: true,
            snap_spacing: true,
            grid_snap: false,
            grid_size: 20.0,
            snap_ctx: None,
            hover_ctx: RefCell::new(None),
            rev: 0,
            edits: 0,
            hints: std::cell::RefCell::new(Vec::new()),
            pencil_smooth: 0.5,
            pencil_close: 15.0,
            eraser_size: 20.0,
            builder_hover: RefCell::new(None),
            viewport: (1200.0, 800.0),
            bucket: RefCell::new(None),
        }
    }

    /// Neues oder geöffnetes Dokument übernehmen.
    pub fn set_document(&mut self, doc: Document) {
        self.active_layer = doc.layers.last().map(|l| l.id).unwrap_or(0);
        self.doc = doc;
        self.selection.clear();
        self.undo.clear();
        self.redo.clear();
        self.pending = None;
        self.last_key = None;
        self.dirty = false;
        self.drag = Drag::None;
        self.pen = None;
        self.node_sel.clear();
        self.hover = None;
        self.needs_fit = true;
        self.layers_version += 1;
        self.text_edit = None;
        self.placeholders.clear();
        self.guide_sel = None;
        self.grad_stop = 0;
    }

    // ───────────── Rückgängig ─────────────

    /// Zustand vor einer Änderung sichern. Gleiche `key`s kurz hintereinander werden zusammengefasst.
    pub fn checkpoint(&mut self, key: Option<&str>) {
        let now = Instant::now();
        if let (Some(k), Some((lk, t, sel))) = (key, &self.last_key) {
            if k == lk && now.duration_since(*t) < Duration::from_millis(1500) && *sel == self.selection {
                self.last_key = Some((k.to_string(), now, self.selection.clone()));
                self.dirty = true;
                self.edits += 1;
                return;
            }
        }
        self.push_undo(self.doc.clone());
        self.last_key = key.map(|k| (k.to_string(), now, self.selection.clone()));
        self.dirty = true;
        self.edits += 1;
    }

    fn push_undo(&mut self, d: Document) {
        self.rev += 1;
        self.undo.push(d);
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Beginn einer Geste: Zustand merken, aber erst bei tatsächlicher Änderung sichern.
    fn begin_change(&mut self) {
        self.pending = Some(self.doc.clone());
    }
    fn touch(&mut self) {
        if let Some(d) = self.pending.take() {
            self.push_undo(d);
            self.last_key = None;
        }
        self.dirty = true;
        self.edits += 1;
    }
    fn end_change(&mut self) {
        self.pending = None;
    }

    /// Änderungszähler (steigt bei jeder Änderung am Dokument).
    pub fn edits(&self) -> u64 {
        self.edits
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        self.finish_pen(false);
        self.rev += 1;
        match self.undo.pop() {
            Some(d) => {
                let cur = std::mem::replace(&mut self.doc, d);
                self.redo.push(cur);
                self.after_history();
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        self.rev += 1;
        match self.redo.pop() {
            Some(d) => {
                let cur = std::mem::replace(&mut self.doc, d);
                self.undo.push(cur);
                self.after_history();
                true
            }
            None => false,
        }
    }

    fn after_history(&mut self) {
        self.text_edit = None;
        if self
            .guide_sel
            .map(|i| i >= self.doc.guides.len())
            .unwrap_or(false)
        {
            self.guide_sel = None;
        }
        self.last_key = None;
        self.dirty = true;
        self.edits += 1;
        self.drag = Drag::None;
        self.node_sel.clear();
        if self.doc.layer(self.active_layer).is_none() {
            self.active_layer = self.doc.layers.last().map(|l| l.id).unwrap_or(0);
        }
        self.prune_selection();
        self.layers_version += 1;
    }

    // ───────────── Auswahl ─────────────

    /// Entfernt gelöschte oder gesperrte Objekte aus der Auswahl.
    pub fn prune_selection(&mut self) {
        let doc = &self.doc;
        self.selection
            .retain(|id| doc.layer_of(*id).map(|l| l.editable()).unwrap_or(false));
        if self.selection.len() != 1 {
            self.node_sel.clear();
        }
    }

    pub fn selected_objects(&self) -> Vec<&Object> {
        self.selection
            .iter()
            .filter_map(|id| self.doc.object(*id))
            .collect()
    }

    pub fn selection_bbox(&self, m: &dyn Metrics) -> Option<Rect> {
        // Inhalt eines gerahmten Bildes: Rahmen um das ganze Bild
        if let [id] = self.selection[..] {
            if self.image_part(id) == ImagePart::Content {
                let q = self.doc.object(id)?.shape.image_quad()?;
                let mut r = Rect::from_points(q[0], q[1]);
                r.include(q[2]);
                r.include(q[3]);
                return Some(r);
            }
        }
        self.selected_objects()
            .into_iter()
            .filter_map(|o| m.bbox(o))
            .reduce(|a, b| a.union(&b))
    }

    pub fn select_all(&mut self) {
        self.selection = self
            .doc
            .layers
            .iter()
            .filter(|l| l.editable())
            .flat_map(|l| l.objects.iter().map(|o| o.id))
            .collect();
        self.node_sel.clear();
    }

    pub fn deselect(&mut self) {
        self.selection.clear();
        self.node_sel.clear();
    }

    /// Kopien der Objekte im Ausgangszustand direkt unter den Originalen einfügen
    /// (Alt beim Verschieben). Gruppen bekommen neue Nummern.
    fn leave_copies(&mut self, orig: &[(u64, Object)]) -> Vec<u64> {
        let mut next_group = self.doc.max_group();
        let mut regroup: Vec<(u64, u64)> = Vec::new();
        let mut ids = Vec::new();
        for (id, ob) in orig {
            let Some((li, oi)) = self.doc.locate(*id) else {
                continue;
            };
            let mut c = ob.clone();
            c.id = self.doc.alloc_id();
            c.locked = false;
            if c.group != 0 {
                c.group = match regroup.iter().find(|(old, _)| *old == c.group) {
                    Some((_, new)) => *new,
                    None => {
                        next_group += 1;
                        regroup.push((c.group, next_group));
                        next_group
                    }
                };
            }
            ids.push(c.id);
            self.doc.layers[li].objects.insert(oi, c);
        }
        ids
    }

    /// Ausgewählte Objekte in Stapelreihenfolge (unten zuerst).
    fn selected_in_order(&self) -> Vec<Object> {
        let mut v: Vec<((usize, usize), Object)> = self
            .selection
            .iter()
            .filter_map(|id| Some((self.doc.locate(*id)?, self.doc.object(*id)?.clone())))
            .collect();
        v.sort_by_key(|(k, _)| *k);
        v.into_iter().map(|(_, o)| o).collect()
    }

    /// Ergänzt die Auswahl um alle übrigen Objekte angeschnittener Gruppen.
    pub fn expand_groups(&mut self) {
        let mut extra = Vec::new();
        for id in &self.selection {
            for mid in self.doc.group_members(*id) {
                if !self.selection.contains(&mid) && !extra.contains(&mid) {
                    extra.push(mid);
                }
            }
        }
        self.selection.extend(extra);
    }

    /// Ist etwas Gesperrtes ausgewählt?
    pub fn selection_locked(&self) -> bool {
        self.selected_objects().iter().any(|o| o.locked)
    }

    /// Zustand für die schwebende Leiste: (gesperrt, gruppierbar, Gruppe auflösbar).
    pub fn selection_actions(&self) -> Option<(bool, bool, bool)> {
        let objs = self.selected_objects();
        if objs.is_empty() || self.text_edit.is_some() {
            return None;
        }
        let locked = objs.iter().any(|o| o.locked);
        let first = objs[0].group;
        let one_group = first != 0 && objs.iter().all(|o| o.group == first);
        let can_group = objs.len() >= 2 && !one_group;
        let can_ungroup = objs.iter().any(|o| o.group != 0);
        Some((locked, can_group, can_ungroup))
    }

    /// Anzahl der ausgewählten Objekte und der Einheiten (eine Gruppe zählt einmal).
    pub fn selection_counts(&self) -> (usize, usize) {
        let objs = self.selected_objects();
        let mut groups: Vec<u64> = Vec::new();
        let mut units = 0;
        for o in &objs {
            if o.group == 0 {
                units += 1;
            } else if !groups.contains(&o.group) {
                groups.push(o.group);
                units += 1;
            }
        }
        (objs.len(), units)
    }

    /// Ausgewählte Objekte sperren bzw. entsperren.
    pub fn set_selection_locked(&mut self, v: bool) -> bool {
        if self.selection.is_empty() {
            return false;
        }
        self.checkpoint(None);
        for id in self.selection.clone() {
            if let Some(o) = self.doc.object_mut(id) {
                o.locked = v;
            }
        }
        self.drag = Drag::None;
        true
    }

    /// Auswahl zu einer Gruppe zusammenfassen. Die Objekte rücken in der Ebene des obersten
    /// Objekts zusammen (an dessen Stelle), ihre Reihenfolge bleibt erhalten.
    pub fn group_selection(&mut self) -> bool {
        let objs = self.selected_in_order();
        if objs.len() < 2 {
            return false;
        }
        let Some((li, _)) = objs.last().and_then(|o| self.doc.locate(o.id)) else {
            return false;
        };
        self.checkpoint(None);
        let gid = self.doc.max_group() + 1;
        let top = objs.last().map(|o| o.id).unwrap_or(0);
        // alle außer dem obersten herausnehmen und direkt unter ihm wieder einfügen
        for o in &objs[..objs.len() - 1] {
            self.doc.remove_object(o.id);
        }
        let Some(pos) = self.doc.layers[li].objects.iter().position(|o| o.id == top) else {
            return false;
        };
        for (k, o) in objs[..objs.len() - 1].iter().enumerate() {
            self.doc.layers[li].objects.insert(pos + k, o.clone());
        }
        for o in self.doc.layers[li].objects.iter_mut() {
            if objs.iter().any(|x| x.id == o.id) {
                o.group = gid;
            }
        }
        self.layers_version += 1;
        true
    }

    /// Gruppen der Auswahl auflösen (die Objekte bleiben ausgewählt).
    pub fn ungroup_selection(&mut self) -> bool {
        if !self.selected_objects().iter().any(|o| o.group != 0) {
            return false;
        }
        self.checkpoint(None);
        for id in self.selection.clone() {
            if let Some(o) = self.doc.object_mut(id) {
                o.group = 0;
            }
        }
        true
    }

    /// Oberstes bearbeitbares Objekt unter `p`.
    pub fn hit(&self, p: Pt, m: &dyn Metrics) -> Option<u64> {
        // Notizheft: was außerhalb der Seite liegt, ist ausgeblendet
        if self.page_clip().is_some_and(|r| !r.contains(p)) {
            return None;
        }
        let tol = 4.0 / self.view.zoom;
        for l in self.doc.layers.iter().rev().filter(|l| l.editable()) {
            for o in l.objects.iter().rev() {
                if m.hit(o, p, tol) {
                    return Some(o.id);
                }
            }
        }
        None
    }

    /// Bildschirmpositionen der 8 Skalierungsanfasser (oben links, im Uhrzeigersinn).
    pub fn handle_positions(&self, m: &dyn Metrics) -> Option<[Pt; 8]> {
        if self.tool != Tool::Select || self.selection.is_empty() || self.selection_locked() {
            return None;
        }
        let (b, fr) = self.selection_frame(m)?;
        let back = fr.invert();
        let (mx, my) = ((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
        let pt = |x: f64, y: f64| self.view.to_screen(back.apply(Pt::new(x, y)));
        Some([
            pt(b.x0, b.y0),
            pt(mx, b.y0),
            pt(b.x1, b.y0),
            pt(b.x1, my),
            pt(b.x1, b.y1),
            pt(mx, b.y1),
            pt(b.x0, b.y1),
            pt(b.x0, my),
        ])
    }

    /// Auswahlrahmen: Haben alle ausgewählten Objekte dieselbe Drehung, liegt er gedreht
    /// um sie (wie das Objekt selbst). Liefert den Rahmen im gedrehten System und die
    /// Abbildung Dokument → System.
    pub fn selection_frame(&self, m: &dyn Metrics) -> Option<(Rect, Affine)> {
        let objs = self.selected_objects();
        let a0 = objs.first()?.angle;
        let same = objs.iter().all(|o| (o.angle - a0).abs() < 1e-6);
        let content = matches!(self.selection[..], [id] if self.image_part(id) != ImagePart::All);
        if a0.abs() < 1e-6 || !same || content {
            return Some((self.selection_bbox(m)?, Affine::IDENTITY));
        }
        // Objekte gedanklich zurückdrehen und dort den Rahmen bestimmen
        let fr = Affine::rotate_about(a0, Pt::new(0.0, 0.0));
        let mut out: Option<Rect> = None;
        for o in objs {
            let mut c = o.clone();
            c.shape.transform(&fr);
            if let Some(b) = m.bbox(&c) {
                out = Some(match out {
                    Some(x) => x.union(&b),
                    None => b,
                });
            }
        }
        Some((out?, fr))
    }

    /// Bildschirmposition des Drehgriffs (über der Mitte der Auswahl).
    pub fn rotate_handle(&self, m: &dyn Metrics) -> Option<Pt> {
        let hs = self.handle_positions(m)?;
        // nach „oben“ im System des Rahmens
        let up = hs[1] - hs[5];
        let up = if up.len() > 1e-6 {
            up.normalized()
        } else {
            Pt::new(0.0, -1.0)
        };
        Some(hs[1] + up * ROTATE_OFFSET)
    }

    fn on_rotate_handle(&self, s: Pt, m: &dyn Metrics) -> bool {
        self.rotate_handle(m)
            .map(|h| h.dist(s) <= HANDLE_RADIUS + 1.0)
            .unwrap_or(false)
    }

    fn handle_at(&self, s: Pt, m: &dyn Metrics) -> Option<usize> {
        let hs = self.handle_positions(m)?;
        hs.iter()
            .enumerate()
            .filter(|(_, h)| h.dist(s) <= HANDLE_RADIUS)
            .min_by(|a, b| a.1.dist(s).total_cmp(&b.1.dist(s)))
            .map(|(i, _)| i)
    }

    /// Cursorname für die Position (für Skalierungsanfasser).
    pub fn cursor_at(&self, s: Pt, m: &dyn Metrics) -> &'static str {
        if matches!(self.drag, Drag::Artboard { .. }) {
            return "grabbing";
        }
        if self.tool == Tool::Select && matches!(self.drag, Drag::None) && self.artboard_label_at(s).is_some()
        {
            return "grab";
        }
        let ab_handle = match self.drag {
            Drag::ArtboardSize { handle, .. } => Some(handle),
            Drag::None => self.artboard_handle_at(s).map(|x| x.1),
            _ => None,
        };
        if let Some(h) = ab_handle {
            return match h {
                0 | 4 => "nwse-resize",
                2 | 6 => "nesw-resize",
                1 | 5 => "ns-resize",
                _ => "ew-resize",
            };
        }
        if self.space_down || matches!(self.drag, Drag::Pan { .. }) {
            return if matches!(self.drag, Drag::Pan { .. }) {
                "grabbing"
            } else {
                "grab"
            };
        }
        if matches!(self.drag, Drag::Rotate { .. }) || self.on_rotate_handle(s, m) {
            return "crosshair";
        }
        if matches!(self.drag, Drag::Guide { .. } | Drag::GuideNew { .. })
            || (matches!(self.tool, Tool::Guide | Tool::Select) && self.hit(self.view.to_doc(s), m).is_none())
        {
            let idx = match self.drag {
                Drag::Guide { idx, .. } | Drag::GuideNew { idx, .. } => Some(idx),
                _ => self.guide_at(s),
            };
            if let Some(g) = idx.and_then(|i| self.doc.guides.get(i)) {
                return if g.is_horizontal() {
                    "ns-resize"
                } else if g.is_vertical() {
                    "ew-resize"
                } else {
                    "move"
                };
            }
        }
        if let Some(h) = self.handle_at(s, m) {
            // Richtung vom Mittelpunkt zum Anfasser (auch bei gedrehtem Rahmen)
            if let Some(hs) = self.handle_positions(m) {
                let c = hs[0].lerp(hs[4], 0.5);
                let d = hs[h] - c;
                let deg = d.y.atan2(d.x).to_degrees().rem_euclid(180.0);
                return if !(22.5..157.5).contains(&deg) {
                    "ew-resize"
                } else if deg < 67.5 {
                    "nwse-resize"
                } else if deg < 112.5 {
                    "ns-resize"
                } else {
                    "nesw-resize"
                };
            }
        }
        self.tool.cursor()
    }

    // ───────────── Objekte anlegen ─────────────

    /// Ebene, in die neue Objekte kommen. Gesperrte/ausgeblendete Ebenen werden abgelehnt.
    fn target_layer(&mut self) -> Option<usize> {
        let idx = self.doc.layer_index(self.active_layer).or_else(|| {
            let i = self.doc.layers.len().checked_sub(1)?;
            self.active_layer = self.doc.layers[i].id;
            Some(i)
        })?;
        let l = &self.doc.layers[idx];
        if !l.visible {
            self.message = Some(trf("Die Ebene „{}“ ist ausgeblendet", &[&(l.name)]));
            return None;
        }
        if l.locked {
            self.message = Some(trf("Die Ebene „{}“ ist gesperrt", &[&(l.name)]));
            return None;
        }
        Some(idx)
    }

    fn insert_object(&mut self, li: usize, shape: Shape, style: Style) -> u64 {
        let id = self.doc.alloc_id();
        self.doc.layers[li].objects.push(Object {
            group: 0,
            locked: false,
            name: String::new(),
            id,
            shape,
            style,
            angle: 0.0,
        });
        id
    }

    fn current_defaults(&self) -> &Style {
        if matches!(self.tool, Tool::Text | Tool::TextPath | Tool::TextArea) {
            &self.text_style
        } else {
            &self.defaults
        }
    }

    /// Strichstil für offene Linien: ohne Füllung, Kontur immer sichtbar.
    fn line_style(&self) -> Style {
        let d = &self.defaults;
        Style {
            fill: None,
            stroke: d.stroke.or(d.fill).or(Some(Color::BLACK)),
            width: if d.stroke.is_some() { d.width.max(0.5) } else { 2.0 },
            opacity: d.opacity,
            gradient: None,
            blend: d.blend,
            marker: d.marker,
            marker_ends: d.marker_ends,
            marker_size: d.marker_size,
            join: d.join,
            cap: d.cap,
            shadow: d.shadow,
            stroke_gradient: None,
            shadow_gradient: None,
            stroke_align: crate::model::StrokeAlign::Center,
            dash: d.dash,
            fx: d.fx,
        }
    }

    // ───────────── Zeigerereignisse ─────────────

    pub fn press(&mut self, s: Pt, mods: Mods, pan: bool, m: &dyn Metrics) {
        self.hints.borrow_mut().clear();
        self.press_at = Some(s);
        // Zeichenwerkzeuge: Ziele vor dem Klick sammeln (alle vorhandenen Objekte), damit
        // schon der erste Punkt einrastet und das neue Objekt nicht an sich selbst klebt.
        let creating = !matches!(self.tool, Tool::Select | Tool::Node) && !pan && !self.space_down;
        self.snap_ctx = (self.smart_snap && creating).then(|| self.build_snap_ctx(m, false));
        self.press_inner(s, mods, pan, m);
        if !creating {
            // Auswählen/Verschieben: Ziele ohne die (nun) ausgewählten Objekte; beim
            // Verschieben einer Zeichenfläche ohne sie selbst und was mit ihr wandert
            self.snap_ctx = match &self.drag {
                _ if !self.smart_snap => None,
                Drag::Pan { .. } => None,
                Drag::Artboard { idx, inside, .. } => {
                    Some(self.build_snap_ctx_ex(m, &|o: &Object| inside.contains(&o.id), Some(*idx)))
                }
                Drag::ArtboardSize { idx, .. } => {
                    Some(self.build_snap_ctx_ex(m, &|_: &Object| false, Some(*idx)))
                }
                _ => Some(self.build_snap_ctx(m, true)),
            };
        }
    }

    fn press_inner(&mut self, s: Pt, mods: Mods, pan: bool, m: &dyn Metrics) {
        let p = self.view.to_doc(s);
        self.pointer = Some(p);
        if pan || self.space_down {
            self.drag = Drag::Pan {
                s0: s,
                ox: self.view.ox,
                oy: self.view.oy,
            };
            return;
        }
        // Aus dem Lineal ziehen: neue Hilfslinie (oben waagerecht, links senkrecht)
        if self.ruler > 0.0
            && (s.x < self.ruler) != (s.y < self.ruler)
            && !self.lock_guides
            && !self
                .doc
                .layer(self.active_layer)
                .map(|l| l.locked)
                .unwrap_or(false)
        {
            let angle = if s.y < self.ruler { 0.0 } else { 90.0 };
            self.begin_change();
            self.touch();
            self.doc.guides.push(Guide {
                pos: p,
                angle,
                layer: self.active_layer,
            });
            self.show_guides = true;
            let idx = self.doc.guides.len() - 1;
            self.guide_sel = Some(idx);
            self.drag = Drag::Guide {
                idx,
                start: p,
                orig: p,
            };
            return;
        }
        if let Some((id, g, _)) = self.textpath_grip() {
            if self.view.to_screen(g).dist(s) <= HANDLE_RADIUS + 2.0 {
                self.begin_change();
                self.drag = Drag::TextOffset { id };
                return;
            }
        }
        if self.text_edit.is_some() && !matches!(self.tool, Tool::Text | Tool::TextArea | Tool::TextPath) {
            self.end_text_edit();
        }
        if self.tool == Tool::Select {
            if let Some((idx, handle)) = self.artboard_handle_at(s) {
                let rect = self.doc.artboard_rects()[idx].1;
                self.begin_change();
                self.drag = Drag::ArtboardSize {
                    idx,
                    handle,
                    rect,
                    orig: Box::new(self.doc.clone()),
                    view: self.view,
                };
                return;
            }
            if let Some(idx) = self.artboard_label_at(s) {
                let rects = self.doc.artboard_rects();
                let r = rects[idx].1.inflate(0.5);
                let inside: Vec<u64> = self
                    .doc
                    .layers
                    .iter()
                    .flat_map(|l| l.objects.iter())
                    .filter(|o| m.bbox(o).map(|b| r.contains_rect(&b)).unwrap_or(false))
                    .map(|o| o.id)
                    .collect();
                self.begin_change();
                self.drag = Drag::Artboard {
                    idx,
                    start: s,
                    orig: Box::new(self.doc.clone()),
                    view: self.view,
                    inside,
                };
                return;
            }
        }
        match self.tool {
            Tool::Select => self.select_press(p, s, mods, m),
            Tool::Node => self.node_press(p, s, mods, m),
            Tool::Shape => self.shape_press(p),
            Tool::Pen => self.pen_press(p, s, mods),
            Tool::Pencil => {
                if self.target_layer().is_some() {
                    self.drag = Drag::Pencil { pts: vec![p] };
                }
            }
            Tool::Text => self.text_press(p, m),
            Tool::TextPath => self.textpath_press(p, m),
            Tool::TextArea => self.area_press(p, m),
            Tool::Curvature => self.curv_press(p, s, mods),
            Tool::Guide => self.guide_press(p, s, mods),
            Tool::Eraser | Tool::Scissors => {
                self.drag = Drag::Erase { pts: vec![p] };
            }
            Tool::Builder => {
                let mut hits = Vec::new();
                self.builder_touch(&[p], &mut hits, m);
                self.drag = Drag::Build {
                    pts: vec![p],
                    hits,
                    alt: mods.alt,
                };
            }
        }
    }

    fn originals(&self) -> Vec<(u64, Object)> {
        self.selection
            .iter()
            .filter_map(|id| self.doc.object(*id).map(|o| (*id, o.clone())))
            .collect()
    }

    fn select_press(&mut self, p: Pt, s: Pt, mods: Mods, m: &dyn Metrics) {
        // Verlaufslinie: Endpunkte und Farbstopps
        if let Some((id, a, b, r)) = self.gradient_line(m) {
            let t = self.grad_target;
            if let Some(g) = self.doc.object(id).and_then(|o| o.style.grad(t).cloned()) {
                for (i, st) in g.stops.iter().enumerate() {
                    let q = a.lerp(b, st.pos);
                    let interior = st.pos > 0.02 && st.pos < 0.98;
                    if interior && self.view.to_screen(q).dist(s) <= HANDLE_RADIUS {
                        self.grad_stop = i;
                        self.begin_change();
                        self.drag = Drag::GradientStop { id, idx: i, r };
                        return;
                    }
                }
                for (end, q) in [(0usize, a), (1, b)] {
                    if self.view.to_screen(q).dist(s) <= HANDLE_RADIUS + 1.0 {
                        // Stopp am Ende auswählen (für die Farbe in der Seitenleiste)
                        let target = if end == 0 { 0.0 } else { 1.0 };
                        if let Some(i) = g
                            .stops
                            .iter()
                            .enumerate()
                            .min_by(|x, y| (x.1.pos - target).abs().total_cmp(&(y.1.pos - target).abs()))
                            .map(|x| x.0)
                        {
                            self.grad_stop = i;
                        }
                        self.begin_change();
                        self.drag = Drag::GradientEnd { id, end, r };
                        return;
                    }
                }
            }
        }
        if self.on_rotate_handle(s, m) {
            if let Some((b, fr)) = self.selection_frame(m) {
                let center = fr
                    .invert()
                    .apply(Pt::new((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0));
                let d = p - center;
                self.begin_change();
                self.drag = Drag::Rotate {
                    center,
                    start_angle: d.y.atan2(d.x),
                    orig: self.originals(),
                };
                return;
            }
        }
        if let Some(handle) = self.handle_at(s, m) {
            if let Some((bbox, frame)) = self.selection_frame(m) {
                self.begin_change();
                // Text und Bilder immer proportional skalieren.
                let uniform = self.selected_objects().iter().any(|o| {
                    matches!(
                        o.shape,
                        Shape::Text { .. } | Shape::TextPath { .. } | Shape::Image { .. }
                    )
                });
                self.drag = Drag::Scale {
                    handle,
                    bbox,
                    orig: self.originals(),
                    uniform,
                    frame,
                };
                return;
            }
        }
        // Inhalt eines gerahmten Bildes verschieben (auch außerhalb des Rahmens greifbar)
        if let [id] = self.selection[..] {
            if self.image_part(id) == ImagePart::Content {
                let inside = self
                    .doc
                    .object(id)
                    .and_then(|o| o.shape.image_quad())
                    .map(|q| in_quad(&q, p))
                    .unwrap_or(false);
                if inside {
                    self.begin_change();
                    self.drag = Drag::Move {
                        start: p,
                        orig: self.originals(),
                        bbox: None,
                        copies: Vec::new(),
                    };
                    return;
                }
                self.frame_mode = None;
            }
        }
        match self.hit(p, m) {
            Some(id) => {
                self.guide_sel = None;
                if let Some(l) = self.doc.layer_of(id) {
                    self.active_layer = l.id;
                }
                // Gruppen werden immer als Ganzes ausgewählt
                let members = self.doc.group_members(id);
                if mods.shift {
                    if self.selection.contains(&id) {
                        self.selection.retain(|x| !members.contains(x));
                    } else {
                        for mid in members {
                            if !self.selection.contains(&mid) {
                                self.selection.push(mid);
                            }
                        }
                    }
                    self.drag = Drag::None;
                    return;
                }
                if !self.selection.contains(&id) {
                    self.selection = members;
                }
                if self.selection_locked() {
                    // gesperrt: nur auswählen, nicht verschieben
                    self.drag = Drag::None;
                    return;
                }
                self.begin_change();
                self.drag = Drag::Move {
                    start: p,
                    orig: self.originals(),
                    bbox: self.selection_bbox(m),
                    copies: Vec::new(),
                };
            }
            None => {
                // Hilfslinie greifen
                if let Some(idx) = self.guide_at(s) {
                    self.selection.clear();
                    self.guide_sel = Some(idx);
                    let orig = self.doc.guides[idx].pos;
                    self.begin_change();
                    self.drag = Drag::Guide { idx, start: p, orig };
                    return;
                }
                let base = if mods.shift {
                    self.selection.clone()
                } else {
                    Vec::new()
                };
                if !mods.shift {
                    self.selection.clear();
                }
                self.drag = Drag::Rubber {
                    start: p,
                    cur: p,
                    base,
                };
            }
        }
    }

    /// Knoten eines Objekts zum Anzeigen/Bearbeiten. Rechtecke, Ellipsen und Linien
    /// liefern ihre Pfadform (sie werden beim ersten Bearbeiten umgewandelt). Gesperrte
    /// Objekte und solche ohne Knoten (Text, Bilder ohne Rahmen): `None`.
    pub fn node_view_of(&self, id: u64) -> Option<Vec<SubPath>> {
        let o = self.doc.object(id)?;
        if o.locked {
            return None;
        }
        match &o.shape {
            Shape::Path { subpaths, .. } => Some(subpaths.clone()),
            Shape::TextPath { path, .. } => Some(path.clone()),
            // Rahmen eines Bildes: Form bleibt bearbeitbar (Knoten, Eckenradius)
            Shape::Image { clip: Some(c), .. } => Some(c.clone()),
            // Rechteck: vier Eckknoten, der Radius steht an den Knoten (wie bei Pfaden)
            &Shape::Rect { x, y, w, h, r } => {
                let n = |px: f64, py: f64| Node {
                    r: r.max(0.0),
                    ..Node::corner(Pt::new(px, py))
                };
                Some(vec![SubPath {
                    nodes: vec![n(x, y), n(x + w, y), n(x + w, y + h), n(x, y + h)],
                    closed: true,
                }])
            }
            Shape::Ellipse { .. } | Shape::Line { .. } => match o.shape.to_path()? {
                Shape::Path { subpaths, .. } => Some(subpaths),
                _ => None,
            },
            _ => None,
        }
    }

    /// Alle ausgewählten Objekte, deren Knoten sich bearbeiten lassen, mit ihren Knoten.
    pub fn node_views(&self) -> Vec<(u64, Vec<SubPath>)> {
        self.selection
            .iter()
            .filter_map(|&id| self.node_view_of(id).map(|v| (id, v)))
            .collect()
    }

    /// Knoten des einzigen ausgewählten Objekts (Text auf Pfad, Tests).
    pub fn node_view(&self) -> Option<(u64, Vec<SubPath>)> {
        match self.selection[..] {
            [id] => self.node_view_of(id).map(|v| (id, v)),
            _ => None,
        }
    }

    /// Knoten des Objekts `id` aus der Knotenauswahl.
    fn nodes_of(&self, id: u64) -> Vec<(usize, usize)> {
        self.node_sel
            .iter()
            .filter(|n| n.0 == id)
            .map(|&(_, si, ni)| (si, ni))
            .collect()
    }

    /// Alle Knoten der Form als Auswahl.
    fn all_refs(id: u64, subs: &[SubPath]) -> Vec<NodeRef> {
        subs.iter()
            .enumerate()
            .flat_map(|(si, sp)| (0..sp.nodes.len()).map(move |ni| (id, si, ni)))
            .collect()
    }

    fn subpaths_mut(&mut self, id: u64) -> Option<&mut Vec<SubPath>> {
        // Rechteck: Eckknoten mit Radius übernehmen (keine Bogenknoten)
        let rect_nodes = match self.doc.object(id).map(|o| &o.shape) {
            Some(Shape::Rect { .. }) if self.selection.contains(&id) => self.node_view_of(id),
            _ => None,
        };
        if let Some(o) = self.doc.object_mut(id) {
            if let (Some(subpaths), Shape::Rect { .. }) = (rect_nodes, &o.shape) {
                o.shape = Shape::Path {
                    subpaths,
                    evenodd: false,
                };
            } else if matches!(
                o.shape,
                Shape::Rect { .. } | Shape::Ellipse { .. } | Shape::Line { .. }
            ) {
                if let Some(p) = o.shape.to_path() {
                    o.shape = p;
                }
            }
        }
        match self.doc.object_mut(id).map(|o| &mut o.shape) {
            Some(Shape::Path { subpaths, .. }) => Some(subpaths),
            Some(Shape::TextPath { path, .. }) => Some(path),
            Some(Shape::Image { clip: Some(c), .. }) => Some(c),
            _ => None,
        }
    }

    /// Sichtbare Anfasser im Knotenwerkzeug: die der gewählten Knoten und ihrer Nachbarn
    /// (Objekt, Teilpfad, Knoten, Anfasser, Lage).
    pub fn visible_handles(&self) -> Vec<(u64, usize, usize, Part, Pt)> {
        let mut out = Vec::new();
        for (id, subs) in self.node_views() {
            for (si, ni, part, p) in self.handles_of(id, &subs) {
                out.push((id, si, ni, part, p));
            }
        }
        out
    }

    fn handles_of(&self, id: u64, subs: &[SubPath]) -> Vec<(usize, usize, Part, Pt)> {
        let mut out = Vec::new();
        let mut idxs: Vec<(usize, usize)> = Vec::new();
        for (si, ni) in self.nodes_of(id) {
            let Some(sp) = subs.get(si) else { continue };
            let n = sp.nodes.len();
            if ni >= n {
                continue;
            }
            let mut add = |i: usize| {
                if !idxs.contains(&(si, i)) {
                    idxs.push((si, i));
                }
            };
            add(ni);
            if ni > 0 || sp.closed {
                add((ni + n - 1) % n);
            }
            if ni + 1 < n || sp.closed {
                add((ni + 1) % n);
            }
        }
        for (si, i) in idxs {
            let sp = &subs[si];
            let n = sp.nodes.len();
            let node = sp.nodes[i];
            let has_in = i > 0 || sp.closed;
            let has_out = i + 1 < n || sp.closed;
            if has_in && node.h_in.dist(node.p) > 1e-9 {
                out.push((si, i, Part::In, node.h_in));
            }
            if has_out && node.h_out.dist(node.p) > 1e-9 {
                out.push((si, i, Part::Out, node.h_out));
            }
        }
        out
    }

    /// Knotenwerkzeug: Erst anklicken wählt einen Knoten aus, erst ein gewählter Knoten
    /// lässt sich verschieben (verhindert versehentliches Verziehen).
    fn node_press(&mut self, p: Pt, s: Pt, mods: Mods, m: &dyn Metrics) {
        // Ziehpunkt für den Eckenradius
        for (w, corner, off) in self.corner_widgets() {
            if self.view.to_screen(w).dist(s) <= HANDLE_RADIUS {
                self.drag = Drag::Fillet { corner, off };
                return;
            }
        }
        let views = self.node_views();
        if !views.is_empty() {
            // Anfasser (nur bei gewählten Knoten sichtbar)
            let mut best = HANDLE_RADIUS;
            let mut handle: Option<(u64, usize, usize, Part)> = None;
            for (id, si, ni, part, hp) in self.visible_handles() {
                let d = self.view.to_screen(hp).dist(s);
                if d <= best {
                    best = d;
                    handle = Some((id, si, ni, part));
                }
            }
            if let Some((id, si, ni, part)) = handle {
                let mirror = views
                    .iter()
                    .find(|v| v.0 == id)
                    .and_then(|v| v.1.get(si)?.nodes.get(ni))
                    .is_some_and(|n| n.is_smooth());
                self.begin_change();
                self.drag = Drag::NodeEdit {
                    start: p,
                    orig: views,
                    handle: Some((id, si, ni, part)),
                    mirror,
                    active: false,
                };
                return;
            }
            // Knoten (aller Objekte, deren Knoten gezeigt werden)
            let mut anchor: Option<NodeRef> = None;
            let mut best = HANDLE_RADIUS;
            for (id, subs) in &views {
                for (si, sp) in subs.iter().enumerate() {
                    for (ni, n) in sp.nodes.iter().enumerate() {
                        let d = self.view.to_screen(n.p).dist(s);
                        if d <= best {
                            best = d;
                            anchor = Some((*id, si, ni));
                        }
                    }
                }
            }
            if let Some(a) = anchor {
                let selected = self.node_sel.contains(&a);
                if mods.shift {
                    if selected {
                        self.node_sel.retain(|x| *x != a);
                    } else {
                        self.node_sel.push(a);
                    }
                    self.drag = Drag::None;
                    return;
                }
                if !selected || self.all_nodes_selected_of(a.0) {
                    // Erster Klick wählt nur aus (auch wenn nach dem Anklicken des Objekts
                    // noch alle Knoten gewählt sind).
                    self.node_sel = vec![a];
                    self.drag = Drag::None;
                    return;
                }
                self.begin_change();
                let handle = if mods.alt {
                    Some((a.0, a.1, a.2, Part::Anchor))
                } else {
                    None
                };
                self.drag = Drag::NodeEdit {
                    start: p,
                    orig: views,
                    handle,
                    mirror: false,
                    active: false,
                };
                return;
            }
        }
        let hit = self.hit(p, m);
        // Umschalt + Klick auf ein weiteres Objekt: es kommt mit allen Knoten dazu
        if mods.shift {
            if let Some(h) = hit.filter(|h| !self.selection.contains(h)) {
                if let Some(subs) = self.node_view_of(h) {
                    self.selection.push(h);
                    self.node_sel.extend(Self::all_refs(h, &subs));
                    self.drag = Drag::None;
                    return;
                }
            }
        }
        // Klick auf ein gewähltes Objekt oder ins Leere: Knoten per Rahmen wählen
        // (der Rahmen erfasst Knoten aller Objekte)
        if hit.is_none() || hit.is_some_and(|h| self.selection.contains(&h)) {
            let had_nodes = !self.node_sel.is_empty();
            let base = if mods.shift {
                self.node_sel.clone()
            } else {
                Vec::new()
            };
            if !mods.shift {
                self.node_sel.clear();
            }
            self.drag = Drag::NodeRubber {
                start: p,
                cur: p,
                base,
                base_objs: self.selection.clone(),
                add: mods.shift,
                had_nodes: had_nodes || hit.is_some(),
            };
            return;
        }
        self.selection = hit.into_iter().collect();
        if let Some(l) = hit.and_then(|id| self.doc.layer_of(id)) {
            self.active_layer = l.id;
        }
        // neu angeklicktes Objekt: alle Knoten gewählt, damit die Eckenradius-Punkte gleich da sind
        self.select_all_nodes();
        self.drag = Drag::None;
    }

    /// Alle Knoten der ausgewählten Objekte wählen.
    pub fn select_all_nodes(&mut self) {
        self.node_sel = self
            .node_views()
            .iter()
            .flat_map(|(id, subs)| Self::all_refs(*id, subs))
            .collect();
    }

    /// Sind alle Knoten (mehr als einer) des Objekts gewählt – der Ausgangszustand nach dem
    /// Anklicken?
    fn all_nodes_selected_of(&self, id: u64) -> bool {
        let total: usize = self
            .node_view_of(id)
            .map(|subs| subs.iter().map(|sp| sp.nodes.len()).sum())
            .unwrap_or(0);
        total > 1 && self.nodes_of(id).len() == total
    }

    /// Sind gerade alle Knoten (mehr als einer) gewählt – der Ausgangszustand nach dem Anklicken?
    fn all_nodes_selected(&self) -> bool {
        let total: usize = self
            .node_view()
            .map(|(_, subs)| subs.iter().map(|sp| sp.nodes.len()).sum())
            .unwrap_or(0);
        total > 1 && self.node_sel.len() == total
    }

    fn shape_press(&mut self, p: Pt) {
        let p = self.snap(p);
        let Some(li) = self.target_layer() else { return };
        let was_dirty = self.dirty;
        self.begin_change();
        self.touch();
        let shape = self.create_geometry(p, p, Mods::default());
        let style = if self.shape_kind == ShapeKind::Line {
            self.line_style()
        } else {
            self.defaults.clone()
        };
        let id = self.insert_object(li, shape, style);
        self.selection = vec![id];
        self.drag = Drag::Create {
            id,
            start: p,
            was_dirty,
        };
    }

    fn create_geometry(&self, start: Pt, cur: Pt, mods: Mods) -> Shape {
        let mut d = cur - start;
        match self.shape_kind {
            ShapeKind::Rect | ShapeKind::Ellipse => {
                if mods.shift {
                    let s = d.x.abs().max(d.y.abs());
                    d = Pt::new(s.copysign(d.x), s.copysign(d.y));
                }
                let (a, b) = if mods.alt {
                    (start - d, start + d)
                } else {
                    (start, start + d)
                };
                let r = Rect::from_points(a, b);
                if self.shape_kind == ShapeKind::Rect {
                    Shape::Rect {
                        x: r.x0,
                        y: r.y0,
                        w: r.width(),
                        h: r.height(),
                        // neue Rechtecke immer ohne Eckenradius (der wird am Objekt gezogen)
                        r: 0.0,
                    }
                } else {
                    Shape::Ellipse {
                        cx: (r.x0 + r.x1) / 2.0,
                        cy: (r.y0 + r.y1) / 2.0,
                        rx: r.width() / 2.0,
                        ry: r.height() / 2.0,
                    }
                }
            }
            ShapeKind::Line => {
                if mods.shift {
                    let len = d.len();
                    let step = std::f64::consts::FRAC_PI_4;
                    let a = (d.y.atan2(d.x) / step).round() * step;
                    d = Pt::new(len * a.cos(), len * a.sin());
                }
                let a = if mods.alt { start - d } else { start };
                Shape::Line { a, b: start + d }
            }
            ShapeKind::Polygon | ShapeKind::Star => {
                let radius = d.len();
                let mut angle = d.y.atan2(d.x) + std::f64::consts::FRAC_PI_2;
                if mods.shift {
                    let step = 15f64.to_radians();
                    angle = (angle / step).round() * step;
                }
                if radius < 1e-9 {
                    angle = 0.0;
                }
                Shape::polygon(
                    start,
                    radius,
                    angle,
                    self.corners,
                    self.shape_kind == ShapeKind::Star,
                )
            }
        }
    }

    fn pen_press(&mut self, p: Pt, s: Pt, mods: Mods) {
        let p = match (mods.shift, self.pen.as_ref()) {
            (true, Some(nodes)) => self.constrain_from(nodes, p),
            _ => self.snap(p),
        };
        if self.pen.is_none() {
            if self.target_layer().is_none() {
                return;
            }
            self.selection.clear();
            self.pen = Some(vec![Node::corner(p)]);
            self.drag = Drag::PenHandle;
            return;
        }
        let close = {
            let nodes = self.pen.as_ref().unwrap();
            nodes.len() >= 2 && self.view.to_screen(nodes[0].p).dist(s) <= HANDLE_RADIUS + 1.0
        };
        if close {
            self.finish_pen(true);
            self.drag = Drag::None;
            return;
        }
        self.pen.as_mut().unwrap().push(Node::corner(p));
        self.drag = Drag::PenHandle;
    }

    /// Zeichenstift abschließen; ab zwei Punkten entsteht ein Pfad.
    pub fn finish_pen(&mut self, closed: bool) -> bool {
        let Some(nodes) = self.pen.take() else {
            return false;
        };
        if nodes.len() < 2 {
            return false;
        }
        let Some(li) = self.target_layer() else {
            return false;
        };
        self.checkpoint(None);
        let style = if closed {
            self.defaults.clone()
        } else {
            self.line_style()
        };
        let shape = Shape::Path {
            subpaths: vec![SubPath { nodes, closed }],
            evenodd: false,
        };
        let id = self.insert_object(li, shape, style);
        self.selection = vec![id];
        true
    }

    /// Klick mit einem Textwerkzeug auf einen vorhandenen Text: Cursor setzen.
    /// Liefert `true`, wenn der Klick damit erledigt ist.
    fn text_click(&mut self, p: Pt, m: &dyn Metrics) -> bool {
        if let Some(te) = self.text_edit {
            let hit = self.hit(p, m);
            if hit == Some(te.id) {
                if let Some(o) = self.doc.object(te.id) {
                    let i = m.text_index(o, p).unwrap_or(te.caret);
                    self.text_edit = Some(TextEdit {
                        id: te.id,
                        caret: i,
                        anchor: i,
                    });
                    self.drag = Drag::TextSelect { id: te.id };
                }
                return true;
            }
            // Klick daneben beendet nur die Bearbeitung.
            self.end_text_edit();
            self.deselect();
            return true;
        }
        let Some(id) = self.hit(p, m) else { return false };
        match self.doc.object(id).map(|o| &o.shape) {
            Some(Shape::Text { .. }) | Some(Shape::TextArea { .. }) => {
                self.selection = vec![id];
                let i = self.doc.object(id).and_then(|o| m.text_index(o, p));
                self.begin_text_edit(id, i);
                self.drag = Drag::TextSelect { id };
                true
            }
            Some(Shape::TextPath { .. }) => {
                self.selection = vec![id];
                let i = self.doc.object(id).and_then(|o| m.text_index(o, p));
                self.begin_text_edit(id, i);
                self.drag = Drag::TextSelect { id };
                true
            }
            _ => false,
        }
    }

    fn text_press(&mut self, p: Pt, m: &dyn Metrics) {
        if self.text_click(p, m) {
            return;
        }
        let Some(li) = self.target_layer() else { return };
        self.checkpoint(None);
        let t = self.text_defaults.clone();
        let mut shape = Shape::text_at(
            Pt::new(p.x, p.y + t.size * 0.35),
            tr("Text"),
            &t.family,
            t.size,
            t.bold,
            t.italic,
        );
        if let Some(f) = shape.text_format_mut() {
            *f = t.fmt;
        }
        let id = self.insert_object(li, shape, self.text_style.clone());
        self.selection = vec![id];
        // Neuer Text: alles markiert, Tippen ersetzt ihn sofort.
        self.begin_text_edit(id, None);
    }

    // ───────────── Text direkt bearbeiten ─────────────

    /// Bearbeitung beginnen; `caret` = None markiert den ganzen Text.
    pub fn begin_text_edit(&mut self, id: u64, caret: Option<usize>) {
        let Some(len) = self
            .doc
            .object(id)
            .and_then(|o| o.shape.text_fields())
            .map(|t| t.0.len())
        else {
            return;
        };
        self.text_edit = Some(match caret {
            Some(c) => TextEdit {
                id,
                caret: c.min(len),
                anchor: c.min(len),
            },
            None => TextEdit {
                id,
                caret: len,
                anchor: 0,
            },
        });
        self.last_key = None;
    }

    /// Bearbeitung beenden. Leer gewordener Text wird entfernt.
    pub fn end_text_edit(&mut self) {
        let Some(te) = self.text_edit.take() else { return };
        let empty = self
            .doc
            .object(te.id)
            .and_then(|o| o.shape.text_fields())
            .map(|t| t.0.trim().is_empty())
            .unwrap_or(false);
        if empty {
            self.checkpoint(None);
            self.doc.remove_object(te.id);
            self.selection.retain(|x| *x != te.id);
        }
        self.last_key = None;
    }

    fn edit_text(&self) -> Option<(TextEdit, String)> {
        let te = self.text_edit?;
        let t = self.doc.object(te.id)?.shape.text_fields()?.0.to_string();
        let len = t.len();
        let fix = |i: usize| {
            let mut i = i.min(len);
            while !t.is_char_boundary(i) {
                i -= 1;
            }
            i
        };
        Some((
            TextEdit {
                id: te.id,
                caret: fix(te.caret),
                anchor: fix(te.anchor),
            },
            t,
        ))
    }

    fn store_text(&mut self, id: u64, text: String, caret: usize, anchor: usize) {
        self.checkpoint(Some("type"));
        if let Some(o) = self.doc.object_mut(id) {
            o.shape.replace_text(text);
        }
        self.placeholders.retain(|x| *x != id);
        self.text_edit = Some(TextEdit { id, caret, anchor });
    }

    /// Text an der Cursorposition einfügen (ersetzt die Markierung).
    pub fn edit_insert(&mut self, s: &str) -> bool {
        let Some((te, mut t)) = self.edit_text() else {
            return false;
        };
        let s = if matches!(
            self.doc.object(te.id).map(|o| &o.shape),
            Some(Shape::TextPath { .. })
        ) {
            s.replace(['\n', '\t'], " ")
        } else {
            s.to_string()
        };
        let (a, b) = te.range();
        t.replace_range(a..b, &s);
        let c = a + s.len();
        self.store_text(te.id, t, c, c);
        // vorgemerktes Format (Cursor ohne Markierung) für den eingefügten Text
        if let Some((pid, pc, pf)) = self.pending_fmt.take() {
            if pid == te.id && pc == a && a == b {
                if let Some(o) = self.doc.object_mut(te.id) {
                    let len = o.shape.text_fields().map(|t| t.0.len()).unwrap_or(0);
                    if let (Some(base), Some(r)) = (o.shape.char_base(), o.shape.rich_mut()) {
                        r.apply_chars(len, &base, Some((a, c)), &move |f: &mut CharFmt| *f = pf.clone());
                    }
                }
            }
        }
        true
    }

    pub fn edit_backspace(&mut self) -> bool {
        let Some((te, mut t)) = self.edit_text() else {
            return false;
        };
        let (mut a, b) = te.range();
        if a == b {
            if a == 0 {
                return false;
            }
            a = prev_boundary(&t, a);
        }
        t.replace_range(a..b, "");
        self.store_text(te.id, t, a, a);
        true
    }

    pub fn edit_delete(&mut self) -> bool {
        let Some((te, mut t)) = self.edit_text() else {
            return false;
        };
        let (a, mut b) = te.range();
        if a == b {
            if b >= t.len() {
                return false;
            }
            b = next_boundary(&t, b);
        }
        t.replace_range(a..b, "");
        self.store_text(te.id, t, a, a);
        true
    }

    /// Cursor bewegen; `extend` erweitert die Markierung (Umschalt).
    pub fn edit_move(&mut self, mv: Move, extend: bool) -> bool {
        let Some((te, t)) = self.edit_text() else {
            return false;
        };
        let (a, b) = te.range();
        let c = te.caret;
        let new = match mv {
            Move::Left if !extend && a != b => a,
            Move::Right if !extend && a != b => b,
            Move::Left => prev_boundary(&t, c),
            Move::Right => next_boundary(&t, c),
            Move::Home => line_start(&t, c),
            Move::End => line_end(&t, c),
            Move::WordLeft => {
                let mut i = c;
                while i > 0 && !t[prev_boundary(&t, i)..i].chars().all(is_word) {
                    i = prev_boundary(&t, i);
                }
                word_at(&t, i).0
            }
            Move::WordRight => {
                let mut i = c;
                while i < t.len() && !t[i..next_boundary(&t, i)].chars().all(is_word) {
                    i = next_boundary(&t, i);
                }
                word_at(&t, i).1
            }
            Move::Up | Move::Down => {
                let ls = line_start(&t, c);
                let col = t[ls..c].chars().count();
                let (s0, e0) = if mv == Move::Up {
                    if ls == 0 {
                        (0, 0)
                    } else {
                        let pe = ls - 1;
                        (line_start(&t, pe), pe)
                    }
                } else {
                    let le = line_end(&t, c);
                    if le >= t.len() {
                        (t.len(), t.len())
                    } else {
                        (le + 1, line_end(&t, le + 1))
                    }
                };
                let line = &t[s0..e0];
                s0 + line.char_indices().nth(col).map(|(i, _)| i).unwrap_or(line.len())
            }
        };
        let anchor = if extend { te.anchor } else { new };
        self.text_edit = Some(TextEdit {
            id: te.id,
            caret: new,
            anchor,
        });
        true
    }

    pub fn edit_select_all(&mut self) -> bool {
        let Some((te, t)) = self.edit_text() else {
            return false;
        };
        self.text_edit = Some(TextEdit {
            id: te.id,
            caret: t.len(),
            anchor: 0,
        });
        true
    }

    /// Markierten Text (für die Zwischenablage).
    pub fn edit_selected_text(&self) -> Option<String> {
        let (te, t) = self.edit_text()?;
        let (a, b) = te.range();
        (a < b).then(|| t[a..b].to_string())
    }

    // ───────────── Text auf Pfad: Griff, Blindtext ─────────────

    /// Griff am Textanfang von Text auf Pfad: (Objekt, Position, Richtungswinkel).
    pub fn textpath_grip(&self) -> Option<(u64, Pt, f64)> {
        if !matches!(self.tool, Tool::Select | Tool::Node | Tool::Text | Tool::TextPath)
            || self.selection.len() != 1
        {
            return None;
        }
        let o = self.doc.object(self.selection[0])?;
        let Shape::TextPath {
            path, offset, fmt, ..
        } = &o.shape
        else {
            return None;
        };
        let (pts, len) = crate::model::textpath_line(path, fmt.path_reverse);
        let total = len.last().copied().unwrap_or(0.0);
        let at = crate::model::textpath_anchor(total, *offset, fmt.align);
        let at = if crate::model::path_is_closed(path) && total > 0.0 {
            at.rem_euclid(total)
        } else {
            at.clamp(0.0, total)
        };
        let (p, a) = crate::model::point_at_length(&pts, &len, at)?;
        Some((o.id, p, a))
    }

    /// Blindtext-Felder neu füllen (nach Erstellen oder Größenänderung).
    pub fn refill_placeholders(&mut self, m: &dyn Metrics) {
        for id in self.placeholders.clone() {
            let Some(o) = self.doc.object(id) else { continue };
            if let Some(text) = m.fill_area(o) {
                if let Some((t, ..)) = self.doc.object_mut(id).and_then(|o| o.shape.text_fields_mut()) {
                    *t = text;
                }
            }
        }
    }

    pub fn motion(&mut self, s: Pt, mods: Mods, m: &dyn Metrics) {
        // Kleines Wackeln beim Klicken ist noch kein Ziehen: nichts verschieben, keine
        // Anfasser oder Kurven aus Versehen. Freihand, Radierer, Text markieren und Schwenken
        // folgen sofort (dort zählt jeder Pixel bzw. es gibt eine eigene Schwelle).
        if let Some(s0) = self.press_at {
            let exempt = matches!(
                self.drag,
                Drag::None
                    | Drag::Pan { .. }
                    | Drag::Pencil { .. }
                    | Drag::Erase { .. }
                    | Drag::TextSelect { .. }
                    | Drag::PenHandle
            );
            if !exempt && s.dist(s0) < CLICK_SLOP {
                return;
            }
            if !matches!(self.drag, Drag::None) {
                self.press_at = None;
            }
        }
        let p = self.view.to_doc(s);
        self.pointer = Some(p);
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        self.drag = match drag {
            Drag::None => {
                self.hover = if matches!(self.tool, Tool::Select | Tool::Node) {
                    self.hit(p, m)
                } else {
                    None
                };
                if self.tool == Tool::Builder {
                    *self.builder_hover.borrow_mut() = self.build_arrangement(m).face_at(p);
                }
                // Vorschau beim Zeichnen: intelligente Hilfslinien schon beim Zeigen,
                // mit Umschalt eingerastete Richtung
                self.hints.borrow_mut().clear();
                if (self.smart_snap || (self.snap_guides && !self.doc.guides.is_empty()))
                    && matches!(
                        self.tool,
                        Tool::Shape | Tool::Pen | Tool::Curvature | Tool::Text | Tool::TextArea | Tool::Guide
                    )
                    && !self.space_down
                {
                    let fresh = matches!(&*self.hover_ctx.borrow(), Some((r, _)) if *r == self.rev);
                    if !fresh && self.smart_snap {
                        let c = self.build_snap_ctx(m, false);
                        *self.hover_ctx.borrow_mut() = Some((self.rev, c));
                    }
                    let q = {
                        let hc = self.hover_ctx.borrow();
                        self.snap_in(p, hc.as_ref().filter(|_| self.smart_snap).map(|x| &x.1))
                    };
                    self.pointer = Some(q);
                }
                if mods.shift {
                    if let Some(nodes) = self.pen.clone() {
                        self.pointer = Some(self.constrain_from(&nodes, p));
                    } else if let Some(pts) = self.curv.clone() {
                        let nodes: Vec<Node> = pts.iter().map(|(q, _)| Node::corner(*q)).collect();
                        self.pointer = Some(self.constrain_from(&nodes, p));
                    }
                }
                Drag::None
            }
            Drag::Pan { s0, ox, oy } => {
                self.view.ox = ox + (s.x - s0.x);
                self.view.oy = oy + (s.y - s0.y);
                Drag::Pan { s0, ox, oy }
            }
            Drag::Move {
                start,
                orig,
                bbox,
                mut copies,
            } => {
                let mut d = p - start;
                if mods.shift {
                    if d.x.abs() > d.y.abs() {
                        d.y = 0.0;
                    } else {
                        d.x = 0.0;
                    }
                }
                if let Some(b) = bbox {
                    let anchors = Self::anchor_points(&orig);
                    d = self.snap_move(&b, d, mods.shift, &anchors);
                }
                if d.len() > 0.0 {
                    self.touch();
                    let t = Affine::translate(d.x, d.y);
                    for (id, ob) in &orig {
                        let part = self.image_part(*id);
                        if let Some(o) = self.doc.object_mut(*id) {
                            *o = ob.clone();
                            o.shape.transform_part(&t, part);
                        }
                    }
                }
                // Alt: Kopie an der alten Stelle zurücklassen (Alt loslassen nimmt sie wieder weg)
                let whole = orig.iter().all(|(id, _)| self.image_part(*id) == ImagePart::All);
                if mods.alt && whole && copies.is_empty() && d.len() > 0.0 {
                    self.touch();
                    copies = self.leave_copies(&orig);
                } else if !mods.alt && !copies.is_empty() {
                    self.touch();
                    for id in copies.drain(..) {
                        self.doc.remove_object(id);
                    }
                }
                // Einzelnes Bild über einer Form: nach kurzer Pause in die Form setzen
                self.update_frame_hover(s, m);
                Drag::Move {
                    start,
                    orig,
                    bbox,
                    copies,
                }
            }
            Drag::Scale {
                handle,
                bbox,
                orig,
                uniform,
                frame,
            } => {
                let rotated = !frame.is_identity();
                // im System des Rahmens skalieren, dann zurückdrehen
                let q = if rotated { frame.apply(p) } else { self.snap(p) };
                let local = scale_affine(handle, &bbox, q, mods, uniform);
                let a = frame.invert().then_before(&local).then_before(&frame);
                self.touch();
                for (id, ob) in &orig {
                    if let Some(o) = self.doc.object_mut(*id) {
                        *o = ob.clone();
                    }
                    self.transform_object(*id, &a, m);
                    if rotated {
                        // Breite/Höhe im gedrehten System ändern die Drehung nicht
                        if let Some(o) = self.doc.object_mut(*id) {
                            o.angle = ob.angle;
                        }
                    }
                }
                self.refill_placeholders(m);
                Drag::Scale {
                    handle,
                    bbox,
                    orig,
                    uniform,
                    frame,
                }
            }
            Drag::Rotate {
                center,
                start_angle,
                orig,
                ..
            } => {
                let d = p - center;
                let mut angle = (d.y.atan2(d.x) - start_angle).to_degrees();
                if mods.shift {
                    angle = (angle / 15.0).round() * 15.0;
                }
                let a = Affine::rotate_about(angle, center);
                self.touch();
                for (id, ob) in &orig {
                    if let Some(o) = self.doc.object_mut(*id) {
                        *o = ob.clone();
                    }
                    self.transform_object(*id, &a, m);
                }
                let shown = self.selected_objects().first().map(|o| o.angle).unwrap_or(angle);
                self.badge = Some((s, Badge::Angle(shown)));
                Drag::Rotate {
                    center,
                    start_angle,
                    orig,
                }
            }
            Drag::Rubber { start, base, .. } => {
                let r = Rect::from_points(start, p);
                let page = self.page_clip();
                let mut sel = base.clone();
                for l in self.doc.layers.iter().filter(|l| l.editable()) {
                    for o in &l.objects {
                        let inside = |b: Rect| b.intersects(&r) && page.map_or(true, |pg| b.intersects(&pg));
                        if !sel.contains(&o.id) && m.bbox(o).map(inside).unwrap_or(false) {
                            sel.push(o.id);
                        }
                    }
                }
                self.selection = sel;
                self.expand_groups();
                Drag::Rubber { start, cur: p, base }
            }
            Drag::Create { id, start, was_dirty } => {
                let shape = self.create_geometry(start, self.snap(p), mods);
                if let Some(o) = self.doc.object_mut(id) {
                    o.shape = shape;
                }
                Drag::Create { id, start, was_dirty }
            }
            Drag::Artboard {
                idx,
                start,
                orig,
                view,
                inside,
            } => {
                self.touch();
                let mut d = Pt::new((s.x - start.x) / view.zoom, (s.y - start.y) / view.zoom);
                // Umschalt: nur waagerecht oder senkrecht
                if mods.shift {
                    if d.x.abs() >= d.y.abs() {
                        d.y = 0.0;
                    } else {
                        d.x = 0.0;
                    }
                }
                // Einrasten wie beim Verschieben von Objekten – gerechnet in den Koordinaten
                // vom Beginn (die erste Zeichenfläche verschiebt beim Ziehen alles andere)
                self.doc.guides = orig.guides.clone();
                let r = orig.artboard_rects()[idx].1;
                let d = self.snap_move(&r, d, mods.shift, &[]);
                self.artboard_drag(idx, d, &orig, view, &inside);
                if idx == 0 {
                    for h in self.hints.borrow_mut().iter_mut() {
                        h.a = h.a - d;
                        h.b = h.b - d;
                    }
                }
                Drag::Artboard {
                    idx,
                    start,
                    orig,
                    view,
                    inside,
                }
            }
            Drag::ArtboardSize {
                idx,
                handle,
                rect,
                orig,
                view,
            } => {
                self.touch();
                // in den Koordinaten vom Beginn rechnen (die erste Zeichenfläche verschiebt
                // beim Ändern die Ansicht); einrasten nur bei den weiteren
                let q = view.to_doc(s);
                self.doc.guides = orig.guides.clone();
                let q = self.snap(q);
                let t = self.artboard_resize(idx, handle, rect, q, &orig, view);
                for h in self.hints.borrow_mut().iter_mut() {
                    h.a = h.a + t;
                    h.b = h.b + t;
                }
                Drag::ArtboardSize {
                    idx,
                    handle,
                    rect,
                    orig,
                    view,
                }
            }
            Drag::Erase { mut pts } => {
                if pts
                    .last()
                    .map(|l| l.dist(p) * self.view.zoom >= 1.0)
                    .unwrap_or(true)
                {
                    pts.push(p);
                }
                Drag::Erase { pts }
            }
            Drag::Build {
                mut pts,
                mut hits,
                alt,
            } => {
                let last = pts.last().copied().unwrap_or(p);
                // Zwischenpunkte, damit schnelles Ziehen keine Fläche überspringt
                let seg = crate::shapeops::resample(&[last, p], 2.0 / self.view.zoom);
                self.builder_touch(&seg, &mut hits, m);
                pts.push(p);
                Drag::Build {
                    pts,
                    hits,
                    alt: alt || mods.alt,
                }
            }
            Drag::Pencil { mut pts } => {
                if pts
                    .last()
                    .map(|l| l.dist(p) * self.view.zoom >= 1.5)
                    .unwrap_or(true)
                {
                    pts.push(p);
                }
                Drag::Pencil { pts }
            }
            Drag::PenHandle => {
                // Liegt der Punkt auf einer Hilfslinie und zeigt der Griff ungefähr entlang,
                // folgt die Kurve der Linie.
                let along = self.pen.as_ref().and_then(|v| v.last()).and_then(|n| {
                    if !(self.snap_guides && self.show_guides) || mods.shift {
                        return None;
                    }
                    let gtol = GUIDE_SNAP / self.view.zoom;
                    self.doc
                        .guides
                        .iter()
                        .filter(|g| self.doc.guide_visible(g) && g.dist(n.p) < 1e-6 && g.dist(p) <= gtol)
                        .min_by(|a, b| a.dist(p).total_cmp(&b.dist(p)))
                        .map(|g| g.project(p))
                });
                if let Some(q) = along {
                    self.hints.borrow_mut().clear();
                    self.hints.borrow_mut().push(SnapHint {
                        a: q,
                        b: q,
                        label: Some(tr("Hilfslinie")),
                    });
                }
                let p = along.unwrap_or(p);
                if let Some(n) = self.pen.as_mut().and_then(|v| v.last_mut()) {
                    let mut d = p - n.p;
                    if mods.shift && d.len() > 1e-9 {
                        let a = (d.y.atan2(d.x) / std::f64::consts::FRAC_PI_4).round()
                            * std::f64::consts::FRAC_PI_4;
                        d = Pt::new(a.cos(), a.sin()) * d.len();
                    }
                    // Erst ab einem deutlichen Zug wird der Punkt rund (kleines Zittern beim
                    // Klicken ergibt eine Ecke).
                    if d.len() * self.view.zoom > PEN_DEADZONE {
                        n.h_out = n.p + d;
                        n.h_in = n.p - d;
                    } else {
                        n.h_out = n.p;
                        n.h_in = n.p;
                    }
                }
                Drag::PenHandle
            }
            Drag::NodeEdit {
                start,
                orig,
                handle,
                mirror,
                mut active,
            } => {
                let mut d = p - start;
                if !active && d.len() * self.view.zoom >= 3.0 {
                    active = true;
                }
                let orig_node = |id: u64, si: usize, ni: usize| -> Option<Node> {
                    orig.iter()
                        .find(|o| o.0 == id)
                        .and_then(|o| o.1.get(si))
                        .and_then(|sp| sp.nodes.get(ni))
                        .copied()
                };
                // Gezogenen Knoten oder Anfasser an Hilfslinien einrasten
                if !matches!(handle, Some((_, _, _, Part::Anchor))) {
                    let anchor = match handle {
                        Some((id, si, ni, Part::In)) => orig_node(id, si, ni).map(|n| n.h_in),
                        Some((id, si, ni, Part::Out)) => orig_node(id, si, ni).map(|n| n.h_out),
                        _ => self
                            .node_sel
                            .first()
                            .and_then(|&(id, si, ni)| orig_node(id, si, ni))
                            .map(|n| n.p),
                    };
                    if let Some(a) = anchor {
                        d = self.snap(a + d) - a;
                    }
                }
                if active {
                    self.touch();
                    match handle {
                        Some((id, si, ni, part)) => {
                            let o = orig_node(id, si, ni);
                            let osubs = orig.iter().find(|o| o.0 == id).map(|o| o.1.clone());
                            if let (Some(subs), Some(osubs), Some(o)) = (self.subpaths_mut(id), osubs, o) {
                                *subs = osubs;
                                if let Some(n) = subs.get_mut(si).and_then(|sp| sp.nodes.get_mut(ni)) {
                                    match part {
                                        Part::Anchor => {
                                            // Anfasser aus einem Punkt herausziehen (Alt)
                                            n.h_out = o.p + d;
                                            n.h_in = o.p - d;
                                        }
                                        Part::In => {
                                            n.h_in = o.h_in + d;
                                            if mirror && !mods.alt {
                                                let len = (o.h_out - o.p).len();
                                                n.h_out = n.p + (n.p - n.h_in).normalized() * len;
                                            }
                                        }
                                        Part::Out => {
                                            n.h_out = o.h_out + d;
                                            if mirror && !mods.alt {
                                                let len = (o.h_in - o.p).len();
                                                n.h_in = n.p + (n.p - n.h_out).normalized() * len;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        None => {
                            let mut d = d;
                            if mods.shift {
                                if d.x.abs() > d.y.abs() {
                                    d.y = 0.0;
                                } else {
                                    d.x = 0.0;
                                }
                            }
                            // gewählte Knoten in allen beteiligten Objekten verschieben
                            for (id, osubs) in &orig {
                                let targets = self.nodes_of(*id);
                                if targets.is_empty() {
                                    continue;
                                }
                                if let Some(subs) = self.subpaths_mut(*id) {
                                    *subs = osubs.clone();
                                    for (si, ni) in targets {
                                        if let Some(n) = subs.get_mut(si).and_then(|sp| sp.nodes.get_mut(ni))
                                        {
                                            n.p = n.p + d;
                                            n.h_in = n.h_in + d;
                                            n.h_out = n.h_out + d;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Drag::NodeEdit {
                    start,
                    orig,
                    handle,
                    mirror,
                    active,
                }
            }
            Drag::TextOffset { id } => {
                if let Some(Shape::TextPath { path, fmt, .. }) = self.doc.object(id).map(|o| &o.shape) {
                    let (pts, len) = crate::model::textpath_line(path, fmt.path_reverse);
                    let total = len.last().copied().unwrap_or(0.0);
                    let d = crate::model::nearest_length(&pts, &len, p);
                    let v = crate::model::textpath_offset(total, d, fmt.align);
                    self.touch();
                    if let Some(Object {
                        shape: Shape::TextPath { offset, .. },
                        ..
                    }) = self.doc.object_mut(id)
                    {
                        *offset = v;
                    }
                }
                Drag::TextOffset { id }
            }
            Drag::GradientEnd { id, end, r } => {
                let q = self.snap(p);
                self.touch();
                let t = self.grad_target;
                if let Some(g) = self
                    .doc
                    .object_mut(id)
                    .and_then(|o| o.style.grad_slot(t).as_mut())
                {
                    let (mut a, mut b) = g.line(&r);
                    if end == 0 {
                        a = q;
                    } else {
                        b = q;
                    }
                    if mods.shift {
                        // Winkel in 15°-Schritten
                        let (fix, mov) = if end == 0 { (b, a) } else { (a, b) };
                        let v = mov - fix;
                        let step = 15f64.to_radians();
                        let t = (v.y.atan2(v.x) / step).round() * step;
                        let q = fix + Pt::new(t.cos(), t.sin()) * v.len();
                        if end == 0 {
                            a = q;
                        } else {
                            b = q;
                        }
                    }
                    g.set_line(a, b, &r);
                }
                Drag::GradientEnd { id, end, r }
            }
            Drag::GradientStop { id, idx, r } => {
                self.touch();
                let tg = self.grad_target;
                if let Some(g) = self
                    .doc
                    .object_mut(id)
                    .and_then(|o| o.style.grad_slot(tg).as_mut())
                {
                    let (a, b) = g.line(&r);
                    let ab = b - a;
                    let l2 = (ab.x * ab.x + ab.y * ab.y).max(1e-12);
                    let t = (((p - a).x * ab.x + (p - a).y * ab.y) / l2).clamp(0.0, 1.0);
                    if let Some(st) = g.stops.get_mut(idx) {
                        st.pos = t;
                    }
                }
                Drag::GradientStop { id, idx, r }
            }
            Drag::GuideNew { idx, origin } => {
                let v = p - origin;
                if v.len() * self.view.zoom > 4.0 {
                    let mut ang = (-v.y).atan2(v.x).to_degrees();
                    let step = (ang / 45.0).round() * 45.0;
                    // Umschalt: 45°-Schritte; auch ohne rastet ein Winkel knapp daneben ein,
                    // damit waagerechte und senkrechte Linien nicht ungewollt schräg werden.
                    if mods.shift || (ang - step).abs() < 3.0 {
                        ang = step;
                    }
                    // Linie: 0° und 180° sind dasselbe
                    let mut ang = normalize_angle(ang);
                    if ang <= -90.0 {
                        ang += 180.0;
                    } else if ang > 90.0 {
                        ang -= 180.0;
                    }
                    if let Some(g) = self.doc.guides.get_mut(idx) {
                        g.angle = ang;
                    }
                    self.guide_angle = ang;
                }
                Drag::GuideNew { idx, origin }
            }
            Drag::Guide { idx, start, orig } => {
                self.touch();
                let (horizontal, vertical) = self
                    .doc
                    .guides
                    .get(idx)
                    .map(|g| (g.is_horizontal(), g.is_vertical()))
                    .unwrap_or((false, false));
                let mut pos = orig + (p - start);
                // an Ankerpunkten, Ecken, Mitten und Kanten einrasten (wie beim Anlegen);
                // gemessen wird dort, wo der Zeiger ist
                let probe = if horizontal {
                    Pt::new(p.x, pos.y)
                } else if vertical {
                    Pt::new(pos.x, p.y)
                } else {
                    pos
                };
                let q = self.snap_with(probe, self.snap_ctx.as_ref(), false);
                // Waagerechte nur senkrecht bewegen, senkrechte nur waagerecht
                if horizontal {
                    pos = Pt::new(orig.x, q.y);
                } else if vertical {
                    pos = Pt::new(q.x, orig.y);
                } else {
                    pos = q;
                }
                if let Some(g) = self.doc.guides.get_mut(idx) {
                    g.pos = pos;
                }
                Drag::Guide { idx, start, orig }
            }
            Drag::TextSelect { id } => {
                if let (Some(te), Some(o)) = (self.text_edit, self.doc.object(id)) {
                    if let Some(i) = m.text_index(o, p) {
                        self.text_edit = Some(TextEdit { caret: i, ..te });
                    }
                }
                Drag::TextSelect { id }
            }
            Drag::Fillet { corner, off } => {
                let b = corner.bisector();
                let along = (p - corner.p).x * b.x + (p - corner.p).y * b.y;
                let r = ((along - off) / fillet_factor(corner.theta)).max(0.0);
                self.set_node_radius(r);
                // Radius der gezogenen Ecke (nach dem Begrenzen)
                let shown = self
                    .node_views()
                    .iter()
                    .flat_map(|(_, subs)| subs.iter().flat_map(|sp| sp.nodes.iter()))
                    .find(|n| n.p.dist(corner.p) < 1e-6)
                    .map(|n| n.r)
                    .unwrap_or(r);
                self.badge = Some((s, Badge::Radius(shown)));
                Drag::Fillet { corner, off }
            }
            Drag::AreaCreate { id, start, was_dirty } => {
                let r = Rect::from_points(start, self.snap(p));
                if let Some(Object {
                    shape: Shape::TextArea { xf, w, h, .. },
                    ..
                }) = self.doc.object_mut(id)
                {
                    *xf = Affine::translate(r.x0, r.y0);
                    *w = r.width().max(1.0);
                    *h = r.height().max(1.0);
                }
                Drag::AreaCreate { id, start, was_dirty }
            }
            Drag::NodeRubber {
                start,
                base,
                base_objs,
                add,
                had_nodes,
                ..
            } => {
                let r = Rect::from_points(start, p);
                let page = self.page_clip();
                let mut sel = base.clone();
                // Knoten aller bearbeitbaren Objekte im Rahmen (nicht nur der ausgewählten)
                let ids: Vec<u64> = self
                    .doc
                    .layers
                    .iter()
                    .filter(|l| l.editable())
                    .flat_map(|l| l.objects.iter().map(|o| o.id))
                    .collect();
                for id in ids {
                    let Some(subs) = self.node_view_of(id) else {
                        continue;
                    };
                    for (si, sp) in subs.iter().enumerate() {
                        for (ni, n) in sp.nodes.iter().enumerate() {
                            let visible = page.map_or(true, |pg| pg.contains(n.p));
                            if r.contains(n.p) && visible && !sel.contains(&(id, si, ni)) {
                                sel.push((id, si, ni));
                            }
                        }
                    }
                }
                // Objektauswahl: mit Umschalt die bisherige plus die erfassten Objekte, sonst
                // nur die erfassten (erfasst der Rahmen nichts, bleibt die Auswahl)
                let mut objs: Vec<u64> = if !add && !sel.is_empty() {
                    Vec::new()
                } else {
                    base_objs.clone()
                };
                for &(id, _, _) in &sel {
                    if !objs.contains(&id) {
                        objs.push(id);
                    }
                }
                self.selection = objs;
                self.node_sel = sel;
                Drag::NodeRubber {
                    start,
                    cur: p,
                    base,
                    base_objs,
                    add,
                    had_nodes,
                }
            }
        };
    }

    pub fn release(&mut self, s: Pt, mods: Mods, m: &dyn Metrics) {
        self.badge = None;
        self.motion(s, mods, m);
        self.press_at = None;
        self.hints.borrow_mut().clear();
        self.snap_ctx = None;
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        if let Drag::Move { .. } = drag {
            if let (Some(t), [img]) = (self.frame_ready(), &self.selection[..]) {
                let img = *img;
                self.place_in_frame(img, t, m);
            }
            self.frame_hover = None;
        }
        match drag {
            Drag::Move { .. }
            | Drag::Scale { .. }
            | Drag::Rotate { .. }
            | Drag::NodeEdit { .. }
            | Drag::GradientEnd { .. }
            | Drag::GradientStop { .. }
            | Drag::GuideNew { .. }
            | Drag::Artboard { .. }
            | Drag::ArtboardSize { .. }
            | Drag::TextOffset { .. } => self.end_change(),
            Drag::Guide { idx, .. } => {
                // Zurück ins Lineal gezogen: Hilfslinie löschen
                let in_ruler = self.ruler > 0.0 && (s.x < self.ruler || s.y < self.ruler);
                if in_ruler && idx < self.doc.guides.len() {
                    self.touch();
                    self.doc.guides.remove(idx);
                    self.guide_sel = None;
                }
                self.end_change();
            }
            Drag::AreaCreate { id, start, .. } => {
                let small = self
                    .doc
                    .object(id)
                    .and_then(|o| o.shape.geom_bbox())
                    .map(|b| b.width().min(b.height()) * self.view.zoom < 12.0)
                    .unwrap_or(true);
                if small {
                    // Nur geklickt: Feld in Standardgröße
                    if let Some(Object {
                        shape: Shape::TextArea { xf, w, h, .. },
                        ..
                    }) = self.doc.object_mut(id)
                    {
                        *xf = Affine::translate(start.x, start.y);
                        *w = 320.0;
                        *h = 180.0;
                    }
                }
                // Feld mit Blindtext füllen und alles markieren
                self.placeholders.push(id);
                self.refill_placeholders(m);
                self.begin_text_edit(id, None);
            }
            Drag::NodeRubber {
                start,
                cur,
                base_objs,
                add,
                had_nodes,
                ..
            } => {
                if start.dist(cur) * self.view.zoom < 2.0 {
                    self.selection = base_objs;
                    match self.hit(start, m).filter(|h| self.selection.contains(h)) {
                        Some(h) if add => {
                            // Umschalt + Klick auf ein gewähltes Objekt: aus der Auswahl nehmen
                            self.selection.retain(|x| *x != h);
                            self.node_sel.retain(|n| n.0 != h);
                        }
                        Some(h) => {
                            // Klick auf das Objekt selbst: nur dieses, mit allen Knoten
                            self.selection = vec![h];
                            self.select_all_nodes();
                        }
                        // Klick ins Leere ohne gewählte Knoten hebt auch die Objektauswahl auf.
                        None if !had_nodes && !add => self.deselect(),
                        None => {}
                    }
                }
            }
            Drag::Create { id, was_dirty, .. } => {
                let tiny = self
                    .doc
                    .object(id)
                    .and_then(|o| m.bbox(o))
                    .map(|b| b.width().max(b.height()) * self.view.zoom < 2.0)
                    .unwrap_or(true);
                if tiny {
                    // Nur geklickt: Objekt wieder entfernen, Schritt verwerfen.
                    self.doc.remove_object(id);
                    if let Some(prev) = self.undo.pop() {
                        self.doc = prev;
                    }
                    self.selection.clear();
                    self.dirty = was_dirty;
                }
            }
            Drag::Erase { pts } => {
                if self.tool == Tool::Scissors {
                    self.cut_along(&pts, m);
                } else {
                    self.erase_along(&pts, m);
                }
            }
            Drag::Build { hits, alt, .. } => {
                self.builder_apply(&hits, alt, m);
            }
            Drag::Pencil { pts } => {
                let z = self.view.zoom;
                let total: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
                let close_d = self.pencil_close.max(0.0) / z;
                // Anfang und Ende nah beieinander: geschlossene Form
                let closed = pts.len() >= 4
                    && close_d > 0.0
                    && pts[0].dist(pts[pts.len() - 1]) <= close_d
                    && total > close_d * 4.0;
                let sm = smooth_points(&pts, self.pencil_smooth, closed);
                let mut pts = simplify(&sm, (1.0 + self.pencil_smooth * 5.0) / z);
                if closed {
                    while pts.len() > 3 && pts[pts.len() - 1].dist(pts[0]) <= close_d {
                        pts.pop();
                    }
                }
                if pts.len() >= 2 {
                    if let Some(li) = self.target_layer() {
                        self.checkpoint(None);
                        let closed = closed && pts.len() >= 3;
                        let nodes = if closed {
                            let v: Vec<(Pt, bool)> = pts.iter().map(|p| (*p, false)).collect();
                            curvature_nodes(&v, true)
                        } else {
                            smooth_nodes(&pts)
                        };
                        let shape = Shape::Path {
                            subpaths: vec![SubPath { nodes, closed }],
                            evenodd: false,
                        };
                        let style = if closed {
                            self.defaults.clone()
                        } else {
                            self.line_style()
                        };
                        let id = self.insert_object(li, shape, style);
                        self.selection = vec![id];
                    }
                }
            }
            _ => {}
        }
    }

    pub fn double_click(&mut self, s: Pt, m: &dyn Metrics) {
        let p = self.view.to_doc(s);
        match self.tool {
            Tool::Pen => {
                if let Some(nodes) = self.pen.as_mut() {
                    let n = nodes.len();
                    if n >= 2 && nodes[n - 1].p.dist(nodes[n - 2].p) * self.view.zoom < 4.0 {
                        nodes.pop();
                    }
                }
                self.finish_pen(false);
                self.drag = Drag::None;
            }
            Tool::Curvature => {
                if let Some(pts) = self.curv.as_mut() {
                    let n = pts.len();
                    if n >= 2 && pts[n - 1].0.dist(pts[n - 2].0) * self.view.zoom < 4.0 {
                        pts.pop();
                    }
                }
                self.finish_curvature(false);
                self.drag = Drag::None;
            }
            Tool::Text | Tool::TextArea | Tool::TextPath if self.text_edit.is_some() => {
                // Doppelklick im Text: Wort markieren
                if let Some((te, t)) = self.edit_text() {
                    let (a, b) = word_at(&t, te.caret);
                    self.text_edit = Some(TextEdit {
                        id: te.id,
                        caret: b,
                        anchor: a,
                    });
                }
            }
            Tool::Select => {
                if let Some(id) = self.hit(p, m) {
                    match self.doc.object(id).map(|o| &o.shape) {
                        Some(Shape::Text { .. }) | Some(Shape::TextArea { .. }) => {
                            self.selection = vec![id];
                            let i = self.doc.object(id).and_then(|o| m.text_index(o, p));
                            self.tool = Tool::Text;
                            self.begin_text_edit(id, i);
                        }
                        Some(Shape::TextPath { .. }) => {
                            self.selection = vec![id];
                            let i = self.doc.object(id).and_then(|o| m.text_index(o, p));
                            self.tool = Tool::TextPath;
                            self.begin_text_edit(id, i);
                        }
                        Some(Shape::Path { .. }) => {
                            self.selection = vec![id];
                            self.tool = Tool::Node;
                        }
                        Some(Shape::Image { clip: Some(_), .. }) => {
                            // Doppelklick: zwischen Bildinhalt und ganzem Objekt wechseln
                            self.selection = vec![id];
                            let content = self.image_part(id) == ImagePart::Content;
                            self.frame_mode = Some((
                                id,
                                if content {
                                    ImagePart::All
                                } else {
                                    ImagePart::Content
                                },
                            ));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    pub fn set_tool(&mut self, tool: Tool) {
        if self.tool == tool {
            return;
        }
        self.finish_pen(false);
        self.finish_curvature(false);
        if !matches!(tool, Tool::Text | Tool::TextArea | Tool::TextPath) {
            self.end_text_edit();
        }
        if tool != Tool::Guide {
            self.guide_sel = None;
        }
        self.drag = Drag::None;
        self.tool = tool;
        self.node_sel.clear();
        if tool == Tool::Node {
            self.select_all_nodes();
        }
        self.hover = None;
    }

    /// Escape: Zeichenstift beenden oder Auswahl aufheben.
    pub fn escape(&mut self) {
        if self.frame_mode.map(|f| f.1 != ImagePart::All).unwrap_or(false) {
            self.frame_mode = None;
        } else if self.text_edit.is_some() {
            self.end_text_edit();
        } else if self.pen.is_some() {
            self.finish_pen(false);
        } else if self.curv.is_some() {
            self.finish_curvature(false);
        } else if !self.node_sel.is_empty() {
            self.node_sel.clear();
        } else {
            self.deselect();
        }
    }

    // ───────────── Ansicht ─────────────

    pub fn fit(&mut self, w: f64, h: f64) {
        let margin = 48.0;
        let b = self.page_clip().unwrap_or_else(|| self.doc.artboards_bbox());
        let z = ((w - 2.0 * margin) / b.width())
            .min((h - 2.0 * margin) / b.height())
            .clamp(0.02, 64.0);
        self.view.zoom = z;
        self.view.ox = ((w - b.width() * z) / 2.0 - b.x0 * z).round();
        self.view.oy = ((h - b.height() * z) / 2.0 - b.y0 * z).round();
        self.needs_fit = false;
    }

    // ───────────── Notizheft ─────────────

    /// Aktuelle Seite in der Notizheft-Ansicht (an die Zahl der Zeichenflächen angepasst).
    pub fn page_index(&self) -> Option<usize> {
        let n = self.doc.artboard_rects().len();
        self.notebook.map(|i| i.min(n.saturating_sub(1)))
    }

    /// Sichtbarer Bereich in der Notizheft-Ansicht (sonst `None`: alles sichtbar).
    pub fn page_clip(&self) -> Option<Rect> {
        let i = self.page_index()?;
        self.doc.artboard_rects().get(i).map(|(_, r)| *r)
    }

    /// Notizheft-Ansicht ein- oder ausschalten. Beim Einschalten wird die Seite unter der
    /// Bildmitte (oder die nächstgelegene) gezeigt.
    pub fn set_notebook(&mut self, on: bool) {
        if !on {
            self.notebook = None;
            return;
        }
        if self.notebook.is_some() {
            return;
        }
        let (w, h) = self.viewport;
        let c = self.view.to_doc(Pt::new(w / 2.0, h / 2.0));
        let rects = self.doc.artboard_rects();
        let dist = |r: &Rect| {
            let dx = (r.x0 - c.x).max(c.x - r.x1).max(0.0);
            let dy = (r.y0 - c.y).max(c.y - r.y1).max(0.0);
            dx * dx + dy * dy
        };
        let i = (0..rects.len())
            .min_by(|&a, &b| dist(&rects[a].1).total_cmp(&dist(&rects[b].1)))
            .unwrap_or(0);
        self.notebook = Some(i);
        self.notebook_acc = 0.0;
        if w > 0.0 && h > 0.0 {
            self.fit(w, h);
        }
    }

    /// Zu Seite `i` blättern (Zoom bleibt). `at_end`: das Seitenende zeigen statt des Anfangs.
    pub fn go_page(&mut self, i: usize, at_end: bool) -> bool {
        let n = self.doc.artboard_rects().len();
        if self.notebook.is_none() || i >= n {
            return false;
        }
        self.notebook = Some(i);
        self.notebook_acc = 0.0;
        self.selection.clear();
        self.node_sel.clear();
        let Some(r) = self.page_clip() else { return false };
        let (w, h) = self.viewport;
        let z = self.view.zoom;
        let m = PAGE_MARGIN;
        self.view.ox = if r.width() * z + 2.0 * m <= w {
            ((w - r.width() * z) / 2.0 - r.x0 * z).round()
        } else {
            m - r.x0 * z
        };
        self.view.oy = if r.height() * z + 2.0 * m <= h {
            ((h - r.height() * z) / 2.0 - r.y0 * z).round()
        } else if at_end {
            h - m - r.y1 * z
        } else {
            m - r.y0 * z
        };
        true
    }

    /// Scrollen in der Notizheft-Ansicht (Bildschirmpixel). Die Seite bleibt im Bild;
    /// wer am Ende weiterscrollt, blättert um. Liefert `true`, wenn umgeblättert wurde.
    pub fn notebook_scroll(&mut self, dx: f64, dy: f64) -> bool {
        let Some(r) = self.page_clip() else {
            self.view.ox -= dx;
            self.view.oy -= dy;
            return false;
        };
        let (w, h) = self.viewport;
        let z = self.view.zoom;
        let m = PAGE_MARGIN;
        // Bereich, in dem die Seite verschoben werden darf (passt sie ganz hinein: keiner)
        let range = |size: f64, lo: f64, hi: f64, view: f64| -> (f64, f64) {
            if size * z + 2.0 * m <= view {
                let o = ((view - size * z) / 2.0 - lo * z).round();
                (o, o)
            } else {
                (view - m - hi * z, m - lo * z)
            }
        };
        let (x_min, x_max) = range(r.width(), r.x0, r.x1, w);
        self.view.ox = (self.view.ox - dx).clamp(x_min, x_max);
        let (y_min, y_max) = range(r.height(), r.y0, r.y1, h);
        let old = self.view.oy;
        let want = old - dy;
        self.view.oy = want.clamp(y_min, y_max);
        let over = want - self.view.oy;
        // Hat sich die Seite noch bewegt, bleibt sie erst einmal am Ende stehen
        if over == 0.0 || dy == 0.0 || (self.view.oy - old).abs() > 0.5 {
            if dy != 0.0 {
                self.notebook_acc = 0.0;
            }
            return false;
        }
        // Richtungswechsel setzt den Überhang zurück
        if over.signum() != self.notebook_acc.signum() {
            self.notebook_acc = 0.0;
        }
        self.notebook_acc += over;
        if self.notebook_acc.abs() < PAGE_FLIP {
            return false;
        }
        let i = self.page_index().unwrap_or(0);
        if self.notebook_acc < 0.0 {
            self.go_page(i + 1, false)
        } else if i > 0 {
            self.go_page(i - 1, true)
        } else {
            self.notebook_acc = 0.0;
            false
        }
    }

    // ───────────── Zeichenflächen ─────────────

    /// Neue Zeichenfläche rechts neben den vorhandenen (gleiche Größe wie die erste).
    pub fn add_artboard(&mut self) -> usize {
        self.checkpoint(None);
        let b = self.doc.artboards_bbox();
        let name = self.doc.next_artboard_name();
        self.doc.artboards.push(crate::model::Artboard {
            name,
            x: b.x1 + 40.0,
            y: 0.0,
            w: self.doc.width,
            h: self.doc.height,
        });
        // Notizheft: gleich auf die neue Seite blättern
        if self.notebook.is_some() {
            self.notebook = Some(self.doc.artboards.len());
        }
        self.needs_fit = true;
        self.layers_version += 1;
        self.doc.artboards.len()
    }

    /// Zeichenfläche `i` mit allem, was ganz darauf liegt, kopieren; die Kopie kommt rechts
    /// neben alle vorhandenen Zeichenflächen. Liefert die Nummer der neuen Zeichenfläche.
    pub fn duplicate_artboard(&mut self, i: usize, m: &dyn Metrics) -> Option<usize> {
        let rects = self.doc.artboard_rects();
        let (name, r) = rects.get(i).cloned()?;
        let bbox = self.doc.artboards_bbox();
        let offset = Pt::new(bbox.x1 + 40.0 - r.x0, 0.0);
        let inside = r.inflate(0.5);
        self.checkpoint(None);
        // Name „… (Kopie)“, bei Bedarf mit Nummer
        let names: Vec<String> = rects.iter().map(|(n, _)| n.clone()).collect();
        let base = trf("{} (Kopie)", &[&name]);
        let mut new_name = base.clone();
        let mut k = 2;
        while names.contains(&new_name) {
            new_name = format!("{base} {k}");
            k += 1;
        }
        self.doc.artboards.push(crate::model::Artboard {
            name: new_name,
            x: r.x0 + offset.x,
            y: r.y0,
            w: r.width(),
            h: r.height(),
        });
        // Objekte jeweils auf ihrer Ebene kopieren; Gruppen bekommen neue Nummern
        let mut next_group = self.doc.max_group();
        let mut regroup: Vec<(u64, u64)> = Vec::new();
        let mut copies: Vec<(usize, Object)> = Vec::new();
        for (li, l) in self.doc.layers.iter().enumerate() {
            for o in &l.objects {
                if m.bbox(o).map(|b| inside.contains_rect(&b)).unwrap_or(false) {
                    copies.push((li, o.clone()));
                }
            }
        }
        let mut ids = Vec::new();
        for (li, mut o) in copies {
            o.id = self.doc.alloc_id();
            if o.group != 0 {
                o.group = match regroup.iter().find(|(old, _)| *old == o.group) {
                    Some((_, new)) => *new,
                    None => {
                        next_group += 1;
                        regroup.push((o.group, next_group));
                        next_group
                    }
                };
            }
            o.shape.translate(offset);
            ids.push(o.id);
            self.doc.layers[li].objects.push(o);
        }
        self.selection.clear();
        if self.notebook.is_some() {
            self.notebook = Some(self.doc.artboards.len());
        }
        self.needs_fit = true;
        self.layers_version += 1;
        Some(self.doc.artboards.len())
    }

    /// Zusätzliche Zeichenfläche entfernen (die Objekte darauf bleiben erhalten).
    /// Jede Zeichenfläche lässt sich entfernen, solange eine weitere bleibt. Fällt die erste
    /// weg, wird die nächste zur Hauptzeichenfläche: sie rückt in den Ursprung, alles andere
    /// wandert mit, und die Ansicht folgt, so dass auf dem Bildschirm nichts springt.
    pub fn delete_artboard(&mut self, i: usize) -> bool {
        if self.doc.artboards.is_empty() || i > self.doc.artboards.len() {
            return false;
        }
        self.checkpoint(None);
        if i == 0 {
            let a = self.doc.artboards.remove(0);
            let t = Pt::new(-a.x, -a.y);
            self.doc.width = a.w;
            self.doc.height = a.h;
            if t.x != 0.0 || t.y != 0.0 {
                for l in &mut self.doc.layers {
                    for o in &mut l.objects {
                        o.shape.translate(t);
                    }
                }
                for g in &mut self.doc.guides {
                    g.pos = g.pos + t;
                }
                for b in &mut self.doc.artboards {
                    b.x += t.x;
                    b.y += t.y;
                }
                self.view.ox -= t.x * self.view.zoom;
                self.view.oy -= t.y * self.view.zoom;
            }
        } else {
            self.doc.artboards.remove(i - 1);
        }
        // Notizheft: auf derselben (bzw. der vorigen) Seite bleiben
        if let Some(p) = self.notebook {
            let p = if p >= i && p > 0 { p - 1 } else { p };
            self.notebook = Some(p.min(self.doc.artboards.len()));
        }
        self.layers_version += 1;
        true
    }

    /// Größe einer Zeichenfläche ändern (0 = Hauptzeichenfläche).
    pub fn set_artboard_size(&mut self, i: usize, w: f64, h: f64) -> bool {
        let (w, h) = (w.max(1.0), h.max(1.0));
        if i == 0 {
            if self.doc.width == w && self.doc.height == h {
                return false;
            }
            self.checkpoint(None);
            self.doc.width = w;
            self.doc.height = h;
        } else {
            let Some(a) = self.doc.artboards.get(i - 1) else {
                return false;
            };
            if a.w == w && a.h == h {
                return false;
            }
            self.checkpoint(None);
            let a = &mut self.doc.artboards[i - 1];
            a.w = w;
            a.h = h;
        }
        self.layers_version += 1;
        true
    }

    pub fn rename_artboard(&mut self, i: usize, name: &str) -> bool {
        let name = name.trim();
        if i == 0 || name.is_empty() || i > self.doc.artboards.len() {
            return false;
        }
        self.checkpoint(None);
        self.doc.artboards[i - 1].name = name.to_string();
        self.layers_version += 1;
        true
    }

    /// Zeichenfläche, deren Namensschild (über der linken oberen Ecke) unter `s` liegt.
    /// Nur bei mehreren Zeichenflächen.
    pub fn artboard_label_at(&self, s: Pt) -> Option<usize> {
        if self.doc.artboards.is_empty() || !self.artboard_mode || self.notebook.is_some() {
            return None;
        }
        self.doc
            .artboard_rects()
            .iter()
            .enumerate()
            .rev()
            .find(|(_, (name, r))| {
                let a = self.view.to_screen(Pt::new(r.x0, r.y0));
                let w = 8.0 * name.chars().count() as f64 + 12.0;
                s.x >= a.x - 2.0 && s.x <= a.x + w && s.y >= a.y - 22.0 && s.y <= a.y
            })
            .map(|(i, _)| i)
    }

    /// Anfasser der Zeichenflächen im Bildschirm: (Zeichenfläche, Anfasser 0–7, Lage).
    /// Nur im Zeichenflächen-Modus mit dem Auswahlwerkzeug.
    pub fn artboard_handles(&self) -> Vec<(usize, usize, Pt)> {
        if !self.artboard_mode || self.tool != Tool::Select {
            return Vec::new();
        }
        let mut out = Vec::new();
        let only = self.page_index();
        for (i, (_, r)) in self.doc.artboard_rects().iter().enumerate() {
            if only.is_some_and(|o| o != i) {
                continue;
            }
            let a = self.view.to_screen(Pt::new(r.x0, r.y0));
            let b = self.view.to_screen(Pt::new(r.x1, r.y1));
            let m = a.lerp(b, 0.5);
            let pts = [
                Pt::new(a.x, a.y),
                Pt::new(m.x, a.y),
                Pt::new(b.x, a.y),
                Pt::new(b.x, m.y),
                Pt::new(b.x, b.y),
                Pt::new(m.x, b.y),
                Pt::new(a.x, b.y),
                Pt::new(a.x, m.y),
            ];
            for (h, p) in pts.into_iter().enumerate() {
                out.push((i, h, p));
            }
        }
        out
    }

    fn artboard_handle_at(&self, s: Pt) -> Option<(usize, usize)> {
        self.artboard_handles()
            .into_iter()
            .filter(|(_, _, p)| p.dist(s) <= HANDLE_RADIUS + 2.0)
            .min_by(|a, b| a.2.dist(s).total_cmp(&b.2.dist(s)))
            .map(|(i, h, _)| (i, h))
    }

    /// Zeichenfläche `idx` so ändern, dass Anfasser `handle` bei `q` liegt. Liefert die
    /// Verschiebung aller übrigen Inhalte (nur bei der ersten Zeichenfläche nicht null).
    fn artboard_resize(
        &mut self,
        idx: usize,
        handle: usize,
        r: Rect,
        q: Pt,
        orig: &Document,
        view: View,
    ) -> Pt {
        let (mut x0, mut y0, mut x1, mut y1) = (r.x0, r.y0, r.x1, r.y1);
        if matches!(handle, 0 | 6 | 7) {
            x0 = q.x.min(x1 - 1.0);
        }
        if matches!(handle, 2 | 3 | 4) {
            x1 = q.x.max(x0 + 1.0);
        }
        if matches!(handle, 0 | 1 | 2) {
            y0 = q.y.min(y1 - 1.0);
        }
        if matches!(handle, 4 | 5 | 6) {
            y1 = q.y.max(y0 + 1.0);
        }
        let mut doc = orig.clone();
        if idx == 0 {
            // Die erste Zeichenfläche beginnt immer bei (0,0): alles andere wandert entgegen,
            // die Ansicht folgt, so dass auf dem Bildschirm nichts springt.
            let t = Pt::new(-x0, -y0);
            doc.width = x1 - x0;
            doc.height = y1 - y0;
            if t.x != 0.0 || t.y != 0.0 {
                for l in &mut doc.layers {
                    for o in &mut l.objects {
                        o.shape.translate(t);
                    }
                }
                for g in &mut doc.guides {
                    g.pos = g.pos + t;
                }
                for a in &mut doc.artboards {
                    a.x += t.x;
                    a.y += t.y;
                }
            }
            self.view = view;
            self.view.ox -= t.x * view.zoom;
            self.view.oy -= t.y * view.zoom;
        } else if let Some(a) = doc.artboards.get_mut(idx - 1) {
            a.x = x0;
            a.y = y0;
            a.w = x1 - x0;
            a.h = y1 - y0;
        }
        self.doc = doc;
        self.layers_version += 1;
        if idx == 0 {
            Pt::new(-x0, -y0)
        } else {
            Pt::new(0.0, 0.0)
        }
    }

    /// Zeichenfläche, in der der Punkt liegt (sonst die erste).
    pub fn artboard_at(&self, p: Pt) -> Rect {
        let rects = self.doc.artboard_rects();
        rects
            .iter()
            .find(|(_, r)| r.contains(p))
            .map(|(_, r)| *r)
            .unwrap_or(rects[0].1)
    }

    /// Zeichenfläche `idx` (mit allem, was ganz darauf liegt) um `d` verschieben.
    fn artboard_drag(&mut self, idx: usize, d: Pt, orig: &Document, view: View, inside: &[u64]) {
        let mut doc = orig.clone();
        if idx == 0 {
            // Die erste Zeichenfläche bleibt im Ursprung: alles andere wandert entgegen,
            // die Ansicht folgt, so dass es auf dem Bildschirm stehen bleibt.
            let t = Pt::new(-d.x, -d.y);
            for l in &mut doc.layers {
                for o in &mut l.objects {
                    if !inside.contains(&o.id) {
                        o.shape.translate(t);
                    }
                }
            }
            for g in &mut doc.guides {
                g.pos = g.pos + t;
            }
            for a in &mut doc.artboards {
                a.x += t.x;
                a.y += t.y;
            }
            self.view = view;
            self.view.ox += d.x * view.zoom;
            self.view.oy += d.y * view.zoom;
        } else {
            if let Some(a) = doc.artboards.get_mut(idx - 1) {
                a.x += d.x;
                a.y += d.y;
            }
            for l in &mut doc.layers {
                for o in &mut l.objects {
                    if inside.contains(&o.id) {
                        o.shape.translate(d);
                    }
                }
            }
        }
        self.doc = doc;
    }

    // ───────────── Skalieren ─────────────

    /// Auswahl um ihren Mittelpunkt skalieren (1 = 100 %). `strokes`: Linienstärken und
    /// Schatten mitskalieren.
    pub fn scale_selection(&mut self, sx: f64, sy: f64, strokes: bool, m: &dyn Metrics) -> bool {
        if self.selection_locked() {
            self.message = Some(tr("Gesperrte Objekte lassen sich nicht verändern").into());
            return false;
        }
        if sx <= 0.0 || sy <= 0.0 || ((sx - 1.0).abs() < 1e-9 && (sy - 1.0).abs() < 1e-9) {
            return false;
        }
        if !self.transform_selection(m, |c| Affine::scale_about(sx, sy, c)) {
            return false;
        }
        if strokes {
            let k = (sx * sy).sqrt();
            for id in self.selection.clone() {
                if let Some(o) = self.doc.object_mut(id) {
                    o.style.width *= k;
                    // Kontur des Textrahmens wie andere Konturen
                    match &mut o.shape {
                        Shape::Text { fmt, .. } | Shape::TextPath { fmt, .. } => fmt.box_width *= k,
                        Shape::TextArea { frame, .. } => frame.width *= k,
                        _ => {}
                    }
                    if let Some(sh) = o.style.shadow.as_mut() {
                        sh.blur *= k;
                        sh.dx *= k;
                        sh.dy *= k;
                    }
                }
            }
        }
        self.refill_placeholders(m);
        true
    }

    /// Objekte aus einem anderen Dokument an derselben Stelle einfügen.
    pub fn paste_objects(&mut self, objs: Vec<Object>) -> bool {
        self.paste_count = 0;
        self.insert_copies(objs, 0.0)
    }

    pub fn zoom_at(&mut self, factor: f64, s: Pt) {
        let old = self.view.zoom;
        let new = (old * factor).clamp(0.02, 64.0);
        self.view.ox = s.x - (s.x - self.view.ox) * new / old;
        self.view.oy = s.y - (s.y - self.view.oy) * new / old;
        self.view.zoom = new;
    }

    // ───────────── Bearbeiten ─────────────

    pub fn delete_selection(&mut self) -> bool {
        if self.guide_sel.is_some() && (self.tool == Tool::Guide || self.selection.is_empty()) {
            return self.delete_guide();
        }
        if self.tool == Tool::Node && !self.node_sel.is_empty() {
            return self.node_delete();
        }
        if self.selection.is_empty() {
            return false;
        }
        if self.selection_locked() {
            self.message = Some(tr("Gesperrte Objekte lassen sich nicht löschen").into());
            return false;
        }
        self.checkpoint(None);
        for id in std::mem::take(&mut self.selection) {
            self.doc.remove_object(id);
        }
        self.node_sel.clear();
        true
    }

    pub fn copy(&mut self) -> bool {
        let objs = self.selected_in_order();
        if objs.is_empty() {
            return false;
        }
        self.clipboard = objs;
        self.paste_count = 0;
        true
    }

    pub fn cut(&mut self) -> bool {
        self.copy() && self.delete_selection()
    }

    fn insert_copies(&mut self, objs: Vec<Object>, offset: f64) -> bool {
        if objs.is_empty() {
            return false;
        }
        let Some(li) = self.target_layer() else {
            return false;
        };
        self.checkpoint(None);
        let mut ids = Vec::new();
        // Gruppen der Kopien bekommen neue Nummern; Kopien sind nie gesperrt
        let mut next_group = self.doc.max_group();
        let mut regroup: Vec<(u64, u64)> = Vec::new();
        for mut o in objs {
            o.id = self.doc.alloc_id();
            o.locked = false;
            if o.group != 0 {
                o.group = match regroup.iter().find(|(old, _)| *old == o.group) {
                    Some((_, new)) => *new,
                    None => {
                        next_group += 1;
                        regroup.push((o.group, next_group));
                        next_group
                    }
                };
            }
            o.shape.translate(Pt::new(offset, offset));
            ids.push(o.id);
            self.doc.layers[li].objects.push(o);
        }
        self.selection = ids;
        true
    }

    pub fn paste(&mut self) -> bool {
        self.paste_count += 1;
        let offset = 10.0 * self.paste_count as f64;
        self.insert_copies(self.clipboard.clone(), offset)
    }

    pub fn duplicate(&mut self) -> bool {
        let objs = self.selected_in_order();
        self.insert_copies(objs, 10.0)
    }

    pub fn nudge(&mut self, dx: f64, dy: f64) -> bool {
        if self.selection_locked() {
            self.message = Some(tr("Gesperrte Objekte lassen sich nicht verändern").into());
            return false;
        }
        if self.selection.is_empty() {
            return false;
        }
        self.checkpoint(Some("nudge"));
        for id in self.selection.clone() {
            if let Some(o) = self.doc.object_mut(id) {
                o.shape.translate(Pt::new(dx, dy));
            }
        }
        true
    }

    /// Anordnen innerhalb der Ebene. `delta` > 0 nach vorne.
    pub fn restack(&mut self, delta: i32, to_end: bool) -> bool {
        if self.selection.is_empty() {
            return false;
        }
        self.checkpoint(None);
        let mut ids: Vec<(usize, usize, u64)> = self
            .selection
            .iter()
            .filter_map(|id| self.doc.locate(*id).map(|(l, o)| (l, o, *id)))
            .collect();
        ids.sort();
        // Reihenfolge so wählen, dass sich die Objekte nicht gegenseitig überholen.
        let forward = (delta > 0) != to_end;
        if forward {
            ids.reverse();
        }
        for (_, _, id) in ids {
            self.doc.restack(id, delta, to_end);
        }
        true
    }

    /// Umriss eines Objekts als Teilpfade (Text über die Schriftmetriken).
    fn outline_of(obj: &Object, m: &dyn Metrics) -> Option<Vec<SubPath>> {
        match &obj.shape {
            Shape::Text { .. } | Shape::TextPath { .. } => m.text_outline(obj),
            Shape::Image { .. } => None,
            s => match s.to_path()? {
                Shape::Path { subpaths, .. } => Some(subpaths),
                _ => None,
            },
        }
    }

    pub fn convert_to_path(&mut self, m: &dyn Metrics) -> bool {
        if self.selection_locked() {
            self.message = Some(tr("Gesperrte Objekte lassen sich nicht verändern").into());
            return false;
        }
        // je Objekt die Teile; Texte werden in einzelne Buchstaben zerlegt
        let converted: Vec<(u64, Vec<Vec<SubPath>>)> = self
            .selected_objects()
            .into_iter()
            .filter(|o| !matches!(o.shape, Shape::Path { .. } | Shape::Image { .. }))
            .filter_map(|o| {
                let subpaths = Self::outline_of(o, m)?;
                let parts = if o.shape.is_text() {
                    let em = o.shape.text_fields().map(|t| t.2).unwrap_or(12.0);
                    let frame = match &o.shape {
                        Shape::Text { xf, .. } | Shape::TextArea { xf, .. } => Some(*xf),
                        _ => None,
                    };
                    glyph_parts(subpaths, frame, em)
                } else {
                    vec![subpaths]
                };
                Some((o.id, parts))
            })
            .collect();
        if converted.is_empty() {
            return false;
        }
        self.checkpoint(None);
        let mut next_group = self.doc.max_group();
        let mut selection = Vec::new();
        for (id, parts) in converted {
            // Glyphen nutzen die Nonzero-Regel; Rechteck & Co. ebenso.
            let path = |subpaths: Vec<SubPath>| Shape::Path {
                subpaths,
                evenodd: false,
            };
            if parts.len() <= 1 {
                if let (Some(o), Some(subs)) = (self.doc.object_mut(id), parts.into_iter().next()) {
                    o.shape = path(subs);
                }
                selection.push(id);
                continue;
            }
            // mehrere Buchstaben: je ein Pfad, als Gruppe an der Stelle des Textes
            let Some((li, oi)) = self.doc.locate(id) else {
                continue;
            };
            let orig = self.doc.layers[li].objects[oi].clone();
            let group = if orig.group != 0 {
                orig.group
            } else {
                next_group += 1;
                next_group
            };
            let mut objs = Vec::new();
            for (k, subs) in parts.into_iter().enumerate() {
                let mut o = orig.clone();
                if k > 0 {
                    o.id = self.doc.alloc_id();
                }
                o.shape = path(subs);
                o.group = group;
                selection.push(o.id);
                objs.push(o);
            }
            self.doc.layers[li].objects.splice(oi..oi + 1, objs);
        }
        self.selection = selection;
        self.expand_groups();
        self.node_sel.clear();
        true
    }

    // ───────────── Drehen, Spiegeln, Pfadoperationen ─────────────

    fn transform_selection(&mut self, m: &dyn Metrics, f: impl Fn(Pt) -> Affine) -> bool {
        if self.selection_locked() {
            self.message = Some(tr("Gesperrte Objekte lassen sich nicht verändern").into());
            return false;
        }
        let Some(b) = self.selection_bbox(m) else {
            return false;
        };
        let a = f(Pt::new((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0));
        self.checkpoint(None);
        for id in self.selection.clone() {
            self.transform_object(id, &a, m);
        }
        true
    }

    /// Objekt transformieren; Drehwinkel und Farbverlauf werden mitgeführt.
    fn transform_object(&mut self, id: u64, a: &Affine, m: &dyn Metrics) {
        let part = self.image_part(id);
        if part != ImagePart::All {
            if let Some(o) = self.doc.object_mut(id) {
                o.shape.transform_part(a, part);
            }
            return;
        }
        let before = self.doc.object(id).and_then(|o| m.bbox(o));
        let Some(o) = self.doc.object_mut(id) else { return };
        o.shape.transform(a);
        let det = a.a * a.d - a.b * a.c;
        if det < 0.0 {
            o.angle = normalize_angle(-o.angle);
        } else {
            // rotate_deg dreht im Uhrzeigersinn (y nach unten); der Winkel zählt gegen den Uhrzeigersinn.
            o.angle = normalize_angle(o.angle - a.b.atan2(a.a).to_degrees());
        }
        let after = self.doc.object(id).and_then(|o| m.bbox(o));
        if let (Some(b0), Some(b1)) = (before, after) {
            if let Some(o) = self.doc.object_mut(id) {
                for t in [PaintTarget::Fill, PaintTarget::Stroke, PaintTarget::Shadow] {
                    if let Some(g) = o.style.grad_slot(t).as_mut() {
                        g.transform(a, &b0, &b1);
                    }
                }
            }
        }
    }

    /// Auswahl auf einen bestimmten Drehwinkel bringen (Grad, gegen den Uhrzeigersinn).
    pub fn set_rotation(&mut self, deg: f64, m: &dyn Metrics) -> bool {
        if self.selection_locked() {
            self.message = Some(tr("Gesperrte Objekte lassen sich nicht verändern").into());
            return false;
        }
        let Some(cur) = self.selected_objects().first().map(|o| o.angle) else {
            return false;
        };
        let delta = normalize_angle(deg - cur);
        if delta.abs() < 1e-9 {
            return false;
        }
        let Some(b) = self.selection_bbox(m) else {
            return false;
        };
        let c = Pt::new((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
        self.checkpoint(Some("angle"));
        let a = Affine::rotate_about(-delta, c);
        for id in self.selection.clone() {
            self.transform_object(id, &a, m);
        }
        // Rundungsfehler vermeiden: angezeigter Wert = eingegebener Wert
        let target = normalize_angle(deg);
        if let Some(&first) = self.selection.first() {
            if let Some(o) = self.doc.object_mut(first) {
                o.angle = target;
            }
        }
        true
    }

    /// Auswahl um ihren Mittelpunkt drehen (Grad, positiv = im Uhrzeigersinn).
    pub fn rotate_selection(&mut self, deg: f64, m: &dyn Metrics) -> bool {
        self.transform_selection(m, |c| Affine::rotate_about(deg, c))
    }

    /// Auswahl waagerecht (`horizontal`) oder senkrecht spiegeln.
    pub fn flip_selection(&mut self, horizontal: bool, m: &dyn Metrics) -> bool {
        let (sx, sy) = if horizontal { (-1.0, 1.0) } else { (1.0, -1.0) };
        self.transform_selection(m, |c| Affine::scale_about(sx, sy, c))
    }

    /// Pfadoperation auf alle ausgewählten Objekte.
    pub fn boolean(&mut self, op: Op, m: &dyn Metrics) -> bool {
        if self.selection_locked() {
            self.message = Some(tr("Gesperrte Objekte lassen sich nicht verändern").into());
            return false;
        }
        let mut items: Vec<(usize, usize, u64)> = self
            .selection
            .iter()
            .filter_map(|id| self.doc.locate(*id).map(|(l, o)| (l, o, *id)))
            .collect();
        if items.len() < 2 {
            self.message = Some(tr("Dafür mindestens zwei Objekte auswählen").into());
            return false;
        }
        items.sort();
        let mut shapes = Vec::new();
        for (_, _, id) in &items {
            let Some(sp) = self.doc.object(*id).and_then(|o| Self::outline_of(o, m)) else {
                continue;
            };
            shapes.push(sp);
        }
        let tol = (0.15 / self.view.zoom).clamp(0.02, 0.5);
        let result = boolean::apply_many(&shapes, op, tol);
        if result.is_empty() {
            self.message = Some(tr("Das Ergebnis wäre leer").into());
            return false;
        }
        let (bottom_l, bottom_o, bottom_id) = items[0];
        let top_id = items[items.len() - 1].2;
        let style_src = if op == Op::Difference { bottom_id } else { top_id };
        let style = self
            .doc
            .object(style_src)
            .map(|o| o.style.clone())
            .unwrap_or_default();
        self.checkpoint(None);
        for (_, _, id) in &items {
            self.doc.remove_object(*id);
        }
        let id = self.doc.alloc_id();
        let layer = &mut self.doc.layers[bottom_l];
        let pos = bottom_o.min(layer.objects.len());
        layer.objects.insert(
            pos,
            Object {
                group: 0,
                locked: false,
                name: String::new(),
                id,
                shape: Shape::Path {
                    subpaths: result,
                    evenodd: true,
                },
                style,
                angle: 0.0,
            },
        );
        self.selection = vec![id];
        self.node_sel.clear();
        true
    }

    // ───────────── Knoten ─────────────

    /// `f` auf jeden gewählten Knoten anwenden (eine Rückgängig-Stufe).
    fn with_selected_nodes(&mut self, f: impl Fn(&mut SubPath, usize)) -> bool {
        let mut work: Vec<(u64, Vec<(usize, usize)>)> = Vec::new();
        for (id, subs) in self.node_views() {
            let valid: Vec<(usize, usize)> = self
                .nodes_of(id)
                .into_iter()
                .filter(|&(si, ni)| subs.get(si).map(|s| ni < s.nodes.len()).unwrap_or(false))
                .collect();
            if !valid.is_empty() {
                work.push((id, valid));
            }
        }
        if work.is_empty() {
            return false;
        }
        self.checkpoint(None);
        for (id, valid) in work {
            if let Some(subs) = self.subpaths_mut(id) {
                for (si, ni) in valid {
                    f(&mut subs[si], ni);
                }
            }
        }
        true
    }

    pub fn node_make_corner(&mut self) -> bool {
        self.with_selected_nodes(|sp, i| {
            let n = &mut sp.nodes[i];
            n.h_in = n.p;
            n.h_out = n.p;
        })
    }

    pub fn node_make_smooth(&mut self) -> bool {
        self.with_selected_nodes(|sp, i| {
            let len = sp.nodes.len();
            let prev = if i > 0 || sp.closed {
                Some(sp.nodes[(i + len - 1) % len].p)
            } else {
                None
            };
            let next = if i + 1 < len || sp.closed {
                Some(sp.nodes[(i + 1) % len].p)
            } else {
                None
            };
            let p = sp.nodes[i].p;
            let dir = match (prev, next) {
                (Some(a), Some(b)) => (b - a).normalized(),
                (None, Some(b)) => (b - p).normalized(),
                (Some(a), None) => (p - a).normalized(),
                _ => return,
            };
            let lin = prev.map(|a| a.dist(p) / 3.0).unwrap_or(0.0);
            let lout = next.map(|b| b.dist(p) / 3.0).unwrap_or(0.0);
            let n = &mut sp.nodes[i];
            n.h_in = p - dir * lin;
            n.h_out = p + dir * lout;
        })
    }

    /// Alle gewählten Knoten löschen; zu kleine Teilpfade verschwinden ganz.
    pub fn node_delete(&mut self) -> bool {
        let ids: Vec<u64> = self
            .node_views()
            .into_iter()
            .map(|v| v.0)
            .filter(|id| self.node_sel.iter().any(|n| n.0 == *id))
            .collect();
        if ids.is_empty() {
            return false;
        }
        self.checkpoint(None);
        for id in ids {
            let mut sel = self.nodes_of(id);
            sel.sort_unstable();
            sel.dedup();
            let mut remove = false;
            if let Some(subs) = self.subpaths_mut(id) {
                for &(si, ni) in sel.iter().rev() {
                    if let Some(sp) = subs.get_mut(si) {
                        if ni < sp.nodes.len() {
                            sp.nodes.remove(ni);
                        }
                    }
                }
                subs.retain(|sp| sp.nodes.len() >= 2);
                remove = subs.is_empty();
            }
            if !remove {
                continue;
            }
            // Bildrahmen ganz gelöscht: Bild bleibt, nur ohne Rahmen
            if let Some(Shape::Image { clip, .. }) = self.doc.object_mut(id).map(|o| &mut o.shape) {
                *clip = None;
            } else {
                self.doc.remove_object(id);
                self.selection.retain(|x| *x != id);
            }
        }
        self.node_sel.clear();
        true
    }

    /// Eckenradius der gewählten Knoten setzen (lebendige Ecken: jederzeit änderbar).
    /// Ein gewählter Knoten → nur dieser; mehrere → alle gemeinsam; keiner → alle Ecken.
    pub fn set_node_radius(&mut self, r: f64) -> bool {
        let r = r.max(0.0);
        // Rechtecke ohne Knotenauswahl (oder mit allen Knoten) haben einen eigenen Eckenradius.
        if self.selection.len() == 1 && (self.node_sel.is_empty() || self.all_nodes_selected()) {
            if let Some(Shape::Rect { .. }) = self.doc.object(self.selection[0]).map(|o| &o.shape) {
                return self.set_corner_radius(r);
            }
        }
        let views = self.node_views();
        if views.is_empty() {
            return false;
        }
        let targets: Vec<NodeRef> = if self.node_sel.is_empty() {
            let mut v = Vec::new();
            for (id, subs) in &views {
                for (si, sp) in subs.iter().enumerate() {
                    for ni in 0..sp.nodes.len() {
                        if corner_at(sp, ni).is_some() && (sp.nodes[ni].is_corner() || sp.nodes[ni].r > 0.0) {
                            v.push((*id, si, ni));
                        }
                    }
                }
            }
            v
        } else {
            self.node_sel.clone()
        };
        if targets.is_empty() {
            return false;
        }
        let node = |id: u64, si: usize, ni: usize| -> Option<Node> {
            views
                .iter()
                .find(|v| v.0 == id)
                .and_then(|v| v.1.get(si))
                .and_then(|s| s.nodes.get(ni))
                .copied()
        };
        let same = targets
            .iter()
            .all(|&(id, si, ni)| node(id, si, ni).map(|n| (n.r - r).abs() < 1e-9).unwrap_or(true));
        if same {
            return false;
        }
        // Radius je Ecke so begrenzen, wie Platz ist (Nachbarecken teilen sich die Kante)
        let limited: Vec<(NodeRef, f64)> = targets
            .iter()
            .map(|&(id, si, ni)| {
                let max = views
                    .iter()
                    .find(|v| v.0 == id)
                    .and_then(|v| v.1.get(si))
                    .and_then(|sp| {
                        crate::model::max_corner_radius(sp, ni, &|k| {
                            targets.contains(&(id, si, k))
                                || sp.nodes.get(k).map(|n| n.r > 0.0).unwrap_or(false)
                        })
                    })
                    .unwrap_or(r);
                ((id, si, ni), r.min(max))
            })
            .collect();
        let unchanged = limited
            .iter()
            .all(|&((id, si, ni), v)| node(id, si, ni).map(|n| (n.r - v).abs() < 1e-9).unwrap_or(true));
        if unchanged {
            return false;
        }
        // Gleiche Knoten kurz hintereinander = ein Rückgängig-Schritt
        self.checkpoint(Some(&format!("fillet{targets:?}")));
        for ((id, si, ni), v) in limited {
            if let Some(n) = self
                .subpaths_mut(id)
                .and_then(|subs| subs.get_mut(si))
                .and_then(|s| s.nodes.get_mut(ni))
            {
                n.r = v;
            }
        }
        true
    }

    /// Radius des ersten gewählten Knotens (für die Seitenleiste).
    fn current_fillet_radius(&self) -> f64 {
        let Some(&(id, si, ni)) = self.node_sel.first() else {
            return 0.0;
        };
        self.node_view_of(id)
            .and_then(|subs| subs.get(si).and_then(|s| s.nodes.get(ni)).map(|n| n.r))
            .unwrap_or(0.0)
    }

    /// Runde Ziehpunkte an gewählten Ecken: (Position, Ecke, Grundabstand) in Dokumentkoordinaten.
    pub fn corner_widgets(&self) -> Vec<(Pt, Corner, f64)> {
        if self.tool != Tool::Node || !matches!(self.drag, Drag::None | Drag::Fillet { .. }) {
            return Vec::new();
        }
        let off = 22.0 / self.view.zoom;
        let mut out = Vec::new();
        for (id, subs) in self.node_views() {
            for (si, ni) in self.nodes_of(id) {
                let Some(sp) = subs.get(si) else { continue };
                if ni >= sp.nodes.len() {
                    continue;
                }
                if let Some(c) = corner_at(sp, ni) {
                    let r = sp.nodes[ni].r;
                    // gleicher Abstand zum Rand der Rundung, egal wie groß der Radius ist
                    let d = off + r * fillet_factor(c.theta);
                    out.push((c.p + c.bisector() * d, c, off));
                }
            }
        }
        out
    }

    // ───────────── Kurvenzeichner, Textfeld, Ebenenwechsel ─────────────

    fn curv_press(&mut self, p: Pt, s: Pt, mods: Mods) {
        let p = match (mods.shift, self.curv.as_ref()) {
            (true, Some(pts)) => {
                let nodes: Vec<Node> = pts.iter().map(|(q, _)| Node::corner(*q)).collect();
                self.constrain_from(&nodes, p)
            }
            _ => self.snap(p),
        };
        if self.curv.is_none() {
            if self.target_layer().is_none() {
                return;
            }
            self.selection.clear();
            self.curv = Some(vec![(p, mods.alt)]);
            return;
        }
        let close = {
            let pts = self.curv.as_ref().unwrap();
            pts.len() >= 2 && self.view.to_screen(pts[0].0).dist(s) <= HANDLE_RADIUS + 1.0
        };
        if close {
            self.finish_curvature(true);
            return;
        }
        self.curv.as_mut().unwrap().push((p, mods.alt));
    }

    /// Linie des Kurvenzeichners, wie sie nach dem nächsten Klick aussieht: mit dem Zeiger
    /// als nächstem Punkt, über dem Startpunkt gleich als geschlossene Form.
    pub fn curvature_preview(&self) -> Option<(Vec<Node>, bool)> {
        let pts = self.curv.as_ref()?;
        if self.curvature_near_start() {
            return Some((curvature_nodes(pts, true), true));
        }
        let mut v = pts.clone();
        if let Some(ptr) = self.pointer {
            if v.last()
                .map(|l| l.0.dist(ptr) * self.view.zoom > 2.0)
                .unwrap_or(true)
            {
                v.push((ptr, false));
            }
        }
        Some((curvature_nodes(&v, false), false))
    }

    /// Steht der Zeiger beim Kurvenzeichner über dem Startpunkt (Klick schließt die Form)?
    pub fn curvature_near_start(&self) -> bool {
        match (self.curv.as_ref(), self.pointer) {
            (Some(pts), Some(p)) if pts.len() >= 2 => {
                self.view.to_screen(pts[0].0).dist(self.view.to_screen(p)) <= HANDLE_RADIUS + 1.0
            }
            _ => false,
        }
    }

    pub fn finish_curvature(&mut self, closed: bool) -> bool {
        let Some(pts) = self.curv.take() else { return false };
        if pts.len() < 2 {
            return false;
        }
        let Some(li) = self.target_layer() else {
            return false;
        };
        self.checkpoint(None);
        let style = if closed {
            self.defaults.clone()
        } else {
            self.line_style()
        };
        let nodes = curvature_nodes(&pts, closed);
        let shape = Shape::Path {
            subpaths: vec![SubPath { nodes, closed }],
            evenodd: false,
        };
        let id = self.insert_object(li, shape, style);
        self.selection = vec![id];
        true
    }

    /// Zeichenstift oder Kurvenzeichner abschließen (Enter).
    pub fn finish_drawing(&mut self) -> bool {
        self.finish_pen(false) || self.finish_curvature(false)
    }

    /// Letzten gesetzten Punkt beim Zeichnen entfernen (Rücktaste).
    pub fn remove_last_point(&mut self) -> bool {
        if let Some(n) = self.pen.as_mut() {
            n.pop();
            if n.is_empty() {
                self.pen = None;
            }
            return true;
        }
        if let Some(c) = self.curv.as_mut() {
            c.pop();
            if c.is_empty() {
                self.curv = None;
            }
            return true;
        }
        false
    }

    fn area_press(&mut self, p: Pt, m: &dyn Metrics) {
        if self.text_click(p, m) {
            return;
        }
        let p = self.snap(p);
        let Some(li) = self.target_layer() else { return };
        let was_dirty = self.dirty;
        self.begin_change();
        self.touch();
        let t = self.text_defaults.clone();
        let shape = Shape::TextArea {
            xf: Affine::translate(p.x, p.y),
            w: 1.0,
            h: 1.0,
            text: LOREM.into(),
            family: t.family,
            size: (t.size * 0.5).clamp(8.0, 24.0),
            bold: t.bold,
            italic: t.italic,
            fmt: t.fmt,
            frame: TextFrame::default(),
            rich: RichText::default(),
        };
        let id = self.insert_object(li, shape, self.text_style.clone());
        self.selection = vec![id];
        self.drag = Drag::AreaCreate {
            id,
            start: p,
            was_dirty,
        };
    }

    /// Farbindex für eine neue Ebene: die erste noch nicht verwendete Farbe.
    fn next_layer_color(&self) -> usize {
        let n = crate::model::LAYER_COLORS.len();
        let used: Vec<usize> = self.doc.layers.iter().map(|l| l.color % n).collect();
        (0..n)
            .find(|c| !used.contains(c))
            .unwrap_or(self.doc.layers.len())
    }

    /// Ausgewählte Objekte in eine andere Ebene verschieben (dort ganz oben).
    pub fn move_selection_to_layer(&mut self, target: u64) -> bool {
        let Some(ti) = self.doc.layer_index(target) else {
            return false;
        };
        if !self.doc.layers[ti].editable() {
            self.message = Some(trf(
                "Die Ebene „{}“ ist gesperrt oder ausgeblendet",
                &[&(self.doc.layers[ti].name)],
            ));
            return false;
        }
        let mut items: Vec<(usize, usize, u64)> = self
            .selection
            .iter()
            .filter_map(|id| self.doc.locate(*id).map(|(l, o)| (l, o, *id)))
            .filter(|(l, _, _)| *l != ti)
            .collect();
        if items.is_empty() {
            return false;
        }
        items.sort();
        self.checkpoint(None);
        let mut moved = Vec::new();
        for (_, _, id) in &items {
            if let Some(o) = self.doc.remove_object(*id) {
                moved.push(o);
            }
        }
        self.doc.layers[ti].objects.extend(moved);
        self.active_layer = target;
        self.layers_version += 1;
        true
    }

    /// Ebenen, auf denen ausgewählte Objekte liegen.
    pub fn selection_layers(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self
            .selection
            .iter()
            .filter_map(|id| self.doc.layer_of(*id).map(|l| l.id))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    // ───────────── Text auf Pfad, Bilder, Zeichenfläche ─────────────

    fn textpath_press(&mut self, p: Pt, m: &dyn Metrics) {
        let Some(id) = self.hit(p, m) else {
            self.message =
                Some(tr("Klicke auf einen Pfad oder eine Form, um Text daran auszurichten").into());
            return;
        };
        let Some(obj) = self.doc.object(id) else { return };
        match &obj.shape {
            Shape::TextPath { .. } | Shape::Text { .. } => {
                self.text_click(p, m);
            }
            Shape::Image { .. } => {
                self.message = Some(tr("Text lässt sich nicht an einem Bild ausrichten").into());
            }
            shape => {
                let Some(Shape::Path { subpaths, .. }) = shape.to_path() else {
                    return;
                };
                let t = self.text_defaults.clone();
                let new_shape = Shape::TextPath {
                    path: subpaths,
                    text: tr("Text auf Pfad").into(),
                    family: t.family,
                    size: t.size,
                    bold: t.bold,
                    italic: t.italic,
                    offset: 0.0,
                    fmt: t.fmt,
                };
                let style = self.text_style.clone();
                self.checkpoint(None);
                if let Some(o) = self.doc.object_mut(id) {
                    o.shape = new_shape;
                    o.style = style;
                    o.angle = 0.0;
                }
                self.selection = vec![id];
                // Platzhaltertext markiert: Tippen ersetzt ihn sofort.
                self.begin_text_edit(id, None);
            }
        }
    }

    pub fn set_text_offset(&mut self, v: f64) -> bool {
        let ids: Vec<u64> = self
            .selected_objects()
            .into_iter()
            .filter(|o| matches!(o.shape, Shape::TextPath { .. }))
            .map(|o| o.id)
            .collect();
        if ids.is_empty() {
            return false;
        }
        self.checkpoint(Some("offset"));
        for id in ids {
            if let Some(Object {
                shape: Shape::TextPath { offset, .. },
                ..
            }) = self.doc.object_mut(id)
            {
                *offset = v;
            }
        }
        true
    }

    /// Bild einfügen (Originaldaten und Pixelgröße). Zu große Bilder werden auf die Seite verkleinert.
    pub fn insert_image(&mut self, data: Rc<Vec<u8>>, w: f64, h: f64, center: Option<Pt>) -> bool {
        if w <= 0.0 || h <= 0.0 {
            return false;
        }
        let Some(li) = self.target_layer() else {
            return false;
        };
        let s = (0.8 * self.doc.width / w).min(0.8 * self.doc.height / h).min(1.0);
        let c = center.unwrap_or(Pt::new(self.doc.width / 2.0, self.doc.height / 2.0));
        let xf = Affine::translate(c.x - w * s / 2.0, c.y - h * s / 2.0).then_before(&Affine::scale(s, s));
        self.checkpoint(None);
        let style = Style {
            fill: None,
            stroke: None,
            width: 1.0,
            opacity: 1.0,
            ..Style::default()
        };
        let id = self.insert_object(
            li,
            Shape::Image {
                xf,
                w,
                h,
                data,
                clip: None,
            },
            style,
        );
        self.selection = vec![id];
        self.tool = Tool::Select;
        true
    }

    /// Größe und Hintergrund der Zeichenfläche ändern.
    pub fn set_artboard(&mut self, w: f64, h: f64, background: Option<Color>) {
        if self.doc.width == w && self.doc.height == h && self.doc.background == background {
            return;
        }
        self.checkpoint(None);
        self.doc.width = w.max(1.0);
        self.doc.height = h.max(1.0);
        self.doc.background = background;
        self.needs_fit = true;
        self.layers_version += 1;
    }

    // ───────────── Ebenen ─────────────

    pub fn add_layer(&mut self) {
        self.checkpoint(None);
        let n = self.doc.layers.len() + 1;
        let mut name = trf("Ebene {}", &[&(n)]);
        let mut k = n;
        while self.doc.layers.iter().any(|l| l.name == name) {
            k += 1;
            name = trf("Ebene {}", &[&(k)]);
        }
        let id = self.doc.alloc_id();
        let pos = self
            .doc
            .layer_index(self.active_layer)
            .map(|i| i + 1)
            .unwrap_or(self.doc.layers.len());
        self.doc.layers.insert(
            pos,
            Layer {
                id,
                color: self.next_layer_color(),
                name,
                visible: true,
                locked: false,
                objects: Vec::new(),
            },
        );
        self.active_layer = id;
        self.layers_version += 1;
    }

    pub fn delete_layer(&mut self) -> bool {
        if self.doc.layers.len() <= 1 {
            self.message = Some(tr("Die letzte Ebene kann nicht gelöscht werden").into());
            return false;
        }
        let Some(i) = self.doc.layer_index(self.active_layer) else {
            return false;
        };
        self.checkpoint(None);
        let lid = self.doc.layers[i].id;
        self.doc.layers.remove(i);
        // Hilfslinien liegen auf der Ebene und verschwinden mit ihr
        self.doc.guides.retain(|g| g.layer != lid);
        self.guide_sel = None;
        self.active_layer = self.doc.layers[i.saturating_sub(1).min(self.doc.layers.len() - 1)].id;
        self.prune_selection();
        self.layers_version += 1;
        true
    }

    pub fn move_layer(&mut self, delta: i32) -> bool {
        let Some(i) = self.doc.layer_index(self.active_layer) else {
            return false;
        };
        let t = i as i64 + delta as i64;
        if t < 0 || t >= self.doc.layers.len() as i64 {
            return false;
        }
        self.checkpoint(None);
        self.doc.move_layer(self.active_layer, delta);
        self.layers_version += 1;
        true
    }

    /// Ebene an Platz `slot` der Ebenenliste setzen (0 = ganz oben, wie angezeigt;
    /// `slot` zählt die Plätze zwischen den Zeilen vor dem Herausnehmen).
    pub fn move_layer_to_slot(&mut self, id: u64, slot: usize) -> bool {
        let mut order: Vec<u64> = self.doc.layers.iter().rev().map(|l| l.id).collect();
        let Some(pos) = order.iter().position(|x| *x == id) else {
            return false;
        };
        let k = if slot > pos { slot - 1 } else { slot }.min(order.len() - 1);
        if k == pos {
            return false;
        }
        self.checkpoint(None);
        order.remove(pos);
        order.insert(k, id);
        let mut layers = std::mem::take(&mut self.doc.layers);
        let mut sorted = Vec::with_capacity(layers.len());
        for lid in order.iter().rev() {
            if let Some(i) = layers.iter().position(|l| l.id == *lid) {
                sorted.push(layers.remove(i));
            }
        }
        self.doc.layers = sorted;
        self.active_layer = id;
        self.layers_version += 1;
        true
    }

    pub fn set_layer_visible(&mut self, id: u64, v: bool) {
        if self.doc.layer(id).map(|l| l.visible) == Some(v) {
            return;
        }
        self.checkpoint(None);
        if let Some(l) = self.doc.layer_mut(id) {
            l.visible = v;
        }
        self.prune_selection();
    }

    pub fn set_layer_locked(&mut self, id: u64, v: bool) {
        if self.doc.layer(id).map(|l| l.locked) == Some(v) {
            return;
        }
        self.checkpoint(None);
        if let Some(l) = self.doc.layer_mut(id) {
            l.locked = v;
        }
        self.prune_selection();
    }

    pub fn rename_layer(&mut self, id: u64, name: &str) {
        let name = name.trim();
        if name.is_empty() || self.doc.layer(id).map(|l| l.name == name).unwrap_or(true) {
            return;
        }
        self.checkpoint(None);
        if let Some(l) = self.doc.layer_mut(id) {
            l.name = name.to_string();
        }
    }

    // ───────────── Kontextleiste ─────────────

    pub fn context_info(&self) -> ContextInfo {
        let sel = self.selected_objects();
        let first_text = sel.iter().find_map(|o| {
            o.shape.text_fields().map(|(text, family, size, bold, italic)| {
                (
                    text.to_string(),
                    TextDefaults {
                        family: family.to_string(),
                        size,
                        bold,
                        italic,
                        fmt: o.shape.text_format().unwrap_or_default(),
                    },
                )
            })
        });
        // Zeichenformat an Markierung/Cursor bzw. über das ganze Feld
        let view = self.text_view(&sel);
        let first_text = first_text.map(|(t, mut d)| {
            if let Some((cf, _, _)) = &view {
                d.family = cf.family.clone();
                d.size = cf.size;
                d.bold = cf.bold;
                d.italic = cf.italic;
                d.fmt.weight = cf.weight;
                d.fmt.underline = cf.underline;
                d.fmt.strike = cf.strike;
            }
            (t, d)
        });
        let text_offset = sel.iter().find_map(|o| match o.shape {
            Shape::TextPath { offset, .. } => Some(offset),
            _ => None,
        });
        let first_rect = sel.iter().find_map(|o| match o.shape {
            Shape::Rect { r, .. } => Some(r),
            _ => None,
        });
        let style = sel
            .first()
            .map(|o| o.style.clone())
            .unwrap_or_else(|| self.current_defaults().clone());
        let single_text = sel.len() == 1 && sel[0].shape.is_text();
        ContextInfo {
            show_shape: self.tool == Tool::Shape,
            poly: match sel.as_slice() {
                [o] => o.shape.regular_polygon().map(|r| (r.corners, r.inner.is_some())),
                _ => None,
            },
            show_radius: (self.tool == Tool::Shape && self.shape_kind == ShapeKind::Rect)
                || first_rect.is_some(),
            radius: first_rect.unwrap_or(self.corner_radius),
            show_text: (matches!(self.tool, Tool::Text | Tool::TextPath | Tool::TextArea) && sel.is_empty())
                || first_text.is_some(),
            text: if single_text {
                first_text.as_ref().map(|t| t.0.clone())
            } else {
                None
            },
            font: first_text
                .map(|t| t.1)
                .unwrap_or_else(|| self.text_defaults.clone()),
            font_mixed: view.as_ref().map(|v| v.1).unwrap_or_default(),
            para: view.as_ref().map(|v| v.2).unwrap_or(ParaFmt {
                list: self.text_defaults.fmt.list,
                indent: 0,
            }),
            show_node: self.tool == Tool::Node && sel.len() == 1,
            node_count: self.node_sel.len(),
            node_radius: self.current_fillet_radius(),
            text_offset,
            // Formen sind schon Pfade (Knoten lassen sich direkt bearbeiten); umwandeln
            // braucht man nur bei Text
            can_convert: sel.iter().any(|o| o.shape.is_text()),
            show_style: !matches!(
                self.tool,
                Tool::Guide | Tool::Eraser | Tool::Scissors | Tool::Builder
            ) || !sel.is_empty(),
            show_fill: sel.is_empty()
                || sel
                    .iter()
                    .any(|o| !matches!(o.shape, Shape::Image { .. } | Shape::Line { .. })),
            show_markers: sel.iter().any(|o| o.shape.is_open())
                || (sel.is_empty() && matches!(self.tool, Tool::Pen | Tool::Pencil | Tool::Curvature)),
            show_pencil: self.tool == Tool::Pencil,
            show_eraser: self.tool == Tool::Eraser,
            eraser_size: self.eraser_size,
            pencil_smooth: self.pencil_smooth,
            pencil_close: self.pencil_close,
            show_stroke: sel.is_empty()
                || sel
                    .iter()
                    .any(|o| !matches!(o.shape, Shape::Image { clip: None, .. })),
            empty_hint: self.tool == Tool::Select && sel.is_empty(),
            style,
            has_selection: !sel.is_empty(),
            can_boolean: sel.len() >= 2,
            show_valign: sel.iter().any(|o| matches!(o.shape, Shape::TextArea { .. }))
                || (self.tool == Tool::TextArea && sel.is_empty()),
            show_list: sel
                .iter()
                .any(|o| matches!(&o.shape, Shape::TextArea { frame, .. } if frame.path.is_none()))
                || (self.tool == Tool::TextArea && sel.is_empty()),
            angle: sel.first().map(|o| o.angle).unwrap_or(0.0),
            grad_stop: self.grad_stop,
            show_guides_panel: self.tool == Tool::Guide,
            guide: self.guide_sel.and_then(|i| self.doc.guides.get(i).copied()),
            guide_angle: self.guide_angle,
            text_frame: sel.iter().find_map(|o| match &o.shape {
                Shape::TextArea { frame, .. } => Some(frame.clone()),
                // Punkttext und Text auf Pfad: Kasten hinter dem Text
                Shape::Text { fmt, .. } | Shape::TextPath { fmt, .. } => Some(fmt.text_box()),
                _ => None,
            }),
            is_text: !sel.is_empty() && sel.iter().all(|o| o.shape.is_text()),
            image: match sel[..] {
                [o] => match &o.shape {
                    Shape::Image { clip, .. } => Some((clip.is_some(), self.image_part(o.id))),
                    _ => None,
                },
                _ => None,
            },
            title: match sel.len() {
                0 => match self.tool {
                    Tool::Select => tr("Auswahl").into(),
                    Tool::Node => tr("Knoten bearbeiten").into(),
                    Tool::Shape => tr("Neue Form").into(),
                    Tool::Pen => tr("Zeichenstift").into(),
                    Tool::Pencil => tr("Freihand").into(),
                    Tool::Text => tr("Neuer Text").into(),
                    Tool::TextPath => tr("Text auf Pfad").into(),
                    Tool::TextArea => tr("Neues Textfeld").into(),
                    Tool::Curvature => tr("Kurvenzeichner").into(),
                    Tool::Guide => tr("Hilfslinien").into(),
                    Tool::Eraser => tr("Radierer").into(),
                    Tool::Scissors => tr("Schere").into(),
                    Tool::Builder => tr("Formerstellung").into(),
                },
                1 => match &sel[0].shape {
                    Shape::Image { clip: Some(_), .. } => tr("Bild mit Rahmen").into(),
                    s => s.kind_name().into(),
                },
                n => trf("{} Objekte", &[&(n)]),
            },
        }
    }

    /// Stiländerung auf die Auswahl anwenden oder – ohne Auswahl – als Vorgabe merken.
    pub fn update_style(&mut self, key: &str, f: impl Fn(&mut Style)) -> bool {
        if self.selection.is_empty() {
            if matches!(self.tool, Tool::Text | Tool::TextPath | Tool::TextArea) {
                f(&mut self.text_style);
                self.text_style.fold_opacity();
            } else {
                f(&mut self.defaults);
                self.defaults.fold_opacity();
            }
            return false;
        }
        self.checkpoint(Some(key));
        for id in self.selection.clone() {
            if let Some(o) = self.doc.object_mut(id) {
                // Formen: eine verborgene Gesamtdeckkraft erst in die Farben übernehmen,
                // damit die angezeigte Deckkraft der Farbe auch wirklich gilt
                let image = matches!(o.shape, Shape::Image { .. });
                if !image {
                    o.style.fold_opacity();
                }
                f(&mut o.style);
                if !image {
                    o.style.fold_opacity();
                }
            }
        }
        // Gewählte Werte auch als Vorgabe für neue Objekte merken.
        if !matches!(self.tool, Tool::Text | Tool::TextPath | Tool::TextArea) {
            f(&mut self.defaults);
            self.defaults.fold_opacity();
        }
        true
    }

    /// Stil nur der ausgewählten Objekte ändern, die `pred` erfüllen. Die Vorgaben für neue
    /// Objekte bleiben unberührt (für Bildfilter und Effekte, die nicht jedes Objekt kennt).
    pub fn update_objects(
        &mut self,
        key: &str,
        pred: impl Fn(&Object) -> bool,
        f: impl Fn(&mut Style),
    ) -> bool {
        let ids: Vec<u64> = self
            .selection
            .iter()
            .copied()
            .filter(|id| self.doc.object(*id).is_some_and(&pred))
            .collect();
        if ids.is_empty() {
            return false;
        }
        self.checkpoint(Some(key));
        for id in ids {
            if let Some(o) = self.doc.object_mut(id) {
                f(&mut o.style);
            }
        }
        true
    }

    /// Ist unter den ausgewählten Objekten ein Bild bzw. ein Nicht-Bild? (für das Effektmenü)
    pub fn selection_image_kinds(&self) -> (bool, bool) {
        let objs = self.selected_objects();
        let img = objs
            .iter()
            .filter(|o| matches!(o.shape, Shape::Image { .. }))
            .count();
        (img > 0, img < objs.len())
    }

    /// Das einzige ausgewählte Bild ohne Rahmen, falls es nachgezeichnet werden kann.
    pub fn traceable_image(&self) -> Option<(u64, Rc<Vec<u8>>)> {
        let [id] = self.selection[..] else { return None };
        let o = self.doc.object(id)?;
        if o.locked {
            return None;
        }
        match &o.shape {
            Shape::Image { data, clip: None, .. } => Some((id, data.clone())),
            _ => None,
        }
    }

    /// Bild durch nachgezeichnete Vektorflächen ersetzen. `layers` (von unten nach oben) liegen in
    /// Pixeln eines `tw` × `th` großen Abbilds des Bildes. Die Flächen werden gruppiert.
    pub fn replace_with_traced(
        &mut self,
        id: u64,
        layers: Vec<crate::trace::Layer>,
        tw: usize,
        th: usize,
    ) -> bool {
        if layers.is_empty() || tw == 0 || th == 0 {
            self.message = Some(tr("Im Bild wurden keine Flächen gefunden").into());
            return false;
        }
        let Some(img) = self.doc.object(id).cloned() else {
            return false;
        };
        let Shape::Image { xf, w, h, .. } = &img.shape else {
            return false;
        };
        let Some((li, oi)) = self.doc.locate(id) else {
            return false;
        };
        let (sx, sy) = (*w / tw as f64, *h / th as f64);
        self.checkpoint(None);
        let gid = self.doc.max_group() + 1;
        self.doc.layers[li].objects.remove(oi);
        let mut ids = Vec::new();
        for (n, layer) in layers.iter().enumerate() {
            let map = |p: Pt| xf.apply(Pt::new(p.x * sx, p.y * sy));
            let subpaths: Vec<SubPath> = layer
                .paths
                .iter()
                .map(|sp| SubPath {
                    closed: sp.closed,
                    nodes: sp
                        .nodes
                        .iter()
                        .map(|nd| Node {
                            r: 0.0,
                            p: map(nd.p),
                            h_in: map(nd.h_in),
                            h_out: map(nd.h_out),
                        })
                        .collect(),
                })
                .collect();
            if subpaths.is_empty() {
                continue;
            }
            let mut st = img.style.clone();
            let [r, g, b] = layer.color;
            st.fill = Some(Color {
                r: r as f64 / 255.0,
                g: g as f64 / 255.0,
                b: b as f64 / 255.0,
                a: 1.0,
            });
            st.stroke = None;
            st.width = 0.0;
            st.gradient = None;
            st.fx = Default::default();
            st.shadow = None;
            st.shadow_gradient = None;
            let nid = self.doc.alloc_id();
            self.doc.layers[li].objects.insert(
                oi + ids.len(),
                Object {
                    id: nid,
                    shape: Shape::Path {
                        subpaths,
                        evenodd: true,
                    },
                    style: st,
                    angle: 0.0,
                    group: gid,
                    locked: false,
                    name: if n == 0 {
                        tr("Nachgezeichnet").into()
                    } else {
                        String::new()
                    },
                },
            );
            ids.push(nid);
        }
        self.selection = ids;
        self.node_sel.clear();
        self.layers_version += 1;
        true
    }

    // ───────────── Formerstellung ─────────────
    //
    // Alle Umrisse (der Auswahl, sonst alles Sichtbaren) bilden einen ebenen Graphen; seine
    // Flächen – auch Zwischenräume und von offenen Linien begrenzte – sind die Teilflächen.
    // Klick: die Fläche wird eine neue Form. Ziehen: die überstrichenen Flächen werden eine
    // Form, die beteiligten Formen zerfallen in ihre übrigen Flächen. Alt: Flächen löschen.

    /// Objekte, deren Umrisse die Flächen bilden: die Auswahl, sonst alles Sichtbare (leer).
    fn build_scope(&self) -> Vec<u64> {
        let mut ids = self.selection.clone();
        ids.sort_unstable();
        ids
    }

    /// Ebener Graph der Umrisse (zwischengespeichert, bis sich etwas ändert).
    fn build_arrangement(&self, m: &dyn Metrics) -> Rc<crate::shapeops::Arrangement> {
        let v = self.view;
        let key = (self.rev, v.zoom, v.ox, v.oy, self.viewport);
        let scope = self.build_scope();
        if let Some(c) = self.bucket.borrow().as_ref() {
            // Ansichtsänderungen spielen nur bei sehr vielen Kurven eine Rolle
            if c.key.0 == self.rev && c.ids == scope && (!c.culled || c.key == key) {
                return c.arr.clone();
            }
        }
        let mut curves: Vec<crate::shapeops::Cubic> = Vec::new();
        for l in self.doc.layers.iter().filter(|l| l.visible) {
            for o in &l.objects {
                if !scope.is_empty() && scope.binary_search(&o.id).is_err() {
                    continue;
                }
                let subs = match &o.shape {
                    Shape::Image { clip: Some(c), .. } => Some(crate::model::effective_subpaths(c)),
                    Shape::Image { .. } => o.shape.image_quad().map(|q| {
                        vec![SubPath {
                            nodes: q.iter().map(|p| Node::corner(*p)).collect(),
                            closed: true,
                        }]
                    }),
                    _ => Self::outline_of(o, m).map(|s| crate::model::effective_subpaths(&s)),
                };
                for sp in subs.unwrap_or_default() {
                    curves.extend(sp.segments());
                }
            }
        }
        // sehr viele Kurven: nur den sichtbaren Bereich (großzügig) betrachten
        let mut culled = false;
        if curves.len() > 4000 {
            let a = v.to_doc(Pt::new(0.0, 0.0));
            let b = v.to_doc(Pt::new(self.viewport.0, self.viewport.1));
            let r = Rect {
                x0: a.x.min(b.x),
                y0: a.y.min(b.y),
                x1: a.x.max(b.x),
                y1: a.y.max(b.y),
            };
            let r = r.inflate(r.width().max(r.height()) * 0.5);
            curves.retain(|c| {
                let bb = crate::geom::cubic_bbox(c.0, c.1, c.2, c.3);
                bb.intersects(&r)
            });
            culled = true;
        }
        let tol = (0.02 / v.zoom).clamp(0.001, 0.2);
        let arr = Rc::new(crate::shapeops::arrangement(&curves, tol));
        *self.bucket.borrow_mut() = Some(BucketCache {
            key,
            ids: scope,
            culled,
            arr: arr.clone(),
        });
        arr
    }

    /// Objekte, mit denen gearbeitet wird: sichtbar, nicht gesperrt, keine Bilder/Texte.
    fn editable_objects(&self) -> Vec<&Object> {
        self.doc
            .layers
            .iter()
            .filter(|l| l.visible && !l.locked)
            .flat_map(|l| l.objects.iter())
            .filter(|o| !o.locked)
            .filter(|o| !matches!(o.shape, Shape::Image { .. }) && !o.shape.is_text())
            .collect()
    }

    /// Flächen, die von den Punkten `pts` überstrichen werden, in `hits` aufnehmen.
    fn builder_touch(&self, pts: &[Pt], hits: &mut Vec<usize>, m: &dyn Metrics) {
        let arr = self.build_arrangement(m);
        for p in pts {
            if let Some(i) = arr.face_at(*p) {
                if !hits.contains(&i) {
                    hits.push(i);
                }
            }
        }
    }

    /// Hervorzuhebende Flächen: während des Ziehens die getroffenen, sonst die unter dem
    /// Zeiger. Zweiter Wert: wird gelöscht (Alt).
    pub fn builder_marks(&self) -> (Vec<Vec<boolean::Ring>>, bool) {
        if self.tool != Tool::Builder {
            return (Vec::new(), false);
        }
        let Some(arr) = self.bucket.borrow().as_ref().map(|c| c.arr.clone()) else {
            return (Vec::new(), false);
        };
        let valid = |i: &usize| *i < arr.faces.len();
        match &self.drag {
            Drag::Build { hits, alt, .. } => (
                hits.iter()
                    .filter(|i| valid(i))
                    .map(|i| arr.region_polys(*i))
                    .collect(),
                *alt,
            ),
            _ => (
                self.builder_hover
                    .borrow()
                    .filter(valid)
                    .map(|i| vec![arr.region_polys(i)])
                    .unwrap_or_default(),
                false,
            ),
        }
    }

    /// Alle Flächen (zum Anzeigen des Netzes) – nur mit Auswahl, sonst wäre es das ganze
    /// Dokument.
    pub fn builder_all(&self) -> Vec<Vec<boolean::Ring>> {
        if self.tool != Tool::Builder || self.selection.is_empty() {
            return Vec::new();
        }
        let Some(arr) = self.bucket.borrow().as_ref().map(|c| c.arr.clone()) else {
            return Vec::new();
        };
        arr.bounded_faces().map(|f| vec![f.poly.clone()]).collect()
    }

    /// Stil für Flächen, die in keiner Form liegen (Zwischenräume): aktuelle Füllung, ohne
    /// Kontur.
    fn gap_style(&self) -> Style {
        let mut style = self.defaults.clone();
        style.stroke = None;
        style.shadow = None;
        if style.fill.is_none() {
            style.fill = Some(Color::rgb(0.6, 0.757, 0.945));
        }
        style
    }

    /// Klick: aus der Fläche wird eine neue Form (die vorhandenen bleiben unverändert).
    fn builder_new_shape(
        &mut self,
        region: Vec<Vec<crate::shapeops::Cubic>>,
        p: Pt,
        m: &dyn Metrics,
    ) -> bool {
        let subs: Vec<SubPath> = region
            .iter()
            .map(|segs| crate::shapeops::segs_to_subpath(segs))
            .collect();
        if subs.is_empty() {
            return false;
        }
        // Über die oberste gefüllte Form legen, die den Punkt enthält; sonst ganz nach
        // unten in die aktuelle Ebene (unter die Linien).
        let tol = 0.5 / self.view.zoom;
        let above = self
            .doc
            .layers
            .iter()
            .enumerate()
            .filter(|(_, l)| l.visible && !l.locked)
            .flat_map(|(li, l)| l.objects.iter().enumerate().map(move |(oi, o)| (li, oi, o)))
            .filter(|(_, _, o)| o.style.fill.is_some() && m.hit(o, p, tol))
            .map(|(li, oi, _)| (li, oi))
            .last();
        let (li, pos) = match above {
            Some((li, oi)) => (li, oi + 1),
            None => match self.target_layer() {
                Some(li) => (li, 0),
                None => return false,
            },
        };
        let style = self.gap_style();
        self.checkpoint(None);
        let id = self.doc.alloc_id();
        self.doc.layers[li].objects.insert(
            pos,
            Object {
                group: 0,
                locked: false,
                name: String::new(),
                id,
                shape: Shape::Path {
                    subpaths: subs,
                    evenodd: true,
                },
                style,
                angle: 0.0,
            },
        );
        self.selection = vec![id];
        true
    }

    /// Loslassen: eine Fläche angeklickt → neue Form; mehrere überstrichen → vereinigen
    /// (Alt: löschen). Beteiligte Formen zerfallen in ihre übrigen Flächen.
    fn builder_apply(&mut self, hits: &[usize], alt: bool, m: &dyn Metrics) -> bool {
        let arr = self.build_arrangement(m);
        let hits: Vec<usize> = hits.iter().copied().filter(|i| *i < arr.faces.len()).collect();
        let Some(&first) = hits.first() else {
            self.message = Some(tr("Hier ist keine geschlossene Fläche").into());
            return false;
        };
        let probe = |i: usize| crate::shapeops::interior_point(&arr.faces[i].poly);
        if !alt && hits.len() == 1 {
            let Some(p) = probe(first) else { return false };
            return self.builder_new_shape(arr.region_of(first), p, m);
        }
        // gefüllte Formen im Bereich: (Stapelplatz, Id, Ringe, Stil)
        let scope = self.build_scope();
        let tol = (0.05 / self.view.zoom).clamp(0.005, 0.5);
        let mut shapes: Vec<((usize, usize), u64, Vec<boolean::Ring>, Style)> = self
            .editable_objects()
            .into_iter()
            .filter(|o| scope.is_empty() || scope.binary_search(&o.id).is_ok())
            .filter(|o| o.style.fill.is_some())
            .filter_map(|o| {
                let subs = crate::model::effective_subpaths(&Self::outline_of(o, m)?);
                if !subs.iter().all(|s| s.closed) {
                    return None;
                }
                Some((
                    self.doc.locate(o.id)?,
                    o.id,
                    boolean::to_rings(&subs, tol),
                    o.style.clone(),
                ))
            })
            .collect();
        shapes.sort_by_key(|x| x.0);
        // oberste Form, in der eine Fläche liegt
        let owner = |i: usize| -> Option<usize> {
            let q = probe(i)?;
            (0..shapes.len())
                .rev()
                .find(|&k| boolean::contains(&shapes[k].2, q))
        };
        let owners_of = |i: usize| -> Vec<usize> {
            match probe(i) {
                Some(q) => (0..shapes.len())
                    .filter(|&k| boolean::contains(&shapes[k].2, q))
                    .collect(),
                None => Vec::new(),
            }
        };
        // beteiligt: Formen, in denen eine überstrichene Fläche liegt
        let mut involved: Vec<usize> = hits.iter().flat_map(|&i| owners_of(i)).collect();
        involved.sort_unstable();
        involved.dedup();
        if involved.is_empty() && alt {
            return false;
        }
        // übrige Flächen der beteiligten Formen bleiben als eigene Formen
        let mut pieces: Vec<(Vec<SubPath>, Style)> = Vec::new();
        for i in 0..arr.faces.len() {
            if hits.contains(&i) || !arr.bounded(i) {
                continue;
            }
            let own = owners_of(i);
            let Some(&top) = own.iter().rev().find(|k| involved.contains(k)) else {
                continue;
            };
            let subs: Vec<SubPath> = arr
                .region_of(i)
                .iter()
                .map(|segs| crate::shapeops::segs_to_subpath(segs))
                .collect();
            pieces.push((subs, shapes[top].3.clone()));
        }
        let merged: Option<(Vec<SubPath>, Style)> = if alt {
            None
        } else {
            let regions: Vec<Vec<Vec<crate::shapeops::Cubic>>> =
                hits.iter().map(|&i| arr.region_of(i)).collect();
            let subs = crate::shapeops::union_regions(&regions);
            let style = owner(first)
                .map(|k| shapes[k].3.clone())
                .unwrap_or_else(|| self.gap_style());
            (!subs.is_empty()).then_some((subs, style))
        };
        // Einfügestelle: Platz der untersten beteiligten Form (sonst wie beim Klick)
        let (li, mut pos) = match involved.first().map(|&k| shapes[k].0) {
            Some(at) => at,
            None => match self.target_layer() {
                Some(li) => (li, 0),
                None => return false,
            },
        };
        let remove: Vec<u64> = involved.iter().map(|&k| shapes[k].1).collect();
        self.checkpoint(None);
        for id in &remove {
            self.doc.remove_object(*id);
        }
        pos = pos.min(self.doc.layers[li].objects.len());
        let add = |doc: &mut Document, subpaths: Vec<SubPath>, style: Style, pos: &mut usize| -> u64 {
            let id = doc.alloc_id();
            doc.layers[li].objects.insert(
                *pos,
                Object {
                    group: 0,
                    locked: false,
                    name: String::new(),
                    id,
                    shape: Shape::Path {
                        subpaths,
                        evenodd: true,
                    },
                    style,
                    angle: 0.0,
                },
            );
            *pos += 1;
            id
        };
        for (subs, style) in pieces {
            add(&mut self.doc, subs, style, &mut pos);
        }
        self.selection = match merged {
            Some((subs, style)) => vec![add(&mut self.doc, subs, style, &mut pos)],
            None => Vec::new(),
        };
        self.node_sel.clear();
        true
    }

    /// Radierer entlang der Spur `pts` anwenden.
    fn erase_along(&mut self, pts: &[Pt], m: &dyn Metrics) -> bool {
        if pts.is_empty() {
            return false;
        }
        let r = (self.eraser_size / 2.0).max(0.25);
        let line = crate::shapeops::resample(pts, (r * 0.25).max(0.25));
        let mut sb = Rect {
            x0: f64::MAX,
            y0: f64::MAX,
            x1: f64::MIN,
            y1: f64::MIN,
        };
        for p in &line {
            sb.x0 = sb.x0.min(p.x);
            sb.y0 = sb.y0.min(p.y);
            sb.x1 = sb.x1.max(p.x);
            sb.y1 = sb.y1.max(p.y);
        }
        let sb = sb.inflate(r);
        // mit Auswahl nur die ausgewählten Objekte, sonst alles unter der Spur
        let targets: Vec<u64> = self
            .editable_objects()
            .into_iter()
            .filter(|o| self.selection.is_empty() || self.selection.contains(&o.id))
            .filter(|o| {
                m.bbox(o)
                    .map(|b| b.inflate(o.style.width).intersects(&sb))
                    .unwrap_or(false)
            })
            .map(|o| o.id)
            .collect();
        if targets.is_empty() {
            return false;
        }
        let tol = (0.2 / self.view.zoom).clamp(0.05, 1.0);
        let mut region: Option<Vec<boolean::Ring>> = None;
        let inside = |p: Pt| crate::shapeops::dist_to_polyline(&line, p) <= r;
        // (Objekt, neue Teilpfade; leer = löschen)
        let mut changes: Vec<(u64, Vec<SubPath>)> = Vec::new();
        for id in targets {
            let Some(o) = self.doc.object(id) else { continue };
            let Some(subs) = Self::outline_of(o, m) else {
                continue;
            };
            let subs = crate::model::effective_subpaths(&subs);
            let filled = o.style.fill.is_some() && subs.iter().all(|s| s.closed);
            if filled {
                let region = region.get_or_insert_with(|| crate::shapeops::stroke_region(pts, r));
                let rings = boolean::to_rings(&subs, tol);
                let before: f64 = rings.iter().map(|x| boolean::area(x)).sum();
                let rest = boolean::rings_op(&rings, region, Op::Difference);
                let after: f64 = rest.iter().map(|x| boolean::area(x)).sum();
                if (before - after).abs() > 0.05 {
                    changes.push((id, boolean::rings_to_paths(rest)));
                }
            } else {
                let mut out = Vec::new();
                let mut changed = false;
                for sp in &subs {
                    let parts = crate::shapeops::erase_line(sp, &inside);
                    if parts.len() != 1 || parts[0] != *sp {
                        changed = true;
                    }
                    out.extend(parts);
                }
                if changed {
                    changes.push((id, out));
                }
            }
        }
        if changes.is_empty() {
            return false;
        }
        self.checkpoint(None);
        for (id, subs) in changes {
            if subs.is_empty() {
                self.doc.remove_object(id);
                self.selection.retain(|x| *x != id);
            } else if let Some(o) = self.doc.object_mut(id) {
                o.shape = Shape::Path {
                    subpaths: subs,
                    evenodd: false,
                };
            }
        }
        self.node_sel.clear();
        true
    }

    /// Schere: die gezogene Linie zerschneidet gefüllte Formen in Stücke (mit den echten
    /// Kurven) und trennt Linien an den Schnittpunkten auf. Jedes Stück wird ein eigenes
    /// Objekt. Mit Auswahl nur die ausgewählten Objekte, sonst alles unter der Linie.
    fn cut_along(&mut self, pts: &[Pt], m: &dyn Metrics) -> bool {
        if pts.len() < 2 {
            return false;
        }
        let z = self.view.zoom;
        let tol = (0.2 / z).clamp(0.01, 1.0);
        let mut line = crate::geom::simplify(pts, 0.6 / z);
        if line.len() < 2 {
            return false;
        }
        // Enden etwas verlängern, damit ein Schnitt bis genau an den Rand trotzdem durchgeht
        let ext = 3.0 / z;
        let n = line.len();
        let d0 = (line[0] - line[1]).normalized();
        let d1 = (line[n - 1] - line[n - 2]).normalized();
        line[0] = line[0] + d0 * ext;
        line[n - 1] = line[n - 1] + d1 * ext;
        let cutter: Vec<crate::shapeops::Cubic> = line
            .windows(2)
            .filter(|w| w[0].dist(w[1]) > 1e-9)
            .map(|w| crate::shapeops::line_cubic(w[0], w[1]))
            .collect();
        let lb = line.iter().fold(Rect::from_points(line[0], line[0]), |r, p| {
            r.union(&Rect::from_points(*p, *p))
        });
        let targets: Vec<u64> = self
            .editable_objects()
            .into_iter()
            .filter(|o| self.selection.is_empty() || self.selection.contains(&o.id))
            .filter(|o| m.bbox(o).map(|b| b.intersects(&lb)).unwrap_or(false))
            .map(|o| o.id)
            .collect();
        // (Objekt, Teile – je Teil die Teilpfade)
        let mut changes: Vec<(u64, Vec<Vec<SubPath>>)> = Vec::new();
        for id in targets {
            let Some(o) = self.doc.object(id) else { continue };
            let Some(subs) = Self::outline_of(o, m) else {
                continue;
            };
            let subs = crate::model::effective_subpaths(&subs);
            let filled = o.style.fill.is_some() && subs.iter().all(|s| s.closed);
            if filled {
                let mut curves: Vec<crate::shapeops::Cubic> =
                    subs.iter().flat_map(|sp| sp.segments()).collect();
                curves.extend(cutter.iter().copied());
                let arr = crate::shapeops::arrangement(&curves, tol);
                let rings = boolean::to_rings(&subs, tol);
                let mut parts: Vec<Vec<SubPath>> = Vec::new();
                for f in arr.bounded_faces() {
                    let Some(q) = crate::shapeops::interior_point(&f.poly) else {
                        continue;
                    };
                    if !boolean::contains(&rings, q) {
                        continue;
                    }
                    if let Some(region) = arr.region_at(q) {
                        parts.push(
                            region
                                .iter()
                                .map(|segs| crate::shapeops::segs_to_subpath(segs))
                                .collect(),
                        );
                    }
                }
                if parts.len() >= 2 {
                    changes.push((id, parts));
                }
            } else {
                let mut parts: Vec<Vec<SubPath>> = Vec::new();
                let mut cut = false;
                for sp in &subs {
                    match crate::shapeops::split_at_cuts(sp, &cutter, tol) {
                        Some(pieces) => {
                            cut = true;
                            parts.extend(pieces.into_iter().map(|p| vec![p]));
                        }
                        None => parts.push(vec![sp.clone()]),
                    }
                }
                if cut && parts.len() >= 2 {
                    changes.push((id, parts));
                }
            }
        }
        if changes.is_empty() {
            return false;
        }
        self.checkpoint(None);
        let mut selection = Vec::new();
        for (id, parts) in changes {
            let Some((li, oi)) = self.doc.locate(id) else {
                continue;
            };
            let orig = self.doc.layers[li].objects[oi].clone();
            let mut objs = Vec::new();
            for (k, subpaths) in parts.into_iter().enumerate() {
                let mut o = orig.clone();
                if k > 0 {
                    o.id = self.doc.alloc_id();
                }
                o.shape = Shape::Path {
                    subpaths,
                    evenodd: false,
                };
                selection.push(o.id);
                objs.push(o);
            }
            self.doc.layers[li].objects.splice(oi..oi + 1, objs);
        }
        self.selection = selection;
        self.node_sel.clear();
        true
    }

    /// Können die ausgewählten Pfade verbunden werden (mindestens ein offener Pfad)?
    pub fn can_join_paths(&self) -> bool {
        self.selected_objects().iter().any(|o| match &o.shape {
            Shape::Path { subpaths, .. } => subpaths.iter().any(|sp| !sp.closed && sp.nodes.len() >= 2),
            Shape::Line { .. } => true,
            _ => false,
        })
    }

    /// Ausgewählte offene Pfade zu einem Pfad verbinden: jeweils die nächstgelegenen Enden
    /// werden verbunden – liegen sie fast aufeinander, verschmelzen sie zu einem Punkt, sonst
    /// entsteht ein gerades Stück. Ein einzelner offener Pfad wird geschlossen.
    pub fn join_paths(&mut self) -> bool {
        if self.selection_locked() {
            self.message = Some(tr("Gesperrte Objekte lassen sich nicht verändern").into());
            return false;
        }
        // beteiligte Objekte in Stapelreihenfolge (das unterste bestimmt den Stil)
        let objs: Vec<Object> = self
            .selected_in_order()
            .into_iter()
            .filter(|o| matches!(o.shape, Shape::Path { .. } | Shape::Line { .. }))
            .collect();
        let mut opens: Vec<Vec<Node>> = Vec::new();
        let mut closed: Vec<SubPath> = Vec::new();
        for o in &objs {
            let subs = match &o.shape {
                Shape::Path { subpaths, .. } => subpaths.clone(),
                other => match other.to_path() {
                    Some(Shape::Path { subpaths, .. }) => subpaths,
                    _ => continue,
                },
            };
            for sp in subs {
                if sp.closed || sp.nodes.len() < 2 {
                    closed.push(sp);
                } else {
                    opens.push(sp.nodes);
                }
            }
        }
        if opens.is_empty() {
            return false;
        }
        let merge_tol = 4.0 / self.view.zoom;
        let single = opens.len() == 1;
        let reversed = |nodes: &mut Vec<Node>| {
            nodes.reverse();
            for n in nodes.iter_mut() {
                std::mem::swap(&mut n.h_in, &mut n.h_out);
            }
        };
        // Enden zweier Linienzüge aneinanderhängen (a endet, b beginnt)
        let attach = |a: &mut Vec<Node>, mut b: Vec<Node>| {
            let (last, first) = (*a.last().unwrap(), b[0]);
            if last.p.dist(first.p) <= merge_tol {
                // fast gleiche Punkte: einer in der Mitte, Anfasser mitverschieben
                let mid = last.p.lerp(first.p, 0.5);
                let node = Node {
                    p: mid,
                    h_in: last.h_in + (mid - last.p),
                    h_out: first.h_out + (mid - first.p),
                    r: last.r.max(first.r),
                };
                *a.last_mut().unwrap() = node;
                b.remove(0);
            }
            a.extend(b);
        };
        while opens.len() > 1 {
            // nächstgelegenes Paar von Enden verschiedener Linienzüge
            let mut best: Option<(f64, usize, bool, usize, bool)> = None;
            for i in 0..opens.len() {
                for j in 0..opens.len() {
                    if i == j {
                        continue;
                    }
                    for ei in [false, true] {
                        for sj in [false, true] {
                            // ei: Ende (true) bzw. Anfang von i; sj: Anfang (true) bzw. Ende von j
                            let a = if ei { opens[i].last() } else { opens[i].first() };
                            let b = if sj { opens[j].first() } else { opens[j].last() };
                            let d = a.unwrap().p.dist(b.unwrap().p);
                            if best.map(|x| d < x.0).unwrap_or(true) {
                                best = Some((d, i, ei, j, sj));
                            }
                        }
                    }
                }
            }
            let Some((_, i, ei, j, sj)) = best else { break };
            let mut b = opens.remove(j);
            let i = if j < i { i - 1 } else { i };
            if !ei {
                reversed(&mut opens[i]);
            }
            if !sj {
                reversed(&mut b);
            }
            attach(&mut opens[i], b);
        }
        let mut nodes = opens.pop().unwrap();
        // Enden fast aufeinander – oder nur ein Pfad gewählt: schließen
        let ends_meet = nodes.len() >= 3 && nodes[0].p.dist(nodes[nodes.len() - 1].p) <= merge_tol;
        let close = ends_meet || single;
        if ends_meet {
            let last = nodes.pop().unwrap();
            let mid = last.p.lerp(nodes[0].p, 0.5);
            nodes[0].h_out = nodes[0].h_out + (mid - nodes[0].p);
            nodes[0].h_in = last.h_in + (mid - last.p);
            nodes[0].p = mid;
        }
        if nodes.len() < 2 || (close && nodes.len() < 3) {
            return false;
        }
        let mut subpaths = vec![SubPath { nodes, closed: close }];
        subpaths.extend(closed);
        self.checkpoint(None);
        let keep = objs[0].id;
        for o in &objs[1..] {
            self.doc.remove_object(o.id);
        }
        if let Some(o) = self.doc.object_mut(keep) {
            o.shape = Shape::Path {
                subpaths,
                evenodd: false,
            };
        }
        self.selection = vec![keep];
        self.node_sel.clear();
        true
    }

    // ───────────── Bibliothek ─────────────

    /// Bibliothekseintrag wird über die Zeichenfläche gezogen: Ziel unter dem Zeiger merken.
    pub fn lib_drag_motion(&mut self, s: Pt, payload: LibPayload, m: &dyn Metrics) {
        let p = self.view.to_doc(s);
        self.pointer = Some(p);
        let target = self.hit(p, m).filter(|id| {
            self.doc
                .object(*id)
                .map(|o| restyle(&mut o.clone(), &payload))
                .unwrap_or(false)
        });
        self.lib_drop = match (target, self.lib_drop.take()) {
            (Some(t), Some((old, since, _))) if old == t => Some((t, since, payload)),
            (Some(t), _) => Some((t, Instant::now(), payload)),
            (None, _) => None,
        };
    }

    pub fn lib_drag_leave(&mut self) {
        self.lib_drop = None;
    }

    /// Ziel und Fortschritt der Wartezeit (0–1).
    pub fn lib_drag_progress(&self) -> Option<(u64, f64)> {
        let (t, since, _) = self.lib_drop.as_ref()?;
        Some((
            *t,
            (since.elapsed().as_secs_f64() / self.frame_dwell.max(0.05)).min(1.0),
        ))
    }

    /// Nach der Wartezeit: so sähe das Ziel mit dem gezogenen Stil aus.
    pub fn lib_drag_preview(&self) -> Option<Object> {
        let (t, p) = self.lib_drag_progress()?;
        if p < 1.0 {
            return None;
        }
        let (_, _, payload) = self.lib_drop.as_ref()?;
        let mut o = self.doc.object(t)?.clone();
        restyle(&mut o, payload).then_some(o)
    }

    /// Loslassen: Stil bzw. Farbe auf das Objekt unter dem Zeiger übertragen.
    pub fn lib_drop_at(&mut self, s: Pt, payload: &LibPayload, m: &dyn Metrics) -> bool {
        self.lib_drop = None;
        let p = self.view.to_doc(s);
        if let LibPayload::Graphic(objs) = payload {
            return self.insert_graphic(objs, p, m);
        }
        let Some(t) = self.hit(p, m) else {
            return false;
        };
        let ok = self
            .doc
            .object(t)
            .map(|o| restyle(&mut o.clone(), payload))
            .unwrap_or(false);
        if !ok {
            return false;
        }
        self.checkpoint(None);
        if let Some(o) = self.doc.object_mut(t) {
            restyle(o, payload);
        }
        self.refill_placeholders(m);
        self.selection = vec![t];
        if let Some(l) = self.doc.layer_of(t) {
            self.active_layer = l.id;
        }
        true
    }

    /// Grafik aus der Bibliothek einfügen: Kopien mit der Mitte bei `at`, mehrere Objekte
    /// werden zu einer Gruppe, vorhandene Gruppen bekommen neue Nummern.
    pub fn insert_graphic(&mut self, objs: &[Object], at: Pt, m: &dyn Metrics) -> bool {
        if objs.is_empty() {
            return false;
        }
        let Some(li) = self.target_layer() else {
            return false;
        };
        let Some(b) = objs.iter().filter_map(|o| m.bbox(o)).reduce(|a, b| a.union(&b)) else {
            return false;
        };
        let d = at - Pt::new((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
        self.checkpoint(None);
        let mut next = self.doc.max_group();
        let whole = if objs.len() > 1 && objs.iter().all(|o| o.group == 0) {
            next += 1;
            next
        } else {
            0
        };
        let mut regroup: Vec<(u64, u64)> = Vec::new();
        let mut ids = Vec::new();
        for o in objs {
            let mut c = o.clone();
            c.id = self.doc.alloc_id();
            c.locked = false;
            c.group = if whole != 0 {
                whole
            } else if c.group != 0 {
                match regroup.iter().find(|(old, _)| *old == c.group) {
                    Some((_, n)) => *n,
                    None => {
                        next += 1;
                        regroup.push((c.group, next));
                        next
                    }
                }
            } else {
                0
            };
            c.shape.translate(d);
            ids.push(c.id);
            self.doc.layers[li].objects.push(c);
        }
        self.selection = ids;
        self.layers_version += 1;
        true
    }

    /// Ausgewählte Objekte in Stapelreihenfolge (für „als Grafik in die Bibliothek“).
    pub fn selected_graphic(&self) -> Vec<Object> {
        self.selected_in_order()
    }

    /// Stil des ersten ausgewählten Objekts (für die Bibliothek): `text` = Textstil eines
    /// Textes, sonst Stil einer Form oder eines Pfades.
    pub fn selected_lib_style(&self, text: bool) -> Option<crate::library::LibStyle> {
        self.selected_objects()
            .into_iter()
            .find(|o| !matches!(o.shape, Shape::Image { .. }) && o.shape.is_text() == text)
            .map(crate::library::style_of)
    }

    /// Füll- bzw. Konturfarbe des ersten ausgewählten Objekts.
    pub fn selected_color(&self, stroke: bool) -> Option<Color> {
        self.selected_objects().into_iter().find_map(|o| {
            if stroke {
                o.style.stroke
            } else {
                o.style.fill.map(|f| {
                    o.style
                        .gradient
                        .as_ref()
                        .and_then(|g| g.stops.first())
                        .map(|s| s.color)
                        .unwrap_or(f)
                })
            }
        })
    }

    /// Farbe aus der Bibliothek als Füllung oder Kontur übernehmen.
    pub fn apply_lib_color(&mut self, c: Color, g: Option<Gradient>, stroke: bool) -> bool {
        let t = if stroke {
            PaintTarget::Stroke
        } else {
            PaintTarget::Fill
        };
        self.update_style("lib-color", move |st| st.set_paint(t, Some(c), g.clone()))
    }

    /// Gespeicherten Stil übernehmen: Darstellung komplett, bei Texten auch die Schrift.
    pub fn apply_lib_style(&mut self, s: &crate::library::LibStyle, m: &dyn Metrics) -> bool {
        // markierter Teil eines Textfeldes: nur dessen Schrift ändern
        if let (Some(t), Some((_, a, b))) = (&s.text, self.rich_range()) {
            if a < b {
                let c = CharFmt {
                    family: t.family.clone(),
                    size: t.size,
                    bold: t.bold,
                    weight: t.fmt.weight_for(t.bold),
                    italic: t.italic,
                    underline: t.fmt.underline,
                    strike: t.fmt.strike,
                };
                return self.set_char_fmt("lib-style", move |f| *f = c.clone());
            }
        }
        let ids: Vec<u64> = self
            .selected_objects()
            .into_iter()
            .filter(|o| !matches!(o.shape, Shape::Image { .. }) && (s.text.is_none() || o.shape.is_text()))
            .map(|o| o.id)
            .collect();
        if ids.is_empty() && !self.selection.is_empty() {
            return false;
        }
        if ids.is_empty() {
            // ohne Auswahl: Vorgabe für neue Objekte
            match &s.text {
                Some(t) => {
                    self.text_style = s.style.clone();
                    self.text_defaults.family = t.family.clone();
                    self.text_defaults.size = t.size;
                    self.text_defaults.bold = t.bold;
                    self.text_defaults.italic = t.italic;
                    self.text_defaults.fmt = t.fmt;
                }
                None => self.defaults = s.style.clone(),
            }
            return false;
        }
        self.checkpoint(None);
        let payload = LibPayload::Style(s.clone());
        for id in ids {
            if let Some(o) = self.doc.object_mut(id) {
                restyle(o, &payload);
            }
        }
        self.refill_placeholders(m);
        true
    }

    /// Anzahl der Ecken: Vorgabe für neue Vielecke/Sterne und bei ausgewählten Vielecken
    /// bzw. Sternen nachträglich (Lage, Größe, Drehung und Eckenradius bleiben).
    pub fn set_corners(&mut self, n: u32) -> bool {
        let n = n.clamp(3, 64);
        self.corners = n;
        let targets: Vec<(u64, crate::model::RegularPolygon)> = self
            .selected_objects()
            .into_iter()
            .filter_map(|o| o.shape.regular_polygon().map(|r| (o.id, r)))
            .filter(|(_, r)| r.corners != n)
            .collect();
        if targets.is_empty() {
            return false;
        }
        self.checkpoint(Some("corners"));
        for (id, rp) in targets {
            let mut shape = Shape::polygon_ratio(rp.center, rp.radius, rp.angle, n, rp.inner);
            if let Shape::Path { subpaths, .. } = &mut shape {
                for nd in subpaths.iter_mut().flat_map(|sp| sp.nodes.iter_mut()) {
                    nd.r = rp.fillet;
                }
            }
            if let Some(o) = self.doc.object_mut(id) {
                o.shape = shape;
            }
        }
        true
    }

    pub fn set_corner_radius(&mut self, r: f64) -> bool {
        let ids: Vec<u64> = self
            .selected_objects()
            .into_iter()
            .filter(|o| matches!(o.shape, Shape::Rect { .. }))
            .map(|o| o.id)
            .collect();
        if ids.is_empty() {
            return false;
        }
        self.checkpoint(Some("radius"));
        for id in ids {
            if let Some(Object {
                shape: Shape::Rect { r: rr, w, h, .. },
                ..
            }) = self.doc.object_mut(id)
            {
                // höchstens halbe kurze Seite (dann ist die Seite ganz rund)
                *rr = r.min(w.abs().min(h.abs()) / 2.0);
            }
        }
        true
    }

    /// Laufende Textbearbeitung in einem Textfeld: (Objekt, Markierung von, bis).
    pub fn rich_range(&self) -> Option<(u64, usize, usize)> {
        let (te, _) = self.edit_text()?;
        self.doc.object(te.id)?.shape.rich()?;
        let (a, b) = te.range();
        Some((te.id, a, b))
    }

    /// Zeichenformat ändern wie in einer Textverarbeitung: markierter Text nur dieser Teil,
    /// Cursor ohne Markierung gilt fürs Weiterschreiben, sonst die ganzen Objekte (dann
    /// werden abweichende Textteile für diese Angabe angeglichen).
    pub fn set_char_fmt(&mut self, key: &str, f: impl Fn(&mut CharFmt)) -> bool {
        if let Some((id, a, b)) = self.rich_range() {
            let Some(o) = self.doc.object(id) else {
                return false;
            };
            let (Some(base), Some(rich), Some(text)) =
                (o.shape.char_base(), o.shape.rich(), o.shape.text_fields())
            else {
                return false;
            };
            let len = text.0.len();
            if a == b && len > 0 {
                let cur = match &self.pending_fmt {
                    Some((pid, pc, pf)) if *pid == id && *pc == a => pf.clone(),
                    _ => rich.format_in(len, &base, a, a).0,
                };
                let mut nf = cur;
                nf.change(&f);
                self.pending_fmt = Some((id, a, nf));
                self.rev += 1;
                return true;
            }
            if a < b {
                self.checkpoint(Some(key));
                if let Some(r) = self.doc.object_mut(id).and_then(|o| o.shape.rich_mut()) {
                    r.apply_chars(len, &base, Some((a, b)), &f);
                }
                return true;
            }
        }
        // Vorgabe für neue Texte
        let td = &mut self.text_defaults;
        let mut d = CharFmt {
            family: td.family.clone(),
            size: td.size,
            bold: td.bold,
            weight: td.fmt.weight_for(td.bold),
            italic: td.italic,
            underline: td.fmt.underline,
            strike: td.fmt.strike,
        };
        d.change(&f);
        td.family = d.family;
        td.size = d.size;
        td.bold = d.bold;
        td.fmt.weight = if d.weight == crate::model::effective_weight(0, d.bold) {
            0
        } else {
            d.weight
        };
        td.italic = d.italic;
        td.fmt.underline = d.underline;
        td.fmt.strike = d.strike;
        let ids: Vec<u64> = self
            .selected_objects()
            .into_iter()
            .filter(|o| o.shape.is_text())
            .map(|o| o.id)
            .collect();
        if ids.is_empty() {
            return false;
        }
        self.checkpoint(Some(key));
        for id in ids {
            let Some(o) = self.doc.object_mut(id) else {
                continue;
            };
            let Some(base) = o.shape.char_base() else { continue };
            let len = o.shape.text_fields().map(|t| t.0.len()).unwrap_or(0);
            let nb = match o.shape.rich_mut() {
                Some(r) => r.apply_chars(len, &base, None, &f),
                None => {
                    let mut b = base.clone();
                    b.change(&f);
                    b
                }
            };
            o.shape.set_char_base(&nb);
        }
        true
    }

    /// Absatzformat (Aufzählung, Einzug) ändern: beim Bearbeiten die Absätze der Markierung
    /// bzw. des Cursors, sonst alle Absätze der gewählten Textfelder.
    pub fn set_para_fmt(&mut self, key: &str, f: impl Fn(&mut ParaFmt)) -> bool {
        let targets: Vec<(u64, Option<(usize, usize)>)> = match self.rich_range() {
            Some((id, a, b)) => vec![(id, Some((a, b)))],
            None => {
                let mut d = ParaFmt {
                    list: self.text_defaults.fmt.list,
                    indent: 0,
                };
                f(&mut d);
                self.text_defaults.fmt.list = d.list;
                self.selected_objects()
                    .into_iter()
                    .filter(|o| o.shape.rich().is_some())
                    .map(|o| (o.id, None))
                    .collect()
            }
        };
        if targets.is_empty() {
            return false;
        }
        self.checkpoint(Some(key));
        for (id, range) in targets {
            let Some(o) = self.doc.object_mut(id) else {
                continue;
            };
            let text = o.shape.text_fields().map(|t| t.0.to_string()).unwrap_or_default();
            let Some(fmt) = o.shape.text_format() else {
                continue;
            };
            if let Some(r) = o.shape.rich_mut() {
                r.apply_paras(&text, range, fmt.list, &f);
            }
            if range.is_none() {
                // Vorgabe des Feldes mitführen
                let mut d = ParaFmt {
                    list: fmt.list,
                    indent: 0,
                };
                f(&mut d);
                if let Some(fm) = o.shape.text_format_mut() {
                    fm.list = d.list;
                }
            }
        }
        true
    }

    /// Zeichen- und Absatzformat für die Seitenleiste: an der Markierung bzw. am Cursor,
    /// sonst über das ganze erste Textobjekt (uneinheitliche Angaben markiert).
    fn text_view(&self, sel: &[&Object]) -> Option<(CharFmt, FmtMixed, ParaFmt)> {
        let (o, a, b, editing) = match self.rich_range() {
            Some((id, a, b)) => (self.doc.object(id)?, a, b, true),
            None => {
                let o = sel.iter().find(|o| o.shape.is_text())?;
                let len = o.shape.text_fields()?.0.len();
                (*o, 0, len, false)
            }
        };
        let base = o.shape.char_base()?;
        let fmt = o.shape.text_format()?;
        let (text, ..) = o.shape.text_fields()?;
        let Some(rich) = o.shape.rich() else {
            return Some((
                base,
                FmtMixed::default(),
                ParaFmt {
                    list: fmt.list,
                    indent: 0,
                },
            ));
        };
        let (mut cf, mixed) = rich.format_in(text.len(), &base, a, b);
        if editing && a == b {
            if let Some((pid, pc, pf)) = &self.pending_fmt {
                if *pid == o.id && *pc == a {
                    cf = pf.clone();
                }
            }
        }
        let pi = text[..a.min(text.len())].matches('\n').count();
        Some((cf, mixed, rich.para(pi, fmt.list)))
    }

    /// Textattribute auf ausgewählte Texte anwenden bzw. als Vorgabe merken.
    pub fn update_text(&mut self, key: &str, f: impl Fn(&mut TextDefaults)) -> bool {
        f(&mut self.text_defaults);
        let ids: Vec<u64> = self
            .selected_objects()
            .into_iter()
            .filter(|o| o.shape.is_text())
            .map(|o| o.id)
            .collect();
        if ids.is_empty() {
            return false;
        }
        self.checkpoint(Some(key));
        for id in ids {
            if let Some((_, family, size, bold, italic)) =
                self.doc.object_mut(id).and_then(|o| o.shape.text_fields_mut())
            {
                let mut t = TextDefaults {
                    family: family.clone(),
                    size: *size,
                    bold: *bold,
                    italic: *italic,
                    fmt: TextFormat::default(),
                };
                f(&mut t);
                *family = t.family;
                *size = t.size;
                *bold = t.bold;
                *italic = t.italic;
            }
            // Absatzformat getrennt: Ausgangswert ist das Format des Objekts selbst.
            if let Some(fm) = self.doc.object_mut(id).and_then(|o| o.shape.text_format_mut()) {
                let mut t = TextDefaults {
                    family: String::new(),
                    size: 0.0,
                    bold: false,
                    italic: false,
                    fmt: *fm,
                };
                f(&mut t);
                *fm = t.fmt;
            }
        }
        true
    }

    pub fn set_text(&mut self, new_text: &str) -> bool {
        if let Some(&id) = self.selection.first() {
            self.placeholders.retain(|x| *x != id);
        }
        if self.selection.len() != 1 {
            return false;
        }
        let id = self.selection[0];
        let same = match self.doc.object(id).and_then(|o| o.shape.text_fields()) {
            Some((text, ..)) => text == new_text,
            None => return false,
        };
        if same {
            return false;
        }
        self.checkpoint(Some("text"));
        if let Some(o) = self.doc.object_mut(id) {
            o.shape.replace_text(new_text.to_string());
        }
        true
    }
}

/// Abstand der Rundungsmitte von der Ecke je Radius-Einheit (entlang der Winkelhalbierenden).
fn fillet_factor(theta: f64) -> f64 {
    (1.0 / (theta / 2.0).sin().max(1e-6) - 1.0).max(1e-3)
}

/// Skalierungsmatrix für das Ziehen an Anfasser `handle` (0 = oben links, im Uhrzeigersinn).
pub fn scale_affine(handle: usize, b: &Rect, p: Pt, mods: Mods, uniform: bool) -> Affine {
    let left = matches!(handle, 0 | 6 | 7);
    let right = matches!(handle, 2 | 3 | 4);
    let top = matches!(handle, 0 | 1 | 2);
    let bottom = matches!(handle, 4 | 5 | 6);
    let (w, h) = (b.width(), b.height());
    let mut sx = 1.0;
    let mut sy = 1.0;
    if w > 1e-9 {
        if left {
            sx = (b.x1 - p.x) / w;
        }
        if right {
            sx = (p.x - b.x0) / w;
        }
    }
    if h > 1e-9 {
        if top {
            sy = (b.y1 - p.y) / h;
        }
        if bottom {
            sy = (p.y - b.y0) / h;
        }
    }
    let corner = (left || right) && (top || bottom);
    let horizontal_edge = (left || right) && !(top || bottom);
    let vertical_edge = (top || bottom) && !(left || right);
    if (mods.shift || uniform) && corner {
        let s = sx.abs().max(sy.abs());
        sx = s.copysign(sx);
        sy = s.copysign(sy);
    } else if uniform && horizontal_edge {
        sy = sx.abs();
    } else if uniform && vertical_edge {
        sx = sy.abs();
    }
    if sx.abs() < 1e-3 {
        sx = 1e-3f64.copysign(sx);
    }
    if sy.abs() < 1e-3 {
        sy = 1e-3f64.copysign(sy);
    }
    let center = Pt::new((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
    let anchor = if mods.alt {
        center
    } else {
        Pt::new(
            if left {
                b.x1
            } else if right {
                b.x0
            } else {
                center.x
            },
            if top {
                b.y1
            } else if bottom {
                b.y0
            } else {
                center.y
            },
        )
    };
    Affine::scale_about(sx, sy, anchor)
}

// ───────────── Hilfslinien, Einrasten, Farbverläufe ─────────────

/// Einrasten auf gleiche Abstände entlang einer Achse (`horizontal`: x-Achse, sonst y).
///
/// `b` ist der Rahmen der verschobenen Auswahl, `boxes` die Rahmen der anderen Objekte.
/// Nur Objekte, die `b` auf der anderen Achse überlappen („in derselben Reihe“), zählen.
/// Gesucht wird die kleinste Korrektur (höchstens `tol`), nach der
/// * die Lücke zum linken und zum rechten Nachbarn gleich groß ist, oder
/// * die Lücke zu einem Nachbarn so groß ist wie eine Lücke zwischen zwei anderen Objekten
///   der Reihe (eine gleichmäßige Reihe lässt sich so fortsetzen).
/// Zurück kommen die Korrektur und die Lücken zum Anzeigen als (Anfang, Ende, Lage auf der
/// anderen Achse).
fn equal_gap_snap(
    b: &Rect,
    boxes: &[Rect],
    horizontal: bool,
    tol: f64,
) -> Option<(f64, Vec<(f64, f64, f64)>)> {
    let ax = |r: &Rect| if horizontal { (r.x0, r.x1) } else { (r.y0, r.y1) };
    let cross = |r: &Rect| if horizontal { (r.y0, r.y1) } else { (r.x0, r.x1) };
    let (b0, b1) = ax(b);
    let (c0, c1) = cross(b);
    let mut row: Vec<&Rect> = boxes
        .iter()
        .filter(|r| {
            let (q0, q1) = cross(r);
            q0 < c1 && q1 > c0
        })
        .collect();
    if row.is_empty() {
        return None;
    }
    row.sort_by(|p, q| ax(p).0.total_cmp(&ax(q).0));
    // Lage der Anzeigelinie: Mitte der Überlappung zweier Rahmen auf der anderen Achse
    let at = |p: &Rect, q: &Rect| {
        let ((p0, p1), (q0, q1)) = (cross(p), cross(q));
        (p0.max(q0) + p1.min(q1)) / 2.0
    };
    // vorhandene Lücken zwischen aufeinanderfolgenden Objekten der Reihe
    let mut refs: Vec<(f64, f64, f64)> = Vec::new();
    for w in row.windows(2) {
        let (e, s) = (ax(w[0]).1, ax(w[1]).0);
        if s - e > 1e-6 {
            refs.push((e, s, at(w[0], w[1])));
        }
    }
    let left = row
        .iter()
        .filter(|r| ax(r).1 <= b0 + tol)
        .max_by(|p, q| ax(p).1.total_cmp(&ax(q).1))
        .copied();
    let right = row
        .iter()
        .filter(|r| ax(r).0 >= b1 - tol)
        .min_by(|p, q| ax(p).0.total_cmp(&ax(q).0))
        .copied();
    let mut best: Option<(f64, Vec<(f64, f64, f64)>)> = None;
    let mut offer = |corr: f64, hints: Vec<(f64, f64, f64)>| {
        if corr.abs() <= tol && best.as_ref().map(|x| corr.abs() < x.0.abs()).unwrap_or(true) {
            best = Some((corr, hints));
        }
    };
    // links und rechts gleich groß
    if let (Some(l), Some(r)) = (left, right) {
        let (gl, gr) = (b0 - ax(l).1, ax(r).0 - b1);
        if gl >= -tol && gr >= -tol {
            let corr = (gr - gl) / 2.0;
            offer(
                corr,
                vec![(ax(l).1, b0 + corr, at(l, b)), (b1 + corr, ax(r).0, at(b, r))],
            );
        }
    }
    // Lücke wie eine vorhandene Lücke der Reihe
    for &(rs, re, rat) in &refs {
        let g = re - rs;
        if let Some(l) = left {
            let corr = ax(l).1 + g - b0;
            let free = right.map(|r| b1 + corr <= ax(r).0 + 1e-6).unwrap_or(true);
            if free {
                offer(corr, vec![(ax(l).1, ax(l).1 + g, at(l, b)), (rs, re, rat)]);
            }
        }
        if let Some(r) = right {
            let corr = ax(r).0 - g - b1;
            let free = left.map(|l| b0 + corr >= ax(l).1 - 1e-6).unwrap_or(true);
            if free {
                offer(corr, vec![(ax(r).0 - g, ax(r).0, at(b, r)), (rs, re, rat)]);
            }
        }
    }
    best
}

impl Editor {
    /// Fangabstand für Hilfslinien in Dokumenteinheiten.
    fn snap_tol(&self) -> f64 {
        6.0 / self.view.zoom
    }

    /// Einrastziele sammeln: Ecken, Mitten und Ankerpunkte anderer Objekte, ihre Konturen
    /// sowie Ecken und Mitte der Seite.
    fn build_snap_ctx(&self, m: &dyn Metrics, skip_selection: bool) -> SnapCtx {
        self.build_snap_ctx_ex(
            m,
            &|o: &Object| skip_selection && self.selection.contains(&o.id),
            None,
        )
    }

    /// Wie `build_snap_ctx`, aber mit eigener Auswahl der übergangenen Objekte und ohne die
    /// Zeichenfläche `skip_board` (beim Verschieben oder Ändern einer Zeichenfläche).
    fn build_snap_ctx_ex(
        &self,
        m: &dyn Metrics,
        skip: &dyn Fn(&Object) -> bool,
        skip_board: Option<usize>,
    ) -> SnapCtx {
        let mut c = SnapCtx::default();
        let add_box =
            |c: &mut SnapCtx, b: Rect, corner: &'static str, mid: &'static str, corners: bool, mids: bool| {
                let cx = (b.x0 + b.x1) / 2.0;
                let cy = (b.y0 + b.y1) / 2.0;
                if corners {
                    for (x, y) in [(b.x0, b.y0), (b.x1, b.y0), (b.x1, b.y1), (b.x0, b.y1)] {
                        c.points.push((Pt::new(x, y), corner));
                    }
                    for x in [b.x0, b.x1] {
                        c.xs.push((x, Pt::new(x, cy)));
                    }
                    for y in [b.y0, b.y1] {
                        c.ys.push((y, Pt::new(cx, y)));
                    }
                }
                if mids {
                    c.points.push((Pt::new(cx, cy), mid));
                    for (x, y) in [(cx, b.y0), (b.x1, cy), (cx, b.y1), (b.x0, cy)] {
                        c.points.push((Pt::new(x, y), tr("Kantenmitte")));
                    }
                    c.xs.push((cx, Pt::new(cx, cy)));
                    c.ys.push((cy, Pt::new(cx, cy)));
                }
            };
        let mut count = 0;
        for l in self.doc.layers.iter().filter(|l| l.visible) {
            for o in &l.objects {
                if skip(o) {
                    continue;
                }
                count += 1;
                let Some(b) = m.bbox(o) else { continue };
                add_box(
                    &mut c,
                    b,
                    tr("Ecke"),
                    tr("Mitte"),
                    self.snap_corners,
                    self.snap_mids,
                );
                if c.boxes.len() < 600 {
                    c.boxes.push(b);
                }
                if let (true, Shape::Path { subpaths, .. }) = (self.snap_anchors, &o.shape) {
                    for sp in subpaths {
                        for n in &sp.nodes {
                            c.points.push((n.p, tr("Ankerpunkt")));
                        }
                    }
                }
                // Konturen für „am Pfad“ (bei sehr vielen Objekten nicht)
                if self.snap_paths && count < 300 && !o.shape.is_text() {
                    let path = match &o.shape {
                        Shape::Image { clip: Some(cl), .. } => Some(crate::model::effective_subpaths(cl)),
                        Shape::Image { .. } => None,
                        s => match s.to_path() {
                            Some(Shape::Path { subpaths, .. }) => Some(subpaths),
                            _ => None,
                        },
                    };
                    for sp in path.unwrap_or_default() {
                        let mut v = Vec::new();
                        if let Some(f) = sp.nodes.first() {
                            v.push(f.p);
                        }
                        for (p0, c1, c2, p3) in sp.segments() {
                            for k in 1..=16 {
                                v.push(crate::geom::cubic_point(p0, c1, c2, p3, k as f64 / 16.0));
                            }
                        }
                        c.paths.push(v);
                    }
                }
            }
        }
        if self.snap_page {
            for (i, (_, r)) in self.doc.artboard_rects().into_iter().enumerate() {
                if skip_board == Some(i) {
                    continue;
                }
                add_box(&mut c, r, tr("Seitenecke"), tr("Seitenmitte"), true, true);
                // andere Zeichenflächen zählen auch für gleiche Abstände
                if skip_board.is_some() {
                    c.boxes.push(r);
                }
            }
        }
        c
    }

    fn snap_guides_point(&self, p: Pt) -> Option<Pt> {
        if !self.snap_guides || !self.show_guides || self.doc.guides.is_empty() {
            return None;
        }
        // Hilfslinien fangen etwas weiter als andere Ziele
        let tol = GUIDE_SNAP / self.view.zoom;
        let mut near: Vec<(f64, Guide)> = self
            .doc
            .guides
            .iter()
            .filter(|g| self.doc.guide_visible(g))
            .map(|g| (g.dist(p), *g))
            .filter(|(d, _)| *d <= tol)
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        if near.len() >= 2 {
            for i in 1..near.len() {
                if let Some(x) = near[0].1.intersect(&near[i].1) {
                    if x.dist(p) <= tol * 1.5 {
                        return Some(x);
                    }
                }
            }
        }
        near.first().map(|(_, g)| g.project(p))
    }

    /// Punkt einrasten: Hilfslinien, dann markante Punkte, Pfade, Ausrichtung an
    /// Kanten/Mitten anderer Objekte und schließlich das Raster.
    pub fn snap(&self, p: Pt) -> Pt {
        self.snap_in(p, self.snap_ctx.as_ref())
    }

    fn snap_in(&self, p: Pt, ctx: Option<&SnapCtx>) -> Pt {
        self.snap_with(p, ctx, true)
    }

    /// Wie `snap_in`; `guides` = false: nicht an Hilfslinien (beim Verschieben einer
    /// Hilfslinie, die sonst an sich selbst klebte).
    fn snap_with(&self, p: Pt, ctx: Option<&SnapCtx>, guides: bool) -> Pt {
        let mut hints = self.hints.borrow_mut();
        hints.clear();
        if let Some(q) = guides.then(|| self.snap_guides_point(p)).flatten() {
            hints.push(SnapHint {
                a: q,
                b: q,
                label: Some(tr("Hilfslinie")),
            });
            return q;
        }
        let tol = self.snap_tol();
        if let (true, Some(c)) = (self.smart_snap, ctx) {
            // markante Punkte
            if let Some((q, label)) = c
                .points
                .iter()
                .map(|(q, l)| (*q, *l, q.dist(p)))
                .filter(|x| x.2 <= tol)
                .min_by(|a, b| a.2.total_cmp(&b.2))
                .map(|(q, l, _)| (q, l))
            {
                hints.push(SnapHint {
                    a: q,
                    b: q,
                    label: Some(label),
                });
                return q;
            }
            // auf einer Kontur
            let mut best: Option<(f64, Pt)> = None;
            for poly in &c.paths {
                for w in poly.windows(2) {
                    let (a, b) = (w[0], w[1]);
                    let ab = b - a;
                    let l2 = ab.x * ab.x + ab.y * ab.y;
                    if l2 < 1e-12 {
                        continue;
                    }
                    let t = (((p - a).x * ab.x + (p - a).y * ab.y) / l2).clamp(0.0, 1.0);
                    let q = a.lerp(b, t);
                    let d = q.dist(p);
                    if d <= tol * 0.7 && best.map(|x| d < x.0).unwrap_or(true) {
                        best = Some((d, q));
                    }
                }
            }
            if let Some((_, q)) = best {
                hints.push(SnapHint {
                    a: q,
                    b: q,
                    label: Some(tr("Pfad")),
                });
                return q;
            }
            // Ausrichtung an Kanten und Mitten (waagerecht/senkrecht)
            let bx =
                c.xs.iter()
                    .filter(|(x, _)| (x - p.x).abs() <= tol)
                    .min_by(|a, b| (a.0 - p.x).abs().total_cmp(&(b.0 - p.x).abs()))
                    .copied();
            let by =
                c.ys.iter()
                    .filter(|(y, _)| (y - p.y).abs() <= tol)
                    .min_by(|a, b| (a.0 - p.y).abs().total_cmp(&(b.0 - p.y).abs()))
                    .copied();
            if bx.is_some() || by.is_some() {
                let q = Pt::new(bx.map(|x| x.0).unwrap_or(p.x), by.map(|y| y.0).unwrap_or(p.y));
                if let Some((_, r)) = bx {
                    hints.push(SnapHint {
                        a: r,
                        b: q,
                        label: None,
                    });
                }
                if let Some((_, r)) = by {
                    hints.push(SnapHint {
                        a: r,
                        b: q,
                        label: None,
                    });
                }
                return q;
            }
        }
        if self.grid_snap && self.grid_size > 0.0 {
            let g = self.grid_size;
            return Pt::new((p.x / g).round() * g, (p.y / g).round() * g);
        }
        p
    }

    /// Mit Umschalt: neuer Punkt nur in 45°-Schritten oder rechtwinklig zum letzten Abschnitt.
    fn constrain_from(&self, nodes: &[Node], p: Pt) -> Pt {
        let Some(last) = nodes.last().map(|n| n.p) else {
            return p;
        };
        let v = p - last;
        if v.len() < 1e-9 {
            return p;
        }
        let mut dirs: Vec<f64> = (0..8).map(|k| k as f64 * std::f64::consts::FRAC_PI_4).collect();
        if nodes.len() >= 2 {
            let prev = nodes[nodes.len() - 2].p;
            let d = last - prev;
            if d.len() > 1e-9 {
                let a = d.y.atan2(d.x);
                for k in 0..4 {
                    dirs.push(a + k as f64 * std::f64::consts::FRAC_PI_2);
                }
            }
        }
        let cur = v.y.atan2(v.x);
        let diff = |a: f64| {
            let mut d = (a - cur) % std::f64::consts::TAU;
            if d > std::f64::consts::PI {
                d -= std::f64::consts::TAU;
            } else if d < -std::f64::consts::PI {
                d += std::f64::consts::TAU;
            }
            d.abs()
        };
        let best = dirs
            .into_iter()
            .min_by(|a, b| diff(*a).total_cmp(&diff(*b)))
            .unwrap_or(cur);
        let dir = Pt::new(best.cos(), best.sin());
        let q = last + dir * (v.x * dir.x + v.y * dir.y);
        self.hints.borrow_mut().push(SnapHint {
            a: last,
            b: q,
            label: None,
        });
        q
    }

    /// Verschiebung so anpassen, dass Kanten oder Mitte der Auswahl an Hilfslinien,
    /// anderen Objekten, der Seite oder am Raster einrasten.
    /// Ankerpunkte der verschobenen Objekte (für das Einrasten von Pfaden).
    fn anchor_points(orig: &[(u64, Object)]) -> Vec<Pt> {
        let mut v = Vec::new();
        for (_, o) in orig {
            if let Shape::Path { subpaths, .. } = &o.shape {
                for sp in subpaths {
                    v.extend(sp.nodes.iter().map(|n| n.p));
                }
            } else if let Some(Shape::Path { subpaths, .. }) = o.shape.to_path() {
                for sp in subpaths {
                    v.extend(sp.nodes.iter().map(|n| n.p));
                }
            }
            if v.len() > 400 {
                v.truncate(400);
                break;
            }
        }
        v
    }

    /// Verschiebung so anpassen, dass Kanten, Mitte oder Ankerpunkte der Auswahl an
    /// Hilfslinien (auch schrägen), anderen Objekten, der Seite oder am Raster einrasten.
    /// Hilfslinien haben Vorrang und halten etwas stärker fest; an einer waagerechten
    /// oder senkrechten Hilfslinie kann die Auswahl entlanggleiten und dabei weiter an
    /// anderen Objekten einrasten.
    fn snap_move(&self, b: &Rect, d: Pt, lock_axis: bool, anchors: &[Pt]) -> Pt {
        let mut hints = self.hints.borrow_mut();
        hints.clear();
        let tol = self.snap_tol();
        let gtol = GUIDE_SNAP / self.view.zoom;
        let cx = (b.x0 + b.x1) / 2.0;
        let cy = (b.y0 + b.y1) / 2.0;
        // Bezugspunkte der Auswahl (vor der Verschiebung): Rahmen, Mitte, Ankerpunkte
        let mut pts: Vec<Pt> = vec![
            Pt::new(b.x0, b.y0),
            Pt::new(cx, b.y0),
            Pt::new(b.x1, b.y0),
            Pt::new(b.x0, cy),
            Pt::new(cx, cy),
            Pt::new(b.x1, cy),
            Pt::new(b.x0, b.y1),
            Pt::new(cx, b.y1),
            Pt::new(b.x1, b.y1),
        ];
        pts.extend_from_slice(anchors);
        let mut out = d;
        let mut fix_x = false;
        let mut fix_y = false;

        // 1. Hilfslinien: nächster Bezugspunkt senkrecht auf die Linie
        if self.snap_guides && self.show_guides {
            let mut bestg: Option<(f64, Pt, usize, Guide)> = None;
            for g in self.doc.guides.iter().filter(|g| self.doc.guide_visible(g)) {
                // Mit Umschalt (nur eine Achse) nur passende Linien
                if lock_axis && !(g.is_vertical() && d.y == 0.0 || g.is_horizontal() && d.x == 0.0) {
                    continue;
                }
                for (i, q) in pts.iter().enumerate() {
                    let q = *q + d;
                    let dist = g.dist(q);
                    if dist <= gtol && bestg.map(|b| dist < b.0).unwrap_or(true) {
                        bestg = Some((dist, g.project(q) - q, i, *g));
                    }
                }
            }
            if let Some((_, corr, i, g)) = bestg {
                out = out + corr;
                if g.is_vertical() {
                    fix_x = true;
                } else if g.is_horizontal() {
                    fix_y = true;
                } else {
                    fix_x = true;
                    fix_y = true;
                }
                // zweite Hilfslinie im Schnittpunkt?
                let q = pts[i] + out;
                for g2 in self.doc.guides.iter().filter(|g| self.doc.guide_visible(g)) {
                    if (g2.angle - g.angle).abs() < 1e-6 || g2.dist(q) > gtol {
                        continue;
                    }
                    if let Some(x) = g.intersect(g2) {
                        if x.dist(q) <= gtol * 1.5 {
                            out = out + (x - q);
                            fix_x = true;
                            fix_y = true;
                            break;
                        }
                    }
                }
                let q = pts[i] + out;
                hints.push(SnapHint {
                    a: q,
                    b: q,
                    label: Some(tr("Hilfslinie")),
                });
            }
        }

        // 2. Intelligente Hilfslinien: Kanten und Mitten anderer Objekte und der Seite
        if let (true, Some(c)) = (self.smart_snap, self.snap_ctx.as_ref()) {
            let best = |cands: &[(f64, Pt)], vals: &[f64]| {
                let mut r: Option<(f64, f64, Pt, usize)> = None;
                for (v, refp) in cands {
                    for (i, c) in vals.iter().enumerate() {
                        let corr = v - c;
                        if corr.abs() <= tol && r.map(|o| corr.abs() < o.0.abs()).unwrap_or(true) {
                            r = Some((corr, *v, *refp, i));
                        }
                    }
                }
                r
            };
            if !fix_x && !(lock_axis && d.x == 0.0) {
                let xs: Vec<f64> = pts.iter().map(|q| q.x + out.x).collect();
                if let Some((corr, v, r, i)) = best(&c.xs, &xs) {
                    out.x += corr;
                    fix_x = true;
                    hints.push(SnapHint {
                        a: r,
                        b: Pt::new(v, pts[i].y + out.y),
                        label: None,
                    });
                }
            }
            if !fix_y && !(lock_axis && d.y == 0.0) {
                let ys: Vec<f64> = pts.iter().map(|q| q.y + out.y).collect();
                if let Some((corr, v, r, i)) = best(&c.ys, &ys) {
                    out.y += corr;
                    fix_y = true;
                    hints.push(SnapHint {
                        a: r,
                        b: Pt::new(pts[i].x + out.x, v),
                        label: None,
                    });
                }
            }
        }

        // 2b. Gleiche Abstände: Lücken zu Nachbarn so einrasten, dass sie so groß sind wie
        // andere Lücken in derselben Reihe (oder links und rechts gleich groß)
        if let (true, true, Some(c)) = (self.smart_snap, self.snap_spacing, self.snap_ctx.as_ref()) {
            let moved = |out: Pt| Rect {
                x0: b.x0 + out.x,
                y0: b.y0 + out.y,
                x1: b.x1 + out.x,
                y1: b.y1 + out.y,
            };
            if !fix_x && !(lock_axis && d.x == 0.0) {
                if let Some((corr, gaps)) = equal_gap_snap(&moved(out), &c.boxes, true, tol) {
                    out.x += corr;
                    fix_x = true;
                    for (g0, g1, at) in gaps {
                        hints.push(SnapHint {
                            a: Pt::new(g0, at),
                            b: Pt::new(g1, at),
                            label: None,
                        });
                    }
                }
            }
            if !fix_y && !(lock_axis && d.y == 0.0) {
                if let Some((corr, gaps)) = equal_gap_snap(&moved(out), &c.boxes, false, tol) {
                    out.y += corr;
                    fix_y = true;
                    for (g0, g1, at) in gaps {
                        hints.push(SnapHint {
                            a: Pt::new(at, g0),
                            b: Pt::new(at, g1),
                            label: None,
                        });
                    }
                }
            }
        }

        // 3. Raster
        if self.grid_snap && self.grid_size > 0.0 {
            let g = self.grid_size;
            if !fix_x {
                out.x = ((b.x0 + out.x) / g).round() * g - b.x0;
            }
            if !fix_y {
                out.y = ((b.y0 + out.y) / g).round() * g - b.y0;
            }
        }
        out
    }

    /// Hilfslinie nahe der Bildschirmposition `s`.
    pub fn guide_at(&self, s: Pt) -> Option<usize> {
        if !self.show_guides || self.lock_guides {
            return None;
        }
        let p = self.view.to_doc(s);
        let tol = 5.0 / self.view.zoom;
        self.doc
            .guides
            .iter()
            .enumerate()
            .filter(|(_, g)| self.doc.guide_visible(g) && !self.doc.guide_locked(g))
            .map(|(i, g)| (i, g.dist(p)))
            .filter(|(_, d)| *d <= tol)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|x| x.0)
    }

    /// Hilfslinienwerkzeug: Klicken und Ziehen legt eine Linie über das ganze Dokument, die
    /// Zugrichtung bestimmt den Winkel (Umschalt: 45°-Schritte). Ziehen an einer vorhandenen
    /// Linie verschiebt sie.
    fn guide_press(&mut self, p: Pt, s: Pt, mods: Mods) {
        self.selection.clear();
        self.node_sel.clear();
        if let Some(idx) = self.guide_at(s) {
            self.guide_sel = Some(idx);
            let orig = self.doc.guides[idx].pos;
            self.begin_change();
            self.drag = Drag::Guide { idx, start: p, orig };
            return;
        }
        if self.lock_guides
            || self
                .doc
                .layer(self.active_layer)
                .map(|l| l.locked)
                .unwrap_or(false)
        {
            self.message = Some(tr("Die aktuelle Ebene ist gesperrt").into());
            return;
        }
        let _ = mods;
        let pos = self.snap(p);
        self.begin_change();
        self.touch();
        self.show_guides = true;
        self.doc.guides.push(Guide {
            pos,
            angle: self.guide_angle,
            layer: self.active_layer,
        });
        let idx = self.doc.guides.len() - 1;
        self.guide_sel = Some(idx);
        self.drag = Drag::GuideNew { idx, origin: pos };
    }

    pub fn delete_guide(&mut self) -> bool {
        let Some(i) = self.guide_sel.filter(|i| *i < self.doc.guides.len()) else {
            return false;
        };
        self.checkpoint(None);
        self.doc.guides.remove(i);
        self.guide_sel = None;
        true
    }

    pub fn clear_guides(&mut self) -> bool {
        if self.doc.guides.is_empty() {
            return false;
        }
        self.checkpoint(None);
        self.doc.guides.clear();
        self.guide_sel = None;
        true
    }

    /// Winkel der gewählten Hilfslinie bzw. für neue Hilfslinien setzen.
    pub fn set_guide_angle(&mut self, deg: f64) -> bool {
        let deg = normalize_angle(deg);
        self.guide_angle = deg;
        let Some(i) = self.guide_sel.filter(|i| *i < self.doc.guides.len()) else {
            return false;
        };
        if (self.doc.guides[i].angle - deg).abs() < 1e-9 {
            return false;
        }
        self.checkpoint(Some("guide-angle"));
        self.doc.guides[i].angle = deg;
        true
    }

    /// Lage der gewählten Hilfslinie: y bei waagerechten, x bei allen anderen.
    pub fn set_guide_position(&mut self, v: f64) -> bool {
        let Some(i) = self.guide_sel.filter(|i| *i < self.doc.guides.len()) else {
            return false;
        };
        self.checkpoint(Some("guide-pos"));
        let g = &mut self.doc.guides[i];
        if g.is_horizontal() {
            g.pos.y = v;
        } else if g.is_vertical() {
            g.pos.x = v;
        } else {
            // schräge Linie: entlang x verschieben (Schnitt mit der Waagerechten durch pos)
            g.pos.x = v;
        }
        true
    }

    // ───────────── Farbverläufe ─────────────

    /// Verlaufslinie des einzigen gewählten Objekts: (Objekt, Anfang, Ende, Rahmen).
    pub fn gradient_line(&self, m: &dyn Metrics) -> Option<(u64, Pt, Pt, Rect)> {
        if self.tool != Tool::Select || self.selection.len() != 1 {
            return None;
        }
        let o = self.doc.object(self.selection[0])?;
        let g = o.style.grad(self.grad_target)?;
        let r = m.bbox(o)?;
        let (a, b) = g.line(&r);
        Some((o.id, a, b, r))
    }

    /// Winkel des Verlaufs des ersten gewählten Objekts.
    pub fn gradient_angle(&self, m: &dyn Metrics) -> f64 {
        self.gradient_angle_of(self.grad_target, m)
    }

    /// Winkel des Verlaufs von Füllung, Kontur oder Schatten.
    pub fn gradient_angle_of(&self, t: PaintTarget, m: &dyn Metrics) -> f64 {
        self.selected_objects()
            .into_iter()
            .find_map(|o| {
                let g = o.style.grad(t)?;
                Some(g.angle(&m.bbox(o)?))
            })
            .unwrap_or(0.0)
    }

    fn gradient_ids(&self) -> Vec<u64> {
        self.selected_objects()
            .into_iter()
            .filter(|o| o.style.grad(self.grad_target).is_some())
            .map(|o| o.id)
            .collect()
    }

    /// Verlauf (für `grad_target`) zu allen gewählten Objekten hinzufügen: von der Farbe
    /// nach Weiß bzw. Dunkel.
    pub fn add_gradient(&mut self) -> bool {
        let t = self.grad_target;
        let ids: Vec<u64> = self
            .selected_objects()
            .into_iter()
            .filter(|o| o.style.grad(t).is_none() && !matches!(o.shape, Shape::Image { .. }))
            .map(|o| o.id)
            .collect();
        if ids.is_empty() {
            return false;
        }
        self.checkpoint(None);
        let fallback = self
            .defaults
            .paint(t)
            .or(self.defaults.fill)
            .unwrap_or(Color::BLACK);
        for id in ids {
            if let Some(o) = self.doc.object_mut(id) {
                let a = o.style.paint(t).unwrap_or(fallback);
                let light = a.r + a.g + a.b > 2.7;
                let b = if light {
                    Color::rgb(0.11, 0.11, 0.11)
                } else {
                    Color::WHITE
                };
                o.style.set_paint(t, Some(a), Some(Gradient::new(a, b)));
            }
        }
        self.grad_stop = 0;
        true
    }

    /// Farbe (mit Verlauf oder einfarbig) für Füllung, Kontur oder Schatten setzen.
    pub fn set_paint(&mut self, t: PaintTarget, c: Option<Color>, g: Option<Gradient>) -> bool {
        let key = match t {
            PaintTarget::Fill => "fill",
            PaintTarget::Stroke => "stroke",
            PaintTarget::Shadow => "shadow",
        };
        if g.is_some() {
            self.grad_target = t;
            self.grad_stop = 0;
        }
        self.update_style(key, move |s| s.set_paint(t, c, g.clone()))
    }

    /// Nur die Deckkraft ändern – bei mehreren Objekten behält jedes seine Farbe.
    pub fn set_paint_alpha(&mut self, t: PaintTarget, a: f64) -> bool {
        let key = match t {
            PaintTarget::Fill => "alpha-fill",
            PaintTarget::Stroke => "alpha-stroke",
            PaintTarget::Shadow => "alpha-shadow",
        };
        let a = a.clamp(0.0, 1.0);
        self.update_style(key, move |s| s.set_paint_alpha(t, a))
    }

    /// Kontur in eine eigene Fläche umwandeln (lässt sich dann wie jede Form bearbeiten).
    /// Ecken, Enden, Strichmuster und Linienenden sehen danach genauso aus wie vorher.
    pub fn outline_stroke(&mut self, m: &dyn Metrics) -> bool {
        if self.selection_locked() {
            return false;
        }
        let items: Vec<(u64, Object)> = self
            .selected_objects()
            .into_iter()
            .filter(|o| o.style.stroke.is_some() && o.style.width > 0.0)
            .filter(|o| !matches!(o.shape, Shape::Image { .. }))
            .map(|o| (o.id, o.clone()))
            .collect();
        if items.is_empty() {
            return false;
        }
        let mut made: Vec<(u64, Object)> = Vec::new();
        for (id, o) in &items {
            let Some(subs) = Self::outline_of(o, m) else {
                continue;
            };
            let subs = crate::model::effective_subpaths(&subs);
            let w = o.style.width;
            let align = if o.shape.is_open() {
                crate::model::StrokeAlign::Center
            } else {
                o.style.stroke_align
            };
            let r = if align == crate::model::StrokeAlign::Center {
                w / 2.0
            } else {
                w
            };
            // wie beim Zeichnen: innen/außen mit doppelter Breite, danach beschnitten
            let dashed = o.style.dash != crate::model::Dash::Solid;
            let st = crate::stroker::StrokeStyle {
                width: 2.0 * r,
                join: o.style.join,
                cap: if dashed && o.style.dash.round_caps() {
                    crate::model::LineCap::Round
                } else {
                    o.style.cap
                },
                dash: o.style.dash.pattern(w),
            };
            let mk = o.style.marker;
            let with_markers = mk != crate::model::Marker::None && o.shape.is_open();
            let mut region = if with_markers {
                // Linie an den Enden kürzen (wie beim Zeichnen), dann die Linienenden dazu
                let ends = o.style.marker_ends;
                let size = o.style.marker_size;
                let trim = mk.trim(w, size);
                let trimmed: Vec<SubPath> = subs
                    .iter()
                    .map(|sp| {
                        crate::model::trim_subpath(
                            sp,
                            if ends.start() { trim } else { 0.0 },
                            if ends.end() { trim } else { 0.0 },
                        )
                    })
                    .collect();
                let mut parts = vec![crate::stroker::stroke_area(&trimmed, &st)];
                for sp in &subs {
                    if let Some((a, b)) = crate::model::subpath_ends(sp) {
                        if ends.start() {
                            parts.push(marker_area(a.0, a.1, w, size, mk));
                        }
                        if ends.end() {
                            parts.push(marker_area(b.0, b.1, w, size, mk));
                        }
                    }
                }
                crate::stroker::union_all(parts)
            } else {
                crate::stroker::stroke_area(&subs, &st)
            };
            if region.is_empty() {
                continue;
            }
            let tol = 0.25;
            let fill = boolean::to_rings(&subs, tol);
            region = match align {
                crate::model::StrokeAlign::Inside => boolean::rings_op(&region, &fill, Op::Intersection),
                crate::model::StrokeAlign::Outside => boolean::rings_op(&region, &fill, Op::Difference),
                crate::model::StrokeAlign::Center => region,
            };
            if region.is_empty() {
                continue;
            }
            let mut style = o.style.clone();
            style.fill = o.style.stroke;
            style.gradient = o.style.stroke_gradient.clone();
            style.stroke = None;
            style.stroke_gradient = None;
            style.marker = crate::model::Marker::None;
            let shape = Shape::Path {
                subpaths: boolean::rings_to_paths(region),
                evenodd: true,
            };
            made.push((
                *id,
                Object {
                    id: 0,
                    group: o.group,
                    locked: false,
                    name: String::new(),
                    shape,
                    style,
                    angle: o.angle,
                },
            ));
        }
        if made.is_empty() {
            return false;
        }
        self.checkpoint(None);
        let mut sel = Vec::new();
        for (id, mut obj) in made {
            let Some((li, oi)) = self.doc.locate(id) else {
                continue;
            };
            obj.id = self.doc.alloc_id();
            sel.push(obj.id);
            let keep = self.doc.layers[li].objects[oi].style.fill.is_some();
            if keep {
                // Fläche bleibt, nur ohne Kontur; die neue Kontur-Fläche liegt darüber
                let st = &mut self.doc.layers[li].objects[oi].style;
                st.stroke = None;
                st.stroke_gradient = None;
                self.doc.layers[li].objects.insert(oi + 1, obj);
            } else {
                self.doc.layers[li].objects[oi] = obj;
            }
        }
        self.selection = sel;
        self.layers_version += 1;
        true
    }

    /// Farbe und Verlauf des ersten gewählten Objekts (für die Bibliothek).
    pub fn selected_paint(&self, t: PaintTarget) -> Option<(Color, Option<Gradient>)> {
        self.selected_objects()
            .into_iter()
            .find_map(|o| Some((o.style.paint(t)?, o.style.grad(t).cloned())))
    }

    pub fn remove_gradient(&mut self) -> bool {
        let ids = self.gradient_ids();
        if ids.is_empty() {
            return false;
        }
        self.checkpoint(None);
        let t = self.grad_target;
        for id in ids {
            if let Some(o) = self.doc.object_mut(id) {
                // Farbe = erste Verlaufsfarbe
                if let Some(g) = o.style.grad_slot(t).take() {
                    if let Some(st) = g.sorted().first() {
                        o.style.set_paint(t, Some(st.color), None);
                    }
                }
            }
        }
        true
    }

    fn with_gradients(&mut self, key: &str, f: impl Fn(&mut Gradient)) -> bool {
        let ids = self.gradient_ids();
        if ids.is_empty() {
            return false;
        }
        self.checkpoint(Some(key));
        let t = self.grad_target;
        for id in ids {
            if let Some(g) = self
                .doc
                .object_mut(id)
                .and_then(|o| o.style.grad_slot(t).as_mut())
            {
                f(g);
            }
        }
        true
    }

    pub fn set_stop_color(&mut self, idx: usize, c: Color) -> bool {
        self.with_gradients(&format!("stop-color{idx}"), |g| {
            if let Some(st) = g.stops.get_mut(idx) {
                st.color = c;
            }
        })
    }

    pub fn set_stop_pos(&mut self, idx: usize, t: f64) -> bool {
        let t = t.clamp(0.0, 1.0);
        self.with_gradients(&format!("stop-pos{idx}"), |g| {
            if let Some(st) = g.stops.get_mut(idx) {
                st.pos = t;
            }
        })
    }

    /// Neuen Farbstopp bei `t` einfügen und auswählen.
    pub fn add_stop(&mut self, t: f64) -> bool {
        let ids = self.gradient_ids();
        if ids.is_empty() {
            return false;
        }
        self.checkpoint(None);
        let mut sel = self.grad_stop;
        let tg = self.grad_target;
        for id in ids {
            if let Some(g) = self
                .doc
                .object_mut(id)
                .and_then(|o| o.style.grad_slot(tg).as_mut())
            {
                sel = g.add_stop(t);
            }
        }
        self.grad_stop = sel;
        true
    }

    /// Ausgewählten Farbstopp entfernen (mindestens zwei bleiben).
    pub fn remove_stop(&mut self) -> bool {
        let idx = self.grad_stop;
        let ok = self
            .selected_objects()
            .into_iter()
            .filter_map(|o| o.style.grad(self.grad_target))
            .any(|g| g.stops.len() > 2 && idx < g.stops.len());
        if !ok {
            return false;
        }
        let changed = self.with_gradients("remove-stop", |g| {
            if g.stops.len() > 2 && idx < g.stops.len() {
                g.stops.remove(idx);
            }
        });
        self.grad_stop = idx.saturating_sub(1);
        changed
    }

    /// Form des Verlaufs: linear, kreisförmig oder Muster (legt bei Bedarf einen Verlauf an).
    pub fn set_gradient_shape(&mut self, shape: crate::model::GradShape) -> bool {
        let t = self.grad_target;
        if self.selected_objects().iter().any(|o| o.style.grad(t).is_none()) {
            self.add_gradient();
        }
        self.with_gradients("gradient-shape", move |g| g.set_shape(shape))
    }

    /// Größe eines Musters.
    pub fn set_pattern_cell(&mut self, v: f64) -> bool {
        self.with_gradients("pattern-cell", move |g| g.cell = v.max(1.0))
    }

    pub fn set_gradient_kind(&mut self, kind: GradientKind) -> bool {
        self.with_gradients("gradient-kind", |g| g.kind = kind)
    }

    pub fn set_gradient_angle(&mut self, deg: f64, m: &dyn Metrics) -> bool {
        let ids = self.gradient_ids();
        if ids.is_empty() {
            return false;
        }
        self.checkpoint(Some("gradient-angle"));
        for id in ids {
            let Some(r) = self.doc.object(id).and_then(|o| m.bbox(o)) else {
                continue;
            };
            let t = self.grad_target;
            if let Some(g) = self
                .doc
                .object_mut(id)
                .and_then(|o| o.style.grad_slot(t).as_mut())
            {
                g.set_angle(deg, &r);
            }
        }
        true
    }

    pub fn set_blend(&mut self, b: Blend) -> bool {
        self.update_style("blend", move |s| s.blend = b)
    }

    /// Absatzausrichtung für Text (auch als Vorgabe).
    pub fn set_text_align(&mut self, a: Align) -> bool {
        self.update_text("align", move |t| t.fmt.align = a)
    }
}

// ───────────── Bilder in Rahmen ─────────────

impl Editor {
    /// Bearbeiteter Teil eines gerahmten Bildes (sonst `All`).
    pub fn image_part(&self, id: u64) -> ImagePart {
        match self.frame_mode {
            Some((fid, part)) if fid == id && self.selection == [id] => {
                let framed = matches!(
                    self.doc.object(id).map(|o| &o.shape),
                    Some(Shape::Image { clip: Some(_), .. })
                );
                if framed {
                    part
                } else {
                    ImagePart::All
                }
            }
            _ => ImagePart::All,
        }
    }

    /// Teil für das gewählte gerahmte Bild festlegen (Seitenleiste).
    pub fn set_frame_mode(&mut self, part: ImagePart) {
        if let [id] = self.selection[..] {
            self.frame_mode = Some((id, part));
        }
    }

    /// Oberstes Objekt unter `p`, das als Rahmen taugt (kein Bild, nicht `exclude`).
    fn frame_target_at(&self, p: Pt, m: &dyn Metrics, exclude: u64) -> Option<u64> {
        let tol = 2.0 / self.view.zoom;
        for l in self.doc.layers.iter().rev().filter(|l| l.editable()) {
            for o in l.objects.iter().rev() {
                if o.id == exclude || matches!(o.shape, Shape::Image { .. }) {
                    continue;
                }
                if m.hit(o, p, tol) {
                    return Some(o.id);
                }
            }
        }
        None
    }

    fn set_frame_hover(&mut self, target: Option<u64>, s: Pt) {
        self.frame_hover = match (target, self.frame_hover) {
            // Ruhig gehalten (wenige Pixel Spiel): Zeit läuft weiter
            (Some(t), Some((t0, since, anchor))) if t == t0 && anchor.dist(s) <= 8.0 => {
                Some((t0, since, anchor))
            }
            (Some(t), _) => Some((t, Instant::now(), s)),
            (None, _) => None,
        };
    }

    /// Während ein einzelnes Bild verschoben wird: Form unter dem Zeiger merken.
    fn update_frame_hover(&mut self, s: Pt, m: &dyn Metrics) {
        let img = match self.selection[..] {
            [id] if matches!(
                self.doc.object(id).map(|o| &o.shape),
                Some(Shape::Image { .. } | Shape::TextArea { .. })
            ) && self.image_part(id) == ImagePart::All =>
            {
                id
            }
            _ => {
                self.frame_hover = None;
                return;
            }
        };
        let p = self.view.to_doc(s);
        let t = self.frame_target_at(p, m, img);
        self.set_frame_hover(t, s);
    }

    /// Bilddatei wird über die Zeichenfläche gezogen (Bildschirmkoordinaten).
    pub fn drop_hover(&mut self, s: Pt, m: &dyn Metrics) {
        let p = self.view.to_doc(s);
        let t = self.frame_target_at(p, m, 0);
        self.set_frame_hover(t, s);
    }

    /// Fortschritt der Wartezeit über einer Form (0–1).
    pub fn frame_progress(&self) -> Option<(u64, f64)> {
        let (t, since, _) = self.frame_hover?;
        Some((
            t,
            (since.elapsed().as_secs_f64() / self.frame_dwell.max(0.05)).min(1.0),
        ))
    }

    /// Form, in die das Bild beim Loslassen gesetzt wird.
    pub fn frame_ready(&self) -> Option<u64> {
        self.frame_progress().filter(|(_, p)| *p >= 1.0).map(|(t, _)| t)
    }

    /// Wie das Objekt `item` (Bild oder Textfeld) in der Form `target` aussehen würde.
    pub fn frame_preview(&self, item: u64, target: u64, m: &dyn Metrics) -> Option<Object> {
        let frame_obj = self.doc.object(target)?;
        // Lebendige Ecken bleiben erhalten, damit der Radius später änderbar ist.
        let outline = match &frame_obj.shape {
            Shape::Path { subpaths, .. } => Some(subpaths.clone()),
            Shape::Rect { x, y, w, h, r } => Some(vec![SubPath {
                nodes: [
                    Pt::new(*x, *y),
                    Pt::new(x + w, *y),
                    Pt::new(x + w, y + h),
                    Pt::new(*x, y + h),
                ]
                .iter()
                .map(|p| Node {
                    r: r.max(0.0),
                    ..Node::corner(*p)
                })
                .collect(),
                closed: true,
            }]),
            _ => Self::outline_of(frame_obj, m),
        }?;
        let fb = (Shape::Path {
            subpaths: outline.clone(),
            evenodd: false,
        })
        .geom_bbox()?;
        let mut obj = self.doc.object(item)?.clone();
        obj.angle = 0.0;
        match &mut obj.shape {
            // Textfeld in Form: Text bricht innerhalb der Form um
            Shape::TextArea {
                xf: txf,
                w: tw,
                h: th,
                frame,
                ..
            } => {
                *txf = Affine::translate(fb.x0, fb.y0);
                *tw = fb.width().max(1.0);
                *th = fb.height().max(1.0);
                let mut local = Shape::Path {
                    subpaths: outline,
                    evenodd: false,
                };
                local.translate(Pt::new(-fb.x0, -fb.y0));
                if let Shape::Path { subpaths, .. } = local {
                    frame.path = Some(subpaths);
                }
                frame.fill = frame_obj.style.fill;
                frame.stroke = frame_obj.style.stroke;
                frame.width = frame_obj.style.width;
                frame.inset = frame.inset.max(8.0);
                if self.placeholders.contains(&item) {
                    if let Some(t) = m.fill_area(&obj) {
                        if let Some((txt, ..)) = obj.shape.text_fields_mut() {
                            *txt = t;
                        }
                    }
                }
            }
            Shape::Image { w, h, xf, clip, .. } => {
                *xf = frame_fit(*w, *h, &fb);
                *clip = Some(outline);
                // Kontur der Form bleibt als Rahmenlinie erhalten
                obj.style.stroke = frame_obj.style.stroke;
                obj.style.width = frame_obj.style.width;
            }
            _ => return None,
        }
        Some(obj)
    }

    /// Bild oder Textfeld in eine Form setzen (die Form wird zum Rahmen).
    /// Ohne eigenen Rückgängig-Schritt.
    pub fn place_in_frame(&mut self, item: u64, target: u64, m: &dyn Metrics) -> bool {
        let Some(obj) = self.frame_preview(item, target, m) else {
            return false;
        };
        let is_image = matches!(obj.shape, Shape::Image { .. });
        self.touch();
        self.doc.remove_object(item);
        let Some((li, oi)) = self.doc.locate(target) else {
            return false;
        };
        self.doc.layers[li].objects[oi] = obj;
        self.selection = vec![item];
        self.frame_hover = None;
        if is_image {
            self.frame_mode = Some((item, ImagePart::Content));
        }
        true
    }

    /// Gewähltes Bild zuschneiden: rechteckiger Rahmen in Bildgröße, danach Rahmen bearbeiten.
    pub fn crop_image(&mut self) -> bool {
        let [id] = self.selection[..] else {
            return false;
        };
        let Some(q) = self.doc.object(id).and_then(|o| match &o.shape {
            Shape::Image { clip: None, .. } => o.shape.image_quad(),
            _ => None,
        }) else {
            return false;
        };
        self.checkpoint(None);
        if let Some(Shape::Image { clip, .. }) = self.doc.object_mut(id).map(|o| &mut o.shape) {
            *clip = Some(vec![SubPath {
                nodes: q.iter().map(|p| Node::corner(*p)).collect(),
                closed: true,
            }]);
        }
        self.frame_mode = Some((id, ImagePart::Frame));
        true
    }

    /// Rahmen entfernen; eine Rahmenlinie bleibt als eigener Pfad erhalten.
    pub fn release_frame(&mut self) -> bool {
        let [id] = self.selection[..] else {
            return false;
        };
        let Some((clip, stroke, width)) = self.doc.object(id).and_then(|o| match &o.shape {
            Shape::Image { clip: Some(c), .. } => Some((c.clone(), o.style.stroke, o.style.width)),
            _ => None,
        }) else {
            return false;
        };
        self.checkpoint(None);
        if let Some(o) = self.doc.object_mut(id) {
            if let Shape::Image { clip: c, .. } = &mut o.shape {
                *c = None;
            }
            o.style.stroke = None;
        }
        // Die Form bleibt mit einfacher Füllung hinter dem Bild zurück.
        if let Some((li, oi)) = self.doc.locate(id) {
            let nid = self.doc.alloc_id();
            self.doc.layers[li].objects.insert(
                oi,
                Object {
                    group: 0,
                    locked: false,
                    name: String::new(),
                    id: nid,
                    shape: Shape::Path {
                        subpaths: clip,
                        evenodd: false,
                    },
                    style: Style {
                        fill: self.defaults.fill.or(Style::default().fill),
                        stroke,
                        width,
                        ..Style::default()
                    },
                    angle: 0.0,
                },
            );
        }
        self.frame_mode = None;
        true
    }

    /// Rahmen der gewählten Textfelder ändern (Füllung, Kontur, Abstand …).
    pub fn set_text_frame(&mut self, key: &str, f: impl Fn(&mut TextFrame), m: &dyn Metrics) -> bool {
        let ids: Vec<u64> = self
            .selected_objects()
            .into_iter()
            .filter(|o| o.shape.is_text())
            .map(|o| o.id)
            .collect();
        if ids.is_empty() {
            return false;
        }
        self.checkpoint(Some(key));
        for id in ids {
            match self.doc.object_mut(id).map(|o| &mut o.shape) {
                Some(Shape::TextArea { frame, .. }) => f(frame),
                Some(Shape::Text { fmt, .. }) | Some(Shape::TextPath { fmt, .. }) => {
                    let mut b = fmt.text_box();
                    f(&mut b);
                    fmt.set_text_box(&b);
                }
                _ => {}
            }
        }
        self.refill_placeholders(m);
        true
    }

    /// Textfeld aus der Form lösen: Die Form bleibt als eigenes Objekt hinter dem Text.
    pub fn release_text_shape(&mut self) -> bool {
        let [id] = self.selection[..] else {
            return false;
        };
        let Some((xf, frame)) = self.doc.object(id).and_then(|o| match &o.shape {
            Shape::TextArea { xf, frame, .. } if frame.path.is_some() => Some((*xf, frame.clone())),
            _ => None,
        }) else {
            return false;
        };
        self.checkpoint(None);
        let mut shape = Shape::Path {
            subpaths: frame.path.clone().unwrap_or_default(),
            evenodd: false,
        };
        shape.transform(&xf);
        if let Some(Shape::TextArea { frame: f, .. }) = self.doc.object_mut(id).map(|o| &mut o.shape) {
            *f = TextFrame {
                inset: f.inset,
                ..TextFrame::default()
            };
        }
        if let Some((li, oi)) = self.doc.locate(id) {
            let nid = self.doc.alloc_id();
            self.doc.layers[li].objects.insert(
                oi,
                Object {
                    group: 0,
                    locked: false,
                    name: String::new(),
                    id: nid,
                    shape,
                    style: Style {
                        fill: frame.fill,
                        stroke: frame.stroke,
                        width: frame.width,
                        ..Style::default()
                    },
                    angle: 0.0,
                },
            );
        }
        true
    }

    /// Nur die Auswahl als eigenes Dokument (zum Exportieren oder Kopieren als Bild): so groß
    /// wie die Auswahl samt Kontur, Schatten und Spiegelung, nach (0, 0) verschoben.
    pub fn selection_doc(&self, m: &dyn Metrics) -> Option<Document> {
        let objs = self.selected_in_order();
        let mut bounds: Option<Rect> = None;
        for o in &objs {
            let Some(b) = m.bbox(o) else { continue };
            let st = &o.style;
            let mut r = if st.stroke.is_some() {
                b.inflate(st.width.max(0.0))
            } else {
                b
            };
            if let Some(sh) = st.shadow {
                let blur = sh.blur.max(0.0) * 2.0;
                let moved = Rect {
                    x0: r.x0 + sh.dx - blur,
                    y0: r.y0 + sh.dy - blur,
                    x1: r.x1 + sh.dx + blur,
                    y1: r.y1 + sh.dy + blur,
                };
                r = r.union(&moved);
            }
            if st.fx.mirror.is_some() {
                r.y1 += b.height();
            }
            bounds = Some(match bounds {
                Some(a) => a.union(&r),
                None => r,
            });
        }
        let b = bounds?;
        if b.width() <= 0.0 || b.height() <= 0.0 {
            return None;
        }
        let mut doc = self.doc.clone();
        doc.artboards.clear();
        doc.guides.clear();
        doc.width = b.width().ceil();
        doc.height = b.height().ceil();
        let t = Affine::translate(-b.x0, -b.y0);
        let keep: Vec<u64> = objs.iter().map(|o| o.id).collect();
        for l in doc.layers.iter_mut() {
            l.objects.retain(|o| keep.contains(&o.id));
            for o in l.objects.iter_mut() {
                let before = m.bbox(o);
                o.shape.transform(&t);
                if let (Some(b0), Some(b1)) = (before, m.bbox(o)) {
                    for pt in [PaintTarget::Fill, PaintTarget::Stroke, PaintTarget::Shadow] {
                        if let Some(g) = o.style.grad_slot(pt).as_mut() {
                            g.transform(&t, &b0, &b1);
                        }
                    }
                }
            }
        }
        Some(doc)
    }

    /// Einheiten der Auswahl (Gruppen als Ganzes): (Gruppe, Objekte, Rahmen).
    pub fn selection_units(&self, m: &dyn Metrics) -> Vec<(u64, Vec<u64>, Rect)> {
        let mut units: Vec<(u64, Vec<u64>, Rect)> = Vec::new();
        for id in self.selection.clone() {
            let Some(o) = self.doc.object(id) else {
                continue;
            };
            let Some(b) = m.bbox(o) else {
                continue;
            };
            let g = o.group;
            if g != 0 {
                if let Some(u) = units.iter_mut().find(|u| u.0 == g) {
                    u.1.push(id);
                    u.2 = u.2.union(&b);
                    continue;
                }
            }
            units.push((g, vec![id], b));
        }
        units
    }

    /// Ausrichten: bei mehreren Einheiten (Objekte, Gruppen) und `align_selection`
    /// aneinander (am Rahmen der Auswahl), sonst an der Seite.
    pub fn align(&mut self, h: Option<i8>, v: Option<i8>, m: &dyn Metrics) -> bool {
        if self.align_selection && self.selection_units(m).len() >= 2 {
            self.align_to_selection(h, v, m)
        } else {
            self.align_to_page(h, v, m)
        }
    }

    /// Objekte aneinander ausrichten: jede Einheit an die linke/mittlere/rechte (bzw.
    /// obere/mittlere/untere) Kante des gemeinsamen Rahmens.
    pub fn align_to_selection(&mut self, h: Option<i8>, v: Option<i8>, m: &dyn Metrics) -> bool {
        if self.selection_locked() {
            self.message = Some(tr("Gesperrte Objekte lassen sich nicht verändern").into());
            return false;
        }
        let units = self.selection_units(m);
        let Some(all) = units.iter().map(|u| u.2).reduce(|a, b| a.union(&b)) else {
            return false;
        };
        let mut moves: Vec<(Vec<u64>, f64, f64)> = Vec::new();
        for (_, ids, b) in &units {
            let dx = match h {
                Some(-1) => all.x0 - b.x0,
                Some(0) => (all.x0 + all.x1 - b.x0 - b.x1) / 2.0,
                Some(_) => all.x1 - b.x1,
                None => 0.0,
            };
            let dy = match v {
                Some(-1) => all.y0 - b.y0,
                Some(0) => (all.y0 + all.y1 - b.y0 - b.y1) / 2.0,
                Some(_) => all.y1 - b.y1,
                None => 0.0,
            };
            if dx.abs() > 1e-9 || dy.abs() > 1e-9 {
                moves.push((ids.clone(), dx, dy));
            }
        }
        if moves.is_empty() {
            return false;
        }
        self.checkpoint(None);
        for (ids, dx, dy) in moves {
            let t = Affine::translate(dx, dy);
            for id in ids {
                self.transform_object(id, &t, m);
            }
        }
        true
    }

    /// Auswahl an der Seite ausrichten: `h`/`v` = −1 (links/oben), 0 (Mitte), 1 (rechts/unten).
    pub fn align_to_page(&mut self, h: Option<i8>, v: Option<i8>, m: &dyn Metrics) -> bool {
        if self.selection_locked() {
            self.message = Some(tr("Gesperrte Objekte lassen sich nicht verändern").into());
            return false;
        }
        let Some(b) = self.selection_bbox(m) else {
            return false;
        };
        // Zeichenfläche, auf der die Auswahl (ihre Mitte) liegt
        let page = self.artboard_at(Pt::new((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0));
        let dx = match h {
            Some(-1) => page.x0 - b.x0,
            Some(0) => (page.x0 + page.x1 - b.width()) / 2.0 - b.x0,
            Some(_) => page.x1 - b.x1,
            None => 0.0,
        };
        let dy = match v {
            Some(-1) => page.y0 - b.y0,
            Some(0) => (page.y0 + page.y1 - b.height()) / 2.0 - b.y0,
            Some(_) => page.y1 - b.y1,
            None => 0.0,
        };
        if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
            return false;
        }
        self.checkpoint(None);
        let t = Affine::translate(dx, dy);
        for id in self.selection.clone() {
            self.transform_object(id, &t, m);
        }
        true
    }

    /// Ausgewählte Objekte (Gruppen als Einheit) mit gleichen Abständen verteilen:
    /// `horizontal` von links nach rechts, sonst von oben nach unten. Die beiden äußeren
    /// Einheiten bleiben stehen, die übrigen rücken so, dass alle Lücken gleich groß sind.
    pub fn distribute(&mut self, horizontal: bool, m: &dyn Metrics) -> bool {
        if self.selection_locked() {
            self.message = Some(tr("Gesperrte Objekte lassen sich nicht verändern").into());
            return false;
        }
        let mut units = self.selection_units(m);
        if units.len() < 3 {
            return false;
        }
        let start = |r: &Rect| if horizontal { r.x0 } else { r.y0 };
        let end = |r: &Rect| if horizontal { r.x1 } else { r.y1 };
        units.sort_by(|a, b| {
            (start(&a.2) + end(&a.2))
                .partial_cmp(&(start(&b.2) + end(&b.2)))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let n = units.len();
        let span = end(&units[n - 1].2) - start(&units[0].2);
        let sizes: f64 = units.iter().map(|u| end(&u.2) - start(&u.2)).sum();
        let gap = (span - sizes) / (n - 1) as f64;
        let mut moves: Vec<(Vec<u64>, f64)> = Vec::new();
        let mut pos = end(&units[0].2) + gap;
        for u in &units[1..n - 1] {
            let d = pos - start(&u.2);
            if d.abs() > 1e-9 {
                moves.push((u.1.clone(), d));
            }
            pos += end(&u.2) - start(&u.2) + gap;
        }
        if moves.is_empty() {
            return false;
        }
        self.checkpoint(None);
        for (ids, d) in moves {
            let t = if horizontal {
                Affine::translate(d, 0.0)
            } else {
                Affine::translate(0.0, d)
            };
            for id in ids {
                self.transform_object(id, &t, m);
            }
        }
        true
    }

    /// Sind genau zwei Formen oder Pfade ausgewählt (kein Text, kein Bild)? Nur dann gibt es einen Übergang.
    pub fn can_blend(&self) -> bool {
        if self.selection.len() != 2 {
            return false;
        }
        self.selected_in_order()
            .iter()
            .all(|o| blend_outline(&o.shape).is_some())
    }

    /// Übergang zwischen zwei ausgewählten Formen: `steps` Zwischenformen, die Form, Farben,
    /// Konturstärke und Deckkraft von der unteren zur oberen Form überblenden. Liegen beide
    /// Formen auf derselben Ebene, werden sie samt Zwischenformen gruppiert.
    pub fn blend_selected(&mut self, steps: usize) -> bool {
        let steps = steps.clamp(1, 200);
        if self.selection.len() != 2 {
            return false;
        }
        if self.selection_locked() {
            self.message = Some(tr("Gesperrte Objekte lassen sich nicht verändern").into());
            return false;
        }
        let objs = self.selected_in_order();
        let (a, b) = (&objs[0], &objs[1]);
        let (Some(pa), Some(pb)) = (blend_outline(&a.shape), blend_outline(&b.shape)) else {
            self.message = Some(
                tr("Ein Übergang geht nur zwischen Formen und Pfaden, nicht mit Text oder Bildern").into(),
            );
            return false;
        };
        let (pa, pb) = blend_paths(&pa, &pb);
        let (Some((la, _)), Some((lb, ob))) = (self.doc.locate(a.id), self.doc.locate(b.id)) else {
            return false;
        };
        let same_layer = la == lb;
        self.checkpoint(None);
        let gid = self.doc.max_group() + 1;
        let mut new_ids = Vec::new();
        for i in 1..=steps {
            let t = i as f64 / (steps + 1) as f64;
            let nodes: Vec<Node> = pa
                .nodes
                .iter()
                .zip(&pb.nodes)
                .map(|(p, q)| Node {
                    r: 0.0,
                    p: p.p.lerp(q.p, t),
                    h_in: p.h_in.lerp(q.h_in, t),
                    h_out: p.h_out.lerp(q.h_out, t),
                })
                .collect();
            let mut st = if t < 0.5 { a.style.clone() } else { b.style.clone() };
            st.fill = lerp_paint(a.style.fill, b.style.fill, t);
            st.stroke = lerp_paint(a.style.stroke, b.style.stroke, t);
            st.width = a.style.width + (b.style.width - a.style.width) * t;
            st.opacity = a.style.opacity + (b.style.opacity - a.style.opacity) * t;
            let id = self.doc.alloc_id();
            let obj = Object {
                id,
                shape: Shape::Path {
                    subpaths: vec![SubPath {
                        nodes,
                        closed: pa.closed,
                    }],
                    evenodd: false,
                },
                style: st,
                angle: 0.0,
                group: gid,
                locked: false,
                name: String::new(),
            };
            // direkt unter der oberen Form einfügen, in Reihenfolge von unten nach oben
            self.doc.layers[lb].objects.insert(ob + i - 1, obj);
            new_ids.push(id);
        }
        if same_layer {
            for o in self.doc.layers[lb].objects.iter_mut() {
                if o.id == a.id || o.id == b.id {
                    o.group = gid;
                }
            }
            new_ids.push(a.id);
            new_ids.push(b.id);
        }
        self.selection = new_ids;
        self.node_sel.clear();
        self.layers_version += 1;
        true
    }
}

/// Mischung zweier optionaler Farben: fehlt eine, gilt die andere.
fn lerp_paint(a: Option<Color>, b: Option<Color>, t: f64) -> Option<Color> {
    match (a, b) {
        (Some(x), Some(y)) => Some(Color {
            r: x.r + (y.r - x.r) * t,
            g: x.g + (y.g - x.g) * t,
            b: x.b + (y.b - x.b) * t,
            a: x.a + (y.a - x.a) * t,
        }),
        (Some(x), None) => Some(x),
        (None, y) => y,
    }
}

/// Umriss (erster Teilpfad mit Eckenradien) einer Form für den Übergang; Text und Bilder nicht.
fn blend_outline(shape: &Shape) -> Option<SubPath> {
    if matches!(
        shape,
        Shape::Text { .. } | Shape::TextPath { .. } | Shape::TextArea { .. } | Shape::Image { .. }
    ) {
        return None;
    }
    match shape.to_path()? {
        Shape::Path { subpaths, .. } => subpaths
            .iter()
            .map(|s| s.effective())
            .find(|s| s.nodes.len() >= 2),
        _ => None,
    }
}

/// Teilpfad in umgekehrter Richtung.
fn reversed_subpath(sp: &SubPath) -> SubPath {
    SubPath {
        nodes: sp
            .nodes
            .iter()
            .rev()
            .map(|n| Node {
                r: n.r,
                p: n.p,
                h_in: n.h_out,
                h_out: n.h_in,
            })
            .collect(),
        closed: sp.closed,
    }
}

/// Teilt die längsten Segmente (ohne die Form zu ändern), bis der Pfad `target` Knoten hat.
fn subdivide_to(sp: &mut SubPath, target: usize) {
    while sp.nodes.len() < target {
        let n = sp.nodes.len();
        if n < 2 {
            break;
        }
        let segs = if sp.closed { n } else { n - 1 };
        let mut best = (0usize, -1.0f64);
        for i in 0..segs {
            let a = sp.nodes[i];
            let b = sp.nodes[(i + 1) % n];
            let len = a.p.dist(a.h_out) + a.h_out.dist(b.h_in) + b.h_in.dist(b.p);
            if len > best.1 {
                best = (i, len);
            }
        }
        let i = best.0;
        let j = (i + 1) % n;
        let (p0, p1, p2, p3) = (sp.nodes[i].p, sp.nodes[i].h_out, sp.nodes[j].h_in, sp.nodes[j].p);
        let p01 = p0.lerp(p1, 0.5);
        let p12 = p1.lerp(p2, 0.5);
        let p23 = p2.lerp(p3, 0.5);
        let p012 = p01.lerp(p12, 0.5);
        let p123 = p12.lerp(p23, 0.5);
        let mid = p012.lerp(p123, 0.5);
        sp.nodes[i].h_out = p01;
        sp.nodes[j].h_in = p23;
        sp.nodes.insert(
            i + 1,
            Node {
                r: 0.0,
                p: mid,
                h_in: p012,
                h_out: p123,
            },
        );
    }
}

/// Zwei Pfade für den Übergang vergleichbar machen: gleiche Knotenzahl (längste Segmente
/// werden geteilt) und die Zuordnung von Anfang und Richtung mit der kleinsten Gesamtstrecke.
/// Ist einer der Pfade offen, werden beide offen behandelt.
fn blend_paths(a: &SubPath, b: &SubPath) -> (SubPath, SubPath) {
    let closed = a.closed && b.closed;
    let prep = |sp: &SubPath| -> SubPath {
        let mut s = sp.clone();
        if s.closed && !closed {
            let first = s.nodes[0];
            s.nodes.push(first);
            s.closed = false;
        }
        s
    };
    let mut pa = prep(a);
    let mut pb = prep(b);
    let n = pa.nodes.len().max(pb.nodes.len());
    subdivide_to(&mut pa, n);
    subdivide_to(&mut pb, n);
    let cost = |x: &SubPath, y: &SubPath| -> f64 {
        x.nodes
            .iter()
            .zip(&y.nodes)
            .map(|(p, q)| {
                let d = p.p.dist(q.p);
                d * d
            })
            .sum()
    };
    let mut best = (f64::MAX, pb.clone());
    for cand in [pb.clone(), reversed_subpath(&pb)] {
        let turns = if closed { n } else { 1 };
        for k in 0..turns {
            let mut c = cand.clone();
            if k > 0 {
                c.nodes.rotate_left(k);
            }
            let v = cost(&pa, &c);
            if v < best.0 {
                best = (v, c);
            }
        }
    }
    (pa, best.1)
}

/// Glatte Kurve durch Punkte (Catmull-Rom als Bézier).
/// Zitternde Freihandpunkte glätten (gleitender Mittelwert); `strength` 0–1.
/// Offene Linien behalten ihre Endpunkte.
pub fn smooth_points(pts: &[Pt], strength: f64, closed: bool) -> Vec<Pt> {
    let k = (strength.clamp(0.0, 1.0) * 8.0).round() as isize;
    let n = pts.len() as isize;
    if k == 0 || n < 3 {
        return pts.to_vec();
    }
    let mut cur = pts.to_vec();
    for _ in 0..2 {
        let prev = cur.clone();
        for i in 0..n {
            if !closed && (i == 0 || i == n - 1) {
                continue;
            }
            let mut sum = Pt::new(0.0, 0.0);
            let mut cnt = 0.0;
            for j in (i - k)..=(i + k) {
                let idx = if closed {
                    j.rem_euclid(n)
                } else if j < 0 || j >= n {
                    continue;
                } else {
                    j
                };
                sum = sum + prev[idx as usize];
                cnt += 1.0;
            }
            cur[i as usize] = sum * (1.0 / cnt);
        }
    }
    cur
}

pub fn smooth_nodes(pts: &[Pt]) -> Vec<Node> {
    let n = pts.len();
    (0..n)
        .map(|i| {
            let p = pts[i];
            let prev = if i > 0 { pts[i - 1] } else { p };
            let next = if i + 1 < n { pts[i + 1] } else { p };
            let t = (next - prev) * (1.0 / 6.0);
            Node {
                r: 0.0,
                p,
                h_in: if i > 0 { p - t } else { p },
                h_out: if i + 1 < n { p + t } else { p },
            }
        })
        .collect()
}

/// Zwischenspeicher des Farbeimers: Graph aller Linien.
#[derive(Debug)]
struct BucketCache {
    key: (u64, f64, f64, f64, (f64, f64)),
    /// betrachtete Objekte (Auswahl; leer = alles Sichtbare)
    ids: Vec<u64>,
    /// nur der sichtbare Bereich wurde betrachtet (dann gilt der Graph nur für diese Ansicht)
    culled: bool,
    arr: Rc<crate::shapeops::Arrangement>,
}

/// Was aus der Bibliothek übertragen wird.
#[derive(Clone, Debug, PartialEq)]
pub enum LibPayload {
    /// Farbe, ggf. mit Verlauf
    Color(Color, Option<Gradient>),
    /// Grafik (Objekte), wird als Kopie eingefügt
    Graphic(Vec<Object>),
    Style(crate::library::LibStyle),
}

/// Farbe oder Stil auf ein Objekt anwenden. `false`, wenn es nicht passt (Bild, Textstil
/// auf eine Form).
fn restyle(o: &mut Object, p: &LibPayload) -> bool {
    if matches!(o.shape, Shape::Image { .. }) {
        return false;
    }
    let is_text = o.shape.is_text();
    match p {
        LibPayload::Color(c, g) => {
            o.style.fill = Some(*c);
            o.style.gradient = g.clone();
        }
        LibPayload::Graphic(_) => return false,
        LibPayload::Style(s) => {
            if s.text.is_some() && !is_text {
                return false;
            }
            if is_text && s.text.is_none() {
                // Formstil auf Text: Farben, Deckkraft und Mischmodus übernehmen
                o.style.fill = s.style.fill;
                o.style.gradient = s.style.gradient.clone();
                o.style.stroke = s.style.stroke;
                o.style.width = s.style.width;
                o.style.opacity = s.style.opacity;
                o.style.blend = s.style.blend;
                o.style.fx = s.style.fx;
                o.style.shadow = s.style.shadow;
                o.style.fold_opacity();
                return true;
            }
            o.style = s.style.clone();
            if let Some(t) = &s.text {
                if let Some((_, family, size, bold, italic)) = o.shape.text_fields_mut() {
                    *family = t.family.clone();
                    *size = t.size;
                    *bold = t.bold;
                    *italic = t.italic;
                }
                if let Some(fm) = o.shape.text_format_mut() {
                    *fm = t.fmt;
                }
                // Textstil fürs ganze Feld: Formatierung einzelner Textteile entfällt
                if let Some(r) = o.shape.rich_mut() {
                    r.runs.clear();
                }
            }
        }
    }
    // Deckkraft des Stils in die Farben übernehmen (die Seitenleiste zeigt nur diese)
    o.style.fold_opacity();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Einfache Metriken für Tests: Rahmen aus der Geometrie, Text als 10×10-Kästchen.
    struct M;
    impl Metrics for M {
        fn bbox(&self, o: &Object) -> Option<Rect> {
            match &o.shape {
                Shape::Text { xf, .. } => {
                    let pos = xf.apply(Pt::new(0.0, 0.0));
                    Some(Rect::from_xywh(pos.x, pos.y - 10.0, 10.0, 10.0))
                }
                s => s.geom_bbox(),
            }
        }
        fn hit(&self, o: &Object, p: Pt, tol: f64) -> bool {
            self.bbox(o).map(|b| b.inflate(tol).contains(p)).unwrap_or(false)
        }
        fn fill_area(&self, o: &Object) -> Option<String> {
            match &o.shape {
                // 1 Wort je 1000 px² Fläche
                Shape::TextArea { w, h, .. } => {
                    let n = ((w * h) / 1000.0).max(1.0) as usize;
                    Some(LOREM.split(' ').cycle().take(n).collect::<Vec<_>>().join(" "))
                }
                _ => None,
            }
        }
        fn text_index(&self, _o: &Object, p: Pt) -> Option<usize> {
            Some((p.x.max(0.0) / 10.0) as usize)
        }
        fn text_outline(&self, o: &Object) -> Option<Vec<SubPath>> {
            let b = self.bbox(o)?;
            match (Shape::Rect {
                x: b.x0,
                y: b.y0,
                w: b.width(),
                h: b.height(),
                r: 0.0,
            })
            .to_path()?
            {
                Shape::Path { subpaths, .. } => Some(subpaths),
                _ => None,
            }
        }
    }

    fn editor() -> Editor {
        let mut e = Editor::new(Document::new(500.0, 500.0, None));
        e.view = View {
            zoom: 1.0,
            ox: 0.0,
            oy: 0.0,
        };
        e
    }

    fn area_with(e: &mut Editor, text: &str) -> u64 {
        let id = e.doc.alloc_id();
        e.doc.layers[0].objects.push(Object {
            id,
            shape: Shape::TextArea {
                xf: Affine::translate(10.0, 10.0),
                w: 200.0,
                h: 100.0,
                text: text.into(),
                family: "Sans".into(),
                size: 12.0,
                bold: false,
                italic: false,
                fmt: TextFormat::default(),
                frame: TextFrame::default(),
                rich: RichText::default(),
            },
            style: Style::default(),
            angle: 0.0,
            group: 0,
            locked: false,
            name: String::new(),
        });
        e.selection = vec![id];
        id
    }

    #[test]
    fn rich_text_editing() {
        let mut e = editor();
        let id = area_with(&mut e, "Hallo Welt\nZweiter");
        // „Hallo“ markieren und fett machen
        e.text_edit = Some(TextEdit {
            id,
            caret: 5,
            anchor: 0,
        });
        assert!(e.set_char_fmt("bold", |f| f.bold = true));
        let rich = e.doc.object(id).unwrap().shape.rich().unwrap().clone();
        assert_eq!((rich.runs[0].start, rich.runs[0].end), (0, 5));
        assert!(e.context_info().font.bold);
        // Cursor ohne Markierung: Format gilt fürs Weiterschreiben
        e.text_edit = Some(TextEdit {
            id,
            caret: 10,
            anchor: 10,
        });
        e.set_char_fmt("family", |f| f.family = "Serif".into());
        assert_eq!(e.context_info().font.family, "Serif");
        assert!(e.edit_insert("!"));
        let rich = e.doc.object(id).unwrap().shape.rich().unwrap().clone();
        assert!(rich
            .runs
            .iter()
            .any(|r| r.start == 10 && r.end == 11 && r.style.family.as_deref() == Some("Serif")));
        // Absatz: Aufzählung nur für den zweiten Absatz
        e.text_edit = Some(TextEdit {
            id,
            caret: 14,
            anchor: 14,
        });
        e.set_para_fmt("list", |p| p.list = crate::model::ListStyle::Dot);
        assert_eq!(e.context_info().para.list, crate::model::ListStyle::Dot);
        e.set_para_fmt("indent", |p| p.indent += 1);
        let rich = e.doc.object(id).unwrap().shape.rich().unwrap().clone();
        assert_eq!(
            rich.para(0, crate::model::ListStyle::None).list,
            crate::model::ListStyle::None
        );
        assert_eq!(rich.para(1, crate::model::ListStyle::None).indent, 1);
        // ganzes Feld: uneinheitlich → markiert; Schrift setzen gleicht an
        e.end_text_edit();
        let info = e.context_info();
        assert!(info.font_mixed.bold && info.font_mixed.family);
        assert!(e.set_char_fmt("family", |f| f.family = "Mono".into()));
        let info = e.context_info();
        assert!(!info.font_mixed.family);
        assert!(info.font_mixed.bold);
        assert_eq!(info.font.family, "Mono");
    }

    fn drag(e: &mut Editor, a: (f64, f64), b: (f64, f64)) {
        e.press(Pt::new(a.0, a.1), Mods::default(), false, &M);
        e.motion(Pt::new(b.0, b.1), Mods::default(), &M);
        e.release(Pt::new(b.0, b.1), Mods::default(), &M);
    }

    #[test]
    fn create_move_undo() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (10.0, 10.0), (110.0, 60.0));
        assert_eq!(e.doc.layers[0].objects.len(), 1);
        assert_eq!(e.selection.len(), 1);
        // reiner Klick erzeugt nichts
        e.press(Pt::new(300.0, 300.0), Mods::default(), false, &M);
        e.release(Pt::new(300.0, 300.0), Mods::default(), &M);
        assert_eq!(e.doc.layers[0].objects.len(), 1);

        e.set_tool(Tool::Select);
        drag(&mut e, (50.0, 30.0), (60.0, 40.0));
        match e.doc.layers[0].objects[0].shape {
            Shape::Rect { x, y, w, .. } => {
                assert!((x - 20.0).abs() < 1e-9 && (y - 20.0).abs() < 1e-9 && (w - 100.0).abs() < 1e-9)
            }
            _ => panic!(),
        }
        assert!(e.undo());
        assert!(matches!(e.doc.layers[0].objects[0].shape, Shape::Rect { x, .. } if (x - 10.0).abs() < 1e-9));
        assert!(e.undo());
        assert!(e.doc.layers[0].objects.is_empty());
        assert!(e.redo());
        assert_eq!(e.doc.layers[0].objects.len(), 1);
    }

    #[test]
    fn scale_handle() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 200.0));
        e.set_tool(Tool::Select);
        // unten rechts (Anfasser 4) auf 300,250 ziehen
        drag(&mut e, (200.0, 200.0), (300.0, 250.0));
        match e.doc.layers[0].objects[0].shape {
            Shape::Rect { x, y, w, h, .. } => {
                assert!((x - 100.0).abs() < 1e-9 && (y - 100.0).abs() < 1e-9);
                assert!((w - 200.0).abs() < 1e-9 && (h - 150.0).abs() < 1e-9);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn rubber_band_and_delete() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (10.0, 10.0), (20.0, 20.0));
        drag(&mut e, (100.0, 100.0), (120.0, 120.0));
        e.set_tool(Tool::Select);
        e.deselect();
        drag(&mut e, (0.0, 0.0), (150.0, 150.0));
        assert_eq!(e.selection.len(), 2);
        assert!(e.delete_selection());
        assert!(e.doc.layers[0].objects.is_empty());
    }

    #[test]
    fn group_lock_and_svg() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (10.0, 10.0), (40.0, 40.0));
        drag(&mut e, (200.0, 200.0), (240.0, 240.0));
        drag(&mut e, (100.0, 100.0), (120.0, 120.0));
        let ids: Vec<u64> = e.doc.layers[0].objects.iter().map(|o| o.id).collect();
        e.set_tool(Tool::Select);
        e.selection = vec![ids[0], ids[2]];
        assert_eq!(e.selection_actions(), Some((false, true, false)));
        assert!(e.group_selection());
        // die Gruppe liegt zusammenhängend an der Stelle des obersten Objekts
        let order: Vec<u64> = e.doc.layers[0].objects.iter().map(|o| o.id).collect();
        assert_eq!(order, vec![ids[1], ids[0], ids[2]]);
        assert_eq!(e.selection_actions(), Some((false, false, true)));
        // Klick auf ein Gruppenmitglied wählt die ganze Gruppe
        e.deselect();
        e.press(Pt::new(20.0, 20.0), Mods::default(), false, &M);
        e.release(Pt::new(20.0, 20.0), Mods::default(), &M);
        assert_eq!(e.selection.len(), 2);
        // Gruppe zusammen verschieben
        drag(&mut e, (20.0, 20.0), (30.0, 20.0));
        assert!((M.bbox(e.doc.object(ids[2]).unwrap()).unwrap().x0 - 110.0).abs() < 1e-6);
        // Speichern und Laden erhält Gruppe
        let back = crate::svg::read(&crate::svg::write(&e.doc, false)).unwrap();
        let g: Vec<u64> = back.layers[0].objects.iter().map(|o| o.group).collect();
        assert!(g[0] == 0 && g[1] != 0 && g[1] == g[2]);
        // Duplizieren: neue Gruppe
        assert!(e.duplicate());
        let groups: Vec<u64> = e.doc.layers[0].objects.iter().map(|o| o.group).collect();
        assert_eq!(groups.len(), 5);
        assert!(groups[3] != 0 && groups[3] == groups[4] && groups[3] != groups[1]);
        // Sperren: nicht verschieb- oder löschbar, aber auswählbar
        e.selection = vec![ids[1]];
        assert!(e.set_selection_locked(true));
        e.deselect();
        drag(&mut e, (220.0, 220.0), (260.0, 220.0));
        assert_eq!(e.selection, vec![ids[1]]);
        assert!((M.bbox(e.doc.object(ids[1]).unwrap()).unwrap().x0 - 200.0).abs() < 1e-6);
        assert!(!e.delete_selection());
        assert!(e.handle_positions(&M).is_none());
        let back = crate::svg::read(&crate::svg::write(&e.doc, false)).unwrap();
        assert!(back.layers[0].objects[0].locked);
        assert!(e.set_selection_locked(false));
        assert!(e.delete_selection());
        // Auflösen
        e.selection = vec![ids[0], ids[2]];
        assert!(e.ungroup_selection());
        assert!(e.doc.object(ids[0]).unwrap().group == 0);
    }

    #[test]
    fn alt_drag_copies() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (10.0, 10.0), (40.0, 40.0));
        e.set_tool(Tool::Select);
        let alt = Mods {
            alt: true,
            ..Mods::default()
        };
        e.press(Pt::new(20.0, 20.0), Mods::default(), false, &M);
        e.motion(Pt::new(70.0, 20.0), alt, &M);
        e.release(Pt::new(70.0, 20.0), alt, &M);
        let objs = &e.doc.layers[0].objects;
        assert_eq!(objs.len(), 2);
        assert!((M.bbox(&objs[0]).unwrap().x0 - 10.0).abs() < 1e-6);
        assert!((M.bbox(&objs[1]).unwrap().x0 - 60.0).abs() < 1e-6);
        assert_eq!(e.selection, vec![objs[1].id]);
        // ein Schritt rückgängig: nur noch das Original an alter Stelle
        assert!(e.undo());
        assert_eq!(e.doc.layers[0].objects.len(), 1);
        // Alt während des Ziehens loslassen: keine Kopie
        e.press(Pt::new(20.0, 20.0), Mods::default(), false, &M);
        e.motion(Pt::new(50.0, 20.0), alt, &M);
        e.motion(Pt::new(70.0, 20.0), Mods::default(), &M);
        e.release(Pt::new(70.0, 20.0), Mods::default(), &M);
        assert_eq!(e.doc.layers[0].objects.len(), 1);
    }

    #[test]
    fn rotated_frame_and_radius_limit() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 200.0));
        e.set_tool(Tool::Select);
        assert!(e.set_rotation(45.0, &M));
        let (b, fr) = e.selection_frame(&M).unwrap();
        assert!(!fr.is_identity());
        assert!((b.width() - 100.0).abs() < 1e-6 && (b.height() - 100.0).abs() < 1e-6);
        // rechten Anfasser (Index 3) im gedrehten System nach außen ziehen
        let hs = e.handle_positions(&M).unwrap();
        let out = (hs[3] - hs[7]).normalized();
        let target = hs[3] + out * 50.0;
        drag(&mut e, (hs[3].x, hs[3].y), (target.x, target.y));
        let (b2, _) = e.selection_frame(&M).unwrap();
        assert!((b2.width() - 150.0).abs() < 1e-3, "{}", b2.width());
        assert!((b2.height() - 100.0).abs() < 1e-3, "{}", b2.height());
        assert!((e.selected_objects()[0].angle - 45.0).abs() < 1e-6);
        // Eckenradius bleibt in den Grenzen
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 160.0));
        assert!(e.convert_to_path(&M));
        e.set_tool(Tool::Node);
        let id = e.selection[0];
        e.node_sel = vec![(id, 0, 0)];
        assert!(e.set_node_radius(1000.0));
        let r = match &e.doc.layers[0].objects[0].shape {
            Shape::Path { subpaths, .. } => subpaths[0].nodes[0].r,
            _ => panic!(),
        };
        assert!((r - 60.0).abs() < 1e-6, "{r}");
        e.select_all_nodes();
        e.set_node_radius(1000.0);
        let r = match &e.doc.layers[0].objects[0].shape {
            Shape::Path { subpaths, .. } => subpaths[0].nodes[1].r,
            _ => panic!(),
        };
        assert!((r - 30.0).abs() < 1e-6, "{r}");
    }

    #[test]
    fn layer_slots() {
        let mut e = editor();
        e.add_layer();
        e.add_layer();
        let shown = |e: &Editor| e.doc.layers.iter().rev().map(|l| l.id).collect::<Vec<_>>();
        let before = shown(&e);
        // oberste Ebene ganz nach unten (Platz hinter der letzten Zeile)
        assert!(e.move_layer_to_slot(before[0], 3));
        assert_eq!(shown(&e), vec![before[1], before[2], before[0]]);
        // wieder ganz nach oben
        assert!(e.move_layer_to_slot(before[0], 0));
        assert_eq!(shown(&e), before);
        // Platz direkt über oder unter sich selbst ändert nichts
        assert!(!e.move_layer_to_slot(before[1], 1));
        assert!(!e.move_layer_to_slot(before[1], 2));
    }

    #[test]
    fn rect_radius_drag_grows() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (300.0, 200.0));
        e.set_tool(Tool::Node);
        // alle Ecken gewählt: Radius wächst bis zur halben kurzen Seite
        for r in [5.0, 20.0, 40.0, 80.0] {
            e.set_node_radius(r);
        }
        match e.doc.layers[0].objects[0].shape {
            Shape::Rect { r, .. } => assert!((r - 50.0).abs() < 1e-6, "{r}"),
            ref other => panic!("{other:?}"),
        }
        assert_eq!(e.corner_widgets().len(), 4);
    }

    #[test]
    fn gradient_paints_for_stroke_and_shadow() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 200.0));
        e.set_tool(Tool::Select);
        e.grad_target = PaintTarget::Stroke;
        assert!(e.add_gradient());
        assert!(e.set_stop_color(1, Color::rgb(1.0, 0.0, 0.0)));
        let red = Color::rgb(1.0, 0.0, 0.0);
        let blue = Color::rgb(0.0, 0.0, 1.0);
        assert!(e.set_paint(PaintTarget::Shadow, Some(red), Some(Gradient::new(red, blue))));
        let st = &e.selected_objects()[0].style;
        assert!(st.grad(PaintTarget::Stroke).is_some() && st.grad(PaintTarget::Fill).is_none());
        assert!(st.shadow.is_some() && st.grad(PaintTarget::Shadow).is_some());
        let back = crate::svg::read(&crate::svg::write(&e.doc, false)).unwrap();
        let bs = &back.layers[0].objects[0].style;
        assert!(bs
            .grad(PaintTarget::Stroke)
            .unwrap()
            .same_colors(st.grad(PaintTarget::Stroke).unwrap()));
        assert!(bs
            .grad(PaintTarget::Shadow)
            .unwrap()
            .same_colors(&Gradient::new(red, blue)));
        // Kontur ohne Verlauf: wieder einfarbig mit der ersten Verlaufsfarbe
        e.grad_target = PaintTarget::Stroke;
        assert!(e.remove_gradient());
        assert!(e.selected_objects()[0].style.grad(PaintTarget::Stroke).is_none());
    }

    #[test]
    fn insert_graphic_groups() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (0.0, 0.0), (10.0, 10.0));
        drag(&mut e, (20.0, 0.0), (30.0, 10.0));
        e.select_all();
        let g = e.selected_graphic();
        assert!(e.insert_graphic(&g, Pt::new(200.0, 200.0), &M));
        let objs = &e.doc.layers[0].objects;
        assert_eq!(objs.len(), 4);
        assert!(objs[2].group != 0 && objs[2].group == objs[3].group);
        let b = M.bbox(&objs[2]).unwrap().union(&M.bbox(&objs[3]).unwrap());
        assert!(((b.x0 + b.x1) / 2.0 - 200.0).abs() < 1e-6);
        assert_eq!(e.selection.len(), 2);
    }

    #[test]
    fn outline_stroke_and_alpha() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 160.0));
        drag(&mut e, (300.0, 100.0), (340.0, 160.0));
        e.set_tool(Tool::Select);
        e.select_all();
        // nur die Deckkraft: beide behalten ihre Farbe
        let id0 = e.doc.layers[0].objects[0].id;
        e.selection = vec![id0];
        e.set_paint(PaintTarget::Fill, Some(Color::rgb(1.0, 0.0, 0.0)), None);
        e.select_all();
        assert!(e.set_paint_alpha(PaintTarget::Fill, 0.5));
        let f0 = e.doc.layers[0].objects[0].style.fill.unwrap();
        let f1 = e.doc.layers[0].objects[1].style.fill.unwrap();
        assert!((f0.r - 1.0).abs() < 1e-9 && (f0.a - 0.5).abs() < 1e-9);
        assert!(f1.r < 0.9 && (f1.a - 0.5).abs() < 1e-9);
        // Kontur zur Fläche
        e.selection = vec![id0];
        let w = e.doc.layers[0].objects[0].style.width;
        assert!(e.outline_stroke(&M));
        assert_eq!(e.doc.layers[0].objects.len(), 3);
        let o = &e.doc.layers[0].objects[1];
        assert!(o.style.stroke.is_none() && o.style.fill.is_some());
        let b = M.bbox(o).unwrap();
        assert!((b.width() - (100.0 + w)).abs() < 0.5, "{}", b.width());
        assert!(e.doc.layers[0].objects[0].style.stroke.is_none());
    }

    #[test]
    fn outline_stroke_keeps_corners_and_ends() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 200.0));
        e.set_tool(Tool::Select);
        let id = e.doc.layers[0].objects[0].id;
        e.selection = vec![id];
        {
            let st = &mut e.doc.object_mut(id).unwrap().style;
            st.fill = None;
            st.width = 10.0;
            st.join = crate::model::LineJoin::Miter;
        }
        assert!(e.outline_stroke(&M));
        // spitze Ecken: das Ergebnis füllt die Ecken vollständig aus
        let o = &e.doc.layers[0].objects[0];
        let b = M.bbox(o).unwrap();
        assert!((b.width() - 110.0).abs() < 1e-6, "{b:?}");
        let Shape::Path { subpaths, .. } = &o.shape else {
            panic!("{:?}", o.shape)
        };
        let rings = boolean::to_rings(subpaths, 0.1);
        let pt_in = |x: f64, y: f64| boolean::contains(&rings, Pt::new(x, y));
        assert!(pt_in(96.0, 96.0), "Ecke außen fehlt");
        assert!(!pt_in(150.0, 150.0), "Mitte muss frei bleiben");
        // offene Linie mit geraden Enden: endet genau am Endpunkt
        let mut e = editor();
        e.set_tool(Tool::Pen);
        for (x, y) in [(10.0, 50.0), (110.0, 50.0)] {
            e.press(Pt::new(x, y), Mods::default(), false, &M);
            e.release(Pt::new(x, y), Mods::default(), &M);
        }
        assert!(e.finish_pen(false));
        let id = e.doc.layers[0].objects[0].id;
        e.selection = vec![id];
        {
            let st = &mut e.doc.object_mut(id).unwrap().style;
            st.width = 10.0;
            st.cap = crate::model::LineCap::Butt;
        }
        assert!(e.outline_stroke(&M));
        let b = M.bbox(&e.doc.layers[0].objects[0]).unwrap();
        assert!((b.x0 - 10.0).abs() < 1e-6 && (b.x1 - 110.0).abs() < 1e-6, "{b:?}");
    }

    #[test]
    fn pen_closes() {
        let mut e = editor();
        e.set_tool(Tool::Pen);
        for (x, y) in [(10.0, 10.0), (100.0, 10.0), (100.0, 100.0), (10.5, 10.5)] {
            e.press(Pt::new(x, y), Mods::default(), false, &M);
            e.release(Pt::new(x, y), Mods::default(), &M);
        }
        assert!(e.pen.is_none());
        match &e.doc.layers[0].objects[0].shape {
            Shape::Path { subpaths, .. } => {
                assert!(subpaths[0].closed);
                assert_eq!(subpaths[0].nodes.len(), 3);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn pencil_draws() {
        let mut e = editor();
        e.set_tool(Tool::Pencil);
        e.press(Pt::new(0.0, 0.0), Mods::default(), false, &M);
        for i in 1..50 {
            let t = i as f64;
            e.motion(
                Pt::new(t * 4.0, (t / 5.0).sin() * 30.0 + 50.0),
                Mods::default(),
                &M,
            );
        }
        e.release(Pt::new(200.0, 50.0), Mods::default(), &M);
        let o = &e.doc.layers[0].objects[0];
        assert!(o.style.fill.is_none() && o.style.stroke.is_some());
    }

    #[test]
    fn locked_layer_blocks_creation() {
        let mut e = editor();
        let id = e.active_layer;
        e.set_layer_locked(id, true);
        e.set_tool(Tool::Shape);
        drag(&mut e, (10.0, 10.0), (50.0, 50.0));
        assert!(e.doc.layers[0].objects.is_empty());
        assert!(e.message.is_some());
    }

    #[test]
    fn layers_and_restack() {
        let mut e = editor();
        e.add_layer();
        assert_eq!(e.doc.layers.len(), 2);
        assert_eq!(e.doc.layers[1].id, e.active_layer);
        assert!(e.move_layer(-1));
        assert_eq!(e.doc.layers[0].id, e.active_layer);
        assert!(e.delete_layer());
        assert_eq!(e.doc.layers.len(), 1);
        assert!(!e.delete_layer());

        e.set_tool(Tool::Shape);
        drag(&mut e, (10.0, 10.0), (20.0, 20.0));
        drag(&mut e, (30.0, 30.0), (40.0, 40.0));
        let first = e.doc.layers[0].objects[0].id;
        e.selection = vec![first];
        assert!(e.restack(1, true));
        assert_eq!(e.doc.layers[0].objects[1].id, first);
    }

    #[test]
    fn style_and_text() {
        let mut e = editor();
        e.set_tool(Tool::Text);
        e.press(Pt::new(50.0, 50.0), Mods::default(), false, &M);
        e.release(Pt::new(50.0, 50.0), Mods::default(), &M);
        assert!(e.text_edit.is_some());
        // mehrere Änderungen derselben Art kurz hintereinander = ein Rückgängig-Schritt
        assert!(e.set_text("Hallo"));
        assert!(e.set_text("Hallo Welt"));
        assert!(e.update_text("size", |t| t.size = 72.0));
        let info = e.context_info();
        assert_eq!(info.text.as_deref(), Some("Hallo Welt"));
        assert_eq!(info.font.size, 72.0);
        assert!(e.update_style("fill", |s| s.fill = Some(Color::rgb(1.0, 0.0, 0.0))));
        assert_eq!(e.selected_objects()[0].style.fill.unwrap().to_hex(), "#ff0000");
        assert!(e.undo()); // Farbe
        assert!(e.undo()); // Größe
        assert!(e.undo()); // Text
        assert!(e.undo()); // Anlegen
        assert!(e.doc.layers[0].objects.is_empty());
        assert!(!e.undo());
    }

    #[test]
    fn node_editing() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 200.0));
        assert!(e.convert_to_path(&M));
        e.set_tool(Tool::Node);
        // Nach dem Wechsel zum Knotenwerkzeug sind alle Knoten gewählt (Eckenradius-Punkte da)
        assert_eq!(e.node_sel.len(), 4);
        assert_eq!(e.corner_widgets().len(), 4);
        // Erster Zug an einem nicht gewählten Knoten wählt nur aus
        drag(&mut e, (100.0, 100.0), (80.0, 90.0));
        let id = e.selection[0];
        assert_eq!(e.node_sel, vec![(id, 0, 0)]);
        let first = |e: &Editor| match &e.doc.layers[0].objects[0].shape {
            Shape::Path { subpaths, .. } => subpaths[0].nodes[0].p,
            _ => panic!(),
        };
        assert_eq!(first(&e), Pt::new(100.0, 100.0));
        // Jetzt verschieben
        drag(&mut e, (100.0, 100.0), (80.0, 90.0));
        assert_eq!(first(&e), Pt::new(80.0, 90.0));
        // Winzige Bewegung (< 3 px) verschiebt nichts
        drag(&mut e, (80.0, 90.0), (81.0, 90.0));
        assert_eq!(first(&e), Pt::new(80.0, 90.0));
        assert!(e.node_make_smooth());
        assert!(e.node_delete());
        match &e.doc.layers[0].objects[0].shape {
            Shape::Path { subpaths, .. } => assert_eq!(subpaths[0].nodes.len(), 3),
            _ => panic!(),
        }
    }

    #[test]
    fn nodes_of_several_objects() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 200.0));
        let a = e.selection[0];
        drag(&mut e, (300.0, 100.0), (400.0, 200.0));
        let b = e.selection[0];
        e.deselect();
        e.set_tool(Tool::Node);
        // Rahmen aus dem Leeren über die oberen Ecken beider Formen
        drag(&mut e, (50.0, 50.0), (450.0, 120.0));
        assert_eq!(e.selection.len(), 2);
        assert_eq!(e.node_sel.len(), 4);
        assert!(e.node_sel.iter().any(|n| n.0 == a) && e.node_sel.iter().any(|n| n.0 == b));
        // gemeinsam verschieben: an einem gewählten Knoten ziehen
        drag(&mut e, (300.0, 100.0), (300.0, 80.0));
        let top = |e: &Editor, id: u64| match &e.doc.object(id).unwrap().shape {
            Shape::Path { subpaths, .. } => subpaths[0].nodes[0].p.y,
            Shape::Rect { y, .. } => *y,
            _ => panic!(),
        };
        assert!((top(&e, a) - 80.0).abs() < 1e-9 && (top(&e, b) - 80.0).abs() < 1e-9);
        // gemeinsam runden (je Objekt begrenzt)
        assert!(e.set_node_radius(10.0));
        assert_eq!(e.corner_widgets().len(), 4);
        // ein Schritt rückgängig
        assert!(e.undo());
        assert!(e.undo());
        assert!((top(&e, a) - 100.0).abs() < 1e-9 && (top(&e, b) - 100.0).abs() < 1e-9);
        // Klick auf eine Form wählt nur diese mit allen Knoten
        e.selection = vec![a, b];
        e.select_all_nodes();
        drag(&mut e, (150.0, 150.0), (150.0, 150.0));
        assert_eq!(e.selection, vec![a]);
        assert_eq!(e.node_sel.len(), 4);
        // Umschalt + Klick nimmt eine weitere Form dazu, noch einmal nimmt sie wieder heraus
        let sh = Mods {
            shift: true,
            ..Mods::default()
        };
        e.press(Pt::new(350.0, 150.0), sh, false, &M);
        e.release(Pt::new(350.0, 150.0), sh, &M);
        assert_eq!(e.selection, vec![a, b]);
        assert_eq!(e.node_sel.len(), 8);
        e.press(Pt::new(350.0, 150.0), sh, false, &M);
        e.release(Pt::new(350.0, 150.0), sh, &M);
        assert_eq!(e.selection, vec![a]);
        assert_eq!(e.node_sel.len(), 4);
        // Knoten mehrerer Objekte löschen
        e.selection = vec![a, b];
        e.node_sel = vec![(a, 0, 0), (b, 0, 1)];
        assert!(e.node_delete());
        let count = |e: &Editor, id: u64| match &e.doc.object(id).unwrap().shape {
            Shape::Path { subpaths, .. } => subpaths[0].nodes.len(),
            _ => panic!(),
        };
        assert_eq!((count(&e, a), count(&e, b)), (3, 3));
    }

    #[test]
    fn node_rubber_and_radius() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 200.0));
        assert!(e.convert_to_path(&M));
        e.set_tool(Tool::Node);
        // Rahmen um die oberen beiden Ecken → gemeinsam runden
        drag(&mut e, (90.0, 90.0), (210.0, 110.0));
        assert_eq!(e.node_sel.len(), 2);
        assert!(e.set_node_radius(10.0));
        assert!(e.set_node_radius(20.0));
        let radii = |e: &Editor| match &e.doc.layers[0].objects[0].shape {
            Shape::Path { subpaths, .. } => subpaths[0].nodes.iter().map(|n| n.r).collect::<Vec<_>>(),
            _ => panic!(),
        };
        assert_eq!(radii(&e), vec![20.0, 20.0, 0.0, 0.0]);
        // Gespeicherte Knoten bleiben 4, die gezeichnete Form hat 6
        let eff = e.doc.layers[0].objects[0].shape.to_path().unwrap();
        match eff {
            Shape::Path { subpaths, .. } => assert_eq!(subpaths[0].nodes.len(), 6),
            _ => panic!(),
        }
        // Einzeln: nur eine untere Ecke anders runden
        drag(&mut e, (200.0, 200.0), (200.0, 200.0));
        assert_eq!(e.node_sel, vec![(e.selection[0], 0, 2)]);
        assert!(e.set_node_radius(5.0));
        assert_eq!(radii(&e), vec![20.0, 20.0, 5.0, 0.0]);
        // Eine schon gerundete Ecke später wieder einzeln ändern
        drag(&mut e, (100.0, 100.0), (100.0, 100.0));
        assert!(e.set_node_radius(40.0));
        assert_eq!(radii(&e), vec![40.0, 20.0, 5.0, 0.0]);
        assert_eq!(e.context_info().node_radius, 40.0);
        assert!(e.undo());
        assert_eq!(radii(&e), vec![20.0, 20.0, 5.0, 0.0]);
    }

    #[test]
    fn text_on_path_and_image() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        e.shape_kind = ShapeKind::Ellipse;
        drag(&mut e, (100.0, 100.0), (200.0, 200.0));
        e.set_tool(Tool::TextPath);
        e.press(Pt::new(150.0, 101.0), Mods::default(), false, &M);
        e.release(Pt::new(150.0, 101.0), Mods::default(), &M);
        assert!(matches!(e.doc.layers[0].objects[0].shape, Shape::TextPath { .. }));
        assert!(e.set_text("Rund herum"));
        assert!(e.set_text_offset(12.0));
        assert_eq!(e.context_info().text_offset, Some(12.0));
        assert!(e.insert_image(Rc::new(vec![1, 2, 3]), 2000.0, 1000.0, None));
        let b = e.doc.layers[0].objects[1].shape.geom_bbox().unwrap();
        assert!((b.width() - 400.0).abs() < 1e-6, "{b:?}");
        e.set_artboard(300.0, 300.0, None);
        assert_eq!(e.doc.width, 300.0);
        assert!(e.undo());
        assert_eq!(e.doc.width, 500.0);
    }

    #[test]
    fn text_scales_uniformly() {
        let mut e = editor();
        e.set_tool(Tool::Text);
        e.press(Pt::new(50.0, 50.0), Mods::default(), false, &M);
        e.release(Pt::new(50.0, 50.0), Mods::default(), &M);
        e.set_tool(Tool::Select);
        let b = e.selection_bbox(&M).unwrap();
        // rechten Anfasser nach rechts ziehen → Text wird gleichmäßig größer
        drag(
            &mut e,
            (b.x1, (b.y0 + b.y1) / 2.0),
            (b.x1 + b.width(), (b.y0 + b.y1) / 2.0),
        );
        match &e.doc.layers[0].objects[0].shape {
            Shape::Text { size, xf, .. } => {
                assert!((size - 72.0).abs() < 1e-6, "{size}");
                assert!((xf.a - 1.0).abs() < 1e-9 && (xf.d - 1.0).abs() < 1e-9);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn copy_paste_duplicate() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (10.0, 10.0), (20.0, 20.0));
        assert!(e.copy());
        assert!(e.paste());
        assert!(e.duplicate());
        assert_eq!(e.doc.layers[0].objects.len(), 3);
        let ids: Vec<u64> = e.doc.layers[0].objects.iter().map(|o| o.id).collect();
        assert!(ids[0] != ids[1] && ids[1] != ids[2]);
    }

    #[test]
    fn rotate_flip_boolean() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 140.0));
        e.set_tool(Tool::Select);
        // Drehgriff liegt über der Mitte der oberen Kante
        let h = e.rotate_handle(&M).unwrap();
        assert!((h.x - 150.0).abs() < 1e-9 && (h.y - (100.0 - ROTATE_OFFSET)).abs() < 1e-9);
        // um 90° drehen: Griff nach rechts neben den Mittelpunkt ziehen
        e.press(h, Mods::default(), false, &M);
        e.motion(
            Pt::new(250.0, 120.0),
            Mods {
                shift: true,
                ..Mods::default()
            },
            &M,
        );
        e.release(
            Pt::new(250.0, 120.0),
            Mods {
                shift: true,
                ..Mods::default()
            },
            &M,
        );
        let b = e.selection_bbox(&M).unwrap();
        assert!(
            (b.width() - 40.0).abs() < 1e-6 && (b.height() - 100.0).abs() < 1e-6,
            "{b:?}"
        );
        assert!(e.undo());
        assert!(e.flip_selection(true, &M));
        assert!(e.rotate_selection(-90.0, &M));

        // zweites Objekt, dann vereinigen
        e.set_tool(Tool::Shape);
        drag(&mut e, (150.0, 60.0), (200.0, 200.0));
        e.set_tool(Tool::Select);
        e.select_all();
        assert!(e.boolean(Op::Union, &M));
        assert_eq!(e.doc.layers[0].objects.len(), 1);
        assert!(matches!(e.doc.layers[0].objects[0].shape, Shape::Path { .. }));
        assert!(e.undo());
        assert_eq!(e.doc.layers[0].objects.len(), 2);
        e.select_all();
        assert!(e.boolean(Op::Difference, &M));
        let only = &e.doc.layers[0].objects[0];
        let area = crate::boolean::total_area(match &only.shape {
            Shape::Path { subpaths, .. } => subpaths,
            _ => panic!(),
        });
        // gedrehtes Rechteck x 130–170, y 70–170; Überlappung 20 × 100
        assert!((area - (4000.0 - 2000.0)).abs() < 1e-6, "{area}");
    }

    #[test]
    fn text_becomes_path() {
        let mut e = editor();
        e.set_tool(Tool::Text);
        e.press(Pt::new(50.0, 50.0), Mods::default(), false, &M);
        e.release(Pt::new(50.0, 50.0), Mods::default(), &M);
        assert!(e.flip_selection(false, &M));
        assert!(e.convert_to_path(&M));
        assert!(matches!(e.doc.layers[0].objects[0].shape, Shape::Path { .. }));
    }

    #[test]
    fn nodes_on_shapes_and_fillet_widget() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 200.0));
        e.set_tool(Tool::Node);
        // Knoten eines Rechtecks sind wählbar, ohne vorher umzuwandeln
        drag(&mut e, (100.0, 100.0), (100.0, 100.0));
        assert_eq!(e.node_sel, vec![(e.selection[0], 0, 0)]);
        assert!(matches!(e.doc.layers[0].objects[0].shape, Shape::Rect { .. }));
        // Ziehpunkt liegt auf der Winkelhalbierenden nach innen
        let w = e.corner_widgets();
        assert_eq!(w.len(), 1);
        let (pos, _, off) = w[0];
        assert!(pos.x > 100.0 && pos.y > 100.0);
        // nach innen ziehen → Radius
        let target = Pt::new(100.0 + off + 30.0, 100.0 + off + 30.0);
        e.press(pos, Mods::default(), false, &M);
        e.motion(target, Mods::default(), &M);
        e.release(target, Mods::default(), &M);
        assert!(e.context_info().node_radius > 10.0);
        match &e.doc.layers[0].objects[0].shape {
            Shape::Path { subpaths, .. } => {
                assert_eq!(subpaths[0].nodes.len(), 4);
                assert!(subpaths[0].nodes[0].r > 10.0);
                assert_eq!(subpaths[0].effective().nodes.len(), 5);
            }
            other => panic!("{other:?}"),
        }
        assert!(e.undo());
        assert!(matches!(e.doc.layers[0].objects[0].shape, Shape::Rect { .. }));
    }

    #[test]
    fn curvature_and_textarea() {
        let mut e = editor();
        e.set_tool(Tool::Curvature);
        for (x, y) in [(10.0, 100.0), (60.0, 40.0), (110.0, 100.0)] {
            e.press(Pt::new(x, y), Mods::default(), false, &M);
            e.release(Pt::new(x, y), Mods::default(), &M);
        }
        assert!(!e.curvature_preview().unwrap().1);
        // über dem Startpunkt: sofort die geschlossene Form, genau wie nach dem Klick
        e.motion(Pt::new(10.0, 100.0), Mods::default(), &M);
        let (nodes, closed) = e.curvature_preview().unwrap();
        assert!(closed);
        assert_eq!(nodes, curvature_nodes(e.curv.as_ref().unwrap(), true));
        e.motion(Pt::new(200.0, 200.0), Mods::default(), &M);
        assert!(!e.curvature_preview().unwrap().1);
        assert!(e.finish_drawing());
        match &e.doc.layers[0].objects[0].shape {
            Shape::Path { subpaths, .. } => {
                let mid = subpaths[0].nodes[1];
                assert!(mid.is_smooth());
            }
            _ => panic!(),
        }
        e.set_tool(Tool::TextArea);
        e.press(Pt::new(20.0, 200.0), Mods::default(), false, &M);
        e.release(Pt::new(20.0, 200.0), Mods::default(), &M);
        match &e.doc.layers[0].objects[1].shape {
            Shape::TextArea { w, h, text, .. } => {
                assert_eq!((*w, *h), (320.0, 180.0));
                assert!(text.starts_with("Lorem"));
            }
            _ => panic!(),
        }
        // Textfeld nicht proportional: Breite ändern, Schriftgröße bleibt
        e.set_tool(Tool::Select);
        let size_before = e.doc.layers[0].objects[1].shape.text_fields().unwrap().2;
        drag(&mut e, (340.0, 290.0), (440.0, 290.0));
        match &e.doc.layers[0].objects[1].shape {
            Shape::TextArea { w, h, size, .. } => {
                assert!((*w - 420.0).abs() < 1e-6 && (*h - 180.0).abs() < 1e-6, "{w} {h}");
                assert_eq!(*size, size_before);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn move_to_other_layer() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (10.0, 10.0), (20.0, 20.0));
        drag(&mut e, (30.0, 30.0), (40.0, 40.0));
        let first_layer = e.doc.layers[0].id;
        e.add_layer();
        assert_eq!(e.doc.layers[1].color, 1);
        let second = e.doc.layers[1].id;
        let id = e.doc.layers[0].objects[0].id;
        e.selection = vec![id];
        assert!(e.move_selection_to_layer(second));
        assert_eq!(e.doc.layers[0].objects.len(), 1);
        assert_eq!(e.doc.layers[1].objects[0].id, id);
        assert_eq!(e.selection_layers(), vec![second]);
        assert!(e.undo());
        assert_eq!(e.doc.layer(first_layer).unwrap().objects.len(), 2);
    }

    #[test]
    fn edit_text_in_canvas() {
        let mut e = editor();
        e.set_tool(Tool::Text);
        e.press(Pt::new(50.0, 50.0), Mods::default(), false, &M);
        e.release(Pt::new(50.0, 50.0), Mods::default(), &M);
        // neuer Text ist komplett markiert → Tippen ersetzt ihn
        assert_eq!(e.edit_selected_text().as_deref(), Some("Text"));
        assert!(e.edit_insert("Hallo"));
        assert!(e.edit_insert(" Wält"));
        let text = |e: &Editor| {
            e.doc.layers[0].objects[0]
                .shape
                .text_fields()
                .unwrap()
                .0
                .to_string()
        };
        assert_eq!(text(&e), "Hallo Wält");
        assert!(e.edit_backspace());
        assert_eq!(text(&e), "Hallo Wäl");
        assert!(e.edit_move(Move::WordLeft, false));
        assert!(e.edit_move(Move::Right, true));
        assert_eq!(e.edit_selected_text().as_deref(), Some("W"));
        assert!(e.edit_insert("\nZ"));
        assert_eq!(text(&e), "Hallo \nZäl");
        assert!(e.edit_move(Move::Up, false));
        assert!(e.edit_move(Move::End, false));
        assert!(e.edit_move(Move::Left, true));
        assert!(e.edit_delete());
        assert_eq!(text(&e), "Hallo\nZäl");
        // Tippen ist ein Rückgängig-Schritt, Anlegen ein weiterer
        e.end_text_edit();
        assert!(e.undo());
        assert_eq!(text(&e), "Text");
        assert_eq!(word_at("ab cd", 4), (3, 5));
    }

    #[test]
    fn empty_text_removed_and_area_placeholder() {
        let mut e = editor();
        e.set_tool(Tool::Text);
        e.press(Pt::new(50.0, 50.0), Mods::default(), false, &M);
        e.release(Pt::new(50.0, 50.0), Mods::default(), &M);
        assert!(e.edit_backspace());
        e.escape();
        assert!(e.doc.layers[0].objects.is_empty());

        e.set_tool(Tool::TextArea);
        e.press(Pt::new(0.0, 0.0), Mods::default(), false, &M);
        e.release(Pt::new(0.0, 0.0), Mods::default(), &M);
        let words = |e: &Editor| {
            e.doc.layers[0].objects[0]
                .shape
                .text_fields()
                .unwrap()
                .0
                .split(' ')
                .count()
        };
        assert_eq!(words(&e), 57); // 320 × 180 / 1000
        assert!(e.text_edit.is_some());
        // Größer ziehen → mehr Blindtext
        e.end_text_edit();
        e.set_tool(Tool::Select);
        e.selection = vec![e.doc.layers[0].objects[0].id];
        drag(&mut e, (320.0, 180.0), (640.0, 360.0));
        assert!(words(&e) > 57);
        // Nach eigener Eingabe bleibt der Text
        e.begin_text_edit(e.selection[0], None);
        assert!(e.edit_insert("Eigener Text"));
        e.end_text_edit();
        drag(&mut e, (640.0, 360.0), (700.0, 400.0));
        assert_eq!(
            e.doc.layers[0].objects[0].shape.text_fields().unwrap().0,
            "Eigener Text"
        );
    }

    #[test]
    fn textpath_offset_grip() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        e.shape_kind = ShapeKind::Line;
        drag(&mut e, (100.0, 100.0), (400.0, 100.0));
        e.set_tool(Tool::TextPath);
        e.press(Pt::new(200.0, 100.0), Mods::default(), false, &M);
        e.release(Pt::new(200.0, 100.0), Mods::default(), &M);
        e.set_tool(Tool::Select);
        let (_, g, _) = e.textpath_grip().unwrap();
        assert!(g.dist(Pt::new(100.0, 100.0)) < 1e-6);
        drag(&mut e, (100.0, 100.0), (180.0, 130.0));
        match &e.doc.layers[0].objects[0].shape {
            Shape::TextPath { offset, .. } => assert!((offset - 80.0).abs() < 1e-6, "{offset}"),
            _ => panic!(),
        }
        assert!(e.undo());
    }
    #[test]
    fn rotation_angle_tracked() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 150.0));
        e.set_tool(Tool::Select);
        assert!(e.set_rotation(30.0, &M));
        assert!((e.context_info().angle - 30.0).abs() < 1e-9);
        assert!(e.set_rotation(-45.0, &M));
        assert!((e.context_info().angle + 45.0).abs() < 1e-9);
        // 90° nach rechts (im Uhrzeigersinn) = −90° gegen den Uhrzeigersinn
        assert!(e.rotate_selection(90.0, &M));
        assert!((e.context_info().angle - (-135.0)).abs() < 1e-6);
        assert!(e.flip_selection(true, &M));
        assert!((e.context_info().angle - 135.0).abs() < 1e-6);
    }

    #[test]
    fn gradient_ops_and_rotation() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (300.0, 200.0));
        e.set_tool(Tool::Select);
        assert!(e.add_gradient());
        assert!((e.gradient_angle(&M) - 0.0).abs() < 1e-6);
        assert!(e.set_gradient_angle(90.0, &M));
        assert!((e.gradient_angle(&M) - 90.0).abs() < 1e-6);
        assert!(e.add_stop(0.5));
        assert_eq!(e.grad_stop, 2);
        assert!(e.set_stop_color(2, Color::rgb(1.0, 0.0, 0.0)));
        assert!(e.remove_stop());
        let n = e.selected_objects()[0]
            .style
            .gradient
            .as_ref()
            .unwrap()
            .stops
            .len();
        assert_eq!(n, 2);
        // Objekt um 90° drehen: der Verlauf dreht sich mit
        assert!(e.set_rotation(90.0, &M));
        let a = e.gradient_angle(&M);
        assert!((a.abs() - 180.0).abs() < 1e-6, "{a}");
        // Verlaufsendpunkt im Dokument ziehen
        let (_, a0, b0, _) = e.gradient_line(&M).unwrap();
        let s0 = e.view.to_screen(b0);
        drag(&mut e, (s0.x, s0.y), (s0.x, s0.y + 40.0));
        let (_, a1, b1, _) = e.gradient_line(&M).unwrap();
        assert!(a1.dist(a0) < 1e-6 && (b1.y - b0.y - 40.0).abs() < 1e-6);
        assert!(e.remove_gradient());
        assert!(e.selected_objects()[0].style.gradient.is_none());
    }

    #[test]
    fn guides_create_move_snap() {
        let mut e = editor();
        e.set_tool(Tool::Guide);
        drag(&mut e, (50.0, 120.0), (50.0, 120.0));
        assert_eq!(e.doc.guides.len(), 1);
        assert!(e.doc.guides[0].is_horizontal());
        // Ziehen legt den Winkel fest, Umschalt rastet in 45°-Schritten ein
        let ctrl = Mods {
            shift: true,
            ..Mods::default()
        };
        e.press(Pt::new(200.0, 300.0), ctrl, false, &M);
        e.motion(Pt::new(206.0, 400.0), ctrl, &M);
        e.release(Pt::new(206.0, 400.0), ctrl, &M);
        assert!(e.doc.guides[1].is_vertical(), "{:?}", e.doc.guides[1]);
        assert!((e.doc.guides[1].pos.x - 200.0).abs() < 1e-9);
        // ohne Umschalt: freier Winkel
        e.press(Pt::new(500.0, 500.0), Mods::default(), false, &M);
        e.motion(Pt::new(600.0, 400.0), Mods::default(), &M);
        e.release(Pt::new(600.0, 400.0), Mods::default(), &M);
        assert!((e.doc.guides[2].angle - 45.0).abs() < 1e-6);
        e.undo();
        assert_eq!(e.doc.guides.len(), 2);
        // Waagerechte verschieben
        drag(&mut e, (400.0, 120.0), (400.0, 150.0));
        assert!((e.doc.guides[0].pos.y - 150.0).abs() < 1e-9);
        // Rechteck rastet am Schnittpunkt ein
        e.set_tool(Tool::Shape);
        drag(&mut e, (197.0, 147.0), (260.0, 220.0));
        let b = e.selected_objects()[0].shape.geom_bbox().unwrap();
        assert!((b.x0 - 200.0).abs() < 1e-9 && (b.y0 - 150.0).abs() < 1e-9);
        // Verschieben rastet die Kante ein
        e.set_tool(Tool::Select);
        drag(&mut e, (230.0, 190.0), (234.0, 190.0));
        let b = e.selected_objects()[0].shape.geom_bbox().unwrap();
        assert!((b.x0 - 200.0).abs() < 1e-9, "{b:?}");
        // Löschen
        e.set_tool(Tool::Guide);
        e.guide_sel = Some(0);
        assert!(e.delete_selection());
        assert_eq!(e.doc.guides.len(), 1);
        e.undo();
        assert_eq!(e.doc.guides.len(), 2);
        // Aus dem Lineal ziehen
        e.ruler = 20.0;
        drag(&mut e, (100.0, 5.0), (100.0, 80.0));
        assert_eq!(e.doc.guides.len(), 3);
        assert!((e.doc.guides[2].pos.y - 80.0).abs() < 1e-9);
        // zurück ins Lineal = löschen
        drag(&mut e, (100.0, 80.0), (100.0, 5.0));
        assert_eq!(e.doc.guides.len(), 2);
    }

    #[test]
    fn text_format_updates() {
        let mut e = editor();
        e.set_tool(Tool::Text);
        drag(&mut e, (50.0, 50.0), (50.0, 50.0));
        e.end_text_edit();
        assert!(e.set_text_align(Align::Center));
        e.update_text("line", |t| t.fmt.line = 1.5);
        let f = e.selected_objects()[0].shape.text_format().unwrap();
        assert_eq!(f.align, Align::Center);
        assert!((f.line - 1.5).abs() < 1e-9);
        // Schriftgröße ändern lässt das Format unverändert
        e.update_text("size", |t| t.size = 20.0);
        let f = e.selected_objects()[0].shape.text_format().unwrap();
        assert_eq!(f.align, Align::Center);
    }
    #[test]
    fn image_into_frame_and_content_mode() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 200.0));
        let frame = e.selection[0];
        e.set_tool(Tool::Select);
        assert!(e.insert_image(Rc::new(vec![1, 2, 3]), 40.0, 20.0, Some(Pt::new(400.0, 400.0))));
        let img = e.selection[0];
        // über die Form ziehen und halten
        e.press(Pt::new(400.0, 400.0), Mods::default(), false, &M);
        e.motion(Pt::new(150.0, 150.0), Mods::default(), &M);
        assert_eq!(e.frame_progress().map(|x| x.0), Some(frame));
        assert!(e.frame_ready().is_none());
        if let Some((t, _, a)) = e.frame_hover {
            e.frame_hover = Some((t, Instant::now() - Duration::from_secs(3), a));
        }
        e.release(Pt::new(150.0, 150.0), Mods::default(), &M);
        assert!(e.doc.object(frame).is_none(), "Form wird zum Rahmen");
        let o = e.doc.object(img).unwrap();
        let Shape::Image { clip, xf, .. } = &o.shape else {
            panic!()
        };
        assert!(clip.is_some());
        // Bild füllt den Rahmen (100×100 ⇒ Skalierung 5)
        assert!((xf.a - 5.0).abs() < 1e-9);
        assert_eq!(e.image_part(img), ImagePart::Content);
        // Inhalt verschieben: Rahmen bleibt
        let before = M.bbox(e.doc.object(img).unwrap()).unwrap();
        drag(&mut e, (150.0, 150.0), (170.0, 150.0));
        let o = e.doc.object(img).unwrap();
        assert_eq!(M.bbox(o).unwrap(), before);
        let Shape::Image { xf, .. } = &o.shape else {
            panic!()
        };
        assert!((xf.e - (150.0 - 100.0 + 20.0)).abs() < 1e-9, "{xf:?}");
        // Escape: ganzes Objekt, Verschieben bewegt beides
        e.escape();
        drag(&mut e, (150.0, 150.0), (160.0, 150.0));
        let b = M.bbox(e.doc.object(img).unwrap()).unwrap();
        assert!((b.x0 - before.x0 - 10.0).abs() < 1e-9);
        // Rahmen lösen
        assert!(e.release_frame());
        assert!(matches!(
            e.doc.object(img).unwrap().shape,
            Shape::Image { clip: None, .. }
        ));
    }

    #[test]
    fn crop_image_frame_mode() {
        let mut e = editor();
        assert!(e.insert_image(Rc::new(vec![0]), 100.0, 50.0, Some(Pt::new(200.0, 200.0))));
        let img = e.selection[0];
        assert!(e.crop_image());
        assert_eq!(e.image_part(img), ImagePart::Frame);
        // Rahmen verkleinern, Bild bleibt gleich
        let a = Affine::scale_about(0.5, 1.0, Pt::new(150.0, 175.0));
        e.transform_object(img, &a, &M);
        let o = e.doc.object(img).unwrap();
        let b = M.bbox(o).unwrap();
        assert!((b.width() - 50.0).abs() < 1e-9);
        let q = o.shape.image_quad().unwrap();
        assert!((q[1].x - q[0].x - 100.0).abs() < 1e-9);
    }
    #[test]
    fn framed_image_keeps_editable_radius() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 200.0));
        let frame = e.selection[0];
        e.set_tool(Tool::Select);
        assert!(e.set_corner_radius(12.0));
        assert!(e.insert_image(Rc::new(vec![1]), 40.0, 20.0, Some(Pt::new(400.0, 400.0))));
        let img = e.selection[0];
        assert!(e.place_in_frame(img, frame, &M));
        let clip = match &e.doc.object(img).unwrap().shape {
            Shape::Image { clip: Some(c), .. } => c.clone(),
            _ => panic!(),
        };
        assert_eq!(clip[0].nodes.len(), 4);
        assert!(clip[0].nodes.iter().all(|n| (n.r - 12.0).abs() < 1e-9));
        // Knotenwerkzeug: Radius aller Ecken ändern
        e.set_tool(Tool::Node);
        assert!(e.node_view().is_some());
        assert!(e.set_node_radius(30.0));
        let clip = match &e.doc.object(img).unwrap().shape {
            Shape::Image { clip: Some(c), .. } => c.clone(),
            _ => panic!(),
        };
        assert!(clip[0].nodes.iter().all(|n| (n.r - 30.0).abs() < 1e-9));
        // SVG behält die Radien
        let r = crate::svg::read(&crate::svg::write(&e.doc, true)).unwrap();
        match &r.layers[0].objects[0].shape {
            Shape::Image { clip: Some(c), .. } => assert!((c[0].nodes[0].r - 30.0).abs() < 1e-9),
            other => panic!("{other:?}"),
        }
        assert_eq!(e.context_info().title, "Bild mit Rahmen");
        assert!(e.context_info().show_stroke && !e.context_info().show_fill);
    }
    #[test]
    fn pencil_closes_and_smooths() {
        let mut e = editor();
        e.set_tool(Tool::Pencil);
        // fast geschlossener, zittriger Kreis
        let mut pts = Vec::new();
        for i in 0..=60 {
            let t = i as f64 / 60.0 * std::f64::consts::TAU * 0.98;
            let wobble = if i % 2 == 0 { 2.0 } else { -2.0 };
            pts.push(Pt::new(
                200.0 + (80.0 + wobble) * t.cos(),
                200.0 + (80.0 + wobble) * t.sin(),
            ));
        }
        e.press(pts[0], Mods::default(), false, &M);
        for p in &pts[1..] {
            e.motion(*p, Mods::default(), &M);
        }
        e.release(*pts.last().unwrap(), Mods::default(), &M);
        let o = e.selected_objects()[0].clone();
        match &o.shape {
            Shape::Path { subpaths, .. } => {
                assert!(subpaths[0].closed, "Anfang und Ende nah: geschlossen");
                assert!(
                    subpaths[0].nodes.len() < 30,
                    "geglättet: {}",
                    subpaths[0].nodes.len()
                );
            }
            _ => panic!(),
        }
        assert!(o.style.fill.is_some());
        // weit offen: bleibt offen, Füllung wird trotzdem angeboten
        drag(&mut e, (10.0, 10.0), (300.0, 40.0));
        assert!(e.selected_objects()[0].shape.is_open());
        let info = e.context_info();
        assert!(info.show_fill && info.show_markers);
    }
    #[test]
    fn textarea_into_shape() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        e.shape_kind = ShapeKind::Ellipse;
        drag(&mut e, (100.0, 100.0), (300.0, 260.0));
        let shape_id = e.selection[0];
        e.set_tool(Tool::TextArea);
        drag(&mut e, (350.0, 350.0), (450.0, 420.0));
        e.end_text_edit();
        let ta = e.selection[0];
        e.set_tool(Tool::Select);
        e.press(Pt::new(400.0, 380.0), Mods::default(), false, &M);
        e.motion(Pt::new(200.0, 180.0), Mods::default(), &M);
        if let Some((t, _, a)) = e.frame_hover {
            assert_eq!(t, shape_id);
            e.frame_hover = Some((t, Instant::now() - Duration::from_secs(3), a));
        }
        e.release(Pt::new(200.0, 180.0), Mods::default(), &M);
        assert!(e.doc.object(shape_id).is_none());
        match &e.doc.object(ta).unwrap().shape {
            Shape::TextArea { w, h, frame, .. } => {
                assert!((w - 200.0).abs() < 1e-6 && (h - 160.0).abs() < 1e-6);
                assert!(frame.path.is_some() && frame.fill.is_some() && frame.inset >= 8.0);
            }
            _ => panic!(),
        }
        assert!(e.set_text_frame("inset", |f| f.inset = 20.0, &M));
        assert!(e.release_text_shape());
        assert_eq!(e.doc.layers[0].objects.len(), 2);
    }

    #[test]
    fn pen_shift_constrains() {
        let mut e = editor();
        e.smart_snap = false;
        e.set_tool(Tool::Pen);
        let ctrl = Mods {
            shift: true,
            ..Mods::default()
        };
        e.press(Pt::new(10.0, 10.0), Mods::default(), false, &M);
        e.release(Pt::new(10.0, 10.0), Mods::default(), &M);
        // fast waagerecht → exakt waagerecht
        e.press(Pt::new(110.0, 17.0), ctrl, false, &M);
        e.release(Pt::new(110.0, 17.0), ctrl, &M);
        // fast 45° → 45°
        e.press(Pt::new(160.0, 63.0), ctrl, false, &M);
        e.release(Pt::new(160.0, 63.0), ctrl, &M);
        let nodes = e.pen.as_ref().unwrap();
        assert!((nodes[1].p.y - 10.0).abs() < 1e-9, "{:?}", nodes[1].p);
        let d = nodes[2].p - nodes[1].p;
        assert!((d.x - d.y).abs() < 1e-9, "{d:?}");
    }

    #[test]
    fn pen_shift_right_angle_to_previous() {
        let mut e = editor();
        e.smart_snap = false;
        e.set_tool(Tool::Pen);
        let ctrl = Mods {
            shift: true,
            ..Mods::default()
        };
        // schräges erstes Segment (30°)
        let a = Pt::new(100.0, 100.0);
        let b = a + Pt::new(30f64.to_radians().cos(), 30f64.to_radians().sin()) * 100.0;
        for q in [a, b] {
            e.press(q, Mods::default(), false, &M);
            e.release(q, Mods::default(), &M);
        }
        // ungefähr rechtwinklig dazu
        let c = b + Pt::new(-0.45, 0.89) * 80.0;
        e.press(c, ctrl, false, &M);
        e.release(c, ctrl, &M);
        let nodes = e.pen.as_ref().unwrap();
        let d1 = nodes[1].p - nodes[0].p;
        let d2 = nodes[2].p - nodes[1].p;
        assert!((d1.x * d2.x + d1.y * d2.y).abs() < 1e-6, "{d2:?}");
    }

    #[test]
    fn smart_snap_to_object_points() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 160.0));
        e.deselect();
        // neues Rechteck startet nahe der Ecke des ersten → rastet ein
        drag(&mut e, (203.0, 163.0), (300.0, 300.0));
        let b = e.selected_objects()[0].shape.geom_bbox().unwrap();
        assert!(
            (b.x0 - 200.0).abs() < 1e-9 && (b.y0 - 160.0).abs() < 1e-9,
            "{b:?}"
        );
        // ohne intelligentes Einrasten nicht
        e.smart_snap = false;
        e.deselect();
        drag(&mut e, (103.0, 403.0), (150.0, 450.0));
        let b = e.selected_objects()[0].shape.geom_bbox().unwrap();
        assert!((b.x0 - 103.0).abs() < 1e-9);
        // Raster
        e.grid_snap = true;
        e.grid_size = 20.0;
        e.deselect();
        drag(&mut e, (318.0, 22.0), (377.0, 61.0));
        let b = e.selected_objects()[0].shape.geom_bbox().unwrap();
        assert!((b.x0 - 320.0).abs() < 1e-9 && (b.y1 - 60.0).abs() < 1e-9, "{b:?}");
    }

    #[test]
    fn snap_targets_can_be_switched_off_one_by_one() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 160.0));
        e.deselect();
        // Ecken aus: der Startpunkt nahe der Ecke bleibt, wo er ist
        e.snap_corners = false;
        e.snap_mids = false;
        e.snap_page = false;
        drag(&mut e, (203.0, 163.0), (300.0, 300.0));
        let b = e.selected_objects()[0].shape.geom_bbox().unwrap();
        assert!((b.x0 - 203.0).abs() < 1e-9, "{b:?}");
        // Ecken wieder an: rastet ein
        e.snap_corners = true;
        e.deselect();
        drag(&mut e, (97.0, 97.0), (50.0, 50.0));
        let b = e.selected_objects()[0].shape.geom_bbox().unwrap();
        assert!((b.x1 - 100.0).abs() < 1e-9, "{b:?}");
    }

    #[test]
    fn smart_snap_move_aligns_centers() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 200.0));
        e.deselect();
        drag(&mut e, (300.0, 330.0), (340.0, 370.0));
        let id = e.selection[0];
        e.set_tool(Tool::Select);
        // Mitte des zweiten (320/350) nahe an x = 150 bringen → senkrechte Mitten fluchten
        drag(&mut e, (320.0, 350.0), (153.0, 352.0));
        let b = e.doc.object(id).unwrap().shape.geom_bbox().unwrap();
        assert!(((b.x0 + b.x1) / 2.0 - 150.0).abs() < 1e-9, "{b:?}");
        assert!(e.hints.borrow().is_empty());
    }

    #[test]
    fn align_selection_to_page() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 160.0));
        e.set_tool(Tool::Select);
        assert!(e.align_to_page(Some(0), Some(0), &M));
        let b = e.selection_bbox(&M).unwrap();
        assert!(((b.x0 + b.x1) / 2.0 - 250.0).abs() < 1e-9);
        assert!(((b.y0 + b.y1) / 2.0 - 250.0).abs() < 1e-9);
        assert!(!e.align_to_page(Some(0), Some(0), &M));
        assert!(e.align_to_page(Some(1), Some(-1), &M));
        let b = e.selection_bbox(&M).unwrap();
        assert!((b.x1 - 500.0).abs() < 1e-9 && b.y0.abs() < 1e-9);
        assert!(e.undo());
    }

    fn rect_at(e: &mut Editor, x: f64, y: f64, w: f64, h: f64) -> u64 {
        let id = e.doc.alloc_id();
        e.doc.layers[0].objects.push(Object {
            id,
            shape: Shape::Rect { x, y, w, h, r: 0.0 },
            style: Style::default(),
            angle: 0.0,
            group: 0,
            locked: false,
            name: String::new(),
        });
        id
    }

    #[test]
    fn equal_gap_snapping_function() {
        let r = |x0: f64, x1: f64| Rect {
            x0,
            y0: 0.0,
            x1,
            y1: 10.0,
        };
        let boxes = [r(0.0, 10.0), r(30.0, 40.0), r(60.0, 70.0)];
        // Reihe fortsetzen: Lücke 17 rastet auf die vorhandenen 20 ein
        let (corr, gaps) = equal_gap_snap(&r(87.0, 97.0), &boxes, true, 6.0).unwrap();
        assert!((corr - 3.0).abs() < 1e-9, "{corr}");
        assert_eq!(gaps.len(), 2);
        // zu weit von jeder vorhandenen Lücke entfernt
        assert!(equal_gap_snap(&r(80.0, 90.0), &boxes, true, 6.0).is_none());
        // links und rechts gleich groß (Lücken 18 und 22 → je 20)
        let two = [r(0.0, 10.0), r(60.0, 70.0)];
        let (corr, _) = equal_gap_snap(&r(28.0, 38.0), &two, true, 6.0).unwrap();
        assert!((corr - 2.0).abs() < 1e-9, "{corr}");
        // andere Reihe (kein Überlappen auf der anderen Achse): nichts
        let far = Rect {
            x0: 28.0,
            y0: 100.0,
            x1: 38.0,
            y1: 110.0,
        };
        assert!(equal_gap_snap(&far, &two, true, 6.0).is_none());
        // senkrecht dasselbe
        let v = |y0: f64, y1: f64| Rect {
            x0: 0.0,
            y0,
            x1: 10.0,
            y1,
        };
        let (corr, _) = equal_gap_snap(&v(28.0, 38.0), &[v(0.0, 10.0), v(60.0, 70.0)], false, 6.0).unwrap();
        assert!((corr - 2.0).abs() < 1e-9, "{corr}");
    }

    #[test]
    fn move_snaps_to_equal_gaps() {
        // a (0–40) und b (120–160) in einer Reihe; c (Breite 40) kommt von unten
        let setup = |spacing: bool| {
            let mut e = editor();
            let _a = rect_at(&mut e, 0.0, 0.0, 40.0, 40.0);
            let _b = rect_at(&mut e, 120.0, 0.0, 40.0, 40.0);
            let c = rect_at(&mut e, 55.0, 200.0, 40.0, 40.0);
            e.snap_spacing = spacing;
            e.selection = vec![c];
            e.set_tool(Tool::Select);
            (e, c)
        };
        // 3 px neben der Mitte (x0 = 63 statt 60): die Lücken werden links und rechts gleich
        let (mut e, c) = setup(true);
        drag(&mut e, (75.0, 220.0), (83.0, 20.0));
        let bx = e.doc.object(c).unwrap().shape.geom_bbox().unwrap();
        assert!((bx.x0 - 60.0).abs() < 1e-9, "{bx:?}");
        // abgeschaltet: keine Korrektur
        let (mut e, c) = setup(false);
        drag(&mut e, (75.0, 220.0), (83.0, 20.0));
        let bx = e.doc.object(c).unwrap().shape.geom_bbox().unwrap();
        assert!((bx.x0 - 63.0).abs() < 1e-9, "{bx:?}");
    }

    #[test]
    fn distribute_equal_gaps() {
        let mut e = editor();
        let a = rect_at(&mut e, 0.0, 0.0, 10.0, 10.0);
        let b = rect_at(&mut e, 20.0, 0.0, 10.0, 10.0);
        let c = rect_at(&mut e, 100.0, 0.0, 10.0, 10.0);
        e.selection = vec![a, b, c];
        assert!(e.distribute(true, &M));
        let bx = e.doc.object(b).unwrap().shape.geom_bbox().unwrap();
        assert!((bx.x0 - 50.0).abs() < 1e-9, "{bx:?}");
        // schon gleichmäßig: nichts mehr zu tun
        assert!(!e.distribute(true, &M));
        // mit zwei Objekten gibt es nichts zu verteilen
        e.selection = vec![a, b];
        assert!(!e.distribute(true, &M));
        // senkrecht
        let d = rect_at(&mut e, 0.0, 200.0, 10.0, 10.0);
        let f = rect_at(&mut e, 0.0, 300.0, 10.0, 10.0);
        e.selection = vec![a, d, f];
        assert!(e.distribute(false, &M));
        let by = e.doc.object(d).unwrap().shape.geom_bbox().unwrap();
        assert!((by.y0 - 150.0).abs() < 1e-9, "{by:?}");
    }

    #[test]
    fn image_filters_and_tracing() {
        use crate::model::{Art, ImgFilter};
        let mut e = editor();
        assert!(e.insert_image(Rc::new(vec![1, 2, 3]), 100.0, 50.0, Some(Pt::new(200.0, 100.0))));
        let img = e.selection[0];
        let r = rect_at(&mut e, 0.0, 0.0, 10.0, 10.0);
        // nur das Bild bekommt den Farbfilter
        e.selection = vec![img, r];
        assert_eq!(e.selection_image_kinds(), (true, true));
        assert!(e.update_objects(
            "tint",
            |o| matches!(o.shape, Shape::Image { .. }),
            |s| s.fx.tint = Some((ImgFilter::Sepia, 1.0))
        ));
        assert!(e.doc.object(img).unwrap().style.fx.tint.is_some());
        assert!(e.doc.object(r).unwrap().style.fx.tint.is_none());
        // Vorgaben für neue Objekte bleiben unberührt
        assert!(e.defaults.fx.tint.is_none());
        // Kunsteffekte gehen auch beim Bild
        e.update_style("art", |s| s.fx.art = Some((Art::Pixel, 0.5)));
        assert!(e.doc.object(img).unwrap().style.fx.art.is_some());
        // Nachzeichnen: Bild wird durch gruppierte Pfade an gleicher Stelle ersetzt
        e.selection = vec![img];
        let (id, _) = e.traceable_image().expect("Bild ohne Rahmen");
        let sq = |x0: f64, y0: f64, x1: f64, y1: f64| SubPath {
            closed: true,
            nodes: [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
                .iter()
                .map(|&(x, y)| Node {
                    r: 0.0,
                    p: Pt::new(x, y),
                    h_in: Pt::new(x, y),
                    h_out: Pt::new(x, y),
                })
                .collect(),
        };
        let layers = vec![
            crate::trace::Layer {
                color: [255, 0, 0],
                paths: vec![sq(0.0, 0.0, 10.0, 5.0)],
            },
            crate::trace::Layer {
                color: [0, 0, 255],
                paths: vec![sq(2.0, 1.0, 4.0, 3.0)],
            },
        ];
        let before = e.doc.layers[0].objects.len();
        assert!(e.replace_with_traced(id, layers, 10, 5));
        let objs = &e.doc.layers[0].objects;
        assert_eq!(objs.len(), before + 1);
        assert!(e.doc.object(img).is_none());
        assert_eq!(e.selection.len(), 2);
        let b = e.doc.object(e.selection[0]).unwrap();
        assert!(b.group != 0 && e.doc.object(e.selection[1]).unwrap().group == b.group);
        let bb = b.shape.geom_bbox().unwrap();
        // 10 × 5 Pixel füllen das 100 × 50 große Bild, das um (200, 100) zentriert war
        assert!(
            (bb.width() - 100.0).abs() < 1e-6 && (bb.height() - 50.0).abs() < 1e-6,
            "{bb:?}"
        );
        assert!(
            (bb.x0 - 150.0).abs() < 1e-6 && (bb.y0 - 75.0).abs() < 1e-6,
            "{bb:?}"
        );
        assert!(b.style.fx.art.is_none() && b.style.fx.tint.is_none());
        assert!(e.undo());
        assert!(e.doc.object(img).is_some());
    }

    #[test]
    fn blend_between_two_shapes() {
        let mut e = editor();
        let a = rect_at(&mut e, 0.0, 0.0, 10.0, 10.0);
        let b = rect_at(&mut e, 100.0, 0.0, 10.0, 10.0);
        e.doc.object_mut(a).unwrap().style.fill = Some(Color::rgb(1.0, 0.0, 0.0));
        e.doc.object_mut(b).unwrap().style.fill = Some(Color::rgb(0.0, 0.0, 1.0));
        e.selection = vec![a, b];
        assert!(e.blend_selected(3));
        let objs = &e.doc.layers[0].objects;
        assert_eq!(objs.len(), 5);
        // unten die erste Form, dann drei Zwischenformen, oben die zweite
        assert_eq!(objs[0].id, a);
        assert_eq!(objs[4].id, b);
        let mid = objs[2].shape.geom_bbox().unwrap();
        assert!((mid.x0 - 50.0).abs() < 1e-6, "{mid:?}");
        let c = objs[2].style.fill.unwrap();
        assert!((c.r - 0.5).abs() < 1e-9 && (c.b - 0.5).abs() < 1e-9);
        // alle fünf bilden eine Gruppe
        assert!(objs[0].group != 0 && objs.iter().all(|o| o.group == objs[0].group));
        assert_eq!(e.selection.len(), 5);
        assert!(e.undo());
        assert_eq!(e.doc.layers[0].objects.len(), 2);
        // ungleiche Knotenzahl: abgerundetes (8 Knoten) und eckiges Rechteck (4 Knoten)
        let mut e = editor();
        let a = rect_at(&mut e, 0.0, 0.0, 10.0, 10.0);
        let b = rect_at(&mut e, 100.0, 0.0, 20.0, 20.0);
        if let Shape::Rect { r, .. } = &mut e.doc.object_mut(b).unwrap().shape {
            *r = 4.0;
        }
        e.selection = vec![a, b];
        assert!(e.blend_selected(1));
        let objs = &e.doc.layers[0].objects;
        match &objs[1].shape {
            Shape::Path { subpaths, .. } => assert_eq!(subpaths[0].nodes.len(), 8),
            s => panic!("{s:?}"),
        }
        // nur genau zwei Objekte
        e.selection = vec![a];
        assert!(!e.blend_selected(2));
    }

    #[test]
    fn textarea_frame_preview() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (300.0, 260.0));
        let shape_id = e.selection[0];
        e.set_tool(Tool::TextArea);
        drag(&mut e, (350.0, 350.0), (450.0, 420.0));
        e.end_text_edit();
        let ta = e.selection[0];
        let o = e.frame_preview(ta, shape_id, &M).unwrap();
        match &o.shape {
            Shape::TextArea { xf, w, h, frame, .. } => {
                let o = xf.apply(Pt::new(0.0, 0.0));
                assert!((o.x - 100.0).abs() < 1e-6 && (o.y - 100.0).abs() < 1e-6);
                assert!((w - 200.0).abs() < 1e-6 && (h - 160.0).abs() < 1e-6);
                assert!(frame.path.is_some());
            }
            _ => panic!(),
        }
        // Vorschau ändert nichts am Dokument
        assert!(e.doc.object(shape_id).is_some());
        assert_eq!(e.doc.layers[0].objects.len(), 2);
    }

    #[test]
    fn guide_almost_horizontal_snaps_flat() {
        let mut e = editor();
        e.set_tool(Tool::Guide);
        e.press(Pt::new(100.0, 100.0), Mods::default(), false, &M);
        e.motion(Pt::new(300.0, 106.0), Mods::default(), &M);
        e.release(Pt::new(300.0, 106.0), Mods::default(), &M);
        assert!(e.doc.guides[0].is_horizontal(), "{:?}", e.doc.guides[0]);
    }

    #[test]
    fn move_snaps_to_angled_guide_and_anchor() {
        let mut e = editor();
        e.smart_snap = false;
        e.doc.guides.push(Guide {
            pos: Pt::new(0.0, 0.0),
            angle: -45.0,
            layer: 0,
        });
        e.set_tool(Tool::Shape);
        drag(&mut e, (300.0, 100.0), (340.0, 140.0));
        let id = e.selection[0];
        e.set_tool(Tool::Select);
        // obere linke Ecke (300/100) Richtung Diagonale y = x schieben
        drag(&mut e, (320.0, 120.0), (220.0, 216.0));
        let b = e.doc.object(id).unwrap().shape.geom_bbox().unwrap();
        let near = [(b.x0, b.y0), (b.x1, b.y0), (b.x0, b.y1), (b.x1, b.y1)]
            .iter()
            .any(|(x, y)| (x - y).abs() < 1e-6);
        assert!(near, "{b:?}");
        assert!(e.hints.borrow().is_empty());
        // Ankerpunkt eines Pfades rastet an einer senkrechten Hilfslinie ein
        e.doc.guides.clear();
        e.doc.guides.push(Guide {
            pos: Pt::new(250.0, 0.0),
            angle: 90.0,
            layer: 0,
        });
        e.set_tool(Tool::Pen);
        for q in [
            Pt::new(400.0, 300.0),
            Pt::new(460.0, 330.0),
            Pt::new(420.0, 380.0),
        ] {
            e.press(q, Mods::default(), false, &M);
            e.release(q, Mods::default(), &M);
        }
        e.finish_pen(false);
        let pid = e.doc.layers[0].objects.last().unwrap().id;
        e.set_tool(Tool::Select);
        e.selection = vec![pid];
        // mittlerer Punkt (460) auf x ≈ 253 schieben
        drag(&mut e, (460.0, 330.0), (253.0, 330.0));
        match &e.doc.object(pid).unwrap().shape {
            Shape::Path { subpaths, .. } => {
                assert!(subpaths[0].nodes.iter().any(|n| (n.p.x - 250.0).abs() < 1e-6));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn guide_wins_over_smart_snap() {
        let mut e = editor();
        e.doc.guides.push(Guide {
            pos: Pt::new(0.0, 0.0),
            angle: -45.0,
            layer: 0,
        });
        e.set_tool(Tool::Shape);
        // weiteres Objekt, dessen Kanten sonst einrasten würden
        drag(&mut e, (100.0, 300.0), (150.0, 350.0));
        e.deselect();
        drag(&mut e, (300.0, 100.0), (340.0, 140.0));
        let id = e.selection[0];
        e.set_tool(Tool::Select);
        // linke untere Ecke (300/140) landet bei (248/246): 1,4 px neben der Diagonale,
        // x liegt zugleich nahe der Kante 150 nicht – y nahe 250 nicht
        drag(&mut e, (320.0, 120.0), (268.0, 222.0));
        let b = e.doc.object(id).unwrap().shape.geom_bbox().unwrap();
        assert!((b.x0 - b.y1).abs() < 1e-6, "{b:?}");
        // weiter entlang gezogen bleibt sie an der Linie hängen
        drag(
            &mut e,
            (b.x0 + 20.0, b.y0 + 20.0),
            (b.x0 + 20.0 + 30.0, b.y0 + 20.0 + 37.0),
        );
        let b = e.doc.object(id).unwrap().shape.geom_bbox().unwrap();
        assert!((b.x0 - b.y1).abs() < 1e-6, "{b:?}");
    }

    #[test]
    fn app_like_guide_snap() {
        let mut e = editor();
        e.view = View {
            zoom: 0.8,
            ox: 57.0,
            oy: 33.0,
        };
        // Hilfslinie mit dem Werkzeug schräg aufziehen
        e.set_tool(Tool::Guide);
        e.press(Pt::new(150.0, 40.0), Mods::default(), false, &M);
        for k in 1..=20 {
            let t = k as f64 / 20.0;
            e.motion(Pt::new(150.0 + 300.0 * t, 40.0 + 300.0 * t), Mods::default(), &M);
        }
        e.release(Pt::new(450.0, 340.0), Mods::default(), &M);
        let g = e.doc.guides[0];
        // gedrehtes Rechteck
        e.set_tool(Tool::Shape);
        drag(&mut e, (500.0, 100.0), (560.0, 160.0));
        let id = e.selection[0];
        e.set_tool(Tool::Select);
        assert!(e.set_rotation(30.0, &M));
        let b0 = e.selection_bbox(&M).unwrap();
        let c0 = e
            .view
            .to_screen(Pt::new((b0.x0 + b0.x1) / 2.0, (b0.y0 + b0.y1) / 2.0));
        // in vielen kleinen Schritten nach links über die Linie ziehen
        e.press(c0, Mods::default(), false, &M);
        let mut hit = 0;
        for k in 1..=80 {
            let q = Pt::new(c0.x - k as f64 * 3.0, c0.y);
            e.motion(q, Mods::default(), &M);
            let o = e.doc.object(id).unwrap();
            let pts: Vec<Pt> = match o.shape.to_path() {
                Some(Shape::Path { subpaths, .. }) => subpaths[0].nodes.iter().map(|n| n.p).collect(),
                _ => vec![],
            };
            if pts.iter().any(|p| g.dist(*p) < 1e-6) {
                hit += 1;
            }
        }
        e.release(Pt::new(c0.x - 240.0, c0.y), Mods::default(), &M);
        assert!(hit >= 3, "nur {hit} Schritte an der Linie, {g:?}");
    }

    #[test]
    fn pen_follows_guide() {
        let mut e = editor();
        e.smart_snap = false;
        e.doc.guides.push(Guide {
            pos: Pt::new(0.0, 200.0),
            angle: 0.0,
            layer: 0,
        });
        e.set_tool(Tool::Pen);
        // Klick 8 px neben der Linie: Punkt liegt auf der Linie
        e.press(Pt::new(100.0, 208.0), Mods::default(), false, &M);
        // Griff ungefähr entlang der Linie ziehen
        e.motion(Pt::new(160.0, 205.0), Mods::default(), &M);
        e.release(Pt::new(160.0, 205.0), Mods::default(), &M);
        let n = e.pen.as_ref().unwrap()[0];
        assert!((n.p.y - 200.0).abs() < 1e-9, "{:?}", n.p);
        assert!(
            (n.h_out.y - 200.0).abs() < 1e-9 && (n.h_out.x - 160.0).abs() < 1e-9,
            "{:?}",
            n.h_out
        );
        // deutlich weg von der Linie: freier Griff
        e.press(Pt::new(300.0, 203.0), Mods::default(), false, &M);
        e.motion(Pt::new(340.0, 260.0), Mods::default(), &M);
        e.release(Pt::new(340.0, 260.0), Mods::default(), &M);
        let n = e.pen.as_ref().unwrap()[1];
        assert!((n.p.y - 200.0).abs() < 1e-9);
        assert!((n.h_out.y - 260.0).abs() < 1e-9);
    }

    #[test]
    fn library_apply() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (10.0, 10.0), (60.0, 60.0));
        let a = e.selection[0];
        e.update_style("w", |s| {
            s.width = 7.0;
            s.opacity = 0.4;
            s.stroke = Some(Color::rgb(1.0, 0.0, 0.0));
        });
        let st = e.selected_lib_style(false).unwrap();
        assert!(e.selected_lib_style(true).is_none());
        e.deselect();
        drag(&mut e, (100.0, 100.0), (160.0, 160.0));
        let b = e.selection[0];
        assert!(e.apply_lib_style(&st, &M));
        assert_eq!(e.doc.object(b).unwrap().style, e.doc.object(a).unwrap().style);
        assert!(e.apply_lib_color(Color::rgb(0.0, 0.0, 1.0), None, false));
        assert_eq!(e.selected_color(false), Some(Color::rgb(0.0, 0.0, 1.0)));
        assert!(e.undo());
        // Textstil auf Text
        e.set_tool(Tool::Text);
        drag(&mut e, (200.0, 200.0), (200.0, 200.0));
        e.end_text_edit();
        let mut ts = st.clone();
        ts.text = Some(crate::library::TextStyle {
            family: "Serif".into(),
            size: 40.0,
            bold: true,
            italic: true,
            fmt: TextFormat::default(),
        });
        assert!(e.apply_lib_style(&ts, &M));
        let (_, fam, size, bold, _) = e.selected_objects()[0].shape.text_fields().unwrap();
        assert_eq!(fam, "Serif");
        assert!(bold && (size - 40.0).abs() < 1e-9);
    }

    #[test]
    fn library_drag_onto_object() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (10.0, 10.0), (60.0, 60.0));
        let a = e.selection[0];
        e.deselect();
        let red = Color::rgb(1.0, 0.0, 0.0);
        e.lib_drag_motion(Pt::new(30.0, 30.0), LibPayload::Color(red, None), &M);
        let (t, p) = e.lib_drag_progress().unwrap();
        assert_eq!(t, a);
        assert!(p < 1.0 && e.lib_drag_preview().is_none());
        if let Some((_, since, _)) = e.lib_drop.as_mut() {
            *since = Instant::now() - Duration::from_secs(5);
        }
        let pv = e.lib_drag_preview().unwrap();
        assert_eq!(pv.style.fill, Some(red));
        // das Dokument bleibt bis zum Loslassen unverändert
        assert_ne!(e.doc.object(a).unwrap().style.fill, Some(red));
        // über leerer Fläche: kein Ziel
        e.lib_drag_motion(Pt::new(300.0, 300.0), LibPayload::Color(red, None), &M);
        assert!(e.lib_drop.is_none());
        assert!(e.lib_drop_at(Pt::new(30.0, 30.0), &LibPayload::Color(red, None), &M));
        assert_eq!(e.doc.object(a).unwrap().style.fill, Some(red));
        assert_eq!(e.selection, vec![a]);
        // Textstil passt nicht auf eine Form
        let ts = crate::library::LibStyle {
            style: Style::default(),
            text: Some(crate::library::TextStyle {
                family: "Serif".into(),
                size: 20.0,
                bold: false,
                italic: false,
                fmt: TextFormat::default(),
            }),
            name: String::new(),
        };
        assert!(!e.lib_drop_at(Pt::new(30.0, 30.0), &LibPayload::Style(ts), &M));
        assert!(e.undo());
    }

    #[test]
    fn shape_builder_merges_regions() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (0.0, 0.0), (100.0, 100.0));
        e.deselect();
        drag(&mut e, (50.0, 0.0), (150.0, 100.0));
        e.select_all();
        e.set_tool(Tool::Builder);
        // links → Mitte: diese beiden Flächen vereinigen, rechts bleibt eigenes Stück
        e.press(Pt::new(20.0, 50.0), Mods::default(), false, &M);
        e.motion(Pt::new(75.0, 50.0), Mods::default(), &M);
        assert_eq!(e.builder_marks().0.len(), 2);
        e.release(Pt::new(75.0, 50.0), Mods::default(), &M);
        let objs = &e.doc.layers[0].objects;
        assert_eq!(objs.len(), 2);
        let merged = e.selected_objects()[0].shape.geom_bbox().unwrap();
        assert!(
            (merged.x0 - 0.0).abs() < 0.05 && (merged.x1 - 100.0).abs() < 0.05,
            "{merged:?}"
        );
        assert!(e.undo());
        // Alt: Mitte löschen
        e.select_all();
        e.press(
            Pt::new(75.0, 50.0),
            Mods {
                alt: true,
                ..Mods::default()
            },
            false,
            &M,
        );
        e.release(
            Pt::new(75.0, 50.0),
            Mods {
                alt: true,
                ..Mods::default()
            },
            &M,
        );
        assert_eq!(e.doc.layers[0].objects.len(), 2);
        assert!(e.selection.is_empty());
    }

    #[test]
    fn shape_builder_uses_open_lines() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (0.0, 0.0), (100.0, 100.0));
        let rect = e.selection[0];
        e.deselect();
        // senkrechte offene Linie quer durch das Rechteck
        e.set_tool(Tool::Pen);
        for q in [Pt::new(50.0, -20.0), Pt::new(50.0, 120.0)] {
            e.press(q, Mods::default(), false, &M);
            e.release(q, Mods::default(), &M);
        }
        e.finish_pen(false);
        e.deselect();
        e.set_tool(Tool::Builder);
        let alt = Mods {
            alt: true,
            ..Mods::default()
        };
        // Alt-Klick in die linke Hälfte: sie verschwindet, rechts bleibt, die Linie auch
        e.press(Pt::new(25.0, 50.0), alt, false, &M);
        e.release(Pt::new(25.0, 50.0), alt, &M);
        let objs = &e.doc.layers[0].objects;
        assert_eq!(objs.len(), 2);
        assert!(e.doc.object(rect).is_none());
        let piece = objs
            .iter()
            .find(|o| o.style.fill.is_some())
            .and_then(|o| o.shape.geom_bbox())
            .unwrap();
        assert!(
            (piece.x0 - 50.0).abs() < 0.05 && (piece.x1 - 100.0).abs() < 0.05,
            "{piece:?}"
        );
        // Ziehen über beide Hälften: wieder ein Rechteck
        e.undo();
        e.press(Pt::new(25.0, 50.0), Mods::default(), false, &M);
        e.motion(Pt::new(75.0, 50.0), Mods::default(), &M);
        e.release(Pt::new(75.0, 50.0), Mods::default(), &M);
        let filled: Vec<&Object> = e.doc.layers[0]
            .objects
            .iter()
            .filter(|o| o.style.fill.is_some())
            .collect();
        assert_eq!(filled.len(), 1);
        let b = filled[0].shape.geom_bbox().unwrap();
        assert!((b.x0 - 0.0).abs() < 0.05 && (b.x1 - 100.0).abs() < 0.05, "{b:?}");
    }

    #[test]
    fn eraser_cuts_shapes_and_lines() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (0.0, 0.0), (100.0, 100.0));
        e.deselect();
        e.set_tool(Tool::Eraser);
        e.eraser_size = 20.0;
        e.press(Pt::new(50.0, -20.0), Mods::default(), false, &M);
        e.motion(Pt::new(50.0, 50.0), Mods::default(), &M);
        e.release(Pt::new(50.0, 120.0), Mods::default(), &M);
        match &e.doc.layers[0].objects[0].shape {
            Shape::Path { subpaths, .. } => {
                assert_eq!(subpaths.len(), 2, "{subpaths:?}");
                let rings = boolean::to_rings(subpaths, 0.1);
                let a: f64 = rings.iter().map(|r| boolean::area(r)).sum();
                assert!((a - 8000.0).abs() < 20.0, "{a}");
            }
            _ => panic!(),
        }
        // offene Linie wird getrennt
        e.undo();
        e.doc.layers[0].objects.clear();
        e.set_tool(Tool::Pen);
        for q in [Pt::new(0.0, 200.0), Pt::new(200.0, 200.0)] {
            e.press(q, Mods::default(), false, &M);
            e.release(q, Mods::default(), &M);
        }
        e.finish_pen(false);
        e.deselect();
        e.set_tool(Tool::Eraser);
        drag(&mut e, (100.0, 150.0), (100.0, 250.0));
        match &e.doc.layers[0].objects[0].shape {
            Shape::Path { subpaths, .. } => assert_eq!(subpaths.len(), 2),
            _ => panic!(),
        }
    }

    #[test]
    fn selection_as_document() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (150.0, 140.0));
        let a = e.selection[0];
        e.deselect();
        drag(&mut e, (300.0, 300.0), (320.0, 320.0));
        e.selection = vec![a];
        let d = e.selection_doc(&M).unwrap();
        let objs: Vec<&Object> = d.layers.iter().flat_map(|l| l.objects.iter()).collect();
        assert_eq!(objs.len(), 1);
        // Kontur 2 → 2 Rand ringsum
        assert_eq!((d.width, d.height), (54.0, 44.0));
        let b = M.bbox(objs[0]).unwrap();
        assert!((b.x0 - 2.0).abs() < 1e-9 && (b.y0 - 2.0).abs() < 1e-9, "{b:?}");
        e.deselect();
        assert!(e.selection_doc(&M).is_none());
    }

    #[test]
    fn align_objects_to_each_other() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (10.0, 10.0), (50.0, 50.0));
        let a = e.selection[0];
        e.deselect();
        drag(&mut e, (100.0, 80.0), (200.0, 120.0));
        let b = e.selection[0];
        e.set_tool(Tool::Select);
        e.selection = vec![a, b];
        // links aneinander: beide bei x = 10
        assert!(e.align(Some(-1), None, &M));
        let x0 = |e: &Editor, id: u64| M.bbox(e.doc.object(id).unwrap()).unwrap().x0;
        assert!((x0(&e, a) - 10.0).abs() < 1e-9 && (x0(&e, b) - 10.0).abs() < 1e-9);
        // senkrecht mittig: Mitte des gemeinsamen Rahmens (10 … 120 → 65)
        assert!(e.align(None, Some(0), &M));
        let cy = |e: &Editor, id: u64| {
            let r = M.bbox(e.doc.object(id).unwrap()).unwrap();
            (r.y0 + r.y1) / 2.0
        };
        assert!((cy(&e, a) - 65.0).abs() < 1e-9 && (cy(&e, b) - 65.0).abs() < 1e-9);
        // umgeschaltet auf „Seite“: die Auswahl als Ganzes an die Seite
        e.align_selection = false;
        assert!(e.align(Some(-1), None, &M));
        assert!((x0(&e, a) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn scissors_cut_shapes_and_lines() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        e.shape_kind = ShapeKind::Ellipse;
        drag(&mut e, (0.0, 0.0), (100.0, 100.0));
        e.deselect();
        e.set_tool(Tool::Scissors);
        // senkrecht mitten durch den Kreis: zwei Hälften mit echten Kurven
        drag(&mut e, (50.0, -20.0), (50.0, 120.0));
        assert_eq!(e.doc.layers[0].objects.len(), 2);
        let mut areas = Vec::new();
        for o in &e.doc.layers[0].objects {
            match &o.shape {
                Shape::Path { subpaths, .. } => {
                    assert_eq!(subpaths.len(), 1);
                    // Bogen bleibt Kurve: wenige Knoten, gebogene Anfasser
                    assert!(subpaths[0].nodes.len() <= 8, "{}", subpaths[0].nodes.len());
                    areas.push(boolean::total_area(subpaths));
                }
                other => panic!("{other:?}"),
            }
        }
        let half = std::f64::consts::PI * 50.0 * 50.0 / 2.0;
        assert!(areas.iter().all(|a| (a - half).abs() < half * 0.02), "{areas:?}");
        assert_eq!(e.selection.len(), 2);
        // Schnitt, der nicht ganz durchgeht, ändert nichts
        e.undo();
        e.deselect();
        drag(&mut e, (50.0, -20.0), (50.0, 40.0));
        assert_eq!(e.doc.layers[0].objects.len(), 1);
        // offene Linie: zwei Stücke als eigene Objekte
        e.doc.layers[0].objects.clear();
        e.set_tool(Tool::Pen);
        for q in [Pt::new(0.0, 200.0), Pt::new(200.0, 200.0)] {
            e.press(q, Mods::default(), false, &M);
            e.release(q, Mods::default(), &M);
        }
        e.finish_pen(false);
        e.deselect();
        e.set_tool(Tool::Scissors);
        drag(&mut e, (100.0, 150.0), (100.0, 250.0));
        assert_eq!(e.doc.layers[0].objects.len(), 2);
        let ends: Vec<f64> = e.doc.layers[0]
            .objects
            .iter()
            .map(|o| match &o.shape {
                Shape::Path { subpaths, .. } => subpaths[0].nodes.last().unwrap().p.x,
                _ => panic!(),
            })
            .collect();
        assert!(
            (ends[0] - 100.0).abs() < 0.5 && (ends[1] - 200.0).abs() < 0.5,
            "{ends:?}"
        );
    }

    #[test]
    fn join_paths_closes_gaps() {
        let mut e = editor();
        let line = |e: &mut Editor, a: (f64, f64), b: (f64, f64)| {
            e.set_tool(Tool::Pen);
            for q in [Pt::new(a.0, a.1), Pt::new(b.0, b.1)] {
                e.press(q, Mods::default(), false, &M);
                e.release(q, Mods::default(), &M);
            }
            e.finish_pen(false);
            e.selection[0]
        };
        let a = line(&mut e, (0.0, 0.0), (100.0, 0.0));
        // zweite Linie beginnt knapp neben dem Ende der ersten (Lücke 1 px) und ist verkehrt herum
        let b = line(&mut e, (100.0, 100.0), (101.0, 0.5));
        // (der Zeichenstift rastet am Ende der ersten Linie ein – Lücke von Hand setzen)
        if let Some(Shape::Path { subpaths, .. }) = e.doc.object_mut(b).map(|o| &mut o.shape) {
            let n = subpaths[0].nodes.last_mut().unwrap();
            *n = Node::corner(Pt::new(101.0, 0.5));
        }
        e.set_tool(Tool::Select);
        e.selection = vec![a, b];
        assert!(e.can_join_paths());
        assert!(e.join_paths());
        assert_eq!(e.doc.layers[0].objects.len(), 1);
        let sp = match &e.doc.object(a).unwrap().shape {
            Shape::Path { subpaths, .. } => subpaths[0].clone(),
            other => panic!("{other:?}"),
        };
        // Lücke geschlossen: drei Punkte, der mittlere zwischen beiden Enden
        assert_eq!(sp.nodes.len(), 3);
        assert!(!sp.closed);
        assert!(sp.nodes[1].p.dist(Pt::new(100.5, 0.25)) < 1e-6);
        // ein einzelner offener Pfad wird geschlossen
        assert!(e.join_paths());
        match &e.doc.object(a).unwrap().shape {
            Shape::Path { subpaths, .. } => assert!(subpaths[0].closed && subpaths[0].nodes.len() == 3),
            _ => panic!(),
        }
        // weite Lücke: gerades Verbindungsstück
        e.doc.layers[0].objects.clear();
        let a = line(&mut e, (0.0, 0.0), (100.0, 0.0));
        let b = line(&mut e, (150.0, 0.0), (250.0, 0.0));
        e.selection = vec![a, b];
        assert!(e.join_paths());
        match &e.doc.object(a).unwrap().shape {
            Shape::Path { subpaths, .. } => {
                assert_eq!(subpaths[0].nodes.len(), 4);
                assert!(!subpaths[0].closed);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn hidden_opacity_and_rect_radius() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (0.0, 0.0), (100.0, 100.0));
        let id = e.selection[0];
        // verborgene Gesamtdeckkraft (z. B. aus einem Stil): Farbleiste auf 100 % wirkt wirklich
        e.doc.object_mut(id).unwrap().style.opacity = 0.5;
        assert!(e.set_paint_alpha(PaintTarget::Fill, 1.0));
        let st = &e.doc.object(id).unwrap().style;
        assert_eq!(st.opacity, 1.0);
        assert_eq!(st.fill.unwrap().a, 1.0);
        // Eckenradius eines Rechtecks gilt nicht für das nächste
        assert!(e.set_corner_radius(12.0));
        e.deselect();
        drag(&mut e, (200.0, 0.0), (300.0, 100.0));
        match e.doc.object(e.selection[0]).unwrap().shape {
            Shape::Rect { r, .. } => assert_eq!(r, 0.0),
            ref other => panic!("{other:?}"),
        }
    }

    #[test]
    fn glyphs_split_into_letters() {
        let rect = |x0: f64, y0: f64, x1: f64, y1: f64| SubPath {
            nodes: vec![
                Node::corner(Pt::new(x0, y0)),
                Node::corner(Pt::new(x1, y0)),
                Node::corner(Pt::new(x1, y1)),
                Node::corner(Pt::new(x0, y1)),
            ],
            closed: true,
        };
        let subs = vec![
            rect(0.0, 0.0, 2.0, 10.0),   // i-Strich
            rect(0.0, -4.0, 2.0, -2.0),  // i-Punkt
            rect(5.0, 0.0, 11.0, 10.0),  // o außen
            rect(7.0, 2.0, 9.0, 8.0),    // o innen
            rect(0.0, 22.0, 2.0, 32.0),  // Buchstabe in der nächsten Zeile
            rect(14.0, 0.0, 20.0, 10.0), // Nachbar
        ];
        let parts = glyph_parts(subs, Some(Affine::IDENTITY), 12.0);
        let sizes: Vec<usize> = parts.iter().map(|p| p.len()).collect();
        assert_eq!(sizes, vec![2, 2, 1, 1], "{parts:?}");
    }

    #[test]
    fn guide_snaps_to_anchor_when_moved() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 160.0));
        e.deselect();
        e.doc.guides.push(Guide {
            pos: Pt::new(0.0, 300.0),
            angle: 0.0,
            layer: e.active_layer,
        });
        e.show_guides = true;
        e.set_tool(Tool::Guide);
        // waagerechte Hilfslinie greifen und knapp neben die Unterkante ziehen
        e.press(Pt::new(150.0, 300.0), Mods::default(), false, &M);
        e.motion(Pt::new(150.0, 162.0), Mods::default(), &M);
        e.release(Pt::new(150.0, 162.0), Mods::default(), &M);
        assert!(
            (e.doc.guides[0].pos.y - 160.0).abs() < 1e-9,
            "{:?}",
            e.doc.guides[0].pos
        );
    }

    #[test]
    fn pattern_and_radial() {
        use crate::model::GradShape;
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (10.0, 10.0), (110.0, 90.0));
        e.grad_target = PaintTarget::Fill;
        assert!(e.set_gradient_shape(GradShape::Dots));
        let g = e.selected_objects()[0].style.gradient.clone().unwrap();
        assert_eq!(g.shape, GradShape::Dots);
        assert!(e.set_pattern_cell(30.0));
        assert!(e.set_gradient_shape(GradShape::Radial));
        let g = e.selected_objects()[0].style.gradient.clone().unwrap();
        assert_eq!((g.shape, g.cell), (GradShape::Radial, 30.0));
        assert!((g.p0.x - 0.5).abs() < 1e-9);
        let back = crate::model::Gradient::from_attr(&g.to_attr()).unwrap();
        assert_eq!((back.shape, back.cell), (GradShape::Radial, 30.0));
    }

    #[test]
    fn polygon_corners_later() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        e.shape_kind = ShapeKind::Star;
        e.corners = 5;
        drag(&mut e, (200.0, 200.0), (260.0, 170.0));
        let id = e.selection[0];
        let rp = e.doc.object(id).unwrap().shape.regular_polygon().unwrap();
        assert_eq!(rp.corners, 5);
        assert!(rp.inner.is_some());
        assert_eq!(e.context_info().poly, Some((5, true)));
        assert!(e.set_corners(7));
        let rp2 = e.doc.object(id).unwrap().shape.regular_polygon().unwrap();
        assert_eq!(rp2.corners, 7);
        assert!((rp2.radius - rp.radius).abs() < 1e-6);
        assert!((rp2.angle - rp.angle).abs() < 1e-6);
        // gedrehtes Vieleck bleibt erkennbar
        e.shape_kind = ShapeKind::Polygon;
        drag(&mut e, (100.0, 400.0), (130.0, 380.0));
        let id2 = e.selection[0];
        e.doc
            .object_mut(id2)
            .unwrap()
            .shape
            .transform(&Affine::rotate_about(33.0, Pt::new(10.0, 10.0)));
        assert_eq!(e.context_info().poly, Some((7, false)));
        assert!(e.set_corners(4));
        assert_eq!(e.context_info().poly, Some((4, false)));
    }

    #[test]
    fn artboard_handles_and_guides_on_layers() {
        let mut e = editor();
        e.add_artboard();
        e.set_tool(Tool::Select);
        // ohne Zeichenflächen-Modus: weder Namen noch Anfasser
        let r1 = e.doc.artboard_rects()[1].1;
        let lbl = e.view.to_screen(Pt::new(r1.x0, r1.y0)) + Pt::new(10.0, -8.0);
        assert_eq!(e.artboard_label_at(lbl), None);
        assert!(e.artboard_handles().is_empty());
        e.artboard_mode = true;
        // rechter Anfasser der zweiten Zeichenfläche: breiter ziehen
        let h = e.view.to_screen(Pt::new(r1.x1, (r1.y0 + r1.y1) / 2.0));
        drag(&mut e, (h.x, h.y), (h.x + 100.0, h.y));
        let r1b = e.doc.artboard_rects()[1].1;
        assert!((r1b.width() - (r1.width() + 100.0)).abs() < 1e-6, "{r1b:?}");
        // erste Zeichenfläche oben links verkleinern: bleibt bei (0,0), Ansicht folgt
        let tl = e.view.to_screen(Pt::new(0.0, 0.0));
        let w0 = e.doc.width;
        drag(&mut e, (tl.x, tl.y), (tl.x + 50.0, tl.y + 20.0));
        assert!((e.doc.width - (w0 - 50.0)).abs() < 1e-6);
        assert!((e.view.ox - 50.0).abs() < 1e-6);
        // Hilfslinie auf einer ausgeblendeten Ebene: unsichtbar und nicht greifbar
        let lid = e.active_layer;
        e.doc.guides.push(Guide {
            pos: Pt::new(0.0, 100.0),
            angle: 0.0,
            layer: lid,
        });
        let s = e.view.to_screen(Pt::new(30.0, 100.0));
        assert!(e.guide_at(s).is_some());
        e.set_layer_visible(lid, false);
        assert!(e.guide_at(s).is_none());
        assert!(!e.doc.guide_visible(&e.doc.guides[0]));
    }

    #[test]
    fn notebook_shows_one_page_and_flips() {
        let mut e = editor();
        e.add_artboard();
        e.needs_fit = false;
        e.viewport = (800.0, 300.0);
        // Objekt auf der zweiten Seite
        e.set_tool(Tool::Shape);
        drag(&mut e, (560.0, 20.0), (600.0, 60.0));
        let id = e.selection[0];
        e.deselect();
        e.set_tool(Tool::Select);
        e.view = View {
            zoom: 1.0,
            ox: 0.0,
            oy: 0.0,
        };
        e.set_notebook(true);
        assert_eq!(e.page_index(), Some(0));
        // passt in die Höhe: Seite ganz im Bild (Abstand 48)
        let r0 = e.page_clip().unwrap();
        assert!((r0.width() - 500.0).abs() < 1e-9);
        let z = e.view.zoom;
        assert!((z - (300.0 - 96.0) / 500.0).abs() < 1e-9);
        // das Objekt der zweiten Seite ist ausgeblendet und nicht anklickbar
        let p = Pt::new(580.0, 40.0);
        assert_eq!(e.hit(p, &M), None);
        // hineinzoomen, damit die Seite höher als das Fenster ist
        e.view.zoom = 1.0;
        e.view.oy = 24.0;
        // nach unten scrollen: erst bis zum Seitenende, dann (über die Schwelle) umblättern
        assert!(!e.notebook_scroll(0.0, 150.0));
        assert!((e.view.oy - (24.0 - 150.0)).abs() < 1e-9);
        assert!(!e.notebook_scroll(0.0, 1000.0));
        assert!((e.view.oy - (300.0 - 24.0 - 500.0)).abs() < 1e-9);
        assert!(!e.notebook_scroll(0.0, 40.0));
        assert!(e.notebook_scroll(0.0, 60.0));
        assert_eq!(e.page_index(), Some(1));
        // neue Seite beginnt oben, das Objekt ist jetzt erreichbar
        assert!((e.view.oy - 24.0).abs() < 1e-9);
        assert_eq!(e.hit(p, &M), Some(id));
        // hinter der letzten Seite geht es nicht weiter, zurück zeigt das Ende der vorigen
        assert!(!e.go_page(2, false));
        assert!(!e.notebook_scroll(0.0, -50.0));
        assert!(e.notebook_scroll(0.0, -50.0));
        assert_eq!(e.page_index(), Some(0));
        assert!((e.view.oy - (300.0 - 24.0 - 500.0)).abs() < 1e-9);
        // ausschalten: wieder alles sichtbar
        e.set_notebook(false);
        assert_eq!(e.page_clip(), None);
        e.view = View {
            zoom: 1.0,
            ox: 0.0,
            oy: 0.0,
        };
        assert_eq!(e.hit(p, &M), Some(id));
    }

    #[test]
    fn artboards_add_move_scale() {
        let mut e = editor();
        e.artboard_mode = true;
        let n = e.add_artboard();
        assert_eq!(n, 1);
        let r1 = e.doc.artboard_rects()[1].1;
        assert!((r1.x0 - 540.0).abs() < 1e-9);
        // Objekt auf der zweiten Zeichenfläche
        e.set_tool(Tool::Shape);
        drag(&mut e, (560.0, 20.0), (600.0, 60.0));
        let id = e.selection[0];
        e.deselect();
        e.set_tool(Tool::Select);
        // am Namen ziehen: Zeichenfläche und Objekt wandern mit
        let lbl = e.view.to_screen(Pt::new(r1.x0, r1.y0)) + Pt::new(10.0, -8.0);
        assert_eq!(e.artboard_label_at(lbl), Some(1));
        drag(&mut e, (lbl.x, lbl.y), (lbl.x + 100.0, lbl.y + 50.0));
        let r1b = e.doc.artboard_rects()[1].1;
        assert!((r1b.x0 - 640.0).abs() < 1e-9 && (r1b.y0 - 50.0).abs() < 1e-9);
        let b = e.doc.object(id).unwrap().shape.geom_bbox().unwrap();
        assert!((b.x0 - 660.0).abs() < 1e-9);
        assert!(e.undo());
        // Hauptzeichenfläche verschieben: bleibt bei (0,0), alles andere wandert entgegen
        let lbl0 = e.view.to_screen(Pt::new(0.0, 0.0)) + Pt::new(10.0, -8.0);
        assert_eq!(e.artboard_label_at(lbl0), Some(0));
        drag(&mut e, (lbl0.x, lbl0.y), (lbl0.x + 30.0, lbl0.y));
        assert!((e.doc.artboards[0].x - 510.0).abs() < 1e-9);
        assert!((e.view.ox - 30.0).abs() < 1e-9);
        // Skalieren auf 50 %
        e.select_all();
        e.selection = vec![id];
        let w0 = e.doc.object(id).unwrap().style.width;
        let b0 = e.doc.object(id).unwrap().shape.geom_bbox().unwrap();
        assert!(e.scale_selection(0.5, 0.5, true, &M));
        let b1 = e.doc.object(id).unwrap().shape.geom_bbox().unwrap();
        assert!((b1.width() - b0.width() / 2.0).abs() < 1e-9);
        assert!((e.doc.object(id).unwrap().style.width - w0 / 2.0).abs() < 1e-9);
        // Kopieren in ein anderes Dokument
        let objs = e.clipboard.clone();
        let mut f = editor();
        e.copy();
        assert!(f.paste_objects(e.clipboard.clone()));
        assert_eq!(f.doc.layers[0].objects.len(), 1);
        let _ = objs;
    }

    #[test]
    fn artboards_snap_and_duplicate() {
        let mut e = editor();
        e.artboard_mode = true;
        e.add_artboard();
        let r0 = e.doc.artboard_rects()[0].1;
        let r1 = e.doc.artboard_rects()[1].1;
        // zweite Zeichenfläche bis knapp an die erste ziehen: rastet an deren Kante ein
        let lbl = e.view.to_screen(Pt::new(r1.x0, r1.y0)) + Pt::new(10.0, -8.0);
        let dx = (r0.x1 + 3.0 - r1.x0) * e.view.zoom;
        drag(&mut e, (lbl.x, lbl.y), (lbl.x + dx, lbl.y + 2.0));
        let r1b = e.doc.artboard_rects()[1].1;
        assert!((r1b.x0 - r0.x1).abs() < 1e-9, "{r1b:?}");
        assert!(r1b.y0.abs() < 1e-9, "{r1b:?}");
        // Zeichenfläche mit Inhalt duplizieren
        e.set_tool(Tool::Shape);
        drag(&mut e, (20.0, 20.0), (60.0, 60.0));
        let id = e.selection[0];
        e.deselect();
        e.set_tool(Tool::Select);
        let n = e.doc.layers[0].objects.len();
        let before = e.doc.artboards_bbox();
        assert_eq!(e.duplicate_artboard(0, &M), Some(2));
        let r2 = e.doc.artboard_rects()[2].1;
        assert!((r2.x0 - (before.x1 + 40.0)).abs() < 1e-9);
        assert!((r2.width() - r0.width()).abs() < 1e-9);
        assert_eq!(e.doc.layers[0].objects.len(), n + 1);
        let orig = e.doc.object(id).unwrap().shape.geom_bbox().unwrap();
        let copy = e.doc.layers[0].objects.last().unwrap();
        assert_ne!(copy.id, id);
        let cb = copy.shape.geom_bbox().unwrap();
        assert!((cb.x0 - orig.x0 - r2.x0).abs() < 1e-9);
        assert!(e.doc.artboards[1].name.ends_with("(Kopie)"));
        assert!(e.undo());
        assert_eq!(e.doc.artboards.len(), 1);
    }

    #[test]
    fn small_jitter_is_a_click_not_a_drag() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 200.0));
        let id = e.selection[0];
        e.set_tool(Tool::Select);
        let b0 = e.doc.object(id).unwrap().shape.geom_bbox().unwrap();
        // 3 Pixel Wackeln beim Klicken: nichts bewegt sich
        drag(&mut e, (150.0, 150.0), (153.0, 151.0));
        assert_eq!(e.doc.object(id).unwrap().shape.geom_bbox().unwrap(), b0);
        // echtes Ziehen verschiebt um den ganzen Weg
        drag(&mut e, (150.0, 150.0), (170.0, 150.0));
        let b1 = e.doc.object(id).unwrap().shape.geom_bbox().unwrap();
        assert!((b1.x0 - b0.x0 - 20.0 / e.view.zoom).abs() < 1e-6, "{b0:?} {b1:?}");
    }

    #[test]
    fn first_artboard_can_be_deleted() {
        let mut e = editor();
        // nur eine Zeichenfläche: bleibt
        assert!(!e.delete_artboard(0));
        e.artboard_mode = true;
        e.add_artboard();
        let r1 = e.doc.artboard_rects()[1].1;
        e.set_tool(Tool::Shape);
        drag(&mut e, (560.0, 20.0), (600.0, 60.0));
        let id = e.selection[0];
        let before = e.doc.object(id).unwrap().shape.geom_bbox().unwrap();
        let screen = e.view.to_screen(Pt::new(before.x0, before.y0));
        assert!(e.delete_artboard(0));
        assert!(e.doc.artboards.is_empty());
        assert!((e.doc.width - r1.width()).abs() < 1e-9);
        assert!((e.doc.height - r1.height()).abs() < 1e-9);
        // das Objekt liegt jetzt relativ zur neuen Hauptzeichenfläche, auf dem Bildschirm
        // an derselben Stelle
        let after = e.doc.object(id).unwrap().shape.geom_bbox().unwrap();
        assert!((after.x0 - (before.x0 - r1.x0)).abs() < 1e-9);
        let s2 = e.view.to_screen(Pt::new(after.x0, after.y0));
        assert!((s2.x - screen.x).abs() < 1e-9 && (s2.y - screen.y).abs() < 1e-9);
        assert!(e.undo());
        assert_eq!(e.doc.artboards.len(), 1);
    }

    #[test]
    fn bucket_tool_fills_lens() {
        let mut e = editor();
        e.viewport = (500.0, 500.0);
        e.set_tool(Tool::Shape);
        e.shape_kind = ShapeKind::Ellipse;
        drag(&mut e, (100.0, 100.0), (300.0, 300.0));
        e.deselect();
        drag(&mut e, (200.0, 100.0), (400.0, 300.0));
        e.deselect();
        e.set_tool(Tool::Builder);
        // Linse in der Mitte: Klick macht sie zur neuen Form (die Kreise bleiben)
        e.motion(Pt::new(250.0, 200.0), Mods::default(), &M);
        assert!(!e.builder_marks().0.is_empty());
        e.press(Pt::new(250.0, 200.0), Mods::default(), false, &M);
        e.release(Pt::new(250.0, 200.0), Mods::default(), &M);
        assert_eq!(e.doc.layers[0].objects.len(), 3);
        let b = e.selected_objects()[0].shape.geom_bbox().unwrap();
        assert!((b.x0 - 200.0).abs() < 2.0 && (b.x1 - 300.0).abs() < 2.0, "{b:?}");
        // außerhalb: nichts
        e.press(Pt::new(20.0, 20.0), Mods::default(), false, &M);
        e.release(Pt::new(20.0, 20.0), Mods::default(), &M);
        assert_eq!(e.doc.layers[0].objects.len(), 3);
    }
}
