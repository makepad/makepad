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
//! A pass placed `at: @pre` runs before the scene (simulation state, a
//! generated texture): it reads no frame colour, is named, and later passes
//! read it by name, as the host's scene may; `size: vec2(w, h)` fixes its
//! output in pixels.
//!
//! A pass with `history: true` may read `@history`: its own output of the
//! previous frame (trails, a latched frame). History is realtime state: the
//! first frame after the passes change, or after the host resets it, has no
//! history, and the slot reads the pass's first input instead while
//! `self.history_ready()` is 0 (a pass never samples a texture it has not
//! written itself); a pass whose first read is `@history` itself (state
//! with no other input) reads zeros then. Locked time refuses history (see [`crate::locked`]).
//!
//! and the host turns it into a [`PassDecl`]. [`PassDecl::source`] makes
//! the Splash shader text: a subclass of `DrawGraphPass` (the standard
//! block below) with one `texture_2d` per read, one `uniform` per value and
//! the author's `pixel`. `program::Programs` compiles each distinct source
//! once (the `gpu` feature).
//!
//! The standard block every pass has:
//! * `self.uv()`, `self.texel()` (one output texel in uv), `self.size()`
//!   (output pixels), `self.aspect()`;
//! * `self.time()` (the canonical time of the frame, or of the frame point
//!   for `@final` passes), `self.frame()` (the frame index),
//!   `self.frame_hash(i)` (a per-frame random in 0..1, constant over the
//!   shutter), `self.ss_tap()` (the supersampling tap of this sub-frame, or
//!   -1), `self.exposure()`, `self.view_depth(d)` (the view distance of a
//!   depth sample), `self.history_ready()` (1 when `@history` holds the
//!   pass's previous output, 0 on a cold start);
//! * `self.luma(c)`, `self.to_srgb(c)`, `self.from_srgb(c)`,
//!   `self.hash(p)`.

use crate::plan::{Format, ProgramId, Resource, Stage};

use std::hash::{Hash, Hasher};


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
    /// A fixed output size in pixels (simulation state, a lookup) instead
    /// of `scale`.
    pub size: Option<(u32, u32)>,
    pub format: Option<Format>,
    pub uniforms: Vec<UniformDecl>,
    /// The body of `pixel`: `fn() -> vec4 { ... }`.
    pub pixel: String,
    /// Extra shader functions the pixel calls (`name: fn(..) {..}` lines).
    pub helpers: String,
    /// The pass keeps its output across frames and may read `@history`.
    pub history: bool,
    /// For diagnostics.
    pub label: String,
}

/// A slot name must be a plain identifier; the standard block's names are
/// taken.
pub fn check_ident(name: &str) -> Result<(), String> {
    const TAKEN: &[&str] = &["uv", "texel", "size", "aspect", "time", "frame", "ss_tap", "exposure", "luma", "hash", "frame_hash", "to_srgb", "from_srgb", "view_depth", "history_ready", "g_cam", "pixel", "vertex", "pos", "world", "geom", "draw_call", "draw_pass", "draw_list", "g_frame", "g_size", "g_misc", "color_format", "depth_clip"];
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
        if !self.history && self.reads.iter().any(|r| r == "history") {
            return Err("reads @history but is not a history pass (add `history: true`)".into());
        }
        if self.history && self.reads.is_empty() {
            return Err("a history pass reads something (@history, its own last output; its first read stands in on a cold start)".into());
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
            size: None,
            format: None,
            uniforms: vec![UniformDecl { name: "amount".into(), width: 1 }, UniformDecl { name: "tint".into(), width: 4 }],
            pixel: "fn() -> vec4 { return self.color.sample(self.uv()) * self.amount }".into(),
            helpers: String::new(),
            history: false,
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
        let mut d = decl();
        d.reads.push("history".into());
        assert!(d.validate().unwrap_err().contains("history: true"));
        d.history = true;
        assert!(d.validate().is_ok());
        d.reads = vec!["history".into()];
        assert!(d.validate().is_ok());
        d.reads = vec![];
        assert!(d.validate().is_err());
        assert!(decl().validate().is_ok());
    }
}
