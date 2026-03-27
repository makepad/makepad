pub use makepad_browser_scene;
pub use makepad_widgets;

use std::sync::Arc;

use makepad_browser_scene::{
    MpBrowserRenderer, MpDocument, MpDocumentId, MpExampleDocument, MpFontKey, MpFontResource,
    MpGlyphRunKey, MpGlyphRunMetrics, MpGlyphRunResource, MpPositionedGlyph, MpPrimitive,
    MpResourceStore, MpScene, MpSceneId, MpTextDecorations,
};
use makepad_widgets::*;

app_main!(App);

const COLOR_BG: Vec4f = Vec4f {
    x: 0.07,
    y: 0.08,
    z: 0.10,
    w: 1.0,
};
const FONT_BYTES: &[u8] = include_bytes!("../../../../widgets/resources/NotoSans-Regular.ttf");

script_mod! {
    use mod.prelude.widgets.*

    let TextLayoutDemoBase = #(TextLayoutDemo::register_widget(vm))
    let TextLayoutDemo = set_type_default() do TextLayoutDemoBase{
        width: Fill
        height: Fill
        draw_bg: mod.draw.DrawColor{color: #(COLOR_BG)}
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(960, 540)
                pass.clear_color: #(COLOR_BG)
                body +: {
                    demo := TextLayoutDemo{}
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
pub struct TextLayoutDemo {
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
    logged: bool,
}

impl Widget for TextLayoutDemo {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.renderer.is_none() {
            self.renderer = Some(MpBrowserRenderer::new(cx.cx));
        }

        cx.begin_turtle(walk, self.layout);
        let rect = Rect {
            pos: dvec2(0.0, 0.0),
            size: cx.current_pass_size(),
        };
        self.draw_bg.draw_abs(cx, rect);

        if self.example_document.is_none() {
            self.example_document = Some(build_document(rect));
        }

        let renderer = self.renderer.as_mut().unwrap();
        let stats = self
            .example_document
            .as_ref()
            .unwrap()
            .draw(renderer, cx, rect)
            .expect("text layout render");

        if !self.logged {
            self.logged = true;
            log!(
                "[makepad-browser-scene] text-layout direct={} isolated_boundaries={} isolated_primitives={} compositor_surfaces={}",
                stats.direct_primitive_count,
                stats.isolated_boundary_count,
                stats.isolated_primitive_count,
                stats.compositor_surface_count
            );
        }

        cx.end_turtle();
        DrawStep::done()
    }
}

fn build_document(viewport: Rect) -> MpExampleDocument {
    let mut scene = MpScene::new(MpSceneId(3), viewport);
    scene.push_primitive(MpPrimitive::solid_rect(
        makepad_browser_scene::MpPrimitiveId(0),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        Rect {
            pos: dvec2(40.0, 40.0),
            size: dvec2(400.0, 180.0),
        },
        vec4(0.16, 0.18, 0.24, 1.0),
    ));

    let glyph_run_key = MpGlyphRunKey(1);
    scene.push_primitive(MpPrimitive::text_run(
        makepad_browser_scene::MpPrimitiveId(1),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        Rect {
            pos: dvec2(72.0, 88.0),
            size: dvec2(640.0, 72.0),
        },
        glyph_run_key,
        vec4(0.95, 0.97, 0.99, 1.0),
    ));

    let mut document = MpDocument::new(MpDocumentId(1), scene);
    document.glyph_runs.insert(glyph_run_key, make_glyph_run());
    let mut resources = MpResourceStore::default();
    resources.fonts.insert(
        MpFontKey(1),
        MpFontResource {
            bytes: Arc::from(FONT_BYTES),
            face_index: 0,
        },
    );
    MpExampleDocument::new(document, resources)
}

fn make_glyph_run() -> MpGlyphRunResource {
    let text = "Makepad browser-scene text";
    let face = ttf_parser::Face::parse(FONT_BYTES, 0).expect("font face");
    let units_per_em = face.units_per_em() as f32;
    let font_size_px = 42.0_f32;
    let px_per_unit = font_size_px / units_per_em;
    let baseline_ascent_px = face.ascender() as f32 * px_per_unit;
    let underline = face.underline_metrics();
    let strikeout = face.strikeout_metrics();

    let mut pen_x = 0.0_f64;
    let mut glyphs = Vec::new();
    for ch in text.chars() {
        let Some(glyph_id) = face.glyph_index(ch) else {
            continue;
        };
        let advance = face.glyph_hor_advance(glyph_id).unwrap_or(0) as f32 * px_per_unit;
        glyphs.push(MpPositionedGlyph {
            glyph_id: glyph_id.0.into(),
            font_size_px,
            origin: dvec2(pen_x, baseline_ascent_px as f64),
            font_slot: 0,
        });
        pen_x += advance as f64;
    }

    MpGlyphRunResource {
        text: text.to_string(),
        font_keys: vec![MpFontKey(1)],
        glyphs,
        metrics: MpGlyphRunMetrics {
            advance_width_px: pen_x as f32,
            baseline_ascent_px,
            underline_offset_px: underline.map(|m| m.position as f32 * px_per_unit).unwrap_or(2.0),
            underline_thickness_px: underline.map(|m| m.thickness as f32 * px_per_unit).unwrap_or(2.0),
            strikeout_offset_px: strikeout.map(|m| m.position as f32 * px_per_unit).unwrap_or(14.0),
            strikeout_thickness_px: strikeout.map(|m| m.thickness as f32 * px_per_unit).unwrap_or(2.0),
        },
        decorations: MpTextDecorations {
            background_color: Some(vec4(0.24, 0.28, 0.38, 1.0)),
            decoration_color: Some(vec4(0.78, 0.84, 0.98, 1.0)),
            underline: true,
            overline: false,
            line_through: false,
            shadows: vec![makepad_browser_scene::MpTextShadow {
                offset: dvec2(2.0, 2.0),
                blur_radius_px: 0.0,
                color: vec4(0.0, 0.0, 0.0, 0.45),
            }],
        },
    }
}
