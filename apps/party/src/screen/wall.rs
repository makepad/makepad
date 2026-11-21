//! The big screen: one picture across the whole window, at its own aspect
//! with black around it, and the party's QR code small in the top-right
//! corner. Nothing else: no labels, panels or buttons.
//!
//! What it shows, the first that applies:
//! - the live guest's camera, redrawn, from the hub's first frame on (while
//!   their call starts, the screen stays on what it showed);
//! - the guests' loops and photos, one at a time: a new one as soon as it
//!   comes, then the next every few seconds.
//!
//! After a while with nothing new and nobody live, the box redraws the
//! pictures it shows in its best quality while it has nothing else to do.
//!
//! Per display frame: an atomic read of the live frame count; when it moved,
//! the newest live frame's texels (made on the live thread) are copied into
//! the one live texture, an upload and one quad. The store is looked at ten
//! times a second, reading its change counter under its lock (every holder
//! keeps it for a few field updates); a change copies the wall's list
//! (shared JPEGs) out. A decoder thread holds the shown loop and the next at
//! the window's size; the screen redraws when a picture changes, not every
//! display frame.

use crate::screen::frames::{Frames, Pic, Size, Want};
use makepad_app_party::api::Party;
use makepad_app_party::live::Standing;
use makepad_app_party::store::{Jpeg, Quality, SourceKind};
use makepad_qr::Qr;
use makepad_widgets::*;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PartyWallBase = #(PartyWall::register_widget(vm))

    mod.widgets.PartyWall = set_type_default() do mod.widgets.PartyWallBase{
        width: Fill
        height: Fill
        draw_bg +: {color: #000}
        draw_tile +: {}
        draw_qr +: {color: #fff}
        draw_note +: {color: #c8bfb4 text_style: theme.font_regular{font_size: 13}}
    }
}

/// Seconds without anything new before the box spends its idle time on
/// best-quality redraws of what it shows.
const IDLE_AFTER: f64 = 75.0;
/// Seconds each loop or photo holds the screen.
const EACH: f64 = 9.0;
/// Seconds between looks at the store.
const SYNC_EVERY: f64 = 0.1;
/// The QR code's margin from the window's corner, in points.
const QR_MARGIN: f64 = 20.0;

/// The largest rect of `aspect` (width / height) in the middle of `rect`.
fn fit(rect: Rect, aspect: f64) -> Rect {
    let ra = rect.size.x / rect.size.y.max(1.0);
    let size = if aspect > ra { dvec2(rect.size.x, rect.size.x / aspect) } else { dvec2(rect.size.y * aspect, rect.size.y) };
    Rect { pos: rect.pos + (rect.size - size) * 0.5, size }
}

struct Tile {
    source: u64,
    /// The take it shows: the wall lists only sources with a finished one.
    take: u64,
    /// The shown frames (shared with the store).
    pictures: Vec<Jpeg>,
    fps: f64,
}

impl Tile {
    fn pic(&self, i: usize) -> Pic {
        Pic::Take(self.take, i)
    }

    fn frame_index(&self, now: f64) -> usize {
        if self.pictures.is_empty() || self.fps <= 0.0 {
            return 0;
        }
        ((now * self.fps) as usize) % self.pictures.len()
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
    draw_qr: DrawColor,
    #[live]
    draw_note: DrawText,
    #[rust]
    party: Option<Arc<Party>>,
    #[rust]
    qr: Option<Qr>,
    #[rust]
    frames: Option<Frames>,
    #[rust]
    tiles: Vec<Tile>,
    #[rust]
    version: u64,
    /// When something last came (a loop, a guest going live or leaving).
    #[rust]
    last_change: f64,
    /// The tile on screen, and since when.
    #[rust]
    shown: usize,
    #[rust]
    shown_since: f64,
    #[rust]
    next: NextFrame,
    #[rust]
    last_upgrade_check: f64,
    #[rust]
    last_sync: f64,
    /// The animation step last drawn: a redraw comes when it moves.
    #[rust]
    drawn_step: i64,
    /// Shown instead of the pictures when the party did not start.
    #[rust]
    message: Option<String>,
    /// What the decoder was last asked for: (store version, the shown and
    /// next tiles, the window's pixels). A change asks again.
    #[rust]
    decoding: Option<(u64, usize, usize, Size)>,
    /// The live guest is on the screen (from the hub's first frame).
    #[rust]
    live_on: bool,
    /// The live picture's one texture, refilled with each new frame.
    #[rust]
    live_texture: Option<Texture>,
    #[rust]
    live_aspect: f64,
    /// The live texture's size in texels.
    #[rust]
    live_size: (usize, usize),
    /// The live frame count last shown.
    #[rust]
    live_frames: u64,
}

impl PartyWall {
    /// The party this screen shows and the address guests join at.
    pub fn set_party(&mut self, cx: &mut Cx, party: Arc<Party>, join_url: String) {
        self.qr = Qr::encode(join_url.as_bytes()).ok();
        self.party = Some(party);
        self.frames = Some(Frames::new());
        self.version = u64::MAX;
        self.last_change = Cx::monotonic_now();
        self.next = cx.new_next_frame();
        self.redraw(cx);
    }

    /// Shows `message` instead of the pictures (the party did not start).
    pub fn set_message(&mut self, cx: &mut Cx, message: String) {
        self.message = Some(message);
        self.redraw(cx);
    }

    /// Picks up the store's changes: the wall's list. True when it changed.
    fn sync(&mut self, now: f64) -> bool {
        let Some(party) = &self.party else { return false };
        let shown = self.tiles.get(self.shown).map(|t| t.source);
        let mut newest = None;
        {
            let store = party.store();
            if store.version == self.version {
                return false;
            }
            self.version = store.version;
            let before: HashSet<u64> = self.tiles.iter().map(|t| t.source).collect();
            self.tiles.clear();
            for (source, take) in store.wall() {
                let fps = match source.kind {
                    SourceKind::Loop { fps } => fps as f64,
                    SourceKind::Photo => 0.0,
                };
                if !before.contains(&source.id) {
                    newest = Some(source.id);
                }
                self.tiles.push(Tile { source: source.id, take: take.id, pictures: take.frames.clone(), fps });
            }
        }
        // Something new from a guest goes up at once; otherwise the screen
        // stays on what it shows (a redraw of it, a removal elsewhere).
        let keep = newest.or(shown);
        match keep.and_then(|s| self.tiles.iter().position(|t| t.source == s)) {
            Some(at) => {
                if newest.is_some() || at != self.shown {
                    self.shown_since = now;
                }
                self.shown = at;
            }
            None => {
                self.shown = 0;
                self.shown_since = now;
            }
        }
        if newest.is_some() {
            self.last_change = now;
        }
        true
    }

    /// Picks up whether the live guest is on (on a look at the store) and
    /// their newest frame (every display frame). True when the screen changes.
    fn sync_live(&mut self, cx: &mut Cx, now: f64, look: bool) -> bool {
        let Some(party) = self.party.clone() else { return false };
        let mut changed = false;
        if look {
            let on = party.live.board().is_some_and(|board| matches!(board.standing, Standing::Live { .. }));
            if on != self.live_on {
                // A guest going live, and the screen's return after them, is something new.
                self.live_on = on;
                self.last_change = now;
                self.shown_since = now;
                if !on {
                    self.live_texture = None;
                }
                changed = true;
            }
        }
        if self.live_on {
            let frames = party.live.frames.load(Ordering::Relaxed);
            if frames != self.live_frames {
                self.live_frames = frames;
                if let Some(frame) = party.live.latest() {
                    let same_size = self.live_size == (frame.width, frame.height);
                    self.live_size = (frame.width, frame.height);
                    match &self.live_texture {
                        Some(texture) if same_size => texture.set_data_u32(cx, frame.width, frame.height, frame.bgra.clone()),
                        _ => {
                            // A new texture when the picture changes shape (the phone turned).
                            let format = TextureFormat::VecBGRAu8_32 { width: frame.width, height: frame.height, data: Some(frame.bgra.clone()), updated: TextureUpdated::Full };
                            self.live_texture = Some(Texture::new_with_format(cx, format));
                        }
                    }
                    self.live_aspect = frame.width as f64 / frame.height.max(1) as f64;
                    changed = true;
                }
            }
        }
        changed
    }

    /// The next tile after [`EACH`] seconds. True when the screen changes.
    fn advance(&mut self, now: f64) -> bool {
        if self.tiles.len() > 1 && now - self.shown_since >= EACH {
            self.shown = (self.shown + 1) % self.tiles.len();
            self.shown_since = now;
            return true;
        }
        false
    }

    /// Moves whenever the picture on screen should change.
    fn step(&self, now: f64) -> i64 {
        let fps = self.tiles.get(self.shown).map_or(0.0, |t| t.fps);
        self.shown as i64 * 1_000_000 + (now * fps) as i64
    }

    /// While nothing new has come for a while and nobody is live, the next
    /// wall picture without a best-quality redraw gets one.
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

    /// Asks the decoder for the shown tile's frames and the next one's,
    /// fitted to `size`, when anything changed since last time.
    fn decode(&mut self, size: Size) {
        let next = if self.tiles.is_empty() { 0 } else { (self.shown + 1) % self.tiles.len() };
        let plan = (self.version, self.shown, next, size);
        let Some(frames) = &mut self.frames else { return };
        if Some(plan) == self.decoding {
            return;
        }
        self.decoding = Some(plan);
        let mut wanted: HashMap<Want, Jpeg> = HashMap::new();
        for tile in [self.shown, next].iter().filter_map(|&at| self.tiles.get(at)) {
            for (i, jpeg) in tile.pictures.iter().enumerate() {
                wanted.insert((tile.pic(i), size), jpeg.clone());
            }
        }
        frames.sync(&wanted);
    }

    /// The shown tile's current frame, fitted in `rect`. False when none of
    /// its frames is decoded yet.
    fn draw_shown(&mut self, cx: &mut Cx2d, rect: Rect, now: f64, size: Size) -> bool {
        let (Some(frames), Some(tile)) = (&self.frames, self.tiles.get(self.shown)) else { return false };
        let want = tile.frame_index(now);
        let count = tile.pictures.len();
        // The newest decoded frame at or before the one due: a loop still
        // decoding plays what it has.
        let Some(frame) = (0..=want).rev().chain(want + 1..count).find_map(|i| frames.get(tile.pic(i), size)) else { return false };
        let (texture, aspect) = (frame.texture.clone(), frame.width as f64 / frame.height.max(1) as f64);
        self.draw_tile.image_scale = vec2(1.0, 1.0);
        self.draw_tile.image_pan = vec2(0.0, 0.0);
        self.draw_tile.draw_vars.set_texture(0, &texture);
        self.draw_tile.draw_abs(cx, fit(rect, aspect));
        true
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

/// What the wall tells the app.
#[derive(Clone, Debug, Default)]
pub enum PartyWallAction {
    #[default]
    None,
    /// A double click on the picture: the window maximises, or restores.
    DoubleClicked,
}

impl Widget for PartyWall {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Hit::FingerDown(fe) = event.hits(cx, self.area) {
            if fe.tap_count == 2 {
                cx.widget_action(self.uid, PartyWallAction::DoubleClicked);
            }
        }
        if self.next.is_event(event).is_some() {
            let now = Cx::monotonic_now();
            let mut changed = false;
            let look = now - self.last_sync >= SYNC_EVERY;
            if look {
                self.last_sync = now;
                changed |= self.sync(now);
            }
            changed |= self.sync_live(cx, now, look);
            if let Some(frames) = &mut self.frames {
                if frames.drain(cx) {
                    changed = true;
                    if frames.pending() == 0 {
                        log!("party: the screen holds {:.1} MB of pictures", frames.bytes() as f64 / (1 << 20) as f64);
                    }
                }
            }
            if !self.live_on {
                changed |= self.advance(now);
                if !self.tiles.is_empty() && now - self.last_change > IDLE_AFTER {
                    self.upgrade(now);
                }
            }
            let step = self.step(now);
            if changed || step != self.drawn_step {
                self.drawn_step = step;
                self.redraw(cx);
            }
            self.next = cx.new_next_frame();
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        let now = Cx::monotonic_now();
        self.draw_bg.draw_abs(cx, rect);
        if let Some(message) = self.message.clone() {
            self.draw_note.draw_abs(cx, dvec2(rect.pos.x + 32.0, rect.pos.y + rect.size.y * 0.45), &message);
            return DrawStep::done();
        }
        let dpi = cx.current_dpi_factor();
        let size = Size::Fit((rect.size.x * dpi).floor().max(1.0) as u32, (rect.size.y * dpi).floor().max(1.0) as u32);
        self.decode(size);
        match self.live_texture.clone().filter(|_| self.live_on) {
            Some(texture) => {
                self.draw_tile.image_scale = vec2(1.0, 1.0);
                self.draw_tile.image_pan = vec2(0.0, 0.0);
                self.draw_tile.draw_vars.set_texture(0, &texture);
                self.draw_tile.draw_abs(cx, fit(rect, self.live_aspect));
            }
            None => {
                self.draw_shown(cx, rect, now, size);
            }
        }
        let side = (rect.size.y * 0.16).clamp(96.0, 200.0);
        self.draw_qr_code(cx, dvec2(rect.pos.x + rect.size.x - side - QR_MARGIN, rect.pos.y + QR_MARGIN), side);
        DrawStep::done()
    }
}
