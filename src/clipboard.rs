//! Thin wrapper around miniquad's OS clipboard.
//!
//! Native (linux X11 / macOS / Windows) talks to the system clipboard via
//! miniquad's `window::clipboard_get`/`set`. On wasm those functions exist
//! but require a JS round-trip through the host; we don't currently plumb
//! the result back, so paste returns `None` and the caller surfaces a
//! status-bar message.

#[cfg(not(target_arch = "wasm32"))]
pub fn copy(text: &str) {
    miniquad::window::clipboard_set(text);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn paste() -> Option<String> {
    miniquad::window::clipboard_get()
}

#[cfg(target_arch = "wasm32")]
pub fn copy(_text: &str) {}

#[cfg(target_arch = "wasm32")]
pub fn paste() -> Option<String> {
    None
}
