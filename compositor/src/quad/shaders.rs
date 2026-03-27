use crate::*;

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
        clip_mask_count: uniform(float(0.0))
        clip_mask_type_0: uniform(float(0.0))
        clip_mask_type_1: uniform(float(0.0))
        clip_mask_type_2: uniform(float(0.0))
        clip_mask_type_3: uniform(float(0.0))
        clip_mask_rect_0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_rect_1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_rect_2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_rect_3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_radius_0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_radius_1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_radius_2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_radius_3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_matrix_0: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        clip_mask_matrix_1: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        clip_mask_matrix_2: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        clip_mask_matrix_3: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))

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

        rect_mask_alpha: fn(rect: vec4, local: vec2f) -> float {
            if local.x < rect.x || local.y < rect.y || local.x > rect.x + rect.z || local.y > rect.y + rect.w {
                return 0.0
            }
            return 1.0
        }

        rounded_mask_alpha: fn(rect: vec4, radius: vec4, local: vec2f) -> float {
            if self.rect_mask_alpha(rect, local) < 0.5 {
                return 0.0
            }
            let min_x = rect.x
            let min_y = rect.y
            let max_x = rect.x + rect.z
            let max_y = rect.y + rect.w
            let tl = max(radius.x, 0.0)
            if tl > 0.0 && local.x < min_x + tl && local.y < min_y + tl {
                let d = local - vec2(min_x + tl, min_y + tl)
                if length(d) <= tl { return 1.0 }
                return 0.0
            }
            let tr = max(radius.y, 0.0)
            if tr > 0.0 && local.x > max_x - tr && local.y < min_y + tr {
                let d = local - vec2(max_x - tr, min_y + tr)
                if length(d) <= tr { return 1.0 }
                return 0.0
            }
            let br = max(radius.z, 0.0)
            if br > 0.0 && local.x > max_x - br && local.y > max_y - br {
                let d = local - vec2(max_x - br, max_y - br)
                if length(d) <= br { return 1.0 }
                return 0.0
            }
            let bl = max(radius.w, 0.0)
            if bl > 0.0 && local.x < min_x + bl && local.y > max_y - bl {
                let d = local - vec2(min_x + bl, max_y - bl)
                if length(d) <= bl { return 1.0 }
                return 0.0
            }
            return 1.0
        }

        clip_mask_alpha: fn(mask_type: float, rect: vec4, radius: vec4, mask_matrix: mat4x4f) -> float {
            let local_h = mask_matrix * self.clip_space
            if abs(local_h.w) <= 0.000001 {
                return 0.0
            }
            let local = local_h.xy / local_h.w
            if mask_type < 0.5 {
                return 1.0
            }
            if mask_type < 1.5 {
                return self.rounded_mask_alpha(rect, radius, local)
            }
            return self.rect_mask_alpha(rect, local)
        }

        accumulated_mask_alpha: fn() -> float {
            var mask = 1.0
            if self.clip_mask_count > 0.5 {
                mask *= self.clip_mask_alpha(self.clip_mask_type_0, self.clip_mask_rect_0, self.clip_mask_radius_0, self.clip_mask_matrix_0)
            }
            if self.clip_mask_count > 1.5 {
                mask *= self.clip_mask_alpha(self.clip_mask_type_1, self.clip_mask_rect_1, self.clip_mask_radius_1, self.clip_mask_matrix_1)
            }
            if self.clip_mask_count > 2.5 {
                mask *= self.clip_mask_alpha(self.clip_mask_type_2, self.clip_mask_rect_2, self.clip_mask_radius_2, self.clip_mask_matrix_2)
            }
            if self.clip_mask_count > 3.5 {
                mask *= self.clip_mask_alpha(self.clip_mask_type_3, self.clip_mask_rect_3, self.clip_mask_radius_3, self.clip_mask_matrix_3)
            }
            return mask
        }

        vertex: fn() {
            let local = self.geom.pos * self.rect_size + self.rect_pos
            self.uv = self.uv_rect.xy + (self.uv_rect.zw - self.uv_rect.xy) * self.geom.pos
            let model_view = self.draw_list.view_transform * self.transform
            let world = model_view * vec4(
                local.x,
                local.y,
                self.draw_depth + self.draw_call.zbias,
                1.0
            )
            let view_pos = self.draw_pass.camera_view * world
            self.clip_space = self.draw_pass.camera_projection * view_pos
            self.vertex_pos = self.clip_space
        }

        pixel: fn() {
            if self.clip_projected() < 0.5 {
                discard()
            }
            let sampled = self.color_texture.sample_as_bgra(self.uv)
            let mask = self.accumulated_mask_alpha()
            let alpha = clamp(sampled.w * self.opacity * mask, 0.0, 1.0)
            let rgb = if self.premultiplied > 0.5 {
                sampled.xyz * self.opacity * mask
            } else {
                sampled.xyz * alpha
            }
            return vec4(rgb, alpha)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    mod.draw.DrawProjectedCornerQuad = mod.std.set_type_default() do #(DrawProjectedCornerQuad::script_shader(vm)){
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
        clip_mask_count: uniform(float(0.0))
        clip_mask_type_0: uniform(float(0.0))
        clip_mask_type_1: uniform(float(0.0))
        clip_mask_type_2: uniform(float(0.0))
        clip_mask_type_3: uniform(float(0.0))
        clip_mask_rect_0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_rect_1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_rect_2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_rect_3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_radius_0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_radius_1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_radius_2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_radius_3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_matrix_0: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        clip_mask_matrix_1: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        clip_mask_matrix_2: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        clip_mask_matrix_3: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        corner_0: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        corner_1: uniform(vec4(1.0, 0.0, 0.0, 1.0))
        corner_2: uniform(vec4(0.0, 1.0, 0.0, 1.0))
        corner_3: uniform(vec4(1.0, 1.0, 0.0, 1.0))

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

        rect_mask_alpha: fn(rect: vec4, local: vec2f) -> float {
            if local.x < rect.x || local.y < rect.y || local.x > rect.x + rect.z || local.y > rect.y + rect.w {
                return 0.0
            }
            return 1.0
        }

        rounded_mask_alpha: fn(rect: vec4, radius: vec4, local: vec2f) -> float {
            if self.rect_mask_alpha(rect, local) < 0.5 {
                return 0.0
            }
            let min_x = rect.x
            let min_y = rect.y
            let max_x = rect.x + rect.z
            let max_y = rect.y + rect.w
            let tl = max(radius.x, 0.0)
            if tl > 0.0 && local.x < min_x + tl && local.y < min_y + tl {
                let d = local - vec2(min_x + tl, min_y + tl)
                if length(d) <= tl { return 1.0 }
                return 0.0
            }
            let tr = max(radius.y, 0.0)
            if tr > 0.0 && local.x > max_x - tr && local.y < min_y + tr {
                let d = local - vec2(max_x - tr, min_y + tr)
                if length(d) <= tr { return 1.0 }
                return 0.0
            }
            let br = max(radius.z, 0.0)
            if br > 0.0 && local.x > max_x - br && local.y > max_y - br {
                let d = local - vec2(max_x - br, max_y - br)
                if length(d) <= br { return 1.0 }
                return 0.0
            }
            let bl = max(radius.w, 0.0)
            if bl > 0.0 && local.x < min_x + bl && local.y > max_y - bl {
                let d = local - vec2(min_x + bl, max_y - bl)
                if length(d) <= bl { return 1.0 }
                return 0.0
            }
            return 1.0
        }

        clip_mask_alpha: fn(mask_type: float, rect: vec4, radius: vec4, mask_matrix: mat4x4f) -> float {
            let local_h = mask_matrix * self.clip_space
            if abs(local_h.w) <= 0.000001 {
                return 0.0
            }
            let local = local_h.xy / local_h.w
            if mask_type < 0.5 {
                return 1.0
            }
            if mask_type < 1.5 {
                return self.rounded_mask_alpha(rect, radius, local)
            }
            return self.rect_mask_alpha(rect, local)
        }

        accumulated_mask_alpha: fn() -> float {
            var mask = 1.0
            if self.clip_mask_count > 0.5 {
                mask *= self.clip_mask_alpha(self.clip_mask_type_0, self.clip_mask_rect_0, self.clip_mask_radius_0, self.clip_mask_matrix_0)
            }
            if self.clip_mask_count > 1.5 {
                mask *= self.clip_mask_alpha(self.clip_mask_type_1, self.clip_mask_rect_1, self.clip_mask_radius_1, self.clip_mask_matrix_1)
            }
            if self.clip_mask_count > 2.5 {
                mask *= self.clip_mask_alpha(self.clip_mask_type_2, self.clip_mask_rect_2, self.clip_mask_radius_2, self.clip_mask_matrix_2)
            }
            if self.clip_mask_count > 3.5 {
                mask *= self.clip_mask_alpha(self.clip_mask_type_3, self.clip_mask_rect_3, self.clip_mask_radius_3, self.clip_mask_matrix_3)
            }
            return mask
        }

        vertex: fn() {
            let local_uv = self.geom.pos
            self.uv = self.uv_rect.xy + (self.uv_rect.zw - self.uv_rect.xy) * local_uv
            let top = mix(self.corner_0, self.corner_1, local_uv.x)
            let bottom = mix(self.corner_2, self.corner_3, local_uv.x)
            self.clip_space = mix(top, bottom, local_uv.y)
            self.vertex_pos = self.clip_space
        }

        pixel: fn() {
            if self.clip_projected() < 0.5 {
                discard()
            }
            let sampled = self.color_texture.sample_as_bgra(self.uv)
            let mask = self.accumulated_mask_alpha()
            let alpha = clamp(sampled.w * self.opacity * mask, 0.0, 1.0)
            let rgb = if self.premultiplied > 0.5 {
                sampled.xyz * self.opacity * mask
            } else {
                sampled.xyz * alpha
            }
            return vec4(rgb, alpha)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    mod.draw.DrawMaskedProjectiveQuad = mod.std.set_type_default() do #(DrawMaskedProjectiveQuad::script_shader(vm)){
        ..mod.draw.DrawQuad
        depth_write: true
        backface_culling: false
        color_texture: texture_2d(float)
        u_has_color_texture: uniform(float(0.0))
        u_transform_matrix: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        u_perspective_matrix: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        u_opacity: uniform(float(1.0))
        u_projective_transform: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        clip_plane_count: uniform(float(0.0))
        clip_plane_0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_7: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_count: uniform(float(0.0))
        clip_mask_type_0: uniform(float(0.0))
        clip_mask_type_1: uniform(float(0.0))
        clip_mask_type_2: uniform(float(0.0))
        clip_mask_type_3: uniform(float(0.0))
        clip_mask_rect_0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_rect_1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_rect_2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_rect_3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_radius_0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_radius_1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_radius_2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_radius_3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_matrix_0: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        clip_mask_matrix_1: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        clip_mask_matrix_2: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        clip_mask_matrix_3: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))

        v_uv: varying(vec2f)
        v_clip_space: varying(vec4f)

        clip_projected: fn() -> float {
            if self.clip_plane_count > 0.5 && dot(self.v_clip_space, self.clip_plane_0) < 0.0 { return 0.0 }
            if self.clip_plane_count > 1.5 && dot(self.v_clip_space, self.clip_plane_1) < 0.0 { return 0.0 }
            if self.clip_plane_count > 2.5 && dot(self.v_clip_space, self.clip_plane_2) < 0.0 { return 0.0 }
            if self.clip_plane_count > 3.5 && dot(self.v_clip_space, self.clip_plane_3) < 0.0 { return 0.0 }
            if self.clip_plane_count > 4.5 && dot(self.v_clip_space, self.clip_plane_4) < 0.0 { return 0.0 }
            if self.clip_plane_count > 5.5 && dot(self.v_clip_space, self.clip_plane_5) < 0.0 { return 0.0 }
            if self.clip_plane_count > 6.5 && dot(self.v_clip_space, self.clip_plane_6) < 0.0 { return 0.0 }
            if self.clip_plane_count > 7.5 && dot(self.v_clip_space, self.clip_plane_7) < 0.0 { return 0.0 }
            return 1.0
        }

        rect_mask_alpha: fn(rect: vec4, local: vec2f) -> float {
            if local.x < rect.x || local.y < rect.y || local.x > rect.x + rect.z || local.y > rect.y + rect.w {
                return 0.0
            }
            return 1.0
        }

        rounded_mask_alpha: fn(rect: vec4, radius: vec4, local: vec2f) -> float {
            if self.rect_mask_alpha(rect, local) < 0.5 {
                return 0.0
            }
            let min_x = rect.x
            let min_y = rect.y
            let max_x = rect.x + rect.z
            let max_y = rect.y + rect.w
            let tl = max(radius.x, 0.0)
            if tl > 0.0 && local.x < min_x + tl && local.y < min_y + tl {
                let d = local - vec2(min_x + tl, min_y + tl)
                if length(d) <= tl { return 1.0 }
                return 0.0
            }
            let tr = max(radius.y, 0.0)
            if tr > 0.0 && local.x > max_x - tr && local.y < min_y + tr {
                let d = local - vec2(max_x - tr, min_y + tr)
                if length(d) <= tr { return 1.0 }
                return 0.0
            }
            let br = max(radius.z, 0.0)
            if br > 0.0 && local.x > max_x - br && local.y > max_y - br {
                let d = local - vec2(max_x - br, max_y - br)
                if length(d) <= br { return 1.0 }
                return 0.0
            }
            let bl = max(radius.w, 0.0)
            if bl > 0.0 && local.x < min_x + bl && local.y > max_y - bl {
                let d = local - vec2(min_x + bl, max_y - bl)
                if length(d) <= bl { return 1.0 }
                return 0.0
            }
            return 1.0
        }

        clip_mask_alpha: fn(mask_type: float, rect: vec4, radius: vec4, mask_matrix: mat4x4f) -> float {
            let local_h = mask_matrix * self.v_clip_space
            if abs(local_h.w) <= 0.000001 {
                return 0.0
            }
            let local = local_h.xy / local_h.w
            if mask_type < 0.5 {
                return 1.0
            }
            if mask_type < 1.5 {
                return self.rounded_mask_alpha(rect, radius, local)
            }
            return self.rect_mask_alpha(rect, local)
        }

        accumulated_mask_alpha: fn() -> float {
            var mask = 1.0
            if self.clip_mask_count > 0.5 {
                mask *= self.clip_mask_alpha(self.clip_mask_type_0, self.clip_mask_rect_0, self.clip_mask_radius_0, self.clip_mask_matrix_0)
            }
            if self.clip_mask_count > 1.5 {
                mask *= self.clip_mask_alpha(self.clip_mask_type_1, self.clip_mask_rect_1, self.clip_mask_radius_1, self.clip_mask_matrix_1)
            }
            if self.clip_mask_count > 2.5 {
                mask *= self.clip_mask_alpha(self.clip_mask_type_2, self.clip_mask_rect_2, self.clip_mask_radius_2, self.clip_mask_matrix_2)
            }
            if self.clip_mask_count > 3.5 {
                mask *= self.clip_mask_alpha(self.clip_mask_type_3, self.clip_mask_rect_3, self.clip_mask_radius_3, self.clip_mask_matrix_3)
            }
            return mask
        }

        vertex: fn() {
            let uv = self.geom.pos
            self.pos = uv
            self.v_uv = uv

            let local = vec4(
                uv.x * self.rect_size.x,
                uv.y * self.rect_size.y,
                0.0,
                1.0
            )
            let transformed = self.u_transform_matrix * local
            let projected = self.u_perspective_matrix * transformed
            let screen_shift = self.rect_pos
            let screen_h = vec4(
                screen_shift.x * projected.w + projected.x,
                screen_shift.y * projected.w + projected.y,
                projected.z,
                projected.w
            )
            self.v_clip_space = self.projective_vertex(screen_h, self.u_projective_transform)
            self.vertex_pos = self.v_clip_space
        }

        pixel: fn() {
            let uv = clamp(self.v_uv, vec2(0.0, 0.0), vec2(1.0, 1.0))
            let mask = self.accumulated_mask_alpha()
            if self.u_has_color_texture > 0.5 {
                return self.color_texture.sample_as_bgra(uv) * (self.u_opacity * mask)
            }
            return vec4(1.0, 0.0, 1.0, mask)
        }

        fragment: fn() {
            if self.clip_projected() < 0.5 {
                discard()
            }
            self.fb0 = self.pixel()
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawProjectedQuad {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub transform: Mat4f,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub uv_rect: Vec4f,
    #[live(1.0)]
    pub opacity: f32,
    #[live(1.0)]
    pub premultiplied: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawProjectedCornerQuad {
    #[deref]
    pub draw_super: DrawQuad,
    #[live(vec4(0.0, 0.0, 1.0, 1.0))]
    pub uv_rect: Vec4f,
    #[live(1.0)]
    pub opacity: f32,
    #[live(1.0)]
    pub premultiplied: f32,
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawMaskedProjectiveQuad {
    #[rust(0.0)]
    pub has_color_texture: f32,
    #[rust(Mat4f::identity())]
    pub transform_matrix: Mat4f,
    #[rust(Mat4f::identity())]
    pub perspective_matrix: Mat4f,
    #[deref]
    pub draw_super: DrawQuad,
    #[live(1.0)]
    pub opacity: f32,
}

impl DrawProjectedQuad {
    pub fn draw(&mut self, cx: &mut Cx2d) {
        self.draw_super.draw_vars.append_group_id = cx.draw_call_group_background().0;
        if self.draw_super.draw_vars.can_instance() {
            let new_area = cx.add_instance(&self.draw_super.draw_vars);
            self.draw_super.draw_vars.area =
                cx.update_area_refs(self.draw_super.draw_vars.area, new_area);
        }
    }
}

impl DrawProjectedCornerQuad {
    pub fn draw(&mut self, cx: &mut Cx2d) {
        self.draw_super.draw_vars.append_group_id = cx.draw_call_group_background().0;
        if self.draw_super.draw_vars.can_instance() {
            let new_area = cx.add_instance(&self.draw_super.draw_vars);
            self.draw_super.draw_vars.area =
                cx.update_area_refs(self.draw_super.draw_vars.area, new_area);
        }
    }
}

impl DrawMaskedProjectiveQuad {
    fn apply_draw_uniforms(&mut self, cx: &mut Cx2d) {
        self.draw_super
            .draw_vars
            .set_uniform(cx.cx, live_id!(u_has_color_texture), &[self.has_color_texture]);
        self.draw_super.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_transform_matrix),
            &self.transform_matrix.v,
        );
        self.draw_super.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_perspective_matrix),
            &self.perspective_matrix.v,
        );
        self.draw_super
            .draw_vars
            .set_uniform(cx.cx, live_id!(u_opacity), &[self.opacity]);
        self.draw_super.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_projective_transform),
            &Mat4f::identity().v,
        );
    }

    pub fn set_texture(&mut self, texture: Option<Texture>) {
        self.has_color_texture = if texture.is_some() { 1.0 } else { 0.0 };
        self.draw_super.draw_vars.texture_slots[0] = texture;
    }

    pub fn set_matrices(&mut self, transform_matrix: Mat4f, perspective_matrix: Mat4f) {
        self.transform_matrix = transform_matrix;
        self.perspective_matrix = perspective_matrix;
    }

    pub fn draw_abs(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.draw_super.rect_pos = rect.pos.into();
        self.draw_super.rect_size = rect.size.into();
        self.draw(cx);
    }

    pub fn draw(&mut self, cx: &mut Cx2d) {
        self.apply_draw_uniforms(cx);
        self.draw_super.draw(cx);
    }
}
