//! Input and gestures for parked windows.
//!
//! The pointer: a parked window is drawn small, but its client lays out at
//! the full desk rect. [`Router::route`] gives each window the event as it
//! must see it: the window under the pointer (or the one that took the
//! press) gets `abs` inverse-mapped through its scaled rect into its desk
//! rect, every window that must not react gets an `abs` far outside the
//! desk (hover-out, never a claim). Same pattern as `vr_pointer_event`.
//!
//! The gestures (`impl App`): drag with the scale following the position,
//! tidy on drop, swap to the centre, park with the keyboard, toggle.
use super::{desk_with_scale, inward_of, restore_inside, shelf, Leave, Mode, into_band, pose_in, next_parked, slot_at, tidy, Item, Warp, Zones, PARK_BELOW};
use makepad_app_host::ClientId;
use crate::layout::LRect;
use crate::{host, App, DragState};
use makepad_widgets::*;

/// Where a window that must not see the pointer is told it is.
const FAR: Vec2d = Vec2d { x: -1.0e7, y: -1.0e7 };

#[derive(Default)]
pub struct Router {
    /// The window a press went to, until the button lifts.
    capture: Option<ClientId>,
}

fn abs_of(event: &Event) -> Option<Vec2d> {
    match event {
        Event::MouseDown(e) => Some(e.abs),
        Event::MouseMove(e) => Some(e.abs),
        Event::MouseUp(e) => Some(e.abs),
        Event::Scroll(e) => Some(e.abs),
        Event::Pinch(e) => Some(e.abs),
        _ => None,
    }
}

/// A copy of a pointer event at `abs` (events themselves are not Clone).
fn with_abs(event: &Event, abs: Vec2d) -> Option<Event> {
    Some(match event {
        Event::MouseDown(e) => { let mut e = e.clone(); e.abs = abs; Event::MouseDown(e) }
        Event::MouseMove(e) => { let mut e = e.clone(); e.abs = abs; Event::MouseMove(e) }
        Event::MouseUp(e) => { let mut e = e.clone(); e.abs = abs; Event::MouseUp(e) }
        Event::Scroll(e) => { let mut e = e.clone(); e.abs = abs; Event::Scroll(e) }
        Event::Pinch(e) => { let mut e = e.clone(); e.abs = abs; Event::Pinch(e) }
        _ => return None,
    })
}

impl Router {
    /// The event per window, topmost first, or None when nothing is parked
    /// (the unwarped path: the event goes to the tiles untouched).
    /// `order` is the visible windows' desk rects, topmost first.
    pub fn route(&mut self, warp: &Warp, event: &Event, order: &[(ClientId, Rect)]) -> Option<Vec<(ClientId, Event)>> {
        let p = abs_of(event)?;
        if !warp.active() && self.capture.is_none() {
            return None;
        }
        let poses: Vec<_> = order
            .iter()
            .map(|(c, r)| warp.pose_for(*c, LRect::new(r.pos.x, r.pos.y, r.size.x, r.size.y)))
            .collect();
        if self.capture.is_none() && !poses.iter().any(|p| p.displaced()) {
            return None;
        }
        // A press starts afresh: a release the WM took for itself (the end
        // of a window drag) never reached here, so the capture may be stale.
        let held = self.capture.filter(|_| !matches!(event, Event::MouseDown(_)));
        let owner = held
            .filter(|c| order.iter().any(|(o, _)| o == c))
            .or_else(|| order.iter().zip(&poses).find(|(_, pose)| pose.contains(p.x, p.y)).map(|((c, _), _)| *c));
        if matches!(event, Event::MouseDown(_)) {
            self.capture = owner;
        } else if matches!(event, Event::MouseUp(_)) {
            self.capture = None;
        }
        // A tiny shelf tile takes no input: only its unfolded half size does.
        let blocked = owner.is_some_and(|o| {
            order.iter().zip(&poses).any(|((c, _), pose)| *c == o && pose.side != 0 && pose.displaced() && !warp.is_unfolded(o))
        });
        // Only the owner sees the pointer. Each window gets its own copy of the
        // event, so the first-wins claim (`Event::hits`, `handled`) is not shared
        // between them: a window under the owner that saw `p` too claimed the
        // same press, and its click (focus, raise) landed after the owner's.
        Some(
            order
                .iter()
                .zip(&poses)
                .map(|((client, rect), pose)| {
                    let at = if Some(*client) == owner && !blocked {
                        let desk = warp.placed(*client, LRect::new(rect.pos.x, rect.pos.y, rect.size.x, rect.size.y));
                        let (x, y) = pose.unmap(desk, p.x, p.y);
                        dvec2(x, y)
                    } else {
                        FAR
                    };
                    with_abs(event, at).map(|ev| (*client, ev))
                })
                .collect::<Option<Vec<_>>>()?,
        )
    }
}

impl App {
    /// Do the screen edges snap windows (the Classic desktop of the
    /// window-snapping style)?
    pub(crate) fn snap_zones_on(&mut self) -> bool {
        let state = self.state_mut();
        crate::snap::zones_enabled(state.style.target, state.warp.mode)
    }

    /// Warp or Classic. Classic: the parked windows grow out of their slots
    /// back to full size inside the display (the desk's move tween), the
    /// zones ease out and the plain background takes over. Warp: the
    /// windows stay where they are, the zones ease in under the warp grid.
    pub(crate) fn set_desktop_mode(&mut self, cx: &mut Cx, mode: Mode) {
        self.close_shell_menu(cx);
        if self.state_mut().warp.mode == mode {
            return;
        }
        if mode == Mode::Classic {
            self.unpark_all(cx);
        }
        let state = self.state_mut();
        state.warp.mode = mode;
        state.warp.cycled.clear();
        state.snap.clear();
        host::persist_desktop_mode(cx, mode);
        // Warp wears its grid at once; Classic keeps the grid until the zones
        // have flattened (the desk then swaps in the plain background), unless
        // another background is up.
        if mode == Mode::Warp || self.state_mut().backdrop.kind != Some(crate::backdrop::Kind::WarpGrid) {
            self.background_index = 0;
            self.apply_background(cx, 0);
        }
        self.redraw_all(cx);
    }

    /// Every parked window back full size inside its display (Classic, or the desktop
    /// going to a tiling style, which has no shelves). Call while the shelves are still on:
    /// the zones say where each parked window is shown.
    pub(crate) fn unpark_all(&mut self, cx: &mut Cx) {
        if let Some(mut desk) = self.desk(cx).borrow_mut::<crate::desk::WmDesk>() {
            desk.begin_unpark();
        }
        let gap = self.state_mut().gap;
        for screen in 0..self.state_mut().screens.live_count() {
            let Some(z) = self.warp_zones(cx, screen) else { continue };
            let area = self.screen_area(cx, screen);
            let layout = self.state_mut().layout_at(screen);
            let floats: Vec<(ClientId, LRect)> =
                layout.rects(area, gap).into_iter().filter(|(c, _)| layout.is_float(*c) && !layout.is_client_fullscreen(*c)).collect();
            for (c, r) in floats {
                let back = restore_inside(&z, r);
                if back != r {
                    self.state_mut().layout_at_mut(screen).set_float_rect(c, back);
                }
                self.warp_flag(c, false);
            }
        }
    }

    /// The saved behaviour, read from the storage (the web's restore).
    pub(crate) fn desktop_mode_response(&mut self, cx: &mut Cx, responses: &[StorageResponse]) {
        let Some(id) = self.desktop_mode_request else { return };
        let Some(response) = responses.iter().find(|r| r.request_id == id) else { return };
        self.desktop_mode_request = None;
        if self.state.is_none() {
            return;
        }
        if let Ok(StorageResult::Value(Some(bytes))) = &response.result {
            if let Some(mode) = std::str::from_utf8(bytes).ok().and_then(Mode::from_name) {
                self.set_desktop_mode(cx, mode);
            }
        }
    }

    fn warp_zones(&self, cx: &mut Cx, screen: usize) -> Option<Zones> {
        let area = self.screen_area(cx, screen);
        self.state.as_ref()?.warp.zones_for(area)
    }

    pub(crate) fn warp_flag(&mut self, client: ClientId, on: bool) {
        if let Some(w) = self.state_mut().layout_of_mut(client).desktop.get_mut(client) {
            w.warped = on;
        }
    }

    /// A float's desk rect and the zones of its screen.
    fn warp_rect(&mut self, cx: &mut Cx, client: ClientId) -> Option<(usize, Zones, LRect)> {
        let screen = self.state_mut().screens.screen_of(client)?;
        let z = self.warp_zones(cx, screen)?;
        let area = self.screen_area(cx, screen);
        let gap = self.state_mut().gap;
        let layout = self.state_mut().layout_at(screen);
        let rect = layout.rect_of(client, area, gap).filter(|_| layout.is_float(client))?;
        Some((screen, z, rect))
    }

    /// Is this float parked in a zone of its display?
    pub(crate) fn warp_is_parked(&mut self, _cx: &mut Cx, client: ClientId) -> bool {
        self.state_mut().layout_of(client).desktop.get(client).is_some_and(|w| w.warped)
    }

    /// The parked window under a SUPER press or a title press, by what is
    /// SHOWN (a parked window's desk rect covers far more than its scaled rect).
    pub(crate) fn warp_window_at(&mut self, cx: &mut Cx, abs: Vec2d) -> Option<ClientId> {
        let desk = self.desk(cx);
        let desk = desk.borrow::<crate::desk::WmDesk>()?;
        if !desk.warp.active() {
            return None;
        }
        desk.window_at(abs).filter(|c| desk.warp_pose(*c).is_some_and(|p| p.displaced()))
    }

    /// The tiny tile takes no input for its app. A press on one belongs to
    /// the WM: a drag begins at once and the press is kept; released without
    /// the pointer having moved the slop it was a click and the window
    /// unfolds (`warp_unfold`), moved further it pulls the window out. The
    /// unfolded half size is not this: its presses go to the app. True when
    /// the press was taken.
    pub(crate) fn warp_shelf_press(&mut self, cx: &mut Cx, event: &Event) -> bool {
        let Event::MouseDown(e) = event else { return false };
        if !e.button.contains(MouseButton::PRIMARY) || crate::super_chord(&e.modifiers) {
            return false;
        }
        let hit = self.warp_window_at(cx, e.abs);
        let Some(client) = hit else { return false };
        let tile = self.desk(cx).borrow::<crate::desk::WmDesk>().is_some_and(|d| {
            d.warp_pose(client).is_some_and(|p| p.side != 0) && !d.warp.is_unfolded(client)
        });
        if !tile || !self.begin_drag_on(cx, client, e.abs, false, false) {
            return false;
        }
        self.state_mut().warp.shelf_press = Some((client, e.clone()));
        true
    }

    /// A click on a tiny tile: it unfolds to its half size in the shelf.
    pub(crate) fn warp_unfold(&mut self, cx: &mut Cx, client: ClientId) {
        let Some(screen) = self.state_mut().screens.screen_of(client) else { return };
        let Some(z) = self.warp_zones(cx, screen) else { return };
        let pose = self.desk(cx).borrow::<crate::desk::WmDesk>().and_then(|d| d.warp_pose(client));
        let Some(side) = pose.map(|p| p.side).filter(|s| *s != 0) else { return };
        let reserve = self.state_mut().dock_clear_height();
        let app = self.state_mut().clients.get(&client).map(|c| c.app.clone()).unwrap_or_default();
        let half = self.state_mut().warp.half_heights.get(&app).copied().unwrap_or(z.screen.h * shelf::HALF_SHARE);
        if let Some(mut d) = self.desk(cx).borrow_mut::<crate::desk::WmDesk>() {
            d.unfold(client, side, z.screen, half, reserve);
        }
        self.shelf_autohide_refresh(cx);
        self.redraw_all(cx);
    }

    /// Fold the unfolded window back to its tile.
    pub(crate) fn warp_fold(&mut self, cx: &mut Cx) {
        let open = self.desk(cx).borrow::<crate::desk::WmDesk>().is_some_and(|d| d.warp.unfold.is_some_and(|u| u.open()));
        if !open {
            return;
        }
        if let Some(mut d) = self.desk(cx).borrow_mut::<crate::desk::WmDesk>() {
            d.fold();
        }
        self.shelf_autohide_refresh(cx);
        self.redraw_all(cx);
    }

    /// A press anywhere but on the unfolded window folds it (the press then
    /// goes on to whatever it hit: another tile unfolds that one, a window
    /// or the desktop takes it as usual).
    pub(crate) fn warp_fold_on_press(&mut self, cx: &mut Cx, event: &Event) {
        let Event::MouseDown(e) = event else { return };
        let inside = self.desk(cx).borrow::<crate::desk::WmDesk>().and_then(|d| d.warp.unfolded_rect()).map(|(_, r)| r);
        let Some(r) = inside else { return };
        let on = e.abs.x >= r.x && e.abs.x <= r.x + r.w && e.abs.y >= r.y && e.abs.y <= r.y + r.h;
        if !on {
            self.warp_fold(cx);
        }
    }

    /// The half window's resize: a press on its inner edge or bottom
    /// corner (the shelf side's mirror) begins one, the pointer drives the
    /// height live (the neighbours follow), the release remembers it for the
    /// app. True when the event is the resize's. Hover over the grip shows
    /// the resize cursor and does not reach the app.
    pub(crate) fn warp_unfold_resize(&mut self, cx: &mut Cx, event: &Event) -> bool {
        let Some((u, rect)) = self.desk(cx).borrow::<crate::desk::WmDesk>().and_then(|d| {
            let u = d.warp.unfold.filter(|u| u.open())?;
            d.warp.unfolded_rect().map(|(_, r)| (u, r))
        }) else { return false };
        let resizing = self.state_mut().warp.unfold_resize;
        match event {
            Event::MouseDown(e) if e.button.contains(MouseButton::PRIMARY) => {
                // An app that draws its own caption may report no drag area
                // there: the slim strip along the top drags the window anyway.
                let own = self.desk(cx).borrow::<crate::desk::WmDesk>().is_some_and(|d| d.draws_own_chrome(u.client));
                if own && shelf::on_caption_strip(rect, (e.abs.x, e.abs.y)) && shelf::resize_grab(u.side, rect, (e.abs.x, e.abs.y)).is_none() {
                    return self.begin_drag_on(cx, u.client, e.abs, false, false);
                }
                if let Some(corner) = shelf::resize_grab(u.side, rect, (e.abs.x, e.abs.y)) {
                    self.state_mut().warp.unfold_resize = Some((corner, (e.abs.x, e.abs.y), u.half_h));
                    return true;
                }
                false
            }
            Event::MouseMove(e) => {
                if let Some((corner, start, start_h)) = resizing {
                    let aspect = rect.w / rect.h.max(1.0);
                    let h = shelf::resized_h(start_h, aspect, u.side, e.abs.x - start.0, e.abs.y - start.1, corner);
                    if let Some(mut d) = self.desk(cx).borrow_mut::<crate::desk::WmDesk>() {
                        if let Some(u) = &mut d.warp.unfold {
                            u.half_h = h;
                        }
                    }
                    self.set_resize_cursor(cx, u.side, corner);
                    self.redraw_all(cx);
                    return true;
                }
                if let Some(corner) = shelf::resize_grab(u.side, rect, (e.abs.x, e.abs.y)) {
                    self.set_resize_cursor(cx, u.side, corner);
                    return true;
                }
                false
            }
            Event::MouseUp(_) if resizing.is_some() => {
                self.state_mut().warp.unfold_resize = None;
                // What it came to (clamped) is the app's half size from now on.
                let shown = self.desk(cx).borrow::<crate::desk::WmDesk>().and_then(|d| d.warp.unfolded_rect()).map(|(_, r)| r.h);
                if let Some(h) = shown {
                    let app = self.state_mut().clients.get(&u.client).map(|c| c.app.clone()).unwrap_or_default();
                    self.state_mut().warp.half_heights.insert(app, h);
                    let all = self.state_mut().warp.half_heights.clone();
                    host::persist_shelf_half(cx, &all);
                    if let Some(mut d) = self.desk(cx).borrow_mut::<crate::desk::WmDesk>() {
                        if let Some(u) = &mut d.warp.unfold {
                            u.half_h = h;
                        }
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn set_resize_cursor(&mut self, cx: &mut Cx, side: i8, corner: bool) {
        cx.set_cursor(match (corner, side < 0) {
            (false, _) => MouseCursor::EwResize,
            (true, true) => MouseCursor::NwseResize,
            (true, false) => MouseCursor::NeswResize,
        });
    }

    /// Drag move: the desk rect that keeps the grabbed pixel under the
    /// cursor while the scale follows the position. `plain` is the ordinary
    /// result, used when spacewarp is off for the screen.
    pub(crate) fn warp_drag_rect(
        &mut self,
        cx: &mut Cx,
        screen: usize,
        client: ClientId,
        start_rect: LRect,
        start: Vec2d,
        abs: Vec2d,
        plain: LRect,
    ) -> LRect {
        let Some(z) = self.warp_zones(cx, screen) else { return plain };
        let g = self.state_mut().warp.grab_start(client, (start.x, start.y), pose_in(&z, start_rect).shown);
        let start = dvec2(g.0, g.1);
        // A drag that began on a shelf item pulls the window out once the
        // pointer is a short way from the shelf, wherever on the display
        // that is: the window then grows under the pointer and lands where
        // it is let go. Over the other shelf it parks there instead.
        let start_pose = pose_in(&z, start_rect);
        let prior = self.state_mut().warp.leave.filter(|l| l.client == client);
        let began_parked = self.warp_is_parked(cx, client) || prior.is_some();
        let side = start_pose.side();
        if began_parked {
            // Pulled off its shelf: the window grows from the size it had to
            // full size by how far the pointer is from the shelf's inner
            // edge, scaled about the grab point so it stays under the pointer;
            // dragged back toward the shelf it shrinks the same way.
            let inner = self.shelf_inner_edge(cx, screen, side, client, start_rect).unwrap_or_else(|| shelf::inner_edge(side, [start_pose.shown]).unwrap_or(abs.x));
            // What the person sees as the tile: its compact or scaled pose.
            let sh = self.desk(cx).borrow::<crate::desk::WmDesk>().and_then(|d| d.warp_pose(client)).map_or(start_pose.shown, |p| p.shown);
            let press = self.state_mut().warp.press_pose.filter(|(c, _)| *c == client).map(|(_, r)| r);
            let mut l = prior.unwrap_or(Leave {
                client,
                side,
                s0: (press.unwrap_or(sh).w / start_rect.w.max(1.0)).clamp(0.05, 1.0),
                // Growth starts from where the pointer is when it is already past the
                // shelf's edge (a grab inside a half window): no step at the first move.
                from: inward_of(&z, side, inner).max(inward_of(&z, side, abs.x)),
                fx: ((start.x - press.unwrap_or(sh).x) / press.unwrap_or(sh).w.max(1.0)).clamp(0.0, 1.0),
                fy: ((start.y - press.unwrap_or(sh).y) / press.unwrap_or(sh).h.max(1.0)).clamp(0.0, 1.0),
                size: (start_rect.w, start_rect.h),
                cursor: (abs.x, abs.y),
            });
            l.cursor = (abs.x, abs.y);
            self.warp_flag(client, false);
            if l.growth(&z) >= 1.0 - 1e-9 {
                // Fully out: from here it is a normal window drag, rebased on
                // the window as it is under the pointer now (the shelf preview
                // may shrink it again; the pull-out's job is done).
                let r = l.desk(&z);
                self.state_mut().warp.leave = None;
                self.state_mut().warp.press_pose = None;
                if let Some(d) = self.drag.as_mut() {
                    d.start = abs;
                    d.start_rect = r;
                }
                return r;
            }
            self.state_mut().warp.leave = Some(l);
            return l.desk(&z);
        }
        // Any other drag is a normal window drag: full size under the pointer,
        // no warp, wherever it goes (the shelf preview is `warp_edge`'s).
        self.state_mut().warp.leave = None;
        self.warp_flag(client, false);
        plain
    }

    /// A window drag near an OPEN shelf: it shrinks continuously toward its
    /// slot by the pointer's distance from the shelf, about the grab point
    /// (`Enter`). A closed (auto-hidden) shelf only opens, by the pointer
    /// touching its screen edge (`shelf_autohide_refresh`); the size does not
    /// change until it is open. True when the preview changed.
    pub(crate) fn warp_edge(&mut self, cx: &mut Cx, abs: Vec2d) -> bool {
        let Some(drag) = self.drag.as_ref() else { return false };
        if drag.resize || !drag.armed {
            return false;
        }
        let (client, screen, start, start_rect) = (drag.client, drag.screen, drag.start, drag.start_rect);
        let Some(z) = self.warp_zones(cx, screen) else { return false };
        if self.state_mut().warp.leave.is_some_and(|l| l.client == client) || self.state_mut().screen_under(abs.x, abs.y) != screen {
            return false;
        }
        let a = z.screen;
        let side: i8 = if abs.x < a.x + a.w * 0.5 { -1 } else { 1 };
        let k = (side > 0) as usize;
        let hidden = self.state_mut().shelf_autohide.hidden(screen)[k];
        let reach = self.desk(cx).borrow::<crate::desk::WmDesk>().map_or([0.0; 2], |d| d.warp.reach_at((a.x + a.w * 0.5, a.y + a.h * 0.5)))[k];
        let reach = if reach > 0.0 { reach } else { z.zone_w() * 0.3 };
        let from_edge = if side < 0 { abs.x - a.x } else { a.x + a.w - abs.x };
        // The scale lives inside the shelf's zone only: none at its outer boundary.
        let span = (z.zone_w() - reach).max(60.0);
        // A closed (auto-hidden) shelf opens by the pointer touching its edge, and
        // the window starts to shrink with it; an open one scales continuously.
        let target = if abs.y < a.y || abs.y > a.y + a.h {
            0.0
        } else if hidden > 0.02 {
            if from_edge <= shelf::EDGE_PX + 2.0 { 1.0 } else { 0.0 }
        } else {
            super::enter_progress(from_edge - reach, span)
        };
        let warp = &mut self.state_mut().warp;
        match &mut warp.enter {
            Some(en) if en.client == client && en.side == side => {
                en.cursor = (abs.x, abs.y);
                en.target = target;
                return true;
            }
            None if target <= 0.0 => return false,
            _ => {}
        }
        let (w, h) = (start_rect.w, start_rect.h);
        let fx = ((start.x - start_rect.x) / w.max(1.0)).clamp(0.0, 1.0);
        let fy = ((start.y - start_rect.y) / h.max(1.0)).clamp(0.0, 1.0);
        let slot = slot_at(&z, side, z.zone_w() * 0.4, w, h);
        let planned = self.warp_plan(cx, screen, Some(client), None, Some((client, slot)));
        // The size the drop settles at: what the tidy gives the window in this shelf.
        let s_slot = planned.into_iter().find(|(c, _, p)| *c == client && *p).map_or_else(|| pose_in(&z, slot).scale, |(_, r, _)| pose_in(&z, r).scale).clamp(super::MIN_SCALE, 0.6);
        let mut en = super::Enter::new(client, side, fx, fy, (w, h), (abs.x, abs.y), s_slot);
        let old = self.state_mut().warp.enter;
        en.e = old.map_or(0.0, |o| o.e);
        en.v = old.map_or(0.0, |o| o.v);
        en.target = target;
        self.state_mut().warp.enter = Some(en);
        true
    }

    /// The inner edge of the shelf on `side` of a display, from where its
    /// items are SHOWN (the dragged window counts at its start rect).
    fn shelf_inner_edge(&mut self, cx: &mut Cx, screen: usize, side: i8, dragged: ClientId, start_rect: LRect) -> Option<f64> {
        let z = self.warp_zones(cx, screen)?;
        let area = self.screen_area(cx, screen);
        let gap = self.state_mut().gap;
        let layout = self.state_mut().layout_at(screen);
        let mut shown: Vec<LRect> = layout
            .rects(area, gap)
            .into_iter()
            .filter(|(c, _)| *c != dragged && layout.is_float(*c) && !layout.is_client_fullscreen(*c) && layout.desktop.get(*c).is_some_and(|w| w.warped))
            .map(|(_, r)| pose_in(&z, r))
            .filter(|p| p.warped() && p.side == side)
            .map(|p| p.shown)
            .collect();
        if self.warp_is_parked(cx, dragged) {
            shown.push(pose_in(&z, start_rect).shown);
        }
        shelf::inner_edge(side, shown)
    }

    /// A tiled window dragged from the tree into a zone of its display
    /// leaves the tree as a float and the drag carries on as a float drag
    /// (the grabbed point stays under the pointer).
    pub(crate) fn warp_take_out(&mut self, cx: &mut Cx, abs: Vec2d) {
        let Some(drag) = self.drag.as_ref() else { return };
        if drag.floating || drag.resize || !drag.armed {
            return;
        }
        let (client, screen, start, sr) = (drag.client, drag.screen, drag.start, drag.start_rect);
        if self.warp_zones(cx, screen).is_none() {
            return;
        }
        if !self.state_mut().warp.enter.is_some_and(|e| e.client == client) || self.state_mut().screen_under(abs.x, abs.y) != screen {
            return;
        }
        let area = self.screen_area(cx, screen);
        let gap = self.state_mut().gap;
        let layout = self.state_mut().layout_at_mut(screen);
        if layout.is_client_fullscreen(client) {
            return;
        }
        let fx = ((start.x - sr.x) / sr.w.max(1.0)).clamp(0.0, 1.0);
        let fy = ((start.y - sr.y) / sr.h.max(1.0)).clamp(0.0, 1.0);
        let rect = LRect::new(abs.x - fx * sr.w, abs.y - fy * sr.h, sr.w, sr.h);
        layout.float_from_tree(client, rect, area, gap);
        if let Some(drag) = self.drag.as_mut() {
            drag.floating = true;
            drag.start = abs;
            drag.start_rect = rect;
        }
        self.state_mut().dragging = vec![client];
    }

    /// Drag end: a SUPER click on a parked window swaps it to the centre;
    /// a moved float is parked and the zones are tidied.
    pub(crate) fn warp_end_drag(&mut self, cx: &mut Cx, drag: &DragState) {
        self.state_mut().warp.press_pose = None;
        // The drag is over: the window is shown by its own pose again at once.
        self.state_mut().warp.armed_drag = None;
        if let Some(mut d) = self.desk(cx).borrow_mut::<crate::desk::WmDesk>() {
            d.warp.dragged.clear();
        }
        let leave = self.state_mut().warp.leave.take().filter(|l| l.client == drag.client);
        // Let go with the pointer still over its shelf, the window goes back
        // on it (parked at the warp law's rect for the drop); in the band it
        // stays out.
        let back = match (leave, self.warp_zones(cx, drag.screen)) {
            (Some(l), Some(z)) => inward_of(&z, l.side, drag.last.x) < 0.0,
            _ => false,
        };
        let mut back_from = None;
        if back {
            if let (Some(screen), Some(z), Some(l)) = (self.state_mut().screens.screen_of(drag.client), self.warp_zones(cx, drag.screen), leave) {
                // The window's logical rect at the size it is shown at now:
                // the drop's tween starts from it, so the size does not jump.
                let sh = l.shown(&z);
                let r = desk_with_scale(&z, l.size.0, l.size.1, l.scale(&z), sh.y + sh.h * 0.5, l.side);
                self.state_mut().layout_at_mut(screen).set_float_rect(drag.client, r);
                back_from = Some(r);
            }
        }
        // The edge preview: a release while it shows shelves the window, in
        // the slot it was previewing; otherwise the drop is a normal one.
        let enter = self.state_mut().warp.enter.take().filter(|e| e.client == drag.client);
        let mut docked = false;
        if let (Some(e), Some(z), Some(screen)) = (enter, self.warp_zones(cx, drag.screen), self.state_mut().screens.screen_of(drag.client)) {
            if !(e.e() > 0.3 && drag.floating && !drag.resize) && e.e() > 0.0 {
                // Let go outside the shelf mid-shrink: the window eases back
                // to full size from the size it is shown at, it does not snap.
                let shown = e.shown(&z);
                if let Some(mut d) = self.desk(cx).borrow_mut::<crate::desk::WmDesk>() {
                    d.release_from(drag.client, shown);
                }
            }
            if e.e() > 0.3 && drag.floating && !drag.resize {
                let shown = e.shown(&z);
                let r = slot_at(&z, e.side, z.zone_w() * 0.4, e.size.0, e.size.1);
                self.state_mut().layout_at_mut(screen).set_float_rect(drag.client, r);
                // The tween starts at the LOGICAL rect whose pose is what is shown now.
                let from = desk_with_scale(&z, e.size.0, e.size.1, e.scale(), shown.y + shown.h * 0.5, e.side);
                if let Some(mut d) = self.desk(cx).borrow_mut::<crate::desk::WmDesk>() {
                    d.release_from(drag.client, from);
                }
                docked = true;
            }
        }
        // Let go mid-growth: the window eases from the pose it shows to its
        // full-size rect with the desk's spring, it does not jump.
        if let (Some(l), Some(z)) = (leave, self.warp_zones(cx, drag.screen)) {
            let shown = back_from.unwrap_or_else(|| l.release_from(&z));
            if let Some(mut d) = self.desk(cx).borrow_mut::<crate::desk::WmDesk>() {
                d.release_from(drag.client, shown);
            }
        }
        if std::env::var_os("MAKEPAD_WM_TRACE_WARP").is_some() {
            log!("warp: drag end client {} floating {} resize {} armed {} screen {}", drag.client, drag.floating, drag.resize, drag.armed, drag.screen);
        }
        if !drag.floating || drag.resize {
            return;
        }
        if !drag.armed {
            return;
        }
        if let Some(screen) = self.state_mut().screens.screen_of(drag.client) {
            // A window pulled out of its zone is already where it is let
            // go, at full size: nothing snaps it elsewhere.
            let anchor = self.warp_grab_anchor(cx, drag);
            self.warp_settle_at(cx, screen, (docked).then_some(drag.client), anchor);
            // A window that came out of the tree and was dropped in the
            // band goes back into it.
            if !self.warp_is_parked(cx, drag.client) {
                let area = self.screen_area(cx, screen);
                let gap = self.state_mut().gap;
                self.state_mut().layout_at_mut(screen).reinsert(drag.client, area, gap);
            }
        }
    }

    /// Pack every parked window of a display into columns; `dragged` (just
    /// dropped) joins the column nearest its drop, or goes back to the band
    /// when it barely entered the zone.
    pub(crate) fn warp_settle(&mut self, cx: &mut Cx, screen: usize, dragged: Option<ClientId>) {
        self.warp_settle_at(cx, screen, dragged, None);
    }

    /// `warp_settle` for a drop: `anchor` (the grab point's fraction of the
    /// window height and the pointer's y) keeps the dropped window's grabbed
    /// point under the pointer in its zone column.
    fn warp_settle_at(&mut self, cx: &mut Cx, screen: usize, dragged: Option<ClientId>, anchor: Option<(f64, f64)>) {
        for (c, rect, parked) in self.warp_plan(cx, screen, dragged, anchor, None) {
            self.state_mut().layout_at_mut(screen).set_float_rect(c, rect);
            self.warp_flag(c, parked);
        }
        self.redraw_all(cx);
    }

    /// What a settle would write: every float's desk rect and whether it is
    /// parked. The drop applies it; the background's landing outline reads it.
    fn warp_plan(&mut self, cx: &mut Cx, screen: usize, dragged: Option<ClientId>, anchor: Option<(f64, f64)>, over: Option<(ClientId, LRect)>) -> Vec<(ClientId, LRect, bool)> {
        let Some(z) = self.warp_zones(cx, screen) else { return Vec::new() };
        let area = self.screen_area(cx, screen);
        let gap = self.state_mut().gap;
        let layout = self.state_mut().layout_at(screen);
        let floats: Vec<(ClientId, LRect)> = layout
            .rects(area, gap)
            .into_iter()
            .filter(|(c, _)| layout.is_float(*c) && !layout.is_client_fullscreen(*c))
            .map(|(c, r)| match over { Some((o, or)) if o == c => (c, or), _ => (c, r) })
            .collect();
        let parked_flags: std::collections::HashSet<ClientId> = floats.iter().map(|(c, _)| *c).filter(|c| layout.desktop.get(*c).is_some_and(|w| w.warped)).collect();
        let mut sides: [Vec<Item>; 2] = [Vec::new(), Vec::new()];
        let mut writes: Vec<(ClientId, LRect, bool)> = Vec::new();
        for (c, r) in floats {
            // Only a window the layout has parked, or the one being docked,
            // is a shelf item; any other window is a normal window wherever
            // its rect is.
            let pose = pose_in(&z, r);
            let on_shelf = Some(c) == dragged || parked_flags.contains(&c);
            let side = if on_shelf && pose.warped() { pose.side() } else { 0 };
            if side == 0 {
                writes.push((c, r, false));
            } else if Some(c) == dragged && pose.scale >= PARK_BELOW {
                let (cx_, cy_) = r.center();
                writes.push((c, into_band(&z, r.w, r.h, cx_, cy_), false));
            } else {
                let fixed = self.desk(cx).borrow::<crate::desk::WmDesk>().and_then(|d| d.warp.compact_slot(c, &z));
                let anchor = anchor.filter(|_| Some(c) == dragged);
                sides[(side > 0) as usize].push(Item { id: c, rect: r, fixed, anchor });
            }
        }
        for (i, side) in [-1i8, 1].into_iter().enumerate() {
            for (id, rect) in tidy(&z, side, &sides[i], dragged) {
                writes.push((id, rect, true));
            }
        }
        writes
    }

    /// The grabbed point of a drag as (fraction of the window height, the
    /// pointer's y): the same grabbed pixel `drag_rect` keeps under the pointer.
    fn warp_grab_anchor(&mut self, cx: &mut Cx, drag: &DragState) -> Option<(f64, f64)> {
        let z = self.warp_zones(cx, drag.screen)?;
        self.warp_grab_anchor_of(&z, drag)
    }

    fn warp_grab_anchor_of(&self, z: &Zones, drag: &DragState) -> Option<(f64, f64)> {
        let p0 = pose_in(z, drag.start_rect);
        let start_y = self.state.as_ref().map_or(drag.start.y, |s| s.warp.grab_start(drag.client, (drag.start.x, drag.start.y), p0.shown).1);
        let fy = ((start_y - p0.shown.y) / p0.shown.h.max(1.0)).clamp(0.0, 1.0);
        Some((fy, drag.last.y))
    }

    /// Where the window being dragged parks if dropped now, as the rect it
    /// will be SHOWN at in its zone (the same plan the drop applies), for the
    /// grid's shelf cue; none when the drop would not park it, nothing is
    /// dragged or spacewarp is off.
    pub(crate) fn warp_landing(&mut self, cx: &mut Cx) -> Option<Rect> {
        let client = *self.state_mut().dragging.first()?;
        let screen = self.state_mut().screens.screen_of(client)?;
        let z = self.warp_zones(cx, screen)?;
        // A parked window being pulled out: the cue traces where it will land
        // in the band, for as long as it is still scaled.
        if let Some(l) = self.state_mut().warp.leave.filter(|l| l.client == client) {
            let r = l.landing(&z)?;
            return Some(Rect { pos: dvec2(r.x, r.y), size: dvec2(r.w, r.h) });
        }
        // Only the edge preview shows where a drop would shelve the window.
        let e = self.state_mut().warp.enter.filter(|e| e.client == client)?;
        let anchor = self.drag.as_ref().and_then(|d| self.warp_grab_anchor_of(&z, d));
        let slot = slot_at(&z, e.side, z.zone_w() * 0.4, e.size.0, e.size.1);
        let plan = self.warp_plan(cx, screen, Some(client), anchor, Some((client, slot)));
        let (_, rect, _) = plan.into_iter().find(|(c, _, parked)| *c == client && *parked)?;
        let shown = pose_in(&z, rect).shown;
        Some(Rect { pos: dvec2(shown.x, shown.y), size: dvec2(shown.w, shown.h) })
    }

    /// Swap a parked window with the focus window: it flies to the centre at
    /// 1:1, the focus window takes its slot. False when it is not parked.
    pub(crate) fn warp_swap(&mut self, cx: &mut Cx, client: ClientId) -> bool {
        let Some((screen, z, rect)) = self.warp_rect(cx, client) else { return false };
        let pose = pose_in(&z, rect);
        if !pose.warped() || !self.warp_is_parked(cx, client) {
            return false;
        }
        let area = self.screen_area(cx, screen);
        let gap = self.state_mut().gap;
        let layout = self.state_mut().layout_at(screen);
        // The partner: the window that was in front in the band. (The press
        // that began this gesture already focused and raised the parked
        // one, so the focus itself is not it.)
        let focus = layout
            .rects(area, gap)
            .into_iter()
            .rev()
            .find(|(c, _)| *c != client && layout.is_float(*c) && !layout.desktop.get(*c).is_some_and(|w| w.warped));
        let (mx, my) = focus.map_or(((z.left + z.right) * 0.5, z.screen.y + z.screen.h * 0.5), |(_, r)| r.center());
        let centred = into_band(&z, rect.w, rect.h, mx, my);
        let layout = self.state_mut().layout_at_mut(screen);
        layout.set_float_rect(client, centred);
        layout.raise_float(client);
        self.warp_flag(client, false);
        if let Some((f, r)) = focus {
            // The focus window takes the slot; the tidy below finds its column.
            let (lx, ly) = rect.center();
            self.state_mut().layout_at_mut(screen).set_float_rect(f, LRect::new(lx - r.w * 0.5, ly - r.h * 0.5, r.w, r.h));
            self.warp_flag(f, true);
        }
        self.focus_client(cx, client);
        self.warp_settle(cx, screen, focus.map(|(f, _)| f));
        true
    }

    /// SUPER+CTRL+`: bring the next parked window of the active display to
    /// the centre (nearest to the centre first, each once per round).
    pub(crate) fn warp_cycle(&mut self, cx: &mut Cx) {
        if !self.state_mut().warp.shelves_on() {
            return;
        }
        let focus = self.state_mut().layout().focused_client();
        let screen = self.state_mut().screens.active;
        let Some(z) = self.warp_zones(cx, screen) else { return };
        let area = self.screen_area(cx, screen);
        let gap = self.state_mut().gap;
        let layout = self.state_mut().layout_at(screen);
        let centre = (z.left + z.right) * 0.5;
        let parked: Vec<(u64, f64)> = layout
            .rects(area, gap)
            .into_iter()
            .filter(|(c, _)| layout.is_float(*c) && layout.desktop.get(*c).is_some_and(|w| w.warped))
            .filter_map(|(c, r)| {
                let p = pose_in(&z, r);
                p.warped().then(|| (c, (p.shown.center().0 - centre).abs()))
            })
            .collect();
        let mut visited = std::mem::take(&mut self.state_mut().warp.cycled);
        let pick = next_parked(&parked, &mut visited);
        if let Some(c) = pick {
            visited.push(c);
            visited.extend(focus);
        }
        self.state_mut().warp.cycled = visited;
        if let Some(c) = pick {
            self.warp_swap(cx, c);
        }
    }

    /// SUPER+DOWN: the focus window, if it is a shelf item, comes off its
    /// shelf to the centre at full size (the keyboard way out, so a shelved
    /// window is never stuck).
    pub(crate) fn warp_unshelf(&mut self, cx: &mut Cx) {
        let Some(client) = self.state_mut().layout().focused_client() else { return };
        if !self.warp_is_parked(cx, client) {
            return;
        }
        let Some((screen, z, rect)) = self.warp_rect(cx, client) else { return };
        // Where the window is SHOWN now: its tween to the band starts there
        // (from its logical rect it would fly in from off the display).
        let shown = self.desk(cx).borrow::<crate::desk::WmDesk>().and_then(|d| d.warp_pose(client)).map_or_else(|| pose_in(&z, rect).shown, |p| p.shown);
        // Back at the band's edge nearest the shelf it came from, mid-height.
        let side = pose_in(&z, rect).side;
        let cxm = if side < 0 { z.left + 8.0 + rect.w * 0.5 } else { z.right - 8.0 - rect.w * 0.5 };
        let centred = into_band(&z, rect.w, rect.h, cxm, z.screen.y + z.screen.h * 0.5);
        if let Some(mut d) = self.desk(cx).borrow_mut::<crate::desk::WmDesk>() {
            if d.warp.unfold.is_some_and(|u| u.client == client) {
                d.warp.unfold = None;
            }
            d.release_from(client, shown);
        }
        let layout = self.state_mut().layout_at_mut(screen);
        layout.set_float_rect(client, centred);
        layout.raise_float(client);
        self.warp_flag(client, false);
        self.focus_client(cx, client);
        self.warp_settle(cx, screen, None);
    }

    /// SUPER+CTRL+A / D: park the focus window in the nearest slot of the
    /// left / right zone; a window parked on the other side comes back to
    /// the band.
    pub(crate) fn warp_park(&mut self, cx: &mut Cx, right: bool) {
        if !self.state_mut().warp.shelves_on() {
            let message = if self.state_mut().warp.tiling { "Tiling: no shelves" } else { "Classic desktop: no shelves" };
            self.show_osd(cx, crate::shell::osd::OsdShow::message(message));
            return;
        }
        let Some(client) = self.state_mut().layout().focused_client() else { return };
        let Some((screen, z, rect)) = self.warp_rect(cx, client) else { return };
        let pose = pose_in(&z, rect);
        let side = if pose.warped() && self.warp_is_parked(cx, client) { pose.side() } else { 0 };
        let want: i8 = if right { 1 } else { -1 };
        if side == want {
            return;
        }
        if side == -want {
            let x = if want > 0 { z.left + rect.w * 0.5 } else { z.right - rect.w * 0.5 };
            let back = into_band(&z, rect.w, rect.h, x, rect.center().1);
            self.state_mut().layout_at_mut(screen).set_float_rect(client, back);
            self.warp_flag(client, false);
            self.warp_settle(cx, screen, None);
            return;
        }
        let slot = slot_at(&z, want, z.zone_w() * 0.4, rect.w, rect.h);
        self.state_mut().layout_at_mut(screen).set_float_rect(client, slot);
        self.warp_flag(client, true);
        self.warp_settle(cx, screen, Some(client));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down(x: f64, y: f64) -> Event {
        Event::MouseDown(MouseDownEvent { abs: dvec2(x, y), button: MouseButton::PRIMARY, window_id: WindowId(0, 0), modifiers: Default::default(), handled: Default::default(), time: 0.0 })
    }
    fn at(event: &Event) -> Vec2d {
        abs_of(event).unwrap()
    }

    /// Two overlapping windows while a third is parked (the router is on):
    /// a press on the overlap goes to the topmost window only.
    #[test]
    fn a_press_on_two_overlapping_windows_goes_to_the_topmost_only() {
        let screen = LRect::new(0.0, 0.0, 1920.0, 1080.0);
        let mut warp = Warp::default();
        warp.set_areas([screen]);
        warp.parked.insert(3);
        let z = Zones::new(screen);
        let parked = super::super::slot_at(&z, 1, z.zone_w() * 0.6, 800.0, 600.0);
        let rect = |r: LRect| Rect { pos: dvec2(r.x, r.y), size: dvec2(r.w, r.h) };
        // topmost first: 2 over 1, 3 parked in the right zone
        let order = [
            (2, Rect { pos: dvec2(700.0, 200.0), size: dvec2(700.0, 600.0) }),
            (1, Rect { pos: dvec2(400.0, 100.0), size: dvec2(700.0, 600.0) }),
            (3, rect(parked)),
        ];
        let mut router = Router::default();
        let routed = router.route(&warp, &down(900.0, 400.0), &order).expect("a parked window turns the router on");
        let seen: Vec<_> = routed.iter().filter(|(_, e)| at(e) != FAR).map(|(c, _)| *c).collect();
        assert_eq!(seen, vec![2], "only the topmost window sees the press");
        // Outside the top window, the one below gets it.
        let routed = router.route(&warp, &down(500.0, 150.0), &order).unwrap();
        let seen: Vec<_> = routed.iter().filter(|(_, e)| at(e) != FAR).map(|(c, _)| *c).collect();
        assert_eq!(seen, vec![1]);
    }
}
