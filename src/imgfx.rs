//! Pixelfilter für Bilder (ohne GTK): Farbfilter und Kunsteffekte.
//!
//! Die Puffer haben das Format von Cairo (ARGB32, vormultipliziert; als `u32` in der
//! Byte-Reihenfolge des Rechners gelesen). Die Deckkraft bleibt bei allen Filtern unverändert.

use crate::model::{ImgFilter, Lens};

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

/// Wasserfarben: weichgezeichnete, leicht abgestufte Farben, dunklere Farbränder an
/// Kanten und Papierstruktur. Die Deckkraft bleibt unverändert.
pub fn watercolor(b: &mut Buf, k: f64) {
    let (w, h) = (b.w, b.h);
    if w < 3 || h < 3 {
        return;
    }
    let k = k.clamp(0.0, 1.0) as f32;
    let n = w * h;
    let mut planes = [vec![0f32; n], vec![0f32; n], vec![0f32; n], vec![0f32; n]];
    let mut orig = vec![[0f32; 3]; n];
    let mut alpha = vec![0u32; n];
    for y in 0..h {
        for x in 0..w {
            let p = b.px(x, y);
            let i = y * w + x;
            let a = p >> 24;
            alpha[i] = a;
            let af = a as f32 / 255.0;
            planes[0][i] = ((p >> 16) & 0xFF) as f32 / 255.0;
            planes[1][i] = ((p >> 8) & 0xFF) as f32 / 255.0;
            planes[2][i] = (p & 0xFF) as f32 / 255.0;
            planes[3][i] = af;
            let (_, c) = split(p);
            orig[i] = c;
        }
    }
    let r = ((w.min(h) as f32) * (0.003 + 0.012 * k)).round().max(1.0) as usize;
    for p in planes.iter_mut() {
        blur_plane(p, w, h, r);
        blur_plane(p, w, h, r);
    }
    // unvormultiplizierte weichgezeichnete Farbe
    let mut soft = vec![[0f32; 3]; n];
    let mut lum = vec![0f32; n];
    for i in 0..n {
        let a = planes[3][i].max(1e-4);
        soft[i] = [
            (planes[0][i] / a).clamp(0.0, 1.0),
            (planes[1][i] / a).clamp(0.0, 1.0),
            (planes[2][i] / a).clamp(0.0, 1.0),
        ];
        lum[i] = 0.299 * soft[i][0] + 0.587 * soft[i][1] + 0.114 * soft[i][2];
    }
    let levels = 16.0 - 9.0 * k;
    let scale = (w.min(h) as f32 / 400.0).max(1.0);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if alpha[i] == 0 {
                continue;
            }
            let (x0, x1) = (x.saturating_sub(1), (x + 1).min(w - 1));
            let (y0, y1) = (y.saturating_sub(1), (y + 1).min(h - 1));
            let gx = lum[y * w + x1] - lum[y * w + x0];
            let gy = lum[y1 * w + x] - lum[y0 * w + x];
            let edge = ((gx * gx + gy * gy).sqrt() * 5.0 / scale.sqrt()).clamp(0.0, 1.0);
            let paper = value_noise(x as f32, y as f32, 2.5 * scale, 5) - 0.5;
            let cloud = value_noise(x as f32, y as f32, 40.0 * scale, 9) - 0.5;
            let dark = 1.0 - 0.45 * k * edge;
            let tex = 1.0 + paper * 0.16 * k + cloud * 0.18 * k;
            let mut c = soft[i];
            for ch in c.iter_mut() {
                let post = (*ch * levels).round() / levels;
                *ch = (*ch * 0.5 + post * 0.5) * dark * tex;
            }
            c = mix(c, orig[i], 0.3 * (1.0 - k));
            b.set(x, y, join(alpha[i], c));
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
    // eindimensionale Abstandstransformation (Felzenszwalb) erst je Spalte, dann je Zeile
    let n = w.max(h);
    let mut line = vec![0f64; n];
    let mut out = vec![0f64; n];
    let mut v = vec![0usize; n];
    let mut z = vec![0f64; n + 1];
    let mut run = |line: &[f64], out: &mut [f64], m: usize| {
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
    };
    for x in 0..w {
        for y in 0..h {
            line[y] = f[y * w + x];
        }
        run(&line, &mut out, h);
        for y in 0..h {
            f[y * w + x] = out[y];
        }
    }
    for y in 0..h {
        line[..w].copy_from_slice(&f[y * w..y * w + w]);
        run(&line, &mut out, w);
        f[y * w..y * w + w].copy_from_slice(&out[..w]);
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

#[cfg(test)]
mod tests {
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
