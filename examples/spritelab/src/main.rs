//! Working-tree sprite-pass lab (not for commit).
//!
//! The smallest possible reproduction of "a submitted billboard draws
//! nothing": an EMPTY preview world containing one immobile sprite per
//! RECIPE, camera parked in front of the row. No map, no floor snapping,
//! no walkers, no brains, no behaviour table — just [`ScreenInstance`]s
//! handed to the same `draw_scene_full` sprite pass the sandbox uses.
//!
//! The row, left to right (each a different submission recipe):
//!   0. small sheet, whole-texture uv, size.zw = 0   — the asset-ui recipe
//!   1. small sheet, half-window uv, size.zw = sheet — the sandbox barrel recipe
//!   2. big sheet (472x434), small uv window, zw = sheet — the sandbox TROO recipe
//!   3. same as 2 but the uv window MIRRORED (u0 > u1)
//!   4. same as 2 but size.zw = 0 (crisp-texel ramp off)
//!   5. same as 1 but quad yaw + PI (facing away — the backface question)
//!
//! Every instance's exact values are logged once (`SPRITELAB case ...`), so
//! the log alone says what the GPU was handed.

use makepad_draw::*;
use makepad_render::{
    preview_scene_state, set_pass_camera, DrawSceneAlpha, DrawSceneCube, DrawSceneScreen,
    DrawSceneSky, DrawSceneTexture, DrawSceneTerrain, PreviewLook, PreviewStage, Renderer,
    SceneDraws, ScreenInstance,
};
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    mod.widgets.SpriteLabBase = #(SpriteLab::register_widget(vm))
    mod.widgets.SpriteLab = set_type_default() do mod.widgets.SpriteLabBase{
        width: Fill
        height: Fill
    }

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(960, 540)
                body +: {
                    lab := SpriteLab{}
                }
            }
        }
    }
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::script_mod(vm);
        makepad_render::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

/// The small sheet (46x32, the barrel shape): left half RED, right half
/// YELLOW, fully opaque — the cutout test cannot hide a single texel.
fn make_small_sheet(cx: &mut Cx) -> Texture {
    let (w, h) = (46usize, 32usize);
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let p = (y * w + x) * 4;
            if x < w / 2 {
                rgba[p..p + 4].copy_from_slice(&[255, 0, 0, 255]);
            } else {
                rgba[p..p + 4].copy_from_slice(&[255, 255, 0, 255]);
            }
        }
    }
    ImageBuffer::new(&rgba, w, h)
        .expect("small sheet")
        .into_new_texture(cx)
}

/// The big sheet (472x434, the troo shape): TRANSPARENT everywhere except
/// an opaque GREEN block exactly under the troo walk-frame uv window
/// (u 0.250..0.333, v 0.143..0.281), an opaque BLUE block one cell to the
/// right, and an opaque MAGENTA border ring for whole-texture sampling.
fn make_big_sheet(cx: &mut Cx) -> Texture {
    let (w, h) = (472usize, 434usize);
    let mut rgba = vec![0u8; w * h * 4];
    let mut fill = |x0: usize, x1: usize, y0: usize, y1: usize, c: [u8; 4]| {
        for y in y0..y1.min(h) {
            for x in x0..x1.min(w) {
                let p = (y * w + x) * 4;
                rgba[p..p + 4].copy_from_slice(&c);
            }
        }
    };
    // Border ring.
    fill(0, w, 0, 8, [255, 0, 255, 255]);
    fill(0, w, h - 8, h, [255, 0, 255, 255]);
    fill(0, 8, 0, h, [255, 0, 255, 255]);
    fill(w - 8, w, 0, h, [255, 0, 255, 255]);
    // The troo walk rot1 frame window: u 0.250..0.333 -> x 118..157,
    // v 0.143..0.281 -> y 62..122.
    fill(118, 158, 62, 122, [0, 200, 0, 255]);
    // One cell right of it (for a second window if wanted).
    fill(177, 218, 62, 120, [0, 90, 255, 255]);
    ImageBuffer::new(&rgba, w, h)
        .expect("big sheet")
        .into_new_texture(cx)
}

#[derive(Script, ScriptHook, Widget)]
pub struct SpriteLab {
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_cube: DrawSceneCube,
    #[live]
    draw_alpha: DrawSceneAlpha,
    #[live]
    draw_sky: DrawSceneSky,
    #[live]
    draw_terrain: DrawSceneTerrain,
    #[live]
    draw_screen: DrawSceneScreen,
    #[redraw]
    #[live]
    draw_bg: DrawSceneTexture,
    #[new]
    pass: DrawPass,
    #[new]
    draw_list: DrawList,
    #[new]
    pass_list: DrawList,
    #[new]
    color_texture: Texture,
    #[new]
    depth_texture: Texture,
    #[rust]
    renderer: Renderer,
    #[rust]
    area: Area,
    #[rust]
    small: Option<Texture>,
    #[rust]
    big: Option<Texture>,
    #[rust(false)]
    initialized: bool,
    #[rust(false)]
    printed: bool,
}

impl SpriteLab {
    fn build_instances(&self) -> Vec<ScreenInstance> {
        let small = self.small.clone().expect("small sheet");
        let big = self.big.clone().expect("big sheet");
        // The troo probe's exact numbers.
        let troo_uv = vec4(0.250, 0.143, 0.333, 0.281);
        let troo_uv_mirrored = vec4(0.333, 0.143, 0.250, 0.281);
        let quad = |x: f32, yaw: f32, tex: &Texture, uv: Vec4f, zw: Vec2f| ScreenInstance {
            texture: tex.clone(),
            pos: vec4(x, 0.8, 0.0, yaw),
            size: vec4(1.0, 1.2, zw.x, zw.y),
            uv,
            tint: vec4(1.0, 1.0, 1.0, 1.0),
            color_adjust: vec4(0.0, 1.0, 1.0, 0.0),
        };
        vec![
            // 0: asset-ui recipe (control).
            quad(-3.75, 0.0, &small, vec4(0.0, 0.0, 1.0, 1.0), vec2f(0.0, 0.0)),
            // 1: sandbox barrel recipe.
            quad(-2.25, 0.0, &small, vec4(0.0, 0.0, 0.5, 1.0), vec2f(46.0, 32.0)),
            // 2: sandbox troo recipe.
            quad(-0.75, 0.0, &big, troo_uv, vec2f(472.0, 434.0)),
            // 3: troo recipe, mirrored pair (u0 > u1).
            quad(0.75, 0.0, &big, troo_uv_mirrored, vec2f(472.0, 434.0)),
            // 4: troo recipe with the crisp-texel ramp OFF.
            quad(2.25, 0.0, &big, troo_uv, vec2f(0.0, 0.0)),
            // 5: barrel recipe facing AWAY (the backface question).
            quad(
                3.75,
                std::f32::consts::PI,
                &small,
                vec4(0.0, 0.0, 0.5, 1.0),
                vec2f(46.0, 32.0),
            ),
        ]
    }
}

impl Widget for SpriteLab {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return DrawStep::done();
        }
        if !self.initialized {
            self.initialized = true;
            self.color_texture = Texture::new_with_format(
                cx.cx,
                TextureFormat::RenderBGRAu8 {
                    size: TextureSize::Auto,
                    initial: true,
                },
            );
            self.depth_texture = Texture::new_with_format(
                cx.cx,
                TextureFormat::DepthD32 {
                    size: TextureSize::Auto,
                    initial: true,
                },
            );
            self.pass.set_color_texture(
                cx.cx,
                &self.color_texture,
                DrawPassClearColor::ClearWith(vec4(0.05, 0.06, 0.09, 1.0)),
            );
            self.pass.set_depth_texture(
                cx.cx,
                &self.depth_texture,
                DrawPassClearDepth::ClearWith(1.0),
            );
            self.small = Some(make_small_sheet(cx.cx));
            self.big = Some(make_big_sheet(cx.cx));
        }
        self.pass.set_size(cx, rect.size);
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, None);
        let look = PreviewLook {
            target: vec3f(0.0, 0.8, 0.0),
            distance: 7.0,
            fov: 45.0,
            yaw: 0.0,
            pitch: 0.0,
        };
        if let Some(scene_state) = preview_scene_state(look, rect, cx.time()) {
            set_pass_camera(cx.cx, &self.pass, &scene_state);
            let cx3d = &mut Cx3d::new(cx.cx);
            self.pass_list.begin_always(cx3d);
            let instances = self.build_instances();
            if !self.printed {
                self.printed = true;
                for (i, inst) in instances.iter().enumerate() {
                    log!(
                        "SPRITELAB case {i}: pos({:.2},{:.2},{:.2}) yaw {:.3} quad {:.2}x{:.2} sheet_zw {}x{} uv({:.3},{:.3},{:.3},{:.3})",
                        inst.pos.x,
                        inst.pos.y,
                        inst.pos.z,
                        inst.pos.w,
                        inst.size.x,
                        inst.size.y,
                        inst.size.z,
                        inst.size.w,
                        inst.uv.x,
                        inst.uv.y,
                        inst.uv.z,
                        inst.uv.w
                    );
                }
                log!(
                    "SPRITELAB camera at (0.00,0.80,7.00) looking -z, fov 45; row at z=0, quads 1.0x1.2"
                );
            }
            self.renderer.set_models(Vec::new());
            let mut draws = SceneDraws {
                cube: &mut self.draw_cube,
                alpha: &mut self.draw_alpha,
                sky: &mut self.draw_sky,
                sky_analytic: None,
                terrain: &mut self.draw_terrain,
                shadow: None,
                shadow_sdf: None,
                firework: None,
                flare: None,
                water: None,
                screen: Some(&mut self.draw_screen),
                screen_instances: &instances,
                view_model: None,
            };
            let stage = PreviewStage {
                ground: false,
                sky: true,
                ground_half: 8.0,
                ground_color: vec4(0.0, 0.0, 0.0, 1.0),
                dark: false,
            };
            self.renderer.draw_preview(
                cx3d,
                &mut self.draw_list,
                &mut draws,
                look,
                stage,
                scene_state,
                None,
                None,
            );
            self.pass_list.end(cx3d);
        }
        cx.end_pass(&self.pass);
        self.draw_bg.draw_vars.set_texture(0, &self.color_texture);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        cx.set_pass_area(&self.pass, self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}
