//pub use makepad_image_formats;
pub use makepad_platform;
pub use makepad_platform::*;
pub use makepad_zune_jpeg;
pub use makepad_zune_png;
pub mod cx_2d;
pub mod cx_3d;
pub mod cx_draw;
pub mod draw_list_2d;
pub mod geometry;
pub mod glyph_capture;
pub mod image_cache;
pub mod match_event;
pub mod nav;
pub mod native_layer;
pub mod overlay;
pub mod render_target;
pub mod scene_3d;
pub mod scene_sun;
pub mod size_expr;
pub mod shader;
pub mod svg;
pub mod text;
pub mod turtle;
pub mod vector;
pub mod xr_sink;

pub use crate::{
    cx_2d::Cx2d,
    cx_3d::Cx3d,
    cx_draw::CxDraw,
    draw_list_2d::{DrawList2d, DrawListExt, ManyInstances, Redrawing, RedrawingApi},
    glyph_capture::{
        CapturedFace, CapturedGlyph, CapturedPictured, CapturedRow, CapturedText, GlyphCapture, OutlineGlyph,
        PicturedText,
    },
    image_cache::{
        decode_image_from_data, evict_image_from_cache, handle_image_cache_network_responses,
        has_enough_pixels, image_size_by_data, is_animated_image, is_decoding_image,
        looks_like_svg, load_image_file_by_path_async, load_image_from_cache,
        load_image_from_data_async, load_image_from_data_async_at_size,
        load_image_http_by_url_async, process_async_image_load, AsyncImageLoad, AsyncLoadResult,
        ImageBuffer, ImageCache, ImageCacheImpl, ImageError, JpgDecodeErrors, PngDecodeErrors,
    },
    match_event::MatchEvent,
    nav::{NavItem, NavOrder, NavRole, NavScrollIndex, NavStop},
    native_layer::NativeLayerDraw,
    overlay::Overlay,
    render_target::{PicturePasses, RenderTarget, RenderTargetFormat, RenderTargetPool},
    scene_3d::{SceneDrawCallAnchor, SceneScope3D, SceneState3D},
    vector::{pack_pair_f16, pack_unorm8x4},
    scene_sun::{
        solar_dir, SceneSun, ShinyConfig, MAT_CANOPY, MAT_GREEN, MAT_NONE, MAT_ROOF,
        MAT_ROUTE_GLOW, MAT_SHADOW, MAT_WALL, MAT_WATER, SOLAR_DECLINATION_DEG,
    },
    size_expr::{SizeExprContext, SizeExprId, SizeExprSimple, SizeExprStore, SizeExprUnit},
    shader::{
        draw_cube::DrawCube, draw_glyph::DrawGlyph, draw_pbr::DrawPbr,
        draw_pbr::DrawPbrMaterialState, draw_pbr::DrawPbrRefractive, draw_pbr::DrawPbrTextureSet,
        draw_nv12::DrawNv12, draw_quad::DrawAffineQuad, draw_quad::DrawCachedQuad, draw_quad::DrawColor, draw_quad::DrawQuad, draw_rotated_text::DrawRotatedText,
        draw_rotated_text::PathGlyphInstance, draw_rotated_text::PathTextPlacement,
        draw_svg_glyph::DrawSvgGlyph, draw_text::stock_text_shader, draw_text::DrawText, draw_text::PreparedTextRun,
        draw_text::TextStyle, draw_text_3d::DrawText3d,
        draw_vector::{DrawVector, FeatherSide},
    },
    /*
    geometry::{
        GeometryGen,
        GeometryQuad2D,
    },*/
    turtle::{
        Align, Base, CellAlign, CellPlacement, DeferredWalk, Distribute, FitBound, Flow, Layout,
        Metrics, ResolveAt, Size, TurtleAlignRange, Walk,
    },
    vector::{GradientStop, VectorPaint},
};

pub use crate::shader::draw_svg::DrawSvg;

/// The draw layer's script registration in its order, one named step per module: `script_mod`
/// runs them all at once, and a host that spreads a VM's registration over frames (a Splash
/// isolate's UI) runs one step at a time.
pub const SCRIPT_MOD_STEPS: &[(&str, fn(&mut ScriptVm) -> ScriptValue)] = &[
    ("turtle", crate::turtle::script_mod),
    ("sdf", crate::shader::sdf::script_mod),
    ("surface", crate::shader::surface::script_mod),
    ("finish", crate::shader::finish::script_mod),
    ("geometry", crate::geometry::script_mod),
    ("draw_quad", crate::shader::draw_quad::script_mod),
    ("paint_lookup", crate::shader::paint_lookup::script_mod),
    ("video_color", crate::shader::video_color::script_mod),
    ("draw_nv12", crate::shader::draw_nv12::script_mod),
    ("draw_nv12_pack", crate::shader::draw_nv12_pack::script_mod),
    ("draw_fxaa", crate::shader::draw_fxaa::script_mod),
    ("draw_cube", crate::shader::draw_cube::script_mod),
    ("glyph_coverage", makepad_script::glyph_coverage::script_mod),
    ("draw_glyph", crate::shader::draw_glyph::script_mod),
    ("draw_text", crate::shader::draw_text::script_mod),
    ("draw_rotated_text", crate::shader::draw_rotated_text::script_mod),
    ("draw_sploded_hairline", crate::shader::draw_sploded_hairline::script_mod),
    ("draw_text_3d", crate::shader::draw_text_3d::script_mod),
    ("draw_vector", crate::shader::draw_vector::script_mod),
    ("ggx", crate::shader::ggx::script_mod),
    ("draw_pbr", crate::shader::draw_pbr::script_mod),
    ("draw_svg", crate::shader::draw_svg::script_mod),
    ("draw_svg_glyph", crate::shader::draw_svg_glyph::script_mod),
];

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    for (_, step) in SCRIPT_MOD_STEPS {
        step(vm);
    }
    NIL
}
/*
pub fn live_design(cx: &mut Cx) {
    crate::geometry::geometry_gen::live_design(cx);
    crate::shader::draw_quad::live_design(cx);
    crate::shader::draw_cube::live_design(cx);
    crate::shader::draw_color::live_design(cx);
    crate::shader::draw_icon::live_design(cx);
    crate::shader::draw_text::live_design(cx);
    crate::shader::draw_line::live_design(cx);
    crate::shader::std::live_design(cx);
    crate::shader::draw_trapezoid::live_design(cx);
}*/
