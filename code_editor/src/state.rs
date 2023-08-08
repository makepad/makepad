use {
    crate::{
        arena::Id,
        change::{ChangeKind, Drift},
        char::CharExt,
        edit_ops,
        inlays::{BlockInlay, InlineInlay},
        line::WrappedElement,
        selection::Affinity,
        widgets::BlockWidget,
        Arena, Change, Line, Point, Selection, Settings, Text, Token,
    },
    std::{
        cmp::Ordering,
        collections::{HashMap, HashSet},
        fs::File,
        io,
        io::BufReader,
        mem,
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

    pub fn width(&self, session_id: SessionId) -> f64 {
        let mut width: f64 = 0.0;
        for line in self.lines(session_id, 0..self.line_count(self.document_id(session_id))) {
            width = width.max(line.width());
        }
        width
    }

    pub fn height(&self, session_id: SessionId) -> f64 {
        let last_line_index = self.line_count(self.document_id(session_id)) - 1;
        let last_line = self.line(session_id, last_line_index);
        let mut y = last_line.y() + last_line.height();
        for block_element in self.block_elements(session_id, last_line_index..last_line_index) {
            match block_element {
                BlockElement::Line {
                    is_inlay: true,
                    line,
                } => y += line.height(),
                BlockElement::Widget(widget) => y += widget.height,
                _ => unreachable!(),
            }
        }
        y
    }

    pub fn document_id(&self, session_id: SessionId) -> DocumentId {
        self.sessions[session_id.0].document
    }

    pub fn max_column_count(&self, session_id: SessionId) -> usize {
        self.sessions[session_id.0].max_column_count
    }

    pub fn find_first_line_ending_after_y(&self, session_id: SessionId, y: f64) -> usize {
        match self.sessions[session_id.0]
            .y
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line_index) => line_index,
            Err(line_index) => line_index.saturating_sub(1),
        }
    }

    pub fn find_first_line_starting_after_y(&self, session_id: SessionId, y: f64) -> usize {
        match self.sessions[session_id.0]
            .y
            .binary_search_by(|current_y| current_y.partial_cmp(&y).unwrap())
        {
            Ok(line_index) => line_index + 1,
            Err(line_index) => line_index,
        }
    }

    pub fn line(&self, session_id: SessionId, line_index: usize) -> Line<'_> {
        let session = &self.sessions[session_id.0];
        let document = &self.documents[self.document_id(session_id).0];
        Line {
            y: session.y.get(line_index).copied(),
            column_count: session.column_count[line_index],
            fold_column_index: session.fold_column_index[line_index],
            scale: session.scale[line_index],
            indent_column_count_after_wrap: session.indent_column_count_after_wrap[line_index],
            text: &document.text.as_lines()[line_index],
            tokens: &document.tokens[line_index],
            inline_inlays_by_byte_index: &document.inline_inlays_by_byte_index[line_index],
            wrap_byte_indices: &session.wrap_byte_indices[line_index],
        }
    }

    pub fn lines(&self, session_id: SessionId, line_range: Range<usize>) -> Lines<'_> {
        let session = &self.sessions[session_id.0];
        let document = &self.documents[self.document_id(session_id).0];
        Lines {
            y: session.y
                [line_range.start.min(session.y.len())..line_range.end.min(session.y.len())]
                .iter(),
            column_count: session.column_count[line_range.start..line_range.end].iter(),
            fold_column_index: session.fold_column_index[line_range.start..line_range.end].iter(),
            scale: session.scale[line_range.start..line_range.end].iter(),
            indent_column_count_after_wrap: session.indent_column_count_after_wrap
                [line_range.start..line_range.end]
                .iter(),
            text: document.text.as_lines()[line_range.start..line_range.end].iter(),
            tokens: document.tokens[line_range.start..line_range.end].iter(),
            inline_inlays_by_byte_index: document.inline_inlays_by_byte_index
                [line_range.start..line_range.end]
                .iter(),
            wrap_byte_indices: session.wrap_byte_indices[line_range.start..line_range.end].iter(),
        }
    }

    pub fn block_elements(
        &self,
        session_id: SessionId,
        line_index_range: Range<usize>,
    ) -> BlockElements<'_> {
        let mut block_inlays = self.documents[self.document_id(session_id).0]
            .block_inlays_by_line_index
            .iter();
        while block_inlays
            .as_slice()
            .first()
            .map_or(false, |&(line_index, _)| {
                line_index < line_index_range.start
            })
        {
            block_inlays.next();
        }
        BlockElements {
            lines: self.lines(session_id, line_index_range.start..line_index_range.end),
            block_inlays_by_line_index: block_inlays,
            line_index: line_index_range.start,
        }
    }

    pub fn selections(&self, session_id: SessionId) -> &[Selection] {
        &self.sessions[session_id.0].selections
    }

    pub fn line_count(&self, document_id: DocumentId) -> usize {
        self.documents[document_id.0].text.as_lines().len()
    }

    pub fn new_file(&mut self, text: Text) -> SessionId {
        let document_id = self.create_document(None, text);
        self.create_session(document_id)
    }

    pub fn open_file(&mut self, path: impl AsRef<Path> + Into<PathBuf>) -> io::Result<SessionId> {
        let document_id = match self.document_ids_by_path.get(path.as_ref()) {
            Some(&document) => document,
            None => {
                let file = File::open(path.as_ref())?;
                self.create_document(
                    Some(path.into()),
                    Text::from_buf_reader(BufReader::new(file))?,
                )
            }
        };
        Ok(self.create_session(document_id))
    }

    pub fn close_file(&mut self, session_id: SessionId) {
        self.destroy_session(session_id);
    }

    pub fn set_max_column_count(&mut self, session_id: SessionId, max_column_count: usize) {
        let session = &mut self.sessions[session_id.0];
        if session.max_column_count == max_column_count {
            return;
        }
        session.max_column_count = max_column_count;
        for line_index in 0..self.line_count(self.document_id(session_id)) {
            self.update_wrap_data(session_id, line_index);
        }
        self.update_y(session_id);
    }

    pub fn set_cursor(&mut self, session_id: SessionId, cursor: Point, affinity: Affinity) {
        let session = &mut self.sessions[session_id.0];
        session.selections.clear();
        session.selections.push(Selection {
            anchor: cursor,
            cursor,
            affinity,
        });
        session.pending_selection_index = Some(0);
    }

    pub fn add_cursor(&mut self, session_id: SessionId, cursor: Point, affinity: Affinity) {
        let session = &mut self.sessions[session_id.0];
        let selection = Selection {
            anchor: cursor,
            cursor,
            affinity,
        };
        session.pending_selection_index = Some(
            match session.selections.binary_search_by(|selection| {
                if selection.end() <= cursor {
                    return Ordering::Less;
                }
                if selection.start() >= cursor {
                    return Ordering::Greater;
                }
                Ordering::Equal
            }) {
                Ok(index) => {
                    session.selections[index] = selection;
                    index
                }
                Err(index) => {
                    session.selections.insert(index, selection);
                    index
                }
            },
        );
    }

    pub fn move_to(&mut self, session_id: SessionId, cursor: Point, affinity: Affinity) {
        let session = &mut self.sessions[session_id.0];
        let mut pending_selection_index = session.pending_selection_index.unwrap();
        session.selections[pending_selection_index].cursor = cursor;
        session.selections[pending_selection_index].affinity = affinity;
        while pending_selection_index > 0 {
            let prev_selection_index = pending_selection_index - 1;
            if !session.selections[prev_selection_index]
                .should_merge(session.selections[pending_selection_index])
            {
                break;
            }
            session.selections.remove(prev_selection_index);
            pending_selection_index -= 1;
        }
        while pending_selection_index + 1 < session.selections.len() {
            let next_selection_index = pending_selection_index + 1;
            if !session.selections[pending_selection_index]
                .should_merge(session.selections[next_selection_index])
            {
                break;
            }
            session.selections.remove(next_selection_index);
        }
        session.pending_selection_index = Some(pending_selection_index);
    }

    pub fn insert(&mut self, session_id: SessionId, additional_text: Text) {
        let mut changes = Vec::new();
        let document_id = self.document_id(session_id);
        edit_ops::insert(
            &mut self.documents[document_id.0].text,
            &self.sessions[session_id.0].selections,
            additional_text,
            &mut changes,
        );
        self.update_document_after_edit(document_id, &changes);
    }

    fn update_document_after_edit(&mut self, document_id: DocumentId, changes: &[Change]) {
        let document = &mut self.documents[document_id.0];
        for change in changes {
            match change.kind {
                ChangeKind::Insert(point, ref additional_text) => {
                    let line_index = point.line_index;
                    let line_count = additional_text.extent().line_count;
                    for (byte_index, _) in &mut document.inline_inlays_by_byte_index[line_index] {
                        *byte_index = match (*byte_index).cmp(&point.byte_index) {
                            Ordering::Less => *byte_index,
                            Ordering::Equal => match change.drift {
                                Drift::Before => {
                                    *byte_index + additional_text.as_lines().first().unwrap().len()
                                }
                                Drift::After => *byte_index,
                            },
                            Ordering::Greater => {
                                *byte_index + additional_text.as_lines().first().unwrap().len()
                            }
                        };
                    }
                    if line_count >= 1 {
                        if line_count >= 2 {
                            document.inline_inlays_by_byte_index.splice(
                                line_index + 1..line_index + 1,
                                (0..line_count - 1).map(|_| Vec::new()),
                            );
                        }
                        for (byte_index, _) in
                            &mut document.inline_inlays_by_byte_index[line_index + line_count - 1]
                        {
                            *byte_index += additional_text.as_lines().last().unwrap().len();
                        }
                        // TODO: Update block inlays
                    }
                }
                ChangeKind::Delete(_range) => {
                    // TODO
                }
            }
        }
        for session_id in self.documents[document_id.0].session_ids.clone() {
            self.update_session_after_edit(session_id, changes);
        }
    }

    fn update_session_after_edit(&mut self, session_id: SessionId, changes: &[Change]) {
        for change in changes {
            match change.kind {
                ChangeKind::Insert(point, ref additional_text) => {
                    let line_index = point.line_index;
                    let line_count = additional_text.extent().line_count;
                    self.update_wrap_data(session_id, line_index);
                    if line_count >= 1 {
                        if line_count >= 2 {
                            let session = &mut self.sessions[session_id.0];
                            session.y.truncate(line_index + 1);
                            session.column_count.splice(
                                line_index + 1..line_index + 1,
                                (0..line_count - 1).map(|_| None),
                            );
                            session.fold_column_index.splice(
                                line_index + 1..line_index + 1,
                                (0..line_count - 1).map(|_| 0),
                            );
                            session.scale.splice(
                                line_index + 1..line_index + 1,
                                (0..line_count - 1).map(|_| 0.0),
                            );
                            session.indent_column_count_after_wrap.splice(
                                line_index + 1..line_index + 1,
                                (0..line_count - 1).map(|_| 0),
                            );
                            session.wrap_byte_indices.splice(
                                line_index + 1..line_index + 1,
                                (0..line_count - 1).map(|_| Vec::new()),
                            );
                        }
                        self.update_wrap_data(session_id, line_index + line_count - 1);
                    }
                }
                ChangeKind::Delete(range) => {
                    let line_index = range.start().line_index;
                    let line_count = range.end().line_index - range.start().line_index;
                    self.update_wrap_data(session_id, line_index);
                    if line_count >= 1 {
                        if line_count >= 2 {
                            let session = &mut self.sessions[session_id.0];
                            session
                                .column_count
                                .drain(line_index..line_index + line_count - 1);
                            session
                                .fold_column_index
                                .drain(line_index..line_index + line_count - 1);
                            session.scale.drain(line_index..line_index + line_count - 1);
                            session
                                .indent_column_count_after_wrap
                                .drain(line_index..line_index + line_count - 1);
                            session
                                .wrap_byte_indices
                                .drain(line_index..line_index + line_count - 1);
                        }
                        self.update_wrap_data(session_id, line_index + 1);
                    }
                }
            }
            for selection in &mut self.sessions[session_id.0].selections {
                *selection = selection.apply_change(change);
            }
        }
        self.update_y(session_id);
    }

    fn create_session(&mut self, document_id: DocumentId) -> SessionId {
        let document = &mut self.documents[document_id.0];
        let line_count = document.text.as_lines().len();
        let session_id = SessionId(self.sessions.insert(Session {
            document: document_id,
            max_column_count: usize::MAX,
            y: Vec::new(),
            column_count: (0..line_count).map(|_| None).collect(),
            fold_column_index: (0..line_count).map(|_| 0).collect(),
            scale: (0..line_count).map(|_| 1.0).collect(),
            indent_column_count_after_wrap: (0..line_count).map(|_| 0).collect(),
            wrap_byte_indices: (0..line_count).map(|_| Vec::new()).collect(),
            selections: vec![Selection::default()].into(),
            pending_selection_index: None,
        }));
        document.session_ids.insert(session_id);
        self.update_y(session_id);
        for line_index in 0..self.line_count(document_id) {
            self.update_column_count(session_id, line_index);
        }
        session_id
    }

    fn update_y(&mut self, session_id: SessionId) {
        let start_line_index = self.sessions[session_id.0].y.len();
        let line_count = self.line_count(self.document_id(session_id));
        if start_line_index == line_count + 1 {
            return;
        }
        let mut y = if start_line_index == 0 {
            0.0
        } else {
            let line = self.line(session_id, start_line_index - 1);
            line.y() + line.height()
        };
        let mut ys = mem::take(&mut self.sessions[session_id.0].y);
        for block_element in self.block_elements(session_id, start_line_index..line_count) {
            match block_element {
                BlockElement::Line { is_inlay, line } => {
                    if !is_inlay {
                        ys.push(y);
                    }
                    y += line.height();
                }
                BlockElement::Widget(widget) => {
                    y += widget.height;
                }
            }
        }
        ys.push(y);
        self.sessions[session_id.0].y = ys;
    }

    fn update_column_count(&mut self, session_id: SessionId, line_index: usize) {
        let mut column_count = 0;
        let mut column_index = 0;
        for wrapped_element in self.line(session_id, line_index).wrapped_elements() {
            match wrapped_element {
                WrappedElement::Text { text, .. } => {
                    column_index += text
                        .chars()
                        .map(|char| char.column_count(self.settings.tab_column_count))
                        .sum::<usize>();
                }
                WrappedElement::Widget(widget) => {
                    column_index += widget.column_count;
                }
                WrappedElement::Wrap => {
                    column_count = column_count.max(column_index);
                    column_index = self
                        .line(session_id, line_index)
                        .indent_column_count_after_wrap();
                }
            }
        }
        self.sessions[session_id.0].column_count[line_index] = Some(column_count.max(column_index));
    }

    fn update_wrap_data(&mut self, session_id: SessionId, line_index: usize) {
        let (indent_column_count_after_wrap, wrap_byte_indices) =
            self.line(session_id, line_index).compute_wrap_data(
                self.sessions[session_id.0].max_column_count,
                self.settings.tab_column_count,
            );
        let session = &mut self.sessions[session_id.0];
        session.indent_column_count_after_wrap[line_index] = indent_column_count_after_wrap;
        session.wrap_byte_indices[line_index] = wrap_byte_indices;
        session.y.truncate(line_index + 1);
        self.update_column_count(session_id, line_index);
    }

    fn destroy_session(&mut self, session_id: SessionId) {
        let document_id = self.document_id(session_id);
        let document = &mut self.documents[document_id.0];
        document.session_ids.remove(&session_id);
        if document.session_ids.is_empty() {
            self.destroy_document(document_id);
        }
        self.sessions.remove(session_id.0);
    }

    fn create_document(&mut self, path: Option<PathBuf>, text: Text) -> DocumentId {
        let line_count = text.as_lines().len();
        let document_id = DocumentId(self.documents.insert(Document {
            path,
            text,
            tokens: (0..line_count).map(|_| Vec::new()).collect(),
            inline_inlays_by_byte_index: (0..line_count).map(|_| Vec::new()).collect(),
            block_inlays_by_line_index: Vec::new(),
            session_ids: HashSet::new(),
        }));
        if let Some(path) = &self.documents[document_id.0].path {
            self.document_ids_by_path.insert(path.clone(), document_id);
        }
        document_id
    }

    fn destroy_document(&mut self, document_id: DocumentId) {
        if let Some(path) = &self.documents[document_id.0].path {
            self.document_ids_by_path.remove(path);
        }
        self.documents.remove(document_id.0);
    }
}

#[derive(Clone, Debug)]
pub struct BlockElements<'a> {
    lines: Lines<'a>,
    block_inlays_by_line_index: Iter<'a, (usize, BlockInlay)>,
    line_index: usize,
}

impl<'a> Iterator for BlockElements<'a> {
    type Item = BlockElement<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .block_inlays_by_line_index
            .as_slice()
            .first()
            .map_or(false, |&(line_index, _)| line_index == self.line_index)
        {
            let (_, block_inlay) = self.block_inlays_by_line_index.next().unwrap();
            return Some(match *block_inlay {
                BlockInlay::Widget(widget) => BlockElement::Widget(widget),
            });
        }
        let line = self.lines.next()?;
        self.line_index += 1;
        Some(BlockElement::Line {
            is_inlay: false,
            line,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BlockElement<'a> {
    Line { is_inlay: bool, line: Line<'a> },
    Widget(BlockWidget),
}

#[derive(Clone, Debug)]
pub struct Lines<'a> {
    pub y: Iter<'a, f64>,
    pub column_count: Iter<'a, Option<usize>>,
    pub fold_column_index: Iter<'a, usize>,
    pub scale: Iter<'a, f64>,
    pub indent_column_count_after_wrap: Iter<'a, usize>,
    pub text: Iter<'a, String>,
    pub tokens: Iter<'a, Vec<Token>>,
    pub inline_inlays_by_byte_index: Iter<'a, Vec<(usize, InlineInlay)>>,
    pub wrap_byte_indices: Iter<'a, Vec<usize>>,
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let text = self.text.next()?;
        Some(Line {
            y: self.y.next().copied(),
            column_count: *self.column_count.next().unwrap(),
            fold_column_index: *self.fold_column_index.next().unwrap(),
            scale: *self.scale.next().unwrap(),
            indent_column_count_after_wrap: *self.indent_column_count_after_wrap.next().unwrap(),
            text,
            tokens: self.tokens.next().unwrap(),
            inline_inlays_by_byte_index: self.inline_inlays_by_byte_index.next().unwrap(),
            wrap_byte_indices: self.wrap_byte_indices.next().unwrap(),
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
    fold_column_index: Vec<usize>,
    scale: Vec<f64>,
    indent_column_count_after_wrap: Vec<usize>,
    wrap_byte_indices: Vec<Vec<usize>>,
    selections: Vec<Selection>,
    pending_selection_index: Option<usize>,
}

#[derive(Clone, Debug)]
struct Document {
    path: Option<PathBuf>,
    text: Text,
    tokens: Vec<Vec<Token>>,
    inline_inlays_by_byte_index: Vec<Vec<(usize, InlineInlay)>>,
    block_inlays_by_line_index: Vec<(usize, BlockInlay)>,
    session_ids: HashSet<SessionId>,
}
