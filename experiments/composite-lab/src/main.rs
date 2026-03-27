pub use makepad_widgets;

use makepad_compositor::{
    MpBackfaceVisibility, MpClipNode, MpClipShape, MpEffectNode, MpNode, MpReferenceFrame,
    MpRenderer, MpScene, MpSceneRoot, MpSurface, MpSurfaceColorFormat, MpSurfaceNode,
    MpSurfaceSource, MpTransformStyle,
};
use makepad_widgets::makepad_draw::text::geom::Point as TextPoint;
use makepad_widgets::*;
use makepad_zune_png::{
    makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions},
    PngEncoder,
};
use std::path::PathBuf;

const MODE_CORE: usize = 0;
const MODE_NESTED: usize = 1;
const MODE_CLIP: usize = 2;
const MODE_MIXED: usize = 3;
const MODE_CSS_MATCH: usize = 4;
const MODE_OPACITY: usize = 5;
const MODE_ALL: usize = 6;
const MODE_NAMES: [&str; 7] = ["core", "nested", "clip", "mixed", "css-match", "opacity", "all"];

app_main!(App);

const PANEL_W: f64 = 220.0;
const PANEL_H: f64 = 180.0;
const GRID_GAP_X: f64 = 36.0;
const GRID_GAP_Y: f64 = 54.0;
const ROOT_PAD: f64 = 28.0;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DrawListCoordProbeBase = #(DrawListCoordProbe::register_widget(vm))
    mod.widgets.DrawListCoordProbe = set_type_default() do mod.widgets.DrawListCoordProbeBase{
        width: Fill
        height: Fill
        draw_bg: mod.draw.DrawColor{color: #fff}
        draw_panel_bg: mod.draw.DrawColor{color: #f3f3f3}
        draw_border: mod.draw.DrawColor{color: #222}
        draw_green: mod.draw.DrawColor{color: #0b0}
        draw_blue: mod.draw.DrawColor{color: #00fa}
        draw_orange: mod.draw.DrawColor{color: #fa0}
        draw_purple: mod.draw.DrawColor{color: #800080}
        draw_black: mod.draw.DrawColor{color: #000}
        draw_red: mod.draw.DrawColor{color: #f00}
        draw_gray: mod.draw.DrawColor{color: #bbb}
        draw_text: mod.draw.DrawText{
            text_style: theme.font_regular{
                font_size: theme.font_size_p
            }
            color: #111
        }
        draw_text_light: mod.draw.DrawText{
            text_style: theme.font_regular{
                font_size: theme.font_size_p
            }
            color: #fff
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1680, 1280)
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

                            probe := mod.widgets.DrawListCoordProbe{}
                        }
                    }
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
        self.capture_path =
            PathBuf::from(format!("/tmp/drawlist-coords-{}.png", MODE_NAMES[self.mode_index]));
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
                cx.redraw_all();
                self.request_capture(cx);
            } else {
                self.capture_poll = None;
            }
            return;
        }

        self.capture_poll = Some(cx.new_next_frame());
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.mode_index = MODE_CORE;
        self.request_capture(cx);
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
        if let Some(next) = self.capture_poll {
            if next.is_event(event).is_some() {
                self.poll_capture(cx);
            }
        }
        if let Some(mut probe) = self.ui.widget(cx, ids!(probe)).borrow_mut::<DrawListCoordProbe>() {
            probe.mode_index = self.mode_index;
        }
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct DrawListCoordProbe {
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
    draw_panel_bg: DrawColor,
    #[live]
    draw_border: DrawColor,
    #[live]
    draw_green: DrawColor,
    #[live]
    draw_blue: DrawColor,
    #[live]
    draw_orange: DrawColor,
    #[live]
    draw_black: DrawColor,
    #[live]
    draw_red: DrawColor,
    #[live]
    draw_gray: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_text_light: DrawText,
    #[rust]
    area: Area,
    #[rust]
    renderer: Option<MpRenderer>,
    #[rust]
    surface_translate: Option<MpSurface>,
    #[rust]
    surface_rotate: Option<MpSurface>,
    #[rust]
    surface_skew: Option<MpSurface>,
    #[rust]
    surface_scale: Option<MpSurface>,
    #[rust]
    surface_nested_parent: Option<MpSurface>,
    #[rust]
    surface_nested_child: Option<MpSurface>,
    #[rust]
    surface_clip: Option<MpSurface>,
    #[rust]
    surface_mixed: Option<MpSurface>,
    #[rust]
    surface_translate_draw_list: Option<DrawList2d>,
    #[rust]
    surface_rotate_draw_list: Option<DrawList2d>,
    #[rust]
    surface_skew_draw_list: Option<DrawList2d>,
    #[rust]
    surface_scale_draw_list: Option<DrawList2d>,
    #[rust]
    surface_nested_parent_draw_list: Option<DrawList2d>,
    #[rust]
    surface_nested_child_draw_list: Option<DrawList2d>,
    #[rust]
    surface_clip_draw_list: Option<DrawList2d>,
    #[rust]
    surface_mixed_draw_list: Option<DrawList2d>,
    #[rust]
    surface_css_orange: Option<MpSurface>,
    #[rust]
    surface_css_orange_draw_list: Option<DrawList2d>,
    #[rust]
    surface_css_purple: Option<MpSurface>,
    #[rust]
    surface_css_purple_draw_list: Option<DrawList2d>,
    #[rust]
    surface_opacity_blue: Option<MpSurface>,
    #[rust]
    surface_opacity_red: Option<MpSurface>,
    #[rust]
    surface_opacity_green: Option<MpSurface>,
    #[rust]
    surface_opacity_black: Option<MpSurface>,
    #[rust]
    surface_opacity_blue_draw_list: Option<DrawList2d>,
    #[rust]
    surface_opacity_red_draw_list: Option<DrawList2d>,
    #[rust]
    surface_opacity_green_draw_list: Option<DrawList2d>,
    #[rust]
    surface_opacity_black_draw_list: Option<DrawList2d>,
    #[live]
    draw_purple: DrawColor,
    #[rust]
    mode_index: usize,
}

fn translation(tx: f32, ty: f32) -> Mat4f {
    Mat4f::translation(vec3(tx, ty, 0.0))
}

fn scale(sx: f32, sy: f32) -> Mat4f {
    Mat4f {
        v: [
            sx, 0.0, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
    }
}

fn rotation_degrees(deg: f32) -> Mat4f {
    Mat4f::rotation(vec3(0.0, 0.0, deg.to_radians()))
}

fn skew_x_degrees(deg: f32) -> Mat4f {
    let t = deg.to_radians().tan();
    Mat4f {
        v: [
            1.0, 0.0, 0.0, 0.0, t, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
    }
}

fn panel_rect(root: DVec2, col: usize, row: usize) -> Rect {
    Rect {
        pos: root + dvec2(col as f64 * (PANEL_W + GRID_GAP_X), row as f64 * (PANEL_H + GRID_GAP_Y)),
        size: dvec2(PANEL_W, PANEL_H),
    }
}

fn bottom_bar(panel: Rect) -> Rect {
    Rect {
        pos: panel.pos + dvec2(0.0, panel.size.y - 16.0),
        size: dvec2(panel.size.x, 16.0),
    }
}

fn content_rect(panel: Rect) -> Rect {
    Rect {
        pos: panel.pos + dvec2(20.0, 20.0),
        size: dvec2(160.0, 120.0),
    }
}

fn title_pos(panel: Rect) -> DVec2 {
    panel.pos + dvec2(12.0, 14.0)
}

fn footer_text_pos(panel: Rect) -> DVec2 {
    panel.pos + dvec2(12.0, panel.size.y - 18.0)
}

fn parent_local_transform_around(origin: DVec2, transform: Mat4f) -> Mat4f {
    Mat4f::mul(
        &Mat4f::translation(vec3(origin.x as f32, origin.y as f32, 0.0)),
        &Mat4f::mul(
            &transform,
            &Mat4f::translation(vec3(-(origin.x as f32), -(origin.y as f32), 0.0)),
        ),
    )
}

impl DrawListCoordProbe {
    fn ensure_resources(&mut self, cx: &mut Cx) {
        if self.renderer.is_none() {
            self.renderer = Some(MpRenderer::new(cx));
        }

        macro_rules! ensure_surface {
            ($slot:ident) => {
                if self.$slot.is_none() {
                    self.$slot = Some(MpSurface::new(
                        cx,
                        dvec2(PANEL_W, PANEL_H),
                        MpSurfaceColorFormat::BgraU8,
                        false,
                    ));
                }
            };
        }
        ensure_surface!(surface_translate);
        ensure_surface!(surface_rotate);
        ensure_surface!(surface_skew);
        ensure_surface!(surface_scale);
        ensure_surface!(surface_nested_parent);
        ensure_surface!(surface_nested_child);
        ensure_surface!(surface_clip);
        ensure_surface!(surface_mixed);
        ensure_surface!(surface_css_orange);
        ensure_surface!(surface_css_purple);
        ensure_surface!(surface_opacity_blue);
        ensure_surface!(surface_opacity_red);
        ensure_surface!(surface_opacity_green);
        ensure_surface!(surface_opacity_black);

        macro_rules! ensure_draw_list {
            ($slot:ident) => {
                if self.$slot.is_none() {
                    self.$slot = Some(DrawList2d::new(cx));
                }
            };
        }
        ensure_draw_list!(surface_translate_draw_list);
        ensure_draw_list!(surface_rotate_draw_list);
        ensure_draw_list!(surface_skew_draw_list);
        ensure_draw_list!(surface_scale_draw_list);
        ensure_draw_list!(surface_nested_parent_draw_list);
        ensure_draw_list!(surface_nested_child_draw_list);
        ensure_draw_list!(surface_clip_draw_list);
        ensure_draw_list!(surface_mixed_draw_list);
        ensure_draw_list!(surface_css_orange_draw_list);
        ensure_draw_list!(surface_css_purple_draw_list);
        ensure_draw_list!(surface_opacity_blue_draw_list);
        ensure_draw_list!(surface_opacity_red_draw_list);
        ensure_draw_list!(surface_opacity_green_draw_list);
        ensure_draw_list!(surface_opacity_black_draw_list);
    }

    fn draw_glyph_text(
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

    fn draw_direct_panel(
        cx: &mut Cx2d,
        panel_bg: &mut DrawColor,
        border: &mut DrawColor,
        fill: &mut DrawColor,
        footer_bg: &mut DrawColor,
        draw_text: &mut DrawText,
        draw_text_light: &mut DrawText,
        panel: Rect,
        title: &str,
        footer: &str,
    ) {
        panel_bg.draw_abs(cx, panel);
        border.draw_abs(
            cx,
            Rect {
                pos: panel.pos,
                size: dvec2(panel.size.x, 2.0),
            },
        );
        border.draw_abs(
            cx,
            Rect {
                pos: panel.pos + dvec2(0.0, panel.size.y - 2.0),
                size: dvec2(panel.size.x, 2.0),
            },
        );
        border.draw_abs(
            cx,
            Rect {
                pos: panel.pos,
                size: dvec2(2.0, panel.size.y),
            },
        );
        border.draw_abs(
            cx,
            Rect {
                pos: panel.pos + dvec2(panel.size.x - 2.0, 0.0),
                size: dvec2(2.0, panel.size.y),
            },
        );
        fill.draw_abs(cx, content_rect(panel));
        footer_bg.draw_abs(cx, bottom_bar(panel));
        Self::draw_glyph_text(
            cx,
            draw_text,
            title,
            title_pos(panel),
            vec4(0.07, 0.07, 0.07, 1.0),
        );
        Self::draw_glyph_text(
            cx,
            draw_text_light,
            footer,
            footer_text_pos(panel),
            vec4(1.0, 1.0, 1.0, 1.0),
        );
    }

    fn render_surface_panel(
        cx: &mut Cx2d,
        surface: &mut MpSurface,
        draw_list: &mut DrawList2d,
        panel_bg: &mut DrawColor,
        border: &mut DrawColor,
        fill: &mut DrawColor,
        footer_bg: &mut DrawColor,
        draw_text: &mut DrawText,
        draw_text_light: &mut DrawText,
        title: &str,
        footer: &str,
    ) {
        surface.resize(cx.cx, dvec2(PANEL_W, PANEL_H));
        surface.begin(cx, None);
        cx.set_pass_shift_scale(surface.pass(), dvec2(0.0, 0.0), dvec2(1.0, 1.0));
        draw_list.begin_always(cx);
        cx.begin_root_turtle_for_pass(Layout::default());
        panel_bg.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(PANEL_W, PANEL_H),
            },
        );
        border.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(PANEL_W, 2.0),
            },
        );
        border.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, PANEL_H - 2.0),
                size: dvec2(PANEL_W, 2.0),
            },
        );
        border.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(2.0, PANEL_H),
            },
        );
        border.draw_abs(
            cx,
            Rect {
                pos: dvec2(PANEL_W - 2.0, 0.0),
                size: dvec2(2.0, PANEL_H),
            },
        );
        fill.draw_abs(
            cx,
            Rect {
                pos: dvec2(20.0, 20.0),
                size: dvec2(160.0, 120.0),
            },
        );
        footer_bg.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, PANEL_H - 16.0),
                size: dvec2(PANEL_W, 16.0),
            },
        );
        Self::draw_glyph_text(cx, draw_text, title, dvec2(12.0, 14.0), vec4(0.07, 0.07, 0.07, 1.0));
        Self::draw_glyph_text(
            cx,
            draw_text_light,
            footer,
            dvec2(12.0, PANEL_H - 18.0),
            vec4(1.0, 1.0, 1.0, 1.0),
        );
        cx.end_pass_sized_turtle();
        draw_list.end(cx);
        surface.end(cx);
    }

    fn render_solid_surface(
        cx: &mut Cx2d,
        surface: &mut MpSurface,
        draw_list: &mut DrawList2d,
        fill: &mut DrawColor,
        size: DVec2,
    ) {
        surface.resize(cx.cx, size);
        surface.begin(cx, None);
        cx.set_pass_shift_scale(surface.pass(), dvec2(0.0, 0.0), dvec2(1.0, 1.0));
        draw_list.begin_always(cx);
        cx.begin_root_turtle_for_pass(Layout::default());
        fill.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, 0.0),
                size,
            },
        );
        cx.end_pass_sized_turtle();
        draw_list.end(cx);
        surface.end(cx);
    }

    fn basic_scene(host_rect: Rect) -> MpScene {
        MpScene::new(MpSceneRoot {
            host_rect,
            page_to_host: Mat4f::identity(),
            clip: None,
        })
    }

    fn draw_scene(&mut self, cx: &mut Cx2d, scene: &MpScene) {
        if let Err(err) = self.renderer.as_mut().unwrap().draw_scene(cx, scene) {
            println!("draw_scene error: {:?}", err);
        }
    }

    fn add_surface_node(
        scene: &mut MpScene,
        parent: Option<usize>,
        clip: Option<usize>,
        local_rect: Rect,
        transform: Mat4f,
        texture: Texture,
    ) {
        let rf = scene.push(MpNode::ReferenceFrame(MpReferenceFrame {
            parent,
            clip,
            local_rect,
            transform,
            perspective: None,
            transform_style: MpTransformStyle::Flat,
            backface_visibility: MpBackfaceVisibility::Visible,
            flattens_descendants: true,
        }));
        scene.push(MpNode::Surface(MpSurfaceNode {
            parent: rf,
            clip,
            local_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: local_rect.size,
            },
            source: MpSurfaceSource::SurfaceTexture(texture),

            backface_visibility: MpBackfaceVisibility::Visible,
        }));
    }

    fn draw_core_section(&mut self, cx: &mut Cx2d, root: DVec2) {
        let direct_a = panel_rect(root, 0, 0);
        let direct_b = panel_rect(root, 1, 0);
        let direct_c = panel_rect(root, 2, 0);
        let composed_translate = panel_rect(root, 0, 1);
        let composed_rotate = panel_rect(root, 1, 1);
        let composed_skew = panel_rect(root, 2, 1);
        let composed_scale = panel_rect(root, 0, 2);

        Self::draw_direct_panel(
            cx,
            &mut self.draw_panel_bg,
            &mut self.draw_border,
            &mut self.draw_green,
            &mut self.draw_black,
            &mut self.draw_text,
            &mut self.draw_text_light,
            direct_a,
            "direct baseline",
            "parent local",
        );
        Self::draw_direct_panel(
            cx,
            &mut self.draw_panel_bg,
            &mut self.draw_border,
            &mut self.draw_blue,
            &mut self.draw_black,
            &mut self.draw_text,
            &mut self.draw_text_light,
            direct_b,
            "direct translated",
            "parent local",
        );
        Self::draw_direct_panel(
            cx,
            &mut self.draw_panel_bg,
            &mut self.draw_border,
            &mut self.draw_orange,
            &mut self.draw_black,
            &mut self.draw_text,
            &mut self.draw_text_light,
            direct_c,
            "surface contract",
            "attach + transform",
        );

        Self::render_surface_panel(
            cx,
            self.surface_translate.as_mut().unwrap(),
            self.surface_translate_draw_list.as_mut().unwrap(),
            &mut self.draw_panel_bg,
            &mut self.draw_border,
            &mut self.draw_green,
            &mut self.draw_black,
            &mut self.draw_text,
            &mut self.draw_text_light,
            "translated surface",
            "attach only",
        );
        Self::render_surface_panel(
            cx,
            self.surface_rotate.as_mut().unwrap(),
            self.surface_rotate_draw_list.as_mut().unwrap(),
            &mut self.draw_panel_bg,
            &mut self.draw_border,
            &mut self.draw_blue,
            &mut self.draw_black,
            &mut self.draw_text,
            &mut self.draw_text_light,
            "rotate + translate",
            "center origin",
        );
        Self::render_surface_panel(
            cx,
            self.surface_skew.as_mut().unwrap(),
            self.surface_skew_draw_list.as_mut().unwrap(),
            &mut self.draw_panel_bg,
            &mut self.draw_border,
            &mut self.draw_orange,
            &mut self.draw_black,
            &mut self.draw_text,
            &mut self.draw_text_light,
            "skew + rotate",
            "top-left then center",
        );
        self.surface_scale.as_mut().unwrap().resize(cx.cx, dvec2(PANEL_W, PANEL_H));
        self.surface_scale.as_mut().unwrap().begin(cx, None);
        cx.set_pass_shift_scale(
            self.surface_scale.as_ref().unwrap().pass(),
            dvec2(0.0, 0.0),
            dvec2(1.0, 1.0),
        );
        self.surface_scale_draw_list.as_mut().unwrap().begin_always(cx);
        cx.begin_root_turtle_for_pass(Layout::default());
        self.draw_panel_bg.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(PANEL_W, PANEL_H),
            },
        );
        self.draw_border.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(PANEL_W, 2.0),
            },
        );
        self.draw_border.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, PANEL_H - 2.0),
                size: dvec2(PANEL_W, 2.0),
            },
        );
        self.draw_border.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(2.0, PANEL_H),
            },
        );
        self.draw_border.draw_abs(
            cx,
            Rect {
                pos: dvec2(PANEL_W - 2.0, 0.0),
                size: dvec2(2.0, PANEL_H),
            },
        );
        self.draw_green.draw_abs(
            cx,
            Rect {
                pos: dvec2(20.0, 20.0),
                size: dvec2(160.0, 120.0),
            },
        );
        self.draw_blue.draw_abs(
            cx,
            Rect {
                pos: dvec2(144.0, 28.0),
                size: dvec2(24.0, 24.0),
            },
        );
        self.draw_orange.draw_abs(
            cx,
            Rect {
                pos: dvec2(34.0, 132.0),
                size: dvec2(52.0, 18.0),
            },
        );
        self.draw_black.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, PANEL_H - 16.0),
                size: dvec2(PANEL_W, 16.0),
            },
        );
        Self::draw_glyph_text(
            cx,
            &mut self.draw_text,
            "scaled surface",
            dvec2(12.0, 14.0),
            vec4(0.07, 0.07, 0.07, 1.0),
        );
        Self::draw_glyph_text(
            cx,
            &mut self.draw_text_light,
            "explicit origin",
            dvec2(12.0, PANEL_H - 18.0),
            vec4(1.0, 1.0, 1.0, 1.0),
        );
        cx.end_pass_sized_turtle();
        self.surface_scale_draw_list.as_mut().unwrap().end(cx);
        self.surface_scale.as_mut().unwrap().end(cx);

        let rotate_origin = composed_rotate.pos + composed_rotate.size * 0.5;
        let skew_origin = composed_skew.pos;
        let scale_origin = composed_scale.pos + dvec2(18.0, 18.0);

        let mut scene = Self::basic_scene(Rect {
            pos: dvec2(0.0, 0.0),
            size: cx.current_pass_size(),
        });
        Self::add_surface_node(
            &mut scene,
            None,
            None,
            composed_translate,
            translation(composed_translate.pos.x as f32, composed_translate.pos.y as f32),
            self.surface_translate.as_ref().unwrap().color_texture().clone(),
        );
        Self::add_surface_node(
            &mut scene,
            None,
            None,
            composed_rotate,
            Mat4f::mul(
                &translation(14.0, -8.0),
                &Mat4f::mul(
                    &parent_local_transform_around(rotate_origin, rotation_degrees(18.0)),
                    &translation(composed_rotate.pos.x as f32, composed_rotate.pos.y as f32),
                ),
            ),
            self.surface_rotate.as_ref().unwrap().color_texture().clone(),
        );
        Self::add_surface_node(
            &mut scene,
            None,
            None,
            composed_skew,
            Mat4f::mul(
                &parent_local_transform_around(
                    composed_skew.pos + composed_skew.size * 0.5,
                    rotation_degrees(-8.0),
                ),
                &Mat4f::mul(
                    &parent_local_transform_around(skew_origin, skew_x_degrees(16.0)),
                    &translation(composed_skew.pos.x as f32, composed_skew.pos.y as f32),
                ),
            ),
            self.surface_skew.as_ref().unwrap().color_texture().clone(),
        );
        Self::add_surface_node(
            &mut scene,
            None,
            None,
            composed_scale,
            Mat4f::mul(
                &parent_local_transform_around(scale_origin, scale(0.82, 1.18)),
                &translation(composed_scale.pos.x as f32, composed_scale.pos.y as f32),
            ),
            self.surface_scale.as_ref().unwrap().color_texture().clone(),
        );

        self.draw_red.draw_abs(
            cx,
            Rect {
                pos: rotate_origin - dvec2(4.0, 4.0),
                size: dvec2(8.0, 8.0),
            },
        );
        self.draw_red.draw_abs(
            cx,
            Rect {
                pos: skew_origin - dvec2(4.0, 4.0),
                size: dvec2(8.0, 8.0),
            },
        );
        self.draw_red.draw_abs(
            cx,
            Rect {
                pos: scale_origin - dvec2(4.0, 4.0),
                size: dvec2(8.0, 8.0),
            },
        );

        self.draw_scene(cx, &scene);

        for panel in [composed_translate, composed_rotate, composed_skew, composed_scale] {
            self.draw_border.draw_abs(
                cx,
                Rect {
                    pos: panel.pos + dvec2(0.0, PANEL_H + 6.0),
                    size: dvec2(PANEL_W, 2.0),
                },
            );
        }
    }

    fn draw_nested_section(&mut self, cx: &mut Cx2d, root: DVec2) {
        let nested_panel = panel_rect(root, 0, 0);

        let child = self.surface_nested_child.as_mut().unwrap();
        child.resize(cx.cx, dvec2(120.0, 96.0));
        child.begin(cx, None);
        cx.set_pass_shift_scale(child.pass(), dvec2(0.0, 0.0), dvec2(1.0, 1.0));
        self.surface_nested_child_draw_list.as_mut().unwrap().begin_always(cx);
        cx.begin_root_turtle_for_pass(Layout::default());
        self.draw_blue.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(120.0, 96.0),
            },
        );
        self.draw_black.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, 80.0),
                size: dvec2(120.0, 16.0),
            },
        );
        Self::draw_glyph_text(
            cx,
            &mut self.draw_text_light,
            "child surface",
            dvec2(8.0, 14.0),
            vec4(1.0, 1.0, 1.0, 1.0),
        );
        cx.end_pass_sized_turtle();
        self.surface_nested_child_draw_list.as_mut().unwrap().end(cx);
        child.end(cx);

        let child_texture = child.color_texture().clone();

        {
            let parent = self.surface_nested_parent.as_mut().unwrap();
            parent.begin(cx, None);
            cx.set_pass_shift_scale(parent.pass(), dvec2(0.0, 0.0), dvec2(1.0, 1.0));
            self.surface_nested_parent_draw_list.as_mut().unwrap().begin_always(cx);
            cx.begin_root_turtle_for_pass(Layout::default());
            self.draw_panel_bg.draw_abs(
                cx,
                Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(PANEL_W, PANEL_H),
                },
            );
            self.draw_border.draw_abs(
                cx,
                Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(PANEL_W, 2.0),
                },
            );
            self.draw_border.draw_abs(
                cx,
                Rect {
                    pos: dvec2(0.0, PANEL_H - 2.0),
                    size: dvec2(PANEL_W, 2.0),
                },
            );
            self.draw_border.draw_abs(
                cx,
                Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(2.0, PANEL_H),
                },
            );
            self.draw_border.draw_abs(
                cx,
                Rect {
                    pos: dvec2(PANEL_W - 2.0, 0.0),
                    size: dvec2(2.0, PANEL_H),
                },
            );
            self.draw_green.draw_abs(
                cx,
                Rect {
                    pos: dvec2(20.0, 20.0),
                    size: dvec2(70.0, 120.0),
                },
            );
            self.draw_black.draw_abs(
                cx,
                Rect {
                    pos: dvec2(0.0, PANEL_H - 16.0),
                    size: dvec2(PANEL_W, 16.0),
                },
            );

            let mut parent_renderer = self.renderer.take().unwrap();
            let mut parent_scene = MpScene::new(MpSceneRoot {
                host_rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(PANEL_W, PANEL_H),
                },
                page_to_host: Mat4f::identity(),
                clip: None,
            });
            Self::add_surface_node(
                &mut parent_scene,
                None,
                None,
                Rect {
                    pos: dvec2(88.0, 32.0),
                    size: dvec2(120.0, 96.0),
                },
                translation(88.0, 32.0),
                child_texture,
            );
            let _ = parent_renderer.draw_scene(cx, &parent_scene);
            self.renderer = Some(parent_renderer);
            Self::draw_glyph_text(
                cx,
                &mut self.draw_text,
                "nested parent",
                dvec2(12.0, 14.0),
                vec4(0.07, 0.07, 0.07, 1.0),
            );
            cx.end_pass_sized_turtle();
            self.surface_nested_parent_draw_list.as_mut().unwrap().end(cx);
            parent.end(cx);
        }

        let parent_texture = self.surface_nested_parent.as_ref().unwrap().color_texture().clone();
        let mut scene = Self::basic_scene(Rect {
            pos: dvec2(0.0, 0.0),
            size: cx.current_pass_size(),
        });
        Self::add_surface_node(
            &mut scene,
            None,
            None,
            nested_panel,
            Mat4f::mul(
                &parent_local_transform_around(
                    nested_panel.pos + nested_panel.size * 0.5,
                    rotation_degrees(8.0),
                ),
                &translation(nested_panel.pos.x as f32, nested_panel.pos.y as f32),
            ),
            parent_texture,
        );
        self.draw_scene(cx, &scene);
        self.draw_border.draw_abs(
            cx,
            Rect {
                pos: nested_panel.pos + dvec2(0.0, PANEL_H + 6.0),
                size: dvec2(PANEL_W, 2.0),
            },
        );
    }

    fn draw_clip_section(&mut self, cx: &mut Cx2d, root: DVec2) {
        let clipped_panel = panel_rect(root, 0, 0);
        let clip_surface = self.surface_clip.as_mut().unwrap();
        clip_surface.resize(cx.cx, dvec2(PANEL_W, PANEL_H));
        clip_surface.begin(cx, None);
        cx.set_pass_shift_scale(clip_surface.pass(), dvec2(0.0, 0.0), dvec2(1.0, 1.0));
        self.surface_clip_draw_list.as_mut().unwrap().begin_always(cx);
        cx.begin_root_turtle_for_pass(Layout::default());
        self.draw_panel_bg.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(PANEL_W, PANEL_H),
            },
        );
        self.draw_border.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(PANEL_W, 2.0),
            },
        );
        self.draw_border.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, PANEL_H - 2.0),
                size: dvec2(PANEL_W, 2.0),
            },
        );
        self.draw_border.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(2.0, PANEL_H),
            },
        );
        self.draw_border.draw_abs(
            cx,
            Rect {
                pos: dvec2(PANEL_W - 2.0, 0.0),
                size: dvec2(2.0, PANEL_H),
            },
        );
        let viewport = Rect {
            pos: dvec2(26.0, 38.0),
            size: dvec2(140.0, 84.0),
        };
        self.draw_gray.draw_abs(cx, viewport);
        cx.begin_page_root_turtle(viewport.pos, viewport.size, Layout::default());
        self.draw_orange.draw_abs(
            cx,
            Rect {
                pos: dvec2(-34.0, 10.0),
                size: dvec2(160.0, 70.0),
            },
        );
        Self::draw_glyph_text(
            cx,
            &mut self.draw_text,
            "translated inside clip",
            dvec2(-28.0, 16.0),
            vec4(0.07, 0.07, 0.07, 1.0),
        );
        cx.end_pass_sized_turtle();
        self.draw_black.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, PANEL_H - 16.0),
                size: dvec2(PANEL_W, 16.0),
            },
        );
        Self::draw_glyph_text(
            cx,
            &mut self.draw_text,
            "clipped viewport",
            dvec2(12.0, 14.0),
            vec4(0.07, 0.07, 0.07, 1.0),
        );
        Self::draw_glyph_text(
            cx,
            &mut self.draw_text_light,
            "viewport clip",
            dvec2(12.0, PANEL_H - 18.0),
            vec4(1.0, 1.0, 1.0, 1.0),
        );
        cx.end_pass_sized_turtle();
        self.surface_clip_draw_list.as_mut().unwrap().end(cx);
        clip_surface.end(cx);

        let clip_origin = clipped_panel.pos + clipped_panel.size * 0.5;
        let mut scene = Self::basic_scene(Rect {
            pos: dvec2(0.0, 0.0),
            size: cx.current_pass_size(),
        });
        let clip_id = scene.push(MpNode::Clip(MpClipNode {
            parent: None,
            prev: None,
            shape: MpClipShape::PlaneSet {
                planes: vec![
                    vec4(1.0, 0.0, 0.0, -(clipped_panel.pos.x as f32)),
                    vec4(-1.0, 0.0, 0.0, (clipped_panel.pos.x + clipped_panel.size.x) as f32),
                    vec4(0.0, 1.0, 0.0, -(clipped_panel.pos.y as f32)),
                    vec4(0.0, -1.0, 0.0, (clipped_panel.pos.y + clipped_panel.size.y) as f32),
                ],
            },
        }));
        Self::add_surface_node(
            &mut scene,
            None,
            Some(clip_id),
            clipped_panel,
            Mat4f::mul(
                &translation(10.0, -4.0),
                &Mat4f::mul(
                    &parent_local_transform_around(clip_origin, rotation_degrees(10.0)),
                    &translation(clipped_panel.pos.x as f32, clipped_panel.pos.y as f32),
                ),
            ),
            clip_surface.color_texture().clone(),
        );
        self.draw_red.draw_abs(
            cx,
            Rect {
                pos: clip_origin - dvec2(4.0, 4.0),
                size: dvec2(8.0, 8.0),
            },
        );
        self.draw_scene(cx, &scene);
        self.draw_border.draw_abs(
            cx,
            Rect {
                pos: clipped_panel.pos + dvec2(0.0, PANEL_H + 6.0),
                size: dvec2(PANEL_W, 2.0),
            },
        );
    }

    fn draw_mixed_section(&mut self, cx: &mut Cx2d, root: DVec2) {
        let mixed_panel = panel_rect(root, 0, 0);
        let mixed = self.surface_mixed.as_mut().unwrap();
        mixed.begin(cx, None);
        cx.set_pass_shift_scale(mixed.pass(), dvec2(0.0, 0.0), dvec2(1.0, 1.0));
        self.surface_mixed_draw_list.as_mut().unwrap().begin_always(cx);
        cx.begin_root_turtle_for_pass(Layout::default());
        self.draw_panel_bg.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(PANEL_W, PANEL_H),
            },
        );
        self.draw_border.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(PANEL_W, 2.0),
            },
        );
        self.draw_border.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, PANEL_H - 2.0),
                size: dvec2(PANEL_W, 2.0),
            },
        );
        self.draw_border.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(2.0, PANEL_H),
            },
        );
        self.draw_border.draw_abs(
            cx,
            Rect {
                pos: dvec2(PANEL_W - 2.0, 0.0),
                size: dvec2(2.0, PANEL_H),
            },
        );
        self.draw_green.draw_abs(
            cx,
            Rect {
                pos: dvec2(18.0, 26.0),
                size: dvec2(72.0, 94.0),
            },
        );
        self.draw_blue.draw_abs(
            cx,
            Rect {
                pos: dvec2(104.0, 26.0),
                size: dvec2(82.0, 46.0),
            },
        );
        self.draw_orange.draw_abs(
            cx,
            Rect {
                pos: dvec2(104.0, 82.0),
                size: dvec2(82.0, 38.0),
            },
        );
        self.draw_black.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, PANEL_H - 16.0),
                size: dvec2(PANEL_W, 16.0),
            },
        );
        Self::draw_glyph_text(
            cx,
            &mut self.draw_text,
            "mixed content",
            dvec2(12.0, 14.0),
            vec4(0.07, 0.07, 0.07, 1.0),
        );
        Self::draw_glyph_text(
            cx,
            &mut self.draw_text,
            "box + glyph + blocks",
            dvec2(26.0, 138.0),
            vec4(0.07, 0.07, 0.07, 1.0),
        );
        Self::draw_glyph_text(
            cx,
            &mut self.draw_text_light,
            "same local basis",
            dvec2(12.0, PANEL_H - 18.0),
            vec4(1.0, 1.0, 1.0, 1.0),
        );
        cx.end_pass_sized_turtle();
        self.surface_mixed_draw_list.as_mut().unwrap().end(cx);
        mixed.end(cx);

        let mut scene = Self::basic_scene(Rect {
            pos: dvec2(0.0, 0.0),
            size: cx.current_pass_size(),
        });
        Self::add_surface_node(
            &mut scene,
            None,
            None,
            mixed_panel,
            Mat4f::mul(
                &parent_local_transform_around(
                    mixed_panel.pos + mixed_panel.size * 0.5,
                    rotation_degrees(-6.0),
                ),
                &translation(mixed_panel.pos.x as f32, mixed_panel.pos.y as f32),
            ),
            mixed.color_texture().clone(),
        );
        self.draw_scene(cx, &scene);
        self.draw_border.draw_abs(
            cx,
            Rect {
                pos: mixed_panel.pos + dvec2(0.0, PANEL_H + 6.0),
                size: dvec2(PANEL_W, 2.0),
            },
        );
    }

    fn draw_opacity_section(&mut self, cx: &mut Cx2d, root: DVec2) {
        let reference_panel = panel_rect(root, 0, 0);
        let compositor_panel = panel_rect(root, 1, 0);
        let blue_rect = Rect {
            pos: dvec2(16.0, 28.0),
            size: dvec2(88.0, 88.0),
        };
        let red_rect = Rect {
            pos: dvec2(48.0, 60.0),
            size: dvec2(88.0, 88.0),
        };
        let green_rect = Rect {
            pos: dvec2(16.0, 148.0),
            size: dvec2(56.0, 24.0),
        };
        let black_rect = Rect {
            pos: dvec2(48.0, 148.0),
            size: dvec2(56.0, 24.0),
        };

        for (panel, title) in [
            (reference_panel, "direct reference"),
            (compositor_panel, "mp effect opacity"),
        ] {
            self.draw_panel_bg.draw_abs(cx, panel);
            self.draw_border.draw_abs(cx, Rect { pos: panel.pos, size: dvec2(panel.size.x, 2.0) });
            self.draw_border.draw_abs(
                cx,
                Rect {
                    pos: panel.pos + dvec2(0.0, panel.size.y - 2.0),
                    size: dvec2(panel.size.x, 2.0),
                },
            );
            self.draw_border.draw_abs(cx, Rect { pos: panel.pos, size: dvec2(2.0, panel.size.y) });
            self.draw_border.draw_abs(
                cx,
                Rect {
                    pos: panel.pos + dvec2(panel.size.x - 2.0, 0.0),
                    size: dvec2(2.0, panel.size.y),
                },
            );
            Self::draw_glyph_text(
                cx,
                &mut self.draw_text,
                title,
                title_pos(panel),
                vec4(0.07, 0.07, 0.07, 1.0),
            );
        }

        self.draw_blue.draw_abs(
            cx,
            Rect {
                pos: reference_panel.pos + blue_rect.pos,
                size: blue_rect.size,
            },
        );
        let direct_red = self.draw_red.color;
        self.draw_red.color = vec4(1.0, 0.0, 0.0, 0.5);
        self.draw_red.draw_abs(
            cx,
            Rect {
                pos: reference_panel.pos + red_rect.pos,
                size: red_rect.size,
            },
        );
        self.draw_red.color = direct_red;

        let direct_green = self.draw_green.color;
        let direct_gray = self.draw_gray.color;
        self.draw_green.color = vec4(126.0 / 255.0, 1.0, 126.0 / 255.0, 1.0);
        self.draw_gray.color = vec4(126.0 / 255.0, 126.0 / 255.0, 126.0 / 255.0, 1.0);
        self.draw_green.draw_abs(
            cx,
            Rect {
                pos: reference_panel.pos + green_rect.pos,
                size: dvec2(32.0, green_rect.size.y),
            },
        );
        self.draw_gray.draw_abs(
            cx,
            Rect {
                pos: reference_panel.pos + dvec2(48.0, green_rect.pos.y),
                size: dvec2(56.0, green_rect.size.y),
            },
        );
        self.draw_green.color = direct_green;
        self.draw_gray.color = direct_gray;

        Self::render_solid_surface(
            cx,
            self.surface_opacity_blue.as_mut().unwrap(),
            self.surface_opacity_blue_draw_list.as_mut().unwrap(),
            &mut self.draw_blue,
            blue_rect.size,
        );
        Self::render_solid_surface(
            cx,
            self.surface_opacity_red.as_mut().unwrap(),
            self.surface_opacity_red_draw_list.as_mut().unwrap(),
            &mut self.draw_red,
            red_rect.size,
        );
        Self::render_solid_surface(
            cx,
            self.surface_opacity_green.as_mut().unwrap(),
            self.surface_opacity_green_draw_list.as_mut().unwrap(),
            &mut self.draw_green,
            green_rect.size,
        );
        Self::render_solid_surface(
            cx,
            self.surface_opacity_black.as_mut().unwrap(),
            self.surface_opacity_black_draw_list.as_mut().unwrap(),
            &mut self.draw_black,
            black_rect.size,
        );

        let mut scene = Self::basic_scene(Rect {
            pos: dvec2(0.0, 0.0),
            size: cx.current_pass_size(),
        });
        let scene_root = scene.push(MpNode::ReferenceFrame(MpReferenceFrame {
            parent: None,
            clip: None,
            local_rect: compositor_panel,
            transform: Mat4f::identity(),
            perspective: None,
            transform_style: MpTransformStyle::Flat,
            backface_visibility: MpBackfaceVisibility::Visible,
            flattens_descendants: true,
        }));
        let element_opacity = scene.push(MpNode::Effect(MpEffectNode {
            parent: scene_root,
            clip: None,
            opacity: 0.5,
            filter: Default::default(),
            blend_mode: Default::default(),
            is_isolated: true,
            mask: None,
        }));
        let group_opacity = scene.push(MpNode::Effect(MpEffectNode {
            parent: scene_root,
            clip: None,
            opacity: 0.5,
            filter: Default::default(),
            blend_mode: Default::default(),
            is_isolated: true,
            mask: None,
        }));

        Self::add_surface_node(
            &mut scene,
            Some(scene_root),
            None,
            Rect {
                pos: compositor_panel.pos + blue_rect.pos,
                size: blue_rect.size,
            },
            translation(
                (compositor_panel.pos.x + blue_rect.pos.x) as f32,
                (compositor_panel.pos.y + blue_rect.pos.y) as f32,
            ),
            self.surface_opacity_blue.as_ref().unwrap().color_texture().clone(),
        );
        Self::add_surface_node(
            &mut scene,
            Some(element_opacity),
            None,
            Rect {
                pos: compositor_panel.pos + red_rect.pos,
                size: red_rect.size,
            },
            translation(
                (compositor_panel.pos.x + red_rect.pos.x) as f32,
                (compositor_panel.pos.y + red_rect.pos.y) as f32,
            ),
            self.surface_opacity_red.as_ref().unwrap().color_texture().clone(),
        );
        Self::add_surface_node(
            &mut scene,
            Some(group_opacity),
            None,
            Rect {
                pos: compositor_panel.pos + green_rect.pos,
                size: green_rect.size,
            },
            translation(
                (compositor_panel.pos.x + green_rect.pos.x) as f32,
                (compositor_panel.pos.y + green_rect.pos.y) as f32,
            ),
            self.surface_opacity_green.as_ref().unwrap().color_texture().clone(),
        );
        Self::add_surface_node(
            &mut scene,
            Some(group_opacity),
            None,
            Rect {
                pos: compositor_panel.pos + black_rect.pos,
                size: black_rect.size,
            },
            translation(
                (compositor_panel.pos.x + black_rect.pos.x) as f32,
                (compositor_panel.pos.y + black_rect.pos.y) as f32,
            ),
            self.surface_opacity_black.as_ref().unwrap().color_texture().clone(),
        );
        self.draw_scene(cx, &scene);
    }

    /// CSS-match test: reproduces the exact scenario from test-transform-ref.html.
    ///
    /// HTML: a 200×150 orange box with 3px border (total 206×156) at position
    /// (100,100), rotated 15deg around its center (transform-origin: 50% 50%).
    /// Inside: a purple 80×60 child at content offset (20,20).
    ///
    /// The CSS transform is: T(cx,cy) * R(15deg) * T(-cx,-cy)
    /// where (cx,cy) = center of border box = (103, 78).
    ///
    /// We test two approaches to see which matches the browser:
    ///
    /// Approach A ("HAVI current"): reference_frame.transform = T(placement) * css_matrix
    ///   where css_matrix already has the origin baked in.
    ///   Surface local_rect at (0,0).
    ///
    /// Approach B ("composite-lab style"): reference_frame.transform =
    ///   parent_local_transform_around(placement + center, R(15)) * T(placement)
    ///   Surface local_rect at (0,0).
    fn draw_css_match_section(&mut self, cx: &mut Cx2d, root: DVec2) {
        let box_w: f64 = 206.0;
        let box_h: f64 = 156.0;
        let box_x: f64 = 100.0;
        let box_y: f64 = 100.0;
        let border: f64 = 3.0;

        // Render the orange box surface (content in local coords)
        {
            let surf = self.surface_css_orange.as_mut().unwrap();
            surf.resize(cx.cx, dvec2(box_w, box_h));
            surf.begin(cx, None);
            cx.set_pass_shift_scale(surf.pass(), dvec2(0.0, 0.0), dvec2(1.0, 1.0));
            self.surface_css_orange_draw_list.as_mut().unwrap().begin_always(cx);
            cx.begin_root_turtle_for_pass(Layout::default());
            // Orange fill (whole box)
            self.draw_orange.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2(box_w, box_h) });
            // Border (4 edges)
            self.draw_border.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2(box_w, border) });
            self.draw_border.draw_abs(cx, Rect { pos: dvec2(0.0, box_h - border), size: dvec2(box_w, border) });
            self.draw_border.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2(border, box_h) });
            self.draw_border.draw_abs(cx, Rect { pos: dvec2(box_w - border, 0.0), size: dvec2(border, box_h) });
            // Purple child: border(3) + padding(0) + relative offset (20,20)
            // In CSS: .child { left: 20px; top: 20px } inside .container with no padding
            // Content area starts at border edge, so child is at (3+20, 3+20) = (23, 23)
            self.draw_purple.draw_abs(cx, Rect { pos: dvec2(border + 20.0, border + 20.0), size: dvec2(80.0, 60.0) });
            cx.end_pass_sized_turtle();
            self.surface_css_orange_draw_list.as_mut().unwrap().end(cx);
            surf.end(cx);
        }

        let orange_texture = self.surface_css_orange.as_ref().unwrap().color_texture().clone();

        // --- Approach A: HAVI-style (T(placement) * css_baked_matrix) ---
        // The CSS baked matrix for rotate(15deg) around center (103, 78):
        //   T(103,78) * R(15deg) * T(-103,-78)
        let cx_origin = box_w as f32 / 2.0; // 103
        let cy_origin = box_h as f32 / 2.0; // 78
        let cos15: f32 = 15.0_f32.to_radians().cos();
        let sin15: f32 = 15.0_f32.to_radians().sin();
        // T(cx,cy) * R * T(-cx,-cy) in column-major:
        let css_baked = Mat4f {
            v: [
                cos15, sin15, 0.0, 0.0,
                -sin15, cos15, 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                cx_origin - cx_origin * cos15 + cy_origin * sin15,
                cy_origin - cx_origin * sin15 - cy_origin * cos15,
                0.0, 1.0,
            ],
        };
        let placement = translation(box_x as f32, box_y as f32);

        // Approach A: transform = T(placement) * css_baked
        let transform_a = Mat4f::mul(&placement, &css_baked);
        eprintln!("[css-match] css_baked tx/ty = ({}, {})", css_baked.v[12], css_baked.v[13]);
        eprintln!("[css-match] A transform tx/ty = ({}, {})", transform_a.v[12], transform_a.v[13]);
        eprintln!("[css-match] A full matrix = {:?}", transform_a.v);

        // Draw approach A at the left side
        let label_a_pos = root;
        Self::draw_glyph_text(cx, &mut self.draw_text, "A: T(place) * css_baked", label_a_pos, vec4(0.07, 0.07, 0.07, 1.0));

        let mut scene_a = Self::basic_scene(Rect {
            pos: dvec2(0.0, 0.0),
            size: cx.current_pass_size(),
        });
        Self::add_surface_node(
            &mut scene_a,
            None, None,
            Rect { pos: dvec2(box_x, box_y), size: dvec2(box_w, box_h) },
            transform_a,
            orange_texture.clone(),
        );
        self.draw_scene(cx, &scene_a);

        // Red reference marker at (100, 100)
        self.draw_red.draw_abs(cx, Rect {
            pos: dvec2(box_x - 3.0, box_y - 3.0),
            size: dvec2(6.0, 6.0),
        });
        // Blue reference marker at center (200, 175) -- box_x + box_w/2, box_y + box_h/2
        self.draw_blue.draw_abs(cx, Rect {
            pos: dvec2(box_x + box_w / 2.0 - 3.0, box_y + box_h / 2.0 - 3.0),
            size: dvec2(6.0, 6.0),
        });

        // --- Approach B: composite-lab style (transform_around * T(placement)) ---
        let offset_b = dvec2(350.0, 0.0);
        let box_x_b = box_x + offset_b.x;
        let box_y_b = box_y + offset_b.y;

        let transform_b = Mat4f::mul(
            &parent_local_transform_around(
                dvec2(box_x_b + box_w / 2.0, box_y_b + box_h / 2.0),
                rotation_degrees(15.0),
            ),
            &translation(box_x_b as f32, box_y_b as f32),
        );

        eprintln!("[css-match] B transform tx/ty = ({}, {})", transform_b.v[12], transform_b.v[13]);
        eprintln!("[css-match] B full matrix = {:?}", transform_b.v);
        let label_b_pos = root + dvec2(350.0, 0.0);
        Self::draw_glyph_text(cx, &mut self.draw_text, "B: around(center) * T(place)", label_b_pos, vec4(0.07, 0.07, 0.07, 1.0));

        let mut scene_b = Self::basic_scene(Rect {
            pos: dvec2(0.0, 0.0),
            size: cx.current_pass_size(),
        });
        Self::add_surface_node(
            &mut scene_b,
            None, None,
            Rect { pos: dvec2(box_x_b, box_y_b), size: dvec2(box_w, box_h) },
            transform_b,
            orange_texture.clone(),
        );
        self.draw_scene(cx, &scene_b);

        self.draw_red.draw_abs(cx, Rect {
            pos: dvec2(box_x_b - 3.0, box_y_b - 3.0),
            size: dvec2(6.0, 6.0),
        });
        self.draw_blue.draw_abs(cx, Rect {
            pos: dvec2(box_x_b + box_w / 2.0 - 3.0, box_y_b + box_h / 2.0 - 3.0),
            size: dvec2(6.0, 6.0),
        });

        // --- Approach C: css_baked * T(placement) (reversed mul order) ---
        let offset_c = dvec2(700.0, 0.0);
        let box_x_c = box_x + offset_c.x;
        let box_y_c = box_y + offset_c.y;

        let placement_c = translation(box_x_c as f32, box_y_c as f32);
        let transform_c = Mat4f::mul(&css_baked, &placement_c);

        eprintln!("[css-match] C transform tx/ty = ({}, {})", transform_c.v[12], transform_c.v[13]);
        eprintln!("[css-match] C full matrix = {:?}", transform_c.v);
        let label_c_pos = root + dvec2(700.0, 0.0);
        Self::draw_glyph_text(cx, &mut self.draw_text, "C: css_baked * T(place)", label_c_pos, vec4(0.07, 0.07, 0.07, 1.0));

        let mut scene_c = Self::basic_scene(Rect {
            pos: dvec2(0.0, 0.0),
            size: cx.current_pass_size(),
        });
        Self::add_surface_node(
            &mut scene_c,
            None, None,
            Rect { pos: dvec2(box_x_c, box_y_c), size: dvec2(box_w, box_h) },
            transform_c,
            orange_texture.clone(),
        );
        self.draw_scene(cx, &scene_c);

        self.draw_red.draw_abs(cx, Rect {
            pos: dvec2(box_x_c - 3.0, box_y_c - 3.0),
            size: dvec2(6.0, 6.0),
        });
        self.draw_blue.draw_abs(cx, Rect {
            pos: dvec2(box_x_c + box_w / 2.0 - 3.0, box_y_c + box_h / 2.0 - 3.0),
            size: dvec2(6.0, 6.0),
        });
    }

}

impl Widget for DrawListCoordProbe {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_resources(cx.cx);
        cx.begin_turtle(walk, self.layout);
        let full_rect = cx.turtle().rect();
        let root = full_rect.pos + dvec2(ROOT_PAD, ROOT_PAD);
        self.draw_bg.draw_abs(cx, full_rect);

        match self.mode_index {
            MODE_CORE => self.draw_core_section(cx, root),
            MODE_NESTED => self.draw_nested_section(cx, root),
            MODE_CLIP => self.draw_clip_section(cx, root),
            MODE_MIXED => self.draw_mixed_section(cx, root),
            MODE_CSS_MATCH => self.draw_css_match_section(cx, root),
            MODE_OPACITY => self.draw_opacity_section(cx, root),
            _ => {
                self.draw_core_section(cx, root);
                self.draw_nested_section(
                    cx,
                    root + dvec2(PANEL_W + GRID_GAP_X, 2.0 * (PANEL_H + GRID_GAP_Y)),
                );
                self.draw_clip_section(
                    cx,
                    root + dvec2(2.0 * (PANEL_W + GRID_GAP_X), 2.0 * (PANEL_H + GRID_GAP_Y)),
                );
                self.draw_mixed_section(
                    cx,
                    root + dvec2(3.0 * (PANEL_W + GRID_GAP_X), 2.0 * (PANEL_H + GRID_GAP_Y)),
                );
            }
        }

        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}
