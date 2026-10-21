// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! Nur unter Windows: Programmsymbol und Versionsangaben in die .exe einbetten
//! (Explorer, Taskleiste, „Eigenschaften“). Unter Linux tut das Skript nichts.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=data/stijl.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("data/stijl.ico");
        res.set("ProductName", "Stijl");
        res.set("FileDescription", "Stijl – Vektorzeichenprogramm");
        res.set("CompanyName", "Bendix Hüsing");
        res.set(
            "LegalCopyright",
            "Copyright © 2026 Bendix Hüsing · GPL-3.0-or-later",
        );
        // fehlt windres, wird ohne Symbol weitergebaut
        if let Err(e) = res.compile() {
            println!("cargo:warning=Programmsymbol nicht eingebettet: {e}");
        }
    }
}
