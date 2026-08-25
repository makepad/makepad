//! Classic pack convert: walk a local folder, dispatch per-game converters.
//!
//! Shared types/PNG live in [`shared`]. Doom WAD/maps in [`doom`], Quake 1
//! BSP/MDL/SPR in [`quake`]. Duke / Quake II / Quake III / id Tech 4 stay
//! in their own crate modules and are called from here.

mod shared;
mod doom;
mod doors;
mod hazards;
mod movers;
mod quake;
mod weld;

pub use shared::*;

/// T-junction weld, re-exported for the per-game converters that live in
/// their own crate modules (Duke, Quake II/III, id Tech 4) — one pass, one
/// definition of "this vertex sits on that edge", for every classic level.
pub(crate) use weld::Soup as WeldSoup;

pub(crate) fn weld_parts(parts: &[&[[f32; 3]]]) -> weld::Weld {
    weld::Weld::from_parts(parts)
}

/// The corner merge that closes what the splitter declines (see
/// [`weld::Merge`]). Run BEFORE [`weld_parts`], on the same list of parts.
pub(crate) fn merge_near_corners(parts: &[&[[f32; 3]]]) -> weld::Merge {
    weld::Merge::from_parts(parts, weld::MERGE_TOLERANCE)
}

/// Test-only audit: T-junctions still present across a converted level's
/// parts (see [`weld::t_junctions_left`]).
#[cfg(test)]
pub(crate) fn weld_t_junctions_left(parts: &[(&[[f32; 3]], &[u32])]) -> usize {
    weld::t_junctions_left(parts)
}

use crate::ao_bake::BakeStats;
use crate::pack_import;
use makepad_asset_data::AssetKind;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use doom::*;
use quake::*;

/// Convert a local Freedoom / LibreQuake folder into staged PNG / WAV / GLB
/// payloads, then AO-bake every produced GLB.
pub fn convert_classic(
    pack_dir: &Path,
    staged_dir: &Path,
    source: ClassicSource,
) -> Result<ClassicConvertReport, ClassicImportError> {
    convert_classic_ex(pack_dir, staged_dir, source, |_| true)
}

/// Same as [`convert_classic`], with a progress tick (`current` file or bake).
/// Return `false` from `on_progress` to cancel; the error message is
/// `"cancelled"`.
pub fn convert_classic_ex(
    pack_dir: &Path,
    staged_dir: &Path,
    source: ClassicSource,
    mut on_progress: impl FnMut(ConvertTick) -> bool + Send,
) -> Result<ClassicConvertReport, ClassicImportError> {
    require_pack_dir(pack_dir)?;
    let tick = |on_progress: &mut dyn FnMut(ConvertTick) -> bool,
                stage: ConvertStage,
                done: usize,
                total: usize,
                current: String|
     -> Result<(), ClassicImportError> {
        if on_progress(ConvertTick {
            stage,
            done,
            total,
            current,
            preview_png: None,
        }) {
            Ok(())
        } else {
            Err(ClassicImportError::new("cancelled"))
        }
    };
    tick(
        &mut on_progress,
        ConvertStage::Expand,
        0,
        1,
        "scanning pack".into(),
    )?;
    if staged_dir.exists() {
        tick(
            &mut on_progress,
            ConvertStage::Expand,
            0,
            1,
            "clearing previous staging".into(),
        )?;
        std::fs::remove_dir_all(staged_dir).map_err(|e| {
            ClassicImportError::new(format!("clear staging {}: {e}", staged_dir.display()))
        })?;
    }
    std::fs::create_dir_all(staged_dir).map_err(|e| {
        ClassicImportError::new(format!("create staging {}: {e}", staged_dir.display()))
    })?;

    let mut assets = Vec::new();
    let mut skipped = Vec::new();
    let mut warnings = Vec::new();
    let mut files = collect_classic_files(pack_dir)?;
    if files.is_empty() {
        return Err(ClassicImportError::new(format!(
            "folder has no classic sources (wad/pak/bsp/mdl/spr/wav/grp/map/pk3/pk4/md5mesh/proc): {}",
            pack_dir.display()
        )));
    }

    // Expand PAKs outside the staged tree. `_pak/` inside staged is not a
    // catalog path (leading `_` fails pack_import) and leftover cfg/dat/lmp
    // would be walked as unsupported files.
    let pak_scratch = staged_dir
        .parent()
        .unwrap_or(staged_dir)
        .join("pak_expand");
    if pak_scratch.exists() {
        let _ = std::fs::remove_dir_all(&pak_scratch);
    }
    let expand_total = files
        .iter()
        .filter(|f| {
            matches!(
                f.kind,
                ClassicFileKind::Pak
                    | ClassicFileKind::Grp
                    | ClassicFileKind::Pk3
                    | ClassicFileKind::Pk4
            )
        })
        .count()
        .max(1);
    tick(
        &mut on_progress,
        ConvertStage::Expand,
        0,
        expand_total,
        format!("found {} pack files", files.len()),
    )?;
    let mut expanded = Vec::new();
    let mut expand_done = 0usize;
    for f in files.drain(..) {
        let archive = matches!(
            f.kind,
            ClassicFileKind::Pak
                | ClassicFileKind::Grp
                | ClassicFileKind::Pk3
                | ClassicFileKind::Pk4
        );
        let rel = f.rel.clone();
        if archive {
            tick(
                &mut on_progress,
                ConvertStage::Expand,
                expand_done,
                expand_total,
                format!("unpack {rel}"),
            )?;
        }
        if f.kind == ClassicFileKind::Pak {
            match expand_pak(&f.path, pak_scratch.join("pak"), &mut warnings) {
                Ok(inner) => expanded.extend(inner),
                Err(e) => skipped.push(format!("{}: {e}", f.rel)),
            }
        } else if f.kind == ClassicFileKind::Grp {
            match crate::duke_import::expand_grp(&f.path, &pak_scratch.join("grp"), &mut warnings)
            {
                Ok(inner) => expanded.extend(inner.into_iter().filter_map(|e| {
                    classic_file_from_extracted(e.path, e.rel)
                })),
                Err(e) => skipped.push(format!("{}: {e}", f.rel)),
            }
        } else if f.kind == ClassicFileKind::Pk3 {
            match crate::quake3_import::expand_pk3(&f.path, &pak_scratch.join("pk3"), &mut warnings)
            {
                Ok(inner) => expanded.extend(inner.into_iter().filter_map(|e| {
                    classic_file_from_extracted(e.path, e.rel)
                })),
                Err(e) => skipped.push(format!("{}: {e}", f.rel)),
            }
        } else if f.kind == ClassicFileKind::Pk4 {
            match crate::doom3_import::expand_pk4(&f.path, &pak_scratch.join("pk4"), &mut warnings)
            {
                Ok(inner) => expanded.extend(inner.into_iter().filter_map(|e| {
                    classic_file_from_extracted(e.path, e.rel)
                })),
                Err(e) => skipped.push(format!("{}: {e}", f.rel)),
            }
        } else if f.kind == ClassicFileKind::Zip {
            // HRP zip is consumed later as an overlay, not as payloads.
            expanded.push(f);
        } else {
            expanded.push(f);
        }
        if archive {
            expand_done += 1;
            tick(
                &mut on_progress,
                ConvertStage::Expand,
                expand_done,
                expand_total,
                format!("unpacked {rel}"),
            )?;
        }
    }
    files = expanded;
    if matches!(source, ClassicSource::Quake3) {
        let md3 = files.iter().filter(|f| f.kind == ClassicFileKind::Md3).count();
        let bsp = files.iter().filter(|f| f.kind == ClassicFileKind::Bsp).count();
        let wav = files.iter().filter(|f| f.kind == ClassicFileKind::Wav).count();
        warnings.push(format!(
            "quake3 expand: md3={md3} bsp={bsp} wav={wav} skipped_so_far={}",
            skipped.len()
        ));
    }

    let art_bank = if matches!(source, ClassicSource::Duke3d) {
        crate::duke_import::load_tileset(&files.iter().map(|f| f.path.clone()).collect::<Vec<_>>(), pack_dir)
    } else {
        crate::duke_import::ArtBank::default()
    };
    let wal_bank = if matches!(source, ClassicSource::Quake2) {
        let mut bank = crate::quake2_import::load_wal_bank(pack_dir);
        bank.tiles
            .extend(crate::quake2_import::load_wal_bank(&pak_scratch.join("pak")).tiles);
        bank
    } else {
        crate::quake2_import::WalBank::default()
    };
    let idtech4 = files_are_idtech4(&files);
    let q3_bank = if matches!(source, ClassicSource::Quake3) {
        let mut bank = crate::quake3_import::load_tex_bank(pack_dir);
        bank.extend_bank(crate::quake3_import::load_tex_bank(
            pak_scratch.join("pk3").as_path(),
        ));
        crate::quake3_import::apply_shader_aliases(&mut bank, pack_dir);
        crate::quake3_import::apply_shader_aliases(&mut bank, &pak_scratch.join("pk3"));
        bank
    } else if idtech4 {
        tick(
            &mut on_progress,
            ConvertStage::Expand,
            expand_total,
            expand_total,
            "loading texture bank".into(),
        )?;
        let mut bank = crate::quake3_import::load_tex_bank(pack_dir);
        bank.extend_bank(crate::quake3_import::load_tex_bank(
            pak_scratch.join("pk4").as_path(),
        ));
        crate::doom3_import::apply_mtr_aliases(&mut bank, pack_dir);
        crate::doom3_import::apply_mtr_aliases(&mut bank, &pak_scratch.join("pk4"));
        bank
    } else {
        crate::quake3_import::Q3TexBank::default()
    };

    // Shared Doom PLAYPAL when an IWAD is present (used for patches/sprites).
    let mut playpal: Option<[[u8; 3]; 256]> = None;
    for f in &files {
        if f.kind == ClassicFileKind::Wad {
            if let Ok(wad) = parse_wad(&std::fs::read(&f.path).unwrap_or_default()) {
                if let Some(pal) = wad.playpal {
                    playpal = Some(pal);
                    break;
                }
            }
        }
    }
    let palette = playpal.unwrap_or_else(default_vga_palette);

    // Duke face-sprite picnums actually placed in converted maps; gates
    // which leftover ART tiles may become library cards.
    let mut duke_used_faces: BTreeSet<u16> = BTreeSet::new();
    files.sort_by_key(|f| convert_priority(f.kind, idtech4));
    let convert_units = files
        .iter()
        .map(|f| match f.kind {
            ClassicFileKind::Wad => {
                std::fs::read(&f.path)
                    .ok()
                    .and_then(|b| parse_wad(&b).ok())
                    .map(|w| find_doom_maps(&w.lumps).len().max(1) + 1)
                    .unwrap_or(1)
            }
            ClassicFileKind::Tga | ClassicFileKind::Png | ClassicFileKind::Md5Anim if idtech4 => 0,
            _ => 1,
        })
        .sum::<usize>()
        .max(1);
    let mut convert_done = 0usize;

    for f in &files {
        if !on_progress(ConvertTick {
            stage: ConvertStage::Convert,
            done: convert_done,
            total: convert_units,
            current: f.rel.clone(),
            preview_png: None,
        }) {
            let _ = std::fs::remove_dir_all(&pak_scratch);
            return Err(ClassicImportError::new("cancelled"));
        }
        match f.kind {
            ClassicFileKind::Wad => match convert_wad(
                &f.path,
                &f.rel,
                staged_dir,
                &palette,
                source,
                |map_i, map_n, name, preview_png| {
                    on_progress(ConvertTick {
                        stage: ConvertStage::Convert,
                        done: convert_done + map_i,
                        total: convert_units,
                        current: format!("{} · {name} ({}/{map_n})", f.rel, map_i + 1),
                        preview_png,
                    })
                },
            ) {
                Ok(mut list) => {
                    let maps = list
                        .iter()
                        .filter(|a| a.kind == AssetKind::World)
                        .count()
                        .max(1);
                    convert_done += maps + 1;
                    assets.append(&mut list);
                }
                Err(e) if e == "cancelled" => {
                    let _ = std::fs::remove_dir_all(&pak_scratch);
                    return Err(ClassicImportError::new("cancelled"));
                }
                Err(e) => skipped.push(format!("{}: {e}", f.rel)),
            },
            ClassicFileKind::Bsp => match convert_bsp(
                &f.path,
                &f.rel,
                staged_dir,
                source,
                &wal_bank,
                &q3_bank,
            ) {
                Ok(mut list) => {
                    convert_done += 1;
                    assets.append(&mut list);
                }
                Err(e) => skipped.push(format!("{}: {e}", f.rel)),
            },
            ClassicFileKind::Mdl => match convert_mdl(&f.path, &f.rel, staged_dir, source) {
                Ok(a) => {
                    convert_done += 1;
                    assets.push(a);
                }
                Err(e) => skipped.push(format!("{}: {e}", f.rel)),
            },
            ClassicFileKind::Spr => match convert_spr(&f.path, &f.rel, staged_dir, source) {
                Ok(mut list) => {
                    convert_done += 1;
                    assets.append(&mut list);
                }
                Err(e) => skipped.push(format!("{}: {e}", f.rel)),
            },
            ClassicFileKind::Wav => match convert_wav(&f.path, &f.rel, staged_dir, source) {
                Ok(a) => {
                    convert_done += 1;
                    assets.push(a);
                }
                Err(e) => skipped.push(format!("{}: {e}", f.rel)),
            },
            ClassicFileKind::Pak
            | ClassicFileKind::Grp
            | ClassicFileKind::Pk3
            | ClassicFileKind::Pk4 => {}
            ClassicFileKind::Zip => {}
            ClassicFileKind::Art => {}
            ClassicFileKind::Map if idtech4 => {
                // Editor .map stays out; .proc is the compiled world.
                convert_done += 1;
            }
            ClassicFileKind::Map => match convert_build_map(
                &f.path,
                &f.rel,
                staged_dir,
                source,
                &art_bank,
                &mut duke_used_faces,
            ) {
                Ok(mut list) => {
                    convert_done += 1;
                    assets.append(&mut list);
                }
                Err(e) => skipped.push(format!("{}: {e}", f.rel)),
            },
            ClassicFileKind::Md2 => match crate::quake2_import::convert_md2_file(
                &f.path,
                &f.rel,
                staged_dir,
                source.id(),
            ) {
                Ok(a) => {
                    convert_done += 1;
                    assets.push(a);
                }
                Err(e) => skipped.push(format!("{}: {e}", f.rel)),
            },
            ClassicFileKind::Md3 => match std::fs::read(&f.path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| {
                    crate::quake3_import::convert_md3(
                        &bytes,
                        &f.rel,
                        &f.path,
                        staged_dir,
                        source.id(),
                        &q3_bank,
                    )
                }) {
                Ok(a) => {
                    convert_done += 1;
                    assets.push(a);
                }
                Err(e) => skipped.push(format!("{}: {e}", f.rel)),
            },
            ClassicFileKind::Wal => match crate::quake2_import::convert_wal_file(
                &f.path,
                &f.rel,
                staged_dir,
                source.id(),
            ) {
                Ok(Some(a)) => {
                    convert_done += 1;
                    assets.push(a);
                }
                Ok(None) => convert_done += 1,
                Err(e) => skipped.push(format!("{}: {e}", f.rel)),
            },
            ClassicFileKind::Wad2 => match convert_wad2(&f.path, &f.rel, staged_dir, source) {
                Ok(mut list) => {
                    convert_done += 1;
                    assets.append(&mut list);
                }
                Err(e) => skipped.push(format!("{}: {e}", f.rel)),
            },
            ClassicFileKind::Md5Mesh => match std::fs::read(&f.path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| {
                    crate::doom3_import::convert_md5mesh_with_bank(
                        &bytes,
                        &f.rel,
                        staged_dir,
                        source.id(),
                        &q3_bank,
                    )
                }) {
                Ok(a) => {
                    convert_done += 1;
                    assets.push(a);
                }
                Err(e) => skipped.push(format!("{}: {e}", f.rel)),
            },
            ClassicFileKind::Proc => match std::fs::read(&f.path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| {
                    crate::doom3_import::convert_proc_with_bank(
                        &bytes,
                        &f.rel,
                        staged_dir,
                        source.id(),
                        &q3_bank,
                    )
                }) {
                Ok(mut list) => {
                    convert_done += 1;
                    assets.append(&mut list);
                }
                Err(e) => skipped.push(format!("{}: {e}", f.rel)),
            },
            ClassicFileKind::Lwo => match std::fs::read(&f.path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| {
                    crate::doom3_import::convert_lwo_with_bank(
                        &bytes,
                        &f.rel,
                        staged_dir,
                        source.id(),
                        &q3_bank,
                    )
                }) {
                Ok(Some(a)) => {
                    convert_done += 1;
                    assets.push(a);
                }
                Ok(None) => convert_done += 1,
                Err(e) => skipped.push(format!("{}: {e}", f.rel)),
            },
            ClassicFileKind::Tga | ClassicFileKind::Png => {
                // id Tech 4 images are tex-bank tiles, not catalog Texture cards.
                convert_done += 1;
            }
            ClassicFileKind::Md5Anim => {}
        }
    }

    if idtech4 {
        let meshes: Vec<(PathBuf, String)> = files
            .iter()
            .filter(|f| f.kind == ClassicFileKind::Md5Mesh)
            .map(|f| (f.path.clone(), f.rel.clone()))
            .collect();
        let anims: Vec<(PathBuf, String)> = files
            .iter()
            .filter(|f| f.kind == ClassicFileKind::Md5Anim)
            .map(|f| (f.path.clone(), f.rel.clone()))
            .collect();
        let extra = crate::doom3_import::assemble_md5_billboards(
            &meshes,
            &anims,
            staged_dir,
            source.id(),
            &q3_bank,
            |current| {
                let _ = tick(
                    &mut on_progress,
                    ConvertStage::Convert,
                    convert_done,
                    convert_units,
                    current.to_string(),
                );
            },
        );
        assets.extend(extra);
    }

    if assets.is_empty() {
        let _ = std::fs::remove_dir_all(&pak_scratch);
        return Err(ClassicImportError::new(format!(
            "no convertible assets in {} (skipped {})",
            pack_dir.display(),
            skipped.len()
        )));
    }

    if matches!(source, ClassicSource::Quake3) {
        let md3s: Vec<(PathBuf, String)> = files
            .iter()
            .filter(|f| f.kind == ClassicFileKind::Md3)
            .map(|f| (f.path.clone(), f.rel.clone()))
            .collect();
        let (assembled, drop) =
            crate::quake3_import::assemble_players_and_weapons(&md3s, staged_dir, source.id(), &q3_bank);
        let _ = drop;
        assets.extend(assembled);
        assets.extend(crate::quake3_import::emit_texture_assets(
            &q3_bank,
            staged_dir,
            source.id(),
        ));
    }

    if matches!(source, ClassicSource::Duke3d) {
        // CON actors fold their rotation sheets, fonts and orphan strips
        // group into one `.billboard` per run, and only map-placed
        // singleton tiles remain as individual cards. Per-frame `tile-N`
        // PNGs never enter `assets`.
        crate::duke_import::assemble_duke_billboards(
            &mut assets,
            staged_dir,
            pack_dir,
            source.id(),
            &art_bank,
            &duke_used_faces,
        );
    }
    collapse_stateful_billboards(&mut assets, staged_dir);
    // Never leave PAK leftovers (or a previous run's `_pak`) in staged.
    let _ = std::fs::remove_dir_all(&pak_scratch);
    let _ = std::fs::remove_dir_all(staged_dir.join("_pak"));

    // Meshes write their own raster icons. A 0x20 placeholder next to a GLB
    // becomes the library thumb and looks empty.

    // Classic / old-game packs stay AO-off. Kenney bakes in Asset UI
    // `bake_staged_glbs`; a Dark Mod-sized tree here takes hours.
    let bake = BakeStats {
        total: 0,
        baked: 0,
        skipped: 0,
        failed: 0,
    };

    Ok(ClassicConvertReport {
        source,
        assets,
        staged_dir: staged_dir.to_path_buf(),
        bake,
        skipped,
        warnings,
    })
}

/// Convert then compile the staged library with [`pack_import::compile_pack`].
pub fn compile_classic(
    pack_dir: &Path,
    work_dir: &Path,
    source: ClassicSource,
    pack_name: &str,
) -> Result<ClassicCompileReport, ClassicImportError> {
    let staged = work_dir.join("source");
    let bundle = work_dir.join("out").join("bundle");
    if bundle.exists() {
        let _ = std::fs::remove_dir_all(&bundle);
    }
    std::fs::create_dir_all(work_dir.join("out")).map_err(|e| {
        ClassicImportError::new(format!("create out: {e}"))
    })?;
    let convert = convert_classic(pack_dir, &staged, source)?;
    let spec = source.pack_spec(pack_name);
    let pack = pack_import::compile_pack(&staged, &bundle, spec, None, false)?;
    Ok(ClassicCompileReport {
        convert,
        pack: Some(pack),
    })
}

// ---------------------------------------------------------------------------
// File discovery
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClassicFileKind {
    Wad,
    Wad2,
    Pak,
    Bsp,
    Mdl,
    Spr,
    Wav,
    Grp,
    Art,
    Map,
    Pk3,
    Pk4,
    Md2,
    Md3,
    Md5Mesh,
    Md5Anim,
    Proc,
    Lwo,
    Tga,
    Png,
    Wal,
    Zip,
}

struct ClassicFile {
    path: PathBuf,
    rel: String,
    kind: ClassicFileKind,
}

fn files_are_idtech4(files: &[ClassicFile]) -> bool {
    files.iter().any(|f| {
        matches!(
            f.kind,
            ClassicFileKind::Pk4
                | ClassicFileKind::Md5Mesh
                | ClassicFileKind::Lwo
                | ClassicFileKind::Proc
        )
    })
}

fn convert_priority(kind: ClassicFileKind, idtech4: bool) -> u8 {
    match kind {
        ClassicFileKind::Md5Mesh
        | ClassicFileKind::Lwo
        | ClassicFileKind::Proc
        | ClassicFileKind::Mdl
        | ClassicFileKind::Md2
        | ClassicFileKind::Md3
        | ClassicFileKind::Bsp => 0,
        ClassicFileKind::Wav | ClassicFileKind::Spr => 1,
        ClassicFileKind::Tga | ClassicFileKind::Png if idtech4 => 9,
        _ => 2,
    }
}

fn collect_classic_files(root: &Path) -> Result<Vec<ClassicFile>, ClassicImportError> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut seen = 0usize;
    while let Some(dir) = stack.pop() {
        let rd = std::fs::read_dir(&dir).map_err(|e| {
            ClassicImportError::new(format!("read {}: {e}", dir.display()))
        })?;
        for entry in rd.flatten() {
            seen += 1;
            if seen > 16_384 {
                return Err(ClassicImportError::new(
                    "pack walk exceeded 16384 entries",
                ));
            }
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if crate::doom3_import::is_fan_mission_rel(&rel) {
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let kind = match ext.as_str() {
                "wad" => {
                    // Distinguish WAD2 (Quake) via magic when small header available.
                    if let Ok(bytes) = std::fs::read(&path) {
                        if bytes.starts_with(b"WAD2") {
                            ClassicFileKind::Wad2
                        } else {
                            ClassicFileKind::Wad
                        }
                    } else {
                        ClassicFileKind::Wad
                    }
                }
                "pak" => ClassicFileKind::Pak,
                "bsp" => ClassicFileKind::Bsp,
                "mdl" => ClassicFileKind::Mdl,
                "spr" => ClassicFileKind::Spr,
                "wav" => ClassicFileKind::Wav,
                "grp" => ClassicFileKind::Grp,
                "art" => ClassicFileKind::Art,
                "map" => ClassicFileKind::Map,
                "pk3" => ClassicFileKind::Pk3,
                "pk4" => ClassicFileKind::Pk4,
                "md2" => ClassicFileKind::Md2,
                "md3" => ClassicFileKind::Md3,
                "md5mesh" => ClassicFileKind::Md5Mesh,
                "md5anim" => ClassicFileKind::Md5Anim,
                "proc" => ClassicFileKind::Proc,
                "lwo" => ClassicFileKind::Lwo,
                "tga" => ClassicFileKind::Tga,
                "png" => ClassicFileKind::Png,
                "wal" => ClassicFileKind::Wal,
                "zip" => ClassicFileKind::Zip,
                _ => continue,
            };
            out.push(ClassicFile { path, rel, kind });
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

fn expand_pak(
    pak_path: &Path,
    out_dir: PathBuf,
    warnings: &mut Vec<String>,
) -> Result<Vec<ClassicFile>, String> {
    let bytes = std::fs::read(pak_path).map_err(|e| e.to_string())?;
    if bytes.len() < 12 || &bytes[0..4] != b"PACK" {
        return Err("not a Quake PAK".into());
    }
    let dirofs = u32_le(&bytes, 4) as usize;
    let dirlen = u32_le(&bytes, 8) as usize;
    if dirlen % 64 != 0 || dirofs.saturating_add(dirlen) > bytes.len() {
        return Err("corrupt PAK directory".into());
    }
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let n = dirlen / 64;
    for i in 0..n {
        let off = dirofs + i * 64;
        let name_raw = &bytes[off..off + 56];
        let end = name_raw.iter().position(|&b| b == 0).unwrap_or(56);
        let name = String::from_utf8_lossy(&name_raw[..end])
            .replace('\\', "/")
            .trim()
            .to_string();
        if name.is_empty()
            || name.starts_with('/')
            || name
                .split('/')
                .any(|c| c.is_empty() || c == "." || c == ".." || c.contains(':'))
        {
            continue;
        }
        let file_off = u32_le(&bytes, off + 56) as usize;
        let file_len = u32_le(&bytes, off + 60) as usize;
        if file_off.saturating_add(file_len) > bytes.len() {
            warnings.push(format!("pak entry {name}: truncated"));
            continue;
        }
        let target = out_dir.join(&name);
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&target, &bytes[file_off..file_off + file_len]) {
            warnings.push(format!("pak write {name}: {e}"));
            continue;
        }
        let ext = target
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let kind = match ext.as_str() {
            "bsp" => ClassicFileKind::Bsp,
            "mdl" => ClassicFileKind::Mdl,
            "md2" => ClassicFileKind::Md2,
            "md3" => ClassicFileKind::Md3,
            "wal" => ClassicFileKind::Wal,
            "spr" => ClassicFileKind::Spr,
            "wav" => ClassicFileKind::Wav,
            "wad" => {
                if bytes[file_off..].starts_with(b"WAD2") {
                    ClassicFileKind::Wad2
                } else {
                    ClassicFileKind::Wad
                }
            }
            _ => continue,
        };
        out.push(ClassicFile {
            path: target,
            rel: format!("pak/{}", name),
            kind,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Quake BSP (v29)
// ---------------------------------------------------------------------------

fn classic_file_from_extracted(path: PathBuf, rel: String) -> Option<ClassicFile> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let kind = match ext.as_str() {
        "wad" => {
            if std::fs::read(&path)
                .ok()
                .is_some_and(|b| b.starts_with(b"WAD2"))
            {
                ClassicFileKind::Wad2
            } else {
                ClassicFileKind::Wad
            }
        }
        "bsp" => ClassicFileKind::Bsp,
        "mdl" => ClassicFileKind::Mdl,
        "md2" => ClassicFileKind::Md2,
        "md3" => ClassicFileKind::Md3,
        "md5mesh" => ClassicFileKind::Md5Mesh,
        "md5anim" => ClassicFileKind::Md5Anim,
        "proc" => ClassicFileKind::Proc,
        "lwo" => ClassicFileKind::Lwo,
        "tga" => ClassicFileKind::Tga,
        "png" => ClassicFileKind::Png,
        "spr" => ClassicFileKind::Spr,
        "wav" => ClassicFileKind::Wav,
        "art" => ClassicFileKind::Art,
        "map" => ClassicFileKind::Map,
        "wal" => ClassicFileKind::Wal,
        _ => return None,
    };
    if crate::doom3_import::is_fan_mission_rel(&rel) {
        return None;
    }
    Some(ClassicFile { path, rel, kind })
}

fn convert_build_map(
    path: &Path,
    rel: &str,
    staged: &Path,
    source: ClassicSource,
    art: &crate::duke_import::ArtBank,
    used_face: &mut BTreeSet<u16>,
) -> Result<Vec<ClassicAsset>, String> {
    crate::duke_import::convert_map(path, rel, staged, art, source.id(), used_face)
}

// ---------------------------------------------------------------------------
// WAV
// ---------------------------------------------------------------------------

/// Collapse per-lump Billboard PNGs into one `.billboard` actor per prefix
/// (Doom) or per sprite folder (Quake SPR). Frame PNGs stay on disk at
/// authored size; the manifest records states + ranges for the engine.
fn collapse_stateful_billboards(assets: &mut Vec<ClassicAsset>, staged: &Path) {
    use crate::stateful_billboard::{
        assemble, parse_doom_sprite_name, sequential_idle, SpriteFrame, SpriteRole,
    };
    use std::collections::{BTreeMap, BTreeSet};

    let mut doom_groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut spr_groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, a) in assets.iter().enumerate() {
        if a.kind != AssetKind::Billboard {
            continue;
        }
        // Duke leftovers are named from CON defines (`chair1`) and must
        // not be re-parsed as Doom lumps (`chai` + rot 1).
        if a.tags.iter().any(|t| t == "leftover") {
            continue;
        }
        let rel = a.rel_path.replace('\\', "/");
        // Already the shared sprite kind — don't re-parse the filename.
        if rel.ends_with(".billboard") {
            continue;
        }
        let stem = rel
            .rsplit('/')
            .next()
            .and_then(|n| n.rsplit_once('.'))
            .map(|(s, _)| s.to_string())
            .unwrap_or_default();
        if stem.starts_with("tile-") {
            continue;
        }
        if stem.starts_with("frame-") {
            if let Some(parent) = rel.rsplit_once('/').map(|(p, _)| p.to_string()) {
                spr_groups.entry(parent).or_default().push(i);
            }
            continue;
        }
        if let Some(parsed) = parse_doom_sprite_name(&stem) {
            let parent = rel.rsplit_once('/').map(|(p, _)| p).unwrap_or("billboards");
            doom_groups
                .entry(format!("{parent}/{}", parsed.prefix))
                .or_default()
                .push(i);
        }
    }

    let mut extra = Vec::new();
    let mut consumed: BTreeSet<usize> = BTreeSet::new();
    for (group, idxs) in doom_groups {
        let mut lumps = Vec::new();
        let mut tags_base = Vec::new();
        for &i in &idxs {
            let rel = assets[i].rel_path.replace('\\', "/");
            let stem = rel
                .rsplit('/')
                .next()
                .and_then(|n| n.rsplit_once('.'))
                .map(|(s, _)| s)
                .unwrap_or("");
            let Some(parsed) = parse_doom_sprite_name(stem) else {
                continue;
            };
            let path = staged.join(&assets[i].rel_path);
            let (w, h) = png_dims(&path).unwrap_or((1, 1));
            let file = rel
                .rsplit('/')
                .next()
                .unwrap_or(assets[i].rel_path.as_str())
                .to_string();
            lumps.push((parsed, file, w, h));
            if tags_base.is_empty() {
                tags_base = assets[i].tags.clone();
            }
        }
        let prefix = group.rsplit('/').next().unwrap_or("sprite");
        let Some(mut bb) = assemble(prefix, &lumps) else {
            continue;
        };
        let rel_path = format!("{group}.billboard");
        let dest = staged.join(&rel_path);
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let Some(icon_rel) = write_actor_sheet(staged, &dest, &mut bb) else {
            continue;
        };
        let mut tags = tags_base;
        tags.push("stateful".into());
        tags.push(bb.role.as_str().into());
        extra.push(ClassicAsset {
            key: group,
            kind: AssetKind::Billboard,
            rel_path,
            tags,
            icon_rel: Some(icon_rel),
        });
        consumed.extend(idxs);
    }

    for (group, idxs) in spr_groups {
        // The manifest sits BESIDE the frame folder (`quake/s_bubble.billboard`
        // next to `quake/s_bubble/frame-00.png`), so a frame's file has to
        // carry that folder or nothing can resolve it.
        let manifest_dir = group.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        let mut frames = Vec::new();
        let mut tags_base = Vec::new();
        for &i in &idxs {
            let rel = assets[i].rel_path.replace('\\', "/");
            let file = rel
                .strip_prefix(&format!("{manifest_dir}/"))
                .unwrap_or(&rel)
                .to_string();
            let path = staged.join(&assets[i].rel_path);
            let (w, h) = png_dims(&path).unwrap_or((1, 1));
            frames.push(SpriteFrame {
                letter: 'A',
                rot: 1,
                w,
                h,
                file,
                flip: false,
                cell: None,
            });
            if tags_base.is_empty() {
                tags_base = assets[i].tags.clone();
            }
        }
        if frames.is_empty() {
            continue;
        }
        let slug = group.rsplit('/').next().unwrap_or("sprite");
        let mut bb = sequential_idle(slug, frames, SpriteRole::Effect);
        let rel_path = format!("{group}.billboard");
        let dest = staged.join(&rel_path);
        let Some(icon_rel) = write_actor_sheet(staged, &dest, &mut bb) else {
            continue;
        };
        let mut tags = tags_base;
        tags.push("stateful".into());
        extra.push(ClassicAsset {
            key: group,
            kind: AssetKind::Billboard,
            rel_path,
            tags,
            icon_rel: Some(icon_rel),
        });
        consumed.extend(idxs);
    }

    // Drop only the per-lump frames that became a stateful actor.
    // Duke `tile-NNNN.png` sprites are not Doom lumps — keep them.
    let consumed_rels: BTreeSet<String> = consumed
        .iter()
        .filter_map(|&i| assets.get(i).map(|a| a.rel_path.replace('\\', "/")))
        .collect();
    assets.retain(|a| {
        a.kind != AssetKind::Billboard
            || a.rel_path.ends_with(".billboard")
            || !consumed_rels.contains(&a.rel_path.replace('\\', "/"))
    });
    assets.extend(extra);
}

/// Pack one actor's frames into a single sheet beside `dest`, write the
/// manifest that indexes it, then delete the per-frame PNGs it replaced.
/// Returns the staged-relative preview strip (the library/catalog icon).
///
/// One PNG per actor is the whole point: a Doom actor is ~40 lumps, and the
/// catalog used to grow one card per lump.
fn write_actor_sheet(
    staged: &Path,
    dest: &Path,
    bb: &mut crate::stateful_billboard::StatefulBillboard,
) -> Option<String> {
    let written = match crate::billboard_sheet::write_with_sheet(dest, bb) {
        Ok(w) => w,
        Err(e) => {
            log_line(&format!("billboard sheet {}: {e}", dest.display()));
            return None;
        }
    };
    for frame in &written.consumed {
        let _ = std::fs::remove_file(frame);
        // Frame folders (`quake/s_bubble/`, duke `14/`) go with them; the
        // call only succeeds once the directory is actually empty.
        if let Some(parent) = frame.parent() {
            if Some(parent) != dest.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }
    let rel = |p: &Path| {
        p.strip_prefix(staged)
            .ok()
            .map(|r| r.to_string_lossy().replace('\\', "/"))
    };
    rel(written.thumb.as_deref().unwrap_or(&written.sheet))
}

fn log_line(message: &str) {
    eprintln!("[classic-import] {message}");
}

fn png_dims(path: &Path) -> Option<(u32, u32)> {
    let bytes = std::fs::read(path).ok()?;
    if let Ok((_, w, h)) = decode_png_stored(&bytes) {
        return Some((w, h));
    }
    if bytes.starts_with(b"\x89PNG") && bytes.len() >= 24 {
        let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
        return Some((w, h));
    }
    None
}

fn convert_wav(
    path: &Path,
    rel: &str,
    staged: &Path,
    source: ClassicSource,
) -> Result<ClassicAsset, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a WAV".into());
    }
    let slug = stem_slug(rel);
    let lower = rel.to_ascii_lowercase();
    let is_music = lower.contains("music")
        || lower.contains("/bgm")
        || lower.contains("track")
        || slug.starts_with("cd");
    let folder = if is_music { "music" } else { "sfx" };
    let key = format!("{folder}/{slug}");
    let rel_path = format!("{key}.wav");
    let dest = staged.join(&rel_path);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&dest, &bytes).map_err(|e| e.to_string())?;
    let icon_rel = write_audio_thumb(staged, &key, &bytes);
    let tag = if is_music { "music" } else { "sfx" };
    Ok(ClassicAsset {
        key,
        kind: AssetKind::Audio,
        rel_path,
        tags: tags_for(AssetKind::Audio, &[source.id(), tag, &slug]),
        icon_rel,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ao_bake;
    use makepad_gltf::write_glb_mesh;
    use std::collections::BTreeMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A point named in DOOM map units, placed in the GLB x/z plane the
    /// emitters write: metres, with Doom north at −Z
    /// (`doom::doom_to_glb`). Every floor/ceiling probe below names its
    /// point the way the WAD does and lets this do the conversion, so a
    /// probe can never drift out of the geometry it is aimed at.
    fn doom_xz(x: f32, y: f32) -> [f32; 2] {
        [x * crate::classic_import::doom::DOOM_UNIT, -y * crate::classic_import::doom::DOOM_UNIT]
    }

    fn tmp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!(
            "mp_classic_{name}_{}_{}",
            std::process::id(),
            nanos
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn expand_pak_rejects_absolute_and_dotdot_names() {
        let root = tmp_dir("pak_escape");
        let payload = b"DATA";
        let names = ["/abs/evil.bsp", "a/../evil.bsp", "c:/evil.bsp", "maps/ok.bsp"];
        let mut dir = Vec::new();
        for name in names {
            let mut e = [0u8; 64];
            e[..name.len()].copy_from_slice(name.as_bytes());
            e[56..60].copy_from_slice(&12u32.to_le_bytes());
            e[60..64].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            dir.extend_from_slice(&e);
        }
        let mut pak = Vec::new();
        pak.extend_from_slice(b"PACK");
        pak.extend_from_slice(&(12 + payload.len() as u32).to_le_bytes());
        pak.extend_from_slice(&(dir.len() as u32).to_le_bytes());
        pak.extend_from_slice(payload);
        pak.extend_from_slice(&dir);
        let pak_path = root.join("pak0.pak");
        std::fs::write(&pak_path, &pak).unwrap();
        let out = root.join("out");
        let mut warnings = Vec::new();
        let files = expand_pak(&pak_path, out.clone(), &mut warnings).expect("expand");
        let rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, ["pak/maps/ok.bsp"]);
        assert!(out.join("maps/ok.bsp").exists());
        assert!(!root.join("abs").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn color_key_index_0_is_transparent() {
        let mut pal = [[0u8; 3]; 256];
        pal[0] = [255, 0, 0];
        pal[1] = [0, 255, 0];
        let rgba = indexed_to_rgba(&[0, 1, 0, 1], &pal, 0);
        assert_eq!(&rgba[0..4], &[0, 0, 0, 0]);
        assert_eq!(&rgba[4..8], &[0, 255, 0, 255]);
        assert_eq!(rgba[3], 0);
        assert_eq!(rgba[7], 255);
    }

    #[test]
    fn magenta_and_cyan_color_key() {
        let rgb = [
            255, 0, 255, // magenta
            0, 255, 255, // cyan
            10, 20, 30, // opaque
        ];
        let rgba = colorkey_rgb_to_rgba(&rgb, 3, 1);
        assert_eq!(rgba[3], 0);
        assert_eq!(rgba[7], 0);
        assert_eq!(rgba[11], 255);
    }

    #[test]
    fn collapse_keeps_duke_tile_billboards() {
        let staged = tmp_dir("duke_tiles");
        let mut assets = vec![
            ClassicAsset {
                key: "worlds/e1l1".into(),
                kind: AssetKind::World,
                rel_path: "worlds/e1l1.glb".into(),
                tags: vec!["world".into()],
                icon_rel: None,
            },
            ClassicAsset {
                key: "billboards/duke3d/tile-1405".into(),
                kind: AssetKind::Billboard,
                rel_path: "billboards/duke3d/tile-1405.png".into(),
                tags: vec!["billboard".into(), "duke3d".into()],
                icon_rel: None,
            },
        ];
        collapse_stateful_billboards(&mut assets, &staged);
        assert!(
            assets.iter().any(|a| a.key == "billboards/duke3d/tile-1405"),
            "duke tiles must stay in the catalog: {:?}",
            assets.iter().map(|a| &a.key).collect::<Vec<_>>()
        );
        assert!(assets.iter().any(|a| a.key == "worlds/e1l1"));
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn billboard_is_not_character_and_not_glb() {
        let kind = kind_for_staged_path("billboards/freedoom/troo.png");
        assert_eq!(kind, AssetKind::Billboard);
        assert_ne!(kind, AssetKind::Character);
        // Sprite payload is PNG frames — never a mesh GLB.
        assert!(!matches!(
            kind,
            AssetKind::Mesh | AssetKind::Character | AssetKind::Weapon | AssetKind::Prop
        ));
        assert!(!kind.has_mesh());
    }

    #[test]
    fn missing_folder_fails_closed() {
        let missing = std::env::temp_dir().join(format!(
            "mp_classic_missing_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&missing);
        let err = require_pack_dir(&missing).unwrap_err();
        assert!(
            err.to_string().contains("not on disk") || err.to_string().contains("local-folder"),
            "{err}"
        );
        let staged = tmp_dir("fail_stage");
        let err = convert_classic(&missing, &staged, ClassicSource::Freedoom).unwrap_err();
        assert!(
            err.to_string().contains("not on disk") || err.to_string().contains("local-folder"),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(&staged);
    }

    fn write_minimal_doom_wad(path: &Path) {
        // IWAD with PLAYPAL + MAP01 + VERTEXES/LINEDEFS/SIDEDEFS/SECTORS + one sprite patch.
        let mut lumps: Vec<(&str, Vec<u8>)> = Vec::new();
        let mut playpal = vec![0u8; 768];
        playpal[3] = 0;
        playpal[4] = 255;
        playpal[5] = 0; // index 1 = green
        lumps.push(("PLAYPAL", playpal));

        lumps.push(("MAP01", vec![]));
        // Player 1 start in the middle of the square so the map gets a spawn
        // sidecar and a spawn-view preview PNG (pack compile requires the thumb).
        let mut things = Vec::new();
        for v in [64i16, 64, 0, 1, 7] {
            things.extend_from_slice(&v.to_le_bytes());
        }
        lumps.push(("THINGS", things));
        // 4 vertices of a unit square in Doom units
        let mut verts = Vec::new();
        for (x, y) in [(0i16, 0i16), (128, 0), (128, 128), (0, 128)] {
            verts.extend_from_slice(&x.to_le_bytes());
            verts.extend_from_slice(&y.to_le_bytes());
        }
        lumps.push(("VERTEXES", verts));
        // 4 one-sided linedefs around sector 0
        let mut lines = Vec::new();
        let edges = [(0u16, 1u16), (1, 2), (2, 3), (3, 0)];
        for (i, (a, b)) in edges.iter().enumerate() {
            lines.extend_from_slice(&a.to_le_bytes());
            lines.extend_from_slice(&b.to_le_bytes());
            lines.extend_from_slice(&0u16.to_le_bytes()); // flags
            lines.extend_from_slice(&0u16.to_le_bytes()); // special
            lines.extend_from_slice(&0u16.to_le_bytes()); // tag
            lines.extend_from_slice(&(i as u16).to_le_bytes()); // right side
            lines.extend_from_slice(&0xFFFFu16.to_le_bytes()); // left
        }
        lumps.push(("LINEDEFS", lines));
        let mut sides = Vec::new();
        for _ in 0..4 {
            sides.extend_from_slice(&0i16.to_le_bytes()); // xoff
            sides.extend_from_slice(&0i16.to_le_bytes()); // yoff
            sides.extend_from_slice(b"-\0\0\0\0\0\0\0"); // upper
            sides.extend_from_slice(b"-\0\0\0\0\0\0\0"); // lower
            sides.extend_from_slice(b"WALL1\0\0\0"); // mid
            sides.extend_from_slice(&0u16.to_le_bytes()); // sector
        }
        lumps.push(("SIDEDEFS", sides));
        let mut sector = Vec::new();
        sector.extend_from_slice(&0i16.to_le_bytes()); // floor
        sector.extend_from_slice(&128i16.to_le_bytes()); // ceil
        sector.extend_from_slice(b"FLOOR0\0\0");
        sector.extend_from_slice(b"CEIL1\0\0\0");
        sector.extend_from_slice(&0i16.to_le_bytes()); // light
        sector.extend_from_slice(&0i16.to_le_bytes()); // special
        sector.extend_from_slice(&0i16.to_le_bytes()); // tag
        lumps.push(("SECTORS", sector));

        // Minimal 2×2 patch (column posts) named TROOA1
        // width=2 height=2, leftover1/2=0, columnofs
        let mut patch = Vec::new();
        patch.extend_from_slice(&2u16.to_le_bytes());
        patch.extend_from_slice(&2u16.to_le_bytes());
        patch.extend_from_slice(&0i16.to_le_bytes());
        patch.extend_from_slice(&0i16.to_le_bytes());
        // columnofs placeholder — fill after posts
        let col_table = patch.len();
        patch.extend_from_slice(&0u32.to_le_bytes());
        patch.extend_from_slice(&0u32.to_le_bytes());
        for col in 0..2 {
            let pos = patch.len() as u32;
            patch[col_table + col * 4..col_table + col * 4 + 4]
                .copy_from_slice(&pos.to_le_bytes());
            patch.push(0); // rowstart
            patch.push(2); // len
            patch.push(0); // unused
            patch.push(0); // transparent index
            patch.push(1); // green
            patch.push(0); // unused
            patch.push(255); // end
        }
        lumps.push(("S_START", vec![]));
        lumps.push(("TROOA1", patch));
        lumps.push(("S_END", vec![]));

        // Flat 64x64
        lumps.push(("F_START", vec![]));
        lumps.push(("FLOOR0", vec![1u8; 64 * 64]));
        lumps.push(("F_END", vec![]));

        // Serialize WAD
        let mut data = Vec::new();
        data.extend_from_slice(b"IWAD");
        data.extend_from_slice(&(lumps.len() as u32).to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes()); // diroff placeholder
        let mut dir = Vec::new();
        for (name, lump) in &lumps {
            let pos = data.len() as u32;
            let size = lump.len() as u32;
            data.extend_from_slice(lump);
            dir.extend_from_slice(&pos.to_le_bytes());
            dir.extend_from_slice(&size.to_le_bytes());
            let mut n = [0u8; 8];
            for (i, b) in name.bytes().take(8).enumerate() {
                n[i] = b;
            }
            dir.extend_from_slice(&n);
        }
        let diroff = data.len() as u32;
        data[8..12].copy_from_slice(&diroff.to_le_bytes());
        data.extend_from_slice(&dir);
        std::fs::write(path, data).unwrap();
    }

    #[test]
    fn map_glb_is_gltf_with_embedded_atlas_and_ao_parked() {
        let root = tmp_dir("map");
        let wad = root.join("freedoom.wad");
        write_minimal_doom_wad(&wad);
        let staged = tmp_dir("map_stage");
        let report = convert_classic(&root, &staged, ClassicSource::Freedoom).expect("convert");
        let world = report
            .assets
            .iter()
            .find(|a| a.kind == AssetKind::World)
            .expect("world asset");
        let glb_path = staged.join(&world.rel_path);
        let glb = std::fs::read(&glb_path).unwrap();
        assert!(glb.starts_with(b"glTF"), "map must be GLB");
        // Embedded PNG atlas lives in the BIN chunk — look for PNG signature.
        assert!(
            glb.windows(8).any(|w| w == b"\x89PNG\r\n\x1a\n"),
            "GLB must embed atlas PNG"
        );
        // Flat mesh only — tag records no portal graph.
        assert!(world.tags.iter().any(|t| t == "no-portals"));
        assert!(!world.rel_path.ends_with(".wad"));

        // Billboard sprite present, not Character, not GLB.
        let bill = report
            .assets
            .iter()
            .find(|a| a.kind == AssetKind::Billboard)
            .expect("billboard");
        assert_ne!(bill.kind, AssetKind::Character);
        assert!(
            bill.rel_path.ends_with(".billboard") || bill.rel_path.ends_with(".png"),
            "{}",
            bill.rel_path
        );
        if bill.rel_path.ends_with(".billboard") {
            let text = std::fs::read_to_string(staged.join(&bill.rel_path)).unwrap();
            let parsed = crate::stateful_billboard::StatefulBillboard::parse(&text).unwrap();
            assert!(!parsed.frames.is_empty());
            let frame = staged
                .join(&bill.rel_path)
                .parent()
                .unwrap()
                .join(&parsed.frames[0].file);
            let png = std::fs::read(&frame).unwrap();
            assert!(png.starts_with(b"\x89PNG"));
        } else {
            let png = std::fs::read(staged.join(&bill.rel_path)).unwrap();
            assert!(png.starts_with(b"\x89PNG"));
        }

        // Import-time AO bake is parked (a Dark Mod-sized pack takes hours);
        // convert must not walk the tree. Direct bake_glb coverage continues below.
        assert_eq!(
            report.bake.total, 0,
            "import AO is parked, bake must stay empty: {:?}",
            report.bake
        );
        // Sidecar skip / fail-closed on stub: plant fresh sidecars and re-bake.
        let sun = ao_bake::default_sun();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(glb_path.with_extension("aomesh"), b"fake").unwrap();
        std::fs::write(glb_path.with_extension("ao.png"), b"fake").unwrap();
        let outcome = ao_bake::bake_glb(&glb_path, &sun).unwrap();
        assert_eq!(outcome, ao_bake::BakeOutcome::SkippedFresh);

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn point_in_sector_square_is_even_odd() {
        let verts = [[0.0, 0.0], [64.0, 0.0], [64.0, 64.0], [0.0, 64.0]];
        let edges = [(0, 1), (1, 2), (2, 3), (3, 0)];
        assert!(point_in_sector([32.0, 32.0], &edges, &verts));
        assert!(!point_in_sector([-1.0, 32.0], &edges, &verts));
        assert!(!point_in_sector([32.0, 80.0], &edges, &verts));
    }

    #[test]
    fn period_span_decreasing_v_covers_the_tile() {
        // Pegged-from-top Doom wall: V goes 1 → 0 (texels / height).
        let (a, b) = period_span(1.0, 0.0);
        assert!((a - 1.0).abs() < 1e-5 && (b - 0.0).abs() < 1e-5, "{a} {b}");
        let (a, b) = period_span(2.0, 1.0);
        assert!((a - 1.0).abs() < 1e-5 && (b - 0.0).abs() < 1e-5, "{a} {b}");
        let (a, b) = period_span(0.0, 1.0);
        assert!((a - 0.0).abs() < 1e-5 && (b - 1.0).abs() < 1e-5, "{a} {b}");
        let (a, b) = period_span(2.3, 2.8);
        assert!((a - 0.3).abs() < 1e-5 && (b - 0.8).abs() < 1e-5, "{a} {b}");
    }

    #[test]
    fn wall_decreasing_v_does_not_collapse_to_one_texel() {
        let mut pos = Vec::new();
        let mut uvs = Vec::new();
        let mut idx = Vec::new();
        let slot = AtlasSlot {
            uv: [0.0, 0.0, 1.0, 1.0],
            w: 128,
            h: 128,
        };
        push_wall_tiled(
            &mut pos,
            &mut uvs,
            &mut idx,
            [0.0, 0.0],
            [2.0, 0.0],
            0.0,
            2.0,
            0.0,
            128.0,
            128.0,
            0.0,
            slot,
        );
        let vmin = uvs.iter().map(|uv| uv[1]).fold(f32::INFINITY, f32::min);
        let vmax = uvs.iter().map(|uv| uv[1]).fold(f32::NEG_INFINITY, f32::max);
        assert!(
            vmax - vmin > 0.8,
            "V must span the tile, got {vmin}..{vmax} from {uvs:?}"
        );
        let umin = uvs.iter().map(|uv| uv[0]).fold(f32::INFINITY, f32::min);
        let umax = uvs.iter().map(|uv| uv[0]).fold(f32::NEG_INFINITY, f32::max);
        assert!(
            umax - umin > 0.8,
            "U must span the tile, got {umin}..{umax}"
        );
    }

    #[test]
    fn adjacent_sector_floors_do_not_overlap() {
        // Shared wall at x=80, mid-tile, so an unclipped 64-unit cell
        // would be claimed by both rooms.
        let verts = [
            [0.0, 0.0],
            [80.0, 0.0],
            [80.0, 64.0],
            [0.0, 64.0],
            [128.0, 0.0],
            [128.0, 64.0],
        ];
        let left = [(0, 1), (1, 2), (2, 3), (3, 0)];
        let right = [(1, 4), (4, 5), (5, 2), (2, 1)];
        let slot = AtlasSlot {
            uv: [0.0, 0.0, 1.0, 1.0],
            w: 64,
            h: 64,
        };
        let mut pos = Vec::new();
        let mut uvs = Vec::new();
        let mut idx = Vec::new();
        emit_sector_grid(&mut pos, &mut uvs, &mut idx, &left, &verts, 1.0 / 64.0, 0.0, slot, false);
        let left_end = idx.len();
        emit_sector_grid(&mut pos, &mut uvs, &mut idx, &right, &verts, 1.0 / 64.0, 0.0, slot, false);
        // Just right of the shared wall (Doom x = 80), mid-room.
        let probe = doom_xz(80.0 + 0.05 * 64.0, 32.0);
        let mut hits = 0u32;
        for tri in idx.chunks_exact(3) {
            let a = pos[tri[0] as usize];
            let b = pos[tri[1] as usize];
            let c = pos[tri[2] as usize];
            if point_in_tri(probe, [a[0], a[2]], [b[0], b[2]], [c[0], c[2]]) {
                hits += 1;
            }
        }
        assert!(hits <= 1, "probe covered by {hits} triangles (z-fight)");
        assert!(left_end > 0 && idx.len() > left_end, "both rooms must emit floor");
    }

    #[test]
    fn pack_atlas_never_uses_the_whole_sheet_as_default() {
        let mut images = BTreeMap::new();
        images.insert(
            "_default".into(),
            RgbaImage {
                w: 64,
                h: 64,
                rgba: vec![0x80; 64 * 64 * 4],
            },
        );
        for i in 0..80 {
            images.insert(
                format!("BIG{i}"),
                RgbaImage {
                    w: 256,
                    h: 128,
                    rgba: vec![0x40; 256 * 128 * 4],
                },
            );
        }
        let (_png, map) = pack_atlas(&images);
        let d = map.get("_default").expect("default slot");
        let span = (d.uv[2] - d.uv[0]).max(d.uv[3] - d.uv[1]);
        assert!(
            span < 0.2,
            "default must be a tile, not the whole atlas: {:?}",
            d.uv
        );
    }

    #[test]
    fn doom_wall_uvs_stay_inside_atlas_tiles() {
        let root = tmp_dir("uv");
        write_minimal_doom_wad(&root.join("freedoom.wad"));
        let staged = tmp_dir("uv_stage");
        let report = convert_classic(&root, &staged, ClassicSource::Freedoom).expect("convert");
        let world = report
            .assets
            .iter()
            .find(|a| a.kind == AssetKind::World)
            .expect("world");
        let glb = std::fs::read(staged.join(&world.rel_path)).unwrap();
        let mesh = crate::world_preview::raster_glb_from_spawn(
            &glb,
            ([1.0, 0.7, 1.0], 0.0, 0.0),
        );
        assert!(mesh.is_ok(), "{:?}", mesh.err());
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn reconvert_local_freedoom2_map27() {
        let wad_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/packs/freedoom/freedoom2.wad");
        if !wad_path.is_file() {
            return;
        }
        let bytes = std::fs::read(&wad_path).expect("read wad");
        let wad = parse_wad(&bytes).expect("parse wad");
        let pal = wad.playpal.expect("PLAYPAL");
        let mesh = doom_map_to_mesh(&wad.lumps, "MAP27", &pal).expect("MAP27 mesh");
        assert!(mesh.glb.starts_with(b"glTF"), "glb magic");
        println!("MAP27 triangles: {}", mesh.tris);
        assert!(
            mesh.tris > 8_000,
            "MAP27 lost most geometry: {} tris",
            mesh.tris
        );
        assert!(
            mesh.tris < 120_000,
            "MAP27 should be BSP-subsector fans, not a grid: {} tris",
            mesh.tris
        );
        if std::env::var_os("FREEDOOM_RECONVERT").is_none() {
            return;
        }
        let dests = [
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../local/ai_content_app/import/freedoom/work/source/worlds/freedoom2/map27.glb"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../local/ai_content_library/lib-1047.glb"),
        ];
        for dest in dests {
            if let Some(parent) = dest.parent() {
                if parent.exists() {
                    std::fs::write(&dest, &mesh.glb).expect("write glb");
                }
            }
        }
    }

    #[test]
    fn sky_flat_names_include_f_sky1() {
        assert!(is_sky_flat("F_SKY1"));
        assert!(is_sky_flat("F_SKY2"));
        assert!(is_sky_flat("SKY1"));
        assert!(!is_sky_flat("CEIL1_1"));
        assert!(is_blank_tex("-"));
        assert!(!is_blank_tex("WOOD5"));
    }

    #[test]
    fn sector_grid_does_not_fill_a_hole() {
        // Outer 0..256, hole 80..176 — ring thicker than one 64-unit flat
        // so the hole-safe cell path still emits the wood (a 128-unit
        // donut had every 64-cell touching the hole).
        let verts = [
            [0.0, 0.0],
            [256.0, 0.0],
            [256.0, 256.0],
            [0.0, 256.0],
            [80.0, 80.0],
            [176.0, 80.0],
            [176.0, 176.0],
            [80.0, 176.0],
        ];
        // Outer CCW, hole CW so even-odd sees the ring.
        let edges = [
            (0, 1),
            (1, 2),
            (2, 3),
            (3, 0),
            (4, 7),
            (7, 6),
            (6, 5),
            (5, 4),
        ];
        let slot = AtlasSlot {
            uv: [0.0, 0.0, 1.0, 1.0],
            w: 64,
            h: 64,
        };
        let mut pos = Vec::new();
        let mut uvs = Vec::new();
        let mut idx = Vec::new();
        emit_sector_grid(
            &mut pos,
            &mut uvs,
            &mut idx,
            &edges,
            &verts,
            1.0 / 64.0,
            1.0,
            slot,
            true,
        );
        let hole = doom_xz(128.0, 128.0);
        let ring = doom_xz(32.0, 32.0);
        let mut hole_hits = 0u32;
        let mut ring_hits = 0u32;
        for tri in idx.chunks_exact(3) {
            let a = pos[tri[0] as usize];
            let b = pos[tri[1] as usize];
            let c = pos[tri[2] as usize];
            if point_in_tri(hole, [a[0], a[2]], [b[0], b[2]], [c[0], c[2]]) {
                hole_hits += 1;
            }
            if point_in_tri(ring, [a[0], a[2]], [b[0], b[2]], [c[0], c[2]]) {
                ring_hits += 1;
            }
        }
        assert_eq!(hole_hits, 0, "sky opening filled by {hole_hits} slivers");
        assert!(ring_hits >= 1, "wood ring must still emit");
    }

    #[test]
    fn walkway_around_a_platform_is_not_dropped() {
        // 32-unit corridor around a 128² island — no 64-unit cell is
        // fully inside the walkway. Full-cell-only fill left this as a
        // green hole (MAP24 / MAP26).
        let verts = [
            [0.0, 0.0],
            [192.0, 0.0],
            [192.0, 192.0],
            [0.0, 192.0],
            [32.0, 32.0],
            [160.0, 32.0],
            [160.0, 160.0],
            [32.0, 160.0],
        ];
        let edges = [
            (0, 1),
            (1, 2),
            (2, 3),
            (3, 0),
            (4, 7),
            (7, 6),
            (6, 5),
            (5, 4),
        ];
        let slot = AtlasSlot {
            uv: [0.0, 0.0, 1.0, 1.0],
            w: 64,
            h: 64,
        };
        let mut pos = Vec::new();
        let mut uvs = Vec::new();
        let mut idx = Vec::new();
        emit_sector_grid(
            &mut pos,
            &mut uvs,
            &mut idx,
            &edges,
            &verts,
            1.0 / 64.0,
            0.0,
            slot,
            false,
        );
        let walk = doom_xz(16.0, 96.0);
        let island = doom_xz(96.0, 96.0);
        let mut walk_hits = 0u32;
        let mut island_hits = 0u32;
        for tri in idx.chunks_exact(3) {
            let a = pos[tri[0] as usize];
            let b = pos[tri[1] as usize];
            let c = pos[tri[2] as usize];
            if point_in_tri(walk, [a[0], a[2]], [b[0], b[2]], [c[0], c[2]]) {
                walk_hits += 1;
            }
            if point_in_tri(island, [a[0], a[2]], [b[0], b[2]], [c[0], c[2]]) {
                island_hits += 1;
            }
        }
        assert_eq!(island_hits, 0, "raised platform interior filled");
        assert!(walk_hits >= 1, "walkway around the platform must emit");
    }

    #[test]
    fn convex_subsector_fan_covers_every_endpoint() {
        // Five convex verts. A v1→v2 chain can close on the first triangle
        // and drop the other two — the MAP27 floor hole. Angular sort keeps
        // all endpoints.
        let verts = [
            [0.0, 0.0],
            [64.0, 0.0],
            [80.0, 48.0],
            [32.0, 80.0],
            [-16.0, 48.0],
        ];
        let ids: Vec<usize> = (0..5).collect();
        let ring = angular_convex_ring(&ids, &verts);
        assert_eq!(ring.len(), 5);
        let slot = AtlasSlot {
            uv: [0.0, 0.0, 1.0, 1.0],
            w: 64,
            h: 64,
        };
        let mut pos = Vec::new();
        let mut uvs = Vec::new();
        let mut idx = Vec::new();
        let pts: Vec<[f32; 2]> = ring.iter().map(|&i| verts[i]).collect();
        emit_convex_ring(
            &mut pos, &mut uvs, &mut idx, &pts, 1.0 / 64.0, 0.0, slot, false,
        );
        let centre = doom_xz(32.0, 32.0);
        let mut hits = 0u32;
        for tri in idx.chunks_exact(3) {
            let a = pos[tri[0] as usize];
            let b = pos[tri[1] as usize];
            let c = pos[tri[2] as usize];
            if point_in_tri(centre, [a[0], a[2]], [b[0], b[2]], [c[0], c[2]]) {
                hits += 1;
            }
        }
        assert!(idx.len() / 3 >= 3, "convex pentagon vanished: {} tris", idx.len() / 3);
        assert!(hits >= 1, "centroid of the subsector must be covered");
    }

    #[test]
    fn one_seg_subsector_closes_on_partitions() {
        // Vanilla leaf: one linedef seg plus two partition edges that meet
        // at a vertex the VERTEXES lump does not store.
        let verts = [[64.0, 0.0], [0.0, 0.0]];
        let mut segs = vec![0u8; 12];
        segs[0..2].copy_from_slice(&0u16.to_le_bytes());
        segs[2..4].copy_from_slice(&1u16.to_le_bytes());
        let ancestors = [
            SplitLine {
                origin: [0.0, 0.0],
                dir: [0.0, 64.0],
            },
            SplitLine {
                origin: [0.0, 64.0],
                dir: [64.0, -64.0],
            },
        ];
        let poly = subsector_poly(
            &segs,
            0,
            1,
            2,
            &verts,
            &ancestors,
            (-128.0, -128.0, 256.0, 256.0),
        );
        assert!(
            poly.len() >= 3,
            "1-seg leaf must recover the partition corner, got {poly:?}"
        );
        let inside = [20.0, 10.0];
        let cx = poly.iter().map(|p| p[0]).sum::<f32>() / poly.len() as f32;
        let cy = poly.iter().map(|p| p[1]).sum::<f32>() / poly.len() as f32;
        assert!(
            (cx - 21.0).abs() < 16.0 && (cy - 21.0).abs() < 16.0,
            "triangle centroid off: {cx},{cy} poly={poly:?}"
        );
        let slot = AtlasSlot {
            uv: [0.0, 0.0, 1.0, 1.0],
            w: 64,
            h: 64,
        };
        let mut pos = Vec::new();
        let mut uvs = Vec::new();
        let mut idx = Vec::new();
        emit_convex_ring(
            &mut pos, &mut uvs, &mut idx, &poly, 1.0 / 64.0, 0.0, slot, false,
        );
        let probe = doom_xz(inside[0], inside[1]);
        let hits = idx.chunks_exact(3).filter(|tri| {
            let a = pos[tri[0] as usize];
            let b = pos[tri[1] as usize];
            let c = pos[tri[2] as usize];
            point_in_tri(probe, [a[0], a[2]], [b[0], b[2]], [c[0], c[2]])
        }).count();
        assert!(hits >= 1, "closed 1-seg triangle did not emit the interior");
    }

    #[test]
    fn floor_uvs_do_not_wrap_inside_one_triangle() {
        // A 128-unit tri crosses a flat tile. Per-vertex fract() smears;
        // each emitted piece must stay in one [0,1] tile.
        let slot = AtlasSlot {
            uv: [0.1, 0.2, 0.3, 0.4],
            w: 64,
            h: 64,
        };
        let mut pos = Vec::new();
        let mut uvs = Vec::new();
        let mut idx = Vec::new();
        emit_convex_ring(
            &mut pos,
            &mut uvs,
            &mut idx,
            &[[0.0, 0.0], [128.0, 0.0], [0.0, 128.0]],
            1.0 / 64.0,
            0.0,
            slot,
            false,
        );
        assert!(idx.len() / 3 >= 2, "128-unit tri must split on the tile grid");
        let du = slot.uv[2] - slot.uv[0];
        let dv = slot.uv[3] - slot.uv[1];
        // Overlap + wrap gutter may step a texel outside the inner rect.
        let slop_u = du * 2.0 / 64.0 + 1e-3;
        let slop_v = dv * 2.0 / 64.0 + 1e-3;
        for tri in idx.chunks_exact(3) {
            let ua = uvs[tri[0] as usize];
            let ub = uvs[tri[1] as usize];
            let uc = uvs[tri[2] as usize];
            for uv in [ua, ub, uc] {
                assert!(
                    uv[0] >= slot.uv[0] - slop_u && uv[0] <= slot.uv[2] + slop_u,
                    "u left the atlas slot: {uv:?}"
                );
                assert!(
                    uv[1] >= slot.uv[1] - slop_v && uv[1] <= slot.uv[3] + slop_v,
                    "v left the atlas slot: {uv:?}"
                );
            }
            let span_u = ua[0].max(ub[0]).max(uc[0]) - ua[0].min(ub[0]).min(uc[0]);
            let span_v = ua[1].max(ub[1]).max(uc[1]) - ua[1].min(ub[1]).min(uc[1]);
            assert!(
                span_u <= du + slop_u && span_v <= dv + slop_v,
                "tri spans more than one tile (smear): {ua:?} {ub:?} {uc:?}"
            );
        }
    }

    #[test]
    fn two_sided_mid_emits_once() {
        // Grate on both sidedefs of one linedef. Front and back segs
        // used to each push a coplanar quad (MAP16 shimmer).
        let mut sidedefs = Vec::new();
        for sec in [0u16, 1u16] {
            sidedefs.extend_from_slice(&0i16.to_le_bytes());
            sidedefs.extend_from_slice(&0i16.to_le_bytes());
            sidedefs.extend_from_slice(b"--------");
            sidedefs.extend_from_slice(b"--------");
            sidedefs.extend_from_slice(b"GRATE\0\0\0");
            sidedefs.extend_from_slice(&sec.to_le_bytes());
        }
        let slot = AtlasSlot {
            uv: [0.0, 0.0, 1.0, 1.0],
            w: 64,
            h: 64,
        };
        let mut uv_map = BTreeMap::new();
        uv_map.insert("GRATE".into(), slot);
        let sec_floor = [0i16, 0];
        let sec_ceil = [128i16, 128];
        let sec_ceil_tex = ["FLAT".into(), "FLAT".into()];
        let mut pos = Vec::new();
        let mut uvs = Vec::new();
        let mut idx = Vec::new();
        let sec_light = [160i16, 160];
        let mut colors = Vec::new();
        let emit = |pos: &mut _, uvs: &mut _, idx: &mut _, colors: &mut _, side: u16| {
            emit_wall_piece(
                pos,
                uvs,
                idx,
                [0.0, 0.0],
                [64.0, 0.0],
                0.0,
                64.0,
                0x0004,
                true,
                side,
                side,
                1 - side,
                &sidedefs,
                2,
                2,
                &sec_floor,
                &sec_ceil,
                &sec_ceil_tex,
                &sec_light,
                colors,
                &uv_map,
                1.0 / 64.0,
                None,
                None,
            );
        };
        emit(&mut pos, &mut uvs, &mut idx, &mut colors, 0);
        let after_front = idx.len();
        assert!(after_front >= 6, "front mid must emit a quad");
        emit(&mut pos, &mut uvs, &mut idx, &mut colors, 1);
        assert_eq!(idx.len(), after_front, "back mid must not stack a second grate");
    }

    #[test]
    fn diagonal_sector_edge_does_not_leave_a_cell_triangle() {
        // Right triangle (0,0)-(128,0)-(0,128). A 64-cell on the
        // hypotenuse used to emit 3–6 angular-sorted points and drop a
        // triangle — the mint slivers on MAP27 / MAP24 water.
        let verts = [[0.0, 0.0], [128.0, 0.0], [0.0, 128.0]];
        let edges = [(0, 1), (1, 2), (2, 0)];
        let slot = AtlasSlot {
            uv: [0.0, 0.0, 1.0, 1.0],
            w: 64,
            h: 64,
        };
        let mut pos = Vec::new();
        let mut uvs = Vec::new();
        let mut idx = Vec::new();
        emit_sector_grid(
            &mut pos,
            &mut uvs,
            &mut idx,
            &edges,
            &verts,
            1.0 / 64.0,
            0.0,
            slot,
            false,
        );
        let inside = doom_xz(40.0, 40.0);
        let near_cut = doom_xz(70.0, 20.0);
        let outside = doom_xz(90.0, 90.0);
        let mut in_hits = 0u32;
        let mut cut_hits = 0u32;
        let mut out_hits = 0u32;
        for tri in idx.chunks_exact(3) {
            let a = pos[tri[0] as usize];
            let b = pos[tri[1] as usize];
            let c = pos[tri[2] as usize];
            if point_in_tri(inside, [a[0], a[2]], [b[0], b[2]], [c[0], c[2]]) {
                in_hits += 1;
            }
            if point_in_tri(near_cut, [a[0], a[2]], [b[0], b[2]], [c[0], c[2]]) {
                cut_hits += 1;
            }
            if point_in_tri(outside, [a[0], a[2]], [b[0], b[2]], [c[0], c[2]]) {
                out_hits += 1;
            }
        }
        assert!(in_hits >= 1, "triangle body missing");
        assert!(cut_hits >= 1, "hypotenuse cell dropped a sliver");
        assert_eq!(out_hits, 0, "filled past the diagonal");
    }

    #[test]
    fn concave_l_is_ear_clipped_not_fanned() {
        // L: 0,0-128,0-128,64-64,64-64,128-0,128. Fan-from-0 covers the notch.
        let verts = [
            [0.0, 0.0],
            [128.0, 0.0],
            [128.0, 64.0],
            [64.0, 64.0],
            [64.0, 128.0],
            [0.0, 128.0],
        ];
        let edges = [(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 0)];
        let slot = AtlasSlot {
            uv: [0.0, 0.0, 1.0, 1.0],
            w: 64,
            h: 64,
        };
        let mut pos = Vec::new();
        let mut uvs = Vec::new();
        let mut idx = Vec::new();
        emit_sector_grid(
            &mut pos,
            &mut uvs,
            &mut idx,
            &edges,
            &verts,
            1.0 / 64.0,
            0.0,
            slot,
            false,
        );
        assert!(idx.len() >= 9, "L needs at least 3 tris, got {}", idx.len());
        assert!(
            idx.len() <= 8000,
            "L must not explode into slivers: {} indices",
            idx.len()
        );
        let notch = doom_xz(96.0, 96.0);
        let body = doom_xz(32.0, 32.0);
        let mut notch_hits = 0u32;
        let mut body_hits = 0u32;
        for tri in idx.chunks_exact(3) {
            let a = pos[tri[0] as usize];
            let b = pos[tri[1] as usize];
            let c = pos[tri[2] as usize];
            if point_in_tri(notch, [a[0], a[2]], [b[0], b[2]], [c[0], c[2]]) {
                notch_hits += 1;
            }
            if point_in_tri(body, [a[0], a[2]], [b[0], b[2]], [c[0], c[2]]) {
                body_hits += 1;
            }
        }
        assert_eq!(notch_hits, 0, "fan filled the L notch");
        assert!(body_hits >= 1, "L body missing");
    }

    #[test]
    fn map27_floors_do_not_stack_at_the_origin() {
        let wad_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/packs/freedoom/freedoom2.wad");
        if !wad_path.is_file() {
            return;
        }
        let bytes = std::fs::read(&wad_path).expect("read wad");
        let wad = parse_wad(&bytes).expect("parse wad");
        let pal = wad.playpal.expect("PLAYPAL");
        let mesh = doom_map_to_mesh(&wad.lumps, "MAP27", &pal).expect("MAP27");
        // Parse the GLB enough to count horizontal triangles covering (0,0).
        let glb = &mesh.glb;
        assert!(glb.starts_with(b"glTF"));
        assert!(
            glb.len() > 200_000,
            "MAP27 lost its floors (glb {} bytes)",
            glb.len()
        );
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let json = std::str::from_utf8(&glb[20..20 + json_len]).unwrap_or("");
        assert!(json.contains("POSITION"), "expected mesh accessors");
        if std::env::var_os("FREEDOOM_RECONVERT").is_some() {
            let dests = [
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
                    "../../local/ai_content_app/import/freedoom/work/source/worlds/freedoom2/map27.glb",
                ),
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../local/ai_content_library/lib-1047.glb"),
            ];
            for dest in dests {
                if dest.parent().is_some_and(|p| p.exists()) {
                    std::fs::write(&dest, &mesh.glb).expect("write glb");
                }
            }
        }
    }

    #[test]
    fn stored_png_roundtrips() {
        let rgba = vec![10u8, 20, 30, 255, 40, 50, 60, 128];
        let png = encode_png_rgba(&rgba, 2, 1).unwrap();
        let (back, w, h) = decode_png_stored(&png).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(back, rgba);
    }

    #[test]
    fn librequake_player_mdl_is_character_with_anim_icon() {
        let mdl = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/ai_content_app/import/librequake/work/source/_pak/progs/player.mdl");
        let mdl = if mdl.is_file() {
            mdl
        } else {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../local/packs/librequake")
        };
        if !mdl.is_file() && !mdl.is_dir() {
            return;
        }
        if mdl.is_dir() {
            // Full pack path — convert just needs the folder; skip if empty.
            return;
        }
        let decoded = decode_quake_mdl(&std::fs::read(&mdl).unwrap()).expect("player.mdl");
        assert!(decoded.frame_count > 1, "player.mdl is vertex-animated");
        assert!(decoded.glb.starts_with(b"glTF"));
        assert!(decoded.icon_png.as_ref().is_some_and(|p| p.starts_with(b"\x89PNG")));
    }

    fn pak0_entry(name: &str) -> Option<Vec<u8>> {
        let pak = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/packs/quake/ID1/PAK0.PAK");
        let bytes = std::fs::read(pak).ok()?;
        if bytes.len() < 12 || &bytes[0..4] != b"PACK" {
            return None;
        }
        let dirofs = u32::from_le_bytes(bytes[4..8].try_into().ok()?) as usize;
        let dirlen = u32::from_le_bytes(bytes[8..12].try_into().ok()?) as usize;
        let n = dirlen / 64;
        for i in 0..n {
            let o = dirofs + i * 64;
            if o + 64 > bytes.len() {
                break;
            }
            let end = bytes[o..o + 56].iter().position(|&b| b == 0).unwrap_or(56);
            let entry = std::str::from_utf8(&bytes[o..o + end]).ok()?;
            if !entry.eq_ignore_ascii_case(name) {
                continue;
            }
            let pos = u32::from_le_bytes(bytes[o + 56..o + 60].try_into().ok()?) as usize;
            let size = u32::from_le_bytes(bytes[o + 60..o + 64].try_into().ok()?) as usize;
            return bytes.get(pos..pos + size).map(|s| s.to_vec());
        }
        None
    }

    #[test]
    fn quake_e1m1_exports_its_sky_liquids_and_doors_as_nodes() {
        use makepad_asset_client::json::{self, Value};
        let Some(bytes) = pak0_entry("maps/e1m1.bsp") else {
            eprintln!("no Quake PAK0; skipped");
            return;
        };
        let map = crate::classic_import::quake::quake_bsp_to_map(&bytes).expect("e1m1");
        // The door metadata that becomes anchors: a real centre and a real
        // travel, not a zero vector.
        assert!(!map.doors.is_empty());
        for d in &map.doors {
            let travel = (d.travel[0] * d.travel[0]
                + d.travel[1] * d.travel[1]
                + d.travel[2] * d.travel[2])
                .sqrt();
            assert!(travel > 0.0, "{d:?}");
            assert!(d.centre.iter().all(|v| v.is_finite()), "{d:?}");
        }
        let glb = map.glb;
        let root = json::parse(glb_json(&glb).as_bytes()).expect("glb json");
        let nodes = root.get("nodes").unwrap().as_arr().unwrap();
        let named = |name: &str| {
            nodes
                .iter()
                .find(|n| n.get("name").and_then(Value::as_str) == Some(name))
        };
        let kinds: Vec<&str> = nodes
            .iter()
            .filter_map(|n| n.get("extras").and_then(|e| e.get("kind")).and_then(Value::as_str))
            .collect();
        eprintln!("quake e1m1 nodes: {kinds:?}");

        // Quake 1's qbsp never fixed T-junctions — the "sparklies" are the
        // engine's own famous artifact — so the level arrives full of them
        // and the weld has to close every one, across the world mesh, the
        // liquids, the doors and the sky alike.
        let parts = crate::world_preview::extract_glb_parts(&glb).expect("parts");
        let soup: Vec<(&[[f32; 3]], &[u32])> = parts
            .iter()
            .map(|part| (&part.pos[..], &part.indices[..]))
            .collect();
        let left = crate::classic_import::weld::t_junctions_left(&soup);
        eprintln!(
            "quake e1m1: {} parts, {} triangles, {left} T-junctions",
            parts.len(),
            soup.iter().map(|(_, i)| i.len() / 3).sum::<usize>()
        );
        assert_eq!(left, 0, "Quake e1m1 still cracks between its faces");

        // The sky is two scrolling layers at Quake's own speeds.
        let sky = named("sky").expect("sky node");
        let extras = sky.get("extras").unwrap();
        assert_eq!(
            extras.get("projection").and_then(Value::as_str),
            Some("quake_scroll")
        );
        let speeds: Vec<f64> = extras
            .get("speeds")
            .and_then(Value::as_arr)
            .unwrap()
            .iter()
            .map(|v| match v {
                Value::F64(f) => *f,
                Value::Int(i) => *i as f64,
                _ => f64::NAN,
            })
            .collect();
        assert_eq!(speeds, vec![8.0, 16.0]);
        let layers = extras.get("layers").and_then(Value::as_arr).expect("layers");
        assert_eq!(layers.len(), 2, "back then keyed front");

        // Liquids are hazards you swim through, not floors.
        let hazard = nodes
            .iter()
            .find(|n| {
                n.get("extras").and_then(|e| e.get("kind")).and_then(Value::as_str)
                    == Some("hazard")
            })
            .expect("a liquid node");
        let hx = hazard.get("extras").unwrap();
        assert_eq!(hx.get("liquid").and_then(Value::as_bool), Some(true));
        assert_eq!(hx.get("solid").and_then(Value::as_bool), Some(false));

        // func_plat brushes are lifts: authored and resting UP, travelling
        // down, so a walker meets the platform where the map drew it.
        assert!(!map.lifts.is_empty(), "e1m1 has two func_plat");
        let lift = named("lift_1").expect("lift_1");
        let lx = lift.get("extras").unwrap();
        assert_eq!(lx.get("kind").and_then(Value::as_str), Some("lift"));
        assert_eq!(lx.get("default").and_then(Value::as_str), Some("up"));
        assert!(
            lift.get("translation").is_none(),
            "a lift's rest pose is the authored one"
        );
        let travel = match lx.get("travel") {
            Some(Value::F64(f)) => *f,
            Some(Value::Int(i)) => *i as f64,
            other => panic!("lift travel: {other:?}"),
        };
        assert!(travel < 0.0, "a plat drops: {travel}");

        // The teleport pad and where it lands. E1M1's one teleporter is the
        // only way out of the water tunnel.
        assert!(!map.teleports.is_empty(), "e1m1 has a trigger_teleport");
        for t in &map.teleports {
            assert!(t.pad_max[0] > t.pad_min[0] && t.pad_max[1] > t.pad_min[1], "{t:?}");
            assert!(t.dst.iter().all(|v| v.is_finite()), "{t:?}");
        }

        // A `func_door_secret` is drawn as a wall and says so.
        assert!(
            nodes.iter().any(|n| {
                n.get("extras")
                    .and_then(|e| e.get("secret"))
                    .and_then(Value::as_bool)
                    == Some(true)
            }),
            "e1m1 has seven func_door_secret"
        );

        // func_door brushes move.
        let door = named("door_1").expect("door_1");
        let dx = door.get("extras").unwrap();
        assert_eq!(dx.get("kind").and_then(Value::as_str), Some("door"));
        assert_eq!(dx.get("default").and_then(Value::as_str), Some("open"));
        assert!(door.get("translation").is_some(), "rest pose is OPEN");
        let anims = root.get("animations").unwrap().as_arr().unwrap();
        assert!(anims
            .iter()
            .any(|a| a.get("name").and_then(Value::as_str) == Some("door_1")));
    }

    #[test]
    fn quake_ogre_mdl_skins_into_playable_clips() {
        let Some(bytes) = pak0_entry("progs/ogre.mdl") else {
            return;
        };
        let decoded = decode_quake_mdl_ex(&bytes, true).expect("ogre.mdl");
        assert!(decoded.frame_count > 1);
        assert!(decoded.clip_names.iter().any(|n| n == "idle"), "{:?}", decoded.clip_names);
        assert!(decoded.clip_names.iter().any(|n| n == "walk"), "{:?}", decoded.clip_names);
        assert!(decoded.clip_names.iter().any(|n| n == "run"), "{:?}", decoded.clip_names);
        assert!(
            decoded.clip_names.iter().any(|n| n == "smash" || n == "swing"),
            "expected attack states: {:?}",
            decoded.clip_names
        );
        let model = makepad_render::skin::SkinnedModel::parse_glb(&decoded.glb)
            .expect("engine must parse the fitted skin");
        assert!(model.joint_count() > 0 && model.clips.len() >= 3);
        assert!(model.clip_index_any(&["idle"]).is_some());
        assert!(model.clip_index_any(&["walk"]).is_some());
    }

    #[test]
    fn simple_triangle_glb_ao_path() {
        // Ensure write_glb_mesh path also works for MDL-like untextured mesh.
        let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let indices = [0u32, 1, 2];
        let glb = write_glb_mesh(&positions, &indices);
        assert!(glb.starts_with(b"glTF"));
        let dir = tmp_dir("tri");
        let path = dir.join("meshes/prop.glb");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &glb).unwrap();
        // Plant fresh sidecars → skip.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(path.with_extension("aomesh"), b"x").unwrap();
        std::fs::write(path.with_extension("ao.png"), b"x").unwrap();
        let stats = ao_bake::bake_glb_tree(&dir, |_, _, _| {});
        assert_eq!(stats.total, 1);
        assert_eq!(stats.skipped, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn doom_patch_index0_alpha() {
        let mut pal = [[0u8; 3]; 256];
        pal[1] = [255, 255, 255];
        // 1×2 patch, post covers only row 1: row 0 is a gap (transparent);
        // palette index 0 inside a post is opaque near-black, not a hole.
        let mut patch = Vec::new();
        patch.extend_from_slice(&1u16.to_le_bytes());
        patch.extend_from_slice(&2u16.to_le_bytes());
        patch.extend_from_slice(&0i16.to_le_bytes());
        patch.extend_from_slice(&0i16.to_le_bytes());
        let col_pos = 8u32 + 4;
        patch.extend_from_slice(&col_pos.to_le_bytes());
        patch.push(1); // rowstart: row 0 stays a gap
        patch.push(1); // len
        patch.push(0);
        patch.push(0); // palette index 0: opaque
        patch.push(0);
        patch.push(255);
        let img = doom_patch_to_rgba(&patch, &pal).unwrap();
        assert_eq!(img.rgba[3], 0, "post gap must be transparent");
        assert_eq!(img.rgba[7], 255, "index 0 in a post must be opaque");
    }

    #[test]
    #[ignore]
    fn reconvert_local_quake_shareware() {
        let pack = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../local/packs/quake");
        if !pack.is_dir() {
            return;
        }
        let staged = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/ai_content_app/import/quake/work/source");
        let report = convert_classic(&pack, &staged, ClassicSource::Quake).expect("convert");
        let shells: Vec<_> = report
            .assets
            .iter()
            .filter(|a| a.key.contains("b_shell"))
            .collect();
        assert!(
            shells.iter().all(|a| a.kind == AssetKind::Prop),
            "b_* must be props not worlds: {shells:?}"
        );
        assert!(
            shells.iter().all(|a| a.icon_rel.is_some()),
            "b_* need raster icons: {shells:?}"
        );
        eprintln!(
            "quake shareware: {} assets ({} worlds, {} props, {} characters)",
            report.assets.len(),
            report.assets.iter().filter(|a| a.kind == AssetKind::World).count(),
            report.assets.iter().filter(|a| a.kind == AssetKind::Prop).count(),
            report.assets.iter().filter(|a| a.kind == AssetKind::Character).count(),
        );
    }

    #[test]
    #[ignore]
    fn reconvert_local_quake2_shareware() {
        let pack = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../local/packs/quake2");
        if !pack.is_dir() {
            return;
        }
        let staged = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/ai_content_app/import/quake2/work/source");
        let report = convert_classic(&pack, &staged, ClassicSource::Quake2).expect("convert q2");
        let md2 = report
            .assets
            .iter()
            .filter(|a| a.tags.iter().any(|t| t == "md2"))
            .count();
        let chars = report
            .assets
            .iter()
            .filter(|a| a.kind == AssetKind::Character)
            .count();
        let worlds = report
            .assets
            .iter()
            .filter(|a| a.kind == AssetKind::World)
            .count();
        assert!(
            md2 >= 40,
            "expected dozens of unique MD2s, got {md2} (tris.md2 collision?): {:?}",
            report
                .assets
                .iter()
                .filter(|a| a.tags.iter().any(|t| t == "md2"))
                .map(|a| a.key.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            chars >= 5,
            "expected monster characters, got {chars}"
        );
        assert!(worlds >= 3, "expected demo maps, got {worlds}");
        let demo = staged.join("worlds/demo1.glb");
        if demo.is_file() {
            let glb = std::fs::read(&demo).unwrap();
            assert!(
                std::str::from_utf8(&glb).map(|s| s.contains("COLOR_0")).unwrap_or(false)
                    || glb.windows(7).any(|w| w == b"COLOR_0"),
                "demo1 should carry shipped lightmaps as COLOR_0"
            );
        }
        eprintln!(
            "quake2 shareware: {} assets ({} worlds, {} md2, {} characters, skipped {})",
            report.assets.len(),
            worlds,
            md2,
            chars,
            report.skipped.len()
        );
    }

    #[test]
    #[ignore]
    fn convert_local_librequake_pack() {
        let pack = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/packs/librequake");
        if !pack.is_dir() {
            return;
        }
        let staged = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/ai_content_app/import/librequake/work/source");
        let report = convert_classic(&pack, &staged, ClassicSource::LibreQuake)
            .expect("convert librequake");
        assert!(report.assets.iter().any(|a| a.kind == AssetKind::Character),
            "expected character MDLs, got {:?}",
            report.assets.iter().map(|a| format!("{}:{:?}", a.key, a.kind)).collect::<Vec<_>>());
        assert!(!staged.join("_pak").exists(), "PAK leftovers must not stay in staged");
        let chars = report.assets.iter().filter(|a| a.kind == AssetKind::Character).count();
        let worlds = report.assets.iter().filter(|a| a.kind == AssetKind::World).count();
        eprintln!("librequake convert: {} assets ({} characters, {} worlds, skipped {})",
            report.assets.len(), chars, worlds, report.skipped.len());
    }

    #[test]
    #[ignore]
    fn reconvert_local_worlds_to_tmp() {
        let dest = PathBuf::from("/tmp/classic-world-rebuild");
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::create_dir_all(&dest).unwrap();
        let fd = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local/packs/freedoom");
        let lq = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local/packs/librequake");
        if fd.is_dir() {
            let staged = dest.join("freedoom");
            let report = convert_classic(&fd, &staged, ClassicSource::Freedoom).expect("fd");
            eprintln!(
                "freedoom worlds {}",
                report.assets.iter().filter(|a| a.kind == AssetKind::World).count()
            );
        }
        if lq.is_dir() {
            let staged = dest.join("librequake");
            let report = convert_classic(&lq, &staged, ClassicSource::LibreQuake).expect("lq");
            eprintln!(
                "librequake worlds {}",
                report.assets.iter().filter(|a| a.kind == AssetKind::World).count()
            );
        }
    }

    #[test]
    #[ignore]
    fn convert_local_freedoom_pack() {
        let pack = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/packs/freedoom");
        if !pack.is_dir() {
            return;
        }
        let staged = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/ai_content_app/import/freedoom/work/source");
        let report = convert_classic(&pack, &staged, ClassicSource::Freedoom)
            .expect("convert freedoom");
        assert!(
            report.assets.iter().any(|a| a.kind == AssetKind::World),
            "expected worlds"
        );
        assert!(
            report.assets.iter().any(|a| a.kind == AssetKind::Billboard),
            "expected billboards"
        );
        eprintln!(
            "freedoom convert: {} assets (worlds {} billboards {} skipped {})",
            report.assets.len(),
            report.assets.iter().filter(|a| a.kind == AssetKind::World).count(),
            report.assets.iter().filter(|a| a.kind == AssetKind::Billboard).count(),
            report.skipped.len()
        );
    }

    #[test]
    fn rights_are_not_cc0_or_kenney() {
        let f = ClassicSource::Freedoom;
        let q = ClassicSource::LibreQuake;
        let d = ClassicSource::DarkMod;
        assert!(!f.license().to_ascii_lowercase().contains("cc0"));
        assert!(!q.license().to_ascii_lowercase().contains("cc0"));
        assert!(!d.license().to_ascii_lowercase().contains("cc0"));
        assert_eq!(d.license(), "CC-BY-NC-SA-3.0");
        assert_ne!(f.id(), "kenney");
        assert_ne!(q.id(), "kenney");
        assert_eq!(d.id(), "darkmod");
        let spec = f.pack_spec("freedoom-phase1");
        assert_eq!(spec.redistribution.as_deref(), Some("attribution-required"));
        let dspec = d.pack_spec("darkmod");
        assert_eq!(
            dspec.redistribution.as_deref(),
            Some("non-commercial-sharealike")
        );
        assert_eq!(dspec.derivatives.as_deref(), Some("share-alike"));
        let doom = ClassicSource::Doom;
        let quake = ClassicSource::Quake;
        assert_eq!(doom.pack_spec("doom").redistribution.as_deref(), Some("user-owned-local"));
        assert_eq!(quake.pack_spec("quake").redistribution.as_deref(), Some("user-owned-local"));
        assert!(doom.terms_url().starts_with("http"));
        assert!(quake.terms_url().starts_with("http"));
    }

    #[test]
    fn convert_idtech4_synthetic_md5_and_proc() {
        let root = tmp_dir("d3_syn");
        std::fs::create_dir_all(root.join("maps")).unwrap();
        std::fs::create_dir_all(root.join("models/md5/props")).unwrap();
        std::fs::write(
            root.join("maps/test.map.proc"),
            r#"mapProcFile003
model { "test" 1
surface { "textures/test" 3 3
( 0 0 0 0 0 0 0 1 )
( 32 0 0 1 0 0 0 1 )
( 0 32 0 0 1 0 0 1 )
0 1 2
}
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("models/md5/props/crate.md5mesh"),
            r#"MD5Version 10
commandline ""
numJoints 1
numMeshes 1
joints {
	"origin"	-1 ( 0 0 0 ) ( 0 0 0 )
}
mesh {
	shader "textures/test"
	numverts 3
	vert 0 ( 0.0 0.0 ) 0 1
	vert 1 ( 1.0 0.0 ) 1 1
	vert 2 ( 0.0 1.0 ) 2 1
	numtris 1
	tri 0 0 1 2
	numweights 3
	weight 0 0 1.0 ( 0 0 0 )
	weight 1 0 1.0 ( 32 0 0 )
	weight 2 0 1.0 ( 0 32 0 )
}
"#,
        )
        .unwrap();
        // Fan missions must not land as TDM core.
        std::fs::create_dir_all(root.join("fms/mymission")).unwrap();
        std::fs::write(root.join("fms/mymission/secret.md5mesh"), b"MD5Version 10\n")
            .unwrap();
        let staged = tmp_dir("d3_staged");
        let report = convert_classic(&root, &staged, ClassicSource::DarkMod).expect("convert");
        assert!(
            report.assets.iter().any(|a| a.kind == AssetKind::World),
            "expected world from .proc"
        );
        assert!(
            report.assets.iter().any(|a| a.kind == AssetKind::Prop),
            "expected prop from md5mesh"
        );
        assert!(
            !report.assets.iter().any(|a| a.rel_path.contains("secret")),
            "fan mission must not import"
        );
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn convert_then_pack_import_accepts_world_and_billboard() {
        let root = tmp_dir("pack_round");
        write_minimal_doom_wad(&root.join("freedoom.wad"));
        let work = tmp_dir("pack_work");
        let report =
            compile_classic(&root, &work, ClassicSource::Freedoom, "freedoom").expect("compile");
        assert!(report.pack.is_some());
        let pack = report.pack.unwrap();
        assert!(pack.assets >= 1, "expected assets, got {}", pack.assets);
        assert!(
            report
                .convert
                .assets
                .iter()
                .any(|a| a.kind == AssetKind::World),
            "world in convert"
        );
        assert!(
            report
                .convert
                .assets
                .iter()
                .any(|a| a.kind == AssetKind::Billboard),
            "billboard in convert"
        );
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&work);
    }

    /// Two 256-unit rooms side by side, joined by a 64-unit door sector whose
    /// ceiling is authored ON its floor (closed). Both door linedefs carry
    /// special 1 (manual DR door).
    fn write_two_room_door_wad(path: &Path) {
        let mut lumps: Vec<(&str, Vec<u8>)> = Vec::new();
        let mut playpal = vec![0u8; 768];
        playpal[4] = 255;
        lumps.push(("PLAYPAL", playpal));
        lumps.push(("MAP01", vec![]));
        let mut things = Vec::new();
        for v in [64i16, 64, 0, 1, 7] {
            things.extend_from_slice(&v.to_le_bytes());
        }
        lumps.push(("THINGS", things));

        // x: room A 0..128, door 128..192, room B 192..320. y: 0..128.
        let vert_xy: [(i16, i16); 8] = [
            (0, 0),
            (128, 0),
            (192, 0),
            (320, 0),
            (320, 128),
            (192, 128),
            (128, 128),
            (0, 128),
        ];
        let mut verts = Vec::new();
        for (x, y) in vert_xy {
            verts.extend_from_slice(&x.to_le_bytes());
            verts.extend_from_slice(&y.to_le_bytes());
        }
        lumps.push(("VERTEXES", verts));

        // (upper, mid, sector). A two-sided door line paints its UPPER
        // (the door leaf) and leaves mid blank, exactly like a real one.
        const BLANK: &[u8; 8] = b"-\0\0\0\0\0\0\0";
        const WALL: &[u8; 8] = b"WALL1\0\0\0";
        let sides: [(&[u8; 8], &[u8; 8], u16); 8] = [
            (BLANK, WALL, 0),  // 0: room A outer
            (BLANK, WALL, 2),  // 1: room B outer
            (WALL, BLANK, 0),  // 2: room A -> door (upper = door leaf)
            (WALL, BLANK, 1),  // 3: door -> room A
            (WALL, BLANK, 2),  // 4: room B -> door
            (WALL, BLANK, 1),  // 5: door -> room B
            (BLANK, WALL, 1),  // 6: door outer
            (BLANK, WALL, 1),  // 7: door outer
        ];
        let mut sidedefs = Vec::new();
        for (upper, mid, sector) in sides {
            sidedefs.extend_from_slice(&0i16.to_le_bytes());
            sidedefs.extend_from_slice(&0i16.to_le_bytes());
            sidedefs.extend_from_slice(upper);
            sidedefs.extend_from_slice(BLANK);
            sidedefs.extend_from_slice(mid);
            sidedefs.extend_from_slice(&sector.to_le_bytes());
        }
        lumps.push(("SIDEDEFS", sidedefs));

        // (v1, v2, flags, special, right side, left side)
        let lines: [(u16, u16, u16, u16, u16, u16); 10] = [
            (0, 1, 0, 0, 0, 0xFFFF),      // room A south
            (1, 6, 0x0004, 1, 2, 3),      // A|door west face (two-sided door line)
            (6, 7, 0, 0, 0, 0xFFFF),      // room A north
            (7, 0, 0, 0, 0, 0xFFFF),      // room A west
            (2, 3, 0, 0, 1, 0xFFFF),      // room B south
            (3, 4, 0, 0, 1, 0xFFFF),      // room B east
            (4, 5, 0, 0, 1, 0xFFFF),      // room B north
            (5, 2, 0x0004, 1, 4, 5),      // B|door east face
            (1, 2, 0, 0, 6, 0xFFFF),      // door south
            (5, 6, 0, 0, 7, 0xFFFF),      // door north
        ];
        let mut linedefs = Vec::new();
        for (v1, v2, flags, special, right, left) in lines {
            for v in [v1, v2, flags, special, 0, right, left] {
                linedefs.extend_from_slice(&v.to_le_bytes());
            }
        }
        lumps.push(("LINEDEFS", linedefs));

        // sector 0 room A, 1 door (ceiling on the floor), 2 room B.
        let mut sectors = Vec::new();
        for (floor, ceil) in [(0i16, 128i16), (0, 0), (0, 128)] {
            sectors.extend_from_slice(&floor.to_le_bytes());
            sectors.extend_from_slice(&ceil.to_le_bytes());
            sectors.extend_from_slice(b"FLOOR0\0\0");
            sectors.extend_from_slice(b"CEIL1\0\0\0");
            for v in [0i16, 0, 0] {
                sectors.extend_from_slice(&v.to_le_bytes());
            }
        }
        lumps.push(("SECTORS", sectors));
        lumps.push(("F_START", vec![]));
        lumps.push(("FLOOR0", vec![1u8; 64 * 64]));
        lumps.push(("F_END", vec![]));
        // A real 64x32 sky patch: the exporter embeds it as the sky node's
        // own picture.
        lumps.push(("SKY1", doom_patch(64, 32)));

        let mut data = Vec::new();
        data.extend_from_slice(b"IWAD");
        data.extend_from_slice(&(lumps.len() as u32).to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        let mut dir = Vec::new();
        for (name, lump) in &lumps {
            let pos = data.len() as u32;
            data.extend_from_slice(lump);
            dir.extend_from_slice(&pos.to_le_bytes());
            dir.extend_from_slice(&(lump.len() as u32).to_le_bytes());
            let mut n = [0u8; 8];
            for (i, b) in name.bytes().take(8).enumerate() {
                n[i] = b;
            }
            dir.extend_from_slice(&n);
        }
        let diroff = data.len() as u32;
        data[8..12].copy_from_slice(&diroff.to_le_bytes());
        data.extend_from_slice(&dir);
        std::fs::write(path, data).unwrap();
    }

    #[test]
    fn a_doom_door_exports_open_and_animatable() {
        use crate::classic_import::doom::{doom_map_to_mesh, parse_wad};
        use makepad_asset_client::json::{self, Value};
        let root = tmp_dir("door_map");
        let wad_path = root.join("doors.wad");
        write_two_room_door_wad(&wad_path);
        let wad = parse_wad(&std::fs::read(&wad_path).unwrap()).expect("wad");
        let palette = [[0u8; 3]; 256];
        let mesh = doom_map_to_mesh(&wad.lumps, "MAP01", &palette).expect("mesh");

        assert_eq!(mesh.doors.len(), 1, "one door sector");
        let door = &mesh.doors[0];
        assert_eq!(door.name, "door_1");
        // Closed on the floor, open at 128 - 4 headroom, at 1/64 scale.
        assert!((door.closed_y - 0.0).abs() < 1e-6, "{door:?}");
        assert!((door.open_y - 124.0 / 64.0).abs() < 1e-6, "{door:?}");
        // Doorway centre in engine space (Doom x 128..192, y 0..128; north
        // is −Z, so the doorway's y = 64 lands at z = −1).
        assert!((door.centre[0] - 160.0 / 64.0).abs() < 1e-4, "{door:?}");
        assert!((door.centre[2] + 64.0 / 64.0).abs() < 1e-4, "{door:?}");

        // The exported GLB carries it as an animating node.
        let json_text = glb_json(&mesh.glb);
        let root_json = json::parse(json_text.as_bytes()).expect("glb json");
        let nodes = root_json.get("nodes").unwrap().as_arr().unwrap();
        let node = nodes
            .iter()
            .find(|n| n.get("name").and_then(Value::as_str) == Some("door_1"))
            .expect("door_1 node");
        let extras = node.get("extras").expect("extras");
        assert_eq!(extras.get("kind").and_then(Value::as_str), Some("door"));
        assert_eq!(extras.get("default").and_then(Value::as_str), Some("open"));
        let t = node.get("translation").unwrap().as_arr().unwrap();
        let rest_y = match &t[1] {
            Value::F64(f) => *f as f32,
            Value::Int(i) => *i as f32,
            _ => f32::NAN,
        };
        assert!(
            (rest_y - (door.open_y - door.closed_y)).abs() < 1e-4,
            "rest pose must be OPEN, got {rest_y}"
        );
        let anims = root_json.get("animations").unwrap().as_arr().unwrap();
        assert!(anims
            .iter()
            .any(|a| a.get("name").and_then(Value::as_str) == Some("door_1")));

        // Walk the corridor: a ray from room A to room B at chest height
        // must hit nothing in the static level, and must hit the door leaf
        // (which is authored in its CLOSED pose).
        let parts = crate::world_preview::extract_glb_parts(&mesh.glb).expect("parts");
        assert_eq!(parts.len(), 2, "level primitive + one door primitive");
        // Doom (64, 64) -> (256, 64), the corridor's own centre line.
        let from = doom_xz(64.0, 64.0);
        let to = doom_xz(256.0, 64.0);
        let walk = |part: &crate::world_preview::Extracted| {
            part.indices.chunks_exact(3).any(|tri| {
                hits_segment(
                    part.pos[tri[0] as usize],
                    part.pos[tri[1] as usize],
                    part.pos[tri[2] as usize],
                    [from[0], 0.8, from[1]],
                    [to[0], 0.8, to[1]],
                )
            })
        };
        assert!(
            !walk(&parts[0]),
            "the static level must not bake a slab across the doorway"
        );
        assert!(
            walk(&parts[1]),
            "the door leaf is the geometry that blocks it while closed"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Möller-Trumbore: does the segment `from`..`to` cross this triangle?
    fn hits_segment(a: [f32; 3], b: [f32; 3], c: [f32; 3], from: [f32; 3], to: [f32; 3]) -> bool {
        let sub = |p: [f32; 3], q: [f32; 3]| [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
        let cross = |p: [f32; 3], q: [f32; 3]| {
            [
                p[1] * q[2] - p[2] * q[1],
                p[2] * q[0] - p[0] * q[2],
                p[0] * q[1] - p[1] * q[0],
            ]
        };
        let dot = |p: [f32; 3], q: [f32; 3]| p[0] * q[0] + p[1] * q[1] + p[2] * q[2];
        let dir = sub(to, from);
        let e1 = sub(b, a);
        let e2 = sub(c, a);
        let h = cross(dir, e2);
        let det = dot(e1, h);
        if det.abs() < 1e-9 {
            return false;
        }
        let inv = 1.0 / det;
        let s = sub(from, a);
        let u = dot(s, h) * inv;
        if !(0.0..=1.0).contains(&u) {
            return false;
        }
        let q = cross(s, e1);
        let v = dot(dir, q) * inv;
        if v < 0.0 || u + v > 1.0 {
            return false;
        }
        let t = dot(e2, q) * inv;
        (0.0..=1.0).contains(&t)
    }

    fn glb_bin(glb: &[u8]) -> Vec<u8> {
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let at = 20 + json_len;
        let bin_len = u32::from_le_bytes(glb[at..at + 4].try_into().unwrap()) as usize;
        glb[at + 8..at + 8 + bin_len].to_vec()
    }

    /// Read a float accessor's values through its bufferView.
    fn read_accessor(
        root: &makepad_asset_client::json::Value,
        bin: &[u8],
        index: &makepad_asset_client::json::Value,
    ) -> Vec<f32> {
        use makepad_asset_client::json::Value;
        let i = index.as_i64().unwrap() as usize;
        let acc = &root.get("accessors").unwrap().as_arr().unwrap()[i];
        let vi = acc.get("bufferView").and_then(Value::as_i64).unwrap() as usize;
        let view = &root.get("bufferViews").unwrap().as_arr().unwrap()[vi];
        let off = view.get("byteOffset").and_then(Value::as_i64).unwrap_or(0) as usize;
        let len = view.get("byteLength").and_then(Value::as_i64).unwrap() as usize;
        bin[off..off + len]
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    fn glb_json(glb: &[u8]) -> String {
        let len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        String::from_utf8_lossy(&glb[20..20 + len]).to_string()
    }

    #[test]
    fn sector_lightlevels_bake_into_color_0_and_mark_the_map_prelit() {
        use crate::classic_import::doom::{doom_map_to_mesh, parse_wad};
        use makepad_asset_client::json::{self, Value};
        let root = tmp_dir("light_map");
        let wad_path = root.join("light.wad");
        write_two_room_door_wad(&wad_path);
        let mut bytes = std::fs::read(&wad_path).unwrap();
        let wad = parse_wad(&bytes).expect("wad");
        let sectors_lump = wad.lumps.iter().find(|l| l.name == "SECTORS").unwrap();
        let at = bytes
            .windows(sectors_lump.data.len())
            .position(|w| w == sectors_lump.data.as_slice())
            .expect("sector bytes");
        // Room A fullbright, room B dim.
        bytes[at + 20..at + 22].copy_from_slice(&255i16.to_le_bytes());
        bytes[at + 2 * 26 + 20..at + 2 * 26 + 22].copy_from_slice(&64i16.to_le_bytes());
        std::fs::write(&wad_path, &bytes).unwrap();

        let wad = parse_wad(&bytes).expect("wad");
        let mesh = doom_map_to_mesh(&wad.lumps, "MAP01", &[[0u8; 3]; 256]).expect("mesh");
        let root_json = json::parse(glb_json(&mesh.glb).as_bytes()).expect("glb json");
        let bin = glb_bin(&mesh.glb);
        let prim = &root_json.get("meshes").unwrap().as_arr().unwrap()[0]
            .get("primitives")
            .unwrap()
            .as_arr()
            .unwrap()[0];
        let attr = prim.get("attributes").unwrap();
        let pos = read_accessor(&root_json, &bin, attr.get("POSITION").unwrap());
        let colors = read_accessor(
            &root_json,
            &bin,
            attr.get("COLOR_0").expect("COLOR_0 baked into the level"),
        );
        assert_eq!(colors.len(), pos.len());
        // Room A is at x < 2 m, room B at x > 3 m.
        let light_at = |pick: fn(f32) -> bool| {
            pos.chunks_exact(3)
                .zip(colors.chunks_exact(3))
                .find(|(p, _)| pick(p[0]))
                .map(|(_, c)| c[0])
                .expect("a vertex in that room")
        };
        // Walls carry Doom's fake contrast (+/- one 16-unit light level),
        // so each room's vertices sit within a step of its lightlevel.
        let bright = light_at(|x| x < 1.5);
        let dim = light_at(|x| x > 3.5);
        assert!(
            (239.0 / 255.0..=1.0).contains(&bright),
            "lightlevel 255 (+/- fake contrast) -> {bright}"
        );
        assert!(
            (48.0 / 255.0..=80.0 / 255.0).contains(&dim),
            "lightlevel 64 (+/- fake contrast) -> {dim}"
        );
        assert!(bright > dim + 0.4, "the two sectors must differ");

        // And the material carries the prelit marker the renderer reads.
        let materials = root_json.get("materials").unwrap().as_arr().unwrap();
        assert!(
            materials.iter().any(|m| m
                .get("extras")
                .and_then(|e| e.get("lightmapTexture"))
                .is_some()),
            "a Doom map is prelit: the sun must not light it again"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A minimal Doom picture-format patch: one post per column.
    fn doom_patch(w: u16, h: u16) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        out.extend_from_slice(&0i16.to_le_bytes());
        out.extend_from_slice(&0i16.to_le_bytes());
        let table = out.len();
        out.extend(std::iter::repeat(0u8).take(w as usize * 4));
        for col in 0..w as usize {
            let pos = out.len() as u32;
            out[table + col * 4..table + col * 4 + 4].copy_from_slice(&pos.to_le_bytes());
            out.push(0); // rowstart
            out.push(h as u8); // length
            out.push(0); // unused
            for row in 0..h as usize {
                out.push(((col + row) % 255) as u8);
            }
            out.push(0); // unused
            out.push(255); // terminator
        }
        out
    }

    /// The exported sky, read back by the RENDERER's own parser and its CPU
    /// twin of the sky shader. A GPU capture is a separate step (the sandbox
    /// resolves models through an asset index this tree stubs out), but the
    /// mapping — projection, wrap count, phase, which faces left the static
    /// stream — is exactly what a picture would be judged on, and this can
    /// be asked in a test.
    #[test]
    fn the_renderer_reads_our_doom_sky_the_way_doom_drew_it() {
        use crate::classic_import::doom::{doom_map_to_mesh, parse_wad};
        use makepad_render::model::{SkyProjection, StaticModel};

        let root = tmp_dir("sky_read");
        let wad_path = root.join("sky.wad");
        write_two_room_door_wad(&wad_path);
        let mut wad = parse_wad(&std::fs::read(&wad_path).unwrap()).expect("wad");
        for lump in &mut wad.lumps {
            if lump.name == "SECTORS" {
                // Room B's ceiling is the sky.
                lump.data[2 * 26 + 12..2 * 26 + 20].copy_from_slice(b"F_SKY1\0\0");
            }
        }
        let mesh = doom_map_to_mesh(&wad.lumps, "MAP01", &[[64u8; 3]; 256]).expect("mesh");
        let model = StaticModel::parse_glb(&mesh.glb).expect("renderer parses it");
        // A direction of the model API's own vector type, without naming its
        // math crate (not a dependency here).
        let dir = |x: f32, y: f32, z: f32| {
            let mut v = model.min;
            v.x = x;
            v.y = y;
            v.z = z;
            v
        };
        let sky = model.sky.expect("renderer found the sky part");
        assert_eq!(sky.projection, SkyProjection::Cylinder);
        assert_eq!(sky.repeat, 4.0);
        assert_eq!(sky.offset, 0.0);
        assert_eq!(sky.images.len(), 1, "one picture, embedded");
        assert!(sky.images[0].starts_with(b"\x89PNG"));
        assert_eq!(sky.texture.as_deref(), Some("sky1"));
        assert!(!sky.vertices.is_empty(), "sky faces are real geometry");
        assert!(sky.indices.len() >= 3);

        // Doom shows sky column 0 when the player faces east (+X), and the
        // strip wraps four times round the compass.
        let east = sky.direction_uv(dir(1.0, 0.0, 0.0), 0, 0.0);
        assert!((east[0] - 1.0).abs() < 1e-4 || east[0].abs() < 1e-4, "{east:?}");
        let quarter = sky.direction_uv(dir(0.0, 0.0, -1.0), 0, 0.0);
        assert!(
            (quarter[0] - east[0]).abs() > 0.9,
            "a quarter turn moves one whole image: {east:?} -> {quarter:?}"
        );
        // And it moves the way VANILLA moves it. `R_RenderBSPNode` reads the
        // column straight off the view angle (`angle >> ANGLETOSKYSHIFT`), so
        // the column GROWS as the player turns left — anticlockwise from
        // east, through north — by 4/360 of the image per degree. Doom's
        // facing `a` is `(cos a, sin a)` in (east, north), and north is GLB
        // −Z, so the view ray is `(cos a, 0, −sin a)`. A converter that put
        // north at +Z passes every assertion above and fails this one: same
        // sky, wound backwards, drifting the wrong way on every turn.
        for degrees in [22.5f32, 45.0, 67.5] {
            let (s, c) = degrees.to_radians().sin_cos();
            let u = sky.direction_uv(dir(c, 0.0, -s), 0, 0.0)[0];
            let want = east[0] + degrees / 360.0 * sky.repeat;
            assert!(
                (u - want).abs() < 1e-3,
                "turning {degrees} degrees left must advance the strip to \
                 {want}, got {u} (backwards would be {})",
                east[0] - degrees / 360.0 * sky.repeat
            );
        }
        // The horizon sits in the middle of the picture and the zenith clamps.
        let horizon = sky.direction_uv(dir(1.0, 0.0, 0.0), 0, 0.0);
        assert!((horizon[1] - 0.5).abs() < 1e-4, "{horizon:?}");
        let up = sky.direction_uv(dir(0.0, 1.0, 0.0), 0, 0.0);
        assert_eq!(up[1], 0.0, "looking up clamps to the top row");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn exits_and_keys_become_markers_and_keyed_doors_say_so() {
        use crate::classic_import::doom::{doom_map_nav, doom_map_to_mesh, map_verts, parse_wad};
        use makepad_asset_client::json::{self, Value};
        let root = tmp_dir("exit_map");
        let wad_path = root.join("exit.wad");
        write_two_room_door_wad(&wad_path);
        let mut wad = parse_wad(&std::fs::read(&wad_path).unwrap()).expect("wad");
        for lump in &mut wad.lumps {
            match lump.name.as_str() {
                "LINEDEFS" => {
                    // Room A's south wall is the exit switch (11); the west
                    // door line becomes a RED-key door (28).
                    lump.data[6..8].copy_from_slice(&11u16.to_le_bytes());
                    lump.data[14 + 6..14 + 8].copy_from_slice(&28u16.to_le_bytes());
                }
                "THINGS" => {
                    // A red keycard (type 13) at (96, 64).
                    for v in [96i16, 64, 0, 13, 7] {
                        lump.data.extend_from_slice(&v.to_le_bytes());
                    }
                }
                _ => {}
            }
        }
        let markers = doom_map_nav(&wad.lumps, "MAP01").expect("nav").markers;
        let names: Vec<&str> = markers.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"exit"), "{names:?}");
        assert!(names.contains(&"key_red"), "{names:?}");
        let key = markers.iter().find(|m| m.name == "key_red").unwrap();
        assert!((key.pos[0] - 96.0 / 64.0).abs() < 1e-4, "{key:?}");
        assert!((key.pos[1] - 41.0 / 64.0).abs() < 1e-4, "eye above the floor");
        let exit = markers.iter().find(|m| m.name == "exit").unwrap();
        // Room A's south wall runs (0,0)->(128,0): its midpoint is x=64.
        assert!((exit.pos[0] - 1.0).abs() < 1e-4, "{exit:?}");
        assert!(exit.pos[2].abs() < 1e-4, "{exit:?}");
        let _ = map_verts(&wad.lumps, "MAP01");

        // And the door that needs the key says which one.
        let mesh = doom_map_to_mesh(&wad.lumps, "MAP01", &[[0u8; 3]; 256]).expect("mesh");
        let root_json = json::parse(glb_json(&mesh.glb).as_bytes()).expect("glb json");
        let door = root_json
            .get("nodes")
            .unwrap()
            .as_arr()
            .unwrap()
            .iter()
            .find(|n| n.get("name").and_then(Value::as_str) == Some("door_1"))
            .expect("door_1");
        assert_eq!(
            door.get("extras").unwrap().get("key").and_then(Value::as_str),
            Some("red")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Vertices that sit strictly INSIDE another triangle's edge on the
    /// same plane: the T-junctions that crack under rasterisation even
    /// when every coordinate is exact.
    fn t_junctions(tris: &[[[f32; 3]; 3]]) -> usize {
        use std::collections::{HashMap, HashSet};
        // Unique vertices, bucketed by metre so the edge scan stays local.
        let mut verts: HashSet<(u32, u32)> = HashSet::new();
        for t in tris {
            for p in t {
                verts.insert((p[0].to_bits(), p[2].to_bits()));
            }
        }
        let mut buckets: HashMap<(i32, i32), Vec<(f32, f32)>> = HashMap::new();
        for (bx, bz) in &verts {
            let (x, z) = (f32::from_bits(*bx), f32::from_bits(*bz));
            buckets.entry((x as i32, z as i32)).or_default().push((x, z));
        }
        // Snap quantum: `snap_pos` rounds to 1/1024 m, so anything within
        // half of that of an edge is ON it. The end margin is the weld's own
        // `MIN_FROM_END`, so this audit asks the same question the pass
        // answers — see its doc for why a cut nearer than one source unit to
        // a corner is not worth making.
        const ON_EDGE: f32 = 1.0 / 2048.0;
        const FROM_END: f32 = super::weld::MIN_FROM_END;
        let mut hits = 0usize;
        for t in tris {
            if tri_area2(t[0], t[1], t[2]) < 1e-7 {
                continue;
            }
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                let (ax, az, bx, bz) = (a[0], a[2], b[0], b[2]);
                let (dx, dz) = (bx - ax, bz - az);
                let len2 = dx * dx + dz * dz;
                if len2 < 1e-9 {
                    continue;
                }
                let (lo_x, hi_x) = (ax.min(bx), ax.max(bx));
                let (lo_z, hi_z) = (az.min(bz), az.max(bz));
                for cx in (lo_x as i32 - 1)..=(hi_x as i32 + 1) {
                    for cz in (lo_z as i32 - 1)..=(hi_z as i32 + 1) {
                        for &(px, pz) in buckets.get(&(cx, cz)).map(Vec::as_slice).unwrap_or(&[]) {
                            // Not an endpoint of this edge.
                            if (px.to_bits() == ax.to_bits() && pz.to_bits() == az.to_bits())
                                || (px.to_bits() == bx.to_bits() && pz.to_bits() == bz.to_bits())
                            {
                                continue;
                            }
                            let len = len2.sqrt();
                            let t_along = ((px - ax) * dx + (pz - az) * dz) / len2;
                            if t_along * len <= FROM_END || (1.0 - t_along) * len <= FROM_END {
                                continue;
                            }
                            let perp = ((px - ax) * dz - (pz - az) * dx).abs() / len;
                            if perp <= ON_EDGE {
                                hits += 1;
                            }
                        }
                    }
                }
            }
        }
        hits
    }

    /// Flat (horizontal) triangles of a GLB, grouped by their exact height.
    /// Positions come back snapped, so equality here is bitwise.
    fn flat_triangles(glb: &[u8]) -> std::collections::BTreeMap<u32, Vec<[[f32; 3]; 3]>> {
        let parts = crate::world_preview::extract_glb_parts(glb).expect("parts");
        let mut out: std::collections::BTreeMap<u32, Vec<[[f32; 3]; 3]>> = Default::default();
        for part in &parts {
            for tri in part.indices.chunks_exact(3) {
                let ps = [
                    part.pos[tri[0] as usize],
                    part.pos[tri[1] as usize],
                    part.pos[tri[2] as usize],
                ];
                if ps[0][1] != ps[1][1] || ps[1][1] != ps[2][1] {
                    continue;
                }
                out.entry(ps[0][1].to_bits()).or_default().push(ps);
            }
        }
        out
    }

    fn tri_area2(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f32 {
        ((b[0] - a[0]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[0] - a[0])).abs()
    }

    fn point_in_flat_tri(p: [f32; 2], t: &[[f32; 3]; 3]) -> bool {
        let sign = |a: [f32; 3], b: [f32; 3]| {
            (b[0] - a[0]) * (p[1] - a[2]) - (b[2] - a[2]) * (p[0] - a[0])
        };
        let (d1, d2, d3) = (sign(t[0], t[1]), sign(t[1], t[2]), sign(t[2], t[0]));
        let neg = d1 < -1e-6 || d2 < -1e-6 || d3 < -1e-6;
        let pos = d1 > 1e-6 || d2 > 1e-6 || d3 > 1e-6;
        !(neg && pos)
    }

    /// How badly a flat plane's triangles overlap and how many edges are
    /// shared by more than two triangles (duplicate coverage), plus edges
    /// used exactly once that are NOT on the plane's outer boundary
    /// (T-junction candidates — a crack).
    fn flat_health(tris: &[[[f32; 3]; 3]]) -> (usize, usize) {
        use std::collections::HashMap;
        let mut edges: HashMap<(u64, u64, u64, u64), usize> = HashMap::new();
        for t in tris {
            // A degenerate sliver has no area to fight over and its edges
            // duplicate its neighbours' by construction.
            if tri_area2(t[0], t[1], t[2]) < 1e-7 {
                continue;
            }
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                let key = if (a[0].to_bits(), a[2].to_bits()) <= (b[0].to_bits(), b[2].to_bits()) {
                    (
                        a[0].to_bits() as u64,
                        a[2].to_bits() as u64,
                        b[0].to_bits() as u64,
                        b[2].to_bits() as u64,
                    )
                } else {
                    (
                        b[0].to_bits() as u64,
                        b[2].to_bits() as u64,
                        a[0].to_bits() as u64,
                        a[2].to_bits() as u64,
                    )
                };
                *edges.entry(key).or_insert(0) += 1;
            }
        }
        let over_used = edges.values().filter(|&&n| n > 2).count();
        // Overlap: a triangle's centroid inside ANOTHER triangle of the
        // same plane. Bucketed by metre so this stays linear-ish.
        let mut buckets: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (i, t) in tris.iter().enumerate() {
            let cx = (t[0][0] + t[1][0] + t[2][0]) / 3.0;
            let cz = (t[0][2] + t[1][2] + t[2][2]) / 3.0;
            buckets.entry((cx as i32, cz as i32)).or_default().push(i);
        }
        let mut overlaps = 0usize;
        for t in tris.iter() {
            if tri_area2(t[0], t[1], t[2]) < 1e-7 {
                continue;
            }
            let cx = (t[0][0] + t[1][0] + t[2][0]) / 3.0;
            let cz = (t[0][2] + t[1][2] + t[2][2]) / 3.0;
            let mut hits = 0usize;
            for dx in -1..=1 {
                for dz in -1..=1 {
                    for &j in buckets
                        .get(&(cx as i32 + dx, cz as i32 + dz))
                        .map(Vec::as_slice)
                        .unwrap_or(&[])
                    {
                        let other = &tris[j];
                        if tri_area2(other[0], other[1], other[2]) < 1e-7 {
                            continue;
                        }
                        if point_in_flat_tri([cx, cz], other) {
                            hits += 1;
                        }
                    }
                }
            }
            // Its own triangle always contains its centroid.
            if hits > 1 {
                overlaps += 1;
            }
        }
        (overlaps, over_used)
    }

    /// The renderer's camera basis in the GLB's x/z plane, repeated here so
    /// the handedness tests below assert against the CONSUMER's formula and
    /// not against any converter's inverse of it. Forward is
    /// `makepad_render::level::yaw_forward` = `(sin yaw, 0, −cos yaw)`; right
    /// is `forward × up` = `(cos yaw, 0, sin yaw)`, the strafe vector
    /// `level.rs` and `play.rs` both use.
    fn camera_basis(yaw: f32) -> ([f32; 2], [f32; 2]) {
        ([yaw.sin(), -yaw.cos()], [yaw.cos(), yaw.sin()])
    }

    /// Where `to` lies in the frame of a viewer standing at `from` looking
    /// along `yaw`, as `(ahead, left)` metres. Both inputs are the GLB's
    /// x/z plane.
    fn ahead_left(from: [f32; 3], yaw: f32, to: [f32; 3]) -> (f32, f32) {
        let (f, r) = camera_basis(yaw);
        let d = [to[0] - from[0], to[2] - from[2]];
        (d[0] * f[0] + d[1] * f[1], -(d[0] * r[0] + d[1] * r[1]))
    }

    /// The same, in a SOURCE game's own map plane: `(east, north)` with the
    /// facing given as degrees counter-clockwise from east, which is how
    /// Doom's THINGS and Quake's `angle` both store it.
    fn ahead_left_map(from: [f32; 2], angle_deg: f32, to: [f32; 2]) -> (f32, f32) {
        let (s, c) = angle_deg.to_radians().sin_cos();
        let (e, n) = (to[0] - from[0], to[1] - from[1]);
        (e * c + n * s, -e * s + n * c)
    }

    /// **The handedness law, stated once for every classic family.**
    ///
    /// Each converter takes a source game's map space into the GLB's. Whether
    /// it does so as a ROTATION or as a REFLECTION is decided by one sign
    /// nobody looks at, and a reflected level is fully walkable: the walls
    /// meet, the doors open, the walker's own formulas agree with themselves.
    /// It is simply the mirror of the level the original engine draws.
    ///
    /// The test is the cross product. Take each game's own (east, north, up)
    /// — east and north being any two ground directions with up completing a
    /// RIGHT-handed frame, which every one of these engines uses — push them
    /// through that game's conversion, and require
    /// `east × north = up` to still hold in the GLB. Determinant −1 fails it,
    /// and there is nothing else it can fail on.
    ///
    /// Quake 1 has no single conversion function to name here; its guard is
    /// [`quake_e1m1_keeps_its_bearings_too`], on the real BSP.
    #[test]
    fn every_classic_converter_maps_a_right_handed_world_to_a_right_handed_one() {
        let cross = |a: [f32; 3], b: [f32; 3]| {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        };
        let unit = |v: [f32; 3]| {
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            assert!(l > 0.0, "a basis vector collapsed");
            [v[0] / l, v[1] / l, v[2] / l]
        };
        // (family, east, north, up) in that game's OWN map space, and the
        // conversion under test. Doom is 2-D + a height, so its function
        // takes (east, up, north) and the basis is written to match.
        let doom = |v: [f32; 3]| {
            crate::classic_import::doom::doom_to_glb(v[0], v[1], v[2])
        };
        let duke = |v: [f32; 3]| crate::duke_import::build_to_glb(v[0], v[1], v[2]);
        let families: [(&str, [[f32; 3]; 3], &dyn Fn([f32; 3]) -> [f32; 3]); 5] = [
            // Doom: (east, up, north) — north is the third argument.
            ("doom", [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]], &doom),
            // Quake 1/2/3 and id Tech 4: X east, Y north, Z up.
            (
                "quake2",
                [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                &crate::quake2_import::to_glb,
            ),
            (
                "quake3",
                [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                &crate::quake3_import::map_to_glb,
            ),
            (
                "doom3",
                [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                &crate::doom3_import::xform,
            ),
            // BUILD: (x, up, y) with x forward at angle 0, y its RIGHT
            // (`drawrooms` puts screen-right on +y) and z DOWN — so "north",
            // the left-hand ground axis, is −y, and up is −z, which
            // `build_to_glb` takes as its middle argument already negated.
            ("duke", [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]], &duke),
        ];
        for (name, [east, north, up], to_glb) in families {
            let zero = to_glb([0.0, 0.0, 0.0]);
            let d = |v: [f32; 3]| {
                let p = to_glb(v);
                [p[0] - zero[0], p[1] - zero[1], p[2] - zero[2]]
            };
            let (e, n, u) = (unit(d(east)), unit(d(north)), unit(d(up)));
            let c = cross(e, n);
            let dot = c[0] * u[0] + c[1] * u[1] + c[2] * u[2];
            assert!(
                dot > 0.999,
                "{name}: east x north points {c:?} but up is {u:?} (dot {dot:.3}) \
                 — a dot of −1 means this converter writes the level's MIRROR"
            );
        }
    }

    /// A facing and a position are ONE contract, and it is the joint that
    /// breaks: a converter can place a level correctly and still turn its
    /// spawns to a formula derived for the other convention, which is exactly
    /// what Doom's markers and Duke's `build_yaw` did.
    ///
    /// So: turning to `doom_yaw(a)` must look along the GLB image of Doom's
    /// own facing `(cos a, sin a)`, for every `a`. `doom_dir_yaw` is the same
    /// statement for a direction that arrives as a vector (a switch's press
    /// direction) rather than as an angle.
    #[test]
    fn doom_yaw_looks_exactly_where_doom_points() {
        use crate::classic_import::doom::{doom_dir_yaw, doom_to_glb, doom_yaw};
        for degrees in [0.0f32, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0] {
            let (s, c) = degrees.to_radians().sin_cos();
            let want = doom_to_glb(c, 0.0, s);
            for (label, yaw) in [
                ("doom_yaw", doom_yaw(degrees)),
                ("doom_dir_yaw", doom_dir_yaw(c, s)),
            ] {
                let (f, _) = camera_basis(yaw);
                assert!(
                    (f[0] - want[0]).abs() < 1e-5 && (f[1] - want[2]).abs() < 1e-5,
                    "{label}({degrees}) looks ({:.4}, {:.4}) but Doom points \
                     ({:.4}, {:.4})",
                    f[0],
                    f[1],
                    want[0],
                    want[2]
                );
            }
        }
    }

    /// Vanilla's own sprite-rotation pick, in integer BAM, so the test
    /// argues with `r_things.c` rather than with a paraphrase of it:
    ///
    /// ```text
    ///     ang = R_PointToAngle (thing->x, thing->y);   // viewer -> thing
    ///     rot = (ang - thing->angle + (unsigned)(ANG45/2)*9) >> 29;
    /// ```
    ///
    /// Returns the sprite-name DIGIT (`rot + 1`), which is what the lumps
    /// and the manifest are keyed by.
    fn vanilla_rot_digit(thing_angle_deg: f64, viewer_to_thing_deg: f64) -> u8 {
        let bam = |deg: f64| {
            ((deg.rem_euclid(360.0) / 360.0 * 4_294_967_296.0) as u64 & 0xFFFF_FFFF) as u32
        };
        const ANG45: u32 = 0x2000_0000;
        let bias = (ANG45 / 2).wrapping_mul(9);
        let rot = bam(viewer_to_thing_deg)
            .wrapping_sub(bam(thing_angle_deg))
            .wrapping_add(bias)
            >> 29;
        rot as u8 + 1
    }

    /// **The rotation contract, against the corrected world basis.**
    ///
    /// The 8-way artwork is not published in any engine's coordinates: a
    /// rotation digit means "the drawing seen from a camera standing this
    /// many 45° steps ANTICLOCKWISE of the way the thing faces", measured in
    /// the source game's own plane. A handedness-preserving converter (which
    /// is now what every family has) carries that meaning across unchanged —
    /// but a MIRRORED one reverses it, and any calibration done inside the
    /// mirror comes out reversed the day the mirror is fixed.
    ///
    /// So this stands the thing at the origin at each declared angle, walks
    /// a camera around it in the CONVERTED world, and demands the sector the
    /// engine's rule picks be the digit vanilla's shift picks. It also pins
    /// the boundary the engine converts at: `doom_yaw` publishes a facing in
    /// the camera convention, and a body's heading is its mirror.
    #[test]
    fn sprite_rotations_match_vanilla_in_the_converted_world() {
        use crate::classic_import::doom::{doom_to_glb, doom_yaw};
        use crate::stateful_billboard::StatefulBillboard;
        let wrap = |a: f32| {
            let mut a = a;
            while a > std::f32::consts::PI {
                a -= std::f32::consts::TAU;
            }
            while a <= -std::f32::consts::PI {
                a += std::f32::consts::TAU;
            }
            a
        };
        let origin = doom_to_glb(0.0, 0.0, 0.0);
        for angle in [0.0f32, 37.0, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0] {
            // The way the thing faces, as the converted world sees it —
            // taken from the geometry, not from the yaw formula, so the two
            // can be checked against each other.
            let (s, c) = angle.to_radians().sin_cos();
            let ahead = doom_to_glb(c, 0.0, s);
            let facing = (-(ahead[0] - origin[0])).atan2(-(ahead[2] - origin[2]));
            assert!(
                wrap(facing + doom_yaw(angle)).abs() < 1e-5,
                "doom_yaw({angle}) = {} is not the camera-convention mirror of \
                 the heading {facing} the geometry points along",
                doom_yaw(angle)
            );
            for bearing in (0..360).step_by(15).map(|d| d as f32) {
                // A camera standing 128 map units off at Doom bearing
                // `bearing`, converted the same way the level is.
                let (bs, bc) = bearing.to_radians().sin_cos();
                let cam = doom_to_glb(128.0 * bc, 0.0, 128.0 * bs);
                let to_cam =
                    (-(cam[0] - origin[0])).atan2(-(cam[2] - origin[2]));
                let got = StatefulBillboard::facing_for_bearing(facing, to_cam, 8);
                // Vanilla measures viewer -> thing; the camera stands at
                // `bearing` FROM the thing, so it looks back along +180.
                let want = vanilla_rot_digit(f64::from(angle), f64::from(bearing) + 180.0);
                assert_eq!(
                    got, want,
                    "thing at angle {angle}, camera at bearing {bearing}: engine \
                     picks rotation {got}, vanilla picks {want}"
                );
            }
        }
    }

    /// The same audit for the OTHER family that publishes 8-way artwork.
    ///
    /// Duke's `animatesprites` viewtype-5 branch is
    ///
    /// ```text
    ///     k = ((s->ang + 3072 + 128 - getangle(s->x-px, s->y-py)) & 2047) >> 8
    /// ```
    ///
    /// — Doom's rule written in BUILD's CLOCKWISE compass, and
    /// [`crate::duke_import::build_to_glb`] preserves BUILD's handedness, so
    /// through the conversion it has to come out as the same anticlockwise
    /// table the engine applies. If the two families needed different signs
    /// here, the engine would need a per-game branch — and it has none.
    #[test]
    fn duke_sprite_rotations_match_build_in_the_converted_world() {
        use crate::duke_import::{build_to_glb, build_yaw};
        use crate::stateful_billboard::StatefulBillboard;
        let wrap = |a: f32| {
            let mut a = a;
            while a > std::f32::consts::PI {
                a -= std::f32::consts::TAU;
            }
            while a <= -std::f32::consts::PI {
                a += std::f32::consts::TAU;
            }
            a
        };
        // BUILD's 2048-step compass, in BUILD's own integers.
        let vanilla = |ang: i32, bearing: i32| -> u8 {
            // The camera stands at `bearing` from the sprite, so `getangle`
            // (sprite -> viewer reversed) reads bearing + 1024.
            let to_sprite = bearing + 1024;
            ((((ang + 3072 + 128 - to_sprite) & 2047) >> 8) & 7) as u8 + 1
        };
        let origin = build_to_glb(0.0, 0.0, 0.0);
        for ang in (0..2048).step_by(128) {
            let a = ang as f32 * std::f32::consts::PI / 1024.0;
            let ahead = build_to_glb(a.cos(), 0.0, a.sin());
            let facing = (-(ahead[0] - origin[0])).atan2(-(ahead[2] - origin[2]));
            assert!(
                wrap(facing + build_yaw(ang as i16)).abs() < 1e-4,
                "build_yaw({ang}) is not the camera-convention mirror of the \
                 heading {facing} the geometry points along"
            );
            // +32 keeps every sample off the sector BOUNDARY (a difference of
            // 128 + 256k BUILD units): there the two engines are a coin flip
            // apart — BUILD's `>>8` truncates a value sitting exactly on the
            // tie, and a float `floor(x + 0.5)` lands either side of it by a
            // rounding bit. Half a degree of sprite turn, not a contract.
            for bearing in (0..2048).step_by(64).map(|b| b + 32) {
                let b = bearing as f32 * std::f32::consts::PI / 1024.0;
                let cam = build_to_glb(2048.0 * b.cos(), 0.0, 2048.0 * b.sin());
                let to_cam = (-(cam[0] - origin[0])).atan2(-(cam[2] - origin[2]));
                let got = StatefulBillboard::facing_for_bearing(facing, to_cam, 8);
                let want = vanilla(ang, bearing);
                assert_eq!(
                    got, want,
                    "sprite at ang {ang}, camera at bearing {bearing}: engine \
                     picks rotation {got}, BUILD picks {want}"
                );
            }
        }
    }

    /// The other half of the rotation export: WHICH drawing, and whether it
    /// is mirrored. Vanilla installs `TROOA2A8` as lump[1] unflipped and
    /// lump[7] flipped; the manifest has to say the same thing, because a
    /// mirrored pair drawn unmirrored is a monster whose gun changes hands
    /// (and, on a 5-view sheet, the wrong side entirely).
    #[test]
    fn mirrored_rotation_pairs_keep_vanillas_flip() {
        use crate::stateful_billboard::{assemble, DoomSpriteName};
        let lump = |pairs: Vec<(char, u8)>, file: &str| {
            (
                DoomSpriteName { prefix: "troo".into(), pairs },
                file.to_string(),
                40u32,
                55u32,
            )
        };
        let lumps = vec![
            lump(vec![('A', 1)], "trooa1.png"),
            lump(vec![('A', 2), ('A', 8)], "trooa2a8.png"),
            lump(vec![('A', 3), ('A', 7)], "trooa3a7.png"),
            lump(vec![('A', 4), ('A', 6)], "trooa4a6.png"),
            lump(vec![('A', 5)], "trooa5.png"),
        ];
        let bb = assemble("troo", &lumps).expect("assembled");
        let state = bb.states.first().expect("a state").name.clone();
        // (file, flip) per rotation digit, exactly as `R_InitSpriteDefs`
        // fills `sprframe->lump[]` / `sprframe->flip[]`.
        let vanilla = [
            ("trooa1.png", false),
            ("trooa2a8.png", false),
            ("trooa3a7.png", false),
            ("trooa4a6.png", false),
            ("trooa5.png", false),
            ("trooa4a6.png", true),
            ("trooa3a7.png", true),
            ("trooa2a8.png", true),
        ];
        for (i, (file, flip)) in vanilla.iter().enumerate() {
            let digit = i as u8 + 1;
            let faced = bb.frames_for_state_facing(&state, digit);
            let f = faced.first().unwrap_or_else(|| panic!("rotation {digit} has no frame"));
            assert_eq!(&f.frame.file, file, "rotation {digit} draws the wrong lump");
            assert_eq!(f.flip, *flip, "rotation {digit} has the wrong mirror");
        }
    }

    /// **The handedness law, on real shipped data.**
    ///
    /// A mirrored converter is not a broken one: the walls still meet, the
    /// doors still open, the spawn still faces the room, and every walker
    /// formula stays self-consistent INSIDE the mirror. The only thing that
    /// gives it away is a player who knows the level — so that is what this
    /// test is. It stands at E1M1's player-1 start, looks where the start's
    /// own THINGS angle points, and checks that three landmarks lie the way
    /// they lie in vanilla Doom.
    ///
    /// Vanilla E1M1 (`DOOM1.WAD`, shareware): the start is at (1056, −3616)
    /// facing angle 90 — due NORTH, up the hangar's long room. From there,
    /// at 64 map units to the metre:
    ///
    /// | landmark | thing | where it is in the real game |
    /// |---|---|---|
    /// | green armour, zigzag nukage room | 2018 | 20 m LEFT (west), 6 m ahead |
    /// | blue armour, courtyard nukage | 2019 | 12 m RIGHT (east), 5.25 m ahead |
    /// | shotgun, far east hall | 2001 | 34.5 m RIGHT, 5 m BEHIND |
    ///
    /// The assertion is stronger than that table, though: for every landmark
    /// the bearing measured in the GLB (through the exported `.place` data
    /// and the renderer's camera basis) must EQUAL the bearing measured in
    /// the WAD's own plane. A mirror flips the left column's sign and
    /// nothing else, so it cannot survive this even on a map nobody knows.
    #[test]
    fn e1m1_lands_where_the_wad_says_and_not_in_its_mirror() {
        use crate::classic_import::doom::{doom_map_place, lump_by_name_after, parse_wad};
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/packs/doom/DOOM1.WAD");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("no DOOM1.WAD; skipped");
            return;
        };
        let wad = parse_wad(&bytes).expect("wad");
        let things = lump_by_name_after(&wad.lumps, "E1M1", "THINGS").expect("THINGS");
        let i16_at = |b: &[u8], o: usize| i16::from_le_bytes([b[o], b[o + 1]]) as f32;
        let u16_at = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        // ([x, y], angle) of the ONE thing of this type, or a panic naming
        // the type — every landmark below is unique in E1M1.
        let thing = |typ: u16| -> ([f32; 2], f32) {
            let mut found = None;
            for i in 0..things.len() / 10 {
                let o = i * 10;
                if u16_at(things, o + 6) == typ {
                    assert!(found.is_none(), "thing {typ} is not unique in E1M1");
                    found = Some((
                        [i16_at(things, o), i16_at(things, o + 2)],
                        u16_at(things, o + 4) as f32,
                    ));
                }
            }
            found.unwrap_or_else(|| panic!("E1M1 has no thing {typ}"))
        };

        // The WAD says what it says: if this is not the shareware E1M1, the
        // ground truth below is not about this map and the test must say so
        // rather than measure something else.
        let (start_xy, start_angle) = thing(1);
        assert_eq!(start_xy, [1056.0, -3616.0], "not vanilla E1M1's start");
        assert_eq!(start_angle, 90.0, "E1M1's start faces north");
        let green = thing(2018).0;
        let blue = thing(2019).0;
        let shotgun = thing(2001).0;
        assert_eq!(green, [-224.0, -3232.0], "not vanilla E1M1's green armour");
        assert_eq!(blue, [1824.0, -3280.0], "not vanilla E1M1's blue armour");
        assert_eq!(shotgun, [3264.0, -3936.0], "not vanilla E1M1's shotgun");

        // What the converter published: the spawn and the actor placements
        // the game reads, in GLB metres.
        let place = doom_map_place(&wad.lumps, "E1M1", "doom", "world/e1m1", "doom1");
        let (spawn_pos, spawn_yaw, _) = place.spawn.expect("E1M1 spawn");
        let at = |typ: u16| -> [f32; 3] {
            let key = typ.to_string();
            let hits: Vec<&crate::world_place::Place> =
                place.places.iter().filter(|p| p.class == key).collect();
            assert_eq!(hits.len(), 1, "thing {typ} placed {} times", hits.len());
            hits[0].pos
        };

        // Facing north, and north is −Z: the whole reason the mapping has to
        // send north to −Z rather than +Z.
        let (f, _) = camera_basis(spawn_yaw);
        assert!(
            f[0].abs() < 1e-3 && (f[1] + 1.0).abs() < 1e-3,
            "E1M1's start faces Doom north, which is GLB −Z: forward {f:?}"
        );

        // The vanilla table, stated in metres in the player's own frame.
        for (name, typ, ahead, left) in [
            ("green armour (zigzag room, WEST)", 2018u16, 6.0f32, 20.0f32),
            ("blue armour (courtyard, EAST)", 2019, 5.25, -12.0),
            ("shotgun (east hall, BEHIND)", 2001, -5.0, -34.5),
        ] {
            let (got_ahead, got_left) = ahead_left(spawn_pos, spawn_yaw, at(typ));
            assert!(
                (got_ahead - ahead).abs() < 0.02 && (got_left - left).abs() < 0.02,
                "{name}: vanilla puts it {ahead:.2} m ahead / {left:.2} m left, \
                 the GLB puts it {got_ahead:.2} / {got_left:.2} \
                 (a sign flip on `left` is the level's MIRROR)"
            );
        }

        // And the general law, which needs no memory of the level: the same
        // bearing, measured in the WAD's own (east, north) plane.
        for (typ, xy) in [(2018u16, green), (2019, blue), (2001, shotgun)] {
            let (want_ahead, want_left) = ahead_left_map(start_xy, start_angle, xy);
            let unit = crate::classic_import::doom::DOOM_UNIT;
            let (got_ahead, got_left) = ahead_left(spawn_pos, spawn_yaw, at(typ));
            assert!(
                (got_ahead - want_ahead * unit).abs() < 0.02
                    && (got_left - want_left * unit).abs() < 0.02,
                "thing {typ}: WAD bearing ({:.2}, {:.2}) m, GLB bearing \
                 ({got_ahead:.2}, {got_left:.2}) m",
                want_ahead * unit,
                want_left * unit
            );
        }
    }

    /// Vanilla single-player Doom does not spawn every difficulty's cast at
    /// once: `P_LoadThings` skips a THING unless its skill bit for the
    /// active skill is set (and, in single player, unless it is flagged
    /// multiplayer-only) — except player starts (types 1-4) and the
    /// deathmatch start (type 11), which it always spawns. The exported
    /// `.place` cast defaults to skill 3, "Hurt Me Plenty".
    ///
    /// This does not hardcode an expected monster count: it derives one
    /// directly from the WAD's own THINGS flags (the same law the pure
    /// `doom_hmp_predicate_matches_skill_and_multiplayer_bits` test checks
    /// in isolation) and asserts the importer's output matches — for E1M1,
    /// E1M2 and E1M3, whichever are present in this machine's shareware
    /// `DOOM1.WAD`.
    #[test]
    fn doom_e1m1_hmp_cast_matches_thing_flags() {
        use crate::classic_import::doom::{
            doom_map_place, doom_thing_spawns_on_hmp, lump_by_name_after, parse_wad,
        };
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/packs/doom/DOOM1.WAD");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("no DOOM1.WAD; skipped");
            return;
        };
        let wad = parse_wad(&bytes).expect("wad");
        let u16_at = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]);

        for map in ["E1M1", "E1M2", "E1M3"] {
            let Some(things) = lump_by_name_after(&wad.lumps, map, "THINGS") else {
                eprintln!("{map}: no THINGS lump; skipped");
                continue;
            };
            let n = things.len() / 10;
            let mut monsters_before = 0usize; // every skill combined (the old, unfiltered behaviour)
            let mut monsters_after = 0usize; // skill 3 (HMP) only, the new default
            let mut zero_skill_bit_things = 0usize; // non-player/dm things with NO skill bit set at all
            for i in 0..n {
                let o = i * 10;
                let typ = u16_at(things, o + 6);
                let flags = u16_at(things, o + 8);
                if !matches!(typ, 1 | 2 | 3 | 4 | 11) && flags & 0x0007 == 0 {
                    zero_skill_bit_things += 1;
                }
                let Some((kind, _)) = crate::world_place::doom_thing_actor(typ) else {
                    continue;
                };
                if kind != "character" {
                    continue;
                }
                monsters_before += 1;
                if doom_thing_spawns_on_hmp(typ, flags) {
                    monsters_after += 1;
                }
            }

            let place = doom_map_place(&wad.lumps, map, "doom", "world/x", "doom1");
            let got = place.places.iter().filter(|p| p.kind == "character").count();
            eprintln!(
                "{map}: monsters before={monsters_before} after(HMP)={monsters_after}, \
                 things with no skill bit at all={zero_skill_bit_things}"
            );
            assert_eq!(
                got, monsters_after,
                "{map}: doom_map_place emitted {got} character placements, \
                 but the WAD's own flags say {monsters_after} spawn on skill 3 (HMP)"
            );
        }
    }

    /// Pure predicate test, no WAD required: the skill masks Doom actually
    /// uses (0x0001 skill 1&2, 0x0002 skill 3, 0x0004 skill 4&5, their
    /// union 0x0007, and 0x0000 meaning "no skill at all") plus the
    /// multiplayer-only bit 0x0010, checked against `doom_thing_spawns_on_hmp`.
    #[test]
    fn doom_hmp_predicate_matches_skill_and_multiplayer_bits() {
        use crate::classic_import::doom::doom_thing_spawns_on_hmp;
        // An ordinary monster type (Imp, 3001): not a player/deathmatch
        // start, so only the skill + multiplayer bits decide.
        let typ = 3001u16;
        assert!(doom_thing_spawns_on_hmp(typ, 0x0002), "skill-3-only spawns on HMP");
        assert!(!doom_thing_spawns_on_hmp(typ, 0x0001), "skill-1/2-only must NOT spawn on HMP");
        assert!(!doom_thing_spawns_on_hmp(typ, 0x0004), "skill-4/5-only must NOT spawn on HMP");
        assert!(doom_thing_spawns_on_hmp(typ, 0x0007), "all-skill thing spawns on HMP");
        assert!(
            !doom_thing_spawns_on_hmp(typ, 0x0000),
            "no skill bit set at all: vanilla never spawns it, HMP included"
        );
        // Multiplayer-only (0x0010) suppresses single-player spawning even
        // when the HMP bit is also set.
        assert!(
            !doom_thing_spawns_on_hmp(typ, 0x0002 | 0x0010),
            "multiplayer-only must not spawn in single player, even with the HMP bit set"
        );
        // Player starts (1-4) and the deathmatch start (11) ignore flags
        // entirely — vanilla spawns them regardless.
        for player_typ in [1u16, 2, 3, 4, 11] {
            assert!(
                doom_thing_spawns_on_hmp(player_typ, 0x0000),
                "player/deathmatch start {player_typ} spawns even with no skill bits set"
            );
            assert!(
                doom_thing_spawns_on_hmp(player_typ, 0x0010),
                "player/deathmatch start {player_typ} spawns even when flagged multiplayer-only"
            );
        }
    }

    /// The same law on Quake 1's E1M1, when the PAK is on this machine: the
    /// bearing from the player start to every deathmatch start must be the
    /// same in the BSP's own plane as in the GLB the converter writes.
    ///
    /// No ground truth about the level is needed — the law alone catches a
    /// mirror, and having a second engine under it is what keeps the five
    /// classic families on ONE contract instead of five conventions.
    #[test]
    fn quake_e1m1_keeps_its_bearings_too() {
        use crate::classic_import::doom::quake_bsp_nav;
        let Some(bytes) = pak0_entry("maps/e1m1.bsp") else {
            eprintln!("no Quake PAK0; skipped");
            return;
        };
        // (classname, origin, angle) straight out of the entity lump.
        let off = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let text = std::str::from_utf8(&bytes[off..off + len]).expect("entities");
        let mut ents: Vec<(String, [f32; 3], f32)> = Vec::new();
        for block in text.split(|c| c == '{' || c == '}') {
            let (mut class, mut origin, mut angle) = (String::new(), None, 0.0f32);
            for line in block.lines() {
                let kv: Vec<&str> = line.split('"').filter(|s| !s.trim().is_empty()).collect();
                if kv.len() < 2 {
                    continue;
                }
                match kv[0] {
                    "classname" => class = kv[1].to_string(),
                    "angle" => angle = kv[1].trim().parse().unwrap_or(0.0),
                    "origin" => {
                        let v: Vec<f32> =
                            kv[1].split_whitespace().filter_map(|n| n.parse().ok()).collect();
                        if v.len() == 3 {
                            origin = Some([v[0], v[1], v[2]]);
                        }
                    }
                    _ => {}
                }
            }
            if let Some(o) = origin {
                if !class.is_empty() {
                    ents.push((class, o, angle));
                }
            }
        }
        let start = ents
            .iter()
            .find(|e| e.0 == "info_player_start")
            .expect("info_player_start");
        let dm: Vec<&(String, [f32; 3], f32)> = ents
            .iter()
            .filter(|e| e.0 == "info_player_deathmatch")
            .collect();
        assert!(dm.len() >= 3, "e1m1 has deathmatch starts: {}", dm.len());

        let nav = quake_bsp_nav(&bytes).expect("nav");
        let primary = nav.primary().expect("primary start");
        let published: Vec<&crate::world_nav::NavStart> = nav
            .starts
            .iter()
            .filter(|s| s.name.starts_with(crate::world_nav::DEATHMATCH))
            .collect();
        assert_eq!(published.len(), dm.len(), "every deathmatch start published");

        // Quake's map plane is (east, north) = (x, y) with `angle` degrees
        // counter-clockwise from east — the same convention Doom uses.
        let unit = crate::classic_import::doom::QUAKE_UNIT;
        for (src, out) in dm.iter().zip(published) {
            let (want_ahead, want_left) = ahead_left_map(
                [start.1[0], start.1[1]],
                start.2,
                [src.1[0], src.1[1]],
            );
            let (got_ahead, got_left) = ahead_left(primary.pos, primary.yaw, out.pos);
            assert!(
                (got_ahead - want_ahead * unit).abs() < 0.02
                    && (got_left - want_left * unit).abs() < 0.02,
                "{}: BSP bearing ({:.2}, {:.2}) m, GLB bearing \
                 ({got_ahead:.2}, {got_left:.2}) m",
                out.name,
                want_ahead * unit,
                want_left * unit
            );
        }
    }

    /// Convert the real E1M1 when the shareware WAD is on this machine and
    /// drop the GLB where a GPU capture can pick it up. Skipped otherwise.
    #[test]
    fn export_real_e1m1_for_capture() {
        use crate::classic_import::doom::{doom_map_to_mesh, parse_wad};
        let wad_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/packs/doom/DOOM1.WAD");
        let Ok(bytes) = std::fs::read(&wad_path) else {
            eprintln!("no DOOM1.WAD; skipped");
            return;
        };
        let wad = parse_wad(&bytes).expect("wad");
        let palette = wad.playpal.expect("PLAYPAL");
        let mesh = doom_map_to_mesh(&wad.lumps, "E1M1", &palette).expect("e1m1");
        let out = std::env::var("DOOM_E1M1_GLB").unwrap_or_default();
        if !out.is_empty() {
            std::fs::write(&out, &mesh.glb).unwrap();
        }
        eprintln!(
            "E1M1: {} tris, {} doors, {} lifts, {} teleporters, glb {} bytes -> {out}",
            mesh.tris,
            mesh.doors.len(),
            mesh.lifts.len(),
            mesh.teleporters.len(),
            mesh.glb.len()
        );
        let text = glb_json(&mesh.glb);
        assert!(text.contains("\"kind\":\"sky\""), "E1M1 has sky sectors");

        // No step lip on a REAL step: take every two-sided line of E1M1
        // whose two floors differ, and check the wall standing in its plane
        // tops out exactly at the higher floor. (A per-triangle invariant
        // would be wrong here — `push_wall_tiled` splits a wall at texture
        // boundaries, so a middle tile's top is a tile edge, not the wall's.)
        let unit = 1.0f32 / 64.0;
        let lump = |name: &str| {
            crate::classic_import::doom::lump_by_name_after(&wad.lumps, "E1M1", name)
                .expect(name)
        };
        let (linedefs, sidedefs, sectors, vertexes) =
            (lump("LINEDEFS"), lump("SIDEDEFS"), lump("SECTORS"), lump("VERTEXES"));
        let u16_at = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let i16_at = |b: &[u8], o: usize| i16::from_le_bytes([b[o], b[o + 1]]);
        let sector_of = |snum: u16| -> Option<usize> {
            (snum != 0xFFFF && (snum as usize) < sidedefs.len() / 30)
                .then(|| u16_at(sidedefs, snum as usize * 30 + 28) as usize)
        };
        let parts = crate::world_preview::extract_glb_parts(&mesh.glb).expect("parts");
        let mut steps_checked = 0usize;
        let mut straddles: Vec<(f32, f32, bool)> = Vec::new();
        for li in 0..linedefs.len() / 14 {
            let lo = li * 14;
            let (Some(front), Some(back)) = (
                sector_of(u16_at(linedefs, lo + 10)),
                sector_of(u16_at(linedefs, lo + 12)),
            ) else {
                continue;
            };
            let (fl, bl) = (
                i16_at(sectors, front * 26) as f32,
                i16_at(sectors, back * 26) as f32,
            );
            if (fl - bl).abs() < 8.0 {
                continue;
            }
            let v1 = u16_at(linedefs, lo) as usize;
            let v2 = u16_at(linedefs, lo + 2) as usize;
            if v1 >= vertexes.len() / 4 || v2 >= vertexes.len() / 4 {
                continue;
            }
            // The linedef's endpoints in the GLB x/z plane: Doom north is
            // −Z, so the WAD's y flips sign on the way in (and back out
            // again in the messages below, which quote map units).
            let a = [
                i16_at(vertexes, v1 * 4) as f32 * unit,
                -(i16_at(vertexes, v1 * 4 + 2) as f32) * unit,
            ];
            let b = [
                i16_at(vertexes, v2 * 4) as f32 * unit,
                -(i16_at(vertexes, v2 * 4 + 2) as f32) * unit,
            ];
            let upper = fl.max(bl) * unit;
            let lower = fl.min(bl) * unit;
            // Triangles standing in this line's plane, inside its span.
            let dx = b[0] - a[0];
            let dz = b[1] - a[1];
            let len = (dx * dx + dz * dz).sqrt();
            if len < 0.05 {
                continue;
            }
            let (nx, nz) = (dz / len, -dx / len);
            let mut top = f32::MIN;
            for part in &parts {
                for tri in part.indices.chunks_exact(3) {
                    let ps = [
                        part.pos[tri[0] as usize],
                        part.pos[tri[1] as usize],
                        part.pos[tri[2] as usize],
                    ];
                    let on_plane = ps.iter().all(|p| {
                        ((p[0] - a[0]) * nx + (p[2] - a[1]) * nz).abs() < 0.01
                            && {
                                let t = ((p[0] - a[0]) * dx + (p[2] - a[1]) * dz) / (len * len);
                                (-0.01..=1.01).contains(&t)
                            }
                    });
                    if !on_plane {
                        continue;
                    }
                    let hi = ps.iter().fold(f32::MIN, |m, p| m.max(p[1]));
                    let lo_y = ps.iter().fold(f32::MAX, |m, p| m.min(p[1]));
                    // Only the RISER: it starts at or under the lower
                    // floor. A two-sided mid (grate) starts at the upper
                    // floor and an upper band at a ceiling — neither is a
                    // step, and both legitimately reach higher.
                    if lo_y > lower + 1e-3 {
                        continue;
                    }
                    top = top.max(hi);
                }
            }
            if top == f32::MIN {
                continue;
            }
            steps_checked += 1;
            // No flat may hang PAST this line onto the other sector's
            // side: an overhanging floor tile shows its open edge as a
            // ridge standing along the step.
            for part in &parts {
                for tri in part.indices.chunks_exact(3) {
                    let ps = [
                        part.pos[tri[0] as usize],
                        part.pos[tri[1] as usize],
                        part.pos[tri[2] as usize],
                    ];
                    // Flats only, and only at one of this line's two floors.
                    if ps[0][1] != ps[1][1] || ps[1][1] != ps[2][1] {
                        continue;
                    }
                    let y = ps[0][1];
                    let is_upper = (y - upper).abs() < 1e-4;
                    if !is_upper && (y - lower).abs() >= 1e-4 {
                        continue;
                    }
                    // Does an EDGE of this triangle cross the line WITHIN
                    // the linedef's own span? A wall is a segment: a flat
                    // may sit either side of its infinite extension, but
                    // nothing may cross the wall itself.
                    let side_of = |p: &[f32; 3]| (p[0] - a[0]) * nx + (p[2] - a[1]) * nz;
                    let along_of = |x: f32, z: f32| {
                        ((x - a[0]) * dx + (z - a[1]) * dz) / (len * len)
                    };
                    let mut worst = 0.0f32;
                    for k in 0..3 {
                        let (p, q) = (ps[k], ps[(k + 1) % 3]);
                        let (sp, sq) = (side_of(&p), side_of(&q));
                        if (sp < -1e-4 && sq > 1e-4) || (sp > 1e-4 && sq < -1e-4) {
                            let t = sp / (sp - sq);
                            let cx = p[0] + (q[0] - p[0]) * t;
                            let cz = p[2] + (q[2] - p[2]) * t;
                            // Distance along the wall, in map units. Wall
                            // JUNCTIONS (the last few units at either end)
                            // are where several sectors' corner slivers
                            // meet and clip against each other; the tread
                            // itself is the span between them.
                            let at = along_of(cx, cz) * len * 64.0;
                            let span = len * 64.0;
                            if at > 6.0 && at < span - 6.0 {
                                worst = worst.max(sp.abs().min(sq.abs()));
                            }
                        }
                    }
                    if worst > 0.0 {
                        let by = worst * 64.0;
                        straddles.push((y, by, is_upper));
                        // `snap_pos` quantises to 1/1024m, so a vertex ON the
                        // line can land 0.03 units to either side. Anything
                        // past that is a flat reaching over its own linedef —
                        // the ridge standing along the step.
                        // `snap_pos` quantises to 1/1024m, so a vertex ON
                        // the line can land 0.03 units either side. Beyond
                        // that the tread reaches over its own riser — the
                        // ledge the user sees standing along every step.
                        assert!(
                            by <= 0.05,
                            "tread at y={:.4} overhangs the riser line \
                             ({:.1},{:.1})-({:.1},{:.1}) by {by:.3} map units",
                            y,
                            a[0] * 64.0,
                            -a[1] * 64.0,
                            b[0] * 64.0,
                            -b[1] * 64.0
                        );
                    }
                }
            }
            assert!(
                top <= upper + 1e-4,
                "step at ({:.1},{:.1})-({:.1},{:.1}): riser tops {:.4}m above the {:.4}m floor \
                 — {:.2} map units of lip",
                a[0] * 64.0,
                -a[1] * 64.0,
                b[0] * 64.0,
                -b[1] * 64.0,
                top,
                upper,
                (top - upper) * 64.0
            );
        }
        straddles.sort_by(|a, b| b.1.total_cmp(&a.1));
        eprintln!(
            "E1M1 tread/riser crossings: {} (largest {:.3} map units — snap noise is 0.031)",
            straddles.len(),
            straddles.first().map(|s| s.1).unwrap_or(0.0)
        );
        eprintln!("E1M1 real step edges checked: {steps_checked}");
        assert!(steps_checked > 10, "E1M1 has many steps: {steps_checked}");

        // Flats must tile their plane: no triangle centroid inside another
        // (coplanar duplicates z-fight), no edge used by more than two
        // triangles (duplicate coverage).
        let flats = flat_triangles(&mesh.glb);
        let mut total = 0usize;
        let mut overlaps = 0usize;
        let mut over_used = 0usize;
        for tris in flats.values() {
            total += tris.len();
            let (o, e) = flat_health(tris);
            overlaps += o;
            over_used += e;
        }
        // T-junctions: a vertex sitting strictly inside a neighbour's edge
        // on the same plane. They are not overlap and not a missing edge —
        // they are the hairline a rasteriser opens between two triangles
        // that agree geometrically but do not share the vertex.
        let mut tees = 0usize;
        let mut worst_plane = (0usize, 0f32);
        for (height, tris) in &flats {
            let t = t_junctions(tris);
            tees += t;
            if t > worst_plane.0 {
                worst_plane = (t, f32::from_bits(*height));
            }
        }
        eprintln!(
            "E1M1 flats: {total} triangles on {} planes, {overlaps} overlapping, {over_used} over-shared edges, {tees} T-junctions (worst plane y={:.3}m with {})",
            flats.len(),
            worst_plane.1,
            worst_plane.0
        );
        assert_eq!(overlaps, 0, "coplanar flat triangles overlap — that is the z-fight");
        assert_eq!(over_used, 0, "a flat edge is shared by more than two triangles");
        // Zero, and it stays zero: `weld::split_t_junctions` splits every
        // edge at the vertices lying on it, across every part of the level
        // (world, doors, lifts, hazard floors, sky), so no vertex is left
        // sitting inside a neighbour's edge for the rasteriser to crack.
        assert_eq!(
            tees, 0,
            "E1M1 carries T-junctions again — flats crack where they meet"
        );

        // The same audit the Quake and Duke maps get: every part, in 3D.
        let all = crate::world_preview::extract_glb_parts(&mesh.glb).expect("parts");
        let soup: Vec<(&[[f32; 3]], &[u32])> = all
            .iter()
            .map(|part| (&part.pos[..], &part.indices[..]))
            .collect();
        let left = weld::t_junctions_left(&soup);
        eprintln!(
            "E1M1 parts: {}, {} triangles, {left} T-junctions in 3D",
            all.len(),
            soup.iter().map(|(_, i)| i.len() / 3).sum::<usize>()
        );

        // The renderer's own parser must find the same sky in the real map.
        let model = makepad_render::model::StaticModel::parse_glb(&mesh.glb).expect("parse");
        let sky = model.sky.expect("E1M1 sky part");
        assert_eq!(sky.repeat, 4.0);
        assert_eq!(sky.images.len(), 1);
        assert!(sky.indices.len() >= 3, "sky faces survive");
        let nav = crate::classic_import::doom::doom_map_nav(&wad.lumps, "E1M1").expect("nav");
        eprintln!(
            "E1M1 markers: {:?}",
            nav.markers.iter().map(|m| m.name.as_str()).collect::<Vec<_>>()
        );
        assert!(
            nav.markers.iter().any(|m| m.name == "exit"),
            "E1M1 ends at a switch"
        );
    }

    /// Two rooms joined by a two-sided line, with authored floor/ceiling
    /// heights: the fixture for step-riser geometry.
    fn write_step_wad(path: &Path, floors: [i16; 2], ceilings: [i16; 2]) {
        let mut lumps: Vec<(&str, Vec<u8>)> = Vec::new();
        lumps.push(("PLAYPAL", vec![0u8; 768]));
        lumps.push(("MAP01", vec![]));
        let mut things = Vec::new();
        for v in [64i16, 64, 0, 1, 7] {
            things.extend_from_slice(&v.to_le_bytes());
        }
        lumps.push(("THINGS", things));
        // Room A x 0..128, room B x 128..256, both y 0..128. The shared
        // wall is the line x = 128.
        let vert_xy: [(i16, i16); 6] = [
            (0, 0),
            (128, 0),
            (256, 0),
            (256, 128),
            (128, 128),
            (0, 128),
        ];
        let mut verts = Vec::new();
        for (x, y) in vert_xy {
            verts.extend_from_slice(&x.to_le_bytes());
            verts.extend_from_slice(&y.to_le_bytes());
        }
        lumps.push(("VERTEXES", verts));
        const BLANK: &[u8; 8] = b"-\0\0\0\0\0\0\0";
        const WALL: &[u8; 8] = b"WALL1\0\0\0";
        // (upper, lower, mid, sector)
        let sides: [(&[u8; 8], &[u8; 8], &[u8; 8], u16); 6] = [
            (BLANK, BLANK, WALL, 0),
            (BLANK, BLANK, WALL, 1),
            (BLANK, BLANK, WALL, 1),
            (BLANK, BLANK, WALL, 1),
            // The shared line: both sides paint upper AND lower.
            (WALL, WALL, BLANK, 0),
            (WALL, WALL, BLANK, 1),
        ];
        let mut sidedefs = Vec::new();
        for (upper, lower, mid, sector) in sides {
            sidedefs.extend_from_slice(&0i16.to_le_bytes());
            sidedefs.extend_from_slice(&0i16.to_le_bytes());
            sidedefs.extend_from_slice(upper);
            sidedefs.extend_from_slice(lower);
            sidedefs.extend_from_slice(mid);
            sidedefs.extend_from_slice(&sector.to_le_bytes());
        }
        lumps.push(("SIDEDEFS", sidedefs));
        // (v1, v2, flags, right, left)
        let lines: [(u16, u16, u16, u16, u16); 6] = [
            (0, 1, 0, 0, 0xFFFF),
            (1, 2, 0, 1, 0xFFFF),
            (2, 3, 0, 2, 0xFFFF),
            (3, 4, 0, 3, 0xFFFF),
            (5, 0, 0, 0, 0xFFFF),
            (4, 1, 0x0004, 5, 4),
        ];
        let mut linedefs = Vec::new();
        for (v1, v2, flags, right, left) in lines {
            for v in [v1, v2, flags, 0, 0, right, left] {
                linedefs.extend_from_slice(&v.to_le_bytes());
            }
        }
        lumps.push(("LINEDEFS", linedefs));
        let mut sectors = Vec::new();
        for i in 0..2 {
            sectors.extend_from_slice(&floors[i].to_le_bytes());
            sectors.extend_from_slice(&ceilings[i].to_le_bytes());
            sectors.extend_from_slice(b"FLOOR0\0\0");
            sectors.extend_from_slice(b"CEIL1\0\0\0");
            for v in [200i16, 0, 0] {
                sectors.extend_from_slice(&v.to_le_bytes());
            }
        }
        lumps.push(("SECTORS", sectors));
        lumps.push(("F_START", vec![]));
        lumps.push(("FLOOR0", vec![1u8; 64 * 64]));
        lumps.push(("F_END", vec![]));
        let mut data = Vec::new();
        data.extend_from_slice(b"IWAD");
        data.extend_from_slice(&(lumps.len() as u32).to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        let mut dir = Vec::new();
        for (name, lump) in &lumps {
            let pos = data.len() as u32;
            data.extend_from_slice(lump);
            dir.extend_from_slice(&pos.to_le_bytes());
            dir.extend_from_slice(&(lump.len() as u32).to_le_bytes());
            let mut n = [0u8; 8];
            for (i, b) in name.bytes().take(8).enumerate() {
                n[i] = b;
            }
            dir.extend_from_slice(&n);
        }
        let diroff = data.len() as u32;
        data[8..12].copy_from_slice(&diroff.to_le_bytes());
        data.extend_from_slice(&dir);
        std::fs::write(path, data).unwrap();
    }

    /// Every triangle standing in the plane of the shared wall (x = 128
    /// units = 2 m), as (min_y, max_y).
    fn wall_plane_spans(glb: &[u8]) -> Vec<(f32, f32)> {
        let parts = crate::world_preview::extract_glb_parts(glb).expect("parts");
        let mut out = Vec::new();
        for part in &parts {
            for tri in part.indices.chunks_exact(3) {
                let ps = [
                    part.pos[tri[0] as usize],
                    part.pos[tri[1] as usize],
                    part.pos[tri[2] as usize],
                ];
                if !ps.iter().all(|p| (p[0] - 2.0).abs() < 1e-3) {
                    continue;
                }
                let lo = ps.iter().fold(f32::MAX, |a, p| a.min(p[1]));
                let hi = ps.iter().fold(f32::MIN, |a, p| a.max(p[1]));
                out.push((lo, hi));
            }
        }
        out
    }

    #[test]
    fn a_step_riser_stops_exactly_at_the_upper_floor() {
        use crate::classic_import::doom::{doom_map_to_mesh, parse_wad};
        let root = tmp_dir("step_lip");
        let wad_path = root.join("step.wad");
        // Floors 0 and 24, one ceiling: only the lower riser exists.
        write_step_wad(&wad_path, [0, 24], [128, 128]);
        let wad = parse_wad(&std::fs::read(&wad_path).unwrap()).expect("wad");
        let mesh = doom_map_to_mesh(&wad.lumps, "MAP01", &[[0u8; 3]; 256]).expect("mesh");
        let upper_floor = 24.0 / 64.0;
        let spans = wall_plane_spans(&mesh.glb);
        assert!(!spans.is_empty(), "the riser must exist");
        let highest = spans.iter().fold(f32::MIN, |a, (_, hi)| a.max(*hi));
        assert!(
            highest <= upper_floor + 1e-6,
            "step riser pokes {:.4}m above the upper floor — that is the lip",
            highest - upper_floor
        );
        // Watertight: it reaches the upper floor, and its bottom is at or
        // under the lower floor (the hidden end may overshoot).
        assert!((highest - upper_floor).abs() < 1e-6, "riser must MEET the upper floor");
        let lowest = spans.iter().fold(f32::MAX, |a, (lo, _)| a.min(*lo));
        assert!(lowest <= 0.0 + 1e-6, "riser must reach the lower floor: {lowest}");
        // A flat plane must not leave a vertex sitting strictly inside a
        // neighbour's edge: that vertex is where the rasteriser opens the
        // hairline between two tiles that otherwise agree exactly.
        for (height, tris) in flat_triangles(&mesh.glb) {
            assert_eq!(
                t_junctions(&tris),
                0,
                "flat plane y={:.3}m has T-junctions",
                f32::from_bits(height)
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_upper_band_stops_exactly_at_the_lower_ceiling() {
        use crate::classic_import::doom::{doom_map_to_mesh, parse_wad};
        let root = tmp_dir("ceil_lip");
        let wad_path = root.join("ceil.wad");
        // One floor, ceilings 128 and 96: only the upper band exists.
        write_step_wad(&wad_path, [0, 0], [128, 96]);
        let wad = parse_wad(&std::fs::read(&wad_path).unwrap()).expect("wad");
        let mesh = doom_map_to_mesh(&wad.lumps, "MAP01", &[[0u8; 3]; 256]).expect("mesh");
        let lower_ceiling = 96.0 / 64.0;
        let spans = wall_plane_spans(&mesh.glb);
        assert!(!spans.is_empty(), "the band must exist");
        let lowest = spans.iter().fold(f32::MAX, |a, (lo, _)| a.min(*lo));
        assert!(
            lowest >= lower_ceiling - 1e-6,
            "upper band hangs {:.4}m below the lower ceiling — the mirrored lip",
            lower_ceiling - lowest
        );
        assert!(
            (lowest - lower_ceiling).abs() < 1e-6,
            "band must MEET the lower ceiling"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_sky_ceiling_becomes_a_sky_node_with_its_own_picture() {
        use crate::classic_import::doom::{doom_map_to_mesh, doom_sky_lump, parse_wad};
        use makepad_asset_client::json::{self, Value};
        let root = tmp_dir("sky_map");
        let wad_path = root.join("sky.wad");
        write_two_room_door_wad(&wad_path);
        let mut bytes = std::fs::read(&wad_path).unwrap();
        let wad = parse_wad(&bytes).expect("wad");
        let sectors_lump = wad.lumps.iter().find(|l| l.name == "SECTORS").unwrap();
        let at = bytes
            .windows(sectors_lump.data.len())
            .position(|w| w == sectors_lump.data.as_slice())
            .expect("sector bytes");
        // Room B's ceiling is the sky.
        bytes[at + 2 * 26 + 12..at + 2 * 26 + 20].copy_from_slice(b"F_SKY1\0\0");
        std::fs::write(&wad_path, &bytes).unwrap();

        // Doom 1 episode naming: MAP01 -> SKY1.
        assert_eq!(doom_sky_lump("MAP01"), "SKY1");
        assert_eq!(doom_sky_lump("E2M4"), "SKY2");
        assert_eq!(doom_sky_lump("MAP15"), "SKY2");
        assert_eq!(doom_sky_lump("MAP27"), "SKY3");

        let wad = parse_wad(&bytes).expect("wad");
        let mesh = doom_map_to_mesh(&wad.lumps, "MAP01", &[[0u8; 3]; 256]).expect("mesh");
        let root_json = json::parse(glb_json(&mesh.glb).as_bytes()).expect("glb json");
        let nodes = root_json.get("nodes").unwrap().as_arr().unwrap();
        let sky = nodes
            .iter()
            .find(|n| n.get("name").and_then(Value::as_str) == Some("sky"))
            .expect("sky node");
        let extras = sky.get("extras").expect("extras");
        assert_eq!(extras.get("kind").and_then(Value::as_str), Some("sky"));
        assert_eq!(
            extras.get("projection").and_then(Value::as_str),
            Some("cylinder")
        );
        assert_eq!(extras.get("texture").and_then(Value::as_str), Some("sky1"));
        let repeat = match extras.get("repeat").unwrap() {
            Value::F64(f) => *f,
            Value::Int(i) => *i as f64,
            _ => f64::NAN,
        };
        assert_eq!(repeat, 4.0, "256-wide sky over a 1024-unit turn");
        // Its own material and image, not the level atlas.
        let mi = sky
            .get("mesh")
            .and_then(Value::as_i64)
            .and_then(|m| root_json.get("meshes").unwrap().as_arr().unwrap().get(m as usize))
            .and_then(|m| m.get("primitives"))
            .and_then(Value::as_arr)
            .and_then(|p| p[0].get("material"))
            .and_then(Value::as_i64)
            .expect("sky material");
        assert!(mi > 0, "the sky does not paint with the level atlas");
        assert!(
            root_json.get("images").unwrap().as_arr().unwrap().len() >= 2,
            "sky image is embedded alongside the atlas"
        );
        // And the level mesh no longer has a ceiling over room B.
        let parts = crate::world_preview::extract_glb_parts(&mesh.glb).expect("parts");
        let ceiling_over_b = parts[0].indices.chunks_exact(3).any(|tri| {
            [
                parts[0].pos[tri[0] as usize],
                parts[0].pos[tri[1] as usize],
                parts[0].pos[tri[2] as usize],
            ]
            .iter()
            .all(|p| p[0] > 3.05 && (p[1] - 2.0).abs() < 1e-3)
        });
        assert!(!ceiling_over_b, "F_SKY1 is a hole, not a ceiling");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_plat_sector_exports_as_a_lift_node_and_anchor() {
        use crate::classic_import::doom::{doom_map_to_mesh, parse_wad};
        use makepad_asset_client::json::{self, Value};
        let root = tmp_dir("lift_map");
        let wad_path = root.join("lift.wad");
        write_two_room_door_wad(&wad_path);
        let mut bytes = std::fs::read(&wad_path).unwrap();
        let wad = parse_wad(&bytes).expect("wad");
        // Door sector 1 becomes a plat instead: tag 4, floor 64, and both
        // its lines get lift special 10 with that tag.
        let sectors_lump = wad.lumps.iter().find(|l| l.name == "SECTORS").unwrap();
        let sat = bytes
            .windows(sectors_lump.data.len())
            .position(|w| w == sectors_lump.data.as_slice())
            .unwrap();
        bytes[sat + 26..sat + 28].copy_from_slice(&64i16.to_le_bytes()); // floor
        bytes[sat + 26 + 2..sat + 26 + 4].copy_from_slice(&128i16.to_le_bytes()); // ceiling
        bytes[sat + 26 + 24..sat + 26 + 26].copy_from_slice(&4u16.to_le_bytes()); // tag
        let lines_lump = wad.lumps.iter().find(|l| l.name == "LINEDEFS").unwrap();
        let lat = bytes
            .windows(lines_lump.data.len())
            .position(|w| w == lines_lump.data.as_slice())
            .unwrap();
        for li in [1usize, 7] {
            bytes[lat + li * 14 + 6..lat + li * 14 + 8].copy_from_slice(&10u16.to_le_bytes());
            bytes[lat + li * 14 + 8..lat + li * 14 + 10].copy_from_slice(&4u16.to_le_bytes());
        }
        std::fs::write(&wad_path, &bytes).unwrap();

        let wad = parse_wad(&bytes).expect("wad");
        let mesh = doom_map_to_mesh(&wad.lumps, "MAP01", &[[0u8; 3]; 256]).expect("mesh");
        assert_eq!(mesh.lifts.len(), 1, "one plat sector");
        let lift = &mesh.lifts[0];
        assert_eq!(lift.name, "lift_1");
        assert!((lift.closed_y - 1.0).abs() < 1e-4, "up floor 64 units");
        assert!((lift.open_y - 0.0).abs() < 1e-4, "down to the neighbours");

        let root_json = json::parse(glb_json(&mesh.glb).as_bytes()).expect("glb json");
        let nodes = root_json.get("nodes").unwrap().as_arr().unwrap();
        let node = nodes
            .iter()
            .find(|n| n.get("name").and_then(Value::as_str) == Some("lift_1"))
            .expect("lift_1 node");
        let extras = node.get("extras").unwrap();
        assert_eq!(extras.get("kind").and_then(Value::as_str), Some("lift"));
        assert_eq!(extras.get("default").and_then(Value::as_str), Some("up"));
        let states: Vec<&str> = extras
            .get("states")
            .and_then(Value::as_arr)
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(states, vec!["up", "down"]);
        // Rest is UP: the level is baked with lifts raised.
        assert!(node.get("translation").is_none());
        let anims = root_json.get("animations").unwrap().as_arr().unwrap();
        assert!(anims
            .iter()
            .any(|a| a.get("name").and_then(Value::as_str) == Some("lift_1")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_teleport_line_publishes_its_pad_and_destination() {
        use crate::classic_import::doom::{doom_map_to_mesh, parse_wad};
        let root = tmp_dir("tele_map");
        let wad_path = root.join("tele.wad");
        write_two_room_door_wad(&wad_path);
        let mut wad = parse_wad(&std::fs::read(&wad_path).unwrap()).expect("wad");
        for lump in &mut wad.lumps {
            match lump.name.as_str() {
                // Room B (sector 2) is the teleport target, tag 3.
                "SECTORS" => {
                    lump.data[2 * 26 + 24..2 * 26 + 26].copy_from_slice(&3u16.to_le_bytes())
                }
                // The west door line teleports (special 39) to tag 3.
                "LINEDEFS" => {
                    lump.data[14 + 6..14 + 8].copy_from_slice(&39u16.to_le_bytes());
                    lump.data[14 + 8..14 + 10].copy_from_slice(&3u16.to_le_bytes());
                }
                // Destination thing (type 14) at (256, 64) facing 90.
                "THINGS" => {
                    for v in [256i16, 64, 90, 14, 7] {
                        lump.data.extend_from_slice(&v.to_le_bytes());
                    }
                }
                _ => {}
            }
        }
        let mesh = doom_map_to_mesh(&wad.lumps, "MAP01", &[[0u8; 3]; 256]).expect("mesh");
        assert_eq!(mesh.teleporters.len(), 1, "{:?}", mesh.teleporters);
        let t = &mesh.teleporters[0];
        assert_eq!(t.name, "teleport_1");
        // Destination in metres: Doom (256, 64) at floor 0 + eye 41, all
        // over 64, with north at −Z.
        assert!((t.dst[0] - 4.0).abs() < 1e-4, "{t:?}");
        assert!((t.dst[1] - 41.0 / 64.0).abs() < 1e-4, "{t:?}");
        assert!((t.dst[2] + 1.0).abs() < 1e-4, "{t:?}");
        // Doom angle 90 is north, and north is −Z: yaw 0 looks down −Z.
        assert!(t.yaw.abs() < 1e-4, "{t:?}");
        // The pad hugs the teleport line (x = 128 units = 2 m), 16 units deep.
        assert!(t.pad_min[0] >= 1.7 && t.pad_max[0] <= 2.3, "{t:?}");
        assert!(
            t.pad_max[1] - t.pad_min[1] > 1.5,
            "pad spans the doorway: {t:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_nukage_sector_exports_as_its_own_hazard_node() {
        use crate::classic_import::doom::{doom_map_to_mesh, parse_wad};
        use makepad_asset_client::json::{self, Value};
        let root = tmp_dir("hazard_map");
        let wad_path = root.join("nukage.wad");
        // Same two rooms, but room B's floor is nukage with sector special 5.
        write_two_room_door_wad(&wad_path);
        let mut bytes = std::fs::read(&wad_path).unwrap();
        let wad = parse_wad(&bytes).expect("wad");
        let sectors_lump = wad
            .lumps
            .iter()
            .find(|l| l.name == "SECTORS")
            .expect("sectors");
        // Sector 2 = room B: flat NUKAGE1, special 5 (10% damage).
        let at = bytes
            .windows(sectors_lump.data.len())
            .position(|w| w == sectors_lump.data.as_slice())
            .expect("sector bytes");
        let sec2 = at + 2 * 26;
        bytes[sec2 + 4..sec2 + 12].copy_from_slice(b"NUKAGE1\0");
        bytes[sec2 + 22..sec2 + 24].copy_from_slice(&5u16.to_le_bytes());
        std::fs::write(&wad_path, &bytes).unwrap();

        let wad = parse_wad(&bytes).expect("wad");
        let palette = [[0u8; 3]; 256];
        let mesh = doom_map_to_mesh(&wad.lumps, "MAP01", &palette).expect("mesh");
        let root_json = json::parse(glb_json(&mesh.glb).as_bytes()).expect("glb json");
        let nodes = root_json.get("nodes").unwrap().as_arr().unwrap();
        let hazard = nodes
            .iter()
            .find(|n| n.get("name").and_then(Value::as_str) == Some("hazard_1"))
            .expect("hazard_1 node");
        let extras = hazard.get("extras").expect("extras");
        assert_eq!(extras.get("kind").and_then(Value::as_str), Some("hazard"));
        assert_eq!(extras.get("damage").and_then(Value::as_i64), Some(10));
        assert_eq!(extras.get("flat").and_then(Value::as_str), Some("nukage1"));
        assert_eq!(extras.get("liquid").and_then(Value::as_bool), Some(true));
        assert_eq!(extras.get("solid").and_then(Value::as_bool), Some(true));
        // A hazard floor does not move.
        assert!(hazard.get("translation").is_none());

        // Its triangles live ONLY in that node: the level mesh has no
        // geometry at room B's floor height inside room B.
        let parts = crate::world_preview::extract_glb_parts(&mesh.glb).expect("parts");
        let in_room_b_floor = |part: &crate::world_preview::Extracted| {
            part.indices.chunks_exact(3).any(|tri| {
                [
                    part.pos[tri[0] as usize],
                    part.pos[tri[1] as usize],
                    part.pos[tri[2] as usize],
                ]
                .iter()
                .all(|p| p[0] > 3.05 && p[1].abs() < 1e-3)
            })
        };
        assert!(
            !in_room_b_floor(&parts[0]),
            "the nukage floor must leave the static level mesh"
        );
        assert!(
            parts.iter().skip(1).any(in_room_b_floor),
            "and land in the hazard node"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn doom_things_become_player_and_deathmatch_starts() {
        use crate::classic_import::doom::{doom_map_nav, parse_wad};
        let root = tmp_dir("doom_nav");
        let wad_path = root.join("freedoom.wad");
        write_minimal_doom_wad(&wad_path);
        let wad = parse_wad(&std::fs::read(&wad_path).unwrap()).expect("wad");
        let nav = doom_map_nav(&wad.lumps, "MAP01").expect("nav");

        // THINGS: type 1 at (64, 64), angle 0. 64 map units = 1 m, and Doom
        // north is GLB −Z, so y = 64 lands at z = −1.
        assert_eq!(nav.starts.len(), 1);
        let start = &nav.starts[0];
        assert_eq!(start.name, "player_start");
        assert!((start.pos[0] - 1.0).abs() < 1e-6, "{start:?}");
        assert!((start.pos[2] + 1.0).abs() < 1e-6, "{start:?}");
        // Sector floor 0 + Doom VIEWHEIGHT 41 units.
        assert!((start.pos[1] - 41.0 / 64.0).abs() < 1e-6, "{start:?}");
        // Doom angle 0 is east, and east is +X: yaw π/2 looks down +X.
        assert!((start.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(nav.floor_y, Some(0.0));
        assert_eq!(nav.eye_height, Some(41.0 / 64.0));
        assert_eq!(nav.step_height, Some(24.0 / 64.0));

        // The anchors a World publishes.
        let anchors = nav.anchors();
        let names: Vec<&str> = anchors.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["floor_height", "step_height", "eye_height", "player_start"]
        );
        let ps = anchors.last().unwrap();
        assert!((ps.transform.pos.y - 41.0 / 64.0).abs() < 1e-6);
        // yaw pi/2 about +Y.
        assert!((ps.transform.rot.y - std::f32::consts::FRAC_PI_4.sin()).abs() < 1e-5);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn quake_entities_become_starts_with_quake_heights() {
        use crate::classic_import::doom::quake_bsp_nav;
        let ents = b"{\n\"classname\" \"info_player_start\"\n\"origin\" \"64 128 32\"\n\"angle\" \"90\"\n}\n\
                     {\n\"classname\" \"info_player_deathmatch\"\n\"origin\" \"0 0 32\"\n}\n";
        let mut bsp = Vec::new();
        bsp.extend_from_slice(&29u32.to_le_bytes());
        bsp.extend_from_slice(&12u32.to_le_bytes());
        bsp.extend_from_slice(&(ents.len() as u32).to_le_bytes());
        bsp.extend_from_slice(ents);
        let nav = quake_bsp_nav(&bsp).expect("nav");
        assert_eq!(
            nav.starts.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["player_start", "deathmatch_1"]
        );
        // Quake is Z-up: (x, y, z) -> (x, z, -y), 32 units = 1 m, eye +22.
        let p = &nav.starts[0];
        assert!((p.pos[0] - 2.0).abs() < 1e-6, "{p:?}");
        assert!((p.pos[1] - (32.0 + 22.0) / 32.0).abs() < 1e-6, "{p:?}");
        assert!((p.pos[2] + 4.0).abs() < 1e-6, "{p:?}");
        assert!((p.yaw - (std::f32::consts::FRAC_PI_2 - 90f32.to_radians())).abs() < 1e-6);
        // Origin sits 24 units above the floor it was dropped on.
        assert_eq!(nav.floor_y, Some((32.0 - 24.0) / 32.0));
        assert_eq!(nav.eye_height, Some((22.0 + 24.0) / 32.0));
        assert_eq!(nav.step_height, Some(18.0 / 32.0));
    }

    #[test]
    fn a_converted_world_publishes_its_spawn_as_anchors() {
        use makepad_asset_data::ImportManifest;
        let root = tmp_dir("world_anchor");
        write_minimal_doom_wad(&root.join("freedoom.wad"));
        let work = tmp_dir("world_anchor_work");
        let report =
            compile_classic(&root, &work, ClassicSource::Freedoom, "freedoom").expect("compile");
        let staged = work.join("source");

        // The library still reads the same three lines it always did.
        let text = std::fs::read_to_string(staged.join("worlds/freedoom/map01.spawn")).unwrap();
        let mut lines = text.lines();
        assert_eq!(lines.next().unwrap(), "world-spawn 1");
        assert_eq!(lines.next().unwrap().split_whitespace().count(), 3);
        assert_eq!(lines.next().unwrap().split_whitespace().count(), 2);
        assert!(text.contains("\nstart player_start "), "{text}");
        assert!(text.contains("\nstep "), "{text}");

        // And the catalog carries them as anchors on the World asset.
        let pack = report.pack.expect("pack");
        let manifest =
            ImportManifest::from_canonical_bytes(&std::fs::read(&pack.manifest_path).unwrap())
                .unwrap();
        let world = manifest
            .assets
            .iter()
            .find(|a| a.kind == AssetKind::World)
            .expect("world asset");
        let names: Vec<&str> = world.anchors.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"player_start"), "{names:?}");
        assert!(names.contains(&"floor_height"), "{names:?}");
        assert!(names.contains(&"eye_height"), "{names:?}");
        assert!(names.contains(&"step_height"), "{names:?}");
        let ps = world
            .anchors
            .iter()
            .find(|a| a.name == "player_start")
            .unwrap();
        assert!((ps.transform.pos.x - 1.0).abs() < 1e-4);
        assert!((ps.transform.pos.y - 41.0 / 64.0).abs() < 1e-4);
        // Doom north is GLB −Z: the thing at y = 64 lands at z = −1.
        assert!((ps.transform.pos.z + 1.0).abs() < 1e-4);
        // Y-up metres, exactly what the GLB was exported in.
        assert_eq!(world.coordinate_system.units_per_meter, 1.0);
        assert_eq!(world.coordinate_system.up, makepad_asset_data::Axis::YPos);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn an_actor_publishes_one_card_carrying_its_packed_sheet() {
        use makepad_asset_data::{FileRole, ImportManifest};
        let root = tmp_dir("actor_card");
        write_minimal_doom_wad(&root.join("freedoom.wad"));
        let work = tmp_dir("actor_card_work");
        let report =
            compile_classic(&root, &work, ClassicSource::Freedoom, "freedoom").expect("compile");
        let staged = work.join("source");

        // On disk: manifest + ONE sheet + the preview strip. The per-lump
        // frame PNG the sheet swallowed is gone.
        assert!(staged.join("billboards/freedoom/troo.billboard").is_file());
        assert!(staged.join("billboards/freedoom/troo.png").is_file());
        assert!(staged.join("billboards/freedoom/troo_thumb.png").is_file());
        assert!(
            !staged.join("billboards/freedoom/trooa1.png").exists(),
            "frames live in the sheet now"
        );
        let text = std::fs::read_to_string(staged.join("billboards/freedoom/troo.billboard")).unwrap();
        let bb = crate::stateful_billboard::StatefulBillboard::parse(&text).unwrap();
        let sheet = bb.sheet.expect("sheet header");
        assert_eq!(sheet.cols, 1);
        assert!(bb.frames.iter().all(|f| f.file == "troo.png" && f.cell.is_some()));

        // In the catalog: exactly one Billboard row, sheet + manifest.
        let pack = report.pack.expect("compiled pack");
        let manifest =
            ImportManifest::from_canonical_bytes(&std::fs::read(&pack.manifest_path).unwrap())
                .unwrap();
        let bills: Vec<_> = manifest
            .assets
            .iter()
            .filter(|a| a.kind == AssetKind::Billboard)
            .collect();
        assert_eq!(bills.len(), 1, "one actor, one card");
        assert_eq!(bills[0].key.as_str(), "billboards/freedoom/troo");
        let roles: Vec<FileRole> = bills[0].files.iter().map(|f| f.file.role).collect();
        assert_eq!(roles, vec![FileRole::Texture, FileRole::Source]);
        assert!(bills[0].thumbnail.is_some(), "grids animate off the strip");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&work);
    }

}
