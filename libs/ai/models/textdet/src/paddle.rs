//! Lowering a Paddle inference program (`inference.json`, PIR) and its
//! parameters (`inference.pdiparams`) to our [`Graph`].
//!
//! The program is read as data and folded while lowering: bias adds, batch
//! norms and the scalar "learnable affine" blocks become part of the
//! convolution before them; an activation (and an affine after it) fuses into
//! the operation it follows when nothing else reads that operation's output.

use crate::ir::{Act, Graph, Op, Postprocess, Preprocess, Tensor};
use makepad_ai_loader::formats::paddle::{read_combined, PaddleTensor};
use makepad_micro_serde::{DeJson, JsonValue};
use std::collections::HashMap;
use std::path::Path;

type Id = u64;

fn obj(v: &JsonValue) -> Option<&HashMap<String, JsonValue>> {
    match v {
        JsonValue::Object(m) => Some(m),
        _ => None,
    }
}

fn arr(v: &JsonValue) -> Option<&Vec<JsonValue>> {
    match v {
        JsonValue::Array(a) => Some(a),
        _ => None,
    }
}

fn num(v: &JsonValue) -> Option<f64> {
    match v {
        JsonValue::U64(n) => Some(*n as f64),
        JsonValue::I64(n) => Some(*n as f64),
        JsonValue::F64(n) => Some(*n),
        _ => None,
    }
}

/// An attribute value with the `{"#": type, "D": value}` wrappers removed.
fn plain(v: &JsonValue) -> JsonValue {
    if let Some(m) = obj(v) {
        if let (Some(_), Some(d)) = (m.get("#"), m.get("D")) {
            return plain(d);
        }
    }
    if let Some(a) = arr(v) {
        return JsonValue::Array(a.iter().map(plain).collect());
    }
    v.clone()
}

struct PirOp {
    name: String,
    inputs: Vec<Id>,
    outputs: Vec<(Id, Vec<i64>)>,
    attrs: HashMap<String, JsonValue>,
    /// For a parameter op: its name.
    param: Option<String>,
}

impl PirOp {
    fn attr(&self, key: &str) -> Result<&JsonValue, String> {
        self.attrs.get(key).ok_or_else(|| format!("{}: attribute {key} missing", self.name))
    }
    fn ints(&self, key: &str) -> Result<Vec<i64>, String> {
        let v = self.attr(key)?;
        arr(v).ok_or_else(|| format!("{}: {key} is not a list", self.name))?
            .iter().map(|x| num(x).map(|n| n as i64).ok_or_else(|| format!("{}: {key} has a non-number", self.name))).collect()
    }
    fn int(&self, key: &str) -> Result<i64, String> {
        num(self.attr(key)?).map(|n| n as i64).ok_or_else(|| format!("{}: {key} is not a number", self.name))
    }
    fn float(&self, key: &str) -> Result<f32, String> {
        num(self.attr(key)?).map(|n| n as f32).ok_or_else(|| format!("{}: {key} is not a number", self.name))
    }
    fn flag(&self, key: &str) -> bool {
        matches!(self.attrs.get(key), Some(JsonValue::Bool(true)))
    }
    fn text(&self, key: &str) -> Option<&str> {
        match self.attrs.get(key) {
            Some(JsonValue::String(s)) => Some(s),
            _ => None,
        }
    }
    fn out(&self) -> Result<Id, String> {
        self.outputs.first().map(|o| o.0).ok_or_else(|| format!("{}: no output", self.name))
    }
}

fn ids(v: Option<&JsonValue>) -> Vec<Id> {
    v.and_then(arr).map(|a| a.iter().filter_map(|x| obj(x).and_then(|m| m.get("%")).and_then(num)).map(|n| n as Id).collect()).unwrap_or_default()
}

fn outputs(v: Option<&JsonValue>) -> Vec<(Id, Vec<i64>)> {
    let one = |x: &JsonValue| -> Option<(Id, Vec<i64>)> {
        let m = obj(x)?;
        let id = num(m.get("%")?)? as Id;
        // TT: {"#": "0.t_dtensor", "D": [dtype, dims, layout, lod, offset]}
        let dims = m.get("TT").and_then(obj).and_then(|t| t.get("D")).and_then(arr)
            .and_then(|d| d.get(1)).and_then(arr)
            .map(|d| d.iter().filter_map(num).map(|n| n as i64).collect()).unwrap_or_default();
        Some((id, dims))
    };
    match v {
        Some(JsonValue::Array(a)) => a.iter().filter_map(one).collect(),
        Some(x) => one(x).into_iter().collect(),
        None => Vec::new(),
    }
}

fn parse_program(text: &str) -> Result<Vec<PirOp>, String> {
    let root = JsonValue::deserialize_json(text).map_err(|e| format!("inference.json: {e:?}"))?;
    let ops = obj(&root).and_then(|r| r.get("program")).and_then(obj).and_then(|p| p.get("regions")).and_then(arr)
        .and_then(|r| r.first()).and_then(obj).and_then(|r| r.get("blocks")).and_then(arr)
        .and_then(|b| b.first()).and_then(obj).and_then(|b| b.get("ops")).and_then(arr)
        .ok_or("inference.json: no program/regions/blocks/ops")?;
    let mut out = Vec::new();
    for op in ops {
        let m = obj(op).ok_or("op is not an object")?;
        let name = match m.get("#") {
            Some(JsonValue::String(s)) => s.clone(),
            _ => return Err("op without a name".into()),
        };
        let mut attrs = HashMap::new();
        let mut param = None;
        if name == "p" {
            // Parameter: A = [_, _, _, name]
            if let Some(JsonValue::String(s)) = m.get("A").and_then(arr).and_then(|a| a.get(3)) {
                param = Some(s.clone());
            }
        } else if let Some(list) = m.get("A").and_then(arr) {
            for a in list {
                if let Some(am) = obj(a) {
                    if let (Some(JsonValue::String(n)), Some(at)) = (am.get("N"), am.get("AT")) {
                        attrs.insert(n.clone(), plain(at));
                    }
                }
            }
        }
        out.push(PirOp { name, inputs: ids(m.get("I")), outputs: outputs(m.get("O")), attrs, param });
    }
    Ok(out)
}

/// A value of the program while lowering.
#[derive(Clone)]
enum Val {
    /// A constant: parameter, reshaped parameter, integer list, scalar.
    Const(Vec<f32>),
    /// A feature map: IR value index, and whether it is `[C, 1, 1]`.
    Feat { value: usize, pointwise: bool },
    /// A list of values (`combine`).
    List(Vec<Id>),
}

struct Lower {
    ops: Vec<Op>,
    weights: Vec<Tensor>,
    vals: HashMap<Id, Val>,
    uses: HashMap<Id, usize>,
}

impl Lower {
    fn val(&self, id: Id) -> Result<&Val, String> {
        self.vals.get(&id).ok_or_else(|| format!("value %{id} is not defined"))
    }
    fn feat(&self, id: Id) -> Result<(usize, bool), String> {
        match self.val(id)? {
            Val::Feat { value, pointwise } => Ok((*value, *pointwise)),
            _ => Err(format!("value %{id} is not a feature map")),
        }
    }
    fn konst(&self, id: Id) -> Result<&[f32], String> {
        match self.val(id)? {
            Val::Const(c) => Ok(c),
            _ => Err(format!("value %{id} is not a constant")),
        }
    }
    fn push(&mut self, op: Op) -> usize {
        self.ops.push(op);
        self.ops.len()
    }
    fn weight(&mut self, shape: Vec<usize>, data: Vec<f32>) -> usize {
        self.weights.push(Tensor { shape, data });
        self.weights.len() - 1
    }
    /// The op that produced `id`, when it may be changed in place (it is the
    /// only reader of that value).
    fn foldable(&mut self, id: Id) -> Option<usize> {
        if self.uses.get(&id).copied().unwrap_or(0) != 1 {
            return None;
        }
        match self.vals.get(&id) {
            Some(Val::Feat { value, .. }) if *value > 0 => Some(*value - 1),
            _ => None,
        }
    }

    /// Per-output-channel `y * scale + shift` folded into a convolution (with
    /// no activation yet). `scale` and `shift` have one entry or one per channel.
    fn fold_affine(&mut self, op: usize, scale: &[f32], shift: &[f32]) -> bool {
        let (weight, bias, out, transposed) = match &self.ops[op] {
            Op::Conv { weight, bias, out_channels, act: Act::None, .. } => (*weight, *bias, *out_channels, false),
            Op::ConvTranspose { weight, bias, out_channels, act: Act::None, .. } => (*weight, *bias, *out_channels, true),
            _ => return false,
        };
        let pick = |v: &[f32], c: usize| if v.len() == 1 { v[0] } else { v[c] };
        if (scale.len() != 1 && scale.len() != out) || (shift.len() != 1 && shift.len() != out) {
            return false;
        }
        let w = &mut self.weights[weight];
        if transposed {
            // [in, out, kh, kw]
            let (inn, k) = (w.shape[0], w.shape[2] * w.shape[3]);
            for i in 0..inn {
                for c in 0..out {
                    for v in &mut w.data[(i * out + c) * k..(i * out + c + 1) * k] {
                        *v *= pick(scale, c);
                    }
                }
            }
        } else {
            let per = w.data.len() / out;
            for c in 0..out {
                for v in &mut w.data[c * per..(c + 1) * per] {
                    *v *= pick(scale, c);
                }
            }
        }
        let new_bias: Vec<f32> = (0..out).map(|c| {
            let b = bias.map_or(0.0, |b| self.weights[b].data[c]);
            b * pick(scale, c) + pick(shift, c)
        }).collect();
        let b = match bias {
            Some(b) => {
                self.weights[b].data = new_bias;
                b
            }
            None => self.weight(vec![out], new_bias),
        };
        match &mut self.ops[op] {
            Op::Conv { bias, .. } | Op::ConvTranspose { bias, .. } => *bias = Some(b),
            _ => unreachable!(),
        }
        true
    }

    fn act_of(&mut self, op: usize) -> Option<&mut Act> {
        match &mut self.ops[op] {
            Op::Conv { act, .. } | Op::ConvTranspose { act, .. } | Op::Add { act, .. } => Some(act),
            _ => None,
        }
    }

    /// Scalar `y * scale + shift` after an activation already fused into `op`.
    fn fold_post_affine(&mut self, op: usize, scale: f32, shift: f32) -> bool {
        let Some(act) = self.act_of(op) else { return false };
        *act = match *act {
            Act::HardSwish => Act::HardSwishAffine { scale, bias: shift },
            Act::HardSwishAffine { scale: s, bias: b } => Act::HardSwishAffine { scale: s * scale, bias: b * scale + shift },
            _ => return false,
        };
        true
    }

    /// `x * scale + shift` on feature `x` (scalars or per channel).
    fn affine(&mut self, x: Id, scale: &[f32], shift: &[f32], out: Id) -> Result<(), String> {
        let (value, pointwise) = self.feat(x)?;
        let op = self.foldable(x).ok_or_else(|| format!("affine on %{x}, which other ops also read"))?;
        let done = if matches!(self.act_of(op), Some(Act::None)) {
            self.fold_affine(op, scale, shift)
        } else if scale.len() == 1 && shift.len() == 1 {
            self.fold_post_affine(op, scale[0], shift[0])
        } else {
            false
        };
        if !done {
            return Err(format!("cannot fold an affine into op {op} ({:?})", self.ops[op]));
        }
        self.vals.insert(out, Val::Feat { value, pointwise });
        Ok(())
    }

    fn activation(&mut self, x: Id, act: Act, out: Id) -> Result<(), String> {
        let (value, pointwise) = self.feat(x)?;
        let fused = match self.foldable(x) {
            Some(op) => match self.act_of(op) {
                Some(a) if *a == Act::None => {
                    *a = act;
                    true
                }
                _ => false,
            },
            None => false,
        };
        let value = if fused { value } else { self.push(Op::Activate { input: value, act }) };
        self.vals.insert(out, Val::Feat { value, pointwise });
        Ok(())
    }
}

fn pads(p: &[i64], name: &str) -> Result<[usize; 4], String> {
    let u = |v: i64| usize::try_from(v).map_err(|_| format!("{name}: negative padding"));
    Ok(match p.len() {
        2 => [u(p[0])?, u(p[1])?, u(p[0])?, u(p[1])?],
        // Paddle's four-value form is [top, bottom, left, right].
        4 => [u(p[0])?, u(p[2])?, u(p[1])?, u(p[3])?],
        _ => return Err(format!("{name}: paddings {p:?}")),
    })
}

fn pair(p: &[i64], name: &str) -> Result<(usize, usize), String> {
    match p {
        [a, b] if *a > 0 && *b > 0 => Ok((*a as usize, *b as usize)),
        _ => Err(format!("{name}: expected two positive values, got {p:?}")),
    }
}

/// Lowers `inference.json` + `inference.pdiparams` from `dir`.
pub fn convert(dir: &Path) -> Result<Graph, String> {
    let text = std::fs::read_to_string(dir.join("inference.json")).map_err(|e| format!("inference.json: {e}"))?;
    let program = parse_program(&text)?;
    let tensors = read_combined(&dir.join("inference.pdiparams"))?;
    lower(&program, tensors)
}

fn lower(program: &[PirOp], tensors: Vec<PaddleTensor>) -> Result<Graph, String> {
    // save_combine writes the parameters sorted by name.
    let mut names: Vec<&str> = program.iter().filter_map(|o| o.param.as_deref()).collect();
    names.sort_unstable();
    if names.len() != tensors.len() {
        return Err(format!("{} parameters in the program, {} in the file", names.len(), tensors.len()));
    }
    let params: HashMap<&str, PaddleTensor> = names.into_iter().zip(tensors).collect();

    let mut uses = HashMap::new();
    for op in program {
        for i in &op.inputs {
            *uses.entry(*i).or_insert(0) += 1;
        }
    }
    let mut l = Lower { ops: Vec::new(), weights: Vec::new(), vals: HashMap::new(), uses };
    let mut output = None;
    for op in program {
        let n = op.name.as_str();
        let input = |k: usize| op.inputs.get(k).copied().ok_or_else(|| format!("{n}: input {k} missing"));
        match n {
            "p" => {
                let name = op.param.as_deref().ok_or("parameter without a name")?;
                let t = &params[name];
                let dims: Vec<i64> = op.outputs.first().map(|o| o.1.clone()).unwrap_or_default();
                if dims.iter().map(|d| *d as usize).collect::<Vec<_>>() != t.dims {
                    return Err(format!("parameter {name}: program says {dims:?}, file {:?}", t.dims));
                }
                l.vals.insert(op.out()?, Val::Const(t.to_f32()?));
            }
            "1.data" => {
                l.vals.insert(op.out()?, Val::Feat { value: 0, pointwise: false });
            }
            "1.full_int_array" => {
                let v = op.ints("value")?.into_iter().map(|x| x as f32).collect();
                l.vals.insert(op.out()?, Val::Const(v));
            }
            "1.full" => {
                l.vals.insert(op.out()?, Val::Const(vec![op.float("value")?]));
            }
            "1.reshape" => {
                let c = l.konst(input(0)?)?.to_vec();
                l.vals.insert(op.out()?, Val::Const(c));
            }
            "0.combine" => {
                l.vals.insert(op.out()?, Val::List(op.inputs.clone()));
            }
            "1.conv2d" | "1.depthwise_conv2d" => {
                let (x, pointwise) = l.feat(input(0)?)?;
                let wid = input(1)?;
                let w = l.konst(wid)?.to_vec();
                let wdims = &params_dims(program, wid)?;
                if op.ints("dilations")?.iter().any(|d| *d != 1) {
                    return Err(format!("{n}: dilation is not supported"));
                }
                if op.text("padding_algorithm") != Some("EXPLICIT") {
                    return Err(format!("{n}: padding algorithm {:?}", op.text("padding_algorithm")));
                }
                let groups = op.int("groups")? as usize;
                let shape: Vec<usize> = wdims.iter().map(|d| *d as usize).collect();
                let weight = l.weight(shape.clone(), w);
                let value = l.push(Op::Conv {
                    input: x, weight, bias: None, out_channels: shape[0], kernel: (shape[2], shape[3]),
                    stride: pair(&op.ints("strides")?, n)?, pad: pads(&op.ints("paddings")?, n)?, groups, act: Act::None,
                });
                l.vals.insert(op.out()?, Val::Feat { value, pointwise: pointwise && shape[2] == 1 && shape[3] == 1 });
            }
            "1.conv2d_transpose" => {
                let (x, _) = l.feat(input(0)?)?;
                let wid = input(1)?;
                let w = l.konst(wid)?.to_vec();
                let shape: Vec<usize> = params_dims(program, wid)?.iter().map(|d| *d as usize).collect();
                if op.int("groups")? != 1 || op.ints("paddings")?.iter().any(|p| *p != 0) || !op.ints("output_padding")?.is_empty() {
                    return Err(format!("{n}: only ungrouped, unpadded transposed convolutions are supported"));
                }
                let weight = l.weight(shape.clone(), w);
                let value = l.push(Op::ConvTranspose {
                    input: x, weight, bias: None, out_channels: shape[1], kernel: (shape[2], shape[3]),
                    stride: pair(&op.ints("strides")?, n)?, act: Act::None,
                });
                l.vals.insert(op.out()?, Val::Feat { value, pointwise: false });
            }
            "1.batch_norm_" => {
                let x = input(0)?;
                let (mean, var, gamma, beta) = (l.konst(input(1)?)?.to_vec(), l.konst(input(2)?)?.to_vec(), l.konst(input(3)?)?.to_vec(), l.konst(input(4)?)?.to_vec());
                let eps = op.float("epsilon")?;
                let scale: Vec<f32> = gamma.iter().zip(&var).map(|(g, v)| g / (v + eps).sqrt()).collect();
                let shift: Vec<f32> = beta.iter().zip(&mean).zip(&scale).map(|((b, m), s)| b - m * s).collect();
                l.affine(x, &scale, &shift, op.out()?)?;
            }
            "1.add" | "1.multiply" => {
                let (a, b) = (input(0)?, input(1)?);
                let is_feat = |id: Id| matches!(l.vals.get(&id), Some(Val::Feat { .. }));
                let out = op.out()?;
                match (is_feat(a), is_feat(b)) {
                    (true, true) => {
                        let ((va, pa), (vb, pb)) = (l.feat(a)?, l.feat(b)?);
                        let value = if n == "1.add" {
                            l.push(Op::Add { a: va, b: vb, act: Act::None })
                        } else if pb && !pa {
                            l.push(Op::Mul { x: va, s: vb })
                        } else if pa && !pb {
                            l.push(Op::Mul { x: vb, s: va })
                        } else {
                            l.push(Op::Mul { x: va, s: vb })
                        };
                        l.vals.insert(out, Val::Feat { value, pointwise: pa && pb });
                    }
                    (true, false) | (false, true) => {
                        let (x, c) = if is_feat(a) { (a, b) } else { (b, a) };
                        let c = l.konst(c)?.to_vec();
                        if n == "1.add" { l.affine(x, &[1.0], &c, out)? } else { l.affine(x, &c, &[0.0], out)? }
                    }
                    (false, false) => return Err(format!("{n} of two constants")),
                }
            }
            "1.relu" => l.activation(input(0)?, Act::Relu, op.out()?)?,
            "1.hardswish" => l.activation(input(0)?, Act::HardSwish, op.out()?)?,
            "1.sigmoid" => l.activation(input(0)?, Act::Sigmoid, op.out()?)?,
            "1.hardsigmoid" => {
                let act = Act::HardSigmoid { slope: op.float("slope")?, offset: op.float("offset")? };
                l.activation(input(0)?, act, op.out()?)?
            }
            "1.pool2d" => {
                let (x, _) = l.feat(input(0)?)?;
                let kernel = l.konst(input(1)?)?.to_vec();
                let avg = op.text("pooling_type") == Some("avg");
                let value = if (op.flag("adaptive") && kernel == [1.0, 1.0] && avg) || (op.flag("global_pooling") && avg) {
                    l.push(Op::GlobalAvgPool { input: x })
                } else if !op.flag("adaptive") && !op.flag("global_pooling") && kernel.len() == 2 && kernel[0] == kernel[1] {
                    let (s, p) = (op.ints("strides")?, op.ints("paddings")?);
                    if s[0] != s[1] || p.iter().any(|v| *v != p[0]) || op.flag("ceil_mode") {
                        return Err(format!("{n}: unsupported strides/paddings"));
                    }
                    l.push(Op::Pool { input: x, kernel: kernel[0] as usize, stride: s[0] as usize, pad: p[0] as usize, max: !avg })
                } else {
                    return Err(format!("{n}: unsupported pooling {:?}", op.attrs.keys().collect::<Vec<_>>()));
                };
                l.vals.insert(op.out()?, Val::Feat { value, pointwise: matches!(l.ops[value - 1], Op::GlobalAvgPool { .. }) });
            }
            "1.nearest_interp" => {
                let (x, _) = l.feat(input(0)?)?;
                if op.inputs.iter().skip(1).any(|i| *i != 0) || op.flag("align_corners") {
                    return Err(format!("{n}: only fixed integer scales are supported"));
                }
                let scale = match op.attr("scale")? {
                    JsonValue::Array(a) => a.iter().filter_map(num).collect::<Vec<_>>(),
                    _ => Vec::new(),
                };
                let s = match scale.as_slice() {
                    [a, b] if a == b && a.fract() == 0.0 && *a >= 1.0 => *a as usize,
                    _ => return Err(format!("{n}: scale {scale:?}")),
                };
                let value = l.push(Op::Upsample { input: x, scale: s });
                l.vals.insert(op.out()?, Val::Feat { value, pointwise: false });
            }
            "1.concat" => {
                let list = match l.val(input(0)?)? {
                    Val::List(v) => v.clone(),
                    _ => return Err("concat of a non-list".into()),
                };
                let axis = l.konst(input(1)?)?.to_vec();
                if axis != [1.0] {
                    return Err(format!("concat on axis {axis:?}"));
                }
                let inputs = list.iter().map(|i| l.feat(*i).map(|f| f.0)).collect::<Result<Vec<_>, _>>()?;
                let value = l.push(Op::Concat { inputs });
                l.vals.insert(op.out()?, Val::Feat { value, pointwise: false });
            }
            "1.fetch" => {
                output = Some(l.feat(input(0)?)?.0);
            }
            other => return Err(format!("unsupported Paddle op {other}")),
        }
    }
    let output = output.ok_or("the program fetches nothing")?;
    if output != l.ops.len() {
        return Err(format!("the fetched value {output} is not the last op ({})", l.ops.len()));
    }
    let graph = Graph { ops: l.ops, weights: l.weights, pre: Preprocess::default(), post: Postprocess::default() };
    graph.validate()?;
    Ok(graph)
}

/// Dimensions of the parameter (or reshaped parameter) with id `id`.
fn params_dims(program: &[PirOp], id: Id) -> Result<Vec<i64>, String> {
    program.iter().find_map(|o| o.outputs.iter().find(|x| x.0 == id).map(|x| x.1.clone()))
        .ok_or_else(|| format!("no dims for %{id}"))
}
