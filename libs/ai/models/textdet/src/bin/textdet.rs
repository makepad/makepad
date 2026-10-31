//! textdet convert <paddle model dir> <out.gguf>
//!     Lower a Paddle detection model (inference.json + inference.pdiparams).
//! textdet run <model.gguf> <picture.rgba> <width> <height> [prob.f32] [--cpu]
//!     Detect lines in a raw RGBA8 picture; print boxes, timings, GPU bytes, and
//!     optionally write the probability map (f32, network size).

use makepad_ai_textdet::{file, paddle, Frame, TextDetector};
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("convert") if args.len() == 3 => convert(&args[1], &args[2]),
        Some("run") if args.len() >= 5 => run(&args[1..]),
        Some("stats") if args.len() == 4 => stats(&args[1], &args[2], &args[3]),
        _ => Err("usage: textdet convert <dir> <out.gguf> | textdet run <model.gguf> <picture.rgba> <w> <h> [prob.f32] [--cpu]".into()),
    };
    if let Err(e) = result {
        eprintln!("textdet: {e}");
        std::process::exit(1);
    }
}

/// Per-op output size and multiply-adds for an input size: where the work is.
fn stats(model: &str, w: &str, h: &str) -> Result<(), String> {
    use makepad_ai_textdet::ir::Op;
    let g = file::load(Path::new(model))?;
    let (w, h): (usize, usize) = (w.parse().map_err(|_| "width")?, h.parse().map_err(|_| "height")?);
    let mut shapes = vec![(3usize, h, w)];
    let mut total = 0f64;
    for (i, op) in g.ops.iter().enumerate() {
        let (shape, macs) = match op {
            Op::Conv { input, weight, out_channels, kernel, stride, pad, groups, .. } => {
                let (c, ih, iw) = shapes[*input];
                let oh = (ih + pad[0] + pad[2] - kernel.0) / stride.0 + 1;
                let ow = (iw + pad[1] + pad[3] - kernel.1) / stride.1 + 1;
                let _ = weight;
                ((*out_channels, oh, ow), (*out_channels * oh * ow * (c / groups) * kernel.0 * kernel.1) as f64)
            }
            Op::ConvTranspose { input, out_channels, kernel, stride, .. } => {
                let (c, ih, iw) = shapes[*input];
                ((*out_channels, (ih - 1) * stride.0 + kernel.0, (iw - 1) * stride.1 + kernel.1), (c * ih * iw * out_channels * kernel.0 * kernel.1) as f64)
            }
            Op::GlobalAvgPool { input } => ((shapes[*input].0, 1, 1), 0.0),
            Op::Pool { input, kernel, stride, pad, .. } => {
                let (c, ih, iw) = shapes[*input];
                ((c, (ih + 2 * pad - kernel) / stride + 1, (iw + 2 * pad - kernel) / stride + 1), 0.0)
            }
            Op::Upsample { input, scale } => { let (c, ih, iw) = shapes[*input]; ((c, ih * scale, iw * scale), 0.0) }
            Op::UpsampleLike { input, like } => ((shapes[*input].0, shapes[*like].1, shapes[*like].2), 0.0),
            Op::Concat { inputs } => ((inputs.iter().map(|v| shapes[*v].0).sum(), shapes[inputs[0]].1, shapes[inputs[0]].2), 0.0),
            Op::Add { a, .. } => (shapes[*a], 0.0),
            Op::Mul { x, .. } => (shapes[*x], 0.0),
            Op::Activate { input, .. } => (shapes[*input], 0.0),
        };
        total += macs;
        let kind = format!("{op:?}");
        println!("{:3} {:>9.1} M  {:?}  {}", i, macs / 1e6, shape, &kind[..kind.find(|c| c == ' ' || c == '{').unwrap_or(kind.len())]);
        shapes.push(shape);
    }
    println!("total {:.2} G multiply-adds", total / 1e9);
    Ok(())
}

fn convert(dir: &str, out: &str) -> Result<(), String> {
    let graph = paddle::convert(Path::new(dir))?;
    let params: usize = graph.weights.iter().map(|w| w.data.len()).sum();
    file::save(&graph, Path::new(out), "PP-OCRv5_mobile_det (PaddlePaddle, Apache-2.0)")?;
    println!("{} ops, {} weights ({} floats) -> {out}", graph.ops.len(), graph.weights.len(), params);
    Ok(())
}

fn run(a: &[String]) -> Result<(), String> {
    let graph = file::load(Path::new(&a[0]))?;
    let (w, h): (u32, u32) = (a[2].parse().map_err(|_| "width")?, a[3].parse().map_err(|_| "height")?);
    let rgba = std::fs::read(&a[1]).map_err(|e| format!("{}: {e}", a[1]))?;
    let cpu = a.iter().any(|x| x == "--cpu");
    let mut det = if cpu { TextDetector::cpu(graph)? } else { TextDetector::from_graph(graph)? };
    let frame = Frame { width: w, height: h, rgba: &rgba };
    // First call builds pipelines and plans buffers; time the second too.
    for pass in 0..3 {
        let boxes = det.detect(&frame)?;
        let t = det.last_timings();
        println!("pass {pass} [{}] input {}x{}: prepare {:.1} ms, network {:.1} ms, boxes {:.1} ms, {} lines, gpu_bytes {}",
            det.backend(), t.input.0, t.input.1, t.prepare_ms, t.network_ms, t.boxes_ms, boxes.len(), det.gpu_bytes());
        if pass == 2 {
            for b in &boxes {
                println!("  box x {:.4} y {:.4} w {:.4} h {:.4} score {:.3}", b.x, b.y, b.w, b.h, b.score);
            }
        }
    }
    if let Some(path) = a.get(4).filter(|p| !p.starts_with("--")) {
        let prep = makepad_ai_textdet::post::prepare(w as usize, h as usize, &rgba, &det.graph().pre)?;
        let prob = det.probability(&prep.input)?;
        let bytes: Vec<u8> = prob.data.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(path, bytes).map_err(|e| e.to_string())?;
        // The prepared input too, so a reference can run on exactly it.
        let input: Vec<u8> = prep.input.data.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(format!("{path}.input"), input).map_err(|e| e.to_string())?;
        println!("probability map {}x{} -> {path} (input -> {path}.input)", prob.w, prob.h);
    }
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines().filter(|l| l.starts_with("VmHWM") || l.starts_with("VmRSS")) {
            println!("{line}");
        }
    }
    Ok(())
}
