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
