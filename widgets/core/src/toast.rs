//! Toaster — the layer that says what just happened, out of the way of what
//! the person is doing.
//!
//! A toast is the opposite of a dialog. A dialog stops the work and demands
//! an answer; a toast reports a fact and expects to be ignored. Everything
//! below follows from that. A toast never takes the pointer, never takes
//! the keyboard, and never blocks what is under it. It leaves on its own.
//! Its action is optional and its dismissal is always possible, because the
//! one thing worse than a message nobody reads is a message nobody can get
//! rid of.
//!
//! **One layer, many callers.** An app declares one `Toaster` — as the
//! last child of the window body, so it draws over every panel — and any
//! code that has a `Cx` can raise a toast with [`ToastAction::Show`]. The
//! layer owns the queue, the lifetimes, the stacking and the chrome, so a
//! module that wants to say "saved" does not have to own a widget.
//!
//! **The countdown pauses under the pointer.** A toast that vanishes while
//! it is being read is a toast that has failed, so hovering the stack
//! freezes every countdown in it, and moving away starts them again from
//! where they were. The remaining time is drawn as a hairline along the
//! card's bottom edge: without it a toast either surprises the reader by
//! leaving or is silently indefinite, and neither is honest.
//!
//! **Lifetimes are a promise about attention, not a number.** `Short` is
//! for a fact that needs no thought, `Long` for one that carries an action
//! worth reaching, and `Indefinite` for something that has not finished or
//! has gone wrong. An error that disappears on its own is an error the
//! person never saw, so the layer refuses to time out a toast that has no
//! way of being read twice: an `Error` intent is `Indefinite` unless the
//! caller insists otherwise.
//!
//! **The stack has a limit.** Only `max_visible` cards are on screen; the
//! rest wait, and the layer says how many are waiting rather than covering
//! the window in cards nobody asked for.
//!
//! **One widget, not one per shape.** The bar that says one thing at a time
//! is `Toaster{max_visible: 1}`, and the strip for a message about the whole
//! app is `Toaster{place: TopCenter}`. Each is a property away, so neither
//! has a name of its own to learn.

use crate::{
    badge::{measure, BadgeIntent, BadgePalette},
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
};

/// How long a toast stays before it takes itself away.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum ToastLife {
    /// About four seconds: a fact that needs no thought.
    #[pick]
    Short = 0,
    /// About eight: long enough to reach an action.
    Long = 1,
    /// Until it is dismissed, or the host takes it away.
    Indefinite = 2,
}

impl ToastLife {
    /// The seconds this lifetime is worth, or `None` for one that stays.
    pub fn seconds(self) -> Option<f64> {
        match self {
            ToastLife::Short => Some(4.0),
            ToastLife::Long => Some(8.0),
            ToastLife::Indefinite => None,
        }
    }
}

/// Where the stack sits in the window.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum ToastPlace {
    TopStart = 0,
    TopCenter = 1,
    TopEnd = 2,
    BottomStart = 3,
    #[pick]
    BottomCenter = 4,
    BottomEnd = 5,
}

impl ToastPlace {
    fn at_top(self) -> bool {
        matches!(self, ToastPlace::TopStart | ToastPlace::TopCenter | ToastPlace::TopEnd)
    }

    /// Where a card of `width` starts across a window of `room`.
    fn x(self, room: f64, width: f64, margin: f64) -> f64 {
        match self {
            ToastPlace::TopStart | ToastPlace::BottomStart => margin,
            ToastPlace::TopCenter | ToastPlace::BottomCenter => (room - width) * 0.5,
            ToastPlace::TopEnd | ToastPlace::BottomEnd => room - width - margin,
        }
    }
}

/// One thing to say.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    /// The caller's own id, so it can update or dismiss this toast later.
    /// Showing a toast whose id is already up replaces it in place.
    pub id: LiveId,
    pub title: String,
    /// A second line, for the detail the title cannot hold.
    pub body: String,
    pub intent: BadgeIntent,
    pub life: ToastLife,
    /// The label of the one thing this toast offers to do. Empty for none.
    pub action: String,
    /// Whether the reader may take it away by hand.
    pub closable: bool,
}

impl Toast {
    pub fn new(id: LiveId, title: &str) -> Self {
        Toast {
            id,
            title: title.to_string(),
            body: String::new(),
            intent: BadgeIntent::Neutral,
            life: ToastLife::Short,
            action: String::new(),
            closable: true,
        }
    }

    pub fn body(mut self, body: &str) -> Self {
        self.body = body.to_string();
        self
    }

    /// The role, and with it the default lifetime: an error stays until it
    /// is dealt with, because one that leaves on its own was never read.
    pub fn intent(mut self, intent: BadgeIntent) -> Self {
        self.intent = intent;
        if intent == BadgeIntent::Error {
            self.life = ToastLife::Indefinite;
        }
        self
    }

    pub fn life(mut self, life: ToastLife) -> Self {
        self.life = life;
        self
    }

    pub fn action(mut self, label: &str) -> Self {
        self.action = label.to_string();
        self
    }

    pub fn closable(mut self, on: bool) -> Self {
        self.closable = on;
        self
    }
}

/// The toast bus.
#[derive(Clone, Debug)]
pub enum ToastAction {
    /// Say something. An id already on screen is replaced in place, which
    /// is how a progress message becomes a result without flickering.
    Show(Toast),
    /// Take one away, whoever put it up.
    Dismiss(LiveId),
    /// Take them all away.
    DismissAll,
    /// The reader pressed a toast's action.
    ActionPressed(LiveId),
    /// A toast went away: by its own clock, by its close button, or
    /// because the host asked.
    Closed(LiveId),
}

/// Every toast action in a pass.
pub fn toast_actions(actions: &Actions) -> impl Iterator<Item = &ToastAction> {
    actions.iter().filter_map(|a| a.downcast_ref::<ToastAction>())
}

/// The id whose action was pressed this pass, if one was.
pub fn toast_action_pressed(actions: &Actions) -> Option<LiveId> {
    toast_actions(actions).find_map(|a| match a {
        ToastAction::ActionPressed(id) => Some(*id),
        _ => None,
    })
}

/// A card on screen, with what is left of its clock.
#[derive(Clone, Debug)]
struct Live {
    toast: Toast,
    /// Seconds left, or `None` for a toast that stays.
    left: Option<f64>,
    /// Where it was drawn, for hit testing.
    rect: Rect,
    close_rect: Rect,
    action_rect: Rect,
    /// 0 while it is arriving, 1 once it has arrived.
    enter: f64,
}

const CARD_W: f64 = 340.0;
const CARD_PAD: f64 = 12.0;
const CARD_GAP: f64 = 8.0;
const MARGIN: f64 = 16.0;
const CLOSE_SIZE: f64 = 16.0;
/// How long a card takes to arrive.
const ENTER_SECS: f64 = 0.18;

/// The card, its edge and the hairline that counts its time down.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawToast {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    border_color: Vec4f,
    #[live]
    accent: Vec4f,
    #[live]
    radius: f32,
    /// What is left of the toast's time, 0..1. Negative draws no line.
    #[live]
    countdown: f32,
    /// 0 while the card is arriving, 1 once it has arrived.
    #[live]
    enter: f32,
    /// Which edge a card arrives from: 1 below, -1 above.
    #[live]
    from_below: f32,
}

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.ToastLife = set_type_default() do #(ToastLife::script_api(vm))
    mod.widgets.splat(mod.widgets.ToastLife)
    mod.widgets.ToastPlace = set_type_default() do #(ToastPlace::script_api(vm))
    mod.widgets.splat(mod.widgets.ToastPlace)

    use mod.widgets.*

    mod.widgets.DrawToastBase = #(DrawToast::script_component(vm))
    set_type_default() do #(DrawToast::script_shader(vm)){
        ..mod.draw.DrawQuad
        countdown: -1.0
        enter: 1.0
        /** which way a card arrives from: 1 below, -1 above -1..1 step 2 */
        from_below: 1.0
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            // Arriving cards slide a little and fade: the movement is what
            // catches the eye, and the fade is what stops it shouting. It
            // slides IN FROM ITS OWN EDGE, so a stack at the top drops down
            // and one at the bottom rises. Always rising made a top-placed
            // card look as though it were being pulled off the screen.
            let lift = (1.0 - self.enter) * 8.0 * self.from_below
            sdf.box(0.5, 0.5 + lift, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
            sdf.fill_keep(vec4(self.color.xyz, self.color.a * self.enter))
            sdf.stroke(vec4(self.border_color.xyz, self.border_color.a * self.enter), 1.0)
            if self.countdown >= 0.0 {
                let h = 2.0
                let w = (self.rect_size.x - 2.0) * self.countdown
                sdf.box(1.0, self.rect_size.y - h - 1.0 + lift, w, h, h * 0.5)
                sdf.fill(vec4(self.accent.xyz, self.accent.a * self.enter))
            }
            return sdf.result
        }
    }

    mod.widgets.ToasterBase = #(Toaster::register_widget(vm))
    /** The one layer that says what just happened. Declare it once. */
    mod.widgets.Toaster = set_type_default() do mod.widgets.ToasterBase{
        width: Fill
        height: Fill
        /** where the stack sits: TopStart TopCenter TopEnd BottomStart BottomCenter BottomEnd */
        place: BottomCenter
        /** how many cards are on screen at once; the rest wait 1..8 step 1 */
        max_visible: 3
        draw_bg +: {
            color: theme.color_surface_container_high
            border_color: theme.color_outline
            accent: theme.color_primary
            radius: theme.radius_m
        }
        draw_title +: {
            text_style: theme.font_label_m
            color: theme.color_on_surface
        }
        draw_body +: {
            text_style: theme.font_body_s
            color: theme.color_on_surface_variant
        }
        draw_action +: {
            text_style: theme.font_label_m
            color: theme.color_primary
        }
        draw_close +: {
            text_style: theme.font_body_m
            color: theme.color_on_surface_variant
        }
        draw_waiting +: {
            text_style: theme.font_body_s
            color: theme.color_on_surface_variant
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct Toaster {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[live]
    draw_list: DrawList2d,
    #[redraw]
    #[live]
    draw_bg: DrawToast,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_body: DrawText,
    #[live]
    draw_action: DrawText,
    #[live]
    draw_close: DrawText,
    #[live]
    draw_waiting: DrawText,
    #[live]
    palette: BadgePalette,
    #[walk]
    walk: Walk,

    /// Where the stack sits.
    #[live]
    pub place: ToastPlace,
    /// How many cards are on screen at once.
    #[live(3usize)]
    pub max_visible: usize,

    #[rust]
    area: Area,
    #[rust]
    window: Rect,
    /// The cards on screen, newest last.
    #[rust]
    live: Vec<Live>,
    /// What is waiting for room.
    #[rust]
    queue: Vec<Toast>,
    /// True while the pointer is over the stack: every clock is frozen.
    #[rust]
    paused: bool,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    last_t: f64,
}

impl Toaster {
    /// Say something. An id already on screen is replaced in place.
    pub fn show(&mut self, cx: &mut Cx, toast: Toast) {
        if let Some(slot) = self.live.iter_mut().find(|l| l.toast.id == toast.id) {
            slot.left = toast.life.seconds();
            slot.toast = toast;
            self.wake(cx);
            return;
        }
        if let Some(slot) = self.queue.iter_mut().find(|t| t.id == toast.id) {
            *slot = toast;
            self.wake(cx);
            return;
        }
        if self.live.len() < self.max_visible.max(1) {
            self.live.push(Live {
                left: toast.life.seconds(),
                toast,
                rect: Rect::default(),
                close_rect: Rect::default(),
                action_rect: Rect::default(),
                enter: 0.0,
            });
        } else {
            self.queue.push(toast);
        }
        self.wake(cx);
    }

    /// Take one away, whoever put it up.
    pub fn dismiss(&mut self, cx: &mut Cx, id: LiveId) {
        let before = self.live.len() + self.queue.len();
        self.live.retain(|l| l.toast.id != id);
        self.queue.retain(|t| t.id != id);
        if self.live.len() + self.queue.len() != before {
            cx.action(ToastAction::Closed(id));
            self.promote();
            self.wake(cx);
        }
    }

    pub fn dismiss_all(&mut self, cx: &mut Cx) {
        for l in std::mem::take(&mut self.live) {
            cx.action(ToastAction::Closed(l.toast.id));
        }
        for t in std::mem::take(&mut self.queue) {
            cx.action(ToastAction::Closed(t.id));
        }
        self.wake(cx);
    }

    /// How many are waiting for room.
    pub fn waiting(&self) -> usize {
        self.queue.len()
    }

    /// Move what is waiting onto the screen, while there is room.
    fn promote(&mut self) {
        while self.live.len() < self.max_visible.max(1) && !self.queue.is_empty() {
            let toast = self.queue.remove(0);
            self.live.push(Live {
                left: toast.life.seconds(),
                toast,
                rect: Rect::default(),
                close_rect: Rect::default(),
                action_rect: Rect::default(),
                enter: 0.0,
            });
        }
    }

    fn wake(&mut self, cx: &mut Cx) {
        self.next_frame = cx.new_next_frame();
        self.draw_list.redraw(cx);
        self.area.redraw(cx);
    }

    /// Run every clock forward. Answers whether anything is still moving.
    fn tick(&mut self, cx: &mut Cx) -> bool {
        let now = cx.seconds_since_app_start();
        let dt = if self.last_t > 0.0 { (now - self.last_t).clamp(0.0, 0.1) } else { 0.0 };
        self.last_t = now;
        let mut moving = false;
        let mut expired: Vec<LiveId> = Vec::new();
        for card in &mut self.live {
            if card.enter < 1.0 {
                card.enter = (card.enter + dt / ENTER_SECS).min(1.0);
                moving = true;
            }
            // A frozen clock is the whole point of hovering a toast: a
            // message that leaves while it is being read has failed.
            if self.paused {
                continue;
            }
            if let Some(left) = card.left.as_mut() {
                *left -= dt;
                moving = true;
                if *left <= 0.0 {
                    expired.push(card.toast.id);
                }
            }
        }
        for id in expired {
            self.live.retain(|l| l.toast.id != id);
            cx.action(ToastAction::Closed(id));
        }
        self.promote();
        moving || !self.live.is_empty() && !self.paused
    }

    fn card_height(&mut self, cx: &mut Cx2d, card: &Live) -> f64 {
        let mut h = CARD_PAD * 2.0 + self.draw_title.text_style.font_size as f64 * 1.5;
        if !card.toast.body.is_empty() {
            h += self.draw_body.text_style.font_size as f64 * 1.5;
        }
        let _ = cx;
        h
    }
}

impl Widget for Toaster {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        // The layer's own rect is the origin its overlay pass is shifted FROM,
        // and nothing else. The room the stack is placed in is the WHOLE PASS:
        // a toaster declared as the last child of a page gets whatever space is
        // left at the bottom of it, and pinning the corners to that strip puts
        // a top-left stack part-way down the screen.
        let origin = cx.turtle().rect().pos;
        cx.end_turtle_with_area(&mut self.area);
        let window = Rect { pos: dvec2(0.0, 0.0), size: cx.current_pass_size() };
        self.window = window;
        if self.live.is_empty() {
            return DrawStep::done();
        }
        self.draw_list.begin_overlay_reuse(cx);
        let pass = cx.current_pass_size();
        cx.begin_root_turtle(pass, Layout::flow_down());

        let cards = self.live.clone();
        // Newest nearest the edge the stack grows from, so the one that
        // just arrived is the one nearest the eye.
        let mut y = if self.place.at_top() {
            window.pos.y + MARGIN
        } else {
            window.pos.y + window.size.y - MARGIN
        };
        let waiting = self.queue.len();
        for (i, card) in cards.iter().enumerate() {
            let h = self.card_height(cx, card);
            let x = window.pos.x + self.place.x(window.size.x, CARD_W, MARGIN);
            let top = if self.place.at_top() { y } else { y - h };
            let rect = Rect { pos: dvec2(x, top), size: dvec2(CARD_W, h) };

            let family = self.palette.family(card.toast.intent);
            self.draw_bg.accent = if card.toast.intent == BadgeIntent::Neutral {
                self.draw_bg.accent
            } else {
                family.base
            };
            self.draw_bg.countdown = match (card.left, card.toast.life.seconds()) {
                (Some(left), Some(total)) if total > 0.0 => (left / total).clamp(0.0, 1.0) as f32,
                _ => -1.0,
            };
            self.draw_bg.enter = card.enter as f32;
            self.draw_bg.from_below = if self.place.at_top() { -1.0 } else { 1.0 };
            self.draw_bg.draw_abs(cx, rect);

            let mut ty = rect.pos.y + CARD_PAD;
            self.draw_title.draw_abs(cx, dvec2(rect.pos.x + CARD_PAD, ty), &card.toast.title);
            ty += self.draw_title.text_style.font_size as f64 * 1.5;
            if !card.toast.body.is_empty() {
                self.draw_body.draw_abs(cx, dvec2(rect.pos.x + CARD_PAD, ty), &card.toast.body);
            }

            // The action sits at the trailing edge, where the eye ends up.
            let mut action_rect = Rect::default();
            if !card.toast.action.is_empty() {
                let w = measure(&self.draw_action, cx, &card.toast.action);
                let ax = rect.pos.x + rect.size.x - CARD_PAD - w
                    - if card.toast.closable { CLOSE_SIZE + CARD_GAP } else { 0.0 };
                let ay = rect.pos.y + (rect.size.y - self.draw_action.text_style.font_size as f64 * 1.4) * 0.5;
                self.draw_action.draw_abs(cx, dvec2(ax, ay), &card.toast.action);
                action_rect = Rect { pos: dvec2(ax - 4.0, rect.pos.y), size: dvec2(w + 8.0, rect.size.y) };
            }
            let mut close_rect = Rect::default();
            if card.toast.closable {
                let cx_pos = rect.pos.x + rect.size.x - CARD_PAD - CLOSE_SIZE;
                let cy_pos = rect.pos.y + (rect.size.y - CLOSE_SIZE) * 0.5;
                self.draw_close.draw_abs(cx, dvec2(cx_pos + 3.0, cy_pos), "\u{00d7}");  // the multiplication sign: the text font has it, the heavy cross it does not
                close_rect = Rect { pos: dvec2(cx_pos, cy_pos), size: dvec2(CLOSE_SIZE, CLOSE_SIZE) };
            }

            if let Some(slot) = self.live.get_mut(i) {
                slot.rect = rect;
                slot.close_rect = close_rect;
                slot.action_rect = action_rect;
            }
            y = if self.place.at_top() { y + h + CARD_GAP } else { top - CARD_GAP };
        }

        // What is waiting, rather than a window full of cards.
        if waiting > 0 {
            let x = window.pos.x + self.place.x(window.size.x, CARD_W, MARGIN);
            let text = if waiting == 1 {
                "1 more".to_string()
            } else {
                format!("{waiting} more")
            };
            let ty = if self.place.at_top() { y } else { y - 14.0 };
            self.draw_waiting.draw_abs(cx, dvec2(x + CARD_PAD, ty), &text);
        }

        cx.end_pass_sized_turtle_with_shift(self.area, dvec2(0.0, 0.0) - origin);
        self.draw_list.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() && self.tick(cx) {
            self.wake(cx);
        }
        if let Event::Actions(actions) = event {
            let mut asks: Vec<ToastAction> = Vec::new();
            for a in actions.iter() {
                if let Some(action) = a.downcast_ref::<ToastAction>() {
                    asks.push(action.clone());
                }
            }
            for ask in asks {
                match ask {
                    ToastAction::Show(toast) => self.show(cx, toast),
                    ToastAction::Dismiss(id) => self.dismiss(cx, id),
                    ToastAction::DismissAll => self.dismiss_all(cx),
                    _ => {}
                }
            }
        }
        if self.live.is_empty() {
            return;
        }
        match event {
            // A toast takes no pointer of its own: it only watches where
            // the pointer is, so that what is under it stays live.
            Event::MouseMove(e) => {
                let over = self.live.iter().any(|l| l.rect.contains(e.abs));
                if over != self.paused {
                    self.paused = over;
                    self.last_t = cx.seconds_since_app_start();
                    self.wake(cx);
                }
            }
            Event::MouseUp(e) => {
                let mut hit: Option<(LiveId, bool)> = None;
                for card in &self.live {
                    if card.toast.closable && card.close_rect.contains(e.abs) {
                        hit = Some((card.toast.id, false));
                        break;
                    }
                    if !card.toast.action.is_empty() && card.action_rect.contains(e.abs) {
                        hit = Some((card.toast.id, true));
                        break;
                    }
                }
                if let Some((id, is_action)) = hit {
                    if is_action {
                        cx.action(ToastAction::ActionPressed(id));
                    }
                    self.dismiss(cx, id);
                }
            }
            _ => {}
        }
    }

    /// The title of the newest toast on screen, so a test can wait on what
    /// was said rather than on pixels.
    fn text(&self) -> String {
        self.live.last().map(|l| l.toast.title.clone()).unwrap_or_default()
    }

    /// How many toasts are on screen.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.live.len().to_string())
    }
}

impl ToasterRef {
    pub fn show(&self, cx: &mut Cx, toast: Toast) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.show(cx, toast);
        }
    }

    pub fn dismiss(&self, cx: &mut Cx, id: LiveId) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.dismiss(cx, id);
        }
    }

    pub fn dismiss_all(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.dismiss_all(cx);
        }
    }

    /// How many are on screen.
    /// Move the stack to another corner. A host that adapts its layout can
    /// say so at runtime rather than declaring one corner for good.
    pub fn set_place(&self, cx: &mut Cx, place: ToastPlace) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.place != place {
                inner.place = place;
                inner.redraw(cx);
            }
        }
    }

    /// Which corner the stack sits in.
    pub fn place(&self) -> ToastPlace {
        self.borrow().map(|inner| inner.place).unwrap_or(ToastPlace::BottomCenter)
    }

    pub fn showing(&self) -> usize {
        self.borrow().map(|inner| inner.live.len()).unwrap_or(0)
    }

    /// How many are waiting for room.
    pub fn waiting(&self) -> usize {
        self.borrow().map(|inner| inner.waiting()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An error that leaves on its own is an error nobody read, so the
    /// role sets the lifetime unless the caller says otherwise.
    #[test]
    fn an_error_stays_until_it_is_dealt_with() {
        let plain = Toast::new(live_id!(a), "Saved");
        assert_eq!(plain.life, ToastLife::Short);
        let bad = Toast::new(live_id!(b), "Could not save").intent(BadgeIntent::Error);
        assert_eq!(bad.life, ToastLife::Indefinite);
        assert_eq!(bad.life.seconds(), None);
        // The caller still has the last word.
        let insisted = Toast::new(live_id!(c), "Could not save")
            .intent(BadgeIntent::Error)
            .life(ToastLife::Long);
        assert_eq!(insisted.life.seconds(), Some(8.0));
    }

    /// The lifetimes are ordered and the indefinite one is not a number.
    #[test]
    fn the_lifetimes_are_ordered() {
        let short = ToastLife::Short.seconds().unwrap();
        let long = ToastLife::Long.seconds().unwrap();
        assert!(short < long);
        assert_eq!(ToastLife::Indefinite.seconds(), None);
    }

    /// The stack grows from the edge it is placed against, and a centred
    /// stack stays centred whatever the window does.
    #[test]
    fn the_stack_sits_where_it_was_placed() {
        assert!(ToastPlace::TopCenter.at_top());
        assert!(!ToastPlace::BottomEnd.at_top());
        assert_eq!(ToastPlace::TopStart.x(1000.0, 340.0, 16.0), 16.0);
        assert_eq!(ToastPlace::BottomCenter.x(1000.0, 340.0, 16.0), 330.0);
        assert_eq!(ToastPlace::TopEnd.x(1000.0, 340.0, 16.0), 644.0);
    }
}
