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
    pending_resize: Option<(u32, u32)>,
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
            pending_resize: None,
        }
    }

    /// Install a callback invoked from `update()` for every queued resize,
    /// after the view's viewport has been updated. Hosts use this to forward window-size
    /// changes into the application — Blitz/Dioxus has no DOM-level resize
    /// event yet, so without a hook the size change is invisible to app code.
    pub fn with_resize_callback<F>(mut self, callback: F) -> Self
    where
        F: FnMut(u32, u32) + 'static,
    {
        self.on_resize = Some(Box::new(callback));
        self
    }

}

impl EventHandler for BlitzMiniquadApp {
    fn update(&mut self) {
        puffin::GlobalProfiler::lock().begin_frame();

        // 
        {
            puffin::profile_scope!("miniquad::update");
            if let Some((w, h)) = self.pending_resize.take() {
                self.view.resize_without_scheduling(w, h);
                if let Some(cb) = &mut self.on_resize {
                    cb(w, h);
                }
            }
            self.view.update();
            
            {
                puffin::profile_scope!("miniquad::draw");
                self.view.draw();
            }
        }
        puffin::GlobalProfiler::lock().end_frame();
    }

    fn draw(&mut self) {}

    fn resize_event(&mut self, width: f32, height: f32) {
        self.pending_resize = Some((width as u32, height as u32));
        miniquad::window::schedule_update();
    }

    fn window_restored_event(&mut self) {
        // Miniquad reports focus gain as a "restored" event on several
        // platforms. In blocking-event-loop mode, explicitly wake the loop so
        // the window repaints when focus returns.
        miniquad::window::schedule_update();
    }

    fn mouse_motion_event(&mut self, x: f32, y: f32) {
        self.view.mouse_motion(x, y);
    }

    fn mouse_wheel_event(&mut self, x: f32, y: f32) {
        self.view.mouse_wheel(x, y);
    }

    fn mouse_button_down_event(&mut self, button: MouseButton, x: f32, y: f32) {
        self.view.mouse_button_down(button, x, y);
    }

    fn mouse_button_up_event(&mut self, button: MouseButton, x: f32, y: f32) {
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
}
