/// Code editor: single view with syntax highlighting, cursor, and selection.
use crate::app::{AppCtx, Overlay, Pane, RightPaneState};
use crate::commands;
use crate::overlay::{filter_paths, filter_strings};
use crate::syntax;
use dioxus::html::geometry::ClientPoint;
use dioxus::html::geometry::PixelsVector2D;
use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;
use dioxus_native_dom::NodeHandle;
use std::sync::Arc;

const LAYER_PAD: f64 = 2.0;
const DRAG_AUTOSCROLL_MARGIN: f64 = 18.0;
const MEASURE_CHARS: usize = 100;
const CURSOR_SCROLL_MARGIN_LINES: usize = 0;
/// Lines rendered above/below the visible viewport so fast scrolling does not
/// reveal blank rows before the next scroll event fires.
const RENDER_OVERSCAN: usize = 10;

#[derive(Clone, Copy, Debug)]
struct Metrics {
    char_width: f64,
    line_height: f64,
    header_height: f64,
    gutter_width: f64,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            char_width: 14.42,
            line_height: 38.0,
            header_height: 0.0,
            gutter_width: 78.0,
        }
    }
}

fn max_scroll_top(line_count: usize, viewport_h: f64, line_height: f64) -> f64 {
    let content_h = LAYER_PAD * 2.0 + (line_count as f64) * line_height;
    (content_h - viewport_h).max(0.0)
}

fn clamp_scroll_top(scroll_top: f64, line_count: usize, viewport_h: f64, line_height: f64) -> f64 {
    scroll_top.clamp(0.0, max_scroll_top(line_count, viewport_h, line_height))
}

/// Slice a logical line's token list to the character range
/// `[start_col, end_col)`, splitting any token that straddles a boundary.
/// Returns `(kind, owned text)` pairs ready to drop into rsx.
fn slice_tokens(
    tokens: &[syntax::Token],
    start_col: usize,
    end_col: usize,
) -> Vec<(syntax::TokenKind, String)> {
    let mut out = Vec::with_capacity(tokens.len());
    let mut pos = 0usize;
    for t in tokens {
        let len = t.text.chars().count();
        let tok_start = pos;
        let tok_end = pos + len;
        pos = tok_end;
        if end_col <= tok_start {
            break;
        }
        if tok_end <= start_col {
            continue;
        }
        if tok_start >= start_col && tok_end <= end_col {
            // Whole token fits — skip the per-char iteration.
            out.push((t.kind, t.text.clone()));
            continue;
        }
        let cut_start = start_col.saturating_sub(tok_start);
        let cut_end = (end_col - tok_start).min(len);
        let text: String = t
            .text
            .chars()
            .skip(cut_start)
            .take(cut_end.saturating_sub(cut_start))
            .collect();
        if !text.is_empty() {
            out.push((t.kind, text));
        }
    }
    out
}

/// Split the column range `[start_col, end_col)` on logical `line` across
/// the line's visual wrap segments and call `push(visual_row, left_px,
/// width_px)` for each non-empty piece. Used by selection / isearch /
/// Compute the buffer (line, col) that `delta` visual rows away from the
/// cursor currently at `(line, col)` lands on. Used for the up/down arrow
/// keys so cursor movement follows what the user sees rather than the
/// underlying logical lines. The original column is preserved as best as
/// possible — out-of-segment falls back to the segment's end.
fn visual_move_target(
    wrap: &crate::wrap::WrapMap,
    line_chars: &[usize],
    line: usize,
    col: usize,
    delta: isize,
) -> (usize, usize) {
    let (cur_row, cur_sub) = wrap.visual_pos(line, col);
    let total = wrap.total_visual_rows();
    let target_row = if delta < 0 {
        cur_row.saturating_sub((-delta) as usize)
    } else {
        cur_row
            .saturating_add(delta as usize)
            .min(total.saturating_sub(1))
    };
    let (tgt_line, seg_start) = wrap.logical_at_visual(target_row);
    let chars = line_chars.get(tgt_line).copied().unwrap_or(0);
    let seg_idx = target_row.saturating_sub(wrap.visual_row_of_line(tgt_line));
    let is_last_seg = seg_idx + 1 >= wrap.rows_for_line(tgt_line);
    let wrap_cols = wrap.wrap_cols();
    let seg_end = if is_last_seg || wrap_cols == 0 {
        chars
    } else {
        seg_start + wrap_cols
    };
    let new_col = (seg_start + cur_sub).min(seg_end);
    (tgt_line, new_col)
}

/// ctrl-hover rect builders so a single buffer span that straddles a wrap
/// boundary still highlights correctly.
fn push_wrapped_rects(
    wrap: &crate::wrap::WrapMap,
    line: usize,
    start_col: usize,
    end_col: usize,
    m: Metrics,
    mut push: impl FnMut(usize, f64, f64),
) {
    if end_col <= start_col {
        return;
    }
    let wrap_cols = wrap.wrap_cols();
    let base_row = wrap.visual_row_of_line(line);
    let n_segs = wrap.rows_for_line(line);
    if wrap_cols == 0 || n_segs <= 1 {
        let left = LAYER_PAD + (start_col as f64) * m.char_width;
        let width = (end_col - start_col) as f64 * m.char_width;
        push(base_row, left, width);
        return;
    }
    let start_seg = start_col / wrap_cols;
    let end_seg = end_col.saturating_sub(1) / wrap_cols;
    let end_seg = end_seg.min(n_segs.saturating_sub(1));
    for seg in start_seg..=end_seg {
        let seg_start = seg * wrap_cols;
        let seg_end = seg_start + wrap_cols;
        let lo = start_col.max(seg_start);
        let hi = end_col.min(seg_end);
        if hi <= lo {
            continue;
        }
        let left = LAYER_PAD + ((lo - seg_start) as f64) * m.char_width;
        let width = (hi - lo) as f64 * m.char_width;
        push(base_row + seg, left, width);
    }
}

/// Convert client coordinates to a logical (line, col) under soft wrap.
/// Clicks past the end of a visual segment clamp to that segment's end
/// column so the cursor doesn't "spill" into the next visual row.
fn line_col_from_client(
    coords: ClientPoint,
    scroll_top: f64,
    m: Metrics,
    wrap: &crate::wrap::WrapMap,
    line_chars: &[usize],
) -> (usize, usize) {
    let rel_x = (coords.x - m.gutter_width - LAYER_PAD).max(0.0);
    let rel_y = (coords.y - m.header_height - LAYER_PAD + scroll_top).max(0.0);
    let visual_row = (rel_y / m.line_height).floor() as usize;
    let sub_col = (rel_x / m.char_width).round() as usize;
    let (line, seg_start) = wrap.logical_at_visual(visual_row);
    let chars = line_chars.get(line).copied().unwrap_or(0);
    let seg_idx = visual_row.saturating_sub(wrap.visual_row_of_line(line));
    let is_last_seg = seg_idx + 1 >= wrap.rows_for_line(line);
    let wrap_cols = wrap.wrap_cols();
    let seg_end = if is_last_seg || wrap_cols == 0 {
        chars
    } else {
        seg_start + wrap_cols
    };
    let col = (seg_start + sub_col).min(seg_end);
    (line, col)
}

#[component]
pub fn CodeEditor() -> Element {
    puffin::profile_function!();
    let ctx: AppCtx = use_context();
    let mut buffers = ctx.buffers;
    let current = ctx.current;
    let mut overlay = ctx.overlay;
    let mut ck_prefix = ctx.ck_prefix;
    let mut minibuf_msg = ctx.minibuf_msg;

    let cursor_blink = use_signal(|| false);
    let mut metrics = use_signal(Metrics::default);

    let mut editor_el = ctx.editor_el;
    let scroll_to_cursor_tick = ctx.scroll_to_cursor_tick;
    let restore_scroll_tick = ctx.restore_scroll_tick;
    let mut body_el: Signal<Option<MountedEvent>> = use_signal(|| None);
    // Editor scroll offset lives on `AppCtx` so `nav_back` (and other
    // out-of-component callers) can snapshot and restore it.
    let mut scroll_top = ctx.scroll_top;
    // Start conservatively large so first render over-emits rows rather than
    // showing blank space if the initial mount measurement is stale. The real
    // viewport height is written by onmounted/onscroll and event-time checks.
    let mut viewport_h = use_signal(|| 2000.0f64);
    // Viewport width feeds the soft-wrap column budget; default keeps the
    // very first render plausible before measurement lands.
    let mut viewport_w = use_signal(|| 1200.0f64);
    let mut last_cursor_line = use_signal(|| 0usize);
    let mut mouse_selecting = use_signal(|| false);
    let mut code_drag_active = use_signal(|| false);
    let mut drag_client_x = use_signal(|| 0.0f64);
    let mut drag_client_y = use_signal(|| 0.0f64);
    // Ctrl-hover affordance: when the user moves the mouse with Ctrl held
    // over an identifier we'd jump to on click, this stores `(line, start_col,
    // end_col)`. Rendered as an underlined rect in the cursor layer.
    let mut ctrl_hover: Signal<Option<(usize, usize, usize)>> = use_signal(|| None);

    // Read current buffer view data.
    let bufs_ref = buffers.read();
    let idx = *current.read();
    let buf_ref = &bufs_ref[idx];
    let text = {
        puffin::profile_scope!("buffer: rope -> String");
        buf_ref.text()
    };
    let line_count = buf_ref.line_count();
    let cursor_line = buf_ref.cursor_line();
    let cursor_col = buf_ref.cursor_col();
    let char_count = buf_ref.len();
    let selection = buf_ref.selection_line_cols();
    let buf_name = buf_ref.name.clone();
    let buf_path = buf_ref.path.clone();
    let buf_dirty = buf_ref.dirty;
    drop(bufs_ref);
    // The dirty state is now conveyed by the colored dot in the modeline
    // (`.ml-dot.dirty`), so the buffer name no longer needs the leading
    // `*` it used to carry.
    let display_name = buf_name.clone();

    let language = syntax::Language::from_path(buf_path.as_deref(), &buf_name);
    let language_label = language.label();
    let highlighted = syntax::highlight(&text, language);

    // Per-line character count, computed once from the highlighted tokens
    // so we don't walk the rope a second time. Feeds the wrap map and the
    // selection-rect end clamping.
    let line_chars: Vec<usize> = highlighted
        .iter()
        .map(|tokens| tokens.iter().map(|t| t.text.chars().count()).sum::<usize>())
        .collect();

    let m = *metrics.read();
    let editor_active = matches!(*ctx.focus.read(), Pane::Left);

    // Soft-wrap column budget. The gutter and a touch of padding eat into
    // the available pixel width; clamp to a minimum of 8 cols so the
    // editor stays usable when the window is comically narrow.
    // `char_width` is 0 until `#measure-line` mounts, so we fall back to 80.
    let code_w_logical = (*viewport_w.read() - m.gutter_width - LAYER_PAD * 2.0).max(0.0);
    let wrap_cols = if m.char_width > 0.0 {
        ((code_w_logical / m.char_width).floor() as usize).max(8)
    } else {
        80
    };
    // Both `wrap_map` and `line_chars` live in `Arc`s so the several event
    // handler closures (including the tokio-spawned drag-autoscroll loop,
    // which needs `Send`) are cheap to construct each render — a clone is
    // one refcount bump, not a `Vec` copy.
    let line_chars: Arc<Vec<usize>> = Arc::new(line_chars);
    let wrap_map: Arc<crate::wrap::WrapMap> =
        Arc::new(crate::wrap::WrapMap::new(&line_chars, wrap_cols));
    let total_visual_rows = wrap_map.total_visual_rows();

    let (cursor_visual_row, cursor_sub_col) = wrap_map.visual_pos(cursor_line, cursor_col);
    let cursor_left = LAYER_PAD + (cursor_sub_col as f64) * m.char_width;
    let cursor_top_px = LAYER_PAD + (cursor_visual_row as f64) * m.line_height;
    let cursor_class = if *cursor_blink.read() { "blink" } else { "" };

    // Viewport virtualization in visual-row space. Only emit DOM for
    // visual rows actually on screen (plus overscan).
    let st = *scroll_top.read();
    let vh = *viewport_h.read();
    let lh = m.line_height.max(1.0);
    let first_visual = ((st / lh).floor() as usize)
        .saturating_sub(RENDER_OVERSCAN)
        .min(total_visual_rows);
    let last_visual = ((((st + vh) / lh).ceil() as usize) + RENDER_OVERSCAN).min(total_visual_rows);
    let last_visual = last_visual.max(first_visual);

    // Translate visual-row range into a logical-line range we iterate over.
    // We always render whole logical lines (all their segments) so the
    // token-slicing logic stays clean. The visual offset of the first
    // rendered logical line drives the top spacer.
    let first_logical = wrap_map.logical_at_visual(first_visual).0.min(line_count);
    let last_logical = if last_visual == 0 || line_count == 0 {
        0
    } else {
        wrap_map
            .logical_at_visual(last_visual.saturating_sub(1))
            .0
            .saturating_add(1)
            .min(line_count)
    };
    let first_logical_visual_row = wrap_map.visual_row_of_line(first_logical);
    let after_last_logical_visual_row = wrap_map.visual_row_of_line(last_logical);

    // Isearch match rects. A single-line match can still straddle a wrap
    // boundary, so we may emit multiple rects per match. Each entry:
    // (visual_row, left_px, width_px, is_current).
    let isearch_rects: Vec<(usize, f64, f64, bool)> = {
        let isearch = ctx.isearch.read();
        match isearch.as_ref() {
            Some(s) if s.buffer == idx && !s.matches.is_empty() => {
                let bufs_r = buffers.read();
                let buf = &bufs_r[idx];
                let qlen = s.query.len();
                let mut out = Vec::new();
                for (mi, &start) in s.matches.iter().enumerate() {
                    let end = start + qlen;
                    let (sl, sc) = buf.offset_to_line_col(start);
                    let (el, ec) = buf.offset_to_line_col(end);
                    if sl != el {
                        // Don't render multi-line matches (rare; query is
                        // typically a single line).
                        continue;
                    }
                    if sl < first_logical || sl >= last_logical {
                        continue;
                    }
                    let is_current = Some(mi) == s.current;
                    push_wrapped_rects(&wrap_map, sl, sc, ec, m, |row, left, width| {
                        out.push((row, left, width, is_current));
                    });
                }
                out
            }
            _ => Vec::new(),
        }
    };

    // Selection rects: walk each logical line in the selection, then split
    // its column range across the line's visual segments.
    let selection_rects: Vec<(usize, f64, f64)> = selection
        .map(|((start_line, start_col), (end_line, end_col))| {
            let s = start_line.max(first_logical);
            let e = end_line.min(last_logical.saturating_sub(1));
            if line_count == 0 || s > e {
                return Vec::new();
            }
            let mut out = Vec::new();
            for line in s..=e {
                let left_col = if line == start_line { start_col } else { 0 };
                let right_col = if line == end_line {
                    end_col
                } else {
                    line_chars.get(line).copied().unwrap_or(0)
                };
                if right_col <= left_col {
                    continue;
                }
                push_wrapped_rects(
                    &wrap_map,
                    line,
                    left_col,
                    right_col,
                    m,
                    |row, left, width| {
                        out.push((row, left, width));
                    },
                );
            }
            out
        })
        .unwrap_or_default();

    // Ctrl-hover rect: a single identifier can still straddle a wrap
    // boundary, so we emit one rect per visual segment it touches.
    let ctrl_hover_rects: Vec<(usize, f64, f64)> = (*ctrl_hover.read())
        .and_then(|(line, sc, ec)| {
            if line < first_logical || line >= last_logical || ec <= sc {
                return None;
            }
            let mut out = Vec::new();
            push_wrapped_rects(&wrap_map, line, sc, ec, m, |row, left, width| {
                out.push((row, left, width));
            });
            Some(out)
        })
        .unwrap_or_default();

    // The cursor-into-view closures below run *after* keystrokes that may
    // have just moved the cursor, so they must re-read the buffer rather
    // than rely on `cursor_visual_row` from this render. We capture the
    // wrap map (cheap `Arc` clone) so the visual conversion stays
    // consistent with whatever was on screen when the user pressed a key.
    let cur_total_visual = total_visual_rows;
    let wrap_for_cursor = Arc::clone(&wrap_map);
    let wrap_for_tick = Arc::clone(&wrap_map);
    let wrap_for_restore = Arc::clone(&wrap_map);

    let mut ensure_cursor_visible = move || {
        let m = *metrics.read();
        let bufs = buffers.read();
        let idx = *current.read();
        let line = bufs[idx].cursor_line();
        let col = bufs[idx].cursor_col();
        drop(bufs);
        let (cur_visual_row, _) = wrap_for_cursor.visual_pos(line, col);
        let lh = m.line_height.max(1.0);
        let st = *scroll_top.read();
        // miniquad doesn't deliver window-resize events through dioxus, and
        // `editor-body`'s onmounted fires only once — so the cached
        // `viewport_h` would otherwise stay frozen at its mount-time value
        // and both the virtualization math and the auto-scroll trigger
        // below would be wrong after any resize. Re-measure here. Calling
        // it from a render context panics on a borrowed RefCell, but this
        // closure runs from event handlers (onkeydown, onmousedown), so the
        // doc isn't borrowed. `client_rect_sync` is idempotent when the
        // layout tree is clean, which is the steady state between events.
        let vh_cached = *viewport_h.read();
        let vh_min = lh * ((CURSOR_SCROLL_MARGIN_LINES * 2 + 1) as f64);
        let vh = body_el
            .read()
            .as_ref()
            .and_then(|el| el.downcast::<NodeHandle>())
            .and_then(|h| h.client_rect_sync())
            .map(|r| r.size.height)
            .filter(|h| *h >= vh_min)
            .unwrap_or(vh_cached);
        if (vh - vh_cached).abs() > 0.5 {
            viewport_h.set(vh);
        }
        last_cursor_line.set(cur_visual_row);

        // Use pixel bounds rather than row numbers. Scroll offsets can be
        // fractional / mid-line after mouse wheel scrolling; with row-only
        // checks a cursor on `top_row` could still be almost entirely clipped
        // above the viewport.
        let cursor_top = LAYER_PAD + (cur_visual_row as f64) * lh;
        let cursor_bottom = cursor_top + lh;
        let viewport_top = st;
        let viewport_bottom = st + vh;
        let new_scroll_y = if cursor_top < viewport_top {
            cursor_top
        } else if cursor_bottom > viewport_bottom {
            cursor_bottom - vh
        } else {
            return;
        };
        let new_scroll_y = clamp_scroll_top(new_scroll_y, cur_total_visual, vh, lh);

        if let Some(ref el) = *body_el.read() {
            let _ = el.scroll(
                PixelsVector2D::new(0.0, new_scroll_y),
                ScrollBehavior::Instant,
            );
            // dioxus-native-dom's programmatic scroll does not dispatch an
            // onscroll event, so keep the signal in sync ourselves — otherwise
            // the next call reads a stale `top_row` and misjudges the margin.
            scroll_top.set(new_scroll_y);
        }
    };

    // Layout-change handler: miniquad fires `resize_event` → main.rs bumps
    // `resize_tick`, and opening/closing the right pane changes the editor
    // panel from 100% to ~50% width without any window resize event. In both
    // cases re-measure the editor body's actual pixel size so soft-wrap uses
    // the real buffer viewport, not the whole screen.
    {
        let resize_tick = ctx.resize_tick;
        let right_pane = ctx.right_pane;
        use_effect(move || {
            let _ = *resize_tick.read();
            let _ = right_pane.read().is_some();
            let el_guard = body_el.read();
            let Some(handle) = el_guard.as_ref().and_then(|el| el.downcast::<NodeHandle>()) else {
                return;
            };
            let Some(rect) = handle.client_rect_sync() else {
                return;
            };
            drop(el_guard);
            if rect.size.width > 0.0 && (rect.size.width - *viewport_w.peek()).abs() > 0.5 {
                viewport_w.set(rect.size.width);
            }
            let lh = metrics.peek().line_height.max(1.0);
            let vh_min = lh * ((CURSOR_SCROLL_MARGIN_LINES * 2 + 1) as f64);
            if rect.size.height >= vh_min && (rect.size.height - *viewport_h.peek()).abs() > 0.5 {
                viewport_h.set(rect.size.height);
            }
        });
    }

    // `nav_back` writes the saved scroll offset into `scroll_top` and bumps
    // `restore_scroll_tick`. We're the only thing that can actually drive
    // the body element's scrollTop, so the effect mirrors the signal value
    // into the DOM. Done as a separate path from `scroll_to_cursor_tick`
    // because that one re-centers the cursor — here we want the *exact*
    // viewport the user left.
    use_effect(move || {
        let tick = *restore_scroll_tick.read();
        if tick == 0 {
            return;
        }
        let target = *scroll_top.peek();
        if let Some(ref el) = body_el.peek().as_ref() {
            let _ = el.scroll(
                PixelsVector2D::new(0.0, target),
                ScrollBehavior::Instant,
            );
            // Suppress the next ensure_cursor_visible's "moving up/down"
            // heuristic from immediately re-scrolling. Set the baseline to
            // the row the restored cursor lives on.
            let bufs = buffers.peek();
            let idx = *current.peek();
            if let Some(buf) = bufs.get(idx) {
                let (row, _) = wrap_for_restore.visual_pos(buf.cursor_line(), buf.cursor_col());
                last_cursor_line.set(row);
            }
        }
    });

    // External cursor jumps (rg/error result click, open-file-at-line) bump
    // `scroll_to_cursor_tick`. The closure-based `ensure_cursor_visible` only
    // runs from key/mouse handlers in this component, so external moves leave
    // the viewport stale. Watch the tick and scroll the cursor into view when
    // it changes.
    use_effect(move || {
        let tick = *scroll_to_cursor_tick.read();
        if tick == 0 {
            return;
        }
        let m = *metrics.peek();
        // Same staleness concern as `ensure_cursor_visible`: the buffer's
        // cursor moved before this effect ran, so re-read it and resolve
        // its visual row through the wrap map captured at render time.
        let bufs = buffers.peek();
        let idx = *current.peek();
        let Some(buf) = bufs.get(idx) else { return };
        let (row, _) = wrap_for_tick.visual_pos(buf.cursor_line(), buf.cursor_col());
        let total = cur_total_visual;
        drop(bufs);

        let lh = m.line_height.max(1.0);
        let vh = *viewport_h.peek();
        let st = *scroll_top.peek();
        let cursor_top = LAYER_PAD + (row as f64) * lh;
        let cursor_bottom = cursor_top + lh;
        let viewport_top = st;
        let viewport_bottom = st + vh;
        let new_scroll_y = if cursor_top < viewport_top {
            cursor_top
        } else if cursor_bottom > viewport_bottom {
            cursor_bottom - vh
        } else {
            last_cursor_line.set(row);
            return;
        };
        let new_scroll_y = clamp_scroll_top(new_scroll_y, total, vh, lh);

        if let Some(ref el) = body_el.peek().as_ref() {
            let _ = el.scroll(
                PixelsVector2D::new(0.0, new_scroll_y),
                ScrollBehavior::Instant,
            );
            scroll_top.set(new_scroll_y);
            last_cursor_line.set(row);
        }
    });

    // Drag autoscroll uses `tokio::time::sleep`, which requires a tokio
    // runtime — disabled on wasm. We capture a snapshot of the wrap map
    // and per-line char counts when the drag effect (re-)runs; edits
    // during a drag are exceedingly rare, so the small staleness window
    // is acceptable in exchange for not rebuilding the wrap map per tick.
    #[cfg(not(target_arch = "wasm32"))]
    use_effect({
        let wrap_for_drag = Arc::clone(&wrap_map);
        let chars_for_drag = Arc::clone(&line_chars);
        let drag_total = total_visual_rows;
        move || {
            if !*mouse_selecting.read() || !*code_drag_active.read() {
                return;
            }
            let wrap_for_drag = Arc::clone(&wrap_for_drag);
            let chars_for_drag = Arc::clone(&chars_for_drag);
            spawn(async move {
                loop {
                    if !*mouse_selecting.read() || !*code_drag_active.read() {
                        break;
                    }

                    let m = *metrics.read();
                    let y = *drag_client_y.read();
                    let st = *scroll_top.read();
                    let vh = *viewport_h.read();
                    let mut delta = 0.0;

                    if y < m.header_height + DRAG_AUTOSCROLL_MARGIN {
                        delta = -m.line_height;
                    } else if y > m.header_height + vh - DRAG_AUTOSCROLL_MARGIN {
                        delta = m.line_height;
                    }

                    if delta != 0.0 {
                        let new_scroll_y =
                            clamp_scroll_top(st + delta, drag_total, vh, m.line_height);
                        if let Some(ref el) = *body_el.read() {
                            let _ = el.scroll(
                                PixelsVector2D::new(0.0, new_scroll_y),
                                ScrollBehavior::Instant,
                            );
                            scroll_top.set(new_scroll_y);
                            let (line, col) = line_col_from_client(
                                ClientPoint::new(*drag_client_x.read(), y),
                                new_scroll_y,
                                m,
                                &wrap_for_drag,
                                &chars_for_drag,
                            );
                            let idx = *current.read();
                            buffers.write()[idx].move_to_with_selection(line, col);
                        }
                    }

                    tokio::time::sleep(std::time::Duration::from_millis(16)).await;
                }
            });
        }
    });

    // Per-handler Arc clones bound at component scope so the move closures
    // in the rsx tree below can each take ownership of their own
    // reference-counted handle without forcing inline block syntax (which
    // the rsx! macro doesn't accept for event attributes).
    let wrap_dbl = Arc::clone(&wrap_map);
    let chars_dbl = Arc::clone(&line_chars);
    let wrap_md = Arc::clone(&wrap_map);
    let chars_md = Arc::clone(&line_chars);
    let wrap_mm = Arc::clone(&wrap_map);
    let chars_mm = Arc::clone(&line_chars);
    // Keyboard arrows use the wrap map too — vertical movement should
    // follow visual rows, not logical lines, when wrap is active.
    let wrap_kd = Arc::clone(&wrap_map);
    let chars_kd = Arc::clone(&line_chars);

    rsx! {
        div {
            id: "editor-panel",
            class: if editor_active { "pane-active" } else { "pane-inactive" },
            tabindex: "0",
            onmounted: move |evt: MountedEvent| {
                editor_el.set(Some(evt.clone()));
                let _ = evt.set_focus(true);
            },
            onkeyup: move |evt: Event<KeyboardData>| {
                // Releasing Ctrl removes the hover affordance. We don't try
                // to re-derive it from mouse position because we don't have
                // a "current mouse pos" signal — the next mousemove will
                // reinstate the highlight if Ctrl is pressed again over a
                // word.
                let modifiers = evt.modifiers();
                let ctrl_still_held = modifiers.contains(Modifiers::CONTROL)
                    || modifiers.contains(Modifiers::META);
                if !ctrl_still_held && ctrl_hover.peek().is_some() {
                    ctrl_hover.set(None);
                }
            },
            onkeydown: move |evt: Event<KeyboardData>| {
                puffin::profile_scope!("editor: onkeydown");
                let key = evt.key();
                // Blitz's default action for Tab is focus traversal. If we let
                // it run after our handler, Tab still reindents/accepts/etc.
                // but keyboard focus leaves the editor, making the cursor look
                // "stuck" until a mouse click focuses the editor again.
                if matches!(key, Key::Tab) {
                    evt.prevent_default();
                }
                let modifiers = evt.modifiers();
                let ctrl = modifiers.contains(Modifiers::CONTROL)
                    || modifiers.contains(Modifiers::META);
                let shift = modifiers.contains(Modifiers::SHIFT);

                // --- 1. Overlay is active: route all keys to overlay logic ---
                let overlay_active = overlay.read().is_some();
                if overlay_active {
                    handle_overlay_key(&key, ctrl, ctx);
                    return;
                }

                // --- 1b. Incremental search active: capture all keys ---
                if ctx.isearch.read().is_some() {
                    if crate::isearch::handle_key(&key, ctrl, ctx) {
                        ensure_cursor_visible();
                        return;
                    }
                }

                // --- 1c. Completion popup is active: nav / accept / dismiss.
                // Most keys close the popup so editing continues seamlessly;
                // C-Space falls through so the dedicated handler below
                // re-fires a new request.
                #[cfg(not(target_arch = "wasm32"))]
                if ctx.completion.read().is_some() {
                    if completion_handle_key(&key, ctrl, ctx) {
                        return;
                    }
                    let is_retrigger =
                        ctrl && matches!(&key, Key::Character(c) if c == " ");
                    if !is_retrigger {
                        ctx.completion.clone().set(None);
                    }
                }

                // --- 2. C-k prefix is active: interpret this as a command ---
                if *ck_prefix.read() {
                    ck_prefix.set(false);
                    minibuf_msg.set(String::new());
                    if handle_ck_command(&key, ctrl, ctx) {
                        return;
                    }
                    // If it wasn't a recognized prefix command, fall through
                    // to normal editor handling.
                }

                // --- 3. Raw C-k: enter prefix state ---
                if ctrl
                    && matches!(&key, Key::Character(c) if c.eq_ignore_ascii_case("k"))
                {
                    ck_prefix.set(true);
                    minibuf_msg.set("C-k -".to_string());
                    return;
                }

                // --- 3b. C-S-f opens ripgrep; C-f / C-r start incremental
                // search (no isearch currently active — the active branch is
                // handled in 1b). ---
                #[cfg(not(target_arch = "wasm32"))]
                if ctrl
                    && shift
                    && matches!(&key, Key::Character(c) if c.eq_ignore_ascii_case("f"))
                {
                    overlay.set(Some(Overlay::RgPrompt {
                        query: String::new(),
                        cursor: 0,
                    }));
                    return;
                }
                if ctrl
                    && !shift
                    && matches!(&key, Key::Character(c) if c.eq_ignore_ascii_case("f"))
                {
                    crate::isearch::start(ctx, crate::app::SearchDir::Forward);
                    return;
                }
                if ctrl
                    && matches!(&key, Key::Character(c) if c.eq_ignore_ascii_case("r"))
                {
                    crate::isearch::start(ctx, crate::app::SearchDir::Backward);
                    return;
                }
                // C-Space: trigger LSP completion at the cursor.
                #[cfg(not(target_arch = "wasm32"))]
                if ctrl && matches!(&key, Key::Character(c) if c == " ") {
                    trigger_completion(ctx);
                    return;
                }

                // --- 4. Normal editor keys ---
                let page_lines = (*viewport_h.read() / metrics.read().line_height)
                    .floor()
                    .max(1.0) as usize;

                if ctrl {
                    match key {
                        Key::Character(ref c) if c.eq_ignore_ascii_case("a") => {
                            let idx = *current.read();
                            buffers.write()[idx].select_all();
                            ensure_cursor_visible();
                            return;
                        }
                        Key::Character(ref c) if c.eq_ignore_ascii_case("z") => {
                            let idx = *current.read();
                            let changed = if shift {
                                buffers.write()[idx].redo()
                            } else {
                                buffers.write()[idx].undo()
                            };
                            if changed {
                                ensure_cursor_visible();
                            }
                            return;
                        }
                        Key::Character(ref c) if c.eq_ignore_ascii_case("y") => {
                            let idx = *current.read();
                            if buffers.write()[idx].redo() {
                                ensure_cursor_visible();
                            }
                            return;
                        }
                        Key::Character(ref c) if c == "g" => {
                            // C-g: cancel any transient state
                            minibuf_msg.set(String::new());
                            ck_prefix.set(false);
                            return;
                        }
                        // C-c: copy selection to the OS clipboard. No-op when
                        // nothing is selected.
                        Key::Character(ref c) if c.eq_ignore_ascii_case("c") => {
                            let idx = *current.read();
                            if let Some(text) = buffers.read()[idx].selection_text() {
                                crate::clipboard::copy(&text);
                            }
                            return;
                        }
                        // C-x: cut selection — copy then delete. No-op when
                        // nothing is selected (so it doesn't clobber the
                        // clipboard with an empty string on accident).
                        Key::Character(ref c) if c.eq_ignore_ascii_case("x") => {
                            let idx = *current.read();
                            let text = buffers.read()[idx].selection_text();
                            if let Some(text) = text {
                                crate::clipboard::copy(&text);
                                buffers.write()[idx].delete_selection();
                                ensure_cursor_visible();
                            }
                            return;
                        }
                        // C-v: paste clipboard contents at the cursor,
                        // replacing the current selection if any (Buffer::insert
                        // handles that).
                        Key::Character(ref c) if c.eq_ignore_ascii_case("v") => {
                            if let Some(text) = crate::clipboard::paste() {
                                if !text.is_empty() {
                                    let idx = *current.read();
                                    buffers.write()[idx].insert(&text);
                                    ensure_cursor_visible();
                                }
                            } else {
                                minibuf_msg.set("paste: clipboard unavailable".to_string());
                            }
                            return;
                        }
                        _ => {}
                    }
                }

                let idx = *current.read();
                let mut bufs = buffers.write();
                let b = &mut bufs[idx];
                match key {
                    Key::Backspace => b.backspace(),
                    Key::Delete => b.delete(),
                    Key::ArrowLeft => {
                        if ctrl {
                            if shift { b.move_word_left_with_selection() } else { b.move_word_left() }
                        } else if shift {
                            b.move_left_with_selection()
                        } else {
                            b.move_left()
                        }
                    }
                    Key::ArrowRight => {
                        if ctrl {
                            if shift { b.move_word_right_with_selection() } else { b.move_word_right() }
                        } else if shift {
                            b.move_right_with_selection()
                        } else {
                            b.move_right()
                        }
                    }
                    Key::ArrowUp => {
                        let (cl, cc) = (b.cursor_line(), b.cursor_col());
                        let (tl, tc) = visual_move_target(&wrap_kd, &chars_kd, cl, cc, -1);
                        if shift { b.move_to_with_selection(tl, tc) } else { b.move_to(tl, tc) }
                    }
                    Key::ArrowDown => {
                        let (cl, cc) = (b.cursor_line(), b.cursor_col());
                        let (tl, tc) = visual_move_target(&wrap_kd, &chars_kd, cl, cc, 1);
                        if shift { b.move_to_with_selection(tl, tc) } else { b.move_to(tl, tc) }
                    }
                    Key::PageUp => {
                        let (cl, cc) = (b.cursor_line(), b.cursor_col());
                        let (tl, tc) = visual_move_target(
                            &wrap_kd,
                            &chars_kd,
                            cl,
                            cc,
                            -(page_lines as isize),
                        );
                        if shift { b.move_to_with_selection(tl, tc) } else { b.move_to(tl, tc) }
                    }
                    Key::PageDown => {
                        let (cl, cc) = (b.cursor_line(), b.cursor_col());
                        let (tl, tc) = visual_move_target(
                            &wrap_kd,
                            &chars_kd,
                            cl,
                            cc,
                            page_lines as isize,
                        );
                        if shift { b.move_to_with_selection(tl, tc) } else { b.move_to(tl, tc) }
                    }
                    Key::Home => {
                        if ctrl {
                            if shift { b.move_to_start_with_selection() } else { b.move_to_start() }
                        } else if shift {
                            b.move_to_line_start_with_selection()
                        } else {
                            b.move_to_line_start()
                        }
                    }
                    Key::End => {
                        if ctrl {
                            if shift { b.move_to_end_with_selection() } else { b.move_to_end() }
                        } else if shift {
                            b.move_to_line_end_with_selection()
                        } else {
                            b.move_to_line_end()
                        }
                    }
                    Key::Enter => b.insert("\n"),
                    Key::Tab => {
                        if language == syntax::Language::Rust {
                            b.retab_current_rust_line();
                        }
                    }
                    Key::Character(ref c) => {
                        if !ctrl {
                            b.insert(c);
                        } else {
                            return;
                        }
                    }
                    _ => { return; }
                }
                drop(bufs);
                ensure_cursor_visible();
            },

            div {
                id: "measure-line",
                onmounted: move |evt: MountedEvent| {
                    if let Some(rect) = evt
                        .downcast::<NodeHandle>()
                        .and_then(|h| h.client_rect_sync())
                    {
                        let mut me = metrics.write();
                        me.char_width = rect.size.width / MEASURE_CHARS as f64;
                        me.line_height = rect.size.height;
                    }
                },
                "{\"M\".repeat(MEASURE_CHARS)}"
            }

            div {
                id: "editor-body",
                onmounted: move |evt: MountedEvent| {
                    body_el.set(Some(evt.clone()));
                    let lh = metrics.read().line_height.max(1.0);
                    if let Some(rect) = evt
                        .downcast::<NodeHandle>()
                        .and_then(|h| h.client_rect_sync())
                        .filter(|rect| {
                            rect.size.height
                                >= lh * ((CURSOR_SCROLL_MARGIN_LINES * 2 + 1) as f64)
                        })
                    {
                        viewport_h.set(rect.size.height);
                        if rect.size.width > 0.0 {
                            viewport_w.set(rect.size.width);
                        }
                    }
                },
                onscroll: move |evt: Event<ScrollData>| {
                    let lh = metrics.read().line_height;
                    let data = evt.data();
                    let vh = data.client_height() as f64;
                    let vw = data.client_width() as f64;
                    if vh >= lh.max(1.0) * ((CURSOR_SCROLL_MARGIN_LINES * 2 + 1) as f64) {
                        viewport_h.set(vh);
                    }
                    if vw > 0.0 {
                        viewport_w.set(vw);
                    }
                    scroll_top.set(clamp_scroll_top(data.scroll_top(), total_visual_rows, vh, lh));
                },

                div { id: "editor-inner",
                    div { id: "gutter",
                        onmounted: move |evt: MountedEvent| {
                            if let Some(rect) = evt
                                .downcast::<NodeHandle>()
                                .and_then(|h| h.client_rect_sync())
                            {
                                metrics.write().gutter_width = rect.size.width;
                            }
                        },
                        onmousedown: move |evt: Event<MouseData>| {
                            evt.prevent_default();
                            code_drag_active.set(false);
                            mouse_selecting.set(false);
                        },
                        // Top spacer: covers visual rows above the first
                        // rendered logical line so the gutter stays
                        // O(viewport).
                        div { style: "height: {first_logical_visual_row as f64 * lh}px;" }
                        // For each rendered logical line we emit one row
                        // per visual segment — the number sits on segment 0,
                        // continuation rows show blank space so the gutter
                        // matches the code area's height exactly.
                        for line_idx in first_logical..last_logical {
                            {
                                let rows = wrap_map.rows_for_line(line_idx);
                                rsx! {
                                    div {
                                        class: "line-num",
                                        key: "gn-{line_idx}-0",
                                        "{line_idx + 1}"
                                    }
                                    for seg in 1..rows {
                                        div {
                                            class: "line-num",
                                            key: "gn-{line_idx}-{seg}",
                                            " "
                                        }
                                    }
                                }
                            }
                        }
                        // Bottom spacer: occupies the visual rows below the
                        // last rendered logical line so the scroll container
                        // matches the full content height.
                        div { style: "height: {(total_visual_rows.saturating_sub(after_last_logical_visual_row)) as f64 * lh}px;" }
                    }

                    div {
                        id: "code-area",
                        ondoubleclick: move |evt: Event<MouseData>| {
                            evt.prevent_default();
                            code_drag_active.set(false);
                            mouse_selecting.set(false);
                            if ctx.isearch.peek().is_some() {
                                ctx.isearch.clone().set(None);
                                ctx.minibuf_msg.clone().set(String::new());
                            }
                            if let Some(el) = editor_el.read().as_ref() {
                                let _ = el.set_focus(true);
                            }
                            let mut focus = ctx.focus;
                            focus.set(Pane::Left);
                            let m = *metrics.read();
                            let current_scroll_top = clamp_scroll_top(
                                *scroll_top.read(),
                                total_visual_rows,
                                *viewport_h.read(),
                                m.line_height,
                            );
                            let (line, col) = line_col_from_client(
                                evt.client_coordinates(),
                                current_scroll_top,
                                m,
                                &wrap_dbl,
                                &chars_dbl,
                            );
                            let idx = *current.read();
                            buffers.write()[idx].select_word_at(line, col);
                        },
                        onmousedown: move |evt: Event<MouseData>| {
                            evt.prevent_default();
                            // Back mouse button: pop navigation history.
                            // Browsers also fire mousedown for buttons 4/5;
                            // handle it before anything else so we don't
                            // move the cursor or steal focus on a back-click.
                            #[cfg(not(target_arch = "wasm32"))]
                            if evt.trigger_button() == Some(MouseButton::Fourth) {
                                crate::app::nav_back(&ctx);
                                return;
                            }
                            // Mousedown during isearch accepts the search — clears
                            // the highlight overlay and lets the click move the
                            // cursor freely.
                            if ctx.isearch.peek().is_some() {
                                ctx.isearch.clone().set(None);
                                ctx.minibuf_msg.clone().set(String::new());
                            }
                            if let Some(el) = editor_el.read().as_ref() {
                                let _ = el.set_focus(true);
                            }
                            let mut focus = ctx.focus;
                            focus.set(Pane::Left);
                            let modifiers = evt.modifiers();
                            let ctrl = modifiers.contains(Modifiers::CONTROL)
                                || modifiers.contains(Modifiers::META);
                            let shift = modifiers.contains(Modifiers::SHIFT);
                            let m = *metrics.read();
                            let current_scroll_top = clamp_scroll_top(
                                *scroll_top.read(),
                                total_visual_rows,
                                *viewport_h.read(),
                                m.line_height,
                            );
                            let (line, col) = line_col_from_client(
                                evt.client_coordinates(),
                                current_scroll_top,
                                m,
                                &wrap_md,
                                &chars_md,
                            );

                            // Ctrl-click: ask the LSP server for the definition
                            // at the clicked position instead of moving the
                            // cursor / starting a selection.
                            #[cfg(not(target_arch = "wasm32"))]
                            if ctrl {
                                // The hover highlight has done its job.
                                if ctrl_hover.peek().is_some() {
                                    ctrl_hover.set(None);
                                }
                                let mut minibuf_msg = ctx.minibuf_msg;
                                let idx = *current.read();
                                let bufs = buffers.read();
                                let Some(b) = bufs.get(idx) else { return };
                                let path = b.path.clone();
                                let root = b.project_root.clone();
                                let abs = b.line_col_to_offset(line, col);
                                let line_start = b.rope.offset_of_line(line.min(b.line_count().saturating_sub(1)));
                                let byte_col = abs.saturating_sub(line_start) as u32;
                                drop(bufs);
                                let Some(p) = path else {
                                    minibuf_msg.set("lsp: buffer has no file path".into());
                                    return;
                                };
                                let Some(r) = root else {
                                    minibuf_msg.set("lsp: buffer has no project root".into());
                                    return;
                                };
                                let mgr = ctx.lsp.peek().clone();
                                let status = mgr.status_for(&r);
                                // Only Ready and Indexing servers can answer.
                                // Disabled/Starting/Error → tell the user; the
                                // alternative is to send the request and have
                                // it hang for minutes during a cold index.
                                match status {
                                    crate::lsp::LspStatus::Ready
                                    | crate::lsp::LspStatus::Indexing { .. } => {
                                        mgr.goto_definition(&r, &p, line as u32, byte_col);
                                        minibuf_msg.set(format!(
                                            "lsp: jumping to definition at {}:{} …",
                                            line + 1,
                                            col + 1
                                        ));
                                    }
                                    crate::lsp::LspStatus::Starting => {
                                        minibuf_msg.set(
                                            "lsp: rust-analyzer still starting — try again in a moment".into(),
                                        );
                                    }
                                    crate::lsp::LspStatus::Error(e) => {
                                        minibuf_msg.set(format!("lsp: no rust-analyzer ({e})"));
                                    }
                                    crate::lsp::LspStatus::Disabled => {
                                        minibuf_msg.set("lsp: no rust-analyzer for this buffer".into());
                                    }
                                }
                                return;
                            }
                            #[cfg(target_arch = "wasm32")]
                            let _ = ctrl;

                            code_drag_active.set(true);
                            mouse_selecting.set(true);
                            drag_client_x.set(evt.client_coordinates().x);
                            drag_client_y.set(evt.client_coordinates().y);
                            if ctrl_hover.peek().is_some() {
                                ctrl_hover.set(None);
                            }
                            // Plain click clears a stale LSP hint so we don't
                            // leave "jumping to definition…" on screen after
                            // the user moves on.
                            if !ctx.minibuf_msg.peek().is_empty() {
                                let mut minibuf_msg = ctx.minibuf_msg;
                                minibuf_msg.set(String::new());
                            }
                            let idx = *current.read();
                            let mut bufs = buffers.write();
                            let b = &mut bufs[idx];
                            if shift {
                                b.move_to_with_selection(line, col);
                            } else {
                                b.move_to(line, col);
                            }
                            drop(bufs);
                        },

                        onmousemove: move |evt: Event<MouseData>| {
                            let modifiers = evt.modifiers();
                            let ctrl_held = modifiers.contains(Modifiers::CONTROL)
                                || modifiers.contains(Modifiers::META);
                            let m = *metrics.read();
                            let current_scroll_top = clamp_scroll_top(
                                *scroll_top.read(),
                                total_visual_rows,
                                *viewport_h.read(),
                                m.line_height,
                            );
                            let (line, col) = line_col_from_client(
                                evt.client_coordinates(),
                                current_scroll_top,
                                m,
                                &wrap_mm,
                                &chars_mm,
                            );

                            // Ctrl-hover: highlight the word the user would
                            // jump to on click. We don't actually call LSP
                            // here — that would mean a network round trip per
                            // mouse-move pixel. Identifier-style word range
                            // is a fine approximation and matches the click
                            // hit-test, since `goto_definition` resolves
                            // wherever inside the identifier the click lands.
                            if ctrl_held && !*mouse_selecting.read() {
                                let idx = *current.read();
                                let next = {
                                    let bufs = buffers.read();
                                    bufs.get(idx).and_then(|b| {
                                        let (start_byte, end_byte) = b.word_range_at(line, col)?;
                                        let (l, sc) = b.offset_to_line_col(start_byte);
                                        let (_, ec) = b.offset_to_line_col(end_byte);
                                        Some((l, sc, ec))
                                    })
                                };
                                if *ctrl_hover.peek() != next {
                                    ctrl_hover.set(next);
                                }
                                return;
                            }
                            if ctrl_hover.peek().is_some() {
                                ctrl_hover.set(None);
                            }

                            if !*mouse_selecting.read() || !*code_drag_active.read() {
                                return;
                            }
                            drag_client_x.set(evt.client_coordinates().x);
                            drag_client_y.set(evt.client_coordinates().y);
                            if !evt.held_buttons().contains(MouseButton::Primary) {
                                code_drag_active.set(false);
                                mouse_selecting.set(false);
                                return;
                            }
                            let idx = *current.read();
                            buffers.write()[idx].move_to_with_selection(line, col);
                        },
                        onmouseup: move |_evt: Event<MouseData>| {
                            code_drag_active.set(false);
                            mouse_selecting.set(false);
                        },
                        onmouseleave: move |_evt: Event<MouseData>| {
                            code_drag_active.set(false);
                            mouse_selecting.set(false);
                            if ctrl_hover.peek().is_some() {
                                ctrl_hover.set(None);
                            }
                        },

                        div { id: "selection-layer",
                            for (row, left, width) in selection_rects.iter().copied() {
                                div {
                                    class: "selection-rect",
                                    key: "sel-{row}-{left}",
                                    style: "left: {left}px; top: {LAYER_PAD + (row as f64) * m.line_height}px; width: {width}px; height: {m.line_height}px;",
                                }
                            }
                            for (row, left, width, is_current) in isearch_rects.iter().copied() {
                                div {
                                    class: if is_current { "search-rect search-rect-current" } else { "search-rect" },
                                    key: "isr-{row}-{left}",
                                    style: "left: {left}px; top: {LAYER_PAD + (row as f64) * m.line_height}px; width: {width}px; height: {m.line_height}px;",
                                }
                            }
                            for (row, left, width) in ctrl_hover_rects.iter().copied() {
                                div {
                                    class: "ctrl-hover-rect",
                                    key: "chr-{row}-{left}",
                                    style: "left: {left}px; top: {LAYER_PAD + (row as f64) * m.line_height}px; width: {width}px; height: {m.line_height}px;",
                                }
                            }
                        }

                        div {
                            id: "cursor",
                            class: "{cursor_class}",
                            style: "left: {cursor_left}px; top: {cursor_top_px}px;",
                        }

                        CompletionPopup {
                            char_width: m.char_width,
                            line_height: m.line_height,
                        }

                        div { id: "highlight-layer",
                            // Top spacer: covers the visual rows above the
                            // first rendered logical line. The highlight-layer's
                            // own CSS padding (2px = LAYER_PAD) covers the
                            // document start, so the spacer only needs to cover
                            // the skipped visual rows.
                            div { style: "height: {first_logical_visual_row as f64 * lh}px;" }
                            // For each rendered logical line emit one
                            // `.code-line` div per wrap segment, with the
                            // slice of tokens that falls inside that segment's
                            // column range.
                            for line_idx in first_logical..last_logical {
                                {
                                    let line_idx = line_idx;
                                    let tokens = &highlighted[line_idx];
                                    let chars = line_chars.get(line_idx).copied().unwrap_or(0);
                                    let segments: Vec<(usize, usize)> =
                                        wrap_map.segments_for_line(line_idx, chars).collect();
                                    rsx! {
                                        for (seg_idx, (s, e)) in segments.into_iter().enumerate() {
                                            div {
                                                class: "code-line",
                                                key: "cl-{line_idx}-{seg_idx}",
                                                {
                                                    let slice = slice_tokens(tokens, s, e);
                                                    rsx! {
                                                        if slice.is_empty() {
                                                            span { class: "syn-plain", " " }
                                                        }
                                                        for (kind, text) in slice.into_iter() {
                                                            span { class: kind.css_class(), "{text}" }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            // Bottom spacer: occupies visual rows below the
                            // last rendered logical line.
                            div { style: "height: {(total_visual_rows.saturating_sub(after_last_logical_visual_row)) as f64 * lh}px;" }
                        }
                    }
                }
            }

            div {
                id: "status-bar",
                class: if editor_active { "active" } else { "inactive" },
                onmousedown: move |evt: Event<MouseData>| {
                    evt.prevent_default();
                    code_drag_active.set(false);
                    mouse_selecting.set(false);
                },
                {
                    let ln = cursor_line + 1;
                    let co = cursor_col + 1;
                    let pos = if cursor_line == 0 {
                        "Top".to_string()
                    } else if cursor_line + 1 >= line_count {
                        "Bot".to_string()
                    } else {
                        let pct = (cursor_line * 100 / line_count.max(1)).min(99);
                        format!("{pct}%")
                    };
                    let _ = char_count;
                    rsx! {
                        span { class: if buf_dirty { "ml-dot ml-dot-dirty" } else { "ml-dot" } }
                        span { class: "ml-name", "{display_name}" }
                        span { class: "ml-pos", "{pos}" }
                        span { class: "ml-lc", "({ln},{co})" }
                        if !language_label.is_empty() {
                            span { class: "ml-mode", "({language_label})" }
                        }
                        ProjectPill {}
                        LspStatusPill {}
                    }
                }
            }
        }
    }
}

/// Modeline pill showing the active project's full path (or `proj: —` when
/// none is loaded). Own component so swapping projects doesn't invalidate
/// the whole CodeEditor render.
///
/// The path is truncated from the right (`/very/long/pat…`) when it
/// wouldn't fit alongside the other status-bar items. Truncation is
/// approximate: status-bar font is 16px Liberation Mono (~9.6 logical
/// pixels per char), and we reserve ~40 chars for the rest of the modeline
/// (buffer name, position, mode, LSP pill). Re-runs on window resize.
#[component]
fn ProjectPill() -> Element {
    let ctx: AppCtx = use_context();
    let label = match ctx.project_root.read().as_ref() {
        Some(root) => format!("proj: {}", root.display()),
        None => "proj: —".to_string(),
    };

    let max_chars = project_pill_char_budget();
    let display = truncate_tail(&label, max_chars);
    rsx! { span { class: "ml-proj", "{display}" } }
}

/// Visible character budget for the project pill.
///
/// Do not call `miniquad::window::*` here: `ProjectPill` is rendered during
/// `doc.initial_build()`, before `miniquad::start` initializes the native
/// display, and those APIs panic noisily even when wrapped in `catch_unwind`.
/// The modeline also has CSS overflow protection, so a stable conservative
/// character budget is good enough.
fn project_pill_char_budget() -> usize {
    80
}

/// Truncate `s` to `max` visible chars by chopping the tail and appending
/// `…` when shortened. Operates on Unicode scalars so we don't slice mid-
/// codepoint.
fn truncate_tail(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let keep: String = s.chars().take(max - 1).collect();
    format!("{keep}…")
}

/// Modeline LSP status pill. Extracted into its own component so it can
/// subscribe to `lsp_tick` (which advances many times per second during
/// rust-analyzer indexing) without dragging the whole CodeEditor re-render
/// — syntax highlighting on a large file is way too expensive to redo at
/// the progress-update cadence.
#[component]
fn LspStatusPill() -> Element {
    let ctx: AppCtx = use_context();
    let _ = *ctx.lsp_tick.read();
    // The pill reflects the LSP session for the active project. With no
    // project loaded there's nothing to key a status on — show "lsp: off"
    // directly (matches LspStatus::Disabled's label).
    let label = match ctx.project_root.read().as_ref() {
        Some(root) => {
            let mgr = ctx.lsp.peek().clone();
            mgr.status_for(root).label()
        }
        None => "lsp: off".to_string(),
    };
    rsx! { span { class: "ml-lsp", "{label}" } }
}

// ── C-k prefix command dispatch ───────────────────────────────────────────

pub(crate) fn handle_ck_command(key: &Key, ctrl: bool, ctx: AppCtx) -> bool {
    let AppCtx {
        mut buffers,
        mut current,
        mut overlay,
        mut right_pane,
        mut right_pane_selected,
        mut focus,
        editor_el,
        right_pane_el,
        mut file_index,
        mut minibuf_msg,
        ..
    } = ctx;
    let _ = (&file_index, &right_pane, &right_pane_el, &minibuf_msg);

    match key {
        // C-k C-f: find file  (native only: requires filesystem)
        #[cfg(not(target_arch = "wasm32"))]
        Key::Character(c) if ctrl && c.eq_ignore_ascii_case("f") => {
            // With no project loaded the index is empty, so we pre-fill the
            // query with `./` to drop straight into path-picker mode.
            let initial_query = match crate::app::active_project_root(&ctx) {
                Some(root) => {
                    file_index.set(crate::files::scan_project(&root));
                    String::new()
                }
                None => {
                    file_index.set(Vec::new());
                    "./".to_string()
                }
            };
            overlay.set(Some(Overlay::FilePicker {
                query: initial_query,
                selected: 0,
            }));
            true
        }
        // C-k b: switch buffer
        Key::Character(c) if !ctrl && c == "b" => {
            overlay.set(Some(Overlay::BufferSwitcher {
                query: String::new(),
                selected: 0,
            }));
            true
        }
        // C-k u: visualize undo history for the current buffer.
        Key::Character(c) if !ctrl && c == "u" => {
            let idx = *current.read();
            let selected = buffers
                .write()
                .get_mut(idx)
                .map(|b| b.undo_tree_current_node())
                .unwrap_or(0);
            overlay.set(Some(Overlay::UndoTree {
                selected,
                origin: selected,
            }));
            true
        }
        // C-k r: revert current buffer from disk, discarding local changes.
        // Useful when the file has been edited by an external tool.
        #[cfg(not(target_arch = "wasm32"))]
        Key::Character(c) if !ctrl && c == "r" => {
            let idx = *current.read();
            let (path, line, col) = {
                let bufs = buffers.read();
                let Some(buf) = bufs.get(idx) else {
                    return true;
                };
                let Some(path) = buf.path.clone() else {
                    minibuf_msg.set("revert: scratch buffer has no file".to_string());
                    return true;
                };
                (path, buf.cursor_line(), buf.cursor_col())
            };
            match crate::buffer::Buffer::from_path(path.clone()) {
                Ok(mut fresh) => {
                    // move_to clamps to whatever lines / cols exist after the
                    // external edit, so the cursor lands somewhere sensible
                    // even if the file shrank.
                    fresh.move_to(line, col);
                    buffers.write()[idx] = fresh;
                    let mut tick = ctx.scroll_to_cursor_tick;
                    let next = tick.peek().wrapping_add(1);
                    tick.set(next);
                    minibuf_msg.set(format!("Reverted {}", path.display()));
                }
                Err(e) => minibuf_msg.set(format!("revert failed: {e}")),
            }
            true
        }
        #[cfg(target_arch = "wasm32")]
        Key::Character(c) if !ctrl && c == "r" => {
            minibuf_msg.set("revert not supported on wasm".to_string());
            true
        }
        // C-k f: rustfmt current file (native only)
        #[cfg(not(target_arch = "wasm32"))]
        Key::Character(c) if !ctrl && c == "f" => {
            let idx = *current.read();
            let (path, line, col) = {
                let mut bufs = buffers.write();
                let Some(buf) = bufs.get_mut(idx) else {
                    minibuf_msg.set(String::new());
                    return true;
                };
                let line = buf.cursor_line();
                let col = buf.cursor_col();
                let save_result = buf.save();
                let path = match save_result {
                    Ok(path) => path.to_path_buf(),
                    Err(e) => {
                        minibuf_msg.set(format!("rustfmt failed: {e}"));
                        return true;
                    }
                };
                (path, line, col)
            };

            match commands::run_rustfmt_file(&path) {
                Ok(msg) if msg.starts_with("Formatted ") => {
                    match crate::buffer::Buffer::from_path(path) {
                        Ok(mut formatted) => {
                            formatted.move_to(line, col);
                            buffers.write()[idx] = formatted;
                            minibuf_msg.set(msg);
                        }
                        Err(e) => minibuf_msg.set(format!("reload after rustfmt failed: {e}")),
                    }
                }
                Ok(msg) => minibuf_msg.set(msg),
                Err(e) => minibuf_msg.set(format!("rustfmt failed: {e}")),
            }
            true
        }
        #[cfg(target_arch = "wasm32")]
        Key::Character(c) if !ctrl && c == "f" => {
            minibuf_msg.set("rustfmt not supported on wasm".to_string());
            true
        }
        // C-k c: compile  (native only)
        #[cfg(not(target_arch = "wasm32"))]
        Key::Character(c) if !ctrl && c == "c" => {
            let Some(root) = crate::app::active_project_root(&ctx) else {
                minibuf_msg.set("compile: no project loaded".to_string());
                return true;
            };
            let out = commands::run_compile(root.clone());
            right_pane.set(Some(RightPaneState {
                title: "*compilation*".to_string(),
                output: out,
                cwd: root,
            }));
            right_pane_selected.set(0);
            true
        }
        // C-k s: save current buffer to its backing file (native only)
        #[cfg(not(target_arch = "wasm32"))]
        Key::Character(c) if c.eq_ignore_ascii_case("s") => {
            let idx = *current.read();
            let mut bufs = buffers.write();
            let msg = match bufs.get_mut(idx) {
                Some(buf) => match buf.save() {
                    Ok(path) => format!("Wrote {}", path.display()),
                    Err(e) => format!("Save failed: {e}"),
                },
                None => String::new(),
            };
            drop(bufs);
            minibuf_msg.set(msg);
            true
        }
        #[cfg(target_arch = "wasm32")]
        Key::Character(c) if c.eq_ignore_ascii_case("s") => {
            minibuf_msg.set("save not supported on wasm".to_string());
            true
        }
        // C-k k: kill (close) current buffer — drop it from the list
        Key::Character(c) if !ctrl && c == "k" => {
            let mut bufs = buffers.write();
            let idx = *current.read();
            if bufs.len() > 1 {
                let (path, root) = (bufs[idx].path.clone(), bufs[idx].project_root.clone());
                bufs.remove(idx);
                drop(bufs);
                #[cfg(not(target_arch = "wasm32"))]
                if let (Some(p), Some(r)) = (path, root) {
                    let mgr = ctx.lsp.peek().clone();
                    mgr.did_close(&r, &p);
                }
                #[cfg(target_arch = "wasm32")]
                let _ = (path, root);
                current.set(idx.min(buffers.read().len().saturating_sub(1)));
            }
            true
        }
        // C-k ←: jump back through the LSP / rg navigation history.
        #[cfg(not(target_arch = "wasm32"))]
        Key::ArrowLeft => {
            crate::app::nav_back(&ctx);
            true
        }
        // C-k 1: close right pane and return focus to the editor.
        Key::Character(c) if !ctrl && c == "1" => {
            right_pane.set(None);
            focus.set(Pane::Left);
            if let Some(ref el) = *editor_el.read() {
                let _ = el.set_focus(true);
            }
            true
        }
        // C-k o: switch focus between editor and right pane.
        Key::Character(c) if !ctrl && c == "o" => {
            if right_pane.read().is_none() {
                return true;
            }
            let next = match *focus.read() {
                Pane::Left => Pane::Right,
                Pane::Right => Pane::Left,
            };
            focus.set(next);
            let target = match next {
                Pane::Left => editor_el.read().clone(),
                Pane::Right => right_pane_el.read().clone(),
            };
            if let Some(ref el) = target {
                let _ = el.set_focus(true);
            }
            true
        }
        _ => false,
    }
}

// ── Overlay key dispatch ───────────────────────────────────────────────────

pub(crate) fn handle_overlay_key(key: &Key, _ctrl: bool, ctx: AppCtx) {
    let AppCtx {
        mut buffers,
        mut current,
        mut overlay,
        mut right_pane,
        mut right_pane_selected,
        mut minibuf_msg,
        file_index,
        ..
    } = ctx;
    let _ = (&file_index, &right_pane, &right_pane_selected);

    let cur = overlay.read().clone();
    let Some(cur) = cur else { return };

    match (cur, key) {
        // Escape cancels undo-tree preview: restore the buffer as it was
        // when the visualizer opened.
        (Overlay::UndoTree { origin, .. }, Key::Escape) => {
            let idx = *current.read();
            let changed = {
                let mut bufs = buffers.write();
                bufs.get_mut(idx)
                    .map(|b| b.restore_undo_tree_node(origin))
                    .unwrap_or(false)
            };
            overlay.set(None);
            minibuf_msg.set(String::new());
            if changed {
                let mut tick = ctx.scroll_to_cursor_tick;
                let next = tick.peek().wrapping_add(1);
                tick.set(next);
            }
        }

        // Escape closes any other overlay.
        (_, Key::Escape) => {
            overlay.set(None);
            minibuf_msg.set(String::new());
        }

        // ── File picker ──
        (
            Overlay::FilePicker {
                mut query,
                selected,
            },
            k,
        ) => match k {
            Key::Enter => {
                activate_file_picker(&ctx, &query, selected);
            }
            // Tab acts as Enter once the user has arrow-navigated to a
            // specific result — they've picked something and want to open
            // it. With `selected == 0` (no navigation yet) it still does
            // path-mode autocomplete, since that's the natural Tab
            // affordance when you're just typing.
            Key::Tab if selected > 0 => {
                activate_file_picker(&ctx, &query, selected);
            }
            Key::Tab => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    if crate::files::is_path_query(&query) {
                        if let Some(new_query) = path_picker_tab_complete(&ctx, &query) {
                            overlay.set(Some(Overlay::FilePicker {
                                query: new_query,
                                selected: 0,
                            }));
                        }
                    }
                }
            }
            Key::Backspace => {
                query.pop();
                overlay.set(Some(Overlay::FilePicker { query, selected: 0 }));
            }
            Key::ArrowDown => {
                overlay.set(Some(Overlay::FilePicker {
                    query,
                    selected: selected + 1,
                }));
            }
            Key::ArrowUp => {
                overlay.set(Some(Overlay::FilePicker {
                    query,
                    selected: selected.saturating_sub(1),
                }));
            }
            Key::Character(c) => {
                query.push_str(c);
                overlay.set(Some(Overlay::FilePicker { query, selected: 0 }));
            }
            _ => {}
        },

        // ── Buffer switcher ──
        (
            Overlay::BufferSwitcher {
                mut query,
                selected,
            },
            k,
        ) => match k {
            Key::Enter => {
                let names: Vec<String> = buffers.read().iter().map(|b| b.name.clone()).collect();
                let filtered = filter_strings(&names, &query);
                if let Some((buf_idx, _)) = filtered.get(selected).cloned() {
                    current.set(buf_idx);
                }
                overlay.set(None);
                minibuf_msg.set(String::new());
            }
            Key::Backspace => {
                query.pop();
                overlay.set(Some(Overlay::BufferSwitcher { query, selected: 0 }));
            }
            Key::ArrowDown => {
                overlay.set(Some(Overlay::BufferSwitcher {
                    query,
                    selected: selected + 1,
                }));
            }
            Key::ArrowUp => {
                overlay.set(Some(Overlay::BufferSwitcher {
                    query,
                    selected: selected.saturating_sub(1),
                }));
            }
            Key::Character(c) => {
                query.push_str(c);
                overlay.set(Some(Overlay::BufferSwitcher { query, selected: 0 }));
            }
            _ => {}
        },

        // ── Undo tree visualizer ──
        (Overlay::UndoTree { selected, origin }, k) => match k {
            Key::Enter => {
                restore_undo_tree_node(&ctx, selected);
                overlay.set(None);
                minibuf_msg.set(String::new());
            }
            Key::ArrowDown => {
                let next = buffers
                    .read()
                    .get(*current.read())
                    .map(|b| b.undo_tree_first_child_node(selected))
                    .unwrap_or(selected);
                restore_undo_tree_node(&ctx, next);
                overlay.set(Some(Overlay::UndoTree {
                    selected: next,
                    origin,
                }));
            }
            Key::ArrowUp => {
                let next = buffers
                    .read()
                    .get(*current.read())
                    .map(|b| b.undo_tree_parent_node(selected))
                    .unwrap_or(selected);
                restore_undo_tree_node(&ctx, next);
                overlay.set(Some(Overlay::UndoTree {
                    selected: next,
                    origin,
                }));
            }
            Key::PageDown => {
                let next = buffers
                    .read()
                    .get(*current.read())
                    .map(|b| b.undo_tree_neighbor(selected, 10))
                    .unwrap_or(selected);
                restore_undo_tree_node(&ctx, next);
                overlay.set(Some(Overlay::UndoTree {
                    selected: next,
                    origin,
                }));
            }
            Key::PageUp => {
                let next = buffers
                    .read()
                    .get(*current.read())
                    .map(|b| b.undo_tree_neighbor(selected, -10))
                    .unwrap_or(selected);
                restore_undo_tree_node(&ctx, next);
                overlay.set(Some(Overlay::UndoTree {
                    selected: next,
                    origin,
                }));
            }
            Key::Home => {
                let next = buffers
                    .read()
                    .get(*current.read())
                    .map(|b| b.undo_tree_first_node())
                    .unwrap_or(selected);
                restore_undo_tree_node(&ctx, next);
                overlay.set(Some(Overlay::UndoTree {
                    selected: next,
                    origin,
                }));
            }
            Key::End => {
                let next = buffers
                    .read()
                    .get(*current.read())
                    .map(|b| b.undo_tree_last_node())
                    .unwrap_or(selected);
                restore_undo_tree_node(&ctx, next);
                overlay.set(Some(Overlay::UndoTree {
                    selected: next,
                    origin,
                }));
            }
            Key::ArrowLeft => {
                let next = buffers
                    .read()
                    .get(*current.read())
                    .map(|b| b.undo_tree_horizontal_node(selected, -1))
                    .unwrap_or(selected);
                restore_undo_tree_node(&ctx, next);
                overlay.set(Some(Overlay::UndoTree {
                    selected: next,
                    origin,
                }));
            }
            Key::ArrowRight => {
                let next = buffers
                    .read()
                    .get(*current.read())
                    .map(|b| b.undo_tree_horizontal_node(selected, 1))
                    .unwrap_or(selected);
                restore_undo_tree_node(&ctx, next);
                overlay.set(Some(Overlay::UndoTree {
                    selected: next,
                    origin,
                }));
            }
            Key::Character(c) if c.eq_ignore_ascii_case("q") => {
                restore_undo_tree_node(&ctx, origin);
                overlay.set(None);
                minibuf_msg.set(String::new());
            }
            _ => {}
        },

        // ── Ripgrep prompt ──
        (
            Overlay::RgPrompt {
                mut query,
                mut cursor,
            },
            k,
        ) => match k {
            Key::Enter => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let Some(root) = crate::app::active_project_root(&ctx) else {
                        overlay.set(None);
                        minibuf_msg.set("rg: no project loaded".to_string());
                        return;
                    };
                    let out = commands::run_ripgrep(&query, root.clone());
                    right_pane.set(Some(RightPaneState {
                        title: format!("*rg: {query}*"),
                        output: out,
                        cwd: root,
                    }));
                    right_pane_selected.set(0);
                }
                #[cfg(target_arch = "wasm32")]
                {
                    minibuf_msg.set("ripgrep not supported on wasm".to_string());
                }
                overlay.set(None);
                #[cfg(not(target_arch = "wasm32"))]
                minibuf_msg.set(String::new());
            }
            Key::Backspace => {
                if cursor > 0 {
                    let prev = prev_char_boundary(&query, cursor);
                    query.replace_range(prev..cursor, "");
                    cursor = prev;
                }
                overlay.set(Some(Overlay::RgPrompt { query, cursor }));
            }
            Key::Delete => {
                if cursor < query.len() {
                    let next = next_char_boundary(&query, cursor);
                    query.replace_range(cursor..next, "");
                }
                overlay.set(Some(Overlay::RgPrompt { query, cursor }));
            }
            Key::ArrowLeft => {
                cursor = prev_char_boundary(&query, cursor);
                overlay.set(Some(Overlay::RgPrompt { query, cursor }));
            }
            Key::ArrowRight => {
                cursor = next_char_boundary(&query, cursor);
                overlay.set(Some(Overlay::RgPrompt { query, cursor }));
            }
            Key::Home => {
                overlay.set(Some(Overlay::RgPrompt { query, cursor: 0 }));
            }
            Key::End => {
                cursor = query.len();
                overlay.set(Some(Overlay::RgPrompt { query, cursor }));
            }
            Key::Character(c) => {
                query.insert_str(cursor, c);
                cursor += c.len();
                overlay.set(Some(Overlay::RgPrompt { query, cursor }));
            }
            _ => {}
        },
    }
}

fn restore_undo_tree_node(ctx: &AppCtx, node: usize) -> bool {
    let mut buffers = ctx.buffers;
    let current = ctx.current;
    let idx = *current.read();
    let changed = {
        let mut bufs = buffers.write();
        bufs.get_mut(idx)
            .map(|b| b.restore_undo_tree_node(node))
            .unwrap_or(false)
    };
    if changed {
        let mut tick = ctx.scroll_to_cursor_tick;
        let next = tick.peek().wrapping_add(1);
        tick.set(next);
    }
    changed
}

/// Activate the file picker's current entry. Called by Enter, and by Tab
/// once the user has arrow-navigated (so a deliberate selection exists).
///
/// Routes to the path-mode handler for foreign-tree queries; otherwise
/// resolves the selected index against the project file index (with the
/// same dotfile filter the picker view uses) and opens it under the active
/// project root.
fn activate_file_picker(ctx: &AppCtx, query: &str, selected: usize) {
    let mut overlay = ctx.overlay;
    let mut minibuf_msg = ctx.minibuf_msg;
    #[cfg(not(target_arch = "wasm32"))]
    {
        if crate::files::is_path_query(query) {
            handle_path_picker_enter(ctx, query, selected);
            return;
        }
        let files = ctx.file_index.read().clone();
        // Mirror the OverlayView's render-side dotfile filter so the
        // `selected` index lines up with what the user actually saw.
        let show_hidden = query.starts_with('.');
        let visible: Vec<_> = if show_hidden {
            files
        } else {
            files
                .into_iter()
                .filter(|p| !crate::files::is_dotfile_path(p))
                .collect()
        };
        let filtered = filter_paths(&visible, query);
        // Project-relative file picker only runs when a project is loaded
        // — the index is populated when C-k C-f was invoked.
        let Some(root) = crate::app::active_project_root(ctx) else {
            overlay.set(None);
            minibuf_msg.set("file: no project loaded".to_string());
            return;
        };
        let full = if let Some((_, picked)) = filtered.get(selected).cloned() {
            root.join(&picked)
        } else {
            root.join(query)
        };
        if let Some(parent) = full.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                minibuf_msg.set(format!("create failed: {e}"));
                overlay.set(None);
                return;
            }
        }
        if !full.exists() {
            if let Err(e) = std::fs::File::create(&full) {
                minibuf_msg.set(format!("create failed: {e}"));
                overlay.set(None);
                return;
            }
        }
        match crate::app::open_file_at(ctx.buffers, ctx.current, full, 0, 0) {
            Ok(()) => {
                overlay.set(None);
                minibuf_msg.set(String::new());
                let mut tick = ctx.scroll_to_cursor_tick;
                let next = tick.peek().wrapping_add(1);
                tick.set(next);
            }
            Err(e) => {
                minibuf_msg.set(format!("open failed: {e}"));
                overlay.set(None);
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (query, selected);
        overlay.set(None);
        minibuf_msg.set("file open not supported on wasm".to_string());
    }
}

/// Tab in path mode. If a unique entry matches the suffix, replace it with
/// that entry's full name (and a trailing `/` if it's a directory); if many
/// match, extend the suffix to the longest common prefix. Returns the new
/// query string, or None if nothing changed.
#[cfg(not(target_arch = "wasm32"))]
fn path_picker_tab_complete(ctx: &AppCtx, query: &str) -> Option<String> {
    let view = crate::overlay::compute_path_picker_view(ctx, query);
    if view.matches.is_empty() {
        return None;
    }
    let base = view.base_dir.to_string_lossy().into_owned();
    let base_with_slash = if base.ends_with('/') {
        base.clone()
    } else {
        format!("{base}/")
    };
    if view.matches.len() == 1 {
        let (name, is_dir) = &view.matches[0];
        let suffix = if *is_dir { "/" } else { "" };
        let new = format!("{base_with_slash}{name}{suffix}");
        if new == query {
            None
        } else {
            Some(new)
        }
    } else {
        let names: Vec<&str> = view.matches.iter().map(|(n, _)| n.as_str()).collect();
        let lcp = crate::files::longest_common_prefix(&names);
        if lcp.len() <= view.suffix.len() {
            return None;
        }
        Some(format!("{base_with_slash}{lcp}"))
    }
}

/// Enter in path mode. Resolves the query — if it's a directory we descend
/// (rewriting the query to end with `/`); if it's a regular file we open it,
/// switching the active project when the file lives outside the current one.
#[cfg(not(target_arch = "wasm32"))]
fn handle_path_picker_enter(ctx: &AppCtx, query: &str, selected: usize) {
    let mut overlay = ctx.overlay;
    let mut minibuf_msg = ctx.minibuf_msg;
    let buffers = ctx.buffers;
    let current = ctx.current;
    let mut scroll_to_cursor_tick = ctx.scroll_to_cursor_tick;

    let view = crate::overlay::compute_path_picker_view(ctx, query);
    let base_with_slash = {
        let s = view.base_dir.to_string_lossy().into_owned();
        if s.ends_with('/') {
            s
        } else {
            format!("{s}/")
        }
    };

    // The user may have typed a full path with no further matches (e.g. typing
    // an exact name then hitting Enter while the suffix is non-empty but
    // unique). Resolve "selected" against the filtered list first; if empty,
    // try the literal query.
    let resolved: std::path::PathBuf = if !view.matches.is_empty() {
        let idx = selected.min(view.matches.len() - 1);
        let (name, _) = &view.matches[idx];
        std::path::PathBuf::from(format!("{base_with_slash}{name}"))
    } else {
        // Resolve relative segments against the active project; fall back
        // to cwd when no project is loaded.
        let cwd_fallback =
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let active = ctx.project_root.read().clone().unwrap_or(cwd_fallback);
        std::path::PathBuf::from(crate::files::expand_path_query(query, &active))
    };

    if resolved.is_dir() {
        let mut s = resolved.to_string_lossy().into_owned();
        if !s.ends_with('/') {
            s.push('/');
        }
        overlay.set(Some(Overlay::FilePicker {
            query: s,
            selected: 0,
        }));
        return;
    }

    if !resolved.exists() {
        if let Some(parent) = resolved.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                minibuf_msg.set(format!("create failed: {e}"));
                overlay.set(None);
                return;
            }
        }
        if let Err(e) = std::fs::File::create(&resolved) {
            minibuf_msg.set(format!("create failed: {e}"));
            overlay.set(None);
            return;
        }
    } else if !resolved.is_file() {
        minibuf_msg.set(format!("not a file: {}", resolved.display()));
        overlay.set(None);
        return;
    }

    match crate::app::open_file_at(buffers, current, resolved.clone(), 0, 0) {
        Ok(()) => {
            // Decide what project root to load with the file:
            //   * the buffer's `find_project_root` (.git / .projectile)
            //     ancestor, if any — preserves Cargo-style project rooting;
            //   * otherwise the file's parent directory — the user-specified
            //     requirement is that any foreign-tree open loads a project,
            //     even when there's no `.git` ancestor.
            let new_root = {
                let bufs = buffers.read();
                let idx = *current.read();
                bufs.get(idx).and_then(|b| {
                    b.project_root.clone().or_else(|| {
                        b.path
                            .as_ref()
                            .and_then(|p| p.parent().map(|q| q.to_path_buf()))
                    })
                })
            };
            if let Some(new_root) = new_root {
                crate::app::switch_active_project(ctx, new_root);
            }
            overlay.set(None);
            minibuf_msg.set(String::new());
            let next = scroll_to_cursor_tick.peek().wrapping_add(1);
            scroll_to_cursor_tick.set(next);
        }
        Err(e) => {
            minibuf_msg.set(format!("open failed: {e}"));
            overlay.set(None);
        }
    }
}

fn prev_char_boundary(s: &str, idx: usize) -> usize {
    let mut i = idx.min(s.len());
    if i == 0 {
        return 0;
    }
    i -= 1;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_char_boundary(s: &str, idx: usize) -> usize {
    let mut i = idx.min(s.len());
    if i >= s.len() {
        return s.len();
    }
    i += 1;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

// ── LSP completion (C-Space) ──────────────────────────────────────────────

/// Trigger or re-trigger an LSP completion popup at the current cursor.
///
/// Computes the identifier-prefix start (the byte offset where the chosen
/// completion will replace text *back to*) and sends a
/// `textDocument/completion` request. The popup is placed into
/// [`AppCtx::completion`] immediately with `loading: true` so the user sees
/// a placeholder; the response handler later populates `items`.
///
/// Bails (with a minibuffer hint) when the buffer isn't a Rust file under
/// a known project or rust-analyzer hasn't reached Ready/Indexing yet —
/// firing a completion request during a cold index would otherwise hang the
/// popup for minutes.
#[cfg(not(target_arch = "wasm32"))]
fn trigger_completion(ctx: AppCtx) {
    let mut completion = ctx.completion;
    let mut minibuf_msg = ctx.minibuf_msg;

    let idx = *ctx.current.read();
    let bufs = ctx.buffers.read();
    let Some(b) = bufs.get(idx) else { return };
    let Some(path) = b.path.clone() else {
        minibuf_msg.set("complete: scratch buffer has no file".into());
        return;
    };
    let Some(root) = b.project_root.clone() else {
        minibuf_msg.set("complete: buffer has no project root".into());
        return;
    };
    let cur_off = b.offset();
    let text = b.text();
    let buf_version = b.version;

    // Walk left through identifier chars to find the prefix the user is
    // typing — that's the range the completion will replace.
    let mut start = cur_off;
    while start > 0 {
        let prev = prev_char_boundary(&text, start);
        let Some(c) = text[prev..start].chars().next() else {
            break;
        };
        if !(c.is_ascii_alphanumeric() || c == '_') {
            break;
        }
        start = prev;
    }
    let (anchor_line, anchor_col) = b.offset_to_line_col(start);
    let line = b.cursor_line();
    let line_start = b.rope.offset_of_line(line);
    let byte_col = cur_off.saturating_sub(line_start) as u32;
    drop(bufs);

    let mgr = ctx.lsp.peek().clone();
    let status = mgr.status_for(&root);
    match status {
        crate::lsp::LspStatus::Ready | crate::lsp::LspStatus::Indexing { .. } => {}
        crate::lsp::LspStatus::Starting => {
            minibuf_msg.set("lsp: rust-analyzer still starting".into());
            return;
        }
        crate::lsp::LspStatus::Disabled => {
            minibuf_msg.set("lsp: no rust-analyzer for this buffer".into());
            return;
        }
        crate::lsp::LspStatus::Error(e) => {
            minibuf_msg.set(format!("lsp: no rust-analyzer ({e})"));
            return;
        }
    }

    // Sync the document synchronously before issuing the completion request.
    // The doc-sync effect would catch up eventually but might race the
    // outgoing request, leaving the server one keystroke behind.
    if mgr.needs_sync(&root, &path, buf_version) {
        mgr.did_open(&root, &path, buf_version, &text);
        mgr.did_change(&root, &path, buf_version, &text);
    }

    let Some(request_id) = mgr.request_completion(&root, &path, line as u32, byte_col) else {
        minibuf_msg.set("lsp: no session for this file".into());
        return;
    };

    completion.set(Some(crate::app::CompletionState {
        request_id,
        buffer_idx: idx,
        replace_start: start,
        anchor_line,
        anchor_col,
        items: Vec::new(),
        selected: 0,
        loading: true,
    }));
}

/// Handle a keystroke while the completion popup is open. Returns true when
/// the popup consumed the key (navigation, accept, dismiss). Returns false
/// for keys that should fall through to normal editor handling — the caller
/// is responsible for dismissing the popup in that case (except for the
/// C-Space re-trigger path, which intentionally keeps the popup alive so
/// the trigger handler can replace it).
#[cfg(not(target_arch = "wasm32"))]
fn completion_handle_key(key: &Key, ctrl: bool, ctx: AppCtx) -> bool {
    let mut completion = ctx.completion;
    let mut buffers = ctx.buffers;
    let mut minibuf_msg = ctx.minibuf_msg;

    match key {
        Key::ArrowDown => {
            let Some(mut state) = completion.read().clone() else {
                return true;
            };
            if !state.items.is_empty() {
                state.selected = (state.selected + 1).min(state.items.len() - 1);
                completion.set(Some(state));
            }
            true
        }
        Key::ArrowUp => {
            let Some(mut state) = completion.read().clone() else {
                return true;
            };
            state.selected = state.selected.saturating_sub(1);
            completion.set(Some(state));
            true
        }
        Key::Enter | Key::Tab => {
            let Some(state) = completion.read().clone() else {
                return true;
            };
            if let Some(item) = state.items.get(state.selected) {
                let mut bufs = buffers.write();
                if let Some(b) = bufs.get_mut(state.buffer_idx) {
                    // Replace [replace_start, cursor] with insert_text via
                    // Buffer::insert, which deletes the selection first.
                    let cur_off = b.offset();
                    if state.replace_start <= cur_off && cur_off <= b.len() {
                        b.selection_anchor = Some(state.replace_start);
                        b.cursor.offset = cur_off;
                        b.insert(&item.insert_text);
                    } else {
                        b.insert(&item.insert_text);
                    }
                }
            }
            completion.set(None);
            minibuf_msg.set(String::new());
            true
        }
        Key::Escape => {
            completion.set(None);
            minibuf_msg.set(String::new());
            true
        }
        Key::Character(c) if ctrl && c.eq_ignore_ascii_case("g") => {
            completion.set(None);
            minibuf_msg.set(String::new());
            true
        }
        _ => false,
    }
}

/// Floating completion popup. Rendered inside `#code-area` so its
/// coordinates share the cursor's scrolled coordinate space — the popup
/// stays anchored to the trigger position when the buffer scrolls.
///
/// Subscribes to `ctx.completion` so it re-renders on selection / item
/// updates without forcing a full `CodeEditor` re-render.
#[component]
fn CompletionPopup(char_width: f64, line_height: f64) -> Element {
    let ctx: AppCtx = use_context();
    let state = ctx.completion.read().clone();
    let Some(state) = state else {
        return rsx! {};
    };

    let top = LAYER_PAD + ((state.anchor_line + 1) as f64) * line_height;
    let left = LAYER_PAD + (state.anchor_col as f64) * char_width;

    const MAX_VISIBLE: usize = 12;
    let n = state.items.len();
    let sel = if n == 0 { 0 } else { state.selected.min(n - 1) };
    let start = sel.saturating_sub(MAX_VISIBLE - 1);
    let end = (start + MAX_VISIBLE).min(n);

    rsx! {
        div {
            class: "completion-popup",
            style: "left: {left}px; top: {top}px;",
            if state.loading && state.items.is_empty() {
                div { class: "completion-empty", "Loading…" }
            } else if state.items.is_empty() {
                div { class: "completion-empty", "(no completions)" }
            } else {
                for i in start..end {
                    {
                        let item = &state.items[i];
                        let cls = if i == sel {
                            "completion-item completion-selected"
                        } else {
                            "completion-item"
                        };
                        rsx! {
                            div { class: "{cls}", key: "{i}",
                                span { class: "completion-label", "{item.label}" }
                                if !item.detail.is_empty() {
                                    span { class: "completion-detail", " — {item.detail}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
