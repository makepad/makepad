use makepad_asset_client::ChatProviderKind;
use makepad_score::model::{
    Affinity, AnchorTarget, Annotation, AnnotationBody, AnnotationId, AnnotationKind,
    AnnotationLayer, AnnotationStyle, BeatRange, ContextFingerprint, ExportPolicy, Id,
    LayerId, LayerPermissions, LayerScope, Score, SemanticAnchor,
};
use std::fmt;

const PROVENANCE_ACTOR: u64 = 0x7363_6f72_6561_6921;
pub const PROVENANCE_LAYER_TITLE: &str = "AI score provenance";
pub const PROVENANCE_KIND: &str = "makepad-score-ai/provenance-v1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenerationProvenance {
    pub provider: ChatProviderKind,
    pub prompt: String,
    pub attempt: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProvenanceError {
    ScoreHasNoMeasure,
    ScoreHasNoStaff,
    IdSpaceExhausted,
    TimeOverflow,
}

impl fmt::Display for ProvenanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ScoreHasNoMeasure => f.write_str("cannot anchor provenance: score has no measure"),
            Self::ScoreHasNoStaff => f.write_str("cannot anchor provenance: score has no staff"),
            Self::IdSpaceExhausted => f.write_str("cannot allocate provenance annotation id"),
            Self::TimeOverflow => f.write_str("cannot anchor provenance: score time overflow"),
        }
    }
}

impl std::error::Error for ProvenanceError {}

/// Records provenance using ordinary semantic annotation entities. The layer
/// is editable and removable like any hand-authored analysis layer; the score
/// itself remains the standard model type.
pub fn record_provenance(
    score: &mut Score,
    provenance: &GenerationProvenance,
) -> Result<AnnotationId, ProvenanceError> {
    let measure = score
        .measures
        .values()
        .min_by_key(|measure| (measure.start, measure.ordinal, measure.id))
        .ok_or(ProvenanceError::ScoreHasNoMeasure)?;
    let staff = score
        .staves
        .values()
        .min_by_key(|staff| staff.id)
        .ok_or(ProvenanceError::ScoreHasNoStaff)?;
    let end = measure
        .start
        .checked_add(measure.extent)
        .map_err(|_| ProvenanceError::TimeOverflow)?;

    let layer_id = next_layer_id(score)?;
    score.annotation_layers.insert(
        layer_id,
        AnnotationLayer {
            id: layer_id,
            title: PROVENANCE_LAYER_TITLE.to_string(),
            owner: score.score_id,
            color_hint: [96, 112, 140, 255],
            visible_by_default: false,
            scope: LayerScope::AllScore,
            permissions: LayerPermissions::Collaborative,
            export_policy: ExportPolicy::Exclude,
        },
    );
    let annotation_id = next_annotation_id(score)?;
    let body = format!(
        "{PROVENANCE_KIND}\nprovider={}\nattempt={}\nprompt-bytes={}\nprompt:\n{}",
        provenance.provider.as_str(),
        provenance.attempt,
        provenance.prompt.len(),
        provenance.prompt
    );
    score.annotations.insert(
        annotation_id,
        Annotation {
            id: annotation_id,
            layer: layer_id,
            kind: AnnotationKind::Analysis,
            anchor: SemanticAnchor {
                primary: AnchorTarget::Measure(measure.id),
                fallback: BeatRange {
                    staff: staff.id,
                    voice: None,
                    start: measure.start,
                    end,
                },
                affinity: Affinity::On,
                context_fingerprint: ContextFingerprint([0; 16]),
                ink: None,
            },
            body: AnnotationBody::Text(body),
            style: AnnotationStyle {
                color_rgba: [96, 112, 140, 255],
                width_milli_staff_space: 1000,
            },
            action: None,
            author: score.score_id,
            created_lamport: u64::from(provenance.attempt),
            modified_lamport: u64::from(provenance.attempt),
        },
    );
    Ok(annotation_id)
}

fn next_layer_id(score: &Score) -> Result<LayerId, ProvenanceError> {
    (1..=u64::MAX)
        .map(|counter| Id::new(PROVENANCE_ACTOR, counter))
        .find(|id| !score.annotation_layers.contains_key(id))
        .ok_or(ProvenanceError::IdSpaceExhausted)
}

fn next_annotation_id(score: &Score) -> Result<AnnotationId, ProvenanceError> {
    (1..=u64::MAX)
        .map(|counter| Id::new(PROVENANCE_ACTOR, counter))
        .find(|id| !score.annotations.contains_key(id))
        .ok_or(ProvenanceError::IdSpaceExhausted)
}
