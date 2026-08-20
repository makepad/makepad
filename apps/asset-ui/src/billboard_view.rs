//! 3D camera-facing viewer for a stateful billboard.
//!
//! The sprite is an unlit, nearest-sampled, alpha-tested quad (Doom/Quake
//! pixels, not a PBR statue). Orbit/zoom the camera; the quad yaws to face
//! it. Named animation states are listed at the bottom — click one to play
//! it. Preview always loops so walk/attack/pain can be inspected.

use makepad_render::{
    preview_scene_state, set_pass_camera, DrawSceneAlpha, DrawSceneCube,
    DrawSceneScreen, DrawSceneShadow, DrawSceneSkinned, DrawSceneSky, DrawSceneTerrain, DrawSceneTexture,
    PreviewLook, PreviewStage, SceneDraws, Renderer,
};
use makepad_asset_importer::stateful_billboard::{SpriteFrame, StatefulBillboard};
use makepad_widgets::*;
use std::path::Path;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.BillboardViewBase = #(BillboardView::register_widget(vm))
    mod.widgets.BillboardView = set_type_default() do mod.widgets.BillboardViewBase{
        width: Fill
        height: Fill
        draw_cube +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
        draw_alpha +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
        draw_terrain +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
        draw_models +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
        // Unlit + nearest + discard keyed texels. The stock screen shader
        // forces alpha 1 and bilinear-samples, which is why sprites looked
        // like a dark cardboard cutout.
        draw_sprite +: {
            alpha_blend: true
            backface_culling: false
            pixel: fn() {
                let color = self.tex.sample_nearest(self.v_uv)
                if color.w < 0.08 {
                    discard()
                }
                return vec4(color.x, color.y, color.z, color.w)
            }
        }
        draw_hud +: {
            text_style: theme.font_regular{font_size: 9}
            color: #xffffffcc
        }
        draw_state +: {
            text_style: theme.font_bold{font_size: 9}
            color: #xc6cfd8
        }
        draw_state_on +: {
            text_style: theme.font_bold{font_size: 9}
            color: #x7db8f0
        }
    }
}

#[derive(Clone)]
struct BbFrame {
    texture: Texture,
    w: u32,
    h: u32,
    flip: bool,
}

struct BbState {
    name: String,
    fps: f32,
    /// `by_rot[0]` = omnidirectional; `by_rot[1..=facings]` = view.
    by_rot: Vec<Vec<BbFrame>>,
}

impl BbState {
    fn frames_for(&self, facing: u8) -> &[BbFrame] {
        let i = facing as usize;
        if let Some(frames) = self.by_rot.get(i) {
            if !frames.is_empty() {
                return frames;
            }
        }
        if let Some(frames) = self.by_rot.first() {
            if !frames.is_empty() {
                return frames;
            }
        }
        self.by_rot.get(1).map(Vec::as_slice).unwrap_or(&[])
    }
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct BillboardView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_bg: DrawSceneTexture,
    #[live]
    draw_cube: DrawSceneCube,
    #[live]
    draw_alpha: DrawSceneAlpha,
    #[live]
    draw_sky: DrawSceneSky,
    #[live]
    draw_terrain: DrawSceneTerrain,
    #[live]
    draw_shadow: DrawSceneShadow,
    #[live]
    draw_models: DrawSceneSkinned,
    #[live]
    draw_sprite: DrawSceneScreen,
    #[live]
    draw_hud: DrawText,
    #[live]
    draw_state: DrawText,
    #[live]
    draw_state_on: DrawText,
    #[live(vec4(0.03, 0.045, 0.075, 1.0))]
    clear_color: Vec4f,
    #[new]
    pass: DrawPass,
    #[new]
    draw_list: DrawList,
    #[new]
    color_texture: Texture,
    #[new]
    depth_texture: Texture,
    #[rust]
    area: Area,
    #[rust(false)]
    initialized: bool,
    #[rust(false)]
    world_built: bool,
    #[rust]
    renderer: Renderer,
    #[rust]
    look: PreviewLook,
    #[rust]
    states: Vec<BbState>,
    #[rust(0usize)]
    state_i: usize,
    #[rust(0usize)]
    frame_i: usize,
    #[rust]
    frame_accum: f64,
    #[rust]
    last_time: Option<f64>,
    #[rust]
    status: String,
    #[rust(0.0f32)]
    orbit_yaw: f32,
    #[rust(1u8)]
    facings: u8,
    #[rust(-0.12f32)]
    orbit_pitch: f32,
    #[rust]
    orbit_last_abs: Option<DVec2>,
    #[rust]
    view_rect: Rect,
    #[rust]
    chip_rects: Vec<Rect>,
    #[rust]
    next_frame: NextFrame,
    /// World metres per authored texel. Locked from the standing
    /// (walk/idle) frame so a wide death splat does not grow to 1.55m tall.
    #[rust(0.02f32)]
    px_scale: f32,
}

const STAND_HEIGHT: f32 = 1.55;

fn viewer_state_name(name: &str) -> bool {
    // pose_a / pose_b are just one letter from the sheet (walk frame A,
    // walk frame B, …), not authored poses. Keep walk/attack/pain/death.
    !name.starts_with("pose_")
}

fn pixel_scale_from_stand(stand_h_px: u32) -> f32 {
    STAND_HEIGHT / stand_h_px.max(1) as f32
}

fn frame_world_size(w: u32, h: u32, px: f32) -> (f32, f32) {
    ((w as f32 * px).max(0.04), (h as f32 * px).max(0.04))
}

/// Frame pixels of a stateful billboard, decoded once per source file.
///
/// A packed-sheet manifest (`sheet <cols> <cell_w> <cell_h>` + `cell <n>`
/// per frame) names ONE PNG for every frame: it is decoded once and each
/// frame is cut out of its cell at the authored size. A legacy manifest
/// still reads one PNG per frame. Both hand back the same pixels, so every
/// viewer plays a sheet-backed actor exactly as it played loose frames.
pub struct BillboardFrames<'a> {
    manifest: &'a Path,
    bb: &'a StatefulBillboard,
    decoded: Vec<(String, Option<ImageBuffer>)>,
}

impl<'a> BillboardFrames<'a> {
    pub fn new(manifest: &'a Path, bb: &'a StatefulBillboard) -> Self {
        Self {
            manifest,
            bb,
            decoded: Vec::new(),
        }
    }

    /// Decoded pixels of one frame, or `None` when its file is missing.
    pub fn image(&mut self, frame: &SpriteFrame) -> Option<ImageBuffer> {
        if !self.decoded.iter().any(|(f, _)| *f == frame.file) {
            let path = self.bb.resolve_frame(self.manifest, frame);
            let image = std::fs::read(&path)
                .ok()
                .and_then(|bytes| ImageBuffer::from_png(&bytes).ok());
            self.decoded.push((frame.file.clone(), image));
        }
        let source = self
            .decoded
            .iter()
            .find(|(f, _)| *f == frame.file)
            .and_then(|(_, i)| i.as_ref())?;
        match self.bb.frame_rect(frame) {
            Some(rect) => cut_cell(source, rect),
            None => Some(source.clone()),
        }
    }
}

/// Copy `(x, y, w, h)` out of a decoded sheet. Refuses a rect that leaves
/// the sheet rather than smearing whatever follows it in memory.
fn cut_cell(sheet: &ImageBuffer, rect: (u32, u32, u32, u32)) -> Option<ImageBuffer> {
    let (x, y, w, h) = (
        rect.0 as usize,
        rect.1 as usize,
        rect.2 as usize,
        rect.3 as usize,
    );
    if w == 0 || h == 0 || x + w > sheet.width || y + h > sheet.height {
        return None;
    }
    if sheet.data.len() < sheet.width * sheet.height {
        return None;
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let start = (y + row) * sheet.width + x;
        for &p in &sheet.data[start..start + w] {
            rgba.push(((p >> 16) & 0xFF) as u8);
            rgba.push(((p >> 8) & 0xFF) as u8);
            rgba.push((p & 0xFF) as u8);
            rgba.push(((p >> 24) & 0xFF) as u8);
        }
    }
    ImageBuffer::new(&rgba, w, h).ok()
}

impl BillboardView {
    pub fn load_manifest(&mut self, cx: &mut Cx, path: &Path) {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let Ok(bb) = StatefulBillboard::parse(&text) else {
            self.states.clear();
            self.status = "not a stateful billboard".into();
            self.area.redraw(cx);
            return;
        };
        let mut pixels = BillboardFrames::new(path, &bb);
        self.facings = bb.resolved_facings();
        let mut states = Vec::new();
        let names: Vec<String> = if bb.states.is_empty() {
            vec![bb.preview.clone()]
        } else {
            bb.states.iter().map(|s| s.name.clone()).collect()
        };
        for name in names {
            if !viewer_state_name(&name) {
                continue;
            }
            let nrot = self.facings.max(1) as usize;
            let mut by_rot = vec![Vec::new(); nrot + 1];
            for rot in 0..=self.facings {
                for faced in bb.frames_for_state_facing(&name, rot) {
                    if let Some(image) = pixels.image(faced.frame) {
                        let w = image.width.max(1) as u32;
                        let h = image.height.max(1) as u32;
                        by_rot[rot as usize].push(BbFrame {
                            texture: image.into_new_texture(cx),
                            w: faced.frame.w.max(w).max(1),
                            h: faced.frame.h.max(h).max(1),
                            flip: faced.flip,
                        });
                    }
                }
            }
            if by_rot.iter().all(|f| f.is_empty()) {
                continue;
            }
            states.push(BbState {
                fps: bb.state_fps(&name) as f32,
                name,
                by_rot,
            });
        }
        let start = states
            .iter()
            .position(|s| s.name == bb.preview)
            .unwrap_or(0);
        let stand_h = states
            .iter()
            .find(|s| matches!(s.name.as_str(), "walk" | "idle" | "ready" | "see"))
            .or_else(|| states.first())
            .and_then(|s| {
                s.by_rot
                    .iter()
                    .flat_map(|f| f.iter())
                    .map(|f| f.h)
                    .max()
            })
            .unwrap_or(56);
        self.px_scale = pixel_scale_from_stand(stand_h);
        self.states = states;
        self.state_i = start;
        self.frame_i = 0;
        self.frame_accum = 0.0;
        self.last_time = None;
        self.status = if self.states.is_empty() {
            "billboard has no frames".into()
        } else {
            format!(
                "{} · {} states · click a state · drag orbit",
                bb.prefix.to_ascii_uppercase(),
                self.states.len()
            )
        };
        self.next_frame = cx.new_next_frame();
        self.area.redraw(cx);
    }

    fn current_frame(&self) -> Option<&BbFrame> {
        let state = self.states.get(self.state_i)?;
        let facing = makepad_asset_importer::stateful_billboard::StatefulBillboard::facing_for_yaw(
            self.orbit_yaw,
            self.facings,
        );
        let frames = state.frames_for(facing);
        if frames.is_empty() {
            return None;
        }
        frames.get(self.frame_i % frames.len())
    }

    fn set_state(&mut self, cx: &mut Cx, index: usize) {
        if index >= self.states.len() || index == self.state_i {
            return;
        }
        self.state_i = index;
        self.frame_i = 0;
        self.frame_accum = 0.0;
        self.area.redraw(cx);
    }

    fn ensure_initialized(&mut self, cx: &mut Cx) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.color_texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        self.depth_texture = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(self.clear_color),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        cx.passes[self.pass.draw_pass_id()].keep_camera_matrix = true;
    }

    fn ensure_world(&mut self) {
        if self.world_built {
            return;
        }
        self.world_built = true;
        self.look.target = vec3f(0.0, 0.7, 0.0);
        self.look.distance = 2.6;
        self.look.fov = 45.0;
    }

    fn pose_sprite(&mut self) {
        let Some((texture, w, h, flip)) = self
            .current_frame()
            .map(|f| (f.texture.clone(), f.w, f.h, f.flip))
        else {
            self.draw_sprite.screen_size = vec4(0.0, 0.0, 0.0, 0.0);
            return;
        };
        let (width, height) = frame_world_size(w, h, self.px_scale);
        self.draw_sprite.draw_vars.set_texture(0, &texture);
        // Constant texel scale: a short wide death frame stays short, not
        // stretched back up to stand height. Feet on the slab.
        // Negative width X-flips Duke/Doom mirrored side views.
        let width = if flip { -width } else { width };
        self.draw_sprite.screen_pos = vec4(0.0, height * 0.5, 0.0, self.orbit_yaw);
        self.draw_sprite.screen_size = vec4(width, height, 0.0, 0.0);
        self.draw_sprite.depth_clip = 1.0;
    }

    fn tick_anim(&mut self, now: f64) -> bool {
        let Some(state) = self.states.get(self.state_i) else {
            return false;
        };
        let facing = makepad_asset_importer::stateful_billboard::StatefulBillboard::facing_for_yaw(
            self.orbit_yaw,
            self.facings,
        );
        let n = state.frames_for(facing).len();
        if n < 2 {
            return false;
        }
        let last = self.last_time.replace(now).unwrap_or(now);
        self.frame_accum += (now - last).min(0.25);
        let step = 1.0 / state.fps.max(1.0) as f64;
        let mut changed = false;
        while self.frame_accum >= step {
            self.frame_accum -= step;
            // Preview always loops — attack/pain/death are one-shots in the
            // manifest for the game, but the viewer has to keep playing.
            self.frame_i = (self.frame_i + 1) % n;
            changed = true;
        }
        changed
    }
}

impl WidgetNode for BillboardView {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl Widget for BillboardView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            if self.tick_anim(cx.seconds_since_app_start()) {
                self.area.redraw(cx);
            }
            if !self.states.is_empty() {
                self.next_frame = cx.new_next_frame();
            }
        }
        // Raw mouse only while orbit-dragging. Clicks start through hits()
        // so a dropdown over/near this pane keeps its FingerDown.
        if self.orbit_last_abs.is_some() {
            match event {
                Event::MouseMove(me) => {
                    if let Some(last) = self.orbit_last_abs {
                        let delta = me.abs - last;
                        self.orbit_yaw -= delta.x as f32 * 0.01;
                        self.orbit_pitch =
                            (self.orbit_pitch + delta.y as f32 * 0.01).clamp(-1.2, 1.2);
                        self.orbit_last_abs = Some(me.abs);
                        self.area.redraw(cx);
                    }
                }
                Event::MouseUp(me) if me.button.is_primary() => {
                    self.orbit_last_abs = None;
                }
                _ => {}
            }
        }

        match event.hits(cx, self.area) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                let mut hit_chip = false;
                for (i, rect) in self.chip_rects.iter().enumerate() {
                    if rect.contains(fe.abs) {
                        self.set_state(cx, i);
                        hit_chip = true;
                        break;
                    }
                }
                if !hit_chip {
                    self.orbit_last_abs = Some(fe.abs);
                    cx.set_cursor(MouseCursor::Grabbing);
                }
            }
            Hit::FingerScroll(se) => {
                let axis = if se.scroll.y.abs() > f64::EPSILON {
                    se.scroll.y
                } else {
                    se.scroll.x
                };
                if axis.abs() > f64::EPSILON {
                    let factor = if axis > 0.0 { 1.0 / 0.92 } else { 0.92 };
                    self.look.distance =
                        (self.look.distance * factor).clamp(1.2, 12.0);
                    self.area.redraw(cx);
                }
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Grab);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return DrawStep::done();
        }
        self.ensure_initialized(cx.cx);
        self.ensure_world();
        self.pose_sprite();
        self.view_rect = rect;
        self.pass.set_size(cx, rect.size);
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(self.clear_color),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));

        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, None);
        self.look.yaw = self.orbit_yaw;
        self.look.pitch = self.orbit_pitch;
        let scene_state = preview_scene_state(self.look, rect, cx.time());
        if let Some(scene_state) = scene_state {
            set_pass_camera(cx.cx, &self.pass, &scene_state);
            let cx3d = &mut Cx3d::new(cx.cx);
            self.renderer.set_models(Vec::new());
            let mut draws = SceneDraws {
                cube: &mut self.draw_cube,
                alpha: &mut self.draw_alpha,
                sky: &mut self.draw_sky,
                sky_analytic: None,
                terrain: &mut self.draw_terrain,
                shadow: Some(&mut self.draw_shadow),
                shadow_sdf: None,
                firework: None,
                flare: None,
                water: None,
                screen: Some(&mut self.draw_sprite),
                screen_instances: &[],
                view_model: None,
            };
            let mut stage = PreviewStage::statue();
            stage.ground_half = 6.0;
            stage.ground_color = vec4(0.22, 0.24, 0.28, 1.0);
            self.renderer.draw_preview(
                cx3d,
                &mut self.draw_list,
                &mut draws,
                self.look,
                stage,
                scene_state,
                None,
                Some(&mut self.draw_models),
            );
        }
        cx.end_pass(&self.pass);

        self.draw_bg.draw_vars.set_texture(0, &self.color_texture);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        cx.set_pass_area(&self.pass, self.area);

        self.chip_rects.clear();
        let mut x = rect.pos.x + 10.0;
        let y = rect.pos.y + rect.size.y - 22.0;
        for (i, state) in self.states.iter().enumerate() {
            let label = if i == self.state_i {
                format!("[{}]", state.name)
            } else {
                state.name.clone()
            };
            let w = (label.len() as f64) * 6.4 + 8.0;
            let chip = Rect {
                pos: dvec2(x, y - 2.0),
                size: dvec2(w, 16.0),
            };
            self.chip_rects.push(chip);
            if i == self.state_i {
                self.draw_state_on.draw_abs(cx, dvec2(x, y), &label);
            } else {
                self.draw_state.draw_abs(cx, dvec2(x, y), &label);
            }
            x += w + 8.0;
        }
        if self.states.is_empty() {
            self.draw_hud.draw_abs(cx, dvec2(rect.pos.x + 10.0, y), &self.status);
        }
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_hides_sheet_letter_poses() {
        assert!(viewer_state_name("walk"));
        assert!(viewer_state_name("attack"));
        assert!(viewer_state_name("idle"));
        assert!(!viewer_state_name("pose_a"));
        assert!(!viewer_state_name("pose_b"));
    }

    #[test]
    fn death_frame_keeps_stand_texel_scale() {
        let px = pixel_scale_from_stand(56);
        let (sw, sh) = frame_world_size(40, 56, px);
        assert!((sh - STAND_HEIGHT).abs() < 0.01, "{sh}");
        let (dw, dh) = frame_world_size(80, 20, px);
        // Wide splat: wider, much shorter — not a 1.55m-tall billboard.
        assert!(dw > sw);
        assert!(dh < sh * 0.5);
        assert!(dh < STAND_HEIGHT * 0.5);
    }

    fn sheet_scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mp-bbview-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 2 cells of 4x6, cell 0 red, cell 1 green, padding transparent.
    fn write_two_cell_sheet(path: &std::path::Path) {
        let (w, h) = (8usize, 6usize);
        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let color = match (x < 4, x >= 4) {
                    (true, _) => [255u8, 0, 0, 255],
                    (_, true) => [0, 255, 0, 255],
                    _ => [0, 0, 0, 0],
                };
                rgba[i..i + 4].copy_from_slice(&color);
            }
        }
        let png =
            makepad_asset_importer::classic_import::encode_png_rgba(&rgba, w as u32, h as u32)
                .unwrap();
        std::fs::write(path, png).unwrap();
    }

    #[test]
    fn sheet_frames_are_cut_out_of_their_cells() {
        let dir = sheet_scratch("cells");
        write_two_cell_sheet(&dir.join("troo.png"));
        let manifest = dir.join("troo.billboard");
        std::fs::write(
            &manifest,
            "stateful-billboard 1\n\
             prefix troo\n\
             role character\n\
             preview walk\n\
             sheet 2 4 6\n\
             state walk 0 2 1 8\n\
             frame 0 A 1 4 6 troo.png cell 0\n\
             frame 1 B 1 3 5 troo.png cell 1\n",
        )
        .unwrap();
        let text = std::fs::read_to_string(&manifest).unwrap();
        let bb = StatefulBillboard::parse(&text).unwrap();
        let mut pixels = BillboardFrames::new(&manifest, &bb);
        let a = pixels.image(&bb.frames[0]).expect("cell 0");
        assert_eq!((a.width, a.height), (4, 6));
        assert_eq!(a.data[0] & 0x00ff_ffff, 0x00ff_0000, "cell 0 is red");
        // A frame smaller than its cell keeps its authored size, top-left.
        let b = pixels.image(&bb.frames[1]).expect("cell 1");
        assert_eq!((b.width, b.height), (3, 5));
        assert_eq!(b.data[0] & 0x00ff_ffff, 0x0000_ff00, "cell 1 is green");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_frames_still_decode_whole_files() {
        let dir = sheet_scratch("legacy");
        write_two_cell_sheet(&dir.join("trooa1.png"));
        let manifest = dir.join("troo.billboard");
        std::fs::write(
            &manifest,
            "stateful-billboard 1\n\
             prefix troo\n\
             role character\n\
             preview walk\n\
             state walk 0 1 1 8\n\
             frame 0 A 1 8 6 trooa1.png\n",
        )
        .unwrap();
        let text = std::fs::read_to_string(&manifest).unwrap();
        let bb = StatefulBillboard::parse(&text).unwrap();
        let mut pixels = BillboardFrames::new(&manifest, &bb);
        let a = pixels.image(&bb.frames[0]).expect("whole png");
        assert_eq!((a.width, a.height), (8, 6), "no sheet header, no cropping");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cell_outside_the_sheet_is_refused() {
        let dir = sheet_scratch("outside");
        write_two_cell_sheet(&dir.join("troo.png"));
        let manifest = dir.join("troo.billboard");
        std::fs::write(
            &manifest,
            "stateful-billboard 1\n\
             prefix troo\n\
             role character\n\
             preview walk\n\
             sheet 2 4 6\n\
             state walk 0 1 1 8\n\
             frame 0 A 1 4 6 troo.png cell 9\n",
        )
        .unwrap();
        let text = std::fs::read_to_string(&manifest).unwrap();
        let bb = StatefulBillboard::parse(&text).unwrap();
        let mut pixels = BillboardFrames::new(&manifest, &bb);
        assert!(
            pixels.image(&bb.frames[0]).is_none(),
            "smeared pixels are worse than a missing frame"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
