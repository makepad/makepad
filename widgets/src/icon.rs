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

            vertex: fn() {
                var pos = vec2(self.geom.x, self.geom.y);

                if self.geom.stroke_mult > 1e5 {
                    let normal = vec2(self.geom.v, self.geom.stroke_dist);
                    let nlen = length(normal);
                    if nlen > 0.0001 {
                        let un = normal / nlen;
                        let screen_scale = length(vec2(un.x * self.svg_scale.x, un.y * self.svg_scale.y));
                        if screen_scale > 0.0001 {
                            let half_px = 0.5 / screen_scale;
                            if self.geom.u < 0.25 {
                                pos = pos + un * half_px;
                            } else if self.geom.u > 0.25 {
                                pos = pos - un * half_px;
                            }
                        }
                    }
                }

                let transformed = pos * self.svg_scale + self.svg_offset;

                let center = self.rect_pos + self.rect_size * 0.5;
                let scaled = transformed - center;
                let cs = cos(self.rotation_angle);
                let sn = sin(self.rotation_angle);
                let rotated = vec2(
                    scaled.x * cs - scaled.y * sn,
                    scaled.x * sn + scaled.y * cs
                ) + center;

                self.v_tcoord = vec2(self.geom.u, self.geom.v);
                self.v_color = vec4(self.geom.color_r, self.geom.color_g, self.geom.color_b, self.geom.color_a);
                self.v_stroke_mult = self.geom.stroke_mult;
                self.v_stroke_dist = self.geom.stroke_dist;
                self.v_shape_id = self.geom.shape_id;
                self.v_param0 = self.geom.param0;
                self.v_param5 = self.geom.param5;

                let grad_type = self.geom.param0;
                if grad_type > 0.5 && grad_type < 1.5 {
                    let p0 = vec2(self.geom.param1, self.geom.param2) * self.svg_scale + self.svg_offset;
                    let p1 = vec2(self.geom.param3, self.geom.param4) * self.svg_scale + self.svg_offset;
                    let p0_r = vec2((p0.x - center.x)*cs - (p0.y - center.y)*sn, (p0.x - center.x)*sn + (p0.y - center.y)*cs) + center;
                    let p1_r = vec2((p1.x - center.x)*cs - (p1.y - center.y)*sn, (p1.x - center.x)*sn + (p1.y - center.y)*cs) + center;
                    self.v_param1 = p0_r.x;
                    self.v_param2 = p0_r.y;
                    self.v_param3 = p1_r.x;
                    self.v_param4 = p1_r.y;
                } else if grad_type > 1.5 {
                    let c = vec2(self.geom.param1, self.geom.param2) * self.svg_scale + self.svg_offset;
                    let c_r = vec2((c.x - center.x)*cs - (c.y - center.y)*sn, (c.x - center.x)*sn + (c.y - center.y)*cs) + center;
                    self.v_param1 = c_r.x;
                    self.v_param2 = c_r.y;
                    self.v_param3 = self.geom.param3 * self.svg_scale.x;
                    self.v_param4 = self.geom.param4 * self.svg_scale.y;
                } else if self.geom.shape_id > 0.5 {
                    let bbox_min = vec2(self.geom.param1, self.geom.param2) * self.svg_scale + self.svg_offset;
                    let bbox_max = vec2(self.geom.param3, self.geom.param4) * self.svg_scale + self.svg_offset;
                    self.v_param1 = bbox_min.x;
                    self.v_param2 = bbox_min.y;
                    self.v_param3 = bbox_max.x;
                    self.v_param4 = bbox_max.y;
                } else {
                    self.v_param1 = self.geom.param1;
                    self.v_param2 = self.geom.param2;
                    self.v_param3 = self.geom.param3;
                    self.v_param4 = self.geom.param4;
                }

                let shifted = rotated + self.draw_list.view_shift;
                self.v_world = shifted;

                let cr = self.geom.clip_radius * max(abs(self.svg_scale.x), abs(self.svg_scale.y));
                let is_shadow = self.geom.stroke_mult < -0.5;
                if cr > 0.0 && !is_shadow {
                    let clip = vec4(
                        max(self.draw_clip.x, self.draw_list.view_clip.x - self.draw_list.view_shift.x),
                        max(self.draw_clip.y, self.draw_list.view_clip.y - self.draw_list.view_shift.y),
                        min(self.draw_clip.z, self.draw_list.view_clip.z - self.draw_list.view_shift.x),
                        min(self.draw_clip.w, self.draw_list.view_clip.w - self.draw_list.view_shift.y)
                    );
                    if rotated.x + cr < clip.x || rotated.y + cr < clip.y
                        || rotated.x - cr > clip.z || rotated.y - cr > clip.w {
                        self.vertex_pos = vec4(2.0, 2.0, 2.0, 1.0);
                        return
                    }
                }

                let world = self.draw_list.view_transform * vec4(
                    shifted.x,
                    shifted.y,
                    self.draw_depth + self.draw_call.zbias + self.geom.zbias,
                    1.
                );
                self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * world)
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
