use crate::{
    animator::{Animator, AnimatorAction, AnimatorImpl, Play},
    button::ButtonAction,
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    let DesktopButtonType = set_type_default() do #(DesktopButtonType::script_api(vm))
    mod.widgets.DesktopButtonType = DesktopButtonType

    mod.widgets.DesktopButtonBase = #(DesktopButton::register_widget(vm))

    mod.widgets.DesktopButton = set_type_default() do mod.widgets.DesktopButtonBase{
        width: 46 height: 29
        draw_bg +: {
            button_type: instance(DesktopButtonType.Fullscreen)
            hover: instance(0.0)
            down: instance(0.0)

            color: uniform(theme.color_label_inner)
            color_hover: uniform(theme.color_label_inner_hover)
            color_down: uniform(theme.color_label_inner_down)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.aa = sdf.aa * 3.0
                let sz = 4.5
                let c = self.rect_size * vec2(0.5, 0.5)

                let color = self.color
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_down, self.down)

                match self.button_type {
                    DesktopButtonType.WindowsMin => {
                        sdf.move_to(c.x - sz, c.y)
                        sdf.line_to(c.x + sz, c.y)
                        sdf.stroke(color, 0.5 + 0.5 * self.draw_pass.dpi_dilate)
                        return sdf.result
                    }
                    DesktopButtonType.WindowsMax => {
                        sdf.rect(c.x - sz, c.y - sz, 2. * sz, 2. * sz)
                        sdf.stroke(color, 0.5 + 0.5 * self.draw_pass.dpi_dilate)
                        return sdf.result
                    }
                    DesktopButtonType.WindowsMaxToggled => {
                        let sz = 5.
                        sdf.rect(c.x - sz + 1., c.y - sz - 1., 2. * sz, 2. * sz)
                        sdf.stroke(#f, 0.5 + 0.5 * self.draw_pass.dpi_dilate)
                        sdf.rect(c.x - sz - 1., c.y - sz + 1., 2. * sz, 2. * sz)
                        sdf.stroke(color, 0.5 + 0.5 * self.draw_pass.dpi_dilate)
                        return sdf.result
                    }
                    DesktopButtonType.WindowsClose => {
                        sdf.move_to(c.x - sz, c.y - sz)
                        sdf.line_to(c.x + sz, c.y + sz)
                        sdf.move_to(c.x - sz, c.y + sz)
                        sdf.line_to(c.x + sz, c.y - sz)
                        sdf.stroke(color, 0.5 + 0.5 * self.draw_pass.dpi_dilate)
                        return sdf.result
                    }
                    DesktopButtonType.XRMode => {
                        let w = 12.
                        let h = 8.
                        sdf.box(c.x - w, c.y - h, 2. * w, 2. * h, 2.)
                        sdf.circle(c.x - 5.5, c.y, 3.5)
                        sdf.subtract()
                        sdf.circle(c.x + 5.5, c.y, 3.5)
                        sdf.subtract()
                        sdf.circle(c.x, c.y + h - 0.75, 2.5)
                        sdf.subtract()
                        sdf.fill(color)
                        return sdf.result
                    }
                    DesktopButtonType.Fullscreen => {
                        let sz = 8.
                        sdf.rect(c.x - sz, c.y - sz, 2. * sz, 2. * sz)
                        sdf.rect(c.x - sz + 1.5, c.y - sz + 1.5, 2. * (sz - 1.5), 2. * (sz - 1.5))
                        sdf.subtract()
                        sdf.rect(c.x - sz + 4., c.y - sz - 2., 2. * (sz - 4.), 2. * (sz + 2.))
                        sdf.subtract()
                        sdf.rect(c.x - sz - 2., c.y - sz + 4., 2. * (sz + 2.), 2. * (sz - 4.))
                        sdf.subtract()
                        sdf.fill(color)
                        return sdf.result
                    }
                    DesktopButtonType.RecordOff => {
                        let rr = 5.0
                        sdf.circle(c.x, c.y, rr)
                        sdf.stroke(#ff3b30, 1.0 + 0.5 * self.draw_pass.dpi_dilate)
                        return sdf.result
                    }
                    DesktopButtonType.RecordOn => {
                        let rr = 5.0
                        sdf.circle(c.x, c.y, rr)
                        sdf.fill(#ff3b30)
                        return sdf.result
                    }
                }
                return #f00
            }
        }
        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {down: 0.0, hover: 0.0}
                    }
                }

                on: AnimatorState{
                    from: {
                        all: Forward {duration: 0.1}
                        down: Snap
                    }
                    apply: {
                        draw_bg: {
                            down: 0.0
                            hover: 1.0
                        }
                    }
                }

                down: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {
                            down: 1.0
                            hover: 1.0
                        }
                    }
                }
            }
        }
    }

}

#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum DesktopButtonType {
    WindowsMin = 1,
    WindowsMax = 2,
    WindowsMaxToggled = 3,
    WindowsClose = 4,
    XRMode = 5,
    #[pick]
    Fullscreen = 6,
    RecordOff = 7,
    RecordOn = 8,
}

#[derive(Script, ScriptHook, Widget, Animator)]
pub struct DesktopButton {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[apply_default]
    animator: Animator,
    #[walk]
    walk: Walk,
    #[visible]
    #[live(true)]
    visible: bool,
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
}

impl Widget for DesktopButton {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if !self.visible {
            return;
        }

        let uid = self.widget_uid();
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(fe) => {
                cx.widget_action(uid, ButtonAction::Pressed(fe.modifiers));
                self.animator_play(cx, ids!(hover.down));
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerLongPress(_) => {
                cx.widget_action(uid, ButtonAction::LongPressed);
            }
            Hit::FingerUp(fe) => {
                if fe.is_over {
                    cx.widget_action(uid, ButtonAction::Clicked(fe.modifiers));
                    if fe.device.has_hovers() {
                        self.animator_play(cx, ids!(hover.on));
                    } else {
                        self.animator_play(cx, ids!(hover.off));
                    }
                } else {
                    cx.widget_action(uid, ButtonAction::Released(fe.modifiers));
                    self.animator_play(cx, ids!(hover.off));
                }
            }
            _ => (),
        };
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }

        let _ = self.draw_bg.draw_walk(cx, walk);
        DrawStep::done()
    }
}

impl DesktopButtonRef {
    pub fn clicked(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            matches!(item.cast(), ButtonAction::Clicked(_))
        } else {
            false
        }
    }
}
