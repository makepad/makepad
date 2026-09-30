//! The CUDA backend of `nn::Graph`: every op's forward and backward on
//! device buffers (kernels in libs/ai/cuda/kernels/train.cu), GEMMs on
//! TF32 tensor cores. Nothing comes back to the host but what a caller reads.

use crate::dsp::{window, Stft};
use crate::nn::{Act, Back, GBuf, Graph, Id, Tensor};
use makepad_ai_cuda::train::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

fn dp(t: &Tensor) -> *const f32 {
    t.dev.as_ref().expect("host tensor on the GPU path").ptr()
}

fn st() -> makepad_ai_cuda::cudaStream_t {
    stream()
}

fn dev_out(rows: usize, cols: usize, buf: DevBuf, like: &Tensor) -> Tensor {
    let mut t = Tensor { rows, cols, data: Vec::new(), dev: Some(buf), seg: rows.max(1), lens: None };
    if rows == like.rows {
        t.seg = like.seg;
        t.lens = like.lens.clone();
    }
    t
}

pub fn upload(mut t: Tensor) -> Tensor {
    t.dev = Some(DevBuf::from_host(&t.data));
    t.data = Vec::new();
    t
}

pub fn copy(d: &DevBuf) -> DevBuf {
    let y = DevBuf::new(d.len());
    if !d.is_empty() {
        ck("copy", unsafe { mkt_scale(d.ptr(), y.mptr(), 1.0, d.len(), st()) });
    }
    y
}

pub fn axpy(x: &DevBuf, y: &DevBuf, s: f32) {
    ck("axpy", unsafe { mkt_axpy(x.ptr(), y.mptr(), s, x.len(), st()) });
}

thread_local! {
    static WINDOWS: RefCell<HashMap<(usize, usize), Arc<DevBuf>>> = RefCell::new(HashMap::new());
    static NORMS: RefCell<HashMap<(usize, usize, usize, usize), Arc<DevBuf>>> = RefCell::new(HashMap::new());
}

fn win_buf(s: Stft) -> Arc<DevBuf> {
    WINDOWS.with(|w| w.borrow_mut().entry((s.n_fft, s.win)).or_insert_with(|| Arc::new(DevBuf::from_host(&window(s.win, s.n_fft)))).clone())
}

fn norm_buf(s: Stft, frames: usize, len: usize) -> Arc<DevBuf> {
    NORMS.with(|w| w.borrow_mut().entry((s.n_fft, s.hop, frames, len)).or_insert_with(|| Arc::new(DevBuf::from_host(&s.overlap_norm(frames, len)))).clone())
}

pub fn act(g: &mut Graph, x: Id, a: Act) -> Id {
    let xv = g.vals[x].clone();
    let n = xv.len();
    let y = DevBuf::new(n);
    ck("act", unsafe { mkt_act_fwd(dp(&xv), y.mptr(), n, a as i32, st()) });
    let out = dev_out(xv.rows, xv.cols, y, &xv);
    let back: Back = Box::new(move |d, gr| {
        if let Some(dx) = gr.acc(x) {
            ck("act_bwd", unsafe { mkt_act_bwd(dp(&xv), d.dev().ptr(), dx.mptr(), n, a as i32, st()) });
        }
    });
    g.push(out, &[x], Some(back))
}

pub fn add(g: &mut Graph, a: Id, b: Id) -> Id {
    let (av, bv) = (g.vals[a].clone(), g.vals[b].clone());
    assert_eq!((av.rows, av.cols), (bv.rows, bv.cols), "add shapes");
    let n = av.len();
    let y = DevBuf::new(n);
    ck("add", unsafe { mkt_add(dp(&av), dp(&bv), y.mptr(), n, st()) });
    let out = dev_out(av.rows, av.cols, y, &av);
    let back: Back = Box::new(move |d, gr| {
        for id in [a, b] {
            if let Some(dx) = gr.acc(id) {
                axpy(d.dev(), dx, 1.0);
            }
        }
    });
    g.push(out, &[a, b], Some(back))
}

pub fn mul(g: &mut Graph, a: Id, b: Id) -> Id {
    let (av, bv) = (g.vals[a].clone(), g.vals[b].clone());
    let n = av.len();
    let y = DevBuf::new(n);
    ck("mul", unsafe { mkt_mul(dp(&av), dp(&bv), y.mptr(), n, st()) });
    let out = dev_out(av.rows, av.cols, y, &av);
    let back: Back = Box::new(move |d, gr| {
        if let Some(da) = gr.acc(a) {
            ck("mul_acc", unsafe { mkt_mul_acc(d.dev().ptr(), dp(&bv), da.mptr(), n, st()) });
        }
        if let Some(db) = gr.acc(b) {
            ck("mul_acc", unsafe { mkt_mul_acc(d.dev().ptr(), dp(&av), db.mptr(), n, st()) });
        }
    });
    g.push(out, &[a, b], Some(back))
}

pub fn scale(g: &mut Graph, x: Id, s: f32) -> Id {
    let xv = g.vals[x].clone();
    let n = xv.len();
    let y = DevBuf::new(n);
    ck("scale", unsafe { mkt_scale(dp(&xv), y.mptr(), s, n, st()) });
    let out = dev_out(xv.rows, xv.cols, y, &xv);
    let back: Back = Box::new(move |d, gr| {
        if let Some(dx) = gr.acc(x) {
            axpy(d.dev(), dx, s);
        }
    });
    g.push(out, &[x], Some(back))
}

pub fn add_row(g: &mut Graph, x: Id, v: Id) -> Id {
    let (xv, vv) = (g.vals[x].clone(), g.vals[v].clone());
    let (rows, cols) = (xv.rows, xv.cols);
    let y = DevBuf::new(rows * cols);
    ck("add_row", unsafe { mkt_add_row(dp(&xv), dp(&vv), y.mptr(), rows, cols, st()) });
    let out = dev_out(rows, cols, y, &xv);
    let back: Back = Box::new(move |d, gr| {
        if let Some(dx) = gr.acc(x) {
            axpy(d.dev(), dx, 1.0);
        }
        if let Some(dv) = gr.acc(v) {
            ck("col_sum", unsafe { mkt_col_dot_acc(d.dev().ptr(), std::ptr::null(), dv.mptr(), rows, cols, st()) });
        }
    });
    g.push(out, &[x, v], Some(back))
}

pub fn mul_row(g: &mut Graph, x: Id, v: Id) -> Id {
    let (xv, vv) = (g.vals[x].clone(), g.vals[v].clone());
    let (rows, cols) = (xv.rows, xv.cols);
    let y = DevBuf::new(rows * cols);
    ck("mul_row", unsafe { mkt_mul_row(dp(&xv), dp(&vv), y.mptr(), rows, cols, st()) });
    let out = dev_out(rows, cols, y, &xv);
    let back: Back = Box::new(move |d, gr| {
        if let Some(dx) = gr.acc(x) {
            ck("mul_row_bwd", unsafe { mkt_mul_row_bwd_x(d.dev().ptr(), dp(&vv), dx.mptr(), rows, cols, st()) });
        }
        if let Some(dv) = gr.acc(v) {
            ck("col_dot", unsafe { mkt_col_dot_acc(d.dev().ptr(), dp(&xv), dv.mptr(), rows, cols, st()) });
        }
    });
    g.push(out, &[x, v], Some(back))
}

pub fn mul_col(g: &mut Graph, x: Id, s: Id) -> Id {
    let (xv, sv) = (g.vals[x].clone(), g.vals[s].clone());
    let (rows, cols) = (xv.rows, xv.cols);
    let y = DevBuf::new(rows * cols);
    ck("mul_col", unsafe { mkt_mul_col(dp(&xv), dp(&sv), y.mptr(), rows, cols, st()) });
    let out = dev_out(rows, cols, y, &xv);
    let back: Back = Box::new(move |d, gr| {
        if let Some(dx) = gr.acc(x) {
            ck("mul_col_bwd", unsafe { mkt_mul_col_bwd_x(d.dev().ptr(), dp(&sv), dx.mptr(), rows, cols, st()) });
        }
        if let Some(ds) = gr.acc(s) {
            ck("row_dot", unsafe { mkt_row_dot_acc(d.dev().ptr(), dp(&xv), ds.mptr(), rows, cols, st()) });
        }
    });
    g.push(out, &[x, s], Some(back))
}

pub fn concat_cols(g: &mut Graph, parts: &[Id]) -> Id {
    let vals: Vec<Arc<Tensor>> = parts.iter().map(|p| g.vals[*p].clone()).collect();
    let rows = vals[0].rows;
    let widths: Vec<usize> = vals.iter().map(|v| v.cols).collect();
    let total: usize = widths.iter().sum();
    let y = DevBuf::new(rows * total);
    let mut off = 0;
    for (v, w) in vals.iter().zip(&widths) {
        ck("concat", unsafe { mkt_copy_cols(dp(v), *w, 0, y.mptr(), total, off, rows, *w, 0, st()) });
        off += w;
    }
    let out = dev_out(rows, total, y, &vals[0]);
    let ids = parts.to_vec();
    let back: Back = Box::new(move |d, gr| {
        let mut off = 0;
        for (id, w) in ids.iter().zip(&widths) {
            if let Some(dx) = gr.acc(*id) {
                ck("concat_bwd", unsafe { mkt_copy_cols(d.dev().ptr(), total, off, dx.mptr(), *w, 0, rows, *w, 1, st()) });
            }
            off += w;
        }
    });
    g.push(out, parts, Some(back))
}

pub fn slice_cols(g: &mut Graph, x: Id, start: usize, len: usize) -> Id {
    let xv = g.vals[x].clone();
    let (rows, cols) = (xv.rows, xv.cols);
    let y = DevBuf::new(rows * len);
    ck("slice", unsafe { mkt_copy_cols(dp(&xv), cols, start, y.mptr(), len, 0, rows, len, 0, st()) });
    let out = dev_out(rows, len, y, &xv);
    let back: Back = Box::new(move |d, gr| {
        if let Some(dx) = gr.acc(x) {
            ck("slice_bwd", unsafe { mkt_copy_cols(d.dev().ptr(), len, 0, dx.mptr(), cols, start, rows, len, 1, st()) });
        }
    });
    g.push(out, &[x], Some(back))
}

pub fn gather_rows(g: &mut Graph, x: Id, idx: &[usize], seg: usize, lens: Option<Vec<u32>>) -> Id {
    let xv = g.vals[x].clone();
    let c = xv.cols;
    let rows = idx.len();
    let di = Arc::new(DevU32::from_host(&idx.iter().map(|i| *i as u32).collect::<Vec<_>>()));
    let y = DevBuf::new(rows * c);
    ck("gather", unsafe { mkt_gather_rows(dp(&xv), di.ptr(), y.mptr(), rows, c, st()) });
    let mut out = Tensor { rows, cols: c, data: Vec::new(), dev: Some(y), seg, lens: lens.map(Arc::new) };
    out.seg = seg.max(1);
    let back: Back = Box::new(move |d, gr| {
        if let Some(dx) = gr.acc(x) {
            ck("scatter", unsafe { mkt_scatter_rows_acc(d.dev().ptr(), di.ptr(), dx.mptr(), rows, c, st()) });
        }
    });
    g.push(out, &[x], Some(back))
}

pub fn linear(g: &mut Graph, x: Id, w: Id, b: Option<Id>) -> Id {
    let (xv, wv) = (g.vals[x].clone(), g.vals[w].clone());
    let (t, k, n) = (xv.rows, xv.cols, wv.rows);
    assert_eq!(wv.cols, k, "linear: input width {k} vs weight {}x{}", wv.rows, wv.cols);
    let y = DevBuf::new(t * n);
    gemm(false, true, t, n, k, 1.0, dp(&xv), k, 0, dp(&wv), k, 0, 0.0, y.mptr(), n, 0, 1);
    let out = dev_out(t, n, y, &xv);
    let back: Back = Box::new(move |d, gr| {
        let d = d.dev();
        if let Some(dx) = gr.acc(x) {
            gemm(false, false, t, k, n, 1.0, d.ptr(), n, 0, dp(&wv), k, 0, 1.0, dx.mptr(), k, 0, 1);
        }
        if let Some(dw) = gr.acc(w) {
            gemm(true, false, n, k, t, 1.0, d.ptr(), n, 0, dp(&xv), k, 0, 1.0, dw.mptr(), k, 0, 1);
        }
    });
    let o = g.push(out, &[x, w], Some(back));
    match b {
        Some(b) => add_row(g, o, b),
        None => o,
    }
}

pub fn im2col(g: &mut Graph, x: Id, k: usize, dil: usize) -> Id {
    let xv = g.vals[x].clone();
    let (rows, c, seg) = (xv.rows, xv.cols, xv.seg);
    let y = DevBuf::new(rows * k * c);
    ck("im2col", unsafe { mkt_im2col(dp(&xv), y.mptr(), rows, c, k as i32, dil as i32, seg, st()) });
    let out = dev_out(rows, k * c, y, &xv);
    let back: Back = Box::new(move |d, gr| {
        if let Some(dx) = gr.acc(x) {
            ck("col2im", unsafe { mkt_col2im_acc(d.dev().ptr(), dx.mptr(), rows, c, k as i32, dil as i32, seg, st()) });
        }
    });
    g.push(out, &[x], Some(back))
}

pub fn dwconv(g: &mut Graph, x: Id, w: Id, k: usize) -> Id {
    let (xv, wv) = (g.vals[x].clone(), g.vals[w].clone());
    let (rows, c, seg) = (xv.rows, xv.cols, xv.seg);
    let y = DevBuf::new(rows * c);
    ck("dwconv", unsafe { mkt_dwconv(dp(&xv), dp(&wv), y.mptr(), rows, c, k as i32, seg, st()) });
    let out = dev_out(rows, c, y, &xv);
    let back: Back = Box::new(move |d, gr| {
        if let Some(dx) = gr.acc(x) {
            ck("dwconv_bwd_x", unsafe { mkt_dwconv_bwd_x(d.dev().ptr(), dp(&wv), dx.mptr(), rows, c, k as i32, seg, st()) });
        }
        if let Some(dw) = gr.acc(w) {
            ck("dwconv_bwd_w", unsafe { mkt_dwconv_bwd_w(d.dev().ptr(), dp(&xv), dw.mptr(), rows, c, k as i32, seg, st()) });
        }
    });
    g.push(out, &[x, w], Some(back))
}

pub fn normalize(g: &mut Graph, x: Id) -> Id {
    let xv = g.vals[x].clone();
    let (rows, cols) = (xv.rows, xv.cols);
    let y = DevBuf::new(rows * cols);
    let rstd = Arc::new(DevBuf::new(rows));
    ck("layer_norm", unsafe { mkt_layer_norm(dp(&xv), y.mptr(), rstd.mptr(), rows, cols, st()) });
    let yp = y.ptr();
    let out = dev_out(rows, cols, y, &xv);
    // The output (xhat) lives in the graph for as long as the backward can run.
    let back: Back = Box::new(move |d, gr| {
        if let Some(dx) = gr.acc(x) {
            ck("layer_norm_bwd", unsafe { mkt_layer_norm_bwd(d.dev().ptr(), yp, rstd.ptr(), dx.mptr(), rows, cols, st()) });
        }
    });
    g.push(out, &[x], Some(back))
}

pub fn rope(g: &mut Graph, x: Id, heads: usize) -> Id {
    let xv = g.vals[x].clone();
    let (rows, c, seg) = (xv.rows, xv.cols, xv.seg);
    let y = DevBuf::new(rows * c);
    ck("rope", unsafe { mkt_rope(dp(&xv), y.mptr(), rows, c, heads as i32, seg, 1.0, 0, st()) });
    let out = dev_out(rows, c, y, &xv);
    let back: Back = Box::new(move |d, gr| {
        if let Some(dx) = gr.acc(x) {
            ck("rope_bwd", unsafe { mkt_rope(d.dev().ptr(), dx.mptr(), rows, c, heads as i32, seg, -1.0, 1, st()) });
        }
    });
    g.push(out, &[x], Some(back))
}

pub fn attention(g: &mut Graph, q: Id, k: Id, v: Id, heads: usize) -> Id {
    let (qv, kv, vv) = (g.vals[q].clone(), g.vals[k].clone(), g.vals[v].clone());
    let (c, t, s) = (qv.cols, qv.seg, kv.seg);
    let b = qv.items();
    assert_eq!(b, kv.items(), "attention items");
    let h = heads;
    let dh = c / h;
    let scale = 1.0 / (dh as f32).sqrt();
    let klen = kv.lens.as_ref().map(|l| Arc::new(DevU32::from_host(l)));
    let split = move |src: *const f32, rows: usize| {
        let o = DevBuf::new(b * rows * c);
        ck("split_heads", unsafe { mkt_split_heads(src, o.mptr(), b, rows, h as i32, dh as i32, st()) });
        o
    };
    let qh = Arc::new(split(dp(&qv), t));
    let kh = Arc::new(split(dp(&kv), s));
    let vh = Arc::new(split(dp(&vv), s));
    let bh = b * h;
    let p = Arc::new(DevBuf::new(bh * t * s));
    gemm(false, true, t, s, dh, 1.0, qh.ptr(), dh, (t * dh) as i64, kh.ptr(), dh, (s * dh) as i64, 0.0, p.mptr(), s, (t * s) as i64, bh);
    ck("softmax", unsafe { mkt_softmax_rows(p.mptr(), bh * t, t, s, h as i32, klen.as_ref().map(|l| l.ptr()).unwrap_or(std::ptr::null()), scale, st()) });
    let o = DevBuf::new(bh * t * dh);
    gemm(false, false, t, dh, s, 1.0, p.ptr(), s, (t * s) as i64, vh.ptr(), dh, (s * dh) as i64, 0.0, o.mptr(), dh, (t * dh) as i64, bh);
    let y = DevBuf::new(b * t * c);
    ck("merge_heads", unsafe { mkt_merge_heads(o.ptr(), y.mptr(), b, t, h as i32, dh as i32, 0, st()) });
    let out = dev_out(b * t, c, y, &qv);
    let record = g.record;
    let back: Back = Box::new(move |d, gr| {
        let _ = record;
        let d_o = split(d.dev().ptr(), t);
        // dV = P^T dO ; dP = dO V^T ; dS = softmax'(dP) ; dQ = dS K ; dK = dS^T Q
        let dvh = DevBuf::new(bh * s * dh);
        gemm(true, false, s, dh, t, 1.0, p.ptr(), s, (t * s) as i64, d_o.ptr(), dh, (t * dh) as i64, 0.0, dvh.mptr(), dh, (s * dh) as i64, bh);
        let dpb = DevBuf::new(bh * t * s);
        gemm(false, true, t, s, dh, 1.0, d_o.ptr(), dh, (t * dh) as i64, vh.ptr(), dh, (s * dh) as i64, 0.0, dpb.mptr(), s, (t * s) as i64, bh);
        ck("softmax_bwd", unsafe { mkt_softmax_bwd_rows(p.ptr(), dpb.mptr(), bh * t, s, scale, st()) });
        if let Some(dq) = gr.acc(q) {
            let dqh = DevBuf::new(bh * t * dh);
            gemm(false, false, t, dh, s, 1.0, dpb.ptr(), s, (t * s) as i64, kh.ptr(), dh, (s * dh) as i64, 0.0, dqh.mptr(), dh, (t * dh) as i64, bh);
            ck("merge dq", unsafe { mkt_merge_heads(dqh.ptr(), dq.mptr(), b, t, h as i32, dh as i32, 1, st()) });
        }
        if let Some(dk) = gr.acc(k) {
            let dkh = DevBuf::new(bh * s * dh);
            gemm(true, false, s, dh, t, 1.0, dpb.ptr(), s, (t * s) as i64, qh.ptr(), dh, (t * dh) as i64, 0.0, dkh.mptr(), dh, (s * dh) as i64, bh);
            ck("merge dk", unsafe { mkt_merge_heads(dkh.ptr(), dk.mptr(), b, s, h as i32, dh as i32, 1, st()) });
        }
        if let Some(dv) = gr.acc(v) {
            ck("merge dv", unsafe { mkt_merge_heads(dvh.ptr(), dv.mptr(), b, s, h as i32, dh as i32, 1, st()) });
        }
    });
    g.push(out, &[q, k, v], Some(back))
}

pub fn masked_loss(g: &mut Graph, x: Id, target: Id, mask: Option<&[f32]>, col_lim: Option<&[u32]>, sq: bool) -> Id {
    let (xv, tv) = (g.vals[x].clone(), g.vals[target].clone());
    assert_eq!(xv.len(), tv.len(), "loss shapes");
    let (rows, cols, seg) = (xv.rows, xv.cols, xv.seg);
    let denom = Graph::loss_denom(rows, cols, seg, mask, col_lim);
    let m = mask.map(DevBuf::from_host);
    let l = col_lim.map(DevU32::from_host);
    let n = xv.len();
    let coef = Arc::new(DevBuf::new(n));
    let out = DevBuf::zeros(1);
    ck("loss", unsafe {
        mkt_loss(
            dp(&xv), dp(&tv), m.as_ref().map(|m| m.ptr()).unwrap_or(std::ptr::null()), l.as_ref().map(|l| l.ptr()).unwrap_or(std::ptr::null()),
            seg, coef.mptr(), out.mptr(), rows, cols, 1.0 / denom, sq as i32, st(),
        )
    });
    let t = Tensor { rows: 1, cols: 1, data: Vec::new(), dev: Some(out), seg: 1, lens: None };
    let back: Back = Box::new(move |d, gr| {
        if let Some(dx) = gr.acc(x) {
            ck("loss_bwd", unsafe { mkt_scaled_acc(coef.ptr(), d.dev().ptr(), dx.mptr(), n, st()) });
        }
    });
    g.push(t, &[x], Some(back))
}

pub fn bce(g: &mut Graph, x: Id, target: Id) -> Id {
    let (xv, tv) = (g.vals[x].clone(), g.vals[target].clone());
    let n = xv.len();
    let coef = Arc::new(DevBuf::new(n));
    let out = DevBuf::zeros(1);
    ck("bce", unsafe { mkt_bce(dp(&xv), dp(&tv), coef.mptr(), out.mptr(), n, st()) });
    let t = Tensor { rows: 1, cols: 1, data: Vec::new(), dev: Some(out), seg: 1, lens: None };
    let back: Back = Box::new(move |d, gr| {
        if let Some(dx) = gr.acc(x) {
            ck("bce_bwd", unsafe { mkt_scaled_acc(coef.ptr(), d.dev().ptr(), dx.mptr(), n, st()) });
        }
    });
    g.push(t, &[x], Some(back))
}

pub fn sum_scalars(g: &mut Graph, terms: &[(Id, f32)]) -> Id {
    let out = DevBuf::zeros(1);
    for (id, w) in terms {
        ck("sum_scalars", unsafe { mkt_axpy_scalar(dp(&g.vals[*id]), out.mptr(), *w, st()) });
    }
    let t = Tensor { rows: 1, cols: 1, data: Vec::new(), dev: Some(out), seg: 1, lens: None };
    let terms_v = terms.to_vec();
    let ids: Vec<Id> = terms.iter().map(|t| t.0).collect();
    let back: Back = Box::new(move |d, gr| {
        for (id, w) in &terms_v {
            if let Some(dx) = gr.acc(*id) {
                ck("sum_scalars_bwd", unsafe { mkt_axpy_scalar(d.dev().ptr(), dx.mptr(), *w, st()) });
            }
        }
    });
    g.push(t, &ids, Some(back))
}

pub fn hn_filter(g: &mut Graph, learn: [Id; 4], src: [Id; 4]) -> Id {
    let lv = learn.map(|i| g.vals[i].clone());
    let sv = src.map(|i| g.vals[i].clone());
    let (rows, f) = (lv[0].rows, lv[0].cols);
    let y = DevBuf::new(rows * 2 * f);
    ck("hn_filter", unsafe {
        mkt_hn_filter(dp(&lv[0]), dp(&lv[1]), dp(&lv[2]), dp(&lv[3]), dp(&sv[0]), dp(&sv[1]), dp(&sv[2]), dp(&sv[3]), y.mptr(), rows, f, st())
    });
    let out = dev_out(rows, 2 * f, y, &lv[0]);
    let back: Back = Box::new(move |d, gr| {
        let ptrs = learn.map(|i| gr.accp(i));
        ck("hn_filter_bwd", unsafe {
            mkt_hn_filter_bwd(
                d.dev().ptr(), dp(&lv[0]), dp(&lv[1]), dp(&lv[2]), dp(&lv[3]), dp(&sv[0]), dp(&sv[1]), dp(&sv[2]), dp(&sv[3]),
                ptrs[0], ptrs[1], ptrs[2], ptrs[3], rows, f, st(),
            )
        });
    });
    g.push(out, &learn, Some(back))
}

pub fn istft(g: &mut Graph, y: Id, s: Stft, len: usize) -> Id {
    let yv = g.vals[y].clone();
    let (t, f2, b) = (yv.seg, yv.cols, yv.items());
    let f = f2 / 2;
    let n = s.n_fft;
    let rows = b * t;
    let win = win_buf(s);
    let norm = norm_buf(s, t, len);
    let (re, im) = (DevBuf::new(rows * f), DevBuf::new(rows * f));
    ck("split_ri", unsafe { mkt_split_ri(dp(&yv), re.mptr(), im.mptr(), rows, f, st()) });
    let (fr, fi) = (DevBuf::new(rows * n), DevBuf::new(rows * n));
    ck("hermitian", unsafe { mkt_hermitian(re.ptr(), im.ptr(), fr.mptr(), fi.mptr(), rows, n as i32, 1.0, 1.0, st()) });
    ck("ifft", unsafe { mkt_fft(fr.mptr(), fi.mptr(), rows, n as i32, 1, st()) });
    let w = DevBuf::new(b * len);
    ck("ola", unsafe { mkt_overlap_add(fr.ptr(), win.ptr(), norm.ptr(), w.mptr(), b, len, t, n as i32, s.hop as i32, 1.0 / n as f32, 0, st()) });
    let out = Tensor { rows: b * len, cols: 1, data: Vec::new(), dev: Some(w), seg: len, lens: None };
    let back: Back = Box::new(move |d, gr| {
        if let Some(dy) = gr.acc(y) {
            let (fr, fi) = (DevBuf::new(rows * n), DevBuf::new(rows * n));
            ck("frame", unsafe { mkt_frame(d.dev().ptr(), norm.ptr(), win.ptr(), fr.mptr(), fi.mptr(), b, len, t, n as i32, s.hop as i32, st()) });
            ck("fft", unsafe { mkt_fft(fr.mptr(), fi.mptr(), rows, n as i32, 0, st()) });
            let (dre, dim) = (DevBuf::new(rows * f), DevBuf::new(rows * f));
            ck("half_c", unsafe { mkt_half_c(fr.ptr(), fi.ptr(), dre.mptr(), dim.mptr(), rows, n as i32, 1.0 / n as f32, 2.0 / n as f32, st()) });
            ck("join_ri", unsafe { mkt_join_ri_acc(dre.ptr(), dim.ptr(), dy.mptr(), rows, f, st()) });
        }
    });
    g.push(out, &[y], Some(back))
}

pub fn stft_mag(g: &mut Graph, x: Id, s: Stft) -> Id {
    let xv = g.vals[x].clone();
    let (len, b) = (xv.seg, xv.items());
    let n = s.n_fft;
    let frames = s.frames(len);
    let bins = s.bins();
    let rows = b * frames;
    let win = win_buf(s);
    let (fr, fi) = (DevBuf::new(rows * n), DevBuf::new(rows * n));
    ck("frame", unsafe { mkt_frame(dp(&xv), std::ptr::null(), win.ptr(), fr.mptr(), fi.mptr(), b, len, frames, n as i32, s.hop as i32, st()) });
    ck("fft", unsafe { mkt_fft(fr.mptr(), fi.mptr(), rows, n as i32, 0, st()) });
    let re = Arc::new(DevBuf::new(rows * bins));
    let im = Arc::new(DevBuf::new(rows * bins));
    ck("half", unsafe { mkt_half(fr.ptr(), fi.ptr(), re.mptr(), im.mptr(), rows, n as i32, 1.0, st()) });
    let m = DevBuf::new(rows * bins);
    ck("mag", unsafe { mkt_mag(re.ptr(), im.ptr(), m.mptr(), rows * bins, st()) });
    let mp = m.ptr();
    let out = Tensor { rows, cols: bins, data: Vec::new(), dev: Some(m), seg: frames, lens: None };
    let back: Back = Box::new(move |d, gr| {
        if let Some(dx) = gr.acc(x) {
            let (dre, dim) = (DevBuf::new(rows * bins), DevBuf::new(rows * bins));
            ck("mag_bwd", unsafe { mkt_mag_bwd(d.dev().ptr(), re.ptr(), im.ptr(), mp, dre.mptr(), dim.mptr(), rows * bins, st()) });
            let (fr, fi) = (DevBuf::new(rows * n), DevBuf::new(rows * n));
            ck("hermitian", unsafe { mkt_hermitian(dre.ptr(), dim.ptr(), fr.mptr(), fi.mptr(), rows, n as i32, n as f32, n as f32 / 2.0, st()) });
            ck("ifft", unsafe { mkt_fft(fr.mptr(), fi.mptr(), rows, n as i32, 1, st()) });
            ck("ola", unsafe { mkt_overlap_add(fr.ptr(), win.ptr(), std::ptr::null(), dx.mptr(), b, len, frames, n as i32, s.hop as i32, 1.0 / n as f32, 1, st()) });
        }
    });
    g.push(out, &[x], Some(back))
}

pub fn stft_complex(waves: &Tensor, s: Stft, frames: usize) -> (Tensor, Tensor) {
    let (len, b) = (waves.seg, waves.items());
    let (n, bins) = (s.n_fft, s.bins());
    let rows = b * frames;
    let x = waves.dev.as_ref().expect("device waves");
    let win = win_buf(s);
    let (fr, fi) = (DevBuf::new(rows * n), DevBuf::new(rows * n));
    ck("frame", unsafe { mkt_frame(x.ptr(), std::ptr::null(), win.ptr(), fr.mptr(), fi.mptr(), b, len, frames, n as i32, s.hop as i32, st()) });
    ck("fft", unsafe { mkt_fft(fr.mptr(), fi.mptr(), rows, n as i32, 0, st()) });
    let (re, im) = (DevBuf::new(rows * bins), DevBuf::new(rows * bins));
    ck("half", unsafe { mkt_half(fr.ptr(), fi.ptr(), re.mptr(), im.mptr(), rows, n as i32, 1.0, st()) });
    let mk = |d: DevBuf| Tensor { rows, cols: bins, data: Vec::new(), dev: Some(d), seg: frames, lens: None };
    (mk(re), mk(im))
}

pub fn log_eps(g: &mut Graph, x: Id, eps: f32) -> Id {
    let xv = g.vals[x].clone();
    let n = xv.len();
    let y = DevBuf::new(n);
    ck("log_eps", unsafe { mkt_log_eps(dp(&xv), y.mptr(), eps, n, st()) });
    let out = dev_out(xv.rows, xv.cols, y, &xv);
    let back: Back = Box::new(move |d, gr| {
        if let Some(dx) = gr.acc(x) {
            ck("log_eps_bwd", unsafe { mkt_log_eps_bwd(dp(&xv), d.dev().ptr(), dx.mptr(), eps, n, st()) });
        }
    });
    g.push(out, &[x], Some(back))
}

/// A gradient buffer's device side (for the optimiser).
pub fn grad_dev(g: &GBuf) -> &DevBuf {
    g.dev()
}

pub fn gather_rows_dev(g: &mut Graph, x: Id, di: Arc<DevU32>, rows: usize, seg: usize, lens: Option<Vec<u32>>) -> Id {
    let xv = g.vals[x].clone();
    let c = xv.cols;
    let y = DevBuf::new(rows * c);
    ck("gather", unsafe { mkt_gather_rows(dp(&xv), di.ptr(), y.mptr(), rows, c, st()) });
    let out = Tensor { rows, cols: c, data: Vec::new(), dev: Some(y), seg: seg.max(1), lens: lens.map(Arc::new) };
    let back: Back = Box::new(move |d, gr| {
        if let Some(dx) = gr.acc(x) {
            ck("scatter", unsafe { mkt_scatter_rows_acc(d.dev().ptr(), di.ptr(), dx.mptr(), rows, c, st()) });
        }
    });
    g.push(out, &[x], Some(back))
}

pub fn mas_align(g: &mut Graph, mu: Id, mel: Id, tok_lens: &[u32], frame_lens: &[u32]) -> (crate::nn::RowIndex, Id) {
    let (mv, ev) = (g.vals[mu].clone(), g.vals[mel].clone());
    let (n, t, d, b) = (mv.seg, ev.seg, mv.cols, mv.items());
    let lp = DevBuf::new(b * n * t);
    // Exact f32 here: the alignment is a discrete decision and must not
    // depend on the GEMM precision.
    let prec = tf32();
    set_tf32(false);
    gemm(false, true, n, t, d, 1.0, dp(&mv), d, (n * d) as i64, dp(&ev), d, (t * d) as i64, 0.0, lp.mptr(), t, (n * t) as i64, b);
    set_tf32(prec);
    ck("mas_logp", unsafe { mkt_mas_logp(lp.mptr(), dp(&mv), dp(&ev), b, n as i32, t as i32, d as i32, st()) });
    let nt = DevU32::from_host(tok_lens);
    let nf = DevU32::from_host(frame_lens);
    let dur = DevU32::zeros(b * n);
    ck("mas", unsafe { mkt_mas(lp.mptr(), nt.ptr(), nf.ptr(), dur.mptr(), b, n as i32, t as i32, st()) });
    let idx = Arc::new(DevU32::zeros(b * t));
    let ld = DevBuf::new(b * n);
    ck("dur_to_idx", unsafe { mkt_dur_to_idx(dur.ptr(), idx.mptr(), ld.mptr(), b, n as i32, t as i32, st()) });
    let ldt = Tensor { rows: b * n, cols: 1, data: Vec::new(), dev: Some(ld), seg: n, lens: None };
    let id = g.input(ldt);
    (crate::nn::RowIndex::Dev(idx, b * t), id)
}

pub fn source_waves(src: &crate::vocoder::SourceCtl) -> (Tensor, Tensor) {
    let n = src.items * src.len;
    let (ph, hz, amp) = (DevBuf::from_host(&src.ph), DevBuf::from_host(&src.hz), DevBuf::from_host(&src.amp));
    let h = DevBuf::new(n);
    ck("harmonic", unsafe { mkt_harmonic(ph.ptr(), hz.ptr(), amp.ptr(), h.mptr(), n, crate::dsp::HARMONIC_TOP_HZ, crate::dsp::SOURCE_LEVEL, st()) });
    let z = DevBuf::new(n);
    ck("randn", unsafe { mkt_randn(z.mptr(), n, src.seed, st()) });
    ck("scale", unsafe { mkt_scale(z.ptr(), z.mptr(), crate::dsp::SOURCE_LEVEL, n, st()) });
    let mk = |d: DevBuf| Tensor { rows: n, cols: 1, data: Vec::new(), dev: Some(d), seg: src.len, lens: None };
    (mk(h), mk(z))
}

pub fn randn(rows: usize, cols: usize, seg: usize, lens: Option<Vec<u32>>, seed: u64) -> Tensor {
    let n = rows * cols;
    let z = DevBuf::new(n);
    ck("randn", unsafe { mkt_randn(z.mptr(), n, seed, st()) });
    Tensor { rows, cols, data: Vec::new(), dev: Some(z), seg: seg.max(1), lens: lens.map(Arc::new) }
}
