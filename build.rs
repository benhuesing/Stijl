// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Bendix Hüsing

//! Windows only: embeds the app icon and version information in the .exe (Explorer,
//! taskbar, “Properties”). Does nothing on other systems.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=data/stijl.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("data/stijl.ico");
        res.set("ProductName", "Stijl");
        res.set("FileDescription", "Stijl – vector graphics editor");
        res.set("CompanyName", "Bendix Hüsing");
        res.set(
            "LegalCopyright",
            "Copyright © 2026 Bendix Hüsing · GPL-3.0-or-later",
        );
        // without windres the build continues without an icon
        if let Err(e) = res.compile() {
            println!("cargo:warning=app icon not embedded: {e}");
        }
    }
}
