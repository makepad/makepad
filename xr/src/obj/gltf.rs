use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};
use std::{path::PathBuf, rc::Rc};

use super::{
    gltf_bridge::GltfRenderer,
    scene_draw::{apply_scene_to_draw_pbr, compose_scene_node_transform, scene_state_from_cx},
    xr_node::xr_widget_world_transform,
    XrNode,
};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.GltfBase = #(Gltf::register_widget(vm))
    mod.widgets.Gltf = set_type_default() do mod.widgets.GltfBase{
        draw_pbr +: {
            light_dir: vec3(0.35, 0.8, 0.45)
            light_color: vec3(1.0, 1.0, 1.0)
            ambient: 0.22
            spec_power: 128.0
            spec_strength: 0.9
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct Gltf {
    #[redraw]
    #[live]
    draw_pbr: DrawPbr,

    #[live]
    src: Option<ScriptHandleRef>,
    #[live]
    env_src: Option<ScriptHandleRef>,
    #[live(vec3(0.0, 0.0, 0.0))]
    mesh_position: Vec3f,
    #[live(vec3(0.0, 0.0, 0.0))]
    mesh_rotation: Vec3f,
    #[live(vec3(1.0, 1.0, 1.0))]
    mesh_scale: Vec3f,
    #[cast]
    #[deref]
    node: XrNode,

    #[rust]
    renderer: Option<GltfRenderer>,
    #[rust]
    loaded_src_handle: Option<ScriptHandle>,
    #[rust]
    loaded_env_handle: Option<ScriptHandle>,
}

enum ResourceResolve {
    Ready {
        handle: ScriptHandle,
        abs_path: PathBuf,
        data: Rc<Vec<u8>>,
    },
    Pending {
        handle: ScriptHandle,
    },
    Error {
        handle: ScriptHandle,
    },
    Missing,
}

impl Gltf {
    pub fn node(&self) -> &XrNode {
        &self.node
    }

    fn mesh_transform(&self) -> Mat4f {
        compose_scene_node_transform(self.mesh_position, self.mesh_rotation, self.mesh_scale)
    }

    fn resource_metadata_by_handle(cx: &mut Cx, handle: ScriptHandle) -> Option<(PathBuf, bool)> {
        let resources = cx.script_data.resources.resources.borrow();
        let resource = resources
            .iter()
            .find(|resource| resource.handle == handle)?;
        Some((PathBuf::from(&resource.abs_path), resource.is_error()))
    }

    fn resolve_resource(cx: &mut Cx, handle_ref: &ScriptHandleRef) -> ResourceResolve {
        let handle = handle_ref.as_handle();

        if let Some(data) = cx.get_resource(handle) {
            let abs_path = Self::resource_metadata_by_handle(cx, handle)
                .map(|metadata| metadata.0)
                .unwrap_or_else(|| PathBuf::from("resource"));
            return ResourceResolve::Ready {
                handle,
                abs_path,
                data,
            };
        }

        cx.load_script_resource(handle);

        if let Some(data) = cx.get_resource(handle) {
            let abs_path = Self::resource_metadata_by_handle(cx, handle)
                .map(|metadata| metadata.0)
                .unwrap_or_else(|| PathBuf::from("resource"));
            return ResourceResolve::Ready {
                handle,
                abs_path,
                data,
            };
        }

        if let Some((_, is_error)) = Self::resource_metadata_by_handle(cx, handle) {
            if is_error {
                return ResourceResolve::Error { handle };
            }
            return ResourceResolve::Pending { handle };
        }

        ResourceResolve::Missing
    }

    fn ensure_env_loaded(&mut self, cx: &mut CxDraw) {
        let Some(handle_ref) = self.env_src.as_ref() else {
            return;
        };
        let handle = handle_ref.as_handle();
        if self.loaded_env_handle == Some(handle) {
            return;
        }

        match Self::resolve_resource(cx, handle_ref) {
            ResourceResolve::Ready {
                handle,
                abs_path,
                data,
            } => {
                let _ =
                    self.draw_pbr
                        .load_default_env_equirect_from_bytes(cx, &data, Some(&abs_path));
                self.loaded_env_handle = Some(handle);
            }
            ResourceResolve::Error { handle } => {
                self.loaded_env_handle = Some(handle);
            }
            ResourceResolve::Pending { handle } => {
                let _ = handle;
            }
            ResourceResolve::Missing => {}
        }
    }

    fn ensure_renderer_loaded(&mut self, cx: &mut CxDraw) {
        let Some(handle_ref) = self.src.as_ref() else {
            return;
        };
        let handle = handle_ref.as_handle();

        if self.loaded_src_handle == Some(handle) {
            return;
        }

        match Self::resolve_resource(cx, handle_ref) {
            ResourceResolve::Ready {
                handle,
                abs_path,
                data,
            } => {
                self.renderer =
                    GltfRenderer::load_from_bytes(&mut self.draw_pbr, cx, &data, Some(&abs_path))
                        .ok();
                self.loaded_src_handle = Some(handle);
            }
            ResourceResolve::Error { handle } => {
                self.renderer = None;
                self.loaded_src_handle = Some(handle);
            }
            ResourceResolve::Pending { handle } => {
                let _ = handle;
            }
            ResourceResolve::Missing => {}
        }
    }
}

impl Widget for Gltf {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.handle_event(cx, event);
        }
        self.node.handle_event(cx, event, scope);
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        let Some(_scene) = scene_state_from_cx(cx) else {
            return DrawStep::done();
        };
        let object_world = xr_widget_world_transform(cx, scope, self.widget_uid(), &self.node);
        let world = Mat4f::mul(&object_world, &self.mesh_transform());

        let _ = apply_scene_to_draw_pbr(&mut self.draw_pbr, cx);
        self.ensure_env_loaded(cx);
        self.ensure_renderer_loaded(cx);
        if let Some(renderer) = self.renderer.as_mut() {
            let _ = renderer.draw_with_transform(&mut self.draw_pbr, cx, world);
        }

        self.node.draw_3d(cx, scope)
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}
