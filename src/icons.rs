//! Eigene symbolische Symbole. Sie werden beim Start in den Cache geschrieben und
//! als Suchpfad im Symbolthema registriert, damit GTK sie wie Systemsymbole
//! einfärbt (hell/dunkel).

use gtk::{gdk, glib};

fn svg(body: &str) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"16\" height=\"16\" viewBox=\"0 0 16 16\">\
<g fill=\"#222222\">{body}</g></svg>"
    )
}

const ICONS: &[(&str, &str)] = &[
    (
        "mondrian-unlocked-symbolic",
        r#"<path d="M3 7h10v8H3z"/><path d="M4.5 7V5a3.5 3.5 0 0 1 7 0H10a2 2 0 0 0-4 0v2z"/>"#,
    ),
    (
        "mondrian-effect-symbolic",
        r#"<path d="M1.4 13.5 9.9 5l1.1 1.1-8.5 8.5z"/><path d="M11 .8l.7 1.6 1.6.7-1.6.7-.7 1.6-.7-1.6-1.6-.7 1.6-.7zM14.2 6.4l.4 1 1 .4-1 .4-.4 1-.4-1-1-.4 1-.4zM5.6 1.2l.4 1 1 .4-1 .4-.4 1-.4-1-1-.4 1-.4z"/>"#,
    ),
    (
        "mondrian-blend-symbolic",
        r#"<path d="M1 5.5h3v5H1z"/><path opacity=".6" d="M6.2 4.5h3.6v7H6.2z"/><path d="M12 3h3v10h-3z"/>"#,
    ),
    (
        "mondrian-distribute-symbolic",
        r#"<path d="M1 1h1.5v14H1zM13.5 1H15v14h-1.5z"/><path d="M5 4h2v8H5zM9 4h2v8H9z"/>"#,
    ),
    (
        "mondrian-group-symbolic",
        r#"<path d="M1 1h4v1.5H2.5V5H1zM11 1h4v4h-1.5V2.5H11zM1 11h1.5v2.5H5V15H1zM13.5 11H15v4h-4v-1.5h2.5z"/><path d="M4 4h5v5H4z"/><path opacity=".55" d="M7 7h5v5H7z"/>"#,
    ),
    (
        "mondrian-ungroup-symbolic",
        r#"<path d="M1 1h6.5v6.5H1z"/><path fill-rule="evenodd" d="M8.5 8.5H15V15H8.5zM10 10v3.5h3.5V10z"/>"#,
    ),
    (
        "mondrian-props-symbolic",
        r#"<path fill-rule="evenodd" d="M8 1C4.1 1 1 3.9 1 7.6 1 11.2 3.9 15 7.4 15c1.4 0 1.7-1 1.2-2-.6-1.1.1-2.2 1.4-2.2h2.2c2 0 3.8-1.4 3.8-3.6C16 3.6 12.3 1 8 1zM4.5 8.5a1.3 1.3 0 1 1 0-2.6 1.3 1.3 0 0 1 0 2.6zm2-3.4a1.3 1.3 0 1 1 0-2.6 1.3 1.3 0 0 1 0 2.6zm3.5 0a1.3 1.3 0 1 1 0-2.6 1.3 1.3 0 0 1 0 2.6zm2.6 2.6a1.3 1.3 0 1 1 0-2.6 1.3 1.3 0 0 1 0 2.6z"/>"#,
    ),
    (
        "mondrian-layers-symbolic",
        r#"<path d="M8 1 15 4.75 8 8.5 1 4.75z"/><path d="M2.6 7.2 8 10.1l5.4-2.9L15 8 8 11.75 1 8z"/><path d="M2.6 10.45 8 13.35l5.4-2.9 1.6.8L8 15 1 11.25z"/>"#,
    ),
    (
        "mondrian-bucket-symbolic",
        r#"<path d="M6.5 1.5 13 8l-5.5 5.5L1 7zM6.5 3.6 3.1 7h8.8z"/><path d="M13.5 10s1.5 1.9 1.5 2.8a1.5 1.5 0 0 1-3 0c0-.9 1.5-2.8 1.5-2.8z"/>"#,
    ),
    (
        "mondrian-eraser-symbolic",
        r#"<path d="M9.6 1.6 14.4 6.4 7.8 13H13v1.5H5.6L1.6 10.5zM5.3 8.2 2.9 10.6l3.4 3.4h.9L9.4 11.8z"/>"#,
    ),
    (
        "mondrian-builder-symbolic",
        r#"<path fill-rule="evenodd" d="M1 1h9v9H1zm1.5 1.5v6h6v-6z"/><path opacity=".55" d="M6 6h9v9H6z"/><path d="M5 12.5c2-1 4-4 9-9l.9.9c-5 5-7 8.4-9.4 9.4z"/>"#,
    ),
    (
        "mondrian-join-miter-symbolic",
        r#"<path d="M3 14V2h11v3.5H6.5V14z"/>"#,
    ),
    (
        "mondrian-join-round-symbolic",
        r#"<path d="M3 14V7a5 5 0 0 1 5-5h6v3.5H8A1.5 1.5 0 0 0 6.5 7v7z"/>"#,
    ),
    (
        "mondrian-join-bevel-symbolic",
        r#"<path d="M3 14V6l4-4h7v3.5H8.4L6.5 7.4V14z"/>"#,
    ),
    (
        "mondrian-cap-butt-symbolic",
        r#"<path d="M1 5.5h9v5H1z"/><path opacity=".5" d="M10 2h1v12h-1z"/>"#,
    ),
    (
        "mondrian-cap-round-symbolic",
        r#"<path d="M1 5.5h9a2.5 2.5 0 0 1 0 5H1z"/><path opacity=".5" d="M10 2h1v12h-1z"/>"#,
    ),
    (
        "mondrian-cap-square-symbolic",
        r#"<path d="M1 5.5h11.5v5H1z"/><path opacity=".5" d="M10 2h1v12h-1z"/>"#,
    ),
    (
        "mondrian-library-symbolic",
        r#"<path d="M1.5 2h3v12h-3zM5.5 2h3v12h-3z"/><path d="M9.6 3.1l2.7-.8 3.2 11.1-2.7.8z"/>"#,
    ),
    (
        "mondrian-page-left-symbolic",
        r#"<path d="M1 1h1.5v14H1z"/><path d="M4 3h9v4H4zM4 9h6v4H4z"/>"#,
    ),
    (
        "mondrian-page-hcenter-symbolic",
        r#"<path d="M2 3h12v4H2zM4 9h8v4H4zM7.25 1h1.5v2h-1.5zM7.25 7h1.5v2h-1.5zM7.25 13h1.5v2h-1.5z"/>"#,
    ),
    (
        "mondrian-page-right-symbolic",
        r#"<path d="M13.5 1H15v14h-1.5z"/><path d="M3 3h9v4H3zM6 9h6v4H6z"/>"#,
    ),
    (
        "mondrian-page-top-symbolic",
        r#"<path d="M1 1h14v1.5H1z"/><path d="M3 4h4v9H3zM9 4h4v6H9z"/>"#,
    ),
    (
        "mondrian-page-vcenter-symbolic",
        r#"<path d="M3 2h4v12H3zM9 4h4v8H9zM1 7.25h2v1.5H1zM7 7.25h2v1.5H7zM13 7.25h2v1.5h-2z"/>"#,
    ),
    (
        "mondrian-page-bottom-symbolic",
        r#"<path d="M1 13.5h14V15H1z"/><path d="M3 3h4v9H3zM9 6h4v6H9z"/>"#,
    ),
    (
        "mondrian-select-symbolic",
        r#"<path d="M3 1v13l3.4-3.3 2.3 4.8 2-1-2.3-4.7H13z"/>"#,
    ),
    (
        "mondrian-node-symbolic",
        r#"<path d="M2 1v10.5l2.7-2.6 1.9 4 1.7-.9-1.9-3.9H10z"/><path fill-rule="evenodd" d="M10 10h5v5h-5zm1.5 1.5v2h2v-2z"/>"#,
    ),
    (
        "mondrian-shapes-symbolic",
        r#"<path fill-rule="evenodd" d="M1 1h9v9H1zm1.5 1.5v6h6v-6z"/><circle cx="11" cy="11" r="4.5"/>"#,
    ),
    (
        "mondrian-pen-symbolic",
        r#"<path fill-rule="evenodd" d="M8 1 3.2 7.2 6 12.5h4l2.8-5.3zm0 4.6a1.4 1.4 0 1 1 0 2.8 1.4 1.4 0 0 1 0-2.8z"/><path d="M7.4 1.5h1.2v4.2H7.4zM5.5 13.5h5V15h-5z"/>"#,
    ),
    (
        "mondrian-pencil-symbolic",
        r#"<path d="M11.6 1.3 14.7 4.4 5.6 13.5 1.3 14.7 2.5 10.4z"/>"#,
    ),
    (
        "mondrian-text-symbolic",
        r#"<path d="M2 2h12v3.5h-1.5L12 3.8H9.1V12.5l1.6.5V14.5H5.3V13l1.6-.5V3.8H4L3.5 5.5H2z"/>"#,
    ),
    (
        "mondrian-hand-symbolic",
        r#"<path d="M8 0 11 3H9v4h4V5l3 3-3 3V9H9v4h2l-3 3-3-3h2V9H3v2L0 8l3-3v2h4V3H5z"/>"#,
    ),
    (
        "mondrian-eye-symbolic",
        r#"<path fill-rule="evenodd" d="M8 3C4.5 3 1.7 5.2.5 8c1.2 2.8 4 5 7.5 5s6.3-2.2 7.5-5C14.3 5.2 11.5 3 8 3zm0 1.5c2.6 0 4.7 1.4 5.8 3.5-1.1 2.1-3.2 3.5-5.8 3.5S3.3 10.1 2.2 8C3.3 5.9 5.4 4.5 8 4.5z"/><circle cx="8" cy="8" r="2"/>"#,
    ),
    (
        "mondrian-eye-off-symbolic",
        r#"<path fill-rule="evenodd" opacity=".45" d="M8 3C4.5 3 1.7 5.2.5 8c1.2 2.8 4 5 7.5 5s6.3-2.2 7.5-5C14.3 5.2 11.5 3 8 3zm0 1.5c2.6 0 4.7 1.4 5.8 3.5-1.1 2.1-3.2 3.5-5.8 3.5S3.3 10.1 2.2 8C3.3 5.9 5.4 4.5 8 4.5z"/><path d="M2.1 1 15 13.9 13.9 15 1 2.1z"/>"#,
    ),
    (
        "mondrian-lock-symbolic",
        r#"<path d="M3 7h10v8H3z"/><path d="M4.5 7V5a3.5 3.5 0 0 1 7 0v2H10V5a2 2 0 0 0-4 0v2z"/>"#,
    ),
    (
        "mondrian-unlock-symbolic",
        r#"<path opacity=".45" d="M3 7h10v8H3z"/><path opacity=".45" d="M4.5 7V5a3.5 3.5 0 0 1 7 0H10a2 2 0 0 0-4 0v2z"/>"#,
    ),
    ("mondrian-union-symbolic", r#"<path d="M1 1h9v5h5v9H6v-5H1z"/>"#),
    (
        "mondrian-subtract-symbolic",
        r#"<path d="M1 1h9v5H6v4H1z"/><path fill-rule="evenodd" opacity=".55" d="M6 6h9v9H6zm1.5 1.5v6h6v-6z"/>"#,
    ),
    (
        "mondrian-intersect-symbolic",
        r#"<path fill-rule="evenodd" opacity=".55" d="M1 1h9v9H1zm1.5 1.5v6h6v-6zM6 6h9v9H6zm1.5 1.5v6h6v-6z"/><path d="M6 6h4v4H6z"/>"#,
    ),
    (
        "mondrian-exclude-symbolic",
        r#"<path fill-rule="evenodd" d="M1 1h9v9H1zM6 6h9v9H6z"/>"#,
    ),
    (
        "mondrian-rotate-left-symbolic",
        r#"<path d="M8 2a6 6 0 1 1-6 6h2a4 4 0 1 0 4-4z"/><path d="M8 0v6L5 3z"/>"#,
    ),
    (
        "mondrian-rotate-right-symbolic",
        r#"<path d="M8 2a6 6 0 1 0 6 6h-2a4 4 0 1 1-4-4z"/><path d="M8 0v6l3-3z"/>"#,
    ),
    (
        "mondrian-flip-h-symbolic",
        r#"<path d="M6.5 2.5v11H1z"/><path opacity=".45" d="M9.5 2.5v11H15z"/><path d="M7.5 1h1v14h-1z"/>"#,
    ),
    (
        "mondrian-flip-v-symbolic",
        r#"<path d="M2.5 6.5h11V1z"/><path opacity=".45" d="M2.5 9.5h11V15z"/><path d="M1 7.5h14v1H1z"/>"#,
    ),
    (
        "mondrian-textpath-symbolic",
        r#"<path d="M4 1h8v2H9v6H7V3H4z"/><path d="M1 12.2c3.6-3.3 10.4-3.3 14 0l-1.1 1.2c-3-2.8-8.8-2.8-11.8 0z"/>"#,
    ),
    (
        "mondrian-rect-symbolic",
        r#"<path fill-rule="evenodd" d="M2 3h12v10H2zm1.5 1.5v7h9v-7z"/>"#,
    ),
    (
        "mondrian-ellipse-symbolic",
        r#"<path fill-rule="evenodd" d="M8 2.5c3.6 0 6.5 2.5 6.5 5.5S11.6 13.5 8 13.5 1.5 11 1.5 8 4.4 2.5 8 2.5zm0 1.5C5.2 4 3 5.8 3 8s2.2 4 5 4 5-1.8 5-4-2.2-4-5-4z"/>"#,
    ),
    (
        "mondrian-line-symbolic",
        r#"<path d="M12.9 2 14 3.1 3.1 14 2 12.9z"/>"#,
    ),
    (
        "mondrian-polygon-symbolic",
        r#"<path fill-rule="evenodd" d="M8 1.5l6.2 4.5-2.4 7.5H4.2L1.8 6zm0 1.9L3.6 6.6l1.7 5.4h5.4l1.7-5.4z"/>"#,
    ),
    (
        "mondrian-star-symbolic",
        r#"<path d="M8 1l2 4.6 5 .4-3.8 3.3 1.2 4.9L8 11.6l-4.4 2.6 1.2-4.9L1 6l5-.4z"/>"#,
    ),
    (
        "mondrian-curvature-symbolic",
        r#"<path d="M1 12c2-8 5-8 7-4s5 4 7-4l1.3.5c-2.4 9-7 9-9.5 4.2C5 4.6 3.5 4.6 2.4 12.4z"/><circle cx="8" cy="8" r="1.8"/>"#,
    ),
    (
        "mondrian-textarea-symbolic",
        r#"<path fill-rule="evenodd" d="M1 1h14v14H1zm1.5 1.5v11h11v-11z"/><path d="M4.5 4.5h7v1.5H8.8v5.5H7.2V6H4.5z"/>"#,
    ),
    ("mondrian-corner-symbolic", r#"<path d="M16 6v10H6z"/>"#),
    (
        "mondrian-guide-symbolic",
        r#"<path d="M0 4h16v1.5H0z"/><path d="M10 0h1.5v16H10z"/><path opacity=".45" d="M2 8h6v6H2z"/>"#,
    ),
    (
        "mondrian-guide-h-symbolic",
        r#"<path d="M0 7.25h16v1.5H0z"/><path opacity=".45" d="M3 2h10v3H3zM3 11h10v3H3z"/>"#,
    ),
    (
        "mondrian-guide-v-symbolic",
        r#"<path d="M7.25 0h1.5v16h-1.5z"/><path opacity=".45" d="M2 3h3v10H2zM11 3h3v10h-3z"/>"#,
    ),
    (
        "mondrian-guide-angle-symbolic",
        r#"<path d="M14.9 0 16 1.1 1.1 16 0 14.9z"/><path opacity=".45" d="M2 2h5v5H2zM9 9h5v5H9z"/>"#,
    ),
    (
        "mondrian-align-left-symbolic",
        r#"<path d="M1 2h14v1.5H1zM1 5.5h9V7H1zM1 9h14v1.5H1zM1 12.5h9V14H1z"/>"#,
    ),
    (
        "mondrian-align-center-symbolic",
        r#"<path d="M1 2h14v1.5H1zM3.5 5.5h9V7h-9zM1 9h14v1.5H1zM3.5 12.5h9V14h-9z"/>"#,
    ),
    (
        "mondrian-align-right-symbolic",
        r#"<path d="M1 2h14v1.5H1zM6 5.5h9V7H6zM1 9h14v1.5H1zM6 12.5h9V14H6z"/>"#,
    ),
    (
        "mondrian-align-justify-symbolic",
        r#"<path d="M1 2h14v1.5H1zM1 5.5h14V7H1zM1 9h14v1.5H1zM1 12.5h9V14H1z"/>"#,
    ),
    (
        "mondrian-valign-top-symbolic",
        r#"<path d="M1 1h14v1.5H1z"/><path opacity=".45" d="M1 13.5h14V15H1z"/><path d="M4 4h8v1.5H4zM4 7h8v1.5H4z"/>"#,
    ),
    (
        "mondrian-valign-middle-symbolic",
        r#"<path opacity=".45" d="M1 1h14v1.5H1zM1 13.5h14V15H1z"/><path d="M4 6h8v1.5H4zM4 8.5h8V10H4z"/>"#,
    ),
    (
        "mondrian-valign-bottom-symbolic",
        r#"<path opacity=".45" d="M1 1h14v1.5H1z"/><path d="M1 13.5h14V15H1z"/><path d="M4 7.5h8V9H4zM4 10.5h8V12H4z"/>"#,
    ),
    (
        "mondrian-line-spacing-symbolic",
        r#"<path d="M7 2h8v1.5H7zM7 7.25h8v1.5H7zM7 12.5h8V14H7z"/><path d="M3 0 5.5 3h-5zM3 16 .5 13h5zM2.3 3h1.4v10H2.3z"/>"#,
    ),
    (
        "mondrian-letter-spacing-symbolic",
        r#"<path d="M2 2h4.5L9 9H7.4l-.6-1.8H3.7L3.1 9H1.5zm1.2 3.8h3L4.7 3.6z"/><path d="M10 2h1.5v7H10zM13 2h1.5v7H13z"/><path d="M0 13l3-2.5v5zM16 13l-3-2.5v5zM3 12.3h10v1.4H3z"/>"#,
    ),
    (
        "mondrian-underline-symbolic",
        r#"<path d="M4 1h1.8v6a2.2 2.2 0 0 0 4.4 0V1H12v6a4 4 0 0 1-8 0z"/><path d="M3 13.5h10V15H3z"/>"#,
    ),
    (
        "mondrian-strike-symbolic",
        r#"<path d="M8 1c-2.4 0-4 1.3-4 3.2 0 .9.3 1.6.9 2.1h2.9C6.4 5.9 5.8 5.3 5.8 4.3 5.8 3.2 6.7 2.6 8 2.6c1.2 0 2.1.6 2.3 1.6h1.7C11.8 2.3 10.2 1 8 1zM9.8 9.6c.4.4.5.8.5 1.3 0 1.1-.9 1.9-2.4 1.9-1.4 0-2.4-.7-2.5-1.8H3.7C3.8 13 5.6 14.4 8 14.4c2.6 0 4.2-1.4 4.2-3.5 0-.5-.1-.9-.3-1.3z"/><path d="M1 7.4h14v1.5H1z"/>"#,
    ),
    (
        "mondrian-smallcaps-symbolic",
        r#"<path fill-rule="evenodd" d="M.5 14 4.7 2h1.8l4.2 12H8.8l-.9-2.9H3.3L2.4 14zm3.3-4.4h3.6L5.6 4.4z"/><path fill-rule="evenodd" d="M10.6 14l2-5.4h1.2l2 5.4h-1.1l-.4-1.3h-2.2l-.4 1.3zm1.8-2.3h1.4l-.7-2.1z"/>"#,
    ),
    (
        "mondrian-ligature-symbolic",
        r#"<path d="M2 6.4h3V4.6C5 2.6 6.2 1.5 8.2 1.5H12v1.6H8.3C7.2 3.1 6.7 3.7 6.7 4.7v1.7H9V8H6.7v6H5V8H2z"/><path d="M9.8 6.4h1.7V14H9.8z"/>"#,
    ),
    (
        "mondrian-eyedropper-symbolic",
        r#"<path d="M11.1 1.4a2.3 2.3 0 0 1 3.5 3.5l-2 2-3.5-3.5z"/><path d="M8.3 3.5l4.2 4.2-1.2 1.2-4.2-4.2z"/><path d="M7.9 6.1l2 2-5.7 5.7-1.9.8-.9 1-.7-.7 1-.9.8-1.9z"/>"#,
    ),
    (
        "mondrian-notebook-symbolic",
        r#"<path fill-rule="evenodd" d="M3.5 1h10A1.5 1.5 0 0 1 15 2.5v11a1.5 1.5 0 0 1-1.5 1.5h-10zM5 2.5v11h8.5v-11z"/><path d="M1 3.5h4.5V5H1zM1 7.25h4.5v1.5H1zM1 11h4.5v1.5H1zM7.5 5h4.5v1.5H7.5z"/>"#,
    ),
    (
        "mondrian-list-symbolic",
        r#"<circle cx="2.5" cy="3.5" r="1.5"/><circle cx="2.5" cy="8" r="1.5"/><circle cx="2.5" cy="12.5" r="1.5"/><path d="M6 2.75h9v1.5H6zM6 7.25h9v1.5H6zM6 11.75h9v1.5H6z"/>"#,
    ),
    (
        "mondrian-click-hint-symbolic",
        r#"<path d="M6.2 4.8 6.2 14.4 8.6 12.1 10.3 15.6 12 14.8 10.3 11.4 13.6 11.2z"/><path d="M5.45 0.6h1.5v2.6h-1.5zM0.8 4.45h2.6v1.5H0.8z"/><path d="M1.7 2.75 2.75 1.7 4.6 3.55 3.55 4.6z"/><path d="M8.6 2.6 10.45 0.75 11.5 1.8 9.65 3.65z"/>"#,
    ),
    (
        "mondrian-indent-more-symbolic",
        r#"<path d="M1 2h14v1.5H1zM7 6h8v1.5H7zM7 9.5h8V11H7zM1 13h14v1.5H1z"/><path d="M1 5.5 4.5 8.25 1 11z"/>"#,
    ),
    (
        "mondrian-indent-less-symbolic",
        r#"<path d="M1 2h14v1.5H1zM7 6h8v1.5H7zM7 9.5h8V11H7zM1 13h14v1.5H1z"/><path d="M4.5 5.5 1 8.25 4.5 11z"/>"#,
    ),
    (
        "mondrian-path-below-symbolic",
        r#"<path d="M1 6.5C3.5 3 12.5 3 15 6.5l-1.2.9C11.7 4.6 4.3 4.6 2.2 7.4z"/><path d="M4 9h2v5H4zM7 9h2v5H7zM10 9h2v5h-2z"/>"#,
    ),
    (
        "mondrian-path-reverse-symbolic",
        r#"<path d="M2 8a6 6 0 0 1 10.2-4.3L14 2v5H9l1.9-1.9A4.2 4.2 0 0 0 3.8 8z"/><path d="M14 8a6 6 0 0 1-10.2 4.3L2 14V9h5l-1.9 1.9A4.2 4.2 0 0 0 12.2 8z"/>"#,
    ),
];

pub fn install() {
    let dir = glib::user_cache_dir().join("mondrian").join("icons");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    for (name, body) in ICONS {
        let path = dir.join(format!("{name}.svg"));
        let content = svg(body);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(content.as_str()) {
            let _ = std::fs::write(&path, content);
        }
    }
    // farbiges App-Symbol (Startseite, Info-Dialog, Fenster)
    let app = dir.join("io.github.mondrian.Mondrian.svg");
    let content = include_str!("../data/io.github.mondrian.Mondrian.svg");
    if std::fs::read_to_string(&app).ok().as_deref() != Some(content) {
        let _ = std::fs::write(&app, content);
    }
    if let Some(display) = gdk::Display::default() {
        gtk::IconTheme::for_display(&display).add_search_path(&dir);
        gtk::Window::set_default_icon_name("io.github.mondrian.Mondrian");
    }
}
