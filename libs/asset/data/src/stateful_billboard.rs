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

use crate::actor_def::{ActorDef, ResourceKind, WeaponDef};
use crate::unit_def::UnitDef;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Version 2: a classic-family character's clips come from its own engine's
/// state table (see [`classic_clips`]). A version-1 manifest carried one
/// letter map for the whole cast; [`StatefulBillboard::parse`] re-derives
/// its clips on the way in, so nothing already published has to be
/// re-imported to die on the right frame.
pub const MANIFEST_VERSION: u32 = 2;
pub const MAGIC: &str = "stateful-billboard";
pub const CONTENT_TYPE: &str = "text/x-stateful-billboard";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpriteRole {
    #[default]
    Character,
    Weapon,
    Item,
    Effect,
    /// A tiled-strategy piece that moves and takes orders.
    Unit,
    /// A tiled-strategy building: immobile, with a cell footprint.
    Structure,
    /// Immobile decoration that blocks its cells.
    Scenery,
    /// A harvestable patch whose frames are richness stages.
    Resource,
}

impl SpriteRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Character => "character",
            Self::Weapon => "weapon",
            Self::Item => "item",
            Self::Effect => "effect",
            Self::Unit => "unit",
            Self::Structure => "structure",
            Self::Scenery => "scenery",
            Self::Resource => "resource",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "weapon" => Self::Weapon,
            "item" => Self::Item,
            "effect" => Self::Effect,
            "unit" => Self::Unit,
            "structure" => Self::Structure,
            "scenery" => Self::Scenery,
            "resource" => Self::Resource,
            _ => Self::Character,
        }
    }

    /// Roles whose artwork lies FLAT on the ground of a tiled map rather
    /// than standing up to face the camera.
    pub fn is_floor_piece(self) -> bool {
        matches!(self, Self::Unit | Self::Structure | Self::Scenery | Self::Resource)
    }
}

/// Uniform-cell layout of a packed sprite sheet: `cols` cells per row, every
/// cell `cell_w`×`cell_h`, frames top-left anchored inside their cell and
/// laid out row-major by cell index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SheetLayout {
    pub cols: u32,
    pub cell_w: u32,
    pub cell_h: u32,
}

impl SheetLayout {
    /// Top-left pixel of `cell`.
    pub fn cell_origin(self, cell: u32) -> (u32, u32) {
        let cols = self.cols.max(1);
        ((cell % cols) * self.cell_w, (cell / cols) * self.cell_h)
    }

    pub fn rows_for(self, cells: u32) -> u32 {
        let cols = self.cols.max(1);
        cells.div_ceil(cols)
    }
}

/// One authored pixel frame. `rot` 0 = all angles; 1 = front.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpriteFrame {
    pub letter: char,
    pub rot: u8,
    pub w: u32,
    pub h: u32,
    /// Path relative to the manifest file. With a packed sheet every frame
    /// names that one sheet PNG.
    pub file: String,
    /// Draw this PNG X-flipped (Doom `A2A8` second pair).
    pub flip: bool,
    /// Cell index inside the packed sheet ([`StatefulBillboard::sheet`]).
    /// `None` on a legacy manifest whose frames are separate PNGs.
    pub cell: Option<u32>,
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

#[derive(Clone, Debug, PartialEq)]
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
    /// Present when every frame lives in one packed sheet PNG beside the
    /// manifest (`sheet <cols> <cell_w> <cell_h>`).
    pub sheet: Option<SheetLayout>,
    /// What this thing IS when a level places it — health, speed, body,
    /// attack, sounds, pickup, burst — carried on the asset itself (see
    /// [`ActorDef::to_manifest`]) so the engine that resolves the artwork by
    /// alias reads the behaviour off the same manifest and never consults a
    /// table keyed by game. Absent on scenery and on manifests written
    /// before the definition rode along.
    pub actor: Option<ActorDef>,
    /// For a `role weapon` sheet: the gun this is the view of.
    pub weapon: Option<WeaponDef>,
    /// World metres one sprite pixel covers when the frame is drawn at its
    /// authored size — the source game's map unit (Doom draws a texel per
    /// map unit, so this is `DOOM_UNIT`), or the Build sprite's own repeat
    /// scale. Declared by the writer so a reader never has to calibrate
    /// pixels against a walker; `0` on manifests written before it existed.
    pub metres_per_pixel: f32,
    /// What this piece IS on a tiled strategy map — cost, hit points,
    /// armour, speed, weapons. Absent on artwork that carries no `unit`
    /// line, which is scenery. See [`unit_def`](crate::unit_def).
    pub unit: Option<UnitDef>,
    /// The neutral owner-colour ramp, in ramp order, as sRGB bytes. A
    /// runtime re-tints exactly these sheet colours to the owning house's
    /// colour; empty means the artwork has no owner colours.
    ///
    /// ```text
    /// remap e8c040 d4ac38 c09830 ...
    /// ```
    pub remap: Vec<[u8; 3]>,
    /// Cells this piece occupies on a tiled map (`footprint <w> <h>`).
    /// `None` = one cell.
    pub footprint: Option<(u32, u32)>,
}

impl Default for StatefulBillboard {
    fn default() -> Self {
        Self {
            prefix: String::new(),
            role: SpriteRole::Character,
            preview: String::new(),
            facings: 0,
            mirrors: 0,
            states: Vec::new(),
            frames: Vec::new(),
            sheet: None,
            actor: None,
            weapon: None,
            metres_per_pixel: 0.0,
            unit: None,
            remap: Vec::new(),
            footprint: None,
        }
    }
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
        if self.metres_per_pixel > 0.0 {
            out.push_str(&format!("metres_per_pixel {:.6}\n", self.metres_per_pixel));
        }
        if let Some(sheet) = self.sheet {
            out.push_str(&format!(
                "sheet {} {} {}\n",
                sheet.cols, sheet.cell_w, sheet.cell_h
            ));
        }
        if let Some((w, h)) = self.footprint {
            out.push_str(&format!("footprint {w} {h}\n"));
        }
        if !self.remap.is_empty() {
            out.push_str("remap");
            for c in &self.remap {
                out.push_str(&format!(" {:02x}{:02x}{:02x}", c[0], c[1], c[2]));
            }
            out.push('\n');
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
        // Trailing tokens are unordered flags; `flip` stays first so a
        // parser that only knows the old format still reads it.
        for (i, f) in self.frames.iter().enumerate() {
            out.push_str(&format!(
                "frame {i} {} {} {} {} {}",
                f.letter, f.rot, f.w, f.h, f.file
            ));
            if f.flip {
                out.push_str(" flip");
            }
            if let Some(cell) = f.cell {
                out.push_str(&format!(" cell {cell}"));
            }
            out.push('\n');
        }
        // The definition lines are written with their resource references
        // already in namespace-relative key form (the importer mapped them
        // when it attached the def), so the writer passes them through.
        let verbatim = |s: &str, _k: ResourceKind| s.to_string();
        if let Some(actor) = &self.actor {
            out.push_str(&actor.to_manifest(&verbatim));
        }
        if let Some(weapon) = &self.weapon {
            out.push_str(&weapon.to_manifest(&verbatim));
        }
        // A strategy definition owns its own `sound` and `weapon` lines, so
        // it is written instead of (never alongside) the shooter pair above.
        if let Some(unit) = &self.unit {
            out.push_str(&unit.to_manifest());
        }
        out
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut lines = text.lines();
        let header = lines.next().unwrap_or("").trim();
        if !header.starts_with(MAGIC) {
            return Err("not a stateful-billboard".into());
        }
        let version: u32 = header[MAGIC.len()..].trim().parse().unwrap_or(1);
        let mut prefix = String::new();
        let mut role = SpriteRole::Character;
        let mut preview = String::new();
        let mut facings = 0u8;
        let mut mirrors = 0u8;
        let mut states = Vec::new();
        let mut frames = Vec::new();
        let mut sheet = None;
        let mut def_lines: Vec<(String, String)> = Vec::new();
        let mut weapon = None;
        let mut metres_per_pixel = 0.0f32;
        // Strategy definitions ride on the same manifest and share the
        // `sound`/`weapon` tags with the shooter definition above, so the
        // lines are collected raw and claimed at the end by whichever
        // definition the manifest actually declared.
        let mut unit_lines: Vec<(String, String)> = Vec::new();
        let mut saw_unit = false;
        let mut remap: Vec<[u8; 3]> = Vec::new();
        let mut footprint: Option<(u32, u32)> = None;
        for line in lines {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_whitespace();
            match parts.next() {
                Some(tag @ ("actor" | "attack" | "sound" | "give" | "explode" | "projectile")) => {
                    def_lines.push((tag.to_string(), line[tag.len()..].trim().to_string()));
                    if tag == "sound" {
                        unit_lines.push((tag.to_string(), line[tag.len()..].trim().to_string()));
                    }
                }
                Some("unit") => {
                    saw_unit = true;
                    unit_lines.push(("unit".to_string(), line["unit".len()..].trim().to_string()));
                }
                Some("footprint") => {
                    let mut num = || parts.next().and_then(|s| s.parse::<u32>().ok());
                    if let (Some(w), Some(h)) = (num(), num()) {
                        if w > 0 && h > 0 {
                            footprint = Some((w, h));
                        }
                    }
                }
                Some("remap") => {
                    remap = parts
                        .filter_map(crate::world_place::parse_hex_rgb)
                        .collect();
                }
                Some("weapon") => {
                    let rest = line["weapon".len()..].trim().to_string();
                    weapon = WeaponDef::from_manifest(&rest);
                    unit_lines.push(("weapon".to_string(), rest));
                }
                Some("prefix") => prefix = parts.next().unwrap_or("").to_ascii_lowercase(),
                Some("role") => role = SpriteRole::parse(parts.next().unwrap_or("")),
                Some("preview") => preview = parts.next().unwrap_or("").to_string(),
                Some("metres_per_pixel") => {
                    metres_per_pixel = parts
                        .next()
                        .and_then(|s| s.parse::<f32>().ok())
                        .filter(|v| v.is_finite() && *v > 0.0)
                        .unwrap_or(0.0);
                }
                Some("facings") => {
                    facings = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                }
                Some("mirrors") => {
                    mirrors = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                }
                Some("sheet") => {
                    let mut num = || parts.next().and_then(|s| s.parse::<u32>().ok());
                    if let (Some(cols), Some(cell_w), Some(cell_h)) = (num(), num(), num()) {
                        if cols > 0 && cell_w > 0 && cell_h > 0 {
                            sheet = Some(SheetLayout { cols, cell_w, cell_h });
                        }
                    }
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
                    // After the file every token is an unordered flag:
                    // `flip`, `cell <n>`, or something a newer writer added
                    // that this reader must ignore. A sheet-only manifest
                    // may drop the file, putting `cell` in its place.
                    let mut file = parts.next().unwrap_or("").to_string();
                    let mut rest: Vec<&str> = parts.collect();
                    if file.eq_ignore_ascii_case("cell") || file.eq_ignore_ascii_case("flip") {
                        rest.insert(0, if file.eq_ignore_ascii_case("cell") { "cell" } else { "flip" });
                        file.clear();
                    }
                    let mut flip = false;
                    let mut cell = None;
                    for (i, token) in rest.iter().enumerate() {
                        if token.eq_ignore_ascii_case("flip") {
                            flip = true;
                        } else if token.eq_ignore_ascii_case("cell") {
                            cell = rest.get(i + 1).and_then(|s| s.parse::<u32>().ok());
                        }
                    }
                    if !file.is_empty() || cell.is_some() {
                        frames.push(SpriteFrame {
                            letter,
                            rot,
                            w,
                            h,
                            file,
                            flip,
                            cell,
                        });
                    }
                }
                _ => {}
            }
        }
        if prefix.is_empty() || frames.is_empty() {
            return Err("billboard missing prefix or frames".into());
        }
        // A version-1 writer filed every classic character under one letter
        // map, which ended an Imp's death on the first frame of its gib
        // burst. The letters themselves were always right; only the clip
        // table was generic. Re-derive it from the actor's own table.
        if version < 2 && role == SpriteRole::Character && classic_clips(&prefix).is_some() {
            states = infer_states(&frames, role, &prefix);
        }
        // Fireball sheets published before projectile presentation existed
        // were valid v2 effects, but their whole A..E lump run was one
        // looping `idle` clip. Upgrade those manifests while reading them:
        // the asset schema, rather than a renderer keyed to a game, owns the
        // distinction between travelling and impact artwork. A freshly
        // imported manifest already carries these names and is left alone.
        if role == SpriteRole::Effect
            && classic_effect_clips(&prefix).is_some()
            && !(states.iter().any(|s| s.name == "fly")
                && states.iter().any(|s| s.name == "burst"))
        {
            states = infer_states(&frames, role, &prefix);
            preview = "fly".into();
        }
        if preview.is_empty() {
            preview = states
                .first()
                .map(|s| s.name.clone())
                .unwrap_or_else(|| "idle".into());
        }
        let refs: Vec<(&str, &str)> = def_lines.iter().map(|(t, r)| (t.as_str(), r.as_str())).collect();
        let actor = ActorDef::from_manifest(&refs);
        let unit = if saw_unit {
            let refs: Vec<(&str, &str)> =
                unit_lines.iter().map(|(t, r)| (t.as_str(), r.as_str())).collect();
            UnitDef::from_manifest(&refs)
        } else {
            None
        };
        // A strategy sheet's `weapon` lines belong to its unit definition,
        // not to a first-person view weapon; claiming both would write the
        // line twice on the way back out.
        if unit.is_some() {
            weapon = None;
        }
        Ok(Self {
            prefix,
            role,
            preview,
            facings,
            mirrors,
            states,
            frames,
            sheet,
            actor,
            weapon,
            metres_per_pixel,
            unit,
            remap,
            footprint,
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

    /// The viewing sector for a body that stands in a world: its own facing
    /// and the bearing to the camera, both as HEADINGS.
    ///
    /// This is the classics' own rule, stated once for every pack that
    /// publishes 8-way artwork rather than once per game. Vanilla Doom
    /// (`r_things.c`) picks
    ///
    /// ```text
    ///     rot = (R_PointToAngle(thing) - thing->angle + (ANG45/2)*9) >> 29
    /// ```
    ///
    /// — with `(ANG45/2)*9` = 202.5° and `>> 29` the 45° bucket, that is
    /// `rot index = round((bearing_to_viewer − facing) / 45°)` in Doom's
    /// anticlockwise compass. Duke's `animatesprites` viewtype-5 branch,
    /// `k = ((ang + 3072 + 128 − getangle(…)) & 2047) >> 8`, is the same
    /// expression in BUILD's clockwise compass, and BUILD's clockwise
    /// compass is what its axis mapping mirrors back. So across families:
    ///
    /// > **rotation `r` is the drawing seen from a camera standing
    /// > `(r−1)·45°` ANTICLOCKWISE of the way the body faces.**
    ///
    /// A heading grows anticlockwise too (`0` faces −Z, `+π/2` faces −X), so
    /// in that convention the sector is one subtraction and no sign per
    /// game. [`Self::facing_for_yaw`] counts the other way — a camera-rig
    /// yaw grows clockwise — and that single mirror is the whole conversion.
    ///
    /// Passing a camera-convention angle as `facing` reflects the body about
    /// the world's X axis: correct due north and south, a HALF TURN due east
    /// and west. That is what made a level's monsters show their backs while
    /// walking straight at the player.
    pub fn facing_for_bearing(facing: f32, bearing_to_camera: f32, facings: u8) -> u8 {
        Self::facing_for_yaw(facing - bearing_to_camera, facings)
    }

    pub fn state_frame_range(&self, name: &str) -> std::ops::Range<usize> {
        self.states
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.first..s.last)
            .unwrap_or(0..self.frames.len())
    }

    /// Animation steps for `name` at `facing`: one frame per letter, and —
    /// for a sheet that packs a whole run under a single letter — one frame
    /// per step of that run. Missing 6/7/8 use the stored 4/3/2 drawing with
    /// `flip` when [`Self::mirrors`] is 8.
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
            // Two conventions share this table. The shooters spell an
            // animation with LETTERS (A, B, C…) and give each letter one
            // drawing per facing. The strategy sheets spell it the other way
            // round: one letter for the whole clip, and each facing carries
            // the WHOLE run of frames back to back (8 facings x 6 walk
            // frames, every one of them letter `A`). Grouping by letter alone
            // therefore collapsed a six-frame walk to its first frame and
            // nothing ever animated. Group by facing as well, and step
            // through the depth the busiest facing actually has: a sheet with
            // one frame per facing is depth 1 and behaves exactly as before.
            let mut groups: Vec<(u8, Vec<&SpriteFrame>)> = Vec::new();
            for f in &pool {
                match groups.iter_mut().find(|(rot, _)| *rot == f.rot) {
                    Some((_, run)) => run.push(f),
                    None => groups.push((f.rot, vec![f])),
                }
            }
            let depth = groups.iter().map(|(_, run)| run.len()).max().unwrap_or(1);
            for step in 0..depth {
                // A facing with a shorter run holds its last drawing rather
                // than dropping out of the clip half way through.
                let sub: Vec<&SpriteFrame> = groups
                    .iter()
                    .filter_map(|(_, run)| run.get(step).or_else(|| run.last()).copied())
                    .collect();
                if let Some((frame, via_mirror)) = pick_facing_frame(&sub, facing) {
                    out.push(FacedFrame {
                        frame,
                        flip: frame.flip ^ (via_mirror && self.mirrors >= 8),
                    });
                }
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

    /// The one packed sheet every frame reads from, if this manifest has one.
    pub fn sheet_file(&self) -> Option<&str> {
        self.sheet?;
        self.frames
            .iter()
            .map(|f| f.file.as_str())
            .find(|f| !f.is_empty())
    }

    /// Where `frame`'s pixels live inside its file: `(x, y, w, h)` in the
    /// packed sheet, or `None` when the file *is* the frame (legacy
    /// per-frame PNGs). `w`/`h` are the authored size, top-left anchored.
    pub fn frame_rect(&self, frame: &SpriteFrame) -> Option<(u32, u32, u32, u32)> {
        let sheet = self.sheet?;
        let cell = frame.cell?;
        if frame.w == 0 || frame.h == 0 || frame.w > sheet.cell_w || frame.h > sheet.cell_h {
            return None;
        }
        let (x, y) = sheet.cell_origin(cell);
        Some((x, y, frame.w, frame.h))
    }

    /// Highest cell index in use, +1 (the number of packed cells).
    pub fn sheet_cells(&self) -> u32 {
        self.frames
            .iter()
            .filter_map(|f| f.cell)
            .max()
            .map_or(0, |m| m + 1)
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

/// The weapon a player HOLDS — the artwork a view model can name.
///
/// Vanilla pairs each held gun with a `…F` MUZZLE FLASH lump (`PISG` with
/// `PISF`) that is drawn over it for the two tics it fires. A flash is an
/// effect, not something to hold: labelling it `weapon` puts it in the
/// answer to "which artwork is the gun in your hands", and a level that
/// picks it arms the player with a puff of light and no gun.
fn crate_weapon(p: &str) -> bool {
    // Fists (PUNG) and the super shotgun (SHT2) are held weapons too — an
    // "item" role made the fist view sprite fall back to the stock mesh.
    if p.starts_with("PUNG") || p.starts_with("SHT2") {
        return true;
    }
    ["PISG", "SHTG", "CHGG", "MISG", "SAWG", "PLSG", "BFGG"]
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
    [
        // The muzzle flashes, paired with the held guns above.
        "PISF", "SHTF", "CHGF", "MISF", "PLSF", "BFGF", //
        "BAL1", "BAL2", "BAL7", "MISL", "PUFF", "BLUD", "TFOG", "IFOG", "BFE1", "BFE2",
        "APLS", "APBX",
    ]
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
                cell: None,
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
    let states = infer_states(&frames, role, prefix);
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
        sheet: None,
        actor: None,
        weapon: None,
        metres_per_pixel: 0.0,
        ..Default::default()
    })
}

/// One clip of a classic-family actor: the letters its engine's state table
/// steps through, whether it cycles, and the cadence those states ran at
/// (35 tics a second in the source; `fps` is the rounded letter rate).
struct ClassicClip {
    name: &'static str,
    from: char,
    to: char,
    looping: bool,
    fps: u8,
}

const fn clip(name: &'static str, from: char, to: char, looping: bool, fps: u8) -> ClassicClip {
    ClassicClip { name, from, to, looping, fps }
}

/// The classic family's own state tables, per sprite prefix: which letters
/// are the walk, the attack, the flinch, the death and the gib burst.
///
/// The source engine hard-codes these per actor (`info.c`), and no two
/// actors agree. The Imp flinches on H and dies I–M with N–U its burst; the
/// Zombieman flinches on G, dies H–L, bursts M–U; the Cacodemon has a single
/// standing frame, bites on B–D and dies G–L; the Baron dies I–O and never
/// bursts. One letter map across the cast is therefore wrong for nearly
/// every member — the old `H–N` death ran the Imp through its pain frame,
/// its five death frames and then the FIRST FRAME OF ITS GIB BURST, and
/// held that: every killed Imp stood as a red splatter for the rest of the
/// level. The last letter of each `death` below is the corpse.
///
/// Doom and Freedoom share these tables exactly — Freedoom replaces the
/// artwork under the same lump names and runs on the same state machine.
fn classic_clips(prefix: &str) -> Option<&'static [ClassicClip]> {
    let p = prefix.to_ascii_lowercase();
    Some(match p.as_str() {
        // Marine.
        "play" => {
            const C: &[ClassicClip] = &[
                clip("walk", 'A', 'D', true, 9),
                clip("attack", 'E', 'F', false, 6),
                clip("pain", 'G', 'G', false, 4),
                clip("death", 'H', 'N', false, 4),
                clip("xdeath", 'O', 'W', false, 7),
            ];
            C
        }
        // Zombieman, Shotgun guy.
        "poss" | "spos" => {
            const C: &[ClassicClip] = &[
                clip("walk", 'A', 'D', true, 4),
                clip("attack", 'E', 'F', false, 4),
                clip("pain", 'G', 'G', false, 6),
                clip("death", 'H', 'L', false, 7),
                clip("xdeath", 'M', 'U', false, 7),
            ];
            C
        }
        // Chaingunner.
        "cpos" => {
            const C: &[ClassicClip] = &[
                clip("walk", 'A', 'D', true, 4),
                clip("attack", 'E', 'F', false, 6),
                clip("pain", 'G', 'G', false, 6),
                clip("death", 'H', 'N', false, 7),
                clip("xdeath", 'O', 'T', false, 7),
            ];
            C
        }
        // Wolfenstein SS.
        "sswv" => {
            const C: &[ClassicClip] = &[
                clip("walk", 'A', 'D', true, 6),
                clip("attack", 'E', 'F', false, 5),
                clip("pain", 'G', 'G', false, 6),
                clip("death", 'H', 'L', false, 7),
                clip("xdeath", 'M', 'U', false, 7),
            ];
            C
        }
        // Imp.
        "troo" => {
            const C: &[ClassicClip] = &[
                clip("walk", 'A', 'D', true, 4),
                clip("attack", 'E', 'G', false, 4),
                clip("pain", 'H', 'H', false, 9),
                clip("death", 'I', 'M', false, 5),
                clip("xdeath", 'N', 'U', false, 7),
            ];
            C
        }
        // Demon / Spectre.
        "sarg" => {
            const C: &[ClassicClip] = &[
                clip("walk", 'A', 'D', true, 9),
                clip("attack", 'E', 'G', false, 4),
                clip("pain", 'H', 'H', false, 9),
                clip("death", 'I', 'N', false, 6),
            ];
            C
        }
        // Cacodemon.
        "head" => {
            const C: &[ClassicClip] = &[
                clip("walk", 'A', 'A', true, 4),
                clip("attack", 'B', 'D', false, 7),
                clip("pain", 'E', 'F', false, 6),
                clip("death", 'G', 'L', false, 4),
            ];
            C
        }
        // Baron of Hell, Hell knight.
        "boss" | "bos2" => {
            const C: &[ClassicClip] = &[
                clip("walk", 'A', 'D', true, 6),
                clip("attack", 'E', 'G', false, 4),
                clip("pain", 'H', 'H', false, 9),
                clip("death", 'I', 'O', false, 4),
            ];
            C
        }
        // Lost soul.
        "skul" => {
            const C: &[ClassicClip] = &[
                clip("walk", 'A', 'B', true, 6),
                clip("attack", 'C', 'D', false, 5),
                clip("pain", 'E', 'E', false, 6),
                clip("death", 'F', 'K', false, 6),
            ];
            C
        }
        // Cyberdemon.
        "cybr" => {
            const C: &[ClassicClip] = &[
                clip("walk", 'A', 'D', true, 6),
                clip("attack", 'E', 'F', false, 6),
                clip("pain", 'G', 'G', false, 4),
                clip("death", 'H', 'P', false, 4),
            ];
            C
        }
        // Spiderdemon.
        "spid" => {
            const C: &[ClassicClip] = &[
                clip("walk", 'A', 'F', true, 6),
                clip("attack", 'G', 'H', false, 9),
                clip("pain", 'I', 'I', false, 6),
                clip("death", 'J', 'S', false, 4),
            ];
            C
        }
        // Arachnotron.
        "bspi" => {
            const C: &[ClassicClip] = &[
                clip("walk", 'A', 'F', true, 6),
                clip("attack", 'G', 'H', false, 9),
                clip("pain", 'I', 'I', false, 6),
                clip("death", 'J', 'P', false, 5),
            ];
            C
        }
        // Arch-vile: its death starts on the same letter it flinches on.
        "vile" => {
            const C: &[ClassicClip] = &[
                clip("walk", 'A', 'F', true, 9),
                clip("attack", 'G', 'P', false, 4),
                clip("pain", 'Q', 'Q', false, 4),
                clip("death", 'Q', 'Z', false, 5),
            ];
            C
        }
        // Revenant: punch G–I, rocket J–K, and it dies from its pain letter.
        "skel" => {
            const C: &[ClassicClip] = &[
                clip("walk", 'A', 'F', true, 9),
                clip("attack", 'G', 'K', false, 5),
                clip("pain", 'L', 'L', false, 4),
                clip("death", 'L', 'Q', false, 5),
            ];
            C
        }
        // Mancubus.
        "fatt" => {
            const C: &[ClassicClip] = &[
                clip("walk", 'A', 'F', true, 4),
                clip("attack", 'G', 'I', false, 5),
                clip("pain", 'J', 'J', false, 6),
                clip("death", 'K', 'T', false, 6),
            ];
            C
        }
        // Pain elemental.
        "pain" => {
            const C: &[ClassicClip] = &[
                clip("walk", 'A', 'C', true, 6),
                clip("attack", 'D', 'F', false, 7),
                clip("pain", 'G', 'G', false, 3),
                clip("death", 'H', 'M', false, 4),
            ];
            C
        }
        // Commander Keen: hangs still, dies B–L, flinches on M.
        "keen" => {
            const C: &[ClassicClip] = &[
                clip("idle", 'A', 'A', true, 4),
                clip("pain", 'M', 'M', false, 4),
                clip("death", 'B', 'L', false, 6),
            ];
            C
        }
        _ => return None,
    })
}

/// Projectile sprite families whose source lumps contain both the looping
/// travelling frames and the one-shot impact in one prefix. Doom's three
/// ordinary monster fireballs all use A/B in flight and C/D/E on contact;
/// treating the whole sheet as a looping `idle` makes an airborne fireball
/// repeatedly explode before it reaches anything.
fn classic_effect_clips(prefix: &str) -> Option<&'static [ClassicClip]> {
    match prefix.to_ascii_lowercase().as_str() {
        "bal1" | "bal2" | "bal7" => {
            const C: &[ClassicClip] = &[
                clip("fly", 'A', 'B', true, 9),
                clip("burst", 'C', 'E', false, 6),
            ];
            Some(C)
        }
        _ => None,
    }
}

fn infer_states(frames: &[SpriteFrame], role: SpriteRole, prefix: &str) -> Vec<AnimState> {
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

    let classic = if role == SpriteRole::Character { classic_clips(prefix) } else { None };
    let classic_effect = if role == SpriteRole::Effect {
        classic_effect_clips(prefix)
    } else {
        None
    };
    match role {
        SpriteRole::Character if classic.is_some() => {
            for c in classic.unwrap_or_default() {
                push(&mut states, c.name, span(c.from, c.to), c.looping, c.fps);
            }
            // An actor whose sheet is missing its whole walk still needs a
            // rest pose to stand in.
            if states.iter().all(|s| s.name != "walk" && s.name != "idle") {
                push(&mut states, "idle", span(letters[0], letters[0]), true, 6);
            }
        }
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
        SpriteRole::Effect if classic_effect.is_some() => {
            for c in classic_effect.unwrap_or_default() {
                push(&mut states, c.name, span(c.from, c.to), c.looping, c.fps);
            }
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
        sheet: None,
        actor: None,
        weapon: None,
        metres_per_pixel: 0.0,
        ..Default::default()
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

    /// The strategy convention: one letter for the whole clip, each facing
    /// carrying its own run of frames. A walk that reads back as one frame
    /// per facing is a unit that slides across the map without moving its
    /// legs — which is exactly what the C&C infantry did.
    #[test]
    fn a_facing_major_run_under_one_letter_keeps_every_step() {
        let mut text = String::from(
            "stateful-billboard 2\nprefix e1\nrole unit\npreview idle\nfacings 8\n\
             metres_per_pixel 0.25\nsheet 8 50 39\n\
             state idle 0 8 1 8\nstate walk 8 56 1 10\n",
        );
        for i in 0..8 {
            text.push_str(&format!("frame {i} A {} 50 39 e1.png cell {i}\n", i + 1));
        }
        for i in 8..56 {
            let rot = (i - 8) / 6 + 1;
            text.push_str(&format!("frame {i} A {rot} 50 39 e1.png cell {i}\n"));
        }
        let bb = StatefulBillboard::parse(&text).expect("a well-formed manifest");
        // Standing is one drawing per facing, as it always was.
        assert_eq!(bb.frames_for_state_facing("idle", 3).len(), 1);
        // Walking is the six frames THAT facing owns, in file order.
        let walk: Vec<u32> = bb
            .frames_for_state_facing("walk", 3)
            .iter()
            .map(|f| f.frame.cell.unwrap_or(0))
            .collect();
        assert_eq!(walk, vec![20, 21, 22, 23, 24, 25]);
        let north: Vec<u32> = bb
            .frames_for_state_facing("walk", 1)
            .iter()
            .map(|f| f.frame.cell.unwrap_or(0))
            .collect();
        assert_eq!(north, vec![8, 9, 10, 11, 12, 13]);
    }

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
                cell: None,
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

    /// A held gun is a weapon; its muzzle flash is not.
    ///
    /// This is the label a level's author queries to find "the gun in your
    /// hands", and both halves of vanilla's pair used to answer it — so a
    /// generated first-person level could arm its player with `PISF`, two
    /// tics of light with no pistol behind them.
    #[test]
    fn a_muzzle_flash_is_an_effect_not_something_to_hold() {
        for held in ["PISG", "SHTG", "CHGG", "MISG", "SAWG", "PLSG", "BFGG"] {
            assert_eq!(sprite_role(held), SpriteRole::Weapon, "{held}");
        }
        for flash in ["PISF", "SHTF", "CHGF", "MISF", "PLSF", "BFGF"] {
            assert_eq!(sprite_role(flash), SpriteRole::Effect, "{flash}");
        }
        // The floor pickups keep answering as items, not as guns to hold.
        for pickup in ["SHOT", "MGUN", "LAUN", "PLAS", "CSAW", "BFUG"] {
            assert_eq!(sprite_role(pickup), SpriteRole::Item, "{pickup}");
        }
    }

    /// Heading of a ground direction — the sim's convention, restated here
    /// so this crate can pin the rule without depending on the sim.
    fn heading(dx: f32, dz: f32) -> f32 {
        (-dx).atan2(-dz)
    }

    /// The pin: a body facing +X, seen by a camera standing on its +X side,
    /// is looking the camera in the face — rotation 1. Reversed selection
    /// (the body's facing handed over in the camera's mirrored convention)
    /// answers 5 here, which is the level walking backwards at you.
    #[test]
    fn a_body_seen_from_the_side_it_faces_shows_its_front() {
        let east = heading(1.0, 0.0);
        assert_eq!(StatefulBillboard::facing_for_bearing(east, east, 8), 1);
        // Straight behind it: the back drawing.
        let west = heading(-1.0, 0.0);
        assert_eq!(StatefulBillboard::facing_for_bearing(east, west, 8), 5);
        // Ninety degrees anticlockwise of "facing east" is north (−Z), which
        // is the body's own LEFT: rotation 3 in every classic's table.
        assert_eq!(StatefulBillboard::facing_for_bearing(east, heading(0.0, -1.0), 8), 3);
        // Its right (south, +Z) is the mirrored partner, 7.
        assert_eq!(StatefulBillboard::facing_for_bearing(east, heading(0.0, 1.0), 8), 7);
    }

    /// Each 45° anticlockwise step of the camera walks the table by exactly
    /// one, from ANY facing — the whole of vanilla's rule.
    #[test]
    fn the_sector_table_steps_once_per_45_degrees_anticlockwise() {
        for facing_deg in [-170.0f32, -90.0, 0.0, 37.0, 90.0, 175.0] {
            let facing = facing_deg.to_radians();
            for step in 0..8u8 {
                let bearing = facing + f32::from(step) * std::f32::consts::FRAC_PI_4;
                assert_eq!(
                    StatefulBillboard::facing_for_bearing(facing, bearing, 8),
                    step + 1,
                    "facing {facing_deg} deg, camera {step} sectors anticlockwise"
                );
            }
        }
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

    /// A full Imp sheet, one front frame per letter A..U, as the WAD ships.
    fn imp_lumps() -> Vec<(DoomSpriteName, String, u32, u32)> {
        ('A'..='U')
            .map(|letter| {
                (
                    DoomSpriteName {
                        prefix: "troo".into(),
                        pairs: vec![(letter, if letter <= 'H' { 1 } else { 0 })],
                    },
                    format!("troo{}.png", letter.to_ascii_lowercase()),
                    40,
                    55,
                )
            })
            .collect()
    }

    fn state<'a>(bb: &'a StatefulBillboard, name: &str) -> &'a AnimState {
        bb.states
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} missing from {:?}", bb.states))
    }

    fn letters_of(bb: &StatefulBillboard, name: &str) -> String {
        let s = state(bb, name);
        (s.first..s.last).map(|i| bb.frames[i].letter).collect()
    }

    #[test]
    fn doom_monster_fireballs_loop_ab_then_burst_cde_once() {
        for prefix in ["bal1", "bal2", "bal7"] {
            let lumps: Vec<_> = ('A'..='E')
                .map(|letter| {
                    (
                        DoomSpriteName {
                            prefix: prefix.into(),
                            pairs: vec![(letter, 0)],
                        },
                        format!("{prefix}{}.png", letter.to_ascii_lowercase()),
                        24,
                        24,
                    )
                })
                .collect();
            let bb = assemble(prefix, &lumps).expect("fireball sheet");
            assert_eq!(bb.role, SpriteRole::Effect, "{prefix}");
            assert_eq!(bb.preview, "fly", "{prefix}");
            assert_eq!(letters_of(&bb, "fly"), "AB", "{prefix}");
            assert!(state(&bb, "fly").r#loop, "{prefix}");
            assert_eq!(letters_of(&bb, "burst"), "CDE", "{prefix}");
            assert!(!state(&bb, "burst").r#loop, "{prefix}");
        }
    }

    #[test]
    fn already_published_fireball_manifest_upgrades_without_reimport() {
        let old = r#"stateful-billboard 2
prefix bal1
role effect
preview idle
facings 1
sheet 5 24 24
state idle 0 5 1 8
frame 0 A 0 24 24 sheet.png cell 0
frame 1 B 0 24 24 sheet.png cell 1
frame 2 C 0 24 24 sheet.png cell 2
frame 3 D 0 24 24 sheet.png cell 3
frame 4 E 0 24 24 sheet.png cell 4
"#;
        let bb = StatefulBillboard::parse(old).expect("legacy v2 fireball");
        assert_eq!(bb.preview, "fly");
        assert_eq!(letters_of(&bb, "fly"), "AB");
        assert!(state(&bb, "fly").r#loop);
        assert_eq!(letters_of(&bb, "burst"), "CDE");
        assert!(!state(&bb, "burst").r#loop);
    }

    /// The bug: one letter map for the whole cast filed the Imp's death as
    /// H–N — its pain frame, its five death frames and the first frame of
    /// its gib burst, which it then held forever. Each classic actor's
    /// clips come from its own state table, so the corpse is the last
    /// death letter and the burst is its own clip.
    #[test]
    fn an_imp_dies_on_m_and_bursts_from_n() {
        let bb = assemble("troo", &imp_lumps()).unwrap();
        assert_eq!(letters_of(&bb, "walk"), "ABCD");
        assert_eq!(letters_of(&bb, "attack"), "EFG");
        assert_eq!(letters_of(&bb, "pain"), "H");
        assert_eq!(letters_of(&bb, "death"), "IJKLM");
        assert_eq!(letters_of(&bb, "xdeath"), "NOPQRSTU");
        assert!(!state(&bb, "death").r#loop);
        assert!(bb.states.iter().all(|s| s.name != "raise"));
        // The header says which table wrote it.
        assert!(bb.to_text().starts_with("stateful-billboard 2\n"));
    }

    /// The cast disagrees on nearly every letter: the table is per actor.
    #[test]
    fn each_classic_actor_keeps_its_own_death_letters() {
        let sheet = |prefix: &str, last: char| -> StatefulBillboard {
            let lumps: Vec<_> = ('A'..=last)
                .map(|letter| {
                    (
                        DoomSpriteName {
                            prefix: prefix.into(),
                            pairs: vec![(letter, 1)],
                        },
                        format!("{prefix}{}.png", letter.to_ascii_lowercase()),
                        40,
                        55,
                    )
                })
                .collect();
            assemble(prefix, &lumps).unwrap()
        };
        let poss = sheet("poss", 'U');
        assert_eq!(letters_of(&poss, "pain"), "G");
        assert_eq!(letters_of(&poss, "death"), "HIJKL");
        assert_eq!(letters_of(&poss, "xdeath"), "MNOPQRSTU");
        let sarg = sheet("sarg", 'N');
        assert_eq!(letters_of(&sarg, "death"), "IJKLMN");
        assert!(sarg.states.iter().all(|s| s.name != "xdeath"));
        let head = sheet("head", 'L');
        assert_eq!(letters_of(&head, "walk"), "A");
        assert_eq!(letters_of(&head, "attack"), "BCD");
        assert_eq!(letters_of(&head, "pain"), "EF");
        assert_eq!(letters_of(&head, "death"), "GHIJKL");
        let boss = sheet("boss", 'O');
        assert_eq!(letters_of(&boss, "death"), "IJKLMNO");
        let vile = sheet("vile", 'Z');
        assert_eq!(letters_of(&vile, "pain"), "Q");
        assert_eq!(letters_of(&vile, "death"), "QRSTUVWXYZ");
    }

    /// Everything already published carries the version-1 clip table. The
    /// parser re-derives a classic character's clips from its letters, so
    /// the running store's Imps die right without a re-import; a manifest
    /// that already says 2 is taken as written.
    #[test]
    fn a_version_one_imp_manifest_is_re_clipped_on_parse() {
        let mut bb = assemble("troo", &imp_lumps()).unwrap();
        // Forge what the old writer put on disk: one generic map.
        bb.states = vec![
            AnimState { name: "walk".into(), first: 0, last: 4, r#loop: true, fps: 8 },
            AnimState { name: "death".into(), first: 7, last: 14, r#loop: false, fps: 6 },
        ];
        let v1 = bb.to_text().replacen("stateful-billboard 2", "stateful-billboard 1", 1);
        let upgraded = StatefulBillboard::parse(&v1).unwrap();
        assert_eq!(letters_of(&upgraded, "death"), "IJKLM");
        assert_eq!(letters_of(&upgraded, "xdeath"), "NOPQRSTU");
        assert_eq!(upgraded.preview, "walk");
        // Written back, it is a version-2 manifest and parses as itself.
        let again = StatefulBillboard::parse(&upgraded.to_text()).unwrap();
        assert_eq!(again.states, upgraded.states);
        // A version-2 manifest is the author's word: hand-edited clips stay.
        let mut authored = upgraded.clone();
        authored.states.retain(|s| s.name != "xdeath");
        let kept = StatefulBillboard::parse(&authored.to_text()).unwrap();
        assert!(kept.states.iter().all(|s| s.name != "xdeath"));
        // A version-1 manifest of something the table does not know keeps
        // whatever its writer said.
        let other = "stateful-billboard 1\nprefix zzzz\nrole character\npreview walk\nfacings 1\nstate walk 0 1 1 8\nframe 0 A 1 4 4 z.png\n";
        let other = StatefulBillboard::parse(other).unwrap();
        assert_eq!(other.states.len(), 1);
    }

    /// The behaviour rides on the artwork's own manifest: attach a class to
    /// an assembled sheet, write it, read it back, and the engine-facing
    /// fields are all there — sounds already as the pack's keys.
    #[test]
    fn a_character_manifest_carries_its_own_definition() {
        let mut bb = assemble("troo", &imp_lumps()).unwrap();
        let key = |s: &str, k: ResourceKind| match k {
            ResourceKind::Sound => format!("sfx/doom1/{s}"),
            ResourceKind::Sprite => format!("billboards/doom1/{s}"),
        };
        let imp = crate::actor_def::doom_actor_def(3001).unwrap();
        // Attached the way the importer does it: keys mapped once, on the
        // way in, so the manifest text is the contract.
        let text = imp.to_manifest(&key);
        let lines: Vec<(&str, &str)> =
            text.lines().filter_map(|l| l.split_once(' ')).collect();
        bb.actor = ActorDef::from_manifest(&lines);
        let again = StatefulBillboard::parse(&bb.to_text()).unwrap();
        let def = again.actor.expect("definition rode along");
        assert_eq!(def.health, 60.0);
        assert_eq!(def.role, crate::actor_def::ActorRole::Monster);
        assert_eq!(def.sounds.sight, "sfx/doom1/dsbgsit1");
        assert_eq!(def.attack.map(|a| a.kind), Some(crate::actor_def::AttackKind::Projectile));
        assert_eq!(again.frames.len(), bb.frames.len());
        // A manifest without the lines is scenery with no opinion.
        let bare = StatefulBillboard::parse(&assemble("troo", &imp_lumps()).unwrap().to_text()).unwrap();
        assert!(bare.actor.is_none());
    }

    #[test]
    fn legacy_manifests_keep_parsing_and_resolve_per_frame_files() {
        // Written before packed sheets existed: no `sheet` header, one PNG
        // per frame, `flip` as the only trailing token.
        let text = "stateful-billboard 1\n\
                    prefix troo\n\
                    role character\n\
                    preview walk\n\
                    facings 8\n\
                    mirrors 8\n\
                    state walk 0 2 1 8\n\
                    frame 0 A 1 40 55 trooa1.png\n\
                    frame 1 A 2 41 55 trooa2a8.png flip\n";
        let bb = StatefulBillboard::parse(text).unwrap();
        assert!(bb.sheet.is_none());
        assert!(bb.frames.iter().all(|f| f.cell.is_none()));
        assert!(bb.frames[1].flip);
        assert_eq!(bb.frame_rect(&bb.frames[1]), None, "no sheet, no cell rect");
        assert_eq!(
            bb.resolve_frame(Path::new("/s/billboards/doom/troo.billboard"), &bb.frames[0]),
            PathBuf::from("/s/billboards/doom/trooa1.png")
        );
    }

    #[test]
    fn sheet_tokens_are_order_free_and_unknown_tokens_are_ignored() {
        // `cell` may arrive before or after `flip`, a sheet-only writer may
        // drop the file, and a newer writer's extra token must not break us.
        let text = "stateful-billboard 1\n\
                    prefix troo\n\
                    role character\n\
                    preview walk\n\
                    sheet 4 40 55\n\
                    lightmap something\n\
                    state walk 0 3 1 8\n\
                    frame 0 A 1 40 55 troo.png cell 0\n\
                    frame 1 A 2 40 55 troo.png cell 1 flip\n\
                    frame 2 A 8 40 55 troo.png flip cell 1 future 7\n";
        let bb = StatefulBillboard::parse(text).unwrap();
        let sheet = bb.sheet.unwrap();
        assert_eq!((sheet.cols, sheet.cell_w, sheet.cell_h), (4, 40, 55));
        assert_eq!(
            bb.frames.iter().map(|f| (f.cell, f.flip)).collect::<Vec<_>>(),
            vec![(Some(0), false), (Some(1), true), (Some(1), true)]
        );
        assert_eq!(bb.frame_rect(&bb.frames[1]), Some((40, 0, 40, 55)));
        assert_eq!(bb.sheet_file(), Some("troo.png"));
        assert_eq!(bb.sheet_cells(), 2);
        // Cell 5 of a 4-wide sheet is row 1, column 1.
        assert_eq!(sheet.cell_origin(5), (40, 55));
        assert_eq!(sheet.rows_for(9), 3);
    }

    #[test]
    fn sprite_titles_prefer_common_names() {
        assert_eq!(sprite_title("cybr"), "Cyberdemon");
        assert_eq!(sprite_title("cpos"), "Chaingunner");
        assert_eq!(sprite_title("fbxp"), "Explosion");
        assert_eq!(sprite_title("zzzz"), "ZZZZ");
    }
}
