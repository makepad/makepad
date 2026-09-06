//! Shared emission for the four classic tiled-RTS importers.

use crate::billboard_sheet;
use crate::classic_import::{
    encode_png_rgba, tags_for, ClassicAsset, ClassicSource, ConvertTick,
};
use crate::stateful_billboard::{AnimState, SpriteFrame, SpriteRole, StatefulBillboard};
use makepad_asset_data::AssetKind;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

pub mod td;
pub mod d2k;
pub mod ra;
mod ra_templates;
pub mod ts;

pub const CELL_M: f32 = 6.0;
pub const TILE_PX: u32 = 24;
pub const METRES_PER_PIXEL: f32 = 0.25;

pub const CONTRACT_ROLES: &[&str] = &[
    "conyard",
    "power",
    "refinery",
    "silo",
    "barracks",
    "vehicle_factory",
    "aircraft_pad",
    "naval_yard",
    "radar",
    "tech",
    "repair",
    "defense",
    "superweapon",
    "wall",
];

pub type RoleTable = &'static [(&'static str, &'static str)];

pub fn role_for(table: RoleTable, key: &str) -> Option<&'static str> {
    table
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
        .map(|(_, role)| *role)
}

/// Replace the old pack-key production vocabulary on one buildable
/// definition with the cross-pack role vocabulary from contract section 7.
pub fn rewrite_unit_roles(key: &str, line: &str, table: RoleTable) -> String {
    if !line.starts_with("unit ") {
        return line.into();
    }
    let class = line
        .split_whitespace()
        .find_map(|token| token.strip_prefix("class="))
        .unwrap_or("");
    let structure = matches!(class, "structure" | "defense");
    let mut producer_role = None;
    let mut output = Vec::new();
    for token in line.split_whitespace() {
        if let Some(value) = token.strip_prefix("producer=") {
            if !structure {
                let roles = mapped_roles(value, table);
                producer_role = roles.first().copied();
                debug_assert!(roles.iter().all(|role| *role == roles[0]));
            }
        } else if let Some(value) = token.strip_prefix("prereq=") {
            let roles = mapped_roles(value, table);
            output.push(format!("prereq={}", roles.join(",")));
        } else if token.starts_with("role=") || token.starts_with("builds_at=") {
            // The table below is authoritative, including when this helper
            // is applied to an already role-aware source line.
        } else if token.starts_with("deploys=") {
            output.push("deploys=conyard".into());
        } else {
            output.push(token.into());
        }
    }
    if structure {
        output.push(format!(
            "role={}",
            role_for(table, key)
                .unwrap_or_else(|| panic!("missing structure role for converter key {key}"))
        ));
    } else if let Some(role) = producer_role {
        output.push(format!("builds_at={role}"));
    }
    output.join(" ")
}

fn mapped_roles(value: &str, table: RoleTable) -> Vec<&'static str> {
    let mut roles = Vec::new();
    for key in value.split([',', '|']) {
        let role = role_for(table, key)
            .unwrap_or_else(|| panic!("missing prerequisite role for converter key {key}"));
        if !roles.contains(&role) {
            roles.push(role);
        }
    }
    roles
}

pub fn positive_unit_cost(line: &str) -> bool {
    line.split_whitespace()
        .find_map(|token| token.strip_prefix("cost="))
        .and_then(|cost| cost.trim_start_matches('+').parse::<i32>().ok())
        .is_some_and(|cost| cost > 0)
}

pub fn roster_key(source: &str, key: &str) -> String {
    format!("billboards/{source}/{key}")
}

#[derive(Clone, Debug)]
pub struct SpritePixels {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub rot: u8,
}

#[derive(Clone, Debug)]
pub struct SpriteState {
    pub name: &'static str,
    pub first: usize,
    /// Exclusive, matching stateful-billboard v2.
    pub last: usize,
    pub looping: bool,
    pub fps: u8,
}

/// TD/RA walls store N/E/S/W adjacency in the low four frame bits, not
/// animation steps. Keep an indexed bank for the map and a still for the
/// library preview. TD/RA's final 16-frame bank contains destroyed remnants
/// (its isolated-wall frame is empty), not a live damaged wall. Sandbags
/// have 32 frames and no live damage bank; cyclone/brick have 48/64.
/// See EA's TIBERIANDAWN CELL.CPP Wall_Update and ODATA.CPP damage levels.
pub fn wall_states(frame_count: usize) -> Vec<SpriteState> {
    if frame_count == 0 {
        return Vec::new();
    }
    let still = |name, first, last| SpriteState {
        name, first, last, looping: false, fps: 1,
    };
    let mut states = vec![
        still("idle", 0, 1),
        still("adjacency", 0, frame_count.min(16)),
    ];
    if frame_count >= 48 {
        states.push(still("damaged", 16, 17));
        states.push(still("damaged_adjacency", 16, 32));
    }
    states
}

#[derive(Clone, Debug)]
pub struct UnitSpec {
    /// Contract-exact `unit`, `weapon`, and `sound` lines.
    pub manifest_lines: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SpriteSpec {
    pub key: String,
    pub role: &'static str,
    pub facings: u8,
    pub frames: Vec<SpritePixels>,
    pub states: Vec<SpriteState>,
    pub unit: Option<UnitSpec>,
    /// Contract lines not represented by the currently shared writer, such
    /// as `remap` and `footprint`.
    pub manifest_lines: Vec<String>,
    pub tags: Vec<&'static str>,
}

#[derive(Clone, Debug)]
pub struct PreviewDot {
    /// Position in playable-cell coordinates.
    pub x: f32,
    pub y: f32,
    pub rgb: [u8; 3],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreviewCrop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug)]
pub struct WorldSpec {
    pub key: String,
    pub width: u16,
    pub height: u16,
    pub terrain_rgba: Vec<u8>,
    pub grid: Vec<String>,
    pub place_text: String,
    pub roster: Vec<String>,
    pub spawn_text: String,
    pub preview_dots: Vec<PreviewDot>,
    /// Optional source-pixel crop. Cropped previews keep their aspect ratio
    /// in the PNG itself instead of adding a square letterbox.
    pub preview_crop: Option<PreviewCrop>,
    pub tags: Vec<String>,
}

/// Output owner shared by all classic tiled-RTS converters.
pub struct RtsEmitter<'a> {
    staged: &'a Path,
    source: &'static str,
    tile_px: u32,
    metres_per_pixel: f32,
    cell_m: f32,
    assets: Vec<ClassicAsset>,
    keys: BTreeSet<String>,
}

/// The ground of a tiled RTS world as a GLB: ONE flat quad carrying the
/// baked terrain image, prelit (COLOR_0 white plus the lightmap marker) so it
/// renders unlit — contract section 1.
///
/// Public and standalone because a world is not always a FILE: the sandbox
/// generates one in memory at runtime and streams it, and it must be the
/// same quad, the same winding and the same prelit marker as a baked one, or
/// a generated map would light differently from an imported one.
pub fn ground_quad_glb(terrain_png: &[u8], width_m: f32, height_m: f32) -> Vec<u8> {
    let positions = [
        [0.0, 0.0, 0.0],
        [width_m, 0.0, 0.0],
        [width_m, 0.0, height_m],
        [0.0, 0.0, height_m],
    ];
    let normals = [[0.0, 1.0, 0.0]; 4];
    let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let marker_uvs = [[0.0, 0.0]; 4];
    let colors = [[1.0, 1.0, 1.0]; 4];
    let indices = [0, 2, 1, 0, 3, 2];
    makepad_gltf::write_glb_mesh_textured_parts(
        &[makepad_gltf::GlbTexturedPart {
            positions: &positions,
            uvs: &uvs,
            indices: &indices,
            base_color_png: terrain_png,
            normals: Some(&normals),
            base_color_factor: None,
            colors: Some(&colors),
            lightmap_png: Some(terrain_png),
            lightmap_uvs: Some(&marker_uvs),
            detail_png: None,
            detail_scale: [0.0, 0.0],
        }],
        true,
    )
}

impl<'a> RtsEmitter<'a> {
    pub fn new(staged: &'a Path, source: &'static str) -> Result<Self, String> {
        Self::new_scaled(staged, source, TILE_PX, METRES_PER_PIXEL)
    }

    pub fn new_scaled(
        staged: &'a Path,
        source: &'static str,
        tile_px: u32,
        metres_per_pixel: f32,
    ) -> Result<Self, String> {
        if tile_px == 0 || !metres_per_pixel.is_finite() || metres_per_pixel <= 0.0 {
            return Err("RTS emitter scale must be positive".into());
        }
        std::fs::create_dir_all(staged).map_err(|e| format!("create {}: {e}", staged.display()))?;
        Ok(Self {
            staged,
            source,
            tile_px,
            metres_per_pixel,
            cell_m: tile_px as f32 * metres_per_pixel,
            assets: Vec::new(),
            keys: BTreeSet::new(),
        })
    }

    pub fn finish(self) -> Vec<ClassicAsset> {
        self.assets
    }

    /// Paint a playable cell rectangle into one tightly packed RGBA image.
    /// The callback receives absolute source-map cell coordinates and a
    /// fresh 24x24 tile to fill.
    pub fn paint_cell_map(
        bounds: (u16, u16, u16, u16),
        painter: impl FnMut(u16, u16, &mut [u8]),
    ) -> Vec<u8> {
        Self::paint_cell_map_scaled(bounds, TILE_PX, painter)
    }

    pub fn paint_cell_map_scaled(
        bounds: (u16, u16, u16, u16),
        tile_px: u32,
        mut painter: impl FnMut(u16, u16, &mut [u8]),
    ) -> Vec<u8> {
        let (bx, by, width, height) = bounds;
        let image_w = width as usize * tile_px as usize;
        let mut image = vec![0u8; image_w * height as usize * tile_px as usize * 4];
        let mut tile = vec![0u8; (tile_px * tile_px * 4) as usize];
        for y in 0..height {
            for x in 0..width {
                tile.fill(0);
                painter(bx + x, by + y, &mut tile);
                for row in 0..tile_px as usize {
                    let src = row * tile_px as usize * 4;
                    let dst = ((y as usize * tile_px as usize + row) * image_w
                        + x as usize * tile_px as usize)
                        * 4;
                    image[dst..dst + tile_px as usize * 4]
                        .copy_from_slice(&tile[src..src + tile_px as usize * 4]);
                }
            }
        }
        image
    }

    pub fn emit_world(&mut self, world: WorldSpec) -> Result<Vec<u8>, String> {
        if world.grid.len() != world.height as usize
            || world.grid.iter().any(|row| row.len() != world.width as usize)
        {
            return Err(format!("{}: grid dimensions do not match world", world.key));
        }
        let terrain_w = world.width as u32 * self.tile_px;
        let terrain_h = world.height as u32 * self.tile_px;
        if world.terrain_rgba.len() != (terrain_w * terrain_h * 4) as usize {
            return Err(format!("{}: terrain dimensions do not match world", world.key));
        }
        let key = format!("worlds/{}", world.key);
        self.reserve(&key)?;
        let glb_rel = format!("{key}.glb");
        let glb_path = self.staged.join(&glb_rel);
        make_parent(&glb_path)?;

        let terrain_png = encode_png_rgba(&world.terrain_rgba, terrain_w, terrain_h)?;
        let width_m = world.width as f32 * self.cell_m;
        let height_m = world.height as f32 * self.cell_m;
        let glb = ground_quad_glb(&terrain_png, width_m, height_m);
        if !glb.starts_with(b"glTF") {
            return Err(format!("{}: GLB encode failed", world.key));
        }
        std::fs::write(&glb_path, glb).map_err(|e| format!("write {}: {e}", glb_path.display()))?;

        // TODO(contract): replace these two tiny writers with
        // makepad_asset_data::{world_grid, world_place} once their RTS v1
        // facts/row support is present in this checkout.
        std::fs::write(
            glb_path.with_extension("grid"),
            grid_text(&world.grid, self.cell_m),
        )
            .map_err(|e| e.to_string())?;
        std::fs::write(
            glb_path.with_extension("place"),
            place_text_with_roster(&world.place_text, &world.roster),
        )
            .map_err(|e| e.to_string())?;
        std::fs::write(glb_path.with_extension("spawn"), world.spawn_text)
            .map_err(|e| e.to_string())?;

        let (preview, preview_w, preview_h) = preview_image(
            &world.terrain_rgba,
            terrain_w,
            terrain_h,
            world.width,
            world.height,
            &world.preview_dots,
            world.preview_crop,
        );
        let preview_png = encode_png_rgba(&preview, preview_w, preview_h)?;
        let preview_rel = format!("{key}.png");
        std::fs::write(self.staged.join(&preview_rel), &preview_png).map_err(|e| e.to_string())?;
        self.assets.push(ClassicAsset {
            key,
            kind: AssetKind::World,
            rel_path: glb_rel,
            tags: world.tags,
            icon_rel: Some(preview_rel),
        });
        Ok(preview_png)
    }

    pub fn emit_sprite(&mut self, spec: SpriteSpec) -> Result<(), String> {
        if spec.frames.is_empty() {
            return Err(format!("{}: sprite has no frames", spec.key));
        }
        let key = format!("billboards/{}/{}", self.source, spec.key);
        self.reserve(&key)?;
        let manifest_rel = format!("{key}.billboard");
        let manifest = self.staged.join(&manifest_rel);
        make_parent(&manifest)?;
        let dir = manifest.parent().unwrap_or(self.staged);
        let stem = manifest.file_stem().and_then(|s| s.to_str()).unwrap_or("sprite");
        let mut frames = Vec::with_capacity(spec.frames.len());
        for (index, frame) in spec.frames.iter().enumerate() {
            if frame.rgba.len() != (frame.width * frame.height * 4) as usize {
                return Err(format!("{} frame {index}: invalid rgba size", spec.key));
            }
            let file = format!("{stem}__frame_{index:04}.png");
            let png = encode_png_rgba(&frame.rgba, frame.width, frame.height)?;
            std::fs::write(dir.join(&file), png).map_err(|e| e.to_string())?;
            frames.push(SpriteFrame {
                letter: 'A',
                rot: frame.rot,
                w: frame.width,
                h: frame.height,
                file,
                flip: false,
                cell: None,
            });
        }
        let mut billboard = StatefulBillboard {
            prefix: spec.key.clone(),
            role: SpriteRole::Effect,
            preview: spec.states.first().map_or_else(|| "idle".into(), |s| s.name.into()),
            facings: spec.facings,
            mirrors: 0,
            states: spec
                .states
                .iter()
                .map(|state| AnimState {
                    name: state.name.into(),
                    first: state.first,
                    last: state.last,
                    r#loop: state.looping,
                    fps: state.fps,
                })
                .collect(),
            frames,
            sheet: None,
            actor: None,
            weapon: None,
            metres_per_pixel: self.metres_per_pixel,
            ..Default::default()
        };
        let written = billboard_sheet::write_with_sheet(&manifest, &mut billboard)?;
        for path in &written.consumed {
            let _ = std::fs::remove_file(path);
        }
        let mut text = std::fs::read_to_string(&manifest).map_err(|e| e.to_string())?;
        text = text.replacen("role effect\n", &format!("role {}\n", spec.role), 1);
        for line in &spec.manifest_lines {
            text.push_str(line);
            text.push('\n');
        }
        if let Some(unit) = &spec.unit {
            for line in &unit.manifest_lines {
                text.push_str(line);
                text.push('\n');
            }
        }
        std::fs::write(&manifest, text).map_err(|e| e.to_string())?;
        let icon_rel = written.thumb.and_then(|path| {
            path.strip_prefix(self.staged)
                .ok()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
        });
        self.assets.push(ClassicAsset {
            key,
            kind: AssetKind::Billboard,
            rel_path: manifest_rel,
            tags: tags_for(AssetKind::Billboard, &spec.tags),
            icon_rel,
        });
        Ok(())
    }

    pub fn emit_texture(
        &mut self,
        key: &str,
        rgba: &[u8],
        width: u32,
        height: u32,
        tags: &[&str],
    ) -> Result<(), String> {
        self.reserve(key)?;
        let rel = format!("{key}.png");
        let path = self.staged.join(&rel);
        make_parent(&path)?;
        std::fs::write(&path, encode_png_rgba(rgba, width, height)?)
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        self.assets.push(ClassicAsset {
            key: key.into(),
            kind: AssetKind::Texture,
            rel_path: rel.clone(),
            tags: tags_for(AssetKind::Texture, tags),
            icon_rel: Some(rel),
        });
        Ok(())
    }

    /// Emit a UTF-8 source document whose catalog key and required on-disk
    /// extension are not necessarily the same. This is used by the plain UI
    /// and bitmap-font manifests that refer to sibling texture assets.
    pub fn emit_source(
        &mut self,
        key: &str,
        rel: &str,
        text: &str,
        tags: &[&str],
    ) -> Result<(), String> {
        let rel_path = Path::new(rel);
        if rel_path.as_os_str().is_empty()
            || rel_path.is_absolute()
            || rel_path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(format!("invalid staged source path {rel}"));
        }
        self.reserve(key)?;
        let path = self.staged.join(rel_path);
        make_parent(&path)?;
        std::fs::write(&path, text.as_bytes())
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        self.assets.push(ClassicAsset {
            key: key.into(),
            kind: AssetKind::Data,
            rel_path: rel.replace('\\', "/"),
            tags: tags_for(AssetKind::Data, tags),
            icon_rel: None,
        });
        Ok(())
    }

    /// Emit one source-authored Data asset with a same-revision texture.
    /// Bitmap-font metrics are the authority, while their PNG atlas is the
    /// texture file consumed by the runtime.
    pub fn emit_source_with_texture(
        &mut self,
        key: &str,
        source_rel: &str,
        text: &str,
        texture_rel: &str,
        rgba: &[u8],
        width: u32,
        height: u32,
        tags: &[&str],
    ) -> Result<(), String> {
        let source_path = Path::new(source_rel);
        let texture_path = Path::new(texture_rel);
        let valid_rel = |path: &Path| {
            !path.as_os_str().is_empty()
                && !path.is_absolute()
                && path
                    .components()
                    .all(|part| matches!(part, Component::Normal(_)))
        };
        if !valid_rel(source_path) || !valid_rel(texture_path) || source_path == texture_path {
            return Err(format!(
                "invalid staged source/texture paths {source_rel}, {texture_rel}"
            ));
        }
        let expected = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| format!("{key}: texture dimensions overflow"))? as usize;
        if rgba.len() != expected {
            return Err(format!("{key}: invalid texture rgba size"));
        }
        self.reserve(key)?;
        let source_path = self.staged.join(source_path);
        let texture_path = self.staged.join(texture_path);
        make_parent(&source_path)?;
        make_parent(&texture_path)?;
        std::fs::write(&source_path, text.as_bytes())
            .map_err(|e| format!("write {}: {e}", source_path.display()))?;
        std::fs::write(&texture_path, encode_png_rgba(rgba, width, height)?)
            .map_err(|e| format!("write {}: {e}", texture_path.display()))?;
        self.assets.push(ClassicAsset {
            key: key.into(),
            kind: AssetKind::Data,
            rel_path: source_rel.replace('\\', "/"),
            tags: tags_for(AssetKind::Data, tags),
            icon_rel: None,
        });
        Ok(())
    }

    pub fn emit_sfx(
        &mut self,
        key: &str,
        sample_rate: u16,
        channels: u8,
        samples: &[i16],
        tags: &[&str],
    ) -> Result<(), String> {
        self.reserve(key)?;
        let rel = format!("{key}.wav");
        let path = self.staged.join(&rel);
        make_parent(&path)?;
        std::fs::write(&path, pcm16_wav(sample_rate, channels, samples))
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        self.assets.push(ClassicAsset {
            key: key.into(),
            kind: AssetKind::Audio,
            rel_path: rel,
            tags: tags_for(AssetKind::Audio, tags),
            icon_rel: None,
        });
        Ok(())
    }

    pub fn emit_wav(
        &mut self,
        key: &str,
        wav: &[u8],
        tags: &[&str],
    ) -> Result<(), String> {
        if wav.get(..4) != Some(b"RIFF") || wav.get(8..12) != Some(b"WAVE") {
            return Err(format!("{key}: invalid WAVE payload"));
        }
        self.reserve(key)?;
        let rel = format!("{key}.wav");
        let path = self.staged.join(&rel);
        make_parent(&path)?;
        std::fs::write(&path, wav).map_err(|e| format!("write {}: {e}", path.display()))?;
        self.assets.push(ClassicAsset {
            key: key.into(),
            kind: AssetKind::Audio,
            rel_path: rel,
            tags: tags_for(AssetKind::Audio, tags),
            icon_rel: None,
        });
        Ok(())
    }

    fn reserve(&mut self, key: &str) -> Result<(), String> {
        if self.keys.insert(key.to_string()) {
            Ok(())
        } else {
            Err(format!("duplicate staged asset key {key}"))
        }
    }
}

pub fn convert_pack(
    source: ClassicSource,
    pack_dir: &Path,
    staged: &Path,
    on_tick: &mut dyn FnMut(ConvertTick),
) -> Result<Vec<ClassicAsset>, String> {
    match source {
        ClassicSource::Cnc => td::convert(pack_dir, staged, on_tick),
        ClassicSource::RedAlert => ra::convert(pack_dir, staged, on_tick),
        ClassicSource::TiberianSun => ts::convert(pack_dir, staged, on_tick),
        ClassicSource::Dune2000 => d2k::convert(pack_dir, staged, on_tick),
        _ => Err(format!("{} is not a tiled RTS source", source.title())),
    }
}

pub fn cell_to_metres(bounds_x: u16, bounds_y: u16, cell: u16) -> (f32, f32) {
    let x = (cell % 64) as f32;
    let y = (cell / 64) as f32;
    (
        (x - bounds_x as f32 + 0.5) * CELL_M,
        (y - bounds_y as f32 + 0.5) * CELL_M,
    )
}

fn grid_text(rows: &[String], cell_m: f32) -> String {
    let width = rows.first().map_or(0, String::len);
    let mut out = format!(
        "world-grid 1\ncell {cell_m:.1}\norigin 0.0 0.0\nsize {} {}\n",
        width,
        rows.len()
    );
    for (y, row) in rows.iter().enumerate() {
        out.push_str(&format!("row {y} {row}\n"));
    }
    out
}

fn place_text_with_roster(place: &str, roster: &[String]) -> String {
    if roster.is_empty() {
        return place.into();
    }
    let lines = place.lines().collect::<Vec<_>>();
    let insert_at = lines
        .iter()
        .rposition(|line| line.starts_with("house "))
        .map(|index| index + 1)
        .or_else(|| lines.iter().position(|line| line.starts_with("place ")))
        .unwrap_or(lines.len());
    let mut output = String::new();
    for (index, line) in lines.iter().enumerate() {
        if index == insert_at {
            for keys in roster.chunks(24) {
                output.push_str(&format!("roster {}\n", keys.join(" ")));
            }
        }
        output.push_str(line);
        output.push('\n');
    }
    if insert_at == lines.len() {
        for keys in roster.chunks(24) {
            output.push_str(&format!("roster {}\n", keys.join(" ")));
        }
    }
    output
}

fn preview_image(
    rgba: &[u8],
    width: u32,
    height: u32,
    cells_w: u16,
    cells_h: u16,
    dots: &[PreviewDot],
    crop: Option<PreviewCrop>,
) -> (Vec<u8>, u32, u32) {
    let crop = crop.filter(|crop| {
        crop.width > 0
            && crop.height > 0
            && crop.x.checked_add(crop.width).is_some_and(|right| right <= width)
            && crop.y.checked_add(crop.height).is_some_and(|bottom| bottom <= height)
    });
    let source = crop.unwrap_or(PreviewCrop {
        x: 0,
        y: 0,
        width: width.max(1),
        height: height.max(1),
    });
    let scale = (512.0 / source.width.max(1) as f32)
        .min(512.0 / source.height.max(1) as f32);
    let draw_w = (source.width as f32 * scale).round().clamp(1.0, 512.0) as u32;
    let draw_h = (source.height as f32 * scale).round().clamp(1.0, 512.0) as u32;
    let (output_w, output_h, ox, oy) = if crop.is_some() {
        (draw_w, draw_h, 0, 0)
    } else {
        (512, 512, (512 - draw_w) / 2, (512 - draw_h) / 2)
    };
    let mut out = vec![0u8; (output_w * output_h * 4) as usize];
    for px in out.chunks_exact_mut(4) {
        px.copy_from_slice(&[20, 20, 20, 255]);
    }
    for y in 0..draw_h {
        let sy = source.y
            + ((y as u64 * source.height as u64) / draw_h as u64)
                .min(source.height.saturating_sub(1) as u64) as u32;
        for x in 0..draw_w {
            let sx = source.x
                + ((x as u64 * source.width as u64) / draw_w as u64)
                    .min(source.width.saturating_sub(1) as u64) as u32;
            let src = ((sy * width + sx) * 4) as usize;
            let dst = (((oy + y) * output_w + ox + x) * 4) as usize;
            out[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
        }
    }
    for dot in dots {
        let source_x = dot.x / cells_w.max(1) as f32 * width as f32;
        let source_y = dot.y / cells_h.max(1) as f32 * height as f32;
        let px = ox as i32
            + ((source_x - source.x as f32) / source.width as f32 * draw_w as f32) as i32;
        let py = oy as i32
            + ((source_y - source.y as f32) / source.height as f32 * draw_h as f32) as i32;
        for dy in -3..=3 {
            for dx in -3..=3 {
                if dx * dx + dy * dy > 9 {
                    continue;
                }
                let (x, y) = (px + dx, py + dy);
                if !(0..output_w as i32).contains(&x) || !(0..output_h as i32).contains(&y) {
                    continue;
                }
                let at = ((y as u32 * output_w + x as u32) * 4) as usize;
                out[at..at + 4].copy_from_slice(&[dot.rgb[0], dot.rgb[1], dot.rgb[2], 255]);
            }
        }
    }
    (out, output_w, output_h)
}

fn pcm16_wav(sample_rate: u16, channels: u8, samples: &[i16]) -> Vec<u8> {
    let channels = channels.max(1) as u16;
    let data_len = (samples.len() * 2) as u32;
    let byte_rate = sample_rate as u32 * channels as u32 * 2;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&(sample_rate as u32).to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

fn make_parent(path: &Path) -> Result<(), String> {
    let parent: PathBuf = path.parent().unwrap_or_else(|| Path::new(".")).into();
    std::fs::create_dir_all(&parent).map_err(|e| format!("create {}: {e}", parent.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_preview_is_static_and_complete_banks_remain_indexable() {
        assert!(wall_states(0).is_empty());
        for count in [1, 16, 17, 32, 33, 48, 49, 64] {
            let states = wall_states(count);
            assert_eq!((states[0].name, states[0].first, states[0].last), ("idle", 0, 1));
            assert_eq!((states[1].first, states[1].last), (0, count.min(16)));
            assert_eq!(states.len(), if count >= 48 { 4 } else { 2 });
            for state in &states {
                assert!(!state.looping);
                assert!(state.first < state.last && state.last <= count);
            }
            if count >= 48 {
                assert_eq!((states[3].name, states[3].first, states[3].last), ("damaged_adjacency", 16, 32));
            }
        }
    }

    /// Read the real local packs without exporting/reimporting any assets.
    /// CNC_PACKS_ROOT may point at a read-only checkout's local/packs.
    #[test]
    #[ignore = "requires the local TD/RA game archives"]
    fn real_td_ra_wall_metadata_preserves_adjacency_and_damage_banks() {
        use crate::cnc_import::{mix::MixFile, shp::Shp};
        let packs = std::env::var_os("CNC_PACKS_ROOT").map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                let mut root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
                loop {
                    let candidate = root.join("local/packs");
                    if candidate.is_dir() { break candidate; }
                    assert!(root.pop(), "set CNC_PACKS_ROOT to local/packs");
                }
            });
        let mut checked = 0;
        for game in ["cnc", "ra"] {
            let bytes = std::fs::read(packs.join(game).join("conquer.mix")).expect("conquer.mix");
            let mix = MixFile::parse(&bytes).expect("valid MIX");
            for key in ["sbag", "cycl", "brik", "barb", "wood", "fenc"] {
                if game == "cnc" && key == "fenc" { continue; }
                let filename = format!("{}.SHP", key.to_ascii_uppercase());
                let raw = mix.by_name(&filename).unwrap_or_else(|| panic!("{game}/{filename}"));
                let shp = Shp::parse(raw).expect("valid real wall SHP");
                let count = shp.frames().len();
                let expected = match key { "cycl" => 48, "brik" => 64, _ => 32 };
                assert_eq!(count, expected, "{game}/{key}");
                assert!(shp.frames()[0].iter().any(|&pixel| pixel != 0), "healthy isolated wall is visible");
                assert!(shp.frames()[count - 16].iter().all(|&pixel| pixel == 0), "final isolated-wall remnant is empty");
                let states = wall_states(count);
                assert_eq!(states.iter().any(|s| s.name == "damaged_adjacency"), count >= 48,
                    "a 32-frame sandbag must never use its empty remnant as a live damaged wall");
                let mut sheet = StatefulBillboard::default();
                sheet.prefix = key.into();
                sheet.preview = "idle".into();
                sheet.facings = 1;
                sheet.frames = shp.frames().iter().enumerate().map(|(i, pixels)| {
                    assert_eq!(pixels.len(), usize::from(shp.width()) * usize::from(shp.height()));
                    SpriteFrame { letter: 'A', rot: 0, w: shp.width().into(), h: shp.height().into(),
                        file: format!("{key}-{i}.png"), flip: false, cell: None }
                }).collect();
                sheet.states = states.iter().map(|state| AnimState {
                    name: state.name.into(), first: state.first, last: state.last,
                    r#loop: state.looping, fps: state.fps,
                }).collect();
                let parsed = StatefulBillboard::parse(&sheet.to_text()).expect("wall manifest roundtrip");
                assert_eq!(parsed.frames.len(), count, "damage drawings retained");
                assert_eq!(parsed.preview_frames().len(), 1);
                assert!(parsed.states.iter().all(|state| !state.r#loop));
                for (bank, first) in [("adjacency", 0), ("damaged_adjacency", 16)] {
                    if first == 16 && count < 48 { continue; }
                    let frames = parsed.frames_for_state_facing(bank, 0);
                    assert_eq!(frames.len(), 16);
                    for mask in 0..16 {
                        assert_eq!(frames[mask].frame.file, format!("{key}-{}.png", first + mask));
                    }
                }
                println!("WALL_REAL {game}/{key} {}x{} frames={count} healthy=16 damaged={} preview=1 loop=false",
                    shp.width(), shp.height(), if count >= 48 { 16 } else { 0 });
                checked += 1;
            }
        }
        assert_eq!(checked, 11);
    }

    #[test]
    fn cnc_import_cell_metres_respects_playable_origin() {
        assert_eq!(cell_to_metres(10, 20, 20 * 64 + 10), (3.0, 3.0));
        assert_eq!(cell_to_metres(10, 20, 21 * 64 + 12), (15.0, 9.0));
    }

    #[test]
    fn cnc_import_grid_writer_has_one_declared_row_per_input_row() {
        let text = grid_text(&["..#".into(), "trw".into()], CELL_M);
        assert!(text.starts_with("world-grid 1\ncell 6.0\norigin 0.0 0.0\nsize 3 2\n"));
        assert_eq!(text.lines().filter(|line| line.starts_with("row ")).count(), 2);
    }

    #[test]
    fn cnc_import_role_tables_and_emitted_unit_lines_follow_contract() {
        for table in [td::ROLE_TABLE, ra::ROLE_TABLE, ts::ROLE_TABLE, d2k::ROLE_TABLE] {
            assert!(!table.is_empty());
            for &(key, role) in table {
                assert!(!key.is_empty());
                assert!(CONTRACT_ROLES.contains(&role), "{key} maps to invalid role {role}");
            }
        }

        let lines = td::role_test_lines()
            .into_iter()
            .chain(ra::role_test_lines())
            .chain(ts::role_test_lines())
            .chain(d2k::role_test_lines());
        let mut definitions = 0usize;
        for line in lines.filter(|line| line.starts_with("unit ")) {
            definitions += 1;
            assert!(!line.contains("producer="), "legacy producer in {line}");
            let class = field(&line, "class").expect("unit class");
            if matches!(class, "structure" | "defense") {
                let role = field(&line, "role").unwrap_or_else(|| panic!("missing role in {line}"));
                assert!(CONTRACT_ROLES.contains(&role), "invalid structure role in {line}");
            } else {
                let role = field(&line, "builds_at")
                    .unwrap_or_else(|| panic!("missing builds_at in {line}"));
                assert!(CONTRACT_ROLES.contains(&role), "invalid build role in {line}");
            }
            if let Some(prerequisites) = field(&line, "prereq") {
                for role in prerequisites.split(',') {
                    assert!(CONTRACT_ROLES.contains(&role), "invalid prerequisite in {line}");
                }
            }
            if field(&line, "deploys").is_some() {
                assert_eq!(field(&line, "deploys"), Some("conyard"));
            }
        }
        assert!(definitions >= 100, "definitions={definitions}");
    }

    #[test]
    fn cnc_import_roster_writer_follows_houses_and_wraps_at_24_keys() {
        let place = "world-place 1\nhouse GDI color=e8c040 side=gdi\nhouse NOD color=d02020 side=nod\nplace one scenery tree 0 0 0 0\n";
        let roster = (0..25).map(|index| format!("billboards/test/{index}")).collect::<Vec<_>>();
        let written = place_text_with_roster(place, &roster);
        let lines = written.lines().collect::<Vec<_>>();
        assert!(lines[1].starts_with("house "));
        assert!(lines[2].starts_with("house "));
        assert!(lines[3].starts_with("roster "));
        assert!(lines[4].starts_with("roster "));
        assert!(lines[5].starts_with("place "));
        assert_eq!(lines[3].split_whitespace().count(), 25);
        assert_eq!(lines[4].split_whitespace().count(), 2);
    }

    #[test]
    fn cnc_import_diamond_preview_crop_has_no_letterbox_and_keeps_dots() {
        let mut rgba = vec![0u8; 4 * 2 * 4];
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[10, 10, 10, 255]);
        }
        let (preview, width, height) = preview_image(
            &rgba,
            4,
            2,
            4,
            2,
            &[PreviewDot { x: 2.0, y: 1.0, rgb: [255, 0, 0] }],
            Some(PreviewCrop { x: 0, y: 0, width: 4, height: 2 }),
        );
        assert_eq!((width, height), (512, 256));
        assert!(!preview.chunks_exact(4).any(|pixel| pixel == [20, 20, 20, 255]));
        let dot = ((128 * width + 256) * 4) as usize;
        assert_eq!(&preview[dot..dot + 4], &[255, 0, 0, 255]);
    }


    fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
        line.split_whitespace()
            .find_map(|token| token.strip_prefix(&format!("{key}=")))
    }
}
