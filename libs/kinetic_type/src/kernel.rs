//! B3, the animator: the kit's `glyph: fn(g, o)` compiled as a kernel over
//! the elements. `g` is the element's `Glyph` record (records.rs), `o` the
//! draw record (`DrawKineticGlyph`'s instance fields, reflected), set to
//! rest before the body runs; the body changes what it wants and must not
//! `return` (the record is written after it).
//!
//! Signals are params (set every frame, no recompile): `beat phase pulse
//! bar bpm energy grow`, the text's `width height size`, the shape ids
//! `alpha0 alphas cube`, `text_at` (the time the text last changed),
//! `bass mid high` (0..1 bands), `floor_y` (the floor's height),
//! `view_w view_h` (a picture's world extent, 0 without one), `band(f)`
//! (the spectrum at log frequency f 0..1, the host's 32 bands), the
//! dials `p1..p4` and each dial by its own name;
//! `time` is the kernel's own time input. The `kinetic` module
//! (kinetic.splash) is imported unqualified.

use crate::kit::{FnSrc, Split};
use crate::records::glyph_layout;
use makepad_script_compute::kernel::{compile_with_modules, Kernel, Layout};
use makepad_script_compute::module::Module;
use makepad_script_compute::Backend;
use std::sync::Arc;

/// The stock motions module.
pub const KINETIC_MODULE: &str = include_str!("kinetic.splash");

/// The signals every animator has, in this order.
pub const SIGNALS: &[(&str, f32)] = &[
    ("beat", 0.0),
    ("phase", 0.0),
    ("pulse", 0.0),
    ("bar", 0.0),
    ("bpm", 120.0),
    ("energy", 0.0),
    ("grow", 1.0),
    ("width", 1.0),
    ("height", 1.0),
    ("size", 1.0),
    ("alpha0", 0.0),
    ("alphas", 0.0),
    ("cube", 0.0),
    ("text_at", -1000000000.0),
    ("bass", 0.0),
    ("mid", 0.0),
    ("high", 0.0),
    ("floor_y", 0.0),
    ("view_w", 0.0),
    ("view_h", 0.0),
];

/// A composed kernel: its source and where each part of the kit sits in it.
pub struct Composed {
    pub source: String,
    /// (first line in `source`, first line in the kit, lines).
    map: Vec<(usize, usize, usize)>,
}

impl Composed {
    /// The kit line of a line of the composed source.
    pub fn kit_line(&self, line: usize) -> Option<usize> {
        self.map.iter().find(|(s, _, n)| line >= *s && line < s + n).map(|(s, k, _)| k + (line - s))
    }
}

fn lines(s: &str) -> usize {
    s.matches('\n').count() + 1
}

/// The body with the author's parameter names bound: `fn kit_glyph(g, o)`
/// returning `o`.
fn wrapper(name: &str, f: &FnSrc, defaults: (&str, &str)) -> (String, usize) {
    let g = f.params.first().map_or(defaults.0, |s| s.as_str());
    let o = f.params.get(1).map_or(defaults.1, |s| s.as_str());
    let head = format!("fn {name}({g}, {o}) {{");
    (format!("{head}{}\n{o}\n}}\n", f.body), 0)
}

/// Compose the animator kernel for a kit with `dials` (name, default).
pub fn compose(split: &Split, dials: &[(String, f32)], out: &str) -> Composed {
    let mut s = String::new();
    let mut map = Vec::new();
    s.push_str("use kinetic.*\n");
    // The kit's own items, line for line (the kit's prefix is its lines 1..).
    map.push((lines(&s), 1, lines(&split.kernel_items)));
    s.push_str(&split.kernel_items);
    s.push('\n');
    s.push_str("let glyphs = input(Glyph)\n");
    // The newest spectrum in 32 log bands; band(f), f 0..1 low to high.
    s.push_str("let spectrum = input(f32)\nfn band(f) { spectrum[clamp(int(f * 32.0), 0, 31)] }\n");
    s.push_str(&format!("let out = output({out})\n"));
    for (name, default) in SIGNALS {
        s.push_str(&format!("let {name} = param({default:?}, -1000000000.0, 1000000000.0)\n"));
    }
    for k in 0..4 {
        let d = dials.get(k).map_or(0.5, |d| d.1);
        s.push_str(&format!("let p{} = param({d:?}, -1000.0, 1000.0)\n", k + 1));
    }
    for (name, d) in dials {
        s.push_str(&format!("let {name} = param({d:?}, -1000.0, 1000.0)\n"));
    }
    if let Some(f) = &split.glyph {
        let (text, _) = wrapper("kit_glyph", f, ("g", "o"));
        map.push((lines(&s), f.line, lines(&text)));
        s.push_str(&text);
    }
    s.push_str("fn instance(i) {\n let g = glyphs[i]\n let o = ");
    s.push_str(out);
    s.push_str("{}\n o.pos = g.rest\n o.rot = vec4(0.0, 0.0, 0.0, 1.0)\n o.scale = vec3(g.k, g.k, g.k)\n o.color = vec4(1.0, 1.0, 1.0, 1.0)\n o.info = vec4(g.t, g.word, g.line, g.index)\n o.shape = g.shape\n");
    if split.glyph.is_some() {
        s.push_str(" out[i] = kit_glyph(g, o)\n}\n");
    } else {
        s.push_str(" out[i] = o\n}\n");
    }
    Composed { source: s, map }
}

/// The camera record a `camera_fn: fn(c)` writes.
pub fn camera_layout() -> Layout {
    use makepad_script_compute::kernel::{FieldTy, LayoutField};
    let f = |name: &str, ty, offset| LayoutField { name: name.into(), ty, offset };
    Layout { name: "Camera".into(), stride: 12, fields: vec![f("eye", FieldTy::Vec3, 0), f("target", FieldTy::Vec3, 3), f("up", FieldTy::Vec3, 6), f("fov", FieldTy::F32, 9), f("roll", FieldTy::F32, 10)] }
}

/// The record a `curve_fn: fn(c)` writes: its point at parameter `u`.
pub fn curve_layout() -> Layout {
    use makepad_script_compute::kernel::{FieldTy, LayoutField};
    Layout { name: "CurvePoint".into(), stride: 4, fields: vec![LayoutField { name: "pos".into(), ty: FieldTy::Vec3, offset: 0 }, LayoutField { name: "u".into(), ty: FieldTy::F32, offset: 3 }] }
}

/// Compose the curve kernel: element i is the point at u = i / (count - 1).
pub fn compose_curve(split: &Split, dials: &[(String, f32)], f: &FnSrc) -> Composed {
    let mut split = split.clone();
    split.glyph = None;
    let mut c = compose(&split, dials, "CurvePoint");
    c.source = c.source.replacen("let glyphs = input(Glyph)\n", "\n", 1);
    let at = c.source.find("fn instance(i)").unwrap_or(c.source.len());
    c.source.truncate(at);
    let v = f.params.first().map_or("c", |s| s.as_str());
    c.map.push((lines(&c.source), f.line, lines(&f.body) + 1));
    c.source.push_str(&format!("fn kit_curve({v}) {{{}\n{v}\n}}\n", f.body));
    c.source.push_str("fn element(i) {\n let c = CurvePoint{}\n c.u = float(i) / float(max(count - 1, 1))\n out[i] = kit_curve(c)\n}\n");
    c
}

/// Compose the camera kernel: `c` starts as the default framing.
pub fn compose_camera(split: &Split, dials: &[(String, f32)], f: &FnSrc) -> Composed {
    let mut split = split.clone();
    split.glyph = None;
    let mut c = compose(&split, dials, "Camera");
    // The camera reads no glyphs (an unread buffer must still be bound).
    c.source = c.source.replacen("let glyphs = input(Glyph)\n", "\n", 1);
    // Replace the instance entry with the camera's.
    let at = c.source.find("fn instance(i)").unwrap_or(c.source.len());
    c.source.truncate(at);
    let cam = f.params.first().map_or("c", |s| s.as_str());
    let head = format!("fn kit_camera({cam}) {{");
    c.map.push((lines(&c.source), f.line, lines(&f.body) + 1));
    c.source.push_str(&format!("{head}{}\n{cam}\n}}\n", f.body));
    c.source.push_str("let base = input(Camera)\nfn element(i) {\n let b = base[0]\n let c = Camera{}\n c.eye = b.eye\n c.target = b.target\n c.up = b.up\n c.fov = b.fov\n c.roll = b.roll\n out[i] = kit_camera(c)\n}\n");
    c
}

/// Compile a composed kernel with the draw record's layout; errors name
/// the kit line.
pub fn compile(c: &Composed, out: &Layout) -> Result<Arc<Kernel>, String> {
    let layouts = [glyph_layout(), out.clone(), camera_layout(), curve_layout()];
    let modules = [Module { path: "kinetic", source: KINETIC_MODULE }];
    compile_with_modules(&c.source, &layouts, Backend::Native, &modules).map_err(|errors| {
        errors
            .iter()
            .map(|e| {
                let (line, col) = e.line_col(&c.source);
                match c.kit_line(line) {
                    Some(k) => format!("kit line {k}: {}", e.message),
                    None if line <= lines(&c.source) => format!("(kinetic runtime, line {line}:{col}): {}", e.message),
                    None => format!("{}", e.message),
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::split;
    use makepad_script_compute::kernel::{FieldTy, LayoutField};

    fn out_layout() -> Layout {
        let f = |name: &str, ty, offset| LayoutField { name: name.into(), ty, offset };
        Layout {
            name: "KineticGlyph".into(),
            stride: 24,
            fields: vec![
                f("pos", FieldTy::Vec3, 0),
                f("rot", FieldTy::Vec4, 3),
                f("scale", FieldTy::Vec3, 7),
                f("shear", FieldTy::Vec2, 10),
                f("color", FieldTy::Vec4, 12),
                f("attr", FieldTy::Vec4, 16),
                f("info", FieldTy::Vec4, 20),
                f("shape", FieldTy::F32, 23),
            ],
        }
    }

    #[test]
    fn a_kit_animator_compiles_runs_and_names_its_lines() {
        let src = "fn lift(g) { g.t * 2.0 }\nKinetic{\n  dials: {swing: 0.25}\n  glyph: fn(g, o) {\n    let w = wave(g.rest.x, width, size, time, 1.0 + swing, 1.0, pulse)\n    o.pos.y = o.pos.y + w.x + lift(g)\n    o.rot = qz(w.y)\n  }\n}\n";
        let sp = split(src).unwrap();
        let c = compose(&sp, &[("swing".into(), 0.25)], "KineticGlyph");
        let k = compile(&c, &out_layout()).unwrap_or_else(|e| panic!("{e}\n{}", c.source));
        let mut recs = vec![0.0f32; 2 * 40];
        recs[14] = 0.0;
        recs[40 + 14] = 1.0;
        recs[40] = 1.0;
        recs[35] = 1.0;
        recs[40 + 35] = 1.0;
        let mut out = vec![0.0f32; 2 * 24];
        let spec = [0.5f32; 32];
        let mut call = k.call();
        call.set_time(0.3);
        call.input("spectrum", &spec).unwrap();
        call.set_param("width", 2.0);
        call.input("glyphs", &recs).unwrap();
        call.output("out", &mut out).unwrap();
        call.run(2).unwrap();
        assert!(out[24 + 1] > 1.5, "the second glyph lifted by lift(g) = 2: {}", out[25]);
        assert!((out[3..7].iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-4, "a unit quaternion");
        // A camera kernel writes its record from the default framing.
        let cam = split("Kinetic{\n  camera_fn: fn(c) {\n    c.eye = vec3(sin(time) * 5.0, 1.0, cos(time) * 5.0)\n    c.fov = c.fov + 10.0\n  }\n}\n").unwrap();
        let ck = compile(&compose_camera(&cam, &[], cam.camera.as_ref().unwrap()), &out_layout()).unwrap_or_else(|e| panic!("{e}"));
        let base = [0.0f32, 0.0, 9.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 50.0, 0.0, 0.0];
        let mut o = [0.0f32; 12];
        let mut call = ck.call();
        call.input("base", &base).unwrap();
        call.input("spectrum", &spec).unwrap();
        call.output("out", &mut o).unwrap();
        call.run(1).unwrap();
        assert_eq!((o[1], o[7], o[9]), (1.0, 1.0, 60.0), "{o:?}");
        let bad = split("Kinetic{\n  glyph: fn(g, o) {\n    o.pos.q = 1.0\n  }\n}\n").unwrap();
        let e = compile(&compose(&bad, &[], "KineticGlyph"), &out_layout()).unwrap_err();
        assert!(e.starts_with("kit line 3"), "{e}");
    }
}
