//! Asset annotation pass: turntable sheet -> vision model -> queryable store
//! metadata.
//!
//! # What the pass owns
//!
//! Exactly two things on an asset's annotation record:
//!
//! * `description` — replaced with one dense construction line (target ≤ 20
//!   tokens) so that a 30-row SQL result stays a few hundred tokens in the
//!   calling chat model's 8k context.
//! * tags prefixed [`VLM_PREFIX`] — the structured facets a level builder
//!   filters on.
//!
//! Everything else on the record (title, kind, categories, creator,
//! generator/backend/model, prompt, provenance, visibility, and every tag
//! without the prefix) is carried through untouched. The store's annotation
//! route is a whole-record PUT, so "carried through" means the pass reads the
//! current record and writes it back with only its own fields recomputed.
//!
//! # Replace semantics
//!
//! [`plan_upload`] recomputes the owned fields FROM SCRATCH on every run: the
//! description is overwritten and the tag set is rebuilt as
//! `(previous tags without the prefix) + (this run's facets)`. No previous
//! value of an owned field can survive a re-run, which is what makes the
//! eventual "annotate with 3.5 now, hard-replace with 3.8 later" a plain
//! re-run rather than a migration. There is deliberately no compatibility
//! shim and no multi-version coexistence logic — the only versioning is the
//! `vlm-v<N>` tag, used to skip already-current assets and to invalidate
//! everything when the prompt or the model changes.

pub mod parse;
pub mod plan;
pub mod sheet;

pub use parse::{parse_record, Record};
pub use plan::{needs_annotation, plan_upload, Annotator, BaseAnnotation, Upload, VLM_PREFIX};

/// Bump when the prompt or the output contract changes: every asset tagged
/// with an older version is re-annotated and its owned fields replaced.
///
/// v3: annotator moves from the local Metal Qwen3.5-9B tower to the
/// Qwen3.8-27B vision pipe on the 5090 (arch-identical tower, projection
/// 4096→5120), and the per-job context now frames the KIT (Kenney set,
/// low-poly construction kit, grid-snapping pieces) alongside the piece
/// name.
///
/// v4: the person-description variant (PROMPT_PERSON, generic observation
/// categories — never per-set feature lists) and the subject zoom
/// (`sheet::zoom_to_subject`): each turntable cell is cropped onto its
/// subject before downscaling, which is what turned "a wooden staff" back
/// into a police officer's bare hand. desc budget 10 → 20 words.
pub const ANNOTATOR_VERSION: u32 = 4;

/// The question put to the vision model about one turntable sheet.
///
/// This prompt is the iterated artifact of the pass. It asks for level
/// construction facts (what the piece is in kit terms, how it connects, which
/// way it faces) rather than a caption, and it fixes a strict key/value reply
/// shape because a 9B model follows that far more reliably than JSON.
pub const PROMPT: &str = include_str!("prompt.txt");

/// The person-description variant, for `kind='character'` assets: the same
/// strict reply shape (so the parser and the plan are shared), but the
/// questions are about the PERSON — age impression, hair/baldness, facial
/// hair, clothing colours, accessories — so "the old man with the beard"
/// becomes a description LIKE match (play-session-1, character lane).
pub const PROMPT_PERSON: &str = include_str!("prompt_person.txt");
