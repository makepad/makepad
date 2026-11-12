use makepad_dynamics::collision_sdf::box_projection;
use makepad_dynamics::sdf::Sdf;
use makepad_dynamics::sdf_torus::Torus;

#[test]
fn torus_field_and_box_projection() {
    let torus = Torus {
        radius: [0.375, 0.125],
    };
    assert_eq!(torus.distance(&[0.375, 0.0, 0.0]), -0.125, "tube center");
    assert_eq!(torus.distance(&[0.5, 0.0, 0.0]), 0.0, "outer surface");
    let mut gradient = [0.0; 3];
    torus.gradient(&mut gradient, &[0.625, 0.0, 0.0]);
    assert_eq!(gradient, [1.0, 0.0, 0.0], "outer radial gradient");

    let box_ = [1.0, 2.0, 3.0, 1.0, 2.0, 3.0];
    let mut point = [1.0, 2.0, 3.0];
    assert_eq!(box_projection(&mut point, &box_), -1.0, "interior distance");
    assert_eq!(point, [1.0, 2.0, 3.0], "interior unchanged");
    point = [2.0, 2.0, 3.0];
    assert_eq!(box_projection(&mut point, &box_), 0.0, "boundary distance");
    assert_eq!(point, [2.0, 2.0, 3.0], "boundary unchanged");
    point = [5.0, -4.0, 3.0];
    assert_eq!(
        box_projection(&mut point, &box_),
        5.0,
        "3-4-5 exterior distance"
    );
    assert_eq!(
        point,
        [5.0 - (3.0 + 1e-6), -4.0 + (4.0 + 1e-6), 3.0],
        "exterior projected inward"
    );
}

/// The half-space below z = 0, its field reading 1.02 times the distance (as a sampled field's error may).
struct Below;

impl Sdf for Below {
    fn distance(&self, point: &[f64; 3]) -> f64 {
        1.02 * point[2]
    }
    fn gradient(&self, gradient: &mut [f64; 3], _point: &[f64; 3]) {
        *gradient = [0.0, 0.0, 1.02];
    }
}

/// The half-space above z = 0.1 um, its field the distance.
struct Above;

impl Sdf for Above {
    fn distance(&self, point: &[f64; 3]) -> f64 {
        1e-4 - point[2]
    }
    fn gradient(&self, gradient: &mut [f64; 3], _point: &[f64; 3]) {
        *gradient = [0.0, 0.0, -1.0];
    }
}

/// Two faces 0.1 um apart, one field reading 2% over its distance: every walk settles where the two read level,
/// at that gap, while a carried touch 0.05 mm under the lower face reads an overlap of 0.9 um. The walks alone
/// list no contact; with the touch carried, the pair keeps it, at the seed's own read.
#[test]
fn a_carried_touch_that_still_reads_an_overlap_stays_listed() {
    use makepad_dynamics::builder::compile;
    use makepad_dynamics::forward::fwd_position;
    use makepad_dynamics::io::make_data;
    use makepad_dynamics::model_spec::*;
    use makepad_dynamics::sdf::attach;
    use makepad_dynamics::types::*;
    use std::sync::Arc;

    let field = |name: &str| GeomSpec { name: Some(name.into()), r#type: GEOM_BOX, size: [5.0, 5.0, 1.0], ..GeomSpec::default() };
    let mut spec = ModelSpec::default();
    // one start of the reference's per cube of the thinner box's thickness: z at -1, -0.6, -0.2, 0.2 and 0.6
    spec.option.sdf_initpoints = 1;
    spec.bodies[0].geoms.push(field("below"));
    spec.bodies.push(BodySpec {
        name: Some("above".into()), ipos: [0.0; 3], mass: 1.0, inertia: [1.0; 3],
        joints: vec![JointSpec { r#type: JNT_FREE, ..JointSpec::default() }],
        geoms: vec![field("above")],
        ..BodySpec::default()
    });
    let mut m = compile(&spec).expect("compile the two fields");
    attach(&mut m, 0, Arc::new(Below));
    attach(&mut m, 1, Arc::new(Above));
    let mut d = make_data(&m);
    fwd_position(&m, &mut d);
    assert_eq!(d.ncon, 0, "the walks settle where both fields read level, 0.1 um apart: no contact");
    d.sdf_seeds = vec![(0, 1, [0.0, 0.0, -0.05])];
    fwd_position(&m, &mut d);
    assert_eq!(d.ncon, 1, "the carried touch whose own read overlaps stays listed");
    let c = &d.contact[0];
    assert!((c.dist + 0.0009).abs() < 1e-12, "at the seed's read, 0.0501 mm less 1.02 x 0.05 mm: {}", c.dist);
    assert!(c.frame[2].abs() > 1.0 - 1e-12, "along the faces' normal: {:?}", &c.frame[..3]);
    assert_eq!(d.sdf_seeds.len(), 1, "carried on to the next collision: {:?}", d.sdf_seeds);
}
