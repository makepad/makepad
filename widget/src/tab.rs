use makepad_render::*;

#[derive(Clone)]
pub struct Tab {
    pub bg: DrawTab,
    pub text: DrawText,
    pub label: String,
    pub animator: Animator,
    pub draw_depth: f32,
    pub abs_origin: Option<Vec2>,
    pub _is_selected: bool,
    pub _is_focussed: bool,
    pub _close_anim_rect: Rect,
    pub _is_down: bool,
    pub _is_drag: bool
}

#[derive(Clone, DrawQuad)]
#[repr(C)]
pub struct DrawTab {
    #[default_shader(self::shader_bg)]
    base: DrawColor,
    border_color: Vec4,
}


#[derive(Clone, PartialEq)]
pub enum TabEvent {
    None,
    DragMove(FingerMoveEvent),
    DragEnd(FingerUpEvent),
    Closing,
    Close,
    Select,
}

impl Tab {
    pub fn new(cx: &mut Cx) -> Self {
        let mut tab = Self {
            label: "Tab".to_string(),
            //is_closeable: true,
            draw_depth: 0.,
            bg: DrawTab ::new(cx, default_shader!()),
            //tab_close: TabClose::new(cx),
            text: DrawText::new(cx, default_shader!())
                .with_draw_depth(0.1),
            animator: Animator::default(),
            abs_origin: None,
            _is_selected: false,
            _is_focussed: false,
            _is_down: false,
            _is_drag: false,
            _close_anim_rect: Rect::default(),
        };
        tab.animator.set_anim_as_last_values(&tab.anim_default(cx));
        tab
    }
    
    pub fn with_draw_depth(self, draw_depth: f32) -> Self {
        Self {draw_depth, ..self}
    }
    
    pub fn style(cx: &mut Cx) {
        self::DrawTab::register_draw_input(cx);
        live_body!(cx, r#"
            
            self::color_bg_selected: #28;
            self::color_bg_normal: #34;
            
            self::color_text_selected_focus: #FFFFFF;
            self::color_text_deselected_focus: #9d;
            self::color_text_selected_defocus: #9d;
            self::color_text_deselected_defocus: #82;
            
            self::layout_bg: Layout {
                align: {fx: 0.0, fy: 0.5},
                walk: {width: Compute, height: Fix(40.)},
                padding: {l: 16.0, t: 1.0, r: 16.0, b: 0.0},
            }
            
            self::text_style_title: TextStyle {
                ..crate::widgetstyle::text_style_normal
            }
            
            self::shader_bg: Shader {
                use makepad_render::drawquad::shader::*;
                 
                draw_input: self::DrawTab;
                
                const border_width: float = 1.0;
                
                fn pixel() -> vec4 {
                    let cx = Df::viewport(pos * rect_size);
                    cx.rect(-1., -1., rect_size.x + 2., rect_size.y + 2.);
                    cx.fill(color);
                    cx.move_to(rect_size.x, 0.);
                    cx.line_to(rect_size.x, rect_size.y);
                    cx.move_to(0., 0.);
                    cx.line_to(0., rect_size.y);
                    return cx.stroke(border_color, 1.);
                }
            }
            
        "#)
    }
    
    pub fn get_bg_color(&self, cx: &Cx) -> Vec4 {
        if self._is_selected {
            live_vec4!(cx, self::color_bg_selected)
        }
        else {
            live_vec4!(cx, self::color_bg_normal)
        }
    }
    
    pub fn get_text_color(&self, cx: &Cx) -> Vec4 {
        if self._is_selected {
            if self._is_focussed {
                live_vec4!(cx, self::color_text_selected_focus)
            }
            else {
                live_vec4!(cx, self::color_text_selected_defocus)
            }
        }
        else {
            if self._is_focussed {
                live_vec4!(cx, self::color_text_deselected_focus)
            }
            else {
                live_vec4!(cx, self::color_text_deselected_defocus)
            }
        }
    }
    
    pub fn anim_default(&self, cx: &Cx) -> Anim {
        Anim {
            play: Play::Cut {duration: 0.05},
            tracks: vec![
                Track::Vec4 {
                    ease: Ease::Lin,
                    keys: vec![(1.0, self.get_bg_color(cx))],
                    bind_to: live_item_id!(makepad_render::drawcolor::DrawColor::color),
                    cut_init: None
                },
                Track::Vec4 {
                    ease: Ease::Lin,
                    keys: vec![(1.0, live_vec4!(cx, self::color_bg_selected))],
                    bind_to: live_item_id!(self::DrawTab::border_color),
                    cut_init: None
                },
                Track::Vec4 {
                    ease: Ease::Lin,
                    keys: vec![(1.0, self.get_text_color(cx))],
                    bind_to: live_item_id!(makepad_render::drawtext::DrawText::color),
                    cut_init: None
                },
            ]
        }
    }
    
    pub fn anim_over(&self, cx: &Cx) -> Anim {
        Anim {
            play: Play::Cut {duration: 0.01},
            ..self.anim_default(cx)
        }
    }
    
    pub fn anim_down(&self, cx: &Cx) -> Anim {
        Anim {
            play: Play::Cut {duration: 0.01},
            ..self.anim_default(cx)
        }
    }
    
    pub fn set_tab_focus(&mut self, cx: &mut Cx, focus: bool) {
        if focus != self._is_focussed {
            self._is_focussed = focus;
            self.animator.play_anim(cx, self.anim_default(cx));
        }
    }
    
    pub fn set_tab_selected(&mut self, cx: &mut Cx, selected: bool) {
        if selected != self._is_selected {
            self._is_selected = selected;
            self.animator.play_anim(cx, self.anim_default(cx));
        }
    }
    
    pub fn set_tab_state(&mut self, cx: &mut Cx, selected: bool, focus: bool) {
        self._is_selected = selected;
        self._is_focussed = focus;
        self.animator.set_anim_as_last_values(&self.anim_default(cx));
    }
    
    pub fn close_tab(&self, cx: &mut Cx) {
        cx.send_trigger(self.bg.area(), Self::trigger_close());
    }
    
    pub fn trigger_close() -> TriggerId {uid!()}
    
    pub fn handle_tab(&mut self, cx: &mut Cx, event: &mut Event) -> TabEvent {
        if let Some(ae) = event.is_animate(cx, &self.animator) {
            self.bg.animate(cx, &mut self.animator, ae.time);
            self.text.animate(cx, &mut self.animator, ae.time);
        }
        
        match event.hits(cx, self.bg.area(), HitOpt::default()) {
            Event::Trigger(_ti) => {
                //TODO figure out why close animations mess everything up
                //f !self.animator.term_anim_playing() {
                return TabEvent::Close;
                //self._close_anim_rect = self._bg_area.get_rect(cx);
                //self.animator.play_anim(cx, self.anim_close(cx));
                //return TabEvent::Closing;
                //}
            },
            Event::FingerDown(_fe) => {
                cx.set_down_mouse_cursor(MouseCursor::Hand);
                self._is_down = true;
                self._is_drag = false;
                self._is_selected = true;
                self._is_focussed = true;
                self.animator.play_anim(cx, self.anim_down(cx));
                return TabEvent::Select;
            },
            Event::FingerHover(fe) => {
                cx.set_hover_mouse_cursor(MouseCursor::Hand);
                match fe.hover_state {
                    HoverState::In => {
                        if self._is_down {
                            self.animator.play_anim(cx, self.anim_down(cx));
                        }
                        else {
                            self.animator.play_anim(cx, self.anim_over(cx));
                        }
                    },
                    HoverState::Out => {
                        self.animator.play_anim(cx, self.anim_default(cx));
                    },
                    _ => ()
                }
            },
            Event::FingerUp(fe) => {
                self._is_down = false;
                
                if fe.is_over {
                    if fe.input_type.has_hovers() {
                        self.animator.play_anim(cx, self.anim_over(cx));
                    }
                    else {
                        self.animator.play_anim(cx, self.anim_default(cx));
                    }
                }
                else {
                    self.animator.play_anim(cx, self.anim_default(cx));
                }
                if self._is_drag {
                    self._is_drag = false;
                    return TabEvent::DragEnd(fe);
                }
            },
            Event::FingerMove(fe) => {
                if !self._is_drag {
                    if fe.move_distance() > 50. {
                        //cx.set_down_mouse_cursor(MouseCursor::Hidden);
                        self._is_drag = true;
                    }
                }
                if self._is_drag {
                    return TabEvent::DragMove(fe);
                }
                //self.animator.play_anim(cx, self.animator.default.clone());
            },
            _ => ()
        };
        TabEvent::None
    }
    
    pub fn get_tab_rect(&mut self, cx: &Cx) -> Rect {
        self.bg.area().get_rect(cx)
    }
    
    pub fn begin_tab(&mut self, cx: &mut Cx) -> Result<(), ()> {
        // pull the bg color from our animation system, uses 'default' value otherwise
        
        if self.animator.need_init(cx) {
            self.animator.init(cx, self.anim_default(cx));
            self.bg.last_animate(&self.animator);
            self.text.last_animate(&self.animator);
        }
        
        self.bg.base.base.draw_depth = self.draw_depth;
        self.text.draw_depth = self.draw_depth;
        
        let layout = if let Some(abs_origin) = self.abs_origin {
            Layout {abs_origin: Some(abs_origin), ..live_layout!(cx, self::layout_bg)}
        }
        else {
            live_layout!(cx, self::layout_bg)
        };
        self.bg.begin_quad(cx, layout);

        self.text.text_style = live_text_style!(cx, self::text_style_title);
        self.text.draw_text_walk(cx, &self.label);
        
        cx.turtle_align_y();
        
        return Ok(())
    }
    
    pub fn end_tab(&mut self, cx: &mut Cx) {
        self.bg.end_quad(cx);
    }
    
    pub fn draw_tab(&mut self, cx: &mut Cx) {
        if self.begin_tab(cx).is_err() {return};
        self.end_tab(cx);
    }
    
}
