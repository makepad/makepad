use {
    crate::{
        document, document::LineInlay, line, Affinity, Diff, Document, Position, Range, Selection,
        Settings, Text, Tokenizer,
    },
    std::collections::HashSet,
};

#[derive(Debug, PartialEq)]
pub struct Context<'a> {
    settings: &'a mut Settings,
    text: &'a mut Text,
    tokenizer: &'a mut Tokenizer,
    text_inlays: &'a mut Vec<Vec<(usize, String)>>,
    line_widget_inlays: &'a mut Vec<Vec<((usize, Affinity), line::Widget)>>,
    wrap_bytes: &'a mut Vec<Vec<usize>>,
    fold_column: &'a mut Vec<usize>,
    scale: &'a mut Vec<f64>,
    line_inlays: &'a mut Vec<(usize, LineInlay)>,
    document_widget_inlays: &'a mut Vec<((usize, Affinity), document::Widget)>,
    summed_heights: &'a mut Vec<f64>,
    selections: &'a mut Vec<Selection>,
    latest_selection_index: &'a mut usize,
    folding_lines: &'a mut HashSet<usize>,
    unfolding_lines: &'a mut HashSet<usize>,
}

impl<'a> Context<'a> {
    pub fn new(
        settings: &'a mut Settings,
        text: &'a mut Text,
        tokenizer: &'a mut Tokenizer,
        text_inlays: &'a mut Vec<Vec<(usize, String)>>,
        line_widget_inlays: &'a mut Vec<Vec<((usize, Affinity), line::Widget)>>,
        wrap_bytes: &'a mut Vec<Vec<usize>>,
        fold_column: &'a mut Vec<usize>,
        scale: &'a mut Vec<f64>,
        line_inlays: &'a mut Vec<(usize, LineInlay)>,
        document_widget_inlays: &'a mut Vec<((usize, Affinity), document::Widget)>,
        summed_heights: &'a mut Vec<f64>,
        selections: &'a mut Vec<Selection>,
        latest_selection_index: &'a mut usize,
        folding_lines: &'a mut HashSet<usize>,
        unfolding_lines: &'a mut HashSet<usize>,
    ) -> Self {
        Self {
            settings,
            text,
            tokenizer,
            text_inlays,
            line_widget_inlays,
            wrap_bytes,
            fold_column,
            scale,
            line_inlays,
            document_widget_inlays,
            summed_heights,
            selections,
            latest_selection_index,
            folding_lines,
            unfolding_lines,
        }
    }

    pub fn document(&self) -> Document<'_> {
        Document::new(
            self.settings,
            self.text,
            self.tokenizer,
            self.text_inlays,
            self.line_widget_inlays,
            self.wrap_bytes,
            self.fold_column,
            self.scale,
            self.line_inlays,
            self.document_widget_inlays,
            self.summed_heights,
            self.selections,
            *self.latest_selection_index,
        )
    }

    pub fn wrap_lines(&mut self, max_column: usize) {
        use {crate::str::StrExt, std::mem};

        for line in 0..self.document().line_count() {
            let old_wrap_byte_count = self.wrap_bytes[line].len();
            self.wrap_bytes[line].clear();
            let mut wrap_bytes = mem::take(&mut self.wrap_bytes[line]);
            let mut byte = 0;
            let mut column = 0;
            let document = self.document();
            for element in document.line(line).elements() {
                match element {
                    line::Element::Token(_, token) => {
                        for string in token.text.split_whitespace_boundaries() {
                            let mut next_column =
                                column + string.column_count(document.settings().tab_column_count);
                            if next_column > max_column {
                                next_column = 0;
                                wrap_bytes.push(byte);
                            }
                            byte += string.len();
                            column = next_column;
                        }
                    }
                    line::Element::Widget(_, widget) => {
                        let mut next_column = column + widget.column_count;
                        if next_column > max_column {
                            next_column = 0;
                            wrap_bytes.push(byte);
                        }
                        column = next_column;
                    }
                }
            }
            self.wrap_bytes[line] = wrap_bytes;
            if self.wrap_bytes[line].len() != old_wrap_byte_count {
                self.summed_heights.truncate(line);
            }
        }
        self.update_summed_heights();
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

    pub fn set_cursor(&mut self, cursor: (Position, Affinity)) {
        self.selections.clear();
        self.selections.push(Selection::from_cursor(cursor));
    }

    pub fn insert_cursor(&mut self, cursor: (Position, Affinity)) {
        use std::cmp::Ordering;

        let selection = Selection::from_cursor(cursor);
        *self.latest_selection_index = match self.selections.binary_search_by(|selection| {
            if selection.end() <= cursor {
                return Ordering::Less;
            }
            if selection.start() >= cursor {
                return Ordering::Greater;
            }
            Ordering::Equal
        }) {
            Ok(index) => {
                self.selections[index] = selection;
                index
            }
            Err(index) => {
                self.selections.insert(index, selection);
                index
            }
        };
    }

    pub fn move_cursor_to(&mut self, select: bool, cursor: (Position, Affinity)) {
        let latest_selection = &mut self.selections[*self.latest_selection_index];
        latest_selection.cursor = cursor;
        if !select {
            latest_selection.anchor = cursor;
        }
        while *self.latest_selection_index > 0 {
            let previous_selection_index = *self.latest_selection_index - 1;
            let previous_selection = self.selections[previous_selection_index];
            let latest_selection = self.selections[*self.latest_selection_index];
            if previous_selection.should_merge(latest_selection) {
                self.selections.remove(previous_selection_index);
                *self.latest_selection_index -= 1;
            } else {
                break;
            }
        }
        while *self.latest_selection_index + 1 < self.selections.len() {
            let next_selection_index = *self.latest_selection_index + 1;
            let latest_selection = self.selections[*self.latest_selection_index];
            let next_selection = self.selections[next_selection_index];
            if latest_selection.should_merge(next_selection) {
                self.selections.remove(next_selection_index);
            } else {
                break;
            }
        }
    }

    pub fn move_cursors_left(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|(position, _), _| move_ops::move_left(document, position))
        });
    }

    pub fn move_cursors_right(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|(position, _), _| move_ops::move_right(document, position))
        });
    }

    pub fn move_cursors_up(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|cursor, column| move_ops::move_up(document, cursor, column))
        });
    }

    pub fn move_cursors_down(&mut self, select: bool) {
        use crate::move_ops;

        self.modify_selections(select, |document, selection| {
            selection.update_cursor(|cursor, column| move_ops::move_down(document, cursor, column))
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
                document::Element::Line(false, line) => {
                    summed_height += line.height();
                    summed_heights.push(summed_height);
                }
                document::Element::Line(true, line) => {
                    summed_height += line.height();
                }
                document::Element::Widget(_, widget) => {
                    summed_height += widget.height;
                }
            }
        }
        *self.summed_heights = summed_heights;
    }

    pub fn fold_line(&mut self, line_index: usize) {
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

    fn modify_selections(
        &mut self,
        select: bool,
        mut f: impl FnMut(&Document<'_>, Selection) -> Selection,
    ) {
        use std::mem;

        let mut selections = mem::take(self.selections);
        let document = self.document();
        for selection in &mut selections {
            *selection = f(&document, *selection);
            if !select {
                *selection = selection.reset_anchor();
            }
        }
        *self.selections = selections;
    }

    fn modify_text(&mut self, mut f: impl FnMut(&mut Text, Range) -> Diff) {
        use crate::diff::Strategy;

        let mut composite_diff = Diff::new();
        let mut prev_end = Position::default();
        let mut diffed_prev_end = Position::default();
        for selection in &mut *self.selections {
            let distance_from_prev_end = selection.start().0 - prev_end;
            let diffed_start = diffed_prev_end + distance_from_prev_end;
            let diffed_end = diffed_start + selection.length();
            let diff = f(&mut self.text, Range::new(diffed_start, diffed_end));
            let diffed_start = diffed_start.apply_diff(&diff, Strategy::InsertBefore);
            let diffed_end = diffed_end.apply_diff(&diff, Strategy::InsertBefore);
            self.text.apply_diff(diff.clone());
            composite_diff = composite_diff.compose(diff);
            prev_end = selection.end().0;
            diffed_prev_end = diffed_end;
            *selection = if selection.anchor <= selection.cursor {
                Selection::new(
                    (diffed_start, selection.start().1),
                    (diffed_end, selection.end().1),
                    selection.preferred_column,
                )
            } else {
                Selection::new(
                    (diffed_end, selection.end().1),
                    (diffed_start, selection.start().1),
                    selection.preferred_column,
                )
            };
        }
        self.update_after_modify_text(composite_diff);
    }

    fn update_after_modify_text(&mut self, diff: Diff) {
        use crate::diff::OperationInfo;

        let mut line = 0;
        for operation in &diff {
            match operation.info() {
                OperationInfo::Delete(length) => {
                    let start_line = line;
                    let end_line = start_line + length.line_count;
                    self.text_inlays.drain(start_line..end_line);
                    self.line_widget_inlays.drain(start_line..end_line);
                    self.wrap_bytes.drain(start_line..end_line);
                    self.fold_column.drain(start_line..end_line);
                    self.scale.drain(start_line..end_line);
                    self.summed_heights.truncate(line);
                }
                OperationInfo::Retain(length) => {
                    line += length.line_count;
                }
                OperationInfo::Insert(length) => {
                    let next_line = line + 1;
                    let line_count = length.line_count;
                    self.text_inlays
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.line_widget_inlays
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.wrap_bytes
                        .splice(next_line..next_line, (0..line_count).map(|_| Vec::new()));
                    self.fold_column
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
