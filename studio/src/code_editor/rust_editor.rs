use {
    std::collections::{
        HashSet,
        HashMap,
    },
    crate::{
        editor_state::{
            EditorState,
            DocumentInner
        },
        code_editor::{
            live_edit_widget::*,
            token::TokenKind,
            token_cache::TokenCache,
            edit_info_cache::EditInfoCache,
            protocol::Request,
            code_editor_impl::{CodeEditorImpl, CodeEditorAction, LinesLayout}
        },
        editor_state::{
            SessionId
        },
    },
    makepad_render::makepad_live_compiler::{TextPos, LivePtr},
    makepad_render::*,
};

live_register!{
    use makepad_render::shader::std::*;
    
    RustEditor: {{RustEditor}} {
        color_picker: ColorPicker,
        editor_impl: {}
    }
}
pub trait LineEditor : std::any::Any {}

#[derive(Live, LiveHook)]
pub struct RustEditor {
    editor_impl: CodeEditorImpl,
    
    color_picker: Option<LivePtr>,
    
    #[rust] lines_layout: LinesLayout,
    #[rust] visible_editors: HashSet<LivePtr>,
//    #[rust] gc_editors: HashSet<LivePtr>,
    #[rust] live_editors: HashMap<LivePtr, Box<dyn LiveEditWidget >>,
}

impl EditInfoCache {
    
    pub fn refresh(&mut self, token_cache: &TokenCache, cx: &mut Cx) {
        if self.is_clean {
            return
        }
        self.is_clean = true;
        
        let lr_cp = cx.live_registry.clone();
        let lr = lr_cp.borrow();
        
        let file_id = LiveFileId(23);
        
        let live_file = &lr.live_files[file_id.to_index()];
        let expanded = &lr.expanded[file_id.to_index()];
        
        if self.lines.len() != token_cache.len() {
            panic!();
        }
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
                            
                            //println!("FOUND TOKEN {:?}", expanded.nodes[node_index]);
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
    
    pub fn draw_line_editors(&mut self, cx: &mut Cx, document_inner: &DocumentInner) {
        // alrigth so now what.
        // we have to go draw our line editors
        
    }
    
    pub fn draw(&mut self, cx: &mut Cx, state: &EditorState) {
        if let Ok((document_inner, session)) = self.editor_impl.begin(cx, state) {
            
            let mut edit_info_cache = document_inner.edit_info_cache.borrow_mut();
            edit_info_cache.refresh(&document_inner.token_cache, cx);
            
            // first we generate the layout structure
            let live_registry_rc = cx.live_registry.clone();
            let live_registry = live_registry_rc.borrow();
            
            let mut live_editors = &mut self.live_editors;
            let mut visible_editors = &mut self.visible_editors;
            let color_picker = self.color_picker;
            
            self.editor_impl.calc_lines_layout(cx, document_inner, &mut self.lines_layout, | cx, line_index, start_y, viewport_start, viewport_end | {
                let edit_info = &edit_info_cache[line_index];
                let mut max_height = 0.0f32;
                for (_token_index, live_ptr) in &edit_info.live_ptrs {
                    let node = live_registry.ptr_to_node(*live_ptr);
                    
                    if let Some((height, editor_type)) = cx.registries.match_live_edit_widget(&live_registry, node){
                        
                        
                        max_height = max_height.max(height);
                    }

                    let height = match node.value {
                        LiveValue::Color(_) => {
                            100.0//ColorPicker::default_height()
                        }
                        _ => 0.0
                    };
                    
                    if start_y + height > viewport_start && start_y < viewport_end {
                        match node.value {
                            LiveValue::Color(_) => {
                                visible_editors.insert(*live_ptr);
                                //line_editors.entry(*live_ptr).or_insert_with( || {
                                //    Box::new(ColorPicker::new_from_ptr(cx, color_picker.unwrap()))
                                //});
                            }
                            _ => ()
                        }
                    }
                    max_height = max_height.max(height);
                }
                return max_height
            });
            
            self.editor_impl.draw_selections(
                cx,
                &session.selections,
                &document_inner.text,
                &self.lines_layout,
            );
            
            self.editor_impl.draw_indent_guides(
                cx,
                &document_inner.indent_cache,
                &self.lines_layout,
            );
            
            self.editor_impl.draw_carets(
                cx,
                &session.selections,
                &session.carets,
                &self.lines_layout
            );

            // ok so now what. we should draw the visible editors
            // however 'where' do we draw them
            
            // alright great. now we can draw the text
            self.editor_impl.draw_text(
                cx,
                &document_inner.text,
                &document_inner.token_cache,
                &self.lines_layout,
            );
            
            self.editor_impl.draw_current_line(cx, &self.lines_layout, session.cursors.last());
            self.editor_impl.draw_linenums(cx, &self.lines_layout, session.cursors.last());
            
            
            self.editor_impl.end(cx, &self.lines_layout);
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
        self.editor_impl.handle_event(cx, state, event, &self.lines_layout, send_request, dispatch_action);
    }
}

