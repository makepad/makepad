//! Dialog — the overlay that stops the work and asks for an answer.
//!
//! A dialog is the opposite of a toast, and the difference is the whole
//! design. A toast reports and expects to be ignored; a dialog interrupts
//! and expects to be answered, so it takes the pointer, takes the keyboard,
//! dims what is behind it, and does not go away by itself. Everything a
//! dialog costs the person using the app is justified only by there being a
//! question that genuinely cannot wait, which is why the library gives the
//! shape a name rather than leaving every app to assemble one out of a
//! modal and some views.
//!
//! **Three slots, and a rule about the third.** A dialog is a `title`, a
//! `body` and a row of actions. The actions are pinned to the bottom and
//! never scroll, because a long body that pushes its buttons off the screen
//! is how a person ends up unable to answer the question they were stopped
//! for. The body scrolls; the actions do not.
//!
//! **The default action and the way out.** Return presses the dialog's
//! default action, Escape leaves without answering, and both are the same
//! promise: a dialog can always be dismissed by keyboard alone. A dialog
//! that is not `dismissable` still answers Return, because a question with
//! no way out at all is a trap, and the library will not help build one.
//! When it closes, the key focus goes back where it came from.
//!
//! **Destructive answers are drawn as such.** `destructive` puts the error
//! role on the confirming action, and moves it away from the edge the hand
//! rests on, because "delete everything" and "cancel" a few pixels apart is
//! a design that will eventually delete everything.
//!
//! **A drawer is a dialog that has chosen a side.** `side` is
//! `PanelEdge.Center` for the card in the middle of the window, and an edge
//! for the panel that comes in from it. The panel stops the work the same
//! way — scrim, pointer taken, keyboard taken — but it is shaped by its
//! edge: on the left or right it is a column with a width, on the top or
//! bottom a row with a height. Navigation, filters and a long list of
//! settings belong there rather than in a centred card, because they are
//! places rather than questions. `Drawer`, `SideSheet` and `BottomSheet` are
//! this widget with a side chosen and no answers, and nothing else.
//!
//! **The side decides what the size means.** `size` is a width in the
//! middle and on the left or right, and a height on the top or bottom,
//! which is why the rungs are named for how much room they take rather than
//! for a number.
//!
//! **No answer, no row of them.** An answer with no label is left out, and
//! with neither label the row goes too. Return then belongs to whatever is
//! inside the panel: a list in a drawer chooses with it, and a dialog that
//! took it for a default answer it does not have would close on the choice.
//!
//! **A sheet is a panel with a grabber.** The grabber is the handle, and
//! dragging it moves the panel between three rungs — a peek, half the room,
//! and the whole of what its `size` asks for — while a drag below the lowest
//! rung sends it back the way it came. A panel without a grabber has no
//! rungs at all: it is open or it is shut, and it should not draw a handle
//! for a thing it cannot do. Neither does a card in the middle, which has
//! no edge to be pulled from.
//!
//! **A panel comes from its edge.** It slides in over
//! `theme.motion_medium_2` rather than appearing; on a panel this large the
//! movement is what says where it came from, and one that simply appears
//! reads as the page having been replaced.
//!
//! **Nesting.** A dialog is an overlay like any other: it holds its level of
//! the scroll block, and takes Escape through the shared claim, so a
//! popover or a menu opened from inside it closes first and one press
//! closes one thing.
//!
//! What stops the page and gives the keyboard back is the modal under it
//! (modal.rs): a press on a row goes to the row even where a panel lies
//! over a list the event reaches first, and closing hands the keyboard back
//! to what had it when the dialog opened, however often the page under it
//! was drawn meanwhile.

use crate::{
    button::ButtonWidgetRefExt,
    label::LabelWidgetRefExt,
    makepad_derive_widget::*,
    makepad_draw::*,
    modal::Modal,
    overlay_place::claim_escape,
    widget::*,
};

/// Where a dialog stands: in the middle of the window, or on the edge it
/// comes in from.
///
/// Named `PanelEdge` rather than for the drawer it shapes on purpose: the
/// widget derive treats any field whose TYPE name begins with "Draw" as a
/// shader layer and calls `area()` on it, so a field of such a type will
/// not compile.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum PanelEdge {
    Left = 0,
    Right = 1,
    Top = 2,
    Bottom = 3,
    /// No edge: a card in the middle of the window.
    #[pick]
    Center = 4,
}

impl PanelEdge {
    /// Whether the dialog stands on an edge rather than in the middle.
    pub fn is_edge(self) -> bool {
        self != PanelEdge::Center
    }

    /// Whether a panel on this edge is a column (left or right) rather
    /// than a row.
    pub fn is_column(self) -> bool {
        matches!(self, PanelEdge::Left | PanelEdge::Right)
    }
}

/// How much room a dialog takes. The rungs are the questions people ask: a
/// confirmation is small, a form is medium, a preview is large. On an edge
/// the same rungs are a list of a few things, navigation, a working panel.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum DialogSize {
    /// A yes or no question.
    Xs = 0,
    /// A short form; on an edge, a list of a few things.
    #[pick]
    Sm = 1,
    /// A longer form; on an edge the usual, navigation or a set of filters.
    Md = 2,
    /// A form beside something to look at; on an edge, a working panel.
    Lg = 3,
    /// Nearly the window.
    Xl = 4,
    /// The window, for a task that has taken over; on an edge, the whole
    /// of it from side to side.
    Full = 5,
}

impl DialogSize {
    /// The width of a card in the middle, in layout points, or `None` for
    /// one that takes the room it is given.
    pub fn width(self) -> Option<f64> {
        match self {
            DialogSize::Xs => Some(320.0),
            DialogSize::Sm => Some(420.0),
            DialogSize::Md => Some(560.0),
            DialogSize::Lg => Some(720.0),
            DialogSize::Xl => Some(920.0),
            DialogSize::Full => None,
        }
    }

    /// The extent of a dialog on `side`, or `None` for one that takes the
    /// room it is given: a width in the middle and on the left or right, a
    /// height on the top or bottom. A column is wider than a row is tall at
    /// the same rung, because a list of things needs width and a set of
    /// choices needs less height.
    pub fn extent(self, side: PanelEdge) -> Option<f64> {
        match side {
            PanelEdge::Center => self.width(),
            PanelEdge::Left | PanelEdge::Right => match self {
                DialogSize::Xs => Some(200.0),
                DialogSize::Sm => Some(260.0),
                DialogSize::Md => Some(360.0),
                DialogSize::Lg => Some(520.0),
                DialogSize::Xl => Some(720.0),
                DialogSize::Full => None,
            },
            PanelEdge::Top | PanelEdge::Bottom => match self {
                DialogSize::Xs => Some(120.0),
                DialogSize::Sm => Some(180.0),
                DialogSize::Md => Some(300.0),
                DialogSize::Lg => Some(460.0),
                DialogSize::Xl => Some(600.0),
                DialogSize::Full => None,
            },
        }
    }
}

/// Where a sheet rests between drags.
///
/// Only meaningful on a panel with a grabber: one without is open or shut
/// and has nothing in between. There is deliberately no `Hidden` rung — a
/// sheet dragged below the lowest one is dismissed through the same path
/// Escape and the scrim already use, so "closed" keeps one meaning and one
/// place that decides it, rather than two that can disagree about a panel
/// the user is looking at.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum SheetDetent {
    /// A peek: the grabber, the title, and the first of what is under them.
    Collapsed = 0,
    /// Half the room the window has.
    Half = 1,
    /// The whole extent this panel's own `size` asks for.
    #[pick]
    Expanded = 2,
}

/// The shortest slide a panel takes; one given this or less is in place at
/// once.
const SLIDE_FLOOR_SECS: f64 = 0.01;

/// How tall a collapsed sheet stands: enough for the grabber, the title and
/// the first line under them. Below this it reads as a bar rather than a
/// panel, which is a different thing and not what a sheet is for.
const SHEET_COLLAPSED: f64 = 96.0;

/// How far open a sheet stands at one rung, given the extent its own `size`
/// asks for and the room the window has.
///
/// `Half` is clamped into the other two rather than taken literally: on a
/// short window half the room can be less than a peek, and on a tall one it
/// can be more than the panel ever asked for, and in both cases the rung
/// that survives is the one the sheet can actually stand at.
fn detent_extent(detent: SheetDetent, base: f64, room: f64) -> f64 {
    let collapsed = SHEET_COLLAPSED.min(base);
    match detent {
        SheetDetent::Collapsed => collapsed,
        SheetDetent::Half => (room * 0.5).clamp(collapsed, base),
        SheetDetent::Expanded => base,
    }
}

/// Which rung a dragged sheet settles at, or `None` when the drag went far
/// enough below the lowest one to mean "let go of me".
///
/// Nearest rung wins. The dismiss threshold is half the collapsed rung
/// rather than zero, so letting go of a sheet that has been pulled most of
/// the way down closes it instead of leaving a sliver on screen that the
/// pointer has already left.
fn settle_detent(extent: f64, collapsed: f64, half: f64, expanded: f64) -> Option<SheetDetent> {
    if extent < collapsed * 0.5 {
        return None;
    }
    [
        (SheetDetent::Collapsed, collapsed),
        (SheetDetent::Half, half),
        (SheetDetent::Expanded, expanded),
    ]
    .into_iter()
    .min_by(|a, b| (a.1 - extent).abs().total_cmp(&(b.1 - extent).abs()))
    .map(|(rung, _)| rung)
}

/// What a dialog reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum DialogAction {
    /// The dialog came up.
    Opened,
    /// The default action was taken, by its button or by Return.
    Confirmed,
    /// The other action was taken.
    Cancelled,
    /// The dialog was left without an answer: Escape, the close mark, a
    /// press outside it, or a sheet dragged below its lowest rung.
    Dismissed,
    /// A sheet was dragged to a new rung and let go there.
    DetentChanged(SheetDetent),
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.DialogSize = set_type_default() do #(DialogSize::script_api(vm))
    mod.widgets.splat(mod.widgets.DialogSize)
    // Not splatted, unlike DialogSize: a splatted variant is exported as a
    // bare name, and Left, Center, Collapsed and Half are words a widget
    // could plausibly want. Written out as `PanelEdge.Left` and
    // `SheetDetent.Half`, they cannot shadow anything.
    mod.widgets.PanelEdge = set_type_default() do #(PanelEdge::script_api(vm))
    mod.widgets.SheetDetent = set_type_default() do #(SheetDetent::script_api(vm))

    use mod.widgets.*

    mod.widgets.DialogBase = #(Dialog::register_widget(vm))
    /** A question that stops the work: a title, a body that scrolls, and a
     * row of answers that does not. */
    mod.widgets.Dialog = set_type_default() do mod.widgets.DialogBase{
        // A widget that derefs another does not inherit its DSL, so the
        // modal's own chrome is spelled out here: the overlay flow that
        // centres the card in the pass, the transparent quad the modal
        // begins with, and the scrim behind it.
        flow: Overlay
        align: Center
        draw_bg +: {
            pixel: fn() {
                return vec4(0. 0. 0. 0.0)
            }
        }
        bg_view := View{
            width: Fill
            height: Fill
            show_bg: true
            draw_bg +: {
                // The scrim is two tokens, not one: `color_scrim` is the
                // colour and `state_scrim_opacity` is how much of it. The
                // colour alone is opaque black, which would hide the page
                // rather than dim it.
                color: uniform(theme.color_scrim)
                dim: uniform(theme.state_scrim_opacity)
                pixel: fn() {
                    return vec4(self.color.xyz, self.color.a * self.dim)
                }
            }
        }

        /** where it stands: PanelEdge.Center, or the edge it comes in from, PanelEdge.Left Right Top Bottom */
        side: PanelEdge.Center
        /** how much room it takes, a width in the middle and on the left or right, a height on the top or bottom: Xs Sm Md Lg Xl Full */
        size: Sm
        /** Escape, the close mark and a press outside all leave without answering */
        dismissable: true
        // The modal underneath swallows presses on its scrim whatever this
        // says; what it must NOT do is close itself, because it would raise
        // its own action on this same widget uid and a lookup by uid answers
        // with whichever came first. The dialog closes itself instead.
        can_dismiss: false
        /** the confirming answer destroys something: it takes the error role */
        destructive: false
        /** a grabber at the leading edge, for a panel on an edge that can be dragged */
        grabber: false
        /** which rung a sheet opens at: SheetDetent.Collapsed Half Expanded */
        detent: SheetDetent.Expanded
        /** the title, also what `text()` answers */
        title: ""
        /** the label on the answer Return takes; empty leaves it out, and Return to what is inside */
        confirm_text: "OK"
        /** the label on the other answer; empty leaves it out */
        cancel_text: ""
        /** how long a panel on an edge takes to come in 0..1 step 0.01 */
        slide_secs: theme.motion_medium_2

        content := RoundedShadowView{
            width: 420.
            height: Fit
            flow: Down
            show_bg: true
            draw_bg +: {
                color: theme.color_surface_container_high
                border_color: theme.color_outline
                border_size: 1.0
                border_radius: theme.radius_l
                shadow_color: theme.color_elevation_4
                shadow_radius: uniform(theme.elevation_4_radius)
                shadow_offset: uniform(vec2(0., theme.elevation_4_offset_y))
            }

            grab := View{
                width: Fill
                height: Fit
                align: Align{x: 0.5}
                padding: Inset{top: 8. bottom: 2.}
                visible: false
                RoundedView{
                    width: 36.
                    height: 4.
                    draw_bg +: {
                        color: theme.color_outline
                        border_radius: 2.
                    }
                }
            }

            header := View{
                width: Fill
                height: Fit
                flow: Right
                align: Align{y: 0.5}
                padding: Inset{left: 20. right: 12. top: 16. bottom: 8.}
                spacing: theme.space_2
                title_label := Label{
                    width: Fill
                    draw_text +: {
                        text_style: theme.font_title_s
                        color: theme.color_on_surface
                    }
                }
                close := ButtonFlat{
                    width: 24.
                    height: 24.
                    text: "\u{00d7}"
                }
            }

            body := View{
                width: Fill
                height: Fit
                flow: Down
                spacing: theme.space_2
                padding: Inset{left: 20. right: 20. top: 0. bottom: 8.}
                scroll_bars: ScrollBars{show_scroll_x: false show_scroll_y: true}
            }

            footer := View{
                width: Fill
                height: Fit
                flow: Right
                align: Align{x: 1.0 y: 0.5}
                spacing: theme.space_2
                padding: Inset{left: 20. right: 20. top: 8. bottom: 16.}
                Filler{}
                cancel := ButtonFlat{text: "Cancel"}
                confirm := Button{text: "OK"}
                /** the same answer in the error role, shown instead of
                 * `confirm` while `destructive` is set */
                confirm_danger := ButtonDanger{text: "Delete" visible: false}
            }
        }
    }

    /** A dialog that only tells: one way out, no question. */
    mod.widgets.AlertDialog = mod.widgets.Dialog{
        size: Xs
        confirm_text: "OK"
        cancel_text: ""
    }

    /** A dialog that asks: an answer and a way out. */
    mod.widgets.ConfirmDialog = mod.widgets.Dialog{
        size: Xs
        confirm_text: "Confirm"
        cancel_text: "Cancel"
    }

    /** A dialog that asks before destroying something. */
    mod.widgets.DangerDialog = mod.widgets.ConfirmDialog{
        destructive: true
        confirm_text: "Delete"
    }

    /** A task that has taken over the window. */
    mod.widgets.FullScreenDialog = mod.widgets.Dialog{
        size: Full
        confirm_text: "Done"
        cancel_text: ""
    }

    /** A panel that comes in from an edge: navigation, filters, settings. */
    mod.widgets.Drawer = mod.widgets.Dialog{
        side: PanelEdge.Left
        size: Md
        // A place rather than a question: it has no answers, so it draws no
        // row of them and leaves Return to what is inside it.
        confirm_text: ""
        cancel_text: ""
        content +: {
            height: Fill
            // The body takes what the header leaves, so a long list scrolls
            // inside the panel, and carries the bottom padding the row of
            // answers would have.
            body +: {
                height: Fill
                padding: Inset{left: 20. right: 20. top: 0. bottom: 16.}
            }
        }
    }

    /** A drawer from the right, for a detail panel beside the work. */
    mod.widgets.SideSheet = mod.widgets.Drawer{
        side: PanelEdge.Right
    }

    /** A panel from the bottom with a grabber: the shape a phone uses for
     * everything, and a good one for a short set of choices anywhere. */
    mod.widgets.BottomSheet = mod.widgets.Drawer{
        side: PanelEdge.Bottom
        grabber: true
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct Dialog {
    #[deref]
    modal: Modal,
    #[live]
    pub side: PanelEdge,
    #[live]
    pub size: DialogSize,
    #[live(true)]
    pub dismissable: bool,
    #[live]
    pub destructive: bool,
    #[live]
    pub grabber: bool,
    #[live(SheetDetent::Expanded)]
    pub detent: SheetDetent,
    #[live]
    pub title: String,
    #[live]
    pub confirm_text: String,
    #[live]
    pub cancel_text: String,
    #[live(0.3)]
    pub slide_secs: f64,
    /// Whether the chrome has been written from the props this open.
    #[rust]
    dressed: bool,
    /// How far open the panel stands right now, along its own axis. Only a
    /// sheet moves this; without a grabber the panel's `size` decides.
    #[rust]
    live_extent: f64,
    /// Whether `live_extent` has been read from `detent` yet, this open.
    #[rust]
    extent_dressed: bool,
    /// The room the window had at the last draw, along this panel's axis.
    /// Kept because a drag arrives on an event, where the pass is not.
    #[rust]
    pass_extent: f64,
    /// Where the pointer took hold, and how far open the panel was then.
    #[rust]
    drag_from: Option<(f64, f64)>,
    /// Whether the press now being held landed ON the card. Its release is
    /// then that control's, wherever it lands; see the scrim check in
    /// `handle_event`. False when no press has been seen at all, which
    /// leaves a release nobody claims free to dismiss as it always was.
    #[rust]
    press_on_card: bool,
    /// How far in the panel is, 0 at its edge and 1 in place.
    #[rust]
    slide: f64,
    #[rust]
    sliding: bool,
    #[rust]
    last_t: f64,
    #[rust]
    next_frame: NextFrame,
}

impl Dialog {
    /// Show the dialog and take the keyboard. The modal under it remembers
    /// where the keyboard was: a handle kept here went stale at the page's
    /// next redraw, and closing gave the keyboard to nothing.
    pub fn open_dialog(&mut self, cx: &mut Cx) {
        self.dressed = false;
        // A sheet opens at its declared rung every time, rather than where
        // the last drag left it: reopening is a new question being asked,
        // not the same panel coming back.
        self.extent_dressed = false;
        self.drag_from = None;
        self.press_on_card = false;
        // A panel given no time to slide is in place from its first frame.
        // Sliding from its edge over the one frame the floor still takes, it
        // stood at its edge on that frame, and a press there was a press on
        // the scrim; a host that moves the panel itself (HamburgerMenu) gives
        // it exactly this floor. A card in the middle comes from nowhere.
        let at_once = !self.side.is_edge() || self.slide_secs <= SLIDE_FLOOR_SECS;
        self.slide = if at_once { 1.0 } else { 0.0 };
        self.sliding = !at_once;
        if self.side.is_edge() {
            self.last_t = cx.seconds_since_app_start();
            self.next_frame = cx.new_next_frame();
        }
        self.modal.open(cx);
        let uid = self.widget_uid();
        cx.widget_action(uid, DialogAction::Opened);
    }

    /// Close it and give the keyboard back to whatever had it.
    ///
    /// The modal does the giving back, and follows the place across the
    /// redraws since the open. The dialog used to keep a handle of its own
    /// from the open and hand the keyboard to it after the modal had: by then
    /// the page had been drawn again, that handle named nothing, and the
    /// keyboard reached no widget, so the next Tab started from nowhere.
    ///
    /// Straight out rather than sliding back: a panel that animates away
    /// has to keep taking the pointer while it does, and one that is
    /// leaving must not swallow the press that follows it.
    pub fn close_dialog(&mut self, cx: &mut Cx) {
        self.modal.close(cx);
        self.sliding = false;
    }

    pub fn is_open(&self) -> bool {
        self.modal.is_open()
    }

    /// How far in a panel on an edge is: 0 at its edge, 1 in place.
    pub fn slide(&self) -> f64 {
        self.slide
    }

    /// Which rung the sheet rests at.
    pub fn detent(&self) -> SheetDetent {
        self.detent
    }

    /// Move the sheet to a rung, the way a drag would leave it there.
    pub fn set_detent(&mut self, cx: &mut Cx, detent: SheetDetent) {
        self.detent = detent;
        if self.pass_extent > 0.0 {
            let base = self.size.extent(self.side).unwrap_or(self.pass_extent);
            self.live_extent = detent_extent(detent, base, self.pass_extent);
            self.extent_dressed = true;
        } else {
            // Nothing has measured the window yet — this is a rung being
            // set in the same breath as the open, before a single draw —
            // so there is no room to compute against. Leave it for the
            // draw that is about to happen, which will have the pass.
            // Computing here would resolve every rung to nothing and then
            // mark the answer as final, which is a sheet that opens
            // invisible and stays that way.
            self.extent_dressed = false;
        }
        self.modal.redraw(cx);
    }

    /// Whether Return has an answer to take. One with no label is not
    /// drawn, and a key must not press a button nobody can see.
    fn has_default_answer(&self) -> bool {
        !self.confirm_text.is_empty()
    }

    /// The areas this dialog counts as its own for the pointer-capture rule:
    /// the scrim, the card, and the grabber.
    ///
    /// The sheet's grabber and the dismissing release are read off raw
    /// events, which never learn that another widget took the press (see
    /// `Fingers::is_mouse_held_outside`). A press on the scrim IS captured —
    /// by the modal's own bg, which it hit tests unconditionally — and a
    /// press on a control inside the card is captured by that control. Only
    /// the second kind means somebody else holds the mouse, so the first has
    /// to be named here or every sheet drag would stand down at once.
    fn pointer_areas(&self, cx: &Cx) -> Vec<Area> {
        let content = self.modal.widget(cx, ids!(content));
        let grab = content.widget(cx, ids!(grab)).area();
        [self.modal.scrim_area(), content.area(), grab]
            .into_iter()
            .filter(|area| !area.is_empty())
            .collect()
    }

    /// Whether the grabber is drawn and heard: only a panel on an edge has
    /// somewhere to be dragged to.
    fn is_sheet(&self) -> bool {
        self.grabber && self.side.is_edge()
    }

    /// Write the props into the chrome. Done on the way into a draw rather
    /// than on apply, because a host sets `title` and the labels at the
    /// moment it opens the dialog, not when the DSL was applied.
    fn dress(&mut self, cx: &mut Cx) {
        let content = self.modal.widget(cx, ids!(content));
        content.label(cx, ids!(title_label)).set_text(cx, &self.title);
        // The destructive answer is a different button, not a recoloured
        // one: a button's face colours are shader uniforms it owns.
        let asks = self.has_default_answer();
        let confirm = content.button(cx, ids!(confirm));
        let confirm_danger = content.button(cx, ids!(confirm_danger));
        confirm.set_text(cx, &self.confirm_text);
        confirm_danger.set_text(cx, &self.confirm_text);
        confirm.set_visible(cx, asks && !self.destructive);
        confirm_danger.set_visible(cx, asks && self.destructive);
        let cancel = content.button(cx, ids!(cancel));
        cancel.set_text(cx, &self.cancel_text);
        // An answer with no label is not an answer: leave it out rather
        // than drawing an empty button. With neither, the row they stand in
        // goes too, or a drawer would end in a strip of padding.
        cancel.set_visible(cx, !self.cancel_text.is_empty());
        content
            .widget(cx, ids!(footer))
            .set_visible(cx, asks || !self.cancel_text.is_empty());
        content.widget(cx, ids!(close)).set_visible(cx, self.dismissable);
        content.widget(cx, ids!(grab)).set_visible(cx, self.is_sheet());
    }

    fn answer(&mut self, cx: &mut Cx, action: DialogAction) {
        let uid = self.widget_uid();
        cx.widget_action(uid, action);
        self.close_dialog(cx);
    }

    /// Advance the slide. Answers whether it is still moving.
    fn step(&mut self, cx: &mut Cx) -> bool {
        if !self.sliding {
            return false;
        }
        let now = cx.seconds_since_app_start();
        let dt = (now - self.last_t).clamp(0.0, 0.1);
        self.last_t = now;
        let secs = self.slide_secs.max(SLIDE_FLOOR_SECS);
        self.slide = (self.slide + dt / secs).min(1.0);
        if self.slide >= 1.0 {
            self.sliding = false;
        }
        self.sliding
    }

    /// The walk a panel on an edge takes, and where it sits while it is
    /// arriving.
    fn panel_walk(&self, pass: DVec2) -> (Walk, DVec2) {
        let column = self.side.is_column();
        // A sheet stands where it has been dragged to; every other panel
        // stands where its size says, on exactly the path it always took.
        let extent = if self.is_sheet() {
            Some(self.live_extent)
        } else {
            self.size.extent(self.side)
        };
        let (w, h) = if column {
            (extent.map(Size::Fixed).unwrap_or(Size::fill()), Size::fill())
        } else {
            (Size::fill(), extent.map(Size::Fixed).unwrap_or(Size::fill()))
        };
        // Eased out: a panel this large arriving at a constant speed reads
        // as being dragged rather than as having been sent for.
        let t = {
            let inv = 1.0 - self.slide.clamp(0.0, 1.0);
            1.0 - inv * inv * inv
        };
        let size = match (column, extent) {
            (true, Some(e)) => dvec2(e, pass.y),
            (true, None) => dvec2(pass.x, pass.y),
            (false, Some(e)) => dvec2(pass.x, e),
            (false, None) => dvec2(pass.x, pass.y),
        };
        let off = 1.0 - t;
        let pos = match self.side {
            PanelEdge::Left => dvec2(-size.x * off, 0.0),
            PanelEdge::Right => dvec2(pass.x - size.x + size.x * off, 0.0),
            PanelEdge::Top => dvec2(0.0, -size.y * off),
            PanelEdge::Bottom => dvec2(0.0, pass.y - size.y + size.y * off),
            // Never asked: the modal's own alignment places a card.
            PanelEdge::Center => DVec2::default(),
        };
        (Walk { width: w, height: h, ..Walk::default() }, pos)
    }
}

impl Widget for Dialog {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.is_open() && !self.dressed {
            self.dressed = true;
            self.dress(cx.cx.cx);
        }
        let on_edge = self.side.is_edge();
        if self.is_open() && on_edge {
            let pass = cx.current_pass_size();
            self.pass_extent = if self.side.is_column() { pass.x } else { pass.y };
            if self.is_sheet() && !self.extent_dressed {
                self.extent_dressed = true;
                let base = self.size.extent(self.side).unwrap_or(self.pass_extent);
                self.live_extent = detent_extent(self.detent, base, self.pass_extent);
            }
            let (panel_walk, pos) = self.panel_walk(pass);
            let content = self.modal.widget(cx.cx.cx, ids!(content));
            let mut view = content.borrow_mut::<crate::view::View>();
            if let Some(view) = view.as_mut() {
                view.walk = panel_walk.with_abs_pos(pos);
            }
            drop(view);
        } else if self.is_open() {
            // The size rung is applied every draw, so a host that changes it
            // between opens gets the width it asked for.
            let content = self.modal.widget(cx.cx.cx, ids!(content));
            let width = match self.size.width() {
                Some(w) => Size::Fixed(w),
                None => Size::fill(),
            };
            let full = self.size == DialogSize::Full;
            let mut view = content.borrow_mut::<crate::view::View>();
            if let Some(view) = view.as_mut() {
                view.walk.width = width;
                if full {
                    view.walk.height = Size::fill();
                }
            }
            drop(view);
        }
        // The alignment that centres a card would move a panel too: an
        // overlay flow shifts every walk by it, the placed ones included,
        // and a drawer asked to stand on the left stood in the middle. It is
        // set aside for the draw rather than left out of the markup, so
        // `side` alone decides where a dialog stands.
        let align = self.modal.layout.align;
        if on_edge {
            self.modal.layout.align = Align::default();
        }
        let step = self.modal.draw_walk(cx, scope, walk);
        self.modal.layout.align = align;
        step
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.is_open() {
            return;
        }
        if self.side.is_edge() && self.next_frame.is_event(event).is_some() {
            if self.step(cx) {
                self.next_frame = cx.new_next_frame();
            }
            self.modal.redraw(cx);
        }
        // Escape and the outside press are the dialog's own: the modal's
        // versions are turned off in `can_dismiss`, so that one press
        // closes one overlay even with a menu open inside this dialog.
        if let Event::KeyDown(ke) = event {
            match ke.key_code {
                KeyCode::Escape if self.dismissable => {
                    if claim_escape(cx) {
                        self.answer(cx, DialogAction::Dismissed);
                        return;
                    }
                }
                // Return takes the default answer, which is the promise a
                // dialog makes to someone working by keyboard. With no such
                // answer the key goes on to what is inside.
                KeyCode::ReturnKey | KeyCode::NumpadEnter if self.has_default_answer() => {
                    self.answer(cx, DialogAction::Confirmed);
                    return;
                }
                _ => {}
            }
        }
        self.modal.handle_event(cx, event, scope);
        // The back gesture leaves a panel: a drawer is somewhere the person
        // went, and back is how one leaves a place. A question in the middle
        // of the window is not a place, and back does not answer it. Asked
        // after the content, which may have somewhere of its own to go back
        // from.
        if self.dismissable && self.side.is_edge() && event.back_pressed() {
            self.answer(cx, DialogAction::Dismissed);
            return;
        }
        // The grabber, and the only place the panel's extent is moved by
        // hand. It runs BEFORE the scrim check below and returns from every
        // arm that did something: a release that ends outside the panel it
        // has just shrunk must not also read as a press on the scrim, which
        // would send the sheet back when the user only let go of it.
        if self.is_sheet() {
            let column = self.side.is_column();
            let along = |p: DVec2| if column { p.x } else { p.y };
            let base = self.size.extent(self.side).unwrap_or(self.pass_extent);
            match event {
                Event::MouseDown(me) if self.drag_from.is_none() => {
                    let grab = self
                        .modal
                        .widget(cx, ids!(content))
                        .widget(cx, ids!(grab))
                        .area()
                        .rect(cx);
                    // A resizer, and a resizer only starts on a press
                    // nothing else is holding. A control inside the sheet
                    // that took this press owns the pointer until it is let
                    // go, and the panel must not move out from under it —
                    // a slider in the body, a scroll bar, the close mark
                    // pressed and dragged over the handle. The content is
                    // dispatched above this, so a capture it took is already
                    // on the list by the time the press is read here.
                    //
                    // The check rather than a capture of its own, and that
                    // is a structural answer rather than a lazy one. A
                    // dialog IS a modal, and by the time this block runs the
                    // modal has taken its sweep lock back:
                    // `Modal::handle_event` lifts it only around the
                    // content's own dispatch. `hits` turns away every area
                    // whose sweep area is not the lock holder's, so a plain
                    // `hits` here answers Nothing at all; and handing it the
                    // scrim as a sweep area unlocks it only by putting it in
                    // SWEEP mode, where the first move off the handle comes
                    // back as a swept `FingerUp` and the capture is dropped
                    // — the one thing a resizer must not do. Capturing
                    // without `hits` means `CxFingers::capture_digit`, which
                    // the platform keeps to itself.
                    //
                    // So this control keeps the OTHER half of the rule, and
                    // keeps it whole: asked here at the press, and asked
                    // again on every move below for as long as the drag is
                    // pending.
                    let mine = self.pointer_areas(cx);
                    if grab.size.x > 0.0
                        && grab.contains(me.abs)
                        && !cx.fingers.is_mouse_held_outside(&mine)
                    {
                        self.drag_from = Some((along(me.abs), self.live_extent));
                    }
                }
                Event::MouseMove(me) => {
                    // The other half of the rule, asked again on every move
                    // while the drag is pending. A press and another
                    // control's capture can land in either order within one
                    // event, and a control can take the pointer after this
                    // drag started; the moment one does, the drag lets go —
                    // and puts the panel back where the press found it,
                    // because a resize made with a pointer that belongs to
                    // someone else is not a resize anybody asked for. The
                    // move itself is not this panel's to consume.
                    if let Some((_, held_extent)) = self.drag_from {
                        let mine = self.pointer_areas(cx);
                        if cx.fingers.is_mouse_held_outside(&mine) {
                            self.drag_from = None;
                            self.live_extent = held_extent;
                            self.modal.redraw(cx);
                        }
                    }
                    if let Some((held_at, held_extent)) = self.drag_from {
                        // Which way makes the panel bigger depends on the
                        // edge it came from: a bottom sheet grows upward.
                        let delta = along(me.abs) - held_at;
                        let toward_open = match self.side {
                            PanelEdge::Bottom | PanelEdge::Right => -delta,
                            PanelEdge::Top | PanelEdge::Left | PanelEdge::Center => delta,
                        };
                        self.live_extent =
                            (held_extent + toward_open).clamp(0.0, base.max(self.pass_extent));
                        cx.set_cursor(if column {
                            MouseCursor::ColResize
                        } else {
                            MouseCursor::RowResize
                        });
                        self.modal.redraw(cx);
                        return;
                    }
                }
                // The mouse press itself taken away: the sheet goes back to
                // the detent it had; nothing changes detent, nothing
                // dismisses. The cancel still goes on to the content.
                Event::FingerCancel(c)
                    if c.device.is_mouse() && cx.fingers.press_taken_away(c.digit_id) && self.drag_from.is_some() =>
                {
                    self.drag_from = None;
                    self.live_extent = detent_extent(self.detent, base, self.pass_extent);
                    self.modal.redraw(cx);
                }
                Event::MouseUp(_) => {
                    if self.drag_from.take().is_some() {
                        let collapsed = SHEET_COLLAPSED.min(base);
                        let half = (self.pass_extent * 0.5).clamp(collapsed, base);
                        match settle_detent(self.live_extent, collapsed, half, base) {
                            Some(rung) => {
                                self.detent = rung;
                                self.live_extent = detent_extent(rung, base, self.pass_extent);
                                let uid = self.widget_uid();
                                cx.widget_action(uid, DialogAction::DetentChanged(rung));
                                self.modal.redraw(cx);
                            }
                            None => self.answer(cx, DialogAction::Dismissed),
                        }
                        return;
                    }
                }
                _ => {}
            }
        }
        // A press that lands outside the card leaves without answering. The
        // modal has already stopped it reaching the page underneath.
        if self.dismissable {
            let card = self.modal.widget(cx, ids!(content)).area().rect(cx);
            let off_card = |abs: DVec2| card.size.x > 0.0 && !card.contains(abs);
            match event {
                // Where the press landed decides whose release this is. A
                // press inside the card belongs to whatever it landed on —
                // a slider, a scroll bar, the sheet's own grabber — and that
                // control keeps the pointer until it is let go, which for a
                // drag is routinely far outside the card. Reading the
                // release alone shut the dialog on the user mid-drag.
                //
                // Only a press ON the card is taken away, so a release with
                // no press of its own behind it still dismisses, as it did
                // before: the press that opened the dialog landed while
                // there was no dialog to hear it.
                Event::MouseDown(me) => self.press_on_card = !off_card(me.abs),
                // The mouse press itself taken away: its release will not
                // come, so there is nothing left to suppress. A cancel of only
                // some capture of a press still held (a list recycled a row)
                // leaves the protection in place for the real release.
                Event::FingerCancel(c) if c.device.is_mouse() && cx.fingers.press_taken_away(c.digit_id) => {
                    self.press_on_card = false
                }
                Event::MouseUp(me) => {
                    if !std::mem::take(&mut self.press_on_card) && off_card(me.abs) {
                        self.answer(cx, DialogAction::Dismissed);
                        return;
                    }
                }
                _ => {}
            }
        }
        if let Event::Actions(actions) = event {
            let content = self.modal.widget(cx, ids!(content));
            if content.button(cx, ids!(confirm)).clicked(actions)
                || content.button(cx, ids!(confirm_danger)).clicked(actions)
            {
                self.answer(cx, DialogAction::Confirmed);
            } else if content.button(cx, ids!(cancel)).clicked(actions) {
                self.answer(cx, DialogAction::Cancelled);
            } else if content.button(cx, ids!(close)).clicked(actions) {
                self.answer(cx, DialogAction::Dismissed);
            }
        }
    }

    /// The title: what the dialog is asking about.
    fn text(&self) -> String {
        self.title.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.title != v {
            self.title = v.to_string();
            self.dressed = false;
            self.modal.redraw(cx);
        }
    }
}

impl DialogRef {
    pub fn open(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open_dialog(cx);
        }
    }

    pub fn close(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.close_dialog(cx);
        }
    }

    pub fn is_open(&self) -> bool {
        self.borrow().map(|inner| inner.is_open()).unwrap_or(false)
    }

    /// Set the question and its answers before opening.
    pub fn ask(&self, cx: &mut Cx, title: &str, confirm: &str, cancel: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.title = title.to_string();
            inner.confirm_text = confirm.to_string();
            inner.cancel_text = cancel.to_string();
            inner.open_dialog(cx);
        }
    }

    pub fn confirmed(&self, actions: &Actions) -> bool {
        self.answered(actions) == Some(DialogAction::Confirmed)
    }

    pub fn cancelled(&self, actions: &Actions) -> bool {
        self.answered(actions) == Some(DialogAction::Cancelled)
    }

    pub fn dismissed(&self, actions: &Actions) -> bool {
        self.answered(actions) == Some(DialogAction::Dismissed)
    }

    /// What the dialog answered this pass, if it answered. Coming up and a
    /// sheet settling on a rung are reports, not answers.
    pub fn answered(&self, actions: &Actions) -> Option<DialogAction> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<DialogAction>() {
            answer @ (DialogAction::Confirmed | DialogAction::Cancelled | DialogAction::Dismissed) => Some(answer),
            _ => None,
        }
    }

    /// Which rung the sheet rests at.
    pub fn detent(&self) -> SheetDetent {
        self.borrow()
            .map(|inner| inner.detent())
            .unwrap_or(SheetDetent::Expanded)
    }

    /// Move the sheet to a rung.
    pub fn set_detent(&self, cx: &mut Cx, detent: SheetDetent) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_detent(cx, detent);
        }
    }

    /// The rung a drag just left the sheet at, if one did.
    pub fn detent_changed(&self, actions: &Actions) -> Option<SheetDetent> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<DialogAction>() {
            DialogAction::DetentChanged(rung) => Some(rung),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::button::ButtonAction;
    use crate::makepad_draw::cx_draw::CxDraw;
    use std::cell::Cell;

    const SIZE: DVec2 = DVec2 { x: 800.0, y: 600.0 };

    fn cx() -> crate::PooledCx {
        crate::checkout_test_cx()
    }

    /// Moves the key focus the way the event loop does between events.
    fn settle_focus(cx: &mut Cx) {
        cx.action(());
        cx.handle_actions();
    }

    /// A window-less pass with the overlay a window keeps.
    struct Target {
        pass: DrawPass,
        draw_list: DrawList2d,
        overlay: Overlay,
    }

    impl Target {
        fn new(cx: &mut Cx) -> Self {
            let overlay = cx.with_vm(|vm| Overlay::script_new(vm));
            Target { pass: DrawPass::new(cx), draw_list: DrawList2d::new(cx), overlay }
        }

        fn draw(&mut self, cx: &mut Cx, root: &WidgetRef) {
            self.pass.set_size(cx, SIZE);
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&self.pass, None);
            self.draw_list.begin_always(&mut cx2d);
            self.overlay.begin(&mut cx2d);
            cx2d.begin_root_turtle(SIZE, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            self.overlay.end(&mut cx2d);
            self.draw_list.end(&mut cx2d);
            cx2d.end_pass(&self.pass);
        }
    }

    fn press(abs: DVec2) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 0.0,
        })
    }

    fn drag(abs: DVec2) -> Event {
        Event::MouseMove(MouseMoveEvent {
            abs,
            lock_delta: DVec2::default(),
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            time: 0.0,
            handled: Cell::new(Area::Empty),
        })
    }

    fn release(abs: DVec2) -> Event {
        Event::MouseUp(MouseUpEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            time: 0.0,
        })
    }

    fn key(key_code: KeyCode) -> Event {
        Event::KeyDown(KeyEvent { key_code, ..Default::default() })
    }

    fn pressed(actions: &Actions, button: &WidgetRef) -> bool {
        actions
            .iter()
            .filter_map(|action| action.as_widget_action())
            .any(|action| action.widget_uid == button.widget_uid() && matches!(action.cast::<ButtonAction>(), ButtonAction::Pressed(_)))
    }

    /// Every report `dialog` made in `actions`, in order.
    fn reports(actions: &Actions, dialog: &WidgetRef) -> Vec<DialogAction> {
        actions
            .iter()
            .filter_map(|action| action.as_widget_action())
            .filter(|action| action.widget_uid == dialog.widget_uid())
            .map(|action| action.cast::<DialogAction>())
            .filter(|action| *action != DialogAction::None)
            .collect()
    }

    /// Every way out of a dialog, Escape or a press on the scrim, gives the
    /// keyboard back to the button that opened it, as that button is after
    /// the page was drawn again while the dialog was up.
    #[test]
    fn the_keyboard_goes_back_to_the_opener_after_the_page_is_drawn_again() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Overlay
                    opener := Button{width: 200. height: 40. text: "Ask"}
                    dialog := ConfirmDialog{title: "Sure?"}
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        let opener = root.widget(&cx, ids!(opener));
        let dialog = root.widget(&cx, ids!(dialog)).as_dialog();

        for (way, event) in [("Escape", key(KeyCode::Escape)), ("the scrim", release(dvec2(780.0, 580.0)))] {
            cx.set_key_focus(opener.area());
            settle_focus(&mut cx);
            let before = opener.area();
            dialog.open(&mut cx);
            target.draw(&mut cx, &root);
            settle_focus(&mut cx);
            root.redraw(&mut cx);
            target.draw(&mut cx, &root);
            assert_ne!(opener.area(), before, "the page was drawn again, so the opener's handle moved");

            root.handle_event(&mut cx, &event, &mut Scope::empty());
            settle_focus(&mut cx);
            assert!(!dialog.is_open(), "{way} closed the dialog");
            assert!(
                cx.has_key_focus(opener.area()),
                "after {way} the keyboard went to {:?}, not back to the opener at {:?}",
                cx.key_focus(),
                opener.area()
            );
            target.draw(&mut cx, &root);
        }
        });
    }

    /// The rungs are the questions people ask, and they only go up. Full
    /// takes the room it is given rather than a width of its own.
    #[test]
    fn the_size_rungs_climb_and_full_is_not_a_number() {
        crate::on_test_cx(|| {
        let rungs = [DialogSize::Xs, DialogSize::Sm, DialogSize::Md, DialogSize::Lg, DialogSize::Xl];
        for side in [PanelEdge::Center, PanelEdge::Left, PanelEdge::Right, PanelEdge::Top, PanelEdge::Bottom] {
            let sizes: Vec<f64> = rungs.iter().map(|s| s.extent(side).unwrap()).collect();
            assert!(sizes.windows(2).all(|w| w[0] < w[1]), "{side:?}: {sizes:?} must climb");
            assert_eq!(DialogSize::Full.extent(side), None, "{side:?}");
        }
        // In the middle the extent is the width a card always had.
        for size in rungs {
            assert_eq!(size.extent(PanelEdge::Center), size.width());
        }
        assert_eq!(DialogSize::Full.width(), None);
        });
    }

    /// A side decides whether the panel is a column or a row, and the size
    /// means the other thing on each.
    #[test]
    fn the_side_decides_what_the_size_means() {
        crate::on_test_cx(|| {
        assert!(PanelEdge::Left.is_column());
        assert!(PanelEdge::Right.is_column());
        assert!(!PanelEdge::Top.is_column());
        assert!(!PanelEdge::Bottom.is_column());
        assert!(!PanelEdge::Center.is_edge());
        // A column is wider than a row is tall at the same rung: a list of
        // things needs width, a set of choices needs less height.
        assert!(DialogSize::Md.extent(PanelEdge::Left).unwrap() > DialogSize::Md.extent(PanelEdge::Top).unwrap());
        assert_eq!(DialogSize::Md.extent(PanelEdge::Left), DialogSize::Md.extent(PanelEdge::Right));
        assert_eq!(DialogSize::Md.extent(PanelEdge::Top), DialogSize::Md.extent(PanelEdge::Bottom));
        // The rungs a drawer had before it was a dialog, to the point.
        let panel = |side| [DialogSize::Sm, DialogSize::Md, DialogSize::Lg].map(|s: DialogSize| s.extent(side).unwrap());
        assert_eq!(panel(PanelEdge::Left), [260.0, 360.0, 520.0]);
        assert_eq!(panel(PanelEdge::Bottom), [180.0, 300.0, 460.0]);
        });
    }

    /// A sheet's three rungs climb — on a window whose half actually falls
    /// between the other two. A 300pt sheet only has three distinct rungs
    /// in a window between 192 and 600 tall; taller than that and half the
    /// room is past the panel itself, which the clamp test below covers.
    #[test]
    fn the_sheet_rungs_climb() {
        crate::on_test_cx(|| {
        let base = DialogSize::Md.extent(PanelEdge::Bottom).unwrap();
        let room = 500.0;
        let peek = detent_extent(SheetDetent::Collapsed, base, room);
        let half = detent_extent(SheetDetent::Half, base, room);
        let full = detent_extent(SheetDetent::Expanded, base, room);
        assert!(peek < half && half < full, "{peek} {half} {full} must climb");
        assert_eq!(full, base, "expanded is what the panel's own size asks for");
        });
    }

    /// Half the room is clamped into the two rungs either side of it: on a
    /// short window half of it is less than a peek, and on a tall one it is
    /// more than the panel ever asked for.
    #[test]
    fn the_half_rung_is_clamped_into_the_ones_it_sits_between() {
        crate::on_test_cx(|| {
        let base = 300.0;
        assert_eq!(
            detent_extent(SheetDetent::Half, base, 60.0),
            SHEET_COLLAPSED,
            "a short window: half of it is below the peek, so the peek wins"
        );
        assert_eq!(
            detent_extent(SheetDetent::Half, base, 4000.0),
            base,
            "a tall window: half of it is past the panel, so the panel wins"
        );
        });
    }

    /// A panel smaller than a peek never claims to be taller than it is.
    #[test]
    fn a_panel_shorter_than_a_peek_is_still_only_itself() {
        crate::on_test_cx(|| {
        let base = 40.0;
        for rung in [SheetDetent::Collapsed, SheetDetent::Half, SheetDetent::Expanded] {
            assert!(
                detent_extent(rung, base, 900.0) <= base,
                "{rung:?} must not exceed the panel's own size"
            );
        }
        });
    }

    /// Letting go settles at the nearest rung.
    #[test]
    fn a_drag_settles_at_the_rung_it_is_nearest() {
        crate::on_test_cx(|| {
        let (peek, half, full) = (96.0, 450.0, 300.0_f64.max(450.0));
        assert_eq!(settle_detent(100.0, peek, half, full), Some(SheetDetent::Collapsed));
        assert_eq!(settle_detent(440.0, peek, half, full), Some(SheetDetent::Half));
        assert_eq!(settle_detent(peek, peek, half, full), Some(SheetDetent::Collapsed));
        });
    }

    /// Dragged most of the way down, letting go closes it rather than
    /// leaving a sliver on screen the pointer has already left.
    #[test]
    fn a_drag_below_the_lowest_rung_lets_the_sheet_go() {
        crate::on_test_cx(|| {
        assert_eq!(settle_detent(10.0, 96.0, 450.0, 600.0), None);
        assert_eq!(settle_detent(0.0, 96.0, 450.0, 600.0), None);
        assert!(
            settle_detent(48.0, 96.0, 450.0, 600.0).is_some(),
            "exactly at the threshold still settles: dismissing is the far side of it"
        );
        });
    }

    /// A page filled by a list-like button, the button that opens the
    /// drawer, and a drawer from the left with a row in its body.
    fn page(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    opener := Button{text: "open"}
                    under := Button{width: Fill height: Fill text: "under"}
                    drawer := Drawer{
                        content +: {
                            body +: {
                                row := Button{width: Fill height: 40. text: "row"}
                            }
                        }
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        })
    }

    /// Open, and put the panel in place without waiting out its slide.
    fn open_in_place(cx: &mut Cx, target: &mut Target, root: &WidgetRef, id: &[LiveId]) {
        let dialog = root.widget(cx, id);
        dialog.as_dialog().open(cx);
        if let Some(mut inner) = dialog.borrow_mut::<Dialog>() {
            inner.slide = 1.0;
            inner.sliding = false;
        }
        target.draw(cx, root);
    }

    /// The drawer and the two sheets are dialogs with a side chosen and no
    /// answers: the markup is read by nothing the compiler checks, so
    /// building them is what proves the presets say what they mean. A plain
    /// dialog stands in the middle, with its answers and no handle.
    #[test]
    fn a_drawer_is_a_dialog_with_a_side_and_no_answers() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Overlay
                    plain := Dialog{}
                    drawer := Drawer{}
                    side := SideSheet{}
                    sheet := BottomSheet{}
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        for (id, side, size, grabber) in [
            (ids!(plain), PanelEdge::Center, DialogSize::Sm, false),
            (ids!(drawer), PanelEdge::Left, DialogSize::Md, false),
            (ids!(side), PanelEdge::Right, DialogSize::Md, false),
            (ids!(sheet), PanelEdge::Bottom, DialogSize::Md, true),
        ] {
            let dialog = root.widget(&cx, id);
            {
                let inner = dialog.borrow::<Dialog>().unwrap_or_else(|| panic!("{id:?} is not a Dialog"));
                assert_eq!((inner.side, inner.size, inner.grabber), (side, size, grabber), "{id:?}");
                assert_eq!(inner.detent, SheetDetent::Expanded, "{id:?}");
                assert!(inner.dismissable, "{id:?}");
                assert_eq!(inner.confirm_text.is_empty(), side.is_edge(), "{id:?}: only a card asks");
            }
            open_in_place(&mut cx, &mut target, &root, id);
            let drawn = |name: &[LiveId]| {
                let rect = dialog.widget(&cx, name).area().rect(&cx);
                rect.size.x > 0.0 && rect.size.y > 0.0
            };
            assert_eq!(drawn(ids!(footer)), !side.is_edge(), "{id:?}: the row of answers");
            assert_eq!(drawn(ids!(confirm)), !side.is_edge(), "{id:?}: the default answer");
            assert_eq!(drawn(ids!(grab)), grabber, "{id:?}: the grabber");
            assert!(drawn(ids!(close)), "{id:?}: the close mark");
            let panel = dialog.widget(&cx, ids!(content)).area().rect(&cx);
            let want = match side {
                PanelEdge::Center => Rect { pos: dvec2(190.0, panel.pos.y), size: dvec2(420.0, panel.size.y) },
                PanelEdge::Left => Rect { pos: dvec2(0.0, 0.0), size: dvec2(360.0, 600.0) },
                PanelEdge::Right => Rect { pos: dvec2(440.0, 0.0), size: dvec2(360.0, 600.0) },
                PanelEdge::Top => Rect { pos: dvec2(0.0, 0.0), size: dvec2(800.0, 300.0) },
                PanelEdge::Bottom => Rect { pos: dvec2(0.0, 300.0), size: dvec2(800.0, 300.0) },
            };
            assert_eq!(panel, want, "{id:?} stands where its side puts it");
            if side == PanelEdge::Center {
                let middle = panel.pos.y + panel.size.y * 0.5;
                assert!((middle - 300.0).abs() < 1.0, "the card is centred, not at {middle}");
            }
            dialog.as_dialog().close(&mut cx);
            target.draw(&mut cx, &root);
        }
        });
    }

    /// The markup is read by nothing the compiler checks, and a mistake in
    /// it is only a line in the running app's log. Evaluated with that log
    /// held, the presets and the properties they are made of raise nothing.
    #[test]
    fn the_presets_and_their_properties_raise_no_script_error() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        cx.with_vm(|vm| {
            // Registering the library is every other widget's business too.
            let _ = vm.take_errors();
            vm.bx.captured_errors = Some(Vec::new());
            let _ = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    filters := Dialog{side: PanelEdge.Right size: Xl grabber: true detent: SheetDetent.Collapsed slide_secs: 0.2}
                    nav := Drawer{size: Xs content +: {body +: {Label{text: "a row"}}}}
                    details := SideSheet{size: Full}
                    sheet := BottomSheet{detent: SheetDetent.Half}
                    card := Dialog{side: PanelEdge.Center}
                }
            });
            let errors = vm.take_errors();
            assert!(errors.is_empty(), "{errors:#?}");
        });
        });
    }

    /// `side` alone puts a dialog on an edge: the alignment that centres a
    /// card does not move the panel, and the answers stay with it.
    #[test]
    fn a_dialog_given_a_side_stands_on_it_with_its_answers() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Overlay
                    filters := Dialog{side: PanelEdge.Right size: Lg confirm_text: "Apply" cancel_text: "Reset"}
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        open_in_place(&mut cx, &mut target, &root, ids!(filters));
        let dialog = root.widget(&cx, ids!(filters));
        let panel = dialog.widget(&cx, ids!(content)).area().rect(&cx);
        assert_eq!(panel, Rect { pos: dvec2(280.0, 0.0), size: dvec2(520.0, 600.0) });
        let confirm = dialog.widget(&cx, ids!(confirm)).area().rect(&cx);
        assert!(confirm.size.x > 0.0 && panel.contains(confirm.pos), "the answers are in the panel");

        let actions = cx.capture_actions(|cx| root.handle_event(cx, &key(KeyCode::ReturnKey), &mut Scope::empty()));
        assert_eq!(reports(&actions, &dialog), vec![DialogAction::Confirmed]);
        assert!(!dialog.as_dialog().is_open());
        });
    }

    /// Return answers a dialog that has a default answer and is left to the
    /// inside of one that has none: a list in a drawer chooses with it.
    #[test]
    fn return_is_left_to_a_panel_with_no_answer() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = page(&mut cx);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        open_in_place(&mut cx, &mut target, &root, ids!(drawer));
        let drawer = root.widget(&cx, ids!(drawer));
        for code in [KeyCode::ReturnKey, KeyCode::NumpadEnter] {
            let actions = cx.capture_actions(|cx| root.handle_event(cx, &key(code), &mut Scope::empty()));
            assert!(reports(&actions, &drawer).is_empty(), "{code:?} answered a drawer that asks nothing");
            assert!(drawer.as_dialog().is_open(), "{code:?} closed the drawer");
        }
        let actions = cx.capture_actions(|cx| root.handle_event(cx, &key(KeyCode::Escape), &mut Scope::empty()));
        assert_eq!(reports(&actions, &drawer), vec![DialogAction::Dismissed]);
        assert!(drawer.as_dialog().dismissed(&actions), "one report, so a lookup by widget finds it");
        });
    }

    /// A panel given no time to slide stands in place on the frame it
    /// opens; one with time starts at its edge.
    #[test]
    fn a_drawer_given_no_time_to_slide_is_in_place_at_once() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = page(&mut cx);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        let drawer = root.widget(&cx, ids!(drawer));
        for (secs, at) in [(SLIDE_FLOOR_SECS, 0.0), (0.0, 0.0), (0.3, -360.0)] {
            if let Some(mut inner) = drawer.borrow_mut::<Dialog>() {
                inner.slide_secs = secs;
            }
            let actions = cx.capture_actions(|cx| drawer.as_dialog().open(cx));
            assert_eq!(reports(&actions, &drawer), vec![DialogAction::Opened]);
            assert_eq!(drawer.as_dialog().answered(&actions), None, "coming up is not an answer");
            target.draw(&mut cx, &root);
            let panel = drawer.widget(&cx, ids!(content)).area().rect(&cx);
            assert_eq!(panel.pos.x, at, "a slide of {secs}s opens with the panel at {}", panel.pos.x);
            drawer.as_dialog().close(&mut cx);
            target.draw(&mut cx, &root);
        }
        });
    }

    /// The keyboard goes back to the button that opened the drawer, though
    /// the page was drawn again while the drawer was out.
    #[test]
    fn closing_gives_the_keyboard_back_to_the_opener_after_the_page_redraws() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = page(&mut cx);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        let opener = root.widget(&cx, ids!(opener));
        cx.set_key_focus(opener.area());
        settle_focus(&mut cx);
        let before = opener.area();

        open_in_place(&mut cx, &mut target, &root, ids!(drawer));
        settle_focus(&mut cx);
        assert!(!cx.has_key_focus(opener.area()), "the drawer took the keyboard");
        root.redraw(&mut cx);
        target.draw(&mut cx, &root);
        assert_ne!(opener.area(), before, "the page was drawn again");

        root.widget(&cx, ids!(drawer)).as_dialog().close(&mut cx);
        settle_focus(&mut cx);
        assert!(cx.has_key_focus(opener.area()), "the keyboard went to {:?}, not the opener {:?}", cx.key_focus(), opener.area());
        });
    }

    /// A press on a row in the drawer reaches the row, and a press on the
    /// scrim sends the drawer back, even when the list the drawer lies over
    /// is walked first; that list hears neither.
    #[test]
    fn a_press_in_the_drawer_never_reaches_the_list_under_it() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = page(&mut cx);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        open_in_place(&mut cx, &mut target, &root, ids!(drawer));
        let under = root.widget(&cx, ids!(under));
        let drawer = root.widget(&cx, ids!(drawer));
        let row = root.widget(&cx, ids!(row));
        let walk = |cx: &mut Cx, event: &Event| {
            cx.capture_actions(|cx| {
                under.handle_event(cx, event, &mut Scope::empty());
                drawer.handle_event(cx, event, &mut Scope::empty());
            })
        };

        let rect = row.area().rect(&cx);
        assert!(rect.size.x > 0.0, "the row is drawn");
        let on_row = rect.pos + rect.size * 0.5;
        assert!(under.area().rect(&cx).contains(on_row), "the row lies over the list");
        let actions = walk(&mut cx, &press(on_row));
        assert!(pressed(&actions, &row), "the row did not hear its press");
        assert!(!pressed(&actions, &under), "the list under the drawer took the press");
        walk(&mut cx, &release(on_row));
        assert!(drawer.as_dialog().is_open());

        let on_scrim = dvec2(700.0, 300.0);
        let actions = walk(&mut cx, &press(on_scrim));
        assert!(!pressed(&actions, &under), "the list under the scrim took the press");
        let actions = walk(&mut cx, &release(on_scrim));
        assert!(!drawer.as_dialog().is_open(), "a press on the scrim sends the drawer back");
        assert_eq!(reports(&actions, &drawer), vec![DialogAction::Dismissed], "and says so once");
        assert!(drawer.as_dialog().dismissed(&actions), "so a lookup by widget finds the drawer's own report");
        assert_eq!(cx.sweep_lock_area(), None, "and gives the pointer back");
        });
    }

    /// A press another control is holding is not the sheet's to use. The
    /// close mark takes the press and keeps it, so the grabber does not
    /// start a resize under it, and the release — far outside the card, as
    /// the end of a drag usually is — is not read as a press on the scrim.
    #[test]
    fn a_press_another_control_holds_neither_drags_the_sheet_nor_dismisses_it() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Overlay
                    sheet := BottomSheet{title: "Choices"}
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        open_in_place(&mut cx, &mut target, &root, ids!(sheet));
        let sheet = root.widget(&cx, ids!(sheet));
        let panel = |cx: &Cx| sheet.widget(cx, ids!(content)).area().rect(cx);
        let was = panel(&cx).size.y;

        // The close mark is a control inside the card: its press captures
        // the mouse and holds it until the release.
        let close = sheet.widget(&cx, ids!(close)).area();
        let close_rect = close.rect(&cx);
        assert!(close_rect.size.x > 0.0, "the close mark is drawn");
        sheet.handle_event(&mut cx, &press(close_rect.pos + close_rect.size * 0.5), &mut Scope::empty());
        assert!(cx.fingers.is_area_captured(close), "the close mark holds the mouse");

        // A press that reaches the grabber while it is held starts nothing.
        let grab = sheet.widget(&cx, ids!(grab)).area().rect(&cx);
        let held = grab.pos + grab.size * 0.5;
        sheet.handle_event(&mut cx, &press(held), &mut Scope::empty());
        sheet.handle_event(&mut cx, &drag(held + dvec2(0.0, 120.0)), &mut Scope::empty());
        target.draw(&mut cx, &root);
        assert_eq!(panel(&cx).size.y, was, "the panel stood still");

        // And letting go outside the card ends that drag, nothing else.
        let outside = dvec2(20.0, 20.0);
        assert!(!panel(&cx).contains(outside), "the release lands off the card");
        let actions = cx.capture_actions(|cx| sheet.handle_event(cx, &release(outside), &mut Scope::empty()));
        assert_eq!(reports(&actions, &sheet), vec![], "no answer of any kind");
        assert!(sheet.as_dialog().is_open(), "the sheet stayed up");
        });
    }

    /// A cancel that ends only some capture of a mouse press still held (a
    /// list recycled the row the press landed on) leaves the dialog's
    /// protection in place: the real release off the card, when it comes, is
    /// still not read as a press on the scrim.
    #[test]
    fn a_partial_cancel_keeps_a_card_press_from_dismissing_on_its_release() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Overlay
                    sheet := BottomSheet{title: "Choices"}
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        open_in_place(&mut cx, &mut target, &root, ids!(sheet));
        let sheet = root.widget(&cx, ids!(sheet));
        let card = sheet.widget(&cx, ids!(content)).area().rect(&cx);
        assert!(card.size.x > 0.0, "the card is drawn");
        sheet.handle_event(&mut cx, &press(card.pos + card.size * 0.5), &mut Scope::empty());
        // Some capture of the still-held press is cancelled — not the press.
        let mouse: crate::event::DigitId = live_id!(mouse).into();
        let partial = finger_cancel(&mut cx, mouse, crate::event::DigitDevice::Mouse { button: MouseButton::PRIMARY }, false);
        sheet.handle_event(&mut cx, &partial, &mut Scope::empty());
        let outside = dvec2(20.0, 20.0);
        assert!(!card.contains(outside));
        let actions = cx.capture_actions(|cx| sheet.handle_event(cx, &release(outside), &mut Scope::empty()));
        assert_eq!(reports(&actions, &sheet), vec![], "the card press's release dismissed the sheet");
        assert!(sheet.as_dialog().is_open());
        });
    }

    /// A `FingerCancel` for `digit`; with `taken_away` the press itself was
    /// cancelled first (`cancel_digit`), as a host or the OS does.
    fn finger_cancel(cx: &mut Cx, digit: crate::event::DigitId, device: crate::event::DigitDevice, taken_away: bool) -> Event {
        if taken_away {
            cx.fingers.cancel_digit(digit);
        }
        Event::FingerCancel(crate::event::FingerCancelEvent {
            window_id: WindowId(1, 1),
            digit_id: digit,
            device,
            abs: dvec2(1.0, 1.0),
            time: 1.0,
            modifiers: KeyModifiers::default(),
        })
    }

    /// A control that takes the pointer AFTER the drag started stops it
    /// there. The press on the grabber was the sheet's — nothing held the
    /// mouse then — but the rule is asked at the press and on every move,
    /// and the moment another control owns the pointer the resize lets go
    /// and the panel goes back to where the press found it.
    #[test]
    fn a_sheet_lets_go_when_another_control_takes_the_pointer_mid_drag() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Overlay
                    sheet := BottomSheet{title: "Choices"}
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        open_in_place(&mut cx, &mut target, &root, ids!(sheet));
        let sheet = root.widget(&cx, ids!(sheet));
        let panel = |cx: &Cx| sheet.widget(cx, ids!(content)).area().rect(cx);
        let was = panel(&cx).size.y;

        // A press nothing else is holding: the grabber takes it, and the
        // sheet follows the hand.
        let grab = sheet.widget(&cx, ids!(grab)).area().rect(&cx);
        let held = grab.pos + grab.size * 0.5;
        sheet.handle_event(&mut cx, &press(held), &mut Scope::empty());
        sheet.handle_event(&mut cx, &drag(held + dvec2(0.0, 60.0)), &mut Scope::empty());
        target.draw(&mut cx, &root);
        assert_eq!(panel(&cx).size.y, was - 60.0, "the drag started and the sheet moved");

        // Now a control inside the card takes the mouse.
        let close = sheet.widget(&cx, ids!(close)).area();
        let close_rect = close.rect(&cx);
        sheet.handle_event(&mut cx, &press(close_rect.pos + close_rect.size * 0.5), &mut Scope::empty());
        assert!(cx.fingers.is_area_captured(close), "the close mark holds the mouse");

        // From here the moves are not the sheet's, and the resize that was
        // under way is undone rather than carried on with.
        sheet.handle_event(&mut cx, &drag(held + dvec2(0.0, 120.0)), &mut Scope::empty());
        target.draw(&mut cx, &root);
        assert_eq!(panel(&cx).size.y, was, "the sheet let go and went back");
        sheet.handle_event(&mut cx, &drag(held + dvec2(0.0, 180.0)), &mut Scope::empty());
        target.draw(&mut cx, &root);
        assert_eq!(panel(&cx).size.y, was, "and stays put for the rest of the drag");

        // And letting go settles nothing: there was no drag left to end.
        let actions = cx.capture_actions(|cx| {
            sheet.handle_event(cx, &release(held + dvec2(0.0, 180.0)), &mut Scope::empty())
        });
        assert_eq!(reports(&actions, &sheet), vec![], "no rung was chosen by a pointer we did not own");
        assert!(sheet.as_dialog().is_open(), "and the sheet is still up");
        });
    }

    /// The grabber is a handle: the sheet follows it, settles at the rung
    /// it was let go nearest, says which, and goes back when it is pulled
    /// below the lowest one.
    #[test]
    fn a_sheet_follows_its_grabber_and_settles_on_a_rung() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Overlay
                    sheet := BottomSheet{title: "Choices"}
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        open_in_place(&mut cx, &mut target, &root, ids!(sheet));
        let sheet = root.widget(&cx, ids!(sheet));
        let panel = |cx: &Cx| sheet.widget(cx, ids!(content)).area().rect(cx);
        assert_eq!(panel(&cx), Rect { pos: dvec2(0.0, 300.0), size: dvec2(800.0, 300.0) }, "open at its full size");

        let grab = sheet.widget(&cx, ids!(grab)).area().rect(&cx);
        assert!(grab.size.x > 0.0, "the grabber is drawn");
        let held = grab.pos + grab.size * 0.5;
        sheet.handle_event(&mut cx, &press(held), &mut Scope::empty());
        sheet.handle_event(&mut cx, &drag(held + dvec2(0.0, 120.0)), &mut Scope::empty());
        target.draw(&mut cx, &root);
        assert_eq!(panel(&cx).size.y, 180.0, "the panel follows the hand");

        // 180 is nearer the peek at 96 than the full 300, which is also
        // where half of a 600 window clamps to.
        let actions = cx.capture_actions(|cx| sheet.handle_event(cx, &release(held + dvec2(0.0, 120.0)), &mut Scope::empty()));
        assert_eq!(reports(&actions, &sheet), vec![DialogAction::DetentChanged(SheetDetent::Collapsed)]);
        assert_eq!(sheet.as_dialog().detent_changed(&actions), Some(SheetDetent::Collapsed));
        assert_eq!(sheet.as_dialog().detent(), SheetDetent::Collapsed);
        assert!(sheet.as_dialog().is_open(), "letting go of the grabber is not a press on the scrim");
        target.draw(&mut cx, &root);
        assert_eq!(panel(&cx).size.y, SHEET_COLLAPSED);

        sheet.as_dialog().set_detent(&mut cx, SheetDetent::Expanded);
        target.draw(&mut cx, &root);
        assert_eq!(panel(&cx).size.y, 300.0, "a rung set by hand is stood at");

        let grab = sheet.widget(&cx, ids!(grab)).area().rect(&cx);
        let held = grab.pos + grab.size * 0.5;
        sheet.handle_event(&mut cx, &press(held), &mut Scope::empty());
        sheet.handle_event(&mut cx, &drag(held + dvec2(0.0, 280.0)), &mut Scope::empty());
        let actions = cx.capture_actions(|cx| sheet.handle_event(cx, &release(held + dvec2(0.0, 280.0)), &mut Scope::empty()));
        assert_eq!(reports(&actions, &sheet), vec![DialogAction::Dismissed]);
        assert!(!sheet.as_dialog().is_open(), "pulled below the peek, the sheet goes back");
        });
    }
}
