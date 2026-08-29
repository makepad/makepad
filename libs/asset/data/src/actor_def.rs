//! What a placed thing IS, beyond where it stands: the import database's
//! behaviour table.
//!
//! [`world_place`](crate::world_place) already answers "which artwork, which
//! spot, which coarse kind". That is enough to DRAW a level and nothing more:
//! every creature ends up equally tough, every floor item is scenery, and no
//! event has a sound. The missing half is per-class behaviour — hit points,
//! how hard something hits, what it gives you when you walk over it, which
//! sound each event plays — and it belongs here, beside the placement table,
//! for the same reason: it is a fact about the SOURCE FORMAT, not about the
//! engine. The engine reads [`ActorDef`] and knows nothing about which game
//! filled it in.
//!
//! Three deliberate choices:
//!
//! - **Slots, not sounds.** An actor names its `sight`/`pain`/`death`/
//!   `attack`/`active` sounds as bare asset stems. Resolving a stem to a
//!   catalog key is the runtime's business ([`sound_alias`]), so the same
//!   table serves a pack whose audio has not been imported yet — the slot is
//!   simply empty and the event falls back to its category default.
//! - **Units are the engine's, not the source's.** Speeds are metres/second
//!   and damage is hit points, converted once here rather than smeared
//!   through the runtime. The derivations are written down next to the
//!   numbers so they can be argued with.
//! - **Absent is honest.** A class this table does not know returns `None`
//!   and the caller applies its own generic default. Guessing a stat is
//!   worse than admitting there is none.

/// What a placement DOES. Distinct from `world_place::Place::kind`, which is
/// the artwork's coarse role — a "pickup" row is scenery until this says what
/// picking it up means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorRole {
    /// Hunts, takes damage, dies.
    Monster,
    /// Vanishes on touch and grants [`ActorDef::give`].
    Item,
    /// An item that also arms a weapon.
    Weapon,
    /// Stands there. Collides if the source said so; never reacts.
    Prop,
    /// A spawn anchor, not a body.
    Player,
}

impl ActorRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Monster => "monster",
            Self::Item => "item",
            Self::Weapon => "weapon",
            Self::Prop => "prop",
            Self::Player => "player",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "monster" => Self::Monster,
            "item" => Self::Item,
            "weapon" => Self::Weapon,
            "prop" => Self::Prop,
            "player" => Self::Player,
            _ => return None,
        })
    }
}

/// How harm is delivered. The engine owns the geometry of each; this only
/// says which one and how much.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttackKind {
    /// Contact range only.
    Melee,
    /// Instant ray along the aim line.
    Hitscan,
    /// A travelling shot.
    Projectile,
}

impl AttackKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Melee => "melee",
            Self::Hitscan => "hitscan",
            Self::Projectile => "projectile",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "melee" => Self::Melee,
            "hitscan" => Self::Hitscan,
            "projectile" => Self::Projectile,
            _ => return None,
        })
    }
}

/// One attack. `damage` is the EXPECTED value of the source's damage roll,
/// not its maximum — a table of maxima makes every fight lethal, and the
/// engine has no reason to reproduce a particular RNG.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActorAttack {
    pub kind: AttackKind,
    /// Hit points per landed attack (all pellets/rays of one trigger pull).
    pub damage: f32,
    /// Attacks per second.
    pub rate: f32,
    /// Metres. For melee this is the reach; for the others, the useful range.
    pub range: f32,
    /// Projectile speed, m/s. Zero for melee and hitscan.
    pub speed: f32,
    /// Cone half-angle in radians. Zero is exact.
    pub spread: f32,
}

/// The five events a creature makes noise for. Each holds a bare asset stem
/// (`"dspistol"`), not a catalog key — see [`sound_alias`].
///
/// Empty means "this class has no sound for that event", which is a real
/// answer (a Lost Soul has no sight sound) and not a missing entry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActorSounds {
    /// First time it notices you.
    pub sight: &'static str,
    /// It was hurt (played at most `pain_chance` of the time).
    pub pain: &'static str,
    pub death: &'static str,
    pub attack: &'static str,
    /// Idle mutter while hunting.
    pub active: &'static str,
    /// Death by overkill (the wet burst); falls back to `death`.
    pub gib: &'static str,
    /// A footfall, when the class walks audibly.
    pub step: &'static str,
    /// A loop the thing hums while it stands (a fan, a fire, a Quake
    /// `ambient_*`); loop-clean by the asset contract.
    pub ambient: &'static str,
}

impl ActorSounds {
    pub const NONE: ActorSounds = ActorSounds {
        sight: "",
        pain: "",
        death: "",
        attack: "",
        active: "",
        gib: "",
        step: "",
        ambient: "",
    };

    pub fn slot(&self, name: &str) -> &'static str {
        match name {
            "sight" => self.sight,
            "pain" => self.pain,
            "death" => self.death,
            "attack" => self.attack,
            "active" => self.active,
            "gib" => self.gib,
            "step" => self.step,
            "ambient" => self.ambient,
            _ => "",
        }
    }
}

/// What walking over an item does. Every field is additive and clamped by the
/// engine against the player's own maxima; `_max` fields RAISE those maxima
/// (a soulsphere is `health: 100, health_max: 200`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Give {
    pub health: f32,
    /// Raise the health ceiling to at least this. Zero leaves it alone.
    pub health_max: f32,
    pub armor: f32,
    pub armor_max: f32,
    /// Fraction of incoming damage the armour soaks (0.33 light, 0.5 heavy).
    /// Zero keeps whatever the player already wears.
    pub armor_absorb: f32,
    /// Ammo pool name (`"bullet"`, `"shell"`, `"rocket"`, `"cell"`) and how
    /// many. An empty name gives no ammo.
    pub ammo: &'static str,
    pub ammo_count: i32,
    /// Weapon id this item arms, if any (see [`weapon_def`]).
    pub weapon: &'static str,
    /// Key colour/name this item adds to the ring.
    pub key: &'static str,
    /// Double every ammo ceiling (a backpack).
    pub expand_ammo: bool,
    /// Human-readable pickup line for the message log.
    pub message: &'static str,
    /// Sound stem played on pickup, overriding the category default.
    pub sound: &'static str,
}

impl Give {
    pub const NONE: Give = Give {
        health: 0.0,
        health_max: 0.0,
        armor: 0.0,
        armor_max: 0.0,
        armor_absorb: 0.0,
        ammo: "",
        ammo_count: 0,
        weapon: "",
        key: "",
        expand_ammo: false,
        message: "",
        sound: "",
    };
}

/// What a body does the instant it dies, beyond falling over: a burst that
/// harms everything around it. A generic property — a fuel drum, a mine, a
/// suicide bomber are all this — with the source's own numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Explosion {
    /// Metres. Harm falls off linearly to nothing at this distance.
    pub radius: f32,
    /// Hit points at the centre.
    pub damage: f32,
    /// Billboard asset that draws the burst in the body's place (a
    /// namespace-relative key, `billboards/doom1/bexp`), played once through
    /// its rest state, after which nothing is drawn. Empty draws nothing.
    pub sprite: &'static str,
    /// Sound of the burst (stem or namespace-relative key), else the body's
    /// own death sound.
    pub sound: &'static str,
}

/// A projectile attack's presentation: the thing that flies and the three
/// sounds of its life. All namespace-relative keys (or bare stems), like
/// every other slot.
///
/// ```text
/// projectile sprite=billboards/doom1/bal1 launch=sfx/doom1/dsfirsht fly=- hit=sfx/doom1/dsfirxpl
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProjectileDef {
    pub sprite: &'static str,
    /// Leaving the muzzle; falls back to `sound attack`.
    pub launch: &'static str,
    /// Looping while in flight, following the shot.
    pub fly: &'static str,
    /// Impact/expiry; falls back to the family's `explosion`.
    pub hit: &'static str,
}

/// Everything the engine needs to run one placed class.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActorDef {
    pub role: ActorRole,
    /// Starting hit points. Ignored for items and props.
    pub health: f32,
    /// Probability 0..1 that a hit interrupts into the pain state.
    pub pain_chance: f32,
    /// Chase speed, metres/second.
    pub speed: f32,
    /// Body radius and standing height, metres. Zero means "derive from the
    /// artwork", which is what the sprite pipeline already does.
    pub radius: f32,
    pub height: f32,
    /// How far it notices a target, metres.
    pub sight_range: f32,
    pub attack: Option<ActorAttack>,
    pub sounds: ActorSounds,
    pub give: Give,
    /// Does not block movement or take shots (Doom's decorative gore).
    pub passable: bool,
    /// Bursts on death, harming everything in reach.
    pub explode: Option<Explosion>,
    /// For a projectile attack: what flies and what it sounds like.
    pub projectile: Option<ProjectileDef>,
}

impl ActorDef {
    /// The shape every unknown class falls back to: a solid nothing. Callers
    /// override the fields they can justify.
    pub const PROP: ActorDef = ActorDef {
        role: ActorRole::Prop,
        health: 0.0,
        pain_chance: 0.0,
        speed: 0.0,
        radius: 0.0,
        height: 0.0,
        sight_range: 0.0,
        attack: None,
        sounds: ActorSounds::NONE,
        give: Give::NONE,
        passable: false,
        explode: None,
        projectile: None,
    };

    const fn monster(health: f32, pain_chance: f32, speed: f32) -> ActorDef {
        ActorDef {
            role: ActorRole::Monster,
            health,
            pain_chance,
            speed,
            sight_range: DEFAULT_SIGHT,
            ..ActorDef::PROP
        }
    }

    const fn item(give: Give) -> ActorDef {
        ActorDef {
            role: ActorRole::Item,
            give,
            ..ActorDef::PROP
        }
    }

    const fn weapon_pickup(give: Give) -> ActorDef {
        ActorDef {
            role: ActorRole::Weapon,
            give,
            ..ActorDef::PROP
        }
    }

    const fn with_attack(mut self, attack: ActorAttack) -> ActorDef {
        self.attack = Some(attack);
        self
    }

    const fn with_projectile(mut self, projectile: ProjectileDef) -> ActorDef {
        self.projectile = Some(projectile);
        self
    }

    const fn with_sounds(mut self, sounds: ActorSounds) -> ActorDef {
        self.sounds = sounds;
        self
    }

    const fn with_body(mut self, radius: f32, height: f32) -> ActorDef {
        self.radius = radius;
        self.height = height;
        self
    }

    /// This def with every LINEAR quantity — body, speed, sight, attack
    /// reach, projectile speed — multiplied by `scale`.
    ///
    /// The table's metres are written for a world whose people stand
    /// [`person_height`] tall. A map declares its own people (its walker
    /// body), and a map whose people are half that height is a world drawn
    /// at half the metre: the same creature must be half as tall, half as
    /// wide, half as fast and half as far-sighted there, or its body wedges
    /// under the map's ceilings (drawn feet-underground) and its gunfire
    /// crosses the whole level. Scalars — hit points, damage, rates, pain,
    /// spread angles — are not lengths and do not scale.
    pub fn scaled(mut self, scale: f32) -> ActorDef {
        self.speed *= scale;
        self.radius *= scale;
        self.height *= scale;
        self.sight_range *= scale;
        if let Some(attack) = &mut self.attack {
            attack.range *= scale;
            attack.speed *= scale;
        }
        if let Some(explode) = &mut self.explode {
            explode.radius *= scale;
        }
        self
    }

    /// This class as manifest lines — the form it travels in on its OWN
    /// asset (a billboard's `.billboard` text), so a runtime that resolves
    /// the artwork by alias has the behaviour in the same breath and never
    /// consults a table keyed by game. `resource` turns a bare resource stem
    /// (a sound lump, a burst sprite prefix) into the namespace-relative key
    /// the pack publishes it under; ids (weapon, ammo pool, key colour) and
    /// text pass through untouched.
    ///
    /// ```text
    /// actor role=monster health=20 pain=0.78125 speed=2.19 radius=0.625 height=1.75 sight=48 passable=0
    /// attack kind=hitscan damage=9 rate=1 range=64 speed=0 spread=0.09
    /// sound sight=sfx/doom1/dsposit1 pain=sfx/doom1/dspopain death=sfx/doom1/dspodth1 attack=sfx/doom1/dspistol active=sfx/doom1/dsposact
    /// give health=10 health_max=0 armor=0 armor_max=0 absorb=0 ammo=- count=0 weapon=- key=- expand=0 sound=- message="Picked up a stimpack."
    /// explode radius=4 damage=128 sprite=billboards/doom1/bexp sound=sfx/doom1/dsbarexp
    /// ```
    pub fn to_manifest(&self, resource: &dyn Fn(&str, ResourceKind) -> String) -> String {
        // Variant slots (`pain1|pain2`) map take by take: each variant is
        // its own resource.
        let res = |s: &str, k: ResourceKind| {
            if s.is_empty() {
                return "-".to_string();
            }
            s.split('|')
                .map(|v| resource(v, k))
                .collect::<Vec<_>>()
                .join("|")
        };
        let mut out = format!(
            "actor role={} health={} pain={} speed={} radius={} height={} sight={} passable={}\n",
            self.role.as_str(),
            self.health,
            self.pain_chance,
            self.speed,
            self.radius,
            self.height,
            self.sight_range,
            u8::from(self.passable)
        );
        if let Some(a) = self.attack {
            out.push_str(&format!(
                "attack kind={} damage={} rate={} range={} speed={} spread={}\n",
                a.kind.as_str(),
                a.damage,
                a.rate,
                a.range,
                a.speed,
                a.spread
            ));
        }
        let s = &self.sounds;
        if *s != ActorSounds::NONE {
            out.push_str(&format!(
                "sound sight={} pain={} death={} attack={} active={}",
                res(s.sight, ResourceKind::Sound),
                res(s.pain, ResourceKind::Sound),
                res(s.death, ResourceKind::Sound),
                res(s.attack, ResourceKind::Sound),
                res(s.active, ResourceKind::Sound)
            ));
            // Newer slots are written only when set, so a class without
            // them round-trips byte-identically with older writers.
            if !s.gib.is_empty() {
                out.push_str(&format!(" gib={}", res(s.gib, ResourceKind::Sound)));
            }
            if !s.step.is_empty() {
                out.push_str(&format!(" step={}", res(s.step, ResourceKind::Sound)));
            }
            if !s.ambient.is_empty() {
                out.push_str(&format!(" ambient={}", res(s.ambient, ResourceKind::Sound)));
            }
            out.push('\n');
        }
        let g = &self.give;
        if *g != Give::NONE {
            out.push_str(&format!(
                "give health={} health_max={} armor={} armor_max={} absorb={} ammo={} count={} weapon={} key={} expand={} sound={} message={}\n",
                g.health,
                g.health_max,
                g.armor,
                g.armor_max,
                g.armor_absorb,
                dash(g.ammo),
                g.ammo_count,
                dash(g.weapon),
                dash(g.key),
                u8::from(g.expand_ammo),
                res(g.sound, ResourceKind::Sound),
                quote(g.message)
            ));
        }
        if let Some(e) = self.explode {
            out.push_str(&format!(
                "explode radius={} damage={} sprite={} sound={}\n",
                e.radius,
                e.damage,
                res(e.sprite, ResourceKind::Sprite),
                res(e.sound, ResourceKind::Sound)
            ));
        }
        if let Some(p) = self.projectile {
            out.push_str(&format!(
                "projectile sprite={} launch={} fly={} hit={}\n",
                res(p.sprite, ResourceKind::Sprite),
                res(p.launch, ResourceKind::Sound),
                res(p.fly, ResourceKind::Sound),
                res(p.hit, ResourceKind::Sound)
            ));
        }
        out
    }

    /// Read a class back from its manifest lines (each already split off
    /// its `actor`/`attack`/`sound`/`give`/`explode` tag: pass the tag and
    /// the rest). Returns `None` until an `actor` line has been seen.
    pub fn from_manifest(lines: &[(&str, &str)]) -> Option<ActorDef> {
        let mut def: Option<ActorDef> = None;
        for (tag, rest) in lines {
            let kv = parse_kv(rest);
            let get = |k: &str| kv.iter().find(|(key, _)| *key == k).map(|(_, v)| v.as_str());
            let num = |k: &str, d: f32| get(k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(d);
            let text = |k: &str| get(k).filter(|v| *v != "-").map(intern).unwrap_or("");
            match *tag {
                "actor" => {
                    def = Some(ActorDef {
                        role: get("role").and_then(ActorRole::parse).unwrap_or(ActorRole::Prop),
                        health: num("health", 0.0),
                        pain_chance: num("pain", 0.0),
                        speed: num("speed", 0.0),
                        radius: num("radius", 0.0),
                        height: num("height", 0.0),
                        sight_range: num("sight", 0.0),
                        passable: num("passable", 0.0) != 0.0,
                        ..ActorDef::PROP
                    });
                }
                "attack" => {
                    let d = def.as_mut()?;
                    d.attack = Some(ActorAttack {
                        kind: get("kind").and_then(AttackKind::parse).unwrap_or(AttackKind::Melee),
                        damage: num("damage", 0.0),
                        rate: num("rate", 1.0),
                        range: num("range", 0.0),
                        speed: num("speed", 0.0),
                        spread: num("spread", 0.0),
                    });
                }
                "sound" => {
                    let d = def.as_mut()?;
                    d.sounds = ActorSounds {
                        sight: text("sight"),
                        pain: text("pain"),
                        death: text("death"),
                        attack: text("attack"),
                        active: text("active"),
                        gib: text("gib"),
                        step: text("step"),
                        ambient: text("ambient"),
                    };
                }
                "projectile" => {
                    let d = def.as_mut()?;
                    d.projectile = Some(ProjectileDef {
                        sprite: text("sprite"),
                        launch: text("launch"),
                        fly: text("fly"),
                        hit: text("hit"),
                    });
                }
                "give" => {
                    let d = def.as_mut()?;
                    d.give = Give {
                        health: num("health", 0.0),
                        health_max: num("health_max", 0.0),
                        armor: num("armor", 0.0),
                        armor_max: num("armor_max", 0.0),
                        armor_absorb: num("absorb", 0.0),
                        ammo: text("ammo"),
                        ammo_count: num("count", 0.0) as i32,
                        weapon: text("weapon"),
                        key: text("key"),
                        expand_ammo: num("expand", 0.0) != 0.0,
                        message: text("message"),
                        sound: text("sound"),
                    };
                }
                "explode" => {
                    let d = def.as_mut()?;
                    d.explode = Some(Explosion {
                        radius: num("radius", 0.0),
                        damage: num("damage", 0.0),
                        sprite: text("sprite"),
                        sound: text("sound"),
                    });
                }
                _ => {}
            }
        }
        def
    }
}

/// What a manifest resource reference names, for the writer's key mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    Sound,
    Sprite,
}

fn dash(s: &str) -> &str {
    if s.is_empty() { "-" } else { s }
}

fn quote(s: &str) -> String {
    if s.is_empty() {
        return "-".into();
    }
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `key=value` tokens, values optionally double-quoted (with `\"` and
/// `\\` escapes) so a pickup message keeps its spaces.
pub fn parse_kv(rest: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let chars: Vec<char> = rest.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let mut key = String::new();
        while i < chars.len() && chars[i] != '=' && !chars[i].is_whitespace() {
            key.push(chars[i]);
            i += 1;
        }
        let mut value = String::new();
        if i < chars.len() && chars[i] == '=' {
            i += 1;
            if i < chars.len() && chars[i] == '"' {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                    }
                    value.push(chars[i]);
                    i += 1;
                }
                i += 1;
            } else {
                while i < chars.len() && !chars[i].is_whitespace() {
                    value.push(chars[i]);
                    i += 1;
                }
            }
        }
        if !key.is_empty() {
            out.push((key, value));
        }
    }
    out
}

/// A manifest-read string as the `&'static str` the def tables are written
/// in. Interned: the vocabulary is a few hundred resource keys and pickup
/// lines per pack, and every load of the same pack hands back the same
/// pointers rather than a fresh leak.
pub fn intern(s: &str) -> &'static str {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static POOL: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let pool = POOL.get_or_init(|| Mutex::new(HashSet::new()));
    let mut pool = pool.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(have) = pool.get(s) {
        return have;
    }
    let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
    pool.insert(leaked);
    leaked
}

/// How a held weapon turns input into fire events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WeaponTrigger {
    /// One fire event for each press edge.
    #[default]
    Semi,
    /// Repeat at [`WeaponDef::rate`] while held.
    Auto,
    /// Accumulate while held and emit one scaled fire event on release.
    Charge,
}

impl WeaponTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Semi => "semi",
            Self::Auto => "auto",
            Self::Charge => "charge",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "semi" => Self::Semi,
            "auto" => Self::Auto,
            "charge" => Self::Charge,
            _ => return None,
        })
    }
}

/// The single delivery resolver a held weapon selects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WeaponDelivery {
    Melee,
    #[default]
    Hitscan,
    Projectile,
    Beam,
}

impl WeaponDelivery {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Melee => "melee",
            Self::Hitscan => "hitscan",
            Self::Projectile => "projectile",
            Self::Beam => "beam",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "melee" => Self::Melee,
            "hitscan" => Self::Hitscan,
            "projectile" => Self::Projectile,
            "beam" => Self::Beam,
            _ => return None,
        })
    }
}

/// Surface mark authored by a weapon manifest. This is semantic content data,
/// not a source-game name: any importer or generated weapon can request the
/// same engine-neutral procedural family.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WeaponMark {
    None,
    #[default]
    Bullet,
    Pellet,
    Scorch,
    Energy,
}

impl WeaponMark {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Bullet => "bullet",
            Self::Pellet => "pellet",
            Self::Scorch => "scorch",
            Self::Energy => "energy",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "none" => Self::None,
            "bullet" => Self::Bullet,
            "pellet" => Self::Pellet,
            "scorch" => Self::Scorch,
            "energy" => Self::Energy,
            _ => return None,
        })
    }
}

const DEFAULT_WEAPON_COLOR: [f32; 4] = [1.0, 0.015, 0.005, 0.82];

fn parse_weapon_color(text: &str) -> Option<[f32; 4]> {
    let values = text
        .split(',')
        .map(str::parse::<f32>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    Some(match values.as_slice() {
        [r, g, b] => [*r, *g, *b, 1.0],
        [r, g, b, a] => [*r, *g, *b, *a],
        _ => return None,
    })
}

/// A weapon the PLAYER can hold. This is the one importer/runtime contract:
/// source-family knowledge ends when these fields are written to the weapon
/// asset's manifest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeaponDef {
    pub id: &'static str,
    pub title: &'static str,
    pub trigger: WeaponTrigger,
    pub delivery: WeaponDelivery,
    /// Inclusive damage roll per landed hit or pellet.
    pub damage_min: f32,
    pub damage_max: f32,
    /// Rays per hitscan fire event. Other deliveries ignore it.
    pub pellets: u32,
    pub rate: f32,
    pub range: f32,
    /// Full spread angle in degrees. The engine converts it at its boundary.
    pub spread: f32,
    /// Projectile velocity in metres/second and maximum lifetime in seconds.
    pub projectile_speed: f32,
    pub projectile_life: f32,
    /// Optional impact burst, with linear falloff to the radius edge.
    pub splash_radius: f32,
    pub splash_damage: f32,
    /// Procedural resident surface mark; pellets still emit one per ray.
    pub mark: WeaponMark,
    /// Tracer/projectile tint, also used by `Energy` burn marks.
    pub color: [f32; 4],
    /// Seconds at which a charge weapon reaches `damage_max`.
    pub charge_max: f32,
    /// Ammo pool drawn from; empty means the weapon never runs dry.
    pub ammo: &'static str,
    /// Rounds spent per trigger pull.
    pub ammo_per_shot: i32,
    pub fire_sound: &'static str,
    /// Played when the trigger is pulled with the pool empty.
    pub empty_sound: &'static str,
    pub raise_sound: &'static str,
    pub idle_sound: &'static str,
    /// Billboard stem for the first-person view model, if the pack has one.
    pub view_sprite: &'static str,
    pub flash_sprite: &'static str,
    pub world_sprite: &'static str,
    pub projectile_sprite: &'static str,
    pub impact_sprite: &'static str,
    pub impact_sound: &'static str,
    pub puff_sprite: &'static str,
    pub blood_sprite: &'static str,
    /// Any visual slot may be supplied as a mesh alias instead of a sprite.
    pub view_model: &'static str,
    pub flash_model: &'static str,
    pub world_model: &'static str,
    pub projectile_model: &'static str,
    pub impact_model: &'static str,
    pub puff_model: &'static str,
    pub blood_model: &'static str,
    /// Slot number for a weapon-select HUD and for `1..9` switching.
    pub slot: u8,
}

impl WeaponDef {
    /// One manifest line, carried on the weapon's own view-sprite asset.
    ///
    /// ```text
    /// weapon id=pistol title=PISTOL slot=2 trigger=semi delivery=hitscan damage=5-15 pellets=1 spread=0 rate=1.9 range=100 projectile_speed=0 projectile_life=0 splash_radius=0 splash_damage=0 mark=bullet color=1,0.015,0.005,0.82 charge_max=0 ammo=bullet per_shot=1 fire=sfx/doom1/dspistol empty=sfx/doom1/dsnoway raise=- idle=- view=billboards/doom1/pisg
    /// ```
    pub fn to_manifest(&self, resource: &dyn Fn(&str, ResourceKind) -> String) -> String {
        // Variant slots (`pain1|pain2`) map take by take: each variant is
        // its own resource.
        let res = |s: &str, k: ResourceKind| {
            if s.is_empty() {
                return "-".to_string();
            }
            s.split('|')
                .map(|v| resource(v, k))
                .collect::<Vec<_>>()
                .join("|")
        };
        let mut out = format!(
            "weapon id={} title={} slot={} trigger={} delivery={} damage={}-{} pellets={} spread={} rate={} range={} projectile_speed={} projectile_life={} splash_radius={} splash_damage={} mark={} color={},{},{},{} charge_max={} ammo={} per_shot={} fire={} empty={} raise={} idle={} view={} flash={} world={} projectile_sprite={} impact={} impact_sound={} puff={} blood={}",
            self.id,
            quote(self.title),
            self.slot,
            self.trigger.as_str(),
            self.delivery.as_str(),
            self.damage_min,
            self.damage_max,
            self.pellets,
            self.spread,
            self.rate,
            self.range,
            self.projectile_speed,
            self.projectile_life,
            self.splash_radius,
            self.splash_damage,
            self.mark.as_str(),
            self.color[0],
            self.color[1],
            self.color[2],
            self.color[3],
            self.charge_max,
            dash(self.ammo),
            self.ammo_per_shot,
            res(self.fire_sound, ResourceKind::Sound),
            res(self.empty_sound, ResourceKind::Sound),
            res(self.raise_sound, ResourceKind::Sound),
            res(self.idle_sound, ResourceKind::Sound),
            res(self.view_sprite, ResourceKind::Sprite),
            res(self.flash_sprite, ResourceKind::Sprite),
            res(self.world_sprite, ResourceKind::Sprite),
            res(self.projectile_sprite, ResourceKind::Sprite),
            res(self.impact_sprite, ResourceKind::Sprite),
            res(self.impact_sound, ResourceKind::Sound),
            res(self.puff_sprite, ResourceKind::Sprite),
            res(self.blood_sprite, ResourceKind::Sprite),
        );
        for (name, value) in [
            ("view_model", self.view_model),
            ("flash_model", self.flash_model),
            ("world_model", self.world_model),
            ("projectile_model", self.projectile_model),
            ("impact_model", self.impact_model),
            ("puff_model", self.puff_model),
            ("blood_model", self.blood_model),
        ] {
            if !value.is_empty() {
                out.push_str(&format!(" {name}={value}"));
            }
        }
        out.push('\n');
        out
    }

    pub fn from_manifest(rest: &str) -> Option<WeaponDef> {
        let kv = parse_kv(rest);
        let get = |k: &str| kv.iter().find(|(key, _)| *key == k).map(|(_, v)| v.as_str());
        let num = |k: &str, d: f32| get(k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(d);
        let text = |k: &str| get(k).filter(|v| *v != "-").map(intern).unwrap_or("");
        let id = text("id");
        if id.is_empty() {
            return None;
        }
        let legacy_kind = get("kind").and_then(AttackKind::parse);
        let delivery = get("delivery")
            .and_then(WeaponDelivery::parse)
            .or_else(|| legacy_kind.map(|kind| match kind {
                AttackKind::Melee => WeaponDelivery::Melee,
                AttackKind::Hitscan => WeaponDelivery::Hitscan,
                AttackKind::Projectile => WeaponDelivery::Projectile,
            }))
            .unwrap_or_default();
        let trigger = get("trigger")
            .and_then(WeaponTrigger::parse)
            .unwrap_or_else(|| if num("auto", 0.0) != 0.0 { WeaponTrigger::Auto } else { WeaponTrigger::Semi });
        let damage = get("damage").unwrap_or("0");
        let (damage_min, damage_max) = damage
            .split_once('-')
            .and_then(|(lo, hi)| Some((lo.parse::<f32>().ok()?, hi.parse::<f32>().ok()?)))
            .unwrap_or_else(|| {
                let n = damage.parse::<f32>().unwrap_or(0.0);
                (n, n)
            });
        let pellets = num("pellets", 1.0).max(1.0) as u32;
        let splash_radius = num("splash_radius", 0.0);
        let splash_damage = num("splash_damage", 0.0);
        let mark = get("mark")
            .and_then(WeaponMark::parse)
            .unwrap_or_else(|| match delivery {
                WeaponDelivery::Melee => WeaponMark::None,
                WeaponDelivery::Hitscan if pellets > 1 => WeaponMark::Pellet,
                WeaponDelivery::Hitscan => WeaponMark::Bullet,
                WeaponDelivery::Projectile if splash_radius > 0.0 && splash_damage > 0.0 => {
                    WeaponMark::Scorch
                }
                WeaponDelivery::Projectile | WeaponDelivery::Beam => WeaponMark::Energy,
            });
        let color = get("color")
            .and_then(parse_weapon_color)
            .unwrap_or(DEFAULT_WEAPON_COLOR);
        Some(WeaponDef {
            id,
            title: text("title"),
            trigger,
            delivery,
            damage_min,
            damage_max: damage_max.max(damage_min),
            pellets,
            rate: num("rate", 1.0),
            range: num("range", 0.0),
            spread: num("spread", 0.0),
            projectile_speed: num("projectile_speed", num("speed", 0.0)),
            projectile_life: num("projectile_life", 0.0),
            splash_radius,
            splash_damage,
            mark,
            color,
            charge_max: num("charge_max", 0.0),
            ammo: text("ammo"),
            ammo_per_shot: num("per_shot", 0.0) as i32,
            fire_sound: text("fire"),
            empty_sound: text("empty"),
            raise_sound: text("raise"),
            idle_sound: text("idle"),
            view_sprite: if get("view").is_some() { text("view") } else { text("sprite") },
            flash_sprite: text("flash"),
            world_sprite: text("world"),
            projectile_sprite: text("projectile_sprite"),
            impact_sprite: text("impact"),
            impact_sound: text("impact_sound"),
            puff_sprite: text("puff"),
            blood_sprite: text("blood"),
            view_model: text("view_model"),
            flash_model: text("flash_model"),
            world_model: text("world_model"),
            projectile_model: text("projectile_model"),
            impact_model: text("impact_model"),
            puff_model: text("puff_model"),
            blood_model: text("blood_model"),
            slot: num("slot", 0.0) as u8,
        })
    }
}

/// An ammo pool: the ceiling a pickup clamps to, doubled by a backpack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AmmoPool {
    pub id: &'static str,
    pub title: &'static str,
    pub max: i32,
    /// What the player starts a level with.
    pub start: i32,
}

/// Metres a creature notices you from. The source formats express sight as
/// "anywhere in the same line of sight"; a finite number is the engine's
/// requirement, not the format's, so one honest default beats a per-class
/// guess.
const DEFAULT_SIGHT: f32 = 48.0;

// ---------------------------------------------------------------------------
// Doom family
// ---------------------------------------------------------------------------

/// Doom monsters move a fixed step per *state*, not per tic, so a raw
/// `mobjinfo.speed` is not a velocity. The conversion is
/// `speed_units * 35 tics/s / state_tics * 1/32 m`, with the walk states
/// of the class in question (4 tics for most, 2 for the fast ones). The
/// results are written literally below so the arithmetic is auditable:
/// speed 8 over 4-tic states is 2.19 m/s, speed 10 over 2-tic states is
/// 5.47 m/s.
const SLOW: f32 = 2.19;
const BRISK: f32 = 3.28;
const FAST: f32 = 4.38;
const CHARGING: f32 = 5.47;

/// Doom `painchance` is out of 256.
const fn pain(chance: u32) -> f32 {
    chance as f32 / 256.0
}

/// Doom damage rolls are `(random % n + 1) * m`; the expected value is
/// `m * (n + 1) / 2`. Spelled out rather than tabulated as a maximum.
const fn roll(n: u32, m: u32) -> f32 {
    (m * (n + 1)) as f32 / 2.0
}

/// Doom/Freedoom THING type -> behaviour. `None` for a class this table does
/// not describe, which the caller must treat as "generic prop".
///
/// Hit points, pain chance and the damage rolls are the source format's own
/// `mobjinfo`/`p_enemy` numbers. Speeds and ranges are converted to engine
/// units by the constants above.
pub fn doom_actor_def(class: u16) -> Option<ActorDef> {
    let d = match class {
        // ---- monsters -------------------------------------------------
        // Zombieman: a pistol at range.
        3004 => ActorDef::monster(20.0, pain(200), SLOW)
            .with_body(0.625, 1.75)
            .with_attack(ActorAttack {
                kind: AttackKind::Hitscan,
                damage: roll(5, 3),
                rate: 1.0,
                range: 64.0,
                speed: 0.0,
                spread: 0.09,
            })
            .with_sounds(ActorSounds {
                sight: "dsposit1",
                pain: "dspopain",
                death: "dspodth1",
                attack: "dspistol",
                active: "dsposact",
                            ..ActorSounds::NONE
            }),
        // Shotgun guy: three pellets at once.
        9 => ActorDef::monster(30.0, pain(170), SLOW)
            .with_body(0.625, 1.75)
            .with_attack(ActorAttack {
                kind: AttackKind::Hitscan,
                damage: roll(5, 3) * 3.0,
                rate: 0.8,
                range: 64.0,
                speed: 0.0,
                spread: 0.12,
            })
            .with_sounds(ActorSounds {
                sight: "dsposit2",
                pain: "dspopain",
                death: "dspodth2",
                attack: "dsshotgn",
                active: "dsposact",
                            ..ActorSounds::NONE
            }),
        // Chaingunner: the same bullet, far more of them.
        65 => ActorDef::monster(70.0, pain(170), SLOW)
            .with_body(0.625, 1.75)
            .with_attack(ActorAttack {
                kind: AttackKind::Hitscan,
                damage: roll(5, 3),
                rate: 4.0,
                range: 64.0,
                speed: 0.0,
                spread: 0.09,
            })
            .with_sounds(ActorSounds {
                sight: "dsposit2",
                pain: "dspopain",
                death: "dspodth2",
                attack: "dsshotgn",
                active: "dsposact",
                            ..ActorSounds::NONE
            }),
        // Wolfenstein SS.
        84 => ActorDef::monster(50.0, pain(170), SLOW)
            .with_body(0.625, 1.75)
            .with_attack(ActorAttack {
                kind: AttackKind::Hitscan,
                damage: roll(5, 3),
                rate: 2.0,
                range: 64.0,
                speed: 0.0,
                spread: 0.1,
            })
            .with_sounds(ActorSounds {
                sight: "dsssit",
                pain: "dspopain",
                death: "dsssdth",
                attack: "dsshotgn",
                active: "dsposact",
                            ..ActorSounds::NONE
            }),
        // Imp: claws close, a fireball far.
        3001 => ActorDef::monster(60.0, pain(200), SLOW)
            .with_body(0.625, 1.75)
            .with_attack(ActorAttack {
                kind: AttackKind::Projectile,
                damage: roll(8, 3),
                rate: 0.7,
                range: 64.0,
                speed: 10.0,
                spread: 0.0,
            })
            .with_projectile(ProjectileDef {
                sprite: "bal1",
                launch: "dsfirsht",
                fly: "",
                hit: "dsfirxpl",
            })
            .with_sounds(ActorSounds {
                sight: "dsbgsit1",
                pain: "dspopain",
                death: "dsbgdth1",
                attack: "dsfirsht",
                active: "dsbgact",
                            ..ActorSounds::NONE
            }),
        // Demon / Spectre: no ranged attack, and it runs.
        3002 | 58 => ActorDef::monster(150.0, pain(180), CHARGING)
            .with_body(0.94, 1.75)
            .with_attack(ActorAttack {
                kind: AttackKind::Melee,
                damage: roll(10, 4),
                rate: 1.0,
                range: 1.6,
                speed: 0.0,
                spread: 0.0,
            })
            .with_sounds(ActorSounds {
                sight: "dssgtsit",
                pain: "dsdmpain",
                death: "dssgtdth",
                attack: "dssgtatk",
                active: "dsdmact",
                            ..ActorSounds::NONE
            }),
        // Lost Soul: always flinches, always charging.
        3006 => ActorDef::monster(100.0, pain(256), CHARGING)
            .with_body(0.5, 1.75)
            .with_attack(ActorAttack {
                kind: AttackKind::Melee,
                damage: roll(8, 3),
                rate: 1.2,
                range: 1.4,
                speed: 0.0,
                spread: 0.0,
            })
            .with_sounds(ActorSounds {
                sight: "",
                pain: "dsdmpain",
                death: "dsfirxpl",
                attack: "dssklatk",
                active: "dsdmact",
                            ..ActorSounds::NONE
            }),
        // Cacodemon.
        3005 => ActorDef::monster(400.0, pain(128), SLOW)
            .with_body(0.97, 1.75)
            .with_attack(ActorAttack {
                kind: AttackKind::Projectile,
                damage: roll(6, 5),
                rate: 0.7,
                range: 80.0,
                speed: 10.0,
                spread: 0.0,
            })
            .with_projectile(ProjectileDef {
                sprite: "bal2",
                launch: "dsfirsht",
                fly: "",
                hit: "dsfirxpl",
            })
            .with_sounds(ActorSounds {
                sight: "dscacsit",
                pain: "dsdmpain",
                death: "dscacdth",
                attack: "dsfirsht",
                active: "dsdmact",
                            ..ActorSounds::NONE
            }),
        // Baron of Hell.
        3003 => ActorDef::monster(1000.0, pain(50), SLOW)
            .with_body(0.75, 2.0)
            .with_attack(ActorAttack {
                kind: AttackKind::Projectile,
                damage: roll(8, 8),
                rate: 0.6,
                range: 80.0,
                speed: 15.0,
                spread: 0.0,
            })
            .with_projectile(ProjectileDef {
                sprite: "bal7",
                launch: "dsfirsht",
                fly: "",
                hit: "dsfirxpl",
            })
            .with_sounds(ActorSounds {
                sight: "dsbrssit",
                pain: "dsdmpain",
                death: "dsbrsdth",
                attack: "dsfirsht",
                active: "dsdmact",
                            ..ActorSounds::NONE
            }),
        // Hell Knight: the Baron at half strength.
        69 => ActorDef::monster(500.0, pain(50), SLOW)
            .with_body(0.75, 2.0)
            .with_attack(ActorAttack {
                kind: AttackKind::Projectile,
                damage: roll(8, 8),
                rate: 0.6,
                range: 80.0,
                speed: 15.0,
                spread: 0.0,
            })
            .with_projectile(ProjectileDef {
                sprite: "bal7",
                launch: "dsfirsht",
                fly: "",
                hit: "dsfirxpl",
            })
            .with_sounds(ActorSounds {
                sight: "dskntsit",
                pain: "dsdmpain",
                death: "dskntdth",
                attack: "dsfirsht",
                active: "dsdmact",
                            ..ActorSounds::NONE
            }),
        // Cyberdemon: no pain sound at all, and it barely flinches.
        16 => ActorDef::monster(4000.0, pain(20), BRISK)
            .with_body(1.25, 3.44)
            .with_attack(ActorAttack {
                kind: AttackKind::Projectile,
                damage: 40.0,
                rate: 0.8,
                range: 100.0,
                speed: 20.0,
                spread: 0.0,
            })
            .with_sounds(ActorSounds {
                sight: "dscybsit",
                pain: "",
                death: "dscybdth",
                attack: "dsrlaunc",
                active: "dsdmact",
                            ..ActorSounds::NONE
            }),
        // Arachnotron.
        68 => ActorDef::monster(500.0, pain(128), BRISK)
            .with_body(2.0, 2.0)
            .with_attack(ActorAttack {
                kind: AttackKind::Projectile,
                damage: roll(8, 5),
                rate: 3.0,
                range: 80.0,
                speed: 25.0,
                spread: 0.05,
            })
            .with_sounds(ActorSounds {
                sight: "dsbspsit",
                pain: "dsdmpain",
                death: "dsbspdth",
                attack: "dsplasma",
                active: "dsbspact",
                            ..ActorSounds::NONE
            }),
        // Arch-vile.
        64 => ActorDef::monster(700.0, pain(10), FAST)
            .with_body(0.625, 1.75)
            .with_attack(ActorAttack {
                kind: AttackKind::Hitscan,
                damage: 40.0,
                rate: 0.5,
                range: 40.0,
                speed: 0.0,
                spread: 0.0,
            })
            .with_sounds(ActorSounds {
                sight: "dsvilsit",
                pain: "dsvipain",
                death: "dsvildth",
                attack: "dsflamst",
                active: "dsvilact",
                            ..ActorSounds::NONE
            }),
        // Revenant.
        66 => ActorDef::monster(300.0, pain(100), CHARGING)
            .with_body(0.625, 1.75)
            .with_attack(ActorAttack {
                kind: AttackKind::Projectile,
                damage: 10.0,
                rate: 0.8,
                range: 80.0,
                speed: 20.0,
                spread: 0.0,
            })
            .with_sounds(ActorSounds {
                sight: "dsskesit",
                pain: "dspopain",
                death: "dsskedth",
                attack: "dsskeatk",
                active: "dsskeact",
                            ..ActorSounds::NONE
            }),
        // Mancubus.
        67 => ActorDef::monster(600.0, pain(80), SLOW)
            .with_body(1.875, 2.0)
            .with_attack(ActorAttack {
                kind: AttackKind::Projectile,
                damage: roll(8, 8),
                rate: 0.6,
                range: 80.0,
                speed: 15.0,
                spread: 0.14,
            })
            .with_sounds(ActorSounds {
                sight: "dsmansit",
                pain: "dsmnpain",
                death: "dsmandth",
                attack: "dsfirsht",
                active: "dsposact",
                            ..ActorSounds::NONE
            }),
        // Pain Elemental.
        71 => ActorDef::monster(400.0, pain(128), SLOW)
            .with_body(0.97, 1.75)
            .with_attack(ActorAttack {
                kind: AttackKind::Projectile,
                damage: roll(8, 3),
                rate: 0.5,
                range: 80.0,
                speed: 10.0,
                spread: 0.0,
            })
            .with_sounds(ActorSounds {
                sight: "dspesit",
                pain: "dspepain",
                death: "dspedth",
                attack: "dssklatk",
                active: "dsdmact",
                            ..ActorSounds::NONE
            }),
        // Commander Keen: hangs there and dies.
        72 => ActorDef {
            speed: 0.0,
            attack: None,
            ..ActorDef::monster(100.0, pain(256), 0.0)
                .with_body(0.5, 2.0)
                .with_sounds(ActorSounds {
                    sight: "",
                    pain: "dskeenpn",
                    death: "dskeendt",
                    attack: "",
                    active: "",
                                    ..ActorSounds::NONE
                })
        },

        // ---- health and armour ----------------------------------------
        2011 => ActorDef::item(Give {
            health: 10.0,
            message: "Stimpack",
            ..Give::NONE
        }),
        2012 => ActorDef::item(Give {
            health: 25.0,
            message: "Medikit",
            ..Give::NONE
        }),
        2014 => ActorDef::item(Give {
            health: 1.0,
            health_max: 200.0,
            message: "Health bonus",
            ..Give::NONE
        }),
        2015 => ActorDef::item(Give {
            armor: 1.0,
            armor_max: 200.0,
            message: "Armor bonus",
            ..Give::NONE
        }),
        2018 => ActorDef::item(Give {
            armor: 100.0,
            armor_max: 100.0,
            armor_absorb: 1.0 / 3.0,
            message: "Armor",
            ..Give::NONE
        }),
        2019 => ActorDef::item(Give {
            armor: 200.0,
            armor_max: 200.0,
            armor_absorb: 0.5,
            message: "Megaarmor",
            ..Give::NONE
        }),
        // Soulsphere and megasphere break the 100 ceiling.
        2013 => ActorDef::item(Give {
            health: 100.0,
            health_max: 200.0,
            message: "Supercharge",
            sound: "dsgetpow",
            ..Give::NONE
        }),
        83 => ActorDef::item(Give {
            health: 200.0,
            health_max: 200.0,
            armor: 200.0,
            armor_max: 200.0,
            armor_absorb: 0.5,
            message: "MegaSphere",
            sound: "dsgetpow",
            ..Give::NONE
        }),
        2024 => ActorDef::item(Give {
            message: "Partial invisibility",
            sound: "dsgetpow",
            ..Give::NONE
        }),
        2022 => ActorDef::item(Give {
            message: "Invulnerability",
            sound: "dsgetpow",
            ..Give::NONE
        }),
        2025 => ActorDef::item(Give {
            message: "Radiation shielding suit",
            sound: "dsgetpow",
            ..Give::NONE
        }),
        2026 => ActorDef::item(Give {
            message: "Computer area map",
            sound: "dsgetpow",
            ..Give::NONE
        }),
        2045 => ActorDef::item(Give {
            message: "Light amplification visor",
            sound: "dsgetpow",
            ..Give::NONE
        }),

        // ---- ammunition ------------------------------------------------
        2007 => ActorDef::item(Give {
            ammo: "bullet",
            ammo_count: 10,
            message: "Clip",
            ..Give::NONE
        }),
        2048 => ActorDef::item(Give {
            ammo: "bullet",
            ammo_count: 50,
            message: "Box of bullets",
            ..Give::NONE
        }),
        2008 => ActorDef::item(Give {
            ammo: "shell",
            ammo_count: 4,
            message: "Shotgun shells",
            ..Give::NONE
        }),
        2049 => ActorDef::item(Give {
            ammo: "shell",
            ammo_count: 20,
            message: "Box of shells",
            ..Give::NONE
        }),
        2010 => ActorDef::item(Give {
            ammo: "rocket",
            ammo_count: 1,
            message: "Rocket",
            ..Give::NONE
        }),
        2046 => ActorDef::item(Give {
            ammo: "rocket",
            ammo_count: 5,
            message: "Box of rockets",
            ..Give::NONE
        }),
        2047 => ActorDef::item(Give {
            ammo: "cell",
            ammo_count: 20,
            message: "Energy cell",
            ..Give::NONE
        }),
        17 => ActorDef::item(Give {
            ammo: "cell",
            ammo_count: 100,
            message: "Energy cell pack",
            ..Give::NONE
        }),
        8 => ActorDef::item(Give {
            expand_ammo: true,
            ammo: "bullet",
            ammo_count: 10,
            message: "Backpack",
            ..Give::NONE
        }),

        // ---- keys -------------------------------------------------------
        5 => ActorDef::item(Give {
            key: "blue",
            message: "Blue keycard",
            ..Give::NONE
        }),
        6 => ActorDef::item(Give {
            key: "yellow",
            message: "Yellow keycard",
            ..Give::NONE
        }),
        13 => ActorDef::item(Give {
            key: "red",
            message: "Red keycard",
            ..Give::NONE
        }),
        40 => ActorDef::item(Give {
            key: "blue",
            message: "Blue skull key",
            ..Give::NONE
        }),
        39 => ActorDef::item(Give {
            key: "yellow",
            message: "Yellow skull key",
            ..Give::NONE
        }),
        38 => ActorDef::item(Give {
            key: "red",
            message: "Red skull key",
            ..Give::NONE
        }),

        // ---- weapons on the floor --------------------------------------
        2001 => ActorDef::weapon_pickup(Give {
            weapon: "shotgun",
            ammo: "shell",
            ammo_count: 8,
            message: "Shotgun",
            sound: "dswpnup",
            ..Give::NONE
        }),
        82 => ActorDef::weapon_pickup(Give {
            weapon: "supershotgun",
            ammo: "shell",
            ammo_count: 8,
            message: "Super shotgun",
            sound: "dswpnup",
            ..Give::NONE
        }),
        2002 => ActorDef::weapon_pickup(Give {
            weapon: "chaingun",
            ammo: "bullet",
            ammo_count: 20,
            message: "Chaingun",
            sound: "dswpnup",
            ..Give::NONE
        }),
        2003 => ActorDef::weapon_pickup(Give {
            weapon: "rocket",
            ammo: "rocket",
            ammo_count: 2,
            message: "Rocket launcher",
            sound: "dswpnup",
            ..Give::NONE
        }),
        2004 => ActorDef::weapon_pickup(Give {
            weapon: "plasma",
            ammo: "cell",
            ammo_count: 40,
            message: "Plasma gun",
            sound: "dswpnup",
            ..Give::NONE
        }),
        2005 => ActorDef::weapon_pickup(Give {
            weapon: "chainsaw",
            message: "Chainsaw",
            sound: "dswpnup",
            ..Give::NONE
        }),
        2006 => ActorDef::weapon_pickup(Give {
            weapon: "bfg",
            ammo: "cell",
            ammo_count: 40,
            message: "BFG9000",
            sound: "dswpnup",
            ..Give::NONE
        }),

        // ---- scenery -----------------------------------------------------
        // An exploding barrel is a body with hit points, not decor.
        // Its burst is the source's `A_Explode`: 128 damage falling off to
        // nothing at 128 units (4 m in the table's metres), drawn by the
        // BEXP sprite in the drum's place, with the explosion's own sound.
        2035 => ActorDef {
            role: ActorRole::Monster,
            health: 20.0,
            pain_chance: 0.0,
            speed: 0.0,
            sight_range: 0.0,
            attack: None,
            sounds: ActorSounds::NONE,
            explode: Some(Explosion {
                radius: 4.0,
                damage: 128.0,
                sprite: "bexp",
                sound: "dsbarexp",
            }),
            ..ActorDef::PROP.with_body(0.5, 1.0)
        },
        2028 => ActorDef::PROP.with_body(0.5, 1.5),
        _ => return None,
    };
    Some(d)
}

const EMPTY_WEAPON: WeaponDef = WeaponDef {
    id: "",
    title: "",
    trigger: WeaponTrigger::Semi,
    delivery: WeaponDelivery::Hitscan,
    damage_min: 0.0,
    damage_max: 0.0,
    pellets: 1,
    rate: 1.0,
    range: 100.0,
    spread: 0.0,
    projectile_speed: 0.0,
    projectile_life: 0.0,
    splash_radius: 0.0,
    splash_damage: 0.0,
    mark: WeaponMark::Bullet,
    color: DEFAULT_WEAPON_COLOR,
    charge_max: 0.0,
    ammo: "",
    ammo_per_shot: 0,
    fire_sound: "",
    empty_sound: "",
    raise_sound: "",
    idle_sound: "",
    view_sprite: "",
    flash_sprite: "",
    world_sprite: "",
    projectile_sprite: "",
    impact_sprite: "",
    impact_sound: "",
    puff_sprite: "",
    blood_sprite: "",
    view_model: "",
    flash_model: "",
    world_model: "",
    projectile_model: "",
    impact_model: "",
    puff_model: "",
    blood_model: "",
    slot: 0,
};

/// The weapons this family's player can hold. Damage remains a dice range
/// per landed pellet/hit; pellet count and delivery stay explicit all the way
/// into the engine instead of being collapsed to an average ray here.
pub fn doom_weapon_def(id: &str) -> Option<WeaponDef> {
    let w = match id {
        "fist" => WeaponDef {
            id: "fist",
            title: "Fist",
            trigger: WeaponTrigger::Semi,
            delivery: WeaponDelivery::Melee,
            damage_min: 2.0,
            damage_max: 20.0,
            rate: 1.9,
            range: 1.6,
            fire_sound: "dspunch",
            view_sprite: "pung",
            blood_sprite: "blud",
            mark: WeaponMark::None,
            slot: 1,
            ..EMPTY_WEAPON
        },
        "chainsaw" => WeaponDef {
            id: "chainsaw",
            title: "Chainsaw",
            trigger: WeaponTrigger::Auto,
            delivery: WeaponDelivery::Melee,
            damage_min: 2.0,
            damage_max: 20.0,
            rate: 8.75,
            range: 1.8,
            fire_sound: "dssawful",
            raise_sound: "dssawup",
            idle_sound: "dssawidl",
            view_sprite: "sawg",
            world_sprite: "csaw",
            blood_sprite: "blud",
            mark: WeaponMark::None,
            slot: 1,
            ..EMPTY_WEAPON
        },
        "pistol" => WeaponDef {
            id: "pistol",
            title: "Pistol",
            trigger: WeaponTrigger::Semi,
            damage_min: 5.0,
            damage_max: 15.0,
            rate: 1.9,
            ammo: "bullet",
            ammo_per_shot: 1,
            fire_sound: "dspistol",
            empty_sound: "dsnoway",
            view_sprite: "pisg",
            flash_sprite: "pisf",
            puff_sprite: "puff",
            blood_sprite: "blud",
            slot: 2,
            ..EMPTY_WEAPON
        },
        "shotgun" => WeaponDef {
            id: "shotgun",
            title: "Shotgun",
            trigger: WeaponTrigger::Semi,
            damage_min: 5.0,
            damage_max: 15.0,
            pellets: 7,
            rate: 1.05,
            spread: 5.6,
            ammo: "shell",
            ammo_per_shot: 1,
            fire_sound: "dsshotgn",
            empty_sound: "dsnoway",
            view_sprite: "shtg",
            flash_sprite: "shtf",
            world_sprite: "shot",
            puff_sprite: "puff",
            blood_sprite: "blud",
            mark: WeaponMark::Pellet,
            slot: 3,
            ..EMPTY_WEAPON
        },
        "supershotgun" => WeaponDef {
            id: "supershotgun",
            title: "Super shotgun",
            trigger: WeaponTrigger::Semi,
            damage_min: 5.0,
            damage_max: 15.0,
            pellets: 20,
            rate: 0.85,
            spread: 11.0,
            ammo: "shell",
            ammo_per_shot: 2,
            fire_sound: "dsdshtgn",
            empty_sound: "dsnoway",
            view_sprite: "sht2",
            flash_sprite: "sht2",
            world_sprite: "sgn2",
            puff_sprite: "puff",
            blood_sprite: "blud",
            mark: WeaponMark::Pellet,
            slot: 3,
            ..EMPTY_WEAPON
        },
        "chaingun" => WeaponDef {
            id: "chaingun",
            title: "Chaingun",
            trigger: WeaponTrigger::Auto,
            damage_min: 5.0,
            damage_max: 15.0,
            rate: 8.7,
            ammo: "bullet",
            ammo_per_shot: 1,
            fire_sound: "dspistol",
            empty_sound: "dsnoway",
            view_sprite: "chgg",
            flash_sprite: "chgf",
            world_sprite: "mgun",
            puff_sprite: "puff",
            blood_sprite: "blud",
            slot: 4,
            ..EMPTY_WEAPON
        },
        "rocket" => WeaponDef {
            id: "rocket",
            title: "Rocket launcher",
            trigger: WeaponTrigger::Auto,
            delivery: WeaponDelivery::Projectile,
            damage_min: 20.0,
            damage_max: 160.0,
            rate: 1.05,
            projectile_speed: 20.0,
            projectile_life: 6.0,
            splash_radius: 4.0,
            splash_damage: 128.0,
            ammo: "rocket",
            ammo_per_shot: 1,
            fire_sound: "dsrlaunc",
            empty_sound: "dsnoway",
            view_sprite: "misg",
            flash_sprite: "misf",
            world_sprite: "laun",
            projectile_sprite: "misl",
            impact_sprite: "misl-burst",
            impact_sound: "dsbarexp",
            mark: WeaponMark::Scorch,
            color: [1.0, 0.18, 0.015, 0.92],
            slot: 5,
            ..EMPTY_WEAPON
        },
        "plasma" => WeaponDef {
            id: "plasma",
            title: "Plasma rifle",
            trigger: WeaponTrigger::Auto,
            delivery: WeaponDelivery::Projectile,
            damage_min: 5.0,
            damage_max: 40.0,
            rate: 11.6,
            projectile_speed: 25.0,
            projectile_life: 5.0,
            ammo: "cell",
            ammo_per_shot: 1,
            fire_sound: "dsplasma",
            empty_sound: "dsnoway",
            view_sprite: "plsg",
            flash_sprite: "plsf",
            world_sprite: "plas",
            projectile_sprite: "plss",
            impact_sprite: "plse",
            mark: WeaponMark::Energy,
            color: [0.08, 0.35, 1.0, 0.92],
            slot: 6,
            ..EMPTY_WEAPON
        },
        "bfg" => WeaponDef {
            id: "bfg",
            title: "BFG9000",
            trigger: WeaponTrigger::Semi,
            delivery: WeaponDelivery::Projectile,
            damage_min: 100.0,
            damage_max: 800.0,
            rate: 0.5,
            projectile_speed: 25.0,
            projectile_life: 5.0,
            splash_radius: 6.0,
            splash_damage: 200.0,
            ammo: "cell",
            ammo_per_shot: 40,
            fire_sound: "dsbfg",
            empty_sound: "dsnoway",
            view_sprite: "bfgg",
            flash_sprite: "bfgf",
            world_sprite: "bfug",
            projectile_sprite: "bfs1",
            impact_sprite: "bfe1",
            mark: WeaponMark::Energy,
            color: [0.15, 1.0, 0.12, 0.95],
            slot: 7,
            ..EMPTY_WEAPON
        },
        _ => return None,
    };
    Some(w)
}

/// Ammo pools for the Doom family, in HUD order.
pub const DOOM_AMMO: &[AmmoPool] = &[
    AmmoPool { id: "bullet", title: "BULL", max: 200, start: 50 },
    AmmoPool { id: "shell", title: "SHEL", max: 50, start: 0 },
    AmmoPool { id: "rocket", title: "RCKT", max: 50, start: 0 },
    AmmoPool { id: "cell", title: "CELL", max: 300, start: 0 },
];

/// Doom's non-actor sounds, addressed by generic event name so the engine
/// never says "ds…" out loud.
pub fn doom_event_sound(event: &str) -> &'static str {
    match event {
        "player_pain" => "dsplpain",
        "player_death" => "dspldeth",
        "player_land" => "dsoof",
        "pickup_item" => "dsitemup",
        "pickup_weapon" => "dswpnup",
        "pickup_power" => "dsgetpow",
        "refused" => "dsnoway",
        "door_open" => "dsdoropn",
        "door_close" => "dsdorcls",
        "platform_start" => "dspstart",
        "platform_stop" => "dspstop",
        "switch" => "dsswtchn",
        "teleport" => "dstelept",
        "explosion" => "dsbarexp",
        // A body burst by overkill (`A_XScream`).
        "gib" => "dsslop",
        _ => "",
    }
}

// ---------------------------------------------------------------------------
// Family dispatch
// ---------------------------------------------------------------------------

/// Which behaviour table a pack's classes are written in. Derived from the
/// `.place` sidecar's `source`, so a pack the importer publishes under a new
/// name inherits the right table by naming, not by a second registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorFamily {
    Doom,
    Unknown,
}

/// Doom, Freedoom and any other WAD-derived pack share Doom's THING numbers,
/// which is exactly why the family is a property of the FORMAT rather than of
/// the pack's name.
pub fn family_of(source: &str) -> ActorFamily {
    let s = source.to_ascii_lowercase();
    if s.contains("doom") || s.contains("heretic") || s.contains("hexen") {
        // Heretic/Hexen reuse the record layout but not the numbers; they
        // resolve to Doom only for the classes that genuinely coincide, and
        // to `None` (generic prop) for the rest. That is the honest answer
        // until their own table exists.
        ActorFamily::Doom
    } else {
        ActorFamily::Unknown
    }
}

/// How tall this family's own PLAYER stands in the metres the behaviour
/// table is written in — the yardstick every linear number in the table
/// shares. A runtime whose map declares people of a different height scales
/// each def by `map_person_height / person_height(source)` (see
/// [`ActorDef::scaled`]) at the one place the table becomes bodies.
pub fn person_height(source: &str) -> f32 {
    match family_of(source) {
        // The source player is 56 units tall and the table's metres put
        // that at 1.75.
        ActorFamily::Doom => 1.75,
        ActorFamily::Unknown => 1.75,
    }
}

/// The behaviour of one placed class, or `None` when this family has no entry
/// for it. `class` is the `.place` row's `class=` field verbatim.
pub fn actor_def(source: &str, class: &str) -> Option<ActorDef> {
    match family_of(source) {
        ActorFamily::Doom => doom_actor_def(class.parse::<u16>().ok()?),
        ActorFamily::Unknown => None,
    }
}

pub fn weapon_def(source: &str, id: &str) -> Option<WeaponDef> {
    match family_of(source) {
        ActorFamily::Doom => doom_weapon_def(id),
        ActorFamily::Unknown => None,
    }
}

pub fn ammo_pools(source: &str) -> &'static [AmmoPool] {
    match family_of(source) {
        ActorFamily::Doom => DOOM_AMMO,
        ActorFamily::Unknown => &[],
    }
}

pub fn event_sound(source: &str, event: &str) -> &'static str {
    match family_of(source) {
        ActorFamily::Doom => doom_event_sound(event),
        ActorFamily::Unknown => "",
    }
}

/// Every weapon id this family's table describes, so a runtime can find a
/// definition by something other than its id (its view sprite, say) without
/// naming any weapon itself.
pub fn weapon_ids(source: &str) -> &'static [&'static str] {
    match family_of(source) {
        ActorFamily::Doom => &[
            "fist",
            "chainsaw",
            "pistol",
            "shotgun",
            "supershotgun",
            "chaingun",
            "rocket",
            "plasma",
            "bfg",
        ],
        ActorFamily::Unknown => &[],
    }
}

/// The weapon the player carries into a level of this family, and the ammo
/// they carry it with.
pub fn starting_weapons(source: &str) -> &'static [&'static str] {
    match family_of(source) {
        ActorFamily::Doom => &["fist", "pistol"],
        ActorFamily::Unknown => &[],
    }
}

/// Turn a sound STEM into a catalog alias, using a sibling billboard alias to
/// learn where this pack keeps its files.
///
/// `billboard_alias` is what the runtime already resolved for the artwork
/// (`doom/doom/billboards/doom1/troo`); the sound sits beside it under the
/// same pack (`doom/doom/sfx/doom1/dspistol`). Deriving it beats a second
/// table: a pack that renames its folders renames both at once.
///
/// Returns `None` for an empty stem or an alias that does not look like a
/// `.../billboards/{pack}/{key}` path — a caller with no example alias must
/// fall back to its category default rather than invent a path.
pub fn sound_alias(billboard_alias: &str, stem: &str) -> Option<String> {
    if stem.is_empty() {
        return None;
    }
    let (head, rest) = billboard_alias.split_once("/billboards/")?;
    let pack = rest.split('/').next().filter(|p| !p.is_empty())?;
    Some(format!("{head}/sfx/{pack}/{stem}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The newer sound slots (gib/step/ambient, variants, the projectile
    /// line) ride the manifest the same way — and a class without them
    /// writes byte-identical lines to the old form.
    #[test]
    fn new_sound_slots_and_projectile_round_trip() {
        let key = |s: &str, _k: ResourceKind| format!("sfx/pack/{s}");
        let def = ActorDef {
            role: ActorRole::Monster,
            health: 30.0,
            sounds: ActorSounds {
                sight: "see1",
                pain: "pain1|pain2",
                death: "die1",
                attack: "shoot1",
                active: "idle1",
                gib: "burst1",
                step: "clop",
                ambient: "hum",
            },
            projectile: Some(ProjectileDef {
                sprite: "ball",
                launch: "shoot1",
                fly: "whoosh",
                hit: "boom",
            }),
            ..ActorDef::PROP
        };
        let text = def.to_manifest(&|s, k| match k {
            ResourceKind::Sound => key(s, k),
            ResourceKind::Sprite => format!("billboards/pack/{s}"),
        });
        let lines: Vec<(&str, &str)> = text
            .lines()
            .filter_map(|l| l.split_once(' ').or(Some((l, ""))))
            .collect();
        let back = ActorDef::from_manifest(&lines).unwrap();
        assert_eq!(back.sounds.gib, "sfx/pack/burst1");
        assert_eq!(back.sounds.step, "sfx/pack/clop");
        assert_eq!(back.sounds.ambient, "sfx/pack/hum");
        assert_eq!(back.sounds.pain, "sfx/pack/pain1|sfx/pack/pain2");
        let p = back.projectile.expect("projectile line survives");
        assert_eq!(p.sprite, "billboards/pack/ball");
        assert_eq!(p.hit, "sfx/pack/boom");
        assert_eq!(p.fly, "sfx/pack/whoosh");
        // Without the new slots the sound line is exactly the old shape.
        let plain = ActorDef {
            sounds: ActorSounds { sight: "a", ..ActorSounds::NONE },
            ..ActorDef::PROP
        };
        let text = plain.to_manifest(&|s, _| s.to_string());
        assert!(text.contains("sound sight=a pain=- death=- attack=- active=-\n"));
        assert!(!text.contains("gib="), "unset slots stay unwritten");
    }

    /// A class survives the trip onto its asset and back: every field the
    /// engine reads, including the burst, the pickup line with its spaces,
    /// and resource stems mapped to the pack's keys.
    #[test]
    fn a_class_round_trips_through_its_manifest_lines() {
        let key = |s: &str, k: ResourceKind| match k {
            ResourceKind::Sound => format!("sfx/doom1/{s}"),
            ResourceKind::Sprite => format!("billboards/doom1/{s}"),
        };
        for class in [3004u16, 3001, 2011, 2001, 2035, 2028, 8] {
            let def = doom_actor_def(class).unwrap();
            let text = def.to_manifest(&key);
            let lines: Vec<(&str, &str)> = text
                .lines()
                .filter_map(|l| l.split_once(' ').or(Some((l, ""))))
                .collect();
            let back = ActorDef::from_manifest(&lines).unwrap();
            assert_eq!(back.role, def.role, "{class}");
            assert_eq!(back.health, def.health, "{class}");
            assert_eq!(back.pain_chance, def.pain_chance, "{class}");
            assert_eq!(back.speed, def.speed, "{class}");
            assert_eq!((back.radius, back.height), (def.radius, def.height), "{class}");
            assert_eq!(back.attack, def.attack, "{class}");
            assert_eq!(back.give.message, def.give.message, "{class}");
            assert_eq!(back.give.ammo, def.give.ammo, "{class}");
            assert_eq!(back.give.weapon, def.give.weapon, "{class}");
            assert_eq!(back.explode.map(|e| (e.radius, e.damage)), def.explode.map(|e| (e.radius, e.damage)), "{class}");
            if !def.sounds.sight.is_empty() {
                assert_eq!(back.sounds.sight, format!("sfx/doom1/{}", def.sounds.sight));
            }
            if let Some(e) = back.explode {
                assert_eq!(e.sprite, "billboards/doom1/bexp");
                assert_eq!(e.sound, "sfx/doom1/dsbarexp");
            }
        }
        let pistol = doom_weapon_def("pistol").unwrap();
        let back = WeaponDef::from_manifest(pistol.to_manifest(&key).trim_start_matches("weapon ")).unwrap();
        assert_eq!(back.id, "pistol");
        assert_eq!((back.damage_min, back.damage_max), (5.0, 15.0));
        assert_eq!(back.trigger, WeaponTrigger::Semi);
        assert_eq!(back.delivery, WeaponDelivery::Hitscan);
        assert_eq!(back.fire_sound, "sfx/doom1/dspistol");
        assert_eq!(back.view_sprite, "billboards/doom1/pisg");
        assert_eq!(back.puff_sprite, "billboards/doom1/puff");
        assert_eq!(back.ammo, "bullet");
        assert_eq!(back.mark, WeaponMark::Bullet);
        assert_eq!(back.color, DEFAULT_WEAPON_COLOR);
        let bfg = doom_weapon_def("bfg").unwrap();
        let bfg_back = WeaponDef::from_manifest(
            bfg.to_manifest(&key).trim_start_matches("weapon "),
        )
        .unwrap();
        assert_eq!(bfg_back.mark, WeaponMark::Energy);
        assert_eq!(bfg_back.color, [0.15, 1.0, 0.12, 0.95]);
    }

    #[test]
    fn quoted_values_keep_their_spaces() {
        let kv = parse_kv(r#"a=1 message="Picked up a \"big\" one." b=-"#);
        assert_eq!(kv[1].1, r#"Picked up a "big" one."#);
        assert_eq!(kv[2].1, "-");
    }

    /// The four classes the reference game is judged by. If these drift the
    /// combat stops feeling like the thing it was imported from.
    #[test]
    fn the_reference_monsters_carry_their_own_strength() {
        let zombie = doom_actor_def(3004).expect("zombieman");
        assert_eq!(zombie.role, ActorRole::Monster);
        assert_eq!(zombie.health, 20.0);
        let imp = doom_actor_def(3001).expect("imp");
        assert_eq!(imp.health, 60.0);
        let demon = doom_actor_def(3002).expect("demon");
        assert_eq!(demon.health, 150.0);
        // The Spectre is the same body under a different drawing.
        assert_eq!(doom_actor_def(58).expect("spectre").health, 150.0);
        let cyber = doom_actor_def(16).expect("cyberdemon");
        assert_eq!(cyber.health, 4000.0);
        // A Cyberdemon has no pain sound, and that absence is data.
        assert_eq!(cyber.sounds.pain, "");
        assert!(cyber.pain_chance < 0.1);
    }

    #[test]
    fn doom_fireball_monsters_carry_projectile_art_and_lifecycle_sounds() {
        for (class, sprite) in [(3001, "bal1"), (3005, "bal2"), (3003, "bal7"), (69, "bal7")] {
            let def = doom_actor_def(class).expect("projectile monster");
            assert_eq!(def.attack.map(|a| a.kind), Some(AttackKind::Projectile), "{class}");
            assert_eq!(
                def.projectile,
                Some(ProjectileDef {
                    sprite,
                    launch: "dsfirsht",
                    fly: "",
                    hit: "dsfirxpl",
                }),
                "{class}"
            );

            let text = def.to_manifest(&|stem, kind| match kind {
                ResourceKind::Sprite => format!("billboards/doom1/{stem}"),
                ResourceKind::Sound => format!("sfx/doom1/{stem}"),
            });
            assert!(
                text.contains(&format!(
                    "projectile sprite=billboards/doom1/{sprite} launch=sfx/doom1/dsfirsht fly=- hit=sfx/doom1/dsfirxpl\n"
                )),
                "{class}: {text}"
            );
        }

        let lost_soul = doom_actor_def(3006).expect("lost soul");
        assert_eq!(lost_soul.attack.map(|a| a.kind), Some(AttackKind::Melee));
        assert_eq!(lost_soul.projectile, None, "a charger is not a travelling shot");
    }

    #[test]
    fn pickups_say_what_they_give() {
        let stim = doom_actor_def(2011).expect("stimpack");
        assert_eq!(stim.role, ActorRole::Item);
        assert_eq!(stim.give.health, 10.0);
        assert_eq!(doom_actor_def(2012).unwrap().give.health, 25.0);
        assert_eq!(doom_actor_def(2007).unwrap().give.ammo_count, 10);
        assert_eq!(doom_actor_def(2007).unwrap().give.ammo, "bullet");
        // A bonus raises the ceiling; a stimpack does not.
        assert_eq!(doom_actor_def(2014).unwrap().give.health_max, 200.0);
        assert_eq!(stim.give.health_max, 0.0);
        // Green armour absorbs a third, blue a half.
        assert!((doom_actor_def(2018).unwrap().give.armor_absorb - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(doom_actor_def(2019).unwrap().give.armor_absorb, 0.5);
        assert_eq!(doom_actor_def(5).unwrap().give.key, "blue");
    }

    /// A floor shotgun must arm the weapon AND stock it; giving one without
    /// the other is the bug that makes a picked-up gun useless.
    #[test]
    fn a_floor_weapon_arms_and_stocks() {
        let g = doom_actor_def(2001).expect("shotgun").give;
        assert_eq!(g.weapon, "shotgun");
        assert_eq!(g.ammo, "shell");
        assert_eq!(g.ammo_count, 8);
        let w = doom_weapon_def(&g.weapon).expect("shotgun def");
        assert_eq!(w.ammo, "shell");
        assert_eq!(w.ammo_per_shot, 1);
    }

    /// The imported roster is the combat contract. Pin the natural Doom
    /// numbers here so no later runtime convenience collapses pellets,
    /// averages damage, or silently turns a semi-auto gun automatic.
    #[test]
    fn the_doom_weapon_table_keeps_its_natural_numbers() {
        let p = doom_weapon_def("pistol").expect("pistol");
        assert_eq!((p.damage_min, p.damage_max, p.rate), (5.0, 15.0, 1.9));
        assert_eq!(p.ammo, "bullet");
        assert_eq!(p.trigger, WeaponTrigger::Semi);

        let fist = doom_weapon_def("fist").unwrap();
        assert_eq!((fist.delivery, fist.damage_min, fist.damage_max, fist.rate),
            (WeaponDelivery::Melee, 2.0, 20.0, 1.9));
        assert_eq!(fist.mark, WeaponMark::None);
        assert_eq!(doom_weapon_def("chainsaw").unwrap().mark, WeaponMark::None);
        let shotgun = doom_weapon_def("shotgun").unwrap();
        assert_eq!((shotgun.pellets, shotgun.damage_min, shotgun.damage_max, shotgun.rate, shotgun.spread),
            (7, 5.0, 15.0, 1.05, 5.6));
        assert_eq!(shotgun.mark, WeaponMark::Pellet);
        let super_shotgun = doom_weapon_def("supershotgun").unwrap();
        assert_eq!((super_shotgun.pellets, super_shotgun.rate, super_shotgun.spread),
            (20, 0.85, 11.0));
        assert_eq!(super_shotgun.mark, WeaponMark::Pellet);
        let chain = doom_weapon_def("chaingun").unwrap();
        assert_eq!((chain.trigger, chain.damage_min, chain.damage_max, chain.rate),
            (WeaponTrigger::Auto, 5.0, 15.0, 8.7));
        let rocket = doom_weapon_def("rocket").unwrap();
        assert_eq!((rocket.delivery, rocket.damage_min, rocket.damage_max, rocket.projectile_speed),
            (WeaponDelivery::Projectile, 20.0, 160.0, 20.0));
        assert_eq!((rocket.splash_radius, rocket.splash_damage), (4.0, 128.0));
        assert_eq!(rocket.mark, WeaponMark::Scorch);
        assert_eq!(rocket.projectile_sprite, "misl");
        assert_eq!(rocket.impact_sprite, "misl-burst");
        let plasma = doom_weapon_def("plasma").unwrap();
        assert_eq!((plasma.trigger, plasma.rate, plasma.damage_min, plasma.damage_max, plasma.projectile_speed),
            (WeaponTrigger::Auto, 11.6, 5.0, 40.0, 25.0));
        assert_eq!(plasma.mark, WeaponMark::Energy);
        assert_eq!(plasma.color, [0.08, 0.35, 1.0, 0.92]);
        let bfg = doom_weapon_def("bfg").unwrap();
        assert_eq!((bfg.trigger, bfg.damage_min, bfg.damage_max, bfg.splash_radius, bfg.splash_damage),
            (WeaponTrigger::Semi, 100.0, 800.0, 6.0, 200.0));
        assert_eq!(bfg.mark, WeaponMark::Energy, "splash does not erase the BFG's energy identity");
        assert_eq!(bfg.color, [0.15, 1.0, 0.12, 0.95]);
        assert_eq!(
            DOOM_AMMO.iter().find(|a| a.id == "bullet").unwrap().start,
            50
        );
    }

    #[test]
    fn an_unknown_class_has_no_opinion() {
        assert!(doom_actor_def(31337).is_none());
        assert!(actor_def("kenney", "3004").is_none());
        assert!(weapon_def("kenney", "pistol").is_none());
        assert_eq!(event_sound("kenney", "player_pain"), "");
    }

    #[test]
    fn freedoom_reads_the_same_table_as_doom() {
        assert_eq!(family_of("freedoom"), ActorFamily::Doom);
        assert_eq!(family_of("doom"), ActorFamily::Doom);
        assert_eq!(family_of("kenney"), ActorFamily::Unknown);
        assert_eq!(actor_def("freedoom", "3001").unwrap().health, 60.0);
    }

    #[test]
    fn a_sound_alias_is_derived_from_a_sibling_billboard() {
        assert_eq!(
            sound_alias("doom/doom/billboards/doom1/troo", "dsbgsit1").as_deref(),
            Some("doom/doom/sfx/doom1/dsbgsit1")
        );
        assert_eq!(
            sound_alias("freedoom/freedoom/billboards/freedoom2/poss", "dspopain").as_deref(),
            Some("freedoom/freedoom/sfx/freedoom2/dspopain")
        );
        // No stem, or an alias that is not a billboard path: no guess.
        assert_eq!(sound_alias("doom/doom/billboards/doom1/troo", ""), None);
        assert_eq!(sound_alias("kenney/nature-kit/tree", "dspistol"), None);
    }

    /// Every sound slot any table entry names must be a plausible stem, and
    /// every weapon a pickup arms must exist. A typo here is silent at
    /// runtime — the sound simply never plays — so the test is the guard.
    #[test]
    fn every_named_weapon_and_sound_resolves() {
        for class in 0u16..=4096 {
            let Some(def) = doom_actor_def(class) else {
                continue;
            };
            if !def.give.weapon.is_empty() {
                assert!(
                    doom_weapon_def(def.give.weapon).is_some(),
                    "class {class} arms an unknown weapon '{}'",
                    def.give.weapon
                );
            }
            for slot in ["sight", "pain", "death", "attack", "active"] {
                let stem = def.sounds.slot(slot);
                assert!(
                    stem.is_empty() || stem.starts_with("ds"),
                    "class {class} {slot} sound '{stem}' is not a lump stem"
                );
            }
        }
    }
}
