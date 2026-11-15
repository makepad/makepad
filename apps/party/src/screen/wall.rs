//! The big screen: every loop and photo guests put on the wall, playing,
//! with the join code beside them; after a while without anything new, one
//! at a time across the whole screen (attract mode), while the box redraws
//! the wall's pictures in its best quality.
//!
//! The host taps a picture to pick it and taps Remove to take it (and every
//! redraw of it) off the box.
//!
//! The wall looks at the store at most ten times a second, reading its
//! change counter under its lock (every holder keeps it for a few field
//! updates); a change copies the wall's list (shared JPEGs) out. The frames
//! are decoded on a thread at the size their tiles are drawn (wall_plan.rs:
//! the wall holds about the screen's pixels per loop frame, however many
//! loops come), and whole only for the picture attract mode shows. It
//! redraws when a loop's frame changes, not every display frame.

use crate::screen::frames::{Frames, Pic, Want};
use makepad_app_party::api::Party;
use makepad_app_party::store::{Jpeg, Quality, SourceKind, Who};
use makepad_app_party::wall_plan::{self, GAP};
use makepad_qr::Qr;
use makepad_widgets::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PartyWallBase = #(PartyWall::register_widget(vm))

    mod.widgets.PartyWall = set_type_default() do mod.widgets.PartyWallBase{
        width: Fill
        height: Fill
        draw_bg +: {color: #14110f}
        draw_tile +: {}
        draw_pick +: {color: #a58cff}
        draw_dim +: {color: #000a}
        draw_qr +: {color: #fff}
        draw_button +: {color: #b3261e}
        draw_title +: {color: #f1ece6 text_style: theme.font_bold{font_size: 26}}
        draw_note +: {color: #c8bfb4 text_style: theme.font_regular{font_size: 13}}
    }
}

/// Seconds without anything new before the wall goes to attract mode.
const ATTRACT_AFTER: f64 = 75.0;
/// Seconds each picture holds the screen in attract mode.
const ATTRACT_EACH: f64 = 9.0;
/// The join panel's width beside the wall.
const PANEL: f64 = 400.0;
/// Seconds between looks at the store.
const SYNC_EVERY: f64 = 0.1;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shown {
    Source(u64),
    Take(u64),
}

struct Tile {
    source: u64,
    shown: Shown,
    /// The shown frames (shared with the store).
    pictures: Vec<Jpeg>,
    fps: f64,
    best: bool,
}

impl Tile {
    fn pic(&self, i: usize) -> Pic {
        match self.shown {
            Shown::Source(id) => Pic::Source(id, i),
            Shown::Take(id) => Pic::Take(id, i),
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct PartyWall {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[area]
    area: Area,
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_tile: DrawImage,
    #[live]
    draw_pick: DrawColor,
    #[live]
    draw_dim: DrawColor,
    #[live]
    draw_qr: DrawColor,
    #[live]
    draw_button: DrawColor,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_note: DrawText,
    #[rust]
    party: Option<Arc<Party>>,
    #[rust]
    join_url: String,
    #[rust]
    qr: Option<Qr>,
    #[rust]
    frames: Option<Frames>,
    #[rust]
    tiles: Vec<Tile>,
    #[rust]
    version: u64,
    #[rust]
    last_change: f64,
    #[rust]
    attract_since: Option<f64>,
    #[rust]
    picked: Option<u64>,
    /// Where each tile and the Remove button were last drawn, for taps.
    #[rust]
    tile_rects: Vec<(u64, Rect)>,
    #[rust]
    remove_rect: Option<Rect>,
    #[rust]
    next: NextFrame,
    #[rust]
    last_upgrade_check: f64,
    #[rust]
    last_sync: f64,
    /// The animation step last drawn: a redraw comes when it moves.
    #[rust]
    drawn_step: i64,
    /// Shown instead of the wall when the party did not start.
    #[rust]
    message: Option<String>,
    /// What the decoder was last asked for: (store version, tile pixels,
    /// the attract picture). A change asks again.
    #[rust]
    decoding: (u64, u32, Option<usize>),
}

impl PartyWall {
    /// The party this wall shows and the address guests join at.
    pub fn set_party(&mut self, cx: &mut Cx, party: Arc<Party>, join_url: String) {
        self.qr = Qr::encode(join_url.as_bytes()).ok();
        self.join_url = join_url;
        self.party = Some(party);
        self.frames = Some(Frames::new());
        self.version = u64::MAX;
        self.last_change = Cx::monotonic_now();
        self.next = cx.new_next_frame();
        self.redraw(cx);
    }

    /// Shows `message` instead of the wall (the party did not start).
    pub fn set_message(&mut self, cx: &mut Cx, message: String) {
        self.message = Some(message);
        self.redraw(cx);
    }

    /// Picks up the store's changes: the wall's list. True when it changed.
    fn sync(&mut self, now: f64) -> bool {
        let Some(party) = &self.party else { return false };
        let new_source;
        {
            let store = party.store();
            if store.version == self.version {
                return false;
            }
            self.version = store.version;
            let before: HashSet<u64> = self.tiles.iter().map(|t| t.source).collect();
            let wall = store.wall();
            new_source = wall.iter().any(|(s, _)| !before.contains(&s.id));
            self.tiles.clear();
            for (source, take) in wall {
                let fps = match source.kind {
                    SourceKind::Loop { fps } => fps as f64,
                    SourceKind::Photo => 0.0,
                };
                let (shown, pictures, best) = match take {
                    Some(t) => (Shown::Take(t.id), &t.frames, t.quality == Quality::Best),
                    None => (Shown::Source(source.id), &source.frames, false),
                };
                self.tiles.push(Tile { source: source.id, shown, pictures: pictures.clone(), fps, best });
            }
        }
        if self.picked.is_some_and(|p| !self.tiles.iter().any(|t| t.source == p)) {
            self.picked = None;
        }
        // Something new from a guest brings the wall back; a redraw of what
        // is already there (the upgrades attract mode itself asks for) does not.
        if new_source {
            self.last_change = now;
            self.attract_since = None;
        }
        true
    }

    /// Moves whenever some picture on screen should change.
    fn step(&self, now: f64) -> i64 {
        if let Some(since) = self.attract_since {
            let at = ((now - since) / ATTRACT_EACH) as i64;
            let fps = self.tiles.get(at as usize % self.tiles.len().max(1)).map_or(0.0, |t| t.fps);
            return at * 1_000_000 + (now * fps) as i64;
        }
        let fps = self.tiles.iter().map(|t| t.fps).fold(0.0, f64::max);
        (now * fps) as i64
    }

    /// In attract mode, while the box has nothing else to do, the next wall
    /// picture without a best-quality redraw gets one.
    fn upgrade(&mut self, now: f64) {
        if now - self.last_upgrade_check < 3.0 {
            return;
        }
        self.last_upgrade_check = now;
        let Some(party) = &self.party else { return };
        if !party.queue.is_empty() {
            return;
        }
        let take = {
            let mut store = party.store();
            if store.busy() {
                return;
            }
            let Some(source) = store.upgrade_candidate() else { return };
            log!("party: the box is idle: source {source} in the best quality");
            store.add_take(source, Quality::Best).ok()
        };
        if let Some(take) = take {
            party.queue.push(take);
        }
    }

    fn frame_index(tile: &Tile, now: f64) -> usize {
        if tile.pictures.is_empty() {
            return 0;
        }
        if tile.fps <= 0.0 {
            return 0;
        }
        ((now * tile.fps) as usize) % tile.pictures.len()
    }

    /// Asks the decoder for every wall frame at `side` pixels, and for the
    /// attract picture's whole, when any of them changed since last time.
    fn decode(&mut self, side: u32, attract_at: Option<usize>) {
        let plan = (self.version, side, attract_at);
        let Some(frames) = &mut self.frames else { return };
        if plan == self.decoding {
            return;
        }
        self.decoding = plan;
        let mut wanted: HashMap<Want, Jpeg> = HashMap::new();
        for tile in &self.tiles {
            for (i, jpeg) in tile.pictures.iter().enumerate() {
                wanted.insert((tile.pic(i), side), jpeg.clone());
            }
        }
        if let Some(tile) = attract_at.and_then(|at| self.tiles.get(at)) {
            for (i, jpeg) in tile.pictures.iter().enumerate() {
                wanted.insert((tile.pic(i), 0), jpeg.clone());
            }
        }
        frames.sync(&wanted);
    }

    /// Draws `tile`'s current frame into `rect`: as a square of `side`
    /// pixels cropped to fill it, or (`side` 0) all of it in the middle of
    /// `rect`. Answers where it went.
    fn draw_picture(&mut self, cx: &mut Cx2d, tile_at: usize, rect: Rect, now: f64, side: u32) -> Rect {
        let whole = side == 0;
        let Some(frames) = &self.frames else { return rect };
        let tile = &self.tiles[tile_at];
        let want = Self::frame_index(tile, now);
        let count = tile.pictures.len();
        // The newest decoded frame at or before the one due: a loop still
        // decoding plays what it has.
        let frame = (0..=want).rev().chain(want + 1..count).find_map(|i| frames.get(tile.pic(i), side));
        let Some(frame) = frame else {
            self.draw_dim.draw_abs(cx, rect);
            return rect;
        };
        let (texture, ta) = (frame.texture.clone(), frame.width as f64 / frame.height.max(1) as f64);
        let ra = rect.size.x / rect.size.y.max(1.0);
        let rect = if whole {
            let size = if ta > ra { dvec2(rect.size.x, rect.size.x / ta) } else { dvec2(rect.size.y * ta, rect.size.y) };
            self.draw_tile.image_scale = vec2(1.0, 1.0);
            self.draw_tile.image_pan = vec2(0.0, 0.0);
            Rect { pos: rect.pos + (rect.size - size) * 0.5, size }
        } else {
            let (sx, sy) = if ta > ra { (ra / ta, 1.0) } else { (1.0, ta / ra) };
            self.draw_tile.image_scale = vec2(sx as f32, sy as f32);
            self.draw_tile.image_pan = vec2(((1.0 - sx) / 2.0) as f32, ((1.0 - sy) / 2.0) as f32);
            rect
        };
        self.draw_tile.draw_vars.set_texture(0, &texture);
        self.draw_tile.draw_abs(cx, rect);
        rect
    }

    fn draw_qr_code(&mut self, cx: &mut Cx2d, at: Vec2d, side: f64) {
        let Some(qr) = &self.qr else { return };
        // A quiet zone of four modules all round, as readers expect.
        let modules = qr.size + 8;
        let m = (side / modules as f64).floor().max(1.0);
        let full = m * modules as f64;
        self.draw_qr.color = vec4(1.0, 1.0, 1.0, 1.0);
        self.draw_qr.draw_abs(cx, Rect { pos: at, size: dvec2(full, full) });
        self.draw_qr.color = vec4(0.0, 0.0, 0.0, 1.0);
        for y in 0..qr.size {
            for x in (0..qr.size).filter(|&x| qr.get(x, y)) {
                let pos = dvec2(at.x + (x + 4) as f64 * m, at.y + (y + 4) as f64 * m);
                self.draw_qr.draw_abs(cx, Rect { pos, size: dvec2(m, m) });
            }
        }
    }

}

impl Widget for PartyWall {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next.is_event(event).is_some() {
            let now = Cx::monotonic_now();
            let mut changed = false;
            if now - self.last_sync >= SYNC_EVERY {
                self.last_sync = now;
                changed |= self.sync(now);
            }
            if let Some(frames) = &mut self.frames {
                if frames.drain(cx) {
                    changed = true;
                    if frames.pending() == 0 {
                        log!("party: the wall holds {:.1} MB of pictures", frames.bytes() as f64 / (1 << 20) as f64);
                    }
                }
            }
            if self.attract_since.is_none() && !self.tiles.is_empty() && now - self.last_change > ATTRACT_AFTER {
                log!("party: attract mode after {:.0} s without anything new", now - self.last_change);
                self.attract_since = Some(now);
                changed = true;
            }
            if self.attract_since.is_some() {
                self.upgrade(now);
            }
            let step = self.step(now);
            if changed || step != self.drawn_step {
                self.drawn_step = step;
                self.redraw(cx);
            }
            self.next = cx.new_next_frame();
        }
        if let Hit::FingerUp(fe) = event.hits(cx, self.area) {
            let now = Cx::monotonic_now();
            if self.attract_since.take().is_some() {
                // A tap in attract mode brings the wall back.
                self.last_change = now;
                self.redraw(cx);
                return;
            }
            self.last_change = now;
            if self.remove_rect.is_some_and(|r| r.contains(fe.abs)) {
                if let (Some(party), Some(source)) = (&self.party, self.picked.take()) {
                    let persist = party.store().delete(Who::Host, source).unwrap_or_default();
                    party.persist(persist);
                    log!("party: the host removed source {source}");
                }
            } else {
                let hit = self.tile_rects.iter().find(|(_, r)| r.contains(fe.abs)).map(|(s, _)| *s);
                self.picked = if hit == self.picked { None } else { hit };
            }
            self.redraw(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        let now = Cx::monotonic_now();
        self.draw_bg.draw_abs(cx, rect);
        self.tile_rects.clear();
        self.remove_rect = None;
        if let Some(message) = self.message.clone() {
            self.draw_title.draw_abs(cx, dvec2(rect.pos.x + 32.0, rect.pos.y + rect.size.y * 0.45), &message);
            return DrawStep::done();
        }

        // The wall's layout, in attract mode too: its tiles stay decoded at
        // their size, the attract picture whole beside them.
        let wall = Rect { pos: rect.pos + dvec2(GAP, GAP), size: dvec2(rect.size.x - PANEL - 2.0 * GAP, rect.size.y - 2.0 * GAP) };
        let (cols, side) = wall_plan::grid(self.tiles.len(), wall.size.x, wall.size.y);
        let pixels = wall_plan::decode_side(side, cx.current_dpi_factor());
        let attract_at = self.attract_since.filter(|_| !self.tiles.is_empty()).map(|since| ((now - since) / ATTRACT_EACH) as usize % self.tiles.len());
        self.decode(pixels, attract_at);

        if let Some(at) = attract_at {
            // One picture across the screen, the next every few seconds.
            let shown = self.draw_picture(cx, at, rect, now, 0);
            let side = (rect.size.y * 0.22).min(220.0);
            let pos = dvec2(rect.pos.x + rect.size.x - side - 24.0, rect.pos.y + rect.size.y - side - 24.0);
            self.draw_qr_code(cx, pos, side);
            if self.tiles[at].best && shown.pos.x - rect.pos.x > 260.0 {
                // On the dark margin beside the picture.
                self.draw_note.draw_abs(cx, dvec2(rect.pos.x + 24.0, rect.pos.y + rect.size.y - 40.0), "redrawn in the best quality");
            }
            return DrawStep::done();
        }

        // The join panel on the right.
        let panel = Rect { pos: dvec2(rect.pos.x + rect.size.x - PANEL, rect.pos.y), size: dvec2(PANEL, rect.size.y) };
        let qr_side = PANEL - 48.0;
        self.draw_title.draw_abs(cx, dvec2(panel.pos.x + 24.0, panel.pos.y + 32.0), "Join the party");
        self.draw_qr_code(cx, dvec2(panel.pos.x + 24.0, panel.pos.y + 84.0), qr_side);
        let url = self.join_url.clone();
        self.draw_note.draw_abs(cx, dvec2(panel.pos.x + 24.0, panel.pos.y + 100.0 + qr_side), &url);
        let lines = ["Scan it with your phone's camera,", "then make a short loop or pick photos."];
        for (i, line) in lines.iter().enumerate() {
            self.draw_note.draw_abs(cx, dvec2(panel.pos.x + 24.0, panel.pos.y + 136.0 + qr_side + i as f64 * 24.0), line);
        }

        if self.tiles.is_empty() {
            self.draw_title.draw_abs(cx, dvec2(wall.pos.x + 32.0, wall.pos.y + wall.size.y * 0.45), "The wall is waiting for your first loop");
            return DrawStep::done();
        }
        // The grid sits in the middle of the wall.
        let rows = self.tiles.len().div_ceil(cols);
        let used = dvec2(cols as f64 * (side + GAP) - GAP, rows as f64 * (side + GAP) - GAP);
        let origin = wall.pos + dvec2((wall.size.x - used.x).max(0.0) / 2.0, (wall.size.y - used.y).max(0.0) / 2.0);
        for i in 0..self.tiles.len() {
            let (col, row) = (i % cols, i / cols);
            let tile_rect = Rect {
                pos: dvec2(origin.x + col as f64 * (side + GAP), origin.y + row as f64 * (side + GAP)),
                size: dvec2(side, side),
            };
            let source = self.tiles[i].source;
            if self.picked == Some(source) {
                self.draw_pick.draw_abs(cx, Rect { pos: tile_rect.pos - dvec2(4.0, 4.0), size: tile_rect.size + dvec2(8.0, 8.0) });
            }
            self.draw_picture(cx, i, tile_rect, now, pixels);
            self.tile_rects.push((source, tile_rect));
            if self.picked == Some(source) {
                let button = Rect { pos: tile_rect.pos + dvec2(12.0, tile_rect.size.y - 56.0), size: dvec2(120.0, 44.0) };
                self.draw_button.draw_abs(cx, button);
                self.draw_note.draw_abs(cx, button.pos + dvec2(28.0, 13.0), "Remove");
                self.remove_rect = Some(button);
            }
        }
        DrawStep::done()
    }
}
