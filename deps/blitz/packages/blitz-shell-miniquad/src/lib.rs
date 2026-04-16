//! Blitz application shell using miniquad.
//!
//! This provides a miniquad-based alternative to `blitz-shell` (which uses winit).
//! The key architectural difference is that miniquad owns the event loop and window,
//! whereas winit's event loop is created and managed externally.
//!
//! ## Embedding
//!
//! Use [`BlitzView`] to embed Blitz rendering into any miniquad-based application
//! (including macroquad). For standalone miniquad apps, [`BlitzMiniquadApp`] provides
//! a ready-made `EventHandler` implementation.

mod application;
mod convert_events;
mod event;
pub mod view;

pub use application::BlitzMiniquadApp;
pub use event::{BlitzShellEvent, BlitzShellProxy};
pub use view::BlitzView;

use blitz_traits::shell::ShellProvider;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Configuration for launching a blitz miniquad application.
#[derive(Default)]
pub struct Config {
    pub stylesheets: Vec<String>,
    pub base_url: Option<String>,
}

/// A simple ShellProvider for miniquad that tracks redraw requests.
pub struct MiniquadShellProvider {
    redraw_requested: Arc<AtomicBool>,
}

impl MiniquadShellProvider {
    pub fn new() -> (Self, Arc<AtomicBool>) {
        let flag = Arc::new(AtomicBool::new(false));
        (
            Self {
                redraw_requested: flag.clone(),
            },
            flag,
        )
    }
}

impl ShellProvider for MiniquadShellProvider {
    fn request_redraw(&self) {
        if !self.redraw_requested.swap(true, Ordering::SeqCst) {
            miniquad::window::schedule_update();
        }
    }
    // Other methods use defaults (no-ops)
}
