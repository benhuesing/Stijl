// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! Pixelfilter für Bilder (ohne GTK): Farbfilter und Kunsteffekte.
//!
//! Die Puffer haben das Format von Cairo (ARGB32, vormultipliziert; als `u32` in der
//! Byte-Reihenfolge des Rechners gelesen). Die Deckkraft bleibt bei allen Filtern unverändert.

use crate::model::{ImgFilter, Lens, Material};

pub struct Buf<'a> {
    pub data: &'a mut [u8],
    pub w: usize,
    pub h: usize,
    pub stride: usize,
}

impl Buf<'_> {
    fn px(&self, x: usize, y: usize) -> u32 {
        let o = y * self.stride + x * 4;
        u32::from_ne_bytes([self.data[o], self.data[o + 1], self.data[o + 2], self.data[o + 3]])
    }
    fn set(&mut self, x: usize, y: usize, v: u32) {
        let o = y * self.stride + x * 4;
        self.data[o..o + 4].copy_from_slice(&v.to_ne_bytes());
    }
}

/// Aus einem vormultiplizierten Pixel: (Alpha 0–255, unvormultipliziertes RGB 0–1).
fn split(p: u32) -> (u32, [f32; 3]) {
    let a = p >> 24;
    if a == 0 {
        return (0, [0.0; 3]);
    }
    let f = 1.0 / a as f32;
    (
        a,
        [
            ((p >> 16) & 0xFF) as f32 * f,
            ((p >> 8) & 0xFF) as f32 * f,
            (p & 0xFF) as f32 * f,
        ],
    )
}

fn join(a: u32, c: [f32; 3]) -> u32 {
    let af = a as f32;
    let ch = |v: f32| ((v.clamp(0.0, 1.0) * af).round() as u32).min(a);
    (a << 24) | (ch(c[0]) << 16) | (ch(c[1]) << 8) | ch(c[2])
}

fn map_colors(b: &mut Buf, f: impl Fn(usize, usize, [f32; 3]) -> [f32; 3]) {
    for y in 0..b.h {
        for x in 0..b.w {
            let (a, c) = split(b.px(x, y));
            if a == 0 {
                continue;
            }
            let n = f(x, y, c);
            b.set(x, y, join(a, n));
        }
    }
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Farbfilter mit Stärke `k` (0–1): 0 = Original, 1 = volle Wirkung.
pub fn tint(b: &mut Buf, filter: ImgFilter, k: f64) {
    let k = k.clamp(0.0, 1.0) as f32;
    map_colors(b, |_, _, c| {
        let [r, g, bl] = c;
        match filter {
            ImgFilter::Sepia => {
                let s = [
                    (0.393 * r + 0.769 * g + 0.189 * bl).min(1.0),
                    (0.349 * r + 0.686 * g + 0.168 * bl).min(1.0),
                    (0.272 * r + 0.534 * g + 0.131 * bl).min(1.0),
                ];
                mix(c, s, k)
            }
            ImgFilter::Mono => {
                let l = 0.299 * r + 0.587 * g + 0.114 * bl;
                mix(c, [l, l, l], k)
            }
            ImgFilter::Invert => mix(c, [1.0 - r, 1.0 - g, 1.0 - bl], k),
        }
    });
}

/// Farbtemperatur: `t` von −1 (kalt, bläulich) über 0 (Original) bis 1 (warm, gelblich-rot).
pub fn temperature(b: &mut Buf, t: f64) {
    let t = t.clamp(-1.0, 1.0) as f32;
    if t.abs() < 1e-6 {
        return;
    }
    map_colors(b, |_, _, c| {
        let [r, g, bl] = c;
        if t > 0.0 {
            [
                (r * (1.0 + 0.18 * t) + 0.06 * t).min(1.0),
                (g * (1.0 + 0.04 * t)).min(1.0),
                bl * (1.0 - 0.28 * t),
            ]
        } else {
            let k = -t;
            [
                r * (1.0 - 0.28 * k),
                (g * (1.0 + 0.02 * k)).min(1.0),
                (bl * (1.0 + 0.18 * k) + 0.06 * k).min(1.0),
            ]
        }
    });
}

/// Mosaik aus Blöcken der Kantenlänge `cell` (Mittelwert je Block).
pub fn pixelate(b: &mut Buf, cell: usize) {
    let cell = cell.max(1);
    if cell == 1 {
        return;
    }
    let mut y0 = 0;
    while y0 < b.h {
        let y1 = (y0 + cell).min(b.h);
        let mut x0 = 0;
        while x0 < b.w {
            let x1 = (x0 + cell).min(b.w);
            let (mut sa, mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64, 0u64);
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = b.px(x, y);
                    sa += (p >> 24) as u64;
                    sr += ((p >> 16) & 0xFF) as u64;
                    sg += ((p >> 8) & 0xFF) as u64;
                    sb += (p & 0xFF) as u64;
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            // Deckkraft bleibt je Pixel erhalten: Farbe mitteln, Alpha pro Pixel zurücksetzen
            let (a_avg, r, g, bl) = (sa / n, sr / n, sg / n, sb / n);
            for y in y0..y1 {
                for x in x0..x1 {
                    let a = b.px(x, y) >> 24;
                    let v = if a_avg == 0 {
                        0
                    } else {
                        let f = a as f32 / a_avg as f32;
                        let ch = |c: u64| ((c as f32 * f).round() as u32).min(a);
                        (a << 24) | (ch(r) << 16) | (ch(g) << 8) | ch(bl)
                    };
                    b.set(x, y, v);
                }
            }
            x0 = x1;
        }
        y0 = y1;
    }
}

fn hash2(x: i64, y: i64, salt: u32) -> f32 {
    let mut h =
        (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= (salt as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    (h & 0xFFFF) as f32 / 65535.0
}

fn value_noise(x: f32, y: f32, scale: f32, salt: u32) -> f32 {
    let (fx, fy) = (x / scale, y / scale);
    let (i, j) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - i, fy - j);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (sx, sy) = (s(tx), s(ty));
    let (i, j) = (i as i64, j as i64);
    let (a, b2, c, d) = (
        hash2(i, j, salt),
        hash2(i + 1, j, salt),
        hash2(i, j + 1, salt),
        hash2(i + 1, j + 1, salt),
    );
    let top = a + (b2 - a) * sx;
    let bot = c + (d - c) * sx;
    top + (bot - top) * sy
}

/// Körnung: Helligkeitsrauschen, Stärke `k` (0–1).
pub fn grain(b: &mut Buf, k: f64) {
    let amp = (0.06 + 0.4 * k.clamp(0.0, 1.0)) as f32;
    map_colors(b, |x, y, c| {
        let n = (hash2(x as i64, y as i64, 7) - 0.5) * amp;
        [c[0] + n, c[1] + n, c[2] + n]
    });
}

fn blur_plane(p: &mut [f32], w: usize, h: usize, r: usize) {
    if r == 0 || w == 0 || h == 0 {
        return;
    }
    let mut tmp = vec![0f32; w.max(h)];
    let norm = 1.0 / (2 * r + 1) as f32;
    // waagerecht
    for y in 0..h {
        let row = &mut p[y * w..(y + 1) * w];
        let at = |i: isize| row[i.clamp(0, w as isize - 1) as usize];
        let mut sum = 0.0;
        for i in -(r as isize)..=(r as isize) {
            sum += at(i);
        }
        for x in 0..w {
            tmp[x] = sum * norm;
            sum += at(x as isize + r as isize + 1) - at(x as isize - r as isize);
        }
        row.copy_from_slice(&tmp[..w]);
    }
    // senkrecht
    for x in 0..w {
        let at = |p: &[f32], i: isize| p[i.clamp(0, h as isize - 1) as usize * w + x];
        let mut sum = 0.0;
        for i in -(r as isize)..=(r as isize) {
            sum += at(p, i);
        }
        for y in 0..h {
            tmp[y] = sum * norm;
            sum += at(p, y as isize + r as isize + 1) - at(p, y as isize - r as isize);
        }
        for y in 0..h {
            p[y * w + x] = tmp[y];
        }
    }
}

// ───────────── Wachsmalstifte, Ölfarbe, Riffelglas ─────────────
//
// Die Muster hängen von Koordinaten in „Einheiten“ ab (Dokumenteinheiten bei Formen,
// Pixeln bei Bildern), damit sie beim Verschieben und Zoomen auf dem Objekt bleiben.

/// Wachsmalstift an der Stelle (x, y): (Papieranteil 0–1, Helligkeitsfaktor). Das Wachs
/// bleibt an den Höhen der Papierstruktur hängen; dazwischen scheint das Papier durch, in
/// schrägen Strichen wie beim Malen.
pub fn crayon_px(x: f32, y: f32, k: f32) -> (f32, f32) {
    let k = k.clamp(0.0, 1.0);
    // Striche schräg (etwa 30°): entlang lang, quer kurz
    let (c, s) = (0.866_f32, 0.5_f32);
    let u = x * c + y * s;
    let v = -x * s + y * c;
    let streak = value_noise(u, v * 7.0, 28.0, 21) * 0.6 + value_noise(u, v * 5.0, 9.0, 22) * 0.4;
    let tooth = value_noise(x, y, 1.4, 23) * 0.55 + value_noise(x, y, 3.2, 24) * 0.45;
    let t = tooth * 0.6 + streak * 0.4;
    // je stärker, desto mehr Papier
    let thr = 0.78 - 0.33 * k;
    let paper = ((t - thr) / 0.12).clamp(0.0, 1.0) * (0.35 + 0.6 * k);
    let shade = 1.0 - (streak - 0.5) * 0.22 * k;
    (paper, shade)
}

/// Ölfarbe an der Stelle (x, y): Helligkeitsfaktor durch kurze Pinselstriche (je Tupfer
/// eine Richtung, Borstenrillen mit leichtem Relief, von oben links beleuchtet) und leichte
/// Farbschwankungen; `size` = Breite eines Strichs.
pub fn oil_px(x: f32, y: f32, k: f32, size: f32) -> f32 {
    let k = k.clamp(0.0, 1.0);
    let size = size.max(1.0);
    // Tupfer: nächster von zufällig versetzten Mittelpunkten eines Rasters
    let cell = size * 2.4;
    let (ci, cj) = ((x / cell).floor() as i64, (y / cell).floor() as i64);
    let mut best = (f32::MAX, 0.0f32, 0.0f32, 0i64, 0i64);
    for dj in -1..=1 {
        for di in -1..=1 {
            let (i, j) = (ci + di, cj + dj);
            let cx = (i as f32 + 0.15 + 0.7 * hash2(i, j, 35)) * cell;
            let cy = (j as f32 + 0.15 + 0.7 * hash2(i, j, 36)) * cell;
            let d = (x - cx).powi(2) + (y - cy).powi(2);
            if d < best.0 {
                best = (d, cx, cy, i, j);
            }
        }
    }
    let (_, cx, cy, i, j) = best;
    // Richtung des Tupfers: meist ähnlich (wie mit einer Hand gemalt), etwas gestreut
    let a = 0.6 + (hash2(i, j, 37) - 0.5) * 1.6;
    let (c, s) = (a.cos(), a.sin());
    let height = |px: f32, py: f32| {
        let u = (px - cx) * c + (py - cy) * s;
        let v = -(px - cx) * s + (py - cy) * c;
        // Borstenrillen quer zum Strich, entlang des Strichs langsam veränderlich
        value_noise(u, v * 7.0, size * 1.4, 32) * 0.7 + value_noise(u, v * 2.0, size * 0.6, 33) * 0.3
    };
    let e = size * 0.08;
    let light = ((height(x, y) - height(x + e, y + e)) * 5.0).clamp(-1.0, 1.0);
    let tone = (hash2(i, j, 34) - 0.5) * 0.14;
    1.0 + k * (light * 0.3 + tone)
}

/// Rasterpunkt an der Stelle (x, y): Deckung 0–1. Punkte im Abstand `cell`, um `angle`
/// (Bogenmaß) gedreht; `fill` (0–1) bestimmt die Punktgröße (1 = Punkte berühren sich
/// fast), `aa` ist die Breite eines Pixels in denselben Einheiten (Kantenglättung).
pub fn halftone_cov(x: f32, y: f32, cell: f32, angle: f32, fill: f32, aa: f32) -> f32 {
    let cell = cell.max(0.5);
    let (c, s) = (angle.cos(), angle.sin());
    let u = (x * c + y * s) / cell;
    let v = (-x * s + y * c) / cell;
    let du = (u - u.round()) * cell;
    let dv = (v - v.round()) * cell;
    let d = (du * du + dv * dv).sqrt();
    // Fläche eines Kreises mit r = 0,5·cell·√(4/π·fill) deckt `fill` der Zelle
    let r = cell * 0.5 * (fill.clamp(0.0, 1.0) * 1.273).sqrt();
    ((r - d) / aa.max(1e-3) + 0.5).clamp(0.0, 1.0)
}

/// Körnung der Risografie an der Stelle (x, y): wie viel Farbe fehlt (0 … etwa 0,4·k).
pub fn riso_grain(x: f32, y: f32, k: f32) -> f32 {
    let k = k.clamp(0.0, 1.0);
    let fine = hash2((x * 1.7).floor() as i64, (y * 1.7).floor() as i64, 61);
    let blot = value_noise(x, y, 6.0, 62);
    ((fine * 0.6 + blot * 0.4 - 0.35) * 0.9 * k).clamp(0.0, 0.45)
}

/// Rasterpunkte auf einem Bild: Vierfarbraster wie im Druck (Cyan, Magenta, Gelb und
/// Schwarz unter verschiedenen Winkeln), so bleiben Farben und Kontraste erhalten.
/// `cell` = Punktabstand in Pixeln, `angle` = Winkel des Schwarzrasters in Grad.
pub fn halftone(b: &mut Buf, k: f64, cell: f64, angle: f64) {
    let k = k.clamp(0.0, 1.0) as f32;
    let cell = cell.max(1.5) as f32;
    let base = (angle as f32).to_radians();
    // übliche Rasterwinkel relativ zum Schwarz (45°): C 15°, M 75°, Y 0°
    let off = |deg: f32| base + (deg - 45.0).to_radians();
    let (ac, am, ay, ak) = (off(15.0), off(75.0), off(0.0), base);
    map_colors(b, |x, y, c| {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let (cc, mm, yy) = (1.0 - c[0], 1.0 - c[1], 1.0 - c[2]);
        let kk = cc.min(mm).min(yy) * 0.8;
        let s = |v: f32| ((v - kk) / (1.0 - kk).max(1e-3)).clamp(0.0, 1.0);
        let cov = |v: f32, a: f32| halftone_cov(px, py, cell, a, v, 1.0);
        let (ic, im, iy) = (cov(s(cc), ac), cov(s(mm), am), cov(s(yy), ay));
        let ik = cov(kk, ak);
        let out = [
            (1.0 - ic) * (1.0 - ik),
            (1.0 - im) * (1.0 - ik),
            (1.0 - iy) * (1.0 - ik),
        ];
        mix(c, out, (0.4 + 0.6 * k).min(1.0))
    });
}

/// Risografie auf einem Bild: Farbkanäle leicht gegeneinander verschoben, wenige Tonstufen
/// und Körnung.
pub fn riso(b: &mut Buf, k: f64) {
    let (w, h) = (b.w, b.h);
    if w < 3 || h < 3 {
        return;
    }
    let kf = k.clamp(0.0, 1.0) as f32;
    let scale = (w.min(h) as f32 / 400.0).max(1.0);
    let shift = ((1.0 + 3.0 * kf) * scale).round() as isize;
    let src: Vec<(u32, [f32; 3])> = (0..w * h).map(|i| split(b.px(i % w, i / w))).collect();
    let at = |x: isize, y: isize| {
        src[(y.clamp(0, h as isize - 1) as usize) * w + x.clamp(0, w as isize - 1) as usize]
    };
    let levels = 9.0 - 4.0 * kf;
    for y in 0..h {
        for x in 0..w {
            let (a, c) = src[y * w + x];
            if a == 0 {
                continue;
            }
            let (xi, yi) = (x as isize, y as isize);
            let r = at(xi - shift, yi).1[0];
            let bl = at(xi + shift, yi + shift / 2).1[2];
            let g = c[1];
            let grain = riso_grain(x as f32 / scale, y as f32 / scale, kf);
            let mut out = [r, g, bl];
            for v in out.iter_mut() {
                let p = (*v * levels).round() / levels;
                *v = (p + (1.0 - p) * grain).clamp(0.0, 1.0);
            }
            b.set(x, y, join(a, out));
        }
    }
}

// ───────────── Kunsteffekte auf Bildern ─────────────
//
// Damit ein Foto erkennbar bleibt und wie ein gemaltes Bild wirkt, arbeiten die Filter
// kantenerhaltend: Flächen werden vereinfacht (Kuwahara-Filter), Umrisse bleiben scharf
// oder werden als Linien betont, und die Farben werden eher kräftiger als blasser.

/// Pixel eines Puffers: (Deckkraft, unvormultipliziertes RGB).
fn read_all(b: &Buf) -> (Vec<u32>, Vec<[f32; 3]>) {
    let n = b.w * b.h;
    let mut alpha = vec![0u32; n];
    let mut col = vec![[0f32; 3]; n];
    for y in 0..b.h {
        for x in 0..b.w {
            let (a, c) = split(b.px(x, y));
            alpha[y * b.w + x] = a;
            col[y * b.w + x] = c;
        }
    }
    (alpha, col)
}

fn luma(c: [f32; 3]) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

/// Sättigung ändern (1 = unverändert).
fn saturate(c: [f32; 3], f: f32) -> [f32; 3] {
    let l = luma(c);
    [
        (l + (c[0] - l) * f).clamp(0.0, 1.0),
        (l + (c[1] - l) * f).clamp(0.0, 1.0),
        (l + (c[2] - l) * f).clamp(0.0, 1.0),
    ]
}

/// Kantenstärke 0–1 (Sobel auf leicht geglätteter Helligkeit).
fn edge_map(col: &[[f32; 3]], w: usize, h: usize, smooth: usize) -> Vec<f32> {
    let mut l: Vec<f32> = col.iter().map(|c| luma(*c)).collect();
    blur_plane(&mut l, w, h, smooth);
    let at = |x: isize, y: isize| {
        l[(y.clamp(0, h as isize - 1) as usize) * w + x.clamp(0, w as isize - 1) as usize]
    };
    let mut e = vec![0f32; w * h];
    for y in 0..h as isize {
        for x in 0..w as isize {
            let gx = at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1)
                - at(x - 1, y - 1)
                - 2.0 * at(x - 1, y)
                - at(x - 1, y + 1);
            let gy = at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1)
                - at(x - 1, y - 1)
                - 2.0 * at(x, y - 1)
                - at(x + 1, y - 1);
            e[y as usize * w + x as usize] = ((gx * gx + gy * gy).sqrt() * 1.6).clamp(0.0, 1.0);
        }
    }
    e
}

/// Kuwahara-Filter: je Pixel der Mittelwert des ruhigsten der vier Quadranten (Radius r).
/// Flächen werden zu Farbfeldern, Kanten bleiben scharf.
fn kuwahara(col: &[[f32; 3]], w: usize, h: usize, r: usize) -> Vec<[f32; 3]> {
    let w1 = w + 1;
    let mut sum = vec![[0f64; 5]; w1 * (h + 1)];
    for y in 0..h {
        let mut row = [0f64; 5];
        for x in 0..w {
            let c = col[y * w + x];
            let l = luma(c) as f64;
            let v = [c[0] as f64, c[1] as f64, c[2] as f64, l, l * l];
            for i in 0..5 {
                row[i] += v[i];
                sum[(y + 1) * w1 + x + 1][i] = sum[y * w1 + x + 1][i] + row[i];
            }
        }
    }
    let rect = |x0: usize, y0: usize, x1: usize, y1: usize| {
        let mut o = [0f64; 5];
        for i in 0..5 {
            o[i] = sum[y1 * w1 + x1][i] - sum[y0 * w1 + x1][i] - sum[y1 * w1 + x0][i] + sum[y0 * w1 + x0][i];
        }
        (o, ((x1 - x0) * (y1 - y0)).max(1) as f64)
    };
    let mut out = vec![[0f32; 3]; w * h];
    let os = Shared::new(&mut out);
    par_bands(h, band_rows(w), |y0, y1| {
        for y in y0..y1 {
            // Sicherheit: die Zeilen y0..y1 gehören nur diesem Band
            let orow = unsafe { os.range(y * w, w) };
            for x in 0..w {
                let (xa, xb) = (x.saturating_sub(r), (x + r + 1).min(w));
                let (ya, yb) = (y.saturating_sub(r), (y + r + 1).min(h));
                let mut best = (f64::MAX, col[y * w + x]);
                for (x0, y0, x1, y1) in [
                    (xa, ya, x + 1, y + 1),
                    (x, ya, xb, y + 1),
                    (xa, y, x + 1, yb),
                    (x, y, xb, yb),
                ] {
                    let (s, cnt) = rect(x0, y0, x1, y1);
                    let mean = s[3] / cnt;
                    let var = s[4] / cnt - mean * mean;
                    if var < best.0 {
                        best = (
                            var,
                            [(s[0] / cnt) as f32, (s[1] / cnt) as f32, (s[2] / cnt) as f32],
                        );
                    }
                }
                orow[x] = best.1;
            }
        }
    });
    out
}

/// Wasserfarben auf einem Bild: Farbflächen (kantenerhaltend vereinfacht), dunklere
/// Farbränder an den Umrissen, leicht lasierende Farben und Papierstruktur.
pub fn watercolor(b: &mut Buf, k: f64) {
    let (w, h) = (b.w, b.h);
    if w < 3 || h < 3 {
        return;
    }
    let k = k.clamp(0.0, 1.0) as f32;
    let scale = (w.min(h) as f32 / 400.0).max(1.0);
    let (alpha, col) = read_all(b);
    let r = ((w.min(h) as f32) * (0.003 + 0.007 * k)).round().max(1.0) as usize;
    let flat = kuwahara(&col, w, h, r);
    let edges = edge_map(&flat, w, h, 1);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if alpha[i] == 0 {
                continue;
            }
            let paper = value_noise(x as f32, y as f32, 2.5 * scale, 5) - 0.5;
            let cloud = value_noise(x as f32, y as f32, 40.0 * scale, 9) - 0.5;
            let mut c = saturate(flat[i], 1.0 + 0.15 * k);
            // lasierend: helle Stellen etwas heller, Farbränder an den Kanten dunkler
            let rim = 1.0 - edges[i] * 0.45 * k;
            for v in c.iter_mut() {
                *v = (*v + (1.0 - *v) * 0.1 * k) * rim * (1.0 + paper * 0.14 * k + cloud * 0.1 * k);
            }
            c = mix(c, col[i], 0.15 * (1.0 - k));
            b.set(x, y, join(alpha[i], c));
        }
    }
}

/// Wachsmalstifte auf einem Bild: kräftige, leicht vereinfachte Farben, Umrisse als
/// gezeichnete Linien, Papier scheint nur in hellen Bereichen durch.
pub fn crayon(b: &mut Buf, k: f64) {
    let (w, h) = (b.w, b.h);
    if w < 3 || h < 3 {
        return;
    }
    let kf = k.clamp(0.0, 1.0) as f32;
    let scale = (w.min(h) as f32 / 400.0).max(1.0);
    let (alpha, col) = read_all(b);
    let r = ((w.min(h) as f32) * 0.004).round().max(1.0) as usize;
    let flat = kuwahara(&col, w, h, r);
    let edges = edge_map(&col, w, h, (scale * 0.8).round() as usize);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if alpha[i] == 0 {
                continue;
            }
            let (px, py) = (x as f32 / scale, y as f32 / scale);
            let (paper, shade) = crayon_px(px, py, kf);
            let mut c = saturate(flat[i], 1.0 + 0.25 * kf);
            // Papier nur, wo es hell ist (in dunklen Flächen deckt das Wachs)
            let light = luma(c).powf(1.5);
            let line = (edges[i] * 1.4).min(1.0) * (0.55 + 0.45 * value_noise(px, py, 1.3, 25));
            for v in c.iter_mut() {
                let mut t = (*v * shade).clamp(0.0, 1.0);
                t += (0.97 - t).max(0.0) * paper * light;
                *v = t * (1.0 - line * 0.8 * kf);
            }
            b.set(x, y, join(alpha[i], c));
        }
    }
}

/// Ölfarbe auf einem Bild: Kuwahara-Filter (Flächen wie Pinselstriche, Kanten bleiben),
/// etwas kräftigere Farben, leicht betonte Umrisse und ein Relief der Striche.
pub fn oil(b: &mut Buf, k: f64) {
    let (w, h) = (b.w, b.h);
    if w < 3 || h < 3 {
        return;
    }
    let kf = k.clamp(0.0, 1.0) as f32;
    let (alpha, col) = read_all(b);
    let r = ((w.min(h) as f32) * (0.004 + 0.012 * kf)).round().max(1.0) as usize;
    let flat = kuwahara(&col, w, h, r);
    let edges = edge_map(&flat, w, h, 1);
    let scale = (w.min(h) as f32 / 400.0).max(1.0);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if alpha[i] == 0 {
                continue;
            }
            // Relief der Striche auf Fotos zurückhaltender, sonst wirkt es wie ein Stempel
            let f = 1.0 + (oil_px(x as f32 / scale, y as f32 / scale, kf, 14.0) - 1.0) * 0.45;
            let c = mix(
                col[i],
                saturate(flat[i], 1.0 + 0.2 * kf),
                (0.4 + 0.6 * kf).min(1.0),
            );
            let dark = 1.0 - edges[i] * 0.3 * kf;
            b.set(
                x,
                y,
                join(alpha[i], [c[0] * f * dark, c[1] * f * dark, c[2] * f * dark]),
            );
        }
    }
}

/// Unvormultiplizierte RGBA-Bytes (R, G, B, A) aus einem Cairo-Puffer.
pub fn to_rgba(data: &[u8], w: usize, h: usize, stride: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let o = y * stride + x * 4;
            let p = u32::from_ne_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
            let (a, c) = split(p);
            out.extend_from_slice(&[
                (c[0] * 255.0).round() as u8,
                (c[1] * 255.0).round() as u8,
                (c[2] * 255.0).round() as u8,
                a as u8,
            ]);
        }
    }
    out
}

/// Zeigt das Bild flache Farbflächen (Logo, Illustration) statt eines Fotos?
/// Wenige Farben (auf 5 Bit je Kanal gerundet) decken fast alle Pixel ab.
pub fn is_flat(rgba: &[u8], w: usize, h: usize) -> bool {
    if w < 4 || h < 4 {
        return false;
    }
    let step = ((w * h) / 40_000).max(1);
    let mut hist: std::collections::HashMap<u16, u32> = std::collections::HashMap::new();
    let mut total = 0u32;
    let mut i = 0;
    while i < w * h {
        let p = &rgba[i * 4..i * 4 + 4];
        if p[3] >= 128 {
            let key = ((p[0] >> 3) as u16) << 10 | ((p[1] >> 3) as u16) << 5 | (p[2] >> 3) as u16;
            *hist.entry(key).or_default() += 1;
            total += 1;
        }
        i += step;
    }
    if total < 50 {
        return false;
    }
    let mut counts: Vec<u32> = hist.values().copied().collect();
    counts.sort_unstable_by(|a, b| b.cmp(a));
    let top: u32 = counts.iter().take(12).sum();
    top as f32 / total as f32 >= 0.9
}

// ───────────────────────────── 3D-Effekte und Texturen ─────────────────────────────
//
// Reine Rechenteile (ohne Cairo): Abstand zum Rand, Höhenprofil der Formen, Beleuchtung
// sowie die Muster für gebürstetes Metall und Hochglanz. Der Zeichencode in `render.rs`
// setzt sie zu Bildern zusammen.

/// Größte Steigung des Reliefs (Höhe je Pixel), damit der steile Rand nicht ausreißt.
const MAX_SLOPE: f32 = 2.5;

/// Exakter euklidischer Abstand jedes Pixels der Form (Deckkraft über 0,5) zum nächsten
/// Pixel außerhalb; außerhalb der Form 0. Pixel am Bildrand zählen nicht als außen.
pub fn inside_distance(alpha: &[f32], w: usize, h: usize) -> Vec<f32> {
    const INF: f64 = 1e20;
    let mut f = vec![0f64; w * h];
    for i in 0..w * h {
        f[i] = if alpha[i] > 0.5 { INF } else { 0.0 };
    }
    // eindimensionale Abstandstransformation (Felzenszwalb) erst je Spalte, dann je Zeile;
    // Spalten bzw. Zeilen sind unabhängig und laufen auf mehreren Kernen
    fn run(line: &[f64], out: &mut [f64], v: &mut [usize], z: &mut [f64], m: usize) {
        let mut k = 0usize;
        v[0] = 0;
        z[0] = -INF;
        z[1] = INF;
        for q in 1..m {
            let mut s;
            loop {
                let vk = v[k];
                s = ((line[q] + (q * q) as f64) - (line[vk] + (vk * vk) as f64))
                    / (2.0 * q as f64 - 2.0 * vk as f64);
                if s <= z[k] && k > 0 {
                    k -= 1;
                } else {
                    break;
                }
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = INF;
        }
        k = 0;
        for q in 0..m {
            while z[k + 1] < q as f64 {
                k += 1;
            }
            let d = q as f64 - v[k] as f64;
            out[q] = d * d + line[v[k]];
        }
    }
    let n = w.max(h);
    let scratch = || (vec![0f64; n], vec![0f64; n], vec![0usize; n], vec![0f64; n + 1]);
    {
        let fs = Shared::new(&mut f);
        par_bands(w, band_rows(h), |x0, x1| {
            let (mut line, mut out, mut v, mut z) = scratch();
            for x in x0..x1 {
                for y in 0..h {
                    // Sicherheit: jede Spalte gehört genau einem Band
                    line[y] = unsafe { fs.get(y * w + x) };
                }
                run(&line, &mut out, &mut v, &mut z, h);
                for y in 0..h {
                    unsafe { fs.set(y * w + x, out[y]) };
                }
            }
        });
    }
    {
        let fs = Shared::new(&mut f);
        par_bands(h, band_rows(w), |y0, y1| {
            let (mut line, mut out, mut v, mut z) = scratch();
            for y in y0..y1 {
                // Sicherheit: jede Zeile gehört genau einem Band
                let row = unsafe { fs.range(y * w, w) };
                line[..w].copy_from_slice(row);
                run(&line, &mut out, &mut v, &mut z, w);
                row.copy_from_slice(&out[..w]);
            }
        });
    }
    f.iter()
        .zip(alpha)
        .map(|(d, a)| {
            if *a > 0.5 {
                // Mitte des Pixels: ein halber Pixel bis zur Kante
                (d.sqrt().min(1e4) as f32 - 0.5).max(0.0)
            } else {
                0.0
            }
        })
        .collect()
}

/// Breite der Randzone, in der das Relief ansteigt, bei innerem Radius `dm` (Pixel).
/// `cap` begrenzt die Randzone des Regentropfens (große Tropfen bleiben flach).
pub fn relief_zone(kind: Lens, dm: f32, cap: f32) -> f32 {
    let z = match kind {
        Lens::Drop => (dm * 0.5).min(cap),
        Lens::Sphere => dm,
        Lens::Crystal => dm * 0.55,
        Lens::Pyramid => dm,
    };
    z.max(1.0)
}

/// Höhenprofil 0…1 bei relativem Randabstand `t` (0 am Rand, 1 ab der Randzone).
pub fn profile(kind: Lens, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match kind {
        // Kreisbogen: steiler Rand, oben flach
        Lens::Drop | Lens::Sphere => (1.0 - (1.0 - t) * (1.0 - t)).sqrt(),
        // gerade Flanken
        Lens::Crystal | Lens::Pyramid => t,
    }
}

/// Relief einer Form: Höhe in Pixeln je Pixel, positiv herausgedrückt, negativ eingedrückt.
/// `dist` ist der Randabstand, `dm` der größte Abstand (innerer Radius) der Form.
pub struct Relief {
    /// normiertes Profil 0…1 (ohne Vorzeichen)
    pub p: Vec<f32>,
    /// Höhe in Pixeln mit Vorzeichen
    pub height: Vec<f32>,
    pub zone: f32,
}

pub fn relief(kind: Lens, dist: &[f32], dm: f32, sign: f32, cap: f32) -> Relief {
    let zone = relief_zone(kind, dm, cap);
    let p: Vec<f32> = dist.iter().map(|d| profile(kind, d / zone)).collect();
    // die Spitze ist so hoch wie 0,8 Randzonen (Kugel: ein Radius), das gibt Flanken um 40°
    let ratio = if kind == Lens::Sphere { 1.0 } else { 0.8 };
    let height = p.iter().map(|v| v * zone * ratio * sign).collect();
    Relief { p, height, zone }
}

/// Steigung (Höhenänderung je Pixel in x und y) an einer Stelle, auf `MAX_SLOPE` begrenzt.
pub fn slope(height: &[f32], w: usize, h: usize, x: usize, y: usize) -> (f32, f32) {
    let at = |xx: usize, yy: usize| height[yy * w + xx];
    let gx = (at((x + 1).min(w - 1), y) - at(x.saturating_sub(1), y)) * 0.5;
    let gy = (at(x, (y + 1).min(h - 1)) - at(x, y.saturating_sub(1))) * 0.5;
    let l = (gx * gx + gy * gy).sqrt();
    if l > MAX_SLOPE {
        (gx / l * MAX_SLOPE, gy / l * MAX_SLOPE)
    } else {
        (gx, gy)
    }
}

/// Helligkeitsänderung durch die Neigung gegenüber einer ebenen Fläche (Licht von links
/// oben): positiv = heller, negativ = dunkler. Bei Steigung 0 ist sie 0.
pub fn light_delta(gx: f32, gy: f32) -> f32 {
    const L: (f32, f32, f32) = (-0.42, -0.59, 0.68);
    let n = (gx * gx + gy * gy + 1.0).sqrt();
    let lit = (-gx * L.0 - gy * L.1 + L.2) / n;
    lit - L.2
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Pseudozufall 0…1 je Gitterpunkt.
fn hash_ij(i: i64, j: i64, salt: u32) -> f32 {
    let mut h = (i as u32).wrapping_mul(0x9E37_79B1) ^ (j as u32).wrapping_mul(0x85EB_CA77) ^ salt;
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h & 0x00FF_FFFF) as f32 / 0x0100_0000 as f32
}

/// Gebürstetes Metall an einer Stelle: −1…1 (positiv hell, negativ dunkel). `(x, y)` ist
/// die Lage im Rahmen des Objekts in Pixeln, `bw` dessen Breite, `scale` Pixel je
/// Dokumenteinheit. Die Striche laufen waagerecht; ein weicher Schimmer liegt quer darüber.
pub fn brushed(x: f32, y: f32, bw: f32, scale: f32) -> f32 {
    let row = (y / (0.7 * scale).max(1.0)).floor() as i64;
    let mut streak = 0.0;
    for (len, wgt, salt) in [(48.0f32, 0.5f32, 11u32), (14.0, 0.3, 23), (4.0, 0.2, 37)] {
        let u = x / (len * scale).max(2.0);
        let i = u.floor();
        let f = u - i;
        let sm = f * f * (3.0 - 2.0 * f);
        let a = hash_ij(i as i64, row, salt);
        let b = hash_ij(i as i64 + 1, row, salt);
        streak += wgt * ((a + (b - a) * sm) - 0.5) * 2.0;
    }
    let u = x / bw.max(1.0);
    let g = |c: f32, wd: f32| (-((u - c) / wd) * ((u - c) / wd)).exp();
    let sheen = 0.6 * g(0.28, 0.14) + 0.4 * g(0.75, 0.18) - 0.3;
    (0.6 * streak + 0.9 * sheen).clamp(-1.0, 1.0)
}

/// Hochglanz an einer Stelle: Deckkraft des weißen Glanzes 0…1 (bei voller Stärke).
/// `(x, y)` ist die Lage im Rahmen des Objekts (Breite `bw`, Höhe `bh`), `d` der Abstand
/// zum Rand, alles in Pixeln, `scale` Pixel je Dokumenteinheit.
pub fn gloss(x: f32, y: f32, bw: f32, bh: f32, d: f32, scale: f32) -> f32 {
    let (bw, bh) = (bw.max(1.0), bh.max(1.0));
    // großes weiches Glanzlicht im oberen Teil: Ausschnitt einer flachen Ellipse, die über
    // den oberen Rand hinausragt und etwa bis zur Mitte reicht
    let (cx, cy) = (bw / 2.0, -0.12 * bh);
    let (a, b) = (0.66 * bw, 0.62 * bh);
    let r = (((x - cx) / a).powi(2) + ((y - cy) / b).powi(2)).sqrt();
    let hl = smoothstep(1.0, 0.9, r);
    let fade = 1.0 - 0.65 * (y / (0.5 * bh)).clamp(0.0, 1.0);
    // das Glanzlicht hält Abstand zum Rand
    let inset = ((d - 2.0 * scale) / (5.0 * scale)).clamp(0.0, 1.0);
    // heller Saum direkt am Rand, oben stärker
    let top = 1.0 - 0.7 * (y / bh).clamp(0.0, 1.0);
    let rim = (-(d / (2.2 * scale)).powi(2)).exp() * 0.35 * top;
    // leichter Widerschein unten
    let glow = 0.10 * smoothstep(0.7, 1.0, y / bh);
    (hl * fade * inset * 0.55 + rim + glow).clamp(0.0, 1.0)
}

/// Feine Körnung von Glas an der Stelle (x, y) in Dokumenteinheiten: −1…1.
pub fn frost_grain(x: f32, y: f32) -> f32 {
    let a = value_noise(x, y, 0.45, 91) - 0.5;
    let b = value_noise(x, y, 1.3, 92) - 0.5;
    (a * 1.4 + b * 0.8).clamp(-1.0, 1.0)
}

/// Neigung der Oberfläche unter einem 3D-Effekt an einem Pixel: Steigung `(gx, gy)`,
/// Tiefe `amt` (0–1) und Stärke der Beleuchtung `lc` (1 = Standard).
#[derive(Clone, Copy, Debug)]
pub struct Tilt {
    pub gx: f32,
    pub gy: f32,
    pub amt: f32,
    pub lc: f32,
}

/// Licht und Schatten über einem Pixel: `lit` (weißes bzw. getöntes Licht, 0–1), `shade`
/// (Schatten, 0–1) und `tint` (wie stark das Licht die Farbe des Objekts annimmt, 0–1).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sheen {
    pub lit: f32,
    pub shade: f32,
    pub tint: f32,
}

/// Lichtrichtung (von links oben, zum Betrachter geneigt) und die Mitte zwischen Licht und
/// Blick (für Glanzlichter).
const LIGHT: (f32, f32, f32) = (-0.42, -0.59, 0.68);

fn surface_normal(gx: f32, gy: f32) -> (f32, f32, f32) {
    let n = (gx * gx + gy * gy + 1.0).sqrt();
    (-gx / n, -gy / n, 1.0 / n)
}

/// Glanzlicht (Blinn) mit Schärfe `power`; eine ebene Fläche hat 0.
fn specular(n: (f32, f32, f32), power: f32) -> f32 {
    let (hx, hy, hz) = (LIGHT.0, LIGHT.1, LIGHT.2 + 1.0);
    let hl = (hx * hx + hy * hy + hz * hz).sqrt();
    let s = |n: (f32, f32, f32)| ((n.0 * hx + n.1 * hy + n.2 * hz) / hl).max(0.0).powf(power);
    let flat = s((0.0, 0.0, 1.0));
    ((s(n) - flat) / (1.0 - flat)).max(0.0)
}

fn screen(a: f32, b: f32) -> f32 {
    1.0 - (1.0 - a.clamp(0.0, 1.0)) * (1.0 - b.clamp(0.0, 1.0))
}

/// Licht und Schatten eines Materials an einem Pixel.
///
/// `(x, y)`: Lage im Rahmen des Objekts (Breite `bw`, Höhe `bh`), `d`: Abstand zum Rand,
/// alles in Pixeln; `scale`: Pixel je Dokumenteinheit; `angle`: Richtung des Materials
/// (Bogenmaß). Mit `tilt` liegt ein 3D-Effekt darunter: Das Material bestimmt dann, wie das
/// Relief das Licht zurückwirft. Ohne Material (`mat = None`) ist die Oberfläche matt –
/// nur weiches Licht und Schatten der Neigung, kein Glanz.
#[allow(clippy::too_many_arguments)]
pub fn material_px(
    mat: Option<(Material, f32)>,
    angle: f32,
    x: f32,
    y: f32,
    bw: f32,
    bh: f32,
    d: f32,
    scale: f32,
    tilt: Option<Tilt>,
) -> Sheen {
    let (bw, bh, scale) = (bw.max(1.0), bh.max(1.0), scale.max(1e-3));
    // matte Grundbeleuchtung des Reliefs
    let (mut dl, mut ds, n, depth) = match tilt {
        Some(t) => {
            let delta = light_delta(t.gx, t.gy);
            (
                (delta * 2.0 * t.amt * t.lc).clamp(0.0, 0.7),
                (-delta * 1.1 * t.amt * t.lc).clamp(0.0, 0.5),
                surface_normal(t.gx, t.gy),
                (2.0 * t.amt).min(1.0) * t.lc.min(1.5),
            )
        }
        None => (0.0, 0.0, (0.0, 0.0, 1.0), 0.0),
    };
    let Some((m, k)) = mat else {
        return Sheen {
            lit: dl,
            shade: ds,
            tint: 0.0,
        };
    };
    let k = k.clamp(0.0, 1.0);
    let relief = tilt.is_some();
    // Lage in Richtung des Materials (u entlang, v quer), in Dokumenteinheiten
    let (c, s) = (angle.cos(), angle.sin());
    let along = |x: f32, y: f32| ((x * c + y * s) / scale, (-x * s + y * c) / scale);
    let mut lit = 0.0f32;
    let mut shade = 0.0f32;
    let mut tint = 0.0f32;
    match m {
        Material::Gloss => {
            // flacher Glanz oben; auf einem Relief klein und scharf, Schatten tiefer
            let flat = gloss(x, y, bw, bh, d, scale) * k * if relief { 0.3 } else { 1.0 };
            lit = flat;
            if relief {
                lit = screen(lit, specular(n, 90.0) * 0.95 * k * depth);
                lit = screen(lit, specular(n, 14.0) * 0.25 * k * depth);
                ds *= 1.0 + 0.5 * k;
            }
        }
        Material::Metal => {
            // spiegelt eine Umgebung: heller Himmel oben, scharfer dunkler Horizont,
            // mittlerer Boden; auf dem Relief folgt sie der Neigung
            let pos = (y / bh - 0.5) * if relief { 0.35 } else { 0.9 };
            let e = if relief { 2.0 * n.2 * n.1 + pos } else { pos };
            let sky = 0.15 + 0.55 * smoothstep(-0.02, -0.5, e);
            let horizon = smoothstep(-0.04, 0.02, e);
            let ground_dark = 0.5 * (1.0 - smoothstep(0.0, 0.45, e));
            let ground_lit = 0.22 * smoothstep(0.15, 0.5, e);
            lit = k * (sky * (1.0 - horizon) + ground_lit * horizon);
            shade = k * ground_dark * horizon;
            if relief {
                lit = screen(lit, specular(n, 40.0) * 0.9 * k * depth);
                // Metall streut kaum: wenig mattes Licht
                dl *= 1.0 - 0.6 * k;
                ds *= 1.0 - 0.2 * k;
            }
            tint = 0.75 * k;
        }
        Material::Brushed => {
            let (u, v) = along(x, y);
            // Ausdehnung entlang der Striche (für den Schimmer)
            let ext = (bw * c.abs() + bh * s.abs()).max(1.0);
            let u0 = [(0.0, 0.0), (bw, 0.0), (0.0, bh), (bw, bh)]
                .iter()
                .map(|p| p.0 * c + p.1 * s)
                .fold(f32::MAX, f32::min);
            let b = brushed(u * scale - u0, v * scale, ext, scale) * k * 0.5;
            if b > 0.0 {
                lit = b;
            } else {
                shade = -b * 0.8;
            }
            if let Some(t) = tilt {
                // die feinen Rillen streuen das Licht quer zu den Strichen: das Glanzlicht wird
                // zu einem langen Streifen quer zur Bürstrichtung (nur die Neigung entlang der
                // Striche zählt)
                let along_g = t.gx * c + t.gy * s;
                let (px, py) = (along_g * c, along_g * s);
                let pn = surface_normal(px, py);
                let aniso = specular(pn, 8.0) * 0.9 + specular(pn, 40.0) * 0.6;
                lit = screen(lit, aniso * k * depth);
                dl *= 1.0 - 0.4 * k;
            }
            tint = 0.5 * k;
        }
        Material::Velvet => {
            let grain = value_noise(x / scale, y / scale, 0.6, 85) - 0.5;
            let rim = (-d / (5.0 * scale)).exp();
            let centre = 1.0 - (-d / (8.0 * scale)).exp();
            lit = k * 0.45 * rim * if relief { 0.4 } else { 1.0 };
            shade = k * (0.18 * centre + 0.08 * grain.max(0.0));
            if relief {
                // Samt schimmert, wo die Fläche vom Blick weg geneigt ist, nicht im Licht
                let graze = (1.0 - n.2).max(0.0).powf(0.8);
                lit = screen(lit, graze * 0.75 * k * depth);
                dl *= 1.0 - 0.7 * k;
                ds *= 1.0 + 0.2 * k;
            }
            tint = 0.65 * k;
        }
    }
    Sheen {
        lit: screen(dl, lit),
        shade: screen(ds, shade).min(0.85),
        tint: tint.clamp(0.0, 1.0),
    }
}

// ───────────────────────────── mehrere Kerne ─────────────────────────────

/// Zahl der Rechenkerne, auf die Pixelarbeit verteilt wird (höchstens 8).
pub fn threads() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .clamp(1, 8)
    })
}

/// Zeilen je Band, damit sich das Verteilen lohnt (jedes Band mindestens etwa 32 000 Pixel
/// bei `w` Pixeln je Zeile).
pub fn band_rows(w: usize) -> usize {
    (32_768 / w.max(1)).max(8)
}

/// Die Zeilen (oder Spalten) `0..n` in Bänder teilen und gleichzeitig bearbeiten:
/// `f(start, ende)` bekommt je ein Band. Unter `min_band` Zeilen je Band lohnt sich das
/// Aufteilen nicht; dann läuft alles im aufrufenden Thread.
pub fn par_bands(n: usize, min_band: usize, f: impl Fn(usize, usize) + Sync) {
    let parts = threads().min(n / min_band.max(1)).max(1);
    if parts <= 1 {
        f(0, n);
        return;
    }
    let band = n.div_ceil(parts);
    std::thread::scope(|s| {
        let f = &f;
        for i in 1..parts {
            let (a, b) = (i * band, ((i + 1) * band).min(n));
            if a < b {
                s.spawn(move || f(a, b));
            }
        }
        f(0, band.min(n));
    });
}

/// Schreibzugriff auf getrennte Teile eines Puffers aus mehreren Threads zugleich
/// (jedes Band schreibt nur seine eigenen Zeilen bzw. Spalten).
pub struct Shared<'a, T> {
    ptr: *mut T,
    len: usize,
    _life: std::marker::PhantomData<&'a mut [T]>,
}

// Sicherheit: Zugriffe gehen nur über die unsicheren Methoden, deren Aufrufer zusichern,
// dass sich die Bereiche verschiedener Threads nicht überschneiden.
unsafe impl<T: Send> Sync for Shared<'_, T> {}
unsafe impl<T: Send> Send for Shared<'_, T> {}

impl<'a, T: Copy> Shared<'a, T> {
    pub fn new(v: &'a mut [T]) -> Self {
        Shared {
            ptr: v.as_mut_ptr(),
            len: v.len(),
            _life: std::marker::PhantomData,
        }
    }
    /// # Safety
    /// Kein anderer Thread darf gleichzeitig auf `start..start + n` zugreifen.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn range(&self, start: usize, n: usize) -> &mut [T] {
        assert!(start + n <= self.len);
        std::slice::from_raw_parts_mut(self.ptr.add(start), n)
    }
    /// # Safety
    /// Kein anderer Thread darf gleichzeitig auf `i` schreiben.
    pub unsafe fn get(&self, i: usize) -> T {
        assert!(i < self.len);
        *self.ptr.add(i)
    }
    /// # Safety
    /// Kein anderer Thread darf gleichzeitig auf `i` zugreifen.
    pub unsafe fn set(&self, i: usize, v: T) {
        assert!(i < self.len);
        *self.ptr.add(i) = v;
    }
}

/// Dreifacher Kastenweichzeichner (annähernd gaußförmig) auf ARGB-Pixeln (`stride` Bytes je
/// Zeile), Ränder werden fortgesetzt. Zeilen und Spalten laufen auf mehreren Kernen.
pub fn box_blur_rgba(data: &mut [u8], w: usize, h: usize, stride: usize, radius: f64) {
    if w == 0 || h == 0 {
        return;
    }
    // drei Durchgänge mit etwa radius/√3 ergeben ungefähr die gewünschte Unschärfe
    let r = ((radius / 3f64.sqrt()).round() as usize).max(1);
    let div = (2 * r + 1) as u32;
    let half = div / 2;
    let mut snap = vec![0u8; h * stride];
    for _ in 0..3 {
        // waagerecht: jede Zeile für sich (Pixel als vier Kanäle nebeneinander)
        {
            let d = Shared::new(data);
            par_bands(h, band_rows(w), |y0, y1| {
                let mut line = vec![0u8; w * 4];
                for y in y0..y1 {
                    // Sicherheit: jede Zeile gehört genau einem Band
                    let row = unsafe { d.range(y * stride, w * 4) };
                    line.copy_from_slice(row);
                    let last = w - 1;
                    let mut sum = [0u32; 4];
                    for i in 0..=2 * r {
                        let p = i.saturating_sub(r).min(last) * 4;
                        for c in 0..4 {
                            sum[c] += line[p + c] as u32;
                        }
                    }
                    for i in 0..w {
                        let o = i * 4;
                        let add = (i + r + 1).min(last) * 4;
                        let sub = i.saturating_sub(r) * 4;
                        for c in 0..4 {
                            row[o + c] = ((sum[c] + half) / div) as u8;
                            sum[c] = sum[c] + line[add + c] as u32 - line[sub + c] as u32;
                        }
                    }
                }
            });
        }
        // senkrecht: aus einer Kopie lesen, jedes Band schreibt seine Zeilen
        snap.copy_from_slice(&data[..h * stride]);
        {
            let src = &snap;
            let d = Shared::new(data);
            par_bands(h, band_rows(w), |y0, y1| {
                let n = w * 4;
                let row_at = |y: isize| {
                    let y = y.clamp(0, h as isize - 1) as usize;
                    &src[y * stride..y * stride + n]
                };
                let mut sum = vec![0u32; n];
                for k in -(r as isize)..=(r as isize) {
                    for (s, v) in sum.iter_mut().zip(row_at(y0 as isize + k)) {
                        *s += *v as u32;
                    }
                }
                for y in y0..y1 {
                    // Sicherheit: jede Zeile gehört genau einem Band
                    let row = unsafe { d.range(y * stride, n) };
                    let (add, sub) = (
                        row_at(y as isize + r as isize + 1),
                        row_at(y as isize - r as isize),
                    );
                    for i in 0..n {
                        row[i] = ((sum[i] + half) / div) as u8;
                        sum[i] = sum[i] + add[i] as u32 - sub[i] as u32;
                    }
                }
            });
        }
    }
}

/// Dreifacher Kastenweichzeichner auf einem Feld von Zahlen (`w` × `h`), Ränder werden
/// fortgesetzt; auf mehreren Kernen.
pub fn blur_field(f: &mut [f32], w: usize, h: usize, radius: f64) {
    if w == 0 || h == 0 {
        return;
    }
    let r = ((radius / 3f64.sqrt()).round() as usize).max(1);
    let div = (2 * r + 1) as f32;
    let mut snap = vec![0f32; w * h];
    for _ in 0..3 {
        {
            let d = Shared::new(f);
            par_bands(h, band_rows(w), |y0, y1| {
                let mut tmp = vec![0f32; w];
                for y in y0..y1 {
                    // Sicherheit: jede Zeile gehört genau einem Band
                    let row = unsafe { d.range(y * w, w) };
                    let px = |row: &[f32], i: isize| row[i.clamp(0, w as isize - 1) as usize];
                    let mut sum = 0f32;
                    for i in -(r as isize)..=(r as isize) {
                        sum += px(row, i);
                    }
                    for i in 0..w {
                        tmp[i] = sum / div;
                        sum += px(row, i as isize + r as isize + 1);
                        sum -= px(row, i as isize - r as isize);
                    }
                    row.copy_from_slice(&tmp);
                }
            });
        }
        snap.copy_from_slice(&f[..w * h]);
        {
            let src = &snap;
            let d = Shared::new(f);
            par_bands(h, band_rows(w), |y0, y1| {
                let at = |y: isize, x: usize| src[(y.clamp(0, h as isize - 1) as usize) * w + x];
                let mut sum = vec![0f32; w];
                for (x, s) in sum.iter_mut().enumerate() {
                    for k in -(r as isize)..=(r as isize) {
                        *s += at(y0 as isize + k, x);
                    }
                }
                for y in y0..y1 {
                    // Sicherheit: jede Zeile gehört genau einem Band
                    let row = unsafe { d.range(y * w, w) };
                    for x in 0..w {
                        row[x] = sum[x] / div;
                        sum[x] += at(y as isize + r as isize + 1, x);
                        sum[x] -= at(y as isize - r as isize, x);
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn materials_light_the_relief_differently() {
        use super::{material_px, Tilt};
        use crate::model::Material;
        // Flanke, die zum Licht zeigt, und eine ebene Stelle
        let toward = Some(Tilt {
            gx: 0.35,
            gy: 0.45,
            amt: 0.8,
            lc: 1.0,
        });
        let flat = Some(Tilt {
            gx: 0.0,
            gy: 0.0,
            amt: 0.8,
            lc: 1.0,
        });
        let at = |m: Option<(Material, f32)>, t| material_px(m, 0.0, 50.0, 50.0, 100.0, 100.0, 50.0, 1.0, t);
        let matte = at(None, toward);
        let gloss = at(Some((Material::Gloss, 1.0)), toward);
        // Hochglanz hat dort ein viel helleres Glanzlicht als matt
        assert!(gloss.lit > matte.lit + 0.2, "{gloss:?} {matte:?}");
        // matt ohne Neigung: nichts
        assert_eq!(at(None, flat), super::Sheen::default());
        // Samt ist in der Fläche dunkel und an steilen Flanken hell
        let steep = Some(Tilt {
            gx: 2.0,
            gy: 0.0,
            amt: 0.8,
            lc: 1.0,
        });
        let v_flat = at(Some((Material::Velvet, 1.0)), flat);
        let v_steep = at(Some((Material::Velvet, 1.0)), steep);
        assert!(v_steep.lit > v_flat.lit + 0.3 && v_flat.shade > 0.1);
        // Metall nimmt die Farbe des Objekts an
        assert!(at(Some((Material::Metal, 1.0)), None).tint > 0.5);
        // Werte bleiben im Bereich
        for m in Material::ALL {
            for i in 0..400 {
                let (x, y) = (i as f32 * 0.37, i as f32 * 0.71);
                for t in [None, toward, steep] {
                    let r = material_px(Some((m, 1.0)), 0.6, x, y, 160.0, 120.0, (i % 40) as f32, 2.0, t);
                    for v in [r.lit, r.shade, r.tint] {
                        assert!((0.0..=1.0).contains(&v), "{m:?} {r:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn parallel_blur_matches_sequential() {
        // Bezug: der frühere Weichzeichner (Zeile für Zeile, Spalte für Spalte)
        fn box_line(line: &mut [u8], n: usize, r: usize) {
            let div = (2 * r + 1) as u32;
            let mut tmp = vec![0u8; n * 4];
            for c in 0..4 {
                let px = |i: isize| -> u32 { line[(i.clamp(0, n as isize - 1) as usize) * 4 + c] as u32 };
                let mut sum: u32 = 0;
                for i in -(r as isize)..=(r as isize) {
                    sum += px(i);
                }
                for i in 0..n {
                    tmp[i * 4 + c] = ((sum + div / 2) / div) as u8;
                    sum += px(i as isize + r as isize + 1);
                    sum -= px(i as isize - r as isize);
                }
            }
            line[..n * 4].copy_from_slice(&tmp);
        }
        let (w, h, stride) = (413usize, 317usize, 416 * 4);
        let mut a = vec![0u8; h * stride];
        let mut x: u32 = 12345;
        for v in a.iter_mut() {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *v = (x >> 24) as u8;
        }
        let mut b = a.clone();
        let radius = 9.0;
        let r = ((radius / 3f64.sqrt()).round() as usize).max(1);
        for _ in 0..3 {
            for y in 0..h {
                box_line(&mut a[y * stride..y * stride + w * 4], w, r);
            }
            let mut col = vec![0u8; h * 4];
            for xx in 0..w {
                for y in 0..h {
                    col[y * 4..y * 4 + 4].copy_from_slice(&a[y * stride + xx * 4..y * stride + xx * 4 + 4]);
                }
                box_line(&mut col, h, r);
                for y in 0..h {
                    a[y * stride + xx * 4..y * stride + xx * 4 + 4].copy_from_slice(&col[y * 4..y * 4 + 4]);
                }
            }
        }
        super::box_blur_rgba(&mut b, w, h, stride, radius);
        for y in 0..h {
            assert_eq!(
                &a[y * stride..y * stride + w * 4],
                &b[y * stride..y * stride + w * 4],
                "Zeile {y}"
            );
        }
        // Zahlenfeld: gleiche Glättung wie nacheinander gerechnet (bis auf Rundung)
        let mut f: Vec<f32> = (0..w * h).map(|i| ((i * 7919) % 101) as f32).collect();
        let total: f32 = f.iter().sum();
        super::blur_field(&mut f, w, h, 6.0);
        let after: f32 = f.iter().sum();
        assert!((after - total).abs() / total < 0.02, "{total} {after}");
        assert!(f.iter().all(|v| (0.0..=100.0).contains(v)));
    }

    #[test]
    fn parallel_distance_is_exact() {
        // Kreis: Abstand zum Rand = Radius minus Abstand zur Mitte (bis auf Pixelraster)
        let (w, h) = (301usize, 257usize);
        let (cx, cy, r) = (150.0f32, 128.0f32, 100.0f32);
        let a: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                if ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() < r {
                    1.0
                } else {
                    0.0
                }
            })
            .collect();
        let d = super::inside_distance(&a, w, h);
        for (i, v) in d.iter().enumerate() {
            let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            let want = r - ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
            if want > 0.0 {
                assert!((v - want).abs() < 1.6, "{x} {y}: {v} {want}");
            } else {
                assert_eq!(*v, 0.0);
            }
        }
    }

    #[test]
    fn crayon_and_oil_stay_in_range() {
        let mut papers = 0;
        for i in 0..2000 {
            let (x, y) = (i as f32 * 0.73, i as f32 * 1.31);
            let (p, s) = super::crayon_px(x, y, 0.8);
            assert!((0.0..=1.0).contains(&p) && s > 0.7 && s < 1.3);
            if p > 0.05 {
                papers += 1;
            }
            let f = super::oil_px(x, y, 1.0, 9.0);
            assert!(f > 0.5 && f < 1.5, "{f}");
        }
        // bei kräftigem Strich scheint stellenweise Papier durch, aber nicht überall
        assert!(papers > 100 && papers < 1900, "{papers}");
        // Stärke 0: unverändert
        assert_eq!(super::crayon_px(3.0, 4.0, 0.0).0, 0.0);
        assert!((super::oil_px(3.0, 4.0, 0.0, 9.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn halftone_and_grain() {
        // Punktmitte deckt, Zellecke nicht; größere Füllung deckt mehr
        assert!(super::halftone_cov(0.0, 0.0, 10.0, 0.0, 0.5, 1.0) > 0.99);
        assert!(super::halftone_cov(5.0, 5.0, 10.0, 0.0, 0.5, 1.0) < 0.01);
        let area = |fill: f32| {
            let mut n = 0.0;
            for i in 0..100 {
                for j in 0..100 {
                    n += super::halftone_cov(i as f32 * 0.1, j as f32 * 0.1, 10.0, 0.7, fill, 0.1);
                }
            }
            n / 10000.0
        };
        let (a, b) = (area(0.2), area(0.6));
        assert!((a - 0.2).abs() < 0.06 && (b - 0.6).abs() < 0.06, "{a} {b}");
        for i in 0..500 {
            let v = super::frost_grain(i as f32 * 0.37, i as f32 * 0.91);
            assert!((-1.0..=1.0).contains(&v));
            let g = super::riso_grain(i as f32 * 0.37, i as f32 * 0.91, 1.0);
            assert!((0.0..=0.45).contains(&g));
        }
    }

    #[test]
    fn oil_keeps_alpha_and_flat_colour() {
        let (w, h) = (24, 24);
        let mut data = vec![0u8; w * h * 4];
        for i in 0..w * h {
            // halbdurchsichtiges Rot, vormultipliziert
            let v: u32 = (128 << 24) | (128 << 16);
            data[i * 4..i * 4 + 4].copy_from_slice(&v.to_ne_bytes());
        }
        let mut b = super::Buf {
            data: &mut data,
            w,
            h,
            stride: w * 4,
        };
        super::oil(&mut b, 0.5);
        super::crayon(&mut b, 0.5);
        super::halftone(&mut b, 0.5, 4.0, 45.0);
        super::riso(&mut b, 0.5);
        for i in 0..w * h {
            let v = u32::from_ne_bytes([data[i * 4], data[i * 4 + 1], data[i * 4 + 2], data[i * 4 + 3]]);
            assert_eq!(v >> 24, 128);
            assert!(((v >> 16) & 255) >= (v >> 8) & 255);
        }
    }

    use super::*;

    fn solid(w: usize, h: usize, argb: u32) -> Vec<u8> {
        let mut v = Vec::new();
        for _ in 0..w * h {
            v.extend_from_slice(&argb.to_ne_bytes());
        }
        v
    }

    fn first(data: &[u8]) -> u32 {
        u32::from_ne_bytes([data[0], data[1], data[2], data[3]])
    }

    #[test]
    fn filters_change_color_but_not_alpha() {
        for f in ImgFilter::ALL {
            let mut d = solid(4, 4, 0x80_40_20_10);
            let mut b = Buf {
                data: &mut d,
                w: 4,
                h: 4,
                stride: 16,
            };
            tint(&mut b, f, 1.0);
            assert_eq!(first(&d) >> 24, 0x80, "{f:?}");
        }
    }

    #[test]
    fn invert_and_mono_values() {
        let mut d = solid(2, 2, 0xFF_00_00_00);
        let mut b = Buf {
            data: &mut d,
            w: 2,
            h: 2,
            stride: 8,
        };
        tint(&mut b, ImgFilter::Invert, 1.0);
        assert_eq!(first(&d), 0xFF_FF_FF_FF);
        let mut d = solid(2, 2, 0xFF_FF_00_00);
        let mut b = Buf {
            data: &mut d,
            w: 2,
            h: 2,
            stride: 8,
        };
        tint(&mut b, ImgFilter::Mono, 1.0);
        let p = first(&d);
        let (r, g, bl) = ((p >> 16) & 255, (p >> 8) & 255, p & 255);
        assert!(r == g && g == bl && (r as i32 - 76).abs() <= 1, "{r} {g} {bl}");
    }

    #[test]
    fn temperature_goes_both_ways() {
        let base = 0xFF_80_80_80;
        let run = |t: f64| {
            let mut d = solid(2, 2, base);
            let mut b = Buf {
                data: &mut d,
                w: 2,
                h: 2,
                stride: 8,
            };
            temperature(&mut b, t);
            first(&d)
        };
        let ch = |p: u32| (((p >> 16) & 255) as i32, (p & 255) as i32);
        let (r0, b0) = ch(run(0.0));
        assert_eq!((r0, b0), (0x80, 0x80));
        let (r1, b1) = ch(run(0.5));
        let (r2, b2) = ch(run(1.0));
        assert!(r1 > r0 && b1 < b0 && r2 > r1 && b2 < b1);
        let (cr, cb) = ch(run(-1.0));
        assert!(cr < 0x80 && cb > 0x80);
    }

    #[test]
    fn pixelate_makes_uniform_blocks() {
        let mut d = Vec::new();
        for i in 0..16u32 {
            d.extend_from_slice(&(0xFF00_0000 | (i * 16) << 16).to_ne_bytes());
        }
        let mut b = Buf {
            data: &mut d,
            w: 4,
            h: 4,
            stride: 16,
        };
        pixelate(&mut b, 2);
        let px = |x: usize, y: usize| {
            u32::from_ne_bytes(d[y * 16 + x * 4..y * 16 + x * 4 + 4].try_into().unwrap())
        };
        assert_eq!(px(0, 0), px(1, 1));
        assert_ne!(px(0, 0), px(2, 0));
    }

    #[test]
    fn art_effects_keep_alpha() {
        let mut d = Vec::new();
        for i in 0..64u32 {
            let a = if i % 5 == 0 { 0 } else { 100 + i * 2 };
            let c = a / 2;
            d.extend_from_slice(&((a << 24) | (c << 16) | (c / 2 << 8) | c).to_ne_bytes());
        }
        let alphas = |d: &[u8]| d.chunks(4).map(|p| p[3]).collect::<Vec<_>>();
        let want = alphas(&d);
        for which in 0..3 {
            let mut e = d.clone();
            let mut b = Buf {
                data: &mut e,
                w: 8,
                h: 8,
                stride: 32,
            };
            match which {
                0 => grain(&mut b, 0.8),
                1 => watercolor(&mut b, 0.8),
                _ => pixelate(&mut b, 3),
            }
            assert_eq!(alphas(&e), want, "Effekt {which}");
        }
    }

    #[test]
    fn flat_images_are_detected() {
        let mut flat = Vec::new();
        for y in 0..40 {
            for x in 0..40 {
                let c = if x < 20 {
                    [200u8, 30, 30]
                } else if y < 20 {
                    [20, 20, 200]
                } else {
                    [255, 255, 255]
                };
                flat.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        assert!(is_flat(&flat, 40, 40));
        let mut photo = Vec::new();
        for y in 0..40u32 {
            for x in 0..40u32 {
                photo.extend_from_slice(&[(x * 6) as u8, (y * 6) as u8, ((x * y) % 256) as u8, 255]);
            }
        }
        assert!(!is_flat(&photo, 40, 40));
    }

    #[test]
    fn distance_to_edge_is_exact() {
        // Rechteck 20×10 in 30×20
        let (w, h) = (30usize, 20usize);
        let mut a = vec![0f32; w * h];
        for y in 5..15 {
            for x in 5..25 {
                a[y * w + x] = 1.0;
            }
        }
        let d = inside_distance(&a, w, h);
        assert_eq!(d[2 * w + 2], 0.0);
        // Randpixel: ein Pixel bis zum nächsten Pixel außerhalb, eine halbe Pixelbreite bis zur Kante
        assert!((d[5 * w + 10] - 0.5).abs() < 0.01, "{}", d[5 * w + 10]);
        // Mitte: fünf Pixel bis zur oberen/unteren Kante
        let mid = d[9 * w + 15];
        assert!((mid - 4.5).abs() < 0.6, "{mid}");
        // nach rechts nimmt der Abstand wieder ab, entlang der Länge bleibt er gleich
        assert!((d[9 * w + 12] - d[9 * w + 18]).abs() < 0.01);
        // Eckpixel: diagonal weiter weg als an der Kante
        assert!(d[6 * w + 6] > d[6 * w + 12] - 0.01 || d[6 * w + 6] <= d[6 * w + 12]);
    }

    #[test]
    fn relief_shapes_rise_from_the_edge() {
        use crate::model::Lens;
        for kind in Lens::ALL {
            let z = relief_zone(kind, 50.0, 260.0);
            assert!(z > 1.0);
            let rim = profile(kind, 0.0);
            let top = profile(kind, 1.0);
            assert!(rim < 0.01 && (top - 1.0).abs() < 1e-6, "{kind:?}");
            let mut last = -1.0;
            for i in 0..=20 {
                let v = profile(kind, i as f32 / 20.0);
                assert!(v >= last - 1e-6, "{kind:?} nicht steigend");
                last = v;
            }
        }
        // Kristall und Pyramide haben gerade Flanken, die Kugel ist am Rand steiler
        assert!((profile(Lens::Pyramid, 0.5) - 0.5).abs() < 1e-6);
        assert!(profile(Lens::Sphere, 0.1) > 0.4);
        // Kristall erreicht die Fläche früher als die Pyramide
        assert!(relief_zone(Lens::Crystal, 100.0, 260.0) < relief_zone(Lens::Pyramid, 100.0, 260.0));
    }

    #[test]
    fn light_follows_the_slope_direction() {
        assert!(light_delta(0.0, 0.0).abs() < 1e-6);
        // Fläche steigt nach rechts an: sie schaut nach links zum Licht → heller
        assert!(light_delta(0.8, 0.0) > 0.05);
        // Fläche steigt nach links an: schaut weg vom Licht → dunkler
        assert!(light_delta(-0.8, 0.0) < -0.1);
        // oben links hell, unten rechts dunkel
        assert!(light_delta(0.5, 0.5) > 0.0 && light_delta(-0.5, -0.5) < 0.0);
        // eingedrückt (Höhe negativ) kehrt alles um
        let up = relief(crate::model::Lens::Pyramid, &[1.0, 2.0, 3.0], 3.0, 1.0, 260.0);
        let down = relief(crate::model::Lens::Pyramid, &[1.0, 2.0, 3.0], 3.0, -1.0, 260.0);
        let (gu, gd) = (slope(&up.height, 3, 1, 1, 0).0, slope(&down.height, 3, 1, 1, 0).0);
        assert!(gu > 0.0 && gd < 0.0);
        assert!(light_delta(gu, 0.0) > 0.0 && light_delta(gd, 0.0) < 0.0);
    }

    #[test]
    fn textures_are_deterministic_and_bounded() {
        let mut dark = 0;
        let mut bright = 0;
        for y in 0..60 {
            for x in 0..200 {
                let v = brushed(x as f32, y as f32, 200.0, 1.0);
                assert!((-1.0..=1.0).contains(&v));
                assert_eq!(v, brushed(x as f32, y as f32, 200.0, 1.0));
                if v < -0.1 {
                    dark += 1;
                }
                if v > 0.1 {
                    bright += 1;
                }
            }
        }
        assert!(dark > 500 && bright > 500, "{dark} {bright}");
        // Striche laufen waagerecht: in einer Zeile ändern sich Nachbarn wenig, zwischen
        // Zeilen stärker
        let (mut along, mut across) = (0.0, 0.0);
        for y in 0..50 {
            for x in 0..150 {
                let v = brushed(x as f32, y as f32 * 2.0, 600.0, 1.0);
                along += (v - brushed(x as f32 + 1.0, y as f32 * 2.0, 600.0, 1.0)).abs();
                across += (v - brushed(x as f32, y as f32 * 2.0 + 2.0, 600.0, 1.0)).abs();
            }
        }
        assert!(along < across * 0.5, "{along} {across}");
        // Hochglanz: oben mehr Glanz als in der Mitte, Rand hell, weit innen nichts
        let top = gloss(100.0, 20.0, 200.0, 100.0, 20.0, 1.0);
        let mid = gloss(100.0, 60.0, 200.0, 100.0, 40.0, 1.0);
        let rim = gloss(100.0, 0.5, 200.0, 100.0, 0.5, 1.0);
        assert!(top > 0.2 && top > mid + 0.1, "{top} {mid}");
        assert!(rim > 0.2, "{rim}");
        assert!(gloss(100.0, 70.0, 200.0, 100.0, 30.0, 1.0) < 0.05);
    }
}
