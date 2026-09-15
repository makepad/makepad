//! Phone shell state and geometry. Application viewports remain stable while
//! their compositor surfaces move between home, foreground and the task viewer.
use crate::{desktop::DesktopStyle, hub::ClientId, mobile_tiles::{HomeOrder, HomeTiles, Slot}};
use makepad_widgets::makepad_platform::SafeAreaInsets;
use makepad_widgets::*;
use std::collections::HashMap;

/// What the phone shell draws AROUND the apps, decided once for the build
/// (`PhoneChrome::for_host` at startup). The desktop's iOS/Android skin is
/// a whole phone: a fake status bar (clock, wifi, battery), the island or
/// camera dot, the home indicator, and the simulator's controls strip
/// (style menu, Desktop, appearance, rotate). A real phone has all of that
/// from its OS — it draws none of it and lays out against the window's
/// real safe-area insets (the status bar / island above, the home
/// indicator below), which the platform reports with every window
/// geometry and the shell keeps current (`set_insets`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum PhoneChrome {
    /// A desktop window playing a phone.
    #[default]
    Simulated,
    /// The phone itself, with the insets its OS reported.
    Device { insets: SafeAreaInsets },
}

impl PhoneChrome {
    pub fn for_host(device: bool, insets: SafeAreaInsets) -> Self {
        if device { PhoneChrome::Device { insets } } else { PhoneChrome::Simulated }
    }
    /// The fake status bar: clock, wifi, battery, the island / camera dot.
    pub fn fake_status(&self) -> bool { matches!(self, PhoneChrome::Simulated) }
    /// The fake home indicator pill.
    pub fn fake_indicator(&self) -> bool { matches!(self, PhoneChrome::Simulated) }
    /// The desktop simulator's controls strip above the phone.
    pub fn controls_strip(&self) -> bool { matches!(self, PhoneChrome::Simulated) }
    /// A new window geometry: the device's insets follow it; the desktop
    /// skin's fake bars never change.
    pub fn set_insets(&mut self, insets: SafeAreaInsets) {
        if let PhoneChrome::Device { insets: current } = self { *current = insets; }
    }
    /// What the top of the screen reserves: the fake status bar (42 in
    /// portrait, 24 in landscape) or the real top inset.
    pub fn top_reserve(&self, screen: Rect) -> f64 {
        match self {
            PhoneChrome::Simulated => if screen.size.x > screen.size.y { 24.0 } else { 42.0 },
            PhoneChrome::Device { insets } => insets.top,
        }
    }
    /// What the bottom reserves: the fake home indicator strip (24) or the
    /// real bottom inset.
    pub fn bottom_reserve(&self, _screen: Rect) -> f64 {
        match self {
            PhoneChrome::Simulated => 24.0,
            PhoneChrome::Device { insets } => insets.bottom,
        }
    }
    /// The shell's home gesture zone: the bottom reserve plus a few points
    /// of slack. A press here is the shell's (a tap goes Home, a swipe up
    /// goes Home or to Recents), never the app's — on the phone that is
    /// the strip under the real home indicator, whose first swipe the OS
    /// defers to the app (`Cx::defer_system_gestures`).
    pub fn bottom_zone(&self, screen: Rect, p: Vec2d) -> bool {
        p.y > screen.pos.y + screen.size.y - self.bottom_reserve(screen) - 4.0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PhoneScreen { #[default] Home, App, Recents, Drawer }

#[derive(Clone, Debug, PartialEq)]
pub enum PhoneHit {
    App(String), Card(ClientId), Home, Recents, Drawer, Back,
    Rotate, Style, Appearance, Desktop, Key(String), Shift, Symbols, HideKeyboard,
    ClearSearch, CancelSearch,
    /// Edit mode's "Done" pill.
    Done,
}

/// How far past its ends a paged scroller or the library follows the
/// finger: a third of the overshoot, the phone's rubber band.
pub const RUBBER: f64 = 0.35;
/// A sideways flick this fast (pages per second) flips one page on
/// release whatever the drag distance was.
pub const FLICK_PAGES_PER_SEC: f64 = 1.2;
/// A vertical flick this fast (points per second) completes the library's
/// open or close.
pub const DRAWER_FLICK_SPEED: f64 = 700.0;

/// Rubber band: inside `[0, max]` the value is the finger's; beyond, only
/// a fraction of the overshoot.
pub fn rubber_band(raw: f64, max: f64) -> f64 {
    if raw < 0.0 {
        raw * RUBBER
    } else if raw > max {
        max + (raw - max) * RUBBER
    } else {
        raw
    }
}

/// A paged scroller the finger drives — the switcher's cards, and the
/// same shape for any paged surface. While dragging the page follows the
/// finger 1:1 (rubber band past the ends); on release it flips to the
/// nearest page, or exactly one page in the flick's direction when the
/// finger was fast, and eases there once. A settled scroller requests no
/// frames (`step` returns false).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paging {
    /// The page shown, fractional between pages.
    pub page: f64,
    target: f64,
    /// A drag in flight: the page it started on, the finger's travel so far.
    drag: Option<(f64, f64)>,
}

impl Default for Paging {
    fn default() -> Self {
        Paging { page: 0.0, target: 0.0, drag: None }
    }
}

impl Paging {
    fn max(count: usize) -> f64 {
        count.saturating_sub(1) as f64
    }
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }
    pub fn settled(&self) -> bool {
        self.drag.is_none() && self.page == self.target
    }
    /// The finger lands; the page it holds is the drag's origin.
    pub fn drag_begin(&mut self) {
        self.drag = Some((self.page, 0.0));
        self.target = self.page;
    }
    /// The finger moved `dx` points (right positive) over pages `width`
    /// points wide: the page follows, the ends rubber-band.
    pub fn drag_move(&mut self, dx: f64, width: f64, count: usize) {
        let Some((start, travel)) = self.drag.as_mut() else { return };
        *travel += dx;
        let raw = *start - *travel / width.max(1.0);
        self.page = rubber_band(raw, Self::max(count));
    }
    /// The finger lifts at `vx` points per second (right positive): a flick
    /// flips one page its way, anything slower settles on the nearest.
    pub fn drag_end(&mut self, vx: f64, width: f64, count: usize) {
        self.drag = None;
        let flick = vx / width.max(1.0);
        let target = if flick < -FLICK_PAGES_PER_SEC {
            self.page.floor() + 1.0
        } else if flick > FLICK_PAGES_PER_SEC {
            self.page.ceil() - 1.0
        } else {
            self.page.round()
        };
        self.target = target.clamp(0.0, Self::max(count));
    }
    /// A wheel or key step: one page, eased.
    pub fn flip(&mut self, by: i64, count: usize) {
        if self.drag.is_some() {
            return;
        }
        self.target = (self.target.round() + by as f64).clamp(0.0, Self::max(count));
    }
    pub fn set(&mut self, page: f64) {
        self.page = page;
        self.target = page;
        self.drag = None;
    }
    /// The page it is going to (or holds).
    pub fn target(&self) -> f64 {
        self.target
    }
    /// Go to `page`, eased; a drag in flight is dropped.
    pub fn set_target(&mut self, page: f64, count: usize) {
        self.drag = None;
        self.target = page.clamp(0.0, Self::max(count));
    }
    /// One eased step toward the settled page; true while still moving.
    pub fn step(&mut self, dt: f64) -> bool {
        if self.drag.is_some() {
            return false;
        }
        let t = 1.0 - (-dt * 14.0).exp();
        self.page += (self.target - self.page) * t;
        if (self.target - self.page).abs() < 0.0005 {
            self.page = self.target;
        }
        self.page != self.target
    }
}

/// A finger resting this long without moving more than
/// [`LONG_PRESS_SLOP`] is a long press: on a home icon it opens edit mode.
pub const LONG_PRESS_SECS: f64 = 0.5;
pub const LONG_PRESS_SLOP: f64 = 8.0;
/// Edit mode's jiggle: every icon rocks this far either way, at this rate,
/// each with its own phase.
pub const JIGGLE_DEGREES: f64 = 2.0;
pub const JIGGLE_HZ: f64 = 1.7;
/// An app opens out of (and closes into) its icon or tile in this long:
/// the openness ease, and the fade a crossfade tile app matches.
pub const OPEN_SECS: f64 = 0.25;
/// How long after the last interaction the phone keeps painting (the
/// wallpaper's drift, a settling ease) before it rests.
pub const INTERACTION_TAIL_SECS: f64 = 1.0;

#[derive(Clone)]
pub struct PhoneGesture {
    pub start: Vec2d,
    pub last: Vec2d,
    pub time: f64,
    /// The farthest the finger has been from where it landed.
    pub max_travel: f64,
    /// The long press fired (edit mode opened, or a drag began).
    pub long_pressed: bool,
    /// Decided once the finger has travelled: a sideways pan of the home
    /// pages (true) or not (false).
    pub pan: Option<bool>,
    /// When the finger last moved, and how fast it was going then (points
    /// per second, up is negative): what tells a flick from a swipe-and-hold.
    pub last_time: f64,
    pub vy: f64,
    pub vx: f64,
    pub hit: Option<PhoneHit>,
    pub bottom: bool,
    pub edge: bool,
    pub screen: PhoneScreen,
}

impl PhoneGesture {
    pub fn new(p: Vec2d, time: f64, hit: Option<PhoneHit>, bottom: bool, edge: bool, screen: PhoneScreen) -> Self {
        PhoneGesture { start: p, last: p, time, max_travel: 0.0, long_pressed: false, pan: None, last_time: time, vy: 0.0, vx: 0.0, hit, bottom, edge, screen }
    }
    /// The finger moved to `p`: the farthest travel is remembered, so a
    /// finger that wandered and came back is no long press.
    pub fn track(&mut self, p: Vec2d) {
        self.max_travel = self.max_travel.max((p - self.start).length());
    }
    /// A long press is due: held [`LONG_PRESS_SECS`] within the slop and
    /// not fired yet. The caller marks it fired.
    pub fn long_press_due(&self, now: f64) -> bool {
        !self.long_pressed && self.max_travel <= LONG_PRESS_SLOP && now - self.time >= LONG_PRESS_SECS
    }
}

/// An icon in flight during edit mode: which one, the slot it holds right
/// now (the order already reflects it), where the finger is and how far
/// the finger is from the icon's centre, so the icon never jumps under it.
#[derive(Clone, Debug, PartialEq)]
pub struct IconDrag {
    pub id: String,
    pub slot: Slot,
    pub pos: Vec2d,
    pub grab: Vec2d,
}

/// Edit mode ("jiggle"): every icon rocks, the pressed one lifts and
/// follows the finger, the others shift aside live. An animation that
/// lasts exactly as long as the mode: `since` is its clock's origin and
/// `lift` eases the held icon up and back down.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HomeEdit {
    pub active: bool,
    pub since: f64,
    pub drag: Option<IconDrag>,
    pub lift: f64,
}

impl HomeEdit {
    /// The icon at `index`'s tilt at `now`, in degrees.
    pub fn jiggle(&self, index: usize, now: f64) -> f64 {
        if !self.active { return 0.0; }
        let t = now - self.since;
        JIGGLE_DEGREES * (t * JIGGLE_HZ * std::f64::consts::TAU + index as f64 * 1.9).sin()
    }
}

/// How long a finger must rest before release for an upward bottom swipe
/// to mean "hold" (the switcher) rather than a flick home.
pub const SWIPE_HOLD_SECS: f64 = 0.15;
/// Points per second upward at release that make a flick.
pub const SWIPE_FLICK_SPEED: f64 = 400.0;

/// Where an upward swipe from the bottom edge goes, the way a phone decides
/// it: a FLICK (still moving fast at release, or released far up the
/// screen, or short and quick) goes home; a swipe the finger HELD before
/// letting go opens the switcher; from the switcher itself any upward
/// swipe goes home; on Android's home page it opens the drawer. `None`
/// when the swipe is too short to mean anything (a tap on the strip is
/// decided by the caller).
pub fn bottom_swipe_target(
    from: PhoneScreen,
    android: bool,
    delta: Vec2d,
    screen_height: f64,
    duration: f64,
    held: bool,
    vy: f64,
) -> Option<PhoneHit> {
    if delta.y >= -45.0 {
        return None;
    }
    if from == PhoneScreen::Home && android {
        return Some(PhoneHit::Drawer);
    }
    if from == PhoneScreen::Recents {
        return Some(PhoneHit::Home);
    }
    let far = delta.y < -screen_height * 0.33;
    let quick = duration < 0.30 && delta.y < -100.0;
    let flick = vy < -SWIPE_FLICK_SPEED;
    Some(if far || (!held && (flick || quick)) { PhoneHit::Home } else { PhoneHit::Recents })
}

#[derive(Clone)]
pub struct PhoneState {
    pub clock: String,
    pub wallpaper_time: f64,
    pub screen: PhoneScreen,
    pub client: Option<ClientId>,
    pub order: Vec<ClientId>,
    pub openness: f64,
    pub overview: f64,
    /// The switcher's cards, paged by the finger.
    pub cards: Paging,
    /// The home pager: the home page(s) then the App Library as the last
    /// page, panned by the finger (the drawer on Android, driven by a
    /// vertical drag through the same pager).
    pub pager: Paging,
    pub home_pages: usize,
    /// The App Library / drawer's arrival, derived from the pager every
    /// step: 0 = the last home page, 1 = the library, the finger's own
    /// value in between (rubber past either end).
    pub drawer: f64,
    /// The home page's icon order and the dock, as arranged (mobile_tiles
    /// `HomeOrder`); `home_order_loaded` once storage answered (or had
    /// nothing), so a first-run default is never saved over a real one.
    pub home: HomeOrder,
    pub home_order_loaded: bool,
    pub edit: HomeEdit,
    /// When the person last touched the phone: the paint clock keeps
    /// running [`INTERACTION_TAIL_SECS`] past it, then rests.
    pub last_interaction: f64,
    pub dismiss_y: f64,
    pub gesture: Option<PhoneGesture>,
    /// Native touch owned by shell navigation; other fingers cannot replace it.
    pub touch: Option<u64>,
    pub keyboard: f64,
    pub keyboard_target: f64,
    pub keyboard_sent_height: f64,
    pub keyboard_client: Option<ClientId>,
    pub search_query: String,
    pub search_focused: bool,
    pub search_scroll: f64,
    pub ime: HashMap<ClientId, makepad_platform::ime::HostedImeState>,
    pub shift: bool,
    pub symbols: bool,
    pub desktop_size: Option<Vec2d>,
    pub desktop_clients: Vec<ClientId>,
    pub desktop_style: DesktopStyle,
    pub viewport: Rect,
    /// What the shell draws around the apps (see [`PhoneChrome`]).
    pub chrome: PhoneChrome,
    /// The desk painted the device's status-bar band from the foreground
    /// app's own frame (its top rows stretched up), so a dark app never
    /// sits under a light strip; the overlay then leaves the band alone.
    pub band_from_app: bool,
    /// The home page's live app tiles (mobile_tiles.rs): which client shows
    /// which tile and in which face.
    pub tiles: HomeTiles,
}
impl Default for PhoneState {
    fn default() -> Self {
        Self { clock: "9:41".into(), wallpaper_time: 0.0, screen: PhoneScreen::Home, client: None, order: Vec::new(),
            openness: 0.0, overview: 0.0, cards: Paging::default(), pager: Paging::default(), home_pages: 1, drawer: 0.0,
            home: HomeOrder::default(), home_order_loaded: false, edit: HomeEdit::default(), last_interaction: 0.0, dismiss_y: 0.0, gesture: None, touch: None,
            keyboard: 0.0, keyboard_target: 0.0, keyboard_sent_height: 0.0, keyboard_client: None,
            search_query: String::new(), search_focused: false, search_scroll: 0.0,
            ime: HashMap::new(), shift: false, symbols: false,
            desktop_size: None, desktop_clients: Vec::new(), desktop_style: DesktopStyle::Omarchy, viewport: Rect::default(),
            chrome: PhoneChrome::default(), band_from_app: false,
            tiles: HomeTiles::default() }
    }
}
impl PhoneState {
    /// The home page (or the app library) is fully shown and nothing is
    /// animating or being dragged: safe to reconfigure a window down to
    /// its tile face without disturbing a closing animation.
    pub fn home_settled(&self) -> bool {
        matches!(self.screen, PhoneScreen::Home | PhoneScreen::Drawer)
            && self.gesture.is_none()
            && self.openness < 0.001
            && self.overview < 0.001
    }
    /// The client the person is looking at full screen (the open app, from
    /// the first frame of its zoom-in until it is dismissed).
    pub fn foreground(&self) -> Option<ClientId> {
        if self.screen == PhoneScreen::App { self.client } else { None }
    }
    /// The home page is on screen at all (tiles need drawing and driving).
    pub fn home_visible(&self) -> bool {
        self.screen != PhoneScreen::App || self.openness < 0.999
    }
    pub fn activate(&mut self, client: ClientId) {
        self.search_focused = false;
        if self.client != Some(client) { self.keyboard_target = 0.0; }
        self.client = Some(client);
        self.order.retain(|c| *c != client);
        self.order.insert(0, client);
        self.cards.set(0.0);
        self.leave_edit();
        self.pager.set_target(self.pager.target().min(self.last_home_page()), self.page_count());
        self.screen = PhoneScreen::App;
        self.dismiss_y = 0.0;
    }
    pub fn navigate(&mut self, screen: PhoneScreen) {
        self.search_focused = false;
        self.screen = screen;
        self.keyboard_target = 0.0;
        self.gesture = None;
        self.dismiss_y = 0.0;
        if screen != PhoneScreen::Home { self.leave_edit(); }
        let count = self.page_count();
        let page = if screen == PhoneScreen::Drawer { self.library_page() } else { self.pager.target().round().min(self.last_home_page()) };
        self.pager.set_target(page, count);
    }
    /// The pager's pages: the home page(s), then the library.
    pub fn page_count(&self) -> usize {
        self.home_pages.max(1) + 1
    }
    pub fn library_page(&self) -> f64 {
        self.home_pages.max(1) as f64
    }
    pub fn last_home_page(&self) -> f64 {
        self.library_page() - 1.0
    }
    /// The finger lands on the home page or the library: the pager holds
    /// its page until the finger lifts.
    pub fn library_drag_begin(&mut self) {
        self.pager.drag_begin();
    }
    /// The finger moved `d` points along the pan (right positive on iOS,
    /// where the pages pan sideways; down positive on Android, whose drawer
    /// is a sheet), over pages `extent` points wide: the pages follow 1:1,
    /// the ends rubber-band, `drawer` follows the pager.
    pub fn library_drag_move(&mut self, d: f64, extent: f64) {
        let count = self.page_count();
        self.pager.drag_move(d, extent, count);
        self.drawer = self.pager.page - self.last_home_page();
    }
    /// The finger lifts at `v` points per second along the pan: a flick
    /// flips one page its way, anything slower settles on the nearest;
    /// the screen follows the page it settles on. Eased from here.
    pub fn library_release(&mut self, v: f64, extent: f64) {
        let count = self.page_count();
        self.pager.drag_end(v, extent, count);
        let screen = if self.pager.target() >= self.library_page() - 0.5 { PhoneScreen::Drawer } else { PhoneScreen::Home };
        self.search_focused = false;
        self.screen = screen;
        self.keyboard_target = 0.0;
        self.dismiss_y = 0.0;
        if screen != PhoneScreen::Home { self.leave_edit(); }
    }
    /// Edit mode opens (a long press on an icon): the jiggle clock starts
    /// at `now`.
    pub fn enter_edit(&mut self, now: f64) {
        if !self.edit.active {
            self.edit = HomeEdit { active: true, since: now, drag: None, lift: 0.0 };
        }
    }
    pub fn leave_edit(&mut self) {
        self.edit.active = false;
        self.edit.drag = None;
    }
    /// The held icon follows the finger; the slot under the finger becomes
    /// its own, the others shifting aside. True when the order changed.
    pub fn drag_icon_to(&mut self, p: Vec2d, slot: Option<Slot>) -> bool {
        let Some(drag) = self.edit.drag.as_mut() else { return false };
        drag.pos = p;
        let Some(slot) = slot else { return false };
        if slot == drag.slot { return false; }
        let from = drag.slot;
        if self.home.move_to(from, slot) {
            if let Some(drag) = self.edit.drag.as_mut() { drag.slot = slot; }
            true
        } else {
            false
        }
    }
    pub fn step(&mut self, dt: f64) -> bool {
        let t = 1.0 - (-dt * 19.0).exp();
        // Recents keeps whatever openness it was entered with: 1 from an
        // app (its card is the app pulled in, and tapping it opens it
        // back up), 0 from Home (cards only — nothing to pull, nothing to
        // wobble while the switcher fades in).
        let open = match self.screen {
            PhoneScreen::App if self.client.is_some() => 1.0,
            PhoneScreen::Recents if self.client.is_some() && self.openness > 0.5 => 1.0,
            _ => 0.0,
        };
        let overview = if self.screen == PhoneScreen::Recents { 1.0 } else { 0.0 };
        let mut active = false;
        let dragging = self.gesture.as_ref().is_some_and(|g| g.bottom);
        for (value, target) in [(&mut self.openness, open), (&mut self.overview, overview)] {
            if !dragging {
                *value += (target - *value) * t;
                if (*value - target).abs() < 0.001 { *value = target; }
                active |= *value != target;
            }
        }
        self.keyboard += (self.keyboard_target - self.keyboard) * t;
        if (self.keyboard_target - self.keyboard).abs() < 0.25 { self.keyboard = self.keyboard_target; }
        active |= self.keyboard != self.keyboard_target;
        active |= self.cards.step(dt);
        active |= self.pager.step(dt);
        self.drawer = self.pager.page - self.last_home_page();
        // Edit mode is an animation for as long as it lasts: the jiggle
        // asks for frames, the held icon's lift eases up and back down.
        let lift = if self.edit.active && self.edit.drag.is_some() { 1.0 } else { 0.0 };
        self.edit.lift += (lift - self.edit.lift) * (1.0 - (-dt * 18.0).exp());
        if (self.edit.lift - lift).abs() < 0.002 { self.edit.lift = lift; }
        active |= self.edit.active || self.edit.lift != lift;
        active
    }
    /// A bottom drag in flight, `dy` points up the screen (negative). From
    /// a full-screen app the app pulls into its card (`openness` stays 1,
    /// `overview` follows the finger); from Home or the library only the
    /// switcher fades in with the finger — nothing is pulled, `openness`
    /// stays 0 — so a release into Recents has its cards already there.
    pub fn bottom_drag(&mut self, from: PhoneScreen, dy: f64, screen_height: f64) {
        self.overview = (-dy / (screen_height * 0.42)).clamp(0.0, 1.0);
        if from == PhoneScreen::App {
            self.openness = 1.0;
        }
    }
    pub fn accepts_app_input(&self) -> bool {
        self.screen == PhoneScreen::App && self.gesture.is_none()
            && self.overview < 0.01 && self.openness > 0.99
    }
    pub fn keyboard_height(&self) -> f64 {
        if self.viewport.size.x > self.viewport.size.y { 184.0 } else { 292.0 }
    }
    pub fn searching(&self) -> bool {
        self.screen == PhoneScreen::Drawer && (self.search_focused || !self.search_query.is_empty())
    }
    /// The phone still needs frames at `now`: something eases, a finger is
    /// down, edit mode jiggles, or the interaction tail has not run out.
    /// After that the phone rests and paints nothing until touched.
    pub fn wants_frames(&self, now: f64, moving: bool) -> bool {
        moving || self.gesture.is_some() || self.edit.active || now - self.last_interaction < INTERACTION_TAIL_SECS
    }
}

pub fn phone_size(style: DesktopStyle) -> Vec2d {
    if style == DesktopStyle::Ios { dvec2(402.0, 874.0) } else { dvec2(412.0, 892.0) }
}
/// The viewport an open app gets: the screen minus what the chrome
/// reserves above (status bar) and below (home indicator).
pub fn app_rect(screen: Rect, chrome: PhoneChrome) -> Rect {
    let top = chrome.top_reserve(screen);
    let bottom = chrome.bottom_reserve(screen);
    Rect { pos: screen.pos + dvec2(0.0, top), size: dvec2(screen.size.x, (screen.size.y - top - bottom).max(1.0)) }
}
pub fn card_rect(screen: Rect, chrome: PhoneChrome, index: f64, page: f64) -> Rect {
    let app = app_rect(screen, chrome);
    let scale = if screen.size.x > screen.size.y { 0.74 } else { 0.76 };
    let size = app.size * scale;
    Rect { pos: app.pos + (app.size - size) * 0.5 + dvec2((index-page)*(size.x+22.0), -4.0), size }
}
pub fn mix_rect(a: Rect, b: Rect, t: f64) -> Rect {
    Rect { pos: a.pos + (b.pos-a.pos)*t, size: a.size + (b.size-a.size)*t }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_orientations_reserve_system_bars_and_keep_selected_card_inside() {
        for size in [phone_size(DesktopStyle::Ios), phone_size(DesktopStyle::Android)] {
            for size in [size, dvec2(size.y, size.x)] {
                let screen = Rect { pos: dvec2(0.0, 32.0), size: size-dvec2(0.0,32.0) };
                let app = app_rect(screen, PhoneChrome::Simulated);
                let card = card_rect(screen, PhoneChrome::Simulated, 2.0, 2.0);
                assert!(app.size.x > 0.0 && app.size.y > 200.0);
                assert!(app.pos.y > screen.pos.y);
                assert!(card.pos.x >= app.pos.x && card.pos.y >= app.pos.y);
                assert!(card.pos.x+card.size.x <= app.pos.x+app.size.x);
                assert!(card.pos.y+card.size.y <= app.pos.y+app.size.y);
            }
        }
    }
    #[test]
    fn the_desktop_skin_fakes_the_phone_and_the_device_draws_none_of_it() {
        let portrait = Rect { pos: dvec2(0.0, 0.0), size: phone_size(DesktopStyle::Ios) };
        let landscape = Rect { pos: dvec2(0.0, 0.0), size: dvec2(portrait.size.y, portrait.size.x) };
        let phone = SafeAreaInsets { top: 59.0, right: 0.0, bottom: 34.0, left: 0.0 };
        // The desktop skin: every fake, the 42/24 reserve, insets ignored.
        let sim = PhoneChrome::for_host(false, phone);
        assert_eq!(sim, PhoneChrome::Simulated);
        assert_eq!(sim, PhoneChrome::default(), "before startup reads the host: the desktop");
        assert!(sim.fake_status() && sim.fake_indicator() && sim.controls_strip());
        let app = app_rect(portrait, sim);
        assert_eq!((app.pos.y, app.size.y), (42.0, portrait.size.y - 42.0 - 24.0));
        assert_eq!(app_rect(landscape, sim).pos.y, 24.0);
        let mut still = sim;
        still.set_insets(phone);
        assert_eq!(still, sim, "a simulated phone never takes insets");
        // The phone: nothing fake, the OS's own insets, kept current.
        let mut dev = PhoneChrome::for_host(true, phone);
        assert!(!dev.fake_status() && !dev.fake_indicator() && !dev.controls_strip());
        let app = app_rect(portrait, dev);
        assert_eq!((app.pos.y, app.size.y), (59.0, portrait.size.y - 59.0 - 34.0));
        let turned = SafeAreaInsets { top: 0.0, right: 59.0, bottom: 21.0, left: 59.0 };
        dev.set_insets(turned);
        let app = app_rect(landscape, dev);
        assert_eq!((app.pos.y, app.size.y), (0.0, landscape.size.y - 21.0));
        assert!(card_rect(landscape, dev, 0.0, 0.0).pos.y >= app.pos.y);
    }

    #[test]
    fn the_home_gesture_zone_is_the_bottom_reserve_on_both_phones() {
        let screen = Rect { pos: dvec2(0.0, 0.0), size: phone_size(DesktopStyle::Ios) };
        let bottom = screen.size.y;
        let at = |y: f64| dvec2(200.0, y);
        // The phone: a touch that starts AT the edge, or anywhere under the
        // real 34 pt home indicator (plus 4 of slack), is the shell's.
        let dev = PhoneChrome::for_host(true, SafeAreaInsets { top: 59.0, right: 0.0, bottom: 34.0, left: 0.0 });
        assert!(dev.bottom_zone(screen, at(bottom - 1.0)));
        assert!(dev.bottom_zone(screen, at(bottom - 37.0)));
        assert!(!dev.bottom_zone(screen, at(bottom - 40.0)));
        // The desktop skin: its fake 24 pt indicator strip, plus the same slack.
        let sim = PhoneChrome::Simulated;
        assert!(sim.bottom_zone(screen, at(bottom - 1.0)));
        assert!(sim.bottom_zone(screen, at(bottom - 27.0)));
        assert!(!sim.bottom_zone(screen, at(bottom - 30.0)));
        // The zone follows the window, not the origin.
        let shifted = Rect { pos: dvec2(80.0, 60.0), size: screen.size };
        assert!(dev.bottom_zone(shifted, dvec2(200.0, 60.0 + bottom - 1.0)));
        assert!(!dev.bottom_zone(shifted, dvec2(200.0, bottom - 1.0)));
    }

    #[test]
    fn a_bottom_swipe_flicks_home_holds_for_the_switcher_and_always_leaves_it() {
        let h = 874.0;
        let up = |y: f64| dvec2(0.0, y);
        // Too short to mean anything.
        assert_eq!(bottom_swipe_target(PhoneScreen::App, false, up(-30.0), h, 0.1, false, -900.0), None);
        // A real finger: 220 pt in 0.45 s, still moving at release — home.
        assert_eq!(bottom_swipe_target(PhoneScreen::App, false, up(-220.0), h, 0.45, false, -700.0), Some(PhoneHit::Home));
        // The same distance, but the finger rested before letting go — the switcher.
        assert_eq!(bottom_swipe_target(PhoneScreen::App, false, up(-220.0), h, 0.45, true, 0.0), Some(PhoneHit::Recents));
        // Released far up the screen: home, held or not.
        assert_eq!(bottom_swipe_target(PhoneScreen::App, false, up(-400.0), h, 1.2, true, 0.0), Some(PhoneHit::Home));
        // Short and quick (the bridge's injected swipe): home.
        assert_eq!(bottom_swipe_target(PhoneScreen::App, false, up(-120.0), h, 0.1, false, 0.0), Some(PhoneHit::Home));
        // Slow, short, not moving: the switcher.
        assert_eq!(bottom_swipe_target(PhoneScreen::App, false, up(-80.0), h, 0.6, false, -100.0), Some(PhoneHit::Recents));
        // From the switcher every upward swipe leaves it.
        assert_eq!(bottom_swipe_target(PhoneScreen::Recents, false, up(-60.0), h, 0.9, true, 0.0), Some(PhoneHit::Home));
        // Android's home page opens the drawer instead.
        assert_eq!(bottom_swipe_target(PhoneScreen::Home, true, up(-200.0), h, 0.1, false, -900.0), Some(PhoneHit::Drawer));
    }

    #[test]
    fn pages_follow_the_finger_flip_one_on_a_flick_and_settle() {
        let (width, count) = (300.0, 4);
        let mut cards = Paging::default();
        // The page follows the finger 1:1.
        cards.drag_begin();
        cards.drag_move(-150.0, width, count);
        assert!((cards.page - 0.5).abs() < 1e-9);
        // A slow release settles on the nearest page, eased, then rests.
        cards.drag_end(-50.0, width, count);
        let mut frames = 0;
        while cards.step(1.0 / 60.0) { frames += 1; assert!(frames < 200); }
        assert_eq!(cards.page, 1.0);
        assert!(cards.settled() && frames > 3, "one ease, not a snap: {frames} frames");
        assert!(!cards.step(1.0 / 60.0), "settled pages request no frames");
        // A short drag with a fast flick advances exactly one page.
        cards.drag_begin();
        cards.drag_move(-20.0, width, count);
        cards.drag_end(-2000.0, width, count);
        while cards.step(1.0 / 60.0) {}
        assert_eq!(cards.page, 2.0);
        // A flick back goes one page back, never several.
        cards.drag_begin();
        cards.drag_move(30.0, width, count);
        cards.drag_end(3000.0, width, count);
        while cards.step(1.0 / 60.0) {}
        assert_eq!(cards.page, 1.0);
        // The ends rubber-band and settle back.
        cards.set(0.0);
        cards.drag_begin();
        cards.drag_move(300.0, width, count);
        assert!(cards.page < 0.0 && cards.page > -0.5, "a third of the overshoot: {}", cards.page);
        cards.drag_end(0.0, width, count);
        while cards.step(1.0 / 60.0) {}
        assert_eq!(cards.page, 0.0);
        cards.set(3.0);
        cards.drag_begin();
        cards.drag_move(-600.0, width, count);
        assert!(cards.page > 3.0 && cards.page < 3.8);
        cards.drag_end(-5000.0, width, count);
        while cards.step(1.0 / 60.0) {}
        assert_eq!(cards.page, 3.0, "a flick past the last page stays on it");
    }

    #[test]
    fn the_library_pans_with_the_finger_and_settles_by_distance_or_flick() {
        let width = 402.0;
        let mut phone = PhoneState::default();
        assert_eq!((phone.page_count(), phone.library_page()), (2, 1.0));
        assert_eq!(phone.drawer, 0.0);
        // The finger drags the page left: the library follows 1:1.
        phone.library_drag_begin();
        phone.library_drag_move(-120.6, width);
        assert!((phone.drawer - 0.3).abs() < 1e-9 && phone.pager.dragging());
        // Held: stepping does not move it.
        phone.step(1.0 / 60.0);
        assert!((phone.drawer - 0.3).abs() < 1e-9);
        // Released short and slow: back home, eased.
        phone.library_release(-50.0, width);
        assert_eq!(phone.screen, PhoneScreen::Home);
        let mut frames = 0;
        while phone.step(1.0 / 60.0) { frames += 1; assert!(frames < 200); }
        assert_eq!(phone.drawer, 0.0);
        assert!(frames > 3, "one ease, not a snap: {frames} frames");
        // A flick completes the open whatever the distance.
        phone.library_drag_begin();
        phone.library_drag_move(-30.0, width);
        phone.library_release(-1500.0, width);
        assert_eq!(phone.screen, PhoneScreen::Drawer);
        while phone.step(1.0 / 60.0) {}
        assert_eq!(phone.drawer, 1.0);
        // Past the library only the rubber band's share; released, it settles back on it.
        phone.library_drag_begin();
        phone.library_drag_move(-200.0, width);
        assert!(phone.drawer > 1.0 && phone.drawer < 1.2, "{}", phone.drawer);
        phone.library_release(0.0, width);
        while phone.step(1.0 / 60.0) {}
        assert_eq!((phone.screen, phone.drawer), (PhoneScreen::Drawer, 1.0));
        // Coming back: the same pan the other way, a flick right closes.
        phone.library_drag_begin();
        phone.library_drag_move(80.4, width);
        assert!((phone.drawer - 0.8).abs() < 1e-6);
        phone.library_release(1500.0, width);
        assert_eq!(phone.screen, PhoneScreen::Home);
        while phone.step(1.0 / 60.0) {}
        assert_eq!(phone.drawer, 0.0);
        assert!(!phone.step(1.0 / 60.0), "settled: no frames");
        // A tap on the page dots goes there eased, and Home brings it back.
        phone.navigate(PhoneScreen::Drawer);
        assert!(phone.step(1.0 / 60.0));
        while phone.step(1.0 / 60.0) {}
        assert_eq!(phone.drawer, 1.0);
        phone.navigate(PhoneScreen::Home);
        while phone.step(1.0 / 60.0) {}
        assert_eq!(phone.drawer, 0.0);
    }

    #[test]
    fn a_long_press_is_half_a_second_without_moving() {
        let mut g = PhoneGesture::new(dvec2(100.0, 100.0), 10.0, Some(PhoneHit::App("sheets".into())), false, false, PhoneScreen::Home);
        assert!(!g.long_press_due(10.3), "too soon");
        g.track(dvec2(104.0, 103.0));
        assert!(g.long_press_due(10.5), "held within the slop");
        g.long_pressed = true;
        assert!(!g.long_press_due(11.0), "fires once");
        let mut g = PhoneGesture::new(dvec2(100.0, 100.0), 10.0, None, false, false, PhoneScreen::Home);
        g.track(dvec2(112.0, 100.0));
        g.track(dvec2(100.0, 100.0));
        assert!(!g.long_press_due(11.0), "a finger that wandered and came back is a drag, not a press");
    }

    #[test]
    fn edit_mode_jiggles_moves_icons_live_and_rests_when_it_ends() {
        let mut phone = PhoneState::default();
        phone.home = HomeOrder { icons: vec!["sheets".into(), "clock".into(), "route".into()], dock: vec!["files".into()] };
        for _ in 0..80 { phone.step(1.0 / 60.0); }
        assert!(!phone.step(1.0 / 60.0), "at rest before");
        phone.enter_edit(5.0);
        assert!(phone.edit.active);
        assert!(phone.step(1.0 / 60.0), "edit mode asks for frames");
        // Every icon rocks within ±2°, each on its own phase.
        let a = phone.edit.jiggle(0, 5.2);
        let b = phone.edit.jiggle(1, 5.2);
        assert!(a.abs() <= JIGGLE_DEGREES + 1e-9 && b.abs() <= JIGGLE_DEGREES + 1e-9);
        assert!((a - b).abs() > 0.01, "phases differ");
        // Sheets is picked up and carried over the last slot: the others shift aside live.
        phone.edit.drag = Some(IconDrag { id: "sheets".into(), slot: Slot::Icon(0), pos: dvec2(0.0, 0.0), grab: dvec2(0.0, 0.0) });
        assert!(phone.drag_icon_to(dvec2(300.0, 0.0), Some(Slot::Icon(2))));
        assert_eq!(phone.home.icons, ["clock", "route", "sheets"]);
        assert_eq!(phone.edit.drag.as_ref().unwrap().slot, Slot::Icon(2));
        assert!(!phone.drag_icon_to(dvec2(310.0, 0.0), Some(Slot::Icon(2))), "same slot: nothing to do");
        assert!(!phone.drag_icon_to(dvec2(310.0, 0.0), None), "off the grid: the icon just follows");
        assert_eq!(phone.edit.drag.as_ref().unwrap().pos, dvec2(310.0, 0.0));
        // Into the dock beside Files.
        assert!(phone.drag_icon_to(dvec2(0.0, 800.0), Some(Slot::Dock(1))));
        assert_eq!((phone.home.icons.len(), phone.home.dock.as_slice()), (2, &["files".to_string(), "sheets".to_string()][..]));
        // The lift eases up while held, back down after the drop.
        for _ in 0..30 { phone.step(1.0 / 60.0); }
        assert!(phone.edit.lift > 0.9);
        phone.edit.drag = None;
        for _ in 0..60 { phone.step(1.0 / 60.0); }
        assert_eq!(phone.edit.lift, 0.0);
        assert!(phone.step(1.0 / 60.0), "still jiggling until Done");
        // Done: the animation ends with the mode; nothing asks for frames.
        phone.leave_edit();
        assert!(!phone.edit.active);
        assert_eq!(phone.edit.jiggle(0, 9.0), 0.0);
        assert!(!phone.step(1.0 / 60.0), "at rest after");
        assert!(!phone.wants_frames(20.0, false));
        phone.last_interaction = 20.0;
        assert!(phone.wants_frames(20.5, false) && !phone.wants_frames(21.5, false), "the tail runs a second");
        // Navigating away also ends it.
        phone.enter_edit(30.0);
        phone.navigate(PhoneScreen::Drawer);
        assert!(!phone.edit.active);
    }

    #[test]
    fn a_bottom_drag_on_home_never_raises_openness() {
        let mut phone = PhoneState::default();
        phone.activate(4);
        phone.navigate(PhoneScreen::Home);
        for _ in 0..80 { phone.step(1.0 / 60.0); }
        assert_eq!(phone.openness, 0.0);
        // The switcher fades in with the finger; the last app is not pulled.
        phone.bottom_drag(PhoneScreen::Home, -120.0, 874.0);
        assert!(phone.overview > 0.3 && phone.overview < 0.4);
        assert_eq!(phone.openness, 0.0);
        // Released into Recents from Home: cards only, openness stays down.
        phone.navigate(PhoneScreen::Recents);
        for _ in 0..80 { phone.step(1.0 / 60.0); }
        assert_eq!(phone.overview, 1.0);
        assert_eq!(phone.openness, 0.0);
        // From a full-screen app the same drag pulls the app into its card.
        phone.activate(4);
        for _ in 0..80 { phone.step(1.0 / 60.0); }
        phone.bottom_drag(PhoneScreen::App, -120.0, 874.0);
        assert_eq!(phone.openness, 1.0);
        assert!(phone.overview > 0.3);
        phone.navigate(PhoneScreen::Recents);
        for _ in 0..80 { phone.step(1.0 / 60.0); }
        assert_eq!((phone.overview, phone.openness), (1.0, 1.0));
    }

    #[test]
    fn home_and_task_switcher_preserve_instances_and_settle() {
        let mut phone = PhoneState::default();
        phone.activate(10); phone.activate(11); phone.activate(10);
        assert_eq!(phone.order, [10,11]);
        for screen in [PhoneScreen::Recents, PhoneScreen::Home, PhoneScreen::App] {
            phone.navigate(screen);
            for _ in 0..80 { phone.step(1.0/60.0); }
            assert_eq!(phone.client, Some(10));
            assert_eq!(phone.order.len(), 2);
            assert!(!phone.step(1.0/60.0));
        }
        assert!(phone.accepts_app_input());
    }
    #[test]
    fn compact_faces_wait_for_the_home_page_to_settle() {
        let mut phone = PhoneState::default();
        assert!(phone.home_settled() && phone.foreground().is_none());
        phone.activate(4);
        for _ in 0..80 { phone.step(1.0/60.0); }
        assert_eq!(phone.foreground(), Some(4));
        assert!(!phone.home_settled() && !phone.home_visible());
        phone.navigate(PhoneScreen::Home);
        assert_eq!(phone.foreground(), None, "dismissed: no longer the app the person looks at");
        assert!(!phone.home_settled(), "the window is still animating into its icon");
        assert!(phone.home_visible());
        for _ in 0..80 { phone.step(1.0/60.0); }
        assert!(phone.home_settled());
        phone.navigate(PhoneScreen::Recents);
        for _ in 0..80 { phone.step(1.0/60.0); }
        assert!(!phone.home_settled(), "Recents keeps every card in its full face");
    }
}
