//! The GPU side of Splash passes: `DrawGraphPass`, the standard block
//! every pass subclasses, and [`Programs`], which compiles each distinct
//! pass source once (see [`crate::pass`]).

use crate::pass::{PassCode, PassDecl, PassFn, PassNamespace};
use crate::plan::ProgramId;
use makepad_draw::*;
use std::collections::HashMap;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    // The shared stdlib (`mod.shared`, registered first) every pass's
    // code has in scope.
    use mod.shared.*

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
        // x = exposure, y = aspect (w / h), z = 1 when @history holds the
        // pass's previous output, w = this pass's pixels per output pixel
        g_misc: uniform(vec4(1.0, 1.0, 0.0, 1.0))
        // The camera for depth reads: proj[10], proj[14], orthographic.
        g_cam: uniform(vec4(-1.0, -0.2, 0.0, 1.0))
        // x = 1 when depth reads go through the view below (`view_depth_at`):
        // for a host whose depth was drawn through any matrix (a
        // homography, an orthographic pass), not a perspective camera.
        g_depth: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // The view (when the host gives one): the inverse view-projection,
        // the eye (w = 1 when set) and the view's forward axis.
        g_ivp: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))
        g_eye: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        g_fwd: uniform(vec4(0.0, 0.0, -1.0, 0.0))
        // Last frame's projection x view (realtime velocity).
        g_pvp: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))

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
        history_ready: fn() -> float { return self.g_misc.z - 2.0 * step(1.5, self.g_misc.z) }
        // 1 in a realtime preview, 0 in a render or an export: a pass may
        // take fewer samples (supersampling taps, march steps) when 1.
        realtime: fn() -> float { return step(1.5, self.g_misc.z) }
        // This pass's pixels per output pixel (supersampling times the
        // pass's scale): a line N output pixels wide is N * px_scale()
        // pass pixels, and fwidth()/dFdx() are in pass pixels. Take
        // derivatives before any branch that differs between pixels: in
        // such a branch they are undefined (Metal gives too small a
        // footprint, so filtered detail aliases).
        px_scale: fn() -> float { return self.g_misc.w }
        // The view distance of a depth-buffer value (a sample of a depth
        // read, ndc z on every backend), for depth of field, outlines and
        // fog passes.
        view_depth: fn(z: float) -> float {
            if self.g_cam.z > 0.5 {
                return 0.0 - (z - self.g_cam.y) / self.g_cam.x
            }
            return self.g_cam.y / (z + self.g_cam.x)
        }
        // The view distance of the depth-buffer value `z` at `uv`: as
        // `view_depth` for a perspective or orthographic camera, or, when
        // the host reads depth through its view (`g_depth.x`), the distance
        // along the view's forward axis of the world point the sample shows.
        view_depth_at: fn(uv: vec2, z: float) -> float {
            if self.g_depth.x > 0.5 {
                return self.view_distance(self.world_at(uv, z))
            }
            return self.view_depth(z)
        }
        // The camera, for raymarch passes: the eye, the world direction
        // through `uv`, and a point's view distance (what `view_depth`
        // gives for the depth buffer, so a pass's depth output and the
        // scene's compare).
        eye: fn() -> vec3 { return self.g_eye.xyz }
        ray_dir: fn(uv: vec2) -> vec3 {
            let n = vec4(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 1.0, 1.0)
            let w = self.g_ivp * n
            return normalize(w.xyz / w.w - self.g_eye.xyz)
        }
        view_distance: fn(p: vec3) -> float { return dot(p - self.g_eye.xyz, self.g_fwd.xyz) }
        // The world point a depth-buffer sample shows at `uv`.
        world_at: fn(uv: vec2, z: float) -> vec3 {
            let n = vec4(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, z, 1.0)
            let w = self.g_ivp * n
            return w.xyz / w.w
        }
        // Where world point `p` was on screen last frame (uv): with `uv`,
        // the camera's screen motion there.
        prev_uv: fn(p: vec3) -> vec2 {
            let q = vec4(p, 1.0)
            let c = self.g_pvp * q
            return vec2(c.x / c.w * 0.5 + 0.5, 0.5 - c.y / c.w * 0.5)
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
            return linear_to_srgb(max(c, vec3(0.0, 0.0, 0.0)))
        }
        from_srgb: fn(c: vec3) -> vec3 {
            return srgb_to_linear(max(c, vec3(0.0, 0.0, 0.0)))
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

/// Compiled pass programs by source hash. A program that failed keeps its
/// error, so a broken pass is reported once, not recompiled every frame.
#[derive(Default)]
pub struct Programs {
    compiled: HashMap<ProgramId, Result<Box<DrawGraphPass>, String>>,
}

thread_local! {
    /// Each distinct pass source is evaluated once per process: every
    /// runner (a view, its bake steps, other views) draws with the same
    /// shader object, so a program compiles on the GPU once, however many
    /// runners use it.
    static EVALUATED: std::cell::RefCell<HashMap<ProgramId, Result<ScriptObjectRef, String>>> = Default::default();
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
        let label = decl.label.clone();
        let made = cx.try_with_vm(|vm| {
            let evaluated = EVALUATED.with(|e| e.borrow().get(&id).cloned());
            let v: ScriptValue = match evaluated {
                Some(Ok(obj)) => obj.as_object().into(),
                Some(Err(e)) => return Err(e),
                None => {
                    let made = pass_object(vm, decl).map(|obj| vm.new_object_ref(obj)).map_err(|e| format!("{label} did not compile: {e}"));
                    EVALUATED.with(|e| e.borrow_mut().insert(id, made.clone()));
                    made?.as_object().into()
                }
            };
            if v.is_err() || v.as_object().is_none() {
                return Err(format!("{label} did not compile"));
            }
            // Edit mode: the pass's functions are the document's, and their
            // literals compile as slots named by where they are written.
            if decl.live_literals && vm.host.cx().live_literals_on() {
                for f in pass_fns(decl) {
                    vm.host.cx_mut().set_literal_source(&adopted_file(f), literal_origins(f));
                }
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


/// The passes' scope and parts (`pass_scope.splash`) and the fused passes'
/// pixels (`fuse.splash`), evaluated once in this VM.
const PASS_SCOPE: &str = include_str!("pass_scope.splash");
const FUSE: &str = include_str!("fuse.splash");
const PASS_SCOPE_FILE: &str = "graph://pass_scope.splash";
const FUSE_FILE: &str = "graph://fuse.splash";

thread_local! {
    /// (the parts object, the fuse chains object) of this thread's VM.
    static PARTS: std::cell::RefCell<Option<(ScriptObjectRef, ScriptObjectRef)>> = Default::default();
}

fn parts(vm: &mut ScriptVm) -> Result<(ScriptObject, ScriptObject), String> {
    if let Some((p, f)) = PARTS.with(|p| p.borrow().as_ref().map(|(p, f)| (p.as_object(), f.as_object()))) {
        return Ok((p, f));
    }
    crate::pass_stdlib(vm);
    vm.capture_errors();
    let p = vm.eval(ScriptMod::new("", "", PASS_SCOPE_FILE, 0, 0, PASS_SCOPE, &[]));
    let f = vm.eval(ScriptMod::new("", "", FUSE_FILE, 0, 0, FUSE, &[]));
    let errors = vm.take_errors();
    let (Some(p), Some(f)) = (p.as_object(), f.as_object()) else { return Err(format!("the pass parts did not evaluate: {}", errors.join("; "))) };
    if !errors.is_empty() {
        return Err(format!("the pass parts did not evaluate: {}", errors.join("; ")));
    }
    let refs = (vm.new_object_ref(p), vm.new_object_ref(f));
    PARTS.with(|parts| *parts.borrow_mut() = Some(refs));
    Ok((p, f))
}

/// The pass's shader as an object of this VM: a `DrawGraphPass` with one
/// `texture_2d` per read, a fragment output per further output, a uniform
/// per value and the document's fns (moved here by their tokens), in the
/// order the pass declares them. No shader text is written.
pub fn pass_object(vm: &mut ScriptVm, decl: &PassDecl) -> Result<ScriptObject, String> {
    let (parts, fuse) = parts(vm)?;
    let scope = vm.end_scope_of(PASS_SCOPE_FILE).ok_or("the pass scope is gone")?;
    let draw = vm.value(vm.modules(), lut_id("draw")).as_object().ok_or("mod.draw is not registered")?;
    let proto = vm.value(draw, lut_id("DrawGraphPass"));
    if proto.as_object().is_none() {
        return Err("mod.draw.DrawGraphPass is not registered".into());
    }
    vm.capture_errors();
    let obj = vm.new_with_proto(proto);
    let part = |vm: &mut ScriptVm, name: &str, args: &[ScriptValue]| {
        let f = vm.value(parts, lut_id(name));
        vm.call(f, args)
    };
    let format = lut_id(decl.format().shader_color_format().trim_start_matches('@'));
    vm.set_value_trap(obj, lut_id("color_format"), format);
    for (i, o) in decl.outputs.iter().enumerate() {
        let v = part(vm, "output", &[((i + 1) as f64).into()]);
        vm.set_value_trap(obj, lut_id(&o.slot), v);
    }
    for i in 0..decl.reads.len() {
        let v = part(vm, "texture", &[]);
        vm.set_value_trap(obj, lut_id(decl.slot(i)), v);
    }
    for u in &decl.uniforms {
        let width = match u.width {
            1 | 2 | 3 | 16 => u.width,
            _ => 4,
        };
        let v = part(vm, &format!("uniform_{width}"), &[]);
        vm.set_value_trap(obj, lut_id(&u.name), v);
    }
    let mut failed = Vec::new();
    for h in &decl.helpers {
        match code_fn(vm, &h.code, h.namespace.as_ref(), fuse, scope) {
            Some(f) => {
                vm.set_value_trap(obj, lut_id(&h.name), f);
            }
            None => failed.push(h.name.clone()),
        }
    }
    match code_fn(vm, &decl.pixel, None, fuse, scope) {
        Some(f) => {
            vm.set_value_trap(obj, id!(pixel), f);
        }
        None => failed.push("pixel".into()),
    }
    let errors = vm.take_errors();
    if !errors.is_empty() || !failed.is_empty() {
        let mut out: Vec<String> = errors.iter().map(|e| located(e, decl)).collect();
        if errors.is_empty() {
            out.push(format!("{} is not a shader function", failed.join(", ")));
        }
        return Err(out.join("; "));
    }
    Ok(obj)
}

/// A pass fn as a fn of this VM: the document's, adopted by its tokens, or
/// a fused pass's chain; in `namespace` when a fused pass's member.
fn code_fn(vm: &mut ScriptVm, code: &PassCode, namespace: Option<&PassNamespace>, fuse: ScriptObject, scope: ScriptObject) -> Option<ScriptObject> {
    let f = match code {
        PassCode::Fn(f) => vm.adopt_fn(&f.image, scope)?,
        PassCode::Chain(n) => {
            let chain = vm.value(fuse, lut_id(&format!("fuse_{n}"))).as_object()?;
            match namespace {
                // Its own fn object: the namespace is the fn's.
                Some(_) => {
                    let image = vm.fn_image(chain)?;
                    let chain_scope = vm.end_scope_of(FUSE_FILE)?;
                    vm.adopt_fn(&image, chain_scope)?
                }
                None => chain,
            }
        }
    };
    if let Some(ns) = namespace {
        let renames: Vec<ShaderFnRename> = ns.renames.iter().map(|(a, b)| ShaderFnRename { from: lut_id(a), to: lut_id(b) }).collect();
        let input = ns.input.then_some(ShaderFnInput { field: id!(color), method: id!(sample), at: id!(uv), param: lut_id("c_in") });
        vm.set_shader_fn_namespace(f, &renames, input);
    }
    Some(f)
}

/// An id whose name the shader compiler can write (a field, a uniform, a
/// texture or a parameter name of the shader it makes).
fn lut_id(name: &str) -> LiveId {
    LiveId::from_str_with_lut(name).unwrap_or_else(|_| LiveId::from_str(name))
}

/// The document fns a pass compiles.
fn pass_fns(decl: &PassDecl) -> Vec<&PassFn> {
    decl.helpers.iter().map(|h| &h.code).chain(std::iter::once(&decl.pixel)).filter_map(|c| match c {
        PassCode::Fn(f) => Some(f),
        PassCode::Chain(_) => None,
    }).collect()
}

/// The module an adopted fn's body is named by (see `ScriptVm::adopt_fn`).
fn adopted_file(f: &PassFn) -> String {
    format!("adopted://{:016x}", f.image.hash())
}

/// An adopted fn's rows as shader literal origins: where its literals are
/// written in the document (what edit mode's live literals are named by).
fn literal_origins(f: &PassFn) -> Vec<LiteralOrigin> {
    let at = f.at();
    let rows = f.text().matches('\n').count() as u32 + 1;
    vec![LiteralOrigin::new(0, rows, 0, &at.file, at.line.saturating_sub(1), at.col.saturating_sub(1))]
}

/// `error` with an adopted fn's `adopted://<hash>` replaced by the file the
/// fn is written in.
fn located(error: &str, decl: &PassDecl) -> String {
    let mut out = error.to_string();
    for f in pass_fns(decl) {
        out = out.replace(&adopted_file(f), &f.at().file);
    }
    out
}
