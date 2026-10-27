use crate::{FrameBinding, FrameRead, FrameSource, FrameViewStatus};
use makepad_widgets::*;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::{Rc, Weak};

script_mod! {
    use mod.prelude.widgets_internal.*
    set_type_default() do #(DrawFrame::script_shader(vm)) {
        ..mod.draw.DrawQuad
        // Each frame view records its own draw call, never batched with
        // another view's, so releasing one view's texture (see
        // `LiveFrameView::release_texture`) touches only that view.
        draw_call_always: true
        tex: texture_2d(float)
        cover: 0.0
        ready: 0.0
        border_radius: 0.0
        color: theme.color_inset
        frame_color: fn() -> vec4 {
            if self.ready < 0.5 { return self.color }
            let tex_size = max(self.tex.size(), vec2(1.0, 1.0))
            let scale = self.rect_size / tex_size
            let factor = mix(min(scale.x, scale.y), max(scale.x, scale.y), self.cover)
            let uv = (self.pos - vec2(0.5, 0.5)) * self.rect_size / (tex_size * factor) + vec2(0.5, 0.5)
            if uv.x < 0.0 || uv.y < 0.0 || uv.x > 1.0 || uv.y > 1.0 { return self.color }
            return self.tex.sample(uv)
        }
        pixel: fn() -> vec4 {
            let color = self.frame_color()
            if self.border_radius <= 0.0 { return color }
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, self.border_radius)
            sdf.fill(color)
            return sdf.result
        }
    }
    mod.widgets.FrameFit = #(FrameFit::script_api(vm))
    mod.widgets.LiveFrameView = set_type_default() do #(LiveFrameView::register_widget(vm)) {
        width: Fill height: Fill fit: mod.widgets.FrameFit.Contain
        status_align: Align{x: 0.0 y: 0.0}
        status_padding: 8
        status_wrap: false
        draw_status +: { text_style: theme.font_regular{font_size: 10} color: theme.color_text_disabled }
    }
}

#[derive(Clone, Copy, Debug, Default, Script, ScriptHook)]
pub enum FrameFit {
    #[default]
    Contain,
    Cover,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawFrame {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    cover: f32,
    #[live]
    ready: f32,
    #[live]
    border_radius: f32,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct LiveFrameView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_frame: DrawFrame,
    #[live]
    draw_status: DrawText,
    /// Placement within the padded frame; defaults preserve the top-left label.
    #[live]
    status_align: Align,
    #[live]
    status_padding: Inset,
    #[live]
    status_wrap: bool,
    #[live]
    fit: FrameFit,
    /// The surface binder resolves this separately from numeric endpoints.
    #[live]
    pub show: String,
    #[rust]
    frame: Option<(Rc<dyn FrameSource>, FrameBinding)>,
    #[rust]
    status: FrameViewStatus,
    #[rust]
    bind_error: Option<crate::FrameBindError>,
    /// What the last draw recorded, for [`refresh_shown_frames`].
    #[rust]
    shown: Rc<RefCell<Shown>>,
}

/// A bound frame view as its last draw recorded it: the binding, its draw
/// call's vars (area included), the status and texture that call shows.
#[derive(Default)]
struct Shown {
    frame: Option<(Rc<dyn FrameSource>, FrameBinding)>,
    vars: Option<DrawVars>,
    status: FrameViewStatus,
    texture: Option<TextureId>,
    listed: bool,
}

/// The bound frame views drawn so far (a view lists itself when it draws).
#[derive(Default)]
struct ShownFrames(Vec<Weak<RefCell<Shown>>>);

/// Call once the frame is drawn (during `Event::Draw`, after the windows
/// that show frames have drawn): every bound frame view still on screen
/// reads its source again -- reading is the demand that keeps a session
/// rendering -- and a new texture goes into the draw call it recorded, in
/// place. The view's draw list is not recorded again and nothing around it
/// draws, so a host whose producers animate redraws only its `FrameStage`
/// each frame, not the views (or the UI) that show the pictures. A view
/// whose status changed (pending, ready, an error) draws again next frame.
/// A view whose list is no longer reached from its pass (a hidden page)
/// leaves the list until it draws again.
pub fn refresh_shown_frames(cx: &mut Cx) {
    let views = std::mem::take(&mut cx.global::<ShownFrames>().0);
    if views.is_empty() {
        return;
    }
    let mut attached: Vec<(DrawPassId, HashSet<DrawListId>)> = Vec::new();
    let mut kept = Vec::with_capacity(views.len());
    for view in views {
        let Some(shown) = view.upgrade() else { continue };
        let mut shown = shown.borrow_mut();
        if refresh_shown(cx, &mut shown, &mut attached) {
            kept.push(view);
        } else {
            shown.listed = false;
            shown.vars = None;
        }
    }
    cx.global::<ShownFrames>().0.append(&mut kept);
}

fn refresh_shown(cx: &mut Cx, shown: &mut Shown, attached: &mut Vec<(DrawPassId, HashSet<DrawListId>)>) -> bool {
    let Shown { frame, vars, status, texture, .. } = shown;
    let (Some((source, binding)), Some(vars)) = (frame.as_ref(), vars.as_mut()) else { return false };
    let area = vars.area;
    if !on_screen(cx, area, attached) {
        return false;
    }
    let (now, picture) = match source.read(binding) {
        FrameRead::Ready(picture) => (FrameViewStatus::Ready, Some(picture)),
        FrameRead::Pending => (FrameViewStatus::Pending, None),
        FrameRead::Invalid(error) => (FrameViewStatus::Invalid(error), None),
    };
    if now != *status {
        cx.redraw_area_in_draw(area);
        return true;
    }
    if let Some(picture) = picture {
        if *texture != Some(picture.texture_id()) {
            vars.set_texture(0, &picture);
            if vars.update_uniforms_on_area(cx, area) {
                *texture = Some(picture.texture_id());
            } else {
                cx.redraw_area_in_draw(area);
            }
        }
    }
    true
}

/// The area's draw list is reached from its pass's main list (and the pass
/// is still sampled): what is on screen, not a page a container stopped
/// drawing. One walk per pass per refresh.
fn on_screen(cx: &Cx, area: Area, attached: &mut Vec<(DrawPassId, HashSet<DrawListId>)>) -> bool {
    if !area.is_valid(cx) {
        return false;
    }
    let Some(list) = area.draw_list_id() else { return false };
    let Some(pass) = cx.draw_lists[list].draw_pass_id else { return false };
    if cx.pass_attachment_is_stale(pass) {
        return false;
    }
    if let Some((_, lists)) = attached.iter().find(|(id, _)| *id == pass) {
        return lists.contains(&list);
    }
    let lists = cx.attached_draw_lists(pass);
    let on = lists.contains(&list);
    attached.push((pass, lists));
    on
}

impl WidgetNode for LiveFrameView {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.draw_frame.area()
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_frame.redraw(cx);
    }
}

impl Widget for LiveFrameView {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_frame.ready = 0.0;
        self.status = match self
            .frame
            .as_ref()
            .map(|(source, binding)| source.read(binding))
        {
            None => self
                .bind_error
                .clone()
                .map(FrameViewStatus::Invalid)
                .unwrap_or_default(),
            Some(FrameRead::Pending) => FrameViewStatus::Pending,
            Some(FrameRead::Invalid(error)) => FrameViewStatus::Invalid(error),
            Some(FrameRead::Ready(texture)) => {
                self.draw_frame.draw_vars.set_texture(0, &texture);
                self.draw_frame.ready = 1.0;
                FrameViewStatus::Ready
            }
        };
        if !matches!(self.status, FrameViewStatus::Ready) {
            self.draw_frame.draw_vars.empty_texture(0);
        }
        self.draw_frame.cover = if matches!(self.fit, FrameFit::Cover) {
            1.0
        } else {
            0.0
        };
        self.draw_frame.draw_walk(cx, walk);
        self.note_shown(cx);
        let label = match &self.status {
            FrameViewStatus::Unbound => "",
            FrameViewStatus::Ready => "",
            FrameViewStatus::Pending => "Waiting for frame",
            FrameViewStatus::Invalid(crate::FrameBindError::NoSession) => "Flow is not running",
            FrameViewStatus::Invalid(crate::FrameBindError::NoSuchNode) => "Frame node not found",
            FrameViewStatus::Invalid(crate::FrameBindError::NotAFrameOutput) => {
                "Output is not a frame"
            }
            FrameViewStatus::Invalid(crate::FrameBindError::StaleBinding) => {
                "Frame binding has expired"
            }
        };
        if !label.is_empty() {
            let frame = self.draw_frame.area().rect(cx);
            let padding = self.status_padding;
            let inner = Rect {
                pos: frame.pos + dvec2(padding.left, padding.top),
                size: dvec2(
                    (frame.size.x - padding.left - padding.right).max(0.0),
                    (frame.size.y - padding.top - padding.bottom).max(0.0),
                ),
            };
            if inner.size.x > 0.0 && inner.size.y > 0.0 {
                let text = self.draw_status.layout(
                    cx,
                    0.0,
                    0.0,
                    self.status_wrap.then_some(inner.size.x as f32),
                    self.status_wrap,
                    Align::default(),
                    label,
                );
                let size = dvec2(
                    text.size_in_lpxs.width as f64,
                    text.size_in_lpxs.height as f64,
                );
                let pos = inner.pos
                    + dvec2(
                        (inner.size.x - size.x).max(0.0) * self.status_align.x,
                        (inner.size.y - size.y).max(0.0) * self.status_align.y,
                    );
                if self.status_wrap {
                    cx.push_clip_rect(inner);
                    self.draw_status.draw_walk_laidout(
                        cx,
                        Walk {
                            abs_pos: Some(pos),
                            width: Size::Fixed(size.x),
                            height: Size::Fixed(size.y),
                            ..Walk::default()
                        },
                        &text,
                    );
                    cx.pop_clip_rect();
                } else {
                    self.draw_status.draw_abs(cx, pos, label);
                }
            }
        }
        DrawStep::done()
    }
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

impl LiveFrameView {
    /// Record what this draw showed, and list a bound view for
    /// [`refresh_shown_frames`].
    fn note_shown(&mut self, cx: &mut Cx) {
        let mut shown = self.shown.borrow_mut();
        if self.frame.is_none() {
            shown.frame = None;
            shown.vars = None;
            return;
        }
        shown.frame = self.frame.clone();
        shown.status = self.status.clone();
        shown.texture = self.draw_frame.draw_vars.texture_slots[0].as_ref().map(Texture::texture_id);
        // The snapshot holds no texture of its own: the recorded call does.
        let mut vars = self.draw_frame.draw_vars.clone();
        vars.empty_texture(0);
        shown.vars = Some(vars);
        if !shown.listed {
            shown.listed = true;
            cx.global::<ShownFrames>().0.push(Rc::downgrade(&self.shown));
        }
    }

    /// Drop this view's own references to the frame texture now: the
    /// staging slot, and the slot of the draw call this view recorded (a
    /// recorded call keeps its own texture clone until it is recorded again,
    /// which a hidden view never does). Every frame view records its own
    /// call (`draw_call_always`), so only this view's call is touched, and
    /// only while its area is still valid and still this shader's call;
    /// other views showing the same texture keep theirs. The recorded call
    /// then also stops claiming a ready picture. This releases a GPU
    /// reference, not the provider's source or lease; the binding is kept.
    fn release_texture(&mut self, cx: &mut Cx) {
        // Not refreshed in place again until it draws.
        self.shown.borrow_mut().vars = None;
        self.draw_frame.draw_vars.empty_texture(0);
        self.draw_frame.ready = 0.0;
        let area = self.draw_frame.area();
        // The instance update checks only the area, so it runs only once
        // the uniform update proved the call is still exactly this shader's.
        if self.draw_frame.draw_vars.update_uniforms_on_area(cx, area) {
            self.draw_frame.draw_vars.update_instance_area_value(cx, &[live_id!(ready)]);
        }
    }
}

impl LiveFrameViewRef {
    /// Let go of the frame texture this view holds, at once, keeping its
    /// binding: the next draw reads the provider's current output afresh and
    /// sets the real status. For a view that is hidden (and so will not
    /// redraw) but may be shown again. It does not release the provider's
    /// source or budget; the provider does that when nothing holds its
    /// texture any more.
    pub fn release_frame_texture(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.release_texture(cx);
            if inner.frame.is_some() {
                inner.status = FrameViewStatus::Pending;
            }
            inner.draw_frame.redraw(cx);
        }
    }
    pub fn set_frame(&self, cx: &mut Cx, source: Rc<dyn FrameSource>, binding: FrameBinding) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.release_texture(cx);
            inner.frame = Some((source, binding));
            inner.bind_error = None;
            inner.status = FrameViewStatus::Pending;
            inner.draw_frame.redraw(cx);
        }
    }
    pub fn clear_frame(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.release_texture(cx);
            inner.frame = None;
            inner.bind_error = None;
            inner.status = FrameViewStatus::Unbound;
            inner.draw_frame.redraw(cx);
        }
    }
    pub fn status(&self) -> FrameViewStatus {
        self.borrow()
            .map(|inner| inner.status.clone())
            .unwrap_or_default()
    }
    pub fn set_bind_error(&self, cx: &mut Cx, error: crate::FrameBindError) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.release_texture(cx);
            inner.frame = None;
            inner.bind_error = Some(error.clone());
            inner.status = FrameViewStatus::Invalid(error);
            inner.draw_frame.redraw(cx);
        }
    }
    pub fn show(&self) -> String {
        self.borrow()
            .map(|inner| inner.show.clone())
            .unwrap_or_default()
    }
}
