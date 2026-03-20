use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};
use std::{path::PathBuf, rc::Rc};

use super::{
    gltf_bridge::GltfRenderer,
    scene_3d::{
        apply_scene_to_draw_pbr, compose_scene_node_transform, register_draw_call_anchor,
        scene_node_world_transform_from_scope, scene_state_from_scope,
    },
};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.Gltf3DBase = #(Gltf3D::register_widget(vm))
    mod.widgets.Gltf3D = set_type_default() do mod.widgets.Gltf3DBase{
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
pub struct Gltf3D {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_pbr: DrawPbr,

    #[live]
    src: Option<ScriptHandleRef>,
    #[live]
    env_src: Option<ScriptHandleRef>,

    #[live(vec3(0.0, 0.0, 0.0))]
    position: Vec3f,
    #[live(vec3(0.0, 0.0, 0.0))]
    rotation: Vec3f,
    #[live(vec3(1.0, 1.0, 1.0))]
    scale: Vec3f,

    #[rust]
    renderer: Option<GltfRenderer>,
    #[rust]
    loaded_src_handle: Option<ScriptHandle>,
    #[rust]
    loaded_env_handle: Option<ScriptHandle>,
    #[rust]
    debug_logged_src_pending: bool,
    #[rust]
    debug_logged_src_ready: bool,
    #[rust]
    debug_logged_src_error: bool,
    #[rust]
    debug_logged_draw_without_renderer: bool,
    #[rust]
    debug_logged_first_draw: bool,
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

impl Gltf3D {
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

    fn ensure_env_loaded(&mut self, cx: &mut Cx2d) {
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

    fn ensure_renderer_loaded(&mut self, cx: &mut Cx2d) {
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
                if !self.debug_logged_src_ready {
                    self.debug_logged_src_ready = true;
                    log!(
                        "gltf3d renderer ready path={} renderer_loaded={}",
                        abs_path.display(),
                        self.renderer.is_some()
                    );
                }
            }
            ResourceResolve::Error { handle } => {
                self.renderer = None;
                self.loaded_src_handle = Some(handle);
                if !self.debug_logged_src_error {
                    self.debug_logged_src_error = true;
                    log!("gltf3d renderer load error handle={:?}", handle);
                }
            }
            ResourceResolve::Pending { handle } => {
                let _ = handle;
                if !self.debug_logged_src_pending {
                    self.debug_logged_src_pending = true;
                    log!("gltf3d renderer pending handle={:?}", handle);
                }
            }
            ResourceResolve::Missing => {}
        }
    }
}

impl Widget for Gltf3D {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.handle_event(cx, event);
        }
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        let Some(scene) = scene_state_from_scope(scope) else {
            return DrawStep::done();
        };
        let cx = &mut Cx2d::new(cx.cx);

        self.ensure_env_loaded(cx);
        self.ensure_renderer_loaded(cx);
        let Some(renderer) = self.renderer.as_mut() else {
            if !self.debug_logged_draw_without_renderer {
                self.debug_logged_draw_without_renderer = true;
                log!("gltf3d draw skipped: renderer not loaded");
            }
            return DrawStep::done();
        };
        if !self.debug_logged_first_draw {
            self.debug_logged_first_draw = true;
            log!("gltf3d draw with renderer");
        }

        apply_scene_to_draw_pbr(&mut self.draw_pbr, cx, &scene);
        let local = compose_scene_node_transform(self.position, self.rotation, self.scale);
        let world = Mat4f::mul(
            &scene_node_world_transform_from_scope(scope),
            &local,
        );
        let _ = renderer.draw_with_transform_anchors(
            &mut self.draw_pbr,
            cx,
            world,
            |area, world_pos| register_draw_call_anchor(scope, area, world_pos),
        );
        DrawStep::done()
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}
