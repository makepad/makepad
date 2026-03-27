use makepad_draw::draw_list_2d::{DrawList2d, DrawListExt};

use super::text_prepare::MpPreparedBrowserScene;
use super::{MpBrowserScene, MpBrowserSceneExecState, MpBrowserTask, MpBrowserTaskId, MpBrowserTaskKind};
use crate::surface::{MpSurface, MpSurfaceColorFormat};
use crate::*;

pub(super) struct TaskSurface {
    pub(super) surface: MpSurface,
    pub(super) draw_list: DrawList2d,
    pub(super) size: DVec2,
}

impl MpBrowserSceneExecState {
    pub(super) fn render_task(
        &mut self,
        cx: &mut Cx2d,
        scene: &MpBrowserScene,
        prepared_text: &MpPreparedBrowserScene,
        task_id: MpBrowserTaskId,
    ) -> Option<Texture> {
        let task = scene.tasks.get(task_id)?;
        if let Some(cache_key) = task.cache_key {
            if let Some(cached) = self.task_cache.get(&cache_key) {
                if cached.size == task.size {
                    self.frame_stats.task_cache_hit_count += 1;
                    return Some(cached.surface.color_texture().clone());
                }
            }
            let mut cached = self.task_cache.remove(&cache_key).unwrap_or_else(|| TaskSurface {
                surface: MpSurface::new(cx.cx, task.size, MpSurfaceColorFormat::BgraU8, false),
                draw_list: DrawList2d::new(cx.cx),
                size: task.size,
            });
            cached.size = task.size;
            cached.surface.resize(cx.cx, task.size);
            self.render_task_into_surface(cx, scene, prepared_text, task_id, task, &mut cached);
            let texture = cached.surface.color_texture().clone();
            self.task_cache.insert(cache_key, cached);
            return Some(texture);
        }

        let slot = self.alloc_scratch(cx.cx, task.size);
        let mut scratch = self.scratch_surfaces[slot]
            .take()
            .expect("scratch surface slot must exist");
        self.render_task_into_surface(cx, scene, prepared_text, task_id, task, &mut scratch);
        let texture = scratch.surface.color_texture().clone();
        self.scratch_surfaces[slot] = Some(scratch);
        Some(texture)
    }

    pub(super) fn alloc_scratch(&mut self, cx: &mut Cx, size: DVec2) -> usize {
        let slot = self.scratch_cursor;
        self.scratch_cursor += 1;
        self.frame_stats.scratch_surface_count = self.scratch_cursor;
        if self.scratch_surfaces.len() <= slot {
            self.scratch_surfaces.push(Some(TaskSurface {
                surface: MpSurface::new(cx, size, MpSurfaceColorFormat::BgraU8, false),
                draw_list: DrawList2d::new(cx),
                size,
            }));
            self.frame_stats.scratch_surface_new_alloc_count += 1;
        } else {
            let scratch = self.scratch_surfaces[slot]
                .as_mut()
                .expect("scratch surface slot must exist");
            scratch.size = size;
            scratch.surface.resize(cx, size);
            self.frame_stats.scratch_surface_reuse_count += 1;
        }
        slot
    }

    pub(super) fn render_task_into_surface(
        &mut self,
        cx: &mut Cx2d,
        scene: &MpBrowserScene,
        prepared_text: &MpPreparedBrowserScene,
        _task_id: MpBrowserTaskId,
        task: &MpBrowserTask,
        surface: &mut TaskSurface,
    ) {
        self.frame_stats.offscreen_task_count += 1;
        self.frame_stats.total_offscreen_pixel_area = self
            .frame_stats
            .total_offscreen_pixel_area
            .saturating_add(pixel_area(task.size));

        surface.surface.resize(cx.cx, task.size);
        surface.surface.begin(cx, None);
        cx.set_pass_shift_scale(surface.surface.pass(), dvec2(0.0, 0.0), dvec2(1.0, 1.0));
        surface.draw_list.begin_always(cx);
        let task_host_rect = task_host_rect(task);
        debug_assert_eq!(task_host_rect.size, task.size, "task host rect must match task surface size");
        // Task scenes re-root by their own task host rect, never the parent
        // scene host rect. This keeps nested text runs local to the task
        // surface. The view-transform translation subtracts the task host rect
        // position so task-scene content maps into the task surface.
        let view_transform = Mat4f::translation(vec3(
            -(task_host_rect.pos.x as f32),
            -(task_host_rect.pos.y as f32),
            0.0,
        ));
        surface
            .draw_list
            .set_view_transform_self_only(cx.cx, &view_transform);
        cx.begin_root_turtle_for_pass(Layout::default());
        match &task.kind {
            MpBrowserTaskKind::Scene(task_scene) => {
                self.draw_scene_inner(cx, task_scene, prepared_text);
            }
            MpBrowserTaskKind::Blur { input, radius } => {
                if let Some(texture) = self.render_task(cx, scene, prepared_text, *input) {
                    self.draw_task_texture.blur_radius = *radius;
                    self.draw_task_texture.opacity = 1.0;
                    self.draw_task_texture.tex_size = vec2(task.size.x as f32, task.size.y as f32);
                    self.draw_task_texture.draw_super.draw_vars.set_texture(0, &texture);
                    self.draw_task_texture.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(0.0, 0.0),
                            size: task.size,
                        },
                    );
                }
            }
        }
        cx.end_pass_sized_turtle();
        surface.draw_list.end(cx);
        surface.surface.end(cx);
    }
}

pub(super) fn task_host_rect(task: &MpBrowserTask) -> Rect {
    match &task.kind {
        MpBrowserTaskKind::Scene(scene) => scene.host_rect,
        MpBrowserTaskKind::Blur { .. } => Rect {
            pos: dvec2(0.0, 0.0),
            size: task.size,
        },
    }
}

pub(super) fn pixel_area(size: DVec2) -> u64 {
    let width = size.x.max(0.0).ceil() as u64;
    let height = size.y.max(0.0).ceil() as u64;
    width.saturating_mul(height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::MpBlendMode;

    #[test]
    fn scene_tasks_use_nested_host_rect_while_blurs_use_zero_origin() {
        let scene_task = MpBrowserTask {
            size: dvec2(120.0, 80.0),
            cache_key: None,
            kind: MpBrowserTaskKind::Scene(Box::new(MpBrowserScene::new(Rect {
                pos: dvec2(30.0, 40.0),
                size: dvec2(120.0, 80.0),
            }))),
        };
        let blur_task = MpBrowserTask {
            size: dvec2(120.0, 80.0),
            cache_key: None,
            kind: MpBrowserTaskKind::Blur {
                input: 0,
                radius: 4.0,
            },
        };

        assert_eq!(task_host_rect(&scene_task).pos, dvec2(30.0, 40.0));
        assert_eq!(task_host_rect(&scene_task).size, dvec2(120.0, 80.0));
        assert_eq!(task_host_rect(&blur_task).pos, dvec2(0.0, 0.0));
        assert_eq!(task_host_rect(&blur_task).size, dvec2(120.0, 80.0));
    }

    #[test]
    fn text_run_task_scene_reroots_by_own_host_rect() {
        let mut outer_scene = MpBrowserScene::new(Rect {
            pos: dvec2(100.0, 200.0),
            size: dvec2(400.0, 300.0),
        });
        let mut nested_scene = MpBrowserScene::new(Rect {
            pos: dvec2(50.0, 60.0),
            size: dvec2(120.0, 80.0),
        });
        nested_scene.push_text_run(super::super::MpBrowserTextRun {
            stable_id: 1,
            local_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(120.0, 20.0),
            },
            transform_id: 0,
            clip_chain_id: 0,
            color: vec4(1.0, 1.0, 1.0, 1.0),
            fonts: Vec::new(),
            glyphs: vec![super::super::MpBrowserGlyphInstance {
                glyph_id: 7,
                font_size_px: 14.0,
                origin: dvec2(5.0, 12.0),
                font_slot: 0,
            }],
            metrics: super::super::MpBrowserTextMetrics::default(),
            decorations: super::super::MpBrowserTextDecorations::default(),
        });
        let task_id = outer_scene.push_task(MpBrowserTask {
            size: dvec2(120.0, 80.0),
            cache_key: None,
            kind: MpBrowserTaskKind::Scene(Box::new(nested_scene)),
        });
        outer_scene.push_picture(super::super::MpBrowserPicture {
            local_rect: Rect {
                pos: dvec2(50.0, 60.0),
                size: dvec2(120.0, 80.0),
            },
            transform_id: 0,
            clip_chain_id: 0,
            task_id,
            opacity: 1.0,
            blend_mode: MpBlendMode::Normal,
        });

        let task_rect = task_host_rect(&outer_scene.tasks[task_id]);
        assert_eq!(task_rect.pos, dvec2(50.0, 60.0));
        assert_eq!(task_rect.size, dvec2(120.0, 80.0));
        assert_ne!(task_rect.pos, outer_scene.host_rect.pos);
    }
}
