use shui::*;

#[derive(Clone, Element)]
pub struct Button{
    pub hit_state:HitState,

    pub bg_area:Area,
    pub layout:Layout,
    pub bg: Quad,

    pub text: Text,

    pub anim:Animation<ButtonState>,
    pub is_down:bool,
    pub label:String
}

#[derive(Clone, PartialEq)]
pub enum ButtonState{
    Default,
    Over,
    Down
}

impl Style for Button{
    fn style(cx:&mut Cx)->Self{
        let bg_sh = Self::def_bg_shader(cx);
        Self{
            hit_state:HitState{
                ..Default::default()
            },
            is_down:false,
            bg_area:Area::Empty,
            layout:Layout{
                align:Align::center(),
                width:Bounds::Compute,
                height:Bounds::Compute,
                margin:Margin::all(1.0),
                padding:Padding{l:16.0,t:14.0,r:16.0,b:14.0},
                ..Default::default()
            },
            label:"OK".to_string(),
            anim:Animation::new(
                ButtonState::Default,
                vec![
                    AnimState::new(
                        ButtonState::Default,
                        AnimMode::Cut{duration:0.5}, 
                        vec![
                            AnimTrack::to_vec4("bg.color",cx.style.bg_normal),
                            AnimTrack::to_float("bg.glow_size",0.0),
                            AnimTrack::to_vec4("bg.border_color",cx.style.text_lo),
                            AnimTrack::to_vec4("text.color",cx.style.text_med),
                            AnimTrack::to_vec4("icon.color",cx.style.text_med),
                            AnimTrack::to_float("width", 80.0)
                        ]
                    ),
                    AnimState::new(
                        ButtonState::Over,
                        AnimMode::Cut{duration:0.05}, 
                        vec![
                            AnimTrack::to_vec4("bg.color", cx.style.bg_top),
                            AnimTrack::to_vec4("bg.border_color", color("white")),
                            AnimTrack::to_float("bg.glow_size", 1.0),
                            AnimTrack::float("width", Ease::Linear, vec![(1.0,140.0)])
                        ]
                    ),
                    AnimState::new(
                        ButtonState::Down,
                        AnimMode::Cut{duration:0.2}, 
                        vec![
                            AnimTrack::vec4("bg.border_color", Ease::Linear, vec![
                                (0.0, color("white")),
                                (1.0, color("white"))
                            ]),
                            AnimTrack::vec4("bg.color", Ease::Linear, vec![
                                (0.0, color("#f")),
                                (1.0, color("#6"))
                            ]),
                            AnimTrack::float("bg.glow_size", Ease::Linear, vec![
                                (0.0, 1.0),
                                (1.0, 1.0)
                            ]),
                            AnimTrack::vec4("icon.color", Ease::Linear, vec![
                                (0.0, color("#0")),
                                (1.0, color("#f")),
                            ]),
                        ]
                    ) 
                ]
            ),
            bg:Quad{
                shader_id:cx.add_shader(bg_sh),
                ..Style::style(cx)
            },
            text:Text{..Style::style(cx)}
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
        match event.hits(cx, self.bg_area, &mut self.hit_state){
            Event::Animate(ae)=>{

                self.anim.calc_area(cx, "bg.color", ae.time, self.bg_area);
                self.anim.calc_area(cx, "bg.border_color", ae.time, self.bg_area);
                
                self.anim.calc_area(cx, "bg.glow_size", ae.time, self.bg_area);

                self.anim.calc_float(cx, "width", ae.time);

                cx.dirty_area = Area::All;
            },
            Event::FingerDown(_fe)=>{
                ret_event = ButtonEvent::Down;
                self.is_down = true;
                self.anim.change_state(cx, ButtonState::Down);
                cx.set_down_mouse_cursor(MouseCursor::Crosshair);
            },
            Event::FingerHover(fe)=>{
                cx.set_hover_mouse_cursor(MouseCursor::Hand);

                match fe.hover_state{
                    HoverState::In=>{
                        if self.is_down{
                            self.anim.change_state(cx, ButtonState::Down);
                        }
                        else{
                            self.anim.change_state(cx, ButtonState::Over);
                        }
                    },
                    HoverState::Out=>{
                        self.anim.change_state(cx, ButtonState::Default);
                    },
                    _=>()
                }
            },
            Event::FingerUp(fe)=>{
                self.is_down = false;
                if fe.is_over{
                    if !fe.is_touch{
                        self.anim.change_state(cx, ButtonState::Over);
                    }
                    else{
                        self.anim.change_state(cx, ButtonState::Default);
                    }
                    ret_event = ButtonEvent::Clicked;
                }
                else{
                    self.anim.change_state(cx, ButtonState::Default);
                }
            },
            _=>()
        };
        ret_event
   }

    pub fn draw_button_with_label(&mut self, cx:&mut Cx, label: &str){

        // pull the bg color from our animation system, uses 'default' value otherwise
        self.bg.color = self.anim.last_vec4("bg.color");
        self.bg_area = self.bg.begin_quad(cx, &Layout{
            width: Bounds::Fix(self.anim.last_float("width")),
            ..self.layout.clone()
        });
        // push the 2 vars we added to bg shader
        self.anim.last_push(cx, "bg.border_color", self.bg_area);
        self.anim.last_push(cx, "bg.glow_size", self.bg_area);

        self.text.draw_text(cx, label);
        
        self.bg.end_quad(cx);

        self.anim.set_area(cx, self.bg_area); // if our area changed, update animation
    }
}
