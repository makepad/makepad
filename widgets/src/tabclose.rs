use render::*;

#[derive(Clone, Element)]
pub struct TabClose{
    pub bg: Quad,
    pub bg_layout:Layout,
    pub text: Text,

    pub animator:Animator,
    pub anim_over:Anim,
    pub anim_down:Anim,
    pub margin:Margin,
    pub _hit_state:HitState,
    pub _is_down:bool,
    pub _bg_area:Area,
}

impl Style for TabClose{
    fn style(cx:&mut Cx)->Self{
        let bg_sh = Self::def_bg_shader(cx);
        Self{
            bg:Quad{
                shader_id:cx.add_shader(bg_sh, "TabClose.bg"),

                ..Style::style(cx)
            }, 
            bg_layout:Layout{
                align:Align::center(),
                width:Bounds::Compute,
                height:Bounds::Compute,
                margin:Margin::all(1.0),
                padding:Padding{l:16.0,t:14.0,r:16.0,b:14.0},
                ..Default::default()
            },
            text:Text{..Style::style(cx)},
            animator:Animator::new(Anim::new(AnimMode::Cut{duration:0.2}, vec![
                AnimTrack::to_vec4("bg.color", color("#a")),
                AnimTrack::to_float("bg.hover", 0.),
                AnimTrack::to_float("bg.down", 0.),
            ])),
            anim_over:Anim::new(AnimMode::Cut{duration:0.5}, vec![
                 AnimTrack::vec4("bg.color", Ease::Linear, vec![
                    (0.0, color("#f")),(1.0, color("#f"))
                ]),
                AnimTrack::to_float("bg.down", 0.),
                AnimTrack::float("bg.hover", Ease::Linear, vec![
                    (0.0, 1.),(1.0, 1.)
                ]),
            ]),
            anim_down:Anim::new(AnimMode::Cut{duration:0.2}, vec![
                AnimTrack::vec4("bg.color", Ease::Linear, vec![
                    (0.0, color("#f55")),(1.0, color("#f55"))
                ]),
                AnimTrack::to_float("bg.hover", 1.),
                AnimTrack::float("bg.down", Ease::OutExpo, vec![
                    (0.0, 0.),(1.0, 3.1415*0.5)
                ]),
            ]),
            margin:Margin::zero(),
            _hit_state:HitState{..Default::default()},
            _is_down:false,
            _bg_area:Area::Empty,
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum TabCloseEvent{
    None,
    Clicked,
    Down
}

impl TabClose{
    pub fn def_bg_shader(cx:&mut Cx)->Shader{
        let mut sh = Quad::def_quad_shader(cx);
        sh.add_ast(shader_ast!({
            let hover:float<Instance>;
            let down:float<Instance>;
            fn pixel()->vec4{
                df_viewport(pos * vec2(w, h));
                let hover_max:float = (hover*0.5+0.5)*0.8;
                let hover_min:float = 1. - hover_max;
                let c:vec2 = vec2(w,h) * 0.5;
                df_rotate(down, c.x,c.y);
                df_move_to(c.x*hover_min, c.y*hover_min);
                df_line_to(c.x+c.x*hover_max, c.y+c.y*hover_max);
                df_move_to(c.x+c.x*hover_max, c.y*hover_min);
                df_line_to(c.x*hover_min, c.y+c.y*hover_max);
                //df_circle(0.5*w,0.5*h,0.5*w);
                df_stroke_keep(color,1.+down*0.2);
                //df_circle(0.5*w,0.5*h,0.5*w*(1.-hover));
                return df_fill(color);
            }
        }));
        sh
    }

    pub fn handle_tab_close(&mut self, cx:&mut Cx, event:&mut Event)->TabCloseEvent{
        
        //let mut ret_event = ButtonEvent::None;
        match event.hits(cx, self._bg_area, &mut self._hit_state){
            Event::Animate(ae)=>{
                self.animator.calc_area(cx, "bg.color", ae.time, self._bg_area);
                self.animator.calc_area(cx, "bg.hover", ae.time, self._bg_area);
                self.animator.calc_area(cx, "bg.down", ae.time, self._bg_area);
            },
            Event::FingerDown(_fe)=>{
                self._is_down = true;
                self.animator.play_anim(cx, self.anim_down.clone());
                return TabCloseEvent::Down;
            },
            Event::FingerHover(fe)=>{
                cx.set_hover_mouse_cursor(MouseCursor::Hand);
                match fe.hover_state{
                    HoverState::In=>{
                        if self._is_down{
                            self.animator.play_anim(cx, self.anim_down.clone());
                        }
                        else{
                            self.animator.play_anim(cx, self.anim_over.clone());
                        }
                    },
                    HoverState::Out=>{
                        self.animator.play_anim(cx, self.animator.default.clone());
                    },
                    _=>()
                }
            },
            Event::FingerUp(fe)=>{
                self._is_down = false;
                if fe.is_over{
                    if !fe.is_touch{
                        self.animator.play_anim(cx, self.anim_over.clone());
                    }
                    else{
                        self.animator.play_anim(cx, self.animator.default.clone());
                    }
                    return TabCloseEvent::Clicked;
                }
                else{
                    self.animator.play_anim(cx, self.animator.default.clone());
                }
            },
            _=>()
        };
        TabCloseEvent::None
    }

    pub fn draw_tab_close(&mut self, cx:&mut Cx){

        // pull the bg color from our animation system, uses 'default' value otherwise
        self.bg.color = self.animator.last_vec4("bg.color");

        self._bg_area = self.bg.draw_quad_walk(cx, Bounds::Fix(10.), Bounds::Fix(10.), self.margin);
        
        // push the 2 vars we added to bg shader
        self.animator.last_push(cx, "bg.hover", self._bg_area);
         self.animator.last_push(cx, "bg.down", self._bg_area);
        //self.animator.last_push(cx, "bg.glow_size", self._bg_area);

        self.animator.set_area(cx, self._bg_area); // if our area changed, update animation
    }
}
