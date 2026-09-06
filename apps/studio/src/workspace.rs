//! The shared workspace's canvas geometry, independent of widgets and `Cx`.
//!
//! Cards refer to existing document, terminal, or activity ids. Moving between
//! the Dock and the canvas does not create a second document or process. The
//! automatic and free arrangements are kept separately so a temporary switch
//! to Auto never erases the user's hand-placed arrangement.

use makepad_widgets::makepad_micro_serde::*;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

pub const MAX_CARDS: usize = 512;
pub const MIN_ZOOM: f64 = 0.00001;
pub const MAX_ZOOM: f64 = 8.0;
pub fn zoom_label(zoom: f64) -> String {
    if zoom < 0.001 {
        format!("{:.3}%", zoom * 100.0)
    } else if zoom < 0.1 {
        format!("{:.1}%", zoom * 100.0)
    } else {
        format!("{:.0}%", zoom * 100.0)
    }
}
const MAX_COORD: f64 = 10_000_000.0;
const GAP: f64 = 72.0;
const STATE_FILE: &str = "canvas.ron";
const MAX_STATE_BYTES: usize = 1_048_576;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, SerRon, DeRon)]
pub enum Mode {
    Structured,
    #[default]
    Canvas,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Structured => "structured",
            Self::Canvas => "canvas",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, SerRon, DeRon)]
pub enum LayoutMode {
    #[default]
    Auto,
    Free,
}

impl LayoutMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Free => "free",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardKind {
    Terminal,
    Code,
    Agent,
    System,
    Run,
    Test,
    App,
}

impl CardKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Code => "code",
            Self::Agent => "agent",
            Self::System => "system",
            Self::Run => "run",
            Self::Test => "test",
            Self::App => "app",
        }
    }

    fn initial_geometry(self) -> Geometry {
        let (w, h) = match self {
            Self::Terminal => (720.0, 460.0),
            Self::Code => (720.0, 540.0),
            Self::App => (720.0, 500.0),
            Self::System | Self::Agent => (440.0, 280.0),
            Self::Run | Self::Test => (520.0, 280.0),
        };
        Geometry {
            x: 0.0,
            y: 0.0,
            w,
            h,
        }
    }

    // Independent roots have useful lanes; a causal parent takes precedence.
    fn lane(self) -> usize {
        match self {
            Self::System | Self::Agent => 0,
            Self::Terminal => 1,
            Self::Code => 2,
            Self::Run | Self::Test | Self::App => 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub id: u64,
    pub kind: CardKind,
    pub title: String,
    /// A factual current summary, supplied by the item's owner.
    pub detail: String,
    /// An actual ownership or causal relationship, never guessed from timing.
    pub parent: Option<u64>,
}

impl Card {
    pub fn new(id: u64, kind: CardKind, title: impl Into<String>) -> Self {
        Self {
            id,
            kind,
            title: title.into(),
            detail: String::new(),
            parent: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, SerRon, DeRon)]
pub struct Geometry {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Geometry {
    pub fn right(self) -> f64 {
        self.x + self.w
    }
    pub fn bottom(self) -> f64 {
        self.y + self.h
    }

    pub fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.right() && y >= self.y && y <= self.bottom()
    }

    pub fn intersects(self, other: Self) -> bool {
        self.x < other.right()
            && self.right() > other.x
            && self.y < other.bottom()
            && self.bottom() > other.y
    }

    fn valid(self) -> bool {
        [self.x, self.y, self.w, self.h]
            .iter()
            .all(|v| v.is_finite())
            && self.x.abs() <= MAX_COORD
            && self.y.abs() <= MAX_COORD
            && self.right().abs() <= MAX_COORD
            && self.bottom().abs() <= MAX_COORD
            && (80.0..=4096.0).contains(&self.w)
            && (80.0..=4096.0).contains(&self.h)
    }
}

/// Pan is in viewport pixels; card coordinates and sizes are in world points.
/// screen = viewport_origin + pan + world * zoom.
#[derive(Clone, Copy, Debug, PartialEq, SerRon, DeRon)]
pub struct Camera {
    pub pan_x: f64,
    pub pan_y: f64,
    pub zoom: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            pan_x: 32.0,
            pan_y: 32.0,
            zoom: 0.7,
        }
    }
}

impl Camera {
    pub fn is_valid(self) -> bool {
        self.pan_x.is_finite()
            && self.pan_y.is_finite()
            && self.zoom.is_finite()
            && self.pan_x.abs() <= MAX_COORD
            && self.pan_y.abs() <= MAX_COORD
            && (MIN_ZOOM..=MAX_ZOOM).contains(&self.zoom)
    }
}

#[derive(Clone, Debug, PartialEq, SerRon, DeRon)]
struct Placement {
    id: u64,
    automatic: Geometry,
    free: Geometry,
}

#[derive(SerRon, DeRon)]
struct StoredWorkspace {
    version: u32,
    mode: Mode,
    layout: LayoutMode,
    camera: Camera,
    placements: Vec<Placement>,
}

#[derive(Clone, Debug, Default)]
pub struct Workspace {
    pub mode: Mode,
    pub layout: LayoutMode,
    pub camera: Camera,
    /// Metadata is refreshed by the live item registry, not restored from disk.
    pub cards: Vec<Card>,
    placements: BTreeMap<u64, Placement>,
}

impl Workspace {
    /// Refresh the live objects without moving existing cards. Unknown or
    /// cyclic parents and duplicate ids reject the whole update atomically.
    /// Removing cards frees their space; it does not compact other cards.
    pub fn reconcile_cards(&mut self, cards: Vec<Card>) -> Result<bool, String> {
        validate_cards(&cards)?;
        if self.cards == cards
            && cards
                .iter()
                .all(|card| self.placements.contains_key(&card.id))
        {
            return Ok(false);
        }
        let ids: HashSet<u64> = cards.iter().map(|card| card.id).collect();
        let by_id: HashMap<u64, &Card> = cards.iter().map(|card| (card.id, card)).collect();
        let mut placements = self.placements.clone();
        placements.retain(|id, _| ids.contains(id));
        let mut pending: Vec<&Card> = cards
            .iter()
            .filter(|card| !placements.contains_key(&card.id))
            .collect();
        // Parents are placed first, independent of the incoming registry order.
        pending.sort_by_key(|card| (card_depth(card, &by_id), card.kind.lane(), card.id));
        for card in pending {
            let parent_kind = card
                .parent
                .and_then(|id| by_id.get(&id))
                .map(|parent| parent.kind);
            let automatic = place_card(card, parent_kind, &placements, LayoutMode::Auto)?;
            let free = place_card(card, parent_kind, &placements, LayoutMode::Free)?;
            placements.insert(
                card.id,
                Placement {
                    id: card.id,
                    automatic,
                    free,
                },
            );
        }
        self.cards = cards;
        self.placements = placements;
        Ok(true)
    }

    pub fn geometry(&self, id: u64) -> Option<Geometry> {
        self.placements.get(&id).map(|p| match self.layout {
            LayoutMode::Auto => p.automatic,
            LayoutMode::Free => p.free,
        })
    }

    /// Changing presentation only selects the corresponding saved arrangement.
    pub fn set_layout(&mut self, layout: LayoutMode) -> bool {
        let changed = self.layout != layout;
        self.layout = layout;
        changed
    }

    /// Dragging is deliberate: Auto never silently becomes Free or gets pinned.
    pub fn move_card(&mut self, id: u64, x: f64, y: f64) -> Result<(), String> {
        if self.layout != LayoutMode::Free {
            return Err("Choose Free layout before moving cards".into());
        }
        let placement = self
            .placements
            .get_mut(&id)
            .ok_or("Unknown workspace card")?;
        let geometry = Geometry {
            x,
            y,
            ..placement.free
        };
        if !geometry.valid() {
            return Err("Card position is outside the finite canvas bounds".into());
        }
        placement.free = geometry;
        Ok(())
    }

    pub fn resize_card(&mut self, id: u64, w: f64, h: f64) -> Result<(), String> {
        let previous = self.geometry(id).ok_or("Unknown workspace card")?;
        let geometry = Geometry { w, h, ..previous };
        if !geometry.valid() {
            return Err("Card size must be finite and between 80 and 4096 points".into());
        }
        if geometry == previous {
            return Ok(());
        }
        if self.layout == LayoutMode::Free {
            self.placements.get_mut(&id).unwrap().free = geometry;
            return Ok(());
        }
        // Most resize frames fit in the available space. Avoid rebuilding the
        // arrangement unless the new rectangle actually reaches a neighbour.
        if !self.placements.iter().any(|(other, placement)| {
            *other != id && geometry.intersects(padded(placement.automatic))
        }) {
            self.placements.get_mut(&id).unwrap().automatic = geometry;
            return Ok(());
        }
        // Fix the resized card in place. Resolve neighbours in their existing
        // vertical order, changing only those that collide with a settled card.
        // Settled rectangles are bottom-sorted: advancing below one means it
        // cannot collide again, so each pair is checked at most once (512²).
        let mut pending: Vec<_> = self
            .placements
            .iter()
            .filter(|(other, _)| **other != id)
            .map(|(other, placement)| (*other, placement.automatic))
            .collect();
        pending.sort_by(|(a_id, a), (b_id, b)| {
            a.y.total_cmp(&b.y)
                .then_with(|| a.x.total_cmp(&b.x))
                .then(a_id.cmp(b_id))
        });
        let mut settled = vec![geometry];
        let mut changed = vec![(id, geometry)];
        for (other, original) in pending {
            let mut rect = original;
            for occupied in &settled {
                if rect.intersects(padded(*occupied)) {
                    rect.y = occupied.bottom() + GAP;
                }
            }
            if !rect.valid() {
                return Err("Resizing would move another card outside the canvas bounds".into());
            }
            if rect != original {
                changed.push((other, rect));
            }
            let index = settled.partition_point(|occupied| occupied.bottom() <= rect.bottom());
            settled.insert(index, rect);
        }
        for (id, geometry) in changed {
            self.placements.get_mut(&id).unwrap().automatic = geometry;
        }
        Ok(())
    }

    pub fn set_camera(&mut self, camera: Camera) -> Result<(), String> {
        if !camera.is_valid() {
            return Err("Invalid canvas camera".into());
        }
        self.camera = camera;
        Ok(())
    }

    /// The visible arrangement's complete extent, useful for fit and minimap.
    pub fn bounds(&self) -> Option<Geometry> {
        let mut extent: Option<Geometry> = None;
        for card in &self.cards {
            let Some(rect) = self.geometry(card.id) else {
                continue;
            };
            extent = Some(match extent {
                None => rect,
                Some(old) => {
                    let x = old.x.min(rect.x);
                    let y = old.y.min(rect.y);
                    Geometry {
                        x,
                        y,
                        w: old.right().max(rect.right()) - x,
                        h: old.bottom().max(rect.bottom()) - y,
                    }
                }
            });
        }
        extent
    }

    pub fn edges(&self) -> Vec<(u64, u64)> {
        self.cards
            .iter()
            .filter_map(|card| card.parent.map(|parent| (parent, card.id)))
            .collect()
    }

    pub fn encode(&self) -> String {
        StoredWorkspace {
            version: 1,
            mode: self.mode,
            layout: self.layout,
            camera: self.camera,
            placements: self.placements.values().cloned().collect(),
        }
        .serialize_ron()
    }

    pub fn decode(text: &str) -> Result<Self, String> {
        if text.len() > MAX_STATE_BYTES {
            return Err("Canvas state is too large".into());
        }
        // Unlike the convenience deserialize_ron method, reject trailing data.
        let mut state = DeRonState::default();
        let mut chars = text.chars();
        state.next(&mut chars);
        state.next_tok(&mut chars).map_err(|e| format!("{e:?}"))?;
        let stored =
            StoredWorkspace::de_ron(&mut state, &mut chars).map_err(|e| format!("{e:?}"))?;
        if state.tok != DeRonTok::Eof {
            return Err("Unexpected data after canvas state".into());
        }
        if stored.version != 1 {
            return Err("Unsupported canvas state version".into());
        }
        if stored.placements.len() > MAX_CARDS {
            return Err("Too many canvas placements".into());
        }
        if !stored.camera.is_valid() {
            return Err("Invalid stored canvas camera".into());
        }
        let mut placements = BTreeMap::new();
        for placement in stored.placements {
            if !placement.automatic.valid() || !placement.free.valid() {
                return Err("Invalid stored card geometry".into());
            }
            if placements.insert(placement.id, placement).is_some() {
                return Err("Duplicate stored card id".into());
            }
        }
        Ok(Self {
            mode: stored.mode,
            layout: stored.layout,
            camera: stored.camera,
            cards: Vec::new(),
            placements,
        })
    }

    /// Corrupt or missing state starts a usable new canvas; no processes or
    /// stale activity cards are restored by the geometry file.
    pub fn load(dir: &Path) -> Self {
        use std::io::Read;
        let text = (|| -> std::io::Result<String> {
            let file = std::fs::File::open(dir.join(STATE_FILE))?;
            let mut text = String::new();
            file.take((MAX_STATE_BYTES + 1) as u64)
                .read_to_string(&mut text)?;
            Ok(text)
        })();
        text.ok()
            .and_then(|text| Self::decode(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        use std::io::{Error, ErrorKind, Write};
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let text = self.encode();
        Self::decode(&text).map_err(|e| Error::new(ErrorKind::InvalidInput, e))?;
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!(
            ".{STATE_FILE}.{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        let result = (|| {
            file.write_all(text.as_bytes())?;
            drop(file);
            std::fs::rename(&path, dir.join(STATE_FILE))
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(path);
        }
        result
    }
}

fn validate_cards(cards: &[Card]) -> Result<(), String> {
    if cards.len() > MAX_CARDS {
        return Err(format!("The canvas supports at most {MAX_CARDS} cards"));
    }
    let mut by_id = HashMap::new();
    for card in cards {
        if card.title.len() > 4096 || card.detail.len() > 65536 {
            return Err("Canvas card metadata is too large".into());
        }
        if by_id.insert(card.id, card).is_some() {
            return Err("Duplicate workspace card id".into());
        }
    }
    for card in cards {
        let mut visited = HashSet::new();
        visited.insert(card.id);
        let mut parent = card.parent;
        while let Some(id) = parent {
            if !visited.insert(id) {
                return Err("Cyclic workspace card relationship".into());
            }
            parent = by_id
                .get(&id)
                .ok_or("Unknown workspace parent card")?
                .parent;
        }
    }
    Ok(())
}

fn card_depth(card: &Card, by_id: &HashMap<u64, &Card>) -> usize {
    let mut depth = 0;
    let mut parent = card.parent;
    while let Some(id) = parent {
        depth += 1;
        parent = by_id[&id].parent;
    }
    depth
}

fn padded(rect: Geometry) -> Geometry {
    Geometry {
        x: rect.x - GAP,
        y: rect.y - GAP,
        w: rect.w + GAP * 2.0,
        h: rect.h + GAP * 2.0,
    }
}

fn place_card(
    card: &Card,
    parent_kind: Option<CardKind>,
    placements: &BTreeMap<u64, Placement>,
    layout: LayoutMode,
) -> Result<Geometry, String> {
    let geometry = |p: &Placement| match layout {
        LayoutMode::Auto => p.automatic,
        LayoutMode::Free => p.free,
    };
    let mut rect = card.kind.initial_geometry();
    if let Some(parent) = card.parent.and_then(|id| placements.get(&id)).map(geometry) {
        rect.x = parent.right() + GAP;
        rect.y = parent.y;
        // A project's terminal, source and run cards are siblings, not a
        // fabricated causal chain. Give their kinds separate columns while
        // leaving their actual parent links intact. More specific ownership
        // (for example a process belonging to a terminal) stays beside that
        // parent. This placement only runs for newly encountered cards.
        if parent_kind == Some(CardKind::System) {
            rect.x += card.kind.lane() as f64 * 800.0;
        } else if matches!(parent_kind, Some(CardKind::Agent | CardKind::Terminal)) {
            // Delegates form a vertical stack directly beside their owner.
            // Owned source/design and run/app activity use adjacent columns,
            // so opening a file never breaks up the delegate stack.
            let lane = match card.kind {
                CardKind::Agent | CardKind::Terminal => 0,
                CardKind::Code | CardKind::System => 1,
                _ => 2,
            };
            rect.x += lane as f64 * 800.0;
        }
    } else {
        rect.x = card.kind.lane() as f64 * 800.0;
    }
    // Only new cards move. Advancing below overlapping rectangles guarantees
    // progress even with manually resized or negative-coordinate neighbours.
    for _ in 0..=placements.len() {
        if !rect.valid() {
            return Err("No room for a card within the canvas bounds".into());
        }
        let mut next_y = rect.y;
        for occupied in placements.values().map(geometry) {
            if rect.intersects(padded(occupied)) {
                next_y = next_y.max(occupied.bottom() + GAP);
            }
        }
        if next_y == rect.y {
            return Ok(rect);
        }
        rect.y = next_y;
    }
    Err("Unable to place canvas card".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_cards() -> Vec<Card> {
        let root = Card::new(1, CardKind::System, "System");
        let mut terminal = Card::new(2, CardKind::Terminal, "Agent terminal");
        terminal.parent = Some(1);
        let mut code = Card::new(3, CardKind::Code, "src/main.rs");
        code.parent = Some(2);
        vec![root, terminal, code]
    }

    #[test]
    fn delegates_stack_beside_parent_with_artifacts_in_adjacent_columns() {
        let mut workspace = Workspace::default();
        let owner = Card::new(1, CardKind::Agent, "Owner");
        let children = [
            (2, CardKind::Agent),
            (3, CardKind::Agent),
            (4, CardKind::Code),
            (5, CardKind::App),
            (6, CardKind::System),
        ];
        let mut cards = vec![owner];
        cards.extend(children.into_iter().map(|(id, kind)| {
            let mut card = Card::new(id, kind, "Owned activity");
            card.parent = Some(1);
            card
        }));
        workspace.reconcile_cards(cards.clone()).unwrap();
        let parent = workspace.geometry(1).unwrap();
        let first = workspace.geometry(2).unwrap();
        let second = workspace.geometry(3).unwrap();
        assert_eq!(first.x, parent.right() + GAP);
        assert_eq!(first.x, second.x);
        assert!(second.y >= first.bottom() + GAP);
        assert!(workspace.geometry(4).unwrap().x > first.x);
        assert!(workspace.geometry(5).unwrap().x > workspace.geometry(4).unwrap().x);
        assert_eq!(
            workspace.geometry(6).unwrap().x,
            workspace.geometry(4).unwrap().x
        );
        cards[1].detail = "Now editing a file".into();
        workspace.reconcile_cards(cards).unwrap();
        assert_eq!(workspace.geometry(2).unwrap(), first);
        assert_eq!(workspace.geometry(3).unwrap(), second);
    }

    #[test]
    fn reconcile_keeps_existing_positions_and_avoids_overlap() {
        let mut workspace = Workspace::default();
        let mut cards = sample_cards();
        workspace.reconcile_cards(cards.clone()).unwrap();
        let initial: Vec<_> = cards
            .iter()
            .map(|c| workspace.geometry(c.id).unwrap())
            .collect();
        assert!(initial[1].x > initial[0].right());
        assert_eq!(initial[2].x, initial[1].right() + GAP + 800.0);
        cards[1].title = "Agent is testing".into();
        cards[2].detail = "Revision 2".into();
        cards.reverse();
        workspace.reconcile_cards(cards.clone()).unwrap();
        for (index, id) in [1, 2, 3].into_iter().enumerate() {
            assert_eq!(workspace.geometry(id), Some(initial[index]));
        }
        let mut test = Card::new(4, CardKind::Test, "cargo test");
        test.parent = Some(2);
        cards.push(test);
        workspace.reconcile_cards(cards.clone()).unwrap();
        let test_position = workspace.geometry(4).unwrap();
        for rect in &initial {
            assert!(!test_position.intersects(*rect));
        }
        cards.retain(|card| card.id != 3);
        workspace.reconcile_cards(cards.clone()).unwrap();
        assert_eq!(workspace.geometry(4), Some(test_position));
        assert_eq!(workspace.geometry(2), Some(initial[1]));
        assert!(workspace.geometry(3).is_none());
        assert!(!workspace.reconcile_cards(cards).unwrap());
    }

    #[test]
    fn system_children_have_distinct_kind_columns_without_changing_parentage() {
        let mut cards = vec![Card::new(1, CardKind::System, "Project")];
        for (id, kind) in [
            (2, CardKind::System),
            (3, CardKind::Terminal),
            (4, CardKind::Code),
            (5, CardKind::Run),
        ] {
            let mut card = Card::new(id, kind, kind.as_str());
            card.parent = Some(1);
            cards.push(card);
        }
        let mut workspace = Workspace::default();
        workspace.reconcile_cards(cards.clone()).unwrap();
        let positions: Vec<_> = (2..=5).map(|id| workspace.geometry(id).unwrap()).collect();
        for pair in positions.windows(2) {
            assert!(pair[1].x > pair[0].right());
            assert_eq!(pair[0].y, pair[1].y);
        }
        assert_eq!(workspace.edges(), vec![(1, 2), (1, 3), (1, 4), (1, 5)]);
        let mut code = Card::new(6, CardKind::Code, "Another file");
        code.parent = Some(1);
        cards.push(code);
        workspace.reconcile_cards(cards).unwrap();
        assert_eq!(workspace.geometry(6).unwrap().x, positions[2].x);
        assert!(workspace.geometry(6).unwrap().y > positions[2].bottom());
        for (id, position) in (2..=5).zip(positions) {
            assert_eq!(workspace.geometry(id), Some(position));
        }
    }

    #[test]
    fn free_arrangement_survives_auto_and_new_items() {
        let mut workspace = Workspace::default();
        let mut cards = sample_cards();
        workspace.reconcile_cards(cards.clone()).unwrap();
        let automatic = workspace.geometry(2).unwrap();
        assert!(workspace.move_card(2, 200.0, 200.0).is_err());
        workspace.set_layout(LayoutMode::Free);
        workspace.move_card(2, -1200.0, 1800.0).unwrap();
        workspace.resize_card(2, 900.0, 600.0).unwrap();
        let manual = workspace.geometry(2).unwrap();
        workspace.set_layout(LayoutMode::Auto);
        assert_eq!(workspace.geometry(2), Some(automatic));
        let mut child = Card::new(4, CardKind::Test, "New test");
        child.parent = Some(2);
        cards.push(child);
        workspace.reconcile_cards(cards).unwrap();
        workspace.set_layout(LayoutMode::Free);
        assert_eq!(workspace.geometry(2), Some(manual));
        assert_eq!(
            workspace.geometry(4).unwrap().x,
            manual.right() + GAP + 1600.0
        );
        assert!(!workspace.geometry(4).unwrap().intersects(manual));
        workspace.set_layout(LayoutMode::Auto);
        assert_eq!(workspace.geometry(2), Some(automatic));
    }

    #[test]
    fn auto_resize_displaces_only_collision_chain_and_keeps_free_arrangement() {
        let mut workspace = Workspace::default();
        let cards = vec![
            Card::new(1, CardKind::Terminal, "First"),
            Card::new(2, CardKind::Terminal, "Second"),
            Card::new(3, CardKind::Terminal, "Third"),
            Card::new(4, CardKind::App, "Other lane"),
        ];
        workspace.reconcile_cards(cards.clone()).unwrap();
        let initial = workspace.placements.clone();
        let anchor = workspace.geometry(1).unwrap();
        workspace.resize_card(1, anchor.w, 900.0).unwrap();
        assert_eq!(workspace.layout, LayoutMode::Auto);
        assert_eq!(
            workspace.geometry(1).unwrap(),
            Geometry { h: 900.0, ..anchor }
        );
        assert_eq!(workspace.geometry(2).unwrap().y, anchor.y + 900.0 + GAP);
        assert_eq!(
            workspace.geometry(3).unwrap().y,
            workspace.geometry(2).unwrap().bottom() + GAP
        );
        assert_eq!(workspace.geometry(4), Some(initial[&4].automatic));
        for (id, placement) in &workspace.placements {
            assert_eq!(placement.free, initial[id].free);
            assert_eq!(placement.automatic.x, initial[id].automatic.x);
            assert!(placement.automatic.valid());
            for (other, neighbour) in &workspace.placements {
                if id != other {
                    assert!(!placement.automatic.intersects(padded(neighbour.automatic)));
                }
            }
        }
        let displaced = workspace.geometry(2).unwrap();
        workspace.resize_card(1, anchor.w, anchor.h).unwrap();
        assert_eq!(
            workspace.geometry(2),
            Some(displaced),
            "shrinking must not shuffle neighbours"
        );
        let encoded = workspace.encode();
        let mut restored = Workspace::decode(&encoded).unwrap();
        restored.reconcile_cards(cards).unwrap();
        assert_eq!(restored.encode(), encoded);
        restored.set_layout(LayoutMode::Free);
        for (id, placement) in initial {
            assert_eq!(restored.geometry(id), Some(placement.free));
        }
    }

    #[test]
    fn auto_width_resize_clears_adjacent_lane_without_global_relayout() {
        let mut workspace = Workspace::default();
        workspace
            .reconcile_cards(vec![
                Card::new(1, CardKind::Terminal, "Terminal"),
                Card::new(2, CardKind::Code, "Source"),
                Card::new(3, CardKind::App, "App"),
            ])
            .unwrap();
        let source = workspace.geometry(2).unwrap();
        let app = workspace.geometry(3).unwrap();
        workspace.resize_card(1, 1200.0, 460.0).unwrap();
        assert_eq!(workspace.geometry(2).unwrap().x, source.x);
        assert_eq!(workspace.geometry(2).unwrap().y, 460.0 + GAP);
        assert!(!workspace
            .geometry(1)
            .unwrap()
            .intersects(padded(workspace.geometry(2).unwrap())));
        assert_eq!(workspace.geometry(3), Some(app));
    }

    #[test]
    fn auto_resize_rolls_back_when_displaced_neighbour_exceeds_bounds() {
        let mut workspace = Workspace::default();
        workspace
            .reconcile_cards(vec![
                Card::new(1, CardKind::Terminal, "First"),
                Card::new(2, CardKind::Terminal, "Second"),
            ])
            .unwrap();
        for placement in workspace.placements.values_mut() {
            placement.automatic.y += MAX_COORD - 1600.0;
        }
        let before = workspace.encode();
        assert!(workspace.resize_card(1, 720.0, 1200.0).is_err());
        assert_eq!(workspace.encode(), before);
        assert!(workspace.resize_card(1, f64::NAN, 460.0).is_err());
        assert!(workspace.resize_card(1, 720.0, 4097.0).is_err());
        assert_eq!(workspace.encode(), before);
    }

    #[test]
    fn bad_registry_updates_and_geometry_are_atomic() {
        let mut workspace = Workspace::default();
        workspace.reconcile_cards(sample_cards()).unwrap();
        let previous = workspace.encode();
        let mut invalid = sample_cards();
        invalid.push(invalid[0].clone());
        assert!(workspace.reconcile_cards(invalid).is_err());
        let mut invalid = sample_cards();
        invalid[0].parent = Some(3);
        assert!(workspace.reconcile_cards(invalid).is_err());
        let mut invalid = sample_cards();
        invalid[2].parent = Some(99);
        assert!(workspace.reconcile_cards(invalid).is_err());
        assert_eq!(workspace.encode(), previous);
        workspace.set_layout(LayoutMode::Free);
        let before = workspace.geometry(2);
        assert!(workspace.move_card(2, f64::NAN, 1.0).is_err());
        assert!(workspace.move_card(2, MAX_COORD, 1.0).is_err());
        assert!(workspace.resize_card(2, -100.0, 1.0).is_err());
        assert_eq!(workspace.geometry(2), before);
        let camera = workspace.camera;
        assert!(workspace
            .set_camera(Camera {
                zoom: f64::INFINITY,
                ..camera
            })
            .is_err());
        assert_eq!(workspace.camera, camera);
    }

    #[test]
    fn geometry_roundtrip_does_not_restore_stale_live_cards() {
        let mut workspace = Workspace::default();
        workspace.reconcile_cards(sample_cards()).unwrap();
        workspace.mode = Mode::Structured;
        workspace.set_layout(LayoutMode::Free);
        workspace.move_card(3, -450.0, 625.0).unwrap();
        workspace
            .set_camera(Camera {
                pan_x: 710.0,
                pan_y: -230.0,
                zoom: 0.025,
            })
            .unwrap();
        let encoded = workspace.encode();
        let mut decoded = Workspace::decode(&encoded).unwrap();
        assert!(decoded.cards.is_empty());
        assert!(decoded.bounds().is_none());
        decoded.reconcile_cards(sample_cards()).unwrap();
        assert_eq!(decoded.encode(), encoded);
        assert_eq!(decoded.geometry(3), workspace.geometry(3));
        assert_eq!(decoded.edges(), vec![(1, 2), (2, 3)]);
        assert_eq!(decoded.bounds(), workspace.bounds());
        assert!(Workspace::decode(&(encoded + " garbage")).is_err());
    }

    #[test]
    fn persisted_duplicates_and_nonfinite_geometry_are_rejected() {
        let rect = CardKind::Terminal.initial_geometry();
        let placement = Placement {
            id: 1,
            automatic: rect,
            free: rect,
        };
        let mut stored = StoredWorkspace {
            version: 1,
            mode: Mode::Canvas,
            layout: LayoutMode::Auto,
            camera: Camera::default(),
            placements: vec![placement.clone(), placement],
        };
        assert!(Workspace::decode(&stored.serialize_ron()).is_err());
        stored.placements.pop();
        stored.placements[0].automatic.x = f64::INFINITY;
        assert!(Workspace::decode(&stored.serialize_ron()).is_err());
        stored.placements[0].automatic.x = 0.0;
        stored.camera.zoom = 0.0;
        assert!(Workspace::decode(&stored.serialize_ron()).is_err());
        assert!(Workspace::decode(&" ".repeat(MAX_STATE_BYTES + 1)).is_err());
    }
}
