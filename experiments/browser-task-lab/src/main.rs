pub use makepad_browser_scene;
pub use makepad_widgets;

use makepad_browser_scene::{
    MpBlendMode, MpBrowserRenderer, MpDocument, MpDocumentId, MpEffectNode, MpEmbed, MpIsolation,
    MpPerCornerRadius, MpPipelineId, MpPrimitive, MpPrimitiveId, MpScene, MpSceneId,
};
use makepad_widgets::*;

app_main!(App);

const BG: Vec4f = Vec4f {
    x: 0.06,
    y: 0.08,
    z: 0.11,
    w: 1.0,
};
const PANEL_W: f64 = 260.0;
const PANEL_H: f64 = 200.0;
const GAP_X: f64 = 26.0;
const GAP_Y: f64 = 28.0;
const ROOT_PAD: f64 = 28.0;

script_mod! {
    use mod.prelude.widgets.*

    let BrowserTaskLabBase = #(BrowserTaskLab::register_widget(vm))
    let BrowserTaskLab = set_type_default() do BrowserTaskLabBase{
        width: Fill
        height: Fill
        draw_bg: mod.draw.DrawColor{color: #(BG)}
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1780, 1180)
                pass.clear_color: #(BG)
                body +: {
                    lab := BrowserTaskLab{}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_browser_scene::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct BrowserTaskLab {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[rust]
    renderer: Option<MpBrowserRenderer>,
    #[rust]
    document: Option<MpDocument>,
    #[rust]
    viewport_size: DVec2,
    #[rust]
    logged: bool,
}

impl Widget for BrowserTaskLab {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.renderer.is_none() {
            self.renderer = Some(MpBrowserRenderer::new(cx.cx));
        }
        cx.begin_turtle(walk, self.layout);
        let viewport = Rect {
            pos: dvec2(0.0, 0.0),
            size: cx.current_pass_size(),
        };
        self.draw_bg.draw_abs(cx, viewport);
        if self.document.is_none() || self.viewport_size != viewport.size {
            self.viewport_size = viewport.size;
            self.document = Some(build_document(viewport));
            self.logged = false;
        }
        let renderer = self.renderer.as_mut().unwrap();
        let document = self.document.as_ref().unwrap();
        let stats = renderer
            .draw_document(cx, document, viewport)
            .expect("browser task lab render");
        if !self.logged {
            self.logged = true;
            log!(
                "[browser-task-lab] direct={} isolated_boundaries={} compositor_surfaces={} scratch_surfaces={} offscreen_area={}",
                stats.direct_primitive_count,
                stats.isolated_boundary_count,
                stats.compositor_surface_count,
                stats.scratch_surface_count,
                stats.total_offscreen_pixel_area,
            );
        }
        cx.end_turtle();
        DrawStep::done()
    }
}

fn color(hex: u32) -> Vec4f {
    vec4(
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
        1.0,
    )
}

fn panel_rect(col: usize, row: usize) -> Rect {
    Rect {
        pos: dvec2(
            ROOT_PAD + col as f64 * (PANEL_W + GAP_X),
            ROOT_PAD + row as f64 * (PANEL_H + GAP_Y),
        ),
        size: dvec2(PANEL_W, PANEL_H),
    }
}

fn panel_inner(panel: Rect) -> Rect {
    Rect {
        pos: panel.pos + dvec2(14.0, 14.0),
        size: panel.size - dvec2(28.0, 28.0),
    }
}

fn add_panel(scene: &mut MpScene, panel: Rect, fill: Vec4f) {
    scene.push_primitive(MpPrimitive::rounded_rect(
        MpPrimitiveId(0),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        panel,
        fill,
        MpPerCornerRadius::uniform(14.0),
    ));
    scene.push_primitive(MpPrimitive::border(
        MpPrimitiveId(0),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        panel,
        vec4(1.0, 1.0, 1.0, 0.08),
        1.0,
        MpPerCornerRadius::uniform(14.0),
    ));
}

fn build_document(viewport: Rect) -> MpDocument {
    let mut scene = MpScene::new(MpSceneId(1), viewport);

    let opacity_panel = panel_rect(0, 0);
    add_panel(&mut scene, opacity_panel, color(0x17202b));
    add_opacity_stack(&mut scene, opacity_panel);

    let blur_panel = panel_rect(1, 0);
    add_panel(&mut scene, blur_panel, color(0x1a2330));
    add_blur_group(&mut scene, blur_panel);

    let mixed_panel = panel_rect(2, 0);
    add_panel(&mut scene, mixed_panel, color(0x1a2230));
    add_mixed_order(&mut scene, mixed_panel);

    let nested_panel = panel_rect(0, 1);
    add_panel(&mut scene, nested_panel, color(0x18212d));
    add_nested_embed(&mut scene, nested_panel);

    let clip_panel = panel_rect(1, 1);
    add_panel(&mut scene, clip_panel, color(0x18232f));
    add_blur_clip_combo(&mut scene, clip_panel);

    let plain_panel = panel_rect(2, 1);
    add_panel(&mut scene, plain_panel, color(0x18222f));
    add_direct_reference(&mut scene, plain_panel);

    let mut document = MpDocument::new(MpDocumentId(1), scene);
    document.push_child_document(MpPipelineId(7), build_child_document());
    document
}

fn add_opacity_stack(scene: &mut MpScene, panel: Rect) {
    let inner = panel_inner(panel);
    let effect_id = scene.push_effect(MpEffectNode {
        spatial_id: scene.root_spatial_id,
        clip_chain_id: scene.root_clip_chain_id,
        opacity: 0.55,
        filters: Vec::new(),
        blend_mode: MpBlendMode::Normal,
        isolation: MpIsolation::Isolate,
        mask: None,
    });
    scene.push_primitive(MpPrimitive::solid_rect(
        MpPrimitiveId(0),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        Rect {
            pos: inner.pos + dvec2(8.0, 16.0),
            size: dvec2(90.0, 120.0),
        },
        color(0x33537e),
    ));
    for (index, fill) in [color(0xff7d4d), color(0x4f8cff), color(0x53d48d)]
        .into_iter()
        .enumerate()
    {
        let mut primitive = MpPrimitive::rounded_rect(
            MpPrimitiveId(0),
            scene.root_spatial_id,
            scene.root_clip_chain_id,
            Rect {
                pos: inner.pos + dvec2(36.0 + index as f64 * 26.0, 34.0 + index as f64 * 18.0),
                size: dvec2(120.0, 58.0),
            },
            fill,
            MpPerCornerRadius::uniform(18.0),
        );
        primitive.effect_id = Some(effect_id);
        scene.push_primitive(primitive);
    }
}

fn add_blur_group(scene: &mut MpScene, panel: Rect) {
    let inner = panel_inner(panel);
    let effect_id = scene.push_effect(MpEffectNode {
        spatial_id: scene.root_spatial_id,
        clip_chain_id: scene.root_clip_chain_id,
        opacity: 1.0,
        filters: vec![makepad_browser_scene::MpFilter::Blur(10.0)],
        blend_mode: MpBlendMode::Normal,
        isolation: MpIsolation::Isolate,
        mask: None,
    });
    scene.push_primitive(MpPrimitive::solid_rect(
        MpPrimitiveId(0),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        Rect {
            pos: inner.pos + dvec2(0.0, 70.0),
            size: dvec2(inner.size.x, 22.0),
        },
        color(0x263446),
    ));
    for index in 0..4 {
        let mut primitive = MpPrimitive::rounded_rect(
            MpPrimitiveId(0),
            scene.root_spatial_id,
            scene.root_clip_chain_id,
            Rect {
                pos: inner.pos + dvec2(12.0 + index as f64 * 34.0, 34.0 + (index % 2) as f64 * 26.0),
                size: dvec2(80.0, 42.0),
            },
            [color(0x4f8cff), color(0xff7d4d), color(0x53d48d), color(0xffcf58)][index],
            MpPerCornerRadius::uniform(14.0),
        );
        primitive.effect_id = Some(effect_id);
        scene.push_primitive(primitive);
    }
}

fn add_mixed_order(scene: &mut MpScene, panel: Rect) {
    let inner = panel_inner(panel);
    let effect_id = scene.push_effect(MpEffectNode {
        spatial_id: scene.root_spatial_id,
        clip_chain_id: scene.root_clip_chain_id,
        opacity: 0.75,
        filters: Vec::new(),
        blend_mode: MpBlendMode::Normal,
        isolation: MpIsolation::Isolate,
        mask: None,
    });
    scene.push_primitive(MpPrimitive::rounded_rect(
        MpPrimitiveId(0),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        Rect {
            pos: inner.pos + dvec2(10.0, 18.0),
            size: dvec2(76.0, 138.0),
        },
        color(0x2a394c),
        MpPerCornerRadius::uniform(18.0),
    ));
    let mut isolated = MpPrimitive::rounded_rect(
        MpPrimitiveId(0),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        Rect {
            pos: inner.pos + dvec2(66.0, 34.0),
            size: dvec2(140.0, 74.0),
        },
        color(0xb971ff),
        MpPerCornerRadius::uniform(20.0),
    );
    isolated.effect_id = Some(effect_id);
    scene.push_primitive(isolated);
    scene.push_primitive(MpPrimitive::border(
        MpPrimitiveId(0),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        Rect {
            pos: inner.pos + dvec2(126.0, 54.0),
            size: dvec2(82.0, 92.0),
        },
        color(0xffffff),
        4.0,
        MpPerCornerRadius::uniform(16.0),
    ));
}

fn add_nested_embed(scene: &mut MpScene, panel: Rect) {
    let inner = panel_inner(panel);
    scene.push_primitive(MpPrimitive::rounded_rect(
        MpPrimitiveId(0),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        Rect {
            pos: inner.pos + dvec2(6.0, 8.0),
            size: dvec2(100.0, 146.0),
        },
        color(0x233140),
        MpPerCornerRadius::uniform(16.0),
    ));
    scene.push_embed(MpEmbed {
        scene_id: MpSceneId(77),
        pipeline_id: MpPipelineId(7),
        spatial_id: scene.root_spatial_id,
        clip_chain_id: scene.root_clip_chain_id,
        effect_id: None,
        bounds: Rect {
            pos: inner.pos + dvec2(94.0, 18.0),
            size: dvec2(132.0, 132.0),
        },
        hit_test_tag: None,
    });
}

fn add_blur_clip_combo(scene: &mut MpScene, panel: Rect) {
    let inner = panel_inner(panel);
    let clip = scene.push_clip(makepad_browser_scene::MpClipNode {
        spatial_id: scene.root_spatial_id,
        kind: makepad_browser_scene::MpClipKind::RoundedRect {
            rect: Rect {
                pos: inner.pos + dvec2(18.0, 18.0),
                size: inner.size - dvec2(36.0, 36.0),
            },
            radius: MpPerCornerRadius::uniform(28.0),
        },
    });
    let chain = scene.push_clip_chain(makepad_browser_scene::MpClipChain {
        parent: Some(scene.root_clip_chain_id),
        clips: vec![clip],
    });
    let effect_id = scene.push_effect(MpEffectNode {
        spatial_id: scene.root_spatial_id,
        clip_chain_id: chain,
        opacity: 1.0,
        filters: vec![makepad_browser_scene::MpFilter::Blur(8.0)],
        blend_mode: MpBlendMode::Normal,
        isolation: MpIsolation::Isolate,
        mask: None,
    });
    for index in 0..5 {
        let mut primitive = MpPrimitive::rounded_rect(
            MpPrimitiveId(0),
            scene.root_spatial_id,
            chain,
            Rect {
                pos: inner.pos + dvec2(8.0 + index as f64 * 26.0, 26.0 + index as f64 * 16.0),
                size: dvec2(120.0, 38.0),
            },
            [color(0x4f8cff), color(0xff7d4d), color(0x53d48d), color(0xffcf58), color(0xb971ff)][index],
            MpPerCornerRadius::uniform(14.0),
        );
        primitive.effect_id = Some(effect_id);
        scene.push_primitive(primitive);
    }
}

fn add_direct_reference(scene: &mut MpScene, panel: Rect) {
    let inner = panel_inner(panel);
    for row in 0..3 {
        for col in 0..3 {
            scene.push_primitive(MpPrimitive::rounded_rect(
                MpPrimitiveId(0),
                scene.root_spatial_id,
                scene.root_clip_chain_id,
                Rect {
                    pos: inner.pos + dvec2(16.0 + col as f64 * 62.0, 18.0 + row as f64 * 48.0),
                    size: dvec2(52.0, 38.0),
                },
                [color(0x4f8cff), color(0xff7d4d), color(0x53d48d)][(row + col) % 3],
                MpPerCornerRadius::uniform(12.0),
            ));
        }
    }
}

fn build_child_document() -> MpDocument {
    let viewport = Rect {
        pos: dvec2(0.0, 0.0),
        size: dvec2(132.0, 132.0),
    };
    let mut scene = MpScene::new(MpSceneId(77), viewport);
    let effect_id = scene.push_effect(MpEffectNode {
        spatial_id: scene.root_spatial_id,
        clip_chain_id: scene.root_clip_chain_id,
        opacity: 0.72,
        filters: vec![makepad_browser_scene::MpFilter::Blur(6.0)],
        blend_mode: MpBlendMode::Normal,
        isolation: MpIsolation::Isolate,
        mask: None,
    });
    scene.push_primitive(MpPrimitive::rounded_rect(
        MpPrimitiveId(0),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        Rect {
            pos: dvec2(4.0, 4.0),
            size: dvec2(124.0, 124.0),
        },
        color(0x223142),
        MpPerCornerRadius::uniform(18.0),
    ));
    for index in 0..4 {
        let mut primitive = MpPrimitive::rounded_rect(
            MpPrimitiveId(0),
            scene.root_spatial_id,
            scene.root_clip_chain_id,
            Rect {
                pos: dvec2(14.0 + index as f64 * 16.0, 18.0 + index as f64 * 18.0),
                size: dvec2(84.0, 34.0),
            },
            [color(0xff7d4d), color(0x4f8cff), color(0x53d48d), color(0xffcf58)][index],
            MpPerCornerRadius::uniform(14.0),
        );
        primitive.effect_id = Some(effect_id);
        scene.push_primitive(primitive);
    }
    MpDocument {
        id: MpDocumentId(7),
        epoch: 0,
        scene,
        glyph_runs: Default::default(),
        child_documents: Vec::new(),
    }
}
