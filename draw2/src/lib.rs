//pub use makepad_image_formats;
pub use makepad_platform2;
pub use makepad_platform2 as makepad_platform;
pub use makepad_platform2::*;
pub mod cx_2d;
pub mod cx_3d;
pub mod cx_draw;
pub mod draw_list_2d;
pub mod geometry;
pub mod match_event;
pub mod nav;
pub mod overlay;
pub mod shader;
pub mod text;
pub mod turtle;

pub use crate::{
    cx_2d::Cx2d,
    cx_3d::Cx3d,
    cx_draw::CxDraw,
    draw_list_2d::{DrawList2d, DrawListExt, ManyInstances, Redrawing, RedrawingApi},
    match_event::MatchEvent,
    nav::{NavItem, NavOrder, NavRole, NavScrollIndex, NavStop},
    overlay::Overlay,
    shader::{
        draw_quad::DrawColor,
        //draw_shape::{DrawShape, Shape, Fill},
        draw_quad::DrawQuad,
        draw_text::DrawText,
        draw_text::TextStyle,
    },
    /*
    geometry::{
        GeometryGen,
        GeometryQuad2D,
    },*/
    turtle::{Align, DeferredWalk, Flow, Layout, Metrics, Size, TurtleAlignRange, Walk},
};

#[cfg(feature = "svg")]
pub use crate::shader::draw_svg::DrawSvg;

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    crate::turtle::script_mod(vm);
    crate::shader::sdf::script_mod(vm);
    crate::geometry::script_mod(vm);
    crate::shader::draw_quad::script_mod(vm);
    crate::shader::draw_text::script_mod(vm);
    #[cfg(feature = "svg")]
    crate::shader::draw_svg::script_mod(vm);
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
