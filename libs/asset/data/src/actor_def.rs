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
}

impl ActorSounds {
    pub const NONE: ActorSounds = ActorSounds {
        sight: "",
        pain: "",
        death: "",
        attack: "",
        active: "",
    };

    pub fn slot(&self, name: &str) -> &'static str {
        match name {
            "sight" => self.sight,
            "pain" => self.pain,
            "death" => self.death,
            "attack" => self.attack,
            "active" => self.active,
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
        self
    }
}

/// A weapon the PLAYER can hold. Same shape as an attack plus the magazine,
/// so the runtime can hand it straight to its gun kit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeaponDef {
    pub id: &'static str,
    pub title: &'static str,
    pub kind: AttackKind,
    pub damage: f32,
    pub rate: f32,
    pub range: f32,
    pub speed: f32,
    pub spread: f32,
    /// Ammo pool drawn from; empty means the weapon never runs dry.
    pub ammo: &'static str,
    /// Rounds spent per trigger pull.
    pub ammo_per_shot: i32,
    /// Held down to keep firing, or one shot per press.
    pub auto: bool,
    pub fire_sound: &'static str,
    /// Played when the trigger is pulled with the pool empty.
    pub empty_sound: &'static str,
    /// Billboard stem for the first-person view model, if the pack has one.
    pub view_sprite: &'static str,
    /// Slot number for a weapon-select HUD and for `1..9` switching.
    pub slot: u8,
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
            .with_sounds(ActorSounds {
                sight: "dsbgsit1",
                pain: "dspopain",
                death: "dsbgdth1",
                attack: "dsfirsht",
                active: "dsbgact",
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
            .with_sounds(ActorSounds {
                sight: "dscacsit",
                pain: "dsdmpain",
                death: "dscacdth",
                attack: "dsfirsht",
                active: "dsdmact",
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
            .with_sounds(ActorSounds {
                sight: "dsbrssit",
                pain: "dsdmpain",
                death: "dsbrsdth",
                attack: "dsfirsht",
                active: "dsdmact",
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
            .with_sounds(ActorSounds {
                sight: "dskntsit",
                pain: "dsdmpain",
                death: "dskntdth",
                attack: "dsfirsht",
                active: "dsdmact",
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
        2035 => ActorDef {
            role: ActorRole::Monster,
            health: 20.0,
            pain_chance: 0.0,
            speed: 0.0,
            sight_range: 0.0,
            attack: None,
            sounds: ActorSounds {
                death: "dsbarexp",
                ..ActorSounds::NONE
            },
            ..ActorDef::PROP.with_body(0.5, 1.0)
        },
        2028 => ActorDef::PROP.with_body(0.5, 1.5),
        _ => return None,
    };
    Some(d)
}

/// The weapons this family's player can hold. Damage is per trigger pull:
/// a shotgun's seven pellets are one 70-point event, because the engine
/// resolves one ray and the player cannot tell the difference.
pub fn doom_weapon_def(id: &str) -> Option<WeaponDef> {
    let w = match id {
        "fist" => WeaponDef {
            id: "fist",
            title: "Fist",
            kind: AttackKind::Melee,
            damage: roll(10, 2),
            rate: 2.0,
            range: 1.6,
            speed: 0.0,
            spread: 0.0,
            ammo: "",
            ammo_per_shot: 0,
            auto: true,
            fire_sound: "dspunch",
            empty_sound: "",
            view_sprite: "punch",
            slot: 1,
        },
        "chainsaw" => WeaponDef {
            id: "chainsaw",
            title: "Chainsaw",
            kind: AttackKind::Melee,
            damage: roll(10, 2),
            rate: 8.75,
            range: 1.8,
            speed: 0.0,
            spread: 0.0,
            ammo: "",
            ammo_per_shot: 0,
            auto: true,
            fire_sound: "dssawful",
            empty_sound: "",
            view_sprite: "sawg",
            slot: 1,
        },
        "pistol" => WeaponDef {
            id: "pistol",
            title: "Pistol",
            kind: AttackKind::Hitscan,
            damage: roll(3, 5),
            rate: 2.0,
            range: 100.0,
            speed: 0.0,
            spread: 0.02,
            ammo: "bullet",
            ammo_per_shot: 1,
            auto: false,
            fire_sound: "dspistol",
            empty_sound: "dsnoway",
            view_sprite: "pisg",
            slot: 2,
        },
        "shotgun" => WeaponDef {
            id: "shotgun",
            title: "Shotgun",
            kind: AttackKind::Hitscan,
            damage: roll(3, 5) * 7.0,
            rate: 0.86,
            range: 100.0,
            speed: 0.0,
            spread: 0.08,
            ammo: "shell",
            ammo_per_shot: 1,
            auto: false,
            fire_sound: "dsshotgn",
            empty_sound: "dsnoway",
            view_sprite: "shtg",
            slot: 3,
        },
        "supershotgun" => WeaponDef {
            id: "supershotgun",
            title: "Super shotgun",
            kind: AttackKind::Hitscan,
            damage: roll(3, 5) * 20.0,
            rate: 0.55,
            range: 100.0,
            speed: 0.0,
            spread: 0.16,
            ammo: "shell",
            ammo_per_shot: 2,
            auto: false,
            fire_sound: "dsdshtgn",
            empty_sound: "dsnoway",
            view_sprite: "sht2",
            slot: 3,
        },
        "chaingun" => WeaponDef {
            id: "chaingun",
            title: "Chaingun",
            kind: AttackKind::Hitscan,
            damage: roll(3, 5),
            rate: 8.75,
            range: 100.0,
            speed: 0.0,
            spread: 0.04,
            ammo: "bullet",
            ammo_per_shot: 1,
            auto: true,
            fire_sound: "dspistol",
            empty_sound: "dsnoway",
            view_sprite: "chgg",
            slot: 4,
        },
        "rocket" => WeaponDef {
            id: "rocket",
            title: "Rocket launcher",
            kind: AttackKind::Projectile,
            damage: 120.0,
            rate: 1.1,
            range: 120.0,
            speed: 25.0,
            spread: 0.0,
            ammo: "rocket",
            ammo_per_shot: 1,
            auto: true,
            fire_sound: "dsrlaunc",
            empty_sound: "dsnoway",
            view_sprite: "misg",
            slot: 5,
        },
        "plasma" => WeaponDef {
            id: "plasma",
            title: "Plasma rifle",
            kind: AttackKind::Projectile,
            damage: roll(8, 5),
            rate: 11.0,
            range: 120.0,
            speed: 40.0,
            spread: 0.0,
            ammo: "cell",
            ammo_per_shot: 1,
            auto: true,
            fire_sound: "dsplasma",
            empty_sound: "dsnoway",
            view_sprite: "plsg",
            slot: 6,
        },
        "bfg" => WeaponDef {
            id: "bfg",
            title: "BFG9000",
            kind: AttackKind::Projectile,
            damage: 400.0,
            rate: 0.5,
            range: 120.0,
            speed: 25.0,
            spread: 0.0,
            ammo: "cell",
            ammo_per_shot: 40,
            auto: false,
            fire_sound: "dsbfg",
            empty_sound: "dsnoway",
            view_sprite: "bfgg",
            slot: 7,
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

    /// The pistol is the yardstick every other number is judged against.
    #[test]
    fn the_pistol_hits_for_ten() {
        let p = doom_weapon_def("pistol").expect("pistol");
        assert_eq!(p.damage, 10.0);
        assert_eq!(p.ammo, "bullet");
        assert!(!p.auto, "the pistol is one shot per press");
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
