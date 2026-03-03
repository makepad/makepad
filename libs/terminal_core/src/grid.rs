use crate::cell::{Cell, Style};

/// Fixed-size 2D cell grid (cols × rows), row-major.
pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    cells: Vec<Cell>,
    dirty: Vec<bool>,
    /// Per-row flag: true if the row soft-wrapped into the next row
    /// (content continued due to reaching the right margin).
    pub line_wrapped: Vec<bool>,
}

impl Grid {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols,
            rows,
            cells: vec![Cell::default(); cols * rows],
            dirty: vec![true; rows],
            line_wrapped: vec![false; rows],
        }
    }

    #[inline]
    pub fn cell(&self, col: usize, row: usize) -> &Cell {
        &self.cells[row * self.cols + col]
    }

    #[inline]
    pub fn cell_mut(&mut self, col: usize, row: usize) -> &mut Cell {
        self.dirty[row] = true;
        &mut self.cells[row * self.cols + col]
    }

    pub fn row_slice(&self, row: usize) -> &[Cell] {
        let start = row * self.cols;
        &self.cells[start..start + self.cols]
    }

    pub fn row_slice_mut(&mut self, row: usize) -> &mut [Cell] {
        self.dirty[row] = true;
        let start = row * self.cols;
        &mut self.cells[start..start + self.cols]
    }

    pub fn is_dirty(&self, row: usize) -> bool {
        self.dirty[row]
    }

    pub fn clear_dirty(&mut self, row: usize) {
        self.dirty[row] = false;
    }

    pub fn mark_all_dirty(&mut self) {
        self.dirty.fill(true);
    }

    /// Clear entire grid with default cells
    pub fn clear(&mut self) {
        for c in &mut self.cells {
            c.clear();
        }
        self.dirty.fill(true);
        self.line_wrapped.fill(false);
    }

    /// Clear a single row with optional background style
    pub fn clear_row(&mut self, row: usize, style: Style) {
        let start = row * self.cols;
        for c in &mut self.cells[start..start + self.cols] {
            c.clear_with_style(style);
        }
        self.dirty[row] = true;
    }

    /// Scroll lines in range [top, bottom) up by `count` lines.
    /// New lines at bottom are cleared.
    pub fn scroll_up(&mut self, top: usize, bottom: usize, count: usize, style: Style) {
        let count = count.min(bottom - top);
        // Move rows up
        for row in top..bottom - count {
            let src_start = (row + count) * self.cols;
            let dst_start = row * self.cols;
            for col in 0..self.cols {
                self.cells[dst_start + col] = self.cells[src_start + col];
            }
            self.dirty[row] = true;
            self.line_wrapped[row] = self.line_wrapped[row + count];
        }
        // Clear new lines at bottom
        for row in (bottom - count)..bottom {
            self.clear_row(row, style);
            self.line_wrapped[row] = false;
        }
    }

    /// Scroll lines in range [top, bottom) down by `count` lines.
    /// New lines at top are cleared.
    pub fn scroll_down(&mut self, top: usize, bottom: usize, count: usize, style: Style) {
        let count = count.min(bottom - top);
        // Move rows down (iterate from bottom to avoid overwrite)
        for row in (top + count..bottom).rev() {
            let src_start = (row - count) * self.cols;
            let dst_start = row * self.cols;
            for col in 0..self.cols {
                self.cells[dst_start + col] = self.cells[src_start + col];
            }
            self.dirty[row] = true;
            self.line_wrapped[row] = self.line_wrapped[row - count];
        }
        // Clear new lines at top
        for row in top..top + count {
            self.clear_row(row, style);
            self.line_wrapped[row] = false;
        }
    }

    /// Resize the grid. Content is preserved where possible.
    pub fn resize(&mut self, new_cols: usize, new_rows: usize) {
        let mut new_cells = vec![Cell::default(); new_cols * new_rows];
        let copy_rows = self.rows.min(new_rows);
        let copy_cols = self.cols.min(new_cols);

        let mut new_wrapped = vec![false; new_rows];
        for row in 0..copy_rows {
            for col in 0..copy_cols {
                new_cells[row * new_cols + col] = self.cells[row * self.cols + col];
            }
            new_wrapped[row] = self.line_wrapped[row];
        }

        self.cells = new_cells;
        self.cols = new_cols;
        self.rows = new_rows;
        self.dirty = vec![true; new_rows];
        self.line_wrapped = new_wrapped;
    }

    /// Insert `count` blank characters at (col, row), shifting right.
    /// Characters pushed past the right edge are lost.
    pub fn insert_blanks(&mut self, col: usize, row: usize, count: usize, style: Style) {
        let row_start = row * self.cols;
        let shift = count.min(self.cols - col);
        // Shift right
        for c in (col + shift..self.cols).rev() {
            self.cells[row_start + c] = self.cells[row_start + c - shift];
        }
        // Clear inserted
        for c in col..col + shift {
            self.cells[row_start + c].clear_with_style(style);
        }
        self.dirty[row] = true;
    }

    /// Delete `count` characters at (col, row), shifting left.
    /// Blank characters fill from the right.
    pub fn delete_chars(&mut self, col: usize, row: usize, count: usize, style: Style) {
        let row_start = row * self.cols;
        let shift = count.min(self.cols - col);
        // Shift left
        for c in col..self.cols - shift {
            self.cells[row_start + c] = self.cells[row_start + c + shift];
        }
        // Clear at right
        for c in (self.cols - shift)..self.cols {
            self.cells[row_start + c].clear_with_style(style);
        }
        self.dirty[row] = true;
    }
}
