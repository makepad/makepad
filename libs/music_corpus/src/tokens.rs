//! Token counts with the model's own tokenizer: the vocabulary and merges
//! of a Qwen GGUF, read by `makepad-ai-llm`, and the chat layout the
//! fine-tune trains on (Qwen's `<|im_start|>role ... <|im_end|>` turns, the
//! assistant turn with an empty think block: answers without reasoning).

use makepad_ai_llm::{GgufFile, LlamaVocab};
use std::path::{Path, PathBuf};

pub struct Tokenizer {
    vocab: LlamaVocab,
    pub file: String,
}

impl Tokenizer {
    pub fn open(path: &Path) -> Result<Tokenizer, String> {
        let gguf = GgufFile::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let vocab = LlamaVocab::from_gguf(&gguf).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Tokenizer { vocab, file: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() })
    }

    /// A Qwen GGUF in the weights folder (`llm/Qwen*.gguf`, not a vision
    /// projector), the smallest first: they share one vocabulary.
    pub fn find() -> Option<PathBuf> {
        let mut roots = Vec::new();
        if let Some(dir) = std::env::var_os("MAKEPAD_ASSET_AI_CACHE") {
            roots.push(PathBuf::from(dir));
        }
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join(".makepad").join("weights"));
        }
        let mut found: Vec<(u64, PathBuf)> = Vec::new();
        for root in roots {
            let Ok(read) = std::fs::read_dir(root.join("llm")) else { continue };
            for e in read.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with("Qwen") && name.ends_with(".gguf") && !name.contains("mmproj") {
                    let size = std::fs::metadata(e.path()).map(|m| m.len()).unwrap_or(u64::MAX);
                    found.push((size, e.path()));
                }
            }
        }
        found.sort();
        found.into_iter().next().map(|f| f.1)
    }

    pub fn pieces(&self) -> usize {
        self.vocab.len()
    }

    pub fn count(&self, text: &str) -> usize {
        self.vocab.tokenize(text, false, true).map_or_else(|_| text.len() / 3, |t| t.len())
    }
}

/// The empty think block of an answer without reasoning.
pub const NO_THINK: &str = "<think>\n\n</think>\n\n";

/// The chat as the model reads it in training.
pub fn chat_text(system: &str, user: &str, assistant: &str) -> String {
    format!("<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{user}<|im_end|>\n<|im_start|>assistant\n{NO_THINK}{assistant}<|im_end|>\n")
}

/// The prompt part (everything before the answer the loss is on).
pub fn prompt_text(system: &str, user: &str) -> String {
    format!("<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{user}<|im_end|>\n<|im_start|>assistant\n{NO_THINK}")
}

/// Token counts of a task: examples, totals and a histogram.
#[derive(Clone, Debug, Default)]
pub struct Tally {
    pub examples: u64,
    pub tokens: u64,
    /// Tokens of the answers (the ones trained on).
    pub target: u64,
    pub lengths: Vec<u32>,
}

/// Histogram bucket edges (tokens per example).
pub const EDGES: [u32; 8] = [256, 512, 1024, 2048, 4096, 8192, 16384, 32768];

impl Tally {
    pub fn add(&mut self, total: usize, target: usize) {
        self.examples += 1;
        self.tokens += total as u64;
        self.target += target as u64;
        self.lengths.push(total as u32);
    }

    pub fn merge(&mut self, o: &Tally) {
        self.examples += o.examples;
        self.tokens += o.tokens;
        self.target += o.target;
        self.lengths.extend_from_slice(&o.lengths);
    }

    /// Counts per bucket: `<= EDGES[i]`, the last for longer.
    pub fn histogram(&self) -> Vec<u64> {
        let mut h = vec![0u64; EDGES.len() + 1];
        for l in &self.lengths {
            h[EDGES.iter().position(|e| l <= e).unwrap_or(EDGES.len())] += 1;
        }
        h
    }

    pub fn quantile(&self, q: f64) -> u32 {
        if self.lengths.is_empty() {
            return 0;
        }
        let mut v = self.lengths.clone();
        v.sort_unstable();
        v[((v.len() - 1) as f64 * q).round() as usize]
    }
}
