pub use makepad_browser_scene;
pub use makepad_widgets;

use makepad_browser_scene::{
    MpBackfaceVisibility, MpBrowserRenderer, MpClipChain, MpClipKind, MpClipNode,
    MpDocument, MpDocumentId, MpPerCornerRadius, MpPrimitive, MpPrimitiveId, MpReferenceFrame,
    MpScene, MpSceneId, MpSpatialId, MpSpatialKind, MpSpatialNode, MpTransformStyle,
};
use makepad_widgets::*;

app_main!(App);

const BG: Vec4f = Vec4f {
    x: 0.07,
    y: 0.09,
    z: 0.12,
    w: 1.0,
};
const PANEL_W: f64 = 230.0;
const PANEL_H: f64 = 180.0;
const GAP_X: f64 = 26.0;
const GAP_Y: f64 = 28.0;
const ROOT_PAD: f64 = 28.0;

script_mod! {
    use mod.prelude.widgets.*

    let BrowserClipLabBase = #(BrowserClipLab::register_widget(vm))
    let BrowserClipLab = set_type_default() do BrowserClipLabBase{
        width: Fill
        height: Fill
        draw_bg: mod.draw.DrawColor{color: #(BG)}
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1680, 1180)
                pass.clear_color: #(BG)
                body +: {
                    lab := BrowserClipLab{}
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
pub struct BrowserClipLab {
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

impl Widget for BrowserClipLab {
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
            .expect("browser clip lab render");
        if !self.logged {
            self.logged = true;
            log!(
                "[browser-clip-lab] direct={} isolated_boundaries={} compositor_surfaces={} scratch_surfaces={}",
                stats.direct_primitive_count,
                stats.isolated_boundary_count,
                stats.compositor_surface_count,
                stats.scratch_surface_count,
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

fn rotation_z(deg: f32) -> Mat4f {
    Mat4f::rotation(vec3(0.0, 0.0, deg.to_radians()))
}

fn around(origin: DVec2, transform: Mat4f) -> Mat4f {
    Mat4f::mul(
        &Mat4f::translation(vec3(origin.x as f32, origin.y as f32, 0.0)),
        &Mat4f::mul(
            &transform,
            &Mat4f::translation(vec3(-(origin.x as f32), -(origin.y as f32), 0.0)),
        ),
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

fn build_document(viewport: Rect) -> MpDocument {
    let mut scene = MpScene::new(MpSceneId(1), viewport);
    let root_spatial_id = scene.root_spatial_id;

    let nested_panel = panel_rect(0, 0);
    add_panel_frame(&mut scene, root_spatial_id, nested_panel, color(0x1d2530));
    add_nested_rect_clip_panel(&mut scene, root_spatial_id, nested_panel);

    let rounded_panel = panel_rect(1, 0);
    add_panel_frame(&mut scene, root_spatial_id, rounded_panel, color(0x1c2431));
    add_multi_rounded_clip_panel(&mut scene, root_spatial_id, rounded_panel);

    let image_mask_panel = panel_rect(2, 0);
    add_panel_frame(&mut scene, root_spatial_id, image_mask_panel, color(0x1b2330));
    add_image_mask_panel(&mut scene, root_spatial_id, image_mask_panel);

    let plane_panel = panel_rect(0, 1);
    add_panel_frame(&mut scene, root_spatial_id, plane_panel, color(0x18222d));
    add_plane_set_panel(&mut scene, root_spatial_id, plane_panel);

    let transformed_panel = panel_rect(1, 1);
    add_panel_frame(&mut scene, root_spatial_id, transformed_panel, color(0x19222f));
    add_transformed_clip_panel(&mut scene, root_spatial_id, transformed_panel);

    let shared_panel = panel_rect(2, 1);
    add_panel_frame(&mut scene, root_spatial_id, shared_panel, color(0x1d2533));
    add_shared_clip_panel(&mut scene, root_spatial_id, shared_panel);

    MpDocument::new(MpDocumentId(1), scene)
}

fn add_panel_frame(scene: &mut MpScene, spatial_id: MpSpatialId, panel: Rect, fill: Vec4f) {
    scene.push_primitive(MpPrimitive::rounded_rect(
        MpPrimitiveId(0),
        spatial_id,
        scene.root_clip_chain_id,
        panel,
        fill,
        MpPerCornerRadius::uniform(14.0),
    ));
    scene.push_primitive(MpPrimitive::border(
        MpPrimitiveId(0),
        spatial_id,
        scene.root_clip_chain_id,
        panel,
        vec4(1.0, 1.0, 1.0, 0.08),
        1.0,
        MpPerCornerRadius::uniform(14.0),
    ));
}

fn add_nested_rect_clip_panel(scene: &mut MpScene, spatial_id: MpSpatialId, panel: Rect) {
    let inner = panel_inner(panel);
    let outer = scene.push_clip(MpClipNode {
        spatial_id,
        kind: MpClipKind::Rect { rect: inner },
    });
    let outer_chain = scene.push_clip_chain(MpClipChain {
        parent: Some(scene.root_clip_chain_id),
        clips: vec![outer],
    });
    let inner_clip = scene.push_clip(MpClipNode {
        spatial_id,
        kind: MpClipKind::Rect {
            rect: Rect {
                pos: inner.pos + dvec2(26.0, 20.0),
                size: inner.size - dvec2(52.0, 40.0),
            },
        },
    });
    let chain = scene.push_clip_chain(MpClipChain {
        parent: Some(outer_chain),
        clips: vec![inner_clip],
    });
    add_stripe_stack(scene, spatial_id, chain, inner.pos - dvec2(24.0, 12.0), inner.size + dvec2(48.0, 24.0));
}

fn add_multi_rounded_clip_panel(scene: &mut MpScene, spatial_id: MpSpatialId, panel: Rect) {
    let inner = panel_inner(panel);
    let first = scene.push_clip(MpClipNode {
        spatial_id,
        kind: MpClipKind::RoundedRect {
            rect: inner,
            radius: MpPerCornerRadius::uniform(28.0),
        },
    });
    let first_chain = scene.push_clip_chain(MpClipChain {
        parent: Some(scene.root_clip_chain_id),
        clips: vec![first],
    });
    let second = scene.push_clip(MpClipNode {
        spatial_id,
        kind: MpClipKind::RoundedRect {
            rect: Rect {
                pos: inner.pos + dvec2(18.0, 18.0),
                size: inner.size - dvec2(36.0, 36.0),
            },
            radius: MpPerCornerRadius {
                tl: 10.0,
                tr: 32.0,
                br: 18.0,
                bl: 26.0,
            },
        },
    });
    let chain = scene.push_clip_chain(MpClipChain {
        parent: Some(first_chain),
        clips: vec![second],
    });
    add_diagonal_cards(scene, spatial_id, chain, inner.pos - dvec2(12.0, 8.0));
}

fn add_image_mask_panel(scene: &mut MpScene, spatial_id: MpSpatialId, panel: Rect) {
    let inner = panel_inner(panel);
    let mask = scene.push_clip(MpClipNode {
        spatial_id,
        kind: MpClipKind::ImageMask {
            rect: Rect {
                pos: inner.pos + dvec2(18.0, 10.0),
                size: inner.size - dvec2(36.0, 20.0),
            },
        },
    });
    let chain = scene.push_clip_chain(MpClipChain {
        parent: Some(scene.root_clip_chain_id),
        clips: vec![mask],
    });
    add_stripe_stack(scene, spatial_id, chain, inner.pos - dvec2(18.0, 0.0), inner.size + dvec2(36.0, 0.0));
    scene.push_primitive(MpPrimitive::border(
        MpPrimitiveId(0),
        spatial_id,
        scene.root_clip_chain_id,
        Rect {
            pos: inner.pos + dvec2(18.0, 10.0),
            size: inner.size - dvec2(36.0, 20.0),
        },
        vec4(1.0, 1.0, 1.0, 0.18),
        2.0,
        MpPerCornerRadius::uniform(8.0),
    ));
}

fn add_plane_set_panel(scene: &mut MpScene, spatial_id: MpSpatialId, panel: Rect) {
    let inner = panel_inner(panel);
    let left = inner.pos.x as f32 + 16.0;
    let top = inner.pos.y as f32 + 8.0;
    let right = (inner.pos.x + inner.size.x - 18.0) as f32;
    let bottom = (inner.pos.y + inner.size.y - 12.0) as f32;
    let planes = vec![
        vec4(1.0, 0.0, 0.0, -left),
        vec4(-1.0, 0.0, 0.0, right),
        vec4(0.18, 1.0, 0.0, -(top + 30.0)),
        vec4(-0.2, -1.0, 0.0, bottom),
    ];
    let clip = scene.push_clip(MpClipNode {
        spatial_id,
        kind: MpClipKind::PlaneSet { planes },
    });
    let chain = scene.push_clip_chain(MpClipChain {
        parent: Some(scene.root_clip_chain_id),
        clips: vec![clip],
    });
    add_plane_clipped_swatches(scene, spatial_id, chain, inner);
}

fn add_transformed_clip_panel(scene: &mut MpScene, spatial_id: MpSpatialId, panel: Rect) {
    let transformed_id = scene.push_spatial_node(MpSpatialNode {
        parent: Some(spatial_id),
        kind: MpSpatialKind::ReferenceFrame(MpReferenceFrame {
            viewport_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: panel.size,
            },
            placement_origin: panel.pos,
            transform: Some(around(dvec2(PANEL_W * 0.5, PANEL_H * 0.5), rotation_z(14.0))),
            perspective: None,
            transform_style: MpTransformStyle::Flat,
            backface_visibility: MpBackfaceVisibility::Visible,
            flattens_descendants: true,
        }),
    });
    let inner = Rect {
        pos: dvec2(14.0, 14.0),
        size: panel.size - dvec2(28.0, 28.0),
    };
    let rounded = scene.push_clip(MpClipNode {
        spatial_id: transformed_id,
        kind: MpClipKind::RoundedRect {
            rect: Rect {
                pos: inner.pos + dvec2(4.0, 6.0),
                size: inner.size - dvec2(8.0, 12.0),
            },
            radius: MpPerCornerRadius::uniform(22.0),
        },
    });
    let chain = scene.push_clip_chain(MpClipChain {
        parent: Some(scene.root_clip_chain_id),
        clips: vec![rounded],
    });
    add_diagonal_cards(scene, transformed_id, chain, inner.pos - dvec2(18.0, 10.0));
}

fn add_shared_clip_panel(scene: &mut MpScene, spatial_id: MpSpatialId, panel: Rect) {
    let inner = panel_inner(panel);
    let shared = scene.push_clip(MpClipNode {
        spatial_id,
        kind: MpClipKind::RoundedRect {
            rect: Rect {
                pos: inner.pos + dvec2(10.0, 10.0),
                size: inner.size - dvec2(20.0, 20.0),
            },
            radius: MpPerCornerRadius {
                tl: 18.0,
                tr: 30.0,
                br: 18.0,
                bl: 30.0,
            },
        },
    });
    let chain = scene.push_clip_chain(MpClipChain {
        parent: Some(scene.root_clip_chain_id),
        clips: vec![shared],
    });
    for row in 0..3 {
        for col in 0..3 {
            scene.push_primitive(MpPrimitive::rounded_rect(
                MpPrimitiveId(0),
                spatial_id,
                chain,
                Rect {
                    pos: inner.pos + dvec2(6.0 + col as f64 * 52.0, 8.0 + row as f64 * 44.0),
                    size: dvec2(44.0, 36.0),
                },
                [color(0x4f8cff), color(0xff7d4d), color(0x53d48d)][(row + col) % 3],
                MpPerCornerRadius::uniform(10.0),
            ));
        }
    }
}

fn add_stripe_stack(
    scene: &mut MpScene,
    spatial_id: MpSpatialId,
    clip_chain_id: makepad_browser_scene::MpClipChainId,
    origin: DVec2,
    size: DVec2,
) {
    for index in 0..6 {
        scene.push_primitive(MpPrimitive::solid_rect(
            MpPrimitiveId(0),
            spatial_id,
            clip_chain_id,
            Rect {
                pos: origin + dvec2(index as f64 * 22.0, index as f64 * 16.0),
                size,
            },
            [
                color(0x4f8cff),
                color(0xff7d4d),
                color(0x53d48d),
                color(0xffcf58),
                color(0xb971ff),
                color(0x6ce6ff),
            ][index],
        ));
    }
}

fn add_diagonal_cards(
    scene: &mut MpScene,
    spatial_id: MpSpatialId,
    clip_chain_id: makepad_browser_scene::MpClipChainId,
    origin: DVec2,
) {
    for index in 0..4 {
        scene.push_primitive(MpPrimitive::rounded_rect(
            MpPrimitiveId(0),
            spatial_id,
            clip_chain_id,
            Rect {
                pos: origin + dvec2(index as f64 * 30.0, index as f64 * 22.0),
                size: dvec2(120.0, 52.0),
            },
            [color(0x4f8cff), color(0xff7d4d), color(0x53d48d), color(0xffcf58)][index],
            MpPerCornerRadius::uniform(16.0),
        ));
    }
}

fn add_plane_clipped_swatches(
    scene: &mut MpScene,
    spatial_id: MpSpatialId,
    clip_chain_id: makepad_browser_scene::MpClipChainId,
    inner: Rect,
) {
    let colors = [
        color(0x4f8cff),
        color(0xff7d4d),
        color(0x53d48d),
        color(0xffcf58),
        color(0xb971ff),
    ];
    for index in 0..5 {
        scene.push_primitive(MpPrimitive::solid_rect(
            MpPrimitiveId(0),
            spatial_id,
            clip_chain_id,
            Rect {
                pos: inner.pos + dvec2(index as f64 * 28.0, index as f64 * 18.0),
                size: dvec2(inner.size.x - 16.0, 34.0),
            },
            colors[index],
        ));
    }
}
