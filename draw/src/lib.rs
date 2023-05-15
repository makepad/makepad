pub use makepad_image_formats;
pub use makepad_platform;
pub use makepad_platform::*;
pub use makepad_vector;

pub mod overlay;
pub mod cx_2d;
pub mod view;
pub mod shader;
pub mod turtle;
pub mod font_atlas;
pub mod geometry;
pub mod nav;
pub mod icon_atlas;
 
pub use crate::{
    font_atlas::Font,
    turtle::{
        Axis,
        Layout,
        Walk,
        Align,
        Padding,
        Flow,
        Size,
        DeferWalk
    },
    overlay::{
        Overlay
    },
    nav::{
        NavRole,
        NavOrder,
        NavStop,
        NavItem,
        NavScrollIndex
    },
    view::{
        View,
        ManyInstances,
        ViewRedrawing,
        ViewRedrawingApi,
    },
    cx_2d::{
        Cx2d
    },
    shader::{
        //draw_shape::{DrawShape, Shape, Fill},
        draw_icon::DrawIcon,
        draw_quad::DrawQuad,
        draw_text::DrawText,
        draw_color::DrawColor,
    },
    geometry::{
        GeometryGen,
        GeometryQuad2D,
    },
};

pub fn live_design(cx: &mut Cx) {
    crate::shader::draw_quad::live_design(cx);
    crate::shader::draw_color::live_design(cx);
    crate::shader::draw_icon::live_design(cx);
    crate::shader::draw_text::live_design(cx);
    crate::geometry::geometry_gen::live_design(cx);
    crate::shader::std::live_design(cx);
    crate::shader::draw_trapezoid::live_design(cx);
}
