//! Load-ready model images. At build/publish time every image embedded in a
//! GLB is decoded once, mipped for its material role and encoded to UASTC
//! (Basis Universal, KTX2); the KTX2 replaces the PNG in the GLB itself
//! (`KHR_texture_basisu`) and carries a content id. A load then remaps UASTC
//! to the GPU's block format (ASTC 4x4 is nearly direct, BC7 a fast per-block
//! remap) straight into the upload buffer: no PNG decode, no mips, no hashing.
//! GPUs without either format get RGBA decoded from UASTC.
//!
//! Normal maps are the exception: they are stored as BC5 (X and Y in two
//! independent channels, Z rebuilt in the shader), which beats every UASTC
//! path on the angle that matters and goes to a GPU that samples BC with no
//! transcode at all; others decode it to RGBA (X, Y, 0).

use crate::material_surface::{PixelSemantic, PreparedTexture};
use makepad_draw::ImageBuffer;
use std::collections::HashMap;

const KTX2_MAGIC: [u8; 12] = [0xAB, b'K', b'T', b'X', b' ', b'2', b'0', 0xBB, b'\r', b'\n', 0x1A, b'\n'];

/// Image bytes that are a KTX2 file (the load-ready form).
pub fn is_ktx2(bytes: &[u8]) -> bool { bytes.starts_with(&KTX2_MAGIC) }

/// The builder-assigned content id of a load-ready image (read from its KTX2
/// header, not hashed); `None` when `bytes` is not one or carries no id.
pub fn content_id(bytes: &[u8]) -> Option<u64> {
    if !is_ktx2(bytes) { return None; }
    match makepad_texcomp::rgtc::Bc5View::parse(bytes) {
        Ok(view) => view.content_id,
        Err(_) => makepad_texcomp::basis::UastcView::parse(bytes).ok()?.content_id,
    }
}

/// A load-ready BC5 normal map: its blocks as they are where BC is sampled,
/// else RGBA mips decoded from them. `None` when `bytes` is not BC5.
fn texture_from_bc5(bytes: &[u8]) -> Option<Result<PreparedTexture, String>> {
    let view = makepad_texcomp::rgtc::Bc5View::parse(bytes).ok()?;
    let hash = view.content_id.unwrap_or(bytes.len() as u64 ^ 0x4b54_5832_4243_3500);
    let (width, height, _) = view.levels[0];
    let max_level = view.levels.len() - 1;
    if crate::material_surface::bc5_normal_maps() {
        let blocks = view.levels.iter().flat_map(|l| l.2.iter().copied()).collect();
        return Some(Ok(PreparedTexture { width: width as usize, height: height as usize, data: Vec::new(), max_level, hash,
            compressed: Some((crate::material_surface::GpuCompressedFormat::Bc5, blocks)) }));
    }
    let data = view.levels.iter().flat_map(|&(w, h, blocks)| xy_bgra(makepad_texcomp::rgtc::decode_image(blocks, w, h, 2))).collect();
    Some(Ok(PreparedTexture { width: width as usize, height: height as usize, data, max_level, hash, compressed: None }))
}

/// Decoded BC5 (X, Y bytes) as BGRA words: X in red, Y in green, blue 0.
fn xy_bgra(xy: Vec<u8>) -> impl Iterator<Item = u32> {
    (0..xy.len() / 2).map(move |i| 0xff00_0000 | (xy[i * 2] as u32) << 16 | (xy[i * 2 + 1] as u32) << 8)
}

/// The texture for load-ready image bytes, in the form this device samples:
/// the UASTC mips remapped to the host's compression target, else RGBA mips
/// decoded from UASTC. `None` when `bytes` is not KTX2.
pub fn texture_from_ktx2(bytes: &[u8]) -> Option<Result<PreparedTexture, String>> {
    if !is_ktx2(bytes) { return None; }
    if let Some(texture) = texture_from_bc5(bytes) { return Some(texture); }
    Some((|| {
        let view = makepad_texcomp::basis::UastcView::parse(bytes)?;
        // An image built without an id still dedupes within its own load.
        let hash = view.content_id.unwrap_or(bytes.len() as u64 ^ 0x4b54_5832_0000_0000);
        if let Some(target) = crate::material_surface::compressed_texture_target() {
            return PreparedTexture::from_uastc_view(&view, target, hash).ok_or_else(|| "KTX2 image has no levels".to_string());
        }
        let &(width, height, _) = view.levels.first().ok_or("empty texture")?;
        let mut data = Vec::new();
        for &(w, h, blocks) in &view.levels {
            data.extend(bgra(&makepad_texcomp::uastc::decode_image(blocks, w, h)?));
        }
        Ok(PreparedTexture { width: width as usize, height: height as usize, data, max_level: view.levels.len() - 1, hash, compressed: None })
    })())
}

/// Level 0 of load-ready image bytes as pixels, for consumers that need
/// pixels (atlas builders, sky parts). `None` when `bytes` is not KTX2.
pub fn image_from_ktx2(bytes: &[u8]) -> Option<Result<ImageBuffer, String>> {
    if !is_ktx2(bytes) { return None; }
    Some((|| {
        let mut image = ImageBuffer::default();
        if let Ok(view) = makepad_texcomp::rgtc::Bc5View::parse(bytes) {
            let (width, height, blocks) = view.levels[0];
            (image.width, image.height) = (width as usize, height as usize);
            image.data = xy_bgra(makepad_texcomp::rgtc::decode_image(blocks, width, height, 2)).collect();
            return Ok(image);
        }
        let view = makepad_texcomp::basis::UastcView::parse(bytes)?;
        let &(width, height, blocks) = view.levels.first().ok_or("empty texture")?;
        image.width = width as usize;
        image.height = height as usize;
        image.data = bgra(&makepad_texcomp::uastc::decode_image(blocks, width, height)?).collect();
        Ok(image)
    })())
}

/// Image bytes from a GLB as pixels: KTX2 or PNG. For the synchronous
/// upload paths.
pub fn image_buffer(bytes: &[u8]) -> Result<ImageBuffer, String> {
    match image_from_ktx2(bytes) { Some(result) => result, None => ImageBuffer::from_png(bytes).map_err(|e| format!("{e:?}")) }
}

fn bgra(rgba: &[u8]) -> impl Iterator<Item = u32> + '_ {
    rgba.chunks_exact(4).map(|p| (p[3] as u32) << 24 | (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32)
}

// ── build time ──────────────────────────────────────────────────────────

/// Each image's material role, from the glTF materials (first use wins).
fn image_semantics(doc: &makepad_gltf::GltfDocument) -> HashMap<usize, PixelSemantic> {
    let textures = doc.textures.as_deref().unwrap_or(&[]);
    let source = |index: Option<usize>| index.and_then(|i| textures.get(i)).and_then(|t| t.source);
    let mut out = HashMap::new();
    for material in doc.materials.as_deref().unwrap_or(&[]) {
        let masked = material.alpha_mode.as_deref() == Some("MASK");
        let pbr = material.pbr_metallic_roughness.as_ref();
        let uses = [
            (source(pbr.and_then(|p| p.base_color_texture.as_ref()).map(|t| t.index)), if masked { PixelSemantic::MaskedColor } else { PixelSemantic::Color }),
            (source(pbr.and_then(|p| p.metallic_roughness_texture.as_ref()).map(|t| t.index)), PixelSemantic::Data),
            (source(material.normal_texture.as_ref().map(|t| t.index)), PixelSemantic::Normal),
            (source(material.occlusion_texture.as_ref().map(|t| t.index)), PixelSemantic::Data),
            (source(material.emissive_texture.as_ref().map(|t| t.index)), PixelSemantic::Color),
        ];
        for (image, semantic) in uses { if let Some(image) = image { out.entry(image).or_insert(semantic); } }
    }
    out
}

/// Images sampled with NEAREST magnification anywhere in the document
/// (pixel art: its exact colours matter and block compression softens them).
fn nearest_images(doc: &makepad_gltf::GltfDocument) -> std::collections::HashSet<usize> {
    const NEAREST: u32 = 9728;
    let samplers = doc.samplers.as_deref().unwrap_or(&[]);
    doc.textures.as_deref().unwrap_or(&[]).iter()
        .filter(|t| t.sampler.and_then(|i| samplers.get(i)).is_some_and(|s| s.mag_filter == Some(NEAREST)))
        .filter_map(|t| t.source).collect()
}

/// A palettized PNG (IHDR colour type 3: pixel art, palette colormaps), or
/// one of at most 16 distinct colours (flat maps): these stay PNG, which is
/// smaller than UASTC for them and keeps their colours exact. A truecolour
/// image with more colours goes to Basis even when it has 256 or fewer: a
/// generated two-colour pattern (a gradient between two colours) is smooth
/// art, not pixel art.
fn low_colour(png: &[u8], image: &ImageBuffer) -> bool {
    if png.get(25) == Some(&3) { return true; }
    let mut seen = std::collections::HashSet::new();
    image.data.iter().all(|p| { seen.insert(*p); seen.len() <= 16 })
}

/// Build time: replace every embedded PNG of `glb` with its load-ready KTX2
/// (decoded once, mipped for its material role, UASTC-encoded at `quality`
/// on `threads` cores; normal maps BC5-encoded). Content ids are the PNG's prepared-texture key,
/// computed here once. Images already KTX2, or not PNG, stay as they are,
/// and so do pixel-art images: NEAREST-magnified or low-colour (see
/// [`low_colour`]); those are small and decode cheaply.
/// Returns `None` when nothing changed.
/// One PNG in the load-ready form (pre-mipped UASTC, or BC5 for a normal
/// map, as KTX2 carrying its content id). `None` when the image is low-colour
/// and stays PNG.
pub fn load_ready_image(png: &[u8], semantic: PixelSemantic, quality: makepad_texcomp::uastc::Quality, threads: usize) -> Result<Option<Vec<u8>>, String> {
    let image = crate::renderer::decode_generated_png(png, 4096, 256 * 1024 * 1024)?;
    if low_colour(png, &image) { return Ok(None); }
    let prepared = PreparedTexture::prepare(image, semantic);
    let id = crate::material_surface::prepared_texture_key(png, semantic);
    if matches!(semantic, PixelSemantic::Normal) { return prepared.to_bc5_ktx2(id, threads).map(Some); }
    prepared.to_uastc(false, quality, threads)?.to_ktx2_with(Some(id)).map(Some)
}

/// [`load_ready_image`], remembered by its exact inputs for this process.
/// Models built from one recipe (a castle's thirty parts sharing a stone,
/// a wood and a flagstone material) embed byte-identical pattern PNGs, and
/// each would be re-encoded to UASTC per model. The key is the whole PNG
/// plus semantic and quality, and the encode is deterministic, so a hit
/// returns exactly the bytes the encode would. `threads` only splits the
/// work. Bounded: past 96 MiB of entries the memo starts over.
fn load_ready_image_memo(png: &[u8], semantic: PixelSemantic, quality: makepad_texcomp::uastc::Quality, threads: usize) -> Result<Option<Vec<u8>>, String> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    type Key = (Vec<u8>, u8, u8);
    static MEMO: Mutex<Option<(HashMap<Key, Option<Vec<u8>>>, usize)>> = Mutex::new(None);
    let key: Key = (png.to_vec(), semantic as u8, quality as u8);
    if let Some((map, _)) = MEMO.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        if let Some(hit) = map.get(&key) { return Ok(hit.clone()); }
    }
    let out = load_ready_image(png, semantic, quality, threads)?;
    let mut memo = MEMO.lock().unwrap_or_else(|e| e.into_inner());
    let (map, bytes) = memo.get_or_insert_with(|| (HashMap::new(), 0));
    let cost = key.0.len() + out.as_ref().map_or(0, |k| k.len());
    if *bytes + cost > 96 << 20 { map.clear(); *bytes = 0; }
    *bytes += cost;
    map.insert(key, out.clone());
    Ok(out)
}

pub fn embed_load_ready_images(glb: &[u8], quality: makepad_texcomp::uastc::Quality, threads: usize) -> Result<Option<Vec<u8>>, String> {
    let parsed = makepad_gltf::parse_glb_bytes(glb).map_err(|e| format!("GLB: {e:?}"))?;
    let semantics = image_semantics(&parsed.document);
    let nearest = nearest_images(&parsed.document);
    drop(parsed);
    let mut failure = None;
    let mut changed = false;
    let out = makepad_gltf::embed_ktx2_images(glb, |index, png| {
        if failure.is_some() || !png.starts_with(b"\x89PNG") || nearest.contains(&index) { return None; }
        let semantic = semantics.get(&index).copied().unwrap_or(PixelSemantic::Color);
        match load_ready_image_memo(png, semantic, quality, threads) {
            Ok(Some(ktx)) => { changed = true; Some(ktx) }
            Ok(None) => None,
            Err(error) => { failure = Some(format!("image {index}: {error}")); None }
        }
    }).map_err(|e| format!("GLB rewrite: {e:?}"))?;
    if let Some(error) = failure { return Err(error); }
    Ok(changed.then_some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_draw::Cx;

    fn textured_glb() -> Vec<u8> {
        // A 64x64 gradient with a metallic-roughness map.
        let rgba: Vec<u8> = (0..64 * 64).flat_map(|i| [(i % 64 * 4) as u8, (i / 64 * 4) as u8, 128, 255]).collect();
        let png = Cx::encode_rgba_as_png(64, 64, &rgba).unwrap();
        let mr = Cx::encode_rgba_as_png(64, 64, &[0, 200, 30, 255].repeat(64 * 64)).unwrap();
        makepad_gltf::write_glb_mesh_textured(&makepad_gltf::GlbTexturedMesh {
            positions: &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]], normals: None,
            uvs: &[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]], indices: &[0, 1, 2],
            base_color_png: &png, metallic_roughness_png: Some(&mr), double_sided: false, colors: None,
        })
    }

    /// The builder swaps each PNG for pre-mipped UASTC in the GLB (KTX2,
    /// KHR_texture_basisu); the model still parses, its images come back as
    /// KTX2 with a content id, and they load as full mip chains close to the
    /// PNG they came from. A second pass changes nothing.
    /// The memo returns exactly what a fresh encode produces, for every
    /// semantic, on a first and on a repeated call.
    #[test]
    fn load_ready_memo_matches_a_fresh_encode() {
        let rgba: Vec<u8> = (0..128 * 128).flat_map(|i| [(i % 128 * 2) as u8, (i / 128 * 2) as u8, (i * 7 % 251) as u8, 255]).collect();
        let png = Cx::encode_rgba_as_png(128, 128, &rgba).unwrap();
        for semantic in [PixelSemantic::Color, PixelSemantic::Data, PixelSemantic::Normal, PixelSemantic::MaskedColor] {
            let fresh = load_ready_image(&png, semantic, makepad_texcomp::uastc::Quality::Fast, 3).unwrap();
            assert!(fresh.is_some());
            for _ in 0..2 {
                assert_eq!(load_ready_image_memo(&png, semantic, makepad_texcomp::uastc::Quality::Fast, 1).unwrap(), fresh);
            }
        }
    }

    #[test]
    fn load_ready_images_replace_pngs_and_load_without_decode() {
        let glb = textured_glb();
        let ready = embed_load_ready_images(&glb, makepad_texcomp::uastc::Quality::Fast, 2).unwrap().expect("PNGs converted");
        assert!(embed_load_ready_images(&ready, makepad_texcomp::uastc::Quality::Fast, 2).unwrap().is_none());
        let doc = makepad_gltf::parse_glb_bytes(&ready).unwrap().document;
        // Only images change: nodes (sockets, movers and their extras),
        // samplers, materials, meshes and accessors are kept as they were.
        let before = makepad_gltf::parse_glb_bytes(&glb).unwrap().document;
        // (Primitive attributes are a hash map: compare them in key order.)
        let shape = |d: &makepad_gltf::GltfDocument| {
            let meshes: Vec<_> = d.meshes_slice().iter().flat_map(|m| m.primitives.iter()
                .map(|p| (p.attributes.iter().collect::<std::collections::BTreeMap<_, _>>(), p.indices, p.material, p.mode))).collect();
            format!("{:?}{:?}{:?}{:?}{meshes:?}", d.nodes, d.samplers, d.materials, d.accessors)
        };
        assert_eq!(shape(&doc), shape(&before));
        assert!(doc.extensions_required.as_deref().unwrap_or(&[]).iter().any(|e| e == "KHR_texture_basisu"));
        // The gradient becomes KTX2; the flat metallic-roughness map is
        // low-colour and stays PNG.
        let mimes: Vec<_> = doc.images_slice().iter().map(|i| i.mime_type.as_deref()).collect();
        assert_eq!(mimes, [Some("image/ktx2"), Some("image/png")]);

        let model = crate::model::StaticModel::parse_glb(&ready).unwrap();
        let bytes = model.texture_png.as_deref().or_else(|| model.draw_layers.first().and_then(|l| l.texture_png.as_deref())).expect("base colour");
        assert!(is_ktx2(bytes));
        assert!(content_id(bytes).is_some());
        let texture = texture_from_ktx2(bytes).unwrap().unwrap();
        assert_eq!((texture.width, texture.height, texture.max_level), (64, 64, 6));
        let reference = PreparedTexture::prepare(crate::renderer::decode_generated_png(&Cx::encode_rgba_as_png(64, 64,
            &(0..64 * 64).flat_map(|i| [(i % 64 * 4) as u8, (i / 64 * 4) as u8, 128, 255]).collect::<Vec<u8>>()).unwrap(), 64, 1 << 20).unwrap(), PixelSemantic::Color);
        assert_eq!(texture.data.len(), reference.data.len());
        let worst = texture.data.iter().zip(&reference.data)
            .flat_map(|(a, b)| (0..4).map(move |s| ((a >> (s * 8)) as u8).abs_diff((b >> (s * 8)) as u8)))
            .max().unwrap();
        assert!(worst <= 12, "UASTC round trip within 12 levels, got {worst}");
        // Pixel consumers get level 0 through the PNG entry point.
        let image = crate::renderer::decode_generated_png(bytes, 4096, 1 << 20).unwrap();
        assert_eq!((image.width, image.height), (64, 64));
        // UASTC is 8 bits per texel plus a third for mips, whatever the
        // content (a gradient's PNG is far smaller; a photo's is larger).
        let bound = glb.len() + 2 * (64 * 64 * 4 / 3 + 512);
        assert!(ready.len() <= bound, "GLB {} bytes, bound {bound}", ready.len());
    }

    /// A normal map's load-ready form is BC5 (X, Y): built from the mipped
    /// normals, it uploads as is where BC is sampled and decodes to RGBA
    /// (X, Y, 0) elsewhere, the same texels either way.
    #[test]
    fn normal_maps_are_stored_as_bc5() {
        let n = 64usize;
        let mut image = ImageBuffer::default();
        image.width = n; image.height = n;
        image.data = (0..n * n).map(|i| {
            let (x, y) = ((i % n) as f32 / n as f32 * 6.283, (i / n) as f32 / n as f32 * 6.283);
            let v = [x.sin() * 0.4, y.cos() * 0.4];
            let z = (1.0 - v[0] * v[0] - v[1] * v[1]).sqrt();
            let b = |c: f32| ((c * 0.5 + 0.5) * 255.0).round() as u32;
            0xff00_0000 | b(v[0]) << 16 | b(v[1]) << 8 | b(z)
        }).collect();
        let prepared = PreparedTexture::prepare(image, PixelSemantic::Normal);
        let ktx = prepared.to_bc5_ktx2(0xabc, 2).unwrap();
        assert!(is_ktx2(&ktx));
        assert_eq!(content_id(&ktx), Some(0xabc));
        crate::material_surface::set_bc5_normal_maps(false);
        let decoded = texture_from_ktx2(&ktx).unwrap().unwrap();
        assert_eq!((decoded.width, decoded.height, decoded.max_level, decoded.hash), (64, 64, 6, 0xabc));
        assert_eq!(decoded.data.len(), prepared.data.len());
        let worst = decoded.data.iter().zip(&prepared.data).flat_map(|(a, b)| [((a >> 16) as u8).abs_diff((b >> 16) as u8), ((a >> 8) as u8).abs_diff((b >> 8) as u8)]).max().unwrap();
        assert!(worst <= 8, "BC5 X/Y within 8 levels of the mipped normals, got {worst}");
        assert!(decoded.data.iter().all(|t| t & 0xff == 0 && t >> 24 == 255), "blue 0, opaque: Z is rebuilt in the shader");
        crate::material_surface::set_bc5_normal_maps(true);
        let blocks = texture_from_ktx2(&ktx).unwrap().unwrap();
        crate::material_surface::set_bc5_normal_maps(false);
        let (format, bytes) = blocks.compressed.as_ref().expect("uploaded as BC5");
        assert_eq!(*format, crate::material_surface::GpuCompressedFormat::Bc5);
        let chain: usize = (0..=6).map(|l| makepad_texcomp::rgtc::encoded_len(64 >> l, 64 >> l, 2)).sum();
        assert_eq!((bytes.len(), blocks.bytes()), (chain, chain));
        assert!(blocks.data.is_empty());
        // Pixel consumers get level 0 decoded.
        let level0 = image_from_ktx2(&ktx).unwrap().unwrap();
        assert_eq!(&level0.data[..], &decoded.data[..64 * 64]);
    }

    /// Pixel art stays PNG: a flat or palettized image, and any image a
    /// texture magnifies with NEAREST.
    #[test]
    fn pixel_art_images_stay_png() {
        // Two flat colours (well under 256): the colour map stays PNG, so
        // nothing is left to convert at all.
        let flat = Cx::encode_rgba_as_png(64, 64, &[10, 20, 30, 255].repeat(64 * 64)).unwrap();
        let glb = makepad_gltf::write_glb_mesh_textured(&makepad_gltf::GlbTexturedMesh {
            positions: &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]], normals: None,
            uvs: &[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]], indices: &[0, 1, 2],
            base_color_png: &flat, metallic_roughness_png: Some(&flat), double_sided: false, colors: None,
        });
        assert!(embed_load_ready_images(&glb, makepad_texcomp::uastc::Quality::Fast, 2).unwrap().is_none());
        // A rich image magnified NEAREST stays PNG too.
        let rich = textured_glb();
        let mut nearest = rich.clone();
        let (from, to) = (b"\"magFilter\":9729", b"\"magFilter\":9728");
        let at = nearest.windows(from.len()).position(|w| w == from).expect("fixture has a sampler to switch");
        nearest[at..at + to.len()].copy_from_slice(to);
        assert!(embed_load_ready_images(&nearest, makepad_texcomp::uastc::Quality::Fast, 2).unwrap().is_none());
        // A generated two-colour pattern (a blend between two colours: at
        // most 256 distinct values) is smooth art and goes to Basis.
        let blend: Vec<u8> = (0..64 * 64).flat_map(|i| { let t = ((i % 64) * 4 + (i / 64)) as u32 % 256; [(40 + t * 150 / 255) as u8, (30 + t * 60 / 255) as u8, (20 + t * 30 / 255) as u8, 255] }).collect();
        let png = Cx::encode_rgba_as_png(64, 64, &blend).unwrap();
        let glb = makepad_gltf::write_glb_mesh_textured(&makepad_gltf::GlbTexturedMesh {
            positions: &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]], normals: None,
            uvs: &[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]], indices: &[0, 1, 2],
            base_color_png: &png, metallic_roughness_png: None, double_sided: false, colors: None,
        });
        let ready = embed_load_ready_images(&glb, makepad_texcomp::uastc::Quality::Fast, 2).unwrap().expect("pattern converted");
        let doc = makepad_gltf::parse_glb_bytes(&ready).unwrap().document;
        assert_eq!(doc.images_slice()[0].mime_type.as_deref(), Some("image/ktx2"));
    }
}
