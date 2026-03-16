use crate::*;

pub const MP_MAX_CLIP_PLANES: usize = 8;
const CLIP_PLANE_IDS: [LiveId; MP_MAX_CLIP_PLANES] = [
    live_id!(clip_plane_0),
    live_id!(clip_plane_1),
    live_id!(clip_plane_2),
    live_id!(clip_plane_3),
    live_id!(clip_plane_4),
    live_id!(clip_plane_5),
    live_id!(clip_plane_6),
    live_id!(clip_plane_7),
];

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.DrawProjectedQuad = mod.std.set_type_default() do #(DrawProjectedQuad::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_texture: texture_2d(float)
        clip_plane_count: uniform(float(0.0))
        clip_plane_0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_7: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        uv: varying(vec2f)
        clip_space: varying(vec4f)

        clip_projected: fn() -> float {
            if self.clip_plane_count > 0.5 && dot(self.clip_space, self.clip_plane_0) < 0.0 { return 0.0 }
            if self.clip_plane_count > 1.5 && dot(self.clip_space, self.clip_plane_1) < 0.0 { return 0.0 }
            if self.clip_plane_count > 2.5 && dot(self.clip_space, self.clip_plane_2) < 0.0 { return 0.0 }
            if self.clip_plane_count > 3.5 && dot(self.clip_space, self.clip_plane_3) < 0.0 { return 0.0 }
            if self.clip_plane_count > 4.5 && dot(self.clip_space, self.clip_plane_4) < 0.0 { return 0.0 }
            if self.clip_plane_count > 5.5 && dot(self.clip_space, self.clip_plane_5) < 0.0 { return 0.0 }
            if self.clip_plane_count > 6.5 && dot(self.clip_space, self.clip_plane_6) < 0.0 { return 0.0 }
            if self.clip_plane_count > 7.5 && dot(self.clip_space, self.clip_plane_7) < 0.0 { return 0.0 }
            return 1.0
        }

        vertex: fn() {
            let local = self.geom.pos * self.rect_size + self.rect_pos
            self.uv = self.uv_rect.xy + (self.uv_rect.zw - self.uv_rect.xy) * self.geom.pos
            let world = (self.draw_list.view_transform * self.transform) * vec4(
                local.x,
                local.y,
                self.draw_depth + self.draw_call.zbias,
                1.0
            )
            self.clip_space = self.draw_pass.camera_projection * (self.draw_pass.camera_view * world)
            self.vertex_pos = self.clip_space
        }

        pixel: fn() {
            if self.clip_projected() < 0.5 {
                discard()
            }
            let sampled = self.color_texture.sample_as_bgra(self.uv)
            let alpha = clamp(sampled.w * self.opacity, 0.0, 1.0)
            let rgb = if self.premultiplied > 0.5 {
                sampled.xyz * self.opacity
            } else {
                sampled.xyz * alpha
            }
            return vec4(rgb, alpha)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}

#[derive(Clone, Debug)]
pub struct MpCompositedQuad {
    pub texture: Texture,
    pub local_rect: Rect,
    pub uv_rect: Rect,
    pub transform: Mat4f,
    pub opacity: f32,
    pub premultiplied: bool,
    pub backface_visible: bool,
    pub depth_write: bool,
    pub clip_planes: Vec<Vec4f>,
}

impl MpCompositedQuad {
    pub fn new(texture: Texture, local_rect: Rect) -> Self {
        Self {
            texture,
            local_rect,
            uv_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(1.0, 1.0),
            },
            transform: Mat4f::identity(),
            opacity: 1.0,
            premultiplied: true,
            backface_visible: true,
            depth_write: true,
            clip_planes: Vec::new(),
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawProjectedQuad {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    transform: Mat4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    uv_rect: Vec4f,
    #[live(1.0)]
    opacity: f32,
    #[live(1.0)]
    premultiplied: f32,
}

impl DrawProjectedQuad {
    fn draw(&mut self, cx: &mut Cx2d) {
        self.draw_super.draw_vars.append_group_id = cx.draw_call_group_background().0;
        if self.draw_super.draw_vars.can_instance() {
            let new_area = cx.add_instance(&self.draw_super.draw_vars);
            self.draw_super.draw_vars.area =
                cx.update_area_refs(self.draw_super.draw_vars.area, new_area);
        }
    }
}

pub struct MpCompositor {
    draw_quad: DrawProjectedQuad,
}

impl MpCompositor {
    pub fn new(cx: &mut Cx) -> Self {
        cx.with_vm(|vm| {
            makepad_draw::script_mod(vm);
            crate::script_mod(vm);
            Self {
                draw_quad: DrawProjectedQuad::script_new_with_default(vm),
            }
        })
    }

    pub fn draw_quad(&mut self, cx: &mut Cx2d, quad: &MpCompositedQuad) {
        if !quad.backface_visible && is_backface_culled(cx, quad) {
            return;
        }

        self.draw_quad.draw_super.rect_pos = quad.local_rect.pos.into();
        self.draw_quad.draw_super.rect_size = quad.local_rect.size.into();
        self.draw_quad.transform = quad.transform;
        self.draw_quad.uv_rect = rect_to_vec4(quad.uv_rect);
        self.draw_quad.opacity = quad.opacity.clamp(0.0, 1.0);
        self.draw_quad.premultiplied = if quad.premultiplied { 1.0 } else { 0.0 };
        self.draw_quad.draw_super.draw_vars.options.depth_write = quad.depth_write;
        self.draw_quad
            .draw_super
            .draw_vars
            .set_texture(0, &quad.texture);
        set_clip_planes(
            cx,
            &mut self.draw_quad.draw_super.draw_vars,
            &quad.clip_planes,
        );
        self.draw_quad.draw(cx);
    }

    pub fn draw_batch(&mut self, cx: &mut Cx2d, quads: &[MpCompositedQuad]) {
        for quad in quads {
            self.draw_quad(cx, quad);
        }
    }
}

fn set_clip_planes(cx: &Cx2d, draw_vars: &mut DrawVars, clip_planes: &[Vec4f]) {
    let count = clip_planes.len().min(MP_MAX_CLIP_PLANES);
    draw_vars.set_uniform(cx.cx, live_id!(clip_plane_count), &[count as f32]);
    for (index, id) in CLIP_PLANE_IDS.iter().enumerate() {
        let plane = clip_planes
            .get(index)
            .copied()
            .unwrap_or_else(|| vec4(0.0, 0.0, 0.0, 0.0));
        draw_vars.set_uniform(cx.cx, *id, &[plane.x, plane.y, plane.z, plane.w]);
    }
}

fn rect_to_vec4(rect: Rect) -> Vec4f {
    vec4(
        rect.pos.x as f32,
        rect.pos.y as f32,
        (rect.pos.x + rect.size.x) as f32,
        (rect.pos.y + rect.size.y) as f32,
    )
}

fn current_view_projection(cx: &Cx2d) -> Option<(Mat4f, Mat4f)> {
    let draw_list_id = *cx.draw_list_stack.last()?;
    let draw_list = &cx.draw_lists[draw_list_id];
    let pass_id = draw_list.draw_pass_id?;
    let pass = &cx.passes[pass_id];
    let pass_view_projection = Mat4f::mul(
        &pass.pass_uniforms.camera_projection,
        &pass.pass_uniforms.camera_view,
    );
    Some((
        pass_view_projection,
        draw_list.draw_list_uniforms.view_transform,
    ))
}

fn is_backface_culled(cx: &Cx2d, quad: &MpCompositedQuad) -> bool {
    let Some((pass_view_projection, draw_list_transform)) = current_view_projection(cx) else {
        return false;
    };

    let reference_mvp = Mat4f::mul(&pass_view_projection, &draw_list_transform);
    let transformed_mvp = Mat4f::mul(&reference_mvp, &quad.transform);

    let Some(reference_area) = projected_signed_area(&reference_mvp, quad.local_rect) else {
        return false;
    };
    let Some(quad_area) = projected_signed_area(&transformed_mvp, quad.local_rect) else {
        return false;
    };

    if reference_area.abs() <= 1e-6 || quad_area.abs() <= 1e-6 {
        return false;
    }

    reference_area.is_sign_positive() != quad_area.is_sign_positive()
}

fn projected_signed_area(mvp: &Mat4f, rect: Rect) -> Option<f32> {
    let p0 = project_point(mvp, rect.pos.x as f32, rect.pos.y as f32)?;
    let p1 = project_point(mvp, (rect.pos.x + rect.size.x) as f32, rect.pos.y as f32)?;
    let p2 = project_point(
        mvp,
        (rect.pos.x + rect.size.x) as f32,
        (rect.pos.y + rect.size.y) as f32,
    )?;
    Some((p1.x - p0.x) * (p2.y - p0.y) - (p1.y - p0.y) * (p2.x - p0.x))
}

fn project_point(mvp: &Mat4f, x: f32, y: f32) -> Option<Vec2f> {
    let clip = mvp.transform_vec4(vec4f(x, y, 0.0, 1.0));
    if clip.w.abs() <= 1e-6 {
        return None;
    }
    Some(vec2(clip.x / clip.w, clip.y / clip.w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winding_flips_for_half_turn_around_y() {
        let rect = Rect {
            pos: dvec2(-1.0, -1.0),
            size: dvec2(2.0, 2.0),
        };
        let reference_mvp = Mat4f::identity();
        let flipped_mvp = Mat4f::mul(
            &reference_mvp,
            &Mat4f::rotation(vec3(0.0, std::f32::consts::PI, 0.0)),
        );

        let reference_area = projected_signed_area(&reference_mvp, rect).unwrap();
        let flipped_area = projected_signed_area(&flipped_mvp, rect).unwrap();

        assert!(reference_area.abs() > 0.0);
        assert!(flipped_area.abs() > 0.0);
        assert_ne!(
            reference_area.is_sign_positive(),
            flipped_area.is_sign_positive()
        );
    }

    #[test]
    fn quad_constructor_defaults_match_compositor_expectations() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let texture = Texture::new(&mut cx);
        let quad = MpCompositedQuad::new(
            texture,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(10.0, 20.0),
            },
        );

        assert_eq!(quad.uv_rect.pos, dvec2(0.0, 0.0));
        assert_eq!(quad.uv_rect.size, dvec2(1.0, 1.0));
        assert_eq!(quad.opacity, 1.0);
        assert!(quad.premultiplied);
        assert!(quad.backface_visible);
        assert!(quad.depth_write);
        assert!(quad.clip_planes.is_empty());
    }
}
