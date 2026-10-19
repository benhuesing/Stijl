//! Bildnachzeichner: macht aus einem Bild mit flachen Farbflächen (Logo, Illustration)
//! bearbeitbare Vektorpfade (ohne GTK).
//!
//! Ablauf: Farbpalette bilden (Median Cut mit k-Means-Nachbesserung), jedes Pixel der nächsten
//! Palettenfarbe zuordnen, die Farben nach Fläche stapeln (die größte liegt unten und deckt
//! alles, die kleineren darüber – so entstehen keine Lücken), die Umrisse jeder Ebene mit
//! Marching Squares auf einem weichgezeichneten Feld finden, vereinfachen und als Bézierkurven
//! mit Ecken ausgeben.

use std::collections::HashMap;

use crate::geom::Pt;
use crate::model::{Node, SubPath};

/// Eine Farbe mit ihren Pfaden (Koordinaten in Pixeln des nachgezeichneten Bildes,
/// gefüllt nach der Even-Odd-Regel).
pub struct Layer {
    pub color: [u8; 3],
    pub paths: Vec<SubPath>,
}

type Rgb = [f32; 3];

fn dist2(a: Rgb, b: Rgb) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

/// Farbpalette aus den deckenden Pixeln.
fn palette(rgba: &[u8], k: usize) -> Vec<Rgb> {
    let n = rgba.len() / 4;
    let step = (n / 80_000).max(1);
    // Histogramm auf 5 Bit je Kanal
    let mut hist: HashMap<u16, (u32, [f64; 3])> = HashMap::new();
    let mut i = 0;
    while i < n {
        let p = &rgba[i * 4..i * 4 + 4];
        if p[3] >= 128 {
            let key = ((p[0] >> 3) as u16) << 10 | ((p[1] >> 3) as u16) << 5 | (p[2] >> 3) as u16;
            let e = hist.entry(key).or_insert((0, [0.0; 3]));
            e.0 += 1;
            for c in 0..3 {
                e.1[c] += p[c] as f64;
            }
        }
        i += step;
    }
    // (Gewicht, mittlere Farbe) je Behälter
    let mut bins: Vec<(f32, Rgb)> = hist
        .values()
        .map(|(c, s)| {
            let c = *c as f64;
            (
                c as f32,
                [(s[0] / c) as f32, (s[1] / c) as f32, (s[2] / c) as f32],
            )
        })
        .collect();
    bins.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    if bins.is_empty() {
        return Vec::new();
    }
    let k = k.clamp(2, 32);
    if bins.len() <= k {
        return bins.into_iter().map(|b| b.1).collect();
    }
    // Median Cut
    let mut boxes: Vec<Vec<(f32, Rgb)>> = vec![bins.clone()];
    while boxes.len() < k {
        // Behälter mit dem größten Spielraum × Gewicht teilen
        let mut best: Option<(usize, f32, usize)> = None;
        for (bi, b) in boxes.iter().enumerate() {
            if b.len() < 2 {
                continue;
            }
            let mut best_ch = (0, 0.0f32);
            for ch in 0..3 {
                let lo = b.iter().map(|x| x.1[ch]).fold(f32::MAX, f32::min);
                let hi = b.iter().map(|x| x.1[ch]).fold(f32::MIN, f32::max);
                if hi - lo > best_ch.1 {
                    best_ch = (ch, hi - lo);
                }
            }
            let weight: f32 = b.iter().map(|x| x.0).sum();
            let score = best_ch.1 * weight.sqrt();
            if best.map(|x| score > x.1).unwrap_or(true) {
                best = Some((bi, score, best_ch.0));
            }
        }
        let Some((bi, _, ch)) = best else { break };
        let mut b = boxes.swap_remove(bi);
        b.sort_by(|x, y| x.1[ch].partial_cmp(&y.1[ch]).unwrap_or(std::cmp::Ordering::Equal));
        let total: f32 = b.iter().map(|x| x.0).sum();
        let mut acc = 0.0;
        let mut cut = 1;
        for (i, x) in b.iter().enumerate() {
            acc += x.0;
            if acc >= total / 2.0 {
                cut = (i + 1).clamp(1, b.len() - 1);
                break;
            }
        }
        let right = b.split_off(cut);
        boxes.push(b);
        boxes.push(right);
    }
    let mut pal: Vec<Rgb> = boxes
        .iter()
        .map(|b| {
            let w: f32 = b.iter().map(|x| x.0).sum();
            let mut c = [0.0; 3];
            for x in b {
                for ch in 0..3 {
                    c[ch] += x.1[ch] * x.0;
                }
            }
            [c[0] / w, c[1] / w, c[2] / w]
        })
        .collect();
    // k-Means-Nachbesserung auf den Behältern
    for _ in 0..4 {
        let mut sum = vec![([0f32; 3], 0f32); pal.len()];
        for (wt, c) in &bins {
            let j = nearest(&pal, *c);
            for ch in 0..3 {
                sum[j].0[ch] += c[ch] * wt;
            }
            sum[j].1 += wt;
        }
        for (j, s) in sum.iter().enumerate() {
            if s.1 > 0.0 {
                pal[j] = [s.0[0] / s.1, s.0[1] / s.1, s.0[2] / s.1];
            }
        }
    }
    pal
}

fn nearest(pal: &[Rgb], c: Rgb) -> usize {
    let mut best = (0, f32::MAX);
    for (i, p) in pal.iter().enumerate() {
        let d = dist2(*p, c);
        if d < best.1 {
            best = (i, d);
        }
    }
    best.0
}

const NONE: u8 = 255;

/// Palettenindex je Pixel (`NONE` für durchsichtige Pixel).
fn label(rgba: &[u8], w: usize, h: usize, pal: &[Rgb]) -> Vec<u8> {
    let mut cache: HashMap<u32, u8> = HashMap::new();
    let mut out = vec![NONE; w * h];
    for i in 0..w * h {
        let p = &rgba[i * 4..i * 4 + 4];
        if p[3] < 128 {
            continue;
        }
        let key = (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32;
        out[i] = *cache
            .entry(key)
            .or_insert_with(|| nearest(pal, [p[0] as f32, p[1] as f32, p[2] as f32]) as u8);
    }
    // einzelne Störpixel (alle vier Nachbarn anders) übernehmen die häufigste Nachbarfarbe
    let copy = out.clone();
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let me = copy[y * w + x];
            if me == NONE {
                continue;
            }
            let nb = [
                copy[y * w + x - 1],
                copy[y * w + x + 1],
                copy[(y - 1) * w + x],
                copy[(y + 1) * w + x],
            ];
            if nb.iter().all(|n| *n != me) {
                let mut best = (nb[0], 0);
                for a in nb {
                    let c = nb.iter().filter(|b| **b == a).count();
                    if c > best.1 {
                        best = (a, c);
                    }
                }
                if best.0 != NONE {
                    out[y * w + x] = best.0;
                }
            }
        }
    }
    out
}

// ───────────── Umrisse (Marching Squares) ─────────────

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Edge {
    H(i32, i32),
    V(i32, i32),
}

/// Geschlossene Umrisse der Maske auf dem weichgezeichneten Feld (Schwelle 0,5).
fn contours(mask: &[bool], w: usize, h: usize) -> Vec<Vec<Pt>> {
    // Feld mit zwei Rändern: innen der erste Rand setzt die Randpixel fort, der äußere ist leer
    let (fw, fh) = (w + 4, h + 4);
    let m = |x: i32, y: i32| -> f32 {
        let cx = x.clamp(0, w as i32 - 1) as usize;
        let cy = y.clamp(0, h as i32 - 1) as usize;
        if mask[cy * w + cx] {
            1.0
        } else {
            0.0
        }
    };
    let mut f = vec![0f32; fw * fh];
    for fy in 1..fh - 1 {
        for fx in 1..fw - 1 {
            let (x, y) = (fx as i32 - 2, fy as i32 - 2);
            let mut s = 0.0;
            for (dy, wy) in [(-1, 1.0), (0, 2.0), (1, 1.0)] {
                for (dx, wx) in [(-1, 1.0), (0, 2.0), (1, 1.0)] {
                    s += m(x + dx, y + dy) * wx * wy;
                }
            }
            f[fy * fw + fx] = s / 16.0;
        }
    }
    let at = |x: i32, y: i32| f[y as usize * fw + x as usize];
    let point = |e: Edge| -> Pt {
        let lerp = |a: f32, b: f32| {
            if (b - a).abs() < 1e-6 {
                0.5
            } else {
                ((0.5 - a) / (b - a)) as f64
            }
        };
        match e {
            Edge::H(x, y) => {
                let t = lerp(at(x, y), at(x + 1, y));
                Pt::new((x - 2) as f64 + 0.5 + t, (y - 2) as f64 + 0.5)
            }
            Edge::V(x, y) => {
                let t = lerp(at(x, y), at(x, y + 1));
                Pt::new((x - 2) as f64 + 0.5, (y - 2) as f64 + 0.5 + t)
            }
        }
    };
    // Segmente je Zelle
    let mut segs: Vec<(Edge, Edge)> = Vec::new();
    for cy in 0..fh as i32 - 1 {
        for cx in 0..fw as i32 - 1 {
            let (tl, tr, br, bl) = (at(cx, cy), at(cx + 1, cy), at(cx + 1, cy + 1), at(cx, cy + 1));
            let case = (tl >= 0.5) as u8
                | ((tr >= 0.5) as u8) << 1
                | ((br >= 0.5) as u8) << 2
                | ((bl >= 0.5) as u8) << 3;
            let top = Edge::H(cx, cy);
            let bottom = Edge::H(cx, cy + 1);
            let left = Edge::V(cx, cy);
            let right = Edge::V(cx + 1, cy);
            match case {
                0 | 15 => {}
                1 | 14 => segs.push((top, left)),
                2 | 13 => segs.push((top, right)),
                3 | 12 => segs.push((left, right)),
                4 | 11 => segs.push((right, bottom)),
                6 | 9 => segs.push((top, bottom)),
                7 | 8 => segs.push((left, bottom)),
                5 | 10 => {
                    let center = (tl + tr + br + bl) / 4.0 >= 0.5;
                    // case 5: tl und br innen
                    let joined = (case == 5) == center;
                    if joined {
                        segs.push((top, right));
                        segs.push((left, bottom));
                    } else {
                        segs.push((top, left));
                        segs.push((right, bottom));
                    }
                }
                _ => {}
            }
        }
    }
    let mut by_edge: HashMap<Edge, Vec<usize>> = HashMap::new();
    for (i, (a, b)) in segs.iter().enumerate() {
        by_edge.entry(*a).or_default().push(i);
        by_edge.entry(*b).or_default().push(i);
    }
    let mut used = vec![false; segs.len()];
    let mut loops = Vec::new();
    for s0 in 0..segs.len() {
        if used[s0] {
            continue;
        }
        used[s0] = true;
        let start = segs[s0].0;
        let mut cur = segs[s0].1;
        let mut pts = vec![point(start)];
        let mut guard = 0;
        while cur != start && guard < 5_000_000 {
            guard += 1;
            pts.push(point(cur));
            let next = by_edge
                .get(&cur)
                .and_then(|v| v.iter().copied().find(|i| !used[*i]));
            let Some(ni) = next else { break };
            used[ni] = true;
            let (a, b) = segs[ni];
            cur = if a == cur { b } else { a };
        }
        if pts.len() >= 3 {
            loops.push(pts);
        }
    }
    loops
}

fn area(p: &[Pt]) -> f64 {
    let mut a = 0.0;
    for i in 0..p.len() {
        let (u, v) = (p[i], p[(i + 1) % p.len()]);
        a += u.x * v.y - v.x * u.y;
    }
    a / 2.0
}

fn seg_dist(p: Pt, a: Pt, b: Pt) -> f64 {
    let d = b - a;
    let l2 = d.x * d.x + d.y * d.y;
    if l2 < 1e-12 {
        return p.dist(a);
    }
    let t = (((p.x - a.x) * d.x + (p.y - a.y) * d.y) / l2).clamp(0.0, 1.0);
    p.dist(Pt::new(a.x + d.x * t, a.y + d.y * t))
}

fn dp(pts: &[Pt], eps: f64, keep: &mut Vec<bool>, lo: usize, hi: usize) {
    if hi <= lo + 1 {
        return;
    }
    let (mut far, mut fd) = (lo, 0.0);
    for i in lo + 1..hi {
        let d = seg_dist(pts[i], pts[lo], pts[hi]);
        if d > fd {
            far = i;
            fd = d;
        }
    }
    if fd > eps {
        keep[far] = true;
        dp(pts, eps, keep, lo, far);
        dp(pts, eps, keep, far, hi);
    }
}

/// Douglas-Peucker für einen geschlossenen Umriss.
fn simplify_closed(pts: &[Pt], eps: f64) -> Vec<Pt> {
    let n = pts.len();
    if n < 4 {
        return pts.to_vec();
    }
    // zwei weit entfernte Ankerpunkte
    let a = 0;
    let b = (0..n)
        .max_by(|&i, &j| {
            pts[i]
                .dist(pts[a])
                .partial_cmp(&pts[j].dist(pts[a]))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(n / 2);
    let mut keep = vec![false; n];
    keep[a] = true;
    keep[b] = true;
    let (lo, hi) = (a.min(b), a.max(b));
    dp(pts, eps, &mut keep, lo, hi);
    // zweite Hälfte: von hi über das Ende bis lo
    let mut rot: Vec<Pt> = pts[hi..].to_vec();
    rot.extend_from_slice(&pts[..=lo]);
    let mut keep2 = vec![false; rot.len()];
    keep2[0] = true;
    let last = rot.len() - 1;
    keep2[last] = true;
    dp(&rot, eps, &mut keep2, 0, last);
    let mut out: Vec<Pt> = (0..n).filter(|&i| keep[i]).map(|i| pts[i]).collect();
    // Punkte der zweiten Hälfte (ohne die Ankerpunkte doppelt zu nehmen)
    let second: Vec<Pt> = (1..last).filter(|&i| keep2[i]).map(|i| rot[i]).collect();
    // zusammensetzen: erste Hälfte reicht von lo bis hi, die zweite von hi zurück zu lo
    out.extend(second);
    out
}

/// Eckpunkte als Bézierknoten: glatte Stellen bekommen Anfasser, Ecken bleiben spitz.
fn to_nodes(pts: &[Pt]) -> Vec<Node> {
    let n = pts.len();
    let mut nodes = Vec::with_capacity(n);
    for i in 0..n {
        let p = pts[i];
        let prev = pts[(i + n - 1) % n];
        let next = pts[(i + 1) % n];
        let (lp, ln) = (p.dist(prev), p.dist(next));
        if lp < 1e-9 || ln < 1e-9 {
            nodes.push(Node::corner(p));
            continue;
        }
        let din = (p - prev).normalized();
        let dout = (next - p).normalized();
        let cos = (din.x * dout.x + din.y * dout.y).clamp(-1.0, 1.0);
        let turn = cos.acos().to_degrees();
        let ratio = lp.max(ln) / lp.min(ln);
        if turn > 50.0 || ratio > 4.0 {
            nodes.push(Node::corner(p));
            continue;
        }
        let t = (next - prev).normalized();
        let hl = lp.min(ln) / 3.0;
        nodes.push(Node {
            p,
            h_in: Pt::new(p.x - t.x * hl, p.y - t.y * hl),
            h_out: Pt::new(p.x + t.x * hl, p.y + t.y * hl),
            r: 0.0,
        });
    }
    nodes
}

/// Bild nachzeichnen. `rgba`: unvormultiplizierte Bytes (R, G, B, A), `colors`: gewünschte
/// Farbanzahl (2–32). Die Ebenen kommen von unten nach oben.
pub fn trace(rgba: &[u8], w: usize, h: usize, colors: usize) -> Vec<Layer> {
    if w < 2 || h < 2 || rgba.len() < w * h * 4 {
        return Vec::new();
    }
    let pal = palette(rgba, colors);
    if pal.is_empty() {
        return Vec::new();
    }
    let labels = label(rgba, w, h, &pal);
    let mut counts = vec![0usize; pal.len()];
    for l in &labels {
        if (*l as usize) < pal.len() {
            counts[*l as usize] += 1;
        }
    }
    // Reihenfolge: größte Fläche zuerst (unten)
    let mut order: Vec<usize> = (0..pal.len()).filter(|&i| counts[i] > 0).collect();
    order.sort_by(|&a, &b| counts[b].cmp(&counts[a]));
    let mut rank = vec![usize::MAX; pal.len()];
    for (r, &i) in order.iter().enumerate() {
        rank[i] = r;
    }
    let mut layers = Vec::new();
    for (r, &ci) in order.iter().enumerate() {
        let mask: Vec<bool> = labels
            .iter()
            .map(|l| (*l as usize) < pal.len() && rank[*l as usize] >= r)
            .collect();
        let mut paths = Vec::new();
        for lp in contours(&mask, w, h) {
            let mut lp: Vec<Pt> = lp
                .into_iter()
                .map(|p| Pt::new(p.x.clamp(0.0, w as f64), p.y.clamp(0.0, h as f64)))
                .collect();
            lp.dedup_by(|a, b| a.dist(*b) < 1e-9);
            if lp.len() < 3 || area(&lp).abs() < 4.0 {
                continue;
            }
            let simple = simplify_closed(&lp, 0.55);
            if simple.len() < 3 {
                continue;
            }
            paths.push(SubPath {
                nodes: to_nodes(&simple),
                closed: true,
            });
        }
        if paths.is_empty() {
            continue;
        }
        let c = pal[ci];
        layers.push(Layer {
            color: [
                c[0].round().clamp(0.0, 255.0) as u8,
                c[1].round().clamp(0.0, 255.0) as u8,
                c[2].round().clamp(0.0, 255.0) as u8,
            ],
            paths,
        });
    }
    layers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flatten(sp: &SubPath) -> Vec<Pt> {
        let mut out = vec![sp.nodes[0].p];
        for (a, c1, c2, b) in sp.segments() {
            for i in 1..=8 {
                let t = i as f64 / 8.0;
                let u = 1.0 - t;
                out.push(Pt::new(
                    u * u * u * a.x + 3.0 * u * u * t * c1.x + 3.0 * u * t * t * c2.x + t * t * t * b.x,
                    u * u * u * a.y + 3.0 * u * u * t * c1.y + 3.0 * u * t * t * c2.y + t * t * t * b.y,
                ));
            }
        }
        out
    }

    fn inside_evenodd(paths: &[SubPath], p: Pt) -> bool {
        let mut c = false;
        for sp in paths {
            let poly = flatten(sp);
            let n = poly.len();
            for i in 0..n {
                let (a, b) = (poly[i], poly[(i + 1) % n]);
                if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
                    c = !c;
                }
            }
        }
        c
    }

    /// Weißer Grund, roter Kreis, blaues Quadrat darin, grüner Balken mit Loch.
    fn logo() -> (Vec<u8>, usize, usize) {
        let (w, h) = (80, 60);
        let mut v = vec![255u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let o = (y * w + x) * 4;
                let (dx, dy) = (x as f64 - 25.0, y as f64 - 30.0);
                let mut c = [255u8, 255, 255];
                if dx * dx + dy * dy < 20.0 * 20.0 {
                    c = [220, 30, 30];
                }
                if (20..32).contains(&x) && (24..36).contains(&y) {
                    c = [20, 40, 200];
                }
                if (50..75).contains(&x) && (10..50).contains(&y) {
                    c = [30, 160, 60];
                }
                let (hx, hy) = (x as f64 - 62.0, y as f64 - 30.0);
                if hx * hx + hy * hy < 5.0 * 5.0 {
                    c = [255, 255, 255];
                }
                v[o..o + 3].copy_from_slice(&c);
            }
        }
        (v, w, h)
    }

    #[test]
    fn traces_logo_faithfully() {
        let (img, w, h) = logo();
        let layers = trace(&img, w, h, 4);
        assert_eq!(layers.len(), 4, "vier Farben");
        // Neu zeichnen (von unten nach oben) und mit dem Original vergleichen
        let mut ok = 0;
        for y in 0..h {
            for x in 0..w {
                let p = Pt::new(x as f64 + 0.5, y as f64 + 0.5);
                let mut col = None;
                for l in &layers {
                    if inside_evenodd(&l.paths, p) {
                        col = Some(l.color);
                    }
                }
                let o = (y * w + x) * 4;
                let want = [img[o], img[o + 1], img[o + 2]];
                if let Some(c) = col {
                    let d: i32 = (0..3).map(|i| (c[i] as i32 - want[i] as i32).abs()).sum();
                    if d < 40 {
                        ok += 1;
                    }
                }
            }
        }
        let share = ok as f64 / (w * h) as f64;
        assert!(share > 0.97, "{share}");
    }

    #[test]
    fn transparent_background_is_left_out() {
        let (w, h) = (30, 30);
        let mut v = vec![0u8; w * h * 4];
        for y in 8..22 {
            for x in 8..22 {
                let o = (y * w + x) * 4;
                v[o..o + 4].copy_from_slice(&[10, 120, 200, 255]);
            }
        }
        let layers = trace(&v, w, h, 3);
        assert_eq!(layers.len(), 1);
        assert!(inside_evenodd(&layers[0].paths, Pt::new(15.0, 15.0)));
        assert!(!inside_evenodd(&layers[0].paths, Pt::new(3.0, 3.0)));
    }

    #[test]
    fn straight_edges_stay_straight_and_corners_sharp() {
        let (w, h) = (40, 40);
        let mut v = vec![255u8; w * h * 4];
        for y in 10..30 {
            for x in 10..30 {
                let o = (y * w + x) * 4;
                v[o..o + 4].copy_from_slice(&[0, 0, 0, 255]);
            }
        }
        let layers = trace(&v, w, h, 2);
        // oberste Ebene = Quadrat: wenige Knoten
        let top = layers.last().unwrap();
        let n: usize = top.paths.iter().map(|p| p.nodes.len()).sum();
        assert!(n <= 12, "{n} Knoten");
    }

    #[test]
    fn empty_and_tiny_inputs() {
        assert!(trace(&[], 0, 0, 4).is_empty());
        assert!(trace(&[0, 0, 0, 0], 1, 1, 4).is_empty());
    }
}
