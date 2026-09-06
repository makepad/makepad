//! Home-screen app tiles: the phone home shows Clock, Weather and Photos as
//! live compact faces of the same client that opens full screen. Nothing
//! here touches processes or windows; this is the geometry of the home page
//! and the per-client presentation bookkeeping the WM drives (which face was
//! asked for, whether the client has confirmed it with a frame of the right
//! size), so the rules are testable without a window manager running.
use crate::hub::ClientId;
use makepad_widgets::*;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileKind {
    /// One of the two square tiles on the first row.
    Small,
    /// The full-width tile under them (portrait) or the third column (landscape).
    Wide,
}

/// The apps that own a home tile, in tile order. Every id is a launcher
/// registry entry; the tile launches it through the same cargo path.
pub const TILE_APPS: [(&str, TileKind); 3] =
    [("clock", TileKind::Small), ("weather", TileKind::Small), ("photos", TileKind::Wide)];

pub fn is_tile_app(app: &str) -> bool {
    TILE_APPS.iter().any(|(id, _)| *id == app)
}

/// Screen margin around the home content (iOS and Android both use 16pt).
pub const HOME_MARGIN: f64 = 16.0;
/// Gap between two tiles and between a tile and the favorites grid.
pub const TILE_GAP: f64 = 14.0;
/// Corner radius of a tile capture on screen.
pub const TILE_RADIUS: f64 = 22.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileSlot {
    pub app: &'static str,
    pub kind: TileKind,
    pub rect: Rect,
}

/// The home page cut into its regions for one screen size.
#[derive(Clone, Debug, PartialEq)]
pub struct HomeLayout {
    pub tiles: Vec<TileSlot>,
    /// Where favorites (app icons) go; empty when nothing fits.
    pub favorites: Rect,
    pub columns: usize,
    /// Row height for favorites; 0 when no row fits.
    pub row_height: f64,
    /// How many favorite icons the page can show without touching the dock.
    pub capacity: usize,
    pub landscape: bool,
}

/// Cut the home page: two small tiles and a wide one under them in portrait,
/// three across in landscape, the favorites grid in what is left above the
/// dock. `top` is where content starts (below the status bar and, on
/// Android, the big clock). Pure geometry, so both orientations are tested.
pub fn home_layout(screen: Rect, top: f64, dock: Rect) -> HomeLayout {
    let landscape = screen.size.x > screen.size.y;
    let m = HOME_MARGIN;
    let left = screen.pos.x + m;
    let width = (screen.size.x - m * 2.0).max(1.0);
    let mut tiles = Vec::new();
    let tiles_bottom;
    if landscape {
        let w = ((width - TILE_GAP * 2.0) / 3.0).max(1.0);
        // Short enough that a row of favorites still fits above the dock.
        let h = (w * 0.56).min((dock.pos.y - top - 126.0).max(60.0)).max(1.0);
        for (index, (app, kind)) in TILE_APPS.iter().enumerate() {
            let x = left + index as f64 * (w + TILE_GAP);
            tiles.push(TileSlot { app, kind: *kind, rect: Rect { pos: dvec2(x, top), size: dvec2(w, h) } });
        }
        tiles_bottom = top + h;
    } else {
        let s = ((width - TILE_GAP) / 2.0).max(1.0);
        let mut y = top;
        for (index, (app, kind)) in TILE_APPS.iter().enumerate() {
            match kind {
                TileKind::Small => {
                    let x = left + index as f64 * (s + TILE_GAP);
                    tiles.push(TileSlot { app, kind: *kind, rect: Rect { pos: dvec2(x, y), size: dvec2(s, s) } });
                }
                TileKind::Wide => {
                    y += s + TILE_GAP;
                    tiles.push(TileSlot { app, kind: *kind, rect: Rect { pos: dvec2(left, y), size: dvec2(width, s) } });
                }
            }
        }
        tiles_bottom = y + s;
    }
    let columns = if landscape { 7 } else { 4 };
    let fav_top = tiles_bottom + TILE_GAP + 6.0;
    // Leave a separate strip for the page indicator / App Library target.
    let fav_bottom = dock.pos.y - 36.0;
    let cell_min = if landscape { 64.0 } else { 88.0 };
    let (favorites, row_height, capacity) = if fav_bottom - fav_top >= cell_min {
        let rows = ((fav_bottom - fav_top) / cell_min).floor().max(1.0) as usize;
        let row_height = ((fav_bottom - fav_top) / rows as f64).min(104.0);
        (
            Rect { pos: dvec2(screen.pos.x + 12.0, fav_top), size: dvec2(screen.size.x - 24.0, row_height * rows as f64) },
            row_height,
            rows * columns,
        )
    } else {
        (Rect { pos: dvec2(screen.pos.x + 12.0, fav_top), size: dvec2(screen.size.x - 24.0, 0.0) }, 0.0, 0)
    };
    HomeLayout { tiles, favorites, columns, row_height, capacity, landscape }
}

/// iOS App Library: every launchable app in a category card. Categories are
/// fixed by app id (an app the table does not know lands in "Other"), so
/// every launch target is reachable from exactly one card.
pub const LIBRARY_GROUPS: [(&str, &[&str]); 5] = [
    ("Utilities", &["clock", "weather", "terminal", "files", "task"]),
    ("Creativity", &["photos", "mixer", "score", "vj", "fab", "fabric"]),
    ("Productivity", &["sheets", "browser", "route", "studio"]),
    ("Media", &["video", "image", "pdf"]),
    ("Other", &[]),
];

/// Group app ids into the library's categories, keeping every id exactly
/// once and the launcher's order inside each card.
pub fn app_library_groups<'a>(apps: &[&'a str]) -> Vec<(&'static str, Vec<&'a str>)> {
    let mut out: Vec<(&'static str, Vec<&'a str>)> = LIBRARY_GROUPS.iter().map(|(name, _)| (*name, Vec::new())).collect();
    for app in apps {
        let index = LIBRARY_GROUPS
            .iter()
            .position(|(_, members)| members.contains(app))
            .unwrap_or(LIBRARY_GROUPS.len() - 1);
        if !out[index].1.contains(app) {
            out[index].1.push(app);
        }
    }
    out.retain(|(_, members)| !members.is_empty());
    out
}

/// The face the WM wants a tile client to show, mirrored from
/// `HostedViewMode` so this module stays free of the wire type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Face {
    #[default]
    Full,
    Tile,
}

impl Face {
    pub fn mode(self) -> HostedViewMode {
        match self {
            Face::Full => HostedViewMode::Full,
            Face::Tile => HostedViewMode::Tile,
        }
    }
}

/// One client that owns a home tile.
#[derive(Clone, Debug, PartialEq)]
pub struct TileClient {
    pub app: String,
    pub client: ClientId,
    /// Launched for the tile and never opened by the person: it is not in
    /// the layout, so neither the desktop nor Recents shows it. Opening it
    /// promotes this same client.
    pub background: bool,
    /// The face last sent to the client, with the viewport it was sent for.
    pub sent: Option<(Face, Vec2d)>,
    /// A frame at the sent viewport arrived: the capture in that face is
    /// trustworthy (a full-screen card is never a stretched tile, a tile is
    /// never a squeezed window).
    pub confirmed: bool,
}

impl TileClient {
    pub fn face(&self) -> Option<Face> {
        self.sent.map(|(face, _)| face)
    }
    /// The client is showing (or about to show) its compact face.
    pub fn in_tile_face(&self) -> bool {
        self.face() == Some(Face::Tile)
    }
    /// The compact capture may be shown on the home page.
    pub fn tile_ready(&self) -> bool {
        self.in_tile_face() && self.confirmed
    }
    /// The full-screen capture may be refreshed from the client's frames.
    pub fn full_ready(&self) -> bool {
        match self.face() {
            Some(Face::Full) => self.confirmed,
            Some(Face::Tile) => false,
            None => true,
        }
    }
}

/// How long after a tile client dies (or fails to spawn) before the home
/// page tries again, and how many tries it gets per session.
pub const RELAUNCH_COOLDOWN: f64 = 6.0;
pub const LAUNCH_ATTEMPTS: u32 = 3;

/// The home tiles' bookkeeping: which client shows which tile, in which
/// face, and the launch budget per app. Plain state over ids.
#[derive(Clone, Debug, Default)]
pub struct HomeTiles {
    clients: Vec<TileClient>,
    attempts: HashMap<String, (u32, f64)>,
}

impl HomeTiles {
    pub fn client_of(&self, app: &str) -> Option<ClientId> {
        self.clients.iter().find(|t| t.app == app).map(|t| t.client)
    }
    pub fn get(&self, client: ClientId) -> Option<&TileClient> {
        self.clients.iter().find(|t| t.client == client)
    }
    fn get_mut(&mut self, client: ClientId) -> Option<&mut TileClient> {
        self.clients.iter_mut().find(|t| t.client == client)
    }
    pub fn is_tile_client(&self, client: ClientId) -> bool {
        self.get(client).is_some()
    }
    pub fn is_background(&self, client: ClientId) -> bool {
        self.get(client).is_some_and(|t| t.background)
    }
    pub fn in_tile_face(&self, client: ClientId) -> bool {
        self.get(client).is_some_and(TileClient::in_tile_face)
    }
    pub fn face_of(&self, client: ClientId) -> Option<Face> {
        self.get(client).and_then(TileClient::face)
    }
    pub fn clients(&self) -> impl Iterator<Item = &TileClient> {
        self.clients.iter()
    }

    /// A client now shows `app`'s tile. `background` is true for a client
    /// launched by the home page itself, false for a window the person
    /// already had open (its tile is an extra face of that window).
    pub fn bind(&mut self, app: &str, client: ClientId, background: bool) {
        self.clients.retain(|t| t.app != app && t.client != client);
        self.clients.push(TileClient { app: app.to_string(), client, background, sent: None, confirmed: false });
    }

    /// The client is gone (died, closed): its tile is empty again.
    pub fn forget_client(&mut self, client: ClientId) -> Option<String> {
        let app = self.get(client).map(|t| t.app.clone());
        self.clients.retain(|t| t.client != client);
        app
    }

    /// Drop every client the caller no longer knows.
    pub fn retain_clients(&mut self, alive: impl Fn(ClientId) -> bool) {
        self.clients.retain(|t| alive(t.client));
    }

    /// The person opened the client: it is a real window from now on.
    /// True when it WAS a background tile (the caller seats it in the layout).
    pub fn promote(&mut self, client: ClientId) -> bool {
        match self.get_mut(client) {
            Some(t) if t.background => {
                t.background = false;
                true
            }
            _ => false,
        }
    }

    /// Record that `face` was sent for `viewport`. Nothing is confirmed
    /// until a frame of that size arrives.
    pub fn note_sent(&mut self, client: ClientId, face: Face, viewport: Vec2d) {
        if let Some(t) = self.get_mut(client) {
            if t.sent != Some((face, viewport)) {
                t.sent = Some((face, viewport));
                t.confirmed = false;
            }
        }
    }

    /// The face the client still needs to be told about, if the one wanted
    /// (or its viewport) differs from what was sent.
    pub fn pending(&self, client: ClientId, face: Face, viewport: Vec2d) -> bool {
        match self.get(client) {
            Some(t) => match t.sent {
                Some((sent, size)) => sent != face || !same_size(size, viewport),
                None => true,
            },
            None => false,
        }
    }

    /// A frame of `size` (logical points) landed. It confirms the sent
    /// face when it matches that face's viewport. Returns the face it
    /// belongs to, or None for a stale frame from the previous viewport.
    /// Before anything was sent the client is in its default full face.
    pub fn note_frame(&mut self, client: ClientId, size: Vec2d) -> Option<Face> {
        let t = self.get_mut(client)?;
        let Some((face, viewport)) = t.sent else { return Some(Face::Full) };
        if same_size(viewport, size) {
            t.confirmed = true;
            Some(face)
        } else {
            None
        }
    }

    /// The launch budget: at most `LAUNCH_ATTEMPTS` per app, spaced by the
    /// cooldown, so a crashing app never respawns every frame.
    pub fn may_launch(&self, app: &str, now: f64) -> bool {
        match self.attempts.get(app) {
            None => true,
            Some((count, last)) => *count < LAUNCH_ATTEMPTS && now - *last >= RELAUNCH_COOLDOWN,
        }
    }
    pub fn note_launch(&mut self, app: &str, now: f64) {
        let entry = self.attempts.entry(app.to_string()).or_insert((0, now));
        entry.0 += 1;
        entry.1 = now;
    }
    /// The launch budget is spent: the tile says so instead of spinning.
    pub fn gave_up(&self, app: &str) -> bool {
        self.attempts.get(app).is_some_and(|(count, _)| *count >= LAUNCH_ATTEMPTS)
    }
    /// A launch worked out (the client drew): the budget resets, so a much
    /// later death gets a fresh set of tries.
    pub fn note_healthy(&mut self, app: &str) {
        self.attempts.remove(app);
    }
}

/// A frame size matches a viewport within the rounding the host's DPI
/// scaling introduces.
pub fn same_size(a: Vec2d, b: Vec2d) -> bool {
    (a.x - b.x).abs() <= 1.5 && (a.y - b.y).abs() <= 1.5
}

/// Which face a tile client should show right now. Full for the client the
/// person is looking at (the open app, from the first frame of its
/// zoom-in); Tile for everyone else, but an opened window only once the
/// home page has settled, so it never reconfigures while it is still
/// animating down into its icon. A background client was never a window
/// and takes its tile face at once.
pub fn wanted_face(client: ClientId, foreground: Option<ClientId>, home_settled: bool, background: bool) -> Option<Face> {
    if foreground == Some(client) {
        Some(Face::Full)
    } else if home_settled || background {
        Some(Face::Tile)
    } else {
        None
    }
}

/// The text a tile shows while its client has nothing to draw yet, from
/// the launcher's own status line (cargo's progress, the first-exec scan).
pub fn placeholder_text(status: &str, connected: bool, gave_up: bool) -> (&'static str, String) {
    if gave_up {
        return ("Could not start", "tap to try again".to_string());
    }
    let headline = if status.starts_with("compiling ") {
        "Compiling…"
    } else if status.starts_with("waiting for another build") {
        "Waiting to compile…"
    } else if status.starts_with("build failed") {
        "Could not build"
    } else if connected {
        "Loading…"
    } else {
        "Starting…"
    };
    (headline, status.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobile::{app_rect, phone_size};
    use crate::mobile_surface::PhoneSurface;

    fn screen(size: Vec2d) -> Rect {
        Rect { pos: dvec2(0.0, 26.0), size: size - dvec2(0.0, 26.0) }
    }

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.pos.x < b.pos.x + b.size.x && b.pos.x < a.pos.x + a.size.x && a.pos.y < b.pos.y + b.size.y && b.pos.y < a.pos.y + a.size.y
    }

    #[test]
    fn portrait_stacks_two_small_tiles_over_a_wide_one_and_leaves_favorites_room() {
        for style in [crate::desktop::DesktopStyle::Ios, crate::desktop::DesktopStyle::Android] {
            let screen = screen(phone_size(style));
            let dock = PhoneSurface::home_dock(screen);
            let layout = home_layout(screen, screen.pos.y + 70.0, dock);
            assert!(!layout.landscape);
            let [clock, weather, photos] = [layout.tiles[0], layout.tiles[1], layout.tiles[2]];
            assert_eq!((clock.app, weather.app, photos.app), ("clock", "weather", "photos"));
            assert_eq!(clock.rect.size, weather.rect.size);
            assert!((clock.rect.size.x - clock.rect.size.y).abs() < 0.01, "small tiles are square");
            assert_eq!(clock.rect.pos.y, weather.rect.pos.y);
            assert!(photos.rect.pos.y > clock.rect.pos.y + clock.rect.size.y);
            assert!((photos.rect.size.x - (screen.size.x - HOME_MARGIN * 2.0)).abs() < 0.01);
            assert!(layout.capacity >= 8, "at least two rows of favorites: {}", layout.capacity);
            assert_eq!(layout.columns, 4);
            for tile in &layout.tiles {
                assert!(!overlaps(tile.rect, layout.favorites));
                assert!(!overlaps(tile.rect, dock));
            }
            assert!(!overlaps(layout.favorites, dock));
        }
    }

    #[test]
    fn landscape_puts_three_tiles_across_and_keeps_the_dock_clear() {
        let size = phone_size(crate::desktop::DesktopStyle::Ios);
        let screen = screen(dvec2(size.y, size.x));
        let dock = PhoneSurface::home_dock(screen);
        let layout = home_layout(screen, screen.pos.y + 44.0, dock);
        assert!(layout.landscape);
        assert_eq!(layout.columns, 7);
        let y = layout.tiles[0].rect.pos.y;
        assert!(layout.tiles.iter().all(|t| t.rect.pos.y == y), "one row");
        let w = layout.tiles[0].rect.size.x;
        assert!(layout.tiles.iter().all(|t| (t.rect.size.x - w).abs() < 0.01), "equal columns");
        let right = layout.tiles[2].rect.pos.x + layout.tiles[2].rect.size.x;
        assert!((right - (screen.pos.x + screen.size.x - HOME_MARGIN)).abs() < 0.5);
        for tile in &layout.tiles {
            assert!(!overlaps(tile.rect, dock));
            assert!(tile.rect.pos.y + tile.rect.size.y <= layout.favorites.pos.y);
        }
        assert!(layout.favorites.pos.y + layout.favorites.size.y <= dock.pos.y);
        assert!(layout.capacity >= layout.columns, "a row of favorites fits beside the dock");
    }

    #[test]
    fn tiles_are_smaller_than_the_full_viewport_in_both_orientations() {
        let size = phone_size(crate::desktop::DesktopStyle::Android);
        for size in [size, dvec2(size.y, size.x)] {
            let screen = screen(size);
            let app = app_rect(screen);
            let layout = home_layout(screen, screen.pos.y + 70.0, PhoneSurface::home_dock(screen));
            for tile in layout.tiles {
                assert!(tile.rect.size.x < app.size.x && tile.rect.size.y < app.size.y);
                assert!(!same_size(tile.rect.size, app.size), "a tile frame is never mistaken for a window frame");
            }
        }
    }

    #[test]
    fn the_app_library_places_every_app_exactly_once() {
        let apps = ["browser", "files", "terminal", "photos", "clock", "weather", "splash", "sheets"];
        let groups = app_library_groups(&apps);
        let mut seen: Vec<&str> = groups.iter().flat_map(|(_, m)| m.iter().copied()).collect();
        seen.sort_unstable();
        let mut expected = apps.to_vec();
        expected.sort_unstable();
        assert_eq!(seen, expected);
        assert!(groups.iter().any(|(name, m)| *name == "Other" && m == &vec!["splash"]));
        assert!(groups.iter().all(|(_, m)| !m.is_empty()));
        let utilities = groups.iter().find(|(n, _)| *n == "Utilities").unwrap();
        assert_eq!(utilities.1, vec!["files", "terminal", "clock", "weather"], "launcher order inside a card");
    }

    #[test]
    fn a_background_tile_client_is_promoted_once_and_keeps_its_identity() {
        let mut tiles = HomeTiles::default();
        tiles.bind("clock", 7, true);
        assert!(tiles.is_background(7));
        assert_eq!(tiles.client_of("clock"), Some(7));
        assert!(tiles.promote(7), "the first open seats it in the layout");
        assert!(!tiles.promote(7), "a second open does nothing");
        assert!(!tiles.is_background(7));
        assert_eq!(tiles.client_of("clock"), Some(7), "same client, never a second process");
        // Rebinding the app to a new client replaces the old entry.
        tiles.bind("clock", 9, false);
        assert_eq!(tiles.client_of("clock"), Some(9));
        assert!(tiles.get(7).is_none());
        assert_eq!(tiles.forget_client(9).as_deref(), Some("clock"));
        assert_eq!(tiles.client_of("clock"), None);
    }

    #[test]
    fn faces_are_confirmed_only_by_a_frame_of_the_sent_viewport() {
        let mut tiles = HomeTiles::default();
        tiles.bind("photos", 3, true);
        let tile = dvec2(370.0, 177.0);
        let full = dvec2(402.0, 782.0);
        assert!(tiles.pending(3, Face::Tile, tile), "nothing sent yet");
        assert_eq!(tiles.note_frame(3, full), Some(Face::Full), "an untouched client is in its full face");
        assert!(tiles.get(3).unwrap().full_ready());
        tiles.note_sent(3, Face::Tile, tile);
        assert!(!tiles.pending(3, Face::Tile, tile));
        assert!(tiles.in_tile_face(3) && !tiles.get(3).unwrap().tile_ready());
        // A stale full-size frame in flight does not confirm the tile.
        assert_eq!(tiles.note_frame(3, full), None);
        assert!(!tiles.get(3).unwrap().tile_ready());
        assert!(!tiles.get(3).unwrap().full_ready(), "the card must not refresh from tile-face frames");
        assert_eq!(tiles.note_frame(3, dvec2(370.4, 177.0)), Some(Face::Tile));
        assert!(tiles.get(3).unwrap().tile_ready());
        // Opening: Full is sent, and the tile is no longer trusted.
        tiles.note_sent(3, Face::Full, full);
        assert!(!tiles.get(3).unwrap().tile_ready());
        assert!(!tiles.get(3).unwrap().full_ready());
        assert_eq!(tiles.note_frame(3, tile), None, "the last tile frame never lands in the card");
        assert_eq!(tiles.note_frame(3, full), Some(Face::Full));
        assert!(tiles.get(3).unwrap().full_ready());
        // Rotating: the same face at a new viewport is pending again.
        assert!(tiles.pending(3, Face::Full, dvec2(874.0, 328.0)));
    }

    #[test]
    fn tile_faces_wait_for_home_to_settle_but_full_is_immediate() {
        assert_eq!(wanted_face(1, Some(1), false, false), Some(Face::Full));
        assert_eq!(wanted_face(1, Some(1), true, false), Some(Face::Full));
        assert_eq!(wanted_face(2, Some(1), false, false), None, "still animating home");
        assert_eq!(wanted_face(2, Some(1), true, false), Some(Face::Tile));
        assert_eq!(wanted_face(2, None, true, false), Some(Face::Tile));
        assert_eq!(wanted_face(3, Some(1), false, true), Some(Face::Tile), "never a window: nothing to wait for");
        assert_eq!(wanted_face(3, Some(3), false, true), Some(Face::Full), "opening wins over background");
    }

    #[test]
    fn the_launch_budget_spaces_retries_and_gives_up() {
        let mut tiles = HomeTiles::default();
        let now = 100.0;
        assert!(tiles.may_launch("weather", now));
        tiles.note_launch("weather", now);
        assert!(!tiles.may_launch("weather", now + 1.0), "no relaunch every frame");
        assert!(tiles.may_launch("weather", now + RELAUNCH_COOLDOWN));
        tiles.note_launch("weather", now + RELAUNCH_COOLDOWN);
        tiles.note_launch("weather", now + RELAUNCH_COOLDOWN * 2.0);
        assert!(tiles.gave_up("weather"));
        assert!(!tiles.may_launch("weather", now + 1000.0));
        tiles.note_healthy("weather");
        assert!(tiles.may_launch("weather", now + 1000.0));
        assert!(tiles.may_launch("clock", now), "budgets are per app");
    }

    #[test]
    fn placeholders_read_the_launcher_status() {
        assert_eq!(placeholder_text("compiling makepad-clock v0.1.0…", false, false).0, "Compiling…");
        assert_eq!(placeholder_text("", false, false).0, "Starting…");
        assert_eq!(placeholder_text("", true, false).0, "Loading…");
        assert_eq!(placeholder_text("build failed — see the app log", false, false).0, "Could not build");
        assert_eq!(placeholder_text("anything", true, true).0, "Could not start");
    }
}
