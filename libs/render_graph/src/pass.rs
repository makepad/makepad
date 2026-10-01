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
//!     preview_scale: 0.5            // and times this in a realtime preview
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
//! with no other input) reads zeros then. A pass may read another history
//! pass's latest output as `"name.prev"` (give it a slot name): last
//! frame's when that pass comes later, which is how a multi-pass
//! simulation (advect, solve, project) feeds its last pass back to its
//! first. Locked time refuses history (see [`crate::locked`]).
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
//!   pass's previous output, 0 on a cold start), `self.realtime()` (1 in a
//!   realtime preview, 0 in a render or an export: fewer samples may do);
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
    /// `preview_scale`: in a realtime preview the pass runs at `scale`
    /// times this (0.5: a quarter of the pixels), in renders and exports at
    /// `scale` (the same shader, so exports are untouched). 1 by default.
    pub preview_scale: f32,
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
    /// Further named outputs written in the same draw (MRT: a raymarch's
    /// G-buffer and depth). The pixel fn writes them as `self.<slot>`.
    pub outputs: Vec<OutputDecl>,
    /// For diagnostics.
    pub label: String,
    /// The pass reads `@color` only at its own pixel
    /// (`self.color.sample(self.uv())`): passes like it that follow one
    /// another run as one ([`fuse`]).
    pub map: bool,
    /// Where the pass's code is written: each function's text (`pixel`,
    /// a helper) and its place in the document, so a compile error names
    /// the document's line.
    pub origins: Vec<(String, CodeAt)>,
}

/// A place in a document's source.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CodeAt {
    pub file: String,
    pub line: u32,
    pub col: u32,
}

/// A further output of a pass: its resource name (what later passes read),
/// its name in the shader, and its format.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OutputDecl {
    pub name: String,
    pub slot: String,
    pub format: Format,
}

/// A slot name must be a plain identifier; the standard block's names are
/// taken.
pub fn check_ident(name: &str) -> Result<(), String> {
    const TAKEN: &[&str] = &["uv", "texel", "size", "aspect", "time", "frame", "ss_tap", "exposure", "luma", "hash", "frame_hash", "to_srgb", "from_srgb", "view_depth", "history_ready", "realtime", "px_scale", "g_cam", "eye", "ray_dir", "view_distance", "g_ivp", "g_eye", "g_fwd", "g_pvp", "world_at", "prev_uv", "pixel", "vertex", "pos", "world", "geom", "draw_call", "draw_pass", "draw_list", "g_frame", "g_size", "g_misc", "color_format", "depth_clip"];
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
        for o in &self.outputs {
            check_ident(&o.slot)?;
            if seen.contains(&o.slot.as_str()) {
                return Err(format!("`{}` is both an output and a read or uniform", o.slot));
            }
            if self.history {
                return Err("a history pass has one output".into());
            }
            seen.push(&o.slot);
        }
        if self.outputs.len() > 3 {
            return Err("a pass writes at most four outputs (itself and three more)".into());
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
    /// Where line `line` (1-based) of [`Self::source`] is written in the
    /// document, when it is inside one of the pass's functions.
    pub fn locate(&self, line: u32) -> Option<CodeAt> {
        let src = self.source();
        for (text, at) in &self.origins {
            let Some(start) = src.find(text.as_str()) else { continue };
            let first = src[..start].matches('\n').count() as u32 + 1;
            let last = first + text.matches('\n').count() as u32;
            if line >= first && line <= last {
                return Some(CodeAt { file: at.file.clone(), line: at.line + (line - first), col: if line == first { at.col } else { 0 } });
            }
        }
        None
    }

    pub fn source(&self) -> String {
        let mut s = String::new();
        // The shared stdlib (`hash12`, `snoise2`, `srgb_to_linear`, `sd_*`, eases, …) is
        // in scope, as in Shader layers, kernels and documents.
        s.push_str("use mod.pod.*\nuse mod.math.*\nuse mod.shader.*\nuse mod.shared.*\nuse mod.draw\nmod.draw.DrawGraphPass{\n");
        s.push_str(&format!("    color_format: {}\n", self.format().shader_color_format()));
        for (i, o) in self.outputs.iter().enumerate() {
            s.push_str(&format!("    {}: fragment_output({}, vec4f)\n", o.slot, i + 1));
        }
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
    let own: Vec<String> = decls.iter().filter_map(|d| d.name.clone()).chain(decls.iter().flat_map(|d| d.outputs.iter().map(|o| o.name.clone()))).collect();
    for d in decls.iter_mut() {
        if d.slots.is_empty() {
            d.slots = d.reads.clone();
        }
        for r in d.reads.iter_mut() {
            if own.contains(r) {
                *r = format!("{prefix}{r}");
            }
        }
        for o in d.outputs.iter_mut() {
            o.name = format!("{prefix}{}", o.name);
        }
        if let Some(n) = &mut d.name {
            *n = format!("{prefix}{n}");
        }
    }
}

/// Whether `d` can run inside a fused pass: a `map` pass writing the frame
/// colour at the frame's size, nothing else.
fn fusable(d: &PassDecl) -> bool {
    d.map && d.name.is_none() && d.scale == 1.0 && d.preview_scale == 1.0 && d.size.is_none() && d.format.is_none() && d.outputs.is_empty() && !d.history
        && d.reads.first().is_some_and(|r| r == "color") && d.slot(0) == "color"
}

/// Rename `self.<name>` for the names in `names` to `self.<prefix><name>`.
fn prefixed(src: &str, names: &[String], prefix: &str) -> String {
    let mut out = String::with_capacity(src.len() + 64);
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if src[i..].starts_with("self.") && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) {
            let start = i + 5;
            let mut end = start;
            while end < b.len() && (b[end].is_ascii_alphanumeric() || b[end] == b'_') {
                end += 1;
            }
            let ident = &src[start..end];
            out.push_str("self.");
            if names.iter().any(|n| n == ident) {
                out.push_str(prefix);
            }
            out.push_str(ident);
            i = end;
        } else {
            let ch = src[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// The helper names a pass defines (`name: fn(` at the start of a line).
fn helper_names(helpers: &str) -> Vec<String> {
    helpers
        .lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let (name, rest) = l.split_once(':')?;
            (rest.trim_start().starts_with("fn") && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.is_empty()).then(|| name.to_string())
        })
        .collect()
}

/// Consecutive `map` passes of one stage run as one pass: each one's pixel
/// becomes a function of the colour so far, called in order on the colour
/// at the pixel, with its uniforms, reads and helpers renamed apart. What a
/// run writes is the same picture without the intermediate targets (only
/// the rounding of the half-float frames between them differs). Returns the
/// passes to run and, for each, the original passes it holds (in order; a
/// pass alone holds itself), whose uniform values go in that order.
pub fn fuse(decls: &[PassDecl]) -> (Vec<PassDecl>, Vec<Vec<usize>>) {
    // At most this many floats of uniforms and textures in one fused pass
    // (the standard block takes its own).
    const MAX_FLOATS: usize = 96;
    const MAX_READS: usize = 8;
    let floats = |d: &PassDecl| d.uniforms.iter().map(|u| if u.width == 1 { 1 } else { 4 }).sum::<usize>();
    let mut out = Vec::new();
    let mut map = Vec::new();
    let mut i = 0;
    while i < decls.len() {
        let mut j = i + 1;
        if fusable(&decls[i]) {
            let (mut f, mut r) = (floats(&decls[i]), decls[i].reads.len());
            while j < decls.len() && fusable(&decls[j]) && decls[j].stage == decls[i].stage {
                let (nf, nr) = (f + floats(&decls[j]), r + decls[j].reads.len() - 1);
                if nf > MAX_FLOATS || nr > MAX_READS {
                    break;
                }
                (f, r) = (nf, nr);
                j += 1;
            }
        }
        if j == i + 1 {
            out.push(decls[i].clone());
            map.push(vec![i]);
            i = j;
            continue;
        }
        let members = &decls[i..j];
        let mut reads = vec!["color".to_string()];
        let mut slots = vec!["color".to_string()];
        let mut uniforms = Vec::new();
        let mut helpers = String::new();
        let mut pixel = String::from("fn() -> vec4 {\n    var c = self.color.sample(self.uv())\n");
        for (k, d) in members.iter().enumerate() {
            let prefix = format!("f{k}_");
            let mut names: Vec<String> = d.uniforms.iter().map(|u| u.name.clone()).collect();
            names.extend((1..d.reads.len()).map(|n| d.slot(n).to_string()));
            names.extend(helper_names(&d.helpers));
            for u in &d.uniforms {
                uniforms.push(UniformDecl { name: format!("{prefix}{}", u.name), width: u.width });
            }
            for n in 1..d.reads.len() {
                reads.push(d.reads[n].clone());
                slots.push(format!("{prefix}{}", d.slot(n)));
            }
            if !d.helpers.trim().is_empty() {
                // Its helpers' own names too, where they are defined.
                let own = helper_names(&d.helpers);
                let renamed: Vec<String> = prefixed(&d.helpers, &names, &prefix)
                    .lines()
                    .map(|l| {
                        let t = l.trim_start();
                        match own.iter().find(|n| t.starts_with(n.as_str()) && t[n.len()..].trim_start().starts_with(':')) {
                            Some(_) => format!("{}{prefix}{t}", &l[..l.len() - t.len()]),
                            None => l.to_string(),
                        }
                    })
                    .collect();
                helpers.push_str(&renamed.join("\n"));
                helpers.push('\n');
            }
            // Its pixel, as a function of the colour so far.
            let body = d.pixel.trim();
            let open = body.find('{').map_or(0, |o| o + 1);
            let close = body.rfind('}').unwrap_or(body.len());
            let inner = prefixed(&body[open..close], &names, &prefix).replace("self.color.sample(self.uv())", "c_in");
            helpers.push_str(&format!("    {prefix}pixel: fn(c_in: vec4) -> vec4 {{{inner}}}\n"));
            pixel.push_str(&format!("    c = self.{prefix}pixel(c)\n"));
        }
        pixel.push_str("    return c\n}");
        out.push(PassDecl {
            name: None,
            stage: members[0].stage,
            reads,
            slots,
            scale: 1.0,
            preview_scale: 1.0,
            size: None,
            format: None,
            uniforms,
            pixel,
            helpers,
            history: false,
            outputs: Vec::new(),
            label: members.iter().map(|d| d.label.as_str()).collect::<Vec<_>>().join(" + "),
            map: true,
            origins: members.iter().flat_map(|d| d.origins.iter().cloned()).collect(),
        });
        map.push((i..j).collect());
        i = j;
    }
    (out, map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_passes_that_follow_one_another_fuse() {
        let m = |label: &str, reads: &[&str], pixel: &str, uniforms: &[&str]| PassDecl {
            name: None,
            stage: Stage::Hdr,
            reads: reads.iter().map(|r| r.to_string()).collect(),
            slots: Vec::new(),
            scale: 1.0,
            preview_scale: 1.0,
            size: None,
            format: None,
            uniforms: uniforms.iter().map(|u| UniformDecl { name: u.to_string(), width: 1 }).collect(),
            pixel: pixel.into(),
            helpers: String::new(),
            history: false,
            outputs: Vec::new(),
            label: label.into(),
            map: true,
            origins: Vec::new(),
        };
        let a = m("a", &["color", "bloom"], "fn() -> vec4 { let c = self.color.sample(self.uv()) return c + self.bloom.sample(self.uv()) * self.amount }", &["amount"]);
        let b = m("b", &["color"], "fn() -> vec4 { let c = self.color.sample(self.uv()) return c * self.amount }", &["amount"]);
        let mut c = m("c", &["color"], "fn() -> vec4 { return self.color.sample(self.uv() + vec2(0.01, 0.0)) }", &[]);
        c.map = false;
        let (out, map) = fuse(&[a, b, c.clone()]);
        assert_eq!(map, vec![vec![0, 1], vec![2]]);
        assert_eq!(out.len(), 2);
        let f = &out[0];
        assert_eq!(f.reads, vec!["color", "bloom"]);
        assert_eq!(f.slots, vec!["color", "f0_bloom"]);
        assert_eq!(f.uniforms.iter().map(|u| u.name.as_str()).collect::<Vec<_>>(), vec!["f0_amount", "f1_amount"]);
        assert!(f.helpers.contains("f0_pixel: fn(c_in: vec4) -> vec4 { let c = c_in return c + self.f0_bloom.sample(self.uv()) * self.f0_amount }"), "{}", f.helpers);
        assert!(f.helpers.contains("f1_pixel: fn(c_in: vec4) -> vec4 { let c = c_in return c * self.f1_amount }"), "{}", f.helpers);
        assert!(f.pixel.contains("c = self.f0_pixel(c)") && f.pixel.contains("c = self.f1_pixel(c)"));
        assert_eq!(out[1], c);
        // A lone map pass is itself.
        let (out, map) = fuse(&out[1..2]);
        assert_eq!((out.len(), map), (1, vec![vec![0]]));
    }

    fn decl() -> PassDecl {
        PassDecl {
            name: Some("soft".into()),
            stage: Stage::Hdr,
            reads: vec!["color".into(), "glow".into()],
            slots: Vec::new(),
            scale: 0.5,
            preview_scale: 1.0,
            size: None,
            format: None,
            uniforms: vec![UniformDecl { name: "amount".into(), width: 1 }, UniformDecl { name: "tint".into(), width: 4 }],
            pixel: "fn() -> vec4 { return self.color.sample(self.uv()) * self.amount }".into(),
            helpers: String::new(),
            history: false,
            outputs: Vec::new(),
            label: "Pass".into(),
            map: false,
            origins: Vec::new(),
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
    fn a_line_of_the_pass_source_is_found_in_the_document() {
        let mut d = decl();
        d.pixel = "fn() -> vec4 {\n    let c = self.color.sample(self.uv())\n    return c * self.amount\n}".into();
        d.origins = vec![(d.pixel.clone(), CodeAt { file: "motion".into(), line: 40, col: 16 })];
        let src = d.source();
        let first = src[..src.find("fn() -> vec4").unwrap()].matches('\n').count() as u32 + 1;
        assert_eq!(d.locate(first), Some(CodeAt { file: "motion".into(), line: 40, col: 16 }));
        assert_eq!(d.locate(first + 2).map(|a| a.line), Some(42));
        assert_eq!(d.locate(1), None, "the header is not the document's");
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
