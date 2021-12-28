use {
    std::collections::{HashSet, HashMap,},
    crate::{
        editor_state::{
            EditorState,
            DocumentInner
        },
        code_editor::{
            protocol::Request,
            code_editor_impl::{CodeEditorImpl, CodeEditorAction, LinesLayout}
        },
        design_editor::{
            inline_widget::*,
            inline_cache::InlineEditBind
        },
        editor_state::{
            SessionId
        },
    },
    makepad_render::makepad_live_compiler::{LivePtr},
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
    bind: InlineEditBind,
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
    
    pub fn draw_widgets(&mut self, cx: &mut Cx) {
        let live_registry_rc = cx.live_registry.clone();
        let live_registry = live_registry_rc.borrow();
        
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
            
            widget.inline_widget.draw_inline(cx, &live_registry, widget.bind);
            
            last_line = Some(line)
        }
        if last_line.is_some() {
            cx.end_turtle();
        }
    }
    
    pub fn calc_layout_with_widgets(&mut self, cx: &mut Cx, path: &str, document_inner: &DocumentInner) {
        let mut inline_cache = document_inner.inline_cache.borrow_mut();
        inline_cache.refresh(cx, path, &document_inner.token_cache);
        
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
        
        self.editor_impl.calc_lines_layout(cx, document_inner, &mut self.lines_layout, | cx, line, start_y, viewport_start, viewport_end | {
            
            let edit_info = &inline_cache[line];
            let mut max_height = 0.0f32;
            
            for bind in &edit_info.items {
                if let Some(matched) = widget_registry.match_inline_widget(&live_registry, *bind) {
                    max_height = max_height.max(matched.height);
                    
                    if start_y + matched.height > viewport_start && start_y < viewport_end {
                        // lets spawn it
                        let ident = WidgetIdent(bind.live_ptr, matched.live_type);
                        widgets.entry(ident).or_insert_with( || {
                            Widget {
                                bind: *bind,
                                inline_widget: widget_registry.new(cx, matched.live_type).unwrap(),
                            }
                        });
                        visible_widgets.insert(ident);
                        widget_draw_order.push((line, ident));
                    }
                }
            }
            return max_height
        });
        
        widgets.retain( | ident, _ | visible_widgets.contains(ident));
    }
    
    pub fn draw(&mut self, cx: &mut Cx, state: &EditorState) {
        if let Ok((document, document_inner, session)) = self.editor_impl.begin(cx, state) {
            let path = document.path.clone().into_os_string().into_string().unwrap();
            
            self.calc_layout_with_widgets(cx, &path, document_inner);
            
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
            
            
            
            // alright great. now we can draw the text
            self.editor_impl.draw_text(
                cx,
                &document_inner.text,
                &document_inner.token_cache,
                &self.lines_layout,
            );
            
            self.editor_impl.draw_current_line(cx, &self.lines_layout, *session.cursors.last_inserted());
            
            self.draw_widgets(cx);
            
            self.editor_impl.draw_linenums(cx, &self.lines_layout, *session.cursors.last_inserted());
            
            self.editor_impl.end(cx, &self.lines_layout);
        }
    }
    
    fn process_live_edit(cx: &mut Cx, state: &mut EditorState, session_id: SessionId) {
        let session = &state.sessions[session_id];
        let document = &state.documents[session.document_id];
        let document_inner = document.inner.as_ref().unwrap();
        
        let mut inline_cache = document_inner.inline_cache.borrow_mut();
        inline_cache.refresh_live_register_range(&document_inner.token_cache);
        
        let token_cache = &document_inner.token_cache;
        let lines = &document_inner.text.as_lines();
        let path = document.path.clone().into_os_string().into_string().unwrap();
        
        let live_registry_rc = cx.live_registry.clone();
        let mut live_registry = live_registry_rc.borrow_mut();
        
        // ok now what.
        match live_registry.live_edit_file(&path, inline_cache.live_register_range.unwrap(), | line | {
            (&lines[line], &token_cache[line].tokens())
        }) {
            Ok(result) => {
                cx.live_edit_result = result;
            }
            Err(_) => {}
        };
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        event: &mut Event,
        send_request: &mut dyn FnMut(Request),
        dispatch_action: &mut dyn FnMut(&mut Cx, CodeEditorAction),
    ) {
        let mut live_edit = false;
        let session_id = self.editor_impl.session_id.unwrap();
        for widget in self.widgets.values_mut() {
            match widget.inline_widget.handle_inline_event(cx, event, widget.bind) {
                InlineWidgetAction::ReplaceText {position, size, text} => {
                    
                    state.replace_text_direct(
                        session_id,
                        position,
                        size,
                        text,
                        send_request
                    );
                    live_edit = true;
                    self.editor_impl.redraw(cx);
                }
                _ => ()
            }
        }
        // what if the code editor changes something?
        self.editor_impl.handle_event(cx, state, event, &self.lines_layout, send_request, &mut |cx, action|{
            match action{
                CodeEditorAction::RedrawViewsForDocument(_)=>{
                    live_edit = true;
                }
            }
            dispatch_action(cx, action);
        });
        
        if live_edit{
            Self::process_live_edit(cx, state, session_id);
        }
    }
}
