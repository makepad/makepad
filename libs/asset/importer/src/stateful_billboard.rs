//! Stateful camera-facing sprite: named animation clips + native-size frames.
//!
//! Doom lumps are `NAME + letter + rotation` (TROOA1, TROOA2A8). The game
//! picks a state (`walk`, `pain`, …) and a viewing rotation. Frame pixels
//! keep their authored size — they are not forced onto a 128² grid.
//!
//! `rot` on each frame:
//! - `0` — omnidirectional (use for every camera angle)
//! - `1..=facings` — 45° (or 360/`facings`) sectors
//!
//! Rotation `1` is the front: CameraRig yaw `0` (camera at −Z looking at
//! the origin). Increasing yaw walks around the actor toward their left
//! (`1` → `2` → `3` …).
//!
//! `mirrors 8` (Doom / Duke): only rots 1..=5 need to be stored. Facings
//! 6/7/8 are the X-flipped drawings of 4/3/2. A frame line may also end
//! with `flip` when the PNG itself is the mirrored pair (Doom `A2A8`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const MANIFEST_VERSION: u32 = 1;
pub const MAGIC: &str = "stateful-billboard";
pub const CONTENT_TYPE: &str = "text/x-stateful-billboard";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpriteRole {
    Character,
    Weapon,
    Item,
    Effect,
}

impl SpriteRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Character => "character",
            Self::Weapon => "weapon",
            Self::Item => "item",
            Self::Effect => "effect",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "weapon" => Self::Weapon,
            "item" => Self::Item,
            "effect" => Self::Effect,
            _ => Self::Character,
        }
    }
}

/// One authored pixel frame. `rot` 0 = all angles; 1 = front.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpriteFrame {
    pub letter: char,
    pub rot: u8,
    pub w: u32,
    pub h: u32,
    /// Path relative to the manifest file.
    pub file: String,
    /// Draw this PNG X-flipped (Doom `A2A8` second pair).
    pub flip: bool,
}

/// One animation step at a camera facing. `flip` is the X-mirror for
/// Duke/Doom 8-way sides that were not stored as their own tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FacedFrame<'a> {
    pub frame: &'a SpriteFrame,
    pub flip: bool,
}

/// Inclusive-exclusive index range into [`StatefulBillboard::frames`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnimState {
    pub name: String,
    pub first: usize,
    pub last: usize,
    pub r#loop: bool,
    pub fps: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatefulBillboard {
    pub prefix: String,
    pub role: SpriteRole,
    pub preview: String,
    /// Authored facings (`8` for Doom/Duke). `0` means infer from frames.
    pub facings: u8,
    /// `8` = missing 6/7/8 are X-flips of 4/3/2. `0` = no pairing.
    pub mirrors: u8,
    pub states: Vec<AnimState>,
    pub frames: Vec<SpriteFrame>,
}

impl StatefulBillboard {
    pub fn to_text(&self) -> String {
        let mut out = format!(
            "{MAGIC} {MANIFEST_VERSION}\nprefix {}\nrole {}\npreview {}\nfacings {}\n",
            self.prefix,
            self.role.as_str(),
            self.preview,
            self.resolved_facings()
        );
        if self.mirrors >= 8 {
            out.push_str("mirrors 8\n");
        }
        for s in &self.states {
            out.push_str(&format!(
                "state {} {} {} {} {}\n",
                s.name,
                s.first,
                s.last,
                u8::from(s.r#loop),
                s.fps
            ));
        }
        for (i, f) in self.frames.iter().enumerate() {
            out.push_str(&format!(
                "frame {i} {} {} {} {} {}",
                f.letter, f.rot, f.w, f.h, f.file
            ));
            if f.flip {
                out.push_str(" flip");
            }
            out.push('\n');
        }
        out
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut lines = text.lines();
        let header = lines.next().unwrap_or("").trim();
        if !header.starts_with(MAGIC) {
            return Err("not a stateful-billboard".into());
        }
        let mut prefix = String::new();
        let mut role = SpriteRole::Character;
        let mut preview = String::new();
        let mut facings = 0u8;
        let mut mirrors = 0u8;
        let mut states = Vec::new();
        let mut frames = Vec::new();
        for line in lines {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_whitespace();
            match parts.next() {
                Some("prefix") => prefix = parts.next().unwrap_or("").to_ascii_lowercase(),
                Some("role") => role = SpriteRole::parse(parts.next().unwrap_or("")),
                Some("preview") => preview = parts.next().unwrap_or("").to_string(),
                Some("facings") => {
                    facings = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                }
                Some("mirrors") => {
                    mirrors = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                }
                Some("state") => {
                    let name = parts.next().unwrap_or("").to_string();
                    let first = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                    let last = parts.next().and_then(|s| s.parse().ok()).unwrap_or(first);
                    let lp = parts.next().and_then(|s| s.parse::<u8>().ok()).unwrap_or(1) != 0;
                    let fps = parts.next().and_then(|s| s.parse().ok()).unwrap_or(8);
                    if !name.is_empty() && last >= first {
                        states.push(AnimState {
                            name,
                            first,
                            last,
                            r#loop: lp,
                            fps,
                        });
                    }
                }
                Some("frame") => {
                    let _idx = parts.next();
                    let letter = parts
                        .next()
                        .and_then(|s| s.chars().next())
                        .unwrap_or('A')
                        .to_ascii_uppercase();
                    let rot = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
                    let w = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
                    let h = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
                    let file = parts.next().unwrap_or("").to_string();
                    let flip = parts.next().is_some_and(|t| t.eq_ignore_ascii_case("flip"));
                    if !file.is_empty() {
                        frames.push(SpriteFrame {
                            letter,
                            rot,
                            w,
                            h,
                            file,
                            flip,
                        });
                    }
                }
                _ => {}
            }
        }
        if prefix.is_empty() || frames.is_empty() {
            return Err("billboard missing prefix or frames".into());
        }
        if preview.is_empty() {
            preview = states
                .first()
                .map(|s| s.name.clone())
                .unwrap_or_else(|| "idle".into());
        }
        Ok(Self {
            prefix,
            role,
            preview,
            facings,
            mirrors,
            states,
            frames,
        })
    }

    pub fn resolved_facings(&self) -> u8 {
        let authored = self.facings;
        let inferred = self.frames.iter().map(|f| f.rot).max().unwrap_or(0);
        authored.max(inferred).max(1)
    }

    /// CameraRig yaw `0` → rot `1` (front). Increasing yaw walks toward
    /// the actor's left (`2`, `3`, …). `facings == 1` returns `0`.
    pub fn facing_for_yaw(yaw: f32, facings: u8) -> u8 {
        let n = facings.max(1);
        if n <= 1 {
            return 0;
        }
        // Negated so dragging the orbit camera to the actor's left shows
        // rot 2 (their left), matching the stored 1→2→3 table.
        let turn = (-yaw).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
        let sector = (turn * f32::from(n) + 0.5).floor() as i32;
        (sector.rem_euclid(i32::from(n)) as u8) + 1
    }

    pub fn state_frame_range(&self, name: &str) -> std::ops::Range<usize> {
        self.states
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.first..s.last)
            .unwrap_or(0..self.frames.len())
    }

    /// Animation steps for `name` at `facing` (one frame per letter).
    /// Missing 6/7/8 use the stored 4/3/2 drawing with `flip` when
    /// [`Self::mirrors`] is 8.
    pub fn frames_for_state_facing(&self, name: &str, facing: u8) -> Vec<FacedFrame<'_>> {
        let src: Vec<&SpriteFrame> = self
            .state_frame_range(name)
            .filter_map(|i| self.frames.get(i))
            .collect();
        if src.is_empty() {
            return self
                .preview_frames()
                .into_iter()
                .map(|frame| FacedFrame {
                    frame,
                    flip: frame.flip,
                })
                .collect();
        }
        let mut letters: Vec<char> = Vec::new();
        for f in &src {
            if !letters.contains(&f.letter) {
                letters.push(f.letter);
            }
        }
        let mut out = Vec::new();
        for letter in letters {
            let pool: Vec<&SpriteFrame> = src.iter().copied().filter(|f| f.letter == letter).collect();
            if let Some((frame, via_mirror)) = pick_facing_frame(&pool, facing) {
                out.push(FacedFrame {
                    frame,
                    flip: frame.flip ^ (via_mirror && self.mirrors >= 8),
                });
            }
        }
        if out.is_empty() {
            return self
                .frames_for_state(name)
                .into_iter()
                .map(|frame| FacedFrame {
                    frame,
                    flip: frame.flip,
                })
                .collect();
        }
        out
    }

    /// Front-facing (rot 1 or 0) frames of the preview state, else every
    /// front-facing frame in letter order. Native sizes — not a sheet.
    pub fn preview_frames(&self) -> Vec<&SpriteFrame> {
        let range = self
            .states
            .iter()
            .find(|s| s.name == self.preview)
            .map(|s| s.first..s.last);
        let src: Vec<&SpriteFrame> = match range {
            Some(r) => r.filter_map(|i| self.frames.get(i)).collect(),
            None => self.frames.iter().collect(),
        };
        let front: Vec<&SpriteFrame> = src
            .iter()
            .copied()
            .filter(|f| f.rot == 1 || f.rot == 0)
            .collect();
        if front.len() >= 2 {
            return front;
        }
        if src.len() >= 2 {
            return src;
        }
        self.frames.first().into_iter().collect()
    }

    pub fn preview_fps(&self) -> u8 {
        self.states
            .iter()
            .find(|s| s.name == self.preview)
            .map(|s| s.fps)
            .unwrap_or(8)
            .clamp(1, 30)
    }

    /// Front-facing frames of `name`, else every frame in that state's range.
    pub fn frames_for_state(&self, name: &str) -> Vec<&SpriteFrame> {
        let range = self
            .states
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.first..s.last);
        let src: Vec<&SpriteFrame> = match range {
            Some(r) => r.filter_map(|i| self.frames.get(i)).collect(),
            None => return self.preview_frames(),
        };
        let front: Vec<&SpriteFrame> = src
            .iter()
            .copied()
            .filter(|f| f.rot == 1 || f.rot == 0)
            .collect();
        if !front.is_empty() {
            return front;
        }
        if !src.is_empty() {
            return src;
        }
        self.preview_frames()
    }

    pub fn state_fps(&self, name: &str) -> u8 {
        self.states
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.fps)
            .unwrap_or(8)
            .clamp(1, 30)
    }

    pub fn state_loops(&self, name: &str) -> bool {
        self.states
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.r#loop)
            .unwrap_or(true)
    }

    pub fn resolve_frame(&self, manifest: &Path, frame: &SpriteFrame) -> PathBuf {
        manifest
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(&frame.file)
    }
}

/// Doom / Freedoom lump: `TROOA1`, `TROOA2A8`, `MEDIA0`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoomSpriteName {
    pub prefix: String,
    pub pairs: Vec<(char, u8)>,
}

pub fn parse_doom_sprite_name(name: &str) -> Option<DoomSpriteName> {
    let n: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if n.len() < 6 {
        return None;
    }
    let prefix = n[..4].to_ascii_lowercase();
    let rest: Vec<char> = n[4..].chars().collect();
    let mut pairs = Vec::new();
    let mut i = 0;
    while i + 1 < rest.len() {
        let letter = rest[i];
        if !letter.is_ascii_alphabetic() {
            break;
        }
        let Some(rot) = rest[i + 1].to_digit(10) else {
            break;
        };
        pairs.push((letter, rot as u8));
        i += 2;
    }
    if pairs.is_empty() {
        return None;
    }
    Some(DoomSpriteName { prefix, pairs })
}

/// Short library title. Doom 4-letter lumps get their common name;
/// unknown prefixes stay as the uppercase code.
pub fn sprite_title(prefix: &str) -> String {
    let p = prefix.to_ascii_uppercase();
    let name = match p.as_str() {
        "PLAY" => "Player",
        "TROO" => "Imp",
        "SARG" => "Pinky",
        "POSS" => "Zombieman",
        "SPOS" => "Shotgun guy",
        "CPOS" => "Chaingunner",
        "HEAD" => "Cacodemon",
        "BOSS" => "Baron of Hell",
        "BOS2" => "Hell knight",
        "SKUL" => "Lost soul",
        "SPID" => "Spiderdemon",
        "BSPI" => "Arachnotron",
        "CYBR" => "Cyberdemon",
        "VILE" => "Arch-vile",
        "SKEL" => "Revenant",
        "FATT" => "Mancubus",
        "PAIN" => "Pain elemental",
        "KEEN" => "Commander Keen",
        "SSWV" => "Wolfenstein SS",
        "PLYC" | "PLYA" => "Player (alt)",
        "PISG" | "PISF" => "Pistol",
        "SHTG" | "SHTF" | "SHT2" => "Shotgun",
        "CHGG" | "CHGF" => "Chaingun",
        "MISG" | "MISF" => "Rocket launcher",
        "SAWG" => "Chainsaw",
        "PLSG" | "PLSF" => "Plasma rifle",
        "BFGG" | "BFGF" => "BFG",
        "ARM1" => "Green armor",
        "ARM2" => "Blue armor",
        "BON1" => "Health bonus",
        "BON2" => "Armor bonus",
        "SOUL" => "Soulsphere",
        "MEGA" => "Megasphere",
        "PINV" => "Invulnerability",
        "PINS" => "Partial invisibility",
        "PSTR" => "Berserk",
        "PMAP" => "Light amp",
        "PVIS" => "Computer map",
        "SUIT" => "Radiation suit",
        "MEDIA" | "MEDI" => "Medikit",
        "STIM" => "Stimpack",
        "CLIP" => "Clip",
        "AMMO" => "Ammo box",
        "SHEL" => "Shells",
        "SBOX" => "Shell box",
        "ROCK" => "Rocket",
        "BROK" => "Rocket box",
        "CELL" => "Cell",
        "CELP" => "Cell pack",
        "BKEY" => "Blue key",
        "RKEY" => "Red key",
        "YKEY" => "Yellow key",
        "BSKU" => "Blue skull",
        "RSKU" => "Red skull",
        "YSKU" => "Yellow skull",
        "SHOT" => "Shotgun (pickup)",
        "MGUN" => "Chaingun (pickup)",
        "LAUN" => "Rocket launcher (pickup)",
        "PLAS" => "Plasma (pickup)",
        "CSAW" => "Chainsaw (pickup)",
        "BFUG" => "BFG (pickup)",
        "SGN2" => "Super shotgun",
        "BAR1" | "BEXP" => "Barrel",
        "FBXP" => "Explosion",
        "FATB" => "Mancubus fireball",
        "MANF" => "Mancubus flame",
        "BAL3" => "Baron ball",
        "BOSF" => "Spawn cube",
        "TRE1" | "TRE2" => "Tree",
        "SMIT" => "Stalagmite",
        "ELEC" => "Tall tech lamp",
        "COLU" | "COL1" | "COL2" | "COL3" | "COL4" | "COL5" | "COL6" => "Column",
        "CAND" | "CBRA" => "Candlestick",
        "TBLU" | "TGRN" | "TRED" => "Column torch",
        "SMBT" | "SMGT" | "SMRT" => "Short torch",
        "FSKU" => "Floating skull",
        "CEYE" => "Evil eye",
        "POL1" | "POL2" | "POL3" | "POL4" | "POL5" | "POL6" => "Gory pole",
        "GOR1" | "GOR2" | "GOR3" | "GOR4" | "GOR5" => "Hanging gore",
        "HDB1" | "HDB2" | "HDB3" | "HDB4" | "HDB5" | "HDB6" => "Hanging body",
        "POB1" | "POB2" => "Pool of blood",
        "BRS1" => "Brain stem",
        "BAL1" | "BAL2" | "BAL7" => "Fireball",
        "MISL" => "Rocket (proj)",
        "PLSS" | "PLSE" | "APLS" | "APBX" => "Plasma bolt",
        "BFS1" | "BFE1" | "BFE2" => "BFG shot",
        "PUFF" => "Bullet puff",
        "BLUD" => "Blood",
        "TFOG" | "IFOG" => "Teleport fog",
        "BBRN" => "Romero head",
        "APLAYER" => "Duke",
        "LIZTROOP" => "Liztroop",
        "LIZMAN" => "Lizman",
        "PIGCOP" => "Pig cop",
        "BOSS1" => "Battlelord",
        "OCTABRAIN" => "Octabrain",
        "DRONE" => "Drone",
        "COMMANDER" => "Commander",
        "ORGANTIC" => "Turret",
        _ => return format_prefix_fallback(&p),
    };
    name.to_string()
}

fn format_prefix_fallback(p: &str) -> String {
    if p.is_empty() {
        return "Sprite".into();
    }
    p.to_ascii_uppercase()
}

fn title_case(s: &str) -> String {
    let mut out = String::new();
    let mut up = true;
    for c in s.chars() {
        if c == '_' || c == '-' {
            out.push(' ');
            up = true;
        } else if up {
            out.extend(c.to_uppercase());
            up = false;
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

pub fn world_title(key: &str) -> String {
    let stem = key.rsplit('/').next().unwrap_or(key);
    let s = stem.to_ascii_lowercase();
    if s == "start" {
        return "Start".into();
    }
    if let Some(rest) = s.strip_prefix("lq_") {
        return format!("LibreQuake {}", rest.to_ascii_uppercase());
    }
    if s.len() >= 4 && s.as_bytes()[0] == b'e' && s.as_bytes()[2] == b'm' {
        return s.to_ascii_uppercase();
    }
    if s.len() >= 4 && s.as_bytes()[0] == b'e' && s.as_bytes()[2] == b'l' {
        return s.to_ascii_uppercase();
    }
    if let Some(n) = s.strip_prefix("map") {
        return format!("MAP{n}");
    }
    title_case(stem)
}

pub fn mesh_title(key: &str) -> String {
    let stem = key.rsplit('/').next().unwrap_or(key);
    match stem {
        "player" => "Player",
        "soldier" => "Grunt",
        "enforcer" => "Enforcer",
        "ogre" => "Ogre",
        "demon" => "Fiend",
        "shambler" => "Shambler",
        "wizard" => "Scrag",
        "dog" => "Rottweiler",
        "zombie" => "Zombie",
        "knight" => "Knight",
        "hknight" => "Death knight",
        "shalrath" => "Vore",
        "tarbaby" => "Spawn",
        "fish" => "Rotfish",
        "boss" => "Chthon",
        "oldone" => "Shub-Niggurath",
        "v_axe" => "Axe (view)",
        "v_shot" | "v_shot2" => "Shotgun (view)",
        "v_nail" | "v_nail2" => "Nailgun (view)",
        "v_rock" | "v_rock2" => "Rocket (view)",
        "v_light" => "Thunderbolt (view)",
        "g_shot" | "g_shot1" => "Shotgun",
        "g_nail" | "g_nail2" => "Nailgun",
        "g_rock" | "g_rock2" => "Rocket launcher",
        "g_light" => "Thunderbolt",
        other => return title_case(other),
    }
    .to_string()
}

pub fn sprite_role(prefix: &str) -> SpriteRole {
    let p = prefix.to_ascii_uppercase();
    if crate_weapon(&p) {
        SpriteRole::Weapon
    } else if crate_character(&p) {
        SpriteRole::Character
    } else if crate_effect(&p) {
        SpriteRole::Effect
    } else {
        SpriteRole::Item
    }
}

fn crate_weapon(p: &str) -> bool {
    ["PISG", "PISF", "SHTG", "SHTF", "CHGG", "CHGF", "MISG", "MISF", "SAWG", "PLSG", "PLSF", "BFGG", "BFGF"]
        .iter()
        .any(|s| p.starts_with(s))
}

fn crate_character(p: &str) -> bool {
    [
        "PLAY", "TROO", "SARG", "BOSS", "BOS2", "HEAD", "SKUL", "SPOS", "CPOS", "POSS",
        "CYBR", "SPID", "BSPI", "VILE", "SKEL", "FATT", "PAIN", "KEEN", "SSWV", "SKEL",
    ]
    .iter()
    .any(|s| p.starts_with(s))
}

fn crate_effect(p: &str) -> bool {
    ["BAL1", "BAL2", "BAL7", "MISL", "PUFF", "BLUD", "TFOG", "IFOG", "BFE1", "BFE2", "APLS", "APBX"]
        .iter()
        .any(|s| p.starts_with(s))
}

/// Build one actor from (parsed name, png rel path, w, h) lumps.
pub fn assemble(
    prefix: &str,
    lumps: &[(DoomSpriteName, String, u32, u32)],
) -> Option<StatefulBillboard> {
    if lumps.is_empty() {
        return None;
    }
    let role = sprite_role(prefix);
    let mut frames = Vec::new();
    for (parsed, file, w, h) in lumps {
        for (i, &(letter, rot)) in parsed.pairs.iter().enumerate() {
            frames.push(SpriteFrame {
                letter,
                rot,
                w: *w,
                h: *h,
                file: file.clone(),
                flip: i > 0,
            });
        }
    }
    frames.sort_by(|a, b| {
        a.letter
            .cmp(&b.letter)
            .then(a.rot.cmp(&b.rot))
            .then(a.file.cmp(&b.file))
    });
    frames.dedup_by(|a, b| a.letter == b.letter && a.rot == b.rot && a.file == b.file);
    let states = infer_states(&frames, role);
    let preview = states
        .iter()
        .find(|s| s.name == "walk" || s.name == "idle" || s.name == "see")
        .or_else(|| states.first())
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "idle".into());
    let facings = frames.iter().map(|f| f.rot).max().unwrap_or(1);
    Some(StatefulBillboard {
        prefix: prefix.to_ascii_lowercase(),
        role,
        preview,
        facings,
        mirrors: if facings >= 5 { 8 } else { 0 },
        states,
        frames,
    })
}

fn infer_states(frames: &[SpriteFrame], role: SpriteRole) -> Vec<AnimState> {
    let mut letters: Vec<char> = Vec::new();
    for f in frames {
        if !letters.contains(&f.letter) {
            letters.push(f.letter);
        }
    }
    letters.sort();
    let mut letter_range: BTreeMap<char, (usize, usize)> = BTreeMap::new();
    for (i, f) in frames.iter().enumerate() {
        letter_range
            .entry(f.letter)
            .and_modify(|r| {
                r.0 = r.0.min(i);
                r.1 = r.1.max(i + 1);
            })
            .or_insert((i, i + 1));
    }
    let span = |from: char, to: char| -> Option<(usize, usize)> {
        let mut lo = usize::MAX;
        let mut hi = 0;
        for c in letters.iter().copied() {
            if c >= from && c <= to {
                if let Some((a, b)) = letter_range.get(&c) {
                    lo = lo.min(*a);
                    hi = hi.max(*b);
                }
            }
        }
        (lo < hi).then_some((lo, hi))
    };

    let mut states = Vec::new();
    let push = |states: &mut Vec<AnimState>, name: &str, range: Option<(usize, usize)>, lp: bool, fps: u8| {
        if let Some((first, last)) = range {
            if last > first {
                states.push(AnimState {
                    name: name.into(),
                    first,
                    last,
                    r#loop: lp,
                    fps,
                });
            }
        }
    };

    match role {
        SpriteRole::Character if letters.len() >= 4 => {
            push(&mut states, "walk", span('A', 'D'), true, 8);
            push(&mut states, "attack", span('E', 'F'), false, 10);
            push(&mut states, "pain", span('G', 'G'), false, 8);
            push(&mut states, "death", span('H', 'N'), false, 6);
            push(&mut states, "xdeath", span('O', 'R'), false, 8);
            push(&mut states, "raise", span('U', 'Z'), false, 8);
            if states.iter().all(|s| s.name != "walk") {
                push(&mut states, "idle", span(letters[0], letters[0]), true, 6);
            }
        }
        SpriteRole::Weapon => {
            push(&mut states, "ready", span('A', 'A'), true, 6);
            push(&mut states, "fire", span('A', 'G'), false, 12);
            push(&mut states, "flash", span('A', 'B'), false, 16);
        }
        _ => {
            let last = *letters.last().unwrap_or(&'A');
            let first = *letters.first().unwrap_or(&'A');
            push(&mut states, "idle", span(first, last), true, 8);
        }
    }

    if states.is_empty() {
        states.push(AnimState {
            name: "idle".into(),
            first: 0,
            last: frames.len(),
            r#loop: true,
            fps: 8,
        });
    }
    states
}

pub fn sequential_idle(prefix: &str, frames: Vec<SpriteFrame>, role: SpriteRole) -> StatefulBillboard {
    let n = frames.len();
    StatefulBillboard {
        prefix: prefix.to_ascii_lowercase(),
        role,
        preview: "idle".into(),
        facings: frames.iter().map(|f| f.rot).max().unwrap_or(1),
        mirrors: 0,
        states: vec![AnimState {
            name: "idle".into(),
            first: 0,
            last: n,
            r#loop: true,
            fps: 8,
        }],
        frames,
    }
}

/// Second bool is true when the stored drawing is the opposite 8-way pair
/// (caller X-flips it when `mirrors >= 8`).
fn pick_facing_frame<'a>(pool: &[&'a SpriteFrame], facing: u8) -> Option<(&'a SpriteFrame, bool)> {
    if let Some(f) = pool.iter().copied().find(|f| f.rot == facing) {
        return Some((f, false));
    }
    if let Some(f) = pool.iter().copied().find(|f| f.rot == 0) {
        return Some((f, false));
    }
    let mirror = match facing {
        2 => 8,
        8 => 2,
        3 => 7,
        7 => 3,
        4 => 6,
        6 => 4,
        _ => 0,
    };
    if mirror != 0 {
        if let Some(f) = pool.iter().copied().find(|f| f.rot == mirror) {
            return Some((f, true));
        }
    }
    // Circular distance over the actual facings range: hand-authored
    // manifests can carry rot values past 8, which must not underflow.
    let facings = pool
        .iter()
        .map(|f| u16::from(if f.rot == 0 { 1 } else { f.rot }))
        .max()
        .unwrap_or(8)
        .max(8);
    pool.iter()
        .copied()
        .min_by_key(|f| {
            let a = if f.rot == 0 { 1u8 } else { f.rot };
            let d = i16::from(a).abs_diff(i16::from(facing.max(1))) % facings;
            d.min(facings - d)
        })
        .map(|f| (f, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mirrored_doom_name() {
        let n = parse_doom_sprite_name("TROOA2A8").unwrap();
        assert_eq!(n.prefix, "troo");
        assert_eq!(n.pairs, vec![('A', 2), ('A', 8)]);
    }

    #[test]
    fn duke_actor_stems_are_not_doom_lumps() {
        assert!(parse_doom_sprite_name("rotategun").is_none());
        assert!(parse_doom_sprite_name("liztroop").is_none());
        assert!(parse_doom_sprite_name("forceripple").is_none());
        assert!(parse_doom_sprite_name("tile-1405").is_none());
    }

    #[test]
    fn five_view_sheet_mirrors_the_missing_quarters() {
        // Duke viewtype 7 stores 5 views; 6/7/8 are the mirrors of 4/3/2.
        let pool: Vec<SpriteFrame> = (1..=5)
            .map(|rot| SpriteFrame {
                letter: 'A',
                rot,
                w: 8,
                h: 8,
                file: format!("r{rot}.png"),
                flip: false,
            })
            .collect();
        let refs: Vec<&SpriteFrame> = pool.iter().collect();
        assert_eq!(pick_facing_frame(&refs, 1).unwrap().0.rot, 1);
        assert_eq!(pick_facing_frame(&refs, 5).unwrap().0.rot, 5);
        assert_eq!(pick_facing_frame(&refs, 8).unwrap(), {
            let f = &refs[1]; // rot 2
            (*f, true)
        });
        assert!(pick_facing_frame(&refs, 8).unwrap().1);
        assert_eq!(pick_facing_frame(&refs, 8).unwrap().0.rot, 2);
        assert_eq!(pick_facing_frame(&refs, 7).unwrap().0.rot, 3);
        assert_eq!(pick_facing_frame(&refs, 6).unwrap().0.rot, 4);
    }

    #[test]
    fn troo_walk_is_front_a_to_d() {
        let mut lumps = Vec::new();
        for (i, letter) in ['A', 'B', 'C', 'D', 'E', 'G', 'H'].into_iter().enumerate() {
            lumps.push((
                DoomSpriteName {
                    prefix: "troo".into(),
                    pairs: vec![(letter, 1)],
                },
                format!("troo{}1.png", letter.to_ascii_lowercase()),
                40 + i as u32,
                55,
            ));
        }
        let bb = assemble("troo", &lumps).unwrap();
        assert_eq!(bb.role, SpriteRole::Character);
        assert_eq!(bb.preview, "walk");
        let walk = bb.states.iter().find(|s| s.name == "walk").unwrap();
        assert_eq!((walk.first, walk.last), (0, 4));
        assert!(walk.r#loop);
        let prev = bb.preview_frames();
        assert_eq!(prev.len(), 4);
        assert_eq!(prev[0].w, 40);
        assert_eq!(prev[3].w, 43);
        let text = bb.to_text();
        let again = StatefulBillboard::parse(&text).unwrap();
        assert_eq!(again.preview, "walk");
        assert_eq!(again.frames.len(), 7);
        assert!(
            again.states.iter().all(|s| !s.name.starts_with("pose_")),
            "sheet letters are frames, not poses: {:?}",
            again.states.iter().map(|s| s.name.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn sprite_titles_prefer_common_names() {
        assert_eq!(sprite_title("cybr"), "Cyberdemon");
        assert_eq!(sprite_title("cpos"), "Chaingunner");
        assert_eq!(sprite_title("fbxp"), "Explosion");
        assert_eq!(sprite_title("zzzz"), "ZZZZ");
    }
}
