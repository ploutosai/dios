//! LSP / rust-analyzer integration.
//!
//! One `LspSession` per crate root. Each session owns the child process and
//! two worker threads (reader/writer over LSP `Content-Length:` framing).
//! Communication with Dioxus is shared state behind an `Arc<Mutex<…>>` plus
//! an `LspManager::tick()` counter the UI polls — same pattern as
//! `commands.rs`.
//!
//! No `lsp-types` dependency; we hand-roll the few messages we care about
//! (initialize, initialized, didOpen/Change/Close, definition, $/progress,
//! shutdown, exit) on top of `serde_json::Value`.
//!
//! Native-only. The wasm build keeps the public surface (`LspStatus`,
//! `LspManager`) compiling but always reports `Disabled`.

use std::path::{Path, PathBuf};

#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::io::{BufRead, BufReader, Write};
#[cfg(not(target_arch = "wasm32"))]
use std::process::{Child, Command, Stdio};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex};

/// Coarse rust-analyzer lifecycle as the user sees it.
#[derive(Debug, Clone)]
pub enum LspStatus {
    /// Wasm, or no Rust buffer seen yet.
    Disabled,
    /// Spawned, waiting for initialize result.
    Starting,
    /// Server reported a $/progress with `kind: report`.
    Indexing {
        pct: Option<u8>,
        message: String,
    },
    Ready,
    Error(String),
}

impl LspStatus {
    pub fn label(&self) -> String {
        match self {
            LspStatus::Disabled => "lsp: off".into(),
            LspStatus::Starting => "lsp: starting".into(),
            LspStatus::Indexing {
                pct: Some(p),
                message,
            } if !message.is_empty() => {
                format!("lsp: {p}% {message}")
            }
            LspStatus::Indexing { pct: Some(p), .. } => format!("lsp: {p}%"),
            LspStatus::Indexing { message, .. } if !message.is_empty() => {
                format!("lsp: {message}")
            }
            LspStatus::Indexing { .. } => "lsp: indexing".into(),
            LspStatus::Ready => "lsp: ready".into(),
            LspStatus::Error(s) => format!("lsp: error ({s})"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Wasm stub
// ─────────────────────────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
#[derive(Clone, Default)]
pub struct LspManager;

#[cfg(target_arch = "wasm32")]
impl LspManager {
    pub fn new() -> Self {
        Self
    }
    pub fn ensure_session(&self, _root: &Path) {}
    pub fn status_for(&self, _root: &Path) -> LspStatus {
        LspStatus::Disabled
    }
    pub fn needs_sync(&self, _root: &Path, _path: &Path, _buf_version: u64) -> bool {
        false
    }
    pub fn did_open(&self, _root: &Path, _path: &Path, _buf_version: u64, _text: &str) {}
    pub fn did_change(&self, _root: &Path, _path: &Path, _buf_version: u64, _text: &str) {}
    pub fn did_close(&self, _root: &Path, _path: &Path) {}
    pub fn goto_definition(&self, _root: &Path, _path: &Path, _line: u32, _utf8_col: u32) {}
    pub fn request_completion(
        &self,
        _root: &Path,
        _path: &Path,
        _line: u32,
        _utf8_col: u32,
    ) -> Option<u64> {
        None
    }
    pub fn drain_ui_actions(&self) -> Vec<UiAction> {
        Vec::new()
    }
    pub fn tick(&self) -> u64 {
        0
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Native implementation
// ─────────────────────────────────────────────────────────────────────────

/// A UI action emitted by background LSP work that the Dioxus poll task
/// drains and applies on the main thread.
#[derive(Debug, Clone)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub enum UiAction {
    JumpTo {
        path: PathBuf,
        /// 0-indexed.
        line: usize,
        /// 0-indexed, *characters* (Unicode scalar values), not bytes — matches
        /// the editor's `move_to(line, col)` contract.
        col: usize,
    },
    /// Transient minibuffer message — used when a definition lookup fails or
    /// the server isn't ready.
    Message(String),
    /// Result of a `textDocument/completion` request. `request_id` matches the
    /// id returned by [`LspManager::request_completion`] so the UI can drop
    /// stale responses (e.g. the user already dismissed the popup or fired a
    /// fresh request).
    Completion {
        request_id: u64,
        items: Vec<CompletionItem>,
    },
}

/// Subset of an LSP `CompletionItem` we surface to the UI. `insert_text` has
/// already had snippet tabstops stripped — the editor inserts it verbatim.
#[derive(Debug, Clone)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub struct CompletionItem {
    pub label: String,
    pub insert_text: String,
    pub detail: String,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Default)]
pub struct LspManager {
    inner: Arc<Mutex<ManagerInner>>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct ManagerInner {
    sessions: HashMap<PathBuf, Arc<Session>>,
    /// Bumped any time a session's state visibly changes, so the UI can
    /// re-render. Drained via `tick()`.
    tick: u64,
    /// Actions queued for the UI thread.
    ui_actions: Vec<UiAction>,
}

#[cfg(not(target_arch = "wasm32"))]
impl LspManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Boot a session for `root` if one doesn't exist yet. Idempotent.
    pub fn ensure_session(&self, root: &Path) {
        let key = root.to_path_buf();
        let mut g = self.inner.lock().unwrap();
        if g.sessions.contains_key(&key) {
            return;
        }
        let session = Session::spawn(key.clone(), self.inner.clone());
        g.sessions.insert(key, session);
        g.tick = g.tick.wrapping_add(1);
    }

    pub fn status_for(&self, root: &Path) -> LspStatus {
        let g = self.inner.lock().unwrap();
        match g.sessions.get(root) {
            Some(s) => s.status.lock().unwrap().clone(),
            None => LspStatus::Disabled,
        }
    }

    /// True iff calling `did_open`/`did_change` with this version would send
    /// anything. Lets the caller skip the rope-to-String walk on no-op runs.
    /// Returns `false` when no session exists for `root` yet.
    pub fn needs_sync(&self, root: &Path, path: &Path, buf_version: u64) -> bool {
        let session = {
            let g = self.inner.lock().unwrap();
            g.sessions.get(root).cloned()
        };
        match session {
            Some(s) => s.needs_sync(path, buf_version),
            None => false,
        }
    }

    pub fn did_open(&self, root: &Path, path: &Path, buf_version: u64, text: &str) {
        let session = {
            let g = self.inner.lock().unwrap();
            g.sessions.get(root).cloned()
        };
        if let Some(s) = session {
            s.did_open(path, buf_version, text);
        }
    }

    pub fn did_change(&self, root: &Path, path: &Path, buf_version: u64, text: &str) {
        let session = {
            let g = self.inner.lock().unwrap();
            g.sessions.get(root).cloned()
        };
        if let Some(s) = session {
            s.did_change(path, buf_version, text);
        }
    }

    pub fn did_close(&self, root: &Path, path: &Path) {
        let session = {
            let g = self.inner.lock().unwrap();
            g.sessions.get(root).cloned()
        };
        if let Some(s) = session {
            s.did_close(path);
        }
    }

    /// Issue a `textDocument/definition`. Response is asynchronous: when it
    /// arrives, the session pushes a `UiAction::JumpTo` into the manager's
    /// queue, which the poll task picks up.
    pub fn goto_definition(&self, root: &Path, path: &Path, line: u32, utf8_col: u32) {
        let session = {
            let g = self.inner.lock().unwrap();
            g.sessions.get(root).cloned()
        };
        if let Some(s) = session {
            s.goto_definition(path, line, utf8_col);
        } else {
            self.push_action(UiAction::Message("lsp: no session for this file".into()));
        }
    }

    /// Issue a `textDocument/completion` request. Returns the JSON-RPC id the
    /// request was sent with, or `None` when there's no session for `root` —
    /// the UI uses the id to match the asynchronous `UiAction::Completion`
    /// reply against the popup it triggered, dropping stale responses.
    pub fn request_completion(
        &self,
        root: &Path,
        path: &Path,
        line: u32,
        utf8_col: u32,
    ) -> Option<u64> {
        let session = {
            let g = self.inner.lock().unwrap();
            g.sessions.get(root).cloned()
        };
        let s = session?;
        Some(s.request_completion(path, line, utf8_col))
    }

    pub fn drain_ui_actions(&self) -> Vec<UiAction> {
        let mut g = self.inner.lock().unwrap();
        std::mem::take(&mut g.ui_actions)
    }

    pub fn tick(&self) -> u64 {
        self.inner.lock().unwrap().tick
    }

    fn push_action(&self, a: UiAction) {
        let mut g = self.inner.lock().unwrap();
        g.ui_actions.push(a);
        g.tick = g.tick.wrapping_add(1);
    }
}

// ─── Session ──────────────────────────────────────────────────────────────

/// One running rust-analyzer instance.
#[cfg(not(target_arch = "wasm32"))]
struct Session {
    status: Arc<Mutex<LspStatus>>,
    /// Outgoing message queue. The writer thread drains it.
    outbox: mpsc::Sender<Outgoing>,
    /// Server's chosen position encoding ("utf-8" if we got it, else
    /// "utf-16"). Set after `initialize` response. Defaults to utf-16
    /// (the LSP default) until then.
    position_encoding: Arc<Mutex<PositionEncoding>>,
    /// Buffer state: per-URI version number we last sent. Bump-and-send
    /// pattern keeps versions monotonic.
    docs: Arc<Mutex<HashMap<String, DocState>>>,
    /// Pending requests by id, mapped to a callback closure.
    pending: Arc<Mutex<HashMap<u64, PendingHandler>>>,
    /// Next request id.
    next_id: Arc<Mutex<u64>>,
    /// Server-reported progress tokens we're tracking, mapped to label.
    progress: Arc<Mutex<HashMap<String, ProgressEntry>>>,
    /// Held so the process is killed when the Session is dropped.
    #[allow(dead_code)]
    child: Arc<Mutex<Option<Child>>>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug)]
enum PositionEncoding {
    Utf8,
    Utf16,
}

#[cfg(not(target_arch = "wasm32"))]
struct DocState {
    /// Version number we last *sent* over the wire, used as the LSP
    /// document version. Monotonic.
    version: i32,
    /// The caller-supplied buffer version (`Buffer::version`) we last
    /// rendered into a `didChange`. Compared cheaply (u64 == u64) instead
    /// of string-comparing the whole rope contents on every effect run.
    last_buf_version: u64,
    /// Whether we've sent didOpen for this URI.
    opened: bool,
}

#[cfg(not(target_arch = "wasm32"))]
struct ProgressEntry {
    title: String,
    message: String,
    percentage: Option<u8>,
}

#[cfg(not(target_arch = "wasm32"))]
enum Outgoing {
    /// Raw JSON value to be written framed.
    Message(serde_json::Value),
    Shutdown,
}

#[cfg(not(target_arch = "wasm32"))]
type PendingHandler = Box<dyn FnOnce(&serde_json::Value, &LspManager) + Send>;

#[cfg(not(target_arch = "wasm32"))]
impl Session {
    fn spawn(root: PathBuf, mgr: Arc<Mutex<ManagerInner>>) -> Arc<Self> {
        let server = std::env::var("RUST_ANALYZER").unwrap_or_else(|_| "rust-analyzer".to_string());
        let mut cmd = Command::new(&server);
        cmd.current_dir(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let status = Arc::new(Mutex::new(LspStatus::Error(format!(
                    "failed to spawn {server}: {e}"
                ))));
                let (tx, _rx) = mpsc::channel();
                return Arc::new(Self {
                    status,
                    outbox: tx,
                    position_encoding: Arc::new(Mutex::new(PositionEncoding::Utf16)),
                    docs: Arc::new(Mutex::new(HashMap::new())),
                    pending: Arc::new(Mutex::new(HashMap::new())),
                    next_id: Arc::new(Mutex::new(1)),
                    progress: Arc::new(Mutex::new(HashMap::new())),
                    child: Arc::new(Mutex::new(None)),
                });
            }
        };

        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        let stderr = child.stderr.take().expect("child stderr");

        let status = Arc::new(Mutex::new(LspStatus::Starting));
        let position_encoding = Arc::new(Mutex::new(PositionEncoding::Utf16));
        let docs = Arc::new(Mutex::new(HashMap::new()));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let next_id = Arc::new(Mutex::new(1u64));
        let progress = Arc::new(Mutex::new(HashMap::new()));
        let child_arc = Arc::new(Mutex::new(Some(child)));

        let (tx, rx) = mpsc::channel::<Outgoing>();

        // Writer thread.
        {
            let mut stdin = stdin;
            std::thread::spawn(move || {
                while let Ok(msg) = rx.recv() {
                    match msg {
                        Outgoing::Message(v) => {
                            if write_message(&mut stdin, &v).is_err() {
                                break;
                            }
                        }
                        Outgoing::Shutdown => break,
                    }
                }
            });
        }

        // Stderr drain. Pass-through to our stderr so r-a's diagnostics are
        // visible (the user expects to see why the server crashed). Gate the
        // forwarding behind a quiet flag if it gets noisy.
        let quiet = std::env::var("LSP_QUIET").is_ok();
        std::thread::spawn(move || {
            let mut r = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match r.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if !quiet {
                            eprint!("rust-analyzer: {}", line);
                        }
                    }
                }
            }
        });

        let session = Arc::new(Self {
            status: status.clone(),
            outbox: tx,
            position_encoding: position_encoding.clone(),
            docs,
            pending: pending.clone(),
            next_id,
            progress: progress.clone(),
            child: child_arc,
        });

        // Reader thread.
        {
            let session = session.clone();
            let mgr_outer = mgr.clone();
            std::thread::spawn(move || {
                let mut stdout = BufReader::new(stdout);
                loop {
                    match read_message(&mut stdout) {
                        Ok(Some(msg)) => {
                            session.handle_incoming(msg, &mgr_outer);
                        }
                        Ok(None) => break,
                        Err(e) => {
                            let mut st = session.status.lock().unwrap();
                            *st = LspStatus::Error(format!("read error: {e}"));
                            bump_tick(&mgr_outer);
                            break;
                        }
                    }
                }
            });
        }

        // Send `initialize`.
        let init_id = session.fresh_id();
        let init_params = serde_json::json!({
            "processId": std::process::id(),
            "clientInfo": { "name": "dios", "version": "0.1" },
            "rootUri": path_to_uri(&root),
            "capabilities": {
                "general": {
                    "positionEncodings": ["utf-8", "utf-16"],
                },
                "textDocument": {
                    "synchronization": {
                        "dynamicRegistration": false,
                        "willSave": false,
                        "didSave": false,
                    },
                    "definition": {
                        "linkSupport": true,
                    },
                    "completion": {
                        "dynamicRegistration": false,
                        "contextSupport": true,
                        "completionItem": {
                            // Plain-text only: keeps the response simple and
                            // lets us insert `insertText` verbatim without
                            // implementing snippet placeholders.
                            "snippetSupport": false,
                            "commitCharactersSupport": false,
                        },
                    },
                },
                "window": {
                    "workDoneProgress": true,
                },
            },
            "workspaceFolders": [
                {
                    "uri": path_to_uri(&root),
                    "name": root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                }
            ],
        });

        {
            let session_inner = session.clone();
            let mgr_for_handler = mgr.clone();
            session.set_pending(
                init_id,
                Box::new(move |result, _mgr| {
                    let encoding = result
                        .get("capabilities")
                        .and_then(|c| c.get("positionEncoding"))
                        .and_then(|v| v.as_str())
                        .map(|s| {
                            if s == "utf-8" {
                                PositionEncoding::Utf8
                            } else {
                                PositionEncoding::Utf16
                            }
                        })
                        .unwrap_or(PositionEncoding::Utf16);
                    *session_inner.position_encoding.lock().unwrap() = encoding;

                    // Tell the server we're ready.
                    session_inner.send_notification("initialized", serde_json::json!({}));

                    let mut st = session_inner.status.lock().unwrap();
                    *st = LspStatus::Ready;
                    drop(st);
                    bump_tick(&mgr_for_handler);
                }),
            );
        }

        session.send_request(init_id, "initialize", init_params);

        session
    }

    fn fresh_id(&self) -> u64 {
        let mut g = self.next_id.lock().unwrap();
        let id = *g;
        *g = g.wrapping_add(1);
        id
    }

    fn set_pending(&self, id: u64, handler: PendingHandler) {
        self.pending.lock().unwrap().insert(id, handler);
    }

    fn send_request(&self, id: u64, method: &str, params: serde_json::Value) {
        puffin::profile_scope!("lsp: send_request", method);
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let _ = self.outbox.send(Outgoing::Message(msg));
    }

    fn send_notification(&self, method: &str, params: serde_json::Value) {
        puffin::profile_scope!("lsp: send_notification", method);
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let _ = self.outbox.send(Outgoing::Message(msg));
    }

    /// True iff calling `did_open`/`did_change` for this `(path, buf_version)`
    /// would actually send something. Callers use this to skip generating
    /// the buffer's text (a rope-to-String walk) on no-op runs.
    fn needs_sync(&self, path: &Path, buf_version: u64) -> bool {
        let uri = path_to_uri(path);
        let docs = self.docs.lock().unwrap();
        match docs.get(&uri) {
            None => true,
            Some(e) if !e.opened => true,
            Some(e) => e.last_buf_version != buf_version,
        }
    }

    fn did_open(&self, path: &Path, buf_version: u64, text: &str) {
        let uri = path_to_uri(path);
        let mut docs = self.docs.lock().unwrap();
        let entry = docs.entry(uri.clone()).or_insert(DocState {
            version: 0,
            last_buf_version: 0,
            opened: false,
        });
        if entry.opened {
            return;
        }
        entry.opened = true;
        entry.version = 1;
        entry.last_buf_version = buf_version;
        drop(docs);

        self.send_notification(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "rust",
                    "version": 1,
                    "text": text,
                }
            }),
        );
    }

    fn did_change(&self, path: &Path, buf_version: u64, text: &str) {
        let uri = path_to_uri(path);
        let mut docs = self.docs.lock().unwrap();
        let entry = match docs.get_mut(&uri) {
            Some(e) if e.opened => e,
            _ => return,
        };
        if entry.last_buf_version == buf_version {
            return;
        }
        entry.version = entry.version.wrapping_add(1);
        let v = entry.version;
        entry.last_buf_version = buf_version;
        drop(docs);

        self.send_notification(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": { "uri": uri, "version": v },
                "contentChanges": [ { "text": text } ],
            }),
        );
    }

    fn did_close(&self, path: &Path) {
        let uri = path_to_uri(path);
        let mut docs = self.docs.lock().unwrap();
        match docs.remove(&uri) {
            Some(e) if e.opened => {}
            _ => return,
        }
        drop(docs);
        self.send_notification(
            "textDocument/didClose",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        );
    }

    fn goto_definition(self: &Arc<Self>, path: &Path, line: u32, utf8_col: u32) {
        let uri = path_to_uri(path);
        let character = match *self.position_encoding.lock().unwrap() {
            PositionEncoding::Utf8 => utf8_col,
            PositionEncoding::Utf16 => {
                // We only know the utf-8 column; the caller didn't give us the
                // line text to re-encode. Use as-is — for ASCII source this is
                // the same number, and r-a is forgiving inside identifiers.
                utf8_col
            }
        };
        let id = self.fresh_id();
        self.set_pending(
            id,
            Box::new(move |result, mgr| {
                let parsed = parse_definition_result(result);
                if let Some(def) = parsed {
                    if let Some(path) = uri_to_path(&def.uri) {
                        mgr.push_action(UiAction::JumpTo {
                            path,
                            line: def.line as usize,
                            col: def.character as usize,
                        });
                    }
                } else {
                    mgr.push_action(UiAction::Message("lsp: no definition".into()));
                }
            }),
        );
        self.send_request(
            id,
            "textDocument/definition",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character },
            }),
        );
    }

    fn request_completion(self: &Arc<Self>, path: &Path, line: u32, utf8_col: u32) -> u64 {
        let uri = path_to_uri(path);
        let character = match *self.position_encoding.lock().unwrap() {
            PositionEncoding::Utf8 => utf8_col,
            // We don't have the line text here to re-encode; r-a accepts the
            // utf-8 column as-is for ASCII source, which is the common case
            // for identifier prefixes the user is completing.
            PositionEncoding::Utf16 => utf8_col,
        };
        let id = self.fresh_id();
        let req_id = id;
        self.set_pending(
            id,
            Box::new(move |result, mgr| {
                let items = parse_completion_result(result);
                mgr.push_action(UiAction::Completion {
                    request_id: req_id,
                    items,
                });
            }),
        );
        self.send_request(
            id,
            "textDocument/completion",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character },
                "context": { "triggerKind": 1 },
            }),
        );
        id
    }

    fn handle_incoming(self: &Arc<Self>, msg: serde_json::Value, mgr: &Arc<Mutex<ManagerInner>>) {
        puffin::profile_scope!("lsp: handle_incoming");
        // Response?
        if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
            // It's either a response (has "result"/"error") or a request from
            // the server (has "method"). The latter we mostly ignore but must
            // reply to so the server doesn't stall.
            if msg.get("method").is_some() {
                // Server-originated request. Reply with a null result so we
                // don't deadlock the protocol.
                let reply = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": serde_json::Value::Null,
                });
                let _ = self.outbox.send(Outgoing::Message(reply));
                return;
            }
            let handler = self.pending.lock().unwrap().remove(&id);
            if let Some(h) = handler {
                let result = msg
                    .get("result")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let fake_mgr = LspManager { inner: mgr.clone() };
                h(&result, &fake_mgr);
            }
            return;
        }

        // Notification.
        let method = match msg.get("method").and_then(|v| v.as_str()) {
            Some(m) => m.to_string(),
            None => return,
        };
        let params = msg
            .get("params")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        match method.as_str() {
            "$/progress" => self.handle_progress(params, mgr),
            "window/logMessage" | "window/showMessage" | "telemetry/event" => { /* drop */ }
            "textDocument/publishDiagnostics" => { /* future milestone */ }
            _ => { /* drop */ }
        }
    }

    fn handle_progress(&self, params: serde_json::Value, mgr: &Arc<Mutex<ManagerInner>>) {
        let token = params.get("token").and_then(|t| {
            t.as_str()
                .map(|s| s.to_string())
                .or_else(|| t.as_u64().map(|n| n.to_string()))
        });
        let value = params.get("value");
        let (Some(token), Some(value)) = (token, value) else {
            return;
        };
        let kind = value.get("kind").and_then(|v| v.as_str()).unwrap_or("");

        let mut progress = self.progress.lock().unwrap();
        match kind {
            "begin" => {
                let title = value
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let message = value
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let percentage = value
                    .get("percentage")
                    .and_then(|v| v.as_u64())
                    .map(|n| n.min(100) as u8);
                progress.insert(
                    token,
                    ProgressEntry {
                        title,
                        message,
                        percentage,
                    },
                );
            }
            "report" => {
                if let Some(e) = progress.get_mut(&token) {
                    if let Some(m) = value.get("message").and_then(|v| v.as_str()) {
                        e.message = m.to_string();
                    }
                    if let Some(p) = value.get("percentage").and_then(|v| v.as_u64()) {
                        e.percentage = Some(p.min(100) as u8);
                    }
                }
            }
            "end" => {
                progress.remove(&token);
            }
            _ => {}
        }

        // Re-derive overall status from progress map. Any tracked progress =>
        // Indexing; empty => Ready (we don't downgrade from Ready to Disabled).
        let next = if let Some(e) = progress.values().next() {
            LspStatus::Indexing {
                pct: e.percentage,
                message: if e.message.is_empty() {
                    e.title.clone()
                } else {
                    e.message.clone()
                },
            }
        } else {
            LspStatus::Ready
        };
        drop(progress);

        *self.status.lock().unwrap() = next;
        bump_tick(mgr);
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Session {
    fn drop(&mut self) {
        // Best-effort polite shutdown. If the writer is already gone (channel
        // closed) it doesn't matter — the child gets killed below.
        let _ = self.outbox.send(Outgoing::Shutdown);
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct ParsedDef {
    uri: String,
    line: u32,
    character: u32,
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_definition_result(v: &serde_json::Value) -> Option<ParsedDef> {
    // Response can be Location | Location[] | LocationLink[] | null.
    if v.is_null() {
        return None;
    }
    let first = if v.is_array() {
        v.as_array().and_then(|a| a.first())?
    } else {
        v
    };

    // LocationLink uses `targetUri` + `targetSelectionRange`.
    if let Some(uri) = first.get("targetUri").and_then(|x| x.as_str()) {
        let range = first
            .get("targetSelectionRange")
            .or_else(|| first.get("targetRange"))?;
        let start = range.get("start")?;
        return Some(ParsedDef {
            uri: uri.to_string(),
            line: start.get("line")?.as_u64()? as u32,
            character: start.get("character")?.as_u64()? as u32,
        });
    }
    // Plain Location: `uri` + `range`.
    let uri = first.get("uri")?.as_str()?;
    let start = first.get("range")?.get("start")?;
    Some(ParsedDef {
        uri: uri.to_string(),
        line: start.get("line")?.as_u64()? as u32,
        character: start.get("character")?.as_u64()? as u32,
    })
}

/// Parse a `textDocument/completion` response into our flat
/// [`CompletionItem`] list. Accepts either `CompletionItem[]` or
/// `CompletionList { items, … }`. We cap at 200 entries — rust-analyzer
/// happily returns thousands for an empty prefix and the popup would stall
/// trying to render them.
#[cfg(not(target_arch = "wasm32"))]
fn parse_completion_result(v: &serde_json::Value) -> Vec<CompletionItem> {
    let items = if v.is_array() {
        match v.as_array() {
            Some(a) => a.clone(),
            None => return Vec::new(),
        }
    } else if let Some(items) = v.get("items").and_then(|x| x.as_array()) {
        items.clone()
    } else {
        return Vec::new();
    };

    items
        .into_iter()
        .take(200)
        .filter_map(|it| {
            let label = it
                .get("label")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            if label.is_empty() {
                return None;
            }
            let detail = it
                .get("detail")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            // textEdit.newText > insertText > label. We ignore textEdit.range
            // — the editor uses its own (word_start..cursor) replace range so
            // the LSP-supplied range and our cursor stay in sync if the user
            // typed between request and response.
            let insert_text = it
                .get("textEdit")
                .and_then(|te| te.get("newText"))
                .and_then(|s| s.as_str())
                .map(str::to_string)
                .or_else(|| {
                    it.get("insertText")
                        .and_then(|s| s.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| label.clone());
            let insert_text = strip_snippet(&insert_text);
            Some(CompletionItem {
                label,
                insert_text,
                detail,
            })
        })
        .collect()
}

/// Remove LSP snippet tabstops (`$0`, `$1`, `${1:foo}`) from a string. We
/// advertise `snippetSupport: false` but rust-analyzer still includes them in
/// some entries, so strip defensively before inserting.
#[cfg(not(target_arch = "wasm32"))]
fn strip_snippet(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' {
            match chars.peek() {
                Some(n) if n.is_ascii_digit() => {
                    while let Some(n) = chars.peek() {
                        if n.is_ascii_digit() {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    continue;
                }
                Some('{') => {
                    chars.next();
                    let mut depth = 1;
                    for c in chars.by_ref() {
                        if c == '{' {
                            depth += 1;
                        } else if c == '}' {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                    }
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
    }
    out
}

// ─── Framing ──────────────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
fn write_message<W: Write>(w: &mut W, v: &serde_json::Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(v)?;
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()
}

#[cfg(not(target_arch = "wasm32"))]
fn read_message<R: BufRead>(r: &mut R) -> std::io::Result<Option<serde_json::Value>> {
    let mut content_length: Option<usize> = None;
    let mut header = String::new();
    loop {
        header.clear();
        let n = r.read_line(&mut header)?;
        if n == 0 {
            return Ok(None);
        }
        if header == "\r\n" || header == "\n" {
            break;
        }
        if let Some(rest) = header.to_lowercase().strip_prefix("content-length:") {
            content_length = rest.trim().parse::<usize>().ok();
        }
    }
    let Some(len) = content_length else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "missing Content-Length",
        ));
    };
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    let v: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(Some(v))
}

// ─── Helpers ──────────────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
fn path_to_uri(p: &Path) -> String {
    let s = p.to_string_lossy();
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        // Best-effort for relative paths — shouldn't really happen because
        // callers absolutize first, but don't blow up if they didn't.
        format!("file://{s}")
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let s = uri.strip_prefix("file://")?;
    Some(PathBuf::from(s))
}

#[cfg(not(target_arch = "wasm32"))]
fn bump_tick(mgr: &Arc<Mutex<ManagerInner>>) {
    let mut g = mgr.lock().unwrap();
    g.tick = g.tick.wrapping_add(1);
}
