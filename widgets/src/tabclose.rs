use render::*;

#[derive(Clone)]
pub struct TabClose{
    pub bg: Quad,
    pub text: Text,

    pub animator:Animator,
    pub anim_over:Anim,
    pub anim_down:Anim,
    pub margin:Margin,
    pub _hit_state:HitState,
    pub _is_down:bool,
    pub _bg_area:Area,
}

impl ElementLife for TabClose{
    fn construct(&mut self, _cx:&mut Cx){}
    fn destruct(&mut self, _cx:&mut Cx){}
}

impl Style for TabClose{
    fn style(cx:&mut Cx)->Self{
        let bg_sh = Self::def_bg_shader(cx);
        Self{
            bg:Quad{
                shader_id:cx.add_shader(bg_sh, "TabClose.bg"),
                ..Style::style(cx)
            }, 
            text:Text{..Style::style(cx)},
            animator:Animator::new(Anim::new(Play::Cut{duration:0.2}, vec![
                Track::vec4("bg.color", Ease::Lin, vec![(1.0, color("#a"))]),
                Track::float("bg.hover", Ease::Lin, vec![(1.0, 0.)]),
                Track::float("bg.down", Ease::Lin, vec![(1.0, 0.)]),
            ])),
            anim_over:Anim::new(Play::Cut{duration:0.2}, vec![
                Track::vec4("bg.color", Ease::Lin, vec![(0.0, color("#f")),(1.0, color("#f"))]),
                Track::float("bg.down", Ease::Lin, vec![(1.0, 0.)]),
                Track::float("bg.hover", Ease::Lin, vec![(0.0, 1.0),(1.0, 1.0)]),
            ]),
            anim_down:Anim::new(Play::Cut{duration:0.2}, vec![
                Track::vec4("bg.color", Ease::Lin, vec![(0.0, color("#f55")),(1.0, color("#f55"))]),
                Track::float("bg.hover", Ease::Lin, vec![(1.0, 1.0)]),
                Track::float("bg.down", Ease::OutExp, vec![(0.0, 0.0),(1.0, 3.1415*0.5)]),
            ]),
            margin:Margin::zero(),
            _hit_state:HitState{
                margin:Some(Margin{
                    l:5.,t:5.,r:5.,b:5.
                }),
                ..Default::default()
            },
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
                let hover_max:float = (hover*0.2+0.8)*0.5;
                let hover_min:float = 1. - hover_max;
                let c:vec2 = vec2(w,h) * 0.5;
                df_rotate(down, c.x,c.y);
                df_move_to(c.x*hover_min, c.y*hover_min);
                df_line_to(c.x+c.x*hover_max, c.y+c.y*hover_max);
                df_move_to(c.x+c.x*hover_max, c.y*hover_min);
                df_line_to(c.x*hover_min, c.y+c.y*hover_max);
                df_stroke_keep(color,1.+down*0.2);
                return df_fill(color);
            }
        }));
        sh
    }

    pub fn handle_tab_close(&mut self, cx:&mut Cx, event:&mut Event)->TabCloseEvent{
        
        //let mut ret_event = ButtonEvent::None;
        match event.hits(cx, self._bg_area, &mut self._hit_state){
            Event::Animate(ae)=>{
                self.animator.calc_write(cx, "bg.color", ae.time, self._bg_area);
                self.animator.calc_write(cx, "bg.hover", ae.time, self._bg_area);
                self.animator.calc_write(cx, "bg.down", ae.time, self._bg_area);
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

        self.bg.color = self.animator.last_vec4("bg.color");
        let bg_inst =  self.bg.draw_quad_walk(cx, Bounds::Fix(10.), Bounds::Fix(10.), self.margin);
        bg_inst.push_float(cx, self.animator.last_float("bg.hover"));
        bg_inst.push_float(cx, self.animator.last_float("bg.down"));
        self._bg_area = bg_inst.get_area();
        self.animator.set_area(cx, self._bg_area); // if our area changed, update animation
    }
}
