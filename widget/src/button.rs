use render::*;

#[derive(Clone)]
pub struct Button{
    pub bg: Quad,
    pub bg_layout:Layout,
    pub text: Text,

    pub animator:Animator,
    pub anim_over:Anim,
    pub anim_down:Anim,

    pub _hit_state:HitState,
    pub _is_down:bool,
    pub _bg_area:Area,
}

impl Style for Button{
    fn style(cx:&mut Cx)->Self{
        let bg_sh = Self::def_bg_shader(cx);
        Self{
            bg:Quad{
                shader:cx.add_shader(bg_sh, "Button.bg"),
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

            animator:Animator::new(Anim::new(Play::Cut{duration:0.5}, vec![
                Track::color("bg.color", Ease::Lin, vec![(1., cx.color("bg_normal"))]),
                Track::float("bg.glow_size", Ease::Lin, vec![(1., 0.0)]),
                Track::color("bg.border_color", Ease::Lin, vec![(1., color("#6"))]),
                Track::color("text.color", Ease::Lin, vec![(1., color("white"))]),
                Track::color("icon.color", Ease::Lin, vec![(1., color("white"))])
            ])),
            anim_over:Anim::new(Play::Cut{duration:0.05}, vec![
                Track::color("bg.color", Ease::Lin, vec![(1., color("#999"))]),
                Track::color("bg.border_color", Ease::Lin, vec![(1., color("white"))]),
                Track::float("bg.glow_size", Ease::Lin, vec![(1., 1.0)])
            ]),
            anim_down:Anim::new(Play::Cut{duration:0.2}, vec![
                Track::color("bg.border_color", Ease::Lin, vec![
                    (0.0, color("white")),(1.0, color("white"))
                ]),
                Track::color("bg.color", Ease::Lin, vec![
                    (0.0, color("#f")),(1.0, color("#6"))
                ]),
                Track::float("bg.glow_size", Ease::Lin, vec![
                    (0.0, 1.0),(1.0, 1.0)
                ]),
                Track::color("icon.color", Ease::Lin, vec![
                    (0.0, color("#0")),(1.0, color("#f")),
                ]),
            ]),

            _hit_state:HitState{..Default::default()},
            _is_down:false,
            _bg_area:Area::Empty,
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum ButtonEvent{
    None,
    Clicked,
    Down,
    Up
}

impl Button{
    pub fn def_bg_shader(cx:&mut Cx)->CxShader{
        let mut sh = Quad::def_quad_shader(cx);
        sh.add_ast(shader_ast!({

            let border_color:vec4<Instance>;
            let glow_size:float<Instance>;

            const glow_color:vec4 = color("#30f");
            const border_radius:float = 6.5;
            const border_width:float = 1.0;

            fn pixel()->vec4{
                df_viewport(pos * vec2(w, h));
                df_box(0., 0., w, h, border_radius);
                df_shape += 3.;
                df_fill_keep(color);
                df_stroke_keep(border_color, border_width);
                df_blur = 2.;
                return df_glow(glow_color, glow_size);
            }
        }));
        sh
    }

    pub fn handle_button(&mut self, cx:&mut Cx, event:&mut Event)->ButtonEvent{
        
        //let mut ret_event = ButtonEvent::None;
        match self._hit_state.hits(cx, self._bg_area, event){
            Event::Animate(ae)=>{
                self.animator.calc_write(cx, "bg.color", ae.time, self._bg_area);
                self.animator.calc_write(cx, "bg.border_color", ae.time, self._bg_area);
                self.animator.calc_write(cx, "bg.glow_size", ae.time, self._bg_area);
            },
            Event::FingerDown(_fe)=>{
                self._is_down = true;
                self.animator.play_anim(cx, self.anim_down.clone());
                return ButtonEvent::Down;
            },
            Event::FingerHover(fe)=>{
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
                    return ButtonEvent::Clicked;
                }
                else{
                    self.animator.play_anim(cx, self.animator.default.clone());
                    return ButtonEvent::Up;
                }
            },
            _=>()
        };
        ButtonEvent::None
   }

    pub fn draw_button_with_label(&mut self, cx:&mut Cx, label: &str){

        // pull the bg color from our animation system, uses 'default' value otherwise
        self.bg.color = self.animator.last_color("bg.color");
        let bg_inst =  self.bg.begin_quad(cx, &self.bg_layout);
        // push the 2 vars we added to bg shader
        bg_inst.push_color(cx, self.animator.last_color("bg.border_color"));
        bg_inst.push_float(cx, self.animator.last_float("bg.glow_size"));

        self.text.draw_text(cx, label);
        
        self._bg_area = self.bg.end_quad(cx, &bg_inst);

        self.animator.update_area_refs(cx, self._bg_area); // if our area changed, update animation
    }
}
