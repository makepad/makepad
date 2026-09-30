//! Splash passes: a fullscreen program written in the shader DSL, compiled
//! at runtime (no recompile of the app), reading named resources.
//!
//! A document writes
//!
//! ```text
//! Pass{
//!     at: @hdr                      // @hdr | @display | @final
//!     name: "soft"                  // optional: later passes read it by name
//!     reads: [@color @glow]         // texture slots, self.color / self.glow
//!     scale: 0.5                    // output size relative to the frame
//!     uniforms: {amount: 0.4 tint: #ff8844}
//!     pixel: "fn() -> vec4 { return self.color.sample(self.uv()) * self.amount }"
//! }
//! ```
//!
//! and the host turns it into a [`PassDecl`]. [`PassDecl::source`] makes
//! the Splash shader text: a subclass of `DrawGraphPass` (the standard
//! block below) with one `texture_2d` per read, one `uniform` per value and
//! the author's `pixel`. [`Programs`] compiles each distinct source once.
//!
//! The standard block every pass has:
//! * `self.uv()`, `self.texel()` (one output texel in uv), `self.size()`
//!   (output pixels), `self.aspect()`;
//! * `self.time()` (the canonical time of the frame, or of the frame point
//!   for `@final` passes), `self.frame()` (the frame index),
//!   `self.frame_hash(i)` (a per-frame random in 0..1, constant over the
//!   shutter), `self.ss_tap()` (the supersampling tap of this sub-frame, or
//!   -1), `self.exposure()`, `self.view_depth(d)` (the view distance of a
//!   depth sample);
//! * `self.luma(c)`, `self.to_srgb(c)`, `self.from_srgb(c)`,
//!   `self.hash(p)`.

use crate::plan::{Format, ProgramId, Resource, Stage};
use makepad_draw::*;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // The standard block of a graph pass: a fullscreen quad whose `pixel`
    // the author writes.
    mod.draw.DrawGraphPass = mod.std.set_type_default() do #(DrawGraphPass::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba16F
        depth_clip: 0.0
        // x = time (s), y = frame index, z = supersampling tap (-1 = none),
        // w = the frame's seed (0..1)
        g_frame: uniform(vec4(0.0, 0.0, -1.0, 0.0))
        // xy = output size in pixels, zw = one output texel in uv
        g_size: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        // x = exposure, y = aspect (w / h)
        g_misc: uniform(vec4(1.0, 1.0, 0.0, 0.0))
        // The camera for depth reads: proj[10], proj[14], orthographic,
        // depth stores clip z / w directly (else z * 0.5 + 0.5).
        g_cam: uniform(vec4(-1.0, -0.2, 0.0, 1.0))

        vertex: fn() {
            self.pos = self.geom.pos
            self.world = vec4(self.geom.pos.x, self.geom.pos.y, 0.0, 1.0)
            self.vertex_pos = vec4(self.geom.pos.x * 2.0 - 1.0, 1.0 - self.geom.pos.y * 2.0, 0.0, 1.0)
        }

        uv: fn() -> vec2 { return self.pos }
        texel: fn() -> vec2 { return self.g_size.zw }
        size: fn() -> vec2 { return self.g_size.xy }
        aspect: fn() -> float { return self.g_misc.y }
        time: fn() -> float { return self.g_frame.x }
        frame: fn() -> float { return self.g_frame.y }
        ss_tap: fn() -> float { return self.g_frame.z }
        exposure: fn() -> float { return self.g_misc.x }
        // The view distance of a depth-buffer value (a sample of a depth
        // read), for depth of field, outlines and fog passes.
        view_depth: fn(d: float) -> float {
            var z = d
            if self.g_cam.w < 0.5 {
                z = d * 2.0 - 1.0
            }
            if self.g_cam.z > 0.5 {
                return 0.0 - (z - self.g_cam.y) / self.g_cam.x
            }
            return self.g_cam.y / (z + self.g_cam.x)
        }
        luma: fn(c: vec3) -> float { return dot(c, vec3(0.2126, 0.7152, 0.0722)) }
        hash: fn(p: vec2) -> float {
            let q = fract(p * vec2(0.1031, 0.1030))
            let r = q + dot(q, q.yx + vec2(33.33, 33.33))
            return fract((r.x + r.y) * r.x)
        }
        // A random number per frame (and `i`), constant over the shutter.
        frame_hash: fn(i: float) -> float {
            return self.hash(vec2(self.g_frame.y * 0.618 + i * 1.7 + 0.5, self.g_frame.w * 97.0 + i * 0.37))
        }
        to_srgb: fn(c: vec3) -> vec3 {
            let x = max(c, vec3(0.0, 0.0, 0.0))
            return mix(x * 12.92, pow(x, vec3(1.0 / 2.4, 1.0 / 2.4, 1.0 / 2.4)) * 1.055 - vec3(0.055, 0.055, 0.055), step(vec3(0.0031308, 0.0031308, 0.0031308), x))
        }
        from_srgb: fn(c: vec3) -> vec3 {
            let x = max(c, vec3(0.0, 0.0, 0.0))
            return mix(x / 12.92, pow((x + vec3(0.055, 0.055, 0.055)) / 1.055, vec3(2.4, 2.4, 2.4)), step(vec3(0.04045, 0.04045, 0.04045), x))
        }

        pixel: fn() -> vec4f {
            return vec4(0.0, 0.0, 0.0, 1.0)
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGraphPass {
    #[deref]
    pub draw_super: DrawQuad,
}

/// One uniform a pass declares: its name and width (1..4 floats).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UniformDecl {
    pub name: String,
    pub width: u8,
}

/// A pass as the host read it from the document (uniform values are the
/// host's keyables, evaluated per frame into [`crate::runner::PassValues`]).
#[derive(Clone, Debug, PartialEq)]
pub struct PassDecl {
    pub name: Option<String>,
    pub stage: Stage,
    /// What each texture slot reads, in order: `color`, `depth`, `glow`,
    /// ..., a pass name, or a texture the host provides by name.
    pub reads: Vec<String>,
    /// The slots' names in the shader (`self.<slot>`); empty = the read's
    /// own name.
    pub slots: Vec<String>,
    pub scale: f32,
    pub format: Option<Format>,
    pub uniforms: Vec<UniformDecl>,
    /// The body of `pixel`: `fn() -> vec4 { ... }`.
    pub pixel: String,
    /// Extra shader functions the pixel calls (`name: fn(..) {..}` lines).
    pub helpers: String,
    /// For diagnostics.
    pub label: String,
}

/// A slot name must be a plain identifier; the standard block's names are
/// taken.
pub fn check_ident(name: &str) -> Result<(), String> {
    const TAKEN: &[&str] = &["uv", "texel", "size", "aspect", "time", "frame", "ss_tap", "exposure", "luma", "hash", "frame_hash", "to_srgb", "from_srgb", "view_depth", "g_cam", "pixel", "vertex", "pos", "world", "geom", "draw_call", "draw_pass", "draw_list", "g_frame", "g_size", "g_misc", "color_format", "depth_clip"];
    let ok = !name.is_empty()
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.len() <= 48;
    if !ok {
        return Err(format!("`{name}` is not a name (letters, digits and _)"));
    }
    if TAKEN.contains(&name) {
        return Err(format!("`{name}` is part of the pass's standard block; choose another name"));
    }
    Ok(())
}

impl PassDecl {
    /// The output format: the given one or the stage's default.
    pub fn format(&self) -> Format {
        self.format.unwrap_or(Format::default_for(self.stage))
    }

    /// The shader name of slot `i`.
    pub fn slot(&self, i: usize) -> &str {
        self.slots.get(i).filter(|s| !s.is_empty()).unwrap_or(&self.reads[i])
    }

    pub fn resources(&self) -> Vec<Resource> {
        self.reads.iter().map(|r| Resource::by_name(r)).collect()
    }

    /// Check names before compiling (the compiler's own errors come later,
    /// from [`Programs::compile`]).
    pub fn validate(&self) -> Result<(), String> {
        if !self.slots.is_empty() && self.slots.len() != self.reads.len() {
            return Err(format!("{} slot names for {} reads", self.slots.len(), self.reads.len()));
        }
        let mut seen: Vec<&str> = Vec::new();
        for i in 0..self.reads.len() {
            let r = self.slot(i);
            check_ident(r)?;
            if seen.contains(&r) {
                return Err(format!("slot `{r}` is read twice (name the slots with `slots: [...]`)"));
            }
            seen.push(r);
        }
        for u in &self.uniforms {
            check_ident(&u.name)?;
            if seen.contains(&u.name.as_str()) {
                return Err(format!("`{}` is both a read and a uniform", u.name));
            }
            if !(1..=4).contains(&u.width) {
                return Err(format!("uniform `{}` must be a number or a vector of up to 4", u.name));
            }
            seen.push(&u.name);
        }
        if let Some(n) = &self.name {
            check_ident(n)?;
        }
        if !self.pixel.trim_start().starts_with("fn") {
            return Err("`pixel` must be a shader function: \"fn() -> vec4 { ... }\"".into());
        }
        Ok(())
    }

    /// The Splash shader source of this pass.
    pub fn source(&self) -> String {
        let mut s = String::new();
        s.push_str("use mod.pod.*\nuse mod.math.*\nuse mod.shader.*\nuse mod.draw\nmod.draw.DrawGraphPass{\n");
        s.push_str(&format!("    color_format: {}\n", self.format().shader_color_format()));
        for i in 0..self.reads.len() {
            s.push_str(&format!("    {}: texture_2d(float)\n", self.slot(i)));
        }
        for u in &self.uniforms {
            let init = match u.width {
                1 => "0.0".to_string(),
                2 => "vec2(0.0, 0.0)".to_string(),
                3 => "vec3(0.0, 0.0, 0.0)".to_string(),
                _ => "vec4(0.0, 0.0, 0.0, 0.0)".to_string(),
            };
            s.push_str(&format!("    {}: uniform({init})\n", u.name));
        }
        if !self.helpers.trim().is_empty() {
            s.push_str(&self.helpers);
            s.push('\n');
        }
        s.push_str("    pixel: ");
        s.push_str(self.pixel.trim());
        s.push_str("\n}\n");
        s
    }

    pub fn program_id(&self) -> ProgramId {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.source().hash(&mut h);
        ProgramId(h.finish())
    }
}

/// Make a kit's pass names its own: every name the kit's passes write, and
/// every read of one, gets `prefix` (the kit's place in the list), so two
/// kits (or two of one kit) never collide. Reads of `color`, attachments
/// and names the kit does not write are left alone; slot names keep the
/// shader's view unchanged.
pub fn namespace(decls: &mut [PassDecl], prefix: &str) {
    let own: Vec<String> = decls.iter().filter_map(|d| d.name.clone()).collect();
    for d in decls.iter_mut() {
        if d.slots.is_empty() {
            d.slots = d.reads.clone();
        }
        for r in d.reads.iter_mut() {
            if own.contains(r) {
                *r = format!("{prefix}{r}");
            }
        }
        if let Some(n) = &mut d.name {
            *n = format!("{prefix}{n}");
        }
    }
}

/// Compiled pass programs by source hash. A program that failed keeps its
/// error, so a broken pass is reported once, not recompiled every frame.
#[derive(Default)]
pub struct Programs {
    compiled: HashMap<ProgramId, Result<Box<DrawGraphPass>, String>>,
}

impl Programs {
    /// Compile `decl`'s program (once). `Ok(None)` while the script VM is
    /// busy elsewhere (try again next frame).
    pub fn compile(&mut self, cx: &mut Cx, decl: &PassDecl) -> Result<Option<ProgramId>, String> {
        decl.validate().map_err(|e| format!("{}: {e}", decl.label))?;
        let id = decl.program_id();
        if let Some(done) = self.compiled.get(&id) {
            return done.as_ref().map(|_| Some(id)).map_err(|e| e.clone());
        }
        let code = decl.source();
        let label = decl.label.clone();
        let made = cx.try_with_vm(|vm| {
            vm.bx.captured_errors = Some(Vec::new());
            let v = vm.eval(ScriptMod { file: format!("graph://pass/{:016x}", id.0), code, ..Default::default() });
            let errors = vm.take_errors();
            if !errors.is_empty() || v.is_err() {
                return Err(format!("{label} did not compile: {}", errors.join("; ")));
            }
            let mut scope = Scope::default();
            let mut d = Box::new(DrawGraphPass::script_new_with_default(vm));
            d.script_apply(vm, &Apply::Eval, &mut scope, v);
            Ok(d)
        });
        let Some(made) = made else { return Ok(None) };
        let out = made.as_ref().map(|_| Some(id)).map_err(|e| e.clone());
        self.compiled.insert(id, made);
        out
    }

    pub fn get_mut(&mut self, id: ProgramId) -> Option<&mut DrawGraphPass> {
        self.compiled.get_mut(&id).and_then(|r| r.as_mut().ok()).map(|b| &mut **b)
    }

    /// Drop programs no longer used (after a document change).
    pub fn retain(&mut self, keep: &[ProgramId]) {
        self.compiled.retain(|k, _| keep.contains(k));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl() -> PassDecl {
        PassDecl {
            name: Some("soft".into()),
            stage: Stage::Hdr,
            reads: vec!["color".into(), "glow".into()],
            slots: Vec::new(),
            scale: 0.5,
            format: None,
            uniforms: vec![UniformDecl { name: "amount".into(), width: 1 }, UniformDecl { name: "tint".into(), width: 4 }],
            pixel: "fn() -> vec4 { return self.color.sample(self.uv()) * self.amount }".into(),
            helpers: String::new(),
            label: "Pass".into(),
        }
    }

    #[test]
    fn source_declares_slots_and_uniforms_in_order() {
        let s = decl().source();
        let color = s.find("color: texture_2d(float)").unwrap();
        let glow = s.find("glow: texture_2d(float)").unwrap();
        assert!(color < glow);
        assert!(s.contains("amount: uniform(0.0)"));
        assert!(s.contains("tint: uniform(vec4(0.0, 0.0, 0.0, 0.0))"));
        assert!(s.contains("color_format: @Rgba16F"));
        let mut d = decl();
        d.stage = Stage::Final;
        assert!(d.source().contains("color_format: @Bgra8NoBlend"));
        assert_ne!(d.program_id(), decl().program_id());
    }

    #[test]
    fn kits_get_their_own_names() {
        let mut a = vec![decl(), PassDecl { name: None, reads: vec!["color".into(), "soft".into()], ..decl() }];
        namespace(&mut a, "k0_");
        assert_eq!(a[0].name.as_deref(), Some("k0_soft"));
        assert_eq!(a[1].reads, vec!["color".to_string(), "k0_soft".to_string()]);
        // The shader still says self.soft.
        assert_eq!(a[1].slot(1), "soft");
        assert!(a[1].source().contains("soft: texture_2d(float)"));
    }

    #[test]
    fn names_are_checked() {
        let mut d = decl();
        d.reads.push("uv".into());
        assert!(d.validate().unwrap_err().contains("standard block"));
        let mut d = decl();
        d.uniforms.push(UniformDecl { name: "color".into(), width: 1 });
        assert!(d.validate().is_err());
        let mut d = decl();
        d.reads = vec!["a b".into()];
        assert!(d.validate().is_err());
        let mut d = decl();
        d.pixel = "return 1".into();
        assert!(d.validate().is_err());
        assert!(decl().validate().is_ok());
    }
}
