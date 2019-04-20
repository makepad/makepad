use widgets::*;
use crate::textbuffer::*;

#[derive(Clone)]
pub struct CodeEditor{
    pub view:View<ScrollBar>,
    pub bg_layout:Layout,
    pub bg: Quad,
    pub cursor: Quad,
    pub tab:Quad,
    pub text: Text,
    pub cursors:Vec<Cursor>,
    pub _hit_state:HitState,
    pub _bg_area:Area,
    pub _text_inst:Option<AlignedInstance>,
    pub _scroll:Vec2,
    pub _monospace_size:Vec2,
    pub _instance_count:usize
}

impl ElementLife for CodeEditor{
    fn construct(&mut self, _cx:&mut Cx){}
    fn destruct(&mut self, _cx:&mut Cx){}
}

impl Style for CodeEditor{
    fn style(cx:&mut Cx)->Self{
        let tab_sh = Self::def_tab_shader(cx);

        let code_editor = Self{
            cursors:vec![Cursor{head:0,tail:0,max:0}],
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
                    ..Style::style(cx)
                }),
                ..Style::style(cx)
            },
            bg:Quad{
                color:color256(30,30,30),
                do_scroll:false,
                ..Style::style(cx)
            },
            cursor:Quad{
                color:color256(30,30,30),
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
                line_spacing:1.4,
                wrapping:Wrapping::Line,
                ..Style::style(cx)
            },
            _hit_state:HitState{no_scrolling:true, ..Default::default()},
            _monospace_size:vec2(0.,0.),
            _scroll:vec2(0.,0.),
            _bg_area:Area::Empty,
            _text_inst:None,
            _instance_count:0,
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
    pub fn handle_code_editor(&mut self, cx:&mut Cx, event:&mut Event, text_buffer:&mut TextBuffer)->CodeEditorEvent{
        match self.view.handle_scroll_bars(cx, event){
            (_,ScrollBarEvent::Scroll{..})=>{
                self.view.redraw_view_area(cx);
            },
            _=>()
        }
        match event.hits(cx, self._bg_area, &mut self._hit_state){
            Event::Animate(_ae)=>{
            },
            Event::FingerDown(_fe)=>{
                // give us the focus
                cx.set_key_focus(self._bg_area)
            },
            Event::FingerHover(_fe)=>{
            },
            Event::FingerUp(_fe)=>{
            },
            Event::FingerMove(_fe)=>{
            },
            Event::KeyDown(ke)=>{
                match ke.key_code{
                    KeyCode::ArrowUp=>{
                        self.cursors_move_up(1, ke.with_shift, text_buffer);
                    },
                    KeyCode::ArrowDown=>{
                        self.cursors_move_down(1, ke.with_shift, text_buffer);
                    },
                    KeyCode::ArrowLeft=>{
                        self.cursors_move_left(1, ke.with_shift, text_buffer);
                    },
                    KeyCode::ArrowRight=>{
                        self.cursors_move_right(1, ke.with_shift, text_buffer);
                    },
                    _=>()
                }
            },
            Event::TextInput(te)=>{
                println!("TextInput {:?}", te);
            }
            _=>()
        };
        CodeEditorEvent::None
   }

    pub fn begin_code_editor(&mut self, cx:&mut Cx, text_buffer:&TextBuffer)->bool{
        // pull the bg color from our animation system, uses 'default' value otherwise
        // self.bg.color = self.animator.last_vec4("bg.color");
        // push the 2 vars we added to bg shader
        //self.text.color = self.animator.last_vec4("text.color");
        self.view.begin_view(cx, &Layout{..Default::default()});
        //   return false
        //}
        if text_buffer.load_id != 0{
            let bg_inst = self.bg.begin_quad(cx, &Layout{
                align:Align::center(),
                ..self.bg_layout.clone()
            });
            self.text.draw_text(cx, "Loading ...");
            self.bg.end_quad(cx, &bg_inst);
            self._bg_area = bg_inst.get_area();
            self.view.end_view(cx);
            return false
        }
        else{
            let bg_inst =self.bg.draw_quad(cx, 0.,0., cx.width_total(false), cx.height_total(false));
            self._bg_area = bg_inst.get_area();

            self._text_inst = Some(self.text.begin_text(cx));
            self._instance_count = 0;
            self._scroll = self.view.get_scroll(cx);
            self._monospace_size = self.text.get_monospace_size(cx);
            
            return true
        }
    }
    
    pub fn end_code_editor(&mut self, cx:&mut Cx, text_buffer:&TextBuffer){
        self.text.end_text(cx, self._text_inst.as_ref().unwrap());
        self.view.end_view(cx);
    }

    pub fn draw_tab_lines(&mut self, cx:&mut Cx, tabs:usize){
        let walk = cx.get_turtle_walk();
        let tab_width = self._monospace_size.x*4.;
        if cx.visible_in_turtle(&Rect{x:walk.x, y:walk.y, w:tab_width * tabs as f32, h:self._monospace_size.y}, &self._scroll){
            for _i in 0..tabs{
                self.tab.draw_quad_walk(cx, Bounds::Fix(tab_width), Bounds::Fix(self._monospace_size.y), Margin::zero());
            }   
            cx.set_turtle_walk(walk);
        }
    }

    pub fn new_line(&mut self, cx:&mut Cx){
        cx.turtle_new_line();
    }

    pub fn draw_text(&mut self, cx:&mut Cx, chunk:&Vec<char>, color:&Vec4){
        if chunk.len()>0{
            let geom = cx.walk_turtle(
                Bounds::Fix(self._monospace_size.x * (chunk.len() as f32)), 
                Bounds::Fix(self._monospace_size.y), 
                Margin::zero(),
                None
            );

            // we need to walk our cursor iterator, 
                
            // lets check if the geom is visible
            if cx.visible_in_turtle(&geom, &self._scroll){
                self.text.color = *color;
                self.text.add_text(cx, geom.x, geom.y, self._text_inst.as_mut().unwrap(), &chunk);
            }

            self._instance_count += chunk.len();
        }
    }

    pub fn cursors_merge(&mut self){
        // merge all cursors
    }

    pub fn cursors_move_up(&mut self, line_count:usize, only_head:bool, text_buffer:&TextBuffer){
        for cursor in &mut self.cursors{
            cursor.move_up(line_count, text_buffer);
            if !only_head{cursor.tail = cursor.head}
        }
        self.cursors_merge()
    }

    pub fn cursors_move_down(&mut self,line_count:usize, only_head:bool, text_buffer:&TextBuffer){
        for cursor in &mut self.cursors{
            cursor.move_down(line_count, text_buffer);
            if !only_head{cursor.tail = cursor.head}
        }
        self.cursors_merge()
    }

    pub fn cursors_move_left(&mut self, char_count:usize, only_head:bool, text_buffer:&TextBuffer){
        for cursor in &mut self.cursors{
            cursor.move_left(char_count, text_buffer);
            if !only_head{cursor.tail = cursor.head}
        }
        self.cursors_merge()
    }

    pub fn cursors_move_right(&mut self,char_count:usize, only_head:bool, text_buffer:&TextBuffer){
        for cursor in &mut self.cursors{
            cursor.move_left(char_count, text_buffer);
            if !only_head{cursor.tail = cursor.head}
        }
        self.cursors_merge()
    }

    pub fn cursors_replace_text(&mut self, text:&str, text_buffer:&mut TextBuffer){

    }
}

#[derive(Clone)]
pub struct Cursor{
    head:usize,
    tail:usize,
    max:usize
}

impl Cursor{
    pub fn has_selection(&self)->bool{
        self.head != self.tail
    }

    pub fn sort(&self)->(usize,usize){
        if self.head > self.tail{
            (self.tail,self.head)
        }
        else{
            (self.head,self.tail)
        }
    }

    pub fn calc_max(&mut self, text_buffer:&TextBuffer){
        let (_row,col) = text_buffer.offset_to_row_col(self.head);
        self.max = col;
    }

    pub fn move_home(&mut self, text_buffer:&TextBuffer){
        self.head = 0;
        self.calc_max(text_buffer);
    }

    pub fn move_end(&mut self, text_buffer:&TextBuffer){
        self.head = text_buffer.get_char_count();
        self.calc_max(text_buffer);
    }

    pub fn move_left(&mut self, char_count:usize,  text_buffer:&TextBuffer){
        if self.head >= char_count{
            self.head -= char_count;
        }
        else{
            self.head = 0;
        }
        self.calc_max(text_buffer);
    }

    pub fn move_right(&mut self, char_count:usize, text_buffer:&TextBuffer){
        if self.head + char_count < text_buffer.get_char_count(){
            self.head += char_count;
        }
        else{
            self.head = text_buffer.get_char_count();
        }
        self.calc_max(text_buffer);
    }

    pub fn move_up(&mut self, line_count:usize, text_buffer:&TextBuffer){
        let (row,_col) = text_buffer.offset_to_row_col(self.head);
        if row >= line_count {
            self.head = text_buffer.row_col_to_offset(row - line_count, self.max);
        }
        else{
            self.head = 0;
        }
    }
    
    pub fn move_down(&mut self, line_count:usize, text_buffer:&TextBuffer){
        let (row,_col) = text_buffer.offset_to_row_col(self.head);
        if row + line_count < text_buffer.get_line_count(){
            self.head = text_buffer.row_col_to_offset(row + line_count, self.max);
        }
        else{
            self.head = text_buffer.get_char_count();
        }
    }
}
