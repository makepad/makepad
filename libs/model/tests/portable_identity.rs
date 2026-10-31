//! One program, one product, on every machine. Models built on a Mac and on
//! Linux must be byte-identical: the store addresses built products by
//! content, so a product that differs per platform splits the store and
//! misses every cache the other machine filled. These goldens were recorded
//! on Linux (a pool node) and pass unchanged on macOS (the Mac mini); a
//! transcendental routed back through the platform libm breaks them there.

use makepad_model::*;

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100_0000_01b3))
}

fn product(ops: Vec<ProgramOp>) -> u64 {
    let built = build_program(&ops, Limits::default(), &mut |alias| Err(format!("no import {alias}")), None).unwrap();
    fnv(&built.compiled.glb)
}

fn programs() -> Vec<(&'static str, Vec<ProgramOp>)> {
    vec![
        ("sphere+cylinder", vec![
            ProgramOp::Op(Operation::sphere("ball", 0.5, 24, 12, true, &Limits::default()).unwrap()),
            ProgramOp::Op(Operation::cylinder("post", 0.2, 2.0, 17, true, &Limits::default()).unwrap()),
        ]),
        ("lathe+sweep", vec![
            ProgramOp::Op(Operation::Construction(ConstructionOperation {
                object: "vase".into(),
                kind: Construction::Lathe {
                    profile: vec![[0.0, 0.0], [0.7, 0.1], [0.8, 0.8], [0.3, 1.7], [0.0, 1.8]],
                    axis: 1,
                    segments: 23,
                    caps: true,
                    material: 0,
                    outward: true
                }
            })),
            ProgramOp::Op(Operation::Construction(ConstructionOperation {
                object: "rail".into(),
                kind: Construction::Sweep {
                    profile: vec![[-0.1, -0.1], [0.1, -0.1], [0.1, 0.1], [-0.1, 0.1]],
                    path: vec![[0.0, 0.0, 0.0], [0.0, 0.4, 1.0], [0.5, 0.9, 2.0], [1.4, 1.1, 2.6]],
                    caps: true,
                    material: 0
                }
            })),
        ]),
        ("shapes", vec![
            ProgramOp::Op(Operation::Construction(ConstructionOperation {
                object: "pill".into(),
                kind: Construction::Shape {
                    shape: Shape::Capsule { radius: 0.3, height: 1.2, segments: 19, rings: 7 },
                    material: 0
                }
            })),
            ProgramOp::Op(Operation::Construction(ConstructionOperation {
                object: "cone".into(),
                kind: Construction::Shape {
                    shape: Shape::Cone { bottom: 0.6, top: 0.2, height: 1.1, segments: 21, smooth: true },
                    material: 0
                }
            })),
            ProgramOp::Op(Operation::Construction(ConstructionOperation {
                object: "ring".into(),
                kind: Construction::Shape {
                    shape: Shape::Torus { radius: 0.8, tube: 0.15, segments: 29, sides: 11 },
                    material: 0
                }
            })),
        ]),
    ]
}

/// Recorded on Linux. A mismatch on another platform means some geometry
/// still goes through the platform libm (see `makepad_csg_math::portable`).
const GOLDEN: &[(&str, u64)] = &[
    ("sphere+cylinder", 0x73f3_858b_4f3d_f1cd),
    ("lathe+sweep", 0x0dc2_4fc8_8b97_c245),
    ("shapes", 0x4e83_3371_811a_a309),
    ("character", 0x1f51_f2ba_c194_bafe),
];

#[test]
fn every_platform_builds_the_same_bytes() {
    let mut got = Vec::new();
    for (name, program) in programs() {
        got.push((name, product(program)));
    }
    // A generated character: the default spec.
    let character = [makepad_model::ProgramOp::Character(Box::new(makepad_model::character::CharacterSpec::defaults()))];
    let built = build_program(&character, Limits::default(), &mut |alias| Err(format!("no import {alias}")), None).unwrap();
    got.push(("character", fnv(&built.compiled.glb)));
    for (name, hash) in &got {
        eprintln!("portable identity: {name} {hash:#018x}");
    }
    assert_eq!(got, GOLDEN, "a built product changed on this platform");
}
