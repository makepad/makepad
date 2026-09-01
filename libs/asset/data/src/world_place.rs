//! Map placement sidecar (`*.place`) next to a World GLB.
//!
//! The game frontend loads this to populate a converted map: each row names
//! a catalog asset by `{source}/{asset_key}` (the same key the importer
//! staged, which pack_import publishes as `{source}/{pack}/{key}`).
//!
//! ```text
//! world-place 1
//! source freedoom
//! world worlds/freedoom2/map01
//! spawn 1.0000 0.6406 2.0000 1.57080 0.00000
//! place thing-0 player - 1.0000 0.6406 2.0000 1.57080 class=player1
//! place thing-1 character billboards/freedoom2/poss 3.0000 0.0000 1.0000 0.00000 class=3004
//! ```

use std::path::Path;

pub const PLACE_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldPlace {
    pub source: String,
    pub world: String,
    pub spawn: Option<([f32; 3], f32, f32)>,
    pub places: Vec<Place>,
    /// The level's own facts about the people in it, written by the
    /// importer so the engine reads them off the map rather than off a
    /// table keyed by game. Every reference is a namespace-relative asset
    /// key, like [`Place::asset`]. Empty/zero on sidecars written before
    /// these lines existed.
    ///
    /// ```text
    /// person 1.75 health=100 max=200
    /// loadout fist pistol
    /// weapon pistol billboards/doom1/pisg
    /// pool bullet BULL 200 50
    /// event door_open sfx/doom1/dsdoropn
    /// ```
    pub family: Family,
}

/// The per-level facts of a map's family — see [`WorldPlace::family`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Family {
    /// How tall this level's PLAYER stands, metres, in the yardstick every
    /// linear number on its actors' definitions is written in. Zero = unknown.
    pub person_height: f32,
    /// Starting hit points and the ceiling pickups may heal toward. Zero
    /// means an older sidecar; runtimes use the classic 100/100 fallback.
    pub person_health: f32,
    pub person_health_max: f32,
    /// Weapon ids the player arrives holding, in the order to select them.
    pub loadout: Vec<String>,
    /// Weapon id → the view-sprite asset key whose manifest defines it.
    pub weapons: Vec<(String, String)>,
    /// Ammo pools: (id, HUD title, ceiling, what the player starts with).
    pub pools: Vec<(String, String, i32, i32)>,
    /// Level/player event → sound asset key (`door_open`, `player_pain`…).
    pub events: Vec<(String, String)>,
    /// Per-mover sound overrides: one row per animated part, listing the
    /// sound key of each transition it makes. The generic `events` table is
    /// the fallback — these rows are for the door that grinds when every
    /// other door hums.
    ///
    /// ```text
    /// mover door_3 open=sfx/doom1/dsbdopn close=sfx/doom1/dsbdcls
    /// mover lift_1 start=sfx/doom1/dspstart stop=sfx/doom1/dspstop move=sfx/doom1/dsstnmov
    /// ```
    pub movers: Vec<(String, Vec<(String, String)>)>,
    /// How this level is PLAYED, when it is not the default first-person
    /// walk: `"rts"` means a tiled top-down strategy map (top-down camera,
    /// unit orders, a walkability grid). Empty on every level written
    /// before modes existed, which is exactly the walker default.
    ///
    /// ```text
    /// mode rts
    /// cell 6.0
    /// grid worlds/valley.grid
    /// house north color=e8c040 side=0
    /// ```
    pub mode: String,
    /// Metres per map cell on a tiled level. Zero = not a tiled level.
    pub cell: f32,
    /// Namespace-relative key of the walkability grid sidecar
    /// ([`world_grid`](crate::world_grid)). Empty = none.
    pub grid: String,
    /// The playable houses: `(name, sRGB colour bytes, side index)`. The
    /// colour is the tint a house's units are remapped to; the side index
    /// groups houses that share a tech tree. Empty on non-strategy levels.
    pub houses: Vec<(String, [u8; 3], u32)>,
    /// Artwork keys whose DEFINITIONS the level wants even though no row
    /// places one — a skirmish map's buildable set.
    ///
    /// A `mode rts` level's production rules come from the definitions that
    /// ride on the artwork (`unit class=… cost=…`), and a runtime only reads
    /// the manifests its rows name. On a multiplayer map that starts empty
    /// that is nothing at all: no construction yard, no tank, no roster. This
    /// line is the level saying which pack content it plays WITH, in the same
    /// namespace-relative keys the rows use, repeatable:
    ///
    /// ```text
    /// roster billboards/cnc/mcv billboards/cnc/fact billboards/cnc/harv
    /// ```
    pub roster: Vec<String>,
    /// The level's own GAMEPLAY RULES, as raw `key=value` pairs in file
    /// order. Every one of them overrides an engine constant, and a level
    /// that declares none plays exactly as the engine's defaults do.
    ///
    /// Dotted keys reach the nested groups; `victory=` is repeatable and
    /// comma-separated so the whitespace-split line still reads:
    ///
    /// ```text
    /// rules credits_per_second=50 min_build_ticks=20 harvest.load_ticks=8
    /// rules power.brownout_scale=0.25 wave.min_units=4 tech_level=2
    /// rules victory=timer,team=north,seconds=600 victory=eliminate
    /// ```
    ///
    /// The engine (not this crate) knows which keys exist; the sidecar just
    /// carries them, so a new rule needs no schema change here.
    pub rules: Vec<(String, String)>,
}

impl Family {
    /// The house row named `name`, if the level has one.
    pub fn house(&self, name: &str) -> Option<&(String, [u8; 3], u32)> {
        self.houses.iter().find(|(n, _, _)| n == name)
    }

    /// A house's tint as 0..1 sRGB components.
    pub fn house_color(&self, name: &str) -> Option<[f32; 3]> {
        self.house(name)
            .map(|(_, c, _)| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0])
    }
}

/// `e8c040` / `#e8c040` -> `[0xe8, 0xc0, 0x40]`.
pub fn parse_hex_rgb(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    // A decimal `r,g,b` triple is the other spelling the contract uses for a
    // colour (§2's `remap <r,g,b> …`), and a reader that only knew hex threw
    // every entry of a converted pack's house ramp away — which read on
    // screen as two houses in one colour.
    if s.contains(',') {
        let mut it = s.split(',');
        let mut byte = || it.next()?.trim().parse::<u8>().ok();
        let (r, g, b) = (byte()?, byte()?, byte()?);
        return it.next().is_none().then_some([r, g, b]);
    }
    if s.len() != 6 {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub id: String,
    pub kind: String,
    /// Catalog asset key (`billboards/freedoom2/poss`), or empty if none.
    pub asset: String,
    pub pos: [f32; 3],
    pub yaw: f32,
    pub class: String,
    /// World-space quad size. Zero means the viewer should infer.
    pub width: f32,
    pub height: f32,
    /// `face` (camera-facing), `wall`, or `floor`.
    pub align: String,
    /// Raw source-format placement flags, preserved so a future difficulty
    /// option can re-derive another cast without re-importing. For Doom /
    /// Freedoom this is the THING record's 16-bit `flags` word verbatim
    /// (skill bits 0x0001/0x0002/0x0004, ambush 0x0008, multiplayer-only
    /// 0x0010). Zero for placements from formats that carry no such flags,
    /// and for old `.place` rows written before this field existed.
    pub flags: u32,
    /// Which house owns this piece. Empty = neutral.
    pub team: String,
    /// Health as a FRACTION of the piece's full hit points, `0..1`. Rows
    /// written before this key existed parse as `1.0` — undamaged.
    pub health: f32,
    /// Metres above the ground plane this floor-aligned card draws at, so
    /// overlapping cards on one flat map have a stable order instead of
    /// z-fighting. Zero = the spawning class picks its own default.
    pub layer: f32,
    /// Richness stage of a `resource` row (frame index into its sheet).
    pub stage: u32,
}

impl Default for Place {
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: String::new(),
            asset: String::new(),
            pos: [0.0; 3],
            yaw: 0.0,
            class: String::new(),
            width: 0.0,
            height: 0.0,
            align: String::new(),
            flags: 0,
            team: String::new(),
            health: 1.0,
            layer: 0.0,
            stage: 0,
        }
    }
}

impl WorldPlace {
    pub fn to_text(&self) -> String {
        let mut out = format!("world-place {PLACE_VERSION}\n");
        out.push_str(&format!("source {}\n", self.source));
        out.push_str(&format!("world {}\n", self.world));
        if let Some((pos, yaw, pitch)) = self.spawn {
            out.push_str(&format!(
                "spawn {:.4} {:.4} {:.4} {:.5} {:.5}\n",
                pos[0], pos[1], pos[2], yaw, pitch
            ));
        }
        let f = &self.family;
        if f.person_height > 0.0 {
            out.push_str(&format!("person {}", f.person_height));
            if f.person_health > 0.0 {
                out.push_str(&format!(" health={}", f.person_health));
            }
            if f.person_health_max > 0.0 {
                out.push_str(&format!(" max={}", f.person_health_max));
            }
            out.push('\n');
        }
        if !f.loadout.is_empty() {
            out.push_str(&format!("loadout {}\n", f.loadout.join(" ")));
        }
        for (id, key) in &f.weapons {
            out.push_str(&format!("weapon {id} {key}\n"));
        }
        for (id, title, max, start) in &f.pools {
            out.push_str(&format!("pool {id} {title} {max} {start}\n"));
        }
        for (event, key) in &f.events {
            out.push_str(&format!("event {event} {key}\n"));
        }
        if !f.mode.is_empty() {
            out.push_str(&format!("mode {}\n", f.mode));
        }
        if f.cell > 0.0 {
            out.push_str(&format!("cell {}\n", f.cell));
        }
        if !f.grid.is_empty() {
            out.push_str(&format!("grid {}\n", f.grid));
        }
        for (name, color, side) in &f.houses {
            out.push_str(&format!(
                "house {name} color={:02x}{:02x}{:02x} side={side}\n",
                color[0], color[1], color[2]
            ));
        }
        if !f.roster.is_empty() {
            out.push_str(&format!("roster {}\n", f.roster.join(" ")));
        }
        if !f.rules.is_empty() {
            out.push_str("rules");
            for (key, value) in &f.rules {
                out.push_str(&format!(" {key}={value}"));
            }
            out.push('\n');
        }
        for (part, sounds) in &f.movers {
            out.push_str(&format!("mover {part}"));
            for (event, key) in sounds {
                out.push_str(&format!(" {event}={key}"));
            }
            out.push('\n');
        }
        for p in &self.places {
            let asset = if p.asset.is_empty() { "-" } else { p.asset.as_str() };
            out.push_str(&format!(
                "place {} {} {} {:.4} {:.4} {:.4} {:.5}",
                p.id, p.kind, asset, p.pos[0], p.pos[1], p.pos[2], p.yaw
            ));
            if p.width > 0.0 && p.height > 0.0 {
                out.push_str(&format!(" w={:.4} h={:.4}", p.width, p.height));
            }
            if !p.team.is_empty() {
                out.push_str(&format!(" team={}", p.team));
            }
            if (p.health - 1.0).abs() > 1e-4 {
                out.push_str(&format!(" hp={:.2}", p.health));
            }
            if !p.align.is_empty() {
                out.push_str(&format!(" align={}", p.align));
            }
            if p.layer != 0.0 {
                out.push_str(&format!(" layer={}", p.layer));
            }
            if p.stage != 0 {
                out.push_str(&format!(" stage={}", p.stage));
            }
            if !p.class.is_empty() {
                out.push_str(&format!(" class={}", p.class));
            }
            if p.flags != 0 {
                out.push_str(&format!(" flags={}", p.flags));
            }
            out.push('\n');
        }
        out
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut source = String::new();
        let mut world = String::new();
        let mut spawn = None;
        let mut places = Vec::new();
        let mut family = Family::default();
        let mut saw = false;
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut it = line.split_whitespace();
            let Some(tag) = it.next() else { continue };
            match tag {
                "world-place" => {
                    let v: u32 = it
                        .next()
                        .ok_or("world-place version")?
                        .parse()
                        .map_err(|_| "world-place version")?;
                    if v != PLACE_VERSION {
                        return Err(format!("unsupported world-place {v}"));
                    }
                    saw = true;
                }
                "source" => source = it.next().unwrap_or("").to_string(),
                "world" => world = it.next().unwrap_or("").to_string(),
                "person" => {
                    family.person_height = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
                    for value in it {
                        if let Some(value) = value.strip_prefix("health=") {
                            family.person_health = value.parse().unwrap_or(0.0);
                        } else if let Some(value) = value.strip_prefix("max=") {
                            family.person_health_max = value.parse().unwrap_or(0.0);
                        }
                    }
                }
                "loadout" => family.loadout = it.map(String::from).collect(),
                "mode" => family.mode = it.next().unwrap_or("").to_string(),
                "cell" => family.cell = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0),
                "grid" => family.grid = it.next().unwrap_or("").to_string(),
                // Repeatable: one long line and several short ones say the
                // same thing, which is what a generator needs.
                "roster" => family.roster.extend(it.map(String::from)),
                // Repeatable, like `roster`: several short lines and one
                // long one say the same thing. Pairs are kept RAW and in
                // file order — the engine owns the vocabulary, not the
                // sidecar, so a rule this reader has never heard of still
                // reaches whoever does know it.
                "rules" => family.rules.extend(
                    it.filter_map(|kv| kv.split_once('='))
                        .filter(|(k, _)| !k.is_empty())
                        .map(|(k, v)| (k.to_string(), v.to_string())),
                ),
                "house" => {
                    let Some(name) = it.next() else { continue };
                    let mut color = [0xffu8; 3];
                    let mut side = 0u32;
                    for kv in it {
                        if let Some(v) = kv.strip_prefix("color=") {
                            if let Some(c) = parse_hex_rgb(v) {
                                color = c;
                            }
                        } else if let Some(v) = kv.strip_prefix("side=") {
                            side = v.parse().unwrap_or(0);
                        }
                    }
                    family.houses.push((name.to_string(), color, side));
                }
                "weapon" => {
                    if let (Some(id), Some(key)) = (it.next(), it.next()) {
                        family.weapons.push((id.to_string(), key.to_string()));
                    }
                }
                "pool" => {
                    let (Some(id), Some(title), Some(max), Some(start)) =
                        (it.next(), it.next(), it.next(), it.next())
                    else {
                        continue;
                    };
                    family.pools.push((
                        id.to_string(),
                        title.to_string(),
                        max.parse().unwrap_or(0),
                        start.parse().unwrap_or(0),
                    ));
                }
                "event" => {
                    if let (Some(event), Some(key)) = (it.next(), it.next()) {
                        family.events.push((event.to_string(), key.to_string()));
                    }
                }
                "mover" => {
                    let Some(part) = it.next() else { continue };
                    let sounds: Vec<(String, String)> = it
                        .filter_map(|kv| kv.split_once('='))
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    if !sounds.is_empty() {
                        family.movers.push((part.to_string(), sounds));
                    }
                }
                "spawn" => {
                    let nums: Vec<&str> = it.collect();
                    if nums.len() < 5 {
                        return Err("spawn needs x y z yaw pitch".into());
                    }
                    let p = |i: usize| nums[i].parse::<f32>().map_err(|_| "spawn number");
                    spawn = Some(([p(0)?, p(1)?, p(2)?], p(3)?, p(4)?));
                }
                "place" => {
                    let id = it.next().ok_or("place id")?.to_string();
                    let kind = it.next().ok_or("place kind")?.to_string();
                    let asset = it.next().ok_or("place asset")?.to_string();
                    let x: f32 = it.next().ok_or("place x")?.parse().map_err(|_| "place x")?;
                    let y: f32 = it.next().ok_or("place y")?.parse().map_err(|_| "place y")?;
                    let z: f32 = it.next().ok_or("place z")?.parse().map_err(|_| "place z")?;
                    let yaw: f32 = it.next().unwrap_or("0").parse().unwrap_or(0.0);
                    let mut class = String::new();
                    let mut width = 0.0f32;
                    let mut height = 0.0f32;
                    let mut align = String::new();
                    // `flags` is a newer, optional attribute: rows written
                    // before it existed simply lack the key and default to 0.
                    let mut flags = 0u32;
                    let mut team = String::new();
                    // Undamaged unless the row says otherwise, so every row
                    // written before `hp=` existed reads as full health.
                    let mut health = 1.0f32;
                    let mut layer = 0.0f32;
                    let mut stage = 0u32;
                    for extra in it {
                        if let Some(v) = extra.strip_prefix("class=") {
                            class = v.to_string();
                        } else if let Some(v) = extra.strip_prefix("w=") {
                            width = v.parse().unwrap_or(0.0);
                        } else if let Some(v) = extra.strip_prefix("h=") {
                            height = v.parse().unwrap_or(0.0);
                        } else if let Some(v) = extra.strip_prefix("align=") {
                            align = v.to_string();
                        } else if let Some(v) = extra.strip_prefix("flags=") {
                            flags = v.parse().unwrap_or(0);
                        } else if let Some(v) = extra.strip_prefix("team=") {
                            team = v.to_string();
                        } else if let Some(v) = extra.strip_prefix("hp=") {
                            health = v.parse::<f32>().unwrap_or(1.0).clamp(0.0, 1.0);
                        } else if let Some(v) = extra.strip_prefix("layer=") {
                            layer = v.parse().unwrap_or(0.0);
                        } else if let Some(v) = extra.strip_prefix("stage=") {
                            stage = v.parse().unwrap_or(0);
                        }
                    }
                    places.push(Place {
                        id,
                        kind,
                        asset: if asset == "-" { String::new() } else { asset },
                        pos: [x, y, z],
                        yaw,
                        class,
                        width,
                        height,
                        align,
                        flags,
                        team,
                        health,
                        layer,
                        stage,
                    });
                }
                _ => {}
            }
        }
        if !saw {
            return Err("not a world-place file".into());
        }
        Ok(Self {
            source,
            world,
            spawn,
            places,
            family,
        })
    }
}

pub fn write_place_sidecar(glb: &Path, place: &WorldPlace) -> Result<(), String> {
    std::fs::write(glb.with_extension("place"), place.to_text()).map_err(|e| e.to_string())
}

/// Doom / Freedoom thing type → (kind, sprite prefix). Prefix matches the
/// collapsed billboard key `billboards/{wad}/{prefix}`.
pub fn doom_thing_actor(typ: u16) -> Option<(&'static str, &'static str)> {
    Some(match typ {
        1 | 2 | 3 | 4 | 11 => ("player", ""),
        3004 => ("character", "poss"),
        9 => ("character", "spos"),
        84 => ("character", "sswv"),
        3001 => ("character", "troo"),
        3002 | 58 => ("character", "sarg"),
        3006 => ("character", "skul"),
        3005 => ("character", "head"),
        3003 => ("character", "boss"),
        16 => ("character", "cybr"),
        68 => ("character", "bspi"),
        64 => ("character", "vile"),
        65 => ("character", "cpos"),
        66 => ("character", "skel"),
        67 => ("character", "fatt"),
        69 => ("character", "bos2"),
        71 => ("character", "pain"),
        72 => ("character", "keen"),
        2001 => ("weapon", "shot"),
        2002 => ("weapon", "mgun"),
        2003 => ("weapon", "laun"),
        2004 => ("weapon", "plas"),
        2005 => ("weapon", "csaw"),
        2006 => ("weapon", "bfug"),
        82 => ("weapon", "csaw"),
        2007 => ("pickup", "clip"),
        2048 => ("pickup", "ammo"),
        2008 => ("pickup", "shel"),
        2049 => ("pickup", "sbox"),
        2010 => ("pickup", "rock"),
        2046 => ("pickup", "brok"),
        2047 => ("pickup", "cell"),
        17 => ("pickup", "celp"),
        2011 => ("pickup", "stim"),
        2012 => ("pickup", "medi"),
        2014 => ("pickup", "bon1"),
        2015 => ("pickup", "bon2"),
        2018 => ("pickup", "arm1"),
        2019 => ("pickup", "arm2"),
        8 => ("pickup", "bpak"),
        2024 => ("pickup", "pins"),
        2022 => ("pickup", "pinv"),
        83 => ("pickup", "mega"),
        5 => ("pickup", "bkey"),
        6 => ("pickup", "ykey"),
        13 => ("pickup", "rkey"),
        38 => ("pickup", "rsku"),
        39 => ("pickup", "ysku"),
        40 => ("pickup", "bsku"),
        2028 => ("prop", "colu"),
        2035 => ("prop", "bar1"),
        _ => return None,
    })
}

/// Quake III classname → (kind, catalog key). Empty asset = spawn / no mesh.
/// Keys match `convert_md3` / `assemble_players_and_weapons` slugs.
pub fn q3_class_actor(class: &str) -> Option<(&'static str, &'static str)> {
    Some(match class {
        "info_player_start" | "info_player_deathmatch" | "info_player_intermission" => {
            ("player", "")
        }
        "weapon_shotgun" => ("weapon", "weapons/shotgun"),
        "weapon_rocketlauncher" => ("weapon", "weapons/rocketl"),
        "weapon_grenadelauncher" => ("weapon", "weapons/grenadel"),
        "weapon_plasmagun" => ("weapon", "weapons/plasma"),
        "weapon_machinegun" => ("weapon", "weapons/machinegun"),
        "weapon_lightning" => ("weapon", "weapons/lightning"),
        "weapon_railgun" => ("weapon", "weapons/railgun"),
        "weapon_bfg" => ("weapon", "weapons/bfg"),
        "weapon_gauntlet" => ("weapon", "weapons/gauntlet"),
        "weapon_grapplinghook" => ("weapon", "weapons/grapple"),
        "item_armor_shard" => ("pickup", "props/armor-shard"),
        "item_armor_combat" => ("pickup", "props/armor-armor_yel"),
        "item_armor_body" => ("pickup", "props/armor-armor_red"),
        "item_health_small" => ("pickup", "props/health-small_cross"),
        "item_health" => ("pickup", "props/health-medium_cross"),
        "item_health_large" => ("pickup", "props/health-large_cross"),
        "item_health_mega" => ("pickup", "props/health-mega_cross"),
        "item_quad" => ("pickup", "props/instant-quad"),
        "item_enviro" => ("pickup", "props/instant-enviro"),
        "item_haste" => ("pickup", "props/instant-haste"),
        "item_invis" => ("pickup", "props/instant-invis"),
        "item_regen" => ("pickup", "props/instant-regen"),
        "item_flight" => ("pickup", "props/instant-flight"),
        "holdable_teleporter" => ("pickup", "props/holdable-teleporter"),
        "holdable_medkit" => ("pickup", "props/holdable-medkit"),
        "ammo_shells" => ("pickup", "props/ammo-shotgunam"),
        "ammo_bullets" => ("pickup", "props/ammo-machinegunam"),
        "ammo_grenades" => ("pickup", "props/ammo-grenadeam"),
        "ammo_rockets" => ("pickup", "props/ammo-rocketam"),
        "ammo_lightning" => ("pickup", "props/ammo-lightningam"),
        "ammo_slugs" => ("pickup", "props/ammo-railgunam"),
        "ammo_cells" => ("pickup", "props/ammo-plasmaam"),
        "ammo_bfg" => ("pickup", "props/ammo-bfgam"),
        "team_CTF_redflag" => ("prop", "props/flags-r_flag"),
        "team_CTF_blueflag" => ("prop", "props/flags-b_flag"),
        _ => return None,
    })
}

/// Catalog key for a Q3 `.md3` path (`models/mapobjects/storch/tall_torch.md3`
/// → `props/storch-tall_torch`; assembled guns stay `weapons/shotgun`).
pub fn q3_md3_catalog_key(rel: &str) -> String {
    let lower = rel.replace('\\', "/").to_ascii_lowercase();
    let lower = lower.trim_start_matches('/');
    if let Some(rest) = lower.strip_prefix("models/weapons2/") {
        if let Some((gun, file)) = rest.split_once('/') {
            let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
            if stem == gun {
                return format!("weapons/{gun}");
            }
        }
    }
    if let Some(rest) = lower.strip_prefix("models/players/") {
        if let Some((who, _)) = rest.split_once('/') {
            return format!("characters/{who}");
        }
    }
    let folder = if lower.contains("/players/") || lower.starts_with("players/") {
        "characters"
    } else if lower.contains("/weapons/")
        || lower.starts_with("weapons/")
        || lower.contains("/weapon")
    {
        "weapons"
    } else {
        "props"
    };
    let n = lower.rsplit_once('.').map(|(s, _)| s).unwrap_or(lower);
    let parts: Vec<&str> = n.split('/').filter(|s| !s.is_empty()).collect();
    let take = if parts.len() >= 2 {
        &parts[parts.len() - 2..]
    } else {
        &parts[..]
    };
    let mut slug = String::new();
    for c in take.join("-").chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        slug = "asset".into();
    }
    format!("{folder}/{slug}")
}

/// Quake / LibreQuake classname → (kind, asset key relative to convert_mdl).
pub fn quake_class_actor(class: &str) -> Option<(&'static str, &'static str)> {
    Some(match class {
        "info_player_start" | "info_player_coop" | "info_player_deathmatch" => ("player", ""),
        "monster_army" => ("character", "characters/soldier"),
        "monster_dog" => ("character", "characters/dog"),
        "monster_ogre" | "monster_ogre_marksman" => ("character", "characters/ogre"),
        "monster_knight" => ("character", "characters/knight"),
        "monster_zombie" => ("character", "characters/zombie"),
        "monster_wizard" => ("character", "characters/wizard"),
        "monster_demon1" => ("character", "characters/demon"),
        "monster_shambler" => ("character", "characters/shambler"),
        "monster_boss" => ("character", "characters/boss"),
        "monster_enforcer" => ("character", "characters/enforcer"),
        "monster_hell_knight" => ("character", "characters/hknight"),
        "monster_shalrath" => ("character", "characters/shalrath"),
        "monster_tarbaby" => ("character", "characters/tarbaby"),
        "monster_fish" => ("character", "characters/fish"),
        "weapon_nailgun" => ("weapon", "weapons/g_nail"),
        "weapon_supernailgun" => ("weapon", "weapons/g_nail2"),
        "weapon_supershotgun" => ("weapon", "weapons/g_shot"),
        "weapon_grenadelauncher" => ("weapon", "weapons/g_rock"),
        "weapon_rocketlauncher" => ("weapon", "weapons/g_rock2"),
        "weapon_lightning" => ("weapon", "weapons/g_light"),
        "item_armor1" | "item_armor2" | "item_armorInv" => ("pickup", ""),
        "item_health" | "item_artifact_super_damage" | "item_artifact_invulnerability" => {
            ("pickup", "")
        }
        "light" => ("light", ""),
        _ if class.starts_with("trap_") || class.starts_with("trigger_") || class.starts_with("func_") => {
            ("trigger", "")
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mover rows survive the round trip and unknown rows stay ignored.
    #[test]
    fn mover_sound_rows_round_trip() {
        let mut place = WorldPlace {
            source: "doom".into(),
            world: "worlds/doom1/e1m1".into(),
            ..WorldPlace::default()
        };
        place.family.movers.push((
            "door_3".into(),
            vec![
                ("open".into(), "sfx/doom1/dsbdopn".into()),
                ("close".into(), "sfx/doom1/dsbdcls".into()),
            ],
        ));
        place.family.movers.push((
            "lift_1".into(),
            vec![("start".into(), "sfx/doom1/dspstart".into())],
        ));
        let text = place.to_text();
        assert!(text.contains("mover door_3 open=sfx/doom1/dsbdopn close=sfx/doom1/dsbdcls\n"));
        let back = WorldPlace::parse(&text).unwrap();
        assert_eq!(back.family.movers, place.family.movers);
        // A sidecar without mover rows parses exactly as before.
        let plain = WorldPlace::parse("world-place 1\nsource doom\nworld w\n").unwrap();
        assert!(plain.family.movers.is_empty());
    }

    #[test]
    fn person_vitals_extend_the_old_height_line_and_round_trip() {
        let mut place = WorldPlace::default();
        place.family.person_height = 1.75;
        place.family.person_health = 100.0;
        place.family.person_health_max = 200.0;
        let text = place.to_text();
        assert!(text.contains("person 1.75 health=100 max=200\n"));
        let back = WorldPlace::parse(&text).unwrap();
        assert_eq!(back.family.person_height, 1.75);
        assert_eq!((back.family.person_health, back.family.person_health_max), (100.0, 200.0));

        let old = WorldPlace::parse("world-place 1\nperson 1.75\n").unwrap();
        assert_eq!((old.family.person_health, old.family.person_health_max), (0.0, 0.0));
    }

    #[test]
    fn place_text_round_trips() {
        let p = WorldPlace {
            source: "freedoom".into(),
            world: "worlds/freedoom2/map01".into(),
            spawn: Some(([1.0, 0.64, 2.0], 1.57, 0.0)),
            places: vec![
                Place {
                    id: "thing-0".into(),
                    kind: "player".into(),
                    asset: String::new(),
                    pos: [1.0, 0.64, 2.0],
                    yaw: 1.57,
                    class: "player1".into(),
                    width: 0.0,
                    height: 0.0,
                    align: String::new(),
                    flags: 0,
                    ..Place::default()
                },
                Place {
                    id: "thing-1".into(),
                    kind: "character".into(),
                    asset: "billboards/freedoom2/poss".into(),
                    pos: [3.0, 0.0, 1.0],
                    yaw: 0.0,
                    class: "3004".into(),
                    width: 0.0,
                    height: 0.0,
                    align: String::new(),
                    flags: 0,
                    ..Place::default()
                },
            ],
            family: Default::default(),
        };
        let parsed = WorldPlace::parse(&p.to_text()).expect("parse");
        assert_eq!(parsed.source, "freedoom");
        assert_eq!(parsed.world, "worlds/freedoom2/map01");
        assert_eq!(parsed.places.len(), 2);
        assert_eq!(parsed.places[1].asset, "billboards/freedoom2/poss");
        assert!(parsed.places[0].asset.is_empty());
    }

    /// The raw THING `flags` word (skill bits + multiplayer-only) must
    /// survive `to_text` -> `parse`, so a future difficulty option can
    /// re-derive another cast from an already-imported `.place` file.
    #[test]
    fn place_flags_round_trip() {
        let p = WorldPlace {
            source: "doom".into(),
            world: "worlds/doom1/e1m1".into(),
            spawn: None,
            places: vec![Place {
                id: "thing-9".into(),
                kind: "character".into(),
                asset: "billboards/doom1/spos".into(),
                pos: [1.0, 0.0, 2.0],
                yaw: 0.0,
                class: "9".into(),
                width: 0.0,
                height: 0.0,
                align: String::new(),
                // skill 3 (0x0002) + skill 4/5 (0x0004) + ambush (0x0008)
                flags: 0x000E,
                ..Place::default()
            }],
            family: Default::default(),
        };
        let text = p.to_text();
        assert!(text.contains("flags=14"), "expected flags=14 in: {text}");
        let parsed = WorldPlace::parse(&text).expect("parse");
        assert_eq!(parsed.places[0].flags, 0x000E);
    }

    /// A `.place` file written before `flags` existed has no `flags=` key on
    /// its `place` rows — that must still parse, defaulting to 0, not error.
    #[test]
    fn place_without_flags_key_parses_as_zero() {
        let text = "world-place 1\n\
             source doom\n\
             world worlds/doom1/e1m1\n\
             place thing-0 character billboards/doom1/poss 1.0000 0.0000 2.0000 0.00000 class=3004\n";
        let parsed = WorldPlace::parse(text).expect("parse");
        assert_eq!(parsed.places.len(), 1);
        assert_eq!(parsed.places[0].flags, 0);
    }

    /// The strategy-map facts (`mode`/`cell`/`grid`/`house`) and the row
    /// keys they bring (`team=`, `hp=`, `layer=`, `stage=`) survive the
    /// round trip, and a sidecar written before they existed still parses
    /// with a full-health, neutral, layer-0 default.
    #[test]
    fn a_roster_survives_the_round_trip_and_accepts_several_lines() {
        let mut place = WorldPlace {
            source: "pack".into(),
            world: "worlds/acres".into(),
            ..WorldPlace::default()
        };
        place.family.mode = "rts".into();
        place.family.cell = 6.0;
        place.family.roster = vec![
            "billboards/pack/mcv".into(),
            "billboards/pack/yard".into(),
        ];
        let back = WorldPlace::parse(&place.to_text()).unwrap();
        assert_eq!(back.family.roster, place.family.roster);

        // Repeatable, because a generator writing one key per line is as
        // legitimate as one writing them all on one.
        let split = WorldPlace::parse(
            "world-place 1\nsource pack\nworld worlds/acres\nmode rts\n\
             roster billboards/pack/mcv\nroster billboards/pack/yard billboards/pack/tank\n",
        )
        .unwrap();
        assert_eq!(
            split.family.roster,
            vec![
                "billboards/pack/mcv".to_string(),
                "billboards/pack/yard".to_string(),
                "billboards/pack/tank".to_string(),
            ]
        );

        // A level that declares none writes none — an older reader sees the
        // sidecar it always saw.
        let mut bare = place.clone();
        bare.family.roster.clear();
        assert!(!bare.to_text().contains("roster"));
    }

    /// A MAP may carry the round's rules. They round-trip verbatim, several
    /// lines are one list, and a sidecar that declares none writes none —
    /// which is the whole point: no rules line, engine defaults, today's
    /// game.
    #[test]
    fn a_rules_line_carries_the_levels_gameplay_constants() {
        let mut place = WorldPlace {
            source: "pack".into(),
            world: "worlds/acres".into(),
            ..WorldPlace::default()
        };
        place.family.mode = "rts".into();
        place.family.rules = vec![
            ("credits_per_second".into(), "50".into()),
            ("harvest.load_ticks".into(), "8".into()),
            ("victory".into(), "timer,team=north,seconds=600".into()),
        ];
        let back = WorldPlace::parse(&place.to_text()).unwrap();
        assert_eq!(back.family.rules, place.family.rules);

        let split = WorldPlace::parse(
            "world-place 1\nsource pack\nworld worlds/acres\nmode rts\n\
             rules credits_per_second=50\nrules wave.min_units=4 tech_level=2\n",
        )
        .unwrap();
        assert_eq!(
            split.family.rules,
            vec![
                ("credits_per_second".to_string(), "50".to_string()),
                ("wave.min_units".to_string(), "4".to_string()),
                ("tech_level".to_string(), "2".to_string()),
            ]
        );

        let mut bare = place.clone();
        bare.family.rules.clear();
        assert!(!bare.to_text().contains("rules"));
    }

    #[test]
    fn strategy_map_facts_and_row_keys_round_trip() {
        let mut place = WorldPlace {
            source: "pack".into(),
            world: "worlds/valley".into(),
            ..WorldPlace::default()
        };
        place.family.mode = "rts".into();
        place.family.cell = 6.0;
        place.family.grid = "worlds/valley.grid".into();
        place.family.houses.push(("north".into(), [0xe8, 0xc0, 0x40], 0));
        place.family.houses.push(("south".into(), [0xd0, 0x20, 0x20], 1));
        place.places.push(Place {
            id: "u-12".into(),
            kind: "unit".into(),
            asset: "billboards/pack/tank".into(),
            pos: [18.0, 0.10, 42.0],
            team: "north".into(),
            health: 0.75,
            align: "floor".into(),
            layer: 0.10,
            class: "vehicle".into(),
            ..Place::default()
        });
        place.places.push(Place {
            id: "r-91".into(),
            kind: "resource".into(),
            asset: "billboards/pack/patch".into(),
            pos: [21.0, 0.04, 27.0],
            align: "floor".into(),
            layer: 0.04,
            stage: 7,
            class: "resource".into(),
            ..Place::default()
        });
        let text = place.to_text();
        assert!(text.contains("mode rts\n"), "{text}");
        assert!(text.contains("cell 6\n"), "{text}");
        assert!(text.contains("grid worlds/valley.grid\n"), "{text}");
        assert!(text.contains("house north color=e8c040 side=0\n"), "{text}");
        assert!(text.contains(" team=north hp=0.75 align=floor layer=0.1"), "{text}");
        assert!(text.contains(" stage=7"), "{text}");
        let back = WorldPlace::parse(&text).expect("parse");
        assert_eq!(back.family.mode, "rts");
        assert_eq!(back.family.cell, 6.0);
        assert_eq!(back.family.grid, "worlds/valley.grid");
        assert_eq!(back.family.houses, place.family.houses);
        assert_eq!(back.family.house_color("south"), Some([0xd0 as f32 / 255.0, 0x20 as f32 / 255.0, 0x20 as f32 / 255.0]));
        assert_eq!(back.places, place.places);

        let old = WorldPlace::parse(
            "world-place 1\nsource doom\nworld w\n\
             place thing-0 character billboards/doom1/poss 1.0 0.0 2.0 0.0 class=3004\n",
        )
        .expect("parse");
        assert!(old.family.mode.is_empty());
        assert!(old.family.houses.is_empty());
        assert_eq!(old.places[0].health, 1.0);
        assert_eq!(old.places[0].layer, 0.0);
        assert_eq!(old.places[0].stage, 0);
        assert!(old.places[0].team.is_empty());
    }

    #[test]
    fn q3_keys_match_assembled_weapons_and_mapobjects() {
        assert_eq!(
            q3_class_actor("weapon_rocketlauncher"),
            Some(("weapon", "weapons/rocketl"))
        );
        assert_eq!(
            q3_md3_catalog_key("models/mapobjects/storch/tall_torch.md3"),
            "props/storch-tall_torch"
        );
        assert_eq!(
            q3_md3_catalog_key("models/weapons2/shotgun/shotgun.md3"),
            "weapons/shotgun"
        );
    }
}
