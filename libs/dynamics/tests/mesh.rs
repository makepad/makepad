use makepad_dynamics::builder::compile;
use makepad_dynamics::forward::step;
use makepad_dynamics::io::make_data;
use makepad_dynamics::model_spec::*;
use makepad_dynamics::types::*;

// Millimetres, grams and seconds, as the builder fixtures. The mesh is the eight corners of a box
// centred on the origin; the compiler builds its hull, so its geom is a convex mesh.
fn world(blocks: &[([f64; 3], [f64; 4])]) -> ModelSpec {
    let mut spec = ModelSpec { modelname: Some("blocks".into()), ..ModelSpec::default() };
    spec.option.gravity = [0.0, 0.0, -9810.0];
    spec.option.timestep = 1.0 / 240.0;
    let mut vert = Vec::new();
    for i in 0..8 {
        vert.extend([if i & 1 != 0 { 20.0 } else { -20.0 }, if i & 2 != 0 { 15.0 } else { -15.0 }, if i & 4 != 0 { 10.0 } else { -10.0 }]);
    }
    spec.meshes.push(MeshSpec { name: Some("block".into()), vert, ..MeshSpec::default() });
    spec.bodies[0].geoms.push(GeomSpec { r#type: GEOM_PLANE, size: [500.0, 500.0, 10.0], ..GeomSpec::default() });
    for (i, &(pos, quat)) in blocks.iter().enumerate() {
        spec.bodies.push(BodySpec {
            name: Some(format!("block{i}")), pos, quat, ipos: [0.0; 3], mass: 80.0,
            inertia: [8666.0, 13333.0, 16666.0],
            joints: vec![JointSpec { r#type: JNT_FREE, ..JointSpec::default() }],
            geoms: vec![GeomSpec {
                r#type: GEOM_MESH, meshname: Some("block".into()), ..GeomSpec::default()
            }],
            ..BodySpec::default()
        });
    }
    spec
}

// The body's local z axis in the world, from the rotation matrix of the body.
fn up(xmat: &[f64], body: usize) -> f64 { xmat[9 * body + 8] }

#[test]
fn box_mesh_dropped_comes_to_rest_flat() {
    let angle: f64 = 0.25;
    let (s, c) = ((angle / 2.0).sin(), (angle / 2.0).cos());
    let r = std::f64::consts::FRAC_1_SQRT_2;
    let m = compile(&world(&[([0.0, 0.0, 40.0], [c, s * r, s * r, 0.0])])).expect("compile the box mesh");
    assert_eq!(m.geom_type[1], GEOM_MESH, "the block's geom is a mesh");
    let mut d = make_data(&m);
    for _ in 0..480 { step(&m, &mut d); }
    assert!(up(&d.xmat, 1) > 0.9999, "the box lies flat: its z axis is {} of vertical", up(&d.xmat, 1));
    assert!(d.xpos[5] > 6.0 && d.xpos[5] < 10.5, "the box rests on its face: centre height {}", d.xpos[5]);
    let speed = d.qvel[..6].iter().fold(0.0_f64, |a, v| a.max(v.abs()));
    assert!(speed < 1.0, "the box is at rest: largest velocity {speed}");
}

#[test]
fn box_meshes_stacked_stay_stacked() {
    let upright = [1.0, 0.0, 0.0, 0.0];
    let m = compile(&world(&[([0.0, 0.0, 10.0], upright), ([0.0, 0.0, 30.5], upright)])).expect("compile the stack");
    let mut d = make_data(&m);
    for _ in 0..480 { step(&m, &mut d); }
    let (low, high) = (&d.xpos[3..6], &d.xpos[6..9]);
    assert!((high[2] - low[2] - 20.0).abs() < 3.0, "the upper box rests on the lower: {} apart", high[2] - low[2]);
    assert!((high[0] - low[0]).abs() < 1.0 && (high[1] - low[1]).abs() < 1.0, "the upper box has not slid off");
    assert!(low[0].abs() < 1.0 && low[1].abs() < 1.0, "the lower box has not moved sideways");
    assert!(up(&d.xmat, 1) > 0.9999 && up(&d.xmat, 2) > 0.9999, "both boxes are upright");
}
