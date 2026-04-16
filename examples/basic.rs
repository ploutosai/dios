use blitz_shell_miniquad::{BlitzMiniquadApp, BlitzShellProxy};
use dioxus::prelude::*;
use dioxus_native_dom::{DioxusDocument, DocumentConfig};

fn app() -> Element {
    rsx! {
        style {
            r#"
            body {{
                font-family: sans-serif;
                background: #1e1e2e;
                color: #cdd6f4;
                margin: 0;
                padding: 20px;
            }}
            * {{ outline: none; border: none !important; }}
            h1 {{
                color: #89b4fa;
                border-bottom: 2px solid #45475a;
                padding-bottom: 10px;
            }}
            .container {{
                max-width: 800px;
                margin: 0 auto;
                background: #313244;
                border-radius: 12px;
                padding: 24px;
                box-shadow: 0 4px 16px rgba(0, 0, 0, 0.3);
            }}
            p {{
                line-height: 1.6;
            }}
            .highlight {{
                color: #f9e2af;
                font-weight: bold;
            }}
            button {{
                background: #89b4fa;
                color: #1e1e2e;
                border: none;
                padding: 10px 20px;
                border-radius: 8px;
                font-size: 16px;
                cursor: pointer;
            }}
            "#
        }
        div { class: "container",
            h1 { "Blitz on Miniquad" }
            p {
                "This is a "
                span { class: "highlight", "Blitz" }
                " web engine rendering HTML/CSS using the "
                span { class: "highlight", "nonaquad" }
                " backend (NanoVG on miniquad)."
            }
            p { "The rendering pipeline is: HTML/CSS → Stylo → Taffy → blitz-paint → anyrender → nonaquad → miniquad (OpenGL)" }
            button { "Click me!" }
        }
    }
}

fn main() {
    let (proxy, event_queue) = BlitzShellProxy::new();

    let vdom = VirtualDom::new(app);
    let mut doc = DioxusDocument::new(vdom, DocumentConfig::default());
    doc.initial_build();

    let conf = miniquad::conf::Conf {
        window_title: "Blitz + Miniquad".to_string(),
        window_width: 1024,
        window_height: 768,
        high_dpi: true,
        ..Default::default()
    };

    miniquad::start(conf, move || {
        Box::new(BlitzMiniquadApp::new(Box::new(doc), proxy, event_queue))
    });
}
