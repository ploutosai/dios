//! Event types and proxy for the miniquad shell.

use std::any::Any;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use blitz_traits::net::NetWaker;

#[derive(Debug, Clone)]
pub enum BlitzShellEvent {
    /// Request a poll of the document
    Poll,
    /// Request a redraw for a specific document
    RequestRedraw { doc_id: usize },
    /// An arbitrary event from the embedder
    Embedder(Arc<dyn Any + Send + Sync>),
}

impl BlitzShellEvent {
    pub fn embedder_event<T: Any + Send + Sync>(value: T) -> Self {
        Self::Embedder(Arc::new(value))
    }
}

/// A proxy for sending events to the miniquad shell from other threads.
#[derive(Clone)]
pub struct BlitzShellProxy {
    sender: Sender<BlitzShellEvent>,
}

impl BlitzShellProxy {
    pub fn new() -> (Self, Receiver<BlitzShellEvent>) {
        let (sender, receiver) = channel();
        (Self { sender }, receiver)
    }

    pub fn send_event(&self, event: BlitzShellEvent) {
        let _ = self.sender.send(event);
        miniquad::window::schedule_update();
    }
}

impl NetWaker for BlitzShellProxy {
    fn wake(&self, client_id: usize) {
        self.send_event(BlitzShellEvent::RequestRedraw { doc_id: client_id });
    }
}
