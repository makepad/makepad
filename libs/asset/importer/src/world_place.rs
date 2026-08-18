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

#[derive(Clone, Debug, PartialEq)]
pub struct WorldPlace {
    pub source: String,
    pub world: String,
    pub spawn: Option<([f32; 3], f32, f32)>,
    pub places: Vec<Place>,
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
        for p in &self.places {
            let asset = if p.asset.is_empty() { "-" } else { p.asset.as_str() };
            out.push_str(&format!(
                "place {} {} {} {:.4} {:.4} {:.4} {:.5}",
                p.id, p.kind, asset, p.pos[0], p.pos[1], p.pos[2], p.yaw
            ));
            if p.width > 0.0 && p.height > 0.0 {
                out.push_str(&format!(" w={:.4} h={:.4}", p.width, p.height));
            }
            if !p.align.is_empty() {
                out.push_str(&format!(" align={}", p.align));
            }
            if !p.class.is_empty() {
                out.push_str(&format!(" class={}", p.class));
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
                    for extra in it {
                        if let Some(v) = extra.strip_prefix("class=") {
                            class = v.to_string();
                        } else if let Some(v) = extra.strip_prefix("w=") {
                            width = v.parse().unwrap_or(0.0);
                        } else if let Some(v) = extra.strip_prefix("h=") {
                            height = v.parse().unwrap_or(0.0);
                        } else if let Some(v) = extra.strip_prefix("align=") {
                            align = v.to_string();
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
                },
            ],
        };
        let parsed = WorldPlace::parse(&p.to_text()).expect("parse");
        assert_eq!(parsed.source, "freedoom");
        assert_eq!(parsed.world, "worlds/freedoom2/map01");
        assert_eq!(parsed.places.len(), 2);
        assert_eq!(parsed.places[1].asset, "billboards/freedoom2/poss");
        assert!(parsed.places[0].asset.is_empty());
    }
}
