pub use makepad_browser_scene;
pub use makepad_widgets;

use std::path::PathBuf;
use std::sync::Arc;

use makepad_browser_scene::{
    MpBrowserRenderer, MpDocument, MpDocumentId, MpExampleDocument, MpFontKey, MpFontResource,
    MpGlyphRunKey, MpGlyphRunMetrics, MpGlyphRunResource, MpPerCornerRadius, MpPositionedGlyph,
    MpPrimitive, MpResourceStore, MpScene, MpSceneId, MpTextDecorations, MpTextShadow,
};
use makepad_widgets::makepad_draw::text::geom::Point as TextPoint;
use makepad_widgets::*;
use makepad_zune_png::{
    makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions},
    PngEncoder,
};

app_main!(App);

const BG: Vec4f = Vec4f {
    x: 0.06,
    y: 0.08,
    z: 0.11,
    w: 1.0,
};
const ROOT_PAD: f64 = 28.0;
const CARD_TOP: f64 = 176.0;
const CARD_H: f64 = 420.0;
const HOST_TITLE_H: f64 = 92.0;
const SHELL_TABBAR_H: f64 = 54.0;
const SHELL_TOOLBAR_H: f64 = 54.0;
const SHELL_CONTENT_TOP: f64 = 132.0;
const FONT_KEY: u64 = 1001;
const FONT_BYTES: &[u8] = include_bytes!("../../../widgets/resources/NotoSans-Regular.ttf");

script_mod! {
    use mod.prelude.widgets.*

    let BrowserTextLabBase = #(BrowserTextLab::register_widget(vm))
    let BrowserTextLab = set_type_default() do BrowserTextLabBase{
        width: Fill
        height: Fill
        draw_bg: mod.draw.DrawColor{color: #(BG)}
        draw_fill: mod.draw.DrawColor{color: #xffffff}
        draw_text_title: mod.draw.DrawText{
            text_style: theme.font_regular{
                font_size: 16.0
            }
            color: #xf1f6fc
        }
        draw_text_body: mod.draw.DrawText{
            text_style: theme.font_regular{
                font_size: 12.0
            }
            color: #xa7bed6
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1760, 980)
                pass.clear_color: #(BG)
                body +: {
                    lab := BrowserTextLab{}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    pending_capture_request: Option<u64>,
    #[rust]
    capture_poll: Option<NextFrame>,
    #[rust]
    capture_path: PathBuf,
    #[rust]
    frames_until_capture: u32,
}

impl App {
    fn encode_png_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(4))
            .ok_or_else(|| "rgba size overflow while encoding png".to_string())?;
        if rgba.len() != expected {
            return Err(format!("expected {} rgba bytes, got {}", expected, rgba.len()));
        }

        let options = EncoderOptions::default()
            .set_width(width as usize)
            .set_height(height as usize)
            .set_depth(BitDepth::Eight)
            .set_colorspace(ColorSpace::RGBA);
        let mut encoder = PngEncoder::new(rgba, options);
        let mut out = Vec::new();
        encoder
            .encode(&mut out)
            .map_err(|err| format!("png encode failed: {err:?}"))?;
        Ok(out)
    }

    fn request_capture(&mut self, cx: &mut Cx) {
        self.capture_path = PathBuf::from("/tmp/browser-text-lab-host.png");
        self.pending_capture_request = Some(cx.request_capture(CaptureSource::Framebuffer));
        self.capture_poll = Some(cx.new_next_frame());
    }

    fn poll_capture(&mut self, cx: &mut Cx) {
        let Some(expected_request_id) = self.pending_capture_request else {
            return;
        };
        for result in cx.drain_capture_results() {
            if result.request_id != expected_request_id {
                continue;
            }
            if let Ok(png) = Self::encode_png_rgba(result.width, result.height, &result.rgba) {
                let _ = std::fs::write(&self.capture_path, png);
                println!("screenshot {}", self.capture_path.display());
            }
            self.pending_capture_request = None;
            self.capture_poll = None;
            cx.quit();
            return;
        }
        self.capture_poll = Some(cx.new_next_frame());
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.frames_until_capture = 60;
        self.capture_poll = Some(cx.new_next_frame());
    }

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
        if let Some(next) = self.capture_poll {
            if next.is_event(event).is_some() {
                if self.pending_capture_request.is_some() {
                    self.poll_capture(cx);
                } else if self.frames_until_capture > 0 {
                    self.frames_until_capture -= 1;
                    self.capture_poll = Some(cx.new_next_frame());
                } else {
                    self.request_capture(cx);
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct DemoFrame {
    card: Rect,
    host: Rect,
}

#[derive(Script, ScriptHook, Widget)]
pub struct BrowserTextLab {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_fill: DrawColor,
    #[live]
    draw_text_title: DrawText,
    #[live]
    draw_text_body: DrawText,
    #[rust]
    renderer: Option<MpBrowserRenderer>,
    #[rust]
    example_document: Option<MpExampleDocument>,
    #[rust]
    viewport_size: DVec2,
    #[rust]
    logged: bool,
}

impl Widget for BrowserTextLab {
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
        let frame = demo_frame(viewport.size);
        self.draw_bg.draw_abs(cx, viewport);
        if self.example_document.is_none() || self.viewport_size != viewport.size {
            self.viewport_size = viewport.size;
            self.example_document = Some(build_document(frame.host.size));
            self.logged = false;
        }

        self.draw_shell_chrome(cx, viewport.size);
        self.draw_demo_frame(cx, frame);

        let renderer = self.renderer.as_mut().unwrap();
        let stats = self
            .example_document
            .as_ref()
            .unwrap()
            .draw(renderer, cx, frame.host)
            .expect("browser text lab render");

        self.draw_legend(cx, frame, viewport.size);

        if !self.logged {
            self.logged = true;
            log!(
                "[browser-text-lab] host={:?} direct={} isolated_boundaries={} compositor_surfaces={} scratch_surfaces={}",
                frame.host,
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

impl BrowserTextLab {
    fn fill_rect(&mut self, cx: &mut Cx2d, rect: Rect, color: Vec4f) {
        self.draw_fill.color = color;
        self.draw_fill.draw_abs(cx, rect);
    }

    fn stroke_rect(&mut self, cx: &mut Cx2d, rect: Rect, width: f64, color: Vec4f) {
        self.fill_rect(
            cx,
            Rect {
                pos: rect.pos,
                size: dvec2(rect.size.x, width),
            },
            color,
        );
        self.fill_rect(
            cx,
            Rect {
                pos: dvec2(rect.pos.x, rect.pos.y + rect.size.y - width),
                size: dvec2(rect.size.x, width),
            },
            color,
        );
        self.fill_rect(
            cx,
            Rect {
                pos: rect.pos,
                size: dvec2(width, rect.size.y),
            },
            color,
        );
        self.fill_rect(
            cx,
            Rect {
                pos: dvec2(rect.pos.x + rect.size.x - width, rect.pos.y),
                size: dvec2(width, rect.size.y),
            },
            color,
        );
    }

    fn draw_shell_chrome(&mut self, cx: &mut Cx2d, size: DVec2) {
        self.fill_rect(
            cx,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(size.x, SHELL_TABBAR_H),
            },
            color(0x0f1722),
        );
        self.fill_rect(
            cx,
            Rect {
                pos: dvec2(0.0, SHELL_TABBAR_H),
                size: dvec2(size.x, SHELL_TOOLBAR_H),
            },
            color(0x152131),
        );
        self.fill_rect(
            cx,
            Rect {
                pos: dvec2(ROOT_PAD, 12.0),
                size: dvec2(200.0, 28.0),
            },
            color(0x24364d),
        );
        self.fill_rect(
            cx,
            Rect {
                pos: dvec2(ROOT_PAD + 214.0, 12.0),
                size: dvec2(180.0, 28.0),
            },
            color(0x1b2a3d),
        );
        self.fill_rect(
            cx,
            Rect {
                pos: dvec2(ROOT_PAD, 66.0),
                size: dvec2(size.x - ROOT_PAD * 2.0, 28.0),
            },
            color(0x213247),
        );
        self.fill_rect(
            cx,
            Rect {
                pos: dvec2(ROOT_PAD, SHELL_CONTENT_TOP),
                size: dvec2(size.x - ROOT_PAD * 2.0, 2.0),
            },
            color(0x223348),
        );
    }

    fn draw_demo_frame(&mut self, cx: &mut Cx2d, frame: DemoFrame) {
        self.fill_rect(cx, frame.card, color(0x121c29));
        self.stroke_rect(cx, frame.card, 1.0, color(0x29384a));
        self.fill_rect(cx, frame.host, color(0x0b1018));
        self.stroke_rect(cx, frame.host, 2.0, color(0x36506b));
    }

    fn draw_legend(&mut self, cx: &mut Cx2d, frame: DemoFrame, size: DVec2) {
        draw_line(
            cx,
            &mut self.draw_text_title,
            "browser-text-lab: direct draw_document into nonzero host rect",
            dvec2(ROOT_PAD, 146.0),
            color(0xf1f6fc),
        );
        draw_line(
            cx,
            &mut self.draw_text_body,
            "This mirrors the HAVI direct browser-scene path. Backgrounds and text should share the same host placement.",
            dvec2(ROOT_PAD, 164.0),
            color(0xa7bed6),
        );
        draw_line(
            cx,
            &mut self.draw_text_body,
            "If only text shifts upward, the remaining bug is in retained text lowering or draw-time text basis, not primitive host placement.",
            dvec2(ROOT_PAD, 182.0),
            color(0xa7bed6),
        );
        draw_line(
            cx,
            &mut self.draw_text_title,
            "Expected browser host rect",
            frame.host.pos + dvec2(14.0, 22.0),
            color(0x7d97b3),
        );
        draw_line(
            cx,
            &mut self.draw_text_body,
            "Capture: /tmp/browser-text-lab-host.png",
            dvec2(ROOT_PAD, size.y - 28.0),
            color(0x7d97b3),
        );
    }
}

fn draw_line(
    cx: &mut Cx2d,
    draw_text: &mut DrawText,
    text: &str,
    pos: DVec2,
    color: Vec4f,
) {
    if let Some(run) = draw_text.prepare_single_line_run(cx, text) {
        let positioned: Vec<_> = run
            .glyphs
            .iter()
            .map(|glyph| {
                (
                    TextPoint::new(
                        pos.x as f32 + glyph.pen_x_in_lpxs + glyph.offset_x_in_lpxs,
                        pos.y as f32,
                    ),
                    glyph.font_size_in_lpxs,
                    glyph.rasterized,
                )
            })
            .collect();
        draw_text.draw_rasterized_glyphs_exact_abs(cx, &positioned, color);
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

fn demo_frame(size: DVec2) -> DemoFrame {
    let card_w = (size.x - ROOT_PAD * 2.0).max(420.0);
    let card_h = (CARD_H.min(size.y - CARD_TOP - ROOT_PAD)).max(320.0);
    let host_pad = 18.0;
    DemoFrame {
        card: Rect {
            pos: dvec2(ROOT_PAD, CARD_TOP),
            size: dvec2(card_w, card_h),
        },
        host: Rect {
            pos: dvec2(ROOT_PAD + host_pad, CARD_TOP + HOST_TITLE_H),
            size: dvec2(card_w - host_pad * 2.0, card_h - HOST_TITLE_H - host_pad * 2.0),
        },
    }
}

fn build_document(host_size: DVec2) -> MpExampleDocument {
    let viewport = Rect {
        pos: dvec2(0.0, 0.0),
        size: host_size,
    };
    let mut scene = MpScene::new(MpSceneId(1), viewport);
    let mut resources = MpResourceStore::default();
    resources.fonts.insert(
        MpFontKey(FONT_KEY),
        MpFontResource {
            bytes: Arc::from(FONT_BYTES),
            face_index: 0,
        },
    );

    scene.push_primitive(MpPrimitive::solid_rect(
        makepad_browser_scene::MpPrimitiveId(0),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        viewport,
        color(0x2dcb72),
    ));
    scene.push_primitive(MpPrimitive::solid_rect(
        makepad_browser_scene::MpPrimitiveId(0),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(host_size.x, 40.0),
        },
        color(0x1f7d4a),
    ));
    scene.push_primitive(MpPrimitive::solid_rect(
        makepad_browser_scene::MpPrimitiveId(0),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        Rect {
            pos: dvec2(0.0, 40.0),
            size: dvec2(54.0, host_size.y - 40.0),
        },
        color(0x174330),
    ));
    scene.push_primitive(MpPrimitive::rounded_rect(
        makepad_browser_scene::MpPrimitiveId(0),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        Rect {
            pos: dvec2(82.0, 16.0),
            size: dvec2(host_size.x - 136.0, 86.0),
        },
        color(0xf6f95b),
        MpPerCornerRadius::uniform(14.0),
    ));
    scene.push_primitive(MpPrimitive::rounded_rect(
        makepad_browser_scene::MpPrimitiveId(0),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        Rect {
            pos: dvec2(82.0, 132.0),
            size: dvec2((host_size.x - 180.0) * 0.58, 86.0),
        },
        color(0x40a7ff),
        MpPerCornerRadius::uniform(14.0),
    ));
    scene.push_primitive(MpPrimitive::rounded_rect(
        makepad_browser_scene::MpPrimitiveId(0),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        Rect {
            pos: dvec2(host_size.x - 612.0, 132.0),
            size: dvec2(560.0, 86.0),
        },
        color(0xff8d4c),
        MpPerCornerRadius::uniform(14.0),
    ));
    scene.push_primitive(MpPrimitive::rounded_rect(
        makepad_browser_scene::MpPrimitiveId(0),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        Rect {
            pos: dvec2(82.0, host_size.y - 40.0),
            size: dvec2(host_size.x - 136.0, 18.0),
        },
        color(0xd9fff1),
        MpPerCornerRadius::uniform(8.0),
    ));

    add_run(
        &mut scene,
        &mut resources,
        MpGlyphRunKey(1),
        Rect {
            pos: dvec2(110.0, 30.0),
            size: dvec2(760.0, 28.0),
        },
        24.0,
        color(0x182331),
        "This title should stay centered inside the yellow block",
        false,
    );
    add_run(
        &mut scene,
        &mut resources,
        MpGlyphRunKey(2),
        Rect {
            pos: dvec2(110.0, 64.0),
            size: dvec2(980.0, 24.0),
        },
        18.0,
        color(0x32445a),
        "If it shifts upward while the yellow block stays put, the remaining bug is text-only.",
        false,
    );
    add_run(
        &mut scene,
        &mut resources,
        MpGlyphRunKey(3),
        Rect {
            pos: dvec2(112.0, 152.0),
            size: dvec2(760.0, 26.0),
        },
        20.0,
        color(0xffffff),
        "Decorated text should stay inside the blue block",
        true,
    );
    add_run(
        &mut scene,
        &mut resources,
        MpGlyphRunKey(4),
        Rect {
            pos: dvec2(host_size.x - 580.0, 154.0),
            size: dvec2(500.0, 24.0),
        },
        20.0,
        color(0xffffff),
        "Right block text baseline",
        false,
    );

    let mut document = MpDocument::new(MpDocumentId(1), scene);
    document.glyph_runs = std::mem::take(&mut resources.glyph_runs);
    MpExampleDocument::new(document, resources)
}

fn add_run(
    scene: &mut MpScene,
    resources: &mut MpResourceStore,
    key: MpGlyphRunKey,
    bounds: Rect,
    font_size_px: f32,
    color: Vec4f,
    text: &str,
    decorate: bool,
) {
    resources
        .glyph_runs
        .insert(key, make_glyph_run(text, font_size_px, decorate));
    scene.push_primitive(MpPrimitive::text_run(
        makepad_browser_scene::MpPrimitiveId(0),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        bounds,
        key,
        color,
    ));
}

fn make_glyph_run(text: &str, font_size_px: f32, decorate: bool) -> MpGlyphRunResource {
    let face = ttf_parser::Face::parse(FONT_BYTES, 0).expect("font face");
    let units_per_em = face.units_per_em() as f32;
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
        font_keys: vec![MpFontKey(FONT_KEY)],
        glyphs,
        metrics: MpGlyphRunMetrics {
            advance_width_px: pen_x as f32,
            baseline_ascent_px,
            underline_offset_px: underline.map(|m| m.position as f32 * px_per_unit).unwrap_or(2.0),
            underline_thickness_px: underline.map(|m| m.thickness as f32 * px_per_unit).unwrap_or(2.0),
            strikeout_offset_px: strikeout.map(|m| m.position as f32 * px_per_unit).unwrap_or(12.0),
            strikeout_thickness_px: strikeout.map(|m| m.thickness as f32 * px_per_unit).unwrap_or(2.0),
        },
        decorations: if decorate {
            MpTextDecorations {
                background_color: Some(vec4(0.24, 0.28, 0.38, 0.40)),
                decoration_color: Some(vec4(0.94, 0.98, 1.0, 1.0)),
                underline: true,
                overline: false,
                line_through: true,
                shadows: vec![MpTextShadow {
                    offset: dvec2(1.0, 1.0),
                    blur_radius_px: 0.0,
                    color: vec4(0.0, 0.0, 0.0, 0.45),
                }],
            }
        } else {
            MpTextDecorations::default()
        },
    }
}
