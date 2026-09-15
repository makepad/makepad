//! Portable skin colors: one shared image and affine derived UV remaps.
//! Source materials and corner UVs remain untouched. Periodic gutters preserve
//! linear filtering; the export sampler intentionally does not generate mips.
use crate::{mesh::Context, Error, Limits, Material, Result, Texture};
use std::collections::BTreeMap;

const MAX_ATLAS_SIDE: u32 = 1024;
const MAX_REPEAT_TILES: i32 = 8;
const MAX_REPEAT_COORDINATE: f32 = 16.0;

pub(crate) type UvBounds = BTreeMap<u32, [[f32; 2]; 2]>;
struct Region {
    origin: [u32; 2],
    low: [i32; 2],
    pixel_size: [u32; 2],
    constant: bool,
}
pub(crate) struct Atlas {
    pub png: Vec<u8>,
    size: [u32; 2],
    regions: BTreeMap<u32, Region>,
    whole_image: bool,
}
impl Atlas {
    pub fn remap(&self, material: u32, uv: [f32; 2]) -> [f32; 2] {
        if self.whole_image { return uv; }
        let region = &self.regions[&material];
        std::array::from_fn(|axis| {
            let pixel = if region.constant { 0.5 }
                else { (uv[axis] as f64 - region.low[axis] as f64) * region.pixel_size[axis] as f64 };
            ((region.origin[axis] as f64 + pixel) / self.size[axis] as f64) as f32
        })
    }
}

struct Tile { id: u32, image: Texture, low: [i32; 2], size: [u32; 2], origin: [u32; 2] }
fn decode(material: &Material, limits: &Limits, ctx: &mut Context<'_>) -> Result<Texture> {
    ctx.checkpoint(1)?;
    let mut limits = limits.clone();
    limits.max_texture_bytes = limits.max_texture_bytes.min(limits.mesh.max_bytes / 4);
    let limits = &limits;
    let mut image = if material.base_color_png.is_empty() {
        Texture::solid(1, 1, [255; 3], limits)?
    } else { Texture::from_png(&material.base_color_png, limits)? };
    for row in image.rgb.chunks_mut(image.width as usize * 3) {
        ctx.checkpoint(row.len() as u64)?;
        for pixel in row.chunks_exact_mut(3) {
            for (channel, factor) in pixel.iter_mut().zip(material.color) {
                let encoded = *channel as f64 / 255.0;
                let linear = if encoded <= 0.04045 { encoded / 12.92 }
                    else { ((encoded + 0.055) / 1.055).powf(2.4) };
                let tinted = linear * factor as f64;
                let encoded = if tinted <= 0.0031308 { tinted * 12.92 }
                    else { 1.055 * tinted.powf(1.0 / 2.4) - 0.055 };
                *channel = (encoded.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
    Ok(image)
}

pub(crate) fn bake(materials: &BTreeMap<u32, Material>, ranges: &UvBounds, limits: &Limits, ctx: &mut Context<'_>) -> Result<Atlas> {
    if ranges.is_empty() { return Err(Error::Invalid("skin atlas has no materials")); }
    if ranges.len() > limits.max_materials { return Err(Error::Budget("skin atlas materials")); }
    // With one image there are no neighboring tiles to bleed into. Preserve
    // arbitrary original repeat coordinates and resolution, baking only color.
    if ranges.len() == 1 {
        let id = *ranges.keys().next().unwrap();
        let image = decode(materials.get(&id).ok_or(Error::Invalid("unknown atlas material"))?, limits, ctx)?;
        let png = image.to_png(limits)?;
        ctx.checkpoint(png.len() as u64)?;
        return Ok(Atlas { png, size: [image.width, image.height], regions: BTreeMap::new(), whole_image: true });
    }
    let side = MAX_ATLAS_SIDE.min(limits.max_texture_dimension);
    let mut tiles = Vec::new();
    let mut decoded_bytes = 0usize;
    let mut decode_limits = limits.clone();
    for (&id, bounds) in ranges {
        decode_limits.max_texture_bytes = limits.max_texture_bytes.saturating_sub(decoded_bytes);
        let image = decode(materials.get(&id).ok_or(Error::Invalid("unknown atlas material"))?, &decode_limits, ctx)?;
        decoded_bytes = decoded_bytes.checked_add(image.rgb.len()).ok_or(Error::Budget("skin atlas decoded bytes"))?;
        let constant = image.width == 1 && image.height == 1;
        let mut low = [0; 2];
        let mut repeats = [1; 2];
        if !constant {
            for axis in 0..2 {
                let min = bounds[0][axis]; let max = bounds[1][axis];
                if !min.is_finite() || !max.is_finite() || min.abs() > MAX_REPEAT_COORDINATE || max.abs() > MAX_REPEAT_COORDINATE {
                    return Err(Error::Invalid("multi-material skin UV coordinates exceed bounded repeat domain"));
                }
                low[axis] = min.floor() as i32;
                let high = (max.ceil() as i32).max(low[axis] + 1);
                repeats[axis] = high - low[axis];
                if repeats[axis] > MAX_REPEAT_TILES { return Err(Error::Budget("skin atlas repeat tiles")); }
            }
        }
        let size = [image.width.checked_mul(repeats[0] as u32).and_then(|v|v.checked_add(2)),
            image.height.checked_mul(repeats[1] as u32).and_then(|v|v.checked_add(2))];
        let size = [size[0].ok_or(Error::Budget("skin atlas width"))?, size[1].ok_or(Error::Budget("skin atlas height"))?];
        if size.iter().any(|&v| v > side) { return Err(Error::Budget("skin atlas tile dimensions")); }
        tiles.push(Tile { id, image, low, size, origin: [0; 2] });
    }
    // Material ID order makes the shelf layout deterministic across replays.
    let (mut x, mut y, mut row_height, mut width) = (0u32, 0u32, 0u32, 0u32);
    for tile in &mut tiles {
        ctx.checkpoint(1)?;
        if x + tile.size[0] > side { x = 0; y += row_height; row_height = 0; }
        if y + tile.size[1] > side { return Err(Error::Budget("skin atlas dimensions")); }
        tile.origin = [x, y];
        x += tile.size[0]; row_height = row_height.max(tile.size[1]); width = width.max(x);
    }
    let height = y + row_height;
    let bytes = width as usize * height as usize * 3;
    // Account for atlas pixels/PNG staging plus decoded inputs before allocating.
    if bytes.saturating_mul(4).saturating_add(decoded_bytes) > limits.mesh.max_bytes {
        return Err(Error::Budget("skin atlas working bytes"));
    }
    let mut image = Texture::solid(width, height, [0; 3], limits)?;
    let mut regions = BTreeMap::new();
    for tile in tiles {
        for dy in 0..tile.size[1] {
            ctx.checkpoint(tile.size[0] as u64)?;
            let sy = (dy + tile.image.height - 1) % tile.image.height;
            for dx in 0..tile.size[0] {
                let sx = (dx + tile.image.width - 1) % tile.image.width;
                let src = (sy as usize * tile.image.width as usize + sx as usize) * 3;
                let dst = ((tile.origin[1] + dy) as usize * width as usize + (tile.origin[0] + dx) as usize) * 3;
                image.rgb[dst..dst+3].copy_from_slice(&tile.image.rgb[src..src+3]);
            }
        }
        regions.insert(tile.id, Region { origin: [tile.origin[0]+1, tile.origin[1]+1], low: tile.low,
            pixel_size: [tile.image.width, tile.image.height], constant: tile.image.width == 1 && tile.image.height == 1 });
    }
    let png = image.to_png(limits)?;
    ctx.checkpoint(png.len() as u64)?;
    Ok(Atlas { png, size: [width, height], regions, whole_image: false })
}
