use makepad_model::*;
use makepad_gltf::JsonValue;
use std::collections::{BTreeMap, BTreeSet};
fn apply(d:&mut Document,id:&str,ops:Vec<Operation>){d.apply(Transaction{request_id:id.into(),expected:d.head(),operations:ops},None).unwrap();}
fn field<'a>(v:&'a JsonValue,key:&str)->&'a JsonValue{let JsonValue::Object(v)=v else{panic!("object")}; &v[key]}
fn arr(v:&JsonValue)->&[JsonValue]{let JsonValue::Array(v)=v else{panic!("array")};v}
fn idx(v:&JsonValue)->usize{match v{JsonValue::U64(i)=>*i as usize,JsonValue::I64(i)=>*i as usize,_=>panic!("index")}}
fn make(skinned:bool)->Document{
    let mut d=Document::new(Limits::default()).unwrap();let ops=vec![
        Operation::sphere("ball", 1.0, 12, 8, true, d.limits()).unwrap(),
    ];apply(&mut d,"shape",ops);
    if skinned{apply(&mut d,"rig",vec![Operation::SetSkeleton{skeleton:Skeleton{joints:vec![Joint{name:"root".into(),parent:None,translation:[0.;3]}]}},Operation::AutoWeights{object:"ball".into()}]);}
    apply(&mut d,"delivery",vec![Operation::Scene(SceneOperation::Lods{object:"ball".into(),levels:vec![LodLevel{target_faces:120,max_error:1.,distance:10.},LodLevel{target_faces:80,max_error:2.,distance:30.}]}),Operation::Scene(SceneOperation::Collider{object:"ball".into(),proxy:CollisionProxy::Sphere})]);d
}
#[test] fn lod_geometry_is_real_reduced_hidden_and_collision_is_portable(){verify(false)}
#[test] fn skinned_lods_keep_common_skin_and_rest_space(){verify(true)}
#[test]
fn painted_colors_survive_every_static_and_skinned_lod() {
    for skinned in [false,true] {
        let mut d=make(skinned);
        let vertices=d.object("ball").unwrap().vertices().iter().map(|v|v.id).collect();
        let color=[0.8,0.12,0.05,1.];
        apply(&mut d,"paint",vec![Operation::Surface(SurfaceOperation::VertexPaint{
            object:"ball".into(),vertices,color,opacity:1.,
        })]);
        let before=d.to_bytes(None).unwrap();
        let product=d.compile(None).unwrap();
        assert_eq!(d.to_bytes(None).unwrap(),before,"derived paint must not change the source");
        let g=makepad_gltf::load_gltf_from_bytes(&product.glb,None).unwrap();
        let nodes=g.document.nodes_slice();
        let base=nodes.iter().position(|node|node.extensions.as_ref().is_some_and(|value|
            matches!(value,JsonValue::Object(fields)if fields.contains_key("MSFT_lod")))).unwrap();
        let levels=arr(field(field(nodes[base].extensions.as_ref().unwrap(),"MSFT_lod"),"ids"));
        assert_eq!(levels.len(),2);
        for level in levels {
            assert_ne!(nodes[idx(level)].mesh, nodes[base].mesh, "paint is checked on derived geometry");
        }
        for node in std::iter::once(base).chain(levels.iter().map(idx)) {
            let mesh=nodes[node].mesh.unwrap();
            for primitive in 0..g.document.meshes_slice()[mesh].primitives.len() {
                let decoded=makepad_gltf::decode_mesh_primitive(&g,mesh,primitive).unwrap();
                let colors=decoded.colors0.expect("COLOR_0 at every LOD");
                assert!(!colors.is_empty());
                assert!(colors.iter().all(|actual|actual.iter().zip(color).all(|(a,b)|(*a as f64-b).abs()<1e-6)),
                    "paint changed at node {node}, skinned={skinned}");
            }
        }
    }
}
fn verify(skinned:bool){let d=make(skinned);let before=d.to_bytes(None).unwrap();let product=d.compile(None).unwrap();assert_eq!(d.to_bytes(None).unwrap(),before);
    if skinned { verify_derived_weights(&d, &product); }
    let g=makepad_gltf::parse_glb_bytes(&product.glb).unwrap();let nodes=g.document.nodes_slice();let base=nodes.iter().position(|n|n.extensions.as_ref().is_some_and(|v|matches!(v,JsonValue::Object(f)if f.contains_key("MSFT_lod")))).unwrap();
    let ids=arr(field(field(nodes[base].extensions.as_ref().unwrap(),"MSFT_lod"),"ids"));assert_eq!(ids.len(),2);
    let count=|i:usize|g.document.meshes_slice()[nodes[i].mesh.unwrap()].primitives.iter().map(|p|g.document.accessors_slice()[p.indices.unwrap()].count/3).sum::<usize>();
    let mut previous=count(base);for id in ids{let id=idx(id);let n=count(id);assert!(n<previous);previous=n;assert!(nodes.iter().all(|n|!n.children.as_ref().is_some_and(|c|c.contains(&id))));if skinned{assert_eq!(nodes[id].skin,Some(0));}}
    let owner=nodes.iter().find(|n|n.name.as_deref()==Some("ball")).unwrap();assert!(matches!(field(field(owner.extras.as_ref().unwrap(),"MAKEPAD_collision"),"kind"),JsonValue::String(s)if s=="sphere"));
    let mut d=d;
    apply(&mut d,"repeat-and-drop",vec![Operation::Scene(SceneOperation::Node{object:"ball".into(),node:SceneNode{transform:Transform{translation:[3.,2.,1.],scale:[2.;3],..Default::default()},..Default::default()}}),
        Operation::Scene(SceneOperation::Lods{object:"ball".into(),levels:vec![
            LodLevel{target_faces:120,max_error:0.,distance:10.},
            LodLevel{target_faces:120,max_error:2.,distance:20.},
            LodLevel{target_faces:120,max_error:2.,distance:30.},
            LodLevel{target_faces:0,max_error:0.,distance:40.},
        ]})]);
    let product=d.compile(None).unwrap();assert!(product.warnings.is_empty(),"an unreducible level is data, not a warning: {:?}",product.warnings);
    let g=makepad_gltf::parse_glb_bytes(&product.glb).unwrap();let nodes=g.document.nodes_slice();
    let owner=nodes.iter().find(|n|n.extensions.as_ref().is_some_and(|e|e.key("MSFT_lod").is_some())).unwrap();
    let levels=arr(field(field(owner.extensions.as_ref().unwrap(),"MSFT_lod"),"ids")).iter().map(|i|&nodes[idx(i)]).collect::<Vec<_>>();
    assert_eq!(levels[0].mesh,owner.mesh,"unreducible first level shares the base mesh");
    assert_ne!(levels[1].mesh,owner.mesh,"other levels still reduce");
    assert_eq!(levels[2].mesh,levels[1].mesh,"unreducible later level shares the previous mesh");
    assert_eq!(levels[3].mesh,None,"drop has no geometry");
    for level in levels{assert_eq!(level.skin,owner.skin);assert_eq!(level.translation,owner.translation);assert_eq!(level.rotation,owner.rotation);assert_eq!(level.scale,owner.scale);assert!(level.children.as_deref().unwrap_or(&[]).is_empty());}
}

fn verify_derived_weights(doc: &Document, product: &CompiledModel) {
    let gltf = makepad_gltf::load_gltf_from_bytes(&product.glb, None).unwrap();
    let mut checked = 0;
    for source in product.primitives.iter().filter(|p| p.object != "ball") {
        // A skinned export puts each object's mesh on its `<name>__skin` node.
        let geometry = format!("{}__skin", source.object);
        let node = gltf
            .document
            .nodes_slice()
            .iter()
            .find(|n| n.name.as_deref() == Some(geometry.as_str()))
            .unwrap_or_else(|| panic!("skinned geometry node {geometry}"));
        let mesh = node.mesh.unwrap();
        let primitive = &gltf.document.meshes_slice()[mesh].primitives[0];
        let decoded = makepad_gltf::decode_mesh_primitive(&gltf, mesh, 0).unwrap();
        let joint_accessor = primitive.attributes["JOINTS_0"];
        let weight_accessor = primitive.attributes["WEIGHTS_0"];
        let joint_bytes = match gltf.document.accessors_slice()[joint_accessor].component_type {
            makepad_gltf::GLTF_COMPONENT_TYPE_UNSIGNED_BYTE => 1,
            makepad_gltf::GLTF_COMPONENT_TYPE_UNSIGNED_SHORT => 2,
            other => panic!("unexpected joint component type {other}"),
        };
        assert_eq!(
            gltf.document.accessors_slice()[weight_accessor].component_type,
            makepad_gltf::GLTF_COMPONENT_TYPE_FLOAT
        );
        let item = |accessor: usize, vertex: usize, size: usize| {
            let a = &gltf.document.accessors_slice()[accessor];
            let view = &gltf.document.buffer_views_slice()[a.buffer_view.unwrap()];
            let start = view.byte_offset.unwrap_or(0)
                + a.byte_offset.unwrap_or(0)
                + vertex * view.byte_stride.unwrap_or(size);
            &gltf.buffers[view.buffer][start..start + size]
        };
        assert_eq!(decoded.indices.len(), source.source_vertices.len());
        for (&index, &vertex) in decoded.indices.iter().zip(&source.source_vertices) {
            let original = doc
                .object("ball")
                .unwrap()
                .vertex(vertex)
                .expect("kept source vertex");
            assert_eq!(
                decoded.positions[index as usize],
                original.position.map(|p| p as f32),
                "derived vertex stays in source rest space"
            );
            let joints = item(joint_accessor, index as usize, 4 * joint_bytes);
            let weights = item(weight_accessor, index as usize, 16);
            let mut actual = Vec::new();
            for lane in 0..4 {
                let weight =
                    f32::from_le_bytes(weights[lane * 4..lane * 4 + 4].try_into().unwrap());
                if weight == 0. {
                    continue;
                }
                let joint = if joint_bytes == 1 {
                    joints[lane] as u32
                } else {
                    u16::from_le_bytes(joints[lane * 2..lane * 2 + 2].try_into().unwrap()) as u32
                };
                actual.push((joint, weight));
            }
            actual.sort_by_key(|&(joint, _)| joint);
            let expected = original
                .weights
                .iter()
                .map(|w| (w.joint, w.weight as f32))
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "derived weights changed for {vertex:?}");
            checked += 1;
        }
    }
    assert!(checked > 0, "weights must be checked on derived levels");
}

#[test]
fn split_uv_untextured_rounded_box_reduces_without_open_edges() {
    let mut doc = Document::new(Limits::default()).unwrap();
    let mut hidden = SurfaceLayer::new(
        "hidden",
        RgbaImage::new(1, 1, [255; 4], doc.limits()).unwrap(),
    );
    hidden.visible = false;
    let mut transparent = hidden.clone();
    transparent.id = "transparent".into();
    transparent.visible = true;
    transparent.opacity = 0.;
    apply(
        &mut doc,
        "rounded-box",
        vec![
            Operation::Construction(ConstructionOperation {
                object: "box".into(),
                kind: Construction::Shape {
                    shape: Shape::Box {
                        size: [0.3, 0.1, 0.06],
                        radius: 0.002,
                        segments: 2,
                    },
                    material: 0,
                },
            }),
            Operation::Surface(SurfaceOperation::Layer {
                material: 0,
                channel: SurfaceChannel::BaseColor,
                layer: hidden,
            }),
            Operation::Surface(SurfaceOperation::Layer {
                material: 0,
                channel: SurfaceChannel::Emissive,
                layer: transparent,
            }),
            Operation::Scene(SceneOperation::Lods {
                object: "box".into(),
                levels: vec![LodLevel {
                    target_faces: 4,
                    max_error: 0.027,
                    distance: 40.,
                }],
            }),
        ],
    );
    let source = doc.object("box").unwrap();
    let mut ctx = mesh::Context::default();
    let before = source.triangulate(&mut ctx).unwrap();
    let mut source_uvs = BTreeMap::<_, BTreeSet<_>>::new();
    for corner in source.corners() {
        source_uvs
            .entry(corner.vertex)
            .or_default()
            .insert(corner.uv.map(f64::to_bits));
    }
    assert!(
        source_uvs.values().any(|uvs| uvs.len() > 1),
        "fixture has split UVs"
    );
    assert!(
        source
            .adjacency(&mut ctx)
            .unwrap()
            .edges
            .values()
            .all(|uses| uses.len() == 2),
        "fixture is closed"
    );
    let snapshot = doc.to_bytes(None).unwrap();
    let product = doc.compile(None).unwrap();
    assert!(product.warnings.is_empty(), "{:?}", product.warnings);
    assert_eq!(
        doc.to_bytes(None).unwrap(),
        snapshot,
        "LOD construction leaves source unchanged"
    );
    let derived = product
        .primitives
        .iter()
        .filter(|p| p.object != "box")
        .collect::<Vec<_>>();
    assert_eq!(derived.len(), 1, "a real derived level is delivered");
    let derived = derived[0];
    assert!(
        derived.faces.len() < before.triangles.len(),
        "split UVs cannot lock an untextured part"
    );
    let mut edges = BTreeMap::new();
    for tri in derived.source_vertices.chunks_exact(3) {
        for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            *edges.entry(mesh::EdgeKey::new(a, b)).or_insert(0) += 1;
        }
    }
    assert!(
        edges.values().all(|&uses| uses == 2),
        "reduction must keep a closed surface"
    );
    for (&vertex, &position) in derived.source_vertices.iter().zip(&derived.positions) {
        assert_eq!(
            position,
            source
                .vertex(vertex)
                .expect("kept source ID")
                .position
                .map(|p| p as f32)
        );
    }
}

#[test]
fn textured_charts_keep_source_wedges_on_every_texture_channel() {
    let mut doc = Document::new(Limits::default()).unwrap();
    let mut ctx = mesh::Context::default();
    let positions = (0..3)
        .flat_map(|y| (0..5).map(move |x| [x as f64 * 0.25, y as f64 * 0.25, 0.]))
        .collect::<Vec<_>>();
    let mut operations = Vec::new();
    for (material, channel) in [
        Some(SurfaceChannel::BaseColor),
        Some(SurfaceChannel::MetallicRoughness),
        Some(SurfaceChannel::Normal),
        Some(SurfaceChannel::Occlusion),
        Some(SurfaceChannel::Emissive),
        None,
    ]
    .into_iter()
    .enumerate()
    {
        let material = material as u32;
        let name = format!("chart_{material}");
        let mut polygons = Vec::new();
        for y in 0..2 {
            for x in 0..4 {
                let vertices = vec![
                    y * 5 + x,
                    y * 5 + x + 1,
                    (y + 1) * 5 + x + 1,
                    (y + 1) * 5 + x,
                ];
                let chart = if x < 2 { 0. } else { 2. };
                let uvs = vertices
                    .iter()
                    .map(|&v| [positions[v as usize][0] + chart, positions[v as usize][1]])
                    .collect();
                polygons.push(mesh::Polygon {
                    vertices,
                    uvs,
                    material,
                });
            }
        }
        let mesh = mesh::Mesh::from_polygons(&positions, &polygons, &mut ctx).unwrap();
        operations.push(Operation::SetMaterial {
            material,
            value: Material::default(),
        });
        operations.push(Operation::ImportMesh {
            object: name.clone(),
            source: mesh.to_bytes(&mut ctx).unwrap(),
        });
        if let Some(channel) = channel {
            operations.push(Operation::Surface(SurfaceOperation::Layer {
                material,
                channel,
                layer: SurfaceLayer::new(
                    "chart",
                    RgbaImage::new(2, 2, [128, 128, 255, 255], doc.limits()).unwrap(),
                ),
            }));
        } else {
            operations.push(Operation::TextureSolid {
                material,
                width: 2,
                height: 2,
                color: [128, 128, 255],
            });
        }
        operations.push(Operation::Scene(SceneOperation::Lods {
            object: name,
            levels: vec![LodLevel {
                target_faces: 4,
                max_error: 0.027,
                distance: 40.,
            }],
        }));
    }
    apply(&mut doc, "charts", operations);
    let product = doc.compile(None).unwrap();
    assert!(product.warnings.is_empty(), "{:?}", product.warnings);
    let gltf = makepad_gltf::load_gltf_from_bytes(&product.glb, None).unwrap();
    for material in 0..6 {
        let name = format!("chart_{material}");
        let owner = gltf
            .document
            .nodes_slice()
            .iter()
            .find(|n| n.name.as_deref() == Some(name.as_str()))
            .unwrap();
        let lod = idx(&arr(field(
            field(owner.extensions.as_ref().unwrap(), "MSFT_lod"),
            "ids",
        ))[0]);
        let node = &gltf.document.nodes_slice()[lod];
        assert_ne!(node.mesh, owner.mesh, "textured chart must actually reduce");
        let derived = product
            .primitives
            .iter()
            .find(|p| Some(p.object.as_str()) == node.name.as_deref())
            .unwrap();
        assert_eq!(derived.material, material);
        let source = doc.object(&name).unwrap();
        assert!(derived.faces.len() < source.triangulate(&mut ctx).unwrap().triangles.len());
        let decoded = makepad_gltf::decode_mesh_primitive(&gltf, node.mesh.unwrap(), 0).unwrap();
        let exported = &gltf.document.materials_slice()[decoded.material.unwrap()];
        let samples_texture = match material {
            0 | 5 => exported
                .pbr_metallic_roughness
                .as_ref()
                .unwrap()
                .base_color_texture
                .is_some(),
            1 => exported
                .pbr_metallic_roughness
                .as_ref()
                .unwrap()
                .metallic_roughness_texture
                .is_some(),
            2 => exported.normal_texture.is_some(),
            3 => exported.occlusion_texture.is_some(),
            4 => exported.emissive_texture.is_some(),
            _ => unreachable!(),
        };
        assert!(
            samples_texture,
            "material {material} exports the texture that constrains its seam"
        );
        let uvs = decoded.texcoords0.as_ref().expect("textured UVs");
        assert_eq!(decoded.indices.len(), derived.source_vertices.len());
        let mut seam_uvs = BTreeMap::<_, BTreeSet<_>>::new();
        for (indices, vertices) in decoded
            .indices
            .chunks_exact(3)
            .zip(derived.source_vertices.chunks_exact(3))
        {
            let points = vertices
                .iter()
                .map(|&id| source.vertex(id).expect("source vertex").position)
                .collect::<Vec<_>>();
            let left = points.iter().all(|p| p[0] <= 0.5);
            let right = points.iter().all(|p| p[0] >= 0.5);
            assert!(left || right, "a derived triangle crossed the UV seam");
            let chart = if left { 0. } else { 2. };
            for ((&index, &vertex), point) in indices.iter().zip(vertices).zip(points) {
                let expected = [(point[0] + chart) as f32, point[1] as f32];
                assert!(
                    source
                        .corners()
                        .iter()
                        .any(|c| c.vertex == vertex && c.uv.map(|v| v as f32) == expected),
                    "expected UV belongs to a source wedge"
                );
                assert_eq!(
                    uvs[index as usize].map(f32::to_bits),
                    expected.map(f32::to_bits),
                    "material {material}: output corner must retain its source wedge"
                );
                if point[0] == 0.5 {
                    seam_uvs
                        .entry(vertex)
                        .or_default()
                        .insert(uvs[index as usize].map(f32::to_bits));
                }
            }
        }
        assert!(
            seam_uvs.values().any(|uvs| uvs.len() == 2),
            "both chart wedges survive at the seam"
        );
    }
}

#[test]
fn many_parts_reuse_meshes_without_a_derived_object_cap(){
    let mut d=Document::new(Limits::default()).unwrap();
    apply(&mut d,"parts",(0..213).map(|i|Operation::Cube{object:format!("part_{i}"),size:[1.;3]}).collect());
    apply(&mut d,"levels",(0..213).map(|i|Operation::Scene(SceneOperation::Lods{object:format!("part_{i}"),levels:vec![
        LodLevel{target_faces:4,max_error:0.,distance:10.},LodLevel{target_faces:0,max_error:0.,distance:30.},
    ]})).collect());
    let product=d.compile(None).unwrap();assert!(product.warnings.is_empty(),"unreducible cubes keep their mesh quietly");
    let g=makepad_gltf::parse_glb_bytes(&product.glb).unwrap();
    assert_eq!(g.document.meshes_slice().len(),214,"repeats and empty levels allocate no meshes");
    assert_eq!(g.document.nodes_slice().iter().filter(|n|n.extensions.as_ref().is_some_and(|e|e.key("MSFT_lod").is_some())).count(),213);
}
#[test]
fn world_space_error_does_not_shrink_with_node_scale(){
    let mut d=Document::new(Limits::default()).unwrap();
    let sphere=Operation::sphere("scaled",0.01,8,4,true,d.limits()).unwrap();
    apply(&mut d,"scaled",vec![sphere,Operation::Scene(SceneOperation::Node{object:"scaled".into(),node:SceneNode{transform:Transform{scale:[100.;3],..Default::default()},..Default::default()}}),
        Operation::Scene(SceneOperation::Lods{object:"scaled".into(),levels:vec![LodLevel{target_faces:4,max_error:0.02,distance:40.}]})]);
    let product=d.compile(None).unwrap();assert!(product.warnings.is_empty(),"world-space edges exceed the error bound: the level keeps its mesh quietly");
    let g=makepad_gltf::parse_glb_bytes(&product.glb).unwrap();let nodes=g.document.nodes_slice();
    let owner=nodes.iter().find(|n|n.extensions.as_ref().is_some_and(|e|e.key("MSFT_lod").is_some())).unwrap();
    let level=idx(&arr(field(field(owner.extensions.as_ref().unwrap(),"MSFT_lod"),"ids"))[0]);
    assert_eq!(nodes[level].mesh,owner.mesh,"local small edges must not permit a large world-space pop");
}

#[test]
fn hard_edges_keep_each_face_normal_on_derived_levels() {
    let mut doc = Document::new(Limits::default()).unwrap();
    let ball = Operation::sphere("ball", 1.0, 24, 16, false, doc.limits()).unwrap();
    apply(&mut doc, "faceted", vec![ball]);
    let mut ctx = mesh::Context::default();
    let before = doc.object("ball").unwrap().triangulate(&mut ctx).unwrap();
    let mut normals_at = BTreeMap::<_, BTreeSet<_>>::new();
    let mut originals = BTreeSet::new();
    for tri in &before.triangles {
        let mut ids = tri.indices.map(|i| before.vertices[i as usize].source_vertex);
        for i in tri.indices {
            let v = &before.vertices[i as usize];
            normals_at.entry(v.source_vertex).or_default().insert(v.normal.map(f64::to_bits));
        }
        ids.sort();
        originals.insert(ids);
    }
    assert!(normals_at.values().all(|n| n.len() > 1), "fixture is faceted: every vertex is on hard edges");
    let target_faces = before.triangles.len() * 9 / 10;
    apply(&mut doc, "lod", vec![Operation::Scene(SceneOperation::Lods {
        object: "ball".into(),
        levels: vec![LodLevel { target_faces, max_error: 0.05, distance: 20. }],
    })]);
    let product = doc.compile(None).unwrap();
    assert!(product.warnings.is_empty(), "{:?}", product.warnings);
    let derived = product.primitives.iter().find(|p| p.object != "ball").expect("a derived level");
    assert!(derived.source_vertices.len() / 3 < before.triangles.len(), "the level reduces");
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let mut kept = 0;
    for ((ids, p), n) in derived.source_vertices.chunks_exact(3).zip(derived.positions.chunks_exact(3)).zip(derived.normals.chunks_exact(3)) {
        let mut key = [ids[0], ids[1], ids[2]];
        key.sort();
        if !originals.contains(&key) { continue; }
        kept += 1;
        let (u, v) = (sub(p[1], p[0]), sub(p[2], p[0]));
        let face = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let len = face.iter().map(|x| x * x).sum::<f32>().sqrt();
        for corner in n {
            let along = (corner[0] * face[0] + corner[1] * face[1] + corner[2] * face[2]) / len;
            assert!(along > 0.999, "an unchanged faceted triangle keeps its own face normal on every corner ({along})");
        }
    }
    assert!(kept * 2 > derived.source_vertices.len() / 3, "most triangles are unchanged originals ({kept})");
}
