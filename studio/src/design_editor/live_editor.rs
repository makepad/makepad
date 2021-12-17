use {
    std::collections::{HashSet, HashMap,},
    crate::{
        editor_state::{
            EditorState,
            DocumentInner
        },
        code_editor::{
            token::TokenKind,
            token_cache::TokenCache,
            edit_info_cache::EditInfoCache,
            protocol::Request,
            code_editor_impl::{CodeEditorImpl, CodeEditorAction, LinesLayout}
        },
        design_editor::{
            inline_widget::*,
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
    
    LiveEditor: {{LiveEditor}} {
        color_picker: ColorPicker,
        widget_layout: Layout {
            align: Align {fx: 0.2, fy: 0.},
            padding: Padding {l: 0, t: .0, r: 0, b: 0}
        }
        editor_impl: {}
    }
}

#[derive(Copy, Clone, Hash, PartialEq, Eq)]
pub struct WidgetIdent(LivePtr, LiveType);

pub struct Widget {
    inline_widget: Box<dyn InlineWidget>
}

#[derive(Live, LiveHook)]
pub struct LiveEditor {
    editor_impl: CodeEditorImpl,
    
    color_picker: Option<LivePtr>,
    
    widget_layout: Layout,
    
    #[rust] lines_layout: LinesLayout,
    
    #[rust] widget_draw_order: Vec<(usize, WidgetIdent)>,
    #[rust] visible_widgets: HashSet<WidgetIdent>,
    #[rust] widgets: HashMap<WidgetIdent, Widget>,
}

impl EditInfoCache {
    
    pub fn refresh(&mut self, token_cache: &TokenCache, cx: &mut Cx) {
        if self.is_clean {
            return
        }
        self.is_clean = true;
        
        let live_registry_rc = cx.live_registry.clone();
        let live_registry = live_registry_rc.borrow();
        
        let file_id = LiveFileId(10);
        
        let live_file = &live_registry.live_files[file_id.to_index()];
        let expanded = &live_registry.expanded[file_id.to_index()];
        
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
                            
                            line_cache.live_ptrs.push((editor_token_index, live_ptr));
                        }
                    }
                }
                column += token.len as u32;
            }
        }
    }
}

impl LiveEditor {
    
    pub fn set_session_id(&mut self, session_id: Option<SessionId>) {
        self.editor_impl.session_id = session_id;
    }
    
    pub fn session_id(&self) -> Option<SessionId> {
        self.editor_impl.session_id
    }
    
    pub fn redraw(&self, cx: &mut Cx) {
        self.editor_impl.redraw(cx);
    }
    
    pub fn draw_line_editors(&mut self, _cx: &mut Cx, _document_inner: &DocumentInner) {
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
            
            let widgets = &mut self.widgets;
            
            let visible_widgets = &mut self.visible_widgets;
            visible_widgets.clear();
            
            let widget_draw_order = &mut self.widget_draw_order;
            widget_draw_order.clear();
            
            let registries = cx.registries.clone();
            let widget_registry = registries.get::<CxInlineWidgetRegistry>();
            
            self.editor_impl.calc_lines_layout(cx, document_inner, &mut self.lines_layout, | cx, line_index, start_y, viewport_start, viewport_end | {

                let edit_info = &edit_info_cache[line_index];
                let mut max_height = 0.0f32;

                for (_token_index, live_ptr) in &edit_info.live_ptrs {
                    let node = live_registry.ptr_to_node(*live_ptr);

                    if let Some(matched) = widget_registry.match_inline_widget(&live_registry, node) {
                        max_height = max_height.max(matched.height);

                        if start_y + matched.height > viewport_start && start_y < viewport_end {
                            // lets spawn it
                            let ident = WidgetIdent(*live_ptr, matched.live_type);
                            widgets.entry(ident).or_insert_with( || {
                                Widget {
                                    inline_widget: widget_registry.new(cx, matched.live_type).unwrap(),
                                }
                            });
                            visible_widgets.insert(ident);
                            widget_draw_order.push((line_index, ident));
                        }
                    }
                }
                return max_height
            });
            
            widgets.retain( | ident, _ | visible_widgets.contains(ident));
            
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
            
            let mut last_line = None;
            
            let line_num_geom = vec2(self.editor_impl.line_num_width, 0.0);
            let origin = cx.get_turtle_pos() + line_num_geom;
            let size = cx.get_turtle_size() - line_num_geom;
            for (line, ident) in &self.widget_draw_order {
                if Some(line) != last_line { // start a new draw segment with the turtle
                    if last_line.is_some() {
                        cx.end_turtle();
                    }
                    // lets look at the line height
                    let ll = &self.lines_layout.lines[*line];
                    cx.begin_turtle(Layout {
                        abs_origin: Some(vec2(origin.x, origin.y + ll.start_y + ll.text_height)),
                        abs_size: Some(vec2(size.x, ll.widget_height)),
                        ..self.widget_layout
                    });
                }
                let widget = self.widgets.get_mut(ident).unwrap();
                widget.inline_widget.draw_inline(cx);
                last_line = Some(line)
            }
            if last_line.is_some() {
                cx.end_turtle();

            }
            
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
        for widget in self.widgets.values_mut(){
            widget.inline_widget.handle_inline_event(cx, event);
        }
        self.editor_impl.handle_event(cx, state, event, &self.lines_layout, send_request, dispatch_action);
    }
}

