use crate::{
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    makepad_derive_widget::*,
    makepad_draw::*,
    tab_close_button::{TabCloseButton, TabCloseButtonAction},
};

use crate::makepad_draw::DrawSvgGlyph;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.TabBase = #(Tab::script_component(vm))

    mod.widgets.Tab = set_type_default() do mod.widgets.TabBase{
        width: Fit
        height: max(theme.tab_height, 23.)

        align: Align{x: 0.0, y: 0.5}
        padding: theme.mspace_3{top: theme.space_2 * 1.2}
        margin: Inset{right: theme.space_1, top: theme.space_1}

        close_button: TabCloseButton{}

        draw_text +: {
            hover: instance(0.0)
            active: instance(0.0)

            text_style: theme.font_regular{
                font_size: theme.font_size_p
            }

            color_dither: uniform(1.0)
            gradient_fill_horizontal: uniform(0.0)

            color: theme.color_label_inner
            color_hover: uniform(theme.color_label_inner_hover)
            color_active: uniform(theme.color_label_inner_active)

            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            color_2_hover: uniform(#FA0)
            color_2_active: uniform(#0F0)

            get_color: fn() {
                let mut col = self.color
                let mut col_hover = self.color_hover
                let mut col_active = self.color_active

                if self.color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let dir = if self.gradient_fill_horizontal > 0.5 self.pos.x + dither else self.pos.y + dither
                    col = mix(self.color, self.color_2, dir)
                    col_hover = mix(self.color_hover, self.color_2_hover, dir)
                    col_active = mix(self.color_active, self.color_2_active, dir)
                }

                return col
                    .mix(col_active, self.active)
                    .mix(col_hover, self.hover)
            }
        }

        draw_bg +: {
            hover: instance(0.0)
            active: instance(0.0)

            overlap_fix: uniform(1.0)
            border_size: uniform(1.)
            border_radius: uniform(theme.corner_radius)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)
            color_dither: uniform(1.)

            color: uniform(theme.color_d_hidden)
            color_hover: uniform(theme.color_u_hidden)
            color_active: uniform(theme.color_bg_app)

            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            color_2_hover: uniform(theme.color_u_hidden)
            color_2_active: uniform(theme.color_bg_app)

            border_color: uniform(theme.color_u_hidden)
            border_color_hover: uniform(theme.color_u_hidden)
            border_color_active: uniform(theme.color_bevel_outset_1)

            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            border_color_2_hover: uniform(theme.color_d_hidden)
            border_color_2_active: uniform(theme.color_d_hidden)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x
                    self.border_size / self.rect_size.y
                )

                let sz_inner_px = vec2(
                    self.rect_size.x - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2.
                )

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x
                    self.rect_size.y / sz_inner_px.y
                )

                sdf.box_y(
                    self.border_size + self.overlap_fix
                    self.border_size
                    self.rect_size.x - self.border_size * 2. - self.overlap_fix
                    self.rect_size.y
                    self.border_radius
                    max(self.border_size * 0.5, 0.5)
                )

                let mut color_fill = self.color
                let mut color_fill_hover = self.color_hover
                let mut color_fill_active = self.color_active

                if self.color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let gradient_fill = vec2(
                        self.pos.x * scale_factor_fill.x - border_sz_uv.x * 2. + dither
                        self.pos.y * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                    )
                    let dir = if self.gradient_fill_horizontal > 0.5 gradient_fill.x else gradient_fill.y
                    color_fill = mix(self.color, self.color_2, dir)
                    color_fill_hover = mix(self.color_hover, self.color_2_hover, dir)
                    color_fill_active = mix(self.color_active, self.color_2_active, dir)
                }

                let mut color_stroke = self.border_color
                let mut color_stroke_hover = self.border_color_hover
                let mut color_stroke_active = self.border_color_active

                if self.border_color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let gradient_border = vec2(
                        self.pos.x + dither
                        self.pos.y + dither
                    )
                    let dir = if self.gradient_border_horizontal > 0.5 gradient_border.x else gradient_border.y
                    color_stroke = mix(self.border_color, self.border_color_2, dir)
                    color_stroke_hover = mix(self.border_color_hover, self.border_color_2_hover, dir)
                    color_stroke_active = mix(self.border_color_active, self.border_color_2_active, dir)
                }

                let fill = color_fill
                    .mix(color_fill_hover, self.hover)
                    .mix(color_fill_active, self.active)

                let stroke = color_stroke
                    .mix(color_stroke_hover, self.hover)
                    .mix(color_stroke_active, self.active)

                sdf.fill_keep(fill)
                sdf.stroke(stroke, self.border_size)

                return sdf.result
            }
        }

        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward{duration: 0.2}}
                    apply: {
                        draw_bg: {hover: 0.0}
                        draw_text: {hover: 0.0}
                    }
                }

                on: AnimatorState{
                    cursor: MouseCursor.Hand
                    from: {all: Forward{duration: 0.1}}
                    apply: {
                        draw_bg: {hover: snap(1.0)}
                        draw_text: {hover: snap(1.0)}
                    }
                }
            }

            active: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward{duration: 0.3}}
                    apply: {
                        close_button: {draw_button: {active: 0.0}}
                        draw_bg: {active: 0.0}
                        draw_text: {active: 0.0}
                    }
                }

                on: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        close_button: {draw_button: {active: 1.0}}
                        draw_bg: {active: 1.0}
                        draw_text: {active: 1.0}
                    }
                }
            }
        }
    }

    mod.widgets.TabFlat = mod.widgets.Tab{
        margin: 0.
        padding: theme.mspace_3

        draw_bg +: {
            border_size: 1.
            border_radius: 0.5
            color_dither: 1.

            color: theme.color_d_hidden
            color_hover: theme.color_d_hidden
            color_active: theme.color_fg_app

            border_color: theme.color_u_hidden
            border_color_hover: theme.color_u_hidden
            border_color_active: theme.color_fg_app

            border_color_2: theme.color_d_hidden
            border_color_2_hover: theme.color_d_hidden
            border_color_2_active: theme.color_fg_app

            overlap_fix: 0.
        }
    }

    mod.widgets.TabGradientX = mod.widgets.Tab{
        draw_bg +: {
            border_size: 1.
            border_radius: theme.corner_radius
            gradient_border_horizontal: 0.0
            gradient_fill_horizontal: 1.0
            color_dither: 1.
        }

        draw_text +: {
            gradient_fill_horizontal: 1.0
        }
    }

    mod.widgets.TabGradientY = mod.widgets.TabGradientX{
        draw_bg +: {
            border_size: theme.beveling
            border_radius: theme.corner_radius
            color_dither: 1.
        }

        draw_text +: {
            gradient_fill_horizontal: 1.0
        }
    }
}

#[derive(Script, ScriptHook, Animator)]
pub struct Tab {
    #[source]
    source: ScriptObjectRef,
    #[rust]
    is_active: bool,
    #[rust]
    is_dragging: bool,

    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_icon: DrawSvgGlyph,
    #[live]
    draw_text: DrawText,
    #[live]
    icon_walk: Walk,

    #[apply_default]
    animator: Animator,

    #[live]
    close_button: TabCloseButton,

    #[live]
    closeable: bool,
    #[live]
    hover: f32,
    #[live]
    active: f32,

    #[live(10.0)]
    min_drag_dist: f64,

    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
}

pub enum TabAction {
    WasPressed,
    CloseWasPressed,
    ShouldTabStartDrag,
    ShouldTabStopDrag,
}

impl Tab {
    pub fn is_active(&self) -> bool {
        self.is_active
    }

    pub fn set_is_active(&mut self, cx: &mut Cx, is_active: bool, animate: Animate) {
        self.is_active = is_active;
        self.animator_toggle(cx, is_active, animate, ids!(active.on), ids!(active.off));
    }

    pub fn draw(&mut self, cx: &mut Cx2d, name: &str) {
        self.draw_bg.begin(cx, self.walk, self.layout);
        if self.closeable {
            self.close_button.draw(cx);
        }

        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_text
            .draw_walk(cx, Walk::fit(), Align::default(), name);
        self.draw_bg.end(cx);
    }

    pub fn area(&self) -> Area {
        self.draw_bg.area()
    }

    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, TabAction),
    ) {
        self.animator_handle_event(cx, event);

        let mut block_hover_out = false;
        match self.close_button.handle_event(cx, event) {
            TabCloseButtonAction::WasPressed if self.closeable => {
                dispatch_action(cx, TabAction::CloseWasPressed)
            }
            TabCloseButtonAction::HoverIn => block_hover_out = true,
            TabCloseButtonAction::HoverOut => self.animator_play(cx, ids!(hover.off)),
            _ => (),
        };

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                if !block_hover_out {
                    self.animator_play(cx, ids!(hover.off));
                }
            }
            Hit::FingerMove(e) => {
                if !self.is_dragging && (e.abs - e.abs_start).length() > self.min_drag_dist {
                    self.is_dragging = true;
                    dispatch_action(cx, TabAction::ShouldTabStartDrag);
                }
            }
            Hit::FingerUp(_) => {
                if self.is_dragging {
                    dispatch_action(cx, TabAction::ShouldTabStopDrag);
                    self.is_dragging = false;
                }
            }
            Hit::FingerDown(fde) => {
                // A primary click/touch selects the tab, but a middle click closes it.
                if fde.is_primary_hit() {
                    dispatch_action(cx, TabAction::WasPressed);
                } else if self.closeable && fde.mouse_button().is_some_and(|b| b.is_middle()) {
                    dispatch_action(cx, TabAction::CloseWasPressed);
                }
            }
            _ => {}
        }
    }
}
