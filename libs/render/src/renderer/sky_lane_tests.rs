use super::*;
use crate::model::tests::room_and_sky_glb;

/// The Asset Store keeps the real E1M1 payload outside the source tree's
/// normal test fixtures. When it is present, pin the whole parsed upload
/// hand-off: the embedded SKY1 PNG must become the resident sky texture,
/// not the renderer's black null texture. A clean checkout simply skips
/// this real-asset diagnostic, like the kit tests in model.rs.
#[test]
fn real_e1m1_parsed_upload_keeps_the_embedded_sky1_pixels() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../local/asset-library/store/cas/objects/92/e2/\
         92e29a55fc78fc51fa84022536634f01dad5af350f734b772ad168b0f005f264",
    );
    let Ok(glb) = std::fs::read(&path) else {
        eprintln!("real E1M1 store payload absent — skipped");
        return;
    };
    let model = StaticModel::parse_glb(&glb).expect("parse real E1M1");
    let sky = model.sky.as_ref().expect("tagged E1M1 sky node");
    assert_eq!(sky.texture.as_deref(), Some("sky1"));
    assert_eq!(sky.images.len(), 1, "one embedded SKY1 image");
    let decoded = ImageBuffer::from_png(&sky.images[0]).expect("decode embedded SKY1");
    assert_eq!((decoded.width, decoded.height), (256, 128));
    assert!(
        decoded.data.iter().any(|pixel| pixel & 0x00ff_ffff != 0),
        "SKY1 itself must not decode as black"
    );
    let expected = decoded.data;

    let mut cx = Cx::new(Box::new(|_, _| {}));
    let mut renderer = Renderer::default();
    renderer
        .load_model_parsed(&mut cx, "doom/e1m1", model, None, None)
        .expect("upload parsed E1M1");
    let loaded = renderer
        .static_models
        .iter()
        .find(|(id, _)| id == "doom/e1m1")
        .and_then(|(_, model)| model.sky.as_ref())
        .expect("resident E1M1 sky");
    assert_ne!(loaded.tex0.texture_id(), cx.null_texture.texture_id());
    match loaded.tex0.get_format(&mut cx) {
        TextureFormat::VecBGRAu8_32 {
            width,
            height,
            data,
            ..
        } => {
            assert_eq!((*width, *height), (256, 128));
            assert_eq!(data.as_deref(), Some(expected.as_slice()));
        }
        other => panic!("Doom sky must be a one-level texture, got {other:?}"),
    }
}

#[test]
/// Which sky gets a mip chain, and why. A cylinder strip must NOT: it is
/// magnified in every view that exists, and the chain is what turns
/// `atan2`'s branch cut into a one-pixel line down the sky (the hairline
/// on Doom maps). The other two keep theirs — Quake's swirl and the
/// equirect poles are real minification.
fn only_the_cylinder_sky_ships_without_mips() {
    use crate::model::SkyProjection;
    assert!(!sky_wants_mips(SkyProjection::Cylinder), "the seam lives here");
    assert!(sky_wants_mips(SkyProjection::QuakeScroll));
    assert!(sky_wants_mips(SkyProjection::Cube));
}

#[test]
/// The cut itself, in the CPU twin of the shader's mapping: two headings
/// a hair apart across it land a whole texture period apart in u — the
/// jump the hardware would have read as "minified to nothing" — while
/// the colour they name is the same texel. Nothing here can fix that;
/// only having no mip levels can.
fn the_longitude_cut_jumps_by_whole_periods() {
    use crate::model::{SkyPart, SkyProjection};
    let part = SkyPart {
        projection: SkyProjection::Cylinder,
        repeat: 4.0,
        speeds: vec![0.0],
        offset: 0.0,
        texture: None,
        v_span: 0.5,
        vertices: Vec::new(),
        indices: Vec::new(),
        min: vec3f(0.0, 0.0, 0.0),
        max: vec3f(0.0, 0.0, 0.0),
        images: Vec::new(),
    };
    let eps = 1.0e-4;
    let left = part.direction_uv(vec3f(eps, 0.0, -1.0), 0, 0.0);
    let right = part.direction_uv(vec3f(-eps, 0.0, -1.0), 0, 0.0);
    let jump = (left[0] - right[0]).abs();
    assert!(
        (jump - part.repeat).abs() < 1.0e-3,
        "one turn of the compass = `repeat` periods of u, got {jump}"
    );
    // Whole periods: the two sides name the same texel, so the picture
    // is continuous even though the coordinate is not.
    assert!((jump - jump.round()).abs() < 1.0e-3);
    // Away from the cut the mapping is smooth — a degree of yaw moves u
    // by a degree's worth, not by a period.
    let a = part.direction_uv(vec3f(0.0, 0.0, 1.0), 0, 0.0);
    let b = part.direction_uv(vec3f(0.017, 0.0, 1.0), 0, 0.0);
    assert!((a[0] - b[0]).abs() < 0.02, "smooth away from the cut");
}

#[test]
fn the_sky_clock_advances_and_can_be_pinned() {
    let mut renderer = Renderer::default();
    assert_eq!(renderer.sky_time(), 0.0);
    renderer.tick_sky(0.25);
    renderer.tick_sky(0.25);
    assert!((renderer.sky_time() - 0.5).abs() < 1.0e-6);

    // A capture pins the clock so the same frame renders the same sky.
    renderer.set_sky_time(3.0);
    assert_eq!(renderer.sky_time(), 3.0);

    // Long sessions wrap rather than losing precision in the offset.
    renderer.set_sky_time(4095.5);
    renderer.tick_sky(1.0);
    assert!(renderer.sky_time() < 1.0, "{}", renderer.sky_time());

    // Garbage dt is ignored rather than poisoning the clock.
    renderer.set_sky_time(2.0);
    renderer.tick_sky(f32::NAN);
    assert_eq!(renderer.sky_time(), 2.0);
}

#[test]
fn a_model_without_a_sky_answers_none() {
    let renderer = Renderer::default();
    assert!(renderer.model_sky("maps/e1m1").is_none());
    assert!(renderer.model_sky_mesh("maps/e1m1").is_none());
}

/// What the shader is handed for a Doom sky, computed the way
/// `draw_sky_faces` computes it — the parse and the draw agreeing on
/// projection code, repeat and scroll is the contract the GPU cannot
/// check for us.
#[test]
fn the_shader_parameters_follow_the_parsed_sky() {
    let m = StaticModel::parse_glb(&room_and_sky_glb(Some("quake_scroll"), 2)).unwrap();
    let sky = m.sky.as_ref().unwrap();
    let time = 1.5;
    let sky_p = vec4(
        sky.projection.code(),
        sky.repeat,
        sky.scroll(0, time),
        sky.scroll(1, time),
    );
    // 2.0 is the branch the shader's `sky_p.x < 2.5` arm takes.
    assert_eq!(sky_p.x, 2.0);
    assert_eq!(sky_p.y, 4.0);
    // The map's static phase (0.25) plus 1.5 s at 8 and 16 units.
    assert_eq!(sky_p.z, 0.25 + 12.0);
    assert_eq!(sky_p.w, 0.25 + 24.0);
    let sky_q = vec4(sky.v_span, 1.0, 0.0, 0.0);
    assert_eq!(sky_q.x, crate::model::SKY_DEFAULT_V_SPAN);
}
