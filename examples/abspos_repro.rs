/// Minimal reproduction of absolute-positioning paint bug in Blitz.
///
/// Three columns side-by-side:
///   LEFT:   absolute div BEFORE the rectangles.
///   MIDDLE: absolute div AFTER the rectangles.
///   RIGHT:  no absolute div at all (control).
///
/// If Blitz paints correctly, all columns should look identical except for the
/// absolute div should NOT push the rectangles down.
/// If the bug exists, one of the absolute-position variants will show the
/// rectangles shifted downward by the height of the absolute div (20px).
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
    height: 100%;
    width: 100%;
    margin: 0 !important;
    padding: 0 !important;
    background: #1e1e2e;
    color: #cdd6f4;
    font-family: sans-serif;
    font-size: 14px;
    line-height: 22px;
}

#root {
    display: flex;
    flex-direction: row;
    width: 100%;
    height: 100%;
    gap: 20px;
    padding: 20px;
}

.column {
    flex: 1;
    display: flex;
    flex-direction: column;
}

.label {
    height: 30px;
    line-height: 30px;
    font-size: 13px;
    color: #89b4fa;
    font-weight: bold;
    padding-left: 4px;
    flex-shrink: 0;
}

/* Scrollable area — like editor-body */
.scroll-area {
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
    background: #313244;
}

/* The relative container — like code-area */
.relative-container {
    position: relative;
    padding: 0;
    margin: 0;
}

/* The absolute div — like the cursor */
.abs-div {
    position: absolute;
    left: 4px;
    top: 4px;
    width: 3px;
    height: 20px;
    background: #f38ba8;
}

/* Normal-flow colored rectangles */
.rect {
    height: 40px;
    margin: 0;
    padding: 0;
}
.rect-a { background: #cba6f7; } /* mauve */
.rect-b { background: #89b4fa; } /* blue */
.rect-c { background: #a6e3a1; } /* green */
.rect-d { background: #f9e2af; } /* yellow */
.rect-e { background: #fab387; } /* peach */
.rect-f { background: #f38ba8; } /* red */
.rect-g { background: #89dceb; } /* sky */
.rect-h { background: #cdd6f4; } /* text */

/* Divider between columns */
.divider {
    width: 2px;
    background: #585b70;
    flex-shrink: 0;
}
"#;

fn app() -> Element {
    rsx! {
        div { id: "root",
            // ── LEFT COLUMN: absolute div before content ──
            div { class: "column",
                div { class: "label", "ABS before" }
                div { class: "scroll-area",
                    div { class: "relative-container",
                        div { class: "abs-div" }
                        div { class: "rect rect-a" }
                        div { class: "rect rect-b" }
                        div { class: "rect rect-c" }
                        div { class: "rect rect-d" }
                        div { class: "rect rect-e" }
                        div { class: "rect rect-f" }
                        div { class: "rect rect-g" }
                        div { class: "rect rect-h" }
                        div { class: "rect rect-a" }
                        div { class: "rect rect-b" }
                        div { class: "rect rect-c" }
                        div { class: "rect rect-d" }
                        div { class: "rect rect-e" }
                        div { class: "rect rect-f" }
                        div { class: "rect rect-g" }
                        div { class: "rect rect-h" }
                    }
                }
            }

            div { class: "divider" }

            // ── MIDDLE COLUMN: absolute div after content ──
            div { class: "column",
                div { class: "label", "ABS after" }
                div { class: "scroll-area",
                    div { class: "relative-container",
                        div { class: "rect rect-a" }
                        div { class: "rect rect-b" }
                        div { class: "rect rect-c" }
                        div { class: "rect rect-d" }
                        div { class: "rect rect-e" }
                        div { class: "rect rect-f" }
                        div { class: "rect rect-g" }
                        div { class: "rect rect-h" }
                        div { class: "rect rect-a" }
                        div { class: "rect rect-b" }
                        div { class: "rect rect-c" }
                        div { class: "rect rect-d" }
                        div { class: "rect rect-e" }
                        div { class: "rect rect-f" }
                        div { class: "rect rect-g" }
                        div { class: "rect rect-h" }
                        div { class: "abs-div" }
                    }
                }
            }

            div { class: "divider" }

            // ── RIGHT COLUMN: no absolute div ──
            div { class: "column",
                div { class: "label", "Control" }
                div { class: "scroll-area",
                    div { class: "relative-container",
                        div { class: "rect rect-a" }
                        div { class: "rect rect-b" }
                        div { class: "rect rect-c" }
                        div { class: "rect rect-d" }
                        div { class: "rect rect-e" }
                        div { class: "rect rect-f" }
                        div { class: "rect rect-g" }
                        div { class: "rect rect-h" }
                        div { class: "rect rect-a" }
                        div { class: "rect rect-b" }
                        div { class: "rect rect-c" }
                        div { class: "rect rect-d" }
                        div { class: "rect rect-e" }
                        div { class: "rect rect-f" }
                        div { class: "rect rect-g" }
                        div { class: "rect rect-h" }
                    }
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
    doc.inner.borrow().print_tree();

    let conf = miniquad::conf::Conf {
        window_title: "AbsPos Repro".to_string(),
        window_width: 800,
        window_height: 600,
        high_dpi: true,
        ..Default::default()
    };

    miniquad::start(conf, move || {
        Box::new(BlitzMiniquadApp::new(Box::new(doc), proxy, event_queue))
    });
}
