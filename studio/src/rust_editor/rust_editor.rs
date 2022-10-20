use {
    crate::{
        makepad_draw_2d::*,
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
        makepad_collab_protocol::{
            CollabRequest,
            unix_path::UnixPath,
        },
        editor_state::{
            SessionId
        },
    },
    
};

live_design!{
    import makepad_draw_2d::shader::std::*;
    import makepad_widgets::fold_button::FoldButton;
    
    RustEditor= {{RustEditor}} {
        
        
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
    
    
    pub fn calc_layout_with_widgets(&mut self, cx: &mut Cx2d, _path: &UnixPath, document_inner: &DocumentInner) {
        
        let token_cache = &document_inner.token_cache;
        
        // first we generate the layout structure
        //let live_registry_rc = cx.live_registry.clone();
        //let live_registry = live_registry_rc.borrow();
        
        let zoom_indent_depth = self.zoom_indent_depth;
        
        self.editor_impl.calc_lines_layout(cx, document_inner, &mut self.lines_layout, | _cx, input | {

            let max_height = 0.0;
            
            let ws = document_inner.indent_cache[input.line].virtual_leading_whitespace();
            
            // ok so. we have to determine wether we are going to fold.
            // if a line starts with # or a comment: fold it
            let mut zoom_out = 0.0;
            let mut zoom_column = 0;
            
            match token_cache[input.line].tokens().first() {
                Some(TokenWithLen {token: FullToken::Comment, ..}) |
                Some(TokenWithLen {token: FullToken::Punct(live_id!(#)), ..}) |
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
        if !self.editor_impl.state_has_document_inner(state){
            return
        }
        
        let (document, document_inner, session) = self.editor_impl.get_state(cx, state);
        
        let path = document.path.clone();
        // if we are folding we need to store the last lead cursor y pos
        // then we calc layout and get a new one, then we scroll, and calc again
        self.calc_layout_with_widgets(
            cx,
            &path,
            document_inner,
        );
        
        self.editor_impl.begin(cx);
        
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
                let end_x = start_x + token.len as f64 * self.editor_impl.text_glyph_size.x * layout.font_scale;
                let end = start + token.len;
                
                // check if we are whitespace. ifso, just skip rendering
                if !token.token.is_whitespace() {
                    self.editor_impl.draw_code_chunk(
                        cx,
                        layout.font_scale,
                        self.text_color(&chars[start..end], token.token, next_token.map( | next_token | next_token.token)),
                        DVec2 {x: start_x, y: layout.start_y + origin.y},
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
        event: &Event,
        send_request: &mut dyn FnMut(CollabRequest),
        dispatch_action: &mut dyn FnMut(&mut Cx, CodeEditorAction),
    ) {
        self.editor_impl.scroll_bars.handle_event_fn(cx, event, &mut |_,_|{});
        
        if self.editor_impl.session_id.is_none() {
            return
        }
        //let session_id = self.editor_impl.session_id.unwrap();
        
        // what if the code editor changes something?
        self.editor_impl.handle_event(
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
            (FullToken::Ident(_), Some(FullToken::Open(Delim::Paren))) => self.text_color_function_identifier,
            (FullToken::Ident(_), Some(FullToken::Punct(live_id!(!)))) => self.text_color_macro_identifier,
            
            (FullToken::Lifetime, _) => self.text_color_lifetime,
            
            (FullToken::Ident(live_id!(if)), _) |
            (FullToken::Ident(live_id!(else)), _) |
            (FullToken::Ident(live_id!(return)), _) |
            (FullToken::Ident(live_id!(match)), _) => self.text_color_branch_keyword,
            
            (FullToken::Ident(live_id!(for)), _) |
            (FullToken::Ident(live_id!(while)), _) |
            (FullToken::Ident(live_id!(break)), _) |
            (FullToken::Ident(live_id!(continue)), _) |
            (FullToken::Ident(live_id!(loop)), _) => self.text_color_loop_keyword,
            
            (FullToken::Ident(live_id!(abstract)), _) |
            (FullToken::Ident(live_id!(async)), _) |
            (FullToken::Ident(live_id!(as)), _) |
            (FullToken::Ident(live_id!(await)), _) |
            (FullToken::Ident(live_id!(become)), _) |
            (FullToken::Ident(live_id!(box)), _) |
            (FullToken::Ident(live_id!(const)), _) |
            (FullToken::Ident(live_id!(crate)), _) |
            (FullToken::Ident(live_id!(do)), _) |
            (FullToken::Ident(live_id!(dyn)), _) |
            (FullToken::Ident(live_id!(enum)), _) |
            (FullToken::Ident(live_id!(extern)), _) |
            (FullToken::Ident(live_id!(false)), _) |
            (FullToken::Ident(live_id!(final)), _) |
            (FullToken::Ident(live_id!(fn)), _) |
            (FullToken::Ident(live_id!(impl)), _) |
            (FullToken::Ident(live_id!(in)), _) |
            (FullToken::Ident(live_id!(let)), _) |
            (FullToken::Ident(live_id!(macro)), _) |
            (FullToken::Ident(live_id!(mod)), _) |
            (FullToken::Ident(live_id!(move)), _) |
            (FullToken::Ident(live_id!(mut)), _) |
            (FullToken::Ident(live_id!(override)), _) |
            (FullToken::Ident(live_id!(priv)), _) |
            (FullToken::Ident(live_id!(pub)), _) |
            (FullToken::Ident(live_id!(ref)), _) |
            (FullToken::Ident(live_id!(self)), _) |
            (FullToken::Ident(live_id!(static)), _) |
            (FullToken::Ident(live_id!(struct)), _) |
            (FullToken::Ident(live_id!(super)), _) |
            (FullToken::Ident(live_id!(trait)), _) |
            (FullToken::Ident(live_id!(true)), _) |
            (FullToken::Ident(live_id!(typeof)), _) |
            (FullToken::Ident(live_id!(unsafe)), _) |
            (FullToken::Ident(live_id!(use)), _) |
            (FullToken::Ident(live_id!(unsized)), _) |
            (FullToken::Ident(live_id!(virtual)), _) |
            (FullToken::Ident(live_id!(yield)), _) |
            (FullToken::Ident(live_id!(where)), _) |
            
            (FullToken::Ident(live_id!(u8)), _) |
            (FullToken::Ident(live_id!(i8)), _) |
            (FullToken::Ident(live_id!(u16)), _) |
            (FullToken::Ident(live_id!(i16)), _) |
            (FullToken::Ident(live_id!(u32)), _) |
            (FullToken::Ident(live_id!(i32)), _) |
            (FullToken::Ident(live_id!(f32)), _) |
            (FullToken::Ident(live_id!(u64)), _) |
            (FullToken::Ident(live_id!(i64)), _) |
            (FullToken::Ident(live_id!(f64)), _) |
            (FullToken::Ident(live_id!(usize)), _) |
            (FullToken::Ident(live_id!(isize)), _) |
            (FullToken::Ident(live_id!(bool)), _) |
            
            (FullToken::Ident(live_id!(instance)), _) |
            (FullToken::Ident(live_id!(uniform)), _) |
            (FullToken::Ident(live_id!(texture)), _) |
            (FullToken::Ident(live_id!(float)), _) |
            (FullToken::Ident(live_id!(vec2)), _) |
            (FullToken::Ident(live_id!(vec3)), _) |
            (FullToken::Ident(live_id!(vec4)), _) |
            (FullToken::Ident(live_id!(mat2)), _) |
            (FullToken::Ident(live_id!(mat3)), _) |
            (FullToken::Ident(live_id!(mat4)), _) |
            (FullToken::Ident(live_id!(ivec2)), _) |
            (FullToken::Ident(live_id!(ivec3)), _) |
            (FullToken::Ident(live_id!(ivec4)), _) |
            (FullToken::Ident(live_id!(bvec2)), _) |
            (FullToken::Ident(live_id!(bvec3)), _) |
            (FullToken::Ident(live_id!(bvec4)), _) => self.text_color_other_keyword,
            (FullToken::Ident(_), _) => {
                if text[0].is_uppercase(){
                    if text.len() > 1 && text[1].is_uppercase() {
                        self.text_color_string
                    }
                    else {
                        self.text_color_type_name
                    }
                }
                else{
                    self.text_color_identifier
                }
            },
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
