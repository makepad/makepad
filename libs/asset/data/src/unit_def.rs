//! What a tiled-strategy piece IS: the `unit` / `sound` / `weapon` manifest
//! lines carried on a sprite asset.
//!
//! [`actor_def`](crate::actor_def) answers the same question for a shooter's
//! cast — a monster that hunts one player. A strategy map asks a different
//! set of questions about the same artwork: what does it cost, which house
//! may build it, what must exist first, how much armour does it wear, and
//! what does its weapon do to each armour class. Those live here, on the
//! asset, for the same reason: they are facts about the SOURCE CONTENT, and
//! the engine that reads them knows nothing about which game filled them in.
//!
//! ```text
//! unit class=vehicle title="Medium Tank" cost=800 hp=400 armor=heavy speed=6.5 sight=30
//!      turn=3.0 sides=house_a,house_b prereq=factory producer=factory power=0
//!      footprint=1x1 turret=billboards/pack/tank-turret weapon=tank_cannon
//! sound select=sfx/pack/ready move=sfx/pack/ackno attack=sfx/pack/fire death=sfx/pack/boom
//! weapon id=tank_cannon damage=30 rate=1.2 range=27 delivery=projectile projectile_speed=60
//!        projectile_sprite=billboards/pack/shell impact=billboards/pack/puff
//!        versus=none:1.0,wood:0.75,light:0.6,heavy:0.25,concrete:0.1
//! ```
//!
//! Everything is optional. A sheet with no `unit` line is scenery, which is
//! a real answer and not a missing entry.

use crate::actor_def::{parse_kv, WeaponDelivery};
use crate::dimensions::ScalePreset;

/// The coarse behaviour family of a placed piece. Determines which systems
/// touch it: movers get a nav path, structures get a footprint, resources
/// get a richness stage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UnitClass {
    /// Ground mover, wheeled/tracked.
    #[default]
    Vehicle,
    /// Ground mover, on foot (smaller, crushable).
    Infantry,
    /// Flying mover; ignores the ground grid.
    Aircraft,
    /// Water mover.
    Boat,
    /// Immobile building with a footprint.
    Structure,
    /// Immobile building that shoots.
    Defense,
    /// A harvestable patch.
    Resource,
    /// Decoration; blocks its cell and does nothing else.
    Scenery,
}

impl UnitClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vehicle => "vehicle",
            Self::Infantry => "infantry",
            Self::Aircraft => "aircraft",
            Self::Boat => "boat",
            Self::Structure => "structure",
            Self::Defense => "defense",
            Self::Resource => "resource",
            Self::Scenery => "scenery",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "vehicle" => Self::Vehicle,
            "infantry" => Self::Infantry,
            "aircraft" => Self::Aircraft,
            "boat" => Self::Boat,
            "structure" => Self::Structure,
            "defense" => Self::Defense,
            "resource" => Self::Resource,
            "scenery" => Self::Scenery,
            _ => return None,
        })
    }

    /// Does this class move under its own orders?
    pub fn is_mover(self) -> bool {
        matches!(self, Self::Vehicle | Self::Infantry | Self::Aircraft | Self::Boat)
    }

    /// Does this class occupy cells permanently?
    pub fn is_static(self) -> bool {
        matches!(self, Self::Structure | Self::Defense | Self::Scenery | Self::Resource)
    }
}

/// What a body is made of, as a NAME. A weapon's `versus` table names one
/// multiplier per name, which is the whole of the strategy-game damage
/// model — and because the vocabulary is open, `armor=ceramic` plus
/// `versus=ceramic:0.4` is a new armour class with no Rust change.
///
/// The five classic names are the BUILT-IN defaults, so `heavy` keeps
/// meaning slot 3 in every world whatever else a level adds.
pub const BUILTIN_ARMOR: [&str; 5] = ["none", "wood", "light", "heavy", "concrete"];

/// The most armour names one world may hold. A `versus` row is an array of
/// this width on every unit, so it is a real budget rather than a taste.
pub const MAX_ARMOR_CLASSES: usize = 16;

/// One world's armour vocabulary: name → slot. Built from what the world's
/// definitions actually name; the five built-ins are always slots 0..5, so
/// content written before open armour keeps its indices exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArmorTable {
    names: Vec<String>,
}

impl Default for ArmorTable {
    fn default() -> Self {
        Self {
            names: BUILTIN_ARMOR.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl ArmorTable {
    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// The slot a name already holds. An unknown (or empty) name is slot 0 —
    /// "unarmoured" — never a panic and never a silent shift of everything
    /// after it.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        if name.is_empty() {
            return Some(0);
        }
        self.names.iter().position(|n| n == name)
    }

    /// The slot for `name`, adding it when the world has room. Returns 0 for
    /// an empty name and for one that arrives after the table is full: a
    /// pack with seventeen armour classes still plays, its overflow simply
    /// reads as unarmoured.
    pub fn intern(&mut self, name: &str) -> usize {
        if name.is_empty() {
            return 0;
        }
        if let Some(i) = self.names.iter().position(|n| n == name) {
            return i;
        }
        if self.names.len() >= MAX_ARMOR_CLASSES {
            return 0;
        }
        self.names.push(name.to_string());
        self.names.len() - 1
    }

    /// Like [`ArmorTable::intern`] but says NO instead of falling back to
    /// slot 0 when the world is full. A `versus` row must never write its
    /// overflow onto the unarmoured multiplier — that would quietly change
    /// what every weapon does to people.
    pub fn try_intern(&mut self, name: &str) -> Option<usize> {
        if name.is_empty() {
            return Some(0);
        }
        if let Some(i) = self.names.iter().position(|n| n == name) {
            return Some(i);
        }
        if self.names.len() >= MAX_ARMOR_CLASSES {
            return None;
        }
        self.names.push(name.to_string());
        Some(self.names.len() - 1)
    }

    pub fn name_of(&self, index: usize) -> &str {
        self.names.get(index).map_or("none", |s| s.as_str())
    }
}

/// Damage multiplier per armour NAME, in the order the manifest declared
/// them. Absent entries are `1.0`, so a weapon that never declares a table
/// hits everything for full damage.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VersusTable(pub Vec<(String, f32)>);

impl VersusTable {
    pub fn get(&self, armor: &str) -> f32 {
        self.0
            .iter()
            .find(|(name, _)| name == armor)
            .map_or(1.0, |(_, v)| *v)
    }

    pub fn set(&mut self, armor: &str, v: f32) {
        if armor.is_empty() {
            return;
        }
        match self.0.iter_mut().find(|(name, _)| name == armor) {
            Some(slot) => slot.1 = v,
            None => self.0.push((armor.to_string(), v)),
        }
    }

    /// Every armour name this table mentions, in declaration order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(name, _)| name.as_str())
    }

    pub fn is_default(&self) -> bool {
        self.0.iter().all(|(_, v)| (*v - 1.0).abs() < 1e-6)
    }

    /// `none:1.0,wood:0.75,light:0.6,heavy:0.25,ceramic:0.4`
    pub fn parse(s: &str) -> Self {
        let mut t = Self::default();
        for entry in s.split(',') {
            let Some((name, value)) = entry.split_once(':') else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            if let Ok(v) = value.trim().parse::<f32>() {
                if v.is_finite() && v >= 0.0 {
                    t.set(name, v);
                }
            }
        }
        t
    }

    pub fn to_text(&self) -> String {
        self.0
            .iter()
            .map(|(name, v)| format!("{name}:{v}"))
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// The four events a strategy piece makes noise for, plus the two a player
/// hears when they command it. Each holds a namespace-relative asset key
/// (`sfx/pack/ready`); empty means the class has no sound for that event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnitSounds {
    /// Acknowledges being selected.
    pub select: String,
    /// Acknowledges a move order.
    pub r#move: String,
    /// Acknowledges an attack order.
    pub attack: String,
    pub death: String,
    /// Took damage.
    pub hit: String,
    /// A structure finished building.
    pub build: String,
}

impl UnitSounds {
    pub fn is_empty(&self) -> bool {
        self.select.is_empty()
            && self.r#move.is_empty()
            && self.attack.is_empty()
            && self.death.is_empty()
            && self.hit.is_empty()
            && self.build.is_empty()
    }
}

/// One weapon a unit may carry. `id` is local to the asset: the `unit`
/// line's `weapon=`/`weapon2=` name one of these.
#[derive(Clone, Debug, PartialEq)]
pub struct UnitWeapon {
    pub id: String,
    /// Hit points per landed shot, before [`UnitWeapon::versus`].
    pub damage: f32,
    /// Shots per second.
    pub rate: f32,
    /// Metres.
    pub range: f32,
    pub delivery: WeaponDelivery,
    /// Metres/second. Zero on a hitscan weapon.
    pub projectile_speed: f32,
    pub splash_radius: f32,
    pub splash_damage: f32,
    /// Namespace-relative asset keys for the shot's presentation.
    pub projectile_sprite: String,
    pub impact: String,
    pub impact_sound: String,
    pub fire_sound: String,
    pub versus: VersusTable,
}

impl Default for UnitWeapon {
    fn default() -> Self {
        Self {
            id: String::new(),
            damage: 0.0,
            rate: 1.0,
            range: 0.0,
            delivery: WeaponDelivery::Hitscan,
            projectile_speed: 0.0,
            splash_radius: 0.0,
            splash_damage: 0.0,
            projectile_sprite: String::new(),
            impact: String::new(),
            impact_sound: String::new(),
            fire_sound: String::new(),
            versus: VersusTable::default(),
        }
    }
}

impl UnitWeapon {
    /// Damage this weapon deals to a body wearing `armor` (an armour NAME).
    pub fn damage_versus(&self, armor: &str) -> f32 {
        self.damage * self.versus.get(armor)
    }

    pub fn to_manifest(&self) -> String {
        let mut out = format!(
            "weapon id={} damage={} rate={} range={} delivery={}",
            self.id,
            self.damage,
            self.rate,
            self.range,
            self.delivery.as_str()
        );
        if self.projectile_speed > 0.0 {
            out.push_str(&format!(" projectile_speed={}", self.projectile_speed));
        }
        if self.splash_radius > 0.0 {
            out.push_str(&format!(" splash_radius={}", self.splash_radius));
        }
        if self.splash_damage > 0.0 {
            out.push_str(&format!(" splash_damage={}", self.splash_damage));
        }
        for (key, value) in [
            ("projectile_sprite", &self.projectile_sprite),
            ("impact", &self.impact),
            ("impact_sound", &self.impact_sound),
            ("fire", &self.fire_sound),
        ] {
            if !value.is_empty() {
                out.push_str(&format!(" {key}={value}"));
            }
        }
        if !self.versus.is_default() {
            out.push_str(&format!(" versus={}", self.versus.to_text()));
        }
        out.push('\n');
        out
    }

    /// Parse the body of a `weapon` line (everything after the tag).
    /// Returns `None` when the line names no `id`.
    pub fn from_manifest(rest: &str) -> Option<Self> {
        let kv = parse_kv(rest);
        let get = |k: &str| {
            kv.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
                .filter(|v| *v != "-")
        };
        let num = |k: &str, d: f32| get(k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(d);
        let text = |k: &str| get(k).unwrap_or("").to_string();
        let id = text("id");
        if id.is_empty() {
            return None;
        }
        Some(Self {
            id,
            damage: num("damage", 0.0),
            rate: num("rate", 1.0),
            range: num("range", 0.0),
            delivery: get("delivery")
                .and_then(WeaponDelivery::parse)
                .unwrap_or(WeaponDelivery::Hitscan),
            projectile_speed: num("projectile_speed", 0.0),
            splash_radius: num("splash_radius", 0.0),
            splash_damage: num("splash_damage", 0.0),
            projectile_sprite: text("projectile_sprite"),
            impact: text("impact"),
            impact_sound: text("impact_sound"),
            fire_sound: text("fire"),
            versus: get("versus").map(VersusTable::parse).unwrap_or_default(),
        })
    }
}

// ---------------------------------------------------------------------------
// VISUAL — one slot, two backends
// ---------------------------------------------------------------------------

/// WHICH BACKEND draws a piece. The default is the sprite card every
/// converted pack ships, so a manifest that says nothing keeps drawing
/// exactly as it did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VisualKind {
    /// A camera-facing sprite sheet, frame-picked from the heading.
    #[default]
    Sprite,
    /// A 3D model (GLB) from the asset store, rotated by the piece's yaw.
    Model,
}

impl VisualKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sprite => "sprite",
            Self::Model => "model",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "sprite" | "card" | "billboard" => Self::Sprite,
            "model" | "mesh" | "glb" => Self::Model,
            _ => return None,
        })
    }
}

/// What kind of shadow a model piece asks for. DECLARED intent: the engine
/// picks the shadow lane it has for the piece's draw path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShadowKind {
    /// A cheap ground blob — what a mover wants.
    #[default]
    Blob,
    /// The model's own geometry, cast properly.
    Mesh,
    /// No shadow at all.
    None,
}

impl ShadowKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blob => "blob",
            Self::Mesh => "mesh",
            Self::None => "none",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "blob" => Self::Blob,
            "mesh" => Self::Mesh,
            "none" | "off" | "0" => Self::None,
            _ => return None,
        })
    }
}

/// How BIG a model piece is drawn: a named play size from the shared
/// dimensions vocabulary ([`ScalePreset`]), or an explicit multiplier.
///
/// Both are multipliers of the piece's own FIT — the uniform scale that
/// contains the model inside the body the piece already occupies (a
/// structure's footprint in cells, a mover's own box) — so a definition
/// that names no scale lands at a sane size on any cell size, and `comic`
/// (the pack's authored play size) IS that fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VisualScale {
    Preset(ScalePreset),
    /// An explicit multiplier of the fit, as `scale=1.5` says.
    Exact(f32),
}

impl Default for VisualScale {
    fn default() -> Self {
        Self::Preset(ScalePreset::Comic)
    }
}

impl VisualScale {
    /// `comic` / `real` / `small` / `handheld`, or a number.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if let Some(p) = ScalePreset::parse(s) {
            return Some(Self::Preset(p));
        }
        let v: f32 = s.parse().ok()?;
        (v.is_finite() && v > 0.0).then_some(Self::Exact(v))
    }

    pub fn to_text(self) -> String {
        match self {
            Self::Preset(p) => p.as_str().to_string(),
            Self::Exact(v) => format!("{v}"),
        }
    }
}

/// The five states a strategy piece drives its rig through, and the ONE
/// vocabulary that resolves them. A definition may name the exact clip for
/// any of them; anything it leaves empty falls back to the shared
/// substring vocabulary in `makepad_render::skin` (`ClipRole`), which is
/// what makes an arbitrary rig from the store animate with no authoring.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VisualClips {
    pub idle: String,
    pub r#move: String,
    pub fire: String,
    pub die: String,
    pub deploy: String,
}

impl VisualClips {
    pub const ROLES: [&'static str; 5] = ["idle", "move", "fire", "die", "deploy"];

    pub fn is_empty(&self) -> bool {
        self.idle.is_empty()
            && self.r#move.is_empty()
            && self.fire.is_empty()
            && self.die.is_empty()
            && self.deploy.is_empty()
    }

    pub fn get(&self, role: &str) -> &str {
        match role {
            "idle" => &self.idle,
            "move" => &self.r#move,
            "fire" => &self.fire,
            "die" => &self.die,
            "deploy" => &self.deploy,
            _ => "",
        }
    }

    pub fn set(&mut self, role: &str, clip: &str) {
        let slot = match role {
            "idle" => &mut self.idle,
            "move" => &mut self.r#move,
            "fire" => &mut self.fire,
            "die" => &mut self.die,
            "deploy" => &mut self.deploy,
            _ => return,
        };
        *slot = clip.to_string();
    }

    /// `idle:Idle,move:Drive,fire:Shoot`
    pub fn parse(s: &str) -> Self {
        let mut out = Self::default();
        for entry in s.split(',') {
            let Some((role, clip)) = entry.split_once(':') else {
                continue;
            };
            out.set(role.trim().to_ascii_lowercase().as_str(), clip.trim());
        }
        out
    }

    pub fn to_text(&self) -> String {
        Self::ROLES
            .iter()
            .filter(|role| !self.get(role).is_empty())
            .map(|role| format!("{role}:{}", self.get(role)))
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// How a piece is DRAWN — the one slot with two backends (RTS_MASHUP §2.1).
///
/// ```text
/// unit class=vehicle title="Medium Tank" cost=800 hp=400 turret=kenney/x/turret
///      visual=model key=kenney/car-kit/truck-flat scale=comic yaw_offset=180
///      shadow=blob clips=idle:Idle,move:Drive,fire:Shoot
/// ```
///
/// `turret=` is the SAME field the sprite lane uses: a sprite piece reads it
/// as a second sheet, a model piece as a second GLB aimed on its own
/// heading.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitVisual {
    pub kind: VisualKind,
    /// The model's asset-store alias (`key=`). Empty on the sprite lane.
    pub model: String,
    pub scale: VisualScale,
    /// DEGREES added to the piece's heading before the model is drawn: the
    /// correction for a GLB authored facing the other way. An asset fact,
    /// not an engine convention — which is why it rides on the definition.
    pub yaw_offset: f32,
    pub shadow: ShadowKind,
    pub clips: VisualClips,
}

impl UnitVisual {
    pub fn is_model(&self) -> bool {
        self.kind == VisualKind::Model
    }

    /// Nothing to write: the sprite lane with no model facts recorded.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The `visual=…` half of a `unit` line, keys in declaration order.
    pub fn to_manifest(&self) -> String {
        let mut out = format!(" visual={}", self.kind.as_str());
        if !self.model.is_empty() {
            out.push_str(&format!(" key={}", self.model));
        }
        if self.scale != VisualScale::default() {
            out.push_str(&format!(" scale={}", self.scale.to_text()));
        }
        if self.yaw_offset != 0.0 {
            out.push_str(&format!(" yaw_offset={}", self.yaw_offset));
        }
        if self.shadow != ShadowKind::default() {
            out.push_str(&format!(" shadow={}", self.shadow.as_str()));
        }
        if !self.clips.is_empty() {
            out.push_str(&format!(" clips={}", self.clips.to_text()));
        }
        out
    }
}

/// The definition of one buildable/placeable piece, as carried on its
/// sprite asset's manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct UnitDef {
    pub class: UnitClass,
    pub title: String,
    /// Credits.
    pub cost: i32,
    /// Hit points at full health.
    pub hp: f32,
    /// What this body is made of, by NAME (`none`, `heavy`, `ceramic`, …).
    /// Empty reads as `none`. The world interns whatever names its
    /// definitions use into an [`ArmorTable`].
    pub armor: String,
    /// Metres/second (movers only).
    pub speed: f32,
    /// Metres — the radius at which an idle unit notices an enemy.
    pub sight: f32,
    /// Radians/second of turn rate. Zero = turns instantly.
    pub turn: f32,
    /// House names allowed to build this. Empty = every house.
    pub sides: Vec<String>,
    /// ROLES that must already stand (`barracks`, `radar`, …). A pack key
    /// here is read as one until its converter emits roles.
    pub prereq: Vec<String>,
    /// What this structure IS across packs: `conyard`, `power`, `refinery`,
    /// `barracks`, `vehicle_factory`, `radar`, `tech`, `defense`, … Empty on
    /// anything that is not a building. THE mashup key: a unit from one pack
    /// queues at another pack's factory because both name the same role.
    pub role: String,
    /// The ROLE of the structure that builds this unit.
    pub builds_at: String,
    /// The structure class key that builds this (legacy; `builds_at` is the
    /// role that replaces it).
    pub producer: String,
    /// Power contributed (positive) or drawn (negative).
    pub power: i32,
    /// Footprint in CELLS (structures).
    pub footprint: (u32, u32),
    /// Namespace-relative key of a separate turret sheet, if the artwork
    /// draws the turret on its own heading.
    pub turret: String,
    /// Namespace-relative key of the sidebar icon, when the pack ships one
    /// separately from the sheet.
    pub icon: String,
    /// This piece can be produced at all.
    pub buildable: bool,
    /// Gathers from resource cells.
    pub harvester: bool,
    /// A harvester's load capacity, in resource units.
    pub capacity: i32,
    /// Accepts harvester loads and converts them to credits.
    pub refinery: bool,
    /// A mobile piece that deploys into `deploys`.
    pub deployer: bool,
    /// The structure key a deployer becomes.
    pub deploys: String,
    /// A unit this structure hands its owner the moment it goes down — a
    /// refinery arriving with its first gatherer. Namespace-relative short
    /// key; empty for everything else.
    pub grants: String,
    /// Drives over infantry.
    pub crushes: bool,
    /// Tech level gate: production is refused while the round's tech level
    /// is below this. Zero = ungated.
    pub tech: i32,
    /// How long this takes to build, SECONDS. Zero = derive it from `cost`
    /// at the round's `credits_per_second`, which is what every pack that
    /// never authored a build time gets.
    pub build_time: f32,
    /// Local weapon ids from [`UnitDef::weapons`].
    pub weapon: String,
    pub weapon2: String,
    pub sounds: UnitSounds,
    /// Every `weapon` line on the manifest, in file order.
    pub weapons: Vec<UnitWeapon>,
    /// HOW this piece is drawn: the sprite card lane (the default, so
    /// nothing changes unless a manifest declares otherwise) or a 3D
    /// model. RTS_MASHUP §2.1 — one slot, two backends.
    pub visual: UnitVisual,
}

impl Default for UnitDef {
    fn default() -> Self {
        Self {
            class: UnitClass::Vehicle,
            title: String::new(),
            cost: 0,
            hp: 0.0,
            armor: String::new(),
            speed: 0.0,
            sight: 0.0,
            turn: 0.0,
            sides: Vec::new(),
            prereq: Vec::new(),
            role: String::new(),
            builds_at: String::new(),
            producer: String::new(),
            power: 0,
            footprint: (1, 1),
            turret: String::new(),
            icon: String::new(),
            buildable: false,
            harvester: false,
            capacity: 0,
            refinery: false,
            deployer: false,
            deploys: String::new(),
            grants: String::new(),
            crushes: false,
            tech: 0,
            build_time: 0.0,
            weapon: String::new(),
            weapon2: String::new(),
            sounds: UnitSounds::default(),
            weapons: Vec::new(),
            visual: UnitVisual::default(),
        }
    }
}

impl UnitDef {
    /// The primary weapon's definition, if the manifest carries one.
    pub fn primary_weapon(&self) -> Option<&UnitWeapon> {
        self.weapon_by_id(&self.weapon)
    }

    pub fn secondary_weapon(&self) -> Option<&UnitWeapon> {
        self.weapon_by_id(&self.weapon2)
    }

    pub fn weapon_by_id(&self, id: &str) -> Option<&UnitWeapon> {
        if id.is_empty() {
            return None;
        }
        self.weapons.iter().find(|w| w.id == id)
    }

    /// The three manifest lines, ready to append to a sprite manifest.
    pub fn to_manifest(&self) -> String {
        let mut out = format!("unit class={}", self.class.as_str());
        if !self.title.is_empty() {
            out.push_str(&format!(" title=\"{}\"", self.title.replace('"', "'")));
        }
        for (key, value) in [
            ("cost", self.cost),
            ("tech", self.tech),
            ("capacity", self.capacity),
        ] {
            if value != 0 {
                out.push_str(&format!(" {key}={value}"));
            }
        }
        if self.hp > 0.0 {
            out.push_str(&format!(" hp={}", self.hp));
        }
        out.push_str(&format!(
            " armor={}",
            if self.armor.is_empty() { "none" } else { self.armor.as_str() }
        ));
        if self.build_time > 0.0 {
            out.push_str(&format!(" build_time={}", self.build_time));
        }
        for (key, value) in [("speed", self.speed), ("sight", self.sight), ("turn", self.turn)] {
            if value > 0.0 {
                out.push_str(&format!(" {key}={value}"));
            }
        }
        if !self.sides.is_empty() {
            out.push_str(&format!(" sides={}", self.sides.join(",")));
        }
        if !self.prereq.is_empty() {
            out.push_str(&format!(" prereq={}", self.prereq.join(",")));
        }
        if !self.producer.is_empty() {
            out.push_str(&format!(" producer={}", self.producer));
        }
        if self.power != 0 {
            out.push_str(&format!(" power={}", self.power));
        }
        if self.footprint != (1, 1) {
            out.push_str(&format!(" footprint={}x{}", self.footprint.0, self.footprint.1));
        }
        if !self.turret.is_empty() {
            out.push_str(&format!(" turret={}", self.turret));
        }
        for (key, value) in [
            ("role", &self.role),
            ("builds_at", &self.builds_at),
            ("icon", &self.icon),
        ] {
            if !value.is_empty() {
                out.push_str(&format!(" {key}={value}"));
            }
        }
        for (key, value) in [
            ("build", self.buildable),
            ("harvester", self.harvester),
            ("refinery", self.refinery),
            ("deploy", self.deployer),
            ("crushes", self.crushes),
        ] {
            if value {
                out.push_str(&format!(" {key}=1"));
            }
        }
        if !self.deploys.is_empty() {
            out.push_str(&format!(" deploys={}", self.deploys));
        }
        if !self.grants.is_empty() {
            out.push_str(&format!(" grants={}", self.grants));
        }
        if !self.weapon.is_empty() {
            out.push_str(&format!(" weapon={}", self.weapon));
        }
        if !self.weapon2.is_empty() {
            out.push_str(&format!(" weapon2={}", self.weapon2));
        }
        // The visual slot is written only when it says something: a pack
        // that never declared one round-trips byte for byte.
        if !self.visual.is_default() {
            out.push_str(&self.visual.to_manifest());
        }
        out.push('\n');
        let s = &self.sounds;
        if !s.is_empty() {
            out.push_str("sound");
            for (key, value) in [
                ("select", &s.select),
                ("move", &s.r#move),
                ("attack", &s.attack),
                ("death", &s.death),
                ("hit", &s.hit),
                ("build", &s.build),
            ] {
                if !value.is_empty() {
                    out.push_str(&format!(" {key}={value}"));
                }
            }
            out.push('\n');
        }
        for w in &self.weapons {
            out.push_str(&w.to_manifest());
        }
        out
    }

    /// Build a definition from a manifest's `(tag, rest)` lines. Returns
    /// `None` when there is no `unit` line — a sheet without one is scenery.
    pub fn from_manifest(lines: &[(&str, &str)]) -> Option<Self> {
        let mut def: Option<UnitDef> = None;
        let mut sounds = UnitSounds::default();
        let mut weapons: Vec<UnitWeapon> = Vec::new();
        for (tag, rest) in lines {
            match *tag {
                "unit" => {
                    let kv = parse_kv(rest);
                    let get = |k: &str| {
                        kv.iter()
                            .find(|(key, _)| key == k)
                            .map(|(_, v)| v.as_str())
                            .filter(|v| *v != "-")
                    };
                    let num = |k: &str, d: f32| get(k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(d);
                    let int = |k: &str, d: i32| {
                        get(k)
                            .and_then(|v| v.trim_start_matches('+').parse::<i32>().ok())
                            .unwrap_or(d)
                    };
                    let flag = |k: &str| num(k, 0.0) != 0.0;
                    let text = |k: &str| get(k).unwrap_or("").to_string();
                    let list = |k: &str| {
                        get(k)
                            .map(|v| {
                                v.split(',')
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                                    .map(String::from)
                                    .collect()
                            })
                            .unwrap_or_default()
                    };
                    // A second `unit` line merges into the first, so a
                    // writer may wrap a long definition over two lines.
                    let base = def.take().unwrap_or_default();
                    def = Some(UnitDef {
                        class: get("class").and_then(UnitClass::parse).unwrap_or(base.class),
                        title: if get("title").is_some() { text("title") } else { base.title },
                        cost: int("cost", base.cost),
                        hp: num("hp", base.hp),
                        armor: if get("armor").is_some() { text("armor") } else { base.armor },
                        speed: num("speed", base.speed),
                        sight: num("sight", base.sight),
                        turn: num("turn", base.turn),
                        sides: if get("sides").is_some() { list("sides") } else { base.sides },
                        prereq: if get("prereq").is_some() { list("prereq") } else { base.prereq },
                        producer: if get("producer").is_some() { text("producer") } else { base.producer },
                        power: int("power", base.power),
                        footprint: get("footprint").and_then(parse_footprint).unwrap_or(base.footprint),
                        turret: if get("turret").is_some() { text("turret") } else { base.turret },
                        role: if get("role").is_some() { text("role") } else { base.role },
                        builds_at: if get("builds_at").is_some() {
                            text("builds_at")
                        } else {
                            base.builds_at
                        },
                        icon: if get("icon").is_some() { text("icon") } else { base.icon },
                        buildable: if get("build").is_some() { flag("build") } else { base.buildable },
                        harvester: if get("harvester").is_some() { flag("harvester") } else { base.harvester },
                        capacity: int("capacity", base.capacity),
                        refinery: if get("refinery").is_some() { flag("refinery") } else { base.refinery },
                        deployer: if get("mcv").is_some() {
                            flag("mcv")
                        } else if get("deploy").is_some() {
                            flag("deploy")
                        } else {
                            base.deployer
                        },
                        deploys: if get("deploys").is_some() { text("deploys") } else { base.deploys },
                        grants: if get("grants").is_some() { text("grants") } else { base.grants },
                        crushes: if get("crushes").is_some() { flag("crushes") } else { base.crushes },
                        tech: int("tech", base.tech),
                        build_time: num("build_time", base.build_time),
                        weapon: if get("weapon").is_some() { text("weapon") } else { base.weapon },
                        weapon2: if get("weapon2").is_some() { text("weapon2") } else { base.weapon2 },
                        sounds: base.sounds,
                        weapons: base.weapons,
                        // The visual slot: `visual=` picks the backend and
                        // the five keys beside it fill it in. Each merges
                        // on its own, so a second `unit` line may add the
                        // clips to a model declared on the first.
                        visual: {
                            let mut v = base.visual;
                            if let Some(kind) = get("visual").and_then(VisualKind::parse) {
                                v.kind = kind;
                            }
                            if get("key").is_some() {
                                v.model = text("key");
                            }
                            if let Some(scale) = get("scale").and_then(VisualScale::parse) {
                                v.scale = scale;
                            }
                            if get("yaw_offset").is_some() {
                                v.yaw_offset = num("yaw_offset", v.yaw_offset);
                            }
                            if let Some(shadow) = get("shadow").and_then(ShadowKind::parse) {
                                v.shadow = shadow;
                            }
                            if let Some(clips) = get("clips") {
                                let clips = VisualClips::parse(clips);
                                for role in VisualClips::ROLES {
                                    if !clips.get(role).is_empty() {
                                        v.clips.set(role, clips.get(role));
                                    }
                                }
                            }
                            v
                        },
                    });
                }
                "sound" => {
                    for (k, v) in parse_kv(rest) {
                        if v == "-" {
                            continue;
                        }
                        match k.as_str() {
                            "select" => sounds.select = v,
                            "move" => sounds.r#move = v,
                            "attack" => sounds.attack = v,
                            "death" => sounds.death = v,
                            "hit" => sounds.hit = v,
                            "build" => sounds.build = v,
                            _ => {}
                        }
                    }
                }
                "weapon" => {
                    if let Some(w) = UnitWeapon::from_manifest(rest) {
                        weapons.push(w);
                    }
                }
                _ => {}
            }
        }
        let mut def = def?;
        def.sounds = sounds;
        def.weapons = weapons;
        // A definition that names no weapon but carries exactly one takes it
        // as its primary, so a one-gun sheet need not repeat the id.
        if def.weapon.is_empty() {
            if let [only] = def.weapons.as_slice() {
                def.weapon = only.id.clone();
            }
        }
        Some(def)
    }
}

/// `3x2` → `(3, 2)`.
fn parse_footprint(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once(['x', 'X'])?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(text: &str) -> Vec<(String, String)> {
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| {
                let l = l.trim();
                let (tag, rest) = l.split_once(char::is_whitespace)?;
                Some((tag.to_string(), rest.trim().to_string()))
            })
            .collect()
    }

    fn parse(text: &str) -> Option<UnitDef> {
        let owned = refs(text);
        let borrowed: Vec<(&str, &str)> =
            owned.iter().map(|(t, r)| (t.as_str(), r.as_str())).collect();
        UnitDef::from_manifest(&borrowed)
    }

    #[test]
    fn a_vehicle_definition_round_trips() {
        let def = UnitDef {
            class: UnitClass::Vehicle,
            title: "Medium Tank".into(),
            cost: 800,
            hp: 400.0,
            armor: "heavy".into(),
            speed: 6.5,
            sight: 30.0,
            turn: 3.0,
            sides: vec!["house_a".into(), "house_b".into()],
            prereq: vec!["factory".into()],
            producer: "factory".into(),
            power: 0,
            footprint: (1, 1),
            turret: "billboards/pack/tank-turret".into(),
            buildable: true,
            crushes: true,
            weapon: "cannon".into(),
            weapons: vec![UnitWeapon {
                id: "cannon".into(),
                damage: 30.0,
                rate: 1.2,
                range: 27.0,
                delivery: WeaponDelivery::Projectile,
                projectile_speed: 60.0,
                projectile_sprite: "billboards/pack/shell".into(),
                impact: "billboards/pack/puff".into(),
                versus: VersusTable::parse("none:1.0,wood:0.75,light:0.6,heavy:0.25,concrete:0.1"),
                ..UnitWeapon::default()
            }],
            sounds: UnitSounds {
                select: "sfx/pack/ready".into(),
                r#move: "sfx/pack/ackno".into(),
                death: "sfx/pack/boom".into(),
                ..UnitSounds::default()
            },
            ..UnitDef::default()
        };
        let text = def.to_manifest();
        assert!(text.contains("unit class=vehicle title=\"Medium Tank\""), "{text}");
        assert!(text.contains("versus=none:1,wood:0.75,light:0.6,heavy:0.25,concrete:0.1"), "{text}");
        let back = parse(&text).expect("parse");
        assert_eq!(back, def);
    }

    #[test]
    fn versus_multiplies_damage_per_armour_class() {
        let w = UnitWeapon {
            damage: 100.0,
            versus: VersusTable::parse("none:1.0,wood:0.75,light:0.6,heavy:0.25,concrete:0.1"),
            ..UnitWeapon::default()
        };
        assert_eq!(w.damage_versus("none"), 100.0);
        assert_eq!(w.damage_versus("heavy"), 25.0);
        assert_eq!(w.damage_versus("concrete"), 10.0);
        // A weapon with no table hits everything for full damage.
        let plain = UnitWeapon { damage: 40.0, ..UnitWeapon::default() };
        assert_eq!(plain.damage_versus("concrete"), 40.0);
        // An entry the table forgot stays at 1.0.
        let partial = UnitWeapon {
            damage: 10.0,
            versus: VersusTable::parse("heavy:0.5"),
            ..UnitWeapon::default()
        };
        assert_eq!(partial.damage_versus("wood"), 10.0);
        assert_eq!(partial.damage_versus("heavy"), 5.0);
        // A name nobody built in: declared, so it works.
        let ceramic = UnitWeapon {
            damage: 10.0,
            versus: VersusTable::parse("ceramic:0.4"),
            ..UnitWeapon::default()
        };
        assert_eq!(ceramic.damage_versus("ceramic"), 4.0);
        assert_eq!(ceramic.damage_versus("heavy"), 10.0);
    }

    #[test]
    fn a_structure_carries_footprint_power_and_refinery() {
        let def = parse(
            "unit class=structure title=\"Ore Refinery\" cost=2000 hp=900 armor=concrete \
             footprint=3x2 power=-30 refinery=1 build=1 producer=yard prereq=power\n",
        )
        .expect("def");
        assert_eq!(def.class, UnitClass::Structure);
        assert_eq!(def.footprint, (3, 2));
        assert_eq!(def.power, -30);
        assert!(def.refinery);
        assert!(def.buildable);
        assert_eq!(def.prereq, vec!["power".to_string()]);
        assert!(def.class.is_static());
        assert!(!def.class.is_mover());
    }

    #[test]
    fn a_sheet_without_a_unit_line_is_scenery() {
        assert!(parse("state idle 0 0 1 8\nframe 0 A 0 12 12 t.png\n").is_none());
    }

    #[test]
    fn a_lone_weapon_line_becomes_the_primary() {
        let def = parse("unit class=infantry hp=50\nweapon id=rifle damage=5 rate=2 range=15\n")
            .expect("def");
        assert_eq!(def.weapon, "rifle");
        assert_eq!(def.primary_weapon().map(|w| w.damage), Some(5.0));
        assert!(def.secondary_weapon().is_none());
    }

    #[test]
    fn a_wrapped_unit_line_merges() {
        let def = parse(
            "unit class=vehicle hp=200 speed=9\nunit sight=24 weapon=gun crushes=1\n\
             weapon id=gun damage=8 rate=4 range=12\n",
        )
        .expect("def");
        assert_eq!(def.hp, 200.0);
        assert_eq!(def.speed, 9.0);
        assert_eq!(def.sight, 24.0);
        assert!(def.crushes);
        assert_eq!(def.weapon, "gun");
    }

    #[test]
    fn a_visual_model_slot_round_trips() {
        let def = parse(
            "unit class=vehicle title=\"Medium Tank\" cost=800 hp=400 armor=heavy speed=6.5 \
             turret=kenney/tower-defense-kit/weapon-turret visual=model \
             key=kenney/car-kit/truck-flat scale=comic yaw_offset=180 shadow=blob \
             clips=idle:Idle,move:Drive,fire:Shoot,die:Destroyed,deploy:Unfold\n",
        )
        .expect("def");
        assert!(def.visual.is_model());
        assert_eq!(def.visual.model, "kenney/car-kit/truck-flat");
        assert_eq!(def.visual.scale, VisualScale::Preset(ScalePreset::Comic));
        assert_eq!(def.visual.yaw_offset, 180.0);
        assert_eq!(def.visual.shadow, ShadowKind::Blob);
        assert_eq!(def.visual.clips.r#move, "Drive");
        assert_eq!(def.visual.clips.die, "Destroyed");
        assert_eq!(def.visual.clips.deploy, "Unfold");
        // The turret is the SAME field the sprite lane uses.
        assert_eq!(def.turret, "kenney/tower-defense-kit/weapon-turret");
        // And it survives a write.
        let back = parse(&def.to_manifest()).expect("re-parse");
        assert_eq!(back.visual, def.visual);
        assert_eq!(back, def);
    }

    #[test]
    fn a_definition_that_declares_no_visual_stays_a_sprite_and_writes_nothing() {
        let def = parse("unit class=infantry hp=50 speed=4\n").expect("def");
        assert_eq!(def.visual, UnitVisual::default());
        assert!(!def.visual.is_model());
        let text = def.to_manifest();
        assert!(!text.contains("visual="), "{text}");
        assert!(!text.contains("scale="), "{text}");
        assert_eq!(parse(&text).expect("re-parse").visual, def.visual);
    }

    #[test]
    fn a_visual_slot_merges_across_two_unit_lines_and_takes_an_exact_scale() {
        let def = parse(
            "unit class=vehicle visual=model key=pack/tank clips=idle:Idle\n\
             unit scale=1.5 clips=move:Drive shadow=none\n",
        )
        .expect("def");
        assert_eq!(def.visual.model, "pack/tank");
        assert_eq!(def.visual.scale, VisualScale::Exact(1.5));
        assert_eq!(def.visual.shadow, ShadowKind::None);
        // The first line's clip survived the second line's clip list.
        assert_eq!(def.visual.clips.idle, "Idle");
        assert_eq!(def.visual.clips.r#move, "Drive");
        assert_eq!(parse(&def.to_manifest()).expect("re-parse").visual, def.visual);
    }

    #[test]
    fn a_positive_power_value_may_be_signed() {
        let def = parse("unit class=structure power=+20\n").expect("def");
        assert_eq!(def.power, 20);
    }
}
