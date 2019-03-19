use render::*;

#[derive(Clone, Element)]
pub struct Tab{
    pub bg_layout:Layout,
    pub bg: Quad,
    pub text: Text,

    pub label:String,

    pub animator:Animator,

    pub _is_selected:bool,
    pub _is_focussed:bool,
    pub _hit_state:HitState,
    pub _bg_area:Area,
    pub _text_area:Area,
    pub _is_down:bool,
    pub _is_drag:bool
}

impl Style for Tab{
    fn style(cx:&mut Cx)->Self{
        let bg_sh = Self::def_bg_shader(cx);
        let mut tab = Self{
            label:"Tab".to_string(),
            bg:Quad{
                shader_id:cx.add_shader(bg_sh,"Tab.bg"),
                ..Style::style(cx)
            },
            bg_layout:Layout{
                align:Align::center(),
                width:Bounds::Compute,
                height:Bounds::Compute,
                margin:Margin::all(0.),
                padding:Padding{l:16.0,t:12.0,r:16.0,b:12.0},
                ..Default::default()
            },
            text:Text{..Style::style(cx)},
            animator:Animator::new(Anim::empty()),
            _is_selected:false,
            _is_focussed:false,
            _hit_state:HitState{..Default::default()},
            _is_down:false,
            _is_drag:false,
            _text_area:Area::Empty,
            _bg_area:Area::Empty,
        };
        tab.animator.default = tab.anim_default(cx);
        tab
    }
}

#[derive(Clone, PartialEq)]
pub enum TabEvent{
    None,
    DragMove(FingerMoveEvent),
    DragEnd(FingerUpEvent),
    Select,
}

impl Tab{

    pub fn get_bg_color(&self, cx:&Cx)->Vec4{
        if self._is_selected{
            cx.color("bg_selected")
        } 
        else {
            cx.color("bg_normal")
        }
    }

    pub fn get_text_color(&self, cx:&Cx)->Vec4{
        if self._is_selected{
            if self._is_focussed{
                cx.color("text_selected_focus")
            }
            else{
                cx.color("text_selected_defocus")
            } 
        }
        else{
            if self._is_focussed{
                cx.color("text_deselected_focus")
            }
            else{
                cx.color("text_deselected_defocus")
            }
        }
    }

    pub fn anim_default(&self, cx:&Cx)->Anim{
        Anim::new(AnimMode::Cut{duration:0.5}, vec![
            AnimTrack::to_vec4("bg.color", self.get_bg_color(cx)),
            AnimTrack::to_vec4("bg.border_color", cx.color("bg_selected")),
            AnimTrack::to_vec4("text.color", self.get_text_color(cx)),
            AnimTrack::to_vec4("icon.color", self.get_text_color(cx))
        ])
    }

    pub fn anim_over(&self, cx:&Cx)->Anim{
        Anim::new(AnimMode::Cut{duration:0.5}, vec![
            AnimTrack::to_vec4("bg.color", self.get_bg_color(cx)),
            AnimTrack::to_vec4("bg.border_color", cx.color("bg_selected")),
            AnimTrack::to_vec4("text.color", self.get_text_color(cx)),
            AnimTrack::to_vec4("icon.color", self.get_text_color(cx))
        ])
    }

    pub fn anim_down(&self, cx:&Cx)->Anim{
        Anim::new(AnimMode::Cut{duration:0.05}, vec![
            AnimTrack::to_vec4("bg.color", self.get_bg_color(cx)),
            AnimTrack::to_vec4("bg.border_color", cx.color("over_border")),
            AnimTrack::to_float("bg.glow_size", 2.0),
            AnimTrack::to_vec4("text.color", self.get_text_color(cx)),
            AnimTrack::to_vec4("icon.color", self.get_text_color(cx))            
        ])
    }
/*
    pub fn anim_down(&self, cx:&Cx)->Anim{
        Anim::new(AnimMode::Cut{duration:0.2}, vec![
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
        ])
    }
   */
    pub fn def_bg_shader(cx:&mut Cx)->Shader{
        let mut sh = Quad::def_quad_shader(cx);
        sh.add_ast(shader_ast!({

            let border_color:vec4<Instance>;
            const border_width:float = 1.0;

            fn pixel()->vec4{
                df_viewport(pos * vec2(w, h));
                df_rect(-1.,-1.,w+2.,h+2.);
                df_fill(color);
                df_move_to(w,0.);
                df_line_to(w,h);
                df_move_to(0.,0.);
                df_line_to(0.,h);
                return df_stroke(border_color, 1.);
            }
        }));
        sh
    }

    pub fn set_tab_focus(&mut self, cx:&mut Cx, focus:bool){
        if focus != self._is_focussed{
            self._is_focussed = focus;
            self.animator.play_anim(cx, self.anim_default(cx));
        }
    }

    pub fn set_tab_selected(&mut self, cx:&mut Cx, selected:bool){
        if selected != self._is_selected{
            self._is_selected = selected;
            self.animator.play_anim(cx, self.anim_default(cx));
        }
    }

    pub fn handle_tab(&mut self, cx:&mut Cx, event:&mut Event)->TabEvent{
        match event.hits(cx, self._bg_area, &mut self._hit_state){
            Event::Animate(ae)=>{
                self.animator.calc_area(cx, "bg.color", ae.time, self._bg_area);
                self.animator.calc_area(cx, "bg.border_color", ae.time, self._bg_area);
                self.animator.calc_area(cx, "text.color", ae.time, self._text_area);
                //self.animator.calc_area(cx, "bg.glow_size", ae.time, self._bg_area);
            },
            Event::FingerDown(_fe)=>{
                cx.set_down_mouse_cursor(MouseCursor::Hand);
                self._is_down = true;
                self._is_drag = false;
                self._is_selected = true;
                self._is_focussed = true;
                self.animator.play_anim(cx, self.anim_down(cx));
                return TabEvent::Select;
            },
            Event::FingerHover(fe)=>{
                cx.set_hover_mouse_cursor(MouseCursor::Hand);
                match fe.hover_state{
                    HoverState::In=>{
                        if self._is_down{
                            self.animator.play_anim(cx, self.anim_down(cx));
                        }
                        else{
                            self.animator.play_anim(cx, self.anim_over(cx));
                        }
                    },
                    HoverState::Out=>{
                        self.animator.play_anim(cx, self.anim_default(cx));
                    },
                    _=>()
                }
            },
            Event::FingerUp(fe)=>{
                self._is_down = false;

                if fe.is_over{
                    if !fe.is_touch{
                        self.animator.play_anim(cx, self.anim_over(cx));
                    }
                    else{
                        self.animator.play_anim(cx, self.anim_default(cx));
                    }
                }
                else{
                    self.animator.play_anim(cx, self.anim_default(cx));
                }
                if self._is_drag{
                    self._is_drag = false;
                    return TabEvent::DragEnd(fe);
                }
            },
            Event::FingerMove(fe)=>{
                if !self._is_drag{
                    if (fe.abs_start.x - fe.abs.x).abs() + (fe.abs_start.y - fe.abs.y).abs() > 10.{
                        self._is_drag = true;
                    }
                }
                if self._is_drag{
                    return TabEvent::DragMove(fe);
                }
                //self.animator.play_anim(cx, self.animator.default.clone());
            },
            _=>()
        };
        TabEvent::None
   }

    pub fn get_tab_rect(&mut self, cx:&Cx)->Rect{
        self._bg_area.get_rect(cx, false)
    }

    pub fn draw_tab(&mut self, cx:&mut Cx){
        // pull the bg color from our animation system, uses 'default' value otherwise
        self.bg.color = self.animator.last_vec4("bg.color");
        self._bg_area = self.bg.begin_quad(cx, &self.bg_layout);
        // push the 2 vars we added to bg shader
        self.text.color = self.animator.last_vec4("text.color");
        self._text_area = self.text.draw_text(cx, &self.label);
        self.bg.end_quad(cx);
        self.animator.last_push(cx, "bg.border_color", self._bg_area);
        //self.animator.last_push(cx, "bg.glow_size", self._bg_area);

        self.animator.set_area(cx, self._bg_area); // if our area changed, update animation
    }

}