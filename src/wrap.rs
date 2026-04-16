//! Soft visual line wrapping.
//!
//! The editor lays out text as a flat sequence of "visual rows" — each
//! logical line is split into one or more rows of at most `wrap_cols`
//! characters. Wrap is purely a display concern; the buffer model still
//! addresses positions by `(logical_line, col)` and offsets in the rope.
//!
//! [`WrapMap`] caches the precomputed prefix sum of rows-per-line so the
//! editor can quickly answer "what visual row is this cursor on?" and
//! "what logical line is this visual row part of?" without walking the
//! whole buffer.

/// Per-buffer wrap layout snapshot. Rebuild whenever `wrap_cols` or the
/// line-char-count vector changes (i.e. on edits or window resize).
#[derive(Clone, Debug)]
pub struct WrapMap {
    /// Soft-wrap width in characters. `0` disables wrapping.
    wrap_cols: usize,
    /// Visual rows per logical line. Always `>= 1` so empty lines still
    /// take one row.
    rows: Vec<u32>,
    /// Prefix sum of `rows`. `cum[i]` is the first visual row of logical
    /// line `i`; `cum[N]` is the total visual-row count.
    cum: Vec<u32>,
}

impl WrapMap {
    pub fn new(line_char_counts: &[usize], wrap_cols: usize) -> Self {
        let rows: Vec<u32> = if wrap_cols == 0 {
            line_char_counts.iter().map(|_| 1).collect()
        } else {
            // `ceil(n / wrap_cols)`, with a floor of 1 so empty lines
            // still occupy a row. A line exactly `wrap_cols` characters
            // long takes one row, not two (otherwise the editor would
            // show a phantom blank line right under it).
            line_char_counts
                .iter()
                .map(|&n| {
                    if n == 0 {
                        1
                    } else {
                        ((n + wrap_cols - 1) / wrap_cols) as u32
                    }
                })
                .collect()
        };
        let mut cum = Vec::with_capacity(rows.len() + 1);
        let mut acc: u32 = 0;
        cum.push(0);
        for &r in &rows {
            acc = acc.saturating_add(r);
            cum.push(acc);
        }
        Self {
            wrap_cols,
            rows,
            cum,
        }
    }

    pub fn wrap_cols(&self) -> usize {
        self.wrap_cols
    }

    pub fn total_visual_rows(&self) -> usize {
        self.cum.last().copied().unwrap_or(0) as usize
    }

    pub fn rows_for_line(&self, line: usize) -> usize {
        self.rows.get(line).copied().unwrap_or(1) as usize
    }

    /// First visual row of logical line `line`. Returns `total_visual_rows`
    /// for `line == line_count`.
    pub fn visual_row_of_line(&self, line: usize) -> usize {
        self.cum
            .get(line)
            .copied()
            .unwrap_or_else(|| self.cum.last().copied().unwrap_or(0)) as usize
    }

    /// Convert a logical `(line, col)` into a visual `(row, sub_col)`.
    pub fn visual_pos(&self, line: usize, col: usize) -> (usize, usize) {
        let base = self.visual_row_of_line(line);
        if self.wrap_cols == 0 {
            return (base, col);
        }
        let max_seg = self.rows_for_line(line).saturating_sub(1);
        let seg = (col / self.wrap_cols).min(max_seg);
        let sub = col - seg * self.wrap_cols;
        (base + seg, sub)
    }

    /// Inverse of [`visual_pos`]: given a visual row, return the logical
    /// line containing it and the column at which that row's segment
    /// starts. Out-of-range rows clamp to the last logical line.
    pub fn logical_at_visual(&self, visual_row: usize) -> (usize, usize) {
        if self.cum.len() <= 1 {
            return (0, 0);
        }
        let target = visual_row as u32;
        // `partition_point` gives the smallest index `i` such that
        // `cum[i] > target`; the containing line is `i - 1`.
        let pos = self.cum.partition_point(|&v| v <= target);
        let line = pos.saturating_sub(1).min(self.rows.len().saturating_sub(1));
        let base = self.cum.get(line).copied().unwrap_or(0);
        let seg = target.saturating_sub(base) as usize;
        let seg = seg.min(self.rows_for_line(line).saturating_sub(1));
        let wrap = if self.wrap_cols == 0 {
            0
        } else {
            self.wrap_cols
        };
        (line, seg * wrap)
    }

    /// Iterate `(start_col, end_col)` segments for a logical line, given
    /// its total character count. Each segment is at most `wrap_cols`
    /// wide; for an empty line we yield a single `(0, 0)` segment.
    pub fn segments_for_line(
        &self,
        line: usize,
        line_chars: usize,
    ) -> impl Iterator<Item = (usize, usize)> + '_ {
        let n_segs = self.rows_for_line(line);
        let wrap = if self.wrap_cols == 0 {
            usize::MAX
        } else {
            self.wrap_cols
        };
        (0..n_segs).map(move |i| {
            let s = i.saturating_mul(wrap);
            let e = s.saturating_add(wrap).min(line_chars);
            (s, e.max(s))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_wrap_when_short() {
        let map = WrapMap::new(&[5, 10, 0, 20], 80);
        assert_eq!(map.total_visual_rows(), 4);
        assert_eq!(map.rows_for_line(0), 1);
        assert_eq!(map.visual_pos(2, 0), (2, 0));
    }

    #[test]
    fn wraps_long_line() {
        // 200 chars wrapped at 80 → 3 segments (80 + 80 + 40).
        let map = WrapMap::new(&[200], 80);
        assert_eq!(map.rows_for_line(0), 3);
        assert_eq!(map.total_visual_rows(), 3);
        assert_eq!(map.visual_pos(0, 0), (0, 0));
        assert_eq!(map.visual_pos(0, 79), (0, 79));
        assert_eq!(map.visual_pos(0, 80), (1, 0));
        assert_eq!(map.visual_pos(0, 159), (1, 79));
        assert_eq!(map.visual_pos(0, 160), (2, 0));
        assert_eq!(map.visual_pos(0, 200), (2, 40));
    }

    #[test]
    fn logical_at_visual_inverts_visual_pos() {
        let map = WrapMap::new(&[5, 200, 30], 80);
        // Line 1 occupies visual rows 1..=3.
        assert_eq!(map.logical_at_visual(0), (0, 0));
        assert_eq!(map.logical_at_visual(1), (1, 0));
        assert_eq!(map.logical_at_visual(2), (1, 80));
        assert_eq!(map.logical_at_visual(3), (1, 160));
        assert_eq!(map.logical_at_visual(4), (2, 0));
    }

    #[test]
    fn segments_cover_line() {
        let map = WrapMap::new(&[200], 80);
        let segs: Vec<_> = map.segments_for_line(0, 200).collect();
        assert_eq!(segs, vec![(0, 80), (80, 160), (160, 200)]);
    }

    #[test]
    fn exact_wrap_width_takes_one_row() {
        // 80 chars at wrap=80 must NOT add a phantom continuation row;
        // the cursor at col 80 lands at the right edge of the only row.
        let map = WrapMap::new(&[80], 80);
        assert_eq!(map.rows_for_line(0), 1);
        assert_eq!(map.total_visual_rows(), 1);
        assert_eq!(map.visual_pos(0, 80), (0, 80));
        let segs: Vec<_> = map.segments_for_line(0, 80).collect();
        assert_eq!(segs, vec![(0, 80)]);
    }
}
