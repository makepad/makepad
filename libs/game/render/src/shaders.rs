//! The five game draw shaders, moved verbatim from gamemaker's game_view.rs.
//! DrawGameTexture composites the offscreen 3D pass into the host pane; the
//! cube/alpha/sky/terrain family renders the world itself.

use makepad_draw::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    mod.draw.DrawGameTexture = mod.std.set_type_default() do #(DrawGameTexture::script_shader(vm)){
        ..mod.draw.DrawQuad
        scene_texture: texture_2d(float)

        pixel: fn() {
            let color = self.scene_texture.sample_as_bgra(self.pos)
            return Pal.premul(color)
        }
    }

    // The game cube: DrawCube + per-instance emission and distance fog.
    mod.draw.DrawGameCube = mod.std.set_type_default() do #(DrawGameCube::script_shader(vm)){
        ..mod.draw.DrawCube
        v_fog: varying(float)

        vertex: fn() {
            let pos = self.get_size() * self.geom.geom_pos + self.get_pos()
            let model_view = self.draw_list.view_transform * self.transform
            let normal4 = model_view * vec4(
                self.geom.geom_normal.x,
                self.geom.geom_normal.y,
                self.geom.geom_normal.z,
                0.0
            )
            let normal = normalize(normal4.xyz)
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            let dp = max(dot(normal, normalize(self.light_dir)), 0.0)
            self.lit_color = self.get_color(dp, normal.y)
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        // One lighting model for every game shader (sun.rs): hemisphere
        // ambient by surface-up-ness, plus the sun's direct term. With the
        // default flat sun (sky == ground == 0.28, color 0.72) this is
        // exactly the constant split the shaders each used to hardcode.
        get_color: fn(dp: float, nrm_y: float) {
            let hemi = clamp(nrm_y * 0.5 + 0.5, 0.0, 1.0)
            let ambient = mix(self.sun_ground, self.sun_sky, hemi)
            let lit = self.color.xyz * (ambient + self.sun_color * dp)
            // Emission: glowing eyes, beacons, bolts (energy ramps at runtime).
            let glowing = lit + self.color.xyz * self.glow * 0.6
            return vec4(glowing, self.color.w)
        }

        pixel: fn() {
            let fogged = mix(self.lit_color.xyz, self.fog_color, self.v_fog)
            return vec4(fogged, self.lit_color.w)
        }
    }

    // Same shading, alpha-blended: water, sensor ghosts, blob shadows.
    mod.draw.DrawGameAlpha = mod.std.set_type_default() do #(DrawGameAlpha::script_shader(vm)){
        ..mod.draw.DrawGameCube
        alpha_blend: true
        backface_culling: false
    }

    // Sky dome: a big cube around the camera, gradient by view direction
    // (the Godot ProceduralSkyMaterial look).
    mod.draw.DrawGameSky = mod.std.set_type_default() do #(DrawGameSky::script_shader(vm)){
        ..mod.draw.DrawCube
        backface_culling: false
        v_dir: varying(vec3f)

        vertex: fn() {
            let pos = self.get_size() * self.geom.geom_pos + self.get_pos()
            let model_view = self.draw_list.view_transform * self.transform
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            self.v_dir = self.geom.geom_pos
            let view_pos = self.draw_pass.camera_view * self.world
            let clip = self.draw_pass.camera_projection * view_pos
            // Pin the sky to the far plane (z ~= w) — the skybox trick Godot's
            // background pass amounts to: the dome never clips against the far
            // plane no matter its world size, and everything else wins depth.
            self.vertex_pos = vec4(clip.x, clip.y, clip.w * 0.99995, clip.w)
        }

        pixel: fn() {
            let y = normalize(self.v_dir).y
            let up = clamp(y * 2.2, 0.0, 1.0)
            let down = clamp((0.0 - y) * 2.2, 0.0, 1.0)
            let sky = mix(self.sky_horizon, self.sky_top, up)
            let ground = mix(self.sky_ground, self.sky_bottom, down)
            let color = mix(ground, sky, step(0.0, y))
            return vec4(color, 1.0)
        }
    }

    // Skinned character mesh: PbrVertex stream (CPU-skinned per frame, uv in
    // ny_nz_uv.zw), textured, lit and fogged like the terrain.
    mod.draw.DrawGameSkinned = mod.std.set_type_default() do #(DrawGameSkinned::script_shader(vm)){
        alpha_blend: false
        backface_culling: true
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.PbrVertex, geom.PbrGeom)
        tex: texture_2d(float)
        v_ambient: varying(vec3f)
        v_direct: varying(vec3f)
        v_uv: varying(vec2f)
        world: varying(vec4f)
        v_fog: varying(float)

        vertex: fn() {
            let pos = vec3(self.geom.pos_nx.x, self.geom.pos_nx.y, self.geom.pos_nx.z)
            let normal_in = vec3(self.geom.pos_nx.w, self.geom.ny_nz_uv.x, self.geom.ny_nz_uv.y)
            let model_view = self.draw_list.view_transform * self.transform
            let world_normal = normalize((model_view * vec4(normal_in.x, normal_in.y, normal_in.z, 0.0)).xyz)
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            let dp = max(dot(world_normal, normalize(self.light_dir)), 0.0)
            let hemi = clamp(world_normal.y * 0.5 + 0.5, 0.0, 1.0)
            self.v_ambient = mix(self.sun_ground, self.sun_sky, hemi)
            self.v_direct = self.sun_color * dp
            self.v_uv = vec2(self.geom.ny_nz_uv.z, self.geom.ny_nz_uv.w)
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        pixel: fn() {
            let albedo = self.tex.sample_as_bgra(self.v_uv)
            let lit = albedo.xyz * (self.v_ambient + self.v_direct)
            return vec4(mix(lit, self.fog_color, self.v_fog), 1.0)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    // The smooth terrain mesh: per-vertex colored triangles, flat normals.
    mod.draw.DrawGameTerrain = mod.std.set_type_default() do #(DrawGameTerrain::script_shader(vm)){
        alpha_blend: false
        backface_culling: true
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.PbrVertex, geom.PbrGeom)
        lit_color: varying(vec4f)
        world: varying(vec4f)
        v_fog: varying(float)

        vertex: fn() {
            let pos = vec3(self.geom.pos_nx.x, self.geom.pos_nx.y, self.geom.pos_nx.z)
            let normal_in = vec3(self.geom.pos_nx.w, self.geom.ny_nz_uv.x, self.geom.ny_nz_uv.y)
            let model_view = self.draw_list.view_transform * self.transform
            let world_normal = normalize((model_view * vec4(normal_in.x, normal_in.y, normal_in.z, 0.0)).xyz)
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            let dp = max(dot(world_normal, normalize(self.light_dir)), 0.0)
            let hemi = clamp(world_normal.y * 0.5 + 0.5, 0.0, 1.0)
            let ambient = mix(self.sun_ground, self.sun_sky, hemi)
            self.lit_color = vec4(
                self.geom.color.xyz * (ambient + self.sun_color * dp),
                self.geom.color.w
            )
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        pixel: fn() {
            return vec4(mix(self.lit_color.xyz, self.fog_color, self.v_fog), self.lit_color.w)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawGameTexture {
    #[deref]
    pub draw_super: DrawQuad,
}

/// DrawCube + per-instance emission (`glow`) and per-instance fog params.
/// Instance-field rule: only #[live] instance fields after the deref chain.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGameCube {
    #[deref]
    pub cube: DrawCube,
    #[live(0.0)]
    pub glow: f32,
    #[live(vec3(0.75, 0.87, 0.96))]
    pub fog_color: Vec3f,
    #[live(0.0)]
    pub fog_density: f32,
    /// Sun terms, written every frame from one [`crate::sun::GameSun`].
    /// Defaults reproduce the legacy hardcoded 0.28/0.72 split.
    #[live(vec3(0.72, 0.72, 0.72))]
    pub sun_color: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_sky: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_ground: Vec3f,
}

/// Alpha-blended variant: water, sensor ghosts, blob shadows.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGameAlpha {
    #[deref]
    pub cube: DrawGameCube,
}

/// Sky dome gradient (colors are instances so Rust sets them per frame).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGameSky {
    #[deref]
    pub cube: DrawCube,
    #[live(vec3(0.32, 0.58, 0.9))]
    pub sky_top: Vec3f,
    #[live(vec3(0.75, 0.87, 0.96))]
    pub sky_horizon: Vec3f,
    #[live(vec3(0.68, 0.75, 0.66))]
    pub sky_ground: Vec3f,
    #[live(vec3(0.3, 0.4, 0.3))]
    pub sky_bottom: Vec3f,
}

/// Skinned character mesh (PbrVertex layout, uv in ny_nz_uv.zw, textured).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGameSkinned {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(1.0)]
    pub depth_clip: f32,
    #[live(vec3(0.35, 0.8, 0.45))]
    pub light_dir: Vec3f,
    #[live(vec3(0.75, 0.87, 0.96))]
    pub fog_color: Vec3f,
    #[live(0.0)]
    pub fog_density: f32,
    /// Sun terms, written every frame from one [`crate::sun::GameSun`].
    #[live(vec3(0.72, 0.72, 0.72))]
    pub sun_color: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_sky: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_ground: Vec3f,
}

/// The smooth terrain mesh (PbrVertex layout: per-vertex color).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGameTerrain {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(1.0)]
    pub depth_clip: f32,
    #[live(vec3(0.35, 0.8, 0.45))]
    pub light_dir: Vec3f,
    #[live(vec3(0.75, 0.87, 0.96))]
    pub fog_color: Vec3f,
    #[live(0.0)]
    pub fog_density: f32,
    /// Sun terms, written every frame from one [`crate::sun::GameSun`].
    #[live(vec3(0.72, 0.72, 0.72))]
    pub sun_color: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_sky: Vec3f,
    #[live(vec3(0.28, 0.28, 0.28))]
    pub sun_ground: Vec3f,
}
