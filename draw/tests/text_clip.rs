//! PNG regression for retained text and quad clipping. Run with:
//! MAKEPAD=headless RUSTFLAGS='--cfg headless' cargo test --release
//! -p makepad-draw --test text_clip -- --ignored --nocapture
//! MAKEPAD_HEADLESS_OUT_DIR optionally retains the reference and clipped PNGs.

use makepad_draw::makepad_script::{
    shader::{ShaderFnCompiler, ShaderMode, ShaderOutput, ShaderType},
    shader_backend::ShaderBackend,
    trap::NoTrap,
};
use makepad_draw::text::{
    geom::{Point, Rect as TextRect},
    image::Image,
    msdfer::Msdfer,
    rasterizer::{AtlasKind, CompletedMsdfJob, OutlineRasterizationMode},
};
use makepad_draw::*;
use std::{cell::RefCell, path::Path, rc::Rc};

const WIDTH: usize = 320;
const HEIGHT: usize = 240;
const CLIP: [usize; 4] = [96, 72, 224, 168];

#[derive(Clone, Copy, Debug)]
enum GlyphPath {
    Sdf,
    Msdf,
    Slug,
    Emoji,
}

#[derive(Clone, Copy)]
struct Scene {
    glyph_path: GlyphPath,
    translation: Vec2d,
    shift: Vec2d,
    list_clip: Option<[usize; 4]>,
    instance_clip: bool,
}

fn render(scene: Scene, directory: &Path) -> Vec<u8> {
    std::fs::create_dir_all(directory).unwrap();
    std::env::set_var("MAKEPAD_HEADLESS_OUT_DIR", directory);
    // Another explicit renderer gate in the same test binary may disable PNGs.
    std::env::set_var("MAKEPAD_HEADLESS_FRAMES", "on");
    std::env::set_var("MAKEPAD_HEADLESS_DPI", "1");
    let mut state = None;
    let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx, event| {
        if matches!(event, Event::Startup) {
            let (text, quad) = cx.with_vm(|vm| {
                makepad_platform::script::script_mod(vm);
                makepad_draw::script_mod(vm);
                let font = if matches!(scene.glyph_path, GlyphPath::Emoji) {
                    "self:../../widgets/resources/NotoColorEmoji.ttf"
                } else {
                    "self:../../widgets/resources/IBMPlexSans-Text.ttf"
                };
                let source = script! {
                    use mod.draw.*
                    use mod.text.*
                    use mod.res.*
                    DrawText{
                        color: #fff
                        text_style: TextStyle{
                            font_size: 48
                            font_family: FontFamily{
                                regular := FontMember{res: crate_resource(#(font))}
                            }
                        }
                    }
                };
                let value = vm.eval(source);
                if scene.list_clip.is_none() && scene.translation == dvec2(0.0, 0.0) {
                    let io_self = value.as_object().unwrap();
                    for backend in [
                        ShaderBackend::Metal,
                        ShaderBackend::Glsl,
                        ShaderBackend::Hlsl,
                        ShaderBackend::Wgsl,
                        ShaderBackend::Rust,
                    ] {
                        let mut output = ShaderOutput::default();
                        output.backend = backend;
                        output.const_table = false;
                        output.pre_collect_rust_instance_io(vm, io_self);
                        output.pre_collect_shader_io(vm, io_self);
                        let vertex = vm
                            .bx
                            .heap
                            .object_method(io_self, live_id!(vertex).into(), NoTrap)
                            .as_object()
                            .unwrap();
                        output.mode = ShaderMode::Vertex;
                        ShaderFnCompiler::compile_shader_def(
                            vm,
                            &mut output,
                            NoTrap,
                            live_id!(vertex),
                            vertex,
                            ShaderType::IoSelf(io_self),
                            vec![],
                        );
                        assert!(
                            !output.has_errors,
                            "text vertex compilation failed for {backend:?}"
                        );
                    }
                    println!(
                        "{:?} text vertex backends: Metal GL/WebGL D3D11 WGSL headless OK",
                        scene.glyph_path
                    );
                }
                let text = DrawText::script_from_value(vm, value);
                let mut quad = DrawColor::script_new_with_default(vm);
                quad.color = vec4(1.0, 0.0, 0.0, 1.0);
                (text, quad)
            });
            let mut window = WindowHandle::new(cx);
            window.configure_window(
                cx,
                dvec2(WIDTH as f64, HEIGHT as f64),
                dvec2(0.0, 0.0),
                false,
                "Text clip regression".into(),
            );
            let pass = DrawPass::new(cx);
            window.set_pass(cx, &pass);
            pass.set_window_clear_color(cx, vec4(0.0, 0.0, 0.0, 1.0));
            state = Some((
                window,
                pass,
                DrawList2d::new(cx),
                DrawList2d::new(cx),
                text,
                quad,
            ));
            cx.redraw_all();
        }
        if let Event::Draw(event) = event {
            let (_, pass, root, list, text, quad) = state.as_mut().unwrap();
            let mut draw = CxDraw::new(cx, event);
            let mut cx = Cx2d::new(&mut draw);
            cx.begin_pass(pass, None);
            root.begin_always(&mut cx);
            cx.begin_root_turtle(dvec2(WIDTH as f64, HEIGHT as f64), Layout::flow_overlay());
            list.begin_always(&mut cx);
            if scene.instance_clip {
                cx.push_clip_rect(Rect {
                    pos: dvec2(90.0, 50.0),
                    size: dvec2(110.0, 100.0),
                });
            }
            quad.draw_abs(
                &mut cx,
                Rect {
                    pos: dvec2(12.0, 12.0),
                    size: dvec2(296.0, 216.0),
                },
            );
            let run = if matches!(scene.glyph_path, GlyphPath::Emoji) {
                "😀😀😀😀😀"
            } else {
                "HHHHHHHH"
            };
            cx.fonts.borrow_mut().set_outline_rasterization_mode(
                if matches!(scene.glyph_path, GlyphPath::Msdf) {
                    OutlineRasterizationMode::Msdf
                } else {
                    OutlineRasterizationMode::Sdf
                },
            );
            for y in [40.0, 120.0] {
                if matches!(scene.glyph_path, GlyphPath::Slug) {
                    text.draw_abs(&mut cx, dvec2(24.0, y), run);
                    assert_eq!(text.texture_index, 3.0, "must exercise vector glyphs");
                } else {
                    // Explicit raster API also exercises SDF/MSDF on hosts that
                    // normally choose vector outlines at every font size.
                    let layout = text.layout(&mut cx, 0.0, 0.0, None, false, Align::default(), run);
                    let mut glyphs = Vec::new();
                    for row in &layout.rows {
                        for glyph in &row.glyphs {
                            let mut raster = glyph.rasterize(glyph.font_size_in_lpxs).unwrap();
                            if matches!(scene.glyph_path, GlyphPath::Msdf) {
                                let rasterizer = cx.fonts.borrow().rasterizer().clone();
                                let request_size = glyph.font_size_in_lpxs.max(
                                    rasterizer
                                        .borrow()
                                        .msdf_resolution()
                                        .min_request_dpxs_per_em
                                        + 1.0,
                                );
                                glyph.rasterize(request_size).unwrap();
                                // Finish queued MSDF work deterministically before the
                                // single capture, using the production rasterizer.
                                let mut rasterizer = rasterizer.borrow_mut();
                                let mut msdfer = Msdfer::new(rasterizer.msdfer().settings());
                                for job in rasterizer.take_queued_msdf_jobs() {
                                    let mut image = Image::new(job.key.size);
                                    msdfer.outline_to_msdf(
                                        &job.outline,
                                        job.dpxs_per_em,
                                        &mut image.subimage_mut(TextRect::from(job.key.size)),
                                    );
                                    rasterizer.apply_completed_msdf_job(CompletedMsdfJob {
                                        key: job.key,
                                        pixels: image.into_pixels(),
                                        epoch: job.epoch,
                                    });
                                }
                                drop(rasterizer);
                                raster = glyph.rasterize(request_size).unwrap();
                            }
                            let expected = match scene.glyph_path {
                                GlyphPath::Sdf => AtlasKind::Grayscale,
                                GlyphPath::Msdf => AtlasKind::Msdf,
                                GlyphPath::Emoji => AtlasKind::Color,
                                GlyphPath::Slug => unreachable!(),
                            };
                            assert_eq!(raster.atlas_kind, expected);
                            glyphs.push((
                                Point::new(
                                    24.0 + row.origin_in_lpxs.x + glyph.origin_in_lpxs.x,
                                    y as f32 + row.origin_in_lpxs.y + glyph.origin_in_lpxs.y,
                                ),
                                glyph.font_size_in_lpxs,
                                raster,
                            ));
                        }
                    }
                    text.draw_rasterized_glyphs_abs(&mut cx, &glyphs, vec4(1.0, 1.0, 1.0, 1.0));
                }
            }
            if scene.instance_clip {
                cx.pop_clip_rect();
            }
            list.end(&mut cx);
            cx.end_pass_sized_turtle();
            root.end(&mut cx);
            cx.end_pass(pass);

            // Retain the emitted instances: only list uniforms change. Quads
            // expect view_clip in list space, so inverse-translate the desired
            // screen rectangle, just as Atlas does for its retained scene.
            let mut transform = Mat4f::identity();
            transform.v[12] = scene.translation.x as f32;
            transform.v[13] = scene.translation.y as f32;
            let uniforms = &mut cx.draw_lists[list.id()].draw_list_uniforms;
            uniforms.view_shift = scene.shift.into();
            if let Some([left, top, right, bottom]) = scene.list_clip {
                uniforms.view_clip = vec4(
                    left as f32 - scene.translation.x as f32,
                    top as f32 - scene.translation.y as f32,
                    right as f32 - scene.translation.x as f32,
                    bottom as f32 - scene.translation.y as f32,
                );
            }
            list.set_view_transform(&mut cx, &transform);
        }
    }))));
    cx.borrow_mut().init_cx_os();
    // The ordinary headless loop captures this app's drawable and returns
    // after one frame; no visible window or persistent process is launched.
    Cx::event_loop(cx);
    let bytes = std::fs::read(directory.join("window_0_frame_000000.png")).unwrap();
    let mut png = makepad_zune_png::PngDecoder::new(
        makepad_zune_png::makepad_zune_core::bytestream::ZCursor::new(bytes),
    );
    let pixels = png.decode_raw().unwrap();
    assert_eq!(png.dimensions().unwrap(), (WIDTH, HEIGHT));
    assert_eq!(pixels.len(), WIDTH * HEIGHT * 4);
    pixels
}

fn diff(a: &[u8], b: &[u8]) -> usize {
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count()
}

#[test]
#[ignore = "requires a MAKEPAD=headless release build"]
fn text_clips_like_quads_after_view_transform() {
    assert!(std::env::var("MAKEPAD")
        .unwrap_or_default()
        .contains("headless"));
    std::env::set_var("MAKEPAD_HEADLESS_DPI", "1");
    let keep = std::env::var_os("MAKEPAD_HEADLESS_OUT_DIR");
    let directory = keep
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("makepad-text-clip-{}", std::process::id()))
        });
    for glyph_path in [
        GlyphPath::Sdf,
        GlyphPath::Msdf,
        GlyphPath::Slug,
        GlyphPath::Emoji,
    ] {
        let name = format!("{glyph_path:?}");
        let base = Scene {
            glyph_path,
            translation: dvec2(0.0, 0.0),
            shift: dvec2(0.0, 0.0),
            list_clip: None,
            instance_clip: false,
        };
        let baseline = render(base, &directory.join(format!("{name}_baseline")));
        let control = render(
            Scene {
                list_clip: Some([0, 0, WIDTH, HEIGHT]),
                ..base
            },
            &directory.join(format!("{name}_control")),
        );
        let changed = diff(&baseline, &control);
        println!(
            "{name} identity pixel diff: {changed}/{} ({:.6}%)",
            WIDTH * HEIGHT,
            changed as f64 * 100.0 / (WIDTH * HEIGHT) as f64
        );
        assert!(changed as f64 / (WIDTH * HEIGHT) as f64 <= 0.005);

        for (case, shift, instance_clip) in [
            ("translated", dvec2(0.0, 0.0), false),
            ("shifted_intersection", dvec2(11.0, -9.0), true),
        ] {
            let scene = Scene {
                translation: dvec2(25.0, 17.0),
                shift,
                instance_clip,
                ..base
            };
            let reference = render(scene, &directory.join(format!("{name}_{case}_reference")));
            let clipped = render(
                Scene {
                    list_clip: Some(CLIP),
                    ..scene
                },
                &directory.join(format!("{name}_{case}_clipped")),
            );
            let [left, top, right, bottom] = CLIP;
            let mut expected = reference.clone();
            let mut outside_ink = 0;
            let mut outside_pixels = 0;
            let mut inside_ink = 0;
            let mut removed_ink = 0;
            let mut quad_boundary = 0;
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    let offset = (y * WIDTH + x) * 4;
                    let pixel = &clipped[offset..offset + 4];
                    let original = &reference[offset..offset + 4];
                    let ink = pixel[1] > 8 || pixel[2] > 8;
                    if x < left || x >= right || y < top || y >= bottom {
                        outside_ink += usize::from(pixel[1] != 0 || pixel[2] != 0);
                        outside_pixels += usize::from(pixel[..3] != [0, 0, 0]);
                        removed_ink += usize::from(original[1] > 8 || original[2] > 8);
                        expected[offset..offset + 4].copy_from_slice(&[0, 0, 0, 255]);
                    } else {
                        inside_ink += usize::from(ink);
                        if (x == left || x == right - 1) && pixel == [255, 0, 0, 255] {
                            quad_boundary += 1;
                        }
                    }
                }
            }
            let changed = diff(&expected, &clipped);
            println!("{name} {case} pixels: outside_glyph={outside_ink} outside_clip={outside_pixels} inside_glyph={inside_ink} removed_glyph={removed_ink} quad_boundary={quad_boundary} crop_diff={changed}/{}", WIDTH * HEIGHT);
            assert_eq!(
                outside_ink, 0,
                "{name} {case}: glyph leaked outside list clip"
            );
            assert_eq!(
                outside_pixels, 0,
                "{name} {case}: pixels leaked outside list clip"
            );
            assert!(
                inside_ink > 100 && removed_ink > 100,
                "must visibly cut glyphs"
            );
            assert!(quad_boundary > 10, "quad must reach the same clip boundary");
            assert!(
                changed as f64 / (WIDTH * HEIGHT) as f64 <= 0.005,
                "{name} {case}: clipping changed glyph sampling"
            );
        }
    }
    if keep.is_none() {
        std::fs::remove_dir_all(directory).unwrap();
    }
}
