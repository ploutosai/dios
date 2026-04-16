/// Minimal reproduction for sibling scroll-container paint/clipping bugs in Blitz.
///
/// The top row has three plain columns with normal content.
/// The bottom row has three otherwise-identical columns, but each content area
/// uses `overflow-y: auto`.
///
/// If Blitz is correct, all six columns should paint their colored rectangles.
/// If the bug is in scroll clipping/layer state, the bottom row may only paint
/// the first scroll column correctly.
use blitz_shell_miniquad::{BlitzMiniquadApp, BlitzShellProxy};
use dioxus::prelude::*;
use dioxus_native_dom::{DioxusDocument, DocumentConfig};

const CSS: &str = r#"
* {
    box-sizing: border-box;
    margin: 0 !important;
    padding: 0;
}

html, body, main, #main {
    width: 100%;
    height: 100%;
    margin: 0 !important;
    padding: 0 !important;
    background: #1e1e2e;
    color: #cdd6f4;
    font-family: sans-serif;
    font-size: 14px;
}

#root {
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
    gap: 16px;
    padding: 16px;
}

.row-title {
    height: 24px;
    line-height: 24px;
    color: #89b4fa;
    font-weight: bold;
}

.row {
    flex: 1;
    display: flex;
    flex-direction: row;
    gap: 16px;
    min-height: 0;
}

.column {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
}

.label {
    height: 28px;
    line-height: 28px;
    color: #89b4fa;
    font-weight: bold;
    padding-left: 4px;
    flex-shrink: 0;
}

.panel {
    flex: 1;
    min-height: 0;
    background: #313244;
}

.panel.scroll {
    overflow-y: auto;
    overflow-x: hidden;
}

.stack {
    width: 100%;
}

.rect {
    height: 40px;
    width: 100%;
}

.a { background: #cba6f7; }
.b { background: #89b4fa; }
.c { background: #a6e3a1; }
.d { background: #f9e2af; }
.e { background: #fab387; }
.f { background: #f38ba8; }
.g { background: #89dceb; }
.h { background: #cdd6f4; }
"#;

#[component]
fn Rects() -> Element {
    rsx! {
        div { class: "stack",
            div { class: "rect a" }
            div { class: "rect b" }
            div { class: "rect c" }
            div { class: "rect d" }
            div { class: "rect e" }
            div { class: "rect f" }
            div { class: "rect g" }
            div { class: "rect h" }
            div { class: "rect a" }
            div { class: "rect b" }
            div { class: "rect c" }
            div { class: "rect d" }
            div { class: "rect e" }
            div { class: "rect f" }
            div { class: "rect g" }
            div { class: "rect h" }
        }
    }
}

fn app() -> Element {
    rsx! {
        div { id: "root",
            div { class: "row-title", "Plain panels (no overflow)" }
            div { class: "row",
                div { class: "column",
                    div { class: "label", "Plain 1" }
                    div { class: "panel", Rects {} }
                }
                div { class: "column",
                    div { class: "label", "Plain 2" }
                    div { class: "panel", Rects {} }
                }
                div { class: "column",
                    div { class: "label", "Plain 3" }
                    div { class: "panel", Rects {} }
                }
            }

            div { class: "row-title", "Scroll panels (overflow-y: auto)" }
            div { class: "row",
                div { class: "column",
                    div { class: "label", "Scroll 1" }
                    div { class: "panel scroll", Rects {} }
                }
                div { class: "column",
                    div { class: "label", "Scroll 2" }
                    div { class: "panel scroll", Rects {} }
                }
                div { class: "column",
                    div { class: "label", "Scroll 3" }
                    div { class: "panel scroll", Rects {} }
                }
            }
        }
    }
}

fn main() {
    let (proxy, event_queue) = BlitzShellProxy::new();

    let vdom = VirtualDom::new(app);
    let mut doc = DioxusDocument::new(vdom, DocumentConfig::default());
    doc.inner.borrow_mut().add_user_agent_stylesheet(CSS);
    doc.initial_build();

    let conf = miniquad::conf::Conf {
        window_title: "Scroll Columns Repro".to_string(),
        window_width: 1100,
        window_height: 900,
        high_dpi: true,
        ..Default::default()
    };

    miniquad::start(conf, move || {
        Box::new(BlitzMiniquadApp::new(Box::new(doc), proxy, event_queue))
    });
}
