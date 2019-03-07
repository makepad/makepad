use shui::*;

#[derive(Clone, Element)]
pub struct Splitter{
    pub hit_state:HitState,
    pub bg_area:Area,
    pub bg: Quad,
    pub axis:Axis,
    pub split_size:f32,
    pub split_mode:SplitterMode,
    pub split_pos:f32,
    pub anim:Animation<SplitterState>,
    pub is_moving:bool
}

#[derive(Clone, PartialEq)]
pub enum SplitterMode{
    AlignBegin,
    AlignEnd,
    Factor
}


#[derive(Clone, PartialEq)]
pub enum SplitterState{
    Default,
    Over,
    Moving
}

pub trait SplitterLike{
    fn handle_splitter(&mut self, cx:&mut Cx, event:&mut Event)->SplitterEvent;
    fn begin_splitter(&mut self, cx:&mut Cx, split_mode:SplitterMode, split_pos:f32, axis:Axis);
    fn mid_splitter(&mut self, cx:&mut Cx);
    fn end_splitter(&mut self, cx:&mut Cx);
}

impl Style for Splitter{
    fn style(cx:&mut Cx)->Self{
        let bg_sh = Self::def_bg_shader(cx);
        Self{
            hit_state:HitState{
                ..Default::default()
            },
            split_mode:SplitterMode::AlignBegin,
            split_pos:50.0,
            split_size:8.0,
            is_moving:false,
            axis:Axis::Horizontal,
            bg_area:Area::Empty,
            anim:Animation::new(
                SplitterState::Default,
                vec![
                    AnimState::new(
                        SplitterState::Default,
                        AnimMode::Cut{duration:0.5}, 
                        vec![
                            AnimTrack::to_vec4("bg.color",cx.style.bg_normal),
                        ]
                    ),
                    AnimState::new(
                        SplitterState::Over,
                        AnimMode::Cut{duration:0.05}, 
                        vec![
                            AnimTrack::to_vec4("bg.color", cx.style.bg_top),
                        ]
                    ),
                    AnimState::new(
                        SplitterState::Moving,
                        AnimMode::Cut{duration:0.2}, 
                        vec![
                            AnimTrack::vec4("bg.color", Ease::Linear, vec![
                                (0.0, color("#f")),
                                (1.0, color("#6"))
                            ]),
                        ]
                    ) 
                ]
            ),
            bg:Quad{
                shader_id:cx.add_shader(bg_sh),
                ..Style::style(cx)
            }
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum SplitterEvent{
    None,
    Moving,
    Moved
}

impl Splitter{
    pub fn def_bg_shader(cx:&mut Cx)->Shader{
        let mut sh = Quad::def_quad_shader(cx);
        sh.add_ast(shader_ast!({

            const border_radius:float = 1.5;

            fn pixel()->vec4{
                df_viewport(pos * vec2(w, h));
                df_box(0., 0., w, h, border_radius);
                return df_fill(color);
            }
        }));
        sh
    }
}

impl SplitterLike for Splitter{
    fn handle_splitter(&mut self, cx:&mut Cx, event:&mut Event)->SplitterEvent{
        let mut ret_event = SplitterEvent::None;
        match event.hits(cx, self.bg_area, &mut self.hit_state){
            Event::Animate(ae)=>{
                self.anim.calc_area(cx, "bg.color", ae.time, self.bg_area);
            },
            Event::FingerDown(_fe)=>{
                ret_event = SplitterEvent::Moving;
                self.is_moving = true;
                self.anim.change_state(cx, SplitterState::Moving);
                cx.set_down_mouse_cursor(MouseCursor::Crosshair);
            },
            Event::FingerHover(fe)=>{
                cx.set_hover_mouse_cursor(MouseCursor::Hand);
                match fe.hover_state{
                    HoverState::In=>{
                        if self.is_moving{
                            self.anim.change_state(cx, SplitterState::Moving);
                        }
                        else{
                            self.anim.change_state(cx, SplitterState::Over);
                        }
                    },
                    HoverState::Out=>{
                        self.anim.change_state(cx, SplitterState::Default);
                    },
                    _=>()
                }
            },
            Event::FingerUp(fe)=>{
                self.is_moving = false;
                if fe.is_over{
                    if !fe.is_touch{
                        self.anim.change_state(cx, SplitterState::Over);
                    }
                    else{
                        self.anim.change_state(cx, SplitterState::Default);
                    }
                    ret_event = SplitterEvent::Moved;
                }
                else{
                    self.anim.change_state(cx, SplitterState::Default);
                }
            },
            _=>()
        };
        ret_event
   }

   fn begin_splitter(&mut self, cx:&mut Cx, split_mode:SplitterMode, split_pos:f32, axis:Axis){
       self.axis = axis;
       self.split_mode = split_mode;
       self.split_pos = split_pos;
       match self.axis{
            Axis::Horizontal=>{
                cx.begin_turtle(&Layout{
                    width:Bounds::Fill,
                    height:Bounds::Fix(split_pos - self.split_size * 0.5),
                    ..Default::default()
                })
            },
            Axis::Vertical=>{
                cx.begin_turtle(&Layout{
                    width:Bounds::Fix(split_pos - self.split_size * 0.5),
                    height:Bounds::Fill,
                    ..Default::default()
                })
            }
       }
   }

   fn mid_splitter(&mut self, cx:&mut Cx){
        match self.axis{
            Axis::Horizontal=>{
                cx.end_turtle();
                cx.move_turtle(0.0,self.split_size);
                cx.begin_turtle(&Layout{
                    width:Bounds::Fill,
                    height:Bounds::Fill,
                    ..Default::default()
                })
            },
            Axis::Vertical=>{
                cx.end_turtle();
                cx.move_turtle(self.split_size, 0.0);
                cx.begin_turtle(&Layout{
                    width:Bounds::Fill,
                    height:Bounds::Fill,
                    ..Default::default()
                })
            }
       }
   }

   fn end_splitter(&mut self, cx:&mut Cx){
       cx.end_turtle();
   }
/*
    pub fn draw_with_label(&mut self, cx:&mut Cx, label: &str){
        
        // pull the bg color from our animation system, uses 'default' value otherwise
        self.bg.color = self.anim.last_vec4("bg.color");
        self.bg_area = self.bg.begin(cx, &Layout{
            w:Value::Fixed(self.anim.last_float("width")),
            ..self.bg_layout.clone()
        });
        // push the 2 vars we added to bg shader
        self.anim.last_push(cx, "bg.border_color", self.bg_area);
        self.anim.last_push(cx, "bg.glow_size", self.bg_area);

        self.text.draw_text(cx, Computed, Computed, label);
        
        self.bg.end(cx);

        self.anim.set_area(cx, self.bg_area); // if our area changed, update animation
    }
    */
}
