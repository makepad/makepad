//! The GPU side of Splash passes: `DrawGraphPass`, the standard block
//! every pass subclasses, and [`Programs`], which compiles each distinct
//! pass source once (see [`crate::pass`]).

use crate::pass::PassDecl;
use crate::plan::ProgramId;
use makepad_draw::*;
use std::collections::HashMap;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // The shared stdlib every pass's code has in scope (`mod.shared`).
    let shared_std = #(crate::pass_stdlib(vm))

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
        let code = decl.source();
        let label = decl.label.clone();
        let file = format!("graph://pass/{:016x}", id.0);
        // Errors name the document's lines where the pass's code is written.
        let at = |line: u32| decl.locate(line);
        let made = cx.try_with_vm(|vm| {
            let evaluated = EVALUATED.with(|e| e.borrow().get(&id).cloned());
            let v: ScriptValue = match evaluated {
                Some(Ok(obj)) => obj.as_object().into(),
                Some(Err(e)) => return Err(e),
                None => {
                    crate::pass_stdlib(vm);
                    vm.bx.captured_errors = Some(Vec::new());
                    let v = vm.eval(ScriptMod { file: file.clone(), code, ..Default::default() });
                    let errors = vm.take_errors();
                    let made = match v.as_object() {
                        Some(obj) if errors.is_empty() && !v.is_err() => Ok(vm.bx.heap.new_object_ref(obj)),
                        _ => Err(format!("{label} did not compile: {}", errors.iter().map(|e| located(e, &file, &at)).collect::<Vec<_>>().join("; "))),
                    };
                    EVALUATED.with(|e| e.borrow_mut().insert(id, made.clone()));
                    v
                }
            };
            if v.is_err() || v.as_object().is_none() {
                return Err(format!("{label} did not compile"));
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


/// `error` with its `file:line:col` in the generated pass source replaced by
/// where that line is written in the document (`at`), when it is.
fn located(error: &str, file: &str, at: &dyn Fn(u32) -> Option<crate::pass::CodeAt>) -> String {
    let Some(i) = error.find(file) else { return error.to_string() };
    let rest = &error[i + file.len()..];
    let mut parts = rest.splitn(3, ':');
    let (Some(""), Some(line)) = (parts.next(), parts.next()) else { return error.to_string() };
    let Ok(line) = line.parse::<u32>() else { return error.to_string() };
    let tail = parts.next().unwrap_or("");
    let col_len = tail.chars().take_while(|c| c.is_ascii_digit()).count();
    let Some(doc) = at(line) else { return error.to_string() };
    format!("{}{}:{}:{}{}", &error[..i], doc.file, doc.line, doc.col.max(1), &tail[col_len..])
}
