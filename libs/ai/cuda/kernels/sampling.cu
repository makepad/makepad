#include <cuda_runtime.h>
#include <math_constants.h>
#include <stdint.h>
#include "launch_profile.cuh"

// Keep in sync with GpuSamplingParams (repr(C)) in launch.rs.
struct SamplingParams {
    uint32_t vocab_start, end_token, min_tokens, top_k, penalty_window;
    uint32_t keep_first, round_logits_bf16;
    float temperature, top_p, repetition_penalty, guidance;
    uint64_t seed;
};

static __device__ __forceinline__ bool before(float a, uint32_t ai, float b, uint32_t bi) {
    return a > b || (a == b && ai < bi);
}

// LLVM's positive-integer powi order, rather than powf/log/exp.
static __device__ float integer_power(float base, uint32_t n) {
    float value = 1.0f;
    while (n) {
        if (n & 1) value = __fmul_rn(value, base);
        n >>= 1;
        if (n) base = __fmul_rn(base, base);
    }
    return value;
}

static __device__ float philox_uniform(uint64_t seed, uint64_t index, uint64_t counter) {
    uint32_t v0 = (uint32_t)counter, v1 = (uint32_t)(counter >> 32);
    uint32_t v2 = (uint32_t)index, v3 = (uint32_t)(index >> 32);
    uint32_t k0 = (uint32_t)seed, k1 = (uint32_t)(seed >> 32);
    #pragma unroll
    for (int r = 0; r < 10; r++) {
        const uint32_t hi0 = __umulhi(0xd2511f53u, v0);
        const uint32_t hi1 = __umulhi(0xcd9e8d57u, v2);
        const uint32_t lo0 = 0xd2511f53u * v0;
        const uint32_t lo1 = 0xcd9e8d57u * v2;
        v0 = hi1 ^ v1 ^ k0;
        v1 = lo1;
        v2 = hi0 ^ v3 ^ k1;
        v3 = lo0;
        k0 += 0x9e3779b9u;
        k1 += 0xbb67ae85u;
    }
    return (float)(v0 >> 8) * (1.0f / 16777216.0f);
}

static __device__ void advance_draw(uint32_t *state) {
    // state = [counter low, counter high, draws, history head].
    // Greedy consumes a draw too; only accepting a token advances the ring.
    if (++state[0] == 0) ++state[1];
    ++state[2];
}

// Fixed-size tiles are sorting geometry, not a candidate limit. Every
// vocabulary entry, including all ties at the kth score, survives to merge.
static __global__ void sample_prepare_kernel(
    const float *logits, const float *unconditional, const uint32_t *allowed,
    const uint32_t *counts, const uint32_t *state, float *original,
    float *sorted, uint32_t *ids, uint32_t n, SamplingParams p) {
    __shared__ float values[256];
    __shared__ uint32_t indices[256];
    const uint32_t t = threadIdx.x;
    const uint32_t i = blockIdx.x * 256 + t;
    float value = -CUDART_INF_F;
    if (i < n) {
        const uint32_t id = p.vocab_start + i;
        if (allowed[i] && !(id == p.end_token && state[2] < p.min_tokens)) {
            value = logits[i];
            if (unconditional) {
                const float u = unconditional[i];
                value = __fadd_rn(u, __fmul_rn(p.guidance, __fsub_rn(value, u)));
            }
            if (p.round_logits_bf16 && isfinite(value)) {
                uint32_t b = __float_as_uint(value);
                b = (b + 0x7fffu + ((b >> 16) & 1u)) & 0xffff0000u;
                value = __uint_as_float(b);
            }
            if (p.repetition_penalty != 1.0f && counts[i]) {
                const float alpha = integer_power(p.repetition_penalty, counts[i]);
                value = value < 0.0f ? __fmul_rn(value, alpha) : __fdiv_rn(value, alpha);
            }
            if (p.temperature != 0.0f && p.temperature != 1.0f) {
                value = __fdiv_rn(value, p.temperature);
            }
        }
        original[i] = value;
    }
    values[t] = value;
    indices[t] = i < n ? i : UINT32_MAX;
    __syncthreads();
    for (uint32_t span = 2; span <= 256; span <<= 1) {
        for (uint32_t stride = span >> 1; stride; stride >>= 1) {
            const uint32_t peer = t ^ stride;
            const float a = values[t], b = values[peer];
            const uint32_t ai = indices[t], bi = indices[peer];
            const bool want_first = ((t & span) == 0) == ((t & stride) == 0);
            const bool take_peer = want_first ? before(b, bi, a, ai) : before(a, ai, b, bi);
            __syncthreads();
            if (take_peer) {
                values[t] = b;
                indices[t] = bi;
            }
            __syncthreads();
        }
    }
    if (i < n) {
        sorted[i] = values[t];
        ids[i] = indices[t];
    }
}

// Merge adjacent sorted runs. Each item binary-searches its destination
// in the other run; the (score desc, index asc) total order makes the
// writes disjoint. O(vocab log^2 vocab), bounded scratch, no tie truncation.
static __global__ void sample_merge_kernel(
    const float *input, const uint32_t *ids, float *output, uint32_t *out_ids,
    uint32_t n, uint32_t run) {
    const uint32_t i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n) return;
    const uint32_t pair = (i / (2 * run)) * (2 * run);
    const bool right = i - pair >= run;
    const uint32_t own = right ? pair + run : pair;
    const uint32_t other = right ? pair : min(pair + run, n);
    const uint32_t end = min(other + run, n);
    uint32_t lo = other, hi = end;
    const float value = input[i];
    const uint32_t id = ids[i];
    while (lo < hi) {
        const uint32_t mid = lo + (hi - lo) / 2;
        if (before(input[mid], ids[mid], value, id)) lo = mid + 1;
        else hi = mid;
    }
    const uint32_t dst = pair + (i - own) + (lo - other);
    output[dst] = value;
    out_ids[dst] = id;
}

static __device__ void sample_reduce_best(float &score, uint32_t &order) {
    for (int delta = 16; delta > 0; delta >>= 1) {
        const float other = __shfl_down_sync(0xffffffffu, score, delta);
        const uint32_t index = __shfl_down_sync(0xffffffffu, order, delta);
        if (before(other, index, score, order)) { score = other; order = index; }
    }
    __shared__ float scores[8];
    __shared__ uint32_t orders[8];
    const uint32_t lane = threadIdx.x & 31, warp = threadIdx.x >> 5;
    if (lane == 0) { scores[warp] = score; orders[warp] = order; }
    __syncthreads();
    score = threadIdx.x < 8 ? scores[lane] : -CUDART_INF_F;
    order = threadIdx.x < 8 ? orders[lane] : UINT32_MAX;
    if (warp == 0) {
        for (int delta = 16; delta > 0; delta >>= 1) {
            const float other = __shfl_down_sync(0xffffffffu, score, delta);
            const uint32_t index = __shfl_down_sync(0xffffffffu, order, delta);
            if (before(other, index, score, order)) { score = other; order = index; }
        }
    }
}

static __global__ void sample_greedy_kernel(
    const float *tiles, const uint32_t *ids, uint32_t *state, uint32_t *out,
    uint32_t n, uint32_t vocab_start) {
    float best = -CUDART_INF_F;
    uint32_t index = UINT32_MAX;
    for (uint32_t i = threadIdx.x * 256; i < n; i += blockDim.x * 256) {
        if (before(tiles[i], ids[i], best, index)) { best = tiles[i]; index = ids[i]; }
    }
    sample_reduce_best(best, index);
    if (threadIdx.x == 0) { *out = vocab_start + index; advance_draw(state); }
}

static __global__ void sample_finish_kernel(
    const float *sorted, const uint32_t *ids, float *weights,
    uint32_t *state, uint32_t *out, uint32_t n, SamplingParams p) {
    __shared__ uint32_t begin, count;
    __shared__ float total, threshold, maximum;
    if (threadIdx.x == 0) {
        threshold = sorted[min(max(p.top_k, 1u), n) - 1];
        begin = 0;
        while (begin < n && !isfinite(sorted[begin])) ++begin;
        count = 0;
        // Nonfinite logits are excluded by the host after top-k selection.
        while (begin + count < n && sorted[begin + count] >= threshold
                && isfinite(sorted[begin + count])) ++count;
        maximum = count ? sorted[begin] : -CUDART_INF_F;
    }
    __syncthreads();
    if (!count) {
        if (threadIdx.x == 0) { *out = p.vocab_start; advance_draw(state); }
        return;
    }
    const bool nucleus = p.top_p < 1.0f;
    const uint32_t length = nucleus ? count : n;
    // weights initially holds original (vocab-order) scores. In the top-p
    // branch the reference sorts candidates, otherwise it retains that
    // original order. Keep both accumulation and tie-breaking orders.
    for (uint32_t i = threadIdx.x; i < length; i += blockDim.x) {
        const float value = nucleus ? sorted[begin + i] : weights[i];
        weights[i] = isfinite(value) && value >= threshold ? expf(value - maximum) : 0.0f;
    }
    __syncthreads();
    if (threadIdx.x == 0) {
        total = 0.0f;
        for (uint32_t i = 0; i < length; i++) total = __fadd_rn(total, weights[i]);
        if (nucleus) {
            float cumsum = 0.0f;
            float kept_total = 0.0f;
            for (uint32_t i = 0; i < length; i++) {
                const float prob = __fdiv_rn(weights[i], total);
                cumsum = __fadd_rn(cumsum, prob);
                if (i >= p.keep_first && __fsub_rn(cumsum, prob) > p.top_p) weights[i] = 0.0f;
                else kept_total = __fadd_rn(kept_total, weights[i]);
            }
            total = kept_total;
        }
    }
    __syncthreads();
    const uint64_t counter = (uint64_t)state[0] | ((uint64_t)state[1] << 32);
    float best = -CUDART_INF_F;
    uint32_t order = UINT32_MAX;
    for (uint32_t i = threadIdx.x; i < length; i += blockDim.x) {
        const float prob = __fdiv_rn(weights[i], total);
        if (!(prob > 0.0f)) continue;
        const uint32_t id = p.vocab_start + (nucleus ? ids[begin + i] : i);
        float u = philox_uniform(p.seed, id, counter);
        u = fminf(fmaxf(u, 1.0e-12f), 1.0f - 1.0e-7f);
        const float score = __fsub_rn(logf(prob), logf(-logf(u)));
        if (before(score, i, best, order)) { best = score; order = i; }
    }
    sample_reduce_best(best, order);
    if (threadIdx.x == 0) {
        *out = p.vocab_start + (nucleus ? ids[begin + order] : order);
        advance_draw(state);
    }
}

// Accept the previous draw only when continuing the sequence. Separating
// this from sampling also permits oracle teacher forcing: replace `token`
// before the next step and both cache input and repetition history agree.
// O(1) per accepted token: one ring slot and at most two count entries.
// The vocabulary-sized counts array is zeroed only at sampler creation.
static __global__ void sample_history_push_kernel(
    uint32_t *counts, uint32_t *history, uint32_t *state,
    const uint32_t *token, uint32_t n, uint32_t vocab_start, uint32_t window) {
    if (!window || !state[2]) return;
    const uint32_t slot = state[3];
    const uint32_t old = history[slot], next = *token;
    // Advance even when the incoming id equals the outgoing one. Keeping
    // the head on device avoids both integer division and a host readback.
    state[3] = slot + 1 == window ? 0 : slot + 1;
    if (old == next) return; // Unchanged slot/counts, including repeated ids.
    if (old >= vocab_start && old - vocab_start < n) --counts[old - vocab_start];
    history[slot] = next;
    if (next >= vocab_start && next - vocab_start < n) ++counts[next - vocab_start];
}

extern "C" cudaError_t makepad_cuda_sample_history_push(
    uint32_t *counts, uint32_t *history, uint32_t *state,
    const uint32_t *token, uint32_t n, uint32_t vocab_start,
    uint32_t window, cudaStream_t stream) {
    if (window) {
        MakepadLaunchTiming timing("sampler.history", stream, 1, window);
        sample_history_push_kernel<<<1, 1, 0, stream>>>(
            counts, history, state, token, n, vocab_start, window);
    }
    return cudaGetLastError();
}

// Workspace is five vocab-sized planes: original/weights, two value
// planes and two index planes. No device allocation or host readback.
extern "C" cudaError_t makepad_cuda_sample_top_k_top_p_f32(
    const float *logits, const float *unconditional, const uint32_t *allowed,
    const uint32_t *counts, uint32_t *state, uint32_t *out,
    float *workspace, uint32_t n, const SamplingParams *params, cudaStream_t stream) {
    if (!n || !params) return cudaErrorInvalidValue;
    const SamplingParams p = *params;
    float *original = workspace, *a = workspace + n, *b = workspace + (size_t)2 * n;
    uint32_t *ai = reinterpret_cast<uint32_t *>(workspace + (size_t)3 * n);
    uint32_t *bi = reinterpret_cast<uint32_t *>(workspace + (size_t)4 * n);
    const uint32_t blocks = (n + 255) / 256;
    {
        MakepadLaunchTiming timing("sampler.prepare", stream, 1, n);
        sample_prepare_kernel<<<blocks, 256, 0, stream>>>(
            logits, unconditional, allowed, counts, state, original, a, ai, n, p);
    }
    if (p.temperature == 0.0f) {
        MakepadLaunchTiming timing("sampler.greedy", stream, 1, n);
        sample_greedy_kernel<<<1, 256, 0, stream>>>(a, ai, state, out, n, p.vocab_start);
    } else {
        for (uint32_t run = 256; run < n; run <<= 1) {
            MakepadLaunchTiming timing("sampler.merge", stream, 1, n);
            sample_merge_kernel<<<blocks, 256, 0, stream>>>(a, ai, b, bi, n, run);
            float *v = a; a = b; b = v;
            uint32_t *i = ai; ai = bi; bi = i;
        }
        MakepadLaunchTiming timing("sampler.finish", stream, 1, n);
        sample_finish_kernel<<<1, 256, 0, stream>>>(a, ai, original, state, out, n, p);
    }
    return cudaGetLastError();
}
