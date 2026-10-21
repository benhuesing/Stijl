// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! Dateien sicher schreiben und Wiederherstellungsdateien verwalten (ohne GTK).
//!
//! `write_atomic` schreibt erst in eine temporäre Datei im selben Ordner und benennt sie
//! danach um. Bricht das Schreiben ab (Absturz, Stromausfall, volle Platte), bleibt die
//! alte Datei unverändert erhalten.
//!
//! Wiederherstellungsdateien liegen in einem eigenen Ordner (`~/.local/state/stijl/recovery`).
//! Jede besteht aus `<kennung>.svg` (Inhalt) und `<kennung>.info` (Herkunft). Die Kennung
//! beginnt mit der Prozessnummer, so erkennt ein neu gestartetes Programm Dateien, die ein
//! abgestürztes hinterlassen hat. Die Originaldatei wird dabei nie verändert.

use crate::i18n::tr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Daten sicher in `path` schreiben: temporäre Datei daneben, dann umbenennen.
/// Rechte einer vorhandenen Datei bleiben erhalten; Verknüpfungen werden aufgelöst.
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    // Bei einer Verknüpfung das Ziel ersetzen, nicht die Verknüpfung selbst
    let target: PathBuf = match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    };
    let dir = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = target
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, tr("kein Dateiname")))?
        .to_string_lossy()
        .to_string();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        if let Ok(meta) = fs::metadata(&target) {
            let _ = fs::set_permissions(&tmp, meta.permissions());
        }
        fs::rename(&tmp, &target)?;
        // Ordnereintrag sichern (nicht überall möglich, daher ohne Fehler)
        if let Ok(d) = fs::File::open(&dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Eine gefundene Wiederherstellungsdatei.
#[derive(Clone, Debug, PartialEq)]
pub struct Recovery {
    /// Kennung (Dateiname ohne Endung)
    pub id: String,
    /// Inhalt als SVG
    pub svg: PathBuf,
    /// ursprünglicher Speicherort des Dokuments (falls es schon einen hatte)
    pub original: Option<PathBuf>,
    /// Name, unter dem das Dokument angezeigt wurde
    pub title: String,
    /// Zeitpunkt der Sicherung (Sekunden seit 1970)
    pub time: u64,
}

/// Kennzeichen dieses Programmlaufs (Prozessnummer und Startzeit, damit eine später
/// wiederverwendete Prozessnummer nicht mit einem abgestürzten Lauf verwechselt wird).
fn session() -> &'static str {
    static S: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    S.get_or_init(|| format!("{}-{}", std::process::id(), now_secs()))
}

/// Kennung für ein Dokument dieses Programmlaufs.
pub fn recovery_id(n: u64) -> String {
    format!("{}-{n}", session())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Wiederherstellungsdatei schreiben (oder ersetzen).
pub fn save_recovery(
    dir: &Path,
    id: &str,
    original: Option<&Path>,
    title: &str,
    svg: &str,
) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    write_atomic(&dir.join(format!("{id}.svg")), svg.as_bytes())?;
    let mut info = String::new();
    if let Some(p) = original {
        info.push_str(&format!("original={}\n", p.to_string_lossy()));
    }
    info.push_str(&format!("title={}\n", title.replace('\n', " ")));
    info.push_str(&format!("time={}\n", now_secs()));
    write_atomic(&dir.join(format!("{id}.info")), info.as_bytes())
}

/// Wiederherstellungsdatei entfernen (nach dem Speichern oder Schließen).
pub fn remove_recovery(dir: &Path, id: &str) {
    let _ = fs::remove_file(dir.join(format!("{id}.svg")));
    let _ = fs::remove_file(dir.join(format!("{id}.info")));
}

/// Alle Wiederherstellungsdateien, die nicht von diesem Programmlauf stammen (also von einem
/// abgestürzten oder unsauber beendeten), neueste zuerst.
pub fn foreign_recoveries(dir: &Path) -> Vec<Recovery> {
    let own = format!("{}-", session());
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("svg") {
            continue;
        }
        let Some(id) = p.file_stem().map(|s| s.to_string_lossy().to_string()) else {
            continue;
        };
        if id.starts_with(&own) {
            continue;
        }
        let info = fs::read_to_string(dir.join(format!("{id}.info"))).unwrap_or_default();
        let mut r = Recovery {
            id,
            svg: p.clone(),
            original: None,
            title: String::new(),
            time: 0,
        };
        for line in info.lines() {
            match line.split_once('=') {
                Some(("original", v)) if !v.is_empty() => r.original = Some(PathBuf::from(v)),
                Some(("title", v)) => r.title = v.to_string(),
                Some(("time", v)) => r.time = v.trim().parse().unwrap_or(0),
                _ => {}
            }
        }
        if r.title.is_empty() {
            r.title = "Unbenannt".into();
        }
        out.push(r);
    }
    out.sort_by(|a, b| b.time.cmp(&a.time));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mondrian-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp_files() {
        let d = tmpdir("atomic");
        let p = d.join("bild.svg");
        write_atomic(&p, b"eins").unwrap();
        write_atomic(&p, b"zwei").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"zwei");
        let names: Vec<_> = fs::read_dir(&d)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
        // in einen nicht vorhandenen Ordner: Fehler, nichts zurückgelassen
        assert!(write_atomic(&d.join("fehlt").join("x.svg"), b"x").is_err());
        let _ = fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_keeps_permissions_and_follows_links() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmpdir("perm");
        let p = d.join("a.svg");
        fs::write(&p, b"alt").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        write_atomic(&p, b"neu").unwrap();
        assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        let link = d.join("link.svg");
        std::os::unix::fs::symlink(&p, &link).unwrap();
        write_atomic(&link, b"ueber-link").unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&p).unwrap(), b"ueber-link");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn recovery_files_round_trip() {
        let d = tmpdir("recovery");
        // eigene Dateien werden nicht als fremd gemeldet
        let own = recovery_id(1);
        save_recovery(&d, &own, Some(Path::new("/home/a/logo.svg")), "logo", "<svg/>").unwrap();
        assert!(foreign_recoveries(&d).is_empty());
        // Datei eines anderen (abgestürzten) Laufs
        save_recovery(&d, "1-7", None, "Unbenannt 2", "<svg>x</svg>").unwrap();
        let f = foreign_recoveries(&d);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].id, "1-7");
        assert_eq!(f[0].original, None);
        assert_eq!(f[0].title, "Unbenannt 2");
        assert_eq!(fs::read_to_string(&f[0].svg).unwrap(), "<svg>x</svg>");
        remove_recovery(&d, "1-7");
        remove_recovery(&d, &own);
        assert!(fs::read_dir(&d).unwrap().next().is_none());
        let _ = fs::remove_dir_all(&d);
    }
}
