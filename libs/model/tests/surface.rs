use makepad_model::*;
use std::collections::BTreeMap;
fn apply(d: &mut Document, name: &str, ops: Vec<SurfaceOperation>) {
    d.apply(
        Transaction {
            request_id: name.into(),
            expected: d.head(),
            operations: ops.into_iter().map(Operation::Surface).collect(),
        },
        None,
    )
    .unwrap();
}
fn document() -> Document {
    let mut d = Document::new(Limits::default()).unwrap();
    d.apply(
        Transaction {
            request_id: "mesh".into(),
            expected: d.head(),
            operations: vec![
                Operation::Plane {
                    object: "low".into(),
                    size: [2., 2.],
                },
                Operation::Plane {
                    object: "high".into(),
                    size: [2., 2.],
                },
            ],
        },
        None,
    )
    .unwrap();
    d
}
fn layer(id: &str, ch: SurfaceChannel, color: [u8; 4], limits: &Limits) -> SurfaceOperation {
    SurfaceOperation::Layer {
        material: 0,
        channel: ch,
        layer: SurfaceLayer::new(id, RgbaImage::new(8, 8, color, limits).unwrap()),
    }
}
fn read_png(loaded: &makepad_gltf::LoadedGltf, texture: usize) -> RgbaImage {
    let index = loaded.document.textures_slice()[texture].source.unwrap();
    RgbaImage::from_png(
        &makepad_gltf::load_image_bytes(loaded, index).unwrap(),
        &Limits::default(),
    )
    .unwrap()
}
#[test]
fn factor_only_conversion_preserves_legacy_pixels_and_explicit_layer_removal() {
    let mut d = document();
    d.apply(Transaction {
        request_id: "legacy-paint".into(), expected: d.head(),
        operations: vec![Operation::TextureSolid { material: 0, width: 4, height: 4, color: [90, 70, 45] }],
    }, None).unwrap();
    let original = d.materials()[&0].base_color_png.clone();
    let factors = SurfaceMaterial { roughness: 0.3, ..Default::default() };
    apply(&mut d, "surface-factors", vec![SurfaceOperation::Material { material: 0, value: factors.clone() }]);
    assert_eq!(d.surface().materials[&0].channels[&SurfaceChannel::BaseColor][0].id, "legacy");
    let loaded = makepad_gltf::load_gltf_from_bytes(&d.compile(None).unwrap().glb, None).unwrap();
    for primitive in loaded.document.meshes_slice().iter().flat_map(|mesh| &mesh.primitives) {
        let material = &loaded.document.materials_slice()[primitive.material.unwrap()];
        let pbr = material.pbr_metallic_roughness.as_ref().unwrap();
        let image = read_png(&loaded, pbr.base_color_texture.as_ref().unwrap().index);
        assert!(image.pixels.chunks_exact(4).all(|pixel| pixel == [90, 70, 45, 255]));
    }
    assert_eq!(d.materials()[&0].base_color_png, original);
    apply(&mut d, "remove-paint", vec![SurfaceOperation::RemoveLayer {
        material: 0, channel: SurfaceChannel::BaseColor, layer: "legacy".into(),
    }, SurfaceOperation::Material { material: 0, value: factors }]);
    assert!(d.surface().materials[&0].channels[&SurfaceChannel::BaseColor].is_empty(),
        "factor edits must not resurrect an explicitly removed legacy layer");
}

#[test]
fn derivation_seeds_a_legacy_bitmap_without_overwriting_its_pixels() {
    let mut d = document();
    let limits = d.limits().clone();
    let mut source = RgbaImage::new(8, 8, [0, 0, 0, 255], &limits).unwrap();
    for (index, pixel) in source.pixels.chunks_exact_mut(4).enumerate() {
        pixel[..3].fill((index / 8 * 30) as u8);
    }
    d.apply(Transaction {
        request_id: "legacy-ramp".into(), expected: d.head(),
        operations: vec![Operation::SetMaterial { material: 0, value: Material {
            base_color_png: source.to_png(&limits).unwrap(), ..Default::default()
        }}],
    }, None).unwrap();
    let recipe = SurfaceDerivation { source_channel: SurfaceChannel::BaseColor, source_layer: None,
        strength: 0.05, roughness_min: 0.2, roughness_max: 0.9, metallic: 0., wrap: false };
    apply(&mut d, "derive-from-legacy", vec![SurfaceOperation::Derive {
        material: 0, channel: SurfaceChannel::Normal, layer: "relief".into(), recipe,
    }]);
    let material = &d.surface().materials[&0];
    assert_eq!(material.channels[&SurfaceChannel::BaseColor][0].image, source);
    assert!(material.channels[&SurfaceChannel::Normal][0].image.pixels[(3 * 8 + 3) * 4 + 1] < 128);
    let bytes = d.to_snapshot_bytes(None).unwrap();
    let reopened = Document::from_bytes(&bytes, limits, None).unwrap();
    assert_eq!(reopened.surface(), d.surface());
    let loaded = makepad_gltf::load_gltf_from_bytes(&reopened.compile(None).unwrap().glb, None).unwrap();
    let primitive = &loaded.document.meshes_slice()[0].primitives[0];
    let material = &loaded.document.materials_slice()[primitive.material.unwrap()];
    let pbr = material.pbr_metallic_roughness.as_ref().unwrap();
    assert_eq!(read_png(&loaded, pbr.base_color_texture.as_ref().unwrap().index), source);
    assert!(read_png(&loaded, material.normal_texture.as_ref().unwrap().index).pixels[(3 * 8 + 3) * 4 + 1] < 128);
}

#[test]
fn rgba_roundtrip_linear_mips_normal_filter_and_coverage_dilation() {
    let l = Limits::default();
    let mut image = RgbaImage::new(3, 1, [0; 4], &l).unwrap();
    image.pixels = vec![255, 0, 0, 255, 0, 0, 255, 0, 255, 0, 0, 255];
    assert_eq!(
        RgbaImage::from_png(&image.to_png(&l).unwrap(), &l).unwrap(),
        image
    );
    let mip = image
        .mip_chain(ImageEncoding::Srgb, &l, &mut mesh::Context::default())
        .unwrap();
    assert_eq!(mip[0].pixels, vec![255, 0, 0, 170]);
    let mut image = RgbaImage::new(2, 1, [255; 4], &l).unwrap();
    image.pixels[..4].copy_from_slice(&[0, 0, 0, 255]);
    let mip = image
        .mip_chain(ImageEncoding::Srgb, &l, &mut mesh::Context::default())
        .unwrap();
    assert!((mip[0].pixels[0] as i32 - 188).abs() <= 1);
    let mut image = RgbaImage::new(2, 1, [128, 128, 255, 255], &l).unwrap();
    image.pixels[..4].copy_from_slice(&[255, 128, 128, 255]);
    let mip = image
        .mip_chain(ImageEncoding::Normal, &l, &mut mesh::Context::default())
        .unwrap();
    let n = &mip[0].pixels;
    assert!(n[0] > 210 && n[2] > 210);
    let mut image = RgbaImage::new(5, 1, [0; 4], &l).unwrap();
    image.pixels[..4].copy_from_slice(&[20, 40, 60, 255]);
    image
        .dilate(4, true, &l, &mut mesh::Context::default())
        .unwrap();
    assert_eq!(&image.pixels[16..], [20, 40, 60, 0]);
}
#[test]
fn layers_masks_pattern_stroke_reorder_and_source_history_roundtrip() {
    let mut d = document();
    let limits = d.limits().clone();
    apply(
        &mut d,
        "layers",
        vec![
            layer("base", SurfaceChannel::BaseColor, [0, 0, 255, 255], &limits),
            layer(
                "paint",
                SurfaceChannel::BaseColor,
                [255, 0, 0, 255],
                &limits,
            ),
        ],
    );
    apply(
        &mut d,
        "mask",
        vec![SurfaceOperation::Mask {
            material: 0,
            channel: SurfaceChannel::BaseColor,
            layer: "paint".into(),
            mask: Some(vec![128; 64]),
        }],
    );
    let mixed = d.surface().materials[&0]
        .flatten(
            SurfaceChannel::BaseColor,
            d.limits(),
            &mut mesh::Context::default(),
        )
        .unwrap()
        .unwrap();
    assert!((mixed.pixels[0] as i32 - 188).abs() < 2 && mixed.pixels[2] > 180);
    apply(
        &mut d,
        "pattern",
        vec![SurfaceOperation::Pattern {
            material: 0,
            channel: SurfaceChannel::BaseColor,
            layer: "paint".into(),
            pattern: SurfacePattern {
                kind: PatternKind::Checker,
                color_a: [1., 0., 0., 1.],
                color_b: [0., 1., 0., 1.],
                scale: [4., 4.],
                seed: 19,
            },
        }],
    );
    apply(
        &mut d,
        "stroke",
        vec![
            SurfaceOperation::Stroke {
                material: 0,
                channel: SurfaceChannel::BaseColor,
                layer: "paint".into(),
                points: vec![[0., 0.5], [1., 0.5]],
                radius: 0.15,
                hardness: 1.,
                opacity: 1.,
                color: [1.; 4],
                mask: false,
            },
            SurfaceOperation::Mips {
                material: 0,
                channel: SurfaceChannel::BaseColor,
                layer: "paint".into(),
            },
        ],
    );
    let l = &d.surface().materials[&0].channels[&SurfaceChannel::BaseColor][1];
    assert_eq!(&l.image.pixels[(3 * 8 + 4) * 4..][..4], [255; 4]);
    assert!(l.pattern.is_none());
    assert_eq!(l.mips.last().unwrap().width, 1);
    let before = d.to_bytes(None).unwrap();
    let head = d.head();
    apply(
        &mut d,
        "move",
        vec![SurfaceOperation::MoveLayer {
            material: 0,
            channel: SurfaceChannel::BaseColor,
            layer: "base".into(),
            index: 1,
        }],
    );
    d.undo(d.head(), None).unwrap();
    assert_eq!(d.head().content, head.content);
    d.redo(d.head(), None).unwrap();
    let restored = Document::from_bytes(&before, Limits::default(), None).unwrap();
    assert_eq!(restored.to_bytes(None).unwrap(), before);
    assert_eq!(
        restored.surface().materials,
        d.surface()
            .materials
            .clone()
            .into_iter()
            .map(|(id, mut m)| {
                m.channels
                    .get_mut(&SurfaceChannel::BaseColor)
                    .unwrap()
                    .reverse();
                (id, m)
            })
            .collect()
    );
}
#[test]
fn pbr_all_channels_factors_mips_vertex_colors_and_tangents_survive_export() {
    let mut d = document();
    let ids = d
        .object("low")
        .unwrap()
        .vertices()
        .iter()
        .map(|v| v.id)
        .collect();
    let m = SurfaceMaterial {
        base_color: [0.4, 0.6, 0.8, 0.5],
        metallic: 0.7,
        roughness: 0.3,
        normal_scale: 0.8,
        occlusion_strength: 0.6,
        emissive: [0.2, 0.4, 0.6],
        emissive_strength: 12.,
        alpha: SurfaceAlpha::Mask,
        alpha_cutoff: 0.2,
        double_sided: true,
        ..Default::default()
    };
    let l = d.limits().clone();
    apply(
        &mut d,
        "pbr",
        vec![
            SurfaceOperation::Material {
                material: 0,
                value: m,
            },
            layer("color", SurfaceChannel::BaseColor, [200, 120, 40, 100], &l),
            layer(
                "mr",
                SurfaceChannel::MetallicRoughness,
                [0, 80, 190, 255],
                &l,
            ),
            layer("normal", SurfaceChannel::Normal, [128, 128, 255, 255], &l),
            layer("ao", SurfaceChannel::Occlusion, [100, 100, 100, 255], &l),
            layer("emit", SurfaceChannel::Emissive, [40, 80, 120, 255], &l),
            SurfaceOperation::VertexPaint {
                object: "low".into(),
                vertices: ids,
                color: [0.1, 0.2, 0.3, 0.4],
                opacity: 1.,
            },
        ],
    );
    let source = d.to_bytes(None).unwrap();
    let compiled = d.compile(None).unwrap();
    assert_eq!(source, d.to_bytes(None).unwrap());
    let loaded = makepad_gltf::load_gltf_from_bytes(&compiled.glb, None).unwrap();
    for primitive in &loaded.document.meshes_slice()[0].primitives {
        let m = &loaded.document.materials_slice()[primitive.material.unwrap()];
        let p = m.pbr_metallic_roughness.as_ref().unwrap();
        assert_eq!(p.base_color_factor, Some([0.4, 0.6, 0.8, 0.5]));
        assert_eq!(p.metallic_factor, Some(0.7));
        assert_eq!(p.roughness_factor, Some(0.3));
        assert_eq!(m.alpha_mode.as_deref(), Some("MASK"));
        assert_eq!(m.double_sided, Some(true));
        assert_eq!(m.alpha_cutoff, Some(0.2));
        assert_eq!(
            read_png(&loaded, p.base_color_texture.as_ref().unwrap().index).pixels[3],
            100
        );
        assert_eq!(
            read_png(
                &loaded,
                p.metallic_roughness_texture.as_ref().unwrap().index
            )
            .pixels[1],
            80
        );
        assert_eq!(
            read_png(&loaded, m.occlusion_texture.as_ref().unwrap().index).pixels[0],
            100
        );
        assert_eq!(
            read_png(&loaded, m.emissive_texture.as_ref().unwrap().index).pixels[2],
            120
        );
        assert!(m.normal_texture.is_some());
        assert!(format!("{:?}", m.extensions).contains("emissiveStrength"));
        assert!(format!("{:?}", m.extras).contains("makepadMips"));
        assert!(primitive.attributes.contains_key("TANGENT"));
        assert!(primitive.attributes.contains_key("COLOR_0"));
    }
    for image in loaded.document.images_slice() {
        assert!(image.uri.is_none());
        assert!(image.buffer_view.is_some());
    }
    let reopened = Document::from_bytes(&source, Limits::default(), None).unwrap();
    assert_eq!(reopened.compile(None).unwrap().glb, compiled.glb);
}
#[test]
fn cancellation_and_budget_failures_leave_complete_surface_unchanged() {
    let l = Limits::default();
    let mut state = SurfaceState::default();
    let legacy = BTreeMap::from([(0, Material::default())]);
    let objects = BTreeMap::new();
    state
        .apply(
            &layer("paint", SurfaceChannel::BaseColor, [0, 0, 255, 255], &l),
            &objects,
            &legacy,
            &l,
            &mut mesh::Context::default(),
        )
        .unwrap();
    let before = state.clone();
    let op = SurfaceOperation::Stroke {
        material: 0,
        channel: SurfaceChannel::BaseColor,
        layer: "paint".into(),
        points: vec![[0., 0.], [1., 1.]],
        radius: 1.,
        hardness: 1.,
        opacity: 1.,
        color: [1.; 4],
        mask: false,
    };
    let calls = std::cell::Cell::new(0);
    let cancel = || {
        calls.set(calls.get() + 1);
        calls.get() > 4
    };
    assert!(state
        .apply(
            &op,
            &objects,
            &legacy,
            &l,
            &mut mesh::Context::new(l.mesh.clone(), Some(&cancel))
        )
        .is_err());
    assert_eq!(state, before);
    let mut tiny = l.clone();
    tiny.mesh.max_work = 5;
    assert!(state
        .apply(
            &op,
            &objects,
            &legacy,
            &tiny,
            &mut mesh::Context::new(tiny.mesh.clone(), None)
        )
        .is_err());
    assert_eq!(state, before);
    let bad = SurfaceOperation::Mask {
        material: 0,
        channel: SurfaceChannel::BaseColor,
        layer: "paint".into(),
        mask: Some(vec![0]),
    };
    assert!(state
        .apply(&bad, &objects, &legacy, &l, &mut mesh::Context::default())
        .is_err());
    assert_eq!(state, before);
}
#[test]
fn normal_ao_color_transfer_projected_stroke_and_reconcile() {
    let mut d = document();
    let l = d.limits().clone();
    let mut ops = vec![layer(
        "paint",
        SurfaceChannel::BaseColor,
        [30, 100, 220, 255],
        &l,
    )];
    for (channel, name) in [
        (SurfaceChannel::Normal, "normals"),
        (SurfaceChannel::Occlusion, "occlusion"),
        (SurfaceChannel::BaseColor, "transfer"),
    ] {
        ops.push(SurfaceOperation::Bake {
            source: "high".into(),
            target: "low".into(),
            material: 0,
            channel,
            layer: name.into(),
            width: 8,
            height: 8,
            max_distance: 1.,
            ao_samples: 8,
            ao_distance: 1.,
            dilation: 1,
        });
    }
    apply(&mut d, "bake", ops);
    let materials = &d.surface().materials[&0];
    assert!(materials.channels[&SurfaceChannel::Normal][0]
        .image
        .pixels
        .chunks_exact(4)
        .all(|p| p[2] > 250));
    assert!(materials.channels[&SurfaceChannel::Occlusion][0]
        .image
        .pixels
        .chunks_exact(4)
        .all(|p| p[0] == 255));
    assert_eq!(
        &materials.channels[&SurfaceChannel::BaseColor][1]
            .image
            .pixels[..4],
        [30, 100, 220, 255]
    );
    let mesh = d.object("low").unwrap();
    let normal = mesh
        .triangulate(&mut mesh::Context::default())
        .unwrap()
        .vertices[0]
        .normal;
    let origin = normal.map(|v| v * 2.);
    let direction = normal.map(|v| -v);
    apply(
        &mut d,
        "project",
        vec![SurfaceOperation::ProjectedStroke {
            object: "low".into(),
            material: 0,
            channel: SurfaceChannel::BaseColor,
            layer: "paint".into(),
            origin,
            direction,
            radius: 0.5,
            depth: 3.,
            hardness: 1.,
            opacity: 1.,
            color: [1., 0., 0., 1.],
            mask: false,
        }],
    );
    assert!(
        d.surface().materials[&0].channels[&SurfaceChannel::BaseColor][0]
            .image
            .pixels
            .chunks_exact(4)
            .any(|p| p == [255, 0, 0, 255])
    );
    let vertices = d
        .object("low")
        .unwrap()
        .vertices()
        .iter()
        .map(|v| v.id)
        .collect();
    apply(
        &mut d,
        "vertex",
        vec![SurfaceOperation::VertexPaint {
            object: "low".into(),
            vertices,
            color: [0.2, 0.4, 0.6, 1.],
            opacity: 1.,
        }],
    );
    let before = BTreeMap::from([("low".into(), d.object("low").unwrap().clone())]);
    let mut after = before.clone();
    let mesh = after.get_mut("low").unwrap();
    mesh.subdivide(1, &mut mesh::Context::default()).unwrap();
    let mut state = d.surface().clone();
    state
        .reconcile(&before, &after, &l, &mut mesh::Context::default())
        .unwrap();
    assert!(state.vertex_colors.len() > d.surface().vertex_colors.len());
    assert!(state.vertex_colors.values().all(|c| c
        .iter()
        .zip([0.2, 0.4, 0.6, 1.])
        .all(|(x, y)| (*x - y).abs() < 1e-10)));
    after.clear();
    state
        .reconcile(&before, &after, &l, &mut mesh::Context::default())
        .unwrap();
    assert!(state.vertex_colors.is_empty());
}
#[test]
fn strict_surface_json_unknown_fields_bad_payloads_and_all_operation_codecs() {
    let l = Limits::default();
    for text in [
        r#"{"op":"surface_material","material":0,"metallic":2}"#,
        r#"{"op":"surface_layer","material":0,"channel":"base_color","layer":"x","width":2,"height":2,"rgba_hex":"FF"}"#,
        r#"{"op":"surface_mips","material":0,"channel":"base_color","layer":"x","surprise":1}"#,
        r#"{"op":"surface_unknown"}"#,
    ] {
        let value = json::parse(text.as_bytes()).unwrap();
        assert!(SurfaceOperation::parse(&value, &l).is_err());
    }
    let mut d = document();
    for (i,text) in [r#"{"op":"surface_material","material":0,"metallic":0.5,"alpha":"blend"}"#,r#"{"op":"surface_layer","material":0,"channel":"base_color","layer":"x","width":2,"height":2,"color":[1,0,0,0.5]}"#,r#"{"op":"surface_pattern","material":0,"channel":"base_color","layer":"x","pattern":{"kind":"noise","color_a":[1,0,0,1],"color_b":[0,1,0,1],"seed":"42"}}"#,r#"{"op":"surface_mask","material":0,"channel":"base_color","layer":"x","mask_hex":"ff0080ff"}"#,r#"{"op":"surface_dilate","material":0,"channel":"base_color","layer":"x","iterations":1}"#,r#"{"op":"surface_mips","material":0,"channel":"base_color","layer":"x"}"#].iter().enumerate() {let value=json::parse(text.as_bytes()).unwrap();apply(&mut d,&format!("json{i}"),vec![SurfaceOperation::parse(&value,&l).unwrap().unwrap()]);}
    let bytes = d.to_bytes(None).unwrap();
    assert_eq!(
        Document::from_bytes(&bytes, l, None)
            .unwrap()
            .to_bytes(None)
            .unwrap(),
        bytes
    );
}
#[test]
fn skinned_pbr_keeps_distinct_materials_uvs_skin_and_animation() {
    let mut d = Document::new(Limits::default()).unwrap();
    d.apply(
        Transaction {
            request_id: "body".into(),
            expected: d.head(),
            operations: vec![Operation::Cube {
                object: "body".into(),
                size: [1.; 3],
            }],
        },
        None,
    )
    .unwrap();
    let face = d.object("body").unwrap().faces()[0].id;
    d.apply(
        Transaction {
            request_id: "rig".into(),
            expected: d.head(),
            operations: vec![
                Operation::SetMaterial {
                    material: 1,
                    value: Material::default(),
                },
                Operation::AssignMaterial {
                    object: "body".into(),
                    faces: vec![face],
                    material: 1,
                },
                Operation::SetSkeleton {
                    skeleton: Skeleton {
                        joints: vec![Joint {
                            name: "root".into(),
                            parent: None,
                            translation: [0.; 3],
                        }],
                    },
                },
                Operation::AutoWeights {
                    object: "body".into(),
                },
                Operation::SetClip {
                    clip: AnimationClip {
                        name: "idle".into(),
                        channels: vec![AnimationChannel {
                            joint: 0,
                            path: AnimationPath::Translation,
                            keys: vec![
                                Keyframe {
                                    time: 0.,
                                    value: [0.; 4],
                                },
                                Keyframe {
                                    time: 1.,
                                    value: [0., 0.1, 0., 0.],
                                },
                            ],
                        }],
                    },
                },
            ],
        },
        None,
    )
    .unwrap();
    let l = d.limits().clone();
    let mut other = layer("blue", SurfaceChannel::BaseColor, [0, 0, 255, 255], &l);
    if let SurfaceOperation::Layer { material, .. } = &mut other {
        *material = 1;
    }
    apply(
        &mut d,
        "paint",
        vec![
            layer("red", SurfaceChannel::BaseColor, [255, 0, 0, 255], &l),
            other,
        ],
    );
    let source = d.to_bytes(None).unwrap();
    let out = d.compile(None).unwrap();
    let loaded = makepad_gltf::load_gltf_from_bytes(&out.glb, None).unwrap();
    let mesh = &loaded.document.meshes_slice()[0];
    assert_eq!(mesh.primitives.len(), 2);
    assert_ne!(mesh.primitives[0].material, mesh.primitives[1].material);
    let colors: Vec<_> = mesh
        .primitives
        .iter()
        .map(|p| {
            assert!(p.attributes.contains_key("JOINTS_0"));
            assert!(p.attributes.contains_key("WEIGHTS_0"));
            let mat = &loaded.document.materials_slice()[p.material.unwrap()];
            let tex = mat
                .pbr_metallic_roughness
                .as_ref()
                .unwrap()
                .base_color_texture
                .as_ref()
                .unwrap();
            read_png(&loaded, tex.index).pixels[..4].to_vec()
        })
        .collect();
    assert_eq!(colors, vec![vec![255, 0, 0, 255], vec![0, 0, 255, 255]]);
    assert_eq!(loaded.document.skins.as_ref().unwrap().len(), 1);
    assert_eq!(loaded.document.animations.as_ref().unwrap().len(), 1);
    assert_eq!(
        Document::from_bytes(&source, l, None)
            .unwrap()
            .compile(None)
            .unwrap()
            .glb,
        out.glb
    );
    assert_eq!(d.to_bytes(None).unwrap(), source);
}
#[test]
fn ao_rays_detect_overhang_and_hdr_emissive_transfer_retains_strength() {
    let l = Limits::default();
    let positions = [
        [-1., 0., -1.],
        [-1., 0., 1.],
        [1., 0., 1.],
        [1., 0., -1.],
        [-1., 0.3, -1.],
        [-1., 0.3, 1.],
        [1., 0.3, 1.],
        [1., 0.3, -1.],
    ];
    let uv = vec![[0., 0.], [0., 1.], [1., 1.], [1., 0.]];
    let high = mesh::Mesh::from_polygons(
        &positions,
        &[
            mesh::Polygon {
                vertices: vec![0, 1, 2, 3],
                uvs: uv.clone(),
                material: 0,
            },
            mesh::Polygon {
                vertices: vec![7, 6, 5, 4],
                uvs: uv.clone(),
                material: 0,
            },
        ],
        &mut mesh::Context::default(),
    )
    .unwrap();
    let low = mesh::Mesh::from_polygons(
        &positions[..4],
        &[mesh::Polygon {
            vertices: vec![0, 1, 2, 3],
            uvs: uv,
            material: 0,
        }],
        &mut mesh::Context::default(),
    )
    .unwrap();
    let objects = BTreeMap::from([("low".into(), low), ("high".into(), high)]);
    let legacy = BTreeMap::from([(0, Material::default())]);
    let mut s = SurfaceState::default();
    s.materials.insert(
        0,
        SurfaceMaterial {
            emissive: [0.25, 0.5, 1.],
            emissive_strength: 30.,
            ..Default::default()
        },
    );
    for (channel, name) in [
        (SurfaceChannel::Occlusion, "ao"),
        (SurfaceChannel::Emissive, "emission"),
    ] {
        s.apply(
            &SurfaceOperation::Bake {
                source: "high".into(),
                target: "low".into(),
                material: 0,
                channel,
                layer: name.into(),
                width: 8,
                height: 8,
                max_distance: 0.1,
                ao_samples: 32,
                ao_distance: 2.,
                dilation: 0,
            },
            &objects,
            &legacy,
            &l,
            &mut mesh::Context::default(),
        )
        .unwrap();
    }
    let ao = &s.materials[&0].channels[&SurfaceChannel::Occlusion][0].image;
    assert!(ao.pixels.chunks_exact(4).any(|p| p[0] < 64));
    let mat = &s.materials[&0];
    assert_eq!(mat.emissive_strength, 30.);
    let emit = mat
        .flatten(SurfaceChannel::Emissive, &l, &mut mesh::Context::default())
        .unwrap()
        .unwrap();
    assert!(emit.pixels[0] > 130 && emit.pixels[1] > 180 && emit.pixels[2] == 255);
}

#[test]
fn bitmap_procedural_stack_derives_pbr_snapshots_and_roundtrips_export() {
    let mut d = document();
    let limits = d.limits().clone();
    let png = RgbaImage::new(16, 16, [180, 140, 90, 255], &limits).unwrap().to_png(&limits).unwrap();
    let hex: String = png.iter().map(|byte| format!("{byte:02x}")).collect();
    let bitmap = json::parse(format!(r#"{{"op":"surface_layer","material":0,"channel":"base_color","layer":"generated-paint","png_hex":"{hex}"}}"#).as_bytes()).unwrap();
    apply(&mut d, "bitmap", vec![SurfaceOperation::parse(&bitmap, &limits).unwrap().unwrap()]);
    for (index, kind) in [PatternKind::Gradient, PatternKind::Perlin, PatternKind::Fbm, PatternKind::Yarn].into_iter().enumerate() {
        let id = format!("detail-{index}");
        let mut layer = SurfaceLayer::new(&id, RgbaImage::new(16, 16, [255; 4], &limits).unwrap());
        layer.blend = SurfaceBlend::Multiply;
        layer.opacity = 0.35;
        apply(&mut d, &format!("pattern-{index}"), vec![
            SurfaceOperation::Layer { material: 0, channel: SurfaceChannel::BaseColor, layer },
            SurfaceOperation::Pattern { material: 0, channel: SurfaceChannel::BaseColor, layer: id,
                pattern: SurfacePattern { kind, color_a: [0.2, 0.2, 0.2, 1.], color_b: [1.; 4], scale: [2., 2.], seed: 918 } },
        ]);
    }
    let original_layers = d.surface().materials[&0].channels[&SurfaceChannel::BaseColor].clone();
    let mut derivations = Vec::new();
    for (channel, layer) in [("normal", "height-normal"), ("metallic_roughness", "height-roughness")] {
        let json = json::parse(format!(r#"{{"op":"surface_derive","material":0,"source_channel":"base_color","channel":"{channel}","layer":"{layer}","strength":0.05,"metallic":0.7}}"#).as_bytes()).unwrap();
        derivations.push(SurfaceOperation::parse(&json, &limits).unwrap().unwrap());
    }
    apply(&mut d, "derive", derivations.clone());
    let m = &d.surface().materials[&0];
    assert_eq!(m.channels[&SurfaceChannel::BaseColor], original_layers);
    assert_eq!(m.metallic, 1.);
    assert_eq!(m.roughness, 1.);
    assert!(m.channels[&SurfaceChannel::Normal][0].derived.is_some());
    let normal = m.flatten(SurfaceChannel::Normal, &limits, &mut mesh::Context::default()).unwrap().unwrap();
    let rough = m.flatten(SurfaceChannel::MetallicRoughness, &limits, &mut mesh::Context::default()).unwrap().unwrap();
    assert!(normal.pixels.chunks_exact(4).any(|p| p[0] != 128 || p[1] != 128));
    assert!(rough.pixels.chunks_exact(4).all(|p| (51..=230).contains(&p[1]) && p[2] == 179));
    assert!(rough.pixels.chunks_exact(4).map(|p| p[1]).any(|r| r != rough.pixels[1]));
    let source = d.to_bytes(None).unwrap();
    let reopened = Document::from_bytes(&source, limits.clone(), None).unwrap();
    assert_eq!(reopened.to_bytes(None).unwrap(), source);
    assert_eq!(reopened.surface(), d.surface());
    let glb = d.compile(None).unwrap().glb;
    assert_eq!(reopened.compile(None).unwrap().glb, glb);
    let loaded = makepad_gltf::load_gltf_from_bytes(&glb, None).unwrap();
    let material = loaded.document.materials_slice().iter().find(|m| m.normal_texture.is_some()).unwrap();
    assert_eq!(read_png(&loaded, material.normal_texture.as_ref().unwrap().index), normal);
    assert_eq!(read_png(&loaded, material.pbr_metallic_roughness.as_ref().unwrap().metallic_roughness_texture.as_ref().unwrap().index), rough);
    let old_snapshot = d.surface().materials[&0].channels[&SurfaceChannel::Normal][0].image.clone();
    apply(&mut d, "change-height", vec![SurfaceOperation::Pattern { material: 0, channel: SurfaceChannel::BaseColor, layer: "detail-3".into(),
        pattern: SurfacePattern { kind: PatternKind::Yarn, color_a: [0.; 4], color_b: [0.; 4], scale: [2., 2.], seed: 11 } }]);
    assert_eq!(d.surface().materials[&0].channels[&SurfaceChannel::Normal][0].image, old_snapshot, "derived output is an explicit snapshot");
    apply(&mut d, "derive-again", derivations);
    assert_ne!(d.surface().materials[&0].channels[&SurfaceChannel::Normal][0].image, old_snapshot);
}

#[test]
fn derive_rejects_invalid_recipes_and_rolls_back_unknown_sources() {
    let limits = Limits::default();
    for extra in ["\"strength\":17", "\"roughness_min\":0.9,\"roughness_max\":0.2", "\"wrap\":\"yes\"", "\"source_layer\":\"\"", "\"unexpected\":1"] {
        let json = json::parse(format!(r#"{{"op":"surface_derive","material":0,"source_channel":"base_color","channel":"normal","layer":"n",{extra}}}"#).as_bytes()).unwrap();
        assert!(SurfaceOperation::parse(&json, &limits).is_err());
    }
    let mut d = document();
    let before = d.to_bytes(None).unwrap();
    let json = json::parse(br#"{"op":"surface_derive","material":0,"source_channel":"base_color","source_layer":"missing","channel":"normal","layer":"n"}"#).unwrap();
    let operation = SurfaceOperation::parse(&json, &limits).unwrap().unwrap();
    assert!(d.apply(Transaction { request_id: "bad-source".into(), expected: d.head(), operations: vec![Operation::Surface(operation)] }, None).is_err());
    assert_eq!(d.to_bytes(None).unwrap(), before);
}
