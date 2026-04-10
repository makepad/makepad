    let mut out = MetalRuntimeCounters::default();
    for stage in stages {
        out.command_batches_begun = out
            .command_batches_begun
            .saturating_add(stage.counters.command_batches_begun);
        out.command_batches_committed = out
            .command_batches_committed
            .saturating_add(stage.counters.command_batches_committed);
        out.command_buffer_commits = out
            .command_buffer_commits
            .saturating_add(stage.counters.command_buffer_commits);
        out.compute_encoder_starts = out
            .compute_encoder_starts
            .saturating_add(stage.counters.compute_encoder_starts);
        out.compute_encoder_ends = out
            .compute_encoder_ends
            .saturating_add(stage.counters.compute_encoder_ends);
        out.compute_dispatches = out
            .compute_dispatches
            .saturating_add(stage.counters.compute_dispatches);
        out.buffer_barriers = out
            .buffer_barriers
            .saturating_add(stage.counters.buffer_barriers);
        out.blit_copy_calls = out
            .blit_copy_calls
            .saturating_add(stage.counters.blit_copy_calls);
        out.fence_waits = out.fence_waits.saturating_add(stage.counters.fence_waits);
        out.fence_updates = out
            .fence_updates
            .saturating_add(stage.counters.fence_updates);
        out.wait_idle_calls = out
            .wait_idle_calls
            .saturating_add(stage.counters.wait_idle_calls);
        out.completion_wait_calls = out
            .completion_wait_calls
            .saturating_add(stage.counters.completion_wait_calls);
        out.readback_calls = out
            .readback_calls
            .saturating_add(stage.counters.readback_calls);
        out.gpu_elapsed_ns = out
            .gpu_elapsed_ns
            .saturating_add(stage.counters.gpu_elapsed_ns);
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExactMetalGenerationStopReason {
    MaxNewTokens,
    EosToken(u32),
}

pub(crate) struct ExactMetalGenerationCursor {
    backend: Arc<Mutex<ExactMetalTextRuntimeSession>>,
    prompt_token_ids: Arc<[u32]>,
    stop_tokens: BTreeSet<u32>,
    max_new_tokens: usize,
    processed_prompt_tokens: usize,
    position: usize,
    pending_next: Option<u32>,
    generated_token_ids: Vec<u32>,
    stop_reason: Option<ExactMetalGenerationStopReason>,
}

#[derive(Clone)]
pub(crate) struct ExactMetalGenerationSnapshot {
    pub(crate) generated_token_ids: Arc<[u32]>,
    pub(crate) stop_reason: Option<ExactMetalGenerationStopReason>,
    #[cfg(test)]
    pub(crate) processed_prompt_tokens: usize,
    #[cfg(test)]
    pub(crate) position: usize,
    #[cfg(test)]
    pub(crate) has_pending_next: bool,
}

pub(crate) struct ExactMetalPromptPrefillNode {
    cursor: Arc<Mutex<ExactMetalGenerationCursor>>,
    value: OnceLock<Result<Arc<ExactMetalGenerationSnapshot>, String>>,
}

enum ExactMetalGenerationDependency {
    PromptPrefill(Arc<ExactMetalPromptPrefillNode>),
    Previous(Arc<ExactMetalGenerationStepNode>),
}

pub(crate) struct ExactMetalGenerationStepNode {
    cursor: Arc<Mutex<ExactMetalGenerationCursor>>,
    target_count: usize,
    dependency: ExactMetalGenerationDependency,
    value: OnceLock<Result<Arc<ExactMetalGenerationSnapshot>, String>>,
}

pub(crate) struct ExactMetalGenerationGraph {
    cursor: Arc<Mutex<ExactMetalGenerationCursor>>,
    prompt_prefill: Arc<ExactMetalPromptPrefillNode>,
    step_nodes: Mutex<Vec<Arc<ExactMetalGenerationStepNode>>>,
    final_snapshot: OnceLock<Result<Arc<ExactMetalGenerationSnapshot>, String>>,
    max_new_tokens: usize,
}

impl ExactMetalTextRuntimeSession {
    pub(crate) fn load(model_path: PathBuf) -> Result<Self, Box<dyn Error>> {
        let mut session = LayerExecutionSession::load(model_path)?;
        let text_io = ExactMetalTextIoWorkspace::load(&mut session)?;
        let kv_append_pipeline =
            compile_default_pipeline(&session.runtime, "kernel_mlx_kv_append_pair_bf16")?;
        let kv_layout =
            GemmaKvCacheLayout::from_text_config(&session.weights.snapshot.config.text_config, 1)?;
        let mut kv_caches = Vec::with_capacity(kv_layout.cache_specs.len());
        for spec in &kv_layout.cache_specs {
            kv_caches.push(RefCell::new(ExactMetalKvCache::load(
                &session.runtime,
                spec.clone(),
            )?));
        }
        Ok(Self {
            session,
            kv_layout,
            kv_caches,
            kv_append_pipeline,
            text_io,
            layer_workspaces: HashMap::new(),
        })
    }

    pub(crate) fn reset_kv_caches(&mut self) {
        for cache in &self.kv_caches {
            cache.borrow_mut().reset();
        }
    }

    pub(crate) fn reset_runtime_counters(&self) {
        self.session.runtime.reset_counters();
    }

    pub(crate) fn runtime_counters(&self) -> MetalRuntimeCounters {
        self.session.runtime.counters()
    }

    fn profile_runtime_stage<F>(
        &mut self,
        stage_name: &'static str,
        f: F,
    ) -> Result<ExactMetalStageProfile, Box<dyn Error>>
    where
        F: FnOnce(&mut Self) -> Result<(), Box<dyn Error>>,
    {
        let runtime = self.session.runtime.clone();
        runtime.reset_counters();
        let started = Instant::now();
        f(self)?;
        runtime.wait_idle()?;
        Ok(ExactMetalStageProfile {
            stage_name,
            elapsed: started.elapsed(),
            counters: runtime.counters(),
        })
    }

    fn profile_decode_step_from_token_id(
        &mut self,
        token_id: u32,
        position: usize,
        prompt_token_count: usize,
        first_generated_token_id: u32,
    ) -> Result<ExactMetalDecodeStepProfile, Box<dyn Error>> {
        let layer_count = self
            .session
            .weights
            .snapshot
            .config
            .text_config
            .num_hidden_layers as usize;
        let input_buffer = self.token_input_buffer()?;
        let embed = self.profile_runtime_stage("embed", |this| {
            this.dequantize_token_embedding_into_buffer(token_id, &input_buffer)
        })?;

        let hidden_a = self.text_io.buffers.standalone_hidden.clone();
        let hidden_b = self.text_io.buffers.hidden_scratch.clone();
        let mut layers = Vec::with_capacity(layer_count);
        for layer_idx in 0..layer_count {
            let attention = self.kv_layout.cache_specs[layer_idx].attention;
            let (input_hidden_buffer, output_hidden_buffer) = if layer_idx % 2 == 0 {
                (&hidden_a, &hidden_b)
            } else {
                (&hidden_b, &hidden_a)
            };
            let runtime = self.session.runtime.clone();
            runtime.reset_counters();
            let started = Instant::now();
            runtime.begin_command_batch()?;
            let layer_result = self.eval_layer_hidden_state_core(
                layer_idx,
                None,
                Some(input_hidden_buffer),
                Some(output_hidden_buffer),
                position,
                false,
            );
            if let Err(err) = layer_result {
                let _ = runtime.discard_command_batch();
                return Err(err);
            }
            runtime.end_command_batch()?;
            runtime.wait_idle()?;
            layers.push(ExactMetalLayerProfile {
                layer_idx,
                attention,
                elapsed: started.elapsed(),
                counters: runtime.counters(),
            });
        }

        let final_hidden = self.final_hidden_buffer()?;
        let head_stages = vec![
            self.profile_runtime_stage("head.final_norm", |this| {
                this.dispatch_final_text_norm_on_hidden_buffer(&final_hidden)
            })?,
            self.profile_runtime_stage("head.logits_qmv", |this| {
                this.dispatch_logits_projection_from_final_norm()
            })?,
            self.profile_runtime_stage("head.argmax_softcap", |this| {
                this.dispatch_argmax_from_logits()
            })?,
        ];
        let head_elapsed = head_stages
            .iter()
            .fold(Duration::ZERO, |sum, stage| sum + stage.elapsed);
        let head = ExactMetalStageProfile {
            stage_name: "head",
            elapsed: head_elapsed,
            counters: sum_metal_runtime_counters(&head_stages),
        };
        let predicted_token_id = self.read_device_argmax_token_id()?;
        Ok(ExactMetalDecodeStepProfile {
            prompt_token_count,
            first_generated_token_id,
            profiled_token_id: token_id,
            profiled_position: position,
            embed,
            layers,
            head,
            head_stages,
            predicted_token_id,
        })
    }

    fn profile_decode_layers_after_prompt(
        &mut self,
        prompt_token_ids: &[u32],
    ) -> Result<ExactMetalDecodeStepProfile, Box<dyn Error>> {
        let first_generated_token_id =
            self.prefill_prompt_greedy_token_id_from_token_ids(prompt_token_ids, 0)?;
        self.profile_decode_step_from_token_id(
            first_generated_token_id,
            prompt_token_ids.len(),
            prompt_token_ids.len(),
            first_generated_token_id,
        )
    }

    pub(crate) fn generation_cursor(
        backend: Arc<Mutex<Self>>,
        prompt_token_ids: Arc<[u32]>,
        stop_tokens: BTreeSet<u32>,
        max_new_tokens: usize,
    ) -> Result<ExactMetalGenerationCursor, Box<dyn Error>> {
        if prompt_token_ids.is_empty() {
            return Err("generation requires at least one prompt token".into());
        }
        Ok(ExactMetalGenerationCursor {
            backend,
            prompt_token_ids,
            stop_tokens,
            max_new_tokens,
            processed_prompt_tokens: 0,
            position: 0,
            pending_next: None,
            generated_token_ids: Vec::with_capacity(max_new_tokens),
            stop_reason: None,
        })
    }

    pub(crate) fn generation_graph(
        backend: Arc<Mutex<Self>>,
        prompt_token_ids: Arc<[u32]>,
        stop_tokens: BTreeSet<u32>,
        max_new_tokens: usize,
    ) -> Result<ExactMetalGenerationGraph, Box<dyn Error>> {
        ExactMetalGenerationGraph::new(Self::generation_cursor(
            backend,
            prompt_token_ids,
            stop_tokens,
            max_new_tokens,
        )?)
    }

    fn kv_cache_for_layer(
        &self,
        layer_idx: usize,
    ) -> Result<RefMut<'_, ExactMetalKvCache>, Box<dyn Error>> {
        let cache_idx = self.kv_layout.cache_idx_for_layer(layer_idx)?;
        self.kv_caches
            .get(cache_idx)
            .ok_or_else(|| format!("missing exact metal KV cache {cache_idx}").into())
            .map(|cache| cache.borrow_mut())
    }

    fn layer_workspace(
        &mut self,
        layer_idx: usize,
    ) -> Result<ExactMetalLayerWorkspace, Box<dyn Error>> {
        if !self.layer_workspaces.contains_key(&layer_idx) {
            let workspace = ExactMetalLayerWorkspace::load(&mut self.session, layer_idx)?;
            self.layer_workspaces.insert(layer_idx, workspace);
        }
        self.layer_workspaces
            .get(&layer_idx)
            .cloned()
            .ok_or_else(|| format!("missing exact metal workspace for layer {layer_idx}").into())
    }

    fn token_input_buffer(&mut self) -> Result<MetalBuffer, Box<dyn Error>> {
        Ok(self.text_io.buffers.standalone_hidden.clone())
    }

    fn final_hidden_buffer(&mut self) -> Result<MetalBuffer, Box<dyn Error>> {
        let layer_count = self
            .session
            .weights
            .snapshot
            .config
            .text_config
            .num_hidden_layers as usize;
        if layer_count == 0 {
            return Ok(self.text_io.buffers.standalone_hidden.clone());
        }
        if layer_count % 2 == 0 {
            Ok(self.text_io.buffers.standalone_hidden.clone())
        } else {
            Ok(self.text_io.buffers.hidden_scratch.clone())
        }
    }

    fn dequantize_token_embedding_into_buffer(
        &mut self,
        token_id: u32,
        dst: &MetalBuffer,
    ) -> Result<(), Box<dyn Error>> {
        let token_idx = usize::try_from(token_id)?;
        if token_idx >= self.text_io.vocab_size {
            return Err(format!(
                "token id {} exceeds exact text IO vocabulary {}",
                token_id, self.text_io.vocab_size
            )
            .into());
        }
        let weight_offset = token_idx
            .checked_mul(self.text_io.embed_weight_row_bytes)
            .ok_or("exact text IO embed weight offset overflow")?;
        let qparams_offset = token_idx
            .checked_mul(self.text_io.embed_qparams_row_bytes)
            .ok_or("exact text IO embed qparams offset overflow")?;
        let runtime = self.session.runtime.clone();
        let owns_command_batch = !runtime.command_batch_is_active();
        let args = MlxAffineDequantRowArgs {
            n: NORM_LEN as u32,
            embed_scale: (self.session.weights.snapshot.config.text_config.hidden_size as f32)
                .sqrt(),
        };
        if owns_command_batch {
            runtime.begin_command_batch()?;
        }
        dispatch_compute_tracked_split(
            &runtime,
            &self.text_io.pipelines.dequant_row,
            bytes_of(&args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &self.text_io.weights.embed_weight,
                    offset_bytes: weight_offset,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &self.text_io.weights.embed_scales,
                    offset_bytes: qparams_offset,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &self.text_io.weights.embed_biases,
                    offset_bytes: qparams_offset,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: dst,
                    offset_bytes: 0,
                },
            ],
            3,
            &[],
            MetalSize {
                width: (NORM_LEN as u64).div_ceil(64),
                height: 1,
                depth: 1,
            },
            MetalSize {
                width: 64,
                height: 1,
                depth: 1,
            },
        )?;
        if owns_command_batch {
            runtime.end_command_batch()?;
        }
        Ok(())
    }

    fn dequantize_next_token_embedding_from_device_buffer(
        &mut self,
        dst: &MetalBuffer,
        history_slot: usize,
    ) -> Result<(), Box<dyn Error>> {
        if history_slot >= DEVICE_GREEDY_DECODE_CHUNK_TOKENS {
            return Err(format!(
                "device greedy decode chunk slot {} exceeds capacity {}",
                history_slot, DEVICE_GREEDY_DECODE_CHUNK_TOKENS
            )
            .into());
        }
        let runtime = self.session.runtime.clone();
        let owns_command_batch = !runtime.command_batch_is_active();
        let args = MlxAffineDequantTokenRowArgs {
            n: NORM_LEN as u32,
            embed_scale: (self.session.weights.snapshot.config.text_config.hidden_size as f32)
                .sqrt(),
            weight_words_per_row: self.text_io.logits_qproj.weight_words_per_row,
            qparams_per_row: self.text_io.logits_qproj.qparams_per_row,
            vocab_size: self.text_io.vocab_size as u32,
            history_slot: history_slot as u32,
        };
        if owns_command_batch {
            runtime.begin_command_batch()?;
        }
        let dispatch_result = dispatch_compute_tracked_split(
            &runtime,
            &self.text_io.pipelines.dequant_row_from_token_buffer,
            bytes_of(&args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &self.text_io.weights.embed_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &self.text_io.weights.embed_scales,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &self.text_io.weights.embed_biases,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &self.text_io.buffers.argmax_index_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: dst,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 6,
                    buffer: &self.text_io.buffers.generated_token_chunk_out,
                    offset_bytes: 0,
                },
            ],
            4,
            &[],
            MetalSize {
                width: (NORM_LEN as u64).div_ceil(64),
                height: 1,
                depth: 1,
            },
            MetalSize {
                width: 64,
                height: 1,
                depth: 1,
            },
        );
        if let Err(err) = dispatch_result {
            if owns_command_batch {
                let _ = runtime.discard_command_batch();
            }
            return Err(err);
        }
        if owns_command_batch {
            runtime.end_command_batch()?;
        }
        Ok(())
    }

    fn read_generated_token_chunk(&self, token_count: usize) -> Result<Vec<u32>, Box<dyn Error>> {
        if token_count > DEVICE_GREEDY_DECODE_CHUNK_TOKENS {
            return Err(format!(
                "requested generated token chunk {} exceeds capacity {}",
                token_count, DEVICE_GREEDY_DECODE_CHUNK_TOKENS
            )
            .into());
        }
        let runtime = self.session.runtime.clone();
        runtime
            .with_readable_buffer_range(
                &self.text_io.buffers.generated_token_chunk_out,
                0,
                token_count * size_of::<u32>(),
                |bytes| {
                    let mut token_ids = Vec::with_capacity(token_count);
                    for chunk in bytes.chunks_exact(size_of::<u32>()) {
                        let token_id = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                        let token_idx = usize::try_from(token_id).map_err(|err| {
                            format!("generated token id conversion failed: {err}")
                        })?;
                        if token_idx >= self.text_io.vocab_size {
                            return Err(format!(
                                "exact text IO generated token {} exceeds vocab {}",
                                token_id, self.text_io.vocab_size
                            ));
                        }
                        token_ids.push(token_id);
                    }
                    Ok(token_ids)
                },
            )
            .map_err(|err| err.into())
    }

    fn dispatch_greedy_head_on_hidden_buffer(
        &mut self,
        hidden_buffer: &MetalBuffer,
    ) -> Result<(), Box<dyn Error>> {
        let runtime = self.session.runtime.clone();
        let owns_command_batch = !runtime.command_batch_is_active();
        if owns_command_batch {
            runtime.begin_command_batch()?;
        }
        self.dispatch_final_text_norm_on_hidden_buffer(hidden_buffer)?;
        self.dispatch_logits_projection_from_final_norm()?;
        self.dispatch_argmax_from_logits()?;
        if owns_command_batch {
            runtime.end_command_batch()?;
        }
        Ok(())
    }

    fn dispatch_final_text_norm_on_hidden_buffer(
        &mut self,
        hidden_buffer: &MetalBuffer,
    ) -> Result<(), Box<dyn Error>> {
        let runtime = self.session.runtime.clone();
        let owns_command_batch = !runtime.command_batch_is_active();
        let n_reads = 4usize;
        let simd_size = 32usize;
        let rms_threadgroup_size = simd_size * NORM_LEN.div_ceil(n_reads).div_ceil(simd_size);
        let rms_args = MlxRmsNormRowArgs {
            n: NORM_LEN as u32,
            eps: self.text_io.eps,
        };
        if owns_command_batch {
            runtime.begin_command_batch()?;
        }
        dispatch_compute_tracked_split(
            &runtime,
            &self.text_io.pipelines.rms,
            bytes_of(&rms_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: hidden_buffer,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &self.text_io.weights.final_norm_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &self.text_io.buffers.final_norm_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            MetalSize {
                width: 1,
                height: 1,
                depth: 1,
            },
            MetalSize {
                width: rms_threadgroup_size as u64,
                height: 1,
                depth: 1,
            },
        )?;
        if owns_command_batch {
            runtime.end_command_batch()?;
        }
        Ok(())
    }

    fn dispatch_logits_projection_from_final_norm(&mut self) -> Result<(), Box<dyn Error>> {
        let runtime = self.session.runtime.clone();
        let owns_command_batch = !runtime.command_batch_is_active();
        let logits_args = self.text_io.logits_qproj.row_args(NORM_LEN as u32);
        if owns_command_batch {
            runtime.begin_command_batch()?;
        }
        dispatch_compute_tracked_split(
            &runtime,
            &self.text_io.pipelines.logits_proj,
            bytes_of(&logits_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &self.text_io.buffers.final_norm_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &self.text_io.weights.embed_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &self.text_io.weights.embed_scales,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &self.text_io.weights.embed_biases,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &self.text_io.buffers.logits_out,
                    offset_bytes: 0,
                },
            ],
            4,
            &[],
            MetalSize {
                width: 1,
                height: (self.text_io.vocab_size as u64).div_ceil(8),
                depth: 1,
            },
            MetalSize {
                width: 32,
                height: 2,
                depth: 1,
            },
        )?;
        if owns_command_batch {
            runtime.end_command_batch()?;
        }
        Ok(())
    }

    fn dispatch_argmax_from_logits(&mut self) -> Result<(), Box<dyn Error>> {
        let runtime = self.session.runtime.clone();
        let owns_command_batch = !runtime.command_batch_is_active();
        let argmax_args = MlxArgmaxSoftcappedBf16Args {
            n: self.text_io.vocab_size as u32,
            softcap: self.text_io.softcap.unwrap_or(0.0),
            has_softcap: u32::from(self.text_io.softcap.is_some()),
        };
        if owns_command_batch {
            runtime.begin_command_batch()?;
        }
        dispatch_compute_tracked_split(
            &runtime,
            &self.text_io.pipelines.argmax_softcapped_bf16,
            bytes_of(&argmax_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &self.text_io.buffers.logits_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &self.text_io.buffers.argmax_index_out,
                    offset_bytes: 0,
                },
            ],
            1,
            &[],
            MetalSize {
                width: 1,
                height: 1,
                depth: 1,
            },
            MetalSize {
                width: 1,
                height: 1,
                depth: 1,
            },
        )?;
        if owns_command_batch {
            runtime.end_command_batch()?;
        }
        Ok(())
    }

    fn read_device_argmax_token_id(&self) -> Result<u32, Box<dyn Error>> {
        let runtime = self.session.runtime.clone();
        let token_id = runtime.with_readable_buffer(
            &self.text_io.buffers.argmax_index_out,
            size_of::<u32>(),
            |bytes| {
                if bytes.len() != size_of::<u32>() {
                    return Err(format!(
                        "exact text IO argmax byte length mismatch: {}",
                        bytes.len()
                    ));
                }
                Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            },
        )?;
        let token_idx = usize::try_from(token_id)?;
        if token_idx >= self.text_io.vocab_size {
            return Err(format!(
                "exact text IO argmax token {} exceeds vocab {}",
                token_id, self.text_io.vocab_size
            )
            .into());
        }
        Ok(token_id)
    }

    fn read_device_greedy_token(&self) -> Result<MlxGreedyToken, Box<dyn Error>> {
        let runtime = self.session.runtime.clone();
        let token_id = self.read_device_argmax_token_id()?;
        let token_idx = usize::try_from(token_id)?;
        let raw_logit = runtime.with_readable_buffer_range(
            &self.text_io.buffers.logits_out,
            token_idx * size_of::<u16>(),
            size_of::<u16>(),
            |bytes| {
                if bytes.len() != size_of::<u16>() {
                    return Err(format!(
                        "exact text IO bf16 logit byte length mismatch: {}",
                        bytes.len()
                    ));
                }
                Ok(bf16_word_to_f32(u16::from_le_bytes([bytes[0], bytes[1]])))
            },
        )?;
        let logit = if let Some(softcap) = self.text_io.softcap {
            bf16_round_to_f32((raw_logit / softcap).tanh() * softcap)
        } else {
            raw_logit
        };
        Ok(MlxGreedyToken { token_id, logit })
    }

    fn read_hidden_words_from_buffer(
        &self,
        hidden_buffer: &MetalBuffer,
    ) -> Result<Vec<u16>, Box<dyn Error>> {
        Ok(bf16_words_from_f32_bits(&read_bf16_buffer_bits(
            &self.session.runtime,
            hidden_buffer,
            NORM_LEN,
        )?))
    }

    fn read_shared_logits_greedy_token(&self) -> Result<MlxGreedyToken, Box<dyn Error>> {
        let runtime = self.session.runtime.clone();
        Ok(runtime.with_readable_buffer(
            &self.text_io.buffers.logits_out,
            self.text_io.vocab_size * size_of::<u16>(),
            |bytes| {
                if bytes.len() != self.text_io.vocab_size * size_of::<u16>() {
                    return Err(format!(
                        "exact text IO logits byte length mismatch: {}",
                        bytes.len()
                    ));
                }
                let mut best_token_id = 0u32;
                let mut best_logit = f32::NEG_INFINITY;
                for (token_idx, word_bytes) in bytes.chunks_exact(size_of::<u16>()).enumerate() {
                    let raw_logit =
                        bf16_word_to_f32(u16::from_le_bytes([word_bytes[0], word_bytes[1]]));
                    let logit = if let Some(softcap) = self.text_io.softcap {
                        bf16_round_to_f32((raw_logit / softcap).tanh() * softcap)
                    } else {
                        raw_logit
                    };
                    let token_id = token_idx as u32;
                    if logit > best_logit || (logit == best_logit && token_id < best_token_id) {
                        best_logit = logit;
                        best_token_id = token_id;
                    }
                }
                Ok(MlxGreedyToken {
                    token_id: best_token_id,
                    logit: best_logit,
                })
            },
        )?)
    }

    fn greedy_token_from_hidden_buffer(
        &mut self,
        hidden_buffer: &MetalBuffer,
    ) -> Result<MlxGreedyToken, Box<dyn Error>> {
        self.dispatch_final_text_norm_on_hidden_buffer(hidden_buffer)?;
        self.dispatch_logits_projection_from_final_norm()?;
        self.read_shared_logits_greedy_token()
    }
