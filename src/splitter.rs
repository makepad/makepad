use shui::*;

#[derive(Clone, Element)]
pub struct Splitter{
    pub hit_state:HitState,
    pub split_area:Area,
    pub split: Quad,
    pub axis:Axis,
    pub draw_size:f32,
    pub align:SplitterAlign,
    pub pos:f32,
    pub calc_pos:f32,
    pub anim:Animation<SplitterState>,
    pub is_moving:bool,
    pub drag_point:f32,
    pub drag_pos_start:f32,
    pub drag_max_pos:f32,
    pub min_pos_offset:f32
}

#[derive(Clone, PartialEq)]
pub enum SplitterAlign{
    First,
    Last,
    Weighted
}


#[derive(Clone, PartialEq)]
pub enum SplitterState{
    Default,
    Over,
    Moving
}

pub trait SplitterLike{
    fn handle_splitter(&mut self, cx:&mut Cx, event:&mut Event)->SplitterEvent;
    fn set_splitter_state(&mut self, align:SplitterAlign, pos:f32, axis:Axis);
    fn begin_splitter(&mut self, cx:&mut Cx);
    fn mid_splitter(&mut self, cx:&mut Cx);
    fn end_splitter(&mut self, cx:&mut Cx);
}

impl Style for Splitter{
    fn style(cx:&mut Cx)->Self{
        let split_sh = Self::def_split_shader(cx);
        Self{
            hit_state:HitState{
                ..Default::default()
            },
            align:SplitterAlign::First,
            pos:50.0,
            calc_pos:0.0,
            min_pos_offset:25.0,
            drag_max_pos:0.0,
            draw_size:8.0,
            is_moving:false,
            axis:Axis::Horizontal,
            split_area:Area::Empty,
            drag_point:0.,
            drag_pos_start:0.,
            anim:Animation::new(
                SplitterState::Default,
                vec![
                    AnimState::new(
                        SplitterState::Default,
                        AnimMode::Cut{duration:0.5}, 
                        vec![
                            AnimTrack::to_vec4("split.color",cx.style.bg_normal),
                        ]
                    ),
                    AnimState::new(
                        SplitterState::Over,
                        AnimMode::Cut{duration:0.05}, 
                        vec![
                            AnimTrack::to_vec4("split.color", color("#5")),
                        ]
                    ),
                    AnimState::new(
                        SplitterState::Moving,
                        AnimMode::Cut{duration:0.2}, 
                        vec![
                            AnimTrack::vec4("split.color", Ease::Linear, vec![
                                (0.0, color("#f")),
                                (1.0, color("#6"))
                            ]),
                        ]
                    ) 
                ]
            ),
            split:Quad{
                shader_id:cx.add_shader(split_sh),
                ..Style::style(cx)
            }
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum SplitterEvent{
    None,
    Moving{new_pos:f32},
}

impl Splitter{
    pub fn def_split_shader(cx:&mut Cx)->Shader{
        let mut sh = Quad::def_quad_shader(cx);
        sh.add_ast(shader_ast!({

            const border_radius:float = 1.5;

            fn pixel()->vec4{
                df_viewport(pos * vec2(w, h));
                df_box(0., 0., w, h, 0.5);
                return df_fill(color);
            }
        }));
        sh
    }
}

impl SplitterLike for Splitter{
    fn handle_splitter(&mut self, cx:&mut Cx, event:&mut Event)->SplitterEvent{
        let mut ret_event = SplitterEvent::None;
        match event.hits(cx, self.split_area, &mut self.hit_state){
            Event::Animate(ae)=>{
                self.anim.calc_area(cx, "split.color", ae.time, self.split_area);
            },
            Event::FingerDown(fe)=>{
                self.is_moving = true;
                self.anim.change_state(cx, SplitterState::Moving);
                match self.axis{
                    Axis::Horizontal=>cx.set_down_mouse_cursor(MouseCursor::RowResize),
                    Axis::Vertical=>cx.set_down_mouse_cursor(MouseCursor::ColResize)
                };
                self.drag_pos_start = self.pos;
                self.drag_point = match self.axis{
                    Axis::Horizontal=>{fe.rel_y},
                    Axis::Vertical=>{fe.rel_x}
                }
            },
            Event::FingerHover(fe)=>{
                match self.axis{
                    Axis::Horizontal=>cx.set_hover_mouse_cursor(MouseCursor::RowResize),
                    Axis::Vertical=>cx.set_hover_mouse_cursor(MouseCursor::ColResize)
                };
                if !self.is_moving{
                    match fe.hover_state{
                        HoverState::In=>{
                            self.anim.change_state(cx, SplitterState::Over);
                        },
                        HoverState::Out=>{
                            self.anim.change_state(cx, SplitterState::Default);
                        },
                        _=>()
                    }
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
                }
                else{
                    self.anim.change_state(cx, SplitterState::Default);
                }
            },
            Event::FingerMove(fe)=>{

                let delta = match self.axis{
                    Axis::Horizontal=>{
                        fe.start_y - fe.abs_y
                    },
                    Axis::Vertical=>{
                        fe.start_x - fe.abs_x
                    }
                };
                let mut pos = match self.align{
                    SplitterAlign::First=>self.drag_pos_start - delta,
                    SplitterAlign::Last=>self.drag_pos_start + delta,
                    SplitterAlign::Weighted=>self.drag_pos_start * self.drag_max_pos - delta
                };
                if pos > self.drag_max_pos - self.min_pos_offset{
                    pos = self.drag_max_pos - self.min_pos_offset
                }
                else if pos < self.min_pos_offset{
                    pos = self.min_pos_offset
                };
                let calc_pos = match self.align{
                    SplitterAlign::First=>{
                        self.pos = pos;
                        pos
                    },
                    SplitterAlign::Last=>{
                        self.pos = pos;
                        self.drag_max_pos - pos
                    },
                    SplitterAlign::Weighted=>{
                        self.pos = pos / self.drag_max_pos;
                        pos
                    }
                };
                if calc_pos != self.calc_pos{
                    self.calc_pos = calc_pos;
                    ret_event = SplitterEvent::Moving{new_pos:self.pos};
                    cx.dirty_area = self.split_area;
                }
            }
            _=>()
        };
        ret_event
   }

   fn set_splitter_state(&mut self, align:SplitterAlign, pos:f32, axis:Axis){
       self.axis = axis;
       self.align = align;
       self.pos = pos;
   }

   fn begin_splitter(&mut self, cx:&mut Cx){
       let rect = cx.turtle_rect();
       self.calc_pos = match self.align{
           SplitterAlign::First=>self.pos,
           SplitterAlign::Last=>match self.axis{
               Axis::Horizontal=>rect.h - self.pos,
               Axis::Vertical=>rect.w - self.pos
           },
           SplitterAlign::Weighted=>self.pos * match self.axis{
               Axis::Horizontal=>rect.h,
               Axis::Vertical=>rect.w 
           }
       };
       match self.axis{
            Axis::Horizontal=>{
                cx.begin_turtle(&Layout{
                    width:Bounds::Fill,
                    height:Bounds::Fix(self.calc_pos),
                    ..Default::default()
                })
            },
            Axis::Vertical=>{
                cx.begin_turtle(&Layout{
                    width:Bounds::Fix(self.calc_pos),
                    height:Bounds::Fill,
                    ..Default::default()
                })
            }
       }
   }

   fn mid_splitter(&mut self, cx:&mut Cx){
        cx.end_turtle();
        match self.axis{
            Axis::Horizontal=>{
                cx.move_turtle(0.0,self.calc_pos + self.draw_size);
            },
            Axis::Vertical=>{
                cx.move_turtle(self.calc_pos + self.draw_size, 0.0);
            }
       };
       cx.begin_turtle(&Layout{
            width:Bounds::Fill,
            height:Bounds::Fill,
            ..Default::default()
       });
   }

   fn end_splitter(&mut self, cx:&mut Cx){
        cx.end_turtle();
        // draw the splitter in the middle of the turtle
        let rect = cx.turtle_rect();
        self.split.color = self.anim.last_vec4("split.color");
        match self.axis{
            Axis::Horizontal=>{
                self.split_area = self.split.draw_quad_abs(cx, true, rect.x, rect.y + self.calc_pos, rect.w, self.draw_size);
                self.drag_max_pos = rect.h;
            },
            Axis::Vertical=>{
                self.split_area = self.split.draw_quad_abs(cx, true, rect.x+ self.calc_pos, rect.y, self.draw_size, rect.h);
                self.drag_max_pos = rect.w;
            }
       };
       self.anim.set_area(cx, self.split_area);
    }
}
