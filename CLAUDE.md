# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run

Native:
- `cargo build` / `cargo check` — builds the `dios` binary.
- `cargo run` — launches the editor in a miniquad window (1200x700). `cargo run -- <path>` opens a folder as a project, or a file (whose crate ancestor becomes the project).
- `cargo run --example <name>` — see `[[example]]` entries in `Cargo.toml`. Most are Blitz layout reproducers; `basic` is a minimal Blitz+miniquad demo; `lsp_smoke` exercises the LSP transport in isolation.

Web (wasm32):
- `cargo build --target wasm32-unknown-unknown` then copy `target/wasm32-unknown-unknown/<profile>/dios.wasm` into `web/` and serve that directory (e.g. `basic-http-server .`). The HTML shell and miniquad's `gl.js` loader live in `web/` already.
- LSP, ripgrep, and compilation are unavailable on wasm — those features degrade to minibuffer messages.

No test suite in this crate.

## Profiling

This app is built on a still-young cross-platform UI stack; expect to need a profiler regularly. Scopes are wired through `puffin` + `puffin_http`. The editor always starts the puffin server on `127.0.0.1:8585`; connect with `puffin_viewer --url 127.0.0.1:8585`. Instrumented spots include miniquad callbacks, Blitz layout/paint, Dioxus reconciliation, the LSP poll task, and most editor components. When investigating a "UI feels slow" bug, start there.

## External path dependencies

Every external dependency lives under `./deps/` — no sibling-checkout requirements. The crate will not build without these:

- `./deps/blitz/packages/*` — vendored Blitz DOM/paint/shell stack, including a custom `blitz-shell-miniquad` and `anyrender_nonaquad`.
- `./deps/nonaquad/{nona,nonaquad}` — immediate-mode vector renderer used as Blitz's paint backend here.
- `./deps/miniquad` — patched miniquad (extra mouse buttons, etc.), wired in via `[patch.crates-io]`.
- `./deps/subsecond`, `./deps/web-time` — in-tree patches for Dioxus's hot-reload and web-time crates.

## Architecture

This is an Emacs-style text editor whose entire UI is a Dioxus component tree rendered through **Blitz** (a headless HTML/CSS engine) onto a **miniquad** GL surface. The notable thing is that there is no platform-native widget toolkit: everything — the gutter, cursor, selection rects, overlays, completion popup, syntax highlighting — is just styled HTML in `src/styles.css`.

### Render pipeline

`main.rs` wires it all up:
1. Build a `VirtualDom` from the `App` component.
2. Wrap it in `DioxusDocument` (from `deps/blitz/packages/dioxus-native-dom`), which maintains a Blitz DOM mirrored from Dioxus's vdom.
3. Inject `styles.css` as a user-agent stylesheet; register `assets/fonts/LiberationMono-Regular.ttf` into the Blitz `FontContext`.
4. Hand the document to `BlitzMiniquadApp` (from `blitz-shell-miniquad`), which drives layout/paint inside miniquad's event loop via `anyrender_nonaquad`.

On native, `main.rs` also spawns a tokio runtime — the editor relies on `tokio::time::sleep` for drag autoscroll, the LSP / commands poll loops, etc.

### State: `AppCtx` in `src/app.rs`

All app state is `Copy` `Signal`s on a single `AppCtx` struct placed into context. Any descendant reads/writes via `use_context`. Fields of note:

- `buffers: Signal<Vec<Buffer>>` + `current: Signal<usize>` — open buffers and focused index.
- `overlay: Signal<Option<Overlay>>` — file picker (fuzzy or path mode), buffer switcher, ripgrep prompt, undo-tree browser. Mutually exclusive modal.
- `right_pane: Signal<Option<RightPaneState>>` — compilation / ripgrep output pane.
- `focus: Signal<Pane>` — keyboard focus, toggled with `C-k o`.
- `ck_prefix: Signal<bool>` — true between the first `C-k` and the next key (Emacs-style two-stroke commands).
- `project_root: Signal<Option<PathBuf>>` — sticky active project. `None` until the user opens a file or passes a path on the CLI. Search/compile target this; buffer switches and result-jumps do not change it. Only the foreign-tree branch of the file picker switches it.
- `file_index`, `minibuf_msg`, `isearch`, `isearch_history`.
- `nav_history: Signal<Vec<NavLocation>>` — origins pushed before LSP/rg/compile jumps. Pop with the back mouse button or `C-k ←`. `NavLocation` stores `(path, line, col, scroll_top)` so the viewport framing is restored, not just the cursor.
- `scroll_top: Signal<f64>` + `restore_scroll_tick: Signal<u64>` — editor scroll offset lives here (not locally in `CodeEditor`) so `nav_back` and other out-of-component callers can snapshot/restore it. `restore_scroll_tick` is bumped by `nav_back` after writing `scroll_top`; an effect in `CodeEditor` mirrors the value to the body element.
- `scroll_to_cursor_tick: Signal<u64>` — bumped by anything that moves the cursor from outside `CodeEditor`; the editor scrolls the cursor into view (with margin) on change.
- `lsp: Signal<LspManager>` + `lsp_tick: Signal<u64>` — LSP session map and a tick the poll task bumps when the manager's internal state changes (status, queued UI actions).
- `completion: Signal<Option<CompletionState>>` — active C-Space completion popup. While `Some`, keystrokes are routed to nav / accept / dismiss before normal editor handling.

### Buffer: `src/buffer.rs`

Text stored in `lapce_xi_rope::Rope`. Cursor is a byte offset; selection is an optional anchor offset plus the cursor. Each `Buffer` carries `path`, `project_root` (nearest `.git`/`.projectile` ancestor — used to key the LSP session, independent of the active project), `dirty`, and a monotonic `version` bumped on every mutation. Undo/redo snapshot the entire rope + cursor + anchor. All motion methods come in bare and `_with_selection` pairs — Shift-arrow etc. call the latter, which extends the anchor.

### Editor view: `src/editor.rs` (the biggest module)

- **Viewport virtualization** is over **visual rows** (post soft-wrap), not source lines. `src/wrap.rs` builds a `WrapMap` keyed off character width and viewport width; the editor only emits DOM for visual rows in `[first_visible - overscan, last_visible + overscan]`. A hidden `#measure-line` with 100 `M`s is measured on mount to calibrate `char_width`/`line_height`; the gutter width is measured similarly. The cursor, selection, search and ctrl-hover rects are all positioned via arithmetic on these metrics + the wrap map.
- **Keyboard dispatch** (`onkeydown` on `#editor-panel`) is a stack of guards: (1) overlay open → `handle_overlay_key`; (2) isearch active → `isearch::handle_key`; (3) completion popup open → `completion_handle_key` (nav keys consume; other keys dismiss and fall through); (4) `ck_prefix` set → `handle_ck_command`, clear prefix; (5) bare `C-k` → set prefix and return; (6) `C-S-f` opens rg, `C-f`/`C-r` start isearch, `C-Space` triggers completion; (7) normal editing. Plain `C-c` / `C-x` / `C-v` are copy/cut/paste via the OS clipboard (`src/clipboard.rs`, miniquad-backed on native, no-op on wasm).
- **`C-k` commands**: `C-f` find file, `b` switch buffer, `f` rustfmt current file, `c` cargo check, `k` kill buffer, `r` revert from disk, `s` save, `o` switch pane focus, `1` close right pane, `←` `nav_back`.
- **Mouse**: ctrl-click does LSP goto-definition, ctrl-hover underlines the would-be target; the back button (X11 button 8 → `MouseButton::Fourth`) calls `nav_back`.
- **Drag autoscroll** uses a tokio `spawn` loop with `tokio::time::sleep(16ms)` — gated `#[cfg(not(target_arch = "wasm32"))]`.
- **`CompletionPopup`** is a small inline component rendered inside `#code-area` so it shares the cursor's scrolled coordinate space (popup stays anchored to the trigger word).

### LSP: `src/lsp.rs`

One `LspSession` per crate root (keyed by `Buffer::project_root`, not the active project). The manager spawns rust-analyzer as a child process with two worker threads (framed `Content-Length:` JSON-RPC over stdin/stdout). No `lsp-types` dependency — `serde_json::Value` plus hand-written `serde_json::json!{}` payloads cover the handful of messages we care about: `initialize` (negotiates utf-8/utf-16 position encoding and advertises completion capability with `snippetSupport: false`), `initialized`, `textDocument/didOpen|didChange|didClose`, `textDocument/definition`, `textDocument/completion`, `$/progress`, polite shutdown on `Drop`.

The manager exposes synchronous methods; results that need to land on the UI thread are pushed onto a `Vec<UiAction>` (Definition jump, transient minibuffer message, completion items). An `App`-level tokio task polls `LspManager::tick()` every 120 ms, mirrors it into `lsp_tick`, and drains `UiAction`s onto Dioxus signals. The completion poll matches each response against the active popup's `request_id` so stale responses (popup already dismissed, fresh request fired) are dropped.

### File picker: `src/overlay.rs` + `src/files.rs`

The file picker has two modes, decided per-keystroke from the query:
- **Fuzzy mode** (default) — fuzzy-match against the cached `file_index` of the active project. Dotfiles are filtered out unless the query begins with `.`.
- **Path mode** — triggered when the query starts with `/`, `~/`, `./`, or `../`. Lists one directory at a time; Tab completes (longest common prefix, or unique entry); Enter on a directory descends, on a file opens. Opening a file outside the active project tree switches the active project to the new file's crate root. Directory listings are cached in a thread-local single-slot to keep navigation under `$HOME` responsive.

### External commands: `src/commands.rs`

`run_ripgrep` and `run_compile` spawn a thread that `Command::spawn`s the child and pushes stdout/stderr lines into an `Arc<Mutex<Vec<String>>>`. Dioxus can't observe the mutex changing, so `RightPaneView` runs an 80ms tick loop (also tokio, native-only) that bumps a local signal to force re-render until the command reports done. The whole module is no-op on wasm.

### Cross-platform gating

Anything touching the filesystem (`files::scan_project`, `Buffer::from_path`), spawning processes (`commands::*`), or talking to a language server (`lsp::*`) is `#[cfg(not(target_arch = "wasm32"))]`. The wasm builds keep the public surface compiling (stub `LspManager`, no-op clipboard, etc.); features degrade to minibuffer messages. When adding features that touch OS resources, follow this pattern — don't break the wasm build.
