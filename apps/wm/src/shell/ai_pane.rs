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
//! ground with a 2 px edge on its RIGHT. Under a glass material the card
//! is framed like a tile instead — the desk's ring, the material's
//! shadow, the child clipped to the same corners (`PaneFrame`) — so the
//! pane reads as one more window on that desk. Open, the pane takes the pointer
//! inside its card and the keyboard; closed, the strip is zero and the
//! card sits just off the desk's left edge, still drawn and ticking (the
//! child's 8 ms tick, the module's engine), so the assistant — started
//! with the desktop, never on the first F10 — is configured, presenting
//! and answering before the pane ever slides in, and a turn in flight
//! finishes behind a hidden one.

use crate::desk::{ease_out_quint, snap_child_rect, snap_to_device, tile_frame, BorderTheme, DrawTileBorder, DrawTilePanel};
use crate::hub::ClientId;
use crate::run_view::MpRunView;
use crate::shell::{DeskTokens, MaterialTokens, ShellTokens};
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
        // The glass frame's ground: the card colour, opaque, rounded.
        draw_ground +: { color: mod.wm_theme.background alpha: 1.0 }
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
/// The card's frame under a glass material: the numbers a tile gets from
/// the same desk (`desk::tile_frame`), so the pane and the tiles beside it
/// share one ring, one radius and one shadow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneFrame {
    /// The ring's thickness: the desk's border size.
    pub inset: f64,
    /// The ring's Sdf2d half-radius.
    pub ring_half: f32,
    /// The child's clip half-radius, concentric inside the ring.
    pub child_half: f32,
    /// Whether the material casts the shadow.
    pub shadow: bool,
}

impl PaneFrame {
    /// `None` under a flat material: the card keeps its docked look, an
    /// opaque ground with the accent edge on its right.
    pub fn for_theme(desk: &DeskTokens, material: &MaterialTokens) -> Option<PaneFrame> {
        if !material.is_glass() {
            return None;
        }
        let (ring_half, child_half, shadow) = tile_frame(desk, material);
        Some(PaneFrame { inset: desk.border_size, ring_half, child_half, shadow })
    }
}

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
    /// Under a glass material: the tile ring and the material's shadow.
    #[live]
    draw_frame: DrawTileBorder,
    /// Under a glass material: the rounded ground under the child.
    #[live]
    draw_ground: DrawTilePanel,
    /// The theme's desk and material tokens, as the WM last pushed them.
    #[rust]
    desk: DeskTokens,
    #[rust]
    material: MaterialTokens,
    /// The ring's colours: a focused tile's, since an open pane holds
    /// the keyboard.
    #[rust]
    ring: BorderTheme,
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
    /// AiChatOverlay{}` exists when this build links `makepad-aichat` and
    /// called its `script_mod`. False, with one log line, when it does not.
    pub fn ensure_overlay(&mut self, cx: &mut Cx) -> bool {
        if self.overlay.is_some() {
            return true;
        }
        let overlay = cx.with_vm(|vm| {
            let widgets = vm.module(id!(widgets));
            let ty = vm.bx.heap.value_path(widgets, ids!(AiChatOverlay), NoTrap);
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

    /// A live theme switch (and the startup push): the card, its right
    /// edge, and the tokens the glass frame is drawn from.
    pub fn set_theme(&mut self, card: Vec4f, edge: Vec4f, tokens: &ShellTokens, borders: &BorderTheme) {
        self.draw_card.color = card;
        self.draw_edge.color = edge;
        self.draw_ground.color = card;
        self.desk = tokens.desk;
        self.material = tokens.material;
        self.ring = *borders;
    }

    /// The card as a tile: the rounded ground, the ring with the
    /// material's shadow, and the child clipped concentrically inside —
    /// snapped to device pixels the way a tile is, so the ring is whole
    /// pixels on every side. Returns the child's rect.
    fn draw_framed_card(&mut self, cx: &mut Cx2d, r: Rect, frame: PaneFrame) -> Rect {
        let dpi = cx.current_dpi_factor().max(1.0);
        let inset = frame.inset;
        let r = snap_to_device(r, dpi, inset);
        let inner = snap_child_rect(
            Rect {
                pos: r.pos + dvec2(inset, inset),
                size: dvec2((r.size.x - inset * 2.0).max(1.0), (r.size.y - inset * 2.0).max(1.0)),
            },
            dpi,
        );
        self.draw_ground.radius = frame.child_half;
        self.draw_ground.draw_abs(cx, inner);
        let m = self.material;
        self.draw_frame.color = self.ring.active;
        self.draw_frame.color_end = self.ring.active_end;
        self.draw_frame.angle = self.ring.angle;
        self.draw_frame.border_size = inset as f32;
        self.draw_frame.corner_radius = frame.ring_half;
        self.draw_frame.shadow_color = Vec4f { w: if frame.shadow { m.shadow_alpha } else { 0.0 }, ..m.shadow_color };
        self.draw_frame.shadow_radius = if frame.shadow { m.shadow_radius as f32 } else { 0.0 };
        self.draw_frame.shadow_offset_y = if frame.shadow { m.shadow_offset_y as f32 } else { 0.0 };
        self.draw_frame.draw_abs(cx, r);
        self.set_child_radius(cx, frame.child_half);
        inner
    }

    /// The child's clip radius: the frame's under glass, none when flat.
    fn set_child_radius(&mut self, cx: &mut Cx, radius: f32) {
        self.with_run_view(cx, |_, view| view.set_corner_radius(radius));
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
        self.anim_started = Some((cx.seconds_since_app_start(), self.t, open));
        self.next_frame = cx.new_next_frame();
        if !open {
            // A hidden pane must not keep the keyboard: typing would go on
            // reaching the chat behind nothing.
            self.focus_pending = false;
            if self.overlay.is_some() {
                cx.set_key_focus(Area::Empty);
            } else {
                self.with_run_view(cx, |cx, v| v.release_keyboard(cx));
            }
        }
        self.redraw(cx);
    }

    /// The child is gone: no client, no picture, pane shut.
    pub fn reset(&mut self, cx: &mut Cx) {
        self.client = None;
        self.with_run_view(cx, |cx, v| v.clear_run_target(cx));
        self.set_open(cx, false);
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
        // The body is ticked whether or not the pane shows (the child's
        // 8 ms tick timer is what makes it run at all; the module's engine
        // pumps on every event); pointer and keys reach it only while the
        // pane is on screen, so a hidden pane's stale area can never
        // swallow a click meant for a tile.
        if let Some(overlay) = self.overlay.clone() {
            let pointer = matches!(event, Event::MouseMove(_) | Event::MouseDown(_) | Event::MouseUp(_) | Event::Scroll(_));
            let keys = matches!(event, Event::KeyDown(_) | Event::KeyUp(_) | Event::TextInput(_));
            if (!pointer && !keys) || self.showing() {
                overlay.handle_event(cx, event, scope);
            }
            return;
        }
        if self.showing() || matches!(event, Event::Timer(_)) {
            self.view.handle_event(cx, event, scope);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // The strip is this widget's walk: `walk` arrives with its width,
        // the desk row lays the desk out after it. The card is drawn
        // right-aligned in the strip — off the row's left edge while
        // hidden, where a child process still keeps a real rect (so it is
        // configured, gets a swapchain and presents its frames) and a
        // module root keeps a live picture: the first slide-in shows the
        // assistant already running, not a "starting…" wash.
        cx.begin_turtle(walk, Layout::default());
        let r = self.card_rect();
        let inner = match PaneFrame::for_theme(&self.desk, &self.material) {
            Some(frame) => self.draw_framed_card(cx, r, frame),
            None => {
                self.draw_card.draw_abs(cx, r);
                self.draw_edge.draw_abs(
                    cx,
                    Rect { pos: dvec2(r.pos.x + r.size.x - EDGE, r.pos.y), size: dvec2(EDGE, r.size.y) },
                );
                self.set_child_radius(cx, 0.0);
                Rect { pos: r.pos, size: dvec2(r.size.x - EDGE, r.size.y) }
            }
        };
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
    fn the_pane_frames_like_a_tile_only_under_glass() {
        let desk = DeskTokens::default();
        let flat = MaterialTokens::default();
        assert_eq!(PaneFrame::for_theme(&desk, &flat), None, "flat keeps the docked card and its edge");
        let glass = MaterialTokens { glass: 1.0, ..MaterialTokens::default() };
        let desk = DeskTokens { corner_radius: 12.0, border_size: 1.5, ..DeskTokens::default() };
        assert_eq!(
            PaneFrame::for_theme(&desk, &glass),
            Some(PaneFrame { inset: 1.5, ring_half: 6.0, child_half: 5.25, shadow: true }),
            "the same numbers a tile gets under this desk"
        );
        // A glass material on a square desk: a square ring, no shadow —
        // the tile rule, so the pane and the tiles never disagree.
        let square = DeskTokens::default();
        assert_eq!(
            PaneFrame::for_theme(&square, &glass),
            Some(PaneFrame { inset: 2.0, ring_half: 0.0, child_half: 0.0, shadow: false })
        );
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
