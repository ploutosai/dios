//! Miniquad application handler for Blitz.
//!
//! A thin wrapper around `BlitzView` that implements `miniquad::EventHandler`.

use crate::event::{BlitzShellEvent, BlitzShellProxy};
use crate::view::BlitzView;

use blitz_dom::Document;
use miniquad::{self, EventHandler, KeyCode, KeyMods, MouseButton};
use std::sync::mpsc::Receiver;

/// The main Blitz application running on miniquad.
///
/// This implements `miniquad::EventHandler` by delegating to an inner `BlitzView`.
/// For embedding in macroquad or other frameworks, use `BlitzView` directly.
pub struct BlitzMiniquadApp {
    pub view: BlitzView,
    on_resize: Option<Box<dyn FnMut(u32, u32)>>,
    puffin_frame_open: bool,
}

impl BlitzMiniquadApp {
    pub fn new(
        doc: Box<dyn Document>,
        proxy: BlitzShellProxy,
        event_queue: Receiver<BlitzShellEvent>,
    ) -> Self {
        let (w, h) = miniquad::window::screen_size();
        Self {
            view: BlitzView::new(doc, proxy, event_queue, w as u32, h as u32),
            on_resize: None,
            puffin_frame_open: false,
        }
    }

    /// Install a callback invoked on every miniquad `resize_event`, after the
    /// view's viewport has been updated. Hosts use this to forward window-size
    /// changes into the application — Blitz/Dioxus has no DOM-level resize
    /// event yet, so without a hook the size change is invisible to app code.
    pub fn with_resize_callback<F>(mut self, callback: F) -> Self
    where
        F: FnMut(u32, u32) + 'static,
    {
        self.on_resize = Some(Box::new(callback));
        self
    }

    fn begin_puffin_frame(&mut self) {
        if !self.puffin_frame_open {
            puffin::GlobalProfiler::lock().new_frame();
            self.puffin_frame_open = true;
        }
    }
}

impl EventHandler for BlitzMiniquadApp {
    fn update(&mut self) {
        // With miniquad's blocking event loop, input callbacks happen before
        // the update/draw pair that presents them. Start the puffin frame at
        // the first callback in that event-loop turn, not blindly at update(),
        // so input + update + draw are grouped together.
        self.begin_puffin_frame();
        puffin::profile_scope!("miniquad::update");
        self.view.update();
    }

    fn draw(&mut self) {
        self.begin_puffin_frame();
        {
            puffin::profile_scope!("miniquad::draw");
            self.view.draw();
        }
        // Puffin has no explicit "finish frame" API; `new_frame` both closes
        // the current frame and starts the next one. Do it after draw so the
        // input/update/draw frame doesn't stay open across idle time.
        puffin::GlobalProfiler::lock().new_frame();
        self.puffin_frame_open = false;
    }

    fn resize_event(&mut self, width: f32, height: f32) {
        self.begin_puffin_frame();
        puffin::profile_scope!("miniquad::resize_event");
        let w = width as u32;
        let h = height as u32;
        self.view.resize(w, h);
        if let Some(cb) = &mut self.on_resize {
            cb(w, h);
        }
    }

    fn mouse_motion_event(&mut self, x: f32, y: f32) {
        self.begin_puffin_frame();
        puffin::profile_scope!("miniquad::mouse_motion_event");
        self.view.mouse_motion(x, y);
    }

    fn mouse_wheel_event(&mut self, x: f32, y: f32) {
        self.begin_puffin_frame();
        puffin::profile_scope!("miniquad::mouse_wheel_event");
        self.view.mouse_wheel(x, y);
    }

    fn mouse_button_down_event(&mut self, button: MouseButton, x: f32, y: f32) {
        self.begin_puffin_frame();
        puffin::profile_scope!("miniquad::mouse_button_down_event");
        self.view.mouse_button_down(button, x, y);
    }

    fn mouse_button_up_event(&mut self, button: MouseButton, x: f32, y: f32) {
        self.begin_puffin_frame();
        puffin::profile_scope!("miniquad::mouse_button_up_event");
        self.view.mouse_button_up(button, x, y);
    }

    fn key_down_event(&mut self, keycode: KeyCode, keymods: KeyMods, repeat: bool) {
        self.begin_puffin_frame();
        puffin::profile_scope!("miniquad::key_down_event");
        self.view.key_down(keycode, keymods, repeat);
    }

    fn key_up_event(&mut self, keycode: KeyCode, keymods: KeyMods) {
        self.begin_puffin_frame();
        puffin::profile_scope!("miniquad::key_up_event");
        self.view.key_up(keycode, keymods);
    }

    fn char_event(&mut self, character: char, keymods: KeyMods, _repeat: bool) {
        self.begin_puffin_frame();
        puffin::profile_scope!("miniquad::char_event");
        self.view.char_event(character, keymods);
    }
}
