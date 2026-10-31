use makepad_model::*;
use makepad_render::{asset_lights::parse_asset_lights, asset_morph::AssetMorph, StaticModel};

fn apply(doc: &mut Document, id: &str, operations: Vec<Operation>) {
    doc.apply(Transaction {request_id: id.into(), expected: doc.head(), operations}, None).unwrap();
}

#[test]
fn authored_detail_can_fill_128_objects_and_excess_refuses_atomically() {
    let mut doc = Document::new(Limits { max_objects: 128, ..Limits::default() }).unwrap();
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
    // Typed operations from a real old-car authoring conversation.
    let mut doc = Document::new(Limits::default()).unwrap();
    let batches = astra_saloon(doc.limits());
    assert_eq!(batches.len(), 3);
    let mut old_limits = Limits::default();
    old_limits.max_objects = 32;
    let mut old = Document::new(old_limits).unwrap();
    for (index, (request_id, operations)) in batches.into_iter().enumerate() {
        let old_source = old.to_bytes(None).unwrap();
        let old_result = old.apply(Transaction {
            request_id: request_id.into(), expected: old.head(), operations: operations.clone()
        }, None);
        if index < 2 {old_result.unwrap();} else {
            assert_eq!(old_result.unwrap_err(), Error::Budget("objects"));
            assert_eq!(old.to_bytes(None).unwrap(), old_source);
        }
        apply(&mut doc, request_id, operations);
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
    let mut doc = Document::new(Limits::default()).unwrap();
    for (request_id, operations) in astra_saloon(doc.limits()) {
        apply(&mut doc, request_id, operations);
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
    let operations = ["body", "cab", "fender", "windows"].into_iter().map(|object|
        Operation::MeshEditing(MeshEditingOperation {
            object: object.into(),
            action: Action::FlipFaces { faces: doc.object(object).unwrap().faces().iter().map(|f|f.id).collect() },
        })
    ).collect();
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

/// Signed volume of what the renderer draws: positive when the faces point out.
fn drawn_volume(glb: &[u8]) -> f64 {
    let model = StaticModel::parse_glb(glb).unwrap();
    let p = |i: u32| { let o = i as usize * makepad_render::MODEL_VERTEX_FLOATS; [model.vertices[o] as f64, model.vertices[o + 1] as f64, model.vertices[o + 2] as f64] };
    model.indices.chunks_exact(3).map(|t| { let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
        a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0]) }).sum::<f64>() / 6.
}

#[test]
fn inward_polygon_shells_draw_outward_in_draft_and_product() {
    for inward in [false, true] {
        let faces = [[0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5], [0, 2, 3, 1], [4, 5, 7, 6]]
            .map(|f| { let mut f = f.to_vec(); if inward { f.reverse(); } mesh::Polygon::new(f) });
        let mut doc = Document::new(Limits::default()).unwrap();
        let operations = vec![Operation::polygon_mesh("body",
            &[[0.,0.,0.],[2.,0.,0.],[0.,1.,0.],[2.,1.,0.],[0.,0.,3.],[2.,0.,3.],[0.,1.,3.],[2.,1.,3.]],
            &faces, doc.limits()).unwrap()];
        apply(&mut doc, "box", operations);
        // The source keeps what was written; only the drawn product faces out.
        assert_eq!(doc.object("body").unwrap().inward_closed_faces(&mut mesh::Context::default()).unwrap().len(), if inward { 6 } else { 0 });
        for glb in [doc.compile_preview(None).unwrap().glb, doc.compile(None).unwrap().glb] {
            assert!((drawn_volume(&glb) - 6.).abs() < 1e-4, "inward={inward}");
        }
    }
}

#[test]
fn captured_inward_saloon_draws_outward() {
    // The recorded conversation wound its body, cab and fender polygons inward.
    let mut doc = Document::new(Limits::default()).unwrap();
    for (request_id, operations) in astra_saloon(doc.limits()) {
        apply(&mut doc, request_id, operations);
        let authored_inward = doc.objects().map(|(_, m)| m.inward_closed_faces(&mut mesh::Context::default()).unwrap().len()).sum::<usize>();
        assert!(authored_inward > 0);
        let draft = doc.compile_preview(None).unwrap();
        assert!(drawn_volume(&draft.glb) > 4., "draft volume {}", drawn_volume(&draft.glb));
        let model = StaticModel::parse_glb(&draft.glb).unwrap();
        assert_eq!(model.indices.len(), draft.triangles * 3);
    }
    assert!(drawn_volume(&doc.compile(None).unwrap().glb) > 4.);
}

fn astra_saloon(limits: &Limits) -> Vec<(&'static str, Vec<Operation>)> {
    vec![
        ("saloon-shape", vec![
        Operation::SetMaterial { material: 1, value: Material { color: [0.16, 0.25, 0.17], ..Default::default() } },
        Operation::Surface(SurfaceOperation::Material {
            material: 1,
            value: SurfaceMaterial {
                base_color: [1.0, 1.0, 1.0, 1.0],
                metallic: 0.5,
                roughness: 0.57,
                ..Default::default()
            }
        }),
        Operation::Surface(SurfaceOperation::Layer {
            material: 1,
            channel: SurfaceChannel::BaseColor,
            layer: SurfaceLayer::new("aged-green", RgbaImage::new(128, 128, [255, 255, 255, 255], limits).unwrap())
        }),
        Operation::Surface(SurfaceOperation::Pattern {
            material: 1,
            channel: SurfaceChannel::BaseColor,
            layer: "aged-green".into(),
            pattern: SurfacePattern {
                kind: PatternKind::Noise,
                color_a: [0.12, 0.2, 0.13, 1.0],
                color_b: [0.23, 0.3, 0.18, 1.0],
                scale: [28.0, 28.0],
                seed: 1936
            }
        }),
        Operation::Surface(SurfaceOperation::Layer {
            material: 1,
            channel: SurfaceChannel::BaseColor,
            layer: SurfaceLayer::new("wear", RgbaImage::new(128, 128, [0, 0, 0, 0], limits).unwrap())
        }),
        Operation::Surface(SurfaceOperation::Stroke {
            material: 1,
            channel: SurfaceChannel::BaseColor,
            layer: "wear".into(),
            points: vec![[0.05, 0.12], [0.3, 0.13], [0.48, 0.11], [0.7, 0.14], [0.94, 0.12]],
            radius: 0.018,
            hardness: 0.5,
            opacity: 0.65,
            color: [0.3, 0.13, 0.055, 1.0],
            mask: false
        }),
        Operation::Surface(SurfaceOperation::Mips { material: 1, channel: SurfaceChannel::BaseColor, layer: "aged-green".into() }),
        Operation::SetMaterial { material: 2, value: Material { color: [0.035, 0.04, 0.035], ..Default::default() } },
        Operation::Surface(SurfaceOperation::Material {
            material: 2,
            value: SurfaceMaterial { base_color: [0.035, 0.04, 0.035, 1.0], roughness: 0.9, ..Default::default() }
        }),
        Operation::SetMaterial { material: 3, value: Material { color: [0.65, 0.66, 0.59], ..Default::default() } },
        Operation::Surface(SurfaceOperation::Material {
            material: 3,
            value: SurfaceMaterial {
                base_color: [0.65, 0.66, 0.59, 1.0],
                metallic: 0.9,
                roughness: 0.29,
                ..Default::default()
            }
        }),
        Operation::SetMaterial { material: 4, value: Material { color: [0.09, 0.17, 0.18], ..Default::default() } },
        Operation::Surface(SurfaceOperation::Material {
            material: 4,
            value: SurfaceMaterial {
                base_color: [0.09, 0.17, 0.18, 1.0],
                metallic: 0.3,
                roughness: 0.14,
                ..Default::default()
            }
        }),
        Operation::SetMaterial { material: 5, value: Material { color: [0.95, 0.77, 0.43], ..Default::default() } },
        Operation::Surface(SurfaceOperation::Material {
            material: 5,
            value: SurfaceMaterial {
                base_color: [0.95, 0.77, 0.43, 1.0],
                roughness: 0.2,
                emissive: [1.0, 0.74, 0.35],
                emissive_strength: 2.0,
                ..Default::default()
            }
        }),
        Operation::SetMaterial { material: 6, value: Material { color: [0.42, 0.025, 0.018], ..Default::default() } },
        Operation::Surface(SurfaceOperation::Material {
            material: 6,
            value: SurfaceMaterial {
                base_color: [0.42, 0.025, 0.018, 1.0],
                roughness: 0.25,
                emissive: [0.7, 0.015, 0.005],
                emissive_strength: 0.5,
                ..Default::default()
            }
        }),
        Operation::polygon_mesh(
            "body",
            &[
                [-0.79, 0.53, -2.05],
                [0.79, 0.53, -2.05],
                [0.79, 0.53, 2.05],
                [-0.79, 0.53, 2.05],
                [-0.85, 1.04, -1.94],
                [0.85, 1.04, -1.94],
                [0.68, 1.17, 1.94],
                [-0.68, 1.17, 1.94]
            ],
            &vec![
                mesh::Polygon {
                    vertices: vec![3, 2, 1, 0],
                    uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    material: 2
                },
                mesh::Polygon {
                    vertices: vec![0, 1, 5, 4],
                    uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    material: 1
                },
                mesh::Polygon {
                    vertices: vec![1, 2, 6, 5],
                    uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    material: 1
                },
                mesh::Polygon {
                    vertices: vec![2, 3, 7, 6],
                    uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    material: 1
                },
                mesh::Polygon {
                    vertices: vec![3, 0, 4, 7],
                    uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    material: 1
                },
                mesh::Polygon {
                    vertices: vec![4, 5, 6, 7],
                    uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    material: 1
                }
            ],
            limits
        ).unwrap(),
        Operation::polygon_mesh(
            "cab",
            &[
                [-0.78, 1.02, -1.5],
                [0.78, 1.02, -1.5],
                [0.74, 1.09, 0.53],
                [-0.74, 1.09, 0.53],
                [-0.65, 1.88, -1.15],
                [0.65, 1.88, -1.15],
                [0.62, 1.88, 0.11],
                [-0.62, 1.88, 0.11]
            ],
            &vec![
                mesh::Polygon { vertices: vec![3, 2, 1, 0], uvs: vec![], material: 1 },
                mesh::Polygon {
                    vertices: vec![0, 1, 5, 4],
                    uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    material: 1
                },
                mesh::Polygon {
                    vertices: vec![1, 2, 6, 5],
                    uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    material: 1
                },
                mesh::Polygon {
                    vertices: vec![2, 3, 7, 6],
                    uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    material: 1
                },
                mesh::Polygon {
                    vertices: vec![3, 0, 4, 7],
                    uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    material: 1
                },
                mesh::Polygon { vertices: vec![4, 5, 6, 7], uvs: vec![], material: 2 }
            ],
            limits
        ).unwrap(),
        Operation::polygon_mesh(
            "windows",
            &[
                [-0.65, 1.29, 0.441],
                [0.65, 1.29, 0.441],
                [0.55, 1.78, 0.19],
                [-0.55, 1.78, 0.19],
                [0.759, 1.27, -0.46],
                [0.734, 1.27, 0.36],
                [0.64, 1.77, 0.035],
                [0.663, 1.77, -0.46],
                [0.768, 1.26, -1.3],
                [0.759, 1.27, -0.57],
                [0.663, 1.77, -0.57],
                [0.674, 1.77, -1.08],
                [-0.759, 1.27, -0.46],
                [-0.734, 1.27, 0.36],
                [-0.64, 1.77, 0.035],
                [-0.663, 1.77, -0.46],
                [-0.768, 1.26, -1.3],
                [-0.759, 1.27, -0.57],
                [-0.663, 1.77, -0.57],
                [-0.674, 1.77, -1.08],
                [-0.57, 1.3, -1.394],
                [0.57, 1.3, -1.394],
                [0.53, 1.77, -1.203],
                [-0.53, 1.77, -1.203]
            ],
            &vec![
                mesh::Polygon { vertices: vec![1, 0, 3, 2], uvs: vec![], material: 4 },
                mesh::Polygon { vertices: vec![4, 5, 6, 7], uvs: vec![], material: 4 },
                mesh::Polygon { vertices: vec![8, 9, 10, 11], uvs: vec![], material: 4 },
                mesh::Polygon { vertices: vec![13, 12, 15, 14], uvs: vec![], material: 4 },
                mesh::Polygon { vertices: vec![17, 16, 19, 18], uvs: vec![], material: 4 },
                mesh::Polygon { vertices: vec![20, 21, 22, 23], uvs: vec![], material: 4 }
            ],
            limits
        ).unwrap(),
        Operation::cylinder("front_left_tire", 0.4, 0.24, 16, true, limits).unwrap(),
        Operation::cylinder("front_right_tire", 0.4, 0.24, 16, true, limits).unwrap(),
        Operation::cylinder("rear_left_tire", 0.4, 0.24, 16, true, limits).unwrap(),
        Operation::cylinder("rear_right_tire", 0.4, 0.24, 16, true, limits).unwrap(),
    ]),
        ("saloon-running-gear-and-trim", vec![
        Operation::Transform {
            object: "front_left_tire".into(),
            vertices: vec![
                mesh::VertexId(1), mesh::VertexId(2), mesh::VertexId(3), mesh::VertexId(4), mesh::VertexId(5),
                mesh::VertexId(6), mesh::VertexId(7), mesh::VertexId(8), mesh::VertexId(9), mesh::VertexId(10),
                mesh::VertexId(11), mesh::VertexId(12), mesh::VertexId(13), mesh::VertexId(14), mesh::VertexId(15),
                mesh::VertexId(16), mesh::VertexId(17), mesh::VertexId(18), mesh::VertexId(19), mesh::VertexId(20),
                mesh::VertexId(21), mesh::VertexId(22), mesh::VertexId(23), mesh::VertexId(24), mesh::VertexId(25),
                mesh::VertexId(26), mesh::VertexId(27), mesh::VertexId(28), mesh::VertexId(29), mesh::VertexId(30),
                mesh::VertexId(31), mesh::VertexId(32)
            ],
            matrix: [[0.0, -1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]
        },
        Operation::Transform {
            object: "front_right_tire".into(),
            vertices: vec![
                mesh::VertexId(1), mesh::VertexId(2), mesh::VertexId(3), mesh::VertexId(4), mesh::VertexId(5),
                mesh::VertexId(6), mesh::VertexId(7), mesh::VertexId(8), mesh::VertexId(9), mesh::VertexId(10),
                mesh::VertexId(11), mesh::VertexId(12), mesh::VertexId(13), mesh::VertexId(14), mesh::VertexId(15),
                mesh::VertexId(16), mesh::VertexId(17), mesh::VertexId(18), mesh::VertexId(19), mesh::VertexId(20),
                mesh::VertexId(21), mesh::VertexId(22), mesh::VertexId(23), mesh::VertexId(24), mesh::VertexId(25),
                mesh::VertexId(26), mesh::VertexId(27), mesh::VertexId(28), mesh::VertexId(29), mesh::VertexId(30),
                mesh::VertexId(31), mesh::VertexId(32)
            ],
            matrix: [[0.0, -1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]
        },
        Operation::Transform {
            object: "rear_left_tire".into(),
            vertices: vec![
                mesh::VertexId(1), mesh::VertexId(2), mesh::VertexId(3), mesh::VertexId(4), mesh::VertexId(5),
                mesh::VertexId(6), mesh::VertexId(7), mesh::VertexId(8), mesh::VertexId(9), mesh::VertexId(10),
                mesh::VertexId(11), mesh::VertexId(12), mesh::VertexId(13), mesh::VertexId(14), mesh::VertexId(15),
                mesh::VertexId(16), mesh::VertexId(17), mesh::VertexId(18), mesh::VertexId(19), mesh::VertexId(20),
                mesh::VertexId(21), mesh::VertexId(22), mesh::VertexId(23), mesh::VertexId(24), mesh::VertexId(25),
                mesh::VertexId(26), mesh::VertexId(27), mesh::VertexId(28), mesh::VertexId(29), mesh::VertexId(30),
                mesh::VertexId(31), mesh::VertexId(32)
            ],
            matrix: [[0.0, -1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]
        },
        Operation::Transform {
            object: "rear_right_tire".into(),
            vertices: vec![
                mesh::VertexId(1), mesh::VertexId(2), mesh::VertexId(3), mesh::VertexId(4), mesh::VertexId(5),
                mesh::VertexId(6), mesh::VertexId(7), mesh::VertexId(8), mesh::VertexId(9), mesh::VertexId(10),
                mesh::VertexId(11), mesh::VertexId(12), mesh::VertexId(13), mesh::VertexId(14), mesh::VertexId(15),
                mesh::VertexId(16), mesh::VertexId(17), mesh::VertexId(18), mesh::VertexId(19), mesh::VertexId(20),
                mesh::VertexId(21), mesh::VertexId(22), mesh::VertexId(23), mesh::VertexId(24), mesh::VertexId(25),
                mesh::VertexId(26), mesh::VertexId(27), mesh::VertexId(28), mesh::VertexId(29), mesh::VertexId(30),
                mesh::VertexId(31), mesh::VertexId(32)
            ],
            matrix: [[0.0, -1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]
        },
        Operation::AssignMaterial {
            object: "front_left_tire".into(),
            faces: vec![
                mesh::FaceId(33), mesh::FaceId(50), mesh::FaceId(67), mesh::FaceId(72), mesh::FaceId(77),
                mesh::FaceId(82), mesh::FaceId(87), mesh::FaceId(92), mesh::FaceId(97), mesh::FaceId(102),
                mesh::FaceId(107), mesh::FaceId(112), mesh::FaceId(117), mesh::FaceId(122), mesh::FaceId(127),
                mesh::FaceId(132), mesh::FaceId(137), mesh::FaceId(142)
            ],
            material: 2
        },
        Operation::AssignMaterial {
            object: "front_right_tire".into(),
            faces: vec![
                mesh::FaceId(33), mesh::FaceId(50), mesh::FaceId(67), mesh::FaceId(72), mesh::FaceId(77),
                mesh::FaceId(82), mesh::FaceId(87), mesh::FaceId(92), mesh::FaceId(97), mesh::FaceId(102),
                mesh::FaceId(107), mesh::FaceId(112), mesh::FaceId(117), mesh::FaceId(122), mesh::FaceId(127),
                mesh::FaceId(132), mesh::FaceId(137), mesh::FaceId(142)
            ],
            material: 2
        },
        Operation::AssignMaterial {
            object: "rear_left_tire".into(),
            faces: vec![
                mesh::FaceId(33), mesh::FaceId(50), mesh::FaceId(67), mesh::FaceId(72), mesh::FaceId(77),
                mesh::FaceId(82), mesh::FaceId(87), mesh::FaceId(92), mesh::FaceId(97), mesh::FaceId(102),
                mesh::FaceId(107), mesh::FaceId(112), mesh::FaceId(117), mesh::FaceId(122), mesh::FaceId(127),
                mesh::FaceId(132), mesh::FaceId(137), mesh::FaceId(142)
            ],
            material: 2
        },
        Operation::AssignMaterial {
            object: "rear_right_tire".into(),
            faces: vec![
                mesh::FaceId(33), mesh::FaceId(50), mesh::FaceId(67), mesh::FaceId(72), mesh::FaceId(77),
                mesh::FaceId(82), mesh::FaceId(87), mesh::FaceId(92), mesh::FaceId(97), mesh::FaceId(102),
                mesh::FaceId(107), mesh::FaceId(112), mesh::FaceId(117), mesh::FaceId(122), mesh::FaceId(127),
                mesh::FaceId(132), mesh::FaceId(137), mesh::FaceId(142)
            ],
            material: 2
        },
        Operation::Scene(SceneOperation::Node {
            object: "front_left_tire".into(),
            node: SceneNode {
                parent: None,
                linked_to: None,
                transform: Transform {
                    translation: [0.88, 0.4, 1.32],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0]
                }
            }
        }),
        Operation::Scene(SceneOperation::Node {
            object: "front_right_tire".into(),
            node: SceneNode {
                parent: None,
                linked_to: None,
                transform: Transform {
                    translation: [-0.88, 0.4, 1.32],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0]
                }
            }
        }),
        Operation::Scene(SceneOperation::Node {
            object: "rear_left_tire".into(),
            node: SceneNode {
                parent: None,
                linked_to: None,
                transform: Transform {
                    translation: [0.88, 0.4, -1.34],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0]
                }
            }
        }),
        Operation::Scene(SceneOperation::Node {
            object: "rear_right_tire".into(),
            node: SceneNode {
                parent: None,
                linked_to: None,
                transform: Transform {
                    translation: [-0.88, 0.4, -1.34],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0]
                }
            }
        }),
        Operation::Scene(SceneOperation::VehicleWheel {
            object: "front_left_tire".into(),
            wheel: VehicleWheel {
                connection: "wheel_front_left".into(),
                pivot: [0.0, 0.0, 0.0],
                radius: 0.4,
                width: 0.24,
                visual: None
            }
        }),
        Operation::Scene(SceneOperation::VehicleWheel {
            object: "front_right_tire".into(),
            wheel: VehicleWheel {
                connection: "wheel_front_right".into(),
                pivot: [0.0, 0.0, 0.0],
                radius: 0.4,
                width: 0.24,
                visual: None
            }
        }),
        Operation::Scene(SceneOperation::VehicleWheel {
            object: "rear_left_tire".into(),
            wheel: VehicleWheel {
                connection: "wheel_rear_left".into(),
                pivot: [0.0, 0.0, 0.0],
                radius: 0.4,
                width: 0.24,
                visual: None
            }
        }),
        Operation::Scene(SceneOperation::VehicleWheel {
            object: "rear_right_tire".into(),
            wheel: VehicleWheel {
                connection: "wheel_rear_right".into(),
                pivot: [0.0, 0.0, 0.0],
                radius: 0.4,
                width: 0.24,
                visual: None
            }
        }),
        Operation::polygon_mesh(
            "fender",
            &[
                [-0.21, 0.57, -0.64],
                [-0.21, 0.81, -0.42],
                [-0.21, 0.94, -0.19],
                [-0.21, 0.94, 0.23],
                [-0.21, 0.8, 0.47],
                [-0.21, 0.57, 0.64],
                [0.21, 0.57, -0.64],
                [0.21, 0.81, -0.42],
                [0.21, 0.94, -0.19],
                [0.21, 0.94, 0.23],
                [0.21, 0.8, 0.47],
                [0.21, 0.57, 0.64]
            ],
            &vec![
                mesh::Polygon {
                    vertices: vec![0, 1, 2, 3, 4, 5],
                    uvs: vec![[0.0, 0.0], [0.17, 0.65], [0.35, 1.0], [0.68, 1.0], [0.87, 0.62], [1.0, 0.0]],
                    material: 1
                },
                mesh::Polygon {
                    vertices: vec![11, 10, 9, 8, 7, 6],
                    uvs: vec![[1.0, 0.0], [0.87, 0.62], [0.68, 1.0], [0.35, 1.0], [0.17, 0.65], [0.0, 0.0]],
                    material: 1
                },
                mesh::Polygon { vertices: vec![1, 0, 6, 7], uvs: vec![], material: 1 },
                mesh::Polygon { vertices: vec![2, 1, 7, 8], uvs: vec![], material: 1 },
                mesh::Polygon { vertices: vec![3, 2, 8, 9], uvs: vec![], material: 1 },
                mesh::Polygon { vertices: vec![4, 3, 9, 10], uvs: vec![], material: 1 },
                mesh::Polygon { vertices: vec![5, 4, 10, 11], uvs: vec![], material: 1 },
                mesh::Polygon { vertices: vec![0, 5, 11, 6], uvs: vec![], material: 2 }
            ],
            limits
        ).unwrap(),
        Operation::Scene(SceneOperation::Node {
            object: "fender".into(),
            node: SceneNode {
                parent: None,
                linked_to: None,
                transform: Transform {
                    translation: [0.86, 0.0, 1.32],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0]
                }
            }
        }),
        Operation::Scene(SceneOperation::Instance {
            object: "fender-fr".into(),
            source: "fender".into(),
            transform: Transform {
                translation: [-0.86, 0.0, 1.32],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0]
            }
        }),
        Operation::Scene(SceneOperation::Instance {
            object: "fender-rl".into(),
            source: "fender".into(),
            transform: Transform {
                translation: [0.86, 0.0, -1.34],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0]
            }
        }),
        Operation::Scene(SceneOperation::Instance {
            object: "fender-rr".into(),
            source: "fender".into(),
            transform: Transform {
                translation: [-0.86, 0.0, -1.34],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0]
            }
        }),
        Operation::Cube { object: "front-bumper".into(), size: [1.94, 0.12, 0.15] },
        Operation::Scene(SceneOperation::Node {
            object: "front-bumper".into(),
            node: SceneNode {
                parent: None,
                linked_to: None,
                transform: Transform {
                    translation: [0.0, 0.51, 2.14],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0]
                }
            }
        }),
        Operation::Cube { object: "rear-bumper".into(), size: [1.94, 0.12, 0.15] },
        Operation::Scene(SceneOperation::Node {
            object: "rear-bumper".into(),
            node: SceneNode {
                parent: None,
                linked_to: None,
                transform: Transform {
                    translation: [0.0, 0.51, -2.14],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0]
                }
            }
        }),
        Operation::Cube { object: "running-board".into(), size: [0.33, 0.09, 1.55] },
        Operation::Scene(SceneOperation::Node {
            object: "running-board".into(),
            node: SceneNode {
                parent: None,
                linked_to: None,
                transform: Transform {
                    translation: [0.88, 0.45, -0.02],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0]
                }
            }
        }),
        Operation::Cube { object: "grille-frame".into(), size: [0.75, 0.64, 0.08] },
        Operation::Scene(SceneOperation::Node {
            object: "grille-frame".into(),
            node: SceneNode {
                parent: None,
                linked_to: None,
                transform: Transform {
                    translation: [0.0, 0.86, 2.01],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0]
                }
            }
        }),
        Operation::Cube { object: "grille-dark".into(), size: [0.65, 0.54, 0.03] },
        Operation::Scene(SceneOperation::Node {
            object: "grille-dark".into(),
            node: SceneNode {
                parent: None,
                linked_to: None,
                transform: Transform {
                    translation: [0.0, 0.86, 2.065],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0]
                }
            }
        }),
        Operation::Cube { object: "grille-slat".into(), size: [0.025, 0.51, 0.025] },
        Operation::Scene(SceneOperation::Node {
            object: "grille-slat".into(),
            node: SceneNode {
                parent: None,
                linked_to: None,
                transform: Transform {
                    translation: [-0.27, 0.86, 2.09],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0]
                }
            }
        }),
        Operation::sphere("headlamp-shell", 0.18, 12, 6, true, limits).unwrap(),
        Operation::Scene(SceneOperation::Node {
            object: "headlamp-shell".into(),
            node: SceneNode {
                parent: None,
                linked_to: None,
                transform: Transform {
                    translation: [0.62, 1.02, 1.92],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 0.9]
                }
            }
        }),
        Operation::sphere("headlamp-lens", 0.145, 12, 6, true, limits).unwrap(),
        Operation::Scene(SceneOperation::Node {
            object: "headlamp-lens".into(),
            node: SceneNode {
                parent: None,
                linked_to: None,
                transform: Transform {
                    translation: [0.62, 1.02, 2.065],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 0.23]
                }
            }
        }),
        Operation::sphere("hubcap", 0.245, 12, 6, true, limits).unwrap(),
        Operation::Scene(SceneOperation::Node {
            object: "hubcap".into(),
            node: SceneNode {
                parent: Some("front_left_tire".into()),
                linked_to: None,
                transform: Transform {
                    translation: [0.13, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [0.22, 1.0, 1.0]
                }
            }
        }),
        Operation::Cube { object: "handle".into(), size: [0.045, 0.035, 0.17] },
        Operation::Scene(SceneOperation::Node {
            object: "handle".into(),
            node: SceneNode {
                parent: None,
                linked_to: None,
                transform: Transform {
                    translation: [0.81, 1.18, -0.32],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0]
                }
            }
        }),
        Operation::Cube { object: "hood-trim".into(), size: [0.025, 0.022, 1.4] },
        Operation::Scene(SceneOperation::Node {
            object: "hood-trim".into(),
            node: SceneNode {
                parent: None,
                linked_to: None,
                transform: Transform {
                    translation: [0.0, 1.153, 1.22],
                    rotation: [-0.018, 0.0, 0.0, 0.9998379869],
                    scale: [1.0, 1.0, 1.0]
                }
            }
        }),
        Operation::Cube { object: "tail-lamp".into(), size: [0.15, 0.11, 0.06] },
        Operation::Scene(SceneOperation::Node {
            object: "tail-lamp".into(),
            node: SceneNode {
                parent: None,
                linked_to: None,
                transform: Transform {
                    translation: [0.63, 0.92, -2.015],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0]
                }
            }
        }),
        Operation::Scene(SceneOperation::Light(LightEmitter {
            name: "headlight-left".into(),
            attachment: Attachment::Root,
            transform: Transform {
                translation: [0.62, 1.02, 2.11],
                rotation: [0.0, 1.0, 0.0, 0.0],
                scale: [1.0, 1.0, 1.0]
            },
            kind: LightKind::Spot { inner: 0.18, outer: 0.48 },
            color: [1.0, 0.82, 0.55],
            intensity: 420.0,
            range: 28.0
        })),
        Operation::Scene(SceneOperation::Light(LightEmitter {
            name: "headlight-right".into(),
            attachment: Attachment::Root,
            transform: Transform {
                translation: [-0.62, 1.02, 2.11],
                rotation: [0.0, 1.0, 0.0, 0.0],
                scale: [1.0, 1.0, 1.0]
            },
            kind: LightKind::Spot { inner: 0.18, outer: 0.48 },
            color: [1.0, 0.82, 0.55],
            intensity: 420.0,
            range: 28.0
        })),
    ]),
        ("saloon-finish", vec![
        Operation::Surface(SurfaceOperation::Material {
            material: 0,
            value: SurfaceMaterial {
                base_color: [0.65, 0.66, 0.59, 1.0],
                metallic: 0.9,
                roughness: 0.3,
                ..Default::default()
            }
        }),
        Operation::AssignMaterial {
            object: "grille-dark".into(),
            faces: vec![
                mesh::FaceId(9), mesh::FaceId(14), mesh::FaceId(19), mesh::FaceId(24), mesh::FaceId(29),
                mesh::FaceId(34)
            ],
            material: 2
        },
        Operation::AssignMaterial {
            object: "tail-lamp".into(),
            faces: vec![
                mesh::FaceId(9), mesh::FaceId(14), mesh::FaceId(19), mesh::FaceId(24), mesh::FaceId(29),
                mesh::FaceId(34)
            ],
            material: 6
        },
        Operation::AssignMaterial {
            object: "headlamp-lens".into(),
            faces: vec![
                mesh::FaceId(63), mesh::FaceId(67), mesh::FaceId(72), mesh::FaceId(77), mesh::FaceId(82),
                mesh::FaceId(87), mesh::FaceId(91), mesh::FaceId(95), mesh::FaceId(100), mesh::FaceId(105),
                mesh::FaceId(110), mesh::FaceId(115), mesh::FaceId(119), mesh::FaceId(123), mesh::FaceId(128),
                mesh::FaceId(133), mesh::FaceId(138), mesh::FaceId(143), mesh::FaceId(147), mesh::FaceId(151),
                mesh::FaceId(156), mesh::FaceId(161), mesh::FaceId(166), mesh::FaceId(171), mesh::FaceId(175),
                mesh::FaceId(179), mesh::FaceId(184), mesh::FaceId(189), mesh::FaceId(194), mesh::FaceId(199),
                mesh::FaceId(203), mesh::FaceId(207), mesh::FaceId(212), mesh::FaceId(217), mesh::FaceId(222),
                mesh::FaceId(227), mesh::FaceId(231), mesh::FaceId(235), mesh::FaceId(240), mesh::FaceId(245),
                mesh::FaceId(250), mesh::FaceId(255), mesh::FaceId(259), mesh::FaceId(263), mesh::FaceId(268),
                mesh::FaceId(273), mesh::FaceId(278), mesh::FaceId(283), mesh::FaceId(287), mesh::FaceId(291),
                mesh::FaceId(296), mesh::FaceId(301), mesh::FaceId(306), mesh::FaceId(311), mesh::FaceId(315),
                mesh::FaceId(319), mesh::FaceId(324), mesh::FaceId(329), mesh::FaceId(334), mesh::FaceId(339),
                mesh::FaceId(343), mesh::FaceId(347), mesh::FaceId(352), mesh::FaceId(357), mesh::FaceId(362),
                mesh::FaceId(367), mesh::FaceId(371), mesh::FaceId(375), mesh::FaceId(380), mesh::FaceId(385),
                mesh::FaceId(390), mesh::FaceId(395)
            ],
            material: 5
        },
        Operation::Scene(SceneOperation::Instance {
            object: "headlamp-shell-r".into(),
            source: "headlamp-shell".into(),
            transform: Transform {
                translation: [-0.62, 1.02, 1.92],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 0.9]
            }
        }),
        Operation::Scene(SceneOperation::Instance {
            object: "headlamp-lens-r".into(),
            source: "headlamp-lens".into(),
            transform: Transform {
                translation: [-0.62, 1.02, 2.065],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 0.23]
            }
        }),
        Operation::Scene(SceneOperation::Instance {
            object: "tail-lamp-r".into(),
            source: "tail-lamp".into(),
            transform: Transform {
                translation: [-0.63, 0.92, -2.015],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0]
            }
        }),
        Operation::Scene(SceneOperation::Instance {
            object: "running-board-r".into(),
            source: "running-board".into(),
            transform: Transform {
                translation: [-0.88, 0.45, -0.02],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0]
            }
        }),
        Operation::Scene(SceneOperation::Instance {
            object: "handle-r".into(),
            source: "handle".into(),
            transform: Transform {
                translation: [-0.81, 1.18, -0.32],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0]
            }
        }),
        Operation::Scene(SceneOperation::Instance {
            object: "handle-rear-l".into(),
            source: "handle".into(),
            transform: Transform {
                translation: [0.825, 1.18, -1.12],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0]
            }
        }),
        Operation::Scene(SceneOperation::Instance {
            object: "handle-rear-r".into(),
            source: "handle".into(),
            transform: Transform {
                translation: [-0.825, 1.18, -1.12],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0]
            }
        }),
        Operation::Scene(SceneOperation::Instance {
            object: "slat2".into(),
            source: "grille-slat".into(),
            transform: Transform {
                translation: [-0.18, 0.86, 2.09],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0]
            }
        }),
        Operation::Scene(SceneOperation::Instance {
            object: "slat3".into(),
            source: "grille-slat".into(),
            transform: Transform {
                translation: [-0.09, 0.86, 2.09],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0]
            }
        }),
        Operation::Scene(SceneOperation::Instance {
            object: "slat4".into(),
            source: "grille-slat".into(),
            transform: Transform {
                translation: [0.0, 0.86, 2.09],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0]
            }
        }),
        Operation::Scene(SceneOperation::Instance {
            object: "slat5".into(),
            source: "grille-slat".into(),
            transform: Transform {
                translation: [0.09, 0.86, 2.09],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0]
            }
        }),
        Operation::Scene(SceneOperation::Instance {
            object: "slat6".into(),
            source: "grille-slat".into(),
            transform: Transform {
                translation: [0.18, 0.86, 2.09],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0]
            }
        }),
        Operation::Scene(SceneOperation::Instance {
            object: "slat7".into(),
            source: "grille-slat".into(),
            transform: Transform {
                translation: [0.27, 0.86, 2.09],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0]
            }
        }),
        Operation::Scene(SceneOperation::Instance { object: "hubcap-fr".into(), source: "hubcap".into(), transform: Transform::default() }),
        Operation::Scene(SceneOperation::Node {
            object: "hubcap-fr".into(),
            node: SceneNode {
                parent: Some("front_right_tire".into()),
                linked_to: Some("hubcap".into()),
                transform: Transform {
                    translation: [-0.13, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [0.22, 1.0, 1.0]
                }
            }
        }),
        Operation::Scene(SceneOperation::Instance { object: "hubcap-rl".into(), source: "hubcap".into(), transform: Transform::default() }),
        Operation::Scene(SceneOperation::Node {
            object: "hubcap-rl".into(),
            node: SceneNode {
                parent: Some("rear_left_tire".into()),
                linked_to: Some("hubcap".into()),
                transform: Transform {
                    translation: [0.13, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [0.22, 1.0, 1.0]
                }
            }
        }),
        Operation::Scene(SceneOperation::Instance { object: "hubcap-rr".into(), source: "hubcap".into(), transform: Transform::default() }),
        Operation::Scene(SceneOperation::Node {
            object: "hubcap-rr".into(),
            node: SceneNode {
                parent: Some("rear_right_tire".into()),
                linked_to: Some("hubcap".into()),
                transform: Transform {
                    translation: [-0.13, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [0.22, 1.0, 1.0]
                }
            }
        }),
        Operation::Surface(SurfaceOperation::Mips { material: 1, channel: SurfaceChannel::BaseColor, layer: "wear".into() }),
    ]),
    ]
}
