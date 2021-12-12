use {
    std::collections::HashMap,
    crate::{
        editor_state::{
            EditorState,
        },
        code_editor::{
            token::TokenKind,
            token_cache::TokenCache,
            edit_info_cache::EditInfoCache,
            protocol::Request,
            code_editor_impl::{CodeEditorImpl, CodeEditorAction, VisibleLines}
        },
        editor_state::{
            SessionId
        },
    },
    makepad_widget::color_picker::ColorPicker,
    makepad_render::makepad_live_compiler::{TokenId, TextPos, LivePtr},
    makepad_render::*,
};

live_register!{
    use makepad_render::shader::std::*;
    
    RustEditor: {{RustEditor}} {
        editor_impl: {}
    }
}

#[derive(Live, LiveHook)]
pub struct RustEditor {
    editor_impl: CodeEditorImpl,
    color_picker: Option<LivePtr>,
    #[rust] visible_lines: VisibleLines,
    #[rust] color_pickers: HashMap<TokenId, ColorPicker>
}

impl EditInfoCache {
    
    pub fn refresh(&mut self, token_cache: &TokenCache, cx: &mut Cx) {
        if self.is_clean {
            return
        }
        self.is_clean = true;
        
        let lr_cp = cx.live_registry.clone();
        let lr = lr_cp.borrow();
        
        let file_id = LiveFileId(16);
        
        let live_file = &lr.live_files[file_id.to_index()];
        let expanded = &lr.expanded[file_id.to_index()];
        
        for (line, line_cache) in self.lines.iter_mut().enumerate() {
            if line_cache.is_clean { // line not dirty
                continue
            }
            line_cache.is_clean = true;
            if line_cache.live_ptrs.len() != 0 {
                panic!();
            }
            let tokens_line = &token_cache[line];
            let mut column = 0;
            for (editor_token_index, token) in tokens_line.tokens().iter().enumerate() {
                if let TokenKind::Identifier = token.kind {
                    if let Some(live_token_index) = live_file.document.find_token_by_pos(TextPos {line: line as u32, column}) {
                        let match_token_id = makepad_live_compiler::TokenId::new(file_id, live_token_index);
                        if let Some(node_index) = expanded.nodes.first_node_with_token_id(match_token_id) {
                            let live_ptr = LivePtr {file_id, index: node_index as u32};
                            
                            // and metadata to spawn up a UI element
                            line_cache.live_ptrs.push((editor_token_index, live_ptr));
                        }
                    }
                }
                column += token.len as u32;
            }
        }
    }
}

impl RustEditor {
    
    pub fn set_session_id(&mut self, session_id: Option<SessionId>) {
        self.editor_impl.session_id = session_id;
    }
    
    pub fn session_id(&self) -> Option<SessionId> {
        self.editor_impl.session_id
    }
    
    pub fn redraw(&self, cx: &mut Cx) {
        self.editor_impl.redraw(cx);
    }
    
    pub fn draw(&mut self, cx: &mut Cx, state: &EditorState) {
        if let Ok((document_inner, session)) = self.editor_impl.begin(cx, state) {
            
            let mut edit_info_cache = document_inner.edit_info_cache.borrow_mut();
            edit_info_cache.refresh(&document_inner.token_cache, cx);
            
            // first we generate the layout structure
            let lr_cp = cx.live_registry.clone();
            let lr = lr_cp.borrow();
            self.editor_impl.calc_visible_lines(cx, document_inner, &mut self.visible_lines, | _cx, line_index | {
                let edit_info = &edit_info_cache[line_index];
                return 0.0;
                for (_token_index, live_ptr) in &edit_info.live_ptrs {
                    let _node = lr.ptr_to_node(*live_ptr);
                    return 100.0
                }
                return 0.0
            });
            
            self.editor_impl.draw_selections(
                cx,
                &session.selections,
                &document_inner.text,
                &self.visible_lines,
            );
            
            self.editor_impl.draw_indent_guides(
                cx,
                &document_inner.indent_cache,
                &self.visible_lines,
            );
            
            self.editor_impl.draw_carets(
                cx,
                &session.selections,
                &session.carets,
                &self.visible_lines
            );
            
            // alright great. now we can draw the text
            self.editor_impl.draw_text(
                cx,
                &document_inner.text,
                &document_inner.token_cache,
                &self.visible_lines,
            );
            
            self.editor_impl.draw_current_line(cx,  &self.visible_lines, session.cursors.last());

            self.editor_impl.draw_linenums(cx, &self.visible_lines, session.cursors.last());

            
            self.editor_impl.end(cx, &self.visible_lines);
        }
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        event: &mut Event,
        send_request: &mut dyn FnMut(Request),
        dispatch_action: &mut dyn FnMut(&mut Cx, CodeEditorAction),
    ) {
        self.editor_impl.handle_event(cx, state, event, &self.visible_lines, send_request, dispatch_action);
    }
}

