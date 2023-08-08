use {
    crate::{
        arena::Id,
        change::{ChangeKind, Drift},
        char::CharExt,
        edit_ops,
        inlays::{BlockInlay, InlineInlay},
        line_ref::Wrapped,
        selection::Affinity,
        widgets::BlockWidget,
        Arena, Change, LineRef, Point, Selection, Settings, Text, Token,
    },
    std::{
        cmp::Ordering,
        collections::{HashMap, HashSet},
        fs::File,
        io,
        io::BufReader,
        iter, mem,
        ops::Range,
        path::{Path, PathBuf},
        slice::Iter,
    },
};

#[derive(Clone, Debug, Default)]
pub struct State {
    settings: Settings,
    sessions: Arena<Session>,
    documents: Arena<Document>,
    document_ids_by_path: HashMap<PathBuf, DocumentId>,
}

impl State {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_settings(settings: Settings) -> Self {
        Self {
            settings,
            ..Self::default()
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn width(&self, session: SessionId) -> f64 {
        let mut width: f64 = 0.0;
        for line_ref in self.line_refs(session, 0..self.line_count(self.document(session))) {
            width = width.max(line_ref.width());
        }
        width
    }

    pub fn height(&self, session_id: SessionId) -> f64 {
        let last_line = self.line_count(self.document(session_id)) - 1;
        let last_line_ref = self.line_ref(session_id, last_line);
        let mut y = last_line_ref.y() + last_line_ref.height();
        for block_element in self.blocks(session_id, last_line..last_line) {
            match block_element {
                Block::LineRef {
                    is_inlay: true,
                    line_ref,
                } => y += line_ref.height(),
                Block::Widget(widget) => y += widget.height,
                _ => unreachable!(),
            }
        }
        y
    }

    pub fn document(&self, session: SessionId) -> DocumentId {
        self.sessions[session.0].document
    }

    pub fn max_column_count(&self, session: SessionId) -> usize {
        self.sessions[session.0].max_column_count
    }

    pub fn find_first_line_ending_after_y(&self, session: SessionId, y: f64) -> usize {
        match self.sessions[session.0]
            .y
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line) => line,
            Err(line) => line.saturating_sub(1),
        }
    }

    pub fn find_first_line_starting_after_y(&self, session: SessionId, y: f64) -> usize {
        match self.sessions[session.0]
            .y
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line) => line + 1,
            Err(line) => line,
        }
    }

    pub fn line_ref(&self, session: SessionId, line: usize) -> LineRef<'_> {
        let session_ref = &self.sessions[session.0];
        let document_ref = &self.documents[self.document(session).0];
        LineRef {
            y: session_ref.y.get(line).copied(),
            column_count: session_ref.column_count[line],
            fold_column: session_ref.fold_column[line],
            scale: session_ref.scale[line],
            indent_column_count: session_ref.indent_column_count[line],
            text: &document_ref.text.as_lines()[line],
            tokens: &document_ref.tokens[line],
            inline_inlays: &document_ref.inline_inlays[line],
            wraps: &session_ref.wraps[line],
        }
    }

    pub fn line_refs(&self, session: SessionId, line_range: Range<usize>) -> LineRefs<'_> {
        let session_ref = &self.sessions[session.0];
        let document_ref = &self.documents[self.document(session).0];
        LineRefs {
            y: session_ref.y[line_range.start.min(session_ref.y.len())
                ..line_range.end.min(session_ref.y.len())]
                .iter(),
            column_count: session_ref.column_count[line_range.start..line_range.end].iter(),
            fold_column_index: session_ref.fold_column[line_range.start..line_range.end].iter(),
            scale: session_ref.scale[line_range.start..line_range.end].iter(),
            indent_column_count: session_ref.indent_column_count[line_range.start..line_range.end]
                .iter(),
            text: document_ref.text.as_lines()[line_range.start..line_range.end].iter(),
            tokens: document_ref.tokens[line_range.start..line_range.end].iter(),
            inline_inlays_by_byte_index: document_ref.inline_inlays
                [line_range.start..line_range.end]
                .iter(),
            wrap_byte_indices: session_ref.wraps[line_range.start..line_range.end].iter(),
        }
    }

    pub fn blocks(&self, session: SessionId, line_range: Range<usize>) -> Blocks<'_> {
        let mut block_inlays = self.documents[self.document(session).0].block_inlays.iter();
        while block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(line_index, _)| line_index < line_range.start)
        {
            block_inlays.next();
        }
        Blocks {
            line_refs: self.line_refs(session, line_range.start..line_range.end),
            block_inlays,
            line: line_range.start,
        }
    }

    pub fn selections(&self, session: SessionId) -> &[Selection] {
        &self.sessions[session.0].selections
    }

    pub fn line_count(&self, document: DocumentId) -> usize {
        self.documents[document.0].text.as_lines().len()
    }

    pub fn new_file(&mut self, text: Text) -> SessionId {
        let document = self.create_document(None, text);
        self.create_session(document)
    }

    pub fn open_file(&mut self, path: impl AsRef<Path> + Into<PathBuf>) -> io::Result<SessionId> {
        let document = match self.document_ids_by_path.get(path.as_ref()) {
            Some(&document) => document,
            None => {
                let file = File::open(path.as_ref())?;
                self.create_document(
                    Some(path.into()),
                    Text::from_buf_reader(BufReader::new(file))?,
                )
            }
        };
        Ok(self.create_session(document))
    }

    pub fn close_file(&mut self, session: SessionId) {
        self.destroy_session(session);
    }

    pub fn set_max_column_count(&mut self, session: SessionId, max_column_count: usize) {
        let session_ref = &mut self.sessions[session.0];
        if session_ref.max_column_count == max_column_count {
            return;
        }
        session_ref.max_column_count = max_column_count;
        for line_index in 0..self.line_count(self.document(session)) {
            self.update_wrap_data(session, line_index);
        }
        self.update_y(session);
    }

    pub fn set_cursor(&mut self, session: SessionId, cursor: Point, affinity: Affinity) {
        let session_ref = &mut self.sessions[session.0];
        session_ref.selections.clear();
        session_ref.selections.push(Selection {
            anchor: cursor,
            cursor,
            affinity,
        });
        session_ref.pending_selection_index = Some(0);
    }

    pub fn add_cursor(&mut self, session: SessionId, cursor: Point, affinity: Affinity) {
        let session_ref = &mut self.sessions[session.0];
        let selection = Selection {
            anchor: cursor,
            cursor,
            affinity,
        };
        session_ref.pending_selection_index = Some(
            match session_ref.selections.binary_search_by(|selection| {
                if selection.end() <= cursor {
                    return Ordering::Less;
                }
                if selection.start() >= cursor {
                    return Ordering::Greater;
                }
                Ordering::Equal
            }) {
                Ok(index) => {
                    session_ref.selections[index] = selection;
                    index
                }
                Err(index) => {
                    session_ref.selections.insert(index, selection);
                    index
                }
            },
        );
    }

    pub fn move_to(&mut self, session: SessionId, cursor: Point, affinity: Affinity) {
        let session_ref = &mut self.sessions[session.0];
        let mut pending_selection_index = session_ref.pending_selection_index.unwrap();
        session_ref.selections[pending_selection_index].cursor = cursor;
        session_ref.selections[pending_selection_index].affinity = affinity;
        while pending_selection_index > 0 {
            let prev_selection_index = pending_selection_index - 1;
            if !session_ref.selections[prev_selection_index]
                .should_merge(session_ref.selections[pending_selection_index])
            {
                break;
            }
            session_ref.selections.remove(prev_selection_index);
            pending_selection_index -= 1;
        }
        while pending_selection_index + 1 < session_ref.selections.len() {
            let next_selection_index = pending_selection_index + 1;
            if !session_ref.selections[pending_selection_index]
                .should_merge(session_ref.selections[next_selection_index])
            {
                break;
            }
            session_ref.selections.remove(next_selection_index);
        }
        session_ref.pending_selection_index = Some(pending_selection_index);
    }

    pub fn insert(&mut self, session: SessionId, text: Text) {
        let mut changes = Vec::new();
        let document = self.document(session);
        edit_ops::insert(
            &mut self.documents[document.0].text,
            &self.sessions[session.0].selections,
            text,
            &mut changes,
        );
        for change in &changes {
            self.apply_change_to_document(document, change);
        }
    }

    fn apply_change_to_document(&mut self, document: DocumentId, change: &Change) {
        self.apply_change_to_inline_inlays(document, change);
    }

    fn apply_change_to_inline_inlays(&mut self, document: DocumentId, change: &Change) {
        let document_ref = &mut self.documents[document.0];
        match change.kind {
            ChangeKind::Insert(point, ref text) => {
                let index = document_ref.inline_inlays[point.line]
                    .iter()
                    .position(|(byte, _)| match byte.cmp(&point.byte) {
                        Ordering::Less => false,
                        Ordering::Equal => match change.drift {
                            Drift::Before => true,
                            Drift::After => false,
                        },
                        Ordering::Greater => true,
                    })
                    .unwrap_or(document_ref.inline_inlays[point.line].len());
                if text.extent().line_count == 0 {
                    for (byte, _) in &mut document_ref.inline_inlays[point.line][index..] {
                        *byte += text.extent().byte_count;
                    }
                } else {
                    let mut inline_inlays = (0..text.as_lines().len())
                        .map(|_| Vec::new())
                        .collect::<Vec<_>>();
                    inline_inlays
                        .first_mut()
                        .unwrap()
                        .splice(..0, document_ref.inline_inlays[point.line].drain(..index));
                    inline_inlays.last_mut().unwrap().splice(
                        ..0,
                        document_ref.inline_inlays[point.line].drain(..).map(
                            |(byte, inline_inlay)| (byte + text.extent().byte_count, inline_inlay),
                        ),
                    );
                    document_ref
                        .inline_inlays
                        .splice(point.line..point.line + 1, inline_inlays);
                }
            }
            ChangeKind::Delete(range) => {
                let start = document_ref.inline_inlays[range.start().line]
                    .iter()
                    .position(|&(byte, _)| byte >= range.start().byte)
                    .unwrap_or(document_ref.inline_inlays[range.start().line].len());
                let end = document_ref.inline_inlays[range.end().line]
                    .iter()
                    .position(|&(byte, _)| byte >= range.end().byte)
                    .unwrap_or(document_ref.inline_inlays[range.end().line].len());
                if range.start().line == range.end().line {
                    document_ref.inline_inlays.drain(start..end);
                    for (byte, _) in &mut document_ref.inline_inlays[range.start().line][start..] {
                        *byte = range.start().byte + (*byte - range.end().byte.min(*byte));
                    }
                } else {
                    let mut inline_inlays = document_ref.inline_inlays[range.start().line]
                        .drain(start..)
                        .collect::<Vec<_>>();
                    inline_inlays.extend(
                        document_ref.inline_inlays[range.end().line]
                            .drain(..end)
                            .map(|(byte, inline_inlay)| {
                                (byte - range.end().byte.min(byte), inline_inlay)
                            }),
                    );
                    document_ref.inline_inlays.splice(
                        range.start().line..range.end().line + 1,
                        iter::once(inline_inlays),
                    );
                }
            }
        }
    }

    fn create_session(&mut self, document: DocumentId) -> SessionId {
        let document_ref = &mut self.documents[document.0];
        let line_count = document_ref.text.as_lines().len();
        let session = SessionId(self.sessions.insert(Session {
            document,
            max_column_count: usize::MAX,
            y: Vec::new(),
            column_count: (0..line_count).map(|_| None).collect(),
            fold_column: (0..line_count).map(|_| 0).collect(),
            scale: (0..line_count).map(|_| 1.0).collect(),
            indent_column_count: (0..line_count).map(|_| 0).collect(),
            wraps: (0..line_count).map(|_| Vec::new()).collect(),
            selections: vec![Selection::default()].into(),
            pending_selection_index: None,
        }));
        document_ref.sessions.insert(session);
        self.update_y(session);
        for line in 0..self.line_count(document) {
            self.update_column_count(session, line);
        }
        session
    }

    fn destroy_session(&mut self, session: SessionId) {
        let document = self.document(session);
        let document_ref = &mut self.documents[document.0];
        document_ref.sessions.remove(&session);
        if document_ref.sessions.is_empty() {
            self.destroy_document(document);
        }
        self.sessions.remove(session.0);
    }

    fn create_document(&mut self, path: Option<PathBuf>, text: Text) -> DocumentId {
        let line_count = text.as_lines().len();
        let document = DocumentId(
            self.documents.insert(Document {
                path,
                text,
                tokens: (0..line_count).map(|_| Vec::new()).collect(),
                inline_inlays: (0..line_count)
                    .map(|line| {
                        if line == 2 {
                            vec![(8, InlineInlay::Text("XXX".to_owned()))]
                        } else {
                            Vec::new()
                        }
                    })
                    .collect::<Vec<_>>(),
                block_inlays: Vec::new(),
                sessions: HashSet::new(),
            }),
        );
        if let Some(path) = &self.documents[document.0].path {
            self.document_ids_by_path.insert(path.clone(), document);
        }
        document
    }

    fn destroy_document(&mut self, document: DocumentId) {
        if let Some(path) = &self.documents[document.0].path {
            self.document_ids_by_path.remove(path);
        }
        self.documents.remove(document.0);
    }

    fn update_y(&mut self, session: SessionId) {
        let start_line = self.sessions[session.0].y.len();
        let end_line = self.line_count(self.document(session));
        if start_line == end_line + 1 {
            return;
        }
        let mut y = if start_line == 0 {
            0.0
        } else {
            let line_ref = self.line_ref(session, start_line - 1);
            line_ref.y() + line_ref.height()
        };
        let mut ys = mem::take(&mut self.sessions[session.0].y);
        for block in self.blocks(session, start_line..end_line) {
            match block {
                Block::LineRef { is_inlay, line_ref } => {
                    if !is_inlay {
                        ys.push(y);
                    }
                    y += line_ref.height();
                }
                Block::Widget(widget) => {
                    y += widget.height;
                }
            }
        }
        ys.push(y);
        self.sessions[session.0].y = ys;
    }

    fn update_column_count(&mut self, session: SessionId, line: usize) {
        let mut column_count = 0;
        let mut column_index = 0;
        for wrapped in self.line_ref(session, line).wrappeds() {
            match wrapped {
                Wrapped::Text { text, .. } => {
                    column_index += text
                        .chars()
                        .map(|char| char.column_count(self.settings.tab_column_count))
                        .sum::<usize>();
                }
                Wrapped::Widget(widget) => {
                    column_index += widget.column_count;
                }
                Wrapped::Wrap => {
                    column_count = column_count.max(column_index);
                    column_index = self.line_ref(session, line).indent_column_count();
                }
            }
        }
        self.sessions[session.0].column_count[line] = Some(column_count.max(column_index));
    }

    fn update_wrap_data(&mut self, session_id: SessionId, line: usize) {
        let (indent_column_count, wraps) = self.line_ref(session_id, line).compute_wrap_data(
            self.sessions[session_id.0].max_column_count,
            self.settings.tab_column_count,
        );
        let session_ref = &mut self.sessions[session_id.0];
        session_ref.indent_column_count[line] = indent_column_count;
        session_ref.wraps[line] = wraps;
        session_ref.y.truncate(line + 1);
        self.update_column_count(session_id, line);
    }
}

#[derive(Clone, Debug)]
pub struct Blocks<'a> {
    line_refs: LineRefs<'a>,
    block_inlays: Iter<'a, (usize, BlockInlay)>,
    line: usize,
}

impl<'a> Iterator for Blocks<'a> {
    type Item = Block<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(line, _)| line == self.line)
        {
            let (_, block_inlay) = self.block_inlays.next().unwrap();
            return Some(match *block_inlay {
                BlockInlay::Widget(widget) => Block::Widget(widget),
            });
        }
        let line = self.line_refs.next()?;
        self.line += 1;
        Some(Block::LineRef {
            is_inlay: false,
            line_ref: line,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Block<'a> {
    LineRef {
        is_inlay: bool,
        line_ref: LineRef<'a>,
    },
    Widget(BlockWidget),
}

#[derive(Clone, Debug)]
pub struct LineRefs<'a> {
    pub y: Iter<'a, f64>,
    pub column_count: Iter<'a, Option<usize>>,
    pub fold_column_index: Iter<'a, usize>,
    pub scale: Iter<'a, f64>,
    pub indent_column_count: Iter<'a, usize>,
    pub text: Iter<'a, String>,
    pub tokens: Iter<'a, Vec<Token>>,
    pub inline_inlays_by_byte_index: Iter<'a, Vec<(usize, InlineInlay)>>,
    pub wrap_byte_indices: Iter<'a, Vec<usize>>,
}

impl<'a> Iterator for LineRefs<'a> {
    type Item = LineRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let text = self.text.next()?;
        Some(LineRef {
            y: self.y.next().copied(),
            column_count: *self.column_count.next().unwrap(),
            fold_column: *self.fold_column_index.next().unwrap(),
            scale: *self.scale.next().unwrap(),
            indent_column_count: *self.indent_column_count.next().unwrap(),
            text,
            tokens: self.tokens.next().unwrap(),
            inline_inlays: self.inline_inlays_by_byte_index.next().unwrap(),
            wraps: self.wrap_byte_indices.next().unwrap(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(Id<Session>);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DocumentId(Id<Document>);

#[derive(Clone, Debug)]
struct Session {
    document: DocumentId,
    max_column_count: usize,
    y: Vec<f64>,
    column_count: Vec<Option<usize>>,
    fold_column: Vec<usize>,
    scale: Vec<f64>,
    indent_column_count: Vec<usize>,
    wraps: Vec<Vec<usize>>,
    selections: Vec<Selection>,
    pending_selection_index: Option<usize>,
}

#[derive(Clone, Debug)]
struct Document {
    path: Option<PathBuf>,
    text: Text,
    tokens: Vec<Vec<Token>>,
    inline_inlays: Vec<Vec<(usize, InlineInlay)>>,
    block_inlays: Vec<(usize, BlockInlay)>,
    sessions: HashSet<SessionId>,
}
