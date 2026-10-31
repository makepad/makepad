use super::*;

#[test]
fn entity_shell_flag_draws_nothing_at_all() {
    let ordinary = Entity::default();
    let shell = Entity { suppress_primitive: true, ..Default::default() };
    let sensor_shell = Entity { suppress_primitive: true, alpha_primitive: true, ..Default::default() };

    assert_eq!(primitive_bucket(&ordinary), Some(PrimitiveBucket::Opaque));
    // Invisible containment: an interior is an open stage.
    assert_eq!(primitive_bucket(&shell), None);
    // Existing sensor alpha semantics win for the nonsensical combined
    // spelling.
    assert_eq!(primitive_bucket(&sensor_shell), Some(PrimitiveBucket::Alpha));
}
