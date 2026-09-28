//! Volumetric smoke clouds (smoke grenades, fire smoke): one camera-facing
//! quad per cloud whose pixels raymarch a noisy density sphere, lit by the
//! sun and the fill, darker at the core, opaque where it is dense.
//!
//! Hosts push the live clouds each frame with
//! [`crate::Renderer::set_smoke_volumes`]; the list is drawn in the late
//! transparent layer (depth-tested, never depth-written) and consumed.
//! Cost: 16 density samples per covered pixel — a cloud filling the screen
//! is one full-screen pass of cheap value noise.

use makepad_draw::*;

/// One cloud. `density` scales opacity (1 = the core fully blocks sight),
/// `age` in seconds drives the billowing, `seed` decorrelates clouds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SmokeVolume {
    pub center: Vec3f,
    pub radius: f32,
    pub density: f32,
    pub age: f32,
    pub seed: f32,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    mod.draw.DrawSceneSmoke = mod.std.set_type_default() do #(DrawSceneSmoke::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        world: varying(vec4f)
        alpha_blend: true
        backface_culling: false
        depth_write: false
        // TRUE world camera, sun and fill (the frame's SunLight), fog.
        eye: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        light_dir: uniform(vec3(0.35, 0.8, 0.45))
        sun_color: uniform(vec3(0.72, 0.72, 0.72))
        sun_sky: uniform(vec3(0.28, 0.28, 0.28))
        fog_color: uniform(vec3(0.75, 0.87, 0.96))
        fog_density: uniform(0.0)
        // Camera right/up in true world space (the billboard axes).
        cam_right: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        cam_up: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        v_ray: varying(vec3f)

        vertex: fn() {
            let c = self.smoke_a.xyz
            let r = self.smoke_a.w
            let to_c = c - self.eye.xyz
            let d = max(length(to_c), 0.0001)
            let fwd = to_c / d
            // Pull the quad to the sphere's near side so the world occludes
            // it correctly; from inside, it hangs just in front of the eye.
            let near = max(d - r, 0.15)
            var half = near * 8.0
            if d > r * 1.02 {
                half = near * r / sqrt(d * d - r * r) * 1.05
            }
            let corner = self.geom.geom_pos
            let wp = self.eye.xyz + fwd * near + self.cam_right.xyz * (corner.x * half * 2.0) + self.cam_up.xyz * (corner.y * half * 2.0)
            self.v_ray = wp - self.eye.xyz
            self.world = self.draw_list.view_transform * vec4(wp.x, wp.y, wp.z, 1.0)
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
            return self.vertex_pos
        }

        hash3: fn(p: vec3) -> float {
            return fract(sin(dot(p, vec3(127.1, 311.7, 74.7))) * 43758.5453)
        }
        noise3: fn(p: vec3) -> float {
            let i = floor(p)
            let f = fract(p)
            let u = f * f * (vec3(3.0, 3.0, 3.0) - f * 2.0)
            let a = mix(self.hash3(i), self.hash3(i + vec3(1.0, 0.0, 0.0)), u.x)
            let b = mix(self.hash3(i + vec3(0.0, 1.0, 0.0)), self.hash3(i + vec3(1.0, 1.0, 0.0)), u.x)
            let c = mix(self.hash3(i + vec3(0.0, 0.0, 1.0)), self.hash3(i + vec3(1.0, 0.0, 1.0)), u.x)
            let d = mix(self.hash3(i + vec3(0.0, 1.0, 1.0)), self.hash3(i + vec3(1.0, 1.0, 1.0)), u.x)
            return mix(mix(a, b, u.y), mix(c, d, u.y), u.z)
        }
        density_at: fn(p: vec3) -> float {
            let c = self.smoke_a.xyz
            let r = self.smoke_a.w
            let q = (p - c) / r
            let fall = clamp(1.0 - dot(q, q), 0.0, 1.0)
            // Billowing: two octaves drifting up and turning with age.
            let t = self.smoke_b.y
            let s = (p - c) * (2.2 / r) + vec3(self.smoke_b.z * 7.0, 0.0 - t * 0.35, self.smoke_b.z * 3.0)
            let n = self.noise3(s) * 0.65 + self.noise3(s * 2.1 + vec3(t * 0.2, 0.0, 0.0)) * 0.35
            return max(fall * 1.6 - (1.0 - n) * 0.9, 0.0) * self.smoke_b.x
        }

        pixel: fn() {
            let dir = normalize(self.v_ray)
            let o = self.eye.xyz
            let c = self.smoke_a.xyz
            let r = self.smoke_a.w
            let oc = o - c
            let b = dot(oc, dir)
            let h = b * b - (dot(oc, oc) - r * r)
            if h <= 0.0 { discard() }
            let sq = sqrt(h)
            let t0 = max(0.0 - b - sq, 0.0)
            let t1 = 0.0 - b + sq
            if t1 <= t0 { discard() }
            let dt = (t1 - t0) / 16.0
            let l = normalize(self.light_dir)
            // Extinction per metre at unit density: a 3.4 m cloud's core
            // blocks sight completely.
            let sigma = 3.0 / max(r, 0.1)
            var trans = 1.0
            var light = vec3(0.0, 0.0, 0.0)
            var i = 0.0
            while i < 16.0 {
                let p = o + dir * (t0 + (i + 0.5) * dt)
                let dens = self.density_at(p)
                if dens > 0.001 {
                    // Self-shadow: one tap toward the sun, plus the core.
                    let toward = self.density_at(p + l * (r * 0.35))
                    let depth = length(p - c) / r
                    let lit = self.sun_color * (exp(0.0 - toward * sigma * r * 0.5) * 0.8)
                        + self.sun_sky * mix(0.55, 1.0, depth)
                    let a = 1.0 - exp(0.0 - dens * sigma * dt)
                    light = light + lit * (0.7 * a * trans)
                    trans = trans * (1.0 - a)
                }
                i = i + 1.0
            }
            let alpha = 1.0 - trans
            if alpha < 0.002 { discard() }
            // Distance fog over the cloud like any other surface.
            let fog = 1.0 - exp(0.0 - length(c - o) * self.fog_density)
            let rgb = mix(light, self.fog_color * alpha, fog)
            return vec4(rgb, alpha)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }
}

/// One smoke cloud draw (instance fields vec4-packed, see DrawSceneFlare).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSceneSmoke {
    #[deref]
    pub draw_vars: DrawVars,
    #[live(1.0)]
    pub depth_clip: f32,
    /// xyz = centre, w = radius.
    #[live(vec4(0.0, 0.0, 0.0, 1.0))]
    pub smoke_a: Vec4f,
    /// x = density, y = age, z = seed, w unused.
    #[live(vec4(1.0, 0.0, 0.0, 0.0))]
    pub smoke_b: Vec4f,
}
