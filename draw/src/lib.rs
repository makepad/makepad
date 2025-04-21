//pub use makepad_image_formats;
pub use makepad_platform;
pub use makepad_platform::*;
pub use makepad_vector;
pub mod match_event;
pub mod overlay;
pub mod cx_2d;
pub mod draw_list_2d;
pub mod cx_3d;
pub mod cx_draw;
pub mod shader;
pub mod turtle;
pub mod font_atlas;
pub mod glyph_rasterizer;
pub mod font_loader;
pub mod sdf_glyph_rasterizer;
pub mod svg_glyph_rasterizer;
pub mod geometry;
pub mod nav;
pub mod icon_atlas;
pub mod text;
pub mod text_shaper;

mod owned_font_face;
 
pub use crate::{
    match_event::MatchEvent, 
    font_atlas::Font,
    turtle::{
        Layout,
        Walk,
        Align,
        Padding,
        Flow,
        Size,
        TurtleAlignRange,
        DeferWalk
    },
    overlay::Overlay,
    nav::{
        NavRole,
        NavOrder,
        NavStop,
        NavItem,
        NavScrollIndex
    },
    draw_list_2d::{
        DrawListExt,
        DrawList2d,
        ManyInstances,
        Redrawing,
        RedrawingApi,
    },
    cx_draw::CxDraw,
    cx_2d::Cx2d,
    cx_3d::Cx3d,
    shader::{
        //draw_shape::{DrawShape, Shape, Fill},
        draw_icon::DrawIcon,
        draw_quad::DrawQuad,
        draw_line::DrawLine,
        draw_text::{Affinity, DrawText, IndexAffinity},
        draw_text_2::DrawText2,
        draw_color::DrawColor,
    },
    geometry::{
        GeometryGen,
        GeometryQuad2D,
    },
};

pub fn live_design(cx: &mut Cx) {
    crate::geometry::geometry_gen::live_design(cx);
    crate::shader::draw_quad::live_design(cx);
    crate::shader::draw_cube::live_design(cx);
    crate::shader::draw_color::live_design(cx);
    crate::shader::draw_icon::live_design(cx);
    crate::shader::draw_text::live_design(cx);
    crate::shader::draw_text_2::live_design(cx);
    crate::shader::draw_line::live_design(cx);
    crate::shader::std::live_design(cx);
    crate::shader::draw_trapezoid::live_design(cx);
}