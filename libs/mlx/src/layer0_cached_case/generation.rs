pub fn profile_decode_layers_after_prompt_token_ids(
    model_path: PathBuf,
    prompt_token_ids: &[u32],
) -> Result<ExactMetalDecodeStepProfile, Box<dyn Error>> {
    let mut backend = ExactMetalTextRuntimeSession::load(model_path)?;
    backend.profile_decode_layers_after_prompt(prompt_token_ids)
}

impl ExactMetalGenerationCursor {
    fn eval_token_next_with_backend(
        backend: &mut ExactMetalTextRuntimeSession,
        token_id: u32,
        position: usize,
    ) -> Result<u32, Box<dyn Error>> {
        if position == 0 {
            backend.reset_kv_caches();
        }
        backend.eval_token_greedy_token_id_from_token_id(token_id, position)
    }

    fn eval_token_chunk_with_backend(
        backend: &mut ExactMetalTextRuntimeSession,
        token_id: u32,
        position: usize,
        token_count: usize,
    ) -> Result<Vec<u32>, Box<dyn Error>> {
        if token_count == 0 {
            return Ok(Vec::new());
        }
        if position == 0 {
            backend.reset_kv_caches();
        }
        let mut current_token = token_id;
        let mut generated = Vec::with_capacity(token_count);
        for step_idx in 0..token_count {
            let next_token = backend
                .eval_token_greedy_token_id_from_token_id(current_token, position + step_idx)?;
            generated.push(next_token);
            current_token = next_token;
        }
        Ok(generated)
    }

    fn ensure_prompt_prefilled_locked(
        &mut self,
        backend: &mut ExactMetalTextRuntimeSession,
    ) -> Result<(), Box<dyn Error>> {
        if self.processed_prompt_tokens >= self.prompt_token_ids.len() {
            return Ok(());
        }
        let remaining_prompt_tokens = &self.prompt_token_ids[self.processed_prompt_tokens..];
        self.pending_next = Some(backend.prefill_prompt_greedy_token_id_from_token_ids(
            remaining_prompt_tokens,
            self.position,
        )?);
        self.processed_prompt_tokens += remaining_prompt_tokens.len();
        self.position += remaining_prompt_tokens.len();
        Ok(())
    }

    pub(crate) fn ensure_prompt_prefilled(&mut self) -> Result<(), Box<dyn Error>> {
        let backend_handle = Arc::clone(&self.backend);
        let mut backend = backend_handle
            .lock()
            .map_err(|_| "exact backend mutex poisoned".to_string())?;
        self.ensure_prompt_prefilled_locked(&mut backend)
    }

    fn snapshot(&self) -> ExactMetalGenerationSnapshot {
        ExactMetalGenerationSnapshot {
            generated_token_ids: Arc::<[u32]>::from(self.generated_token_ids.clone()),
            stop_reason: self.stop_reason,
            #[cfg(test)]
            processed_prompt_tokens: self.processed_prompt_tokens,
            #[cfg(test)]
            position: self.position,
            #[cfg(test)]
            has_pending_next: self.pending_next.is_some(),
        }
    }

    pub(crate) fn ensure_generated(
        &mut self,
        requested_count: usize,
    ) -> Result<(), Box<dyn Error>> {
        let target = requested_count.min(self.max_new_tokens);
        let backend_handle = Arc::clone(&self.backend);
        let mut backend = backend_handle
            .lock()
            .map_err(|_| "exact backend mutex poisoned".to_string())?;
        while self.generated_token_ids.len() < target {
            if self.stop_reason.is_some() {
                break;
            }
            if self.pending_next.is_none() {
                if self.processed_prompt_tokens < self.prompt_token_ids.len() {
                    self.ensure_prompt_prefilled_locked(&mut backend)?;
                } else if let Some(&last_generated) = self.generated_token_ids.last() {
                    let input_position = self
                        .position
                        .checked_sub(1)
                        .ok_or("generation cursor position underflow")?;
                    let remaining_target = target.saturating_sub(self.generated_token_ids.len());
                    let remaining_max = self
                        .max_new_tokens
                        .saturating_sub(self.generated_token_ids.len());
                    let chunk_len = remaining_target
                        .min(remaining_max)
                        .min(DEVICE_GREEDY_DECODE_CHUNK_TOKENS);
                    if chunk_len > 1 {
                        let chunk_tokens = Self::eval_token_chunk_with_backend(
                            &mut backend,
                            last_generated,
                            input_position,
                            chunk_len,
                        )?;
                        for token_id in chunk_tokens {
                            if self.stop_tokens.contains(&token_id) {
                                self.stop_reason =
                                    Some(ExactMetalGenerationStopReason::EosToken(token_id));
                                break;
                            }
                            self.generated_token_ids.push(token_id);
                            self.position += 1;
                            if self.generated_token_ids.len() >= self.max_new_tokens {
                                self.stop_reason =
                                    Some(ExactMetalGenerationStopReason::MaxNewTokens);
                                break;
                            }
                            if self.generated_token_ids.len() >= target {
                                break;
                            }
                        }
                        continue;
                    }
                    self.pending_next = Some(Self::eval_token_next_with_backend(
                        &mut backend,
                        last_generated,
                        input_position,
                    )?);
                }
            }
            let next_token = self
                .pending_next
                .take()
                .ok_or_else(|| "generation cursor missing pending next token".to_string())?;
            if self.stop_tokens.contains(&next_token) {
                self.stop_reason = Some(ExactMetalGenerationStopReason::EosToken(next_token));
                break;
            }
            self.generated_token_ids.push(next_token);
            self.position += 1;
            if self.generated_token_ids.len() >= self.max_new_tokens {
                self.stop_reason = Some(ExactMetalGenerationStopReason::MaxNewTokens);
                break;
            }
            if self.generated_token_ids.len() >= target {
                break;
            }
            self.pending_next = Some(Self::eval_token_next_with_backend(
                &mut backend,
                next_token,
                self.position - 1,
            )?);
        }
        if self.generated_token_ids.len() >= self.max_new_tokens && self.stop_reason.is_none() {
            self.stop_reason = Some(ExactMetalGenerationStopReason::MaxNewTokens);
            self.pending_next = None;
        }
        Ok(())
    }

    pub(crate) fn ensure_finished(&mut self) -> Result<(), Box<dyn Error>> {
        self.ensure_generated(self.max_new_tokens)
    }

    #[cfg(test)]
    pub(crate) fn generated_token_ids(&self) -> &[u32] {
        &self.generated_token_ids
    }

    #[cfg(test)]
    pub(crate) fn processed_prompt_tokens(&self) -> usize {
        self.processed_prompt_tokens
    }

    #[cfg(test)]
    pub(crate) fn position(&self) -> usize {
        self.position
    }

    #[cfg(test)]
    pub(crate) fn has_pending_next(&self) -> bool {
        self.pending_next.is_some()
    }
}

impl ExactMetalPromptPrefillNode {
    fn new(cursor: Arc<Mutex<ExactMetalGenerationCursor>>) -> Self {
        Self {
            cursor,
            value: OnceLock::new(),
        }
    }

    fn eval(&self) -> Result<Arc<ExactMetalGenerationSnapshot>, String> {
        self.value
            .get_or_init(|| {
                let mut cursor = self
                    .cursor
                    .lock()
                    .map_err(|_| "generation cursor mutex poisoned".to_string())?;
                cursor
                    .ensure_prompt_prefilled()
                    .map_err(|err| err.to_string())?;
                Ok(Arc::new(cursor.snapshot()))
            })
            .clone()
    }
}

impl ExactMetalGenerationStepNode {
    fn new(
        cursor: Arc<Mutex<ExactMetalGenerationCursor>>,
        target_count: usize,
        dependency: ExactMetalGenerationDependency,
    ) -> Self {
        Self {
            cursor,
            target_count,
            dependency,
            value: OnceLock::new(),
        }
    }

    fn eval(&self) -> Result<Arc<ExactMetalGenerationSnapshot>, String> {
        self.value
            .get_or_init(|| {
                match &self.dependency {
                    ExactMetalGenerationDependency::PromptPrefill(node) => {
                        node.eval()?;
                    }
                    ExactMetalGenerationDependency::Previous(node) => {
                        node.eval()?;
                    }
                }
                let mut cursor = self
                    .cursor
                    .lock()
                    .map_err(|_| "generation cursor mutex poisoned".to_string())?;
                cursor
                    .ensure_generated(self.target_count)
                    .map_err(|err| err.to_string())?;
                Ok(Arc::new(cursor.snapshot()))
            })
            .clone()
    }
}

impl ExactMetalGenerationGraph {
    fn new(cursor: ExactMetalGenerationCursor) -> Result<Self, Box<dyn Error>> {
        let max_new_tokens = cursor.max_new_tokens;
        let cursor = Arc::new(Mutex::new(cursor));
        Ok(Self {
            prompt_prefill: Arc::new(ExactMetalPromptPrefillNode::new(Arc::clone(&cursor))),
            cursor,
            step_nodes: Mutex::new(Vec::with_capacity(max_new_tokens)),
            final_snapshot: OnceLock::new(),
            max_new_tokens,
        })
    }

    fn step_node(
        &self,
        requested_count: usize,
    ) -> Result<Arc<ExactMetalGenerationStepNode>, String> {
        let target = requested_count.min(self.max_new_tokens);
        if target == 0 {
            return Err("generation step nodes start at token count 1".to_string());
        }
        let mut nodes = self
            .step_nodes
            .lock()
            .map_err(|_| "generation step-node mutex poisoned".to_string())?;
        while nodes.len() < target {
            let next_count = nodes.len() + 1;
            let dependency = if let Some(prev) = nodes.last() {
                ExactMetalGenerationDependency::Previous(Arc::clone(prev))
            } else {
                ExactMetalGenerationDependency::PromptPrefill(Arc::clone(&self.prompt_prefill))
            };
            nodes.push(Arc::new(ExactMetalGenerationStepNode::new(
                Arc::clone(&self.cursor),
                next_count,
                dependency,
            )));
        }
        nodes
            .get(target - 1)
            .cloned()
            .ok_or_else(|| format!("missing generation step node {target}"))
    }

    pub(crate) fn generated_token_ids_up_to(
        &self,
        requested_count: usize,
    ) -> Result<Arc<[u32]>, String> {
        let target = requested_count.min(self.max_new_tokens);
        if target == 0 {
            return Ok(Arc::<[u32]>::from(Vec::<u32>::new()));
        }
        Ok(self.step_node(target)?.eval()?.generated_token_ids.clone())
    }

    pub(crate) fn finish_snapshot(&self) -> Result<Arc<ExactMetalGenerationSnapshot>, String> {
        self.final_snapshot
            .get_or_init(|| {
                let mut cursor = self
                    .cursor
                    .lock()
                    .map_err(|_| "generation cursor mutex poisoned".to_string())?;
                cursor.ensure_finished().map_err(|err| err.to_string())?;
                Ok(Arc::new(cursor.snapshot()))
            })
            .clone()
    }
}

fn optional_private_weight_buffer(
    session: &mut LayerExecutionSession,
    enabled: bool,
    name: &str,
) -> Result<Option<MetalBuffer>, Box<dyn Error>> {
    if enabled {
        Ok(Some(session.private_weight_buffer(name)?))
    } else {
        Ok(None)
    }
}

fn create_bf16_buffer(
    runtime: &MetalRuntime,
    len_words: usize,
    storage: BufferStorageMode,
) -> Result<MetalBuffer, Box<dyn Error>> {
    Ok(runtime.create_buffer(len_words * size_of::<u16>(), storage)?)
}

fn compile_default_pipeline(
    runtime: &MetalRuntime,
    name: &str,
) -> Result<MetalPipeline, Box<dyn Error>> {
    compile_pipeline(runtime, name, 0)
}

fn compile_pipeline(
    runtime: &MetalRuntime,
    name: &str,
    smem_bytes: usize,
) -> Result<MetalPipeline, Box<dyn Error>> {
    Ok(runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: name.to_string(),
        base_name: name.to_string(),
        constants: Vec::new(),
        smem_bytes,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?)
}

fn read_bf16_buffer_bits(
    runtime: &MetalRuntime,
    buffer: &MetalBuffer,
    len_words: usize,
) -> Result<Vec<u32>, Box<dyn Error>> {
    Ok(
        runtime.with_readable_buffer(buffer, len_words * size_of::<u16>(), |bytes| {
            Ok(decode_bf16_buffer_bits(bytes))
        })?,
    )
}

fn bytes_of<T>(value: &T) -> &[u8] {
    unsafe { slice::from_raw_parts((value as *const T).cast::<u8>(), size_of::<T>()) }
}

fn bytes_from_bf16_words(words: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(words.len() * 2);
    for word in words {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out
}

fn read_f32_file_as_bf16_words(path: &Path) -> Result<Vec<u16>, Box<dyn Error>> {
    let bytes = fs::read(path)?;
    if bytes.len() % size_of::<f32>() != 0 {
        return Err(format!(
            "f32 input file {} length {} is not a multiple of {}",
            path.display(),
            bytes.len(),
            size_of::<f32>()
        )
        .into());
    }
    let mut words = Vec::with_capacity(bytes.len() / size_of::<f32>());
    for chunk in bytes.chunks_exact(size_of::<f32>()) {
        let value = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        words.push((bf16_round_to_f32(value).to_bits() >> 16) as u16);
    }
    Ok(words)
}

fn write_bf16_words_as_f32_file(path: &Path, words: &[u16]) -> Result<(), Box<dyn Error>> {
    let mut bytes = Vec::with_capacity(words.len() * size_of::<f32>());
    for &word in words {
        bytes.extend_from_slice(&bf16_word_to_f32(word).to_le_bytes());
    }
    fs::write(path, bytes)?;
    Ok(())
}

fn bf16_words_from_f32_bits(bits: &[u32]) -> Vec<u16> {
    bits.iter()
        .copied()
        .map(|bits| (bits >> 16) as u16)
        .collect()
}

fn bf16_word_to_f32(word: u16) -> f32 {
    f32::from_bits((word as u32) << 16)
}

fn bf16_round_to_f32(value: f32) -> f32 {
    let bits = value.to_bits();
    let lsb = (bits >> 16) & 1;
    let rounded = bits.wrapping_add(0x7FFF + lsb) & 0xFFFF_0000;
    f32::from_bits(rounded)
}

fn decode_bf16_buffer_bits(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .map(bf16_word_to_f32)
        .map(f32::to_bits)
        .collect()
}

fn decode_u32_buffer_words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(size_of::<u32>())
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn read_router_output_from_device(
    runtime: &MetalRuntime,
    router_scaled_out: &MetalBuffer,
    router_proj_out: &MetalBuffer,
    router_probs_out: &MetalBuffer,
    moe_top_k_indices: &MetalBuffer,
    moe_top_k_weights: &MetalBuffer,
    router_scaled_len: usize,
    expert_count: usize,
    top_k: usize,
) -> Result<Layer0CachedRouterOutput, Box<dyn Error>> {
    Ok(Layer0CachedRouterOutput {
        router_scaled_bits: decode_bf16_buffer_bits(
            &runtime.read_buffer(router_scaled_out, router_scaled_len * size_of::<u16>())?,
        ),
        expert_scores_bits: decode_bf16_buffer_bits(
            &runtime.read_buffer(router_proj_out, expert_count * size_of::<u16>())?,
        ),
        router_probs_bits: decode_bf16_buffer_bits(
            &runtime.read_buffer(router_probs_out, expert_count * size_of::<u16>())?,
        ),
        top_k_indices: decode_u32_buffer_words(
            &runtime.read_buffer(moe_top_k_indices, top_k * size_of::<u32>())?,
        ),
        top_k_weights_bits: decode_bf16_buffer_bits(
            &runtime.read_buffer(moe_top_k_weights, top_k * size_of::<u16>())?,
        ),
    })
}

fn bits_to_f32(bits: &[u32]) -> Vec<f32> {
    bits.iter().copied().map(f32::from_bits).collect()
}

fn flatten_heads_to_tensor(
    bits: &[u32],
    head_count: usize,
    head_dim: usize,
) -> Result<KvTensor<f32>, Box<dyn Error>> {
    let shape = KvTensorShape {
        batch_size: 1,
        kv_head_count: head_count,
        seq_len: 1,
        head_dim,
    };
    KvTensor::from_vec(shape, bits_to_f32(bits)).map_err(|err| err.into())
}

fn attention_prob_bits_from_logits(
    logits_bits: &[u32],
    q_head_count: usize,
    seq_len: usize,
) -> Vec<u32> {
    let mut prob_bits = Vec::with_capacity(q_head_count * seq_len);
    for q_head in 0..q_head_count {
        let row_start = q_head * seq_len;
        let row = &logits_bits[row_start..row_start + seq_len];
        let max_score = row
            .iter()
            .copied()
            .map(f32::from_bits)
            .fold(f32::NEG_INFINITY, f32::max);
        let exp_scores = row
            .iter()
            .copied()
            .map(f32::from_bits)
            .map(|score| (score - max_score).exp())
            .collect::<Vec<_>>();
        let exp_sum = exp_scores.iter().copied().sum::<f32>();
        for value in exp_scores {
            prob_bits.push(bf16_round_to_f32(value / exp_sum).to_bits());
        }
    }
    prob_bits
}

fn read_exact_kv_cache_tensor_bits(
    runtime: &MetalRuntime,
    cache: &ExactMetalKvCache,
    buffer: &MetalBuffer,
) -> Result<Vec<u32>, Box<dyn Error>> {
    let row_stride_words = cache.row_stride_words()?;
    let storage_words = cache
        .spec
        .batch_size
        .checked_mul(cache.spec.kv_head_count)
        .and_then(|value| value.checked_mul(row_stride_words))
        .ok_or("exact metal KV cache readback overflow")?;
    let storage_bits = read_bf16_buffer_bits(runtime, buffer, storage_words)?;
    let mut out = Vec::with_capacity(
        cache
            .spec
            .batch_size
            .checked_mul(cache.spec.kv_head_count)
            .and_then(|value| value.checked_mul(cache.stored_tokens))
            .and_then(|value| value.checked_mul(cache.spec.head_dim))
            .ok_or("exact metal KV tensor compact size overflow")?,
    );
    let start_slot = cache.start_slot();
    for batch in 0..cache.spec.batch_size {
        for head in 0..cache.spec.kv_head_count {
            let row_base = (batch * cache.spec.kv_head_count + head)
                .checked_mul(row_stride_words)
                .ok_or("exact metal KV compact row base overflow")?;
            for token in 0..cache.stored_tokens {
                let slot = (start_slot + token) % cache.spec.max_tokens;
                let token_base = row_base
                    .checked_add(slot * cache.spec.head_dim)
                    .ok_or("exact metal KV compact token base overflow")?;
                out.extend_from_slice(&storage_bits[token_base..token_base + cache.spec.head_dim]);
            }
        }
    }
    Ok(out)
}

fn read_attention_logits_bits(
    runtime: &MetalRuntime,
    buffer: &MetalBuffer,
    q_head_count: usize,
    seq_len: usize,
    row_stride_words: usize,
) -> Result<Vec<u32>, Box<dyn Error>> {
    let storage_words = q_head_count
        .checked_mul(row_stride_words)
        .ok_or("attention logits storage overflow")?;
    let storage_bits = read_bf16_buffer_bits(runtime, buffer, storage_words)?;
    let mut out = Vec::with_capacity(
        q_head_count
            .checked_mul(seq_len)
            .ok_or("attention logits compact size overflow")?,
    );
    for q_head in 0..q_head_count {
        let row_base = q_head
            .checked_mul(row_stride_words)
            .ok_or("attention logits row base overflow")?;
        out.extend_from_slice(&storage_bits[row_base..row_base + seq_len]);
    }
    Ok(out)
}

fn mlx_softmax_threads_per_threadgroup(
    seq_len: usize,
    max_threads_per_threadgroup: u64,
) -> Result<MetalSize, Box<dyn Error>> {
    const MLX_SOFTMAX_N_READS: usize = 4;
    const MLX_SIMD_WIDTH: usize = 32;

    let threadgroup_needed = seq_len.max(1).div_ceil(MLX_SOFTMAX_N_READS);
    let simds_needed = threadgroup_needed.div_ceil(MLX_SIMD_WIDTH).max(1);
    let threadgroup_width = u64::try_from(
        MLX_SIMD_WIDTH
            .checked_mul(simds_needed)
            .ok_or("softmax threadgroup size overflow")?,
    )?;
    if threadgroup_width > max_threads_per_threadgroup {
        return Err(format!(
            "softmax threadgroup width {} exceeds pipeline max {} for seq_len {}",
            threadgroup_width, max_threads_per_threadgroup, seq_len
        )
        .into());
    }
    Ok(MetalSize {
        width: threadgroup_width,
        height: 1,
        depth: 1,
    })
}

fn compute_cached_attention_metal(
    runtime: &MetalRuntime,
    logits_pipeline: &MetalPipeline,
    softmax_pipeline: &MetalPipeline,
    weighted_sum_pipeline: &MetalPipeline,
    q_buffer: &MetalBuffer,
    cache: &ExactMetalKvCache,
    q_head_count: usize,
    q_heads_per_kv: usize,
    head_dim: usize,
    logits_buffer: &MetalBuffer,
    probs_buffer: &MetalBuffer,
    out_buffer: &MetalBuffer,
) -> Result<(Vec<u32>, Vec<u32>, Vec<u32>), Box<dyn Error>> {
    let seq_len = cache.seq_len();
    let kv_row_stride = cache.row_stride_words()?;
    let logits_row_stride = cache.spec.max_tokens;
    let logits_args = MlxGqaAttentionLogitsSeqArgs {
        head_dim: head_dim as u32,
        q_head_stride: head_dim as u32,
        kv_row_stride: kv_row_stride as u32,
        q_head_count: q_head_count as u32,
        q_heads_per_kv: q_heads_per_kv as u32,
        seq_len: seq_len as u32,
        start_slot: cache.start_slot() as u32,
        capacity: cache.spec.max_tokens as u32,
    };
    let softmax_args = MlxSoftmaxRowsArgs {
        row_stride: logits_row_stride as u32,
        row_count: q_head_count as u32,
        seq_len: seq_len as u32,
    };
    let weighted_sum_args = MlxGqaAttentionWeightedSumArgs {
        probs_row_stride: logits_row_stride as u32,
        head_dim: head_dim as u32,
        kv_row_stride: kv_row_stride as u32,
        out_head_stride: head_dim as u32,
        q_head_count: q_head_count as u32,
        q_heads_per_kv: q_heads_per_kv as u32,
        seq_len: seq_len as u32,
        start_slot: cache.start_slot() as u32,
        capacity: cache.spec.max_tokens as u32,
    };
    let threadgroups_logits = MetalSize {
        width: seq_len as u64,
        height: q_head_count as u64,
        depth: 1,
    };
    let softmax_threads_per_threadgroup =
        mlx_softmax_threads_per_threadgroup(seq_len, softmax_pipeline.max_threads_per_threadgroup)?;
    let threadgroups_output = MetalSize {
        width: (head_dim as u64).div_ceil(64),
        height: 1,
        depth: q_head_count as u64,
    };
    let logits_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 1,
        depth: 1,
    };
    let output_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 4,
        depth: 1,
    };

    runtime.begin_command_batch()?;
    runtime.dispatch_compute(
        logits_pipeline,
        bytes_of(&logits_args),
        &[
            MetalBufferBindingRef {
                index: 1,
                buffer: q_buffer,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: &cache.key_buffer,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: logits_buffer,
                offset_bytes: 0,
            },
        ],
        &[],
        threadgroups_logits,
        logits_threads_per_threadgroup,
    )?;
    runtime.memory_barrier_buffers()?;
    runtime.dispatch_compute(
        softmax_pipeline,
        bytes_of(&softmax_args),
        &[
            MetalBufferBindingRef {
                index: 1,
                buffer: logits_buffer,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: probs_buffer,
                offset_bytes: 0,
            },
        ],
        &[],
        MetalSize {
            width: q_head_count as u64,
            height: 1,
            depth: 1,
        },
        softmax_threads_per_threadgroup,
    )?;
    runtime.memory_barrier_buffers()?;
    runtime.dispatch_compute(
        weighted_sum_pipeline,
        bytes_of(&weighted_sum_args),
        &[
            MetalBufferBindingRef {
                index: 1,
                buffer: probs_buffer,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: &cache.value_buffer,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: out_buffer,
                offset_bytes: 0,
            },
        ],
        &[],
        threadgroups_output,
        output_threads_per_threadgroup,
    )?;
    runtime.end_command_batch()?;
    runtime.wait_idle()?;

    let logits_bits = read_attention_logits_bits(
        runtime,
        logits_buffer,
        q_head_count,
        seq_len,
        logits_row_stride,
    )?;
    let prob_bits = read_attention_logits_bits(
        runtime,
        probs_buffer,
        q_head_count,
        seq_len,
        logits_row_stride,
    )?;
    let out_bits = read_bf16_buffer_bits(runtime, out_buffer, q_head_count * head_dim)?;
    Ok((logits_bits, prob_bits, out_bits))
}

fn moe_weighted_expert_out_bits(
    down_bits: &[u32],
    top_k_weights_bits: &[u32],
    hidden: usize,
) -> Result<Vec<u32>, Box<dyn Error>> {
    if top_k_weights_bits.len() != ROUTER_TOP_K {
        return Err(format!(
            "expected {} routed expert weights, got {}",
            ROUTER_TOP_K,
            top_k_weights_bits.len()
        )
        .into());
    }
    let expected_len = ROUTER_TOP_K
        .checked_mul(hidden)
        .ok_or("expert_out size overflow")?;
    if down_bits.len() != expected_len {
        return Err(format!(
            "expert_down length mismatch: got {} expected {}",
            down_bits.len(),
            expected_len
        )
        .into());
    }

    let mut out = Vec::with_capacity(hidden);
    for hidden_index in 0..hidden {
        let mut acc = 0.0f32;
        for (expert_slot, &weight_bits) in top_k_weights_bits.iter().enumerate() {
            let down = f32::from_bits(down_bits[expert_slot * hidden + hidden_index]);
            let weight = f32::from_bits(weight_bits);
            let weighted = bf16_round_to_f32(down * weight);
            acc = bf16_round_to_f32(acc + weighted);
        }
        out.push(acc.to_bits());
    }
    Ok(out)
}

fn validate_hash_and_prefix<const N: usize>(
    label: &str,
    bits: &[u32],
    expected_hash: u64,
    expected_prefix: &[u32; N],
) -> Result<(), Box<dyn Error>> {
    let hash = fnv1a64_u32_words(bits);
    let prefix = &bits[..bits.len().min(N)];
    if expected_hash != 0 && hash != expected_hash {
        return Err(format!(
            "{label} hash mismatch: got 0x{hash:016X} expected 0x{expected_hash:016X}"
        )
        .into());
    }
    if expected_prefix.iter().any(|word| *word != 0) && prefix != expected_prefix {
        return Err(format!("{label} prefix mismatch").into());
    }
    Ok(())
}

fn print_prefix(label: &str, bits: &[u32], count: usize) {
    print!("{label}=");
    for (index, bit) in bits.iter().take(count).enumerate() {
        if index != 0 {
            print!(",");
        }
        print!("0x{bit:08X}");
    }
    println!();
}

fn print_first16(label: &str, bits: &[u32]) {
    print_prefix(label, bits, 16);
}

fn print_cached_artifacts(artifacts: &Layer0CachedArtifacts) {
    println!("backend={}", artifacts.backend_name);
    println!("model_path={}", artifacts.model_path.display());
    println!("prefill_rope_offset={}", artifacts.prefill_rope_offset);
    println!("decode_rope_offset={}", artifacts.decode_rope_offset);
    println!("prefill_activation_phase={PREFILL_ACTIVATION_PHASE}");
    println!("decode_activation_phase={DECODE_ACTIVATION_PHASE}");
    println!("q_head_count={}", artifacts.q_head_count);
    println!("k_head_count={}", artifacts.k_head_count);
    println!("v_head_count={}", artifacts.v_head_count);
    println!("q_heads_per_kv={}", artifacts.q_heads_per_kv);
    println!("head_dim={}", artifacts.head_dim);
    println!("stage={}", artifacts.stage_name());
    println!(
        "prefill_k_cache_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.prefill_k_bits)
    );
    println!(
        "prefill_v_proj_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.prefill_v_proj_bits)
    );
    println!(
        "prefill_v_cache_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.prefill_v_bits)
    );
    println!(
        "decode_q_rope_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.decode_q_bits)
    );
    println!(
        "decode_k_rope_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.decode_k_bits)
    );
    println!(
        "decode_v_proj_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.decode_v_proj_bits)
    );
    println!(
        "decode_v_norm_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.decode_v_bits)
    );
    println!(
        "full_k_cache_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.full_k_bits)
    );
    println!(
        "full_v_cache_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.full_v_bits)
    );
    println!(
        "attention_scores_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.attention_score_bits)
    );
    println!(
        "attention_probs_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.attention_prob_bits)
    );
    println!(
        "attention_output_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.attention_out_bits)
    );
    if let Some(bits) = &artifacts.attention_oproj_bits {
        println!("attention_oproj_fnv1a64=0x{:016X}", fnv1a64_u32_words(bits));
    }
    if let Some(bits) = &artifacts.post_attention_norm_bits {
        println!(
            "attention_post_attn_norm_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.post_attention_residual_bits {
        println!(
            "attention_post_attn_residual_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.pre_feedforward_norm_bits {
        println!(
            "attention_pre_ffn_norm_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.dense_gate_bits {
        println!(
            "attention_pre_ffn_gate_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.dense_up_bits {
        println!(
            "attention_pre_ffn_up_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.dense_geglu_bits {
        println!(
            "attention_pre_ffn_geglu_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.dense_down_bits {
        println!(
            "attention_pre_ffn_down_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(router_output) = &artifacts.router_output {
        println!(
            "router_scaled_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(&router_output.router_scaled_bits)
        );
        println!(
            "expert_scores_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(&router_output.expert_scores_bits)
        );
        println!(
            "router_probs_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(&router_output.router_probs_bits)
        );
        println!(
            "router_topk_indices_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(&router_output.top_k_indices)
        );
        println!(
            "router_topk_weights_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(&router_output.top_k_weights_bits)
        );
    }
    if let Some(bits) = &artifacts.moe_expert_gate_bits {
        println!(
            "attention_moe_expert_gate_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.moe_expert_up_bits {
        println!(
            "attention_moe_expert_up_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.moe_expert_geglu_bits {
        println!(
            "attention_moe_expert_geglu_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.moe_expert_down_bits {
        println!(
            "attention_moe_expert_down_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.post_ffn_norm1_bits {
        println!(
            "attention_post_ffn_norm1_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.moe_expert_out_bits {
        println!(
            "attention_moe_expert_out_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.moe_post_ffn_norm2_bits {
        println!(
            "attention_moe_post_ffn_norm2_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.moe_merge_bits {
        println!(
            "attention_moe_merge_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.post_ffn_residual_bits {
        println!(
            "attention_post_ffn_residual_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    print_first16(
        "prefill_k_cache_first16_f32_bits",
        &artifacts.prefill_k_bits,
    );
    print_first16(
        "prefill_v_proj_first16_f32_bits",
        &artifacts.prefill_v_proj_bits,
    );
    print_first16(
        "prefill_v_cache_first16_f32_bits",
        &artifacts.prefill_v_bits,
    );
    print_first16("decode_q_rope_first16_f32_bits", &artifacts.decode_q_bits);
    print_first16("decode_k_rope_first16_f32_bits", &artifacts.decode_k_bits);
    print_first16(
        "decode_v_proj_first16_f32_bits",
        &artifacts.decode_v_proj_bits,
    );
    print_first16("decode_v_norm_first16_f32_bits", &artifacts.decode_v_bits);
    print_first16("full_k_cache_first16_f32_bits", &artifacts.full_k_bits);
    print_first16("full_v_cache_first16_f32_bits", &artifacts.full_v_bits);
    print_first16(
        "attention_scores_first16_f32_bits",
        &artifacts.attention_score_bits,
    );
    print_first16(
        "attention_probs_first16_f32_bits",
        &artifacts.attention_prob_bits,
    );
    print_first16(
        "attention_output_first16_f32_bits",
        &artifacts.attention_out_bits,
    );
    if let Some(bits) = &artifacts.attention_oproj_bits {
        print_first16("attention_oproj_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.post_attention_norm_bits {
        print_first16("attention_post_attn_norm_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.post_attention_residual_bits {
        print_first16("attention_post_attn_residual_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.pre_feedforward_norm_bits {
        print_first16("attention_pre_ffn_norm_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.dense_gate_bits {
        print_first16("attention_pre_ffn_gate_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.dense_up_bits {
        print_first16("attention_pre_ffn_up_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.dense_geglu_bits {
        print_first16("attention_pre_ffn_geglu_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.dense_down_bits {
        print_first16("attention_pre_ffn_down_first16_f32_bits", bits);
    }
    if let Some(router_output) = &artifacts.router_output {
        print_first16(
            "router_scaled_first16_f32_bits",
            &router_output.router_scaled_bits,
        );
        print_first16(
            "expert_scores_first16_f32_bits",
            &router_output.expert_scores_bits,
        );
        print_first16(
            "router_probs_first16_f32_bits",
            &router_output.router_probs_bits,
        );
        print!("top_k_indices=");
        for (index, value) in router_output.top_k_indices.iter().enumerate() {
            if index != 0 {
                print!(",");
            }
            print!("{value}");
        }
        println!();
        print_prefix(
            "top_k_weights_first8_f32_bits",
            &router_output.top_k_weights_bits,
            ROUTER_TOP_K,
        );
    }
    if let Some(bits) = &artifacts.moe_expert_gate_bits {
        print_first16("attention_moe_expert_gate_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.moe_expert_up_bits {
        print_first16("attention_moe_expert_up_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.moe_expert_geglu_bits {
        print_first16("attention_moe_expert_geglu_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.moe_expert_down_bits {
        print_first16("attention_moe_expert_down_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.post_ffn_norm1_bits {
        print_first16("attention_post_ffn_norm1_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.moe_expert_out_bits {
        print_first16("attention_moe_expert_out_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.moe_post_ffn_norm2_bits {
        print_first16("attention_moe_post_ffn_norm2_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.moe_merge_bits {
        print_first16("attention_moe_merge_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.post_ffn_residual_bits {
        print_first16("attention_post_ffn_residual_first16_f32_bits", bits);
    }
    println!("status=ok");
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Layer0CachedStage {
    AttentionOproj = 0,
    PostAttentionResidual = 1,
    PreFeedforwardNorm = 2,
    DenseGate = 3,
    DenseUp = 4,
    DenseGeGlu = 5,
    DenseDown = 6,
    PostFfnNorm1 = 7,
    Router = 8,
    MoeExpertGate = 9,
    MoeExpertUp = 10,
    MoeExpertGeGlu = 11,
    MoeExpertDown = 12,
    MoeExpertOut = 13,
    MoePostFfnNorm2 = 14,
    MoeMerge = 15,
    PostFfnResidual = 16,
}

impl Layer0CachedStage {
    const fn bit(self) -> u32 {
        1u32 << (self as u8)
    }

    pub const fn cli_flag(self) -> &'static str {
        match self {
            Self::AttentionOproj => "--oproj",
            Self::PostAttentionResidual => "--residual",
            Self::PreFeedforwardNorm => "--pre-ffn-norm",
            Self::DenseGate => "--dense-gate",
            Self::DenseUp => "--dense-up",
            Self::DenseGeGlu => "--dense-geglu",
            Self::DenseDown => "--dense-down",
            Self::PostFfnNorm1 => "--post-ffn-norm1",
            Self::Router => "--router",
            Self::MoeExpertGate => "--moe-expert-gate",
            Self::MoeExpertUp => "--moe-expert-up",
            Self::MoeExpertGeGlu => "--moe-expert-geglu",
            Self::MoeExpertDown => "--moe-expert-down",
            Self::MoeExpertOut => "--moe-expert-out",
            Self::MoePostFfnNorm2 => "--moe-post-ffn-norm2",
            Self::MoeMerge => "--moe-merge",
            Self::PostFfnResidual => "--post-ffn-residual",
        }
    }

    pub const fn stage_name(self) -> &'static str {
        match self {
            Self::AttentionOproj => "attention_oproj_cached",
            Self::PostAttentionResidual => "attention_post_attn_residual_cached",
            Self::PreFeedforwardNorm => "attention_pre_ffn_norm_cached",
            Self::DenseGate => "attention_pre_ffn_gate_cached",
            Self::DenseUp => "attention_pre_ffn_up_cached",
            Self::DenseGeGlu => "attention_pre_ffn_geglu_cached",
            Self::DenseDown => "attention_pre_ffn_down_cached",
            Self::PostFfnNorm1 => "attention_post_ffn_norm1_cached",
            Self::Router => "attention_router_cached",
            Self::MoeExpertGate => "attention_moe_expert_gate_cached",
            Self::MoeExpertUp => "attention_moe_expert_up_cached",
            Self::MoeExpertGeGlu => "attention_moe_expert_geglu_cached",
            Self::MoeExpertDown => "attention_moe_expert_down_cached",
            Self::MoeExpertOut => "attention_moe_expert_out_cached",
            Self::MoePostFfnNorm2 => "attention_moe_post_ffn_norm2_cached",
            Self::MoeMerge => "attention_moe_merge_cached",
            Self::PostFfnResidual => "attention_post_ffn_residual_cached",
        }
    }

    pub fn from_cli_flag(flag: &str) -> Option<Self> {
        match flag {
            "--oproj" => Some(Self::AttentionOproj),
            "--residual" => Some(Self::PostAttentionResidual),
            "--pre-ffn-norm" => Some(Self::PreFeedforwardNorm),
            "--dense-gate" => Some(Self::DenseGate),
            "--dense-up" => Some(Self::DenseUp),
            "--dense-geglu" => Some(Self::DenseGeGlu),
            "--dense-down" => Some(Self::DenseDown),
            "--post-ffn-norm1" => Some(Self::PostFfnNorm1),
            "--router" => Some(Self::Router),
            "--moe-expert-gate" => Some(Self::MoeExpertGate),
            "--moe-expert-up" => Some(Self::MoeExpertUp),
            "--moe-expert-geglu" => Some(Self::MoeExpertGeGlu),
            "--moe-expert-down" => Some(Self::MoeExpertDown),
            "--moe-expert-out" => Some(Self::MoeExpertOut),
            "--moe-post-ffn-norm2" => Some(Self::MoePostFfnNorm2),
            "--moe-merge" => Some(Self::MoeMerge),
            "--post-ffn-residual" | "--layer-output" => Some(Self::PostFfnResidual),
            _ => None,
        }
    }
}

const LAYER0_CACHED_EVALUATION_ORDER: [Layer0CachedStage; 17] = [
    Layer0CachedStage::AttentionOproj,
    Layer0CachedStage::PostAttentionResidual,
    Layer0CachedStage::PreFeedforwardNorm,
    Layer0CachedStage::DenseGate,
    Layer0CachedStage::DenseUp,
    Layer0CachedStage::DenseGeGlu,
    Layer0CachedStage::DenseDown,
    Layer0CachedStage::PostFfnNorm1,
    Layer0CachedStage::Router,
    Layer0CachedStage::MoeExpertGate,
    Layer0CachedStage::MoeExpertUp,
    Layer0CachedStage::MoeExpertGeGlu,
    Layer0CachedStage::MoeExpertDown,
    Layer0CachedStage::MoeExpertOut,
    Layer0CachedStage::MoePostFfnNorm2,
    Layer0CachedStage::MoeMerge,
    Layer0CachedStage::PostFfnResidual,
];

const LAYER0_CACHED_DISPLAY_ORDER: [Layer0CachedStage; 17] = [
    Layer0CachedStage::PostFfnResidual,
    Layer0CachedStage::MoeMerge,
    Layer0CachedStage::MoePostFfnNorm2,
    Layer0CachedStage::MoeExpertOut,
    Layer0CachedStage::PostFfnNorm1,
    Layer0CachedStage::DenseDown,
    Layer0CachedStage::MoeExpertDown,
    Layer0CachedStage::MoeExpertGeGlu,
    Layer0CachedStage::MoeExpertUp,
    Layer0CachedStage::MoeExpertGate,
    Layer0CachedStage::DenseGeGlu,
    Layer0CachedStage::DenseUp,
    Layer0CachedStage::DenseGate,
    Layer0CachedStage::Router,
    Layer0CachedStage::PreFeedforwardNorm,
    Layer0CachedStage::PostAttentionResidual,
    Layer0CachedStage::AttentionOproj,
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Layer0CachedPlan {
    mask: u32,
}

impl Layer0CachedPlan {
    pub const fn new() -> Self {
        Self { mask: 0 }
    }

    pub const fn is_empty(self) -> bool {
        self.mask == 0
    }

    pub const fn requires(self, stage: Layer0CachedStage) -> bool {
        (self.mask & stage.bit()) != 0
    }

    pub fn require_stage(&mut self, stage: Layer0CachedStage) {
        match stage {
            Layer0CachedStage::AttentionOproj => {}
            Layer0CachedStage::PostAttentionResidual => {
                self.require_stage(Layer0CachedStage::AttentionOproj);
            }
            Layer0CachedStage::PreFeedforwardNorm => {
                self.require_stage(Layer0CachedStage::PostAttentionResidual);
            }
            Layer0CachedStage::DenseGate | Layer0CachedStage::DenseUp => {
                self.require_stage(Layer0CachedStage::PreFeedforwardNorm);
            }
            Layer0CachedStage::DenseGeGlu => {
                self.require_stage(Layer0CachedStage::DenseGate);
                self.require_stage(Layer0CachedStage::DenseUp);
            }
            Layer0CachedStage::DenseDown => {
                self.require_stage(Layer0CachedStage::DenseGeGlu);
            }
            Layer0CachedStage::PostFfnNorm1 => {
                self.require_stage(Layer0CachedStage::DenseDown);
            }
            Layer0CachedStage::Router => {
                self.require_stage(Layer0CachedStage::PostAttentionResidual);
            }
            Layer0CachedStage::MoeExpertGate => {
                self.require_stage(Layer0CachedStage::Router);
            }
            Layer0CachedStage::MoeExpertUp => {
                self.require_stage(Layer0CachedStage::MoeExpertGate);
            }
            Layer0CachedStage::MoeExpertGeGlu => {
                self.require_stage(Layer0CachedStage::MoeExpertUp);
            }
            Layer0CachedStage::MoeExpertDown => {
                self.require_stage(Layer0CachedStage::MoeExpertGeGlu);
            }
            Layer0CachedStage::MoeExpertOut => {
                self.require_stage(Layer0CachedStage::MoeExpertDown);
            }
            Layer0CachedStage::MoePostFfnNorm2 => {
                self.require_stage(Layer0CachedStage::MoeExpertOut);
            }
            Layer0CachedStage::MoeMerge => {
                self.require_stage(Layer0CachedStage::PostFfnNorm1);
                self.require_stage(Layer0CachedStage::MoePostFfnNorm2);
            }
            Layer0CachedStage::PostFfnResidual => {
                self.require_stage(Layer0CachedStage::MoeMerge);
            }
        }
        self.mask |= stage.bit();
    }

    pub fn evaluation_order(self) -> Vec<Layer0CachedStage> {
        LAYER0_CACHED_EVALUATION_ORDER
            .iter()
            .copied()
            .filter(|stage| self.requires(*stage))
            .collect()
    }

    pub fn display_stage(self) -> Option<Layer0CachedStage> {
        LAYER0_CACHED_DISPLAY_ORDER
            .iter()
            .copied()
            .find(|stage| self.requires(*stage))
    }
}

