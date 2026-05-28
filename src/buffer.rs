/// Rope-based text buffer with cursor and selection management.
use lapce_xi_rope::{LinesMetric, Rope};
use std::path::PathBuf;

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

fn floor_char_boundary(s: &str, idx: usize) -> usize {
    let mut i = idx.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn brace_depth_after_line(line: &str, mut depth: usize) -> usize {
    let mut chars = line.chars().peekable();
    let mut in_string = false;
    let mut in_char = false;
    let mut escape = false;
    while let Some(ch) = chars.next() {
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if in_char {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '\'' {
                in_char = false;
            }
            continue;
        }
        match ch {
            '/' if chars.peek() == Some(&'/') => break,
            '"' => in_string = true,
            '\'' => in_char = true,
            '{' | '(' | '[' => depth = depth.saturating_add(1),
            '}' | ')' | ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth
}

/// Byte offset cursor position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPos {
    /// Byte offset into the rope.
    pub offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub active: usize,
}

impl Selection {
    pub fn start(self) -> usize {
        self.anchor.min(self.active)
    }

    pub fn end(self) -> usize {
        self.anchor.max(self.active)
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.active
    }
}

pub struct Buffer {
    pub rope: Rope,
    pub cursor: CursorPos,
    pub selection_anchor: Option<usize>,
    /// Display name shown in the modeline / buffer switcher.
    pub name: String,
    /// Backing file path, or None for scratch buffers.
    pub path: Option<PathBuf>,
    /// Project this buffer belongs to: nearest ancestor of `path` containing
    /// `.git` or `.projectile`. Used by the LSP layer to key sessions per
    /// crate, independent of the user-facing active project.
    pub project_root: Option<PathBuf>,
    /// True when the in-memory rope differs from the on-disk file. Cleared on
    /// load and save; tracked through undo/redo via the history snapshot.
    pub dirty: bool,
    /// Monotonic counter bumped on every mutation. Lets external observers
    /// (LSP sync) detect that the rope changed without rope comparison.
    pub version: u64,
    history: Vec<HistoryNode>,
    current_history: usize,
}

#[derive(Clone)]
struct HistoryEntry {
    rope: Rope,
    cursor_offset: usize,
    selection_anchor: Option<usize>,
    dirty: bool,
}

#[derive(Clone)]
struct HistoryNode {
    entry: HistoryEntry,
    parent: Option<usize>,
    children: Vec<usize>,
    /// Child to follow for ordinary `redo`. Updated when the user undoes
    /// from a child, redoes into one, or creates a new branch.
    preferred_child: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct UndoTreeEntry {
    pub id: usize,
    pub current: bool,
}

#[derive(Debug, Clone)]
pub struct UndoTreeVisualGraph {
    pub width: f64,
    pub height: f64,
    pub nodes: Vec<UndoTreeVisualNode>,
    pub edges: Vec<UndoTreeVisualEdge>,
}

#[derive(Debug, Clone)]
pub struct UndoTreeVisualNode {
    pub id: usize,
    pub x: f64,
    pub y: f64,
    pub selected: bool,
    pub current: bool,
}

#[derive(Debug, Clone)]
pub struct UndoTreeVisualEdge {
    pub parent: usize,
    pub child: usize,
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub angle_deg: f64,
    pub active: bool,
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        Buffer::with_name(text, "*scratch*".to_string(), None)
    }

    pub fn with_name(text: &str, name: String, path: Option<PathBuf>) -> Self {
        let rope = Rope::from(text);
        let root = HistoryNode {
            entry: HistoryEntry {
                rope: rope.clone(),
                cursor_offset: 0,
                selection_anchor: None,
                dirty: false,
            },
            parent: None,
            children: Vec::new(),
            preferred_child: None,
        };
        Buffer {
            rope,
            cursor: CursorPos { offset: 0 },
            selection_anchor: None,
            name,
            path,
            project_root: None,
            dirty: false,
            version: 0,
            history: vec![root],
            current_history: 0,
        }
    }

    fn bump_version(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    /// Persist the buffer to its backing path. Returns an error if there is no
    /// path or the write fails. Clears the dirty flag on success.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn save(&mut self) -> std::io::Result<&std::path::Path> {
        let Some(path) = self.path.clone() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "buffer has no file path",
            ));
        };
        std::fs::write(&path, self.text())?;
        self.dirty = false;
        self.sync_current_history_snapshot();
        Ok(self.path.as_deref().unwrap())
    }

    /// Load a file from disk, returning a Buffer. The buffer's project root
    /// is set to the nearest ancestor containing `.git` or `.projectile`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_path(path: PathBuf) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(&path)?;
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        let project_root = crate::files::find_project_root(&path);
        let mut buf = Buffer::with_name(&text, name, Some(path));
        buf.project_root = project_root;
        Ok(buf)
    }

    pub fn text(&self) -> String {
        String::from(&self.rope)
    }

    pub fn len(&self) -> usize {
        self.rope.len()
    }

    /// Total number of lines (at least 1).
    pub fn line_count(&self) -> usize {
        self.rope.measure::<LinesMetric>() + 1
    }

    /// Line number (0-indexed) of the cursor.
    pub fn cursor_line(&self) -> usize {
        self.rope.line_of_offset(self.offset())
    }

    /// Column (0-indexed, in displayed Unicode scalar values from line start)
    /// of the cursor. Buffer offsets are bytes, but editor geometry is in
    /// monospace character cells, so columns must not count UTF-8 continuation
    /// bytes (for example, `—` is one cell but three bytes).
    pub fn cursor_col(&self) -> usize {
        let line = self.cursor_line();
        let line_start = self.rope.offset_of_line(line);
        let text = self.text();
        text[line_start..self.offset().min(text.len())]
            .chars()
            .count()
    }

    pub fn offset(&self) -> usize {
        self.cursor.offset
    }

    pub fn selection(&self) -> Option<Selection> {
        let anchor = self.selection_anchor?;
        let selection = Selection {
            anchor,
            active: self.offset(),
        };
        if selection.is_empty() {
            None
        } else {
            Some(selection)
        }
    }

    pub fn clear_selection(&mut self) {
        self.selection_anchor = None;
    }

    pub fn undo(&mut self) -> bool {
        self.sync_current_history_snapshot();
        let current = self.current_history;
        let Some(parent) = self.history.get(current).and_then(|n| n.parent) else {
            return false;
        };
        self.history[parent].preferred_child = Some(current);
        self.restore_history_node(parent)
    }

    pub fn redo(&mut self) -> bool {
        self.sync_current_history_snapshot();
        let current = self.current_history;
        let Some(node) = self.history.get(current) else {
            return false;
        };
        let child = node
            .preferred_child
            .filter(|id| node.children.contains(id))
            .or_else(|| node.children.last().copied());
        let Some(child) = child else { return false };
        self.history[current].preferred_child = Some(child);
        self.restore_history_node(child)
    }

    /// Current undo-tree node. Synchronizes the node first so cancelling the
    /// visualizer can restore exactly the state that was visible when it was
    /// opened, including cursor/selection and dirty flag.
    pub fn undo_tree_current_node(&mut self) -> usize {
        self.sync_current_history_snapshot();
        self.current_history
    }

    pub fn undo_tree_entries(&self) -> Vec<UndoTreeEntry> {
        let mut entries = Vec::with_capacity(self.history.len());
        if !self.history.is_empty() {
            self.collect_undo_tree_entries(0, &mut entries);
        }
        entries
    }

    fn undo_tree_positions(&self) -> (Vec<isize>, Vec<usize>) {
        let mut xs = vec![0isize; self.history.len()];
        let mut depths = vec![0usize; self.history.len()];
        let mut next_leaf_x = 0isize;
        if !self.history.is_empty() {
            self.assign_undo_tree_positions(0, 0, &mut next_leaf_x, &mut xs, &mut depths);
        }
        (xs, depths)
    }

    fn assign_undo_tree_positions(
        &self,
        node_id: usize,
        depth: usize,
        next_leaf_x: &mut isize,
        xs: &mut [isize],
        depths: &mut [usize],
    ) -> isize {
        const COL_SPACING: isize = 4;
        let Some(node) = self.history.get(node_id) else {
            return *next_leaf_x;
        };
        depths[node_id] = depth;
        let children: Vec<usize> = node
            .children
            .iter()
            .copied()
            .filter(|id| *id < self.history.len())
            .collect();
        if children.is_empty() {
            let x = *next_leaf_x;
            *next_leaf_x += COL_SPACING;
            xs[node_id] = x;
            return x;
        }

        let mut first_child_x = None;
        let mut last_child_x = 0isize;
        for child in children {
            let child_x =
                self.assign_undo_tree_positions(child, depth + 1, next_leaf_x, xs, depths);
            if first_child_x.is_none() {
                first_child_x = Some(child_x);
            }
            last_child_x = child_x;
        }
        let x = match first_child_x {
            Some(first) if first != last_child_x => (first + last_child_x) / 2,
            Some(first) => first,
            None => {
                let x = *next_leaf_x;
                *next_leaf_x += COL_SPACING;
                x
            }
        };
        xs[node_id] = x;
        x
    }

    pub fn undo_tree_visual_graph(&self, selected: usize) -> UndoTreeVisualGraph {
        if self.history.is_empty() {
            return UndoTreeVisualGraph {
                width: 180.0,
                height: 80.0,
                nodes: Vec::new(),
                edges: Vec::new(),
            };
        }
        let selected = if selected < self.history.len() {
            selected
        } else {
            self.current_history
        };
        let (xs, depths) = self.undo_tree_positions();
        let min_x = xs.iter().copied().min().unwrap_or(0);
        let max_x = xs.iter().copied().max().unwrap_or(0);
        let max_depth = depths.iter().copied().max().unwrap_or(0);

        const X_SCALE: f64 = 15.0;
        const Y_STEP: f64 = 58.0;
        const PAD_X: f64 = 40.0;
        const PAD_Y: f64 = 34.0;
        const NODE_RADIUS: f64 = 9.0;
        const EDGE_THICKNESS: f64 = 2.0;

        let raw_width = ((max_x - min_x) as f64) * X_SCALE + PAD_X * 2.0;
        let graph_width = raw_width.max(180.0);
        let x_offset = (graph_width - raw_width) / 2.0;
        let graph_height = (max_depth as f64) * Y_STEP + PAD_Y * 2.0;

        let node_xy = |id: usize| -> (f64, f64) {
            (
                ((xs[id] - min_x) as f64) * X_SCALE + PAD_X + x_offset,
                (depths[id] as f64) * Y_STEP + PAD_Y,
            )
        };

        let mut active_path = vec![false; self.history.len()];
        let mut n = Some(selected);
        while let Some(id) = n {
            if id >= self.history.len() || active_path[id] {
                break;
            }
            active_path[id] = true;
            n = self.history[id].parent;
        }

        let mut edges = Vec::new();
        for (parent, node) in self.history.iter().enumerate() {
            let (px, py) = node_xy(parent);
            for &child in &node.children {
                if child >= self.history.len() {
                    continue;
                }
                let (cx, cy) = node_xy(child);
                let dx = cx - px;
                let dy = cy - py;
                let len = (dx * dx + dy * dy).sqrt();
                if len <= NODE_RADIUS * 2.0 {
                    continue;
                }
                let ux = dx / len;
                let uy = dy / len;
                let sx = px + ux * NODE_RADIUS;
                let sy = py + uy * NODE_RADIUS;
                let ex = cx - ux * NODE_RADIUS;
                let ey = cy - uy * NODE_RADIUS;
                let width = ((ex - sx) * (ex - sx) + (ey - sy) * (ey - sy)).sqrt();
                let angle_deg = (ey - sy).atan2(ex - sx).to_degrees();
                edges.push(UndoTreeVisualEdge {
                    parent,
                    child,
                    left: sx,
                    top: sy - EDGE_THICKNESS / 2.0,
                    width,
                    angle_deg,
                    active: active_path[child],
                });
            }
        }

        let nodes = (0..self.history.len())
            .map(|id| {
                let (x, y) = node_xy(id);
                UndoTreeVisualNode {
                    id,
                    x,
                    y,
                    selected: id == selected,
                    current: id == self.current_history,
                }
            })
            .collect();

        UndoTreeVisualGraph {
            width: graph_width,
            height: graph_height,
            nodes,
            edges,
        }
    }

    pub fn undo_tree_neighbor(&self, selected: usize, delta: isize) -> usize {
        let entries = self.undo_tree_entries();
        if entries.is_empty() {
            return self.current_history;
        }
        let pos = entries
            .iter()
            .position(|e| e.id == selected)
            .or_else(|| entries.iter().position(|e| e.current))
            .unwrap_or(0);
        let next = if delta < 0 {
            pos.saturating_sub((-delta) as usize)
        } else {
            pos.saturating_add(delta as usize).min(entries.len() - 1)
        };
        entries[next].id
    }

    pub fn undo_tree_first_node(&self) -> usize {
        self.undo_tree_entries()
            .first()
            .map(|e| e.id)
            .unwrap_or(self.current_history)
    }

    pub fn undo_tree_last_node(&self) -> usize {
        self.undo_tree_entries()
            .last()
            .map(|e| e.id)
            .unwrap_or(self.current_history)
    }

    pub fn undo_tree_parent_node(&self, selected: usize) -> usize {
        self.history
            .get(selected)
            .and_then(|n| n.parent)
            .unwrap_or(selected)
    }

    pub fn undo_tree_horizontal_node(&self, selected: usize, dir: isize) -> usize {
        if selected >= self.history.len() || dir == 0 {
            return selected;
        }
        let (xs, depths) = self.undo_tree_positions();
        let selected_depth = depths[selected];
        let selected_x = xs[selected];
        let mut best: Option<(usize, isize)> = None;
        for id in 0..self.history.len() {
            if id == selected || depths[id] != selected_depth {
                continue;
            }
            let dx = xs[id] - selected_x;
            if (dir < 0 && dx >= 0) || (dir > 0 && dx <= 0) {
                continue;
            }
            let dist = dx.abs();
            if best.map(|(_, best_dist)| dist < best_dist).unwrap_or(true) {
                best = Some((id, dist));
            }
        }
        best.map(|(id, _)| id).unwrap_or(selected)
    }

    pub fn undo_tree_first_child_node(&self, selected: usize) -> usize {
        self.history
            .get(selected)
            .and_then(|n| {
                n.preferred_child
                    .filter(|id| n.children.contains(id))
                    .or_else(|| n.children.first().copied())
            })
            .unwrap_or(selected)
    }

    /// Restore a node for undo-tree preview/accept. This moves through the
    /// tree without creating or deleting history nodes.
    pub fn restore_undo_tree_node(&mut self, target: usize) -> bool {
        self.sync_current_history_snapshot();
        self.restore_history_node(target)
    }

    fn restore_history_node(&mut self, target: usize) -> bool {
        if target == self.current_history || target >= self.history.len() {
            return false;
        }
        let entry = self.history[target].entry.clone();
        if let Some(parent) = self.history[target].parent {
            self.history[parent].preferred_child = Some(target);
        }
        self.restore(entry);
        self.current_history = target;
        self.bump_version();
        true
    }

    fn collect_undo_tree_entries(&self, node_id: usize, entries: &mut Vec<UndoTreeEntry>) {
        let Some(node) = self.history.get(node_id) else {
            return;
        };
        entries.push(UndoTreeEntry {
            id: node_id,
            current: node_id == self.current_history,
        });
        for child in node.children.iter().copied() {
            self.collect_undo_tree_entries(child, entries);
        }
    }

    pub fn selection_line_cols(&self) -> Option<((usize, usize), (usize, usize))> {
        let selection = self.selection()?;
        Some((
            self.offset_to_line_col(selection.start()),
            self.offset_to_line_col(selection.end()),
        ))
    }

    /// Selected substring, or `None` when no non-empty selection exists.
    /// Used by the copy / cut handlers.
    pub fn selection_text(&self) -> Option<String> {
        let sel = self.selection()?;
        let text = self.text();
        Some(text[sel.start()..sel.end()].to_string())
    }

    pub fn line_text(&self, line: usize) -> String {
        let max_line = self.line_count() - 1;
        let line = line.min(max_line);
        let start = self.rope.offset_of_line(line);
        let end = self.line_end_offset(line).min(self.len());
        self.text()[start..end].to_string()
    }

    pub fn retab_current_brace_line(&mut self) {
        if let Some(selection) = self.selection() {
            self.retab_brace_selection(selection);
            return;
        }

        let line = self.cursor_line();
        let col = self.cursor_col();
        let line_text = self.line_text(line);
        let leading_ws = line_text
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .count();

        let target_spaces = self.brace_indent_for_line(line) * 4;
        let current_prefix_bytes = line_text
            .char_indices()
            .nth(leading_ws)
            .map(|(i, _)| i)
            .unwrap_or(line_text.len());
        let new_prefix = " ".repeat(target_spaces);
        let old_prefix_cols = leading_ws;
        let new_prefix_cols = target_spaces;
        let target_col = if col <= old_prefix_cols {
            new_prefix_cols
        } else if new_prefix_cols >= old_prefix_cols {
            col.saturating_add(new_prefix_cols - old_prefix_cols)
        } else {
            col.saturating_sub(old_prefix_cols - new_prefix_cols)
        };
        if current_prefix_bytes == new_prefix.len() && line_text.starts_with(&new_prefix) {
            self.move_to(line, target_col);
            return;
        }

        let parent = self.begin_edit();
        let start = self.rope.offset_of_line(line);
        let end = start + current_prefix_bytes;
        self.rope.edit(start..end, Rope::from(new_prefix.as_str()));
        self.cursor.offset = self.line_col_to_offset(line, target_col);
        self.clear_selection();
        self.dirty = true;
        self.record_edit(parent);
        self.bump_version();
    }

    fn retab_brace_selection(&mut self, selection: Selection) {
        let (start_line, _) = self.offset_to_line_col(selection.start());
        let (mut end_line, end_col) = self.offset_to_line_col(selection.end());
        if end_col == 0 && end_line > start_line {
            end_line -= 1;
        }

        let mut edits = Vec::new();
        for line in start_line..=end_line {
            let line_text = self.line_text(line);
            let leading_ws = line_text
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .count();
            let current_prefix_bytes = line_text
                .char_indices()
                .nth(leading_ws)
                .map(|(i, _)| i)
                .unwrap_or(line_text.len());
            let new_prefix = " ".repeat(self.brace_indent_for_line(line) * 4);
            if current_prefix_bytes == new_prefix.len() && line_text.starts_with(&new_prefix) {
                continue;
            }
            let start = self.rope.offset_of_line(line);
            edits.push((start, start + current_prefix_bytes, new_prefix));
        }

        if edits.is_empty() {
            return;
        }

        let map_offset = |offset: usize| -> usize {
            let mut delta = 0isize;
            for (start, end, new_prefix) in &edits {
                if offset < *start {
                    break;
                }
                let old_len = end.saturating_sub(*start);
                let new_len = new_prefix.len();
                if offset == *start {
                    return (*start as isize + delta) as usize;
                }
                if offset <= *end {
                    let within = if offset == *end {
                        new_len
                    } else {
                        (offset - *start).min(new_len)
                    };
                    return (*start as isize + delta + within as isize) as usize;
                }
                delta += new_len as isize - old_len as isize;
            }
            (offset as isize + delta) as usize
        };
        let new_cursor = map_offset(self.cursor.offset);
        let new_anchor = self.selection_anchor.map(map_offset);

        let parent = self.begin_edit();
        for (start, end, new_prefix) in edits.iter().rev() {
            self.rope
                .edit(*start..*end, Rope::from(new_prefix.as_str()));
        }
        let len = self.len();
        self.cursor.offset = new_cursor.min(len);
        self.selection_anchor = new_anchor.map(|anchor| anchor.min(len));
        self.normalize_selection();
        self.dirty = true;
        self.record_edit(parent);
        self.bump_version();
    }

    fn brace_indent_for_line(&self, target_line: usize) -> usize {
        let mut depth = 0usize;
        for line in 0..target_line.min(self.line_count()) {
            depth = brace_depth_after_line(&self.line_text(line), depth);
        }
        let current = self.line_text(target_line);
        if matches!(current.trim_start().chars().next(), Some('}' | ')' | ']')) {
            depth = depth.saturating_sub(1);
        }
        depth
    }

    pub fn move_to_offset(&mut self, offset: usize) {
        let text = self.text();
        self.cursor.offset = floor_char_boundary(&text, offset);
        self.clear_selection();
    }

    pub fn move_to_offset_with_selection(&mut self, offset: usize) {
        self.ensure_selection_anchor();
        let text = self.text();
        self.cursor.offset = floor_char_boundary(&text, offset);
        self.normalize_selection();
    }

    pub fn select_to(&mut self, line: usize, col: usize) {
        let offset = self.line_col_to_offset(line, col);
        self.move_to_offset_with_selection(offset);
    }

    pub fn set_cursor(&mut self, line: usize, col: usize) {
        let offset = self.line_col_to_offset(line, col);
        self.move_to_offset(offset);
    }

    pub fn delete_selection(&mut self) -> bool {
        let Some(selection) = self.selection() else {
            return false;
        };
        let parent = self.begin_edit();
        let start = selection.start();
        let end = selection.end();
        self.rope.edit(start..end, Rope::from(""));
        self.cursor.offset = start;
        self.clear_selection();
        self.dirty = true;
        self.record_edit(parent);
        self.bump_version();
        true
    }

    /// Insert text at cursor position.
    pub fn insert(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let parent = self.begin_edit();
        let off = if let Some(selection) = self.selection() {
            let start = selection.start();
            let end = selection.end();
            self.rope.edit(start..end, Rope::from(""));
            start
        } else {
            self.offset()
        };
        self.rope.edit(off..off, Rope::from(text));
        self.cursor.offset = off + text.len();
        self.clear_selection();
        self.dirty = true;
        self.record_edit(parent);
        self.bump_version();
    }

    /// Delete the grapheme before the cursor (backspace).
    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        let off = self.offset();
        if off == 0 {
            return;
        }
        if let Some(prev) = self.rope.prev_grapheme_offset(off) {
            let parent = self.begin_edit();
            self.rope.edit(prev..off, Rope::from(""));
            self.cursor.offset = prev;
            self.dirty = true;
            self.record_edit(parent);
            self.bump_version();
        }
    }

    /// Delete the grapheme after the cursor.
    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        let off = self.offset();
        if off >= self.len() {
            return;
        }
        if let Some(next) = self.rope.next_grapheme_offset(off) {
            let parent = self.begin_edit();
            self.rope.edit(off..next, Rope::from(""));
            self.dirty = true;
            self.record_edit(parent);
            self.bump_version();
        }
    }

    /// Move cursor left by one grapheme.
    pub fn move_left(&mut self) {
        if let Some(selection) = self.selection() {
            self.move_to_offset(selection.start());
            return;
        }
        let off = self.offset();
        if let Some(prev) = self.rope.prev_grapheme_offset(off) {
            self.cursor.offset = prev;
        }
    }

    pub fn move_left_with_selection(&mut self) {
        let off = self.offset();
        if let Some(prev) = self.rope.prev_grapheme_offset(off) {
            self.move_to_offset_with_selection(prev);
        }
    }

    pub fn move_word_left(&mut self) {
        if let Some(selection) = self.selection() {
            self.move_to_offset(selection.start());
            return;
        }
        let target = self.word_left_offset();
        self.move_to_offset(target);
    }

    pub fn move_word_left_with_selection(&mut self) {
        let target = self.word_left_offset();
        self.move_to_offset_with_selection(target);
    }

    /// Move cursor right by one grapheme.
    pub fn move_right(&mut self) {
        if let Some(selection) = self.selection() {
            self.move_to_offset(selection.end());
            return;
        }
        let off = self.offset();
        if let Some(next) = self.rope.next_grapheme_offset(off) {
            self.cursor.offset = next;
        }
    }

    pub fn move_right_with_selection(&mut self) {
        let off = self.offset();
        if let Some(next) = self.rope.next_grapheme_offset(off) {
            self.move_to_offset_with_selection(next);
        }
    }

    pub fn move_word_right(&mut self) {
        if let Some(selection) = self.selection() {
            self.move_to_offset(selection.end());
            return;
        }
        let target = self.word_right_offset();
        self.move_to_offset(target);
    }

    pub fn move_word_right_with_selection(&mut self) {
        let target = self.word_right_offset();
        self.move_to_offset_with_selection(target);
    }

    /// Move cursor to start of current line.
    pub fn move_to_line_start(&mut self) {
        let line = self.cursor_line();
        self.move_to_offset(self.rope.offset_of_line(line));
    }

    pub fn move_to_line_start_with_selection(&mut self) {
        let line = self.cursor_line();
        self.move_to_offset_with_selection(self.rope.offset_of_line(line));
    }

    /// Move cursor to end of current line.
    pub fn move_to_line_end(&mut self) {
        let line = self.cursor_line();
        self.move_to_offset(self.line_end_offset(line));
    }

    pub fn move_to_line_end_with_selection(&mut self) {
        let line = self.cursor_line();
        self.move_to_offset_with_selection(self.line_end_offset(line));
    }

    /// Move cursor to start of buffer.
    pub fn move_to_start(&mut self) {
        self.move_to_offset(0);
    }

    pub fn move_to_start_with_selection(&mut self) {
        self.move_to_offset_with_selection(0);
    }

    /// Move cursor to end of buffer.
    pub fn move_to_end(&mut self) {
        self.move_to_offset(self.len());
    }

    pub fn move_to_end_with_selection(&mut self) {
        self.move_to_offset_with_selection(self.len());
    }

    pub fn select_all(&mut self) {
        self.selection_anchor = Some(0);
        self.cursor.offset = self.len();
        self.normalize_selection();
    }

    /// Move cursor to a specific line and column (0-indexed, byte column).
    /// Clamps to valid positions.
    pub fn move_to(&mut self, line: usize, col: usize) {
        self.set_cursor(line, col);
    }

    pub fn move_to_with_selection(&mut self, line: usize, col: usize) {
        self.select_to(line, col);
    }

    /// Byte range of the identifier-style word at `(line, col)`, if any.
    /// Returns `None` when the position lands on whitespace, punctuation,
    /// or past end-of-buffer. Identifier = `[A-Za-z0-9_]`.
    pub fn word_range_at(&self, line: usize, col: usize) -> Option<(usize, usize)> {
        let text = self.text();
        if text.is_empty() {
            return None;
        }
        let mut offset = self.line_col_to_offset(line, col).min(text.len());
        if offset == text.len() {
            offset = prev_char_boundary(&text, offset);
        }
        let ch = text[offset..].chars().next()?;
        let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
        if !is_word(ch) {
            return None;
        }
        let mut start = offset;
        while start > 0 {
            let prev = prev_char_boundary(&text, start);
            let Some(c) = text[prev..start].chars().next() else {
                break;
            };
            if !is_word(c) {
                break;
            }
            start = prev;
        }
        let mut end = offset + ch.len_utf8();
        while end < text.len() {
            let Some(c) = text[end..].chars().next() else {
                break;
            };
            if !is_word(c) {
                break;
            }
            end += c.len_utf8();
        }
        Some((start, end))
    }

    pub fn select_word_at(&mut self, line: usize, col: usize) {
        let mut offset = self.line_col_to_offset(line, col).min(self.len());
        let text = self.text();
        if text.is_empty() {
            self.move_to_offset(0);
            return;
        }
        if offset == text.len() {
            offset = prev_char_boundary(&text, offset);
        }
        let Some(ch) = text[offset..].chars().next() else {
            self.move_to_offset(offset);
            return;
        };
        if ch.is_whitespace() {
            self.move_to_offset(offset);
            return;
        }

        let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
        let (start, end) = if is_word(ch) {
            let mut start = offset;
            while start > 0 {
                let prev = prev_char_boundary(&text, start);
                let Some(c) = text[prev..start].chars().next() else {
                    break;
                };
                if !is_word(c) {
                    break;
                }
                start = prev;
            }
            let mut end = offset + ch.len_utf8();
            while end < text.len() {
                let Some(c) = text[end..].chars().next() else {
                    break;
                };
                if !is_word(c) {
                    break;
                }
                end += c.len_utf8();
            }
            (start, end)
        } else {
            (offset, offset + ch.len_utf8())
        };
        self.selection_anchor = Some(start);
        self.cursor.offset = end.min(self.len());
        self.normalize_selection();
    }

    pub fn offset_to_line_col(&self, offset: usize) -> (usize, usize) {
        let text = self.text();
        let offset = floor_char_boundary(&text, offset.min(text.len()));
        let line = self.rope.line_of_offset(offset);
        let line_start = self.rope.offset_of_line(line);
        (line, text[line_start..offset].chars().count())
    }

    pub fn line_col_to_offset(&self, line: usize, col: usize) -> usize {
        let max_line = self.line_count() - 1;
        let line = line.min(max_line);
        let line_start = self.rope.offset_of_line(line);
        let line_end = self.line_end_offset(line);
        let text = self.text();
        let line_text = &text[line_start..line_end.min(text.len())];
        match line_text.char_indices().nth(col) {
            Some((byte_idx, _)) => line_start + byte_idx,
            None => line_end,
        }
    }

    fn line_end_offset(&self, line: usize) -> usize {
        let max_line = self.line_count() - 1;
        if line < max_line {
            self.rope.offset_of_line(line + 1).saturating_sub(1)
        } else {
            self.len()
        }
    }

    fn word_left_offset(&self) -> usize {
        let text = self.text();
        let mut i = floor_char_boundary(&text, self.offset());
        if i == 0 {
            return 0;
        }

        while i > 0 {
            let prev = prev_char_boundary(&text, i);
            let Some(c) = text[prev..i].chars().next() else {
                break;
            };
            if is_word_char(c) {
                break;
            }
            i = prev;
        }
        while i > 0 {
            let prev = prev_char_boundary(&text, i);
            let Some(c) = text[prev..i].chars().next() else {
                break;
            };
            if !is_word_char(c) {
                break;
            }
            i = prev;
        }
        i
    }

    fn word_right_offset(&self) -> usize {
        let text = self.text();
        let mut i = floor_char_boundary(&text, self.offset());
        if i >= text.len() {
            return text.len();
        }

        while i < text.len() {
            let Some(c) = text[i..].chars().next() else {
                break;
            };
            if is_word_char(c) {
                break;
            }
            i += c.len_utf8();
        }
        while i < text.len() {
            let Some(c) = text[i..].chars().next() else {
                break;
            };
            if !is_word_char(c) {
                break;
            }
            i += c.len_utf8();
        }
        i
    }

    fn ensure_selection_anchor(&mut self) {
        if self.selection_anchor.is_none() {
            self.selection_anchor = Some(self.offset());
        }
    }

    fn normalize_selection(&mut self) {
        if self.selection().is_none() {
            self.clear_selection();
        }
    }

    fn snapshot(&self) -> HistoryEntry {
        HistoryEntry {
            rope: self.rope.clone(),
            cursor_offset: self.cursor.offset,
            selection_anchor: self.selection_anchor,
            dirty: self.dirty,
        }
    }

    fn restore(&mut self, entry: HistoryEntry) {
        self.rope = entry.rope;
        let text = self.text();
        self.cursor.offset = floor_char_boundary(&text, entry.cursor_offset);
        self.selection_anchor = entry
            .selection_anchor
            .map(|anchor| floor_char_boundary(&text, anchor));
        self.dirty = entry.dirty;
        self.normalize_selection();
    }

    fn sync_current_history_snapshot(&mut self) {
        let snapshot = self.snapshot();
        if let Some(node) = self.history.get_mut(self.current_history) {
            node.entry = snapshot;
        }
    }

    fn begin_edit(&mut self) -> usize {
        self.sync_current_history_snapshot();
        self.current_history
    }

    fn record_edit(&mut self, parent: usize) {
        if self.history.is_empty() {
            return;
        }
        let parent = parent.min(self.history.len() - 1);
        let id = self.history.len();
        self.history.push(HistoryNode {
            entry: self.snapshot(),
            parent: Some(parent),
            children: Vec::new(),
            preferred_child: None,
        });
        self.history[parent].children.push(id);
        self.history[parent].preferred_child = Some(id);
        self.current_history = id;
    }
}
