use shui::*;

#[derive(Clone, Element)]
pub struct ScrollBar{
    pub hit_state:HitState,
    pub bar_size:f32,
    pub sb_area:Area,
    pub sb_layout:Layout,
    pub sb: Quad,

    pub orientation:ScrollBarOrientation,
    pub anim:Animation<ScrollBarState>,
    pub event:ScrollBarEvent,
    // state
    pub visible:bool,
    pub view_total:f32, // the total view area
    pub view_visible:f32, // the visible view area
    pub scroll_size:f32, // the size of the scrollbar
    pub scroll_pos:f32, // the position of the scrollbar
}

#[derive(Clone, PartialEq)]
pub enum ScrollBarState{
    Default,
    Over,
    Scrolling
}

impl Style for ScrollBar{
    fn style(cx:&mut Cx)->Self{
        let sh = Self::def_shader(cx);
        Self{
            bar_size:8.0,
            visible:false,
            view_total:0.0,
            view_visible:0.0,
            scroll_size:0.0,
            scroll_pos:0.0,
            orientation:ScrollBarOrientation::Horizontal,
            hit_state:HitState::new(),
            sb_area:Area::Empty,
            sb_layout:Layout{
                align:Align::center(),
                w:Computed,
                h:Computed,
                margin:Margin::i32(1),
                ..Layout::paddedf(16.0,14.0,16.0,14.0)
            },
            anim:Animation::new(
                ScrollBarState::Default,
                vec![
                    AnimState::new(
                        ScrollBarState::Default,
                        AnimMode::Cut{duration:0.5}, 
                        vec![
                            AnimTrack::to_vec4("sb.color",color("#5"))
                        ]
                    ),
                    AnimState::new(
                        ScrollBarState::Over,
                        AnimMode::Cut{duration:0.05}, 
                        vec![
                            AnimTrack::to_vec4("sb.color",color("#7"))
                        ]
                    ),
                    AnimState::new(
                        ScrollBarState::Scrolling,
                        AnimMode::Cut{duration:0.2}, 
                        vec![
                            AnimTrack::to_vec4("sb.color",color("#9"))
                        ]
                    ) 
                ]
            ),
            sb:Quad{
                shader_id:cx.add_shader(sh),
                ..Style::style(cx)
            },
            event:ScrollBarEvent::None
        }
    }
}


impl ScrollBar{
    pub fn def_shader(cx:&mut Cx)->Shader{
        let mut sh = Quad::def_shader(cx);
        sh.add_ast(shader_ast!({

            let is_vertical:float<Instance>;
            let view_total:float<Instance>;
            let view_visible:float<Instance>;
            let scroll_pos:float<Instance>;

            const border_radius:float = 2.0;

            fn pixel()->vec4{
                df_viewport(pos * vec2(w, h));
                if is_vertical > 0.5{
                    df_box(0., scroll_pos, w, h*(view_visible/view_total), border_radius);
                }
                else{
                    df_box(scroll_pos, 0., w*(view_visible/view_total), h, border_radius);
                }
                return df_fill_keep(color);
            }
        }));
        sh
    }

    fn make_event(&mut self){
        match self.orientation{
            ScrollBarOrientation::Horizontal=>{
                self.event = ScrollBarEvent::ScrollHorizontal{
                        scroll_pos:self.scroll_pos,
                        view_total:self.view_total,
                        view_visible:self.view_visible
                };
            },
            ScrollBarOrientation::Vertical=>{
                self.event = ScrollBarEvent::ScrollVertical{
                        scroll_pos:self.scroll_pos,
                        view_total:self.view_total,
                        view_visible:self.view_visible
                };
            }
        }        
    }
}


impl ScrollBarLike<ScrollBar> for ScrollBar{
    fn new(cx: &mut Cx,orientation:ScrollBarOrientation)->ScrollBar{
        return ScrollBar{
            orientation:orientation,
            ..Style::style(cx)
        }
    }

    fn handle(&mut self, cx:&mut Cx, event:&mut Event)->ScrollBarEvent{

        self.event = ScrollBarEvent::None;
        if !self.visible{
            return self.event.clone();
        }

        match event.hits(cx, self.sb_area, &mut self.hit_state){
            Event::Animate(ae)=>{
                self.anim.calc_area(cx, "sb.color", ae.time, self.sb_area);
                //self.anim.calc_area(cx, "bg.handle_color", ae.time, self.sb_area);
            },
            Event::FingerDown(fe)=>{
                self.anim.change_state(cx, ScrollBarState::Scrolling);
                self.make_event();
            },
            Event::FingerHover(fe)=>{
                match fe.hover_state{
                    HoverState::In=>{
                        self.anim.change_state(cx, ScrollBarState::Over);
                    },
                    HoverState::Out=>{
                        self.anim.change_state(cx, ScrollBarState::Default);
                    },
                    _=>()
                }
            },
            Event::FingerUp(fe)=>{
                self.event = ScrollBarEvent::ScrollDone;
                cx.captured_fingers[fe.digit] = Area::Empty;
                if fe.is_over{
                    if !fe.is_touch{
                        self.anim.change_state(cx, ScrollBarState::Over);
                    }
                    else{
                        self.anim.change_state(cx, ScrollBarState::Default);
                    }
                }
                else{
                    self.anim.change_state(cx, ScrollBarState::Default);
                }
            },
            Event::FingerMove(_fe)=>{
                // drag the thing

            },
            _=>()
        };
        self.event.clone()
    }

    fn draw_with_view_size(&mut self, cx:&mut Cx, view_rect:Rect, view_total:Vec2){
        // pull the bg color from our animation system, uses 'default' value otherwise
        self.sb.color = self.anim.last_vec4("sb.color");

        match self.orientation{
             ScrollBarOrientation::Horizontal=>{
                self.visible = view_total.x > view_rect.w;
                if !self.visible{
                    return
                }
                // compute if we need a vertical one, so make space bottom right
                self.scroll_size = if view_total.y > view_rect.h{
                    view_rect.w - self.bar_size
                }
                else{
                    view_rect.w
                };
                self.view_total = view_total.x;
                self.view_visible = view_rect.w;
                self.sb_area = self.sb.draw_abs(
                    cx, true,   
                    view_rect.x, 
                    view_rect.y + view_rect.h - self.bar_size, 
                    self.scroll_size,
                    self.bar_size, 
                );
                self.sb_area.push_float(cx, "is_vertical", 0.0);
             },
             ScrollBarOrientation::Vertical=>{
                // compute if we need a horizontal one
                self.visible = view_total.y > view_rect.h;
                if !self.visible{
                    return
                }
                // check if we need a horizontal slider, ifso make space bottom right
                self.scroll_size = if view_total.x > view_rect.w{
                    view_rect.h - self.bar_size
                }
                else{
                    view_rect.h
                };
                self.view_total = view_total.y;
                self.view_visible = view_rect.h;
            
                self.sb_area = self.sb.draw_abs(
                    cx, true,   
                    view_rect.x + view_rect.w - self.bar_size, 
                    view_rect.y, 
                    self.bar_size,
                    self.scroll_size
                );
                self.sb_area.push_float(cx, "is_vertical", 1.0);
            }
        }
        // push the var added to the sb shader
        self.sb_area.push_float(cx, "view_total", self.view_total);
        self.sb_area.push_float(cx, "view_visible", self.view_visible);
        self.sb_area.push_float(cx, "scroll_pos", self.scroll_pos);
        // the only animating thing
        //self.anim.last_push(cx, "sb.handle_color", self.sb_area);

        self.anim.set_area(cx, self.sb_area); // if our area changed, update animation
    }
}
