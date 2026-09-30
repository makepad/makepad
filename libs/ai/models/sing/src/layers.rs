//! Building blocks shared by the acoustic model and the vocoder. Each block
//! has a `declare_*` (parameter names, shapes, init) and an apply function
//! over a [`Graph`]; parameter names are `<prefix>.<part>`.

use crate::dsp::Rng;
use crate::nn::{Act, Graph, Id, Init, Params};

pub fn declare_linear(p: &mut Params, rng: &mut Rng, name: &str, d_in: usize, d_out: usize, gain: f32) {
    p.declare(&format!("{name}.w"), d_out, d_in, Init::Fan(gain), rng);
    p.declare(&format!("{name}.b"), 1, d_out, Init::Zeros, rng);
}

pub fn linear(g: &mut Graph, name: &str, x: Id) -> Id {
    let w = g.p(&format!("{name}.w"));
    let b = g.p(&format!("{name}.b"));
    g.linear(x, w, Some(b))
}

pub fn declare_norm(p: &mut Params, rng: &mut Rng, name: &str, d: usize) {
    p.declare(&format!("{name}.g"), 1, d, Init::Ones, rng);
    p.declare(&format!("{name}.b"), 1, d, Init::Zeros, rng);
}

pub fn norm(g: &mut Graph, name: &str, x: Id) -> Id {
    let gn = g.p(&format!("{name}.g"));
    let b = g.p(&format!("{name}.b"));
    g.layer_norm(x, gn, b)
}

pub fn declare_conv(p: &mut Params, rng: &mut Rng, name: &str, d_in: usize, d_out: usize, k: usize) {
    p.declare(&format!("{name}.w"), d_out, d_in * k, Init::Fan(1.0), rng);
    p.declare(&format!("{name}.b"), 1, d_out, Init::Zeros, rng);
}

pub fn conv(g: &mut Graph, name: &str, x: Id, k: usize) -> Id {
    let w = g.p(&format!("{name}.w"));
    let b = g.p(&format!("{name}.b"));
    g.conv1d(x, w, Some(b), k, 1)
}

pub fn declare_dw(p: &mut Params, rng: &mut Rng, name: &str, d: usize, k: usize) {
    p.declare(&format!("{name}.w"), k, d, Init::Normal((1.0 / k as f32).sqrt()), rng);
    p.declare(&format!("{name}.b"), 1, d, Init::Zeros, rng);
}

pub fn dw(g: &mut Graph, name: &str, x: Id, k: usize) -> Id {
    let w = g.p(&format!("{name}.w"));
    let b = g.p(&format!("{name}.b"));
    g.dwconv1d(x, w, b, k)
}

// --- multi-head self-attention with rotary positions ------------------------

pub fn declare_attn(p: &mut Params, rng: &mut Rng, name: &str, d: usize) {
    for part in ["q", "k", "v"] {
        declare_linear(p, rng, &format!("{name}.{part}"), d, d, 1.0);
    }
    declare_linear(p, rng, &format!("{name}.o"), d, d, 0.5);
}

pub fn attn(g: &mut Graph, name: &str, x: Id, heads: usize) -> Id {
    let q = linear(g, &format!("{name}.q"), x);
    let k = linear(g, &format!("{name}.k"), x);
    let v = linear(g, &format!("{name}.v"), x);
    let q = g.rope(q, heads);
    let k = g.rope(k, heads);
    let a = g.attention(q, k, v, heads);
    linear(g, &format!("{name}.o"), a)
}

// --- feed-forward --------------------------------------------------------------

pub fn declare_ffn(p: &mut Params, rng: &mut Rng, name: &str, d: usize, hidden: usize) {
    declare_linear(p, rng, &format!("{name}.1"), d, hidden, 1.0);
    declare_linear(p, rng, &format!("{name}.2"), hidden, d, 0.5);
}

pub fn ffn(g: &mut Graph, name: &str, x: Id) -> Id {
    let h = linear(g, &format!("{name}.1"), x);
    let h = g.act(h, Act::Gelu);
    linear(g, &format!("{name}.2"), h)
}

// --- transformer block (pre-norm) --------------------------------------------------

pub fn declare_transformer(p: &mut Params, rng: &mut Rng, name: &str, d: usize, hidden: usize) {
    declare_norm(p, rng, &format!("{name}.n1"), d);
    declare_attn(p, rng, &format!("{name}.attn"), d);
    declare_norm(p, rng, &format!("{name}.n2"), d);
    declare_ffn(p, rng, &format!("{name}.ffn"), d, hidden);
}

pub fn transformer(g: &mut Graph, name: &str, x: Id, heads: usize) -> Id {
    let h = norm(g, &format!("{name}.n1"), x);
    let h = attn(g, &format!("{name}.attn"), h, heads);
    let x = g.add(x, h);
    let h = norm(g, &format!("{name}.n2"), x);
    let h = ffn(g, &format!("{name}.ffn"), h);
    g.add(x, h)
}

// --- conformer block: attention, convolution module, feed-forward ----------------

pub fn declare_conformer(p: &mut Params, rng: &mut Rng, name: &str, d: usize, hidden: usize, k: usize) {
    declare_norm(p, rng, &format!("{name}.n1"), d);
    declare_attn(p, rng, &format!("{name}.attn"), d);
    declare_norm(p, rng, &format!("{name}.n2"), d);
    declare_linear(p, rng, &format!("{name}.pw1"), d, 2 * d, 1.0);
    declare_dw(p, rng, &format!("{name}.dw"), d, k);
    declare_norm(p, rng, &format!("{name}.n3"), d);
    declare_linear(p, rng, &format!("{name}.pw2"), d, d, 0.5);
    declare_norm(p, rng, &format!("{name}.n4"), d);
    declare_ffn(p, rng, &format!("{name}.ffn"), d, hidden);
    declare_norm(p, rng, &format!("{name}.out"), d);
}

pub fn conformer(g: &mut Graph, name: &str, x: Id, heads: usize, k: usize) -> Id {
    let h = norm(g, &format!("{name}.n1"), x);
    let h = attn(g, &format!("{name}.attn"), h, heads);
    let x = g.add(x, h);
    let h = norm(g, &format!("{name}.n2"), x);
    let h = linear(g, &format!("{name}.pw1"), h);
    let h = g.glu(h);
    let h = dw(g, &format!("{name}.dw"), h, k);
    let h = norm(g, &format!("{name}.n3"), h);
    let h = g.act(h, Act::Silu);
    let h = linear(g, &format!("{name}.pw2"), h);
    let x = g.add(x, h);
    let h = norm(g, &format!("{name}.n4"), x);
    let h = ffn(g, &format!("{name}.ffn"), h);
    let x = g.add(x, h);
    norm(g, &format!("{name}.out"), x)
}

// --- ConvNeXt-1D block -----------------------------------------------------------

pub fn declare_convnext(p: &mut Params, rng: &mut Rng, name: &str, d: usize, hidden: usize, k: usize) {
    declare_dw(p, rng, &format!("{name}.dw"), d, k);
    declare_norm(p, rng, &format!("{name}.n"), d);
    declare_ffn(p, rng, &format!("{name}.ffn"), d, hidden);
    p.declare(&format!("{name}.gamma"), 1, d, Init::Const(0.1), rng);
}

pub fn convnext(g: &mut Graph, name: &str, x: Id, k: usize) -> Id {
    let h = dw(g, &format!("{name}.dw"), x, k);
    let h = norm(g, &format!("{name}.n"), h);
    let h = ffn(g, &format!("{name}.ffn"), h);
    let gm = g.p(&format!("{name}.gamma"));
    let h = g.mul_row(h, gm);
    g.add(x, h)
}
