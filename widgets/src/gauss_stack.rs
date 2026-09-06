//! Shared Gaussian render passes for windows and layered compositors.
use crate::{
    gauss_view::{GaussBlurSnapshot, GAUSS_VIEW_LEVELS},
    makepad_draw::*,
    window::{DrawGaussDownsample, DrawGaussScene, DrawGaussUpsample},
};
const GAUSS_STACK_LEVELS: usize = GAUSS_VIEW_LEVELS;
const GAUSS_FLOOR_LEVEL: usize = 2;
const GAUSS_SMOOTH_LEVEL_START: usize = 3;

struct GaussSmoothStage {
    pass: DrawPass,
    draw_list: DrawList2d,
    texture: Texture,
}

struct GaussStackLevel {
    pass: DrawPass,
    draw_list: DrawList2d,
    texture: Texture,
    // One tent-upsample per resolution doubling from this level's own size back up to the
    // floor size; the last stage's texture is what the snapshot exposes. Empty for levels
    // at or above the floor resolution.
    smooth_stages: Vec<GaussSmoothStage>,
}

pub(crate) struct GaussStack {
    scene_pass: DrawPass,
    scene_draw_list: DrawList2d,
    scene_texture: Texture,
    _scene_depth_texture: Texture,
    levels: Vec<GaussStackLevel>,
}

fn gauss_fast() -> bool {
    thread_local! { static ON: bool = std::env::var_os("MAKEPAD_GAUSS_FAST").is_some(); }
    ON.with(|v| *v)
}

pub(crate) fn gauss_render_texture_y_flip_for_os(os_type: &OsType) -> f32 {
    match os_type {
        OsType::Android(_) => 1.0,
        _ => 0.0,
    }
}

impl GaussStack {
    pub(crate) fn new(cx: &mut Cx) -> Self {
        let scene_pass = DrawPass::new_with_name(cx, "gauss_scene");
        let scene_draw_list = DrawList2d::new(cx);
        let scene_texture = Self::new_render_texture(cx);
        let scene_depth_texture = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        scene_pass.set_color_texture(
            cx,
            &scene_texture,
            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
        );
        scene_pass.set_depth_texture(cx, &scene_depth_texture, DrawPassClearDepth::ClearWith(1.0));
        scene_pass.set_live_with_parent(cx, true);

        let mut levels = Vec::with_capacity(GAUSS_STACK_LEVELS);
        for index in 0..GAUSS_STACK_LEVELS {
            let pass = DrawPass::new_with_name(cx, &format!("gauss_mip_{index}"));
            pass.set_live_with_parent(cx, true);
            let draw_list = DrawList2d::new(cx);
            let texture = Self::new_render_texture(cx);
            pass.set_color_texture(
                cx,
                &texture,
                DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
            );
            let stage_count = if index >= GAUSS_SMOOTH_LEVEL_START {
                index - GAUSS_FLOOR_LEVEL
            } else {
                0
            };
            let mut smooth_stages = Vec::with_capacity(stage_count);
            for stage in 0..stage_count {
                let smooth_pass =
                    DrawPass::new_with_name(cx, &format!("gauss_smooth_mip_{index}_{stage}"));
                smooth_pass.set_live_with_parent(cx, true);
                let smooth_draw_list = DrawList2d::new(cx);
                let smooth_texture = Self::new_render_texture(cx);
                smooth_pass.set_color_texture(
                    cx,
                    &smooth_texture,
                    DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
                );
                smooth_stages.push(GaussSmoothStage {
                    pass: smooth_pass,
                    draw_list: smooth_draw_list,
                    texture: smooth_texture,
                });
            }
            levels.push(GaussStackLevel {
                pass,
                draw_list,
                texture,
                smooth_stages,
            });
        }

        Self {
            scene_pass,
            scene_draw_list,
            scene_texture,
            _scene_depth_texture: scene_depth_texture,
            levels,
        }
    }

    fn new_render_texture(cx: &mut Cx) -> Texture {
        Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Auto,
                initial: true,
            },
        )
    }

    pub(crate) fn begin_scene(&mut self, cx: &mut Cx2d) {
        cx.make_child_pass(&self.scene_pass);
        cx.begin_pass(&self.scene_pass, None);
        self.scene_draw_list.begin_always(cx);
        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_down());
    }

    pub(crate) fn begin_scene_sized(&mut self, cx: &mut Cx2d, size: Vec2d) {
        let dpi = cx.current_dpi_factor();
        self.scene_pass.set_size(cx, size);
        cx.make_child_pass(&self.scene_pass);
        cx.begin_pass(&self.scene_pass, Some(dpi));
        self.scene_draw_list.begin_always(cx);
        cx.begin_root_turtle(size, Layout::flow_overlay());
    }

    /// Passes in producer-before-consumer order. Layered compositors explicitly
    /// link this chain, so recycled pool IDs cannot reorder texture dependencies.
    pub(crate) fn dependencies(&self, count: usize) -> Vec<DrawPassId> {
        let mut passes = vec![self.scene_pass.draw_pass_id()];
        passes.extend(
            self.levels
                .iter()
                .take(count)
                .map(|l| l.pass.draw_pass_id()),
        );
        for level in self.levels.iter().take(count) {
            passes.extend(level.smooth_stages.iter().map(|s| s.pass.draw_pass_id()));
        }
        passes
    }

    pub(crate) fn end_scene(&mut self, cx: &mut Cx2d) {
        cx.end_pass_sized_turtle();
        self.scene_draw_list.end(cx);
        cx.end_pass(&self.scene_pass);
    }

    pub(crate) fn snapshot(
        &self,
        root_size: Vec2d,
        source_y_flip: f32,
        dpi_factor: f64,
    ) -> GaussBlurSnapshot {
        GaussBlurSnapshot {
            scene_texture: self.scene_texture.clone(),
            mip_textures: self
                .levels
                .iter()
                .map(|level| {
                    if let Some(stage) = level.smooth_stages.last() {
                        stage.texture.clone()
                    } else {
                        level.texture.clone()
                    }
                })
                .collect(),
            source_size: root_size,
            source_y_flip,
            dpi_factor,
        }
    }

    fn level_size(root_size: Vec2d, dpi: f64, index: usize) -> Vec2d {
        let min_logical_size = 1.0 / dpi.max(1.0);
        let scale = (1usize << (index + 1)) as f64;
        dvec2(
            (root_size.x / scale).max(min_logical_size),
            (root_size.y / scale).max(min_logical_size),
        )
    }

    pub(crate) fn draw_mip_chain(
        &mut self,
        cx: &mut Cx2d,
        downsample: &mut DrawGaussDownsample,
        root_size: Vec2d,
    ) {
        self.draw_mip_chain_to(cx, downsample, root_size, GAUSS_STACK_LEVELS);
    }

    pub(crate) fn draw_mip_chain_to(
        &mut self,
        cx: &mut Cx2d,
        downsample: &mut DrawGaussDownsample,
        root_size: Vec2d,
        count: usize,
    ) {
        let dpi = cx.current_dpi_factor();
        let mut source_texture = self.scene_texture.clone();

        for (index, level) in self.levels.iter_mut().take(count).enumerate() {
            // MAKEPAD_GAUSS_FAST=1: probe rig — stop the chain early to
            // measure how much of a frame the pass COUNT itself costs.
            if gauss_fast() && index > 3 {
                break;
            }
            let level_size = Self::level_size(root_size, dpi, index);

            level.pass.set_size(cx, level_size);
            cx.make_child_pass(&level.pass);
            cx.begin_pass(&level.pass, Some(dpi));
            level.draw_list.begin_always(cx);

            let pass_size = cx.current_pass_size();
            cx.begin_root_turtle(pass_size, Layout::flow_overlay());
            downsample.draw_vars.set_texture(0, &source_texture);
            downsample.draw_abs(
                cx,
                Rect {
                    pos: dvec2(0.0, 0.0),
                    size: pass_size,
                },
            );
            cx.end_pass_sized_turtle();

            level.draw_list.end(cx);
            cx.end_pass(&level.pass);
            source_texture = level.texture.clone();
        }
    }

    // Re-home each deep mip at the floor resolution: starting from the level's own raw mip,
    // tent-upsample one resolution doubling at a time until the floor size is reached. The
    // progressive doubling matters — a single stretch from 1/64 straight to 1/8 would keep the
    // source's texel lattice; each doubling convolves another tent on top and gaussianizes it.
    pub(crate) fn draw_high_blur_chain(
        &mut self,
        cx: &mut Cx2d,
        upsample: &mut DrawGaussUpsample,
        root_size: Vec2d,
    ) {
        self.draw_high_blur_chain_to(cx, upsample, root_size, GAUSS_STACK_LEVELS);
    }

    pub(crate) fn draw_high_blur_chain_to(
        &mut self,
        cx: &mut Cx2d,
        upsample: &mut DrawGaussUpsample,
        root_size: Vec2d,
        count: usize,
    ) {
        if gauss_fast() {
            return;
        }
        let dpi = cx.current_dpi_factor();
        for index in GAUSS_SMOOTH_LEVEL_START..count.min(self.levels.len()) {
            let level = &mut self.levels[index];
            let mut source_texture = level.texture.clone();
            for (stage_index, stage) in level.smooth_stages.iter_mut().enumerate() {
                let stage_size = Self::level_size(root_size, dpi, index - 1 - stage_index);
                stage.pass.set_size(cx, stage_size);
                cx.make_child_pass(&stage.pass);
                cx.begin_pass(&stage.pass, Some(dpi));
                stage.draw_list.begin_always(cx);

                let pass_size = cx.current_pass_size();
                cx.begin_root_turtle(pass_size, Layout::flow_overlay());
                upsample.draw_vars.set_texture(0, &source_texture);
                upsample.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(0.0, 0.0),
                        size: pass_size,
                    },
                );
                cx.end_pass_sized_turtle();

                stage.draw_list.end(cx);
                cx.end_pass(&stage.pass);
                source_texture = stage.texture.clone();
            }
        }
    }

    pub(crate) fn draw_scene(
        &mut self,
        cx: &mut Cx2d,
        scene: &mut DrawGaussScene,
        root_size: Vec2d,
    ) {
        self.draw_scene_region(
            cx,
            scene,
            root_size,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: root_size,
            },
        );
    }

    pub(crate) fn draw_scene_region(
        &mut self,
        cx: &mut Cx2d,
        scene: &mut DrawGaussScene,
        root_size: Vec2d,
        rect: Rect,
    ) {
        scene.draw_vars.set_uniform(
            cx,
            live_id!(source_offset),
            &[
                (rect.pos.x / root_size.x) as f32,
                (rect.pos.y / root_size.y) as f32,
            ],
        );
        scene.draw_vars.set_uniform(
            cx,
            live_id!(source_scale),
            &[
                (rect.size.x / root_size.x) as f32,
                (rect.size.y / root_size.y) as f32,
            ],
        );
        let source_y_flip = gauss_render_texture_y_flip_for_os(cx.os_type());
        scene
            .draw_vars
            .set_uniform(cx, live_id!(source_y_flip), &[source_y_flip]);
        scene.draw_vars.set_texture(0, &self.scene_texture);
        scene.draw_abs(cx, rect);
    }
}
