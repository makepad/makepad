use shui::*;

#[derive(Clone, Element)]
pub struct Button{
    pub view:View,
    pub hit_state:HitState,
    pub bg_area:Area,
    pub layout:Layout,
    pub bg: Quad,
    pub bg_layout:Layout,
    pub text: Text,
    pub anim:Animation<ButtonState>,
    pub label:String,
    pub event:ButtonEvent
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
            bg_layout:Layout{
                align:Align::center(),
                w:Computed,
                h:Computed,
                margin:Margin::i32(1),
                ..Layout::paddedf(16.0,14.0,16.0,14.0)
            },
            hit_state:HitState::new(),
            view:View::new(),
            bg_area:Area::Empty,
            layout:Layout{
                w:Computed,
                h:Computed,
                ..Layout::new()
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
                        AnimMode::Cut{duration:0.5}, 
                        vec![
                            AnimTrack::to_vec4("bg.color", cx.style.bg_top),
                            AnimTrack::to_vec4("bg.border_color", color("white")),
                            AnimTrack::to_float("bg.glow_size", 1.0),
                            AnimTrack::float("width", Ease::OutBounce, vec![(1.0,140.0)])
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
            text:Text{..Style::style(cx)},
            event:ButtonEvent::None
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum ButtonEvent{
    None,
    Clicked
}

impl Button{
    pub fn def_bg_shader(cx:&mut Cx)->Shader{
        let mut sh = Quad::def_shader(cx);
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

    pub fn handle(&mut self, cx:&mut Cx, event:&Event)->ButtonEvent{
        match event.hits(cx, self.bg_area, &mut self.hit_state){
            Event::Animate(ae)=>{
                //let value = self.anim.calc_vec4(cx, "bg.color", ae.time, vec4(0.0,0.0,0.0,0.0));
                //println!("{:?} {}", value, ae.time);
                self.anim.calc_area(cx, "bg.color", ae.time, self.bg_area);
                self.anim.calc_area(cx, "bg.border_color", ae.time, self.bg_area);
                self.anim.calc_area(cx, "bg.glow_size", ae.time, self.bg_area);

                //let width = self.anim.last_float("width");
                //println!("{}", width);
                self.anim.calc_float(cx, "width", ae.time);

                cx.dirty_area = Area::All;
            },
            Event::FingerDown(_fe)=>{
                self.event = ButtonEvent::Clicked;
                self.anim.change_state(cx, ButtonState::Down);
            },
            Event::FingerHover(fe)=>{
                match fe.hover_state{
                    HoverState::In=>{
                        self.anim.change_state(cx, ButtonState::Over);
                    },
                    HoverState::Out=>{
                        self.anim.change_state(cx, ButtonState::Default);
                    },
                    _=>()
                }
            },
            Event::FingerMove(_fe)=>{
            },
            _=>{
                 self.event = ButtonEvent::None
            }
        };
        self.event.clone()
   }

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
}
