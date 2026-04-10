fn default_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors")
}

fn model_root_dir(model_path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    if model_path.is_dir() {
        return Ok(model_path.to_path_buf());
    }
    model_path.parent().map(Path::to_path_buf).ok_or_else(|| {
        format!(
            "model path {} has no parent directory",
            model_path.display()
        )
        .into()
    })
}

fn create_private_buffer_with_concatenated_tensors(
    runtime: &MetalRuntime,
    weights: &MlxIndexedSafetensors,
    tensor_names: &[&str],
) -> Result<MetalBuffer, Box<dyn Error>> {
    let mut bytes = Vec::new();
    for tensor_name in tensor_names {
        bytes.extend(weights.read_tensor_bytes(tensor_name)?);
    }
    Ok(runtime.create_buffer_with_bytes(&bytes, BufferStorageMode::Private)?)
}

fn create_private_buffer_with_concatenated_expert_tensors(
    runtime: &MetalRuntime,
    weights: &MlxIndexedSafetensors,
    tensor_names: &[&str],
) -> Result<MetalBuffer, Box<dyn Error>> {
    struct ExpertTensorBytes {
        bytes: Vec<u8>,
        expert_chunk_bytes: usize,
    }

    let first_name = *tensor_names
        .first()
        .ok_or("expected at least one expert tensor to concatenate")?;
    let first_entry = weights.tensor(first_name)?;
    if first_entry.shape.len() != 3 {
        return Err(format!(
            "expected rank-3 expert tensor for {}, got {:?}",
            first_name, first_entry.shape
        )
        .into());
    }
    let expert_count = usize::try_from(first_entry.shape[0])?;
    let row_width = first_entry.shape[2];
    let dtype = first_entry.dtype;
    let element_bytes = usize::try_from(dtype.byte_width())?;

    let mut total_bytes = 0usize;
    let mut tensors = Vec::with_capacity(tensor_names.len());
    for tensor_name in tensor_names {
        let entry = weights.tensor(tensor_name)?;
        if entry.shape.len() != 3 {
            return Err(format!(
                "expected rank-3 expert tensor for {}, got {:?}",
                tensor_name, entry.shape
            )
            .into());
        }
        if entry.dtype != dtype
            || entry.shape[0] != first_entry.shape[0]
            || entry.shape[2] != row_width
        {
            return Err(format!(
                "expert tensor layout mismatch for {}: dtype={:?} shape={:?}, expected dtype={:?} expert_count={} row_width={}",
                tensor_name,
                entry.dtype,
                entry.shape,
                dtype,
                first_entry.shape[0],
                row_width,
            )
            .into());
        }
        let rows = usize::try_from(entry.shape[1])?;
        let expert_chunk_bytes = rows
            .checked_mul(usize::try_from(row_width)?)
            .and_then(|value| value.checked_mul(element_bytes))
            .ok_or("expert tensor chunk size overflow")?;
        let bytes = weights.read_tensor_bytes(tensor_name)?;
        if bytes.len()
            != expert_count
                .checked_mul(expert_chunk_bytes)
                .ok_or("expert tensor size overflow")?
        {
            return Err(format!(
                "expert tensor byte size mismatch for {}: got {} expected {}",
                tensor_name,
                bytes.len(),
                expert_count * expert_chunk_bytes
            )
            .into());
        }
        total_bytes = total_bytes
            .checked_add(bytes.len())
            .ok_or("combined expert tensor size overflow")?;
        tensors.push(ExpertTensorBytes {
            bytes,
            expert_chunk_bytes,
        });
    }

    let mut bytes = Vec::with_capacity(total_bytes);
    for expert_idx in 0..expert_count {
        for tensor in &tensors {
            let start = expert_idx
                .checked_mul(tensor.expert_chunk_bytes)
                .ok_or("expert tensor slice start overflow")?;
            let end = start
                .checked_add(tensor.expert_chunk_bytes)
                .ok_or("expert tensor slice end overflow")?;
            bytes.extend_from_slice(&tensor.bytes[start..end]);
        }
    }
    Ok(runtime.create_buffer_with_bytes(&bytes, BufferStorageMode::Private)?)
}

struct LayerExecutionSession {
    model_path: PathBuf,
    weights: MlxIndexedSafetensors,
    runtime: MetalRuntime,
    private_weight_buffers: HashMap<String, MetalBuffer>,
}

impl LayerExecutionSession {
    fn load(model_path: PathBuf) -> Result<Self, Box<dyn Error>> {
        let model_root = model_root_dir(&model_path)?;
        let weights = MlxIndexedSafetensors::load(&model_root)?;
        let runtime =
            MetalRuntime::new().map_err(|err| format!("MetalRuntime::new failed: {err}"))?;
        if !runtime.features().has_bfloat {
            return Err("Metal device does not report BF16 support".into());
        }
        Ok(Self {
            model_path,
            weights,
            runtime,
            private_weight_buffers: HashMap::new(),
        })
    }

    fn private_weight_buffer(&mut self, name: &str) -> Result<MetalBuffer, Box<dyn Error>> {
        if let Some(buffer) = self.private_weight_buffers.get(name) {
            return Ok(buffer.clone());
        }
        let bytes = self.weights.read_tensor_bytes(name)?;
        let buffer = self
            .runtime
            .create_buffer_with_bytes(&bytes, BufferStorageMode::Private)?;
        self.private_weight_buffers
            .insert(name.to_string(), buffer.clone());
        Ok(buffer)
    }
}

struct ExactMetalKvCache {
    spec: GemmaKvCacheSpec,
    key_buffer: MetalBuffer,
    value_buffer: MetalBuffer,
    stored_tokens: usize,
    next_position: usize,
}

impl ExactMetalKvCache {
    fn load(runtime: &MetalRuntime, spec: GemmaKvCacheSpec) -> Result<Self, Box<dyn Error>> {
        let storage_words = spec
            .batch_size
            .checked_mul(spec.kv_head_count)
            .and_then(|value| value.checked_mul(spec.max_tokens))
            .and_then(|value| value.checked_mul(spec.head_dim))
            .ok_or("exact metal KV cache storage overflow")?;
        Ok(Self {
            key_buffer: create_bf16_buffer(runtime, storage_words, BufferStorageMode::Private)?,
            value_buffer: create_bf16_buffer(runtime, storage_words, BufferStorageMode::Private)?,
            spec,
            stored_tokens: 0,
            next_position: 0,
        })
    }

    fn reset(&mut self) {
        self.stored_tokens = 0;
        self.next_position = 0;
    }

    fn capacity_tokens(&self) -> usize {
        self.spec.max_tokens
    }

    fn row_stride_words(&self) -> Result<usize, Box<dyn Error>> {
        self.spec
            .max_tokens
            .checked_mul(self.spec.head_dim)
            .ok_or_else(|| "exact metal KV row stride overflow".into())
    }

    fn start_slot(&self) -> usize {
        match self.spec.attention {
            GemmaAttentionKind::Full => 0,
            GemmaAttentionKind::Sliding if self.stored_tokens < self.spec.max_tokens => 0,
            GemmaAttentionKind::Sliding => self.next_position % self.spec.max_tokens,
        }
    }

    fn seq_len(&self) -> usize {
        self.stored_tokens
    }

    fn append_token_from_buffers(
        &mut self,
        runtime: &MetalRuntime,
        src_k: &MetalBuffer,
        src_v: &MetalBuffer,
    ) -> Result<(), Box<dyn Error>> {
        if self.spec.attention == GemmaAttentionKind::Full
            && self.stored_tokens >= self.spec.max_tokens
        {
            return Err(format!(
                "exact metal full KV cache overflow: attempted token {} with capacity {}",
                self.next_position + 1,
                self.spec.max_tokens
            )
            .into());
        }

        let slot = self.next_position % self.spec.max_tokens;
        let head_dim_words = self.spec.head_dim;
        let row_stride_words = self.row_stride_words()?;
        let bytes_per_head = head_dim_words * size_of::<u16>();

        for head in 0..self.spec.kv_head_count {
            let src_offset = head
                .checked_mul(bytes_per_head)
                .ok_or("exact metal KV src offset overflow")?;
            let dst_word_offset = head
                .checked_mul(row_stride_words)
                .and_then(|value| value.checked_add(slot * head_dim_words))
                .ok_or("exact metal KV dst offset overflow")?;
            let dst_offset = dst_word_offset
                .checked_mul(size_of::<u16>())
                .ok_or("exact metal KV dst byte offset overflow")?;
            runtime.copy_buffer_range(
                src_k,
                src_offset,
                &self.key_buffer,
                dst_offset,
                bytes_per_head,
            )?;
            runtime.copy_buffer_range(
                src_v,
                src_offset,
                &self.value_buffer,
                dst_offset,
                bytes_per_head,
            )?;
        }

        self.next_position = self
            .next_position
            .checked_add(1)
            .ok_or("exact metal KV next_position overflow")?;
        self.stored_tokens = self
            .stored_tokens
            .saturating_add(1)
            .min(self.spec.max_tokens);
        Ok(())
    }

    fn append_token_from_buffers_compute(
        &mut self,
        runtime: &MetalRuntime,
        append_pipeline: &MetalPipeline,
        src_k: &MetalBuffer,
        src_v: &MetalBuffer,
    ) -> Result<(), Box<dyn Error>> {
        if self.spec.attention == GemmaAttentionKind::Full
            && self.stored_tokens >= self.spec.max_tokens
        {
            return Err(format!(
                "exact metal full KV cache overflow: attempted token {} with capacity {}",
                self.next_position + 1,
                self.spec.max_tokens
            )
            .into());
        }

        let slot = self.next_position % self.spec.max_tokens;
        let row_stride_words = self.row_stride_words()?;
        let args = MlxKvAppendBf16Args {
            head_dim: self.spec.head_dim as u32,
            src_row_stride: self.spec.head_dim as u32,
            dst_row_stride: row_stride_words as u32,
            head_count: self.spec.kv_head_count as u32,
            slot: slot as u32,
        };
        let threadgroups = MetalSize {
            width: (self.spec.head_dim as u64).div_ceil(64),
            height: self.spec.kv_head_count as u64,
            depth: 1,
        };
        let threads_per_threadgroup = MetalSize {
            width: 64,
            height: 1,
            depth: 1,
        };

        dispatch_compute_tracked_split(
            runtime,
            append_pipeline,
            bytes_of(&args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: src_k,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: src_v,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &self.key_buffer,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &self.value_buffer,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            threadgroups,
            threads_per_threadgroup,
        )?;
        self.next_position = self
            .next_position
            .checked_add(1)
            .ok_or("exact metal KV next_position overflow")?;
        self.stored_tokens = self
            .stored_tokens
            .saturating_add(1)
            .min(self.spec.max_tokens);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct ExactMetalQprojLayout {
    weight_words_per_row: u32,
    qparams_per_row: u32,
    out_rows: u32,
}

impl ExactMetalQprojLayout {
    fn out_len(self) -> usize {
        self.out_rows as usize
    }

    fn uses_fast_qmv(self, n_in: u32) -> bool {
        self.out_rows % 8 == 0 && n_in % 512 == 0
    }

    fn row_args(self, n_in: u32) -> MlxAffineQprojRowArgs {
        MlxAffineQprojRowArgs {
            n_in,
            weight_words_per_row: self.weight_words_per_row,
            qparams_per_row: self.qparams_per_row,
            out_rows: self.out_rows,
        }
    }

    fn selected_experts_args(
        self,
        n_in: u32,
        input_row_stride: u32,
    ) -> MlxAffineSelectedExpertsQprojRowArgs {
        MlxAffineSelectedExpertsQprojRowArgs {
            n_in,
            weight_words_per_row: self.weight_words_per_row,
            qparams_per_row: self.qparams_per_row,
            out_rows: self.out_rows,
            input_row_stride,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ExactMetalRopeLayout {
    half_dims: u32,
    row_stride: u32,
    row_count: u32,
    base_log2: f32,
}

impl ExactMetalRopeLayout {
    fn args(self, position: usize) -> Result<MlxRopeSingleArgs, Box<dyn Error>> {
        Ok(MlxRopeSingleArgs {
            half_dims: self.half_dims,
            row_stride: self.row_stride,
            row_count: self.row_count,
            offset: i32::try_from(position)?,
            scale: ROPE_SCALE,
            base_log2: self.base_log2,
        })
    }
}

#[derive(Clone)]
struct ExactMetalLayerBuffers {
    x: MetalBuffer,
    h: MetalBuffer,
    qkv_proj_out: MetalBuffer,
    q_norm: MetalBuffer,
    q_rope: MetalBuffer,
    k_norm: MetalBuffer,
    k_rope: MetalBuffer,
    v_norm: MetalBuffer,
    attention_logits: MetalBuffer,
    attention_probs: MetalBuffer,
    attn_out: MetalBuffer,
    o_proj_out: MetalBuffer,
    post_attention_norm_out: MetalBuffer,
    residual_out: MetalBuffer,
    pre_feedforward_norm_out: MetalBuffer,
    mlp_gate_up_out: MetalBuffer,
    geglu_out: MetalBuffer,
    mlp_down_out: MetalBuffer,
    router_scaled_out: MetalBuffer,
    router_proj_out: MetalBuffer,
    router_probs_out: MetalBuffer,
    pre_feedforward_norm2_out: MetalBuffer,
    moe_top_k_indices: MetalBuffer,
    moe_top_k_weights: MetalBuffer,
    expert_gate_up_out: MetalBuffer,
    expert_geglu_out: MetalBuffer,
    expert_down_out: MetalBuffer,
    post_feedforward_norm1_out: MetalBuffer,
    moe_weighted_out: MetalBuffer,
    moe_post_ffn_norm2_out: MetalBuffer,
    moe_merge_out: MetalBuffer,
    post_ffn_residual_out: MetalBuffer,
}

#[derive(Clone)]
struct ExactMetalLayerWeights {
    input_norm_weight: MetalBuffer,
    qkv_proj_weight: MetalBuffer,
    qkv_proj_scales: MetalBuffer,
    qkv_proj_biases: MetalBuffer,
    q_norm_weight: MetalBuffer,
    k_norm_weight: MetalBuffer,
    v_norm_weight: MetalBuffer,
    o_weight: MetalBuffer,
    o_scales: MetalBuffer,
    o_biases: MetalBuffer,
    post_attention_norm_weight: MetalBuffer,
    pre_feedforward_norm_weight: MetalBuffer,
    pre_feedforward_norm2_weight: MetalBuffer,
    mlp_gate_up_weight: MetalBuffer,
    mlp_gate_up_scales: MetalBuffer,
    mlp_gate_up_biases: MetalBuffer,
    mlp_down_weight: MetalBuffer,
    mlp_down_scales: MetalBuffer,
    mlp_down_biases: MetalBuffer,
    router_scale_weight: MetalBuffer,
    router_proj_weight: MetalBuffer,
    router_proj_scales: MetalBuffer,
    router_proj_biases: MetalBuffer,
    router_per_expert_scale: MetalBuffer,
    expert_gate_up_weight: MetalBuffer,
    expert_gate_up_scales: MetalBuffer,
    expert_gate_up_biases: MetalBuffer,
    expert_down_weight: MetalBuffer,
    expert_down_scales: MetalBuffer,
    expert_down_biases: MetalBuffer,
    post_feedforward_norm1_weight: MetalBuffer,
    post_feedforward_norm2_weight: MetalBuffer,
}

#[derive(Clone)]
struct ExactMetalLayerPipelines {
    rms: MetalPipeline,
    proj: MetalPipeline,
    proj_fast: MetalPipeline,
    head_norm: MetalPipeline,
    rope: MetalPipeline,
    attention_logits_seq: MetalPipeline,
    attention_softmax_rows: MetalPipeline,
    attention_weighted_sum: MetalPipeline,
    o_proj_fast: MetalPipeline,
    residual: MetalPipeline,
    weighted_sum_rows: MetalPipeline,
    geglu: MetalPipeline,
    geglu_strided: MetalPipeline,
    router_scale_pair: MetalPipeline,
    router_topk: MetalPipeline,
    selected_expert_proj: MetalPipeline,
}

#[derive(Clone)]
struct ExactMetalLayerWorkspace {
    qkv_proj: ExactMetalQprojLayout,
    q_proj: ExactMetalQprojLayout,
    k_proj: ExactMetalQprojLayout,
    o_proj: ExactMetalQprojLayout,
    mlp_gate_up: ExactMetalQprojLayout,
    mlp_gate: ExactMetalQprojLayout,
    mlp_down: ExactMetalQprojLayout,
    router_proj: ExactMetalQprojLayout,
    expert_gate_up: ExactMetalQprojLayout,
    expert_gate: ExactMetalQprojLayout,
    expert_down: ExactMetalQprojLayout,
    post_attention_norm_len: usize,
    pre_feedforward_norm_len: usize,
    pre_feedforward_norm2_len: usize,
    post_feedforward_norm1_len: usize,
    post_feedforward_norm2_len: usize,
    q_head_count: usize,
    k_head_count: usize,
    v_head_count: usize,
    q_heads_per_kv: usize,
    head_dim: usize,
    kv_cache_capacity_tokens: usize,
    eps: f32,
    q_rope: ExactMetalRopeLayout,
    k_rope: ExactMetalRopeLayout,
    buffers: ExactMetalLayerBuffers,
    weights: ExactMetalLayerWeights,
    pipelines: ExactMetalLayerPipelines,
}

#[derive(Clone)]
struct ExactMetalTextIoBuffers {
    standalone_hidden: MetalBuffer,
    hidden_scratch: MetalBuffer,
    final_norm_out: MetalBuffer,
    logits_out: MetalBuffer,
    argmax_index_out: MetalBuffer,
    generated_token_chunk_out: MetalBuffer,
}

#[derive(Clone)]
struct ExactMetalTextIoWeights {
    embed_weight: MetalBuffer,
    embed_scales: MetalBuffer,
    embed_biases: MetalBuffer,
    final_norm_weight: MetalBuffer,
}

#[derive(Clone)]
struct ExactMetalTextIoPipelines {
    dequant_row: MetalPipeline,
    dequant_row_from_token_buffer: MetalPipeline,
    rms: MetalPipeline,
    logits_proj: MetalPipeline,
    argmax_softcapped_bf16: MetalPipeline,
}

#[derive(Clone)]
struct ExactMetalTextIoWorkspace {
    embed_weight_row_bytes: usize,
    embed_qparams_row_bytes: usize,
    logits_qproj: ExactMetalQprojLayout,
    vocab_size: usize,
    eps: f32,
    softcap: Option<f32>,
    buffers: ExactMetalTextIoBuffers,
    weights: ExactMetalTextIoWeights,
    pipelines: ExactMetalTextIoPipelines,
}

fn dispatch_exact_mlx_qmv_row(
    runtime: &MetalRuntime,
    generic_pipeline: &MetalPipeline,
    fast_pipeline: &MetalPipeline,
    layout: ExactMetalQprojLayout,
    args: &MlxAffineQprojRowArgs,
    bindings: &[MetalBufferBindingRef<'_>],
    threadgroups: MetalSize,
    threads_per_threadgroup: MetalSize,
) -> Result<(), Box<dyn Error>> {
    let pipeline = if layout.uses_fast_qmv(args.n_in) {
        fast_pipeline
    } else {
        generic_pipeline
    };
    runtime.dispatch_compute_tracked(
        pipeline,
        bytes_of(args),
        &bindings[..bindings.len() - 1],
        &bindings[bindings.len() - 1..],
        &[],
        threadgroups,
        threads_per_threadgroup,
    )?;
    Ok(())
}

fn dispatch_compute_tracked_split<const N: usize>(
    runtime: &MetalRuntime,
    pipeline: &MetalPipeline,
    args_bytes: &[u8],
    bindings: [MetalBufferBindingRef<'_>; N],
    output_start: usize,
    threadgroup_memory_lengths: &[(u64, usize)],
    threadgroups: MetalSize,
    threads_per_threadgroup: MetalSize,
) -> Result<(), Box<dyn Error>> {
    runtime.dispatch_compute_tracked(
        pipeline,
        args_bytes,
        &bindings[..output_start],
        &bindings[output_start..],
        threadgroup_memory_lengths,
        threadgroups,
        threads_per_threadgroup,
    )?;
    Ok(())
}

impl ExactMetalLayerWorkspace {
