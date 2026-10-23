// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! Export als PNG, JPG, PDF oder SVG.

use crate::i18n::{tr, trf};
use std::path::Path;

use gtk::{cairo, gdk_pixbuf, glib};

use crate::model::{Color, Document, Object, Shape};
use crate::render::draw_document;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Pdf,
    Svg,
}

impl Format {
    pub const ALL: [Format; 4] = [Format::Png, Format::Jpeg, Format::Pdf, Format::Svg];
    pub fn label(self) -> &'static str {
        match self {
            Format::Png => tr("PNG-Bild"),
            Format::Jpeg => tr("JPG-Bild"),
            Format::Pdf => tr("PDF-Dokument"),
            Format::Svg => tr("SVG-Datei"),
        }
    }
    pub fn ext(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Pdf => "pdf",
            Format::Svg => "svg",
        }
    }
    pub fn pattern(self) -> &'static [&'static str] {
        match self {
            Format::Png => &["*.png"],
            Format::Jpeg => &["*.jpg", "*.jpeg"],
            Format::Pdf => &["*.pdf"],
            Format::Svg => &["*.svg"],
        }
    }
}

/// Farben im PDF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    /// RGB wie auf dem Bildschirm
    Rgb,
    /// CMYK für den Druck
    Cmyk,
    /// CMYK als PDF/X-4 mit eingebettetem Profil (für Druckereien)
    PdfX,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub format: Format,
    /// Hintergrund des Dokuments mit ausgeben (sonst transparent; JPG: weiß).
    pub background: bool,
    /// Auflösungsfaktor für Rasterformate (1 = Dokumentpixel).
    pub scale: f64,
    /// JPG-Qualität 1–100.
    pub quality: u8,
    /// SVG: verwendete Schriften als `@font-face` in die Datei einbetten.
    pub embed_fonts: bool,
    /// Auflösung der Effekte (Schatten, Glas, 3D, Texturen, Kunsteffekte) in dpi – PDF und SVG
    pub effect_dpi: f64,
    /// PDF: Text als durchsuchbare Schrift (sonst als Umrisse)
    pub pdf_text: bool,
    /// PDF: Farben
    pub color: ColorMode,
    /// PDF: Farbprofil der Druckbedingung (ICC)
    pub profile: Option<std::path::PathBuf>,
    /// PDF: Anschnitt (Beschnittzugabe) in mm
    pub bleed_mm: f64,
    /// Titel des Dokuments (PDF)
    pub title: String,
}

/// Bekannte Orte für ICC-Profile; liefert lesbare CMYK-Ausgabeprofile (Pfad, Beschreibung),
/// gängige Druckbedingungen zuerst.
pub fn find_profiles() -> Vec<(std::path::PathBuf, String)> {
    let mut dirs: Vec<std::path::PathBuf> = vec![
        "/usr/share/color/icc".into(),
        "/usr/local/share/color/icc".into(),
        "/var/lib/color/icc".into(),
        "/usr/share/texlive/texmf-dist/tex/generic/colorprofiles".into(),
        "/usr/share/texmf/tex/generic/colorprofiles".into(),
    ];
    dirs.push(glib::user_data_dir().join("color").join("icc"));
    // Windows: Farbprofile des Systems
    if let Some(root) = std::env::var_os("SystemRoot") {
        dirs.push(
            std::path::PathBuf::from(root)
                .join("System32")
                .join("spool")
                .join("drivers")
                .join("color"),
        );
    }
    // macOS
    dirs.push("/Library/ColorSync/Profiles".into());
    dirs.push(
        glib::home_dir()
            .join("Library")
            .join("ColorSync")
            .join("Profiles"),
    );
    dirs.push(glib::home_dir().join(".color").join("icc"));
    let mut found: Vec<(std::path::PathBuf, String)> = Vec::new();
    let mut stack: Vec<(std::path::PathBuf, u8)> = dirs.into_iter().map(|d| (d, 0)).collect();
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if depth < 3 {
                    stack.push((p, depth + 1));
                }
                continue;
            }
            let ext = p.extension().and_then(|x| x.to_str()).map(|x| x.to_lowercase());
            if !matches!(ext.as_deref(), Some("icc") | Some("icm")) {
                continue;
            }
            if e.metadata().map(|m| m.len() > 8 << 20).unwrap_or(true) {
                continue;
            }
            let Ok(data) = std::fs::read(&p) else { continue };
            if let Some(prof) = crate::icc::CmykProfile::parse(&data) {
                let desc = if prof.description.is_empty() {
                    p.file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default()
                } else {
                    prof.description.clone()
                };
                if !found.iter().any(|(_, d)| *d == desc) {
                    found.push((p, desc));
                }
            }
        }
    }
    let rank = |d: &str| match crate::icc::output_condition(d) {
        "FOGRA51" => 0,
        "FOGRA39" => 1,
        "FOGRA52" => 2,
        "FOGRA29" => 3,
        "Custom" => 9,
        _ => 5,
    };
    found.sort_by(|a, b| rank(&a.1).cmp(&rank(&b.1)).then(a.1.cmp(&b.1)));
    found
}

/// Objekte mit Effekten, die andere Programme nicht kennen, als Bild (für den SVG-Export).
fn needs_raster(o: &Object) -> bool {
    let fx = &o.style.fx;
    fx.tex.is_some()
        || fx.lens.is_some_and(|(_, k)| k.abs() > 0.004)
        || fx.art.is_some()
        || fx.mirror.is_some()
        || (matches!(o.shape, Shape::Image { .. }) && (fx.tint.is_some() || fx.temp.abs() > 1e-9))
}

/// Effekt-Objekte durch Bilder ersetzen (so sehen sie überall gleich aus).
fn flatten_effects(doc: &Document, res: f64) -> Document {
    let mut out = doc.clone();
    for layer in out.layers.iter_mut() {
        for o in layer.objects.iter_mut() {
            if !needs_raster(o) {
                continue;
            }
            if let Some((png, r)) = crate::render::rasterize_object(doc, o.id, res) {
                let blend = o.style.blend;
                o.shape = Shape::Image {
                    xf: crate::geom::Affine::translate(r.x0, r.y0),
                    w: r.width(),
                    h: r.height(),
                    data: std::rc::Rc::new(png),
                    clip: None,
                };
                o.style = crate::model::Style {
                    blend,
                    ..Default::default()
                };
                o.style.fill = None;
                o.style.stroke = None;
            }
        }
    }
    out
}

/// Kästen hinter Punkttext und Text auf Pfad als eigene Formen vor den Text legen (andere
/// Programme kennen die Kästen nicht).
fn split_text_boxes(doc: &mut Document) {
    let mut next = doc
        .layers
        .iter()
        .flat_map(|l| l.objects.iter())
        .map(|o| o.id)
        .max()
        .unwrap_or(0)
        + 1;
    for layer in doc.layers.iter_mut() {
        let mut out: Vec<Object> = Vec::with_capacity(layer.objects.len());
        for mut o in layer.objects.drain(..) {
            let boxed = matches!(o.shape, Shape::Text { .. } | Shape::TextPath { .. })
                && o.shape.text_format().is_some_and(|f| f.has_box());
            if boxed {
                if let (Some(sub), Some(f)) =
                    (crate::render::text_box_outline(&o.shape), o.shape.text_format())
                {
                    let mut st = crate::model::Style {
                        fill: f.box_fill,
                        stroke: f.box_stroke,
                        width: f.box_width,
                        ..Default::default()
                    };
                    st.gradient = None;
                    out.push(Object {
                        id: next,
                        shape: Shape::Path {
                            subpaths: sub,
                            evenodd: false,
                        },
                        style: st,
                        angle: o.angle,
                        group: o.group,
                        locked: o.locked,
                        name: String::new(),
                    });
                    next += 1;
                }
                if let Some(f) = o.shape.text_format_mut() {
                    f.box_fill = None;
                    f.box_stroke = None;
                }
            }
            out.push(o);
        }
        layer.objects = out;
    }
    doc.next_id = doc.next_id.max(next);
}

/// PDF mit Cairo schreiben (alle Seiten), dann bei Bedarf nach CMYK/PDF/X umwandeln.
fn pdf_bytes(docs: &[Document], opt: &Options, background: bool) -> Result<Vec<u8>, String> {
    // 96 Pixel = 72 Punkte
    let k = 72.0 / 96.0;
    let bleed = opt.bleed_mm.max(0.0) * 72.0 / 25.4;
    let first = docs.first().ok_or(tr("Keine Zeichenfläche gewählt"))?;
    let tmp = std::env::temp_dir().join(format!("stijl-export-{}.pdf", std::process::id()));
    let prev_res = crate::render::set_effect_resolution(opt.effect_dpi / 72.0);
    let prev_text = crate::render::set_text_as_glyphs(opt.pdf_text);
    let result = (|| -> Result<(), String> {
        let surface = cairo::PdfSurface::new(
            first.width * k + 2.0 * bleed,
            first.height * k + 2.0 * bleed,
            &tmp,
        )
        .map_err(|e| e.to_string())?;
        {
            let cr = cairo::Context::new(&surface).map_err(|e| e.to_string())?;
            for doc in docs {
                surface
                    .set_size(doc.width * k + 2.0 * bleed, doc.height * k + 2.0 * bleed)
                    .map_err(|e| e.to_string())?;
                let _ = cr.save();
                if let (true, Some(c)) = (background, doc.background) {
                    cr.set_source_rgba(c.r, c.g, c.b, c.a);
                    cr.paint().map_err(|e| e.to_string())?;
                }
                cr.translate(bleed, bleed);
                cr.scale(k, k);
                draw_document(&cr, doc);
                let _ = cr.restore();
                cr.show_page().map_err(|e| e.to_string())?;
            }
        }
        surface.finish();
        Ok(())
    })();
    crate::render::set_effect_resolution(prev_res);
    crate::render::set_text_as_glyphs(prev_text);
    let data = result.and_then(|_| std::fs::read(&tmp).map_err(|e| e.to_string()));
    let _ = std::fs::remove_file(&tmp);
    let data = data?;
    if opt.color == ColorMode::Rgb {
        return Ok(data);
    }
    let icc = match &opt.profile {
        Some(p) => Some(std::fs::read(p).map_err(|e| trf("Farbprofil: {}", &[&(e)]))?),
        None => None,
    };
    let profile = match &icc {
        Some(d) => Some(
            crate::icc::CmykProfile::parse(d)
                .ok_or(tr("Das Farbprofil ist kein lesbares CMYK-Druckprofil"))?,
        ),
        None => None,
    };
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    crate::pdfx::convert(
        &data,
        &crate::pdfx::Options {
            profile: profile.as_ref().zip(icc.as_deref()),
            pdfx: opt.color == ColorMode::PdfX,
            bleed,
            title: opt.title.clone(),
            time,
        },
    )
}

/// Schriftdatei (TrueType/OpenType) zu einer Familie über fontconfig; `None`, wenn die
/// Schrift fehlt, ersetzt würde, zu groß ist oder in einer Sammlung (.ttc) steckt.
fn font_file(family: &str, bold: bool, italic: bool) -> Option<(Vec<u8>, &'static str)> {
    // Fontconfig (fc-match) gibt es unter Windows und macOS nicht: Schriften werden dort nicht
    // eingebettet
    if cfg!(any(windows, target_os = "macos")) {
        return None;
    }
    let pattern = format!(
        "{family}:{}:{}",
        if bold { "weight=bold" } else { "weight=regular" },
        if italic { "slant=italic" } else { "slant=roman" }
    );
    let out = std::process::Command::new("fc-match")
        .args(["-f", "%{file}\n%{family}", &pattern])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (file, found) = text.split_once('\n')?;
    // nur die gewünschte Familie, keine Ersatzschrift
    let wanted = family.to_lowercase();
    if !found.split(',').any(|f| f.trim().to_lowercase() == wanted) {
        return None;
    }
    let path = std::path::Path::new(file.trim());
    let kind = match path.extension()?.to_str()?.to_lowercase().as_str() {
        "ttf" => "ttf",
        "otf" => "otf",
        _ => return None,
    };
    let data = std::fs::read(path).ok()?;
    (data.len() <= 8 << 20).then_some((data, kind))
}

fn raster(doc: &Document, scale: f64, bg: Option<Color>) -> Result<cairo::ImageSurface, String> {
    let w = (doc.width * scale).round().max(1.0) as i32;
    let h = (doc.height * scale).round().max(1.0) as i32;
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).map_err(|e| e.to_string())?;
    {
        let cr = cairo::Context::new(&surface).map_err(|e| e.to_string())?;
        if let Some(c) = bg {
            cr.set_source_rgba(c.r, c.g, c.b, c.a);
            cr.paint().map_err(|e| e.to_string())?;
        }
        // genau auf die ganzzahlige Bildgröße strecken (sonst bleibt am Rand eine nur
        // teilweise gefüllte Pixelreihe als heller Streifen)
        cr.scale(w as f64 / doc.width.max(1e-9), h as f64 / doc.height.max(1e-9));
        draw_document(&cr, doc);
    }
    surface.flush();
    Ok(surface)
}

/// Dokument als PNG-Daten (z. B. für die Zwischenablage).
pub fn png_bytes(doc: &Document, scale: f64, bg: Option<Color>) -> Result<Vec<u8>, String> {
    let surface = raster(doc, scale, bg)?;
    let mut png: Vec<u8> = Vec::new();
    surface.write_to_png(&mut png).map_err(|e| e.to_string())?;
    Ok(png)
}

pub fn export(doc: &Document, path: &Path, opt: &Options) -> Result<(), String> {
    let bg = if opt.background { doc.background } else { None };
    match opt.format {
        Format::Png => {
            let surface = raster(doc, opt.scale, bg)?;
            let mut png: Vec<u8> = Vec::new();
            surface.write_to_png(&mut png).map_err(|e| e.to_string())?;
            crate::fsutil::write_atomic(path, &png).map_err(|e| e.to_string())
        }
        Format::Jpeg => {
            // JPG kennt keine Transparenz: fehlenden Hintergrund durch Weiß ersetzen.
            let mut bg = bg.unwrap_or(Color::WHITE);
            if bg.a < 1.0 {
                bg = Color::WHITE;
            }
            let mut surface = raster(doc, opt.scale, Some(bg))?;
            let (w, h) = (surface.width(), surface.height());
            let stride = surface.stride() as usize;
            let data = surface.data().map_err(|e| e.to_string())?;
            let mut rgb = Vec::with_capacity((w * h * 3) as usize);
            for y in 0..h as usize {
                let row = &data[y * stride..y * stride + w as usize * 4];
                for px in row.chunks_exact(4) {
                    // Cairo speichert ARGB32 als native 32-Bit-Werte (auf x86: B, G, R, A).
                    let v = u32::from_ne_bytes([px[0], px[1], px[2], px[3]]);
                    rgb.push(((v >> 16) & 0xff) as u8);
                    rgb.push(((v >> 8) & 0xff) as u8);
                    rgb.push((v & 0xff) as u8);
                }
            }
            drop(data);
            let pixbuf = gdk_pixbuf::Pixbuf::from_bytes(
                &glib::Bytes::from_owned(rgb),
                gdk_pixbuf::Colorspace::Rgb,
                false,
                8,
                w,
                h,
                w * 3,
            );
            let q = opt.quality.clamp(1, 100).to_string();
            pixbuf
                .savev(path, "jpeg", &[("quality", q.as_str())])
                .map_err(|e| e.to_string())
        }
        Format::Pdf => {
            let data = pdf_bytes(std::slice::from_ref(doc), opt, bg.is_some())?;
            crate::fsutil::write_atomic(path, &data).map_err(|e| e.to_string())
        }
        Format::Svg => {
            // für andere Programme: Effekte als Bilder, Textfelder als feste Zeilen
            let mut flat = flatten_effects(doc, opt.effect_dpi / 96.0);
            split_text_boxes(&mut flat);
            let font: Option<&dyn Fn(&str, bool, bool) -> Option<(Vec<u8>, &'static str)>> =
                if opt.embed_fonts { Some(&font_file) } else { None };
            let text =
                crate::svg::write_export(&flat, opt.background, font, crate::render::area_export_lines);
            crate::fsutil::write_atomic(path, text.as_bytes()).map_err(|e| e.to_string())
        }
    }
}

/// Mehrere Zeichenflächen als mehrseitiges PDF (eine Seite je Zeichenfläche).
pub fn export_pdf_pages(docs: &[Document], path: &Path, opt: &Options) -> Result<(), String> {
    let data = pdf_bytes(docs, opt, opt.background)?;
    crate::fsutil::write_atomic(path, &data).map_err(|e| e.to_string())
}
