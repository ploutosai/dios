//! Macroquad integration for embedding the Dios editor.

use std::sync::{mpsc, Arc};

use ::macroquad::input::utils::{register_input_subscriber, repeat_all_miniquad_input};
use ::macroquad::window::miniquad::{
    self, EventHandler, KeyCode, KeyMods, MouseButton, TouchPhase,
};
use blitz_dom::FontContext;
use blitz_shell_miniquad::{BlitzShellProxy, BlitzView};
use dioxus::prelude::*;
use dioxus_native_dom::{DioxusDocument, DocumentConfig};
use linebender_resource_handle::Blob;

use crate::app::{AppCtx, Pane, SearchDir};
use crate::buffer::Buffer;
use crate::editor::CodeEditor;
use crate::lsp::LspManager;
use crate::overlay::{OverlayView, RightPaneView};

const EDITOR_FONT: &[u8] = include_bytes!("../../assets/fonts/LiberationMono-Regular.ttf");

/// Editor rectangle in macroquad logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl EditorRect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn size_u32(self) -> (u32, u32) {
        (
            self.width.max(1.0).round() as u32,
            self.height.max(1.0).round() as u32,
        )
    }

    fn paint_origin_u32(self) -> (u32, u32) {
        (
            self.x.max(0.0).round() as u32,
            self.y.max(0.0).round() as u32,
        )
    }
}

/// Configuration for a macroquad-hosted editor.
#[derive(Clone, Debug)]
pub struct EditorConfig {
    pub buffer_name: String,
    pub initial_text: String,
    pub font_size_px: f32,
    pub line_height_px: f32,
    /// Additional CSS appended after Dios' editor stylesheet and the default
    /// macroquad embedding stylesheet.
    pub extra_css: String,
}

impl EditorConfig {
    pub fn new(buffer_name: impl Into<String>, initial_text: impl Into<String>) -> Self {
        Self {
            buffer_name: buffer_name.into(),
            initial_text: initial_text.into(),
            font_size_px: 18.0,
            line_height_px: 28.0,
            extra_css: String::new(),
        }
    }

    pub fn with_font_metrics(mut self, font_size_px: f32, line_height_px: f32) -> Self {
        self.font_size_px = font_size_px;
        self.line_height_px = line_height_px;
        self
    }

    pub fn with_extra_css(mut self, css: impl Into<String>) -> Self {
        self.extra_css = css.into();
        self
    }
}

#[derive(Clone)]
struct InitialBuffer {
    name: String,
    text: String,
}

#[derive(Clone)]
struct TextUpdateTx(mpsc::Sender<String>);

/// A ready-to-use Dios editor component for macroquad apps.
///
/// Typical frame usage:
///
/// ```ignore
/// let mut editor = dios::embedded::macroquad::Editor::new("main.rs", source);
/// loop {
///     editor.update();
///     if let Some(text) = editor.take_changed_text() {
///         // react to the edited text
///     }
///
///     // draw your macroquad scene here...
///
///     editor.draw(20.0, 20.0, 600.0, 680.0);
///     macroquad::prelude::next_frame().await;
/// }
/// ```
pub struct Editor {
    view: BlitzView,
    input_subscriber: usize,
    text_rx: mpsc::Receiver<String>,
    latest_text: String,
    pending_text: Option<String>,
    rect: EditorRect,
    last_size: (u32, u32),
    #[cfg(not(target_arch = "wasm32"))]
    resize_tx: tokio::sync::watch::Sender<u64>,
    resize_tick: u64,
    #[cfg(not(target_arch = "wasm32"))]
    runtime: tokio::runtime::Runtime,
}

impl Editor {
    /// Create an editor with default styling.
    pub fn new(buffer_name: impl Into<String>, initial_text: impl Into<String>) -> Self {
        Self::with_config(EditorConfig::new(buffer_name, initial_text))
    }

    /// Create an editor with explicit configuration.
    ///
    /// Call this after macroquad has initialized its window (inside the
    /// `#[macroquad::main]` async function, before the frame loop is fine).
    pub fn with_config(config: EditorConfig) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let runtime = tokio::runtime::Runtime::new().expect("create dios embedded tokio runtime");

        let (text_tx, text_rx) = mpsc::channel::<String>();
        let input_subscriber = register_input_subscriber();
        let latest_text = config.initial_text.clone();

        #[cfg(not(target_arch = "wasm32"))]
        let (view, resize_tx) = {
            let _guard = runtime.enter();
            build_view(&config, text_tx)
        };

        #[cfg(target_arch = "wasm32")]
        let view = build_view(&config, text_tx);

        let (w, h) = miniquad::window::screen_size();
        let rect = EditorRect::new(0.0, 0.0, w, h);

        Self {
            view,
            input_subscriber,
            text_rx,
            latest_text,
            pending_text: None,
            rect,
            last_size: rect.size_u32(),
            #[cfg(not(target_arch = "wasm32"))]
            resize_tx,
            resize_tick: 0,
            #[cfg(not(target_arch = "wasm32"))]
            runtime,
        }
    }

    /// Forward input, process Dioxus/Blitz work, and drain text changes.
    /// Call once near the start of each macroquad frame.
    pub fn update(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        let _guard = self.runtime.enter();

        {
            let mut forwarder = BlitzInputForwarder {
                view: &mut self.view,
                rect: self.rect,
            };
            repeat_all_miniquad_input(&mut forwarder, self.input_subscriber);
        }

        self.view.update();
        self.drain_text_updates();
    }

    /// Draw the editor on top of whatever the host has already batched.
    ///
    /// This flushes macroquad's current draw queue before handing the shared
    /// miniquad context to Blitz. Call it after drawing your scene and before
    /// `next_frame().await`.
    pub fn draw(&mut self, x: f32, y: f32, width: f32, height: f32) {
        self.draw_rect(EditorRect::new(x, y, width, height));
    }

    /// Draw using an [`EditorRect`].
    pub fn draw_rect(&mut self, rect: EditorRect) {
        #[cfg(not(target_arch = "wasm32"))]
        let _guard = self.runtime.enter();

        self.set_rect(rect);

        let mut gl = unsafe { ::macroquad::window::get_internal_gl() };
        gl.flush();
        let (x, y) = self.rect.paint_origin_u32();
        self.view.draw_with_ctx_at(gl.quad_context, x, y);
    }

    /// Return the latest edited text if it changed since the previous call.
    pub fn take_changed_text(&mut self) -> Option<String> {
        self.pending_text.take()
    }

    /// Latest text observed from the editor.
    pub fn latest_text(&self) -> &str {
        &self.latest_text
    }

    pub fn needs_redraw(&self) -> bool {
        self.view.needs_redraw()
    }

    fn set_rect(&mut self, rect: EditorRect) {
        self.rect = rect;
        let size = rect.size_u32();
        if size == self.last_size {
            return;
        }
        self.last_size = size;
        // `resize` only updates viewport state and schedules a future frame;
        // Dioxus polling remains centralized in `update()`, and Blitz resolve
        // remains centralized in `draw*()`.
        self.view.resize(size.0, size.1);
        self.resize_tick = self.resize_tick.wrapping_add(1);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = self.resize_tx.send(self.resize_tick);
    }

    fn drain_text_updates(&mut self) {
        while let Ok(text) = self.text_rx.try_recv() {
            if text != self.latest_text {
                self.latest_text = text.clone();
                self.pending_text = Some(text);
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn build_view(
    config: &EditorConfig,
    text_tx: mpsc::Sender<String>,
) -> (BlitzView, tokio::sync::watch::Sender<u64>) {
    let (resize_tx, resize_rx) = tokio::sync::watch::channel::<u64>(0);
    let view = build_view_inner(config, text_tx, resize_rx);
    (view, resize_tx)
}

#[cfg(target_arch = "wasm32")]
fn build_view(config: &EditorConfig, text_tx: mpsc::Sender<String>) -> BlitzView {
    build_view_inner(config, text_tx)
}

#[cfg(not(target_arch = "wasm32"))]
fn build_view_inner(
    config: &EditorConfig,
    text_tx: mpsc::Sender<String>,
    resize_rx: tokio::sync::watch::Receiver<u64>,
) -> BlitzView {
    let (proxy, event_queue) = BlitzShellProxy::new();

    let initial = InitialBuffer {
        name: config.buffer_name.clone(),
        text: config.initial_text.clone(),
    };

    let mut vdom = VirtualDom::new(EmbeddedEditorApp)
        .with_root_context(initial)
        .with_root_context(TextUpdateTx(text_tx));
    vdom = vdom.with_root_context(resize_rx);

    let mut doc = DioxusDocument::new(vdom, document_config());
    doc.inner
        .borrow_mut()
        .add_user_agent_stylesheet(include_str!("../styles.css"));
    let css = embedded_css(config);
    doc.inner.borrow_mut().add_user_agent_stylesheet(&css);
    if !config.extra_css.is_empty() {
        doc.inner
            .borrow_mut()
            .add_user_agent_stylesheet(&config.extra_css);
    }
    doc.initial_build();

    let (w, h) = miniquad::window::screen_size();
    let gl = unsafe { ::macroquad::window::get_internal_gl() };
    BlitzView::new_shared(
        Box::new(doc),
        proxy,
        event_queue,
        gl.quad_context,
        w as u32,
        h as u32,
    )
}

#[cfg(target_arch = "wasm32")]
fn build_view_inner(config: &EditorConfig, text_tx: mpsc::Sender<String>) -> BlitzView {
    let (proxy, event_queue) = BlitzShellProxy::new();

    let initial = InitialBuffer {
        name: config.buffer_name.clone(),
        text: config.initial_text.clone(),
    };

    let vdom = VirtualDom::new(EmbeddedEditorApp)
        .with_root_context(initial)
        .with_root_context(TextUpdateTx(text_tx));

    let mut doc = DioxusDocument::new(vdom, document_config());
    doc.inner
        .borrow_mut()
        .add_user_agent_stylesheet(include_str!("../styles.css"));
    let css = embedded_css(config);
    doc.inner.borrow_mut().add_user_agent_stylesheet(&css);
    if !config.extra_css.is_empty() {
        doc.inner
            .borrow_mut()
            .add_user_agent_stylesheet(&config.extra_css);
    }
    doc.initial_build();

    let (w, h) = miniquad::window::screen_size();
    let gl = unsafe { ::macroquad::window::get_internal_gl() };
    BlitzView::new_shared(
        Box::new(doc),
        proxy,
        event_queue,
        gl.quad_context,
        w as u32,
        h as u32,
    )
}

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

#[component]
fn EmbeddedEditorApp() -> Element {
    let initial = use_context::<InitialBuffer>();
    let initial_for_buffer = initial.clone();

    let ctx = AppCtx {
        buffers: use_signal(move || {
            vec![Buffer::with_name(
                &initial_for_buffer.text,
                initial_for_buffer.name.clone(),
                None,
            )]
        }),
        current: use_signal(|| 0usize),
        overlay: use_signal(|| None),
        right_pane: use_signal(|| None),
        right_pane_selected: use_signal(|| 0usize),
        focus: use_signal(|| Pane::Left),
        editor_el: use_signal(|| None),
        right_pane_el: use_signal(|| None),
        ck_prefix: use_signal(|| false),
        minibuf_msg: use_signal(String::new),
        project_root: use_signal(|| None),
        file_index: use_signal(Vec::new),
        isearch: use_signal(|| None),
        isearch_history: use_signal(Vec::new),
        resize_tick: use_signal(|| 0u64),
        scroll_to_cursor_tick: use_signal(|| 0u64),
        lsp: use_signal(LspManager::new),
        lsp_tick: use_signal(|| 0u64),
        nav_history: use_signal(Vec::new),
        scroll_top: use_signal(|| 0.0f64),
        restore_scroll_tick: use_signal(|| 0u64),
        completion: use_signal(|| None),
    };
    use_context_provider(|| ctx);

    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut resize_tick = ctx.resize_tick;
        let rx = use_context::<tokio::sync::watch::Receiver<u64>>();
        use_effect(move || {
            let mut rx = rx.clone();
            spawn(async move {
                while rx.changed().await.is_ok() {
                    resize_tick.set(*rx.borrow_and_update());
                }
            });
        });
    }

    {
        let tx = use_context::<TextUpdateTx>();
        let buffers = ctx.buffers;
        let current = ctx.current;
        let mut last_sent = use_signal(|| None::<(usize, u64)>);
        use_effect(move || {
            let idx = *current.read();
            let (version, text) = {
                let bufs = buffers.read();
                let Some(buf) = bufs.get(idx) else { return };
                (buf.version, buf.text())
            };
            let key = (idx, version);
            if *last_sent.peek() != Some(key) {
                last_sent.set(Some(key));
                let _ = tx.0.send(text);
            }
        });
    }

    let show_right = ctx.right_pane.read().is_some();
    let has_overlay = ctx.overlay.read().is_some();

    rsx! {
        div { id: "app-root",
            div { id: "main-split",
                class: if show_right { "split-active" } else { "split-single" },
                CodeEditor {}
                if show_right {
                    RightPaneView {}
                }
            }
            EmbeddedMinibuffer {}
            if has_overlay {
                OverlayView {}
            }
        }
    }
}

#[component]
fn EmbeddedMinibuffer() -> Element {
    let ctx: AppCtx = use_context();
    let msg = ctx.minibuf_msg.read().clone();
    let ck = *ctx.ck_prefix.read();
    let isearch = ctx.isearch.read().clone();

    if let Some(s) = isearch {
        let label = match s.direction {
            SearchDir::Forward => "I-search",
            SearchDir::Backward => "I-search backward",
        };
        let failing = !s.query.is_empty() && s.matches.is_empty();
        let prefix = if failing { "Failing " } else { "" };
        return rsx! {
            div { id: "minibuffer",
                span { class: "mb-search-label", "{prefix}{label}: " }
                span { class: "mb-search-query", "{s.query}" }
                span { class: "mb-search-caret", " " }
            }
        };
    }

    let display = if ck {
        "C-k ".to_string()
    } else if !msg.is_empty() {
        msg
    } else {
        let bufs = ctx.buffers.read();
        let idx = *ctx.current.read();
        let (name, dirty) = bufs
            .get(idx)
            .map(|b| (b.name.clone(), b.dirty))
            .unwrap_or_default();
        let prefix = if dirty { "*" } else { "" };
        format!("{prefix}{name}")
    };

    rsx! {
        div { id: "minibuffer", "{display}" }
    }
}

struct BlitzInputForwarder<'a> {
    view: &'a mut BlitzView,
    rect: EditorRect,
}

impl BlitzInputForwarder<'_> {
    fn local(&self, x: f32, y: f32) -> (f32, f32) {
        let scale = miniquad::window::dpi_scale();
        (x / scale - self.rect.x, y / scale - self.rect.y)
    }
}

impl EventHandler for BlitzInputForwarder<'_> {
    fn update(&mut self) {}
    fn draw(&mut self) {}

    fn resize_event(&mut self, _width: f32, _height: f32) {
        // Component resize is driven by Editor::draw(_x, _y, width, height),
        // not by the host window's full size.
    }

    fn mouse_motion_event(&mut self, x: f32, y: f32) {
        let (x, y) = self.local(x, y);
        self.view.mouse_motion(x, y);
    }

    fn mouse_wheel_event(&mut self, x: f32, y: f32) {
        let (mx, my) = ::macroquad::input::mouse_position();
        let (mx, my) = self.local(mx, my);
        self.view.mouse_motion(mx, my);
        self.view.mouse_wheel(x, y);
    }

    fn mouse_button_down_event(&mut self, button: MouseButton, x: f32, y: f32) {
        let (x, y) = self.local(x, y);
        self.view.mouse_button_down(button, x, y);
    }

    fn mouse_button_up_event(&mut self, button: MouseButton, x: f32, y: f32) {
        let (x, y) = self.local(x, y);
        self.view.mouse_button_up(button, x, y);
    }

    fn key_down_event(&mut self, keycode: KeyCode, keymods: KeyMods, repeat: bool) {
        self.view.key_down(keycode, keymods, repeat);
    }

    fn key_up_event(&mut self, keycode: KeyCode, keymods: KeyMods) {
        self.view.key_up(keycode, keymods);
    }

    fn char_event(&mut self, character: char, keymods: KeyMods, _repeat: bool) {
        self.view.char_event(character, keymods);
    }

    fn touch_event(&mut self, _phase: TouchPhase, _id: u64, _x: f32, _y: f32) {}
}

fn embedded_css(config: &EditorConfig) -> String {
    let cursor_h = (config.line_height_px - 2.0).max(1.0);
    let gutter_w = (config.font_size_px * 3.0).max(44.0);
    let line_num_size = (config.font_size_px - 4.0).max(10.0);
    let status_h = (config.line_height_px - 4.0).max(18.0);
    let minibuf_h = (config.line_height_px - 2.0).max(20.0);
    let minibuf_line_h = (minibuf_h - 6.0).max(14.0);
    let status_font = (config.font_size_px - 4.0).max(10.0);
    let minibuf_font = (config.font_size_px - 4.0).max(10.0);

    format!(
        r#"
html, body, main, #main {{
    background: transparent;
}}

#app-root {{
    position: absolute;
    left: 0;
    top: 0;
    width: 100%;
    height: 100%;
    border: 2px solid rgba(210, 205, 180, 0.85);
    box-shadow: 0 16px 40px rgba(0, 0, 0, 0.40);
    overflow: hidden;
}}

#editor-panel {{
    font-size: {font_size}px;
    line-height: {line_height}px;
}}

#measure-line {{
    font-size: {font_size}px;
    line-height: {line_height}px;
}}

#highlight-layer {{
    font-size: {font_size}px;
    line-height: {line_height}px;
}}

.code-line {{
    height: {line_height}px;
    line-height: {line_height}px;
    font-size: {font_size}px;
}}

.selection-rect,
.search-rect,
.ctrl-hover-rect {{
    height: {line_height}px;
}}

.line-num {{
    height: {line_height}px;
    line-height: {line_height}px;
    font-size: {line_num_size}px;
}}

#gutter {{
    min-width: {gutter_w}px;
    width: {gutter_w}px;
}}

#cursor {{
    width: 2px;
    height: {cursor_h}px;
}}

#status-bar {{
    height: {status_h}px;
    min-height: {status_h}px;
    max-height: {status_h}px;
    line-height: {status_h}px;
    font-size: {status_font}px;
}}

#minibuffer {{
    height: {minibuf_h}px;
    min-height: {minibuf_h}px;
    max-height: {minibuf_h}px;
    line-height: {minibuf_line_h}px;
    font-size: {minibuf_font}px;
    padding: 3px 10px;
}}
"#,
        font_size = config.font_size_px,
        line_height = config.line_height_px,
        line_num_size = line_num_size,
        gutter_w = gutter_w,
        cursor_h = cursor_h,
        status_h = status_h,
        status_font = status_font,
        minibuf_h = minibuf_h,
        minibuf_line_h = minibuf_line_h,
        minibuf_font = minibuf_font,
    )
}
