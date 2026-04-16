use blitz_dom::DocumentConfig;
use blitz_html::HtmlDocument;
use blitz_shell_miniquad::{BlitzMiniquadApp, BlitzShellProxy};

fn main() {
    let html = r#"
    <!DOCTYPE html>
    <html>
    <head>
        <style>
            body {
                font-family: sans-serif;
                background: #1e1e2e;
                color: #cdd6f4;
                margin: 0;
                padding: 20px;
            }
            * { outline: none; border: none !important; }
            h1 {
                color: #89b4fa;
                border-bottom: 2px solid #45475a;
                padding-bottom: 10px;
            }
            .container {
                max-width: 800px;
                margin: 0 auto;
                background: #313244;
                border-radius: 12px;
                padding: 24px;
                box-shadow: 0 4px 16px rgba(0, 0, 0, 0.3);
            }
            p {
                line-height: 1.6;
            }
            .highlight {
                color: #f9e2af;
                font-weight: bold;
            }
            button {
                background: #89b4fa;
                color: #1e1e2e;
                border: none;
                padding: 10px 20px;
                border-radius: 8px;
                font-size: 16px;
                cursor: pointer;
            }
        </style>
    </head>
    <body>
        <div class="container">
            <h1>Blitz on Miniquad</h1>
            <p>This is a <span class="highlight">Blitz</span> web engine rendering HTML/CSS
               using the <span class="highlight">nonaquad</span> backend (NanoVG on miniquad).</p>
            <p>The rendering pipeline is: HTML/CSS → Stylo → Taffy → blitz-paint → anyrender → nonaquad → miniquad (OpenGL)</p>
            <button>Click me!</button>
        </div>
    </body>
    </html>
    "#;

    let (proxy, event_queue) = BlitzShellProxy::new();

    let doc = HtmlDocument::from_html(
        html,
        DocumentConfig {
            ..Default::default()
        },
    );

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
