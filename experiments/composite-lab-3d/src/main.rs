pub use makepad_widgets;

use makepad_compositor::{
    MpBackfaceVisibility, MpNode, MpReferenceFrame, MpRenderer, MpScene, MpSceneRoot,
    MpSurface, MpSurfaceColorFormat, MpSurfaceNode, MpSurfaceSource, MpTransformStyle,
};
use makepad_widgets::makepad_draw::text::geom::Point as TextPoint;
use makepad_widgets::*;
use makepad_zune_png::{
    makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions},
    PngEncoder,
};
use std::path::PathBuf;

const MODE_TILT: usize = 0;
const MODE_STACK: usize = 1;
const MODE_BACKFACE: usize = 2;
const MODE_ALL: usize = 3;
const MODE_NAMES: [&str; 4] = ["tilt", "stack", "backface", "all"];

const ROOT_PAD: f64 = 28.0;
const SECTION_GAP_X: f64 = 28.0;
const SECTION_GAP_Y: f64 = 32.0;

const CARD_W: f64 = 252.0;
const CARD_H: f64 = 188.0;
const BASE_CARD_W: f64 = 320.0;
const BASE_CARD_H: f64 = 240.0;

const CARD_OVERVIEW: usize = 0;
const CARD_CONTROLS: usize = 1;
const CARD_REPORTS: usize = 2;
const CARD_PROFILE: usize = 3;
const CARD_ALERTS: usize = 4;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.CompositorLab3dProbeBase = #(CompositorLab3dProbe::register_widget(vm))
    mod.widgets.CompositorLab3dProbe = set_type_default() do mod.widgets.CompositorLab3dProbeBase{
        width: Fill
        height: Fill
        draw_bg: mod.draw.DrawColor{color: #xffffff}
        draw_fill: mod.draw.DrawColor{color: #xffffff}
        draw_text_title: mod.draw.DrawText{
            text_style: theme.font_regular{
                font_size: 16.0
            }
            color: #x182331
        }
        draw_text_body: mod.draw.DrawText{
            text_style: theme.font_regular{
                font_size: 13.0
            }
            color: #x213244
        }
        draw_text_muted: mod.draw.DrawText{
            text_style: theme.font_regular{
                font_size: 13.0
            }
            color: #x718398
        }
        draw_text_light: mod.draw.DrawText{
            text_style: theme.font_regular{
                font_size: 13.0
            }
            color: #xffffff
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1760, 1320)
                pass.clear_color: #xffffff
                body +: {
                    shell := View{
                        width: Fill
                        height: Fill
                        flow: Down

                        toolbar := RoundedView{
                            width: Fill
                            height: 72
                            flow: Right
                            spacing: 12
                            padding: Inset{left: 20 top: 18 right: 20 bottom: 18}
                            draw_bg +: {color: #xdedede radius: 0.0}

                            toolbar_label := Label{
                                text: "shell chrome top bar"
                                draw_text.color: #x222222
                                draw_text.text_style.font_size: 16.0
                            }
                        }

                        content_row := View{
                            width: Fill
                            height: Fill
                            flow: Right

                            sidebar := RoundedView{
                                width: 96
                                height: Fill
                                flow: Down
                                spacing: 10
                                padding: Inset{left: 12 top: 16 right: 12 bottom: 16}
                                draw_bg +: {color: #xefefef radius: 0.0}

                                sidebar_label := Label{
                                    text: "left chrome"
                                    draw_text.color: #x222222
                                    draw_text.text_style.font_size: 14.0
                                }
                            }

                            probe := mod.widgets.CompositorLab3dProbe{}
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct CardSpec {
    accent_color: Vec4f,
    action_color: Vec4f,
    title: &'static str,
    footer: &'static str,
    variant: i32,
}

struct LabCardSurface {
    surface: MpSurface,
    draw_list: DrawList2d,
    spec: CardSpec,
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
    mode_index: usize,
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
        self.capture_path = PathBuf::from(format!(
            "/tmp/composite-lab-3d-{}.png",
            MODE_NAMES[self.mode_index]
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
            if self.mode_index < MODE_ALL {
                self.mode_index += 1;
                self.frames_until_capture = 4;
                self.capture_poll = Some(cx.new_next_frame());
                cx.redraw_all();
            } else {
                self.capture_poll = None;
                cx.quit();
            }
            return;
        }

        self.capture_poll = Some(cx.new_next_frame());
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.mode_index = MODE_TILT;
        self.frames_until_capture = 6;
        self.capture_poll = Some(cx.new_next_frame());
        self.ui.redraw(cx);
    }

    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_compositor::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        if let Some(mut probe) = self
            .ui
            .widget(cx, ids!(probe))
            .borrow_mut::<CompositorLab3dProbe>()
        {
            probe.mode_index = self.mode_index;
        }
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

#[derive(Script, ScriptHook, Widget)]
pub struct CompositorLab3dProbe {
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
    #[live]
    draw_text_muted: DrawText,
    #[live]
    draw_text_light: DrawText,
    #[rust]
    area: Area,
    #[rust]
    renderer: Option<MpRenderer>,
    #[rust]
    cards: Vec<LabCardSurface>,
    #[rust(true)]
    cards_dirty: bool,
    #[rust]
    mode_index: usize,
}

fn color_from_hex(hex: u32) -> Vec4f {
    vec4(
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
        1.0,
    )
}

fn card_specs() -> [CardSpec; 5] {
    [
        CardSpec {
            accent_color: color_from_hex(0x4f8cff),
            action_color: color_from_hex(0x4f8cff),
            title: "overview",
            footer: "surface source",
            variant: 0,
        },
        CardSpec {
            accent_color: color_from_hex(0x45b97a),
            action_color: color_from_hex(0x45b97a),
            title: "controls",
            footer: "action panel",
            variant: 1,
        },
        CardSpec {
            accent_color: color_from_hex(0xe48a3a),
            action_color: color_from_hex(0xe48a3a),
            title: "reports",
            footer: "summary tile",
            variant: 2,
        },
        CardSpec {
            accent_color: color_from_hex(0x7c68ff),
            action_color: color_from_hex(0x7c68ff),
            title: "profile",
            footer: "stack near",
            variant: 0,
        },
        CardSpec {
            accent_color: color_from_hex(0xff6b8a),
            action_color: color_from_hex(0xff6b8a),
            title: "alerts",
            footer: "stack far",
            variant: 1,
        },
    ]
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

fn css_rotate_z(angle: f32) -> Mat4f {
    let c = angle.cos();
    let s = angle.sin();
    Mat4f {
        v: [
            c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
    }
}

fn css_rotation_matrix(rotation: Vec3f) -> Mat4f {
    Mat4f::mul(
        &css_rotate_z(rotation.z),
        &Mat4f::mul(&css_rotate_y(rotation.y), &css_rotate_x(rotation.x)),
    )
}

fn css_perspective_matrix(distance: f32) -> Mat4f {
    if distance <= 0.0 {
        return Mat4f::identity();
    }
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

fn around_local_origin(origin: DVec2, transform: Mat4f) -> Mat4f {
    Mat4f::mul(
        &Mat4f::translation(vec3(origin.x as f32, origin.y as f32, 0.0)),
        &Mat4f::mul(
            &transform,
            &Mat4f::translation(vec3(-(origin.x as f32), -(origin.y as f32), 0.0)),
        ),
    )
}

fn translation3(x: f32, y: f32, z: f32) -> Mat4f {
    Mat4f::translation(vec3(x, y, z))
}

fn card_size() -> DVec2 {
    dvec2(CARD_W, CARD_H)
}

fn card_center() -> DVec2 {
    dvec2(CARD_W * 0.5, CARD_H * 0.5)
}

fn card_leaf_transform(anchor: DVec2, rotation: Vec3f, z: f32) -> Mat4f {
    Mat4f::mul(
        &translation3(anchor.x as f32, anchor.y as f32, z),
        &around_local_origin(card_center(), css_rotation_matrix(rotation)),
    )
}

fn card_leaf_perspective(anchor: DVec2, distance: f32) -> Mat4f {
    // MpReferenceFrame::perspective lives in scene space.
    // A direct DrawProjectiveQuad demo can use a local-center perspective matrix,
    // but the compositor scene needs that same perspective resolved around
    // the card's scene-space center before it is attached to the scene graph.
    around_local_origin(anchor + card_center(), css_perspective_matrix(distance))
}

fn section_body(rect: Rect) -> Rect {
    Rect {
        pos: rect.pos + dvec2(18.0, 64.0),
        size: dvec2((rect.size.x - 36.0).max(1.0), (rect.size.y - 82.0).max(1.0)),
    }
}

impl CompositorLab3dProbe {
    fn ensure_resources(&mut self, cx: &mut Cx) {
        if self.renderer.is_none() {
            self.renderer = Some(MpRenderer::new(cx));
        }
        if self.cards.is_empty() {
            self.cards = card_specs()
                .into_iter()
                .map(|spec| LabCardSurface {
                    surface: MpSurface::new(cx, card_size(), MpSurfaceColorFormat::BgraU8, false),
                    draw_list: DrawList2d::new(cx),
                    spec,
                })
                .collect();
            self.cards_dirty = true;
        }
    }

    fn draw_glyph_text(cx: &mut Cx2d, draw_text: &mut DrawText, text: &str, pos: DVec2, color: Vec4f) {
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

    fn fill_rect(cx: &mut Cx2d, draw_fill: &mut DrawColor, rect: Rect, color: Vec4f) {
        draw_fill.color = color;
        draw_fill.draw_abs(cx, rect);
    }

    fn draw_surface_rect(
        cx: &mut Cx2d,
        draw_fill: &mut DrawColor,
        size: DVec2,
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
        color: Vec4f,
    ) {
        let sx = size.x / BASE_CARD_W;
        let sy = size.y / BASE_CARD_H;
        Self::fill_rect(
            cx,
            draw_fill,
            Rect {
                pos: dvec2(x0 * sx, y0 * sy),
                size: dvec2((x1 - x0) * sx, (y1 - y0) * sy),
            },
            color,
        );
    }

    fn paint_card_surface(
        cx: &mut Cx2d,
        draw_fill: &mut DrawColor,
        draw_text_title: &mut DrawText,
        draw_text_body: &mut DrawText,
        draw_text_light: &mut DrawText,
        spec: CardSpec,
        size: DVec2,
    ) {
        let bg = color_from_hex(0xf4f7fb);
        let dark = color_from_hex(0x1b2634);
        let mid = color_from_hex(0x708296);
        let mid_light = color_from_hex(0x93a6ba);
        let border = color_from_hex(0xc6d0dc);
        let panel = color_from_hex(0xffffff);
        let quiet = color_from_hex(0x77889b);
        let quiet_light = color_from_hex(0x9aa9b9);
        let subtle = color_from_hex(0x243646);
        let subtle_text = color_from_hex(0x3f5368);
        let subtle_text_light = color_from_hex(0x7b8ea1);
        let muted = color_from_hex(0xeef2f7);
        let button_bg = color_from_hex(0xedf2f7);
        let footer_bg = color_from_hex(0x16202c);
        let scale_x = size.x / BASE_CARD_W;
        let scale_y = size.y / BASE_CARD_H;

        Self::draw_surface_rect(cx, draw_fill, size, 0.0, 0.0, BASE_CARD_W, BASE_CARD_H, bg);
        Self::draw_surface_rect(cx, draw_fill, size, 0.0, 0.0, BASE_CARD_W, 10.0, spec.accent_color);
        Self::draw_surface_rect(cx, draw_fill, size, 0.0, BASE_CARD_H - 20.0, BASE_CARD_W, BASE_CARD_H, footer_bg);
        Self::draw_surface_rect(cx, draw_fill, size, 0.0, 0.0, BASE_CARD_W, 1.0, border);
        Self::draw_surface_rect(cx, draw_fill, size, 0.0, BASE_CARD_H - 1.0, BASE_CARD_W, BASE_CARD_H, dark);
        Self::draw_surface_rect(cx, draw_fill, size, 0.0, 0.0, 1.0, BASE_CARD_H, dark);
        Self::draw_surface_rect(cx, draw_fill, size, BASE_CARD_W - 1.0, 0.0, BASE_CARD_W, BASE_CARD_H, dark);

        Self::draw_glyph_text(
            cx,
            draw_text_title,
            spec.title,
            dvec2(22.0 * scale_x, 30.0 * scale_y),
            dark,
        );
        Self::draw_glyph_text(
            cx,
            draw_text_body,
            "offscreen surface",
            dvec2(22.0 * scale_x, 46.0 * scale_y),
            quiet,
        );

        Self::draw_surface_rect(cx, draw_fill, size, 22.0, 60.0, 166.0, 72.0, dark);
        Self::draw_surface_rect(cx, draw_fill, size, 22.0, 84.0, 286.0, 92.0, mid);
        Self::draw_surface_rect(cx, draw_fill, size, 22.0, 100.0, 258.0, 108.0, mid_light);

        Self::draw_surface_rect(cx, draw_fill, size, 22.0, 126.0, 298.0, 172.0, panel);
        Self::draw_surface_rect(cx, draw_fill, size, 22.0, 126.0, 298.0, 127.0, border);
        Self::draw_surface_rect(cx, draw_fill, size, 22.0, 171.0, 298.0, 172.0, border);
        Self::draw_surface_rect(cx, draw_fill, size, 22.0, 126.0, 23.0, 172.0, border);
        Self::draw_surface_rect(cx, draw_fill, size, 297.0, 126.0, 298.0, 172.0, border);
        Self::draw_surface_rect(cx, draw_fill, size, 34.0, 142.0, 184.0, 150.0, mid_light);

        match spec.variant {
            0 => {
                Self::draw_surface_rect(cx, draw_fill, size, 22.0, 188.0, 136.0, 224.0, spec.action_color);
                Self::draw_surface_rect(cx, draw_fill, size, 42.0, 202.0, 112.0, 210.0, panel);
                Self::draw_surface_rect(cx, draw_fill, size, 160.0, 194.0, 286.0, 202.0, quiet);
                Self::draw_surface_rect(cx, draw_fill, size, 160.0, 210.0, 248.0, 218.0, quiet_light);
            }
            1 => {
                Self::draw_surface_rect(cx, draw_fill, size, 22.0, 188.0, 44.0, 210.0, subtle);
                Self::draw_surface_rect(cx, draw_fill, size, 26.0, 192.0, 40.0, 206.0, spec.action_color);
                Self::draw_surface_rect(cx, draw_fill, size, 58.0, 194.0, 226.0, 202.0, subtle_text);
                Self::draw_surface_rect(cx, draw_fill, size, 58.0, 212.0, 264.0, 220.0, subtle_text_light);
                Self::draw_surface_rect(cx, draw_fill, size, 238.0, 188.0, 298.0, 228.0, button_bg);
                Self::draw_surface_rect(cx, draw_fill, size, 252.0, 202.0, 284.0, 210.0, quiet);
                Self::draw_surface_rect(cx, draw_fill, size, 238.0, 188.0, 239.0, 228.0, border);
                Self::draw_surface_rect(cx, draw_fill, size, 297.0, 188.0, 298.0, 228.0, border);
                Self::draw_surface_rect(cx, draw_fill, size, 238.0, 188.0, 298.0, 189.0, border);
                Self::draw_surface_rect(cx, draw_fill, size, 238.0, 227.0, 298.0, 228.0, border);
            }
            _ => {
                Self::draw_surface_rect(cx, draw_fill, size, 22.0, 188.0, 122.0, 228.0, muted);
                Self::draw_surface_rect(cx, draw_fill, size, 22.0, 188.0, 23.0, 228.0, border);
                Self::draw_surface_rect(cx, draw_fill, size, 121.0, 188.0, 122.0, 228.0, border);
                Self::draw_surface_rect(cx, draw_fill, size, 22.0, 188.0, 122.0, 189.0, border);
                Self::draw_surface_rect(cx, draw_fill, size, 22.0, 227.0, 122.0, 228.0, border);
                Self::draw_surface_rect(cx, draw_fill, size, 138.0, 188.0, 298.0, 228.0, spec.action_color);
                Self::draw_surface_rect(cx, draw_fill, size, 176.0, 202.0, 260.0, 210.0, panel);
            }
        }

        Self::draw_glyph_text(
            cx,
            draw_text_light,
            spec.footer,
            dvec2(14.0 * scale_x, (BASE_CARD_H - 18.0) * scale_y),
            vec4(1.0, 1.0, 1.0, 1.0),
        );
    }

    fn refresh_cards(&mut self, cx: &mut Cx2d) {
        if !self.cards_dirty {
            return;
        }

        let draw_fill = &mut self.draw_fill;
        let draw_text_title = &mut self.draw_text_title;
        let draw_text_body = &mut self.draw_text_body;
        let draw_text_light = &mut self.draw_text_light;

        for card in &mut self.cards {
            card.surface.begin(cx, None);
            cx.set_pass_shift_scale(card.surface.pass(), dvec2(0.0, 0.0), dvec2(1.0, 1.0));
            card.draw_list.begin_always(cx);
            cx.begin_root_turtle_for_pass(Layout::default());
            Self::paint_card_surface(
                cx,
                draw_fill,
                draw_text_title,
                draw_text_body,
                draw_text_light,
                card.spec,
                card_size(),
            );
            cx.end_pass_sized_turtle();
            card.draw_list.end(cx);
            card.surface.end(cx);
        }
        self.cards_dirty = false;
    }

    fn draw_section_frame(&mut self, cx: &mut Cx2d, rect: Rect, title: &str, subtitle: &str) -> Rect {
        let bg = color_from_hex(0xf7f9fc);
        let border = color_from_hex(0xd0d8e2);
        let header_rule = color_from_hex(0xe4eaf1);

        Self::fill_rect(cx, &mut self.draw_fill, rect, bg);
        Self::fill_rect(
            cx,
            &mut self.draw_fill,
            Rect {
                pos: rect.pos,
                size: dvec2(rect.size.x, 1.0),
            },
            border,
        );
        Self::fill_rect(
            cx,
            &mut self.draw_fill,
            Rect {
                pos: rect.pos + dvec2(0.0, rect.size.y - 1.0),
                size: dvec2(rect.size.x, 1.0),
            },
            border,
        );
        Self::fill_rect(
            cx,
            &mut self.draw_fill,
            Rect {
                pos: rect.pos,
                size: dvec2(1.0, rect.size.y),
            },
            border,
        );
        Self::fill_rect(
            cx,
            &mut self.draw_fill,
            Rect {
                pos: rect.pos + dvec2(rect.size.x - 1.0, 0.0),
                size: dvec2(1.0, rect.size.y),
            },
            border,
        );
        Self::draw_glyph_text(
            cx,
            &mut self.draw_text_title,
            title,
            rect.pos + dvec2(18.0, 24.0),
            color_from_hex(0x182331),
        );
        Self::draw_glyph_text(
            cx,
            &mut self.draw_text_muted,
            subtitle,
            rect.pos + dvec2(18.0, 44.0),
            color_from_hex(0x718398),
        );
        Self::fill_rect(
            cx,
            &mut self.draw_fill,
            Rect {
                pos: rect.pos + dvec2(0.0, 62.0),
                size: dvec2(rect.size.x, 1.0),
            },
            header_rule,
        );
        section_body(rect)
    }

    fn draw_caption(&mut self, cx: &mut Cx2d, pos: DVec2, title: &str, subtitle: &str) {
        Self::draw_glyph_text(
            cx,
            &mut self.draw_text_body,
            title,
            pos,
            color_from_hex(0x213244),
        );
        Self::draw_glyph_text(
            cx,
            &mut self.draw_text_muted,
            subtitle,
            pos + dvec2(0.0, 16.0),
            color_from_hex(0x718398),
        );
    }

    fn draw_guide_line(&mut self, cx: &mut Cx2d, pos: DVec2, width: f64) {
        Self::fill_rect(
            cx,
            &mut self.draw_fill,
            Rect {
                pos,
                size: dvec2(width, 2.0),
            },
            color_from_hex(0x1f2a37),
        );
    }

    fn draw_marker(&mut self, cx: &mut Cx2d, center: DVec2, color: Vec4f) {
        Self::fill_rect(
            cx,
            &mut self.draw_fill,
            Rect {
                pos: center - dvec2(4.0, 4.0),
                size: dvec2(8.0, 8.0),
            },
            color,
        );
    }

    fn scene_for_pass(&self, cx: &mut Cx2d) -> MpScene {
        MpScene::new(MpSceneRoot {
            host_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: cx.current_pass_size(),
            },
            page_to_host: Mat4f::identity(),
            clip: None,
        })
    }

    fn add_reference_frame(
        scene: &mut MpScene,
        parent: Option<usize>,
        size: DVec2,
        transform: Mat4f,
        perspective: Option<Mat4f>,
        transform_style: MpTransformStyle,
        backface_visibility: MpBackfaceVisibility,
        flattens_descendants: bool,
    ) -> usize {
        scene.push(MpNode::ReferenceFrame(MpReferenceFrame {
            parent,
            clip: None,
            local_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size,
            },
            transform,
            perspective,
            transform_style,
            backface_visibility,
            flattens_descendants,
        }))
    }

    fn add_surface_node(
        scene: &mut MpScene,
        parent: usize,
        size: DVec2,
        texture: Texture,
        backface_visibility: MpBackfaceVisibility,
    ) {
        scene.push(MpNode::Surface(MpSurfaceNode {
            parent,
            clip: None,
            local_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size,
            },
            source: MpSurfaceSource::SurfaceTexture(texture),

            backface_visibility,
        }));
    }

    fn card_texture(&self, index: usize) -> Texture {
        self.cards[index].surface.color_texture().clone()
    }

    fn add_flat_card(&self, scene: &mut MpScene, anchor: DVec2, texture: Texture) {
        let rf = Self::add_reference_frame(
            scene,
            None,
            card_size(),
            translation3(anchor.x as f32, anchor.y as f32, 0.0),
            None,
            MpTransformStyle::Flat,
            MpBackfaceVisibility::Visible,
            true,
        );
        Self::add_surface_node(
            scene,
            rf,
            card_size(),
            texture,
            MpBackfaceVisibility::Visible,
        );
    }

    fn add_leaf_card(
        &self,
        scene: &mut MpScene,
        anchor: DVec2,
        rotation: Vec3f,
        z: f32,
        perspective: f32,
        texture: Texture,
        backface_visibility: MpBackfaceVisibility,
    ) {
        let rf = Self::add_reference_frame(
            scene,
            None,
            card_size(),
            card_leaf_transform(anchor, rotation, z),
            Some(card_leaf_perspective(anchor, perspective)),
            MpTransformStyle::Preserve3D,
            backface_visibility,
            false,
        );
        Self::add_surface_node(scene, rf, card_size(), texture, backface_visibility);
    }

    fn draw_scene(&mut self, cx: &mut Cx2d, scene: &MpScene) {
        let _ = self.renderer.as_mut().unwrap().draw_scene(cx, scene);
    }

    fn draw_tilt_section(&mut self, cx: &mut Cx2d, rect: Rect) {
        let body = self.draw_section_frame(
            cx,
            rect,
            "independent 3d cards",
            "each card is one offscreen surface. perspective and rotation live on the compositor reference frame.",
        );

        let label_y = body.pos.y + 4.0;
        let card_y = body.pos.y + 56.0;
        let start_x = body.pos.x + 6.0;
        let gap = 34.0;
        let anchors = [
            dvec2(start_x, card_y),
            dvec2(start_x + (CARD_W + gap), card_y),
            dvec2(start_x + 2.0 * (CARD_W + gap), card_y),
            dvec2(start_x + 3.0 * (CARD_W + gap), card_y),
        ];
        let captions = [
            ("flat baseline", "same surface, no 3d pose"),
            ("rotateX", "preserve-3d leaf frame"),
            ("rotateY", "perspective around center"),
            ("rotateX + rotateY", "same scene model, mixed angles"),
        ];
        for (anchor, (title, subtitle)) in anchors.into_iter().zip(captions) {
            self.draw_caption(cx, dvec2(anchor.x, label_y), title, subtitle);
            self.draw_guide_line(cx, anchor + dvec2(0.0, CARD_H + 12.0), CARD_W);
        }

        let mut scene = self.scene_for_pass(cx);
        self.add_flat_card(&mut scene, anchors[0], self.card_texture(CARD_OVERVIEW));
        self.add_leaf_card(
            &mut scene,
            anchors[1],
            vec3(0.42, 0.0, 0.0),
            0.0,
            980.0,
            self.card_texture(CARD_CONTROLS),
            MpBackfaceVisibility::Visible,
        );
        self.add_leaf_card(
            &mut scene,
            anchors[2],
            vec3(0.0, -0.50, 0.0),
            0.0,
            980.0,
            self.card_texture(CARD_REPORTS),
            MpBackfaceVisibility::Visible,
        );
        self.add_leaf_card(
            &mut scene,
            anchors[3],
            vec3(0.34, -0.32, 0.0),
            0.0,
            1060.0,
            self.card_texture(CARD_PROFILE),
            MpBackfaceVisibility::Visible,
        );
        self.draw_scene(cx, &scene);
    }

    fn draw_stack_section(&mut self, cx: &mut Cx2d, rect: Rect) {
        let body = self.draw_section_frame(
            cx,
            rect,
            "shared preserve-3d stack",
            "shared perspective ancestor. sibling surfaces mix translateZ and rotation in one compositor island.",
        );
        let group_rect = Rect {
            pos: body.pos + dvec2(8.0, 54.0),
            size: dvec2((body.size.x - 16.0).max(1.0), (body.size.y - 62.0).max(1.0)),
        };

        self.draw_caption(
            cx,
            group_rect.pos + dvec2(4.0, 0.0),
            "perspective origin",
            "red marker: shared island center",
        );
        self.draw_guide_line(
            cx,
            group_rect.pos + dvec2(8.0, group_rect.size.y - 18.0),
            (group_rect.size.x - 16.0).max(1.0),
        );

        let mut scene = self.scene_for_pass(cx);
        let island = Self::add_reference_frame(
            &mut scene,
            None,
            group_rect.size,
            translation3(group_rect.pos.x as f32, group_rect.pos.y as f32, 0.0),
            Some(around_local_origin(
                group_rect.pos + group_rect.size * 0.5,
                css_perspective_matrix(1180.0),
            )),
            MpTransformStyle::Preserve3D,
            MpBackfaceVisibility::Visible,
            false,
        );

        let children = [
            (
                dvec2(42.0, 92.0),
                vec3(0.16, -0.24, 0.0),
                -50.0,
                CARD_ALERTS,
                MpBackfaceVisibility::Visible,
            ),
            (
                dvec2(166.0, 44.0),
                vec3(0.06, 0.14, 0.0),
                20.0,
                CARD_PROFILE,
                MpBackfaceVisibility::Visible,
            ),
            (
                dvec2(292.0, 90.0),
                vec3(-0.14, 0.28, 0.0),
                84.0,
                CARD_REPORTS,
                MpBackfaceVisibility::Visible,
            ),
        ];
        for (anchor, rotation, z, card_index, backface_visibility) in children {
            let child = Self::add_reference_frame(
                &mut scene,
                Some(island),
                card_size(),
                card_leaf_transform(anchor, rotation, z),
                None,
                MpTransformStyle::Preserve3D,
                backface_visibility,
                false,
            );
            Self::add_surface_node(
                &mut scene,
                child,
                card_size(),
                self.card_texture(card_index),
                backface_visibility,
            );
        }

        self.draw_scene(cx, &scene);
        self.draw_marker(
            cx,
            group_rect.pos + group_rect.size * 0.5,
            color_from_hex(0xff4d4f),
        );
    }

    fn draw_backface_section(&mut self, cx: &mut Cx2d, rect: Rect) {
        let body = self.draw_section_frame(
            cx,
            rect,
            "backface visibility",
            "the middle card flips away. the right card uses the same back-facing pose but remains visible.",
        );

        let label_y = body.pos.y + 4.0;
        let card_y = body.pos.y + 56.0;
        let start_x = body.pos.x + 10.0;
        let gap = 28.0;
        let anchors = [
            dvec2(start_x, card_y),
            dvec2(start_x + (CARD_W + gap), card_y),
            dvec2(start_x + 2.0 * (CARD_W + gap), card_y),
        ];
        let captions = [
            ("front-facing", "reference control"),
            ("back-facing hidden", "backface_visibility: hidden"),
            ("back-facing visible", "backface_visibility: visible"),
        ];
        for (anchor, (title, subtitle)) in anchors.into_iter().zip(captions) {
            self.draw_caption(cx, dvec2(anchor.x, label_y), title, subtitle);
            self.draw_guide_line(cx, anchor + dvec2(0.0, CARD_H + 12.0), CARD_W);
        }

        let mut scene = self.scene_for_pass(cx);
        self.add_leaf_card(
            &mut scene,
            anchors[0],
            vec3(0.10, -0.18, 0.0),
            0.0,
            980.0,
            self.card_texture(CARD_CONTROLS),
            MpBackfaceVisibility::Visible,
        );
        self.add_leaf_card(
            &mut scene,
            anchors[1],
            vec3(0.06, 2.72, 0.0),
            0.0,
            980.0,
            self.card_texture(CARD_ALERTS),
            MpBackfaceVisibility::Hidden,
        );
        self.add_leaf_card(
            &mut scene,
            anchors[2],
            vec3(0.06, 2.72, 0.0),
            0.0,
            980.0,
            self.card_texture(CARD_ALERTS),
            MpBackfaceVisibility::Visible,
        );
        self.draw_scene(cx, &scene);
    }

    fn draw_all_sections(&mut self, cx: &mut Cx2d, root: DVec2, full_width: f64, full_height: f64) {
        let top_rect = Rect {
            pos: root,
            size: dvec2(full_width, 352.0),
        };
        let stack_rect = Rect {
            pos: root + dvec2(0.0, top_rect.size.y + SECTION_GAP_Y),
            size: dvec2(620.0, (full_height - top_rect.size.y - SECTION_GAP_Y).max(320.0)),
        };
        let backface_rect = Rect {
            pos: dvec2(stack_rect.pos.x + stack_rect.size.x + SECTION_GAP_X, stack_rect.pos.y),
            size: dvec2(
                (full_width - stack_rect.size.x - SECTION_GAP_X).max(320.0),
                stack_rect.size.y,
            ),
        };
        self.draw_tilt_section(cx, top_rect);
        self.draw_stack_section(cx, stack_rect);
        self.draw_backface_section(cx, backface_rect);
    }
}

impl Widget for CompositorLab3dProbe {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_resources(cx.cx);
        self.refresh_cards(cx);

        cx.begin_turtle(walk, self.layout);
        let full_rect = cx.turtle().rect();
        self.draw_bg.draw_abs(cx, full_rect);

        let root = full_rect.pos + dvec2(ROOT_PAD, ROOT_PAD);
        let width = (full_rect.size.x - 2.0 * ROOT_PAD).max(320.0);
        let height = (full_rect.size.y - 2.0 * ROOT_PAD).max(320.0);

        match self.mode_index {
            MODE_TILT => self.draw_tilt_section(
                cx,
                Rect {
                    pos: root,
                    size: dvec2(width, 352.0),
                },
            ),
            MODE_STACK => self.draw_stack_section(
                cx,
                Rect {
                    pos: root,
                    size: dvec2(width, 420.0),
                },
            ),
            MODE_BACKFACE => self.draw_backface_section(
                cx,
                Rect {
                    pos: root,
                    size: dvec2(width, 352.0),
                },
            ),
            _ => self.draw_all_sections(cx, root, width, height),
        }

        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_card_rotation(rotation: Vec3f) -> Mat4f {
        around_local_origin(card_center(), css_rotation_matrix(rotation))
    }

    fn local_card_perspective(distance: f32) -> Mat4f {
        around_local_origin(card_center(), css_perspective_matrix(distance))
    }

    fn project_xy(matrix: Mat4f, point: DVec2) -> DVec2 {
        let clip = matrix.transform_vec4(vec4f(point.x as f32, point.y as f32, 0.0, 1.0));
        dvec2((clip.x / clip.w) as f64, (clip.y / clip.w) as f64)
    }

    fn assert_close(actual: DVec2, expected: DVec2) {
        assert!((actual.x - expected.x).abs() < 0.0001, "x mismatch: actual={actual:?} expected={expected:?}");
        assert!((actual.y - expected.y).abs() < 0.0001, "y mismatch: actual={actual:?} expected={expected:?}");
    }

    #[test]
    fn resolved_scene_perspective_matches_direct_plane_attachment() {
        let anchor = dvec2(640.0, 220.0);
        let rotation = vec3(0.0, -0.50, 0.0);
        let distance = 980.0;
        let direct_rotation = local_card_rotation(rotation);
        let direct_perspective = local_card_perspective(distance);
        let scene_transform = card_leaf_transform(anchor, rotation, 0.0);
        let scene_perspective = card_leaf_perspective(anchor, distance);

        for point in [
            dvec2(0.0, 0.0),
            dvec2(CARD_W, 0.0),
            dvec2(0.0, CARD_H),
            dvec2(CARD_W, CARD_H),
        ] {
            let direct = anchor + project_xy(Mat4f::mul(&direct_perspective, &direct_rotation), point);
            let scene = project_xy(Mat4f::mul(&scene_perspective, &scene_transform), point);
            assert_close(scene, direct);
        }
    }
}
