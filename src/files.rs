//! Project file discovery and fuzzy matching utilities.

use std::path::{Path, PathBuf};

const IGNORE_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "vendor",
    ".cache",
    "dist",
    "build",
    "pkg",
    "web_build",
];

/// Walk up from `start` looking for a directory containing `.git` or
/// `.projectile`. Returns the first such ancestor, or None.
#[cfg(not(target_arch = "wasm32"))]
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };
    loop {
        if dir.join(".git").exists() || dir.join(".projectile").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Walk the project tree rooted at `root` and return a sorted list of files
/// (relative to `root`). Ignores `.git`, build outputs, and other noisy
/// directories. On wasm this returns an empty list — there is no filesystem.
pub fn scan_project(root: &Path) -> Vec<PathBuf> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = root;
        Vec::new()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut out = Vec::new();
        scan_dir(root, root, &mut out, 0);
        out.sort();
        out
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn scan_dir(root: &Path, dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            // Skip known noisy build/cache dirs, and don't recurse into
            // hidden directories — they'd bloat the index and rarely
            // contain something the user wants to fuzzy-pick. Dotfiles at
            // *file* level are still indexed so typing `.` in the picker
            // can surface them.
            if IGNORE_DIRS.iter().any(|d| *d == name_str) {
                continue;
            }
            if name_str.starts_with('.') {
                continue;
            }
            scan_dir(root, &path, out, depth + 1);
        } else if ft.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_path_buf());
            } else {
                out.push(path);
            }
        }
    }
}

/// True if `path`'s basename starts with `.` — used to gate dotfile
/// visibility in the project picker.
pub fn is_dotfile_path(path: &Path) -> bool {
    path.file_name()
        .map(|n| n.to_string_lossy().starts_with('.'))
        .unwrap_or(false)
}

/// Subsequence fuzzy match. Returns a score if every char of `query`
/// appears in `haystack` in order (case-insensitive), higher = better match.
pub fn fuzzy_score(query: &str, haystack: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.chars().flat_map(|c| c.to_lowercase()).collect();
    let h: Vec<char> = haystack.chars().flat_map(|c| c.to_lowercase()).collect();

    let mut qi = 0;
    let mut score: i32 = 0;
    let mut prev_match: Option<usize> = None;
    let mut consecutive: i32 = 0;

    for (i, &c) in h.iter().enumerate() {
        if qi >= q.len() {
            break;
        }
        if c == q[qi] {
            // Base match
            score += 1;
            // Bonus for consecutive matches
            if prev_match.map(|p| p + 1 == i).unwrap_or(false) {
                consecutive += 1;
                score += 5 + consecutive;
            } else {
                consecutive = 0;
            }
            // Bonus for matching after separator (word boundary)
            if i == 0
                || h.get(i.wrapping_sub(1))
                    .map(|&pc| matches!(pc, '/' | '_' | '-' | '.' | ' '))
                    .unwrap_or(false)
            {
                score += 4;
            }
            prev_match = Some(i);
            qi += 1;
        }
    }

    if qi == q.len() {
        // Prefer shorter haystacks as tiebreaker
        score -= (h.len() as i32) / 20;
        Some(score)
    } else {
        None
    }
}

/// Filter + sort candidates against a query using fuzzy_score. Returns
/// `(original_index, key_string)` pairs ordered best-match first.
pub fn fuzzy_filter_str(items: &[String], query: &str) -> Vec<(usize, String)> {
    puffin::profile_function!();
    let mut scored: Vec<(i32, usize, String)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| fuzzy_score(query, item).map(|s| (s, i, item.clone())))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().map(|(_, i, t)| (i, t)).collect()
}

/// True if a file-picker query should be interpreted as a literal filesystem
/// path rather than a fuzzy match against the project file index.
///
/// Path mode is opt-in when a project is active: `./foo`, `../foo`, `~/foo`,
/// or an absolute path. Bare `.` / `..` are left to the project fuzzy matcher
/// so typing normal fuzzy patterns does not temporarily drop into directory
/// listing mode.
pub fn is_path_query(query: &str) -> bool {
    query.starts_with('/')
        || query == "~"
        || query.starts_with("~/")
        || query.starts_with("./")
        || query.starts_with("../")
}

/// Expand a path-mode query into an absolute filesystem path string.
/// Resolves `~` against `$HOME` and `./`/`../` against `active_project`.
/// Does not touch the filesystem — purely string manipulation.
#[cfg(not(target_arch = "wasm32"))]
pub fn expand_path_query(query: &str, active_project: &Path) -> String {
    if let Some(rest) = query.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        if home.is_empty() {
            return query.to_string();
        }
        let mut s = home;
        if !s.ends_with('/') {
            s.push('/');
        }
        s.push_str(rest);
        s
    } else if query == "~" {
        std::env::var("HOME").unwrap_or_default()
    } else if query.starts_with("./") || query.starts_with("../") || query == "." || query == ".." {
        let mut s = active_project.to_string_lossy().into_owned();
        if !s.ends_with('/') {
            s.push('/');
        }
        s.push_str(query);
        s
    } else {
        query.to_string()
    }
}

/// Split a path-mode query at its final `/`. The directory side **includes**
/// the trailing slash so callers can compose new queries by appending. If the
/// expanded query has no `/`, returns `(expanded, "")`.
#[cfg(not(target_arch = "wasm32"))]
pub fn split_path_query(expanded: &str) -> (String, String) {
    if let Some(idx) = expanded.rfind('/') {
        (
            expanded[..=idx].to_string(),
            expanded[idx + 1..].to_string(),
        )
    } else {
        (expanded.to_string(), String::new())
    }
}

/// Listing of a single directory. Directories first (alphabetic), then files
/// (alphabetic). Hidden entries (leading `.`) are kept — users typing in path
/// mode have explicit intent and may want them. Returns `(name, is_dir)`.
///
/// Results are cached in a single-slot thread-local: while the user is
/// typing into the path picker the base directory stays the same for many
/// keystrokes, and re-running `readdir` + per-entry `file_type` on a large
/// folder (e.g. `$HOME`) every render visibly stalls input. Callers wanting
/// fresh data should close the picker and reopen it, or call
/// [`invalidate_dir_cache`].
#[cfg(not(target_arch = "wasm32"))]
pub fn list_dir(dir: &Path) -> Vec<(String, bool)> {
    puffin::profile_function!();
    DIR_CACHE.with(|cell| {
        if let Some((cached_dir, entries)) = cell.borrow().as_ref() {
            if cached_dir == dir {
                return entries.clone();
            }
        }
        let fresh = list_dir_uncached(dir);
        *cell.borrow_mut() = Some((dir.to_path_buf(), fresh.clone()));
        fresh
    })
}

/// Drop the cached directory listing — call before opening the file picker
/// so the first frame doesn't re-show stale entries from the previous
/// session. Currently unused; reserved for future "refresh" hotkey.
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
pub fn invalidate_dir_cache() {
    DIR_CACHE.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static DIR_CACHE: std::cell::RefCell<Option<(PathBuf, Vec<(String, bool)>)>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(not(target_arch = "wasm32"))]
fn list_dir_uncached(dir: &Path) -> Vec<(String, bool)> {
    puffin::profile_function!();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, bool)> = entries
        .flatten()
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            (name, is_dir)
        })
        .collect();
    out.sort_by(|a, b| match (a.1, b.1) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.0.cmp(&b.0),
    });
    out
}

/// Longest common prefix of a set of strings, byte-wise. Used by Tab
/// completion in path mode.
pub fn longest_common_prefix(strs: &[&str]) -> String {
    let Some(first) = strs.first() else {
        return String::new();
    };
    let mut end = first.len();
    for s in &strs[1..] {
        let a = first.as_bytes();
        let b = s.as_bytes();
        let mut i = 0;
        while i < end && i < b.len() && a[i] == b[i] {
            i += 1;
        }
        end = i;
        if end == 0 {
            break;
        }
    }
    // Snap to a UTF-8 char boundary in case we cut mid-codepoint.
    while end > 0 && !first.is_char_boundary(end) {
        end -= 1;
    }
    first[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_query_requires_explicit_path_prefix() {
        assert!(is_path_query("./src"));
        assert!(is_path_query("../src"));
        assert!(is_path_query("/tmp/file"));
        assert!(is_path_query("~/file"));
        assert!(is_path_query("~"));

        assert!(!is_path_query(""));
        assert!(!is_path_query("."));
        assert!(!is_path_query(".."));
        assert!(!is_path_query("src/main.rs"));
        assert!(!is_path_query("main.rs"));
    }
}
