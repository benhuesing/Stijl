//! Grundlegende Geometrie: Punkte, Rechtecke, affine Transformationen und Kurvenhilfen.

use std::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

impl Pt {
    pub const fn new(x: f64, y: f64) -> Self {
        Pt { x, y }
    }
    pub fn dist(self, o: Pt) -> f64 {
        (self - o).len()
    }
    pub fn len(self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
    pub fn normalized(self) -> Pt {
        let l = self.len();
        if l < 1e-12 {
            Pt::new(0.0, 0.0)
        } else {
            Pt::new(self.x / l, self.y / l)
        }
    }
    pub fn lerp(self, o: Pt, t: f64) -> Pt {
        Pt::new(self.x + (o.x - self.x) * t, self.y + (o.y - self.y) * t)
    }
}

impl Add for Pt {
    type Output = Pt;
    fn add(self, o: Pt) -> Pt {
        Pt::new(self.x + o.x, self.y + o.y)
    }
}
impl Sub for Pt {
    type Output = Pt;
    fn sub(self, o: Pt) -> Pt {
        Pt::new(self.x - o.x, self.y - o.y)
    }
}
impl Mul<f64> for Pt {
    type Output = Pt;
    fn mul(self, s: f64) -> Pt {
        Pt::new(self.x * s, self.y * s)
    }
}

/// Achsenparalleles Rechteck (x0 <= x1, y0 <= y1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Rect {
    pub fn from_points(a: Pt, b: Pt) -> Rect {
        Rect {
            x0: a.x.min(b.x),
            y0: a.y.min(b.y),
            x1: a.x.max(b.x),
            y1: a.y.max(b.y),
        }
    }
    pub fn from_xywh(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect::from_points(Pt::new(x, y), Pt::new(x + w, y + h))
    }
    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> f64 {
        self.y1 - self.y0
    }
    pub fn union(&self, o: &Rect) -> Rect {
        Rect {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }
    pub fn include(&mut self, p: Pt) {
        self.x0 = self.x0.min(p.x);
        self.y0 = self.y0.min(p.y);
        self.x1 = self.x1.max(p.x);
        self.y1 = self.y1.max(p.y);
    }
    pub fn contains_rect(&self, o: &Rect) -> bool {
        o.x0 >= self.x0 && o.y0 >= self.y0 && o.x1 <= self.x1 && o.y1 <= self.y1
    }

    pub fn contains(&self, p: Pt) -> bool {
        p.x >= self.x0 && p.x <= self.x1 && p.y >= self.y0 && p.y <= self.y1
    }
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x0 <= o.x1 && o.x0 <= self.x1 && self.y0 <= o.y1 && o.y0 <= self.y1
    }
    pub fn inflate(&self, d: f64) -> Rect {
        Rect {
            x0: self.x0 - d,
            y0: self.y0 - d,
            x1: self.x1 + d,
            y1: self.y1 + d,
        }
    }
}

/// Affine Matrix in SVG-Konvention: x' = a·x + c·y + e, y' = b·x + d·y + f
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Default for Affine {
    fn default() -> Self {
        Affine::IDENTITY
    }
}

impl Affine {
    pub const IDENTITY: Affine = Affine {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub fn translate(tx: f64, ty: f64) -> Affine {
        Affine {
            e: tx,
            f: ty,
            ..Affine::IDENTITY
        }
    }
    pub fn scale(sx: f64, sy: f64) -> Affine {
        Affine {
            a: sx,
            d: sy,
            ..Affine::IDENTITY
        }
    }
    pub fn rotate_deg(deg: f64) -> Affine {
        let r = deg.to_radians();
        let (s, c) = r.sin_cos();
        Affine {
            a: c,
            b: s,
            c: -s,
            d: c,
            e: 0.0,
            f: 0.0,
        }
    }
    /// Skaliert um einen Ankerpunkt.
    pub fn scale_about(sx: f64, sy: f64, anchor: Pt) -> Affine {
        Affine::translate(anchor.x, anchor.y)
            .then_before(&Affine::scale(sx, sy))
            .then_before(&Affine::translate(-anchor.x, -anchor.y))
    }

    /// `self * o`: erst `o` anwenden, dann `self` (wie SVG-Transformlisten von rechts nach links).
    pub fn then_before(&self, o: &Affine) -> Affine {
        Affine {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            e: self.a * o.e + self.c * o.f + self.e,
            f: self.b * o.e + self.d * o.f + self.f,
        }
    }

    /// Umkehrabbildung (bei nicht invertierbarer Matrix die Identität).
    pub fn invert(&self) -> Affine {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-12 {
            return Affine::IDENTITY;
        }
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        Affine {
            a,
            b,
            c,
            d,
            e: -(a * self.e + c * self.f),
            f: -(b * self.e + d * self.f),
        }
    }

    /// Drehung um einen Punkt (Grad, im Uhrzeigersinn auf dem Bildschirm).
    pub fn rotate_about(deg: f64, center: Pt) -> Affine {
        Affine::translate(center.x, center.y)
            .then_before(&Affine::rotate_deg(deg))
            .then_before(&Affine::translate(-center.x, -center.y))
    }

    pub fn apply(&self, p: Pt) -> Pt {
        Pt::new(
            self.a * p.x + self.c * p.y + self.e,
            self.b * p.x + self.d * p.y + self.f,
        )
    }

    pub fn is_identity(&self) -> bool {
        (self.a - 1.0).abs() < 1e-9
            && self.b.abs() < 1e-9
            && self.c.abs() < 1e-9
            && (self.d - 1.0).abs() < 1e-9
            && self.e.abs() < 1e-9
            && self.f.abs() < 1e-9
    }

    /// Keine Drehung/Scherung: Rechtecke bleiben Rechtecke.
    pub fn is_axis_aligned(&self) -> bool {
        self.b.abs() < 1e-9 && self.c.abs() < 1e-9
    }

    /// Mittlerer Skalierungsfaktor (für Strichstärken und Schriftgrößen).
    pub fn mean_scale(&self) -> f64 {
        (self.a * self.d - self.b * self.c).abs().sqrt()
    }

    /// Liest ein SVG-`transform`-Attribut.
    pub fn parse_svg(s: &str) -> Affine {
        let mut result = Affine::IDENTITY;
        let mut rest = s;
        while let Some(open) = rest.find('(') {
            let name = rest[..open]
                .trim()
                .trim_start_matches(',')
                .trim()
                .to_ascii_lowercase();
            let Some(close_rel) = rest[open..].find(')') else {
                break;
            };
            let args: Vec<f64> = parse_numbers(&rest[open + 1..open + close_rel]);
            rest = &rest[open + close_rel + 1..];
            let get = |i: usize, def: f64| args.get(i).copied().unwrap_or(def);
            let m = match name.as_str() {
                "matrix" if args.len() >= 6 => Affine {
                    a: args[0],
                    b: args[1],
                    c: args[2],
                    d: args[3],
                    e: args[4],
                    f: args[5],
                },
                "translate" => Affine::translate(get(0, 0.0), get(1, 0.0)),
                "scale" => {
                    let sx = get(0, 1.0);
                    Affine::scale(sx, get(1, sx))
                }
                "rotate" => {
                    let r = Affine::rotate_deg(get(0, 0.0));
                    if args.len() >= 3 {
                        Affine::translate(args[1], args[2])
                            .then_before(&r)
                            .then_before(&Affine::translate(-args[1], -args[2]))
                    } else {
                        r
                    }
                }
                "skewx" => Affine {
                    c: get(0, 0.0).to_radians().tan(),
                    ..Affine::IDENTITY
                },
                "skewy" => Affine {
                    b: get(0, 0.0).to_radians().tan(),
                    ..Affine::IDENTITY
                },
                _ => Affine::IDENTITY,
            };
            result = result.then_before(&m);
        }
        result
    }
}

/// Zerlegt eine Liste von Zahlen (Kommas, Leerzeichen, "1-2", ".5.5").
pub fn parse_numbers(s: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let mut lex = NumLexer::new(s);
    while let Some(n) = lex.next_number() {
        out.push(n);
    }
    out
}

/// Kleiner Zahlen-Lexer, auch für SVG-Pfaddaten genutzt.
pub struct NumLexer<'a> {
    pub s: &'a [u8],
    pub i: usize,
}

impl<'a> NumLexer<'a> {
    pub fn new(s: &'a str) -> Self {
        NumLexer {
            s: s.as_bytes(),
            i: 0,
        }
    }
    pub fn skip_sep(&mut self) {
        while self.i < self.s.len() && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',') {
            self.i += 1;
        }
    }
    pub fn peek(&mut self) -> Option<u8> {
        self.skip_sep();
        self.s.get(self.i).copied()
    }
    /// Einzelnes Flag (0/1) für Bogen-Befehle, darf ohne Trennzeichen stehen.
    pub fn next_flag(&mut self) -> Option<bool> {
        self.skip_sep();
        match self.s.get(self.i) {
            Some(b'0') => {
                self.i += 1;
                Some(false)
            }
            Some(b'1') => {
                self.i += 1;
                Some(true)
            }
            _ => None,
        }
    }
    pub fn next_number(&mut self) -> Option<f64> {
        self.skip_sep();
        let start = self.i;
        let s = self.s;
        let mut j = self.i;
        if j < s.len() && (s[j] == b'+' || s[j] == b'-') {
            j += 1;
        }
        let mut digits = 0;
        while j < s.len() && s[j].is_ascii_digit() {
            j += 1;
            digits += 1;
        }
        if j < s.len() && s[j] == b'.' {
            j += 1;
            while j < s.len() && s[j].is_ascii_digit() {
                j += 1;
                digits += 1;
            }
        }
        if digits == 0 {
            return None;
        }
        if j < s.len() && (s[j] == b'e' || s[j] == b'E') {
            let mut k = j + 1;
            if k < s.len() && (s[k] == b'+' || s[k] == b'-') {
                k += 1;
            }
            if k < s.len() && s[k].is_ascii_digit() {
                while k < s.len() && s[k].is_ascii_digit() {
                    k += 1;
                }
                j = k;
            }
        }
        self.i = j;
        std::str::from_utf8(&s[start..j]).ok()?.parse().ok()
    }
}

/// Begrenzungsrahmen einer kubischen Bézierkurve (exakt über Extremstellen).
pub fn cubic_bbox(p0: Pt, p1: Pt, p2: Pt, p3: Pt) -> Rect {
    let mut r = Rect::from_points(p0, p3);
    for axis in 0..2 {
        let (a0, a1, a2, a3) = if axis == 0 {
            (p0.x, p1.x, p2.x, p3.x)
        } else {
            (p0.y, p1.y, p2.y, p3.y)
        };
        // Ableitung: 3[(−a0+3a1−3a2+a3)t² + 2(a0−2a1+a2)t + (a1−a0)]
        let a = -a0 + 3.0 * a1 - 3.0 * a2 + a3;
        let b = 2.0 * (a0 - 2.0 * a1 + a2);
        let c = a1 - a0;
        let mut ts = Vec::with_capacity(2);
        if a.abs() < 1e-12 {
            if b.abs() > 1e-12 {
                ts.push(-c / b);
            }
        } else {
            let disc = b * b - 4.0 * a * c;
            if disc >= 0.0 {
                let sq = disc.sqrt();
                ts.push((-b + sq) / (2.0 * a));
                ts.push((-b - sq) / (2.0 * a));
            }
        }
        for t in ts {
            if t > 0.0 && t < 1.0 {
                r.include(cubic_point(p0, p1, p2, p3, t));
            }
        }
    }
    r
}

pub fn cubic_point(p0: Pt, p1: Pt, p2: Pt, p3: Pt, t: f64) -> Pt {
    let u = 1.0 - t;
    p0 * (u * u * u) + p1 * (3.0 * u * u * t) + p2 * (3.0 * u * t * t) + p3 * (t * t * t)
}

/// Ramer-Douglas-Peucker: vereinfacht eine Punktfolge.
pub fn simplify(points: &[Pt], eps: f64) -> Vec<Pt> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0usize, points.len() - 1)];
    while let Some((s, e)) = stack.pop() {
        let (a, b) = (points[s], points[e]);
        let ab = b - a;
        let len = ab.len();
        let mut max_d = 0.0;
        let mut idx = s;
        for (i, p) in points.iter().enumerate().take(e).skip(s + 1) {
            let d = if len < 1e-12 {
                p.dist(a)
            } else {
                ((*p - a).x * ab.y - (*p - a).y * ab.x).abs() / len
            };
            if d > max_d {
                max_d = d;
                idx = i;
            }
        }
        if max_d > eps {
            keep[idx] = true;
            stack.push((s, idx));
            stack.push((idx, e));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect()
}

/// Bogen (SVG-Endpunktparametrisierung) in kubische Segmente umwandeln.
/// Liefert eine Liste von (Kontrollpunkt1, Kontrollpunkt2, Endpunkt).
pub fn arc_to_cubics(
    p0: Pt,
    rx: f64,
    ry: f64,
    phi_deg: f64,
    large: bool,
    sweep: bool,
    p1: Pt,
) -> Vec<(Pt, Pt, Pt)> {
    if p0.dist(p1) < 1e-12 {
        return Vec::new();
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-12 || ry < 1e-12 {
        return vec![(p0.lerp(p1, 1.0 / 3.0), p0.lerp(p1, 2.0 / 3.0), p1)];
    }
    let phi = phi_deg.to_radians();
    let (sin_phi, cos_phi) = phi.sin_cos();
    let dx = (p0.x - p1.x) / 2.0;
    let dy = (p0.y - p1.y) / 2.0;
    let x1p = cos_phi * dx + sin_phi * dy;
    let y1p = -sin_phi * dx + cos_phi * dy;
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut coef = if den.abs() < 1e-12 {
        0.0
    } else {
        (num / den).max(0.0).sqrt()
    };
    if large == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * y1p / ry;
    let cyp = -coef * ry * x1p / rx;
    let cx = cos_phi * cxp - sin_phi * cyp + (p0.x + p1.x) / 2.0;
    let cy = sin_phi * cxp + cos_phi * cyp + (p0.y + p1.y) / 2.0;

    let angle = |ux: f64, uy: f64, vx: f64, vy: f64| -> f64 {
        let dot = ux * vx + uy * vy;
        let len = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
        let mut a = (dot / len).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 {
            a = -a;
        }
        a
    };
    let theta1 = angle(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
    let mut dtheta = angle(
        (x1p - cxp) / rx,
        (y1p - cyp) / ry,
        (-x1p - cxp) / rx,
        (-y1p - cyp) / ry,
    );
    if !sweep && dtheta > 0.0 {
        dtheta -= 2.0 * std::f64::consts::PI;
    } else if sweep && dtheta < 0.0 {
        dtheta += 2.0 * std::f64::consts::PI;
    }

    let segs = (dtheta.abs() / (std::f64::consts::FRAC_PI_2) - 1e-9)
        .ceil()
        .max(1.0) as usize;
    let delta = dtheta / segs as f64;
    let k = 4.0 / 3.0 * (delta / 4.0).tan();
    let point = |t: f64| -> (Pt, Pt) {
        let (s, c) = t.sin_cos();
        let p = Pt::new(
            cx + rx * c * cos_phi - ry * s * sin_phi,
            cy + rx * c * sin_phi + ry * s * cos_phi,
        );
        let d = Pt::new(
            -rx * s * cos_phi - ry * c * sin_phi,
            -rx * s * sin_phi + ry * c * cos_phi,
        );
        (p, d)
    };
    let mut out = Vec::with_capacity(segs);
    let mut t = theta1;
    for i in 0..segs {
        let (pa, da) = point(t);
        let t2 = t + delta;
        let (mut pb, db) = point(t2);
        if i == segs - 1 {
            pb = p1;
        }
        out.push((pa + da * k, pb - db * k, pb));
        t = t2;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_parse() {
        let m = Affine::parse_svg("translate(10,20) scale(2)");
        assert_eq!(m.apply(Pt::new(1.0, 1.0)), Pt::new(12.0, 22.0));
        let r = Affine::parse_svg("rotate(90)");
        let p = r.apply(Pt::new(1.0, 0.0));
        assert!((p.x).abs() < 1e-9 && (p.y - 1.0).abs() < 1e-9);
    }

    #[test]
    fn inverse() {
        let m = Affine::parse_svg("translate(5 7) rotate(30) scale(2 3)");
        let p = Pt::new(3.0, -4.0);
        let q = m.invert().apply(m.apply(p));
        assert!(q.dist(p) < 1e-9);
    }

    #[test]
    fn numbers() {
        assert_eq!(parse_numbers("1-2.5.5e1,3"), vec![1.0, -2.5, 5.0, 3.0]);
    }

    #[test]
    fn bbox_of_curve() {
        let r = cubic_bbox(
            Pt::new(0.0, 0.0),
            Pt::new(0.0, 10.0),
            Pt::new(10.0, 10.0),
            Pt::new(10.0, 0.0),
        );
        assert!((r.y1 - 7.5).abs() < 1e-9);
    }

    #[test]
    fn rdp() {
        let pts: Vec<Pt> = (0..=10).map(|i| Pt::new(i as f64, 0.0)).collect();
        assert_eq!(simplify(&pts, 0.1).len(), 2);
    }

    #[test]
    fn arc_half_circle() {
        let segs = arc_to_cubics(Pt::new(0.0, 0.0), 5.0, 5.0, 0.0, false, true, Pt::new(10.0, 0.0));
        assert_eq!(segs.len(), 2);
        let mid = segs[0].2;
        assert!((mid.x - 5.0).abs() < 1e-6 && (mid.y.abs() - 5.0).abs() < 1e-6);
    }
}
