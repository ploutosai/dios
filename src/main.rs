use std::sync::Arc;

use blitz_dom::FontContext;
use blitz_shell_miniquad::{BlitzMiniquadApp, BlitzShellProxy};
use dioxus::prelude::*;
use dioxus_native_dom::{DioxusDocument, DocumentConfig};
use linebender_resource_handle::Blob;

mod app;
mod buffer;
mod clipboard;
mod commands;
mod editor;
mod files;
mod isearch;
mod lsp;
mod overlay;
mod syntax;
mod wrap;

use app::App;

const EDITOR_FONT: &[u8] = include_bytes!("../assets/fonts/LiberationMono-Regular.ttf");

fn document_config() -> DocumentConfig {
    let mut font_ctx = FontContext::default();
    font_ctx
        .collection
        .register_fonts(Blob::new(Arc::new(blitz_dom::BULLET_FONT) as _), None);
    font_ctx
        .collection
        .register_fonts(Blob::new(Arc::new(EDITOR_FONT) as _), None);

    DocumentConfig {
        font_ctx: Some(font_ctx),
        ..Default::default()
    }
}

fn app() -> Element {
    rsx! {
        App {}
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn start_puffin_server() -> Option<puffin_http::Server> {
    puffin::set_scopes_on(true);
    let addr = std::env::var("PUFFIN_ADDR").unwrap_or_else(|_| "127.0.0.1:8585".to_string());
    match puffin_http::Server::new(&addr) {
        Ok(server) => {
            eprintln!(
                "puffin server listening on {addr} — run `puffin_viewer --url {addr}` to connect"
            );
            Some(server)
        }
        Err(e) => {
            eprintln!("puffin server failed to start on {addr}: {e}");
            None
        }
    }
}

fn main() {
    // Native only: a tokio runtime so `tokio::time::sleep` (used by Dioxus
    // `spawn` loops) has a reactor to register timers with.
    #[cfg(not(target_arch = "wasm32"))]
    let _rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
    #[cfg(not(target_arch = "wasm32"))]
    let _rt_guard = _rt.enter();

    // Start the puffin profiling server (native only). The handle must be
    // kept alive for the lifetime of the program. See docs/profiling.md.
    #[cfg(not(target_arch = "wasm32"))]
    let _puffin_server = start_puffin_server();

    let (proxy, event_queue) = BlitzShellProxy::new();

    // Resize plumbing: miniquad's `resize_event` would otherwise be invisible
    // to Dioxus. The callback bumps a watch channel; a task in the App
    // component mirrors it into a Signal that subscribers can react to.
    #[cfg(not(target_arch = "wasm32"))]
    let (resize_tx, resize_rx) = tokio::sync::watch::channel::<u64>(0);

    let vdom = VirtualDom::new(app);
    #[cfg(not(target_arch = "wasm32"))]
    let vdom = vdom.with_root_context(resize_rx);
    let mut doc = DioxusDocument::new(vdom, document_config());
    doc.inner
        .borrow_mut()
        .add_user_agent_stylesheet(include_str!("styles.css"));
    doc.initial_build();

    let mut conf = miniquad::conf::Conf {
        window_title: "Editor".to_string(),
        window_width: 1200,
        window_height: 700,
        high_dpi: true,
        ..Default::default()
    };
    conf.platform.blocking_event_loop = true;

    miniquad::start(conf, move || {
        let app = BlitzMiniquadApp::new(Box::new(doc), proxy, event_queue);
        #[cfg(not(target_arch = "wasm32"))]
        let app = {
            let mut tick = 0u64;
            app.with_resize_callback(move |_w, _h| {
                tick = tick.wrapping_add(1);
                let _ = resize_tx.send(tick);
            })
        };
        Box::new(app)
    });
}
