//! Toolbar — a row of controls with two fixed ends and a middle that takes
//! what is left.
//!
//! Every bar in an application is the same three columns: something at the
//! leading edge, something at the trailing edge, and one thing between them
//! that gets whatever room the two ends do not want. Written by hand that
//! middle is where it goes wrong. A `Fill` child in a row takes the WHOLE
//! row, so the trailing actions are laid out into nothing and never painted
//! — silently, because a widget with no width is still a widget. The middle
//! here is a deferred fill instead: reserved before the trailing slot is
//! drawn and sized after it, which is the only arrangement where both ends
//! are certain to get their room.
//!
//! # The bars are one bar
//!
//! `AppBar` is this row at the top of a window — a title, a navigation
//! control before it, actions after it, a rule along the bottom edge.
//! `BottomAppBar` is the same row at the bottom: the rule moves to the top
//! edge, and one further slot past the actions carries the action the screen
//! exists for. Neither is a separate widget. The day two of them disagree
//! about how tall a bar is, or how far its ink sits from the edge, is the
//! day one application looks like two.
//!
//! # Flat, and raised
//!
//! A raised bar changes its own SURFACE and casts no shadow. A shadow would
//! have to be drawn over the content below, which the bar neither owns nor
//! can redraw; the surface it owns entirely. [`toolbar_raised`] decides when
//! it changes, and it has two thresholds rather than one — a bar that lifts
//! and settles at the same number flickers while the content comes to rest
//! on the boundary.
//!
//! Nothing here watches the scroll. A bar has no way of knowing which view
//! is underneath it, so the host reads its own offset and says.
//!
//! # What it does not do
//!
//! It does not collapse, fold, or scroll what is in it. A row too narrow for
//! its controls overruns, because a bar that quietly drops controls drops
//! them where nobody will look for them, and the overflow question already
//! belongs to `ButtonGroup`. It is not the application menu bar — that is
//! `MenuBar` — and it does not own the window's caption buttons.
//!
//! [`PageHeader`] is the same idea as a block rather than a row, for the top
//! of a page rather than the top of a window: a trail above, a title with an
//! optional subtitle and a description, and the page's actions beside them.
//! It shares the middle's arithmetic and nothing else.

use crate::{
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    widget_tree::CxWidgetExt,
};

/// Which edge of a bar carries the rule that separates it from the content.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum ToolbarEdge {
    /// Neither. A bar floating on a surface of its own needs no rule.
    #[pick]
    #[default]
    None,
    /// The bar is under the content, so the rule is above it.
    Top,
    /// The bar is over the content, so the rule is below it.
    Bottom,
}

impl ToolbarEdge {
    /// The thickness weights for the top and the bottom rule.
    ///
    /// The enum is resolved here rather than branched on in the pixel
    /// shader: two floats multiplied by a thickness cannot behave one way on
    /// one graphics backend and another way on the next, and a branch on an
    /// enum is one more thing that can.
    pub fn weights(self) -> (f32, f32) {
        match self {
            ToolbarEdge::None => (0.0, 0.0),
            ToolbarEdge::Top => (1.0, 0.0),
            ToolbarEdge::Bottom => (0.0, 1.0),
        }
    }
}

/// The gaps before and after the filling middle.
///
/// A gap only exists between two things that are there. Turtle spacing sits
/// between every pair of walks, including an empty leading slot and the
/// middle, and that indents the title of every bar with no navigation
/// control by a gap nobody asked for — so the bar lays its columns out with
/// no spacing at all and puts these two in by hand.
pub fn middle_gaps(has_leading: bool, has_trailing: bool, gap: f64) -> (f64, f64) {
    let gap = gap.max(0.0);
    let before = if has_leading { gap } else { 0.0 };
    let after = if has_trailing { gap } else { 0.0 };
    (before, after)
}

/// Whether the bar stands raised, from how far the content under it has
/// scrolled and whether it is raised already.
///
/// Two thresholds, not one. Content does not come to rest on a number: a
/// spring, a rubber band, or the last few pixels of a trackpad flick leave
/// it drifting a fraction either side, and a bar that lifts and settles at
/// the same number changes colour on every one of those fractions.
/// `settle_at` below `lift_at` gives it somewhere to stay.
///
/// A `lift_at` of zero means the bar lifts as soon as the content has moved
/// at all. It still does not lift at rest, which is the one state a bar has
/// to get right.
pub fn toolbar_raised(scroll: f64, raised: bool, lift_at: f64, settle_at: f64) -> bool {
    let lift = lift_at.max(0.0);
    // A settle point above the lift point would raise and lower the bar on
    // the same movement, so it is held below it whatever a caller wrote.
    let settle = settle_at.clamp(0.0, lift);
    if raised {
        // At the settle point the bar is still lifted — it settles as the
        // content passes it, not on reaching it. But at the very top it is
        // always flat, whatever the thresholds were set to: a bar lifted
        // over content that has not moved has nothing to lift away from.
        scroll > 0.0 && scroll >= settle
    } else {
        scroll > 0.0 && scroll >= lift
    }
}

/// Whether a slot has something in it that wants drawing.
fn shown(slot: &WidgetRef) -> bool {
    !slot.is_empty() && slot.visible()
}

/// Draw a slot at the walk it asks for, under its own name in the widget
/// tree, and answer whether anything was drawn.
///
/// The slots are put in the tree here rather than by a container, because
/// nothing else puts them there: without it a host cannot reach
/// `ids!(header.actions)` at all.
fn draw_slot(
    cx: &mut Cx2d,
    scope: &mut Scope,
    uid: WidgetUid,
    name: LiveId,
    slot: &WidgetRef,
) -> bool {
    if !shown(slot) {
        return false;
    }
    cx.widget_tree_insert_child(uid, name, slot.clone());
    let walk = slot.walk(cx.cx.cx);
    let _ = slot.draw_walk(cx, scope, walk);
    true
}

/// One run of text, wrapped to the column it is in and set from the leading
/// edge. The layouter only wraps inside a wrapping row flow, the way `Label`
/// arranges it.
fn draw_wrapped(cx: &mut Cx2d, draw: &mut DrawText, text: &str) {
    cx.begin_turtle(
        Walk::fill_fit(),
        Layout {
            flow: Flow::right_wrap(),
            ..Layout::default()
        },
    );
    draw.draw_walk(cx, Walk::fill_fit(), Align::default(), text);
    cx.end_turtle();
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ToolbarEdge = #(ToolbarEdge::script_api(vm))

    set_type_default() do #(DrawToolbar::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.ToolbarBase = #(Toolbar::register_widget(vm))
    mod.widgets.PageHeaderBase = #(PageHeader::register_widget(vm))

    /** A row of controls: something at each end, and a middle that takes
     * what the two ends leave. */
    mod.widgets.Toolbar = set_type_default() do mod.widgets.ToolbarBase{
        width: Fill
        // Fit, and the bar takes `bar_height`; a number written on the walk
        // wins over it.
        height: Fit
        // Overwritten every draw: the three columns are the bar, so they are
        // laid out left to right with no turtle spacing, and the gaps come
        // from `gap`.
        flow: Right
        /** room at the leading and trailing edges */
        padding: Inset{top: 0., bottom: 0., left: theme.space_3, right: theme.space_3}
        margin: 0.

        /** the bar's height when the walk does not fix one 24..96 step 1 */
        bar_height: 48.
        /** the gap between the ends and the middle 0..48 step 1 */
        gap: theme.space_2
        /** which edge carries the rule: None, Top or Bottom */
        edge: mod.widgets.ToolbarEdge.None
        /** the line in the middle, drawn when the centre slot is empty */
        title: ""
        /** where the middle's content sits across it: 0 leading, 0.5 centred, 1 trailing 0..1 step 0.5 */
        middle_align: 0.
        /** how far the floating action stands proud of the bar's middle 0..24 step 1 */
        float_lift: 0.
        /** how far content must scroll before a flat bar lifts 0..80 step 1 */
        lift_at: 8.
        /** how far it must come back before a raised bar settles 0..80 step 1 */
        settle_at: 2.
        /** drawn at all */
        visible: true

        // Bare slots, not named instances: a slot takes a VALUE, so a caller
        // writes `leading: Button{...}` and not `leading := Button{}`, which
        // would make a child and leave the slot empty. They start invisible,
        // so an end nobody filled costs no gap.
        leading: View{width: Fit height: Fit visible: false}
        center: View{width: Fit height: Fit visible: false}
        trailing: View{width: Fit height: Fit visible: false}
        floating: View{width: Fit height: Fit visible: false}

        /** The bar's surface: one fill, and a hairline along whichever edge
         * `edge` names. The two edge weights are written from Rust every
         * draw, so one place knows which edge a bar carries. */
        draw_bg +: {
            /** flat-to-raised mix 0..1 step 0.01 */
            raised: instance(0.0)

            // Plain, because each has a field on the draw struct behind it.
            edge_top: 0.0
            edge_bottom: 0.0

            /** the bar with nothing scrolled under it */
            color: uniform(theme.color_surface_container_low)
            /** the bar with content scrolled under it */
            color_raised: uniform(theme.color_surface_container_high)
            /** the rule along the bar's edge at rest */
            edge_color: uniform(theme.color_outline_variant)
            /** the rule once the bar is raised */
            edge_color_raised: uniform(theme.color_outline)
            /** the rule's thickness in points 0..4 step 0.5 */
            edge_size: uniform(theme.size_divider)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                sdf.fill(mix(self.color, self.color_raised, self.raised))

                let rule = mix(self.edge_color, self.edge_color_raised, self.raised)
                // A rect of no height still has an edge for the antialiasing
                // to find, so an edge the bar does not carry is skipped
                // rather than drawn at nothing.
                let top = self.edge_size * self.edge_top
                if top > 0.0 {
                    sdf.rect(0.0, 0.0, self.rect_size.x, top)
                    sdf.fill(rule)
                }
                let bottom = self.edge_size * self.edge_bottom
                if bottom > 0.0 {
                    sdf.rect(0.0, self.rect_size.y - bottom, self.rect_size.x, bottom)
                    sdf.fill(rule)
                }
                return sdf.result
            }
        }

        /** The title's ink. */
        draw_title +: {
            color: theme.color_text
            text_style: theme.font_bold{
                /** title type size 6..32 step 0.5 */
                font_size: theme.type_title_s_size
                line_spacing: 1.0
            }
        }

        animator: Animator{
            /** flat at rest, raised once content has scrolled under it */
            raised: {
                default: @off
                /** back at the top: the bar's own quiet surface */
                off: AnimatorState{
                    from: {all: Forward{duration: theme.motion_short_3}}
                    ease: theme.motion_ease_standard
                    apply: {draw_bg: {raised: 0.0}}
                }
                /** content underneath: a firmer surface, and a firmer rule */
                on: AnimatorState{
                    from: {all: Forward{duration: theme.motion_short_2}}
                    ease: theme.motion_ease_standard
                    apply: {draw_bg: {raised: 1.0}}
                }
            }
        }
    }

    /** The bar across the top of a window: a title, a navigation control
     * before it, actions after it, and a rule along the bottom edge. */
    mod.widgets.AppBar = mod.widgets.Toolbar{
        bar_height: 56.
        edge: mod.widgets.ToolbarEdge.Bottom
    }

    /** The bar across the bottom of a window: the same row with its rule on
     * the top edge, and a slot past the actions for the one action the
     * screen is for. */
    mod.widgets.BottomAppBar = mod.widgets.Toolbar{
        bar_height: 64.
        edge: mod.widgets.ToolbarEdge.Top
    }

    /** The block at the top of a page: a trail above it, a title with an
     * optional subtitle and description, and the page's actions beside
     * them. */
    mod.widgets.PageHeader = set_type_default() do mod.widgets.PageHeaderBase{
        width: Fill
        height: Fit
        // Overwritten every draw: the trail stands above the title row, and
        // a page header laid out any other way is a different widget.
        flow: Down
        padding: Inset{top: theme.space_2, bottom: theme.space_3, left: 0., right: 0.}
        /** gap between the trail and the title row 0..32 step 1 */
        spacing: theme.space_2
        /** gap between the words and the actions 0..48 step 1 */
        gap: theme.space_3
        /** gap between the title, the subtitle and the description 0..24 step 1 */
        text_spacing: theme.space_1
        /** the page's name */
        title: ""
        /** the quiet line under the title: whose it is, when it changed */
        subtitle: ""
        /** the paragraph under both: what the page is for */
        description: ""
        /** drawn at all */
        visible: true

        breadcrumb: View{width: Fit height: Fit visible: false}
        actions: View{width: Fit height: Fit visible: false}

        /** The header's ground. Transparent, so it takes the colour of
         * whatever page it is dropped on. */
        draw_bg +: {
            color: #00000000
        }

        draw_title +: {
            color: theme.color_text
            text_style: theme.font_bold{
                /** title type size 6..40 step 0.5 */
                font_size: theme.type_title_l_size
                line_spacing: 1.2
            }
        }
        draw_subtitle +: {
            color: theme.color_on_surface_variant
            text_style: theme.font_regular{
                /** subtitle type size 6..32 step 0.5 */
                font_size: theme.type_body_m_size
                line_spacing: 1.3
            }
        }
        draw_description +: {
            color: theme.color_on_surface_variant
            text_style: theme.font_regular{
                /** description type size 6..32 step 0.5 */
                font_size: theme.type_body_s_size
                line_spacing: 1.35
            }
        }
    }
}

/// The bar's surface: the fill, and the hairline along one edge.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawToolbar {
    #[deref]
    draw_super: DrawQuad,
    /// Which edge carries the rule, as the weights the shader multiplies its
    /// thickness by. Written from Rust every draw.
    #[live]
    edge_top: f32,
    #[live]
    edge_bottom: f32,
}

/// A row with two fixed ends and a filling middle.
#[derive(Script, ScriptHook, Widget, Animator)]
pub struct Toolbar {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[apply_default]
    animator: Animator,

    #[redraw]
    #[live]
    pub draw_bg: DrawToolbar,
    #[live]
    draw_title: DrawText,

    /// Before everything: a back control, a menu button, a mark.
    #[find]
    #[live]
    pub leading: WidgetRef,
    /// The middle. Whatever is put here takes the room the two ends leave,
    /// and replaces the title rather than sharing the middle with it.
    #[find]
    #[live]
    pub center: WidgetRef,
    /// After it: the bar's actions.
    #[find]
    #[live]
    pub trailing: WidgetRef,
    /// Past the actions: the one action the screen is for.
    #[find]
    #[live]
    pub floating: WidgetRef,

    /// The line in the middle, drawn when the centre slot is empty.
    #[live]
    pub title: String,
    /// Where the middle's content sits across it: 0 leading, 0.5 centred.
    #[live]
    pub middle_align: f64,

    /// The bar's height when the walk does not fix one.
    #[live(48.0)]
    pub bar_height: f64,
    /// The gap between the ends and the middle.
    #[live(8.0)]
    pub gap: f64,
    #[live]
    pub edge: ToolbarEdge,
    /// How far the floating action stands proud of the bar's middle.
    #[live]
    pub float_lift: f64,
    /// The two thresholds [`toolbar_raised`] switches on.
    #[live(8.0)]
    pub lift_at: f64,
    #[live(2.0)]
    pub settle_at: f64,

    #[visible]
    #[live(true)]
    visible: bool,
}

impl Toolbar {
    pub fn set_title(&mut self, cx: &mut Cx, title: &str) {
        if self.title != title {
            self.title = title.to_string();
            self.draw_bg.redraw(cx);
        }
    }

    /// Whether the bar is drawn raised at this moment.
    pub fn raised(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(raised.on))
    }

    pub fn set_raised(&mut self, cx: &mut Cx, raised: bool) {
        self.animator_toggle(cx, raised, Animate::Yes, ids!(raised.on), ids!(raised.off));
    }

    /// Raise or settle the bar for a scroll offset the host has read from
    /// whatever view is under it, and answer what the bar now is.
    ///
    /// This is the whole of the scroll story: the bar cannot find the view
    /// beneath it, so the host that owns both says how far it has moved.
    pub fn set_raised_for_scroll(&mut self, cx: &mut Cx, scroll: f64) -> bool {
        let raised = toolbar_raised(scroll, self.raised(cx), self.lift_at, self.settle_at);
        self.set_raised(cx, raised);
        raised
    }

    /// The slot under one of the four names.
    fn slot(&self, name: LiveId) -> WidgetRef {
        if name == live_id!(leading) {
            return self.leading.clone();
        }
        if name == live_id!(center) {
            return self.center.clone();
        }
        if name == live_id!(trailing) {
            return self.trailing.clone();
        }
        self.floating.clone()
    }

    /// Draw one end of the bar inside a column of its own, and answer
    /// whether anything was drawn.
    ///
    /// The column is the bar's full height with the slot centred in it, so
    /// the ROW's own align pass has nothing left to move: every column ends
    /// up where it was laid, which is what lets the middle be sized against
    /// them. A slot sizes itself, so a `Fill` widget dropped into one has
    /// nothing to fill.
    fn draw_end(&mut self, cx: &mut Cx2d, scope: &mut Scope, name: LiveId) -> bool {
        let slot = self.slot(name);
        if !shown(&slot) {
            return false;
        }
        cx.widget_tree_insert_child(self.uid, name, slot.clone());
        let mut frame = Walk::new(Size::fit(), Size::fill());
        // The floating action and the actions before it are two separate
        // things, so there is a gap between them; nothing else in the row
        // pays for it.
        if name == live_id!(floating) && shown(&self.trailing) {
            frame.margin.left = self.gap.max(0.0);
        }
        cx.begin_turtle(
            frame,
            Layout {
                flow: Flow::right(),
                align: Align { x: 0.5, y: 0.5 },
                ..Layout::default()
            },
        );
        let mut slot_walk = slot.walk(cx.cx.cx);
        // A centred column places a child by its OUTER box, margins and all,
        // so room left under the floating action lifts it by half of that
        // room. It stays inside the bar; a host that wants it standing over
        // the edge gives the bar the height to do it in.
        if name == live_id!(floating) && self.float_lift > 0.0 {
            slot_walk.margin.bottom += self.float_lift * 2.0;
        }
        let _ = slot.draw_walk(cx, scope, slot_walk);
        cx.end_turtle();
        true
    }

    /// The middle column: the centre slot, or the title, or neither.
    ///
    /// The walk is always consumed, even when there is nothing to put in it.
    /// It is the deferred fill the two ends were measured against, and a
    /// fill that is reserved and never taken leaves the row a hole.
    fn draw_middle(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) {
        let align = Align {
            x: self.middle_align.clamp(0.0, 1.0),
            y: 0.5,
        };
        cx.begin_turtle(
            walk,
            Layout {
                flow: Flow::right(),
                align,
                ..Layout::default()
            },
        );
        if shown(&self.center) {
            // The middle holds ONE thing. A centre slot and a title in the
            // same bar would have to share a width neither can be measured
            // against, and a title quietly overrunning the search box beside
            // it is worse than a title that stands down.
            let slot = self.center.clone();
            cx.widget_tree_insert_child(self.uid, live_id!(center), slot.clone());
            let slot_walk = slot.walk(cx.cx.cx);
            let _ = slot.draw_walk(cx, scope, slot_walk);
        } else if !self.title.is_empty() {
            let title = self.title.clone();
            // A bounded WIDTH, never Fit: the layouter cuts a walked run that
            // will not fit, and a Fit run would run out over the actions. The
            // height is Fit so the column's align has something left over to
            // centre the line with — a Fill height would take the whole bar
            // and set the title against its top edge.
            self.draw_title
                .draw_walk(cx, Walk::new(Size::fill(), Size::fit()), align, &title);
        }
        cx.end_turtle();
    }
}

impl Widget for Toolbar {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        let mut walk = walk;
        // A bar puts things at both ends of a width it is given, so a Fit
        // width is a bar with no ends: it would be laid out and never
        // painted. Taken as Fill instead of drawn as nothing.
        if walk.width.is_fit() {
            walk.width = Size::fill();
        }
        if walk.height.is_fit() {
            walk.height = Size::Fixed(self.bar_height);
        }

        let (top, bottom) = self.edge.weights();
        self.draw_bg.edge_top = top;
        self.draw_bg.edge_bottom = bottom;

        let mut layout = self.layout;
        layout.flow = Flow::right();
        layout.spacing = 0.0;
        layout.align = Align::default();
        self.draw_bg.begin(cx, walk, layout);

        let lead = self.draw_end(cx, scope, live_id!(leading));
        let ends = shown(&self.trailing) || shown(&self.floating);
        let (before, after) = middle_gaps(lead, ends, self.gap);
        let mut middle = Walk::new(Size::fill(), Size::fill());
        middle.margin.left = before;
        middle.margin.right = after;

        // Deferred: drawn in order, the middle would take the whole row and
        // leave the trailing slot nowhere to be.
        let mut deferred = cx.defer_walk_turtle(middle);
        if deferred.is_none() {
            self.draw_middle(cx, scope, middle);
        }
        self.draw_end(cx, scope, live_id!(trailing));
        self.draw_end(cx, scope, live_id!(floating));
        if let Some(deferred) = deferred.as_mut() {
            let resolved = deferred.resolve(cx);
            self.draw_middle(cx, scope, resolved);
        }

        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }
        if !self.visible {
            return;
        }
        for slot in [&self.leading, &self.center, &self.trailing, &self.floating] {
            slot.handle_event(cx, event, scope);
        }
    }

    fn text(&self) -> String {
        self.title.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.set_title(cx, v);
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.title.clone())
    }
}

impl ToolbarRef {
    pub fn title(&self) -> String {
        self.borrow().map(|inner| inner.title.clone()).unwrap_or_default()
    }

    pub fn set_title(&self, cx: &mut Cx, title: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_title(cx, title);
        }
    }

    pub fn raised(&self, cx: &Cx) -> bool {
        self.borrow().map(|inner| inner.raised(cx)).unwrap_or(false)
    }

    pub fn set_raised(&self, cx: &mut Cx, raised: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_raised(cx, raised);
        }
    }

    /// Raise or settle the bar for a scroll offset, and answer what it now
    /// is. The whole of the scroll story lives at the call site: the bar
    /// cannot see the view under it.
    pub fn set_raised_for_scroll(&self, cx: &mut Cx, scroll: f64) -> bool {
        if let Some(mut inner) = self.borrow_mut() {
            return inner.set_raised_for_scroll(cx, scroll);
        }
        false
    }

    /// The widget in a slot, for a caller that needs the real thing.
    pub fn leading(&self) -> WidgetRef {
        self.borrow().map(|inner| inner.leading.clone()).unwrap_or_default()
    }

    pub fn center(&self) -> WidgetRef {
        self.borrow().map(|inner| inner.center.clone()).unwrap_or_default()
    }

    pub fn trailing(&self) -> WidgetRef {
        self.borrow().map(|inner| inner.trailing.clone()).unwrap_or_default()
    }

    pub fn floating(&self) -> WidgetRef {
        self.borrow().map(|inner| inner.floating.clone()).unwrap_or_default()
    }
}

/// The block at the top of a page: a trail, a title, and the actions.
#[derive(Script, ScriptHook, Widget)]
pub struct PageHeader {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    /// The header's ground, transparent by default: a page header takes the
    /// colour of the page it is on.
    #[redraw]
    #[live]
    pub draw_bg: DrawColor,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_subtitle: DrawText,
    #[live]
    draw_description: DrawText,

    /// The trail above the title.
    #[find]
    #[live]
    pub breadcrumb: WidgetRef,
    /// The page's actions, beside the words rather than under them.
    #[find]
    #[live]
    pub actions: WidgetRef,

    #[live]
    pub title: String,
    #[live]
    pub subtitle: String,
    #[live]
    pub description: String,

    /// The gap between the words and the actions.
    #[live(12.0)]
    pub gap: f64,
    /// The gap between the title, the subtitle and the description.
    #[live(3.0)]
    pub text_spacing: f64,

    #[visible]
    #[live(true)]
    visible: bool,
}

impl PageHeader {
    pub fn set_title(&mut self, cx: &mut Cx, title: &str) {
        if self.title != title {
            self.title = title.to_string();
            self.draw_bg.redraw(cx);
        }
    }

    pub fn set_subtitle(&mut self, cx: &mut Cx, subtitle: &str) {
        if self.subtitle != subtitle {
            self.subtitle = subtitle.to_string();
            self.draw_bg.redraw(cx);
        }
    }

    pub fn set_description(&mut self, cx: &mut Cx, description: &str) {
        if self.description != description {
            self.description = description.to_string();
            self.draw_bg.redraw(cx);
        }
    }

    /// The words, in the column the actions left them.
    fn draw_words(&mut self, cx: &mut Cx2d, walk: Walk) {
        cx.begin_turtle(
            walk,
            Layout {
                flow: Flow::Down,
                spacing: self.text_spacing,
                ..Layout::default()
            },
        );
        let title = self.title.clone();
        if !title.is_empty() {
            draw_wrapped(cx, &mut self.draw_title, &title);
        }
        let subtitle = self.subtitle.clone();
        if !subtitle.is_empty() {
            draw_wrapped(cx, &mut self.draw_subtitle, &subtitle);
        }
        let description = self.description.clone();
        if !description.is_empty() {
            draw_wrapped(cx, &mut self.draw_description, &description);
        }
        cx.end_turtle();
    }
}

impl Widget for PageHeader {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        let mut walk = walk;
        // The words wrap to a column, and a Fit width gives them no column
        // to wrap to: they would be laid out and never painted.
        if walk.width.is_fit() {
            walk.width = Size::fill();
        }
        let mut layout = self.layout;
        layout.flow = Flow::Down;
        self.draw_bg.begin(cx, walk, layout);

        draw_slot(cx, scope, self.uid, live_id!(breadcrumb), &self.breadcrumb);

        // The words and the actions are a row, and the words are the same
        // deferred fill the toolbar's middle is: taken in order they would
        // have the whole row, and the actions would land past the edge.
        cx.begin_turtle(
            Walk::fill_fit(),
            Layout {
                flow: Flow::right(),
                spacing: 0.0,
                align: Align::default(),
                ..Layout::default()
            },
        );
        let mut column = Walk::new(Size::fill(), Size::fit());
        if shown(&self.actions) {
            column.margin.right = self.gap.max(0.0);
        }
        let mut deferred = cx.defer_walk_turtle(column);
        if deferred.is_none() {
            self.draw_words(cx, column);
        }
        draw_slot(cx, scope, self.uid, live_id!(actions), &self.actions);
        if let Some(deferred) = deferred.as_mut() {
            let resolved = deferred.resolve(cx);
            self.draw_words(cx, resolved);
        }
        cx.end_turtle();

        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.visible && !matches!(event, Event::FingerCancel(_)) {
            return;
        }
        for slot in [&self.breadcrumb, &self.actions] {
            slot.handle_event(cx, event, scope);
        }
    }

    fn text(&self) -> String {
        self.title.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.set_title(cx, v);
    }

    /// The title and the subtitle as one line, so a test can wait on the
    /// wording rather than on which property it came from.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        if self.subtitle.is_empty() {
            return Some(self.title.clone());
        }
        Some(format!("{} \u{2014} {}", self.title, self.subtitle))
    }
}

impl PageHeaderRef {
    pub fn set_title(&self, cx: &mut Cx, title: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_title(cx, title);
        }
    }

    pub fn set_subtitle(&self, cx: &mut Cx, subtitle: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_subtitle(cx, subtitle);
        }
    }

    pub fn set_description(&self, cx: &mut Cx, description: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_description(cx, description);
        }
    }

    pub fn breadcrumb(&self) -> WidgetRef {
        self.borrow().map(|inner| inner.breadcrumb.clone()).unwrap_or_default()
    }

    pub fn actions(&self) -> WidgetRef {
        self.borrow().map(|inner| inner.actions.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gap belongs between two things. A bar with no navigation control
    /// starts its title at the padding, not a gap in from it.
    #[test]
    fn a_gap_only_exists_between_two_things_that_are_there() {
        assert_eq!(middle_gaps(true, true, 8.0), (8.0, 8.0));
        assert_eq!(middle_gaps(false, true, 8.0), (0.0, 8.0));
        assert_eq!(middle_gaps(true, false, 8.0), (8.0, 0.0));
        assert_eq!(middle_gaps(false, false, 8.0), (0.0, 0.0));
    }

    #[test]
    fn a_negative_gap_is_no_gap() {
        assert_eq!(middle_gaps(true, true, -4.0), (0.0, 0.0));
    }

    /// A bar over content that has not moved is flat, whatever the
    /// thresholds say.
    #[test]
    fn a_bar_over_content_that_has_not_moved_is_flat() {
        assert!(!toolbar_raised(0.0, false, 8.0, 2.0));
        assert!(!toolbar_raised(0.0, true, 8.0, 2.0));
        // Even one told to lift at nothing: at rest is the one state a bar
        // has to get right, and a bar raised over unmoved content is a bar
        // that is always raised.
        assert!(!toolbar_raised(0.0, false, 0.0, 0.0));
        assert!(!toolbar_raised(0.0, true, 0.0, 0.0));
    }

    /// THE POINT OF THE TWO THRESHOLDS: between them the bar keeps doing
    /// whatever it was already doing. With one threshold the bar's colour
    /// changes on every fraction of a point the content drifts by as it
    /// comes to rest on the boundary.
    #[test]
    fn between_the_thresholds_the_bar_stays_as_it_was() {
        assert!(!toolbar_raised(5.0, false, 8.0, 2.0), "not lifted yet");
        assert!(toolbar_raised(5.0, true, 8.0, 2.0), "and not settled yet");
    }

    #[test]
    fn it_lifts_at_the_top_threshold_and_settles_at_the_bottom_one() {
        assert!(toolbar_raised(8.0, false, 8.0, 2.0));
        assert!(toolbar_raised(2.0, true, 8.0, 2.0), "2 is still above the settle point");
        assert!(!toolbar_raised(1.0, true, 8.0, 2.0));
    }

    /// Two thresholds written the wrong way round would raise and lower the
    /// bar on the same movement, which is the flicker they exist to stop.
    #[test]
    fn a_settle_point_above_the_lift_point_is_held_below_it() {
        // A settle of 40 is pulled down to the lift point, 10.
        assert!(toolbar_raised(20.0, true, 10.0, 40.0));
        assert!(!toolbar_raised(5.0, true, 10.0, 40.0));
    }

    /// Which edge carries the rule is decided once, in Rust, so the shader
    /// has no branch to take.
    #[test]
    fn each_edge_names_one_rule() {
        assert_eq!(ToolbarEdge::None.weights(), (0.0, 0.0));
        assert_eq!(ToolbarEdge::Top.weights(), (1.0, 0.0));
        assert_eq!(ToolbarEdge::Bottom.weights(), (0.0, 1.0));
        assert_eq!(ToolbarEdge::default(), ToolbarEdge::None);
    }
}
