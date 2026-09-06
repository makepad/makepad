use makepad_game_sim::soft_body::*;
use std::sync::Arc;

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + b[i])
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn length(p: [f32; 3]) -> f32 {
    p.iter().map(|v| v * v).sum::<f32>().sqrt()
}
fn binding_point(def: &SoftBodyDefinition, positions: &[[f32; 3]], b: SoftBodyBinding) -> [f32; 3] {
    let t = def.tetrahedra[b.tetrahedron as usize];
    std::array::from_fn(|axis| {
        (0..4)
            .map(|i| positions[t[i] as usize][axis] * b.weights[i])
            .sum()
    })
}
fn sphere() -> Arc<SoftBodyDefinition> {
    Arc::new(SoftBodyDefinition::ellipsoid([0.; 3], [0.5; 3]).unwrap())
}
fn frame(state: &SoftBodyState) -> SoftBodyFrame {
    let mut frame = SoftBodyFrame::default();
    state.write_frame(&mut frame);
    frame
}
fn quiet() -> SoftBodySettings {
    SoftBodySettings {
        gravity: [0.; 3],
        ..Default::default()
    }
}
fn pose(x: f32, y: f32, z: f32) -> SoftBodyPose {
    SoftBodyPose {
        translation: [x, y, z],
        ..Default::default()
    }
}
fn max_displacement(def: &SoftBodyDefinition, frame: &SoftBodyFrame) -> f32 {
    def.rest_positions
        .iter()
        .zip(&frame.positions)
        .map(|(a, b)| length(sub(*a, *b)))
        .fold(0., f32::max)
}

#[test]
fn enclosing_cage_preserves_smooth_authored_points_and_rest_affine_identity() {
    let center = [0.1, -0.2, 0.3];
    let radii = [0.5, 0.6, 0.4];
    let def = Arc::new(SoftBodyDefinition::ellipsoid(center, radii).unwrap());
    assert_eq!(
        (
            def.rest_positions.len(),
            def.tetrahedra.len(),
            def.surface_samples.len()
        ),
        (43, 80, 42)
    );
    // Curved sphere points between cage vertices remain bindable. No scale
    // or altered vertex positions are allowed to hide an inscribed cage.
    for y in -8..=8 {
        for az in 0..31 {
            let z = y as f32 / 8.;
            let radial = (1. - z * z).max(0.).sqrt();
            let angle = az as f32 * std::f32::consts::TAU / 31.;
            let p = add(
                center,
                [
                    radii[0] * radial * angle.cos(),
                    radii[1] * z,
                    radii[2] * radial * angle.sin(),
                ],
            );
            let binding = def.bind_point(p).unwrap();
            assert!(length(sub(binding_point(&def, &def.rest_positions, binding), p)) < 2e-5);
        }
    }
    let rotation = [
        0.,
        std::f32::consts::FRAC_1_SQRT_2,
        0.,
        std::f32::consts::FRAC_1_SQRT_2,
    ];
    let anchor = SoftBodyPose {
        translation: [3., 2., -4.],
        rotation,
    };
    let mut state = SoftBodyState::new(def.clone(), quiet(), anchor).unwrap();
    let stats = state.step(1. / 60., anchor, &[]).unwrap();
    assert!(!stats.recovered);
    let frame = frame(&state);
    assert!(max_displacement(&def, &frame) < 1e-5);
    for matrix in &frame.tetrahedra {
        for r in 0..3 {
            for c in 0..4 {
                assert!((matrix.matrix[r][c] - if r == c { 1. } else { 0. }).abs() < 2e-4);
            }
        }
    }
}

#[test]
fn retained_impulse_deforms_volume_then_damps_back_toward_rest() {
    let def = sphere();
    let anchor = pose(0., 1., 0.);
    let mut state = SoftBodyState::new(def.clone(), quiet(), anchor).unwrap();
    let binding = def.bind_point([0., 0.5, 0.]).unwrap();
    state.apply_impulse(binding, [0.035, 0.02, 0.01]).unwrap();
    let mut largest = 0.0_f32;
    for _ in 0..12 {
        let stats = state.step(1. / 60., anchor, &[]).unwrap();
        assert!(!stats.recovered);
        assert!(
            stats.min_volume_ratio > 0.7 && stats.max_volume_error < 0.3,
            "{stats:?}"
        );
        largest = largest.max(max_displacement(&def, &frame(&state)));
    }
    assert!(
        largest > 0.002,
        "physics did not move the authored body: {largest}"
    );
    for _ in 0..240 {
        assert!(!state.step(1. / 60., anchor, &[]).unwrap().recovered);
    }
    assert!(
        max_displacement(&def, &frame(&state)) < largest * 0.1,
        "damping did not dissipate deformation"
    );
}

#[test]
fn controller_motion_drives_inertia_without_changing_controller_or_rigidly_dragging_shell() {
    let def = sphere();
    let mut state = SoftBodyState::new(def.clone(), quiet(), pose(0., 1., 0.)).unwrap();
    let driven = pose(0.04, 1., 0.);
    let stats = state.step(1. / 60., driven, &[]).unwrap();
    assert!(!stats.recovered);
    let frame = frame(&state);
    let core = def.anchors[0] as usize;
    assert!(length(sub(frame.positions[core], def.rest_positions[core])) < 1e-5);
    assert!(
        frame
            .positions
            .iter()
            .zip(&def.rest_positions)
            .any(|(p, r)| p[0] - r[0] < -0.002),
        "shell was rigidly teleported"
    );
    assert!(stats.max_edge_strain > 0.001);
    assert_eq!(
        driven,
        pose(0.04, 1., 0.),
        "controller input remains immutable"
    );
}

#[test]
fn visible_surface_contacts_resolve_plane_sphere_capsule_and_box() {
    let cases = [
        (
            pose(0., 0.45, 0.),
            SoftBodyCollider::Plane {
                normal: [0., 1., 0.],
                offset: 0.,
            },
        ),
        (
            pose(0., 0.75, 0.),
            SoftBodyCollider::Sphere {
                center: [0.7, 0.75, 0.],
                radius: 0.25,
            },
        ),
        (
            pose(0., 0.75, 0.),
            SoftBodyCollider::Capsule {
                a: [0.7, 0., 0.],
                b: [0.7, 1.5, 0.],
                radius: 0.25,
            },
        ),
        (
            pose(0., 0.75, 0.),
            SoftBodyCollider::Box {
                center: [0.7, 0.75, 0.],
                half_extents: [0.25, 0.3, 0.3],
                rotation: [0., 0., 0., 1.],
            },
        ),
    ];
    for (anchor, collider) in cases {
        let def = sphere();
        let settings = SoftBodySettings {
            iterations: 12,
            contact_radius: 0.01,
            ..quiet()
        };
        let mut state = SoftBodyState::new(def.clone(), settings, anchor).unwrap();
        let mut contacts = 0;
        for _ in 0..30 {
            let stats = state.step(1. / 60., anchor, &[collider]).unwrap();
            contacts += stats.contacts;
            assert!(!stats.recovered, "{collider:?}: {stats:?}");
        }
        assert!(contacts > 0, "no real contact corrections for {collider:?}");
        let frame = frame(&state);
        let mut worst = f32::INFINITY;
        for &sample in &def.surface_samples {
            let p = add(
                anchor.translation,
                binding_point(&def, &frame.positions, sample),
            );
            let distance = match collider {
                SoftBodyCollider::Plane { .. } => p[1],
                SoftBodyCollider::Sphere { center, radius } => length(sub(p, center)) - radius,
                SoftBodyCollider::Capsule { a, b, radius } => {
                    length(sub(p, [a[0], p[1].clamp(a[1], b[1]), a[2]])) - radius
                }
                SoftBodyCollider::Box {
                    center,
                    half_extents,
                    ..
                } => {
                    let q = sub(p, center);
                    let outside = std::array::from_fn(|i| (q[i].abs() - half_extents[i]).max(0.));
                    length(outside)
                        + (0..3)
                            .map(|i| q[i].abs() - half_extents[i])
                            .fold(f32::NEG_INFINITY, f32::max)
                            .min(0.)
                }
            };
            worst = worst.min(distance);
        }
        assert!(
            worst >= settings.contact_radius - 0.003,
            "embedded surface penetrates {collider:?}: {worst}"
        );
    }
}

#[test]
fn gravity_sags_the_retained_shell_while_core_stays_anchored() {
    let def = sphere();
    let anchor = pose(0., 2., 0.);
    let mut state = SoftBodyState::new(def.clone(), SoftBodySettings::default(), anchor).unwrap();
    for _ in 0..90 {
        let stats = state.step(1. / 60., anchor, &[]).unwrap();
        assert!(!stats.recovered);
        assert!(stats.max_volume_error < 0.3);
    }
    let deformed = frame(&state);
    let core = def.anchors[0] as usize;
    assert!(length(sub(deformed.positions[core], def.rest_positions[core])) < 1e-5);
    let sag = deformed
        .positions
        .iter()
        .zip(&def.rest_positions)
        .enumerate()
        .filter(|(i, _)| *i != core)
        .map(|(_, (p, r))| p[1] - r[1])
        .sum::<f32>()
        / 42.;
    assert!(sag < -1e-5, "gravity had no physical shell response: {sag}");
}

#[test]
fn affine_output_and_rigid_eye_frame_follow_deformation_without_stretching_eyes() {
    let def = sphere();
    let state = SoftBodyState::new(def.clone(), quiet(), SoftBodyPose::default()).unwrap();
    let binding = def.bind_point([0.12, 0.08, 0.4]).unwrap();
    let rest = binding_point(&def, &def.rest_positions, binding);
    let transform = SoftBodyAffine {
        matrix: [
            [0., -1.25, 0., 0.1],
            [0.7, 0., 0., -0.2],
            [0., 0., 1.1, 0.05],
        ],
    };
    let mut deformed = frame(&state);
    deformed.positions = def
        .rest_positions
        .iter()
        .map(|p| transform.transform_point(*p))
        .collect();
    deformed.tetrahedra.fill(transform);
    let eye = deformed.attachment(&def, binding).unwrap();
    assert!(length(sub(eye.position, transform.transform_point(rest))) < 1e-5);
    let q = eye.rotation;
    assert!((q.iter().map(|v| v * v).sum::<f32>() - 1.).abs() < 1e-5);
    assert!(
        q[0].abs() < 1e-4
            && q[1].abs() < 1e-4
            && (q[2].abs() - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4
    );
    // A rigid quaternion is all the eye receives. Its radius cannot inherit
    // the body's 0.7/1.25/1.1 stretch; the body normal still must correct it.
    let normal = transform.transform_normal([1., 1., 0.]).unwrap();
    let expected = [-1. / 1.25, 1. / 0.7, 0.];
    let len = length(expected);
    assert!(length(sub(normal, expected.map(|v| v / len))) < 1e-5);
}

#[test]
fn repeated_inputs_are_deterministic_and_reset_clears_velocity() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<SoftBodyState>();
    let def = sphere();
    let mut a = SoftBodyState::new(def.clone(), quiet(), pose(0., 1., 0.)).unwrap();
    let mut b = a.clone();
    let binding = def.bind_point([0.4, 0.1, 0.]).unwrap();
    for tick in 0..120 {
        let anchor = pose((tick % 20) as f32 * 0.002, 1., 0.);
        if tick == 10 {
            a.apply_impulse(binding, [0.01, 0.02, 0.]).unwrap();
            b.apply_impulse(binding, [0.01, 0.02, 0.]).unwrap();
        }
        assert_eq!(
            a.step(1. / 60., anchor, &[]).unwrap(),
            b.step(1. / 60., anchor, &[]).unwrap()
        );
        assert_eq!(frame(&a), frame(&b));
    }
    let teleported = pose(100., 20., -50.);
    a.reset(teleported).unwrap();
    assert!(!a.step(1. / 60., teleported, &[]).unwrap().recovered);
    assert!(max_displacement(&def, &frame(&a)) < 2e-4);
}

#[test]
fn malformed_inputs_refuse_without_mutating_retained_state() {
    let def = sphere();
    let anchor = pose(0., 1., 0.);
    let mut state = SoftBodyState::new(def.clone(), quiet(), anchor).unwrap();
    let initial = frame(&state);
    for dt in [f32::NAN, -1., 0., 0.1] {
        assert!(state.step(dt, anchor, &[]).is_err());
        assert_eq!(frame(&state), initial);
    }
    let invalid = SoftBodyCollider::Sphere {
        center: [0.; 3],
        radius: -1.,
    };
    assert!(state.step(1. / 60., anchor, &[invalid]).is_err());
    assert!(state
        .step(
            1. / 60.,
            SoftBodyPose {
                rotation: [0.; 4],
                ..anchor
            },
            &[]
        )
        .is_err());
    assert!(state
        .step(
            1. / 60.,
            anchor,
            &vec![
                SoftBodyCollider::Plane {
                    normal: [0., 1., 0.],
                    offset: 0.
                };
                65
            ]
        )
        .is_err());
    assert!(state
        .apply_impulse(
            SoftBodyBinding {
                tetrahedron: 0,
                weights: [1.; 4]
            },
            [1.; 3]
        )
        .is_err());
    assert_eq!(frame(&state), initial);
    for case in 0..5 {
        let mut broken = (*def).clone();
        match case {
            0 => broken.tetrahedra[0].swap(1, 2),
            1 => broken.tetrahedra[0][1] = broken.tetrahedra[0][0],
            2 => broken.rest_positions[0][0] = f32::NAN,
            3 => broken.surface_samples[0].weights = [0.; 4],
            _ => broken.anchors.push(broken.anchors[0]),
        }
        assert!(SoftBodyState::new(Arc::new(broken), quiet(), anchor).is_err());
    }
    assert!(def.bind_point([100., 0., 0.]).is_err());
    assert!(SoftBodyState::new(
        def,
        SoftBodySettings {
            iterations: 255,
            ..quiet()
        },
        anchor
    )
    .is_err());
}

#[test]
fn impossible_core_contact_reports_bounded_recovery_with_valid_rest_output() {
    let def = sphere();
    let mut state = SoftBodyState::new(def.clone(), quiet(), SoftBodyPose::default()).unwrap();
    let result = state
        .step(
            1. / 60.,
            SoftBodyPose::default(),
            &[SoftBodyCollider::Plane {
                normal: [0., 1., 0.],
                offset: 100.,
            }],
        )
        .unwrap();
    assert!(
        result.recovered,
        "impossible core constraint must not silently produce an inverted body: {result:?}"
    );
    let frame = frame(&state);
    assert!(frame.positions.iter().flatten().all(|v| v.is_finite()));
    assert!(max_displacement(&def, &frame) < 1e-5);
}
