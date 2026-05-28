//! Overlay components: file picker, buffer switcher, ripgrep prompt, plus the
//! right-hand command output pane.

use crate::app::{AppCtx, Overlay, Pane, RightPaneState};
use crate::buffer::Buffer;
#[cfg(not(target_arch = "wasm32"))]
use crate::commands;
use crate::files;
use dioxus::html::geometry::PixelsVector2D;
use dioxus::prelude::*;
use dioxus_native_dom::NodeHandle;
use std::path::PathBuf;

/// Matches `.right-line { height: 28px }` in `styles.css`. Layout-engine
/// cost dominates if we measure each row, so the row geometry is hard-coded.
const RIGHT_LINE_H: f64 = 28.0;
/// Matches `#right-body { padding: 4px 8px }` — top padding shifts row
/// origins by this amount.
const RIGHT_BODY_PAD_TOP: f64 = 4.0;
/// Rows of context to keep visible above/below the selected row.
const RIGHT_SCROLL_MARGIN_LINES: usize = 2;

const MAX_RESULTS: usize = 20;
/// Approximate Liberation Mono character width at `.overlay-prompt`'s 20px font.
/// Used only to keep the file-picker caret scrolled into view.
const OVERLAY_QUERY_CHAR_W: f64 = 12.0;
const OVERLAY_QUERY_SCROLL_MARGIN: f64 = 36.0;

#[component]
pub fn OverlayView() -> Element {
    puffin::profile_function!();
    let ctx: AppCtx = use_context();

    // Mounted handle for the undo-tree scroll viewport. Captured in
    // `onmounted` on the visual list div; consumed by the use_effect
    // below to scroll the selected node into view when navigation moves
    // it past the visible region. Has to live at component top-level so
    // hook order stays consistent across overlay-kind transitions.
    let mut undo_viewport_el: Signal<Option<MountedEvent>> = use_signal(|| None);
    let mut file_picker_query_el: Signal<Option<MountedEvent>> = use_signal(|| None);
    let mut file_picker_query_scroll = use_signal(|| 0.0f64);

    // Keep the file picker's logical caret visible when the query is wider
    // than the prompt. Home/End and cursor movement update Overlay::cursor;
    // this effect mirrors that to the horizontal scroll offset.
    use_effect(move || {
        let overlay = ctx.overlay.read().clone();
        let Some(Overlay::FilePicker { query, cursor, .. }) = overlay else {
            file_picker_query_scroll.set(0.0);
            return;
        };
        let Some(handle) = file_picker_query_el.read().clone() else {
            return;
        };
        let viewport_w = handle
            .downcast::<NodeHandle>()
            .and_then(|h| h.client_rect_sync())
            .map(|r| r.size.width)
            .unwrap_or(0.0);
        if viewport_w <= 0.0 {
            return;
        }
        let cursor = clamp_cursor_to_char_boundary(&query, cursor);
        let cursor_col = query[..cursor].chars().count() as f64;
        let content_w = (query.chars().count() as f64) * OVERLAY_QUERY_CHAR_W;
        let max_scroll = (content_w - viewport_w).max(0.0);
        let cursor_x = cursor_col * OVERLAY_QUERY_CHAR_W;
        let stored_scroll = *file_picker_query_scroll.peek();
        let scroll = stored_scroll.min(max_scroll);
        let desired = if cursor == 0 {
            0.0
        } else if cursor_x < scroll + OVERLAY_QUERY_SCROLL_MARGIN {
            (cursor_x - OVERLAY_QUERY_SCROLL_MARGIN).max(0.0)
        } else if cursor_x > scroll + viewport_w - OVERLAY_QUERY_SCROLL_MARGIN {
            (cursor_x + OVERLAY_QUERY_SCROLL_MARGIN - viewport_w).max(0.0)
        } else {
            scroll
        }
        .min(max_scroll);

        if (desired - stored_scroll).abs() > 0.5 {
            let _ = handle.scroll(PixelsVector2D::new(desired, 0.0), ScrollBehavior::Instant);
            file_picker_query_scroll.set(desired);
        }
    });

    // Auto-scroll the undo-tree viewport so the selected node is in view.
    // Re-runs whenever the overlay signal changes — keyboard navigation
    // writes a new `Overlay::UndoTree { selected }` on every move.
    #[cfg(not(target_arch = "wasm32"))]
    use_effect(move || {
        let overlay = ctx.overlay.read().clone();
        let Some(Overlay::UndoTree { selected, .. }) = overlay else {
            return;
        };
        let handle = undo_viewport_el.peek().clone();
        let Some(handle) = handle else { return };
        let Some(node_handle) = handle.downcast::<NodeHandle>() else {
            return;
        };
        let Some(rect) = node_handle.client_rect_sync() else {
            return;
        };
        let viewport_h = rect.size.height;
        if viewport_h <= 0.0 {
            return;
        }
        let target_y = {
            let bufs = ctx.buffers.peek();
            let idx = *ctx.current.peek();
            bufs.get(idx).and_then(|b| {
                let graph = b.undo_tree_visual_graph(selected);
                graph
                    .nodes
                    .into_iter()
                    .find(|n| n.id == selected)
                    .map(|n| n.y)
            })
        };
        let Some(target_y) = target_y else { return };
        // Centre the node in the viewport, clamped to >= 0.
        let desired = (target_y - viewport_h * 0.5).max(0.0);
        let _ = handle.scroll(PixelsVector2D::new(0.0, desired), ScrollBehavior::Instant);
    });

    let overlay_opt = ctx.overlay.read().clone();
    let Some(overlay) = overlay_opt else {
        return rsx! {};
    };

    match overlay {
        Overlay::FilePicker {
            query,
            cursor,
            selected,
        } => {
            if files::is_path_query(&query) {
                render_path_picker(&ctx, &query, cursor, selected, file_picker_query_el)
            } else {
                let cursor = clamp_cursor_to_char_boundary(&query, cursor);
                let before = &query[..cursor];
                let after = &query[cursor..];
                let files = ctx.file_index.read().clone();
                // Hide dotfiles from the project picker unless the user
                // started the query with `.` (mirrors the path-picker rule).
                let show_hidden = query.starts_with('.');
                let visible_index: Vec<_> = if show_hidden {
                    files
                } else {
                    files
                        .into_iter()
                        .filter(|p| !files::is_dotfile_path(p))
                        .collect()
                };
                let filtered = filter_paths(&visible_index, &query);
                let is_empty = filtered.is_empty();
                let selected = selected.min(filtered.len().saturating_sub(1));
                let start = selected.saturating_sub(MAX_RESULTS.saturating_sub(1));
                let visible: Vec<(usize, String)> =
                    filtered.into_iter().skip(start).take(MAX_RESULTS).collect();

                rsx! {
                    div { class: "overlay",
                        div { class: "overlay-box",
                            div { class: "overlay-prompt",
                                span { class: "overlay-label", "Find file: " }
                                span {
                                    class: "overlay-query-window",
                                    onmounted: move |evt: MountedEvent| {
                                        file_picker_query_el.set(Some(evt));
                                    },
                                    span { class: "overlay-query", "{before}" }
                                    span { class: "overlay-caret", " " }
                                    span { class: "overlay-query", "{after}" }
                                }
                            }
                            div { class: "overlay-list",
                                for (i, item) in visible.into_iter().enumerate() {
                                    {
                                        let result_idx = start + i;
                                        let cls = if result_idx == selected { "overlay-item selected" } else { "overlay-item" };
                                        rsx! { div { class: "{cls}", "{item.1}" } }
                                    }
                                }
                                if is_empty {
                                    div { class: "overlay-empty", "(no matches)" }
                                }
                            }
                        }
                    }
                }
            }
        }
        Overlay::BufferSwitcher { query, selected } => {
            let bufs = ctx.buffers.read();
            let labels = buffer_switcher_labels(&bufs);
            let dirty: Vec<bool> = bufs.iter().map(|b| b.dirty).collect();
            drop(bufs);
            let filtered = filter_strings(&labels, &query);
            let is_empty = filtered.is_empty();
            let selected = selected.min(filtered.len().saturating_sub(1));
            let start = selected.saturating_sub(MAX_RESULTS.saturating_sub(1));
            let visible: Vec<(usize, String)> =
                filtered.into_iter().skip(start).take(MAX_RESULTS).collect();

            rsx! {
                div { class: "overlay",
                    div { class: "overlay-box",
                        div { class: "overlay-prompt",
                            span { class: "overlay-label", "Switch to buffer: " }
                            span { class: "overlay-query", "{query}" }
                            span { class: "overlay-caret", " " }
                        }
                        div { class: "overlay-list",
                            for (i, item) in visible.into_iter().enumerate() {
                                {
                                    let result_idx = start + i;
                                    let cls = if result_idx == selected { "overlay-item selected" } else { "overlay-item" };
                                    let prefix = if dirty.get(item.0).copied().unwrap_or(false) { "*" } else { "" };
                                    rsx! { div { class: "{cls}", "{prefix}{item.1}" } }
                                }
                            }
                            if is_empty {
                                div { class: "overlay-empty", "(no matches)" }
                            }
                        }
                    }
                }
            }
        }
        Overlay::RgPrompt { query, cursor } => {
            let cursor = cursor.min(query.len());
            let before = &query[..cursor];
            let after = &query[cursor..];
            rsx! {
                div { class: "overlay",
                    div { class: "overlay-box narrow",
                        div { class: "overlay-prompt",
                            span { class: "overlay-label", "Ripgrep: " }
                            span { class: "overlay-query", "{before}" }
                            span { class: "overlay-caret", " " }
                            span { class: "overlay-query", "{after}" }
                        }
                        div { class: "overlay-hint",
                            "Enter to search, Esc to cancel"
                        }
                    }
                }
            }
        }
        Overlay::UndoTree { selected, .. } => {
            let bufs = ctx.buffers.read();
            let idx = *ctx.current.read();
            let (name, graph) = bufs
                .get(idx)
                .map(|b| (b.name.clone(), b.undo_tree_visual_graph(selected)))
                .unwrap_or_else(|| {
                    (
                        "<no buffer>".to_string(),
                        crate::buffer::UndoTreeVisualGraph {
                            width: 180.0,
                            height: 80.0,
                            nodes: Vec::new(),
                            edges: Vec::new(),
                        },
                    )
                });
            drop(bufs);
            let is_empty = graph.nodes.is_empty();
            let graph_w = graph.width;
            let graph_h = graph.height;
            rsx! {
                div { class: "overlay",
                    div { class: "overlay-box undo-tree-box",
                        div { class: "overlay-prompt",
                            span { class: "overlay-label", "Undo tree: " }
                            span { class: "overlay-query", "{name}" }
                        }
                        div {
                            class: "overlay-list undo-tree-list undo-tree-visual",
                            onmounted: move |evt: MountedEvent| {
                                undo_viewport_el.set(Some(evt));
                            },
                            if is_empty {
                                div { class: "overlay-empty", "(no undo history)" }
                            } else {
                                div {
                                    class: "undo-tree-canvas",
                                    style: "width: {graph_w}px; height: {graph_h}px;",
                                    for edge in graph.edges.into_iter() {
                                        {
                                            let cls = if edge.active { "undo-tree-edge active" } else { "undo-tree-edge" };
                                            rsx! {
                                                div {
                                                    key: "edge-{edge.parent}-{edge.child}",
                                                    class: "{cls}",
                                                    style: "left: {edge.left}px; top: {edge.top}px; width: {edge.width}px; transform: rotate({edge.angle_deg}deg);",
                                                }
                                            }
                                        }
                                    }
                                    for node in graph.nodes.into_iter() {
                                        {
                                            let cls = if node.selected {
                                                "undo-tree-node selected"
                                            } else if node.current {
                                                "undo-tree-node current"
                                            } else {
                                                "undo-tree-node"
                                            };
                                            let left = node.x - 9.0;
                                            let top = node.y - 9.0;
                                            rsx! {
                                                div {
                                                    key: "node-{node.id}",
                                                    class: "{cls}",
                                                    style: "left: {left}px; top: {top}px;",
                                                    div { class: "undo-tree-node-core" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn RightPaneView() -> Element {
    puffin::profile_function!();
    let ctx: AppCtx = use_context();
    let mut right_pane_el = ctx.right_pane_el;
    let mut right_pane_selected = ctx.right_pane_selected;
    let mut focus = ctx.focus;
    let mut right_body_el: Signal<Option<MountedEvent>> = use_signal(|| None);
    let mut right_body_scroll = use_signal(|| 0.0f64);

    // Output lines live behind an `Arc<Mutex<Vec<String>>>` updated from a
    // worker thread — Dioxus can't track that. Tick a local signal while the
    // command is running so the view refreshes. Tokio timers are unavailable
    // on wasm (and commands never run there), so the loop is gated off.
    let mut tick = use_signal(|| 0u64);

    #[cfg(not(target_arch = "wasm32"))]
    use_effect(move || {
        let pane = ctx.right_pane.read().clone();
        let Some(pane) = pane else { return };
        let output = pane.output.clone();
        spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                let done = output.is_done();
                let next = tick.peek().wrapping_add(1);
                tick.set(next);
                if done {
                    break;
                }
            }
        });
    });

    // Keyboard navigation through results doesn't auto-scroll the body,
    // so the selected row drifts off-screen. Watch the selection and bring
    // the corresponding raw line into view, with a few rows of context.
    use_effect(move || {
        let raw_selected = *right_pane_selected.read();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let pane = ctx.right_pane.peek().clone();
            let Some(pane) = pane else { return };
            let lines = pane.output.snapshot();
            let root = pane.cwd.clone();
            let result_indices: Vec<usize> = lines
                .iter()
                .enumerate()
                .filter_map(|(i, l)| commands::parse_location(l, &root).map(|_| i))
                .collect();
            if result_indices.is_empty() {
                return;
            }
            let sel = raw_selected.min(result_indices.len() - 1);
            let row_idx = result_indices[sel];

            let Some(body) = right_body_el.peek().clone() else {
                return;
            };
            let viewport_h = body
                .downcast::<NodeHandle>()
                .and_then(|h| h.client_rect_sync())
                .map(|r| r.size.height)
                .unwrap_or(0.0);
            if viewport_h <= 0.0 {
                return;
            }
            let st = *right_body_scroll.peek();
            let row_top = RIGHT_BODY_PAD_TOP + (row_idx as f64) * RIGHT_LINE_H;
            let row_bottom = row_top + RIGHT_LINE_H;
            let margin = (RIGHT_SCROLL_MARGIN_LINES as f64) * RIGHT_LINE_H;

            let new_st = if row_top < st + margin {
                (row_top - margin).max(0.0)
            } else if row_bottom > st + viewport_h - margin {
                (row_bottom - viewport_h + margin).max(0.0)
            } else {
                return;
            };

            let _ = body.scroll(PixelsVector2D::new(0.0, new_st), ScrollBehavior::Instant);
            right_body_scroll.set(new_st);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = raw_selected;
        }
    });

    let _ = *tick.read();

    let pane = ctx.right_pane.read().clone();
    let Some(RightPaneState { title, output, cwd }) = pane else {
        return rsx! {};
    };

    let lines = output.snapshot();
    let done = output.is_done();

    // Pre-compute which raw line indices are clickable locations. Indexed by
    // raw line index so click/keyboard handlers can look them up cheaply.
    #[cfg(not(target_arch = "wasm32"))]
    let results: Vec<(usize, PathBuf, usize, usize)> = {
        lines
            .iter()
            .enumerate()
            .filter_map(|(i, l)| commands::parse_location(l, &cwd).map(|(p, ln, c)| (i, p, ln, c)))
            .collect()
    };
    #[cfg(target_arch = "wasm32")]
    let _ = cwd;
    #[cfg(target_arch = "wasm32")]
    let results: Vec<(usize, PathBuf, usize, usize)> = Vec::new();

    let result_count = results.len();
    let raw_selected = *right_pane_selected.read();
    let sel_clamped = if result_count == 0 {
        0
    } else {
        raw_selected.min(result_count - 1)
    };
    // Map result index → raw line index so we can mark the right line as
    // selected while iterating.
    let selected_line_idx = results.get(sel_clamped).map(|r| r.0);
    let pane_active = matches!(*focus.read(), Pane::Right);

    rsx! {
        div {
            id: "right-panel",
            class: if pane_active { "pane-active" } else { "pane-inactive" },
            tabindex: "0",
            onmounted: move |evt: MountedEvent| {
                right_pane_el.set(Some(evt.clone()));
                if matches!(*focus.peek(), Pane::Right) {
                    let _ = evt.set_focus(true);
                }
            },
            onkeydown: move |evt: Event<KeyboardData>| {
                puffin::profile_scope!("right-pane: onkeydown");
                handle_right_pane_key(&evt, ctx, result_count);
            },
            onmousedown: move |_| {
                focus.set(Pane::Right);
            },
            div {
                id: "right-header",
                class: if pane_active { "active" } else { "inactive" },
                span { class: "right-title", "{title}" }
                span { class: "right-status",
                    if done { "done" } else { "running..." }
                }
            }
            div { id: "right-body",
                onmounted: move |evt: MountedEvent| {
                    right_body_el.set(Some(evt));
                },
                onscroll: move |evt: Event<ScrollData>| {
                    right_body_scroll.set(evt.data().scroll_top());
                },
                for (i, line) in lines.iter().enumerate() {
                    {
                        let is_selected = Some(i) == selected_line_idx;
                        // Look up whether this line is a clickable result, and
                        // if so its 0-indexed result position so we can update
                        // `right_pane_selected` on click.
                        let click_result_idx = results
                            .iter()
                            .position(|(li, _, _, _)| *li == i);
                        let mut classes = String::from("right-line");
                        if click_result_idx.is_some() {
                            classes.push_str(" rg-clickable");
                        }
                        if is_selected {
                            classes.push_str(" rg-selected");
                        }
                        let on_click = click_result_idx.map(|ri| ri);
                        rsx! {
                            div {
                                class: "{classes}",
                                key: "{i}",
                                onclick: move |_| {
                                    let Some(ri) = on_click else { return };
                                    right_pane_selected.set(ri);
                                    activate_result(ctx, ri);
                                },
                                RgLine { text: line.clone() }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn RgLine(text: String) -> Element {
    // Render path:line: prefix as a distinct span if present.
    let mut parts = text.splitn(3, ':');
    let a = parts.next().unwrap_or("");
    let b = parts.next();
    let c = parts.next();
    match (b, c) {
        (Some(b), Some(c)) if b.parse::<usize>().is_ok() && !a.is_empty() && is_path_like(a) => {
            rsx! {
                span { class: "rg-path", "{a}" }
                span { class: "rg-colon", ":" }
                span { class: "rg-lineno", "{b}" }
                span { class: "rg-colon", ":" }
                span { class: "rg-text", "{c}" }
            }
        }
        _ => rsx! { span { class: "rg-text", "{text}" } },
    }
}

fn is_path_like(s: &str) -> bool {
    if s.contains('/') {
        return true;
    }
    let lower = s.to_ascii_lowercase();
    [
        ".rs", ".toml", ".md", ".css", ".html", ".js", ".sh", ".c", ".h", ".cc",
        ".hh", ".cpp", ".hpp", ".cxx", ".hxx",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

fn handle_right_pane_key(evt: &Event<KeyboardData>, ctx: AppCtx, result_count: usize) {
    let key = evt.key();
    let modifiers = evt.modifiers();
    let ctrl = modifiers.contains(Modifiers::CONTROL) || modifiers.contains(Modifiers::META);

    let mut ck_prefix = ctx.ck_prefix;
    let mut minibuf_msg = ctx.minibuf_msg;
    let mut right_pane_selected = ctx.right_pane_selected;

    // C-k prefix is global — also active in the right pane so the user can
    // do things like C-k o (switch back), C-k 1 (close), C-k b (switch buf).
    if *ck_prefix.read() {
        ck_prefix.set(false);
        minibuf_msg.set(String::new());
        crate::editor::handle_ck_command(&key, ctrl, ctx);
        return;
    }
    if ctrl && matches!(&key, Key::Character(c) if c.eq_ignore_ascii_case("k")) {
        ck_prefix.set(true);
        minibuf_msg.set("C-k -".to_string());
        return;
    }
    if ctrl && matches!(&key, Key::Character(c) if c == "g") {
        minibuf_msg.set(String::new());
        ck_prefix.set(false);
        return;
    }

    let cur = right_pane_selected
        .read()
        .min(result_count.saturating_sub(1));

    match key {
        Key::ArrowDown => {
            if result_count > 0 {
                right_pane_selected.set((cur + 1).min(result_count - 1));
            }
        }
        Key::ArrowUp => {
            right_pane_selected.set(cur.saturating_sub(1));
        }
        Key::PageDown => {
            if result_count > 0 {
                right_pane_selected.set((cur + 10).min(result_count - 1));
            }
        }
        Key::PageUp => {
            right_pane_selected.set(cur.saturating_sub(10));
        }
        Key::Home => {
            right_pane_selected.set(0);
        }
        Key::End => {
            if result_count > 0 {
                right_pane_selected.set(result_count - 1);
            }
        }
        Key::Enter => {
            activate_result(ctx, cur);
        }
        _ => {}
    }
}

/// Open the file referenced by the selected result and move focus back to the
/// editor pane so the user can keep typing.
fn activate_result(ctx: AppCtx, result_idx: usize) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut focus = ctx.focus;
        let mut minibuf_msg = ctx.minibuf_msg;
        let editor_el = ctx.editor_el;

        let (lines, root) = match ctx.right_pane.read().as_ref() {
            Some(p) => (p.output.snapshot(), p.cwd.clone()),
            None => return,
        };
        let entry = lines
            .iter()
            .enumerate()
            .filter_map(|(i, l)| commands::parse_location(l, &root).map(|(p, ln, c)| (i, p, ln, c)))
            .nth(result_idx);
        let Some((_, full, line_no, col)) = entry else {
            return;
        };
        // parse_location returns 1-indexed line/col; the buffer wants 0-indexed.
        let line0 = line_no.saturating_sub(1);
        let col0 = col.saturating_sub(1);
        // Remember the spot we came from so the back mouse button / C-x ←
        // can return here. Pushed before the open so an open failure can
        // roll it back.
        crate::app::push_nav_origin(&ctx);
        match crate::app::open_file_at(ctx.buffers, ctx.current, full, line0, col0) {
            Ok(()) => {
                focus.set(Pane::Left);
                if let Some(ref el) = *editor_el.read() {
                    let _ = el.set_focus(true);
                }
                let mut tick = ctx.scroll_to_cursor_tick;
                let next = tick.peek().wrapping_add(1);
                tick.set(next);
            }
            Err(e) => {
                let mut hist = ctx.nav_history;
                hist.write().pop();
                minibuf_msg.set(format!("open failed: {e}"));
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (ctx, result_idx);
    }
}

pub fn filter_paths(paths: &[PathBuf], query: &str) -> Vec<(usize, String)> {
    let keys: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    files::fuzzy_filter_str(&keys, query)
}

/// Path-mode listing for the file picker. `base_dir` is the directory we
/// read, `suffix` is the typed text after the final `/`, `matches` are the
/// dir entries whose name starts with `suffix` (case-insensitive). Hidden
/// in `overlay.rs` because both rendering and the Enter/Tab key handler
/// need the exact same view.
#[cfg(not(target_arch = "wasm32"))]
pub struct PathPickerView {
    pub base_dir: std::path::PathBuf,
    pub suffix: String,
    pub matches: Vec<(String, bool)>,
}

/// Compute the path-mode view for the picker. Active project root is read
/// out of `ctx` to resolve relative queries.
#[cfg(not(target_arch = "wasm32"))]
pub fn compute_path_picker_view(ctx: &AppCtx, query: &str) -> PathPickerView {
    let cwd_fallback = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let active = ctx.project_root.read().clone().unwrap_or(cwd_fallback);
    let expanded = files::expand_path_query(query, &active);
    let (base, suffix) = files::split_path_query(&expanded);
    let base_dir = std::path::PathBuf::from(&base);
    let entries = files::list_dir(&base_dir);
    let suffix_lower = suffix.to_lowercase();
    // Hide dotfiles by default. The user opts back in by typing a leading
    // `.` in the suffix — same convention as a shell `ls`/glob.
    let include_hidden = suffix.starts_with('.');
    let matches: Vec<(String, bool)> = entries
        .into_iter()
        .filter(|(name, _)| {
            if !include_hidden && name.starts_with('.') {
                return false;
            }
            name.to_lowercase().starts_with(&suffix_lower)
        })
        .collect();
    PathPickerView {
        base_dir,
        suffix,
        matches,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn render_path_picker(
    ctx: &AppCtx,
    query: &str,
    cursor: usize,
    selected: usize,
    mut query_el: Signal<Option<MountedEvent>>,
) -> Element {
    let cursor = clamp_cursor_to_char_boundary(query, cursor);
    let before = &query[..cursor];
    let after = &query[cursor..];
    let view = compute_path_picker_view(ctx, query);
    let is_empty = view.matches.is_empty();
    let selected = selected.min(view.matches.len().saturating_sub(1));
    let start = selected.saturating_sub(MAX_RESULTS.saturating_sub(1));
    let visible: Vec<(usize, (String, bool))> = view
        .matches
        .into_iter()
        .enumerate()
        .skip(start)
        .take(MAX_RESULTS)
        .collect();

    rsx! {
        div { class: "overlay",
            div { class: "overlay-box",
                div { class: "overlay-prompt",
                    span { class: "overlay-label", "Find file: " }
                    span {
                        class: "overlay-query-window",
                        onmounted: move |evt: MountedEvent| {
                            query_el.set(Some(evt));
                        },
                        span { class: "overlay-query", "{before}" }
                        span { class: "overlay-caret", " " }
                        span { class: "overlay-query", "{after}" }
                    }
                }
                div { class: "overlay-list",
                    for (orig_i, (name, is_dir)) in visible.into_iter() {
                        {
                            let cls = if orig_i == selected { "overlay-item selected" } else { "overlay-item" };
                            let suffix = if is_dir { "/" } else { "" };
                            rsx! { div { class: "{cls}", "{name}{suffix}" } }
                        }
                    }
                    if is_empty {
                        div { class: "overlay-empty", "(no entries)" }
                    }
                }
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn render_path_picker(
    _ctx: &AppCtx,
    query: &str,
    cursor: usize,
    _selected: usize,
    mut query_el: Signal<Option<MountedEvent>>,
) -> Element {
    let cursor = clamp_cursor_to_char_boundary(query, cursor);
    let before = &query[..cursor];
    let after = &query[cursor..];
    rsx! {
        div { class: "overlay",
            div { class: "overlay-box",
                div { class: "overlay-prompt",
                    span { class: "overlay-label", "Find file: " }
                    span {
                        class: "overlay-query-window",
                        onmounted: move |evt: MountedEvent| {
                            query_el.set(Some(evt));
                        },
                        span { class: "overlay-query", "{before}" }
                        span { class: "overlay-caret", " " }
                        span { class: "overlay-query", "{after}" }
                    }
                }
                div { class: "overlay-list",
                    div { class: "overlay-empty", "(path mode not available on wasm)" }
                }
            }
        }
    }
}

fn clamp_cursor_to_char_boundary(s: &str, cursor: usize) -> usize {
    let mut cursor = cursor.min(s.len());
    while cursor > 0 && !s.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

pub fn filter_strings(items: &[String], query: &str) -> Vec<(usize, String)> {
    files::fuzzy_filter_str(items, query)
}

/// Labels for the buffer switcher. Unique basenames stay compact (`lib.rs`),
/// but duplicate basenames include two parent components so projects are easy
/// to tell apart (`dios/src/lib.rs`, `macroquad/src/lib.rs`).
pub fn buffer_switcher_labels(buffers: &[Buffer]) -> Vec<String> {
    let mut counts = std::collections::HashMap::<&str, usize>::new();
    for buf in buffers {
        *counts.entry(buf.name.as_str()).or_default() += 1;
    }

    buffers
        .iter()
        .map(|buf| {
            if counts.get(buf.name.as_str()).copied().unwrap_or(0) > 1 {
                buffer_tail_label(buf)
            } else {
                buf.name.clone()
            }
        })
        .collect()
}

fn buffer_tail_label(buf: &Buffer) -> String {
    let Some(path) = buf.path.as_ref() else {
        return buf.name.clone();
    };
    let mut parts: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        return buf.name.clone();
    }
    let keep = parts.len().saturating_sub(3);
    parts.drain(0..keep);
    parts.join("/")
}
