//! Equations on the GPU: the per-vertex (`per_pixel_N`) and custom-wave
//! per-point code compiled from the EEL syntax tree ([`crate::expr`]) into
//! makepad shader source, so each grid vertex / wave point evaluates in
//! parallel instead of in the CPU VM.
//!
//! MilkDrop runs these blocks SEQUENTIALLY over vertices or points with
//! variables that persist from one element to the next. A block evaluates
//! correctly in parallel only when no element can observe another's writes:
//! [`analyze`] finds variables that may be read before they are written in
//! an element AND are written somewhere in the block — state carried
//! across elements. Those blocks, and blocks using `megabuf`/`gmegabuf`,
//! keep the CPU path; [`GpuPlan`] says which and why.
//!
//! Variables that the host resets for every element (a vertex's `x`, `y`,
//! `rad`, `ang` and the per-frame motion values it starts from; a point's
//! `sample`, `value1`, `value2`, `x`, `y`, `r`..`a`) never carry state.
//!
//! EEL semantics live in helper functions on `DrawMilkBase` (`eel_div`,
//! `eel_mod`, `eel_pow`, comparisons within 1e-5, ...), so the emitted code
//! stays plain. Loops are capped at [`GPU_LOOP_CAP`] iterations.

use crate::expr::{parse, Expr};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

/// Iteration cap for `loop` / `while` evaluated on the GPU. Lower than the
/// CPU cap: a long loop per fragment can trip the GPU watchdog.
pub const GPU_LOOP_CAP: u32 = 256;

/// Whether a block runs on the GPU, and the generated source if so.
#[derive(Clone, Debug, PartialEq)]
pub enum GpuPlan {
    /// No code: the host's defaults apply.
    Empty,
    /// Makepad shader members implementing the block.
    Gpu(String),
    /// The block must run on the CPU, for this reason.
    Cpu(String),
}

/// What a block does with its variables.
#[derive(Clone, Debug, Default)]
pub struct Analysis {
    /// Variables that may be read in an element before being written in it.
    pub read_first: HashSet<String>,
    /// Variables the block writes.
    pub written: HashSet<String>,
    pub memory: bool,
}

impl Analysis {
    /// Variables whose value flows from one element to the next, given the
    /// ones the host resets per element.
    pub fn carried(&self, reset: &[&str]) -> Vec<String> {
        let mut out: Vec<String> = self
            .read_first
            .intersection(&self.written)
            .filter(|n| !reset.contains(&n.as_str()))
            .cloned()
            .collect();
        out.sort();
        out
    }
}

pub(crate) fn analyze(e: &Expr) -> Analysis {
    let mut a = Analysis::default();
    let mut assigned = HashSet::new();
    walk(e, &mut assigned, &mut a);
    a
}

fn walk(e: &Expr, assigned: &mut HashSet<String>, a: &mut Analysis) {
    match e {
        Expr::Num(_) => {}
        Expr::Var(n) => {
            if !assigned.contains(n) {
                a.read_first.insert(n.clone());
            }
        }
        Expr::Mem(_, i) => {
            a.memory = true;
            walk(i, assigned, a);
        }
        Expr::Unary(_, x) => walk(x, assigned, a),
        Expr::Binary(_, x, y) => {
            walk(x, assigned, a);
            walk(y, assigned, a);
        }
        Expr::Assign(op, target, value) => {
            walk(value, assigned, a);
            match target.as_ref() {
                Expr::Var(n) => {
                    if *op != "=" && !assigned.contains(n) {
                        a.read_first.insert(n.clone());
                    }
                    a.written.insert(n.clone());
                    assigned.insert(n.clone());
                }
                other => walk(other, assigned, a),
            }
        }
        Expr::If(c, x, y) => {
            walk(c, assigned, a);
            let mut sx = assigned.clone();
            walk(x, &mut sx, a);
            let mut sy = assigned.clone();
            walk(y, &mut sy, a);
            *assigned = sx.intersection(&sy).cloned().collect();
        }
        Expr::And(x, y) | Expr::Or(x, y) => {
            walk(x, assigned, a);
            let mut sy = assigned.clone();
            walk(y, &mut sy, a);
        }
        Expr::Loop(n, body) => {
            walk(n, assigned, a);
            let mut sb = assigned.clone();
            walk(body, &mut sb, a);
        }
        Expr::While(body) => walk(body, assigned, a),
        Expr::Call(_, args) | Expr::Seq(args) => args.iter().for_each(|x| walk(x, assigned, a)),
    }
}

fn has_side_effects(e: &Expr) -> bool {
    match e {
        Expr::Num(_) | Expr::Var(_) => false,
        Expr::Assign(..) | Expr::Loop(..) | Expr::While(..) => true,
        Expr::Call(name, args) => name == "rand" || args.iter().any(has_side_effects),
        Expr::Mem(_, x) | Expr::Unary(_, x) => has_side_effects(x),
        Expr::Binary(_, x, y) | Expr::And(x, y) | Expr::Or(x, y) => has_side_effects(x) || has_side_effects(y),
        Expr::If(c, x, y) => has_side_effects(c) || has_side_effects(x) || has_side_effects(y),
        Expr::Seq(items) => items.iter().any(has_side_effects),
    }
}

/// One element's evaluation: every variable the block names is a local
/// `v_<name>` initialised from `bind` (an input, a uniform) or 0.
struct Gen<'a> {
    out: String,
    indent: usize,
    temp: usize,
    names: HashMap<String, String>,
    bind: &'a dyn Fn(&str) -> Option<String>,
    rand_sites: usize,
}

fn fnum(v: f64) -> String {
    if !v.is_finite() {
        return "0.0".into();
    }
    let s = format!("{v}");
    if s.contains('.') || s.contains('e') {
        s
    } else {
        format!("{s}.0")
    }
}

impl Gen<'_> {
    fn line(&mut self, text: &str) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
        self.out.push_str(text);
        self.out.push('\n');
    }

    fn fresh(&mut self) -> String {
        self.temp += 1;
        format!("t_{}", self.temp)
    }

    fn var(&mut self, name: &str) -> String {
        if let Some(v) = self.names.get(name) {
            return v.clone();
        }
        let base: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        let mut v = format!("v_{base}");
        let mut k = 2;
        while self.names.values().any(|x| *x == v) {
            v = format!("v_{base}_{k}");
            k += 1;
        }
        self.names.insert(name.to_string(), v.clone());
        v
    }

    /// Declare every variable of the block up front, so assignments inside
    /// branches and loops write the one variable.
    fn declare_all(&mut self, e: &Expr) {
        let mut names = Vec::new();
        collect_vars(e, &mut names);
        names.sort();
        names.dedup();
        for n in names {
            let v = self.var(&n);
            let init = (self.bind)(&n).unwrap_or_else(|| "0.0".into());
            self.line(&format!("var {v} = {init}"));
        }
    }

    fn snapshot(&mut self, code: String) -> String {
        let t = self.fresh();
        self.line(&format!("let {t} = {code}"));
        t
    }

    fn val(&mut self, e: &Expr) -> Result<String, String> {
        Ok(match e {
            Expr::Num(v) => fnum(*v),
            Expr::Var(n) => self.var(n),
            Expr::Mem(..) => return Err("megabuf".into()),
            Expr::Unary(op, x) => {
                let x = self.val(x)?;
                if *op == "-" {
                    format!("(-{x})")
                } else {
                    format!("self.eel_not({x})")
                }
            }
            Expr::Binary(op, x, y) => {
                let mut a = self.val(x)?;
                if has_side_effects(y) {
                    a = self.snapshot(a);
                }
                let b = self.val(y)?;
                match *op {
                    "+" | "-" | "*" => format!("({a} {op} {b})"),
                    "/" => format!("self.eel_div({a}, {b})"),
                    "%" => format!("self.eel_mod({a}, {b})"),
                    "^" => format!("self.eel_pow({a}, {b})"),
                    "==" => format!("self.eel_eq({a}, {b})"),
                    "!=" => format!("(1.0 - self.eel_eq({a}, {b}))"),
                    "<" => format!("self.eel_lt({a}, {b})"),
                    ">" => format!("self.eel_lt({b}, {a})"),
                    "<=" => format!("(1.0 - self.eel_lt({b}, {a}))"),
                    ">=" => format!("(1.0 - self.eel_lt({a}, {b}))"),
                    "&" => format!("self.eel_bitand({a}, {b})"),
                    "|" => format!("self.eel_bitor({a}, {b})"),
                    _ => return Err(format!("operator {op}")),
                }
            }
            Expr::Assign(op, target, value) => {
                let Expr::Var(n) = target.as_ref() else { return Err("megabuf".into()) };
                let v = self.val(value)?;
                let name = self.var(n);
                let rhs = match *op {
                    "=" => v,
                    "+=" => format!("({name} + {v})"),
                    "-=" => format!("({name} - {v})"),
                    "*=" => format!("({name} * {v})"),
                    "/=" => format!("self.eel_div({name}, {v})"),
                    "%=" => format!("self.eel_mod({name}, {v})"),
                    "^=" => format!("self.eel_pow({name}, {v})"),
                    "&=" => format!("self.eel_bitand({name}, {v})"),
                    "|=" => format!("self.eel_bitor({name}, {v})"),
                    _ => return Err(format!("operator {op}")),
                };
                self.line(&format!("{name} = {rhs}"));
                name
            }
            Expr::If(c, x, y) => {
                let r = self.fresh();
                self.line(&format!("var {r} = 0.0"));
                let c = self.val(c)?;
                self.line(&format!("if abs({c}) >= 0.00001 {{"));
                self.indent += 1;
                let a = self.val(x)?;
                self.line(&format!("{r} = {a}"));
                self.indent -= 1;
                self.line("} else {");
                self.indent += 1;
                let b = self.val(y)?;
                self.line(&format!("{r} = {b}"));
                self.indent -= 1;
                self.line("}");
                r
            }
            Expr::And(x, y) | Expr::Or(x, y) => {
                let is_and = matches!(e, Expr::And(..));
                let r = self.fresh();
                self.line(&format!("var {r} = 0.0"));
                let a = self.val(x)?;
                if is_and {
                    self.line(&format!("if abs({a}) >= 0.00001 {{"));
                    self.indent += 1;
                    let b = self.val(y)?;
                    self.line(&format!("if abs({b}) >= 0.00001 {{ {r} = 1.0 }}"));
                    self.indent -= 1;
                    self.line("}");
                } else {
                    self.line(&format!("if abs({a}) >= 0.00001 {{"));
                    self.line(&format!("    {r} = 1.0"));
                    self.line("} else {");
                    self.indent += 1;
                    let b = self.val(y)?;
                    self.line(&format!("if abs({b}) >= 0.00001 {{ {r} = 1.0 }}"));
                    self.indent -= 1;
                    self.line("}");
                }
                r
            }
            Expr::Loop(n, body) => {
                let n = self.val(n)?;
                let c = self.fresh();
                self.line(&format!("var {c} = clamp(floor({n}), 0.0, {})", fnum(GPU_LOOP_CAP as f64)));
                self.line("loop {");
                self.indent += 1;
                self.line(&format!("if {c} <= 0.0 {{ break }}"));
                self.line(&format!("{c} = {c} - 1.0"));
                self.val(body)?;
                self.indent -= 1;
                self.line("}");
                "0.0".into()
            }
            Expr::While(body) => {
                let c = self.fresh();
                self.line(&format!("var {c} = 0.0"));
                self.line("loop {");
                self.indent += 1;
                self.line(&format!("if {c} >= {} {{ break }}", fnum(GPU_LOOP_CAP as f64)));
                self.line(&format!("{c} = {c} + 1.0"));
                let v = self.val(body)?;
                self.line(&format!("if abs({v}) < 0.00001 {{ break }}"));
                self.indent -= 1;
                self.line("}");
                "0.0".into()
            }
            Expr::Seq(items) => {
                let mut last = "0.0".to_string();
                for it in items {
                    last = self.val(it)?;
                }
                last
            }
            Expr::Call(name, args) => self.call(name, args)?,
        })
    }

    fn call(&mut self, name: &str, args: &[Expr]) -> Result<String, String> {
        let mut vals = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let v = self.val(a)?;
            // A later argument with effects must not change an earlier one.
            let later_effects = args[i + 1..].iter().any(has_side_effects);
            vals.push(if later_effects { self.snapshot(v) } else { v });
        }
        let one = |f: &str| -> Result<String, String> {
            match vals.first() {
                Some(a) => Ok(format!("{f}({a})")),
                None => Ok("0.0".into()),
            }
        };
        let two = |f: &str| -> Result<String, String> {
            match (vals.first(), vals.get(1)) {
                (Some(a), Some(b)) => Ok(format!("{f}({a}, {b})")),
                _ => Ok("0.0".into()),
            }
        };
        match name {
            "sin" | "cos" | "tan" | "atan" | "exp" | "floor" | "ceil" | "abs" | "sign" => one(name),
            "int" => one("floor"),
            "asin" => one("self.eel_asin"),
            "acos" => one("self.eel_acos"),
            "sqrt" => Ok(vals.first().map(|a| format!("sqrt(abs({a}))")).unwrap_or_else(|| "0.0".into())),
            "sqr" => Ok(vals.first().map(|a| format!("({a} * {a})")).unwrap_or_else(|| "0.0".into())),
            "log" => one("self.eel_log"),
            "log10" => one("self.eel_log10"),
            "invsqrt" => one("self.eel_invsqrt"),
            "bnot" => one("self.eel_not"),
            "min" | "max" | "atan2" => two(name),
            "pow" => two("self.eel_pow"),
            "above" => match (vals.first(), vals.get(1)) {
                (Some(a), Some(b)) => Ok(format!("self.eel_lt({b}, {a})")),
                _ => Ok("0.0".into()),
            },
            "below" => two("self.eel_lt"),
            "equal" => two("self.eel_eq"),
            "band" => two("self.eel_band"),
            "bor" => two("self.eel_bor"),
            "sigmoid" => two("self.eel_sigmoid"),
            "fmod" => two("self.eel_fmod"),
            "rand" => {
                self.rand_sites += 1;
                let k = self.rand_sites as f64 * 17.13;
                Ok(vals
                    .first()
                    .map(|a| format!("(self.gpu_rand(seed + vec2({}, {})) * max({a}, 0.0))", fnum(k), fnum(k * 0.37)))
                    .unwrap_or_else(|| "0.0".into()))
            }
            // Unknown functions evaluate to 0, as on the CPU.
            _ => Ok("0.0".into()),
        }
    }
}

fn collect_vars(e: &Expr, out: &mut Vec<String>) {
    match e {
        Expr::Var(n) => out.push(n.clone()),
        Expr::Num(_) => {}
        Expr::Mem(_, x) | Expr::Unary(_, x) | Expr::While(x) => collect_vars(x, out),
        Expr::Binary(_, x, y) | Expr::Assign(_, x, y) | Expr::And(x, y) | Expr::Or(x, y) | Expr::Loop(x, y) => {
            collect_vars(x, out);
            collect_vars(y, out);
        }
        Expr::If(c, x, y) => {
            collect_vars(c, out);
            collect_vars(x, out);
            collect_vars(y, out);
        }
        Expr::Call(_, args) | Expr::Seq(args) => args.iter().for_each(|x| collect_vars(x, out)),
    }
}

/// Plan a block: parse, analyse, and emit its statements with `bind`
/// supplying each variable's initial value. `reset` names variables the
/// host re-initialises per element. On success returns the statements and
/// the variable-name map (EEL name → emitted local).
fn plan(
    src: &str,
    reset: &[&str],
    bind: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<(String, HashMap<String, String>)>, String> {
    if src.trim().is_empty() {
        return Ok(None);
    }
    let ast = parse(src).map_err(|e| format!("does not compile: {e}"))?;
    let analysis = analyze(&ast);
    // A block that writes nothing changes nothing.
    if analysis.written.is_empty() {
        return Ok(None);
    }
    if analysis.memory {
        return Err("uses megabuf/gmegabuf".into());
    }
    let carried = analysis.carried(reset);
    if !carried.is_empty() {
        return Err(format!("carries state across elements: {}", carried.join(", ")));
    }
    let mut g = Gen { out: String::new(), indent: 2, temp: 0, names: HashMap::new(), bind, rand_sites: 0 };
    g.declare_all(&ast);
    g.val(&ast)?;
    Ok(Some((g.out, g.names)))
}

/// Bindings every GPU block shares: frame time, audio, q1..q32, mesh and
/// screen metrics.
fn common_binding(name: &str) -> Option<String> {
    let q = |i: usize| format!("self.u_q{}.{}", i / 4, ['x', 'y', 'z', 'w'][i % 4]);
    Some(match name {
        "time" => "self.u_time.x".into(),
        "fps" => "self.u_time.y".into(),
        "frame" => "self.u_time.z".into(),
        "progress" => "self.u_time.w".into(),
        "bass" => "self.u_audio.x".into(),
        "mid" => "self.u_audio.y".into(),
        "treb" => "self.u_audio.z".into(),
        "vol" => "self.u_audio.w".into(),
        "bass_att" => "self.u_audio_att.x".into(),
        "mid_att" => "self.u_audio_att.y".into(),
        "treb_att" => "self.u_audio_att.z".into(),
        "vol_att" => "self.u_audio_att.w".into(),
        "meshx" => "self.u_mesh.x".into(),
        "meshy" => "self.u_mesh.y".into(),
        "aspectx" => "self.u_aspect.x".into(),
        "aspecty" => "self.u_aspect.y".into(),
        "pixelsx" => "self.u_pixels.x".into(),
        "pixelsy" => "self.u_pixels.y".into(),
        _ => {
            let n = name.strip_prefix('q')?.parse::<usize>().ok()?;
            if (1..=32).contains(&n) {
                q(n - 1)
            } else {
                return None;
            }
        }
    })
}

/// Variables the mesh resets at every vertex.
pub const VERTEX_RESET: &[&str] =
    &["x", "y", "rad", "ang", "zoom", "zoomexp", "rot", "warp", "cx", "cy", "dx", "dy", "sx", "sy"];

/// The per-vertex block as the mesh shader's `milk_vertex` member.
pub fn per_vertex(src: &str) -> GpuPlan {
    let bind = |name: &str| -> Option<String> {
        Some(match name {
            "x" => "x_in".into(),
            "y" => "y_in".into(),
            "rad" => "rad_in".into(),
            "ang" => "ang_in".into(),
            "zoom" => "self.u_pf0.x".into(),
            "zoomexp" => "self.u_pf0.y".into(),
            "rot" => "self.u_pf0.z".into(),
            "warp" => "self.u_pf0.w".into(),
            "cx" => "self.u_pf1.x".into(),
            "cy" => "self.u_pf1.y".into(),
            "dx" => "self.u_pf1.z".into(),
            "dy" => "self.u_pf1.w".into(),
            "sx" => "self.u_pf2.x".into(),
            "sy" => "self.u_pf2.y".into(),
            _ => return common_binding(name),
        })
    };
    match plan(src, VERTEX_RESET, &bind) {
        Ok(None) => GpuPlan::Empty,
        Err(reason) => GpuPlan::Cpu(reason),
        Ok(Some((body, names))) => {
            let v = |n: &str, default: &str| names.get(n).cloned().unwrap_or_else(|| default.to_string());
            let mut out = String::new();
            let _ = writeln!(out, "    milk_vertex: fn(f: vec2, x_in: float, y_in: float, rad_in: float, ang_in: float) -> vec2 {{");
            out.push_str("        let seed = vec2(x_in * 91.7 + 3.1, y_in * 57.3 + 7.9)\n");
            out.push_str(&body);
            let _ = writeln!(
                out,
                "        return self.warp_uv(f, rad_in, vec4({}, {}, {}, {}), vec4({}, {}, {}, {}), vec4({}, {}, 0.0, 0.0))",
                v("zoom", "self.u_pf0.x"),
                v("zoomexp", "self.u_pf0.y"),
                v("rot", "self.u_pf0.z"),
                v("warp", "self.u_pf0.w"),
                v("cx", "self.u_pf1.x"),
                v("cy", "self.u_pf1.y"),
                v("dx", "self.u_pf1.z"),
                v("dy", "self.u_pf1.w"),
                v("sx", "self.u_pf2.x"),
                v("sy", "self.u_pf2.y"),
            );
            out.push_str("    }\n");
            GpuPlan::Gpu(out)
        }
    }
}

/// Variables a wave resets at every point.
pub const POINT_RESET: &[&str] = &["sample", "value1", "value2", "x", "y", "r", "g", "b", "a"];

/// A custom wave's per-point block as the wave shader's `milk_point_xy`
/// and `milk_point_rgba` members (the same code twice; each end of a line
/// segment needs the position of both ends but only its own colour).
pub fn per_point(src: &str) -> GpuPlan {
    let bind = |name: &str| -> Option<String> {
        Some(match name {
            "sample" => "sample_in".into(),
            "value1" => "value1_in".into(),
            "value2" => "value2_in".into(),
            "x" => "(0.5 + value1_in)".into(),
            "y" => "(0.5 + value2_in)".into(),
            "r" => "self.u_wave_rgba.x".into(),
            "g" => "self.u_wave_rgba.y".into(),
            "b" => "self.u_wave_rgba.z".into(),
            "a" => "self.u_wave_rgba.w".into(),
            _ => {
                if let Some(n) = name.strip_prefix('t').and_then(|n| n.parse::<usize>().ok()) {
                    if (1..=8).contains(&n) {
                        return Some(format!("self.u_wave_t{}.{}", (n - 1) / 4, ['x', 'y', 'z', 'w'][(n - 1) % 4]));
                    }
                }
                return common_binding(name);
            }
        })
    };
    match plan(src, POINT_RESET, &bind) {
        Ok(None) => GpuPlan::Empty,
        Err(reason) => GpuPlan::Cpu(reason),
        Ok(Some((body, names))) => {
            let v = |n: &str, default: &str| names.get(n).cloned().unwrap_or_else(|| default.to_string());
            let sig = "sample_in: float, value1_in: float, value2_in: float";
            let seed = "        let seed = vec2(sample_in * 83.1 + 1.7, value1_in * 13.9 + 5.3)\n";
            let mut out = String::new();
            let _ = writeln!(out, "    milk_point_xy: fn({sig}) -> vec2 {{");
            out.push_str(seed);
            out.push_str(&body);
            let _ = writeln!(out, "        return vec2({}, {})", v("x", "(0.5 + value1_in)"), v("y", "(0.5 + value2_in)"));
            out.push_str("    }\n");
            let _ = writeln!(out, "    milk_point_rgba: fn({sig}) -> vec4 {{");
            out.push_str(seed);
            out.push_str(&body);
            let _ = writeln!(
                out,
                "        return vec4({}, {}, {}, {})",
                v("r", "self.u_wave_rgba.x"),
                v("g", "self.u_wave_rgba.y"),
                v("b", "self.u_wave_rgba.z"),
                v("a", "self.u_wave_rgba.w")
            );
            out.push_str("    }\n");
            GpuPlan::Gpu(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_vertex_block_goes_to_the_gpu() {
        let GpuPlan::Gpu(src) = per_vertex("zoom = zoom + 0.1*rad; rot = rot + if(above(x, 0.5), 0.02, -0.02);") else {
            panic!()
        };
        assert!(src.contains("var v_zoom = self.u_pf0.x"), "{src}");
        assert!(src.contains("v_zoom = (v_zoom + (0.1 * v_rad))"), "{src}");
        assert!(src.contains("if abs(self.eel_lt(0.5, v_x)) >= 0.00001 {"), "{src}");
        assert!(src.contains("return self.warp_uv(f, rad_in, vec4(v_zoom, self.u_pf0.y, v_rot,"), "{src}");
    }

    #[test]
    fn carried_state_and_memory_stay_on_the_cpu() {
        // `n` counts vertices: each vertex sees the previous one's value.
        assert!(matches!(per_vertex("n = n + 1; zoom = zoom + n*0.0001;"), GpuPlan::Cpu(r) if r.contains("n")));
        assert!(matches!(per_vertex("zoom = megabuf(3);"), GpuPlan::Cpu(r) if r.contains("megabuf")));
        // Written before read in every vertex: parallel-safe.
        assert!(matches!(per_vertex("t = rad*2; zoom = zoom + t*0.01;"), GpuPlan::Gpu(_)));
        // Read but never written: a constant 0, parallel-safe.
        assert!(matches!(per_vertex("zoom = zoom + unset;"), GpuPlan::Gpu(_)));
        // Written on one branch only, read later: may carry.
        assert!(matches!(per_vertex("if(above(x,0.5), k=1, 0); zoom = zoom + k;"), GpuPlan::Cpu(_)));
        assert_eq!(per_vertex("  // nothing\n"), GpuPlan::Empty);
        assert_eq!(per_vertex("rad * 2;"), GpuPlan::Empty);
    }

    #[test]
    fn per_point_blocks() {
        let GpuPlan::Gpu(src) = per_point("x = 0.5 + sin(sample*6.28)*0.3; y = 0.5 + value1; r = t1;") else { panic!() };
        assert!(src.contains("milk_point_xy: fn(sample_in: float, value1_in: float, value2_in: float) -> vec2 {"), "{src}");
        assert!(src.contains("var v_t1 = self.u_wave_t0.x"), "{src}");
        assert!(src.contains("return vec4(v_r, self.u_wave_rgba.y, self.u_wave_rgba.z, self.u_wave_rgba.w)"), "{src}");
        // t1 accumulating across points must stay on the CPU.
        assert!(matches!(per_point("t1 = t1 + 0.01; x = t1;"), GpuPlan::Cpu(_)));
    }
}
