//! Post-GLTF AO bake. Every imported 3D model that lands as a GLB goes
//! through this — Kenney kits, flattened Doom/Quake maps, Quake MDLs.
//!
//! Sidecars (`<stem>.aomesh`, `<stem>.ao.png`, optional `<stem>.shadowsdf`)
//! are derived, never source. Fail-closed per mesh: a bake error writes
//! nothing and the caller reports the failure. Fresh sidecars (mtime newer
//! than the GLB) are skipped, same resume rule as `tools/ao_bake`.

use makepad_render::ao_atlas::AoAtlas;
use makepad_render::model::StaticModel;
use makepad_render::shadow_sdf;
use makepad_render::sun::SunLight;
use std::path::{Path, PathBuf};

/// Ceiling on one model's AO texture (matches `tools/ao_bake`).
const MODEL_ATLAS_MAX: usize = 512;
/// World/map GLBs (LibreQuake e0m* is 3–10 MB) make `parse_glb_baked` hang
/// for minutes–hours. Kenney props and Quake MDLs stay well under this.
const AO_MAX_BYTES: u64 = 512 * 1024;
const AO_MAX_TRIANGLES: usize = 16_384;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BakeOutcome {
    Baked,
    SkippedFresh,
    /// Map-sized mesh — atlas already lives in the World GLB.
    SkippedLarge,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BakeStats {
    pub total: usize,
    pub baked: usize,
    pub skipped: usize,
    pub failed: usize,
}

pub fn default_sun() -> SunLight {
    SunLight::default()
}

pub fn collect_glbs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext == "glb" {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

#[cfg(not(target_arch = "wasm32"))]
fn mtime(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn sidecar_fresh_vs(glb: &Path, sidecar: &Path) -> bool {
    matches!((mtime(glb), mtime(sidecar)), (Some(g), Some(s)) if s > g)
}

#[cfg(target_arch = "wasm32")]
pub fn sidecar_fresh_vs(_glb: &Path, _sidecar: &Path) -> bool {
    false
}

/// Copy fresh AO/shadow sidecars from a source tree into `staged` so a
/// library that already ran `ao_bake` does not re-pay the bake.
pub fn seed_ao_from_source(pack_dir: &Path, staged: &Path) {
    for staged_glb in collect_glbs(staged) {
        let Ok(rel) = staged_glb.strip_prefix(staged) else {
            continue;
        };
        let src_glb = pack_dir.join(rel);
        if !src_glb.is_file() {
            continue;
        }
        for ext in ["aomesh", "ao.png", "shadowsdf"] {
            let src = src_glb.with_extension(ext);
            let dst = staged_glb.with_extension(ext);
            if sidecar_fresh_vs(&src_glb, &src) {
                let _ = std::fs::copy(&src, &dst);
            }
        }
    }
}

/// How many independent model bakes may run at once. Same unit as
/// `tools/ao_bake`: one GLB per worker, all cores, work-stealing cursor.
pub fn bake_thread_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 32)
}

/// Bake every GLB under `root`. `on_progress(done, total, current_name)`
/// is called before and after each file. Counts are honest; no ETAs.
pub fn bake_glb_tree(
    root: &Path,
    on_progress: impl FnMut(usize, usize, &str) + Send,
) -> BakeStats {
    bake_glb_tree_ex(root, None, on_progress)
}

/// Same as [`bake_glb_tree`], stopping early when `cancel` is set.
/// Models bake in parallel (one GLB per worker). Sidecar writes are
/// per-file so workers do not share output.
pub fn bake_glb_tree_ex(
    root: &Path,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on_progress: impl FnMut(usize, usize, &str) + Send,
) -> BakeStats {
    let glbs = collect_glbs(root);
    let bake_total = glbs.len();
    if bake_total == 0 {
        return BakeStats::default();
    }
    let sun = default_sun();
    let threads = bake_thread_count().min(bake_total);
    let cursor = std::sync::atomic::AtomicUsize::new(0);
    let baked = std::sync::atomic::AtomicUsize::new(0);
    let skipped = std::sync::atomic::AtomicUsize::new(0);
    let failed = std::sync::atomic::AtomicUsize::new(0);
    let progress = std::sync::Mutex::new(on_progress);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                use std::sync::atomic::Ordering::{Relaxed, SeqCst};
                loop {
                    if cancel.is_some_and(|c| c.load(SeqCst)) {
                        break;
                    }
                    let i = cursor.fetch_add(1, Relaxed);
                    let Some(glb) = glbs.get(i) else {
                        break;
                    };
                    let current = glb
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("model");
                    let done = baked.load(Relaxed) + skipped.load(Relaxed) + failed.load(Relaxed);
                    if let Ok(mut progress) = progress.lock() {
                        progress(done, bake_total, current);
                    }
                    match bake_glb(glb, &sun) {
                        Ok(BakeOutcome::Baked) => {
                            baked.fetch_add(1, Relaxed);
                        }
                        Ok(BakeOutcome::SkippedFresh) | Ok(BakeOutcome::SkippedLarge) => {
                            skipped.fetch_add(1, Relaxed);
                        }
                        Err(_) => {
                            failed.fetch_add(1, Relaxed);
                        }
                    }
                    let done = baked.load(Relaxed) + skipped.load(Relaxed) + failed.load(Relaxed);
                    if let Ok(mut progress) = progress.lock() {
                        progress(done, bake_total, "");
                    }
                }
            });
        }
    });
    BakeStats {
        total: bake_total,
        baked: baked.into_inner(),
        skipped: skipped.into_inner(),
        failed: failed.into_inner(),
    }
}

/// Bake one GLB the same way as `tools/ao_bake --model`.
pub fn bake_glb(glb: &Path, sun: &SunLight) -> Result<BakeOutcome, String> {
    let ext = glb
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "glb" {
        return Err(format!("not a glb: {}", glb.display()));
    }
    let ao_png = glb.with_extension("ao.png");
    let aomesh = glb.with_extension("aomesh");
    let sdf = glb.with_extension("shadowsdf");
    let meta_len = std::fs::metadata(glb).map(|m| m.len()).unwrap_or(0);
    if meta_len > AO_MAX_BYTES {
        return Ok(BakeOutcome::SkippedLarge);
    }
    let ao_fresh = sidecar_fresh_vs(glb, &ao_png) && sidecar_fresh_vs(glb, &aomesh);
    if ao_fresh {
        if !sidecar_fresh_vs(glb, &sdf) {
            if let Ok(bytes) = std::fs::read(glb) {
                if let Ok(model) = StaticModel::parse_glb(&bytes) {
                    if let Some(atlas) = shadow_sdf::bake_model_atlas(&model, sun) {
                        let _ = std::fs::write(&sdf, atlas.to_shadowsdf(0));
                    }
                }
            }
        }
        return Ok(BakeOutcome::SkippedFresh);
    }

    let bytes = std::fs::read(glb).map_err(|e| format!("read {}: {e}", glb.display()))?;
    if !bytes.starts_with(b"glTF") {
        return Err(format!("not a glTF binary: {}", glb.display()));
    }
    // Cheap parse first: a map-sized mesh must not enter the ray bake.
    if let Ok(plain) = StaticModel::parse_glb(&bytes) {
        if plain.triangle_count() > AO_MAX_TRIANGLES {
            return Ok(BakeOutcome::SkippedLarge);
        }
    }
    let mut atlas = AoAtlas::new(MODEL_ATLAS_MAX);
    atlas.fill = true;
    let model = StaticModel::parse_glb_baked(&bytes, &mut atlas)
        .map_err(|e| format!("bake {}: {e}", glb.display()))?;
    std::fs::write(&aomesh, model.to_aomesh())
        .map_err(|e| format!("write {}: {e}", aomesh.display()))?;
    write_gray_png(&ao_png, &atlas.pixels, atlas.size)
        .map_err(|e| format!("write {}: {e}", ao_png.display()))?;
    if let Some(atlas) = shadow_sdf::bake_model_atlas(&model, sun) {
        let _ = std::fs::write(&sdf, atlas.to_shadowsdf(0));
    }
    Ok(BakeOutcome::Baked)
}

fn write_gray_png(path: &Path, pixels: &[u8], size: usize) -> std::io::Result<()> {
    if pixels.len() < size * size {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "AO pixel buffer shorter than size*size",
        ));
    }
    let mut raw = Vec::with_capacity((size + 1) * size);
    for y in 0..size {
        raw.push(0);
        raw.extend_from_slice(&pixels[y * size..y * size + size]);
    }
    let mut zlib = vec![0x78, 0x01];
    let mut off = 0usize;
    while off < raw.len() {
        let take = (raw.len() - off).min(65535);
        let last = off + take == raw.len();
        zlib.push(if last { 0x01 } else { 0x00 });
        let n = take as u16;
        zlib.extend_from_slice(&n.to_le_bytes());
        zlib.extend_from_slice(&(!n).to_le_bytes());
        zlib.extend_from_slice(&raw[off..off + take]);
        off += take;
    }
    let mut s1 = 1u32;
    let mut s2 = 0u32;
    for &b in &raw {
        s1 = (s1 + b as u32) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    zlib.extend_from_slice(&((s2 << 16) | s1).to_be_bytes());

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(size as u32).to_be_bytes());
    ihdr.extend_from_slice(&(size as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    push_png_chunk(&mut out, b"IHDR", &ihdr);
    push_png_chunk(&mut out, b"IDAT", &zlib);
    push_png_chunk(&mut out, b"IEND", &[]);
    std::fs::write(path, out)
}

fn push_png_chunk(out: &mut Vec<u8>, typ: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(typ);
    out.extend_from_slice(data);
    let mut crc_src = Vec::with_capacity(4 + data.len());
    crc_src.extend_from_slice(typ);
    crc_src.extend_from_slice(data);
    out.extend_from_slice(&png_crc(&crc_src).to_be_bytes());
}

fn png_crc(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bake_skips_fresh_sidecars_and_fails_closed_on_non_glb() {
        let tmp = std::env::temp_dir().join(format!(
            "mp_ao_bake_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let glb = tmp.join("prop.glb");
        std::fs::write(&glb, b"glTF\x02\x00\x00\x00").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(glb.with_extension("aomesh"), b"fake-aomesh").unwrap();
        std::fs::write(glb.with_extension("ao.png"), b"fake-ao").unwrap();
        let sun = default_sun();
        assert_eq!(bake_glb(&glb, &sun).unwrap(), BakeOutcome::SkippedFresh);

        let not_glb = tmp.join("notes.txt");
        std::fs::write(&not_glb, b"hello").unwrap();
        let err = bake_glb(&not_glb, &sun).unwrap_err();
        assert!(err.contains("not a glb"), "{err}");

        let bad = tmp.join("broken.glb");
        std::fs::write(&bad, b"not-gltf-bytes").unwrap();
        let err = bake_glb(&bad, &sun).unwrap_err();
        assert!(
            err.contains("not a glTF") || err.contains("bake"),
            "fail-closed: {err}"
        );
        assert!(!bad.with_extension("aomesh").exists());
        assert!(!bad.with_extension("ao.png").exists());

        let huge = tmp.join("map.glb");
        std::fs::write(&huge, vec![0u8; (AO_MAX_BYTES as usize) + 8]).unwrap();
        assert_eq!(bake_glb(&huge, &sun).unwrap(), BakeOutcome::SkippedLarge);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
