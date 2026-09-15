use makepad_model::transform::*;
use makepad_model::*;
use std::collections::BTreeMap;

fn apply(doc: &mut Document, request: &str, operations: Vec<Operation>) {
    doc.apply(
        Transaction {
            request_id: request.into(),
            expected: doc.head(),
            operations,
        },
        None,
    )
    .unwrap();
}

fn parsed(text: &str, limits: &Limits) -> Vec<Operation> {
    parse_operations(&json::parse(text.as_bytes()).unwrap(), limits).unwrap()
}

fn primitive(kind: &str, smooth: Option<bool>, segments: u32, rings: u32) -> Document {
    let mut doc = Document::new(Limits::default()).unwrap();
    let style = smooth
        .map(|v| format!(",\"smooth\":{v}"))
        .unwrap_or_default();
    let dimensions = if kind == "sphere" {
        format!(",\"rings\":{rings}")
    } else {
        ",\"height\":1.8".into()
    };
    let text = format!(
        r#"[{{"op":"{kind}","object":"shape","radius":0.7,"segments":{segments}{dimensions}{style}}}]"#
    );
    let ops = parsed(&text, doc.limits());
    apply(&mut doc, "primitive", ops);
    doc
}

fn near(a: [f64; 3], b: [f64; 3], tolerance: f64) {
    assert!(length(sub(a, b)) < tolerance, "{a:?} != {b:?}");
}

fn outward(mesh: &mesh::Mesh) {
    let triangles = mesh.triangulate(&mut mesh::Context::default()).unwrap();
    for triangle in &triangles.triangles {
        let vertices = triangle.indices.map(|i| &triangles.vertices[i as usize]);
        let [a, b, c] = vertices.map(|v| v.position);
        let normal = normalized(cross(sub(b, a), sub(c, a))).unwrap();
        assert!(dot(normal, mul(add(add(a, b), c), 1. / 3.)) > 0.);
        for vertex in vertices {
            assert!(dot(normal, vertex.normal) > 0.);
        }
    }
    assert!(
        mesh.validate(&mut mesh::Context::default())
            .unwrap()
            .is_closed_manifold
    );
}

#[test]
fn sphere_normals_are_analytic_at_poles_and_uv_seams_without_changing_topology() {
    for (segments, rings) in [(3, 2), (12, 6), (31, 15)] {
        let doc = primitive("sphere", None, segments, rings);
        let explicit = primitive("sphere", Some(true), segments, rings);
        let flat = primitive("sphere", Some(false), segments, rings);
        let mesh = doc.object("shape").unwrap();
        let flat = flat.object("shape").unwrap();
        assert_eq!(mesh, explicit.object("shape").unwrap());
        assert_eq!(mesh.vertices(), flat.vertices());
        assert_eq!(mesh.faces(), flat.faces());
        assert_eq!(mesh.corners().len(), flat.corners().len());
        for (corner, old) in mesh.corners().iter().zip(flat.corners()) {
            assert_eq!(
                (corner.id, corner.vertex, corner.uv),
                (old.id, old.vertex, old.uv)
            );
            let position = mesh.vertex(corner.vertex).unwrap().position;
            near(corner.normal.unwrap(), normalized(position).unwrap(), 1e-12);
        }
        let mut seams = 0;
        for vertex in mesh.vertices() {
            let corners = mesh
                .corners()
                .iter()
                .filter(|c| c.vertex == vertex.id)
                .collect::<Vec<_>>();
            if corners.iter().any(|c| c.uv[0] == 0.) && corners.iter().any(|c| c.uv[0] == 1.) {
                seams += 1;
                assert!(corners.iter().all(|c| c.normal == corners[0].normal));
            }
            if vertex.position[0] == 0. && vertex.position[2] == 0. {
                assert_eq!(corners.len(), segments as usize);
                assert!(corners
                    .iter()
                    .all(|c| c.normal == Some([0., vertex.position[1].signum(), 0.])));
            }
        }
        assert_eq!(seams, rings as usize - 1);
        outward(mesh);
    }
}

#[test]
fn cylinder_sides_are_radial_with_seam_continuity_and_hard_flat_caps() {
    for segments in [3, 12, 31] {
        let doc = primitive("cylinder", None, segments, 0);
        let explicit = primitive("cylinder", Some(true), segments, 0);
        let mesh = doc.object("shape").unwrap();
        assert_eq!(mesh, explicit.object("shape").unwrap());
        for face in mesh.faces() {
            let corners = mesh.face_corners(face.id).unwrap();
            let y = mesh.vertex(corners[0].vertex).unwrap().position[1];
            let cap = corners
                .iter()
                .all(|c| mesh.vertex(c.vertex).unwrap().position[1] == y);
            for corner in corners {
                let p = mesh.vertex(corner.vertex).unwrap().position;
                let expected = if cap {
                    [0., y.signum(), 0.]
                } else {
                    normalized([p[0], 0., p[2]]).unwrap()
                };
                near(corner.normal.unwrap(), expected, 1e-12);
            }
        }
        for vertex in mesh.vertices() {
            let corners = mesh
                .corners()
                .iter()
                .filter(|c| c.vertex == vertex.id)
                .collect::<Vec<_>>();
            let wall = corners
                .iter()
                .filter(|c| c.normal.unwrap()[1] == 0.)
                .collect::<Vec<_>>();
            let cap = corners
                .iter()
                .filter(|c| c.normal.unwrap()[1] != 0.)
                .collect::<Vec<_>>();
            assert_eq!(wall.len(), 2);
            assert_eq!(cap.len(), 1);
            near(wall[0].normal.unwrap(), wall[1].normal.unwrap(), 1e-12);
            assert!(dot(wall[0].normal.unwrap(), cap[0].normal.unwrap()).abs() < 1e-12);
        }
        outward(mesh);
    }
}

#[test]
fn faceted_option_matches_legacy_polygon_bytes_and_saved_sources_stay_faceted() {
    for kind in ["sphere", "cylinder"] {
        let doc = primitive(kind, Some(false), 12, 6);
        let mesh = doc.object("shape").unwrap();
        assert!(mesh.corners().iter().all(|c| c.normal.is_none()));
        // Rebuild via the exact pre-smooth primitive representation: polygons
        // with UVs, no explicit normals, and the original vertex/face order.
        let indices = mesh
            .vertices()
            .iter()
            .enumerate()
            .map(|(i, v)| (v.id, i as u32))
            .collect::<BTreeMap<_, _>>();
        let polygons = mesh
            .faces()
            .iter()
            .map(|face| {
                let corners = mesh.face_corners(face.id).unwrap();
                mesh::Polygon {
                    vertices: corners.iter().map(|c| indices[&c.vertex]).collect(),
                    uvs: corners.iter().map(|c| c.uv).collect(),
                    material: face.material,
                }
            })
            .collect::<Vec<_>>();
        let positions = mesh
            .vertices()
            .iter()
            .map(|v| v.position)
            .collect::<Vec<_>>();
        let legacy =
            mesh::Mesh::from_polygons(&positions, &polygons, &mut mesh::Context::default())
                .unwrap();
        assert_eq!(
            mesh.to_bytes(&mut mesh::Context::default()).unwrap(),
            legacy.to_bytes(&mut mesh::Context::default()).unwrap()
        );
        let source = doc.to_bytes(None).unwrap();
        let reopened = Document::from_bytes(&source, Limits::default(), None).unwrap();
        assert_eq!(reopened.to_bytes(None).unwrap(), source);
        assert_eq!(reopened.object("shape").unwrap(), &legacy);
        assert_eq!(
            reopened.compile(None).unwrap().glb,
            doc.compile(None).unwrap().glb
        );
        let triangles = reopened
            .object("shape")
            .unwrap()
            .triangulate(&mut mesh::Context::default())
            .unwrap();
        for triangle in triangles.triangles {
            let [a, b, c] = triangle.indices.map(|i| &triangles.vertices[i as usize]);
            let normal = normalized(cross(
                sub(b.position, a.position),
                sub(c.position, a.position),
            ))
            .unwrap();
            for v in [a, b, c] {
                near(v.normal, normal, 1e-12);
            }
        }
    }
}

#[test]
fn analytic_normals_survive_rotated_nonuniform_and_mirrored_mesh_transforms() {
    for kind in ["sphere", "cylinder"] {
        for scale in [[2.3, 0.6, 1.4], [-2.3, 0.6, 1.4]] {
            let mut doc = primitive(kind, None, 12, 8);
            let before = doc.object("shape").unwrap().clone();
            let transform = Transform {
                translation: [3., -1., 2.],
                rotation: quat_axis_angle([1., 2., 3.], 0.67).unwrap(),
                scale,
            };
            apply(
                &mut doc,
                "transform",
                vec![Operation::Transform {
                    object: "shape".into(),
                    vertices: before.vertices().iter().map(|v| v.id).collect(),
                    matrix: transform.matrix().unwrap(),
                }],
            );
            let after = doc.object("shape").unwrap();
            for corner in after.corners() {
                let old = before.corner(corner.id).unwrap();
                let n = old.normal.unwrap();
                let expected = normalized(quat_rotate(
                    transform.rotation,
                    std::array::from_fn(|i| n[i] / scale[i]),
                ))
                .unwrap();
                near(corner.normal.unwrap(), expected, 1e-12);
                assert_eq!(corner.uv, old.uv);
            }
            let compiled = doc.compile(None).unwrap();
            let loaded = makepad_gltf::load_gltf_from_bytes(&compiled.glb, None).unwrap();
            let decoded = makepad_gltf::decode_mesh_primitive(&loaded, 0, 0).unwrap();
            let normals = decoded.normals.unwrap();
            assert_eq!(normals, compiled.primitives[0].normals);
            for triangle in decoded.indices.chunks_exact(3) {
                let [a, b, c] = std::array::from_fn::<_, 3, _>(|i| {
                    decoded.positions[triangle[i] as usize].map(f64::from)
                });
                let direction = normalized(cross(sub(b, a), sub(c, a))).unwrap();
                for i in triangle {
                    assert!(dot(direction, normals[*i as usize].map(f64::from)) > 0.);
                }
            }
        }
    }
}

#[test]
fn glossy_primitives_export_stored_normals_uvs_and_low_roughness_pbr_without_source_mutation() {
    for kind in ["sphere", "cylinder"] {
        let mut doc = primitive(kind, None, 16, 8);
        let ops = parsed(
            r#"[{"op":"surface_material","material":0,"base_color":[0.5,0.15,0.25,1],"metallic":0,"roughness":0.08}]"#,
            doc.limits(),
        );
        apply(&mut doc, "gloss", ops);
        let source = doc.to_bytes(None).unwrap();
        let head = doc.head();
        let compiled = doc.compile(None).unwrap();
        let loaded = makepad_gltf::load_gltf_from_bytes(&compiled.glb, None).unwrap();
        let primitive = &loaded.document.meshes_slice()[0].primitives[0];
        let material = &loaded.document.materials_slice()[primitive.material.unwrap()];
        let pbr = material.pbr_metallic_roughness.as_ref().unwrap();
        assert_eq!(pbr.roughness_factor, Some(0.08));
        assert_eq!(pbr.metallic_factor, Some(0.));
        assert_eq!(pbr.base_color_factor, Some([0.5, 0.15, 0.25, 1.]));
        let decoded = makepad_gltf::decode_mesh_primitive(&loaded, 0, 0).unwrap();
        let normals = decoded.normals.unwrap();
        assert_eq!(normals, compiled.primitives[0].normals);
        assert!(normals
            .iter()
            .all(|n| (length(n.map(f64::from)) - 1.).abs() < 1e-6));
        let uvs = decoded.texcoords0.unwrap();
        assert!(uvs.iter().any(|uv| uv[0] == 0.));
        assert!(uvs.iter().any(|uv| uv[0] == 1.));
        let tri = doc
            .object("shape")
            .unwrap()
            .triangulate(&mut mesh::Context::default())
            .unwrap();
        let expected = tri
            .triangles
            .iter()
            .flat_map(|t| {
                t.indices
                    .map(|i| tri.vertices[i as usize].normal.map(|v| v as f32))
            })
            .collect::<Vec<_>>();
        assert_eq!(normals, expected);
        assert_eq!(doc.head(), head);
        assert_eq!(doc.to_bytes(None).unwrap(), source);
        let reopened = Document::from_bytes(&source, Limits::default(), None).unwrap();
        assert_eq!(reopened.compile(None).unwrap().glb, compiled.glb);
    }
}

#[test]
fn smooth_is_a_strict_optional_boolean_and_primitive_budgets_still_apply() {
    for (kind, dimension) in [("sphere", "\"rings\":6"), ("cylinder", "\"height\":2")] {
        for value in ["null", "0", "1", "\"true\"", "[]", "{}"] {
            let text = format!(
                r#"[{{"op":"{kind}","object":"shape","radius":1,"segments":12,{dimension},"smooth":{value}}}]"#
            );
            assert!(
                parse_operations(&json::parse(text.as_bytes()).unwrap(), &Limits::default())
                    .is_err(),
                "accepted {text}"
            );
        }
        let text = format!(
            r#"[{{"op":"{kind}","object":"shape","radius":1,"segments":12,{dimension},"smooth":true}}]"#
        );
        let mut limits = Limits::default();
        limits.mesh.max_corners = 8;
        assert!(parse_operations(&json::parse(text.as_bytes()).unwrap(), &limits).is_err());
    }
}
