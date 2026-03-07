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
pub mod image_cache;
pub mod match_event;
pub mod nav;
pub mod overlay;
pub mod shader;
pub mod svg;
pub mod text;
pub mod turtle;
pub mod vector;

pub use crate::{
    cx_2d::Cx2d,
    cx_3d::Cx3d,
    cx_draw::CxDraw,
    draw_list_2d::{DrawList2d, DrawListExt, ManyInstances, Redrawing, RedrawingApi},
    image_cache::{
        handle_image_cache_network_responses, load_image_file_by_path_async, load_image_from_cache,
        load_image_from_data_async, load_image_http_by_url_async, process_async_image_load,
        AsyncImageLoad, AsyncLoadResult, ImageBuffer, ImageCache, ImageCacheImpl, ImageError,
        JpgDecodeErrors, PngDecodeErrors,
    },
    match_event::MatchEvent,
    nav::{NavItem, NavOrder, NavRole, NavScrollIndex, NavStop},
    overlay::Overlay,
    shader::{
        draw_glyph::DrawGlyph, draw_pbr::DrawPbr, draw_pbr::DrawPbrMaterialState,
        draw_pbr::DrawPbrTextureSet, draw_quad::DrawColor, draw_quad::DrawQuad,
        draw_rotated_text::DrawRotatedText, draw_rotated_text::PathGlyphInstance,
        draw_rotated_text::PathTextPlacement, draw_svg_glyph::DrawSvgGlyph, draw_text::DrawText,
        draw_text::TextStyle, draw_text_3d::DrawText3d, draw_vector::DrawVector,
    },
    /*
    geometry::{
        GeometryGen,
        GeometryQuad2D,
    },*/
    turtle::{Align, DeferredWalk, Flow, Layout, Metrics, Size, TurtleAlignRange, Walk},
    vector::{GradientStop, VectorPaint},
};

pub use crate::shader::draw_svg::DrawSvg;

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    crate::turtle::script_mod(vm);
    crate::shader::sdf::script_mod(vm);
    crate::geometry::script_mod(vm);
    crate::shader::draw_quad::script_mod(vm);
    crate::shader::draw_glyph::script_mod(vm);
    crate::shader::draw_text::script_mod(vm);
    crate::shader::draw_rotated_text::script_mod(vm);
    crate::shader::draw_text_3d::script_mod(vm);
    crate::shader::draw_vector::script_mod(vm);
    crate::shader::draw_pbr::script_mod(vm);
    crate::shader::draw_svg::script_mod(vm);
    crate::shader::draw_svg_glyph::script_mod(vm);
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
