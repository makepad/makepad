use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
};
live_design!{
    link widgets;
    use link::theme::*;
    use link::shaders::*;

    pub FoldButtonBase = {{FoldButton}} {}

    pub FoldButton = <FoldButtonBase> {
        width: Fit, height: Fit,
        padding: {left: 5., right: 10., top: 5., bottom: 5.}

        layout: {
            flow: Right,
            spacing: 8.0,
            align: {x: 0.0, y: 0.5}
        }

        open_text: "Show More"
        close_text: "Show Less"
        triangle_size: 5.0

        draw_bg: {
            instance active: 0.0
            instance hover: 0.0
            instance triangle_size: 2.5

            uniform fade: 1.0
            uniform rect_color: vec4(-1.0, -1.0, -1.0, 0.0)
            uniform rect_color_hover: (THEME_COLOR_OUTSET_2_HOVER)
            uniform border_color: (THEME_COLOR_BEVEL)
            uniform border_radius: 2.0
            uniform color: #666
            uniform color_active: #000
            uniform color_hover: #000
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.clear(vec4(0.));

                // Draw background with rounded corners
                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    self.border_radius
                )

                sdf.fill_keep(
                    mix(self.rect_color, self.rect_color_hover, self.hover)
                )

                sdf.stroke(
                    self.border_color,
                    1.0
                )

                // Draw triangle - positioned from left edge with y-axis center alignment
                let sz = self.triangle_size;
                let triangle_x = 5.0 + sz;  // Position from left with padding
                let c = vec2(triangle_x, self.rect_size.y * 0.5);
                // Rotate triangle based on active state
                sdf.rotate(self.active * 0.5 * PI + 0.5 * PI, c.x, c.y);
                sdf.move_to(c.x - sz, c.y + sz);
                sdf.line_to(c.x, c.y - sz);
                sdf.line_to(c.x + sz, c.y + sz);
                sdf.close_path();
                sdf.fill(
                    mix(
                        mix(self.color, self.color_hover, self.hover),
                            mix(self.color_active, self.color_hover, self.hover),
                                self.active
                    )
                );
                return sdf.result * self.fade;
            }
        }

        draw_text: {
            instance hover: 0.0
            instance down: 0.0

            color: #666
            uniform color_hover: #000

            text_style: <THEME_FONT_REGULAR> {
                font_size: 11.0
            }

            fn get_color(self) -> vec4 {
                return mix(
                    self.color,
                    self.color_hover,
                    self.hover
                )
            }
        }

        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
                    redraw: true
                    apply: {
                        draw_bg: {hover: 0.0}
                        draw_text: {hover: 0.0}
                    }
                }

                on = {
                    from: {all: Snap}
                    redraw: true
                    apply: {
                        draw_bg: {hover: 1.0}
                        draw_text: {hover: 1.0}
                    }
                }
            }

            active = {
                default: on
                off = {
                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.96, d2: 0.97}
                    redraw: true
                    apply: {
                        active: 0.0,

                        draw_bg: {active: [{time: 0.0, value: 1.0}, {time: 1.0, value: 0.0}]}
                    }
                }
                on = {
                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.98, d2: 0.95}
                    redraw: true
                    apply: {
                        active: 1.0
                        draw_bg: {active: [{time: 0.0, value: 0.0}, {time: 1.0, value: 1.0}]}
                    }
                }
            }
        }
    }
}

#[derive(Live, Widget)]
pub struct FoldButton {
    #[animator] animator: Animator,

    #[redraw] #[live] draw_bg: DrawQuad,
    #[redraw] #[live] draw_text: DrawText,

    #[walk] walk: Walk,
    #[layout] layout: Layout,

    #[live] active: f64,
    #[live] triangle_size: f64,
    #[live] open_text: ArcStringMut,
    #[live] close_text: ArcStringMut,

    #[action_data] #[rust] action_data: WidgetActionData,
}
impl LiveHook for FoldButton {
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.apply_over(cx, live!(
            draw_bg: {
                triangle_size: (self.triangle_size)
            }
        ));
    }
}
impl Widget for FoldButton {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        let res = self.animator_handle_event(cx, event);

        if res.is_animating() {
            if self.animator.is_track_animating(cx, ids!(active)) {
                let mut value = [0.0];
                self.draw_bg.get_instance(cx, ids!(active), &mut value);
                cx.widget_action(uid, &scope.path, FoldButtonAction::Animating(value[0] as f64))
            }
            if res.must_redraw() {
                self.draw_bg.redraw(cx);
            }
        }

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(_fe) => {
                if self.animator_in_state(cx, ids!(active.on)) {
                    self.animator_play(cx, ids!(active.off));
                    cx.widget_action(uid, &scope.path, FoldButtonAction::Closing)
                } else {
                    self.animator_play(cx, ids!(active.on));
                    cx.widget_action(uid, &scope.path, FoldButtonAction::Opening)
                }
                self.animator_play(cx, ids!(hover.on));
            },
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerUp(fe) => {
                if fe.is_over {
                    if fe.device.has_hovers() {
                        self.animator_play(cx, ids!(hover.on));
                    } else {
                        self.animator_play(cx, ids!(hover.off));
                    }
                } else {
                    self.animator_play(cx, ids!(hover.off));
                }
            }
            _ => ()
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);
        let label_walk = walk.with_margin_left(self.triangle_size * 2.0 + 10.0);
        let text = if self.active > 0.5 {
            self.close_text.as_ref()
        } else {
            self.open_text.as_ref()
        };
        self.draw_text.draw_walk(cx, label_walk, Align::default(), text);
        self.draw_bg.end(cx);
        DrawStep::done()
    }
}

impl FoldButton {
    pub fn set_is_open(&mut self, cx: &mut Cx, is_open: bool, animate: Animate) {
        self.animator_toggle(cx, is_open, animate, ids!(active.on), ids!(active.off))
    }

    pub fn opening(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FoldButtonAction::Opening = item.cast() {
                return true
            }
        }
        false
    }

    pub fn closing(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FoldButtonAction::Closing = item.cast() {
                return true
            }
        }
        false
    }

    pub fn animating(&self, actions: &Actions) -> Option<f64> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FoldButtonAction::Animating(v) = item.cast() {
                return Some(v)
            }
        }
        None
    }
    /// Get the open and close texts
    pub fn texts(&self) -> (String, String) {
        (self.open_text.as_ref().to_string(), self.close_text.as_ref().to_string())
    }
    /// Set the open and close texts
    pub fn set_texts(&mut self, cx: &mut Cx, open_text: &str, close_text: &str) {
        self.open_text.as_mut_empty().push_str(open_text);
        self.close_text.as_mut_empty().push_str(close_text);
        self.redraw(cx);
    }
}

impl FoldButtonRef {
    pub fn opening(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FoldButtonAction::Opening = item.cast() {
                return true
            }
        }
        false
    }

    pub fn closing(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FoldButtonAction::Closing = item.cast() {
                return true
            }
        }
        false
    }

    pub fn animating(&self, actions: &Actions) -> Option<f64> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FoldButtonAction::Animating(v) = item.cast() {
                return Some(v)
            }
        }
        None
    }

    pub fn open_float(&self) -> f64 {
        if let Some(inner) = self.borrow() {
            inner.active
        } else {
            1.0
        }
    }

    /// See [`FoldButton::texts()`].
    pub fn texts(&self) -> Option<(String, String)> {
        if let Some(inner) = self.borrow() {
            Some(inner.texts())
        } else {
            None
        }
    }

    /// See [`FoldButton::set_texts()`].
    pub fn set_texts(&self, cx: &mut Cx, open_text: &str, close_text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_texts(cx, open_text, close_text);
        }
    }
}


#[derive(Clone, Debug, DefaultNone)]
pub enum FoldButtonAction {
    None,
    Opening,
    Closing,
    Animating(f64)
}