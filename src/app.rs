//! Top-level App component. Owns the buffer list, overlay state, and command
//! panes, and exposes them via context to child components.

use crate::buffer::Buffer;
use crate::commands::CommandOutput;
use crate::editor::CodeEditor;
use crate::lsp::LspManager;
use crate::overlay::{OverlayView, RightPaneView};
use dioxus::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;

/// Which pane currently receives keyboard input.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Left,
    Right,
}

/// Shared application state. All fields are `Copy` signals so they can be
/// consumed freely from any descendant via `use_context`.
#[derive(Clone, Copy)]
pub struct AppCtx {
    pub buffers: Signal<Vec<Buffer>>,
    pub current: Signal<usize>,
    pub overlay: Signal<Option<Overlay>>,
    pub right_pane: Signal<Option<RightPaneState>>,
    /// Selected result row in the right pane, indexed into the parsed-location
    /// list (not raw line index). Resets to 0 when a new pane is opened.
    pub right_pane_selected: Signal<usize>,
    /// Which pane has keyboard focus. Switched with C-k o.
    pub focus: Signal<Pane>,
    /// Mounted handles used to programmatically move keyboard focus when the
    /// user switches panes via C-k o.
    pub editor_el: Signal<Option<MountedEvent>>,
    pub right_pane_el: Signal<Option<MountedEvent>>,
    /// True while waiting for the second keystroke of a `C-k` prefix
    /// (Emacs-style two-stroke command, e.g. `C-k C-f`).
    pub ck_prefix: Signal<bool>,
    /// Transient message shown in the minibuffer (status / errors).
    pub minibuf_msg: Signal<String>,
    /// Project root used for file discovery and command execution.
    /// `None` means no project is currently loaded — search/compile are
    /// disabled and the file picker can only navigate via foreign-tree
    /// path mode (`./`, `../`, `~/`, `/`). Explicit buffer switches update
    /// this to the newly focused buffer's project when one can be inferred.
    pub project_root: Signal<Option<PathBuf>>,
    /// Cached list of files under `project_root` for the file picker.
    /// Empty whenever `project_root` is `None`.
    pub file_index: Signal<Vec<PathBuf>>,
    /// Active incremental-search session. None when not searching.
    pub isearch: Signal<Option<ISearch>>,
    /// Non-persisted incremental-search history, oldest to newest.
    pub isearch_history: Signal<Vec<String>>,
    /// Bumped every time miniquad fires a window-resize event. Components
    /// that care about the editor body's pixel size (e.g. `CodeEditor` for
    /// viewport virtualization) subscribe to this and re-measure when it
    /// changes.
    pub resize_tick: Signal<u64>,
    /// Bumped by anything that moves the cursor from outside the editor
    /// component (jumping to an rg/error hit, opening a file at a line).
    /// `CodeEditor` watches this and scrolls the cursor into view.
    pub scroll_to_cursor_tick: Signal<u64>,
    /// LSP / rust-analyzer manager. One handle, shared by clone; the manager
    /// itself wraps `Arc<Mutex<…>>` internally.
    pub lsp: Signal<LspManager>,
    /// Bumped by the LSP poll task whenever the manager's internal tick
    /// advances (status change, progress update, UI action queued). Components
    /// that depend on LSP state subscribe to this.
    pub lsp_tick: Signal<u64>,
    /// Stack of locations the user jumped *from* (LSP goto-definition,
    /// rg/compile result activation). Popping returns to the most recent
    /// origin, mimicking the back button in a web browser.
    pub nav_history: Signal<Vec<NavLocation>>,
    /// Current vertical scroll offset of the editor body, in pixels.
    /// Lives on `AppCtx` (instead of locally inside `CodeEditor`) so that
    /// non-editor callers — `push_nav_origin`, the LSP poll task, the right
    /// pane — can snapshot and restore it across jumps.
    pub scroll_top: Signal<f64>,
    /// Bumped when something outside the editor wants the editor body
    /// scrolled to the exact value currently in `scroll_top` (instead of
    /// merely "scrolled enough to make the cursor visible"). Used by
    /// `nav_back` to restore the user's viewport on return.
    pub restore_scroll_tick: Signal<u64>,
    /// Active LSP completion popup. `Some` while a `C-Space` popup is on
    /// screen — keystrokes are routed to it for navigation / accept /
    /// dismiss before normal editor handling.
    pub completion: Signal<Option<CompletionState>>,
    /// Per-project remembered compile command (the one the user last ran
    /// via the C-k c prompt). Keyed by active project root; not persisted
    /// across sessions. Projects without an entry default to `cargo check`.
    pub compile_commands: Signal<HashMap<PathBuf, String>>,
    /// Active compile-command prompt rendered in the minibuffer. Modeled
    /// after `isearch` rather than as an `Overlay` variant because the UI
    /// lives inline in the minibuffer (no centered overlay box), and a
    /// dedicated signal makes the Minibuffer's subscription unambiguous.
    pub compile_prompt: Signal<Option<CompilePromptState>>,
    /// Active goto-line prompt rendered in the minibuffer.
    pub goto_line_prompt: Signal<Option<GotoLinePromptState>>,
}

/// State of an active compile-command prompt in the minibuffer.
#[derive(Clone)]
pub struct CompilePromptState {
    pub query: String,
    /// Byte offset within `query` where the caret sits.
    pub cursor: usize,
}

/// State of an active goto-line prompt in the minibuffer.
#[derive(Clone)]
pub struct GotoLinePromptState {
    pub query: String,
    /// Byte offset within `query` where the caret sits.
    pub cursor: usize,
}

/// State of an open completion popup, anchored at the buffer offset where
/// `C-Space` was triggered. `request_id` matches the LSP request so a
/// late-arriving response for a popup the user already dismissed is dropped.
#[derive(Clone, Debug)]
pub struct CompletionState {
    pub request_id: u64,
    /// Buffer this popup was triggered for; closes if the user switches.
    pub buffer_idx: usize,
    /// Byte offset where the inserted completion should start — typically
    /// the start of the identifier the user is typing. The end of the
    /// replace range is the buffer's current cursor at accept time.
    pub replace_start: usize,
    /// Buffer line of `replace_start`, used for popup positioning so the
    /// box stays anchored where the trigger word starts.
    pub anchor_line: usize,
    /// Buffer column (Unicode scalar count) of `replace_start`.
    pub anchor_col: usize,
    pub items: Vec<crate::lsp::CompletionItem>,
    pub selected: usize,
    /// True between the LSP request and its response — the popup shows a
    /// "Loading…" placeholder.
    pub loading: bool,
}

/// A saved cursor position, used by [`AppCtx::nav_history`].
///
/// Keyed by `path` rather than buffer index because indices shift when
/// the user kills buffers between the push and the pop. `offset` is a
/// byte offset; we restore by line/col so a small edit between the push
/// and pop doesn't strand the cursor mid-grapheme.
#[derive(Clone, Debug)]
pub struct NavLocation {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    /// Editor body's scroll offset (in pixels) at the moment of the push.
    /// Restored on `nav_back` so the viewport returns to its original
    /// framing — restoring just `(line, col)` would re-center the cursor
    /// near the top of the screen, which feels wrong when the user was
    /// looking at it mid-page.
    pub scroll_top: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SearchDir {
    Forward,
    Backward,
}

/// Active emacs-style incremental search. While present, all editor keys are
/// captured by the isearch handler — typing extends the query, C-f / C-r step
/// through results, Esc/C-g restores the original cursor, Enter accepts.
#[derive(Clone)]
pub struct ISearch {
    pub query: String,
    pub direction: SearchDir,
    /// Buffer index this search is operating on. If the user switches buffers
    /// the search becomes stale; we just cancel on buffer switch.
    pub buffer: usize,
    /// Cursor offset and selection anchor when search started — restored on
    /// Esc / C-g.
    pub origin: usize,
    pub origin_anchor: Option<usize>,
    /// All match start offsets in buffer order, recomputed when the query
    /// changes.
    pub matches: Vec<usize>,
    /// Index into `matches` of the currently focused result. None when there
    /// are no matches at all.
    pub current: Option<usize>,
    /// Index into `AppCtx::isearch_history` while browsing history with
    /// Up/Down. None means the user is editing a fresh empty query.
    pub history_index: Option<usize>,
}

#[derive(Clone)]
pub enum Overlay {
    FilePicker {
        query: String,
        cursor: usize,
        selected: usize,
    },
    BufferSwitcher {
        query: String,
        selected: usize,
    },
    RgPrompt {
        query: String,
        cursor: usize,
    },
    UndoTree {
        selected: usize,
        origin: usize,
    },
}

#[derive(Clone)]
pub struct RightPaneState {
    pub title: String,
    pub output: CommandOutput,
    /// Working directory the underlying command ran in. Stored so that
    /// `commands::parse_location` can resolve relative paths in output even
    /// after the user switches to a buffer in a different project.
    pub cwd: PathBuf,
}

#[component]
pub fn App() -> Element {
    puffin::profile_function!();
    // CLI: first positional arg, if any, is treated as a project. A directory
    // becomes the active project directly; a regular file is opened and we
    // pick the project from its containing crate / parent. Without an arg
    // we start with no project — the user has to open a file (via path
    // picker) to load one.
    #[cfg(not(target_arch = "wasm32"))]
    let (initial_project, initial_buffer) = parse_cli_startup();
    #[cfg(target_arch = "wasm32")]
    let (initial_project, initial_buffer): (Option<PathBuf>, Option<Buffer>) = (None, None);

    let initial_file_index = match initial_project.as_ref() {
        Some(root) => crate::files::scan_project(root),
        None => Vec::new(),
    };

    let ctx = AppCtx {
        buffers: use_signal(|| {
            initial_buffer
                .map(|b| vec![b])
                .unwrap_or_else(|| vec![Buffer::new("")])
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
        project_root: use_signal(|| initial_project.clone()),
        file_index: use_signal(|| initial_file_index),
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
        compile_commands: use_signal(HashMap::new),
        compile_prompt: use_signal(|| None),
        goto_line_prompt: use_signal(|| None),
    };
    use_context_provider(|| ctx);

    // Mirror the watch channel that main.rs feeds from miniquad's resize
    // callback into a Dioxus signal so components can `use_effect` on it.
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

    // LSP poll task: workers mutate `LspManager` behind a Mutex, which
    // Dioxus can't observe. A runtime task polls that manager at 120 ms, but
    // only wakes the UI task when the manager's visible tick changes. This is
    // important with miniquad's blocking event loop: a Dioxus `sleep` inside
    // the UI task would wake the window every 120 ms even when nothing changed,
    // forcing ~8 redraws/sec while idle.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut lsp_tick = ctx.lsp_tick;
        let lsp = ctx.lsp;
        let mut buffers = ctx.buffers;
        let mut current = ctx.current;
        let mut minibuf_msg = ctx.minibuf_msg;
        let mut scroll_to_cursor_tick = ctx.scroll_to_cursor_tick;
        let nav_ctx = ctx;
        use_effect(move || {
            let mgr = lsp.peek().clone();
            let initial_tick = mgr.tick();
            let (tick_tx, mut tick_rx) = tokio::sync::watch::channel(initial_tick);

            let mgr_for_poll = mgr.clone();
            tokio::spawn(async move {
                let mut last_tick = initial_tick;
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                    let tick = mgr_for_poll.tick();
                    if tick == last_tick {
                        continue;
                    }
                    last_tick = tick;
                    if tick_tx.send(tick).is_err() {
                        break;
                    }
                }
            });

            spawn(async move {
                loop {
                    let mgr_tick = *tick_rx.borrow_and_update();
                    if mgr_tick != *lsp_tick.peek() {
                        lsp_tick.set(mgr_tick);
                    }
                    // Drain queued UI actions on the main thread.
                    for action in mgr.drain_ui_actions() {
                        match action {
                            crate::lsp::UiAction::JumpTo { path, line, col } => {
                                // Remember the spot we came from so the back
                                // mouse button / C-k ← can return here.
                                crate::app::push_nav_origin(&nav_ctx);
                                if crate::app::open_file_at(buffers, current, path, line, col)
                                    .is_ok()
                                {
                                    let next = scroll_to_cursor_tick.peek().wrapping_add(1);
                                    scroll_to_cursor_tick.set(next);
                                    // Clear the "jumping to definition…" hint
                                    // we set when the request was issued.
                                    minibuf_msg.set(String::new());
                                } else {
                                    // Open failed — the push we just did would
                                    // leak; drop it so back doesn't return us
                                    // to nowhere.
                                    let mut hist = nav_ctx.nav_history;
                                    hist.write().pop();
                                }
                            }
                            crate::lsp::UiAction::Message(m) => {
                                minibuf_msg.set(m);
                            }
                            crate::lsp::UiAction::Completion { request_id, items } => {
                                // Drop stale responses: the user may have
                                // dismissed the popup or fired a fresh
                                // request before this one arrived.
                                let mut completion = nav_ctx.completion;
                                let current = completion.peek().clone();
                                if let Some(mut state) = current {
                                    if state.request_id == request_id {
                                        state.loading = false;
                                        state.items = items;
                                        state.selected = 0;
                                        completion.set(Some(state));
                                    }
                                }
                            }
                        }
                    }
                    let _ = &mut buffers;
                    let _ = &mut current;

                    if tick_rx.changed().await.is_err() {
                        break;
                    }
                }
            });
        });
    }

    // Document sync: emit didOpen (first time) and didChange (subsequent)
    // for the current Rust buffer. Reads `buffers` so it re-runs on edits;
    // reads `lsp_tick` so it gets a chance to send a deferred didOpen
    // once the session transitions past Starting.
    //
    // The effect fires far more often than the buffer actually changes —
    // many times per second during rust-analyzer indexing, plus once per
    // mouse drag step / cursor move (Buffer mutators all go through
    // `buffers.write()` which triggers the signal). So we early-out via
    // a cheap version check before walking the rope to build a String.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let buffers_sig = ctx.buffers;
        let current_sig = ctx.current;
        let lsp = ctx.lsp;
        let lsp_tick_sig = ctx.lsp_tick;
        use_effect(move || {
            puffin::profile_scope!("lsp: doc-sync effect");
            let _ = *lsp_tick_sig.read();
            let idx = *current_sig.read();
            let bufs = buffers_sig.read();
            let Some(buf) = bufs.get(idx) else { return };
            let Some(path) = buf.path.clone() else { return };
            let Some(root) = buf.project_root.clone() else {
                return;
            };
            if path.extension().and_then(|s| s.to_str()) != Some("rs") {
                return;
            }
            let buf_version = buf.version;
            drop(bufs);

            let mgr = lsp.peek().clone();
            mgr.ensure_session(&root);
            // Skip the rope walk + LSP send when we've already pushed this
            // exact version — by far the common case during indexing.
            if !mgr.needs_sync(&root, &path, buf_version) {
                return;
            }
            match mgr.status_for(&root) {
                crate::lsp::LspStatus::Ready | crate::lsp::LspStatus::Indexing { .. } => {
                    // Only now do we materialise the rope.
                    let text = {
                        puffin::profile_scope!("lsp: rope->string for didChange");
                        let bufs = buffers_sig.peek();
                        bufs.get(idx).map(|b| b.text())
                    };
                    let Some(text) = text else { return };
                    mgr.did_open(&root, &path, buf_version, &text);
                    mgr.did_change(&root, &path, buf_version, &text);
                }
                _ => {}
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
            Minibuffer {}
            if has_overlay {
                OverlayView {}
            }
        }
    }
}

/// The active project root: target of compile / ripgrep / file-picker.
///
/// Explicit file opens and buffer switches update it to the focused buffer's
/// project when one can be inferred. Location jumps (LSP / rg / compile
/// results) intentionally do not force a project switch unless their caller
/// asks for one.
///
/// Returns `None` when no project has been loaded yet (the user launched
/// `dios` with no arg and hasn't opened a file).
///
/// Per-buffer crate roots live on `Buffer::project_root` and are used by
/// the LSP layer to key sessions, independent of this value.
pub fn active_project_root(ctx: &AppCtx) -> Option<PathBuf> {
    ctx.project_root.read().clone()
}

/// Set the active project to `new_root` and refresh the cached file index.
/// No-op if `new_root` already equals the current active project.
#[cfg(not(target_arch = "wasm32"))]
pub fn switch_active_project(ctx: &AppCtx, new_root: PathBuf) {
    if ctx.project_root.read().as_ref() == Some(&new_root) {
        return;
    }
    let mut project_root = ctx.project_root;
    let mut file_index = ctx.file_index;
    file_index.set(crate::files::scan_project(&new_root));
    project_root.set(Some(new_root));
}

/// Update the active project to match buffer `idx`, if that buffer has a
/// backing file/project. For files without a `.git` / `.projectile` ancestor,
/// fall back to the file's parent directory. Do not keep the previous active
/// root just because it happens to contain the file: that can leave the
/// project pill/search/compile target stuck on an ancestor or on another
/// buffer's project after an explicit buffer switch.
#[cfg(not(target_arch = "wasm32"))]
pub fn switch_active_project_to_buffer(ctx: &AppCtx, idx: usize) {
    let (crate_root, path) = {
        let bufs = ctx.buffers.read();
        let Some(buf) = bufs.get(idx) else { return };
        (buf.project_root.clone(), buf.path.clone())
    };

    if let Some(root) = crate_root {
        switch_active_project(ctx, root);
        return;
    }

    let Some(path) = path else { return };
    if let Some(parent) = path.parent() {
        switch_active_project(ctx, parent.to_path_buf());
    }
}

/// Snapshot the current buffer's cursor as a [`NavLocation`] and push it
/// onto [`AppCtx::nav_history`]. Call this immediately *before* moving the
/// cursor as part of a jump (LSP goto-definition, rg / compile result
/// activation) so [`nav_back`] can return the user to the original spot.
///
/// No-op when the current buffer has no backing path (scratch buffers
/// can't be re-opened by path, so there's nowhere meaningful to jump back
/// to).
#[cfg(not(target_arch = "wasm32"))]
pub fn push_nav_origin(ctx: &AppCtx) {
    let bufs = ctx.buffers.read();
    let idx = *ctx.current.read();
    let Some(buf) = bufs.get(idx) else { return };
    let Some(path) = buf.path.clone() else { return };
    let line = buf.cursor_line();
    let col = buf.cursor_col();
    drop(bufs);
    let scroll_top = *ctx.scroll_top.read();
    let mut history = ctx.nav_history;
    history.write().push(NavLocation {
        path,
        line,
        col,
        scroll_top,
    });
}

/// Pop the most recent [`NavLocation`] off [`AppCtx::nav_history`] and
/// restore it. Bound to the back mouse button and to `C-k ←`.
#[cfg(not(target_arch = "wasm32"))]
pub fn nav_back(ctx: &AppCtx) {
    let mut history = ctx.nav_history;
    let entry = history.write().pop();
    let Some(NavLocation {
        path,
        line,
        col,
        scroll_top,
    }) = entry
    else {
        let mut minibuf_msg = ctx.minibuf_msg;
        minibuf_msg.set("nav: history empty".to_string());
        return;
    };
    match open_file_at(ctx.buffers, ctx.current, path, line, col) {
        Ok(()) => {
            // Restore the exact scroll offset rather than bumping
            // scroll_to_cursor_tick. The cursor-into-view path re-centers
            // the cursor near the top of the viewport, which loses the
            // visual context the user had when they jumped.
            let mut top = ctx.scroll_top;
            top.set(scroll_top);
            let mut tick = ctx.restore_scroll_tick;
            let next = tick.peek().wrapping_add(1);
            tick.set(next);
            let mut minibuf_msg = ctx.minibuf_msg;
            minibuf_msg.set(String::new());
        }
        Err(e) => {
            let mut minibuf_msg = ctx.minibuf_msg;
            minibuf_msg.set(format!("nav: back failed: {e}"));
        }
    }
}

/// Open a file in a buffer (reusing one if already open) and place the cursor
/// at the given line/column (both 0-indexed). Returns Ok(()) on success.
#[cfg(not(target_arch = "wasm32"))]
pub fn open_file_at(
    mut buffers: Signal<Vec<Buffer>>,
    mut current: Signal<usize>,
    full: PathBuf,
    line: usize,
    col: usize,
) -> std::io::Result<()> {
    let existing = buffers
        .read()
        .iter()
        .position(|b| b.path.as_ref() == Some(&full));
    let target_idx = match existing {
        Some(i) => i,
        None => {
            let buf = Buffer::from_path(full.clone())?;
            let mut bufs = buffers.write();
            if bufs.len() == 1 && bufs[0].len() == 0 && bufs[0].path.is_none() {
                bufs[0] = buf;
                0
            } else {
                bufs.push(buf);
                bufs.len() - 1
            }
        }
    };
    current.set(target_idx);
    buffers.write()[target_idx].move_to(line, col);
    Ok(())
}

#[component]
fn Minibuffer() -> Element {
    let ctx: AppCtx = use_context();
    let msg = ctx.minibuf_msg.read().clone();
    let ck = *ctx.ck_prefix.read();
    let isearch = ctx.isearch.read().clone();
    let compile_prompt = ctx.compile_prompt.read().clone();
    let goto_line_prompt = ctx.goto_line_prompt.read().clone();

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

    if let Some(CompilePromptState { query, cursor }) = compile_prompt {
        let cursor = cursor.min(query.len());
        let before = &query[..cursor];
        let after = &query[cursor..];
        return rsx! {
            div { id: "minibuffer",
                span { class: "mb-search-label", "Compile: " }
                span { class: "mb-search-query", "{before}" }
                span { class: "mb-search-caret", " " }
                span { class: "mb-search-query", "{after}" }
            }
        };
    }

    if let Some(GotoLinePromptState { query, cursor }) = goto_line_prompt {
        let cursor = cursor.min(query.len());
        let before = &query[..cursor];
        let after = &query[cursor..];
        return rsx! {
            div { id: "minibuffer",
                span { class: "mb-search-label", "Goto line: " }
                span { class: "mb-search-query", "{before}" }
                span { class: "mb-search-caret", " " }
                span { class: "mb-search-query", "{after}" }
            }
        };
    }

    let display = if ck {
        "C-k ".to_string()
    } else if !msg.is_empty() {
        msg
    } else {
        // Default modeline-like status line.
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

/// Read `argv[1]` and decide the startup project / buffer.
///
/// - directory  → that directory is the project, no buffer is auto-opened
/// - regular file → open the file and pick the project from its
///   `find_project_root` ancestor, or its parent dir if none has `.git`
/// - anything else (missing, broken path, etc.) → no project, scratch buffer
///
/// The CLI argument is taken verbatim — we do **not** require the folder to
/// contain `.git` or `.projectile`.
#[cfg(not(target_arch = "wasm32"))]
fn parse_cli_startup() -> (Option<PathBuf>, Option<Buffer>) {
    let Some(arg) = std::env::args().nth(1) else {
        return (None, None);
    };
    let path = std::path::PathBuf::from(&arg);
    let Ok(meta) = std::fs::metadata(&path) else {
        eprintln!("dios: cannot stat {arg}");
        return (None, None);
    };
    let canonical = std::fs::canonicalize(&path).unwrap_or(path);
    if meta.is_dir() {
        (Some(canonical), None)
    } else if meta.is_file() {
        match Buffer::from_path(canonical.clone()) {
            Ok(buf) => {
                let project = buf
                    .project_root
                    .clone()
                    .or_else(|| canonical.parent().map(|p| p.to_path_buf()));
                (project, Some(buf))
            }
            Err(e) => {
                eprintln!("dios: cannot open {arg}: {e}");
                (None, None)
            }
        }
    } else {
        (None, None)
    }
}
