//! Emacs-style incremental search. Keys are captured here while
//! `AppCtx::isearch` is `Some`. Typing extends the query, C-f / C-r step
//! through results in either direction, Esc/C-g cancels (cursor restored to
//! origin), Enter accepts (cursor stays on the current match).
//!
//! Match offsets are recomputed against the rope only when the query changes
//! (typing, backspace) — repeats just step through `matches`. Direction lives
//! on the state purely so the prompt label and wrap-around behavior reflect
//! the user's intent; both C-f and C-r work regardless of how the search was
//! started.

use crate::app::{AppCtx, ISearch, SearchDir};
use crate::buffer::Buffer;
use dioxus::prelude::*;
use lapce_xi_rope::Rope;

/// Find all start offsets where `needle` occurs in `rope`. Plain
/// case-sensitive substring search. Empty needle returns no matches.
pub fn find_all(rope: &Rope, needle: &str) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    let text = String::from(rope);
    text.match_indices(needle).map(|(i, _)| i).collect()
}

/// Pick the initial focused match for a freshly recomputed match list,
/// preferring matches in the search direction relative to `origin`.
fn pick_initial(matches: &[usize], dir: SearchDir, origin: usize) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    match dir {
        SearchDir::Forward => matches
            .iter()
            .position(|&m| m >= origin)
            .or_else(|| Some(0)),
        SearchDir::Backward => matches
            .iter()
            .rposition(|&m| m <= origin)
            .or_else(|| Some(matches.len() - 1)),
    }
}

fn recompute(state: &mut ISearch, rope: &Rope) {
    state.matches = find_all(rope, &state.query);
    state.current = pick_initial(&state.matches, state.direction, state.origin);
}

fn set_query(state: &mut ISearch, query: String, buffers: &mut Signal<Vec<Buffer>>) {
    state.query = query;
    let rope = {
        let bufs = buffers.read();
        bufs[state.buffer].rope.clone()
    };
    recompute(state, &rope);
    apply_cursor(state, buffers);
}

fn remember_query(ctx: AppCtx, query: &str) {
    if query.is_empty() {
        return;
    }
    let mut history = ctx.isearch_history;
    let mut items = history.write();
    if let Some(pos) = items.iter().position(|item| item == query) {
        items.remove(pos);
    }
    items.push(query.to_string());
}

fn browse_history(
    state: &mut ISearch,
    dir: SearchDir,
    ctx: AppCtx,
    buffers: &mut Signal<Vec<Buffer>>,
) {
    if !state.query.is_empty() && state.history_index.is_none() {
        return;
    }

    let history = ctx.isearch_history.read();
    if history.is_empty() {
        return;
    }

    let next = match (state.history_index, dir) {
        (None, SearchDir::Backward) => Some(history.len() - 1),
        (None, SearchDir::Forward) => return,
        (Some(0), SearchDir::Backward) => Some(0),
        (Some(i), SearchDir::Backward) => Some(i - 1),
        (Some(i), SearchDir::Forward) if i + 1 < history.len() => Some(i + 1),
        (Some(_), SearchDir::Forward) => None,
    };

    match next {
        Some(i) => {
            let query = history[i].clone();
            drop(history);
            state.history_index = Some(i);
            set_query(state, query, buffers);
        }
        None => {
            drop(history);
            state.history_index = None;
            set_query(state, String::new(), buffers);
        }
    }
}

fn step(state: &mut ISearch, dir: SearchDir) {
    if state.matches.is_empty() {
        return;
    }
    let n = state.matches.len();
    state.direction = dir;
    state.current = Some(match (state.current, dir) {
        (Some(i), SearchDir::Forward) => (i + 1) % n,
        (Some(i), SearchDir::Backward) => (i + n - 1) % n,
        (None, SearchDir::Forward) => 0,
        (None, SearchDir::Backward) => n - 1,
    });
}

/// Apply the active match (or `origin` if no match) to the buffer cursor.
fn apply_cursor(state: &ISearch, buffers: &mut Signal<Vec<Buffer>>) {
    let Some(idx) = state.current else {
        let mut bufs = buffers.write();
        if let Some(b) = bufs.get_mut(state.buffer) {
            b.move_to_offset(state.origin);
        }
        return;
    };
    let off = state.matches[idx] + state.query.len();
    let mut bufs = buffers.write();
    if let Some(b) = bufs.get_mut(state.buffer) {
        b.move_to_offset(off);
    }
}

/// Restore the buffer to its pre-search cursor & anchor. Called on
/// Esc / C-g.
fn restore(state: &ISearch, buffers: &mut Signal<Vec<Buffer>>) {
    let mut bufs = buffers.write();
    if let Some(b) = bufs.get_mut(state.buffer) {
        b.move_to_offset(state.origin);
        b.selection_anchor = state.origin_anchor;
    }
}

/// Begin a new isearch in the given direction, anchored at the active
/// buffer's current cursor.
pub fn start(ctx: AppCtx, dir: SearchDir) {
    let idx = *ctx.current.read();
    let mut buffers = ctx.buffers;
    let (origin, origin_anchor) = {
        let mut bufs = buffers.write();
        let Some(buf) = bufs.get_mut(idx) else {
            return;
        };
        let origin = buf.offset();
        let origin_anchor = buf.selection_anchor;
        // Hide any existing selection underneath the search highlights;
        // restored on cancel.
        buf.selection_anchor = None;
        (origin, origin_anchor)
    };
    let mut isearch = ctx.isearch;
    isearch.set(Some(ISearch {
        query: String::new(),
        direction: dir,
        buffer: idx,
        origin,
        origin_anchor,
        matches: Vec::new(),
        current: None,
        history_index: None,
    }));
}

/// Returns true if the key was consumed by isearch handling. The caller
/// should also bump `scroll_to_cursor_tick` when this returns true and the
/// cursor moved.
pub fn handle_key(key: &Key, ctrl: bool, ctx: AppCtx) -> bool {
    let mut isearch = ctx.isearch;
    let mut buffers = ctx.buffers;
    let mut minibuf_msg = ctx.minibuf_msg;

    let Some(mut state) = isearch.peek().clone() else {
        return false;
    };

    // Buffer switched out from under us: treat as cancel.
    if *ctx.current.read() != state.buffer {
        isearch.set(None);
        minibuf_msg.set(String::new());
        return false;
    }

    match key {
        // Cancel: restore cursor.
        Key::Escape => {
            restore(&state, &mut buffers);
            isearch.set(None);
            minibuf_msg.set(String::new());
        }
        Key::Character(c) if ctrl && c == "g" => {
            restore(&state, &mut buffers);
            isearch.set(None);
            minibuf_msg.set(String::new());
        }
        // Accept: leave cursor where it is.
        Key::Enter => {
            remember_query(ctx, &state.query);
            isearch.set(None);
            minibuf_msg.set(String::new());
        }
        // Step forward / backward.
        Key::Character(c) if ctrl && c.eq_ignore_ascii_case("f") => {
            // First C-f with empty query just opens the search; subsequent
            // ones step. Same for C-r.
            if state.query.is_empty() {
                state.direction = SearchDir::Forward;
            } else {
                step(&mut state, SearchDir::Forward);
            }
            apply_cursor(&state, &mut buffers);
            isearch.set(Some(state));
        }
        Key::Character(c) if ctrl && c.eq_ignore_ascii_case("r") => {
            if state.query.is_empty() {
                state.direction = SearchDir::Backward;
            } else {
                step(&mut state, SearchDir::Backward);
            }
            apply_cursor(&state, &mut buffers);
            isearch.set(Some(state));
        }
        Key::ArrowUp => {
            browse_history(&mut state, SearchDir::Backward, ctx, &mut buffers);
            isearch.set(Some(state));
        }
        Key::ArrowDown => {
            browse_history(&mut state, SearchDir::Forward, ctx, &mut buffers);
            isearch.set(Some(state));
        }
        Key::Backspace => {
            state.history_index = None;
            state.query.pop();
            let query = state.query.clone();
            set_query(&mut state, query, &mut buffers);
            isearch.set(Some(state));
        }
        Key::Character(c) if !ctrl => {
            state.history_index = None;
            state.query.push_str(c);
            let query = state.query.clone();
            set_query(&mut state, query, &mut buffers);
            isearch.set(Some(state));
        }
        // Any other key (arrows, etc.) is swallowed while in isearch — the
        // user can press Enter or Esc to exit and then move freely.
        _ => {}
    }
    true
}
