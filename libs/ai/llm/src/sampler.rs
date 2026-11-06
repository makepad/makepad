//! The sampler's arithmetic, defined once.
//!
//! A sampled token comes out of three stages.
//!
//! WEIGHING turns one logit row into unnormalised weights: the penalties come
//! off the logits, the penalised maximum `m` is found, every token gets
//! `exp_poly((l - m) / T)`, and the weights are summed in a fixed tree. It
//! hands on the sum and the heaviest [`WEIGH_CANDIDATES`] tokens, and every
//! weight when the nucleus turns out wider than those. The CUDA executor weighs
//! on the device (`mkllm_sample_weigh` in the LLM kernels) with the same
//! operations in the same order: the exponential is the polynomial below, op
//! for op with explicit fused multiply-adds, and the sum is the same tree. Both
//! give the same bits, so every other backend weighs here and the two meet in
//! [`RowWeights`].
//!
//! FINISHING applies top-k and top-p to the heaviest tokens and renormalises
//! what they keep. DRAWING walks the kept tokens in token order. Both run here
//! for every backend.
//!
//! The sampler is defined by this file. A full-vocabulary `expf` summed in
//! token order cannot be matched on a device, whose sum runs in parallel and
//! whose `expf` is not the host's; a polynomial and a fixed tree can.

use crate::{LlamaError, Result};

/// Tokens per summation chunk. The weights of a chunk are summed by halving
/// the stride from 512 down to 1, and the chunk sums are then added in order.
pub(crate) const WEIGH_CHUNK: usize = 1024;

/// How many of the heaviest tokens weighing hands on. Top-p at chat settings
/// keeps far fewer; a row whose nucleus is wider comes with every weight.
pub(crate) const WEIGH_CANDIDATES: usize = 1024;

/// The temperature floor: a temperature this low is greedy in all but name.
const MIN_TEMPERATURE: f32 = 1e-6;

/// `e^x` for `x <= 0`, to within 3 ulp, and exactly 1 at 0.
///
/// `x = k ln2 + r` with `k` the nearest integer to `x log2(e)` (ties to even)
/// and `ln2` split in two so `r` is exact; then a degree-6 Taylor polynomial in
/// `r`, scaled by `2^k`. Below -87 the result would leave the normal range and
/// is 0, and so is NaN. Every step is one IEEE operation with a constant given
/// by its bits; the device kernel spells the same steps out with
/// `__fmul_rn`/`__fmaf_rn`/`rintf`, so the two agree bit for bit.
pub(crate) fn exp_poly(x: f32) -> f32 {
    const LOG2_E: f32 = f32::from_bits(0x3FB8_AA3B);
    const LN2_HI: f32 = f32::from_bits(0x3F31_7200);
    const LN2_LO: f32 = f32::from_bits(0x35BF_BE8E);
    const C6: f32 = f32::from_bits(0x3AB6_0B61); // 1/720
    const C5: f32 = f32::from_bits(0x3C08_8889); // 1/120
    const C4: f32 = f32::from_bits(0x3D2A_AAAB); // 1/24
    const C3: f32 = f32::from_bits(0x3E2A_AAAB); // 1/6
    if !(x >= -87.0) {
        return 0.0;
    }
    let k = (x * LOG2_E).round_ties_even();
    let r = (-k).mul_add(LN2_HI, x);
    let r = (-k).mul_add(LN2_LO, r);
    let mut p = C6;
    p = p.mul_add(r, C5);
    p = p.mul_add(r, C4);
    p = p.mul_add(r, C3);
    p = p.mul_add(r, 0.5);
    p = p.mul_add(r, 1.0);
    p = p.mul_add(r, 1.0);
    let scale = f32::from_bits(((k as i32 + 127) as u32) << 23);
    p * scale
}

/// Everything weighing one row needs, and what decides whether the heaviest
/// tokens are enough.
#[derive(Clone, Debug, PartialEq)]
pub struct WeighSpec {
    /// Already floored at the minimum temperature.
    pub temperature: f32,
    /// 0 keeps every token.
    pub top_k: usize,
    /// Already clamped to `[0, 1]`; 1 keeps every token top-k kept.
    pub top_p: f32,
    /// `(token, penalty)` by token id, each token once. Subtracted from the
    /// logit before anything else; tokens past the vocabulary are ignored.
    pub penalties: Vec<(u32, f32)>,
}

impl WeighSpec {
    pub fn new(temperature: f32, top_k: usize, top_p: f32, penalties: Vec<(u32, f32)>) -> Self {
        Self {
            temperature: temperature.max(MIN_TEMPERATURE),
            top_k,
            top_p: top_p.clamp(0.0, 1.0),
            penalties,
        }
    }
}

/// One weighed row.
#[derive(Clone, Debug, PartialEq)]
pub struct RowWeights {
    pub vocab: usize,
    /// The tree sum of every weight.
    pub sum: f32,
    /// The heaviest `min(vocab, WEIGH_CANDIDATES)` tokens as `(token, weight)`,
    /// heaviest first, ties by token id.
    pub top: Vec<(u32, f32)>,
    /// Every weight by token id, present exactly when [`needs_all`] says the
    /// row's kept set may reach past `top`.
    pub all: Option<Vec<f32>>,
}

/// What a forward hands back for one of its output rows.
#[derive(Clone, Debug)]
pub enum RowReadout {
    /// The raw logits, for a row nothing samples from.
    Logits,
    /// The row weighed for sampling.
    Weights(WeighSpec),
}

/// One output row, as its [`RowReadout`] asked.
#[derive(Clone, Debug)]
pub enum RowOut {
    Logits(Vec<f32>),
    Weights(RowWeights),
}

/// The weights of a logit row, by token id, and their tree sum.
///
/// The penalised maximum must be finite: a row of NaN, of `-inf`, or with a
/// `+inf` has no distribution to give.
pub(crate) fn weights(logits: &[f32], spec: &WeighSpec) -> Result<(Vec<f32>, f32)> {
    if logits.is_empty() {
        return Err(LlamaError::format("cannot sample from empty logits"));
    }
    let mut v = logits.to_vec();
    for &(token, penalty) in &spec.penalties {
        if let Some(value) = v.get_mut(token as usize) {
            *value -= penalty;
        }
    }
    let max = v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if !max.is_finite() {
        return Err(LlamaError::format("logits contain no finite value"));
    }
    let temperature = spec.temperature;
    for value in v.iter_mut() {
        *value = exp_poly((*value - max) / temperature);
    }
    let sum = tree_sum(&v);
    Ok((v, sum))
}

/// Sum by chunks of [`WEIGH_CHUNK`], each a stride-halving tree over its
/// zero-padded chunk, the chunk sums added in order.
pub(crate) fn tree_sum(values: &[f32]) -> f32 {
    let mut sum = 0.0f32;
    let mut chunk = [0.0f32; WEIGH_CHUNK];
    for part in values.chunks(WEIGH_CHUNK) {
        chunk[..part.len()].copy_from_slice(part);
        chunk[part.len()..].fill(0.0);
        let mut stride = WEIGH_CHUNK / 2;
        while stride > 0 {
            for i in 0..stride {
                chunk[i] += chunk[i + stride];
            }
            stride /= 2;
        }
        sum += chunk[0];
    }
    sum
}

/// A token's rank key: heavier first, then lower id. Weights are never
/// negative, so their bits order like their values.
fn rank_key(token: usize, weight: f32) -> u64 {
    (u64::from(weight.to_bits()) << 32) | u64::from(u32::MAX - token as u32)
}

fn from_rank_key(key: u64) -> (u32, f32) {
    (u32::MAX - key as u32, f32::from_bits((key >> 32) as u32))
}

/// The `count` heaviest tokens, heaviest first, ties by token id.
pub(crate) fn heaviest(weights: &[f32], count: usize) -> Vec<(u32, f32)> {
    let count = count.min(weights.len());
    if count == 0 {
        return Vec::new();
    }
    let mut keys: Vec<u64> = weights
        .iter()
        .enumerate()
        .map(|(token, &weight)| rank_key(token, weight))
        .collect();
    if count < keys.len() {
        keys.select_nth_unstable_by(count - 1, |a, b| b.cmp(a));
        keys.truncate(count);
    }
    keys.sort_unstable_by(|a, b| b.cmp(a));
    keys.into_iter().map(from_rank_key).collect()
}

/// How many of the heaviest tokens top-k lets through.
fn kept_by_top_k(vocab: usize, spec: &WeighSpec) -> usize {
    if spec.top_k > 0 {
        spec.top_k.min(vocab)
    } else {
        vocab
    }
}

/// Where top-p cuts `order` (heaviest first), or `None` when the cumulative
/// probability never reaches `top_p` inside it.
fn top_p_cut(order: &[(u32, f32)], sum: f32, top_p: f32) -> Option<usize> {
    let mut cumulative = 0.0f32;
    for (index, &(_, weight)) in order.iter().enumerate() {
        cumulative += weight / sum;
        if cumulative >= top_p {
            return Some(index + 1);
        }
    }
    None
}

/// Whether finishing a row may need tokens past its heaviest `top`: top-k
/// keeps more than `top` holds, and top-p does not cut inside `top` first.
pub(crate) fn needs_all(top: &[(u32, f32)], sum: f32, vocab: usize, spec: &WeighSpec) -> bool {
    if kept_by_top_k(vocab, spec) <= top.len() {
        return false;
    }
    !(spec.top_p < 1.0 && top_p_cut(top, sum, spec.top_p).is_some())
}

/// Weigh a row on the host. The reference every backend's weights equal.
pub(crate) fn weigh_row(logits: &[f32], spec: &WeighSpec) -> Result<RowWeights> {
    let (all, sum) = weights(logits, spec)?;
    let top = heaviest(&all, WEIGH_CANDIDATES);
    let all = needs_all(&top, sum, all.len(), spec).then_some(all);
    Ok(RowWeights {
        vocab: logits.len(),
        sum,
        top,
        all,
    })
}

/// A finished distribution: the kept tokens with their probabilities, by
/// token id, every probability above zero.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Distribution(Vec<(u32, f32)>);

impl Distribution {
    /// All of the mass on one token.
    #[cfg(test)]
    pub fn certain(token: u32) -> Self {
        Self(vec![(token, 1.0)])
    }

    /// A distribution given densely by token id, zeros dropped.
    #[cfg(test)]
    pub fn from_dense(probs: &[f32]) -> Self {
        Self(
            probs
                .iter()
                .enumerate()
                .filter(|&(_, &p)| p > 0.0)
                .map(|(token, &p)| (token as u32, p))
                .collect(),
        )
    }

    /// The distribution densely over `vocab` token ids.
    #[cfg(test)]
    pub fn to_dense(&self, vocab: usize) -> Vec<f32> {
        let mut dense = vec![0.0f32; vocab];
        for &(token, p) in &self.0 {
            dense[token as usize] = p;
        }
        dense
    }

    pub fn entries(&self) -> &[(u32, f32)] {
        &self.0
    }

    /// The probability of `token`, 0 when it was not kept.
    pub fn prob(&self, token: u32) -> f32 {
        self.0
            .binary_search_by_key(&token, |&(id, _)| id)
            .map(|index| self.0[index].1)
            .unwrap_or(0.0)
    }

    /// Inverse-CDF draw at `u` in `[0, 1)`, walking the tokens by id.
    /// Rounding can leave the cumulative just short of `u`; the last token
    /// takes it.
    pub fn draw(&self, u: f32) -> Result<u32> {
        let mut cumulative = 0.0f32;
        for &(token, probability) in &self.0 {
            cumulative += probability;
            if cumulative >= u {
                return Ok(token);
            }
        }
        self.0
            .last()
            .map(|&(token, _)| token)
            .ok_or_else(|| LlamaError::format("probability vector is all zeros"))
    }

    /// The normalised residual `max(0, p - q)` a rejected draft is replaced
    /// from, `q` given sparsely over the same token ids. With
    /// [`speculative_acceptance`](crate::session) this makes the emitted
    /// token exactly `p`-distributed. When `q == p` everywhere the residual is
    /// empty, which only an accepted draft can produce, and then it is `p`.
    pub fn residual(&self, draft: &[(u32, f32)]) -> Self {
        let mut q: Vec<(u32, f32)> = draft.to_vec();
        q.sort_unstable_by_key(|&(token, _)| token);
        let q_of = |token: u32| {
            q.binary_search_by_key(&token, |&(id, _)| id)
                .map(|index| q[index].1)
                .unwrap_or(0.0)
        };
        let residual: Vec<(u32, f32)> = self
            .0
            .iter()
            .map(|&(token, p)| (token, (p - q_of(token)).max(0.0)))
            .collect();
        let mut total = 0.0f32;
        for &(_, r) in &residual {
            total += r;
        }
        if !(total > 0.0) {
            return self.clone();
        }
        Self(
            residual
                .into_iter()
                .map(|(token, r)| (token, r / total))
                .filter(|&(_, r)| r > 0.0)
                .collect(),
        )
    }
}

/// Apply top-k and top-p to a weighed row and renormalise what they keep.
///
/// Top-k keeps the heaviest `top_k` (all when 0); top-p then keeps the
/// shortest heaviest-first prefix whose probability reaches `top_p`. The kept
/// probabilities `w / sum` are summed in token order and divided by that sum.
pub(crate) fn finish(row: &RowWeights, spec: &WeighSpec) -> Result<Distribution> {
    if !(row.sum > 0.0) {
        return Err(LlamaError::format("logit softmax underflowed to zero"));
    }
    let keep = kept_by_top_k(row.vocab, spec);
    let wide;
    let order: &[(u32, f32)] = match &row.all {
        None => {
            if needs_all(&row.top, row.sum, row.vocab, spec) {
                return Err(LlamaError::format(
                    "a row whose nucleus is wider than its candidates came without its weights",
                ));
            }
            &row.top
        }
        Some(all) if spec.top_p >= 1.0 && keep == row.vocab => {
            // Every token is kept: no order needed.
            wide = all
                .iter()
                .enumerate()
                .map(|(token, &weight)| (token as u32, weight))
                .collect::<Vec<_>>();
            &wide
        }
        Some(all) => {
            wide = heaviest(all, keep);
            &wide
        }
    };
    let limit = keep.min(order.len());
    let cut = if spec.top_p < 1.0 {
        top_p_cut(&order[..limit], row.sum, spec.top_p).unwrap_or(limit)
    } else {
        limit
    };
    let mut kept: Vec<(u32, f32)> = order[..cut]
        .iter()
        .map(|&(token, weight)| (token, weight / row.sum))
        .collect();
    kept.sort_unstable_by_key(|&(token, _)| token);
    let mut total = 0.0f32;
    for &(_, p) in &kept {
        total += p;
    }
    if !(total > 0.0) {
        return Err(LlamaError::format("sampling kept no probability mass"));
    }
    Ok(Distribution(
        kept.into_iter()
            .map(|(token, p)| (token, p / total))
            .filter(|&(_, p)| p > 0.0)
            .collect(),
    ))
}

/// Hand back host logit rows as `readout` asks, weighing on the host.
pub(crate) fn rows_on_host(logits: &[f32], vocab: usize, readout: &[RowReadout]) -> Result<Vec<RowOut>> {
    if vocab == 0 || logits.len() != vocab * readout.len() {
        return Err(LlamaError::format(format!(
            "{} logits for {} rows of vocab {vocab}",
            logits.len(),
            readout.len()
        )));
    }
    logits
        .chunks_exact(vocab)
        .zip(readout)
        .map(|(row, out)| match out {
            RowReadout::Logits => Ok(RowOut::Logits(row.to_vec())),
            RowReadout::Weights(spec) => weigh_row(row, spec).map(RowOut::Weights),
        })
        .collect()
}

/// Weigh and finish a host logit row.
pub(crate) fn distribution(logits: &[f32], spec: &WeighSpec) -> Result<Distribution> {
    if let Some(token) = certain_token(logits, spec) {
        return Ok(Distribution(vec![(token, 1.0)]));
    }
    finish(&weigh_row(logits, spec)?, spec)
}

/// The one token of a row whose every other logit is `-inf`, as the session
/// hands a round's already-drawn bonus token to the next.
///
/// Exactly what weighing and finishing such a row gives, without either: the
/// token weighs `exp_poly(0) = 1`, every other token 0, the sum is 1, and
/// whatever top-k and top-p keep, the token is all of the mass.
fn certain_token(logits: &[f32], spec: &WeighSpec) -> Option<u32> {
    let mut found = None;
    for (token, &logit) in logits.iter().enumerate() {
        if logit != f32::NEG_INFINITY {
            if found.is_some() {
                return None;
            }
            found = Some(token);
        }
    }
    let token = u32::try_from(found?).ok()?;
    let penalty = spec
        .penalties
        .binary_search_by_key(&token, |&(id, _)| id)
        .map(|index| spec.penalties[index].1)
        .unwrap_or(0.0);
    (logits[token as usize] - penalty).is_finite().then_some(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(temperature: f32, top_k: usize, top_p: f32) -> WeighSpec {
        WeighSpec::new(temperature, top_k, top_p, Vec::new())
    }

    /// A deterministic logit row with a realistic spread.
    fn row(vocab: usize, seed: u64) -> Vec<f32> {
        let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        (0..vocab)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let u = (x >> 40) as f32 / (1u32 << 24) as f32;
                // Mostly noise around -4, a few strong candidates.
                if u > 0.999 { 8.0 + 10.0 * u } else { -4.0 + 6.0 * u }
            })
            .collect()
    }

    #[test]
    fn the_exponential_is_within_three_ulp_monotone_and_exact_at_zero() {
        assert_eq!(exp_poly(0.0).to_bits(), 1.0f32.to_bits());
        assert_eq!(exp_poly(-0.0).to_bits(), 1.0f32.to_bits());
        assert_eq!(exp_poly(f32::NAN), 0.0);
        assert_eq!(exp_poly(f32::NEG_INFINITY), 0.0);
        assert_eq!(exp_poly(-87.000_01), 0.0);
        let mut worst = 0i64;
        let mut previous = 0.0f32;
        let mut x = -87.0f32;
        let mut checked = 0u32;
        while x < 0.0 {
            let got = exp_poly(x);
            let want = f64::from(x).exp() as f32;
            worst = worst.max((i64::from(got.to_bits()) - i64::from(want.to_bits())).abs());
            assert!(got >= previous, "exp_poly falls at {x}: {got} < {previous}");
            assert!(got > 0.0, "exp_poly({x}) left the normal range");
            previous = got;
            checked += 1;
            x = f32::from_bits(x.to_bits() - 97);
        }
        assert!(checked > 1_000_000, "walked only {checked} points");
        assert!(worst <= 3, "exp_poly is {worst} ulp off");
    }

    #[test]
    fn the_tree_sum_pairs_by_halving_strides() {
        // Values whose sum depends on the pairing: 1 + 2^-24 is not
        // representable, so adding the small ones to the big one first loses
        // them and adding them to each other first keeps them.
        let mut values = vec![2f32.powi(-24); 3000];
        values[0] = 1.0;
        values[1024] = 1.0;
        let mut want = 0.0f32;
        for part in values.chunks(WEIGH_CHUNK) {
            let mut s = part.to_vec();
            s.resize(WEIGH_CHUNK, 0.0);
            let mut stride = WEIGH_CHUNK / 2;
            while stride > 0 {
                let next: Vec<f32> = (0..stride).map(|i| s[i] + s[i + stride]).collect();
                s[..stride].copy_from_slice(&next);
                stride /= 2;
            }
            want += s[0];
        }
        assert_eq!(tree_sum(&values).to_bits(), want.to_bits());
        let sequential: f32 = values.iter().fold(0.0, |a, &b| a + b);
        assert_ne!(tree_sum(&values).to_bits(), sequential.to_bits(), "the test does not bite");
    }

    #[test]
    fn the_heaviest_tokens_rank_by_weight_then_id() {
        let small = [0.5, 2.0, 0.5, 0.0, 2.0, 1.0, 0.5];
        assert_eq!(
            heaviest(&small, 5),
            vec![(1, 2.0), (4, 2.0), (5, 1.0), (0, 0.5), (2, 0.5)]
        );
        assert_eq!(heaviest(&small, 100).len(), small.len());
        let big = row(50_000, 3);
        let (all, _) = weights(&big, &spec(0.7, 0, 0.9)).unwrap();
        let mut full: Vec<(u32, f32)> = all.iter().enumerate().map(|(t, &w)| (t as u32, w)).collect();
        full.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        assert_eq!(heaviest(&all, WEIGH_CANDIDATES), full[..WEIGH_CANDIDATES].to_vec());
    }

    #[test]
    fn penalties_come_off_the_logit_before_the_maximum() {
        let logits = vec![3.0, 1.0, 0.0, 2.0];
        let penalised = WeighSpec::new(1.0, 0, 1.0, vec![(0, 2.5), (9, 100.0)]);
        let (w, _) = weights(&logits, &penalised).unwrap();
        // Token 3 is now the maximum and weighs exactly 1.
        assert_eq!(w[3], 1.0);
        assert_eq!(w[0].to_bits(), exp_poly(0.5 - 2.0).to_bits());
        assert_eq!(w[1].to_bits(), exp_poly(-1.0).to_bits());
        let rewarded = WeighSpec::new(1.0, 0, 1.0, vec![(2, -5.0)]);
        let (w, _) = weights(&logits, &rewarded).unwrap();
        assert_eq!(w[2], 1.0, "a negative penalty can lift a token past the old maximum");
        assert!(weights(&[f32::NAN, f32::NEG_INFINITY], &spec(1.0, 0, 1.0)).is_err());
        assert!(weights(&[f32::INFINITY, 0.0], &spec(1.0, 0, 1.0)).is_err());
    }

    #[test]
    fn top_k_and_top_p_keep_the_heaviest_prefix_and_renormalise_in_token_order() {
        let logits: Vec<f32> = vec![0.0, 3.0, 1.0, 3.0, 2.0];
        let all = distribution(&logits, &spec(1.0, 0, 1.0)).unwrap();
        assert_eq!(all.entries().len(), 5);
        let two = distribution(&logits, &spec(1.0, 2, 1.0)).unwrap();
        assert_eq!(two.entries().iter().map(|e| e.0).collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(two.prob(1), 0.5);
        let nucleus = distribution(&logits, &spec(1.0, 0, 0.8)).unwrap();
        assert_eq!(nucleus.entries().iter().map(|e| e.0).collect::<Vec<_>>(), vec![1, 3, 4]);
        let sum: f32 = nucleus.entries().iter().map(|e| e.1).sum();
        assert!((sum - 1.0).abs() < 1e-6);
        let cold = distribution(&logits, &spec(0.0, 0, 1.0)).unwrap();
        // Two tokens tie for the maximum: the floor temperature splits them.
        assert_eq!(cold.entries(), &[(1, 0.5), (3, 0.5)]);
    }

    #[test]
    fn a_nucleus_wider_than_the_candidates_comes_with_every_weight_and_finishes_the_same() {
        let flat: Vec<f32> = (0..5000).map(|t| (t % 7) as f32 * 1e-3).collect();
        let wide = spec(1.0, 0, 0.9);
        let weighed = weigh_row(&flat, &wide).unwrap();
        assert!(weighed.all.is_some(), "a flat row's 0.9 nucleus is wider than 1024");
        let finished = finish(&weighed, &wide).unwrap();
        assert!(finished.entries().len() > WEIGH_CANDIDATES);
        // The same row finished from a full sort.
        let (all, sum) = weights(&flat, &wide).unwrap();
        let mut order: Vec<(u32, f32)> = all.iter().enumerate().map(|(t, &w)| (t as u32, w)).collect();
        order.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let cut = top_p_cut(&order, sum, 0.9).unwrap();
        let mut kept: Vec<(u32, f32)> = order[..cut].iter().map(|&(t, w)| (t, w / sum)).collect();
        kept.sort_by_key(|e| e.0);
        let total = kept.iter().fold(0.0f32, |a, e| a + e.1);
        let want: Vec<(u32, f32)> = kept.iter().map(|&(t, p)| (t, p / total)).collect();
        assert_eq!(finished.entries(), want.as_slice());

        let peaked = row(5000, 9);
        let narrow = weigh_row(&peaked, &wide).unwrap();
        assert!(narrow.all.is_none(), "a peaked row's nucleus fits the candidates");
        let everything = weigh_row(&peaked, &spec(1.0, 0, 1.0)).unwrap();
        assert!(everything.all.is_some(), "top_p 1 keeps every token");
        let top_k = weigh_row(&peaked, &spec(1.0, 20, 1.0)).unwrap();
        assert!(top_k.all.is_none(), "top_k 20 fits the candidates");
        // Dropping the weights a row needs is refused, not truncated.
        let mut stripped = weighed.clone();
        stripped.all = None;
        assert!(finish(&stripped, &wide).is_err());
    }

    #[test]
    fn a_row_with_one_finite_logit_is_certain_exactly_as_weighing_says() {
        let mut one_hot = vec![f32::NEG_INFINITY; 3000];
        one_hot[1234] = 0.0;
        for spec in [
            spec(0.7, 0, 0.9),
            spec(1.0, 0, 1.0),
            spec(1.0, 40, 1.0),
            WeighSpec::new(0.7, 0, 0.9, vec![(1234, 1.5), (7, 2.0)]),
        ] {
            assert_eq!(certain_token(&one_hot, &spec), Some(1234));
            assert_eq!(
                distribution(&one_hot, &spec).unwrap(),
                finish(&weigh_row(&one_hot, &spec).unwrap(), &spec).unwrap()
            );
        }
        let mut two = one_hot.clone();
        two[5] = -3.0;
        assert_eq!(certain_token(&two, &spec(0.7, 0, 0.9)), None);
        let mut nan = one_hot.clone();
        nan[5] = f32::NAN;
        assert_eq!(certain_token(&nan, &spec(0.7, 0, 0.9)), None);
        let poisoned = WeighSpec::new(0.7, 0, 0.9, vec![(1234, f32::NAN)]);
        assert_eq!(certain_token(&one_hot, &poisoned), None);
        assert!(distribution(&one_hot, &poisoned).is_err());
    }

    #[test]
    fn the_draw_and_the_residual_match_their_dense_definitions() {
        let logits: Vec<f32> = (0..4000).map(|t| ((t * 37) % 101) as f32 * 0.05).collect();
        let target = distribution(&logits, &spec(0.7, 0, 0.95)).unwrap();
        assert!(target.entries().len() > 3);
        let mut dense = vec![0.0f32; logits.len()];
        for &(token, p) in target.entries() {
            dense[token as usize] = p;
        }
        for step in 0..200 {
            let u = step as f32 / 200.0;
            let mut cumulative = 0.0f32;
            let mut want = None;
            for (token, &p) in dense.iter().enumerate() {
                if p <= 0.0 {
                    continue;
                }
                cumulative += p;
                if cumulative >= u {
                    want = Some(token as u32);
                    break;
                }
            }
            assert_eq!(target.draw(u).unwrap(), want.unwrap());
        }
        let first = target.entries()[0].0;
        let draft = vec![(first, target.prob(first) * 2.0), (3999, 0.5)];
        let residual = target.residual(&draft);
        let mut want = dense.clone();
        for &(token, q) in &draft {
            want[token as usize] = (want[token as usize] - q).max(0.0);
        }
        let total = want.iter().fold(0.0f32, |a, &b| a + b);
        for (token, value) in want.iter().enumerate() {
            assert_eq!(residual.prob(token as u32).to_bits(), (value / total).to_bits());
        }
        assert_eq!(residual.prob(first), 0.0);
        // q == p everywhere: the residual is p itself.
        assert_eq!(target.residual(target.entries()), target);
        let one = Distribution::certain(7);
        assert_eq!(one.draw(0.999).unwrap(), 7);
    }
}
