pub use makepad_image_formats;
pub use makepad_platform;
pub use makepad_platform::*;
pub use makepad_vector;

pub mod overlay;
pub mod cx_2d;
pub mod view;
pub mod shader;
pub mod turtle;
pub mod font;
pub mod geometry;
pub mod nav;

pub use crate::{
    font::Font,
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
        draw_shape::{DrawShape, Shape, Fill},
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
    crate::shader::draw_shape::live_design(cx);
    crate::shader::draw_text::live_design(cx);
    crate::geometry::geometry_gen::live_design(cx);
    crate::shader::std::live_design(cx);
    crate::font::live_design(cx);
}
