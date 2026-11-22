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
    // A prop's footprint joins the baked ground light field, padded so the
    // shadow it casts beside it lands inside.
    use super::lights::{ground_field_window, GROUND_FIELD_MAX_SPAN};
    let (lo, hi) = bounds(4.0);
    assert_eq!(ground_field_window([(lo, hi)]), Some((-6.0, -6.0, 16.0, 16.0)));
    assert_eq!(ground_field_window(std::iter::empty()), None);
    // Statics spread past the field's span: it centres on the bulk of their
    // footprint (a town), not on the low corner a lone prop far off sets.
    let town = (vec3f(0.0, 0.0, 0.0), vec3f(100.0, 5.0, 100.0));
    let lone = (vec3f(-900.0, 0.0, 0.0), vec3f(-899.0, 1.0, 1.0));
    let (x0, z0, span, full) = ground_field_window([lone, town]).unwrap();
    assert_eq!(span, GROUND_FIELD_MAX_SPAN);
    assert!(full > span, "the cap bit: {full}");
    assert!(x0 <= 0.0 && x0 + span >= 100.0, "the town stays inside along x: {x0}");
    assert!(z0 <= 0.0 && z0 + span >= 100.0, "and along z: {z0}");
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

#[test]
fn a_known_solid_casts_above_the_fallback_span() {
    let (lo, hi) = bounds(CASTER_ONLY_MAX_SPAN + 1.0);
    assert!(!casts_as_caster_only(None, false, lo, hi));

    let mut renderer = Renderer::default();
    let id = "gen/surface/solid";
    renderer.set_model_casts_shadow(id, true);
    assert!(casts_as_caster_only(
        renderer.model_casts_shadow.get(id).copied(),
        false,
        lo,
        hi,
    ));
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
