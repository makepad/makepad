//! Tour — a run of steps, each pointing at something already on the screen.
//!
//! The thing a tour has to get right is not the card. It is the pointing:
//! the reader has to be sure, without being told twice, which control the
//! words are about. So everything else on the screen is dimmed, a hole is
//! cut around the one widget the step names, and the card sits beside the
//! hole rather than anywhere fixed. A card in a corner with an arrow drawn
//! to the target is the other school, and it fails the moment the target is
//! near that corner.
//!
//! # What it points at
//!
//! A step names its target by id path, not by a widget reference: the
//! target is looked up in the widget tree each time it is needed. That is
//! what lets a tour be written in the DSL beside the page it describes, and
//! it is also what makes the next rule possible.
//!
//! # A target that is not there is stepped over
//!
//! A tour outlives the page it was written for. A control gets hidden
//! behind a setting, a row is empty today, a whole panel is off in this
//! build — and the step that named it would otherwise cut a hole around
//! nothing and say "press this" about a blank patch of screen. Instead the
//! step is skipped: Next goes to the next step whose target is BOTH in the
//! tree and drawn, and the count on the card counts the steps the reader
//! will actually see, not the ones the DSL listed.
//!
//! # Advancing scrolls first
//!
//! A target inside a scrolling panel may be nowhere near the viewport. The
//! step asks every scrolling ancestor to bring it in before the card is
//! placed, and the hole follows the target while that scroll settles, so
//! the reader watches the page come to the thing rather than being shown a
//! hole where the thing will be. A panel the tour is declared INSIDE is the
//! one exception: it is mid-dispatch the whole time the tour is running
//! code and cannot be asked anything. Declare a tour beside the panels it
//! points into — last in the body, where an overlay belongs — and that
//! case does not arise.
//!
//! # What it deliberately does not do
//!
//! It does not let the reader operate the thing it is pointing at: while a
//! tour is running it holds the sweep lock, and the only live controls are
//! its own. A tour whose steps could be answered out of order would have to
//! re-check every later step against a page the reader had changed, and the
//! honest version of that is a checklist, not a tour. It also does not
//! remember whether it has been shown; that belongs to whatever knows who
//! the reader is.
use crate::{
    button::ButtonWidgetRefExt,
    label::LabelWidgetRefExt,
    makepad_derive_widget::*,
    makepad_draw::*,
    overlay_place::{claim_escape, place, span_inboard, PlaceAlign, PlaceRequest, Placement, Side},
    view::*,
    widget::*,
    widget_async::ScriptAsyncResult,
    widget_tree::CxWidgetExt,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    /** One step: the id path of the thing it points at, and what it says
     * about it. */
    mod.widgets.TourStep = #(TourStep::script_api(vm))

    mod.widgets.TourBase = #(Tour::register_widget(vm))

    set_type_default() do #(DrawTourMask::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    /** The panel beside the hole: what this step is, where it comes in the
     * run, and the three ways on. */
    mod.widgets.TourCard = RoundedShadowView{
        // The tour hands the card a fixed width every draw, so `Fill` here
        // resolves against that width and not against the window.
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_2
        padding: Inset{left: 16. right: 16. top: 14. bottom: 12.}
        draw_bg +: {
            color: theme.color_surface_container_high
            border_color: theme.color_outline
            border_size: 1.0
            border_radius: theme.radius_l
            shadow_color: theme.color_elevation_4
            shadow_radius: uniform(theme.elevation_4_radius)
            shadow_offset: uniform(vec2(0., theme.elevation_4_offset_y))
        }

        /** the step's title */
        heading := Label{
            width: Fill
            draw_text +: {
                text_style: theme.font_title_s
                color: theme.color_on_surface
            }
        }
        /** the step's words; it wraps at the card's width */
        body := TextBox{
            width: Fill
            text: ""
        }
        footer := View{
            width: Fill
            height: Fit
            flow: Right
            align: Align{y: 0.5}
            spacing: theme.space_2
            /** "2 of 5", over the steps that will be shown */
            count := Label{
                width: Fit
                draw_text +: {color: theme.color_text_meta}
            }
            Filler{}
            /** leave without finishing */
            skip := ButtonFlat{text: "Skip"}
            /** the step before this one */
            back := ButtonFlat{text: "Back"}
            /** the step after this one, or the end of the run */
            next := Button{text: "Next"}
        }
    }

    /** A run of steps over the page: a dimmed screen with a hole around the
     * step's target, and a card beside the hole. */
    mod.widgets.Tour = set_type_default() do mod.widgets.TourBase{
        // No `width`/`height`: a tour claims NO slot in the layout that
        // holds it (`on_after_apply` pins its walk to empty) and paints on
        // its own overlay lists over the whole pass instead. Declaring
        // `Fill` would make it a deferred fill taking a share of its
        // parent's spare length whether or not it ever ran.
        flow: Overlay

        /** how wide the card is, in points 160..640 step 10 */
        card_width: 300.
        /** room between the hole and the card, in points 0..64 step 1 */
        gap: 12.
        /** how far the hole is grown past the target's rect, in points 0..40 step 1 */
        pad: 6.
        /** room kept between the card and the window's edges, in points 0..64 step 1 */
        edge: 12.
        /** the onward button's label on every step but the last */
        next_text: "Next"
        /** the onward button's label on the last step */
        done_text: "Done"

        draw_mask +: {
            // Rust owns these four: they are the target's rect, and they
            // change every time the step changes or the page moves.
            hole_x: 0.0
            hole_y: 0.0
            hole_w: 0.0
            hole_h: 0.0

            // The scrim is a colour and a strength: the colour alone is
            // opaque black, which would hide the page rather than dim it.
            color: uniform(theme.color_scrim)
            /** how much of the scrim colour is laid over the page 0..1 step 0.01 */
            dim: uniform(theme.state_scrim_opacity)
            /** the hole's corner rounding 0..32 step 0.5 */
            hole_radius: uniform(theme.corner_radius * 2.0)
            /** the ring drawn round the hole, in pixels 0..6 step 0.5 */
            ring_size: uniform(1.5)
            ring_color: uniform(theme.color_primary)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                // The dim is ONE shape — the quad less the hole — and not
                // four rectangles around the hole: four would meet along
                // seams that each antialias against nothing and read as
                // bright hairlines across the screen. The rect is a pixel
                // proud of the quad on every side so its own antialiased
                // edge falls outside what is painted.
                sdf.rect(-1.0, -1.0, self.rect_size.x + 2.0, self.rect_size.y + 2.0)
                sdf.box(self.hole_x, self.hole_y, self.hole_w, self.hole_h, self.hole_radius)
                sdf.subtract()
                sdf.fill(vec4(self.color.xyz, self.color.a * self.dim))

                if self.ring_size > 0.0 {
                    sdf.box(self.hole_x, self.hole_y, self.hole_w, self.hole_h, self.hole_radius)
                    sdf.stroke(self.ring_color, self.ring_size)
                }
                return sdf.result
            }
        }

        card := mod.widgets.TourCard{}
    }
}

/// The scrim, with the hole cut in it.
///
/// The hole is four numbers rather than a rect because the shader wants
/// them one at a time, and they are plain instances rather than uniforms
/// because Rust writes them on every draw.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTourMask {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hole_x: f32,
    #[live]
    hole_y: f32,
    #[live]
    hole_w: f32,
    #[live]
    hole_h: f32,
}

/// One step of a tour.
#[derive(Script, ScriptHook, Default)]
pub struct TourStep {
    #[source]
    source: ScriptObjectRef,
    /// Dotted id path of the widget this step points at, searched for from
    /// the tour outward — "send", or "footer.send" when a bare name would
    /// be ambiguous.
    #[live]
    pub target: String,
    /// The line at the top of the card.
    #[live]
    pub heading: String,
    /// What the step says. It wraps at the card's width.
    #[live]
    pub body: String,
}

/// Where the hole goes when the step has no target on screen: far enough
/// out that subtracting it changes nothing and the ring is never drawn.
const NO_HOLE: f64 = -10000.0;

/// Room the card's own overlay root keeps around it before the shift, so
/// its shadow never starts at a negative coordinate.
const ROOT_MARGIN: f64 = 48.0;

/// A widget's area, or nothing when the widget cannot be asked for one.
///
/// A widget that is mid-dispatch is mutably borrowed, and `area()` on a
/// borrowed widget panics rather than answering. Everything between a tour
/// and the root is mid-dispatch the whole time the tour is running code, so
/// the borrow has to be tested before the question is asked — and
/// `try_widget_uid` answers None for exactly the widgets that cannot be
/// asked anything else either.
fn askable_area(widget: &WidgetRef) -> Option<Area> {
    widget.try_widget_uid()?;
    Some(widget.area())
}

/// Turn a dotted id path into the ids a widget lookup takes.
fn id_path(path: &str) -> Vec<LiveId> {
    path.split('.')
        .filter(|part| !part.is_empty())
        .map(LiveId::from_str)
        .collect()
}

/// The target's rect, grown by `pad` on every side: the hole is a little
/// larger than the thing so the thing is not touching the dim.
fn grown(rect: Rect, pad: f64) -> Rect {
    Rect {
        pos: dvec2(rect.pos.x - pad, rect.pos.y - pad),
        size: dvec2(rect.size.x + pad * 2.0, rect.size.y + pad * 2.0),
    }
}

/// Which side of the hole the card takes.
///
/// Below first, then above, then right, then left: below is where a reader
/// looks for the caption of a thing, and the sides are what is left when a
/// target is tall enough to leave no room above or below it — a sidebar, a
/// full-height panel. The first side the card FITS on wins; when it fits
/// nowhere the roomiest side does, so a cramped window still puts the card
/// where the most of it can be seen.
fn card_side(hole: Rect, card: DVec2, bounds: Rect, gap: f64) -> Side {
    let room = |side: Side| -> f64 {
        match side {
            Side::Bottom => (bounds.pos.y + bounds.size.y) - (hole.pos.y + hole.size.y + gap),
            Side::Top => (hole.pos.y - gap) - bounds.pos.y,
            Side::Right => (bounds.pos.x + bounds.size.x) - (hole.pos.x + hole.size.x + gap),
            Side::Left => (hole.pos.x - gap) - bounds.pos.x,
        }
    };
    let need = |side: Side| -> f64 {
        if side.is_vertical() {
            card.y
        } else {
            card.x
        }
    };
    const ORDER: [Side; 4] = [Side::Bottom, Side::Top, Side::Right, Side::Left];
    for side in ORDER {
        if need(side) <= room(side) {
            return side;
        }
    }
    let mut best = ORDER[0];
    let mut slack = room(best) - need(best);
    for side in ORDER.into_iter().skip(1) {
        let other = room(side) - need(side);
        if other > slack {
            best = side;
            slack = other;
        }
    }
    best
}

/// Where the card goes beside the hole.
///
/// The side comes from `card_side`; the alignment along it and the pull
/// back inboard are the same ones every other anchored panel in the library
/// uses. The card keeps its own size afterwards and is then clamped onto
/// the bounds on both axes: a card that overlaps the hole is a nuisance, a
/// card with its buttons off the bottom of the window is a trap.
fn place_card(hole: Rect, card: DVec2, bounds: Rect, gap: f64) -> Rect {
    let side = card_side(hole, card, bounds, gap);
    let placed = place(&PlaceRequest {
        anchor: hole,
        size: card,
        bounds,
        gap,
        placement: Placement::new(side, PlaceAlign::Center),
        match_anchor_width: false,
    });
    Rect {
        pos: dvec2(
            span_inboard(placed.rect.pos.x, card.x, bounds.pos.x, bounds.size.x),
            span_inboard(placed.rect.pos.y, card.y, bounds.pos.y, bounds.size.y),
        ),
        size: card,
    }
}

/// The first step at or after `at`, walking forward or back, whose target
/// is on screen. `at` may be -1 or past the end, which is how a caller asks
/// "is there anything left this way".
fn seek(present: &[bool], at: isize, forward: bool) -> Option<usize> {
    let len = present.len() as isize;
    let mut i = at;
    while i >= 0 && i < len {
        if present[i as usize] {
            return Some(i as usize);
        }
        i += if forward { 1 } else { -1 };
    }
    None
}

/// Where step `at` stands among the steps that will be shown, and how many
/// those are. The card counts what the reader will see: a run of five with
/// two targets missing is three steps long, and saying "2 of 5" about it
/// would promise two steps that never arrive.
fn tally(present: &[bool], at: usize) -> (usize, usize) {
    let total = present.iter().filter(|p| **p).count();
    let upto = present.len().min(at + 1);
    let n = present[..upto].iter().filter(|p| **p).count();
    (n, total)
}

/// What a tour reports to its host.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum TourAction {
    /// The run began.
    Started,
    /// The step now showing, by its index in `steps`.
    Step(usize),
    /// The onward button was pressed on the last step: the run ended.
    Finished,
    /// Left early: Skip, Escape, or the back gesture.
    Skipped,
    #[default]
    None,
}

#[derive(Script, Widget)]
pub struct Tour {
    #[source]
    source: ScriptObjectRef,
    /// The card is a child of this view; the tour draws it itself, on the
    /// overlay, and the view is never walked in the parent's layout.
    #[deref]
    view: View,
    #[live]
    draw_mask: DrawTourMask,

    /// The steps, in the order they are shown.
    #[live]
    pub steps: Vec<TourStep>,

    #[live(300.0)]
    pub card_width: f64,
    #[live(12.0)]
    pub gap: f64,
    #[live(6.0)]
    pub pad: f64,
    #[live(12.0)]
    pub edge: f64,
    #[live]
    pub next_text: String,
    #[live]
    pub done_text: String,

    /// Two lists, not one: the card is measured at its overlay root and the
    /// whole list is then shifted into place, which is the proven way to
    /// place a panel whose size is only known once it is drawn. Shifting
    /// one list holding both would take the full-screen mask with it.
    #[rust]
    mask_list: Option<DrawList2d>,
    #[rust]
    card_list: Option<DrawList2d>,

    #[rust]
    running: bool,
    /// Which step is showing, as an index into `steps`.
    #[rust]
    at: usize,
    /// The hole of the last draw, so the follow frame can tell whether the
    /// target has moved without redrawing to find out.
    #[rust]
    hole: Rect,
    /// Whether the card has been written from the step showing.
    #[rust]
    dressed: bool,
    /// A frame asked for while the tour runs: the target can move for
    /// reasons that never reach this widget as an event.
    #[rust]
    follow: NextFrame,
    /// The area holding the sweep lock, kept so it is released with the
    /// same owner it was taken with.
    #[rust]
    locked: Area,
}

impl ScriptHook for Tour {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.mask_list = Some(DrawList2d::script_new(vm));
        self.card_list = Some(DrawList2d::script_new(vm));
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        // A tour occupies no space in the layout that holds it. Forced here
        // rather than only left out of the DSL, so an instance writing
        // `Tour{height: Fill}` cannot turn it into a deferred fill.
        self.view.walk = Walk::empty();
        // The steps may have just changed under a running tour.
        self.dressed = false;
        let cx = vm.cx_mut();
        if let Some(list) = &self.mask_list {
            list.redraw(cx);
        }
        if let Some(list) = &self.card_list {
            list.redraw(cx);
        }
    }
}

impl Tour {
    fn card(&self, cx: &Cx) -> WidgetRef {
        self.view.widget(cx, ids!(card))
    }

    fn repaint(&mut self, cx: &mut Cx) {
        self.view.redraw(cx);
        if let Some(list) = &self.mask_list {
            list.redraw(cx);
        }
        if let Some(list) = &self.card_list {
            list.redraw(cx);
        }
    }

    /// The widget a step points at, whatever its depth: the search starts
    /// at the tour and works outward, so a tour declared beside the page it
    /// describes finds the page's own ids without being told where it sits.
    fn target_of(&self, cx: &Cx, i: usize) -> Option<WidgetRef> {
        let step = self.steps.get(i)?;
        let path = id_path(&step.target);
        if path.is_empty() {
            return None;
        }
        // From the tour's own children outward. The `_from_borrowed` form
        // is the one to use here: the tour is mid-dispatch whenever it asks
        // this, so the tree cannot read the tour's children for itself and
        // is handed them.
        let found = cx.widget_tree().find_flood_from_borrowed(
            self.view.widget_uid(),
            &path,
            |visit| self.view.children(visit),
        );
        if found.is_empty() {
            None
        } else {
            Some(found)
        }
    }

    /// The step's target rect, or nothing when the target is not in the
    /// tree, has never been drawn, or is a widget this tour sits inside.
    /// "Drawn" and not merely "declared" is the test that matters: a widget
    /// behind a closed tab is in the tree and is not on the screen.
    fn target_rect(&self, cx: &Cx, i: usize) -> Option<Rect> {
        let rect = askable_area(&self.target_of(cx, i)?)?.rect(cx);
        if rect.size.x > 0.0 && rect.size.y > 0.0 {
            Some(rect)
        } else {
            None
        }
    }

    /// Which steps have something to point at, right now.
    fn present(&self, cx: &Cx) -> Vec<bool> {
        (0..self.steps.len())
            .map(|i| self.target_rect(cx, i).is_some())
            .collect()
    }

    fn hole_now(&self, cx: &Cx) -> Option<Rect> {
        self.target_rect(cx, self.at).map(|r| grown(r, self.pad))
    }

    /// Ask every scrolling ancestor of the step's target to bring it into
    /// view.
    ///
    /// The nav machinery does this for Tab, but only for widgets that
    /// register a nav stop, and a tour points at whatever it likes. The
    /// trigger is the one the scroll bars already answer; an ancestor that
    /// does not scroll has no scroll bars and ignores it.
    ///
    /// The walk stops at the first ancestor the tour itself sits inside,
    /// because those are mid-dispatch and cannot be asked anything. So a
    /// tour cannot scroll a panel it is declared inside — which is the
    /// same reason an overlay is declared beside the things it covers,
    /// last in the body, rather than within one of them.
    fn scroll_to(&self, cx: &mut Cx, i: usize) {
        let Some(target) = self.target_of(cx, i) else {
            return;
        };
        let (Some(from), Some(target_uid)) = (askable_area(&target), target.try_widget_uid())
        else {
            return;
        };
        if from.is_empty() {
            return;
        }
        let areas = {
            let tree = cx.widget_tree();
            let mut areas = Vec::new();
            let mut up = tree.parent_of(target_uid);
            while let Some(above) = up {
                let Some(area) = askable_area(&tree.widget(above)) else {
                    break;
                };
                if !area.is_empty() {
                    areas.push(area);
                }
                up = tree.parent_of(above);
            }
            areas
        };
        for area in areas {
            cx.send_trigger(
                area,
                Trigger {
                    id: live_id!(scroll_focus_nav),
                    from,
                },
            );
        }
    }

    /// Write the step showing into the card.
    fn dress(&mut self, cx: &mut Cx) {
        let Some(step) = self.steps.get(self.at) else {
            return;
        };
        let heading = step.heading.clone();
        let body = step.body.clone();
        let card = self.card(cx);
        card.label(cx, ids!(heading)).set_text(cx, &heading);
        card.label(cx, ids!(body)).set_text(cx, &body);

        let present = self.present(cx);
        let (n, total) = tally(&present, self.at);
        card.label(cx, ids!(count))
            .set_text(cx, &format!("{n} of {total}"));

        let last = seek(&present, self.at as isize + 1, true).is_none();
        let text = if last { &self.done_text } else { &self.next_text };
        card.button(cx, ids!(next)).set_text(cx, text);
        // Disabled rather than hidden: hiding it would change the width of
        // the button row between the first step and the second, and a row
        // that changes shape as you read it is read again each time.
        let first = seek(&present, self.at as isize - 1, false).is_none();
        card.button(cx, ids!(back)).set_disabled(cx, first);
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Which step is showing, by its index in `steps`.
    pub fn step(&self) -> usize {
        self.at
    }

    /// Begin at the first step with something to point at. A tour whose
    /// targets are all absent does not begin at all: there is nothing it
    /// could say that would be true.
    pub fn start(&mut self, cx: &mut Cx) {
        if self.running {
            return;
        }
        let present = self.present(cx);
        let Some(first) = seek(&present, 0, true) else {
            return;
        };
        self.running = true;
        self.at = first;
        self.dressed = false;
        self.follow = cx.new_next_frame();
        self.scroll_to(cx, first);
        self.repaint(cx);
        let uid = self.widget_uid();
        cx.widget_action(uid, TourAction::Started);
        cx.widget_action(uid, TourAction::Step(first));
    }

    /// Leave without finishing.
    pub fn stop(&mut self, cx: &mut Cx) {
        self.end(cx, TourAction::Skipped);
    }

    pub fn next(&mut self, cx: &mut Cx) {
        if !self.running {
            return;
        }
        let present = self.present(cx);
        match seek(&present, self.at as isize + 1, true) {
            Some(i) => self.go_to(cx, i),
            None => self.end(cx, TourAction::Finished),
        }
    }

    pub fn back(&mut self, cx: &mut Cx) {
        if !self.running {
            return;
        }
        let present = self.present(cx);
        if let Some(i) = seek(&present, self.at as isize - 1, false) {
            self.go_to(cx, i);
        }
    }

    fn go_to(&mut self, cx: &mut Cx, i: usize) {
        self.at = i;
        self.dressed = false;
        // The scroll is asked for BEFORE the step is shown: a card placed
        // against a target that is still off screen points at nothing for
        // as long as the scroll takes. The hole follows the target from
        // there, on the follow frame.
        self.scroll_to(cx, i);
        self.repaint(cx);
        let uid = self.widget_uid();
        cx.widget_action(uid, TourAction::Step(i));
    }

    fn end(&mut self, cx: &mut Cx, action: TourAction) {
        if !self.running {
            return;
        }
        self.running = false;
        self.dressed = false;
        if !self.locked.is_empty() {
            cx.sweep_unlock(self.locked);
            self.locked = Area::Empty;
        }
        self.repaint(cx);
        let uid = self.widget_uid();
        cx.widget_action(uid, action);
    }
}

impl Widget for Tour {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        _args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(start) {
            vm.with_cx_mut(|cx| self.start(cx));
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(stop) {
            vm.with_cx_mut(|cx| self.stop(cx));
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(next) {
            vm.with_cx_mut(|cx| self.next(cx));
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(back) {
            vm.with_cx_mut(|cx| self.back(cx));
            return ScriptAsyncResult::Return(NIL);
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.follow.is_event(event).is_some() && self.running {
            // The page can move under a running tour with no event this
            // widget would ever hear: a scroll settling, a window resized,
            // a panel relaid out. So it is looked at, once a frame, and the
            // redraw only happens when the hole has actually moved.
            let now = self.hole_now(cx).unwrap_or_default();
            if now != self.hole {
                self.repaint(cx);
            }
            self.follow = cx.new_next_frame();
        }
        if !self.running {
            return;
        }

        if let Event::KeyDown(ke) = event {
            // A running tour owns the screen, so it owns these keys: there
            // is nothing else on it to type into.
            match ke.key_code {
                KeyCode::Escape => {
                    if claim_escape(cx) {
                        self.stop(cx);
                        return;
                    }
                }
                KeyCode::ReturnKey | KeyCode::NumpadEnter | KeyCode::ArrowRight => {
                    self.next(cx);
                    return;
                }
                KeyCode::ArrowLeft => {
                    self.back(cx);
                    return;
                }
                _ => {}
            }
        }

        // The card's own widgets use plain `hits`, which this tour's lock
        // would turn away: lift it around their dispatch and take it again
        // after.
        let card = self.card(cx);
        let held = !self.locked.is_empty();
        if held {
            cx.sweep_unlock(self.locked);
        }
        card.handle_event(cx, event, scope);
        if held {
            cx.sweep_lock(self.locked);
        }

        if let Event::Actions(actions) = event {
            if card.button(cx, ids!(next)).clicked(actions) {
                self.next(cx);
            } else if card.button(cx, ids!(back)).clicked(actions) {
                self.back(cx);
            } else if card.button(cx, ids!(skip)).clicked(actions) {
                self.stop(cx);
            }
        }

        if event.back_pressed() {
            self.stop(cx);
        }
    }

    /// The incoming walk is ignored: a tour is not laid out by its parent
    /// at all. Both its lists are begun on every draw, running or not — a
    /// list that is not begun keeps showing whatever it showed last.
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        let (Some(mut mask_list), Some(mut card_list)) =
            (self.mask_list.take(), self.card_list.take())
        else {
            return DrawStep::done();
        };
        let pass = cx.current_pass_size();
        let bounds = Rect {
            pos: dvec2(self.edge, self.edge),
            size: pass - dvec2(self.edge * 2.0, self.edge * 2.0),
        };
        let hole = if self.running {
            let cx: &mut Cx = cx;
            self.hole_now(cx)
        } else {
            None
        };

        mask_list.begin_overlay_reuse(cx);
        cx.begin_root_turtle_for_pass(Layout::default());
        if self.running {
            let cut = hole.unwrap_or(Rect {
                pos: dvec2(NO_HOLE, NO_HOLE),
                size: dvec2(0.0, 0.0),
            });
            self.draw_mask.hole_x = cut.pos.x as f32;
            self.draw_mask.hole_y = cut.pos.y as f32;
            self.draw_mask.hole_w = cut.size.x as f32;
            self.draw_mask.hole_h = cut.size.y as f32;
            self.draw_mask.draw_abs(
                cx,
                Rect {
                    pos: dvec2(0.0, 0.0),
                    size: pass,
                },
            );
        }
        cx.end_pass_sized_turtle();
        mask_list.end(cx);
        self.mask_list = Some(mask_list);

        card_list.begin_overlay_reuse(cx);
        cx.begin_root_turtle(
            pass,
            Layout {
                padding: Inset {
                    left: ROOT_MARGIN,
                    top: ROOT_MARGIN,
                    right: ROOT_MARGIN,
                    bottom: ROOT_MARGIN,
                },
                ..Layout::flow_down()
            },
        );
        if self.running {
            if !self.dressed {
                self.dressed = true;
                let cx: &mut Cx = cx;
                self.dress(cx);
            }
            let card = {
                let cx: &mut Cx = cx;
                self.card(cx)
            };
            let _ = card.draw_walk(
                cx,
                scope,
                Walk {
                    width: Size::Fixed(self.card_width),
                    height: Size::fit(),
                    ..Walk::default()
                },
            );
            // Sizes are honest mid-draw; only positions lie. So the card is
            // drawn at the overlay root, measured there, and the whole list
            // shifted to where it belongs.
            let drawn = card.area().rect(cx);
            let spot = match hole {
                Some(hole) => place_card(hole, drawn.size, bounds, self.gap),
                // Nothing to point at: the card takes the middle of the
                // screen and says its piece anyway, rather than sitting
                // beside a hole that is not there.
                None => Rect {
                    pos: dvec2(
                        bounds.pos.x + (bounds.size.x - drawn.size.x) * 0.5,
                        bounds.pos.y + (bounds.size.y - drawn.size.y) * 0.5,
                    ),
                    size: drawn.size,
                },
            };
            cx.end_pass_sized_turtle_with_shift(Area::Empty, spot.pos - drawn.pos);
        } else {
            cx.end_pass_sized_turtle();
        }
        card_list.end(cx);
        self.card_list = Some(card_list);

        self.hole = hole.unwrap_or_default();
        if self.running {
            // Re-asserted every draw: the mask's area is a fresh handle
            // after each one, and the lock has to be held by the handle the
            // hit test will see.
            let area = self.draw_mask.area();
            let cx: &mut Cx = cx;
            cx.sweep_lock(area);
            self.locked = area;
        }
        DrawStep::done()
    }

    /// The heading of the step showing: what the tour is talking about.
    fn text(&self) -> String {
        self.steps
            .get(self.at)
            .map(|step| step.heading.clone())
            .unwrap_or_default()
    }
}

impl TourRef {
    pub fn start(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.start(cx);
        }
    }

    pub fn stop(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.stop(cx);
        }
    }

    pub fn next(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.next(cx);
        }
    }

    pub fn back(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.back(cx);
        }
    }

    pub fn is_running(&self) -> bool {
        self.borrow().map(|inner| inner.is_running()).unwrap_or(false)
    }

    pub fn step(&self) -> usize {
        self.borrow().map(|inner| inner.step()).unwrap_or(0)
    }

    /// Every action this tour raised this pass is scanned, not just the
    /// first: starting raises Started and Step together, and a host asking
    /// for one must not be answered with the other.
    fn raised(&self, actions: &Actions, wanted: TourAction) -> bool {
        let uid = self.widget_uid();
        actions.iter().any(|action| {
            action
                .as_widget_action()
                .map(|wa| wa.widget_uid == uid && wa.cast::<TourAction>() == wanted)
                .unwrap_or(false)
        })
    }

    pub fn started(&self, actions: &Actions) -> bool {
        self.raised(actions, TourAction::Started)
    }

    /// The step now showing, when it changed this pass.
    pub fn stepped(&self, actions: &Actions) -> Option<usize> {
        let uid = self.widget_uid();
        actions.iter().find_map(|action| {
            let wa = action.as_widget_action()?;
            if wa.widget_uid != uid {
                return None;
            }
            match wa.cast::<TourAction>() {
                TourAction::Step(i) => Some(i),
                _ => None,
            }
        })
    }

    /// The run reached the end.
    pub fn finished(&self, actions: &Actions) -> bool {
        self.raised(actions, TourAction::Finished)
    }

    /// The run was left early.
    pub fn skipped(&self, actions: &Actions) -> bool {
        self.raised(actions, TourAction::Skipped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An 800x600 window with the tour's default edge inset, and the
    /// default card.
    fn bounds() -> Rect {
        Rect {
            pos: dvec2(12.0, 12.0),
            size: dvec2(776.0, 576.0),
        }
    }

    fn card() -> DVec2 {
        dvec2(300.0, 160.0)
    }

    fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect {
            pos: dvec2(x, y),
            size: dvec2(w, h),
        }
    }

    const GAP: f64 = 12.0;

    // -- which side the card takes ---------------------------------------

    #[test]
    fn a_target_near_the_top_takes_the_card_below_it() {
        let hole = r(300.0, 20.0, 200.0, 40.0);
        assert_eq!(card_side(hole, card(), bounds(), GAP), Side::Bottom);
    }

    #[test]
    fn a_target_near_the_bottom_takes_the_card_above_it() {
        // Room below is 6 points; above there is nearly the whole window.
        let hole = r(300.0, 520.0, 200.0, 50.0);
        assert_eq!(card_side(hole, card(), bounds(), GAP), Side::Top);
    }

    #[test]
    fn a_tall_target_on_the_left_takes_the_card_to_its_right() {
        // A sidebar: no room above it or below it, so the card goes beside.
        let hole = r(10.0, 20.0, 120.0, 550.0);
        assert_eq!(card_side(hole, card(), bounds(), GAP), Side::Right);
    }

    #[test]
    fn a_tall_target_on_the_right_takes_the_card_to_its_left() {
        let hole = r(660.0, 20.0, 120.0, 550.0);
        assert_eq!(card_side(hole, card(), bounds(), GAP), Side::Left);
    }

    #[test]
    fn a_target_in_the_middle_takes_the_card_below_it() {
        let hole = r(350.0, 280.0, 100.0, 40.0);
        assert_eq!(card_side(hole, card(), bounds(), GAP), Side::Bottom);
        // And lands centred on it, one gap under its lower edge.
        assert_eq!(place_card(hole, card(), bounds(), GAP), r(250.0, 332.0, 300.0, 160.0));
    }

    #[test]
    fn the_card_is_wholly_on_screen_wherever_the_target_is() {
        let b = bounds();
        for x in [-40.0, 0.0, 200.0, 400.0, 700.0, 820.0] {
            for y in [-40.0, 0.0, 150.0, 300.0, 560.0, 640.0] {
                let spot = place_card(r(x, y, 120.0, 40.0), card(), b, GAP);
                assert!(spot.pos.x >= b.pos.x, "left edge at {x},{y}: {spot:?}");
                assert!(spot.pos.y >= b.pos.y, "top edge at {x},{y}: {spot:?}");
                assert!(
                    spot.pos.x + spot.size.x <= b.pos.x + b.size.x,
                    "right edge at {x},{y}: {spot:?}"
                );
                assert!(
                    spot.pos.y + spot.size.y <= b.pos.y + b.size.y,
                    "bottom edge at {x},{y}: {spot:?}"
                );
            }
        }
    }

    #[test]
    fn a_window_too_small_for_the_card_still_answers() {
        // Nothing fits on any side; the card takes the roomiest and is then
        // pulled inboard, where the low edges win.
        let small = r(8.0, 8.0, 100.0, 80.0);
        let spot = place_card(r(20.0, 20.0, 40.0, 20.0), card(), small, GAP);
        assert_eq!(spot.pos, small.pos);
    }

    // -- which step comes next --------------------------------------------

    #[test]
    fn a_step_whose_target_is_gone_is_stepped_over() {
        let present = [true, false, true];
        assert_eq!(seek(&present, 1, true), Some(2), "forward past the gap");
        assert_eq!(seek(&present, 1, false), Some(0), "and back past it");
    }

    #[test]
    fn several_missing_targets_in_a_row_are_stepped_over_together() {
        let present = [true, false, false, false, true];
        assert_eq!(seek(&present, 1, true), Some(4));
        assert_eq!(seek(&present, 3, false), Some(0));
    }

    #[test]
    fn seeking_past_either_end_finds_nothing() {
        let present = [true, true];
        assert_eq!(seek(&present, 2, true), None);
        assert_eq!(seek(&present, -1, false), None);
    }

    #[test]
    fn a_tour_with_nothing_on_screen_has_no_first_step() {
        assert_eq!(seek(&[false, false], 0, true), None);
        assert_eq!(seek(&[], 0, true), None);
    }

    #[test]
    fn the_count_counts_the_steps_that_will_be_shown() {
        let present = [true, false, true, true];
        assert_eq!(tally(&present, 0), (1, 3));
        assert_eq!(tally(&present, 2), (2, 3));
        assert_eq!(tally(&present, 3), (3, 3));
    }

    // -- the odds and ends -------------------------------------------------

    #[test]
    fn an_id_path_splits_on_dots_and_ignores_the_empty_parts() {
        assert_eq!(id_path("send"), vec![LiveId::from_str("send")]);
        assert_eq!(
            id_path("footer.send"),
            vec![LiveId::from_str("footer"), LiveId::from_str("send")]
        );
        assert!(id_path("").is_empty());
        assert!(id_path("...").is_empty());
    }

    #[test]
    fn the_hole_is_the_target_grown_on_every_side() {
        assert_eq!(grown(r(100.0, 50.0, 40.0, 20.0), 6.0), r(94.0, 44.0, 52.0, 32.0));
        assert_eq!(grown(r(100.0, 50.0, 40.0, 20.0), 0.0), r(100.0, 50.0, 40.0, 20.0));
    }
}
