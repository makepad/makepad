use makepad_model::*;
use makepad_render::{asset_lights::parse_asset_lights, asset_morph::AssetMorph, StaticModel};

fn apply(doc: &mut Document, id: &str, operations: Vec<Operation>) {
    doc.apply(Transaction {request_id: id.into(), expected: doc.head(), operations}, None).unwrap();
}

#[test]
fn authored_detail_can_fill_128_objects_and_excess_refuses_atomically() {
    let mut doc = Document::new(Limits::default()).unwrap();
    assert_eq!(doc.limits().max_objects, 128);
    let mut operations = Vec::new();
    for i in 0..128 {
        let object = format!("detail_{i:03}");
        operations.push(Operation::Cube {object: object.clone(), size: [0.1; 3]});
        operations.push(Operation::Scene(SceneOperation::Node {object, node: SceneNode {
            transform: Transform {translation: [i as f64 * 0.1, 0., 0.], ..Default::default()}, ..Default::default()
        }}));
    }
    apply(&mut doc, "details", operations);
    let bytes = doc.to_bytes(None).unwrap();
    let restored = Document::from_bytes(&bytes, Limits::default(), None).unwrap();
    assert_eq!(restored.objects().count(), 128);
    let product = restored.compile(None).unwrap();
    assert_eq!(product.triangles, 128 * 12);
    assert_eq!(product.primitives.len(), 128);
    assert_eq!(StaticModel::parse_glb(&product.glb).unwrap().indices.len(), 128 * 36);
    let error = doc.apply(Transaction {request_id: "excess".into(), expected: doc.head(),
        operations: vec![Operation::Cube {object: "excess".into(), size: [1.; 3]}]}, None).unwrap_err();
    assert_eq!(error, Error::Budget("objects"));
    assert_eq!(doc.to_bytes(None).unwrap(), bytes);
}

#[test]
fn captured_astra_saloon_batches_compile_and_keep_the_authored_components() {
    // Recorded model.apply payloads from a real old-car authoring conversation.
    let fixture = json::parse(include_bytes!("fixtures/astra_saloon.json")).unwrap();
    let mut doc = Document::new(Limits::default()).unwrap();
    let batches = fixture.as_arr().unwrap();
    assert_eq!(batches.len(), 3);
    let mut old_limits = Limits::default();
    old_limits.max_objects = 32;
    let mut old = Document::new(old_limits).unwrap();
    for (index, batch) in batches.iter().enumerate() {
        let operations = parse_operations(batch.get("operations").unwrap(), doc.limits()).unwrap();
        let old_source = old.to_bytes(None).unwrap();
        let old_result = old.apply(Transaction {
            request_id: batch.get("request_id").unwrap().as_str().unwrap().into(), expected: old.head(), operations: operations.clone()
        }, None);
        if index < 2 {old_result.unwrap();} else {
            assert_eq!(old_result.unwrap_err(), Error::Budget("objects"));
            assert_eq!(old.to_bytes(None).unwrap(), old_source);
        }
        apply(&mut doc, batch.get("request_id").unwrap().as_str().unwrap(), operations);
        let head = doc.head();
        let draft = doc.compile_preview(None).unwrap();
        assert_eq!(draft.head, head);
        assert!(draft.triangles > 100);
        let model = StaticModel::parse_glb(&draft.glb).unwrap();
        assert!(model.driven_parts.is_empty());
        assert_eq!(model.indices.len(), draft.triangles * 3);
    }
    let source = doc.to_snapshot_bytes(None).unwrap();
    let restored = Document::from_bytes(&source, Limits::default(), None).unwrap();
    assert!(restored.objects().count() > 32);
    let compiled = restored.compile(None).unwrap();
    let model = StaticModel::parse_glb(&compiled.glb).unwrap();
    assert_eq!(model.driven_parts.len(), 4);
    let parsed = makepad_gltf::parse_glb_bytes(&compiled.glb).unwrap();
    assert_eq!(parse_asset_lights(&parsed.document).unwrap().len(), 2);
    assert_eq!(restored.scene().wheels.len(), 4);
    assert!(restored.surface().memory_bytes() > 128 * 128 * 4);
    for name in ["body", "cab", "windows", "front_left_tire", "grille-frame", "headlamp-lens"] {
        assert!(restored.object(name).is_some(), "missing actual component {name}");
    }
}

#[test]
fn incomplete_wheel_binding_is_a_static_draft_but_still_refuses_publication() {
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(&mut doc, "partial", vec![Operation::Cube {object: "wheel".into(), size: [1.; 3]},
        Operation::Scene(SceneOperation::VehicleWheel {object: "wheel".into(), wheel: VehicleWheel {
            connection: VEHICLE_WHEEL_CONNECTIONS[0].into(), pivot: [0.; 3], radius: 0.5, width: 1., visual: None
        }})]);
    let source = doc.to_bytes(None).unwrap();
    assert!(doc.compile(None).is_err());
    let draft = doc.compile_preview(None).unwrap();
    assert_eq!(draft.head, doc.head());
    let model = StaticModel::parse_glb(&draft.glb).unwrap();
    assert!(model.driven_parts.is_empty());
    assert_eq!(model.indices.len(), 36);
    assert_eq!(doc.to_bytes(None).unwrap(), source);
    assert!(doc.compile(None).is_err());
    assert_eq!(doc.compile_preview(Some(&|| true)).unwrap_err(), Error::Mesh(mesh::MeshError::Cancelled));
    assert_eq!(doc.to_bytes(None).unwrap(), source);
}

#[test]
fn incomplete_rig_draft_preserves_textures_shape_weights_and_joint_light_rest_placement() {
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(&mut doc, "geometry", vec![Operation::Cube {object: "body".into(), size: [1.; 3]},
        Operation::TextureSolid {material: 0, width: 4, height: 4, color: [90, 70, 45]}]);
    let vertices = doc.object("body").unwrap().vertices().iter().map(|v|v.id).collect::<Vec<_>>();
    let root = Transform {translation: [1., 2., 3.], rotation: transform::quat_axis_angle([0.,1.,0.], 0.7).unwrap(), scale: [2., 1., 0.5]};
    let child = Transform {translation: [0., 1., 0.], rotation: transform::quat_axis_angle([1.,0.,0.], 0.4).unwrap(), ..Default::default()};
    let lamp = Transform {translation: [0.3, 0.2, 0.1], rotation: [0., 1., 0., 0.], ..Default::default()};
    apply(&mut doc, "partial-rig", vec![
        Operation::SetSkeleton {skeleton: Skeleton {joints: (0..5).map(|i|Joint {name: format!("joint_{i}"), parent: if i == 0 {None} else {Some(0)}, translation: [0.; 3]}).collect()}},
        Operation::Rig(RigOperation::Rest {joint: 0, transform: root}),
        Operation::Rig(RigOperation::Rest {joint: 1, transform: child}),
        Operation::SetWeights {object: "body".into(), vertex: vertices[0], weights: (0..5).map(|joint|mesh::JointWeight {joint, weight: 0.2}).collect()},
        Operation::Rig(RigOperation::Morph {name: "rise".into(), object: "body".into(), deltas: vertices.iter().map(|&v|(v,[0.,0.3,0.])).collect(), weight: 0.5}),
        Operation::Scene(SceneOperation::Light(LightEmitter {name: "joint_lamp".into(), attachment: Attachment::Joint(1), transform: lamp,
            kind: LightKind::Spot {inner: 0.1, outer: 0.4}, color: [1., 0.8, 0.6], intensity: 42., range: 10.})),
    ]);
    let source = doc.to_bytes(None).unwrap();
    assert!(doc.compile(None).is_err());
    let draft = doc.compile_preview(None).unwrap();
    let parsed = makepad_gltf::parse_glb_bytes(&draft.glb).unwrap();
    assert!(parsed.document.skins.as_ref().is_none_or(Vec::is_empty));
    assert!(parsed.document.animations.as_ref().is_none_or(Vec::is_empty));
    let model = StaticModel::parse_glb(&draft.glb).unwrap();
    assert_eq!(model.indices.len(), 36);
    let morph = AssetMorph::parse(&draft.glb, false).unwrap().unwrap();
    assert_eq!(morph.defaults[0], 0.5);
    assert_eq!(morph.pixels[1], 0.3);
    assert!((draft.bounds[0][1] + 0.35).abs() < 1e-6);
    assert!(makepad_render::model::embedded_base_color_png(&draft.glb).is_some());
    let emitters = parse_asset_lights(&parsed.document).unwrap();
    assert_eq!(emitters.len(), 1);
    let identity = Default::default();
    let light = emitters[0].placed(&identity, None);
    let world = transform::matrix_mul(transform::matrix_mul(root.matrix().unwrap(),child.matrix().unwrap()),lamp.matrix().unwrap());
    let p = transform::transform_point(world, [0.; 3]);
    let d = transform::normalized(transform::transform_vector(world, [0., 0., -1.])).unwrap();
    for (actual,expected) in [light.pos.x,light.pos.y,light.pos.z].into_iter().zip(p).chain([light.dir.x,light.dir.y,light.dir.z].into_iter().zip(d)) {
        assert!((actual as f64 - expected).abs() < 1e-5);
    }
    assert_eq!(doc.to_bytes(None).unwrap(), source);
}

#[test]
fn draft_keeps_geometry_budget_admission() {
    let mut limits = Limits::default();
    limits.mesh.max_triangles = 12;
    let mut doc = Document::new(limits).unwrap();
    apply(&mut doc, "two", vec![Operation::Cube {object: "a".into(), size: [1.; 3]}, Operation::Cube {object: "b".into(), size: [1.; 3]}]);
    let head = doc.head();
    assert_eq!(doc.compile_preview(None).unwrap_err(), Error::Budget("compiled triangles"));
    assert_eq!(doc.head(), head);
}

fn signed_volume(mesh: &mesh::Mesh) -> f64 {
    let triangles = mesh.triangulate(&mut mesh::Context::default()).unwrap();
    triangles.triangles.iter().map(|triangle| {
        let [a,b,c] = triangle.indices.map(|i|triangles.vertices[i as usize].position);
        transform::dot(a,transform::cross(b,c))/6.
    }).sum()
}

#[test]
fn actual_saloon_winding_repair_keeps_source_identity_and_exports_outward_normals() {
    let fixture = json::parse(include_bytes!("fixtures/astra_saloon.json")).unwrap();
    let mut doc = Document::new(Limits::default()).unwrap();
    for batch in fixture.as_arr().unwrap() {
        let operations = parse_operations(batch.get("operations").unwrap(), doc.limits()).unwrap();
        apply(&mut doc, batch.get("request_id").unwrap().as_str().unwrap(), operations);
    }
    let before = doc.to_bytes(None).unwrap();
    for object in ["body", "cab", "fender"] {
        assert!(signed_volume(doc.object(object).unwrap()) < 0., "actual authored {object} was inside-out");
        let report = doc.object(object).unwrap().validate_global(&mut mesh::Context::default()).unwrap();
        assert_eq!(report.orientation_errors, 1, "{object}: {report:?}");
    }
    for object in ["front_left_tire", "headlamp-shell", "front-bumper"] {
        assert!(signed_volume(doc.object(object).unwrap()) > 0., "engine primitive {object} must stay outward");
    }
    let operations = ["body", "cab", "fender", "windows"].into_iter().map(|object| json::obj(vec![
        ("op",json::s("flip_faces")), ("object",json::s(object)),
        ("faces",json::Value::Arr(doc.object(object).unwrap().faces().iter().map(|f|json::s(f.id.0.to_string())).collect()))
    ])).collect();
    let operations = parse_operations(&json::Value::Arr(operations), doc.limits()).unwrap();
    apply(&mut doc, "repair-winding", operations);
    for object in ["body", "cab", "fender"] {
        assert!(signed_volume(doc.object(object).unwrap()) > 0., "repaired {object} must face outward");
        assert!(doc.object(object).unwrap().validate_global(&mut mesh::Context::default()).unwrap().is_valid_solid);
    }
    let windows = doc.object("windows").unwrap();
    let triangles = windows.triangulate(&mut mesh::Context::default()).unwrap();
    let directions = [[0.,0.,1.],[1.,0.,0.],[1.,0.,0.],[-1.,0.,0.],[-1.,0.,0.],[0.,0.,-1.]];
    for triangle in &triangles.triangles {
        let side = windows.faces().iter().position(|f|f.id==triangle.source_face).unwrap();
        assert!(transform::dot(triangles.vertices[triangle.indices[0] as usize].normal,directions[side]) > 0.8);
    }
    let repaired = doc.to_bytes(None).unwrap();
    let restored = Document::from_bytes(&repaired, Limits::default(), None).unwrap();
    let product = restored.compile(None).unwrap();
    for primitive in product.primitives.iter().filter(|p|["body","cab","fender","windows"].contains(&p.object.as_str())) {
        for (positions,normals) in primitive.positions.chunks_exact(3).zip(primitive.normals.chunks_exact(3)) {
            let [a,b,c] = [positions[0],positions[1],positions[2]].map(|p|p.map(f64::from));
            let direction = transform::cross(transform::sub(b,a),transform::sub(c,a));
            for normal in normals {assert!(transform::dot(direction,normal.map(f64::from)) > 0.);}
        }
    }
    assert_eq!(StaticModel::parse_glb(&product.glb).unwrap().driven_parts.len(), 4);
    doc.undo(doc.head(), None).unwrap();
    let original = Document::from_bytes(&before, Limits::default(), None).unwrap();
    for (name,mesh) in original.objects() {assert_eq!(doc.object(name).unwrap(), mesh);}
}
