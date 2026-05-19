//! Embeddable Blitz view that can be used in any miniquad-based application.
//!
//! Unlike `BlitzMiniquadApp`, this does not implement `miniquad::EventHandler`.
//! Instead, it exposes methods that the host application calls to drive
//! the rendering pipeline and forward events.

use crate::convert_events::{create_key_event, mq_mods_to_kbt, mq_mouse_button_to_blitz};
use crate::event::{BlitzShellEvent, BlitzShellProxy};
use crate::MiniquadShellProvider;

use anyrender::WindowRenderer;
use anyrender_nonaquad::NonaquadWindowRenderer;
use blitz_dom::Document;
use blitz_paint::paint_scene;
use blitz_traits::events::{
    BlitzPointerEvent, BlitzPointerId, BlitzWheelDelta, BlitzWheelEvent, KeyState,
    MouseEventButton, MouseEventButtons, PointerCoords, PointerDetails, UiEvent,
};
use blitz_traits::shell::Viewport;

use miniquad::{KeyCode, KeyMods, MouseButton};

/// Apply the press/release of `keycode` to `keymods` if `keycode` is itself
/// a modifier key. X11 reports `XKeyEvent::state` as the modifier mask
/// *before* the event in both directions — so pressing Left Control
/// arrives with `ctrl: false` and releasing it arrives with `ctrl: true`.
/// Mouse and wheel events have no modifier mask of their own and read
/// from the cached `view.modifiers`, so without this correction modifier
/// state would be permanently stale after any modifier press or release.
/// Applying it once at the view boundary keeps every downstream consumer
/// accurate.
fn correct_self_modifier(keycode: KeyCode, mut keymods: KeyMods, pressed: bool) -> KeyMods {
    match keycode {
        KeyCode::LeftShift | KeyCode::RightShift => keymods.shift = pressed,
        KeyCode::LeftControl | KeyCode::RightControl => keymods.ctrl = pressed,
        KeyCode::LeftAlt | KeyCode::RightAlt => keymods.alt = pressed,
        KeyCode::LeftSuper | KeyCode::RightSuper => keymods.logo = pressed,
        _ => {}
    }
    keymods
}

use futures_util::task::{waker_ref, ArcWake};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::task::Context as TaskContext;
use web_time::Instant;

struct MiniquadWaker;

impl ArcWake for MiniquadWaker {
    fn wake_by_ref(_arc_self: &Arc<Self>) {
        miniquad::window::schedule_update();
    }
}

/// An embeddable Blitz HTML/CSS view.
///
/// This can be embedded into any miniquad-based application (including macroquad).
/// The host application is responsible for calling `update()`, `draw()`, and
/// forwarding input events.
pub struct BlitzView {
    pub doc: Box<dyn Document>,
    pub renderer: NonaquadWindowRenderer,
    pub proxy: BlitzShellProxy,
    pub event_queue: Receiver<BlitzShellEvent>,
    pub redraw_flag: Arc<AtomicBool>,
    waker: Arc<MiniquadWaker>,

    // Input state
    buttons: MouseEventButtons,
    pointer_x: f32,
    pointer_y: f32,
    modifiers: KeyMods,
    pending_key_text: Option<char>,
    animation_start: Instant,

    /// When `true`, a mouse-motion event has been received since the last
    /// `update()` but not yet dispatched into the document.  We coalesce all
    /// per-frame motion events into a single dispatch to avoid redundant
    /// hit-testing and VDOM reconciliation (a 1 kHz mouse can easily produce
    /// 60+ events between frames).
    pending_pointer_move: bool,

    // Window state
    width: u32,
    height: u32,
    needs_redraw: bool,
}

impl BlitzView {
    /// Create a new BlitzView that owns its own rendering backend (standalone mode).
    pub fn new(
        doc: Box<dyn Document>,
        proxy: BlitzShellProxy,
        event_queue: Receiver<BlitzShellEvent>,
        width: u32,
        height: u32,
    ) -> Self {
        let renderer = NonaquadWindowRenderer::new_active(width, height);
        Self::new_inner(doc, proxy, event_queue, renderer, width, height)
    }

    /// Create a new BlitzView using an external rendering backend (embedded mode).
    /// Pass macroquad's context via `unsafe { get_internal_gl().quad_context }`.
    pub fn new_shared(
        doc: Box<dyn Document>,
        proxy: BlitzShellProxy,
        event_queue: Receiver<BlitzShellEvent>,
        ctx: &mut dyn miniquad::RenderingBackend,
        width: u32,
        height: u32,
    ) -> Self {
        let renderer = NonaquadWindowRenderer::new_active_shared(ctx, width, height);
        Self::new_inner(doc, proxy, event_queue, renderer, width, height)
    }

    fn new_inner(
        mut doc: Box<dyn Document>,
        proxy: BlitzShellProxy,
        event_queue: Receiver<BlitzShellEvent>,
        renderer: NonaquadWindowRenderer,
        width: u32,
        height: u32,
    ) -> Self {
        let (shell_provider, redraw_flag) = MiniquadShellProvider::new();
        let viewport = Viewport::new(
            width,
            height,
            miniquad::window::dpi_scale(),
            blitz_traits::shell::ColorScheme::Light,
        );

        {
            let mut inner = doc.inner_mut();
            inner.set_viewport(viewport);
            inner.set_shell_provider(Arc::new(shell_provider));
        }

        Self {
            doc,
            renderer,
            proxy,
            event_queue,
            redraw_flag,
            waker: Arc::new(MiniquadWaker),
            buttons: MouseEventButtons::None,
            pointer_x: 0.0,
            pointer_y: 0.0,
            modifiers: KeyMods {
                shift: false,
                ctrl: false,
                alt: false,
                logo: false,
            },
            pending_key_text: None,
            animation_start: Instant::now(),
            pending_pointer_move: false,
            width,
            height,
            needs_redraw: true,
        }
    }

    fn poll_document(&mut self) -> bool {
        let waker = waker_ref(&self.waker);
        let cx = TaskContext::from_waker(&*waker);
        self.doc.poll(Some(cx))
    }

    fn poll_document_until_pending(&mut self) -> bool {
        const MAX_DOCUMENT_POLLS: usize = 8;
        let mut changed = false;
        for _ in 0..MAX_DOCUMENT_POLLS {
            if !self.poll_document() {
                return changed;
            }
            changed = true;
        }
        miniquad::window::schedule_update();
        changed
    }

    /// Process queued events (call once per frame before draw).
    pub fn update(&mut self) {
        puffin::profile_function!();
        while let Ok(event) = self.event_queue.try_recv() {
            match event {
                BlitzShellEvent::Poll => {
                    puffin::profile_scope!("doc.poll (shell event)");
                    if self.poll_document_until_pending() {
                        self.needs_redraw = true;
                    }
                }
                BlitzShellEvent::RequestRedraw { .. } => {
                    self.needs_redraw = true;
                }
                BlitzShellEvent::Embedder(_) => {}
            }
        }

        // Dispatch the coalesced pointer-move (at most one per frame) before
        // polling the VDOM so Dioxus sees the updated position.
        self.flush_pending_pointer_move();

        // Single per-frame reconciliation point.  All event handlers
        // (key_down, char_event, mouse_button_down/up, pointer-move, wheel)
        // call handle_ui_event() to fire Dioxus signal mutations, but defer
        // the expensive VDOM diff + DOM mutation to this single poll().
        // This coalesces e.g. 14 held-key repeats into one reconciliation.
        // `poll()` is a no-op when there is no pending work.
        {
            puffin::profile_scope!("doc.poll (frame)");
            if self.poll_document_until_pending() {
                self.needs_redraw = true;
            }
        }

        if self.redraw_flag.swap(false, Ordering::SeqCst) {
            self.needs_redraw = true;
        }
    }

    /// Render the view. Call after `update()`.
    /// This clears the screen and commits the frame — use for standalone mode.
    pub fn draw(&mut self) {
        puffin::profile_function!();
        // Miniquad owns the buffer swap after EventHandler::draw returns.
        // If we early-return here, it will still swap, exposing whatever stale
        // contents happen to be in the back buffer (old frames / flicker).
        // Blocking mode already prevents draw() from being called while idle,
        // so every scheduled frame must fully repaint.
        let animation_time = self.animation_time();

        let mut inner = self.doc.inner_mut();
        {
            puffin::profile_scope!("blitz: resolve (style + layout)");
            inner.resolve(animation_time);
        }

        let (width, height) = inner.viewport().window_size;
        let scale = inner.viewport().scale_f64();

        {
            puffin::profile_scope!("blitz: renderer.render");
            self.renderer.render(|scene| {
                puffin::profile_scope!("blitz: paint_scene");
                paint_scene(scene, &inner, scale, width, height, 0, 0);
            });
        }

        drop(inner);
        self.needs_redraw = false;
    }

    /// Render the view using an external rendering backend (embedded mode).
    /// Does not clear the screen or commit the frame.
    /// Pass macroquad's context via `unsafe { get_internal_gl().quad_context }`.
    pub fn draw_with_ctx(&mut self, ctx: &mut dyn miniquad::RenderingBackend) {
        self.draw_with_ctx_at(ctx, 0, 0);
    }

    /// Render the view into a shared backend, offset by `x`/`y` logical pixels.
    ///
    /// This is useful for embedding Blitz inside a larger miniquad/macroquad
    /// scene: the document lays out in its own viewport size, then paints at
    /// the requested host-window coordinates.
    pub fn draw_with_ctx_at(
        &mut self,
        ctx: &mut dyn miniquad::RenderingBackend,
        x: u32,
        y: u32,
    ) {
        puffin::profile_function!();
        let animation_time = self.animation_time();

        let mut inner = self.doc.inner_mut();
        {
            puffin::profile_scope!("blitz: resolve (style + layout)");
            inner.resolve(animation_time);
        }

        let (width, height) = inner.viewport().window_size;
        let scale = inner.viewport().scale_f64();

        {
            puffin::profile_scope!("blitz: renderer.render_with_ctx");
            self.renderer.render_with_ctx(ctx, |scene| {
                puffin::profile_scope!("blitz: paint_scene");
                paint_scene(scene, &inner, scale, width, height, x, y);
            });
        }

        drop(inner);
        self.needs_redraw = false;
    }

    /// Returns whether the view needs a redraw.
    pub fn needs_redraw(&self) -> bool {
        self.needs_redraw
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.renderer.set_size(width, height);

        let mut inner = self.doc.inner_mut();
        inner.viewport_mut().window_size = (width, height);
        inner
            .viewport_mut()
            .set_hidpi_scale(miniquad::window::dpi_scale());
        drop(inner);

        self.needs_redraw = true;
        miniquad::window::schedule_update();
    }

    pub fn mouse_motion(&mut self, x: f32, y: f32) {
        puffin::profile_function!();
        self.pointer_x = x;
        self.pointer_y = y;

        // Plain mouse motion should not wake the blocking event loop: it would
        // force a full repaint for every mouse sample.  The editor only needs
        // motion events while dragging/selecting, or while Ctrl/Meta-hover is
        // active for goto-definition affordances.  Wheel/click handlers still
        // use the latest pointer coordinates stored above.
        let motion_can_change_ui = self.buttons != MouseEventButtons::None
            || self.modifiers.ctrl
            || self.modifiers.logo;
        self.pending_pointer_move = motion_can_change_ui;
        if motion_can_change_ui {
            miniquad::window::schedule_update();
        }
    }

    pub fn mouse_wheel(&mut self, x: f32, y: f32) {
        puffin::profile_function!();
        // miniquad reports wheel deltas in coarse "notches" on mice and
        // fractional values on touchpads. Convert to pixels here so the DOM
        // scroller can preserve fractional/smooth deltas instead of snapping
        // everything through a line-step multiplier.
        const WHEEL_LINE_PX: f64 = 102.0;
        let event = UiEvent::Wheel(BlitzWheelEvent {
            delta: BlitzWheelDelta::Pixels(x as f64 * WHEEL_LINE_PX, y as f64 * WHEEL_LINE_PX),
            coords: self.pointer_coords(),
            buttons: self.buttons,
            mods: mq_mods_to_kbt(self.modifiers),
        });
        self.doc.handle_ui_event(event);
        self.needs_redraw = true;
        miniquad::window::schedule_update();
    }

    pub fn mouse_button_down(&mut self, button: MouseButton, x: f32, y: f32) {
        puffin::profile_function!();
        self.pointer_x = x;
        self.pointer_y = y;
        // Flush any pending motion so the pointer-move precedes the click.
        self.flush_pending_pointer_move();
        let btn = mq_mouse_button_to_blitz(button);
        self.buttons |= btn.into();

        let event = UiEvent::PointerDown(BlitzPointerEvent {
            id: BlitzPointerId::Mouse,
            is_primary: true,
            coords: self.pointer_coords(),
            button: btn,
            buttons: self.buttons,
            mods: mq_mods_to_kbt(self.modifiers),
            details: PointerDetails::default(),
        });
        {
            puffin::profile_scope!("doc.handle_ui_event(PointerDown)");
            self.doc.handle_ui_event(event);
        }
        // poll() deferred to update() — coalesced with other events in this frame.
        self.needs_redraw = true;
        miniquad::window::schedule_update();
    }

    pub fn mouse_button_up(&mut self, button: MouseButton, x: f32, y: f32) {
        puffin::profile_function!();
        self.pointer_x = x;
        self.pointer_y = y;
        // Flush any pending motion so the pointer-move precedes the release.
        self.flush_pending_pointer_move();
        let btn = mq_mouse_button_to_blitz(button);
        self.buttons ^= btn.into();

        let event = UiEvent::PointerUp(BlitzPointerEvent {
            id: BlitzPointerId::Mouse,
            is_primary: true,
            coords: self.pointer_coords(),
            button: btn,
            buttons: self.buttons,
            mods: mq_mods_to_kbt(self.modifiers),
            details: PointerDetails::default(),
        });
        {
            puffin::profile_scope!("doc.handle_ui_event(PointerUp)");
            self.doc.handle_ui_event(event);
        }
        // poll() deferred to update() — coalesced with other events in this frame.
        self.needs_redraw = true;
        miniquad::window::schedule_update();
    }

    pub fn key_down(&mut self, keycode: KeyCode, keymods: KeyMods, repeat: bool) {
        puffin::profile_function!();
        let keymods = correct_self_modifier(keycode, keymods, true);
        self.modifiers = keymods;
        let key_text = self.pending_key_text.take();
        let key_event = create_key_event(keycode, keymods, KeyState::Pressed, repeat, key_text);
        {
            puffin::profile_scope!("doc.handle_ui_event(KeyDown)");
            self.doc.handle_ui_event(UiEvent::KeyDown(key_event));
        }
        // poll() deferred to update() — multiple key events per frame are
        // coalesced into a single VDOM reconciliation pass.
        self.needs_redraw = true;
        miniquad::window::schedule_update();
    }

    pub fn key_up(&mut self, keycode: KeyCode, keymods: KeyMods) {
        puffin::profile_function!();
        let keymods = correct_self_modifier(keycode, keymods, false);
        self.modifiers = keymods;
        let key_event = create_key_event(keycode, keymods, KeyState::Released, false, None);
        self.doc.handle_ui_event(UiEvent::KeyUp(key_event));
        miniquad::window::schedule_update();
    }

    pub fn char_event(&mut self, character: char, keymods: KeyMods) {
        puffin::profile_function!();
        self.modifiers = keymods;

        // Filter out events that aren't real text input. miniquad's
        // `char_event` fires for *every* keystroke that produces a code
        // point on the host — including control characters (Backspace =
        // 0x08, Enter = 0x0A/0x0D, Tab = 0x09, Esc = 0x1B, Delete = 0x7F)
        // and Ctrl-modified shortcuts (Ctrl-A → 'a'). If we forwarded
        // those as IME Commit, an `<input>` would re-insert the BS char
        // right after KeyDown deleted, and Ctrl-A would type the letter.
        let code = character as u32;
        let is_control = code < 0x20 || code == 0x7f;
        let has_command_mod = keymods.ctrl || keymods.alt || keymods.logo;
        if is_control || has_command_mod {
            self.pending_key_text = None;
            return;
        }

        // On X11, miniquad delivers `char_event` before the matching
        // `key_down_event`. Preserve that translated X11 text so KeyDown can
        // expose `KeyboardEvent.key` as the actual produced character (e.g.
        // Shift+3 -> "#") while keeping `KeyboardEvent.code` as the physical
        // key. Other backends that deliver KeyDown first simply won't use this.
        self.pending_key_text = Some(character);

        let event = UiEvent::Ime(blitz_traits::events::BlitzImeEvent::Commit(
            character.to_string(),
        ));
        {
            puffin::profile_scope!("doc.handle_ui_event(Ime)");
            self.doc.handle_ui_event(event);
        }
        // poll() deferred to update() — multiple char events per frame are
        // coalesced into a single VDOM reconciliation pass.
        self.needs_redraw = true;
        miniquad::window::schedule_update();
    }

    /// Dispatch the coalesced pointer-move event if one is pending.
    ///
    /// This is called once per frame from `update()`, and also before
    /// `mouse_button_down` / `mouse_button_up` to preserve event ordering
    /// (a move must precede the click at its position).
    fn flush_pending_pointer_move(&mut self) {
        if !self.pending_pointer_move {
            return;
        }
        self.pending_pointer_move = false;

        puffin::profile_scope!("flush_pending_pointer_move");
        let event = UiEvent::PointerMove(BlitzPointerEvent {
            id: BlitzPointerId::Mouse,
            is_primary: true,
            coords: self.pointer_coords(),
            button: MouseEventButton::default(),
            buttons: self.buttons,
            mods: mq_mods_to_kbt(self.modifiers),
            details: PointerDetails::default(),
        });
        {
            puffin::profile_scope!("doc.handle_ui_event(PointerMove)");
            self.doc.handle_ui_event(event);
        }
        // Do not unconditionally mark the frame dirty.  PointerMove is often
        // just a new mouse position; DOM hover changes request redraw via the
        // shell provider, and Dioxus signal mutations are picked up by the
        // following poll_document_until_pending() call.
    }

    // Private helpers

    fn animation_time(&self) -> f64 {
        Instant::now()
            .duration_since(self.animation_start)
            .as_secs_f64()
    }

    fn pointer_coords(&self) -> PointerCoords {
        PointerCoords {
            screen_x: self.pointer_x,
            screen_y: self.pointer_y,
            client_x: self.pointer_x,
            client_y: self.pointer_y,
            page_x: self.pointer_x,
            page_y: self.pointer_y,
        }
    }
}
