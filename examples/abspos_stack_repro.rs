/// Repro for absolute-position paint interaction without horizontal layout noise.
///
/// Three stacked sections:
/// 1. Absolute child before normal-flow rectangles
/// 2. Absolute child after normal-flow rectangles
/// 3. Control with no absolute child
///
/// If Blitz is correct, all three sections should show the same colored
/// rectangles, with only a small red marker overlaid in sections 1 and 2.
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
}

#root {
    display: flex;
    flex-direction: column;
    gap: 16px;
    width: 100%;
    height: 100%;
    padding: 16px;
}

.section {
    display: flex;
    flex-direction: column;
    flex: 1;
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

.scroll-area {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    background: #313244;
}

.relative-container {
    position: relative;
}

.abs-div {
    position: absolute;
    left: 8px;
    top: 8px;
    width: 3px;
    height: 20px;
    background: #f38ba8;
}

.rect {
    height: 40px;
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

fn app() -> Element {
    rsx! {
        div { id: "root",
            div { class: "section",
                div { class: "label", "ABS before" }
                div { class: "scroll-area",
                    div { class: "relative-container",
                        div { class: "abs-div" }
                        Rects {}
                    }
                }
            }

            div { class: "section",
                div { class: "label", "ABS after" }
                div { class: "scroll-area",
                    div { class: "relative-container",
                        Rects {}
                        div { class: "abs-div" }
                    }
                }
            }

            div { class: "section",
                div { class: "label", "Control" }
                div { class: "scroll-area",
                    div { class: "relative-container",
                        Rects {}
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

    let conf = miniquad::conf::Conf {
        window_title: "AbsPos Stack Repro".to_string(),
        window_width: 900,
        window_height: 900,
        high_dpi: true,
        ..Default::default()
    };

    miniquad::start(conf, move || {
        Box::new(BlitzMiniquadApp::new(Box::new(doc), proxy, event_queue))
    });
}
