//! The AI pane: a left-side column that hosts the assistant and PUSHES
//! the desk in — never over it.
//!
//! The chat is NOT the window manager's — it is an app of its own
//! (`apps/aichat`), seated here instead of in a tile, in one of two
//! bodies. On a desktop it is the aichat CHILD PROCESS, launched on first
//! use and presented by one `MpRunView` that forwards its input. Where
//! the chat is linked in as a MODULE — the web superbuild, or a desktop
//! that switched `aichat` to module hosting — the body is the chat's own
//! root, `mod.widgets.AiChatOverlay{}`, instantiated BY NAME on first open
//! exactly as a Window's F10 slot does; the WM's services reach it as
//! in-process links (`pane_links.rs`), never as frames.
//!
//! This widget owns nothing but the slot. It is the first child of the
//! desk row, LEFT of the desk, and it reserves a STRIP: its own walk width
//! (decision 17). The slide (easeOutQuint, 260 ms in, 200 ms out) animates
//! that strip from nothing to the card's width plus one outer gap, and
//! the desk — a Fill sibling in the same row — narrows by exactly that
//! every frame, so the tiles reflow beside the pane instead of vanishing
//! under it; the card itself sits right-aligned in the strip, an opaque
//! ground with a 2 px edge on its RIGHT. Open, the pane takes the pointer
//! inside its card and the keyboard; closed, the strip is zero and the
//! card sits just off the desk's left edge, still drawn, so the assistant —
//! started with the desktop, never on the first F10 — is configured,
//! presenting and answering before the pane ever slides in, and a turn in
//! flight finishes behind a hidden one. Closed at rest, once the app has
//! drawn its own frame, its client is told it is hidden (`HostedVisible`):
//! it keeps answering but holds its drawing, so an off-screen card no longer
//! keeps the whole desktop drawing at the app's frame rate. An open tells it
//! it shows again and starts the slide with its next frame (`BodyShown`):
//! the picture it held meanwhile may be hours old, and the slide's first
//! frame already shows a quarter of the card.

use crate::desk::ease_out_quint;
use makepad_app_host::ClientId;
use crate::run_view::MpRunView;
use crate::domain_view::MpDomainView;
use crate::tile::TileHost;
use makepad_widgets::makepad_script::script_eval;
use makepad_widgets::makepad_script::trap::NoTrap;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ShellAiPaneBase = #(ShellAiPane::register_widget(vm))

    mod.widgets.ShellAiPane = set_type_default() do mod.widgets.ShellAiPaneBase {
        // The strip: zero while closed, the card plus a gap when open. The
        // widget rewrites this width itself as it slides.
        width: 0
        height: Fill
        draw_card +: { color: mod.wm_theme.background }
        draw_edge +: { color: mod.wm_theme.accent }
        run := MpRunView{}
    }
}

/// Seconds the slide-in takes.
const SLIDE_IN: f64 = 0.26;
/// Seconds the slide-out takes.
const SLIDE_OUT: f64 = 0.20;
/// The pane's width, before the screen-fraction clamp.
const PANE_WIDTH: f64 = 440.0;
/// The most of the desk the pane may take.
const PANE_MAX_FRACTION: f64 = 0.4;
/// The edge on the pane's right.
const EDGE: f64 = 2.0;
/// The narrowest the card ever gets.
const PANE_MIN_WIDTH: f64 = 240.0;

/// The pane's geometry as plain numbers, testable without a `Cx`: the card
/// width for a row that wide, the strip the row reserves at slide position
/// `t`, and the card's rect inside a strip.
pub struct PaneStrip;

impl PaneStrip {
    /// The card's width: 440 px, at most 40 % of the row, never under 240.
    pub fn card_width(row_width: f64) -> f64 {
        PANE_WIDTH.min(row_width * PANE_MAX_FRACTION).max(PANE_MIN_WIDTH)
    }

    /// What the row reserves at slide position `t` (0 hidden, 1 shown):
    /// the card plus one outer gap, eased — the desk beside it narrows by
    /// exactly this.
    pub fn strip(t: f64, card_width: f64, gap: f64) -> f64 {
        (ease_out_quint(t) * (card_width + gap)).max(0.0)
    }

    /// The card, right-aligned in a strip of `strip` px starting at
    /// `row.pos.x`: at rest one gap in from the row's left edge, and at
    /// strip zero fully off the row's left edge. Full desk height minus
    /// the gaps.
    pub fn card_rect(row: Rect, strip: f64, card_width: f64, gap: f64) -> Rect {
        let x = row.pos.x + strip - card_width;
        let y = row.pos.y + gap;
        let h = (row.size.y - gap * 2.0).max(100.0);
        Rect { pos: dvec2(x, y), size: dvec2(card_width, h) }
    }
}

/// What the pane's body is told of its showing, testable without a `Cx`
/// (the module doc): hidden while the pane is closed at rest, and an open
/// that told a hidden client it shows waits for its fresh frame.
#[derive(Default)]
struct BodyShown {
    /// The body's presents when its client was last told it is hidden. A
    /// present since is a client that does not know (a new image of its
    /// process): it is told again.
    told_hidden: Option<u64>,
    /// An open waits for a present past this one before the slide starts.
    opening_after: Option<u64>,
}

/// What the body's showing asks for now ([`BodyShown::step`]).
#[derive(Debug, PartialEq)]
enum BodyStep {
    Rest,
    /// Tell the client it is hidden.
    Hide,
    /// The fresh frame an open waited for came: start the slide.
    Slide,
}

impl BodyShown {
    /// The pane opens: true when its client was told it is hidden, so it
    /// must be told it shows and the slide wait ([`Self::shown_at`]).
    fn opening(&mut self) -> bool {
        self.told_hidden.take().is_some()
    }

    /// The client was told it shows with `presents` frames made: the slide
    /// waits for the next.
    fn shown_at(&mut self, presents: u64) {
        self.opening_after = Some(presents);
    }

    /// The client was told it is hidden with `presents` frames made.
    fn hidden_at(&mut self, presents: u64) {
        self.told_hidden = Some(presents);
    }

    /// The pane closes: an open's wait is over.
    fn cancel(&mut self) {
        self.opening_after = None;
    }

    /// What to do with the card `showing` (open, or sliding), the app having
    /// `drawn` its own frame, the body at `presents` frames.
    fn step(&mut self, showing: bool, drawn: bool, presents: u64) -> BodyStep {
        if let Some(after) = self.opening_after {
            if presents <= after {
                return BodyStep::Rest;
            }
            self.opening_after = None;
            return BodyStep::Slide;
        }
        if showing || !drawn || self.told_hidden == Some(presents) {
            return BodyStep::Rest;
        }
        BodyStep::Hide
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct ShellAiPane {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[live]
    draw_card: DrawColor,
    #[live]
    draw_edge: DrawColor,
    #[rust]
    open: bool,
    /// 0 = fully hidden, 1 = fully shown.
    #[rust]
    t: f64,
    #[rust]
    anim_started: Option<(f64, f64, bool)>,
    #[rust]
    client: Option<ClientId>,
    /// The desk ROW's rect (under the bar, pane and desk together) and the
    /// outer gap, as the WM last told us; the card width and the strip
    /// derive from them.
    #[rust]
    row_rect: Rect,
    #[rust]
    gap: f64,
    /// The strip this widget currently reserves (its own walk width).
    #[rust]
    strip: f64,
    #[rust]
    next_frame: NextFrame,
    /// The keyboard was asked for while the body had no live area;
    /// claimed right after its next draw.
    #[rust]
    focus_pending: bool,
    /// The chat's module root, when the chat runs in-process: made by
    /// name on the first open, kept for the life of the desk.
    #[rust]
    overlay: Option<WidgetRef>,
    /// What the body's client was told of its showing.
    #[rust]
    shown: BodyShown,
}

impl ShellAiPane {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The chat runs in this process: the body is its module root.
    pub fn is_local(&self) -> bool {
        self.overlay.is_some()
    }

    /// Seat the chat's module root here, made BY NAME: `mod.widgets.
    /// AiChatOverlay{}` exists when this build links `makepad-app-aichat` and
    /// called its `script_mod`. False, with one log line, when it does not.
    pub fn ensure_overlay(&mut self, cx: &mut Cx) -> bool {
        if self.overlay.is_some() {
            return true;
        }
        let overlay = cx.with_vm(|vm| {
            let widgets = vm.module(id!(widgets));
            let ty = vm.heap().value_path(widgets, ids!(AiChatOverlay), NoTrap);
            if ty.as_object().is_none() {
                return None;
            }
            let value = script_eval!(vm, {
                use mod.widgets.*
                AiChatOverlay {}
            });
            Some(WidgetRef::script_from_value(vm, value))
        });
        match overlay {
            Some(overlay) => {
                cx.widget_tree_insert_child(self.widget_uid(), live_id!(overlay), overlay.clone());
                self.overlay = Some(overlay);
                true
            }
            None => {
                log!("wm: this build does not link the assistant (no mod.widgets.AiChatOverlay); nothing to seat in the pane");
                false
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_domain_client(&mut self, cx: &mut Cx, client: ClientId) {
        let overlay = cx.with_vm(|vm| {
            let value = script_eval!(vm, { use mod.widgets.* MpDomainView {} });
            WidgetRef::script_from_value(vm, value)
        });
        if let Some(mut view) = overlay.borrow_mut::<MpDomainView>() { view.set_client(client); }
        cx.widget_tree_insert_child(self.widget_uid(), live_id!(overlay), overlay.clone());
        self.overlay = Some(overlay);
        self.client = Some(client);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_domain_view<R>(&mut self, cx: &mut Cx,
        f: impl FnOnce(&mut Cx, &mut MpDomainView) -> R) -> Option<R> {
        let mut view = self.overlay.as_ref()?.borrow_mut::<MpDomainView>()?;
        Some(f(cx, &mut view))
    }

    pub fn client(&self) -> Option<ClientId> {
        self.client
    }

    pub fn set_client(&mut self, client: Option<ClientId>) {
        self.client = client;
    }

    /// Where the desk row is and how far from its edges the card sits.
    pub fn set_geometry(&mut self, row_rect: Rect, gap: f64) {
        self.row_rect = row_rect;
        self.gap = gap;
        self.apply_strip();
    }

    /// The card's width for the current row.
    fn card_width(&self) -> f64 {
        PaneStrip::card_width(self.row_rect.size.x)
    }

    /// The strip the row reserves right now, and the walk that claims it.
    fn apply_strip(&mut self) {
        self.strip = PaneStrip::strip(self.t, self.card_width(), self.gap);
        self.view.walk.width = Size::Fixed(self.strip);
    }

    /// The reserved strip in px: what the desk beside the pane has lost.
    pub fn strip(&self) -> f64 {
        self.strip
    }

    /// The card is on the move: the desk snaps its tiles to the layout
    /// every frame instead of tweening after the strip.
    pub fn is_sliding(&self) -> bool {
        self.anim_started.is_some()
    }

    /// The card as drawn right now: right-aligned in the strip.
    fn card_rect(&self) -> Rect {
        PaneStrip::card_rect(self.row_rect, self.strip, self.card_width(), self.gap)
    }

    /// True while the pane is on screen at all (open, or sliding out).
    fn showing(&self) -> bool {
        self.open || self.t > 0.0
    }

    pub fn contains(&self, p: DVec2) -> bool {
        self.open && self.card_rect().contains(p)
    }

    pub fn set_open(&mut self, cx: &mut Cx, open: bool) {
        if self.open == open {
            return;
        }
        self.open = open;
        self.shown.cancel();
        if open && self.shown.opening() {
            if let Some(Some(presents)) = self.with_body(cx, |cx, b| b.set_visible(cx, true).then(|| b.presents())) {
                // The slide starts with its next frame (`sync_body`).
                self.shown.shown_at(presents);
                return;
            }
        }
        self.slide(cx);
        if !open {
            // A hidden pane must not keep the keyboard: typing would go on
            // reaching the chat behind nothing.
            self.focus_pending = false;
            if self.overlay.is_some() {
                if let Some(mut view) = self.overlay.as_ref().and_then(|o| o.borrow_mut::<MpDomainView>()) { view.release_keyboard(cx); }
                cx.set_key_focus(Area::Empty);
            } else {
                self.with_run_view(cx, |cx, v| v.release_keyboard(cx));
            }
        }
        self.redraw(cx);
    }

    /// Slide toward `open` from where the card is now.
    fn slide(&mut self, cx: &mut Cx) {
        self.anim_started = Some((cx.seconds_since_app_start(), self.t, self.open));
        self.next_frame = cx.new_next_frame();
    }

    /// The body that has a client to tell what shows of it: the child's run
    /// view, or a HAL domain's view. None for a module root (it draws in
    /// this process).
    fn with_body<R>(&mut self, cx: &mut Cx, f: impl FnOnce(&mut Cx, &mut dyn TileHost) -> R) -> Option<R> {
        match self.overlay.clone() {
            Some(overlay) => {
                let mut view = overlay.borrow_mut::<MpDomainView>()?;
                Some(f(cx, &mut *view))
            }
            None => self.with_run_view(cx, |cx, v| f(cx, v)),
        }
    }

    /// Tell the body's client what shows of it ([`BodyShown`]), and start an
    /// open's slide once the frame it waited for came.
    fn sync_body(&mut self, cx: &mut Cx) {
        let showing = self.showing();
        let Some((drawn, presents)) = self.with_body(cx, |_, b| (b.app_drawn(), b.presents())) else { return };
        match self.shown.step(showing, drawn, presents) {
            BodyStep::Rest => {}
            BodyStep::Slide => {
                self.slide(cx);
                self.redraw(cx);
            }
            BodyStep::Hide => {
                if self.with_body(cx, |cx, b| b.set_visible(cx, false)) == Some(true) {
                    self.shown.hidden_at(presents);
                }
            }
        }
    }

    /// The child is gone: no client, no picture, pane shut.
    pub fn reset(&mut self, cx: &mut Cx) {
        self.shown = BodyShown::default();
        self.client = None;
        self.with_run_view(cx, |cx, v| v.clear_run_target(cx));
        self.set_open(cx, false);
        if self.overlay.as_ref().is_some_and(|o| o.borrow_mut::<MpDomainView>().is_some()) {
            self.overlay = None;
        }
    }

    /// Advance the slide; true while it still moves.
    fn animate(&mut self, now: f64) -> bool {
        let Some((start, from, opening)) = self.anim_started else { return false };
        let dur = if opening { SLIDE_IN } else { SLIDE_OUT };
        let to = if opening { 1.0 } else { 0.0 };
        let p = ((now - start) / dur).clamp(0.0, 1.0);
        self.t = from + (to - from) * p;
        if p >= 1.0 {
            self.t = to;
            self.anim_started = None;
            self.apply_strip();
            return false;
        }
        self.apply_strip();
        true
    }

    pub fn with_run_view<R>(&mut self, cx: &mut Cx, f: impl FnOnce(&mut Cx, &mut MpRunView) -> R) -> Option<R> {
        let run = self.view.widget(cx, ids!(run));
        let mut view = run.borrow_mut::<MpRunView>()?;
        Some(f(cx, &mut view))
    }

    /// Claim the keyboard for the body. It is not drawn at all while the
    /// pane is hidden, so at open time its area is stale (or, the first
    /// time, empty): the claim is then kept pending and made right after
    /// the pane's next draw, when the area is live again.
    pub fn focus_keyboard(&mut self, cx: &mut Cx) {
        let ok = match &self.overlay {
            Some(overlay) if overlay.borrow_mut::<MpDomainView>().is_some() => {
                overlay.borrow_mut::<MpDomainView>().unwrap().focus_keyboard(cx)
            }
            Some(overlay) => {
                let input = overlay.text_input(cx, ids!(panel.input));
                let live = input.area().is_valid(cx);
                if live {
                    input.set_key_focus(cx);
                }
                live
            }
            None => self.with_run_view(cx, |cx, v| v.focus_keyboard(cx)).unwrap_or(false),
        };
        self.focus_pending = !ok;
        if !ok {
            self.redraw(cx);
        }
    }
}

impl Widget for ShellAiPane {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            let now = cx.seconds_since_app_start();
            if self.animate(now) {
                self.next_frame = cx.new_next_frame();
            }
            // The strip changed: the desk beside us must lay out again,
            // not just this widget.
            cx.redraw_all();
        }
        // The body hears every event whether or not the pane shows: the
        // child's run view sends its Ticks at the WM's next frame (and polls
        // its transport on a timer), the module's engine pumps on every
        // event. A hidden run view that missed its frame event never ticked
        // its child again: no frame ever came, and its "starting…" card
        // kept the whole desktop redrawing. Pointer and keys reach the body
        // only while the pane is on screen, so a hidden pane's stale area
        // can never swallow a click meant for a tile.
        let pointer = matches!(event, Event::MouseMove(_) | Event::MouseDown(_) | Event::MouseUp(_) | Event::Scroll(_));
        let keys = matches!(event, Event::KeyDown(_) | Event::KeyUp(_) | Event::TextInput(_));
        if (pointer || keys) && !self.showing() {
            return;
        }
        match self.overlay.clone() {
            Some(overlay) => overlay.handle_event(cx, event, scope),
            None => self.view.handle_event(cx, event, scope),
        }
        // A frame arrives with an event of its own, never with input.
        if !(pointer || keys) {
            self.sync_body(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // The strip is this widget's walk: `walk` arrives with its width,
        // the desk row lays the desk out after it. The card is drawn
        // right-aligned in the strip — off the row's left edge while
        // hidden, where a child process still keeps a real rect (so it is
        // configured, gets a swapchain and presents its first frame) and a
        // module root keeps a live picture: the first slide-in shows the
        // assistant already running, not a "starting…" wash.
        cx.begin_turtle(walk, Layout::default());
        let r = self.card_rect();
        self.draw_card.draw_abs(cx, r);
        self.draw_edge.draw_abs(
            cx,
            Rect { pos: dvec2(r.pos.x + r.size.x - EDGE, r.pos.y), size: dvec2(EDGE, r.size.y) },
        );
        let inner = Rect { pos: r.pos, size: dvec2(r.size.x - EDGE, r.size.y) };
        match self.overlay.clone() {
            Some(overlay) => overlay.draw_walk_all(cx, scope, Walk::abs_rect(inner)),
            None => {
                while self.view.draw_walk(cx, scope, Walk::abs_rect(inner)).is_step() {}
            }
        }
        cx.end_turtle();
        if self.focus_pending && self.open {
            // The body just drew: its area is live, the claim can land.
            self.focus_keyboard(cx);
        }
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_pane_hides_its_client_and_an_open_waits_for_its_fresh_frame() {
        let mut s = BodyShown::default();
        // Not before the app's own frame: the first open needs its picture.
        assert_eq!(s.step(false, false, 3), BodyStep::Rest);
        assert_eq!(s.step(false, true, 4), BodyStep::Hide);
        s.hidden_at(4);
        // Closed at rest nothing more is said, however often it is asked.
        for _ in 0..100 {
            assert_eq!(s.step(false, true, 4), BodyStep::Rest);
        }
        // A frame while hidden: a client that does not know it (a new image
        // of its process) is told again.
        assert_eq!(s.step(false, true, 5), BodyStep::Hide);
        s.hidden_at(5);
        // An open tells it it shows, and the slide waits for its next frame.
        assert!(s.opening());
        s.shown_at(5);
        assert_eq!(s.step(true, true, 5), BodyStep::Rest, "the old picture never slides in");
        assert_eq!(s.step(true, true, 6), BodyStep::Slide);
        assert_eq!(s.step(true, true, 7), BodyStep::Rest, "open, it draws freely");
        // Closed again, it is hidden once the slide-out has ended.
        assert_eq!(s.step(true, true, 9), BodyStep::Rest, "sliding out");
        assert_eq!(s.step(false, true, 9), BodyStep::Hide);
        s.hidden_at(9);
        // A close while an open waits ends the wait; the client, told it
        // shows, is hidden again at rest.
        assert!(s.opening());
        s.shown_at(9);
        s.cancel();
        assert_eq!(s.step(false, true, 9), BodyStep::Hide);
        // An open of a client never told it is hidden slides at once.
        let mut fresh = BodyShown::default();
        assert!(!fresh.opening());
        assert_eq!(fresh.step(true, false, 0), BodyStep::Rest);
    }

    #[test]
    fn the_card_width_follows_the_row_within_its_bounds() {
        assert_eq!(PaneStrip::card_width(1400.0), 440.0);
        assert_eq!(PaneStrip::card_width(800.0), 320.0, "40 % of a narrow row");
        assert_eq!(PaneStrip::card_width(300.0), 240.0, "never under the minimum");
    }

    #[test]
    fn the_strip_grows_from_nothing_to_the_card_plus_a_gap() {
        assert_eq!(PaneStrip::strip(0.0, 440.0, 10.0), 0.0);
        assert_eq!(PaneStrip::strip(1.0, 440.0, 10.0), 450.0);
        let mid = PaneStrip::strip(0.5, 440.0, 10.0);
        assert!(mid > 0.0 && mid < 450.0, "{mid}");
    }

    #[test]
    fn the_card_rests_one_gap_in_and_hides_off_the_left_edge() {
        let row = Rect { pos: dvec2(0.0, 26.0), size: dvec2(1400.0, 874.0) };
        let w = PaneStrip::card_width(row.size.x);
        let rest = PaneStrip::card_rect(row, PaneStrip::strip(1.0, w, 10.0), w, 10.0);
        assert_eq!(rest.pos, dvec2(10.0, 36.0));
        assert_eq!(rest.size, dvec2(440.0, 854.0));
        // The desk starts where the strip ends: a tile one gap past the
        // card's right edge, never under it.
        let strip = PaneStrip::strip(1.0, w, 10.0);
        assert_eq!(row.pos.x + strip + 10.0, rest.pos.x + rest.size.x + 10.0);
        let hidden = PaneStrip::card_rect(row, 0.0, w, 10.0);
        assert_eq!(hidden.pos.x + hidden.size.x, row.pos.x, "fully off the left edge");
    }
}
