use render::*;

#[derive(Clone, Element)]
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
                shader_id:cx.add_shader(bg_sh, "Button.bg"),
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

            animator:Animator::new(Anim::new(AnimMode::Cut{duration:0.5}, vec![
                AnimTrack::to_vec4("bg.color", cx.style_color("bg_normal")),
                AnimTrack::to_float("bg.glow_size", 0.0),
                AnimTrack::to_vec4("bg.border_color", cx.style_color("text_lo")),
                AnimTrack::to_vec4("text.color", cx.style_color("text_med")),
                AnimTrack::to_vec4("icon.color", cx.style_color("text_med"))
            ])),
            anim_over:Anim::new(AnimMode::Cut{duration:0.05}, vec![
                AnimTrack::to_vec4("bg.color", cx.style_color("bg_top")),
                AnimTrack::to_vec4("bg.border_color", color("white")),
                AnimTrack::to_float("bg.glow_size", 1.0)
            ]),
            anim_down:Anim::new(AnimMode::Cut{duration:0.2}, vec![
                AnimTrack::vec4("bg.border_color", Ease::Linear, vec![
                    (0.0, color("white")),(1.0, color("white"))
                ]),
                AnimTrack::vec4("bg.color", Ease::Linear, vec![
                    (0.0, color("#f")),(1.0, color("#6"))
                ]),
                AnimTrack::float("bg.glow_size", Ease::Linear, vec![
                    (0.0, 1.0),(1.0, 1.0)
                ]),
                AnimTrack::vec4("icon.color", Ease::Linear, vec![
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
    Down
}

impl Button{
    pub fn def_bg_shader(cx:&mut Cx)->Shader{
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
        let mut ret_event = ButtonEvent::None;
        match event.hits(cx, self._bg_area, &mut self._hit_state){
            Event::Animate(ae)=>{

                self.animator.calc_area(cx, "bg.color", ae.time, self._bg_area);
                self.animator.calc_area(cx, "bg.border_color", ae.time, self._bg_area);
                
                self.animator.calc_area(cx, "bg.glow_size", ae.time, self._bg_area);
            },
            Event::FingerDown(_fe)=>{
                ret_event = ButtonEvent::Down;
                self._is_down = true;
                self.animator.play_anim(cx, self.anim_down.clone());
                cx.set_down_mouse_cursor(MouseCursor::Crosshair);
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
                    ret_event = ButtonEvent::Clicked;
                }
                else{
                    self.animator.play_anim(cx, self.animator.default.clone());
                }
            },
            _=>()
        };
        ret_event
   }

    pub fn draw_button_with_label(&mut self, cx:&mut Cx, label: &str){

        // pull the bg color from our animation system, uses 'default' value otherwise
        self.bg.color = self.animator.last_vec4("bg.color");
        self._bg_area = self.bg.begin_quad(cx, &self.bg_layout);
        // push the 2 vars we added to bg shader
        self.animator.last_push(cx, "bg.border_color", self._bg_area);
        self.animator.last_push(cx, "bg.glow_size", self._bg_area);

        self.text.draw_text(cx, label);
        
        self.bg.end_quad(cx);

        self.animator.set_area(cx, self._bg_area); // if our area changed, update animation
    }
}
