//! Classic pack convert: walk a local folder, dispatch per-game converters.
//!
//! Shared types/PNG live in [`shared`]. Doom WAD/maps in [`doom`], Quake 1
//! BSP/MDL/SPR in [`quake`]. Duke / Quake II / Quake III / id Tech 4 stay
//! in their own crate modules and are called from here.

mod shared;
mod doom;
mod quake;

pub use shared::*;

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
        bank.images.extend(
            crate::quake3_import::load_tex_bank(pak_scratch.join("pk3").as_path()).images,
        );
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
            ClassicFileKind::Md3 if crate::quake3_import::is_lod_md3(&f.rel) => {
                convert_done += 1;
            }
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
        if !assembled.is_empty() {
            assets.retain(|a| !drop.contains(&a.key));
            assets.extend(assembled);
        }
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

    // AO bake is parked: a large id Tech 4 pack takes hours and hides convert
    // bugs. Sidecars remain optional; re-enable bake_glb_tree here later.
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
        let Some(bb) = assemble(prefix, &lumps) else {
            continue;
        };
        let rel_path = format!("{group}.billboard");
        let dest = staged.join(&rel_path);
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&dest, bb.to_text()).is_err() {
            continue;
        }
        let icon_rel = bb.preview_frames().first().map(|f| {
            if let Some(parent) = dest.parent() {
                parent.join(&f.file)
            } else {
                PathBuf::from(&f.file)
            }
        });
        let icon_rel = icon_rel.and_then(|p| {
            p.strip_prefix(staged)
                .ok()
                .map(|r| r.to_string_lossy().replace('\\', "/"))
        });
        let mut tags = tags_base;
        tags.push("stateful".into());
        tags.push(bb.role.as_str().into());
        extra.push(ClassicAsset {
            key: group,
            kind: AssetKind::Billboard,
            rel_path,
            tags,
            icon_rel,
        });
        consumed.extend(idxs);
    }

    for (group, idxs) in spr_groups {
        let mut frames = Vec::new();
        let mut tags_base = Vec::new();
        for &i in &idxs {
            let rel = assets[i].rel_path.replace('\\', "/");
            let file = rel.rsplit('/').next().unwrap_or("").to_string();
            let path = staged.join(&assets[i].rel_path);
            let (w, h) = png_dims(&path).unwrap_or((1, 1));
            frames.push(SpriteFrame {
                letter: 'A',
                rot: 1,
                w,
                h,
                file,
                flip: false,
            });
            if tags_base.is_empty() {
                tags_base = assets[i].tags.clone();
            }
        }
        if frames.is_empty() {
            continue;
        }
        let slug = group.rsplit('/').next().unwrap_or("sprite");
        let bb = sequential_idle(slug, frames, SpriteRole::Effect);
        let rel_path = format!("{group}.billboard");
        let dest = staged.join(&rel_path);
        if std::fs::write(&dest, bb.to_text()).is_err() {
            continue;
        }
        let mut tags = tags_base;
        tags.push("stateful".into());
        extra.push(ClassicAsset {
            key: group,
            kind: AssetKind::Billboard,
            rel_path,
            tags,
            icon_rel: None,
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
    let tag = if is_music { "music" } else { "sfx" };
    Ok(ClassicAsset {
        key,
        kind: AssetKind::Audio,
        rel_path,
        tags: tags_for(AssetKind::Audio, &[source.id(), tag, &slug]),
        icon_rel: None,
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
        // Just right of the shared wall (world x = 80/64).
        let probe = [80.0 / 64.0 + 0.05, 0.0, 32.0 / 64.0];
        let mut hits = 0u32;
        for tri in idx.chunks_exact(3) {
            let a = pos[tri[0] as usize];
            let b = pos[tri[1] as usize];
            let c = pos[tri[2] as usize];
            if point_in_tri([probe[0], probe[2]], [a[0], a[2]], [b[0], b[2]], [c[0], c[2]]) {
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
        let hole = [128.0 / 64.0, 128.0 / 64.0];
        let ring = [32.0 / 64.0, 32.0 / 64.0];
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
        let walk = [16.0 / 64.0, 96.0 / 64.0];
        let island = [96.0 / 64.0, 96.0 / 64.0];
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
        let centre = [32.0 / 64.0, 32.0 / 64.0];
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
        let probe = [inside[0] / 64.0, inside[1] / 64.0];
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
        let emit = |pos: &mut _, uvs: &mut _, idx: &mut _, side: u16| {
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
                &uv_map,
                1.0 / 64.0,
            );
        };
        emit(&mut pos, &mut uvs, &mut idx, 0);
        let after_front = idx.len();
        assert!(after_front >= 6, "front mid must emit a quad");
        emit(&mut pos, &mut uvs, &mut idx, 1);
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
        let inside = [40.0 / 64.0, 40.0 / 64.0];
        let near_cut = [70.0 / 64.0, 20.0 / 64.0];
        let outside = [90.0 / 64.0, 90.0 / 64.0];
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
        let notch = [96.0 / 64.0, 96.0 / 64.0];
        let body = [32.0 / 64.0, 32.0 / 64.0];
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

}
