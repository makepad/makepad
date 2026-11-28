//! Chat room is measured off both the HTTP route and the generation worker.
//! A memo entry contains only a keyed hash and a token count. Each loaded
//! vocabulary owns its memo, so unloading also discards a quiet chat's entries.
use crate::protocol::{ChatMeasure, ChatRoomRequest, PictureTokens};

pub(super) fn measurement(
    model: &str,
    window: usize,
    budget: u32,
    prompt: Option<usize>,
    tail: Option<usize>,
    pictures: Vec<PictureTokens>,
    sees: &str,
) -> ChatMeasure {
    let reserve = super::answer_reserve(budget);
    ChatMeasure {
        model: model.into(),
        window: window as u64,
        reserve: reserve as u64,
        prompt_tokens: prompt.map(|n| n as u64),
        tail_tokens: tail.map(|n| n as u64),
        pictures,
        sees: sees.into(),
        fits: prompt.map(|n| super::fits_window(n, reserve, window)),
    }
}

pub(crate) fn measure(request: ChatRoomRequest) -> Result<ChatMeasure, String> {
    let budget =
        super::generation_budget(&request.model, super::requested_tokens(request.max_tokens), true);
    #[cfg(feature = "llm")]
    let window = crate::llm_context::loaded().or_else(|| super::current_fit_window(&request.model)).unwrap_or(0) as usize;
    #[cfg(not(feature = "llm"))]
    let window = 0; // This build cannot serve a model.
    #[cfg(feature = "llm")]
    if let Some(resident) = resident::get(&request.model) {
        return resident.measure(request, window, budget);
    }
    let pictures = request
        .chat_messages
        .iter()
        .flatten()
        .filter(|row| row.role != "assistant")
        .flat_map(|row| row.images.iter().flatten())
        .map(|pic| PictureTokens { key: pic.key.clone(), tokens: None })
        .collect();
    Ok(measurement(&request.model, window, budget, None, None, pictures, "unknown"))
}

#[cfg(feature = "llm")]
pub(super) mod resident {
    use super::*;
    use crate::protocol::{render_chat, split_chat_images, think_prefill_for_request, PromptPiece};
    use makepad_ai_llm::{GgufFile, LlamaVocab, VisionConfig};
    use std::collections::{hash_map::RandomState, HashMap};
    use std::hash::{BuildHasher, Hash};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex, OnceLock, Weak};

    pub(crate) struct Geometry {
        path: Option<PathBuf>,
        config: OnceLock<VisionConfig>,
        pub(crate) image_px: usize,
        lanes: usize,
    }

    impl Geometry {
        pub(crate) fn new(path: Option<PathBuf>, image_px: usize, lanes: usize) -> Self {
            Self { path, config: OnceLock::new(), image_px, lanes }
        }

        pub(crate) fn read(&self) -> Result<Option<&VisionConfig>, String> {
            if self.config.get().is_none() {
                if let Some(path) = self.path.as_ref().filter(|p| p.is_file()) {
                    let header = GgufFile::open(path).map_err(|e| format!("load vision tower: {e:?}"))?;
                    let config = VisionConfig::from_gguf(&header)
                        .map_err(|e| format!("load vision tower: {e:?}"))?;
                    let _ = self.config.set(config);
                }
            }
            Ok(self.config.get())
        }

        pub(crate) fn grid(&self, w: usize, h: usize) -> Result<Option<(usize, usize)>, String> {
            Ok(self.read()?.filter(|_| self.lanes == 1).map(|config| self.fitted_grid(config, w, h)))
        }

        pub(crate) fn fitted_grid(&self, config: &VisionConfig, w: usize, h: usize) -> (usize, usize) {
            let (w, h) = crate::vision_backend::fitted_size(w, h, self.image_px);
            config.token_grid(w, h)
        }

        pub(crate) fn set(&self, config: VisionConfig) {
            let _ = self.config.set(config);
        }

        pub(crate) fn sees(&self) -> &'static str {
            if self.lanes > 1 {
                "unknown"
            } else if self.config.get().is_some() {
                "yes"
            } else if self.path.is_none() {
                "no"
            } else {
                "unknown"
            }
        }
    }

    #[derive(Default)]
    struct Memo {
        hash: RandomState,
        counts: HashMap<u64, usize>,
    }

    impl Memo {
        fn count(
            &mut self,
            key: impl Hash,
            tokenize: impl FnOnce() -> Result<usize, String>,
        ) -> Result<usize, String> {
            let key = self.hash.hash_one(key);
            if let Some(&n) = self.counts.get(&key) {
                return Ok(n);
            }
            let n = tokenize()?;
            self.counts.insert(key, n);
            Ok(n)
        }
    }

    pub(crate) struct Resident {
        model: String,
        vocab: Arc<LlamaVocab>,
        geometry: Arc<Geometry>,
        // Only the measure thread takes this lock. Neither generation nor the
        // HTTP route waits on tokenization; the last owner drops the memo.
        memo: Mutex<Memo>,
        #[cfg(test)]
        calls: std::sync::atomic::AtomicUsize,
    }

    fn models() -> &'static Mutex<HashMap<String, Weak<Resident>>> {
        static MODELS: OnceLock<Mutex<HashMap<String, Weak<Resident>>>> = OnceLock::new();
        MODELS.get_or_init(Default::default)
    }
    pub(super) fn get(model: &str) -> Option<Arc<Resident>> {
        models().lock().unwrap().get(model)?.upgrade()
    }

    pub(crate) struct Publication(Arc<Resident>);

    impl Drop for Publication {
        fn drop(&mut self) {
            let mut models = models().lock().unwrap();
            if models.get(&self.0.model).is_some_and(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(&self.0))) {
                models.remove(&self.0.model);
            }
        }
    }

    pub(crate) fn publish(model: String, vocab: Arc<LlamaVocab>, geometry: Arc<Geometry>) -> Publication {
        let resident = Arc::new(Resident {
            model,
            vocab,
            geometry,
            memo: Default::default(),
            #[cfg(test)]
            calls: Default::default(),
        });
        models().lock().unwrap().insert(resident.model.clone(), Arc::downgrade(&resident));
        Publication(resident)
    }

    #[cfg(test)]
    pub(crate) fn tokenizer_calls(model: &str) -> usize {
        get(model).unwrap().calls.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn render_request(request: &mut ChatRoomRequest, row_end: impl FnMut(usize)) -> String {
        if request.thinking == Some(false) {
            super::super::disable_thinking(&mut request.chat_system);
        }
        render_chat(
            request.chat_system.as_deref().unwrap_or_default(),
            request.chat_messages.iter().flatten().map(|row| {
                (row.role.as_str(), row.text.as_str(), row.images.iter().flatten().map(|p| p.key.clone()))
            }),
            think_prefill_for_request(&request.model, request.thinking),
            Some(request.prompt.as_deref().unwrap_or_default()),
            row_end,
        )
    }

    impl Resident {
        fn count_text(&self, text: &str, first: bool, last: bool) -> Result<usize, String> {
            #[cfg(test)]
            self.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            super::super::tokenize_run(&self.vocab, text, first, last).map(|ids| ids.len())
        }

        pub(super) fn measure(
            &self,
            mut request: ChatRoomRequest,
            window: usize,
            budget: u32,
        ) -> Result<ChatMeasure, String> {
            let mut ends = Vec::new();
            let prompt = render_request(&mut request, |end| ends.push(end));
            let rows = request.chat_messages.as_deref().unwrap_or_default();
            let raw = request.prompt.as_deref().unwrap_or_default();
            let tail = rows.last().map_or(raw, |r| r.text.as_str());
            self.geometry.read()?;
            let sees = self.geometry.sees();
            let mut sizes = HashMap::new();
            for p in rows.iter().flat_map(|r| r.images.iter().flatten()) {
                if p.key.len() != 16 || !p.key.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err("a picture key must contain 16 hexadecimal digits".into());
                }
                let size = (p.w as usize, p.h as usize);
                crate::vision_backend::check_image_dimensions(size.0, size.1).map_err(|e| e.to_string())?;
                if sizes.insert(p.key.as_str(), size).is_some_and(|prior| prior != size) {
                    return Err(format!("picture {} has conflicting dimensions", p.key));
                }
            }
            let mut memo = self.memo.lock().unwrap();
            // Tokenize at the ChatML control token which opens each rendered row.
            // Preserve the job's BOS/EOS treatment across a split text run:
            // Only the first text run receives automatic special tokens.
            // Its automatic EOS is stripped when it is also the final run.
            let mut count = Some(0usize);
            let mut pictures = Vec::new();
            let mut start = 0;
            let has_picture = self.vocab.add_eos_token()
                && split_chat_images(&prompt).iter().any(|piece| matches!(piece, PromptPiece::Image(_)));
            for end in ends {
                let row = &prompt[start..end];
                let pieces = split_chat_images(row);
                let last_piece = pieces.len().saturating_sub(1);
                // The hashed bytes contain the role, rendered text and ordered
                // markers. Picture geometry is priced afresh, outside the memo.
                let first_row = start == 0;
                let last_row = end == prompt.len();
                let strip_first_eos = first_row && !has_picture && self.vocab.add_eos_token();
                let text_tokens = memo.count((row, first_row, last_row, strip_first_eos), || {
                    let mut tokens = 0;
                    for (i, piece) in pieces.iter().enumerate() {
                        if let PromptPiece::Text(text) = piece {
                            let first = first_row && i == 0;
                            let strip_eos = (last_row && i == last_piece) || (first && strip_first_eos);
                            tokens += self.count_text(text, first, strip_eos)?;
                        }
                    }
                    Ok(tokens)
                })?;
                count = count.map(|n| n + text_tokens);
                for piece in pieces {
                    if let PromptPiece::Image(key) = piece {
                        let &(w, h) =
                            sizes.get(key).ok_or_else(|| format!("picture {key} has no dimensions"))?;
                        let tokens = self.geometry.grid(w, h)?.map(|(w, h)| w * h);
                        count = count.zip(tokens).map(|(total, n)| total + n);
                        pictures.push(PictureTokens { key: key.into(), tokens: tokens.map(|n| n as u64) });
                    }
                }
                start = end;
            }
            let tail_role = rows.last().map(|row| row.role.as_str());
            let tail = memo.count(("tail", tail_role, tail), || self.count_text(tail, false, false))?;
            Ok(measurement(&request.model, window, budget, count, Some(tail), pictures, sees))
        }
    }
}
