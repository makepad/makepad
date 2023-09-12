use {
    crate::{
        char::CharExt,
        history::{EditKind, History},
        inlays::{BlockInlay, InlineInlay},
        iter::IteratorExt,
        selection::SelectionSet,
        settings::Settings,
        state::SessionId,
        str::StrExt,
        text::{Change, Drift, Edit, Length, Position, Text},
        token::{Token, TokenKind},
        tokenizer::Tokenizer,
    },
    std::{
        cell::{Ref, RefCell},
        cmp::Ordering,
        collections::HashMap,
        iter,
        ops::Range,
        rc::Rc,
        sync::mpsc::Sender,
    },
};

#[derive(Clone, Debug)]
pub struct Document(Rc<DocumentInner>);

impl Document {
    pub fn new(text: Text) -> Self {
        let line_count = text.as_lines().len();
        let tokens: Vec<_> = (0..line_count)
            .map(|line| tokenize(&text.as_lines()[line]).collect::<Vec<_>>())
            .collect();
        let session = Self(Rc::new(DocumentInner {
            history: RefCell::new(History::from(text)),
            layout: RefCell::new(DocumentLayout {
                tokens,
                inline_inlays: (0..line_count).map(|_| Vec::new()).collect(),
                block_inlays: Vec::new(),
            }),
            tokenizer: RefCell::new(Tokenizer::new(line_count)),
            edit_senders: RefCell::new(HashMap::new()),
        }));
        session.0.tokenizer.borrow_mut().update(
            &session.0.history.borrow().as_text(),
            &mut session.0.layout.borrow_mut().tokens,
        );
        session
    }

    pub fn as_text(&self) -> Ref<'_, Text> {
        Ref::map(self.0.history.borrow(), |history| history.as_text())
    }

    pub fn layout(&self) -> Ref<'_, DocumentLayout> {
        self.0.layout.borrow()
    }

    pub fn edit_selections(
        &self,
        session_id: SessionId,
        kind: EditKind,
        selections: &SelectionSet,
        settings: &Settings,
        mut f: impl FnMut(Editor<'_>, Position, Length),
    ) {
        let mut history = self.0.history.borrow_mut();
        history.push_or_extend_group(session_id, kind, selections);
        let mut edits = Vec::new();
        let mut line_ranges = Vec::new();
        let mut prev_start = Position::zero();
        let mut prev_adjusted_start = Position::zero();
        let mut prev_edit_start = 0;
        for &selection in selections {
            let mut adjusted_start = prev_adjusted_start + (selection.start() - prev_start);
            for edit in &edits[prev_edit_start..] {
                adjusted_start = adjusted_start.apply_edit(edit);
            }
            let edit_start = edits.len();
            f(
                Editor {
                    history: &mut *history,
                    edits: &mut edits,
                },
                adjusted_start,
                selection.length(),
            );
            for edit in &edits[edit_start..] {
                match edit.change {
                    Change::Insert(position, ref text) if text.as_lines().len() > 1 => {
                        line_ranges.push(Range {
                            start: if history.as_text().as_lines()[position.line_index]
                                [..position.byte_index]
                                .chars()
                                .all(|char| char.is_whitespace())
                            {
                                position.line_index
                            } else {
                                position.line_index + 1
                            },
                            end: position.line_index + text.as_lines().len(),
                        });
                    }
                    _ => {}
                }
            }
            prev_start = selection.start();
            prev_adjusted_start = adjusted_start;
            prev_edit_start = edit_start;
        }
        drop(history);
        self.autoindent(
            &line_ranges,
            settings.use_soft_tabs,
            settings.tab_column_count,
            settings.indent_column_count,
            &mut edits,
        );
        self.apply_edits(session_id, None, &edits);
    }

    pub fn edit_lines(
        &self,
        origin_id: SessionId,
        kind: EditKind,
        selections: &SelectionSet,
        mut f: impl FnMut(&str) -> (usize, usize, String),
    ) {
        let mut edits = Vec::new();
        self.0
            .history
            .borrow_mut()
            .push_or_extend_group(origin_id, kind, selections);
        for line_range in selections
            .iter()
            .copied()
            .map(|selection| selection.line_range())
            .merge(|line_range_0, line_range_1| {
                if line_range_0.end >= line_range_1.start {
                    Ok(line_range_0.start..line_range_1.end)
                } else {
                    Err((line_range_0, line_range_1))
                }
            })
        {
            for line in line_range {
                self.edit_lines_internal(line, &mut edits, &mut f);
            }
        }
        self.apply_edits(origin_id, None, &edits);
    }

    pub fn add_session(
        &mut self,
        session_id: SessionId,
        edit_sender: Sender<(Option<SelectionSet>, Vec<Edit>)>,
    ) {
        self.0
            .edit_senders
            .borrow_mut()
            .insert(session_id, edit_sender);
    }

    pub fn remove_session(&mut self, session_id: SessionId) {
        self.0.edit_senders.borrow_mut().remove(&session_id);
    }

    fn autoindent(
        &self,
        line_ranges: &[Range<usize>],
        use_soft_tabs: bool,
        tab_column_count: usize,
        indent_column_count: usize,
        edits: &mut Vec<Edit>,
    ) {
        fn next_line_indentation_column_count(
            line: &str,
            tab_column_count: usize,
            indent_column_count: usize,
        ) -> Option<usize> {
            if let Some(indentation) = line.leading_whitespace() {
                let mut indentation_column_count = indentation.column_count(tab_column_count);
                if line
                    .chars()
                    .rev()
                    .find_map(|char| {
                        if char.is_opening_delimiter() {
                            return Some(true);
                        }
                        if char.is_closing_delimiter() {
                            return Some(false);
                        }
                        None
                    })
                    .unwrap_or(false)
                {
                    indentation_column_count += indent_column_count;
                };
                Some(indentation_column_count)
            } else {
                None
            }
        }

        for line_range in line_ranges
            .iter()
            .cloned()
            .merge(|line_range_0, line_range_1| {
                if line_range_0.end >= line_range_1.start {
                    Ok(line_range_0.start..line_range_1.end)
                } else {
                    Err((line_range_0, line_range_1))
                }
            })
        {
            let mut desired_indentation_column_count = self.as_text().as_lines()
                [..line_range.start]
                .iter()
                .rev()
                .find_map(|line| {
                    next_line_indentation_column_count(line, tab_column_count, indent_column_count)
                })
                .unwrap_or(0);
            for line in line_range {
                if self.as_text().as_lines()[line]
                    .chars()
                    .find_map(|char| {
                        if char.is_closing_delimiter() {
                            return Some(true);
                        }
                        if !char.is_whitespace() {
                            return Some(false);
                        }
                        None
                    })
                    .unwrap_or(false)
                {
                    desired_indentation_column_count -= 4;
                }
                self.edit_lines_internal(line, edits, |line| {
                    crate::state::reindent(line, use_soft_tabs, tab_column_count, |_| {
                        desired_indentation_column_count
                    })
                });
                if let Some(next_line_indentation_column_count) = next_line_indentation_column_count(
                    &self.as_text().as_lines()[line],
                    tab_column_count,
                    indent_column_count,
                ) {
                    desired_indentation_column_count = next_line_indentation_column_count;
                }
            }
        }
    }

    fn edit_lines_internal(
        &self,
        line: usize,
        edits: &mut Vec<Edit>,
        mut f: impl FnMut(&str) -> (usize, usize, String),
    ) {
        let mut history = self.0.history.borrow_mut();
        let (byte, delete_byte_count, insert_text) = f(&history.as_text().as_lines()[line]);
        if delete_byte_count > 0 {
            let edit = Edit {
                change: Change::Delete(
                    Position {
                        line_index: line,
                        byte_index: byte,
                    },
                    Length {
                        line_count: 0,
                        byte_count: delete_byte_count,
                    },
                ),
                drift: Drift::Before,
            };
            edits.push(edit.clone());
            history.apply_edit(edit);
        }
        if !insert_text.is_empty() {
            let edit = Edit {
                change: Change::Insert(
                    Position {
                        line_index: line,
                        byte_index: byte,
                    },
                    insert_text.into(),
                ),
                drift: Drift::Before,
            };
            edits.push(edit.clone());
            history.apply_edit(edit);
        }
    }

    pub fn force_new_group(&mut self) {
        self.0.history.borrow_mut().force_new_group()
    }

    pub fn undo(&mut self, origin_id: SessionId, selections: &SelectionSet) -> bool {
        let mut changes = Vec::new();
        let selections = self.0.history.borrow_mut().undo(selections, &mut changes);
        if let Some(selections) = selections {
            self.apply_edits(origin_id, Some(selections), &changes);
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self, origin_id: SessionId, selections: &SelectionSet) -> bool {
        let mut changes = Vec::new();
        let selections = self.0.history.borrow_mut().redo(selections, &mut changes);
        if let Some(selections) = selections {
            self.apply_edits(origin_id, Some(selections), &changes);
            true
        } else {
            false
        }
    }

    fn apply_edits(&self, origin_id: SessionId, selections: Option<SelectionSet>, edits: &[Edit]) {
        for edit in edits {
            self.apply_change_to_tokens(&edit.change);
            self.apply_change_to_inline_inlays(&edit.change, edit.drift);
            self.0.tokenizer.borrow_mut().apply_change(&edit.change);
        }
        self.0.tokenizer.borrow_mut().update(
            self.0.history.borrow().as_text(),
            &mut self.0.layout.borrow_mut().tokens,
        );
        for (&session_id, edit_sender) in &*self.0.edit_senders.borrow() {
            if session_id == origin_id {
                edit_sender
                    .send((selections.clone(), edits.to_vec()))
                    .unwrap();
            } else {
                edit_sender
                    .send((
                        None,
                        edits
                            .iter()
                            .cloned()
                            .map(|edit| Edit {
                                change: edit.change,
                                drift: Drift::Before,
                            })
                            .collect(),
                    ))
                    .unwrap();
            }
        }
    }

    fn apply_change_to_tokens(&self, change: &Change) {
        let mut layout = self.0.layout.borrow_mut();
        let tokens = &mut layout.tokens;
        match *change {
            Change::Insert(point, ref text) => {
                let mut byte = 0;
                let mut index = tokens[point.line_index]
                    .iter()
                    .position(|token| {
                        if byte + token.len > point.byte_index {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(tokens[point.line_index].len());
                if byte != point.byte_index {
                    let token = tokens[point.line_index][index];
                    let mid = point.byte_index - byte;
                    tokens[point.line_index][index] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    index += 1;
                    tokens[point.line_index].insert(
                        index,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                if text.length().line_count == 0 {
                    tokens[point.line_index]
                        .splice(index..index, tokenize(text.as_lines().first().unwrap()));
                } else {
                    let mut new_tokens = (0..text.as_lines().len())
                        .map(|line| tokenize(&text.as_lines()[line]).collect::<Vec<_>>())
                        .collect::<Vec<_>>();
                    new_tokens
                        .first_mut()
                        .unwrap()
                        .splice(..0, tokens[point.line_index][..index].iter().copied());
                    new_tokens
                        .last_mut()
                        .unwrap()
                        .splice(..0, tokens[point.line_index][index..].iter().copied());
                    tokens.splice(point.line_index..point.line_index + 1, new_tokens);
                }
            }
            Change::Delete(start, length) => {
                let end = start + length;
                let mut byte = 0;
                let mut start_token = tokens[start.line_index]
                    .iter()
                    .position(|token| {
                        if byte + token.len > start.byte_index {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(tokens[start.line_index].len());
                if byte != start.byte_index {
                    let token = tokens[start.line_index][start_token];
                    let mid = start.byte_index - byte;
                    tokens[start.line_index][start_token] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    start_token += 1;
                    tokens[start.line_index].insert(
                        start_token,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                let mut byte = 0;
                let mut end_token = tokens[end.line_index]
                    .iter()
                    .position(|token| {
                        if byte + token.len > end.byte_index {
                            return true;
                        }
                        byte += token.len;
                        false
                    })
                    .unwrap_or(tokens[end.line_index].len());
                if byte != end.byte_index {
                    let token = tokens[end.line_index][end_token];
                    let mid = end.byte_index - byte;
                    tokens[end.line_index][end_token] = Token {
                        len: mid,
                        kind: token.kind,
                    };
                    end_token += 1;
                    tokens[end.line_index].insert(
                        end_token,
                        Token {
                            len: token.len - mid,
                            kind: token.kind,
                        },
                    );
                }
                if length.line_count == 0 {
                    tokens[start.line_index].drain(start_token..end_token);
                } else {
                    let mut new_tokens = tokens[start.line_index][..start_token]
                        .iter()
                        .copied()
                        .collect::<Vec<_>>();
                    new_tokens.extend(tokens[end.line_index][end_token..].iter().copied());
                    tokens.splice(start.line_index..end.line_index + 1, iter::once(new_tokens));
                }
            }
        }
    }

    fn apply_change_to_inline_inlays(&self, change: &Change, drift: Drift) {
        let mut layout = self.0.layout.borrow_mut();
        let inline_inlays = &mut layout.inline_inlays;
        match *change {
            Change::Insert(point, ref text) => {
                let index = inline_inlays[point.line_index]
                    .iter()
                    .position(|(byte, _)| match byte.cmp(&point.byte_index) {
                        Ordering::Less => false,
                        Ordering::Equal => match drift {
                            Drift::Before => true,
                            Drift::After => false,
                        },
                        Ordering::Greater => true,
                    })
                    .unwrap_or(inline_inlays[point.line_index].len());
                if text.length().line_count == 0 {
                    for (byte, _) in &mut inline_inlays[point.line_index][index..] {
                        *byte += text.length().byte_count;
                    }
                } else {
                    let mut new_inline_inlays = (0..text.as_lines().len())
                        .map(|_| Vec::new())
                        .collect::<Vec<_>>();
                    new_inline_inlays
                        .first_mut()
                        .unwrap()
                        .splice(..0, inline_inlays[point.line_index].drain(..index));
                    new_inline_inlays.last_mut().unwrap().splice(
                        ..0,
                        inline_inlays[point.line_index]
                            .drain(..)
                            .map(|(byte, inline_inlay)| {
                                (byte + text.length().byte_count, inline_inlay)
                            }),
                    );
                    inline_inlays.splice(point.line_index..point.line_index + 1, new_inline_inlays);
                }
            }
            Change::Delete(start, length) => {
                let end = start + length;
                let start_inlay = inline_inlays[start.line_index]
                    .iter()
                    .position(|&(byte, _)| byte >= start.byte_index)
                    .unwrap_or(inline_inlays[start.line_index].len());
                let end_inlay = inline_inlays[end.line_index]
                    .iter()
                    .position(|&(byte, _)| byte >= end.byte_index)
                    .unwrap_or(inline_inlays[end.line_index].len());
                if length.line_count == 0 {
                    inline_inlays[start.line_index].drain(start_inlay..end_inlay);
                    for (byte, _) in &mut inline_inlays[start.line_index][start_inlay..] {
                        *byte = start.byte_index + (*byte - end.byte_index.min(*byte));
                    }
                } else {
                    let mut new_inline_inlays = inline_inlays[start.line_index]
                        .drain(..start_inlay)
                        .collect::<Vec<_>>();
                    new_inline_inlays.extend(inline_inlays[end.line_index].drain(end_inlay..).map(
                        |(byte, inline_inlay)| {
                            (
                                start.byte_index + byte - end.byte_index.min(byte),
                                inline_inlay,
                            )
                        },
                    ));
                    inline_inlays.splice(
                        start.line_index..end.line_index + 1,
                        iter::once(new_inline_inlays),
                    );
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct DocumentLayout {
    pub tokens: Vec<Vec<Token>>,
    pub inline_inlays: Vec<Vec<(usize, InlineInlay)>>,
    pub block_inlays: Vec<(usize, BlockInlay)>,
}

#[derive(Debug)]
pub struct Editor<'a> {
    history: &'a mut History,
    edits: &'a mut Vec<Edit>,
}

impl<'a> Editor<'a> {
    pub fn as_text(&mut self) -> &Text {
        self.history.as_text()
    }

    pub fn apply_edit(&mut self, edit: Edit) {
        self.history.apply_edit(edit.clone());
        self.edits.push(edit);
    }
}

#[derive(Debug)]
struct DocumentInner {
    history: RefCell<History>,
    layout: RefCell<DocumentLayout>,
    tokenizer: RefCell<Tokenizer>,
    edit_senders: RefCell<HashMap<SessionId, Sender<(Option<SelectionSet>, Vec<Edit>)>>>,
}

fn tokenize(text: &str) -> impl Iterator<Item = Token> + '_ {
    text.split_whitespace_boundaries().map(|string| Token {
        len: string.len(),
        kind: if string.chars().next().unwrap().is_whitespace() {
            TokenKind::Whitespace
        } else {
            TokenKind::Unknown
        },
    })
}
