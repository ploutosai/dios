//! Repro for the undo-tree overlay scroll/clip bug.
//!
//! The overlay-box mimics the production layout:
//!   - flex column with explicit `height: 70vh`
//!   - prompt header (fixed-height row)
//!   - scroll viewport (`flex: 1; min-height: 0; overflow-y: auto`)
//!   - inside the viewport, a `position: relative` canvas sized via inline
//!     `width`/`height` to deliberately exceed the viewport
//!   - inside the canvas, several `position: absolute` "dots"
//!
//! Expected: the scroll viewport scrolls vertically with the wheel; nodes
//! that fall outside the viewport are clipped.
//!
//! Observed in production: no scroll, and dots beyond the viewport leak
//! out under the overlay-box.
//!
//! Click anywhere on the dark backdrop to dismiss-and-reopen the overlay
//! (so we can re-test fresh).
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
    overflow: hidden;
}

#editor {
    width: 100%;
    height: 100%;
    background: #313244;
    color: #f5c2e7;
    padding: 16px;
    /* Long-enough text to make leaks under the overlay visible. */
    line-height: 22px;
    font-size: 14px;
    white-space: pre;
}

.overlay {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 8vh;
    background: rgba(0, 0, 0, 0.32);
    z-index: 100;
}

.overlay-box {
    background: #cdd6f4;
    color: #1e1e2e;
    border: 2px solid #45475a;
    width: 80%;
    max-width: 1100px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.3);
    height: 70vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    position: relative;
}

.overlay-prompt {
    padding: 8px 14px;
    background: #bcc0cc;
    border-bottom: 1px solid #45475a;
    height: 40px;
    min-height: 40px;
    max-height: 40px;
    display: flex;
    align-items: center;
    flex-shrink: 0;
    color: #1e1e2e;
    font-weight: bold;
}

.scroll-viewport {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    padding: 18px 0;
    background: #d5d7e0;
    /* Force the scroll viewport to also be the positioning ancestor for the
       absolute children. Without this Blitz paints the dots past the
       viewport's clip rect even though their containing block (.canvas)
       sits inside this overflow:auto box. */
    position: relative;
    /* Force a stacking context + paint containment. `position: relative`
       alone wasn't enough for Blitz to clip absolutely-positioned
       descendants to this overflow box — explicitly opting in here makes
       the viewport an isolated paint root. */
    isolation: isolate;
    contain: paint;
    z-index: 0;
}

.canvas {
    position: relative;
    display: block;
    margin: 0 auto !important;
    background: #eff1f5;
}

.node {
    position: absolute;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    border: 2px solid #6c7086;
    background: #cdd6f4;
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.22);
}

.node-core {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: #6c7086;
    margin: 4px auto !important;
}
"#;

const CANVAS_W: f64 = 200.0;
const CANVAS_H: f64 = 3000.0;
const NODE_COUNT: usize = 40;

fn app() -> Element {
    rsx! {
        // Background "editor": pile of horizontal stripes so leaks past the
        // overlay are obvious.
        div { id: "editor",
            for i in 0..80 {
                div { "row {i}: leaks-show-up-here-leaks-show-up-here-leaks-show-up-here-leaks-show-up-here" }
            }
        }

        // Modal overlay matching the undo-tree structure.
        div { class: "overlay",
            div { class: "overlay-box",
                div { class: "overlay-prompt", "Repro: should scroll and clip" }
                div { class: "scroll-viewport",
                    div {
                        class: "canvas",
                        style: "width: {CANVAS_W}px; height: {CANVAS_H}px;",
                        for i in 0..NODE_COUNT {
                            {
                                let y = 30.0 + (i as f64) * 70.0;
                                let left = if i % 2 == 0 { 60.0 } else { 110.0 };
                                rsx! {
                                    div {
                                        class: "node",
                                        style: "left: {left}px; top: {y}px;",
                                        div { class: "node-core" }
                                    }
                                }
                            }
                        }
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
        window_title: "AbsPos Overlay Scroll Repro".to_string(),
        window_width: 1200,
        window_height: 800,
        high_dpi: true,
        ..Default::default()
    };

    miniquad::start(conf, move || {
        Box::new(BlitzMiniquadApp::new(Box::new(doc), proxy, event_queue))
    });
}
