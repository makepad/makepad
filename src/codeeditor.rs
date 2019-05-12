use widgets::*;
use crate::textbuffer::*;

#[derive(Clone)]
pub struct CodeEditor{
    pub view:View<ScrollBar>,
    pub bg_layout:Layout,
    pub bg: Quad,
    pub cursor: Quad,
    pub selection: Quad,
    pub highlight: Quad,
    pub cursor_row: Quad,
    pub paren_pair: Quad,
    pub tab:Quad,
    pub text: Text,
    pub cursors:CursorSet,
    
    pub open_font_size:f32,
    pub folded_font_size:f32,
    
    pub colors:SyntaxColors,

    pub _hit_state:HitState,
    pub _bg_area:Area,
    pub _text_inst:Option<AlignedInstance>,
    pub _text_area:Area,
    pub _scroll_pos:Vec2,
    pub _last_finger_move:Option<Vec2>,
    pub _paren_stack:Vec<ParenItem>,
    //pub _paren_list:Vec<ParenItem>,
    pub _line_geometry:Vec<LineGeom>,
    pub _anim_select:Vec<AnimSelect>,
    pub _token_chunks:Vec<TokenChunk>,
    pub _visible_lines:usize,

    pub _select_scroll:Option<SelectScroll>,
    pub _grid_select_corner:Option<TextPos>,
    pub _line_chunk:Vec<(f32,char)>,

    pub _highlight_selection:Vec<char>,
    pub _highlight_token:Vec<char>,

    pub _anim_font_size:f32,
    pub _line_largest_font:f32,
    pub _anim_folding:AnimFolding,

    pub _monospace_size:Vec2,
    pub _instance_count:usize,
    pub _first_on_line:bool,
    pub _draw_cursors:DrawCursors,
    pub _draw_search:DrawCursors,

    pub _last_tabs:usize,
    pub _newline_tabs:usize,
}

#[derive(Clone)]
pub enum AnimFoldingState{
    Open,
    Opening(f32,f32,f32),
    Folded,
    Folding(f32,f32,f32)
}

impl AnimFoldingState{
    fn is_animating(&self)->bool{
        match self{
            AnimFoldingState::Open=>false,
            AnimFoldingState::Folded=>false,
            _=>true
        }
    }

    fn is_folded(&self)->bool{
        match self{
            AnimFoldingState::Open=>false,
            _=>true
        }
    }

    fn get_font_size(&self, open_size:f32, folded_size:f32)->f32{
        match self{
            AnimFoldingState::Open=>open_size,
            AnimFoldingState::Folded=>folded_size,
            AnimFoldingState::Opening(f, _, _)=>f*folded_size + (1.-f)*open_size,
            AnimFoldingState::Folding(f, _, _)=>f*open_size + (1.-f)*folded_size,
        }
    }

    fn do_folding(&mut self, speed:f32, speed2:f32){
        *self = match self{
            AnimFoldingState::Open=>AnimFoldingState::Folding(1.0, speed,speed2),
            AnimFoldingState::Folded=>AnimFoldingState::Folded,
            AnimFoldingState::Opening(f, _, _)=>AnimFoldingState::Folding(1.0 - *f, speed,speed2),
            AnimFoldingState::Folding(f, _, _)=>AnimFoldingState::Folding(*f, speed,speed2),
        }
    }

    fn do_opening(&mut self, speed:f32, speed2:f32){
        *self = match self{
            AnimFoldingState::Open=>AnimFoldingState::Open,
            AnimFoldingState::Folded=>AnimFoldingState::Opening(1.0, speed, speed2),
            AnimFoldingState::Opening(f,_,_)=>AnimFoldingState::Opening(*f, speed, speed2),
            AnimFoldingState::Folding(f,_,_)=>AnimFoldingState::Opening(1.0 - *f, speed, speed2),
        }
    }

    fn next_anim_step(&mut self){
        *self = match self{
            AnimFoldingState::Open=>AnimFoldingState::Open,
            AnimFoldingState::Folded=>AnimFoldingState::Folded,
            AnimFoldingState::Opening(f,speed, speed2)=>{
                let new_f = *f * *speed;
                if new_f < 0.001{
                    AnimFoldingState::Open
                }
                else{
                    AnimFoldingState::Opening(new_f,*speed * *speed2, *speed2)
                }
            },
            AnimFoldingState::Folding(f, speed, speed2)=>{
                let new_f = *f * *speed;
                if new_f < 0.001{
                    AnimFoldingState::Folded
                }
                else{
                    AnimFoldingState::Folding(new_f,*speed * *speed2, *speed2)
                }
            },
        }
    }
}

#[derive(Clone)]
pub struct AnimFolding{
    pub state:AnimFoldingState,
    pub first_visible:(usize,f32),
    pub did_animate:bool
}

#[derive(Clone)]
pub struct AnimSelect{
    pub ypos:f32,
    pub invert:bool,
    pub time:f64
}

#[derive(Clone, Default)]
pub struct LineGeom{
    walk:Vec2,
    font_size:f32
}

#[derive(Clone, Default)]
pub struct SelectScroll{
//    pub margin:Margin,
    pub delta:Vec2,
    pub abs:Vec2,
    pub at_end:bool
}

#[derive(Clone)]
pub struct ParenItem{
    pair_start:usize,
    geom_open:Option<Rect>,
    geom_close:Option<Rect>,
    marked:bool,
    exp_paren:char
}

#[derive(Clone)]
pub struct SyntaxColors{
    whitespace:Color,
    keyword:Color,
    flow:Color,
    identifier:Color,
    call:Color,
    type_name:Color,
    string:Color,
    number:Color,
    comment:Color,
    paren:Color,
    operator:Color,
    delimiter:Color,
}

impl ElementLife for CodeEditor{
    fn construct(&mut self, _cx:&mut Cx){}
    fn destruct(&mut self, _cx:&mut Cx){}
}

impl Style for CodeEditor{
    fn style(cx:&mut Cx)->Self{
        let tab_sh = Self::def_tab_shader(cx);
        let selection_sh = Self::def_selection_shader(cx);
        let highlight_sh = Self::def_highlight_shader(cx);
        let cursor_sh = Self::def_cursor_shader(cx);
        let cursor_row_sh = Self::def_cursor_row_shader(cx);
        let paren_pair_sh = Self::def_paren_pair_shader(cx);
        let code_editor = Self{
            cursors:CursorSet::new(),
            colors:SyntaxColors{
                whitespace:color256(110,110,110),

                keyword:color256(91,155,211),
                flow:color256(196,133,190),
                identifier:color256(212,212,212),
                call:color256(220,220,174),
                type_name:color256(86,201,177),

                string:color256(204,145,123),
                number:color256(182,206,170),

                comment:color256(99,141,84),

                paren:color256(212,212,212),
                operator:color256(212,212,212),
                delimiter:color256(212,212,212)
            },
            tab:Quad{
                color:color("#5"),
                shader_id:cx.add_shader(tab_sh, "Editor.tab"),
                ..Style::style(cx)
            },
            view:View{
                scroll_h:Some(ScrollBar{
                    ..Style::style(cx)
                }),
                scroll_v:Some(ScrollBar{
                    smoothing:Some(0.15),
                    ..Style::style(cx)
                }),
                ..Style::style(cx)
            },
            bg:Quad{
                color:color256(30,30,30),
                do_scroll:false,
                ..Style::style(cx)
            },
            selection:Quad{
                color:color256(42,78,117),
                shader_id:cx.add_shader(selection_sh, "Editor.selection"),
                ..Style::style(cx)
            }, 
            highlight:Quad{
                color:color256a(75,75,95,128),
                shader_id:cx.add_shader(highlight_sh, "Editor.highlight"),
                ..Style::style(cx)
            }, 
            cursor:Quad{
                color:color256(136,136,136),
                shader_id:cx.add_shader(cursor_sh, "Editor.cursor"),
                ..Style::style(cx)
            },
            cursor_row:Quad{
                color:color256(136,136,136),
                shader_id:cx.add_shader(cursor_row_sh, "Editor.cursor_row"),
                ..Style::style(cx)
            },
            paren_pair:Quad{
                color:color256(136,136,136),
                shader_id:cx.add_shader(paren_pair_sh, "Editor.paren_pair"),
                ..Style::style(cx)
            },
            bg_layout:Layout{
                width:Bounds::Fill,
                height:Bounds::Fill,
                margin:Margin::all(0.),
                padding:Padding{l:4.0,t:4.0,r:4.0,b:4.0},
                ..Default::default()
            },
            text:Text{
                font_id:cx.load_font(&cx.font("mono_font")),
                font_size:11.0,
                brightness:1.05,
                line_spacing:1.4,
                wrapping:Wrapping::Line,
                ..Style::style(cx)
            },
            open_font_size:11.0,
            folded_font_size:0.5,

            _hit_state:HitState{no_scrolling:true, ..Default::default()},
            _monospace_size:Vec2::zero(),
            _last_finger_move:None,
            _first_on_line:true,
            _scroll_pos:Vec2::zero(),
            _visible_lines:0, 
            _line_geometry:Vec::new(),
            _token_chunks:Vec::new(),
            _anim_select:Vec::new(),
            _grid_select_corner:None,
            _bg_area:Area::Empty,
            _text_inst:None,
            _text_area:Area::Empty,
            _instance_count:0,
            _anim_font_size:11.0,
            _line_largest_font:0.,
            _anim_folding:AnimFolding{
                state:AnimFoldingState::Open,
                first_visible:(0,0.0),
                did_animate:false,
            },
            _select_scroll:None,
            _draw_cursors:DrawCursors::new(),
            _draw_search:DrawCursors::new(),
            _paren_stack:Vec::new(),
            _line_chunk:Vec::new(),
            _highlight_selection:Vec::new(),
            _highlight_token:Vec::new(),
            //_paren_list:Vec::new(),
            _last_tabs:0,
            _newline_tabs:0
        };
        //tab.animator.default = tab.anim_default(cx);
        code_editor
    }
}

#[derive(Clone, PartialEq)]
pub enum CodeEditorEvent{
    None,
    Change
}

impl CodeEditor{

    pub fn def_tab_shader(cx:&mut Cx)->Shader{
        let mut sh = Quad::def_quad_shader(cx);
        sh.add_ast(shader_ast!({
            fn pixel()->vec4{
                df_viewport(pos * vec2(w, h));
                df_move_to(1.,-1.);
                df_line_to(1.,h+1.);
                return df_stroke(color, 0.8);
            }
        }));
        sh
    }

    pub fn def_cursor_shader(cx:&mut Cx)->Shader{
        let mut sh = Quad::def_quad_shader(cx);
        sh.add_ast(shader_ast!({
            fn pixel()->vec4{
                return vec4(color.rgb*color.a,color.a)
            }
        }));
        sh
    }

    pub fn def_selection_shader(cx:&mut Cx)->Shader{
        let mut sh = Quad::def_quad_shader(cx);
        sh.add_ast(shader_ast!({
            let prev_x:float<Instance>;
            let prev_w:float<Instance>;
            let next_x:float<Instance>;
            let next_w:float<Instance>;
            const gloopiness:float = 8.;
            const border_radius:float = 2.;

            fn vertex()->vec4{ // custom vertex shader because we widen the draweable area a bit for the gloopiness
                let shift:vec2 = -draw_list_scroll * draw_list_do_scroll;
                let clipped:vec2 = clamp(
                    geom*vec2(w+16., h) + vec2(x, y) + shift - vec2(8.,0.),
                    draw_list_clip.xy,
                    draw_list_clip.zw
                );
                pos = (clipped - shift - vec2(x,y)) / vec2(w, h);
                return vec4(clipped,0.,1.) * camera_projection;
            }

            fn pixel()->vec4{
                df_viewport(pos * vec2(w, h));
                df_box(0., 0., w, h, border_radius);
                if prev_w > 0.{
                    df_box(prev_x, -h, prev_w, h, border_radius);
                    df_gloop(gloopiness);
                }
                if next_w > 0.{
                    df_box(next_x, h, next_w, h, border_radius);
                    df_gloop(gloopiness);
                }
                return df_fill(color);
            }
        }));
        sh
    }

   pub fn def_paren_pair_shader(cx:&mut Cx)->Shader{
        let mut sh = Quad::def_quad_shader(cx);
        sh.add_ast(shader_ast!({
            fn pixel()->vec4{
                df_viewport(pos * vec2(w, h));
                df_box(0.,0.,w,h,1.);
                return df_stroke(color, 1.);
            }
        }));
        sh
    }

   pub fn def_cursor_row_shader(cx:&mut Cx)->Shader{
        let mut sh = Quad::def_quad_shader(cx);
        sh.add_ast(shader_ast!({
            fn pixel()->vec4{
                df_viewport(pos * vec2(w, h));
                df_move_to(0.,1.);
                df_line_to(w,1.);
                df_move_to(0.,h-1.);
                df_line_to(w,h-1.);
                return df_stroke(color, 0.8);
            }
        }));
        sh
    }

   pub fn def_highlight_shader(cx:&mut Cx)->Shader{
        let mut sh = Quad::def_quad_shader(cx);
        sh.add_ast(shader_ast!({
            fn pixel()->vec4{
                df_viewport(pos * vec2(w, h));
                df_box(0.,0.,w,h,2.);
                return df_fill(color);
            }
        }));
        sh
    }

    pub fn handle_code_editor(&mut self, cx:&mut Cx, event:&mut Event, text_buffer:&mut TextBuffer)->CodeEditorEvent{
        match self.view.handle_scroll_bars(cx, event){
            (_,ScrollBarEvent::Scroll{..}) | (ScrollBarEvent::Scroll{..},_)=>{
                if let Some(last_finger_move) = self._last_finger_move{
                    if let Some(grid_select_corner) = self._grid_select_corner{
                        let pos = self.compute_grid_text_pos_from_abs(cx, last_finger_move);
                        self.cursors.grid_select(grid_select_corner, pos, text_buffer);
                    }
                    else{
                        let offset = self.text.find_closest_offset(cx, &self._text_area, last_finger_move);
                        self.cursors.set_last_cursor_head(offset, text_buffer);
                    }
                }
                // the editor actually redraws on scroll, its because we don't actually
                // generate the entire file as GPU text-buffer just the visible area
                // in JS this wasn't possible performantly but in Rust its a breeze.
                self.view.redraw_view_area(cx);
            },
            _=>()
        }
        match event.hits(cx, self._bg_area, &mut self._hit_state){
            Event::Animate(_ae)=>{
            },
            Event::FingerDown(fe)=>{
                cx.set_down_mouse_cursor(MouseCursor::Text);
                // give us the focus
                cx.set_key_focus(self._bg_area);
                let offset = self.text.find_closest_offset(cx, &self._text_area, fe.abs);
                match fe.tap_count{
                    1=>{
                    },
                    2=>{
                        if let Some(chunk) = CursorSet::get_nearest_token_chunk(offset, &self._token_chunks){
                            self.cursors.set_last_clamp_range((chunk.offset, chunk.len));
                        }
                    },
                    3=>{
                        let range = text_buffer.get_nearest_line_range(offset);
                        self.cursors.set_last_clamp_range(range);
                    },
                    _=>{
                        let range = (0, text_buffer.calc_char_count());
                        self.cursors.set_last_clamp_range(range);
                    }
                }
                // ok so we should scan a range 

                if fe.modifiers.shift{
                    if fe.modifiers.logo || fe.modifiers.control{ // grid select
                        let pos = self.compute_grid_text_pos_from_abs(cx, fe.abs);
                        self._grid_select_corner = Some(self.cursors.grid_select_corner(pos, text_buffer));
                        self.cursors.grid_select(self._grid_select_corner.unwrap(), pos, text_buffer);
                    }
                    else{ // simply place selection
                        self.cursors.clear_and_set_last_cursor_head(offset, text_buffer);
                    }
                }
                else{ // cursor drag with possible add
                    if fe.modifiers.logo || fe.modifiers.control{
                        self.cursors.add_last_cursor_head_and_tail(offset, text_buffer);
                    }
                    else{
                        self.cursors.clear_and_set_last_cursor_head_and_tail(offset, text_buffer);
                    }
                }
                self.view.redraw_view_area(cx);
                self._last_finger_move = Some(fe.abs);
                self.update_highlight(text_buffer);

            },
            Event::FingerHover(_fe)=>{
                cx.set_hover_mouse_cursor(MouseCursor::Text);
            },
            Event::FingerUp(fe)=>{

                // do the folded scrollnav
                if self._anim_folding.state.is_folded() && !fe.modifiers.shift{
                    let (start,end) = self.cursors.get_last_cursor_order();
                    if start == end{
                        self.scroll_last_cursor_to_top(cx, text_buffer);
                    }
                }

                self.cursors.clear_last_clamp_range();
                //self.cursors.end_cursor_drag(text_buffer);
                self._select_scroll = None;
                self._last_finger_move = None;
                self._grid_select_corner = None;
                self.update_highlight(text_buffer);
            },
            Event::FingerMove(fe)=>{
                if let Some(grid_select_corner) = self._grid_select_corner{
                    let pos = self.compute_grid_text_pos_from_abs(cx, fe.abs);
                    self.cursors.grid_select(grid_select_corner, pos, text_buffer);
                }
                else{
                    let offset = self.text.find_closest_offset(cx, &self._text_area, fe.abs);
                    self.cursors.set_last_cursor_head(offset, text_buffer);
                }

                self._last_finger_move = Some(fe.abs);
                // determine selection drag scroll dynamics
                let pow_scale = 0.1;
                let pow_fac = 3.;
                let max_speed = 40.;
                let pad_scroll = 20.;
                let rect = Rect{
                    x:fe.rect.x+pad_scroll,
                    y:fe.rect.y+pad_scroll,
                    w:fe.rect.w-2.*pad_scroll,
                    h:fe.rect.h-2.*pad_scroll,
                };
                let delta = Vec2{
                    x:if fe.abs.x < rect.x{
                        -((rect.x - fe.abs.x) * pow_scale).powf(pow_fac).min(max_speed)
                    }
                    else if fe.abs.x > rect.x + rect.w{
                        ((fe.abs.x - (rect.x + rect.w)) * pow_scale).powf(pow_fac).min(max_speed)
                    }
                    else{
                        0.
                    },
                    y:if fe.abs.y < rect.y{
                        -((rect.y - fe.abs.y) * pow_scale).powf(pow_fac).min(max_speed)
                    }
                    else if fe.abs.y > rect.y + rect.h{
                        ((fe.abs.y - (rect.y + rect.h)) * pow_scale).powf(pow_fac).min(max_speed)
                    }
                    else{
                        0.
                    }
                };
                let last_scroll_none = self._select_scroll.is_none();
                if delta.x !=0. || delta.y != 0.{
                   self._select_scroll = Some(SelectScroll{
                       abs:fe.abs,
                       delta:delta,
                       at_end:false
                   })
                }
                else{
                    self._select_scroll = None;
                }
                if last_scroll_none{
                    self.view.redraw_view_area(cx);
                }
                
            },
            Event::KeyDown(ke)=>{
                let cursor_moved = match ke.key_code{
                    KeyCode::ArrowUp=>{
                        self.cursors.move_up(1, ke.modifiers.shift, text_buffer);
                        true
                    },
                    KeyCode::ArrowDown=>{
                        self.cursors.move_down(1, ke.modifiers.shift, text_buffer);
                        true
                    },
                    KeyCode::ArrowLeft=>{
                        if ke.modifiers.logo || ke.modifiers.control{ // token skipping
                            self.cursors.move_left_nearest_token(ke.modifiers.shift, &self._token_chunks, text_buffer)
                        }
                        else{
                            self.cursors.move_left(1, ke.modifiers.shift, text_buffer);
                        }
                        true
                    },
                    KeyCode::ArrowRight=>{
                        if ke.modifiers.logo || ke.modifiers.control{ // token skipping
                            self.cursors.move_right_nearest_token(ke.modifiers.shift, &self._token_chunks, text_buffer)
                        }
                        else{
                            self.cursors.move_right(1, ke.modifiers.shift, text_buffer);
                        }
                        true
                    },
                    KeyCode::PageUp=>{
                        
                        self.cursors.move_up(self._visible_lines.max(5) - 4, ke.modifiers.shift, text_buffer);
                        true
                    },
                    KeyCode::PageDown=>{
                        self.cursors.move_down(self._visible_lines.max(5) - 4, ke.modifiers.shift, text_buffer);
                        true
                    },
                    KeyCode::Home=>{
                        self.cursors.move_home(ke.modifiers.shift, text_buffer);
                        true
                    },
                    KeyCode::End=>{
                        self.cursors.move_end(ke.modifiers.shift, text_buffer);
                        true
                    },
                    KeyCode::Backspace=>{
                        self.cursors.backspace(text_buffer);
                        true
                    },
                    KeyCode::Delete=>{
                        self.cursors.delete(text_buffer);
                        true
                    },
                    KeyCode::KeyZ=>{
                        if ke.modifiers.logo || ke.modifiers.control{
                            if ke.modifiers.shift{ // redo
                                text_buffer.redo(true, &mut self.cursors);
                                true
                            }
                            else{ // undo
                                text_buffer.undo(true, &mut self.cursors);
                                true
                            }
                        }
                        else{
                            false
                        }
                    },
                    KeyCode::KeyX=>{ // cut, the actual copy comes from the TextCopy event from the platform layer
                        if ke.modifiers.logo || ke.modifiers.control{ // cut
                            self.cursors.replace_text("", text_buffer);
                            true
                        }
                        else{
                            false
                        }
                    },
                    KeyCode::KeyA=>{ // select all
                        if ke.modifiers.logo || ke.modifiers.control{ // cut
                            self.cursors.select_all(text_buffer);
                            // don't scroll!
                            self.view.redraw_view_area(cx);
                            false
                        }
                        else{
                            false
                        }
                    },
                    KeyCode::Escape=>{
                        self.do_code_folding(cx);
                        false
                    },
                    KeyCode::Alt=>{
                        // how do we find the center line of the view
                        // its simply the top line
                        self.do_code_folding(cx);
                        false
                        //return CodeEditorEvent::FoldStart
                    },
                    KeyCode::Tab=>{
                        if ke.modifiers.shift{
                            self.cursors.remove_tab(text_buffer, 4);
                        }
                        else{
                            self.cursors.insert_tab(text_buffer,"    ");
                        } 
                        true
                    },
                    _=>false
                };
                if cursor_moved{
                    self.update_highlight(text_buffer);

                    self.scroll_last_cursor_visible(cx, text_buffer);
                    self.view.redraw_view_area(cx);
                }
            },
            Event::KeyUp(ke)=>{
                match ke.key_code{
                    KeyCode::Alt=>{
                        self.do_code_unfolding(cx);
                    },
                    KeyCode::Escape=>{
                        self.do_code_unfolding(cx);
                    }
                    _=>(),
                }
            },
            Event::TextInput(te)=>{
                if te.replace_last{
                    text_buffer.undo(false, &mut self.cursors);
                }
                
                if !te.was_paste && te.input.len() == 1{
                    match te.input.chars().next().unwrap(){
                        '\n'=>{
                            self.cursors.insert_newline_with_indent(text_buffer);
                        },
                        '('=>{
                            self.cursors.insert_around("(",")",text_buffer);
                        },
                        '['=>{
                            self.cursors.insert_around("[","]",text_buffer);
                        },
                        '{'=>{
                            self.cursors.insert_around("{","}",text_buffer);
                        },
                        ')'=>{
                            self.cursors.overwrite_if_exists(")", text_buffer);
                        },
                        ']'=>{
                            self.cursors.overwrite_if_exists("]", text_buffer);
                        },
                        '}'=>{
                            self.cursors.overwrite_if_exists("}", text_buffer);
                        },
                        _=>{
                            self.cursors.replace_text(&te.input, text_buffer);
                        }
                    }  
                    // lets insert a newline
                } 
                else{
                    self.cursors.replace_text(&te.input, text_buffer);
                }
                self.scroll_last_cursor_visible(cx, text_buffer);
                self.view.redraw_view_area(cx);
            },
            Event::TextCopy(_)=>match event{ // access the original event
                Event::TextCopy(req)=>{
                    req.response = Some(self.cursors.get_all_as_string(text_buffer));
                },
                _=>()
            },
            _=>()
        };
        CodeEditorEvent::None
   }

    fn do_code_folding(&mut self, cx:&mut Cx){
        // start code folding anim
        let speed = 0.98;
        self._anim_folding.state.do_folding(speed, 0.95);
        self._anim_folding.first_visible = self.compute_first_visible_line(cx);
        self.view.redraw_view_area(cx);
    }

    fn do_code_unfolding(&mut self, cx:&mut Cx){
        let speed = 0.96;
        self._anim_folding.state.do_opening(speed, 0.97);
        self._anim_folding.first_visible = self.compute_first_visible_line(cx);
        self.view.redraw_view_area(cx);
        // return to normal size
    }

    pub fn begin_code_editor(&mut self, cx:&mut Cx, text_buffer:&TextBuffer)->Result<(),()>{

        self.view.begin_view(cx, &Layout{..Default::default()})?;

        if text_buffer.load_id != 0{
            let bg_inst = self.bg.begin_quad(cx, &Layout{
                align:Align::left_top(),
                ..self.bg_layout.clone()
            });
            self.text.color = color("#666");
            self.text.draw_text(cx, "...");
            self.bg.end_quad(cx, &bg_inst);
            self._bg_area = bg_inst.into_area();
            self.view.end_view(cx);
            return Err(())
        }
        else{
            let bg_inst = self.bg.draw_quad(cx, Rect{x:0.,y:0., w:cx.width_total(false), h:cx.height_total(false)});
            let bg_area = bg_inst.into_area();
            cx.update_area_refs(self._bg_area, bg_area);
            self._bg_area = bg_area;

            // makers before selection
            cx.new_instance_layer(self.highlight.shader_id, 0);

            // selection before text
            cx.new_instance_layer(self.selection.shader_id, 0);

            self._text_inst = Some(self.text.begin_text(cx));
            self._instance_count = 0;
   
            if let Some(select_scroll) = &mut self._select_scroll{
                let scroll_pos = self.view.get_scroll_pos(cx);
                if self.view.set_scroll_pos(cx, Vec2{
                    x:scroll_pos.x + select_scroll.delta.x,
                    y:scroll_pos.y + select_scroll.delta.y
                }){
                    self.view.redraw_view_area(cx);
                }
                else{
                    select_scroll.at_end = true;
                }
            }

            self._scroll_pos = self.view.get_scroll_pos(cx);

            self._monospace_size = self.text.get_monospace_size(cx, None);
            self._line_geometry.truncate(0);
            self._draw_cursors = DrawCursors::new();
            self._first_on_line = true;
            self._visible_lines = 0;
            self._newline_tabs = 0;
            self._last_tabs = 0;

            let anim_folding = &mut self._anim_folding;
            if anim_folding.state.is_animating(){
                anim_folding.state.next_anim_step();
                if anim_folding.state.is_animating(){
                    self.view.redraw_view_area(cx);
                }
                anim_folding.did_animate = true;
            }
            else{
                anim_folding.did_animate = false;
            }
           
            self._paren_stack.truncate(0);
            self._anim_font_size = anim_folding.state.get_font_size(self.open_font_size, self.folded_font_size);

            self._draw_cursors.set_next(&self.cursors.set);
            // cursor after text
            cx.new_instance_layer(self.cursor.shader_id, 0);

            self._token_chunks.truncate(0);

            return Ok(())
        }
    }
    
    fn update_highlight(&mut self, text_buffer:&TextBuffer){
        self._highlight_selection = self.cursors.get_selection_highlight(text_buffer);
        self._highlight_token = self.cursors.get_token_highlight(text_buffer, &self._token_chunks);
    }

    fn new_line(&mut self, cx:&mut Cx){
        // line geometry is used for scrolling look up of cursors
        let line_geom = LineGeom{
            walk:cx.get_rel_turtle_walk(),
            font_size:self._line_largest_font
        };

        // add a bit of room to the right
        cx.turtle_new_line_min_height(self._monospace_size.y);
        self._first_on_line = true;
        let mut draw_cursors = &mut self._draw_cursors;
        if !draw_cursors.first{ // we have some selection data to emit
           draw_cursors.emit_selection(true);
           draw_cursors.first = true;
        }
       
        // highlighting the selection
        let hl_len = self._highlight_selection.len();
        if  hl_len != 0{
            for bp in 0..self._line_chunk.len().max(hl_len) - hl_len{
                let mut found = true;
                for ip in 0..hl_len{
                    if self._highlight_selection[ip] != self._line_chunk[bp+ip].1{
                        found = false;
                        break;
                    }
                }
                if found{ // output a rect
                    let pos = cx.turtle_origin();
                    let min_x = self._line_chunk[bp].0;
                    let max_x = self._line_chunk[bp + hl_len].0;
                    self.highlight.draw_quad(cx, Rect{
                        x:min_x - pos.x,
                        y:line_geom.walk.y,
                        w:max_x - min_x,
                        h:self._monospace_size.y,
                    });
                }
            }
            self._line_chunk.truncate(0);
        }
        // search for all markings
        self._line_geometry.push(line_geom);
        self._line_largest_font = self.text.font_size;

        // we could modify our visibility window here by computing the DY we are going to have the moment we know it
        if self._anim_folding.did_animate{
            let (line, pos) = self._anim_folding.first_visible;
            if line == self._line_geometry.len(){ // the line is going to be the next one
                let walk = cx.get_rel_turtle_walk();
                let dy =  pos - walk.y;
                self._scroll_pos = Vec2{
                    x:self._scroll_pos.x,
                    y:self._scroll_pos.y - dy
                };
                // update the line pos
                self._anim_folding.first_visible = (line, walk.y);
           }
        }
    }

    fn draw_tabs(&mut self, cx:&mut Cx, geom_y:f32, tabs:usize){
        let y_pos = geom_y - cx.turtle_origin().y;
        let tab_width = self._monospace_size.x*4.;
        for i in 0..tabs{
            self.tab.draw_quad(cx, Rect{
                x: (tab_width * i as f32),
                y: y_pos, 
                w: tab_width,
                h:self._monospace_size.y
            });
        }
    }
    // drawing a text chunk
    pub fn draw_chunk(&mut self, cx:&mut Cx, chunk:&Vec<char>, next_char:char, offset:usize, token_type:TokenType){
        if chunk.len()>0{
            
            // maintain paren stack
            if token_type == TokenType::ParenOpen{
                
                let marked = if let Some(pos) = self.cursors.get_last_cursor_singular(){
                    pos == offset || pos == offset + 1 && next_char != '(' &&  next_char != '{' && next_char !='['
                }
                else{false};
                
                self._paren_stack.push(ParenItem{
                    pair_start:self._token_chunks.len(),
                    geom_open:None,
                    geom_close:None,
                    marked:marked,
                    exp_paren:chunk[0]
                });
                if self._paren_stack.len() == 2 && chunk[0] == '{'{ // one level down
                    self.set_font_size(cx, self._anim_font_size);
                }
            }

            // lets check if the geom is visible
            if let Some(geom) = cx.walk_turtle_text(
                self._monospace_size.x * (chunk.len() as f32), 
                self._monospace_size.y,
                self._scroll_pos){
                
                // determine chunk color
                self.text.color = match token_type{
                    TokenType::Whitespace => {
                        if self._first_on_line && chunk[0] == ' '{
                            let tabs = chunk.len()>>2;
                            self._last_tabs = tabs;
                            self._newline_tabs = tabs;
                            self.draw_tabs(cx, geom.y, tabs);
                        }
                        self.colors.whitespace
                    },
                    TokenType::Newline=>{
                        if self._first_on_line{
                            self._newline_tabs = 0;
                            self.draw_tabs(cx, geom.y, self._last_tabs);
                        }
                        else{
                            self._last_tabs = self._newline_tabs;
                           self._newline_tabs = 0;
                        }
                        self.colors.whitespace
                    },
                    TokenType::Keyword=> self.colors.keyword,
                    TokenType::Flow=> self.colors.flow,
                    TokenType::Identifier=>{
                        if *chunk == self._highlight_token{
                            self.highlight.draw_quad_abs(cx, geom);
                        }
                        self.colors.identifier
                    }
                    TokenType::Call=>{
                        if *chunk == self._highlight_token{
                            self.highlight.draw_quad_abs(cx, geom);                            
                        }
                        self.colors.call
                    },
                    TokenType::TypeName=>{
                        if *chunk == self._highlight_token{
                            self.highlight.draw_quad_abs(cx, geom);                            
                        }
                        self.colors.type_name
                    },
                    TokenType::String=> self.colors.string,
                    TokenType::Number=> self.colors.number,
                    TokenType::Comment=> self.colors.comment,
                    TokenType::ParenOpen=>{
                        self._paren_stack.last_mut().unwrap().geom_open = Some(geom);
                        self.colors.paren
                    },
                    TokenType::ParenClose=>{
                        if let Some(paren) = self._paren_stack.last_mut(){
                            paren.geom_close = Some(geom);
                        }
                        self.colors.paren
                    },
                    TokenType::Operator=> self.colors.operator,
                    TokenType::Delimiter=> self.colors.delimiter,
                    TokenType::Block=>self.colors.operator
                };

                if self._first_on_line{
                    self._first_on_line = false;
                    self._visible_lines += 1;
                }

                // we need to find the next cursor point we need to do something at
                let cursors = &self.cursors.set;
                let last_cursor = self.cursors.last_cursor;
                let draw_cursors = &mut self._draw_cursors;
                let height = self._monospace_size.y;
                
                // actually generate the GPU data for the text
                if self._highlight_selection.len() > 0{ // slow loop
                    let draw_search = &mut self._draw_search;
                    let line_chunk = &mut self._line_chunk;
                    self.text.add_text(cx, geom.x, geom.y, offset, self._text_inst.as_mut().unwrap(), &chunk, |ch, offset, x, w|{
                        line_chunk.push((x,ch));
                        draw_search.mark_text(cursors, ch, offset, x, geom.y, w, height, last_cursor);
                        draw_cursors.mark_text(cursors, ch, offset, x, geom.y, w, height, last_cursor)
                    });
                }
                else{ // fast loop
                    self.text.add_text(cx, geom.x, geom.y, offset, self._text_inst.as_mut().unwrap(), &chunk, |ch, offset, x, w|{
                        draw_cursors.mark_text(cursors, ch, offset, x, geom.y, w, height, last_cursor)
                    });
                }
            }
            let mut pair_token = self._token_chunks.len();
            if token_type == TokenType::ParenClose{
                if self._paren_stack.len()>0{
                    let last = self._paren_stack.pop().unwrap();
                    self._token_chunks[last.pair_start].pair_token = pair_token;
                    pair_token = last.pair_start;
                    if !last.geom_open.is_none() || !last.geom_close.is_none(){
                        if let Some(pos) = self.cursors.get_last_cursor_singular(){
                            // cursor is near the last one.
                            if pos == offset || pos == offset + 1 && next_char != ')' &&  next_char != '}' && next_char !=']'{
                                // peek at the paren stack
                                if let Some(geom_open) = last.geom_open{
                                    self.paren_pair.draw_quad_abs(cx, geom_open);
                                }
                                if let Some(geom_close) = last.geom_close{
                                    self.paren_pair.draw_quad_abs(cx, geom_close);
                                }
                            }
                            // check if the cursor is near the first
                            else if last.marked{
                                if let Some(geom_open) = last.geom_open{
                                    self.paren_pair.draw_quad_abs(cx, geom_open);
                                }
                                if let Some(geom_close) = last.geom_close{
                                    self.paren_pair.draw_quad_abs(cx, geom_close);
                                }
                            }
                        };
                    }
                }
                if self._paren_stack.len() == 1{ // root level
                    self.set_font_size(cx, self.open_font_size);
                }
            }
            else if token_type == TokenType::Newline{
                self.new_line(cx);
            }

            self._instance_count += chunk.len();
            self._token_chunks.push(TokenChunk{
                offset:offset,
                pair_token:pair_token,
                len:chunk.len(),
                token_type:token_type
            });            
        }
    }

    pub fn end_code_editor(&mut self, cx:&mut Cx, text_buffer:&TextBuffer){

        // lets insert an empty newline at the bottom so its nicer to scroll
        self.new_line(cx);
        //cx.turtle_new_line();
        cx.walk_turtle(Bounds::Fix(0.0),  Bounds::Fix(self._monospace_size.y),  Margin::zero(), None);
        
        self.text.end_text(cx, self._text_inst.as_ref().unwrap());
        // lets draw cursors and selection rects.
        //let draw_cursor = &self._draw_cursor;
        let pos = cx.turtle_origin();
        cx.new_instance_layer(self.cursor.shader_id, 0);

        // draw the cursors    
        for rc in &self._draw_cursors.cursors{
           self.cursor.draw_quad(cx, Rect{x:rc.x - pos.x, y:rc.y - pos.y, w:rc.w, h:rc.h});
        }

        self._text_area = self._text_inst.take().unwrap().inst.into_area();
        // draw selections
        let sel = &mut self._draw_cursors.selections;

        // do silly selection animations
        if !self._anim_folding.state.is_animating(){
            let mut anim_select_any = false;
            for i in 0..sel.len(){
                let cur = &mut sel[i];
                let start_time = if self._select_scroll.is_none() && !self._last_finger_move.is_none(){1.}else{0.};
                // silly selection animation start
                if i < self._anim_select.len() &&  cur.rc.y < self._anim_select[i].ypos{
                    // insert new one at the top
                    self._anim_select.insert(i, AnimSelect{time:start_time, invert:true, ypos:cur.rc.y});
                }
                let (wtime, htime, invert) = if i < self._anim_select.len(){
                    let len = self._anim_select.len()-1;
                    let anim = &mut self._anim_select[i];
                    anim.ypos = cur.rc.y;
                    if anim.time <= 0.0001{
                        anim.time = 0.0
                    }
                    else{
                        anim.time = anim.time *0.55;//= 0.1;
                        anim_select_any = true;
                    }
                    if i == len{
                        (anim.time, anim.time, i == 0 && anim.invert)
                    }
                    else{
                        (anim.time, 0., i == 0 && anim.invert)
                    }
                }
                else{
                    self._anim_select.push(AnimSelect{time:start_time,invert:i == 0, ypos:cur.rc.y});
                    anim_select_any = true;
                    (start_time,start_time,false)
                };
                let wtime = 1.0 - wtime as f32;
                let htime = 1.0 - htime as f32;
                
                if invert{
                    cur.rc.w = cur.rc.w * wtime;
                    cur.rc.h = cur.rc.h * htime;
                }
                else{
                    cur.rc.x = cur.rc.x + (cur.rc.w * (1.-wtime));
                    cur.rc.w = cur.rc.w * wtime;
                    cur.rc.h = cur.rc.h * htime;
                }
            }
            self._anim_select.truncate(sel.len());
            if anim_select_any{
                self.view.redraw_view_area(cx);        
            }
        }

        // draw selections
        for i in 0..sel.len(){
            let cur = &sel[i];
            
            let mk_inst = self.selection.draw_quad(cx, Rect{x:cur.rc.x - pos.x, y:cur.rc.y - pos.y, w:cur.rc.w, h:cur.rc.h});

            // do we have a prev?
            if i > 0 && sel[i-1].index == cur.index{
                let p_rc = &sel[i-1].rc;
                mk_inst.push_vec2(cx, Vec2{x:p_rc.x - cur.rc.x, y:p_rc.w}); // prev_x, prev_w
            }
            else{
                mk_inst.push_vec2(cx, Vec2{x:0., y:-1.}); // prev_x, prev_w
            }
            // do we have a next
            if i < sel.len() - 1 && sel[i+1].index == cur.index{
                let n_rc = &sel[i+1].rc;
                mk_inst.push_vec2(cx, Vec2{x:n_rc.x - cur.rc.x, y:n_rc.w}); // prev_x, prev_w
            }
            else{
                mk_inst.push_vec2(cx, Vec2{x:0., y:-1.}); // prev_x, prev_w
            }
        }
        
         // code folding
        if self._anim_folding.state.is_folded(){
            // lets give the view a whole extra page of space
            cx.walk_turtle(Bounds::Fix(0.0),  Bounds::Fix(cx.height_total(false)),  Margin::zero(), None);
        }

        // do select scrolling
        if let Some(select_scroll) = self._select_scroll.clone(){
            if let Some(grid_select_corner) = self._grid_select_corner{
               // self.cursors.grid_select(offset, text_buffer);
                let pos = self.compute_grid_text_pos_from_abs(cx, select_scroll.abs);
                self.cursors.grid_select(grid_select_corner, pos, text_buffer);
            }
            else{
                let offset = self.text.find_closest_offset(cx, &self._text_area, select_scroll.abs);
                self.cursors.set_last_cursor_head(offset, text_buffer);
            }
            if select_scroll.at_end{
                self._select_scroll = None;
            }
        }

        // place the IME
        if self._bg_area == cx.key_focus{
            if let Some(last_cursor) = self._draw_cursors.last_cursor{
                let rc = self._draw_cursors.cursors[last_cursor];
                let scroll_pos = self.view.get_scroll_pos(cx);
                cx.show_text_ime(rc.x - scroll_pos.x, rc.y - scroll_pos.y);
            }
            else{ // current last cursors is not visible
                cx.hide_text_ime();
            }
        }
        
        self.view.end_view(cx);

        if self._anim_folding.did_animate{
            // update scroll_pos
            self.view.set_scroll_pos(cx, self._scroll_pos);
        }
    }

    // set it once per line otherwise the LineGeom stuff isn't really working out.
    pub fn set_font_size(&mut self, cx:&Cx, font_size:f32){
        self.text.font_size = font_size;
        if font_size > self._line_largest_font{
            self._line_largest_font = font_size;
        }
        self._monospace_size = self.text.get_monospace_size(cx, None);
    }

   fn scroll_last_cursor_to_top(&mut self, cx:&mut Cx, text_buffer:&TextBuffer){
       // ok lets get the last cursor pos
       let pos = self.cursors.get_last_cursor_text_pos(text_buffer);
       // lets find the line offset in the line geometry
       if pos.row < self._line_geometry.len(){
           let geom = &self._line_geometry[pos.row.max(1) - 1];
           // ok now we want the y scroll to be geom.y
           self.view.set_scroll_target(cx, Vec2{x:0.0,y:geom.walk.y});
       }
   }
 
    fn scroll_last_cursor_visible(&mut self, cx:&mut Cx, text_buffer:&TextBuffer){
        // so we have to compute (approximately) the rect of our cursor
        if self.cursors.last_cursor >= self.cursors.set.len(){
            panic!("LAST CURSOR INVALID");
        }
        //let offset = self.cursors.set[self.cursors.last_cursor].head;
        let pos = self.cursors.get_last_cursor_text_pos(text_buffer);
        //text_buffer.offset_to_text_pos(offset);
        // alright now lets query the line geometry
        let row = pos.row.min(self._line_geometry.len()-1);
        if row < self._line_geometry.len(){
            let geom = &self._line_geometry[row];
            let mono_size = self.text.get_monospace_size(cx, Some(geom.font_size));
            let rect = Rect{
                x:(pos.col as f32) * mono_size.x,
                y:geom.walk.y - mono_size.y * 1.,
                w:mono_size.x * 4.,
                h:mono_size.y * 4.
            };
            // scroll this cursor into view
            self.view.scroll_into_view(cx, rect);
        }
    }

    fn compute_grid_text_pos_from_abs(&mut self, cx:&Cx, abs:Vec2)->TextPos{
        // 
        let rel = self._bg_area.abs_to_rel_scrolled(cx, abs);
        let mut mono_size = Vec2::zero();
        for (row, geom) in self._line_geometry.iter().enumerate(){
            //let geom = &self._line_geometry[pos.row];
            mono_size = self.text.get_monospace_size(cx, Some(geom.font_size));
            if rel.y < geom.walk.y || rel.y >= geom.walk.y && rel.y <= geom.walk.y + mono_size.y{ // its on the right line
                let col = (rel.x.max(0.) / mono_size.x) as usize; // do a dumb calc
                return TextPos{row:row, col:col};
            }
        }
        // otherwise the file is too short, lets use the last line
        TextPos{row:self._line_geometry.len() - 1, col: (rel.x.max(0.) / mono_size.x) as usize}
    }

    fn compute_first_visible_line(&self, cx:&Cx)->(usize,f32){
        let scroll = self.view.get_scroll_pos(cx);
        let mut last_y = 0.0;
        for (line, geom) in self._line_geometry.iter().enumerate(){
            if geom.walk.y >= scroll.y{
                if line>0{
                    return (line - 1, last_y)
                }
                return (line, geom.walk.y)
            }
            last_y = geom.walk.y;
        }
        return (0,0.0)
    }

  

}


#[derive(Clone)]
pub struct DrawSel{
    index:usize,
    rc:Rect,
}

#[derive(Clone)]
pub struct DrawCursors{
    pub head:usize,
    pub start:usize,
    pub end:usize,
    pub next_index:usize,
    pub left_top:Vec2,
    pub right_bottom:Vec2,
    pub last_w:f32,
    pub first:bool,
    pub empty:bool,
    pub cursors:Vec<Rect>,
    pub last_cursor:Option<usize>,
    pub selections:Vec<DrawSel>
}

impl DrawCursors{
    pub fn new()->DrawCursors{
        DrawCursors{
            start:0,
            end:0,
            head:0,
            first:true,
            empty:true,
            next_index:0,
            left_top:Vec2::zero(),
            right_bottom:Vec2::zero(),
            last_w:0.0,
            cursors:Vec::new(),
            selections:Vec::new(),
            last_cursor:None
        }
    }

    pub fn set_next(&mut self, cursors:&Vec<Cursor>)->bool{
        if self.next_index < cursors.len(){
            self.emit_selection(false);
            let cursor = &cursors[self.next_index];
            let (start,end) = cursor.order();
            self.start = start;
            self.end = end;
            self.head = cursor.head;
            self.next_index += 1;
            self.last_w = 0.0;
            self.right_bottom.y = 0.;
            self.first = true;
            self.empty = true;
            true
        }
        else{
            false
        }
    }

    pub fn emit_cursor(&mut self, x:f32, y:f32, h:f32){
        self.cursors.push(Rect{
            x:x,
            y:y,
            w:1.5,
            h:h
        })
    }

    pub fn emit_selection(&mut self, on_new_line:bool){
        if !self.first{
            self.first = true;
            if !self.empty || on_new_line{
                self.selections.push(DrawSel{
                    index:self.next_index - 1,
                    rc:Rect{
                        x:self.left_top.x,
                        y:self.left_top.y,
                        w:(self.right_bottom.x - self.left_top.x) + if on_new_line{self.last_w} else {0.0},
                        h:self.right_bottom.y - self.left_top.y
                    }
                })
            }
            self.right_bottom.y = 0.;
        }
    }

    pub fn process_geom(&mut self, last_cursor:usize, offset:usize, x:f32, y:f32, w:f32, h:f32){
        if offset == self.head{ // emit a cursor
            if self.next_index - 1 == last_cursor{
                self.last_cursor = Some(self.cursors.len());
            }
            self.emit_cursor(x, y, h);
        }
        if self.first{ // store left top of rect
            self.first = false;
            self.left_top.x = x;
            self.left_top.y = y;
            self.empty = true;
        }
        else{
            self.empty = false;
        }
        // current right/bottom
        self.last_w = w;
        self.right_bottom.x = x;
        if y + h > self.right_bottom.y{
            self.right_bottom.y = y + h;
        }
    }

    pub fn mark_text(&mut self, cursors:&Vec<Cursor>, ch:char, offset:usize, x:f32, y:f32, w:f32, h:f32, last_cursor:usize)->f32{
        // check if we need to skip cursors
        while offset >= self.end{ // jump to next cursor
            if offset == self.end{ // process the last bit here
                self.process_geom(last_cursor, offset, x, y, w, h);
                self.emit_selection(false);
            }
            if !self.set_next(cursors){ // cant go further
                return 0.0
            }
        }
        // in current cursor range, update values
        if offset >= self.start && offset <= self.end{
            self.process_geom(last_cursor, offset, x, y, w, h);
            if offset == self.end{
                self.emit_selection(false);
            }
            if ch == '\n'{
                return 0.0
            }
            else if ch == ' ' && offset < self.end{
                return 2.0
            }
        }
        return 0.0
    }
}