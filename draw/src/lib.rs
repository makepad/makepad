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
        draw_cube::DrawCube, draw_glyph::DrawGlyph, draw_pbr::DrawPbr,
        draw_pbr::DrawPbrMaterialState, draw_pbr::DrawPbrTextureSet, draw_quad::DrawColor,
        draw_quad::DrawQuad, draw_rotated_text::DrawRotatedText,
        draw_rotated_text::PathGlyphInstance, draw_rotated_text::PathTextPlacement,
        draw_svg_glyph::DrawSvgGlyph, draw_text::DrawText, draw_text::TextStyle,
        draw_text_3d::DrawText3d, draw_vector::DrawVector,
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

const DRAW_REGISTRY_MODULE: LiveId = live_id!(makepad_draw_registered);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DrawModule {
    Turtle,
    Sdf,
    Geometry,
    DrawQuad,
    DrawCube,
    DrawGlyph,
    DrawPbr,
    DrawRotatedText,
    DrawSvg,
    DrawSvgGlyph,
    DrawText,
    DrawText3d,
    DrawVector,
}

pub const ALL_DRAW_MODULES: &[DrawModule] = &[
    DrawModule::Turtle,
    DrawModule::Sdf,
    DrawModule::Geometry,
    DrawModule::DrawQuad,
    DrawModule::DrawCube,
    DrawModule::DrawGlyph,
    DrawModule::DrawPbr,
    DrawModule::DrawRotatedText,
    DrawModule::DrawSvg,
    DrawModule::DrawSvgGlyph,
    DrawModule::DrawText,
    DrawModule::DrawText3d,
    DrawModule::DrawVector,
];

impl DrawModule {
    fn marker_id(self) -> LiveId {
        match self {
            DrawModule::Turtle => live_id!(turtle),
            DrawModule::Sdf => live_id!(sdf),
            DrawModule::Geometry => live_id!(geometry),
            DrawModule::DrawQuad => live_id!(draw_quad),
            DrawModule::DrawCube => live_id!(draw_cube),
            DrawModule::DrawGlyph => live_id!(draw_glyph),
            DrawModule::DrawPbr => live_id!(draw_pbr),
            DrawModule::DrawRotatedText => live_id!(draw_rotated_text),
            DrawModule::DrawSvg => live_id!(draw_svg),
            DrawModule::DrawSvgGlyph => live_id!(draw_svg_glyph),
            DrawModule::DrawText => live_id!(draw_text),
            DrawModule::DrawText3d => live_id!(draw_text_3d),
            DrawModule::DrawVector => live_id!(draw_vector),
        }
    }

    fn dependencies(self) -> &'static [DrawModule] {
        match self {
            DrawModule::DrawRotatedText => &[DrawModule::DrawText],
            DrawModule::DrawSvg => &[DrawModule::DrawVector],
            DrawModule::DrawSvgGlyph => &[DrawModule::DrawGlyph],
            DrawModule::DrawText3d => &[DrawModule::DrawRotatedText],
            _ => &[],
        }
    }

    fn register_script_mod(self, vm: &mut ScriptVm) {
        match self {
            DrawModule::Turtle => {
                crate::turtle::script_mod(vm);
            }
            DrawModule::Sdf => {
                crate::shader::sdf::script_mod(vm);
            }
            DrawModule::Geometry => {
                crate::geometry::script_mod(vm);
            }
            DrawModule::DrawQuad => {
                crate::shader::draw_quad::script_mod(vm);
            }
            DrawModule::DrawCube => {
                crate::shader::draw_cube::script_mod(vm);
            }
            DrawModule::DrawGlyph => {
                crate::shader::draw_glyph::script_mod(vm);
            }
            DrawModule::DrawPbr => {
                crate::shader::draw_pbr::script_mod(vm);
            }
            DrawModule::DrawRotatedText => {
                crate::shader::draw_rotated_text::script_mod(vm);
            }
            DrawModule::DrawSvg => {
                crate::shader::draw_svg::script_mod(vm);
            }
            DrawModule::DrawSvgGlyph => {
                crate::shader::draw_svg_glyph::script_mod(vm);
            }
            DrawModule::DrawText => {
                crate::shader::draw_text::script_mod(vm);
            }
            DrawModule::DrawText3d => {
                crate::shader::draw_text_3d::script_mod(vm);
            }
            DrawModule::DrawVector => {
                crate::shader::draw_vector::script_mod(vm);
            }
        }
    }
}

fn draw_registry_module(vm: &mut ScriptVm) -> ScriptObject {
    let existing = vm
        .bx
        .heap
        .value(vm.bx.heap.modules, DRAW_REGISTRY_MODULE.into(), NoTrap);
    if let Some(module) = existing.as_object() {
        module
    } else {
        vm.new_module(DRAW_REGISTRY_MODULE)
    }
}

fn draw_module_registered(vm: &mut ScriptVm, module: DrawModule) -> bool {
    let registry = draw_registry_module(vm);
    vm.bx
        .heap
        .value(registry, module.marker_id().into(), NoTrap)
        .as_object()
        .is_some()
}

fn mark_draw_module_registered(vm: &mut ScriptVm, module: DrawModule) {
    let registry = draw_registry_module(vm);
    vm.bx
        .heap
        .set_value_def(registry, module.marker_id().into(), registry.into());
}

fn register_draw_module_recursive(vm: &mut ScriptVm, module: DrawModule) {
    if draw_module_registered(vm, module) {
        return;
    }

    for dependency in module.dependencies() {
        register_draw_module_recursive(vm, *dependency);
    }

    module.register_script_mod(vm);
    mark_draw_module_registered(vm, module);
}

pub fn register_draw_modules(vm: &mut ScriptVm, modules: &[DrawModule]) {
    for module in modules {
        register_draw_module_recursive(vm, *module);
    }
}

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    register_draw_modules(vm, ALL_DRAW_MODULES);
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
