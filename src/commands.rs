//! External command execution (ripgrep, cargo). Commands run in worker threads
//! and stream their output lines into a shared `CommandOutput`.
//!
//! On wasm, actually spawning processes is not available — the struct still
//! exists so the UI types stay cross-platform, but `run_*` entrypoints are
//! gated off.

use std::sync::{Arc, Mutex};

#[cfg(not(target_arch = "wasm32"))]
use std::io::{BufRead, BufReader};
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::process::{Command, Stdio};

#[derive(Clone)]
pub struct CommandOutput {
    /// Shared lines. The worker thread appends, the UI reads and renders.
    pub lines: Arc<Mutex<Vec<String>>>,
    /// Set to true when the command has exited.
    pub done: Arc<Mutex<bool>>,
}

impl CommandOutput {
    pub fn new() -> Self {
        Self {
            lines: Arc::new(Mutex::new(Vec::new())),
            done: Arc::new(Mutex::new(false)),
        }
    }

    pub fn snapshot(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }

    pub fn is_done(&self) -> bool {
        *self.done.lock().unwrap()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn push(&self, line: String) {
        self.lines.lock().unwrap().push(line);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn mark_done(&self) {
        *self.done.lock().unwrap() = true;
    }
}

/// Run `rg --fixed-strings --line-number PATTERN` under `cwd`. Returns an
/// output handle that gets filled asynchronously by a worker thread.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_ripgrep(pattern: &str, cwd: PathBuf) -> CommandOutput {
    let out = CommandOutput::new();
    let out_clone = out.clone();
    let pattern = pattern.to_string();

    std::thread::spawn(move || {
        out_clone.push(format!(
            "-*- mode: ripgrep-search; default-directory: \"{}\" -*-",
            cwd.display()
        ));
        out_clone.push(String::new());
        out_clone.push(format!("rg --fixed-strings --line-number {pattern}"));
        out_clone.push(String::new());

        let child = Command::new("rg")
            .arg("--fixed-strings")
            .arg("--line-number")
            .arg("--color=never")
            .arg("--no-heading")
            .arg("--")
            .arg(&pattern)
            .current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        match child {
            Ok(mut child) => {
                if let Some(stdout) = child.stdout.take() {
                    let reader = BufReader::new(stdout);
                    for line in reader.lines().flatten() {
                        out_clone.push(line);
                    }
                }
                let _ = child.wait();
            }
            Err(e) => {
                out_clone.push(format!("rg: failed to spawn: {e}"));
            }
        }
        out_clone.push(String::new());
        out_clone.push("Ripgrep finished".to_string());
        out_clone.mark_done();
    });

    out
}

/// Run rustfmt on one file, returning a compact status/error message.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_rustfmt_file(path: &Path) -> std::io::Result<String> {
    let output = Command::new("rustfmt")
        .arg("--edition")
        .arg("2024")
        .arg(path)
        .output()?;
    if output.status.success() {
        return Ok(format!("Formatted {}", path.display()));
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = stderr
        .lines()
        .chain(stdout.lines())
        .find(|line| !line.trim().is_empty())
        .unwrap_or("rustfmt failed");
    Ok(format!("rustfmt failed: {detail}"))
}

/// Run an arbitrary user-supplied command line under `cwd`. The string is
/// whitespace-split into argv — no shell parsing, so quoted arguments and
/// shell features (pipes, redirects, env-var expansion) won't work, but it
/// covers the typical compile commands (`cargo check`, `cargo test foo`,
/// `make -j8`). Streams both stdout and stderr.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_command(cmd: &str, cwd: PathBuf) -> CommandOutput {
    let out = CommandOutput::new();
    let out_clone = out.clone();
    let cmd = cmd.to_string();

    std::thread::spawn(move || {
        out_clone.push(format!(
            "-*- mode: compilation; default-directory: \"{}\" -*-",
            cwd.display()
        ));
        out_clone.push(String::new());
        out_clone.push(cmd.clone());
        out_clone.push(String::new());

        let mut parts = cmd.split_whitespace();
        let Some(program) = parts.next() else {
            out_clone.push("(empty command)".to_string());
            out_clone.mark_done();
            return;
        };
        let child = Command::new(program)
            .args(parts)
            .current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        match child {
            Ok(mut child) => {
                let stdout = child.stdout.take();
                let stderr = child.stderr.take();
                let out_a = out_clone.clone();
                let out_b = out_clone.clone();

                let ta = stdout.map(|s| {
                    std::thread::spawn(move || {
                        for line in BufReader::new(s).lines().flatten() {
                            out_a.push(line);
                        }
                    })
                });
                let tb = stderr.map(|s| {
                    std::thread::spawn(move || {
                        for line in BufReader::new(s).lines().flatten() {
                            out_b.push(line);
                        }
                    })
                });

                if let Some(t) = ta {
                    let _ = t.join();
                }
                if let Some(t) = tb {
                    let _ = t.join();
                }
                let _ = child.wait();
            }
            Err(e) => {
                out_clone.push(format!("failed to spawn: {e}"));
            }
        }
        out_clone.push(String::new());
        out_clone.push("Compilation finished".to_string());
        out_clone.mark_done();
    });

    out
}

/// Try to parse a clickable file location out of a single output line.
/// Matches two shapes:
///   * cargo:  `... --> path:line:col`  (column may be missing)
///   * rg:     `path:line:rest`         (no column — defaults to 1)
///
/// Returns `(absolute_path, line_no_1_indexed, col_1_indexed)`.
#[cfg(not(target_arch = "wasm32"))]
pub fn parse_location(line: &str, root: &std::path::Path) -> Option<(PathBuf, usize, usize)> {
    // cargo error/note: leading whitespace then `--> path:line:col`.
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix("--> ") {
        if let Some((p, l, c)) = parse_path_line_col(rest) {
            return Some((resolve_path(p, root), l, c));
        }
    }
    // ripgrep: starts with `path:line:...`. Reject lines whose first segment
    // is not actually a path-looking string so we don't mark random `:`
    // delimited text as clickable.
    if let Some((p, l, c)) = parse_path_line_col(line) {
        if looks_like_path(p) {
            return Some((resolve_path(p, root), l, c));
        }
    }
    None
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_path_line_col(s: &str) -> Option<(&str, usize, usize)> {
    let mut parts = s.splitn(4, ':');
    let path = parts.next()?;
    if path.is_empty() {
        return None;
    }
    let lineno: usize = parts.next()?.parse().ok()?;
    let col: usize = parts.next().and_then(|c| c.parse().ok()).unwrap_or(1);
    Some((path, lineno, col))
}

#[cfg(not(target_arch = "wasm32"))]
fn looks_like_path(s: &str) -> bool {
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

#[cfg(not(target_arch = "wasm32"))]
fn resolve_path(p: &str, root: &std::path::Path) -> PathBuf {
    let p = p.strip_prefix("./").unwrap_or(p);
    root.join(p)
}
