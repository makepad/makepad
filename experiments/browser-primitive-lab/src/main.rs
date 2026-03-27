pub use makepad_browser_scene;
pub use makepad_widgets;

use std::sync::Arc;

use makepad_browser_scene::{
    MpBackfaceVisibility, MpBorder, MpBoxShadow, MpBrowserRenderer, MpConicGradient,
    MpDocument, MpDocumentId, MpExampleDocument, MpImage, MpImageKey, MpImageResource,
    MpLinearGradient, MpPerCornerRadius, MpPrimitive, MpPrimitiveId, MpPrimitiveKind,
    MpRadialGradient, MpReferenceFrame, MpResourceStore, MpRoundedRect, MpScene, MpSceneId,
    MpSpatialId, MpSpatialKind, MpSpatialNode, MpTransformStyle,
};
use makepad_widgets::*;

app_main!(App);

const BG: Vec4f = Vec4f {
    x: 0.06,
    y: 0.08,
    z: 0.11,
    w: 1.0,
};
const PANEL_W: f64 = 220.0;
const PANEL_H: f64 = 180.0;
const GAP_X: f64 = 28.0;
const GAP_Y: f64 = 30.0;
const ROOT_PAD: f64 = 28.0;
const IMAGE_KEY: MpImageKey = MpImageKey(1);

script_mod! {
    use mod.prelude.widgets.*

    let PrimitiveLabBase = #(PrimitiveLab::register_widget(vm))
    let PrimitiveLab = set_type_default() do PrimitiveLabBase{
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
                    lab := PrimitiveLab{}
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
pub struct PrimitiveLab {
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
    example_document: Option<MpExampleDocument>,
    #[rust]
    viewport_size: DVec2,
    #[rust]
    logged: bool,
}

impl Widget for PrimitiveLab {
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
        if self.example_document.is_none() || self.viewport_size != viewport.size {
            self.viewport_size = viewport.size;
            self.example_document = Some(build_document(viewport));
            self.logged = false;
        }
        let renderer = self.renderer.as_mut().unwrap();
        let stats = self
            .example_document
            .as_ref()
            .unwrap()
            .draw(renderer, cx, viewport)
            .expect("browser primitive lab");
        if !self.logged {
            self.logged = true;
            log!(
                "[browser-primitive-lab] direct={} isolated_boundaries={} isolated_primitives={} compositor_surfaces={} scratch_surfaces={}",
                stats.direct_primitive_count,
                stats.isolated_boundary_count,
                stats.isolated_primitive_count,
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

fn skew_x(deg: f32) -> Mat4f {
    let t = deg.to_radians().tan();
    Mat4f {
        v: [
            1.0, 0.0, 0.0, 0.0, t, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
    }
}

fn scale_xy(sx: f32, sy: f32) -> Mat4f {
    Mat4f {
        v: [
            sx, 0.0, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
    }
}

fn css_rotate_x(angle: f32) -> Mat4f {
    let c = angle.cos();
    let s = angle.sin();
    Mat4f {
        v: [
            1.0, 0.0, 0.0, 0.0, 0.0, c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
    }
}

fn css_rotate_y(angle: f32) -> Mat4f {
    let c = angle.cos();
    let s = angle.sin();
    Mat4f {
        v: [
            c, 0.0, -s, 0.0, 0.0, 1.0, 0.0, 0.0, s, 0.0, c, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
    }
}

fn css_perspective(distance: f32) -> Mat4f {
    Mat4f {
        v: [
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            -1.0 / distance,
            0.0,
            0.0,
            0.0,
            1.0,
        ],
    }
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

fn build_document(viewport: Rect) -> MpExampleDocument {
    let mut scene = MpScene::new(MpSceneId(1), viewport);
    let root_spatial_id = scene.root_spatial_id;
    let mut resources = MpResourceStore::default();
    resources.images.insert(
        IMAGE_KEY,
        MpImageResource {
            size: dvec2(64.0, 64.0),
            rgba8: Arc::from(checkerboard_rgba(64, 64)),
        },
    );

    add_panel_frame(&mut scene, root_spatial_id, panel_rect(0, 0));
    add_panel_frame(&mut scene, root_spatial_id, panel_rect(1, 0));
    add_panel_frame(&mut scene, root_spatial_id, panel_rect(2, 0));
    add_panel_frame(&mut scene, root_spatial_id, panel_rect(0, 1));
    add_panel_frame(&mut scene, root_spatial_id, panel_rect(1, 1));
    add_panel_frame(&mut scene, root_spatial_id, panel_rect(2, 1));

    let translate_panel = panel_rect(0, 0);
    populate_panel(&mut scene, root_spatial_id, IMAGE_KEY, translate_panel.pos + dvec2(10.0, 12.0));

    let scale_panel = panel_rect(1, 0);
    let scale_id = push_reference_frame(
        &mut scene,
        root_spatial_id,
        scale_panel,
        Some(around(
            dvec2(PANEL_W * 0.5, PANEL_H * 0.5),
            scale_xy(0.86, 1.12),
        )),
        None,
        MpTransformStyle::Flat,
        true,
    );
    populate_panel(&mut scene, scale_id, IMAGE_KEY, dvec2(0.0, 0.0));

    let rotate_panel = panel_rect(2, 0);
    let rotate_id = push_reference_frame(
        &mut scene,
        root_spatial_id,
        rotate_panel,
        Some(around(dvec2(PANEL_W * 0.5, PANEL_H * 0.5), rotation_z(16.0))),
        None,
        MpTransformStyle::Flat,
        true,
    );
    populate_panel(&mut scene, rotate_id, IMAGE_KEY, dvec2(0.0, 0.0));

    let skew_panel = panel_rect(0, 1);
    let skew_id = push_reference_frame(
        &mut scene,
        root_spatial_id,
        skew_panel,
        Some(around(dvec2(18.0, 18.0), skew_x(18.0))),
        None,
        MpTransformStyle::Flat,
        true,
    );
    populate_panel(&mut scene, skew_id, IMAGE_KEY, dvec2(0.0, 0.0));

    let nested_panel = panel_rect(1, 1);
    let nested_outer = push_reference_frame(
        &mut scene,
        root_spatial_id,
        nested_panel,
        Some(around(dvec2(PANEL_W * 0.5, PANEL_H * 0.5), rotation_z(-9.0))),
        None,
        MpTransformStyle::Flat,
        true,
    );
    let nested_inner = scene.push_spatial_node(MpSpatialNode {
        parent: Some(nested_outer),
        kind: MpSpatialKind::ReferenceFrame(MpReferenceFrame {
            viewport_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: nested_panel.size,
            },
            placement_origin: dvec2(24.0, 12.0),
            transform: Some(around(dvec2(90.0, 80.0), scale_xy(0.88, 1.08))),
            perspective: None,
            transform_style: MpTransformStyle::Flat,
            backface_visibility: MpBackfaceVisibility::Visible,
            flattens_descendants: true,
        }),
    });
    populate_panel(&mut scene, nested_inner, IMAGE_KEY, dvec2(0.0, 0.0));

    let perspective_panel = panel_rect(2, 1);
    let perspective_id = push_reference_frame(
        &mut scene,
        root_spatial_id,
        perspective_panel,
        Some(around(
            dvec2(PANEL_W * 0.5, PANEL_H * 0.5),
            Mat4f::mul(&css_rotate_y(-0.42), &css_rotate_x(0.34)),
        )),
        Some(around(
            perspective_panel.pos + dvec2(PANEL_W * 0.5, PANEL_H * 0.5),
            css_perspective(900.0),
        )),
        MpTransformStyle::Preserve3D,
        false,
    );
    populate_panel(&mut scene, perspective_id, IMAGE_KEY, dvec2(0.0, 0.0));

    let overlap_a = scene.push_spatial_node(MpSpatialNode {
        parent: Some(perspective_id),
        kind: MpSpatialKind::ReferenceFrame(MpReferenceFrame {
            viewport_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: perspective_panel.size,
            },
            placement_origin: dvec2(22.0, 28.0),
            transform: Some(Mat4f::translation(vec3(0.0, 0.0, 40.0))),
            perspective: None,
            transform_style: MpTransformStyle::Preserve3D,
            backface_visibility: MpBackfaceVisibility::Visible,
            flattens_descendants: false,
        }),
    });
    add_overlay_card(&mut scene, overlap_a, color(0xf16c2f), dvec2(0.0, 0.0), dvec2(110.0, 72.0));

    let overlap_b = scene.push_spatial_node(MpSpatialNode {
        parent: Some(perspective_id),
        kind: MpSpatialKind::ReferenceFrame(MpReferenceFrame {
            viewport_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: perspective_panel.size,
            },
            placement_origin: dvec2(86.0, 84.0),
            transform: Some(Mat4f::translation(vec3(0.0, 0.0, -24.0))),
            perspective: None,
            transform_style: MpTransformStyle::Preserve3D,
            backface_visibility: MpBackfaceVisibility::Visible,
            flattens_descendants: false,
        }),
    });
    add_overlay_card(&mut scene, overlap_b, color(0x6ca8ff), dvec2(0.0, 0.0), dvec2(96.0, 64.0));

    let document = MpDocument::new(MpDocumentId(1), scene);
    MpExampleDocument::new(document, resources)
}

fn push_reference_frame(
    scene: &mut MpScene,
    parent: MpSpatialId,
    panel: Rect,
    transform: Option<Mat4f>,
    perspective: Option<Mat4f>,
    transform_style: MpTransformStyle,
    flattens_descendants: bool,
) -> MpSpatialId {
    scene.push_spatial_node(MpSpatialNode {
        parent: Some(parent),
        kind: MpSpatialKind::ReferenceFrame(MpReferenceFrame {
            viewport_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: panel.size,
            },
            placement_origin: panel.pos,
            transform,
            perspective,
            transform_style,
            backface_visibility: MpBackfaceVisibility::Visible,
            flattens_descendants,
        }),
    })
}

fn add_panel_frame(scene: &mut MpScene, spatial_id: MpSpatialId, panel: Rect) {
    scene.push_primitive(MpPrimitive::solid_rect(
        MpPrimitiveId(0),
        spatial_id,
        scene.root_clip_chain_id,
        panel,
        color(0x141a22),
    ));
    scene.push_primitive(MpPrimitive::border(
        MpPrimitiveId(0),
        spatial_id,
        scene.root_clip_chain_id,
        panel,
        color(0x334052),
        1.0,
        8.0,
    ));
}

fn populate_panel(scene: &mut MpScene, spatial_id: MpSpatialId, image_key: MpImageKey, origin: DVec2) {
    let panel = Rect {
        pos: origin + dvec2(16.0, 14.0),
        size: dvec2(PANEL_W - 32.0, PANEL_H - 30.0),
    };
    scene.push_primitive(MpPrimitive {
        id: MpPrimitiveId(0),
        spatial_id,
        clip_chain_id: scene.root_clip_chain_id,
        effect_id: None,
        bounds: panel,
        kind: MpPrimitiveKind::BoxShadow(MpBoxShadow {
            color: vec4(0.0, 0.0, 0.0, 0.35),
            box_offset: vec2(10.0, 12.0),
            box_size: vec2((panel.size.x - 20.0) as f32, (panel.size.y - 28.0) as f32),
            sigma: 12.0,
            corner_radius_px: 18.0,
            inset: false,
        }),
        hit_test_tag: None,
    });
    scene.push_primitive(MpPrimitive::rounded_rect(
        MpPrimitiveId(0),
        spatial_id,
        scene.root_clip_chain_id,
        panel,
        color(0xeaf0f8),
        MpPerCornerRadius::uniform(18.0),
    ));
    scene.push_primitive(MpPrimitive {
        id: MpPrimitiveId(0),
        spatial_id,
        clip_chain_id: scene.root_clip_chain_id,
        effect_id: None,
        bounds: Rect {
            pos: panel.pos + dvec2(0.0, 0.0),
            size: dvec2(panel.size.x, 34.0),
        },
        kind: MpPrimitiveKind::LinearGradient(MpLinearGradient {
            start: vec2(0.0, 0.5),
            end: vec2(1.0, 0.5),
            repeating: false,
            stops: vec![
                makepad_browser_scene::MpGradientStop {
                    offset: 0.0,
                    color: color(0x4f8cff),
                },
                makepad_browser_scene::MpGradientStop {
                    offset: 1.0,
                    color: color(0x8f67ff),
                },
            ],
        }),
        hit_test_tag: None,
    });
    scene.push_primitive(MpPrimitive {
        id: MpPrimitiveId(0),
        spatial_id,
        clip_chain_id: scene.root_clip_chain_id,
        effect_id: None,
        bounds: Rect {
            pos: panel.pos + dvec2(18.0, 18.0),
            size: dvec2(68.0, 68.0),
        },
        kind: MpPrimitiveKind::Image(MpImage { image_key }),
        hit_test_tag: None,
    });
    scene.push_primitive(MpPrimitive {
        id: MpPrimitiveId(0),
        spatial_id,
        clip_chain_id: scene.root_clip_chain_id,
        effect_id: None,
        bounds: Rect {
            pos: panel.pos + dvec2(102.0, 54.0),
            size: dvec2(64.0, 64.0),
        },
        kind: MpPrimitiveKind::RadialGradient(MpRadialGradient {
            center: vec2(0.5, 0.5),
            radius: vec2(0.5, 0.5),
            repeating: false,
            stops: vec![
                makepad_browser_scene::MpGradientStop {
                    offset: 0.0,
                    color: vec4(1.0, 0.74, 0.35, 1.0),
                },
                makepad_browser_scene::MpGradientStop {
                    offset: 1.0,
                    color: vec4(1.0, 0.74, 0.35, 0.0),
                },
            ],
        }),
        hit_test_tag: None,
    });
    scene.push_primitive(MpPrimitive {
        id: MpPrimitiveId(0),
        spatial_id,
        clip_chain_id: scene.root_clip_chain_id,
        effect_id: None,
        bounds: Rect {
            pos: panel.pos + dvec2(92.0, 92.0),
            size: dvec2(78.0, 52.0),
        },
        kind: MpPrimitiveKind::ConicGradient(MpConicGradient {
            center: vec2(0.5, 0.5),
            start_angle_rad: 0.4,
            repeating: false,
            stops: vec![
                makepad_browser_scene::MpGradientStop {
                    offset: 0.0,
                    color: color(0xff5c8a),
                },
                makepad_browser_scene::MpGradientStop {
                    offset: 0.5,
                    color: color(0x4f8cff),
                },
                makepad_browser_scene::MpGradientStop {
                    offset: 1.0,
                    color: color(0x45c98f),
                },
            ],
        }),
        hit_test_tag: None,
    });
    scene.push_primitive(MpPrimitive {
        id: MpPrimitiveId(0),
        spatial_id,
        clip_chain_id: scene.root_clip_chain_id,
        effect_id: None,
        bounds: Rect {
            pos: panel.pos + dvec2(20.0, 108.0),
            size: dvec2(58.0, 30.0),
        },
        kind: MpPrimitiveKind::RoundedRect(MpRoundedRect {
            color: color(0x16202c),
            radius: MpPerCornerRadius::uniform(10.0),
        }),
        hit_test_tag: None,
    });
    scene.push_primitive(MpPrimitive {
        id: MpPrimitiveId(0),
        spatial_id,
        clip_chain_id: scene.root_clip_chain_id,
        effect_id: None,
        bounds: panel,
        kind: MpPrimitiveKind::Border(MpBorder {
            color: color(0x223144),
            width: 2.0,
            radius: MpPerCornerRadius::uniform(18.0),
        }),
        hit_test_tag: None,
    });
}

fn add_overlay_card(scene: &mut MpScene, spatial_id: MpSpatialId, fill: Vec4f, pos: DVec2, size: DVec2) {
    scene.push_primitive(MpPrimitive::rounded_rect(
        MpPrimitiveId(0),
        spatial_id,
        scene.root_clip_chain_id,
        Rect { pos, size },
        fill,
        MpPerCornerRadius::uniform(14.0),
    ));
    scene.push_primitive(MpPrimitive::border(
        MpPrimitiveId(0),
        spatial_id,
        scene.root_clip_chain_id,
        Rect { pos, size },
        vec4(1.0, 1.0, 1.0, 0.35),
        2.0,
        MpPerCornerRadius::uniform(14.0),
    ));
}

fn checkerboard_rgba(width: usize, height: usize) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let even = ((x / 8) + (y / 8)) % 2 == 0;
            let c = if even { color(0xffffff) } else { color(0x1b2634) };
            rgba.push((c.x * 255.0) as u8);
            rgba.push((c.y * 255.0) as u8);
            rgba.push((c.z * 255.0) as u8);
            rgba.push(255);
        }
    }
    rgba
}
