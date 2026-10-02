//! The engine's own material programs, written in Splash: the hook-aware
//! lighting composition, the Unlit kind, the error material and the IBL
//! lighting feature. Each is a set of method overrides a program installs on
//! the Pbr lane (see `pbr.rs` in makepad-render for the call sites).
//!
//! The lane's contract these rely on: `mat_compose(albedo, metal, a, s, l,
//! ss, sa, sl, n, ldir, v, sun_rad, f, lobe)` builds the lit colour from the
//! ambient diffuse (a), sun diffuse (s), legacy lamp diffuse (l), sun
//! specular (ss), ambient specular (sa), clustered-light total (sl), the
//! normal, the sun direction, the view direction, the sun's radiance, its
//! Fresnel term and its specular lobe; `mat_light`, `mat_lighting` and
//! `mat_ambient` exist with stock bodies; `params` is the material's vec4;
//! `v_csm` is the true world position; `detail_map` is free on the lane.
use makepad_draw::*;
use makepad_draw::makepad_platform::makepad_script::script_eval;

script_mod! {
    use mod.prelude.widgets_internal.*

    // Light through the hooks: the sun and every clustered light pass
    // `light` (the clustered ones inside cluster_light), and the sums pass
    // `lighting`. Installed only on programs with either hook, so a
    // material without them keeps the stock composition bit for bit.
    mod.draw.mat_compose_hooked = fn(albedo: vec3, metal: float, a: vec3, s: vec3, l: vec3, ss: vec3, sa: vec3, sl: vec3, n: vec3, ldir: vec3, v: vec3, sun_rad: vec3, f: vec3, lobe: float) -> vec3 {
        let ndl = max(dot(n, ldir), 0.0)
        let brdf = albedo * ((1.0 - metal) * ndl) + f * (lobe * ndl)
        let direct = self.mat_light(sun_rad, ldir, n, v, brdf) + albedo * ((1.0 - metal) * l) + sl
        let ambient = albedo * ((1.0 - metal) * a) + sa
        return self.mat_lighting(direct, ambient)
    }

    // Unlit: the surface colour lifted by params.w stops of HDR intensity
    // (0 = as authored), with no light, shadow or reflection. Lines, neon,
    // cards, UI in the world.
    mod.draw.mat_compose_unlit = fn(albedo: vec3, metal: float, a: vec3, s: vec3, l: vec3, ss: vec3, sa: vec3, sl: vec3, n: vec3, ldir: vec3, v: vec3, sun_rad: vec3, f: vec3, lobe: float) -> vec3 {
        return albedo * exp2(clamp(self.params.w, -16.0, 16.0))
    }

    // The error material: a hatched magenta surface, unlit, so a material
    // that failed to compile is unmistakable in a preview.
    mod.draw.mat_error_surface = fn(base: vec4) -> vec4 {
        let p = self.v_csm.xyz
        let s = step(0.5, fract((p.x + p.y + p.z) * 4.0))
        return vec4(mix(vec3(1.0, 0.0, 1.0), vec3(0.12, 0.0, 0.12), s), 1.0)
    }
    mod.draw.mat_compose_flat = fn(albedo: vec3, metal: float, a: vec3, s: vec3, l: vec3, ss: vec3, sa: vec3, sl: vec3, n: vec3, ldir: vec3, v: vec3, sun_rad: vec3, f: vec3, lobe: float) -> vec3 {
        return albedo
    }

    // ---- IBL (ibl.rs builds the texture) ------------------------------
    // detail_map holds one meta row: texels 0..8 the SH9 irradiance
    // coefficients, texel 9 = (levels, level_height, intensity, rotation in
    // radians); then the prefiltered atlas: `levels` equirect levels of
    // `level_height` rows (roughness k / (levels - 1)). Rows count from the
    // top: size() is the allocation, which a backend may make taller than
    // the rows it holds.
    mod.draw.mat_ibl_meta = fn(i: float) -> vec4 {
        let size = self.detail_map.size()
        return self.detail_map.sample_nearest(vec2((i + 0.5) / size.x, 0.5 / size.y))
    }
    mod.draw.mat_ibl_dir = fn(d: vec3) -> vec3 {
        let r = self.mat_ibl_meta(9.0).w
        let c = cos(r)
        let s = sin(r)
        return vec3(d.x * c - d.z * s, d.y, d.x * s + d.z * c)
    }
    // One atlas level, bilinear by hand (float textures are unfiltered).
    mod.draw.mat_ibl_level = fn(uv: vec2, k: float) -> vec3 {
        let size = self.detail_map.size()
        let meta = self.mat_ibl_meta(9.0)
        let w = size.x
        let h = meta.y
        let px = uv.x * w - 0.5
        let py = clamp(uv.y * h - 0.5, 0.0, h - 1.0)
        let x0 = floor(px)
        let y0 = floor(py)
        let fx = px - x0
        let fy = py - y0
        let xa = (x0 - floor(x0 / w) * w + 0.5) / w
        let xb = (x0 + 1.0 - floor((x0 + 1.0) / w) * w + 0.5) / w
        let ya = (1.0 + k * h + y0 + 0.5) / size.y
        let yb = (1.0 + k * h + min(y0 + 1.0, h - 1.0) + 0.5) / size.y
        let a = mix(self.detail_map.sample_nearest(vec2(xa, ya)).xyz, self.detail_map.sample_nearest(vec2(xb, ya)).xyz, fx)
        let b = mix(self.detail_map.sample_nearest(vec2(xa, yb)).xyz, self.detail_map.sample_nearest(vec2(xb, yb)).xyz, fx)
        return mix(a, b, fy)
    }
    // Prefiltered specular radiance along r at this roughness: replaces the
    // analytic sky reflection.
    mod.draw.mat_ibl_sky_env = fn(r: vec3, rough: float) -> vec3 {
        let meta = self.mat_ibl_meta(9.0)
        let d = self.mat_ibl_dir(r)
        let uv = vec2(fract(0.5 + atan2(d.x, 0.0 - d.z) / 6.2831853), acos(clamp(d.y, -1.0, 1.0)) / 3.14159265)
        let lf = clamp(rough, 0.0, 1.0) * (meta.x - 1.0)
        let k0 = floor(lf)
        let k1 = min(k0 + 1.0, meta.x - 1.0)
        return mix(self.mat_ibl_level(uv, k0), self.mat_ibl_level(uv, k1), lf - k0) * meta.z
    }
    // SH9 irradiance for the ambient diffuse term (in the lane's
    // irradiance/pi units).
    mod.draw.mat_ibl_ambient = fn(n0: vec3, a: vec3) -> vec3 {
        let n = self.mat_ibl_dir(n0)
        var e = self.mat_ibl_meta(0.0).xyz * 0.282095
        e = e + self.mat_ibl_meta(1.0).xyz * (0.488603 * n.y)
        e = e + self.mat_ibl_meta(2.0).xyz * (0.488603 * n.z)
        e = e + self.mat_ibl_meta(3.0).xyz * (0.488603 * n.x)
        e = e + self.mat_ibl_meta(4.0).xyz * (1.092548 * n.x * n.y)
        e = e + self.mat_ibl_meta(5.0).xyz * (1.092548 * n.y * n.z)
        e = e + self.mat_ibl_meta(6.0).xyz * (0.315392 * (3.0 * n.z * n.z - 1.0))
        e = e + self.mat_ibl_meta(7.0).xyz * (1.092548 * n.x * n.z)
        e = e + self.mat_ibl_meta(8.0).xyz * (0.546274 * (n.x * n.x - n.y * n.y))
        return max(e, vec3(0.0, 0.0, 0.0)) * (self.mat_ibl_meta(9.0).z / 3.14159265)
    }
}

/// The engine overrides a program can carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Builtin {
    /// Route the sun and the sums through `light` / `lighting`.
    HookedCompose,
    /// The Unlit kind: the base colour lifted by the host's stops
    /// (`params.w`).
    Unlit,
    /// Unlit lighting on a program whose `params` are the author's: the
    /// surface colour as it is.
    Flat,
    /// The error material (surface and composition).
    Error,
    /// Image-based lighting from the atlas in `detail_map`.
    Ibl,
}

/// Register the built-in programs' functions (idempotent per VM).
pub fn register(vm: &mut ScriptVm) {
    let draw = script_eval!(vm, { mod.draw });
    if let Some(draw) = draw.as_object() {
        let present = vm.bx.heap.value(draw, id!(mat_compose_hooked).into(), NoTrap);
        if present.as_object().is_some_and(|o| vm.bx.heap.is_fn(o)) {
            return;
        }
    }
    script_mod(vm);
}

/// The (method, function) overrides of a built-in, to pass as
/// `ProgramRequest::overrides`.
pub fn overrides(vm: &mut ScriptVm, builtin: Builtin) -> Vec<(LiveId, ScriptObject)> {
    register(vm);
    let f = |v: ScriptValue| v.as_object();
    let pairs: Vec<(LiveId, Option<ScriptObject>)> = match builtin {
        Builtin::HookedCompose => vec![(id!(mat_compose), f(script_eval!(vm, { mod.draw.mat_compose_hooked })))],
        Builtin::Unlit => vec![(id!(mat_compose), f(script_eval!(vm, { mod.draw.mat_compose_unlit })))],
        Builtin::Flat => vec![(id!(mat_compose), f(script_eval!(vm, { mod.draw.mat_compose_flat })))],
        Builtin::Error => vec![
            (id!(surface), f(script_eval!(vm, { mod.draw.mat_error_surface }))),
            (id!(mat_compose), f(script_eval!(vm, { mod.draw.mat_compose_flat }))),
        ],
        Builtin::Ibl => vec![
            (id!(mat_ibl_meta), f(script_eval!(vm, { mod.draw.mat_ibl_meta }))),
            (id!(mat_ibl_dir), f(script_eval!(vm, { mod.draw.mat_ibl_dir }))),
            (id!(mat_ibl_level), f(script_eval!(vm, { mod.draw.mat_ibl_level }))),
            (id!(sky_env), f(script_eval!(vm, { mod.draw.mat_ibl_sky_env }))),
            (id!(mat_ambient), f(script_eval!(vm, { mod.draw.mat_ibl_ambient }))),
        ],
    };
    pairs.into_iter().filter_map(|(id, o)| o.filter(|o| vm.bx.heap.is_fn(*o)).map(|o| (id, o))).collect()
}
