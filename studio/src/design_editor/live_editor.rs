use {
    crate::{
        makepad_platform::*,
        makepad_component::{
            fold_button::{FoldButton, FoldButtonAction},
            ComponentMap,
        },
        makepad_live_compiler::{LiveTokenId, LiveEditEvent},
        makepad_live_tokenizer::{
            full_token::{FullToken, Delim},
            TokenWithLen,
            text::{Text},
        },
        editor_state::{
            EditorState,
            DocumentInner
        },
        code_editor::{
            token_cache::TokenCache,
            code_editor_impl::{CodeEditorImpl, CodeEditorAction, LinesLayout, LineLayoutOutput}
        },
        collab::{
            collab_protocol::CollabRequest,
        },
        design_editor::{
            inline_widget::*,
            inline_cache::InlineEditBind
        },
        editor_state::{
            SessionId
        },
    },
    
};

live_register!{
    use makepad_platform::shader::std::*;
    use makepad_component::fold_button::FoldButton;
    
    LiveEditor: {{LiveEditor}} {
        
        
        fold_button: FoldButton {
            bg_quad: {no_h_scroll: true}
        }
        
        widget_layout: {
            align: {fx: 0.2, fy: 0},
            padding: {left: 0, top: 0, right: 0, bottom: 0}
        }
        
        zoom_indent_depth: 8
        
        text_color_type_name: #56c9b1;
        text_color_comment: #638d54
        text_color_lifetime: #d4d4d4
        text_color_identifier: #d4d4d4
        text_color_function_identifier: #dcdcae
        text_color_macro_identifier: #dcdcae
        text_color_branch_keyword: #c485be
        text_color_loop_keyword: #ff8c00
        text_color_other_keyword: #5b9bd3
        text_color_bool: #5b9bd3
        text_color_number: #b6ceaa
        text_color_punctuator: #d4d4d4
        text_color_string: #cc917b
        text_color_whitespace: #6e6e6e
        text_color_unknown: #808080
        text_color_color: #cc917b
        
        editor_impl: {}
    }
}

#[derive(Copy, Debug, Clone, Hash, PartialEq, Eq)]
pub struct WidgetIdent(LiveTokenId, LiveType);

pub struct Widget {
    bind: InlineEditBind,
    inline_widget: Box<dyn InlineWidget>
}

#[derive(Live)]
pub struct LiveEditor {
    editor_impl: CodeEditorImpl,
    
    widget_layout: Layout,
    fold_button: Option<LivePtr>,
    
    zoom_indent_depth: usize,
    
    
    text_color_color: Vec4,
    text_color_type_name: Vec4,
    text_color_comment: Vec4,
    text_color_lifetime: Vec4,
    text_color_identifier: Vec4,
    text_color_macro_identifier: Vec4,
    text_color_function_identifier: Vec4,
    text_color_branch_keyword: Vec4,
    text_color_loop_keyword: Vec4,
    text_color_other_keyword: Vec4,
    text_color_bool: Vec4,
    text_color_number: Vec4,
    text_color_punctuator: Vec4,
    text_color_string: Vec4,
    text_color_whitespace: Vec4,
    text_color_unknown: Vec4,
    
    #[rust] delayed_reparse_document: Option<LiveEditEvent>,
    #[rust] lines_layout: LinesLayout,
    #[rust] widget_draw_order: Vec<(usize, WidgetIdent)>,
    #[rust] widgets: ComponentMap<WidgetIdent, Widget>,
    #[rust] fold_buttons: ComponentMap<u64, FoldButton>,
}

impl LiveHook for LiveEditor {
    fn after_apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        let registries = cx.registries.clone();
        for widget in self.widgets.values_mut() {
            registries.get::<CxInlineWidgetRegistry>().apply(cx, apply_from, index, nodes, widget.inline_widget.as_mut());
        }
        if let Some(index) = nodes.child_by_name(index, id!(fold_button)) {
            for fold_button in self.fold_buttons.values_mut() {
                fold_button.apply(cx, apply_from, index, nodes);
            }
        }
        self.editor_impl.redraw(cx);
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
    
    pub fn draw_widgets(&mut self, cx: &mut Cx2d) {
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
                let layout = &self.lines_layout.lines[*line];
                
                cx.begin_turtle(Layout {
                    abs_origin: Some(vec2(
                        origin.x,
                        origin.y + layout.start_y + layout.text_height
                    )),
                    abs_size: Some(vec2(
                        size.x,
                        layout.widget_height
                    )),
                    ..self.widget_layout
                });
            }
            let widget = self.widgets.get_mut(ident).unwrap();
            
            widget.inline_widget.draw_widget(cx, &live_registry, widget.bind);
            
            last_line = Some(line)
        }
        if last_line.is_some() {
            cx.end_turtle();
        }
    }
    
    pub fn draw_fold_buttons(&mut self, cx: &mut Cx2d, document_inner: &DocumentInner) {
        let mut last_line = None;
        let origin = cx.get_turtle_pos();
        
        let inline_cache = document_inner.inline_cache.borrow_mut();
        
        for (line, _) in &self.widget_draw_order {
            if Some(line) != last_line { // start a new draw segment with the turtle
                let layout = &self.lines_layout.lines[*line];
                let line_opened = inline_cache[*line].line_opened;
                let fold_button_id = inline_cache[*line].fold_button_id.unwrap();
                let fold_button = self.fold_button;
                let fb = self.fold_buttons.get_or_insert(cx, fold_button_id, | cx | {
                    let mut btn = FoldButton::new_from_option_ptr(cx, fold_button);
                    btn.set_is_open(cx, line_opened > 0.5, Animate::No);
                    btn
                });
                
                fb.draw_abs(cx, vec2(origin.x, origin.y + layout.start_y), 1.0 - layout.zoom_out);
            }
            last_line = Some(line)
        }
        self.fold_buttons.retain_visible();
    }
    
    pub fn calc_layout_with_widgets(&mut self, cx: &mut Cx2d, path: &str, document_inner: &DocumentInner) {
        
        let mut inline_cache = document_inner.inline_cache.borrow_mut();
        inline_cache.refresh(cx, path, &document_inner.token_cache);
        
        let token_cache = &document_inner.token_cache;
        
        // first we generate the layout structure
        let live_registry_rc = cx.live_registry.clone();
        let live_registry = live_registry_rc.borrow();
        
        let widgets = &mut self.widgets;
        
        let widget_draw_order = &mut self.widget_draw_order;
        
        let registries = cx.registries.clone();
        let widget_registry = registries.get::<CxInlineWidgetRegistry>();
        let zoom_indent_depth = self.zoom_indent_depth;
        
        self.editor_impl.calc_lines_layout(cx, document_inner, &mut self.lines_layout, | cx, input | {
            if input.clear {
                widget_draw_order.clear();
            }
            let edit_info = &inline_cache[input.line];
            let mut max_height = 0.0f32;
            
            let ws = document_inner.indent_cache[input.line].virtual_leading_whitespace();
            
            // ok so. we have to determine wether we are going to fold.
            // if a line starts with # or a comment: fold it
            let mut zoom_out = 0.0;
            let mut zoom_column = 0;
            
            match token_cache[input.line].tokens().first() {
                Some(TokenWithLen {token: FullToken::Comment, ..}) |
                Some(TokenWithLen {token: FullToken::Punct(id!(#)), ..}) |
                None => {
                    zoom_out = input.zoom_out
                }
                Some(TokenWithLen {token: FullToken::Whitespace, ..}) if token_cache[input.line].tokens().len() == 1 => {
                    zoom_out = input.zoom_out
                }
                _ => ()
            }
            
            if ws >= zoom_indent_depth {
                zoom_column = zoom_indent_depth;
                zoom_out = input.zoom_out;
            }
            
            for bind in &edit_info.items {
                
                if let Some(matched) = widget_registry.match_inline_widget(&live_registry, *bind) {
                    let cache_line = &inline_cache.lines[input.line];
                    
                    let widget_height = matched.height * cache_line.line_opened;
                    
                    max_height = max_height.max(widget_height);
                    
                    if input.start_y + matched.height > input.viewport_start && input.start_y < input.viewport_end {
                        // lets spawn it
                        let ident = WidgetIdent(bind.live_token_id, matched.live_type);
                        widgets.get_or_insert(cx, ident, | cx | {
                            Widget {
                                bind: *bind,
                                inline_widget: widget_registry.new(cx, matched.live_type).unwrap(),
                            }
                        });
                        
                        widget_draw_order.push((input.line, ident));
                    }
                }
            }
            return LineLayoutOutput {
                zoom_out,
                zoom_column,
                widget_height: max_height
            }
        });
        widgets.retain_visible();
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d, state: &EditorState) {
        if let Ok((document, document_inner, session)) = self.editor_impl.begin(cx, state) {
            let path = document.path.clone().into_os_string().into_string().unwrap();
            
            // if we are folding we need to store the last lead cursor y pos
            // then we calc layout and get a new one, then we scroll, and calc again
            self.calc_layout_with_widgets(
                cx,
                &path,
                document_inner,
            );
            
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
            
            self.draw_text(
                cx,
                &document_inner.text,
                &document_inner.token_cache,
            );
            
            self.editor_impl.draw_current_line(
                cx,
                &self.lines_layout,
                *session.cursors.last_inserted()
            );
            
            self.editor_impl.draw_message_lines(
                cx,
                &document_inner.msg_cache,
                state,
                &self.lines_layout,
            );
            
            self.draw_widgets(cx);
            
            self.editor_impl.draw_linenums(
                cx,
                &self.lines_layout,
                *session.cursors.last_inserted()
            );
            
            self.draw_fold_buttons(cx, document_inner);
            
            self.editor_impl.end(cx, &self.lines_layout);
        }
    }
    
    pub fn draw_text(
        &mut self,
        cx: &mut Cx2d,
        text: &Text,
        token_cache: &TokenCache,
    ) {
        let lines_layout = &self.lines_layout;
        let origin = cx.get_turtle_pos();
        //let mut start_y = visible_lines.start_y;
        for (line_index, (chars, token_info)) in text
            .as_lines()
            .iter()
            .zip(token_cache.iter())
            .skip(lines_layout.view_start)
            .take(lines_layout.view_end - lines_layout.view_start)
            .enumerate()
        {
            let line_index = line_index + lines_layout.view_start;
            let layout = &lines_layout.lines[line_index];
            
            let mut start_x = origin.x + self.editor_impl.line_num_width + layout.zoom_displace;
            let mut start = 0;
            
            let mut token_iter = token_info.tokens().iter().peekable();
            while let Some(token) = token_iter.next() {
                
                let next_token = token_iter.peek();
                let end_x = start_x + token.len as f32 * self.editor_impl.text_glyph_size.x * layout.font_scale;
                let end = start + token.len;
                
                // check if we are whitespace. ifso, just skip rendering
                if !token.token.is_whitespace() {
                    self.editor_impl.draw_code_chunk(
                        cx,
                        layout.font_scale,
                        self.text_color(&chars[start..end], token.token, next_token.map( | next_token | next_token.token)),
                        Vec2 {x: start_x, y: layout.start_y + origin.y},
                        &chars[start..end]
                    );
                }
                start = end;
                start_x = end_x;
            }
        }
    }
    
    fn process_live_edit(&mut self, cx: &mut Cx, state: &mut EditorState, session_id: SessionId) {
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
            Ok(event) => {
                match event {
                    Some(LiveEditEvent::ReparseDocument) => {
                        inline_cache.invalidate_all();
                        self.delayed_reparse_document = event;
                    }
                    Some(_) => {
                        self.delayed_reparse_document = None;
                        cx.live_edit_event = event;
                    }
                    _ => ()
                }
            }
            Err(e) => {
                let e = live_registry.live_error_to_live_file_error(e);
                eprintln!("PARSE ERROR {}", e);
            }
        };
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        event: &mut Event,
        send_request: &mut dyn FnMut(CollabRequest),
        dispatch_action: &mut dyn FnMut(&mut Cx, CodeEditorAction),
    ) {
        if self.editor_impl.scroll_view.handle_event(cx, event) {
            self.editor_impl.scroll_view.redraw(cx);
        }
        
        let mut live_edit = false;
        if self.editor_impl.session_id.is_none() {
            return
        }
        let session_id = self.editor_impl.session_id.unwrap();
        for widget in self.widgets.values_mut() {
            match widget.inline_widget.handle_widget_event(cx, event, widget.bind) {
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
        
        let mut fold_actions = Vec::new();
        for (fold_button_id, fold_button) in self.fold_buttons.iter_mut() {
            fold_button.handle_event_with_fn(cx, event, &mut | _, action | fold_actions.push((action, *fold_button_id)));
        }
        for (action, fold_button_id) in fold_actions {
            match action {
                FoldButtonAction::Animating(opened) => {
                    let session = &state.sessions[session_id];
                    let document = &state.documents[session.document_id];
                    let document_inner = document.inner.as_ref().unwrap();
                    let mut inline_cache = document_inner.inline_cache.borrow_mut();
                    if let Some(line) = inline_cache.iter_mut().find( | line | line.fold_button_id == Some(fold_button_id)) {
                        line.line_opened = opened;
                    }
                    self.editor_impl.redraw(cx);
                }
                _ => ()
            }
        }
        
        // what if the code editor changes something?
        let delayed_reparse_document = &mut self.delayed_reparse_document;
        self.editor_impl.handle_event_with_fn(
            cx,
            state,
            event,
            &self.lines_layout,
            send_request,
            &mut | cx,
            action | {
                match action {
                    CodeEditorAction::RedrawViewsForDocument(_) => {
                        live_edit = true;
                    }
                    CodeEditorAction::CursorBlink => {
                        if delayed_reparse_document.is_some() {
                            let live_registry_rc = cx.live_registry.clone();
                            let mut live_registry = live_registry_rc.borrow_mut();
                            let live_edit_event = delayed_reparse_document.take();
                            match live_registry.process_next_originals_and_expand() {
                                Err(errs) => {
                                    for e in errs {
                                        let e = live_registry.live_error_to_live_file_error(e);
                                        eprintln!("PARSE ERROR {}", e);
                                    }
                                }
                                Ok(()) => {
                                    cx.live_edit_event = live_edit_event
                                }
                            }
                        }
                    }
                }
                dispatch_action(cx, action);
            }
        );
        
        if live_edit {
            self.process_live_edit(cx, state, session_id);
        }
    }
    
    fn text_color(&self, text: &[char], token: FullToken, next_token: Option<FullToken>) -> Vec4 {
        match (token, next_token) {
            (FullToken::Comment, _) => self.text_color_comment,
            (FullToken::Ident(id), _) if id.is_capitalised() => {
                if text.len() > 1 && text[1].is_uppercase() {
                    self.text_color_string
                }
                else {
                    self.text_color_type_name
                }
            },
            (FullToken::Ident(_), Some(FullToken::Open(Delim::Paren))) => self.text_color_function_identifier,
            (FullToken::Ident(_), Some(FullToken::Punct(id!(!)))) => self.text_color_macro_identifier,
            
            (FullToken::Lifetime, _) => self.text_color_lifetime,
            
            (FullToken::Ident(id!(if)), _) |
            (FullToken::Ident(id!(else)), _) |
            (FullToken::Ident(id!(match)), _) => self.text_color_branch_keyword,
            
            (FullToken::Ident(id!(for)), _) |
            (FullToken::Ident(id!(while)), _) |
            (FullToken::Ident(id!(break)), _) |
            (FullToken::Ident(id!(continue)), _) |
            (FullToken::Ident(id!(loop)), _) => self.text_color_loop_keyword,
            
            (FullToken::Ident(id!(abstract)), _) |
            (FullToken::Ident(id!(async)), _) |
            (FullToken::Ident(id!(as)), _) |
            (FullToken::Ident(id!(await)), _) |
            (FullToken::Ident(id!(become)), _) |
            (FullToken::Ident(id!(box)), _) |
            (FullToken::Ident(id!(const)), _) |
            (FullToken::Ident(id!(crate)), _) |
            (FullToken::Ident(id!(do)), _) |
            (FullToken::Ident(id!(dyn)), _) |
            (FullToken::Ident(id!(enum)), _) |
            (FullToken::Ident(id!(extern)), _) |
            (FullToken::Ident(id!(false)), _) |
            (FullToken::Ident(id!(final)), _) |
            (FullToken::Ident(id!(fn)), _) |
            (FullToken::Ident(id!(impl)), _) |
            (FullToken::Ident(id!(in)), _) |
            (FullToken::Ident(id!(let)), _) |
            (FullToken::Ident(id!(macro)), _) |
            (FullToken::Ident(id!(mod)), _) |
            (FullToken::Ident(id!(move)), _) |
            (FullToken::Ident(id!(mut)), _) |
            (FullToken::Ident(id!(override)), _) |
            (FullToken::Ident(id!(priv)), _) |
            (FullToken::Ident(id!(pub)), _) |
            (FullToken::Ident(id!(ref)), _) |
            (FullToken::Ident(id!(self)), _) |
            (FullToken::Ident(id!(static)), _) |
            (FullToken::Ident(id!(struct)), _) |
            (FullToken::Ident(id!(super)), _) |
            (FullToken::Ident(id!(trait)), _) |
            (FullToken::Ident(id!(true)), _) |
            (FullToken::Ident(id!(typeof)), _) |
            (FullToken::Ident(id!(unsafe)), _) |
            (FullToken::Ident(id!(use)), _) |
            (FullToken::Ident(id!(unsized)), _) |
            (FullToken::Ident(id!(virtual)), _) |
            (FullToken::Ident(id!(yield)), _) |
            (FullToken::Ident(id!(where)), _) |
            
            (FullToken::Ident(id!(u8)), _) |
            (FullToken::Ident(id!(i8)), _) |
            (FullToken::Ident(id!(u16)), _) |
            (FullToken::Ident(id!(i16)), _) |
            (FullToken::Ident(id!(u32)), _) |
            (FullToken::Ident(id!(i32)), _) |
            (FullToken::Ident(id!(f32)), _) |
            (FullToken::Ident(id!(u64)), _) |
            (FullToken::Ident(id!(i64)), _) |
            (FullToken::Ident(id!(f64)), _) |
            (FullToken::Ident(id!(usize)), _) |
            (FullToken::Ident(id!(isize)), _) |
            (FullToken::Ident(id!(bool)), _) |
            
            (FullToken::Ident(id!(instance)), _) |
            (FullToken::Ident(id!(uniform)), _) |
            (FullToken::Ident(id!(texture)), _) |
            (FullToken::Ident(id!(float)), _) |
            (FullToken::Ident(id!(vec2)), _) |
            (FullToken::Ident(id!(vec3)), _) |
            (FullToken::Ident(id!(vec4)), _) |
            (FullToken::Ident(id!(mat2)), _) |
            (FullToken::Ident(id!(mat3)), _) |
            (FullToken::Ident(id!(mat4)), _) |
            (FullToken::Ident(id!(ivec2)), _) |
            (FullToken::Ident(id!(ivec3)), _) |
            (FullToken::Ident(id!(ivec4)), _) |
            (FullToken::Ident(id!(bvec2)), _) |
            (FullToken::Ident(id!(bvec3)), _) |
            (FullToken::Ident(id!(bvec4)), _) => self.text_color_other_keyword,
            (FullToken::Ident(_), _) => self.text_color_identifier,
            (FullToken::Bool(_), _) => self.text_color_bool,
            
            (FullToken::Float(_), _) |
            (FullToken::Int(_), _) |
            (FullToken::OtherNumber, _) => self.text_color_number,
            
            (FullToken::Punct(_), _) => self.text_color_punctuator,
            (FullToken::String, _) => self.text_color_string,
            (FullToken::Whitespace, _) => self.text_color_whitespace,
            (FullToken::Color(_), _) => self.text_color_color,
            (FullToken::Unknown, _) => self.text_color_unknown,
            (FullToken::Open(_), _) |
            (FullToken::Close(_), _) => self.text_color_punctuator,
        }
    }
    
}
