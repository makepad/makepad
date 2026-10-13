//! The detector file: GGUF with the op list as a u32 array (`textdet.ops`),
//! pre/post-processing settings as metadata, and one F32 tensor per weight
//! (`w.<index>`, dimensions innermost first as GGUF stores them).
//!
//! Op encoding: `[tag, field count, fields...]`, floats as their bits.

use crate::ir::{Act, Graph, Op, Postprocess, Preprocess, Tensor};
use makepad_ai_loader::formats::gguf::{GgufArray, GgufFile, GgufKeyValue, GgufString, GgufValue};
use makepad_ai_loader::formats::gguf_write::{self, TensorSpec};
use makepad_ai_loader::quant::TensorType;
use std::path::Path;

pub const ARCH: &str = "textdet-db";

fn act_words(act: Act) -> [u32; 3] {
    let (code, a, b) = act.code();
    [code, a.to_bits(), b.to_bits()]
}

fn read_act(w: &[u32]) -> Result<Act, String> {
    Act::from_code(w[0], f32::from_bits(w[1]), f32::from_bits(w[2])).ok_or_else(|| format!("unknown activation {}", w[0]))
}

const NONE: u32 = u32::MAX;

fn opt(v: Option<usize>) -> u32 {
    v.map_or(NONE, |v| v as u32)
}

fn read_opt(v: u32) -> Option<usize> {
    (v != NONE).then_some(v as usize)
}

pub fn encode_ops(ops: &[Op]) -> Vec<u32> {
    let mut out = Vec::new();
    for op in ops {
        let (tag, fields): (u32, Vec<u32>) = match op {
            Op::Conv { input, weight, bias, out_channels, kernel, stride, pad, groups, act } => {
                let mut f = vec![*input as u32, *weight as u32, opt(*bias), *out_channels as u32, kernel.0 as u32, kernel.1 as u32,
                    stride.0 as u32, stride.1 as u32, pad[0] as u32, pad[1] as u32, pad[2] as u32, pad[3] as u32, *groups as u32];
                f.extend(act_words(*act));
                (1, f)
            }
            Op::ConvTranspose { input, weight, bias, out_channels, kernel, stride, act } => {
                let mut f = vec![*input as u32, *weight as u32, opt(*bias), *out_channels as u32, kernel.0 as u32, kernel.1 as u32, stride.0 as u32, stride.1 as u32];
                f.extend(act_words(*act));
                (2, f)
            }
            Op::Add { a, b, act } => {
                let mut f = vec![*a as u32, *b as u32];
                f.extend(act_words(*act));
                (3, f)
            }
            Op::Mul { x, s } => (4, vec![*x as u32, *s as u32]),
            Op::GlobalAvgPool { input } => (5, vec![*input as u32]),
            Op::Pool { input, kernel, stride, pad, max } => (6, vec![*input as u32, *kernel as u32, *stride as u32, *pad as u32, *max as u32]),
            Op::UpsampleLike { input, like } => (7, vec![*input as u32, *like as u32]),
            Op::Upsample { input, scale } => (8, vec![*input as u32, *scale as u32]),
            Op::Concat { inputs } => (9, inputs.iter().map(|v| *v as u32).collect()),
            Op::Activate { input, act } => {
                let mut f = vec![*input as u32];
                f.extend(act_words(*act));
                (10, f)
            }
        };
        out.push(tag);
        out.push(fields.len() as u32);
        out.extend(fields);
    }
    out
}

pub fn decode_ops(words: &[u32]) -> Result<Vec<Op>, String> {
    let mut ops = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let tag = words[i];
        let n = *words.get(i + 1).ok_or("truncated op list")? as usize;
        let f = words.get(i + 2..i + 2 + n).ok_or("truncated op fields")?;
        let need = |k: usize| if n == k { Ok(()) } else { Err(format!("op tag {tag} has {n} fields, expected {k}")) };
        let u = |k: usize| f[k] as usize;
        ops.push(match tag {
            1 => {
                need(16)?;
                Op::Conv { input: u(0), weight: u(1), bias: read_opt(f[2]), out_channels: u(3), kernel: (u(4), u(5)), stride: (u(6), u(7)),
                    pad: [u(8), u(9), u(10), u(11)], groups: u(12), act: read_act(&f[13..16])? }
            }
            2 => {
                need(11)?;
                Op::ConvTranspose { input: u(0), weight: u(1), bias: read_opt(f[2]), out_channels: u(3), kernel: (u(4), u(5)), stride: (u(6), u(7)), act: read_act(&f[8..11])? }
            }
            3 => {
                need(5)?;
                Op::Add { a: u(0), b: u(1), act: read_act(&f[2..5])? }
            }
            4 => { need(2)?; Op::Mul { x: u(0), s: u(1) } }
            5 => { need(1)?; Op::GlobalAvgPool { input: u(0) } }
            6 => { need(5)?; Op::Pool { input: u(0), kernel: u(1), stride: u(2), pad: u(3), max: f[4] != 0 } }
            7 => { need(2)?; Op::UpsampleLike { input: u(0), like: u(1) } }
            8 => { need(2)?; Op::Upsample { input: u(0), scale: u(1) } }
            9 => Op::Concat { inputs: f.iter().map(|v| *v as usize).collect() },
            10 => {
                need(4)?;
                Op::Activate { input: u(0), act: read_act(&f[1..4])? }
            }
            _ => return Err(format!("unknown op tag {tag}")),
        });
        i += 2 + n;
    }
    Ok(ops)
}

fn kv(key: &str, value: GgufValue) -> GgufKeyValue {
    GgufKeyValue { key: key.to_string(), value }
}

/// Writes `graph` to `path` (which must not exist).
pub fn save(graph: &Graph, path: &Path, source: &str) -> Result<(), String> {
    graph.validate()?;
    let pre = &graph.pre;
    let post = &graph.post;
    let meta = vec![
        kv("general.architecture", GgufValue::String(GgufString::new(ARCH.as_bytes().to_vec()))),
        kv("general.source", GgufValue::String(GgufString::new(source.as_bytes().to_vec()))),
        kv("textdet.ops", GgufValue::Array(GgufArray::Uint32(encode_ops(&graph.ops)))),
        kv("textdet.max_side", GgufValue::Uint32(pre.max_side as u32)),
        kv("textdet.multiple", GgufValue::Uint32(pre.multiple as u32)),
        kv("textdet.mean", GgufValue::Array(GgufArray::Float32(pre.mean.to_vec()))),
        kv("textdet.std", GgufValue::Array(GgufArray::Float32(pre.std.to_vec()))),
        kv("textdet.bgr", GgufValue::Bool(pre.bgr)),
        kv("textdet.thresh", GgufValue::Float32(post.thresh)),
        kv("textdet.box_thresh", GgufValue::Float32(post.box_thresh)),
        kv("textdet.unclip", GgufValue::Float32(post.unclip)),
        kv("textdet.min_size", GgufValue::Float32(post.min_size)),
    ];
    let specs: Vec<TensorSpec> = graph.weights.iter().enumerate().map(|(i, t)| TensorSpec {
        name: format!("w.{i}"),
        dimensions: t.shape.iter().rev().map(|d| *d as u64).collect(),
        tensor_type: TensorType::F32,
    }).collect();
    let layout = gguf_write::create(path, &meta, &specs).map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new().write(true).open(path).map_err(|e| e.to_string())?;
    for (i, t) in graph.weights.iter().enumerate() {
        let bytes: Vec<u8> = t.data.iter().flat_map(|v| v.to_le_bytes()).collect();
        gguf_write::write_range(&mut file, &layout, i, 0, &bytes).map_err(|e| e.to_string())?;
    }
    file.sync_all().map_err(|e| e.to_string())
}

/// Reads a detector file.
pub fn load(path: &Path) -> Result<Graph, String> {
    let gguf = GgufFile::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let arch = gguf.get_value("general.architecture").and_then(|v| v.as_string()).map(|s| s.to_string_lossy().into_owned());
    if arch.as_deref() != Some(ARCH) {
        return Err(format!("{}: not a text detector file ({arch:?})", path.display()));
    }
    let words = gguf.get_value("textdet.ops").and_then(|v| v.as_array()).and_then(|a| a.as_u32_slice()).ok_or("textdet.ops missing")?;
    let ops = decode_ops(words)?;
    let u = |k: &str| gguf.get_value(k).and_then(|v| v.as_u32()).ok_or_else(|| format!("{k} missing"));
    let f = |k: &str| gguf.get_value(k).and_then(|v| v.as_f32()).ok_or_else(|| format!("{k} missing"));
    let three = |k: &str| -> Result<[f32; 3], String> {
        match gguf.get_value(k).and_then(|v| v.as_array()) {
            Some(GgufArray::Float32(v)) if v.len() == 3 => Ok([v[0], v[1], v[2]]),
            _ => Err(format!("{k} missing")),
        }
    };
    let bgr = matches!(gguf.get_value("textdet.bgr"), Some(GgufValue::Bool(true)));
    let pre = Preprocess { max_side: u("textdet.max_side")? as usize, multiple: u("textdet.multiple")? as usize, mean: three("textdet.mean")?, std: three("textdet.std")?, bgr };
    let post = Postprocess { thresh: f("textdet.thresh")?, box_thresh: f("textdet.box_thresh")?, unclip: f("textdet.unclip")?, min_size: f("textdet.min_size")? };
    let mut weights = Vec::new();
    for i in 0.. {
        let name = format!("w.{i}");
        let Some(info) = gguf.get_tensor(&name) else { break };
        if info.tensor_type != TensorType::F32 {
            return Err(format!("{name}: expected F32"));
        }
        let shape: Vec<usize> = info.dimensions.iter().rev().map(|d| *d as usize).collect();
        let bytes = gguf.read_tensor_bytes(&name).map_err(|e| e.to_string())?;
        let data = bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        weights.push(Tensor { shape, data });
    }
    let graph = Graph { ops, weights, pre, post };
    graph.validate()?;
    Ok(graph)
}
