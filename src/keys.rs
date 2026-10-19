//! Tastenkürzel (ohne GTK): alle Kürzel des Programms mit ihren Standardwerten, eigene
//! Belegungen, Konfliktprüfung und Speicherformat.
//!
//! Kürzel sind Zeichenketten im GTK-Format („<Control>n“, „<Shift>t“, „F9“).

use std::collections::BTreeMap;

use crate::editor::Tool;

/// Was ein Kürzel auslöst.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// Aktion des Fensters (`win.…`), über die Anwendung als Kürzel registriert
    Action(&'static str),
    /// Werkzeug wählen (nur Taste mit optional Umschalt)
    Tool(Tool),
    /// Auswahl waagerecht bzw. senkrecht spiegeln (nur Taste mit optional Umschalt)
    Flip { horizontal: bool },
    /// feste Taste, nicht änderbar (nur zur Anzeige)
    Fixed,
}

pub struct Def {
    pub id: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub keys: &'static [&'static str],
    pub kind: Kind,
}

const fn d(
    id: &'static str,
    label: &'static str,
    group: &'static str,
    keys: &'static [&'static str],
    kind: Kind,
) -> Def {
    Def {
        id,
        label,
        group,
        keys,
        kind,
    }
}

pub const GROUP_FILE: &str = "Datei";
pub const GROUP_EDIT: &str = "Bearbeiten";
pub const GROUP_ARRANGE: &str = "Anordnen";
pub const GROUP_VIEW: &str = "Ansicht";
pub const GROUP_TOOLS: &str = "Werkzeuge";
pub const GROUP_FIXED: &str = "Feste Tasten";

pub const GROUPS: [&str; 6] = [
    GROUP_FILE,
    GROUP_EDIT,
    GROUP_ARRANGE,
    GROUP_VIEW,
    GROUP_TOOLS,
    GROUP_FIXED,
];

use Kind::{Action, Fixed, Flip};

pub const DEFS: &[Def] = &[
    d(
        "new",
        "Neues Dokument",
        GROUP_FILE,
        &["<Control>n", "<Control>t"],
        Action("win.new"),
    ),
    d("open", "Öffnen", GROUP_FILE, &["<Control>o"], Action("win.open")),
    d(
        "save",
        "Speichern",
        GROUP_FILE,
        &["<Control>s"],
        Action("win.save"),
    ),
    d(
        "save-as",
        "Speichern unter",
        GROUP_FILE,
        &["<Control><Shift>s"],
        Action("win.save-as"),
    ),
    d(
        "export",
        "Exportieren",
        GROUP_FILE,
        &["<Control>e"],
        Action("win.export"),
    ),
    d(
        "close-tab",
        "Tab schließen",
        GROUP_FILE,
        &["<Control>w"],
        Action("win.close-tab"),
    ),
    d(
        "insert-image",
        "Bild einfügen",
        GROUP_FILE,
        &["<Control>i"],
        Action("win.insert-image"),
    ),
    d(
        "preferences",
        "Einstellungen",
        GROUP_FILE,
        &["<Control>comma"],
        Action("win.preferences"),
    ),
    d(
        "undo",
        "Rückgängig",
        GROUP_EDIT,
        &["<Control>z"],
        Action("win.undo"),
    ),
    d(
        "redo",
        "Wiederholen",
        GROUP_EDIT,
        &["<Control><Shift>z", "<Control>y"],
        Action("win.redo"),
    ),
    d(
        "duplicate",
        "Duplizieren",
        GROUP_EDIT,
        &["<Control>d"],
        Action("win.duplicate"),
    ),
    d(
        "to-path",
        "Text in Pfade umwandeln",
        GROUP_EDIT,
        &["<Control><Shift>c"],
        Action("win.to-path"),
    ),
    d(
        "group",
        "Gruppieren",
        GROUP_ARRANGE,
        &["<Control>g"],
        Action("win.group"),
    ),
    d(
        "ungroup",
        "Gruppierung auflösen",
        GROUP_ARRANGE,
        &["<Control><Shift>g"],
        Action("win.ungroup"),
    ),
    d(
        "raise",
        "Schritt nach vorne",
        GROUP_ARRANGE,
        &["<Control>bracketright"],
        Action("win.raise"),
    ),
    d(
        "lower",
        "Schritt nach hinten",
        GROUP_ARRANGE,
        &["<Control>bracketleft"],
        Action("win.lower"),
    ),
    d(
        "raise-top",
        "In den Vordergrund",
        GROUP_ARRANGE,
        &["<Control><Shift>bracketright", "<Control>braceright"],
        Action("win.raise-top"),
    ),
    d(
        "lower-bottom",
        "In den Hintergrund",
        GROUP_ARRANGE,
        &["<Control><Shift>bracketleft", "<Control>braceleft"],
        Action("win.lower-bottom"),
    ),
    d(
        "flip-h",
        "Waagerecht spiegeln",
        GROUP_ARRANGE,
        &["<Shift>h"],
        Flip { horizontal: true },
    ),
    d(
        "flip-v",
        "Senkrecht spiegeln",
        GROUP_ARRANGE,
        &["<Shift>v"],
        Flip { horizontal: false },
    ),
    d(
        "zoom-in",
        "Vergrößern",
        GROUP_VIEW,
        &["<Control>plus", "<Control>equal", "<Control>KP_Add"],
        Action("win.zoom-in"),
    ),
    d(
        "zoom-out",
        "Verkleinern",
        GROUP_VIEW,
        &["<Control>minus", "<Control>KP_Subtract"],
        Action("win.zoom-out"),
    ),
    d(
        "zoom-100",
        "Originalgröße",
        GROUP_VIEW,
        &["<Control>1"],
        Action("win.zoom-100"),
    ),
    d(
        "zoom-fit",
        "Ganze Seite",
        GROUP_VIEW,
        &["<Control>0"],
        Action("win.zoom-fit"),
    ),
    d(
        "view-notebook",
        "Notizheft-Ansicht ein/aus",
        GROUP_VIEW,
        &["<Control><Shift>n"],
        Action("win.notebook"),
    ),
    d(
        "toggle-panel",
        "Eigenschaften ein-/ausblenden",
        GROUP_VIEW,
        &["F9"],
        Action("win.toggle-panel"),
    ),
    d(
        "tool-select",
        "Auswahl",
        GROUP_TOOLS,
        &["v"],
        Kind::Tool(Tool::Select),
    ),
    d(
        "tool-node",
        "Knoten bearbeiten",
        GROUP_TOOLS,
        &["a"],
        Kind::Tool(Tool::Node),
    ),
    d("tool-shape", "Form", GROUP_TOOLS, &["r"], Kind::Tool(Tool::Shape)),
    d(
        "tool-pen",
        "Zeichenstift",
        GROUP_TOOLS,
        &["p"],
        Kind::Tool(Tool::Pen),
    ),
    d(
        "tool-curvature",
        "Kurvenstift",
        GROUP_TOOLS,
        &["<Shift>p"],
        Kind::Tool(Tool::Curvature),
    ),
    d(
        "tool-pencil",
        "Bleistift",
        GROUP_TOOLS,
        &["n"],
        Kind::Tool(Tool::Pencil),
    ),
    d("tool-text", "Text", GROUP_TOOLS, &["t"], Kind::Tool(Tool::Text)),
    d(
        "tool-textpath",
        "Text auf Pfad",
        GROUP_TOOLS,
        &["<Shift>t"],
        Kind::Tool(Tool::TextPath),
    ),
    d(
        "tool-guide",
        "Hilfslinie",
        GROUP_TOOLS,
        &["g"],
        Kind::Tool(Tool::Guide),
    ),
    d(
        "tool-bucket",
        "Farbeimer",
        GROUP_TOOLS,
        &["k"],
        Kind::Tool(Tool::Bucket),
    ),
    d(
        "tool-eraser",
        "Radierer",
        GROUP_TOOLS,
        &["<Shift>e"],
        Kind::Tool(Tool::Eraser),
    ),
    d(
        "tool-builder",
        "Formerstellung",
        GROUP_TOOLS,
        &["<Shift>m"],
        Kind::Tool(Tool::Builder),
    ),
    d(
        "fixed-delete",
        "Auswahl löschen",
        GROUP_FIXED,
        &["Delete", "BackSpace"],
        Fixed,
    ),
    d(
        "fixed-escape",
        "Abbrechen, Auswahl aufheben",
        GROUP_FIXED,
        &["Escape"],
        Fixed,
    ),
    d(
        "fixed-finish",
        "Zeichnen beenden",
        GROUP_FIXED,
        &["Return"],
        Fixed,
    ),
    d(
        "fixed-nudge",
        "Auswahl verschieben (mit Umschalt: 10 px)",
        GROUP_FIXED,
        &["Left", "Right", "Up", "Down"],
        Fixed,
    ),
    d(
        "fixed-pan",
        "Ansicht verschieben (gedrückt halten, ziehen)",
        GROUP_FIXED,
        &["space"],
        Fixed,
    ),
    d(
        "fixed-pages",
        "Nächste/vorige Seite (Notizheft)",
        GROUP_FIXED,
        &["Page_Down", "Page_Up"],
        Fixed,
    ),
    d("fixed-cut", "Ausschneiden", GROUP_FIXED, &["<Control>x"], Fixed),
    d("fixed-copy", "Kopieren", GROUP_FIXED, &["<Control>c"], Fixed),
    d("fixed-paste", "Einfügen", GROUP_FIXED, &["<Control>v"], Fixed),
    d(
        "fixed-all",
        "Alles auswählen",
        GROUP_FIXED,
        &["<Control>a"],
        Fixed,
    ),
    d("fixed-tab", "Tabulator im Text", GROUP_FIXED, &["Tab"], Fixed),
    d(
        "fixed-newline",
        "Neue Zeile im Absatz (Textfeld)",
        GROUP_FIXED,
        &["<Shift>Return"],
        Fixed,
    ),
];

pub fn def(id: &str) -> Option<&'static Def> {
    DEFS.iter().find(|d| d.id == id)
}

/// Eigene Belegungen: Kennung → Kürzel (leer = abgeschaltet). Fehlt eine Kennung, gilt
/// der Standard.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Keymap {
    custom: BTreeMap<String, Vec<String>>,
}

/// Auf Groß-/Kleinschreibung und Reihenfolge der Modifikatoren vereinheitlichen,
/// damit „<Shift><Control>N“ und „<Control><Shift>n“ gleich sind.
pub fn normalize(accel: &str) -> String {
    let mut mods: Vec<&str> = Vec::new();
    let mut rest = accel.trim();
    while let Some(r) = rest.strip_prefix('<') {
        let Some(end) = r.find('>') else { break };
        mods.push(match &r[..end].to_ascii_lowercase()[..] {
            "control" | "ctrl" | "primary" => "<Control>",
            "shift" => "<Shift>",
            "alt" | "mod1" => "<Alt>",
            "super" | "mod4" => "<Super>",
            _ => "",
        });
        rest = &r[end + 1..];
    }
    mods.retain(|m| !m.is_empty());
    mods.sort_by_key(|m| match *m {
        "<Control>" => 0,
        "<Shift>" => 1,
        "<Alt>" => 2,
        _ => 3,
    });
    mods.dedup();
    // einzelne Buchstaben klein, benannte Tasten unverändert
    let key = if rest.chars().count() == 1 {
        rest.to_lowercase()
    } else {
        rest.to_string()
    };
    format!("{}{key}", mods.concat())
}

impl Keymap {
    /// Aktuelle Kürzel einer Kennung (Standard oder eigene Belegung).
    pub fn keys(&self, id: &str) -> Vec<String> {
        if let Some(c) = self.custom.get(id) {
            return c.clone();
        }
        def(id)
            .map(|d| d.keys.iter().map(|k| normalize(k)).collect())
            .unwrap_or_default()
    }

    pub fn is_custom(&self, id: &str) -> bool {
        self.custom.contains_key(id)
    }

    pub fn any_custom(&self) -> bool {
        !self.custom.is_empty()
    }

    /// Eigene Belegung setzen; entspricht sie dem Standard, wird sie zurückgenommen.
    pub fn set(&mut self, id: &str, keys: Vec<String>) {
        let keys: Vec<String> = keys.iter().map(|k| normalize(k)).collect();
        let default: Vec<String> = def(id)
            .map(|d| d.keys.iter().map(|k| normalize(k)).collect())
            .unwrap_or_default();
        if keys == default {
            self.custom.remove(id);
        } else {
            self.custom.insert(id.to_string(), keys);
        }
    }

    pub fn reset(&mut self, id: &str) {
        self.custom.remove(id);
    }

    pub fn reset_all(&mut self) {
        self.custom.clear();
    }

    /// Wer belegt dieses Kürzel schon (außer `except`)?
    pub fn owner(&self, accel: &str, except: &str) -> Option<&'static Def> {
        let a = normalize(accel);
        DEFS.iter()
            .filter(|d| d.id != except)
            .find(|d| self.keys(d.id).contains(&a))
    }

    /// Kennung zu einem gedrückten Kürzel (für Werkzeuge und Spiegeln auf der Zeichenfläche).
    pub fn find_canvas(&self, accel: &str) -> Option<&'static Def> {
        let a = normalize(accel);
        DEFS.iter()
            .filter(|d| matches!(d.kind, Kind::Tool(_) | Kind::Flip { .. }))
            .find(|d| self.keys(d.id).contains(&a))
    }

    /// Erstes Kürzel einer Fensteraktion (für Anzeige in Menüs).
    pub fn accel_for_action(&self, action: &str) -> Option<String> {
        DEFS.iter()
            .find(|d| matches!(d.kind, Action(a) if a == action))
            .and_then(|d| self.keys(d.id).into_iter().next())
    }

    /// Speicherformat: je Zeile „kennung=kürzel|kürzel“ (nach `=` leer = abgeschaltet).
    pub fn to_text(&self) -> String {
        self.custom
            .iter()
            .map(|(k, v)| format!("{k}={}\n", v.join("|")))
            .collect()
    }

    pub fn from_text(text: &str) -> Keymap {
        let mut m = Keymap::default();
        for line in text.lines() {
            let Some((id, v)) = line.split_once('=') else {
                continue;
            };
            let id = id.trim();
            // nur bekannte, änderbare Kennungen
            if !def(id).is_some_and(|d| d.kind != Fixed) {
                continue;
            }
            let keys: Vec<String> = v
                .split('|')
                .map(str::trim)
                .filter(|k| !k.is_empty())
                .map(normalize)
                .collect();
            m.set(id, keys);
        }
        m
    }
}

/// Ist das Kürzel für diese Art zulässig? `Err` enthält den Grund.
pub fn allowed(kind: Kind, accel: &str) -> Result<(), &'static str> {
    let a = normalize(accel);
    let has_ctrl_or_alt = a.contains("<Control>") || a.contains("<Alt>");
    let key = a.rsplit('>').next().unwrap_or("");
    match kind {
        Kind::Fixed => Err("Diese Taste ist fest."),
        Action(_) => {
            let fkey = key.len() >= 2 && key.starts_with('F') && key[1..].chars().all(|c| c.is_ascii_digit());
            if has_ctrl_or_alt || fkey {
                Ok(())
            } else {
                Err("Kürzel brauchen Strg oder Alt (oder eine F-Taste), sonst stören sie die Texteingabe.")
            }
        }
        Kind::Tool(_) | Kind::Flip { .. } => {
            if has_ctrl_or_alt {
                Err("Werkzeugtasten gehen nur als einzelne Taste, mit oder ohne Umschalt.")
            } else if key.chars().count() == 1 {
                Ok(())
            } else {
                Err("Hier ist ein Buchstabe, eine Ziffer oder ein Zeichen nötig.")
            }
        }
    }
}

/// Lesbarer Text eines Kürzels ohne GTK: „Strg+Umschalt+S“.
pub fn label(accel: &str) -> String {
    let a = normalize(accel);
    let mut out: Vec<String> = Vec::new();
    let mut rest = a.as_str();
    while let Some(r) = rest.strip_prefix('<') {
        let Some(end) = r.find('>') else { break };
        out.push(
            match &r[..end] {
                "Control" => "Strg",
                "Shift" => "Umschalt",
                "Alt" => "Alt",
                "Super" => "Super",
                o => o,
            }
            .to_string(),
        );
        rest = &r[end + 1..];
    }
    out.push(match rest {
        "bracketright" => "]".into(),
        "bracketleft" => "[".into(),
        "braceright" => "}".into(),
        "braceleft" => "{".into(),
        "comma" => ",".into(),
        "plus" => "+".into(),
        "minus" => "−".into(),
        "equal" => "=".into(),
        "KP_Add" => "Num +".into(),
        "KP_Subtract" => "Num −".into(),
        "space" => "Leertaste".into(),
        "Delete" => "Entf".into(),
        "BackSpace" => "⌫".into(),
        "Escape" => "Esc".into(),
        "Return" => "Enter".into(),
        "Left" => "←".into(),
        "Right" => "→".into(),
        "Up" => "↑".into(),
        "Down" => "↓".into(),
        "Tab" => "Tab".into(),
        "Page_Down" => "Bild ↓".into(),
        "Page_Up" => "Bild ↑".into(),
        k if k.chars().count() == 1 => k.to_uppercase(),
        k => k.to_string(),
    });
    out.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_no_conflicts() {
        let m = Keymap::default();
        for d in DEFS {
            for k in m.keys(d.id) {
                let other = m.owner(&k, d.id);
                assert!(other.is_none(), "{k}: {} und {}", d.id, other.unwrap().id);
            }
        }
    }

    #[test]
    fn ids_are_unique_and_groups_known() {
        for (i, a) in DEFS.iter().enumerate() {
            assert!(GROUPS.contains(&a.group), "{}", a.id);
            assert!(DEFS[i + 1..].iter().all(|b| b.id != a.id), "{}", a.id);
        }
    }

    #[test]
    fn normalizing_is_stable() {
        assert_eq!(normalize("<Shift><Control>N"), "<Control><Shift>n");
        assert_eq!(normalize("<Primary>s"), "<Control>s");
        assert_eq!(normalize("F9"), "F9");
        assert_eq!(normalize("<Control>comma"), "<Control>comma");
    }

    #[test]
    fn custom_keys_replace_and_reset() {
        let mut m = Keymap::default();
        assert_eq!(m.keys("save"), vec!["<Control>s"]);
        m.set("save", vec!["<Control><Alt>s".into()]);
        assert!(m.is_custom("save"));
        assert_eq!(m.keys("save"), vec!["<Control><Alt>s"]);
        // die alte Belegung ist frei, die neue besetzt
        assert!(m.owner("<Control>s", "export").is_none());
        assert_eq!(m.owner("<Control><Alt>s", "export").map(|d| d.id), Some("save"));
        // zurück auf den Standard
        m.set("save", vec!["<Control>s".into()]);
        assert!(!m.is_custom("save"));
        m.set("save", vec![]);
        assert!(m.keys("save").is_empty());
        m.reset("save");
        assert_eq!(m.keys("save"), vec!["<Control>s"]);
    }

    #[test]
    fn text_roundtrip_and_unknown_ids() {
        let mut m = Keymap::default();
        m.set("tool-pen", vec!["b".into()]);
        m.set("undo", vec!["<Control>z".into(), "<Control><Alt>u".into()]);
        m.set("zoom-fit", vec![]);
        let t = m.to_text();
        let back = Keymap::from_text(&format!("{t}bogus=x\nfixed-delete=q\n"));
        assert_eq!(back, m);
        assert_eq!(back.keys("zoom-fit"), Vec::<String>::new());
        assert_eq!(back.find_canvas("b").map(|d| d.id), Some("tool-pen"));
        assert!(back.find_canvas("p").is_none());
        assert_eq!(back.keys("fixed-delete"), vec!["Delete", "BackSpace"]);
    }

    #[test]
    fn canvas_lookup_and_action_accel() {
        let m = Keymap::default();
        assert_eq!(m.find_canvas("<Shift>t").map(|d| d.id), Some("tool-textpath"));
        assert_eq!(m.find_canvas("v").map(|d| d.id), Some("tool-select"));
        assert_eq!(m.find_canvas("<Shift>h").map(|d| d.id), Some("flip-h"));
        assert_eq!(m.accel_for_action("win.undo").as_deref(), Some("<Control>z"));
        assert_eq!(m.accel_for_action("win.nope"), None);
    }

    #[test]
    fn allowed_rules() {
        assert!(allowed(Action("win.save"), "<Control>s").is_ok());
        assert!(allowed(Action("win.save"), "F5").is_ok());
        assert!(allowed(Action("win.save"), "s").is_err());
        assert!(allowed(Kind::Tool(Tool::Pen), "b").is_ok());
        assert!(allowed(Kind::Tool(Tool::Pen), "<Shift>b").is_ok());
        assert!(allowed(Kind::Tool(Tool::Pen), "<Control>b").is_err());
        assert!(allowed(Kind::Tool(Tool::Pen), "F5").is_err());
        assert!(allowed(Fixed, "x").is_err());
    }

    #[test]
    fn labels_are_readable() {
        assert_eq!(label("<Control><Shift>s"), "Strg+Umschalt+S");
        assert_eq!(label("<Control>bracketright"), "Strg+]");
        assert_eq!(label("F9"), "F9");
        assert_eq!(label("space"), "Leertaste");
    }
}
