pub use makepad_compositor;
pub use makepad_widgets;

use makepad_compositor::{
    MpBlendMode, MpBrowserFontResource, MpBrowserGlyphInstance, MpBrowserPicture,
    MpBrowserPrimitive, MpBrowserPrimitiveKind, MpBrowserScene, MpBrowserTask, MpBrowserTaskKind,
    MpBrowserTextDecorations, MpBrowserTextMetrics, MpBrowserTextRun, MpBrowserTextShadow,
    MpRenderer,
};
use makepad_widgets::makepad_draw::text::geom::Point as TextPoint;
use makepad_widgets::*;
use makepad_zune_png::{
    makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions},
    PngEncoder,
};
use std::path::PathBuf;
use std::sync::Arc;

app_main!(App);

const BG: Vec4f = Vec4f {
    x: 0.05,
    y: 0.07,
    z: 0.10,
    w: 1.0,
};
const ROOT_PAD: f64 = 28.0;
const CARD_TOP: f64 = 176.0;
const FONT_BYTES: &[u8] = include_bytes!("../../../widgets/resources/NotoSans-Regular.ttf");
const CARD_H: f64 = 420.0;
const HOST_TITLE_H: f64 = 92.0;
const SHELL_TABBAR_H: f64 = 54.0;
const SHELL_TOOLBAR_H: f64 = 54.0;
const SHELL_CONTENT_TOP: f64 = 132.0;

script_mod! {
    use mod.prelude.widgets.*

    let BrowserCacheLabBase = #(BrowserCacheLab::register_widget(vm))
    let BrowserCacheLab = set_type_default() do BrowserCacheLabBase{
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
                    lab := BrowserCacheLab{}
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
    #[rust]
    capture_mode_index: usize,
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

    fn capture_modes() -> [DemoMode; 3] {
        [DemoMode::Direct, DemoMode::Cached, DemoMode::Translated]
    }

    fn set_lab_mode(&mut self, cx: &mut Cx, mode: DemoMode) {
        if let Some(mut lab) = self.ui.widget(cx, ids!(lab)).borrow_mut::<BrowserCacheLab>() {
            if lab.mode != mode {
                lab.mode = mode;
                lab.logged = false;
                lab.redraw(cx);
            } else {
                lab.redraw(cx);
            }
        }
        cx.redraw_all();
    }

    fn request_capture(&mut self, cx: &mut Cx) {
        let mode = Self::capture_modes()[self.capture_mode_index];
        self.capture_path = PathBuf::from(format!(
            "/tmp/browser-cache-lab-{}.png",
            match mode {
                DemoMode::Direct => 1,
                DemoMode::Cached => 2,
                DemoMode::Translated => 3,
            }
        ));
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
            self.capture_mode_index += 1;
            if self.capture_mode_index < Self::capture_modes().len() {
                self.set_lab_mode(cx, Self::capture_modes()[self.capture_mode_index]);
                self.frames_until_capture = 4;
                self.capture_poll = Some(cx.new_next_frame());
            } else {
                self.capture_poll = None;
                println!("screenshots /tmp/browser-cache-lab-1.png /tmp/browser-cache-lab-2.png /tmp/browser-cache-lab-3.png");
                cx.quit();
            }
            return;
        }

        self.capture_poll = Some(cx.new_next_frame());
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.capture_mode_index = 0;
        self.set_lab_mode(cx, Self::capture_modes()[self.capture_mode_index]);
        self.frames_until_capture = 6;
        self.capture_poll = Some(cx.new_next_frame());
    }

    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_compositor::script_mod(vm);
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DemoMode {
    Direct,
    Cached,
    Translated,
}

#[derive(Clone, Copy)]
struct DemoFrame {
    card: Rect,
    host: Rect,
}

#[derive(Clone, Copy)]
struct DemoPalette {
    bg: u32,
    top: u32,
    rail: u32,
    main: u32,
    aux_a: u32,
    aux_b: u32,
    footer: u32,
}

impl Default for DemoMode {
    fn default() -> Self {
        Self::Direct
    }
}

impl DemoMode {
    fn title(self) -> &'static str {
        match self {
            Self::Direct => "Mode 1: direct browser scene",
            Self::Cached => "Mode 2: cached task + picture",
            Self::Translated => "Mode 3: direct + manual root translation",
        }
    }

    fn note(self) -> &'static str {
        match self {
            Self::Direct => "This should fail. If host_rect is ignored, content lands at window origin and overlaps the chrome.",
            Self::Cached => "This should succeed if the offscreen task path re-roots correctly. No overlap ambiguity remains in single-mode view.",
            Self::Translated => "This should succeed if adding one root translation is the missing contract in direct browser-scene submission.",
        }
    }

    fn signature(self) -> &'static str {
        match self {
            Self::Direct => "Signature: green root + yellow main block",
            Self::Cached => "Signature: magenta root + cyan main block",
            Self::Translated => "Signature: amber root + blue main block",
        }
    }

    fn palette(self) -> DemoPalette {
        match self {
            Self::Direct => DemoPalette {
                bg: 0x2ed06e,
                top: 0x1f7d4a,
                rail: 0x174330,
                main: 0xf6f95b,
                aux_a: 0x40a7ff,
                aux_b: 0xff8d4c,
                footer: 0xd9fff1,
            },
            Self::Cached => DemoPalette {
                bg: 0xd64fd2,
                top: 0x842f88,
                rail: 0x521d55,
                main: 0x67f2ff,
                aux_a: 0xfff36a,
                aux_b: 0x8f7bff,
                footer: 0xffd7fb,
            },
            Self::Translated => DemoPalette {
                bg: 0xffb347,
                top: 0xb86a19,
                rail: 0x704014,
                main: 0x5ac8ff,
                aux_a: 0xff6f61,
                aux_b: 0x5a7dff,
                footer: 0xfff2cf,
            },
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct BrowserCacheLab {
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
    renderer: Option<MpRenderer>,
    #[rust]
    direct_scene: Option<MpBrowserScene>,
    #[rust]
    cached_scene: Option<MpBrowserScene>,
    #[rust]
    translated_scene: Option<MpBrowserScene>,
    #[rust]
    viewport_size: DVec2,
    #[rust]
    mode: DemoMode,
    #[rust]
    logged: bool,
}

impl Widget for BrowserCacheLab {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::KeyDown(key_event) = event {
            if key_event.is_repeat {
                return;
            }
            let next_mode = match key_event.key_code {
                KeyCode::Key1 | KeyCode::Numpad1 => Some(DemoMode::Direct),
                KeyCode::Key2 | KeyCode::Numpad2 => Some(DemoMode::Cached),
                KeyCode::Key3 | KeyCode::Numpad3 => Some(DemoMode::Translated),
                _ => None,
            };
            if let Some(next_mode) = next_mode {
                if self.mode != next_mode {
                    self.mode = next_mode;
                    self.logged = false;
                }
                self.redraw(cx);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.renderer.is_none() {
            self.renderer = Some(MpRenderer::new(cx.cx));
        }

        cx.begin_turtle(walk, self.layout);
        let viewport = Rect {
            pos: dvec2(0.0, 0.0),
            size: cx.current_pass_size(),
        };
        let frame = demo_frame(viewport.size);
        self.draw_bg.draw_abs(cx, viewport);

        if self.viewport_size != viewport.size || self.direct_scene.is_none() {
            self.viewport_size = viewport.size;
            self.rebuild_scenes(viewport, frame.host);
            self.logged = false;
        }

        self.draw_shell_chrome(cx, viewport.size);
        self.draw_demo_frame(cx, frame);

        let renderer = self.renderer.as_mut().unwrap();
        match self.mode {
            DemoMode::Direct => renderer.draw_browser_scene(cx, self.direct_scene.as_ref().unwrap()),
            DemoMode::Cached => renderer.draw_browser_scene(cx, self.cached_scene.as_ref().unwrap()),
            DemoMode::Translated => draw_browser_scene_with_host_translation(
                cx,
                renderer,
                self.translated_scene.as_ref().unwrap(),
            ),
        }

        self.draw_legend(cx, frame, viewport.size);

        if !self.logged {
            self.logged = true;
            log!(
                "[browser-cache-lab] mode={:?} host={:?}",
                self.mode,
                frame.host,
            );
        }

        cx.end_turtle();
        DrawStep::done()
    }
}

impl BrowserCacheLab {
    fn rebuild_scenes(&mut self, viewport: Rect, host: Rect) {
        self.direct_scene = Some(build_browser_content_scene(host, DemoMode::Direct.palette()));
        self.cached_scene = Some(build_cached_scene(viewport, host, DemoMode::Cached.palette()));
        self.translated_scene = Some(build_browser_content_scene(host, DemoMode::Translated.palette()));
    }

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
            "browser-cache-lab: isolate one submission path at a time",
            dvec2(ROOT_PAD, 146.0),
            color(0xf1f6fc),
        );
        draw_line(
            cx,
            &mut self.draw_text_body,
            "Press 1, 2, 3 to switch modes. Each mode draws only one browser-scene path.",
            dvec2(ROOT_PAD, 164.0),
            color(0xa7bed6),
        );
        draw_line(
            cx,
            &mut self.draw_text_body,
            self.mode.title(),
            frame.card.pos + dvec2(18.0, 28.0),
            color(0xf1f6fc),
        );
        draw_line(
            cx,
            &mut self.draw_text_body,
            self.mode.note(),
            frame.card.pos + dvec2(18.0, 50.0),
            color(0xa7bed6),
        );
        draw_line(
            cx,
            &mut self.draw_text_body,
            self.mode.signature(),
            frame.card.pos + dvec2(18.0, 68.0),
            color(0x7d97b3),
        );
        draw_line(
            cx,
            &mut self.draw_text_body,
            "Expected browser host rect",
            frame.host.pos + dvec2(14.0, 22.0),
            color(0x7d97b3),
        );
        draw_line(
            cx,
            &mut self.draw_text_body,
            "Fake shell chrome above is intentional. Mode 1 should overlap it if direct root placement is broken.",
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

fn build_browser_content_scene(host_rect: Rect, palette: DemoPalette) -> MpBrowserScene {
    let mut scene = MpBrowserScene::new(host_rect);
    let size = host_rect.size;
    let top_h = 38.0;
    let rail_w = 54.0;
    let gutter = 14.0;
    let footer_h = 26.0;
    let main_x = rail_w + gutter * 2.0;
    let main_w = (size.x - main_x - gutter).max(80.0);
    let card_h = ((size.y - top_h - footer_h - gutter * 4.0) * 0.5).max(42.0);
    let second_y = top_h + gutter * 3.0 + card_h;

    scene.push_primitive(MpBrowserPrimitive {
        local_rect: Rect {
            pos: dvec2(0.0, 0.0),
            size,
        },
        transform_id: 0,
        clip_chain_id: 0,
        kind: MpBrowserPrimitiveKind::SolidRect {
            color: color(palette.bg),
        },
    });
    scene.push_primitive(MpBrowserPrimitive {
        local_rect: Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(size.x, top_h),
        },
        transform_id: 0,
        clip_chain_id: 0,
        kind: MpBrowserPrimitiveKind::SolidRect {
            color: color(palette.top),
        },
    });
    scene.push_primitive(MpBrowserPrimitive {
        local_rect: Rect {
            pos: dvec2(0.0, top_h),
            size: dvec2(rail_w, size.y - top_h),
        },
        transform_id: 0,
        clip_chain_id: 0,
        kind: MpBrowserPrimitiveKind::SolidRect {
            color: color(palette.rail),
        },
    });
    scene.push_primitive(MpBrowserPrimitive {
        local_rect: Rect {
            pos: dvec2(main_x, top_h + gutter),
            size: dvec2(main_w, card_h),
        },
        transform_id: 0,
        clip_chain_id: 0,
        kind: MpBrowserPrimitiveKind::RoundedRect {
            color: color(palette.main),
            radius: vec4(12.0, 12.0, 12.0, 12.0),
        },
    });
    scene.push_primitive(MpBrowserPrimitive {
        local_rect: Rect {
            pos: dvec2(main_x, second_y),
            size: dvec2(main_w * 0.56, card_h),
        },
        transform_id: 0,
        clip_chain_id: 0,
        kind: MpBrowserPrimitiveKind::RoundedRect {
            color: color(palette.aux_a),
            radius: vec4(12.0, 12.0, 12.0, 12.0),
        },
    });
    scene.push_primitive(MpBrowserPrimitive {
        local_rect: Rect {
            pos: dvec2(main_x + main_w * 0.62, second_y),
            size: dvec2(main_w * 0.38, card_h),
        },
        transform_id: 0,
        clip_chain_id: 0,
        kind: MpBrowserPrimitiveKind::RoundedRect {
            color: color(palette.aux_b),
            radius: vec4(12.0, 12.0, 12.0, 12.0),
        },
    });
    scene.push_primitive(MpBrowserPrimitive {
        local_rect: Rect {
            pos: dvec2(main_x, size.y - footer_h - gutter),
            size: dvec2(main_w, footer_h),
        },
        transform_id: 0,
        clip_chain_id: 0,
        kind: MpBrowserPrimitiveKind::RoundedRect {
            color: color(palette.footer),
            radius: vec4(8.0, 8.0, 8.0, 8.0),
        },
    });
    scene.push_text_run(make_text_run(
        Rect {
            pos: dvec2(main_x + 18.0, top_h + gutter + 18.0),
            size: dvec2(main_w - 36.0, 28.0),
        },
        color(0x182331),
        "Title text should stay inside yellow",
        false,
    ));
    scene.push_text_run(make_text_run(
        Rect {
            pos: dvec2(main_x + 18.0, second_y + 18.0),
            size: dvec2(main_w * 0.56 - 36.0, 24.0),
        },
        color(0xf6fbff),
        "Subtitle text should stay inside lower block",
        true,
    ));
    scene.push_primitive(MpBrowserPrimitive {
        local_rect: Rect {
            pos: dvec2(0.0, 0.0),
            size,
        },
        transform_id: 0,
        clip_chain_id: 0,
        kind: MpBrowserPrimitiveKind::Border {
            color: vec4(0.0, 0.0, 0.0, 0.22),
            width: 2.0,
            radius: vec4(0.0, 0.0, 0.0, 0.0),
        },
    });
    scene
}

fn make_text_run(local_rect: Rect, color: Vec4f, text: &str, decorate: bool) -> MpBrowserTextRun {
    let face = ttf_parser::Face::parse(FONT_BYTES, 0).expect("font face");
    let font_size_px = if decorate { 20.0 } else { 24.0 };
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
        glyphs.push(MpBrowserGlyphInstance {
            glyph_id: glyph_id.0.into(),
            font_size_px,
            origin: dvec2(pen_x, baseline_ascent_px as f64),
            font_slot: 0,
        });
        pen_x += advance as f64;
    }

    MpBrowserTextRun {
        stable_id: 0,
        local_rect,
        transform_id: 0,
        clip_chain_id: 0,
        color,
        fonts: vec![MpBrowserFontResource {
            key: 1,
            bytes: Arc::from(FONT_BYTES),
            face_index: 0,
        }],
        glyphs,
        metrics: MpBrowserTextMetrics {
            advance_width_px: pen_x as f32,
            baseline_ascent_px,
            underline_offset_px: underline.map(|m| m.position as f32 * px_per_unit).unwrap_or(2.0),
            underline_thickness_px: underline.map(|m| m.thickness as f32 * px_per_unit).unwrap_or(2.0),
            strikeout_offset_px: strikeout.map(|m| m.position as f32 * px_per_unit).unwrap_or(12.0),
            strikeout_thickness_px: strikeout.map(|m| m.thickness as f32 * px_per_unit).unwrap_or(2.0),
        },
        decorations: if decorate {
            MpBrowserTextDecorations {
                background_color: Some(vec4(0.12, 0.16, 0.22, 0.35)),
                decoration_color: Some(vec4(1.0, 1.0, 1.0, 0.9)),
                underline: true,
                overline: false,
                line_through: true,
                shadows: vec![MpBrowserTextShadow {
                    offset: dvec2(1.0, 1.0),
                    blur_radius_px: 0.0,
                    color: vec4(0.0, 0.0, 0.0, 0.45),
                }],
            }
        } else {
            MpBrowserTextDecorations::default()
        },
    }
}

fn build_cached_scene(viewport: Rect, host_rect: Rect, palette: DemoPalette) -> MpBrowserScene {
    let child_scene = build_browser_content_scene(
        Rect {
            pos: dvec2(0.0, 0.0),
            size: host_rect.size,
        },
        palette,
    );
    let mut scene = MpBrowserScene::new(viewport);
    let task_id = scene.push_task(MpBrowserTask {
        size: host_rect.size,
        cache_key: Some(7),
        kind: MpBrowserTaskKind::Scene(Box::new(child_scene)),
    });
    scene.push_picture(MpBrowserPicture {
        local_rect: host_rect,
        transform_id: 0,
        clip_chain_id: 0,
        task_id,
        opacity: 1.0,
        blend_mode: MpBlendMode::Normal,
    });
    scene
}

fn draw_browser_scene_with_host_translation(
    cx: &mut Cx2d,
    renderer: &mut MpRenderer,
    scene: &MpBrowserScene,
) {
    debug_assert!(
        !cx.draw_list_stack.is_empty(),
        "browser scene draw requires an active draw list"
    );
    let draw_list_id = *cx.draw_list_stack.last().unwrap();
    let previous_view_transform = cx.cx.draw_lists[draw_list_id].draw_list_uniforms.view_transform;
    let host_translation = Mat4f::translation(vec3(
        scene.host_rect.pos.x as f32,
        scene.host_rect.pos.y as f32,
        0.0,
    ));
    cx.cx.draw_lists[draw_list_id].draw_list_uniforms.view_transform =
        Mat4f::mul(&previous_view_transform, &host_translation);
    renderer.draw_browser_scene(cx, scene);
    cx.cx.draw_lists[draw_list_id].draw_list_uniforms.view_transform = previous_view_transform;
}
