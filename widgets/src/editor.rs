use render::*;
use crate::textbuffer::*;
use crate::scrollbar::*;
//use proc_macro2::TokenStream;
use std::str::FromStr;

#[derive(Clone)]
pub struct Editor{
    pub path:String,
    pub view:View<ScrollBar>,
    pub bg_layout:Layout,
    pub bg: Quad,
    pub tab:Quad,
    pub text: Text,
    pub _hit_state:HitState,
    pub _bg_area:Area,

    pub col_keyword:Vec4,
    pub col_flow_keyword:Vec4,
    pub col_identifier:Vec4,
    pub col_operator:Vec4,
    pub col_function:Vec4,
    pub col_number:Vec4,
    pub col_paren:Vec4,
    pub col_comment:Vec4,
    pub col_string:Vec4,
    pub col_delim:Vec4,
    pub col_type:Vec4
}

impl ElementLife for Editor{
    fn construct(&mut self, _cx:&mut Cx){}
    fn destruct(&mut self, _cx:&mut Cx){}
}

impl Style for Editor{
    fn style(cx:&mut Cx)->Self{
        let tab_sh = Self::def_tab_shader(cx);
        //let text_sh = Self::def_text_shader(cx);
        let editor = Self{
            tab:Quad{
                color:color("#5"),
                shader_id:cx.add_shader(tab_sh, "Editor.tab"),
                ..Style::style(cx)
            },
            path:"".to_string(),
            view:View{
                scroll_h:Some(ScrollBar{
                    ..Style::style(cx)
                }),
                scroll_v:Some(ScrollBar{
                    ..Style::style(cx)
                }),
                ..Style::style(cx)
            },
            bg:Quad{
                color:color256(30,30,30),
                ..Style::style(cx)
            },
            bg_layout:Layout{
                width:Bounds::Compute,
                height:Bounds::Compute,
                margin:Margin::all(0.),
                padding:Padding{l:4.0,t:4.0,r:4.0,b:4.0},
                ..Default::default()
            },
            text:Text{
                //shader_id:cx.add_shader(text_sh, "Editor.text"),
                font_id:cx.load_font(&cx.font("mono_font")),
                font_size:11.0,
                line_spacing:1.4,
                wrapping:Wrapping::Line,
                ..Style::style(cx)
            },
            _hit_state:HitState{..Default::default()},
            _bg_area:Area::Empty,
            // syntax highlighting colors
            col_keyword:color256(91,155,211),
            col_flow_keyword:color256(196,133,190),
            col_identifier:color256(212,212,212),
            col_operator:color256(212,212,212),
            col_function:color256(220,220,174),
            col_type:color256(86,201,177),
            col_number:color256(182,206,170),
            col_comment:color256(99,141,84),
            col_paren:color256(212,212,212),
            col_string:color256(204,145,123),
            col_delim:color256(212,212,212)            
        };
        //tab.animator.default = tab.anim_default(cx);
        editor
    }
}

#[derive(Clone, PartialEq)]
pub enum EditorEvent{
    None,
    Change
}

macro_rules! next_char{
    ($x:ident) =>{
        if let Some(c) = $x.next(){c}else{0 as char}
    }
}

macro_rules! match_keyword{
    ($nc:ident, $iter:ident, $chunk:ident, $( $chars:literal ),*) =>{
        $(
        if $nc == $chars{
            $chunk.push($nc);
            $nc = if let Some(c) = $iter.next(){c}else{0 as char};
            true
        }
        else{
            false
        }
        ) &&*
    }
}

impl Editor{

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
/*
subpixel AA test
    pub fn def_text_shader(cx:&mut Cx)->Shader{
       let mut sh = Text::def_text_shader(cx);
        sh.add_ast(shader_ast!({
            
            fn df_fill_keep3(color:vec4, dist:vec3)->vec4{
                let f:vec4 = vec4(
                    df_calc_blur(dist.x),
                    df_calc_blur(dist.y),
                    df_calc_blur(dist.z),
                    df_calc_blur(dist.y)
                );
                let source:vec4 = vec4(color.rgb * color.a, color.a);
                let dest:vec4 = df_result;
                df_result = source * f + dest * (1. - source.a * f);
                return df_result;
            }

            fn pixel()->vec4{
                df_viewport(tex_coord * tex_size * 0.05);

                let dist:vec2 = vec2(0.0005/df_aa,0.);

                let s1:vec4 = sample2d(texture, tex_coord.xy - dist * 2.);
                let d1:float =  max(min(s1.r, s1.g), min(max(s1.r, s1.g), s1.b)) - 0.5;

                let s2:vec4 = sample2d(texture, tex_coord.xy - dist);
                let d2:float =  max(min(s2.r, s2.g), min(max(s2.r, s2.g), s2.b)) - 0.5;

                let s3:vec4 = sample2d(texture, tex_coord.xy);
                let d3:float =  max(min(s3.r, s3.g), min(max(s3.r, s3.g), s3.b)) - 0.5;

                let s4:vec4 = sample2d(texture, tex_coord.xy + dist);
                let d4:float =  max(min(s4.r, s4.g), min(max(s4.r, s4.g), s4.b)) - 0.5;

                let s5:vec4 = sample2d(texture, tex_coord.xy + dist * 2.);
                let d5:float =  max(min(s5.r, s5.g), min(max(s5.r, s5.g), s5.b)) - 0.5;
                
                let d:vec3 = vec3(
                    - ((d1+d2+d3)/3.) -0.5 / df_aa, 
                    - ((d2+d3+d4)/3.) -0.5 / df_aa, 
                    - ((d3+d4+d5)/3.) -0.5 / df_aa
                );

                return df_fill_keep3(color, d); 
            }
        }));
        sh
    }*/

    pub fn handle_editor(&mut self, cx:&mut Cx, event:&mut Event, text_buffer:&mut TextBuffer)->EditorEvent{
        self.view.handle_scroll_bars(cx, event);
        match event.hits(cx, self._bg_area, &mut self._hit_state){
            Event::Animate(_ae)=>{
            },
            Event::FingerDown(_fe)=>{
            },
            Event::FingerHover(_fe)=>{
            },
            Event::FingerUp(_fe)=>{
            },
            Event::FingerMove(_fe)=>{
            },
            _=>()
        };
        EditorEvent::None
   }

    pub fn draw_editor(&mut self, cx:&mut Cx, text_buffer:&mut TextBuffer){
        
        // pull the bg color from our animation system, uses 'default' value otherwise
       // self.bg.color = self.animator.last_vec4("bg.color");
        // push the 2 vars we added to bg shader
        //self.text.color = self.animator.last_vec4("text.color");
        self.view.begin_view(cx, &Layout{..Default::default()});
        if text_buffer.load_id != 0{
            self._bg_area = self.bg.begin_quad(cx, &Layout{
                align:Align::center(),
                ..self.bg_layout.clone()
            });
            self.text.draw_text(cx, "Loading ...");
        }
        else{
            //let tok_str = TokenStream::from_str(&text_buffer.text);

            //let expr = syn::parse_str::<syn::File>(&text_buffer.text);
            self._bg_area = self.bg.begin_quad(cx, &self.bg_layout);
            self.draw_rust(cx, &text_buffer.text);
        }

        self.bg.end_quad(cx);
        self.view.end_view(cx);
         //self.animator.set_area(cx, self._bg_area); // if our area changed, update animation
    }



    pub fn draw_rust(&mut self, cx:&mut Cx, text:&str)->Area{
        let area = self.text.begin_chunks(cx);
        if let Area::Empty = area{
            return area
        }

        let mut chunk = Vec::new();
        let mut width = 0.0;
        let mut count = 0;
        let font_size = self.text.font_size;
        let mut iter = text.chars();
        let mut c:char;
        let mut nc = next_char!(iter);
        let glyph = &cx.fonts[self.text.font_id].glyphs[65];
        let fixed_width = glyph.advance * self.text.font_size;
        let line_height = self.text.font_size * self.text.line_spacing;
        let mut after_newline = true;
        let mut last_tabs = 0;
        let mut newline_tabs = 0;
        loop{
            let mut do_newline = false;
            c = nc; nc = next_char!(iter);
            if c == 0 as char{
                break;
            }           
            match c{
                ' '=>{ // eat as many spaces as possible
                    if after_newline{ // consume spaces in groups of 4
                        chunk.push(c);
                        let walk = cx.get_turtle_walk();
                        let mut counter = 1;
                        while nc == ' '{
                            chunk.push(nc);
                            counter += 1;
                            nc = next_char!(iter);
                        }
                        let tabs = counter >> 2;
                        let left = counter & 3;
                        last_tabs = tabs;
                        newline_tabs = tabs;
                        for _i in 0..tabs{
                            self.tab.draw_quad_walk(cx, Bounds::Fix(fixed_width*4.), Bounds::Fix(line_height), Margin::zero());
                        }
                        for _i in 0..left{
                            chunk.push(' ');
                        }
                        cx.set_turtle_walk(walk);
                    }
                    else{
                        chunk.push(c);
                        while nc == ' '{
                            chunk.push(nc);
                            nc = next_char!(iter);
                        }
                    }
                },
                '\t'=>{ // eat as many tabs as possible
                    // lets output tab lines
                    self.tab.draw_quad_walk(cx, Bounds::Fix(fixed_width*4.), Bounds::Fix(line_height), Margin::zero());
                    //chunk.push(c);
                    while nc == '\t'{
                        self.tab.draw_quad_walk(cx, Bounds::Fix(fixed_width*4.), Bounds::Fix(line_height), Margin::zero());
                        //chunk.push(nc);
                        nc = next_char!(iter);
                    }
                },
                '/'=>{
                    after_newline = false;
                    chunk.push(c);
                    if nc == '/'{
                        while nc != '\n'{
                            chunk.push(nc);
                            nc = next_char!(iter);
                        }
                        self.text.color = self.col_comment;
                    }
                    else{
                        self.text.color = self.col_operator;
                    }
                },
                '\''=>{
                    let mut lc = 0 as char;
                    chunk.push(c);
                    while nc != (0 as char)  && nc!='\n' && (nc != '\'' || lc != '\\' && c == '\\' && nc == '\''){
                        chunk.push(nc);
                        lc = c;
                        c = nc;
                        nc = next_char!(iter);
                    };
                    chunk.push(nc);
                    nc = next_char!(iter);
                    self.text.color = self.col_string;
                },
                '"'=>{
                    chunk.push(c);
                    while nc != (0 as char) && nc!='\n' && (nc != '"' || c == '\\' && nc == '"'){
                        chunk.push(nc);
                        nc = next_char!(iter);
                    };
                    chunk.push(nc);
                    nc = next_char!(iter);
                    self.text.color = self.col_string;
                },
                '\r'=>{
                },
                '\n'=>{
                    if after_newline{
                        for _i in 0..last_tabs{
                            self.tab.draw_quad_walk(cx, Bounds::Fix(fixed_width*4.), Bounds::Fix(line_height), Margin::zero());
                        }
                    }
                    else {
                        last_tabs = newline_tabs;
                    }
                    chunk.push(c);
                    do_newline = true;
                    after_newline = true;
                    newline_tabs = 0;
                },
                '0'...'9'=>{ // try to parse numbers
                    after_newline = false;
                    self.text.color = self.col_number;
                    chunk.push(c);
                    if nc == 'x'{ // parse a hex number
                        chunk.push(nc);
                        nc = next_char!(iter);
                        while nc >= '0' && nc <='9' || nc >= 'a' && nc <= 'f' || nc >= 'A' && nc <='F' || nc == '_'{
                            chunk.push(nc);
                            nc = next_char!(iter);
                        }
                    }
                    else if nc == 'b'{ // parse a binary
                        chunk.push(nc);
                        nc = next_char!(iter);
                        while nc == '0' || nc =='1' || nc == '_'{
                            chunk.push(nc);
                            nc = next_char!(iter);
                        }
                    }
                    else{
                        while nc >= '0' && nc <='9' || nc == '_'{
                            chunk.push(nc);
                            nc = next_char!(iter);
                        }
                        if nc == 'u' || nc == 'i'{
                            chunk.push(nc);
                            nc = next_char!(iter);
                            if match_keyword!(nc,iter,chunk,'8'){
                            }
                            else if match_keyword!(nc,iter,chunk,'1','6'){
                            }
                            else if match_keyword!(nc,iter,chunk,'3','2'){
                            }
                            else if match_keyword!(nc,iter,chunk,'6','4'){
                            }
                        }
                        else if nc == '.'{
                            chunk.push(nc);
                            nc = next_char!(iter);
                            // again eat as many numbers as possible
                            while nc >= '0' && nc <='9' || nc == '_'{
                                chunk.push(nc);
                                nc = next_char!(iter);
                            }
                            if nc == 'f' { // the f32, f64 postfix
                                chunk.push(nc);
                                nc = next_char!(iter);
                                if match_keyword!(nc,iter,chunk,'3','2'){
                                }
                                else if match_keyword!(nc,iter,chunk,'6','4'){
                                }
                            }
                        }
                    }
                },
                '(' | ')'=>{
                    after_newline = false;
                    chunk.push(c);
                    self.text.color = self.col_paren;
                },
                '{' | '}'=>{
                    after_newline = false;
                    chunk.push(c);
                    self.text.color = self.col_paren;
                },
                '[' | ']'=>{
                    after_newline = false;
                    chunk.push(c);
                    self.text.color = self.col_paren;
                },
                'a'...'z'=>{ // try to parse keywords or identifiers
                    after_newline = false;
                    chunk.push(c);
                    let mut is_keyword = false;
                    let mut is_flow_keyword = false;

                    match c{
                        'a'=>{
                            if match_keyword!(nc,iter,chunk,'s'){
                                is_keyword = true;
                            }
                        },
                        'b'=>{ 
                            if match_keyword!(nc,iter,chunk,'r','e','a','k'){
                                is_flow_keyword = true;
                                is_keyword = true;
                            }
                        },
                        'c'=>{
                            if match_keyword!(nc,iter,chunk,'o'){
                                if match_keyword!(nc,iter,chunk,'n','s','t'){
                                    is_keyword = true;
                                }
                                else if match_keyword!(nc,iter,chunk,'n','t','i','n','u','e'){
                                    is_flow_keyword = true;
                                    is_keyword = true;
                                }
                            }
                            else if match_keyword!(nc,iter,chunk,'r','a','t','e'){
                                is_keyword = true;
                            }
                        },
                        'e'=>{
                            if match_keyword!(nc,iter,chunk,'l','s','e'){
                                is_flow_keyword = true;
                                is_keyword = true;
                            }
                            else if match_keyword!(nc,iter,chunk,'n','u','m'){
                                is_keyword = true;
                            }
                            else if match_keyword!(nc,iter,chunk,'x','t','e','r','n'){
                                is_keyword = true;
                            }
                        },
                        'f'=>{
                            if match_keyword!(nc,iter,chunk,'a','l','s','e'){
                                is_keyword = true;
                            }
                            else if match_keyword!(nc,iter,chunk,'n'){
                                is_keyword = true;
                            }
                            else if match_keyword!(nc,iter,chunk,'o','r'){
                                is_flow_keyword = true;
                                is_keyword = true;
                            }
                        },
                        'i'=>{
                            if match_keyword!(nc,iter,chunk,'f'){
                                is_flow_keyword = true;
                                is_keyword = true;
                            }
                            else if match_keyword!(nc,iter,chunk,'m','p','l'){
                                is_keyword = true;
                            }
                            else if match_keyword!(nc,iter,chunk,'i','n'){
                                is_keyword = true;
                            }
                        },
                        'l'=>{
                            if match_keyword!(nc,iter,chunk,'e','t'){
                                is_keyword = true;
                            }
                            else if match_keyword!(nc,iter,chunk,'o','o','p'){
                                is_flow_keyword = true;
                                is_keyword = true;
                            }
                        },
                        'm'=>{
                            if match_keyword!(nc,iter,chunk,'a','t','c'){
                                is_keyword = true;
                                is_flow_keyword = true;
                            }
                            else if match_keyword!(nc,iter,chunk,'o'){
                                if match_keyword!(nc,iter,chunk,'d'){
                                    is_keyword = true;
                                }
                                else if match_keyword!(nc,iter,chunk,'v','e'){
                                    is_keyword = true;
                                }
                            }
                            else if match_keyword!(nc,iter,chunk,'u','t'){
                                is_keyword = true;
                            }
                        },
                        'p'=>{ // pub
                            if match_keyword!(nc,iter,chunk,'u','b'){ 
                                is_keyword = true;
                            }
                        },
                        'r'=>{
                            if match_keyword!(nc,iter,chunk,'e'){
                                if match_keyword!(nc,iter,chunk,'f'){
                                    is_keyword = true;
                                }
                                else if match_keyword!(nc,iter,chunk,'t','u','r','n'){
                                    is_keyword = true;
                                    is_flow_keyword = true;
                                }
                            }
                        },
                        's'=>{
                            if match_keyword!(nc,iter,chunk,'e','l','f'){
                                is_keyword = true;
                            }
                            if match_keyword!(nc,iter,chunk,'u','p','e','r'){
                                is_keyword = true;
                            }
                            else if match_keyword!(nc,iter,chunk,'t'){
                                if match_keyword!(nc,iter,chunk,'a','t','i','c'){
                                    is_keyword = true;
                                }
                                else if match_keyword!(nc,iter,chunk,'r','u','c','t'){
                                    is_keyword = true;
                                }
                            }
                        },
                        't'=>{
                            if match_keyword!(nc,iter,chunk,'y','p','e'){
                                is_keyword = true;
                            }
                            else if match_keyword!(nc,iter,chunk,'r'){
                                if match_keyword!(nc,iter,chunk,'r','a','i','t'){
                                    is_keyword = true;
                                }
                                else if match_keyword!(nc,iter,chunk,'u','e'){
                                    is_keyword = true;
                                }
                            }
                        },
                        'u'=>{ // use
                            if match_keyword!(nc,iter,chunk,'s','e'){ 
                                is_keyword = true;
                            }
                            else if match_keyword!(nc,iter,chunk,'n','s','a','f','e'){ 
                                is_keyword = true;
                            }
                        },
                        'w'=>{ // use
                            if match_keyword!(nc,iter,chunk,'h'){
                                if match_keyword!(nc,iter,chunk,'e','r','e'){
                                    is_keyword = true;
                                }
                                else if match_keyword!(nc,iter,chunk,'i','l','e'){
                                    is_flow_keyword = true;
                                    is_keyword = true;
                                }
                            }
                        }, 
                         _=>{}
                    }

                    while nc >= '0' && nc <='9' || nc >= 'a' && nc <= 'z' || nc >= 'A' && nc <='Z' || nc == '_' || nc == '$'{
                        is_keyword = false;
                        chunk.push(nc);
                        nc = next_char!(iter);
                    }
                    if is_keyword{
                        if is_flow_keyword{
                            self.text.color = self.col_flow_keyword;
                        }
                        else{
                            self.text.color = self.col_keyword;
                        }
                    }
                    else{
                        if nc == '(' || nc == '!'{
                            self.text.color = self.col_function;
                        }
                        else{
                            self.text.color = self.col_identifier;
                        }
                    }
                   
                },
                'A'...'Z'=>{
                    after_newline = false;
                    chunk.push(c);
                    let mut is_keyword = false;
                    if c == 'S'{
                        if match_keyword!(nc,iter,chunk,'e','l','f'){
                            is_keyword = true;
                        }
                    }

                    while nc >= '0' && nc <='9' || nc >= 'a' && nc <= 'z' || nc >= 'A' && nc <='Z' || nc == '_' || nc == '$'{
                        is_keyword = false;
                        chunk.push(nc);
                        nc = next_char!(iter);
                    }
                    if is_keyword{
                        self.text.color = self.col_keyword;
                    }
                    else{
                        self.text.color = self.col_type;
                    }
                },
                _=>{
                    after_newline = false;
                    chunk.push(c);
                    // unknown type
                    self.text.color = self.col_identifier;
                }
            }
            if chunk.len()>0{
                let height = font_size * self.text.line_spacing;
                let geom = cx.walk_turtle(
                    Bounds::Fix(fixed_width * (chunk.len() as f32)), 
                    Bounds::Fix(height), 
                    Margin::zero(),
                    None
                );

                self.text.draw_chunk(cx, geom.x, geom.y, &area, &chunk);
                count += chunk.len();
                width = 0.0;
                chunk.truncate(0);
                if do_newline{
                    cx.turtle_new_line();
                }
            }
        }
        self.text.end_chunks(cx, count);
        return area
    }
}