//! The host side of one decode token, runnable with no GPU and no weight data: what the CUDA
//! executor (`cuda_exec::real`, `Compiled::execute`) and the session run on the CPU around the
//! device launch. For benchmarks of the host code (rwbench's `llm_host`): a GGUF's header and tensor
//! inventory is mapped the way a session maps it, the hybrid decode graph is built over it (once per
//! attention key bucket, as `ensure_compiled_graph` does), and each token then runs the layout
//! validation, the input writes (token ids, positions, attention mask, state rows), the readback
//! shaping (`collect_hybrid_decode_run`), the single-token collapse and greedy sampling.
//! The device launch is the only thing missing; its logits are a fixed vector of the model's vocab.
//! The CUDA graph planner (`plan_raw_graph`) stays out: it lives in `cuda_exec/real.rs`, which
//! compiles only with the CUDA kernels built, and calls into the kernel library for metadata.

use crate::error::{LlamaError, Result};
use crate::model::LlamaModel;
use crate::runtime::{
    allocate_hybrid_shared_cache_tensors, build_hybrid_decode_graph_with_sequences, build_hybrid_decode_writes,
    collect_hybrid_decode_run, import_hybrid_graph_context, validate_hybrid_decode_layout, HybridDecodeBatchLayout,
    HybridDecodeGraph, HybridDecodeOutputConfig, HybridDecodeSpec, HybridSharedCacheTensorIds,
    ImportedHybridGraphContext, LogitsProbeInput,
};
use crate::session::{argmax_token_id, collapse_last_token_run};
use crate::weights::LoadedGgufWeights;
use crate::{Context, TensorId, TensorType};
use std::collections::BTreeMap;
use std::path::Path;

/// Graphs are keyed by the attention key count rounded up to this many rows (session.rs).
pub const GRAPH_KEY_BUCKET: usize = 256;

struct Built {
    ctx: Context,
    decode: HybridDecodeGraph,
}

pub struct HostDecodeBench {
    spec: HybridDecodeSpec,
    weights: LoadedGgufWeights,
    shared_cache: HybridSharedCacheTensorIds,
    max_context: usize,
    built: Option<Built>,
    /// The device's logits for one token: the model's vocab of f32, fixed
    logits: Vec<u8>,
}

fn mix(h: u64, v: u64) -> u64 {
    (h.rotate_left(5) ^ v).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

impl HostDecodeBench {
    /// Map `gguf` and prepare the decode spec and cache tensors (a session's load, minus the device).
    pub fn new(gguf: &Path, max_context: u32) -> Result<Self> {
        let mut model = LlamaModel::load(gguf)?;
        model.set_load_mtp(false);
        model.validate_layout()?;
        let plan = model.execution_plan()?;
        let max_context = max_context.next_multiple_of(GRAPH_KEY_BUCKET as u32);
        let spec = model.hybrid_decode_spec(max_context, 1, TensorType::F16, TensorType::F16, TensorType::F32, TensorType::F32)?;
        let mut weights = plan
            .full_weights
            .map_and_load(&model.gguf, 1 << 30)
            .ok_or_else(|| LlamaError::format("could not map the gguf"))?;
        let shared_cache = allocate_hybrid_shared_cache_tensors(&mut weights.ctx, &weights.tensor_ids, &spec)?;
        Ok(HostDecodeBench { spec, weights, shared_cache, max_context: max_context as usize, built: None, logits: Vec::new() })
    }

    /// The graph for a decode step at `cache_tokens` rows (what the session does when the key
    /// bucket changes): import the context, build the decode graph. Returns a digest of its shape.
    pub fn build_graph(&mut self, cache_tokens: usize) -> Result<u64> {
        let key_count = cache_tokens.next_multiple_of(GRAPH_KEY_BUCKET).min(self.max_context);
        let ImportedHybridGraphContext { mut ctx, tensor_ids, shared_cache } =
            import_hybrid_graph_context(&self.weights, Some(&self.shared_cache), false)?;
        let decode = build_hybrid_decode_graph_with_sequences(&mut ctx, &tensor_ids, &self.spec, shared_cache.as_ref(), 1, 1, 0, key_count, 1)?;
        let mut h = mix(0, decode.graph.nodes.len() as u64);
        h = mix(h, decode.graph.leafs.len() as u64);
        for &id in &decode.graph.nodes {
            let t = ctx.tensor(id).ok_or_else(|| LlamaError::format("graph node is not a tensor"))?;
            h = mix(h, (t.ne[0] as u64) << 32 ^ t.ne[1] as u64 ^ (t.ne[2] as u64) << 16);
        }
        let vocab = ctx.tensor(decode.result_logits).ok_or_else(|| LlamaError::format("no logits tensor"))?.ne[0] as usize;
        if self.logits.len() != vocab * 4 {
            let mut x = 0x2545_F491_4F6C_DD1Du64;
            self.logits = Vec::with_capacity(vocab * 4);
            for _ in 0..vocab {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let f = ((x >> 40) as f32) / (1u64 << 24) as f32 * 20.0 - 10.0;
                self.logits.extend_from_slice(&f.to_le_bytes());
            }
        }
        self.built = Some(Built { ctx, decode });
        Ok(h)
    }

    /// One decode token at position `position` of the sequence: returns (sampled token, digest).
    pub fn step(&mut self, token: i32, position: usize) -> Result<(i32, u64)> {
        let built = self.built.as_ref().ok_or_else(|| LlamaError::format("build_graph first"))?;
        let cache_tokens = position + 1;
        let mut layout = HybridDecodeBatchLayout::from_contiguous_positions_and_outputs(&[position as i32], cache_tokens, &[0])?;
        layout.recurrent_state_rows = vec![0];
        if built.decode.input_recurrent_state_rows.is_none() {
            layout.recurrent_state_rows.clear();
        }
        validate_hybrid_decode_layout(&built.ctx, &self.spec, &built.decode, &layout)?;
        let writes = build_hybrid_decode_writes(&built.ctx, &self.spec, &built.decode, LogitsProbeInput::TokenIds(&[token]), &layout)?;
        let mut h = 0u64;
        for (id, bytes) in &writes {
            h = mix(h, *id as u64 ^ (bytes.len() as u64) << 32);
            let mut i = 0;
            while i < bytes.len() {
                h = mix(h, bytes[i] as u64);
                i += 37;
            }
        }
        // the device's answer: the fixed logits with one position raised, read back as bytes
        let mut out = self.logits.clone();
        let at = (position.wrapping_mul(7919) % (out.len() / 4)) * 4;
        out[at..at + 4].copy_from_slice(&40.0f32.to_le_bytes());
        let mut outputs: BTreeMap<TensorId, Vec<u8>> = BTreeMap::new();
        outputs.insert(built.decode.result_logits, out);
        let run = collect_hybrid_decode_run(&built.ctx, &built.decode, HybridDecodeOutputConfig::LOGITS_ONLY, &outputs)?;
        let run = collapse_last_token_run(run)?;
        let next = argmax_token_id(&run.logits)?;
        Ok((next, mix(h, next as u64)))
    }
}
