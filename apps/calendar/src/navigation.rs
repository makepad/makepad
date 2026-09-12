//! Exclusive body dispatch and viewport-owned overlay placement.
use crate::presentation::rect;
use crate::presentation::*;
use makepad_widgets::animator::Ease;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.calendar.*
    mod.widgets.CalendarBodyDeckBase = #(CalendarBodyDeck::register_widget(vm))
    mod.widgets.CalendarBodyDeck = set_type_default() do mod.widgets.CalendarBodyDeckBase{
        width:Fill height:Fill flow:Overlay show_bg:false texture_caching:true
        active: @month
        draw_bg +: {image:texture_2d(float) pixel:fn(){return self.image.sample(self.pos)}}
        draw_old +: {image:texture_2d(float) opacity:uniform(1.0) pixel:fn(){return self.image.sample(self.pos)*self.opacity}}
        draw_paper.color:paper
    }
    mod.widgets.CalendarOverlayHostBase = #(CalendarOverlayHost::register_widget(vm))
    mod.widgets.CalendarOverlayHost = set_type_default() do mod.widgets.CalendarOverlayHostBase{
        width:Fill height:Fill flow:Overlay show_bg:false texture_caching:true
        draw_bg +: {image:texture_2d(float) opacity:uniform(1.0) pixel:fn(){return self.image.sample(self.pos)*self.opacity}}
    }
    mod.widgets.CalendarStackPage = StackNavigationView{
        show_bg:true draw_bg.color:paper
        offset:0.0
        header +: {
            height:52 padding:Inset{left:16 right:16 top:4 bottom:4}
            draw_bg.color:paper
            content +: {height:Fill show_bg:false
                title_container +: {height:Fill show_bg:false
                    title +: {padding:0 margin:0 draw_text +: {color:ink text_style:theme.font_bold{font_size:12.75}}}
                }
                button_container +: {height:Fill show_bg:false
                    left_button +: {width:44 height:44 padding:0 margin:0
                        draw_bg +: {pixel:fn(){return vec4(0.0,0.0,0.0,0.0)}}
                        draw_icon.color:action
                    }
                }
                right_container := Plain{width:Fill height:Fill align:Align{x:1.0 y:0.5}
                    header_action := QuietAction{width:72 text:"Done" draw_text.color:action}
                }
            }
        }
        body +: {margin:Inset{top:52} padding:0 show_bg:true draw_bg.color:paper}
        animator +: {slide: {default:@hide
            hide: AnimatorState{ease:Ease.Bezier{cp0:0.2 cp1:0.0 cp2:0.0 cp3:1.0} from:{all:Forward{duration:0.2}} apply:{offset:1.0}}
            show: AnimatorState{ease:Ease.Bezier{cp0:0.2 cp1:0.0 cp2:0.0 cp3:1.0} from:{all:Forward{duration:0.24}} apply:{offset:0.0}}
        }}
    }
}
fn curve() -> Ease {
    Ease::Bezier {
        cp0: 0.2,
        cp1: 0.0,
        cp2: 0.0,
        cp3: 1.0,
    }
}
#[derive(Script, ScriptHook, Widget)]
pub struct CalendarBodyDeck {
    #[deref]
    view: View,
    #[live]
    active: LiveId,
    #[live]
    draw_old: DrawQuad,
    #[live]
    draw_paper: DrawColor,
    #[live(false)]
    pub reduced_motion: bool,
    #[rust]
    frozen: Vec<(ViewTextureSnapshot, f32, f64)>,
    #[rust]
    progress: f64,
    #[rust]
    last: f64,
    #[rust]
    next: NextFrame,
    #[rust]
    duration: f64,
    #[rust]
    direction: f64,
}
impl CalendarBodyDeck {
    pub fn activate(&mut self, cx: &mut Cx, id: LiveId) {
        if self.active == id {
            return;
        }
        self.transition(cx, 0);
        self.active = id;
        self.view.redraw(cx);
    }
    pub fn cut(&mut self, cx: &mut Cx) {
        self.frozen.clear();
        self.progress = 1.0;
        self.last = 0.0;
        self.direction = 0.0;
        self.view.redraw(cx);
    }
    pub fn transition(&mut self, cx: &mut Cx, direction: i32) {
        if let Some(frame) = self.view.take_texture_snapshot(cx) {
            let t = curve().map(self.progress) as f32;
            for (_, weight, offset) in &mut self.frozen {
                *weight *= 1.0 - t;
                *offset -= self.direction * t as f64;
            }
            let weight = if self.frozen.is_empty() { 1.0 } else { t };
            self.frozen
                .push((frame, weight, self.direction * (1.0 - t as f64)));
            self.frozen.retain(|(_, weight, _)| *weight > 0.001);
            if self.frozen.len() > 4 {
                self.frozen.remove(0);
            }
            let sum: f32 = self.frozen.iter().map(|(_, w, _)| w).sum();
            for (_, w, _) in &mut self.frozen {
                *w /= sum.max(0.001);
            }
            self.progress = 0.0;
            self.last = 0.0;
            self.next = cx.new_next_frame();
        }
        self.direction = if self.reduced_motion {
            0.0
        } else {
            direction as f64
        };
        self.duration = if self.reduced_motion {
            0.08
        } else if direction == 0 {
            0.12
        } else {
            0.18
        };
        self.view.redraw(cx);
    }
}
impl Widget for CalendarBodyDeck {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Some(e) = self.next.is_event(event) {
            if self.last != 0.0 {
                self.progress =
                    (self.progress + (e.time - self.last) / self.duration.max(0.001)).min(1.0);
            }
            self.last = e.time;
            if self.progress < 1.0 {
                self.next = cx.new_next_frame();
            } else {
                self.frozen.clear();
            }
            self.view.redraw(cx);
        }
        // Outgoing cached pixels never dispatch; only the selected child is live.
        if let Some((_, child)) = self.view.children.iter().find(|(id, _)| *id == self.active) {
            child.handle_event(cx, event, scope);
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let r = cx.peek_walk_turtle(walk);
        let t = if self.frozen.is_empty() {
            1.0
        } else {
            curve().map(self.progress)
        };
        let slide = self.direction != 0.0 && !self.frozen.is_empty();
        cx.begin_turtle(walk, Layout::default());
        self.draw_paper.color = cx.with_vm(CalendarColors::resolve).paper;
        self.draw_paper.draw_abs(cx, r);
        if slide {
            for (frame, weight, offset) in &self.frozen {
                self.draw_old.draw_vars.set_texture(0, frame.texture());
                self.draw_old
                    .draw_vars
                    .set_uniform(cx, live_id!(opacity), &[*weight]);
                self.draw_old.draw_abs(
                    cx,
                    rect(
                        r.pos.x + (offset - self.direction * t) * r.size.x,
                        r.pos.y,
                        r.size.x,
                        r.size.y,
                    ),
                );
            }
        }
        let all = std::mem::take(&mut self.view.children);
        self.view
            .children
            .extend(all.iter().filter(|(id, _)| *id == self.active).cloned());
        let incoming = if slide {
            self.direction * (1.0 - t) * r.size.x
        } else {
            0.0
        };
        // Both the pixels and the live incoming hit geometry travel within the same clipped viewport.
        self.view.draw_walk(
            cx,
            scope,
            fixed(rect(r.pos.x + incoming, r.pos.y, r.size.x, r.size.y)),
        )?;
        self.view.children = all;
        if !slide {
            let mut accumulated = t as f32;
            for (frame, weight, _) in &self.frozen {
                let contribution = weight * (1.0 - t as f32);
                accumulated += contribution;
                self.draw_old.draw_vars.set_texture(0, frame.texture());
                self.draw_old.draw_vars.set_uniform(
                    cx,
                    live_id!(opacity),
                    &[contribution / accumulated.max(0.00001)],
                );
                self.draw_old.draw_abs(cx, r);
            }
        }
        cx.end_turtle();
        DrawStep::done()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum OverlayKind {
    #[default]
    None,
    Detail,
    Editor,
    Search,
    Calendars,
    Confirmation,
    Agenda,
}
#[derive(Clone, Debug, Default)]
pub enum OverlayAction {
    #[default]
    None,
    Dismiss,
}
pub fn overlay_rect(
    bounds: Rect,
    kind: OverlayKind,
    compact: bool,
    short: bool,
    anchor: Rect,
) -> Rect {
    if short {
        return bounds;
    }
    if compact {
        return if kind == OverlayKind::Editor {
            rect(
                bounds.pos.x,
                bounds.pos.y + 20.0,
                bounds.size.x,
                bounds.size.y - 20.0,
            )
        } else {
            bounds
        };
    }
    let (w, h) = match kind {
        OverlayKind::Editor => (480.0, 640.0),
        OverlayKind::Calendars => (320.0, 252.0),
        OverlayKind::Confirmation => (320.0, 188.0),
        _ => (360.0, 600.0),
    };
    let w = w.min((bounds.size.x - 32.0).max(0.0));
    let h = h.min((bounds.size.y - 32.0).max(0.0));
    if matches!(kind, OverlayKind::Editor | OverlayKind::Confirmation) {
        return rect(
            bounds.pos.x + (bounds.size.x - w) * 0.5,
            bounds.pos.y + (bounds.size.y - h) * 0.5,
            w,
            h,
        );
    }
    let x = if anchor.pos.x + anchor.size.x + 8.0 + w <= bounds.pos.x + bounds.size.x - 16.0 {
        anchor.pos.x + anchor.size.x + 8.0
    } else {
        anchor.pos.x - w - 8.0
    };
    rect(
        x.clamp(
            bounds.pos.x + 16.0,
            bounds.pos.x + (bounds.size.x - w - 16.0).max(16.0),
        ),
        anchor.pos.y.clamp(
            bounds.pos.y + 16.0,
            bounds.pos.y + (bounds.size.y - h - 16.0).max(16.0),
        ),
        w,
        h,
    )
}
#[derive(Script, ScriptHook, Widget)]
pub struct CalendarOverlayHost {
    #[deref]
    view: View,
    #[rust]
    pub active: OverlayKind,
    #[rust]
    pub compact: bool,
    #[rust]
    pub short: bool,
    #[rust]
    pub anchor: Rect,
    #[rust]
    pub keyboard_occlusion: f64,
    #[rust]
    panel_rect: Rect,
    #[rust]
    opening: bool,
    #[rust]
    amount: f64,
    #[rust]
    from: f64,
    #[rust]
    progress: f64,
    #[rust]
    last: f64,
    #[rust]
    next: NextFrame,
    #[live(false)]
    pub reduced_motion: bool,
}
impl CalendarOverlayHost {
    pub fn show(&mut self, cx: &mut Cx, kind: OverlayKind, anchor: Rect) {
        if self.active != kind {
            self.amount = 0.0;
        }
        self.active = kind;
        self.anchor = anchor;
        self.start(cx, true);
    }
    pub fn hide_immediately(&mut self, cx: &mut Cx) {
        self.active = OverlayKind::None;
        self.amount = 0.0;
        self.view.redraw(cx);
    }
    pub fn close(&mut self, cx: &mut Cx) {
        self.start(cx, false);
    }
    fn start(&mut self, cx: &mut Cx, open: bool) {
        self.opening = open;
        self.from = self.amount;
        self.progress = 0.0;
        self.last = 0.0;
        self.next = cx.new_next_frame();
        self.view.redraw(cx);
    }
    pub fn id(&self) -> LiveId {
        match self.active {
            OverlayKind::Detail => live_id!(detail_panel),
            OverlayKind::Editor => live_id!(editor_panel),
            OverlayKind::Search => live_id!(search_panel),
            OverlayKind::Calendars => live_id!(calendars_panel),
            OverlayKind::Confirmation => live_id!(confirmation),
            OverlayKind::Agenda => live_id!(agenda_panel),
            OverlayKind::None => LiveId(0),
        }
    }
    pub fn is_open(&self) -> bool {
        self.active != OverlayKind::None
    }
}
impl Widget for CalendarOverlayHost {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.is_open() {
            return;
        }
        if let Some(e) = self.next.is_event(event) {
            let duration = if self.reduced_motion {
                0.08
            } else if self.active == OverlayKind::Editor {
                if self.opening {
                    0.26
                } else {
                    0.20
                }
            } else if self.opening {
                0.14
            } else {
                0.10
            };
            if self.last != 0.0 {
                self.progress = (self.progress + (e.time - self.last) / duration).min(1.0);
            }
            self.last = e.time;
            let t = curve().map(self.progress);
            self.amount = self.from + ((if self.opening { 1.0 } else { 0.0 }) - self.from) * t;
            if self.progress < 1.0 {
                self.next = cx.new_next_frame();
            } else if !self.opening {
                self.active = OverlayKind::None;
            }
            self.view.redraw(cx);
        }
        if self.opening {
            if let Some((_, panel)) = self.view.children.iter().find(|(id, _)| *id == self.id()) {
                panel.handle_event(cx, event, scope);
            }
            match event.hits(cx, self.view.area()) {
                Hit::FingerDown(_) => {}
                Hit::FingerUp(e) if e.was_tap() && !self.panel_rect.contains(e.abs) => {
                    cx.widget_action(self.widget_uid(), OverlayAction::Dismiss)
                }
                _ => {}
            }
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.is_open() {
            return DrawStep::done();
        }
        let bounds = cx.peek_walk_turtle(walk);
        let usable = rect(
            bounds.pos.x,
            bounds.pos.y,
            bounds.size.x,
            (bounds.size.y - self.keyboard_occlusion).max(52.0),
        );
        let mut r = overlay_rect(usable, self.active, self.compact, self.short, self.anchor);
        if !self.reduced_motion {
            r.pos.y += (1.0 - self.amount)
                * if self.active == OverlayKind::Editor && self.compact {
                    r.size.y
                } else {
                    6.0
                };
        }
        self.panel_rect = r;
        let all = std::mem::take(&mut self.view.children);
        let id = self.id();
        if let Some((_, panel)) = all.iter().find(|(key, _)| *key == id) {
            let mut panel = panel.clone();
            script_apply_eval!(cx,panel,{width:#(r.size.x) height:#(r.size.y) abs_pos:#(r.pos)});
            self.view.children.push((id, panel));
        }
        self.view
            .draw_bg
            .draw_vars
            .set_uniform(cx, live_id!(opacity), &[self.amount as f32]);
        self.view.draw_walk(cx, scope, walk)?;
        self.view.children = all;
        DrawStep::done()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlays_center_and_clamp_at_all_acceptance_sizes() {
        let wide = rect(0.0, 0.0, 1240.0, 800.0);
        assert_eq!(
            overlay_rect(wide, OverlayKind::Editor, false, false, Rect::default()),
            rect(380.0, 80.0, 480.0, 640.0)
        );
        let compact = rect(0.0, 0.0, 402.0, 780.0);
        assert_eq!(
            overlay_rect(compact, OverlayKind::Editor, true, false, Rect::default()),
            rect(0.0, 20.0, 402.0, 760.0)
        );
        let short = rect(0.0, 0.0, 874.0, 300.0);
        assert_eq!(
            overlay_rect(short, OverlayKind::Editor, false, true, Rect::default()),
            short
        );
        let detail = overlay_rect(
            wide,
            OverlayKind::Detail,
            false,
            false,
            rect(1230.0, 795.0, 10.0, 5.0),
        );
        assert!(detail.pos.x >= 16.0 && detail.pos.x + detail.size.x <= 1224.0);
        assert!(detail.pos.y >= 16.0 && detail.pos.y + detail.size.y <= 784.0);
    }
}
