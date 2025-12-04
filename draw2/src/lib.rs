//pub use makepad_image_formats;
pub use makepad_platform2;
pub use makepad_platform2 as makepad_platform;
pub use makepad_platform2::*;
pub mod match_event;
pub mod overlay;
pub mod cx_2d;
pub mod draw_list_2d;
pub mod cx_3d;
pub mod cx_draw;
pub mod shader;
pub mod turtle;
pub mod geometry;
pub mod nav;
pub mod text;
 
pub use crate::{
    match_event::MatchEvent, 
    turtle::{
        Layout,
        Walk,
        Metrics,
        Align,
        Padding,
        Flow,
        Size,
        TurtleAlignRange,
        DeferredWalk
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
    //shader::{
        //draw_shape::{DrawShape, Shape, Fill},
        //draw_quad::DrawQuad,
        //draw_text::DrawText,
    //},
    /*
    geometry::{
        GeometryGen,
        GeometryQuad2D,
    },*/
};

pub fn script_run(vm:&mut ScriptVm)->ScriptValue{
    vm.heap.new_module(id!(shaders));
    crate::shader::draw_quad::script_run(vm);
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