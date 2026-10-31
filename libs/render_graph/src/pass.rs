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


/// One uniform a pass declares: its name and width (1..4 floats, or 16:
/// a `mat4x4f`).
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
    /// `scale_height`: `scale` is of a frame this many pixels tall instead
    /// of the frame (a bloom pyramid's levels keep their size, so the light
    /// spreads over the same part of the picture at 720p as at 1080p). At
    /// that height it is `scale` exactly.
    pub scale_height: Option<f32>,
    /// A fixed output size in pixels (simulation state, a lookup) instead
    /// of `scale`.
    pub size: Option<(u32, u32)>,
    pub format: Option<Format>,
    pub uniforms: Vec<UniformDecl>,
    /// `pixel`: the document's `fn() -> vec4 { ... }` (or a fused pass's
    /// chain of its members' pixels).
    pub pixel: PassCode,
    /// Further shader functions the pixel calls (`self.<name>(..)`), in order.
    pub helpers: Vec<PassHelper>,
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
    /// An editor's document: in edit mode its literals compile live (named
    /// by where the pass's fns are written; see `Cx::set_live_literals`).
    pub live_literals: bool,
}

/// A shader function of a pass: the document's fn, moved to the GPU's VM by
/// its tokens (no text is written or read again).
#[derive(Clone, Debug)]
pub struct PassFn {
    pub image: std::sync::Arc<makepad_script::ScriptFnImage>,
    /// Where it is written in the document, as the host names it (its
    /// VM's place unless the host maps it to an editor's file; compile
    /// errors and edit mode's live literals are named by it).
    pub at: CodeAt,
}

impl PassFn {
    pub fn new(image: makepad_script::ScriptFnImage) -> Self {
        let at = CodeAt { file: image.file().to_string(), line: image.line(), col: image.col() };
        Self { image: std::sync::Arc::new(image), at }
    }

    /// Where it is written in the document.
    pub fn at(&self) -> CodeAt {
        self.at.clone()
    }

    /// The fn as written (diagnostics).
    pub fn text(&self) -> &str {
        self.image.text()
    }
}

impl PartialEq for PassFn {
    fn eq(&self, other: &Self) -> bool {
        self.image.hash() == other.image.hash() && self.at == other.at
    }
}

impl PassDecl {
    /// The document fns the pass compiles (its own and its helpers'), for a
    /// host that names their places ([`PassFn::at`]).
    pub fn fns_mut(&mut self) -> impl Iterator<Item = &mut PassFn> {
        self.helpers.iter_mut().map(|h| &mut h.code).chain(std::iter::once(&mut self.pixel)).filter_map(|c| match c {
            PassCode::Fn(f) => Some(f),
            PassCode::Chain(_) => None,
        })
    }
}

/// A pass's code: the document's fn, or a fused pass's pixel, which runs
/// its first `n` members' pixels (`self.f<k>_pixel`) in order on the colour
/// at the pixel ([`fuse`]).
#[derive(Clone, Debug, PartialEq)]
pub enum PassCode {
    Fn(PassFn),
    Chain(usize),
}

impl PassCode {
    /// The fn as written (diagnostics; empty for a fused chain).
    pub fn text(&self) -> &str {
        match self {
            PassCode::Fn(f) => f.text(),
            PassCode::Chain(_) => "",
        }
    }

    /// The document fn, when it is one.
    pub fn as_fn(&self) -> Option<&PassFn> {
        match self {
            PassCode::Fn(f) => Some(f),
            PassCode::Chain(_) => None,
        }
    }
}

/// One further shader function of a pass, under its field name, and (in a
/// fused pass) the names its member's code reads `self` by.
#[derive(Clone, Debug, PartialEq)]
pub struct PassHelper {
    pub name: String,
    pub code: PassCode,
    pub namespace: Option<PassNamespace>,
}

/// How a fused member's function reads `self`: `self.<from>` is
/// `self.<to>` (its uniforms, slots and helpers under the member's prefix),
/// and with `input` it takes the colour so far as a first parameter in
/// place of its read of the colour at its own pixel
/// (`self.color.sample(self.uv())`). The shader compiler's fn namespace.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PassNamespace {
    pub renames: Vec<(String, String)>,
    pub input: bool,
}

impl PassNamespace {
    /// This namespace, then `outer` on what it gives (a fused pass fused
    /// again).
    fn then(&self, outer: &PassNamespace) -> PassNamespace {
        let apply = |ns: &PassNamespace, n: &str| ns.renames.iter().find(|(a, _)| a == n).map_or(n.to_string(), |(_, b)| b.clone());
        let mut renames: Vec<(String, String)> = self.renames.iter().map(|(a, b)| (a.clone(), apply(outer, b))).collect();
        for (a, b) in &outer.renames {
            if !self.renames.iter().any(|(x, _)| x == a) {
                renames.push((a.clone(), b.clone()));
            }
        }
        PassNamespace { renames, input: self.input || outer.input }
    }
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
            if !(1..=4).contains(&u.width) && u.width != 16 {
                return Err(format!("uniform `{}` must be a number, a vector of up to 4 or a mat4x4f", u.name));
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
        Ok(())
    }

    /// The program's identity: everything its shader is made of (the
    /// declarations and its fns, by their tokens and where they are
    /// written: an editor's live literals are named by their places, so the
    /// same fn moved is another program).
    pub fn program_id(&self) -> ProgramId {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.format().shader_color_format().hash(&mut h);
        for o in &self.outputs {
            o.slot.hash(&mut h);
        }
        for i in 0..self.reads.len() {
            self.slot(i).hash(&mut h);
        }
        self.uniforms.hash(&mut h);
        let code = |c: &PassCode, h: &mut std::collections::hash_map::DefaultHasher| match c {
            PassCode::Fn(f) => (0u8, f.image.hash(), &f.at).hash(h),
            PassCode::Chain(n) => (1u8, *n as u64).hash(h),
        };
        for helper in &self.helpers {
            helper.name.hash(&mut h);
            code(&helper.code, &mut h);
            if let Some(ns) = &helper.namespace {
                ns.renames.hash(&mut h);
                ns.input.hash(&mut h);
            }
        }
        code(&self.pixel, &mut h);
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
    d.map && d.name.is_none() && d.scale == 1.0 && d.scale_height.is_none() && d.preview_scale == 1.0 && d.size.is_none() && d.format.is_none() && d.outputs.is_empty() && !d.history
        && d.reads.first().is_some_and(|r| r == "color") && d.slot(0) == "color"
}

/// The most passes one fused pass runs (its pixel is one of the chains
/// `fuse_2` .. `fuse_16` of `fuse.splash`).
pub const MAX_FUSED: usize = 16;

/// Consecutive `map` passes of one stage run as one pass: each one's pixel
/// becomes a function of the colour so far, called in order on the colour
/// at the pixel, with its uniforms, reads and helpers renamed apart. What a
/// run writes is the same picture without the intermediate targets (only
/// the rounding of the half-float frames between them differs). Returns the
/// passes to run and, for each, the original passes it holds (in order; a
/// pass alone holds itself), whose uniform values go in that order.
pub fn fuse(decls: &[PassDecl]) -> (Vec<PassDecl>, Vec<Vec<usize>>) {
    // At most this many floats of uniforms in one fused pass: a draw
    // call's uniform block (platform DRAW_CALL_MAX_UNIFORM_FLOATS, 16 KB)
    // less the standard block's.
    const MAX_FLOATS: usize = 4096 - 64;
    const MAX_READS: usize = 8;
    let floats = |d: &PassDecl| d.uniforms.iter().map(|u| match u.width { 1 => 1, 16 => 16, _ => 4 }).sum::<usize>();
    let mut out = Vec::new();
    let mut map = Vec::new();
    let mut i = 0;
    while i < decls.len() {
        let mut j = i + 1;
        if fusable(&decls[i]) {
            let (mut f, mut r) = (floats(&decls[i]), decls[i].reads.len());
            while j < decls.len() && j - i < MAX_FUSED && fusable(&decls[j]) && decls[j].stage == decls[i].stage {
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
        let mut helpers = Vec::new();
        for (k, d) in members.iter().enumerate() {
            // Its uniforms, further slots and helpers under its prefix: its
            // code reads them by their own names (the fn namespace).
            let prefix = format!("f{k}_");
            let mut names: Vec<String> = d.uniforms.iter().map(|u| u.name.clone()).collect();
            names.extend((1..d.reads.len()).map(|n| d.slot(n).to_string()));
            names.extend(d.helpers.iter().map(|h| h.name.clone()));
            let renames = PassNamespace { renames: names.iter().map(|n| (n.clone(), format!("{prefix}{n}"))).collect(), input: false };
            for u in &d.uniforms {
                uniforms.push(UniformDecl { name: format!("{prefix}{}", u.name), width: u.width });
            }
            for n in 1..d.reads.len() {
                reads.push(d.reads[n].clone());
                slots.push(format!("{prefix}{}", d.slot(n)));
            }
            let within = |ns: &Option<PassNamespace>, outer: &PassNamespace| Some(match ns {
                Some(inner) => inner.then(outer),
                None => outer.clone(),
            });
            for h in &d.helpers {
                helpers.push(PassHelper { name: format!("{prefix}{}", h.name), code: h.code.clone(), namespace: within(&h.namespace, &renames) });
            }
            // Its pixel, as a function of the colour so far.
            helpers.push(PassHelper { name: format!("{prefix}pixel"), code: d.pixel.clone(), namespace: Some(PassNamespace { input: true, ..renames }) });
        }
        let pixel = PassCode::Chain(members.len());
        out.push(PassDecl {
            name: None,
            stage: members[0].stage,
            reads,
            slots,
            scale: 1.0,
            preview_scale: 1.0,
            scale_height: None,
            size: None,
            format: None,
            uniforms,
            pixel,
            helpers,
            history: false,
            outputs: Vec::new(),
            label: members.iter().map(|d| d.label.as_str()).collect::<Vec<_>>().join(" + "),
            map: true,
            live_literals: members.iter().any(|d| d.live_literals),
        });
        map.push((i..j).collect());
        i = j;
    }
    (out, map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_script::*;

    /// A Splash fn as a pass's code (made in a VM of its own).
    fn code(src: &str) -> PassCode {
        let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
        let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
        let v = vm.eval(ScriptMod::new("", "", "test.splash", 0, 0, src, &[]));
        PassCode::Fn(PassFn::new(vm.fn_image(v.as_object().unwrap()).unwrap()))
    }

    #[test]
    fn map_passes_that_follow_one_another_fuse() {
        let m = |label: &str, reads: &[&str], pixel: &str, uniforms: &[&str]| PassDecl {
            name: None,
            stage: Stage::Hdr,
            reads: reads.iter().map(|r| r.to_string()).collect(),
            slots: Vec::new(),
            scale: 1.0,
            preview_scale: 1.0,
            scale_height: None,
            size: None,
            format: None,
            uniforms: uniforms.iter().map(|u| UniformDecl { name: u.to_string(), width: 1 }).collect(),
            pixel: code(pixel),
            helpers: Vec::new(),
            history: false,
            outputs: Vec::new(),
            label: label.into(),
            map: true,
            live_literals: false,
        };
        let a_pixel = "fn() -> vec4 { let c = self.color.sample(self.uv()) return c + self.bloom.sample(self.uv()) * self.amount }";
        let out_member_pixel = |p: &str| code(p);
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
        // Each member's pixel is a helper of the fused pass, reading its own
        // names under its prefix and the colour so far as its input.
        let names: Vec<&str> = f.helpers.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["f0_pixel", "f1_pixel"]);
        let ns0 = f.helpers[0].namespace.as_ref().unwrap();
        assert!(ns0.input);
        assert_eq!(ns0.renames, vec![("amount".to_string(), "f0_amount".to_string()), ("bloom".to_string(), "f0_bloom".to_string())]);
        assert_eq!(f.helpers[0].code, out_member_pixel(&a_pixel));
        assert_eq!(f.helpers[1].namespace.as_ref().unwrap().renames, vec![("amount".to_string(), "f1_amount".to_string())]);
        assert_eq!(f.pixel, PassCode::Chain(2));
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
            scale_height: None,
            size: None,
            format: None,
            uniforms: vec![UniformDecl { name: "amount".into(), width: 1 }, UniformDecl { name: "tint".into(), width: 4 }],
            pixel: code("fn() -> vec4 { return self.color.sample(self.uv()) * self.amount }"),
            helpers: Vec::new(),
            history: false,
            outputs: Vec::new(),
            label: "Pass".into(),
            map: false,
            live_literals: false,
        }
    }

    #[test]
    fn the_program_is_what_the_shader_is_made_of() {
        let mut d = decl();
        d.stage = Stage::Final;
        assert_ne!(d.program_id(), decl().program_id(), "the output format differs");
        let mut d = decl();
        d.pixel = code("fn() -> vec4 { return self.color.sample(self.uv()) * self.amount * 2.0 }");
        assert_ne!(d.program_id(), decl().program_id(), "the code differs");
        assert_eq!(decl().program_id(), decl().program_id());
    }

    #[test]
    fn kits_get_their_own_names() {
        let mut a = vec![decl(), PassDecl { name: None, reads: vec!["color".into(), "soft".into()], ..decl() }];
        namespace(&mut a, "k0_");
        assert_eq!(a[0].name.as_deref(), Some("k0_soft"));
        assert_eq!(a[1].reads, vec!["color".to_string(), "k0_soft".to_string()]);
        // The shader still says self.soft.
        assert_eq!(a[1].slot(1), "soft");
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
