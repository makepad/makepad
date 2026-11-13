use makepad_model::*;

#[test]
fn large_program_counts_survive_source_roundtrip() {
    let limits = Limits::default();
    let empty = mesh::Mesh::new().to_bytes(&mut mesh::Context::default()).unwrap();
    // Only one drawable plane is needed: the test exercises authoring counts,
    // not thousands of redundant triangulations or dense skin-weight arrays.
    let mut program = vec![ProgramOp::Op(Operation::Plane { object: "object_0".into(), size: [1.; 2] })];
    for i in 1..2_100 {
        program.push(ProgramOp::Op(Operation::ImportMesh { object: format!("object_{i}"), source: empty.clone() }));
    }
    for material in 1..600 {
        program.push(ProgramOp::Op(Operation::SetMaterial { material, value: Material::default() }));
    }
    program.push(ProgramOp::Op(Operation::SetSkeleton { skeleton: Skeleton {
        joints: (0..130).map(|i| Joint {
            name: format!("joint_{i}"), parent: if i == 0 { None } else { Some(0) }, translation: [0.; 3],
        }).collect(),
    } }));
    program.push(ProgramOp::Op(Operation::AutoWeights { object: "object_0".into() }));
    program.push(ProgramOp::Op(Operation::SetClip { clip: AnimationClip {
        name: "long_clip".into(),
        channels: vec![AnimationChannel { joint: 0, path: AnimationPath::Translation,
            keys: (0..70_000).map(|i| Keyframe { time: i as f64 * 0.1, value: [0.; 4] }).collect(),
        }],
    } }));
    let built = build_program(&program, limits.clone(), &mut |_| unreachable!(), None).unwrap();
    let source = built.document.to_bytes(None).unwrap();
    let reopened = Document::from_bytes(&source, limits, None).unwrap();
    for doc in [&built.document, &reopened] {
        assert_eq!(doc.objects().count(), 2_100, "all objects survive");
        assert_eq!(doc.materials().len(), 600, "all materials survive");
        assert_eq!(doc.skeleton().unwrap().joints.len(), 130, "all joints survive");
        assert_eq!(doc.clips()["long_clip"].channels[0].keys.len(), 70_000, "all keys survive");
    }
    assert_eq!(reopened.head(), built.document.head());
}

#[test]
fn hostile_document_object_count_is_bounded_by_source_bytes() {
    let mut source = [0u8; 64];
    source[..8].copy_from_slice(b"MPMODEL\0");
    source[8..12].copy_from_slice(&3u32.to_le_bytes());
    // After the 40-byte head and empty material table, claim 2^31 objects.
    // The count is rejected before allocating or reading any object records.
    source[56..60].copy_from_slice(&(1u32 << 31).to_le_bytes());
    assert!(matches!(Document::from_bytes(&source, Limits::default(), None), Err(Error::Budget("source count"))));
}
