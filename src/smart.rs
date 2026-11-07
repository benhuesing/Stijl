// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © Bendix Hüsing

//! Tabellen und Diagramme: Objekte, die aus Daten entstehen.
//!
//! Eine Tabelle oder ein Diagramm ist in der Zeichnung eine gewöhnliche Gruppe aus Formen
//! (Flächen, Linien, Textfeldern) – so zeichnen, exportieren und drucken sie sich wie alles
//! andere. Das erste Objekt der Gruppe, ein unsichtbarer (oder gefüllter) Rahmen, trägt die
//! Daten ([`Smart`]) und bestimmt die Größe; ändern sich die Daten, wird der Rest der
//! Gruppe aus ihnen neu gebaut ([`build`]).

use crate::geom::{Affine, Pt, Rect};
use crate::i18n::tr;
use crate::model::{
    Align, Color, LineCap, LineJoin, Node, Shape, Style, SubPath, TextFormat, TextFrame, VAlign,
};
use crate::sheet::{evaluate, format_number, Value};

/// Daten eines Objekts, das aus ihnen gebaut wird.
#[derive(Clone, Debug, PartialEq)]
pub enum Smart {
    Table(Table),
    Chart(Chart),
}

/// Tabelle: Zellen (Text, Zahlen, Formeln) in Zeilen und Spalten.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    /// Eingaben, zeilenweise (alle Zeilen gleich lang)
    pub cells: Vec<Vec<String>>,
    /// relative Breiten der Spalten und Höhen der Zeilen
    pub cols: Vec<f64>,
    pub rows: Vec<f64>,
    /// erste Zeile als Kopfzeile (fett, eigene Füllung)
    pub header: bool,
    /// Ausrichtung je Spalte (`None`: Zahlen rechts, Text links)
    pub align: Vec<Option<Align>>,
    pub family: String,
    pub size: f64,
    pub text: Color,
    pub header_text: Color,
    pub header_fill: Option<Color>,
    /// Hintergrund der Zellen
    pub fill: Option<Color>,
    pub border: Option<Color>,
    pub border_width: f64,
    /// Abweichendes Aussehen einzelner Zellen (zeilenweise, so groß wie `cells`)
    pub fmt: Vec<Vec<Fmt>>,
    /// verbundene Zellen (oben, links, unten, rechts – jeweils einschließlich); es gilt
    /// der Inhalt der Zelle oben links
    pub merges: Vec<(usize, usize, usize, usize)>,
    /// Kennung, über die Diagramme ihre Daten aus dieser Tabelle holen (0 = keine)
    pub key: u64,
}

/// Wie eine Zahl in einer Zelle erscheint.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum NumFmt {
    /// wie eingegeben bzw. errechnet
    #[default]
    General,
    /// feste Nachkommastellen, auf Wunsch mit Tausendertrennzeichen
    Number {
        decimals: u8,
        group: bool,
    },
    Percent {
        decimals: u8,
    },
    Currency {
        symbol: String,
        decimals: u8,
    },
    Scientific {
        decimals: u8,
    },
    Date(DateStyle),
    Time {
        seconds: bool,
    },
    DateTime,
    /// die Eingabe selbst (auch Formeln als Text)
    Text,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateStyle {
    Short,
    Long,
    Iso,
}

impl NumFmt {
    fn encode(&self) -> String {
        match self {
            NumFmt::General => "g".into(),
            NumFmt::Number { decimals, group } => format!("n{decimals}{}", if *group { "g" } else { "" }),
            NumFmt::Percent { decimals } => format!("p{decimals}"),
            NumFmt::Currency { symbol, decimals } => format!("c{decimals}/{}", enc(symbol)),
            NumFmt::Scientific { decimals } => format!("e{decimals}"),
            NumFmt::Date(DateStyle::Short) => "ds".into(),
            NumFmt::Date(DateStyle::Long) => "dl".into(),
            NumFmt::Date(DateStyle::Iso) => "di".into(),
            NumFmt::Time { seconds } => if *seconds { "ts" } else { "t" }.into(),
            NumFmt::DateTime => "dt".into(),
            NumFmt::Text => "x".into(),
        }
    }

    fn decode(s: &str) -> Option<NumFmt> {
        let digits = |t: &str| {
            t.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<u8>()
                .ok()
                .map(|d| d.min(10))
        };
        Some(match s {
            "g" => NumFmt::General,
            "ds" => NumFmt::Date(DateStyle::Short),
            "dl" => NumFmt::Date(DateStyle::Long),
            "di" => NumFmt::Date(DateStyle::Iso),
            "t" => NumFmt::Time { seconds: false },
            "ts" => NumFmt::Time { seconds: true },
            "dt" => NumFmt::DateTime,
            "x" => NumFmt::Text,
            _ => {
                let (k, rest) = s.split_at(1);
                match k {
                    "n" => NumFmt::Number {
                        decimals: digits(rest)?,
                        group: rest.ends_with('g'),
                    },
                    "p" => NumFmt::Percent {
                        decimals: digits(rest)?,
                    },
                    "e" => NumFmt::Scientific {
                        decimals: digits(rest)?,
                    },
                    "c" => {
                        let (d, sym) = rest.split_once('/')?;
                        NumFmt::Currency {
                            symbol: dec(sym),
                            decimals: digits(d)?,
                        }
                    }
                    _ => return None,
                }
            }
        })
    }

    /// Nachkommastellen (bei Formaten, die welche haben).
    pub fn decimals(&self) -> Option<u8> {
        match self {
            NumFmt::Number { decimals, .. }
            | NumFmt::Percent { decimals }
            | NumFmt::Currency { decimals, .. }
            | NumFmt::Scientific { decimals } => Some(*decimals),
            _ => None,
        }
    }

    pub fn with_decimals(&self, d: u8) -> NumFmt {
        let mut f = self.clone();
        match &mut f {
            NumFmt::Number { decimals, .. }
            | NumFmt::Percent { decimals }
            | NumFmt::Currency { decimals, .. }
            | NumFmt::Scientific { decimals } => *decimals = d,
            _ => {}
        }
        f
    }
}

/// Zahl mit festen Nachkommastellen (`group`: Tausendertrennzeichen).
fn fixed(x: f64, decimals: u8, group: bool, comma: bool) -> String {
    let s = format!("{:.*}", decimals as usize, x.abs());
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    let mut out = String::new();
    if x < 0.0 && s.chars().any(|c| c.is_ascii_digit() && c != '0') {
        out.push('-');
    }
    for (i, ch) in int.chars().enumerate() {
        if group && i > 0 && (int.len() - i) % 3 == 0 {
            out.push(if comma { '.' } else { ',' });
        }
        out.push(ch);
    }
    if !frac.is_empty() {
        out.push(if comma { ',' } else { '.' });
        out.push_str(frac);
    }
    out
}

/// Monatsname in der Sprache der Oberfläche.
fn month_name(m: u32) -> &'static str {
    use crate::i18n::Lang;
    const EN: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    const DE: [&str; 12] = [
        "Januar",
        "Februar",
        "März",
        "April",
        "Mai",
        "Juni",
        "Juli",
        "August",
        "September",
        "Oktober",
        "November",
        "Dezember",
    ];
    const ES: [&str; 12] = [
        "enero",
        "febrero",
        "marzo",
        "abril",
        "mayo",
        "junio",
        "julio",
        "agosto",
        "septiembre",
        "octubre",
        "noviembre",
        "diciembre",
    ];
    const FR: [&str; 12] = [
        "janvier",
        "février",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "août",
        "septembre",
        "octobre",
        "novembre",
        "décembre",
    ];
    let i = (m.clamp(1, 12) - 1) as usize;
    match crate::i18n::lang() {
        Lang::De => DE[i],
        Lang::Es => ES[i],
        Lang::Fr => FR[i],
        _ => EN[i],
    }
}

/// Wert einer Zelle in ihrem Zahlenformat (`input`: die Eingabe, für „Text“).
pub fn format_value(v: &Value, f: &NumFmt, input: &str, comma: bool) -> String {
    use crate::i18n::Lang;
    if *f == NumFmt::Text {
        return input.to_string();
    }
    let x = match v {
        Value::Num(n) | Value::Date(n) | Value::Time(n) => *n,
        _ => return v.display(comma),
    };
    let hm = |x: f64, secs: bool| {
        let t = (x.rem_euclid(1.0) * 86_400.0).round() as i64 % 86_400;
        if secs {
            format!("{:02}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
        } else {
            format!("{:02}:{:02}", t / 3600, t / 60 % 60)
        }
    };
    let lang = crate::i18n::lang();
    match f {
        NumFmt::General | NumFmt::Text => v.display(comma),
        NumFmt::Number { decimals, group } => fixed(x, *decimals, *group, comma),
        NumFmt::Percent { decimals } => {
            let n = fixed(x * 100.0, *decimals, false, comma);
            if lang == Lang::En {
                format!("{n}%")
            } else {
                format!("{n} %")
            }
        }
        NumFmt::Currency { symbol, decimals } => {
            let n = fixed(x.abs(), *decimals, true, comma);
            let minus = if x < 0.0 { "-" } else { "" };
            if comma {
                format!("{minus}{n} {symbol}")
            } else {
                format!("{minus}{symbol}{n}")
            }
        }
        NumFmt::Scientific { decimals } => {
            let s = format!("{:.*e}", *decimals as usize, x);
            let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
            let e: i32 = e.parse().unwrap_or(0);
            let m = if comma { m.replace('.', ",") } else { m.to_string() };
            format!("{m}E{}{:02}", if e < 0 { '-' } else { '+' }, e.abs())
        }
        NumFmt::Date(style) => {
            let (y, m, d) = crate::sheet::date_parts(x);
            match style {
                DateStyle::Iso => format!("{y}-{m:02}-{d:02}"),
                DateStyle::Short => {
                    if lang == Lang::De {
                        format!("{d:02}.{m:02}.{y}")
                    } else {
                        format!("{d:02}/{m:02}/{y}")
                    }
                }
                DateStyle::Long => match lang {
                    Lang::De => format!("{d}. {} {y}", month_name(m)),
                    Lang::Es => format!("{d} de {} de {y}", month_name(m)),
                    _ => format!("{d} {} {y}", month_name(m)),
                },
            }
        }
        NumFmt::Time { seconds } => hm(x, *seconds),
        NumFmt::DateTime => format!(
            "{} {}",
            format_value(v, &NumFmt::Date(DateStyle::Short), input, comma),
            hm(x, false)
        ),
    }
}

/// Abweichendes Aussehen einer Zelle bzw. eines Diagrammteils: nur gesetzte Felder gelten
/// (`fill: Some(None)` heißt ausdrücklich „keine Füllung“).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Fmt {
    pub fill: Option<Option<Color>>,
    pub stroke: Option<Option<Color>>,
    pub width: Option<f64>,
    pub family: Option<String>,
    pub size: Option<f64>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    /// Textfarbe
    pub color: Option<Color>,
    pub align: Option<Align>,
    pub valign: Option<VAlign>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    /// Strichstärke 1–1000 (0: aus „fett“)
    pub weight: Option<u16>,
    pub small_caps: Option<bool>,
    pub no_ligatures: Option<bool>,
    /// Zeilenabstand (Vielfaches der Schriftgröße) und Zeichenabstand (1/1000 em)
    pub line: Option<f64>,
    pub tracking: Option<f64>,
    /// Text senkrecht: 1 von unten nach oben, 2 von oben nach unten (0 waagerecht)
    pub vertical: Option<u8>,
    /// Zahlenformat
    pub number: Option<NumFmt>,
}

/// Wirksames Aussehen einer Zelle bzw. eines Diagrammteils.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub fill: Option<Color>,
    pub stroke: Option<Color>,
    pub width: f64,
    pub family: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub color: Color,
    pub align: Align,
    pub valign: VAlign,
    pub underline: bool,
    pub strike: bool,
    pub weight: u16,
    pub small_caps: bool,
    pub no_ligatures: bool,
    pub line: f64,
    pub tracking: f64,
    pub vertical: u8,
    pub number: NumFmt,
}

impl Look {
    fn text(family: &str, size: f64, color: Color) -> Look {
        Look {
            fill: None,
            stroke: None,
            width: 0.0,
            family: family.to_string(),
            size,
            bold: false,
            italic: false,
            color,
            align: Align::Left,
            valign: VAlign::Middle,
            underline: false,
            strike: false,
            weight: 0,
            small_caps: false,
            no_ligatures: false,
            line: 1.2,
            tracking: 0.0,
            vertical: 0,
            number: NumFmt::General,
        }
    }

    pub fn apply(&mut self, f: &Fmt) {
        if let Some(v) = f.fill {
            self.fill = v;
        }
        if let Some(v) = f.stroke {
            self.stroke = v;
        }
        if let Some(v) = f.width {
            self.width = v;
        }
        if let Some(v) = &f.family {
            self.family = v.clone();
        }
        if let Some(v) = f.size {
            self.size = v;
        }
        if let Some(v) = f.bold {
            self.bold = v;
        }
        if let Some(v) = f.italic {
            self.italic = v;
        }
        if let Some(v) = f.color {
            self.color = v;
        }
        if let Some(v) = f.align {
            self.align = v;
        }
        if let Some(v) = f.valign {
            self.valign = v;
        }
        if let Some(v) = f.underline {
            self.underline = v;
        }
        if let Some(v) = f.strike {
            self.strike = v;
        }
        if let Some(v) = f.weight {
            self.weight = v;
        }
        if let Some(v) = f.small_caps {
            self.small_caps = v;
        }
        if let Some(v) = f.no_ligatures {
            self.no_ligatures = v;
        }
        if let Some(v) = f.line {
            self.line = v;
        }
        if let Some(v) = f.tracking {
            self.tracking = v;
        }
        if let Some(v) = f.vertical {
            self.vertical = v;
        }
        if let Some(v) = &f.number {
            self.number = v.clone();
        }
    }

    /// Was sich gegenüber `before` geändert hat, in `f` festhalten.
    pub fn diff_into(&self, before: &Look, f: &mut Fmt) {
        if self.fill != before.fill {
            f.fill = Some(self.fill);
        }
        if self.stroke != before.stroke {
            f.stroke = Some(self.stroke);
        }
        if self.width != before.width {
            f.width = Some(self.width);
        }
        if self.family != before.family {
            f.family = Some(self.family.clone());
        }
        if self.size != before.size {
            f.size = Some(self.size);
        }
        if self.bold != before.bold {
            f.bold = Some(self.bold);
        }
        if self.italic != before.italic {
            f.italic = Some(self.italic);
        }
        if self.color != before.color {
            f.color = Some(self.color);
        }
        if self.align != before.align {
            f.align = Some(self.align);
        }
        if self.valign != before.valign {
            f.valign = Some(self.valign);
        }
        if self.underline != before.underline {
            f.underline = Some(self.underline);
        }
        if self.strike != before.strike {
            f.strike = Some(self.strike);
        }
        if self.weight != before.weight {
            f.weight = Some(self.weight);
        }
        if self.small_caps != before.small_caps {
            f.small_caps = Some(self.small_caps);
        }
        if self.no_ligatures != before.no_ligatures {
            f.no_ligatures = Some(self.no_ligatures);
        }
        if self.line != before.line {
            f.line = Some(self.line);
        }
        if self.tracking != before.tracking {
            f.tracking = Some(self.tracking);
        }
        if self.vertical != before.vertical {
            f.vertical = Some(self.vertical);
        }
        if self.number != before.number {
            f.number = Some(self.number.clone());
        }
    }

    /// Absatz- und Zeichenformat für ein Textfeld.
    pub fn text_format(&self) -> TextFormat {
        TextFormat {
            align: self.align,
            valign: self.valign,
            para: 0.0,
            line: self.line,
            tracking: self.tracking,
            underline: self.underline,
            strike: self.strike,
            weight: self.weight,
            small_caps: self.small_caps,
            no_ligatures: self.no_ligatures,
            ..TextFormat::default()
        }
    }

    /// Fläche und Kontur als Stil (für die Eigenschaftenleiste).
    pub fn style(&self) -> Style {
        Style {
            fill: self.fill,
            stroke: self.stroke,
            width: self.width,
            ..Style::default()
        }
    }
}

/// Was ein gebautes Teil darstellt (zum Auswählen einzelner Zellen und Datenpunkte).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Linien, Achsen …
    Other,
    /// Kasten bzw. Text einer Zelle (Zeile, Spalte)
    Cell(usize, usize),
    CellText(usize, usize),
    /// Datenpunkt (Reihe, Nummer): Kreisstück, Balken, Punkt
    Point(usize, usize),
    /// ganze Reihe: Linie bzw. Farbfeld der Legende
    Series(usize),
    /// ein Text eines Diagramms
    Text(TextKey),
}

/// Ein Text eines Diagramms (lässt sich einzeln gestalten).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TextKey {
    /// Eintrag der Legende
    Legend(usize),
    /// Zahl an der Werteachse (von unten gezählt)
    Tick(usize),
    /// Beschriftung unter der Achse
    Category(usize),
    /// Wert an einem Datenpunkt (Reihe, Nummer)
    Value(usize, usize),
    /// Titel der x- bzw. y-Achse
    XTitle,
    YTitle,
}

impl TextKey {
    fn encode(self) -> String {
        match self {
            TextKey::Legend(i) => format!("l{i}"),
            TextKey::Tick(i) => format!("t{i}"),
            TextKey::Category(i) => format!("c{i}"),
            TextKey::Value(a, b) => format!("v{a}.{b}"),
            TextKey::XTitle => "x".into(),
            TextKey::YTitle => "y".into(),
        }
    }

    fn decode(s: &str) -> Option<TextKey> {
        let (k, rest) = s.split_at(s.char_indices().nth(1).map(|x| x.0).unwrap_or(s.len()));
        match k {
            "l" => rest.parse().ok().map(TextKey::Legend),
            "t" => rest.parse().ok().map(TextKey::Tick),
            "c" => rest.parse().ok().map(TextKey::Category),
            "x" if rest.is_empty() => Some(TextKey::XTitle),
            "y" if rest.is_empty() => Some(TextKey::YTitle),
            "v" => {
                let (a, b) = rest.split_once('.')?;
                Some(TextKey::Value(a.parse().ok()?, b.parse().ok()?))
            }
            _ => None,
        }
    }
}

/// Art des Diagramms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ChartKind {
    #[default]
    Pie,
    Donut,
    Bar,
    Line,
}

impl ChartKind {
    pub const ALL: [ChartKind; 4] = [ChartKind::Pie, ChartKind::Donut, ChartKind::Bar, ChartKind::Line];

    pub fn name(self) -> &'static str {
        match self {
            ChartKind::Pie => "pie",
            ChartKind::Donut => "donut",
            ChartKind::Bar => "bar",
            ChartKind::Line => "line",
        }
    }

    pub fn from_name(s: &str) -> Option<ChartKind> {
        Self::ALL.into_iter().find(|k| k.name() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            ChartKind::Pie => tr("Pie Chart"),
            ChartKind::Donut => tr("Donut Chart"),
            ChartKind::Bar => tr("Bar Chart"),
            ChartKind::Line => tr("Line Chart"),
        }
    }
}

/// Diagramm: Daten wie eine kleine Tabelle – erste Spalte die Beschriftungen, darin die
/// Werte (auch Formeln); mit Kopfzeile nennt die erste Zeile die Reihen.
#[derive(Clone, Debug, PartialEq)]
pub struct Chart {
    pub kind: ChartKind,
    pub data: Vec<Vec<String>>,
    /// erste Zeile: Namen der Reihen (sonst sind alle Zeilen Daten)
    pub header: bool,
    /// Farben der Reihen (Balken, Linien) bzw. der Stücke (Kreis)
    pub colors: Vec<Color>,
    pub family: String,
    pub size: f64,
    pub text: Color,
    pub legend: bool,
    /// Werte (Kreis: Anteile in Prozent) an die Stücke, Balken und Punkte schreiben
    pub values: bool,
    /// Tabelle, aus der die Daten kommen (ihre `key`; 0 = eigene Daten). Die Daten werden
    /// auch hier mitgeführt: fehlt die Tabelle, bleibt das Diagramm, wie es war.
    pub source: u64,
    /// Aussehen der Reihen (Kreis: der Stücke) und einzelner Datenpunkte (Reihe, Nummer)
    pub series_fmt: Vec<Fmt>,
    pub points: Vec<(usize, usize, Fmt)>,
    /// Schrift der Beschriftungen (Legende, Achsen) und der Werte
    pub labels_fmt: Fmt,
    pub values_fmt: Fmt,
    /// Abweichungen einzelner Texte
    pub texts: Vec<(TextKey, Fmt)>,
    /// Reihen in Zeilen (`Some(true)`) bzw. Spalten (`Some(false)`); `None`: selbst erkennen
    pub orient: Option<bool>,
    /// Titel der Achsen (leer: keiner; der x-Achse sonst aus der Ecke oben links)
    pub x_title: String,
    pub y_title: String,
}

/// Wie eine Tabelle aufgebaut ist: Kopfzeile (Namen über den Spalten), Beschriftungsspalte
/// (Namen vor den Zeilen) und ob die Reihen in Zeilen liegen; dazu die genutzte Größe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataLayout {
    pub head: bool,
    pub labels: bool,
    pub by_rows: bool,
    pub rows: usize,
    pub cols: usize,
}

/// Aufbau einer Tabelle erkennen (wie Tabellenkalkulationen): Ist die Ecke oben links leer,
/// sind erste Zeile und erste Spalte Beschriftungen; sonst je, wenn darin Text steht (oder
/// Jahreszahlen). Die Reihen liegen in Zeilen, wenn die Daten breiter als hoch sind – sonst
/// in Spalten. `force` legt das fest.
pub fn data_layout(vals: &[Vec<Value>], force: Option<bool>) -> DataLayout {
    let filled = |v: &Value| match v {
        Value::Empty => false,
        Value::Text(t) => !t.trim().is_empty(),
        _ => true,
    };
    let rows = vals
        .iter()
        .rposition(|r| r.iter().any(filled))
        .map_or(0, |i| i + 1);
    let cols = vals
        .iter()
        .take(rows)
        .map(|r| r.iter().rposition(filled).map_or(0, |i| i + 1))
        .max()
        .unwrap_or(0);
    let at = |r: usize, c: usize| vals.get(r).and_then(|row| row.get(c)).unwrap_or(&Value::Empty);
    let texty = |v: &Value| matches!(v, Value::Text(t) if !t.trim().is_empty());
    let year = |v: &Value| matches!(v, Value::Num(n) if n.fract() == 0.0 && (1000.0..3000.0).contains(n));
    let corner_empty = !filled(at(0, 0));
    let row_text = (1..cols).any(|c| texty(at(0, c)));
    let col_text = (1..rows).any(|r| texty(at(r, 0)));
    let row_years = cols > 1 && (1..cols).all(|c| year(at(0, c)) || !filled(at(0, c)));
    let col_years = rows > 1 && (1..rows).all(|r| year(at(r, 0)) || !filled(at(r, 0)));
    let mut head = rows > 1 && (row_text || (corner_empty && cols > 1) || (row_years && !col_years));
    let mut labels = cols > 1 && (col_text || (corner_empty && rows > 1) || (col_years && !row_years));
    // Text in der Ecke: sie gehört zu beidem, wenn eins davon erkannt ist
    if texty(at(0, 0)) && (head || labels) && rows > 1 && cols > 1 {
        head |= !(1..cols).any(|c| matches!(at(0, c), Value::Num(_))) || row_years;
        labels |= !(1..rows).any(|r| matches!(at(r, 0), Value::Num(_))) || col_years;
    }
    let data_r = rows - usize::from(head);
    let data_c = cols - usize::from(labels);
    DataLayout {
        head,
        labels,
        by_rows: force.unwrap_or(data_c > data_r),
        rows,
        cols,
    }
}

/// Farben für Reihen und Stücke (GNOME-Akzentfarben).
pub const PALETTE: [Color; 9] = [
    Color::rgb(0.20784313725490197, 0.5176470588235295, 0.8941176470588236),
    Color::rgb(0.9019607843137255, 0.17647058823529413, 0.25882352941176473),
    Color::rgb(0.22745098039215686, 0.5803921568627451, 0.2901960784313726),
    Color::rgb(0.7843137254901961, 0.5333333333333333, 0.0),
    Color::rgb(0.5686274509803921, 0.2549019607843137, 0.6745098039215687),
    Color::rgb(0.12941176470588237, 0.5647058823529412, 0.6431372549019608),
    Color::rgb(0.9294117647058824, 0.3568627450980392, 0.0),
    Color::rgb(0.8352941176470589, 0.3803921568627451, 0.6),
    Color::rgb(0.43529411764705883, 0.5137254901960784, 0.5882352941176471),
];

impl Table {
    /// Neue Tabelle mit Kopfzeile und Beispielinhalt.
    #[allow(dead_code)]
    pub fn sample() -> Table {
        let cells = vec![
            vec![tr("Item").to_string(), tr("Amount").to_string()],
            vec!["A".into(), "120".into()],
            vec!["B".into(), "80".into()],
            vec!["C".into(), "45".into()],
            vec![tr("Total").to_string(), "=SUM(B2:B4)".into()],
        ];
        Table::with_cells(cells)
    }

    pub fn with_cells(cells: Vec<Vec<String>>) -> Table {
        let mut t = Table {
            cells,
            cols: Vec::new(),
            rows: Vec::new(),
            // alle Zellen sehen gleich aus; Hervorhebungen setzt man je Zelle
            header: false,
            align: Vec::new(),
            family: "Sans".into(),
            size: 12.0,
            text: Color::rgb(0.12941176470588237, 0.12941176470588237, 0.12941176470588237),
            header_text: Color::WHITE,
            header_fill: Some(PALETTE[0]),
            fill: Some(Color::WHITE),
            border: Some(Color::rgb(0.8, 0.8196078431372549, 0.8509803921568627)),
            border_width: 1.0,
            fmt: Vec::new(),
            merges: Vec::new(),
            key: 0,
        };
        t.normalize();
        t
    }

    /// Leere Tabelle mit Kopfzeile (für das Datenwerkzeug).
    pub fn empty(rows: usize, cols: usize) -> Table {
        Table::with_cells(vec![vec![String::new(); cols.max(1)]; rows.max(1)])
    }

    pub fn n_rows(&self) -> usize {
        self.cells.len()
    }

    pub fn n_cols(&self) -> usize {
        self.cells.iter().map(|r| r.len()).max().unwrap_or(0)
    }

    /// Zeilen gleich lang machen, Breiten/Höhen und Ausrichtungen ergänzen.
    pub fn normalize(&mut self) {
        if self.cells.is_empty() {
            self.cells.push(vec![String::new()]);
        }
        let n = self.n_cols().max(1);
        for r in &mut self.cells {
            r.resize(n, String::new());
        }
        self.cols.resize(n, 1.0);
        self.rows.resize(self.cells.len(), 1.0);
        self.align.resize(n, None);
        self.fmt.resize(self.cells.len(), Vec::new());
        for r in &mut self.fmt {
            r.resize(n, Fmt::default());
        }
        for w in self.cols.iter_mut().chain(self.rows.iter_mut()) {
            if !w.is_finite() || *w <= 0.0 {
                *w = 1.0;
            }
        }
        // Verbindungen: in der Tabelle, mehr als eine Zelle, ohne Überschneidung
        let (nr, nc) = (self.cells.len(), n);
        let mut kept: Vec<(usize, usize, usize, usize)> = Vec::new();
        for m in std::mem::take(&mut self.merges) {
            let (r0, c0, r1, c1) = (
                m.0.min(m.2),
                m.1.min(m.3),
                m.0.max(m.2).min(nr - 1),
                m.1.max(m.3).min(nc - 1),
            );
            if r0 > r1 || c0 > c1 || (r0 == r1 && c0 == c1) || r0 >= nr || c0 >= nc {
                continue;
            }
            if kept
                .iter()
                .any(|k| r0 <= k.2 && k.0 <= r1 && c0 <= k.3 && k.1 <= c1)
            {
                continue;
            }
            kept.push((r0, c0, r1, c1));
        }
        self.merges = kept;
    }

    /// Verbindung, zu der eine Zelle gehört.
    pub fn merge_at(&self, r: usize, c: usize) -> Option<(usize, usize, usize, usize)> {
        self.merges
            .iter()
            .copied()
            .find(|m| (m.0..=m.2).contains(&r) && (m.1..=m.3).contains(&c))
    }

    /// Bereich so erweitern, dass er verbundene Zellen ganz enthält.
    pub fn expand(&self, range: (usize, usize, usize, usize)) -> (usize, usize, usize, usize) {
        let mut g = range;
        loop {
            let before = g;
            for m in &self.merges {
                if m.0 <= g.2 && g.0 <= m.2 && m.1 <= g.3 && g.1 <= m.3 {
                    g = (g.0.min(m.0), g.1.min(m.1), g.2.max(m.2), g.3.max(m.3));
                }
            }
            if g == before {
                return g;
            }
        }
    }

    /// Zellen verbinden (der Inhalt oben links bleibt, die übrigen werden geleert).
    pub fn merge(&mut self, range: (usize, usize, usize, usize)) -> bool {
        let g = self.expand(range);
        if g.0 == g.2 && g.1 == g.3 {
            return false;
        }
        self.merges
            .retain(|m| !(m.0 <= g.2 && g.0 <= m.2 && m.1 <= g.3 && g.1 <= m.3));
        for r in g.0..=g.2 {
            for c in g.1..=g.3 {
                if (r, c) != (g.0, g.1) {
                    self.cells[r][c].clear();
                }
            }
        }
        self.merges.push(g);
        true
    }

    /// Verbindungen in einem Bereich lösen.
    pub fn split(&mut self, range: (usize, usize, usize, usize)) -> bool {
        let n = self.merges.len();
        let g = range;
        self.merges
            .retain(|m| !(m.0 <= g.2 && g.0 <= m.2 && m.1 <= g.3 && g.1 <= m.3));
        self.merges.len() != n
    }

    /// Bezüge in Formeln nachziehen, wenn bei `at` eine Zeile (`row`) bzw. Spalte eingefügt wird.
    fn shift_refs(&mut self, row: bool, at: usize) {
        for line in self.cells.iter_mut() {
            for cell in line.iter_mut() {
                if cell.starts_with('=') {
                    *cell = crate::sheet::map_refs(cell, |r, c, _, _| {
                        Some(match row {
                            true if r >= at => (r + 1, c),
                            false if c >= at => (r, c + 1),
                            _ => (r, c),
                        })
                    });
                }
            }
        }
    }

    /// Bezüge in Formeln nachziehen, wenn bei `at` eine Zeile (`row`) bzw. Spalte entfällt:
    /// dahinter rücken sie nach, ein Bereich wird kleiner, ein Bezug auf die entfernte Zelle
    /// selbst wird ungültig (#REF!).
    fn unshift_refs(&mut self, row: bool, at: usize) {
        use crate::sheet::RefRole;
        for line in self.cells.iter_mut() {
            for cell in line.iter_mut() {
                if cell.starts_with('=') {
                    *cell = crate::sheet::map_refs_in(cell, |r, c, _, _, role| {
                        let k = if row { r } else { c };
                        let k = if k > at {
                            k - 1
                        } else if k < at {
                            k
                        } else {
                            match role {
                                RefRole::Single => return None,
                                RefRole::Start => k,
                                RefRole::End => k.checked_sub(1)?,
                            }
                        };
                        Some(if row { (k, c) } else { (r, k) })
                    });
                }
            }
        }
    }

    /// Zellen teilen – `cols`: nebeneinander, sonst untereinander. Verbundene Zellen werden
    /// in ihre Spalten bzw. Zeilen getrennt; sonst bekommt jede Spalte (Zeile) des Bereichs
    /// eine neue daneben, über die die übrigen Zellen dieser Spalte (Zeile) hinweglaufen.
    /// Gibt den Bereich danach zurück.
    pub fn split_cells(
        &mut self,
        range: (usize, usize, usize, usize),
        cols: bool,
    ) -> Option<(usize, usize, usize, usize)> {
        if !cols {
            // untereinander = nebeneinander in der gespiegelten Tabelle
            let align = self.align.clone();
            self.transpose();
            let g = self.split_cells((range.1, range.0, range.3, range.2), true);
            self.transpose();
            self.align = align;
            self.normalize();
            return g.map(|g| (g.1, g.0, g.3, g.2));
        }
        let g = self.expand(range);
        let inside = |m: &(usize, usize, usize, usize)| m.0 >= g.0 && m.2 <= g.2 && m.1 >= g.1 && m.3 <= g.3;
        // verbundene Zellen über mehrere Spalten: in Spalten trennen
        if self.merges.iter().any(|m| inside(m) && m.1 != m.3) {
            let mut kept = Vec::new();
            for m in std::mem::take(&mut self.merges) {
                if inside(&m) && m.1 != m.3 {
                    if m.0 != m.2 {
                        kept.extend((m.1..=m.3).map(|c| (m.0, c, m.2, c)));
                    }
                } else {
                    kept.push(m);
                }
            }
            self.merges = kept;
            self.normalize();
            return Some(g);
        }
        for c in (g.1..=g.3).rev() {
            let w = self.cols.get(c).copied().unwrap_or(1.0);
            self.insert_col(c + 1);
            self.cols[c] = w / 2.0;
            self.cols[c + 1] = w / 2.0;
            let mut covered = vec![false; self.n_rows()];
            for m in &mut self.merges {
                if m.1 <= c && c <= m.3 {
                    if m.0 >= g.0 && m.2 <= g.2 {
                        // im Bereich: senkrecht verbundene Zelle – bleibt in ihrer Spalte
                        for r in m.0..=m.2 {
                            covered[r] = true;
                        }
                        continue;
                    }
                    // außerhalb: läuft auch über die neue Spalte
                    if m.3 == c {
                        m.3 = c + 1;
                    }
                    for r in m.0..=m.2 {
                        covered[r] = true;
                    }
                }
            }
            // senkrecht verbundene Zellen im Bereich auch in der neuen Spalte verbinden
            let extra: Vec<_> = self
                .merges
                .iter()
                .filter(|m| m.1 == c && m.3 == c && m.0 >= g.0 && m.2 <= g.2)
                .map(|m| (m.0, c + 1, m.2, c + 1))
                .collect();
            self.merges.extend(extra);
            for (r, cov) in covered.iter().enumerate() {
                if !cov && (r < g.0 || r > g.2) {
                    self.merges.push((r, c, r, c + 1));
                }
            }
            self.normalize();
        }
        let n = g.3 - g.1 + 1;
        Some((g.0, g.1, g.2, g.3 + n))
    }

    /// Zeilen und Spalten tauschen (Hilfe für `split_cells`).
    fn transpose(&mut self) {
        let (nr, nc) = (self.n_rows(), self.n_cols());
        self.cells = (0..nc)
            .map(|c| (0..nr).map(|r| self.cells[r][c].clone()).collect())
            .collect();
        self.fmt = (0..nc)
            .map(|c| (0..nr).map(|r| self.fmt[r][c].clone()).collect())
            .collect();
        std::mem::swap(&mut self.rows, &mut self.cols);
        for m in &mut self.merges {
            *m = (m.1, m.0, m.3, m.2);
        }
        for line in self.cells.iter_mut() {
            for cell in line.iter_mut() {
                if cell.starts_with('=') {
                    *cell = crate::sheet::map_refs(cell, |r, c, _, _| Some((c, r)));
                }
            }
        }
        self.align = vec![None; nr];
    }

    pub fn insert_row(&mut self, at: usize) {
        let at = at.min(self.cells.len());
        self.shift_refs(true, at);
        // neue Zeile sieht aus wie die darüber (bzw. die erste wie die darunter)
        let like = self
            .fmt
            .get(if at > 0 { at - 1 } else { 0 })
            .cloned()
            .unwrap_or_default();
        let h = self
            .rows
            .get(if at > 0 { at - 1 } else { 0 })
            .copied()
            .unwrap_or(1.0);
        self.cells.insert(at, vec![String::new(); self.n_cols().max(1)]);
        self.rows.insert(at.min(self.rows.len()), h);
        self.fmt.insert(at.min(self.fmt.len()), like);
        for m in &mut self.merges {
            if at <= m.0 {
                m.0 += 1;
                m.2 += 1;
            } else if at <= m.2 {
                m.2 += 1;
            }
        }
        self.normalize();
    }

    pub fn insert_col(&mut self, at: usize) {
        let at = at.min(self.n_cols());
        self.shift_refs(false, at);
        for r in &mut self.cells {
            r.insert(at.min(r.len()), String::new());
        }
        for r in &mut self.fmt {
            let like = r
                .get(if at > 0 { at - 1 } else { 0 })
                .cloned()
                .unwrap_or_default();
            r.insert(at.min(r.len()), like);
        }
        let w = self
            .cols
            .get(if at > 0 { at - 1 } else { 0 })
            .copied()
            .unwrap_or(1.0);
        self.cols.insert(at.min(self.cols.len()), w);
        self.align.insert(at.min(self.align.len()), None);
        for m in &mut self.merges {
            if at <= m.1 {
                m.1 += 1;
                m.3 += 1;
            } else if at <= m.3 {
                m.3 += 1;
            }
        }
        self.normalize();
    }

    pub fn remove_row(&mut self, at: usize) {
        if self.cells.len() > 1 && at < self.cells.len() {
            self.unshift_refs(true, at);
            self.cells.remove(at);
            if at < self.rows.len() {
                self.rows.remove(at);
            }
            if at < self.fmt.len() {
                self.fmt.remove(at);
            }
            self.merges.retain(|m| !(m.0 == at && m.2 == at));
            for m in &mut self.merges {
                if at < m.0 {
                    m.0 -= 1;
                    m.2 -= 1;
                } else if at <= m.2 {
                    m.2 -= 1;
                }
            }
            self.normalize();
        }
    }

    pub fn remove_col(&mut self, at: usize) {
        if self.n_cols() > 1 && at < self.n_cols() {
            self.unshift_refs(false, at);
            for r in self.cells.iter_mut() {
                if at < r.len() {
                    r.remove(at);
                }
            }
            for r in self.fmt.iter_mut() {
                if at < r.len() {
                    r.remove(at);
                }
            }
            if at < self.cols.len() {
                self.cols.remove(at);
            }
            if at < self.align.len() {
                self.align.remove(at);
            }
            self.merges.retain(|m| !(m.1 == at && m.3 == at));
            for m in &mut self.merges {
                if at < m.1 {
                    m.1 -= 1;
                    m.3 -= 1;
                } else if at <= m.3 {
                    m.3 -= 1;
                }
            }
            self.normalize();
        }
    }

    /// Lage der Zellen im Rahmen: Grenzen der Spalten (x) und Zeilen (y).
    pub fn grid(&self, frame: Rect) -> (Vec<f64>, Vec<f64>) {
        let edges = |weights: &[f64], a: f64, len: f64| {
            let total: f64 = weights.iter().sum::<f64>().max(1e-9);
            let mut out = vec![a];
            let mut acc = 0.0;
            for w in weights {
                acc += w;
                out.push(a + len * acc / total);
            }
            out
        };
        (
            edges(&self.cols, frame.x0, frame.width()),
            edges(&self.rows, frame.y0, frame.height()),
        )
    }

    /// Zelle an einem Punkt (Zeile, Spalte).
    pub fn cell_at(&self, frame: Rect, p: Pt) -> Option<(usize, usize)> {
        let (xs, ys) = self.grid(frame);
        let c = xs.windows(2).position(|w| p.x >= w[0] && p.x < w[1])?;
        let r = ys.windows(2).position(|w| p.y >= w[0] && p.y < w[1])?;
        // verbundene Zellen: die oben links
        Some(self.merge_at(r, c).map_or((r, c), |m| (m.0, m.1)))
    }

    /// Ausfüllen (wie mit dem Kästchen unten rechts in einer Tabellenkalkulation): Zellen
    /// `r0..=r1` × `c0..=c1` um `dr` Zeilen bzw. `dc` Spalten fortsetzen (negativ: nach oben
    /// bzw. links; nur eines von beiden). Zahlen und „Text 1“ zählen weiter, Formeln
    /// wandern mit, alles andere wird wiederholt; das Aussehen kommt mit. Reicht es über
    /// die Tabelle hinaus, kommen unten bzw. rechts Zeilen/Spalten dazu.
    pub fn fill(&mut self, range: (usize, usize, usize, usize), dr: isize, dc: isize, comma: bool) {
        let (r0, c0, r1, c1) = range;
        if dr != 0 {
            let need = r1 as isize + dr + 1;
            while (self.n_rows() as isize) < need {
                let n = self.n_rows();
                self.insert_row(n);
            }
            let n = dr.unsigned_abs();
            for c in c0..=c1 {
                let src: Vec<String> = (r0..=r1).map(|r| self.cells[r][c].clone()).collect();
                let fmts: Vec<Fmt> = (r0..=r1).map(|r| self.fmt[r][c].clone()).collect();
                for j in 0..n {
                    let (pos, r) = if dr > 0 {
                        ((src.len() + j) as isize, r1 + 1 + j)
                    } else {
                        (-(j as isize) - 1, r0 - 1 - j)
                    };
                    self.cells[r][c] = series_value(&src, pos, comma, true);
                    self.fmt[r][c] = fmts[pos.rem_euclid(fmts.len() as isize) as usize].clone();
                }
            }
        } else if dc != 0 {
            let need = c1 as isize + dc + 1;
            while (self.n_cols() as isize) < need {
                let n = self.n_cols();
                self.insert_col(n);
            }
            let n = dc.unsigned_abs();
            for r in r0..=r1 {
                let src: Vec<String> = self.cells[r][c0..=c1].to_vec();
                let fmts: Vec<Fmt> = self.fmt[r][c0..=c1].to_vec();
                for j in 0..n {
                    let (pos, c) = if dc > 0 {
                        ((src.len() + j) as isize, c1 + 1 + j)
                    } else {
                        (-(j as isize) - 1, c0 - 1 - j)
                    };
                    self.cells[r][c] = series_value(&src, pos, comma, false);
                    self.fmt[r][c] = fmts[pos.rem_euclid(fmts.len() as isize) as usize].clone();
                }
            }
        }
    }

    /// Zellen `r0..=r1` × `c0..=c1` um `dr`/`dc` verschieben (Inhalt und Aussehen; was dort
    /// war, wird überschrieben, die alte Stelle leer). Bezüge auf die verschobenen Zellen
    /// wandern in allen Formeln mit.
    pub fn move_block(&mut self, range: (usize, usize, usize, usize), dr: isize, dc: isize) -> bool {
        let (r0, c0, r1, c1) = range;
        let (nr, nc) = (self.n_rows() as isize, self.n_cols() as isize);
        if (dr == 0 && dc == 0)
            || r0 as isize + dr < 0
            || c0 as isize + dc < 0
            || r1 as isize + dr >= nr
            || c1 as isize + dc >= nc
        {
            return false;
        }
        let inside = |r: usize, c: usize| (r0..=r1).contains(&r) && (c0..=c1).contains(&c);
        // Verbindungen im Block wandern mit, angeschnittene lösen sich
        let target = (
            (r0 as isize + dr) as usize,
            (c0 as isize + dc) as usize,
            (r1 as isize + dr) as usize,
            (c1 as isize + dc) as usize,
        );
        let hits = |m: &(usize, usize, usize, usize), g: (usize, usize, usize, usize)| {
            m.0 <= g.2 && g.0 <= m.2 && m.1 <= g.3 && g.1 <= m.3
        };
        let mut moved = Vec::new();
        for m in std::mem::take(&mut self.merges) {
            if inside(m.0, m.1) && inside(m.2, m.3) {
                moved.push((
                    (m.0 as isize + dr) as usize,
                    (m.1 as isize + dc) as usize,
                    (m.2 as isize + dr) as usize,
                    (m.3 as isize + dc) as usize,
                ));
            } else if !hits(&m, (r0, c0, r1, c1)) && !hits(&m, target) {
                moved.push(m);
            }
        }
        self.merges = moved;
        // Bezüge auf den Block nachziehen
        for row in self.cells.iter_mut() {
            for cell in row.iter_mut() {
                if cell.starts_with('=') {
                    *cell = crate::sheet::map_refs(cell, |r, c, _, _| {
                        Some(if inside(r, c) {
                            ((r as isize + dr) as usize, (c as isize + dc) as usize)
                        } else {
                            (r, c)
                        })
                    });
                }
            }
        }
        let block: Vec<Vec<(String, Fmt)>> = (r0..=r1)
            .map(|r| {
                (c0..=c1)
                    .map(|c| {
                        (
                            std::mem::take(&mut self.cells[r][c]),
                            std::mem::take(&mut self.fmt[r][c]),
                        )
                    })
                    .collect()
            })
            .collect();
        for (i, row) in block.into_iter().enumerate() {
            for (j, (text, f)) in row.into_iter().enumerate() {
                let (r, c) = ((r0 + i) as isize + dr, (c0 + j) as isize + dc);
                self.cells[r as usize][c as usize] = text;
                self.fmt[r as usize][c as usize] = f;
            }
        }
        true
    }

    /// Aussehen einer Zelle (`num`: ihr Wert ist eine Zahl – steht dann rechts).
    pub fn look(&self, r: usize, c: usize, num: bool) -> Look {
        let head = self.header && r == 0;
        let mut l = Look {
            fill: if head { self.header_fill } else { self.fill },
            stroke: self.border,
            width: self.border_width,
            family: self.family.clone(),
            size: self.size,
            bold: head,
            italic: false,
            align: self.align.get(c).copied().flatten().unwrap_or(if head {
                Align::Center
            } else if num {
                Align::Right
            } else {
                Align::Left
            }),
            ..Look::text(
                &self.family,
                self.size,
                if head { self.header_text } else { self.text },
            )
        };
        if let Some(f) = self.fmt.get(r).and_then(|row| row.get(c)) {
            l.apply(f);
        }
        l
    }

    /// Aussehen aller Zellen.
    pub fn looks(&self, comma: bool) -> Vec<Vec<Look>> {
        let vals = evaluate(&self.cells, comma);
        (0..self.n_rows())
            .map(|r| {
                (0..self.n_cols())
                    .map(|c| {
                        let num = vals.get(r).and_then(|v| v.get(c)).is_some_and(|v| v.is_number());
                        self.look(r, c, num)
                    })
                    .collect()
            })
            .collect()
    }
}

impl Chart {
    /// Neues Diagramm mit Beispieldaten.
    pub fn sample(kind: ChartKind) -> Chart {
        let data = vec![
            vec![String::new(), tr("Amount").to_string()],
            vec!["A".into(), "40".into()],
            vec!["B".into(), "25".into()],
            vec!["C".into(), "20".into()],
            vec!["D".into(), "15".into()],
        ];
        Chart {
            kind,
            data,
            header: true,
            colors: PALETTE.to_vec(),
            family: "Sans".into(),
            size: 12.0,
            text: Color::rgb(0.12941176470588237, 0.12941176470588237, 0.12941176470588237),
            legend: true,
            values: true,
            source: 0,
            series_fmt: Vec::new(),
            points: Vec::new(),
            labels_fmt: Fmt::default(),
            values_fmt: Fmt::default(),
            texts: Vec::new(),
            orient: None,
            x_title: String::new(),
            y_title: String::new(),
        }
    }

    fn color(&self, i: usize) -> Color {
        if self.colors.is_empty() {
            PALETTE[i % PALETTE.len()]
        } else {
            self.colors[i % self.colors.len()]
        }
    }

    /// Aufbau der Daten (siehe [`data_layout`]).
    pub fn layout(&self, comma: bool) -> DataLayout {
        data_layout(&evaluate(&self.data, comma), self.orient)
    }

    /// Beschriftungen (x-Achse bzw. Stücke), Namen der Reihen und Werte (ausgewertet; keine
    /// Zahl = 0). Der Aufbau der Daten wird erkannt: Kopfzeile, Beschriftungsspalte und ob
    /// die Reihen in Zeilen oder Spalten liegen.
    pub fn series(&self, comma: bool) -> (Vec<String>, Vec<String>, Vec<Vec<f64>>) {
        let vals = evaluate(&self.data, comma);
        let l = data_layout(&vals, self.orient);
        let at = |r: usize, c: usize| {
            vals.get(r)
                .and_then(|row| row.get(c))
                .cloned()
                .unwrap_or(Value::Empty)
        };
        let (r0, c0) = (usize::from(l.head), usize::from(l.labels));
        // Reihen über `outer`, Punkte über `inner` (in Tabellenzeilen bzw. -spalten)
        let (outer, inner): (Vec<usize>, Vec<usize>) = if l.by_rows {
            ((r0..l.rows).collect(), (c0..l.cols).collect())
        } else {
            ((c0..l.cols).collect(), (r0..l.rows).collect())
        };
        let cell = |o: usize, i: usize| if l.by_rows { at(o, i) } else { at(i, o) };
        let names: Vec<String> = outer
            .iter()
            .enumerate()
            .map(|(k, o)| {
                let n = if l.by_rows {
                    if l.labels {
                        at(*o, 0).display(comma)
                    } else {
                        String::new()
                    }
                } else if l.head {
                    at(0, *o).display(comma)
                } else {
                    String::new()
                };
                if n.trim().is_empty() {
                    crate::i18n::trf("Series {}", &[&(k + 1)])
                } else {
                    n
                }
            })
            .collect();
        let labels: Vec<String> = inner
            .iter()
            .enumerate()
            .map(|(k, i)| {
                let n = if l.by_rows {
                    if l.head {
                        at(0, *i).display(comma)
                    } else {
                        String::new()
                    }
                } else if l.labels {
                    at(*i, 0).display(comma)
                } else {
                    String::new()
                };
                if n.trim().is_empty() {
                    (k + 1).to_string()
                } else {
                    n
                }
            })
            .collect();
        // ohne eine einzige Zahl gibt es nichts zu zeichnen
        if !outer
            .iter()
            .any(|o| inner.iter().any(|i| cell(*o, *i).as_num().is_some()))
        {
            return (Vec::new(), Vec::new(), Vec::new());
        }
        let series = outer
            .iter()
            .map(|o| {
                inner
                    .iter()
                    .map(|i| cell(*o, *i).as_num().unwrap_or(0.0))
                    .collect()
            })
            .collect();
        (labels, names, series)
    }

    /// Diagrammart, die zu den Daten passt: Beschriftungen wie Jahre, Daten oder steigende
    /// Zahlen (eine Zeitreihe) → Linien; mehrere Reihen → Balken; eine Reihe mit wenigen,
    /// nicht negativen Werten → Kreis; sonst Balken.
    pub fn suggest_kind(&self, comma: bool) -> ChartKind {
        let (labels, _, series) = self.series(comma);
        // nur echte Beschriftungen zählen (nicht die selbst durchnummerierten)
        let l = self.layout(comma);
        let named = if l.by_rows { l.head } else { l.labels };
        let nums: Vec<Option<f64>> = labels
            .iter()
            .map(|l| crate::sheet::parse_number(l, comma))
            .collect();
        let dates = labels
            .iter()
            .filter(|l| {
                let p: Vec<&str> = l.split(['.', '/', '-']).collect();
                p.len() == 3
                    && p.iter()
                        .all(|x| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit()))
            })
            .count();
        let rising = named
            && labels.len() >= 3
            && nums.iter().all(|n| n.is_some())
            && nums.windows(2).all(|w| w[0] < w[1]);
        if rising || (named && labels.len() >= 3 && dates == labels.len()) {
            return ChartKind::Line;
        }
        if series.len() > 1 {
            return ChartKind::Bar;
        }
        let ok_pie = series
            .first()
            .is_some_and(|v| !v.is_empty() && v.len() <= 8 && v.iter().all(|x| *x >= 0.0));
        if ok_pie {
            ChartKind::Pie
        } else {
            ChartKind::Bar
        }
    }

    /// Titel der x-Achse: eigener, sonst der Text in der Ecke oben links (wenn er die
    /// Beschriftungen benennt).
    pub fn x_axis_title(&self, comma: bool) -> String {
        if !self.x_title.trim().is_empty() {
            return self.x_title.clone();
        }
        let vals = evaluate(&self.data, comma);
        let l = data_layout(&vals, self.orient);
        match vals.first().and_then(|r| r.first()) {
            Some(Value::Text(t)) if l.head && l.labels && !l.by_rows => t.clone(),
            _ => String::new(),
        }
    }

    fn pie(&self) -> bool {
        matches!(self.kind, ChartKind::Pie | ChartKind::Donut)
    }

    fn point_fmt(&self, si: usize, i: usize) -> Option<&Fmt> {
        self.points
            .iter()
            .find(|(a, b, _)| *a == si && *b == i)
            .map(|p| &p.2)
    }

    /// Abweichungen eines Datenpunkts (zum Ändern; legt sie bei Bedarf an).
    pub fn point_fmt_mut(&mut self, si: usize, i: usize) -> &mut Fmt {
        let k = match self.points.iter().position(|(a, b, _)| *a == si && *b == i) {
            Some(k) => k,
            None => {
                self.points.push((si, i, Fmt::default()));
                self.points.len() - 1
            }
        };
        &mut self.points[k].2
    }

    pub fn series_fmt_mut(&mut self, si: usize) -> &mut Fmt {
        if self.series_fmt.len() <= si {
            self.series_fmt.resize(si + 1, Fmt::default());
        }
        &mut self.series_fmt[si]
    }

    /// Aussehen einer Reihe: Farbe (Balken, Punkte, Legende) und Linie.
    pub fn series_look(&self, si: usize) -> Look {
        let line = self.kind == ChartKind::Line;
        let mut l = Look::text(&self.family, self.size, self.text);
        l.fill = Some(self.color(si));
        let f = self.series_fmt.get(si).cloned().unwrap_or_default();
        if let Some(v) = f.fill {
            l.fill = v;
        }
        l.stroke = if line {
            l.fill.or(Some(self.color(si)))
        } else {
            None
        };
        l.width = if line { (self.size * 0.2).max(1.0) } else { 1.0 };
        if let Some(v) = f.stroke {
            l.stroke = v;
        }
        if let Some(v) = f.width {
            l.width = v;
        }
        l
    }

    /// Aussehen eines Datenpunkts (Kreis: Stück `i`; sonst Balken bzw. Punkt).
    pub fn point_look(&self, si: usize, i: usize) -> Look {
        let s = self.size;
        let mut l = if self.pie() {
            let mut l = Look::text(&self.family, s, self.text);
            l.fill = Some(self.color(i));
            if let Some(Some(c)) = self.series_fmt.get(i).and_then(|f| f.fill) {
                l.fill = Some(c);
            }
            // feine Trennlinie zwischen den Stücken
            l.stroke = Some(Color::WHITE);
            l.width = (s * 0.12).max(0.5);
            l
        } else {
            let mut l = self.series_look(si);
            if self.kind == ChartKind::Line {
                l.stroke = Some(Color::WHITE);
                l.width = (s * 0.08).max(0.5);
            }
            l
        };
        if let Some(f) = self.point_fmt(si, i) {
            l.apply(f);
        }
        l
    }

    /// Schrift der Beschriftungen.
    pub fn labels_look(&self) -> Look {
        let mut l = Look::text(&self.family, self.size, self.text);
        l.apply(&self.labels_fmt);
        l
    }

    /// Schrift der Werte (Kreis: fett, auf dem Stück lesbar; sonst etwas kleiner).
    pub fn values_look(&self, bg: Option<Color>) -> Look {
        let pie = self.pie();
        let mut l = Look::text(
            &self.family,
            if pie { self.size } else { self.size * 0.8 },
            match bg {
                Some(c) if pie => contrast(c),
                _ => self.text,
            },
        );
        l.bold = pie;
        l.align = Align::Center;
        l.apply(&self.values_fmt);
        l
    }

    /// Abweichungen eines einzelnen Textes (zum Ändern; legt sie bei Bedarf an).
    pub fn text_fmt_mut(&mut self, key: TextKey) -> &mut Fmt {
        let k = match self.texts.iter().position(|t| t.0 == key) {
            Some(k) => k,
            None => {
                self.texts.push((key, Fmt::default()));
                self.texts.len() - 1
            }
        };
        &mut self.texts[k].1
    }

    /// Aussehen eines einzelnen Textes: Beschriftungen bzw. Werte, dazu seine Abweichungen.
    pub fn text_look(&self, key: TextKey) -> Look {
        let lab = self.labels_look();
        let mut l = match key {
            // links: Farbe vor dem Text; rechts: Text rechtsbündig, Farbe dahinter
            TextKey::Legend(_) => {
                let a = if lab.align == Align::Right {
                    Align::Right
                } else {
                    Align::Left
                };
                sized(&lab, 1.0, a, VAlign::Middle)
            }
            TextKey::Tick(_) => sized(&lab, 0.85, Align::Right, VAlign::Middle),
            TextKey::Category(_) => sized(&lab, 0.85, Align::Center, VAlign::Top),
            TextKey::XTitle | TextKey::YTitle => {
                let mut t = sized(&lab, 0.95, Align::Center, VAlign::Middle);
                t.bold = true;
                t
            }
            TextKey::Value(si, i) => {
                let bg = if self.pie() {
                    self.point_look(si, i).fill
                } else {
                    None
                };
                let vl = self.values_look(bg);
                let a = if self.pie() { Align::Center } else { vl.align };
                sized(&vl, 1.0, a, VAlign::Middle)
            }
        };
        if let Some((_, f)) = self.texts.iter().find(|t| t.0 == key) {
            l.apply(f);
        }
        l
    }
}

/// Wert an Stelle `pos` einer Reihe, die mit `src` beginnt (Stellen `0..src.len()` sind
/// `src` selbst; `vertical`: die Reihe läuft nach unten, sonst nach rechts).
fn series_value(src: &[String], pos: isize, comma: bool, vertical: bool) -> String {
    let k = src.len() as isize;
    if k == 0 {
        return String::new();
    }
    let step_of = |v: &[f64]| {
        if v.len() < 2 {
            1.0
        } else {
            (v[v.len() - 1] - v[0]) / (v.len() - 1) as f64
        }
    };
    let clean = |x: f64| (x * 1e9).round() / 1e9;
    // lauter Zahlen: weiterzählen (eine Zahl: in Einerschritten)
    let nums: Option<Vec<f64>> = src
        .iter()
        .map(|s| {
            if s.trim_start().starts_with('=') {
                None
            } else {
                crate::sheet::parse_number(s, comma)
            }
        })
        .collect();
    if let Some(v) = nums {
        let step = step_of(&v);
        return format_number(clean(v[0] + step * pos as f64), comma);
    }
    // gleicher Text mit Zahl am Ende („Woche 1“, „Q1“): die Zahl zählt weiter
    let split = |s: &str| -> Option<(String, i64)> {
        let digits = s.len() - s.trim_end_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 || digits > 15 {
            return None;
        }
        let (head, tail) = s.split_at(s.len() - digits);
        if head.is_empty() || head.starts_with('=') {
            return None;
        }
        Some((head.to_string(), tail.parse().ok()?))
    };
    let parts: Option<Vec<(String, i64)>> = src.iter().map(|s| split(s)).collect();
    if let Some(p) = parts.filter(|p| p.iter().all(|x| x.0 == p[0].0)) {
        let v: Vec<f64> = p.iter().map(|x| x.1 as f64).collect();
        let step = step_of(&v).round();
        let n = (v[0] + step * pos as f64).round() as i64;
        if n >= 0 {
            return format!("{}{n}", p[0].0);
        }
    }
    // sonst wiederholen; Formeln wandern mit
    let i = pos.rem_euclid(k);
    let s = &src[i as usize];
    if s.trim_start().starts_with('=') {
        let off = pos - i;
        let (dr, dc) = if vertical { (off, 0) } else { (0, off) };
        return crate::sheet::shift_formula(s, dr, dc);
    }
    s.clone()
}

// ───────────────────────────── Speichern ─────────────────────────────

/// Zeichen, die in den gespeicherten Daten maskiert werden.
fn enc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '%' | '|' | '^' | ';' | '=' | ',' | ':' | '\n' | '\r' | '\t' => {
                o.push_str(&format!("%{:02X}", c as u32))
            }
            c => o.push(c),
        }
    }
    o
}

fn dec(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() {
            let hex = |x: u8| (x as char).to_digit(16).unwrap_or(0) as u8;
            out.push(hex(b[i + 1]) * 16 + hex(b[i + 2]));
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn enc_grid(g: &[Vec<String>]) -> String {
    g.iter()
        .map(|r| r.iter().map(|c| enc(c)).collect::<Vec<_>>().join("|"))
        .collect::<Vec<_>>()
        .join("^")
}

fn dec_grid(s: &str) -> Vec<Vec<String>> {
    s.split('^').map(|r| r.split('|').map(dec).collect()).collect()
}

fn enc_color(c: Option<Color>) -> String {
    match c {
        None => "none".into(),
        Some(c) if c.a < 1.0 => format!(
            "{}{:02x}",
            c.to_hex(),
            (c.a.clamp(0.0, 1.0) * 255.0).round() as u8
        ),
        Some(c) => c.to_hex(),
    }
}

impl Fmt {
    pub fn is_empty(&self) -> bool {
        *self == Fmt::default()
    }

    /// Felder vergessen, die in `other` gesetzt sind.
    pub fn clear_like(&mut self, other: &Fmt) {
        macro_rules! clear {
            ($($f:ident),*) => {
                $(if other.$f.is_some() {
                    self.$f = None;
                })*
            };
        }
        clear!(
            fill,
            stroke,
            width,
            family,
            size,
            bold,
            italic,
            color,
            align,
            valign,
            underline,
            strike,
            weight,
            small_caps,
            no_ligatures,
            line,
            tracking,
            vertical,
            number
        );
    }

    /// Als Text: `schlüssel:wert` durch Kommas getrennt.
    fn encode(&self) -> String {
        let mut f: Vec<String> = Vec::new();
        if let Some(c) = self.fill {
            f.push(format!("f:{}", enc_color(c)));
        }
        if let Some(c) = self.stroke {
            f.push(format!("s:{}", enc_color(c)));
        }
        if let Some(w) = self.width {
            f.push(format!("w:{w}"));
        }
        if let Some(v) = &self.family {
            f.push(format!("ff:{}", enc(v)));
        }
        if let Some(v) = self.size {
            f.push(format!("z:{v}"));
        }
        if let Some(v) = self.bold {
            f.push(format!("b:{}", v as u8));
        }
        if let Some(v) = self.italic {
            f.push(format!("i:{}", v as u8));
        }
        if let Some(c) = self.color {
            f.push(format!("c:{}", enc_color(Some(c))));
        }
        if let Some(a) = self.align {
            f.push(format!("a:{}", align_name(Some(a))));
        }
        if let Some(v) = self.valign {
            f.push(format!("v:{}", v.name()));
        }
        for (k, v) in [
            ("u", self.underline),
            ("k", self.strike),
            ("sc", self.small_caps),
            ("nl", self.no_ligatures),
        ] {
            if let Some(v) = v {
                f.push(format!("{k}:{}", v as u8));
            }
        }
        if let Some(v) = self.weight {
            f.push(format!("wt:{v}"));
        }
        if let Some(v) = self.line {
            f.push(format!("ln:{v}"));
        }
        if let Some(v) = self.tracking {
            f.push(format!("tr:{v}"));
        }
        if let Some(v) = self.vertical {
            f.push(format!("vt:{v}"));
        }
        if let Some(v) = &self.number {
            f.push(format!("nf:{}", v.encode()));
        }
        f.join(",")
    }

    fn decode(s: &str) -> Fmt {
        let mut f = Fmt::default();
        for part in s.split(',') {
            let Some((k, v)) = part.split_once(':') else {
                continue;
            };
            let num = || v.parse::<f64>().ok().filter(|x| x.is_finite());
            match k {
                "f" => f.fill = Some(dec_color(v)),
                "s" => f.stroke = Some(dec_color(v)),
                "w" => f.width = num().map(|x| x.clamp(0.0, 100.0)),
                "ff" => f.family = Some(dec(v)),
                "z" => f.size = num().map(|x| x.clamp(1.0, 1000.0)),
                "b" => f.bold = Some(v == "1"),
                "i" => f.italic = Some(v == "1"),
                "c" => f.color = dec_color(v),
                "a" => f.align = parse_align(v),
                "v" => f.valign = VAlign::ALL.into_iter().find(|a| a.name() == v),
                "u" => f.underline = Some(v == "1"),
                "k" => f.strike = Some(v == "1"),
                "sc" => f.small_caps = Some(v == "1"),
                "nl" => f.no_ligatures = Some(v == "1"),
                "wt" => f.weight = v.parse::<u16>().ok().map(|w| w.min(1000)),
                "ln" => f.line = num().map(|x| x.clamp(0.5, 5.0)),
                "tr" => f.tracking = num().map(|x| x.clamp(-300.0, 2000.0)),
                "vt" => f.vertical = v.parse::<u8>().ok().filter(|x| *x <= 2),
                "nf" => f.number = NumFmt::decode(v),
                _ => {}
            }
        }
        f
    }
}

fn parse_align(a: &str) -> Option<Align> {
    match a {
        "left" => Some(Align::Left),
        "center" => Some(Align::Center),
        "right" => Some(Align::Right),
        "justify" => Some(Align::Justify),
        _ => None,
    }
}

/// Liste von Formaten (durch `|` getrennt).
fn enc_fmts(v: &[Fmt]) -> String {
    v.iter().map(|f| f.encode()).collect::<Vec<_>>().join("|")
}

fn dec_fmts(s: &str) -> Vec<Fmt> {
    if s.is_empty() {
        return Vec::new();
    }
    s.split('|').map(Fmt::decode).collect()
}

fn dec_color(s: &str) -> Option<Color> {
    if s == "none" {
        None
    } else {
        Color::parse(s)
    }
}

fn nums(v: &[f64]) -> String {
    v.iter()
        .map(|x| format!("{}", (x * 1000.0).round() / 1000.0))
        .collect::<Vec<_>>()
        .join(",")
}

fn align_name(a: Option<Align>) -> &'static str {
    match a {
        None => "auto",
        Some(Align::Left) => "left",
        Some(Align::Center) => "center",
        Some(Align::Right) => "right",
        Some(Align::Justify) => "justify",
    }
}

impl Smart {
    /// Als Text für das Attribut `mondrian:smart`.
    pub fn encode(&self) -> String {
        let mut f: Vec<(&str, String)> = Vec::new();
        match self {
            Smart::Table(t) => {
                f.push(("type", "table".into()));
                f.push(("cols", nums(&t.cols)));
                f.push(("rows", nums(&t.rows)));
                f.push(("header", (t.header as u8).to_string()));
                f.push((
                    "align",
                    t.align
                        .iter()
                        .map(|a| align_name(*a))
                        .collect::<Vec<_>>()
                        .join(","),
                ));
                f.push(("family", enc(&t.family)));
                f.push(("size", format!("{}", t.size)));
                f.push(("text", enc_color(Some(t.text))));
                f.push(("header-text", enc_color(Some(t.header_text))));
                f.push(("header-fill", enc_color(t.header_fill)));
                f.push(("fill", enc_color(t.fill)));
                f.push(("border", enc_color(t.border)));
                f.push(("border-width", format!("{}", t.border_width)));
                f.push(("key", t.key.to_string()));
                f.push(("cells", enc_grid(&t.cells)));
                if !t.merges.is_empty() {
                    f.push((
                        "merges",
                        t.merges
                            .iter()
                            .map(|m| format!("{}.{}.{}.{}", m.0, m.1, m.2, m.3))
                            .collect::<Vec<_>>()
                            .join(","),
                    ));
                }
                // Zellformate: Liste der verschiedenen, dazu je Zelle deren Nummer (0 = keins)
                let mut pal: Vec<Fmt> = Vec::new();
                let grid: Vec<String> = t
                    .fmt
                    .iter()
                    .map(|row| {
                        row.iter()
                            .map(|cf| {
                                if cf.is_empty() {
                                    return "0".to_string();
                                }
                                let k = match pal.iter().position(|p| p == cf) {
                                    Some(k) => k,
                                    None => {
                                        pal.push(cf.clone());
                                        pal.len() - 1
                                    }
                                };
                                (k + 1).to_string()
                            })
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .collect();
                if !pal.is_empty() {
                    f.push(("fmts", enc_fmts(&pal)));
                    f.push(("cellfmt", grid.join("^")));
                }
            }
            Smart::Chart(c) => {
                f.push(("type", "chart".into()));
                f.push(("kind", c.kind.name().into()));
                f.push((
                    "colors",
                    c.colors.iter().map(|c| c.to_hex()).collect::<Vec<_>>().join(","),
                ));
                f.push(("family", enc(&c.family)));
                f.push(("size", format!("{}", c.size)));
                f.push(("text", enc_color(Some(c.text))));
                f.push(("legend", (c.legend as u8).to_string()));
                f.push(("values", (c.values as u8).to_string()));
                f.push(("source", c.source.to_string()));
                f.push(("header", (c.header as u8).to_string()));
                if let Some(o) = c.orient {
                    f.push(("orient", if o { "rows" } else { "cols" }.to_string()));
                }
                if !c.x_title.is_empty() {
                    f.push(("x-title", enc(&c.x_title)));
                }
                if !c.y_title.is_empty() {
                    f.push(("y-title", enc(&c.y_title)));
                }
                f.push(("data", enc_grid(&c.data)));
                if c.series_fmt.iter().any(|f| !f.is_empty()) {
                    f.push(("series-fmt", enc_fmts(&c.series_fmt)));
                }
                let pts: Vec<&(usize, usize, Fmt)> = c.points.iter().filter(|p| !p.2.is_empty()).collect();
                if !pts.is_empty() {
                    f.push((
                        "points",
                        pts.iter()
                            .map(|(a, b, _)| format!("{a}.{b}"))
                            .collect::<Vec<_>>()
                            .join(","),
                    ));
                    f.push((
                        "point-fmt",
                        pts.iter().map(|p| p.2.encode()).collect::<Vec<_>>().join("|"),
                    ));
                }
                if !c.labels_fmt.is_empty() {
                    f.push(("labels-fmt", c.labels_fmt.encode()));
                }
                if !c.values_fmt.is_empty() {
                    f.push(("values-fmt", c.values_fmt.encode()));
                }
                let texts: Vec<&(TextKey, Fmt)> = c.texts.iter().filter(|t| !t.1.is_empty()).collect();
                if !texts.is_empty() {
                    f.push((
                        "texts",
                        texts.iter().map(|t| t.0.encode()).collect::<Vec<_>>().join(","),
                    ));
                    f.push((
                        "text-fmt",
                        texts.iter().map(|t| t.1.encode()).collect::<Vec<_>>().join("|"),
                    ));
                }
            }
        }
        let mut s = String::from("v=1");
        for (k, v) in f {
            s.push(';');
            s.push_str(k);
            s.push('=');
            s.push_str(&v);
        }
        s
    }

    pub fn decode(s: &str) -> Option<Smart> {
        let mut m = std::collections::HashMap::new();
        for part in s.split(';') {
            if let Some((k, v)) = part.split_once('=') {
                m.insert(k.trim(), v);
            }
        }
        let get = |k: &str| m.get(k).copied();
        let num = |k: &str, d: f64| {
            get(k)
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite())
                .unwrap_or(d)
        };
        let flag = |k: &str, d: bool| get(k).map(|v| v == "1").unwrap_or(d);
        let list = |k: &str| -> Vec<f64> {
            get(k)
                .map(|v| v.split(',').filter_map(|x| x.parse().ok()).collect())
                .unwrap_or_default()
        };
        match get("type")? {
            "table" => {
                let mut t = Table::with_cells(dec_grid(get("cells").unwrap_or("")));
                t.cols = list("cols");
                t.rows = list("rows");
                t.header = flag("header", true);
                t.align = get("align")
                    .map(|v| v.split(',').map(parse_align).collect())
                    .unwrap_or_default();
                t.family = get("family").map(dec).unwrap_or_else(|| "Sans".into());
                t.size = num("size", 12.0).clamp(1.0, 1000.0);
                t.text = get("text").and_then(dec_color).unwrap_or(t.text);
                t.header_text = get("header-text").and_then(dec_color).unwrap_or(t.header_text);
                t.header_fill = get("header-fill").map(dec_color).unwrap_or(t.header_fill);
                t.fill = get("fill").map(dec_color).unwrap_or(t.fill);
                t.border = get("border").map(dec_color).unwrap_or(t.border);
                t.border_width = num("border-width", 1.0).clamp(0.0, 100.0);
                t.key = get("key").and_then(|v| v.parse().ok()).unwrap_or(0);
                t.merges = get("merges")
                    .map(|v| {
                        v.split(',')
                            .filter_map(|m| {
                                let n: Vec<usize> = m.split('.').filter_map(|x| x.parse().ok()).collect();
                                (n.len() == 4).then(|| (n[0], n[1], n[2], n[3]))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if let (Some(pal), Some(grid)) = (get("fmts"), get("cellfmt")) {
                    let pal = dec_fmts(pal);
                    t.fmt = grid
                        .split('^')
                        .map(|row| {
                            row.split(',')
                                .map(|k| {
                                    k.parse::<usize>()
                                        .ok()
                                        .and_then(|k| k.checked_sub(1))
                                        .and_then(|k| pal.get(k).cloned())
                                        .unwrap_or_default()
                                })
                                .collect()
                        })
                        .collect();
                }
                t.normalize();
                // ältere Dateien: jede zweite Zeile hinterlegt – als Füllung dieser Zellen
                if let Some(Some(st)) = get("stripes").map(dec_color) {
                    for r in 0..t.n_rows() {
                        if (r + usize::from(!t.header)) % 2 == 0 && !(t.header && r == 0) {
                            for f in &mut t.fmt[r] {
                                if f.fill.is_none() {
                                    f.fill = Some(Some(st));
                                }
                            }
                        }
                    }
                }
                // ältere Dateien: feste Kopfzeile – ihr Aussehen als Format der ersten Zeile
                if t.header {
                    for c in 0..t.n_cols() {
                        let col_align = t.align.get(c).copied().flatten();
                        let f = &mut t.fmt[0][c];
                        f.fill.get_or_insert(t.header_fill);
                        f.color.get_or_insert(t.header_text);
                        f.bold.get_or_insert(true);
                        if col_align.is_none() {
                            f.align.get_or_insert(Align::Center);
                        }
                    }
                    t.header = false;
                }
                Some(Smart::Table(t))
            }
            "chart" => {
                let mut c = Chart::sample(get("kind").and_then(ChartKind::from_name).unwrap_or_default());
                c.data = dec_grid(get("data").unwrap_or(""));
                c.colors = get("colors")
                    .map(|v| v.split(',').filter_map(Color::parse).collect())
                    .unwrap_or_default();
                c.family = get("family").map(dec).unwrap_or_else(|| "Sans".into());
                c.size = num("size", 12.0).clamp(1.0, 1000.0);
                c.text = get("text").and_then(dec_color).unwrap_or(c.text);
                c.legend = flag("legend", true);
                c.values = flag("values", true);
                c.source = get("source").and_then(|v| v.parse().ok()).unwrap_or(0);
                c.header = flag("header", true);
                c.orient = match get("orient") {
                    Some("rows") => Some(true),
                    Some("cols") => Some(false),
                    _ => None,
                };
                c.x_title = get("x-title").map(dec).unwrap_or_default();
                c.y_title = get("y-title").map(dec).unwrap_or_default();
                c.series_fmt = get("series-fmt").map(dec_fmts).unwrap_or_default();
                if let (Some(ids), Some(fm)) = (get("points"), get("point-fmt")) {
                    c.points = ids
                        .split(',')
                        .zip(fm.split('|'))
                        .filter_map(|(id, f)| {
                            let (a, b) = id.split_once('.')?;
                            Some((a.parse().ok()?, b.parse().ok()?, Fmt::decode(f)))
                        })
                        .collect();
                }
                c.labels_fmt = get("labels-fmt").map(Fmt::decode).unwrap_or_default();
                c.values_fmt = get("values-fmt").map(Fmt::decode).unwrap_or_default();
                if let (Some(ids), Some(fm)) = (get("texts"), get("text-fmt")) {
                    c.texts = ids
                        .split(',')
                        .zip(fm.split('|'))
                        .filter_map(|(k, f)| Some((TextKey::decode(k)?, Fmt::decode(f))))
                        .collect();
                }
                Some(Smart::Chart(c))
            }
            _ => None,
        }
    }
}

// ───────────────────────────── Bauen ─────────────────────────────

/// Ein gebautes Teil: Form, Stil und Name (für die Ebenenliste).
pub type Part = (Shape, Style);

fn filled(c: Color) -> Style {
    Style {
        fill: Some(c),
        stroke: None,
        ..Style::default()
    }
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Shape {
    Shape::Rect {
        x,
        y,
        w: w.max(0.0),
        h: h.max(0.0),
        r: 0.0,
    }
}

/// Textfeld in einem Kasten (ein- oder mehrzeilig, umbrechend) mit der Schrift von `l`.
fn label(text: &str, b: Rect, l: &Look, inset: f64) -> Part {
    let fmt = l.text_format();
    (
        Shape::TextArea {
            xf: Affine::translate(b.x0, b.y0),
            w: b.width().max(1.0),
            h: b.height().max(1.0),
            text: text.to_string(),
            family: l.family.clone(),
            size: l.size,
            bold: l.bold,
            italic: l.italic,
            fmt,
            frame: TextFrame {
                inset,
                // Zellen und Beschriftungen weichen nichts aus
                no_flow: true,
                ..TextFrame::default()
            },
            rich: Default::default(),
        },
        filled(l.color),
    )
}

/// Schrift `l` in anderer Größe und Ausrichtung.
fn sized(l: &Look, k: f64, align: Align, valign: VAlign) -> Look {
    Look {
        size: l.size * k,
        align,
        valign,
        ..l.clone()
    }
}

/// Fläche und Kontur eines Teils.
fn painted(l: &Look) -> Style {
    Style {
        fill: l.fill,
        stroke: l.stroke.filter(|_| l.width > 0.0),
        width: l.width,
        join: LineJoin::Round,
        ..Style::default()
    }
}

/// Weiß oder fast Schwarz – was auf `bg` besser lesbar ist.
fn contrast(bg: Color) -> Color {
    let l = 0.2126 * bg.r + 0.7152 * bg.g + 0.0722 * bg.b;
    if l > 0.6 {
        Color::rgb(0.12941176470588237, 0.12941176470588237, 0.12941176470588237)
    } else {
        Color::WHITE
    }
}

fn line_path(segments: &[(Pt, Pt)]) -> Shape {
    Shape::Path {
        subpaths: segments
            .iter()
            .map(|(a, b)| SubPath {
                nodes: vec![Node::corner(*a), Node::corner(*b)],
                closed: false,
            })
            .collect(),
        evenodd: false,
    }
}

fn stroked(c: Color, w: f64) -> Style {
    Style {
        fill: None,
        stroke: Some(c),
        width: w,
        cap: LineCap::Square,
        join: LineJoin::Miter,
        ..Style::default()
    }
}

/// Bogen um `c` mit Radius `r` von `a0` bis `a1` (Bogenmaß, im Uhrzeigersinn auf dem
/// Bildschirm) als Knoten: kubische Stücke von höchstens 90°.
fn arc_nodes(c: Pt, r: f64, a0: f64, a1: f64) -> Vec<Node> {
    let n = ((a1 - a0).abs() / std::f64::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    let step = (a1 - a0) / n as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan() * r;
    let at = |a: f64| Pt::new(c.x + r * a.cos(), c.y + r * a.sin());
    let tan = |a: f64| Pt::new(-a.sin(), a.cos());
    let mut nodes: Vec<Node> = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let a = a0 + step * i as f64;
        let p = at(a);
        let t = tan(a);
        nodes.push(Node {
            r: 0.0,
            p,
            h_in: if i == 0 {
                p
            } else {
                Pt::new(p.x - t.x * k, p.y - t.y * k)
            },
            h_out: if i == n {
                p
            } else {
                Pt::new(p.x + t.x * k, p.y + t.y * k)
            },
        });
    }
    nodes
}

/// Kreisstück (oder Ringstück mit Innenradius `ri`).
fn slice(c: Pt, r: f64, ri: f64, a0: f64, a1: f64) -> Shape {
    let full = (a1 - a0).abs() >= std::f64::consts::TAU - 1e-9;
    let mut subpaths = Vec::new();
    if full {
        // ganzer Kreis bzw. Ring: zwei geschlossene Kreise (Ring: gerade-ungerade)
        let mut outer = arc_nodes(c, r, a0, a0 + std::f64::consts::TAU);
        outer.pop();
        if let Some(first) = outer.first_mut() {
            first.h_in = arc_in(c, r, a0);
        }
        subpaths.push(SubPath {
            nodes: outer,
            closed: true,
        });
        if ri > 0.0 {
            let mut inner = arc_nodes(c, ri, a0, a0 + std::f64::consts::TAU);
            inner.pop();
            if let Some(first) = inner.first_mut() {
                first.h_in = arc_in(c, ri, a0);
            }
            subpaths.push(SubPath {
                nodes: inner,
                closed: true,
            });
        }
        return Shape::Path {
            subpaths,
            evenodd: true,
        };
    }
    let mut nodes = arc_nodes(c, r, a0, a1);
    if ri > 0.0 {
        let mut inner = arc_nodes(c, ri, a1, a0);
        // Übergang außen → innen und innen → außen gerade
        if let Some(f) = inner.first_mut() {
            f.h_in = f.p;
        }
        nodes.extend(inner);
    } else {
        nodes.push(Node::corner(c));
    }
    if let Some(f) = nodes.first_mut() {
        f.h_in = f.p;
    }
    if let Some(l) = nodes.last_mut() {
        l.h_out = l.p;
    }
    subpaths.push(SubPath { nodes, closed: true });
    Shape::Path {
        subpaths,
        evenodd: false,
    }
}

/// Eingehender Anfasser am Anfang eines geschlossenen Vollkreises (Viertelbogen davor).
fn arc_in(c: Pt, r: f64, a: f64) -> Pt {
    let k = 4.0 / 3.0 * (std::f64::consts::FRAC_PI_2 / 4.0).tan() * r;
    let p = Pt::new(c.x + r * a.cos(), c.y + r * a.sin());
    Pt::new(p.x + a.sin() * k, p.y - a.cos() * k)
}

/// „Schöne“ Schrittweite für eine Achse (1, 2, 2,5, 5 × 10ⁿ).
fn nice_step(range: f64, ticks: f64) -> f64 {
    if range <= 0.0 || !range.is_finite() {
        return 1.0;
    }
    let raw = range / ticks.max(1.0);
    let mag = 10f64.powf(raw.log10().floor());
    let f = raw / mag;
    let nice = if f <= 1.0 {
        1.0
    } else if f <= 2.0 {
        2.0
    } else if f <= 2.5 {
        2.5
    } else if f <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * mag
}

/// Teile eines Objekts aus Daten im Rahmen `frame` (ohne den Rahmen selbst).
pub fn build(smart: &Smart, frame: Rect, comma: bool) -> Vec<Part> {
    build_parts(smart, frame, comma)
        .into_iter()
        .map(|(p, _)| p)
        .collect()
}

/// Wie [`build`], mit der Rolle jedes Teils (gleiche Reihenfolge).
pub fn build_parts(smart: &Smart, frame: Rect, comma: bool) -> Vec<(Part, Role)> {
    match smart {
        Smart::Table(t) => build_table(t, frame, comma),
        Smart::Chart(c) => build_chart(c, frame, comma),
    }
}

fn build_table(t: &Table, frame: Rect, comma: bool) -> Vec<(Part, Role)> {
    let (xs, ys) = t.grid(frame);
    let (nr, nc) = (t.n_rows(), t.n_cols());
    let vals = evaluate(&t.cells, comma);
    let looks = t.looks(comma);
    // Fläche einer Zelle (verbundene: die ganze Verbindung); verdeckte Zellen: keine
    let area = |r: usize, c: usize| -> Option<Rect> {
        match t.merge_at(r, c) {
            Some(m) if (m.0, m.1) != (r, c) => None,
            Some(m) => Some(Rect::from_xywh(
                xs[m.1],
                ys[m.0],
                xs[m.3 + 1] - xs[m.1],
                ys[m.2 + 1] - ys[m.0],
            )),
            None => Some(Rect::from_xywh(
                xs[c],
                ys[r],
                xs[c + 1] - xs[c],
                ys[r + 1] - ys[r],
            )),
        }
    };
    // Kästen der Zellen: erst die mit der üblichen Kontur, darüber die abweichenden (deren
    // Linien sollen ganz zu sehen sein)
    let mut plain = Vec::new();
    let mut special = Vec::new();
    for r in 0..nr {
        for c in 0..nc {
            let l = &looks[r][c];
            let Some(b) = area(r, c) else { continue };
            if l.fill.is_none() && (l.stroke.is_none() || l.width <= 0.0) {
                continue;
            }
            let mut st = painted(l);
            st.join = LineJoin::Miter;
            st.cap = LineCap::Square;
            let part = ((rect(b.x0, b.y0, b.width(), b.height()), st), Role::Cell(r, c));
            if l.stroke == t.border && l.width == t.border_width {
                plain.push(part);
            } else {
                special.push(part);
            }
        }
    }
    let mut out = plain;
    out.extend(special);
    // Texte (auch leere Zellen bekommen ein Textfeld: dort wird getippt)
    for r in 0..nr {
        for c in 0..nc {
            let l = &looks[r][c];
            let Some(b) = area(r, c) else { continue };
            let text = format_value(&vals[r][c], &l.number, &t.cells[r][c], comma);
            let part = match l.vertical {
                // senkrecht: Textfeld quer liegend gebaut, dann gedreht
                1 | 2 => {
                    let ((mut shape, st), role) = (
                        label(
                            &text,
                            Rect::from_xywh(0.0, 0.0, b.height(), b.width()),
                            l,
                            l.size * 0.45,
                        ),
                        Role::CellText(r, c),
                    );
                    if let Shape::TextArea { xf, .. } = &mut shape {
                        *xf = if l.vertical == 1 {
                            Affine::translate(b.x0, b.y1).then_before(&Affine::rotate_deg(-90.0))
                        } else {
                            Affine::translate(b.x1, b.y0).then_before(&Affine::rotate_deg(90.0))
                        };
                    }
                    ((shape, st), role)
                }
                _ => (label(&text, b, l, l.size * 0.45), Role::CellText(r, c)),
            };
            out.push(part);
        }
    }
    out
}

fn build_chart(ch: &Chart, frame: Rect, comma: bool) -> Vec<(Part, Role)> {
    let mut out: Vec<(Part, Role)> = Vec::new();
    let (labels, names, series) = ch.series(comma);
    let lab = ch.labels_look();
    let s = lab.size;
    if labels.is_empty() || series.is_empty() {
        out.push((
            label(
                tr("No data"),
                frame,
                &sized(&lab, 1.0, Align::Center, VAlign::Middle),
                0.0,
            ),
            Role::Other,
        ));
        return out;
    }
    let pie = ch.pie();
    // Legende: beim Kreis die Stücke, sonst die Reihen (bei nur einer Reihe keine)
    let entries: Vec<(String, Option<Color>, Role)> = if pie {
        labels
            .iter()
            .enumerate()
            .map(|(i, l)| (l.clone(), ch.point_look(0, i).fill, Role::Point(0, i)))
            .collect()
    } else if names.len() > 1 {
        names
            .iter()
            .enumerate()
            .map(|(i, l)| (l.clone(), ch.series_look(i).fill, Role::Series(i)))
            .collect()
    } else {
        Vec::new()
    };
    let mut plot = frame;
    if ch.legend && !entries.is_empty() {
        let row = s * 1.6;
        let side = frame.width() >= frame.height() * 1.25;
        let swatch = |x: f64, y: f64, c: Option<Color>, role: Role, out: &mut Vec<(Part, Role)>| {
            let st = Style {
                fill: c,
                stroke: None,
                ..Style::default()
            };
            out.push(((rect(x, y + (row - s) / 2.0, s, s), st), role));
        };
        if side {
            // rechts, untereinander
            let lw = (frame.width() * 0.32).min(s * 12.0);
            plot = Rect::from_xywh(frame.x0, frame.y0, frame.width() - lw, frame.height());
            let top = frame.y0 + (frame.height() - row * entries.len() as f64).max(0.0) / 2.0;
            for (i, (name, c, role)) in entries.iter().enumerate() {
                let y = top + row * i as f64;
                let x = plot.x1 + s * 0.6;
                // rechtsbündig: Text zuerst, Farbe dahinter
                let k = TextKey::Legend(i);
                let tl = ch.text_look(k);
                let tw = lw - s * 2.1;
                let (sx, tx) = if tl.align == Align::Right {
                    (x + tw + s * 0.5, x)
                } else {
                    (x, x + s * 1.5)
                };
                swatch(sx, y, *c, *role, &mut out);
                let b = Rect::from_xywh(tx, y, tw, row);
                out.push((label(name, b, &tl, 0.0), Role::Text(k)));
            }
        } else {
            // unten, nebeneinander (gleich breite Spalten)
            let n = entries.len();
            let per_row = ((frame.width() / (s * 9.0)).floor() as usize).clamp(1, n);
            let rows = n.div_ceil(per_row);
            let lh = row * rows as f64;
            plot = Rect::from_xywh(frame.x0, frame.y0, frame.width(), frame.height() - lh - s * 0.5);
            let cw = frame.width() / per_row as f64;
            for (i, (name, c, role)) in entries.iter().enumerate() {
                let x = frame.x0 + cw * (i % per_row) as f64;
                let y = plot.y1 + s * 0.5 + row * (i / per_row) as f64;
                let k = TextKey::Legend(i);
                let tl = ch.text_look(k);
                let tw = cw - s * 1.7;
                let (sx, tx) = if tl.align == Align::Right {
                    (x + tw + s * 0.2, x)
                } else {
                    (x, x + s * 1.5)
                };
                swatch(sx, y, *c, *role, &mut out);
                let b = Rect::from_xywh(tx, y, tw, row);
                out.push((label(name, b, &tl, 0.0), Role::Text(k)));
            }
        }
    }
    if plot.width() <= s || plot.height() <= s {
        return out;
    }
    if pie {
        let vals: Vec<f64> = series[0].iter().map(|v| v.max(0.0)).collect();
        let total: f64 = vals.iter().sum();
        let c = Pt::new((plot.x0 + plot.x1) / 2.0, (plot.y0 + plot.y1) / 2.0);
        let r = (plot.width().min(plot.height()) / 2.0 - s * 0.3).max(1.0);
        let ri = if ch.kind == ChartKind::Donut {
            r * 0.55
        } else {
            0.0
        };
        if total <= 0.0 {
            return out;
        }
        let mut a = -std::f64::consts::FRAC_PI_2;
        let mut texts = Vec::new();
        for (i, v) in vals.iter().enumerate() {
            if *v <= 0.0 {
                continue;
            }
            let sweep = v / total * std::f64::consts::TAU;
            let pl = ch.point_look(0, i);
            out.push(((slice(c, r, ri, a, a + sweep), painted(&pl)), Role::Point(0, i)));
            if ch.values && sweep > 0.25 {
                let k = TextKey::Value(0, i);
                let vl = ch.text_look(k);
                let vs = vl.size;
                let mid = a + sweep / 2.0;
                let rr = if ri > 0.0 { (r + ri) / 2.0 } else { r * 0.62 };
                let p = Pt::new(c.x + rr * mid.cos(), c.y + rr * mid.sin());
                let pct = format_number((v / total * 1000.0).round() / 10.0, comma);
                let t = if comma {
                    format!("{pct} %")
                } else {
                    format!("{pct}%")
                };
                let b = Rect::from_xywh(p.x - vs * 3.0, p.y - vs * 0.8, vs * 6.0, vs * 1.6);
                texts.push((label(&t, b, &vl, 0.0), Role::Text(k)));
            }
            a += sweep;
        }
        out.extend(texts);
        return out;
    }
    // Achsen: Werte links, Beschriftungen unten
    let all: Vec<f64> = series.iter().flatten().copied().collect();
    let lo = all.iter().copied().fold(0.0, f64::min);
    let hi = all.iter().copied().fold(0.0, f64::max);
    let step = nice_step(hi - lo, 5.0);
    let lo = (lo / step).floor() * step;
    let hi = ((hi / step).ceil() * step).max(lo + step);
    // Achsentitel: links (gedreht) bzw. unter den Beschriftungen
    let x_title = ch.x_axis_title(comma);
    let y_title = ch.y_title.clone();
    let yt_w = if y_title.trim().is_empty() { 0.0 } else { s * 1.8 };
    let xt_h = if x_title.trim().is_empty() { 0.0 } else { s * 1.8 };
    let tick_w = s * 3.6;
    let pa = Rect::from_xywh(
        plot.x0 + yt_w + tick_w,
        plot.y0 + s * 0.6,
        plot.width() - yt_w - tick_w - s * 0.4,
        plot.height() - s * 2.4 - xt_h,
    );
    if pa.width() <= 0.0 || pa.height() <= 0.0 {
        return out;
    }
    let ymap = |v: f64| pa.y1 - (v - lo) / (hi - lo) * pa.height();
    let grid_col = Color::rgb(0.8509803921568627, 0.8588235294117647, 0.8784313725490196);
    let mut grid = Vec::new();
    let mut k = 0;
    loop {
        let v = lo + step * k as f64;
        if v > hi + step * 1e-6 || k > 50 {
            break;
        }
        let y = ymap(v);
        grid.push((Pt::new(pa.x0, y), Pt::new(pa.x1, y)));
        let b = Rect::from_xywh(plot.x0 + yt_w, y - s * 0.8, tick_w - s * 0.5, s * 1.6);
        let tk = TextKey::Tick(k);
        out.push((
            label(&format_number(v, comma), b, &ch.text_look(tk), 0.0),
            Role::Text(tk),
        ));
        k += 1;
    }
    out.insert(
        0,
        (
            (line_path(&grid), stroked(grid_col, (s * 0.06).max(0.5))),
            Role::Other,
        ),
    );
    // Nulllinie kräftiger
    let zero = ymap(0.0);
    out.push((
        (
            line_path(&[(Pt::new(pa.x0, zero), Pt::new(pa.x1, zero))]),
            stroked(
                Color::rgb(0.45098039215686275, 0.4588235294117647, 0.47843137254901963),
                (s * 0.08).max(0.75),
            ),
        ),
        Role::Other,
    ));
    let n = labels.len();
    let slot = pa.width() / n as f64;
    for (i, l) in labels.iter().enumerate() {
        let b = Rect::from_xywh(pa.x0 + slot * i as f64, pa.y1 + s * 0.2, slot, s * 1.6);
        let k = TextKey::Category(i);
        out.push((label(l, b, &ch.text_look(k), 0.0), Role::Text(k)));
    }
    if xt_h > 0.0 {
        let b = Rect::from_xywh(pa.x0, pa.y1 + s * 1.9, pa.width(), xt_h);
        let k = TextKey::XTitle;
        out.push((label(&x_title, b, &ch.text_look(k), 0.0), Role::Text(k)));
    }
    if yt_w > 0.0 {
        // um −90° gedreht, von unten nach oben zu lesen, mittig neben der Achse
        let k = TextKey::YTitle;
        let ((mut shape, st), role) = (
            label(
                &y_title,
                Rect::from_xywh(0.0, 0.0, pa.height(), yt_w),
                &ch.text_look(k),
                0.0,
            ),
            Role::Text(k),
        );
        if let Shape::TextArea { xf, .. } = &mut shape {
            *xf = Affine::translate(plot.x0, pa.y1).then_before(&Affine::rotate_deg(-90.0));
        }
        out.push(((shape, st), role));
    }
    match ch.kind {
        ChartKind::Bar => {
            let group = slot * 0.72;
            let bw = group / series.len() as f64;
            for (si, vals) in series.iter().enumerate() {
                for (i, v) in vals.iter().enumerate() {
                    let x = pa.x0 + slot * i as f64 + (slot - group) / 2.0 + bw * si as f64;
                    let (y0, y1) = (ymap(v.max(0.0)), ymap(v.min(0.0)));
                    let pl = ch.point_look(si, i);
                    let mut st = painted(&pl);
                    st.join = LineJoin::Miter;
                    out.push(((rect(x, y0, bw * 0.92, y1 - y0), st), Role::Point(si, i)));
                    if ch.values {
                        let k = TextKey::Value(si, i);
                        let vl = ch.text_look(k);
                        let vs = vl.size;
                        let ty = if *v >= 0.0 { y0 - vs * 1.9 } else { y1 };
                        let b = Rect::from_xywh(x - s, ty, bw * 0.92 + s * 2.0, vs * 1.9);
                        out.push((label(&format_number(*v, comma), b, &vl, 0.0), Role::Text(k)));
                    }
                }
            }
        }
        _ => {
            for (si, vals) in series.iter().enumerate() {
                let sl = ch.series_look(si);
                let pts: Vec<Pt> = vals
                    .iter()
                    .enumerate()
                    .map(|(i, v)| Pt::new(pa.x0 + slot * (i as f64 + 0.5), ymap(*v)))
                    .collect();
                let mut st = Style {
                    fill: None,
                    ..painted(&sl)
                };
                st.cap = LineCap::Round;
                out.push((
                    (
                        Shape::Path {
                            subpaths: vec![SubPath {
                                nodes: pts.iter().map(|p| Node::corner(*p)).collect(),
                                closed: false,
                            }],
                            evenodd: false,
                        },
                        st,
                    ),
                    Role::Series(si),
                ));
                let dot = (s * 0.3).max(1.5);
                for (i, (p, v)) in pts.iter().zip(vals).enumerate() {
                    let pl = ch.point_look(si, i);
                    out.push((
                        (
                            Shape::Ellipse {
                                cx: p.x,
                                cy: p.y,
                                rx: dot,
                                ry: dot,
                            },
                            painted(&pl),
                        ),
                        Role::Point(si, i),
                    ));
                    if ch.values {
                        let k = TextKey::Value(si, i);
                        let vl = ch.text_look(k);
                        let vs = vl.size;
                        let b = Rect::from_xywh(p.x - vs * 3.0, p.y - dot - vs * 1.9, vs * 6.0, vs * 1.9);
                        out.push((label(&format_number(*v, comma), b, &vl, 0.0), Role::Text(k)));
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_survives_saving() {
        let mut t = Table::sample();
        t.cells[1][0] = "a|b;c=d%e^f\ng".into();
        t.align[1] = Some(Align::Right);
        t.fmt[2][1] = Fmt {
            fill: Some(None),
            stroke: Some(Some(Color {
                a: 0.5,
                ..Color::rgb(1.0, 0.0, 0.0)
            })),
            width: Some(2.5),
            family: Some("DejaVu, Serif: x".into()),
            bold: Some(true),
            color: Some(Color::rgb(0.0, 0.0, 1.0)),
            align: Some(Align::Center),
            valign: Some(VAlign::Bottom),
            ..Fmt::default()
        };
        t.fmt[3][0] = t.fmt[2][1].clone();
        // Farben werden als #rrggbb gespeichert: verglichen wird die gespeicherte Form
        let s = Smart::Table(t.clone());
        let back = Smart::decode(&s.encode()).unwrap();
        assert_eq!(back.encode(), s.encode());
        let Smart::Table(bt) = &back else { panic!() };
        assert_eq!(bt.cells, t.cells);
        assert_eq!(bt.fmt[2][1].family.as_deref(), Some("DejaVu, Serif: x"));
        assert_eq!(bt.fmt[2][1].fill, Some(None));
        assert_eq!(bt.fmt[3][0].valign, Some(VAlign::Bottom));
        assert!(bt.fmt[1][1].is_empty());
        let mut c = Chart::sample(ChartKind::Donut);
        c.values = false;
        c.header = false;
        c.colors = vec![Color::rgb(1.0, 0.0, 0.0)];
        c.point_fmt_mut(0, 2).fill = Some(Some(Color::rgb(0.0, 1.0, 0.0)));
        c.series_fmt_mut(1).stroke = Some(None);
        c.labels_fmt.size = Some(9.0);
        c.values_fmt.italic = Some(true);
        let s = Smart::Chart(c);
        assert_eq!(Smart::decode(&s.encode()).unwrap().encode(), s.encode());
        assert_eq!(Smart::decode("v=1;type=nonsense"), None);
        // kaputte Werte: Voreinstellungen
        let Some(Smart::Table(t)) = Smart::decode("type=table;size=abc;cols=x,2;cells=1|2^3") else {
            panic!()
        };
        assert_eq!(t.size, 12.0);
        assert_eq!(t.cols, vec![2.0, 1.0]);
        assert_eq!(t.cells, vec![vec!["1", "2"], vec!["3", ""]]);
        // alte Streifen werden zu Füllungen der Zellen
        let Some(Smart::Table(t)) = Smart::decode("type=table;stripes=#ff0000;cells=a^b^c^d") else {
            panic!()
        };
        let red = Some(Some(Color::rgb(1.0, 0.0, 0.0)));
        assert_eq!(
            t.fmt.iter().map(|r| r[0].fill).collect::<Vec<_>>(),
            vec![Some(Some(PALETTE[0])), None, red, None]
        );
        // alte feste Kopfzeile: als Format der ersten Zeile, danach sehen alle Zellen gleich aus
        let Some(Smart::Table(t)) = Smart::decode("type=table;header=1;align=,right;cells=a|b^c|d") else {
            panic!()
        };
        assert!(!t.header);
        assert_eq!(t.fmt[0][0].bold, Some(true));
        assert_eq!(t.fmt[0][0].align, Some(Align::Center));
        assert_eq!(t.fmt[0][1].align, None);
        assert_eq!(t.look(0, 1, false).align, Align::Right);
        assert!(t.fmt[1][0].is_empty());
        let Some(Smart::Table(t)) = Smart::decode("type=table;header=0;cells=a^b") else {
            panic!()
        };
        assert!(t.fmt[0][0].is_empty());
    }

    #[test]
    fn cells_split_into_columns_and_rows() {
        let grid = |n: usize, m: usize| -> Vec<Vec<String>> {
            (0..n)
                .map(|r| (0..m).map(|c| format!("{r}{c}")).collect())
                .collect()
        };
        // eine einzelne Zelle: neue Spalte, die übrigen Zellen der Spalte laufen darüber
        let mut t = Table::with_cells(grid(3, 2));
        t.cells[2][1] = "=A1+B1".into();
        let g = t.split_cells((1, 0, 1, 0), true).unwrap();
        assert_eq!(g, (1, 0, 1, 1));
        assert_eq!(t.n_cols(), 3);
        assert_eq!(t.cols, vec![0.5, 0.5, 1.0]);
        assert_eq!(t.merge_at(0, 0), Some((0, 0, 0, 1)));
        assert_eq!(t.merge_at(2, 1), Some((2, 0, 2, 1)));
        assert_eq!(t.merge_at(1, 0), None);
        assert_eq!(t.merge_at(1, 1), None);
        assert_eq!(t.cells[1][1], "");
        assert_eq!(t.cells[0][2], "01");
        // Bezug auf die verschobene Spalte zieht mit
        assert_eq!(t.cells[2][2], "=A1+C1");
        // untereinander
        let mut t = Table::with_cells(grid(2, 3));
        let g = t.split_cells((0, 1, 0, 1), false).unwrap();
        assert_eq!(g, (0, 1, 1, 1));
        assert_eq!(t.n_rows(), 3);
        assert_eq!(t.merge_at(0, 0), Some((0, 0, 1, 0)));
        assert_eq!(t.merge_at(1, 2), Some((0, 2, 1, 2)));
        assert_eq!(t.merge_at(0, 1), None);
        assert_eq!(t.cells[2][2], "12");
        assert_eq!(t.rows.len(), 3);
        // verbundene Zelle: erst in Spalten, dann in Zeilen getrennt
        let mut t = Table::with_cells(grid(3, 3));
        assert!(t.merge((0, 0, 1, 1)));
        assert_eq!(t.split_cells((0, 0, 0, 0), true), Some((0, 0, 1, 1)));
        assert_eq!(t.n_cols(), 3);
        assert_eq!(t.merge_at(0, 0), Some((0, 0, 1, 0)));
        assert_eq!(t.merge_at(0, 1), Some((0, 1, 1, 1)));
        assert_eq!(t.split_cells((0, 0, 1, 1), false), Some((0, 0, 1, 1)));
        assert!(t.merges.is_empty());
        assert_eq!(t.n_rows(), 3);
        // eine senkrecht verbundene Zelle nebeneinander teilen: beide Hälften bleiben verbunden
        let mut t = Table::with_cells(grid(3, 2));
        assert!(t.merge((0, 0, 1, 0)));
        assert_eq!(t.split_cells((0, 0, 1, 0), true), Some((0, 0, 1, 1)));
        assert_eq!(t.merge_at(0, 0), Some((0, 0, 1, 0)));
        assert_eq!(t.merge_at(0, 1), Some((0, 1, 1, 1)));
        assert_eq!(t.merge_at(2, 0), Some((2, 0, 2, 1)));
    }

    #[test]
    fn tables_are_built_from_cells() {
        let t = Table::sample();
        let frame = Rect::from_xywh(10.0, 20.0, 200.0, 100.0);
        let parts = build(&Smart::Table(t.clone()), frame, false);
        let texts: Vec<(String, Rect, Align, bool)> = parts
            .iter()
            .filter_map(|(sh, _)| match sh {
                Shape::TextArea {
                    xf,
                    w,
                    h,
                    text,
                    fmt,
                    bold,
                    ..
                } => Some((
                    text.clone(),
                    Rect::from_xywh(xf.e, xf.f, *w, *h),
                    fmt.align,
                    *bold,
                )),
                _ => None,
            })
            .collect();
        // Summe gerechnet, Zahlen rechts; die erste Zeile sieht aus wie alle anderen
        let total = texts.iter().find(|t| t.0 == "245").expect("Summe");
        assert_eq!(total.2, Align::Right);
        assert_eq!(total.1, Rect::from_xywh(110.0, 100.0, 100.0, 20.0));
        let head = texts.iter().find(|t| t.0 == "Item").unwrap();
        assert!(!head.3 && head.2 == Align::Left);
        assert_eq!(texts.iter().find(|t| t.0 == "A").unwrap().2, Align::Left);
        // jede Zelle ein Kasten mit Füllung und Kontur, alle gleich
        let boxes: Vec<&Style> = parts
            .iter()
            .filter(|(s, _)| matches!(s, Shape::Rect { .. }))
            .map(|p| &p.1)
            .collect();
        assert_eq!(boxes.len(), 10);
        assert_eq!(boxes[0].fill, Some(Color::WHITE));
        assert_eq!(boxes[2].fill, Some(Color::WHITE));
        // eine Zelle anders: ihr Kasten liegt obenauf, ihr Text ist kursiv
        let mut t2 = t.clone();
        t2.fmt[1][0].stroke = Some(Some(Color::BLACK));
        t2.fmt[1][0].italic = Some(true);
        let parts = build_parts(&Smart::Table(t2), frame, false);
        let last_box = parts
            .iter()
            .filter(|(p, _)| matches!(p.0, Shape::Rect { .. }))
            .last()
            .unwrap();
        assert_eq!(last_box.1, Role::Cell(1, 0));
        assert_eq!(last_box.0 .1.stroke, Some(Color::BLACK));
        assert!(parts.iter().any(
            |((s, _), r)| *r == Role::CellText(1, 0) && matches!(s, Shape::TextArea { italic: true, .. })
        ));
        // Zelle unter einem Punkt
        assert_eq!(t.cell_at(frame, Pt::new(150.0, 45.0)), Some((1, 1)));
        assert_eq!(t.cell_at(frame, Pt::new(5.0, 45.0)), None);
    }

    #[test]
    fn charts_understand_how_tables_are_laid_out() {
        let ch = |data: &[&[&str]]| {
            let mut c = Chart::sample(ChartKind::Bar);
            c.data = data
                .iter()
                .map(|r| r.iter().map(|s| s.to_string()).collect())
                .collect();
            c
        };
        // Jahre oben, Geschlecht links, Ecke leer: zwei Reihen (M, F) über die Jahre
        let t = ch(&[
            &["", "2011", "2012", "2013", "2014"],
            &["M", "1", "1", "4", "5"],
            &["F", "1", "2", "2", "3"],
        ]);
        let (labels, names, series) = t.series(false);
        assert_eq!(labels, ["2011", "2012", "2013", "2014"]);
        assert_eq!(names, ["M", "F"]);
        assert_eq!(series, vec![vec![1.0, 1.0, 4.0, 5.0], vec![1.0, 2.0, 2.0, 3.0]]);
        assert_eq!(t.suggest_kind(false), ChartKind::Line);
        // dieselbe Tabelle gekippt: Jahre links
        let t2 = ch(&[
            &["Year", "M", "F"],
            &["2011", "1", "1"],
            &["2012", "1", "2"],
            &["2013", "4", "2"],
            &["2014", "5", "3"],
        ]);
        let (labels, names, series) = t2.series(false);
        assert_eq!(labels, ["2011", "2012", "2013", "2014"]);
        assert_eq!(names, ["M", "F"]);
        assert_eq!(series[1], vec![1.0, 2.0, 2.0, 3.0]);
        assert_eq!(t2.x_axis_title(false), "Year");
        assert_eq!(t2.suggest_kind(false), ChartKind::Line);
        // ohne Kopfzeile, nur Zahlen: eine Reihe, Punkte durchnummeriert
        let t3 = ch(&[&["3"], &["5"], &["2"]]);
        let (labels, names, series) = t3.series(false);
        assert_eq!(labels, ["1", "2", "3"]);
        assert_eq!(names.len(), 1);
        assert_eq!(series, vec![vec![3.0, 5.0, 2.0]]);
        assert_eq!(t3.suggest_kind(false), ChartKind::Pie);
        // Text-Beschriftungen und eine Zahlenspalte (Kopf mit Text in der Ecke)
        let t4 = ch(&[
            &["Fruit", "Sales"],
            &["Apples", "30"],
            &["Pears", "20"],
            &["", ""],
        ]);
        let (labels, names, series) = t4.series(false);
        assert_eq!(labels, ["Apples", "Pears"]);
        assert_eq!(names, ["Sales"]);
        assert_eq!(series, vec![vec![30.0, 20.0]]);
        // Reihen erzwingen
        let mut t5 = t.clone();
        t5.orient = Some(false);
        let (labels, names, _) = t5.series(false);
        assert_eq!(labels, ["M", "F"]);
        assert_eq!(names, ["2011", "2012", "2013", "2014"]);
        // Achsentitel werden gebaut (y gedreht) und gespeichert
        let mut t6 = t.clone();
        t6.kind = ChartKind::Line;
        t6.y_title = "Amount".into();
        t6.x_title = "Year".into();
        let parts = build_parts(
            &Smart::Chart(t6.clone()),
            Rect::from_xywh(0.0, 0.0, 400.0, 250.0),
            false,
        );
        let yt = parts
            .iter()
            .find(|(_, r)| *r == Role::Text(TextKey::YTitle))
            .unwrap();
        assert!(matches!(&yt.0 .0, Shape::TextArea { xf, text, .. } if text == "Amount" && xf.b < -0.99));
        assert!(parts.iter().any(|(_, r)| *r == Role::Text(TextKey::XTitle)));
        let back = Smart::decode(&Smart::Chart(t6.clone()).encode()).unwrap();
        assert_eq!(back, Smart::Chart(t6));
    }

    #[test]
    fn cells_fill_and_move_like_in_a_spreadsheet() {
        let mut t = Table::with_cells(vec![
            vec!["1".into(), "Q1".into(), "a".into(), "=A1*2".into()],
            vec!["3".into(), "Q2".into(), "b".into(), "=$A$1+A2".into()],
        ]);
        t.fmt[1][2].bold = Some(true);
        // zwei Zeilen weiter: Zahlen in Zweierschritten, Q zählt, Text wiederholt sich,
        // Formeln wandern mit; die Tabelle wächst
        t.fill((0, 0, 1, 3), 2, 0, false);
        assert_eq!(t.n_rows(), 4);
        let col = |t: &Table, c: usize| t.cells.iter().map(|r| r[c].clone()).collect::<Vec<_>>();
        assert_eq!(col(&t, 0), ["1", "3", "5", "7"]);
        assert_eq!(col(&t, 1), ["Q1", "Q2", "Q3", "Q4"]);
        assert_eq!(col(&t, 2), ["a", "b", "a", "b"]);
        assert_eq!(col(&t, 3), ["=A1*2", "=$A$1+A2", "=A3*2", "=$A$1+A4"]);
        assert_eq!(t.fmt[3][2].bold, Some(true));
        // eine Zahl nach rechts: in Einerschritten (Komma als Dezimalzeichen)
        let mut u = Table::with_cells(vec![vec!["1,5".into(), String::new()]]);
        u.fill((0, 0, 0, 0), 0, 3, true);
        assert_eq!(u.cells[0], ["1,5", "2,5", "3,5", "4,5"]);
        // nach oben
        let mut v = Table::with_cells(vec![
            vec![String::new()],
            vec![String::new()],
            vec!["10".into()],
            vec!["20".into()],
        ]);
        v.fill((2, 0, 3, 0), -2, 0, false);
        assert_eq!(col(&v, 0), ["-10", "0", "10", "20"]);
        // verschieben: Bezüge darauf ziehen mit
        let mut w = Table::with_cells(vec![
            vec!["5".into(), String::new(), String::new()],
            vec!["=A1+1".into(), String::new(), String::new()],
        ]);
        assert!(w.move_block((0, 0, 0, 0), 0, 2));
        assert_eq!(w.cells[0], ["", "", "5"]);
        assert_eq!(w.cells[1][0], "=C1+1");
        assert!(!w.move_block((0, 2, 0, 2), 0, 1));
    }

    #[test]
    fn rows_and_columns_change() {
        let mut t = Table::sample();
        t.insert_col(1);
        t.insert_row(0);
        assert_eq!((t.n_rows(), t.n_cols()), (6, 3));
        assert_eq!(t.cols.len(), 3);
        assert_eq!(t.align.len(), 3);
        t.remove_col(1);
        t.remove_row(0);
        assert_eq!(t, Table::sample());
        // Bezüge rücken mit, ein Bereich wird kleiner, ein entfernter Bezug ungültig
        let mut f = Table::with_cells(vec![
            vec!["1".into(), "=A2".into()],
            vec!["2".into(), "=SUM(A1:A3)".into()],
            vec!["3".into(), "=A3*2".into()],
        ]);
        f.remove_row(2);
        assert_eq!(f.cells[1][1], "=SUM(A1:A2)");
        f.remove_row(0);
        assert_eq!(f.cells[0][1], "=SUM(A1:A1)");
        let mut g = Table::with_cells(vec![vec!["1".into(), "2".into(), "=A1+B1".into()]]);
        g.remove_col(0);
        assert_eq!(g.cells[0][1], "=#REF!+A1");
        let mut one = Table::with_cells(vec![vec!["x".into()]]);
        one.remove_col(0);
        one.remove_row(0);
        assert_eq!((one.n_rows(), one.n_cols()), (1, 1));
    }

    #[test]
    fn charts_are_built() {
        let frame = Rect::from_xywh(0.0, 0.0, 300.0, 200.0);
        let pie = build(&Smart::Chart(Chart::sample(ChartKind::Pie)), frame, true);
        // vier Stücke, vier Legendenfelder und Beschriftungen, Prozente mit Komma
        let slices = pie
            .iter()
            .filter(|(s, st)| matches!(s, Shape::Path { .. }) && st.fill.is_some())
            .count();
        assert_eq!(slices, 4);
        let texts: Vec<String> = pie
            .iter()
            .filter_map(|(s, _)| s.text_fields().map(|t| t.0.to_string()))
            .collect();
        assert!(texts.contains(&"40 %".to_string()), "{texts:?}");
        assert!(texts.contains(&"D".to_string()));
        // Stücke liegen im Kreis links der Legende
        for (s, _) in pie.iter().filter(|(s, _)| matches!(s, Shape::Path { .. })) {
            let b = s.geom_bbox().unwrap();
            assert!(b.x1 <= 300.0 - 300.0 * 0.32 + 1e-6, "{b:?}");
        }
        // Balken: einer je Wert, Höhe nach Wert; Achse 0–40 in Zehnern
        let bars = build(&Smart::Chart(Chart::sample(ChartKind::Bar)), frame, false);
        let heights: Vec<f64> = bars
            .iter()
            .filter_map(|(s, _)| match s {
                Shape::Rect { h, w, .. } if *w > 12.0 => Some(*h),
                _ => None,
            })
            .collect();
        assert_eq!(heights.len(), 4);
        assert!((heights[0] / heights[3] - 40.0 / 15.0).abs() < 1e-9);
        let ticks: Vec<String> = bars
            .iter()
            .filter_map(|(s, _)| s.text_fields().map(|t| t.0.to_string()))
            .collect();
        for t in ["0", "10", "20", "30", "40"] {
            assert!(ticks.contains(&t.to_string()), "{ticks:?}");
        }
        // Linie mit zwei Reihen: zwei Linien, Legende mit beiden Namen
        let mut c = Chart::sample(ChartKind::Line);
        for (i, row) in c.data.iter_mut().enumerate() {
            row.push(if i == 0 {
                "2027".into()
            } else {
                format!("{}", i * 3)
            });
        }
        let line = build(&Smart::Chart(c), frame, false);
        let lines = line
            .iter()
            .filter(|(s, st)| matches!(s, Shape::Path { subpaths, .. } if subpaths.len() == 1 && subpaths[0].nodes.len() == 4) && st.fill.is_none())
            .count();
        assert_eq!(lines, 2);
        // erste Zeile mit Zahl statt Namen: sie zählt als Daten; Rollen der Teile
        let mut nh = Chart::sample(ChartKind::Pie);
        nh.data[0][0] = "Z".into();
        nh.data[0][1] = "10".into();
        let (labels, _, series) = nh.series(false);
        assert_eq!(labels.len(), 5);
        assert_eq!(series[0][0], 10.0);
        nh.point_fmt_mut(0, 1).fill = Some(Some(Color::BLACK));
        let parts = build_parts(&Smart::Chart(nh), frame, false);
        let p1: Vec<&Style> = parts
            .iter()
            .filter(|(_, r)| *r == Role::Point(0, 1))
            .map(|p| &p.0 .1)
            .collect();
        // Stück und Farbfeld der Legende
        assert_eq!(p1.len(), 2);
        assert!(p1.iter().all(|st| st.fill == Some(Color::BLACK)));
        assert!(parts
            .iter()
            .any(|(_, r)| matches!(r, Role::Text(TextKey::Value(0, _)))));
        // keine Daten
        let mut e = Chart::sample(ChartKind::Pie);
        e.data.truncate(1);
        let empty = build(&Smart::Chart(e), frame, false);
        assert_eq!(empty.len(), 1);
        // Ring ganz aus einem Stück
        let mut one = Chart::sample(ChartKind::Donut);
        one.data.truncate(2);
        let ring = build(&Smart::Chart(one), Rect::from_xywh(0.0, 0.0, 100.0, 100.0), false);
        assert!(ring
            .iter()
            .any(|(s, _)| matches!(s, Shape::Path { subpaths, evenodd: true } if subpaths.len() == 2)));
    }
}
