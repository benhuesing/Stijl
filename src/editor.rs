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
    Style, SubPath, TextFormat, TextFrame, ViewMode,
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
    /// Wie viel Text (Bytes) passt in ein Textfeld? `None`: alles.
    fn area_fit(&self, _obj: &Object) -> Option<usize> {
        None
    }
    /// Oberer Punkt des Cursors vor Zeichen `idx` (Dokumentkoordinaten).
    fn caret_top(&self, _obj: &Object, _idx: usize) -> Option<Pt> {
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
/// Anfasser der Auswahl: so weit (Bildschirmpixel) um ihre Mitte reagieren sie auf Klicks
/// und Berührungen – mehr, als man sieht (gut mit dem Finger zu treffen).
pub const SELECT_HANDLE_HIT: f64 = 12.0;
/// Seitenanfasser erst ab dieser Seitenlänge (Bildschirmpixel); kleinere Auswahlen haben
/// nur die Eckanfasser, damit nichts übereinanderliegt.
pub const SIDE_HANDLE_MIN: f64 = 64.0;

/// Ist Anfasser `i` (0 = oben links, im Uhrzeigersinn) zu sehen und zu greifen?
pub fn handle_shown(hs: &[Pt; 8], i: usize) -> bool {
    i % 2 == 0 || hs[(i + 7) % 8].dist(hs[(i + 1) % 8]) >= SIDE_HANDLE_MIN
}

/// Werkzeuge beim Ansehen eines PDFs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ViewTool {
    /// Text und Objekte auswählen (zum Kopieren)
    #[default]
    Select,
    /// Ansicht verschieben
    Hand,
    /// Textmarker
    Highlight,
    /// Laserpointer: leuchtende Spur, die nach einer Sekunde verschwindet
    Laser,
}

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
    /// Daten (D): Tabelle aufziehen, in Zellen tippen; aus Tabellen werden Diagramme
    Data,
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
/// Abstand zwischen den Seiten im Notizheft (Dokumenteinheiten).
pub const PAGE_GAP: f64 = 24.0;
/// So lange leuchtet eine Spur des Laserpointers (Millisekunden).
pub const LASER_MS: u64 = 1000;
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
    /// Laserpointer (die Punkte stehen in `Editor::laser`)
    Laser,
    /// Seitenrand am Lineal ziehen (0 oben, 1 rechts, 2 unten, 3 links) auf der Seite `page`
    Margin {
        side: usize,
        page: Rect,
    },
    /// Spaltenzwischenraum `gutter` am Lineal ziehen: `edge` – seine Breite ändern (sonst
    /// verschieben); `grab`: Abstand des Zeigers zur Mitte beim Greifen
    Column {
        gutter: usize,
        edge: bool,
        grab: f64,
    },
    /// Griff eines neuen Punkts des Zeichenstifts ziehen; `start` = wo gedrückt wurde (vor
    /// dem Einrasten)
    PenHandle {
        start: Pt,
    },
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
    /// Tabelle aufziehen (Datenwerkzeug); `id` ist ihr Rahmen.
    DataCreate {
        id: u64,
        start: Pt,
    },
    /// Zellen einer gewählten Tabelle aufziehen (wie in einer Tabellenkalkulation)
    CellSelect {
        anchor: u64,
        r: usize,
        c: usize,
    },
    /// Spalten- bzw. Zeilenköpfe überstreichen (ab Spalte/Zeile `start`)
    HeadSelect {
        anchor: u64,
        col: bool,
        start: usize,
    },
    /// Ausfüllkästchen der gewählten Zellen ziehen: um `ext` Zeilen bzw. Spalten fortsetzen
    CellFill {
        anchor: u64,
        range: (usize, usize, usize, usize),
        ext: (isize, isize),
    },
    /// Beim Schreiben einer Formel Zellen anklicken bzw. aufziehen: der Bezug ab Zelle
    /// (`r`, `c`) wird in die Formel geschrieben
    RefPick {
        anchor: u64,
        r: usize,
        c: usize,
    },
    /// Gewählte Zellen an ihrem Rand verschieben (um `off` Zeilen und Spalten)
    CellMove {
        anchor: u64,
        range: (usize, usize, usize, usize),
        grab: (usize, usize),
        off: (isize, isize),
    },
    /// Breite einer Spalte bzw. Höhe einer Zeile ziehen: Größen beim Drücken (Dokument-
    /// einheiten), die mitgehenden Spalten/Zeilen und der Rahmen
    TableSize {
        anchor: u64,
        col: bool,
        idx: usize,
        targets: Vec<usize>,
        sizes: Vec<f64>,
        frame: Rect,
        start: Pt,
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

/// Zeilen und Spalten einer Tabelle ändern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableOp {
    /// Zeilen darunter bzw. darüber, Spalten rechts bzw. links (so viele wie gewählt)
    AddRow,
    AddRowAbove,
    AddCol,
    AddColLeft,
    /// gewählte Zeilen bzw. Spalten entfernen
    RemoveRow,
    RemoveCol,
}

/// Eine Zelle wird bearbeitet: Tabelle (Rahmen), Zeile, Spalte und das Textfeld der Zelle,
/// in dem getippt wird (es zeigt dabei die Eingabe, z. B. die Formel, statt des Ergebnisses).
#[derive(Clone, Debug, PartialEq)]
pub struct CellEdit {
    pub anchor: u64,
    pub row: usize,
    pub col: usize,
    pub part: u64,
    /// Länge des Rückgängig-Stapels nach dem Sichern zu Beginn (Tippschritte darüber
    /// werden beim Übernehmen zu einem Schritt zusammengefasst)
    undo_base: usize,
    /// Dokument war vorher schon geändert (bleibt so, wenn nichts getippt wurde)
    was_dirty: bool,
    /// durch Lostippen begonnen (Pfeiltasten wechseln dann die Zelle, wie in
    /// Tabellenkalkulationen); sonst bewegen sie den Cursor
    pub typed: bool,
}

/// Gewählter Teil einer Tabelle bzw. eines Diagramms: Fläche, Kontur und Schrift in der
/// Eigenschaftenleiste gelten dann nur ihm (sonst dem Ganzen).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// Zellen von (Zeile, Spalte) bis (Zeile, Spalte) einschließlich; die erste Ecke bleibt
    /// beim Erweitern (Umschalttaste) fest
    Cells(usize, usize, usize, usize),
    /// Datenpunkt (Reihe, Nummer): Kreisstück, Balken, Punkt
    Point(usize, usize),
    /// ganze Reihe (Linie, Farbfeld der Legende)
    Series(usize),
    /// ein Text (Legende, Achse, Wert)
    Text(crate::smart::TextKey),
}

impl Pick {
    /// Zellenbereich geordnet (oben, links, unten, rechts), auf die Tabelle begrenzt.
    pub fn cell_range(self, rows: usize, cols: usize) -> Option<(usize, usize, usize, usize)> {
        let Pick::Cells(a, b, c, d) = self else {
            return None;
        };
        if rows == 0 || cols == 0 {
            return None;
        }
        let (r0, r1) = (a.min(c).min(rows - 1), a.max(c).min(rows - 1));
        let (c0, c1) = (b.min(d).min(cols - 1), b.max(d).min(cols - 1));
        Some((r0, c0, r1, c1))
    }
}

/// Spalten- und Zeilenköpfe über bzw. links neben einer gewählten Tabelle (Bildschirmpixel).
/// Höhe der Buchstabenleiste, Abstand zur Tabelle (Platz für die Anfasser dazwischen) und
/// Breite der Zahlenleiste.
pub const HEAD_BAND: f64 = 20.0;
pub const HEAD_GAP: f64 = 12.0;
pub const HEAD_ROW_W: f64 = 26.0;

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
/// Einen Teilpfad vereinfachen (Abweichung höchstens etwa `eps`): die Kurve wird
/// abgetastet, Ecken (Knicke von mehr als 35°) bleiben fest, dazwischen bleiben nur die
/// nötigen Punkte, durch die eine glatte Kurve gelegt wird.
pub fn simplify_subpath(sp: &SubPath, eps: f64) -> SubPath {
    let n = sp.nodes.len();
    if n < 3 {
        return sp.clone();
    }
    let segs = if sp.closed { n } else { n - 1 };
    let node = |i: usize| &sp.nodes[i % n];
    // Richtung, in der die Kurve einen Knoten verlässt bzw. erreicht
    let out_dir = |i: usize| {
        let a = node(i);
        let h = if a.h_out.dist(a.p) > 1e-9 {
            a.h_out
        } else {
            node(i + 1).p
        };
        h - a.p
    };
    let in_dir = |i: usize| {
        let a = node(i);
        let h = if a.h_in.dist(a.p) > 1e-9 {
            a.h_in
        } else {
            node(i + n - 1).p
        };
        a.p - h
    };
    let is_corner = |i: usize| {
        if !sp.closed && (i == 0 || i == n - 1) {
            return true;
        }
        let (u, v) = (in_dir(i), out_dir(i));
        let (lu, lv) = (u.len(), v.len());
        if lu < 1e-9 || lv < 1e-9 {
            return true;
        }
        let cos = (u.x * v.x + u.y * v.y) / (lu * lv);
        cos < 35f64.to_radians().cos()
    };
    // abtasten: Punkte mit Eck-Kennzeichen
    let mut pts: Vec<(Pt, bool)> = Vec::new();
    for i in 0..segs {
        let (a, b) = (node(i), node(i + 1));
        pts.push((a.p, is_corner(i)));
        for k in 1..16 {
            let t = k as f64 / 16.0;
            let u = 1.0 - t;
            let p = a.p * (u * u * u)
                + a.h_out * (3.0 * u * u * t)
                + b.h_in * (3.0 * u * t * t)
                + b.p * (t * t * t);
            pts.push((p, false));
        }
    }
    if !sp.closed {
        pts.push((node(n - 1).p, true));
    }
    // zwischen festen Punkten vereinfachen
    let fixed: Vec<usize> = (0..pts.len()).filter(|i| pts[*i].1).collect();
    let mut keep: Vec<(Pt, bool)> = Vec::new();
    let run = |from: usize, to: usize, keep: &mut Vec<(Pt, bool)>| {
        let line: Vec<Pt> = (from..=to).map(|i| pts[i % pts.len()].0).collect();
        let s = crate::geom::simplify(&line, eps);
        for (j, p) in s.iter().enumerate() {
            if j + 1 < s.len() {
                keep.push((*p, j == 0 && pts[from % pts.len()].1));
            }
        }
    };
    if fixed.is_empty() {
        // geschlossen ohne Ecken: von Punkt 0 ringsum
        run(0, pts.len(), &mut keep);
    } else {
        for (k, &f) in fixed.iter().enumerate() {
            let to = match fixed.get(k + 1) {
                Some(t) => *t,
                None if sp.closed => fixed[0] + pts.len(),
                None => f,
            };
            if to > f {
                run(f, to, &mut keep);
            }
        }
        if !sp.closed {
            keep.push((pts[pts.len() - 1].0, true));
        }
    }
    if keep.len() < 2 || keep.len() >= n {
        return sp.clone();
    }
    SubPath {
        nodes: curvature_nodes(&keep, sp.closed),
        closed: sp.closed,
    }
}

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
    /// Fließtext im Notizheft in Arbeit: nur Textoptionen (die Lage ergibt sich aus der Seite)
    pub body: bool,
    /// genau eine Tabelle bzw. ein Diagramm ausgewählt (oder eine Zelle wird bearbeitet):
    /// Rahmen und Daten
    pub smart: Option<(u64, crate::smart::Smart)>,
    /// gewählte Zelle: Rahmen, Zeile, Spalte, Eingabe (Formelleiste)
    pub cell: Option<(u64, usize, usize, String)>,
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
    /// Textfarbe an der Markierung bzw. am Cursor (sonst die Füllung des Textes)
    pub text_color: Option<Color>,
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
    /// Tabelle/Diagramm: gewählter Teil, sein Aussehen (für die Textfarbe) und ob sich
    /// Fläche/Kontur bzw. die Schrift einstellen lassen
    pub pick: Option<Pick>,
    /// Textfeld: weicht Objekten davor aus (None: kein Textfeld gewählt)
    pub flow: Option<bool>,
    /// Auswahl drängt Text weg (Effekt; None: nichts gewählt)
    pub wrap: Option<bool>,
    /// … mit diesem Abstand und als Rechteck (sonst entlang des Umrisses)
    pub wrap_gap: f64,
    pub wrap_box: bool,
    pub smart_look: Option<crate::smart::Look>,
    pub smart_paint: bool,
    pub smart_text: bool,
}

pub struct Editor {
    pub doc: Document,
    pub selection: Vec<u64>,
    pub active_layer: u64,
    undo: Vec<Document>,
    redo: Vec<Document>,
    pending: Option<Document>,
    /// Vorschau eines Befehls aus der Seitenleiste (Skalieren, Wiederholen …): Zustand davor
    preview: Option<PreviewSnap>,
    /// gerade wird die Vorschau neu berechnet (Änderungen gehören zu ihr)
    previewing: bool,
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
    /// Nur ansehen (PDF in der Betrachtung): Ziehen verschiebt die Ansicht, nichts lässt sich
    /// auswählen oder ändern
    pub read_only: bool,
    /// Präsentation: Seite füllt den Bildschirm, ein Klick blättert weiter
    pub presenting: bool,
    /// Werkzeug beim Ansehen und Farbe des Textmarkers; Strichstärke des laufenden Strichs
    pub view_tool: ViewTool,
    pub highlight_color: Color,
    pub highlight_width: f64,
    /// Markierungen in der Präsentation eines eigenen Dokuments: nur zu sehen, nicht Teil
    /// des Dokuments (zurückgenommene zum Wiederholen)
    pub marks: Vec<Object>,
    marks_redo: Vec<Object>,
    /// Markierungen werden Teil des Dokuments (PDF: als Anmerkungen gespeichert)
    pub keep_marks: bool,
    /// Spur des Laserpointers (Punkt, Zeit)
    pub laser: Vec<(Pt, Instant)>,
    /// Formatierungszeichen zeigen (¶, Leerzeichen, Seitenumbrüche)
    pub show_marks: bool,
    /// Fließtext: Prüfsumme beim letzten Verteilen, Grundformat und Prüfsummen je Feld
    body_key: u64,
    /// Kopf- und Fußzeile: Inhalt beim letzten Abgleich (daran ist zu erkennen, welches Feld
    /// geändert wurde)
    band_hash: [u64; 2],
    /// Stand beim letzten Bestimmen der Aussparungen (`update_flow`)
    flow_seen: u64,
    /// Datum für das Datumsfeld der Kopf-/Fußzeilen, wie es erscheint (setzt das Fenster)
    pub date_text: String,
    body_base: Option<BodyBase>,
    body_hashes: Vec<u64>,
    pub hover: Option<u64>,
    pub pointer: Option<Pt>,
    pub space_down: bool,
    pub clipboard: Vec<Object>,
    paste_count: u32,
    /// Die Oberfläche soll das Texteingabefeld fokussieren.
    pub request_text_focus: bool,
    /// Text wird direkt im Dokument bearbeitet.
    pub text_edit: Option<TextEdit>,
    /// Zelle, in die gerade getippt wird
    pub cell_edit: Option<CellEdit>,
    /// zuletzt gewählte Zelle einer Tabelle (Rahmen, Zeile, Spalte): Formelleiste, Spalten-
    /// und Zeilenköpfe
    pub cell_sel: Option<(u64, usize, usize)>,
    /// gewählter Teil einer Tabelle bzw. eines Diagramms (Rahmen, Teil); gilt nur, solange
    /// genau dieses Objekt ausgewählt ist
    pub pick: Option<(u64, Pick)>,
    /// Klick auf eine schon ausgewählte Tabelle bzw. ein Diagramm: beim Loslassen ohne
    /// Ziehen wird dieser Teil gewählt
    click_pick: Option<(u64, Option<Pick>)>,
    /// letzter Klick in eine Zelle (Zeit, Bildschirmpunkt, Tabelle, Zeile, Spalte): ein
    /// zweiter kurz danach an derselben Stelle beginnt das Tippen (Doppelklick)
    last_cell_click: Option<(Instant, Pt, u64, usize, usize)>,
    /// Bezug, der beim Schreiben einer Formel durch Klicken eingefügt wurde (Bytes im Text):
    /// ein weiterer Klick ersetzt ihn
    ref_point: Option<(usize, usize)>,
    /// Formel, die gerade in der Formelleiste geschrieben wird (Tabelle, Text): ihre Bezüge
    /// werden in der Tabelle farbig umrandet
    pub formula_draft: Option<(u64, String)>,
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
    hover_ctx: RefCell<Option<((u64, u64), SnapCtx)>>,
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
        let pages = doc.view.pages();
        let mut ed = Editor {
            doc,
            selection: Vec::new(),
            active_layer: active,
            undo: Vec::new(),
            redo: Vec::new(),
            pending: None,
            preview: None,
            previewing: false,
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
                // neue Texte: automatischer Zeilenabstand
                fmt: TextFormat {
                    line_auto: true,
                    ..TextFormat::default()
                },
            },
            view: View {
                zoom: 1.0,
                ox: 0.0,
                oy: 0.0,
            },
            needs_fit: true,
            // Seiten untereinander (Notizheft, Präsentation): auf der ersten Seite
            notebook: pages.then_some(0),
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
            read_only: false,
            presenting: false,
            view_tool: ViewTool::Select,
            highlight_color: Color::rgb(1.0, 0.86, 0.0),
            highlight_width: 14.0,
            marks: Vec::new(),
            marks_redo: Vec::new(),
            keep_marks: false,
            laser: Vec::new(),
            show_marks: false,
            body_key: 0,
            band_hash: [0; 2],
            flow_seen: 0,
            date_text: String::new(),
            body_base: None,
            body_hashes: Vec::new(),
            clipboard: Vec::new(),
            paste_count: 0,
            request_text_focus: false,
            text_edit: None,
            cell_edit: None,
            cell_sel: None,
            pick: None,
            click_pick: None,
            last_cell_click: None,
            ref_point: None,
            formula_draft: None,
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
        };
        ed.read_only = ed.doc.view == ViewMode::Presentation;
        ed.apply_layout();
        ed.rebuild_all_smart();
        ed
    }

    /// Tabellen und Diagramme aus ihren Daten neu bauen (nach dem Öffnen: gespeicherte
    /// Lagen sind gerundet, ältere Dateien hatten andere Teile).
    pub fn rebuild_all_smart(&mut self) {
        let anchors: Vec<u64> = self
            .doc
            .layers
            .iter()
            .flat_map(|l| &l.objects)
            .filter(|o| o.smart.is_some())
            .map(|o| o.id)
            .collect();
        for a in anchors {
            self.rebuild_smart(a);
        }
    }

    /// Neues oder geöffnetes Dokument übernehmen.
    pub fn set_document(&mut self, doc: Document) {
        self.active_layer = doc.layers.last().map(|l| l.id).unwrap_or(0);
        self.notebook = doc.view.pages().then_some(0);
        self.read_only = doc.view == ViewMode::Presentation;
        self.presenting = false;
        self.marks.clear();
        self.marks_redo.clear();
        self.laser.clear();
        self.body_key = 0;
        self.body_base = None;
        self.body_hashes.clear();
        self.doc = doc;
        // Seiten passend zur Ansicht stellen (ältere Dateien: nebeneinander)
        self.apply_layout();
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
        self.cell_edit = None;
        self.cell_sel = None;
        self.pick = None;
        self.rebuild_all_smart();
    }

    // ───────────── Vorschau ─────────────

    /// Vorschau beginnen: Befehle mit Einstellungen in der Seitenleiste zeigen ihr Ergebnis,
    /// solange man die Werte ändert; erst „Anwenden“ behält es (als ein Rückgängig-Schritt).
    pub fn preview_begin(&mut self) {
        if self.preview.is_some() {
            self.preview_end(true);
        }
        self.preview = Some(PreviewSnap {
            doc: self.doc.clone(),
            selection: self.selection.clone(),
            undo_len: self.undo.len(),
            redo: self.redo.clone(),
            dirty: self.dirty,
        });
    }

    pub fn preview_active(&self) -> bool {
        self.preview.is_some()
    }

    /// Vorschau neu berechnen: Ausgangszustand wiederherstellen, dann `f` anwenden.
    pub fn preview_apply(&mut self, f: impl FnOnce(&mut Editor) -> bool) -> bool {
        let Some(p) = self.preview.as_ref() else {
            return false;
        };
        self.doc = p.doc.clone();
        self.selection = p.selection.clone();
        self.undo.truncate(p.undo_len);
        self.dirty = p.dirty;
        self.last_key = None;
        self.node_sel.clear();
        self.previewing = true;
        let ok = f(self);
        self.previewing = false;
        // Hinweise erst beim Anwenden, nicht bei jeder Änderung eines Wertes
        self.message = None;
        self.rev += 1;
        self.layers_version += 1;
        ok
    }

    /// Vorschau beenden: `keep` behält das Ergebnis (ein Rückgängig-Schritt), sonst wird der
    /// Ausgangszustand wiederhergestellt.
    pub fn preview_end(&mut self, keep: bool) {
        let Some(p) = self.preview.take() else {
            return;
        };
        if keep {
            self.undo.truncate(p.undo_len + 1);
        } else {
            self.doc = p.doc;
            self.selection = p.selection;
            self.undo.truncate(p.undo_len);
            self.redo = p.redo;
            self.dirty = p.dirty;
            self.rev += 1;
            self.layers_version += 1;
        }
        self.last_key = None;
    }

    // ───────────── Rückgängig ─────────────

    /// Zustand vor einer Änderung sichern. Gleiche `key`s kurz hintereinander werden zusammengefasst.
    pub fn checkpoint(&mut self, key: Option<&str>) {
        // eine andere Änderung während einer Vorschau: die Vorschau gilt als angewendet
        if self.preview.is_some() && !self.previewing {
            self.preview_end(true);
        }
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
        if self.preview.is_some() && !self.previewing {
            self.preview_end(true);
        }
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
        if self.temporary_marks() {
            return !self.marks.is_empty();
        }
        !self.undo.is_empty()
    }

    /// Verlauf vergessen (z. B. nachdem ein PDF neu geschrieben wurde: frühere Zustände
    /// beziehen sich auf die alte Datei).
    pub fn forget_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.pending = None;
        self.last_key = None;
        self.rev += 1;
    }
    pub fn can_redo(&self) -> bool {
        if self.temporary_marks() {
            return !self.marks_redo.is_empty();
        }
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        // während einer Vorschau: nur die Vorschau zurücknehmen
        if self.preview.is_some() {
            self.preview_end(false);
            return true;
        }
        // Präsentation eines eigenen Dokuments: nur die eigenen Markierungen
        if self.temporary_marks() {
            return match self.marks.pop() {
                Some(m) => {
                    self.marks_redo.push(m);
                    true
                }
                None => false,
            };
        }
        self.finish_pen(false);
        self.rev += 1;
        match self.undo.pop() {
            Some(d) => {
                let view = self.doc.view;
                let cur = std::mem::replace(&mut self.doc, d);
                self.redo.push(cur);
                self.keep_view(view);
                self.after_history();
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        if self.temporary_marks() {
            return match self.marks_redo.pop() {
                Some(m) => {
                    self.marks.push(m);
                    true
                }
                None => false,
            };
        }
        self.rev += 1;
        match self.redo.pop() {
            Some(d) => {
                let view = self.doc.view;
                let cur = std::mem::replace(&mut self.doc, d);
                self.undo.push(cur);
                self.keep_view(view);
                self.after_history();
                true
            }
            None => false,
        }
    }

    /// Nach Rückgängig/Wiederholen bleibt die Ansicht (sie ist kein Schritt im Verlauf);
    /// die Seiten stehen wieder passend dazu.
    fn keep_view(&mut self, view: ViewMode) {
        self.doc.view = view;
        self.apply_layout();
    }

    fn after_history(&mut self) {
        self.text_edit = None;
        // Zelle wurde bearbeitet: ihr Textfeld zeigte die Eingabe – wieder das Ergebnis
        if let Some(ce) = self.cell_edit.take() {
            self.rebuild_smart(ce.anchor);
        }
        if self
            .cell_sel
            .is_some_and(|(a, _, _)| self.doc.object(a).is_none())
        {
            self.cell_sel = None;
        }
        if self.pick.is_some_and(|(a, _)| self.doc.object(a).is_none()) {
            self.pick = None;
        }
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
    /// Notizheft ohne Auswahl: der Fließtext gilt als ausgewählt, damit die Eigenschaften
    /// seine Textoptionen zeigen (und nicht die des zuletzt gewählten Objekts). Liefert, ob
    /// sich die Auswahl geändert hat.
    pub fn keep_body_selected(&mut self) -> bool {
        if self.doc.view != ViewMode::Notebook
            || self.read_only
            || !self.selection.is_empty()
            || self.text_edit.is_some()
        {
            return false;
        }
        let body = self.body_frames();
        if body.is_empty() {
            return false;
        }
        self.selection = body;
        true
    }

    pub fn prune_selection(&mut self) {
        let doc = &self.doc;
        // Fließtext liegt in einer gesperrten Ebene, wird im Notizheft aber bearbeitet
        let notebook = doc.view == ViewMode::Notebook;
        self.selection.retain(|id| {
            doc.layer_of(*id).map(|l| l.editable()).unwrap_or(false)
                || (notebook
                    && doc
                        .object(*id)
                        .is_some_and(|o| is_body(&o.shape) || is_band(&o.shape)))
        });
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
        let masks = self.doc.masks();
        self.selected_objects()
            .into_iter()
            .filter_map(|o| self.visible_bbox(o, &masks, m))
            .reduce(|a, b| a.union(&b))
    }

    pub fn select_all(&mut self) {
        // Notizheft: nur Text (Strg A markiert im Text)
        if self.doc.view == ViewMode::Notebook {
            return;
        }
        self.selection = self
            .doc
            .layers
            .iter()
            .filter(|l| l.editable())
            .flat_map(|l| l.objects.iter().filter(|o| !is_body(&o.shape)).map(|o| o.id))
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
            crate::model::regroup_ids(&mut c, &mut regroup, &mut next_group);
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
        // vorhandene Gruppen werden zu Untergruppen der neuen
        for o in self.doc.layers[li].objects.iter_mut() {
            if objs.iter().any(|x| x.id == o.id) {
                if o.group != 0 {
                    o.sub.insert(0, o.group);
                }
                o.group = gid;
            }
        }
        self.layers_version += 1;
        true
    }

    /// Gruppen der Auswahl um eine Ebene auflösen: die äußerste Gruppe verschwindet, ihre
    /// Untergruppen bleiben als eigene Gruppen erhalten (die Objekte bleiben ausgewählt).
    pub fn ungroup_selection(&mut self) -> bool {
        // Tabellen und Diagramme bleiben beisammen
        if self.single_smart().is_some() {
            return false;
        }
        if !self.selected_objects().iter().any(|o| o.group != 0) {
            return false;
        }
        self.checkpoint(None);
        for id in self.selection.clone() {
            if let Some(o) = self.doc.object_mut(id) {
                // eine Schnittmaske hängt an ihrer Gruppe; löst man die äußerste Gruppe
                // der Maske auf, wird sie wieder ein gewöhnliches Objekt
                if o.mask && o.sub.is_empty() {
                    o.mask = false;
                }
                o.group = if o.sub.is_empty() { 0 } else { o.sub.remove(0) };
            }
        }
        self.doc.tidy_groups();
        true
    }

    /// Schnittmaske erstellen: das oberste ausgewählte Objekt begrenzt alle anderen; alle
    /// zusammen werden eine Gruppe (vorhandene Gruppen werden Untergruppen). Das oberste
    /// muss ein einzelnes Objekt sein (keine Gruppe, kein Bild).
    pub fn make_clip_mask(&mut self) -> Result<(), String> {
        self.expand_groups();
        let objs = self.selected_in_order();
        let Some(top) = objs.last().cloned() else {
            return Err(tr("Select at least two objects for this").into());
        };
        if objs.len() < 2 {
            return Err(tr("Select at least two objects for this").into());
        }
        if top.group != 0 || matches!(top.shape, Shape::Image { .. }) || top.mask {
            return Err(tr("The top object must be a single shape, path or text").into());
        }
        if !self.group_selection() {
            return Err(tr("Select at least two objects for this").into());
        }
        if let Some(o) = self.doc.object_mut(top.id) {
            o.mask = true;
        }
        self.layers_version += 1;
        Ok(())
    }

    /// Schnittmasken der Auswahl lösen: die Masken werden wieder gewöhnliche Objekte, ihre
    /// Gruppe wird aufgelöst.
    pub fn release_clip_mask(&mut self) -> bool {
        let masks: Vec<(u64, u64)> = self
            .selected_objects()
            .iter()
            .filter(|o| o.mask)
            .map(|o| (o.id, o.inner_group()))
            .collect();
        if masks.is_empty() {
            return false;
        }
        self.checkpoint(None);
        for (id, g) in masks {
            if let Some(o) = self.doc.object_mut(id) {
                o.mask = false;
            }
            self.doc.remove_group_level(g);
        }
        self.layers_version += 1;
        true
    }

    /// Enthält die Auswahl eine Schnittmaske?
    pub fn selection_has_mask(&self) -> bool {
        self.selected_objects().iter().any(|o| o.mask)
    }

    /// Oberstes bearbeitbares Objekt unter `p`.
    pub fn hit(&self, p: Pt, m: &dyn Metrics) -> Option<u64> {
        self.hit_if(p, m, &|_| true)
    }

    /// Oberster Text unter `p` (Präsentation: es lässt sich nur Text markieren; Bilder und
    /// Formen darüber verdecken ihn dabei nicht).
    fn hit_text(&self, p: Pt, m: &dyn Metrics) -> Option<u64> {
        self.hit_if(p, m, &|o| is_text_shape(&o.shape))
    }

    /// Wie `hit`, nur unter den Objekten, für die `keep` gilt.
    fn hit_if(&self, p: Pt, m: &dyn Metrics, keep: &dyn Fn(&Object) -> bool) -> Option<u64> {
        // Notizheft: was neben den Seiten liegt, ist ausgeblendet
        if !self.on_shown_page(p) {
            return None;
        }
        let tol = 4.0 / self.view.zoom;
        let masks = self.doc.masks();
        for l in self.doc.layers.iter().rev().filter(|l| l.visible) {
            for o in l.objects.iter().rev() {
                // Schnittmasken sind unsichtbar; begrenzte Objekte treffen nur, wo man sie sieht.
                // Der Fließtext (eigene, gesperrte Ebene) lässt sich nur im Notizheft
                // bearbeiten und in der Präsentation markieren; im Notizheft ist er alles, was
                // sich anklicken lässt.
                let body = is_body(&o.shape);
                let skip = match self.doc.view {
                    ViewMode::Artboards => body || l.locked,
                    ViewMode::Notebook => !body,
                    ViewMode::Presentation => l.locked && !body,
                };
                if o.mask || skip || !keep(o) {
                    continue;
                }
                if m.hit(o, p, tol) && crate::model::clips_of(o, &masks).iter().all(|c| m.hit(c, p, 0.0)) {
                    return Some(o.id);
                }
            }
        }
        None
    }

    /// Sichtbarer Rahmen eines Objekts: bei begrenzten Objekten höchstens der Rahmen der
    /// Schnittmaske.
    pub fn visible_bbox(
        &self,
        o: &Object,
        masks: &std::collections::HashMap<u64, Object>,
        m: &dyn Metrics,
    ) -> Option<Rect> {
        let mut b = m.bbox(o)?;
        for c in crate::model::clips_of(o, masks) {
            let cb = m.bbox(c)?;
            b = Rect {
                x0: b.x0.max(cb.x0),
                y0: b.y0.max(cb.y0),
                x1: b.x1.min(cb.x1),
                y1: b.y1.min(cb.y1),
            };
            if b.x0 > b.x1 || b.y0 > b.y1 {
                return None;
            }
        }
        Some(b)
    }

    /// Bildschirmpositionen der 8 Skalierungsanfasser (oben links, im Uhrzeigersinn).
    pub fn handle_positions(&self, m: &dyn Metrics) -> Option<[Pt; 8]> {
        if self.tool != Tool::Select || self.selection.is_empty() || self.selection_locked() {
            return None;
        }
        // Fließtext: Lage und Größe ergeben sich aus der Seite
        if self.selection.iter().any(|id| self.is_body_id(*id)) {
            return None;
        }
        // Zellen einer Tabelle gewählt bzw. in Arbeit: keine Anfasser (die Tabelle ändert
        // man dann an ihren Köpfen)
        if self.cell_edit.is_some() || self.table_pick().is_some() {
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
        // Tabellen dreht man nicht
        if self
            .single_smart()
            .is_some_and(|a| matches!(self.smart_data(a), Some(crate::smart::Smart::Table(_))))
        {
            return None;
        }
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
            .map(|h| h.dist(s) <= SELECT_HANDLE_HIT - 1.0)
            .unwrap_or(false)
    }

    fn handle_at(&self, s: Pt, m: &dyn Metrics) -> Option<usize> {
        let hs = self.handle_positions(m)?;
        // Bei kleinen Auswahlen schrumpft der Greifbereich, damit man sie noch in der
        // Mitte fassen und verschieben kann
        let short = hs[0].dist(hs[2]).min(hs[2].dist(hs[4]));
        let reach = (short / 3.0).clamp(HANDLE_RADIUS, SELECT_HANDLE_HIT);
        hs.iter()
            .enumerate()
            .filter(|(i, h)| handle_shown(&hs, *i) && h.dist(s) <= reach)
            .min_by(|a, b| a.1.dist(s).total_cmp(&b.1.dist(s)))
            .map(|(i, _)| i)
    }

    /// Wird die Auswahl gerade verschoben, skaliert oder gedreht (und hat sich schon etwas
    /// bewegt)? Dann sind die Anfasser ausgeblendet, damit man die echten Kanten sieht.
    pub fn transforming(&self) -> bool {
        matches!(
            self.drag,
            Drag::Move { .. } | Drag::Scale { .. } | Drag::Rotate { .. }
        ) && self.pending.is_none()
    }

    /// Cursorname für die Position (für Skalierungsanfasser).
    pub fn cursor_at(&self, s: Pt, m: &dyn Metrics) -> &'static str {
        if self.presenting && matches!(self.view_tool, ViewTool::Select | ViewTool::Hand) {
            return "default";
        }
        if self.read_only {
            if matches!(self.drag, Drag::Pan { .. }) {
                return "grabbing";
            }
            return match self.view_tool {
                ViewTool::Hand => "grab",
                ViewTool::Highlight | ViewTool::Laser => "crosshair",
                ViewTool::Select => {
                    let p = self.view.to_doc(s);
                    let text = self
                        .hit(p, m)
                        .and_then(|id| self.doc.object(id))
                        .is_some_and(|o| o.shape.text_fields().is_some());
                    if text || matches!(self.drag, Drag::TextSelect { .. }) {
                        "text"
                    } else {
                        "default"
                    }
                }
            };
        }
        if matches!(self.drag, Drag::Artboard { .. }) {
            return "grabbing";
        }
        // Tabelle: Griff, Grenzen der Köpfe, Zellen einer gewählten Tabelle
        match self.drag {
            Drag::TableSize { col, .. } => return if col { "col-resize" } else { "row-resize" },
            Drag::CellSelect { .. } => return "cell",
            Drag::CellFill { .. } => return "crosshair",
            Drag::CellMove { .. } => return "grabbing",
            Drag::None if matches!(self.tool, Tool::Select | Tool::Data) => {
                if self.on_fill_handle(s) {
                    return "crosshair";
                }
                if self.on_pick_border(s) {
                    return "move";
                }
                if self.table_grip_at(s).is_some() {
                    return "move";
                }
                if let Some((_, col, _)) = self.table_edge_at(s) {
                    return if col { "col-resize" } else { "row-resize" };
                }
                if self.table_head_at(s).is_some() {
                    return "default";
                }
                if self.tool == Tool::Select && self.cell_edit.is_none() {
                    let p = self.view.to_doc(s);
                    if let Some(a) = self.single_smart() {
                        if self.table_cell_at(a, p).is_some() {
                            return "cell";
                        }
                    }
                    // Notizheft: über dem Fließtext (bzw. der leeren Seite) der Textcursor
                    if self.doc.view == ViewMode::Notebook && !self.read_only {
                        let m0 = self.hit_quick(p);
                        if m0 {
                            return "text";
                        }
                    }
                }
            }
            _ => {}
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
            self.message = Some(trf("The layer “{}” is hidden", &[&(l.name)]));
            return None;
        }
        if l.locked {
            self.message = Some(trf("The layer “{}” is locked", &[&(l.name)]));
            return None;
        }
        Some(idx)
    }

    /// Tabelle oder Diagramm in den Rahmen `frame` setzen (Dokument-Koordinaten): ein
    /// Rahmen mit den Daten und die daraus gebauten Teile als Gruppe; ausgewählt.
    #[allow(dead_code)]
    pub fn insert_smart(&mut self, smart: crate::smart::Smart, frame: Rect) -> Option<u64> {
        let li = self.target_layer()?;
        if frame.width() <= 0.0 || frame.height() <= 0.0 {
            return None;
        }
        self.checkpoint(None);
        Some(self.insert_smart_at(li, smart, frame))
    }

    /// Wie `insert_smart`, in die Ebene `li` und ohne Rückgängig-Punkt.
    fn insert_smart_at(&mut self, li: usize, smart: crate::smart::Smart, frame: Rect) -> u64 {
        let gid = self.doc.max_group() + 1;
        // Zellen haben ihre eigenen Kästen; der Rahmen bleibt unsichtbar
        let fill = None;
        let id = self.doc.alloc_id();
        let name = match &smart {
            crate::smart::Smart::Table(_) => tr("Table").to_string(),
            crate::smart::Smart::Chart(c) => c.kind.label().to_string(),
        };
        self.doc.layers[li].objects.push(Object {
            group: gid,
            sub: Vec::new(),
            mask: false,
            locked: false,
            smart: Some(Box::new(smart)),
            name,
            id,
            shape: Shape::Rect {
                x: frame.x0,
                y: frame.y0,
                w: frame.width(),
                h: frame.height(),
                r: 0.0,
            },
            style: Style {
                fill,
                stroke: None,
                ..Style::default()
            },
            angle: 0.0,
        });
        self.rebuild_smart(id);
        self.selection = self.doc.group_members(id);
        self.layers_version += 1;
        id
    }

    /// Rahmen (mit Daten) der Tabelle bzw. des Diagramms, zu dem ein Objekt gehört.
    pub fn smart_anchor_of(&self, id: u64) -> Option<u64> {
        let (li, _) = self.doc.locate(id)?;
        let o = self.doc.object(id)?;
        if o.smart.is_some() {
            return Some(id);
        }
        let chain = o.chain();
        if chain.is_empty() {
            return None;
        }
        self.doc.layers[li]
            .objects
            .iter()
            .find(|a| a.smart.is_some() && chain.contains(&a.inner_group()))
            .map(|a| a.id)
    }

    /// Zelle einer Tabelle unter einem Punkt: (Rahmen, Zeile, Spalte).
    pub fn table_cell_at(&self, id: u64, p: Pt) -> Option<(u64, usize, usize)> {
        let a = self.smart_anchor_of(id)?;
        let o = self.doc.object(a)?;
        let crate::smart::Smart::Table(t) = o.smart.as_deref()? else {
            return None;
        };
        let (r, c) = t.cell_at(smart_frame(o)?, p)?;
        Some((a, r, c))
    }

    /// Genau eine Tabelle bzw. ein Diagramm ist ausgewählt (ganz): sein Rahmen.
    pub fn single_smart(&self) -> Option<u64> {
        let anchors = self.smart_anchors();
        let [a] = anchors[..] else { return None };
        let mut members = self.doc.group_members(a);
        let mut sel = self.selection.clone();
        members.sort_unstable();
        sel.sort_unstable();
        (members == sel).then_some(a)
    }

    /// In eine Zelle tippen: ihr Textfeld zeigt die Eingabe (auch Formeln) und wird
    /// bearbeitet; Übernehmen mit `end_text_edit`, Abbrechen mit `cancel_cell_edit`.
    pub fn begin_cell_edit(&mut self, anchor: u64, row: usize, col: usize) -> bool {
        if self.read_only {
            return false;
        }
        self.end_text_edit();
        let Some(o) = self.doc.object(anchor) else {
            return false;
        };
        let Some(crate::smart::Smart::Table(t)) = o.smart.as_deref() else {
            return false;
        };
        if row >= t.n_rows() || col >= t.n_cols() {
            return false;
        }
        let input = t.cells[row][col].clone();
        // frisch gebaut, damit die Teile sicher zu den Daten passen (gespeicherte Lagen sind
        // gerundet); das Textfeld der Zelle anhand seiner Rolle finden
        self.rebuild_smart(anchor);
        let Some(part) = self
            .smart_roles(anchor)
            .into_iter()
            .find(|(_, r)| *r == crate::smart::Role::CellText(row, col))
            .map(|x| x.0)
        else {
            return false;
        };
        let was_dirty = self.dirty;
        self.checkpoint(None);
        if let Some(p) = self.doc.object_mut(part) {
            p.shape.replace_text(input.clone());
        }
        self.cell_edit = Some(CellEdit {
            anchor,
            row,
            col,
            part,
            undo_base: self.undo.len(),
            was_dirty,
            typed: false,
        });
        self.cell_sel = Some((anchor, row, col));
        self.pick = Some((anchor, Pick::Cells(row, col, row, col)));
        self.ref_point = None;
        self.selection = vec![part];
        self.begin_text_edit(part, Some(input.len()));
        true
    }

    /// Getippte Eingabe in die Tabelle übernehmen (ein Schritt zum Rückgängigmachen).
    fn commit_cell(&mut self, ce: CellEdit) {
        let text = self
            .doc
            .object(ce.part)
            .and_then(|o| o.shape.text_fields())
            .map(|t| t.0.to_string())
            .unwrap_or_default();
        // die Tippschritte zählen nicht einzeln
        self.undo.truncate(ce.undo_base);
        self.redo.clear();
        let Some(crate::smart::Smart::Table(mut t)) = self.smart_data(ce.anchor).cloned() else {
            return;
        };
        let changed = t
            .cells
            .get(ce.row)
            .and_then(|r| r.get(ce.col))
            .is_some_and(|old| *old != text);
        if changed {
            t.cells[ce.row][ce.col] = text;
            if let Some(o) = self.doc.object_mut(ce.anchor) {
                o.smart = Some(Box::new(crate::smart::Smart::Table(t)));
            }
        } else if self.undo.len() == ce.undo_base && ce.undo_base > 0 {
            // nichts geändert: auch kein Schritt
            self.undo.pop();
            self.dirty = ce.was_dirty;
        }
        self.rebuild_smart(ce.anchor);
        if changed {
            self.sync_charts(ce.anchor);
        }
        self.selection = self.doc.group_members(ce.anchor);
    }

    /// Tippen in eine Zelle abbrechen (Esc): alles wie vorher.
    pub fn cancel_cell_edit(&mut self) -> bool {
        let Some(ce) = self.cell_edit.take() else {
            return false;
        };
        self.text_edit = None;
        self.undo.truncate(ce.undo_base);
        if self.undo.len() == ce.undo_base && ce.undo_base > 0 {
            self.undo.pop();
        }
        self.dirty = ce.was_dirty;
        self.rebuild_smart(ce.anchor);
        self.selection = self.doc.group_members(ce.anchor);
        self.last_key = None;
        true
    }

    /// Zelle übernehmen und in eine andere wechseln: Tab (`dc` ±1, am Zeilenende in die
    /// nächste Zeile, hinter der letzten Zelle kommt eine Zeile dazu), Eingabe (`dr` ±1).
    pub fn cell_edit_move(&mut self, dr: isize, dc: isize) -> bool {
        let Some(ce) = self.cell_edit.clone() else {
            return false;
        };
        self.end_text_edit();
        let Some(crate::smart::Smart::Table(t)) = self.smart_data(ce.anchor).cloned() else {
            return false;
        };
        let (rows, cols) = (t.n_rows() as isize, t.n_cols() as isize);
        // verbundene Zellen: von ihrem Rand aus weiter
        let m = t
            .merge_at(ce.row, ce.col)
            .unwrap_or((ce.row, ce.col, ce.row, ce.col));
        let (mut r, mut c) = (
            if dr > 0 {
                m.2 as isize + dr
            } else {
                ce.row as isize + dr
            },
            if dc > 0 {
                m.3 as isize + dc
            } else {
                ce.col as isize + dc
            },
        );
        if c >= cols {
            c = 0;
            r += 1;
        } else if c < 0 {
            c = cols - 1;
            r -= 1;
        }
        if r < 0 {
            return true;
        }
        if r >= rows {
            if dc == 0 {
                // Eingabe in der letzten Zeile: fertig
                return true;
            }
            // Tab am Ende: neue Zeile, die Tabelle wächst um sie
            let Some(frame) = self.doc.object(ce.anchor).and_then(smart_frame) else {
                return false;
            };
            let sum: f64 = t.rows.iter().sum();
            let mut t = t;
            t.insert_row(rows as usize);
            let h = frame.height() * t.rows.iter().sum::<f64>() / sum.max(1e-9);
            self.set_smart(ce.anchor, crate::smart::Smart::Table(t));
            if let Some(o) = self.doc.object_mut(ce.anchor) {
                o.shape = Shape::Rect {
                    x: frame.x0,
                    y: frame.y0,
                    w: frame.width(),
                    h,
                    r: 0.0,
                };
            }
            self.rebuild_smart(ce.anchor);
        }
        let (r, c) = (r as usize, c as usize);
        let (r, c) = match self.smart_data(ce.anchor) {
            Some(crate::smart::Smart::Table(t)) => t.merge_at(r, c).map_or((r, c), |m| (m.0, m.1)),
            _ => (r, c),
        };
        let typed = ce.typed;
        let ok = self.begin_cell_edit(ce.anchor, r, c);
        if let Some(ce) = &mut self.cell_edit {
            ce.typed = typed;
        }
        ok
    }

    /// Pfeiltaste beim Tippen in einer Zelle: Zelle wechseln (durch Lostippen begonnen;
    /// sonst nur oben/unten und am Anfang bzw. Ende des Textes nach links/rechts).
    pub fn cell_edit_arrow(&mut self, dr: isize, dc: isize) -> bool {
        let (Some(ce), Some(te)) = (self.cell_edit.clone(), self.text_edit) else {
            return false;
        };
        let len = self
            .doc
            .object(te.id)
            .and_then(|o| o.shape.text_fields())
            .map(|t| t.0.len())
            .unwrap_or(0);
        let multiline = self
            .doc
            .object(te.id)
            .and_then(|o| o.shape.text_fields())
            .is_some_and(|t| t.0.contains('\n'));
        let go = ce.typed
            || (dr != 0 && !multiline)
            || (dc < 0 && te.caret == 0 && te.anchor == 0)
            || (dc > 0 && te.caret == len && te.anchor == len);
        if !go {
            return false;
        }
        let a = ce.anchor;
        self.cell_edit_move(dr, dc);
        // weiter gewählt (nicht tippend) – wie nach Eingabe in Tabellenkalkulationen
        if let Some(ce) = &self.cell_edit {
            if ce.typed {
                let (r, c) = (ce.row, ce.col);
                self.end_text_edit();
                if let Some(crate::smart::Smart::Table(t)) = self.smart_data(a) {
                    let g = t.expand((r, c, r, c));
                    self.pick = Some((a, Pick::Cells(g.0, g.1, g.2, g.3)));
                }
            }
        }
        true
    }

    /// Gewählte Zellen verbinden bzw. ihre Verbindungen lösen.
    pub fn merge_cells(&mut self, merge: bool) -> bool {
        let Some((a, range)) = self.table_pick() else {
            return false;
        };
        let Some(crate::smart::Smart::Table(mut t)) = self.smart_data(a).cloned() else {
            return false;
        };
        let range = t.expand(range);
        let changed = if merge { t.merge(range) } else { t.split(range) };
        if !changed {
            return false;
        }
        self.end_text_edit();
        self.set_smart(a, crate::smart::Smart::Table(t));
        self.pick = Some((a, Pick::Cells(range.0, range.1, range.2, range.3)));
        self.cell_sel = Some((a, range.0, range.1));
        self.selection = self.doc.group_members(a);
        true
    }

    /// Gewählte Zellen teilen (`cols`: nebeneinander, sonst untereinander); verbundene Zellen
    /// werden dabei in ihre Spalten bzw. Zeilen getrennt.
    pub fn split_cells(&mut self, cols: bool) -> bool {
        let Some((a, range)) = self.table_pick() else {
            return false;
        };
        let Some(crate::smart::Smart::Table(mut t)) = self.smart_data(a).cloned() else {
            return false;
        };
        let Some(g) = t.split_cells(range, cols) else {
            return false;
        };
        self.end_text_edit();
        self.set_smart(a, crate::smart::Smart::Table(t));
        self.pick = Some((a, Pick::Cells(g.0, g.1, g.2, g.3)));
        self.cell_sel = Some((a, g.0, g.1));
        self.selection = self.doc.group_members(a);
        true
    }

    /// Lassen sich die gewählten Zellen verbinden bzw. teilen?
    pub fn can_merge(&self) -> (bool, bool) {
        let Some((a, g)) = self.table_pick() else {
            return (false, false);
        };
        let Some(crate::smart::Smart::Table(t)) = self.smart_data(a) else {
            return (false, false);
        };
        let g = t.expand(g);
        // teilen geht immer (verbundene Zellen werden getrennt, sonst neue Spalten/Zeilen)
        let split = true;
        let merge = (g.0 != g.2 || g.1 != g.3) && t.merge_at(g.0, g.1) != Some(g);
        (merge, split)
    }

    /// Aussehen der gewählten Zellen ändern (z. B. Zahlenformat, senkrechter Text).
    pub fn edit_table_looks(&mut self, key: &str, g: &dyn Fn(&mut crate::smart::Look)) -> bool {
        let Some(a) = self.smart_target() else {
            return false;
        };
        if !matches!(self.smart_data(a), Some(crate::smart::Smart::Table(_))) {
            return false;
        }
        self.edit_looks(a, key, true, g)
    }

    /// Vorschlag beim Schreiben einer Formel: das angefangene Wort vor dem Cursor zu einem
    /// Funktionsnamen ergänzen. Liefert (Rest, ganzer Name).
    pub fn formula_suggestion(&self) -> Option<(String, String)> {
        let te = self.text_edit?;
        self.cell_edit.as_ref()?;
        if te.caret != te.anchor {
            return None;
        }
        let t = self.doc.object(te.id)?.shape.text_fields()?.0;
        crate::sheet::suggest_function(t, te.caret)
    }

    /// Vorschlag übernehmen: das angefangene Wort wird zum Funktionsnamen mit „(“.
    pub fn accept_suggestion(&mut self) -> bool {
        let Some((rest, full)) = self.formula_suggestion() else {
            return false;
        };
        let Some(te) = self.text_edit else { return false };
        let typed = full.len() - rest.len();
        let start = te.caret.saturating_sub(typed);
        self.text_edit = Some(TextEdit {
            id: te.id,
            caret: te.caret,
            anchor: start,
        });
        self.edit_insert(&format!("{full}("))
    }

    /// Zeilen bzw. Spalten einfügen (vor oder hinter den gewählten Zellen, so viele wie
    /// gewählt; ohne Wahl an der zuletzt angeklickten Zelle, sonst am Ende) bzw. die
    /// gewählten entfernen. Die übrigen behalten ihre Größe, die Tabelle wächst bzw.
    /// schrumpft; die neuen Zeilen/Spalten sind danach gewählt.
    pub fn table_op(&mut self, op: TableOp) -> bool {
        let Some(anchor) = self.smart_target() else {
            return false;
        };
        self.end_text_edit();
        let Some(crate::smart::Smart::Table(mut t)) = self.smart_data(anchor).cloned() else {
            return false;
        };
        let Some(frame) = self.doc.object(anchor).and_then(smart_frame) else {
            return false;
        };
        let (rows, cols) = (t.n_rows(), t.n_cols());
        let (sum_r, sum_c): (f64, f64) = (t.rows.iter().sum(), t.cols.iter().sum());
        let range = self
            .active_pick()
            .and_then(|p| p.cell_range(rows, cols))
            .or_else(|| match self.cell_sel {
                Some((a, r, c)) if a == anchor && r < rows && c < cols => Some((r, c, r, c)),
                _ => None,
            });
        let (r0, c0, r1, c1) = range.unwrap_or((rows - 1, cols - 1, rows - 1, cols - 1));
        let (nr, nc) = (r1 - r0 + 1, c1 - c0 + 1);
        let pick = match op {
            TableOp::AddRowAbove => {
                for _ in 0..nr {
                    t.insert_row(r0);
                }
                Some(Pick::Cells(r0, 0, r0 + nr - 1, cols - 1))
            }
            TableOp::AddRow => {
                for _ in 0..nr {
                    t.insert_row(r1 + 1);
                }
                Some(Pick::Cells(r1 + 1, 0, r1 + nr, cols - 1))
            }
            TableOp::AddColLeft => {
                for _ in 0..nc {
                    t.insert_col(c0);
                }
                Some(Pick::Cells(0, c0, rows - 1, c0 + nc - 1))
            }
            TableOp::AddCol => {
                for _ in 0..nc {
                    t.insert_col(c1 + 1);
                }
                Some(Pick::Cells(0, c1 + 1, rows - 1, c1 + nc))
            }
            TableOp::RemoveRow => {
                for _ in 0..nr {
                    t.remove_row(r0);
                }
                None
            }
            TableOp::RemoveCol => {
                for _ in 0..nc {
                    t.remove_col(c0);
                }
                None
            }
        };
        let (n_rows, n_cols) = (t.n_rows(), t.n_cols());
        if (n_rows, n_cols) == (rows, cols) {
            return false;
        }
        // jede Zeile/Spalte behält ihre Größe: der Rahmen wächst bzw. schrumpft
        let w = frame.width() * t.cols.iter().sum::<f64>() / sum_c.max(1e-9);
        let h = frame.height() * t.rows.iter().sum::<f64>() / sum_r.max(1e-9);
        if !self.set_smart(anchor, crate::smart::Smart::Table(t)) {
            return false;
        }
        if let Some(o) = self.doc.object_mut(anchor) {
            o.shape = Shape::Rect {
                x: frame.x0,
                y: frame.y0,
                w,
                h,
                r: 0.0,
            };
        }
        self.rebuild_smart(anchor);
        self.pick = pick.map(|p| (anchor, p));
        let (r, c) = match pick {
            Some(Pick::Cells(r, c, _, _)) => (r, c),
            _ => (r0.min(n_rows - 1), c0.min(n_cols - 1)),
        };
        self.cell_sel = Some((anchor, r, c));
        self.selection = self.doc.group_members(anchor);
        true
    }

    /// Eingabe einer Zelle setzen (Formelleiste).
    pub fn set_cell_input(&mut self, anchor: u64, row: usize, col: usize, text: &str) -> bool {
        let Some(crate::smart::Smart::Table(mut t)) = self.smart_data(anchor).cloned() else {
            return false;
        };
        let Some(cell) = t.cells.get_mut(row).and_then(|r| r.get_mut(col)) else {
            return false;
        };
        *cell = text.to_string();
        self.set_smart(anchor, crate::smart::Smart::Table(t))
    }

    /// Eingabe der gewählten Zelle (für die Formelleiste).
    pub fn cell_input(&self) -> Option<(u64, usize, usize, String)> {
        let (a, r, c) = self.cell_sel?;
        if let Some(ce) = &self.cell_edit {
            let t = self.doc.object(ce.part)?.shape.text_fields()?.0.to_string();
            return Some((ce.anchor, ce.row, ce.col, t));
        }
        let crate::smart::Smart::Table(t) = self.smart_data(a)? else {
            return None;
        };
        Some((a, r, c, t.cells.get(r)?.get(c)?.clone()))
    }

    /// Tabelle bzw. Diagramm, dessen Eigenschaften gerade gezeigt werden (in eine Zelle
    /// wird getippt oder genau dieses Objekt ist ganz ausgewählt).
    pub fn smart_target(&self) -> Option<u64> {
        self.cell_edit
            .as_ref()
            .map(|c| c.anchor)
            .or_else(|| self.single_smart())
    }

    /// Gewählter Teil (nur, wenn er zur gezeigten Tabelle bzw. zum Diagramm gehört).
    pub fn active_pick(&self) -> Option<Pick> {
        let a = self.smart_target()?;
        self.pick.filter(|p| p.0 == a).map(|p| p.1)
    }

    /// Bezüge der Formel, die gerade in eine Zelle geschrieben wird: (Byte von, bis,
    /// Bereich, Farbe).
    pub fn formula_marks(&self) -> Vec<(usize, usize, (usize, usize, usize, usize), usize)> {
        let Some(ce) = &self.cell_edit else {
            // in der Formelleiste
            return match &self.formula_draft {
                Some((a, t)) if self.smart_target() == Some(*a) => crate::sheet::formula_refs(t),
                _ => Vec::new(),
            };
        };
        let Some(t) = self.doc.object(ce.part).and_then(|o| o.shape.text_fields()) else {
            return Vec::new();
        };
        crate::sheet::formula_refs(t.0)
    }

    /// Bezug aus dem Zeigen noch gültig (Cursor steht direkt dahinter, nichts markiert)?
    fn live_ref_point(&self) -> Option<(usize, usize)> {
        let (a, b) = self.ref_point?;
        let te = self.text_edit?;
        let t = self.doc.object(te.id)?.shape.text_fields()?.0;
        (te.caret == b && te.anchor == b && b <= t.len() && t.is_char_boundary(a)).then_some((a, b))
    }

    /// Beim Schreiben einer Formel: Bezug auf Zellen (`r0`,`c0`)–(`r1`,`c1`) am Cursor
    /// einfügen bzw. den eben eingefügten ersetzen.
    fn put_ref(&mut self, r0: usize, c0: usize, r1: usize, c1: usize) {
        let name = |r: usize, c: usize| format!("{}{}", crate::sheet::col_name(c), r + 1);
        let (a, b) = ((r0.min(r1), c0.min(c1)), (r0.max(r1), c0.max(c1)));
        let text = if a == b {
            name(a.0, a.1)
        } else {
            format!("{}:{}", name(a.0, a.1), name(b.0, b.1))
        };
        let Some(te) = self.text_edit else { return };
        let (s0, s1) = self.live_ref_point().unwrap_or(te.range());
        self.text_edit = Some(TextEdit {
            id: te.id,
            caret: s1,
            anchor: s0,
        });
        self.edit_insert(&text);
        self.ref_point = Some((s0, s0 + text.len()));
    }

    /// Zelle der gezeigten Tabelle unter einem Bildschirmpunkt (`clamp`: außerhalb die
    /// nächste): (Rahmen, Zeile, Spalte).
    pub fn table_cell_screen(&self, s: Pt, clamp: bool) -> Option<(u64, usize, usize)> {
        let a = self.smart_target()?;
        let p = self.view.to_doc(s);
        if !clamp {
            return self.table_cell_at(a, p);
        }
        let (r, c) = self.cell_toward(a, p)?;
        let crate::smart::Smart::Table(t) = self.smart_data(a)? else {
            return None;
        };
        Some((
            a,
            r.clamp(0, t.n_rows() as isize - 1) as usize,
            c.clamp(0, t.n_cols() as isize - 1) as usize,
        ))
    }

    /// Gewählte Zellen: mit den Pfeiltasten wandern (Umschalttaste: erweitern); über
    /// verbundene Zellen hinweg.
    pub fn move_pick(&mut self, dr: isize, dc: isize, extend: bool) -> bool {
        let Some((a, (r0, c0, r1, c1))) = self.table_pick() else {
            return false;
        };
        let Some(crate::smart::Smart::Table(t)) = self.smart_data(a) else {
            return false;
        };
        let (nr, nc) = (t.n_rows() as isize, t.n_cols() as isize);
        let Some((_, Pick::Cells(ar, ac, br, bc))) = self.pick else {
            return false;
        };
        if extend {
            let r = (br as isize + dr).clamp(0, nr - 1) as usize;
            let c = (bc as isize + dc).clamp(0, nc - 1) as usize;
            let g = t.expand((ar.min(r), ac.min(c), ar.max(r), ac.max(c)));
            // die feste Ecke bleibt vorn
            let (fr, fc) = (if r < ar { g.2 } else { g.0 }, if c < ac { g.3 } else { g.1 });
            let (er, ec) = (if r < ar { g.0 } else { g.2 }, if c < ac { g.1 } else { g.3 });
            self.pick = Some((a, Pick::Cells(fr, fc, er, ec)));
            return true;
        }
        // von der Kante des Gewählten aus einen Schritt weiter
        let r = if dr > 0 {
            r1 as isize + dr
        } else if dr < 0 {
            r0 as isize + dr
        } else {
            r0 as isize
        };
        let c = if dc > 0 {
            c1 as isize + dc
        } else if dc < 0 {
            c0 as isize + dc
        } else {
            c0 as isize
        };
        let (r, c) = (r.clamp(0, nr - 1) as usize, c.clamp(0, nc - 1) as usize);
        let g = t.expand((r, c, r, c));
        self.pick = Some((a, Pick::Cells(g.0, g.1, g.2, g.3)));
        self.cell_sel = Some((a, g.0, g.1));
        true
    }

    /// In die gewählte Zelle tippen (wie in einer Tabellenkalkulation): `replace` ersetzt
    /// den Inhalt durch `text`, sonst steht der Cursor am Ende.
    pub fn edit_pick(&mut self, text: &str, replace: bool) -> bool {
        let Some((a, (r, c, _, _))) = self.table_pick() else {
            return false;
        };
        if !self.begin_cell_edit(a, r, c) {
            return false;
        }
        if replace {
            if let Some(te) = self.text_edit {
                let len = self
                    .doc
                    .object(te.id)
                    .and_then(|o| o.shape.text_fields())
                    .map(|t| t.0.len())
                    .unwrap_or(0);
                self.text_edit = Some(TextEdit {
                    id: te.id,
                    caret: len,
                    anchor: 0,
                });
            }
            if let Some(ce) = &mut self.cell_edit {
                ce.typed = true;
            }
            return self.edit_insert(text);
        }
        true
    }

    /// Gewählte Zellen als Text (Tabulator zwischen Spalten, Zeilenumbruch zwischen Zeilen;
    /// wie angezeigt, also mit Ergebnissen statt Formeln).
    pub fn cells_text(&self) -> Option<String> {
        let (a, (r0, c0, r1, c1)) = self.table_pick()?;
        let crate::smart::Smart::Table(t) = self.smart_data(a)? else {
            return None;
        };
        let comma = crate::i18n::decimal_comma();
        let vals = crate::sheet::evaluate(&t.cells, comma);
        Some(
            (r0..=r1)
                .map(|r| {
                    (c0..=c1)
                        .map(|c| vals[r][c].display(comma).replace(['\t', '\n'], " "))
                        .collect::<Vec<_>>()
                        .join("\t")
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    /// Gewählte Zellen leeren (Ausschneiden).
    pub fn clear_picked_cells(&mut self) -> bool {
        let Some((a, (r0, c0, r1, c1))) = self.table_pick() else {
            return false;
        };
        let Some(crate::smart::Smart::Table(mut t)) = self.smart_data(a).cloned() else {
            return false;
        };
        for row in &mut t.cells[r0..=r1] {
            for cell in &mut row[c0..=c1] {
                cell.clear();
            }
        }
        self.set_smart(a, crate::smart::Smart::Table(t))
    }

    /// Text (Zeilen, Spalten durch Tabulatoren getrennt) ab der gewählten Zelle einfügen;
    /// die Tabelle wächst, wenn er nicht hineinpasst. Danach ist der eingefügte Bereich
    /// gewählt.
    pub fn paste_cells(&mut self, text: &str) -> bool {
        let Some((a, (r0, c0, _, _))) = self.table_pick() else {
            return false;
        };
        let Some(o) = self.doc.object(a) else { return false };
        let (Some(crate::smart::Smart::Table(t)), Some(frame)) = (o.smart.as_deref(), smart_frame(o)) else {
            return false;
        };
        let mut t = t.clone();
        let rows: Vec<Vec<String>> = text
            .trim_end_matches(['\n', '\r'])
            .split('\n')
            .map(|l| {
                l.trim_end_matches('\r')
                    .split('\t')
                    .map(|s| s.to_string())
                    .collect()
            })
            .collect();
        if rows.is_empty() {
            return false;
        }
        let w = rows.iter().map(|r| r.len()).max().unwrap_or(1);
        let (sum_r, sum_c): (f64, f64) = (t.rows.iter().sum(), t.cols.iter().sum());
        let (n0r, n0c) = (t.n_rows(), t.n_cols());
        while t.n_rows() < r0 + rows.len() {
            let n = t.n_rows();
            t.insert_row(n);
        }
        while t.n_cols() < c0 + w {
            let n = t.n_cols();
            t.insert_col(n);
        }
        for (i, row) in rows.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                t.cells[r0 + i][c0 + j] = v.clone();
            }
        }
        let grown = (t.n_rows(), t.n_cols()) != (n0r, n0c);
        let (fw, fh) = (
            frame.width() * t.cols.iter().sum::<f64>() / sum_c.max(1e-9),
            frame.height() * t.rows.iter().sum::<f64>() / sum_r.max(1e-9),
        );
        self.end_text_edit();
        if !self.set_smart(a, crate::smart::Smart::Table(t)) {
            return false;
        }
        if grown {
            if let Some(o) = self.doc.object_mut(a) {
                o.shape = Shape::Rect {
                    x: frame.x0,
                    y: frame.y0,
                    w: fw,
                    h: fh,
                    r: 0.0,
                };
            }
            self.rebuild_smart(a);
        }
        self.pick = Some((a, Pick::Cells(r0, c0, r0 + rows.len() - 1, c0 + w - 1)));
        self.selection = self.doc.group_members(a);
        true
    }

    /// Gewählte Zellen der gezeigten Tabelle: (Rahmen, oben, links, unten, rechts).
    pub fn table_pick(&self) -> Option<(u64, (usize, usize, usize, usize))> {
        let a = self.smart_target()?;
        let crate::smart::Smart::Table(t) = self.smart_data(a)? else {
            return None;
        };
        let r = self.active_pick()?.cell_range(t.n_rows(), t.n_cols())?;
        Some((a, r))
    }

    /// Gebaute Teile einer Tabelle bzw. eines Diagramms in Ebenenreihenfolge (ohne Rahmen).
    fn smart_members(&self, anchor: u64) -> Vec<u64> {
        let (Some((li, _)), Some(o)) = (self.doc.locate(anchor), self.doc.object(anchor)) else {
            return Vec::new();
        };
        let inner = o.inner_group();
        self.doc.layers[li]
            .objects
            .iter()
            .filter(|p| p.id != anchor && p.chain().contains(&inner))
            .map(|p| p.id)
            .collect()
    }

    /// Was die gebauten Teile darstellen (leer, wenn sie nicht mehr zu den Daten passen).
    fn smart_roles(&self, anchor: u64) -> Vec<(u64, crate::smart::Role)> {
        let Some(o) = self.doc.object(anchor) else {
            return Vec::new();
        };
        let (Some(sm), Some(frame)) = (o.smart.as_deref(), smart_frame(o)) else {
            return Vec::new();
        };
        let roles: Vec<crate::smart::Role> =
            crate::smart::build_parts(sm, frame, crate::i18n::decimal_comma())
                .into_iter()
                .map(|p| p.1)
                .collect();
        let members = self.smart_members(anchor);
        if roles.len() != members.len() {
            return Vec::new();
        }
        members.into_iter().zip(roles).collect()
    }

    /// Einzeln wählbarer Teil unter einem Punkt (Zelle, Datenpunkt, Reihe, Beschriftung).
    fn pick_at(&self, anchor: u64, p: Pt, m: &dyn Metrics) -> Option<Pick> {
        use crate::smart::Role;
        match self.smart_data(anchor)? {
            crate::smart::Smart::Table(t) => {
                let (_, r, c) = self.table_cell_at(anchor, p)?;
                let g = t.expand((r, c, r, c));
                Some(Pick::Cells(g.0, g.1, g.2, g.3))
            }
            crate::smart::Smart::Chart(_) => {
                let id = self.hit(p, m)?;
                match self.smart_roles(anchor).into_iter().find(|x| x.0 == id)?.1 {
                    Role::Point(a, b) => Some(Pick::Point(a, b)),
                    Role::Series(a) => Some(Pick::Series(a)),
                    Role::Text(k) => Some(Pick::Text(k)),
                    _ => None,
                }
            }
        }
    }

    /// Teile, die zum gewählten Teil eines Diagramms gehören (zum Hervorheben).
    pub fn picked_members(&self) -> Vec<u64> {
        use crate::smart::Role;
        let (Some(a), Some(pk)) = (self.smart_target(), self.active_pick()) else {
            return Vec::new();
        };
        self.smart_roles(a)
            .into_iter()
            .filter(|(_, r)| match (pk, *r) {
                (Pick::Point(a, b), Role::Point(c, d)) => a == c && b == d,
                (Pick::Series(a), Role::Series(b)) | (Pick::Series(a), Role::Point(b, _)) => a == b,
                (Pick::Text(a), Role::Text(b)) => a == b,
                _ => false,
            })
            .map(|x| x.0)
            .collect()
    }

    /// Köpfe der gezeigten Tabelle auf dem Bildschirm: Rahmen, Grenzen der Spalten (x) und
    /// Zeilen (y) und die linke obere Ecke.
    fn head_geom(&self) -> Option<(u64, Vec<f64>, Vec<f64>, Pt)> {
        if self.read_only || self.presenting {
            return None;
        }
        let a = self.smart_target()?;
        let o = self.doc.object(a)?;
        let crate::smart::Smart::Table(t) = o.smart.as_deref()? else {
            return None;
        };
        let frame = smart_frame(o)?;
        let (xs, ys) = t.grid(frame);
        let v = self.view;
        let xs = xs.iter().map(|x| v.to_screen(Pt::new(*x, 0.0)).x).collect();
        let ys = ys.iter().map(|y| v.to_screen(Pt::new(0.0, *y)).y).collect();
        Some((a, xs, ys, v.to_screen(Pt::new(frame.x0, frame.y0))))
    }

    fn in_col_band(s: Pt, tl: Pt) -> bool {
        s.y >= tl.y - HEAD_BAND - HEAD_GAP && s.y < tl.y - HEAD_GAP + 1.0
    }

    fn in_row_band(s: Pt, tl: Pt) -> bool {
        s.x >= tl.x - HEAD_ROW_W - HEAD_GAP && s.x < tl.x - HEAD_GAP + 1.0
    }

    /// Spalten- (true) bzw. Zeilenkopf (false) der gezeigten Tabelle unter einem
    /// Bildschirmpunkt: (Rahmen, Spalte?, Nummer).
    pub fn table_head_at(&self, s: Pt) -> Option<(u64, bool, usize)> {
        let (a, xs, ys, tl) = self.head_geom()?;
        if Self::in_col_band(s, tl) {
            if let Some(c) = xs.windows(2).position(|w| s.x >= w[0] && s.x < w[1]) {
                return Some((a, true, c));
            }
        }
        if Self::in_row_band(s, tl) {
            if let Some(r) = ys.windows(2).position(|w| s.y >= w[0] && s.y < w[1]) {
                return Some((a, false, r));
            }
        }
        None
    }

    /// Griff zum Verschieben der gezeigten Tabelle (wo sich Spalten- und Zeilenköpfe treffen).
    pub fn table_grip_at(&self, s: Pt) -> Option<u64> {
        let (a, _, _, tl) = self.head_geom()?;
        // runder Knopf, unten bündig mit der Buchstabenleiste
        let y1 = tl.y - HEAD_GAP;
        let y0 = y1 - HEAD_BAND.max(HEAD_ROW_W);
        (Self::in_row_band(s, tl) && s.y >= y0 - 1.0 && s.y < y1 + 1.0).then_some(a)
    }

    /// Grenze zwischen zwei Spalten- bzw. Zeilenköpfen (zum Ziehen der Größe): (Rahmen,
    /// Spalte?, Nummer der Spalte/Zeile davor).
    pub fn table_edge_at(&self, s: Pt) -> Option<(u64, bool, usize)> {
        let (a, xs, ys, tl) = self.head_geom()?;
        let near = 4.0;
        if Self::in_col_band(s, tl) {
            if let Some(c) = (1..xs.len()).rev().find(|i| (s.x - xs[*i]).abs() <= near) {
                return Some((a, true, c - 1));
            }
        }
        if Self::in_row_band(s, tl) {
            if let Some(r) = (1..ys.len()).rev().find(|i| (s.y - ys[*i]).abs() <= near) {
                return Some((a, false, r - 1));
            }
        }
        None
    }

    /// Spalte bzw. Zeile auf Höhe eines Bildschirmpunkts (außerhalb: die erste bzw. letzte).
    fn head_index(edges: &[f64], v: f64) -> usize {
        let n = edges.len().saturating_sub(1).max(1);
        edges.windows(2).position(|w| v >= w[0] && v < w[1]).unwrap_or(
            if edges.first().is_some_and(|e| v < *e) {
                0
            } else {
                n - 1
            },
        )
    }

    /// Gewählte Zellen auf dem Bildschirm: (Rahmen, Bereich, Rechteck).
    fn pick_screen(&self) -> Option<(u64, (usize, usize, usize, usize), Rect)> {
        if self.cell_edit.is_some() {
            return None;
        }
        let (a, rg) = self.table_pick()?;
        let (_, xs, ys, _) = self.head_geom()?;
        let (r0, c0, r1, c1) = rg;
        Some((
            a,
            rg,
            Rect::from_points(Pt::new(xs[c0], ys[r0]), Pt::new(xs[c1 + 1], ys[r1 + 1])),
        ))
    }

    /// Ausfüllkästchen unten rechts an den gewählten Zellen (Bildschirmmitte).
    pub fn fill_handle(&self) -> Option<Pt> {
        let (_, _, r) = self.pick_screen()?;
        Some(Pt::new(r.x1, r.y1))
    }

    fn on_fill_handle(&self, s: Pt) -> bool {
        self.fill_handle()
            .is_some_and(|h| (s.x - h.x).abs() <= 5.0 && (s.y - h.y).abs() <= 5.0)
    }

    /// Auf dem Rand der gewählten Zellen (dort lassen sie sich verschieben)?
    fn on_pick_border(&self, s: Pt) -> bool {
        let Some((_, _, r)) = self.pick_screen() else {
            return false;
        };
        let d = 3.5;
        let outer = Rect::from_points(Pt::new(r.x0 - d, r.y0 - d), Pt::new(r.x1 + d, r.y1 + d));
        let inner = Rect::from_points(Pt::new(r.x0 + d, r.y0 + d), Pt::new(r.x1 - d, r.y1 - d));
        let inside = |q: &Rect| s.x >= q.x0 && s.x <= q.x1 && s.y >= q.y0 && s.y <= q.y1;
        inside(&outer) && !(inner.x1 > inner.x0 && inner.y1 > inner.y0 && inside(&inner))
    }

    /// Zelle (Zeile, Spalte) unter einem Dokumentpunkt; außerhalb der Tabelle die nächste,
    /// unten und rechts auch darüber hinaus (gedachte weitere Zeilen/Spalten gleicher Größe
    /// wie die letzte).
    fn cell_toward(&self, a: u64, p: Pt) -> Option<(isize, isize)> {
        let o = self.doc.object(a)?;
        let crate::smart::Smart::Table(t) = o.smart.as_deref()? else {
            return None;
        };
        let f = smart_frame(o)?;
        let (xs, ys) = t.grid(f);
        let idx = |edges: &[f64], v: f64| -> isize {
            let n = edges.len() - 1;
            if v < edges[0] {
                return 0;
            }
            if v >= edges[n] {
                let last = (edges[n] - edges[n - 1]).max(1.0);
                return (n - 1) as isize + ((v - edges[n]) / last).floor() as isize + 1;
            }
            edges
                .windows(2)
                .position(|w| v >= w[0] && v < w[1])
                .unwrap_or(n - 1) as isize
        };
        Some((idx(&ys, p.y), idx(&xs, p.x)))
    }

    /// Gewählte Zellen ausfüllen bzw. verschieben (beim Loslassen).
    fn finish_cells(&mut self, drag: &Drag) {
        let (a, mut t, frame) = match drag {
            Drag::CellFill { anchor, .. } | Drag::CellMove { anchor, .. } => {
                let Some(o) = self.doc.object(*anchor) else { return };
                let (Some(crate::smart::Smart::Table(t)), Some(f)) = (o.smart.as_deref(), smart_frame(o))
                else {
                    return;
                };
                (*anchor, t.clone(), f)
            }
            _ => return,
        };
        let (rows, cols) = (t.n_rows(), t.n_cols());
        let (sum_r, sum_c): (f64, f64) = (t.rows.iter().sum(), t.cols.iter().sum());
        let new_pick = match *drag {
            Drag::CellFill { range, ext, .. } => {
                if ext == (0, 0) {
                    return;
                }
                t.fill(range, ext.0, ext.1, crate::i18n::decimal_comma());
                let (r0, c0, r1, c1) = range;
                let (dr, dc) = ext;
                Pick::Cells(
                    (r0 as isize + dr.min(0)) as usize,
                    (c0 as isize + dc.min(0)) as usize,
                    (r1 as isize + dr.max(0)) as usize,
                    (c1 as isize + dc.max(0)) as usize,
                )
            }
            Drag::CellMove { range, off, .. } => {
                if !t.move_block(range, off.0, off.1) {
                    return;
                }
                let (r0, c0, r1, c1) = range;
                let m = |v: usize, d: isize| (v as isize + d) as usize;
                Pick::Cells(m(r0, off.0), m(c0, off.1), m(r1, off.0), m(c1, off.1))
            }
            _ => return,
        };
        // neue Zeilen/Spalten behalten ihre Größe: der Rahmen wächst
        let w = frame.width() * t.cols.iter().sum::<f64>() / sum_c.max(1e-9);
        let h = frame.height() * t.rows.iter().sum::<f64>() / sum_r.max(1e-9);
        let grown = (t.n_rows(), t.n_cols()) != (rows, cols);
        if !self.set_smart(a, crate::smart::Smart::Table(t)) {
            return;
        }
        if grown {
            if let Some(o) = self.doc.object_mut(a) {
                o.shape = Shape::Rect {
                    x: frame.x0,
                    y: frame.y0,
                    w,
                    h,
                    r: 0.0,
                };
            }
            self.rebuild_smart(a);
        }
        self.pick = Some((a, new_pick));
        if let Pick::Cells(r, c, _, _) = new_pick {
            self.cell_sel = Some((a, r, c));
        }
        self.selection = self.doc.group_members(a);
    }

    /// Ziehen an einer Grenze der Köpfe beginnen: die Spalte bzw. Zeile davor wird breiter
    /// oder schmaler (mit ihr alle ganz gewählten Spalten/Zeilen), die Tabelle wächst mit.
    fn table_size_press(&mut self, a: u64, col: bool, idx: usize, p: Pt) {
        self.end_text_edit();
        let Some(o) = self.doc.object(a) else { return };
        let (Some(crate::smart::Smart::Table(t)), Some(frame)) = (o.smart.as_deref(), smart_frame(o)) else {
            return;
        };
        let (nr, nc) = (t.n_rows(), t.n_cols());
        let (weights, len) = if col {
            (&t.cols, frame.width())
        } else {
            (&t.rows, frame.height())
        };
        let total: f64 = weights.iter().sum::<f64>().max(1e-9);
        let sizes: Vec<f64> = weights.iter().map(|w| w * len / total).collect();
        let targets: Vec<usize> = match self.active_pick().and_then(|pk| pk.cell_range(nr, nc)) {
            Some((r0, c0, r1, c1)) if col && r0 == 0 && r1 + 1 == nr && (c0..=c1).contains(&idx) => {
                (c0..=c1).collect()
            }
            Some((r0, c0, r1, c1)) if !col && c0 == 0 && c1 + 1 == nc && (r0..=r1).contains(&idx) => {
                (r0..=r1).collect()
            }
            _ => vec![idx],
        };
        self.selection = self.doc.group_members(a);
        self.begin_change();
        self.drag = Drag::TableSize {
            anchor: a,
            col,
            idx,
            targets,
            sizes,
            frame,
            start: p,
        };
    }

    /// Klick auf einen Spalten- bzw. Zeilenkopf: ganze Spalte/Zeile wählen (mit
    /// Umschalttaste bis dorthin erweitern).
    fn head_press(&mut self, a: u64, col: bool, i: usize, shift: bool) {
        self.end_text_edit();
        let Some(crate::smart::Smart::Table(t)) = self.smart_data(a) else {
            return;
        };
        let (nr, nc) = (t.n_rows(), t.n_cols());
        let corner = match self.pick {
            Some((pa, Pick::Cells(r, c, _, _))) if pa == a && shift => Some((r, c)),
            _ => None,
        };
        let pk = if col {
            Pick::Cells(0, corner.map_or(i, |x| x.1), nr - 1, i)
        } else {
            Pick::Cells(corner.map_or(i, |x| x.0), 0, i, nc - 1)
        };
        self.pick = Some((a, pk));
        self.cell_sel = Some((a, if col { 0 } else { i }, if col { i } else { 0 }));
        self.selection = self.doc.group_members(a);
        let start = match pk {
            Pick::Cells(r0, c0, _, _) => {
                if col {
                    c0
                } else {
                    r0
                }
            }
            _ => i,
        };
        self.drag = Drag::HeadSelect {
            anchor: a,
            col,
            start,
        };
    }

    /// Aussehen der gewählten Teile ändern (alle Zellen bzw. das Ganze, wenn nichts
    /// gewählt ist). `g` bekommt das bisherige Aussehen jedes Teils; festgehalten wird, was
    /// `g` daran ändert. `text`: eine Schriftänderung (sonst Fläche/Kontur).
    fn edit_looks(
        &mut self,
        anchor: u64,
        key: &str,
        text: bool,
        g: &dyn Fn(&mut crate::smart::Look),
    ) -> bool {
        use crate::smart::Smart;
        let Some(mut sm) = self.smart_data(anchor).cloned() else {
            return false;
        };
        let pick = self.active_pick();
        let comma = crate::i18n::decimal_comma();
        match &mut sm {
            Smart::Table(t) => {
                let looks = t.looks(comma);
                let (nr, nc) = (t.n_rows(), t.n_cols());
                let Some((r0, c0, r1, c1)) = pick
                    .and_then(|p| p.cell_range(nr, nc))
                    .or_else(|| (nr > 0 && nc > 0).then(|| (0, 0, nr - 1, nc - 1)))
                else {
                    return false;
                };
                for r in r0..=r1 {
                    for c in c0..=c1 {
                        let before = &looks[r][c];
                        let mut after = before.clone();
                        g(&mut after);
                        after.diff_into(before, &mut t.fmt[r][c]);
                    }
                }
            }
            Smart::Chart(ch) => match pick {
                None => {
                    if !text {
                        return false;
                    }
                    // das ganze Diagramm: alle Texte (Beschriftungen und Werte); eigene
                    // Abweichungen einzelner Texte weichen der neuen Einstellung
                    let mut changed = crate::smart::Fmt::default();
                    let before = ch.labels_look();
                    let mut after = before.clone();
                    g(&mut after);
                    after.diff_into(&before, &mut changed);
                    after.diff_into(&before, &mut ch.labels_fmt);
                    let before = ch.values_look(None);
                    let mut after = before.clone();
                    g(&mut after);
                    // die Ausrichtung gilt der Legende, nicht den Werten
                    after.align = before.align;
                    after.diff_into(&before, &mut changed);
                    after.diff_into(&before, &mut ch.values_fmt);
                    for (_, f) in ch.texts.iter_mut() {
                        f.clear_like(&changed);
                    }
                }
                Some(Pick::Text(k)) => {
                    if !text {
                        return false;
                    }
                    let before = ch.text_look(k);
                    let mut after = before.clone();
                    g(&mut after);
                    after.diff_into(&before, ch.text_fmt_mut(k));
                }
                Some(Pick::Point(si, i)) => {
                    if text {
                        return false;
                    }
                    let before = ch.point_look(si, i);
                    let mut after = before.clone();
                    g(&mut after);
                    after.diff_into(&before, ch.point_fmt_mut(si, i));
                }
                Some(Pick::Series(si)) => {
                    if text {
                        return false;
                    }
                    let before = ch.series_look(si);
                    let mut after = before.clone();
                    g(&mut after);
                    after.diff_into(&before, ch.series_fmt_mut(si));
                }
                Some(Pick::Cells(..)) => return false,
            },
        }
        if self.smart_data(anchor) == Some(&sm) {
            return false;
        }
        // Tippen in einer Zelle erst übernehmen (die Teile werden neu gebaut) – nur, wenn sich
        // wirklich etwas ändert; dann mit den übernommenen Daten noch einmal
        if self.cell_edit.is_some() {
            self.end_text_edit();
            return self.edit_looks(anchor, key, text, g);
        }
        self.checkpoint(Some(key));
        if let Some(o) = self.doc.object_mut(anchor) {
            o.smart = Some(Box::new(sm));
        }
        self.rebuild_smart(anchor);
        self.selection = self.doc.group_members(anchor);
        true
    }

    /// Fläche/Kontur aus der Eigenschaftenleiste für eine Tabelle bzw. ein Diagramm. Ein
    /// Diagramm als Ganzes ändert seinen Rahmen (Hintergrund).
    fn smart_style(&mut self, anchor: u64, key: &str, f: &dyn Fn(&mut Style)) -> bool {
        let chart = matches!(self.smart_data(anchor), Some(crate::smart::Smart::Chart(_)));
        if chart && self.active_pick().is_none() {
            self.checkpoint(Some(key));
            if let Some(o) = self.doc.object_mut(anchor) {
                o.style.fold_opacity();
                f(&mut o.style);
                o.style.fold_opacity();
            }
            return true;
        }
        self.edit_looks(anchor, key, false, &|l| {
            let mut st = l.style();
            f(&mut st);
            l.fill = st.fill;
            l.stroke = st.stroke;
            l.width = st.width;
        })
    }

    /// Schrift (Familie, Größe, fett, kursiv, Ausrichtung) für eine Tabelle bzw. ein
    /// Diagramm.
    fn smart_text(&mut self, anchor: u64, key: &str, f: &dyn Fn(&mut TextDefaults)) -> bool {
        self.edit_looks(anchor, key, true, &|l| {
            let mut t = TextDefaults {
                family: l.family.clone(),
                size: l.size,
                bold: l.bold,
                italic: l.italic,
                fmt: l.text_format(),
            };
            f(&mut t);
            set_look_text(l, &t);
        })
    }

    fn smart_char(&mut self, anchor: u64, key: &str, f: &dyn Fn(&mut CharFmt)) -> bool {
        self.edit_looks(anchor, key, true, &|l| {
            let mut cf = CharFmt {
                family: l.family.clone(),
                size: l.size,
                bold: l.bold,
                weight: crate::model::effective_weight(l.weight, l.bold),
                italic: l.italic,
                underline: l.underline,
                strike: l.strike,
                color: None,
            };
            let w0 = cf.weight;
            cf.change(f);
            l.family = cf.family;
            l.size = cf.size.max(1.0);
            l.bold = cf.bold;
            if cf.weight != w0 {
                l.weight = cf.weight;
            }
            l.italic = cf.italic;
            l.underline = cf.underline;
            l.strike = cf.strike;
        })
    }

    /// Textfarbe der gewählten Zellen bzw. der Schrift eines Diagramms.
    pub fn set_smart_text_color(&mut self, c: Option<Color>) -> bool {
        let (Some(a), Some(c)) = (self.smart_target(), c) else {
            return false;
        };
        self.edit_looks(a, "smart-text-color", true, &move |l| l.color = c)
    }

    pub fn set_smart_text_alpha(&mut self, alpha: f64) -> bool {
        let Some(a) = self.smart_target() else {
            return false;
        };
        let alpha = alpha.clamp(0.0, 1.0);
        self.edit_looks(a, "smart-text-alpha", true, &move |l| l.color.a = alpha)
    }

    /// Tabelle bzw. Diagramm in gewöhnliche Formen und Texte umwandeln: die Daten fallen
    /// weg, die Teile bleiben als Gruppe (ausgewählt).
    pub fn release_smart(&mut self) -> bool {
        let Some(a) = self.smart_target() else {
            return false;
        };
        self.end_text_edit();
        let members = self.doc.group_members(a);
        self.checkpoint(None);
        // ein unsichtbarer Rahmen wird nicht mehr gebraucht
        let invisible = self
            .doc
            .object(a)
            .is_some_and(|o| o.style.fill.is_none() && o.style.stroke.is_none() && o.style.shadow.is_none());
        if let Some(o) = self.doc.object_mut(a) {
            o.smart = None;
        }
        if invisible {
            self.doc.remove_object(a);
        }
        self.selection = members
            .into_iter()
            .filter(|id| self.doc.object(*id).is_some())
            .collect();
        self.pick = None;
        self.cell_sel = None;
        self.layers_version += 1;
        true
    }

    /// Eigenschaftenleiste für eine Tabelle bzw. ein Diagramm: Aussehen des gewählten Teils.
    fn smart_info(&self, info: &mut ContextInfo) {
        use crate::smart::Smart;
        let Some(a) = self.smart_target() else {
            return;
        };
        let Some(sm) = self.smart_data(a) else {
            return;
        };
        let pick = self.active_pick();
        let comma = crate::i18n::decimal_comma();
        let (look, paint, text, frame_style) = match sm {
            Smart::Table(t) => {
                let (nr, nc) = (t.n_rows(), t.n_cols());
                let (r, c) = match pick.and_then(|p| p.cell_range(nr, nc)) {
                    Some((r0, c0, _, _)) => (r0, c0),
                    // ganze Tabelle: eine Zelle unter dem Kopf zeigt, wie sie aussieht
                    None => (1.min(nr.saturating_sub(1)), 0),
                };
                let Some(l) = t.looks(comma).get(r).and_then(|row| row.get(c)).cloned() else {
                    return;
                };
                (l, true, true, None)
            }
            Smart::Chart(ch) => match pick {
                None => (
                    ch.labels_look(),
                    true,
                    !matches!(
                        ch.kind,
                        crate::smart::ChartKind::Pie | crate::smart::ChartKind::Donut
                    ) || ch.legend
                        || ch.values,
                    self.doc.object(a).map(|o| o.style.clone()),
                ),
                Some(Pick::Point(si, i)) => (ch.point_look(si, i), true, false, None),
                Some(Pick::Series(si)) => (ch.series_look(si), true, false, None),
                Some(Pick::Text(k)) => (ch.text_look(k), false, true, None),
                Some(Pick::Cells(..)) => (ch.labels_look(), true, true, None),
            },
        };
        info.style = frame_style.unwrap_or_else(|| look.style());
        info.font = TextDefaults {
            family: look.family.clone(),
            size: look.size,
            bold: look.bold,
            italic: look.italic,
            fmt: look.text_format(),
        };
        info.font_mixed = FmtMixed::default();
        info.show_text = text;
        info.show_style = true;
        info.show_fill = paint;
        info.show_stroke = paint;
        info.is_text = false;
        info.text = None;
        info.text_frame = None;
        info.text_offset = None;
        info.show_markers = false;
        info.show_list = false;
        info.show_valign = matches!(sm, Smart::Table(_));
        info.can_convert = false;
        info.image = None;
        info.poly = None;
        info.show_radius = false;
        info.pick = pick;
        info.smart_look = Some(look);
        info.smart_paint = paint;
        info.smart_text = text;
    }

    /// Textfelder machen Objekten Platz, die vor ihnen liegen und Text wegdrängen (Effekt
    /// „Text wegdrängen“; Bilder, Tabellen und Diagramme haben ihn von sich aus): Aussparungen
    /// (`TextFrame::holes`) neu bestimmen, in der Form der Objekte (der Abstand ist der
    /// Innenabstand des Feldes). Kein Schritt zum Rückgängigmachen; liefert, ob sich etwas
    /// geändert hat.
    pub fn update_flow(&mut self) -> bool {
        // nichts Wesentliches geändert (Lage und Größe der Objekte, Textfelder): nichts zu tun
        // – das wird bei jedem Zeichnen gefragt
        let key = self.flow_key();
        if key == self.flow_seen {
            return false;
        }
        self.flow_seen = key;
        // Hindernisse in Zeichenreihenfolge: (Ebene, Stelle, Umriss und Rechteck im Dokument)
        let obstacles: Vec<(usize, usize, Vec<SubPath>, Rect)> = self
            .wrap_obstacles()
            .into_iter()
            .map(|(li, oi, _, subs, b)| (li, oi, subs, b))
            .collect();
        let mut changed = false;
        for li in 0..self.doc.layers.len() {
            // Text in Tabellen und Diagrammen weicht nichts aus
            let smart_groups: Vec<u64> = self.doc.layers[li]
                .objects
                .iter()
                .filter(|o| o.smart.is_some())
                .map(|o| o.inner_group())
                .collect();
            for oi in 0..self.doc.layers[li].objects.len() {
                let o = &self.doc.layers[li].objects[oi];
                let Shape::TextArea { xf, w, h, frame, .. } = &o.shape else {
                    continue;
                };
                let in_smart = !smart_groups.is_empty() && o.chain().iter().any(|g| smart_groups.contains(g));
                if (frame.no_flow || in_smart) && frame.holes.is_empty() {
                    continue;
                }
                let holes = if frame.no_flow || in_smart || obstacles.is_empty() {
                    Vec::new()
                } else {
                    // nur Objekte davor, die das Feld berühren
                    let corners =
                        [(0.0, 0.0), (*w, 0.0), (*w, *h), (0.0, *h)].map(|(x, y)| xf.apply(Pt::new(x, y)));
                    let mut area = Rect::from_points(corners[0], corners[2]);
                    for c in corners {
                        area = area.union(&Rect::from_points(c, c));
                    }
                    let shapes: Vec<&Vec<SubPath>> = obstacles
                        .iter()
                        .filter(|x| (x.0, x.1) > (li, oi) && x.3.intersects(&area))
                        .map(|x| &x.2)
                        .collect();
                    if shapes.is_empty() {
                        Vec::new()
                    } else {
                        flow_holes(xf, *w, *h, &shapes)
                    }
                };
                if frame.holes != holes {
                    if let Shape::TextArea { frame, .. } = &mut self.doc.layers[li].objects[oi].shape {
                        frame.holes = holes;
                    }
                    changed = true;
                }
            }
        }
        if changed {
            self.rev += 1;
        }
        changed
    }

    /// Drängt die Auswahl Text weg (für die Eigenschaftenleiste): beim ersten Objekt bzw.
    /// beim Rahmen einer gezeigten Tabelle oder eines Diagramms.
    pub fn wrap_state(&self) -> Option<bool> {
        if let Some(a) = self.smart_target() {
            return self.doc.object(a).map(pushes_text);
        }
        self.selected_objects().first().map(|o| pushes_text(o))
    }

    /// Effekt „Text wegdrängen“ an- bzw. abschalten (Tabellen und Diagramme: ihr Rahmen).
    pub fn set_wrap(&mut self, on: bool) -> bool {
        let ids: Vec<u64> = match self.smart_target() {
            Some(a) => vec![a],
            None => self.selected_objects().iter().map(|o| o.id).collect(),
        };
        if ids.is_empty() {
            return false;
        }
        self.checkpoint(Some("wrap"));
        for id in ids {
            if let Some(o) = self.doc.object_mut(id) {
                let default = o.smart.is_some() || matches!(o.shape, Shape::Image { .. });
                o.style.fx.wrap = if on == default { None } else { Some(on) };
            }
        }
        true
    }

    /// Objekte, deren Einstellungen zum Wegdrängen gelten: die Auswahl bzw. der Rahmen einer
    /// gezeigten Tabelle oder eines Diagramms.
    fn wrap_targets(&self) -> Vec<u64> {
        match self.smart_target() {
            Some(a) => vec![a],
            None => self.selected_objects().iter().map(|o| o.id).collect(),
        }
    }

    /// Text wegdrängen: Abstand des Textes zum Objekt.
    pub fn set_wrap_gap(&mut self, gap: f64) -> bool {
        let gap = gap.clamp(0.0, 1000.0);
        let ids: Vec<u64> = self
            .wrap_targets()
            .into_iter()
            .filter(|id| {
                self.doc
                    .object(*id)
                    .is_some_and(|o| (o.style.fx.wrap_gap() - gap).abs() > 1e-9)
            })
            .collect();
        if ids.is_empty() {
            return false;
        }
        self.checkpoint(Some("wrap-gap"));
        for id in ids {
            if let Some(o) = self.doc.object_mut(id) {
                o.style.fx.wrap_gap = Some(gap);
            }
        }
        true
    }

    /// Text wegdrängen: als Rechteck um das Objekt bzw. entlang seines Umrisses.
    pub fn set_wrap_box(&mut self, boxed: bool) -> bool {
        let ids: Vec<u64> = self
            .wrap_targets()
            .into_iter()
            .filter(|id| self.doc.object(*id).is_some_and(|o| o.style.fx.wrap_box != boxed))
            .collect();
        if ids.is_empty() {
            return false;
        }
        self.checkpoint(None);
        for id in ids {
            if let Some(o) = self.doc.object_mut(id) {
                o.style.fx.wrap_box = boxed;
            }
        }
        true
    }

    /// Kennung für alles, wovon die Aussparungen der Textfelder abhängen.
    fn flow_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hs = std::collections::hash_map::DefaultHasher::new();
        self.rev.hash(&mut hs);
        for (li, l) in self.doc.layers.iter().enumerate() {
            (li, l.visible).hash(&mut hs);
            for o in &l.objects {
                let text_area = matches!(o.shape, Shape::TextArea { .. });
                if !(text_area || pushes_text(o)) {
                    continue;
                }
                o.id.hash(&mut hs);
                if let Shape::TextArea { xf, w, h, frame, .. } = &o.shape {
                    for v in [xf.a, xf.b, xf.c, xf.d, xf.e, xf.f, *w, *h] {
                        v.to_bits().hash(&mut hs);
                    }
                    (frame.no_flow, frame.path.is_some(), frame.holes.len()).hash(&mut hs);
                }
                if pushes_text(o) {
                    if let Some(b) = o.shape.geom_bbox() {
                        for v in [b.x0, b.y0, b.x1, b.y1] {
                            v.to_bits().hash(&mut hs);
                        }
                    }
                    format!("{:?}{}", o.style.fx.wrap_gap, o.style.fx.wrap_box).hash(&mut hs);
                    o.angle.to_bits().hash(&mut hs);
                }
            }
        }
        hs.finish()
    }

    /// Flächen, aus denen die gewählten Objekte Text wegdrängen (zum gestrichelten Zeigen
    /// beim Bearbeiten).
    pub fn wrap_preview(&self) -> Vec<Vec<SubPath>> {
        if self.read_only || self.doc.view == ViewMode::Notebook {
            return Vec::new();
        }
        let sel = self.wrap_targets();
        if sel.is_empty() {
            return Vec::new();
        }
        self.wrap_obstacles()
            .into_iter()
            .filter(|x| x.2.iter().any(|id| sel.contains(id)))
            .map(|x| x.3)
            .collect()
    }

    /// Alles, was Text wegdrängt, in Zeichenreihenfolge: (Ebene, Stelle, beteiligte Objekte,
    /// Fläche mit Abstand, ihr Rechteck). Gruppierte Objekte, die als Rechteck wegdrängen,
    /// bilden zusammen ein Rechteck (an der Stelle ihres vordersten Objekts).
    fn wrap_obstacles(&self) -> Vec<(usize, usize, Vec<u64>, Vec<SubPath>, Rect)> {
        let bbox_of = |subs: &Vec<SubPath>| {
            Shape::Path {
                subpaths: subs.clone(),
                evenodd: false,
            }
            .geom_bbox()
            .filter(|b| b.width() > 0.0 || b.height() > 0.0)
        };
        let mut out = Vec::new();
        // Gruppe → (Ebene, Stelle, Objekte, Rechteck, Abstand)
        let mut groups: Vec<(u64, usize, usize, Vec<u64>, Rect, f64)> = Vec::new();
        for (li, l) in self.doc.layers.iter().enumerate() {
            if !l.visible {
                continue;
            }
            let smart_groups: Vec<u64> = l
                .objects
                .iter()
                .filter(|o| o.smart.is_some())
                .map(|o| o.inner_group())
                .collect();
            for (oi, o) in l.objects.iter().enumerate() {
                if o.mask || !pushes_text(o) {
                    continue;
                }
                // Teile von Tabellen und Diagrammen zählen nur über ihren Rahmen
                if o.smart.is_none()
                    && !smart_groups.is_empty()
                    && o.chain().iter().any(|g| smart_groups.contains(g))
                {
                    continue;
                }
                if o.group != 0 && o.smart.is_none() && o.style.fx.wrap_box {
                    let Some(b) = wrap_outline(o).as_ref().and_then(bbox_of) else {
                        continue;
                    };
                    let gap = o.style.fx.wrap_gap();
                    match groups.iter_mut().find(|g| g.0 == o.group && g.1 == li) {
                        Some(g) => {
                            g.2 = g.2.max(oi);
                            g.3.push(o.id);
                            g.4 = g.4.union(&b);
                            g.5 = g.5.max(gap);
                        }
                        None => groups.push((o.group, li, oi, vec![o.id], b, gap)),
                    }
                    continue;
                }
                if let Some(subs) = wrap_area(o) {
                    if let Some(b) = bbox_of(&subs) {
                        out.push((li, oi, vec![o.id], subs, b));
                    }
                }
            }
        }
        for (_, li, oi, ids, b, gap) in groups {
            let r = Rect {
                x0: b.x0 - gap,
                y0: b.y0 - gap,
                x1: b.x1 + gap,
                y1: b.y1 + gap,
            };
            let subs = vec![SubPath {
                nodes: [
                    Pt::new(r.x0, r.y0),
                    Pt::new(r.x1, r.y0),
                    Pt::new(r.x1, r.y1),
                    Pt::new(r.x0, r.y1),
                ]
                .iter()
                .map(|p| Node::corner(*p))
                .collect(),
                closed: true,
            }];
            out.push((li, oi, ids, subs, r));
        }
        out.sort_by_key(|x| (x.0, x.1));
        out
    }

    /// Diagramme, die ihre Daten aus dieser Tabelle holen, nachziehen (ohne eigenen Schritt).
    fn sync_charts(&mut self, table: u64) {
        let Some(crate::smart::Smart::Table(t)) = self.smart_data(table).cloned() else {
            return;
        };
        if t.key == 0 {
            return;
        }
        let charts: Vec<u64> = self
            .doc
            .layers
            .iter()
            .flat_map(|l| &l.objects)
            .filter(
                |o| matches!(o.smart.as_deref(), Some(crate::smart::Smart::Chart(c)) if c.source == t.key),
            )
            .map(|o| o.id)
            .collect();
        for id in charts {
            if let Some(o) = self.doc.object_mut(id) {
                if let Some(crate::smart::Smart::Chart(c)) = o.smart.as_deref_mut() {
                    if c.data == t.cells && c.header == t.header {
                        continue;
                    }
                    c.data = t.cells.clone();
                    c.header = t.header;
                }
            }
            self.rebuild_smart(id);
        }
    }

    /// Neue Kennung für eine Tabelle (Verknüpfung mit Diagrammen).
    fn new_table_key(&self) -> u64 {
        self.doc
            .layers
            .iter()
            .flat_map(|l| &l.objects)
            .filter_map(|o| match o.smart.as_deref() {
                Some(crate::smart::Smart::Table(t)) => Some(t.key),
                Some(crate::smart::Smart::Chart(c)) => Some(c.source),
                None => None,
            })
            .max()
            .unwrap_or(0)
            + 1
    }

    /// Diagramm aus einer Tabelle: rechts daneben, mit ihr verknüpft (ändert sich die
    /// Tabelle, folgt das Diagramm). Wird ausgewählt.
    pub fn create_chart_from_table(&mut self, table: u64) -> Option<u64> {
        let o = self.doc.object(table)?;
        let crate::smart::Smart::Table(mut t) = o.smart.as_deref()?.clone() else {
            return None;
        };
        let frame = smart_frame(o)?;
        let (li, _) = self.doc.locate(table)?;
        self.checkpoint(None);
        if t.key == 0 {
            t.key = self.new_table_key();
            if let Some(o) = self.doc.object_mut(table) {
                o.smart = Some(Box::new(crate::smart::Smart::Table(t.clone())));
            }
        }
        let mut c = crate::smart::Chart::sample(crate::smart::ChartKind::Pie);
        c.data = t.cells.clone();
        c.header = t.header;
        // passende Art: Zeitreihe → Linien, mehrere Reihen → Balken, Anteile → Kreis
        c.kind = c.suggest_kind(crate::i18n::decimal_comma());
        c.source = t.key;
        c.family = t.family.clone();
        let h = frame.height().max(180.0);
        let f = Rect::from_xywh(frame.x1 + 24.0, frame.y0, h * 1.5, h);
        let id = self.insert_smart_at(li, crate::smart::Smart::Chart(c), f);
        self.cell_sel = None;
        Some(id)
    }

    /// Daten eines Diagramms als Tabelle darunter (verknüpft), z. B. wenn die Tabelle
    /// gelöscht wurde. Gibt die neue Tabelle zurück (ausgewählt).
    pub fn table_from_chart(&mut self, chart: u64) -> Option<u64> {
        let o = self.doc.object(chart)?;
        let crate::smart::Smart::Chart(mut c) = o.smart.as_deref()?.clone() else {
            return None;
        };
        let frame = smart_frame(o)?;
        let (li, _) = self.doc.locate(chart)?;
        self.checkpoint(None);
        let key = self.new_table_key();
        c.source = key;
        if let Some(o) = self.doc.object_mut(chart) {
            o.smart = Some(Box::new(crate::smart::Smart::Chart(c.clone())));
        }
        let mut t = crate::smart::Table::with_cells(c.data.clone());
        t.key = key;
        t.header = c.header;
        t.family = c.family.clone();
        let (rows, cols) = (t.n_rows() as f64, t.n_cols() as f64);
        let f = Rect::from_xywh(frame.x0, frame.y1 + 24.0, (cols * 90.0).max(180.0), rows * 26.0);
        Some(self.insert_smart_at(li, crate::smart::Smart::Table(t), f))
    }

    fn data_press(&mut self, p: Pt, m: &dyn Metrics, ended_cell: bool) {
        // in eine vorhandene Tabelle: Zelle bearbeiten; Diagramm: auswählen
        if let Some((a, r, c)) = self.hit(p, m).and_then(|id| self.table_cell_at(id, p)) {
            self.begin_cell_edit(a, r, c);
            return;
        }
        if let Some(a) = self.hit(p, m).and_then(|id| self.smart_anchor_of(id)) {
            self.selection = self.doc.group_members(a);
            return;
        }
        if ended_cell {
            return;
        }
        let Some(li) = self.target_layer() else { return };
        let p = self.snap(p);
        self.checkpoint(None);
        let t = crate::smart::Table::empty(3, 3);
        let id = self.insert_smart_at(
            li,
            crate::smart::Smart::Table(t),
            Rect::from_xywh(p.x, p.y, 1.0, 1.0),
        );
        self.drag = Drag::DataCreate { id, start: p };
    }

    /// Gehört das Objekt zu einer Tabelle oder einem Diagramm (Rahmen oder gebautes Teil)?
    pub fn in_smart(&self, id: u64) -> bool {
        let Some((li, _)) = self.doc.locate(id) else {
            return false;
        };
        let Some(o) = self.doc.object(id) else {
            return false;
        };
        if o.smart.is_some() {
            return true;
        }
        let chain = o.chain();
        !chain.is_empty()
            && self.doc.layers[li]
                .objects
                .iter()
                .any(|a| a.smart.is_some() && chain.contains(&a.inner_group()))
    }

    /// Rahmen (mit Daten) der ausgewählten Tabellen und Diagramme.
    pub fn smart_anchors(&self) -> Vec<u64> {
        self.selected_objects()
            .iter()
            .filter(|o| o.smart.is_some())
            .map(|o| o.id)
            .collect()
    }

    /// Daten einer Tabelle bzw. eines Diagramms.
    pub fn smart_data(&self, anchor: u64) -> Option<&crate::smart::Smart> {
        self.doc.object(anchor).and_then(|o| o.smart.as_deref())
    }

    /// Daten ändern und neu aufbauen (ein Schritt zum Rückgängigmachen).
    pub fn set_smart(&mut self, anchor: u64, smart: crate::smart::Smart) -> bool {
        if self.smart_data(anchor).map_or(true, |d| *d == smart) {
            return false;
        }
        self.checkpoint(None);
        if let Some(o) = self.doc.object_mut(anchor) {
            o.smart = Some(Box::new(smart));
        }
        let done = self.rebuild_smart(anchor);
        self.sync_charts(anchor);
        done
    }

    /// Die Teile einer Tabelle bzw. eines Diagramms aus seinen Daten neu bauen (ohne
    /// Rückgängig-Punkt): alle übrigen Objekte seiner Gruppe werden ersetzt.
    pub fn rebuild_smart(&mut self, anchor: u64) -> bool {
        let Some((li, _)) = self.doc.locate(anchor) else {
            return false;
        };
        let Some(a) = self.doc.object(anchor).cloned() else {
            return false;
        };
        let Some(smart) = a.smart.as_deref() else {
            return false;
        };
        let Some(frame) = smart_frame(&a) else {
            return false;
        };
        // Tabellen dreht man nicht (Zellen werden im ungedrehten Raster gewählt): eine
        // gedrehte wieder gerade stellen; Diagramme: Teile mitdrehen
        let mut turn = smart_turn(&a);
        if turn.is_some() && matches!(smart, crate::smart::Smart::Table(_)) {
            if let Some(o) = self.doc.object_mut(anchor) {
                o.shape = Shape::Rect {
                    x: frame.x0,
                    y: frame.y0,
                    w: frame.width(),
                    h: frame.height(),
                    r: 0.0,
                };
                o.angle = 0.0;
            }
            turn = None;
        }
        let angle = if turn.is_some() { a.angle } else { 0.0 };
        // aufgelöste Gruppe: wieder eine eigene
        let (group, sub) = if a.group == 0 {
            let g = self.doc.max_group() + 1;
            if let Some(o) = self.doc.object_mut(anchor) {
                o.group = g;
            }
            (g, Vec::new())
        } else {
            (a.group, a.sub.clone())
        };
        let inner = sub.last().copied().unwrap_or(group);
        // Tabellen: die Zellen zeichnen sich selbst, der Rahmen bleibt leer (ältere
        // Dateien hatten ihn gefüllt)
        if matches!(smart, crate::smart::Smart::Table(_)) {
            if let Some(o) = self.doc.object_mut(anchor) {
                o.style.fill = None;
                o.style.stroke = None;
            }
        }
        let was_selected = self.selection.contains(&anchor);
        let old: Vec<u64> = self.doc.layers[li]
            .objects
            .iter()
            .filter(|o| o.id != anchor && o.chain().contains(&inner))
            .map(|o| o.id)
            .collect();
        self.doc.layers[li].objects.retain(|o| !old.contains(&o.id));
        self.selection.retain(|id| !old.contains(id));
        let parts = crate::smart::build(smart, frame, crate::i18n::decimal_comma());
        let Some(pos) = self.doc.layers[li].objects.iter().position(|o| o.id == anchor) else {
            return false;
        };
        let mut new_ids = Vec::new();
        for (k, (mut shape, style)) in parts.into_iter().enumerate() {
            if let Some(t) = &turn {
                shape.transform(t);
            }
            let id = self.doc.alloc_id();
            new_ids.push(id);
            self.doc.layers[li].objects.insert(
                pos + 1 + k,
                Object {
                    group,
                    sub: sub.clone(),
                    mask: false,
                    locked: a.locked,
                    smart: None,
                    name: String::new(),
                    id,
                    shape,
                    style,
                    angle,
                },
            );
        }
        if was_selected {
            self.selection.extend(new_ids);
        }
        self.layers_version += 1;
        true
    }

    fn insert_object(&mut self, li: usize, shape: Shape, style: Style) -> u64 {
        let id = self.doc.alloc_id();
        self.doc.layers[li].objects.push(Object {
            group: 0,
            sub: Vec::new(),
            mask: false,
            locked: false,
            smart: None,
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
            fill_link: None,
            // die Kontur kommt aus der Kontur oder der Füllung der Vorgabe
            stroke_link: if d.stroke.is_some() {
                d.stroke_link.clone()
            } else {
                d.fill_link.clone()
            },
        }
    }

    // ───────────── Zeigerereignisse ─────────────

    pub fn press(&mut self, s: Pt, mods: Mods, pan: bool, m: &dyn Metrics) {
        // Vollbild der Präsentation: ein Klick blättert weiter (Textmarker und Laserpointer
        // zeichnen)
        if self.presenting && matches!(self.view_tool, ViewTool::Select | ViewTool::Hand) {
            let i = self.page_index().unwrap_or(0);
            self.go_page(i + 1, false);
            return;
        }
        let pan = pan || (self.read_only && self.view_tool == ViewTool::Hand);
        // Klick auf die Zeichenfläche während einer Vorschau: sie gilt als angewendet
        // (Verschieben der Ansicht nicht – dabei sieht man sie sich ja an)
        if self.preview.is_some() && !pan && !self.space_down {
            self.preview_end(true);
        }
        self.hints.borrow_mut().clear();
        self.press_at = Some(s);
        // Zeichenwerkzeuge: Ziele vor dem Klick sammeln (alle vorhandenen Objekte), damit
        // schon der erste Punkt einrastet und das neue Objekt nicht an sich selbst klebt.
        let creating = !matches!(self.tool, Tool::Select | Tool::Node) && !pan && !self.space_down;
        if self.read_only {
            // Ansehen: kein Einrasten
            self.snap_ctx = None;
            self.press_inner(s, mods, pan, m);
            return;
        }
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
        if self.read_only {
            match self.view_tool {
                ViewTool::Select => self.view_select_press(p, mods, m),
                ViewTool::Highlight => {
                    // Strichstärke nach der Schrift unter dem Anfang
                    let size = self
                        .hit(p, m)
                        .and_then(|id| self.doc.object(id))
                        .and_then(|o| o.shape.text_fields().map(|t| t.2 * text_scale(&o.shape)));
                    self.highlight_width = size.map(|s| s * 1.25).unwrap_or(14.0).clamp(4.0, 80.0);
                    self.drag = Drag::Pencil { pts: vec![p] };
                }
                ViewTool::Laser => {
                    self.laser_alive();
                    self.laser.push((p, Instant::now()));
                    self.drag = Drag::Laser;
                }
                ViewTool::Hand => {}
            }
            return;
        }
        // Notizheft: am Lineal die Seitenränder ziehen (Hilfslinien gibt es hier nicht)
        if self.doc.view == ViewMode::Notebook && self.ruler > 0.0 && (s.x < self.ruler || s.y < self.ruler) {
            if let Some((gutter, edge, grab)) = self.column_handle_at(s) {
                self.drag = Drag::Column { gutter, edge, grab };
            } else if let Some((side, page)) = self.margin_handle_at(s) {
                self.drag = Drag::Margin { side, page };
            }
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
        // Formel schreiben: Klick in eine andere Zelle derselben Tabelle fügt ihren Bezug ein
        if let (Some(ce), Some(te)) = (self.cell_edit.clone(), self.text_edit) {
            if matches!(self.tool, Tool::Select | Tool::Data) {
                if let Some((_, r, c)) = self.table_cell_at(ce.anchor, p) {
                    let text = self
                        .doc
                        .object(te.id)
                        .and_then(|o| o.shape.text_fields())
                        .map(|t| t.0.to_string())
                        .unwrap_or_default();
                    let pointing =
                        self.live_ref_point().is_some() || crate::sheet::wants_ref(&text, te.range().0);
                    if (r, c) != (ce.row, ce.col) && text.starts_with('=') && pointing {
                        self.put_ref(r, c, r, c);
                        self.drag = Drag::RefPick {
                            anchor: ce.anchor,
                            r,
                            c,
                        };
                        return;
                    }
                }
            }
        }
        // Klick in die Zelle, in die gerade getippt wird: Cursor setzen (z. B. der zweite
        // Klick eines Doppelklicks, der das Tippen begonnen hat)
        if let (Some(ce), Some(te)) = (self.cell_edit.clone(), self.text_edit) {
            if self.table_cell_at(ce.anchor, p) == Some((ce.anchor, ce.row, ce.col)) {
                if let Some(o) = self.doc.object(te.id) {
                    let i = m.text_index(o, p).unwrap_or(te.caret);
                    self.text_edit = Some(TextEdit {
                        id: te.id,
                        caret: i,
                        anchor: if mods.shift { te.anchor } else { i },
                    });
                    self.drag = Drag::TextSelect { id: te.id };
                }
                return;
            }
        }
        // zweiter Klick kurz nach dem ersten in dieselbe Zelle einer Tabelle: Tippen beginnen
        // (unabhängig davon, in welcher Reihenfolge das Fenster Klicks meldet)
        if self.tool == Tool::Select && self.cell_edit.is_none() && !mods.shift {
            let cell = self
                .hit(p, m)
                .and_then(|id| self.smart_anchor_of(id))
                .and_then(|a| self.table_cell_at(a, p));
            if let Some((a, r, c)) = cell {
                let now = Instant::now();
                let again = self.last_cell_click.is_some_and(|(t, s0, a0, r0, c0)| {
                    (a0, r0, c0) == (a, r, c)
                        && now.duration_since(t) < Duration::from_millis(500)
                        && s0.dist(s) < 6.0
                });
                if again {
                    self.last_cell_click = None;
                    if self.begin_cell_edit(a, r, c) {
                        if let Some(te) = self.text_edit {
                            if let Some(i) = self.doc.object(te.id).and_then(|o| m.text_index(o, p)) {
                                self.text_edit = Some(TextEdit {
                                    id: te.id,
                                    caret: i,
                                    anchor: i,
                                });
                            }
                            self.drag = Drag::TextSelect { id: te.id };
                        }
                        return;
                    }
                }
                self.last_cell_click = Some((now, s, a, r, c));
            } else {
                self.last_cell_click = None;
            }
        }
        // Tabelle: Griff zum Verschieben, Grenzen und Köpfe der Spalten und Zeilen
        if matches!(self.tool, Tool::Select | Tool::Data) {
            // Ausfüllkästchen und Rand der gewählten Zellen
            if self.on_fill_handle(s) || self.on_pick_border(s) {
                if let Some((a, range, _)) = self.pick_screen() {
                    if self.on_fill_handle(s) {
                        self.drag = Drag::CellFill {
                            anchor: a,
                            range,
                            ext: (0, 0),
                        };
                    } else {
                        let (r0, c0, r1, c1) = range;
                        let g = self.cell_toward(a, p).unwrap_or((r0 as isize, c0 as isize));
                        let grab = (
                            g.0.clamp(r0 as isize, r1 as isize) as usize,
                            g.1.clamp(c0 as isize, c1 as isize) as usize,
                        );
                        self.drag = Drag::CellMove {
                            anchor: a,
                            range,
                            grab,
                            off: (0, 0),
                        };
                    }
                    return;
                }
            }
            if let Some(a) = self.table_grip_at(s) {
                self.end_text_edit();
                self.selection = self.doc.group_members(a);
                if !self.selection_locked() {
                    self.begin_change();
                    self.drag = Drag::Move {
                        start: p,
                        orig: self.originals(),
                        bbox: self.selection_bbox(m),
                        copies: Vec::new(),
                    };
                }
                return;
            }
            if let Some((a, col, i)) = self.table_edge_at(s) {
                self.table_size_press(a, col, i, p);
                return;
            }
            if let Some((a, col, i)) = self.table_head_at(s) {
                self.head_press(a, col, i, mods.shift);
                return;
            }
        }
        self.click_pick = None;
        // Tippen in einer Zelle beendet: dieser Klick legt keine neue Tabelle an
        let ended_cell = self.cell_edit.is_some();
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
            Tool::Data => self.data_press(p, m, ended_cell),
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
        // Notizheft: ein Klick in den Fließtext (oder auf eine leere Stelle einer Seite)
        // setzt dort den Cursor
        if !mods.shift && self.notebook_body_click(p, false, m) {
            return;
        }
        match self.hit(p, m) {
            Some(id) => {
                self.guide_sel = None;
                // Tabelle bzw. Diagramm: schon ausgewählt, dann wählt ein Klick (ohne Ziehen)
                // eine Zelle bzw. einen Datenpunkt; Umschalttaste erweitert die Zellen
                if let Some(a) = self.smart_anchor_of(id) {
                    let whole = self.single_smart() == Some(a);
                    let under = self.pick_at(a, p, m);
                    if let (
                        true,
                        true,
                        Some((pa, Pick::Cells(r0, c0, _, _))),
                        Some(Pick::Cells(r, c, _, _)),
                    ) = (mods.shift, whole, self.pick, under)
                    {
                        if pa == a {
                            self.pick = Some((a, Pick::Cells(r0, c0, r, c)));
                            self.cell_sel = Some((a, r, c));
                            self.drag = Drag::None;
                            return;
                        }
                    }
                    // gewählte Tabelle: Zellen aufziehen (verschoben wird sie am Griff)
                    if let (true, false, Some(Pick::Cells(r, c, r1, c1))) = (whole, mods.shift, under) {
                        self.pick = Some((a, Pick::Cells(r, c, r1, c1)));
                        self.cell_sel = Some((a, r, c));
                        self.guide_sel = None;
                        self.drag = Drag::CellSelect { anchor: a, r, c };
                        return;
                    }
                    if whole {
                        self.click_pick = Some((a, under));
                    } else {
                        self.pick = None;
                    }
                }
                // Tabelle: angeklickte Zelle merken (Formelleiste, Spaltenköpfe)
                if let Some(c) = self.table_cell_at(id, p) {
                    self.cell_sel = Some(c);
                }
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
        let start = p;
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
            self.drag = Drag::PenHandle { start };
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
        self.drag = Drag::PenHandle { start };
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
        // Präsentation: nur Text lässt sich markieren
        let only_text = self.doc.view == ViewMode::Presentation;
        let hit_any = |e: &Self| if only_text { e.hit_text(p, m) } else { e.hit(p, m) };
        if let Some(te) = self.text_edit {
            let hit = hit_any(self);
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
        // Text in Tabellen und Diagrammen entsteht aus deren Daten: nicht direkt bearbeiten
        let Some(id) = hit_any(self).filter(|id| self.read_only || !self.in_smart(*id)) else {
            return false;
        };
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

    /// Auswählen beim Ansehen: in Text die Zeichen markieren (Ziehen), andere Objekte nur
    /// auswählen (nicht verschieben), daneben einen Rahmen aufziehen.
    fn view_select_press(&mut self, p: Pt, mods: Mods, m: &dyn Metrics) {
        // nur Text: ein Klick setzt den Cursor bzw. markiert darin, Bilder und Formen lassen
        // sich nicht auswählen; Aufziehen wählt ganze Texte (zum Kopieren)
        if self.text_click(p, m) {
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

    /// Text der Auswahl zum Kopieren: markierte Zeichen, sonst der Text aller ausgewählten
    /// Textobjekte (in der Reihenfolge des Dokuments, durch Leerzeilen getrennt).
    pub fn copy_text(&self) -> Option<String> {
        if let Some(t) = self.edit_selected_text().filter(|t| !t.is_empty()) {
            return Some(t.replace('\u{2028}', "\n"));
        }
        let parts: Vec<String> = self
            .doc
            .layers
            .iter()
            .flat_map(|l| &l.objects)
            .filter(|o| self.selection.contains(&o.id))
            .filter_map(|o| o.shape.text_fields().map(|t| t.0.replace('\u{2028}', "\n")))
            .filter(|t| !t.trim().is_empty())
            .collect();
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    }

    /// Strich des Textmarkers als Objekt (beim Ansehen; ganz oben, durchscheinend, Farbe
    /// multipliziert). Fast waagerechte Striche werden gerade.
    fn finish_highlight(&mut self, pts: &[Pt]) {
        let z = self.view.zoom;
        let width = self.highlight_width;
        let total: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
        if pts.len() < 2 || total * z < 4.0 {
            return;
        }
        let (y0, y1) = pts
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.y), b.max(p.y)));
        let (x0, x1) = pts
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.x), b.max(p.x)));
        let straight = y1 - y0 < width * 0.7 && x1 - x0 > width;
        let pts: Vec<Pt> = if straight {
            let y = pts.iter().map(|p| p.y).sum::<f64>() / pts.len() as f64;
            vec![Pt::new(x0, y), Pt::new(x1, y)]
        } else {
            let sm = smooth_points(pts, 0.5, false);
            crate::geom::simplify(&sm, 1.5 / z)
        };
        if pts.len() < 2 {
            return;
        }
        let shape = Shape::Path {
            subpaths: vec![SubPath {
                nodes: smooth_nodes(&pts),
                closed: false,
            }],
            evenodd: false,
        };
        let style = Style {
            fill: None,
            stroke: Some(self.highlight_color),
            width,
            opacity: 0.45,
            blend: Blend::Multiply,
            cap: crate::model::LineCap::Round,
            join: crate::model::LineJoin::Round,
            gradient: None,
            stroke_gradient: None,
            shadow: None,
            ..Style::default()
        };
        // eigenes Dokument in der Präsentation: nur zu sehen, bis die Präsentation endet
        if self.temporary_marks() {
            self.marks.push(Object {
                group: 0,
                sub: Vec::new(),
                mask: false,
                locked: true,
                smart: None,
                name: String::new(),
                id: 0,
                shape,
                style,
                angle: 0.0,
            });
            self.marks_redo.clear();
            return;
        }
        let Some(li) = self.doc.layers.iter().rposition(|l| l.visible && !l.locked) else {
            return;
        };
        self.checkpoint(None);
        self.insert_object(li, shape, style);
    }

    fn text_press(&mut self, p: Pt, m: &dyn Metrics) {
        // Notizheft: ein Klick auf die Seite schreibt Fließtext (kein eigenes Textfeld)
        if self.notebook_body_click(p, true, m) {
            return;
        }
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
        // Zelle: Eingabe in die Tabelle übernehmen
        if let Some(ce) = self.cell_edit.take() {
            if ce.part == te.id {
                self.commit_cell(ce);
                self.last_key = None;
                return;
            }
        }
        // Fließtext: bleibt, solange er Text hat (die Seiten verwaltet `reflow_body`)
        if self.is_body_id(te.id) {
            let frames = self.body_frames();
            let all_empty = frames.iter().all(|id| {
                self.doc
                    .object(*id)
                    .and_then(|o| o.shape.text_fields())
                    .is_some_and(|t| t.0.trim().is_empty())
            });
            if all_empty && !self.read_only {
                self.checkpoint(None);
                for id in &frames {
                    self.doc.remove_object(*id);
                }
                self.drop_empty_body_layer();
                self.layers_version += 1;
            }
            // nach dem Schreiben ist nichts ausgewählt (das Feld selbst fasst man nicht an)
            self.selection.retain(|x| !frames.contains(x));
            self.last_key = None;
            return;
        }
        // Kopf-/Fußzeile: leer geworden, verschwindet sie auf allen Seiten
        if let Some(kind) = self.doc.object(te.id).and_then(|o| match &o.shape {
            Shape::TextArea { frame, .. } if frame.band != 0 => Some(frame.band),
            _ => None,
        }) {
            let empty = self
                .doc
                .object(te.id)
                .and_then(|o| o.shape.text_fields())
                .map(|t| t.0.trim().is_empty())
                .unwrap_or(true);
            let ids: Vec<u64> = self.band_frames(kind).into_iter().map(|f| f.1).collect();
            if empty && !self.read_only {
                self.checkpoint(None);
                for id in &ids {
                    self.doc.remove_object(*id);
                }
                self.layers_version += 1;
            }
            self.selection.retain(|x| !ids.contains(x));
            self.last_key = None;
            return;
        }
        let empty = self
            .doc
            .object(te.id)
            .and_then(|o| o.shape.text_fields())
            .map(|t| t.0.trim().is_empty())
            .unwrap_or(false);
        if empty && !self.read_only {
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
                // Fließtext: am Anfang einer Seite mit der vorigen verbinden
                return self.is_body_id(te.id) && self.body_join(te.id, true);
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
                return self.is_body_id(te.id) && self.body_join(te.id, false);
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
        // Fließtext: über den Seitenrand hinweg auf die Nachbarseite (ohne Markierung)
        if !extend && a == b && self.is_body_id(te.id) {
            let back = match mv {
                Move::Left | Move::WordLeft => c == 0,
                Move::Up => line_start(&t, c) == 0,
                _ => false,
            };
            let fwd = match mv {
                Move::Right | Move::WordRight => c >= t.len(),
                Move::Down => line_end(&t, c) >= t.len(),
                _ => false,
            };
            if back || fwd {
                if let Some(n) = self.body_neighbour(te.id, if back { -1 } else { 1 }) {
                    let len = self
                        .doc
                        .object(n)
                        .and_then(|o| o.shape.text_fields())
                        .map(|t| t.0.len())
                        .unwrap_or(0);
                    let at = if back { len } else { 0 };
                    self.selection = vec![n];
                    self.text_edit = Some(TextEdit {
                        id: n,
                        caret: at,
                        anchor: at,
                    });
                    return true;
                }
            }
        }
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
                    | Drag::Laser
                    | Drag::Erase { .. }
                    | Drag::TextSelect { .. }
                    | Drag::PenHandle { .. }
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
                self.hover = if !self.read_only
                    && !self.presenting
                    && matches!(self.tool, Tool::Select | Tool::Node)
                {
                    // Fließtext wird nicht hervorgehoben (ein Klick setzt nur den Cursor)
                    self.hit(p, m).filter(|id| !self.is_body_id(*id))
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
                    let fresh =
                        matches!(&*self.hover_ctx.borrow(), Some((r, _)) if *r == (self.rev, self.edits));
                    if !fresh && self.smart_snap {
                        let c = self.build_snap_ctx(m, false);
                        *self.hover_ctx.borrow_mut() = Some(((self.rev, self.edits), c));
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
                let q = if rotated {
                    frame.apply(p)
                } else {
                    self.snap_scale(handle, &bbox, p, mods, uniform, &orig)
                };
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
                let pages = self.shown_pages();
                let mut sel = base.clone();
                let pres = self.doc.view == ViewMode::Presentation;
                for l in self
                    .doc
                    .layers
                    .iter()
                    .filter(|l| if pres { l.visible } else { l.editable() })
                {
                    for o in &l.objects {
                        let inside = |b: Rect| {
                            b.intersects(&r)
                                && pages
                                    .as_ref()
                                    .map_or(true, |ps| ps.iter().any(|pg| b.intersects(pg)))
                        };
                        // Präsentation: nur Texte (auch der Fließtext)
                        let kind_ok = if self.doc.view == ViewMode::Presentation {
                            is_text_shape(&o.shape)
                        } else {
                            !is_body(&o.shape)
                        };
                        if !sel.contains(&o.id) && kind_ok && m.bbox(o).map(inside).unwrap_or(false) {
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
            Drag::Laser => {
                self.laser.push((p, Instant::now()));
                Drag::Laser
            }
            Drag::Column { gutter, edge, grab } => {
                self.drag_column(gutter, edge, grab, p);
                Drag::Column { gutter, edge, grab }
            }
            Drag::Margin { side: 4, page } => {
                self.drag_first_indent(page, p);
                Drag::Margin { side: 4, page }
            }
            Drag::Margin { side, page } => {
                let mut m = self.doc.margins;
                let v = match side {
                    0 => p.y - page.y0,
                    1 => page.x1 - p.x,
                    2 => page.y1 - p.y,
                    _ => p.x - page.x0,
                };
                // gegenüberliegender Rand: dazwischen bleiben mindestens 20 Einheiten Text
                let (size, other) = if side % 2 == 0 {
                    (page.height(), m[(side + 2) % 4])
                } else {
                    (page.width(), m[(side + 2) % 4])
                };
                let step = self.margin_step();
                let v = (v / step).round() * step;
                m[side] = v.clamp(0.0, (size - other - 20.0).max(0.0));
                self.set_margins(m, Some("margins"));
                Drag::Margin { side, page }
            }
            Drag::PenHandle { start } => {
                // Weg des Zeigers seit dem Drücken (Bildschirmpixel)
                let travel = (p - start).len() * self.view.zoom;
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
                        label: Some(tr("Guide")),
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
                    // Klicken ergibt eine Ecke). Gemessen wird der Weg des Zeigers seit dem
                    // Drücken: der Punkt selbst kann eingerastet sein und bei starker
                    // Vergrößerung ein ganzes Stück vom Zeiger entfernt liegen.
                    if travel > PEN_DEADZONE {
                        n.h_out = n.p + d;
                        n.h_in = n.p - d;
                    } else {
                        n.h_out = n.p;
                        n.h_in = n.p;
                    }
                }
                Drag::PenHandle { start }
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
            Drag::CellSelect { anchor, r, c } => {
                // Zellen bis zum Zeiger (außerhalb: bis zum Rand der Tabelle)
                if let (Some(o), true) = (self.doc.object(anchor), self.single_smart() == Some(anchor)) {
                    if let (Some(crate::smart::Smart::Table(t)), Some(f)) =
                        (o.smart.as_deref(), smart_frame(o))
                    {
                        let q = Pt::new(p.x.clamp(f.x0, f.x1 - 1e-6), p.y.clamp(f.y0, f.y1 - 1e-6));
                        if let Some((r1, c1)) = t.cell_at(f, q) {
                            // verbundene Zellen ganz
                            let g = t.expand((r.min(r1), c.min(c1), r.max(r1), c.max(c1)));
                            self.pick = Some((anchor, Pick::Cells(g.0, g.1, g.2, g.3)));
                        }
                    }
                }
                Drag::CellSelect { anchor, r, c }
            }
            Drag::RefPick { anchor, r, c } => {
                if let Some((r1, c1)) = self.cell_toward(anchor, p) {
                    if let Some(crate::smart::Smart::Table(t)) = self.smart_data(anchor) {
                        let r1 = r1.clamp(0, t.n_rows() as isize - 1) as usize;
                        let c1 = c1.clamp(0, t.n_cols() as isize - 1) as usize;
                        self.put_ref(r, c, r1, c1);
                    }
                }
                Drag::RefPick { anchor, r, c }
            }
            Drag::CellFill { anchor, range, .. } => {
                let (r0, c0, r1, c1) = range;
                let mut ext = (0, 0);
                if let Some((r, c)) = self.cell_toward(anchor, p) {
                    let dr = if r > r1 as isize {
                        r - r1 as isize
                    } else if r < r0 as isize {
                        r - r0 as isize
                    } else {
                        0
                    };
                    let dc = if c > c1 as isize {
                        c - c1 as isize
                    } else if c < c0 as isize {
                        c - c0 as isize
                    } else {
                        0
                    };
                    // die Richtung, in die weiter gezogen wurde
                    ext = if dr.abs() >= dc.abs() { (dr, 0) } else { (0, dc) };
                    // höchstens 500 neue Zeilen/Spalten auf einmal
                    ext = (ext.0.clamp(-(r0 as isize), 500), ext.1.clamp(-(c0 as isize), 500));
                }
                Drag::CellFill { anchor, range, ext }
            }
            Drag::CellMove {
                anchor,
                range,
                grab,
                mut off,
            } => {
                if let (Some((r, c)), Some(crate::smart::Smart::Table(t))) =
                    (self.cell_toward(anchor, p), self.smart_data(anchor))
                {
                    let (r0, c0, r1, c1) = range;
                    let (nr, nc) = (t.n_rows() as isize, t.n_cols() as isize);
                    off = (
                        (r - grab.0 as isize).clamp(-(r0 as isize), nr - 1 - r1 as isize),
                        (c - grab.1 as isize).clamp(-(c0 as isize), nc - 1 - c1 as isize),
                    );
                }
                Drag::CellMove {
                    anchor,
                    range,
                    grab,
                    off,
                }
            }
            Drag::HeadSelect { anchor, col, start } => {
                if let Some((a, xs, ys, _)) = self.head_geom().filter(|g| g.0 == anchor) {
                    let (nr, nc) = (ys.len() - 1, xs.len() - 1);
                    self.pick = Some((
                        a,
                        if col {
                            Pick::Cells(0, start, nr - 1, Self::head_index(&xs, s.x))
                        } else {
                            Pick::Cells(start, 0, Self::head_index(&ys, s.y), nc - 1)
                        },
                    ));
                }
                Drag::HeadSelect { anchor, col, start }
            }
            Drag::TableSize {
                anchor,
                col,
                idx,
                targets,
                sizes,
                frame,
                start,
            } => {
                let d = if col { p.x - start.x } else { p.y - start.y };
                let size = (sizes[idx] + d).max(4.0);
                let mut v = sizes.clone();
                if targets.contains(&idx) {
                    for &i in &targets {
                        v[i] = size;
                    }
                } else {
                    v[idx] = size;
                }
                let total: f64 = v.iter().sum();
                if let Some(crate::smart::Smart::Table(mut t)) = self.smart_data(anchor).cloned() {
                    if col {
                        t.cols = v;
                    } else {
                        t.rows = v;
                    }
                    let (w, h) = if col {
                        (total, frame.height())
                    } else {
                        (frame.width(), total)
                    };
                    if self.smart_data(anchor) != Some(&crate::smart::Smart::Table(t.clone())) {
                        self.touch();
                        if let Some(o) = self.doc.object_mut(anchor) {
                            o.smart = Some(Box::new(crate::smart::Smart::Table(t)));
                            o.shape = Shape::Rect {
                                x: frame.x0,
                                y: frame.y0,
                                w,
                                h,
                                r: 0.0,
                            };
                        }
                        self.rebuild_smart(anchor);
                    }
                }
                Drag::TableSize {
                    anchor,
                    col,
                    idx,
                    targets,
                    sizes,
                    frame,
                    start,
                }
            }
            Drag::DataCreate { id, start } => {
                let r = Rect::from_points(start, self.snap(p));
                if let Some(o) = self.doc.object_mut(id) {
                    o.shape = Shape::Rect {
                        x: r.x0,
                        y: r.y0,
                        w: r.width().max(1.0),
                        h: r.height().max(1.0),
                        r: 0.0,
                    };
                }
                self.rebuild_smart(id);
                Drag::DataCreate { id, start }
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
                let pages = self.shown_pages();
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
                            let visible = pages
                                .as_ref()
                                .map_or(true, |ps| ps.iter().any(|pg| pg.contains(n.p)));
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
        // nur geklickt (nicht gezogen): Teil einer schon gewählten Tabelle bzw. eines Diagramms
        if let Some((a, pk)) = self.click_pick.take() {
            if self.press_at.is_some() && self.single_smart() == Some(a) {
                self.pick = pk.map(|pk| (a, pk));
            }
        }
        self.press_at = None;
        self.hints.borrow_mut().clear();
        self.snap_ctx = None;
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        if matches!(drag, Drag::CellFill { .. } | Drag::CellMove { .. }) {
            self.finish_cells(&drag);
        }
        if let Drag::Move { .. } = drag {
            if let (Some(t), [img]) = (self.frame_ready(), &self.selection[..]) {
                let img = *img;
                self.place_in_frame(img, t, m);
            }
            self.frame_hover = None;
        }
        // Tabellen und Diagramme: nach dem Skalieren neu aufbauen (Schrift bleibt gleich groß,
        // Zellen und Diagramm füllen den neuen Rahmen)
        if let Drag::Scale { .. } | Drag::Rotate { .. } = drag {
            self.rebuild_selected_smart();
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
            | Drag::TableSize { .. }
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
            Drag::DataCreate { id, start } => {
                let small = self
                    .doc
                    .object(id)
                    .and_then(|o| o.shape.geom_bbox())
                    .map(|b| b.width().min(b.height()) * self.view.zoom < 12.0)
                    .unwrap_or(true);
                if small {
                    // nur geklickt: Tabelle in Standardgröße
                    if let Some(o) = self.doc.object_mut(id) {
                        o.shape = Shape::Rect {
                            x: start.x,
                            y: start.y,
                            w: 300.0,
                            h: 96.0,
                            r: 0.0,
                        };
                    }
                }
                self.rebuild_smart(id);
                // gleich in die erste Zelle tippen
                self.begin_cell_edit(id, 0, 0);
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
            Drag::Pencil { pts } if self.read_only => self.finish_highlight(&pts),
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
        if self.read_only || self.presenting {
            return;
        }
        let p = self.view.to_doc(s);
        // Notizheft: Doppelklick in den oberen bzw. unteren Rand – Kopf- bzw. Fußzeile
        if self.doc.view == ViewMode::Notebook && self.editing_band().is_none() {
            if let Some((kind, page)) = self.band_at(p) {
                if self.start_band(kind, page, Some(p), m) {
                    return;
                }
            }
        }
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
            Tool::Select | Tool::Data
                if self.hit(p, m).and_then(|id| self.table_cell_at(id, p)).is_some() =>
            {
                if let Some((a, r, c)) = self.hit(p, m).and_then(|id| self.table_cell_at(id, p)) {
                    // der zweite Druck hat das Tippen schon begonnen
                    if self
                        .cell_edit
                        .as_ref()
                        .is_some_and(|ce| (ce.anchor, ce.row, ce.col) == (a, r, c))
                    {
                        return;
                    }
                    // ein schon begonnenes Verschieben (erster Druck) gilt nicht
                    if !matches!(self.drag, Drag::None) {
                        self.pending = None;
                        self.drag = Drag::None;
                    }
                    self.click_pick = None;
                    self.begin_cell_edit(a, r, c);
                }
            }
            Tool::Select => {
                if let Some(id) = self.hit(p, m).filter(|id| !self.in_smart(*id)) {
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
        // im Notizheft gibt es nur das Textwerkzeug
        let tool = if self.doc.view == ViewMode::Notebook {
            Tool::Text
        } else {
            tool
        };
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
        if self.preview.is_some() {
            self.preview_end(false);
        } else if self.frame_mode.map(|f| f.1 != ImagePart::All).unwrap_or(false) {
            self.frame_mode = None;
        } else if self.text_edit.is_some() {
            self.end_text_edit();
        } else if self.active_pick().is_some() {
            // Zellen bzw. Teil gewählt: zurück zur ganzen Tabelle bzw. zum Diagramm
            self.pick = None;
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
        // Notizheft: die aktuelle Seite ganz; sonst alle Zeichenflächen
        let b = self
            .page_clip()
            .or_else(|| {
                let i = self.page_index()?;
                self.doc.artboard_rects().get(i).map(|(_, r)| *r)
            })
            .unwrap_or_else(|| self.doc.artboards_bbox());
        let z = ((w - 2.0 * margin) / b.width())
            .min((h - 2.0 * margin) / b.height())
            .clamp(0.02, 64.0);
        self.view.zoom = z;
        self.view.ox = ((w - b.width() * z) / 2.0 - b.x0 * z).round();
        self.view.oy = ((h - b.height() * z) / 2.0 - b.y0 * z).round();
        self.needs_fit = false;
    }

    // ───────────── Ansichten: Zeichenflächen, Notizheft, Präsentation ─────────────

    /// Ansicht wechseln. Notizheft und Präsentation stellen die Seiten untereinander (mit
    /// allem, was darauf liegt), die Zeichenflächen-Ansicht stellt sie wieder dorthin, wo
    /// sie vorher standen. Kein Schritt zum Rückgängigmachen: die Ansicht gilt für jeden
    /// Zustand des Dokuments. In der Präsentation lässt sich nichts ändern.
    pub fn set_view_mode(&mut self, mode: ViewMode) -> bool {
        if self.doc.view == mode && mode.pages() == self.notebook.is_some() {
            return false;
        }
        let page = self.page_near_view();
        if mode == ViewMode::Presentation {
            self.end_text_edit();
            self.deselect();
            self.cell_sel = None;
            self.pick = None;
        }
        self.drag = Drag::None;
        // Text in Arbeit: im Notizheft nur der Fließtext, sonst kein Fließtext
        if let Some(te) = self.text_edit {
            if (mode == ViewMode::Notebook) != self.is_body_id(te.id) {
                self.end_text_edit();
            }
        }
        if mode != ViewMode::Notebook {
            let body: Vec<u64> = self.body_frames();
            self.selection.retain(|x| !body.contains(x));
        } else {
            self.selection
                .retain(|x| self.doc.object(*x).is_some_and(|o| is_body(&o.shape)));
        }
        self.doc.view = mode;
        if mode == ViewMode::Notebook {
            self.set_tool(Tool::Text);
        }
        self.keep_body_selected();
        if self.apply_layout() {
            self.dirty = true;
            self.edits += 1;
        }
        self.read_only = mode == ViewMode::Presentation;
        self.presenting = self.presenting && mode == ViewMode::Presentation;
        self.marks.clear();
        self.marks_redo.clear();
        self.laser.clear();
        self.notebook = mode.pages().then_some(page);
        self.notebook_acc = 0.0;
        // dieselbe Seite wie vorher in die Bildmitte (die Seiten stehen jetzt woanders)
        if mode.pages() {
            self.go_page(page, false);
        }
        self.needs_fit = true;
        self.layers_version += 1;
        true
    }

    /// Ansicht des Dokuments.
    pub fn view_mode(&self) -> ViewMode {
        self.doc.view
    }

    /// Lage der weiteren Zeichenflächen, wenn die Seiten untereinander stehen: mittig unter
    /// der ersten, mit gleichem Abstand.
    fn stacked_positions(&self) -> Vec<(f64, f64)> {
        let w0 = self.doc.width;
        let mut y = self.doc.height + PAGE_GAP;
        self.doc
            .artboards
            .iter()
            .map(|a| {
                let at = ((w0 - a.w) / 2.0, y);
                y += a.h + PAGE_GAP;
                at
            })
            .collect()
    }

    /// Zeichenflächen an neue Stellen (für alle außer der ersten); was auf einer liegt (bzw.
    /// ihr am nächsten ist), wandert mit – Gruppen als Ganzes. Liefert, ob sich etwas bewegt.
    fn move_pages(&mut self, targets: &[(f64, f64)]) -> bool {
        let n = self.doc.artboards.len().min(targets.len());
        let deltas: Vec<Pt> = std::iter::once(Pt::new(0.0, 0.0))
            .chain((0..n).map(|i| {
                let a = &self.doc.artboards[i];
                Pt::new(targets[i].0 - a.x, targets[i].1 - a.y)
            }))
            .collect();
        if deltas.iter().all(|d| d.x.abs() < 1e-9 && d.y.abs() < 1e-9) {
            return false;
        }
        let rects: Vec<Rect> = self.doc.artboard_rects().into_iter().map(|(_, r)| r).collect();
        // Einheiten: Gruppen als Ganzes, sonst jedes Objekt für sich
        let unit = |o: &Object| {
            if o.group != 0 {
                (true, o.group)
            } else {
                (false, o.id)
            }
        };
        let mut boxes: std::collections::HashMap<(bool, u64), Rect> = std::collections::HashMap::new();
        for o in self.doc.layers.iter().flat_map(|l| &l.objects) {
            if let Some(b) = rough_box(&o.shape) {
                boxes.entry(unit(o)).and_modify(|r| *r = r.union(&b)).or_insert(b);
            }
        }
        let page_of = |b: &Rect| -> usize {
            let c = Pt::new((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
            if let Some(i) = rects.iter().position(|r| r.contains(c)) {
                return i;
            }
            let overlap = |r: &Rect| {
                let w = (r.x1.min(b.x1) - r.x0.max(b.x0)).max(0.0);
                let h = (r.y1.min(b.y1) - r.y0.max(b.y0)).max(0.0);
                w * h
            };
            let dist = |r: &Rect| {
                let dx = (r.x0 - c.x).max(c.x - r.x1).max(0.0);
                let dy = (r.y0 - c.y).max(c.y - r.y1).max(0.0);
                dx * dx + dy * dy
            };
            (0..rects.len())
                .max_by(|&i, &j| {
                    overlap(&rects[i])
                        .total_cmp(&overlap(&rects[j]))
                        .then(dist(&rects[j]).total_cmp(&dist(&rects[i])))
                })
                .unwrap_or(0)
        };
        let moves: std::collections::HashMap<(bool, u64), Pt> = boxes
            .iter()
            .map(|(k, b)| (*k, deltas.get(page_of(b)).copied().unwrap_or(Pt::new(0.0, 0.0))))
            .collect();
        for l in &mut self.doc.layers {
            for o in &mut l.objects {
                if let Some(d) = moves.get(&unit(o)) {
                    if d.x != 0.0 || d.y != 0.0 {
                        o.shape.translate(*d);
                    }
                }
            }
        }
        for (a, t) in self.doc.artboards.iter_mut().zip(targets) {
            a.x = t.0;
            a.y = t.1;
        }
        true
    }

    /// Seiten untereinander stellen; die bisherige Lage wird gemerkt.
    fn stack_pages(&mut self) -> bool {
        let targets = self.stacked_positions();
        for (a, t) in self.doc.artboards.iter_mut().zip(&targets) {
            if a.free.is_none() && ((a.x - t.0).abs() > 1e-9 || (a.y - t.1).abs() > 1e-9) {
                a.free = Some((a.x, a.y));
            }
        }
        self.move_pages(&targets)
    }

    /// Seiten wieder dorthin, wo sie in der Zeichenflächen-Ansicht standen.
    fn unstack_pages(&mut self) -> bool {
        let targets: Vec<(f64, f64)> = self
            .doc
            .artboards
            .iter()
            .map(|a| a.free.unwrap_or((a.x, a.y)))
            .collect();
        for a in &mut self.doc.artboards {
            a.free = None;
        }
        self.move_pages(&targets)
    }

    /// Lage der Seiten passend zur Ansicht des Dokuments.
    fn apply_layout(&mut self) -> bool {
        if self.doc.view.pages() {
            self.stack_pages()
        } else {
            self.unstack_pages()
        }
    }

    /// Nach Änderungen an den Seiten (neu, entfernt, umsortiert, andere Größe): im Notizheft
    /// wieder lückenlos untereinander.
    pub fn keep_stacked(&mut self) {
        if self.doc.view.pages() {
            self.stack_pages();
        }
    }

    /// Seite unter der Bildmitte (oder die nächstgelegene).
    fn page_near_view(&self) -> usize {
        let (w, h) = self.viewport;
        let c = self.view.to_doc(Pt::new(w / 2.0, h / 2.0));
        let rects = self.doc.artboard_rects();
        let dist = |r: &Rect| {
            let dx = (r.x0 - c.x).max(c.x - r.x1).max(0.0);
            let dy = (r.y0 - c.y).max(c.y - r.y1).max(0.0);
            dx * dx + dy * dy
        };
        (0..rects.len())
            .min_by(|&a, &b| dist(&rects[a].1).total_cmp(&dist(&rects[b].1)))
            .unwrap_or(0)
    }

    /// Aktuelle Seite (Notizheft, Präsentation): beim Blättern die gewählte, sonst die in
    /// der Bildmitte.
    pub fn page_index(&self) -> Option<usize> {
        let n = self.doc.artboard_rects().len();
        let i = self.notebook?;
        Some(if self.presenting { i } else { self.page_near_view() }.min(n.saturating_sub(1)))
    }

    /// Im Vollbild der Präsentation ist nur die aktuelle Seite zu sehen (sonst `None`).
    pub fn page_clip(&self) -> Option<Rect> {
        if !self.presenting {
            return None;
        }
        let i = self
            .notebook?
            .min(self.doc.artboard_rects().len().saturating_sub(1));
        self.doc.artboard_rects().get(i).map(|(_, r)| *r)
    }

    /// Sichtbare Seiten (Notizheft und Präsentation: was daneben liegt, ist ausgeblendet);
    /// `None`: alles sichtbar.
    pub fn shown_pages(&self) -> Option<Vec<Rect>> {
        if let Some(r) = self.page_clip() {
            return Some(vec![r]);
        }
        self.notebook?;
        Some(self.doc.artboard_rects().into_iter().map(|(_, r)| r).collect())
    }

    /// Liegt der Punkt auf einer sichtbaren Seite?
    fn on_shown_page(&self, p: Pt) -> bool {
        self.shown_pages()
            .map_or(true, |ps| ps.iter().any(|r| r.contains(p)))
    }

    /// Alle Seiten zusammen (Notizheft).
    fn pages_bbox(&self) -> Rect {
        self.doc.artboards_bbox()
    }

    /// Zu Seite `i` (Zoom bleibt). Passt sie ganz ins Bild, steht sie in der Mitte, sonst
    /// ihr Anfang oben (`at_end`: ihr Ende unten).
    pub fn go_page(&mut self, i: usize, at_end: bool) -> bool {
        let n = self.doc.artboard_rects().len();
        if self.notebook.is_none() || i >= n {
            return false;
        }
        self.notebook = Some(i);
        self.notebook_acc = 0.0;
        if self.presenting {
            return self.present_fit();
        }
        let r = self.doc.artboard_rects()[i].1;
        let (w, h) = self.viewport;
        let z = self.view.zoom;
        let m = PAGE_MARGIN;
        let all = self.pages_bbox();
        self.view.ox = if all.width() * z + 2.0 * m <= w {
            ((w - all.width() * z) / 2.0 - all.x0 * z).round()
        } else if r.width() * z + 2.0 * m <= w {
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

    /// Präsentation: die aktuelle Seite so groß wie möglich in die Mitte (ohne Rand).
    pub fn present_fit(&mut self) -> bool {
        let Some(r) = self.page_clip() else { return false };
        let (w, h) = self.viewport;
        if w <= 0.0 || h <= 0.0 || r.width() <= 0.0 || r.height() <= 0.0 {
            return false;
        }
        let z = (w / r.width()).min(h / r.height());
        self.view.zoom = z;
        self.view.ox = (w - r.width() * z) / 2.0 - r.x0 * z;
        self.view.oy = (h - r.height() * z) / 2.0 - r.y0 * z;
        true
    }

    /// Scrollen im Notizheft und in der Präsentation (Bildschirmpixel): die Seiten laufen
    /// durch, bleiben aber im Bild. Im Vollbild der Präsentation wird stattdessen umgeblättert,
    /// wenn man am Ende weiterscrollt. Liefert `true`, wenn sich die aktuelle Seite ändert.
    pub fn notebook_scroll(&mut self, dx: f64, dy: f64) -> bool {
        if self.notebook.is_none() {
            self.view.ox -= dx;
            self.view.oy -= dy;
            return false;
        }
        let (w, h) = self.viewport;
        let z = self.view.zoom;
        let m = PAGE_MARGIN;
        // Bereich, in dem die Seiten verschoben werden dürfen (passen sie hinein: keiner)
        let range = |size: f64, lo: f64, hi: f64, view: f64| -> (f64, f64) {
            if size * z + 2.0 * m <= view {
                let o = ((view - size * z) / 2.0 - lo * z).round();
                (o, o)
            } else {
                (view - m - hi * z, m - lo * z)
            }
        };
        let Some(r) = self.page_clip() else {
            let before = self.page_index();
            let all = self.pages_bbox();
            let (x_min, x_max) = range(all.width(), all.x0, all.x1, w);
            self.view.ox = (self.view.ox - dx).clamp(x_min, x_max);
            let (y_min, y_max) = range(all.height(), all.y0, all.y1, h);
            self.view.oy = (self.view.oy - dy).clamp(y_min, y_max);
            return self.page_index() != before;
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

    /// Vollbild der Präsentation: eine Seite zur Zeit (die in der Bildmitte).
    pub fn set_presenting(&mut self, on: bool) {
        if on == self.presenting {
            return;
        }
        if on {
            if self.doc.view != ViewMode::Presentation {
                self.set_view_mode(ViewMode::Presentation);
            }
            self.notebook = Some(self.page_near_view());
            self.presenting = true;
        } else {
            self.presenting = false;
            // weiter bei der zuletzt gezeigten Seite
            let i = self.notebook.unwrap_or(0);
            self.needs_fit = false;
            self.go_page(i, false);
        }
        self.laser.clear();
        self.notebook_acc = 0.0;
    }

    /// Laserpointer: noch leuchtende Punkte (ältere verblassen und fallen weg). `true`,
    /// solange etwas zu sehen ist.
    pub fn laser_alive(&mut self) -> bool {
        let now = Instant::now();
        self.laser
            .retain(|(_, t)| now.duration_since(*t) < Duration::from_millis(LASER_MS));
        !self.laser.is_empty()
    }

    /// Eigene Markierungen in der Präsentation (verschwinden beim Verlassen; in PDFs werden
    /// sie Teil der Datei).
    pub fn temporary_marks(&self) -> bool {
        self.doc.view == ViewMode::Presentation && !self.keep_marks
    }

    // ───────────── Fließtext (Notizheft) ─────────────

    /// Felder des Fließtexts in der Reihenfolge der Seiten.
    pub fn body_frames(&self) -> Vec<u64> {
        let rects = self.doc.artboard_rects();
        let mut v: Vec<(usize, f64, u64)> = self
            .doc
            .layers
            .iter()
            .flat_map(|l| &l.objects)
            .filter(|o| is_body(&o.shape))
            .map(|o| {
                let c = match &o.shape {
                    Shape::TextArea { xf, w, h, .. } => xf.apply(Pt::new(w.min(4.0), h.min(4.0))),
                    _ => Pt::new(0.0, 0.0),
                };
                let page = rects
                    .iter()
                    .position(|(_, r)| r.contains(c))
                    .unwrap_or(usize::MAX);
                (page, c.y, o.id)
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        v.into_iter().map(|x| x.2).collect()
    }

    /// Bereich des Fließtexts auf Seite `page` (die Seite ohne ihren Rand).
    fn body_box(&self, page: usize) -> Option<Rect> {
        let r = self.doc.artboard_rects().get(page)?.1;
        let [t, rt, b, l] = self.margins_for(r);
        Some(Rect {
            x0: r.x0 + l,
            y0: r.y0 + t,
            x1: r.x1 - rt,
            y1: r.y1 - b,
        })
    }

    /// Raster beim Ziehen der Ränder: ½ mm, 1 mm/10, 1/16 Zoll, 1 pt bzw. 1 px.
    fn margin_step(&self) -> f64 {
        match self.doc.unit.as_str() {
            "px" => 1.0,
            "pt" => 96.0 / 72.0,
            "in" => 96.0 / 16.0,
            "cm" => 96.0 / 25.4,
            _ => 96.0 / 25.4 / 2.0,
        }
    }

    /// Anfasser der Seitenränder am Lineal (Notizheft, aktuelle Seite): (Seite 0–3, Lage im
    /// Bildschirm, Seite).
    pub fn margin_handles(&self) -> Vec<(usize, Pt, Rect)> {
        if self.doc.view != ViewMode::Notebook || self.ruler <= 0.0 || self.presenting {
            return Vec::new();
        }
        let Some(i) = self.page_index() else {
            return Vec::new();
        };
        let Some(r) = self.doc.artboard_rects().get(i).map(|x| x.1) else {
            return Vec::new();
        };
        let [t, rt, b, l] = self.margins_for(r);
        let v = self.view;
        let band = self.ruler;
        let mut out = vec![
            (0, Pt::new(band / 2.0, v.to_screen(Pt::new(0.0, r.y0 + t)).y), r),
            (1, Pt::new(v.to_screen(Pt::new(r.x1 - rt, 0.0)).x, band / 2.0), r),
            (2, Pt::new(band / 2.0, v.to_screen(Pt::new(0.0, r.y1 - b)).y), r),
            (3, Pt::new(v.to_screen(Pt::new(r.x0 + l, 0.0)).x, band / 2.0), r),
        ];
        // Einzug der ersten Zeile (Dreieck oben links; nur mit Fließtext)
        if let Some((fmt, _, inset)) = self.body_fmt() {
            let x = r.x0 + l + inset + fmt.first.max(0.0);
            out.push((4, Pt::new(v.to_screen(Pt::new(x, 0.0)).x, band / 2.0), r));
        }
        out
    }

    /// Einzug der ersten Zeile am Lineal ziehen (alle Absätze des Fließtexts).
    fn drag_first_indent(&mut self, page: Rect, p: Pt) {
        let Some((fmt, _, inset)) = self.body_fmt() else {
            return;
        };
        let [_, rt, _, l] = self.margins_for(page);
        let room = (page.width() - l - rt - 2.0 * inset).max(0.0);
        let step = self.margin_step();
        let v = p.x - (page.x0 + l + inset);
        let v = ((v / step).round() * step).clamp(0.0, room / 2.0);
        if (v - fmt.first).abs() < 1e-9 {
            return;
        }
        self.set_body_first(v);
    }

    /// Einzug der ersten Zeile aller Absätze des Fließtexts (Einheiten des Dokuments).
    pub fn set_body_first(&mut self, v: f64) -> bool {
        let v = v.clamp(0.0, 10000.0);
        self.set_body_fmt("first-indent", |f| f.first = v)
    }

    /// Spaltenzahl des Fließtexts.
    pub fn set_body_columns(&mut self, n: u32) -> bool {
        let n = n.clamp(1, crate::model::MAX_COLUMNS);
        self.set_body_fmt("columns", |f| f.columns = n)
    }

    /// Absatzformat aller Felder des Fließtexts ändern (ein Schritt; gleiche Schlüssel
    /// hintereinander werden einer).
    fn set_body_fmt(&mut self, key: &str, f: impl Fn(&mut TextFormat)) -> bool {
        let frames = self.body_frames();
        let changed = frames
            .iter()
            .any(|id| match self.doc.object(*id).map(|o| &o.shape) {
                Some(Shape::TextArea { fmt, .. }) => {
                    let mut g = *fmt;
                    f(&mut g);
                    g != *fmt
                }
                _ => false,
            });
        if !changed {
            return false;
        }
        self.checkpoint(Some(key));
        for id in frames {
            if let Some(Shape::TextArea { fmt, .. }) = self.doc.object_mut(id).map(|o| &mut o.shape) {
                f(fmt);
            }
        }
        if let Some(b) = self.body_base.as_mut() {
            f(&mut b.fmt);
        }
        true
    }

    /// Anfasser eines Seitenrands unter `s` (im Lineal).
    pub fn margin_handle_at(&self, s: Pt) -> Option<(usize, Rect)> {
        self.margin_handles()
            .into_iter()
            .filter(|(side, h, _)| match side {
                // links oben: Einzug der ersten Zeile, links unten: Seitenrand
                4 => s.y < self.ruler / 2.0 && (s.x - h.x).abs() <= 6.0,
                3 => s.y >= self.ruler / 2.0 && s.y < self.ruler && (s.x - h.x).abs() <= 6.0,
                _ if side % 2 == 0 => s.x < self.ruler && (s.y - h.y).abs() <= 6.0,
                _ => s.y < self.ruler && (s.x - h.x).abs() <= 6.0,
            })
            .min_by(|a, b| a.1.dist(s).total_cmp(&b.1.dist(s)))
            .map(|(side, _, r)| (side, r))
    }

    /// Cursor über dem Lineal (Notizheft: an den Rändern zum Ziehen).
    pub fn ruler_cursor(&self, s: Pt) -> &'static str {
        match self.drag {
            Drag::Margin { side, .. } => {
                return if side == 0 || side == 2 {
                    "row-resize"
                } else {
                    "col-resize"
                }
            }
            Drag::Column { .. } => return "col-resize",
            _ => {}
        }
        if self.column_handle_at(s).is_some() {
            return "col-resize";
        }
        match self.margin_handle_at(s) {
            Some((side, _)) if side == 0 || side == 2 => "row-resize",
            Some(_) => "col-resize",
            None => "default",
        }
    }

    /// Format und Schriftgröße des Fließtexts (sein erstes Feld).
    pub fn body_fmt(&self) -> Option<(TextFormat, f64, f64)> {
        let id = *self.body_frames().first()?;
        match &self.doc.object(id)?.shape {
            Shape::TextArea { fmt, size, frame, .. } => Some((*fmt, *size, frame.inset.max(0.0))),
            _ => None,
        }
    }

    /// Spalten des Fließtexts auf der aktuellen Seite (Dokument): (Anfang, Ende) je Spalte.
    pub fn body_columns(&self) -> Vec<(f64, f64)> {
        if self.doc.view != ViewMode::Notebook {
            return Vec::new();
        }
        let (Some(i), Some((fmt, size, inset))) = (self.page_index(), self.body_fmt()) else {
            return Vec::new();
        };
        let Some(b) = self.body_box(i) else {
            return Vec::new();
        };
        if fmt.cols() < 2 {
            return Vec::new();
        }
        fmt.column_spans((b.width() - 2.0 * inset).max(1.0), size)
            .into_iter()
            .map(|(x, w)| (b.x0 + inset + x, b.x0 + inset + x + w))
            .collect()
    }

    /// Spaltenzwischenraum am oberen Lineal unter `s`: (Nummer, an seinem Rand?, Abstand des
    /// Zeigers zur Mitte in Dokumenteinheiten).
    pub fn column_handle_at(&self, s: Pt) -> Option<(usize, bool, f64)> {
        if self.ruler <= 0.0 || s.y >= self.ruler || s.x < self.ruler {
            return None;
        }
        let cols = self.body_columns();
        let v = self.view;
        let x = v.to_doc(s).x;
        for (i, w) in cols.windows(2).enumerate() {
            let (a, b) = (
                v.to_screen(Pt::new(w[0].1, 0.0)).x,
                v.to_screen(Pt::new(w[1].0, 0.0)).x,
            );
            let mid = (w[0].1 + w[1].0) / 2.0;
            if (s.x - a).abs() <= 4.0 || (s.x - b).abs() <= 4.0 {
                return Some((i, true, x - mid));
            }
            if s.x > a && s.x < b {
                return Some((i, false, x - mid));
            }
        }
        None
    }

    /// Spalten am Lineal ändern: Zwischenraum verschieben (die Spalten daneben werden
    /// breiter bzw. schmaler) oder an seinem Rand breiter/schmaler ziehen.
    fn drag_column(&mut self, gutter: usize, edge: bool, grab: f64, p: Pt) {
        let cols = self.body_columns();
        let Some((mut fmt, size, _)) = self.body_fmt() else {
            return;
        };
        if gutter + 1 >= cols.len() {
            return;
        }
        let mut widths: Vec<f64> = cols.iter().map(|(a, b)| b - a).collect();
        let mut gap = fmt.gutter(size);
        let mid = (cols[gutter].1 + cols[gutter + 1].0) / 2.0;
        let min = (size * 2.0).max(10.0);
        if edge {
            // so breit, wie der Zeiger von der Mitte entfernt ist (beide Seiten gleich)
            let g = ((p.x - mid).abs() * 2.0).max(2.0);
            let total: f64 = widths.iter().sum::<f64>() + gap * (cols.len() - 1) as f64;
            let room = total - g * (cols.len() - 1) as f64;
            if room < min * cols.len() as f64 {
                return;
            }
            let k = room / widths.iter().sum::<f64>();
            widths.iter_mut().for_each(|w| *w *= k);
            gap = g;
        } else {
            let d = (p.x - grab - mid)
                .max(min - widths[gutter])
                .min(widths[gutter + 1] - min);
            widths[gutter] += d;
            widths[gutter + 1] -= d;
        }
        let step = self.margin_step();
        let gap = (gap / step).round().max(1.0) * step;
        fmt.col_gap = gap;
        fmt.set_column_widths(&widths);
        let frames = self.body_frames();
        let same = frames.iter().all(|id| {
            matches!(self.doc.object(*id).map(|o| &o.shape), Some(Shape::TextArea { fmt: f, .. }) if *f == fmt)
        });
        if same {
            return;
        }
        self.checkpoint(Some("columns"));
        for id in frames {
            if let Some(Shape::TextArea { fmt: f, .. }) = self.doc.object_mut(id).map(|o| &mut o.shape) {
                *f = fmt;
            }
        }
    }

    /// Ränder auf einer Seite der Größe `r` (mindestens 20 Einheiten Platz für den Text).
    pub fn margins_for(&self, r: Rect) -> [f64; 4] {
        let [t, rt, b, l] = self.doc.margins.map(|m| m.max(0.0));
        let fit = |a: f64, c: f64, size: f64| {
            let room = (size - 20.0).max(0.0);
            if a + c <= room || a + c <= 0.0 {
                (a, c)
            } else {
                let k = room / (a + c);
                (a * k, c * k)
            }
        };
        let (l, rt) = fit(l, rt, r.width());
        let (t, b) = fit(t, b, r.height());
        [t, rt, b, l]
    }

    /// Seite `page` für den Fließtext (fehlt sie, kommt eine neue ans Ende, so groß wie die
    /// letzte). Liefert den Bereich des Textes darauf.
    fn ensure_body_page(&mut self, page: usize) -> Option<Rect> {
        while self.doc.artboard_rects().len() <= page {
            let last = self.doc.artboard_rects().last()?.1;
            let b = self.doc.artboards_bbox();
            self.doc.artboards.push(crate::model::Artboard {
                name: self.doc.next_artboard_name(),
                x: b.x1 + 40.0,
                y: 0.0,
                w: last.width(),
                h: last.height(),
                background: self.doc.background,
                free: None,
                auto: true,
            });
            if !self.doc.page_src.is_empty() {
                self.doc.page_src.push(usize::MAX);
            }
            self.keep_stacked();
            self.layers_version += 1;
        }
        self.body_box(page)
    }

    /// Neues (leeres) Feld des Fließtexts, ganz unten in der untersten Ebene – alles andere
    /// liegt davor und kann den Text verdrängen.
    fn new_body_frame(&mut self, base: &BodyBase, at: Rect) -> u64 {
        let id = self.doc.alloc_id();
        let o = Object {
            group: 0,
            sub: Vec::new(),
            mask: false,
            locked: false,
            smart: None,
            name: tr("Body Text").to_string(),
            id,
            shape: Shape::TextArea {
                xf: Affine::translate(at.x0, at.y0),
                w: at.width().max(1.0),
                h: at.height().max(1.0),
                text: String::new(),
                family: base.family.clone(),
                size: base.size,
                bold: base.bold,
                italic: base.italic,
                fmt: base.fmt,
                frame: TextFrame {
                    body: true,
                    ..TextFrame::default()
                },
                rich: RichText::default(),
            },
            style: Style {
                fill: base.fill,
                stroke: None,
                ..Style::default()
            },
            angle: 0.0,
        };
        let li = self.body_layer();
        self.doc.layers[li].objects.insert(0, o);
        self.layers_version += 1;
        id
    }

    /// Ebene des Fließtexts: die unterste, eigene und gesperrte Ebene (nur im Notizheft zu
    /// bearbeiten). Fehlt sie, wird sie angelegt. Liefert ihren Index.
    fn body_layer(&mut self) -> usize {
        if let Some(i) = self
            .doc
            .layers
            .iter()
            .position(|l| l.objects.iter().any(|o| is_body(&o.shape)))
        {
            return i;
        }
        let id = self.doc.alloc_id();
        let color = self.doc.layers.len();
        self.doc.layers.insert(
            0,
            Layer {
                id,
                name: tr("Body Text").into(),
                visible: true,
                locked: true,
                color,
                objects: Vec::new(),
            },
        );
        self.layers_version += 1;
        0
    }

    /// Ebene des Fließtexts entfernen, wenn kein Fließtext mehr darauf liegt (und sonst nichts).
    fn drop_empty_body_layer(&mut self) {
        let empty_body = self.doc.layers.len() > 1
            && self.doc.layers[0].locked
            && self.doc.layers[0].objects.is_empty()
            && self.doc.layers[0].name == tr("Body Text");
        if empty_body {
            let id = self.doc.layers[0].id;
            self.doc.layers.remove(0);
            if self.active_layer == id {
                self.active_layer = self.doc.layers.last().map(|l| l.id).unwrap_or(0);
            }
            self.layers_version += 1;
        }
    }

    /// Feld des Fließtexts setzen: Lage, Grundformat, Text und ob es einen Absatz fortsetzt.
    fn set_body_frame(&mut self, id: u64, at: Rect, base: &BodyBase, t: String, r: RichText, joined: bool) {
        let Some(o) = self.doc.object_mut(id) else { return };
        o.style.fill = base.fill;
        if let Shape::TextArea {
            xf,
            w,
            h,
            text,
            family,
            size,
            bold,
            italic,
            fmt,
            frame,
            rich,
        } = &mut o.shape
        {
            let nxf = Affine::translate(at.x0, at.y0);
            let (nw, nh) = (at.width().max(1.0), at.height().max(1.0));
            if *xf != nxf || (*w - nw).abs() > 1e-9 || (*h - nh).abs() > 1e-9 {
                *xf = nxf;
                *w = nw;
                *h = nh;
                frame.holes.clear();
            }
            *text = t;
            *rich = r;
            *family = base.family.clone();
            *size = base.size;
            *bold = base.bold;
            *italic = base.italic;
            *fmt = base.fmt;
            frame.joined = joined;
            frame.body = true;
            frame.path = None;
            // Abstände regeln die Seitenränder und das Wegdrängen der Objekte
            frame.inset = 0.0;
        }
    }

    fn body_hash(&self, frames: &[u64]) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hs = std::collections::hash_map::DefaultHasher::new();
        for id in frames {
            if let Some(o) = self.doc.object(*id) {
                body_frame_hash(o).hash(&mut hs);
            }
        }
        for (_, r) in self.doc.artboard_rects() {
            for v in [r.x0, r.y0, r.x1, r.y1] {
                v.to_bits().hash(&mut hs);
            }
        }
        for m in self.doc.margins {
            m.to_bits().hash(&mut hs);
        }
        hs.finish() | 1
    }

    /// Fließtext auf die Seiten verteilen: was auf einer Seite keinen Platz hat, läuft auf
    /// die nächste (bei Bedarf eine neue), Platz wird von der folgenden Seite aufgefüllt;
    /// nicht mehr gebrauchte neue Seiten fallen weg. Der Cursor bleibt an seiner Stelle im
    /// Text. Liefert, ob sich etwas geändert hat. Kein eigener Schritt im Verlauf: es folgt
    /// aus der Änderung davor.
    pub fn reflow_body(&mut self, m: &dyn Metrics) -> bool {
        let bands = self.sync_bands();
        let frames = self.body_frames();
        if frames.is_empty() {
            self.body_key = 0;
            self.body_hashes.clear();
            return bands;
        }
        let key = self.body_hash(&frames);
        if key == self.body_key {
            return false;
        }
        // gemeinsames Grundformat: das zuletzt geänderte Feld gibt es vor
        let bases: Vec<BodyBase> = frames
            .iter()
            .filter_map(|id| self.doc.object(*id).and_then(body_base_of))
            .collect();
        let Some(first) = bases.first().cloned() else {
            return false;
        };
        let base = match &self.body_base {
            Some(b) => bases
                .iter()
                .find(|x| *x != b)
                .cloned()
                .unwrap_or_else(|| b.clone()),
            None => first,
        };
        let base_changed = self.body_base.as_ref() != Some(&base);
        // ab dem ersten veränderten Feld (eins davor: dort kann Platz frei geworden sein)
        let hashes: Vec<u64> = frames
            .iter()
            .map(|id| self.doc.object(*id).map(|o| body_frame_hash(o)).unwrap_or(0))
            .collect();
        let misplaced = frames.iter().enumerate().position(|(k, id)| {
            let at = self.body_box(k);
            match (self.doc.object(*id).map(|o| &o.shape), at) {
                (Some(Shape::TextArea { xf, w, h, .. }), Some(at)) => {
                    // (gespeicherte Lagen sind gerundet: tausendstel Pixel zählen nicht)
                    (xf.e - at.x0).abs() > 1e-3
                        || (xf.f - at.y0).abs() > 1e-3
                        || (w - at.width()).abs() > 1e-3
                        || (h - at.height()).abs() > 1e-3
                }
                _ => true,
            }
        });
        let changed = (0..frames.len())
            .find(|&k| self.body_hashes.get(k) != Some(&hashes[k]))
            .unwrap_or(frames.len());
        // ab diesem Feld ist alles wie beim letzten Verteilen (dort darf man aufhören)
        let settled = if base_changed || misplaced.is_some() || self.body_hashes.len() != frames.len() {
            usize::MAX
        } else {
            (0..frames.len())
                .rev()
                .find(|&k| self.body_hashes.get(k) != Some(&hashes[k]))
                .map_or(0, |k| k + 1)
        };
        let start = if base_changed {
            0
        } else {
            changed
                .min(misplaced.unwrap_or(usize::MAX))
                .min(frames.len() - 1)
                .saturating_sub(1)
        };
        let dl = base.fmt.list;
        // Text ab `start` als Ganzes (mit Cursor und Markierung in Textkoordinaten)
        let te = self.text_edit;
        let mut story = String::new();
        let mut story_rich = RichText::default();
        let mut caret = None;
        let mut anchor = None;
        let mut first_joined = false;
        for (k, id) in frames[start..].iter().enumerate() {
            let Some(Shape::TextArea {
                text, rich, frame, ..
            }) = self.doc.object(*id).map(|o| &o.shape)
            else {
                continue;
            };
            let newline = k > 0 && !frame.joined;
            if k == 0 {
                first_joined = frame.joined && start > 0;
            }
            let off = if k == 0 {
                0
            } else {
                story.len() + usize::from(newline)
            };
            if let Some(te) = te.filter(|te| te.id == *id) {
                caret = Some(off + te.caret.min(text.len()));
                anchor = Some(off + te.anchor.min(text.len()));
            }
            if k == 0 {
                story = text.clone();
                story_rich = rich.clone();
            } else {
                let (t, r) = RichText::join(&story, &story_rich, newline, text, rich, dl);
                story = t;
                story_rich = r;
            }
        }
        // bisheriger Stand ab `start` (um aufzuhören, sobald sich nichts mehr ändert)
        let old: Vec<(String, RichText, bool)> = frames[start..]
            .iter()
            .filter_map(|id| match self.doc.object(*id).map(|o| &o.shape) {
                Some(Shape::TextArea {
                    text, rich, frame, ..
                }) => Some((text.clone(), rich.clone(), frame.joined)),
                _ => None,
            })
            .collect();
        let mut checks = 0;
        // verteilen
        let mut ids = frames.clone();
        let mut placed: Vec<(usize, usize, usize)> = Vec::new();
        let mut rest = story;
        let mut rest_rich = story_rich;
        let mut joined = first_joined;
        let mut offset = 0usize;
        let mut k = start;
        let mut est = 4000usize;
        let mut empty_run = 0;
        loop {
            let Some(at) = self.ensure_body_page(k) else { break };
            let id = match ids.get(k) {
                Some(id) => *id,
                None => {
                    let id = self.new_body_frame(&base, at);
                    ids.push(id);
                    id
                }
            };
            // so viel wie nötig ausprobieren (lange Texte nicht ganz auf jede Seite legen)
            let (keep, tail, tail_joined, all) = loop {
                let cap = if rest.len() > est {
                    char_floor(&rest, est)
                } else {
                    rest.len()
                };
                let (t, r) = if cap < rest.len() {
                    (
                        rest[..cap].to_string(),
                        RichText::split(&rest, &rest_rich, cap, false, dl).0,
                    )
                } else {
                    (rest.clone(), rest_rich.clone())
                };
                self.set_body_frame(id, at, &base, t, r, joined);
                let fit = self.doc.object(id).and_then(|o| m.area_fit(o));
                match fit {
                    None if cap < rest.len() => {
                        est = est.saturating_mul(2);
                        continue;
                    }
                    None => break (rest.len(), rest.len(), false, true),
                    Some(p) => {
                        let p = char_floor(&rest, p.min(cap));
                        let b = rest.as_bytes();
                        if p < rest.len() && b[p] == b'\n' {
                            break (p, p + 1, false, false);
                        }
                        if p > 0 && b[p - 1] == b'\n' {
                            break (p - 1, p, false, false);
                        }
                        break (p, p, true, false);
                    }
                }
            };
            // Seitenumbruch: danach geht es auf der nächsten Seite weiter (der Umbruch bleibt
            // am Ende dieser Seite; ein Absatzumbruch direkt dahinter fällt an die Grenze)
            let pb = rest[..if all { rest.len() } else { keep }.min(rest.len())]
                .find(crate::model::PAGE_BREAK)
                .or_else(|| {
                    // genau an der Stelle, an der die Seite voll ist
                    (!all && rest[keep..].starts_with(crate::model::PAGE_BREAK)).then_some(keep)
                });
            let (keep, tail, tail_joined, all) = match pb {
                Some(q) => {
                    let after = q + crate::model::PAGE_BREAK.len_utf8();
                    if rest[after..].starts_with('\n') {
                        (after, after + 1, false, false)
                    } else {
                        (after, after, true, false)
                    }
                }
                None => (keep, tail, tail_joined, all),
            };
            // Seiten, auf die gar nichts passt: nicht endlos neue anlegen
            let (keep, tail, tail_joined, all) = if keep == 0 && tail == 0 && !all {
                empty_run += 1;
                if empty_run >= 3 {
                    (rest.len(), rest.len(), false, true)
                } else {
                    (keep, tail, tail_joined, all)
                }
            } else {
                empty_run = 0;
                (keep, tail, tail_joined, all)
            };
            est = est.max(keep * 2 + 1000);
            let (head_r, tail_r) = if all {
                (rest_rich.clone(), RichText::default())
            } else {
                RichText::split(&rest, &rest_rich, keep, tail > keep, dl)
            };
            let head = rest[..keep].to_string();
            let tail_text = rest[tail..].to_string();
            let j = k - start;
            // aufhören nur, wenn die folgenden Seiten seit dem letzten Verteilen unverändert sind
            let same = !all
                && k + 1 >= settled
                && old.len() == frames.len() - start
                && j + 1 < old.len()
                && head == old[j].0
                && head_r == old[j].1
                && joined == old[j].2
                && tail_joined == old[j + 1].2;
            self.set_body_frame(id, at, &base, head, head_r, joined);
            placed.push((k, offset, keep));
            if all || k > 5000 {
                break;
            }
            // die übrigen Seiten wären wie vorher: so lassen
            if same && checks < 3 {
                checks += 1;
                let (mut ot, mut or) = (old[j + 1].0.clone(), old[j + 1].1.clone());
                for (t, r, jn) in &old[j + 2..] {
                    let (a, b) = RichText::join(&ot, &or, !jn, t, r, dl);
                    ot = a;
                    or = b;
                }
                if ot == tail_text && or == tail_r {
                    k = frames.len() - 1;
                    break;
                }
            }
            offset += tail;
            rest = tail_text;
            rest_rich = tail_r;
            joined = tail_joined;
            k += 1;
        }
        // übrige Felder und nicht mehr gebrauchte neue Seiten entfernen
        let last = k;
        for id in ids.iter().skip(last + 1) {
            self.doc.remove_object(*id);
            self.selection.retain(|x| x != id);
            self.layers_version += 1;
        }
        ids.truncate(last + 1);
        loop {
            let rects = self.doc.artboard_rects();
            let n = rects.len();
            if n <= last + 1 || n < 2 {
                break;
            }
            let r = rects[n - 1].1;
            let auto = self.doc.artboards.get(n - 2).is_some_and(|a| a.auto);
            let busy = self.doc.layers.iter().flat_map(|l| &l.objects).any(|o| {
                !is_body(&o.shape)
                    && rough_box(&o.shape)
                        .is_some_and(|b| r.contains(Pt::new((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0)))
            });
            if !auto || busy {
                break;
            }
            self.remove_artboard(n - 1);
        }
        // Cursor und Markierung an dieselbe Stelle im Text
        if let (Some(c), Some(te)) = (caret, te) {
            if let Some(&(kk, off, len)) = placed.iter().find(|(_, off, len)| c >= *off && c <= off + len) {
                if let Some(&id) = ids.get(kk) {
                    let a = anchor
                        .filter(|a| *a >= off && *a <= off + len)
                        .map(|a| a - off)
                        .unwrap_or(c - off);
                    if id != te.id && self.selection.contains(&te.id) {
                        self.selection = vec![id];
                    }
                    self.text_edit = Some(TextEdit {
                        id,
                        caret: c - off,
                        anchor: a,
                    });
                }
            }
        }
        let frames = self.body_frames();
        self.body_hashes = frames
            .iter()
            .map(|id| self.doc.object(*id).map(|o| body_frame_hash(o)).unwrap_or(0))
            .collect();
        self.body_base = Some(base);
        self.body_key = self.body_hash(&frames);
        true
    }

    /// Im Fließtext schreiben: an der Stelle `p` (sonst am Ende); gibt es noch keinen, beginnt
    /// er auf der ersten Seite.
    pub fn begin_body_edit(&mut self, p: Option<Pt>, m: &dyn Metrics) -> bool {
        if self.read_only {
            return false;
        }
        let mut frames = self.body_frames();
        if frames.is_empty() {
            let Some(at) = self.body_box(0) else {
                return false;
            };
            self.checkpoint(None);
            let base = BodyBase {
                family: self.text_defaults.family.clone(),
                size: 16.0,
                bold: false,
                italic: false,
                fmt: TextFormat {
                    line: 1.35,
                    para: 0.5,
                    line_auto: true,
                    ..TextFormat::default()
                },
                fill: Some(Color::rgb(0.13, 0.13, 0.13)),
            };
            let id = self.new_body_frame(&base, at);
            self.body_base = Some(base);
            frames.push(id);
        }
        let rects = self.doc.artboard_rects();
        let hit = p.and_then(|p| {
            frames.iter().copied().find(|id| {
                self.doc
                    .object(*id)
                    .and_then(|o| m.bbox(o))
                    .zip(rects.iter().find(|(_, r)| r.contains(p)))
                    .is_some_and(|(b, (_, r))| r.contains(Pt::new((b.x0 + b.x1) / 2.0, b.y0 + 1.0)))
            })
        });
        let (id, idx) = match (hit, p) {
            (Some(id), Some(p)) => (id, self.doc.object(id).and_then(|o| m.text_index(o, p))),
            _ => (*frames.last().unwrap_or(&0), None),
        };
        let len = self
            .doc
            .object(id)
            .and_then(|o| o.shape.text_fields())
            .map(|t| t.0.len())
            .unwrap_or(0);
        self.selection = vec![id];
        self.begin_text_edit(id, Some(idx.unwrap_or(len)));
        if p.is_some() {
            self.drag = Drag::TextSelect { id };
        }
        true
    }

    /// Seitenumbruch an der Schreibmarke (nur im Fließtext): der Text danach beginnt auf
    /// einer neuen Seite.
    pub fn insert_page_break(&mut self) -> bool {
        match self.text_edit {
            Some(te) if self.is_body_id(te.id) => {
                let s = format!("{}\n", crate::model::PAGE_BREAK);
                self.edit_insert(&s)
            }
            _ => false,
        }
    }

    /// Notizheft: Klick in den Fließtext bzw. auf eine leere Stelle einer Seite – der Cursor
    /// kommt dorthin (`create`: gibt es noch keinen Fließtext, beginnt er). Liefert, ob der
    /// Klick damit erledigt ist.
    fn notebook_body_click(&mut self, p: Pt, create: bool, m: &dyn Metrics) -> bool {
        if self.doc.view != ViewMode::Notebook || self.read_only {
            return false;
        }
        // Kopf-/Fußzeile in Arbeit: ein Klick hinein setzt dort den Cursor (auf jeder Seite)
        if let Some(kind) = self.editing_band() {
            let inside = self.band_frames(kind).into_iter().map(|f| f.1).find(|id| {
                self.doc
                    .object(*id)
                    .and_then(|o| o.shape.geom_bbox())
                    .is_some_and(|b| b.contains(p))
            });
            if let Some(id) = inside {
                let i = self.doc.object(id).and_then(|o| m.text_index(o, p));
                self.selection = vec![id];
                self.begin_text_edit(id, i);
                self.drag = Drag::TextSelect { id };
                return true;
            }
        }
        // im Notizheft lässt sich nur der Fließtext anklicken: überall auf einer Seite
        let on_page = (create || !self.body_frames().is_empty())
            && self.doc.artboard_rects().iter().any(|(_, r)| r.contains(p));
        if !on_page {
            return false;
        }
        if let Some(te) = self.text_edit {
            if !self.is_body_id(te.id) {
                self.end_text_edit();
            }
        }
        self.begin_body_edit(Some(p), m)
    }

    /// Ränder des Fließtexts (oben, rechts, unten, links; alle Seiten). `key`: aufeinander
    /// folgende Änderungen (Ziehen am Lineal) sind ein Schritt.
    pub fn set_margins(&mut self, m: [f64; 4], key: Option<&str>) -> bool {
        let m = m.map(|x| if x.is_finite() { x.max(0.0) } else { 0.0 });
        if m.iter().zip(self.doc.margins).all(|(a, b)| (a - b).abs() < 1e-9) {
            return false;
        }
        self.checkpoint(key);
        self.doc.margins = m;
        true
    }

    /// Notizheft, ohne Schriftmetriken: liegt der Punkt im Fließtext einer Seite (und über
    /// keinem anderen Objekt, soweit es sich ohne Metriken sagen lässt)?
    fn hit_quick(&self, p: Pt) -> bool {
        let frames = self.body_frames();
        if frames.is_empty() {
            return false;
        }
        let in_body = frames.iter().any(|id| {
            self.doc
                .object(*id)
                .and_then(|o| o.shape.geom_bbox())
                .is_some_and(|b| b.contains(p))
        });
        let other = self
            .doc
            .layers
            .iter()
            .filter(|l| l.editable())
            .flat_map(|l| &l.objects)
            .any(|o| !is_body(&o.shape) && o.shape.geom_bbox().is_some_and(|b| b.contains(p)));
        in_body && !other
    }

    // ───────────── Geöffnete Texte (ODT): Bilder und Tabellen setzen ─────────────

    /// Bilder und Tabellen eines geöffneten Textes an ihre Stellen setzen: der Text wird auf
    /// die Seiten verteilt, jedes Objekt kommt an die Zeile seines Absatzes (bzw. auf seine
    /// Seite) und drängt den Text weg; danach geht es mit dem nächsten weiter. Kein Schritt
    /// im Verlauf (gehört zum Öffnen).
    pub fn place_floats(&mut self, floats: Vec<crate::odt::Floating>, m: &dyn Metrics) {
        let settle = |e: &mut Editor| {
            for _ in 0..40 {
                e.update_flow();
                if !e.reflow_body(m) {
                    break;
                }
            }
        };
        settle(self);
        let li = match self.doc.layers.iter().rposition(|l| {
            l.visible && !l.locked && !l.objects.iter().any(|o| is_body(&o.shape) || is_band(&o.shape))
        }) {
            Some(i) => i,
            None => {
                let id = self.doc.alloc_id();
                self.doc.layers.push(Layer {
                    id,
                    name: tr("Layer 1").into(),
                    visible: true,
                    locked: false,
                    color: 0,
                    objects: Vec::new(),
                });
                self.layers_version += 1;
                self.doc.layers.len() - 1
            }
        };
        for f in floats {
            // Feld des Fließtexts mit der Stelle (zwischen Feldern fällt nur bei einem neuen
            // Absatz ein Umbruch weg)
            let frames = self.body_frames();
            let mut pos = 0usize;
            let mut found = None;
            for (k, id) in frames.iter().enumerate() {
                let Some(Shape::TextArea {
                    text,
                    frame,
                    xf,
                    w,
                    h,
                    ..
                }) = self.doc.object(*id).map(|o| &o.shape)
                else {
                    continue;
                };
                if k > 0 && !frame.joined {
                    pos += 1;
                }
                if f.offset >= pos && f.offset <= pos + text.len() {
                    found = Some((
                        *id,
                        f.offset - pos,
                        text.len(),
                        Rect::from_xywh(xf.e, xf.f, *w, *h),
                    ));
                }
                pos += text.len();
                if found.is_some() && f.offset < pos {
                    break;
                }
            }
            let rects = self.doc.artboard_rects();
            let (x, y, area) = match (f.page, found) {
                (Some(pg), _) => {
                    let r = rects
                        .get(pg)
                        .or(rects.last())
                        .map(|x| x.1)
                        .unwrap_or(Rect::from_xywh(0.0, 0.0, 1.0, 1.0));
                    (r.x0 + f.x.unwrap_or(0.0), r.y0 + f.y, r)
                }
                (None, Some((id, local, len, fr))) => {
                    let c = self
                        .doc
                        .object(id)
                        .and_then(|o| m.caret_top(o, local))
                        .unwrap_or_else(|| {
                            let k = if len == 0 { 0.0 } else { local as f64 / len as f64 };
                            Pt::new(fr.x0, fr.y0 + fr.height() * k)
                        });
                    let x = match (f.halign, f.x) {
                        (Some(Align::Center), _) => fr.x0 + (fr.width() - f.w).max(0.0) / 2.0,
                        (Some(Align::Right), _) => fr.x1 - f.w,
                        (_, Some(dx)) => fr.x0 + dx,
                        (_, None) => c.x,
                    };
                    let y = if f.x.is_none() { c.y } else { c.y + f.y };
                    (x, y, fr)
                }
                (None, None) => continue,
            };
            match f.kind {
                crate::odt::FloatKind::Image(data) => {
                    self.insert_object(
                        li,
                        Shape::Image {
                            xf: Affine::translate(x, y),
                            w: f.w,
                            h: f.h,
                            data,
                            clip: None,
                        },
                        Style {
                            fill: None,
                            stroke: None,
                            ..Style::default()
                        },
                    );
                }
                crate::odt::FloatKind::Table(mut t) => {
                    // Schrift wie der Fließtext, so breit wie der Textbereich
                    if let Some(Shape::TextArea { family, size, .. }) = frames
                        .first()
                        .and_then(|id| self.doc.object(*id))
                        .map(|o| &o.shape)
                    {
                        t.family = family.clone();
                        t.size = *size;
                    }
                    let rows = t.n_rows() as f64;
                    let h = rows * t.size * 2.0;
                    self.insert_smart_at(
                        li,
                        crate::smart::Smart::Table(*t),
                        Rect::from_xywh(area.x0, y, area.width(), h),
                    );
                }
            }
            settle(self);
        }
        self.layers_version += 1;
    }

    // ───────────── Kopf- und Fußzeile (Notizheft) ─────────────

    /// Bereich der Kopf- bzw. Fußzeile auf einer Seite: im oberen bzw. unteren Rand, so breit
    /// wie der Fließtext.
    pub fn band_box(&self, kind: u8, r: Rect) -> Rect {
        let [t, rt, b, l] = self.margins_for(r);
        let x0 = r.x0 + l;
        let x1 = (r.x1 - rt).max(x0 + 1.0);
        if kind == crate::model::BAND_HEADER {
            Rect {
                x0,
                y0: r.y0 + t * 0.2,
                x1,
                y1: r.y0 + (t * 0.8).max(t * 0.2 + 1.0),
            }
        } else {
            Rect {
                x0,
                y0: r.y1 - (b * 0.8).max(b * 0.2 + 1.0),
                x1,
                y1: r.y1 - b * 0.2,
            }
        }
    }

    /// Felder der Kopf- bzw. Fußzeile: (Seite, Kennung), nach Seiten geordnet; Felder neben
    /// den Seiten haben die Seite `usize::MAX`.
    fn band_frames(&self, kind: u8) -> Vec<(usize, u64)> {
        let rects = self.doc.artboard_rects();
        let mut v: Vec<(usize, u64)> = self
            .doc
            .layers
            .iter()
            .flat_map(|l| &l.objects)
            .filter_map(|o| match &o.shape {
                Shape::TextArea { xf, w, h, frame, .. } if frame.band == kind => {
                    let c = xf.apply(Pt::new(w / 2.0, h / 2.0));
                    let page = rects
                        .iter()
                        .position(|(_, r)| r.contains(c))
                        .unwrap_or(usize::MAX);
                    Some((page, o.id))
                }
                _ => None,
            })
            .collect();
        v.sort();
        v
    }

    /// Gibt es eine Kopf- bzw. Fußzeile?
    pub fn has_band(&self, kind: u8) -> bool {
        !self.band_frames(kind).is_empty()
    }

    /// Kopf- bzw. Fußzeile in Arbeit: ihre Bereiche auf allen Seiten (`true`: Kopfzeile).
    pub fn band_boxes_editing(&self) -> Vec<(bool, Rect)> {
        let Some(kind) = self.editing_band() else {
            return Vec::new();
        };
        let rects = self.doc.artboard_rects();
        let skip = self.doc.bands_skip_first && rects.len() > 1;
        rects
            .iter()
            .enumerate()
            .filter(|(i, _)| !(skip && *i == 0))
            .map(|(_, (_, r))| (kind == crate::model::BAND_HEADER, self.band_box(kind, *r)))
            .collect()
    }

    /// Kopf-/Fußzeile, die gerade bearbeitet wird.
    pub fn editing_band(&self) -> Option<u8> {
        let te = self.text_edit?;
        match &self.doc.object(te.id)?.shape {
            Shape::TextArea { frame, .. } if frame.band != 0 => Some(frame.band),
            _ => None,
        }
    }

    /// Kopf- bzw. Fußzeile am Punkt `p` (im oberen bzw. unteren Rand einer Seite).
    fn band_at(&self, p: Pt) -> Option<(u8, usize)> {
        let rects = self.doc.artboard_rects();
        let i = rects.iter().position(|(_, r)| r.contains(p))?;
        let r = rects[i].1;
        let [t, _, b, _] = self.margins_for(r);
        if p.y < r.y0 + t {
            Some((crate::model::BAND_HEADER, i))
        } else if p.y > r.y1 - b {
            Some((crate::model::BAND_FOOTER, i))
        } else {
            None
        }
    }

    /// Kopf- und Fußzeilen abgleichen: das zuletzt geänderte (bzw. bearbeitete) Feld gibt den
    /// Inhalt für alle Seiten vor; fehlende Felder kommen dazu, überzählige fallen weg, jedes
    /// steht im Rand seiner Seite und kennt Seitenzahl, Seitenanzahl und Datum. Eine leere
    /// Kopf-/Fußzeile verschwindet, sobald sie nicht mehr bearbeitet wird. Kein Schritt im
    /// Verlauf. Liefert, ob sich etwas geändert hat.
    fn sync_bands(&mut self) -> bool {
        use std::hash::{Hash, Hasher};
        let content = |o: &Object| -> u64 {
            let mut hs = std::collections::hash_map::DefaultHasher::new();
            if let Shape::TextArea {
                text,
                family,
                size,
                bold,
                italic,
                fmt,
                rich,
                ..
            } = &o.shape
            {
                text.hash(&mut hs);
                family.hash(&mut hs);
                size.to_bits().hash(&mut hs);
                (bold, italic).hash(&mut hs);
                format!("{fmt:?}{rich:?}{:?}", o.style.fill).hash(&mut hs);
            }
            hs.finish() | 1
        };
        let rects = self.doc.artboard_rects();
        let n = rects.len();
        let editing = self.text_edit.map(|t| t.id);
        let mut changed = false;
        for (k, kind) in [crate::model::BAND_HEADER, crate::model::BAND_FOOTER]
            .into_iter()
            .enumerate()
        {
            let frames = self.band_frames(kind);
            if frames.is_empty() {
                self.band_hash[k] = 0;
                continue;
            }
            let ids: Vec<u64> = frames.iter().map(|f| f.1).collect();
            let edited = editing.filter(|id| ids.contains(id));
            let src = edited
                .or_else(|| {
                    ids.iter()
                        .copied()
                        .find(|id| self.doc.object(*id).map(content) != Some(self.band_hash[k]))
                })
                .unwrap_or(ids[0]);
            let Some(src_obj) = self.doc.object(src).cloned() else {
                continue;
            };
            let empty = src_obj
                .shape
                .text_fields()
                .map(|t| t.0.trim().is_empty())
                .unwrap_or(true);
            if empty && edited.is_none() {
                for id in &ids {
                    self.doc.remove_object(*id);
                }
                self.band_hash[k] = 0;
                self.layers_version += 1;
                changed = true;
                continue;
            }
            // je Seite ein Feld: vorhandene behalten, doppelte und solche neben den Seiten weg
            let mut by_page: Vec<Option<u64>> = vec![None; n];
            for (page, id) in &frames {
                match by_page.get_mut(*page) {
                    Some(slot) if slot.is_none() || Some(*id) == edited => {
                        if let Some(old) = slot.replace(*id) {
                            self.doc.remove_object(old);
                            changed = true;
                        }
                    }
                    _ => {
                        if Some(*id) != edited {
                            self.doc.remove_object(*id);
                            changed = true;
                        }
                    }
                }
            }
            let li = match self.doc.locate(src) {
                Some((li, _)) => li,
                None => self.body_layer(),
            };
            for (i, (_, r)) in rects.iter().enumerate() {
                let bx = self.band_box(kind, *r);
                let id = match by_page[i] {
                    Some(id) => id,
                    None => {
                        let mut o = src_obj.clone();
                        o.id = self.doc.alloc_id();
                        let id = o.id;
                        self.doc.layers[li].objects.insert(0, o);
                        self.layers_version += 1;
                        changed = true;
                        id
                    }
                };
                // Titelseite ohne Kopf-/Fußzeile (bei nur einer Seite gibt es sie doch)
                let page_no = if self.doc.bands_skip_first && i == 0 && n > 1 {
                    0
                } else {
                    i as u32 + 1
                };
                let date = self.date_text.clone();
                let Some(o) = self.doc.object_mut(id) else {
                    continue;
                };
                let before = o.clone();
                o.style.fill = src_obj.style.fill;
                if let (
                    Shape::TextArea {
                        xf,
                        w,
                        h,
                        text,
                        family,
                        size,
                        bold,
                        italic,
                        fmt,
                        frame,
                        rich,
                    },
                    Shape::TextArea {
                        text: st,
                        family: sf,
                        size: ss,
                        bold: sb,
                        italic: si,
                        fmt: sfmt,
                        rich: sr,
                        ..
                    },
                ) = (&mut o.shape, &src_obj.shape)
                {
                    *xf = Affine::translate(bx.x0, bx.y0);
                    *w = bx.width().max(1.0);
                    *h = bx.height().max(1.0);
                    *text = st.clone();
                    *rich = sr.clone();
                    *family = sf.clone();
                    *size = *ss;
                    *bold = *sb;
                    *italic = *si;
                    *fmt = *sfmt;
                    frame.band = kind;
                    frame.body = false;
                    frame.inset = 0.0;
                    frame.no_flow = true;
                    frame.page_no = page_no;
                    frame.page_count = n as u32;
                    frame.date = date;
                    frame.editing = editing == Some(id);
                }
                if *o != before {
                    changed = true;
                }
            }
            self.band_hash[k] = content(&src_obj);
        }
        if changed {
            self.rev += 1;
        }
        changed
    }

    /// Kopf- bzw. Fußzeile auf Seite `page` bearbeiten (fehlt sie, entsteht sie – auf allen
    /// Seiten); der Cursor kommt an `p` bzw. ans Ende.
    fn start_band(&mut self, kind: u8, page: usize, p: Option<Pt>, m: &dyn Metrics) -> bool {
        if self.doc.view != ViewMode::Notebook || self.read_only {
            return false;
        }
        let n = self.doc.artboard_rects().len();
        if page >= n || (self.doc.bands_skip_first && page == 0 && n > 1) {
            return false;
        }
        self.end_text_edit();
        if !self.has_band(kind) {
            // neu: Schrift wie der Fließtext, etwas kleiner, mittig
            let (family, size, fill) = match self.body_fmt() {
                Some(_) => {
                    let id = self.body_frames()[0];
                    let o = self.doc.object(id);
                    (
                        o.and_then(|o| o.shape.text_fields().map(|t| t.1.to_string()))
                            .unwrap_or_else(|| self.text_defaults.family.clone()),
                        o.and_then(|o| o.shape.text_fields().map(|t| t.2))
                            .map(|s| (s * 0.75).round().max(6.0))
                            .unwrap_or(12.0),
                        o.and_then(|o| o.style.fill),
                    )
                }
                None => (self.text_defaults.family.clone(), 12.0, None),
            };
            let r = self.doc.artboard_rects()[page].1;
            let bx = self.band_box(kind, r);
            self.checkpoint(None);
            let id = self.doc.alloc_id();
            let o = Object {
                group: 0,
                sub: Vec::new(),
                mask: false,
                locked: false,
                smart: None,
                name: if kind == crate::model::BAND_HEADER {
                    tr("Header").to_string()
                } else {
                    tr("Footer").to_string()
                },
                id,
                shape: Shape::TextArea {
                    xf: Affine::translate(bx.x0, bx.y0),
                    w: bx.width().max(1.0),
                    h: bx.height().max(1.0),
                    text: String::new(),
                    family,
                    size,
                    bold: false,
                    italic: false,
                    fmt: TextFormat {
                        align: crate::model::Align::Center,
                        valign: crate::model::VAlign::Middle,
                        line_auto: true,
                        ..TextFormat::default()
                    },
                    frame: TextFrame {
                        band: kind,
                        no_flow: true,
                        ..TextFrame::default()
                    },
                    rich: RichText::default(),
                },
                style: Style {
                    fill: fill.or(Some(Color::rgb(0.33, 0.33, 0.33))),
                    stroke: None,
                    ..Style::default()
                },
                angle: 0.0,
            };
            let li = self.body_layer();
            self.doc.layers[li].objects.insert(0, o);
            self.layers_version += 1;
            // als Bearbeitung beginnen, damit der Abgleich die leere Zeile nicht entfernt
            self.text_edit = Some(TextEdit {
                id,
                caret: 0,
                anchor: 0,
            });
        }
        self.sync_bands();
        let Some(id) = self
            .band_frames(kind)
            .into_iter()
            .find(|f| f.0 == page)
            .map(|f| f.1)
        else {
            return false;
        };
        let caret = p.and_then(|p| self.doc.object(id).and_then(|o| m.text_index(o, p)));
        self.selection = vec![id];
        self.begin_text_edit(id, caret.or(Some(usize::MAX)));
        self.sync_bands();
        true
    }

    /// Kopf- bzw. Fußzeile an (beginnt auf der aktuellen Seite) oder aus (entfernt sie auf
    /// allen Seiten).
    pub fn set_band(&mut self, kind: u8, on: bool, m: &dyn Metrics) -> bool {
        if on == self.has_band(kind) {
            return false;
        }
        if on {
            let n = self.doc.artboard_rects().len();
            let mut page = self.page_index().unwrap_or(0);
            if self.doc.bands_skip_first && page == 0 && n > 1 {
                page = 1;
            }
            return self.start_band(kind, page, None, m);
        }
        if self.editing_band() == Some(kind) {
            self.text_edit = None;
        }
        self.checkpoint(None);
        let ids: Vec<u64> = self.band_frames(kind).into_iter().map(|f| f.1).collect();
        for id in &ids {
            self.doc.remove_object(*id);
        }
        self.selection.retain(|x| !ids.contains(x));
        self.band_hash[usize::from(kind == crate::model::BAND_FOOTER)] = 0;
        self.layers_version += 1;
        true
    }

    /// Kopf- und Fußzeile nicht auf der ersten Seite (Titelseite).
    pub fn set_bands_skip_first(&mut self, on: bool) -> bool {
        if self.doc.bands_skip_first == on {
            return false;
        }
        self.checkpoint(None);
        self.doc.bands_skip_first = on;
        // wird gerade die Zeile der ersten Seite bearbeitet, endet das
        if let (true, Some(kind), Some(te)) = (on, self.editing_band(), self.text_edit) {
            if self.band_frames(kind).iter().any(|f| f.0 == 0 && f.1 == te.id) {
                self.text_edit = None;
                self.selection.clear();
            }
        }
        self.sync_bands();
        true
    }

    /// Feld (Seitenzahl, Seitenanzahl, Datum) an der Cursorstelle einfügen – nur in einer
    /// Kopf- oder Fußzeile.
    pub fn insert_field(&mut self, c: char) -> bool {
        if self.editing_band().is_none() || !crate::model::is_field(c) {
            return false;
        }
        self.edit_insert(&c.to_string())
    }

    /// Gehört das Objekt zum Fließtext?
    pub fn is_body_id(&self, id: u64) -> bool {
        self.doc.object(id).is_some_and(|o| is_body(&o.shape))
    }

    /// Nachbarfeld des Fließtexts (`dir` −1: davor, +1: dahinter).
    fn body_neighbour(&self, id: u64, dir: isize) -> Option<u64> {
        let frames = self.body_frames();
        let i = frames.iter().position(|x| *x == id)? as isize + dir;
        (i >= 0).then(|| frames.get(i as usize).copied()).flatten()
    }

    /// Fließtext: Umbruch zwischen zwei Seiten entfernen (Rücktaste am Anfang einer Seite
    /// bzw. Entf am Ende). Steht dort kein Absatzumbruch, wird das letzte bzw. erste Zeichen
    /// der Nachbarseite gelöscht.
    fn body_join(&mut self, id: u64, backward: bool) -> bool {
        let (prev, next) = if backward {
            match self.body_neighbour(id, -1) {
                Some(p) => (p, id),
                None => return false,
            }
        } else {
            match self.body_neighbour(id, 1) {
                Some(n) => (id, n),
                None => return false,
            }
        };
        let joined = matches!(self.doc.object(next).map(|o| &o.shape), Some(Shape::TextArea { frame, .. }) if frame.joined);
        if !joined {
            self.checkpoint(Some("type"));
            if let Some(Shape::TextArea { frame, .. }) = self.doc.object_mut(next).map(|o| &mut o.shape) {
                frame.joined = true;
            }
            return true;
        }
        let Some(t) = self
            .doc
            .object(if backward { prev } else { next })
            .and_then(|o| o.shape.text_fields())
            .map(|t| t.0.to_string())
        else {
            return false;
        };
        if backward {
            let len = t.len();
            self.text_edit = Some(TextEdit {
                id: prev,
                caret: len,
                anchor: len,
            });
            self.edit_backspace()
        } else {
            self.text_edit = Some(TextEdit {
                id: next,
                caret: 0,
                anchor: 0,
            });
            self.edit_delete()
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
            // wie die erste Zeichenfläche; lässt sich danach einzeln ändern
            background: self.doc.background,
            free: None,
            auto: false,
        });
        if !self.doc.page_src.is_empty() {
            self.doc.page_src.push(usize::MAX);
        }
        // Notizheft: unter die anderen Seiten und gleich dorthin
        if self.doc.view.pages() {
            self.keep_stacked();
            let n = self.doc.artboards.len();
            self.go_page(n, false);
            self.layers_version += 1;
            return n;
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
        let base = trf("{} (copy)", &[&name]);
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
            background: self.doc.artboard_backgrounds().get(i).copied().flatten(),
            free: None,
            auto: false,
        });
        if !self.doc.page_src.is_empty() {
            self.doc.page_src.push(usize::MAX);
        }
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
            crate::model::regroup_ids(&mut o, &mut regroup, &mut next_group);
            o.shape.translate(offset);
            ids.push(o.id);
            self.doc.layers[li].objects.push(o);
        }
        self.selection.clear();
        self.layers_version += 1;
        if self.doc.view.pages() {
            self.keep_stacked();
            let n = self.doc.artboards.len();
            self.go_page(n, false);
            return Some(n);
        }
        self.needs_fit = true;
        Some(self.doc.artboards.len())
    }

    /// Zusätzliche Zeichenfläche entfernen (die Objekte darauf bleiben erhalten).
    /// Jede Zeichenfläche lässt sich entfernen, solange eine weitere bleibt. Fällt die erste
    /// weg, wird die nächste zur Hauptzeichenfläche: sie rückt in den Ursprung, alles andere
    /// wandert mit, und die Ansicht folgt, so dass auf dem Bildschirm nichts springt.
    pub fn delete_artboard(&mut self, i: usize) -> bool {
        if self.doc.artboards.is_empty() || i > self.doc.artboards.len() || self.temporary_marks() {
            return false;
        }
        self.checkpoint(None);
        self.remove_artboard(i);
        true
    }

    /// Seite entfernen – mit allem, was auf ihr liegt (Betrachtung eines PDFs: dort gehören
    /// die Objekte zur Seite).
    pub fn delete_page(&mut self, i: usize, m: &dyn Metrics) -> bool {
        let Some((_, r)) = self.doc.artboard_rects().get(i).cloned() else {
            return false;
        };
        if self.doc.artboards.is_empty() || self.temporary_marks() {
            return false;
        }
        self.checkpoint(None);
        for l in &mut self.doc.layers {
            l.objects.retain(|o| {
                !m.bbox(o)
                    .is_some_and(|b| r.contains(Pt::new((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0)))
            });
        }
        self.selection.clear();
        self.node_sel.clear();
        self.remove_artboard(i);
        true
    }

    /// Zeigt die Zeichenfläche `i`: im Notizheft blättern, sonst sie in die Bildmitte holen
    /// (verkleinert, falls sie nicht ganz ins Bild passt).
    pub fn show_artboard(&mut self, i: usize) -> bool {
        if self.notebook.is_some() {
            return self.go_page(i, false);
        }
        let Some((_, r)) = self.doc.artboard_rects().get(i).cloned() else {
            return false;
        };
        let (w, h) = self.viewport;
        if w <= 0.0 || h <= 0.0 || r.width() <= 0.0 || r.height() <= 0.0 {
            return false;
        }
        let m = PAGE_MARGIN;
        let z = self
            .view
            .zoom
            .min((w - 2.0 * m).max(1.0) / r.width())
            .min((h - 2.0 * m).max(1.0) / r.height());
        self.view.zoom = z;
        self.view.ox = ((w - r.width() * z) / 2.0 - r.x0 * z).round();
        self.view.oy = ((h - r.height() * z) / 2.0 - r.y0 * z).round();
        true
    }

    /// Zeichenfläche `i` entfernen (ohne Rückgängig-Punkt).
    fn remove_artboard(&mut self, i: usize) {
        if i < self.doc.page_src.len() {
            self.doc.page_src.remove(i);
        }
        // vorgegebene Namen („Seite 3“) rücken mit nach
        let mut defaults: Vec<bool> = self
            .doc
            .artboard_rects()
            .iter()
            .enumerate()
            .map(|(k, (name, _))| self.doc.is_default_artboard_name(name, k))
            .collect();
        defaults.remove(i);
        if i == 0 {
            let a = self.doc.artboards.remove(0);
            self.doc.width = a.w;
            self.doc.height = a.h;
            self.doc.background = a.background;
            self.doc.first_name = if defaults[0] { String::new() } else { a.name };
            self.shift_document(Pt::new(-a.x, -a.y));
        } else {
            self.doc.artboards.remove(i - 1);
        }
        let names: Vec<String> = (0..defaults.len())
            .map(|k| self.doc.default_artboard_name(k))
            .collect();
        for (k, a) in self.doc.artboards.iter_mut().enumerate() {
            if defaults[k + 1] {
                a.name = names[k + 1].clone();
            }
        }
        // Notizheft: auf derselben (bzw. der vorigen) Seite bleiben
        if let Some(p) = self.notebook {
            let p = if p >= i && p > 0 { p - 1 } else { p };
            self.notebook = Some(p.min(self.doc.artboards.len()));
        }
        self.keep_stacked();
        self.layers_version += 1;
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
        self.keep_stacked();
        self.layers_version += 1;
        true
    }

    pub fn rename_artboard(&mut self, i: usize, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() || i > self.doc.artboards.len() {
            return false;
        }
        if i == 0 {
            // der vorgegebene Name wird nicht eigens gespeichert
            let new = if self.doc.is_default_artboard_name(name, 0) {
                String::new()
            } else {
                name.to_string()
            };
            if new == self.doc.first_name {
                return false;
            }
            self.checkpoint(None);
            self.doc.first_name = new;
        } else {
            if self.doc.artboards[i - 1].name == name {
                return false;
            }
            self.checkpoint(None);
            self.doc.artboards[i - 1].name = name.to_string();
        }
        self.layers_version += 1;
        true
    }

    /// Alles im Dokument (Objekte, Hilfslinien, weitere Zeichenflächen) um `t` verschieben;
    /// die Ansicht folgt, so dass auf dem Bildschirm nichts springt.
    fn shift_document(&mut self, t: Pt) {
        if t.x == 0.0 && t.y == 0.0 {
            return;
        }
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
            if let Some((fx, fy)) = &mut b.free {
                *fx += t.x;
                *fy += t.y;
            }
        }
        self.view.ox -= t.x * self.view.zoom;
        self.view.oy -= t.y * self.view.zoom;
    }

    /// Zeichenfläche `from` an die Stelle `to` verschieben (Reihenfolge der Seiten beim
    /// Blättern und im PDF; die Lage auf der Zeichenfläche bleibt). Vorgegebene Namen
    /// („Seite 3“) werden neu durchgezählt, eigene bleiben. Wird eine andere Zeichenfläche
    /// zur ersten, rückt sie in den Ursprung und alles andere wandert mit.
    pub fn move_artboard(&mut self, from: usize, to: usize) -> bool {
        let rects = self.doc.artboard_rects();
        let n = rects.len();
        // eigenes Dokument in der Präsentation: nichts ändern
        if from >= n || to >= n || from == to || self.temporary_marks() {
            return false;
        }
        self.checkpoint(None);
        let bgs = self.doc.artboard_backgrounds();
        let frees: Vec<Option<(f64, f64)>> = std::iter::once(None)
            .chain(self.doc.artboards.iter().map(|a| a.free))
            .collect();
        let autos: Vec<bool> = std::iter::once(false)
            .chain(self.doc.artboards.iter().map(|a| a.auto))
            .collect();
        // alle Zeichenflächen als eine Liste; leerer Name = vorgegebener Name
        let mut all: Vec<crate::model::Artboard> = rects
            .iter()
            .zip(bgs)
            .enumerate()
            .map(|(i, ((name, r), bg))| crate::model::Artboard {
                name: if self.doc.is_default_artboard_name(name, i) {
                    String::new()
                } else {
                    name.clone()
                },
                x: r.x0,
                y: r.y0,
                w: r.width(),
                h: r.height(),
                background: bg,
                free: frees.get(i).copied().flatten(),
                auto: autos.get(i).copied().unwrap_or(false),
            })
            .collect();
        let a = all.remove(from);
        all.insert(to, a);
        if self.doc.page_src.len() == n {
            let s = self.doc.page_src.remove(from);
            self.doc.page_src.insert(to, s);
        }
        for (i, a) in all.iter_mut().enumerate().skip(1) {
            if a.name.is_empty() {
                a.name = self.doc.default_artboard_name(i);
            }
        }
        let first = all.remove(0);
        self.doc.width = first.w;
        self.doc.height = first.h;
        self.doc.background = first.background;
        self.doc.first_name = first.name;
        self.doc.artboards = all;
        // alles so verschieben, dass die neue erste Zeichenfläche im Ursprung liegt
        self.shift_document(Pt::new(-first.x, -first.y));
        if let Some(p) = self.notebook {
            self.notebook = Some(if p == from {
                to
            } else if from < p && p <= to {
                p - 1
            } else if to <= p && p < from {
                p + 1
            } else {
                p
            });
        }
        self.keep_stacked();
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
        if matches!(handle, 2..=4) {
            x1 = q.x.max(x0 + 1.0);
        }
        if matches!(handle, 0..=2) {
            y0 = q.y.min(y1 - 1.0);
        }
        if matches!(handle, 4..=6) {
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
            self.message = Some(tr("Locked objects cannot be changed").into());
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
        // gewählte Zellen einer Tabelle: nur ihren Inhalt löschen
        if let (Some(a), Some(pk)) = (self.smart_target(), self.active_pick()) {
            if let Some(crate::smart::Smart::Table(mut t)) = self.smart_data(a).cloned() {
                if let Some((r0, c0, r1, c1)) = pk.cell_range(t.n_rows(), t.n_cols()) {
                    for row in &mut t.cells[r0..=r1] {
                        for cell in &mut row[c0..=c1] {
                            cell.clear();
                        }
                    }
                    let done = self.set_smart(a, crate::smart::Smart::Table(t));
                    self.selection = self.doc.group_members(a);
                    return done;
                }
            }
        }
        if self.selection.is_empty() {
            return false;
        }
        if self.selection_locked() {
            self.message = Some(tr("Locked objects cannot be deleted").into());
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
            crate::model::regroup_ids(&mut o, &mut regroup, &mut next_group);
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
            self.message = Some(tr("Locked objects cannot be changed").into());
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
            self.message = Some(tr("Locked objects cannot be changed").into());
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
            // Schnittmaske aus Text: bleibt ein einziges Objekt (ein zusammengesetzter Pfad)
            if self.doc.object(id).is_some_and(|o| o.mask) {
                if let Some(o) = self.doc.object_mut(id) {
                    o.shape = path(parts.concat());
                }
                selection.push(id);
                continue;
            }
            // mehrere Buchstaben: je ein Pfad, als Gruppe an der Stelle des Textes
            let Some((li, oi)) = self.doc.locate(id) else {
                continue;
            };
            let orig = self.doc.layers[li].objects[oi].clone();
            // die Buchstaben bilden eine eigene Gruppe (in einer Gruppe als Untergruppe)
            next_group += 1;
            let (group, sub) = if orig.group != 0 {
                let mut sub = orig.sub.clone();
                sub.push(next_group);
                (orig.group, sub)
            } else {
                (next_group, Vec::new())
            };
            let mut objs = Vec::new();
            for (k, subs) in parts.into_iter().enumerate() {
                let mut o = orig.clone();
                if k > 0 {
                    o.id = self.doc.alloc_id();
                }
                o.shape = path(subs);
                o.group = group;
                o.sub = sub.clone();
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
            self.message = Some(tr("Locked objects cannot be changed").into());
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
        self.rebuild_selected_smart();
        true
    }

    /// Tabellen und Diagramme der Auswahl nach Drehen, Spiegeln oder Skalieren neu aufbauen,
    /// damit Rahmen und Teile zusammenpassen.
    fn rebuild_selected_smart(&mut self) {
        for a in self.smart_anchors() {
            self.rebuild_smart(a);
        }
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
            self.message = Some(tr("Locked objects cannot be changed").into());
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
        for a in self.smart_anchors() {
            if let Some(o) = self.doc.object_mut(a) {
                o.angle = target;
            }
        }
        self.rebuild_selected_smart();
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
            self.message = Some(tr("Locked objects cannot be changed").into());
            return false;
        }
        let mut items: Vec<(usize, usize, u64)> = self
            .selection
            .iter()
            .filter_map(|id| self.doc.locate(*id).map(|(l, o)| (l, o, *id)))
            .collect();
        if items.len() < 2 {
            self.message = Some(tr("Select at least two objects for this").into());
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
        // mit echten Kurven; nur wenn das nicht verlässlich gelingt, mit Vielecken
        let result = crate::shapeops::boolean_curves(&shapes, op, tol)
            .unwrap_or_else(|| boolean::apply_many(&shapes, op, tol));
        if result.is_empty() {
            self.message = Some(tr("The result would be empty").into());
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
                sub: Vec::new(),
                mask: false,
                locked: false,
                smart: None,
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
                "The layer “{}” is locked or hidden",
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
            self.message = Some(tr("Click a path or shape to align text along it").into());
            return;
        };
        let Some(obj) = self.doc.object(id) else { return };
        match &obj.shape {
            Shape::TextPath { .. } | Shape::Text { .. } => {
                self.text_click(p, m);
            }
            Shape::Image { .. } => {
                self.message = Some(tr("Text cannot be aligned along an image").into());
            }
            shape => {
                let Some(Shape::Path { subpaths, .. }) = shape.to_path() else {
                    return;
                };
                let t = self.text_defaults.clone();
                let new_shape = Shape::TextPath {
                    path: subpaths,
                    text: tr("Text on Path").into(),
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

    /// Hintergrund einer einzelnen Zeichenfläche ändern (0 = erste).
    pub fn set_artboard_background(&mut self, i: usize, bg: Option<Color>) -> bool {
        if self.doc.artboard_backgrounds().get(i).copied() == Some(bg) || i > self.doc.artboards.len() {
            return false;
        }
        self.checkpoint(None);
        self.doc.set_artboard_background(i, bg);
        self.layers_version += 1;
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
        let mut name = trf("Layer {}", &[&(n)]);
        let mut k = n;
        while self.doc.layers.iter().any(|l| l.name == name) {
            k += 1;
            name = trf("Layer {}", &[&(k)]);
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
            self.message = Some(tr("The last layer cannot be deleted").into());
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
        let mut info = ContextInfo {
            body: self.doc.view == ViewMode::Notebook
                && sel.iter().any(|o| is_body(&o.shape) || is_band(&o.shape)),
            wrap: self.wrap_state(),
            wrap_gap: self
                .wrap_targets()
                .first()
                .and_then(|id| self.doc.object(*id))
                .map(|o| o.style.fx.wrap_gap())
                .unwrap_or(crate::model::WRAP_GAP),
            wrap_box: self
                .wrap_targets()
                .first()
                .and_then(|id| self.doc.object(*id))
                .is_some_and(|o| o.style.fx.wrap_box),
            flow: sel.iter().find_map(|o| match &o.shape {
                Shape::TextArea { frame, .. } => Some(!frame.no_flow),
                _ => None,
            }),
            pick: None,
            smart_look: None,
            smart_paint: false,
            smart_text: false,
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
            para: view
                .as_ref()
                .map(|v| v.2)
                .unwrap_or(ParaFmt::plain(self.text_defaults.fmt.list)),
            show_node: self.tool == Tool::Node && !sel.is_empty(),
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
            // eine gewählte Hilfslinie zeigt ihre Einstellungen (auch mit dem Auswahlwerkzeug)
            empty_hint: self.tool == Tool::Select && sel.is_empty() && self.guide_sel.is_none(),
            text_color: view
                .as_ref()
                .and_then(|(cf, _, _)| cf.color)
                .or_else(|| sel.iter().find(|o| o.shape.is_text()).and_then(|o| o.style.fill)),
            smart: self
                .cell_edit
                .as_ref()
                .map(|c| c.anchor)
                .or_else(|| self.single_smart())
                .and_then(|a| Some((a, self.smart_data(a)?.clone()))),
            cell: self
                .cell_input()
                .filter(|c| self.cell_edit.is_some() || self.single_smart() == Some(c.0)),
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
            show_guides_panel: self.tool == Tool::Guide || self.guide_sel.is_some(),
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
                    Tool::Select => tr("Select").into(),
                    Tool::Node => tr("Edit Nodes").into(),
                    Tool::Shape => tr("New Shape").into(),
                    Tool::Pen => tr("Pen").into(),
                    Tool::Pencil => tr("Freehand").into(),
                    Tool::Text => tr("New Text").into(),
                    Tool::TextPath => tr("Text on Path").into(),
                    Tool::TextArea => tr("New Text Box").into(),
                    Tool::Curvature => tr("Curvature Pen").into(),
                    Tool::Guide => tr("Guides").into(),
                    Tool::Eraser => tr("Eraser").into(),
                    Tool::Scissors => tr("Scissors").into(),
                    Tool::Builder => tr("Shape Builder").into(),
                    Tool::Data => tr("New Table").into(),
                },
                1 => match &sel[0].shape {
                    Shape::Image { clip: Some(_), .. } => tr("Framed Image").into(),
                    s => s.kind_name().into(),
                },
                n => trf("{} objects", &[&(n)]),
            },
        };
        // beim Schreiben: Ausrichtung und Zeilenabstand des Absatzes am Cursor
        if self.rich_range().is_some() {
            info.font.fmt = info.para.apply(&info.font.fmt);
        }
        self.smart_info(&mut info);
        info
    }

    /// Stiländerung auf die Auswahl anwenden oder – ohne Auswahl – als Vorgabe merken.
    pub fn update_style(&mut self, key: &str, f: impl Fn(&mut Style)) -> bool {
        // Tabelle bzw. Diagramm: die gewählten Zellen bzw. Teile
        if let Some(a) = self.smart_target() {
            return self.smart_style(a, key, &f);
        }
        if self.selection.is_empty() {
            if matches!(self.tool, Tool::Text | Tool::TextPath | Tool::TextArea) {
                f(&mut self.text_style);
                self.text_style.fold_opacity();
                self.text_style.strip_effects();
            } else {
                f(&mut self.defaults);
                self.defaults.fold_opacity();
                self.defaults.strip_effects();
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
        // Gewählte Werte auch als Vorgabe für neue Objekte merken – ohne Effekte (die
        // kommen bei neuen Objekten erst nachträglich dazu).
        if !matches!(self.tool, Tool::Text | Tool::TextPath | Tool::TextArea) {
            f(&mut self.defaults);
            self.defaults.fold_opacity();
            self.defaults.strip_effects();
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
            self.message = Some(tr("No areas were found in the image").into());
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
                    sub: Vec::new(),
                    mask: false,
                    locked: false,
                    smart: None,
                    name: if n == 0 {
                        tr("Traced").into()
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
        // Auch Änderungen ohne eigene Rückgängig-Stufe zählen (Pfeiltasten, Werte in der
        // Seitenleiste werden zusammengefasst) – sonst sähe das Werkzeug alte Umrisse.
        let stamp = (self.rev, self.edits);
        let key = (stamp, v.zoom, v.ox, v.oy, self.viewport);
        let scope = self.build_scope();
        if let Some(c) = self.bucket.borrow().as_ref() {
            // Ansichtsänderungen spielen nur bei sehr vielen Kurven eine Rolle
            if c.key.0 == stamp && c.ids == scope && (!c.culled || c.key == key) {
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
                sub: Vec::new(),
                mask: false,
                locked: false,
                smart: None,
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
        self.keep_builder_scope(&[], &[id]);
        true
    }

    /// Loslassen: eine Fläche angeklickt → neue Form; mehrere überstrichen → vereinigen
    /// (Alt: löschen). Beteiligte Formen zerfallen in ihre übrigen Flächen.
    fn builder_apply(&mut self, hits: &[usize], alt: bool, m: &dyn Metrics) -> bool {
        let arr = self.build_arrangement(m);
        let hits: Vec<usize> = hits.iter().copied().filter(|i| *i < arr.faces.len()).collect();
        let Some(&first) = hits.first() else {
            self.message = Some(tr("There is no closed area here").into());
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
                    sub: Vec::new(),
                    mask: false,
                    locked: false,
                    smart: None,
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
        let mut added = Vec::new();
        for (subs, style) in pieces {
            added.push(add(&mut self.doc, subs, style, &mut pos));
        }
        if let Some((subs, style)) = merged {
            added.push(add(&mut self.doc, subs, style, &mut pos));
        }
        self.keep_builder_scope(&remove, &added);
        self.node_sel.clear();
        true
    }

    /// Nach einem Schritt der Formerstellung: Die Auswahl bestimmt, mit welchen Formen
    /// gearbeitet wird. War nichts ausgewählt, bleibt es so (alle Formen); sonst ersetzen die
    /// neuen Formen die verbrauchten. Früher war danach nur die neue Form ausgewählt – das
    /// Werkzeug sah dann nur noch sie und fand sonst keine Flächen mehr.
    fn keep_builder_scope(&mut self, removed: &[u64], added: &[u64]) {
        if self.selection.is_empty() {
            return;
        }
        self.selection.retain(|id| !removed.contains(id));
        self.selection.extend_from_slice(added);
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
            self.message = Some(tr("Locked objects cannot be changed").into());
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
            if whole != 0 {
                c.group = whole;
                c.sub.clear();
            } else {
                crate::model::regroup_ids(&mut c, &mut regroup, &mut next);
            }
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
        // beim Schreiben: Schrift und Absatz am Cursor bzw. der Markierung
        if text {
            if let Some((id, ..)) = self.rich_range() {
                let o = self.doc.object(id)?;
                let (cf, _, pf) = self.text_view(&[o])?;
                let base = o.shape.text_format()?;
                let mut fmt = pf.apply(&base);
                fmt.before = pf.space_before(&base);
                fmt.para = pf.space_after(&base);
                fmt.weight = cf.weight;
                fmt.underline = cf.underline;
                fmt.strike = cf.strike;
                fmt.list = crate::model::ListStyle::None;
                let mut style = o.style.clone();
                if let Some(c) = cf.color {
                    style.fill = Some(c);
                }
                return Some(crate::library::LibStyle {
                    style,
                    text: Some(crate::library::TextStyle {
                        family: cf.family,
                        size: cf.size,
                        bold: cf.bold,
                        italic: cf.italic,
                        fmt,
                    }),
                    name: String::new(),
                });
            }
        }
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

    /// Farbe aus der Bibliothek als Füllung oder Kontur übernehmen. Mit `link` (Kennung der
    /// Bibliotheksfarbe) bleibt das Objekt mit ihr verknüpft (globale Farbe).
    pub fn apply_lib_color(
        &mut self,
        c: Color,
        g: Option<Gradient>,
        link: Option<String>,
        stroke: bool,
    ) -> bool {
        let t = if stroke {
            PaintTarget::Stroke
        } else {
            PaintTarget::Fill
        };
        let link = link
            .filter(|_| g.is_none())
            .map(|id| crate::model::ColorLink { id, color: c });
        self.update_style("lib-color", move |st| {
            st.set_paint(t, Some(c), g.clone());
            if stroke {
                st.stroke_link = link.clone();
            } else {
                st.fill_link = link.clone();
            }
        })
    }

    /// Globale Farben: verknüpfte Farben auf den Stand der Bibliotheken bringen (`lookup`:
    /// Kennung → aktuelle Farbe). Rückgängig machbar; liefert die Zahl geänderter Farben.
    pub fn sync_library_colors(&mut self, lookup: &dyn Fn(&str) -> Option<Color>) -> usize {
        let mut d = self.doc.clone();
        let n = d.sync_linked_colors(lookup);
        if n > 0 {
            self.checkpoint(None);
            self.doc = d;
        }
        n
    }

    /// Gespeicherten Stil übernehmen: Darstellung komplett, bei Texten auch die Schrift.
    pub fn apply_lib_style(&mut self, s: &crate::library::LibStyle, m: &dyn Metrics) -> bool {
        // Tabelle bzw. Diagramm: Schrift und Textfarbe der gewählten Teile
        if let (Some(a), Some(t)) = (self.smart_target(), &s.text) {
            let td = TextDefaults {
                family: t.family.clone(),
                size: t.size,
                bold: t.bold,
                italic: t.italic,
                fmt: t.fmt,
            };
            let color = s.style.fill;
            return self.edit_looks(a, "lib-style", true, &move |l| {
                // die senkrechte Lage in der Zelle bleibt
                let v = l.valign;
                set_look_text(l, &td);
                l.valign = v;
                if let Some(c) = color {
                    l.color = c;
                }
            });
        }
        // beim Schreiben in einem Textfeld: Teil eines Absatzes markiert – nur dessen Schrift;
        // sonst (Cursor, ganze oder mehrere Absätze) ist der Stil ein Absatzformat: Schrift der
        // ganzen Absätze, Ausrichtung, Zeilenabstand und Abstände
        if let (Some(t), Some((id, a, b))) = (&s.text, self.rich_range()) {
            let c = CharFmt {
                family: t.family.clone(),
                size: t.size,
                bold: t.bold,
                weight: t.fmt.weight_for(t.bold),
                italic: t.italic,
                underline: t.fmt.underline,
                strike: t.fmt.strike,
                color: s.style.fill,
            };
            let text = self
                .doc
                .object(id)
                .and_then(|o| o.shape.text_fields())
                .map(|t| t.0.to_string())
                .unwrap_or_default();
            let (a, b) = (a.min(text.len()), b.min(text.len()));
            let pa = text[..a].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let pb = text[b..].find('\n').map(|i| b + i).unwrap_or(text.len());
            let part = a < b && !text[a..b].contains('\n') && (a > pa || b < pb);
            let area = matches!(
                self.doc.object(id).map(|o| &o.shape),
                Some(Shape::TextArea { .. })
            );
            if part || !area {
                return self.set_char_fmt("lib-style", move |f| *f = c.clone());
            }
            let te = self.text_edit;
            self.text_edit = te.map(|t| TextEdit {
                id: t.id,
                caret: pb,
                anchor: pa,
            });
            let mut changed = pa < pb && self.set_char_fmt("lib-style", move |f| *f = c.clone());
            self.text_edit = te;
            let f = t.fmt;
            changed |= self.set_para_fmt("lib-style", move |p| {
                p.align = Some(f.align);
                p.line = Some(if f.line_auto { 0.0 } else { f.line });
                p.before = Some(f.before);
                p.after = Some(f.para);
            });
            return changed;
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
        if let Some(a) = self.smart_target() {
            return self.smart_char(a, key, &f);
        }
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
            color: None,
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
                let mut d = ParaFmt::plain(self.text_defaults.fmt.list);
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
                let mut d = ParaFmt::plain(fmt.list);
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
            return Some((base, FmtMixed::default(), ParaFmt::plain(fmt.list)));
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
        if let Some(a) = self.smart_target() {
            return self.smart_text(a, key, &f);
        }
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
    let right = matches!(handle, 2..=4);
    let top = matches!(handle, 0..=2);
    let bottom = matches!(handle, 4..=6);
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
///
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
                        c.points.push((Pt::new(x, y), tr("Edge centre")));
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
                    tr("Corner"),
                    tr("Centre"),
                    self.snap_corners,
                    self.snap_mids,
                );
                if c.boxes.len() < 600 {
                    c.boxes.push(b);
                }
                if let (true, Shape::Path { subpaths, .. }) = (self.snap_anchors, &o.shape) {
                    for sp in subpaths {
                        for n in &sp.nodes {
                            c.points.push((n.p, tr("Anchor point")));
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
                add_box(&mut c, r, tr("Page corner"), tr("Page centre"), true, true);
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
                label: Some(tr("Guide")),
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
                    label: Some(tr("Path")),
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
    /// Zeiger beim Skalieren einrasten: zuerst der gezogene Anfasser selbst (Ecke/Kante an
    /// Punkten, Kanten, Hilfslinien); sonst Ankerpunkte der Auswahl, die beim Skalieren
    /// mitwandern, an Punkten oder Kanten/Mitten anderer Objekte.
    fn snap_scale(
        &self,
        handle: usize,
        b: &Rect,
        p: Pt,
        mods: Mods,
        uniform: bool,
        orig: &[(u64, Object)],
    ) -> Pt {
        let q = self.snap(p);
        if q.dist(p) > 1e-9 {
            return q;
        }
        let Some(c) = self.snap_ctx.as_ref().filter(|_| self.smart_snap) else {
            return p;
        };
        let tol = self.snap_tol();
        let anchors = Self::anchor_points(orig);
        if anchors.is_empty() {
            return p;
        }
        // Lage der Ankerpunkte hängt je Achse linear vom Zeiger ab: Steigung ausmessen
        let a0 = scale_affine(handle, b, p, mods, uniform);
        let ax = scale_affine(handle, b, p + Pt::new(1.0, 0.0), mods, uniform);
        let ay = scale_affine(handle, b, p + Pt::new(0.0, 1.0), mods, uniform);
        let moved: Vec<(Pt, f64, f64)> = anchors
            .iter()
            .map(|a| {
                let m = a0.apply(*a);
                (m, ax.apply(*a).x - m.x, ay.apply(*a).y - m.y)
            })
            .collect();
        let mut out = p;
        let mut hints = self.hints.borrow_mut();
        // markante Punkte (beide Achsen)
        let mut best_pt: Option<(f64, Pt, Pt)> = None;
        for (m, kx, ky) in &moved {
            if kx.abs() < 1e-6 && ky.abs() < 1e-6 {
                continue;
            }
            for (t, _) in &c.points {
                let d = t.dist(*m);
                if d <= tol && best_pt.map(|x| d < x.0).unwrap_or(true) {
                    // Achsen, die sich nicht bewegen, müssen schon passen
                    let okx = kx.abs() > 1e-6 || (t.x - m.x).abs() < 1e-6;
                    let oky = ky.abs() > 1e-6 || (t.y - m.y).abs() < 1e-6;
                    if okx && oky {
                        best_pt = Some((d, *t, *m));
                    }
                }
            }
        }
        if let Some((_, t, m)) = best_pt {
            let (kx, ky) = moved
                .iter()
                .find(|x| x.0 == m)
                .map(|x| (x.1, x.2))
                .unwrap_or((0.0, 0.0));
            if kx.abs() > 1e-6 {
                out.x += (t.x - m.x) / kx;
            }
            if ky.abs() > 1e-6 {
                out.y += (t.y - m.y) / ky;
            }
            hints.push(SnapHint {
                a: t,
                b: t,
                label: Some(tr("Anchor point")),
            });
            return out;
        }
        // Kanten und Mitten anderer Objekte, je Achse
        let axis = |vals: &[(f64, Pt)], x: bool| {
            let mut r: Option<(f64, f64, f64, Pt, Pt)> = None;
            for (m, kx, ky) in &moved {
                let (v, k) = if x { (m.x, *kx) } else { (m.y, *ky) };
                if k.abs() < 1e-6 {
                    continue;
                }
                for (t, refp) in vals {
                    let corr = t - v;
                    if corr.abs() <= tol && r.map(|o| corr.abs() < o.0.abs()).unwrap_or(true) {
                        r = Some((corr, k, *t, *refp, *m));
                    }
                }
            }
            r
        };
        if let Some((corr, k, t, refp, m)) = axis(&c.xs, true) {
            out.x += corr / k;
            hints.push(SnapHint {
                a: refp,
                b: Pt::new(t, m.y),
                label: None,
            });
        }
        if let Some((corr, k, t, refp, m)) = axis(&c.ys, false) {
            out.y += corr / k;
            hints.push(SnapHint {
                a: refp,
                b: Pt::new(m.x, t),
                label: None,
            });
        }
        out
    }

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
                    label: Some(tr("Guide")),
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
            self.message = Some(tr("The current layer is locked").into());
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
    /// Textfarbe: beim Schreiben die der Markierung (ohne Markierung: für das, was als
    /// Nächstes getippt wird); sonst die Füllung der gewählten Texte, eigene Farben ihrer
    /// Textteile entfallen dann.
    pub fn set_text_color(&mut self, c: Option<Color>) -> bool {
        if self.text_edit.is_some() {
            return self.set_char_fmt("text-color", move |f| f.color = c);
        }
        let changed = self.set_paint(PaintTarget::Fill, c, None);
        let ids: Vec<u64> = self
            .selected_objects()
            .iter()
            .filter(|o| {
                o.shape
                    .rich()
                    .is_some_and(|r| r.runs.iter().any(|t| t.style.color.is_some()))
            })
            .map(|o| o.id)
            .collect();
        for id in ids {
            if let Some(r) = self.doc.object_mut(id).and_then(|o| o.shape.rich_mut()) {
                for t in &mut r.runs {
                    t.style.color = None;
                }
                r.runs.retain(|t| !t.style.is_empty());
            }
        }
        changed
    }

    /// Deckkraft der Textfarbe (wie `set_text_color`).
    pub fn set_text_alpha(&mut self, a: f64) -> bool {
        if self.text_edit.is_some() {
            let sel = self.selected_objects();
            let cur = self
                .text_view(&sel)
                .and_then(|(cf, _, _)| cf.color)
                .or_else(|| sel.iter().find_map(|o| o.style.fill))
                .unwrap_or(Color::BLACK);
            return self.set_char_fmt("text-alpha", move |f| f.color = Some(Color { a, ..cur }));
        }
        self.set_paint_alpha(PaintTarget::Fill, a)
    }

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
    /// Kontur versetzen: um jede ausgewählte Form eine neue Fläche im Abstand `d`
    /// (positiv: nach außen, negativ: nach innen) mit dem Stil des Originals. Nach außen
    /// liegt sie unter dem Original (z. B. als Rand eines Aufklebers), nach innen darüber.
    /// Offene Pfade werden zu einer Fläche der Breite 2·d. Liefert die Zahl neuer Objekte.
    pub fn offset_path(&mut self, d: f64, join: crate::model::LineJoin, m: &dyn Metrics) -> usize {
        if d.abs() < 1e-6 || !d.is_finite() {
            return 0;
        }
        let items: Vec<Object> = self
            .selected_objects()
            .into_iter()
            .filter(|o| !o.mask && !matches!(o.shape, Shape::Image { .. }))
            .cloned()
            .collect();
        let mut made: Vec<(u64, Object)> = Vec::new();
        for o in &items {
            let Some(subs) = Self::outline_of(o, m) else {
                continue;
            };
            let subs = crate::model::effective_subpaths(&subs);
            let closed: Vec<SubPath> = subs.iter().filter(|s| s.closed).cloned().collect();
            let open: Vec<SubPath> = subs.iter().filter(|s| !s.closed).cloned().collect();
            let st = crate::stroker::StrokeStyle {
                width: 2.0 * d.abs(),
                join,
                cap: crate::model::LineCap::Round,
                dash: Vec::new(),
            };
            let tol = 0.25;
            let mut region: Vec<boolean::Ring> = Vec::new();
            if !closed.is_empty() {
                let fill = boolean::to_rings(&closed, tol);
                let band = crate::stroker::stroke_area(&closed, &st);
                region = boolean::rings_op(&fill, &band, if d > 0.0 { Op::Union } else { Op::Difference });
            }
            if d > 0.0 && !open.is_empty() {
                let band = crate::stroker::stroke_area(&open, &st);
                region = if region.is_empty() {
                    band
                } else {
                    boolean::rings_op(&region, &band, Op::Union)
                };
            }
            if region.is_empty() {
                continue;
            }
            let mut style = o.style.clone();
            if open.len() == subs.len() {
                // aus einer Linie wird eine Fläche in der Farbe der Linie
                style.fill = o.style.stroke.or(o.style.fill);
                style.stroke = None;
            }
            style.marker = crate::model::Marker::None;
            made.push((
                o.id,
                Object {
                    id: 0,
                    shape: Shape::Path {
                        subpaths: boolean::rings_to_paths(region),
                        evenodd: true,
                    },
                    style,
                    angle: o.angle,
                    group: o.group,
                    sub: o.sub.clone(),
                    mask: false,
                    locked: false,
                    smart: None,
                    name: String::new(),
                },
            ));
        }
        if made.is_empty() {
            return 0;
        }
        self.checkpoint(None);
        let mut sel = Vec::new();
        for (id, mut obj) in made {
            let Some((li, oi)) = self.doc.locate(id) else {
                continue;
            };
            obj.id = self.doc.alloc_id();
            sel.push(obj.id);
            let at = if d > 0.0 { oi } else { oi + 1 };
            self.doc.layers[li].objects.insert(at, obj);
        }
        let n = sel.len();
        self.selection = sel;
        self.node_sel.clear();
        self.layers_version += 1;
        n
    }

    /// Pfade vereinfachen: weniger Knoten bei fast gleicher Form; Ecken bleiben Ecken.
    /// Mehrmals angewendet wird weiter vereinfacht. Liefert die Zahl gesparter Knoten.
    pub fn simplify_paths(&mut self, m: &dyn Metrics) -> usize {
        if self.selection_locked() {
            self.message = Some(tr("Locked objects cannot be changed").into());
            return 0;
        }
        let mut changes: Vec<(u64, Vec<SubPath>, usize)> = Vec::new();
        for o in self.selected_objects() {
            let Shape::Path { subpaths, .. } = &o.shape else {
                continue;
            };
            let Some(b) = m.bbox(o) else { continue };
            let eps = (b.width().hypot(b.height()) * 0.003).max(0.2);
            let before: usize = subpaths.iter().map(|s| s.nodes.len()).sum();
            let eff = crate::model::effective_subpaths(subpaths);
            let new: Vec<SubPath> = eff.iter().map(|sp| simplify_subpath(sp, eps)).collect();
            let after: usize = new.iter().map(|s| s.nodes.len()).sum();
            if after < before {
                changes.push((o.id, new, before - after));
            }
        }
        if changes.is_empty() {
            return 0;
        }
        self.checkpoint(None);
        let mut saved = 0;
        for (id, new, n) in changes {
            if let Some(Object {
                shape: Shape::Path { subpaths, .. },
                ..
            }) = self.doc.object_mut(id)
            {
                *subpaths = new;
                saved += n;
            }
        }
        self.node_sel.clear();
        saved
    }

    /// Auswahl wiederholen: `count` Kopien, jede gegenüber der vorigen um (dx, dy) verschoben,
    /// um `angle` Grad (gegen den Uhrzeigersinn) gedreht und mit `scale` skaliert – um die
    /// Mitte der Auswahl oder um `pivot`. Original und Kopien werden eine Gruppe.
    #[allow(clippy::too_many_arguments)]
    pub fn repeat_selection(
        &mut self,
        count: usize,
        dx: f64,
        dy: f64,
        angle: f64,
        scale: f64,
        pivot: Option<Pt>,
        m: &dyn Metrics,
    ) -> bool {
        if count == 0 || self.selection.is_empty() || !scale.is_finite() || scale <= 0.0 {
            return false;
        }
        if self.selection_locked() {
            self.message = Some(tr("Locked objects cannot be changed").into());
            return false;
        }
        self.expand_groups();
        let Some(b) = self.selection_bbox(m) else {
            return false;
        };
        let c = pivot.unwrap_or(Pt::new((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0));
        let step = Affine::translate(dx, dy)
            .then_before(&Affine::rotate_about(-angle, c))
            .then_before(&Affine::scale_about(scale, scale, c));
        let originals = self.selected_in_order();
        let count = count.min(1000);
        self.checkpoint(None);
        let mut all: Vec<u64> = originals.iter().map(|o| o.id).collect();
        let mut t = Affine::IDENTITY;
        let mut regroup: Vec<(u64, u64)> = Vec::new();
        let mut next = self.doc.max_group();
        for _ in 0..count {
            t = step.then_before(&t);
            regroup.clear();
            for o in &originals {
                let Some((li, _)) = self.doc.locate(o.id) else {
                    continue;
                };
                let mut c = o.clone();
                c.id = self.doc.alloc_id();
                c.locked = false;
                crate::model::regroup_ids(&mut c, &mut regroup, &mut next);
                let id = c.id;
                self.doc.layers[li].objects.push(c);
                self.transform_object(id, &t, m);
                all.push(id);
            }
        }
        self.selection = all;
        self.group_selection();
        self.layers_version += 1;
        true
    }

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
                    sub: o.sub.clone(),
                    mask: o.mask,
                    locked: false,
                    smart: None,
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
        self.set_paragraph(
            "align",
            move |p| p.align = Some(a),
            move |t| t.fmt.align = a,
            |p| p.align = None,
        )
    }

    /// Absatzformat wie in einer Textverarbeitung: beim Schreiben in einem Textfeld nur die
    /// Absätze der Markierung bzw. des Cursors (`in_edit`), sonst der ganze Text (`whole`) –
    /// dann gleicht `all` die eigenen Angaben der Absätze an.
    pub fn set_paragraph(
        &mut self,
        key: &str,
        in_edit: impl Fn(&mut ParaFmt),
        whole: impl Fn(&mut TextDefaults),
        all: impl Fn(&mut ParaFmt),
    ) -> bool {
        if self.smart_target().is_none() {
            if let Some((id, _, _)) = self.rich_range() {
                if matches!(
                    self.doc.object(id).map(|o| &o.shape),
                    Some(Shape::TextArea { .. })
                ) {
                    return self.set_para_fmt(key, in_edit);
                }
            }
        }
        let changed = self.update_text(key, whole);
        let ids: Vec<u64> = self
            .selected_objects()
            .into_iter()
            .filter(|o| o.shape.rich().is_some_and(|r| !r.paras.is_empty()))
            .map(|o| o.id)
            .collect();
        for id in ids {
            let Some(o) = self.doc.object_mut(id) else {
                continue;
            };
            let text = o.shape.text_fields().map(|t| t.0.to_string()).unwrap_or_default();
            let Some(fmt) = o.shape.text_format() else {
                continue;
            };
            if let Some(r) = o.shape.rich_mut() {
                r.apply_paras(&text, None, fmt.list, &all);
                let tidy = std::mem::take(r).tidy(fmt.list);
                *r = tidy;
            }
        }
        changed
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
                // nicht in Bilder, nicht in Textfelder (sie machen Bildern Platz) und nicht in
                // Teile von Tabellen und Diagrammen
                if o.id == exclude || matches!(o.shape, Shape::Image { .. }) {
                    continue;
                }
                if m.hit(o, p, tol) {
                    // Textfeld bzw. Tabelle/Diagramm obenauf: hier nichts (auch nicht in das,
                    // was darunter liegt)
                    if matches!(o.shape, Shape::TextArea { .. }) || self.in_smart(o.id) {
                        return None;
                    }
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
                    sub: Vec::new(),
                    mask: false,
                    locked: false,
                    smart: None,
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
                no_flow: f.no_flow,
                ..TextFrame::default()
            };
        }
        if let Some((li, oi)) = self.doc.locate(id) {
            let nid = self.doc.alloc_id();
            self.doc.layers[li].objects.insert(
                oi,
                Object {
                    group: 0,
                    sub: Vec::new(),
                    mask: false,
                    locked: false,
                    smart: None,
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
            if let Some(g) = st.fx.glow {
                r = r.inflate(crate::model::glow_radius(g));
            }
            if st.fx.mirror.is_some() {
                r.y1 += b.height() * st.fx.mirror_len + st.fx.mirror_gap.max(0.0);
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
        let masks = self.doc.masks();
        for id in self.selection.clone() {
            let Some(o) = self.doc.object(id) else {
                continue;
            };
            let Some(b) = self.visible_bbox(o, &masks, m) else {
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
            self.message = Some(tr("Locked objects cannot be changed").into());
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
            self.message = Some(tr("Locked objects cannot be changed").into());
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
            self.message = Some(tr("Locked objects cannot be changed").into());
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
            self.message = Some(tr("Locked objects cannot be changed").into());
            return false;
        }
        let objs = self.selected_in_order();
        let (a, b) = (&objs[0], &objs[1]);
        let (Some(pa), Some(pb)) = (blend_outline(&a.shape), blend_outline(&b.shape)) else {
            self.message =
                Some(tr("A blend only works between shapes and paths, not with text or images").into());
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
                sub: Vec::new(),
                mask: false,
                locked: false,
                smart: None,
                name: String::new(),
            };
            // direkt unter der oberen Form einfügen, in Reihenfolge von unten nach oben
            self.doc.layers[lb].objects.insert(ob + i - 1, obj);
            new_ids.push(id);
        }
        if same_layer {
            for o in self.doc.layers[lb].objects.iter_mut() {
                if o.id == a.id || o.id == b.id {
                    if o.group != 0 {
                        o.sub.insert(0, o.group);
                    }
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
/// Vergrößerung eines Textes durch seine Lage (Matrix), für die sichtbare Schriftgröße.
fn text_scale(shape: &Shape) -> f64 {
    match shape {
        Shape::Text { xf, .. } | Shape::TextArea { xf, .. } => (xf.a * xf.d - xf.b * xf.c).abs().sqrt(),
        _ => 1.0,
    }
}

/// Schrift- und Absatzformat (aus der Eigenschaftenleiste oder einem Textstil) in das
/// Aussehen einer Zelle bzw. eines Diagrammtextes übernehmen.
fn set_look_text(l: &mut crate::smart::Look, t: &TextDefaults) {
    l.family = t.family.clone();
    l.size = t.size.max(1.0);
    l.bold = if t.fmt.weight > 0 {
        t.fmt.weight >= 600
    } else {
        t.bold
    };
    l.weight = t.fmt.weight;
    l.italic = t.italic;
    l.align = t.fmt.align;
    l.valign = t.fmt.valign;
    l.underline = t.fmt.underline;
    l.strike = t.fmt.strike;
    l.small_caps = t.fmt.small_caps;
    l.no_ligatures = t.fmt.no_ligatures;
    l.line = t.fmt.line;
    l.tracking = t.fmt.tracking;
}

/// Drängt das Objekt Text weg? (Effekt; Bilder, Tabellen und Diagramme von sich aus)
pub fn pushes_text(o: &Object) -> bool {
    o.style
        .fx
        .wrap
        .unwrap_or(o.smart.is_some() || matches!(o.shape, Shape::Image { .. }))
}

/// Umriss, dem Text ausweicht (im Dokument): Rahmen einer Tabelle bzw. eines Diagramms,
/// Beschnitt bzw. Fläche eines Bildes, Umriss einer Form; Texte als Rechteck.
fn wrap_outline(o: &Object) -> Option<Vec<SubPath>> {
    let rect = |r: Rect| {
        vec![SubPath {
            nodes: [
                Pt::new(r.x0, r.y0),
                Pt::new(r.x1, r.y0),
                Pt::new(r.x1, r.y1),
                Pt::new(r.x0, r.y1),
            ]
            .iter()
            .map(|p| Node::corner(*p))
            .collect(),
            closed: true,
        }]
    };
    if o.smart.is_some() {
        let out = smart_frame(o).map(rect)?;
        let Some(t) = smart_turn(o) else {
            return Some(out);
        };
        let mut sh = Shape::Path {
            subpaths: out,
            evenodd: false,
        };
        sh.transform(&t);
        return match sh {
            Shape::Path { subpaths, .. } => Some(subpaths),
            _ => None,
        };
    }
    match &o.shape {
        Shape::Image { clip: Some(c), .. } => Some(c.clone()),
        Shape::Image { .. } => {
            let q = o.shape.image_quad()?;
            Some(vec![SubPath {
                nodes: q.iter().map(|p| Node::corner(*p)).collect(),
                closed: true,
            }])
        }
        s if s.is_text() => s.geom_bbox().map(rect),
        s => match s.to_path()? {
            Shape::Path { subpaths, .. } => {
                // offene Linien zählen wie geschlossen
                Some(
                    subpaths
                        .into_iter()
                        .map(|mut sp| {
                            sp.closed = true;
                            sp
                        })
                        .collect(),
                )
            }
            _ => None,
        },
    }
}

thread_local! {
    /// Flächen, die Text wegdrängen (mit Abstand), nach Umriss und Einstellungen
    static WRAP_CACHE: RefCell<std::collections::HashMap<u64, Vec<SubPath>>> =
        RefCell::new(std::collections::HashMap::new());
}

/// Fläche, aus der ein Objekt Text wegdrängt (im Dokument): sein Umriss oder das Rechteck
/// darum, um den eingestellten Abstand vergrößert.
pub fn wrap_area(o: &Object) -> Option<Vec<SubPath>> {
    use std::hash::{Hash, Hasher};
    let subs = wrap_outline(o)?;
    let gap = o.style.fx.wrap_gap();
    let boxed = o.style.fx.wrap_box;
    let bbox = |subs: &Vec<SubPath>| {
        Shape::Path {
            subpaths: subs.clone(),
            evenodd: false,
        }
        .geom_bbox()
    };
    if boxed {
        let b = bbox(&subs)?;
        let r = Rect {
            x0: b.x0 - gap,
            y0: b.y0 - gap,
            x1: b.x1 + gap,
            y1: b.y1 + gap,
        };
        return Some(vec![SubPath {
            nodes: [
                Pt::new(r.x0, r.y0),
                Pt::new(r.x1, r.y0),
                Pt::new(r.x1, r.y1),
                Pt::new(r.x0, r.y1),
            ]
            .iter()
            .map(|p| Node::corner(*p))
            .collect(),
            closed: true,
        }]);
    }
    if gap < 0.01 {
        return Some(subs);
    }
    let mut hs = std::collections::hash_map::DefaultHasher::new();
    gap.to_bits().hash(&mut hs);
    for sp in &subs {
        sp.closed.hash(&mut hs);
        for n in &sp.nodes {
            for v in [n.p.x, n.p.y, n.h_in.x, n.h_in.y, n.h_out.x, n.h_out.y, n.r] {
                v.to_bits().hash(&mut hs);
            }
        }
    }
    let key = hs.finish();
    if let Some(hit) = WRAP_CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return Some(hit);
    }
    // Umriss plus ein Band der doppelten Breite rundherum
    let closed = crate::model::effective_subpaths(&subs);
    let tol = 0.5;
    let fill = boolean::to_rings(&closed, tol);
    let st = crate::stroker::StrokeStyle {
        width: 2.0 * gap,
        join: crate::model::LineJoin::Round,
        cap: crate::model::LineCap::Round,
        dash: Vec::new(),
    };
    let band = crate::stroker::stroke_area(&closed, &st);
    let region = if fill.is_empty() {
        band
    } else {
        boolean::rings_op(&fill, &band, Op::Union)
    };
    let out = if region.is_empty() {
        subs
    } else {
        boolean::rings_to_paths(region)
    };
    WRAP_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 400 {
            c.clear();
        }
        c.insert(key, out.clone());
    });
    Some(out)
}

/// Aussparungen eines Textfeldes (Matrix `xf`, Größe `w` × `h`) für Umrisse im Dokument:
/// vereinigt, auf das Feld begrenzt, in Feldkoordinaten.
fn flow_holes(xf: &Affine, w: f64, h: f64, shapes: &[&Vec<SubPath>]) -> Vec<Vec<Pt>> {
    let inv = xf.invert();
    let tol = 0.5;
    let mut region: Vec<boolean::Ring> = Vec::new();
    for subs in shapes {
        let mut sh = Shape::Path {
            subpaths: crate::model::effective_subpaths(subs),
            evenodd: false,
        };
        sh.transform(&inv);
        let Shape::Path { subpaths, .. } = sh else {
            continue;
        };
        let rings = boolean::to_rings(&subpaths, tol);
        region = if region.is_empty() {
            rings
        } else {
            boolean::rings_op(&region, &rings, Op::Union)
        };
    }
    if region.is_empty() {
        return Vec::new();
    }
    let field = vec![vec![
        Pt::new(0.0, 0.0),
        Pt::new(w, 0.0),
        Pt::new(w, h),
        Pt::new(0.0, h),
    ]];
    boolean::rings_op(&region, &field, Op::Intersection)
        .into_iter()
        .filter(|r| r.len() >= 3 && boolean::area(r).abs() > 0.25)
        .collect()
}

/// Rahmen einer Tabelle bzw. eines Diagramms: das Rechteck des Rahmenobjekts (sonst sein
/// Umriss, etwa nach dem Drehen).
/// Grundformat des Fließtexts (für alle seine Felder gleich).
#[derive(Clone, Debug, PartialEq)]
struct BodyBase {
    family: String,
    size: f64,
    bold: bool,
    italic: bool,
    fmt: TextFormat,
    /// Textfarbe
    fill: Option<Color>,
}

/// Text jeder Art (Punkttext, Textfeld, Text auf Pfad)?
fn is_text_shape(sh: &Shape) -> bool {
    matches!(
        sh,
        Shape::Text { .. } | Shape::TextArea { .. } | Shape::TextPath { .. }
    )
}

/// Kopf- oder Fußzeile des Notizhefts?
pub fn is_band(sh: &Shape) -> bool {
    matches!(sh, Shape::TextArea { frame, .. } if frame.band != 0)
}

/// Gehört die Form zum Fließtext des Notizhefts?
pub fn is_body(sh: &Shape) -> bool {
    matches!(sh, Shape::TextArea { frame, .. } if frame.body)
}

fn body_base_of(o: &Object) -> Option<BodyBase> {
    match &o.shape {
        Shape::TextArea {
            family,
            size,
            bold,
            italic,
            fmt,
            ..
        } => Some(BodyBase {
            family: family.clone(),
            size: *size,
            bold: *bold,
            italic: *italic,
            fmt: *fmt,
            fill: o.style.fill,
        }),
        _ => None,
    }
}

/// Prüfsumme eines Fließtext-Feldes (Text, Format, Farbe, Lage, Aussparungen).
fn body_frame_hash(o: &Object) -> u64 {
    use std::hash::{Hash, Hasher};
    let sh = &o.shape;
    let mut hs = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}", o.style.fill).hash(&mut hs);
    if let Shape::TextArea {
        xf,
        w,
        h,
        text,
        family,
        size,
        bold,
        italic,
        fmt,
        frame,
        rich,
    } = sh
    {
        text.hash(&mut hs);
        family.hash(&mut hs);
        for v in [xf.a, xf.b, xf.c, xf.d, xf.e, xf.f, *w, *h, *size] {
            v.to_bits().hash(&mut hs);
        }
        (bold, italic, frame.joined).hash(&mut hs);
        format!("{fmt:?}{:?}", frame.holes).hash(&mut hs);
        if !rich.is_plain() {
            format!("{rich:?}").hash(&mut hs);
        }
    }
    hs.finish()
}

/// Größtes `i ≤ at`, das auf einer Zeichengrenze liegt.
fn char_floor(t: &str, at: usize) -> usize {
    let mut i = at.min(t.len());
    while !t.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Ungefähre Lage einer Form (ohne Schriftmetriken: Text als Punkt seiner Grundlinie).
fn rough_box(sh: &Shape) -> Option<Rect> {
    sh.geom_bbox().or_else(|| match sh {
        Shape::Text { xf, .. } => {
            let p = Pt::new(xf.e, xf.f);
            Some(Rect::from_points(p, p))
        }
        Shape::TextPath { path, .. } => Shape::Path {
            subpaths: path.clone(),
            evenodd: false,
        }
        .geom_bbox(),
        _ => None,
    })
}

/// Rahmen einer Tabelle bzw. eines Diagramms – bei einem gedrehten Diagramm der ungedrehte
/// (die Teile werden darin gebaut und dann mit `smart_turn` gedreht).
fn smart_frame(o: &Object) -> Option<Rect> {
    match (&o.shape, smart_turn(o)) {
        (Shape::Rect { x, y, w, h, .. }, None) => Some(Rect::from_xywh(*x, *y, *w, *h)),
        (s, None) => s.geom_bbox(),
        (s, Some(_)) => {
            let mut back = s.clone();
            back.transform(&smart_turn_about(o, true)?);
            back.geom_bbox()
        }
    }
}

/// Drehung eines Diagramms um die Mitte seines Rahmens (`None`: nicht gedreht).
fn smart_turn(o: &Object) -> Option<Affine> {
    smart_turn_about(o, false)
}

/// Drehung (`back`: zurück in die ungedrehte Lage).
fn smart_turn_about(o: &Object, back: bool) -> Option<Affine> {
    if o.angle.abs() < 1e-9 || o.smart.is_none() {
        return None;
    }
    let b = o.shape.geom_bbox()?;
    let c = Pt::new((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
    // der Winkel zählt gegen den Uhrzeigersinn, rotate_about im Uhrzeigersinn
    Some(Affine::rotate_about(if back { o.angle } else { -o.angle }, c))
}

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
/// Zustand vor einer Vorschau (siehe `Editor::preview_begin`).
#[derive(Clone)]
struct PreviewSnap {
    doc: Document,
    selection: Vec<u64>,
    undo_len: usize,
    redo: Vec<Document>,
    dirty: bool,
}

struct BucketCache {
    /// (Änderungsstand, Zahl der Bearbeitungen), Zoom, Verschiebung, Größe der Ansicht
    key: ((u64, u64), f64, f64, f64, (f64, f64)),
    /// betrachtete Objekte (Auswahl; leer = alles Sichtbare)
    ids: Vec<u64>,
    /// nur der sichtbare Bereich wurde betrachtet (dann gilt der Graph nur für diese Ansicht)
    culled: bool,
    arr: Rc<crate::shapeops::Arrangement>,
}

/// Was aus der Bibliothek übertragen wird.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum LibPayload {
    /// Farbe, ggf. mit Verlauf, und Kennung der Bibliotheksfarbe (globale Farben)
    Color(Color, Option<Gradient>, Option<String>),
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
        LibPayload::Color(c, g, link) => {
            o.style.set_paint(PaintTarget::Fill, Some(*c), g.clone());
            o.style.fill_link = link
                .as_ref()
                .filter(|_| g.is_none())
                .map(|id| crate::model::ColorLink::new(id, *c));
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
        /// Feste Zeichenbreite (halbe Schriftgröße), Zeilenhöhe = Schriftgröße
        fn area_fit(&self, o: &Object) -> Option<usize> {
            let Shape::TextArea { w, h, text, size, .. } = &o.shape else {
                return None;
            };
            let per = ((w / (size * 0.5)).floor() as usize).max(1);
            let lines = (h / size).floor() as usize;
            if lines == 0 {
                return (!text.is_empty()).then_some(0);
            }
            let (mut line, mut col) = (0usize, 0usize);
            for (i, ch) in text.char_indices() {
                if ch == '\n' {
                    line += 1;
                    col = 0;
                    if line >= lines {
                        return Some(i);
                    }
                    continue;
                }
                if col == per {
                    line += 1;
                    col = 0;
                    if line >= lines {
                        return Some(i);
                    }
                }
                col += 1;
            }
            None
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
            sub: Vec::new(),
            mask: false,
            locked: false,
            smart: None,
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
    fn rich_text_joins_and_splits() {
        use crate::model::{ListStyle, ParaFmt, RunStyle, TextRun};
        let bold = RunStyle {
            bold: Some(true),
            ..RunStyle::default()
        };
        let list = ParaFmt {
            list: ListStyle::Dot,
            indent: 1,
            ..ParaFmt::default()
        };
        // „ab|cd\nef“ mit fettem „bc“ und einer Aufzählung im zweiten Absatz
        let text = "abcd\nef";
        let rich = RichText {
            runs: vec![TextRun {
                start: 1,
                end: 3,
                style: bold.clone(),
            }],
            paras: vec![ParaFmt::default(), list],
        };
        // mitten im Absatz teilen: der Rest setzt ihn fort
        let (h, t) = RichText::split(text, &rich, 2, false, ListStyle::None);
        assert_eq!(
            h.runs,
            vec![TextRun {
                start: 1,
                end: 2,
                style: bold.clone()
            }]
        );
        assert_eq!(
            t.runs,
            vec![TextRun {
                start: 0,
                end: 1,
                style: bold.clone()
            }]
        );
        assert_eq!(t.paras, vec![ParaFmt::default(), list]);
        let (back, br) = RichText::join("ab", &h, false, "cd\nef", &t, ListStyle::None);
        assert_eq!(back, text);
        assert_eq!(br.paras, rich.paras);
        assert_eq!(
            br.runs.iter().map(|r| (r.start, r.end)).collect::<Vec<_>>(),
            vec![(1, 2), (2, 3)]
        );
        // am Absatzumbruch teilen: er fällt weg, der Rest beginnt mit dem zweiten Absatz
        let (h, t) = RichText::split(text, &rich, 4, true, ListStyle::None);
        assert!(h.paras.is_empty());
        assert_eq!(t.paras, vec![list]);
        let (back, _) = RichText::join("abcd", &h, true, "ef", &t, ListStyle::None);
        assert_eq!(back, text);
        // ohne Format bleibt es ohne
        let (j, r) = RichText::join(
            "a",
            &RichText::default(),
            true,
            "b",
            &RichText::default(),
            ListStyle::None,
        );
        assert_eq!(j, "a\nb");
        assert!(r.is_plain());
    }

    #[test]
    fn body_text_flows_from_page_to_page() {
        let mut e = editor();
        e.viewport = (800.0, 600.0);
        assert!(e.set_view_mode(ViewMode::Notebook));
        // Textwerkzeug: ein Klick auf die Seite beginnt den Fließtext (kein Textfeld)
        e.set_tool(Tool::Text);
        drag(&mut e, (200.0, 200.0), (200.0, 200.0));
        let frames = e.body_frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(e.text_edit.map(|t| t.id), Some(frames[0]));
        // Seitenbereich: die Seite ohne ihren Rand
        let b = M.bbox(e.doc.object(frames[0]).unwrap()).unwrap();
        let m = crate::model::DEFAULT_MARGIN;
        assert!((b.x0 - m).abs() < 1e-6 && (b.width() - (500.0 - 2.0 * m)).abs() < 1e-6);
        // 43 Zeichen je Zeile, 21 Zeilen: gut 900 Zeichen je Seite
        let words = "abcd efgh ".repeat(200);
        assert!(e.edit_insert(&words));
        assert!(e.reflow_body(&M));
        assert!(!e.reflow_body(&M));
        let frames = e.body_frames();
        assert_eq!(frames.len(), 3);
        assert_eq!(e.doc.artboard_rects().len(), 3);
        assert!(e.doc.artboards.iter().all(|a| a.auto));
        // der Text bleibt derselbe, nur verteilt; der Cursor steht am Ende auf der letzten Seite
        let texts: Vec<String> = frames
            .iter()
            .map(|id| {
                e.doc
                    .object(*id)
                    .unwrap()
                    .shape
                    .text_fields()
                    .unwrap()
                    .0
                    .to_string()
            })
            .collect();
        assert_eq!(texts.concat(), words);
        assert_eq!(texts[0].len(), 43 * 21);
        let te = e.text_edit.unwrap();
        assert_eq!(te.id, frames[2]);
        assert_eq!(te.caret, texts[2].len());
        // jedes Feld liegt auf seiner Seite
        let rects = e.doc.artboard_rects();
        for (k, id) in frames.iter().enumerate() {
            let b = M.bbox(e.doc.object(*id).unwrap()).unwrap();
            assert!(rects[k].1.contains(Pt::new(b.x0 + 1.0, b.y0 + 1.0)));
        }
        // Rücktaste am Anfang einer Seite löscht das letzte Zeichen der vorigen
        e.text_edit = Some(TextEdit {
            id: frames[1],
            caret: 0,
            anchor: 0,
        });
        assert!(e.edit_backspace());
        e.reflow_body(&M);
        let all: String = e
            .body_frames()
            .iter()
            .map(|id| {
                e.doc
                    .object(*id)
                    .unwrap()
                    .shape
                    .text_fields()
                    .unwrap()
                    .0
                    .to_string()
            })
            .collect();
        assert_eq!(all.len(), words.len() - 1);
        // Pfeil nach rechts am Ende einer Seite: weiter auf der nächsten
        let frames = e.body_frames();
        let len0 = e
            .doc
            .object(frames[0])
            .unwrap()
            .shape
            .text_fields()
            .unwrap()
            .0
            .len();
        e.text_edit = Some(TextEdit {
            id: frames[0],
            caret: len0,
            anchor: len0,
        });
        assert!(e.edit_move(Move::Right, false));
        assert_eq!(e.text_edit.map(|t| (t.id, t.caret)), Some((frames[1], 0)));
        // fast alles löschen: die neuen Seiten fallen wieder weg
        let n2 = e
            .doc
            .object(frames[2])
            .unwrap()
            .shape
            .text_fields()
            .unwrap()
            .0
            .len();
        for (id, len) in [(frames[2], n2), (frames[1], 43 * 21)] {
            e.text_edit = Some(TextEdit {
                id,
                caret: len,
                anchor: 0,
            });
            assert!(e.edit_backspace());
            e.reflow_body(&M);
        }
        assert_eq!(e.body_frames().len(), 1);
        assert_eq!(e.doc.artboard_rects().len(), 1);
        // Absätze: ein Umbruch an der Seitengrenze wird nicht als Leerzeile übertragen
        let id = e.body_frames()[0];
        let len = e.doc.object(id).unwrap().shape.text_fields().unwrap().0.len();
        e.text_edit = Some(TextEdit {
            id,
            caret: len,
            anchor: 0,
        });
        let paras = "x\n".repeat(25);
        assert!(e.edit_insert(paras.trim_end()));
        e.reflow_body(&M);
        let frames = e.body_frames();
        assert_eq!(frames.len(), 2);
        let second = e.doc.object(frames[1]).unwrap();
        let Shape::TextArea { text, frame, .. } = &second.shape else {
            panic!()
        };
        assert!(!frame.joined && !text.starts_with('\n'));
        assert_eq!(text.matches('x').count(), 4);
        // auf der letzten Seite weiterschreiben: auch dort läuft der Text auf eine neue Seite
        let last = *frames.last().unwrap();
        let len = e.doc.object(last).unwrap().shape.text_fields().unwrap().0.len();
        e.text_edit = Some(TextEdit {
            id: last,
            caret: len,
            anchor: len,
        });
        e.selection = vec![last];
        for _ in 0..30 {
            assert!(e.edit_insert("word word word word word word word word "));
            e.reflow_body(&M);
        }
        let frames = e.body_frames();
        assert_eq!(frames.len(), 3);
        assert_eq!(e.doc.artboard_rects().len(), 3);
        assert_eq!(e.text_edit.map(|t| t.id), Some(frames[2]));
        for id in &frames {
            assert_eq!(M.area_fit(e.doc.object(*id).unwrap()), None);
        }
        // wieder löschen: die neue Seite fällt weg
        let id = frames[2];
        let len = e.doc.object(id).unwrap().shape.text_fields().unwrap().0.len();
        e.text_edit = Some(TextEdit {
            id,
            caret: len,
            anchor: 0,
        });
        assert!(e.edit_backspace());
        e.reflow_body(&M);
        let id = e.body_frames()[1];
        let t = e
            .doc
            .object(id)
            .unwrap()
            .shape
            .text_fields()
            .unwrap()
            .0
            .to_string();
        let cut = t.find("word").unwrap_or(t.len());
        e.text_edit = Some(TextEdit {
            id,
            caret: t.len(),
            anchor: cut,
        });
        assert!(e.edit_backspace());
        e.reflow_body(&M);
        let frames = e.body_frames();
        assert_eq!(frames.len(), 2);
        assert_eq!(e.doc.artboard_rects().len(), 2);
        // gespeichert und wieder geöffnet: dieselben Felder und Seiten
        let back = crate::svg::read(&crate::svg::write(&e.doc, false)).unwrap();
        let mut e2 = Editor::new(back);
        assert_eq!(e2.body_frames().len(), 2);
        assert!(!e2.reflow_body(&M) || e2.body_frames().len() == 2);
        assert_eq!(e2.doc.artboard_rects().len(), 2);
        // auf der Zeichenfläche lässt sich der Fließtext nicht anfassen
        e.set_view_mode(ViewMode::Artboards);
        let b = M.bbox(e.doc.object(frames[0]).unwrap()).unwrap();
        assert_eq!(e.hit(Pt::new(b.x0 + 5.0, b.y0 + 5.0), &M), None);
    }

    #[test]
    fn text_colour_applies_to_the_selected_text() {
        let mut e = editor();
        e.set_tool(Tool::TextArea);
        drag(&mut e, (10.0, 10.0), (300.0, 100.0));
        assert!(e.edit_insert("Hallo Welt"));
        let id = e.text_edit.unwrap().id;
        e.text_edit = Some(TextEdit {
            id,
            caret: 5,
            anchor: 0,
        });
        let red = Color::rgb(1.0, 0.0, 0.0);
        assert!(e.set_text_color(Some(red)));
        let o = e.doc.object(id).unwrap();
        let fill = o.style.fill;
        let r = o.shape.rich().unwrap();
        assert_eq!(r.runs.len(), 1);
        assert_eq!(
            (r.runs[0].start, r.runs[0].end, r.runs[0].style.color),
            (0, 5, Some(red))
        );
        // die Seitenleiste zeigt die Farbe an der Markierung
        assert_eq!(e.context_info().text_color, Some(red));
        // gespeichert und wieder geöffnet
        let back = crate::svg::read(&crate::svg::write(&e.doc, false)).unwrap();
        let o2 = back
            .layers
            .iter()
            .flat_map(|l| &l.objects)
            .find(|o| o.shape.is_text())
            .unwrap();
        assert_eq!(
            o2.shape.rich().unwrap().runs[0].style.color.map(|c| c.to_hex()),
            Some(red.to_hex())
        );
        // ohne Bearbeiten: der ganze Text, eigene Farben entfallen
        e.end_text_edit();
        e.selection = vec![id];
        let blue = Color::rgb(0.0, 0.0, 1.0);
        assert!(e.set_text_color(Some(blue)));
        let o = e.doc.object(id).unwrap();
        assert_ne!(o.style.fill, fill);
        assert!(o
            .shape
            .rich()
            .unwrap()
            .runs
            .iter()
            .all(|t| t.style.color.is_none()));
    }

    #[test]
    fn page_breaks_and_columns_in_body_text() {
        let mut e = editor();
        e.viewport = (800.0, 600.0);
        e.ruler = 20.0;
        e.set_view_mode(ViewMode::Notebook);
        e.view = View {
            zoom: 1.0,
            ox: 100.0,
            oy: 50.0,
        };
        let at = e.view.to_screen(Pt::new(200.0, 200.0));
        drag(&mut e, (at.x, at.y), (at.x, at.y));
        assert!(e.edit_insert("Erste Seite"));
        // Seitenumbruch (nur im Fließtext): der Rest beginnt auf einer neuen Seite
        assert!(e.insert_page_break());
        assert!(e.edit_insert("Zweite"));
        e.reflow_body(&M);
        let frames = e.body_frames();
        assert_eq!(frames.len(), 2);
        let text = |e: &Editor, id: u64| {
            e.doc
                .object(id)
                .unwrap()
                .shape
                .text_fields()
                .unwrap()
                .0
                .to_string()
        };
        assert_eq!(
            text(&e, frames[0]),
            format!("Erste Seite{}", crate::model::PAGE_BREAK)
        );
        // gültiges XML (U+000C ist darin verboten) und zurück
        let svg = crate::svg::write(&e.doc, false);
        assert!(!svg.contains('\u{c}'));
        let back = crate::svg::read(&svg).unwrap();
        assert!(back.layers.iter().flat_map(|l| &l.objects).any(|o| o
            .shape
            .text_fields()
            .is_some_and(|t| t.0.ends_with(crate::model::PAGE_BREAK))));
        assert_eq!(text(&e, frames[1]), "Zweite");
        assert_eq!(e.text_edit.map(|t| (t.id, t.caret)), Some((frames[1], 6)));
        // Rücktaste am Anfang der zweiten Seite: der Umbruch ist wieder weg
        e.text_edit = Some(TextEdit {
            id: frames[1],
            caret: 0,
            anchor: 0,
        });
        assert!(e.edit_backspace());
        e.reflow_body(&M);
        assert!(e.edit_backspace());
        e.reflow_body(&M);
        assert_eq!(e.body_frames().len(), 1);
        assert_eq!(text(&e, e.body_frames()[0]), "Erste SeiteZweite");
        // zwei Spalten: Zwischenraum am Lineal verschieben und breiter ziehen
        let id = e.body_frames()[0];
        e.selection = vec![id];
        if let Some(Shape::TextArea { fmt, .. }) = e.doc.object_mut(id).map(|o| &mut o.shape) {
            fmt.columns = 2;
        }
        let cols = e.body_columns();
        assert_eq!(cols.len(), 2);
        let w0 = cols[0].1 - cols[0].0;
        let mid = e.view.to_screen(Pt::new((cols[0].1 + cols[1].0) / 2.0, 0.0));
        assert_eq!(e.ruler_cursor(Pt::new(mid.x, 10.0)), "col-resize");
        drag(&mut e, (mid.x, 10.0), (mid.x + 30.0, 10.0));
        let cols = e.body_columns();
        assert!((cols[0].1 - cols[0].0 - (w0 + 30.0)).abs() < 1.0, "{cols:?}");
        let edge = e.view.to_screen(Pt::new(cols[0].1, 0.0));
        let gap0 = cols[1].0 - cols[0].1;
        drag(&mut e, (edge.x, 10.0), (edge.x - 10.0, 10.0));
        let cols = e.body_columns();
        assert!(cols[1].0 - cols[0].1 > gap0 + 10.0);
        // Einzug der ersten Zeile: Dreieck oben links, darunter bleibt der Seitenrand
        let (_, fi, _) = e.margin_handles().into_iter().find(|h| h.0 == 4).unwrap();
        let m0 = e.doc.margins;
        assert_eq!(e.margin_handle_at(Pt::new(fi.x, 15.0)).map(|h| h.0), Some(3));
        assert_eq!(e.ruler_cursor(Pt::new(fi.x, 4.0)), "col-resize");
        drag(&mut e, (fi.x, 4.0), (fi.x + 20.0, 4.0));
        assert_eq!(e.doc.margins, m0);
        let first = match &e.doc.object(id).unwrap().shape {
            Shape::TextArea { fmt, .. } => fmt.first,
            _ => unreachable!(),
        };
        assert!((first - 20.0).abs() < 1.0, "{first}");
        // gespeichert und wieder geöffnet
        let back = crate::svg::read(&crate::svg::write(&e.doc, false)).unwrap();
        let f = back
            .layers
            .iter()
            .flat_map(|l| &l.objects)
            .find_map(|o| match &o.shape {
                Shape::TextArea { fmt, .. } => Some(*fmt),
                _ => None,
            })
            .unwrap();
        let mine = match &e.doc.object(id).unwrap().shape {
            Shape::TextArea { fmt, .. } => *fmt,
            _ => unreachable!(),
        };
        assert!((f.col_gap - mine.col_gap).abs() < 1e-3);
        assert!((f.col_widths[0] - mine.col_widths[0]).abs() < 1e-3);
        assert!((f.first - mine.first).abs() < 1e-3);
    }

    #[test]
    fn headers_and_footers_with_fields() {
        use crate::model::{band_display, BAND_FOOTER, BAND_HEADER, FIELD_PAGE, FIELD_PAGES};
        let mut e = editor();
        e.viewport = (800.0, 600.0);
        e.set_view_mode(ViewMode::Notebook);
        e.view = View {
            zoom: 1.0,
            ox: 100.0,
            oy: 50.0,
        };
        e.date_text = "7 November 2026".into();
        let at = e.view.to_screen(Pt::new(200.0, 200.0));
        drag(&mut e, (at.x, at.y), (at.x, at.y));
        assert!(e.edit_insert("Eins"));
        assert!(e.insert_page_break());
        assert!(e.edit_insert("Zwei"));
        e.reflow_body(&M);
        assert_eq!(e.doc.artboard_rects().len(), 2);
        // Doppelklick in den oberen Rand: Kopfzeile auf allen Seiten
        let r = e.doc.artboard_rects()[0].1;
        let top = e
            .view
            .to_screen(Pt::new(r.x0 + 100.0, r.y0 + e.doc.margins[0] / 2.0));
        e.double_click(top, &M);
        assert_eq!(e.editing_band(), Some(BAND_HEADER));
        assert!(e.context_info().body);
        assert!(e.edit_insert("Seite "));
        assert!(e.insert_field(FIELD_PAGE));
        assert!(e.edit_insert(" von "));
        assert!(e.insert_field(FIELD_PAGES));
        e.reflow_body(&M);
        let frames = e.band_frames(BAND_HEADER);
        assert_eq!(frames.iter().map(|f| f.0).collect::<Vec<_>>(), vec![0, 1]);
        // im Fließtext dagegen keine Felder
        e.end_text_edit();
        e.reflow_body(&M);
        assert!(!e.insert_field(FIELD_PAGE));
        let shown = |e: &Editor, page: usize| {
            let id = e.band_frames(BAND_HEADER)[page].1;
            band_display(&e.doc.object(id).unwrap().shape)
                .and_then(|s| s.text_fields().map(|t| t.0.to_string()))
                .unwrap_or_default()
        };
        assert_eq!(shown(&e, 0), "Seite 1 von 2");
        assert_eq!(shown(&e, 1), "Seite 2 von 2");
        // gespeichert und wieder geöffnet: die Felder bleiben Felder
        e.doc.date = "2026-11-07".into();
        let back = crate::svg::read(&crate::svg::write(&e.doc, false)).unwrap();
        assert_eq!(back.date, "2026-11-07");
        let heads: Vec<String> = back
            .layers
            .iter()
            .flat_map(|l| &l.objects)
            .filter(|o| is_band(&o.shape))
            .filter_map(|o| o.shape.text_fields().map(|t| t.0.to_string()))
            .collect();
        assert_eq!(heads.len(), 2);
        assert!(heads[0].contains(FIELD_PAGE));
        // Titelseite ohne Kopfzeile
        assert!(e.set_bands_skip_first(true));
        assert_eq!(shown(&e, 1), "Seite 2 von 2");
        let first = e.band_frames(BAND_HEADER)[0].1;
        assert!(
            matches!(&e.doc.object(first).unwrap().shape, Shape::TextArea { frame, .. } if frame.page_no == 0)
        );
        e.double_click(top, &M);
        assert_eq!(e.editing_band(), None);
        assert!(e.set_bands_skip_first(false));
        // Fußzeile ohne Text verschwindet wieder
        let bottom = e
            .view
            .to_screen(Pt::new(r.x0 + 100.0, r.y1 - e.doc.margins[2] / 2.0));
        e.double_click(bottom, &M);
        assert_eq!(e.editing_band(), Some(BAND_FOOTER));
        e.reflow_body(&M);
        assert!(e.has_band(BAND_FOOTER));
        e.end_text_edit();
        e.reflow_body(&M);
        assert!(!e.has_band(BAND_FOOTER));
        // ausschalten: weg auf allen Seiten, Rückgängig holt sie zurück
        assert!(e.set_band(BAND_HEADER, false, &M));
        assert!(!e.has_band(BAND_HEADER));
        assert!(e.undo());
        e.reflow_body(&M);
        assert_eq!(e.band_frames(BAND_HEADER).len(), 2);
        // der Fließtext bleibt davon unberührt
        assert_eq!(e.body_frames().len(), 2);
    }

    #[test]
    fn opening_an_odt_places_pages_tables_and_header() {
        let imp = crate::odt::import(&crate::odt::tests::sample_odt(), &|_| true).unwrap();
        let mut e = Editor::new(imp.doc);
        e.viewport = (800.0, 600.0);
        assert_eq!(e.doc.view, ViewMode::Notebook);
        e.place_floats(imp.floats, &M);
        // Seitenumbruch: zwei Seiten, Kopfzeile auf beiden
        assert_eq!(e.doc.artboard_rects().len(), 2);
        assert_eq!(e.band_frames(crate::model::BAND_HEADER).len(), 2);
        // die Tabelle steht auf der ersten Seite, so breit wie der Text, und drängt ihn weg
        let t = e
            .doc
            .layers
            .iter()
            .flat_map(|l| &l.objects)
            .find(|o| o.smart.is_some())
            .unwrap();
        let b = smart_frame(t).unwrap();
        let r0 = e.doc.artboard_rects()[0].1;
        assert!(r0.contains(Pt::new(b.x0 + 1.0, b.y0 + 1.0)), "{b:?}");
        assert!((b.x0 - (r0.x0 + e.doc.margins[3])).abs() < 1.0);
        assert!(pushes_text(t));
        // gespeichert und wieder geöffnet bleibt es ein Dokument mit Fließtext
        let back = crate::svg::read(&crate::svg::write(&e.doc, false)).unwrap();
        let mut e2 = Editor::new(back);
        e2.reflow_body(&M);
        assert_eq!(e2.body_frames().len(), 2);
    }

    #[test]
    fn paragraphs_have_their_own_format() {
        let mut e = editor();
        let id = e.insert_object(
            0,
            Shape::TextArea {
                xf: Affine::translate(0.0, 0.0),
                w: 400.0,
                h: 400.0,
                text: "Titel\nErster Absatz\nZweiter Absatz".into(),
                family: "Sans".into(),
                size: 12.0,
                bold: false,
                italic: false,
                fmt: TextFormat::default(),
                frame: TextFrame::default(),
                rich: Default::default(),
            },
            Style::default(),
        );
        let paras = |e: &Editor| match &e.doc.object(id).unwrap().shape {
            Shape::TextArea { rich, fmt, text, .. } => {
                let n = text.matches('\n').count() + 1;
                ((0..n).map(|i| rich.para(i, fmt.list)).collect::<Vec<_>>(), *fmt)
            }
            _ => unreachable!(),
        };
        // beim Schreiben: nur der Absatz am Cursor
        e.selection = vec![id];
        e.begin_text_edit(id, Some(2));
        assert!(e.set_text_align(Align::Center));
        assert!(e.set_paragraph(
            "before",
            |p| p.before = Some(1.0),
            |t| t.fmt.before = 1.0,
            |p| p.before = None
        ));
        let (p, f) = paras(&e);
        assert_eq!(p[0].align, Some(Align::Center));
        assert_eq!(p[1].align, None);
        assert_eq!(p[0].space_before(&f), 1.0);
        assert_eq!(p[1].space_before(&f), 0.0);
        assert_eq!(e.context_info().font.fmt.align, Align::Center);
        e.text_edit = Some(TextEdit {
            id,
            caret: 8,
            anchor: 8,
        });
        assert_eq!(e.context_info().font.fmt.align, Align::Left);
        // Enter im zentrierten Absatz: beide Teile bleiben zentriert
        e.text_edit = Some(TextEdit {
            id,
            caret: 3,
            anchor: 3,
        });
        assert!(e.edit_insert("\n"));
        assert_eq!(paras(&e).0[0].align, Some(Align::Center));
        assert_eq!(paras(&e).0[1].align, Some(Align::Center));
        // Bibliotheksstil am Cursor: Schrift des ganzen Absatzes und sein Absatzformat
        let style = crate::library::LibStyle {
            style: Style::default(),
            text: Some(crate::library::TextStyle {
                family: "Serif".into(),
                size: 30.0,
                bold: true,
                italic: false,
                fmt: TextFormat {
                    align: Align::Right,
                    para: 2.0,
                    ..TextFormat::default()
                },
            }),
            name: String::new(),
        };
        e.text_edit = Some(TextEdit {
            id,
            caret: 9,
            anchor: 9,
        });
        assert!(e.apply_lib_style(&style, &M));
        let (p, f) = paras(&e);
        assert_eq!(p[2].align, Some(Align::Right));
        assert_eq!(p[2].space_after(&f), 2.0);
        assert_eq!(p[3].align, None);
        let o = e.doc.object(id).unwrap();
        let (cf, mixed, _) = e.text_view(&[o]).unwrap();
        assert_eq!(cf.size, 30.0);
        assert!(!mixed.size);
        // ohne Bearbeitung: für den ganzen Text, eigene Angaben fallen weg
        e.end_text_edit();
        e.selection = vec![id];
        assert!(e.set_text_align(Align::Justify));
        let (p, f) = paras(&e);
        assert_eq!(f.align, Align::Justify);
        assert!(p.iter().all(|p| p.align.is_none()));
        // gespeichert und wieder geöffnet
        e.begin_text_edit(id, Some(0));
        e.set_paragraph(
            "line",
            |p| p.line = Some(0.0),
            |t| t.fmt.line_auto = true,
            |p| p.line = None,
        );
        let back = crate::svg::read(&crate::svg::write(&e.doc, false)).unwrap();
        let rb = back
            .layers
            .iter()
            .flat_map(|l| &l.objects)
            .find_map(|o| match &o.shape {
                Shape::TextArea { rich, .. } => Some(rich.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(rb.para(0, Default::default()).line, Some(0.0));
        assert_eq!(rb.para(2, Default::default()).after, Some(2.0));
    }

    #[test]
    fn grouped_objects_push_text_as_one_rectangle() {
        let mut e = editor();
        let text_area = |e: &Editor, id: u64| match e.doc.object(id).map(|o| &o.shape) {
            Some(Shape::TextArea { frame, .. }) => frame.clone(),
            _ => panic!(),
        };
        let li = 0;
        let id = e.insert_object(
            li,
            Shape::TextArea {
                xf: Affine::translate(0.0, 0.0),
                w: 400.0,
                h: 400.0,
                text: "Text".into(),
                family: "Sans".into(),
                size: 12.0,
                bold: false,
                italic: false,
                fmt: TextFormat::default(),
                frame: TextFrame::default(),
                rich: Default::default(),
            },
            Style::default(),
        );
        let a = e.insert_object(
            li,
            Shape::Rect {
                x: 100.0,
                y: 100.0,
                w: 40.0,
                h: 40.0,
                r: 0.0,
            },
            Style::default(),
        );
        let b = e.insert_object(
            li,
            Shape::Rect {
                x: 200.0,
                y: 180.0,
                w: 40.0,
                h: 40.0,
                r: 0.0,
            },
            Style::default(),
        );
        e.selection = vec![a, b];
        e.group_selection();
        assert_ne!(e.doc.object(a).unwrap().group, 0);
        e.selection = vec![a, b];
        assert!(e.set_wrap(true));
        e.update_flow();
        assert_eq!(text_area(&e, id).holes.len(), 2);
        assert!(e.set_wrap_box(true));
        assert!(e.set_wrap_gap(0.0));
        e.update_flow();
        let f = text_area(&e, id);
        assert_eq!(f.holes.len(), 1);
        let hb = Shape::Path {
            subpaths: vec![SubPath {
                nodes: f.holes[0].iter().map(|p| Node::corner(*p)).collect(),
                closed: true,
            }],
            evenodd: false,
        }
        .geom_bbox()
        .unwrap();
        assert!(
            (hb.x0 - 100.0).abs() < 0.5 && (hb.x1 - 240.0).abs() < 0.5,
            "{hb:?}"
        );
        assert!(
            (hb.y0 - 100.0).abs() < 0.5 && (hb.y1 - 220.0).abs() < 0.5,
            "{hb:?}"
        );
        assert_eq!(e.wrap_preview().len(), 1);
    }

    #[test]
    fn page_margins_are_dragged_on_the_ruler() {
        let mut e = editor();
        e.viewport = (800.0, 600.0);
        e.ruler = 20.0;
        e.set_view_mode(ViewMode::Notebook);
        e.view = View {
            zoom: 1.0,
            ox: 100.0,
            oy: 50.0,
        };
        let m0 = e.doc.margins;
        let hs = e.margin_handles();
        assert_eq!(hs.len(), 4);
        // linker Rand: Anfasser im oberen Lineal über dem Textanfang
        let (_, left, _) = hs.iter().find(|h| h.0 == 3).copied().unwrap();
        assert!((left.x - (100.0 + m0[3])).abs() < 1e-6);
        assert_eq!(e.ruler_cursor(left), "col-resize");
        drag(&mut e, (left.x, left.y), (left.x + 40.0, left.y));
        assert!((e.doc.margins[3] - (m0[3] + 40.0)).abs() < 1.0);
        assert_eq!(e.doc.margins[1], m0[1]);
        // ein Schritt zum Rückgängigmachen
        assert!(e.undo());
        assert_eq!(e.doc.margins, m0);
        // unterer Rand im linken Lineal; zu weit gezogen bleibt Platz für Text
        let (_, bottom, _) = e.margin_handles().into_iter().find(|h| h.0 == 2).unwrap();
        drag(&mut e, (bottom.x, bottom.y), (bottom.x, 0.0));
        assert!(e.doc.margins[2] <= 500.0 - m0[0] - 20.0 + 1e-6);
        // daneben im Lineal: keine Hilfslinie im Notizheft
        let guides = e.doc.guides.len();
        drag(&mut e, (300.0, 5.0), (300.0, 200.0));
        assert_eq!(e.doc.guides.len(), guides);
        // auf der Zeichenfläche keine Anfasser
        e.set_view_mode(ViewMode::Artboards);
        assert!(e.margin_handles().is_empty());
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
    fn pen_click_stays_a_corner_when_zoomed_in_with_grid() {
        // 16-px-Symbol bei starker Vergrößerung mit Raster: der Punkt rastet ein ganzes Stück
        // neben dem Zeiger ein; ein kleines Zittern beim Klicken darf keine Kurve ergeben
        let mut e = editor();
        e.set_tool(Tool::Pen);
        e.grid_snap = true;
        e.grid_size = 1.0;
        e.view.zoom = 40.0;
        for (x, y) in [(3.4, 3.4), (12.4, 3.4), (12.4, 12.4)] {
            let s = e.view.to_screen(Pt::new(x, y));
            e.press(s, Mods::default(), false, &M);
            e.motion(s + Pt::new(2.0, 1.0), Mods::default(), &M);
            e.release(s + Pt::new(2.0, 1.0), Mods::default(), &M);
        }
        let nodes = e.pen.clone().unwrap();
        assert_eq!(nodes.len(), 3);
        for n in &nodes {
            assert!(n.h_in == n.p && n.h_out == n.p, "{n:?}");
        }
        // ein deutlicher Zug ergibt weiterhin eine Kurve
        let s = e.view.to_screen(Pt::new(3.4, 12.4));
        e.press(s, Mods::default(), false, &M);
        e.motion(s + Pt::new(30.0, 0.0), Mods::default(), &M);
        e.release(s + Pt::new(30.0, 0.0), Mods::default(), &M);
        let n = *e.pen.as_ref().unwrap().last().unwrap();
        assert!(n.h_out != n.p);
    }

    #[test]
    fn selection_handles_are_easy_to_grab() {
        let mut e = editor();
        e.set_tool(Tool::Select);
        let big = rect_at(&mut e, 100.0, 100.0, 200.0, 100.0);
        e.selection = vec![big];
        // etwas neben der Ecke trifft noch (größer als das Sichtbare)
        assert_eq!(e.handle_at(Pt::new(92.0, 93.0), &M), Some(0));
        assert_eq!(e.handle_at(Pt::new(200.0, 109.0), &M), Some(1));
        assert_eq!(e.handle_at(Pt::new(150.0, 150.0), &M), None);
        // kleine Auswahl: nur die Ecken, und die Mitte bleibt zum Verschieben frei
        let small = rect_at(&mut e, 400.0, 100.0, 30.0, 30.0);
        e.selection = vec![small];
        assert_eq!(e.handle_at(Pt::new(415.0, 100.0), &M), None);
        assert_eq!(e.handle_at(Pt::new(415.0, 115.0), &M), None);
        assert_eq!(e.handle_at(Pt::new(398.0, 98.0), &M), Some(0));
        // beim Verschieben sind die Anfasser ausgeblendet – erst wenn sich etwas bewegt
        e.press(Pt::new(415.0, 115.0), Mods::default(), false, &M);
        assert!(!e.transforming());
        e.motion(Pt::new(430.0, 125.0), Mods::default(), &M);
        assert!(e.transforming());
        e.release(Pt::new(430.0, 125.0), Mods::default(), &M);
        assert!(!e.transforming());
    }

    #[test]
    fn new_shapes_do_not_inherit_effects() {
        let mut e = editor();
        let a = rect_at(&mut e, 0.0, 0.0, 50.0, 50.0);
        e.selection = vec![a];
        e.update_style("fill", |s| s.fill = Some(Color::rgb(1.0, 0.0, 0.0)));
        e.update_style("lens", |s| s.fx.lens = Some((crate::model::Lens::Drop, 0.5)));
        e.update_style("shadow", |s| s.shadow = Some(crate::model::Shadow::default()));
        // die Farbe gilt für neue Formen, der 3D-Effekt und der Schatten nicht
        assert_eq!(e.defaults.fill, Some(Color::rgb(1.0, 0.0, 0.0)));
        assert!(e.defaults.fx.lens.is_none() && e.defaults.shadow.is_none());
        e.deselect();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (150.0, 150.0));
        let o = e.doc.layers[0].objects.last().unwrap();
        assert!(o.style.fx.lens.is_none() && o.style.shadow.is_none());
        assert_eq!(o.style.fill, Some(Color::rgb(1.0, 0.0, 0.0)));
    }

    #[test]
    fn previews_can_be_applied_or_cancelled() {
        let mut e = editor();
        let a = rect_at(&mut e, 0.0, 0.0, 10.0, 10.0);
        e.selection = vec![a];
        let before = e.doc.clone();
        let undo0 = e.undo.len();
        // mehrmals ändern, dann abbrechen: alles wie vorher
        e.preview_begin();
        for n in [2, 5, 3] {
            assert!(e.preview_apply(|e| e.repeat_selection(n, 20.0, 0.0, 0.0, 1.0, None, &M)));
        }
        assert_eq!(e.doc.layers[0].objects.len(), 4);
        e.preview_end(false);
        assert_eq!(e.doc, before);
        assert_eq!(e.undo.len(), undo0);
        assert_eq!(e.selection, vec![a]);
        // anwenden: genau ein Rückgängig-Schritt
        e.preview_begin();
        e.preview_apply(|e| e.scale_selection(2.0, 2.0, true, &M));
        e.preview_apply(|e| e.scale_selection(3.0, 3.0, true, &M));
        e.preview_end(true);
        assert_eq!(e.undo.len(), undo0 + 1);
        let b = e.selection_bbox(&M).unwrap();
        assert!((b.width() - 30.0).abs() < 1e-6, "{b:?}");
        assert!(e.undo());
        assert_eq!(e.doc, before);
        // eine andere Änderung während der Vorschau wendet sie an
        e.preview_begin();
        e.preview_apply(|e| e.scale_selection(2.0, 2.0, true, &M));
        e.nudge(5.0, 0.0);
        assert!(!e.preview_active());
        assert_eq!(e.undo.len(), undo0 + 2);
        // Rückgängig während der Vorschau nimmt nur die Vorschau zurück
        let mid = e.doc.clone();
        e.preview_begin();
        e.preview_apply(|e| e.scale_selection(2.0, 2.0, true, &M));
        assert!(e.undo());
        assert_eq!(e.doc, mid);
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
    fn double_click_types_into_a_cell_in_every_order() {
        let mut e = editor();
        e.set_tool(Tool::Data);
        drag(&mut e, (100.0, 100.0), (400.0, 220.0));
        let a = e.cell_edit.as_ref().unwrap().anchor;
        e.end_text_edit();
        e.set_tool(Tool::Select);
        let f = e.doc.object(a).and_then(smart_frame).unwrap();
        let s = e
            .view
            .to_screen(Pt::new(f.x0 + f.width() * 0.5, f.y0 + f.height() * 0.5));
        for (selected, dbl_first) in [(false, false), (false, true), (true, false), (true, true)] {
            e.end_text_edit();
            e.deselect();
            e.pick = None;
            if selected {
                e.selection = e.doc.group_members(a);
            }
            e.press(s, Mods::default(), false, &M);
            e.release(s, Mods::default(), &M);
            if dbl_first {
                e.double_click(s, &M);
                e.press(s, Mods::default(), false, &M);
            } else {
                e.press(s, Mods::default(), false, &M);
                e.double_click(s, &M);
            }
            e.release(s, Mods::default(), &M);
            assert_eq!(
                e.cell_edit.as_ref().map(|c| (c.row, c.col)),
                Some((1, 1)),
                "{selected} {dbl_first}"
            );
            assert!(e.text_edit.is_some());
        }
        // auch dicht am Rand der Zelle (dort ließe sich eine gewählte Zelle verschieben)
        e.end_text_edit();
        e.selection = e.doc.group_members(a);
        e.pick = None;
        let (_, ys) = match e.smart_data(a) {
            Some(crate::smart::Smart::Table(t)) => t.grid(f),
            _ => panic!(),
        };
        let s = e
            .view
            .to_screen(Pt::new(f.x0 + f.width() * 0.5, ys[2] - 1.5 / e.view.zoom));
        for _ in 0..2 {
            e.press(s, Mods::default(), false, &M);
            e.release(s, Mods::default(), &M);
        }
        assert_eq!(e.cell_edit.as_ref().map(|c| (c.row, c.col)), Some((1, 1)));
    }

    #[test]
    fn clicking_cells_while_writing_a_formula_inserts_references() {
        use crate::smart::Smart;
        let mut e = editor();
        e.set_tool(Tool::Data);
        drag(&mut e, (100.0, 100.0), (400.0, 220.0));
        let a = e.cell_edit.as_ref().unwrap().anchor;
        e.end_text_edit();
        e.set_tool(Tool::Select);
        let f = e.doc.object(a).and_then(smart_frame).unwrap();
        let at = |e: &Editor, r: usize, c: usize| {
            let (xs, ys) = match e.smart_data(a) {
                Some(Smart::Table(t)) => t.grid(f),
                _ => panic!(),
            };
            e.view
                .to_screen(Pt::new((xs[c] + xs[c + 1]) / 2.0, (ys[r] + ys[r + 1]) / 2.0))
        };
        assert!(e.begin_cell_edit(a, 2, 2));
        assert!(e.edit_insert("=SUM("));
        // Klick: Bezug; noch ein Klick ersetzt ihn; Ziehen macht einen Bereich daraus
        let (p, q) = (at(&e, 0, 0), at(&e, 1, 0));
        e.press(p, Mods::default(), false, &M);
        e.release(p, Mods::default(), &M);
        let text = |e: &Editor| e.cell_input().unwrap().3;
        assert_eq!(text(&e), "=SUM(A1");
        e.press(q, Mods::default(), false, &M);
        e.release(q, Mods::default(), &M);
        assert_eq!(text(&e), "=SUM(A2");
        let (p, q) = (at(&e, 0, 0), at(&e, 1, 1));
        e.press(p, Mods::default(), false, &M);
        e.motion(q, Mods::default(), &M);
        e.release(q, Mods::default(), &M);
        assert_eq!(text(&e), "=SUM(A1:B2");
        assert_eq!(e.cell_edit.as_ref().map(|c| (c.row, c.col)), Some((2, 2)));
        // eingefärbt: ein Bereich
        let marks = e.formula_marks();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].2, (0, 0, 1, 1));
        // nach einer Zahl ist kein Bezug möglich: Klick beendet das Tippen
        assert!(e.edit_insert(")*2"));
        let p = at(&e, 0, 1);
        e.press(p, Mods::default(), false, &M);
        e.release(p, Mods::default(), &M);
        assert!(e.cell_edit.is_none());
        match e.smart_data(a) {
            Some(Smart::Table(t)) => assert_eq!(t.cells[2][2], "=SUM(A1:B2)*2"),
            _ => panic!(),
        }
    }

    #[test]
    fn cells_of_a_reopened_table_can_be_edited() {
        // gespeicherte Lagen sind gerundet: nach dem Öffnen muss das Tippen trotzdem gehen
        let mut e = editor();
        e.set_tool(Tool::Data);
        drag(&mut e, (101.37, 99.71), (377.23, 211.19));
        let a = e.cell_edit.as_ref().unwrap().anchor;
        assert!(e.edit_insert("x"));
        e.end_text_edit();
        // gezoomt aufgezogen: Lage mit vielen Nachkommastellen
        if let Some(o) = e.doc.object_mut(a) {
            o.shape = Shape::Rect {
                x: 101.371234567,
                y: 99.7101234567,
                w: 275.86012345,
                h: 111.4812345,
                r: 0.0,
            };
        }
        e.rebuild_smart(a);
        let svg = crate::svg::write(&e.doc, false);
        let doc = crate::svg::read(&svg).unwrap();
        let mut e = Editor::new(doc);
        e.set_tool(Tool::Select);
        let a2 = e
            .doc
            .layers
            .iter()
            .flat_map(|l| &l.objects)
            .find(|o| o.smart.is_some())
            .map(|o| o.id)
            .unwrap();
        let _ = a;
        let f = e.doc.object(a2).and_then(smart_frame).unwrap();
        let s = e
            .view
            .to_screen(Pt::new(f.x0 + f.width() * 0.8, f.y0 + f.height() * 0.8));
        e.press(s, Mods::default(), false, &M);
        e.release(s, Mods::default(), &M);
        e.press(s, Mods::default(), false, &M);
        e.release(s, Mods::default(), &M);
        assert_eq!(e.cell_edit.as_ref().map(|c| (c.row, c.col)), Some((2, 2)));
        assert!(e.edit_insert("=A1&\"!\""));
        e.end_text_edit();
        match e.smart_data(a2) {
            Some(crate::smart::Smart::Table(t)) => assert_eq!(t.cells[2][2], "=A1&\"!\""),
            _ => panic!(),
        }
        assert!(!e.dirty || e.can_undo());
    }

    #[test]
    fn picked_cells_work_with_the_keyboard_and_clipboard() {
        use crate::smart::Smart;
        let mut e = editor();
        e.set_tool(Tool::Data);
        drag(&mut e, (100.0, 100.0), (400.0, 220.0));
        let a = e.cell_edit.as_ref().unwrap().anchor;
        e.end_text_edit();
        e.set_tool(Tool::Select);
        let table = |e: &Editor| match e.smart_data(a) {
            Some(Smart::Table(t)) => t.clone(),
            _ => panic!(),
        };
        e.pick = Some((a, Pick::Cells(0, 0, 0, 0)));
        // tippen ersetzt den Inhalt, Eingabe geht nach unten
        assert!(e.edit_pick("4", true));
        assert!(e.edit_insert("2"));
        assert!(e.cell_edit_move(1, 0));
        e.end_text_edit();
        assert_eq!(table(&e).cells[0][0], "42");
        assert_eq!(e.active_pick(), Some(Pick::Cells(1, 0, 1, 0)));
        // Pfeiltasten: wandern, mit Umschalt erweitern
        assert!(e.move_pick(-1, 0, false));
        assert!(e.move_pick(1, 1, true));
        assert_eq!(e.active_pick(), Some(Pick::Cells(0, 0, 1, 1)));
        assert!(e.move_pick(5, 5, false));
        assert_eq!(e.active_pick(), Some(Pick::Cells(2, 2, 2, 2)));
        // Esc: zurück zur ganzen Tabelle
        e.escape();
        assert_eq!(e.active_pick(), None);
        assert_eq!(e.single_smart(), Some(a));
        // kopieren und an anderer Stelle einfügen (die Tabelle wächst)
        e.pick = Some((a, Pick::Cells(0, 0, 0, 1)));
        assert_eq!(e.cells_text().as_deref(), Some("42\t"));
        e.pick = Some((a, Pick::Cells(2, 1, 2, 1)));
        assert!(e.paste_cells("a\tb\tc\n1\t2\t3\n"));
        let t = table(&e);
        assert_eq!((t.n_rows(), t.n_cols()), (4, 4));
        assert_eq!(t.cells[3][3], "3");
        assert_eq!(e.active_pick(), Some(Pick::Cells(2, 1, 3, 3)));
        assert!(e.clear_picked_cells());
        assert_eq!(table(&e).cells[2][1], "");
        // Formel in der Formelleiste: ihre Bezüge werden markiert
        e.formula_draft = Some((a, "=SUM(A1:B2)+C3".into()));
        let marks = e.formula_marks();
        assert_eq!(marks.len(), 2);
        assert_eq!(marks[0].2, (0, 0, 1, 1));
        // Schrift ändern, ohne dass sich etwas ändert, beendet das Tippen nicht
        assert!(e.begin_cell_edit(a, 0, 0));
        let size = table(&e).looks(false)[0][0].size;
        assert!(!e.update_text("size", |t| t.size = size));
        assert!(e.cell_edit.is_some());
        assert!(e.update_text("size", |t| t.size = 20.0));
        assert!(e.cell_edit.is_none());
        assert_eq!(table(&e).looks(false)[0][0].size, 20.0);
        assert_eq!(table(&e).cells[0][0], "42");
    }

    #[test]
    fn cells_merge_turn_vertical_take_number_formats_and_suggest_functions() {
        use crate::smart::{DateStyle, NumFmt, Role, Smart};
        let mut e = editor();
        e.set_tool(Tool::Data);
        drag(&mut e, (100.0, 100.0), (400.0, 220.0));
        let a = e.cell_edit.as_ref().unwrap().anchor;
        assert!(e.edit_insert("Title"));
        e.end_text_edit();
        e.set_tool(Tool::Select);
        let table = |e: &Editor| match e.smart_data(a) {
            Some(Smart::Table(t)) => t.clone(),
            _ => panic!(),
        };
        let cell_text = |e: &Editor, r: usize, c: usize| {
            e.smart_roles(a)
                .into_iter()
                .find(|(_, role)| *role == Role::CellText(r, c))
                .and_then(|(id, _)| e.doc.object(id))
                .map(|o| o.shape.clone())
        };
        // A1:C1 verbinden: ein Textfeld über drei Spalten, B1 und C1 ohne eigenes
        e.pick = Some((a, Pick::Cells(0, 0, 0, 2)));
        assert_eq!(e.can_merge(), (true, true));
        assert!(e.merge_cells(true));
        assert_eq!(table(&e).merges, vec![(0, 0, 0, 2)]);
        let f = e.doc.object(a).and_then(smart_frame).unwrap();
        assert!(
            matches!(cell_text(&e, 0, 0), Some(Shape::TextArea { w, .. }) if (w - f.width()).abs() < 1e-6)
        );
        assert!(cell_text(&e, 0, 1).is_none());
        assert_eq!(e.can_merge(), (false, true));
        // ein Klick in B1 wählt die ganze Verbindung, darin wird oben links getippt
        let (xs, ys) = table(&e).grid(f);
        let p = e
            .view
            .to_screen(Pt::new((xs[1] + xs[2]) / 2.0, (ys[0] + ys[1]) / 2.0));
        e.press(p, Mods::default(), false, &M);
        e.release(p, Mods::default(), &M);
        assert_eq!(e.active_pick(), Some(Pick::Cells(0, 0, 0, 2)));
        // nach unten wandern: aus der Verbindung heraus
        assert!(e.move_pick(1, 0, false));
        assert_eq!(e.active_pick(), Some(Pick::Cells(1, 0, 1, 0)));
        assert!(e.move_pick(-1, 0, false));
        assert_eq!(e.active_pick(), Some(Pick::Cells(0, 0, 0, 2)));
        // Spalte einfügen verbreitert die Verbindung
        e.pick = Some((a, Pick::Cells(1, 1, 1, 1)));
        assert!(e.table_op(TableOp::AddCol));
        assert_eq!(table(&e).merges, vec![(0, 0, 0, 3)]);
        // trennen
        e.pick = Some((a, Pick::Cells(0, 0, 0, 0)));
        assert!(e.merge_cells(false));
        assert!(table(&e).merges.is_empty());
        // senkrechter Text
        e.pick = Some((a, Pick::Cells(1, 0, 1, 0)));
        assert!(e.edit_table_looks("vertical", &|l| l.vertical = 1));
        assert!(matches!(cell_text(&e, 1, 0), Some(Shape::TextArea { xf, .. }) if xf.b < -0.99));
        assert!(e.begin_cell_edit(a, 1, 0));
        assert!(e.edit_insert("up"));
        e.end_text_edit();
        assert_eq!(table(&e).cells[1][0], "up");
        // Zahlenformate
        let shown = |e: &Editor, r: usize, c: usize| match cell_text(e, r, c) {
            Some(Shape::TextArea { text, .. }) => text,
            _ => panic!(),
        };
        let mut t = table(&e);
        t.cells[2][0] = "1234.5".into();
        t.cells[2][1] = "2026-11-05".into();
        t.cells[2][2] = "0.256".into();
        e.set_smart(a, Smart::Table(t));
        e.pick = Some((a, Pick::Cells(2, 0, 2, 0)));
        assert!(e.edit_table_looks("number", &|l| l.number = NumFmt::Currency {
            symbol: "£".into(),
            decimals: 2
        }));
        assert_eq!(shown(&e, 2, 0), "£1,234.50");
        e.pick = Some((a, Pick::Cells(2, 1, 2, 1)));
        assert!(e.edit_table_looks("number", &|l| l.number = NumFmt::Date(DateStyle::Long)));
        assert_eq!(shown(&e, 2, 1), "5 November 2026");
        e.pick = Some((a, Pick::Cells(2, 2, 2, 2)));
        assert!(e.edit_table_looks("number", &|l| l.number = NumFmt::Percent { decimals: 1 }));
        assert_eq!(shown(&e, 2, 2), "25.6%");
        assert_eq!(
            e.context_info().smart_look.map(|l| l.number),
            Some(NumFmt::Percent { decimals: 1 })
        );
        // gespeichert und gelesen
        let back = Smart::decode(&Smart::Table(table(&e)).encode()).unwrap();
        assert_eq!(back, Smart::Table(table(&e)));
        // Formel: Vorschlag, Eingabe übernimmt ihn
        assert!(e.begin_cell_edit(a, 2, 3));
        assert!(e.edit_insert("=su"));
        assert_eq!(e.formula_suggestion(), Some(("M".into(), "SUM".into())));
        assert!(e.accept_suggestion());
        assert_eq!(e.cell_input().unwrap().3, "=SUM(");
        assert_eq!(e.formula_suggestion(), None);
        // Pfeiltasten: nach Lostippen wechseln sie die Zelle
        e.cancel_cell_edit();
        e.pick = Some((a, Pick::Cells(1, 1, 1, 1)));
        assert!(e.edit_pick("7", true));
        assert!(e.cell_edit_arrow(0, 1));
        assert!(e.cell_edit.is_none());
        assert_eq!(table(&e).cells[1][1], "7");
        assert_eq!(e.active_pick(), Some(Pick::Cells(1, 2, 1, 2)));
        // nach Doppelklick bewegen links/rechts den Cursor, am Ende geht es weiter
        assert!(e.begin_cell_edit(a, 1, 1));
        assert!(e.text_edit.is_some_and(|t| t.caret == 1));
        assert!(!e.cell_edit_arrow(0, -1));
        assert!(e.cell_edit_arrow(0, 1));
        assert_eq!(e.cell_edit.as_ref().map(|c| (c.row, c.col)), Some((1, 2)));
    }

    #[test]
    fn images_are_not_placed_into_text_boxes() {
        let mut e = editor();
        e.set_tool(Tool::TextArea);
        drag(&mut e, (100.0, 100.0), (300.0, 200.0));
        e.end_text_edit();
        e.set_tool(Tool::Shape);
        drag(&mut e, (400.0, 100.0), (500.0, 200.0));
        let shape = e.selection[0];
        let p = e.view.to_doc(Pt::new(150.0, 150.0));
        assert_eq!(e.frame_target_at(p, &M, 0), None);
        let q = e.view.to_doc(Pt::new(450.0, 150.0));
        assert_eq!(e.frame_target_at(q, &M, 0), Some(shape));
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
        assert_eq!(e.context_info().title, "Framed Image");
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
            sub: Vec::new(),
            mask: false,
            locked: false,
            smart: None,
            name: String::new(),
        });
        id
    }

    #[test]
    fn nested_groups_and_clip_masks() {
        let mut e = editor();
        let a = rect_at(&mut e, 0.0, 0.0, 10.0, 10.0);
        let b = rect_at(&mut e, 20.0, 0.0, 10.0, 10.0);
        let c = rect_at(&mut e, 40.0, 0.0, 10.0, 10.0);
        e.selection = vec![a, b];
        assert!(e.group_selection());
        let g1 = e.doc.object(a).unwrap().group;
        // Gruppe in einer Gruppe
        e.selection = vec![a, b, c];
        assert!(e.group_selection());
        let g2 = e.doc.object(c).unwrap().group;
        assert_ne!(g1, g2);
        assert_eq!(
            (e.doc.object(a).unwrap().group, &e.doc.object(a).unwrap().sub),
            (g2, &vec![g1])
        );
        assert_eq!(e.doc.group_members(c).len(), 3);
        // Speichern und Öffnen behält die Verschachtelung
        let back = crate::svg::read(&crate::svg::write(&e.doc, true)).unwrap();
        let o = &back.layers[0].objects;
        assert!(o[0].group != 0 && o[0].group == o[2].group);
        assert_eq!(o[0].sub.len(), 1);
        assert_eq!(o[0].sub, o[1].sub);
        assert!(o[2].sub.is_empty());
        // Kopien bekommen auf allen Ebenen neue Nummern
        e.selection = vec![a, b, c];
        assert!(e.duplicate());
        let copy = e.doc.object(e.selection[0]).unwrap().clone();
        assert!(copy.group != g2 && copy.sub.len() == 1 && copy.sub[0] != g1);
        e.delete_selection();
        // Aufheben nimmt eine Ebene weg
        e.selection = vec![a, b, c];
        assert!(e.ungroup_selection());
        assert_eq!(e.doc.object(a).unwrap().group, g1);
        assert_eq!(e.doc.object(b).unwrap().group, g1);
        assert_eq!(e.doc.object(c).unwrap().group, 0);

        // Schnittmaske: das oberste Objekt begrenzt die übrigen
        let m = rect_at(&mut e, 5.0, 0.0, 30.0, 10.0);
        e.selection = vec![a, m];
        e.make_clip_mask().unwrap();
        let om = e.doc.object(m).unwrap().clone();
        assert!(om.mask);
        assert_eq!(e.doc.object(b).unwrap().group, om.group);
        assert_eq!(e.doc.object(a).unwrap().sub, vec![g1]);
        // nur der sichtbare Teil trifft, die Maske selbst nie
        e.selection.clear();
        assert_eq!(e.hit(Pt::new(2.0, 5.0), &M), None);
        assert_eq!(e.hit(Pt::new(7.0, 5.0), &M), Some(a));
        assert_eq!(e.hit(Pt::new(34.5, 5.0), &M), None);
        // Auswahlrahmen: höchstens die Maske
        e.selection = e.doc.group_members(a);
        let bb = e.selection_bbox(&M).unwrap();
        assert!(
            (bb.x0 - 5.0).abs() < 1e-9 && (bb.x1 - 35.0).abs() < 1e-9,
            "{bb:?}"
        );
        // Speichern: clipPath für andere Programme, die Maske kommt zurück
        let text = crate::svg::write(&e.doc, true);
        assert!(
            text.contains("<clipPath id=\"mask") && text.contains("clip-path=\"url(#mask"),
            "{text}"
        );
        let back = crate::svg::read(&text).unwrap();
        let o = &back.layers[0].objects;
        let bm = o.iter().find(|x| x.mask).expect("Maske");
        assert!(o.iter().filter(|x| !x.mask && x.group == bm.group).count() == 2);
        let clipped = o.iter().find(|x| !x.mask && x.group == bm.group).unwrap();
        assert_eq!(crate::model::clips_of(clipped, &back.masks()).len(), 1);
        assert!(crate::model::clips_of(&o[0], &back.masks()).is_empty());
        // Lösen: die Maske wird wieder sichtbar, die Gruppe darum verschwindet
        e.selection = vec![m];
        assert!(e.release_clip_mask());
        assert!(!e.doc.object(m).unwrap().mask);
        assert_eq!(e.doc.object(m).unwrap().group, 0);
        assert_eq!(e.doc.object(a).unwrap().group, g1);
        // eine Gruppe als oberstes Objekt geht nicht
        e.selection = vec![c, a, b];
        assert!(e.make_clip_mask().is_err() || !e.doc.object(b).unwrap().mask);
    }

    #[test]
    fn global_colors_follow_the_library() {
        let mut lib = crate::library::Library::new("Test");
        let blue = Color::rgb(0.0, 0.0, 1.0);
        assert!(lib.add_paint(blue, None));
        let id = lib.color_link(0).unwrap();
        let mut e = editor();
        let a = rect_at(&mut e, 0.0, 0.0, 10.0, 10.0);
        let b = rect_at(&mut e, 20.0, 0.0, 10.0, 10.0);
        let c = rect_at(&mut e, 40.0, 0.0, 10.0, 10.0);
        e.selection = vec![a, b, c];
        assert!(e.apply_lib_color(blue, None, Some(id.clone()), false));
        // b bekommt eine eigene Deckkraft (bleibt verknüpft), c eine andere Farbe von Hand
        e.selection = vec![b];
        e.update_style("alpha", |st| st.set_paint_alpha(PaintTarget::Fill, 0.5));
        e.doc.object_mut(c).unwrap().style.fill = Some(Color::rgb(0.0, 1.0, 0.0));
        let red = Color::rgb(1.0, 0.0, 0.0);
        let (lid, _) = lib.set_color(0, red).unwrap();
        assert_eq!(lid, id);
        let n = e.sync_library_colors(&|k| lib.linked_color(k));
        assert_eq!(n, 2);
        let fill = |e: &Editor, id| e.doc.object(id).unwrap().style.fill.unwrap();
        assert_eq!(fill(&e, a), red);
        assert!((fill(&e, b).r - 1.0).abs() < 1e-9 && (fill(&e, b).a - 0.5).abs() < 1e-9);
        assert_eq!(fill(&e, c), Color::rgb(0.0, 1.0, 0.0));
        assert!(e.doc.object(c).unwrap().style.fill_link.is_none());
        assert!(e.undo());
        assert_eq!(fill(&e, a), blue);
        // eine von Hand gewählte Farbe löst die Verknüpfung
        e.redo();
        e.selection = vec![a];
        e.update_style("fill", |st| st.set_paint(PaintTarget::Fill, Some(blue), None));
        assert!(e.doc.object(a).unwrap().style.fill_link.is_none());
        // Verläufe werden nicht verknüpft
        let lp = LibPayload::Color(red, Some(Gradient::new(red, blue)), Some(id.clone()));
        let mut o = e.doc.object(b).unwrap().clone();
        assert!(restyle(&mut o, &lp));
        assert!(o.style.fill_link.is_none());
        // die Kennung überlebt das Speichern der Bibliothek und das Speichern des Dokuments
        let back = crate::library::Library::from_document("Test", &lib.to_document());
        assert_eq!(back.color_link(0).as_deref(), Some(id.as_str()));
        let d2 = crate::svg::read(&crate::svg::write(&e.doc, false)).unwrap();
        let o2 = d2.layers.iter().flat_map(|l| &l.objects).nth(1).unwrap();
        assert_eq!(o2.style.fill_link.as_ref().map(|l| l.id.clone()), Some(id));
        assert!(d2
            .layers
            .iter()
            .flat_map(|l| &l.objects)
            .nth(2)
            .unwrap()
            .style
            .fill_link
            .is_none());
    }

    #[test]
    fn offset_simplify_and_repeat() {
        use crate::model::LineJoin;
        let mut e = editor();
        let a = rect_at(&mut e, 10.0, 10.0, 40.0, 20.0);
        // nach außen: neue Fläche darunter, 5 größer
        e.selection = vec![a];
        assert_eq!(e.offset_path(5.0, LineJoin::Miter, &M), 1);
        let out = e.doc.object(e.selection[0]).unwrap().clone();
        let b = out.shape.geom_bbox().unwrap();
        assert!(
            (b.x0 - 5.0).abs() < 0.3 && (b.x1 - 55.0).abs() < 0.3 && (b.y0 - 5.0).abs() < 0.3,
            "{b:?}"
        );
        let order: Vec<u64> = e.doc.layers[0].objects.iter().map(|o| o.id).collect();
        assert_eq!(order, vec![out.id, a]);
        // nach innen: darüber, kleiner; zu weit nach innen: nichts
        e.selection = vec![a];
        assert_eq!(e.offset_path(-4.0, LineJoin::Round, &M), 1);
        let inn = e.doc.object(e.selection[0]).unwrap().shape.geom_bbox().unwrap();
        assert!(
            (inn.x0 - 14.0).abs() < 0.3 && (inn.y1 - 26.0).abs() < 0.3,
            "{inn:?}"
        );
        e.selection = vec![a];
        assert_eq!(e.offset_path(-30.0, LineJoin::Round, &M), 0);

        // Vereinfachen: ein Kreis aus 200 Eckpunkten wird zu wenigen glatten Knoten
        let pts: Vec<Node> = (0..200)
            .map(|i| {
                let t = i as f64 / 200.0 * std::f64::consts::TAU;
                Node::corner(Pt::new(100.0 + 50.0 * t.cos(), 100.0 + 50.0 * t.sin()))
            })
            .collect();
        let id = e.doc.alloc_id();
        e.doc.layers[0].objects.push(Object {
            id,
            shape: Shape::Path {
                subpaths: vec![SubPath {
                    nodes: pts,
                    closed: true,
                }],
                evenodd: false,
            },
            style: Style::default(),
            angle: 0.0,
            group: 0,
            sub: Vec::new(),
            mask: false,
            locked: false,
            smart: None,
            name: String::new(),
        });
        e.selection = vec![id];
        let saved = e.simplify_paths(&M);
        let Shape::Path { subpaths, .. } = &e.doc.object(id).unwrap().shape else {
            panic!()
        };
        let n = subpaths[0].nodes.len();
        assert!(saved > 150 && n < 40, "{n} Knoten");
        // die Form bleibt ein Kreis (Knoten liegen auf dem Rand)
        for nd in &subpaths[0].nodes {
            assert!((nd.p.dist(Pt::new(100.0, 100.0)) - 50.0).abs() < 1.0);
        }
        // ein Quadrat behält seine vier Ecken
        let sq = SubPath {
            nodes: [(0.0, 0.0), (5.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]
                .iter()
                .map(|(x, y)| Node::corner(Pt::new(*x, *y)))
                .collect(),
            closed: true,
        };
        let s2 = simplify_subpath(&sq, 0.5);
        assert_eq!(s2.nodes.len(), 4);

        // Wiederholen: 3 Kopien, jeweils 30 nach rechts, alles eine Gruppe
        let mut e = editor();
        let r = rect_at(&mut e, 0.0, 0.0, 10.0, 10.0);
        e.selection = vec![r];
        assert!(e.repeat_selection(3, 30.0, 0.0, 0.0, 1.0, None, &M));
        assert_eq!(e.doc.layers[0].objects.len(), 4);
        let xs: Vec<f64> = e.doc.layers[0]
            .objects
            .iter()
            .map(|o| o.shape.geom_bbox().unwrap().x0)
            .collect();
        assert_eq!(xs, vec![0.0, 30.0, 60.0, 90.0]);
        let g = e.doc.layers[0].objects[0].group;
        assert!(g != 0 && e.doc.layers[0].objects.iter().all(|o| o.group == g));
        // Drehen um einen Punkt: 4 × 90° um (50, 50) – die letzte Kopie liegt wieder am Anfang
        let mut e = editor();
        let r = rect_at(&mut e, 90.0, 45.0, 10.0, 10.0);
        e.selection = vec![r];
        assert!(e.repeat_selection(4, 0.0, 0.0, 90.0, 1.0, Some(Pt::new(50.0, 50.0)), &M));
        let last = e.doc.layers[0].objects[4].shape.geom_bbox().unwrap();
        assert!(
            (last.x0 - 90.0).abs() < 1e-6 && (last.y0 - 45.0).abs() < 1e-6,
            "{last:?}"
        );
        let second = e.doc.layers[0].objects[1].shape.geom_bbox().unwrap();
        // gegen den Uhrzeigersinn: von rechts nach oben
        assert!(second.y1 < 50.0 && (second.x0 - 45.0).abs() < 1e-6, "{second:?}");
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
        assert!(e.apply_lib_color(Color::rgb(0.0, 0.0, 1.0), None, None, false));
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
        e.lib_drag_motion(Pt::new(30.0, 30.0), LibPayload::Color(red, None, None), &M);
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
        e.lib_drag_motion(Pt::new(300.0, 300.0), LibPayload::Color(red, None, None), &M);
        assert!(e.lib_drop.is_none());
        assert!(e.lib_drop_at(Pt::new(30.0, 30.0), &LibPayload::Color(red, None, None), &M));
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
        // Auswahl bleibt der Arbeitsbereich: übriges Stück und neue Fläche
        assert_eq!(e.selection.len(), 2);
        let merged = e
            .doc
            .object(*e.selection.last().unwrap())
            .unwrap()
            .shape
            .geom_bbox()
            .unwrap();
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
        // die übrigen Stücke bleiben der Arbeitsbereich
        assert_eq!(e.selection.len(), 2);
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
        // Objekt auf der zweiten Zeichenfläche (rechts neben der ersten)
        e.set_tool(Tool::Shape);
        drag(&mut e, (560.0, 20.0), (600.0, 60.0));
        let id = e.selection[0];
        e.deselect();
        e.set_tool(Tool::Select);
        let free = (e.doc.artboards[0].x, e.doc.artboards[0].y);
        let before = e.doc.object(id).unwrap().shape.geom_bbox().unwrap();
        e.view = View {
            zoom: 1.0,
            ox: 0.0,
            oy: 0.0,
        };
        e.set_view_mode(ViewMode::Notebook);
        assert_eq!(e.view_mode(), ViewMode::Notebook);
        assert_eq!(e.page_index(), Some(0));
        // die Seiten stehen untereinander, das Objekt ist mitgewandert
        let rects = e.doc.artboard_rects();
        let (r0, r1) = (rects[0].1, rects[1].1);
        assert!((r1.y0 - (r0.y1 + PAGE_GAP)).abs() < 1e-9);
        assert!((r1.x0 - r0.x0).abs() < 1e-9);
        let ob = e.doc.object(id).unwrap().shape.geom_bbox().unwrap();
        assert!(r1.contains(Pt::new((ob.x0 + ob.x1) / 2.0, (ob.y0 + ob.y1) / 2.0)));
        assert_eq!(e.doc.artboards[0].free, Some(free));
        // kein Ausschnitt einer einzelnen Seite: alle Seiten sind zu sehen
        assert_eq!(e.page_clip(), None);
        assert_eq!(e.shown_pages().unwrap().len(), 2);
        // Ganze Seite: die aktuelle passt ins Bild (Abstand 48)
        e.fit(800.0, 300.0);
        let z = e.view.zoom;
        assert!((z - (300.0 - 96.0) / r0.height()).abs() < 1e-9);
        // durchgehend scrollen: die zweite Seite folgt ohne Umblättern
        e.view.zoom = 1.0;
        e.view.oy = 24.0;
        e.view.ox = 0.0;
        assert!(!e.notebook_scroll(0.0, 100.0));
        assert!((e.view.oy - (24.0 - 100.0)).abs() < 1e-9);
        assert!(e.notebook_scroll(0.0, r0.height()));
        assert_eq!(e.page_index(), Some(1));
        // am Ende der letzten Seite ist Schluss
        e.notebook_scroll(0.0, 10_000.0);
        assert!((e.view.oy - (300.0 - 24.0 - r1.y1)).abs() < 1e-9);
        // im Notizheft lässt sich nur Text anklicken; in der Präsentation das Objekt dort,
        // wo es jetzt liegt
        let c = Pt::new((ob.x0 + ob.x1) / 2.0, (ob.y0 + ob.y1) / 2.0);
        assert_eq!(e.hit(c, &M), None);
        e.set_view_mode(ViewMode::Presentation);
        assert_eq!(e.hit(c, &M), Some(id));
        e.set_view_mode(ViewMode::Notebook);
        assert_eq!(e.tool, Tool::Text);
        // zu einer Seite springen: passt sie ins Bild, steht sie in der Mitte
        e.view.zoom = 0.2;
        assert!(e.go_page(0, false));
        let top = e.view.to_screen(Pt::new(0.0, r0.y0)).y;
        let bottom = e.view.to_screen(Pt::new(0.0, r0.y1)).y;
        assert!((top - (300.0 - bottom)).abs() < 1.0);
        assert!(!e.go_page(2, false));
        // Rückgängig ändert die Ansicht nicht: die Seiten bleiben untereinander
        e.set_tool(Tool::Shape);
        assert_eq!(e.tool, Tool::Text);
        let at = e.view.to_screen(Pt::new(r0.x0 + 100.0, r0.y0 + 100.0));
        drag(&mut e, (at.x, at.y), (at.x, at.y));
        assert_eq!(e.body_frames().len(), 1);
        assert!(e.doc.layers[0].locked);
        assert!(e.undo());
        assert!(e.body_frames().is_empty());
        assert_eq!(e.view_mode(), ViewMode::Notebook);
        assert!((e.doc.artboard_rects()[1].1.y0 - r1.y0).abs() < 1e-9);
        // zurück zu den Zeichenflächen: alles wieder an seinem Platz
        e.set_view_mode(ViewMode::Artboards);
        assert_eq!(e.page_clip(), None);
        assert_eq!(e.shown_pages(), None);
        assert_eq!((e.doc.artboards[0].x, e.doc.artboards[0].y), free);
        assert_eq!(e.doc.artboards[0].free, None);
        assert_eq!(e.doc.object(id).unwrap().shape.geom_bbox().unwrap(), before);
    }

    #[test]
    fn presentation_marks_are_temporary_and_the_laser_fades() {
        let mut e = editor();
        e.add_artboard();
        e.viewport = (800.0, 600.0);
        let objects = |e: &Editor| e.doc.layers.iter().map(|l| l.objects.len()).sum::<usize>();
        let n = objects(&e);
        assert!(e.set_view_mode(ViewMode::Presentation));
        assert!(e.read_only && e.temporary_marks());
        e.view_tool = ViewTool::Highlight;
        drag(&mut e, (100.0, 100.0), (300.0, 100.0));
        // die Markierung ist zu sehen, gehört aber nicht zum Dokument
        assert_eq!(e.marks.len(), 1);
        assert_eq!(objects(&e), n);
        assert!(e.can_undo());
        assert!(e.undo());
        assert!(e.marks.is_empty());
        assert!(e.redo());
        assert_eq!(e.marks.len(), 1);
        // PDF: Markierungen werden Teil des Dokuments
        e.keep_marks = true;
        drag(&mut e, (100.0, 140.0), (300.0, 140.0));
        assert_eq!(objects(&e), n + 1);
        e.keep_marks = false;
        // Laserpointer: leuchtet, ändert nichts
        e.view_tool = ViewTool::Laser;
        drag(&mut e, (100.0, 200.0), (200.0, 260.0));
        assert!(e.laser_alive());
        assert_eq!(objects(&e), n + 1);
        // Vollbild: eine Seite zur Zeit, mit dem Textmarker wird nicht geblättert
        e.set_presenting(true);
        assert!(e.page_clip().is_some());
        let page = e.page_index();
        e.view_tool = ViewTool::Highlight;
        drag(&mut e, (100.0, 100.0), (300.0, 100.0));
        assert_eq!(e.page_index(), page);
        e.view_tool = ViewTool::Select;
        drag(&mut e, (100.0, 100.0), (100.0, 100.0));
        assert_eq!(e.page_index(), page.map(|p| p + 1));
        e.set_presenting(false);
        assert_eq!(e.page_clip(), None);
        // wieder bearbeiten: die eigenen Markierungen sind weg
        e.set_view_mode(ViewMode::Notebook);
        assert!(!e.read_only);
        assert!(e.marks.is_empty());
    }

    #[test]
    fn automatic_line_spacing_and_body_selection() {
        let mut e = editor();
        e.set_view_mode(ViewMode::Artboards);
        // neue Texte: automatischer Zeilenabstand; ein fester Wert schaltet ihn ab
        e.set_tool(Tool::Text);
        drag(&mut e, (100.0, 100.0), (300.0, 160.0));
        assert!(e.edit_insert("Hallo"));
        e.end_text_edit();
        let id = e.doc.layers[0].objects.last().unwrap().id;
        let fmt = |e: &Editor| match &e.doc.object(id).unwrap().shape {
            Shape::TextArea { fmt, .. } | Shape::Text { fmt, .. } => *fmt,
            _ => unreachable!(),
        };
        assert!(fmt(&e).line_auto);
        let back = crate::svg::read(&crate::svg::write(&e.doc, false)).unwrap();
        assert!(back.layers.iter().flat_map(|l| &l.objects).any(|o| matches!(
            &o.shape,
            Shape::TextArea { fmt, .. } | Shape::Text { fmt, .. } if fmt.line_auto
        )));
        e.selection = vec![id];
        e.update_text("line", |t| {
            t.fmt.line = 1.2;
            t.fmt.line_auto = false;
        });
        assert!(!fmt(&e).line_auto);
        // fest 120 % (der Standardwert) bleibt beim Speichern fest
        let back = crate::svg::read(&crate::svg::write(&e.doc, false)).unwrap();
        assert!(back.layers.iter().flat_map(|l| &l.objects).any(|o| matches!(
            &o.shape,
            Shape::TextArea { fmt, .. } | Shape::Text { fmt, .. } if !fmt.line_auto && (fmt.line - 1.2).abs() < 1e-9
        )));
        // Notizheft: ohne Auswahl gilt der Fließtext als ausgewählt (die Eigenschaften
        // zeigen seine Textoptionen, nicht die des zuletzt gewählten Objekts)
        e.set_view_mode(ViewMode::Notebook);
        e.viewport = (800.0, 600.0);
        let r = e.doc.artboard_rects()[0].1;
        let at = e.view.to_screen(Pt::new(r.x0 + 60.0, r.y0 + 60.0));
        drag(&mut e, (at.x, at.y), (at.x, at.y));
        assert!(e.edit_insert("Text"));
        e.end_text_edit();
        e.set_view_mode(ViewMode::Artboards);
        e.selection = vec![id];
        e.set_view_mode(ViewMode::Notebook);
        assert_eq!(e.selection, e.body_frames());
        assert!(e.context_info().body);
        e.deselect();
        assert!(e.keep_body_selected());
        assert_eq!(e.selection, e.body_frames());
    }

    #[test]
    fn presentation_selects_only_text() {
        let mut e = editor();
        e.set_view_mode(ViewMode::Artboards);
        e.set_tool(Tool::Text);
        drag(&mut e, (100.0, 100.0), (300.0, 160.0));
        assert!(e.edit_insert("Hallo Welt"));
        e.end_text_edit();
        let text = e.doc.layers[0].objects.last().unwrap().id;
        // ein Rechteck halb über dem Text
        e.set_tool(Tool::Shape);
        drag(&mut e, (80.0, 90.0), (200.0, 260.0));
        e.deselect();
        e.view = View {
            zoom: 1.0,
            ox: 0.0,
            oy: 0.0,
        };
        assert!(e.set_view_mode(ViewMode::Presentation));
        e.view = View {
            zoom: 1.0,
            ox: 0.0,
            oy: 0.0,
        };
        e.view_tool = ViewTool::Select;
        // das Rechteck lässt sich nicht auswählen …
        drag(&mut e, (120.0, 220.0), (120.0, 220.0));
        assert!(e.selection.is_empty());
        assert!(e.text_edit.is_none());
        // … und verdeckt den Text darunter nicht
        drag(&mut e, (110.0, 110.0), (110.0, 110.0));
        assert_eq!(e.text_edit.map(|t| t.id), Some(text));
        // Aufziehen wählt nur Texte
        e.end_text_edit();
        e.deselect();
        drag(&mut e, (20.0, 20.0), (450.0, 450.0));
        assert_eq!(e.selection, vec![text]);
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
        assert!(e.doc.artboards[1].name.ends_with("(copy)"));
        assert!(e.undo());
        assert_eq!(e.doc.artboards.len(), 1);
    }

    #[test]
    fn scaling_snaps_anchor_points() {
        let mut e = editor();
        e.set_tool(Tool::Shape);
        // Ziel: Rechteck mit linker Kante bei x = 300
        drag(&mut e, (300.0, 300.0), (360.0, 360.0));
        e.deselect();
        // Dreieck 100…200: die Spitze oben liegt in der Mitte (x = 150)
        e.shape_kind = ShapeKind::Polygon;
        e.corners = 3;
        drag(&mut e, (100.0, 100.0), (200.0, 200.0));
        let id = e.selection[0];
        e.set_tool(Tool::Select);
        e.selection = vec![id];
        let b = e.selection_frame(&M).unwrap().0;
        let tip_x = |e: &Editor| {
            let o = e.doc.object(id).unwrap();
            let Some(Shape::Path { subpaths, .. }) = o.shape.to_path() else {
                panic!()
            };
            subpaths[0]
                .nodes
                .iter()
                .min_by(|a, b| a.p.y.total_cmp(&b.p.y))
                .unwrap()
                .p
                .x
        };
        let tip0 = tip_x(&e);
        // rechten Anfasser so ziehen, dass die Spitze knapp neben x = 300 landet (sie
        // bewegt sich langsamer als die Kante)
        let k = (tip0 - b.x0) / b.width();
        let want_edge = b.x1 + (300.0 - 3.0 - tip0) / k;
        let h = e.view.to_screen(Pt::new(b.x1, (b.y0 + b.y1) / 2.0));
        let to = e.view.to_screen(Pt::new(want_edge, (b.y0 + b.y1) / 2.0));
        drag(&mut e, (h.x, h.y), (to.x, to.y));
        assert!((tip_x(&e) - 300.0).abs() < 1e-6, "{}", tip_x(&e));
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
        e.doc.background = Some(Color::WHITE);
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
        // eigene Farbe der zweiten Zeichenfläche
        let red = Color::rgb(1.0, 0.0, 0.0);
        assert!(e.set_artboard_background(1, Some(red)));
        assert_eq!(e.doc.artboard_backgrounds(), vec![Some(Color::WHITE), Some(red)]);
        assert_eq!(e.doc.artboard_doc(1).background, Some(red));
        assert!(e.delete_artboard(0));
        assert!(e.doc.artboards.is_empty());
        assert_eq!(e.doc.background, Some(red));
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
    fn artboards_can_be_reordered() {
        let mut e = editor();
        e.doc.background = Some(Color::WHITE);
        e.add_artboard();
        e.add_artboard();
        assert!(e.rename_artboard(2, "Back"));
        let red = Color::rgb(1.0, 0.0, 0.0);
        assert!(e.set_artboard_background(1, Some(red)));
        let rects = e.doc.artboard_rects();
        // etwas auf der zweiten Zeichenfläche
        e.set_tool(Tool::Shape);
        let s0 = e.view.to_screen(Pt::new(rects[1].1.x0 + 20.0, 20.0));
        drag(&mut e, (s0.x, s0.y), (s0.x + 30.0, s0.y + 30.0));
        let id = e.selection[0];
        let screen = |e: &Editor| {
            let b = e.doc.object(id).unwrap().shape.geom_bbox().unwrap();
            e.view.to_screen(Pt::new(b.x0, b.y0))
        };
        let before = screen(&e);
        e.notebook = Some(1);
        // die zweite wird die erste: sie rückt in den Ursprung, alles wandert mit
        assert!(e.move_artboard(1, 0));
        let names: Vec<String> = e.doc.artboard_rects().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, vec!["Artboard 1", "Artboard 2", "Back"]);
        assert_eq!(e.doc.background, Some(red));
        assert_eq!(e.doc.artboard_backgrounds()[1], Some(Color::WHITE));
        assert_eq!(
            e.doc.artboard_rects()[1].1,
            Rect::from_xywh(-rects[1].1.x0, 0.0, rects[0].1.width(), rects[0].1.height())
        );
        assert_eq!(e.notebook, Some(0));
        let after = screen(&e);
        assert!((after.x - before.x).abs() < 1e-9 && (after.y - before.y).abs() < 1e-9);
        // eigener Name nach vorn: bleibt erhalten
        assert!(e.move_artboard(2, 0));
        assert_eq!(e.doc.artboard_rects()[0].0, "Back");
        assert_eq!(e.doc.first_name, "Back");
        assert!(e.undo());
        assert!(e.undo());
        assert_eq!(e.doc.background, Some(Color::WHITE));
        assert_eq!(e.doc.artboard_rects()[0].1.x0, 0.0);
        // Herkunft der Seiten (PDF) wandert mit, beim Löschen werden die Namen neu gezählt
        e.doc.pages = true;
        e.doc.page_src = vec![0, 1, 2];
        e.doc.artboards[0].name = "Page 2".into();
        assert!(e.move_artboard(0, 2));
        assert_eq!(e.doc.page_src, vec![1, 2, 0]);
        assert!(e.delete_artboard(0));
        assert_eq!(e.doc.page_src, vec![2, 0]);
        let names: Vec<String> = e.doc.artboard_rects().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, vec!["Back", "Page 2"]);
        // nur ansehen: Ziehen verschiebt die Ansicht, ändert nichts
        e.deselect();
        e.read_only = true;
        e.view_tool = ViewTool::Hand;
        e.set_tool(Tool::Select);
        let n = e.doc.layers[0].objects.len();
        let ox = e.view.ox;
        drag(&mut e, (10.0, 10.0), (60.0, 10.0));
        assert_eq!(e.doc.layers[0].objects.len(), n);
        assert!(e.selection.is_empty());
        assert!((e.view.ox - ox - 50.0).abs() < 1e-9);
        // Seite mit ihrem Inhalt entfernen (Betrachtung), Seite zeigen, Verlauf vergessen
        e.read_only = false;
        let r1 = e.doc.artboard_rects()[1].1;
        e.set_tool(Tool::Shape);
        let a = e.view.to_screen(Pt::new(r1.x0 + 10.0, r1.y0 + 10.0));
        drag(&mut e, (a.x, a.y), (a.x + 20.0, a.y + 20.0));
        let n = e.doc.layers[0].objects.len();
        e.viewport = (800.0, 600.0);
        assert!(e.show_artboard(1));
        let c = e.view.to_doc(Pt::new(400.0, 300.0));
        assert!(r1.contains(c));
        assert!(e.delete_page(1, &M));
        assert_eq!(e.doc.layers[0].objects.len(), n - 1);
        assert_eq!(e.doc.artboard_rects().len(), 1);
        assert!(e.can_undo());
        e.forget_history();
        assert!(!e.can_undo() && !e.can_redo());
    }

    #[test]
    fn viewing_selects_text_and_highlights() {
        let mut e = editor();
        let id = area_with(&mut e, "Hello world example");
        e.read_only = true;
        e.view_tool = ViewTool::Select;
        let n = e.doc.layers[0].objects.len();
        // im Text ziehen markiert Zeichen, ändert aber nichts
        drag(&mut e, (30.0, 20.0), (80.0, 20.0));
        assert_eq!(e.text_edit.map(|t| t.id), Some(id));
        assert_eq!(e.copy_text().as_deref(), Some("lo wo"));
        assert!(!e.dirty && !e.can_undo());
        // daneben klicken beendet das Markieren; Rahmen wählt das Objekt (ganzer Text)
        drag(&mut e, (400.0, 400.0), (400.0, 400.0));
        assert!(e.text_edit.is_none());
        drag(&mut e, (5.0, 5.0), (300.0, 200.0));
        assert_eq!(e.selection, vec![id]);
        assert_eq!(e.copy_text().as_deref(), Some("Hello world example"));
        // Objekte lassen sich nicht verschieben
        let before = e.doc.object(id).unwrap().shape.clone();
        drag(&mut e, (50.0, 50.0), (120.0, 90.0));
        assert_eq!(e.doc.object(id).unwrap().shape, before);
        // Textmarker: fast waagerecht wird gerade, Breite nach der Schrift
        e.view_tool = ViewTool::Highlight;
        drag(&mut e, (20.0, 30.0), (150.0, 33.0));
        assert_eq!(e.doc.layers[0].objects.len(), n + 1);
        let o = e.doc.layers[0].objects.last().unwrap();
        assert_eq!(o.style.blend, Blend::Multiply);
        assert!((o.style.width - 15.0).abs() < 1e-9, "{}", o.style.width);
        let b = o.shape.geom_bbox().unwrap();
        assert!(b.height() < 1e-6 && (b.width() - 130.0).abs() < 1e-6, "{b:?}");
        // ein Klick ist kein Strich
        drag(&mut e, (20.0, 30.0), (20.0, 30.0));
        assert_eq!(e.doc.layers[0].objects.len(), n + 1);
        assert!(e.undo());
        assert_eq!(e.doc.layers[0].objects.len(), n);
    }

    #[test]
    fn presenting_turns_pages_and_fills_the_screen() {
        let mut e = editor();
        e.add_artboard();
        e.set_view_mode(ViewMode::Notebook);
        e.go_page(0, false);
        e.viewport = (1000.0, 500.0);
        e.presenting = true;
        assert!(e.present_fit());
        let r = e.page_clip().unwrap();
        let z = (1000.0 / r.width()).min(500.0 / r.height());
        assert!((e.view.zoom - z).abs() < 1e-9);
        let n = e.doc.layers[0].objects.len();
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 200.0));
        // ein Klick blättert, nichts wird gezeichnet
        assert_eq!(e.page_index(), Some(1));
        assert_eq!(e.doc.layers[0].objects.len(), n);
        assert_eq!(e.cursor_at(Pt::new(10.0, 10.0), &M), "default");
    }

    #[test]
    fn selected_guides_show_their_settings() {
        let mut e = editor();
        e.doc.guides.push(crate::model::Guide {
            pos: Pt::new(0.0, 50.0),
            angle: 0.0,
            layer: 0,
        });
        e.set_tool(Tool::Select);
        e.guide_sel = Some(0);
        let info = e.context_info();
        assert!(info.show_guides_panel && !info.empty_hint);
        e.guide_sel = None;
        assert!(e.context_info().empty_hint);
    }

    #[test]
    fn tables_and_charts_rebuild_from_their_data() {
        use crate::smart::{Chart, ChartKind, Smart, Table};
        let mut e = editor();
        let frame = Rect::from_xywh(10.0, 10.0, 200.0, 100.0);
        let id = e.insert_smart(Smart::Table(Table::sample()), frame).unwrap();
        let members = e.doc.group_members(id);
        assert!(members.len() > 5);
        assert_eq!(e.selection.len(), members.len());
        let texts = |e: &Editor| -> Vec<String> {
            e.doc
                .layers
                .iter()
                .flat_map(|l| &l.objects)
                .filter_map(|o| o.shape.text_fields().map(|t| t.0.to_string()))
                .collect()
        };
        assert!(texts(&e).contains(&"245".to_string()));
        // Zelle ändern: Summe folgt, ein Schritt zum Rückgängigmachen, Auswahl bleibt ganz
        let Some(Smart::Table(mut t)) = e.smart_data(id).cloned() else {
            panic!()
        };
        t.cells[1][1] = "200".into();
        assert!(e.set_smart(id, Smart::Table(t.clone())));
        assert!(!e.set_smart(id, Smart::Table(t)));
        assert!(texts(&e).contains(&"325".to_string()), "{:?}", texts(&e));
        assert_eq!(e.selection.len(), e.doc.group_members(id).len());
        assert!(e.undo());
        assert!(texts(&e).contains(&"245".to_string()));
        // Speichern und Laden: Daten bleiben am Rahmen
        let svg = crate::svg::write(&e.doc, false);
        let doc = crate::svg::read(&svg).unwrap();
        let a = doc
            .layers
            .iter()
            .flat_map(|l| &l.objects)
            .find(|o| o.smart.is_some())
            .unwrap();
        assert_eq!(a.smart.as_deref(), e.smart_data(id));
        assert_eq!(doc.group_members(a.id).len(), e.doc.group_members(id).len());
        // Diagramm: Art wechseln baut neu
        let c = e
            .insert_smart(Smart::Chart(Chart::sample(ChartKind::Pie)), frame)
            .unwrap();
        let n_pie = e.doc.group_members(c).len();
        let Some(Smart::Chart(mut ch)) = e.smart_data(c).cloned() else {
            panic!()
        };
        ch.kind = ChartKind::Bar;
        assert!(e.set_smart(c, Smart::Chart(ch)));
        assert_ne!(e.doc.group_members(c).len(), n_pie);
        // Kopieren: die Kopie hat eigene Daten und lässt sich ändern
        e.selection = e.doc.group_members(id);
        assert!(e.duplicate());
        let copy = e.smart_anchors()[0];
        assert_ne!(copy, id);
        let Some(Smart::Table(mut t2)) = e.smart_data(copy).cloned() else {
            panic!()
        };
        t2.cells[0][0] = "Kopie".into();
        assert!(e.set_smart(copy, Smart::Table(t2)));
        assert!(texts(&e).contains(&"Kopie".to_string()));
        assert_eq!(texts(&e).iter().filter(|t| t.as_str() == "Item").count(), 1);
    }

    #[test]
    fn data_tool_types_into_cells_and_charts_follow() {
        use crate::smart::{ChartKind, Smart};
        let mut e = editor();
        e.set_tool(Tool::Data);
        // Tabelle aufziehen: drei mal drei Zellen, die erste wird gleich bearbeitet
        drag(&mut e, (50.0, 50.0), (350.0, 170.0));
        let ce = e.cell_edit.clone().expect("Zelle wird bearbeitet");
        assert_eq!((ce.row, ce.col), (0, 0));
        let a = ce.anchor;
        let table = |e: &Editor| match e.smart_data(a) {
            Some(Smart::Table(t)) => t.clone(),
            _ => panic!(),
        };
        assert_eq!((table(&e).n_rows(), table(&e).n_cols()), (3, 3));
        let undo0 = e.undo.len();
        // tippen, Tab: nächste Zelle; Formel; Eingabe: nach unten
        assert!(e.edit_insert("Name"));
        assert!(e.cell_edit_move(0, 1));
        assert!(e.edit_insert("Wert"));
        assert!(e.cell_edit_move(1, 0));
        assert_eq!(e.cell_edit.as_ref().map(|c| (c.row, c.col)), Some((1, 1)));
        assert!(e.edit_insert("40"));
        assert!(e.cell_edit_move(1, 0));
        assert!(e.edit_insert("=B2*2"));
        e.end_text_edit();
        let t = table(&e);
        assert_eq!(t.cells[0][..2], ["Name".to_string(), "Wert".to_string()]);
        assert_eq!(t.cells[2][1], "=B2*2");
        // je Zelle ein Schritt zum Rückgängigmachen (nicht je Taste; der erste Schritt
        // gehört zur ersten Zelle und zählt in `undo0` schon mit)
        assert_eq!(e.undo.len(), undo0 + 3);
        let texts = |e: &Editor| -> Vec<String> {
            e.doc
                .group_members(a)
                .iter()
                .filter_map(|id| e.doc.object(*id))
                .filter_map(|o| o.shape.text_fields().map(|t| t.0.to_string()))
                .filter(|t| !t.is_empty())
                .collect()
        };
        assert!(texts(&e).contains(&"80".to_string()), "{:?}", texts(&e));
        // beim Bearbeiten steht die Formel in der Zelle, sonst das Ergebnis
        let n = e.undo.len();
        assert!(e.begin_cell_edit(a, 2, 1));
        assert!(texts(&e).contains(&"=B2*2".to_string()));
        assert_eq!(e.cell_input().map(|c| c.3), Some("=B2*2".to_string()));
        // Esc: nichts geändert, kein Schritt
        assert!(e.edit_insert("9"));
        assert!(e.cancel_cell_edit());
        assert_eq!(e.undo.len(), n);
        assert_eq!(table(&e).cells[2][1], "=B2*2");
        assert!(texts(&e).contains(&"80".to_string()));
        // Tab hinter der letzten Zelle: neue Zeile
        assert!(e.begin_cell_edit(a, 2, 2));
        assert!(e.cell_edit_move(0, 1));
        assert_eq!(table(&e).n_rows(), 4);
        assert_eq!(e.cell_edit.as_ref().map(|c| (c.row, c.col)), Some((3, 0)));
        e.end_text_edit();
        // Diagramm aus der Tabelle: rechts daneben, folgt den Daten
        assert_eq!(e.single_smart(), Some(a));
        let c = e.create_chart_from_table(a).unwrap();
        let Some(Smart::Chart(ch)) = e.smart_data(c).cloned() else {
            panic!()
        };
        assert_eq!(ch.kind, ChartKind::Pie);
        assert_eq!(ch.data, table(&e).cells);
        assert!(e.set_cell_input(a, 1, 1, "55"));
        let Some(Smart::Chart(ch)) = e.smart_data(c).cloned() else {
            panic!()
        };
        assert_eq!(ch.data[1][1], "55");
        // Tabelle weg: das Diagramm behält seine Daten; daraus wieder eine Tabelle
        e.selection = e.doc.group_members(a);
        assert!(!e.ungroup_selection());
        e.delete_selection();
        assert!(e.smart_data(c).is_some());
        let t2 = e.table_from_chart(c).unwrap();
        let Some(Smart::Table(t2d)) = e.smart_data(t2).cloned() else {
            panic!()
        };
        assert_eq!(t2d.cells[1][1], "55");
        assert!(e.set_cell_input(t2, 1, 1, "70"));
        let Some(Smart::Chart(ch)) = e.smart_data(c).cloned() else {
            panic!()
        };
        assert_eq!(ch.data[1][1], "70");
        // Zeilen und Spalten: hinter der gewählten Zelle, der Rahmen wächst mit
        e.selection = e.doc.group_members(t2);
        e.cell_sel = Some((t2, 0, 0));
        let f0 = e.doc.object(t2).and_then(smart_frame).unwrap();
        assert!(e.table_op(TableOp::AddCol));
        let Some(Smart::Table(t3)) = e.smart_data(t2).cloned() else {
            panic!()
        };
        assert_eq!(t3.n_cols(), 4);
        assert_eq!(t3.cells[1][2], "70");
        let f1 = e.doc.object(t2).and_then(smart_frame).unwrap();
        assert!((f1.width() - f0.width() * 4.0 / 3.0).abs() < 1e-9);
        assert!(e.table_op(TableOp::RemoveCol));
        assert!(e.table_op(TableOp::RemoveRow));
        let Some(Smart::Table(t4)) = e.smart_data(t2).cloned() else {
            panic!()
        };
        assert_eq!((t4.n_rows(), t4.n_cols()), (t2d.n_rows() - 1, 3));
        // Doppelklick mit dem Auswahlwerkzeug in eine Zelle
        e.set_tool(Tool::Select);
        let Some(frame) = e.doc.object(t2).and_then(smart_frame) else {
            panic!()
        };
        let s = e.view.to_screen(Pt::new(frame.x0 + 5.0, frame.y0 + 5.0));
        e.double_click(s, &M);
        assert_eq!(
            e.cell_edit.as_ref().map(|c| (c.anchor, c.row, c.col)),
            Some((t2, 0, 0))
        );
    }

    #[test]
    fn rotated_charts_keep_their_parts_together() {
        use crate::smart::{Chart, ChartKind, Smart};
        let mut e = editor();
        let li = 0;
        let c = e.insert_smart_at(
            li,
            Smart::Chart(Chart::sample(ChartKind::Bar)),
            Rect::from_xywh(100.0, 100.0, 300.0, 200.0),
        );
        e.selection = e.doc.group_members(c);
        let boxes = |e: &Editor| -> Vec<Rect> {
            e.smart_members(c)
                .iter()
                .filter_map(|id| e.doc.object(*id).and_then(|o| o.shape.geom_bbox()))
                .collect()
        };
        let before = boxes(&e);
        assert!(e.set_rotation(30.0, &M));
        // alle Teile gedreht, der Winkel überall gleich
        for id in e.doc.group_members(c) {
            assert!((e.doc.object(id).unwrap().angle - 30.0).abs() < 1e-6);
        }
        let o = e.doc.object(c).unwrap();
        let fr = smart_frame(o).unwrap();
        assert!((fr.width() - 300.0).abs() < 1e-6 && (fr.height() - 200.0).abs() < 1e-6);
        // neu aufgebaut (z. B. weil sich die Tabelle ändert): bleibt gedreht
        e.rebuild_smart(c);
        let turned = boxes(&e);
        assert_eq!(turned.len(), before.len());
        assert_ne!(turned, before);
        // zurück auf 0: wie vorher
        e.selection = e.doc.group_members(c);
        assert!(e.set_rotation(0.0, &M));
        let back = boxes(&e);
        // (um die Mitte der Auswahl gedreht – die kann sich etwas verschieben)
        for (a, b) in back.iter().zip(&before) {
            assert!((a.width() - b.width()).abs() < 1e-6 && (a.height() - b.height()).abs() < 1e-6);
            assert!(((a.x0 - back[0].x0) - (b.x0 - before[0].x0)).abs() < 1e-6);
            assert!(((a.y0 - back[0].y0) - (b.y0 - before[0].y0)).abs() < 1e-6);
        }
        // Tabellen dreht man nicht
        let t = e.insert_smart_at(
            li,
            Smart::Table(crate::smart::Table::sample()),
            Rect::from_xywh(0.0, 400.0, 200.0, 100.0),
        );
        e.selection = e.doc.group_members(t);
        e.set_rotation(20.0, &M);
        let o = e.doc.object(t).unwrap();
        assert_eq!(o.angle, 0.0);
        assert!(matches!(o.shape, Shape::Rect { .. }));
    }

    #[test]
    fn table_cells_and_chart_parts_take_their_own_look() {
        use crate::smart::{ChartKind, Smart};
        let mut e = editor();
        e.set_tool(Tool::Data);
        drag(&mut e, (50.0, 50.0), (350.0, 170.0));
        let a = e.cell_edit.as_ref().unwrap().anchor;
        assert!(e.edit_insert("Name"));
        e.end_text_edit();
        e.set_tool(Tool::Select);
        let table = |e: &Editor| match e.smart_data(a) {
            Some(Smart::Table(t)) => t.clone(),
            _ => panic!(),
        };
        let frame = e.doc.object(a).and_then(smart_frame).unwrap();
        let at = |e: &Editor, r: usize, c: usize| {
            let (xs, ys) = table(e).grid(frame);
            e.view
                .to_screen(Pt::new((xs[c] + xs[c + 1]) / 2.0, (ys[r] + ys[r + 1]) / 2.0))
        };
        let click = |e: &mut Editor, s: Pt, shift: bool| {
            let m = Mods {
                shift,
                ..Mods::default()
            };
            e.press(s, m, false, &M);
            e.release(s, m, &M);
        };
        // Doppelklick wie im Fenster: zwei Drücke, der Doppelklick kommt nach bzw. vor dem
        // zweiten – beide Male wird getippt
        e.deselect();
        let s = at(&e, 1, 1);
        click(&mut e, s, false);
        e.press(s, Mods::default(), false, &M);
        e.double_click(s, &M);
        e.release(s, Mods::default(), &M);
        assert_eq!(e.cell_edit.as_ref().map(|c| (c.row, c.col)), Some((1, 1)));
        e.end_text_edit();
        e.deselect();
        let s = at(&e, 2, 0);
        click(&mut e, s, false);
        e.double_click(s, &M);
        e.press(s, Mods::default(), false, &M);
        e.release(s, Mods::default(), &M);
        assert_eq!(e.cell_edit.as_ref().map(|c| (c.row, c.col)), Some((2, 0)));
        assert!(e.edit_insert("x"));
        e.end_text_edit();
        assert_eq!(table(&e).cells[2][0], "x");
        // ganze Tabelle: Füllung für jede Zelle
        e.deselect();
        let q = at(&e, 0, 0);
        click(&mut e, q, false);
        assert_eq!(e.single_smart(), Some(a));
        assert_eq!(e.active_pick(), None);
        let red = Color::rgb(1.0, 0.0, 0.0);
        assert!(e.set_paint(PaintTarget::Fill, Some(red), None));
        let t = table(&e);
        assert!(t.looks(false).iter().flatten().all(|l| l.fill == Some(red)));
        // zweiter Klick: eine Zelle; Umschalttaste erweitert
        let q = at(&e, 1, 0);
        click(&mut e, q, false);
        assert_eq!(e.active_pick(), Some(Pick::Cells(1, 0, 1, 0)));
        let q = at(&e, 2, 1);
        click(&mut e, q, true);
        assert_eq!(e.active_pick().unwrap().cell_range(3, 3), Some((1, 0, 2, 1)));
        assert!(e.update_style("width", |st| st.width = 3.0));
        assert!(e.update_text("bold", |t| t.italic = true));
        let looks = table(&e).looks(false);
        assert!(looks[1][0].italic && looks[2][1].italic && !looks[0][0].italic && !looks[1][2].italic);
        assert_eq!(looks[2][1].width, 3.0);
        assert_eq!(looks[0][0].width, 1.0);
        // Textfarbe
        let blue = Color::rgb(0.0, 0.0, 1.0);
        assert!(e.set_smart_text_color(Some(blue)));
        assert_eq!(table(&e).looks(false)[2][0].color, blue);
        assert_eq!(e.context_info().smart_look.map(|l| l.color), Some(blue));
        // Spaltenkopf „C“: ganze Spalte
        let tl = e.view.to_screen(Pt::new(frame.x0, frame.y0));
        let head = Pt::new(at(&e, 0, 2).x, tl.y - HEAD_GAP - HEAD_BAND / 2.0);
        assert_eq!(e.table_head_at(head), Some((a, true, 2)));
        click(&mut e, head, false);
        assert_eq!(e.active_pick().unwrap().cell_range(3, 3), Some((0, 2, 2, 2)));
        // Zeilenkopf „3“
        let rh = Pt::new(tl.x - HEAD_GAP - HEAD_ROW_W / 2.0, at(&e, 2, 0).y);
        click(&mut e, rh, false);
        assert_eq!(e.active_pick().unwrap().cell_range(3, 3), Some((2, 0, 2, 2)));
        assert!(e.set_char_fmt("size", |f| f.size = 20.0));
        let looks = table(&e).looks(false);
        assert_eq!(looks[2][2].size, 20.0);
        assert_eq!(looks[1][2].size, 12.0);
        // gespeichert und wieder gelesen: gleiches Aussehen
        let back = Smart::decode(&Smart::Table(table(&e)).encode()).unwrap();
        let Smart::Table(bt) = back else { panic!() };
        assert_eq!(bt.looks(false), table(&e).looks(false));
        // Diagramm: Kopfzeile aus, dann zählt die erste Zeile mit
        assert!(e.create_chart_from_table(a).is_some());
        let c = e.single_smart().unwrap();
        let chart = |e: &Editor| match e.smart_data(c) {
            Some(Smart::Chart(ch)) => ch.clone(),
            _ => panic!(),
        };
        assert!(!chart(&e).header);
        let mut t = table(&e);
        t.header = true;
        e.set_smart(a, Smart::Table(t));
        assert!(chart(&e).header);
        // ein Stück anklicken (zweiter Klick) und einfärben
        let mut ch = chart(&e);
        ch.kind = ChartKind::Bar;
        ch.data = vec![vec!["A".into(), "4".into()], vec!["B".into(), "2".into()]];
        ch.source = 0;
        e.selection = e.doc.group_members(c);
        e.set_smart(c, Smart::Chart(ch));
        let bar = e
            .smart_roles(c)
            .into_iter()
            .find(|(_, r)| *r == crate::smart::Role::Point(0, 1))
            .map(|x| x.0)
            .unwrap();
        let bb = e.doc.object(bar).and_then(|o| o.shape.geom_bbox()).unwrap();
        let sp = e
            .view
            .to_screen(Pt::new((bb.x0 + bb.x1) / 2.0, (bb.y0 + bb.y1) / 2.0));
        click(&mut e, sp, false);
        assert_eq!(e.active_pick(), Some(Pick::Point(0, 1)));
        assert_eq!(e.picked_members(), vec![bar]);
        let green = Color::rgb(0.0, 1.0, 0.0);
        assert!(e.set_paint(PaintTarget::Fill, Some(green), None));
        assert_eq!(chart(&e).point_look(0, 1).fill, Some(green));
        assert_ne!(chart(&e).point_look(0, 0).fill, Some(green));
        // Schrift eines Diagrammteils gilt nicht für Balken
        assert!(!e.update_text("size", |t| t.size = 30.0));
        // ganzes Diagramm: Schrift für Beschriftungen und Werte, Fläche für den Rahmen
        e.pick = None;
        assert!(e.update_text("size", |t| t.size = 16.0));
        assert_eq!(chart(&e).labels_look().size, 16.0);
        assert!(e.set_paint(PaintTarget::Fill, Some(red), None));
        assert_eq!(e.doc.object(c).map(|o| o.style.fill), Some(Some(red)));
        // gewählte Zellen löschen: nur ihr Inhalt
        e.selection = e.doc.group_members(a);
        e.pick = Some((a, Pick::Cells(2, 0, 2, 0)));
        assert!(e.delete_selection());
        assert_eq!(table(&e).cells[2][0], "");
        assert!(e.doc.object(a).is_some());
        e.selection = e.doc.group_members(c);
        // in gewöhnliche Formen umwandeln
        let n = e.doc.group_members(c).len();
        assert!(e.release_smart());
        assert!(e.doc.object(c).is_some_and(|o| o.smart.is_none()));
        assert_eq!(e.selection.len(), n);
        e.selection = e.doc.group_members(a);
        assert!(e.release_smart());
        // der leere Rahmen der Tabelle ist weg, die Zellen bleiben
        assert!(e.doc.object(a).is_none());
        assert!(e.selection.len() >= 18);
        assert!(e.smart_target().is_none());
    }

    #[test]
    fn tables_select_by_dragging_resize_rows_and_move_by_the_grip() {
        use crate::smart::{ChartKind, Smart, TextKey};
        let mut e = editor();
        e.set_tool(Tool::Data);
        drag(&mut e, (100.0, 100.0), (400.0, 220.0));
        let a = e.cell_edit.as_ref().unwrap().anchor;
        e.end_text_edit();
        e.set_tool(Tool::Select);
        let table = |e: &Editor| match e.smart_data(a) {
            Some(Smart::Table(t)) => t.clone(),
            _ => panic!(),
        };
        let frame = |e: &Editor| e.doc.object(a).and_then(smart_frame).unwrap();
        let at = |e: &Editor, r: usize, c: usize| {
            let (xs, ys) = table(e).grid(frame(e));
            e.view
                .to_screen(Pt::new((xs[c] + xs[c + 1]) / 2.0, (ys[r] + ys[r + 1]) / 2.0))
        };
        let pull = |e: &mut Editor, a: Pt, b: Pt| {
            e.press(a, Mods::default(), false, &M);
            e.motion(b, Mods::default(), &M);
            e.release(b, Mods::default(), &M);
        };
        // erstes Ziehen an der (noch nicht gewählten) Tabelle verschiebt sie
        e.deselect();
        let f0 = frame(&e);
        let (p, q) = (at(&e, 1, 1), at(&e, 1, 1) + Pt::new(20.0, 0.0));
        pull(&mut e, p, q);
        let f1 = frame(&e);
        assert!((f1.x0 - f0.x0 - 20.0 / e.view.zoom).abs() < 1e-6);
        // jetzt gewählt: Ziehen wählt Zellen und verschiebt nichts
        let (p, q) = (at(&e, 0, 0), at(&e, 2, 1));
        pull(&mut e, p, q);
        assert_eq!(frame(&e), f1);
        assert_eq!(e.active_pick().unwrap().cell_range(3, 3), Some((0, 0, 2, 1)));
        // am Griff verschieben
        let tl = e.view.to_screen(Pt::new(f1.x0, f1.y0));
        let grip = tl - Pt::new(HEAD_GAP + HEAD_ROW_W / 2.0, HEAD_GAP + HEAD_BAND / 2.0);
        assert_eq!(e.table_grip_at(grip), Some(a));
        pull(&mut e, grip, grip + Pt::new(0.0, 30.0));
        let f2 = frame(&e);
        assert!((f2.y0 - f1.y0 - 30.0 / e.view.zoom).abs() < 1e-6);
        assert_eq!(f2.width(), f1.width());
        // Köpfe überstreichen: Spalten A bis C
        let tl = e.view.to_screen(Pt::new(f2.x0, f2.y0));
        let hy = tl.y - HEAD_GAP - HEAD_BAND / 2.0;
        let (p, q) = (Pt::new(at(&e, 0, 0).x, hy), Pt::new(at(&e, 0, 2).x + 300.0, hy));
        pull(&mut e, p, q);
        assert_eq!(e.active_pick().unwrap().cell_range(3, 3), Some((0, 0, 2, 2)));
        // Grenze zwischen A und B ziehen: alle gewählten Spalten werden gleich breit, die
        // Tabelle wächst
        let (xs, _) = table(&e).grid(frame(&e));
        let edge = Pt::new(e.view.to_screen(Pt::new(xs[1], 0.0)).x, hy);
        assert_eq!(e.table_edge_at(edge), Some((a, true, 0)));
        let w0 = xs[1] - xs[0];
        let undo = e.undo.len();
        let z = e.view.zoom;
        pull(&mut e, edge, edge + Pt::new(10.0 * z, 0.0));
        let (xs, _) = table(&e).grid(frame(&e));
        for c in 0..3 {
            assert!((xs[c + 1] - xs[c] - (w0 + 10.0)).abs() < 1e-6, "{xs:?}");
        }
        assert!((frame(&e).width() - 3.0 * (w0 + 10.0)).abs() < 1e-6);
        assert_eq!(e.undo.len(), undo + 1);
        // alle Zeilen gewählt (oben): alle gleich hoch; nur eine Zelle gewählt: die Höhe der
        // zweiten Zeile ändert nur sie
        e.pick = Some((a, Pick::Cells(1, 1, 1, 1)));
        let rh = Pt::new(tl.x - HEAD_GAP - HEAD_ROW_W / 2.0, 0.0);
        let (_, ys) = table(&e).grid(frame(&e));
        let h0 = ys[1] - ys[0];
        let edge = Pt::new(rh.x, e.view.to_screen(Pt::new(0.0, ys[2])).y);
        assert_eq!(e.table_edge_at(edge), Some((a, false, 1)));
        let z = e.view.zoom;
        pull(&mut e, edge, edge + Pt::new(0.0, 15.0 * z));
        let (_, ys) = table(&e).grid(frame(&e));
        assert!((ys[2] - ys[1] - (h0 + 15.0)).abs() < 1e-6);
        assert!((ys[1] - ys[0] - h0).abs() < 1e-6);
        // ganze Tabelle skalieren: Zeilen und Spalten wachsen im Verhältnis
        let mut t = table(&e);
        let f = frame(&e);
        if let Some(o) = e.doc.object_mut(a) {
            o.shape = Shape::Rect {
                x: f.x0,
                y: f.y0,
                w: f.width() * 2.0,
                h: f.height(),
                r: 0.0,
            };
        }
        let (xs2, _) = t.grid(frame(&e));
        assert!((xs2[1] - xs2[0] - 2.0 * (w0 + 10.0)).abs() < 1e-6);
        // Zeilen über und Spalten links der Wahl einfügen (so viele wie gewählt)
        t = table(&e);
        let (r0, c0) = (t.n_rows(), t.n_cols());
        e.pick = Some((a, Pick::Cells(1, 1, 2, 1)));
        assert!(e.table_op(TableOp::AddRowAbove));
        assert_eq!(table(&e).n_rows(), r0 + 2);
        assert_eq!(e.active_pick(), Some(Pick::Cells(1, 0, 2, c0 - 1)));
        e.pick = Some((a, Pick::Cells(0, 2, 0, 2)));
        assert!(e.table_op(TableOp::AddColLeft));
        assert_eq!(table(&e).n_cols(), c0 + 1);
        assert!(e.table_op(TableOp::RemoveCol));
        assert_eq!(table(&e).n_cols(), c0);
        // Diagramm: ganz gewählt ändert die Schrift alle Texte, ein Text für sich allein
        let c = e.create_chart_from_table(a).unwrap();
        let mut ch = match e.smart_data(c) {
            Some(Smart::Chart(ch)) => ch.clone(),
            _ => panic!(),
        };
        ch.kind = ChartKind::Bar;
        ch.source = 0;
        ch.data = vec![
            vec![String::new(), "x".into()],
            vec!["A".into(), "4".into()],
            vec!["B".into(), "2".into()],
        ];
        e.set_smart(c, Smart::Chart(ch));
        e.selection = e.doc.group_members(c);
        e.pick = None;
        let red = Color::rgb(1.0, 0.0, 0.0);
        assert!(e.set_smart_text_color(Some(red)));
        assert!(e.update_text("underline", |t| t.fmt.underline = true));
        let chart = |e: &Editor| match e.smart_data(c) {
            Some(Smart::Chart(ch)) => ch.clone(),
            _ => panic!(),
        };
        for k in [TextKey::Category(0), TextKey::Tick(1), TextKey::Value(0, 1)] {
            let l = chart(&e).text_look(k);
            assert!(l.color == red && l.underline, "{k:?}");
        }
        e.pick = Some((c, Pick::Text(TextKey::Category(1))));
        assert!(e.update_text("size", |t| t.size = 30.0));
        assert!(e.update_text("tracking", |t| t.fmt.tracking = 100.0));
        assert_eq!(chart(&e).text_look(TextKey::Category(1)).size, 30.0);
        assert_ne!(chart(&e).text_look(TextKey::Category(0)).size, 30.0);
        let parts = e.smart_roles(c);
        let big = parts
            .iter()
            .find(|(_, r)| *r == crate::smart::Role::Text(TextKey::Category(1)))
            .and_then(|(id, _)| e.doc.object(*id))
            .unwrap();
        assert!(
            matches!(&big.shape, Shape::TextArea { size, fmt, .. } if *size == 30.0 && fmt.tracking == 100.0)
        );
        // gespeichert und wieder gelesen
        let back = Smart::decode(&Smart::Chart(chart(&e)).encode()).unwrap();
        assert_eq!(back, Smart::Chart(chart(&e)));
        // ganzes Diagramm wieder: die eigene Größe des einen Textes weicht
        e.pick = None;
        assert!(e.update_text("size", |t| t.size = 14.0));
        assert_eq!(chart(&e).text_look(TextKey::Category(1)).size, 14.0 * 0.85);
    }

    #[test]
    fn picked_cells_fill_down_and_move_by_their_border() {
        use crate::smart::Smart;
        let mut e = editor();
        e.set_tool(Tool::Data);
        drag(&mut e, (100.0, 100.0), (400.0, 220.0));
        let a = e.cell_edit.as_ref().unwrap().anchor;
        assert!(e.edit_insert("Week 1"));
        e.end_text_edit();
        e.set_tool(Tool::Select);
        let table = |e: &Editor| match e.smart_data(a) {
            Some(Smart::Table(t)) => t.clone(),
            _ => panic!(),
        };
        // Anfasser und Drehgriff: bei gewählten Zellen keine, bei der Tabelle kein Drehgriff
        assert!(e.handle_positions(&M).is_none());
        e.pick = None;
        assert!(e.handle_positions(&M).is_some());
        assert!(e.rotate_handle(&M).is_none());
        e.pick = Some((a, Pick::Cells(0, 0, 0, 0)));
        assert!(e.handle_positions(&M).is_none());
        // Ausfüllkästchen zwei Zeilen über das Ende hinaus ziehen: die Tabelle wächst
        let h = e.fill_handle().unwrap();
        let f = e.doc.object(a).and_then(smart_frame).unwrap();
        let row = table(&e).grid(f).1[1] - f.y0;
        let to = e.view.to_screen(Pt::new(f.x0 + 5.0, f.y1 + row * 1.5));
        e.press(h, Mods::default(), false, &M);
        e.motion(to, Mods::default(), &M);
        e.release(to, Mods::default(), &M);
        let t = table(&e);
        assert_eq!(t.n_rows(), 5);
        let col: Vec<String> = t.cells.iter().map(|r| r[0].clone()).collect();
        assert_eq!(col, ["Week 1", "Week 2", "Week 3", "Week 4", "Week 5"]);
        assert_eq!(e.active_pick(), Some(Pick::Cells(0, 0, 4, 0)));
        let f2 = e.doc.object(a).and_then(smart_frame).unwrap();
        assert!((f2.height() - f.height() * 5.0 / 3.0).abs() < 1e-6);
        // am Rand verschieben: eine Spalte nach rechts
        e.pick = Some((a, Pick::Cells(1, 0, 2, 0)));
        let (xs, ys) = table(&e).grid(f2);
        let edge = e.view.to_screen(Pt::new(xs[0], (ys[1] + ys[2]) / 2.0));
        assert!(e.on_pick_border(edge));
        let to = e
            .view
            .to_screen(Pt::new((xs[1] + xs[2]) / 2.0, (ys[1] + ys[2]) / 2.0));
        e.press(edge, Mods::default(), false, &M);
        e.motion(to, Mods::default(), &M);
        e.release(to, Mods::default(), &M);
        let t = table(&e);
        assert_eq!(t.cells[1][1], "Week 2");
        assert_eq!(t.cells[2][1], "Week 3");
        assert_eq!(t.cells[1][0], "");
        assert_eq!(e.active_pick(), Some(Pick::Cells(1, 1, 2, 1)));
        assert_eq!(e.doc.object(a).and_then(smart_frame), Some(f2));
    }

    #[test]
    fn text_boxes_flow_around_tables_and_pictures_in_front() {
        use crate::smart::{Smart, Table};
        let mut e = editor();
        let li = 0;
        let text_area = |e: &Editor, id: u64| match e.doc.object(id).map(|o| &o.shape) {
            Some(Shape::TextArea { frame, .. }) => frame.clone(),
            _ => panic!(),
        };
        let id = e.insert_object(
            li,
            Shape::TextArea {
                xf: Affine::translate(100.0, 100.0),
                w: 300.0,
                h: 200.0,
                text: "Text".into(),
                family: "Sans".into(),
                size: 12.0,
                bold: false,
                italic: false,
                fmt: TextFormat::default(),
                frame: TextFrame {
                    inset: 8.0,
                    ..TextFrame::default()
                },
                rich: Default::default(),
            },
            Style::default(),
        );
        assert!(!e.update_flow());
        // Tabelle davor, rechts über den Rand hinaus: Aussparung im Feld, begrenzt
        let t = e.insert_smart_at(
            li,
            Smart::Table(Table::empty(2, 2)),
            Rect::from_xywh(300.0, 150.0, 200.0, 60.0),
        );
        assert!(e.update_flow());
        let f = text_area(&e, id);
        assert_eq!(f.holes.len(), 1);
        let hb = Shape::Path {
            subpaths: vec![SubPath {
                nodes: f.holes[0].iter().map(|p| Node::corner(*p)).collect(),
                closed: true,
            }],
            evenodd: false,
        }
        .geom_bbox()
        .unwrap();
        // mit dem Abstand des Wegdrängens rundherum (rechts auf das Feld begrenzt)
        let g = crate::model::WRAP_GAP;
        for (a, b) in [
            (hb.x0, 200.0 - g),
            (hb.y0, 50.0 - g),
            (hb.x1, 300.0),
            (hb.y1, 110.0 + g),
        ] {
            assert!((a - b).abs() < 0.6, "{hb:?}");
        }
        assert!(f.shaped());
        let subs = f.flow_path(300.0, 200.0).unwrap();
        assert_eq!(subs.len(), 2);
        // die Zellen der Tabelle selbst weichen nichts aus
        for m in e.doc.group_members(t) {
            if let Some(Shape::TextArea { frame, .. }) = e.doc.object(m).map(|o| &o.shape) {
                assert!(frame.holes.is_empty() && frame.no_flow);
            }
        }
        assert!(!e.update_flow());
        // ausgeschaltet: keine Aussparung; dahinter liegende Objekte zählen nicht
        if let Some(Shape::TextArea { frame, .. }) = e.doc.object_mut(id).map(|o| &mut o.shape) {
            frame.no_flow = true;
        }
        assert!(e.update_flow());
        assert!(text_area(&e, id).holes.is_empty());
        if let Some(Shape::TextArea { frame, .. }) = e.doc.object_mut(id).map(|o| &mut o.shape) {
            frame.no_flow = false;
        }
        // Tabelle nach hinten: weg aus dem Weg
        let members = e.doc.group_members(t);
        let l = &mut e.doc.layers[li].objects;
        let moved: Vec<Object> = l.iter().filter(|o| members.contains(&o.id)).cloned().collect();
        l.retain(|o| !members.contains(&o.id));
        for (k, o) in moved.into_iter().enumerate() {
            l.insert(k, o);
        }
        e.update_flow();
        assert!(text_area(&e, id).holes.is_empty());
        // eine Form drängt Text erst mit dem Effekt weg – dann in ihrer eigenen Form
        let circle = e.insert_object(
            li,
            Shape::Ellipse {
                cx: 250.0,
                cy: 200.0,
                rx: 40.0,
                ry: 40.0,
            },
            Style::default(),
        );
        e.update_flow();
        assert!(text_area(&e, id).holes.is_empty());
        e.selection = vec![circle];
        assert_eq!(e.wrap_state(), Some(false));
        assert!(e.set_wrap(true));
        assert_eq!(e.wrap_state(), Some(true));
        e.update_flow();
        let f = text_area(&e, id);
        assert_eq!(f.holes.len(), 1);
        assert!(f.holes[0].len() > 8, "rund, nicht eckig");
        // Abstand und Rechteck statt Umriss; gestrichelt zu sehen, solange gewählt
        let bbox_of = |e: &Editor| {
            let f = text_area(e, id);
            Shape::Path {
                subpaths: vec![SubPath {
                    nodes: f.holes[0].iter().map(|p| Node::corner(*p)).collect(),
                    closed: true,
                }],
                evenodd: false,
            }
            .geom_bbox()
            .unwrap()
        };
        let x_full = bbox_of(&e).x0;
        assert!(e.set_wrap_gap(4.0));
        assert!(e.context_info().wrap_gap == 4.0);
        e.update_flow();
        let x4 = bbox_of(&e).x0;
        assert!(
            (x4 - x_full - (crate::model::WRAP_GAP - 4.0)).abs() < 1.0,
            "{x_full} {x4}"
        );
        assert!(e.set_wrap_box(true));
        assert!(e.context_info().wrap_box);
        e.update_flow();
        assert_eq!(text_area(&e, id).holes[0].len(), 4);
        assert_eq!(e.wrap_preview().len(), 1);
        let fx = crate::model::Effects::from_attr(&e.doc.object(circle).unwrap().style.fx.to_attr());
        assert!(fx.wrap_box && fx.wrap_gap == Some(4.0));
        e.set_wrap_box(false);
        e.set_wrap_gap(crate::model::WRAP_GAP);
        // Tabelle ohne den Effekt
        e.selection = e.doc.group_members(t);
        e.set_wrap(true);
        assert!(e.set_wrap(false));
        assert_eq!(e.doc.object(t).unwrap().style.fx.wrap, Some(false));
        e.selection = vec![circle];
        e.set_wrap(false);
        assert_eq!(e.doc.object(circle).unwrap().style.fx.wrap, None);
        e.update_flow();
        assert!(text_area(&e, id).holes.is_empty());
        // gespeichert wird nur der Schalter
        if let Some(Shape::TextArea { frame, .. }) = e.doc.object_mut(id).map(|o| &mut o.shape) {
            frame.no_flow = true;
        }
        let back = crate::svg::read(&crate::svg::write(&e.doc, false)).unwrap();
        let found = back.layers.iter().flat_map(|l| &l.objects).any(
            |o| matches!(&o.shape, Shape::TextArea { frame, text, .. } if text == "Text" && frame.no_flow),
        );
        assert!(found);
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
        // ohne Auswahl bleibt nichts ausgewählt (das Werkzeug arbeitet mit allen Formen)
        assert!(e.selection.is_empty());
        let newest = e.doc.layers[0].objects.iter().max_by_key(|o| o.id).unwrap();
        let b = newest.shape.geom_bbox().unwrap();
        assert!((b.x0 - 200.0).abs() < 2.0 && (b.x1 - 300.0).abs() < 2.0, "{b:?}");
        // außerhalb: nichts
        e.press(Pt::new(20.0, 20.0), Mods::default(), false, &M);
        e.release(Pt::new(20.0, 20.0), Mods::default(), &M);
        assert_eq!(e.doc.layers[0].objects.len(), 3);
        // gleich weiter: die linke Sichel ist immer noch zu finden
        e.motion(Pt::new(130.0, 200.0), Mods::default(), &M);
        assert!(!e.builder_marks().0.is_empty());
        e.press(Pt::new(130.0, 200.0), Mods::default(), false, &M);
        e.release(Pt::new(130.0, 200.0), Mods::default(), &M);
        assert_eq!(e.doc.layers[0].objects.len(), 4);
    }

    #[test]
    fn shape_builder_sees_nudged_shapes() {
        // mehrmals mit den Pfeiltasten verschoben (eine Rückgängig-Stufe): das Werkzeug muss
        // die neue Lage kennen
        let mut e = editor();
        e.viewport = (500.0, 500.0);
        e.set_tool(Tool::Shape);
        drag(&mut e, (100.0, 100.0), (200.0, 200.0));
        e.deselect();
        e.set_tool(Tool::Builder);
        e.motion(Pt::new(150.0, 150.0), Mods::default(), &M);
        assert!(!e.builder_marks().0.is_empty());
        e.select_all();
        e.nudge(10.0, 0.0);
        e.deselect();
        // Graph nach dem ersten Schritt
        e.motion(Pt::new(150.0, 150.0), Mods::default(), &M);
        assert!(!e.builder_marks().0.is_empty());
        e.select_all();
        for _ in 0..19 {
            e.nudge(10.0, 0.0);
        }
        e.deselect();
        e.motion(Pt::new(150.0, 150.0), Mods::default(), &M);
        assert!(e.builder_marks().0.is_empty());
        e.motion(Pt::new(350.0, 150.0), Mods::default(), &M);
        assert!(!e.builder_marks().0.is_empty());
    }

    #[test]
    fn shape_builder_keeps_working_with_a_selection() {
        // mit Auswahl: nach dem ersten Klick kennt das Werkzeug weiterhin alle ausgewählten
        // Formen (früher nur noch die neue)
        let mut e = editor();
        e.viewport = (500.0, 500.0);
        e.set_tool(Tool::Shape);
        e.shape_kind = ShapeKind::Ellipse;
        drag(&mut e, (100.0, 100.0), (300.0, 300.0));
        e.deselect();
        drag(&mut e, (200.0, 100.0), (400.0, 300.0));
        e.select_all();
        e.set_tool(Tool::Builder);
        e.press(Pt::new(250.0, 200.0), Mods::default(), false, &M);
        e.release(Pt::new(250.0, 200.0), Mods::default(), &M);
        assert_eq!(e.selection.len(), 3);
        for p in [Pt::new(130.0, 200.0), Pt::new(370.0, 200.0)] {
            e.motion(p, Mods::default(), &M);
            assert!(!e.builder_marks().0.is_empty(), "{p:?}");
        }
    }
}
