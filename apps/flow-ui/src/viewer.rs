//! Full-window media inspection. Images keep the eased fit/1:1 camera; video,
//! audio, mesh, and splat values use the shared viewers in the same frame.

use crate::values::{media_kind, MediaKind};
use makepad_flow::ValueBytes;
use makepad_media_view::{AudioPlayer, MeshView, SplatView, VideoPlayer};
use makepad_widgets::makepad_draw::event::{TouchState, TouchUpdateEvent};
use makepad_widgets::*;

const MIN_ZOOM: f64 = 0.05;
const MAX_ZOOM: f64 = 16.0;
const FIT_MARGIN: f64 = 24.0;
const ZOOM_STEP: f64 = 1.1;
const DRAG_THRESHOLD: f64 = 4.0;
const CLOSE_BUTTON_SIZE: f64 = 28.0;
const CLOSE_BUTTON_MARGIN: f64 = 16.0;
const GALLERY_BUTTON_WIDTH: f64 = 44.0;
const GALLERY_BUTTON_HEIGHT: f64 = 64.0;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ImageViewerBase = #(ImageViewer::register_widget(vm))

    let ViewerFade = CachedView{
        draw_bg +: {
            viewer_opacity: uniform(1.0)
            pixel: fn() {
                return self.image.sample(self.pos * self.scale + self.shift)
                    * self.viewer_opacity
            }
        }
    }

    let GalleryButton = ButtonFlat{
        width: Fill
        height: Fill
        margin: 0
        padding: 0
        draw_bg +: {
            color: #x17191eef
            color_hover: theme.flow_surface_hover
            color_down: theme.flow_surface_raised
            border_color: theme.flow_edge
            border_radius: 12.0
        }
        draw_text +: {
            color: theme.flow_text
            text_style: theme.font_regular{font_size: 28}
        }
    }

    mod.widgets.ImageViewer = set_type_default() do mod.widgets.ImageViewerBase{
        width: Fill
        height: Fill
        visible: false
        flow: Overlay
        grab_key_focus: true

        backdrop := SolidView{
            width: Fill
            height: Fill
            draw_bg +: {color: #x090a0df2}
        }
        stage := View{
            width: Fill
            height: Fill
            flow: Overlay
            picture := SolidView{
                width: 1
                height: 1
                flow: Overlay
                show_bg: true
                draw_bg +: {
                    pixel: fn() {
                        let screen_pos = self.rect_pos + self.pos * self.rect_size
                        let cell = floor(screen_pos.x / 10.0)
                            + floor(screen_pos.y / 10.0)
                        let shade = modf(cell, 2.0)
                        let color = #x202329.mix(#x2a2e34, shade)
                        return Pal.premul(color)
                    }
                }
                image := Image{
                    width: Fill
                    height: Fill
                    fit: ImageFit.Stretch
                    draw_bg +: {
                        // Negative forces pixels; positive carries DPI for the
                        // automatic high-magnification threshold.
                        get_color_scale_pan: fn(scale: vec2, pan: vec2) {
                            let uv = self.pos * scale + pan
                            if self.sample_mode < 0.0
                                || (self.image_dim_w > 0.0
                                    && self.rect_size.x / self.image_dim_w
                                        * self.sample_mode > 4.0) {
                                return self.image_texture.sample_nearest(uv)
                            }
                            return self.image_texture.sample_as_bgra(uv)
                        }
                    }
                }
            }
            media_layer := View{
                width: Fill
                height: Fill
                visible: false
                flow: Overlay
                video := mod.widgets.VideoPlayer{
                    width: Fill
                    height: Fill
                    visible: false
                }
                audio := mod.widgets.AudioPlayer{
                    width: Fill
                    height: Fill
                    visible: false
                }
                mesh := mod.widgets.MeshView{
                    width: Fill
                    height: Fill
                    visible: false
                }
                text_panel := View{
                    width: Fill
                    height: Fill
                    visible: false
                    padding: Inset{left: 64 right: 64 top: 56 bottom: 8}
                    document := TextInput{
                        width: Fill
                        height: Fill
                        is_multiline: true
                        is_read_only: true
                        empty_text: ""
                        draw_text +: {
                            color: theme.flow_text
                            text_style: theme.font_code{font_size: 11}
                        }
                    }
                }
                splat := mod.widgets.SplatView{
                    width: Fill
                    height: Fill
                    visible: false
                }
            }
        }
        bottom_wrap := View{
            width: Fill
            height: Fill
            flow: Down
            align: Align{y: 1.0}
            padding: Inset{left: 18 right: 18 bottom: 16}
            bar := ViewerFade{
                width: Fill
                height: 48
                flow: Overlay
                bar_surface := RoundedShadowView{
                    width: Fill
                    height: Fill
                    flow: Right
                    align: Align{y: 0.5}
                    spacing: theme.space_2
                    padding: Inset{left: 12 right: 10 top: 6 bottom: 6}
                    show_bg: true
                    draw_bg +: {
                        color: #x17191eef
                        border_color: #xffffff18
                        border_size: 1.0
                        border_radius: 10.0
                        shadow_color: #x00000088
                        shadow_radius: 14.0
                    }
                    type_label := Label{
                        width: Fit
                        height: Fit
                        draw_text +: {
                            color: theme.flow_accent
                            text_style: theme.font_bold{font_size: 8.5}
                        }
                    }
                    title := Label{
                        width: Fit
                        height: Fit
                        draw_text +: {
                            color: theme.flow_text
                            text_style: theme.font_bold{font_size: 10.0}
                        }
                    }
                    size := Label{
                        width: Fit
                        height: Fit
                        draw_text +: {
                            color: theme.flow_text_muted
                            text_style: theme.font_regular{font_size: 8.5}
                        }
                    }
                    zoom := Label{
                        width: Fill
                        height: Fit
                        draw_text +: {
                            color: theme.flow_text_muted
                            text_style: theme.font_regular{font_size: 8.5}
                        }
                    }
                    previous := ButtonFlatter{text: "← Previous"}
                    next := ButtonFlatter{text: "Next →"}
                    fit := ButtonFlat{text: "Fit"}
                    fit_width := ButtonFlat{text: "Fit width"}
                    actual := ButtonFlat{text: "1:1 px"}
                    double := ButtonFlat{text: "2:1"}
                    pixels := ButtonFlat{text: "Pixels: auto"}
                    save := Button{text: "Save…"}
                    copy := ButtonFlat{text: "Copy digest"}
                    close := ButtonFlatter{text: "×"}
                }
            }
        }
        previous_wrap := View{
            width: 44
            height: 64
            flow: Overlay
            previous_asset := GalleryButton{text: "‹"}
        }
        next_wrap := View{
            width: 44
            height: 64
            flow: Overlay
            next_asset := GalleryButton{text: "›"}
        }
        top_close := ViewerFade{
            width: 28
            height: 28
            margin: Inset{left: 16 top: 16}
            flow: Overlay
            top_close_button := ButtonFlatIcon{
                width: Fill
                height: Fill
                margin: 0
                padding: 0
                icon_walk: Walk{width: 12 height: 12}
                draw_bg +: {
                    color: theme.flow_surface_translucent
                    color_hover: theme.flow_surface_hover
                    color_down: theme.flow_surface_raised
                    color_focus: theme.flow_surface_hover
                    border_color: theme.flow_edge
                    border_color_hover: theme.flow_edge_soft
                    border_color_down: theme.flow_edge_soft
                    border_color_focus: theme.flow_edge_soft
                    border_radius: 14.0
                }
                draw_icon +: {
                    color: theme.flow_text
                    svg: crate_resource("self:resources/icons/close.svg")
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ImageViewerItem {
    pub node: String,
    pub port: String,
    pub bytes: ValueBytes,
}

#[derive(Clone, Debug, Default)]
pub enum ImageViewerAction {
    #[default]
    None,
    Close,
    Save,
    CopyDigest(String),
    Step(i32),
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    start: Vec2d,
    pan: Vec2d,
    outside: bool,
}

#[derive(Clone, Copy, Debug)]
struct Pinch {
    distance: f64,
    zoom: f64,
    pan: Vec2d,
    midpoint: Vec2d,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ImageViewer {
    #[deref]
    view: View,
    #[rust]
    item: Option<ImageViewerItem>,
    #[rust(1.0)]
    zoom: f64,
    #[rust(1.0)]
    target_zoom: f64,
    #[rust(1.0)]
    dpi_factor: f64,
    #[rust]
    pan: Vec2d,
    #[rust]
    target_pan: Vec2d,
    #[rust]
    drag: Option<Drag>,
    #[rust]
    pinch: Option<Pinch>,
    #[rust]
    image_size: Vec2d,
    #[rust]
    pixelated: bool,
    #[rust(1.0)]
    opacity: f64,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    last_time: f64,
    #[rust]
    picture_rect: Option<Rect>,
    #[rust]
    close_button_rect: Option<Rect>,
    #[rust]
    gallery_stage_rect: Option<Rect>,
    /// Video/audio controls are drawn by their media widgets at the bottom
    /// of their own surface. Keep that band clear of the viewer toolbar.
    #[rust]
    media_rect: Option<Rect>,
    #[rust]
    shown_percent: Option<i64>,
    #[rust]
    fit_mode: bool,
    #[rust]
    snap_to_fit: bool,
    #[rust]
    media_kind: MediaKind,
}

/// Layout-points-per-image-pixel scale which contains the image inside the
/// viewport while leaving an equal margin on all four sides.
fn fit_scale(viewport: Vec2d, image: Vec2d, margin: f64) -> f64 {
    if viewport.x <= 0.0 || viewport.y <= 0.0 || image.x <= 0.0 || image.y <= 0.0 {
        return 1.0;
    }
    let available = dvec2(
        (viewport.x - margin * 2.0).max(1.0),
        (viewport.y - margin * 2.0).max(1.0),
    );
    (available.x / image.x)
        .min(available.y / image.y)
        .max(1e-6)
}

fn fit_width_scale(viewport: Vec2d, image: Vec2d, margin: f64) -> f64 {
    if viewport.x <= 0.0 || image.x <= 0.0 {
        return 1.0;
    }
    ((viewport.x - margin * 2.0).max(1.0) / image.x).max(1e-6)
}

fn center_offset(viewport: Vec2d, image: Vec2d, scale: f64) -> Vec2d {
    (viewport - image * scale) * 0.5
}

fn close_button_rect(stage: Rect) -> Rect {
    Rect {
        pos: stage.pos + dvec2(CLOSE_BUTTON_MARGIN, CLOSE_BUTTON_MARGIN),
        size: dvec2(CLOSE_BUTTON_SIZE, CLOSE_BUTTON_SIZE),
    }
}

fn media_label(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => "IMAGE",
        MediaKind::Video => "VIDEO",
        MediaKind::Audio => "AUDIO",
        MediaKind::Mesh => "3D MESH",
        MediaKind::Splat => "3D SPLAT",
        MediaKind::Text => "TEXT",
        MediaKind::Unknown => "MEDIA",
    }
}

/// The image is centred on an axis while it is smaller than the viewport. On
/// an overflowing axis, its pan stops exactly when either image edge reaches
/// the corresponding viewport edge.
fn clamp_pan(viewport: Vec2d, image: Vec2d, scale: f64, pan: Vec2d) -> Vec2d {
    let overflow = image * scale - viewport;
    let limit = dvec2(overflow.x.max(0.0) * 0.5, overflow.y.max(0.0) * 0.5);
    dvec2(
        pan.x.clamp(-limit.x, limit.x),
        pan.y.clamp(-limit.y, limit.y),
    )
}

fn wheel_notch() -> f64 {
    if cfg!(target_os = "macos") {
        32.0
    } else if cfg!(target_os = "windows") {
        120.0
    } else if cfg!(target_os = "linux") {
        60.0
    } else {
        100.0
    }
}

fn scroll_zoom(zoom: f64, scroll_y: f64, notch: f64) -> f64 {
    (zoom * ZOOM_STEP.powf(-scroll_y / notch)).clamp(MIN_ZOOM, MAX_ZOOM)
}

/// Pan after changing zoom while keeping the image point below `anchor`
/// fixed. Coordinates are relative to the stage centre.
pub(crate) fn zoom_pan_at_cursor(
    old_pan: Vec2d,
    anchor: Vec2d,
    old_zoom: f64,
    new_zoom: f64,
) -> Vec2d {
    if old_zoom <= 0.0 || !old_zoom.is_finite() || !new_zoom.is_finite() {
        return old_pan;
    }
    anchor - (anchor - old_pan) * (new_zoom / old_zoom)
}

fn ease(current: f64, target: f64, dt: f64) -> f64 {
    let next = current + (target - current) * (1.0 - (-dt * 14.0).exp());
    if (next - target).abs() < 1e-3 { target } else { next }
}

impl ImageViewer {
    pub fn is_open(&self) -> bool {
        self.item.is_some()
    }

    #[cfg(test)]
    pub(crate) fn zoom(&self) -> f64 {
        self.target_zoom
    }

    pub fn show(&mut self, cx: &mut Cx, item: ImageViewerItem) -> Result<(), String> {
        self.clear_media(cx);
        let kind = media_kind(&item.bytes);
        self.media_kind = kind;
        let image = self.view.image(cx, ids!(image));
        match kind {
            MediaKind::Image => {
                image.load_image_from_data(cx, &item.bytes.bytes).map_err(|error| {
                    format!("could not decode {}: {error:?}", item.bytes.content_type)
                })?;
                self.image_size = image
                    .size_in_pixels(cx)
                    .map(|(w, h)| dvec2(w as f64, h as f64))
                    .unwrap_or(dvec2(1.0, 1.0));
                if let Some(mut image) = image.borrow_mut() {
                    image.draw_bg.image_dim_w = self.image_size.x as f32;
                    image.draw_bg.image_dim_h = self.image_size.y as f32;
                    image.draw_bg.rotation = 0.0;
                    image.draw_bg.sample_mode =
                        if self.pixelated { -1.0 } else { self.dpi_factor as f32 };
                }
            }
            MediaKind::Video => self
                .view
                .widget(cx, ids!(video))
                .borrow_mut::<VideoPlayer>()
                .ok_or_else(|| "video viewer is unavailable".to_string())?
                .load_bytes(cx, &item.bytes.bytes, &item.bytes.content_type)?,
            MediaKind::Audio => self
                .view
                .widget(cx, ids!(audio))
                .borrow_mut::<AudioPlayer>()
                .ok_or_else(|| "audio viewer is unavailable".to_string())?
                .load_bytes(cx, &item.bytes.bytes, &item.bytes.content_type)?,
            MediaKind::Mesh => {
                let target = self.view.widget(cx, ids!(mesh));
                let mut mesh = target.borrow_mut::<MeshView>()
                    .ok_or_else(|| "mesh viewer is unavailable".to_string())?;
                mesh.set_dark_enabled(cx, true);
                mesh.set_show_hud(cx, false);
                mesh.load_bytes(cx, &item.bytes.bytes, &item.bytes.content_type)?;
            },
            MediaKind::Splat => self
                .view
                .widget(cx, ids!(splat))
                .borrow_mut::<SplatView>()
                .ok_or_else(|| "splat viewer is unavailable".to_string())?
                .load_bytes(cx, &item.bytes.bytes, &item.bytes.content_type)?,
            MediaKind::Text | MediaKind::Unknown => {
                let text = if kind == MediaKind::Text {
                    let mut text: String = String::from_utf8_lossy(&item.bytes.bytes).chars().take(256 * 1024).collect();
                    if item.bytes.bytes.len() > text.len() {
                        text.push_str("\n\nPreview truncated. Save the file to read the complete content.");
                    }
                    text
                } else {
                    format!("{}\n{}\n\nSave this file to open it in a compatible application.", item.bytes.content_type, crate::faces::size_text(item.bytes.bytes.len()))
                };
                self.view.text_input(cx, ids!(document)).set_text(cx, &text);
            }
        }
        if kind != MediaKind::Image {
            self.image_size = dvec2(1.0, 1.0);
            image.set_texture(cx, None);
            self.view.widget(cx, ids!(media_layer)).set_visible(cx, true);
            let target = match kind {
                MediaKind::Video => ids!(video),
                MediaKind::Audio => ids!(audio),
                MediaKind::Mesh => ids!(mesh),
                MediaKind::Splat => ids!(splat),
                MediaKind::Text | MediaKind::Unknown => ids!(text_panel),
                MediaKind::Image => ids!(picture),
            };
            self.view.widget(cx, target).set_visible(cx, true);
        }
        self.view.label(cx, ids!(title)).set_text(
            cx,
            &if item.port.is_empty() { item.node.clone() } else { format!("{}.{}", item.node, item.port) },
        );
        self.view
            .label(cx, ids!(type_label))
            .set_text(cx, media_label(kind));
        self.view.label(cx, ids!(size)).set_text(
            cx,
            &if kind == MediaKind::Image {
                format!("{}×{}", self.image_size.x as usize, self.image_size.y as usize)
            } else {
                crate::faces::size_text(item.bytes.bytes.len())
            },
        );
        self.view.label(cx, ids!(zoom)).set_text(
            cx,
            match kind {
                MediaKind::Image => "100 %",
                MediaKind::Video => "native transport · drag timeline",
                MediaKind::Audio => "waveform transport · drag to scrub",
                MediaKind::Mesh | MediaKind::Splat => "drag to orbit · scroll to zoom",
                MediaKind::Text | MediaKind::Unknown => "",
            },
        );
        self.view.widget(cx, ids!(save)).set_visible(cx, true);
        self.view.widget(cx, ids!(copy)).set_visible(cx, true);
        self.item = Some(item);
        self.zoom = 1.0;
        self.target_zoom = 1.0;
        self.dpi_factor = 1.0;
        self.pan = Vec2d::default();
        self.target_pan = Vec2d::default();
        self.opacity = 0.18;
        self.picture_rect = None;
        self.close_button_rect = None;
        self.gallery_stage_rect = None;
        self.media_rect = None;
        self.shown_percent = None;
        self.fit_mode = true;
        self.snap_to_fit = true;
        self.view.set_visible(cx, true);
        self.view
            .view(cx, ids!(picture))
            .set_visible(cx, false);
        for id in [ids!(fit), ids!(fit_width), ids!(actual), ids!(double), ids!(pixels)] {
            self.view.widget(cx, id).set_visible(cx, kind == MediaKind::Image);
        }
        self.sync_control_opacity(cx);
        cx.set_key_focus(self.view.area());
        self.next_frame = cx.new_next_frame();
        self.redraw(cx);
        Ok(())
    }

    pub fn show_status(&mut self, cx: &mut Cx, title: &str, message: &str) {
        let _ = self.show(cx, ImageViewerItem {
            node: title.to_string(),
            port: String::new(),
            bytes: ValueBytes {
                digest: String::new(),
                content_type: "text/plain".to_string(),
                bytes: message.as_bytes().to_vec().into(),
            },
        });
        self.view.label(cx, ids!(type_label)).set_text(cx, "");
        self.view.label(cx, ids!(size)).set_text(cx, "");
        self.view.widget(cx, ids!(save)).set_visible(cx, false);
        self.view.widget(cx, ids!(copy)).set_visible(cx, false);
    }

    pub fn close(&mut self, cx: &mut Cx) {
        if self.item.is_none() {
            return;
        }
        // Dismiss the whole overlay in this event. Fading only the image
        // exposes its checkerboard while the modal is still on screen.
        self.view.set_visible(cx, false);
        self.item = None;
        self.drag = None;
        self.pinch = None;
        self.next_frame = NextFrame::default();
        self.clear_media(cx);
        self.redraw(cx);
    }

    fn stage_rect(&self, cx: &Cx) -> Rect {
        self.view.view(cx, ids!(stage)).area().rect(cx)
    }

    fn refresh_dpi_factor(&mut self, cx: &mut Cx) {
        let area = self.view.area();
        let dpi_factor = cx.get_dpi_factor_of(&area);
        if dpi_factor.is_finite() && dpi_factor > 0.0 {
            self.dpi_factor = dpi_factor;
        }
    }

    fn fit_zoom(&self, stage: Rect) -> f64 {
        fit_scale(stage.size, self.image_size, FIT_MARGIN) * self.dpi_factor
    }

    fn target_layout_scale(&self) -> f64 {
        self.target_zoom / self.dpi_factor.max(1e-6)
    }

    fn sync_control_opacity(&self, cx: &mut Cx) {
        let opacity = [self.opacity as f32];
        self.view
            .view(cx, ids!(bar))
            .set_uniform(cx, id!(viewer_opacity), &opacity);
        self.view
            .view(cx, ids!(top_close))
            .set_uniform(cx, id!(viewer_opacity), &opacity);
    }

    fn sync_picture(&mut self, cx: &mut Cx) {
        if self.item.is_none() {
            return;
        }
        self.refresh_dpi_factor(cx);
        let stage = self.stage_rect(cx);
        if stage.size.x > 0.0
            && stage.size.y > 0.0
            && self.gallery_stage_rect != Some(stage)
        {
            let y = stage.pos.y + (stage.size.y - GALLERY_BUTTON_HEIGHT) * 0.5;
            for (id, x) in [
                (ids!(previous_wrap), stage.pos.x + CLOSE_BUTTON_MARGIN),
                (ids!(next_wrap), stage.pos.x + stage.size.x - CLOSE_BUTTON_MARGIN - GALLERY_BUTTON_WIDTH),
            ] {
                self.view.view(cx, id).set_walk(cx, Walk {
                    abs_pos: Some(dvec2(x, y)),
                    width: Size::Fixed(GALLERY_BUTTON_WIDTH),
                    height: Size::Fixed(GALLERY_BUTTON_HEIGHT),
                    ..Walk::default()
                });
            }
            self.gallery_stage_rect = Some(stage);
        }
        let close_rect = close_button_rect(stage);
        if stage.size.x > 0.0
            && stage.size.y > 0.0
            && self.close_button_rect != Some(close_rect)
        {
            self.view.view(cx, ids!(top_close)).set_walk(
                cx,
                Walk {
                    abs_pos: Some(close_rect.pos),
                    width: Size::Fixed(close_rect.size.x),
                    height: Size::Fixed(close_rect.size.y),
                    ..Walk::default()
                },
            );
            self.close_button_rect = Some(close_rect);
        }
        self.sync_control_opacity(cx);
        let media_safe_bottom = match self.media_kind {
            MediaKind::Video | MediaKind::Audio | MediaKind::Text | MediaKind::Unknown => 76.0,
            _ => 0.0,
        };
        let media_rect = Rect {
            pos: stage.pos,
            size: dvec2(stage.size.x, (stage.size.y - media_safe_bottom).max(1.0)),
        };
        if self.media_rect != Some(media_rect) {
            self.view.view(cx, ids!(media_layer)).set_walk(
                cx,
                Walk {
                    abs_pos: Some(media_rect.pos),
                    width: Size::Fixed(media_rect.size.x.max(1.0)),
                    height: Size::Fixed(media_rect.size.y),
                    ..Walk::default()
                },
            );
            self.media_rect = Some(media_rect);
        }
        if self.media_kind != MediaKind::Image {
            self.view.view(cx, ids!(picture)).set_visible(cx, false);
            return;
        }
        if stage.size.x <= 0.0 || stage.size.y <= 0.0 {
            self.view
                .view(cx, ids!(picture))
                .set_visible(cx, false);
            if let Some(mut image) = self.view.image(cx, ids!(image)).borrow_mut() {
                image.draw_bg.opacity = 0.0;
            }
            self.next_frame = cx.new_next_frame();
            return;
        }
        self.view
            .view(cx, ids!(picture))
            .set_visible(cx, true);
        if self.fit_mode {
            let fit_zoom = self.fit_zoom(stage);
            if (self.target_zoom - fit_zoom).abs() > 1e-9 {
                self.target_zoom = fit_zoom;
                self.next_frame = cx.new_next_frame();
            }
            self.target_pan = Vec2d::default();
            if self.snap_to_fit {
                self.zoom = fit_zoom;
                self.pan = Vec2d::default();
                self.snap_to_fit = false;
            }
        }
        self.target_pan = clamp_pan(
            stage.size,
            self.image_size,
            self.target_layout_scale(),
            self.target_pan,
        );
        let scale = self.zoom / self.dpi_factor;
        self.pan = clamp_pan(stage.size, self.image_size, scale, self.pan);
        let size = self.image_size * scale;
        let pos = stage.pos + center_offset(stage.size, self.image_size, scale) + self.pan;
        let picture_rect = Rect { pos, size };
        let rect_changed = match self.picture_rect.as_ref() {
            Some(previous) => {
                (previous.pos - picture_rect.pos).length() > 1e-4
                    || (previous.size - picture_rect.size).length() > 1e-4
            }
            None => true,
        };
        if rect_changed {
            self.view.view(cx, ids!(picture)).set_walk(
                cx,
                Walk {
                    abs_pos: Some(pos),
                    width: Size::Fixed(size.x.max(1.0)),
                    height: Size::Fixed(size.y.max(1.0)),
                    ..Walk::default()
                },
            );
            self.picture_rect = Some(picture_rect);
        }
        if let Some(mut image) = self.view.image(cx, ids!(image)).borrow_mut() {
            image.draw_bg.opacity = self.opacity as f32;
            image.draw_bg.image_dim_w = self.image_size.x as f32;
            image.draw_bg.image_dim_h = self.image_size.y as f32;
            image.draw_bg.rotation = 0.0;
            image.draw_bg.sample_mode =
                if self.pixelated { -1.0 } else { self.dpi_factor as f32 };
        }
        self.update_zoom_label(cx);
    }

    fn update_zoom_label(&mut self, cx: &mut Cx) {
        let percent = (self.target_zoom * 100.0).round() as i64;
        if self.shown_percent != Some(percent) {
            self.shown_percent = Some(percent);
            self.view
                .label(cx, ids!(zoom))
                .set_text(cx, &format!("{percent} %"));
        }
    }

    fn fit(&mut self, cx: &mut Cx) {
        self.fit_mode = true;
        self.snap_to_fit = false;
        self.target_pan = Vec2d::default();
        self.sync_picture(cx);
        self.next_frame = cx.new_next_frame();
    }

    fn actual(&mut self, cx: &mut Cx) {
        self.fit_mode = false;
        self.target_zoom = 1.0;
        self.target_pan = Vec2d::default();
        self.update_zoom_label(cx);
        self.next_frame = cx.new_next_frame();
    }

    fn double(&mut self, cx: &mut Cx) {
        self.fit_mode = false;
        self.target_zoom = 2.0;
        self.target_pan = Vec2d::default();
        self.update_zoom_label(cx);
        self.next_frame = cx.new_next_frame();
    }

    fn fit_width(&mut self, cx: &mut Cx) {
        self.refresh_dpi_factor(cx);
        let stage = self.stage_rect(cx);
        self.fit_mode = false;
        self.target_zoom =
            fit_width_scale(stage.size, self.image_size, FIT_MARGIN) * self.dpi_factor;
        self.target_pan = Vec2d::default();
        self.update_zoom_label(cx);
        self.next_frame = cx.new_next_frame();
    }

    fn zoom_at(&mut self, cx: &mut Cx, abs: Vec2d, new_zoom: f64) {
        let stage = self.stage_rect(cx);
        let anchor = abs - (stage.pos + stage.size * 0.5);
        let new_zoom = new_zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self.fit_mode = false;
        self.target_pan = zoom_pan_at_cursor(self.pan, anchor, self.zoom, new_zoom);
        self.target_zoom = new_zoom;
        self.target_pan = clamp_pan(
            stage.size,
            self.image_size,
            self.target_layout_scale(),
            self.target_pan,
        );
        self.update_zoom_label(cx);
        self.next_frame = cx.new_next_frame();
    }

    fn toggle_fit_actual(&mut self, cx: &mut Cx) {
        if self.fit_mode {
            self.actual(cx);
        } else {
            self.fit(cx);
        }
    }

    #[cfg(test)]
    pub(crate) fn media_kind(&self) -> MediaKind {
        self.media_kind
    }

    fn clear_media(&mut self, cx: &mut Cx) {
        self.view.widget(cx, ids!(media_layer)).set_visible(cx, false);
        self.view.widget(cx, ids!(text_panel)).set_visible(cx, false);
        let video = self.view.widget(cx, ids!(video));
        video.set_visible(cx, false);
        if let Some(mut video) = video.borrow_mut::<VideoPlayer>() {
            video.clear(cx);
        }
        let audio = self.view.widget(cx, ids!(audio));
        audio.set_visible(cx, false);
        if let Some(mut audio) = audio.borrow_mut::<AudioPlayer>() {
            audio.clear(cx);
        }
        let mesh = self.view.widget(cx, ids!(mesh));
        mesh.set_visible(cx, false);
        if let Some(mut mesh) = mesh.borrow_mut::<MeshView>() {
            mesh.clear(cx);
        }
        let splat = self.view.widget(cx, ids!(splat));
        splat.set_visible(cx, false);
        if let Some(mut splat) = splat.borrow_mut::<SplatView>() {
            splat.clear(cx);
        };
    }

    fn handle_touch(&mut self, cx: &mut Cx, event: &TouchUpdateEvent) {
        let mut points = event
            .touches
            .iter()
            .filter(|touch| touch.state != TouchState::Stop)
            .map(|touch| touch.abs);
        let (Some(a), Some(b)) = (points.next(), points.next()) else {
            self.pinch = None;
            return;
        };
        let midpoint = (a + b) * 0.5;
        let distance = (b - a).length().max(1.0);
        let pinch = *self.pinch.get_or_insert(Pinch {
            distance,
            zoom: self.zoom,
            pan: self.pan,
            midpoint,
        });
        let zoom = (pinch.zoom * distance / pinch.distance).clamp(MIN_ZOOM, MAX_ZOOM);
        let stage = self.stage_rect(cx);
        let centre = stage.pos + stage.size * 0.5;
        let start_anchor = pinch.midpoint - centre;
        let current_anchor = midpoint - centre;
        self.target_pan = current_anchor
            - (start_anchor - pinch.pan) * (zoom / pinch.zoom.max(1e-6));
        self.fit_mode = false;
        self.target_zoom = zoom;
        self.target_pan = clamp_pan(
            stage.size,
            self.image_size,
            self.target_layout_scale(),
            self.target_pan,
        );
        self.update_zoom_label(cx);
        self.next_frame = cx.new_next_frame();
    }

    pub fn actions(&mut self, cx: &mut Cx, actions: &Actions) -> Vec<ImageViewerAction> {
        let mut out = Vec::new();
        out.extend(
            actions
                .iter()
                .filter_map(|action| action.as_widget_action())
                .filter(|action| action.widget_uid == self.view.widget_uid())
                .map(|action| action.cast::<ImageViewerAction>()),
        );
        if self.view.button(cx, ids!(close)).clicked(actions) {
            out.push(ImageViewerAction::Close);
        }
        if self
            .view
            .button(cx, ids!(top_close_button))
            .clicked(actions)
        {
            out.push(ImageViewerAction::Close);
        }
        if self.view.button(cx, ids!(save)).clicked(actions) {
            out.push(ImageViewerAction::Save);
        }
        if self.view.button(cx, ids!(copy)).clicked(actions) {
            if let Some(item) = self.item.as_ref() {
                out.push(ImageViewerAction::CopyDigest(item.bytes.digest.clone()));
            }
        }
        if self.view.button(cx, ids!(previous)).clicked(actions)
            || self.view.button(cx, ids!(previous_asset)).clicked(actions)
        {
            out.push(ImageViewerAction::Step(-1));
        }
        if self.view.button(cx, ids!(next)).clicked(actions)
            || self.view.button(cx, ids!(next_asset)).clicked(actions)
        {
            out.push(ImageViewerAction::Step(1));
        }
        if self.view.button(cx, ids!(fit)).clicked(actions) {
            self.fit(cx);
        }
        if self.view.button(cx, ids!(fit_width)).clicked(actions) {
            self.fit_width(cx);
        }
        if self.view.button(cx, ids!(actual)).clicked(actions) {
            self.actual(cx);
        }
        if self.view.button(cx, ids!(double)).clicked(actions) {
            self.double(cx);
        }
        if self.view.button(cx, ids!(pixels)).clicked(actions) {
            self.pixelated = !self.pixelated;
            self.view.button(cx, ids!(pixels)).set_text(
                cx,
                if self.pixelated { "Pixels: on" } else { "Pixels: auto" },
            );
            self.sync_picture(cx);
        }
        out
    }
}

impl Widget for ImageViewer {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let step = self.view.draw_walk(cx, scope, walk);
        if step.is_done() {
            // The parent draw list is cleared before child drawing starts, so
            // stage.area() is only current after this view completes its draw.
            self.sync_picture(cx);
        }
        step
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.item.is_none() {
            return;
        }
        self.view.handle_event(cx, event, scope);
        if let Some(nf) = self.next_frame.is_event(event) {
            let dt = (nf.time - self.last_time).clamp(0.0, 0.1);
            self.last_time = nf.time;
            self.zoom = ease(self.zoom, self.target_zoom, dt);
            self.pan.x = ease(self.pan.x, self.target_pan.x, dt);
            self.pan.y = ease(self.pan.y, self.target_pan.y, dt);
            self.opacity = ease(self.opacity, 1.0, dt);
            self.sync_picture(cx);
            if (self.zoom - self.target_zoom).abs() > 1e-4
                || (self.pan - self.target_pan).length() > 0.05
                || self.opacity < 0.999
            {
                self.next_frame = cx.new_next_frame();
            }
            self.redraw(cx);
        }
        if let Event::TouchUpdate(event) = event {
            self.handle_touch(cx, event);
        }
        if let Event::KeyDown(event) = event {
            match event.key_code {
                KeyCode::Escape => {
                    cx.widget_action(self.view.widget_uid(), ImageViewerAction::Close)
                }
                KeyCode::Key0 if self.media_kind == MediaKind::Image => self.fit(cx),
                KeyCode::Key1 if self.media_kind == MediaKind::Image => self.actual(cx),
                KeyCode::Key2 if self.media_kind == MediaKind::Image => self.double(cx),
                KeyCode::ArrowLeft if cx.has_key_focus(self.view.area()) => {
                    cx.widget_action(self.view.widget_uid(), ImageViewerAction::Step(-1))
                }
                KeyCode::ArrowRight if cx.has_key_focus(self.view.area()) => {
                    cx.widget_action(self.view.widget_uid(), ImageViewerAction::Step(1))
                }
                _ => {}
            }
        }
        match event.hits(cx, self.view.area()) {
            Hit::FingerScroll(event) => {
                if self.media_kind == MediaKind::Image {
                    let zoom = scroll_zoom(self.target_zoom, event.scroll.y, wheel_notch());
                    self.zoom_at(cx, event.abs, zoom);
                }
            }
            Hit::FingerDown(event) => {
                cx.set_key_focus(self.view.area());
                if self.media_kind != MediaKind::Image {
                    return;
                }
                let picture = self.view.view(cx, ids!(picture)).area().rect(cx);
                let bar = self.view.view(cx, ids!(bar)).area().rect(cx);
                let top_close = self.view.view(cx, ids!(top_close)).area().rect(cx);
                let previous = self.view.view(cx, ids!(previous_wrap)).area().rect(cx);
                let next = self.view.view(cx, ids!(next_wrap)).area().rect(cx);
                if bar.contains(event.abs) || top_close.contains(event.abs)
                    || previous.contains(event.abs) || next.contains(event.abs)
                {
                    return;
                }
                if picture.contains(event.abs) && event.tap_count >= 2 {
                    self.toggle_fit_actual(cx);
                    return;
                }
                self.drag = Some(Drag {
                    start: event.abs,
                    pan: self.target_pan,
                    outside: !picture.contains(event.abs),
                });
                if picture.contains(event.abs) {
                    cx.set_cursor(MouseCursor::Grabbing);
                }
            }
            Hit::FingerMove(event) => {
                if let Some(drag) = self.drag {
                    if !drag.outside {
                        self.target_pan = drag.pan + (event.abs - drag.start);
                        let stage = self.stage_rect(cx);
                        self.target_pan = clamp_pan(
                            stage.size,
                            self.image_size,
                            self.target_layout_scale(),
                            self.target_pan,
                        );
                        self.pan = self.target_pan;
                        self.sync_picture(cx);
                        self.redraw(cx);
                    }
                }
            }
            Hit::FingerUp(event) => {
                cx.set_cursor(MouseCursor::Default);
                if let Some(drag) = self.drag.take() {
                    if drag.outside && (event.abs - drag.start).length() <= DRAG_THRESHOLD {
                        cx.widget_action(self.view.widget_uid(), ImageViewerAction::Close);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw_frame(
        cx: &mut Cx,
        viewer: &mut ImageViewer,
        pass: &DrawPass,
        draw_list: &mut DrawList2d,
        size: Vec2d,
    ) {
        let event = DrawEvent {
            redraw_all: true,
            ..Default::default()
        };
        let mut cx_draw = CxDraw::new(cx, &event);
        let cx = &mut Cx2d::new(&mut cx_draw);
        cx.begin_pass(pass, Some(1.0));
        draw_list.begin_always(cx);
        cx.begin_root_turtle(size, Layout::flow_overlay());
        viewer.draw_walk_all(cx, &mut Scope::empty(), Walk::fill());
        cx.end_turtle();
        draw_list.end(cx);
        cx.end_pass(pass);
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn fit_scale_uses_the_limiting_landscape_axis() {
        let scale = fit_scale(dvec2(1000.0, 800.0), dvec2(1600.0, 900.0), 24.0);
        assert_close(scale, 952.0 / 1600.0);
    }

    #[test]
    fn fit_scale_uses_the_limiting_portrait_axis() {
        let scale = fit_scale(dvec2(1000.0, 800.0), dvec2(600.0, 1200.0), 24.0);
        assert_close(scale, 752.0 / 1200.0);
    }

    #[test]
    fn fit_scale_enlarges_a_small_picture_to_the_margin() {
        let scale = fit_scale(dvec2(1000.0, 800.0), dvec2(100.0, 80.0), 24.0);
        assert_close(scale, 752.0 / 80.0);
    }

    #[test]
    fn centre_offset_centres_both_smaller_and_larger_axes() {
        let offset = center_offset(dvec2(1000.0, 800.0), dvec2(1200.0, 200.0), 1.0);
        assert!((offset - dvec2(-100.0, 300.0)).length() < 1e-9);
    }

    #[test]
    fn close_button_stays_at_stage_top_left_across_picture_transforms() {
        let stage = Rect {
            pos: dvec2(37.0, 59.0),
            size: dvec2(1000.0, 800.0),
        };
        let expected = Rect {
            pos: dvec2(53.0, 75.0),
            size: dvec2(28.0, 28.0),
        };

        for (zoom, pan) in [
            (0.25, dvec2(0.0, 0.0)),
            (1.0, dvec2(120.0, -80.0)),
            (8.0, dvec2(-900.0, 650.0)),
        ] {
            let image = dvec2(1600.0, 1200.0);
            let _picture = Rect {
                pos: stage.pos + center_offset(stage.size, image, zoom) + pan,
                size: image * zoom,
            };
            assert_eq!(close_button_rect(stage), expected);
        }
    }

    #[test]
    fn pan_is_zero_on_a_small_axis_and_clamped_on_an_overflowing_axis() {
        let viewport = dvec2(1000.0, 800.0);
        let image = dvec2(1200.0, 400.0);
        assert_eq!(
            clamp_pan(viewport, image, 1.0, dvec2(350.0, -250.0)),
            dvec2(100.0, 0.0)
        );
        assert_eq!(
            clamp_pan(viewport, image, 1.0, dvec2(-350.0, 250.0)),
            dvec2(-100.0, 0.0)
        );
    }

    #[test]
    fn scroll_zoom_is_geometric_per_notch_and_bounded() {
        assert_close(scroll_zoom(1.0, -60.0, 60.0), 1.1);
        assert_close(scroll_zoom(1.0, 60.0, 60.0), 1.0 / 1.1);
        assert_close(scroll_zoom(MAX_ZOOM, -60.0, 60.0), MAX_ZOOM);
        assert_close(scroll_zoom(MIN_ZOOM, 60.0, 60.0), MIN_ZOOM);
    }

    #[test]
    fn opening_lays_out_a_visible_fitted_centred_picture_after_two_frames() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let mut viewer = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::theme::script_mod(vm);
            makepad_media_view::script_mod(vm);
            super::script_mod(vm);
            ImageViewer::script_new_with_default(vm)
        });
        viewer
            .show(
                &mut cx,
                ImageViewerItem {
                    node: "image".into(),
                    port: "value".into(),
                    bytes: ValueBytes {
                        digest: "test".into(),
                        content_type: "image/svg+xml".into(),
                        bytes: br#"<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024"/>"#
                            .as_slice()
                            .into(),
                    },
                },
            )
            .unwrap();

        let size = dvec2(1280.0, 720.0);
        let pass = DrawPass::new(&mut cx);
        pass.set_size(&mut cx, size);
        let mut draw_list = DrawList2d::new(&mut cx);
        draw_frame(&mut cx, &mut viewer, &pass, &mut draw_list, size);
        draw_frame(&mut cx, &mut viewer, &pass, &mut draw_list, size);

        let stage = viewer.stage_rect(&cx);
        let picture = viewer.view.view(&cx, ids!(picture));
        let picture_walk = picture.walk(&mut cx);
        let picture_rect = picture.area().rect(&cx);
        assert!(picture.visible());
        assert!(matches!(picture_walk.width, Size::Fixed(width) if width > 0.0));
        assert!(matches!(picture_walk.height, Size::Fixed(height) if height > 0.0));
        assert!(picture_rect.size.x > 0.0 && picture_rect.size.y > 0.0);
        assert!(stage.contains(picture_rect.pos));
        assert!(stage.contains(picture_rect.pos + picture_rect.size));
        assert!(
            (picture_rect.pos + picture_rect.size * 0.5 - (stage.pos + stage.size * 0.5))
                .length()
                < 1e-9
        );
        let image = viewer.view.image(&cx, ids!(image));
        let image = image.borrow().unwrap();
        assert_eq!(image.draw_bg.rotation, 0.0);
        assert_eq!(image.draw_bg.sample_mode, 1.0);
    }

    #[test]
    fn zoom_at_cursor_keeps_the_same_image_point_under_the_cursor() {
        let pan = dvec2(17.0, -9.0);
        let anchor = dvec2(120.0, 45.0);
        let old_zoom = 0.75;
        let new_zoom = 3.2;
        let point = (anchor - pan) / old_zoom;
        let next_pan = zoom_pan_at_cursor(pan, anchor, old_zoom, new_zoom);
        let landed = point * new_zoom + next_pan;
        assert!((landed - anchor).length() < 1e-9);
    }

    #[test]
    fn zoom_at_cursor_closes_over_repeated_steps() {
        let anchor = dvec2(-73.0, 61.0);
        let mut pan = Vec2d::default();
        let mut zoom = 1.0;
        for next in [1.4, 2.8, 0.35, 7.0, 1.0] {
            let point = (anchor - pan) / zoom;
            pan = zoom_pan_at_cursor(pan, anchor, zoom, next);
            zoom = next;
            assert!((point * zoom + pan - anchor).length() < 1e-9);
        }
    }

    fn tiny_wav() -> Vec<u8> {
        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&38u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8_000u32.to_le_bytes());
        wav.extend_from_slice(&16_000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&2u32.to_le_bytes());
        wav.extend_from_slice(&0i16.to_le_bytes());
        wav
    }

    #[test]
    fn modal_routes_each_media_kind_to_its_widget() {
        let cases = [
            ("video/mp4", b"video".to_vec(), MediaKind::Video, live_id!(video)),
            ("audio/wav", tiny_wav(), MediaKind::Audio, live_id!(audio)),
            (
                "model/gltf-binary",
                b"glTF\x02\0\0\0\x10\0\0\0".to_vec(),
                MediaKind::Mesh,
                live_id!(mesh),
            ),
            (
                "application/x-ply",
                b"ply\nformat ascii 1.0\nend_header\n".to_vec(),
                MediaKind::Splat,
                live_id!(splat),
            ),
        ];
        for (content_type, bytes, expected, child) in cases {
            let mut cx = Cx::new(Box::new(|_, _| {}));
            let mut viewer = cx.with_vm(|vm| {
                makepad_widgets::script_mod(vm);
                crate::theme::script_mod(vm);
                makepad_media_view::script_mod(vm);
                super::script_mod(vm);
                ImageViewer::script_new_with_default(vm)
            });
            viewer
                .show(
                    &mut cx,
                    ImageViewerItem {
                        node: "output".into(),
                        port: "value".into(),
                        bytes: ValueBytes {
                            digest: content_type.into(),
                            content_type: content_type.into(),
                            bytes: bytes.into(),
                        },
                    },
                )
                .unwrap();
            assert_eq!(viewer.media_kind(), expected);
            assert!(viewer.view.widget(&mut cx, &[child]).visible());
        }
    }

    #[test]
    fn json_asset_opens_in_the_read_only_document_surface() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let mut viewer = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::theme::script_mod(vm);
            makepad_media_view::script_mod(vm);
            super::script_mod(vm);
            ImageViewer::script_new_with_default(vm)
        });
        let json = "{\"prompt\":\"a brass fox\",\"steps\":8}";
        viewer
            .show(
                &mut cx,
                ImageViewerItem {
                    node: "asset_42".into(),
                    port: "manifest".into(),
                    bytes: ValueBytes {
                        digest: "ast_json".into(),
                        content_type: "application/json; charset=utf-8".into(),
                        bytes: json.as_bytes().to_vec().into(),
                    },
                },
            )
            .unwrap();

        assert_eq!(viewer.media_kind(), MediaKind::Text);
        assert!(viewer.is_open());
        assert!(viewer.view.widget(&mut cx, ids!(text_panel)).visible());
        assert_eq!(viewer.view.text_input(&mut cx, ids!(document)).text(), json);
        assert!(!viewer.view.widget(&mut cx, ids!(video)).visible());
        assert!(!viewer.view.widget(&mut cx, ids!(audio)).visible());
    }

    #[test]
    fn loading_status_uses_the_text_surface_and_hides_file_actions() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let mut viewer = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::theme::script_mod(vm);
            makepad_media_view::script_mod(vm);
            super::script_mod(vm);
            ImageViewer::script_new_with_default(vm)
        });
        viewer.show_status(&mut cx, "Asset", "Loading original content…");

        assert!(viewer.is_open());
        assert!(viewer.view.widget(&mut cx, ids!(text_panel)).visible());
        assert_eq!(
            viewer.view.text_input(&mut cx, ids!(document)).text(),
            "Loading original content…"
        );
        assert!(!viewer.view.widget(&mut cx, ids!(save)).visible());
        assert!(!viewer.view.widget(&mut cx, ids!(copy)).visible());
    }

    #[test]
    fn switching_from_audio_clears_the_previous_player() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let mut viewer = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::theme::script_mod(vm);
            makepad_media_view::script_mod(vm);
            super::script_mod(vm);
            ImageViewer::script_new_with_default(vm)
        });
        viewer
            .show(
                &mut cx,
                ImageViewerItem {
                    node: "audio".into(),
                    port: "value".into(),
                    bytes: ValueBytes {
                        digest: "audio".into(),
                        content_type: "audio/wav".into(),
                        bytes: tiny_wav().into(),
                    },
                },
            )
            .unwrap();
        assert!(viewer.view.widget(&mut cx, ids!(audio)).visible());
        assert!(viewer
            .view
            .widget(&mut cx, ids!(audio))
            .borrow::<AudioPlayer>()
            .is_some_and(|audio| audio.is_loaded()));

        viewer
            .show(
                &mut cx,
                ImageViewerItem {
                    node: "manifest".into(),
                    port: "value".into(),
                    bytes: ValueBytes {
                        digest: "text".into(),
                        content_type: "text/plain".into(),
                        bytes: b"ready".to_vec().into(),
                    },
                },
            )
            .unwrap();
        assert_eq!(viewer.media_kind(), MediaKind::Text);
        assert!(viewer.view.widget(&mut cx, ids!(text_panel)).visible());
        assert!(!viewer.view.widget(&mut cx, ids!(audio)).visible());
        assert!(viewer
            .view
            .widget(&mut cx, ids!(audio))
            .borrow::<AudioPlayer>()
            .is_some_and(|audio| !audio.is_loaded()));
    }
}
