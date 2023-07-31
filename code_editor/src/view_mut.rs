use {
    crate::{
        line, view, view::LineInlay, Bias, BiasedTextPos, Sel, Settings, Text, TextDiff,
        TextPos, TextRange, Tokenizer, View,
    },
    std::collections::HashSet,
};

#[derive(Debug, PartialEq)]
pub struct ViewMut<'a> {
    settings: &'a mut Settings,
    max_col: &'a mut Option<usize>,
    text: &'a mut Text,
    tokenizer: &'a mut Tokenizer,
    inline_text_inlays: &'a mut Vec<Vec<(usize, String)>>,
    inline_widget_inlays: &'a mut Vec<Vec<((usize, Bias), line::Widget)>>,
    soft_breaks: &'a mut Vec<Vec<usize>>,
    start_col_after_wrap: &'a mut Vec<usize>,
    fold_col: &'a mut Vec<usize>,
    scale: &'a mut Vec<f64>,
    line_inlays: &'a mut Vec<(usize, LineInlay)>,
    block_widget_inlays: &'a mut Vec<((usize, Bias), view::Widget)>,
    summed_heights: &'a mut Vec<f64>,
    sels: &'a mut Vec<Sel>,
    latest_sel_index: &'a mut usize,
    folding_lines: &'a mut HashSet<usize>,
    unfolding_lines: &'a mut HashSet<usize>,
}

impl<'a> ViewMut<'a> {
    pub fn new(
        settings: &'a mut Settings,
        max_col: &'a mut Option<usize>,
        text: &'a mut Text,
        tokenizer: &'a mut Tokenizer,
        inline_text_inlays: &'a mut Vec<Vec<(usize, String)>>,
        inline_widget_inlays: &'a mut Vec<Vec<((usize, Bias), line::Widget)>>,
        soft_breaks: &'a mut Vec<Vec<usize>>,
        start_col_after_wrap: &'a mut Vec<usize>,
        fold_col: &'a mut Vec<usize>,
        scale: &'a mut Vec<f64>,
        line_inlays: &'a mut Vec<(usize, LineInlay)>,
        block_widget_inlays: &'a mut Vec<((usize, Bias), view::Widget)>,
        summed_heights: &'a mut Vec<f64>,
        sels: &'a mut Vec<Sel>,
        latest_sel_index: &'a mut usize,
        folding_lines: &'a mut HashSet<usize>,
        unfolding_lines: &'a mut HashSet<usize>,
    ) -> Self {
        Self {
            settings,
            max_col,
            text,
            tokenizer,
            inline_text_inlays,
            inline_widget_inlays,
            soft_breaks,
            start_col_after_wrap,
            fold_col,
            scale,
            line_inlays,
            block_widget_inlays,
            summed_heights,
            sels,
            latest_sel_index,
            folding_lines,
            unfolding_lines,
        }
    }

    pub fn document(&self) -> View<'_> {
        View::new(
            self.settings,
            self.text,
            self.tokenizer,
            self.inline_text_inlays,
            self.inline_widget_inlays,
            self.soft_breaks,
            self.start_col_after_wrap,
            self.fold_col,
            self.scale,
            self.line_inlays,
            self.block_widget_inlays,
            self.summed_heights,
            self.sels,
            *self.latest_sel_index,
        )
    }

    pub fn set_max_col(&mut self, max_col: Option<usize>) {
        if *self.max_col == max_col {
            return;
        }
        *self.max_col = max_col;
        self.wrap_lines();
    }

    pub fn replace(&mut self, replace_with: Text) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::replace(range, replace_with.clone()))
    }

    pub fn enter(&mut self) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::enter(range))
    }

    pub fn delete(&mut self) {
        use crate::edit_ops;

        self.modify_text(|_, range| edit_ops::delete(range))
    }

    pub fn backspace(&mut self) {
        use crate::edit_ops;

        self.modify_text(edit_ops::backspace)
    }

    pub fn set_cursor_pos(&mut self, pos: BiasedTextPos) {
        use crate::Cursor;

        self.sels.clear();
        self.sels.push(Sel::from(Cursor::from(pos)));
        *self.latest_sel_index = 0;
    }

    pub fn insert_cursor(&mut self, pos: BiasedTextPos) {
        use {crate::Cursor, std::cmp::Ordering};

        let sel = Sel::from(Cursor::from(pos));
        *self.latest_sel_index = match self.sels.binary_search_by(|sel| {
            if sel.end() <= pos {
                return Ordering::Less;
            }
            if sel.start() >= pos {
                return Ordering::Greater;
            }
            Ordering::Equal
        }) {
            Ok(index) => {
                self.sels[index] = sel;
                index
            }
            Err(index) => {
                self.sels.insert(index, sel);
                index
            }
        };
    }

    pub fn move_cursor_to(&mut self, select: bool, pos: BiasedTextPos) {
        let latest_sel = &mut self.sels[*self.latest_sel_index];
        latest_sel.cursor.pos = pos;
        if !select {
            latest_sel.anchor = pos;
        }
        while *self.latest_sel_index > 0 {
            let previous_sel_index = *self.latest_sel_index - 1;
            let previous_sel = self.sels[previous_sel_index];
            let latest_sel = self.sels[*self.latest_sel_index];
            if previous_sel.try_merge(latest_sel).is_some() {
                self.sels.remove(previous_sel_index);
                *self.latest_sel_index -= 1;
            } else {
                break;
            }
        }
        while *self.latest_sel_index + 1 < self.sels.len() {
            let next_sel_index = *self.latest_sel_index + 1;
            let latest_sel = self.sels[*self.latest_sel_index];
            let next_sel = self.sels[next_sel_index];
            if latest_sel.try_merge(next_sel).is_some() {
                self.sels.remove(next_sel_index);
            } else {
                break;
            }
        }
    }

    pub fn move_cursors_left(&mut self, select: bool) {
        use crate::{move_ops, Cursor};

        self.modify_sels(select, |view, sel| {
            sel.update_cursor(|cursor| Cursor {
                pos: BiasedTextPos {
                    pos: move_ops::move_left(view.text().as_lines(), cursor.pos.pos),
                    bias: Bias::Before,
                },
                col: None,
            })
        });
    }

    pub fn move_cursors_right(&mut self, select: bool) {
        use crate::{move_ops, Cursor};

        self.modify_sels(select, |view, sel| {
            sel.update_cursor(|cursor| Cursor {
                pos: BiasedTextPos {
                    pos: move_ops::move_right(view.text().as_lines(), cursor.pos.pos),
                    bias: Bias::After,
                },
                col: None,
            })
        });
    }

    pub fn move_cursors_up(&mut self, select: bool) {
        use crate::move_ops;

        let tab_width = self.settings.tab_width;
        self.modify_sels(select, |document, sel| {
            sel.update_cursor(|cursor| move_ops::move_up(document, cursor, tab_width))
        });
    }

    pub fn move_cursors_down(&mut self, select: bool) {
        use crate::move_ops;

        let tab_width = self.settings.tab_width;
        self.modify_sels(select, |document, sel| {
            sel.update_cursor(|cursor| move_ops::move_down(document, cursor, tab_width))
        });
    }

    pub fn update_summed_heights(&mut self) {
        use std::mem;

        let start = self.summed_heights.len();
        let mut summed_height = if start == 0 {
            0.0
        } else {
            self.summed_heights[start - 1]
        };
        let mut summed_heights = mem::take(self.summed_heights);
        for element in self
            .document()
            .elements(start, self.document().line_count())
        {
            match element {
                view::Element::Line(false, line) => {
                    summed_height += line.height();
                    summed_heights.push(summed_height);
                }
                view::Element::Line(true, line) => {
                    summed_height += line.height();
                }
                view::Element::Widget(_, widget) => {
                    summed_height += widget.height;
                }
            }
        }
        *self.summed_heights = summed_heights;
    }

    pub fn fold_line(&mut self, line_index: usize, fold_col: usize) {
        self.fold_col[line_index] = fold_col;
        self.unfolding_lines.remove(&line_index);
        self.folding_lines.insert(line_index);
    }

    pub fn unfold_line(&mut self, line_index: usize) {
        self.folding_lines.remove(&line_index);
        self.unfolding_lines.insert(line_index);
    }

    pub fn update_fold_animations(&mut self) -> bool {
        use std::mem;

        if self.folding_lines.is_empty() && self.unfolding_lines.is_empty() {
            return false;
        }
        let folding_lines = mem::take(self.folding_lines);
        let mut new_folding_lines = HashSet::new();
        for line in folding_lines {
            self.scale[line] *= 0.9;
            if self.scale[line] < 0.001 {
                self.scale[line] = 0.0;
            } else {
                new_folding_lines.insert(line);
            }
            self.summed_heights.truncate(line);
        }
        *self.folding_lines = new_folding_lines;
        let unfolding_lines = mem::take(self.unfolding_lines);
        let mut new_unfolding_lines = HashSet::new();
        for line in unfolding_lines {
            self.scale[line] = 1.0 - 0.9 * (1.0 - self.scale[line]);
            if self.scale[line] > 1.0 - 0.001 {
                self.scale[line] = 1.0;
            } else {
                new_unfolding_lines.insert(line);
            }
            self.summed_heights.truncate(line);
        }
        *self.unfolding_lines = new_unfolding_lines;
        self.update_summed_heights();
        true
    }

    fn wrap_lines(&mut self) {
        use crate::str::StrExt;

        for line in 0..self.document().line_count() {
            let old_wrap_byte_count = self.soft_breaks[line].len();
            self.soft_breaks[line].clear();
            self.start_col_after_wrap[line] = 0;
            if let Some(&max_col) = self.max_col.as_ref() {
                let mut byte = 0;
                let mut col = 0;
                let document = self.document();
                let line_ref = document.line(line);
                let mut start_col_after_wrap = line_ref
                    .text()
                    .indentation()
                    .col_count(document.settings().tab_width);
                for element in line_ref.inline_elements() {
                    match element {
                        line::Element::Token(_, token) => {
                            for string in token.text.split_whitespace_boundaries() {
                                if start_col_after_wrap
                                    + string.col_count(document.settings().tab_width)
                                    > max_col
                                {
                                    start_col_after_wrap = 0;
                                }
                            }
                        }
                        line::Element::Widget(_, widget) => {
                            if start_col_after_wrap + widget.col_count > max_col {
                                start_col_after_wrap = 0;
                            }
                        }
                    }
                }
                let mut soft_breaks = Vec::new();
                for element in line_ref.inline_elements() {
                    match element {
                        line::Element::Token(_, token) => {
                            for string in token.text.split_whitespace_boundaries() {
                                let mut next_col = col
                                    + string.col_count(document.settings().tab_width);
                                if next_col > max_col {
                                    next_col = start_col_after_wrap;
                                    soft_breaks.push(byte);
                                }
                                byte += string.len();
                                col = next_col;
                            }
                        }
                        line::Element::Widget(_, widget) => {
                            let mut next_col = col + widget.col_count;
                            if next_col > max_col {
                                next_col = start_col_after_wrap;
                                soft_breaks.push(byte);
                            }
                            col = next_col;
                        }
                    }
                }
                self.soft_breaks[line] = soft_breaks;
                self.start_col_after_wrap[line] = start_col_after_wrap;
            }
            if self.soft_breaks[line].len() != old_wrap_byte_count {
                self.summed_heights.truncate(line);
            }
        }
        self.update_summed_heights();
    }

    fn modify_sels(
        &mut self,
        select: bool,
        mut f: impl FnMut(&View<'_>, Sel) -> Sel,
    ) {
        use std::mem;

        let mut sels = mem::take(self.sels);
        let document = self.document();
        for sel in &mut sels {
            *sel = f(&document, *sel);
            if !select {
                *sel = sel.reset_anchor();
            }
        }
        *self.sels = sels;
        let mut current_sel_index = 0;
        while current_sel_index + 1 < self.sels.len() {
            let next_sel_index = current_sel_index + 1;
            let current_sel = self.sels[current_sel_index];
            let next_sel = self.sels[next_sel_index];
            assert!(current_sel.start() <= next_sel.start());
            if let Some(merged_sel) = current_sel.try_merge(next_sel) {
                self.sels[current_sel_index] = merged_sel;
                self.sels.remove(next_sel_index);
                if next_sel_index < *self.latest_sel_index {
                    *self.latest_sel_index -= 1;
                }
            } else {
                current_sel_index += 1;
            }
        }
    }

    fn modify_text(&mut self, mut f: impl FnMut(&mut Text, TextRange) -> TextDiff) {
        use crate::{text_pos::ApplyDiffMode, Cursor};

        let mut composite_diff = TextDiff::new();
        let mut prev_end = TextPos::default();
        let mut diffed_prev_end = TextPos::default();
        for sel in &mut *self.sels {
            let distance_from_prev_end = sel.start().pos - prev_end;
            let diffed_start = diffed_prev_end + distance_from_prev_end;
            let diffed_end = diffed_start + sel.len();
            let diff = f(&mut self.text, TextRange::new(diffed_start, diffed_end));
            let diffed_start = diffed_start.apply_diff(&diff, ApplyDiffMode::InsertBefore);
            let diffed_end = diffed_end.apply_diff(&diff, ApplyDiffMode::InsertBefore);
            self.text.apply_diff(diff.clone());
            composite_diff = composite_diff.compose(diff);
            prev_end = sel.end().pos;
            diffed_prev_end = diffed_end;
            let anchor_pos;
            let cursor_pos;
            if sel.anchor <= sel.cursor.pos {
                anchor_pos = BiasedTextPos {
                    pos: diffed_start,
                    bias: sel.start().bias,
                };
                cursor_pos = BiasedTextPos {
                    pos: diffed_end,
                    bias: sel.end().bias,
                };
            } else {
                anchor_pos = BiasedTextPos {
                    pos: diffed_end,
                    bias: sel.end().bias,
                };
                cursor_pos = BiasedTextPos {
                    pos: diffed_start,
                    bias: sel.start().bias,
                };
            }
            *sel = Sel {
                anchor: anchor_pos,
                cursor: Cursor {
                    pos: cursor_pos,
                    col: None,
                },
            };
        }
        self.update_after_modify_text(composite_diff);
    }

    fn update_after_modify_text(&mut self, diff: TextDiff) {
        use crate::text_diff::OpInfo;

        let mut line = 0;
        for operation in &diff {
            match operation.info() {
                OpInfo::Delete(length) => {
                    let start_line = line;
                    let end_line = start_line + length.lines;
                    self.inline_text_inlays.drain(start_line..end_line);
                    self.inline_widget_inlays.drain(start_line..end_line);
                    self.soft_breaks.drain(start_line..end_line);
                    self.start_col_after_wrap.drain(start_line..end_line);
                    self.fold_col.drain(start_line..end_line);
                    self.scale.drain(start_line..end_line);
                    self.summed_heights.truncate(line);
                }
                OpInfo::Retain(length) => {
                    line += length.lines;
                }
                OpInfo::Insert(length) => {
                    let next_line = line + 1;
                    let line_count = length.lines;
                    self.inline_text_inlays
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.inline_widget_inlays
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.soft_breaks
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.start_col_after_wrap
                        .splice(next_line..next_line, (0..line_count).map(|_| 0));
                    self.fold_col
                        .splice(next_line..next_line, (0..line_count).map(|_| 0));
                    self.scale
                        .splice(next_line..next_line, (0..line_count).map(|_| 1.0));
                    self.summed_heights.truncate(line);
                    line += line_count;
                }
            }
        }
        self.tokenizer.retokenize(&diff, &self.text);
        self.update_summed_heights();
    }
}
