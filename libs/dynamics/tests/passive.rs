use makepad_dynamics::passive::{added_mass_forces, read_fluid_geom_interaction, viscous_forces};
use makepad_dynamics::types::PI;

fn close(actual: &[f64; 6], expected: &[f64; 6]) {
    for i in 0..6 {
        assert!(
            (actual[i] - expected[i]).abs() <= 1e-12,
            "component {i}: {} != {}",
            actual[i],
            expected[i]
        );
    }
}

#[test]
fn added_mass_momentum_and_acceleration() {
    // C: p = rho M v, L = rho I w; F += p x w;
    // torque += p x v + L x w. Here p=(16,-12,10), L=(14,44,78),
    // p x w=(-56,-38,44), p x v=(8,24,16), L x w=(-24,36,-16).
    let velocity = [1.0, 2.0, 3.0, 4.0, -2.0, 1.0];
    let mass = [2.0, 3.0, 5.0];
    let inertia = [7.0, 11.0, 13.0];
    let mut force = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    added_mass_forces(&velocity, None, 2.0, &mass, &inertia, &mut force);
    close(&force, &[-15.0, 62.0, 3.0, -52.0, -33.0, 50.0]);

    // Optional acceleration subtracts rho I alpha and rho M a first.
    // Its contribution is (-7,22,-52,8,-18,40).
    let mut force = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    added_mass_forces(
        &velocity,
        Some(&[0.5, -1.0, 2.0, -2.0, 3.0, -4.0]),
        2.0,
        &mass,
        &inertia,
        &mut force,
    );
    close(&force, &[-22.0, 84.0, -49.0, -44.0, -51.0, 90.0]);
}

#[test]
fn viscous_stokes_drag_and_lift() {
    // Unit sphere: D=2, volume=4pi/3, A_proj=A_max=pi, I=8pi/15.
    // rho=2, mu=1/2, |v|=5: linear drag coefficient = 3pi+3pi=6pi.
    // Magnus = (1/4)*rho*volume*(w x v) = (-16pi/3,4pi,0).
    // Kutta vanishes because the sphere's projection normal is parallel to v.
    // Angular coefficient = 4pi + 2*(2*(3/5)*(8pi/15)) = 132pi/25.
    let mut force = [0.0; 6];
    viscous_forces(
        &[0.0, 0.0, 2.0, 3.0, 4.0, 0.0],
        2.0,
        0.5,
        &[1.0; 3],
        0.25,
        0.7,
        0.3,
        0.4,
        0.6,
        &mut force,
    );
    close(
        &force,
        &[
            0.0,
            0.0,
            -264.0 * PI / 25.0,
            -70.0 * PI / 3.0,
            -20.0 * PI,
            0.0,
        ],
    );

    // Ellipsoid (2,1,1), v=(1,1,0): normalized projection coefficients
    // a=2/5, b=c=8/5, proj_denom=34/25, A_proj=pi*sqrt(17/5).
    // (normal x v) x v = (6/(5sqrt(2)),-6/(5sqrt(2)),0).
    // With Kutta coefficient and rho both 1, multiply by A_proj*25/34.
    let mut force = [0.0; 6];
    viscous_forces(
        &[0.0, 0.0, 0.0, 1.0, 1.0, 0.0],
        1.0,
        0.0,
        &[2.0, 1.0, 1.0],
        0.0,
        1.0,
        0.0,
        0.0,
        0.0,
        &mut force,
    );
    let lift = PI * (17.0_f64 / 5.0).sqrt() * 25.0 / 34.0 * 6.0 / (5.0 * 2.0_f64.sqrt());
    close(&force, &[0.0, 0.0, 0.0, lift, -lift, 0.0]);

    // Along the short projected section: (1,2,4), v=(0,0,2),
    // A_proj=2pi and A_max=8pi. With blunt=1/2 and slender=1/4,
    // rho=1, mu=0: drag=2*(pi+6pi/4)=5pi, hence F_z=-10pi.
    let mut force = [0.0; 6];
    viscous_forces(
        &[0.0, 0.0, 0.0, 0.0, 0.0, 2.0],
        1.0,
        0.0,
        &[1.0, 2.0, 4.0],
        0.0,
        0.0,
        0.5,
        0.25,
        0.0,
        &mut force,
    );
    close(&force, &[0.0, 0.0, 0.0, 0.0, 0.0, -10.0 * PI]);

    // Zero motion must preserve accumulated forces, including degenerate axes.
    for size in [[1.0, 2.0, 3.0], [0.0; 3]] {
        let mut force = [1.0, -2.0, 3.0, -4.0, 5.0, -6.0];
        viscous_forces(
            &[0.0; 6], 2.0, 0.5, &size, 1.0, 1.0, 1.0, 1.0, 1.0, &mut force,
        );
        assert_eq!(force, [1.0, -2.0, 3.0, -4.0, 5.0, -6.0]);
    }
}

#[test]
fn ellipsoid_moments_and_coefficient_layout_exact() {
    // A unit rotation about each axis isolates ellipsoid_max_moment through
    // angular drag: I_dir=(8/15)*pi*d_dir*max(d_next,d_last)^4.
    // Axes (1,2,4) give fourth powers (256,256,16), all exactly represented.
    let moments = [
        8.0 / 15.0 * PI * 1.0 * 256.0,
        8.0 / 15.0 * PI * 2.0 * 256.0,
        8.0 / 15.0 * PI * 4.0 * 16.0,
    ];
    for dir in 0..3 {
        let mut velocity = [0.0; 6];
        velocity[dir] = 1.0;
        let mut force = [0.0; 6];
        viscous_forces(
            &velocity,
            1.0,
            0.0,
            &[1.0, 2.0, 4.0],
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            &mut force,
        );
        let mut expected = [0.0; 6];
        expected[dir] = -(moments[dir] * moments[dir]).sqrt();
        assert_eq!(force, expected, "ellipsoid moment axis {dir}");
    }
    let mut interaction = 0.0;
    let mut blunt = 0.0;
    let mut slender = 0.0;
    let mut angular = 0.0;
    let mut kutta = 0.0;
    let mut magnus = 0.0;
    let mut mass = [0.0; 3];
    let mut inertia = [0.0; 3];
    read_fluid_geom_interaction(
        &[
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ],
        &mut interaction,
        &mut blunt,
        &mut slender,
        &mut angular,
        &mut kutta,
        &mut magnus,
        &mut mass,
        &mut inertia,
    );
    assert_eq!(
        [interaction, blunt, slender, angular, kutta, magnus],
        [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]
    );
    assert_eq!(mass, [7.0, 8.0, 9.0]);
    assert_eq!(inertia, [10.0, 11.0, 12.0]);
}
