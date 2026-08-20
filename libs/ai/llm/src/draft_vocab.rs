//! The restricted output vocabulary of the MTP draft head.
//!
//! A draft step reads ~1.32 GB of weights on Qwen3.8-27B and **1.03 GB of that
//! is the LM head** — 78 % of the cost of proposing one token, spent producing
//! logits for 248320 tokens the model will essentially never emit. Restricting
//! the *draft* head to the tokens the model actually produces cuts a draft step
//! to roughly a third without touching verification: the verify pass keeps the
//! full head, so a draft for a token outside the set is simply a rejection and
//! every losslessness/determinism property is unchanged.
//!
//! The set is built from the model's own outputs (generate a corpus, count
//! token frequencies, keep the smallest prefix covering `coverage` of the mass)
//! and stored as a versioned sidecar next to the gguf, so a session is
//! reproducible from the file alone.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::error::{LlamaError, Result};

/// "MKDV", little-endian.
const MAGIC: u32 = 0x5644_4b4d;
const VERSION: u32 = 1;
const HEADER_BYTES: usize = 4 * 8;

/// Draft ids are dense `0..ids.len()`; `ids[draft_id]` is the real token id.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DraftVocab {
    /// Full vocabulary the model was built with, so a mismatched sidecar is
    /// rejected instead of silently mapping to the wrong tokens.
    pub vocab_size: u32,
    /// Fraction of corpus token occurrences the kept set covers.
    pub coverage_permille: u32,
    /// Token occurrences counted while building the set.
    pub corpus_tokens: u64,
    /// Kept real token ids, ascending and unique.
    pub ids: Vec<i32>,
}

impl DraftVocab {
    /// `<model>.draftvocab`, next to the gguf.
    pub fn sidecar_path(model_path: &Path) -> PathBuf {
        let mut name = model_path.as_os_str().to_os_string();
        name.push(".draftvocab");
        PathBuf::from(name)
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn coverage(&self) -> f64 {
        self.coverage_permille as f64 / 1000.0
    }

    /// Real token id for a draft-head output index.
    pub fn real_token(&self, draft_id: usize) -> Result<i32> {
        self.ids.get(draft_id).copied().ok_or_else(|| {
            LlamaError::format(format!(
                "draft id {draft_id} is outside the {}-entry draft vocabulary",
                self.ids.len()
            ))
        })
    }

    /// Keep the smallest set of ids covering `target_coverage` of `counts`,
    /// always including `required` (the stop tokens — a draft head that cannot
    /// propose EOS turns every end-of-turn into a forced rejection), then pad
    /// up to a multiple of `align` with the next most frequent ids.
    ///
    /// Ties break on the lower token id, so the same corpus always yields the
    /// same set.
    pub fn select(
        counts: &[u64],
        target_coverage: f64,
        required: &[i32],
        align: usize,
    ) -> Result<Self> {
        if counts.is_empty() {
            return Err(LlamaError::format("draft vocabulary needs a non-empty corpus histogram"));
        }
        let total: u64 = counts.iter().sum();
        if total == 0 {
            return Err(LlamaError::format("draft vocabulary corpus counted zero tokens"));
        }
        let target = target_coverage.clamp(0.0, 1.0);

        let mut order: Vec<u32> = (0..counts.len() as u32).collect();
        order.sort_unstable_by(|a, b| {
            counts[*b as usize]
                .cmp(&counts[*a as usize])
                .then_with(|| a.cmp(b))
        });

        let mut kept: BTreeSet<i32> = BTreeSet::new();
        for &token in required {
            if (token as usize) < counts.len() && token >= 0 {
                kept.insert(token);
            }
        }
        let mut covered: u64 = kept
            .iter()
            .map(|token| counts[*token as usize])
            .sum();
        let mut cursor = 0usize;
        while (covered as f64) < target * total as f64 && cursor < order.len() {
            let token = order[cursor] as i32;
            cursor += 1;
            if kept.insert(token) {
                covered += counts[token as usize];
            }
        }
        // Pad to the alignment with the next most frequent ids so the head's
        // row count is kernel-friendly; padding only ever adds coverage.
        let align = align.max(1);
        while kept.len() % align != 0 && cursor < order.len() {
            let token = order[cursor] as i32;
            cursor += 1;
            if kept.insert(token) {
                covered += counts[token as usize];
            }
        }

        let vocab_size = u32::try_from(counts.len())
            .map_err(|_| LlamaError::format("vocabulary size does not fit in u32"))?;
        let coverage_permille = ((covered as f64 / total as f64) * 1000.0).round() as u32;
        Ok(Self {
            vocab_size,
            coverage_permille: coverage_permille.min(1000),
            corpus_tokens: total,
            ids: kept.into_iter().collect(),
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_BYTES + self.ids.len() * 4);
        out.extend_from_slice(&MAGIC.to_le_bytes());
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&self.vocab_size.to_le_bytes());
        out.extend_from_slice(&(self.ids.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.coverage_permille.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // reserved
        out.extend_from_slice(&self.corpus_tokens.to_le_bytes());
        for id in &self.ids {
            out.extend_from_slice(&id.to_le_bytes());
        }
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < HEADER_BYTES {
            return Err(LlamaError::format("draft vocabulary sidecar is truncated"));
        }
        let u32_at = |offset: usize| {
            u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ])
        };
        if u32_at(0) != MAGIC {
            return Err(LlamaError::format("draft vocabulary sidecar has a bad magic"));
        }
        let version = u32_at(4);
        if version != VERSION {
            return Err(LlamaError::format(format!(
                "draft vocabulary sidecar version {version} is not {VERSION}"
            )));
        }
        let vocab_size = u32_at(8);
        let count = u32_at(12) as usize;
        let coverage_permille = u32_at(16);
        let corpus_tokens = u64::from_le_bytes([
            bytes[24], bytes[25], bytes[26], bytes[27], bytes[28], bytes[29], bytes[30], bytes[31],
        ]);
        let want = HEADER_BYTES + count * 4;
        if bytes.len() < want {
            return Err(LlamaError::format(format!(
                "draft vocabulary sidecar holds {} bytes, needs {want}",
                bytes.len()
            )));
        }
        let mut ids = Vec::with_capacity(count);
        let mut previous = -1i32;
        for index in 0..count {
            let offset = HEADER_BYTES + index * 4;
            let id = i32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]);
            if id <= previous {
                return Err(LlamaError::format(
                    "draft vocabulary sidecar ids are not strictly ascending",
                ));
            }
            if id < 0 || id as u32 >= vocab_size {
                return Err(LlamaError::format(format!(
                    "draft vocabulary sidecar id {id} is outside the vocabulary"
                )));
            }
            previous = id;
            ids.push(id);
        }
        Ok(Self {
            vocab_size,
            coverage_permille,
            corpus_tokens,
            ids,
        })
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        std::fs::write(path, self.to_bytes())
            .map_err(|err| LlamaError::format(format!("writing {}: {err}", path.display())))
    }

    pub fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .map_err(|err| LlamaError::format(format!("reading {}: {err}", path.display())))?;
        Self::from_bytes(&bytes)
    }

    /// Load the sidecar beside `model_path`, if it is there and matches.
    pub fn load_for_model(model_path: &Path, vocab_size: u32) -> Result<Option<Self>> {
        let path = Self::sidecar_path(model_path);
        if !path.exists() {
            return Ok(None);
        }
        let vocab = Self::read(&path)?;
        if vocab.vocab_size != vocab_size {
            return Err(LlamaError::format(format!(
                "{} was built for a {}-token vocabulary, model has {vocab_size}",
                path.display(),
                vocab.vocab_size
            )));
        }
        if vocab.ids.is_empty() {
            return Err(LlamaError::format(format!("{} is empty", path.display())));
        }
        Ok(Some(vocab))
    }
}

#[cfg(test)]
mod tests {
    use super::DraftVocab;

    #[test]
    fn selection_keeps_the_smallest_prefix_reaching_coverage() {
        // 100 occurrences: 60 + 30 + 6 + 4.
        let counts = vec![60, 30, 6, 4];
        let vocab = DraftVocab::select(&counts, 0.9, &[], 1).unwrap();
        assert_eq!(vocab.ids, vec![0, 1]);
        assert_eq!(vocab.coverage_permille, 900);
        assert_eq!(vocab.corpus_tokens, 100);
    }

    #[test]
    fn required_tokens_are_always_kept() {
        let counts = vec![60, 30, 6, 4];
        // Token 3 is the rarest and would never make a 90% cut on its own.
        let vocab = DraftVocab::select(&counts, 0.9, &[3], 1).unwrap();
        assert!(vocab.ids.contains(&3));
        assert!(vocab.coverage_permille >= 900);
    }

    #[test]
    fn selection_pads_to_the_alignment_and_stays_ascending() {
        let counts = vec![60, 30, 6, 4];
        let vocab = DraftVocab::select(&counts, 0.5, &[], 4).unwrap();
        assert_eq!(vocab.ids.len(), 4);
        assert!(vocab.ids.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn selection_is_deterministic_under_ties() {
        let counts = vec![10, 10, 10, 10];
        let first = DraftVocab::select(&counts, 0.5, &[], 1).unwrap();
        let second = DraftVocab::select(&counts, 0.5, &[], 1).unwrap();
        assert_eq!(first, second);
        // Ties break on the lower id.
        assert_eq!(first.ids, vec![0, 1]);
    }

    #[test]
    fn roundtrip_through_the_sidecar_format() {
        let counts = vec![60, 30, 6, 4];
        let vocab = DraftVocab::select(&counts, 0.97, &[], 2).unwrap();
        let bytes = vocab.to_bytes();
        assert_eq!(DraftVocab::from_bytes(&bytes).unwrap(), vocab);
    }

    #[test]
    fn corrupt_sidecars_are_rejected() {
        let counts = vec![60, 30, 6, 4];
        let vocab = DraftVocab::select(&counts, 0.97, &[], 1).unwrap();
        let mut bytes = vocab.to_bytes();
        bytes[0] ^= 0xff;
        assert!(DraftVocab::from_bytes(&bytes).is_err());

        let mut bytes = vocab.to_bytes();
        bytes[4] = 9; // version
        assert!(DraftVocab::from_bytes(&bytes).is_err());

        let bytes = vocab.to_bytes();
        assert!(DraftVocab::from_bytes(&bytes[..bytes.len() - 2]).is_err());
    }

    #[test]
    fn real_token_maps_draft_ids_back() {
        let counts = vec![1, 5, 1, 9];
        let vocab = DraftVocab::select(&counts, 0.8, &[], 1).unwrap();
        assert_eq!(vocab.ids, vec![1, 3]);
        assert_eq!(vocab.real_token(0).unwrap(), 1);
        assert_eq!(vocab.real_token(1).unwrap(), 3);
        assert!(vocab.real_token(2).is_err());
    }
}
