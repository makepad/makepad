//! The compute-kernel tools every app's AI offers, run: their definitions
//! are `makepad-kernel-tools` (re-exported here); the guide text and the
//! check are here. Stage's studio API and Sandbox's chat both call these,
//! so their AIs see the same names, descriptions and guide text.
//!
//! `kernel_check` compiles the source (with the layouts and modules the
//! call names), reports every problem with its line, column and the line's
//! text, describes what compiled (entry, buffers, params, math, worst-case
//! cost, parallel and four-wide, the time for a million elements, whether
//! untrusted work of that kernel would be admitted) and dry-runs it on a
//! few synthetic elements as untrusted work (admission, watchdog), reporting
//! non-finite outputs, emit or loop overflow and host-component errors.

use crate::engine::engine;
use makepad_script_compute::admission::{self, JobBudget, Origin};
use makepad_script_compute::kernel::{Access, FieldTy, Kernel, KernelKind, Layout, LayoutField, MathMode};
use makepad_script_compute::module::{Module, STD};
use makepad_script_compute::sched::Job;
use makepad_script_compute::{Backend, ShaderError};
use makepad_strict_json::{self as json, Value};
use std::time::Duration;

pub use makepad_kernel_tools::{find, KernelToolDef, CHECK, GUIDE, TOOLS};

const GUIDE_TEXT: &str = include_str!("guide.md");

/// The kernel guide `kernel_guide` answers: the reference card, then the
/// stdlib and the host components, listed from the code.
pub fn guide() -> String {
    let mut s = String::from(GUIDE_TEXT);
    s += "\n### Prelude (unqualified)\n";
    s += &fn_list(makepad_script_compute::kernel::KERNEL_PRELUDE);
    for (path, src) in STD {
        s += &format!("\n### {path}\n{}", header(src));
        s += &fn_list(src);
    }
    s += "\n## Host components\nHeavy Rust functions a kernel calls by name. A slice argument is three: a buffer, a word offset and a word length (clamped to the buffer).\n";
    for h in makepad_script_compute::host::all() {
        s += &format!("- `{}`: {}\n", h.name, h.doc);
    }
    s
}

/// A module's first comment paragraph, as one line.
fn header(src: &str) -> String {
    let mut out = Vec::new();
    for line in src.lines() {
        match line.trim().strip_prefix("//") {
            Some(t) => out.push(t.trim().to_string()),
            None => break,
        }
    }
    if out.is_empty() {
        String::new()
    } else {
        format!("{}\n", out.join(" "))
    }
}

/// `name(args)` of every top-level function, comma separated.
fn fn_list(src: &str) -> String {
    let fns: Vec<String> = src
        .lines()
        .filter_map(|l| l.strip_prefix("fn "))
        .filter_map(|l| l.find(')').map(|e| l[..=e].to_string()))
        .collect();
    format!("{}\n", fns.join(", "))
}

/// Run kernel tool `name` (dotted or underscored) with `args` (checked
/// against its schema by the registry). `open` is unused (a kernel check
/// always names its source). `Err` is a refusal to show the model.
pub fn call(name: &str, args: &Value, _open: Option<&str>) -> Result<Value, String> {
    match find(name).map(|tool| tool.api_name) {
        Some("kernel_guide") => Ok(json::obj(vec![("guide", json::s(guide()))])),
        Some("kernel_check") => check(args),
        _ => Err(format!("no kernel tool `{name}`")),
    }
}

fn problem(line: usize, col: usize, message: String, text: &str) -> Value {
    json::obj(vec![("line", Value::Int(line as i64)), ("col", Value::Int(col as i64)), ("message", json::s(message)), ("text", json::s(text.trim_end()))])
}

fn compile_problems(errs: &[ShaderError], src: &str) -> Value {
    let lines: Vec<&str> = src.lines().collect();
    Value::Arr(
        errs.iter()
            .take(20)
            .map(|e| {
                let (line, col) = e.line_col(src);
                let text = lines.get(line.saturating_sub(1)).copied().unwrap_or("");
                problem(line, col, e.message.clone(), text)
            })
            .collect(),
    )
}

fn field_ty(s: &str) -> Option<FieldTy> {
    Some(match s {
        "f32" | "float" => FieldTy::F32,
        "i32" | "int" | "u32" => FieldTy::I32,
        "vec2" => FieldTy::Vec2,
        "vec3" => FieldTy::Vec3,
        "vec4" => FieldTy::Vec4,
        "mat4" => FieldTy::Mat4,
        _ => return None,
    })
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Int(n) => Some(*n as f64),
        Value::F64(n) => Some(*n),
        _ => None,
    }
}

fn layouts_of(args: &Value) -> Result<Vec<Layout>, String> {
    let mut out = Vec::new();
    for l in args.get("layouts").and_then(Value::as_arr).unwrap_or(&[]) {
        let name = l.get("name").and_then(Value::as_str).ok_or("a layout needs a `name`")?;
        let stride = l.get("stride").and_then(num).ok_or_else(|| format!("layout `{name}` needs a `stride` (words)"))? as u32;
        let mut fields = Vec::new();
        for f in l.get("fields").and_then(Value::as_arr).unwrap_or(&[]) {
            let fname = f.get("name").and_then(Value::as_str).ok_or_else(|| format!("layout `{name}`: a field needs a `name`"))?;
            let ty = f.get("type").and_then(Value::as_str).unwrap_or("f32");
            let ty = field_ty(ty).ok_or_else(|| format!("layout `{name}`: field `{fname}` has type `{ty}`; use f32, i32, vec2, vec3, vec4 or mat4"))?;
            let offset = f.get("offset").and_then(num).ok_or_else(|| format!("layout `{name}`: field `{fname}` needs an `offset` (words)"))? as u32;
            fields.push(LayoutField { name: fname.to_string(), ty, offset });
        }
        out.push(Layout { name: name.to_string(), stride, fields });
    }
    Ok(out)
}

fn access_name(a: Access) -> String {
    match a {
        Access::Read => "read".into(),
        Access::Write => "write".into(),
        Access::Emit { width, capacity } => format!("emit {capacity} x {width} words"),
        Access::EmitCount => "emit count".into(),
    }
}

/// What a compiled kernel is, for the model.
fn describe(k: &Kernel) -> Value {
    let kind = match k.kind {
        KernelKind::Map => "map".to_string(),
        KernelKind::Reduce(op, lanes) => format!("reduce {op:?} ({lanes} lanes)").to_lowercase(),
    };
    let buffers = Value::Arr(
        k.buffers()
            .iter()
            .skip(1)
            .map(|b| json::obj(vec![("name", json::s(b.name.clone())), ("access", json::s(access_name(b.access))), ("stride", Value::Int(b.stride as i64))]))
            .collect(),
    );
    let params = Value::Arr(k.params().iter().map(|p| json::obj(vec![("name", json::s(p.name.clone())), ("default", Value::F64(p.default as f64)), ("min", Value::F64(p.min as f64)), ("max", Value::F64(p.max as f64))])).collect());
    let est = admission::estimate(k, 1_000_000, 8);
    let element_us = admission::element_ps(k, k.backend()) as f64 / 1e6;
    json::obj(vec![
        ("entry", json::s(k.entry.clone())),
        ("kind", json::s(kind)),
        ("math", json::s(if k.math == MathMode::Portable { "portable" } else { "fast" })),
        ("buffers", buffers),
        ("params", params),
        ("worst_ops_per_element", Value::Int(k.cost.min(i64::MAX as u64) as i64)),
        ("parallel", Value::Bool(k.parallel_safe)),
        ("four_wide", Value::Bool(k.simd())),
        ("worst_ms_per_million_on_8_threads", Value::F64((est.wall_ns as f64 / 1e6 * 100.0).round() / 100.0)),
        ("worst_us_per_element", Value::F64((element_us * 1000.0).round() / 1000.0)),
        ("untrusted_admissible", Value::Bool(element_us <= 1000.0)),
    ])
}

/// `kernel_check`.
pub fn check(args: &Value) -> Result<Value, String> {
    let src = args.get("source").and_then(Value::as_str).ok_or("give the kernel's `source`")?;
    let layouts = layouts_of(args)?;
    let mods: Vec<(String, String)> = args
        .get("modules")
        .and_then(Value::as_arr)
        .unwrap_or(&[])
        .iter()
        .filter_map(|m| Some((m.get("path")?.as_str()?.to_string(), m.get("source")?.as_str()?.to_string())))
        .collect();
    let modules: Vec<Module> = mods.iter().map(|(p, s)| Module { path: p, source: s }).collect();
    let kernel = match makepad_script_compute::kernel::compile_with_modules(src, &layouts, Backend::Native, &modules) {
        Ok(k) => k,
        Err(errs) => return Ok(json::obj(vec![("ok", Value::Bool(false)), ("problems", compile_problems(&errs, src))])),
    };
    let about = describe(&kernel);
    let (problems, run) = dry_run(&kernel, args);
    Ok(json::obj(vec![("ok", Value::Bool(problems.is_empty())), ("problems", Value::Arr(problems)), ("kernel", about), ("dry_run", run)]))
}

/// A deterministic ramp for an input the call did not give.
fn ramp(words: usize) -> Vec<u32> {
    (0..words).map(|k| ((k % 97) as f32 * 0.25 - 4.0).to_bits()).collect()
}

fn dry_run(k: &std::sync::Arc<Kernel>, args: &Value) -> (Vec<Value>, Value) {
    let n = args.get("count").and_then(num).unwrap_or(64.0).clamp(1.0, 4096.0) as usize;
    let mut problems = Vec::new();
    let mut job = Job::new(k.clone(), n);
    if let Some(Value::Obj(pairs)) = args.get("params") {
        for (name, v) in pairs {
            if let Some(x) = num(v) {
                if !job.set_param(name, x as f32) {
                    problems.push(problem(0, 0, format!("params: the kernel has no param `{name}`"), ""));
                }
            }
        }
    }
    let given = args.get("inputs");
    let mut written: Vec<(String, usize)> = Vec::new();
    let mut emits: Vec<(String, u32)> = Vec::new();
    for b in k.buffers().iter().skip(1) {
        let r = match b.access {
            Access::Read => {
                let words = match given.and_then(|g| g.get(&b.name)).and_then(Value::as_arr) {
                    Some(vals) if !vals.is_empty() => vals.iter().map(|v| (num(v).unwrap_or(0.0) as f32).to_bits()).collect(),
                    _ => ramp((n * b.stride as usize).max(64)),
                };
                job.input_vec_u32(&b.name, words)
            }
            Access::Write => {
                written.push((b.name.clone(), b.stride as usize));
                job.output_u32(&b.name, vec![0; n * b.stride as usize])
            }
            Access::Emit { width, capacity } => {
                emits.push((b.name.clone(), width));
                job.output_u32(&b.name, vec![0; n * (width * capacity) as usize])
            }
            Access::EmitCount => job.output_u32(&b.name, vec![0; n]),
        };
        if let Err(e) = r {
            problems.push(problem(0, 0, format!("dry run: {e}"), ""));
        }
    }
    // As untrusted work: admission and the watchdog bound what an AI's
    // kernel may cost.
    let budget = JobBudget { wall: Duration::from_millis(500) };
    let r = engine().scheduler().run_sync(&mut job, Origin::Ai, budget);
    let mut run = vec![("elements", Value::Int(n as i64))];
    match r {
        Err(e) => {
            problems.push(problem(0, 0, format!("dry run: {e}"), ""));
        }
        Ok(()) => {
            let stats = job.stats();
            run.push(("ns_per_element", Value::F64((stats.nanos as f64 / n as f64 * 10.0).round() / 10.0)));
            if stats.overflowed {
                problems.push(problem(0, 0, "dry run: an element emitted more records than its emit_buffer slots, or a loop with a run-time bound reached 1024 iterations".into(), ""));
            }
            if stats.host_error {
                problems.push(problem(0, 0, "dry run: a host component call failed (its results were zero)".into(), ""));
            }
            if !stats.reduced.is_empty() {
                run.push(("reduced", Value::Arr(stats.reduced.iter().map(|x| Value::F64(*x as f64)).collect())));
            }
            let mut outs = Vec::new();
            for (name, stride) in &written {
                let Some(words) = job.out_u32(name) else { continue };
                let bad = words.iter().filter(|w| !f32::from_bits(**w).is_finite()).count();
                if bad > 0 {
                    problems.push(problem(0, 0, format!("dry run: `{name}` has {bad} NaN or infinite words (if it holds floats: a division by zero, sqrt or log of a negative, or an unwritten input?)"), ""));
                }
                let first: Vec<Value> = words
                    .iter()
                    .take((*stride).min(16))
                    .map(|w| {
                        let x = f32::from_bits(*w);
                        if x.is_finite() { Value::F64(x as f64) } else { json::s(format!("{x}")) }
                    })
                    .collect();
                outs.push(json::obj(vec![("buffer", json::s(name.clone())), ("first_record", Value::Arr(first))]));
            }
            for (name, _) in &emits {
                let total: u64 = job.out_u32(&format!("{name}_count")).map_or(0, |c| c.iter().map(|x| *x as u64).sum());
                outs.push(json::obj(vec![("buffer", json::s(name.clone())), ("emitted", Value::Int(total as i64))]));
            }
            run.push(("outputs", Value::Arr(outs)));
        }
    }
    (problems, json::obj(run))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &str) -> Value {
        call("kernel_check", &json::parse(args.as_bytes()).unwrap(), None).unwrap()
    }

    #[test]
    fn the_check_reports_lines_facts_and_dry_run_findings() {
        // A compile error names its line and column and quotes the line.
        let v = run(r#"{"source": "let pos = output(vec3)\nfn vertex(i) {\n pos[i] = vec3(float(i), undefined_thing, 0.0) }"}"#);
        assert_eq!(v.get("ok"), Some(&Value::Bool(false)));
        let p = &v.get("problems").unwrap().as_arr().unwrap()[0];
        assert_eq!(p.get("line").and_then(Value::as_i64), Some(3));
        assert!(p.get("text").and_then(Value::as_str).unwrap().contains("undefined_thing"));
        // A good kernel: facts and a clean dry run.
        let v = run(r#"{"source": "let pos = output(vec3)\nlet amp = param(1)\nfn vertex(i) { pos[i] = vec3(float(i), amp, 0.0) }", "params": {"amp": 2}}"#);
        assert_eq!(v.get("ok"), Some(&Value::Bool(true)), "{}", v.to_json());
        let k = v.get("kernel").unwrap();
        assert_eq!(k.get("parallel"), Some(&Value::Bool(true)));
        assert_eq!(k.get("entry").and_then(Value::as_str), Some("vertex"));
        let first = v.get("dry_run").unwrap().get("outputs").unwrap().as_arr().unwrap()[0].get("first_record").unwrap().as_arr().unwrap().to_vec();
        assert_eq!(first[1], Value::F64(2.0));
        // NaN outputs, emit overflow and a capped run-time loop are found.
        let v = run(r#"{"source": "let o = output(f32)\nfn element(i) { o[i] = sqrt(-1.0 - float(i)) }"}"#);
        assert!(v.get("problems").unwrap().to_json().contains("NaN"), "{}", v.to_json());
        let v = run(r#"{"source": "let s = emit_buffer(2, 1)\nfn element(i) { emit(s, 1.0, 2.0)\n emit(s, 3.0, 4.0) }"}"#);
        assert!(v.get("problems").unwrap().to_json().contains("emitted more"), "{}", v.to_json());
        let v = run(r#"{"source": "let o = output(f32)\nlet n = param(5000)\nfn element(i) { let s = 0.0\n for k in 0..int(n) { s = s + 1.0 }\n o[i] = s }"}"#);
        assert!(v.get("problems").unwrap().to_json().contains("1024"), "{}", v.to_json());
        // A layout the call declares.
        let v = run(r#"{"source": "let inst = output(Inst)\nfn instance(i) { inst[i].pos = vec3(1.0, 2.0, 3.0)\n inst[i].id = i }", "layouts": [{"name": "Inst", "stride": 4, "fields": [{"name": "pos", "type": "vec3", "offset": 0}, {"name": "id", "type": "i32", "offset": 3}]}]}"#);
        assert_eq!(v.get("ok"), Some(&Value::Bool(true)), "{}", v.to_json());
        // Too slow for untrusted work: refused by admission, said so.
        let v = run(r#"{"source": "let o = output(f32)\nfn element(i) { let s = 0.0\n for a in 0..2000 { for b in 0..900 { s = s + sin(s) } }\n o[i] = s }"}"#);
        assert_eq!(v.get("ok"), Some(&Value::Bool(false)), "{}", v.to_json());
    }

    #[test]
    fn the_guide_lists_the_stdlib_and_host_components() {
        let g = guide();
        assert!(g.contains("### std.curve") && g.contains("fbm2(p, octaves, lacunarity, gain)") && g.contains("poly.triangulate"), "{}", &g[g.len() - 2000..]);
        assert!(g.contains("int(k) / 2"));
        // The card stays a card.
        assert!(g.len() < 24_000, "{} chars", g.len());
    }
}
