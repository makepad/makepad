use {
    crate::{
        makepad_platform::*,
        makepad_editor_core::{
            text::{Text},
        },
        rust_editor::rust_tokenizer::{
            full_token::{FullToken, Delim},
            TokenWithLen,
        },
        editor_state::{
            EditorState,
            DocumentInner
        },
        code_editor::{
            code_editor_impl::{CodeEditorImpl, CodeEditorAction, LinesLayout, LineLayoutOutput}
        },
        rust_editor::rust_tokenizer::token_cache::TokenCache,
        makepad_collab_protocol::CollabRequest,
        editor_state::{
            SessionId
        },
    },
    
};

live_register!{
    use makepad_platform::shader::std::*;
    use makepad_component::fold_button::FoldButton;
    
    RustEditor: {{RustEditor}} {
        
        
        fold_button: FoldButton {
            bg_quad: {no_h_scroll: true}
        }
        
        widget_layout: {
            align: {x: 0.2, y: 0},
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

#[derive(Live)]
pub struct RustEditor {
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
    
    #[rust] lines_layout: LinesLayout,
}

impl LiveHook for RustEditor {
    fn after_apply(&mut self, cx: &mut Cx, _from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        self.editor_impl.redraw(cx);
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
    
    
    pub fn calc_layout_with_widgets(&mut self, cx: &mut Cx2d, _path: &str, document_inner: &DocumentInner) {
        
        let token_cache = &document_inner.token_cache;
        
        // first we generate the layout structure
        //let live_registry_rc = cx.live_registry.clone();
        //let live_registry = live_registry_rc.borrow();
        
        let zoom_indent_depth = self.zoom_indent_depth;
        
        self.editor_impl.calc_lines_layout(cx, document_inner, &mut self.lines_layout, | _cx, input | {

            let max_height = 0.0f32;
            
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
            
            return LineLayoutOutput {
                zoom_out,
                zoom_column,
                widget_height: max_height
            }
        });
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
            
            self.editor_impl.draw_linenums(
                cx,
                &self.lines_layout,
                *session.cursors.last_inserted()
            );
            
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
        let origin = cx.turtle().pos();
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
        
        if self.editor_impl.session_id.is_none() {
            return
        }
        //let session_id = self.editor_impl.session_id.unwrap();
        
        // what if the code editor changes something?
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
                    }
                    CodeEditorAction::CursorBlink => {
                    }
                }
                dispatch_action(cx, action);
            }
        );
        
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
            (FullToken::Ident(id!(return)), _) |
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
