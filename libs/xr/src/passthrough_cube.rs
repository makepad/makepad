use makepad_widgets::{makepad_platform::*, Cx2d};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.DrawPassthroughCubeAtlas = mod.std.set_type_default() do #(DrawPassthroughCubeAtlas::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.PbrVertex, geom.PbrGeom)

        camera_texture: texture_video()
        previous_env_texture: texture_2d(float)
        v_uv: varying(vec2f)

        vertex: fn() {
            let world = vec4(
                self.geom.pos_nx.x,
                self.geom.pos_nx.y,
                self.geom.pos_nx.z,
                1.0
            );
            self.v_uv = self.geom.ny_nz_uv.zw;
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * world);
        }

        face_dir_from_atlas_uv: fn(atlas_uv: vec2f) -> vec3f {
            let clamped = clamp(atlas_uv, vec2(0.0, 0.0), vec2(0.999999, 0.999999));
            let scaled = clamped * vec2(3.0, 2.0);
            let face_col = floor(scaled.x);
            let face_row = floor(scaled.y);
            let face_uv = fract(scaled);
            let u = face_uv.x * 2.0 - 1.0;
            let v = face_uv.y * 2.0 - 1.0;

            if face_row < 0.5 {
                if face_col < 0.5 {
                    return normalize(vec3(1.0, -v, -u));
                }
                if face_col < 1.5 {
                    return normalize(vec3(-1.0, -v, u));
                }
                return normalize(vec3(u, 1.0, v));
            }

            if face_col < 0.5 {
                return normalize(vec3(u, -1.0, -v));
            }
            if face_col < 1.5 {
                return normalize(vec3(u, -v, 1.0));
            }
            return normalize(vec3(-u, -v, -1.0));
        }

        face_local_uv: fn(atlas_uv: vec2f) -> vec2f {
            let clamped = clamp(atlas_uv, vec2(0.0, 0.0), vec2(0.999999, 0.999999));
            return fract(clamped * vec2(3.0, 2.0));
        }

        project_camera_uv: fn(dir_world: vec3f) -> vec3f {
            let cam_right = normalize(self.camera_right);
            let cam_up = normalize(self.camera_up);
            let cam_forward = normalize(self.camera_forward);
            let cam_x = dot(dir_world, cam_right);
            let cam_y = dot(dir_world, cam_up);
            let cam_z = dot(dir_world, cam_forward);

            let aspect = max(self.source_size.x / max(self.source_size.y, 1.0), 1.0);
            let tan_half_y_base = tan(self.camera_fov_y_degrees * 0.5 * 0.01745329251);
            let projection_scale = max(self.camera_projection_scale, 0.0001);
            let tan_half_y = tan_half_y_base * projection_scale;
            let tan_half_x = tan_half_y * aspect;
            let safe_z = max(cam_z, 0.0001);
            let uv = vec2(
                0.5 + cam_x / (2.0 * tan_half_x * safe_z),
                0.5 - cam_y / (2.0 * tan_half_y * safe_z)
            ) + self.camera_center_offset_uv;
            return vec3(uv.x, uv.y, cam_z);
        }

        sample_camera_rgb: fn(coord: vec2f) -> vec3f {
            if self.camera_enabled <= 0.5 {
                return vec3(0.0, 0.0, 0.0);
            }

            let coord_90 = vec2(1.0 - coord.y, coord.x);
            let coord_180 = vec2(1.0 - coord.x, 1.0 - coord.y);
            let coord_270 = vec2(coord.y, 1.0 - coord.x);
            let is_90 = step(0.5, self.rotation_steps) * step(self.rotation_steps, 1.5);
            let is_180 = step(1.5, self.rotation_steps) * step(self.rotation_steps, 2.5);
            let is_270 = step(2.5, self.rotation_steps);
            let is_0 = 1.0 - is_90 - is_180 - is_270;
            let sample_coord = coord * is_0 + coord_90 * is_90 + coord_180 * is_180 + coord_270 * is_270;
            let sample = self.camera_texture.sample_video(sample_coord).xyz;

            let y = (sample.y * 255.0 - 16.0) / 219.0;
            let u = (sample.x * 255.0 - 128.0) / 224.0;
            let v = (sample.z * 255.0 - 128.0) / 224.0;
            let r = y + 1.8556 * u;
            let g = y - 0.1873 * u - 0.4681 * v;
            let b = y + 1.5748 * v;
            let exposure = max(self.camera_exposure, 0.0);
            return vec3(
                clamp(r * exposure, 0.0, 1.0),
                clamp(g * exposure, 0.0, 1.0),
                clamp(b * exposure, 0.0, 1.0)
            );
        }

        pixel: fn() {
            let atlas_uv = clamp(self.v_uv, vec2(0.0, 0.0), vec2(1.0, 1.0));
            let previous = self.previous_env_texture.sample_as_bgra(atlas_uv).xyz;
            if self.camera_enabled <= 0.5 {
                return vec4(previous, 1.0);
            }

            if self.bootstrap_mix > 0.5 {
                let bootstrap_uv = self.face_local_uv(atlas_uv);
                let camera = self.sample_camera_rgb(bootstrap_uv);
                return vec4(camera, 1.0);
            }

            let dir_world = self.face_dir_from_atlas_uv(atlas_uv);
            let projection = self.project_camera_uv(dir_world);
            let sample_uv = clamp(projection.xy, vec2(0.0, 0.0), vec2(1.0, 1.0));
            let edge_margin = min(
                min(sample_uv.x, 1.0 - sample_uv.x),
                min(sample_uv.y, 1.0 - sample_uv.y)
            );
            let visible = step(0.0, projection.z)
                * step(0.0, projection.x)
                * step(0.0, projection.y)
                * step(projection.x, 1.0)
                * step(projection.y, 1.0);
            let capture_weight = max(
                0.0,
                visible * smoothstep(0.01, 0.08, edge_margin) * clamp(self.update_strength, 0.0, 1.0)
            );
            let camera = self.sample_camera_rgb(sample_uv);
            return vec4(mix(previous, camera, capture_weight), 1.0);
        }

        fragment: fn() {
            self.fb0 = self.pixel();
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawPassthroughCubeAtlas {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub source_size: Vec2f,
    #[live]
    pub camera_enabled: f32,
    #[live]
    pub rotation_steps: f32,
    #[live]
    pub bootstrap_mix: f32,
    #[live]
    pub update_strength: f32,
    #[live]
    pub camera_fov_y_degrees: f32,
    #[live]
    pub camera_projection_scale: f32,
    #[live]
    pub camera_exposure: f32,
    #[live]
    pub camera_center_offset_uv: Vec2f,
    #[live]
    pub camera_right: Vec3f,
    #[live]
    pub camera_up: Vec3f,
    #[live]
    pub camera_forward: Vec3f,
}

impl DrawPassthroughCubeAtlas {
    pub fn draw_geometry(&mut self, cx: &mut Cx2d, geometry_id: GeometryId) {
        self.draw_vars.append_group_id = cx.draw_call_group_background().0;
        self.draw_vars.geometry_id = Some(geometry_id);
        if cx.new_draw_call(&self.draw_vars).is_some() && self.draw_vars.can_instance() {
            let new_area = cx.add_aligned_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
}
