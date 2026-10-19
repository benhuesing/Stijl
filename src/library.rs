//! Bibliotheken: gesammelte Farben, Stile und Textstile,
//! die sich mit einem Klick auf andere Objekte übertragen lassen.
//!
//! Jede Bibliothek wird als kleine SVG-Datei gespeichert (Ebene „Farben“ mit Farbfeldern,
//! Ebene „Stile“ mit Musterobjekten). So bleiben alle Stilangaben – Verläufe, Kontur,
//! Deckkraft, Mischmodus, Linienenden, Schrift – ohne eigenes Dateiformat erhalten.

use crate::geom::{Affine, Pt};
use crate::model::{
    Color, Document, Gradient, Layer, Marker, Node, Object, Shape, Style, SubPath, TextFormat,
};
use crate::svg;

/// Schrift eines Textstils.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub family: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub fmt: TextFormat,
}

/// Ein gespeicherter Stil: Darstellung und – bei Texten – die Schrift.
#[derive(Clone, Debug, PartialEq)]
pub struct LibStyle {
    pub style: Style,
    pub text: Option<TextStyle>,
    /// optionaler Name (z. B. „Überschrift“)
    pub name: String,
}

/// Gespeicherte Grafik (z. B. ein Logo): Objekte, die als Kopie eingefügt werden.
#[derive(Clone, Debug, PartialEq)]
pub struct LibGraphic {
    pub name: String,
    pub objects: Vec<Object>,
}

/// Ebenen-Name einer Grafik in der Bibliotheksdatei: „Grafik: Name“.
const GRAPHIC_PREFIX: &str = "Grafik:";

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Library {
    pub name: String,
    pub colors: Vec<Color>,
    /// optionale Namen der Farben (gleiche Reihenfolge; fehlende = ohne Namen)
    pub color_names: Vec<String>,
    /// Verläufe der Farben (gleiche Reihenfolge; fehlende = einfarbig)
    pub gradients: Vec<Option<Gradient>>,
    pub styles: Vec<LibStyle>,
    /// Grafiken (Objektgruppen)
    pub graphics: Vec<LibGraphic>,
    /// Mitgelieferte Standardbibliothek: nicht änderbar und wird nie gespeichert
    pub locked: bool,
}

/// Name der mitgelieferten Standardbibliothek.
pub const BUILTIN_NAME: &str = "Mondrian";

/// Standardbibliothek „Mondrian“: Grundfarben und drei Textstile.
pub fn builtin() -> Library {
    let mut lib = Library::new(BUILTIN_NAME);
    lib.locked = true;
    for hex in [
        "#000000", "#5e5c64", "#c0bfbc", "#ffffff", "#e01b24", "#ff7800", "#f6d32d", "#33d17a", "#3584e4",
        "#1a5fb4", "#9141ac", "#986a44",
    ] {
        if let Some(c) = Color::parse(hex) {
            lib.colors.push(c);
        }
    }
    let dark = Color::parse("#241f31").unwrap_or(Color::BLACK);
    let text = |name: &str, family: &str, size: f64, bold: bool, italic: bool, fill: Color| LibStyle {
        style: Style {
            fill: Some(fill),
            stroke: None,
            ..Style::default()
        },
        text: Some(TextStyle {
            family: family.into(),
            size,
            bold,
            italic,
            fmt: TextFormat::default(),
        }),
        name: name.into(),
    };
    lib.styles
        .push(text("Überschrift", "Sans", 48.0, true, false, dark));
    lib.styles.push(text("Text", "Sans", 16.0, false, false, dark));
    lib.styles.push(text(
        "Zitat",
        "Serif",
        22.0,
        false,
        true,
        Color::parse("#5e5c64").unwrap_or(dark),
    ));
    lib
}

const COLORS_LAYER: &str = "Farben";
const STYLES_LAYER: &str = "Stile";

fn same_color(a: Color, b: Color) -> bool {
    (a.r - b.r).abs() < 1e-3
        && (a.g - b.g).abs() < 1e-3
        && (a.b - b.b).abs() < 1e-3
        && (a.a - b.a).abs() < 1e-3
}

/// Stil eines Objekts für die Bibliothek.
pub fn style_of(obj: &Object) -> LibStyle {
    let text = obj
        .shape
        .text_fields()
        .map(|(_, family, size, bold, italic)| TextStyle {
            family: family.to_string(),
            size,
            bold,
            italic,
            fmt: obj.shape.text_format().unwrap_or_default(),
        });
    LibStyle {
        style: obj.style.clone(),
        text,
        name: String::new(),
    }
}

impl Library {
    pub fn new(name: &str) -> Library {
        Library {
            name: name.to_string(),
            ..Library::default()
        }
    }

    /// Grafik anhängen (gleiche Objekte werden nicht doppelt aufgenommen).
    pub fn add_graphic(&mut self, name: &str, objects: Vec<Object>) -> bool {
        if objects.is_empty() {
            return false;
        }
        let same = |a: &[Object], b: &[Object]| {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|(x, y)| x.shape == y.shape && x.style == y.style)
        };
        if self.graphics.iter().any(|g| same(&g.objects, &objects)) {
            return false;
        }
        let name = if name.trim().is_empty() {
            format!("Grafik {}", self.graphics.len() + 1)
        } else {
            name.trim().to_string()
        };
        self.graphics.push(LibGraphic { name, objects });
        true
    }

    /// Farbe anhängen (doppelte werden nicht erneut aufgenommen). Nur noch in den Tests
    /// genutzt; die Oberfläche nimmt Farben und Verläufe über `add_paint` auf.
    #[cfg(test)]
    pub fn add_color(&mut self, c: Color) -> bool {
        self.add_paint(c, None)
    }

    /// Farbe oder Verlauf anhängen (doppelte werden nicht erneut aufgenommen).
    pub fn add_paint(&mut self, c: Color, g: Option<Gradient>) -> bool {
        let dup = (0..self.colors.len()).any(|i| match (&g, self.gradient(i)) {
            (None, None) => same_color(self.colors[i], c),
            (Some(a), Some(b)) => a.same_colors(b),
            _ => false,
        });
        if dup {
            return false;
        }
        let n = self.colors.len();
        self.colors.push(c);
        self.color_names.resize(n, String::new());
        self.color_names.push(String::new());
        self.gradients.resize(n, None);
        self.gradients.push(g);
        true
    }

    /// Verlauf der Farbe `i` (None = einfarbig).
    pub fn gradient(&self, i: usize) -> Option<&Gradient> {
        self.gradients.get(i).and_then(|g| g.as_ref())
    }

    /// Name der Farbe `i` (leer, wenn unbenannt).
    pub fn color_name(&self, i: usize) -> &str {
        self.color_names.get(i).map(|s| s.as_str()).unwrap_or("")
    }

    pub fn set_color_name(&mut self, i: usize, name: &str) -> bool {
        if i >= self.colors.len() || self.color_name(i) == name {
            return false;
        }
        self.color_names.resize(self.colors.len(), String::new());
        self.color_names[i] = name.to_string();
        true
    }

    pub fn set_style_name(&mut self, i: usize, name: &str) -> bool {
        match self.styles.get_mut(i) {
            Some(s) if s.name != name => {
                s.name = name.to_string();
                true
            }
            _ => false,
        }
    }

    pub fn remove_color(&mut self, i: usize) -> bool {
        if i >= self.colors.len() {
            return false;
        }
        self.colors.remove(i);
        if i < self.color_names.len() {
            self.color_names.remove(i);
        }
        if i < self.gradients.len() {
            self.gradients.remove(i);
        }
        true
    }

    pub fn add_style(&mut self, s: LibStyle) -> bool {
        if self.styles.contains(&s) {
            return false;
        }
        self.styles.push(s);
        true
    }

    /// Als Dokument (zum Speichern als SVG).
    pub fn to_document(&self) -> Document {
        let mut doc = Document::new(400.0, 200.0, None);
        doc.layers[0].name = COLORS_LAYER.into();
        for (i, c) in self.colors.iter().enumerate() {
            let id = doc.alloc_id();
            doc.layers[0].objects.push(Object {
                group: 0,
                locked: false,
                name: self.color_name(i).to_string(),
                id,
                shape: Shape::Rect {
                    x: 4.0 + (i % 12) as f64 * 32.0,
                    y: 4.0 + (i / 12) as f64 * 32.0,
                    w: 28.0,
                    h: 28.0,
                    r: 0.0,
                },
                style: Style {
                    fill: Some(*c),
                    stroke: None,
                    gradient: self.gradient(i).cloned(),
                    ..Style::default()
                },
                angle: 0.0,
            });
        }
        let lid = doc.alloc_id();
        let mut layer = Layer {
            id: lid,
            name: STYLES_LAYER.into(),
            visible: true,
            locked: false,
            color: 1,
            objects: Vec::new(),
        };
        for (i, s) in self.styles.iter().enumerate() {
            let (x, y) = (4.0 + (i % 6) as f64 * 64.0, 100.0 + (i / 6) as f64 * 48.0);
            let shape = match &s.text {
                Some(t) => Shape::Text {
                    xf: Affine::translate(x, y + 30.0),
                    text: "Aa".into(),
                    family: t.family.clone(),
                    size: t.size,
                    bold: t.bold,
                    italic: t.italic,
                    fmt: t.fmt,
                },
                // Linienenden gibt es nur bei offenen Pfaden: dann eine Linie als Muster
                None if s.style.marker != Marker::None => Shape::Path {
                    subpaths: vec![SubPath {
                        nodes: vec![
                            Node::corner(Pt::new(x, y + 20.0)),
                            Node::corner(Pt::new(x + 56.0, y + 20.0)),
                        ],
                        closed: false,
                    }],
                    evenodd: false,
                },
                None => Shape::Rect {
                    x,
                    y,
                    w: 56.0,
                    h: 40.0,
                    r: 0.0,
                },
            };
            let id = doc.alloc_id();
            layer.objects.push(Object {
                group: 0,
                locked: false,
                name: s.name.clone(),
                id,
                shape,
                style: s.style.clone(),
                angle: 0.0,
            });
        }
        doc.layers.push(layer);
        // je Grafik eine Ebene „Grafik: Name“
        for g in &self.graphics {
            let lid = doc.alloc_id();
            let mut gl = Layer {
                id: lid,
                name: format!("{GRAPHIC_PREFIX} {}", g.name),
                visible: true,
                locked: false,
                color: 2,
                objects: Vec::new(),
            };
            for o in &g.objects {
                let mut c = o.clone();
                c.id = doc.alloc_id();
                gl.objects.push(c);
            }
            doc.layers.push(gl);
        }
        doc
    }

    pub fn from_document(name: &str, doc: &Document) -> Library {
        let mut lib = Library::new(name);
        for layer in &doc.layers {
            if layer.name == COLORS_LAYER {
                for o in &layer.objects {
                    if let Some(c) = o.style.fill {
                        lib.colors.push(c);
                        lib.color_names.push(o.name.clone());
                        lib.gradients.push(o.style.gradient.clone());
                    }
                }
            } else if let Some(n) = layer.name.strip_prefix(GRAPHIC_PREFIX) {
                if !layer.objects.is_empty() {
                    lib.graphics.push(LibGraphic {
                        name: n.trim().to_string(),
                        objects: layer.objects.clone(),
                    });
                }
            } else if layer.name == STYLES_LAYER {
                for o in &layer.objects {
                    let mut st = style_of(o);
                    st.name = o.name.clone();
                    lib.styles.push(st);
                }
            }
        }
        lib
    }

    pub fn to_svg(&self) -> String {
        svg::write(&self.to_document(), true)
    }

    pub fn from_svg(name: &str, text: &str) -> Result<Library, String> {
        svg::read(text).map(|d| Library::from_document(name, &d))
    }
}

/// Dateiname für eine Bibliothek (ohne Sonderzeichen, die Dateisysteme stören).
pub fn file_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let clean = clean.trim();
    format!("{}.svg", if clean.is_empty() { "Bibliothek" } else { clean })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Blend, Gradient, MarkerEnds};

    #[test]
    fn roundtrip() {
        let mut lib = Library::new("Marke");
        assert!(lib.add_color(Color::rgb(1.0, 0.0, 0.0)));
        assert!(lib.set_color_name(0, "Signalrot"));
        let logo = |x: f64, g: u64| Object {
            id: 1,
            group: g,
            locked: false,
            name: String::new(),
            shape: Shape::Rect {
                x,
                y: 0.0,
                w: 10.0,
                h: 10.0,
                r: 0.0,
            },
            style: Style::default(),
            angle: 0.0,
        };
        assert!(lib.add_graphic("Logo", vec![logo(0.0, 7), logo(20.0, 7)]));
        assert!(!lib.add_graphic("Doppelt", vec![logo(0.0, 7), logo(20.0, 7)]));
        let mut gr = Gradient::new(Color::rgb(1.0, 1.0, 0.0), Color::rgb(0.0, 0.5, 1.0));
        gr.add_stop(0.5);
        assert!(lib.add_paint(gr.stops[0].color, Some(gr.clone())));
        assert!(!lib.add_paint(gr.stops[0].color, Some(gr.clone())));
        assert!(!lib.add_color(Color::rgb(1.0, 0.0, 0.0)));
        lib.add_color(Color {
            r: 0.2,
            g: 0.4,
            b: 0.6,
            a: 0.5,
        });
        let mut st = Style {
            fill: Some(Color::rgb(0.0, 0.5, 0.0)),
            stroke: Some(Color::rgb(0.1, 0.1, 0.1)),
            width: 4.0,
            opacity: 0.7,
            fx: crate::model::Effects {
                tex: Some((crate::model::Texture::Frost, 1.0)),
                ..Default::default()
            },
            join: crate::model::LineJoin::Round,
            cap: crate::model::LineCap::Square,
            ..Style::default()
        };
        lib.add_style(LibStyle {
            style: st.clone(),
            text: None,
            name: String::new(),
        });
        st.gradient = Some(Gradient::new(
            Color::rgb(1.0, 1.0, 0.0),
            Color::rgb(0.0, 0.0, 1.0),
        ));
        st.blend = Blend::Multiply;
        lib.add_style(LibStyle {
            style: st.clone(),
            text: None,
            name: String::new(),
        });
        let line = Style {
            fill: None,
            marker: Marker::Arrow,
            marker_ends: MarkerEnds::End,
            ..Style::default()
        };
        lib.add_style(LibStyle {
            style: line,
            text: None,
            name: String::new(),
        });
        lib.add_style(LibStyle {
            style: Style::default(),
            text: Some(TextStyle {
                family: "Serif".into(),
                size: 31.0,
                bold: true,
                italic: false,
                fmt: TextFormat::default(),
            }),
            name: "Zitat".into(),
        });
        let back = Library::from_svg("Marke", &lib.to_svg()).unwrap();
        assert!((back.colors[2].a - 0.5).abs() < 1e-2);
        assert_eq!(back.styles.len(), 4);
        let s0 = &back.styles[0].style;
        assert_eq!(s0.fx.frost(), Some(1.0));
        assert_eq!(
            (s0.join, s0.cap),
            (crate::model::LineJoin::Round, crate::model::LineCap::Square)
        );
        // Deckkraft steckt nach dem Laden in den Farben
        assert!((s0.width - 4.0).abs() < 1e-9 && (s0.opacity - 1.0).abs() < 1e-6);
        assert!((s0.fill.unwrap().a - 0.7).abs() < 1e-3);
        assert!(back.styles[1].style.gradient.is_some());
        assert_eq!(back.styles[2].style.marker, Marker::Arrow);
        let t = back.styles[3].text.as_ref().unwrap();
        assert_eq!(t.family, "Serif");
        assert_eq!(back.styles[3].name, "Zitat");
        assert_eq!(back.color_name(0), "Signalrot");
        assert_eq!(back.graphics.len(), 1);
        assert_eq!(back.graphics[0].name, "Logo");
        assert_eq!(back.graphics[0].objects.len(), 2);
        assert!(back.graphics[0].objects[0].group != 0);
        assert_eq!(back.colors.len(), 3);
        assert!(back.gradient(1).map(|g| g.same_colors(&gr)).unwrap_or(false));
        assert!(back.gradient(0).is_none());
        let b = builtin();
        assert!(b.locked && b.colors.len() == 12 && b.styles.len() == 3);
        assert!(t.bold && (t.size - 31.0).abs() < 1e-9);
        assert_eq!(file_name("Mein/Kit"), "Mein_Kit.svg");
    }
}
