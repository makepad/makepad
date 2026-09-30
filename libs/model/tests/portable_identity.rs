//! One program, one product, on every machine. Models built on a Mac and on
//! Linux must be byte-identical: the store addresses built products by
//! content, so a product that differs per platform splits the store and
//! misses every cache the other machine filled. These goldens were recorded
//! on Linux (node 165) and pass unchanged on macOS (the Mac mini); a
//! transcendental routed back through the platform libm breaks them there.

use makepad_model::json::{self, Value};
use makepad_model::build_program;
use makepad_model::Limits;

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100_0000_01b3))
}

fn product(program: &str) -> u64 {
    let ops = match json::parse(program.as_bytes()).unwrap() {
        Value::Arr(ops) => ops,
        _ => unreachable!(),
    };
    let built = build_program(&ops, Limits::default(), &mut |alias| Err(format!("no import {alias}")), None).unwrap();
    fnv(&built.compiled.glb)
}

const PROGRAMS: &[(&str, &str)] = &[
    ("sphere+cylinder", r#"[{"op":"sphere","object":"ball","radius":0.5,"segments":24,"rings":12,"smooth":true},
        {"op":"cylinder","object":"post","radius":0.2,"height":2,"segments":17,"smooth":true}]"#),
    ("lathe+sweep", r#"[{"op":"lathe","object":"vase","profile":[[0,0],[0.7,0.1],[0.8,0.8],[0.3,1.7],[0,1.8]],"axis":1,"segments":23,"caps":true,"material":0,"outward":true},
        {"op":"sweep","object":"rail","profile":[[-0.1,-0.1],[0.1,-0.1],[0.1,0.1],[-0.1,0.1]],"path":[[0,0,0],[0,0.4,1],[0.5,0.9,2],[1.4,1.1,2.6]],"caps":true,"material":0}]"#),
    ("shapes", r#"[{"op":"capsule","object":"pill","radius":0.3,"height":1.2,"segments":19,"rings":7,"material":0},
        {"op":"cone","object":"cone","radius":0.6,"top_radius":0.2,"height":1.1,"segments":21,"smooth":true,"material":0},
        {"op":"torus","object":"ring","radius":0.8,"tube":0.15,"segments":29,"sides":11,"material":0}]"#),
    ("character", r#"[{"op":"character","preset":"halcyon_warden"}]"#),
];

/// Recorded on Linux. A mismatch on another platform means some geometry
/// still goes through the platform libm (see `makepad_csg_math::portable`).
const GOLDEN: &[(&str, u64)] = &[
    ("sphere+cylinder", 0x73f3_858b_4f3d_f1cd),
    ("lathe+sweep", 0x0dc2_4fc8_8b97_c245),
    ("shapes", 0x4e83_3371_811a_a309),
    ("character", 0x95fb_81d9_a066_ffc0),
];

#[test]
fn every_platform_builds_the_same_bytes() {
    let mut got = Vec::new();
    for (name, program) in PROGRAMS {
        got.push((*name, product(program)));
    }
    for (name, hash) in &got {
        eprintln!("portable identity: {name} {hash:#018x}");
    }
    assert_eq!(got, GOLDEN, "a built product changed on this platform");
}
