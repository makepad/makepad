//! Full-window image inspection: one textured quad over a quiet checker,
//! with an eased fit/1:1 camera. Bytes are supplied by the app's value cache.

use makepad_flow::ValueBytes;
use makepad_widgets::makepad_draw::event::{TouchState, TouchUpdateEvent};
use makepad_widgets::*;

const MIN_ZOOM: f64 = 0.1;
const MAX_ZOOM: f64 = 16.0;
const DRAG_THRESHOLD: f64 = 4.0;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ImageViewerBase = #(ImageViewer::register_widget(vm))
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
            picture := RoundedView{
                width: 1
                height: 1
                flow: Overlay
                show_bg: true
                draw_bg +: {
                    pixel: fn() {
                        let cell = floor((self.pos.x * self.rect_size.x) / 10.0)
                            + floor((self.pos.y * self.rect_size.y) / 10.0)
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
                        // `rotation` is an otherwise-unused flag for forcing
                        // point sampling. Auto mode switches above 4x.
                        get_color_scale_pan: fn(scale: vec2, pan: vec2) {
                            let uv = self.pos * scale + pan
                            if self.rotation > 0.5
                                || (self.image_dim_w > 0.0
                                    && self.rect_size.x / self.image_dim_w > 4.0) {
                                return self.image_texture.sample_nearest(uv)
                            }
                            return self.image_texture.sample_as_bgra(uv)
                        }
                    }
                }
            }
        }
        bottom_wrap := View{
            width: Fill
            height: Fill
            flow: Down
            align: Align{y: 1.0}
            padding: Inset{left: 18 right: 18 bottom: 16}
            bar := RoundedShadowView{
                width: Fill
                height: 42
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
                title := Label{
                    width: Fit
                    height: Fit
                    draw_text +: {
                        color: theme.flow_text
                        text_style: theme.font_bold{font_size: 9.5}
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
                previous := ButtonFlatter{text: "←"}
                next := ButtonFlatter{text: "→"}
                fit := ButtonFlat{text: "Fit"}
                actual := ButtonFlat{text: "1:1"}
                pixels := ButtonFlat{text: "Pixels: auto"}
                save := Button{text: "Save…"}
                copy := ButtonFlat{text: "Copy digest"}
                close := ButtonFlatter{text: "×"}
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
    shown_percent: Option<i64>,
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
    pub fn show(&mut self, cx: &mut Cx, item: ImageViewerItem) -> Result<(), String> {
        let image = self.view.image(cx, ids!(image));
        image
            .load_image_from_data(cx, &item.bytes.bytes)
            .map_err(|error| format!("could not decode {}: {error:?}", item.bytes.content_type))?;
        self.image_size = image
            .size_in_pixels(cx)
            .map(|(w, h)| dvec2(w as f64, h as f64))
            .unwrap_or(dvec2(1.0, 1.0));
        if let Some(mut image) = image.borrow_mut() {
            image.draw_bg.image_dim_w = self.image_size.x as f32;
            image.draw_bg.image_dim_h = self.image_size.y as f32;
            image.draw_bg.rotation = if self.pixelated { 1.0 } else { 0.0 };
        }
        self.view.label(cx, ids!(title)).set_text(
            cx,
            &format!("{}.{}", item.node, item.port),
        );
        self.view.label(cx, ids!(size)).set_text(
            cx,
            &format!("{}×{}", self.image_size.x as usize, self.image_size.y as usize),
        );
        self.item = Some(item);
        self.zoom = 1.0;
        self.target_zoom = 1.0;
        self.pan = Vec2d::default();
        self.target_pan = Vec2d::default();
        self.opacity = 0.18;
        self.picture_rect = None;
        self.shown_percent = None;
        self.view.set_visible(cx, true);
        cx.set_key_focus(self.view.area());
        self.next_frame = cx.new_next_frame();
        self.sync_picture(cx);
        self.redraw(cx);
        Ok(())
    }

    pub fn close(&mut self, cx: &mut Cx) {
        self.item = None;
        self.drag = None;
        self.pinch = None;
        self.view.set_visible(cx, false);
        self.redraw(cx);
    }

    fn stage_rect(&self, cx: &Cx) -> Rect {
        self.view.view(cx, ids!(stage)).area().rect(cx)
    }

    fn fit_scale(&self, cx: &Cx) -> f64 {
        let stage = self.stage_rect(cx);
        if stage.size.x <= 0.0 || stage.size.y <= 0.0 || self.image_size.x <= 0.0 || self.image_size.y <= 0.0 {
            return 1.0;
        }
        (stage.size.x / self.image_size.x)
            .min(stage.size.y / self.image_size.y)
            .max(1e-6)
    }

    fn actual_scale(&self, cx: &Cx) -> f64 {
        self.fit_scale(cx) * self.zoom
    }

    fn sync_picture(&mut self, cx: &mut Cx) {
        if self.item.is_none() {
            return;
        }
        let stage = self.stage_rect(cx);
        let scale = self.fit_scale(cx) * self.zoom;
        let size = self.image_size * scale;
        let pos = stage.pos + (stage.size - size) * 0.5 + self.pan;
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
            image.draw_bg.rotation = if self.pixelated { 1.0 } else { 0.0 };
        }
        if self.shown_percent.is_none() {
            self.update_zoom_label(cx);
        }
    }

    fn update_zoom_label(&mut self, cx: &mut Cx) {
        let percent = (self.fit_scale(cx) * self.target_zoom * 100.0).round() as i64;
        if self.shown_percent != Some(percent) {
            self.shown_percent = Some(percent);
            self.view
                .label(cx, ids!(zoom))
                .set_text(cx, &format!("{percent}%"));
        }
    }

    fn fit(&mut self, cx: &mut Cx) {
        self.target_zoom = 1.0;
        self.target_pan = Vec2d::default();
        self.update_zoom_label(cx);
        self.next_frame = cx.new_next_frame();
    }

    fn actual(&mut self, cx: &mut Cx) {
        self.target_zoom = (1.0 / self.fit_scale(cx)).clamp(MIN_ZOOM, MAX_ZOOM);
        self.target_pan = Vec2d::default();
        self.update_zoom_label(cx);
        self.next_frame = cx.new_next_frame();
    }

    fn zoom_at(&mut self, cx: &mut Cx, abs: Vec2d, new_zoom: f64) {
        let stage = self.stage_rect(cx);
        let anchor = abs - (stage.pos + stage.size * 0.5);
        let new_zoom = new_zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self.target_pan = zoom_pan_at_cursor(
            self.target_pan,
            anchor,
            self.target_zoom,
            new_zoom,
        );
        self.target_zoom = new_zoom;
        self.update_zoom_label(cx);
        self.next_frame = cx.new_next_frame();
    }

    fn toggle_fit_actual(&mut self, cx: &mut Cx) {
        if (self.actual_scale(cx) - 1.0).abs() < 0.02 {
            self.fit(cx);
        } else {
            self.actual(cx);
        }
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
            zoom: self.target_zoom,
            pan: self.target_pan,
            midpoint,
        });
        let zoom = (pinch.zoom * distance / pinch.distance).clamp(MIN_ZOOM, MAX_ZOOM);
        let stage = self.stage_rect(cx);
        let centre = stage.pos + stage.size * 0.5;
        let start_anchor = pinch.midpoint - centre;
        let current_anchor = midpoint - centre;
        self.target_pan = current_anchor
            - (start_anchor - pinch.pan) * (zoom / pinch.zoom.max(1e-6));
        self.target_zoom = zoom;
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
        if self.view.button(cx, ids!(save)).clicked(actions) {
            out.push(ImageViewerAction::Save);
        }
        if self.view.button(cx, ids!(copy)).clicked(actions) {
            if let Some(item) = self.item.as_ref() {
                out.push(ImageViewerAction::CopyDigest(item.bytes.digest.clone()));
            }
        }
        if self.view.button(cx, ids!(previous)).clicked(actions) {
            out.push(ImageViewerAction::Step(-1));
        }
        if self.view.button(cx, ids!(next)).clicked(actions) {
            out.push(ImageViewerAction::Step(1));
        }
        if self.view.button(cx, ids!(fit)).clicked(actions) {
            self.fit(cx);
        }
        if self.view.button(cx, ids!(actual)).clicked(actions) {
            self.actual(cx);
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
        self.sync_picture(cx);
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if self.item.is_none() {
            return;
        }
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
                KeyCode::Key0 => self.fit(cx),
                KeyCode::Key1 => self.actual(cx),
                KeyCode::ArrowLeft => {
                    cx.widget_action(self.view.widget_uid(), ImageViewerAction::Step(-1))
                }
                KeyCode::ArrowRight => {
                    cx.widget_action(self.view.widget_uid(), ImageViewerAction::Step(1))
                }
                _ => {}
            }
        }
        match event.hits(cx, self.view.area()) {
            Hit::FingerScroll(event) => {
                let factor = (-event.scroll.y * 0.0035).exp();
                self.zoom_at(cx, event.abs, self.target_zoom * factor);
            }
            Hit::FingerDown(event) => {
                cx.set_key_focus(self.view.area());
                let picture = self.view.view(cx, ids!(picture)).area().rect(cx);
                let bar = self.view.view(cx, ids!(bar)).area().rect(cx);
                if bar.contains(event.abs) {
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
}
