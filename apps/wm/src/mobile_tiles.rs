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

/// The tile apps THIS build can start (apps.rs `Launchable`): a build
/// without a clock or a weather app lays out no tile for them, instead
/// of a card that could never start.
pub fn tile_apps(launchable: &crate::apps::Launchable) -> Vec<(&'static str, TileKind)> {
    TILE_APPS
        .iter()
        .copied()
        .filter(|(id, _)| crate::clients::find_app(id).is_some_and(|app| launchable.allows(&app)))
        .collect()
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
    home_layout_for(screen, top, dock, &TILE_APPS)
}

/// [`home_layout`] for the tile apps a build has (see [`tile_apps`]): the
/// small tiles share the first row, a wide tile takes the row under them
/// — the top row when there are no small ones — and the favorites start
/// right under the last tile, or at `top` when there is none.
pub fn home_layout_for(screen: Rect, top: f64, dock: Rect, tile_apps: &[(&'static str, TileKind)]) -> HomeLayout {
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
        for (index, (app, kind)) in tile_apps.iter().enumerate() {
            let x = left + index as f64 * (w + TILE_GAP);
            tiles.push(TileSlot { app, kind: *kind, rect: Rect { pos: dvec2(x, top), size: dvec2(w, h) } });
        }
        tiles_bottom = if tile_apps.is_empty() { top - TILE_GAP - 6.0 } else { top + h };
    } else {
        let s = ((width - TILE_GAP) / 2.0).max(1.0);
        let smalls = tile_apps.iter().filter(|(_, kind)| *kind == TileKind::Small).count();
        let mut y = top;
        let mut small = 0;
        let mut rows = 0;
        for (app, kind) in tile_apps.iter() {
            match kind {
                TileKind::Small => {
                    let x = left + small as f64 * (s + TILE_GAP);
                    small += 1;
                    rows = rows.max(1);
                    tiles.push(TileSlot { app, kind: *kind, rect: Rect { pos: dvec2(x, y), size: dvec2(s, s) } });
                }
                TileKind::Wide => {
                    if smalls > 0 {
                        y += s + TILE_GAP;
                    }
                    rows += 1;
                    tiles.push(TileSlot { app, kind: *kind, rect: Rect { pos: dvec2(left, y), size: dvec2(width, s) } });
                }
            }
        }
        tiles_bottom = if rows == 0 { top - TILE_GAP - 6.0 } else { y + s };
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
    ("Utilities", &["clock", "weather", "calculator", "terminal", "files", "task"]),
    ("Creativity", &["photos", "mixer", "score", "vj", "fab", "fabric"]),
    ("Productivity", &["sheets", "finance", "mail", "notes", "calendar", "reminders", "browser", "route", "studio"]),
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

/// A tile app whose compact face shows the SAME content as its full face
/// (the picture wall: the tile is one picture of the wall): opening it is
/// a location-accurate crossfade, never a zoom that stretches the tile's
/// pixels. Clock and Weather show different content in each face and
/// keep the zoom.
pub fn crossfade_app(app: &str) -> bool {
    app == "photos"
}

/// The dock holds at most this many icons.
pub const DOCK_MAX: usize = 4;
/// The dock's apps on a first run, left to right.
pub const PINNED: [&str; 4] = ["browser", "files", "photos", "terminal"];

/// One place an icon can sit on the home page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// The favorites grid, in reading order.
    Icon(usize),
    /// The dock, left to right.
    Dock(usize),
}

/// The home page's icon order and the dock's members: what the person
/// arranged (edit mode), kept in the WM's storage namespace as the
/// `home.order` document. The default on first run is the shell's old
/// rule: the launcher's order minus the pinned apps, the pinned apps in
/// the dock.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HomeOrder {
    pub icons: Vec<String>,
    pub dock: Vec<String>,
}

impl HomeOrder {
    /// The first-run order: `apps` in launcher order, the `pinned` ones
    /// (that the build has) in the dock, everything else on the page.
    pub fn default_for(apps: &[&str], pinned: &[&str]) -> Self {
        let dock: Vec<String> = pinned.iter().filter(|id| apps.contains(id)).take(DOCK_MAX).map(|id| id.to_string()).collect();
        let icons = apps.iter().filter(|id| !dock.iter().any(|d| d == *id)).map(|id| id.to_string()).collect();
        HomeOrder { icons, dock }
    }
    /// The build's apps changed since the order was saved: an app that is
    /// gone leaves, a new one lands at the end of the page, nothing is
    /// listed twice.
    pub fn reconcile(&mut self, apps: &[&str]) {
        self.dock.retain(|id| apps.contains(&id.as_str()));
        self.dock.truncate(DOCK_MAX);
        self.icons.retain(|id| apps.contains(&id.as_str()) && !self.dock.contains(id));
        let mut seen = Vec::new();
        self.icons.retain(|id| if seen.contains(id) { false } else { seen.push(id.clone()); true });
        for app in apps {
            if !self.icons.iter().any(|id| id == app) && !self.dock.iter().any(|id| id == app) {
                self.icons.push(app.to_string());
            }
        }
    }
    pub fn slot_of(&self, id: &str) -> Option<Slot> {
        if let Some(i) = self.icons.iter().position(|x| x == id) { return Some(Slot::Icon(i)); }
        self.dock.iter().position(|x| x == id).map(Slot::Dock)
    }
    pub fn at(&self, slot: Slot) -> Option<&str> {
        match slot {
            Slot::Icon(i) => self.icons.get(i).map(String::as_str),
            Slot::Dock(i) => self.dock.get(i).map(String::as_str),
        }
    }
    /// Move the icon at `from` to `to`, the others shifting aside: within
    /// the page or the dock the icon takes the target index; page → dock
    /// only while the dock has room (`DOCK_MAX`); dock → page always. True
    /// when something moved.
    pub fn move_to(&mut self, from: Slot, to: Slot) -> bool {
        if from == to { return false; }
        match (from, to) {
            (Slot::Icon(a), Slot::Icon(b)) => {
                if a >= self.icons.len() { return false; }
                let id = self.icons.remove(a);
                let b = b.min(self.icons.len());
                self.icons.insert(b, id);
                true
            }
            (Slot::Dock(a), Slot::Dock(b)) => {
                if a >= self.dock.len() { return false; }
                let id = self.dock.remove(a);
                let b = b.min(self.dock.len());
                self.dock.insert(b, id);
                true
            }
            (Slot::Icon(a), Slot::Dock(b)) => {
                if a >= self.icons.len() || self.dock.len() >= DOCK_MAX { return false; }
                let id = self.icons.remove(a);
                let b = b.min(self.dock.len());
                self.dock.insert(b, id);
                true
            }
            (Slot::Dock(a), Slot::Icon(b)) => {
                if a >= self.dock.len() { return false; }
                let id = self.dock.remove(a);
                let b = b.min(self.icons.len());
                self.icons.insert(b, id);
                true
            }
        }
    }
    /// The `home.order` document: one line per icon, `icon <id>` for the
    /// page, `dock <id>` for the dock, in order.
    pub fn to_document(&self) -> Vec<u8> {
        let mut out = String::new();
        for id in &self.icons { out.push_str("icon "); out.push_str(id); out.push('\n'); }
        for id in &self.dock { out.push_str("dock "); out.push_str(id); out.push('\n'); }
        out.into_bytes()
    }
    /// A saved document; `None` when it is not one (the next save replaces it).
    pub fn from_document(bytes: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(bytes).ok()?;
        let mut order = HomeOrder::default();
        let mut any = false;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() { continue; }
            let (kind, id) = line.split_once(' ')?;
            let id = id.trim();
            if id.is_empty() { return None; }
            match kind {
                "icon" => order.icons.push(id.to_string()),
                "dock" => order.dock.push(id.to_string()),
                _ => return None,
            }
            any = true;
        }
        any.then_some(order)
    }
}

/// The cell of favorite `index` in `layout`'s grid: the drawing and the
/// edit-mode hit-testing share it.
pub fn favorite_cell(layout: &HomeLayout, index: usize) -> Rect {
    let cell = layout.favorites.size.x / layout.columns.max(1) as f64;
    Rect {
        pos: dvec2(
            layout.favorites.pos.x + (index % layout.columns.max(1)) as f64 * cell,
            layout.favorites.pos.y + (index / layout.columns.max(1)) as f64 * layout.row_height,
        ),
        size: dvec2(cell, layout.row_height),
    }
}

/// The cell of dock icon `index` when the dock holds `count`.
pub fn dock_cell(dock: Rect, count: usize, index: usize) -> Rect {
    let cell = dock.size.x / count.max(1) as f64;
    Rect { pos: dvec2(dock.pos.x + index as f64 * cell, dock.pos.y), size: dvec2(cell, dock.size.y) }
}

/// Where a dragged icon would land at `p`: the dock cell under it (a
/// dragged page icon lands BETWEEN two dock icons, so the dock is cut
/// into `count + 1` targets while it has room), else the favorites cell —
/// a point past the last icon lands at the end.
pub fn slot_at(layout: &HomeLayout, dock: Rect, icons: usize, dock_count: usize, dragging_from_page: bool, p: Vec2d) -> Option<Slot> {
    let grown = Rect { pos: dock.pos - dvec2(0.0, 8.0), size: dock.size + dvec2(0.0, 16.0) };
    if grown.contains(p) {
        let targets = if dragging_from_page { dock_count + 1 } else { dock_count.max(1) };
        let cell = dock.size.x / targets as f64;
        let index = (((p.x - dock.pos.x) / cell).floor().max(0.0) as usize).min(targets.saturating_sub(1));
        return Some(Slot::Dock(index));
    }
    if layout.row_height <= 0.0 || layout.columns == 0 { return None; }
    let fav = layout.favorites;
    if p.y < fav.pos.y - 8.0 || p.x < fav.pos.x || p.x > fav.pos.x + fav.size.x { return None; }
    let rows = (icons + layout.columns - 1) / layout.columns;
    if p.y > fav.pos.y + rows.max(1) as f64 * layout.row_height + 8.0 { return None; }
    let cell = fav.size.x / layout.columns as f64;
    let col = (((p.x - fav.pos.x) / cell).floor().max(0.0) as usize).min(layout.columns - 1);
    let row = ((p.y - fav.pos.y) / layout.row_height).floor().max(0.0) as usize;
    Some(Slot::Icon((row * layout.columns + col).min(icons.saturating_sub(1))))
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
    use crate::mobile::{app_rect, phone_size, PhoneChrome};
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
            let dock = PhoneSurface::home_dock(screen, PhoneChrome::Simulated);
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
    fn a_build_without_some_tile_apps_lays_out_only_the_tiles_it_has() {
        let screen = screen(phone_size(crate::desktop::DesktopStyle::Ios));
        let dock = PhoneSurface::home_dock(screen, PhoneChrome::Simulated);
        let top = screen.pos.y + 70.0;
        let all = home_layout(screen, top, dock);
        // Only the wide tile: it takes the top row, favorites move up.
        let photos = home_layout_for(screen, top, dock, &[("photos", TileKind::Wide)]);
        assert_eq!(photos.tiles.len(), 1);
        assert_eq!(photos.tiles[0].rect.pos.y, top);
        assert_eq!(photos.tiles[0].rect.size, all.tiles[2].rect.size);
        assert!(photos.favorites.pos.y < all.favorites.pos.y);
        assert!(photos.capacity > all.capacity);
        // No tiles at all: favorites start at the top.
        let none = home_layout_for(screen, top, dock, &[]);
        assert!(none.tiles.is_empty());
        assert_eq!(none.favorites.pos.y, top);
        // One small tile keeps its square and the wide one goes under it.
        let two = home_layout_for(screen, top, dock, &[("clock", TileKind::Small), ("photos", TileKind::Wide)]);
        assert_eq!(two.tiles[0].rect, all.tiles[0].rect);
        assert_eq!(two.tiles[1].rect, all.tiles[2].rect);
        // The build's tile apps follow what it can launch.
        let linked = crate::apps::Launchable { linked: vec!["photos"], processes: false };
        assert_eq!(tile_apps(&linked), vec![("photos", TileKind::Wide)]);
    }

    #[test]
    fn landscape_puts_three_tiles_across_and_keeps_the_dock_clear() {
        let size = phone_size(crate::desktop::DesktopStyle::Ios);
        let screen = screen(dvec2(size.y, size.x));
        let dock = PhoneSurface::home_dock(screen, PhoneChrome::Simulated);
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
            let app = app_rect(screen, PhoneChrome::Simulated);
            let layout = home_layout(screen, screen.pos.y + 70.0, PhoneSurface::home_dock(screen, PhoneChrome::Simulated));
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

    #[test]
    fn the_home_order_defaults_to_the_old_rule_and_moves_icons_with_the_dock_capped() {
        let apps = ["browser", "files", "terminal", "sheets", "photos", "clock", "weather", "route", "finance"];
        let mut order = HomeOrder::default_for(&apps, &["browser", "files", "photos", "terminal", "extra"]);
        assert_eq!(order.dock, ["browser", "files", "photos", "terminal"]);
        assert_eq!(order.icons, ["sheets", "clock", "weather", "route", "finance"]);
        // Move Sheets to the end of the row: everyone shifts aside.
        assert!(order.move_to(Slot::Icon(0), Slot::Icon(4)));
        assert_eq!(order.icons, ["clock", "weather", "route", "finance", "sheets"]);
        assert!(order.move_to(Slot::Icon(4), Slot::Icon(1)));
        assert_eq!(order.icons, ["clock", "sheets", "weather", "route", "finance"]);
        assert!(!order.move_to(Slot::Icon(1), Slot::Icon(1)), "the same slot is no move");
        // A full dock takes nobody; make room and it does, between two.
        assert!(!order.move_to(Slot::Icon(0), Slot::Dock(1)));
        assert!(order.move_to(Slot::Dock(3), Slot::Icon(0)));
        assert_eq!(order.icons[0], "terminal");
        assert_eq!(order.dock.len(), 3);
        assert!(order.move_to(Slot::Icon(1), Slot::Dock(1)));
        assert_eq!(order.dock, ["browser", "clock", "files", "photos"]);
        assert_eq!(order.slot_of("clock"), Some(Slot::Dock(1)));
        assert_eq!(order.slot_of("terminal"), Some(Slot::Icon(0)));
        assert_eq!(order.at(Slot::Dock(9)), None);
        // Within the dock.
        assert!(order.move_to(Slot::Dock(0), Slot::Dock(3)));
        assert_eq!(order.dock, ["clock", "files", "photos", "browser"]);
        // A build that lost an app and gained one.
        order.reconcile(&["files", "photos", "browser", "sheets", "weather", "route", "finance", "mixer"]);
        assert_eq!(order.dock, ["files", "photos", "browser"]);
        assert_eq!(order.icons.last().map(String::as_str), Some("mixer"));
        assert!(!order.icons.iter().any(|id| id == "clock" || id == "terminal"));
    }

    #[test]
    fn the_home_order_round_trips_through_its_document() {
        let order = HomeOrder { icons: vec!["sheets".into(), "route".into()], dock: vec!["files".into(), "photos".into()] };
        let doc = order.to_document();
        assert_eq!(std::str::from_utf8(&doc).unwrap(), "icon sheets\nicon route\ndock files\ndock photos\n");
        assert_eq!(HomeOrder::from_document(&doc), Some(order.clone()));
        assert_eq!(HomeOrder::from_document(b""), None, "nothing saved yet");
        assert_eq!(HomeOrder::from_document(b"garbage"), None);
        assert_eq!(HomeOrder::from_document(b"icon \n"), None);
        assert_eq!(HomeOrder::from_document(b"\n icon sheets \n"), Some(HomeOrder { icons: vec!["sheets".into()], dock: vec![] }));
    }

    #[test]
    fn dragged_icons_land_in_page_cells_or_between_dock_icons() {
        let screen = screen(phone_size(crate::desktop::DesktopStyle::Ios));
        let dock = PhoneSurface::home_dock(screen, PhoneChrome::Simulated);
        let layout = home_layout(screen, screen.pos.y + 70.0, dock);
        let cell = favorite_cell(&layout, 0);
        assert_eq!(cell.pos, layout.favorites.pos);
        assert_eq!(favorite_cell(&layout, layout.columns).pos.y, layout.favorites.pos.y + layout.row_height);
        assert_eq!(favorite_cell(&layout, 1).pos.x, layout.favorites.pos.x + cell.size.x);
        // Five icons on the page: the middle of the third cell is slot 2, past
        // the fifth is the end, off the grid is nothing.
        let centre = |r: Rect| r.pos + r.size * 0.5;
        assert_eq!(slot_at(&layout, dock, 5, 4, true, centre(favorite_cell(&layout, 2))), Some(Slot::Icon(2)));
        assert_eq!(slot_at(&layout, dock, 5, 4, true, centre(favorite_cell(&layout, 7))), Some(Slot::Icon(4)));
        assert_eq!(slot_at(&layout, dock, 5, 4, true, dvec2(screen.pos.x + 30.0, layout.favorites.pos.y - 200.0)), None);
        // The dock: four cells for its own icons, five targets for a newcomer.
        let d2 = dock_cell(dock, 4, 2);
        assert_eq!(d2.pos.x, dock.pos.x + dock.size.x * 0.5);
        assert_eq!(slot_at(&layout, dock, 5, 4, false, centre(d2)), Some(Slot::Dock(2)));
        assert_eq!(slot_at(&layout, dock, 5, 3, true, dvec2(dock.pos.x + dock.size.x - 1.0, centre(dock).y)), Some(Slot::Dock(3)));
        assert_eq!(slot_at(&layout, dock, 5, 3, true, dvec2(dock.pos.x + 1.0, centre(dock).y)), Some(Slot::Dock(0)));
    }
}
