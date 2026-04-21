use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

use crate::makepad_draw::DrawSvg;

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.IconBase = #(Icon::register_widget(vm))

    mod.widgets.Icon = set_type_default() do mod.widgets.IconBase{
        width: Fit
        height: Fit

        icon_walk: Walk{
            width: 17.5
            height: Fit
        }

        draw_bg +: {
            color_dither: uniform(1.0)
            color: instance(#0000)
            color_2: instance(vec4(-1.0, -1.0, -1.0, -1.0))
            gradient_fill_horizontal: uniform(0.0)

            pixel: fn() {
                let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                let mut color_2 = self.color_2

                let mut gradient_fill_dir = self.pos.y + dither
                if self.gradient_fill_horizontal > 0.5 {
                    gradient_fill_dir = self.pos.x + dither
                }

                if self.color_2.x < -0.5 {
                    color_2 = self.color
                }

                return mix(self.color, color_2, gradient_fill_dir)
            }
        }
    }

    mod.widgets.IconGradientX = mod.widgets.Icon{}
    mod.widgets.IconGradientY = mod.widgets.Icon{}

    mod.widgets.IconRotated = mod.widgets.Icon{
        draw_icon +: {
            rotation_angle: uniform(0.0)

            transform_svg_point: fn(pos: vec2) -> vec2 {
                 let center = self.rect_pos + self.rect_size * 0.5;
                 let scaled = pos - center;
                 let cs = cos(self.rotation_angle);
                 let sn = sin(self.rotation_angle);
                 return vec2(
                     scaled.x * cs - scaled.y * sn,
                     scaled.x * sn + scaled.y * cs
                 ) + center;
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct Icon {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_icon: DrawSvg,
    #[live]
    icon_walk: Walk,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
}

impl Widget for Icon {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_bg.end(cx);
        DrawStep::done()
    }
}
