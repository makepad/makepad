//! The one model definition's tensor library: 2-D f32 tensors (`[rows =
//! time, cols = channels]`) and a graph that, when it records, keeps what each
//! op needs for its backward. Inference runs the same forward code with
//! recording off.
//!
//! Two backends behind the same ops: the CPU (the reference; its matmuls go
//! through the Metal/cuBLAS gateway above a size threshold) and, on Linux and
//! Windows, a fully device-resident CUDA path (`nn_gpu`, the trainer's).
//!
//! Batches: a tensor's rows are `items * seg`; `seg` is the rows per item.
//! Convolutions, RoPE positions, attention and the STFT family never cross an
//! item, and `lens` (valid rows per item) masks attention keys.

use crate::dsp::{Rng, Stft};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub use makepad_ai_cuda::train::DevBuf;

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub use makepad_ai_cuda::train::DevU32;

/// No device buffers off Linux/Windows: `Option<DevBuf>` is always None.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub enum DevBuf {}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub enum DevU32 {}

/// Row indices for a gather: on the host, or computed on the device (an
/// alignment the host never sees).
#[derive(Clone)]
pub enum RowIndex {
    Host(Vec<usize>),
    Dev(Arc<DevU32>, usize),
}

impl RowIndex {
    pub fn len(&self) -> usize {
        match self {
            RowIndex::Host(v) => v.len(),
            RowIndex::Dev(_, n) => *n,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Monotonic alignment search on the host: logp [N, T] (only the first `nt`
/// tokens and `nf` frames used); frames per token.
pub fn mas_host(logp: &mut [f32], n: usize, t: usize, nt: usize, nf: usize) -> Vec<u32> {
    const NEG: f32 = -1e30;
    for i in 0..nt {
        if i > 0 {
            logp[i * t] = NEG;
        }
    }
    for j in 1..nf {
        let prev: Vec<f32> = (0..nt).map(|i| logp[i * t + j - 1]).collect();
        for i in 0..nt {
            let stay = prev[i];
            let mv = if i > 0 { prev[i - 1] } else { NEG };
            let mut best = stay.max(mv);
            if i > j || nt - 1 - i > nf - 1 - j {
                best = NEG;
            }
            logp[i * t + j] += best;
        }
    }
    let mut dur = vec![0u32; n];
    let mut i = nt - 1;
    for j in (0..nf).rev() {
        dur[i] += 1;
        if i > 0 && j > 0 && (i == j || logp[(i - 1) * t + j - 1] > logp[i * t + j - 1]) {
            i -= 1;
        }
    }
    dur
}

macro_rules! gpu {
    ($self:ident, $e:expr) => {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if $self.gpu {
            return $e;
        }
    };
}

pub struct Tensor {
    pub rows: usize,
    pub cols: usize,
    /// Host values (empty when the tensor lives on the device).
    pub data: Vec<f32>,
    pub dev: Option<DevBuf>,
    /// Rows per batch item.
    pub seg: usize,
    /// Valid rows per item (attention keys past it are masked).
    pub lens: Option<Arc<Vec<u32>>>,
}

impl Clone for Tensor {
    fn clone(&self) -> Tensor {
        Tensor {
            rows: self.rows,
            cols: self.cols,
            data: self.data.clone(),
            dev: self.dev.as_ref().map(|_d| {
                #[cfg(any(target_os = "linux", target_os = "windows"))]
                {
                    crate::nn_gpu::copy(_d)
                }
                #[cfg(not(any(target_os = "linux", target_os = "windows")))]
                {
                    unreachable!()
                }
            }),
            seg: self.seg,
            lens: self.lens.clone(),
        }
    }
}

impl std::fmt::Debug for Tensor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tensor {}x{} seg {}{}", self.rows, self.cols, self.seg, if self.dev.is_some() { " (device)" } else { "" })
    }
}

impl PartialEq for Tensor {
    fn eq(&self, o: &Tensor) -> bool {
        self.rows == o.rows && self.cols == o.cols && self.host() == o.host()
    }
}

impl Tensor {
    pub fn new(rows: usize, cols: usize, data: Vec<f32>) -> Tensor {
        assert_eq!(rows * cols, data.len(), "tensor shape {rows}x{cols} vs {} values", data.len());
        Tensor { rows, cols, data, dev: None, seg: rows.max(1), lens: None }
    }
    /// `rows / seg` items of `seg` rows each; `lens` = valid rows per item.
    pub fn batched(rows: usize, cols: usize, data: Vec<f32>, seg: usize, lens: Option<Vec<u32>>) -> Tensor {
        assert!(seg > 0 && rows % seg == 0, "rows {rows} not a multiple of seg {seg}");
        let mut t = Tensor::new(rows, cols, data);
        t.seg = seg;
        t.lens = lens.map(Arc::new);
        t
    }
    pub fn zeros(rows: usize, cols: usize) -> Tensor {
        Tensor::new(rows, cols, vec![0.0; rows * cols])
    }
    pub fn scalar(v: f32) -> Tensor {
        Tensor::new(1, 1, vec![v])
    }
    pub fn row(&self, r: usize) -> &[f32] {
        &self.data[r * self.cols..(r + 1) * self.cols]
    }
    pub fn items(&self) -> usize {
        self.rows / self.seg.max(1)
    }
    pub fn len(&self) -> usize {
        self.rows * self.cols
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// The values on the host (downloads a device tensor).
    pub fn host(&self) -> Vec<f32> {
        match &self.dev {
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            Some(d) => d.to_host(),
            #[cfg(not(any(target_os = "linux", target_os = "windows")))]
            Some(d) => match *d {},
            None => self.data.clone(),
        }
    }
    fn meta_like(mut self, o: &Tensor) -> Tensor {
        if self.rows == o.rows {
            self.seg = o.seg;
            self.lens = o.lens.clone();
        }
        self
    }
}

// ---------------------------------------------------------------------------
// Matmul gateway (CPU backend)
// ---------------------------------------------------------------------------

static FORCE_CPU: AtomicBool = AtomicBool::new(false);
/// Below this many multiply-accumulates a GPU dispatch costs more than it saves.
const GPU_MIN_MACS: usize = 2 * 1024 * 1024;

/// Route every CPU-backend matmul through the CPU loops (no gateway).
pub fn force_cpu(v: bool) {
    FORCE_CPU.store(v, Ordering::Relaxed);
}

fn gpu_ok(m: usize, k: usize, n: usize) -> bool {
    !FORCE_CPU.load(Ordering::Relaxed) && m * k * n >= GPU_MIN_MACS
}

/// C[m,n] = A[m,k] B[k,n].
pub fn mm_nn(a: &[f32], b: &[f32], m: usize, k: usize, n: usize) -> Vec<f32> {
    if gpu_ok(m, k, n) {
        if let Some(c) = makepad_ai_metal::try_matmul_nn_f32(a, b, m, k, n) {
            return c;
        }
    }
    let mut c = vec![0.0; m * n];
    for i in 0..m {
        let crow = &mut c[i * n..(i + 1) * n];
        for p in 0..k {
            let av = a[i * k + p];
            if av == 0.0 {
                continue;
            }
            let brow = &b[p * n..(p + 1) * n];
            for j in 0..n {
                crow[j] += av * brow[j];
            }
        }
    }
    c
}

/// C[m,n] = A[m,k] Bt[n,k]^T.
pub fn mm_nt(a: &[f32], bt: &[f32], m: usize, k: usize, n: usize) -> Vec<f32> {
    if gpu_ok(m, k, n) {
        if let Some(c) = makepad_ai_metal::try_matmul_nt_f32(a, bt, m, k, n) {
            return c;
        }
    }
    let mut c = vec![0.0; m * n];
    for i in 0..m {
        let arow = &a[i * k..(i + 1) * k];
        for j in 0..n {
            let brow = &bt[j * k..(j + 1) * k];
            let mut s = 0.0;
            for p in 0..k {
                s += arow[p] * brow[p];
            }
            c[i * n + j] = s;
        }
    }
    c
}

pub fn transpose(a: &[f32], rows: usize, cols: usize) -> Vec<f32> {
    let mut t = vec![0.0; a.len()];
    for r in 0..rows {
        for c in 0..cols {
            t[c * rows + r] = a[r * cols + c];
        }
    }
    t
}

/// C[m,n] = A[k,m]^T B[k,n].
pub fn mm_tn(a: &[f32], b: &[f32], k: usize, m: usize, n: usize) -> Vec<f32> {
    mm_nn(&transpose(a, k, m), b, m, k, n)
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub enum Init {
    Zeros,
    Ones,
    Const(f32),
    /// Uniform in ±sqrt(3/fan_in) * gain (fan_in = cols).
    Fan(f32),
    Normal(f32),
}

#[derive(Clone, Default)]
pub struct Params {
    pub names: Vec<String>,
    pub vals: Vec<Arc<Tensor>>,
    index: HashMap<String, usize>,
}

impl Params {
    pub fn new() -> Params {
        Params::default()
    }

    pub fn declare(&mut self, name: &str, rows: usize, cols: usize, init: Init, rng: &mut Rng) {
        assert!(!self.index.contains_key(name), "parameter {name} declared twice");
        let data = match init {
            Init::Zeros => vec![0.0; rows * cols],
            Init::Ones => vec![1.0; rows * cols],
            Init::Const(v) => vec![v; rows * cols],
            Init::Fan(gain) => {
                let a = (3.0 / cols.max(1) as f32).sqrt() * gain;
                (0..rows * cols).map(|_| (rng.unit() * 2.0 - 1.0) * a).collect()
            }
            Init::Normal(std) => (0..rows * cols).map(|_| rng.normal() * std).collect(),
        };
        self.insert(name, Tensor::new(rows, cols, data));
    }

    pub fn insert(&mut self, name: &str, t: Tensor) {
        if let Some(&i) = self.index.get(name) {
            self.vals[i] = Arc::new(t);
            return;
        }
        self.index.insert(name.to_string(), self.vals.len());
        self.names.push(name.to_string());
        self.vals.push(Arc::new(t));
    }

    pub fn id(&self, name: &str) -> usize {
        match self.index.get(name) {
            Some(i) => *i,
            None => panic!("missing parameter {name}"),
        }
    }

    pub fn get(&self, name: &str) -> &Tensor {
        &self.vals[self.id(name)]
    }

    pub fn has(&self, name: &str) -> bool {
        self.index.contains_key(name)
    }

    pub fn count(&self) -> usize {
        self.vals.iter().map(|t| t.len()).sum()
    }

    /// Parameters whose names start with `prefix`.
    pub fn count_prefix(&self, prefix: &str) -> usize {
        self.names.iter().zip(&self.vals).filter(|(n, _)| n.starts_with(prefix)).map(|(_, t)| t.len()).sum()
    }
}

// ---------------------------------------------------------------------------
// Graph
// ---------------------------------------------------------------------------

pub type Id = usize;

/// A gradient: host values or a device buffer.
pub enum GBuf {
    Host(Vec<f32>),
    Dev(DevBuf),
}

impl GBuf {
    pub fn host(&self) -> &[f32] {
        match self {
            GBuf::Host(v) => v,
            GBuf::Dev(_) => panic!("device gradient on the CPU path"),
        }
    }
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    pub fn dev(&self) -> &DevBuf {
        match self {
            GBuf::Dev(d) => d,
            GBuf::Host(_) => panic!("host gradient on the GPU path"),
        }
    }
    pub fn to_host(&self) -> Vec<f32> {
        match self {
            GBuf::Host(v) => v.clone(),
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            GBuf::Dev(d) => d.to_host(),
            #[cfg(not(any(target_os = "linux", target_os = "windows")))]
            GBuf::Dev(d) => match *d {},
        }
    }
}

pub struct Grads {
    pub(crate) g: Vec<Option<GBuf>>,
    pub(crate) needs: Vec<bool>,
    #[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
    pub(crate) sizes: Vec<usize>,
}

impl Grads {
    pub fn needs(&self, id: Id) -> bool {
        self.needs[id]
    }
    pub fn add(&mut self, id: Id, d: &[f32]) {
        if !self.needs[id] {
            return;
        }
        match &mut self.g[id] {
            Some(GBuf::Host(v)) => {
                for (a, b) in v.iter_mut().zip(d) {
                    *a += *b;
                }
            }
            slot => *slot = Some(GBuf::Host(d.to_vec())),
        }
    }
    pub fn add_vec(&mut self, id: Id, d: Vec<f32>) {
        if !self.needs[id] {
            return;
        }
        match &mut self.g[id] {
            Some(GBuf::Host(v)) => {
                for (a, b) in v.iter_mut().zip(&d) {
                    *a += *b;
                }
            }
            slot => *slot = Some(GBuf::Host(d)),
        }
    }
    /// The device gradient buffer of `id` to accumulate into (zeroed on first
    /// use), or None when `id` needs no gradient.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    pub fn acc(&mut self, id: Id) -> Option<&DevBuf> {
        if !self.needs[id] {
            return None;
        }
        if self.g[id].is_none() {
            self.g[id] = Some(GBuf::Dev(DevBuf::zeros(self.sizes[id])));
        }
        Some(self.g[id].as_ref().unwrap().dev())
    }
    /// Raw pointer form of `acc` (null when not needed).
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    pub fn accp(&mut self, id: Id) -> *mut f32 {
        self.acc(id).map(|d| d.mptr()).unwrap_or(std::ptr::null_mut())
    }
}

pub(crate) type Back = Box<dyn Fn(&GBuf, &mut Grads)>;

pub struct Graph<'p> {
    pub params: &'p Params,
    /// Device copies of the parameters (the GPU backend).
    pub dev_params: Option<&'p [Arc<Tensor>]>,
    pub(crate) vals: Vec<Arc<Tensor>>,
    pub(crate) backs: Vec<Option<Back>>,
    pub(crate) needs: Vec<bool>,
    param_of: Vec<Option<usize>>,
    pub record: bool,
    pub gpu: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Act {
    Gelu = 0,
    Silu = 1,
    Relu = 2,
    LeakyRelu = 3,
    Tanh = 4,
    Sigmoid = 5,
    /// exp(min(x, 10)).
    Exp = 6,
}

pub(crate) fn act_fwd(a: Act, x: f32) -> f32 {
    match a {
        Act::Gelu => 0.5 * x * (1.0 + (0.797_884_6 * (x + 0.044_715 * x * x * x)).tanh()),
        Act::Silu => x / (1.0 + (-x).exp()),
        Act::Relu => x.max(0.0),
        Act::LeakyRelu => if x > 0.0 { x } else { 0.1 * x },
        Act::Tanh => x.tanh(),
        Act::Sigmoid => 1.0 / (1.0 + (-x).exp()),
        Act::Exp => x.min(10.0).exp(),
    }
}

fn act_grad(a: Act, x: f32) -> f32 {
    match a {
        Act::Gelu => {
            let u = 0.797_884_6 * (x + 0.044_715 * x * x * x);
            let t = u.tanh();
            0.5 * (1.0 + t) + 0.5 * x * (1.0 - t * t) * 0.797_884_6 * (1.0 + 3.0 * 0.044_715 * x * x)
        }
        Act::Silu => {
            let s = 1.0 / (1.0 + (-x).exp());
            s * (1.0 + x * (1.0 - s))
        }
        Act::Relu => if x > 0.0 { 1.0 } else { 0.0 },
        Act::LeakyRelu => if x > 0.0 { 1.0 } else { 0.1 },
        Act::Tanh => 1.0 - x.tanh().powi(2),
        Act::Sigmoid => {
            let s = 1.0 / (1.0 + (-x).exp());
            s * (1.0 - s)
        }
        Act::Exp => if x < 10.0 { x.exp() } else { 0.0 },
    }
}

/// Source row of tap `j` for row `r` in a same-padded convolution within
/// segments of `seg` rows.
#[inline]
fn tap(r: usize, j: usize, dil: usize, pad: usize, seg: usize) -> Option<usize> {
    let base = r / seg * seg;
    let s = (r - base) as isize + (j * dil) as isize - pad as isize;
    if s >= 0 && (s as usize) < seg { Some(base + s as usize) } else { None }
}

impl<'p> Graph<'p> {
    /// A CPU graph.
    pub fn new(params: &'p Params, record: bool) -> Graph<'p> {
        Graph { params, dev_params: None, vals: Vec::new(), backs: Vec::new(), needs: Vec::new(), param_of: Vec::new(), record, gpu: false }
    }

    /// A device graph over device copies of `params` (same order).
    pub fn new_gpu(params: &'p Params, dev_params: &'p [Arc<Tensor>], record: bool) -> Graph<'p> {
        let mut g = Graph::new(params, record);
        g.dev_params = Some(dev_params);
        g.gpu = true;
        g
    }

    pub fn val(&self, id: Id) -> &Tensor {
        &self.vals[id]
    }

    /// The values of `id` on the host.
    pub fn host(&self, id: Id) -> Vec<f32> {
        self.vals[id].host()
    }

    pub fn shape(&self, id: Id) -> (usize, usize) {
        let t = &self.vals[id];
        (t.rows, t.cols)
    }

    pub(crate) fn push(&mut self, t: Tensor, inputs: &[Id], back: Option<Back>) -> Id {
        let needs = self.record && inputs.iter().any(|i| self.needs[*i]);
        self.vals.push(Arc::new(t));
        self.backs.push(if needs { back } else { None });
        self.needs.push(needs);
        self.param_of.push(None);
        self.vals.len() - 1
    }

    /// A constant input (no gradient); uploaded on a device graph.
    pub fn input(&mut self, t: Tensor) -> Id {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        let t = if self.gpu && t.dev.is_none() { crate::nn_gpu::upload(t) } else { t };
        self.vals.push(Arc::new(t));
        self.backs.push(None);
        self.needs.push(false);
        self.param_of.push(None);
        self.vals.len() - 1
    }

    /// A parameter leaf.
    pub fn p(&mut self, name: &str) -> Id {
        let i = self.params.id(name);
        let v = match self.dev_params {
            Some(d) => d[i].clone(),
            None => self.params.vals[i].clone(),
        };
        self.vals.push(v);
        self.backs.push(None);
        self.needs.push(self.record);
        self.param_of.push(Some(i));
        self.vals.len() - 1
    }

    /// Stop the gradient.
    pub fn detach(&mut self, x: Id) -> Id {
        let t = (*self.vals[x]).clone();
        self.vals.push(Arc::new(t));
        self.backs.push(None);
        self.needs.push(false);
        self.param_of.push(None);
        self.vals.len() - 1
    }

    /// Backpropagate from the scalar `loss`; the gradient of every parameter
    /// (indexed like `Params::vals`, None where unused).
    pub fn backward(&mut self, loss: Id) -> Vec<Option<GBuf>> {
        let n = self.vals.len();
        let sizes = self.vals.iter().map(|t| t.len()).collect();
        let mut grads = Grads { g: (0..n).map(|_| None).collect(), needs: self.needs.clone(), sizes };
        let seed = vec![1.0; self.vals[loss].len()];
        grads.g[loss] = Some(match self.gpu {
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            true => GBuf::Dev(DevBuf::from_host(&seed)),
            _ => GBuf::Host(seed),
        });
        for id in (0..=loss).rev() {
            if self.param_of[id].is_some() {
                continue;
            }
            let Some(back) = self.backs[id].take() else { continue };
            let Some(d) = grads.g[id].take() else { continue };
            back(&d, &mut grads);
        }
        let mut out: Vec<Option<GBuf>> = (0..self.params.vals.len()).map(|_| None).collect();
        for id in 0..n {
            if let (Some(p), Some(g)) = (self.param_of[id], grads.g[id].take()) {
                match (&mut out[p], g) {
                    (Some(GBuf::Host(v)), GBuf::Host(g)) => {
                        for (a, b) in v.iter_mut().zip(&g) {
                            *a += *b;
                        }
                    }
                    #[cfg(any(target_os = "linux", target_os = "windows"))]
                    (Some(GBuf::Dev(v)), GBuf::Dev(g)) => crate::nn_gpu::axpy(&g, v, 1.0),
                    (slot, g) => *slot = Some(g),
                }
            }
        }
        out
    }

    // --- elementwise ---------------------------------------------------------

    pub fn act(&mut self, x: Id, a: Act) -> Id {
        gpu!(self, crate::nn_gpu::act(self, x, a));
        let xv = self.vals[x].clone();
        let t = Tensor::new(xv.rows, xv.cols, xv.data.iter().map(|v| act_fwd(a, *v)).collect()).meta_like(&xv);
        let back: Back = Box::new(move |d, g| {
            g.add_vec(x, d.host().iter().zip(&xv.data).map(|(d, x)| d * act_grad(a, *x)).collect());
        });
        self.push(t, &[x], Some(back))
    }

    pub fn add(&mut self, a: Id, b: Id) -> Id {
        gpu!(self, crate::nn_gpu::add(self, a, b));
        let (av, bv) = (self.vals[a].clone(), self.vals[b].clone());
        assert_eq!((av.rows, av.cols), (bv.rows, bv.cols), "add shapes");
        let t = Tensor::new(av.rows, av.cols, av.data.iter().zip(&bv.data).map(|(x, y)| x + y).collect()).meta_like(&av);
        let back: Back = Box::new(move |d, g| {
            g.add(a, d.host());
            g.add(b, d.host());
        });
        self.push(t, &[a, b], Some(back))
    }

    pub fn sub(&mut self, a: Id, b: Id) -> Id {
        let nb = self.scale(b, -1.0);
        self.add(a, nb)
    }

    pub fn mul(&mut self, a: Id, b: Id) -> Id {
        gpu!(self, crate::nn_gpu::mul(self, a, b));
        let (av, bv) = (self.vals[a].clone(), self.vals[b].clone());
        assert_eq!((av.rows, av.cols), (bv.rows, bv.cols), "mul shapes");
        let t = Tensor::new(av.rows, av.cols, av.data.iter().zip(&bv.data).map(|(x, y)| x * y).collect()).meta_like(&av);
        let back: Back = Box::new(move |d, g| {
            let d = d.host();
            if g.needs(a) {
                g.add_vec(a, d.iter().zip(&bv.data).map(|(d, y)| d * y).collect());
            }
            if g.needs(b) {
                g.add_vec(b, d.iter().zip(&av.data).map(|(d, x)| d * x).collect());
            }
        });
        self.push(t, &[a, b], Some(back))
    }

    pub fn scale(&mut self, x: Id, s: f32) -> Id {
        gpu!(self, crate::nn_gpu::scale(self, x, s));
        let xv = self.vals[x].clone();
        let t = Tensor::new(xv.rows, xv.cols, xv.data.iter().map(|v| v * s).collect()).meta_like(&xv);
        let back: Back = Box::new(move |d, g| g.add_vec(x, d.host().iter().map(|v| v * s).collect()));
        self.push(t, &[x], Some(back))
    }

    /// x[T,C] + v[1,C] (v broadcast over rows).
    pub fn add_row(&mut self, x: Id, v: Id) -> Id {
        gpu!(self, crate::nn_gpu::add_row(self, x, v));
        let (xv, vv) = (self.vals[x].clone(), self.vals[v].clone());
        assert_eq!(vv.len(), xv.cols, "add_row width");
        let c = xv.cols;
        let mut data = xv.data.clone();
        for (i, o) in data.iter_mut().enumerate() {
            *o += vv.data[i % c];
        }
        let back: Back = Box::new(move |d, g| {
            let d = d.host();
            g.add(x, d);
            if g.needs(v) {
                let mut s = vec![0.0; c];
                for (i, dv) in d.iter().enumerate() {
                    s[i % c] += dv;
                }
                g.add_vec(v, s);
            }
        });
        let t = Tensor::new(xv.rows, c, data).meta_like(&xv);
        self.push(t, &[x, v], Some(back))
    }

    /// x[T,C] * v[1,C].
    pub fn mul_row(&mut self, x: Id, v: Id) -> Id {
        gpu!(self, crate::nn_gpu::mul_row(self, x, v));
        let (xv, vv) = (self.vals[x].clone(), self.vals[v].clone());
        assert_eq!(vv.len(), xv.cols, "mul_row width");
        let c = xv.cols;
        let data = xv.data.iter().enumerate().map(|(i, a)| a * vv.data[i % c]).collect();
        let t = Tensor::new(xv.rows, c, data).meta_like(&xv);
        let back: Back = Box::new(move |d, g| {
            let d = d.host();
            if g.needs(x) {
                g.add_vec(x, d.iter().enumerate().map(|(i, dv)| dv * vv.data[i % c]).collect());
            }
            if g.needs(v) {
                let mut s = vec![0.0; c];
                for (i, dv) in d.iter().enumerate() {
                    s[i % c] += dv * xv.data[i];
                }
                g.add_vec(v, s);
            }
        });
        self.push(t, &[x, v], Some(back))
    }

    /// x[T,C] * s[T,1] (a per-row gain).
    pub fn mul_col(&mut self, x: Id, s: Id) -> Id {
        gpu!(self, crate::nn_gpu::mul_col(self, x, s));
        let (xv, sv) = (self.vals[x].clone(), self.vals[s].clone());
        assert_eq!(sv.len(), xv.rows, "mul_col height");
        let c = xv.cols;
        let data = xv.data.iter().enumerate().map(|(i, a)| a * sv.data[i / c]).collect();
        let t = Tensor::new(xv.rows, c, data).meta_like(&xv);
        let back: Back = Box::new(move |d, g| {
            let d = d.host();
            if g.needs(x) {
                g.add_vec(x, d.iter().enumerate().map(|(i, dv)| dv * sv.data[i / c]).collect());
            }
            if g.needs(s) {
                let mut r = vec![0.0; sv.len()];
                for (i, dv) in d.iter().enumerate() {
                    r[i / c] += dv * xv.data[i];
                }
                g.add_vec(s, r);
            }
        });
        self.push(t, &[x, s], Some(back))
    }

    // --- shape ---------------------------------------------------------------

    pub fn concat_cols(&mut self, parts: &[Id]) -> Id {
        gpu!(self, crate::nn_gpu::concat_cols(self, parts));
        let first = self.vals[parts[0]].clone();
        let rows = first.rows;
        let widths: Vec<usize> = parts.iter().map(|p| self.vals[*p].cols).collect();
        let total: usize = widths.iter().sum();
        let mut data = vec![0.0; rows * total];
        let mut off = 0;
        for (p, w) in parts.iter().zip(&widths) {
            let v = &self.vals[*p];
            assert_eq!(v.rows, rows, "concat rows");
            for r in 0..rows {
                data[r * total + off..r * total + off + w].copy_from_slice(&v.data[r * w..(r + 1) * w]);
            }
            off += w;
        }
        let ids = parts.to_vec();
        let back: Back = Box::new(move |d, g| {
            let d = d.host();
            let mut off = 0;
            for (p, w) in ids.iter().zip(&widths) {
                if g.needs(*p) {
                    let mut s = vec![0.0; rows * w];
                    for r in 0..rows {
                        s[r * w..(r + 1) * w].copy_from_slice(&d[r * total + off..r * total + off + w]);
                    }
                    g.add_vec(*p, s);
                }
                off += w;
            }
        });
        let t = Tensor::new(rows, total, data).meta_like(&first);
        self.push(t, parts, Some(back))
    }

    pub fn slice_cols(&mut self, x: Id, start: usize, len: usize) -> Id {
        gpu!(self, crate::nn_gpu::slice_cols(self, x, start, len));
        let xv = self.vals[x].clone();
        let (rows, cols) = (xv.rows, xv.cols);
        let mut data = vec![0.0; rows * len];
        for r in 0..rows {
            data[r * len..(r + 1) * len].copy_from_slice(&xv.data[r * cols + start..r * cols + start + len]);
        }
        let back: Back = Box::new(move |d, g| {
            let d = d.host();
            let mut s = vec![0.0; rows * cols];
            for r in 0..rows {
                s[r * cols + start..r * cols + start + len].copy_from_slice(&d[r * len..(r + 1) * len]);
            }
            g.add_vec(x, s);
        });
        let t = Tensor::new(rows, len, data).meta_like(&xv);
        self.push(t, &[x], Some(back))
    }

    /// Rows of x picked by `idx` (embedding lookup, length regulation); the
    /// result has `seg` rows per item and valid lengths `lens`.
    pub fn gather_rows(&mut self, x: Id, idx: &[usize], seg: usize, lens: Option<Vec<u32>>) -> Id {
        gpu!(self, crate::nn_gpu::gather_rows(self, x, idx, seg, lens));
        let xv = self.vals[x].clone();
        let c = xv.cols;
        let mut data = Vec::with_capacity(idx.len() * c);
        for &i in idx {
            data.extend_from_slice(xv.row(i));
        }
        let idx = idx.to_vec();
        let n = xv.len();
        let back: Back = Box::new(move |d, g| {
            let d = d.host();
            let mut s = vec![0.0; n];
            for (r, &i) in idx.iter().enumerate() {
                for k in 0..c {
                    s[i * c + k] += d[r * c + k];
                }
            }
            g.add_vec(x, s);
        });
        let rows = data.len() / c;
        self.push(Tensor::batched(rows, c, data, seg, lens), &[x], Some(back))
    }

    /// `gather_rows` with a `RowIndex`.
    pub fn gather_rows_idx(&mut self, x: Id, idx: &RowIndex, seg: usize, lens: Option<Vec<u32>>) -> Id {
        match idx {
            RowIndex::Host(v) => self.gather_rows(x, v, seg, lens),
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            RowIndex::Dev(d, n) => crate::nn_gpu::gather_rows_dev(self, x, d.clone(), *n, seg, lens),
            #[cfg(not(any(target_os = "linux", target_os = "windows")))]
            RowIndex::Dev(d, _) => match **d {},
        }
    }

    /// Align token means `mu` [B*N, D] to frames `mel` [B*T, D] (values only,
    /// no gradient) by monotonic alignment search under a unit Gaussian:
    /// (the encoder row of every frame, ln(1 + frames) per token [B*N, 1]).
    pub fn mas_align(&mut self, mu: Id, mel: Id, tok_lens: &[u32], frame_lens: &[u32]) -> (RowIndex, Id) {
        gpu!(self, crate::nn_gpu::mas_align(self, mu, mel, tok_lens, frame_lens));
        let (mv, ev) = (self.vals[mu].clone(), self.vals[mel].clone());
        let (n, t, d, b) = (mv.seg, ev.seg, mv.cols, mv.items());
        let mut idx = vec![0usize; b * t];
        let mut logdur = vec![0.0f32; b * n];
        for item in 0..b {
            let mut lp = vec![0.0f32; n * t];
            for i in 0..n {
                let m = &mv.data[(item * n + i) * d..(item * n + i + 1) * d];
                let a: f32 = m.iter().map(|v| v * v).sum();
                for j in 0..t {
                    let e = &ev.data[(item * t + j) * d..(item * t + j + 1) * d];
                    let dot: f32 = m.iter().zip(e).map(|(x, y)| x * y).sum();
                    let c: f32 = e.iter().map(|v| v * v).sum();
                    lp[i * t + j] = dot - 0.5 * a - 0.5 * c;
                }
            }
            let dur = mas_host(&mut lp, n, t, tok_lens[item] as usize, frame_lens[item] as usize);
            let mut f = 0;
            for i in 0..n {
                logdur[item * n + i] = (1.0 + dur[i] as f32).ln();
                for _ in 0..dur[i] {
                    if f < t {
                        idx[item * t + f] = item * n + i;
                        f += 1;
                    }
                }
            }
            for q in f..t {
                idx[item * t + q] = item * n + n - 1;
            }
        }
        let ld = self.input(Tensor::batched(b * n, 1, logdur, n, None));
        (RowIndex::Host(idx), ld)
    }

    // --- linear algebra ------------------------------------------------------

    /// x[T,in] W[out,in]^T + b[1,out].
    pub fn linear(&mut self, x: Id, w: Id, b: Option<Id>) -> Id {
        gpu!(self, crate::nn_gpu::linear(self, x, w, b));
        let (xv, wv) = (self.vals[x].clone(), self.vals[w].clone());
        let (t, k, n) = (xv.rows, xv.cols, wv.rows);
        assert_eq!(wv.cols, k, "linear: input width {k} vs weight {}x{}", wv.rows, wv.cols);
        let y = mm_nt(&xv.data, &wv.data, t, k, n);
        let back: Back = Box::new(move |d, g| {
            let d = d.host();
            if g.needs(x) {
                g.add_vec(x, mm_nn(d, &wv.data, t, n, k));
            }
            if g.needs(w) {
                g.add_vec(w, mm_tn(d, &xv.data, t, n, k));
            }
        });
        let out = Tensor::new(t, n, y).meta_like(&self.vals[x]);
        let out = self.push(out, &[x, w], Some(back));
        match b {
            Some(b) => self.add_row(out, b),
            None => out,
        }
    }

    /// Dense 1-D convolution over time, same padding: x[T,Cin], w[Cout, k*Cin]
    /// (tap-major: column j*Cin + c is tap j of input channel c).
    pub fn conv1d(&mut self, x: Id, w: Id, b: Option<Id>, k: usize, dilation: usize) -> Id {
        let col = self.im2col(x, k, dilation);
        self.linear(col, w, b)
    }

    pub(crate) fn im2col(&mut self, x: Id, k: usize, dil: usize) -> Id {
        gpu!(self, crate::nn_gpu::im2col(self, x, k, dil));
        let xv = self.vals[x].clone();
        let (t, c, seg) = (xv.rows, xv.cols, xv.seg);
        let pad = (k - 1) * dil / 2;
        let mut col = vec![0.0; t * k * c];
        for r in 0..t {
            for j in 0..k {
                if let Some(s) = tap(r, j, dil, pad, seg) {
                    col[r * k * c + j * c..r * k * c + (j + 1) * c].copy_from_slice(xv.row(s));
                }
            }
        }
        let back: Back = Box::new(move |d, g| {
            let d = d.host();
            let mut s = vec![0.0; t * c];
            for r in 0..t {
                for j in 0..k {
                    if let Some(src) = tap(r, j, dil, pad, seg) {
                        for q in 0..c {
                            s[src * c + q] += d[r * k * c + j * c + q];
                        }
                    }
                }
            }
            g.add_vec(x, s);
        });
        let out = Tensor::new(t, k * c, col).meta_like(&xv);
        self.push(out, &[x], Some(back))
    }

    /// Depthwise 1-D convolution, same padding: x[T,C], w[k,C], b[1,C].
    pub fn dwconv1d(&mut self, x: Id, w: Id, b: Id, k: usize) -> Id {
        let out = self.dwconv_nobias(x, w, k);
        self.add_row(out, b)
    }

    fn dwconv_nobias(&mut self, x: Id, w: Id, k: usize) -> Id {
        gpu!(self, crate::nn_gpu::dwconv(self, x, w, k));
        let (xv, wv) = (self.vals[x].clone(), self.vals[w].clone());
        let (t, c, seg) = (xv.rows, xv.cols, xv.seg);
        let pad = (k - 1) / 2;
        let mut y = vec![0.0; t * c];
        for r in 0..t {
            let yr = &mut y[r * c..(r + 1) * c];
            for j in 0..k {
                let Some(src) = tap(r, j, 1, pad, seg) else { continue };
                let xr = xv.row(src);
                let wr = wv.row(j);
                for q in 0..c {
                    yr[q] += xr[q] * wr[q];
                }
            }
        }
        let xv2 = xv.clone();
        let back: Back = Box::new(move |d, g| {
            let d = d.host();
            let mut dx = vec![0.0; t * c];
            let mut dw = vec![0.0; k * c];
            for r in 0..t {
                let dr = &d[r * c..(r + 1) * c];
                for j in 0..k {
                    let Some(src) = tap(r, j, 1, pad, seg) else { continue };
                    let so = src * c;
                    for q in 0..c {
                        dx[so + q] += dr[q] * wv.data[j * c + q];
                        dw[j * c + q] += dr[q] * xv2.data[so + q];
                    }
                }
            }
            g.add_vec(x, dx);
            g.add_vec(w, dw);
        });
        let out = Tensor::new(t, c, y).meta_like(&xv);
        self.push(out, &[x, w], Some(back))
    }

    /// LayerNorm over channels with gain and bias [1,C].
    pub fn layer_norm(&mut self, x: Id, gain: Id, bias: Id) -> Id {
        let n = self.normalize(x);
        let n = self.mul_row(n, gain);
        self.add_row(n, bias)
    }

    fn normalize(&mut self, x: Id) -> Id {
        gpu!(self, crate::nn_gpu::normalize(self, x));
        let xv = self.vals[x].clone();
        let (t, c) = (xv.rows, xv.cols);
        let mut xhat = vec![0.0; t * c];
        let mut rstd = vec![0.0; t];
        for r in 0..t {
            let row = xv.row(r);
            let mean = row.iter().sum::<f32>() / c as f32;
            let var = row.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / c as f32;
            let rs = 1.0 / (var + 1e-5).sqrt();
            rstd[r] = rs;
            for q in 0..c {
                xhat[r * c + q] = (row[q] - mean) * rs;
            }
        }
        let xh = Arc::new(xhat.clone());
        let back: Back = Box::new(move |d, g| {
            let d = d.host();
            let mut dx = vec![0.0; t * c];
            for r in 0..t {
                let dr = &d[r * c..(r + 1) * c];
                let xr = &xh[r * c..(r + 1) * c];
                let m1 = dr.iter().sum::<f32>() / c as f32;
                let m2 = dr.iter().zip(xr).map(|(a, b)| a * b).sum::<f32>() / c as f32;
                for q in 0..c {
                    dx[r * c + q] = rstd[r] * (dr[q] - m1 - xr[q] * m2);
                }
            }
            g.add_vec(x, dx);
        });
        let out = Tensor::new(t, c, xhat).meta_like(&xv);
        self.push(out, &[x], Some(back))
    }

    /// a * sigmoid(b) where x = [a | b] along channels.
    pub fn glu(&mut self, x: Id) -> Id {
        let c = self.vals[x].cols / 2;
        let a = self.slice_cols(x, 0, c);
        let b = self.slice_cols(x, c, c);
        let s = self.act(b, Act::Sigmoid);
        self.mul(a, s)
    }

    /// Rotary position embedding on each head's channels (pairs i, i + d/2);
    /// the position is the row within its item.
    pub fn rope(&mut self, x: Id, heads: usize) -> Id {
        gpu!(self, crate::nn_gpu::rope(self, x, heads));
        let xv = self.vals[x].clone();
        let (t, c, seg) = (xv.rows, xv.cols, xv.seg);
        let dh = c / heads;
        let half = dh / 2;
        let freqs: Vec<f32> = (0..half).map(|i| 10000f32.powf(-(i as f32) / half as f32)).collect();
        let rot = Arc::new(move |data: &[f32], sign: f32| {
            let mut y = data.to_vec();
            for r in 0..t {
                let pos = (r % seg) as f32;
                for h in 0..heads {
                    for i in 0..half {
                        let (s, cs) = (pos * freqs[i]).sin_cos();
                        let a = r * c + h * dh + i;
                        let b = a + half;
                        let (xa, xb) = (data[a], data[b]);
                        y[a] = xa * cs - sign * xb * s;
                        y[b] = sign * xa * s + xb * cs;
                    }
                }
            }
            y
        });
        let y = rot(&xv.data, 1.0);
        let rb = rot.clone();
        let back: Back = Box::new(move |d, g| g.add_vec(x, rb(d.host(), -1.0)));
        let out = Tensor::new(t, c, y).meta_like(&xv);
        self.push(out, &[x], Some(back))
    }

    /// Multi-head softmax attention within items: q [B*T, C], k and v
    /// [B*S, C]; keys past k's `lens` are masked.
    pub fn attention(&mut self, q: Id, k: Id, v: Id, heads: usize) -> Id {
        gpu!(self, crate::nn_gpu::attention(self, q, k, v, heads));
        let (qv, kv, vv) = (self.vals[q].clone(), self.vals[k].clone(), self.vals[v].clone());
        let (c, t, s) = (qv.cols, qv.seg, kv.seg);
        let items = qv.items();
        assert_eq!(items, kv.items(), "attention items");
        let klens: Vec<usize> = match &kv.lens {
            Some(l) => l.iter().map(|v| *v as usize).collect(),
            None => vec![s; items],
        };
        let dh = c / heads;
        let scale = 1.0 / (dh as f32).sqrt();
        let block = move |x: &Tensor, b: usize, rows: usize, h: usize| -> Vec<f32> {
            let mut o = Vec::with_capacity(rows * dh);
            for r in 0..rows {
                let row = b * rows + r;
                o.extend_from_slice(&x.data[row * c + h * dh..row * c + (h + 1) * dh]);
            }
            o
        };
        let mut out = vec![0.0; items * t * c];
        let mut probs = Vec::new();
        for b in 0..items {
            for h in 0..heads {
                let (qh, kh, vh) = (block(&qv, b, t, h), block(&kv, b, s, h), block(&vv, b, s, h));
                let mut p = mm_nt(&qh, &kh, t, dh, s);
                for r in 0..t {
                    let row = &mut p[r * s..(r + 1) * s];
                    let mut mx = f32::NEG_INFINITY;
                    for (j, v) in row.iter_mut().enumerate() {
                        *v *= scale;
                        if j < klens[b] {
                            mx = mx.max(*v);
                        }
                    }
                    let mut sum = 0.0;
                    for (j, v) in row.iter_mut().enumerate() {
                        *v = if j < klens[b] { (*v - mx).exp() } else { 0.0 };
                        sum += *v;
                    }
                    for v in row.iter_mut() {
                        *v /= sum.max(1e-30);
                    }
                }
                let o = mm_nn(&p, &vh, t, s, dh);
                for r in 0..t {
                    let row = b * t + r;
                    out[row * c + h * dh..row * c + (h + 1) * dh].copy_from_slice(&o[r * dh..(r + 1) * dh]);
                }
                if self.record {
                    probs.push(p);
                }
            }
        }
        let back: Back = Box::new(move |d, g| {
            let d = d.host();
            let mut dq = vec![0.0; items * t * c];
            let mut dk = vec![0.0; items * s * c];
            let mut dv = vec![0.0; items * s * c];
            for b in 0..items {
                for h in 0..heads {
                    let p = &probs[b * heads + h];
                    let (qh, kh, vh) = (block(&qv, b, t, h), block(&kv, b, s, h), block(&vv, b, s, h));
                    let mut doh = Vec::with_capacity(t * dh);
                    for r in 0..t {
                        let row = b * t + r;
                        doh.extend_from_slice(&d[row * c + h * dh..row * c + (h + 1) * dh]);
                    }
                    let dvh = mm_tn(p, &doh, t, s, dh);
                    let mut dp = mm_nt(&doh, &vh, t, dh, s);
                    for r in 0..t {
                        let pr = &p[r * s..(r + 1) * s];
                        let dr = &mut dp[r * s..(r + 1) * s];
                        let dot: f32 = pr.iter().zip(dr.iter()).map(|(a, b)| a * b).sum();
                        for j in 0..s {
                            dr[j] = pr[j] * (dr[j] - dot) * scale;
                        }
                    }
                    let dqh = mm_nn(&dp, &kh, t, s, dh);
                    let dkh = mm_tn(&dp, &qh, t, s, dh);
                    for r in 0..t {
                        let row = b * t + r;
                        dq[row * c + h * dh..row * c + (h + 1) * dh].copy_from_slice(&dqh[r * dh..(r + 1) * dh]);
                    }
                    for r in 0..s {
                        let row = b * s + r;
                        dk[row * c + h * dh..row * c + (h + 1) * dh].copy_from_slice(&dkh[r * dh..(r + 1) * dh]);
                        dv[row * c + h * dh..row * c + (h + 1) * dh].copy_from_slice(&dvh[r * dh..(r + 1) * dh]);
                    }
                }
            }
            g.add_vec(q, dq);
            g.add_vec(k, dk);
            g.add_vec(v, dv);
        });
        let o = Tensor::new(items * t, c, out).meta_like(&self.vals[q]);
        self.push(o, &[q, k, v], Some(back))
    }

    // --- losses (scalars) ------------------------------------------------------

    /// Mean |x - target| over rows weighted by `mask` (per row) and, per item,
    /// only the first `col_lim[item]` columns.
    pub fn l1_loss(&mut self, x: Id, target: Id, mask: Option<&[f32]>, col_lim: Option<&[u32]>) -> Id {
        self.masked_loss(x, target, mask, col_lim, false)
    }

    pub fn mse_loss(&mut self, x: Id, target: Id, mask: Option<&[f32]>, col_lim: Option<&[u32]>) -> Id {
        self.masked_loss(x, target, mask, col_lim, true)
    }

    /// The normaliser of a masked loss: sum over rows of mask * counted columns.
    pub(crate) fn loss_denom(rows: usize, cols: usize, seg: usize, mask: Option<&[f32]>, col_lim: Option<&[u32]>) -> f32 {
        let mut d = 0.0;
        for r in 0..rows {
            let w = mask.map(|m| m[r]).unwrap_or(1.0);
            let c = col_lim.map(|l| (l[r / seg] as usize).min(cols)).unwrap_or(cols);
            d += w * c as f32;
        }
        d.max(1.0)
    }

    fn masked_loss(&mut self, x: Id, target: Id, mask: Option<&[f32]>, col_lim: Option<&[u32]>, sq: bool) -> Id {
        gpu!(self, crate::nn_gpu::masked_loss(self, x, target, mask, col_lim, sq));
        let (xv, tv) = (self.vals[x].clone(), self.vals[target].clone());
        assert_eq!(xv.len(), tv.len(), "loss shapes");
        let (rows, c, seg) = (xv.rows, xv.cols, xv.seg);
        let denom = Self::loss_denom(rows, c, seg, mask, col_lim);
        let mut loss = 0.0;
        let mut grad = vec![0.0; xv.len()];
        for (i, (a, b)) in xv.data.iter().zip(&tv.data).enumerate() {
            let (r, col) = (i / c, i % c);
            let mut w = mask.map(|m| m[r]).unwrap_or(1.0);
            if let Some(l) = col_lim {
                if col >= l[r / seg] as usize {
                    w = 0.0;
                }
            }
            if w == 0.0 {
                continue;
            }
            let e = a - b;
            if sq {
                loss += w * e * e;
                grad[i] = 2.0 * w * e / denom;
            } else {
                loss += w * e.abs();
                grad[i] = w * e.signum() / denom;
            }
        }
        let back: Back = Box::new(move |d, g| {
            let s = d.host()[0];
            g.add_vec(x, grad.iter().map(|v| v * s).collect())
        });
        self.push(Tensor::scalar(loss / denom), &[x], Some(back))
    }

    /// Binary cross-entropy with logits, x against target 0/1 (same shape).
    pub fn bce_logits(&mut self, x: Id, target: Id) -> Id {
        gpu!(self, crate::nn_gpu::bce(self, x, target));
        let (xv, tv) = (self.vals[x].clone(), self.vals[target].clone());
        let n = xv.len() as f32;
        let mut loss = 0.0;
        let mut grad = vec![0.0; xv.len()];
        for (i, (z, y)) in xv.data.iter().zip(&tv.data).enumerate() {
            loss += z.max(0.0) - z * y + (1.0 + (-z.abs()).exp()).ln();
            grad[i] = (1.0 / (1.0 + (-z).exp()) - y) / n;
        }
        let back: Back = Box::new(move |d, g| {
            let s = d.host()[0];
            g.add_vec(x, grad.iter().map(|v| v * s).collect())
        });
        self.push(Tensor::scalar(loss / n), &[x], Some(back))
    }

    /// Weighted sum of scalars.
    pub fn sum_scalars(&mut self, terms: &[(Id, f32)]) -> Id {
        gpu!(self, crate::nn_gpu::sum_scalars(self, terms));
        let v: f32 = terms.iter().map(|(id, w)| self.vals[*id].data[0] * w).sum();
        let terms = terms.to_vec();
        let ids: Vec<Id> = terms.iter().map(|t| t.0).collect();
        let back: Back = Box::new(move |d, g| {
            let d = d.host()[0];
            for (id, w) in &terms {
                g.add(*id, &[d * w]);
            }
        });
        self.push(Tensor::scalar(v), &ids, Some(back))
    }

    // --- audio ops ---------------------------------------------------------------

    /// The vocoder's source filter: Y = exp(gh) e^{i theta} H + exp(gn) N, with
    /// theta from the unnormalised (pa, pb); all [T, F]; H = (hr, hi) and
    /// N = (nr, ni) the fixed source spectra. Output [T, 2F] = [re | im].
    #[allow(clippy::too_many_arguments)]
    pub fn hn_filter(&mut self, gh: Id, gn: Id, pa: Id, pb: Id, hr: Id, hi: Id, nr: Id, ni: Id) -> Id {
        gpu!(self, crate::nn_gpu::hn_filter(self, [gh, gn, pa, pb], [hr, hi, nr, ni]));
        let [ghv, gnv, pav, pbv, hrv, hiv, nrv, niv] = [gh, gn, pa, pb, hr, hi, nr, ni].map(|i| self.vals[i].clone());
        let (t, f) = (ghv.rows, ghv.cols);
        let mut y = vec![0.0; t * 2 * f];
        for r in 0..t {
            for q in 0..f {
                let i = r * f + q;
                let ah = act_fwd(Act::Exp, ghv.data[i]);
                let an = act_fwd(Act::Exp, gnv.data[i]);
                let (a, b) = (pav.data[i], pbv.data[i]);
                let m = (a * a + b * b + 1e-6).sqrt();
                let (cs, sn) = (a / m, b / m);
                let (xr, xi) = (hrv.data[i] * cs - hiv.data[i] * sn, hrv.data[i] * sn + hiv.data[i] * cs);
                y[r * 2 * f + q] = ah * xr + an * nrv.data[i];
                y[r * 2 * f + f + q] = ah * xi + an * niv.data[i];
            }
        }
        let back: Back = Box::new(move |d, g| {
            let d = d.host();
            let mut dgh = vec![0.0; t * f];
            let mut dgn = vec![0.0; t * f];
            let mut dpa = vec![0.0; t * f];
            let mut dpb = vec![0.0; t * f];
            for r in 0..t {
                for q in 0..f {
                    let i = r * f + q;
                    let (dyr, dyi) = (d[r * 2 * f + q], d[r * 2 * f + f + q]);
                    let ah = act_fwd(Act::Exp, ghv.data[i]);
                    let an = act_fwd(Act::Exp, gnv.data[i]);
                    let (a, b) = (pav.data[i], pbv.data[i]);
                    let m2 = a * a + b * b + 1e-6;
                    let m = m2.sqrt();
                    let (cs, sn) = (a / m, b / m);
                    let (hr, hi) = (hrv.data[i], hiv.data[i]);
                    let (xr, xi) = (hr * cs - hi * sn, hr * sn + hi * cs);
                    let gate_h = if ghv.data[i] < 10.0 { 1.0 } else { 0.0 };
                    let gate_n = if gnv.data[i] < 10.0 { 1.0 } else { 0.0 };
                    dgh[i] = (dyr * xr + dyi * xi) * ah * gate_h;
                    dgn[i] = (dyr * nrv.data[i] + dyi * niv.data[i]) * an * gate_n;
                    let dcs = ah * (dyr * hr + dyi * hi);
                    let dsn = ah * (-dyr * hi + dyi * hr);
                    let m3 = m2 * m;
                    dpa[i] = dcs * (b * b + 1e-6) / m3 - dsn * a * b / m3;
                    dpb[i] = dsn * (a * a + 1e-6) / m3 - dcs * a * b / m3;
                }
            }
            g.add_vec(gh, dgh);
            g.add_vec(gn, dgn);
            g.add_vec(pa, dpa);
            g.add_vec(pb, dpb);
        });
        let out = Tensor::new(t, 2 * f, y).meta_like(&self.vals[gh]);
        self.push(out, &[gh, gn, pa, pb], Some(back))
    }

    /// iSTFT of [B*T, 2F] (re | im, T = frames per item = seg) to waveforms
    /// [B*len, 1] with `len` samples per item.
    pub fn istft(&mut self, y: Id, st: Stft, len: usize) -> Id {
        gpu!(self, crate::nn_gpu::istft(self, y, st, len));
        let yv = self.vals[y].clone();
        let (t, f2, items) = (yv.seg, yv.cols, yv.items());
        let f = f2 / 2;
        let split = |data: &[f32], b: usize| {
            let mut re = vec![0.0; t * f];
            let mut im = vec![0.0; t * f];
            for r in 0..t {
                let row = (b * t + r) * f2;
                re[r * f..(r + 1) * f].copy_from_slice(&data[row..row + f]);
                im[r * f..(r + 1) * f].copy_from_slice(&data[row + f..row + f2]);
            }
            (re, im)
        };
        let mut wave = Vec::with_capacity(items * len);
        for b in 0..items {
            let (re, im) = split(&yv.data, b);
            wave.extend(st.inverse(&re, &im, t, len));
        }
        let back: Back = Box::new(move |d, g| {
            let d = d.host();
            let mut dy = vec![0.0; items * t * f2];
            for b in 0..items {
                let (dr, di) = st.inverse_adjoint(&d[b * len..(b + 1) * len], t);
                for r in 0..t {
                    let row = (b * t + r) * f2;
                    dy[row..row + f].copy_from_slice(&dr[r * f..(r + 1) * f]);
                    dy[row + f..row + f2].copy_from_slice(&di[r * f..(r + 1) * f]);
                }
            }
            g.add_vec(y, dy);
        });
        self.push(Tensor::batched(items * len, 1, wave, len, None), &[y], Some(back))
    }

    /// STFT magnitude of waveforms [B*len, 1]: [B*frames, bins].
    pub fn stft_mag(&mut self, x: Id, st: Stft) -> Id {
        gpu!(self, crate::nn_gpu::stft_mag(self, x, st));
        let xv = self.vals[x].clone();
        let (len, items) = (xv.seg, xv.items());
        let bins = st.bins();
        let frames = st.frames(len);
        let mut res = Vec::new();
        let mut ims = Vec::new();
        let mut mag = Vec::with_capacity(items * frames * bins);
        for b in 0..items {
            let (re, im) = st.forward(&xv.data[b * len..(b + 1) * len]);
            mag.extend(re.iter().zip(&im).map(|(a, b)| (a * a + b * b + 1e-9).sqrt()));
            res.push(re);
            ims.push(im);
        }
        let magc = Arc::new(mag.clone());
        let back: Back = Box::new(move |d, g| {
            let d = d.host();
            let mut dx = Vec::with_capacity(items * len);
            for b in 0..items {
                let o = b * frames * bins;
                let (re, im) = (&res[b], &ims[b]);
                let dre: Vec<f32> = (0..frames * bins).map(|i| d[o + i] * re[i] / magc[o + i]).collect();
                let dim: Vec<f32> = (0..frames * bins).map(|i| d[o + i] * im[i] / magc[o + i]).collect();
                dx.extend(st.forward_adjoint(&dre, &dim, len));
            }
            g.add_vec(x, dx);
        });
        self.push(Tensor::batched(items * frames, bins, mag, frames, None), &[x], Some(back))
    }

    /// The source waveforms of a vocoder batch (made on the device on a GPU graph).
    pub fn source_waves(&self, src: &crate::vocoder::SourceCtl) -> (Tensor, Tensor) {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if self.gpu && !src.host {
            return crate::nn_gpu::source_waves(src);
        }
        src.host_waves()
    }

    /// The complex STFT of constant waveforms [B*len, 1] (no gradient), the
    /// first `frames` frames of each item: (re, im) inputs [B*frames, bins].
    pub fn stft_const(&mut self, waves: Tensor, st: Stft, frames: usize) -> (Id, Id) {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if self.gpu {
            let waves = if waves.dev.is_none() { crate::nn_gpu::upload(waves) } else { waves };
            let (re, im) = crate::nn_gpu::stft_complex(&waves, st, frames);
            return (self.input(re), self.input(im));
        }
        let (len, items, bins) = (waves.seg, waves.items(), st.bins());
        let mut re = Vec::with_capacity(items * frames * bins);
        let mut im = Vec::with_capacity(items * frames * bins);
        for b in 0..items {
            let (r, i) = st.forward(&waves.data[b * len..(b + 1) * len]);
            re.extend_from_slice(&r[..frames * bins]);
            im.extend_from_slice(&i[..frames * bins]);
        }
        let re = self.input(Tensor::batched(items * frames, bins, re, frames, None));
        let im = self.input(Tensor::batched(items * frames, bins, im, frames, None));
        (re, im)
    }

    /// ln(x + eps).
    pub fn log_eps(&mut self, x: Id, eps: f32) -> Id {
        gpu!(self, crate::nn_gpu::log_eps(self, x, eps));
        let xv = self.vals[x].clone();
        let y = xv.data.iter().map(|v| (v + eps).ln()).collect();
        let (rows, cols) = (xv.rows, xv.cols);
        let xv2 = xv.clone();
        let back: Back = Box::new(move |d, g| g.add_vec(x, d.host().iter().zip(&xv2.data).map(|(d, v)| d / (v + eps)).collect()));
        let out = Tensor::new(rows, cols, y).meta_like(&xv);
        self.push(out, &[x], Some(back))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks d(loss)/d(param) against central differences for a graph builder.
    pub(crate) fn grad_check(params: &mut Params, name: &str, build: &dyn Fn(&mut Graph) -> Id) {
        let pid = params.id(name);
        let analytic = {
            let mut g = Graph::new(params, true);
            let loss = build(&mut g);
            g.backward(loss)[pid].as_ref().expect("no gradient").to_host()
        };
        let n = params.vals[pid].len();
        for i in (0..n).step_by((n / 7).max(1)) {
            let eps = 3e-3;
            let orig = params.vals[pid].data[i];
            let mut eval = |v: f32| {
                let mut t = (*params.vals[pid]).clone();
                t.data[i] = v;
                params.vals[pid] = Arc::new(t);
                let mut g = Graph::new(params, false);
                let l = build(&mut g);
                g.val(l).data[0] as f64
            };
            let num = (eval(orig + eps) - eval(orig - eps)) / (2.0 * eps as f64);
            eval(orig);
            let a = analytic[i] as f64;
            assert!((num - a).abs() <= 3e-2 * num.abs().max(a.abs()) + 2e-3, "{name}[{i}]: numeric {num} vs analytic {a}");
        }
    }

    #[test]
    fn gradients_match_finite_differences() {
        force_cpu(true);
        let mut rng = Rng::new(7);
        let mut p = Params::new();
        p.declare("x", 12, 8, Init::Normal(1.0), &mut rng);
        p.declare("w", 8, 8 * 3, Init::Fan(1.0), &mut rng);
        p.declare("b", 1, 8, Init::Normal(0.1), &mut rng);
        p.declare("dw", 5, 8, Init::Normal(0.3), &mut rng);
        p.declare("g", 1, 8, Init::Normal(1.0), &mut rng);
        p.declare("wq", 8, 8, Init::Fan(1.0), &mut rng);
        let target = Tensor::new(12, 8, (0..96).map(|_| rng.normal()).collect());
        let mask = [1.0, 0.0, 1.0, 1.0, 0.5, 1.0, 1.0, 1.0, 1.0, 0.0, 1.0, 1.0];
        let build = move |g: &mut Graph| {
            // Two items of 6 rows; the second has 4 valid keys.
            let x0 = g.p("x");
            let x = g.gather_rows(x0, &(0..12).collect::<Vec<_>>(), 6, Some(vec![6, 4]));
            let w = g.p("w");
            let b = g.p("b");
            let y = g.conv1d(x, w, Some(b), 3, 1);
            let dw = g.p("dw");
            let b2 = g.p("b");
            let y = g.dwconv1d(y, dw, b2, 5);
            let gg = g.p("g");
            let bb = g.p("b");
            let y = g.layer_norm(y, gg, bb);
            let y = g.act(y, Act::Gelu);
            let wq = g.p("wq");
            let q = g.linear(y, wq, None);
            let q = g.rope(q, 2);
            let a = g.attention(q, y, y, 2);
            let a = g.glu(a);
            let a2 = g.concat_cols(&[a, a]);
            let t = g.input(target.clone());
            let l1 = g.mse_loss(a2, t, None, Some(&[8, 5]));
            let l2 = g.mse_loss(y, t, Some(&mask), None);
            g.sum_scalars(&[(l1, 1.0), (l2, 0.5)])
        };
        for name in ["x", "w", "b", "dw", "g", "wq"] {
            grad_check(&mut p, name, &build);
        }
    }

    #[test]
    fn audio_op_gradients_match_finite_differences() {
        force_cpu(true);
        let mut rng = Rng::new(3);
        let st = Stft { n_fft: 64, hop: 16, win: 60 };
        let len = 192;
        let frames = len / 16;
        let f = st.bins();
        let mut p = Params::new();
        for n in ["gh", "gn", "pa", "pb"] {
            p.declare(n, 2 * frames, f, Init::Normal(0.5), &mut rng);
        }
        let mut spec = Vec::new();
        for _ in 0..2 {
            let src: Vec<f32> = (0..len).map(|_| rng.normal()).collect();
            let (r, i) = st.forward(&src);
            spec.push((r[..frames * f].to_vec(), i[..frames * f].to_vec()));
        }
        let cat = |k: usize| -> Vec<f32> { spec.iter().flat_map(|s| if k == 0 { s.0.clone() } else { s.1.clone() }).collect() };
        let (hr, hi) = (cat(0), cat(1));
        let (nr, ni) = (hi.clone(), hr.clone());
        let st2 = Stft { n_fft: 32, hop: 8, win: 32 };
        let f2 = st2.frames(len);
        let tm: Vec<f32> = (0..2 * f2 * st2.bins()).map(|i| ((i as f32) * 0.37).sin()).collect();
        let build = |g: &mut Graph| {
            let (a, b, c, d) = (g.p("gh"), g.p("gn"), g.p("pa"), g.p("pb"));
            let a = g.gather_rows(a, &(0..2 * frames).collect::<Vec<_>>(), frames, None);
            let mk = |g: &mut Graph, v: &Vec<f32>| g.input(Tensor::batched(2 * frames, f, v.clone(), frames, None));
            let (h1, h2, n1, n2) = (mk(g, &hr), mk(g, &hi), mk(g, &nr), mk(g, &ni));
            let y = g.hn_filter(a, b, c, d, h1, h2, n1, n2);
            let w = g.istft(y, st, len);
            let m = g.stft_mag(w, st2);
            let lm = g.log_eps(m, 1e-3);
            let t = g.input(Tensor::batched(2 * f2, st2.bins(), tm.clone(), f2, None));
            g.mse_loss(lm, t, None, Some(&[st2.bins() as u32, 9]))
        };
        for name in ["gh", "gn", "pa", "pb"] {
            grad_check(&mut p, name, &build);
        }
    }
}
