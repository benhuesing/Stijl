// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! Kontur in Fläche: die Fläche, die eine Kontur bedeckt – mit denselben Ecken (spitz,
//! rund, abgeschrägt), Enden (gerade, rund, überstehend) und Strichmustern, wie Cairo sie
//! zeichnet. Die Kontur wird in konvexe Stücke zerlegt (je Abschnitt ein Viereck, dazu
//! Ecken und Enden), die anschließend vereinigt werden.

use crate::boolean::{self, Op, Ring};
use crate::geom::Pt;
use crate::model::{LineCap, LineJoin, SubPath};

/// Wie Cairo: spitze Ecken bis zum Zehnfachen der halben Linienbreite, sonst abgeschrägt.
pub const MITER_LIMIT: f64 = 10.0;

/// Darstellung einer Kontur.
#[derive(Clone, Debug)]
pub struct StrokeStyle {
    pub width: f64,
    pub join: LineJoin,
    pub cap: LineCap,
    /// Strichmuster (Längen abwechselnd Strich/Lücke), leer = durchgezogen
    pub dash: Vec<f64>,
}

/// Punkt einer abgeflachten Linie; `corner`: Knoten des Pfades (dort gilt die Eckenform).
#[derive(Clone, Copy, Debug)]
struct Vtx {
    p: Pt,
    corner: bool,
}

fn cross(a: Pt, b: Pt) -> f64 {
    a.x * b.y - a.y * b.x
}

fn unit(v: Pt) -> Pt {
    let l = v.len();
    if l < 1e-12 {
        Pt::new(0.0, 0.0)
    } else {
        Pt::new(v.x / l, v.y / l)
    }
}

fn perp(d: Pt) -> Pt {
    Pt::new(-d.y, d.x)
}

fn flatten_cubic(p0: Pt, c1: Pt, c2: Pt, p3: Pt, tol: f64, depth: u32, out: &mut Vec<Vtx>) {
    let chord = p3 - p0;
    let len = chord.len();
    let flat = if len < 1e-12 {
        c1.dist(p0).max(c2.dist(p0))
    } else {
        (cross(c1 - p0, chord).abs() / len).max(cross(c2 - p0, chord).abs() / len)
    };
    if flat <= tol || depth >= 14 {
        out.push(Vtx { p: p3, corner: false });
        return;
    }
    let p01 = p0.lerp(c1, 0.5);
    let p12 = c1.lerp(c2, 0.5);
    let p23 = c2.lerp(p3, 0.5);
    let a = p01.lerp(p12, 0.5);
    let b = p12.lerp(p23, 0.5);
    let m = a.lerp(b, 0.5);
    flatten_cubic(p0, p01, a, m, tol, depth + 1, out);
    flatten_cubic(m, b, p23, p3, tol, depth + 1, out);
}

/// Teilpfad als Linienzug (Knoten als Ecken markiert). Doppelte Punkte entfallen.
fn polyline(sp: &SubPath, tol: f64) -> Vec<Vtx> {
    let mut out: Vec<Vtx> = Vec::new();
    for (p0, c1, c2, p3) in sp.segments() {
        if out.is_empty() {
            out.push(Vtx { p: p0, corner: true });
        }
        flatten_cubic(p0, c1, c2, p3, tol, 0, &mut out);
        if let Some(l) = out.last_mut() {
            l.corner = true;
        }
    }
    if out.is_empty() {
        if let Some(n) = sp.nodes.first() {
            out.push(Vtx { p: n.p, corner: true });
        }
    }
    let mut clean: Vec<Vtx> = Vec::with_capacity(out.len());
    for v in out {
        match clean.last_mut() {
            Some(l) if l.p.dist(v.p) < 1e-9 => l.corner |= v.corner,
            _ => clean.push(v),
        }
    }
    if sp.closed && clean.len() > 1 && clean[0].p.dist(clean[clean.len() - 1].p) < 1e-9 {
        clean.pop();
    }
    clean
}

/// Linienzug nach dem Strichmuster aufteilen. Liefert offene Stücke; ein Stück aus einem
/// einzigen Punkt (Strichlänge 0) behält seine Richtung für die Enden.
fn dash_pieces(pts: &[Vtx], closed: bool, pattern: &[f64]) -> Vec<(Vec<Vtx>, Pt)> {
    let mut path: Vec<Vtx> = pts.to_vec();
    if closed {
        if let Some(f) = pts.first() {
            path.push(*f);
        }
    }
    let total: f64 = pattern.iter().sum();
    if pattern.is_empty() || total <= 1e-9 || path.len() < 2 {
        return vec![(path, Pt::new(1.0, 0.0))];
    }
    let mut out: Vec<(Vec<Vtx>, Pt)> = Vec::new();
    let mut idx = 0usize;
    let mut left = pattern[0];
    let mut on = true;
    let mut cur: Vec<Vtx> = vec![path[0]];
    let mut dir = Pt::new(1.0, 0.0);
    for w in path.windows(2) {
        let (a, b) = (w[0].p, w[1].p);
        let len = a.dist(b);
        if len < 1e-12 {
            continue;
        }
        dir = unit(b - a);
        let mut pos = 0.0;
        while len - pos > left {
            pos += left;
            let q = Vtx {
                p: a + dir * pos,
                corner: false,
            };
            if on {
                cur.push(q);
                out.push((std::mem::take(&mut cur), dir));
            } else {
                cur = vec![q];
            }
            on = !on;
            idx = (idx + 1) % pattern.len();
            left = pattern[idx];
        }
        left -= len - pos;
        if on {
            cur.push(w[1]);
        }
    }
    if on && !cur.is_empty() {
        out.push((cur, dir));
    } else if !on && left <= 1e-9 && !closed {
        // ein Strich beginnt genau am Ende (z. B. der letzte Punkt einer gepunkteten Linie)
        if let Some(l) = path.last() {
            out.push((vec![*l], dir));
        }
    }
    out
}

fn circle(c: Pt, r: f64) -> Ring {
    let n = ((r * 2.0).ceil() as usize).clamp(16, 96);
    (0..n)
        .map(|i| {
            let t = i as f64 / n as f64 * std::f64::consts::TAU;
            Pt::new(c.x + r * t.cos(), c.y + r * t.sin())
        })
        .collect()
}

/// Konvexe Stücke einer Kontur entlang eines Linienzugs.
fn pieces(pts: &[Vtx], closed: bool, st: &StrokeStyle, zero_dir: Pt, out: &mut Vec<Ring>) {
    let h = st.width / 2.0;
    if pts.len() == 1 || (pts.len() == 2 && pts[0].p.dist(pts[1].p) < 1e-9) {
        // Punkt (Strichlänge 0): nur die Enden zeichnen etwas
        let c = pts[0].p;
        match st.cap {
            LineCap::Round => out.push(circle(c, h)),
            LineCap::Square => {
                let d = unit(zero_dir);
                let (d, n) = (d * h, perp(d) * h);
                out.push(vec![c - d + n, c + d + n, c + d - n, c - d - n]);
            }
            LineCap::Butt => {}
        }
        return;
    }
    let n = pts.len();
    let segs = if closed { n } else { n - 1 };
    for i in 0..segs {
        let a = pts[i].p;
        let b = pts[(i + 1) % n].p;
        let d = unit(b - a);
        if d.len() < 0.5 {
            continue;
        }
        let nn = perp(d) * h;
        out.push(vec![a + nn, b + nn, b - nn, a - nn]);
    }
    // Ecken
    let joins: Vec<usize> = if closed {
        (0..n).collect()
    } else {
        (1..n - 1).collect()
    };
    for i in joins {
        let v = pts[i].p;
        let prev = pts[(i + n - 1) % n].p;
        let next = pts[(i + 1) % n].p;
        let d0 = unit(v - prev);
        let d1 = unit(next - v);
        if d0.len() < 0.5 || d1.len() < 0.5 {
            continue;
        }
        let turn = cross(d0, d1);
        if turn.abs() < 1e-12 && d0.x * d1.x + d0.y * d1.y > 0.0 {
            continue;
        }
        // außen liegt die Seite, von der die Linie wegknickt
        let s = if turn > 0.0 { -1.0 } else { 1.0 };
        let (p0, p1) = (v + perp(d0) * (h * s), v + perp(d1) * (h * s));
        let join = if pts[i].corner { st.join } else { LineJoin::Bevel };
        match join {
            LineJoin::Round => out.push(circle(v, h)),
            LineJoin::Bevel => out.push(vec![v, p0, p1]),
            LineJoin::Miter => {
                // Schnittpunkt der beiden äußeren Kanten
                let den = cross(d0, d1);
                let miter = if den.abs() > 1e-12 {
                    let t = cross(p1 - p0, d1) / den;
                    Some(p0 + d0 * t)
                } else {
                    None
                };
                match miter.filter(|m| m.dist(v) <= MITER_LIMIT * h) {
                    Some(m) => out.push(vec![v, p0, m, p1]),
                    None => out.push(vec![v, p0, p1]),
                }
            }
        }
    }
    // Enden
    if !closed {
        let ends = [
            (pts[0].p, unit(pts[0].p - pts[1].p)),
            (pts[n - 1].p, unit(pts[n - 1].p - pts[n - 2].p)),
        ];
        for (c, d) in ends {
            match st.cap {
                LineCap::Round => out.push(circle(c, h)),
                LineCap::Square => {
                    let nn = perp(d) * h;
                    out.push(vec![c + nn, c + nn + d * h, c - nn + d * h, c - nn]);
                }
                LineCap::Butt => {}
            }
        }
    }
}

/// Stücke paarweise vereinigen (schneller als eines nach dem anderen).
pub fn union_all(mut parts: Vec<Vec<Ring>>) -> Vec<Ring> {
    parts.retain(|p| !p.is_empty());
    while parts.len() > 1 {
        let mut next = Vec::with_capacity(parts.len() / 2 + 1);
        let mut it = parts.into_iter();
        while let Some(a) = it.next() {
            match it.next() {
                Some(b) => next.push(boolean::rings_op(&a, &b, Op::Union)),
                None => next.push(a),
            }
        }
        parts = next;
    }
    parts.pop().unwrap_or_default()
}

/// Fläche der Kontur um die Teilpfade (mittig auf der Linie).
pub fn stroke_area(subpaths: &[SubPath], st: &StrokeStyle) -> Vec<Ring> {
    if st.width <= 0.0 {
        return Vec::new();
    }
    let tol = (st.width / 200.0).clamp(0.01, 0.2);
    let mut parts: Vec<Vec<Ring>> = Vec::new();
    for sp in subpaths {
        let pts = polyline(sp, tol);
        if pts.is_empty() {
            continue;
        }
        let dashed = !st.dash.is_empty() && st.dash.iter().sum::<f64>() > 1e-9;
        if dashed {
            for (piece, dir) in dash_pieces(&pts, sp.closed, &st.dash) {
                let mut rings = Vec::new();
                pieces(&piece, false, st, dir, &mut rings);
                parts.extend(rings.into_iter().map(|r| boolean::rings_op(&[r], &[], Op::Union)));
            }
        } else {
            let mut rings = Vec::new();
            pieces(
                &pts,
                sp.closed && pts.len() > 2,
                st,
                Pt::new(1.0, 0.0),
                &mut rings,
            );
            parts.extend(rings.into_iter().map(|r| boolean::rings_op(&[r], &[], Op::Union)));
        }
    }
    union_all(parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Node;

    fn area(rings: &[Ring]) -> f64 {
        rings
            .iter()
            .map(|r| {
                let mut a = 0.0;
                for i in 0..r.len() {
                    let (p, q) = (r[i], r[(i + 1) % r.len()]);
                    a += p.x * q.y - q.x * p.y;
                }
                a / 2.0
            })
            .sum::<f64>()
            .abs()
    }

    fn line(pts: &[(f64, f64)], closed: bool) -> SubPath {
        SubPath {
            nodes: pts.iter().map(|&(x, y)| Node::corner(Pt::new(x, y))).collect(),
            closed,
        }
    }

    fn st(join: LineJoin, cap: LineCap) -> StrokeStyle {
        StrokeStyle {
            width: 10.0,
            join,
            cap,
            dash: Vec::new(),
        }
    }

    #[test]
    fn caps_change_the_length() {
        let l = [line(&[(0.0, 0.0), (100.0, 0.0)], false)];
        let butt = area(&stroke_area(&l, &st(LineJoin::Miter, LineCap::Butt)));
        let square = area(&stroke_area(&l, &st(LineJoin::Miter, LineCap::Square)));
        let round = area(&stroke_area(&l, &st(LineJoin::Miter, LineCap::Round)));
        assert!((butt - 1000.0).abs() < 1.0, "{butt}");
        assert!((square - 1100.0).abs() < 1.0, "{square}");
        let disc = std::f64::consts::PI * 25.0;
        assert!((round - (1000.0 + disc)).abs() < 3.0, "{round}");
    }

    #[test]
    fn corners_follow_the_join() {
        // Quadrat 100×100, Linie 10 breit: spitz = volle Ecken, abgeschrägt etwas weniger
        let sq = [line(
            &[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)],
            true,
        )];
        let miter = area(&stroke_area(&sq, &st(LineJoin::Miter, LineCap::Butt)));
        let bevel = area(&stroke_area(&sq, &st(LineJoin::Bevel, LineCap::Butt)));
        let round = area(&stroke_area(&sq, &st(LineJoin::Round, LineCap::Butt)));
        assert!((miter - (110.0 * 110.0 - 90.0 * 90.0)).abs() < 1.0, "{miter}");
        // je Ecke fehlt ein Dreieck 5×5/2
        assert!((bevel - (miter - 4.0 * 12.5)).abs() < 1.0, "{bevel}");
        assert!(round > bevel && round < miter, "{round}");
    }

    #[test]
    fn dashes_split_the_line() {
        let l = [line(&[(0.0, 0.0), (100.0, 0.0)], false)];
        let mut s = st(LineJoin::Miter, LineCap::Butt);
        s.dash = vec![30.0, 20.0];
        let rings = stroke_area(&l, &s);
        // Striche 0–30 und 50–80 und ein Rest 100 ist frei: zwei Stücke
        assert_eq!(rings.len(), 2, "{rings:?}");
        assert!((area(&rings) - 600.0).abs() < 1.0);
        // Punkte (Länge 0) mit runden Enden
        s.dash = vec![0.0, 20.0];
        s.cap = LineCap::Round;
        let dots = stroke_area(&l, &s);
        assert_eq!(dots.len(), 6, "{}", dots.len());
    }
}
