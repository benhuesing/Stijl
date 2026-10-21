// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! Nach neuen Versionen suchen (GitHub Releases).
//!
//! Gefragt wird nur nach der neuesten veröffentlichten Version; es wird nichts
//! heruntergeladen oder installiert. Ist eine neuere Version da, öffnet das Programm auf
//! Wunsch die Release-Seite im Browser. Die Abfrage läuft über das `curl` des Systems
//! (Windows 10/11 und macOS bringen es mit), damit Stijl keine eigene TLS-Bibliothek braucht.
//! Unter Flatpak und Snap kümmert sich die Paketverwaltung um Updates.

use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Repository auf GitHub (Besitzer/Name).
pub const REPO: &str = "benhuesing/Stijl";

/// Abstand zwischen zwei automatischen Abfragen.
pub const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Eine veröffentlichte Version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Versionsnummer ohne „v“, z. B. „0.9.40“
    pub version: String,
    /// Seite mit Downloads und Änderungen
    pub url: String,
}

/// Ist die Suche nach Updates standardmäßig eingeschaltet? Unter Windows und macOS ja;
/// unter Linux kommen Updates meist über die Paketverwaltung.
pub fn enabled_by_default() -> bool {
    cfg!(any(windows, target_os = "macos"))
}

/// Läuft das Programm als Flatpak oder Snap? Dann gibt es keine eigene Update-Suche.
pub fn managed_externally() -> bool {
    std::env::var_os("FLATPAK_ID").is_some() || std::env::var_os("SNAP").is_some()
}

/// Sekunden seit 1970.
pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Ist wieder eine automatische Abfrage fällig? (Auch wenn die Uhr zurückgestellt wurde.)
pub fn due(last: i64, now: i64) -> bool {
    last <= 0 || now < last || now - last >= INTERVAL.as_secs() as i64
}

/// Versionsnummer in Zahlen: „v0.9.40“ → [0, 9, 40]. Zusätze wie „-beta“ werden
/// abgeschnitten.
pub fn parse_version(s: &str) -> Option<Vec<u64>> {
    let s = s.trim();
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    let core = s.split(['-', '+']).next().unwrap_or("");
    if core.is_empty() {
        return None;
    }
    core.split('.').map(|p| p.parse::<u64>().ok()).collect()
}

/// Ist `remote` neuer als `local`? (Fehlende Stellen zählen als 0.)
pub fn is_newer(remote: &str, local: &str) -> bool {
    let (Some(r), Some(l)) = (parse_version(remote), parse_version(local)) else {
        return false;
    };
    let n = r.len().max(l.len());
    let at = |v: &[u64], i: usize| v.get(i).copied().unwrap_or(0);
    for i in 0..n {
        match at(&r, i).cmp(&at(&l, i)) {
            std::cmp::Ordering::Greater => return true,
            std::cmp::Ordering::Less => return false,
            std::cmp::Ordering::Equal => {}
        }
    }
    false
}

/// Wert eines Textfeldes aus JSON (erstes Vorkommen von `"key": "…"`).
fn json_string(json: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let mut rest = &json[json.find(&pat)? + pat.len()..];
    rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let mut chars = rest.strip_prefix('"')?.chars();
    let mut out = String::new();
    loop {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?).unwrap_or('?'));
                }
                c => out.push(c),
            },
            c => out.push(c),
        }
    }
}

/// Antwort von `/releases/latest` auswerten.
pub fn parse_latest(json: &str) -> Option<Release> {
    let tag = json_string(json, "tag_name")?;
    // nur harmlose Zeichen in die Adresse übernehmen
    if tag.is_empty()
        || !tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
    {
        return None;
    }
    parse_version(&tag)?;
    let version = tag.strip_prefix(['v', 'V']).unwrap_or(&tag).to_string();
    Some(Release {
        version,
        url: format!("https://github.com/{REPO}/releases/tag/{tag}"),
    })
}

/// Neueste Version bei GitHub abfragen (blockiert bis zu 15 Sekunden – nicht im
/// Hauptthread aufrufen).
pub fn fetch_latest() -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let agent = format!("User-Agent: Stijl/{}", env!("CARGO_PKG_VERSION"));
    let mut cmd = Command::new(curl_path());
    cmd.args([
        "--silent",
        "--show-error",
        "--fail",
        "--location",
        "--max-time",
        "15",
        "--proto",
        "=https",
        "--header",
        "Accept: application/vnd.github+json",
        "--header",
        &agent,
        &url,
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // kein Konsolenfenster aufblitzen lassen
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    parse_latest(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| "unexpected answer".into())
}

/// `curl` des Systems (unter Windows fest aus System32, damit kein fremdes Programm aus
/// dem Arbeitsordner gestartet wird).
fn curl_path() -> std::path::PathBuf {
    #[cfg(windows)]
    {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        return std::path::PathBuf::from(root).join("System32").join("curl.exe");
    }
    #[cfg(target_os = "macos")]
    {
        return "/usr/bin/curl".into();
    }
    #[allow(unreachable_code)]
    "curl".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.9.40"), Some(vec![0, 9, 40]));
        assert_eq!(parse_version("1.2.3-beta.1"), Some(vec![1, 2, 3]));
        assert_eq!(parse_version("x1"), None);
        assert_eq!(parse_version(""), None);
        assert!(is_newer("v0.9.40", "0.9.39"));
        assert!(is_newer("0.10.0", "0.9.99"));
        assert!(is_newer("1.0", "0.9.39"));
        assert!(!is_newer("0.9.39", "0.9.39"));
        assert!(!is_newer("0.9.39.0", "0.9.39"));
        assert!(!is_newer("0.9.38", "0.9.39"));
        assert!(!is_newer("Mist", "0.9.39"));
    }

    #[test]
    fn latest_json() {
        let json = r#"{"url":"https://api.github.com/x","html_url":"https://github.com/benhuesing/Stijl/releases/tag/v0.9.40",
            "author":{"login":"benhuesing","html_url":"https://github.com/benhuesing"},
            "tag_name" : "v0.9.40","name":"Stijl 0.9.40 \u2013 \"Test\"","draft":false}"#;
        let r = parse_latest(json).unwrap();
        assert_eq!(r.version, "0.9.40");
        assert_eq!(r.url, "https://github.com/benhuesing/Stijl/releases/tag/v0.9.40");
        assert_eq!(json_string(json, "name").unwrap(), "Stijl 0.9.40 – \"Test\"");
        // seltsame Tags werden nicht in eine Adresse übernommen
        assert!(parse_latest(r#"{"tag_name":"v1.0/../../evil"}"#).is_none());
        assert!(parse_latest(r#"{"message":"Not Found"}"#).is_none());
        assert!(parse_latest("").is_none());
    }

    #[test]
    fn schedule() {
        let day = INTERVAL.as_secs() as i64;
        assert!(due(0, 1000));
        assert!(!due(1000, 1000 + day - 1));
        assert!(due(1000, 1000 + day));
        // Uhr zurückgestellt
        assert!(due(1000, 500));
    }
}
