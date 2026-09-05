//! CPU-only authoring → existing writer → real renderer parser/pose tests.
use makepad_csg::{evaluate_program, mesh_document, CsgBudgets, MeshedModel};
use makepad_render::skin::{SkinnedModel, SKIN_VERTEX_FLOATS};

const SPRIGLET: &str = include_str!("../examples/spriglet.splash");

fn build(source: &str) -> (MeshedModel, Vec<u8>) {
    let doc = evaluate_program(source, CsgBudgets::default()).unwrap();
    let model = mesh_document(doc, |partial| assert!(partial.model.rig.is_none())).unwrap();
    let glb = model.rig.as_ref().unwrap().to_glb(&model).unwrap();
    (model, glb)
}

fn packed(model: &SkinnedModel, clip: Option<usize>, time: f32) -> Vec<f32> {
    let mut pose = model.rest_pose();
    if let Some(clip) = clip { model.sample_clip(clip, time, &mut pose); }
    let mut palette = Vec::new();
    model.palette(&pose, &mut palette);
    let mut packed = Vec::new();
    model.skin_to_packed(&palette, &mut packed);
    packed
}

fn close(got: &[f32], expected: [f32; 3]) {
    for i in 0..3 { assert!((got[i] - expected[i]).abs() < 1e-5, "got {got:?}, expected {expected:?}"); }
}

#[test]
fn original_character_rest_bend_rigid_shell_and_colors_roundtrip() {
    let (authored, glb) = build(SPRIGLET);
    let rig = authored.rig.as_ref().unwrap();
    let model = SkinnedModel::parse_glb(&glb).unwrap();
    assert_eq!(model.joint_count(), rig.joint_count());
    assert_eq!(model.joint_count(), 3);
    assert_eq!(model.skipped_unskinned, 0);
    assert_eq!(model.clips.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), rig.clip_names().collect::<Vec<_>>());
    assert_eq!(model.gait_clips(), Some((0, 1)));
    assert_eq!(model.node_parent(model.node_index("crown").unwrap()), model.node_index("sway"));
    assert_eq!(model.clips[1].duration, 2.0);

    let rest = packed(&model, None, 0.0);
    let bent = packed(&model, Some(1), 0.5); // 45 degrees around sway.
    let influences: Vec<_> = rig.influences().collect();
    let mut vi = 0;
    let mut smooth = 0;
    let mut moved = 0;
    for part in &authored.parts {
        for p in &part.mesh.vertices {
            let p = [p.x as f32, p.y as f32, p.z as f32];
            close(&rest[vi*SKIN_VERTEX_FLOATS..][..3], p);
            let (joints, weights) = influences[vi];
            assert!(weights.iter().all(|w| w.is_finite() && *w >= 0.0));
            assert!((weights.iter().sum::<f32>() - 1.0).abs() < 1e-6);
            let expected = if part.name == "foot" { p } else {
                let s = std::f32::consts::FRAC_1_SQRT_2;
                let rotated = [p[0]*s - (p[1]-0.8)*s, 0.8 + p[0]*s + (p[1]-0.8)*s, p[2]];
                let w = if part.name == "stem" {
                    // Selection excludes the crown even where it is closer.
                    assert!(joints.iter().zip(weights).all(|(j, w)| *w == 0.0 || *j == 0 || *j == 2));
                    smooth += usize::from(weights.iter().filter(|w| **w > 0.0).count() == 2);
                    joints.iter().zip(weights).filter(|(j, _)| **j == 2).map(|(_, w)| *w).sum::<f32>()
                } else {
                    assert_eq!(*weights, [1.0, 0.0, 0.0, 0.0]);
                    assert_eq!(joints[0], 1); // Crown shell remains a rigid object.
                    1.0
                };
                std::array::from_fn(|i| p[i]*(1.0-w) + rotated[i]*w)
            };
            let got = &bent[vi*SKIN_VERTEX_FLOATS..][..3];
            close(got, expected);
            moved += usize::from((got[0]-p[0]).abs() > 0.01);
            vi += 1;
        }
    }
    assert!(smooth > 50 && moved > 50);
    assert_eq!(vi, model.vertex_count());

    // The actual character texture loader sees the authored palette, and
    // the renderer retains the UVs needed to select each part's color.
    let png = makepad_render::embedded_base_color_png(&glb).unwrap();
    use makepad_zune_png::{PngDecoder, makepad_zune_core::bytestream::ZCursor};
    let mut decoder = PngDecoder::new(ZCursor::new(png));
    let rgba = decoder.decode_raw().unwrap();
    let width = authored.parts.len().next_power_of_two();
    assert_eq!(decoder.dimensions(), Some((width, 1)));
    let loaded = makepad_gltf::load_gltf_from_bytes(&glb, None).unwrap();
    let primitive = &loaded.document.meshes_slice()[0].primitives[0];
    let uvs = makepad_gltf::read_accessor_f32x2(&loaded, primitive.attributes["TEXCOORD_0"]).unwrap();
    let mut vi = 0;
    for (pi, part) in authored.parts.iter().enumerate() {
        let expected = part.color.map(|v| (v*255.0).round() as u8);
        assert_eq!(&rgba[pi*4..pi*4+4], &expected);
        for _ in &part.mesh.vertices {
            assert_eq!(uvs[vi], [(pi as f32+0.5)/width as f32, 0.5]);
            // UV survives both pose samples in the real packed CPU output.
            assert_eq!(rest[vi*SKIN_VERTEX_FLOATS+4].to_bits(), bent[vi*SKIN_VERTEX_FLOATS+4].to_bits());
            vi += 1;
        }
    }
}

#[test]
fn joint_edit_regenerates_deterministically_and_changes_bending() {
    let (_, original) = build(SPRIGLET);
    assert_eq!(original, build(SPRIGLET).1);
    let edited = SPRIGLET.replace("pos: vec3(0, 0.8, 0)", "pos: vec3(0, 0.65, 0)");
    let (_, changed) = build(&edited);
    assert_ne!(original, changed);
    assert_eq!(changed, build(&edited).1);
    let a = SkinnedModel::parse_glb(&original).unwrap();
    let b = SkinnedModel::parse_glb(&changed).unwrap();
    for (a, b) in packed(&a, None, 0.0).chunks_exact(SKIN_VERTEX_FLOATS).zip(packed(&b, None, 0.0).chunks_exact(SKIN_VERTEX_FLOATS)) {
        close(a, [b[0], b[1], b[2]]);
    }
    assert_ne!(packed(&a, Some(1), 0.5), packed(&b, Some(1), 0.5));
}

const SIMPLE: &str = "csg.part(\"p\", csg.box({size:vec3(1,1,1)}), {})\n";

#[test]
fn selected_top_four_ties_and_exact_overrides_are_deterministic() {
    let joints = (0..6).map(|i| format!("csg.joint(\"j{i}\",{{pos:vec3(0,0,0)}})\n")).collect::<String>();
    let auto = "csg.bind(\"p\",{joints:[\"j5\",\"j4\",\"j3\",\"j2\",\"j1\"]})\n";
    let source = format!("{SIMPLE}{joints}{auto}");
    let (model, _) = build(&source);
    for (j, w) in model.rig.unwrap().influences() {
        assert_eq!(*j, [1,2,3,4]);
        assert_eq!(*w, [0.25;4]);
    }
    for exact in ["csg.bind(\"p\",{rigid:\"j0\"})\n", "csg.bind(\"p\",{weights:[{joint:\"j0\",weight:0.25},{joint:\"j5\",weight:0.75}]})\n"] {
        let before = build(&format!("{SIMPLE}{joints}{exact}{auto}")).1;
        let (model, after) = build(&format!("{source}{exact}"));
        assert_eq!(before, after, "automatic choices must not overwrite authored binding");
        let rig = model.rig.unwrap();
        assert!(rig.influences().all(|(j, _)| j[0] == 0));
        if exact.contains("weights") { assert!(rig.influences().all(|(_, w)| *w == [0.25,0.75,0.0,0.0])); }
    }
}

fn refuses(source: &str, diagnostic: &str) {
    let result = evaluate_program(source, CsgBudgets::default());
    let error = result.expect_err("invalid rig must fail before any preview/GLB/publication").to_string();
    assert!(error.contains(diagnostic), "expected {diagnostic:?}: {error}");
}

#[test]
fn malformed_rigs_fail_closed_before_any_preview() {
    let root = "csg.joint(\"r\",{pos:vec3(0,0,0)})\ncsg.bind(\"p\",{rigid:\"r\"})\n";
    for (extra, diagnostic) in [
        ("csg.joint(\"r\",{pos:vec3(1,0,0)})", "duplicate joint"),
        ("csg.joint(\"a\",{pos:vec3(0,0,0),parent:\"absent\"})", "missing joint"),
        ("csg.joint(\"a\",{pos:vec3(0,0,0),parent:\"b\"})\ncsg.joint(\"b\",{pos:vec3(0,0,0),parent:\"a\"})", "joint cycle a -> b -> a"),
        ("csg.joint(\"nan\",{pos:vec3(0/0,0,0)})", "finite"),
        ("csg.bind(\"p\",{rigid:\"absent\"})", "missing joint"),
        ("csg.bind(\"absent\",{rigid:\"r\"})", "missing part"),
        ("csg.bind(\"p\",{joints:[\"r\",\"r\"]})", "duplicate selected"),
        ("csg.bind(\"p\",{weights:[{joint:\"r\",weight:-1}]})", "nonnegative"),
        ("csg.bind(\"p\",{weights:[{joint:\"r\",weight:0/0}]})", "nonnegative"),
        ("csg.bind(\"p\",{weights:[{joint:\"r\",weight:0.5}]})", "sum to one"),
        ("csg.bind(\"p\",{joints:[\"r\"],radius:0})", "radius"),
        ("csg.anim(\"p\",{kind:\"swing\"})", "parent/pivot/anim"),
        ("csg.clip(\"walk\",[{joint:\"missing\",axis:\"z\",keys:[vec2(0,0),vec2(1,30)]}])", "missing joint"),
        ("csg.clip(\"walk\",[{joint:\"r\",axis:\"z\",keys:[vec2(0,0),vec2(0,30)]}])", "increasing times"),
        ("csg.clip(\"walk\",[{joint:\"r\",axis:\"z\",keys:[vec2(0,0),vec2(61,30)]}])", "increasing times"),
        ("csg.clip(\"walk\",[{joint:\"r\",axis:\"z\",keys:[vec2(0,0),vec2(1,1/0)]}])", "finite keys"),
        ("csg.clip(\"walk\",[])", "channels"),
        ("csg.joint(\"a\",{pos:vec3(0,0,0),parnt:\"r\"})", "unknown option"),
    ] { refuses(&format!("{SIMPLE}{root}{extra}"), diagnostic); }
    refuses(&format!("{SIMPLE}csg.joint(\"r\",{{pos:vec3(0,0,0)}})"), "needs csg.bind");
    refuses(&format!("{SIMPLE}csg.bind(\"p\",{{rigid:\"r\"}})"), "requires csg.joint");
    let clip = "csg.clip(\"walk\",[{joint:\"r\",axis:\"z\",keys:[vec2(0,0),vec2(1,30)]}])\n";
    refuses(&format!("{SIMPLE}{root}{clip}{clip}"), "duplicate clip");
}

#[test]
fn rig_budgets_and_post_mesh_failure_never_return_a_final_skin() {
    let source = format!("{SIMPLE}for i in 0..65 {{csg.joint(\"j\"+i,{{pos:vec3(0,0,0)}})}}");
    refuses(&source, "maximum 64 joints");
    let source = format!("{SIMPLE}csg.joint(\"r\",{{pos:vec3(0,0,0)}})\ncsg.bind(\"p\",{{rigid:\"r\"}})\nfor i in 0..17 {{csg.clip(\"c\"+i,[{{joint:\"r\",axis:\"x\",keys:[vec2(0,0),vec2(1,0)]}}])}}");
    refuses(&source, "maximum 16 clips");
    let keys = (0..129).map(|i| format!("vec2({},0)", i as f32 / 10.0)).collect::<Vec<_>>();
    let root = format!("{SIMPLE}csg.joint(\"r\",{{pos:vec3(0,0,0)}})\ncsg.bind(\"p\",{{rigid:\"r\"}})\n");
    refuses(&format!("{root}csg.clip(\"c\",[{{joint:\"r\",axis:\"x\",keys:[{}]}}])", keys.join(",")), "2..128");
    let channels = (0..33).map(|i| format!("{{joint:\"j{i}\",axis:\"z\",keys:keys}}")).collect::<Vec<_>>().join(",");
    let source = format!("{SIMPLE}for i in 0..33 {{csg.joint(\"j\"+i,{{pos:vec3(0,0,0)}})}}\ncsg.bind(\"p\",{{rigid:\"j0\"}})\nlet keys=[{}]\ncsg.clip(\"c\",[{channels}])", keys[..128].join(","));
    refuses(&source, "maximum 4096 total keys");
    let entries = (0..5).map(|i| format!("{{joint:\"j{i}\",weight:0.2}}")).collect::<Vec<_>>().join(",");
    refuses(&format!("{root}csg.bind(\"p\",{{weights:[{entries}]}})"), "1..4 influences");
    let mut budgets = CsgBudgets::default();
    budgets.max_triangles = 1;
    let doc = evaluate_program(SPRIGLET, budgets).unwrap();
    let mut previews = 0;
    assert!(mesh_document(doc, |_| previews += 1).is_err());
    assert_eq!(previews, 0);
    let source = SPRIGLET.replace("vec3(0.16, 0.7, 0.16)", "vec3(50, 50, 50)").replace("vec3(0, 0.8, 0))", "vec3(0, 49, 0))");
    let doc = evaluate_program(&source, CsgBudgets::default()).unwrap();
    assert!(mesh_document(doc, |p| assert!(p.model.rig.is_none())).unwrap_err().to_string().contains("mesh must be finite"));
}

#[test]
fn cancellation_after_the_last_preview_still_refuses_final_binding() {
    use makepad_csg_math::thread_pool::{with_cancel, CancelToken};
    let token = CancelToken::new();
    let mut previews = 0;
    let result = with_cancel(&token, || {
        let doc = evaluate_program(SPRIGLET, CsgBudgets::default()).unwrap();
        mesh_document(doc, |partial| {
            previews += 1;
            assert!(partial.model.rig.is_none());
            if partial.completed == partial.total { token.cancel(); }
        })
    });
    assert_eq!(previews, 5);
    assert_eq!(result.unwrap_err(), makepad_csg::CsgError::Cancelled);
}

#[test]
fn maximum_joint_selection_and_near_triangle_budget_bind_on_cpu() {
    // Deliberately exercise the worst allowed candidate count at useful
    // model density, not just tiny boxes. This remains below the ordinary
    // source, part and triangle limits; the shared deadline remains active.
    let mut source = String::new();
    for i in 0..64 {
        source.push_str(&format!("csg.joint(\"j{i}\",{{pos:vec3({},0,0)}})\n", i as f64 * 0.01));
    }
    let selected = (0..64).map(|i| format!("\"j{i}\"")).collect::<Vec<_>>().join(",");
    source.push_str(&format!("let selected=[{selected}]\nlet shape=csg.sphere({{r:0.25,seg:64}})\n"));
    for i in 0..32 {
        source.push_str(&format!("csg.part(\"p{i}\",shape,{{}})\ncsg.bind(\"p{i}\",{{joints:selected}})\n"));
    }
    let started = std::time::Instant::now();
    let (authored, glb) = build(&source);
    let elapsed = started.elapsed();
    assert!((120_000..=150_000).contains(&authored.triangles));
    let rig = authored.rig.as_ref().unwrap();
    assert_eq!(rig.joint_count(), 64);
    let mut vertices = 0;
    for (joints, weights) in rig.influences() {
        assert!(joints.iter().all(|j| *j < 64));
        assert!(weights.iter().all(|w| w.is_finite() && *w > 0.0));
        assert!((weights.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        vertices += 1;
    }
    assert!(vertices > 60_000);
    let parsed = SkinnedModel::parse_glb(&glb).unwrap();
    assert_eq!((parsed.joint_count(), parsed.vertex_count()), (64, vertices));
    let rest = packed(&parsed, None, 0.0);
    let expected = authored.parts.iter().flat_map(|part| &part.mesh.vertices);
    for (actual, expected) in rest.chunks_exact(SKIN_VERTEX_FLOATS).zip(expected) {
        close(actual, [expected.x as f32, expected.y as f32, expected.z as f32]);
    }
    eprintln!("64-joint rig: {} triangles, {vertices} vertices, build+bind+GLB={elapsed:?}", authored.triangles);
}
