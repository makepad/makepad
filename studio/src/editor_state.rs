use {
    crate::{
        code_editor::{
            cursor_set::CursorSet,
            indent_cache::IndentCache,
            protocol::{TextFileTag, TextFileId, Request},
            token_cache::TokenCache,
        },
        design_editor::{
            inline_cache::InlineCache,
        },
        editors::EditorViewId,
    },
    makepad_render::{
        makepad_live_tokenizer::{
            delta::{self, Delta, Whose},
            position::Position,
            position_set::PositionSet,
            range_set::RangeSet,
            size::Size,
            text::Text,
        },
        *,
    },
    std::{
        cell::RefCell,
        collections::{HashMap, HashSet, VecDeque},
        iter,
        mem,
        path::PathBuf,
    },
};

#[derive(Default)]
pub struct EditorState {
    pub session_id_allocator: GenIdAllocator<SessionTag>,
    pub sessions_by_session_id: GenIdMap<SessionTag, Session>,
    pub document_id_allocator: GenIdAllocator<DocumentTag>,
    pub documents_by_document_id: GenIdMap<DocumentTag, Document>,
    pub document_ids_by_path: HashMap<PathBuf, DocumentId>,
    pub document_ids_by_file_id: GenIdMap<TextFileTag, DocumentId>,
    pub outstanding_document_id_queue: VecDeque<DocumentId>,
}

impl EditorState {
    pub fn new() -> EditorState {
        EditorState::default()
    }

    pub fn create_session(
        &mut self,
        path: PathBuf,
        send_request: &mut dyn FnMut(Request),
    ) -> SessionId {
        let document_id = self.get_or_create_document(path, send_request);
        let session_id = self.session_id_allocator.allocate();
        let session = Session {
            session_view: None,
            cursors: CursorSet::new(),
            selections: RangeSet::new(),
            carets: PositionSet::new(),
            document_id,
        };
        self.sessions_by_session_id.insert(session_id, session);
        let document = &mut self.documents_by_document_id[document_id];
        document.session_ids.insert(session_id);
        session_id
    }

    pub fn destroy_session(
        &mut self,
        session_id: SessionId,
        send_request: &mut dyn FnMut(Request),
    ) { 
        let session = &self.sessions_by_session_id[session_id];
        let document_id = session.document_id;
        let document = &mut self.documents_by_document_id[document_id];
        document.session_ids.remove(&session_id);
        if document.session_ids.is_empty() {
            self.destroy_document(document_id, send_request);
        }
        self.sessions_by_session_id.remove(session_id);
        self.session_id_allocator.deallocate(session_id);
    }

    pub fn get_or_create_document(
        &mut self,
        path: PathBuf,
        send_request: &mut dyn FnMut(Request),
    ) -> DocumentId {
        match self.document_ids_by_path.get(&path) {
            Some(document_id) => {
                let document = &mut self.documents_by_document_id[*document_id];
                document.should_be_destroyed = false;
                *document_id
            },
            None => {
                let document_id = self.document_id_allocator.allocate();
                self.documents_by_document_id.insert(
                    document_id,
                    Document {
                        session_ids: HashSet::new(),
                        should_be_destroyed: false,
                        path: path.clone(),
                        inner: None,
                    },
                );
                self.document_ids_by_path.insert(path.clone(), document_id);
                self.outstanding_document_id_queue.push_back(document_id);
                send_request(Request::OpenFile(path));
                document_id
            }
        }
    }

    pub fn handle_open_file_response(
        &mut self,
        file_id: TextFileId,
        revision: usize,
        text: Text,
        send_request: &mut dyn FnMut(Request),
    ) -> DocumentId {
        let document_id = self.outstanding_document_id_queue.pop_front().unwrap();
        let document = &mut self.documents_by_document_id[document_id];
        let token_cache = TokenCache::new(&text);
        let indent_cache = IndentCache::new(&text);
        let inline_cache = RefCell::new(InlineCache::new(&text));
        
        document.inner = Some(DocumentInner {
            file_id,
            revision,
            text,
            token_cache,
            indent_cache,
            inline_cache,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            outstanding_deltas: VecDeque::new(),
        });
        self.document_ids_by_file_id.insert(file_id, document_id);
        if document.should_be_destroyed {
            self.destroy_document_deferred(document_id, send_request);
        }
        document_id
    }

    pub fn destroy_document(
        &mut self,
        document_id: DocumentId,
        send_request: &mut dyn FnMut(Request),
    ) {
        let document = &mut self.documents_by_document_id[document_id];
        if document.inner.is_some() {
            self.destroy_document_deferred(document_id, send_request);
        } else {
            document.should_be_destroyed = true;
        }
    }

    fn destroy_document_deferred(&mut self, document_id: DocumentId, send_request: &mut dyn FnMut(Request)) {
        let document = &mut self.documents_by_document_id[document_id];
        let inner = document.inner.as_ref().unwrap();
        let file_id = inner.file_id;
        self.document_ids_by_file_id.remove(file_id);
        self.documents_by_document_id.remove(document_id);
        self.document_id_allocator.deallocate(document_id);
        send_request(Request::CloseFile(file_id))
    }

    pub fn add_cursor(&mut self, session_id: SessionId, position: Position) {
        let session = &mut self.sessions_by_session_id[session_id];
        session.cursors.add(position);
        session.update_selections_and_carets();
    }

    pub fn move_cursors_left(&mut self, session_id: SessionId, select: bool) {
        let session = &mut self.sessions_by_session_id[session_id];
        let document = &self.documents_by_document_id[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        session.cursors.move_left(&document_inner.text, select);
        session.update_selections_and_carets();
    }

    pub fn move_cursors_right(&mut self, session_id: SessionId, select: bool) {
        let session = &mut self.sessions_by_session_id[session_id];
        let document = &self.documents_by_document_id[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        session.cursors.move_right(&document_inner.text, select);
        session.update_selections_and_carets();
    }

    pub fn move_cursors_up(&mut self, session_id: SessionId, select: bool) {
        let session = &mut self.sessions_by_session_id[session_id];
        let document = &self.documents_by_document_id[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        session.cursors.move_up(&document_inner.text, select);
        session.update_selections_and_carets();
    }

    pub fn move_cursors_down(&mut self, session_id: SessionId, select: bool) {
        let session = &mut self.sessions_by_session_id[session_id];
        let document = &self.documents_by_document_id[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        session.cursors.move_down(&document_inner.text, select);
        session.update_selections_and_carets();
    }

    pub fn move_cursors_to(&mut self, session_id: SessionId, position: Position, select: bool) {
        let session = &mut self.sessions_by_session_id[session_id];
        session.cursors.move_to(position, select);
        session.update_selections_and_carets();
    }

    pub fn replace_text_direct(
        &mut self,
        session_id: SessionId,
        position: Position,
        size: Size,
        text: Text,
        send_request: &mut dyn FnMut(Request),
    ) {
        let mut builder = delta::Builder::new();
        
        // we need to retain from 0 to position
        builder.retain(position - Position::origin());
        builder.delete(size);
        builder.insert(text);
        let delta = builder.build();
        
        self.edit(session_id, delta, send_request);
    }


    pub fn insert_text(
        &mut self,
        session_id: SessionId,
        text: Text,
        send_request: &mut dyn FnMut(Request),
    ) {
        let session = &self.sessions_by_session_id[session_id];

        let mut builder_0 = delta::Builder::new();
        for span in session.selections.spans() {
            if span.is_included {
                builder_0.delete(span.len);
            } else {
                builder_0.retain(span.len);
            }
        }
        let delta_0 = builder_0.build();

        let mut builder_1 = delta::Builder::new();
        let mut position = Position::origin();
        for distance in session.carets.distances() {
            position += distance;
            builder_1.retain(distance);
            if session.selections.contains_position(position) {
                continue;
            }
            builder_1.insert(text.clone());
            position += text.len();
        }
        let delta_1 = builder_1.build();

        let (_, new_delta_1) = delta_0.clone().transform(delta_1);
        let delta = delta_0.compose(new_delta_1);

        self.edit(session_id, delta, send_request);
    }

    pub fn insert_newline(
        &mut self,
        session_id: SessionId,
        send_request: &mut dyn FnMut(Request)
    ) {
        let session = &self.sessions_by_session_id[session_id];
        let document = &self.documents_by_document_id[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();

        let mut builder_0 = delta::Builder::new();
        for span in session.selections.spans() {
            if span.is_included {
                builder_0.delete(span.len);
            } else {
                builder_0.retain(span.len);
            }
        }
        let delta_0 = builder_0.build();

        let mut builder_1 = delta::Builder::new();
        let mut position = Position::origin();
        for distance in session.carets.distances() {
            if distance.line == 0 {
                position += distance;
                builder_1.retain(distance);
            } else {
                position.line += distance.line;
                position.column = 0;
                builder_1.retain(Size {
                    line: distance.line,
                    column: 0,
                });
                let indent_info = &document_inner.indent_cache[position.line];
                match indent_info.leading_whitespace() {
                    Some(_) => {
                        position.column += distance.column;
                        builder_1.retain(Size {
                            line: 0,
                            column: distance.column
                        });
                    }
                    None => {
                        builder_1.delete(Size {
                            line: 0,
                            column: distance.column
                        });
                    }
                }
            }
            if session.selections.contains_position(position) {
                continue;
            }
            let text = Text::from(vec![vec![], vec![]]);
            builder_1.insert(text.clone());
            position += text.len();
        }
        let delta_1 = builder_1.build();

        let (_, new_delta_1) = delta_0.clone().transform(delta_1);
        let delta = delta_0.compose(new_delta_1);

        self.edit(session_id, delta, send_request);

        self.autoindent(session_id, send_request);
    }

    fn autoindent(
        &mut self,
        session_id: SessionId,
        send_request: &mut dyn FnMut(Request)
    ) {
        let session = &self.sessions_by_session_id[session_id];
        let document = &self.documents_by_document_id[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        
        let mut builder = delta::Builder::new();
        let mut position = Position::origin();
        for distance in session.carets.distances() {
            position += distance;
            builder.retain(distance);
            let indent_count = (0..position.line).rev().find_map(|line| {
                let indent_info = &document_inner.indent_cache[line];
                indent_info.leading_whitespace().map(|leading_whitespace| {
                    let mut indent_count = (leading_whitespace + 3) / 4;
                    let mut delimiter_count = 0;
                    let token_info = &document_inner.token_cache[line];
                    for token in token_info.tokens() {
                        if token.token.is_open_delimiter() {
                            delimiter_count += 1;
                        }
                        if token.token.is_close_delimiter() {
                            delimiter_count -= 1;
                        }
                    }
                    if delimiter_count > 0 {
                        indent_count += 1;
                    }
                    indent_count
                })
            }).unwrap_or(0);
            let text = Text::from(vec![iter::repeat(' ').take(indent_count * 4).collect::<Vec<_>>()]);
            let len = text.len();
            builder.insert(text);
            position += len;
        }
        let delta = builder.build();

        self.edit(session_id, delta, send_request);
    }

    pub fn insert_backspace(
        &mut self,
        session_id: SessionId,
        send_request: &mut dyn FnMut(Request),
    ) {
        let session = &self.sessions_by_session_id[session_id];
        let document = &self.documents_by_document_id[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();

        let mut builder_0 = delta::Builder::new();
        for span in session.selections.spans() {
            if span.is_included {
                builder_0.delete(span.len);
            } else {
                builder_0.retain(span.len);
            }
        }
        let delta_0 = builder_0.build();

        let mut builder_1 = delta::Builder::new();
        let mut position = Position::origin();
        for distance in session.carets.distances() {
            position += distance;
            if !session.selections.contains_position(position) {
                if position.column == 0 {
                    if position.line != 0 {
                        builder_1.retain(Size {
                            line: distance.line - 1,
                            column: document_inner.text.as_lines()[position.line - 1].len(),
                        });
                        builder_1.delete(Size { line: 1, column: 0 })
                    }
                } else {
                    builder_1.retain(Size {
                        line: distance.line,
                        column: distance.column - 1,
                    });
                    builder_1.delete(Size { line: 0, column: 1 });
                }
            } else {
                builder_1.retain(distance);
            }
        }
        let delta_1 = builder_1.build();

        let (_, new_delta_1) = delta_0.clone().transform(delta_1);
        let delta = delta_0.compose(new_delta_1);

        self.edit(session_id, delta, send_request);
    }

    fn edit(
        &mut self,
        session_id: SessionId,
        delta: Delta,
        send_request: &mut dyn FnMut(Request),
    ) {
        let session = &self.sessions_by_session_id[session_id];
        let document = &mut self.documents_by_document_id[session.document_id];
        let document_inner = document.inner.as_mut().unwrap();

        let inverse_delta = delta.clone().invert(&document_inner.text);
        document_inner.undo_stack.push(Edit {
            cursors: session.cursors.clone(),
            delta: inverse_delta,
        });

        let session = &mut self.sessions_by_session_id[session_id];
        session.apply_delta(&delta, Whose::Ours);

        self.apply_delta(session_id, delta, send_request);
    }

    pub fn undo(&mut self, session_id: SessionId, send_request: &mut dyn FnMut(Request)) {
        let session = &self.sessions_by_session_id[session_id];
        let document = &mut self.documents_by_document_id[session.document_id];
        let document_inner = document.inner.as_mut().unwrap();
        if let Some(undo) = document_inner.undo_stack.pop() {
            let inverse_delta = undo.delta.clone().invert(&document_inner.text);
            document_inner.redo_stack.push(Edit {
                cursors: session.cursors.clone(),
                delta: inverse_delta,
            });

            let session = &mut self.sessions_by_session_id[session_id];
            session.cursors = undo.cursors;
            session.update_selections_and_carets();
    
            self.apply_delta(session_id, undo.delta, send_request);
        }
    }

    pub fn redo(&mut self, session_id: SessionId, send_request: &mut dyn FnMut(Request)) {
        let session = &self.sessions_by_session_id[session_id];
        let document = &mut self.documents_by_document_id[session.document_id];
        let document_inner = document.inner.as_mut().unwrap();
        if let Some(redo) = document_inner.redo_stack.pop() {
            let inverse_delta = redo.delta.clone().invert(&document_inner.text);
            document_inner.undo_stack.push(Edit {
                cursors: session.cursors.clone(),
                delta: inverse_delta,
            });

            let session = &mut self.sessions_by_session_id[session_id];
            session.cursors = redo.cursors;
            session.update_selections_and_carets();
    
            self.apply_delta(session_id, redo.delta, send_request);
        }
    }

    fn apply_delta(
        &mut self,
        session_id: SessionId,
        delta: Delta,
        send_request: &mut dyn FnMut(Request),
    ) {
        let session = &mut self.sessions_by_session_id[session_id];
        let document_id = session.document_id;

        let document = &self.documents_by_document_id[document_id];
        for other_session_id in document.session_ids.iter().cloned() {
            if other_session_id == session_id {
                continue;
            }

            let other_session = &mut self.sessions_by_session_id[other_session_id];
            other_session.apply_delta(&delta, Whose::Theirs);
        }

        let document = &mut self.documents_by_document_id[document_id];
        document.apply_delta(delta.clone());
        document.schedule_apply_delta_request(delta, send_request);
    }

    pub fn handle_apply_delta_response(
        &mut self,
        file_id: TextFileId,
        send_request: &mut dyn FnMut(Request),
    ) {
        let document_id = self.document_ids_by_file_id[file_id];
        let document = &mut self.documents_by_document_id[document_id];
        let document_inner = document.inner.as_mut().unwrap();

        document_inner.outstanding_deltas.pop_front();
        document_inner.revision += 1;
        if let Some(outstanding_delta) = document_inner.outstanding_deltas.front() {
            send_request(Request::ApplyDelta(
                file_id,
                document_inner.revision,
                outstanding_delta.clone(),
            ));
        }
    }

    pub fn handle_delta_applied_notification(
        &mut self,
        file_id: TextFileId,
        delta: Delta,
    ) -> DocumentId {
        let document_id = self.document_ids_by_file_id[file_id];
        let document = &mut self.documents_by_document_id[document_id];
        let document_inner = document.inner.as_mut().unwrap();

        let mut delta = delta;
        for outstanding_delta_ref in &mut document_inner.outstanding_deltas {
            let outstanding_delta = mem::replace(outstanding_delta_ref, Delta::identity());
            let (new_delta, new_outstanding_delta) = delta.transform(outstanding_delta);
            delta = new_delta;
            *outstanding_delta_ref = new_outstanding_delta;
        }

        transform_edit_stack(&mut document_inner.undo_stack, delta.clone());
        transform_edit_stack(&mut document_inner.redo_stack, delta.clone());

        for session_id in document.session_ids.iter().cloned() {
            let session = &mut self.sessions_by_session_id[session_id];
            session.apply_delta(&delta, Whose::Theirs);
        }

        let document = &mut self.documents_by_document_id[document_id];
        let document_inner = document.inner.as_mut().unwrap();
        document_inner.revision += 1;
        document.apply_delta(delta);

        document_id
    }
}

pub enum SessionTag {}
pub type SessionId = GenId<SessionTag>;

pub struct Session {
    pub session_view: Option<EditorViewId>,
    pub cursors: CursorSet,
    pub selections: RangeSet,
    pub carets: PositionSet,
    pub document_id: DocumentId,
}

impl Session {
    fn apply_delta(&mut self, delta: &Delta, whose: Whose) {
        self.cursors.apply_delta(&delta, whose);
        self.update_selections_and_carets();
    }

    fn update_selections_and_carets(&mut self) {
        self.selections = self.cursors.selections();
        self.carets = self.cursors.carets();
    }
}

pub enum DocumentTag {}
pub type DocumentId = GenId<DocumentTag>;

pub struct Document {
    pub session_ids: HashSet<SessionId>,
    pub should_be_destroyed: bool,
    pub path: PathBuf,
    pub inner: Option<DocumentInner>,
}

impl Document {
    fn apply_delta(&mut self, delta: Delta) {
        let inner = self.inner.as_mut().unwrap();
        inner.token_cache.invalidate(&delta);
        inner.indent_cache.invalidate(&delta);
        inner.inline_cache.borrow_mut().invalidate(&delta);
        inner.text.apply_delta(delta);
        inner.token_cache.refresh(&inner.text);
        inner.indent_cache.refresh(&inner.text);
    }

    fn schedule_apply_delta_request(
        &mut self,
        delta: Delta,
        send_request: &mut dyn FnMut(Request),
    ) {
        let inner = self.inner.as_mut().unwrap();
        if inner.outstanding_deltas.len() == 2 {
            let outstanding_delta = inner.outstanding_deltas.pop_back().unwrap();
            inner
                .outstanding_deltas
                .push_back(outstanding_delta.compose(delta));
        } else {
            inner.outstanding_deltas.push_back(delta.clone());
            if inner.outstanding_deltas.len() == 1 {
                send_request(Request::ApplyDelta(
                    inner.file_id,
                    inner.revision,
                    inner.outstanding_deltas.front().unwrap().clone(),
                ));
            }
        }
    }
}

pub struct DocumentInner {
    pub file_id: TextFileId,
    pub revision: usize,
    pub text: Text,
    pub token_cache: TokenCache,
    pub indent_cache: IndentCache,
    pub inline_cache: RefCell<InlineCache>,
    pub undo_stack: Vec<Edit>,
    pub redo_stack: Vec<Edit>,
    pub outstanding_deltas: VecDeque<Delta>,
}

#[derive(Debug)]
pub struct Edit {
    pub cursors: CursorSet,
    pub delta: Delta,
}

fn transform_edit_stack(edit_stack: &mut Vec<Edit>, delta: Delta) {
    let mut delta = delta;
    for edit in edit_stack.iter_mut().rev() {
        let edit_delta = mem::replace(&mut edit.delta, Delta::identity());
        edit.cursors.apply_delta(&delta, Whose::Theirs);
        let (new_delta, new_edit_delta) = delta.transform(edit_delta);
        delta = new_delta;
        edit.delta = new_edit_delta;
    }
}
