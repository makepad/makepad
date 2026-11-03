//! The CPU device: a session's buffers as words in host memory, its kernels on the AIR backends
//! over the executor's workers, and the library ops in Rust. Every result is bit-identical for
//! any worker count, and between the interpreter and native code.
//!
//! The library reductions use a fixed tree: blocks of 256 values (the identity past the end),
//! each folded as `s[t] = op(s[t], s[t + w])` for w = 128, 64, .. 1, then the same over the
//! block results until one is left. A GPU workgroup of 256 folds its shared memory in the same
//! order, so the two devices agree.
use makepad_script_compute::kernel::{Access, KernelRef};
use makepad_script_compute::Backend;
use makepad_script_compute_sched::kernel::{CancelToken, KernelError, KernelExt, RunStats};
use makepad_script_compute_sched::sched::{Executor, ThreadExecutor};
use std::sync::{Arc, Mutex};

/// A buffer's element type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElemTy {
    F32,
    I32,
    U32,
    /// Two words: low, then high (the kernels' `f64`).
    F64,
    Vec2,
    Vec3,
    Vec4,
}

impl ElemTy {
    pub fn words(self) -> usize {
        match self {
            ElemTy::F32 | ElemTy::I32 | ElemTy::U32 => 1,
            ElemTy::F64 | ElemTy::Vec2 => 2,
            ElemTy::Vec3 => 3,
            ElemTy::Vec4 => 4,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ElemTy::F32 => "f32",
            ElemTy::I32 => "i32",
            ElemTy::U32 => "u32",
            ElemTy::F64 => "f64",
            ElemTy::Vec2 => "vec2",
            ElemTy::Vec3 => "vec3",
            ElemTy::Vec4 => "vec4",
        }
    }

    pub fn from_name(name: &str) -> Option<ElemTy> {
        Some(match name {
            "f32" => ElemTy::F32,
            "i32" => ElemTy::I32,
            "u32" => ElemTy::U32,
            "f64" => ElemTy::F64,
            "vec2" => ElemTy::Vec2,
            "vec3" => ElemTy::Vec3,
            "vec4" => ElemTy::Vec4,
            _ => return None,
        })
    }

    /// Whether its words are f32 components.
    pub fn is_f32(self) -> bool {
        matches!(self, ElemTy::F32 | ElemTy::Vec2 | ElemTy::Vec3 | ElemTy::Vec4)
    }
}

pub(crate) enum Data {
    Owned(Vec<u32>),
    /// A session input, shared with the app by reference: read-only.
    Shared(Arc<[u32]>),
}

impl Data {
    pub(crate) fn words(&self) -> &[u32] {
        match self {
            Data::Owned(v) => v,
            Data::Shared(a) => a,
        }
    }
}

pub(crate) struct Buffer {
    pub ty: ElemTy,
    pub data: Data,
}

impl Buffer {
    pub(crate) fn len_words(&self) -> usize {
        self.data.words().len()
    }

    pub(crate) fn read_only(&self) -> bool {
        matches!(self.data, Data::Shared(_))
    }
}

struct Slot {
    gen: u32,
    buffer: Option<Buffer>,
}

/// A kernel run as recorded: the params by name, the kernel's buffers bound to session buffers,
/// and an optional repeat (`round` or `layer` set per round).
pub(crate) struct Run {
    pub kernel: usize,
    pub count: usize,
    pub params: Vec<(String, f32)>,
    /// (the kernel's buffer index, the session buffer)
    pub bind: Vec<(usize, usize)>,
    pub repeat: Option<Repeat>,
}

pub(crate) struct Repeat {
    pub rounds: u32,
    pub param: &'static str,
    /// Rounds count down (a sweep from the top layer).
    pub down: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Fold {
    Sum,
    Min,
    Max,
}

impl Fold {
    fn identity(self) -> f32 {
        match self {
            Fold::Sum => 0.0,
            Fold::Min => f32::INFINITY,
            Fold::Max => f32::NEG_INFINITY,
        }
    }

    #[inline(always)]
    fn apply(self, a: f32, b: f32) -> f32 {
        match self {
            Fold::Sum => a + b,
            Fold::Min => {
                if b < a {
                    b
                } else {
                    a
                }
            }
            Fold::Max => {
                if b > a {
                    b
                } else {
                    a
                }
            }
        }
    }
}

/// A grid the multigrid transfers walk: the fine level's cells per axis (even), whether values
/// live on its vertices (`n + 1` per axis) or cells, and the components per point.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Grid {
    pub cells: [usize; 3],
    pub vertex: bool,
    pub comps: usize,
}

impl Grid {
    pub(crate) fn coarse(&self) -> Grid {
        Grid { cells: [self.cells[0] / 2, self.cells[1] / 2, self.cells[2] / 2], ..*self }
    }

    pub(crate) fn points(&self) -> [usize; 3] {
        let e = self.vertex as usize;
        [self.cells[0] + e, self.cells[1] + e, self.cells[2] + e]
    }

    pub(crate) fn words(&self) -> usize {
        let p = self.points();
        p[0] * p[1] * p[2] * self.comps
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Transfer {
    /// Vertex restriction by the transpose of trilinear prolongation (Galerkin MG).
    Transpose,
    /// The transpose scaled by 1/8.
    FullWeight,
    /// Cell restriction: the sum of the 8 children, or their mean.
    Sum,
    Average,
    /// Vertex prolongation.
    Trilinear,
    /// Cell prolongation: each child takes its parent.
    Injection,
}

pub(crate) enum Op {
    Fill { buf: usize, word: u32 },
    Copy { src: usize, dst: usize },
    Write { buf: usize, at: usize, words: Vec<u32> },
    Free { buf: usize },
    Run(Run),
    /// A reduce kernel's lanes into `block[slot..]`.
    Reduce { run: Run, block: usize, slot: usize },
    /// The tree reduction of `a` (or of `a[i] * b[i]`) into `block[slot]`.
    Fold { a: usize, b: Option<usize>, fold: Fold, block: usize, slot: usize },
    /// Exclusive prefix sum of integers; `dst[n]` takes the total when it is there.
    Scan { src: usize, dst: usize },
    Restrict { fine: usize, coarse: usize, grid: Grid, kind: Transfer },
    Prolong { coarse: usize, fine: usize, grid: Grid, kind: Transfer },
}

impl Op {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Op::Fill { .. } => "fill",
            Op::Copy { .. } => "copy",
            Op::Write { .. } => "write",
            Op::Free { .. } => "free",
            Op::Run(_) | Op::Reduce { .. } => "",
            Op::Fold { b: Some(_), .. } => "dot",
            Op::Fold { fold: Fold::Sum, .. } => "sum",
            Op::Fold { fold: Fold::Min, .. } => "min",
            Op::Fold { fold: Fold::Max, .. } => "max",
            Op::Scan { .. } => "scan",
            Op::Restrict { .. } => "restrict",
            Op::Prolong { .. } => "prolong",
        }
    }
}

/// What a device run of one op did, for the stats and the work budget.
#[derive(Default)]
pub(crate) struct OpDone {
    pub work: u64,
    pub nanos: u64,
    pub elements: u64,
}

pub(crate) enum OpError {
    Kernel(KernelError),
}

/// The running call's cancel token, where the session's cancel and interrupt reach it.
#[derive(Default)]
pub(crate) struct CancelSlot(Mutex<Option<CancelToken>>);

impl CancelSlot {
    pub(crate) fn set(&self, token: Option<CancelToken>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = token;
    }

    pub(crate) fn cancel(&self) {
        if let Some(token) = self.0.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            token.cancel();
        }
    }
}

pub(crate) struct CpuDevice {
    slots: Vec<Slot>,
    free_slots: Vec<usize>,
    kernels: Vec<KernelRef>,
    exec: Option<Arc<dyn Executor>>,
    threads: usize,
    backend: Backend,
    seed: u32,
}

/// Elements below which a library op runs on the caller alone.
const PARALLEL_MIN: usize = 1 << 16;
const BLOCK: usize = 256;

impl CpuDevice {
    pub(crate) fn new(exec: Option<Arc<dyn Executor>>, threads: usize, backend: Backend, seed: u32) -> CpuDevice {
        CpuDevice { slots: Vec::new(), free_slots: Vec::new(), kernels: Vec::new(), exec, threads: threads.max(1), backend, seed }
    }

    pub(crate) fn backend(&self) -> Backend {
        self.backend
    }

    pub(crate) fn threads(&self) -> usize {
        self.threads
    }

    fn exec(&self) -> &dyn Executor {
        match &self.exec {
            Some(e) => &**e,
            None => ThreadExecutor::shared(),
        }
    }

    /// A new buffer (zeros) and its (slot, generation).
    pub(crate) fn alloc(&mut self, ty: ElemTy, data: Data) -> (usize, u32) {
        let buffer = Some(Buffer { ty, data });
        if let Some(slot) = self.free_slots.pop() {
            let s = &mut self.slots[slot];
            s.gen = s.gen.wrapping_add(1);
            s.buffer = buffer;
            return (slot, s.gen);
        }
        self.slots.push(Slot { gen: 1, buffer });
        (self.slots.len() - 1, 1)
    }

    /// The live buffer a handle names.
    pub(crate) fn get(&self, slot: usize, gen: u32) -> Option<&Buffer> {
        self.slots.get(slot).filter(|s| s.gen == gen).and_then(|s| s.buffer.as_ref())
    }

    pub(crate) fn buffer(&self, slot: usize) -> &Buffer {
        self.slots[slot].buffer.as_ref().expect("a recorded op names a live buffer")
    }

    fn buffer_mut(&mut self, slot: usize) -> &mut Buffer {
        self.slots[slot].buffer.as_mut().expect("a recorded op names a live buffer")
    }

    fn owned(&mut self, slot: usize) -> &mut Vec<u32> {
        match &mut self.buffer_mut(slot).data {
            Data::Owned(v) => v,
            Data::Shared(_) => unreachable!("written buffers are checked at record time"),
        }
    }

    /// A source buffer and a distinct, written destination (checked at record time).
    fn pair(&mut self, src: usize, dst: usize) -> (&[u32], &mut Vec<u32>) {
        let (from, to) = if src < dst {
            let (l, r) = self.slots.split_at_mut(dst);
            (&l[src], &mut r[0])
        } else {
            let (l, r) = self.slots.split_at_mut(src);
            (&r[0], &mut l[dst])
        };
        let from = from.buffer.as_ref().expect("a recorded op names a live buffer").data.words();
        match &mut to.buffer.as_mut().expect("a recorded op names a live buffer").data {
            Data::Owned(v) => (from, v),
            Data::Shared(_) => unreachable!("written buffers are checked at record time"),
        }
    }

    /// Invalidates the handle at once (its slot is reused only after the free op ran).
    pub(crate) fn retire(&mut self, slot: usize) {
        let s = &mut self.slots[slot];
        s.gen = s.gen.wrapping_add(1);
    }

    pub(crate) fn add_kernel(&mut self, kernel: KernelRef) -> usize {
        self.kernels.push(kernel);
        self.kernels.len() - 1
    }

    pub(crate) fn kernel(&self, k: usize) -> &KernelRef {
        &self.kernels[k]
    }

    pub(crate) fn read(&self, slot: usize, at: usize, n: usize) -> &[u32] {
        &self.buffer(slot).data.words()[at..at + n]
    }

    /// Runs one op. `work_left` is the session's remaining kernel work.
    pub(crate) fn execute(&mut self, op: Op, work_left: u64, cancel: &CancelSlot) -> Result<OpDone, OpError> {
        match op {
            Op::Fill { buf, word } => {
                let v = self.owned(buf);
                v.fill(word);
                Ok(OpDone { elements: v.len() as u64, ..OpDone::default() })
            }
            Op::Copy { src, dst } => {
                let (from, to) = self.pair(src, dst);
                to.copy_from_slice(from);
                Ok(OpDone { elements: from.len() as u64, ..OpDone::default() })
            }
            Op::Write { buf, at, words } => {
                self.owned(buf)[at..at + words.len()].copy_from_slice(&words);
                Ok(OpDone { elements: words.len() as u64, ..OpDone::default() })
            }
            Op::Free { buf } => {
                self.slots[buf].buffer = None;
                self.free_slots.push(buf);
                Ok(OpDone::default())
            }
            Op::Run(run) => self.run(&run, work_left, cancel).map(|(done, _)| done),
            Op::Reduce { run, block, slot } => {
                let (done, lanes) = self.run(&run, work_left, cancel)?;
                let out = self.owned(block);
                for (k, x) in lanes.iter().enumerate() {
                    out[slot + k] = x.to_bits();
                }
                Ok(done)
            }
            Op::Fold { a, b, fold, block, slot } => {
                let n = self.buffer(a).len_words();
                let x = {
                    let av = self.buffer(a).data.words();
                    match b {
                        Some(b) => {
                            let bv = self.buffer(b).data.words();
                            self.fold_tree(n, fold, &|i| f32::from_bits(av[i]) * f32::from_bits(bv[i]))
                        }
                        None => self.fold_tree(n, fold, &|i| f32::from_bits(av[i])),
                    }
                };
                self.owned(block)[slot] = x.to_bits();
                Ok(OpDone { elements: n as u64, ..OpDone::default() })
            }
            Op::Scan { src, dst } => {
                let (input, out) = self.pair(src, dst);
                let n = input.len();
                let mut acc = 0u32;
                for (o, x) in out.iter_mut().zip(input) {
                    *o = acc;
                    acc = acc.wrapping_add(*x);
                }
                if out.len() > n {
                    out[n] = acc;
                }
                Ok(OpDone { elements: n as u64, ..OpDone::default() })
            }
            Op::Restrict { fine, coarse, grid, kind } => {
                let (from, to) = self.pair(fine, coarse);
                restrict(grid, kind, from, to);
                Ok(OpDone { elements: grid.coarse().words() as u64, ..OpDone::default() })
            }
            Op::Prolong { coarse, fine, grid, kind } => {
                let (from, to) = self.pair(coarse, fine);
                prolong(grid, kind, from, to);
                Ok(OpDone { elements: grid.words() as u64, ..OpDone::default() })
            }
        }
    }

    /// One kernel run (every round of a repeat): its stats and a reduce kernel's lanes.
    fn run(&mut self, run: &Run, work_left: u64, cancel: &CancelSlot) -> Result<(OpDone, Vec<f32>), OpError> {
        let kernel = self.kernels[run.kernel].clone();
        // Written buffers move out of the table for the call (a written buffer binds once),
        // read ones are borrowed in place.
        let mut written: Vec<(usize, usize, Vec<u32>)> = Vec::new();
        for &(kb, b) in &run.bind {
            if kernel.buffer_view(kb).access != Access::Read {
                let words = std::mem::replace(self.owned(b), Vec::new());
                written.push((kb, b, words));
            }
        }
        let result = self.call(&kernel, run, &mut written, work_left, cancel);
        for (_, b, words) in written {
            *self.owned(b) = words;
        }
        result
    }

    fn call(&self, kernel: &KernelRef, run: &Run, written: &mut [(usize, usize, Vec<u32>)], work_left: u64, cancel: &CancelSlot) -> Result<(OpDone, Vec<f32>), OpError> {
        let mut call = kernel.call();
        for (name, value) in &run.params {
            call.set_param(name, *value);
        }
        call.set_seed(self.seed);
        for &(kb, b) in &run.bind {
            let view = kernel.buffer_view(kb);
            if view.access == Access::Read {
                call.input_u32(view.name, self.buffer(b).data.words()).map_err(OpError::Kernel)?;
            }
        }
        for (kb, _, words) in written.iter_mut() {
            call.output_u32(kernel.buffer_view(*kb).name, words).map_err(OpError::Kernel)?;
        }
        cancel.set(Some(call.cancel_token()));
        let rounds = run.repeat.as_ref().map_or(1, |r| r.rounds);
        let mut done = OpDone::default();
        let mut lanes = Vec::new();
        let mut result = Ok(());
        for round in 0..rounds {
            if let Some(r) = &run.repeat {
                let index = if r.down { rounds - 1 - round } else { round };
                call.set_param(r.param, index as f32);
            }
            call.set_work_limit(work_left.saturating_sub(done.work).max(1));
            let stats: Result<RunStats, KernelError> = match self.backend {
                Backend::Interp => call.run_interp(run.count),
                Backend::Native => call.run_on_executor(run.count, self.exec(), self.threads),
            };
            match stats {
                Ok(stats) => {
                    done.work += stats.work;
                    done.nanos += stats.nanos;
                    done.elements += stats.elements as u64;
                    lanes = stats.reduced;
                }
                Err(e) => {
                    result = Err(OpError::Kernel(e));
                    break;
                }
            }
        }
        cancel.set(None);
        result.map(|_| (done, lanes))
    }

    /// The fixed-tree fold of `n` values (see the module docs).
    fn fold_tree(&self, n: usize, fold: Fold, value: &(dyn Fn(usize) -> f32 + Sync)) -> f32 {
        if n == 0 {
            return fold.identity();
        }
        let blocks = n.div_ceil(BLOCK);
        let mut level = vec![0f32; blocks];
        let fold_block = |b: usize| {
            let mut s = [fold.identity(); BLOCK];
            let start = b * BLOCK;
            for (t, x) in s.iter_mut().enumerate().take(n.saturating_sub(start).min(BLOCK)) {
                *x = value(start + t);
            }
            fold_256(&mut s, fold)
        };
        let parts = if n >= PARALLEL_MIN && self.backend == Backend::Native { (self.exec().workers() + 1).min(self.threads).min(blocks) } else { 1 };
        if parts <= 1 {
            for (b, x) in level.iter_mut().enumerate() {
                *x = fold_block(b);
            }
        } else {
            let per = blocks.div_ceil(parts);
            let chunks: Vec<Mutex<&mut [f32]>> = level.chunks_mut(per).map(Mutex::new).collect();
            self.exec().fan_out(chunks.len(), &|w| {
                let mut part = chunks[w].lock().unwrap_or_else(|e| e.into_inner());
                for (k, x) in part.iter_mut().enumerate() {
                    *x = fold_block(w * per + k);
                }
            });
        }
        while level.len() > 1 {
            level = level
                .chunks(BLOCK)
                .map(|chunk| {
                    let mut s = [fold.identity(); BLOCK];
                    s[..chunk.len()].copy_from_slice(chunk);
                    fold_256(&mut s, fold)
                })
                .collect();
        }
        level[0]
    }
}

#[inline(always)]
fn fold_256(s: &mut [f32; BLOCK], fold: Fold) -> f32 {
    let mut w = BLOCK / 2;
    while w > 0 {
        for t in 0..w {
            s[t] = fold.apply(s[t], s[t + w]);
        }
        w /= 2;
    }
    s[0]
}

/// Per axis, the fine points a coarse vertex gathers from (offset, weight): the transpose of
/// trilinear prolongation.
fn vertex_taps(coarse: usize, fine_points: usize) -> impl Iterator<Item = (usize, f32)> {
    let centre = 2 * coarse;
    [(-1i64, 0.5f32), (0, 1.0), (1, 0.5)].into_iter().filter_map(move |(d, w)| {
        let f = centre as i64 + d;
        (f >= 0 && (f as usize) < fine_points).then_some((f as usize, w))
    })
}

/// Per axis, the coarse vertices a fine vertex interpolates (index, weight).
fn trilinear_taps(fine: usize) -> impl Iterator<Item = (usize, f32)> {
    let c = fine / 2;
    let odd = fine % 2 == 1;
    [(c, if odd { 0.5 } else { 1.0 }), (c + 1, 0.5)].into_iter().take(if odd { 2 } else { 1 })
}

fn index(p: [usize; 3], x: usize, y: usize, z: usize) -> usize {
    (z * p[1] + y) * p[0] + x
}

fn restrict(grid: Grid, kind: Transfer, fine: &[u32], out: &mut [u32]) {
    let (fp, cp, comps) = (grid.points(), grid.coarse().points(), grid.comps);
    let f = |i: usize| f32::from_bits(fine[i]);
    for z in 0..cp[2] {
        for y in 0..cp[1] {
            for x in 0..cp[0] {
                for c in 0..comps {
                    let mut acc = 0.0f32;
                    if grid.vertex {
                        let scale = if kind == Transfer::FullWeight { 0.125 } else { 1.0 };
                        for (fz, wz) in vertex_taps(z, fp[2]) {
                            for (fy, wy) in vertex_taps(y, fp[1]) {
                                for (fx, wx) in vertex_taps(x, fp[0]) {
                                    acc += wx * wy * wz * scale * f(index(fp, fx, fy, fz) * comps + c);
                                }
                            }
                        }
                    } else {
                        for dz in 0..2 {
                            for dy in 0..2 {
                                for dx in 0..2 {
                                    acc += f(index(fp, 2 * x + dx, 2 * y + dy, 2 * z + dz) * comps + c);
                                }
                            }
                        }
                        if kind == Transfer::Average {
                            acc *= 0.125;
                        }
                    }
                    out[index(cp, x, y, z) * comps + c] = acc.to_bits();
                }
            }
        }
    }
}

fn prolong(grid: Grid, kind: Transfer, coarse: &[u32], out: &mut [u32]) {
    let (fp, cp, comps) = (grid.points(), grid.coarse().points(), grid.comps);
    let g = |i: usize| f32::from_bits(coarse[i]);
    for z in 0..fp[2] {
        for y in 0..fp[1] {
            for x in 0..fp[0] {
                for c in 0..comps {
                    let v = if kind == Transfer::Injection {
                        g(index(cp, x / 2, y / 2, z / 2) * comps + c)
                    } else {
                        let mut acc = 0.0f32;
                        for (cz, wz) in trilinear_taps(z) {
                            for (cy, wy) in trilinear_taps(y) {
                                for (cx, wx) in trilinear_taps(x) {
                                    acc += wx * wy * wz * g(index(cp, cx, cy, cz) * comps + c);
                                }
                            }
                        }
                        acc
                    };
                    out[index(fp, x, y, z) * comps + c] = v.to_bits();
                }
            }
        }
    }
}

/// f32 to f16, rounding to nearest even (inf past the range, NaN kept quiet).
pub(crate) fn f16_bits(x: f32) -> u16 {
    let b = x.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32;
    let man = b & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if man != 0 { 0x200 | (man >> 13) as u16 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = man | 0x80_0000;
        let shift = (14 - e) as u32;
        let rounded = (m + (1 << (shift - 1)) - 1 + ((m >> shift) & 1)) >> shift;
        return sign | rounded as u16;
    }
    let m = man + 0xfff + ((man >> 13) & 1);
    sign | (((e as u32) << 10) + (m >> 13)) as u16
}

/// `x` in `[lo, hi]` as 0..=255, rounding to nearest even (NaN: 0).
pub(crate) fn u8_level(x: f32, lo: f32, hi: f32) -> u8 {
    let t = (x - lo) / (hi - lo);
    if t.is_nan() {
        return 0;
    }
    (t.clamp(0.0, 1.0) * 255.0).round_ties_even() as u8
}
