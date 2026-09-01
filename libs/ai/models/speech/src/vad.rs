//! Silero VAD v5 (16 kHz path) in plain Rust.
//!
//! Loads the official `silero_vad.onnx` released by the silero-vad project and
//! runs its 16 kHz branch directly: a tiny weight file (~2.3 MB) whose graph is
//! fixed, so instead of an ONNX runtime we parse just the weight tensors out of
//! the protobuf and hard-code the network: STFT-as-conv -> magnitude ->
//! 4x (conv1d + relu) -> LSTM cell -> conv head -> sigmoid. One 512-sample
//! chunk (32 ms) in, one speech probability out. Costs ~0.3 MFLOP per chunk —
//! microseconds on any CPU, so it can gate audio packets in real time.
//!
//! The model resolves like the other voice models: `MAKEPAD_VAD_MODEL` env var
//! first, else `silero_vad.onnx` in the working directory.
//!
//! Validated against onnxruntime output; see `tests/silero_vad.rs`.

use std::collections::VecDeque;

/// Silero operates on 16 kHz mono audio.
pub const VAD_SAMPLE_RATE: usize = 16_000;
/// One VAD evaluation consumes exactly this many samples (32 ms).
pub const VAD_CHUNK_SAMPLES: usize = 512;

/// Samples carried over from the previous chunk, prepended to each new one.
const CONTEXT_SAMPLES: usize = 64;
/// Reflection padding appended before the STFT.
const TAIL_PAD: usize = 64;
const MERGED_LEN: usize = CONTEXT_SAMPLES + VAD_CHUNK_SAMPLES; // 576
const PADDED_LEN: usize = MERGED_LEN + TAIL_PAD; // 640

const STFT_FILTERS: usize = 258; // 129 real + 129 imaginary
const STFT_BINS: usize = 129;
const STFT_KERNEL: usize = 256;
const STFT_HOP: usize = 128;
const STFT_FRAMES: usize = (PADDED_LEN - STFT_KERNEL) / STFT_HOP + 1; // 4
const HIDDEN: usize = 128;

const MODEL_PATH_ENV: &str = "MAKEPAD_VAD_MODEL";
const DEFAULT_MODEL_PATH: &str = "silero_vad.onnx";

/// The model path, if the file actually exists.
pub fn vad_model_path_if_present() -> Option<String> {
    if let Ok(path) = std::env::var(MODEL_PATH_ENV) {
        if std::path::Path::new(&path).is_file() {
            return Some(path);
        }
    }
    if std::path::Path::new(DEFAULT_MODEL_PATH).is_file() {
        return Some(DEFAULT_MODEL_PATH.to_string());
    }
    None
}

#[derive(Debug)]
pub enum VadError {
    Io(String),
    Parse(String),
    MissingTensor(String),
    BadShape(String),
}

/// A conv1d layer's weights: `[c_out][c_in][kernel]` flattened, plus bias.
struct Conv1d {
    weight: Vec<f32>,
    bias: Vec<f32>,
    c_in: usize,
    c_out: usize,
    kernel: usize,
    stride: usize,
    pad: usize,
}

impl Conv1d {
    /// Standard 1D convolution with zero padding, optional ReLU. Input and
    /// output are `[channel][time]` flattened. Returns the output frame count.
    fn apply(&self, input: &[f32], t_in: usize, relu: bool, out: &mut Vec<f32>) -> usize {
        let t_out = (t_in + 2 * self.pad - self.kernel) / self.stride + 1;
        out.resize(self.c_out * t_out, 0.0);
        for co in 0..self.c_out {
            let w_co = &self.weight[co * self.c_in * self.kernel..(co + 1) * self.c_in * self.kernel];
            for t in 0..t_out {
                let mut acc = self.bias[co];
                let start = (t * self.stride) as isize - self.pad as isize;
                for ci in 0..self.c_in {
                    let w = &w_co[ci * self.kernel..ci * self.kernel + self.kernel];
                    let x = &input[ci * t_in..ci * t_in + t_in];
                    for k in 0..self.kernel {
                        let ti = start + k as isize;
                        if ti >= 0 && (ti as usize) < t_in {
                            acc += w[k] * x[ti as usize];
                        }
                    }
                }
                out[co * t_out + t] = if relu { acc.max(0.0) } else { acc };
            }
        }
        t_out
    }
}

pub struct SileroVad {
    /// STFT analysis filters: `[258][256]`, applied at stride 128, no bias.
    stft_basis: Vec<f32>,
    encoder: [Conv1d; 4],
    /// LSTM cell weights, PyTorch layout: `[4*HIDDEN][HIDDEN]` with gate order
    /// input/forget/cell/output.
    rnn_w_ih: Vec<f32>,
    rnn_w_hh: Vec<f32>,
    rnn_b_ih: Vec<f32>,
    rnn_b_hh: Vec<f32>,
    /// Final 1x1 conv head: `[HIDDEN]` weights + bias, then sigmoid.
    head_w: Vec<f32>,
    head_b: f32,

    // Streaming state.
    context: [f32; CONTEXT_SAMPLES],
    h: [f32; HIDDEN],
    c: [f32; HIDDEN],

    // Scratch, allocated once.
    padded: Vec<f32>,
    stft_out: Vec<f32>,
    mag: Vec<f32>,
    buf_a: Vec<f32>,
    buf_b: Vec<f32>,
    gates: Vec<f32>,
}

impl SileroVad {
    /// Load from the model path resolved by [`vad_model_path_if_present`].
    pub fn from_makepad_env() -> Result<Self, VadError> {
        let path = vad_model_path_if_present().ok_or_else(|| {
            VadError::Io(format!(
                "no VAD model: set {MODEL_PATH_ENV} or put {DEFAULT_MODEL_PATH} in the working directory"
            ))
        })?;
        Self::load(&path)
    }

    pub fn load(path: &str) -> Result<Self, VadError> {
        let bytes =
            std::fs::read(path).map_err(|err| VadError::Io(format!("{path}: {err}")))?;
        Self::from_onnx_bytes(&bytes)
    }

    /// Parse the ONNX file and pull out the 16 kHz branch weights.
    pub fn from_onnx_bytes(bytes: &[u8]) -> Result<Self, VadError> {
        let mut tensors = Vec::new();
        let mut model = Pb::new(bytes);
        while !model.done() {
            let (field, wire) = model.key()?;
            if field == 7 && wire == 2 {
                collect_constants(model.bytes()?, &mut tensors)?;
            } else {
                model.skip(wire)?;
            }
        }

        // Both sample-rate branches carry the same tensor names; the
        // `then_branch` of the top-level If is the 16 kHz one.
        let find = |suffix: &str| -> Result<RawTensor, VadError> {
            for (name, tensor) in &tensors {
                if name.contains("then_branch") && name.ends_with(suffix) {
                    return Ok(tensor.clone());
                }
            }
            Err(VadError::MissingTensor(suffix.to_string()))
        };
        let expect = |tensor: RawTensor, dims: &[usize], what: &str| -> Result<Vec<f32>, VadError> {
            if tensor.dims != dims {
                return Err(VadError::BadShape(format!(
                    "{what}: got {:?}, want {:?}",
                    tensor.dims, dims
                )));
            }
            Ok(tensor.data)
        };

        let stft_basis = expect(
            find("stft.forward_basis_buffer")?,
            &[STFT_FILTERS, 1, STFT_KERNEL],
            "stft basis",
        )?;
        let enc_dims: [(usize, usize, usize); 4] = [
            (128, STFT_BINS, 1),
            (64, 128, 2),
            (64, 64, 2),
            (128, 64, 1),
        ];
        let mut encoder = Vec::new();
        for (index, (c_out, c_in, stride)) in enc_dims.iter().enumerate() {
            let weight = expect(
                find(&format!("encoder.{index}.reparam_conv.weight"))?,
                &[*c_out, *c_in, 3],
                "encoder weight",
            )?;
            let bias = expect(
                find(&format!("encoder.{index}.reparam_conv.bias"))?,
                &[*c_out],
                "encoder bias",
            )?;
            encoder.push(Conv1d {
                weight,
                bias,
                c_in: *c_in,
                c_out: *c_out,
                kernel: 3,
                stride: *stride,
                pad: 1,
            });
        }
        let encoder: [Conv1d; 4] = encoder
            .try_into()
            .map_err(|_| VadError::Parse("encoder collect".into()))?;

        let rnn_w_ih = expect(find("decoder.rnn.weight_ih")?, &[4 * HIDDEN, HIDDEN], "rnn w_ih")?;
        let rnn_w_hh = expect(find("decoder.rnn.weight_hh")?, &[4 * HIDDEN, HIDDEN], "rnn w_hh")?;
        let rnn_b_ih = expect(find("decoder.rnn.bias_ih")?, &[4 * HIDDEN], "rnn b_ih")?;
        let rnn_b_hh = expect(find("decoder.rnn.bias_hh")?, &[4 * HIDDEN], "rnn b_hh")?;
        let head_w = expect(find("decoder.decoder.2.weight")?, &[1, HIDDEN, 1], "head weight")?;
        let head_b = expect(find("decoder.decoder.2.bias")?, &[1], "head bias")?[0];

        Ok(Self {
            stft_basis,
            encoder,
            rnn_w_ih,
            rnn_w_hh,
            rnn_b_ih,
            rnn_b_hh,
            head_w,
            head_b,
            context: [0.0; CONTEXT_SAMPLES],
            h: [0.0; HIDDEN],
            c: [0.0; HIDDEN],
            padded: vec![0.0; PADDED_LEN],
            stft_out: vec![0.0; STFT_FILTERS * STFT_FRAMES],
            mag: vec![0.0; STFT_BINS * STFT_FRAMES],
            buf_a: Vec::new(),
            buf_b: Vec::new(),
            gates: vec![0.0; 4 * HIDDEN],
        })
    }

    /// Forget all streaming state (context and LSTM memory). Call between
    /// unrelated audio streams.
    pub fn reset(&mut self) {
        self.context = [0.0; CONTEXT_SAMPLES];
        self.h = [0.0; HIDDEN];
        self.c = [0.0; HIDDEN];
    }

    /// Speech probability (0..1) for the next 512 samples of the stream.
    pub fn process_chunk(&mut self, chunk: &[f32]) -> f32 {
        assert_eq!(
            chunk.len(),
            VAD_CHUNK_SAMPLES,
            "SileroVad processes exactly {VAD_CHUNK_SAMPLES}-sample chunks"
        );

        // Previous-chunk context, this chunk, then a reflected tail.
        self.padded[..CONTEXT_SAMPLES].copy_from_slice(&self.context);
        self.padded[CONTEXT_SAMPLES..MERGED_LEN].copy_from_slice(chunk);
        for i in 0..TAIL_PAD {
            self.padded[MERGED_LEN + i] = self.padded[MERGED_LEN - 2 - i];
        }

        // STFT as strided convolution, then per-bin magnitude.
        for f in 0..STFT_FILTERS {
            let w = &self.stft_basis[f * STFT_KERNEL..(f + 1) * STFT_KERNEL];
            for t in 0..STFT_FRAMES {
                let x = &self.padded[t * STFT_HOP..t * STFT_HOP + STFT_KERNEL];
                let mut acc = 0.0;
                for k in 0..STFT_KERNEL {
                    acc += w[k] * x[k];
                }
                self.stft_out[f * STFT_FRAMES + t] = acc;
            }
        }
        for bin in 0..STFT_BINS {
            for t in 0..STFT_FRAMES {
                let re = self.stft_out[bin * STFT_FRAMES + t];
                let im = self.stft_out[(STFT_BINS + bin) * STFT_FRAMES + t];
                self.mag[bin * STFT_FRAMES + t] = (re * re + im * im).sqrt();
            }
        }

        // Encoder: conv+relu x4 shrinks 4 frames down to 1.
        let mut scratch_a = std::mem::take(&mut self.buf_a);
        let mut scratch_b = std::mem::take(&mut self.buf_b);
        let mut t = self.encoder[0].apply(&self.mag, STFT_FRAMES, true, &mut scratch_a);
        t = self.encoder[1].apply(&scratch_a, t, true, &mut scratch_b);
        t = self.encoder[2].apply(&scratch_b, t, true, &mut scratch_a);
        t = self.encoder[3].apply(&scratch_a, t, true, &mut scratch_b);
        debug_assert_eq!(t, 1);
        let features = &scratch_b[..HIDDEN];

        // LSTM cell, PyTorch gate order: input, forget, cell, output.
        for g in 0..4 * HIDDEN {
            let mut acc = self.rnn_b_ih[g] + self.rnn_b_hh[g];
            let w_ih = &self.rnn_w_ih[g * HIDDEN..(g + 1) * HIDDEN];
            let w_hh = &self.rnn_w_hh[g * HIDDEN..(g + 1) * HIDDEN];
            for j in 0..HIDDEN {
                acc += w_ih[j] * features[j] + w_hh[j] * self.h[j];
            }
            self.gates[g] = acc;
        }
        let mut prob_in = 0.0;
        for j in 0..HIDDEN {
            let i_gate = sigmoid(self.gates[j]);
            let f_gate = sigmoid(self.gates[HIDDEN + j]);
            let g_gate = self.gates[2 * HIDDEN + j].tanh();
            let o_gate = sigmoid(self.gates[3 * HIDDEN + j]);
            let c_new = f_gate * self.c[j] + i_gate * g_gate;
            let h_new = o_gate * c_new.tanh();
            self.c[j] = c_new;
            self.h[j] = h_new;
            // Head: 1x1 conv over relu(h), all frames (there is exactly one).
            prob_in += self.head_w[j] * h_new.max(0.0);
        }
        let prob = sigmoid(prob_in + self.head_b);

        self.context
            .copy_from_slice(&chunk[VAD_CHUNK_SAMPLES - CONTEXT_SAMPLES..]);
        self.buf_a = scratch_a;
        self.buf_b = scratch_b;
        prob
    }
}

#[inline]
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Feeds arbitrary-sized sample packets into [`SileroVad`]'s fixed chunk size.
pub struct VadStream {
    vad: SileroVad,
    pending: VecDeque<f32>,
    prob: f32,
}

impl VadStream {
    pub fn new(vad: SileroVad) -> Self {
        Self {
            vad,
            pending: VecDeque::new(),
            prob: 0.0,
        }
    }

    /// Push samples; returns the newest speech probability if one or more
    /// full chunks were evaluated, `None` if still buffering.
    pub fn push(&mut self, samples: &[f32]) -> Option<f32> {
        self.pending.extend(samples.iter().copied());
        let mut updated = None;
        let mut chunk = [0.0f32; VAD_CHUNK_SAMPLES];
        while self.pending.len() >= VAD_CHUNK_SAMPLES {
            for sample in chunk.iter_mut() {
                *sample = self.pending.pop_front().unwrap();
            }
            self.prob = self.vad.process_chunk(&chunk);
            updated = Some(self.prob);
        }
        updated
    }

    /// The most recent speech probability.
    pub fn prob(&self) -> f32 {
        self.prob
    }

    pub fn reset(&mut self) {
        self.vad.reset();
        self.pending.clear();
        self.prob = 0.0;
    }
}

// ---------------------------------------------------------------------------
// Minimal protobuf reading: just enough to find Constant tensors in the graph.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct RawTensor {
    dims: Vec<usize>,
    data: Vec<f32>,
}

struct Pb<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Pb<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn done(&self) -> bool {
        self.pos >= self.data.len()
    }

    fn varint(&mut self) -> Result<u64, VadError> {
        let mut out = 0u64;
        let mut shift = 0u32;
        loop {
            let byte = *self
                .data
                .get(self.pos)
                .ok_or_else(|| VadError::Parse("varint past end".into()))?;
            self.pos += 1;
            if shift < 64 {
                out |= ((byte & 0x7f) as u64) << shift;
            }
            if byte & 0x80 == 0 {
                return Ok(out);
            }
            shift += 7;
        }
    }

    fn key(&mut self) -> Result<(u64, u32), VadError> {
        let key = self.varint()?;
        Ok((key >> 3, (key & 7) as u32))
    }

    fn bytes(&mut self) -> Result<&'a [u8], VadError> {
        let len = self.varint()? as usize;
        let end = self
            .pos
            .checked_add(len)
            .filter(|end| *end <= self.data.len())
            .ok_or_else(|| VadError::Parse("length past end".into()))?;
        let out = &self.data[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn skip(&mut self, wire: u32) -> Result<(), VadError> {
        match wire {
            0 => {
                self.varint()?;
            }
            1 => self.pos += 8,
            2 => {
                self.bytes()?;
            }
            5 => self.pos += 4,
            _ => return Err(VadError::Parse(format!("wire type {wire}"))),
        }
        if self.pos > self.data.len() {
            return Err(VadError::Parse("skip past end".into()));
        }
        Ok(())
    }
}

/// Walk a GraphProto, recording every float tensor held by a Constant node
/// under its output name, recursing into If/Loop subgraphs.
fn collect_constants(
    graph: &[u8],
    out: &mut Vec<(String, RawTensor)>,
) -> Result<(), VadError> {
    let mut pb = Pb::new(graph);
    while !pb.done() {
        let (field, wire) = pb.key()?;
        // GraphProto.node = 1
        if field == 1 && wire == 2 {
            parse_node(pb.bytes()?, out)?;
        } else {
            pb.skip(wire)?;
        }
    }
    Ok(())
}

fn parse_node(node: &[u8], out: &mut Vec<(String, RawTensor)>) -> Result<(), VadError> {
    let mut pb = Pb::new(node);
    let mut first_output = None;
    let mut tensor = None;
    let mut subgraphs = Vec::new();
    while !pb.done() {
        let (field, wire) = pb.key()?;
        match (field, wire) {
            // NodeProto.output = 2
            (2, 2) => {
                let name = pb.bytes()?;
                if first_output.is_none() {
                    first_output = Some(String::from_utf8_lossy(name).into_owned());
                }
            }
            // NodeProto.attribute = 5
            (5, 2) => {
                parse_attribute(pb.bytes()?, &mut tensor, &mut subgraphs)?;
            }
            _ => pb.skip(wire)?,
        }
    }
    if let (Some(name), Some(tensor)) = (first_output, tensor) {
        out.push((name, tensor));
    }
    for graph in subgraphs {
        collect_constants(graph, out)?;
    }
    Ok(())
}

fn parse_attribute<'a>(
    attr: &'a [u8],
    tensor: &mut Option<RawTensor>,
    subgraphs: &mut Vec<&'a [u8]>,
) -> Result<(), VadError> {
    let mut pb = Pb::new(attr);
    while !pb.done() {
        let (field, wire) = pb.key()?;
        match (field, wire) {
            // AttributeProto.t = 5
            (5, 2) => {
                if let Some(parsed) = parse_tensor(pb.bytes()?)? {
                    *tensor = Some(parsed);
                }
            }
            // AttributeProto.g = 6, .graphs = 11
            (6, 2) | (11, 2) => subgraphs.push(pb.bytes()?),
            _ => pb.skip(wire)?,
        }
    }
    Ok(())
}

/// Parse a TensorProto; returns `None` for non-float tensors.
fn parse_tensor(tensor: &[u8]) -> Result<Option<RawTensor>, VadError> {
    let mut pb = Pb::new(tensor);
    let mut dims = Vec::new();
    let mut data_type = 0u64;
    let mut data = Vec::new();
    while !pb.done() {
        let (field, wire) = pb.key()?;
        match (field, wire) {
            // TensorProto.dims = 1 (packed or repeated varint)
            (1, 0) => dims.push(pb.varint()? as usize),
            (1, 2) => {
                let mut packed = Pb::new(pb.bytes()?);
                while !packed.done() {
                    dims.push(packed.varint()? as usize);
                }
            }
            // TensorProto.data_type = 2 (1 == float32)
            (2, 0) => data_type = pb.varint()?,
            // TensorProto.float_data = 4 (packed floats)
            (4, 2) => {
                for chunk in pb.bytes()?.chunks_exact(4) {
                    data.push(f32::from_le_bytes(chunk.try_into().unwrap()));
                }
            }
            (4, 5) => {
                let start = pb.pos;
                pb.skip(5)?;
                data.push(f32::from_le_bytes(
                    pb.data[start..start + 4].try_into().unwrap(),
                ));
            }
            // TensorProto.raw_data = 9
            (9, 2) => {
                let raw = pb.bytes()?;
                if data_type == 1 {
                    data.reserve(raw.len() / 4);
                    for chunk in raw.chunks_exact(4) {
                        data.push(f32::from_le_bytes(chunk.try_into().unwrap()));
                    }
                }
            }
            _ => pb.skip(wire)?,
        }
    }
    if data_type != 1 || data.is_empty() {
        return Ok(None);
    }
    let expect: usize = dims.iter().product();
    if expect != data.len() {
        return Err(VadError::Parse(format!(
            "tensor size mismatch: dims {dims:?} vs {} values",
            data.len()
        )));
    }
    Ok(Some(RawTensor { dims, data }))
}
