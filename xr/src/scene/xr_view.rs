use crate::xr_node::{xr_widget_world_transform, XrNode};
use crate::*;
use makepad_widgets::event::XrFingerTip;
use std::cell::Cell;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.draw.DrawXrFingerCursor = mod.std.set_type_default() do #(DrawXrFingerCursor::script_shader(vm)){
        ..mod.draw.DrawQuad
        fill_color: vec4(0.26, 0.78, 1.0, 0.22)
        stroke_color: vec4(0.92, 0.97, 1.0, 0.96)
        stroke_width: 2.0

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size);
            let center = self.rect_size * 0.5;
            let radius = min(self.rect_size.x, self.rect_size.y) * 0.5 - self.stroke_width;
            sdf.circle(center.x, center.y, radius.max(1.0));
            sdf.fill_keep(self.fill_color);
            sdf.stroke(self.stroke_color, self.stroke_width);
            return sdf.result;
        }
    }

    mod.widgets.XrViewBase = #(XrView::register_widget(vm))
    mod.widgets.XrView = set_type_default() do mod.widgets.XrViewBase{
        pixel_scale: 0.0004
        dpi_factor: 3.0
        logical_size: vec2(320, 400)
        depth_scale: 300.0
        draw_cursor: mod.draw.DrawXrFingerCursor{}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawXrFingerCursor {
    #[deref] draw_super: DrawQuad,
    #[live] fill_color: Vec4f,
    #[live] stroke_color: Vec4f,
    #[live] stroke_width: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct XrFingerCursor {
    pos: Vec2d,
    size: f64,
    depth: f32,
    is_left: bool,
}

#[derive(Clone, Copy, Debug)]
struct XrPanelRayHit {
    projected: Vec3f,
    cursor_depth: f32,
    touch_z: f32,
}

#[derive(Script, WidgetRef, WidgetRegister)]
pub struct XrView {
    #[uid] uid: WidgetUid,
    #[source] source: ScriptObjectRef,
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[rust] area: Area,

    // 3D placement
    #[deref] node: XrNode,

    // Panel rendering
    #[live(vec2(320.0, 400.0))] logical_size: Vec2d,
    #[live(0.0004)] pixel_scale: f32,
    #[live(3.0)] dpi_factor: f64,
    #[live(300.0)] depth_scale: f32,
    #[live] draw_cursor: DrawXrFingerCursor,
    #[new] draw_list: DrawList2d,

    // 2D children
    #[rust] child_widgets: Vec<(LiveId, WidgetRef)>,
    #[rust] finger_cursor: Option<XrFingerCursor>,
}

impl XrView {
    const XR_CURSOR_HOVER_FRONT: f32 = 96.0;
    const XR_CURSOR_HOVER_BACK: f32 = -48.0;
    const XR_CURSOR_SIZE_NEAR: f64 = 30.0;
    const XR_CURSOR_SIZE_FAR: f64 = 16.0;

    pub(crate) fn node(&self) -> &XrNode {
        &self.node
    }

    fn panel_matrix(&self, world_transform: Mat4f) -> Mat4f {
        let scale = self.pixel_scale.max(0.00001) * self.dpi_factor.max(1.0) as f32;
        let local_depth = Mat4f::nonuniform_scaled_translation(
            vec3(1.0, 1.0, self.depth_scale.max(0.00001)),
            vec3(0.0, 0.0, 0.0),
        );
        let local_panel = Mat4f::nonuniform_scaled_translation(
            vec3(scale, -scale, scale),
            vec3(
                -(self.logical_size.x as f32) * scale * 0.5,
                (self.logical_size.y as f32) * scale * 0.5,
                0.0,
            ),
        );
        let object_to_world = Mat4f::mul(&local_panel, &local_depth);
        Mat4f::mul(&world_transform, &object_to_world)
    }

    fn hit_matrix(&self, world_transform: Mat4f) -> Mat4f {
        let scale = self.pixel_scale.max(0.00001) * self.dpi_factor.max(1.0) as f32;
        let local_panel = Mat4f::nonuniform_scaled_translation(
            vec3(scale, -scale, scale),
            vec3(
                -(self.logical_size.x as f32) * scale * 0.5,
                (self.logical_size.y as f32) * scale * 0.5,
                0.0,
            ),
        );
        Mat4f::mul(&world_transform, &local_panel)
    }

    fn panel_ray_hit(
        hit_matrix: &Mat4f,
        ray_origin: Vec3f,
        ray_dir: Vec3f,
        touch_z: f32,
    ) -> Option<XrPanelRayHit> {
        let inv = hit_matrix.invert();
        let origin = inv.transform_vec4(vec4(ray_origin.x, ray_origin.y, ray_origin.z, 1.0)).to_vec3f();
        let dir = inv.transform_vec4(vec4(ray_dir.x, ray_dir.y, ray_dir.z, 0.0)).to_vec3f();
        if dir.z.abs() <= 1.0e-6 { return None; }
        let t = -origin.z / dir.z;
        if t < 0.0 { return None; }
        Some(XrPanelRayHit {
            projected: origin + dir * t,
            cursor_depth: origin.z,
            touch_z,
        })
    }

    fn panel_normal_hit(hit_matrix: &Mat4f, tip_pos: Vec3f) -> XrPanelRayHit {
        let inv = hit_matrix.invert();
        let local = inv
            .transform_vec4(vec4(tip_pos.x, tip_pos.y, tip_pos.z, 1.0))
            .to_vec3f();
        XrPanelRayHit {
            projected: vec3f(local.x, local.y, 0.0),
            cursor_depth: local.z,
            touch_z: local.z,
        }
    }

    fn contains_local(&self, local: Vec3f) -> bool {
        local.x >= 0.0
            && local.y >= 0.0
            && local.x <= self.logical_size.x as f32
            && local.y <= self.logical_size.y as f32
    }

    pub(crate) fn hits_parent_ray(&self, ray_origin: Vec3f, ray_dir: Vec3f) -> bool {
        if !self.node.visible() {
            return false;
        }
        let hit_mat = self.hit_matrix(self.node.local_transform());
        Self::panel_ray_hit(&hit_mat, ray_origin, ray_dir, 0.0)
            .is_some_and(|hit| self.contains_local(hit.projected))
    }

    fn cursor_from_hit(&self, hit: XrPanelRayHit, is_left: bool) -> Option<XrFingerCursor> {
        if !self.contains_local(hit.projected) {
            return None;
        }
        if hit.cursor_depth > Self::XR_CURSOR_HOVER_FRONT || hit.cursor_depth < Self::XR_CURSOR_HOVER_BACK {
            return None;
        }
        let distance = hit.cursor_depth.abs().min(Self::XR_CURSOR_HOVER_FRONT);
        let proximity = 1.0 - (distance / Self::XR_CURSOR_HOVER_FRONT);
        let size =
            Self::XR_CURSOR_SIZE_FAR + (Self::XR_CURSOR_SIZE_NEAR - Self::XR_CURSOR_SIZE_FAR) * proximity as f64;
        Some(XrFingerCursor {
            pos: dvec2(hit.projected.x as f64, hit.projected.y as f64),
            size,
            depth: hit.cursor_depth,
            is_left,
        })
    }
}

impl ScriptHook for XrView {
    fn on_before_apply(&mut self, _vm: &mut ScriptVm, apply: &Apply, _scope: &mut Scope, _value: ScriptValue) {
        if apply.is_reload() {
            self.child_widgets.clear();
        }
    }

    fn on_after_apply(&mut self, vm: &mut ScriptVm, apply: &Apply, scope: &mut Scope, value: ScriptValue) {
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                self.child_widgets.clear();
                let mut anon_index = 0usize;
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        let id = if let Some(id) = kv.key.as_id() {
                            Some(id)
                        } else if kv.key.is_nil() {
                            let id = LiveId(anon_index as u64);
                            anon_index += 1;
                            Some(id)
                        } else {
                            None
                        };
                        let Some(id) = id else { continue };
                        if !WidgetRef::value_is_newable_widget(vm, kv.value) { continue }
                        let child = WidgetRef::script_from_value_scoped(vm, scope, kv.value);
                        self.child_widgets.push((id, child.clone()));
                        vm.cx_mut()
                            .widget_tree_insert_child_deep(self.uid, id, child);
                    }
                });
            }
        }
        vm.with_cx_mut(|cx| {
            cx.widget_tree_mark_dirty(self.uid);
        });
    }
}

impl WidgetNode for XrView {
    fn widget_uid(&self) -> WidgetUid { self.uid }
    fn walk(&mut self, _cx: &mut Cx) -> Walk { self.walk }
    fn area(&self) -> Area { self.area }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for (id, child) in &self.child_widgets {
            visit(*id, child.clone());
        }
    }

    fn redraw(&mut self, cx: &mut Cx) { cx.redraw_all(); }

    fn visible(&self) -> bool { self.node.visible() }
    fn set_visible(&mut self, cx: &mut Cx, visible: bool) { self.node.set_visible(cx, visible); }
}

impl Widget for XrView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.node.visible() && event.requires_visibility() { return; }

        // Forward XrLocal events — transform finger tips to panel-local 2D coords
        if let Event::XrLocal(xr_event) = event {
            let world_transform = self.node.local_transform();
            let hit_mat = self.hit_matrix(world_transform);
            let mut local_tips = SmallVec::new();
            let mut finger_cursor = None;
            for tip in &xr_event.finger_tips {
                let use_normal_projection = if tip.is_left {
                    xr_event.update.state.left_hand.in_view()
                        && xr_event
                            .update
                            .state
                            .left_hand
                            .tip_active(XrHand::INDEX_TIP)
                } else {
                    xr_event.update.state.right_hand.in_view()
                        && xr_event
                            .update
                            .state
                            .right_hand
                            .tip_active(XrHand::INDEX_TIP)
                };

                let hit = if use_normal_projection {
                    Some(Self::panel_normal_hit(&hit_mat, tip.pos))
                } else {
                    Self::panel_ray_hit(&hit_mat, tip.pos, tip.ray_dir, tip.touch_z)
                };

                if let Some(hit) = hit {
                    if let Some(candidate) = self.cursor_from_hit(hit, tip.is_left) {
                        let replace_cursor = finger_cursor
                            .map(|current: XrFingerCursor| candidate.depth.abs() < current.depth.abs())
                            .unwrap_or(true);
                        if replace_cursor {
                            finger_cursor = Some(candidate);
                        }
                    }
                    local_tips.push(XrFingerTip {
                        index: tip.index,
                        is_left: tip.is_left,
                        pos: vec3f(hit.projected.x, hit.projected.y, hit.touch_z),
                        ray_dir: vec3f(0.0, 0.0, -1.0),
                        touch_z: hit.touch_z,
                        handled: Cell::new(Area::Empty),
                    });
                }
            }
            self.finger_cursor = finger_cursor;
            let local_event = XrLocalEvent {
                finger_tips: local_tips,
                update: xr_event.update.clone(),
                modifiers: xr_event.modifiers,
                time: xr_event.time,
            };
            let event = Event::XrLocal(local_event.clone());
            for i in 0..self.child_widgets.len() {
                let child = self.child_widgets[i].1.clone();
                child.handle_event(cx, &event, scope);
            }
            local_event.process_end(cx);
            return;
        }

        // Forward other events to children
        for i in 0..self.child_widgets.len() {
            let child = self.child_widgets[i].1.clone();
            child.handle_event(cx, event, scope);
        }

        if matches!(event, Event::MouseLeave(_) | Event::MouseUp(_)) {
            self.finger_cursor = None;
        }
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        if !self.node.visible() { return DrawStep::done(); }

        let world_transform = xr_widget_world_transform(cx, scope, self.uid, &self.node);
        let matrix = self.panel_matrix(world_transform);

        // Draw 2D children into a DrawList2d with the panel transform
        let cx2d = &mut Cx2d::new(cx.cx);
        let previous_dpi = cx2d.current_dpi_factor();
        cx2d.set_current_pass_dpi_factor(self.dpi_factor.max(1.0));

        self.draw_list.begin_always(cx2d);
        self.draw_list.set_view_transform(cx2d, &matrix);
        let size = dvec2(self.logical_size.x.max(1.0), self.logical_size.y.max(1.0));
        cx2d.begin_root_turtle(size, Layout::flow_down());

        for i in 0..self.child_widgets.len() {
            let child = self.child_widgets[i].1.clone();
            child.draw_all(cx2d, scope);
        }

        if let Some(cursor) = self.finger_cursor {
            self.draw_cursor.fill_color = if cursor.is_left {
                vec4f(0.30, 0.78, 1.0, 0.24)
            } else {
                vec4f(1.0, 0.72, 0.26, 0.24)
            };
            self.draw_cursor.stroke_color = if cursor.is_left {
                vec4f(0.92, 0.97, 1.0, 0.96)
            } else {
                vec4f(1.0, 0.95, 0.86, 0.96)
            };
            self.draw_cursor.stroke_width = 2.0;
            self.draw_cursor.draw_abs(
                cx2d,
                Rect {
                    pos: dvec2(
                        cursor.pos.x - cursor.size * 0.5,
                        cursor.pos.y - cursor.size * 0.5,
                    ),
                    size: dvec2(cursor.size, cursor.size),
                },
            );
        }

        cx2d.end_pass_sized_turtle();
        self.draw_list.end(cx2d);
        cx2d.set_current_pass_dpi_factor(previous_dpi);

        DrawStep::done()
    }
}
