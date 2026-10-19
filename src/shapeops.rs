//! Formerstellung, Radierer und Farbeimer.
//!
//! * Formerstellung: Überlappende Formen werden in ihre kleinsten Teilflächen
//!   („Atome“) zerlegt. Über die mit dem Werkzeug gestrichen wird, werden zu einer
//!   Form vereinigt.
//! * Radierer: Ein runder Pinsel zieht eine Spur; gefüllte Formen verlieren die Fläche
//!   der Spur, Linien werden an der Spur aufgetrennt.

use crate::boolean::{self, area, Op, Ring};
use crate::geom::{cubic_point, Pt, Rect};
use crate::model::{Node, SubPath};

/// Kleinste Teilfläche überlappender Formen.
#[derive(Clone, Debug)]
pub struct Atom {
    /// Ausgerichtete Ringe (eine zusammenhängende Fläche, ggf. mit Löchern)
    pub rings: Vec<Ring>,
    /// oberstes Objekt, zu dem die Fläche gehört (bestimmt den Stil)
    pub owner: u64,
    /// alle Objekte, die diese Fläche bedecken
    #[allow(dead_code)]
    pub covers: Vec<u64>,
    pub bbox: Rect,
}

/// Flächen kleiner als das (in Dokumenteinheiten²) gelten als Rechenrauschen.
const MIN_AREA: f64 = 0.5;

fn rings_area(r: &[Ring]) -> f64 {
    r.iter().map(|x| area(x)).sum()
}

fn rings_bbox(r: &[Ring]) -> Rect {
    let mut b = Rect {
        x0: f64::MAX,
        y0: f64::MAX,
        x1: f64::MIN,
        y1: f64::MIN,
    };
    for ring in r {
        for p in ring {
            b.x0 = b.x0.min(p.x);
            b.y0 = b.y0.min(p.y);
            b.x1 = b.x1.max(p.x);
            b.y1 = b.y1.max(p.y);
        }
    }
    b
}

/// Ringe in zusammenhängende Flächen aufteilen (jede Außenkontur mit ihren Löchern).
pub fn components(rings: Vec<Ring>) -> Vec<Vec<Ring>> {
    let (outer, holes): (Vec<Ring>, Vec<Ring>) = rings.into_iter().partition(|r| area(r) > 0.0);
    let mut groups: Vec<Vec<Ring>> = outer.into_iter().map(|r| vec![r]).collect();
    for h in holes {
        let probe = h[0];
        // kleinste Außenkontur, die das Loch enthält
        let best = groups
            .iter()
            .enumerate()
            .filter(|(_, g)| boolean::contains(&g[..1], probe))
            .min_by(|a, b| area(&a.1[0]).total_cmp(&area(&b.1[0])))
            .map(|(i, _)| i);
        if let Some(i) = best {
            groups[i].push(h);
        }
    }
    groups
}

/// Formen (unten nach oben, je `(id, Ringe)`) in ihre Teilflächen zerlegen.
pub fn atoms(shapes: &[(u64, Vec<Ring>)]) -> Vec<Atom> {
    // (Fläche, Objekte darüber)
    let mut regions: Vec<(Vec<Ring>, Vec<u64>)> = Vec::new();
    let mut acc: Vec<Ring> = Vec::new();
    for (id, s) in shapes {
        if s.is_empty() {
            continue;
        }
        let mut next = Vec::new();
        for (r, cov) in regions {
            let inter = boolean::rings_op(&r, s, Op::Intersection);
            if rings_area(&inter) > MIN_AREA {
                let mut c = cov.clone();
                c.push(*id);
                next.push((inter, c));
            }
            let diff = boolean::rings_op(&r, s, Op::Difference);
            if rings_area(&diff) > MIN_AREA {
                next.push((diff, cov));
            }
        }
        let rest = if acc.is_empty() {
            s.clone()
        } else {
            boolean::rings_op(s, &acc, Op::Difference)
        };
        if rings_area(&rest) > MIN_AREA {
            next.push((rest, vec![*id]));
        }
        acc = if acc.is_empty() {
            s.clone()
        } else {
            boolean::rings_op(&acc, s, Op::Union)
        };
        regions = next;
    }
    let mut out = Vec::new();
    for (r, cov) in regions {
        for comp in components(r) {
            if rings_area(&comp) <= MIN_AREA {
                continue;
            }
            out.push(Atom {
                bbox: rings_bbox(&comp),
                owner: *cov.last().unwrap_or(&0),
                covers: cov.clone(),
                rings: comp,
            });
        }
    }
    out
}

/// Liegt `p` in der Teilfläche?
pub fn atom_contains(a: &Atom, p: Pt) -> bool {
    p.x >= a.bbox.x0
        && p.x <= a.bbox.x1
        && p.y >= a.bbox.y0
        && p.y <= a.bbox.y1
        && boolean::contains(&a.rings, p)
}

/// Punkte einer Zeigerspur in gleichmäßigen Abständen (höchstens `step`).
pub fn resample(points: &[Pt], step: f64) -> Vec<Pt> {
    let mut out: Vec<Pt> = Vec::new();
    for &p in points {
        match out.last().copied() {
            None => out.push(p),
            Some(last) => {
                let d = last.dist(p);
                if d < 1e-9 {
                    continue;
                }
                let n = (d / step).ceil() as usize;
                for k in 1..=n {
                    out.push(last.lerp(p, k as f64 / n as f64));
                }
            }
        }
    }
    out
}

/// Abstand von `p` zur Polylinie.
pub fn dist_to_polyline(line: &[Pt], p: Pt) -> f64 {
    if line.len() == 1 {
        return line[0].dist(p);
    }
    let mut best = f64::MAX;
    for w in line.windows(2) {
        let (a, b) = (w[0], w[1]);
        let ab = b - a;
        let l2 = ab.x * ab.x + ab.y * ab.y;
        let t = if l2 < 1e-12 {
            0.0
        } else {
            (((p - a).x * ab.x + (p - a).y * ab.y) / l2).clamp(0.0, 1.0)
        };
        best = best.min(a.lerp(b, t).dist(p));
    }
    best
}

fn circle_ring(c: Pt, r: f64, n: usize) -> Ring {
    (0..n)
        .map(|k| {
            let t = k as f64 / n as f64 * std::f64::consts::TAU;
            Pt::new(c.x + r * t.cos(), c.y + r * t.sin())
        })
        .collect()
}

/// Kapsel (dicke Strecke mit runden Enden) als Polygon.
fn capsule(a: Pt, b: Pt, r: f64) -> Ring {
    let d = b - a;
    let len = d.len();
    if len < 1e-9 {
        return circle_ring(a, r, 24);
    }
    let ang = d.y.atan2(d.x);
    let half = 12;
    let mut ring = Vec::with_capacity(2 * half + 2);
    // um b herum (von −90° bis +90° relativ zur Richtung), dann um a herum
    for k in 0..=half {
        let t = ang - std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * k as f64 / half as f64;
        ring.push(Pt::new(b.x + r * t.cos(), b.y + r * t.sin()));
    }
    for k in 0..=half {
        let t = ang + std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * k as f64 / half as f64;
        ring.push(Pt::new(a.x + r * t.cos(), a.y + r * t.sin()));
    }
    if area(&ring) < 0.0 {
        ring.reverse();
    }
    ring
}

/// Fläche der Radiererspur (Vereinigung runder Kapseln entlang der Punkte).
pub fn stroke_region(points: &[Pt], r: f64) -> Vec<Ring> {
    let pts = resample(points, (r * 1.5).max(0.5));
    let pts = crate::geom::simplify(&pts, r * 0.15);
    let mut acc: Vec<Ring> = Vec::new();
    if pts.len() == 1 {
        return vec![circle_ring(pts[0], r, 32)];
    }
    for w in pts.windows(2) {
        let c = vec![capsule(w[0], w[1], r)];
        acc = if acc.is_empty() {
            c
        } else {
            boolean::rings_op(&acc, &c, Op::Union)
        };
    }
    acc
}

/// Kurvenstück zwischen `t0` und `t1` (De Casteljau).
pub fn sub_cubic(p0: Pt, c1: Pt, c2: Pt, p3: Pt, t0: f64, t1: f64) -> (Pt, Pt, Pt, Pt) {
    // erst bei t1 teilen (linker Teil), dann den linken Teil bei t0/t1
    let split = |p0: Pt, c1: Pt, c2: Pt, p3: Pt, t: f64| {
        let a = p0.lerp(c1, t);
        let b = c1.lerp(c2, t);
        let c = c2.lerp(p3, t);
        let d = a.lerp(b, t);
        let e = b.lerp(c, t);
        let f = d.lerp(e, t);
        ((p0, a, d, f), (f, e, c, p3))
    };
    let (left, _) = split(p0, c1, c2, p3, t1.clamp(0.0, 1.0));
    if t1 <= 1e-12 {
        return (p0, p0, p0, p0);
    }
    let u = (t0 / t1).clamp(0.0, 1.0);
    let (_, right) = split(left.0, left.1, left.2, left.3, u);
    right
}

/// Linie (offener oder geschlossener Teilpfad ohne Füllung) dort auftrennen, wo sie im
/// Radierer liegt. Liefert die übrig bleibenden offenen Stücke.
pub fn erase_line(sp: &SubPath, inside: &dyn Fn(Pt) -> bool) -> Vec<SubPath> {
    const N: usize = 32;
    // Folge von Kurvenstücken außerhalb; None = Lücke
    let mut pieces: Vec<Vec<(Pt, Pt, Pt, Pt)>> = vec![Vec::new()];
    let mut touched = false;
    for (p0, c1, c2, p3) in sp.segments() {
        let at = |t: f64| cubic_point(p0, c1, c2, p3, t);
        let mut ins: Vec<bool> = (0..=N).map(|k| inside(at(k as f64 / N as f64))).collect();
        if ins.iter().all(|x| !x) {
            pieces.last_mut().unwrap().push((p0, c1, c2, p3));
            continue;
        }
        touched = true;
        // Grenzen genauer bestimmen (Halbierung)
        let refine = |mut a: f64, mut b: f64, a_in: bool| {
            for _ in 0..20 {
                let m = (a + b) / 2.0;
                if inside(at(m)) == a_in {
                    a = m;
                } else {
                    b = m;
                }
            }
            (a + b) / 2.0
        };
        let mut t_start = 0.0;
        let mut cur_in = ins[0];
        // Endpunkte exakt
        ins[N] = inside(p3);
        for k in 1..=N {
            if ins[k] != cur_in {
                let tb = refine((k - 1) as f64 / N as f64, k as f64 / N as f64, cur_in);
                if !cur_in {
                    pieces
                        .last_mut()
                        .unwrap()
                        .push(sub_cubic(p0, c1, c2, p3, t_start, tb));
                    pieces.push(Vec::new());
                }
                t_start = tb;
                cur_in = ins[k];
            }
        }
        if !cur_in {
            pieces
                .last_mut()
                .unwrap()
                .push(sub_cubic(p0, c1, c2, p3, t_start, 1.0));
        } else if !pieces.last().unwrap().is_empty() {
            pieces.push(Vec::new());
        }
    }
    if !touched {
        return vec![sp.clone()];
    }
    // Bei geschlossenen Pfaden hängen letztes und erstes Stück zusammen
    if sp.closed && pieces.len() >= 2 {
        let first_starts_at_node0 = sp.segments().first().map(|s| !inside(s.0)).unwrap_or(false);
        if first_starts_at_node0 && !pieces.last().unwrap().is_empty() {
            let mut last = pieces.pop().unwrap();
            last.extend(pieces.remove(0));
            pieces.insert(0, last);
        }
    }
    pieces
        .into_iter()
        .filter(|segs| !segs.is_empty())
        .filter(|segs| {
            // winzige Reste weglassen
            let len: f64 = segs.iter().map(|s| s.0.dist(s.3)).sum();
            len > 0.5
        })
        .map(|segs| {
            let mut nodes: Vec<Node> = Vec::with_capacity(segs.len() + 1);
            for (i, (a, b, c, d)) in segs.iter().enumerate() {
                if i == 0 {
                    let mut n = Node::corner(*a);
                    n.h_out = *b;
                    nodes.push(n);
                } else if let Some(last) = nodes.last_mut() {
                    last.h_out = *b;
                }
                let mut n = Node::corner(*d);
                n.h_in = *c;
                nodes.push(n);
            }
            SubPath { nodes, closed: false }
        })
        .collect()
}

// ───────────── Farbeimer ─────────────
//
// Alle Linien (Konturen, Pfade, Freihandstriche) werden an ihren Schnittpunkten geteilt.
// Die Teilstücke bilden einen ebenen Graphen; seine Flächen sind die Zwischenräume.
// Die Fläche unter dem Klick wird aus den echten Kurvenstücken zusammengesetzt.

/// Kubische Bézierkurve (Anfang, Steuerpunkt, Steuerpunkt, Ende).
pub type Cubic = (Pt, Pt, Pt, Pt);

fn ctrl_box(c: &Cubic) -> Rect {
    let xs = [c.0.x, c.1.x, c.2.x, c.3.x];
    let ys = [c.0.y, c.1.y, c.2.y, c.3.y];
    Rect {
        x0: xs.iter().cloned().fold(f64::MAX, f64::min),
        y0: ys.iter().cloned().fold(f64::MAX, f64::min),
        x1: xs.iter().cloned().fold(f64::MIN, f64::max),
        y1: ys.iter().cloned().fold(f64::MIN, f64::max),
    }
}

fn split_half(c: &Cubic) -> (Cubic, Cubic) {
    let (p0, c1, c2, p3) = *c;
    let a = p0.lerp(c1, 0.5);
    let b = c1.lerp(c2, 0.5);
    let cc = c2.lerp(p3, 0.5);
    let d = a.lerp(b, 0.5);
    let e = b.lerp(cc, 0.5);
    let f = d.lerp(e, 0.5);
    ((p0, a, d, f), (f, e, cc, p3))
}

fn boxes_touch(a: &Rect, b: &Rect, tol: f64) -> bool {
    a.x0 <= b.x1 + tol && b.x0 <= a.x1 + tol && a.y0 <= b.y1 + tol && b.y0 <= a.y1 + tol
}

/// Schnittpunkte zweier Kurven: (t auf a, t auf b, Punkt). Leer bei deckungsgleichen
/// Stücken (zu viele Treffer).
pub fn intersect_cubics(a: &Cubic, b: &Cubic, tol: f64) -> Vec<(f64, f64, Pt)> {
    let mut out: Vec<(f64, f64, Pt)> = Vec::new();
    #[allow(clippy::too_many_arguments)]
    fn rec(
        a: Cubic,
        ta: (f64, f64),
        b: Cubic,
        tb: (f64, f64),
        tol: f64,
        depth: u32,
        out: &mut Vec<(f64, f64, Pt)>,
        budget: &mut u32,
    ) {
        if *budget == 0 {
            return;
        }
        let (ba, bb) = (ctrl_box(&a), ctrl_box(&b));
        if !boxes_touch(&ba, &bb, tol) {
            return;
        }
        *budget -= 1;
        let sa = ba.width().max(ba.height());
        let sb = bb.width().max(bb.height());
        if (sa <= tol && sb <= tol) || depth > 45 {
            let p = a.0.lerp(a.3, 0.5).lerp(b.0.lerp(b.3, 0.5), 0.5);
            out.push(((ta.0 + ta.1) / 2.0, (tb.0 + tb.1) / 2.0, p));
            return;
        }
        if sa >= sb {
            let (a1, a2) = split_half(&a);
            let m = (ta.0 + ta.1) / 2.0;
            rec(a1, (ta.0, m), b, tb, tol, depth + 1, out, budget);
            rec(a2, (m, ta.1), b, tb, tol, depth + 1, out, budget);
        } else {
            let (b1, b2) = split_half(&b);
            let m = (tb.0 + tb.1) / 2.0;
            rec(a, ta, b1, (tb.0, m), tol, depth + 1, out, budget);
            rec(a, ta, b2, (m, tb.1), tol, depth + 1, out, budget);
        }
    }
    let mut budget = 20_000;
    rec(*a, (0.0, 1.0), *b, (0.0, 1.0), tol, 0, &mut out, &mut budget);
    // benachbarte Treffer zusammenfassen
    out.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut merged: Vec<(f64, f64, Pt)> = Vec::new();
    for r in out {
        if let Some(last) = merged.last() {
            if last.2.dist(r.2) <= tol * 4.0 {
                continue;
            }
        }
        merged.push(r);
    }
    if merged.len() > 12 || budget == 0 {
        // deckungsgleiche Linien: nicht teilen
        return Vec::new();
    }
    merged
}

/// Fläche des Graphen: Rand als Kurvenstücke und als Polygon.
#[derive(Clone, Debug)]
pub struct Face {
    pub segs: Vec<Cubic>,
    pub poly: Vec<Pt>,
    pub area: f64,
    pub bbox: Rect,
}

impl Face {
    fn contains(&self, p: Pt) -> bool {
        if p.x < self.bbox.x0 || p.x > self.bbox.x1 || p.y < self.bbox.y0 || p.y > self.bbox.y1 {
            return false;
        }
        let mut inside = false;
        let n = self.poly.len();
        for i in 0..n {
            let a = self.poly[i];
            let b = self.poly[(i + 1) % n];
            if (a.y > p.y) != (b.y > p.y) {
                let x = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
                if x > p.x {
                    inside = !inside;
                }
            }
        }
        inside
    }
}

/// Ebener Graph aus Kurven mit seinen Flächen.
#[derive(Clone, Debug, Default)]
pub struct Arrangement {
    pub faces: Vec<Face>,
    /// Vorzeichen der Fläche von Außenrändern (unbegrenzte Flächen)
    outer_sign: f64,
}

fn reverse(c: &Cubic) -> Cubic {
    (c.3, c.2, c.1, c.0)
}

fn flatten_cubic(c: &Cubic, out: &mut Vec<Pt>) {
    let len = c.0.dist(c.1) + c.1.dist(c.2) + c.2.dist(c.3);
    let n = ((len / 2.0).ceil() as usize).clamp(2, 48);
    for k in 0..n {
        out.push(cubic_point(c.0, c.1, c.2, c.3, k as f64 / n as f64));
    }
}

/// Aus allen Kurven den ebenen Graphen bilden. `tol`: Genauigkeit in Dokumenteinheiten.
pub fn arrangement(curves: &[Cubic], tol: f64) -> Arrangement {
    let curves: Vec<Cubic> = curves
        .iter()
        .copied()
        .filter(|c| c.0.dist(c.1) + c.1.dist(c.2) + c.2.dist(c.3) > tol)
        .collect();
    let n = curves.len();
    let boxes: Vec<Rect> = curves.iter().map(ctrl_box).collect();
    // Schnittpunkte je Kurve: (t, Punkt)
    let mut cuts: Vec<Vec<(f64, Pt)>> = vec![Vec::new(); n];
    // nach x sortiert durchgehen, damit nur nahe Kurven verglichen werden
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|a, b| boxes[*a].x0.total_cmp(&boxes[*b].x0));
    for (oi, &i) in order.iter().enumerate() {
        for &j in &order[oi + 1..] {
            if boxes[j].x0 > boxes[i].x1 + tol {
                break;
            }
            if !boxes_touch(&boxes[i], &boxes[j], tol) {
                continue;
            }
            for (ta, tb, p) in intersect_cubics(&curves[i], &curves[j], tol * 0.25) {
                cuts[i].push((ta, p));
                cuts[j].push((tb, p));
            }
        }
    }
    // Knoten zusammenführen (gleiche Punkte innerhalb der Toleranz)
    let eps = tol * 2.0;
    let mut verts: Vec<Pt> = Vec::new();
    let mut grid: std::collections::HashMap<(i64, i64), Vec<usize>> = std::collections::HashMap::new();
    let mut vid = |p: Pt, verts: &mut Vec<Pt>| -> usize {
        let key = ((p.x / eps).floor() as i64, (p.y / eps).floor() as i64);
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(v) = grid.get(&(key.0 + dx, key.1 + dy)) {
                    for &k in v {
                        if verts[k].dist(p) <= eps {
                            return k;
                        }
                    }
                }
            }
        }
        verts.push(p);
        grid.entry(key).or_default().push(verts.len() - 1);
        verts.len() - 1
    };
    // Kanten: (von, nach, Kurve)
    let mut edges: Vec<(usize, usize, Cubic)> = Vec::new();
    for (i, c) in curves.iter().enumerate() {
        let mut ts: Vec<(f64, Pt)> = cuts[i]
            .iter()
            .copied()
            .filter(|(t, _)| *t > 1e-4 && *t < 1.0 - 1e-4)
            .collect();
        ts.sort_by(|a, b| a.0.total_cmp(&b.0));
        ts.dedup_by(|a, b| a.1.dist(b.1) <= eps);
        let mut t0 = 0.0;
        let mut p_start = c.0;
        for (t, pt) in ts.iter().copied().chain(std::iter::once((1.0, c.3))) {
            let mut piece = sub_cubic(c.0, c.1, c.2, c.3, t0, t);
            // Enden genau auf die Schnittpunkte legen
            let ds = p_start - piece.0;
            let de = pt - piece.3;
            piece = (p_start, piece.1 + ds, piece.2 + de, pt);
            let u = vid(piece.0, &mut verts);
            let v = vid(piece.3, &mut verts);
            let len = piece.0.dist(piece.1) + piece.1.dist(piece.2) + piece.2.dist(piece.3);
            if !(u == v && len < eps * 4.0) {
                edges.push((u, v, (verts[u], piece.1, piece.2, verts[v])));
            }
            t0 = t;
            p_start = pt;
        }
    }
    // offene Enden (Grad 1) wiederholt entfernen
    let mut alive = vec![true; edges.len()];
    loop {
        let mut deg = vec![0usize; verts.len()];
        for (k, e) in edges.iter().enumerate() {
            if alive[k] {
                deg[e.0] += 1;
                deg[e.1] += 1;
            }
        }
        let mut changed = false;
        for (k, e) in edges.iter().enumerate() {
            if alive[k] && e.0 != e.1 && (deg[e.0] <= 1 || deg[e.1] <= 1) {
                alive[k] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let edges: Vec<(usize, usize, Cubic)> = edges
        .into_iter()
        .zip(alive)
        .filter(|(_, a)| *a)
        .map(|(e, _)| e)
        .collect();
    // Halbkanten: 2k = vorwärts, 2k+1 = rückwärts
    let half = |h: usize| -> Cubic {
        let e = &edges[h / 2].2;
        if h % 2 == 0 {
            *e
        } else {
            reverse(e)
        }
    };
    let origin = |h: usize| -> usize {
        let e = &edges[h / 2];
        if h % 2 == 0 {
            e.0
        } else {
            e.1
        }
    };
    let angle = |h: usize| -> f64 {
        let c = half(h);
        let q = cubic_point(c.0, c.1, c.2, c.3, 0.02);
        let d = q - c.0;
        d.y.atan2(d.x)
    };
    let mut out_at: Vec<Vec<(f64, usize)>> = vec![Vec::new(); verts.len()];
    for h in 0..edges.len() * 2 {
        out_at[origin(h)].push((angle(h), h));
    }
    for v in &mut out_at {
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    // Position jeder Halbkante in der Liste ihres Ursprungs
    let mut pos = vec![0usize; edges.len() * 2];
    for v in &out_at {
        for (i, (_, h)) in v.iter().enumerate() {
            pos[*h] = i;
        }
    }
    let next = |h: usize| -> usize {
        let twin = h ^ 1;
        let v = origin(twin);
        let list = &out_at[v];
        let i = pos[twin];
        list[(i + list.len() - 1) % list.len()].1
    };
    let mut seen = vec![false; edges.len() * 2];
    let mut faces = Vec::new();
    for start in 0..edges.len() * 2 {
        if seen[start] {
            continue;
        }
        let mut segs = Vec::new();
        let mut h = start;
        let mut guard = 0;
        loop {
            seen[h] = true;
            segs.push(half(h));
            h = next(h);
            guard += 1;
            if h == start || guard > 100_000 || seen[h] {
                break;
            }
        }
        if h != start {
            continue;
        }
        let mut poly = Vec::new();
        for c in &segs {
            flatten_cubic(c, &mut poly);
        }
        if poly.len() < 3 {
            continue;
        }
        let area = area(&poly);
        let bbox = rings_bbox(std::slice::from_ref(&poly));
        faces.push(Face {
            segs,
            poly,
            area,
            bbox,
        });
    }
    let outer_sign = faces
        .iter()
        .max_by(|a, b| a.area.abs().total_cmp(&b.area.abs()))
        .map(|f| f.area.signum())
        .unwrap_or(1.0);
    Arrangement { faces, outer_sign }
}

impl Arrangement {
    /// Fläche unter `p`: Außenrand und Löcher (Ränder darin liegender, getrennter
    /// Linienzüge) – jeweils als Kurvenstücke.
    pub fn region_at(&self, p: Pt) -> Option<Vec<Vec<Cubic>>> {
        let inner = |f: &&Face| f.area.signum() != self.outer_sign && f.area.abs() > 1e-6;
        let face = self
            .faces
            .iter()
            .filter(inner)
            .filter(|f| f.contains(p))
            .min_by(|a, b| a.area.abs().total_cmp(&b.area.abs()))?;
        // Außenränder anderer Linienzüge, die in der Fläche liegen, werden zu Löchern
        let mut holes: Vec<&Face> = self
            .faces
            .iter()
            .filter(|f| f.area.signum() == self.outer_sign)
            .filter(|f| f.area.abs() < face.area.abs())
            .filter(|f| !f.contains(p))
            .filter(|f| face.contains(f.poly[0]))
            .collect();
        // nur die äußersten Löcher
        let all = holes.clone();
        holes.retain(|h| {
            !all.iter()
                .any(|o| !std::ptr::eq(*o, *h) && o.area.abs() > h.area.abs() && o.contains(h.poly[0]))
        });
        let mut out = vec![face.segs.clone()];
        out.extend(holes.into_iter().map(|h| h.segs.clone()));
        Some(out)
    }
}

/// Kurvenstücke eines Randes als geschlossener Teilpfad.
pub fn segs_to_subpath(segs: &[Cubic]) -> SubPath {
    let n = segs.len();
    let nodes = (0..n)
        .map(|i| {
            let prev = &segs[(i + n - 1) % n];
            let s = &segs[i];
            Node {
                p: s.0,
                h_in: prev.2,
                h_out: s.1,
                r: 0.0,
            }
        })
        .collect();
    SubPath { nodes, closed: true }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Shape;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Vec<Ring> {
        match (Shape::Rect { x, y, w, h, r: 0.0 }).to_path().unwrap() {
            Shape::Path { subpaths, .. } => boolean::to_rings(&subpaths, 0.1),
            _ => unreachable!(),
        }
    }

    #[test]
    fn atoms_of_two_rects() {
        // zwei überlappende Quadrate: links, Mitte, rechts
        let a = rect(0.0, 0.0, 20.0, 20.0);
        let b = rect(10.0, 0.0, 20.0, 20.0);
        let at = atoms(&[(1, a), (2, b)]);
        assert_eq!(at.len(), 3, "{at:?}");
        let mid = at.iter().find(|x| atom_contains(x, Pt::new(15.0, 10.0))).unwrap();
        assert_eq!(mid.owner, 2);
        assert_eq!(mid.covers, vec![1, 2]);
        assert!((rings_area(&mid.rings) - 200.0).abs() < 1.0);
        let left = at.iter().find(|x| atom_contains(x, Pt::new(5.0, 10.0))).unwrap();
        assert_eq!(left.owner, 1);
    }

    #[test]
    fn components_split() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let mut both = a.clone();
        both.extend(rect(20.0, 0.0, 10.0, 10.0));
        assert_eq!(components(both).len(), 2);
    }

    #[test]
    fn eraser_region_and_line() {
        let reg = stroke_region(&[Pt::new(0.0, 0.0), Pt::new(100.0, 0.0)], 10.0);
        let ar = rings_area(&reg);
        // 100×20 Rechteck + Kreis (π·100)
        assert!(
            (ar - (2000.0 + std::f64::consts::PI * 100.0)).abs() < 60.0,
            "{ar}"
        );
        // waagerechte Linie wird an einer senkrechten Spur aufgetrennt
        let line = SubPath {
            nodes: vec![
                Node::corner(Pt::new(0.0, 50.0)),
                Node::corner(Pt::new(100.0, 50.0)),
            ],
            closed: false,
        };
        let spur = [Pt::new(50.0, 0.0), Pt::new(50.0, 100.0)];
        let parts = erase_line(&line, &|p| dist_to_polyline(&spur, p) <= 5.0);
        assert_eq!(parts.len(), 2);
        assert!((parts[0].nodes.last().unwrap().p.x - 45.0).abs() < 0.01);
        assert!((parts[1].nodes[0].p.x - 55.0).abs() < 0.01);
        // geschlossenes Quadrat ohne Füllung: an einer Stelle aufgetrennt → ein offener Zug
        let sq = SubPath {
            nodes: [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]
                .iter()
                .map(|(x, y)| Node::corner(Pt::new(*x, *y)))
                .collect(),
            closed: true,
        };
        let parts = erase_line(&sq, &|p| {
            dist_to_polyline(&[Pt::new(50.0, -20.0), Pt::new(50.0, 20.0)], p) <= 5.0
        });
        assert_eq!(parts.len(), 1, "{parts:?}");
        assert!(!parts[0].closed);
        assert!((parts[0].nodes[0].p.x - 55.0).abs() < 0.01);
    }

    fn line(a: Pt, b: Pt) -> Cubic {
        (a, a.lerp(b, 1.0 / 3.0), a.lerp(b, 2.0 / 3.0), b)
    }

    fn circle(c: Pt, r: f64) -> Vec<Cubic> {
        let k = 0.5523 * r;
        let (x, y) = (c.x, c.y);
        vec![
            (
                Pt::new(x + r, y),
                Pt::new(x + r, y + k),
                Pt::new(x + k, y + r),
                Pt::new(x, y + r),
            ),
            (
                Pt::new(x, y + r),
                Pt::new(x - k, y + r),
                Pt::new(x - r, y + k),
                Pt::new(x - r, y),
            ),
            (
                Pt::new(x - r, y),
                Pt::new(x - r, y - k),
                Pt::new(x - k, y - r),
                Pt::new(x, y - r),
            ),
            (
                Pt::new(x, y - r),
                Pt::new(x + k, y - r),
                Pt::new(x + r, y - k),
                Pt::new(x + r, y),
            ),
        ]
    }

    #[test]
    fn bucket_lens_of_two_circles() {
        let mut cs = circle(Pt::new(100.0, 100.0), 50.0);
        cs.extend(circle(Pt::new(150.0, 100.0), 50.0));
        let arr = arrangement(&cs, 0.01);
        let reg = arr.region_at(Pt::new(125.0, 100.0)).expect("Linse");
        assert_eq!(reg.len(), 1);
        // echte Kurven: die Bögen beider Kreise (an den ursprünglichen Knoten geteilt)
        assert_eq!(reg[0].len(), 4, "{:?}", reg[0]);
        let sp = segs_to_subpath(&reg[0]);
        let rings = boolean::to_rings(&[sp], 0.01);
        let a: f64 = rings.iter().map(|r| area(r)).sum::<f64>().abs();
        // Fläche der Linse: 2r²acos(d/2r) − (d/2)·√(4r²−d²)
        let (r, d) = (50.0f64, 50.0f64);
        let exact = 2.0 * r * r * (d / (2.0 * r)).acos() - d / 2.0 * (4.0 * r * r - d * d).sqrt();
        assert!((a - exact).abs() < exact * 0.01, "{a} vs {exact}");
        // die Schnittpunkte liegen genau auf beiden Kreisen
        for s in &reg[0] {
            assert!(
                (s.0.dist(Pt::new(100.0, 100.0)) - 50.0).abs() < 0.05
                    || (s.0.dist(Pt::new(150.0, 100.0)) - 50.0).abs() < 0.05
            );
        }
        // linke Mondsichel
        let left = arr.region_at(Pt::new(70.0, 100.0)).unwrap();
        assert!(left[0].len() >= 3);
        // außerhalb: nichts
        assert!(arr.region_at(Pt::new(300.0, 300.0)).is_none());
    }

    #[test]
    fn bucket_crossing_lines_and_hole() {
        // Dreieck aus drei sich kreuzenden, überstehenden Linien
        let ls = vec![
            line(Pt::new(-10.0, 0.0), Pt::new(110.0, 0.0)),
            line(Pt::new(0.0, -10.0), Pt::new(55.0, 100.0)),
            line(Pt::new(100.0, -10.0), Pt::new(45.0, 100.0)),
        ];
        let arr = arrangement(&ls, 0.01);
        let reg = arr.region_at(Pt::new(50.0, 20.0)).expect("Dreieck");
        assert_eq!(reg[0].len(), 3);
        // Ring zwischen zwei Kreisen: innerer Kreis wird Loch
        let mut cs = circle(Pt::new(0.0, 0.0), 100.0);
        cs.extend(circle(Pt::new(0.0, 0.0), 30.0));
        let arr = arrangement(&cs, 0.01);
        let reg = arr.region_at(Pt::new(60.0, 0.0)).unwrap();
        assert_eq!(reg.len(), 2);
        let inner = arr.region_at(Pt::new(0.0, 0.0)).unwrap();
        assert_eq!(inner.len(), 1);
    }
}
