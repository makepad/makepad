use {
    crate::{
        makepad_platform::*,
        makepad_editor_core::{
            delta::{self, Delta},
            position::Position,
            position_set::PositionSet,
            range_set::RangeSet,
            size::Size,
            text::Text,
        },
        build::build_protocol::BuildMsg,
        code_editor::{
            cursor_set::CursorSet,
            indent_cache::IndentCache,
            msg_cache::MsgCache,
        },
        rust_editor::rust_tokenizer::token_cache::TokenCache,
        makepad_collab_protocol::{
            CollabRequest,
            TextFileId,
            unix_path::UnixPathBuf,
        },
        editors::EditorViewId,
    },

    std::{
        collections::{HashMap, HashSet, VecDeque},
        iter,
        mem,
    },
};

/// This type contains all the state for the code editor that is not directly related to the UI. It
/// contains a `Session` for each open file tab, and a `Document` for each open file.
///
/// Each session refers to exactly one document, and each document is referred to by one or more
/// session. In addition, each document refers back to the sessions that refer to it. The resulting
/// data structure is a cyclic graph. These are hard to represent directly in Rust, so we use the
/// typical approach of storing the sessions and documents in some kind of arena, and then referring
/// to them by id.
///
/// When the user clicks on a file in the file tree, we create both a new session and a new document
/// for this file. Subsequent attempts to open the same file should create a new session that refers
/// to the same document. To figure out whether we already have a document for a given file, we
/// maintain a mapping from file paths to document ids.
///
/// When a document is first created, it starts out in an uninitialized state, while we fetch its
/// contents from the collab server. Although the initial message to open the file for a document and
/// fetch its content uses a file path, subsequent messages to the collab server for the same
/// document uses a file id (this is analagous to how opening a file uses a path, but subsequent
/// operations on that file use a file handle). We maintain a map from file ids to document ids so
/// that when we receive a response or notification from the collab server with a given file, we can
/// find the corresponding document.
#[derive(Default)]
pub struct EditorState {
    /// An arena for all sessions in this code editor.
    pub sessions: LiveIdMap<SessionId, Session>,
    /// An arena for all documents in this code editor.
    pub documents: LiveIdMap<DocumentId, Document>,
    /// A map from file paths to document ids (see above for why this is needed)
    pub documents_by_path: HashMap<UnixPathBuf, DocumentId>,
    /// A map from file ids to document ids (see above for why this is needed)
    pub documents_by_file: LiveIdMap<TextFileId, DocumentId>,
    /// The queue of outstanding documents for this code editor. A document is outstanding if it has
    /// been created, but we have not yet received its contents from the collab server.
    pub outstanding_document_queue: VecDeque<DocumentId>,
    pub messages: Vec<BuildMsg>,
}

impl EditorState {
    /// Creates a new `EditorState`.
    pub fn new() -> EditorState {
        EditorState::default()
    }

    /// Either gets or creates the document for the file with the given `path`, and then creates a
    /// session that refers to this document. Returns the id of the newly created session.
    ///
    /// If the document did not yet exist, the `send_request` callback is used to send a request to
    /// the collab server to open the document's file and fetch its contents.
    pub fn create_session(
        &mut self,
        path: UnixPathBuf,
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

    /// Destroys the session with the given `session_id`. If this session was the last session that
    /// referred to its document, the document is scheduled to be destroyed as well.
    ///
    /// If the document is already initialized, it is destroyed immediately, and the `send_request`
    /// callback is used to send a request to the collab server to close the document's file.
    /// Otherwise, destroying the document is deferred until it becomes initialized.
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

    /// Either gets or creates the document for the file with the given `path`.
    ///
    /// If the document did not yet exist, it is created in an uninitialized state, and the
    /// `send_request` callback is used to send a request to the collab server to open the
    /// document's file and fetch its contents.
    pub fn get_or_create_document(
        &mut self,
        path: UnixPathBuf,
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

    /// Handles an open file response from the collab server.
    /// 
    /// This is usually received in response to a request to the collab server to open the
    /// document's file and fetch its contents. At this point, we can fully initialize the
    /// document. If the document was scheduled to be destroyed while we were waiting for
    /// the document to become fully initialized, it can now be destroyed, and the `send_request`
    /// callback is used to send a request to the collab server to close the document's file.
    pub fn handle_open_file_response(
        &mut self,
        file_id: TextFileId,
        revision: u32,
        text: Text,
        send_request: &mut dyn FnMut(CollabRequest),
    ) -> DocumentId {
        let document_id = self.outstanding_document_queue.pop_front().unwrap();
        let document = &mut self.documents[document_id];
        let token_cache = TokenCache::new(&text);
        let indent_cache = IndentCache::new(&text);
        let msg_cache = MsgCache::new(&text);

        document.inner = Some(DocumentInner {
            file_id,
            revision: revision as usize,
            text,
            token_cache,
            indent_cache,
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

    /// Schedules the document with the given `document_id` to be destroyed.
    ///
    /// If the document is already initialized, it is destroyed immediately, and the `send_request`
    /// callback is used to send a request to the collab server to close the document's file.
    /// Otherwise, destroying the document is deferred until it becomes initialized.
    pub fn destroy_document(
        &mut self,
        document_id: DocumentId,
        send_request: &mut dyn FnMut(CollabRequest),
    ) {
        let document = &mut self.documents[document_id];
        if document.inner.is_some() {
            // The document is already initialized, so destroy it immediately.
            self.destroy_document_deferred(document_id, send_request);
        } else {
            // The document is not yet initialized, so scheduled it to be destroyed later.
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

    /// Adds a cursor to the cursor set of the session with the given `session_id`, wotj tje caret
    /// at the given position.
    pub fn add_cursor(&mut self, session_id: SessionId, position: Position) {
        let session = &mut self.sessions[session_id];
        session.cursors.add(position);
        session.update_selections_and_carets();
        session.injected_char_stack.clear();
    }

    // Replaces the cursor set of the session with the given `session_id` with a single cursor, such
    // that the caret is at the start of the contents of the document referred to by this session,
    // given `text` and the selection covers the entire given `text`.
    pub fn select_all(&mut self, session_id: SessionId) {
        let session = &mut self.sessions[session_id];
        let document = &self.documents[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        session.cursors.select_all(&document_inner.text);
        session.update_selections_and_carets();
        session.injected_char_stack.clear();
    }

    /// Move all cursors in the cursor set of the session with the given `session_id` one column to
    /// the left.
    pub fn move_cursors_left(&mut self, session_id: SessionId, select: bool) {
        let session = &mut self.sessions[session_id];
        let document = &self.documents[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        session.cursors.move_left(&document_inner.text, select);
        session.update_selections_and_carets();
    }

    /// Move all cursors in the cursor set of the session with the given `session_id` one column to
    /// the left.
    pub fn move_cursors_right(&mut self, session_id: SessionId, select: bool) {
        let session = &mut self.sessions[session_id];
        let document = &self.documents[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        session.cursors.move_right(&document_inner.text, select);
        session.update_selections_and_carets();
        session.injected_char_stack.clear();
    }

    /// Move all cursors in the cursor set of the session with the given `session_id` one line up.
    pub fn move_cursors_up(&mut self, session_id: SessionId, select: bool) {
        let session = &mut self.sessions[session_id];
        let document = &self.documents[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        session.cursors.move_up(&document_inner.text, select);
        session.update_selections_and_carets();
        session.injected_char_stack.clear();
    }

    /// Move all cursors in the cursor set of the session with the given `session_id` one line down.
    pub fn move_cursors_down(&mut self, session_id: SessionId, select: bool) {
        let session = &mut self.sessions[session_id];
        let document = &self.documents[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        session.cursors.move_down(&document_inner.text, select);
        session.update_selections_and_carets();
        session.injected_char_stack.clear();
    }

    /// Move all cursors in the cursor set of the session with the given `session_id` to the given
    /// `position`.
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

    /// For each cursor in the cursor set of the session with the given `session_id`, removes the
    /// selection of the cursor, and then inserts the given text at the caret of the cursor.
    pub fn insert_text(
        &mut self,
        session_id: SessionId,
        text: Text,
        send_request: &mut dyn FnMut(CollabRequest),
    ) {
        let session = &self.sessions[session_id];

        // If the character to be inserted is the same as an automatically injected character, we
        // skip over the automatically injected character rather than insert the same character
        // again.
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

        // If the character to be inserted is an opening delimiter, we automatically insert the
        // corresponding closing delimiter.
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

        // Build a delta and a list of cursor offsets for the edit operation.
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
                builder_1.insert(Text::from_lines(vec![vec![injected_char]]));
            }
            offsets.push(text.len());
            position = cursor.end();
        }
        let delta_1 = builder_1.build();

        let (_, new_delta_1) = delta_0.clone().transform(delta_1);
        let delta = delta_0.compose(new_delta_1);

        // Apply the edit operation.
        self.edit(
            session_id,
            Some(EditGroup::Char),
            delta,
            &offsets,
            send_request,
        );

        // If we automatically inserted a character, store it on the injected character stack.
        let session = &mut self.sessions[session_id];
        if let Some(injected_char) = injected_char {
            session.injected_char_stack.push(injected_char);
        }
    }

    /// For each cursor in the cursor set of the session with the given `session_id`, removes the
    /// selection of the cursor, and then inserts a newline at the caret of the cursor.
    pub fn insert_newline(
        &mut self,
        session_id: SessionId,
        send_request: &mut dyn FnMut(CollabRequest),
    ) {
        let session = &self.sessions[session_id];
        let document = &self.documents[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();

        // Build a delta and a list of cursor offsets for the edit operation.
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

            // Automatically indent the text to be inserted.
            //
            // This is sort of a hack. I designed multiple cursors so that all cursors are applied
            // at once, but operations such as autoindenting are much asier to implement if you
            // apply one cursor at a time.
            //
            // This should be refactored in the future, by in the meantime we work around the 
            // problem by only performing autoindenting if there is just a single cursor.
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
            let text = Text::from_lines(vec![
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

        // Apply the edit operation.
        self.edit(session_id, None, delta, &offsets, send_request);
    }

    /// For each cursor in the cursor set of the session with the given `session_id`, if the
    /// selection of the cursor is empty, inserts a backspace at the caret of the cursor.
    /// Otherwise, removes the selection of the cursor.
    pub fn insert_backspace(
        &mut self,
        session_id: SessionId,
        send_request: &mut dyn FnMut(CollabRequest),
    ) {
        let session = &self.sessions[session_id];
        let document = &self.documents[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();

        // If we automatically injected a character during the previous edit operation, the inverse
        // of that character is the character that triggered the injection.
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

        // Build a delta and a list of cursor offsets for the edit operation.
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
                    // Automatically dedent indented text.
                    //
                    // This is sort of a hack. I designed multiple cursors so that all cursors are
                    // applied at once, but operations such as autoindenting are much asier to
                    // implement if you apply one cursor at a time.
                    //
                    // This should be refactored in the future, by in the meantime we work around
                    // the problem by only performing autoindenting if there is just a single cursor.
                    let lines = &document_inner.text.as_lines()[cursor.start().line];
                    
                    if session.cursors.len() == 1
                        && lines[..cursor.start().column]
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
                                column: cursor.start().column as u32,
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
                                column: cursor.start().column as u32,
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

                        // If we're deleting the character that triggered an automatic character
                        // injection, we also remove the automatically injected character.
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

        // Apply the edit operation.
        self.edit(
            session_id,
            Some(EditGroup::Backspace),
            delta,
            &offsets,
            send_request,
        );
    }

    /// For each cursor in the cursor set of the session with the given `session_id`, if the
    /// selection of the cursor is empty, deletes the character at the caret of the cursor.
    /// Otherwise, removes the selection of the cursor.
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

    // Applies an edit operation to the text.
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

        // Compute the inverse delta so we can put it on the undo stack.
        let inverse_delta = delta.clone().invert(&document_inner.text);

        // Figure out if the edit operation should be grouped with the previous one on the undo
        // stack.
        let group_undo = edit_group.map_or(false, | edit_group | {
            document_inner
                .edit_group
                .map_or(false, | current_edit_group | current_edit_group == edit_group)
        });

        if group_undo {
            // Group the edit operation with the previous one on the undo stack.
            let edit = document_inner.undo_stack.pop().unwrap();
            document_inner.undo_stack.push(Edit {
                injected_char_stack: edit.injected_char_stack,
                cursors: edit.cursors,
                delta: inverse_delta.compose(edit.delta),
            });
        } else {
            // Add the edit operation to the undo stack.
            document_inner.edit_group = edit_group;
            document_inner.undo_stack.push(Edit {
                injected_char_stack: session.injected_char_stack.clone(),
                cursors: session.cursors.clone(),
                delta: inverse_delta,
            });
        }
        document_inner.redo_stack.clear();

        // Apply the edit operation to the session.
        let session = &mut self.sessions[session_id];
        session.apply_delta(&delta);
        session.apply_offsets(offsets);

        // Apply the edit operation to the document.
        self.apply_delta(session_id, delta, send_request);
    }

    /// Undoes the last edit operation.
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

    /// Redoes the last edit operation.
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

    /// Handles and apply delta response from the collab server.
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
                document_inner.revision as u32,
                outstanding_delta.clone(),
            ));
        }
    }

    /// Handles a notification from the collab server that a remote delta was applied.
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

/// An id for a `Session`. This can be used to refer to a session without borrowing it.
#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct SessionId(pub LiveId);

/// A `Session` represents an open file tab in the code editor. Each session refers to exactly one
/// `Document`, and stores the state that is unique for each session. Among other things, this
/// includes the set of cursors for the session.
pub struct Session {
    pub session_view: Option<EditorViewId>,
    /// The stack of characters that were automatically injected after the cursor during the last few
    /// edit operations. If the next character to be typed is the same as an automatically injected
    /// character, we skip over the automatically injected character rather than insert the same
    /// character again.
    pub injected_char_stack: Vec<char>,
    /// The set of cursors for this session.
    pub cursors: CursorSet,
    /// The minimal set of non-overlapping ranges that covers the selections of all cursors for this
    /// session. This information can be derived from the set of cursors, but is cached here
    /// because it is somewhat expensive to compute.
    pub selections: RangeSet,
    /// The minimal set of positions that covers the carets of all cursors for this session. This
    /// inforation can be derived from the set of cursors, but is cached here because it is somewhat
    /// expensive to compute.
    pub carets: PositionSet,
    /// The document referred to by this session.
    pub document_id: DocumentId,
}

impl Session {
    // Applies a delta to this session. This applies the delta to the set of cursors for this
    // session, and then recomputes the derived information for this set of cursors.
    fn apply_delta(&mut self, delta: &Delta) {
        self.cursors.apply_delta(delta);
        self.update_selections_and_carets();
    }

    // Applies an offset to each cursor. This applies the offsets to the set of cursors for this
    // session, and then recomputes the derived information for this set of cursor.
    fn apply_offsets(&mut self, offsets: &[Size]) {
        self.cursors.apply_offsets(offsets);
        self.update_selections_and_carets();
    }

    // Recomputes the derived information for the set of cursors for this session.
    fn update_selections_and_carets(&mut self) {
        self.selections = self.cursors.selections();
        self.carets = self.cursors.carets();
    }
}

/// An id for a `Document`. This can be used to refer to a document without borrowing from it.
#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct DocumentId(pub LiveId);

/// A `Document` represents an open file in the code editor. Each document is referred to by one or
/// more `Session`s, and stores the state that is shared between each session. Among other things,
/// this includes the text for this document, the token state, and the undo state.
pub struct Document {
    /// The sessions that refer to this document.
    pub session_ids: HashSet<SessionId>,
    pub should_be_destroyed: bool,
    pub path: UnixPathBuf,
    pub inner: Option<DocumentInner>,
}

impl Document {
    // Applies a delta to this `Document`. This applies the delta to the text for this document, and
    // then invalidates the cached data for this text.
    fn apply_delta(&mut self, delta: Delta) {
        let inner = self.inner.as_mut().unwrap();

        inner.token_cache.invalidate(&delta);
        inner.indent_cache.invalidate(&delta);
        inner.msg_cache.invalidate(&delta);

        inner.text.apply_delta(delta);

        inner.token_cache.refresh(&inner.text);
        inner.indent_cache.refresh(&inner.text);
    }

    // Schedules a request to the collab server to apply this delta to the remote document.
    //
    // The actual request is sent by calling the `send_request` callback. However, you should always
    // call this function rather than call `send_request` directly, so the document has a chance to
    // update its queue of outstanding deltas.
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
                    inner.revision as u32,
                    inner.outstanding_deltas.front().unwrap().clone(),
                ));
            }
        }
    }
}

pub struct DocumentInner {
    pub file_id: TextFileId,
    /// The revision of this document.
    pub revision: usize,
    /// The text for this document
    pub text: Text,
    /// A line-based cache containing the tokens for each line.
    pub token_cache: TokenCache,
    /// A line-based cache containing the indent level for each line.
    pub indent_cache: IndentCache,
    pub msg_cache: MsgCache,
    //// Whether the last typed character was a backspace character or a non-backspace character.
    pub edit_group: Option<EditGroup>,
    /// The undo stack for this document.
    pub undo_stack: Vec<Edit>,
    /// The redo stack for this document.
    pub redo_stack: Vec<Edit>,
    /// The queue of outstanding deltas for this document. A delta is outstanding if it has been
    /// applied to the local document, but we have not yet received confirmation from the collab
    /// server that it has been applied to the remote document.
    pub outstanding_deltas: VecDeque<Delta>,
}

/// An `EditGroup` keeps track of whether the last typed character was a backspace character or a
/// non-backspace character.
///
/// This is necessary because when a sequence of backspace characters is typed, they should be
/// grouped together into a single edit operation. Similarly, when a sequence of non-backspace
/// characters is typed, they should be grouped together into a single operation. However,
/// alternating sequences of backspace and non-backspace characters should not be grouped together.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditGroup {
    /// Non-backspace character
    Char,
    /// Backspace character
    Backspace,
}

/// An `Edit` represents an atomic edit operation.
///
/// The primary purpose of this type is to be stored on the undo stack.
#[derive(Debug)]
pub struct Edit {
    /// The state of the injected character stack at the time of this `Edit`. We store this
    /// explicitly because it cannot be recovered from the `delta` alone.
    pub injected_char_stack: Vec<char>,
    /// The state of the cursor set at the time of this `Edit`. We store this explicitly
    /// because it cannot be recovered from the `delta` alone.
    pub cursors: CursorSet,
    /// A delta representing the change made to the text by this `Edit`.
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
