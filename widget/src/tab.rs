use makepad_render::*;
use crate::buttonlogic::*;
use crate::tabclose::*;

#[derive(Clone)]
pub struct Tab {
    pub bg: Quad,
    pub text: Text,
    pub tab_close: TabClose,
    pub label: String,
    pub is_closeable: bool,
    pub animator: Animator,
    pub z: f32,
    pub abs_origin: Option<Vec2>,
    pub _is_selected: bool,
    pub _is_focussed: bool,
    pub _bg_area: Area,
    pub _bg_inst: Option<InstanceArea>,
    pub _text_area: Area,
    pub _close_anim_rect: Rect,
    pub _is_down: bool,
    pub _is_drag: bool
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
            is_closeable: true,
            z: 0.,
            bg: Quad ::new(cx),
            tab_close: TabClose::new(cx),
            text: Text::new(cx),
            animator: Animator::default(),
            abs_origin: None,
            _is_selected: false,
            _is_focussed: false,
            _is_down: false,
            _is_drag: false,
            _close_anim_rect: Rect::default(),
            _text_area: Area::Empty,
            _bg_area: Area::Empty,
            _bg_inst: None,
        };
        tab.animator.set_anim_as_last_values(&tab.anim_default(cx));
        tab
    }

    pub fn style(cx: &mut Cx) {
        
        live!(cx, r#"
            
            self::color_bg_selected: #28;
            self::color_bg_normal: #34;
            
            self::color_text_selected_focus: #f;
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
                use makepad_render::quad::shader::*;
                
                instance border_color: vec4;
                
                const border_width: float = 1.0;
                
                fn pixel() -> vec4 {
                    let cx = Df::viewport(pos * vec2(w, h));
                    cx.rect(-1., -1., w + 2., h + 2.);
                    cx.fill(color);
                    cx.move_to(w, 0.);
                    cx.line_to(w, h);
                    cx.move_to(0., 0.);
                    cx.line_to(0., h);
                    return cx.stroke(border_color, 1.);
                }
            }
            
        "#)
    }
    
    pub fn get_bg_color(&self, cx: &Cx) -> Color {
        if self._is_selected {
            live_color!(cx, self::color_bg_selected)
        }
        else {
            live_color!(cx, self::color_bg_normal)
        }
    }
    
    pub fn get_text_color(&self, cx: &Cx) -> Color {
        if self._is_selected {
            if self._is_focussed {
                live_color!(cx, self::color_text_selected_focus)
            }
            else {
                live_color!(cx, self::color_text_selected_defocus)
            }
        }
        else {
            if self._is_focussed {
                live_color!(cx, self::color_text_deselected_focus)
            }
            else {
                live_color!(cx, self::color_text_deselected_defocus)
            }
        }
    }
    
    pub fn anim_default(&self, cx: &Cx) -> Anim {
        Anim {
            play: Play::Cut {duration: 0.05},
            tracks: vec![
                Track::Color {
                    ease: Ease::Lin,
                    keys: vec![(1.0, self.get_bg_color(cx))],
                    live_id: live_id!(makepad_render::quad::shader::color),
                    cut_init: None
                },
                Track::Color {
                    ease: Ease::Lin,
                    keys: vec![(1.0, live_color!(cx, self::color_bg_selected))],
                    live_id: live_id!(self::shader_bg::border_color),
                    cut_init: None
                },
                Track::Color {
                    ease: Ease::Lin,
                    keys: vec![(1.0, self.get_text_color(cx))],
                    live_id: live_id!(makepad_render::text::shader::color),
                    cut_init: None
                },
            ]
        }
    }
    
    pub fn anim_over(&self, cx: &Cx) -> Anim {
        Anim{
            play: Play::Cut {duration: 0.01},
            ..self.anim_default(cx)
        }
    }
    
    pub fn anim_down(&self, cx: &Cx) -> Anim {
        Anim{
            play: Play::Cut {duration: 0.01},
            ..self.anim_default(cx)
        }
    }
    
    pub fn anim_close(&self, _cx: &Cx) -> Anim {
        Anim{
            play: Play::Single {duration: 0.1, cut: true, term: true, end: 1.0},
            tracks: vec![
                Track::Float{
                    live_id: live_id!(self::tab_closing),
                    ease: Ease::OutExp,
                    keys: vec![(0.0, 1.0), (1.0, 0.0)],
                    cut_init: None
                }
            ]
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
    
    pub fn handle_tab(&mut self, cx: &mut Cx, event: &mut Event) -> TabEvent {
        
        if !self.animator.term_anim_playing() {
            match self.tab_close.handle_tab_close(cx, event) {
                ButtonEvent::Down => {
                    self._close_anim_rect = self._bg_area.get_rect(cx);
                    self.animator.play_anim(cx, self.anim_close(cx));
                    return TabEvent::Closing;
                },
                _ => ()
            }
        }
        
        match event.hits(cx, self._bg_area, HitOpt::default()) {
            Event::Animate(ae) => {
                // its playing the term anim, run a redraw
                if self.animator.term_anim_playing() {
                    self.animator.calc_float(cx, live_id!(self::tab_closing), ae.time);
                    cx.redraw_child_area(self._bg_area);
                }
                else {
                    self.animator.calc_area(cx, self._bg_area, ae.time);
                    self.animator.calc_area(cx, self._text_area, ae.time);
                }
            },
            Event::AnimEnded(_ae) => {
                if self.animator.term_anim_playing() {
                    return TabEvent::Close;
                }
                else {
                    self.animator.end();
                }
            },
            Event::FingerDown(_fe) => {
                if self.animator.term_anim_playing() {
                    return TabEvent::None
                }
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
                    if !fe.is_touch {
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
                    if fe.move_distance() > 10. {
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
        self._bg_area.get_rect(cx)
    }
    
    pub fn begin_tab(&mut self, cx: &mut Cx) -> Result<(), ()> {
        // pull the bg color from our animation system, uses 'default' value otherwise
        self.bg.shader = live_shader!(cx, self::shader_bg);
        self.bg.z = self.z;
        self.bg.color = self.animator.last_color(cx, live_id!(makepad_render::quad::shader::color));
        
        // check if we are closing
        if self.animator.term_anim_playing() {
            // so so BUT how would we draw this thing with its own clipping
            let bg_inst = self.bg.draw_quad(
                cx,
                Walk::wh(
                    Width::Fix(self._close_anim_rect.w * self.animator.last_float(cx, live_id!(self::tab_closing))),
                    Height::Fix(self._close_anim_rect.h),
                )
            );
            bg_inst.push_last_color(cx, &self.animator, live_id!(self::shader_bg::border_color));
            self._bg_area = bg_inst.into();
            self.animator.set_area(cx, self._bg_area);
            return Err(())
        }
        else {
            let layout = if let Some(abs_origin) = self.abs_origin {
                Layout {abs_origin: Some(abs_origin), ..live_layout!(cx, self::layout_bg)}
            }
            else {
                live_layout!(cx, self::layout_bg)
            };
            let bg_inst = self.bg.begin_quad(cx, layout);
            bg_inst.push_last_color(cx, &self.animator, live_id!(self::shader_bg::border_color));
            if self.is_closeable {
                self.tab_close.draw_tab_close(cx);
                cx.turtle_align_y();
            }
            // push the 2 vars we added to bg shader
            self.text.z = self.z;
            self.text.text_style = live_text_style!(cx, self::text_style_title);
            self.text.color = self.animator.last_color(cx, live_id!(makepad_render::text::shader::color));
            self._text_area = self.text.draw_text(cx, &self.label);
            
            cx.turtle_align_y();
            self._bg_inst = Some(bg_inst);
            
            return Ok(())
        }
    }
    
    pub fn end_tab(&mut self, cx: &mut Cx) {
        if let Some(bg_inst) = self._bg_inst.take() {
            self._bg_area = self.bg.end_quad(cx, bg_inst);
            self.animator.set_area(cx, self._bg_area); // if our area changed, update animation
        }
    }
    
    pub fn draw_tab(&mut self, cx: &mut Cx) {
        if self.begin_tab(cx).is_err() {return};
        self.end_tab(cx);
    }
    
}
