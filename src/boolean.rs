//! Boolesche Pfadoperationen (Vereinigen, Abziehen, Schnittmenge, Ausschließen).
//!
//! Die Pfade werden zu Polygonen abgeflacht, an allen Schnittpunkten geteilt; jedes
//! Teilstück wird danach eingeordnet, ob es innerhalb, außerhalb oder auf dem Rand der
//! anderen Form liegt. Die passenden Stücke werden wieder zu Ringen verkettet.

use std::collections::HashMap;

use crate::geom::Pt;
use crate::model::{Node, SubPath};

pub type Ring = Vec<Pt>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Union,
    Difference,
    Intersection,
    Xor,
}

const EPS: f64 = 1e-9;

fn cross(a: Pt, b: Pt) -> f64 {
    a.x * b.y - a.y * b.x
}

fn dot(a: Pt, b: Pt) -> f64 {
    a.x * b.x + a.y * b.y
}

// ───────────── Abflachen ─────────────

fn flatten_cubic(p0: Pt, c1: Pt, c2: Pt, p3: Pt, tol: f64, depth: u32, out: &mut Vec<Pt>) {
    let chord = p3 - p0;
    let len = chord.len();
    let flat = if len < EPS {
        c1.dist(p0).max(c2.dist(p0))
    } else {
        (cross(c1 - p0, chord).abs() / len).max(cross(c2 - p0, chord).abs() / len)
    };
    if flat <= tol || depth >= 12 {
        out.push(p3);
        return;
    }
    // De-Casteljau-Teilung bei t = 0,5
    let p01 = p0.lerp(c1, 0.5);
    let p12 = c1.lerp(c2, 0.5);
    let p23 = c2.lerp(p3, 0.5);
    let a = p01.lerp(p12, 0.5);
    let b = p12.lerp(p23, 0.5);
    let m = a.lerp(b, 0.5);
    flatten_cubic(p0, p01, a, m, tol, depth + 1, out);
    flatten_cubic(m, b, p23, p3, tol, depth + 1, out);
}

/// Teilpfade zu geschlossenen Polygonen abflachen (offene Pfade werden geschlossen).
pub fn flatten(subpaths: &[SubPath], tol: f64) -> Vec<Ring> {
    let mut rings = Vec::new();
    for sp in subpaths {
        if sp.nodes.len() < 2 {
            continue;
        }
        let mut pts = vec![sp.nodes[0].p];
        let mut segs = sp.segments();
        if !sp.closed {
            // Schlusssegment als gerade Linie
            let a = sp.nodes[sp.nodes.len() - 1].p;
            let b = sp.nodes[0].p;
            segs.push((a, a, b, b));
        }
        for (p0, c1, c2, p3) in segs {
            if c1.dist(p0) < EPS && c2.dist(p3) < EPS {
                pts.push(p3);
            } else {
                flatten_cubic(p0, c1, c2, p3, tol, 0, &mut pts);
            }
        }
        let ring = cleanup(pts);
        if ring.len() >= 3 && area(&ring).abs() > 1e-9 {
            rings.push(ring);
        }
    }
    rings
}

/// Entfernt doppelte und kollineare Punkte; der Ring ist implizit geschlossen.
fn cleanup(mut pts: Vec<Pt>) -> Ring {
    pts.dedup_by(|a, b| a.dist(*b) < 1e-7);
    while pts.len() > 1 && pts[0].dist(pts[pts.len() - 1]) < 1e-7 {
        pts.pop();
    }
    let mut changed = true;
    while changed && pts.len() >= 3 {
        changed = false;
        let n = pts.len();
        let mut keep = Vec::with_capacity(n);
        for i in 0..n {
            let prev = pts[(i + n - 1) % n];
            let cur = pts[i];
            let next = pts[(i + 1) % n];
            let a = cur - prev;
            let b = next - cur;
            let collinear = cross(a, b).abs() <= 1e-9 * (a.len() * b.len()).max(1e-12) && dot(a, b) >= 0.0;
            if collinear {
                changed = true;
            } else {
                keep.push(cur);
            }
        }
        if changed {
            pts = keep;
        }
    }
    pts
}

pub fn area(r: &[Pt]) -> f64 {
    let n = r.len();
    let mut s = 0.0;
    for i in 0..n {
        let a = r[i];
        let b = r[(i + 1) % n];
        s += a.x * b.y - b.x * a.y;
    }
    s / 2.0
}

/// Windungszahl der Ringe um `p`.
fn winding(rings: &[Ring], p: Pt) -> i32 {
    let mut w = 0;
    for r in rings {
        let n = r.len();
        for i in 0..n {
            let a = r[i];
            let b = r[(i + 1) % n];
            if a.y <= p.y {
                if b.y > p.y && cross(b - a, p - a) > 0.0 {
                    w += 1;
                }
            } else if b.y <= p.y && cross(b - a, p - a) < 0.0 {
                w -= 1;
            }
        }
    }
    w
}

/// Ringe so ausrichten, dass Außenkonturen positive und Löcher negative Fläche haben
/// (Verschachtelungstiefe gerade/ungerade).
fn normalize(rings: Vec<Ring>) -> Vec<Ring> {
    let n = rings.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let probe = rings[i][0];
        let mut depth = 0;
        for (j, other) in rings.iter().enumerate() {
            if i != j && winding(std::slice::from_ref(other), probe) != 0 {
                depth += 1;
            }
        }
        let mut r = rings[i].clone();
        let want_positive = depth % 2 == 0;
        if (area(&r) > 0.0) != want_positive {
            r.reverse();
        }
        out.push(r);
    }
    out
}

// ───────────── Schnittpunkte ─────────────

struct Edge {
    a: Pt,
    b: Pt,
}

fn edges(rings: &[Ring]) -> Vec<Edge> {
    let mut v = Vec::new();
    for r in rings {
        let n = r.len();
        for i in 0..n {
            v.push(Edge {
                a: r[i],
                b: r[(i + 1) % n],
            });
        }
    }
    v
}

fn bbox_overlap(e: &Edge, f: &Edge) -> bool {
    e.a.x.min(e.b.x) <= f.a.x.max(f.b.x) + EPS
        && f.a.x.min(f.b.x) <= e.a.x.max(e.b.x) + EPS
        && e.a.y.min(e.b.y) <= f.a.y.max(f.b.y) + EPS
        && f.a.y.min(f.b.y) <= e.a.y.max(e.b.y) + EPS
}

/// Parameter `t` von `p` auf der Strecke a→b (Projektion).
fn param(a: Pt, b: Pt, p: Pt) -> f64 {
    let d = b - a;
    let l2 = dot(d, d);
    if l2 < EPS * EPS {
        0.0
    } else {
        dot(p - a, d) / l2
    }
}

/// Teilungsstellen für alle Kanten von A und B berechnen.
fn split_points(ea: &[Edge], eb: &[Edge]) -> (Vec<Vec<(f64, Pt)>>, Vec<Vec<(f64, Pt)>>) {
    let mut sa: Vec<Vec<(f64, Pt)>> = ea.iter().map(|e| vec![(0.0, e.a), (1.0, e.b)]).collect();
    let mut sb: Vec<Vec<(f64, Pt)>> = eb.iter().map(|e| vec![(0.0, e.a), (1.0, e.b)]).collect();
    let te = 1e-9;
    for (i, e) in ea.iter().enumerate() {
        for (j, f) in eb.iter().enumerate() {
            if !bbox_overlap(e, f) {
                continue;
            }
            let r = e.b - e.a;
            let s = f.b - f.a;
            let d = cross(r, s);
            let scale = r.len() * s.len();
            if scale < EPS {
                continue;
            }
            if d.abs() > 1e-12 * scale {
                let q = f.a - e.a;
                let t = cross(q, s) / d;
                let u = cross(q, r) / d;
                if t < -te || t > 1.0 + te || u < -te || u > 1.0 + te {
                    continue;
                }
                // Treffer auf vorhandenen Eckpunkten exakt übernehmen
                let p = if t <= te {
                    e.a
                } else if t >= 1.0 - te {
                    e.b
                } else if u <= te {
                    f.a
                } else if u >= 1.0 - te {
                    f.b
                } else {
                    e.a + r * t
                };
                sa[i].push((t.clamp(0.0, 1.0), p));
                sb[j].push((u.clamp(0.0, 1.0), p));
            } else if cross(f.a - e.a, r).abs() <= 1e-9 * r.len().max(1.0) {
                // kollinear: Endpunkte der jeweils anderen Kante einfügen
                for p in [f.a, f.b] {
                    let t = param(e.a, e.b, p);
                    if t > te && t < 1.0 - te {
                        sa[i].push((t, p));
                    }
                }
                for p in [e.a, e.b] {
                    let u = param(f.a, f.b, p);
                    if u > te && u < 1.0 - te {
                        sb[j].push((u, p));
                    }
                }
            }
        }
    }
    (sa, sb)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    Inside,
    Outside,
    SharedSame,
    SharedOpposite,
}

fn pieces(edges: &[Edge], splits: Vec<Vec<(f64, Pt)>>) -> Vec<(Pt, Pt)> {
    let mut out = Vec::new();
    for (_e, mut sp) in edges.iter().zip(splits) {
        sp.sort_by(|a, b| a.0.total_cmp(&b.0));
        for w in sp.windows(2) {
            let (p, q) = (w[0].1, w[1].1);
            if p.dist(q) > 1e-9 {
                out.push((p, q));
            }
        }
    }
    out
}

fn classify(piece: (Pt, Pt), other_edges: &[Edge], other: &[Ring]) -> Class {
    let (p, q) = piece;
    let mid = p.lerp(q, 0.5);
    let dir = q - p;
    for f in other_edges {
        let fd = f.b - f.a;
        let len = fd.len();
        if len < EPS {
            continue;
        }
        // Liegt die Mitte auf dieser Kante und ist das Stück parallel dazu?
        if (cross(mid - f.a, fd) / len).abs() < 1e-7 {
            let t = param(f.a, f.b, mid);
            if t > -1e-9 && t < 1.0 + 1e-9 && (cross(dir, fd) / (len * dir.len().max(EPS))).abs() < 1e-6 {
                return if dot(dir, fd) > 0.0 {
                    Class::SharedSame
                } else {
                    Class::SharedOpposite
                };
            }
        }
    }
    if winding(other, mid) != 0 {
        Class::Inside
    } else {
        Class::Outside
    }
}

fn key(p: Pt) -> (i64, i64) {
    ((p.x * 1e6).round() as i64, (p.y * 1e6).round() as i64)
}

/// Gerichtete Stücke zu geschlossenen Ringen verketten.
fn chain(pieces: Vec<(Pt, Pt)>) -> Vec<Ring> {
    let mut by_start: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, (p, _)) in pieces.iter().enumerate() {
        by_start.entry(key(*p)).or_default().push(i);
    }
    let mut used = vec![false; pieces.len()];
    let mut rings = Vec::new();
    for start in 0..pieces.len() {
        if used[start] {
            continue;
        }
        let start_key = key(pieces[start].0);
        let mut ring = vec![pieces[start].0];
        let mut cur = start;
        loop {
            used[cur] = true;
            let end = pieces[cur].1;
            if key(end) == start_key {
                break;
            }
            ring.push(end);
            let next = by_start
                .get(&key(end))
                .and_then(|v| v.iter().copied().find(|&k| !used[k]));
            match next {
                Some(n) => cur = n,
                None => break, // offen geblieben (sollte nicht vorkommen)
            }
        }
        let ring = cleanup(ring);
        if ring.len() >= 3 && area(&ring).abs() > 1e-6 {
            rings.push(ring);
        }
    }
    rings
}

fn boolean_rings(a: &[Ring], b: &[Ring], op: Op) -> Vec<Ring> {
    if op == Op::Xor {
        let mut r = boolean_rings(a, b, Op::Difference);
        r.extend(boolean_rings(b, a, Op::Difference));
        return r;
    }
    let ea = edges(a);
    let eb = edges(b);
    let (sa, sb) = split_points(&ea, &eb);
    let pa = pieces(&ea, sa);
    let pb = pieces(&eb, sb);
    let mut keep = Vec::new();
    for p in pa {
        let c = classify(p, &eb, b);
        let k = match op {
            Op::Union => matches!(c, Class::Outside | Class::SharedSame),
            Op::Intersection => matches!(c, Class::Inside | Class::SharedSame),
            Op::Difference => matches!(c, Class::Outside | Class::SharedOpposite),
            Op::Xor => unreachable!(),
        };
        if k {
            keep.push(p);
        }
    }
    for p in pb {
        let c = classify(p, &ea, a);
        match op {
            Op::Union if c == Class::Outside => keep.push(p),
            Op::Intersection if c == Class::Inside => keep.push(p),
            Op::Difference if c == Class::Inside => keep.push((p.1, p.0)),
            _ => {}
        }
    }
    chain(keep)
}

/// Teilpfade als ausgerichtete Ringe (Außenkonturen positiv, Löcher negativ).
pub fn to_rings(subpaths: &[SubPath], tol: f64) -> Vec<Ring> {
    normalize(flatten(subpaths, tol))
}

/// Boolesche Operation direkt auf Ringen; das Ergebnis ist wieder ausgerichtet.
pub fn rings_op(a: &[Ring], b: &[Ring], op: Op) -> Vec<Ring> {
    let r: Vec<Ring> = boolean_rings(a, b, op)
        .into_iter()
        .filter(|r| r.len() >= 3)
        .collect();
    normalize(r)
}

/// Liegt `p` innerhalb der (ausgerichteten) Ringe?
pub fn contains(rings: &[Ring], p: Pt) -> bool {
    winding(rings, p) != 0
}

/// Ringe als Eckpunkt-Pfade.
pub fn rings_to_paths(rings: Vec<Ring>) -> Vec<SubPath> {
    rings_to_subpaths(rings)
}

/// Boolesche Operation auf zwei Pfaden (Liste von Teilpfaden). Ergebnis als Eckpunkt-Pfade.
#[cfg(test)]
pub fn apply(a: &[SubPath], b: &[SubPath], op: Op, tol: f64) -> Vec<SubPath> {
    let ra = normalize(flatten(a, tol));
    let rb = normalize(flatten(b, tol));
    rings_to_subpaths(boolean_rings(&ra, &rb, op))
}

/// Mehrere Formen nacheinander verknüpfen (Reihenfolge: unten nach oben).
///
/// * Union/Intersection/Xor: alle der Reihe nach.
/// * Difference: die unterste Form minus alle anderen.
pub fn apply_many(shapes: &[Vec<SubPath>], op: Op, tol: f64) -> Vec<SubPath> {
    let mut iter = shapes.iter().map(|s| normalize(flatten(s, tol)));
    let Some(first) = iter.next() else {
        return Vec::new();
    };
    let mut acc = first;
    for next in iter {
        acc = boolean_rings(&acc, &next, op);
    }
    rings_to_subpaths(acc)
}

fn rings_to_subpaths(rings: Vec<Ring>) -> Vec<SubPath> {
    rings
        .into_iter()
        .map(|r| SubPath {
            nodes: r.into_iter().map(Node::corner).collect(),
            closed: true,
        })
        .collect()
}

/// Gesamtfläche (Außenkonturen positiv, Löcher negativ).
#[cfg(test)]
pub fn total_area(subpaths: &[SubPath]) -> f64 {
    normalize(flatten(subpaths, 0.05)).iter().map(|r| area(r)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Shape;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Vec<SubPath> {
        match (Shape::Rect { x, y, w, h, r: 0.0 }).to_path().unwrap() {
            Shape::Path { subpaths, .. } => subpaths,
            _ => unreachable!(),
        }
    }
    fn circle(cx: f64, cy: f64, r: f64) -> Vec<SubPath> {
        match (Shape::Ellipse { cx, cy, rx: r, ry: r }).to_path().unwrap() {
            Shape::Path { subpaths, .. } => subpaths,
            _ => unreachable!(),
        }
    }
    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn overlapping_squares() {
        let a = rect(0.0, 0.0, 2.0, 2.0);
        let b = rect(1.0, 1.0, 2.0, 2.0);
        assert!(close(total_area(&apply(&a, &b, Op::Union, 0.1)), 7.0));
        assert!(close(total_area(&apply(&a, &b, Op::Intersection, 0.1)), 1.0));
        assert!(close(total_area(&apply(&a, &b, Op::Difference, 0.1)), 3.0));
        assert!(close(total_area(&apply(&a, &b, Op::Xor, 0.1)), 6.0));
        let u = apply(&a, &b, Op::Union, 0.1);
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].nodes.len(), 8);
    }

    #[test]
    fn shared_edges() {
        let a = rect(0.0, 0.0, 1.0, 1.0);
        let b = rect(1.0, 0.0, 1.0, 1.0);
        let u = apply(&a, &b, Op::Union, 0.1);
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].nodes.len(), 4);
        assert!(close(total_area(&u), 2.0));
        assert!(apply(&a, &b, Op::Intersection, 0.1).is_empty());
        assert!(close(total_area(&apply(&a, &b, Op::Difference, 0.1)), 1.0));
    }

    #[test]
    fn identical() {
        let a = rect(0.0, 0.0, 3.0, 3.0);
        assert!(close(total_area(&apply(&a, &a, Op::Union, 0.1)), 9.0));
        assert!(close(total_area(&apply(&a, &a, Op::Intersection, 0.1)), 9.0));
        assert!(apply(&a, &a, Op::Difference, 0.1).is_empty());
    }

    #[test]
    fn hole() {
        let a = rect(0.0, 0.0, 4.0, 4.0);
        let b = rect(1.0, 1.0, 2.0, 2.0);
        let d = apply(&a, &b, Op::Difference, 0.1);
        assert_eq!(d.len(), 2);
        assert!(close(total_area(&d), 12.0));
        // Loch wieder füllen
        let u = apply(&d, &b, Op::Union, 0.1);
        assert!(close(total_area(&u), 16.0));
    }

    #[test]
    fn touching_corner_and_disjoint() {
        let a = rect(0.0, 0.0, 1.0, 1.0);
        let b = rect(1.0, 1.0, 1.0, 1.0);
        assert!(close(total_area(&apply(&a, &b, Op::Union, 0.1)), 2.0));
        let c = rect(5.0, 5.0, 1.0, 1.0);
        let u = apply(&a, &c, Op::Union, 0.1);
        assert_eq!(u.len(), 2);
        assert!(apply(&a, &c, Op::Intersection, 0.1).is_empty());
    }

    #[test]
    fn circles() {
        let a = circle(0.0, 0.0, 10.0);
        let b = circle(10.0, 0.0, 10.0);
        let full = std::f64::consts::PI * 100.0;
        let i = total_area(&apply(&a, &b, Op::Intersection, 0.01));
        // Linsenfläche zweier Kreise mit Abstand r: r²(2π/3 − √3/2)
        let lens = 100.0 * (2.0 * std::f64::consts::PI / 3.0 - 3f64.sqrt() / 2.0);
        assert!((i - lens).abs() < 0.5, "{i} vs {lens}");
        let u = total_area(&apply(&a, &b, Op::Union, 0.01));
        assert!((u - (2.0 * full - lens)).abs() < 1.0);
    }

    #[test]
    fn many() {
        let shapes = vec![
            rect(0.0, 0.0, 10.0, 10.0),
            rect(2.0, 2.0, 2.0, 2.0),
            rect(6.0, 6.0, 2.0, 2.0),
        ];
        let d = apply_many(&shapes, Op::Difference, 0.1);
        assert!(close(total_area(&d), 92.0));
        let u = apply_many(&shapes, Op::Union, 0.1);
        assert!(close(total_area(&u), 100.0));
    }

    #[test]
    fn reversed_input_orientation() {
        let mut a = rect(0.0, 0.0, 2.0, 2.0);
        a[0].nodes.reverse();
        let b = rect(1.0, 1.0, 2.0, 2.0);
        assert!(close(total_area(&apply(&a, &b, Op::Union, 0.1)), 7.0));
    }
}
