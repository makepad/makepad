//! Offline ambient-occlusion pre-baker for the stock model library.
//!
//! Walks a resources tree and bakes one shrink-wrapped AO texture PER MODEL:
//! `<model>.ao.png` beside its `.glb`, plus the `<model>.aomesh` geometry the
//! texture's uvs belong to.
//!
//! # Why per model, not per pack
//!
//! Props draw one call per asset, so a texture per asset adds no binds — and
//! a pack atlas divided a fixed 1024 among up to 40 models, which starved
//! exactly the models that needed it (a whole house on a 160 square). Each
//! model baked alone gets precisely the texels its surface area demands
//! (`MODEL_TEXELS_PER_UNIT`) and not one row more. It also makes every bake
//! independent, which is what lets this tool spread MODELS across cores
//! instead of triangles.
//!
//! # Why offline
//!
//! The bake is hundreds of rays per texel. That is a real startup cost the
//! player would pay every single run for an answer that never changes: the
//! geometry is fixed, the sampler is deterministic, and nothing about a
//! Kenney crate differs between one launch and the next. Baking 4,700 models
//! at load is not a slow startup, it is an impossible one.
//!
//! # Determinism
//!
//! The output must be reproducible or a cached texture cannot be trusted
//! against the mesh it came from. Every input is a file on disk and the
//! sampler takes no clock, no RNG and no thread order, so re-running over an
//! unchanged model produces byte-identical output — asserted by `--verify`.
//!
//! # Evaluator
//!
//! Which engine answers the texels is `--baker lightmapper|bakerboy|aobaker`
//! (mirrors the `AO_BAKER` env var; `lightmapper` is the default) — see
//! [`makepad_render::ao_atlas::AoBakerKind`].
//!
//! # Shadow-SDF sidecars
//!
//! Beside the AO pair, every model also gets its silhouette-SDF shadow atlas
//! (`shadow_sdf.rs` — the dynamic-caster tier) baked offline into
//! `<model>.shadowsdf`, and every glb that parses as a gait RIG additionally
//! gets `<file>.glb.shadowsdf` keyed by rest hash (the `.skinao` scheme).
//! The runtime prefers these over its own off-thread first-sight bake — the
//! same "answer never changes, stop paying for it at launch" argument as the
//! AO, at ~24 ms per rig. The bake projects at the SUN's elevation, so the
//! sun is an input: `--sun x,y,z` (default: the sandbox worlds' standing
//! `0.55,0.62,0.56`). The runtime rejects a sidecar baked for a different
//! sun and falls back to its live bake.
//!
//! # Usage
//!
//! ```text
//! ao-bake <resources-dir> [--verify] [--pack NAME] [--quiet] [--baker NAME]
//!         [--lm-strength S] [--lm-gamma G] [--sun X,Y,Z]
//! ao-bake --model <file.glb> [--baker NAME] [--lm-strength S] [--lm-gamma G] [--sun X,Y,Z]
//! ```

use makepad_draw::{vec3f, Vec3f};
use makepad_render::ao_atlas::AoAtlas;
use makepad_render::model::StaticModel;
use makepad_render::shadow_sdf;
use makepad_render::skin::SkinnedModel;
use makepad_render::sun::SunLight;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Ceiling on ONE model's texture, texels per side.
///
/// Half `ATLAS_MAX`: Quest is the target, and at the shrink-wrap density the
/// only models that hit even this are the big detailed ones (houses, ships) —
/// exactly the ones a player never inspects from a hand's width away. The
/// scaler degrades their density gracefully rather than failing them.
const MODEL_ATLAS_MAX: usize = 512;

/// The sun the shadow-SDF sidecars bake against when `--sun` is not given:
/// the standing sun of the sandbox worlds (sandbox_view.rs SunConfig). Only
/// its elevation enters the bake (`SunLight::shadow_len_per_unit`), and the
/// runtime falls back to a live bake when its sun disagrees.
const DEFAULT_SDF_SUN: Vec3f = Vec3f { x: 0.55, y: 0.62, z: 0.56 };

struct Args {
    root: PathBuf,
    verify: bool,
    only: Option<String>,
    quiet: bool,
    /// Bake exactly one `.glb` into its own texture beside it.
    model: Option<PathBuf>,
    /// Sun direction the shadow-SDF sidecars bake against.
    sun: Vec3f,
}

fn parse_args() -> Args {
    let mut root = None;
    let mut verify = false;
    let mut only = None;
    let mut quiet = false;
    let mut model = None;
    let mut sun = DEFAULT_SDF_SUN;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--verify" => verify = true,
            "--quiet" => quiet = true,
            "--pack" => only = it.next(),
            "--model" => model = it.next().map(PathBuf::from),
            // The evaluator switch, as a flag: it simply sets the env var the
            // bake dispatches on, so a flag and an inherited AO_BAKER behave
            // identically (the flag wins). Set before any bake thread spawns.
            "--baker" => {
                let Some(b) = it.next() else {
                    eprintln!("ao-bake: --baker needs a value (lightmapper|bakerboy|aobaker)");
                    std::process::exit(2);
                };
                std::env::set_var("AO_BAKER", b);
            }
            // Lightmapper art knobs (see ao_lightmapper's module docs) — like
            // --baker, flags that set the env vars the engine reads.
            "--lm-strength" => {
                let Some(v) = it.next() else {
                    eprintln!("ao-bake: --lm-strength needs a value (default 1.0)");
                    std::process::exit(2);
                };
                std::env::set_var("AO_LM_STRENGTH", v);
            }
            "--lm-gamma" => {
                let Some(v) = it.next() else {
                    eprintln!("ao-bake: --lm-gamma needs a value (default 2.2)");
                    std::process::exit(2);
                };
                std::env::set_var("AO_LM_GAMMA", v);
            }
            "--sun" => {
                let parts: Option<Vec<f32>> = it.next().map(|v| {
                    v.split(',').filter_map(|c| c.trim().parse::<f32>().ok()).collect()
                });
                match parts.as_deref() {
                    Some([x, y, z]) if *y > 0.0 => sun = vec3f(*x, *y, *z),
                    _ => {
                        eprintln!("ao-bake: --sun needs X,Y,Z with Y (up) positive");
                        std::process::exit(2);
                    }
                }
            }
            _ => root = Some(PathBuf::from(a)),
        }
    }
    if let Some(m) = model {
        return Args { root: m.clone(), verify, only, quiet, model: Some(m), sun };
    }
    Args {
        root: root.unwrap_or_else(|| {
            eprintln!(
                "usage: ao-bake <resources-dir> [--verify] [--pack NAME] [--quiet] [--baker NAME] [--sun X,Y,Z]\n       \
                 ao-bake --model <file.glb> [--baker NAME] [--sun X,Y,Z]"
            );
            std::process::exit(2);
        }),
        verify,
        only,
        quiet,
        model: None,
        sun,
    }
}

/// Cheap gate before paying a skinned parse: a gait rig's glb must carry
/// the glTF `animations` and `skins` arrays, and their key names appear
/// verbatim in the JSON chunk. Statics fail this in a substring scan.
fn looks_rigged(bytes: &[u8]) -> bool {
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    has(b"\"animations\"") && has(b"\"skins\"")
}

/// The rig-keyed shadow sidecar path: appended to the FULL file name
/// (`hero.glb.shadowsdf`), the `.skinao` convention — distinct from the
/// model-keyed `hero.shadowsdf`, which replaces the extension like
/// `.aomesh` does.
fn rig_sidecar_path(glb: &Path) -> PathBuf {
    PathBuf::from(format!("{}.shadowsdf", glb.display()))
}

/// Bake one glb's RIG shadow sidecar bytes, if it is a gait rig at all:
/// parses skinned, resolves the same idle/walk pair the sandbox cast
/// loader does, and bakes through the exact runtime seed path
/// (`shadow_sdf::bake_rig_atlas`). Keyed by rest hash inside the bytes.
fn rig_shadowsdf_bytes(bytes: &[u8], sun: &SunLight) -> Option<Vec<u8>> {
    let rig = SkinnedModel::parse_glb(bytes).ok()?;
    let atlas = shadow_sdf::bake_rig_atlas(&rig, sun)?;
    Some(atlas.to_shadowsdf(rig.rest_hash()))
}

/// Every `.glb` under `dir`, grouped by the pack directory that holds it.
fn packs(root: &Path) -> BTreeMap<PathBuf, Vec<PathBuf>> {
    let mut out: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "glb") {
                // The pack is the directory the model sits in. Sorted, so the
                // atlas packing order does not depend on the filesystem.
                out.entry(dir.clone()).or_default().push(p);
            }
        }
    }
    for v in out.values_mut() {
        v.sort();
    }
    out
}

/// Minimal greyscale PNG writer.
///
/// The atlas is single-channel and this tool has no image dependency — the
/// repo rule is zero EXTERNAL crates, and the in-repo deflate does the
/// compressing. That matters at library scale: AO atlases are mostly-white
/// with soft gradients, and stored (uncompressed) blocks shipped every one
/// of those white texels — measured 200 MB for one pack. Deflate folds them
/// to a few percent.
fn write_gray_png(path: &Path, pixels: &[u8], size: usize) -> std::io::Result<()> {
    fn crc32(data: &[u8]) -> u32 {
        makepad_fast_inflate::crc32(data)
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let mut full = kind.to_vec();
        full.extend_from_slice(body);
        out.extend_from_slice(&full);
        out.extend_from_slice(&crc32(&full).to_be_bytes());
    }

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(size as u32).to_be_bytes());
    ihdr.extend_from_slice(&(size as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]); // 8-bit greyscale
    chunk(&mut png, b"IHDR", &ihdr);

    // Raw scanlines, each prefixed with filter type 0. The runtime decoder
    // (renderer.rs gray_png_texture) accepts exactly this layout — change it
    // there too or every model silently loses its AO.
    let mut raw = Vec::with_capacity((size + 1) * size);
    for y in 0..size {
        raw.push(0);
        raw.extend_from_slice(&pixels[y * size..y * size + size]);
    }
    chunk(&mut png, b"IDAT", &makepad_fast_inflate::zlib_compress(&raw, 9));
    chunk(&mut png, b"IEND", b"");
    std::fs::write(path, png)
}

fn main() {
    let args = parse_args();
    let sun = SunLight { dir: args.sun.normalize(), ..SunLight::default() };

    // Single-model mode: one house, its own texture, seconds per iteration.
    //
    // This is the loop for judging how the AO LOOKS. Baking a whole pack to
    // inspect one model wastes minutes per change, and a shared atlas makes the
    // result hard to read — one model alone fills its texture, so what you see
    // is the unwrap and the occlusion rather than the packing.
    if let Some(path) = &args.model {
        let started = std::time::Instant::now();
        let bytes = std::fs::read(path).unwrap_or_else(|e| {
            eprintln!("ao-bake: {}: {e}", path.display());
            std::process::exit(2);
        });
        let mut atlas = AoAtlas::new(MODEL_ATLAS_MAX);
        // A lone model owns its whole texture — bake at the density the
        // texture affords, not the density a 40-model pack could.
        atlas.fill = true;
        let model = match StaticModel::parse_glb_baked(&bytes, &mut atlas) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("ao-bake: {}: {e}", path.display());
                std::process::exit(1);
            }
        };
        let side = path.with_extension("aomesh");
        if let Err(e) = std::fs::write(&side, model.to_aomesh()) {
            eprintln!("ao-bake: write {}: {e}", side.display());
            std::process::exit(1);
        }
        let out = path.with_extension("ao.png");
        if let Err(e) = write_gray_png(&out, &atlas.pixels, atlas.size) {
            eprintln!("ao-bake: write {}: {e}", out.display());
            std::process::exit(1);
        }
        println!(
            "{} -> {} ({}x{}, {} KB) in {:.1}s — {} overflowed, spread {:.1} deg",
            path.display(),
            out.display(),
            atlas.size,
            atlas.size,
            atlas.kilobytes(),
            started.elapsed().as_secs_f32(),
            atlas.overflowed,
            atlas.max_chart_spread,
        );
        println!(
            "evaluator {}: {:.2}s bake-only",
            atlas.bake_evaluator, atlas.bake_seconds
        );
        // The shadow-SDF sidecars ride along, exactly as in the batch path.
        match shadow_sdf::bake_model_atlas(&model, &sun) {
            Some(a) => {
                let out = path.with_extension("shadowsdf");
                let b = a.to_shadowsdf(0);
                if let Err(e) = std::fs::write(&out, &b) {
                    eprintln!("ao-bake: write {}: {e}", out.display());
                    std::process::exit(1);
                }
                println!("shadow sdf: {} ({} rows, {} bytes)", out.display(), a.rows, b.len());
            }
            None => println!("shadow sdf: silhouette degenerate — none written"),
        }
        if looks_rigged(&bytes) {
            if let Some(b) = rig_shadowsdf_bytes(&bytes, &sun) {
                let out = rig_sidecar_path(path);
                if let Err(e) = std::fs::write(&out, &b) {
                    eprintln!("ao-bake: write {}: {e}", out.display());
                    std::process::exit(1);
                }
                println!("shadow sdf (rig): {} ({} bytes)", out.display(), b.len());
            }
        }
        return;
    }

    if !args.root.is_dir() {
        eprintln!("ao-bake: {} is not a directory", args.root.display());
        std::process::exit(2);
    }

    let groups = packs(&args.root);
    let jobs: Vec<(PathBuf, Vec<PathBuf>)> = groups
        .into_iter()
        .filter(|(dir, _)| {
            args.only.as_ref().is_none_or(|only| {
                dir.strip_prefix(&args.root)
                    .unwrap_or(dir)
                    .to_string_lossy()
                    .contains(only.as_str())
            })
        })
        .collect();
    let started = std::time::Instant::now();

    // The superseded pack atlases. The runtime prefers a model's own texture
    // but falls back to these, and a stale fallback beside fresh sidecars is
    // exactly the mixed state that cost days of "the AO looks wrong".
    for (dir, _) in &jobs {
        let _ = std::fs::remove_file(dir.join("ao_atlas.png"));
    }

    // MODELS across threads, not triangles. Every bake is independent now
    // that no atlas is shared, and a model is a coarse enough unit that a
    // work-stealing cursor keeps all cores busy to the tail. (Each bake still
    // fans its charts out internally; the brief oversubscription is cheaper
    // than idle cores between small props.)
    let files: Vec<&PathBuf> = jobs.iter().flat_map(|(_, models)| models).collect();
    let threads = std::thread::available_parallelism().map_or(8, |n| n.get());
    let cursor = std::sync::atomic::AtomicUsize::new(0);
    let done = std::sync::atomic::AtomicUsize::new(0);
    let skipped = std::sync::atomic::AtomicUsize::new(0);
    let mismatched = std::sync::atomic::AtomicUsize::new(0);
    let texture_bytes = std::sync::atomic::AtomicUsize::new(0);
    let sdf_written = std::sync::atomic::AtomicUsize::new(0);
    let rig_written = std::sync::atomic::AtomicUsize::new(0);
    let sdf_fresh = std::sync::atomic::AtomicUsize::new(0);
    let rig_fresh = std::sync::atomic::AtomicUsize::new(0);
    let quiet = args.quiet;
    let verify = args.verify;
    let sun = &sun;
    let sun_len = sun.shadow_len_per_unit();
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                use std::sync::atomic::Ordering::Relaxed;
                let i = cursor.fetch_add(1, Relaxed);
                let Some(glb) = files.get(i) else { break };
                let png = glb.with_extension("ao.png");
                let mesh = glb.with_extension("aomesh");
                let sdf = glb.with_extension("shadowsdf");
                let rig_sdf = rig_sidecar_path(glb);
                // Resume for free, per ARTEFACT: outputs newer than their glb
                // are done. A killed run picks up where it stopped, re-running
                // after new models land bakes only the new ones, and a library
                // baked before the shadow sidecars existed pays only the
                // (cheap) sdf pass over its shipped .aomesh files. (Touch the
                // glb — or delete the sidecars — to force a model through.)
                let stamp =
                    |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
                let g = stamp(glb);
                let fresh =
                    |p: &std::path::Path| matches!((g, stamp(p)), (Some(g), Some(s)) if s > g);
                // Freshness for a shadow sidecar is mtime AND SUN: the bake
                // is sun-dependent (`len_per_unit` in the header), and a
                // sidecar baked for a different sun stays mtime-fresh
                // forever while the runtime rejects it every launch — the
                // silent "0 sidecars written, yet the cast blob-tiers"
                // state. A 44-byte header read settles it.
                let for_sun = |p: &std::path::Path| {
                    use std::io::Read;
                    let mut header = [0u8; makepad_render::shadow_sdf::SHADOWSDF_HEADER_LEN];
                    std::fs::File::open(p)
                        .and_then(|mut f| f.read_exact(&mut header))
                        .ok()
                        .and_then(|_| shadow_sdf::ShadowSdfAtlas::header_len_per_unit(&header))
                        .is_some_and(|lpu| (lpu - sun_len).abs() <= 1.0e-3)
                };
                let need_ao = verify || !(fresh(&png) && fresh(&mesh));
                let need_sdf = verify || !fresh(&sdf) || !for_sun(&sdf);
                // A static prop never grows a RIG sidecar, so absence alone
                // cannot mean stale — the sniff on the bytes below settles
                // whether this glb is a rig at all.
                let mut need_rig = verify || !fresh(&rig_sdf) || !for_sun(&rig_sdf);
                if !need_ao && !need_sdf && !need_rig {
                    sdf_fresh.fetch_add(1, Relaxed);
                    rig_fresh.fetch_add(1, Relaxed);
                    done.fetch_add(1, Relaxed);
                    continue;
                }
                if !need_sdf {
                    sdf_fresh.fetch_add(1, Relaxed);
                }
                if !need_rig {
                    rig_fresh.fetch_add(1, Relaxed);
                }
                let Ok(bytes) = std::fs::read(glb) else {
                    skipped.fetch_add(1, Relaxed);
                    continue;
                };
                need_rig = need_rig && looks_rigged(&bytes);
                // The model whose silhouette the sdf sidecar bakes: the
                // freshly baked one when the AO pass runs, else the shipped
                // .aomesh — the very mesh the runtime loads, so sidecar and
                // runtime agree on the geometry by construction either way.
                let mut model: Option<StaticModel> = None;
                if need_ao {
                    let mut atlas = AoAtlas::new(MODEL_ATLAS_MAX);
                    atlas.fill = true;
                    match StaticModel::parse_glb_baked(&bytes, &mut atlas) {
                        Ok(m) => {
                            if verify {
                                let mut fresh_png = Vec::new();
                                let tmp =
                                    std::env::temp_dir().join(format!("ao_verify_{i}.png"));
                                if write_gray_png(&tmp, &atlas.pixels, atlas.size).is_ok() {
                                    fresh_png = std::fs::read(&tmp).unwrap_or_default();
                                    let _ = std::fs::remove_file(&tmp);
                                }
                                if std::fs::read(&png).ok().as_deref()
                                    != Some(fresh_png.as_slice())
                                    || std::fs::read(&mesh).ok().as_deref()
                                        != Some(m.to_aomesh().as_slice())
                                {
                                    println!("MISMATCH {}", glb.display());
                                    mismatched.fetch_add(1, Relaxed);
                                }
                            } else {
                                // The baked geometry ships beside the texture.
                                // Its uvs are this bake's packing, so the
                                // runtime cannot recompute them — it must load
                                // the very mesh that was baked.
                                if let Err(e) = std::fs::write(&mesh, m.to_aomesh()) {
                                    eprintln!("  write {}: {e}", mesh.display());
                                }
                                if let Err(e) = write_gray_png(&png, &atlas.pixels, atlas.size) {
                                    eprintln!("  write {}: {e}", png.display());
                                } else {
                                    texture_bytes.fetch_add(atlas.pixels.len(), Relaxed);
                                }
                            }
                            model = Some(m);
                        }
                        Err(e) => {
                            if !quiet {
                                eprintln!("  skip {}: {e}", glb.display());
                            }
                            skipped.fetch_add(1, Relaxed);
                            // A glb the static baker rejects can still be a
                            // rig; only bail when it is not one.
                            if !need_rig {
                                continue;
                            }
                        }
                    }
                } else if need_sdf {
                    model = std::fs::read(&mesh)
                        .ok()
                        .and_then(|b| StaticModel::from_aomesh(&b));
                }
                if need_sdf {
                    let fresh_sdf = model
                        .as_ref()
                        .and_then(|m| shadow_sdf::bake_model_atlas(m, sun))
                        .map(|a| a.to_shadowsdf(0));
                    if verify {
                        // Option vs Option: an unbakeable silhouette matching
                        // an absent file is agreement, not a miss.
                        if std::fs::read(&sdf).ok() != fresh_sdf {
                            println!("MISMATCH {} (shadowsdf)", glb.display());
                            mismatched.fetch_add(1, Relaxed);
                        }
                    } else if let Some(b) = &fresh_sdf {
                        if let Err(e) = std::fs::write(&sdf, b) {
                            eprintln!("  write {}: {e}", sdf.display());
                        } else {
                            sdf_written.fetch_add(1, Relaxed);
                        }
                    }
                }
                if need_rig {
                    let fresh_rig = rig_shadowsdf_bytes(&bytes, sun);
                    if verify {
                        if std::fs::read(&rig_sdf).ok() != fresh_rig {
                            println!("MISMATCH {} (rig shadowsdf)", glb.display());
                            mismatched.fetch_add(1, Relaxed);
                        }
                    } else if let Some(b) = &fresh_rig {
                        if let Err(e) = std::fs::write(&rig_sdf, b) {
                            eprintln!("  write {}: {e}", rig_sdf.display());
                        } else {
                            rig_written.fetch_add(1, Relaxed);
                        }
                    }
                }
                let n = done.fetch_add(1, Relaxed) + 1;
                if !quiet && n % 250 == 0 {
                    println!(
                        "  [{n}/{}] {:.0}s elapsed",
                        files.len(),
                        started.elapsed().as_secs_f32()
                    );
                }
            });
        }
    });

    let secs = started.elapsed().as_secs_f32();
    println!(
        "\n{} packs, {} models baked with {} in {secs:.1}s ({} skipped, {threads} threads) — {} MB of textures, {} shadow sdf sidecars ({} rigs)",
        jobs.len(),
        done.load(std::sync::atomic::Ordering::Relaxed),
        makepad_render::ao_atlas::AoBakerKind::current().name(),
        skipped.load(std::sync::atomic::Ordering::Relaxed),
        texture_bytes.load(std::sync::atomic::Ordering::Relaxed) / (1024 * 1024),
        sdf_written.load(std::sync::atomic::Ordering::Relaxed),
        rig_written.load(std::sync::atomic::Ordering::Relaxed),
    );
    if verify {
        let m = mismatched.load(std::sync::atomic::Ordering::Relaxed);
        if m > 0 {
            eprintln!("{m} models did not reproduce — the bake is not deterministic");
            std::process::exit(1);
        }
        println!("verify: every model reproduced byte for byte");
    }
}
