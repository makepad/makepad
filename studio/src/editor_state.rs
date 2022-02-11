use {
    crate::{
        makepad_platform::*,
        makepad_live_tokenizer::{
            delta::{self, Delta},
            position::Position,
            position_set::PositionSet,
            range::Range,
            range_set::RangeSet,
            size::Size,
            text::Text,
        },
        builder::builder_protocol::BuilderMsg,
        code_editor::{
            cursor_set::CursorSet,
            indent_cache::IndentCache,
            msg_cache::MsgCache,
            token_cache::TokenCache,
        },
        collab::collab_protocol::{CollabRequest, TextFileId},
        design_editor::inline_cache::InlineCache,
        editors::EditorViewId,
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
    pub sessions: LiveIdMap<SessionId, Session>,
    pub documents: LiveIdMap<DocumentId, Document>,
    pub documents_by_path: HashMap<PathBuf, DocumentId>,
    pub documents_by_file: LiveIdMap<TextFileId, DocumentId>,
    pub outstanding_document_queue: VecDeque<DocumentId>,
    pub messages: Vec<BuilderMsg>,
}

impl EditorState {
    pub fn new() -> EditorState {
        EditorState::default()
    }
    
    pub fn create_session(
        &mut self,
        path: PathBuf,
        send_request: &mut dyn FnMut(CollabRequest),
    ) -> SessionId {
        let document_id = self.get_or_create_document(path, send_request);
        let session_id = self.sessions.insert_unique(Session {
            session_view: None,
            injected_char_stack: Vec::new(),
            cursors: CursorSet::new(),
            selections: RangeSet::new(),
            carets: PositionSet::new(),
            document_id,
        });
        let document = &mut self.documents[document_id];
        document.session_ids.insert(session_id);
        session_id
    }
    
    pub fn destroy_session(
        &mut self,
        session_id: SessionId,
        send_request: &mut dyn FnMut(CollabRequest),
    ) {
        let session = &self.sessions[session_id];
        let document_id = session.document_id;
        let document = &mut self.documents[document_id];
        document.session_ids.remove(&session_id);
        if document.session_ids.is_empty() {
            self.destroy_document(document_id, send_request);
        }
        self.sessions.remove(&session_id);
    }
    
    pub fn get_or_create_document(
        &mut self,
        path: PathBuf,
        send_request: &mut dyn FnMut(CollabRequest),
    ) -> DocumentId {
        match self.documents_by_path.get(&path) {
            Some(document_id) => {
                let document = &mut self.documents[*document_id];
                document.should_be_destroyed = false;
                *document_id
            }
            None => {
                let document_id = self.documents.insert_unique(Document {
                    session_ids: HashSet::new(),
                    should_be_destroyed: false,
                    path: path.clone(),
                    inner: None,
                });
                self.documents_by_path.insert(path.clone(), document_id);
                self.outstanding_document_queue.push_back(document_id);
                send_request(CollabRequest::OpenFile(path));
                document_id
            }
        }
    }
    
    pub fn handle_open_file_response(
        &mut self,
        file_id: TextFileId,
        revision: usize,
        text: Text,
        send_request: &mut dyn FnMut(CollabRequest),
    ) -> DocumentId {
        let document_id = self.outstanding_document_queue.pop_front().unwrap();
        let document = &mut self.documents[document_id];
        let token_cache = TokenCache::new(&text);
        let indent_cache = IndentCache::new(&text);
        let msg_cache = MsgCache::new(&text);
        
        let inline_cache = RefCell::new(InlineCache::new(&text));
        
        document.inner = Some(DocumentInner {
            file_id,
            revision,
            text,
            token_cache,
            indent_cache,
            inline_cache,
            msg_cache,
            edit_group: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            outstanding_deltas: VecDeque::new(),
        });
        self.documents_by_file.insert(file_id, document_id);
        if document.should_be_destroyed {
            self.destroy_document_deferred(document_id, send_request);
        }
        document_id
    }
    
    pub fn destroy_document(
        &mut self,
        document_id: DocumentId,
        send_request: &mut dyn FnMut(CollabRequest),
    ) {
        let document = &mut self.documents[document_id];
        if document.inner.is_some() {
            self.destroy_document_deferred(document_id, send_request);
        } else {
            document.should_be_destroyed = true;
        }
    }
    
    fn destroy_document_deferred(
        &mut self,
        document_id: DocumentId,
        send_request: &mut dyn FnMut(CollabRequest),
    ) {
        let document = &mut self.documents[document_id];
        let inner = document.inner.as_ref().unwrap();
        let file_id = inner.file_id;
        self.documents_by_file.remove(&file_id);
        self.documents_by_path.remove(&document.path);
        self.documents.remove(&document_id);
        send_request(CollabRequest::CloseFile(file_id))
    }
    
    pub fn add_cursor(&mut self, session_id: SessionId, position: Position) {
        let session = &mut self.sessions[session_id];
        session.cursors.add(position);
        session.update_selections_and_carets();
        session.injected_char_stack.clear();
    }
    
    pub fn select_all(&mut self, session_id: SessionId) {
        let session = &mut self.sessions[session_id];
        let document = &self.documents[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        session.cursors.select_all(&document_inner.text);
        session.update_selections_and_carets();
        session.injected_char_stack.clear();
    }
    
    pub fn move_cursors_left(&mut self, session_id: SessionId, select: bool) {
        let session = &mut self.sessions[session_id];
        let document = &self.documents[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        session.cursors.move_left(&document_inner.text, select);
        session.update_selections_and_carets();
    }
    
    pub fn move_cursors_right(&mut self, session_id: SessionId, select: bool) {
        let session = &mut self.sessions[session_id];
        let document = &self.documents[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        session.cursors.move_right(&document_inner.text, select);
        session.update_selections_and_carets();
        session.injected_char_stack.clear();
    }
    
    pub fn move_cursors_up(&mut self, session_id: SessionId, select: bool) {
        let session = &mut self.sessions[session_id];
        let document = &self.documents[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        session.cursors.move_up(&document_inner.text, select);
        session.update_selections_and_carets();
        session.injected_char_stack.clear();
    }
    
    pub fn move_cursors_down(&mut self, session_id: SessionId, select: bool) {
        let session = &mut self.sessions[session_id];
        let document = &self.documents[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        session.cursors.move_down(&document_inner.text, select);
        session.update_selections_and_carets();
        session.injected_char_stack.clear();
    }
    
    pub fn move_cursors_to(&mut self, session_id: SessionId, position: Position, select: bool) {
        let session = &mut self.sessions[session_id];
        session.cursors.move_to(position, select);
        session.update_selections_and_carets();
        session.injected_char_stack.clear();
    }
    
    pub fn replace_text_direct(
        &mut self,
        session_id: SessionId,
        position: Position,
        size: Size,
        text: Text,
        send_request: &mut dyn FnMut(CollabRequest),
    ) {
        let session = &self.sessions[session_id];
        
        let mut builder = delta::Builder::new();
        builder.retain(position - Position::origin());
        builder.delete(size);
        builder.insert(text);
        let delta = builder.build();
        
        let mut offsets = Vec::new();
        for _ in &session.cursors {
            offsets.push(Size::zero());
        }
        
        self.edit(session_id, None, delta, &offsets, send_request);
    }
    
    pub fn insert_text(
        &mut self,
        session_id: SessionId,
        text: Text,
        send_request: &mut dyn FnMut(CollabRequest),
    ) {
        let session = &self.sessions[session_id];
        
        if let Some(ch) = text.as_lines().first().and_then( | line | line.first()) {
            if let Some(injected_char) = session.injected_char_stack.last() {
                if ch == injected_char {
                    let session = &mut self.sessions[session_id];
                    let document = &self.documents[session.document_id];
                    let document_inner = document.inner.as_ref().unwrap();
                    session.cursors.move_right(&document_inner.text, false);
                    session.update_selections_and_carets();
                    session.injected_char_stack.pop();
                    return;
                }
            }
        }
        
        let injected_char = text
            .as_lines()
            .first()
            .and_then( | line | line.first())
            .and_then( | ch | match ch {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            _ => None,
        });
        
        let mut offsets = Vec::new();
        
        let mut builder_0 = delta::Builder::new();
        let mut position = Position::origin();
        for cursor in &session.cursors {
            builder_0.retain(cursor.start() - position);
            builder_0.delete(cursor.end() - cursor.start());
            position = cursor.end();
        }
        let delta_0 = builder_0.build();
        
        let mut builder_1 = delta::Builder::new();
        let mut position = Position::origin();
        for cursor in &session.cursors {
            builder_1.retain(cursor.start() - position);
            builder_1.insert(text.clone());
            if let Some(injected_char) = injected_char {
                builder_1.insert(Text::from(vec![vec![injected_char]]));
            }
            offsets.push(text.len());
            position = cursor.end();
        }
        let delta_1 = builder_1.build();
        
        let (_, new_delta_1) = delta_0.clone().transform(delta_1);
        let delta = delta_0.compose(new_delta_1);
        
        self.edit(
            session_id,
            Some(EditGroup::Char),
            delta,
            &offsets,
            send_request,
        );
        
        let session = &mut self.sessions[session_id];
        if let Some(injected_char) = injected_char {
            session.injected_char_stack.push(injected_char);
        }
    }
    
    pub fn insert_newline(
        &mut self,
        session_id: SessionId,
        send_request: &mut dyn FnMut(CollabRequest),
    ) {
        let session = &self.sessions[session_id];
        let document = &self.documents[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        
        let mut offsets = Vec::new();
        
        let mut builder_0 = delta::Builder::new();
        let mut position = Position::origin();
        for cursor in &session.cursors {
            builder_0.retain(cursor.start() - position);
            builder_0.delete(cursor.end() - cursor.start());
            position = cursor.end();
        }
        let delta_0 = builder_0.build();
        
        let mut builder_1 = delta::Builder::new();
        let mut position = Position::origin();
        for cursor in &session.cursors {
            builder_1.retain(cursor.start() - position);
            
            let mut indent_count = 0;
            
            // This is sort of a hack. I designed the multiple cursor system so that all
            // cursors are applied at once, but operations such autoindenting are much
            // easier to implement if you apply one cursor at a time. In the future I'd
            // like to refactor the editor to always apply one cursor at a time, but in the
            // meantime I'll work around this problem by only performing autoindenting if
            // there is just a single cursor.
            if session.cursors.len() == 1 {
                let lines = document_inner.text.as_lines();
                if let Some((first_non_whitespace_line_before, first_non_whitespace_char_before)) =
                lines[cursor.start().line][..cursor.start().column]
                    .iter()
                    .rev()
                    .find( | ch | !ch.is_whitespace())
                    .map( | &ch | (cursor.start().line, ch))
                    .or_else( || {
                    (0..cursor.start().line).rev().find_map( | line | {
                        lines[line]
                            .iter()
                            .rev()
                            .find( | ch | !ch.is_whitespace())
                            .map( | &ch | (line, ch))
                    })
                })
                {
                    indent_count = (document_inner.indent_cache[first_non_whitespace_line_before] .leading_whitespace() .unwrap() + 3) / 4;
                    match first_non_whitespace_char_before {
                        '(' | '[' | '{' => indent_count += 1,
                        _ => {}
                    }
                }
            };
            let text = Text::from(vec![
                vec![],
                iter::repeat(' ').take(indent_count * 4).collect::<Vec<_ >> (),
            ]);
            let len = text.len();
            builder_1.insert(text);
            offsets.push(len);
            position = cursor.end();
        }
        let delta_1 = builder_1.build();
        
        let (_, new_delta_1) = delta_0.clone().transform(delta_1);
        let delta = delta_0.compose(new_delta_1);
        
        self.edit(session_id, None, delta, &offsets, send_request);
    }
    
    pub fn insert_backspace(
        &mut self,
        session_id: SessionId,
        send_request: &mut dyn FnMut(CollabRequest),
    ) {
        let session = &self.sessions[session_id];
        let document = &self.documents[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        
        let last_injected_char_inverse =
        session
            .injected_char_stack
            .last()
            .map( | last_injected_char | match last_injected_char {
            ')' => '(',
            ']' => '[',
            '}' => '{',
            _ => panic!(),
        });
        
        let mut offsets = Vec::new();
        
        let mut builder_0 = delta::Builder::new();
        let mut position = Position::origin();
        for cursor in &session.cursors {
            builder_0.retain(cursor.start() - position);
            builder_0.delete(cursor.end() - cursor.start());
            position = cursor.end();
        }
        let delta_0 = builder_0.build();
        
        let mut builder_1 = delta::Builder::new();
        let mut position = Position::origin();
        for cursor in &session.cursors {
            offsets.push(Size::zero());
            if cursor.head == cursor.tail {
                if cursor.start().column == 0 {
                    if cursor.start().line != 0 {
                        builder_1.retain(
                            Position {
                                line: cursor.start().line - 1,
                                column: document_inner.text.as_lines()[cursor.start().line - 1]
                                    .len(),
                            } -position,
                        );
                        builder_1.delete(Size {line: 1, column: 0});
                    }
                } else {
                    // This is sort of a hack. I designed the multiple cursor system so that all
                    // cursors are applied at once, but operations such as autodedenting are much
                    // easier to implement if you apply one cursor at a time. In the future I'd
                    // like to refactor the editor to always apply one cursor at a time, but in the
                    // meantime I'll work around this problem by only performing autodedenting if
                    // there is just a single cursor.
                    if session.cursors.len() == 1
                        && document_inner.text.as_lines()[cursor.start().line]
                    [..cursor.start().column]
                        .iter()
                        .all( | &ch | ch.is_whitespace())
                    {
                        if cursor.start().line == 0 {
                            builder_1.retain(
                                Position {
                                    line: cursor.start().line,
                                    column: 0,
                                } -position,
                            );
                            builder_1.delete(Size {
                                line: 0,
                                column: cursor.start().column,
                            })
                        } else {
                            builder_1.retain(
                                Position {
                                    line: cursor.start().line - 1,
                                    column: document_inner.text.as_lines()[cursor.start().line - 1]
                                        .len(),
                                } -position,
                            );
                            builder_1.delete(Size {
                                line: 1,
                                column: cursor.start().column,
                            });
                        }
                    } else {
                        builder_1.retain(
                            Position {
                                line: cursor.start().line,
                                column: cursor.start().column - 1,
                            } -position,
                        );
                        builder_1.delete(Size {line: 0, column: 1});
                        if let Some(last_injected_char_inverse) = last_injected_char_inverse {
                            if document_inner.text.as_lines()[cursor.start().line]
                            [cursor.start().column - 1]
                                == last_injected_char_inverse
                            {
                                builder_1.delete(Size {line: 0, column: 1});
                            }
                        }
                    }
                }
            }
            offsets.push(Size::zero());
            position = cursor.start();
        }
        let delta_1 = builder_1.build();
        
        let (_, new_delta_1) = delta_0.clone().transform(delta_1);
        let delta = delta_0.compose(new_delta_1);
        
        self.edit(
            session_id,
            Some(EditGroup::Backspace),
            delta,
            &offsets,
            send_request,
        );
    }
    
    pub fn delete(&mut self, session_id: SessionId, send_request: &mut dyn FnMut(CollabRequest)) {
        let session = &self.sessions[session_id];
        let document = &self.documents[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        
        let mut offsets = Vec::new();
        
        let mut builder_0 = delta::Builder::new();
        let mut position = Position::origin();
        for cursor in &session.cursors {
            builder_0.retain(cursor.start() - position);
            builder_0.delete(cursor.end() - cursor.start());
            position = cursor.end();
        }
        let delta_0 = builder_0.build();
        
        let mut builder_1 = delta::Builder::new();
        let mut position = Position::origin();
        for cursor in &session.cursors {
            if cursor.head != cursor.tail {
                continue;
            }
            builder_1.retain(cursor.start() - position);
            if cursor.start().column == document_inner.text.as_lines()[cursor.start().line].len() {
                if cursor.start().line == document_inner.text.as_lines().len() - 1 {
                    continue;
                }
                builder_1.delete(Size {line: 1, column: 0});
            } else {
                builder_1.delete(Size {line: 0, column: 1});
            }
            offsets.push(Size::zero());
            position = cursor.start();
        }
        let delta_1 = builder_1.build();
        
        let (_, new_delta_1) = delta_0.clone().transform(delta_1);
        let delta = delta_0.compose(new_delta_1);
        
        self.edit(
            session_id,
            Some(EditGroup::Backspace),
            delta,
            &offsets,
            send_request,
        );
    }
    
    fn edit(
        &mut self,
        session_id: SessionId,
        edit_group: Option<EditGroup>,
        delta: Delta,
        offsets: &[Size],
        send_request: &mut dyn FnMut(CollabRequest),
    ) {
        let session = &self.sessions[session_id];
        let document = &mut self.documents[session.document_id];
        let document_inner = document.inner.as_mut().unwrap();
        
        let inverse_delta = delta.clone().invert(&document_inner.text);
        let group_undo = edit_group.map_or(false, | edit_group | {
            document_inner
                .edit_group
                .map_or(false, | current_edit_group | current_edit_group == edit_group)
        });
        if group_undo {
            let edit = document_inner.undo_stack.pop().unwrap();
            document_inner.undo_stack.push(Edit {
                injected_char_stack: edit.injected_char_stack,
                cursors: edit.cursors,
                delta: inverse_delta.compose(edit.delta),
            });
        } else {
            document_inner.edit_group = edit_group;
            document_inner.undo_stack.push(Edit {
                injected_char_stack: session.injected_char_stack.clone(),
                cursors: session.cursors.clone(),
                delta: inverse_delta,
            });
        }
        document_inner.redo_stack.clear();
        
        let session = &mut self.sessions[session_id];
        session.apply_delta(&delta);
        session.apply_offsets(offsets);
        
        self.apply_delta(session_id, delta, send_request);
    }
    
    pub fn undo(&mut self, session_id: SessionId, send_request: &mut dyn FnMut(CollabRequest)) {
        let session = &self.sessions[session_id];
        let document = &mut self.documents[session.document_id];
        let document_inner = document.inner.as_mut().unwrap();
        if let Some(undo) = document_inner.undo_stack.pop() {
            document_inner.edit_group = None;
            
            let inverse_delta = undo.delta.clone().invert(&document_inner.text);
            document_inner.redo_stack.push(Edit {
                injected_char_stack: session.injected_char_stack.clone(),
                cursors: session.cursors.clone(),
                delta: inverse_delta,
            });
            
            let session = &mut self.sessions[session_id];
            session.cursors = undo.cursors;
            session.update_selections_and_carets();
            
            self.apply_delta(session_id, undo.delta, send_request);
        }
    }
    
    pub fn redo(&mut self, session_id: SessionId, send_request: &mut dyn FnMut(CollabRequest)) {
        let session = &self.sessions[session_id];
        let document = &mut self.documents[session.document_id];
        let document_inner = document.inner.as_mut().unwrap();
        if let Some(redo) = document_inner.redo_stack.pop() {
            document_inner.edit_group = None;
            
            let inverse_delta = redo.delta.clone().invert(&document_inner.text);
            document_inner.undo_stack.push(Edit {
                injected_char_stack: session.injected_char_stack.clone(),
                cursors: session.cursors.clone(),
                delta: inverse_delta,
            });
            
            let session = &mut self.sessions[session_id];
            session.cursors = redo.cursors;
            session.update_selections_and_carets();
            
            self.apply_delta(session_id, redo.delta, send_request);
        }
    }
    
    fn apply_delta(
        &mut self,
        session_id: SessionId,
        delta: Delta,
        send_request: &mut dyn FnMut(CollabRequest),
    ) {
        let session = &mut self.sessions[session_id];
        let document_id = session.document_id;
        
        let document = &self.documents[document_id];
        for other_session_id in document.session_ids.iter().cloned() {
            if other_session_id == session_id {
                continue;
            }
            
            let other_session = &mut self.sessions[other_session_id];
            other_session.apply_delta(&delta);
        }
        
        let document = &mut self.documents[document_id];
        document.apply_delta(delta.clone());
        document.schedule_apply_delta_request(delta, send_request);
    }
    
    pub fn handle_apply_delta_response(
        &mut self,
        file_id: TextFileId,
        send_request: &mut dyn FnMut(CollabRequest),
    ) {
        let document_id = self.documents_by_file[file_id];
        let document = &mut self.documents[document_id];
        let document_inner = document.inner.as_mut().unwrap();
        
        document_inner.outstanding_deltas.pop_front();
        document_inner.revision += 1;
        if let Some(outstanding_delta) = document_inner.outstanding_deltas.front() {
            send_request(CollabRequest::ApplyDelta(
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
        let document_id = self.documents_by_file[file_id];
        let document = &mut self.documents[document_id];
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
            let session = &mut self.sessions[session_id];
            session.apply_delta(&delta);
        }
        
        let document = &mut self.documents[document_id];
        let document_inner = document.inner.as_mut().unwrap();
        document_inner.revision += 1;
        document.apply_delta(delta);
        
        document_id
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct SessionId(pub LiveId);

pub struct Session {
    pub session_view: Option<EditorViewId>,
    pub injected_char_stack: Vec<char>,
    pub cursors: CursorSet,
    pub selections: RangeSet,
    pub carets: PositionSet,
    pub document_id: DocumentId,
}

impl Session {
    fn apply_delta(&mut self, delta: &Delta) {
        self.cursors.apply_delta(delta);
        self.update_selections_and_carets();
    }
    
    fn apply_offsets(&mut self, offsets: &[Size]) {
        self.cursors.apply_offsets(offsets);
        self.update_selections_and_carets();
    }
    
    fn update_selections_and_carets(&mut self) {
        self.selections = self.cursors.selections();
        self.carets = self.cursors.carets();
    }
}

pub struct BuilderMsgRange {
    pub range: Range,
    pub index: usize,
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct DocumentId(pub LiveId);

pub struct Document {
    pub session_ids: HashSet<SessionId>,
    pub should_be_destroyed: bool,
    pub path: PathBuf,
    //pub msg_ranges: Vec<BuilderMsgRange>,
    pub inner: Option<DocumentInner>,
}

impl Document {
    fn apply_delta(&mut self, delta: Delta) {
        let inner = self.inner.as_mut().unwrap();
        
        inner.token_cache.invalidate(&delta);
        inner.indent_cache.invalidate(&delta);
        inner.msg_cache.invalidate(&delta);
        inner.inline_cache.borrow_mut().invalidate(&delta);
        
        inner.text.apply_delta(delta);
        
        inner.token_cache.refresh(&inner.text);
        inner.indent_cache.refresh(&inner.text);
    }
    
    fn schedule_apply_delta_request(
        &mut self,
        delta: Delta,
        send_request: &mut dyn FnMut(CollabRequest),
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
                send_request(CollabRequest::ApplyDelta(
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
    pub msg_cache: MsgCache,
    pub inline_cache: RefCell<InlineCache>,
    pub edit_group: Option<EditGroup>,
    pub undo_stack: Vec<Edit>,
    pub redo_stack: Vec<Edit>,
    pub outstanding_deltas: VecDeque<Delta>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditGroup {
    Char,
    Backspace,
}

#[derive(Debug)]
pub struct Edit {
    pub injected_char_stack: Vec<char>,
    pub cursors: CursorSet,
    pub delta: Delta,
}

fn transform_edit_stack(edit_stack: &mut Vec<Edit>, delta: Delta) {
    let mut delta = delta;
    for edit in edit_stack.iter_mut().rev() {
        let edit_delta = mem::replace(&mut edit.delta, Delta::identity());
        edit.cursors.apply_delta(&delta);
        let (new_delta, new_edit_delta) = delta.transform(edit_delta);
        delta = new_delta;
        edit.delta = new_edit_delta;
    }
}
