use super::{
    sample_token_from_logits_f32, MlxQwen35MoeDecodeState, MlxQwen35MoeRuntimeSession,
    MlxQwen35MoeStopReason, QwenSamplingOptions, QwenSamplingRng,
};
use std::collections::BTreeSet;
use std::error::Error;
use std::sync::{Arc, Mutex, OnceLock};

pub(crate) trait QwenGenerationBackend {
    fn preferred_generation_stride(&self) -> usize {
        1
    }

    fn prefill_prompt(&mut self, prompt_token_ids: &[u32]) -> Result<u32, String>;

    fn eval_next_token(&mut self, token_id: u32, position: usize) -> Result<u32, String>;

    fn eval_token_chunk(
        &mut self,
        token_id: u32,
        position: usize,
        token_count: usize,
    ) -> Result<Vec<u32>, String> {
        if token_count == 0 {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(token_count);
        let mut current_token_id = token_id;
        let mut current_position = position;
        for _ in 0..token_count {
            let next_token_id = self.eval_next_token(current_token_id, current_position)?;
            out.push(next_token_id);
            current_token_id = next_token_id;
            current_position += 1;
        }
        Ok(out)
    }
}

struct QwenReferenceGenerationBackend {
    runtime: Arc<MlxQwen35MoeRuntimeSession>,
    decode_state: MlxQwen35MoeDecodeState,
    sampling: QwenSamplingOptions,
    disallowed_token_ids: Vec<u32>,
    rng: QwenSamplingRng,
}

impl QwenReferenceGenerationBackend {
    fn new(runtime: Arc<MlxQwen35MoeRuntimeSession>, do_sample: bool) -> Result<Self, String> {
        let decode_state = runtime.new_decode_state().map_err(|err| err.to_string())?;
        let sampling = runtime.sampling_options(do_sample);
        let disallowed_token_ids = runtime.generation_disallowed_token_ids();
        Ok(Self {
            runtime,
            decode_state,
            sampling,
            disallowed_token_ids,
            rng: QwenSamplingRng::new(0),
        })
    }

    fn sample_from_logits(&mut self, logits: &[f32]) -> Result<u32, String> {
        sample_token_from_logits_f32(
            logits,
            &self.disallowed_token_ids,
            &self.sampling,
            &mut self.rng,
        )
        .map(|token| token.token_id)
    }
}

impl QwenGenerationBackend for QwenReferenceGenerationBackend {
    fn prefill_prompt(&mut self, prompt_token_ids: &[u32]) -> Result<u32, String> {
        let mut next_token_id = None;
        for (position, &token_id) in prompt_token_ids.iter().enumerate() {
            let logits = self
                .runtime
                .eval_token_logits_reference_f32(token_id, position, &mut self.decode_state)
                .map_err(|err| err.to_string())?;
            next_token_id = Some(self.sample_from_logits(&logits)?);
        }
        next_token_id.ok_or_else(|| "generation requires at least one prompt token".to_string())
    }

    fn eval_next_token(&mut self, token_id: u32, position: usize) -> Result<u32, String> {
        let logits = self
            .runtime
            .eval_token_logits_reference_f32(token_id, position, &mut self.decode_state)
            .map_err(|err| err.to_string())?;
        self.sample_from_logits(&logits)
    }
}

pub(crate) struct MlxQwen35MoeGenerationCursor {
    backend: Arc<Mutex<Box<dyn QwenGenerationBackend>>>,
    prompt_token_ids: Arc<[u32]>,
    stop_tokens: BTreeSet<u32>,
    max_new_tokens: Option<usize>,
    processed_prompt_tokens: usize,
    position: usize,
    pending_next: Option<u32>,
    generated_token_ids: Vec<u32>,
    stop_reason: Option<MlxQwen35MoeStopReason>,
}

#[derive(Clone)]
pub(crate) struct MlxQwen35MoeGenerationSnapshot {
    pub(crate) generated_token_ids: Arc<[u32]>,
    pub(crate) stop_reason: Option<MlxQwen35MoeStopReason>,
    #[cfg(test)]
    pub(crate) processed_prompt_tokens: usize,
    #[cfg(test)]
    pub(crate) position: usize,
    #[cfg(test)]
    pub(crate) has_pending_next: bool,
}

struct MlxQwen35MoePromptPrefillNode {
    cursor: Arc<Mutex<MlxQwen35MoeGenerationCursor>>,
    value: OnceLock<Result<Arc<MlxQwen35MoeGenerationSnapshot>, String>>,
}

enum MlxQwen35MoeGenerationDependency {
    PromptPrefill(Arc<MlxQwen35MoePromptPrefillNode>),
    Previous(Arc<MlxQwen35MoeGenerationStepNode>),
}

struct MlxQwen35MoeGenerationStepNode {
    cursor: Arc<Mutex<MlxQwen35MoeGenerationCursor>>,
    target_count: usize,
    dependency: MlxQwen35MoeGenerationDependency,
    value: OnceLock<Result<Arc<MlxQwen35MoeGenerationSnapshot>, String>>,
}

pub(crate) struct MlxQwen35MoeGenerationGraph {
    cursor: Arc<Mutex<MlxQwen35MoeGenerationCursor>>,
    prompt_prefill: Arc<MlxQwen35MoePromptPrefillNode>,
    step_nodes: Mutex<Vec<Arc<MlxQwen35MoeGenerationStepNode>>>,
    final_snapshot: OnceLock<Result<Arc<MlxQwen35MoeGenerationSnapshot>, String>>,
    max_new_tokens: Option<usize>,
    step_stride: usize,
}

pub(crate) fn reference_generation_backend(
    runtime: Arc<MlxQwen35MoeRuntimeSession>,
    do_sample: bool,
) -> Result<Box<dyn QwenGenerationBackend>, Box<dyn Error>> {
    Ok(Box::new(QwenReferenceGenerationBackend::new(
        runtime, do_sample,
    )?))
}

pub(crate) fn start_generation_graph(
    backend: Box<dyn QwenGenerationBackend>,
    prompt_token_ids: Arc<[u32]>,
    stop_tokens: BTreeSet<u32>,
    max_new_tokens: Option<usize>,
) -> Result<MlxQwen35MoeGenerationGraph, Box<dyn Error>> {
    let step_stride = backend.preferred_generation_stride().max(1);
    MlxQwen35MoeGenerationGraph::new(
        MlxQwen35MoeGenerationCursor::new(
            Arc::new(Mutex::new(backend)),
            prompt_token_ids,
            stop_tokens,
            max_new_tokens,
        )?,
        step_stride,
    )
}

impl MlxQwen35MoeGenerationCursor {
    fn new(
        backend: Arc<Mutex<Box<dyn QwenGenerationBackend>>>,
        prompt_token_ids: Arc<[u32]>,
        stop_tokens: BTreeSet<u32>,
        max_new_tokens: Option<usize>,
    ) -> Result<Self, Box<dyn Error>> {
        if prompt_token_ids.is_empty() {
            return Err("generation requires at least one prompt token".into());
        }
        Ok(Self {
            backend,
            prompt_token_ids,
            stop_tokens,
            max_new_tokens,
            processed_prompt_tokens: 0,
            position: 0,
            pending_next: None,
            generated_token_ids: Vec::with_capacity(max_new_tokens.unwrap_or(32)),
            stop_reason: None,
        })
    }

    fn target_count(&self, requested_count: usize) -> usize {
        self.max_new_tokens
            .map_or(requested_count, |limit| requested_count.min(limit))
    }

    fn remaining_generation_limit(&self) -> usize {
        self.max_new_tokens.map_or(usize::MAX, |limit| {
            limit.saturating_sub(self.generated_token_ids.len())
        })
    }

    fn reached_generation_limit(&self) -> bool {
        self.max_new_tokens
            .is_some_and(|limit| self.generated_token_ids.len() >= limit)
    }

    fn ensure_prompt_prefilled_locked(
        &mut self,
        backend: &mut dyn QwenGenerationBackend,
    ) -> Result<(), String> {
        if self.processed_prompt_tokens >= self.prompt_token_ids.len() {
            return Ok(());
        }
        let remaining_prompt_tokens = &self.prompt_token_ids[self.processed_prompt_tokens..];
        self.pending_next = Some(backend.prefill_prompt(remaining_prompt_tokens)?);
        self.processed_prompt_tokens += remaining_prompt_tokens.len();
        self.position += remaining_prompt_tokens.len();
        Ok(())
    }

    fn ensure_prompt_prefilled(&mut self) -> Result<(), String> {
        let backend_handle = Arc::clone(&self.backend);
        let mut backend = backend_handle
            .lock()
            .map_err(|_| "qwen generation backend mutex poisoned".to_string())?;
        self.ensure_prompt_prefilled_locked(backend.as_mut())
    }

    fn snapshot(&self) -> MlxQwen35MoeGenerationSnapshot {
        MlxQwen35MoeGenerationSnapshot {
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

    fn ensure_generated(&mut self, requested_count: usize) -> Result<(), String> {
        let target = self.target_count(requested_count);
        let backend_handle = Arc::clone(&self.backend);
        let mut backend = backend_handle
            .lock()
            .map_err(|_| "qwen generation backend mutex poisoned".to_string())?;
        while self.generated_token_ids.len() < target {
            if self.stop_reason.is_some() {
                break;
            }
            if self.pending_next.is_none() {
                if self.processed_prompt_tokens < self.prompt_token_ids.len() {
                    self.ensure_prompt_prefilled_locked(backend.as_mut())?;
                } else if let Some(&last_generated) = self.generated_token_ids.last() {
                    let input_position = self
                        .position
                        .checked_sub(1)
                        .ok_or_else(|| "generation cursor position underflow".to_string())?;
                    let remaining_target = target.saturating_sub(self.generated_token_ids.len());
                    let remaining_max = self.remaining_generation_limit();
                    let chunk_stride = backend.preferred_generation_stride().max(1);
                    let chunk_len = remaining_target.min(remaining_max).min(chunk_stride);
                    if chunk_len > 1 {
                        let chunk_tokens =
                            backend.eval_token_chunk(last_generated, input_position, chunk_len)?;
                        for token_id in chunk_tokens {
                            if self.stop_tokens.contains(&token_id) {
                                self.stop_reason = Some(MlxQwen35MoeStopReason::EosToken(token_id));
                                break;
                            }
                            self.generated_token_ids.push(token_id);
                            self.position += 1;
                            if self.reached_generation_limit() {
                                self.stop_reason = Some(MlxQwen35MoeStopReason::MaxNewTokens);
                                break;
                            }
                            if self.generated_token_ids.len() >= target {
                                break;
                            }
                        }
                        continue;
                    }
                    self.pending_next =
                        Some(backend.eval_next_token(last_generated, input_position)?);
                }
            }
            let next_token = self
                .pending_next
                .take()
                .ok_or_else(|| "generation cursor missing pending next token".to_string())?;
            if self.stop_tokens.contains(&next_token) {
                self.stop_reason = Some(MlxQwen35MoeStopReason::EosToken(next_token));
                break;
            }
            self.generated_token_ids.push(next_token);
            self.position += 1;
            if self.reached_generation_limit() {
                self.stop_reason = Some(MlxQwen35MoeStopReason::MaxNewTokens);
                break;
            }
        }
        if self.reached_generation_limit() && self.stop_reason.is_none() {
            self.stop_reason = Some(MlxQwen35MoeStopReason::MaxNewTokens);
            self.pending_next = None;
        }
        Ok(())
    }

    fn ensure_finished(&mut self) -> Result<(), String> {
        if let Some(limit) = self.max_new_tokens {
            self.ensure_generated(limit)
        } else {
            while self.stop_reason.is_none() {
                let next_target = self.generated_token_ids.len().saturating_add(32);
                self.ensure_generated(next_target)?;
            }
            Ok(())
        }
    }
}

impl MlxQwen35MoePromptPrefillNode {
    fn new(cursor: Arc<Mutex<MlxQwen35MoeGenerationCursor>>) -> Self {
        Self {
            cursor,
            value: OnceLock::new(),
        }
    }

    fn eval(&self) -> Result<Arc<MlxQwen35MoeGenerationSnapshot>, String> {
        self.value
            .get_or_init(|| {
                let mut cursor = self
                    .cursor
                    .lock()
                    .map_err(|_| "generation cursor mutex poisoned".to_string())?;
                cursor.ensure_prompt_prefilled()?;
                Ok(Arc::new(cursor.snapshot()))
            })
            .clone()
    }
}

impl MlxQwen35MoeGenerationStepNode {
    fn new(
        cursor: Arc<Mutex<MlxQwen35MoeGenerationCursor>>,
        target_count: usize,
        dependency: MlxQwen35MoeGenerationDependency,
    ) -> Self {
        Self {
            cursor,
            target_count,
            dependency,
            value: OnceLock::new(),
        }
    }

    fn eval(&self) -> Result<Arc<MlxQwen35MoeGenerationSnapshot>, String> {
        self.value
            .get_or_init(|| {
                match &self.dependency {
                    MlxQwen35MoeGenerationDependency::PromptPrefill(node) => {
                        node.eval()?;
                    }
                    MlxQwen35MoeGenerationDependency::Previous(node) => {
                        node.eval()?;
                    }
                }
                let mut cursor = self
                    .cursor
                    .lock()
                    .map_err(|_| "generation cursor mutex poisoned".to_string())?;
                cursor.ensure_generated(self.target_count)?;
                Ok(Arc::new(cursor.snapshot()))
            })
            .clone()
    }
}

impl MlxQwen35MoeGenerationGraph {
    fn new(
        cursor: MlxQwen35MoeGenerationCursor,
        step_stride: usize,
    ) -> Result<Self, Box<dyn Error>> {
        let max_new_tokens = cursor.max_new_tokens;
        let cursor = Arc::new(Mutex::new(cursor));
        Ok(Self {
            prompt_prefill: Arc::new(MlxQwen35MoePromptPrefillNode::new(Arc::clone(&cursor))),
            cursor,
            step_nodes: Mutex::new(Vec::with_capacity(max_new_tokens.unwrap_or(32))),
            final_snapshot: OnceLock::new(),
            max_new_tokens,
            step_stride: step_stride.max(1),
        })
    }

    fn step_node(
        &self,
        requested_count: usize,
    ) -> Result<Arc<MlxQwen35MoeGenerationStepNode>, String> {
        let target = self
            .max_new_tokens
            .map_or(requested_count, |limit| requested_count.min(limit));
        if target == 0 {
            return Err("generation step nodes start at token count 1".to_string());
        }
        let mut nodes = self
            .step_nodes
            .lock()
            .map_err(|_| "generation step-node mutex poisoned".to_string())?;
        while nodes.len() < target {
            let next_count = nodes.len() + 1;
            let dependency = if next_count > self.step_stride {
                let dependency_index = next_count
                    .checked_sub(self.step_stride)
                    .and_then(|index| index.checked_sub(1))
                    .ok_or_else(|| "generation step dependency underflow".to_string())?;
                let prev = nodes.get(dependency_index).cloned().ok_or_else(|| {
                    format!("missing generation dependency {}", dependency_index + 1)
                })?;
                MlxQwen35MoeGenerationDependency::Previous(prev)
            } else {
                MlxQwen35MoeGenerationDependency::PromptPrefill(Arc::clone(&self.prompt_prefill))
            };
            nodes.push(Arc::new(MlxQwen35MoeGenerationStepNode::new(
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

    pub(crate) fn snapshot_up_to(
        &self,
        requested_count: usize,
    ) -> Result<Arc<MlxQwen35MoeGenerationSnapshot>, String> {
        let target = self
            .max_new_tokens
            .map_or(requested_count, |limit| requested_count.min(limit));
        if target == 0 {
            let cursor = self
                .cursor
                .lock()
                .map_err(|_| "generation cursor mutex poisoned".to_string())?;
            return Ok(Arc::new(cursor.snapshot()));
        }
        self.step_node(target)?.eval()
    }

    pub(crate) fn finish_snapshot(&self) -> Result<Arc<MlxQwen35MoeGenerationSnapshot>, String> {
        self.final_snapshot
            .get_or_init(|| {
                let mut cursor = self
                    .cursor
                    .lock()
                    .map_err(|_| "generation cursor mutex poisoned".to_string())?;
                cursor.ensure_finished()?;
                Ok(Arc::new(cursor.snapshot()))
            })
            .clone()
    }

    pub(crate) fn step_stride(&self) -> usize {
        self.step_stride
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Arc;

    #[derive(Default)]
    struct MockBackendStats {
        prefill_calls: usize,
        eval_calls: usize,
        chunk_calls: Vec<usize>,
    }

    struct MockBackend {
        stats: Arc<Mutex<MockBackendStats>>,
        prefill_next: Option<u32>,
        next_tokens: VecDeque<u32>,
    }

    impl MockBackend {
        fn new(
            prefill_next: u32,
            next_tokens: impl Into<Vec<u32>>,
        ) -> (Self, Arc<Mutex<MockBackendStats>>) {
            let stats = Arc::new(Mutex::new(MockBackendStats::default()));
            (
                Self {
                    stats: Arc::clone(&stats),
                    prefill_next: Some(prefill_next),
                    next_tokens: VecDeque::from(next_tokens.into()),
                },
                stats,
            )
        }
    }

    fn mock_backend(
        prefill_next: u32,
        next_tokens: impl Into<Vec<u32>>,
    ) -> (MockBackend, Arc<Mutex<MockBackendStats>>) {
        MockBackend::new(prefill_next, next_tokens)
    }

    impl QwenGenerationBackend for MockBackend {
        fn preferred_generation_stride(&self) -> usize {
            4
        }

        fn prefill_prompt(&mut self, _prompt_token_ids: &[u32]) -> Result<u32, String> {
            self.stats.lock().unwrap().prefill_calls += 1;
            self.prefill_next
                .take()
                .ok_or_else(|| "mock prefill called more than once".to_string())
        }

        fn eval_next_token(&mut self, _token_id: u32, _position: usize) -> Result<u32, String> {
            self.stats.lock().unwrap().eval_calls += 1;
            self.next_tokens
                .pop_front()
                .ok_or_else(|| "mock backend ran out of next tokens".to_string())
        }

        fn eval_token_chunk(
            &mut self,
            mut token_id: u32,
            mut position: usize,
            token_count: usize,
        ) -> Result<Vec<u32>, String> {
            self.stats.lock().unwrap().chunk_calls.push(token_count);
            let mut out = Vec::with_capacity(token_count);
            for _ in 0..token_count {
                token_id = self.eval_next_token(token_id, position)?;
                out.push(token_id);
                position += 1;
            }
            Ok(out)
        }
    }

    fn mock_graph(
        backend: MockBackend,
        stop_tokens: BTreeSet<u32>,
        max_new_tokens: Option<usize>,
    ) -> MlxQwen35MoeGenerationGraph {
        MlxQwen35MoeGenerationGraph::new(
            MlxQwen35MoeGenerationCursor::new(
                Arc::new(Mutex::new(Box::new(backend))),
                Arc::<[u32]>::from([1u32, 2u32]),
                stop_tokens,
                max_new_tokens,
            )
            .unwrap(),
            4,
        )
        .unwrap()
    }

    #[test]
    fn qwen_generation_graph_prefill_and_steps_are_lazy_and_cumulative() {
        let (backend, _) = mock_backend(11, vec![12, 13, 14]);
        let graph = mock_graph(backend, BTreeSet::new(), Some(3));
        let snapshot1 = graph.snapshot_up_to(1).unwrap();
        assert_eq!(&*snapshot1.generated_token_ids, &[11]);
        assert_eq!(snapshot1.stop_reason, None);
        assert_eq!(snapshot1.processed_prompt_tokens, 2);
        assert_eq!(snapshot1.position, 3);

        let snapshot2 = graph.snapshot_up_to(2).unwrap();
        assert_eq!(&*snapshot2.generated_token_ids, &[11, 12]);
        assert_eq!(snapshot2.stop_reason, None);

        let snapshot3 = graph.finish_snapshot().unwrap();
        assert_eq!(&*snapshot3.generated_token_ids, &[11, 12, 13]);
        assert_eq!(
            snapshot3.stop_reason,
            Some(MlxQwen35MoeStopReason::MaxNewTokens)
        );
        assert!(!snapshot3.has_pending_next);
    }

    #[test]
    fn qwen_generation_graph_stops_on_eos_without_appending_it() {
        let (backend, _) = mock_backend(21, vec![99, 42, 43, 44]);
        let graph = mock_graph(backend, BTreeSet::from([99u32]), Some(8));
        let snapshot = graph.finish_snapshot().unwrap();
        assert_eq!(&*snapshot.generated_token_ids, &[21]);
        assert_eq!(
            snapshot.stop_reason,
            Some(MlxQwen35MoeStopReason::EosToken(99))
        );
    }

    #[test]
    fn qwen_generation_graph_uses_stride_sized_chunk_dependencies() {
        let (backend, stats) = mock_backend(11, vec![12, 13, 14, 15, 16, 17, 18, 19]);
        let backend = Arc::new(Mutex::new(
            Box::new(backend) as Box<dyn QwenGenerationBackend>
        ));
        let graph = MlxQwen35MoeGenerationGraph::new(
            MlxQwen35MoeGenerationCursor::new(
                Arc::clone(&backend),
                Arc::<[u32]>::from([1u32, 2u32]),
                BTreeSet::new(),
                Some(9),
            )
            .unwrap(),
            4,
        )
        .unwrap();
        let snapshot = graph.snapshot_up_to(9).unwrap();
        assert_eq!(
            &*snapshot.generated_token_ids,
            &[11, 12, 13, 14, 15, 16, 17, 18, 19]
        );
        let stats = stats.lock().unwrap();
        assert_eq!(stats.chunk_calls, vec![4, 4]);
        assert_eq!(stats.eval_calls, 8);
    }
}
