//! Reading a kit: one `Kinetic{...}` Splash document, evaluated whole in
//! the VM ([`load`]).
//!
//! ```text
//! fn lift(g) { ... }                 // top-level fns: kernel helpers (CPU)
//! Kinetic{
//!     name: "Wave Line"  text: "KINETIC TEXT"  case: @upper  font: @bold
//!     size: 1.0  depth: 0.35  bevel: 0.03  bevel_type: @round  material: @metal
//!     layout: @line | @cloud  wrap: 12  align: @center  line_gap: 1.2  tracking: 0.0
//!     copies: 1  alphabet: "#%&"  cells: {res: 12 layers: 2 fill: @block}
//!     colors: {bg: #05060d a: #ffc84a b: #2a1450 c: #49e6ff}
//!     dials: {swing: 0.0 drive: 0.0 split: 0.5}  // p1.. in order, 0..1
//!     camera: {fov: 50 dist: 9 height: 0}   // dist, height in cap heights
//!     ground: {y: -0.7 size: 14}         // a floor plane (its look: `floor: fn() -> vec4`)
//!     picture: {width: 1024 height: 256 view: 2}   // glyphs into a picture; backdrop = the screen
//!     grid: {u: 96 v: 32 copies: 1}      // a grid shaped by the look's `surface: fn(uv)` hook
//!     post: [Glow{threshold: 0.62 strength: 0.9}]
//!     glyph: fn(g, o) { ... }            // the animator: a kernel (CPU)
//!     camera_fn: fn(c) { ... }           // optional camera kernel (CPU); c.share -> self.k_share
//!     curve: {points: 256 closed: true up: [0, 1, 0]}  curve_fn: fn(c) { c.pos = ... }   // a path at c.u, even by arc length
//!     dying: 0.8                          // a shorter text's surplus stays 0.8 s (g.dying = 1)
//!     look: fn() -> vec4 { ... }         // every other fn: the glyph shader
//! }
//! ```
//!
//! Stock shader helpers besides the look's lighting: `self.fwidth(v)` (both
//! draws), and on the backdrop `self.eye()`, `self.ray(uv)`,
//! `self.plane_hit(n, d)` (this pixel's ray on the plane dot(n, p) = d) and
//! `self.text_plane()` (the z = 0 point under the pixel).
//!
//! `glyph`, `camera_fn` and `curve_fn` are kernels: crate::kernel compiles
//! them from their fn objects (makepad-script-compute's vm_kernel), with
//! whatever top-level fns they reach. Every other `name: fn` field is a
//! member of the kit's draw, derived from `DrawKineticGlyph` (`look`,
//! `deform`, `floor`, helpers) or, for `backdrop`, `DrawKineticBackdrop`
//! (view.rs). The rest is read as values ([`KitValues`]).

/// What every kit is evaluated with, on its first line (so its own lines
/// keep their numbers).
const KIT_USES: &str = "use mod.std.* use mod.pod.* use mod.math.* use mod.shader.* use mod.draw use mod.shared.* use mod.kin.* ";

/// The host's side of every kit: the kernel entries, `band`, the dial
/// accessors (kit.splash).
const KIT_GLUE: &str = include_str!("kit.splash");

/// The fields of a kit that are kernels, not draw members.
pub const KERNEL_FIELDS: &[&str] = &["glyph", "camera_fn", "curve_fn"];

/// A kit evaluated: its object (kept alive while the host builds from it)
/// and its values.
pub struct Kit {
    pub object: ScriptObjectRef,
    pub values: KitValues,
}

impl Kit {
    /// The kit's fn field `name` (a script fn), if it has one.
    pub fn fn_field(&self, vm: &ScriptVm, name: &str) -> Option<ScriptObject> {
        field(vm, self.object.as_object(), name).as_object().filter(|f| vm.bx.heap.as_fn(*f).is_some())
    }

    /// The draw members the kit writes (`name: fn` fields other than the
    /// kernels), in the order written.
    pub fn shader_fns(&self, vm: &ScriptVm) -> Vec<(LiveId, ScriptValue)> {
        fields(vm, self.object.as_object())
            .into_iter()
            .filter(|(k, v)| {
                let name = k.to_string();
                !KERNEL_FIELDS.contains(&name.as_str()) && v.as_object().is_some_and(|f| vm.bx.heap.as_fn(f).is_some())
            })
            .collect()
    }
}

/// Evaluate a kit's Splash text (`file` names it in diagnostics) and read
/// its values.
pub fn load(vm: &mut ScriptVm, source: &str, file: &str) -> Result<Kit, String> {
    script_mod(vm);
    vm.bx.captured_errors = Some(Vec::new());
    // A body of its own the VM reclaims once nothing holds the kit.
    let v = vm.eval_transient(ScriptMod { file: file.to_string(), code: format!("{KIT_USES}{source}"), ..Default::default() });
    let errors = vm.take_errors();
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    let module = vm.bx.heap.value(vm.bx.heap.modules, LiveId::from_str(KIT_MODULE).into(), NoTrap).as_object();
    let kinetic = module.map(|m| vm.bx.heap.value(m, LiveId::from_str("Kinetic").into(), NoTrap));
    let o = v.as_object().filter(|o| Some(vm.bx.heap.proto(*o)) == kinetic).ok_or_else(|| format!("{file}: a kit is one `Kinetic{{ ... }}` object"))?;
    let object = vm.bx.heap.new_object_ref(o);
    let values = read_values(vm, o)?;
    Ok(Kit { object, values })
}

/// An object's own fields, in the order written.
fn fields(vm: &ScriptVm, o: ScriptObject) -> Vec<(LiveId, ScriptValue)> {
    let mut out = Vec::new();
    vm.bx.heap.object_data(o).map_iter_ordered(|k, v| {
        if let Some(k) = k.as_id() {
            out.push((k, v));
        }
    });
    out
}

// ---------------------------------------------------------------------------
// The values (read through the document VM)
// ---------------------------------------------------------------------------

use crate::shapes::{CellSpec, ShapeSpec};
use makepad_draw::*;
use makepad_text_mesh::letters::{BevelProfile, FontSource, TextAlign};

/// The kit's type settings, palette, camera, floor and passes.
#[derive(Clone, Debug)]
pub struct KitValues {
    pub name: String,
    pub text: String,
    pub upper: bool,
    pub lower: bool,
    /// The shape settings (its `text` is set per text).
    pub shape: ShapeSpec,
    pub copies: usize,
    /// bg, a, b, c.
    pub colors: [Vec4f; 4],
    /// 0 matte, 1 metal, 2 neon, 3 plastic, 4 glass, 5 holo.
    pub material: f32,
    pub dials: Vec<(String, f32)>,
    pub fov: f32,
    /// Camera distance in cap heights; None = frame the text.
    pub dist: Option<f32>,
    /// Camera height in cap heights; None = level (raised over a floor).
    pub height: Option<f32>,
    pub floor: Option<(Option<f32>, Option<f32>)>,
    /// `picture: {width height view}`: the glyphs draw flat into a picture
    /// of width x height pixels showing `view` cap heights vertically (an
    /// orthographic view), and the backdrop is the frame (a screen).
    pub picture: Option<(u32, u32, f32)>,
    /// `grid: {u v copies}`: a u x v grid the look's `surface(uv)` hook
    /// shapes (a globe, a knot, a ribbon printed with the picture), drawn
    /// `copies` times (`self.attr.x` = the copy).
    pub surface: Option<(u32, u32, u32)>,
    /// `curve: {points: 256}`: the curve_fn sampled at this many points and
    /// resampled evenly by arc length (see crate::curve).
    pub curve_points: Option<u32>,
    /// `curve: {closed: true up: [0, 1, 0]}`: how the curve is framed.
    pub curve_frames: crate::curve::Frames,
    /// `grow` runs 0..1 over this many beats (0: stays 1).
    pub cycle_beats: f32,
    pub pingpong: bool,
    /// `dying: 0.8`: when a new text needs fewer elements than the last,
    /// the surplus stays this many seconds as dying records (`g.dying` 1,
    /// `g.changed_at` the change, `g.from` where it was) so a kit can fly
    /// or fade them out; None: they vanish with the old text.
    pub dying: Option<f32>,
    pub passes: Vec<makepad_render_graph::PassDecl>,
    pub pass_values: Vec<Vec<[f32; 4]>>,
}

fn field(vm: &ScriptVm, o: ScriptObject, name: &str) -> ScriptValue {
    let v = vm.bx.heap.value(o, LiveId::from_str(name).into(), NoTrap);
    if v.is_err() {
        NIL
    } else {
        v
    }
}

fn text_of(vm: &mut ScriptVm, v: ScriptValue) -> Option<String> {
    if v.is_nil() {
        return None;
    }
    if let Some(id) = v.as_id() {
        return Some(id.to_string());
    }
    if v.is_string_like() {
        return vm.bx.heap.cast_to_owned_string(v, "kinetic kit");
    }
    None
}

fn num(v: ScriptValue) -> Option<f32> {
    v.as_number().map(|n| n as f32).or_else(|| v.as_bool().map(|b| if b { 1.0 } else { 0.0 }))
}

fn color(v: ScriptValue) -> Option<Vec4f> {
    v.as_color().map(Vec4f::from_u32)
}

/// The module kits are read in: `Kinetic` plus the graph's post kits.
pub const KIT_MODULE: &str = "kin";

/// Registers the kit module (once per VM).
pub fn script_mod(vm: &mut ScriptVm) {
    let have = vm.bx.heap.value(vm.bx.heap.modules, LiveId::from_str(KIT_MODULE).into(), NoTrap).as_object().is_some();
    if have {
        return;
    }
    let m = vm.new_module(LiveId::from_str(KIT_MODULE));
    let proto = vm.bx.heap.new_object();
    vm.bx.heap.set_value_def(m, LiveId::from_str("Kinetic").into(), proto.into());
    let types: Vec<(&str, &str)> = makepad_render_graph::kits::KITS.iter().map(|k| (k.name, k.kind)).chain([("Pass", "pass")]).collect();
    makepad_render_graph::script::register_post_types(vm, m, &types);
    for e in makepad_render_graph::script::install_kits(vm, &format!("mod.{KIT_MODULE}"), None) {
        log!("kinetic: graph kits: {e}");
    }
    vm.bx.captured_errors = Some(Vec::new());
    vm.eval(ScriptMod { file: "kinetic_type/kit.splash".into(), code: KIT_GLUE.into(), ..Default::default() });
    for e in vm.take_errors() {
        log!("kinetic: kit.splash: {e}");
    }
}

/// Reads the values of the evaluated kit `o`.
fn read_values(vm: &mut ScriptVm, o: ScriptObject) -> Result<KitValues, String> {
    let mut shape = ShapeSpec::default();
    let s = |vm: &mut ScriptVm, n: &str| {
        let v = field(vm, o, n);
        text_of(vm, v)
    };
    let f = |vm: &ScriptVm, n: &str| num(field(vm, o, n));
    let mut bold = true;
    if let Some(font) = s(vm, "font") {
        bold = font == "bold";
        shape.font = if font.contains('/') || font.contains('.') { FontSource::Path(font.into()) } else { FontSource::Bundled(font) };
    }
    shape.weight = f(vm, "weight");
    // `@bold` (the default) is Inter at 800, the display weight kinetic
    // type has always been set in.
    if bold {
        shape.font = FontSource::Bundled("inter".into());
        shape.weight = shape.weight.or(Some(800.0));
    }
    // Variable-font axes by four-letter tag: `axes: {wdth: 125 slnt: -8}`
    // (`wght` is `weight`).
    let axes = field(vm, o, "axes");
    if let Some(a) = axes.as_object() {
        for (tag, v) in fields(vm, a) {
            let Some(v) = num(v) else { continue };
            let tag = tag.to_string();
            let b = tag.as_bytes();
            if b.len() != 4 {
                return Err(format!("axes: `{tag}` is not a four-letter axis tag (wdth, wght, slnt, opsz, GRAD, ...)"));
            }
            if tag == "wght" {
                shape.weight = Some(v);
            } else {
                shape.axes.push((u32::from_be_bytes([b[0], b[1], b[2], b[3]]), v));
            }
        }
    }
    if let Some(v) = f(vm, "size") {
        shape.size = v.max(0.001);
    }
    if let Some(v) = f(vm, "depth") {
        shape.depth = v.max(0.0);
    }
    if let Some(v) = f(vm, "bevel") {
        shape.bevel = v.max(0.0);
    }
    if let Some(b) = s(vm, "bevel_type") {
        shape.bevel_profile = BevelProfile::by_name(&b).ok_or_else(|| format!("bevel_type: @{b} is not one of {}", BevelProfile::NAMES.join(", ")))?;
        if shape.bevel == 0.0 && b != "flat" {
            shape.bevel = (shape.depth * 0.25).min(shape.size * 0.06);
        }
        if matches!(shape.bevel_profile, BevelProfile::Round | BevelProfile::Cove | BevelProfile::Ogee) {
            shape.bevel_segments = 4;
        }
    }
    if let Some(v) = f(vm, "bevel_rings") {
        shape.bevel_segments = v.clamp(1.0, 8.0) as u32;
    }
    if let Some(v) = f(vm, "detail") {
        shape.detail = v;
    }
    if let Some(v) = f(vm, "tracking") {
        shape.tracking = v;
    }
    if let Some(v) = f(vm, "line_gap") {
        shape.line_height = v;
    }
    if let Some(v) = f(vm, "wrap") {
        shape.wrap = Some(v);
    }
    if let Some(a) = s(vm, "align") {
        shape.align = match a.as_str() {
            "left" => TextAlign::Left,
            "right" => TextAlign::Right,
            _ => TextAlign::Center,
        };
    }
    if s(vm, "layout").as_deref() == Some("cloud") {
        shape.cloud = true;
    }
    if let Some(a) = s(vm, "alphabet") {
        shape.alphabet = a;
    }
    let cells = field(vm, o, "cells");
    if let Some(c) = cells.as_object() {
        let fill = {
            let v = field(vm, c, "fill");
            text_of(vm, v)
        };
        shape.cells = Some(CellSpec {
            res: num(field(vm, c, "res")).unwrap_or(12.0) as u32,
            layers: num(field(vm, c, "layers")).unwrap_or(2.0) as u32,
            block: fill.as_deref() == Some("block"),
        });
    }
    let case = s(vm, "case");
    let mut colors = [vec4(0.02, 0.02, 0.04, 1.0), vec4(1.0, 1.0, 1.0, 1.0), vec4(0.2, 0.2, 0.3, 1.0), vec4(1.0, 0.45, 0.2, 1.0)];
    let cv = field(vm, o, "colors");
    if let Some(c) = cv.as_object() {
        for (k, n) in ["bg", "a", "b", "c"].iter().enumerate() {
            if let Some(v) = color(field(vm, c, n)) {
                colors[k] = v;
            }
        }
    }
    let material = match s(vm, "material").as_deref() {
        None | Some("matte") => 0.0,
        Some("metal") | Some("chrome") | Some("gold") => 1.0,
        Some("neon") => 2.0,
        Some("plastic") => 3.0,
        Some("glass") => 4.0,
        Some("holo") => 5.0,
        Some(other) => return Err(format!("material: @{other} is not one of matte, metal, neon, plastic, glass, holo")),
    };
    let mut dials = Vec::new();
    let dv = field(vm, o, "dials");
    if let Some(d) = dv.as_object() {
        for (name, v) in fields(vm, d) {
            dials.push((name.to_string(), num(v).unwrap_or(0.5)));
        }
    }
    // A dial is a kernel param and a shader function by its name: it may
    // not take a name the kernel or the look already has.
    const TAKEN: &[&str] = &[
        "time", "seed", "count", "p1", "p2", "p3", "p4", "pos", "rot", "scale", "shear", "color", "attr", "info", "shape", "face", "nrm", "wpos", "lpos", "luv", "p", "bands",
        "key", "rim", "cap", "n", "vd", "eye", "content", "screen_uv", "finish", "shade", "env", "spec", "hue", "fog", "look", "floor", "deform", "backdrop", "picture", "ink",
        "qrot", "qturn", "hash1", "phase", "pulse", "beat", "bar", "bpm", "energy", "fwidth", "ray", "plane_hit", "text_plane", "k_share", "dying",
    ];
    for (name, _) in &dials {
        let module_fn = crate::kernel::KINETIC_MODULE.lines().filter_map(|l| l.strip_prefix("fn ")).any(|l| l.split('(').next() == Some(name.as_str()));
        if TAKEN.contains(&name.as_str()) || module_fn || crate::kernel::SIGNALS.iter().any(|(s, _)| s == name) {
            return Err(format!("dial `{name}` clashes with a name kits already have; call it something else (e.g. `{name}_amt`)"));
        }
    }
    let (mut fov, mut dist, mut height) = (50.0, None, None);
    let cam = field(vm, o, "camera");
    if let Some(c) = cam.as_object() {
        fov = num(field(vm, c, "fov")).unwrap_or(fov);
        dist = num(field(vm, c, "dist"));
        height = num(field(vm, c, "height"));
    }
    let fl = field(vm, o, "ground");
    let floor = fl.as_object().map(|c| (num(field(vm, c, "y")), num(field(vm, c, "size"))));
    let pic = field(vm, o, "picture");
    let picture = pic.as_object().map(|c| {
        let w = num(field(vm, c, "width")).unwrap_or(1024.0).clamp(16.0, 4096.0) as u32;
        let h = num(field(vm, c, "height")).unwrap_or(256.0).clamp(16.0, 4096.0) as u32;
        (w, h, num(field(vm, c, "view")).unwrap_or(2.0).max(0.01))
    });
    let sf = field(vm, o, "grid");
    let surface = sf.as_object().map(|c| {
        let u = num(field(vm, c, "u")).unwrap_or(96.0).clamp(2.0, 1024.0) as u32;
        let v = num(field(vm, c, "v")).unwrap_or(32.0).clamp(2.0, 1024.0) as u32;
        (u, v, num(field(vm, c, "copies")).unwrap_or(1.0).clamp(1.0, 256.0) as u32)
    });
    let cv = field(vm, o, "curve");
    let curve_points = cv.as_object().map(|c| num(field(vm, c, "points")).unwrap_or(256.0).clamp(4.0, 4096.0) as u32);
    let curve_frames = cv.as_object().map_or(crate::curve::Frames::default(), |c| crate::curve::Frames {
        closed: num(field(vm, c, "closed")).unwrap_or(0.0) > 0.5,
        up: field(vm, c, "up").as_array().and_then(|a| {
            let at = |i| num(vm.bx.heap.array_index(a, i, NoTrap));
            Some([at(0)?, at(1)?, at(2)?])
        }),
    });
    let (mut cycle_beats, mut pingpong) = (0.0, false);
    let cy = field(vm, o, "cycle");
    if let Some(c) = cy.as_object() {
        cycle_beats = num(field(vm, c, "beats")).unwrap_or(4.0);
        pingpong = num(field(vm, c, "pingpong")).unwrap_or(0.0) > 0.5;
    }
    // Passes: kits call their template, `Pass{}`s are read as they are.
    let mut passes = Vec::new();
    let mut pass_values = Vec::new();
    let post = field(vm, o, "post");
    let items = makepad_render_graph::script::post_items(vm, post);
    let module = vm.bx.heap.value(vm.bx.heap.modules, LiveId::from_str(KIT_MODULE).into(), NoTrap).as_object().ok_or("the kit module is not registered")?;
    for (k, item) in items.into_iter().enumerate() {
        let label = format!("post[{k}]");
        let kind = makepad_render_graph::script::post_kind(vm, item).ok_or_else(|| format!("{label} is a kit (Glow{{..}}) or a Pass{{..}}"))?;
        let reads = match makepad_render_graph::script::read_post_entry(vm, module, &kind, item, &label, &format!("p{k}_"))? {
            makepad_render_graph::script::PostEntry::Passes(reads) => reads,
            makepad_render_graph::script::PostEntry::Host(_) => return Err(format!("{label} is a kit (Glow{{..}}) or a Pass{{..}}")),
        };
        for read in reads {
            let mut decl = read.decl;
            let mut v = Vec::new();
            for (name, val) in read.uniforms {
                let (width, value) = if let Some(n) = num(val) {
                    (1, [n, 0.0, 0.0, 0.0])
                } else if let Some(c) = color(val) {
                    (4, [c.x, c.y, c.z, c.w])
                } else {
                    return Err(format!("{}: uniform `{name}` is a number or a colour", decl.label));
                };
                decl.uniforms.push(makepad_render_graph::UniformDecl { name, width });
                v.push(value);
            }
            decl.validate().map_err(|e| format!("{}: {e}", decl.label))?;
            passes.push(decl);
            pass_values.push(v);
        }
    }
    Ok(KitValues {
        name: s(vm, "name").unwrap_or_default(),
        text: s(vm, "text").unwrap_or_default(),
        upper: case.as_deref() == Some("upper"),
        lower: case.as_deref() == Some("lower"),
        shape,
        copies: f(vm, "copies").unwrap_or(1.0).clamp(1.0, 64.0) as usize,
        colors,
        material,
        dials,
        fov,
        dist,
        height,
        floor,
        picture,
        surface,
        curve_points,
        curve_frames,
        cycle_beats,
        pingpong,
        dying: f(vm, "dying").filter(|d| *d > 0.0).map(|d| d.min(30.0)),
        passes,
        pass_values,
    })
}
