use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

use crate::makepad_draw::DrawSvg;

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.IconBase = #(Icon::register_widget(vm))

    mod.widgets.Icon = set_type_default() do mod.widgets.IconBase{
        width: Fit
        height: Fit

        // Don't clip: SVGs are fit by content bounds (no stroke extent), so a tight clip squares off round caps.
        clip_x: false
        clip_y: false

        /** icon side in pixels; 0 keeps icon_walk's width 0..64 step 1 */
        size: 0.0

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

    /** The icon inked with a two-stop gradient down its height: color at
     * the top, color_2 at the bottom. */
    mod.widgets.IconGradientX = mod.widgets.Icon{
        draw_icon +: {
            /** the ink at the gradient's start */
            color: theme.color_primary
            /** the ink at the gradient's end; a negative alpha keeps the start ink throughout */
            color_2: instance(theme.color_tertiary)
            /** gradient axis: 0 down the icon, 1 across it 0..1 step 1 */
            gradient_fill_horizontal: instance(0.0)

            get_color: fn() {
                let base = self.eval_gradient()
                if self.color.x < 0.0 {
                    return base
                }
                let mut ink = self.color
                if self.color_2.x >= 0.0 {
                    // The icon's place in its box, from the world position
                    // the vertex stage hands down.
                    let uv = (self.v_world - self.draw_list.view_shift - self.rect_pos) / self.rect_size
                    let t = clamp(mix(uv.y, uv.x, self.gradient_fill_horizontal), 0.0, 1.0)
                    ink = mix(self.color, self.color_2, t)
                }
                return vec4(ink.rgb * ink.a * base.a, ink.a * base.a)
            }
        }
    }

    /** The gradient icon turned sideways: color at the left, color_2 at the right. */
    mod.widgets.IconGradientY = mod.widgets.IconGradientX{
        draw_icon.gradient_fill_horizontal: 1.0
    }

    /** The icon on a filled disc: the primary role as the disc, its
     * on-colour as the ink. size sets the icon; the padding grows the disc. */
    mod.widgets.IconFilled = mod.widgets.Icon{
        padding: theme.space_1
        draw_bg +: {
            color: theme.color_primary
            /** disc corner radius; the box clamps it to a circle 0..999 step 0.5 */
            radius: uniform(theme.radius_full)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0., 0., self.rect_size.x, self.rect_size.y, self.radius)
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_icon +: {
            color: theme.color_on_primary
        }
    }

    /** The icon on a light disc: the primary container as the disc, its
     * on-colour as the ink. */
    mod.widgets.IconLight = mod.widgets.IconFilled{
        draw_bg +: {
            color: theme.color_primary_container
        }
        draw_icon +: {
            color: theme.color_on_primary_container
        }
    }

    /** The icon in an outlined ring: no disc, a one-pixel outline in the
     * primary role, and the primary ink. */
    mod.widgets.IconOutline = mod.widgets.IconFilled{
        draw_bg +: {
            color: theme.color_primary
            /** outline thickness in pixels 0..4 step 0.5 */
            border_size: uniform(1.0)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(
                    self.border_size
                    self.border_size
                    self.rect_size.x - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2.
                    self.radius
                )
                sdf.stroke(self.color, self.border_size)
                return sdf.result
            }
        }
        draw_icon +: {
            color: theme.color_primary
        }
    }

    mod.widgets.IconRotated = mod.widgets.Icon{
        draw_icon +: {
            rotation_angle: uniform(0.0)

            transform_svg_point: fn(pos: vec2) -> vec2 {
                 // The hook works in the rect's own space: DrawSvg adds
                 // rect_pos after it, so the pivot is the local centre.
                 let center = self.rect_size * 0.5;
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
    /// The icon's side in pixels; 0 keeps `icon_walk`'s width. One number
    /// so the presets and their hosts size an icon without touching the walk.
    #[live]
    size: f64,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
}

impl Widget for Icon {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);
        let icon_walk = if self.size > 0.0 {
            Walk {
                width: Size::Fixed(self.size),
                height: Size::fit(),
                ..self.icon_walk
            }
        } else {
            self.icon_walk
        };
        self.draw_icon.draw_walk(cx, icon_walk);
        self.draw_bg.end(cx);
        DrawStep::done()
    }
}
