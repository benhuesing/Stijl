// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! ICC-Farbprofile (ohne GTK): sRGB → CMYK über das Ausgabeprofil einer Druckbedingung
//! (z. B. „Coated FOGRA39“ oder „PSO Coated v3“).
//!
//! Unterstützt werden Druckerprofile mit Lab- oder XYZ-Verbindungsfarbraum und den
//! Tabellenarten `mft1`, `mft2` (Version 2) sowie `mBA ` (Version 4). Umgerechnet wird
//! farbmetrisch relativ (Tabelle B2A1, ersatzweise B2A0). Neutrale Grautöne und Schwarz
//! werden nur mit Schwarz (K) gedruckt, wie es für Text und Linien üblich ist.

/// Ausgabeprofil, das RGB in CMYK umrechnet.
pub struct CmykProfile {
    pcs_lab: bool,
    lut: Lut,
    /// Beschreibung aus dem Profil (z. B. „Coated FOGRA39 (ISO 12647-2:2004)“)
    pub description: String,
}

enum Lut {
    /// mft1/mft2: Eingangskurven, Gitter, Ausgangskurven (Werte 0–1)
    Legacy {
        input: Vec<Vec<f64>>,
        grid: usize,
        clut: Vec<f64>,
        output: Vec<Vec<f64>>,
        out_ch: usize,
    },
    /// mBA: B-Kurven, Matrix, M-Kurven, Gitter, A-Kurven
    V4 {
        b: Vec<Curve>,
        matrix: Option<[f64; 12]>,
        m: Vec<Curve>,
        grid: [usize; 3],
        clut: Vec<f64>,
        a: Vec<Curve>,
        out_ch: usize,
    },
}

#[derive(Clone, Debug)]
enum Curve {
    Identity,
    Gamma(f64),
    Table(Vec<f64>),
    Param(u16, [f64; 7]),
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(o)?, *d.get(o + 1)?]))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *d.get(o)?,
        *d.get(o + 1)?,
        *d.get(o + 2)?,
        *d.get(o + 3)?,
    ]))
}

fn s15(d: &[u8], o: usize) -> Option<f64> {
    Some(u32_at(d, o)? as i32 as f64 / 65536.0)
}

fn sig(d: &[u8], o: usize) -> Option<&[u8]> {
    d.get(o..o + 4)
}

/// Daten eines Eintrags der Tag-Tabelle.
fn tag<'a>(d: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let n = u32_at(d, 128)? as usize;
    for i in 0..n.min(200) {
        let e = 132 + 12 * i;
        if sig(d, e)? == name {
            let off = u32_at(d, e + 4)? as usize;
            let len = u32_at(d, e + 8)? as usize;
            return d.get(off..off.checked_add(len)?);
        }
    }
    None
}

impl Curve {
    fn eval(&self, x: f64) -> f64 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Curve::Identity => x,
            Curve::Gamma(g) => x.powf(*g),
            Curve::Table(t) => interp1(t, x),
            Curve::Param(kind, p) => {
                let (g, a, b, c, d, e, f) = (p[0], p[1], p[2], p[3], p[4], p[5], p[6]);
                let y = match kind {
                    0 => x.powf(g),
                    1 => {
                        if x >= -b / a {
                            (a * x + b).max(0.0).powf(g)
                        } else {
                            0.0
                        }
                    }
                    2 => {
                        if x >= -b / a {
                            (a * x + b).max(0.0).powf(g) + c
                        } else {
                            c
                        }
                    }
                    3 => {
                        if x >= d {
                            (a * x + b).max(0.0).powf(g)
                        } else {
                            c * x
                        }
                    }
                    _ => {
                        if x >= d {
                            (a * x + b).max(0.0).powf(g) + e
                        } else {
                            c * x + f
                        }
                    }
                };
                y.clamp(0.0, 1.0)
            }
        }
    }
}

fn interp1(t: &[f64], x: f64) -> f64 {
    if t.is_empty() {
        return x;
    }
    if t.len() == 1 {
        return t[0];
    }
    let p = x.clamp(0.0, 1.0) * (t.len() - 1) as f64;
    let i = (p.floor() as usize).min(t.len() - 2);
    let f = p - i as f64;
    t[i] * (1.0 - f) + t[i + 1] * f
}

/// Kurve ('curv' oder 'para') ab `o`; liefert Kurve und Länge in Bytes (auf 4 aufgerundet).
fn read_curve(d: &[u8], o: usize) -> Option<(Curve, usize)> {
    match sig(d, o)? {
        b"curv" => {
            let n = u32_at(d, o + 8)? as usize;
            let c = match n {
                0 => Curve::Identity,
                1 => Curve::Gamma(u16_at(d, o + 12)? as f64 / 256.0),
                _ => {
                    let mut t = Vec::with_capacity(n);
                    for i in 0..n {
                        t.push(u16_at(d, o + 12 + 2 * i)? as f64 / 65535.0);
                    }
                    Curve::Table(t)
                }
            };
            Some((c, (12 + 2 * n + 3) & !3))
        }
        b"para" => {
            let kind = u16_at(d, o + 8)?;
            let count = match kind {
                0 => 1,
                1 => 3,
                2 => 4,
                3 => 5,
                4 => 7,
                _ => return None,
            };
            let mut p = [0.0; 7];
            for (i, v) in p.iter_mut().enumerate().take(count) {
                *v = s15(d, o + 12 + 4 * i)?;
            }
            Some((Curve::Param(kind, p), (12 + 4 * count + 3) & !3))
        }
        _ => None,
    }
}

fn read_curves(d: &[u8], o: usize, n: usize) -> Option<Vec<Curve>> {
    let mut v = Vec::with_capacity(n);
    let mut pos = o;
    for _ in 0..n {
        let (c, len) = read_curve(d, pos)?;
        v.push(c);
        pos += len;
    }
    Some(v)
}

fn read_lut(t: &[u8]) -> Option<Lut> {
    match sig(t, 0)? {
        typ @ (b"mft2" | b"mft1") => {
            let wide = typ == b"mft2";
            let in_ch = *t.get(8)? as usize;
            let out_ch = *t.get(9)? as usize;
            let grid = *t.get(10)? as usize;
            if in_ch != 3 || out_ch == 0 || out_ch > 8 || grid < 2 {
                return None;
            }
            let (in_n, out_n, mut pos) = if wide {
                (u16_at(t, 48)? as usize, u16_at(t, 50)? as usize, 52)
            } else {
                (256, 256, 48)
            };
            let val = |p: usize| -> Option<f64> {
                if wide {
                    Some(u16_at(t, p)? as f64 / 65535.0)
                } else {
                    Some(*t.get(p)? as f64 / 255.0)
                }
            };
            let sz = if wide { 2 } else { 1 };
            let mut input = Vec::new();
            for _ in 0..in_ch {
                let mut c = Vec::with_capacity(in_n);
                for i in 0..in_n {
                    c.push(val(pos + i * sz)?);
                }
                pos += in_n * sz;
                input.push(c);
            }
            let cells = grid.checked_pow(3)? * out_ch;
            let mut clut = Vec::with_capacity(cells);
            for i in 0..cells {
                clut.push(val(pos + i * sz)?);
            }
            pos += cells * sz;
            let mut output = Vec::new();
            for _ in 0..out_ch {
                let mut c = Vec::with_capacity(out_n);
                for i in 0..out_n {
                    c.push(val(pos + i * sz)?);
                }
                pos += out_n * sz;
                output.push(c);
            }
            Some(Lut::Legacy {
                input,
                grid,
                clut,
                output,
                out_ch,
            })
        }
        b"mBA " => {
            let in_ch = *t.get(8)? as usize;
            let out_ch = *t.get(9)? as usize;
            if in_ch != 3 || out_ch == 0 || out_ch > 8 {
                return None;
            }
            let off = |k: usize| u32_at(t, 12 + 4 * k).map(|v| v as usize);
            let (ob, om, omc, oc, oa) = (off(0)?, off(1)?, off(2)?, off(3)?, off(4)?);
            let b = if ob > 0 {
                read_curves(t, ob, 3)?
            } else {
                vec![Curve::Identity; 3]
            };
            let matrix = if om > 0 {
                let mut m = [0.0; 12];
                for (i, v) in m.iter_mut().enumerate() {
                    *v = s15(t, om + 4 * i)?;
                }
                Some(m)
            } else {
                None
            };
            let m = if omc > 0 {
                read_curves(t, omc, 3)?
            } else {
                vec![Curve::Identity; 3]
            };
            let a = if oa > 0 {
                read_curves(t, oa, out_ch)?
            } else {
                vec![Curve::Identity; out_ch]
            };
            if oc == 0 {
                return None;
            }
            let grid = [
                *t.get(oc)? as usize,
                *t.get(oc + 1)? as usize,
                *t.get(oc + 2)? as usize,
            ];
            if grid.iter().any(|g| *g < 2) {
                return None;
            }
            let prec = *t.get(oc + 16)? as usize;
            let cells = grid[0] * grid[1] * grid[2] * out_ch;
            let mut clut = Vec::with_capacity(cells);
            for i in 0..cells {
                let v = match prec {
                    1 => *t.get(oc + 20 + i)? as f64 / 255.0,
                    _ => u16_at(t, oc + 20 + 2 * i)? as f64 / 65535.0,
                };
                clut.push(v);
            }
            Some(Lut::V4 {
                b,
                matrix,
                m,
                grid,
                clut,
                a,
                out_ch,
            })
        }
        _ => None,
    }
}

/// Dreidimensionale Tabelle linear zwischen den Gitterpunkten auswerten.
fn clut3(clut: &[f64], grid: [usize; 3], out_ch: usize, x: [f64; 3], out: &mut [f64]) {
    let mut idx = [0usize; 3];
    let mut fr = [0f64; 3];
    for k in 0..3 {
        let p = x[k].clamp(0.0, 1.0) * (grid[k] - 1) as f64;
        let i = (p.floor() as usize).min(grid[k] - 2);
        idx[k] = i;
        fr[k] = p - i as f64;
    }
    for o in out.iter_mut().take(out_ch) {
        *o = 0.0;
    }
    for corner in 0..8 {
        let mut w = 1.0;
        let mut cell = 0;
        for k in 0..3 {
            let bit = (corner >> (2 - k)) & 1;
            w *= if bit == 1 { fr[k] } else { 1.0 - fr[k] };
            cell = cell * grid[k] + idx[k] + bit;
        }
        if w == 0.0 {
            continue;
        }
        for (c, o) in out.iter_mut().enumerate().take(out_ch) {
            *o += w * clut[cell * out_ch + c];
        }
    }
}

/// sRGB (0–1) → CIE XYZ mit D50-Weißpunkt (Bradford-angepasst, wie im ICC-Standard).
pub fn srgb_to_xyz_d50(r: f64, g: f64, b: f64) -> [f64; 3] {
    let lin = |v: f64| {
        let v = v.clamp(0.0, 1.0);
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (lin(r), lin(g), lin(b));
    [
        0.4360747 * r + 0.3850649 * g + 0.1430804 * b,
        0.2225045 * r + 0.7168786 * g + 0.0606169 * b,
        0.0139322 * r + 0.0971045 * g + 0.7141733 * b,
    ]
}

/// XYZ (D50) → CIE Lab.
pub fn xyz_to_lab(xyz: [f64; 3]) -> [f64; 3] {
    let wp = [0.9642, 1.0, 0.8249];
    let f = |t: f64| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(xyz[0] / wp[0]), f(xyz[1] / wp[1]), f(xyz[2] / wp[2]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// Lesbare Beschreibung aus einem Profil ('desc' oder 'mluc').
pub fn description(d: &[u8]) -> Option<String> {
    let t = tag(d, b"desc")?;
    match sig(t, 0)? {
        b"desc" => {
            let n = u32_at(t, 8)? as usize;
            let s = t.get(12..12 + n)?;
            let s: String = s.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect();
            Some(s.trim().to_string())
        }
        b"mluc" => {
            let n = u32_at(t, 8)? as usize;
            if n == 0 {
                return None;
            }
            let len = u32_at(t, 20)? as usize;
            let off = u32_at(t, 24)? as usize;
            let raw = t.get(off..off + len)?;
            let units: Vec<u16> = raw
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            Some(
                String::from_utf16_lossy(&units)
                    .trim_end_matches('\0')
                    .trim()
                    .to_string(),
            )
        }
        _ => None,
    }
}

/// Bekannte Druckbedingung zu einer Profilbeschreibung (für PDF/X).
pub fn output_condition(desc: &str) -> &'static str {
    let d = desc.to_lowercase();
    if d.contains("fogra51") || d.contains("pso coated v3") {
        "FOGRA51"
    } else if d.contains("fogra52") || d.contains("pso uncoated v3") {
        "FOGRA52"
    } else if d.contains("fogra39") || d.contains("iso coated v2") || d.contains("isocoated_v2") {
        "FOGRA39"
    } else if d.contains("fogra29") || d.contains("uncoated") {
        "FOGRA29"
    } else if d.contains("gracol") {
        "CGATS TR 006"
    } else if d.contains("swop") {
        "CGATS TR 001"
    } else {
        "Custom"
    }
}

impl CmykProfile {
    /// Profil lesen; `None`, wenn es kein CMYK-Ausgabeprofil mit lesbarer Tabelle ist.
    pub fn parse(d: &[u8]) -> Option<CmykProfile> {
        if d.len() < 132 || sig(d, 36)? != b"acsp" {
            return None;
        }
        if sig(d, 16)? != b"CMYK" {
            return None;
        }
        let pcs_lab = match sig(d, 20)? {
            b"Lab " => true,
            b"XYZ " => false,
            _ => return None,
        };
        let lut = tag(d, b"B2A1")
            .and_then(read_lut)
            .or_else(|| tag(d, b"B2A0").and_then(read_lut))?;
        let out_ch = match &lut {
            Lut::Legacy { out_ch, .. } | Lut::V4 { out_ch, .. } => *out_ch,
        };
        if out_ch != 4 {
            return None;
        }
        Some(CmykProfile {
            pcs_lab,
            lut,
            description: description(d).unwrap_or_default(),
        })
    }

    /// sRGB (0–1) → CMYK (0–1). Neutrale Grautöne nur mit Schwarz.
    pub fn convert(&self, r: f64, g: f64, b: f64) -> [f64; 4] {
        if let Some(k) = neutral_k(r, g, b) {
            return [0.0, 0.0, 0.0, k];
        }
        let xyz = srgb_to_xyz_d50(r, g, b);
        let mut out = [0f64; 4];
        match &self.lut {
            Lut::Legacy {
                input,
                grid,
                clut,
                output,
                out_ch,
            } => {
                let x = if self.pcs_lab {
                    // Lab in der Version-2-Kodierung (L 100 → 0xFF00)
                    let lab = xyz_to_lab(xyz);
                    [
                        (lab[0] / 100.0 * 65280.0 / 65535.0).clamp(0.0, 1.0),
                        ((lab[1] + 128.0) * 256.0 / 65535.0).clamp(0.0, 1.0),
                        ((lab[2] + 128.0) * 256.0 / 65535.0).clamp(0.0, 1.0),
                    ]
                } else {
                    [
                        xyz[0] * 32768.0 / 65535.0,
                        xyz[1] * 32768.0 / 65535.0,
                        xyz[2] * 32768.0 / 65535.0,
                    ]
                };
                let x = [
                    interp1(&input[0], x[0]),
                    interp1(&input[1], x[1]),
                    interp1(&input[2], x[2]),
                ];
                let mut tmp = [0f64; 8];
                clut3(clut, [*grid; 3], *out_ch, x, &mut tmp);
                for c in 0..4 {
                    out[c] = interp1(&output[c], tmp[c]);
                }
            }
            Lut::V4 {
                b,
                matrix,
                m,
                grid,
                clut,
                a,
                out_ch,
            } => {
                let x = if self.pcs_lab {
                    let lab = xyz_to_lab(xyz);
                    [lab[0] / 100.0, (lab[1] + 128.0) / 255.0, (lab[2] + 128.0) / 255.0]
                } else {
                    [
                        xyz[0] * 32768.0 / 65535.0,
                        xyz[1] * 32768.0 / 65535.0,
                        xyz[2] * 32768.0 / 65535.0,
                    ]
                };
                let mut v = [b[0].eval(x[0]), b[1].eval(x[1]), b[2].eval(x[2])];
                if let Some(mt) = matrix {
                    let o = [
                        mt[0] * v[0] + mt[1] * v[1] + mt[2] * v[2] + mt[9],
                        mt[3] * v[0] + mt[4] * v[1] + mt[5] * v[2] + mt[10],
                        mt[6] * v[0] + mt[7] * v[1] + mt[8] * v[2] + mt[11],
                    ];
                    v = [o[0].clamp(0.0, 1.0), o[1].clamp(0.0, 1.0), o[2].clamp(0.0, 1.0)];
                }
                v = [m[0].eval(v[0]), m[1].eval(v[1]), m[2].eval(v[2])];
                let mut tmp = [0f64; 8];
                clut3(clut, *grid, *out_ch, v, &mut tmp);
                for c in 0..4 {
                    out[c] = a[c].eval(tmp[c]);
                }
            }
        }
        out.map(|v| v.clamp(0.0, 1.0))
    }
}

/// Neutrales Grau (auch Schwarz und Weiß): Anteil Schwarz, sonst `None`.
fn neutral_k(r: f64, g: f64, b: f64) -> Option<f64> {
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    (mx - mn < 0.004).then(|| (1.0 - (r + g + b) / 3.0).clamp(0.0, 1.0))
}

/// Einfache Umrechnung ohne Profil (Grautöne nur mit Schwarz).
pub fn naive_cmyk(r: f64, g: f64, b: f64) -> [f64; 4] {
    if let Some(k) = neutral_k(r, g, b) {
        return [0.0, 0.0, 0.0, k];
    }
    let k = 1.0 - r.max(g).max(b);
    if k >= 1.0 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let d = 1.0 - k;
    [(1.0 - r - k) / d, (1.0 - g - k) / d, (1.0 - b - k) / d, k].map(|v| v.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOGRA: &str = "/usr/share/texlive/texmf-dist/tex/generic/colorprofiles/FOGRA39L_coated.icc";

    #[test]
    fn color_math() {
        // sRGB-Weiß ist der D50-Weißpunkt
        let w = srgb_to_xyz_d50(1.0, 1.0, 1.0);
        assert!((w[0] - 0.9642).abs() < 1e-3 && (w[1] - 1.0).abs() < 1e-3 && (w[2] - 0.8249).abs() < 1e-3);
        let lab = xyz_to_lab(w);
        assert!(
            (lab[0] - 100.0).abs() < 0.05 && lab[1].abs() < 0.1 && lab[2].abs() < 0.1,
            "{lab:?}"
        );
        let red = xyz_to_lab(srgb_to_xyz_d50(1.0, 0.0, 0.0));
        assert!(
            (red[0] - 54.29).abs() < 0.5 && (red[1] - 80.8).abs() < 1.5,
            "{red:?}"
        );
        // ohne Profil
        assert_eq!(naive_cmyk(1.0, 0.0, 0.0), [0.0, 1.0, 1.0, 0.0]);
        assert_eq!(naive_cmyk(0.0, 0.0, 0.0), [0.0, 0.0, 0.0, 1.0]);
        let g = naive_cmyk(0.5, 0.5, 0.5);
        assert!(g[0] == 0.0 && (g[3] - 0.5).abs() < 1e-9);
        assert_eq!(output_condition("Coated FOGRA39 (ISO 12647-2:2004)"), "FOGRA39");
        assert_eq!(output_condition("PSO Coated v3"), "FOGRA51");
    }

    #[test]
    fn synthetic_mft2_profile() {
        // kleines Profil: Lab → CMYK, 2er-Gitter, K = 1 − L, C = a-Anteil
        let mut t = Vec::new();
        t.extend_from_slice(b"mft2\0\0\0\0");
        t.extend_from_slice(&[3, 4, 2, 0]);
        for i in 0..9 {
            let v: i32 = if i % 4 == 0 { 0x10000 } else { 0 };
            t.extend_from_slice(&v.to_be_bytes());
        }
        t.extend_from_slice(&2u16.to_be_bytes());
        t.extend_from_slice(&2u16.to_be_bytes());
        for _ in 0..3 {
            t.extend_from_slice(&0u16.to_be_bytes());
            t.extend_from_slice(&65535u16.to_be_bytes());
        }
        for l in 0..2 {
            for a in 0..2 {
                for _b in 0..2 {
                    let c = a as u16 * 65535;
                    let k = (1 - l) as u16 * 65535;
                    for v in [c, 0, 0, k] {
                        t.extend_from_slice(&v.to_be_bytes());
                    }
                }
            }
        }
        for _ in 0..4 {
            t.extend_from_slice(&0u16.to_be_bytes());
            t.extend_from_slice(&65535u16.to_be_bytes());
        }
        let mut d = vec![0u8; 128];
        d[16..20].copy_from_slice(b"CMYK");
        d[20..24].copy_from_slice(b"Lab ");
        d[36..40].copy_from_slice(b"acsp");
        d.extend_from_slice(&1u32.to_be_bytes());
        let off = 128 + 4 + 12;
        d.extend_from_slice(b"B2A1");
        d.extend_from_slice(&(off as u32).to_be_bytes());
        d.extend_from_slice(&(t.len() as u32).to_be_bytes());
        d.extend_from_slice(&t);
        let p = CmykProfile::parse(&d).expect("Profil");
        // Weiß/Schwarz/Grau: nur K
        assert_eq!(p.convert(1.0, 1.0, 1.0), [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(p.convert(0.0, 0.0, 0.0), [0.0, 0.0, 0.0, 1.0]);
        // Rot: L ≈ 54, a ≈ 81 → K ≈ 0,46·(0xFF00/0xFFFF), C ≈ (81+128)·256/65535
        let c = p.convert(1.0, 0.0, 0.0);
        assert!((c[3] - (1.0 - 0.543 * 65280.0 / 65535.0)).abs() < 0.02, "{c:?}");
        assert!((c[0] - (80.8 + 128.0) * 256.0 / 65535.0).abs() < 0.02, "{c:?}");
    }

    #[test]
    fn fogra39_if_installed() {
        let Ok(d) = std::fs::read(FOGRA) else { return };
        let p = CmykProfile::parse(&d).expect("FOGRA39 lesbar");
        assert!(p.description.contains("FOGRA39"), "{}", p.description);
        let red = p.convert(1.0, 0.0, 0.0);
        // Rot: kaum Cyan und Schwarz, viel Magenta und Gelb
        assert!(
            red[0] < 0.1 && red[1] > 0.85 && red[2] > 0.8 && red[3] < 0.1,
            "{red:?}"
        );
        let blue = p.convert(0.0, 0.0, 1.0);
        assert!(blue[0] > 0.8 && blue[1] > 0.6 && blue[2] < 0.15, "{blue:?}");
        let yellow = p.convert(1.0, 1.0, 0.0);
        assert!(
            yellow[0] < 0.15 && yellow[1] < 0.15 && yellow[2] > 0.85,
            "{yellow:?}"
        );
        // ein Hautton: alles im mittleren Bereich, Summe plausibel
        let skin = p.convert(0.9, 0.7, 0.6);
        let sum: f64 = skin.iter().sum();
        assert!(sum > 0.3 && sum < 1.2, "{skin:?}");
    }
}
