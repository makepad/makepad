//! The pipeline PRESENTATION vocabulary that outlived the wire (aicore P7):
//! the shared stage-weight table and the aggregate arithmetic every card
//! uses, plus the id spelling. The wire tests left with the store's
//! pipeline routes — runs are drawn from the app's own engine now.

use makepad_asset_client::{
    aggregate_permille, default_stage_weight, PipelineId, DEFAULT_STAGE_WEIGHTS,
    NEUTRAL_STAGE_WEIGHT,
};

/// Every published weight resolves through the ONE shared table, and an
/// unknown kind lands on the neutral weight rather than zero (a zero would
/// erase the stage from the bar).
#[test]
fn the_default_weights_are_one_shared_table() {
    for (kind, weight) in DEFAULT_STAGE_WEIGHTS {
        assert_eq!(default_stage_weight(kind), *weight, "{kind}");
        assert!(*weight > 0, "{kind}");
    }
    assert_eq!(default_stage_weight("nonsense.kind"), NEUTRAL_STAGE_WEIGHT);
}

/// The aggregate is the weighted mean, bounded 0..=1000, and an empty run
/// is 0 rather than a division by nothing.
#[test]
fn the_aggregate_is_a_weighted_mean() {
    assert_eq!(aggregate_permille([].into_iter()), 0);
    assert_eq!(aggregate_permille([(1u16, 1000u16)].into_iter()), 1000);
    assert_eq!(aggregate_permille([(1, 0), (1, 1000)].into_iter()), 500);
    // Weights bias the bar toward the expensive stage.
    assert_eq!(aggregate_permille([(1, 0), (3, 1000)].into_iter()), 750);
}

#[test]
fn a_pipeline_id_is_exactly_its_transport_spelling() {
    let id = PipelineId::parse("pipe_0123456789abcdef0123456789abcdef").expect("parse");
    assert_eq!(id.to_string(), "pipe_0123456789abcdef0123456789abcdef");
    assert!(PipelineId::parse("pipe_short").is_none());
    assert!(PipelineId::parse("job_0123456789abcdef0123456789abcdef").is_none());
}
