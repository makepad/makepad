use super::*;

fn bounds(span: f32) -> (Vec3f, Vec3f) {
    (vec3f(0.0, 0.0, 0.0), vec3f(span, span * 0.5, span))
}

#[test]
fn a_prop_casts_and_a_level_does_not() {
    // A fence, a shed, a kit building: all well under the span.
    for span in [1.0, 4.0, 20.0, 39.9] {
        let (lo, hi) = bounds(span);
        assert!(
            casts_as_caster_only(None, false, lo, hi),
            "a {span} m prop must still cast"
        );
    }
    // A Doom/Duke map arrives as one static hundreds of metres across.
    for span in [40.1, 200.0, 4000.0] {
        let (lo, hi) = bounds(span);
        assert!(
            !casts_as_caster_only(None, false, lo, hi),
            "a {span} m level must not shadow its own interior"
        );
    }
}

#[test]
fn a_prelit_model_never_casts() {
    let (lo, hi) = bounds(3.0);
    assert!(!casts_as_caster_only(None, true, lo, hi));
}

/// The host's answer beats the heuristic in both directions.
#[test]
fn an_explicit_answer_wins() {
    let (big_lo, big_hi) = bounds(500.0);
    let (small_lo, small_hi) = bounds(2.0);
    assert!(casts_as_caster_only(Some(true), false, big_lo, big_hi));
    assert!(casts_as_caster_only(Some(true), true, big_lo, big_hi));
    assert!(!casts_as_caster_only(Some(false), false, small_lo, small_hi));
}

/// Setting it re-kicks the bake, and setting the same answer twice does
/// not — the bake is expensive and the signature is what gates it.
#[test]
fn changing_the_answer_re_kicks_the_bake() {
    let mut renderer = Renderer::default();
    renderer.placed_scene_signature = Some(7);
    let rev = renderer.models_rev;
    renderer.set_model_casts_shadow("maps/e1m1", false);
    assert_eq!(renderer.placed_scene_signature, None);
    assert_eq!(renderer.models_rev, rev.wrapping_add(1));

    renderer.placed_scene_signature = Some(9);
    renderer.set_model_casts_shadow("maps/e1m1", false);
    assert_eq!(renderer.placed_scene_signature, Some(9), "no needless re-kick");
    assert_eq!(renderer.models_rev, rev.wrapping_add(1));
}
