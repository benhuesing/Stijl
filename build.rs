//! Nur unter Windows: Programmsymbol und Versionsangaben in die .exe einbetten
//! (Explorer, Taskleiste, „Eigenschaften“). Unter Linux tut das Skript nichts.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=data/mondrian.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("data/mondrian.ico");
        res.set("ProductName", "Mondrian");
        res.set("FileDescription", "Mondrian – Vektorzeichenprogramm");
        res.set("LegalCopyright", "GPL-3.0-or-later");
        // fehlt windres, wird ohne Symbol weitergebaut
        if let Err(e) = res.compile() {
            println!("cargo:warning=Programmsymbol nicht eingebettet: {e}");
        }
    }
}
