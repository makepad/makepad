//! ONE packed sprite sheet per stateful-billboard actor.
//!
//! A Doom actor is ~40 lumps and a Duke actor ~30 tiles. Writing one PNG per
//! frame floods every consumer downstream: the catalog got 483 `bossa2a8`
//! cards, the library 1,599 loose frame files. So the classic importers pack
//! every distinct frame of an actor into one uniform-cell sheet, rewrite the
//! manifest to point at it (`sheet <cols> <cell_w> <cell_h>` + a trailing
//! `cell <n>` per frame) and delete the per-frame PNGs they replaced.
//!
//! Layout (the contract `apps/vj/src/billboard.rs::cut_cell` reads back):
//! - `cols` cells per row, cell `cell_w`×`cell_h` = the largest frame,
//! - cell index is row-major (`x = cell % cols`, `y = cell / cols`),
//! - each frame sits TOP-LEFT inside its cell at its authored `w`×`h`,
//! - frames that share a source PNG (Doom `A2A8` mirror pairs) share a cell.
//!
//! Beside the sheet goes `<stem>_thumb.png`: the 128²-tile animated strip
//! the library grid and the catalog thumbnail play (1024 wide, ≥256 tall so
//! it satisfies the published-thumbnail contract).

use crate::anim_icon;
use crate::classic_import::{decode_png_stored, encode_png_rgba};
use crate::stateful_billboard::{SheetLayout, StatefulBillboard};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Cells per row, unless the cells are too wide for [`MAX_SHEET_DIM`].
pub const SHEET_COLS: u32 = 8;
/// Neither sheet axis may exceed this (well under `MAX_TEXTURE_DIM`).
pub const MAX_SHEET_DIM: u32 = 8192;
/// Preview strip tiles (one row of the 1024-wide anim sheet).
pub const MAX_THUMB_TILES: usize = 8;

/// Suffix of the animated preview strip written beside `<stem>.billboard`.
pub const THUMB_SUFFIX: &str = "_thumb";

/// What [`write_with_sheet`] put on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WrittenBillboard {
    pub manifest: PathBuf,
    /// The one packed sheet; every frame of the manifest reads from it.
    pub sheet: PathBuf,
    /// Animated 128²-tile strip (`None` when the preview had no frames).
    pub thumb: Option<PathBuf>,
    /// Per-frame PNGs the sheet replaced, in canonical order.
    pub consumed: Vec<PathBuf>,
}

/// Sheet name for a manifest path (`troo.billboard` → `troo.png`).
pub fn sheet_name(manifest: &Path) -> Option<String> {
    let stem = manifest.file_stem()?.to_str()?;
    Some(format!("{stem}.png"))
}

/// Preview-strip name for a manifest path (`troo.billboard` → `troo_thumb.png`).
pub fn thumb_name(manifest: &Path) -> Option<String> {
    let stem = manifest.file_stem()?.to_str()?;
    Some(format!("{stem}{THUMB_SUFFIX}.png"))
}

/// Pack `bb`'s frames into one sheet beside `manifest`, rewrite `bb` to read
/// from it, then write the manifest text. The per-frame PNGs are left on
/// disk; the caller deletes the ones it owns (see [`WrittenBillboard`]).
pub fn write_with_sheet(
    manifest: &Path,
    bb: &mut StatefulBillboard,
) -> Result<WrittenBillboard, String> {
    let dir = manifest.parent().unwrap_or_else(|| Path::new("."));
    let sheet_file = sheet_name(manifest).ok_or("billboard manifest has no stem")?;
    let packed = pack_frames(bb, dir, &sheet_file)?;

    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let sheet_path = dir.join(&sheet_file);
    std::fs::write(&sheet_path, &packed.png).map_err(|e| e.to_string())?;

    // The manifest must describe the sheet before the strip is cut, so the
    // strip reads exactly the pixels a consumer would.
    bb.sheet = Some(packed.layout);
    for (frame, cell) in bb.frames.iter_mut().zip(packed.cell_of_frame.iter()) {
        frame.file = sheet_file.clone();
        frame.cell = Some(*cell);
    }
    for (frame, (w, h)) in bb.frames.iter_mut().zip(packed.size_of_frame.iter()) {
        frame.w = *w;
        frame.h = *h;
    }

    let thumb = write_thumb_strip(manifest, bb, &packed);
    std::fs::write(manifest, bb.to_text()).map_err(|e| e.to_string())?;
    Ok(WrittenBillboard {
        manifest: manifest.to_path_buf(),
        sheet: sheet_path,
        thumb,
        consumed: packed.consumed,
    })
}

struct Packed {
    layout: SheetLayout,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    png: Vec<u8>,
    /// Cell index per frame (parallel to `bb.frames`).
    cell_of_frame: Vec<u32>,
    /// Decoded size per frame (the PNG is authoritative over the manifest).
    size_of_frame: Vec<(u32, u32)>,
    consumed: Vec<PathBuf>,
}

fn pack_frames(bb: &StatefulBillboard, dir: &Path, sheet_file: &str) -> Result<Packed, String> {
    if bb.frames.is_empty() {
        return Err("billboard has no frames".into());
    }
    // First-seen order, so the cell numbering is a function of the manifest
    // alone (no filesystem or hash-map iteration order).
    let mut cell_of_file: BTreeMap<String, u32> = BTreeMap::new();
    let mut cells: Vec<(Vec<u8>, u32, u32)> = Vec::new();
    let mut consumed: Vec<PathBuf> = Vec::new();
    let mut cell_of_frame = Vec::with_capacity(bb.frames.len());
    let mut size_of_frame = Vec::with_capacity(bb.frames.len());
    for frame in &bb.frames {
        if frame.file.is_empty() {
            return Err("frame without a file cannot be packed".into());
        }
        if frame.file == sheet_file {
            return Err(format!("frame already points at the sheet {sheet_file}"));
        }
        if let Some(&cell) = cell_of_file.get(&frame.file) {
            cell_of_frame.push(cell);
            let (_, w, h) = &cells[cell as usize];
            size_of_frame.push((*w, *h));
            continue;
        }
        let path = dir.join(&frame.file);
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let (rgba, w, h) =
            decode_png_stored(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        if w == 0 || h == 0 {
            return Err(format!("{}: empty frame", path.display()));
        }
        let cell = cells.len() as u32;
        cell_of_file.insert(frame.file.clone(), cell);
        cells.push((rgba, w, h));
        consumed.push(path);
        cell_of_frame.push(cell);
        size_of_frame.push((w, h));
    }

    let cell_w = cells.iter().map(|(_, w, _)| *w).max().unwrap_or(1).max(1);
    let cell_h = cells.iter().map(|(_, _, h)| *h).max().unwrap_or(1).max(1);
    let count = cells.len() as u32;
    let cols = sheet_cols(count, cell_w)?;
    let rows = count.div_ceil(cols);
    let width = cols * cell_w;
    let height = rows.checked_mul(cell_h).ok_or("sheet height overflow")?;
    if height > MAX_SHEET_DIM {
        return Err(format!("sheet {width}x{height} exceeds {MAX_SHEET_DIM}"));
    }
    let layout = SheetLayout {
        cols,
        cell_w,
        cell_h,
    };

    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    for (i, (src, w, h)) in cells.iter().enumerate() {
        let (ox, oy) = layout.cell_origin(i as u32);
        blit(&mut rgba, width, ox, oy, src, *w, *h);
    }
    let png = encode_png_rgba(&rgba, width, height)?;
    Ok(Packed {
        layout,
        width,
        height,
        rgba,
        png,
        cell_of_frame,
        size_of_frame,
        consumed,
    })
}

/// Cells per row: [`SHEET_COLS`], narrowed so the sheet stays inside
/// [`MAX_SHEET_DIM`] even for very wide frames.
fn sheet_cols(cells: u32, cell_w: u32) -> Result<u32, String> {
    if cell_w > MAX_SHEET_DIM {
        return Err(format!("frame width {cell_w} exceeds {MAX_SHEET_DIM}"));
    }
    let fit = (MAX_SHEET_DIM / cell_w).max(1);
    Ok(SHEET_COLS.min(cells.max(1)).min(fit).max(1))
}

fn blit(dst: &mut [u8], dst_w: u32, ox: u32, oy: u32, src: &[u8], w: u32, h: u32) {
    for y in 0..h as usize {
        let s = y * w as usize * 4;
        let d = ((oy as usize + y) * dst_w as usize + ox as usize) * 4;
        let n = w as usize * 4;
        if s + n > src.len() || d + n > dst.len() {
            return;
        }
        dst[d..d + n].copy_from_slice(&src[s..s + n]);
    }
}

/// The animated strip the library grid and the catalog thumbnail play: the
/// preview state's front frames as 128² tiles on a 1024-wide sheet. Padded
/// to two rows so it clears the 256px published-thumbnail floor.
fn write_thumb_strip(
    manifest: &Path,
    bb: &StatefulBillboard,
    packed: &Packed,
) -> Option<PathBuf> {
    let dir = manifest.parent().unwrap_or_else(|| Path::new("."));
    let name = thumb_name(manifest)?;
    let mut tiles = Vec::new();
    for frame in bb.preview_frames().into_iter().take(MAX_THUMB_TILES) {
        // SKIP a frame we cannot cut, never abandon the strip. A pack is
        // allowed one malformed frame — a zero-size cell, or artwork wider
        // than the sheet's cell — and giving up on the whole thumbnail for
        // it published a BLANK library tile that no re-import could ever
        // repair. The frames that did cut are a perfectly good preview.
        let Some((x, y, w, h)) = bb.frame_rect(frame) else { continue };
        let Some(cut) = cut_rect(&packed.rgba, packed.width, packed.height, x, y, w, h)
        else {
            continue;
        };
        tiles.push(anim_icon::fit_tile(&cut, w as usize, h as usize));
    }
    if tiles.is_empty() {
        return None;
    }
    // The REAL frame count, before the padding below. This is the number the
    // strip declares and a consumer plays; everything after it is clear
    // tiles bought for height, not animation.
    let frames = tiles.len() as u32;
    let fps = bb.preview_fps() as f32;
    // `pack_sheet` sizes the sheet from the tile count; one extra clear tile
    // buys the second row that ThumbnailMeta::validate demands (>=256px).
    while tiles.len() <= anim_icon::SHEET_W / anim_icon::TILE {
        tiles.push(anim_icon::fit_tile(&[], 0, 0));
    }
    let sheet = anim_icon::pack_sheet(&tiles).ok()?;
    // Stamp the actor's OWN frame count and rate over the packer's default:
    // the manifest knows how many frames it drew and how fast they run, and
    // that is what the catalog should say rather than "however many cells
    // happen to look painted".
    let png = anim_icon::stamp_layout(
        &sheet.png,
        makepad_asset_data::ThumbnailCells { count: frames, ..sheet.cells() },
        fps,
    );
    let path = dir.join(name);
    std::fs::write(&path, png).ok()?;
    Some(path)
}

/// The still a SINGLE-frame sprite shows in a grid: its one tile fitted into
/// the 128² cell grid and padded to the two rows a published thumbnail's
/// 256px floor demands, as PNG bytes.
///
/// A classic pack is mostly single tiles — a can, a bottle, a chair — and
/// they are not stateful billboards, so they never reached the strip above.
/// They published with no thumbnail at all and drew as blank library tiles
/// that no re-import could fill, because the raw 46×49 tile beside them is
/// far under the floor. This is that tile, in the shape the contract wants.
pub fn still_icon_png(src_png: &[u8]) -> Option<Vec<u8>> {
    let (rgba, w, h) = decode_png_stored(src_png).ok()?;
    if w == 0 || h == 0 {
        return None;
    }
    let mut tiles = vec![anim_icon::fit_tile(&rgba, w as usize, h as usize)];
    while tiles.len() <= anim_icon::SHEET_W / anim_icon::TILE {
        tiles.push(anim_icon::fit_tile(&[], 0, 0));
    }
    let sheet = anim_icon::pack_sheet(&tiles).ok()?;
    // One real cell: a consumer that plays the strip shows the still and
    // stops, rather than cycling through the clear padding.
    Some(anim_icon::stamp_layout(
        &sheet.png,
        makepad_asset_data::ThumbnailCells { count: 1, ..sheet.cells() },
        1.0,
    ))
}

fn cut_rect(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Option<Vec<u8>> {
    if w == 0 || h == 0 || x + w > src_w || y + h > src_h {
        return None;
    }
    let mut out = vec![0u8; (w as usize) * (h as usize) * 4];
    for row in 0..h as usize {
        let s = ((y as usize + row) * src_w as usize + x as usize) * 4;
        let d = row * w as usize * 4;
        let n = w as usize * 4;
        if s + n > src.len() {
            return None;
        }
        out[d..d + n].copy_from_slice(&src[s..s + n]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stateful_billboard::{AnimState, SpriteFrame, SpriteRole};

    fn solid_png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        encode_png_rgba(&rgba, w, h).unwrap()
    }

    /// A single tile — a can, a bottle, a chair — is most of a classic pack,
    /// and it published NO thumbnail because the raw 46x49 artwork is far
    /// under the 256px floor a published thumbnail must clear. The still it
    /// gets now is that same artwork, in the shape the contract wants.
    #[test]
    fn a_single_tile_still_clears_the_published_thumbnail_floor() {
        let png = solid_png(46, 49, [200, 40, 40]);
        let icon = still_icon_png(&png).expect("still icon");
        let (w, h) = crate::thumbs::png_dims(&icon).expect("dims");
        assert!(
            w >= makepad_asset_data::limits::THUMBNAIL_MIN_DIM
                && h >= makepad_asset_data::limits::THUMBNAIL_MIN_DIM,
            "{w}x{h} must clear the floor"
        );
        // One REAL cell: a consumer plays the still and stops rather than
        // cycling the clear padding that bought the height.
        let (cells, _fps) = anim_icon::read_layout(&icon).expect("stamped layout");
        assert_eq!(cells.count, 1, "one real frame");
    }

    fn frame(letter: char, rot: u8, w: u32, h: u32, file: &str, flip: bool) -> SpriteFrame {
        SpriteFrame {
            letter,
            rot,
            w,
            h,
            file: file.into(),
            flip,
            cell: None,
        }
    }

    fn actor(frames: Vec<SpriteFrame>) -> StatefulBillboard {
        let n = frames.len();
        StatefulBillboard {
            prefix: "troo".into(),
            role: SpriteRole::Character,
            preview: "walk".into(),
            facings: 8,
            mirrors: 8,
            states: vec![AnimState {
                name: "walk".into(),
                first: 0,
                last: n,
                r#loop: true,
                fps: 8,
            }],
            frames,
            sheet: None,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mp-bbsheet-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn packs_uniform_cells_row_major_top_left() {
        let dir = scratch("geometry");
        for (name, w, h, rgb) in [
            ("a.png", 4u32, 6u32, [255u8, 0, 0]),
            ("b.png", 8, 3, [0, 255, 0]),
            ("c.png", 2, 2, [0, 0, 255]),
        ] {
            std::fs::write(dir.join(name), solid_png(w, h, rgb)).unwrap();
        }
        let mut bb = actor(vec![
            frame('A', 1, 4, 6, "a.png", false),
            frame('B', 1, 8, 3, "b.png", false),
            frame('C', 1, 2, 2, "c.png", false),
        ]);
        let manifest = dir.join("troo.billboard");
        let written = write_with_sheet(&manifest, &mut bb).unwrap();
        let layout = bb.sheet.expect("sheet header");
        // Cell = the largest frame, 3 cells → 3 columns.
        assert_eq!((layout.cols, layout.cell_w, layout.cell_h), (3, 8, 6));
        assert_eq!(
            bb.frames.iter().map(|f| f.cell).collect::<Vec<_>>(),
            vec![Some(0), Some(1), Some(2)]
        );
        assert!(bb.frames.iter().all(|f| f.file == "troo.png"));
        assert_eq!(written.consumed.len(), 3);

        let sheet = std::fs::read(&written.sheet).unwrap();
        let (rgba, w, h) = decode_png_stored(&sheet).unwrap();
        assert_eq!((w, h), (24, 6));
        let px = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
        };
        // Top-left anchored inside each cell, transparent elsewhere.
        assert_eq!(px(0, 0), [255, 0, 0, 255]);
        assert_eq!(px(3, 5), [255, 0, 0, 255]);
        assert_eq!(px(4, 0), [0, 0, 0, 0], "cell 0 pads right of a 4px frame");
        assert_eq!(px(8, 0), [0, 255, 0, 255]);
        assert_eq!(px(8, 3), [0, 0, 0, 0], "cell 1 pads below a 3px frame");
        assert_eq!(px(16, 1), [0, 0, 255, 255]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mirror_pairs_share_one_cell() {
        let dir = scratch("mirror");
        std::fs::write(dir.join("trooa2a8.png"), solid_png(4, 4, [1, 2, 3])).unwrap();
        std::fs::write(dir.join("trooa1.png"), solid_png(4, 4, [4, 5, 6])).unwrap();
        let mut bb = actor(vec![
            frame('A', 1, 4, 4, "trooa1.png", false),
            frame('A', 2, 4, 4, "trooa2a8.png", false),
            frame('A', 8, 4, 4, "trooa2a8.png", true),
        ]);
        let manifest = dir.join("troo.billboard");
        let written = write_with_sheet(&manifest, &mut bb).unwrap();
        assert_eq!(
            bb.frames.iter().map(|f| f.cell).collect::<Vec<_>>(),
            vec![Some(0), Some(1), Some(1)],
            "the same PNG is packed once"
        );
        assert!(bb.frames[2].flip, "flip survives the rewrite");
        assert_eq!(written.consumed.len(), 2);
        assert_eq!(bb.sheet.unwrap().cols, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifest_round_trips_through_the_parser() {
        let dir = scratch("roundtrip");
        for (name, rgb) in [("a.png", [9u8, 9, 9]), ("b.png", [8, 8, 8])] {
            std::fs::write(dir.join(name), solid_png(5, 7, rgb)).unwrap();
        }
        let mut bb = actor(vec![
            frame('A', 1, 5, 7, "a.png", false),
            frame('B', 1, 5, 7, "b.png", true),
        ]);
        let manifest = dir.join("troo.billboard");
        write_with_sheet(&manifest, &mut bb).unwrap();
        let text = std::fs::read_to_string(&manifest).unwrap();
        assert!(text.contains("\nsheet 2 5 7\n"), "{text}");
        assert!(text.contains("frame 0 A 1 5 7 troo.png cell 0\n"), "{text}");
        assert!(text.contains("frame 1 B 1 5 7 troo.png flip cell 1\n"), "{text}");
        let again = StatefulBillboard::parse(&text).unwrap();
        assert_eq!(again.sheet, bb.sheet);
        assert_eq!(again.frames, bb.frames);
        assert_eq!(again.frame_rect(&again.frames[1]), Some((5, 0, 5, 7)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview_strip_is_a_1024_wide_two_row_anim_sheet() {
        let dir = scratch("thumb");
        for (i, rgb) in [[200u8, 0, 0], [0, 200, 0], [0, 0, 200]].into_iter().enumerate() {
            std::fs::write(dir.join(format!("f{i}.png")), solid_png(12, 16, rgb)).unwrap();
        }
        let mut bb = actor(vec![
            frame('A', 1, 12, 16, "f0.png", false),
            frame('B', 1, 12, 16, "f1.png", false),
            frame('C', 1, 12, 16, "f2.png", false),
        ]);
        let manifest = dir.join("troo.billboard");
        let written = write_with_sheet(&manifest, &mut bb).unwrap();
        let thumb = written.thumb.expect("preview strip");
        assert!(thumb.ends_with("troo_thumb.png"));
        let (_, w, h) = decode_png_stored(&std::fs::read(&thumb).unwrap()).unwrap();
        assert_eq!(w as usize, anim_icon::SHEET_W);
        assert_eq!(h, 256, "two rows clear the 256px thumbnail floor");
        // The strip declares its own layout: EIGHT cells wide by two rows,
        // but only the three real frames, at the actor's own rate — the
        // padding that bought the 256px floor is not animation.
        let (cells, fps) = anim_icon::read_layout(&std::fs::read(&thumb).unwrap())
            .expect("the strip says what it is");
        assert_eq!(cells.cols, anim_icon::SHEET_W as u32 / anim_icon::TILE as u32);
        assert_eq!((cells.cell_w, cells.cell_h), (128, 128));
        assert_eq!(cells.count, 3, "three frames, not nine padded cells");
        assert_eq!(fps, bb.preview_fps() as f32);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Doom-sized actor: 40 lumps of 60x70, sprite-shaped (a blob of a few
    /// palette colours on transparent). Guards the compressed PNG writer —
    /// stored deflate made the 1024x256 preview strip alone a 1 MB file.
    #[test]
    fn an_actor_sheet_and_strip_stay_small() {
        let dir = scratch("size");
        let (fw, fh) = (60u32, 70u32);
        for i in 0..40u32 {
            let mut rgba = vec![0u8; (fw * fh * 4) as usize];
            for y in 0..fh {
                for x in 0..fw {
                    let (dx, dy) = (x as f32 - 30.0, y as f32 - 40.0);
                    if dx * dx / 400.0 + dy * dy / 900.0 > 1.0 {
                        continue;
                    }
                    let p = ((y * fw + x) * 4) as usize;
                    let shade = ((x + y + i * 3) % 5) as u8 * 30;
                    rgba[p..p + 4].copy_from_slice(&[90 + shade, 40 + shade / 2, 30, 255]);
                }
            }
            std::fs::write(
                dir.join(format!("f{i:02}.png")),
                encode_png_rgba(&rgba, fw, fh).unwrap(),
            )
            .unwrap();
        }
        let frames: Vec<SpriteFrame> = (0..40u32)
            .map(|i| {
                frame(
                    (b'A' + (i / 5) as u8) as char,
                    (i % 5 + 1) as u8,
                    fw,
                    fh,
                    &format!("f{i:02}.png"),
                    false,
                )
            })
            .collect();
        let mut bb = actor(frames);
        let manifest = dir.join("troo.billboard");
        let written = write_with_sheet(&manifest, &mut bb).unwrap();
        let sheet = std::fs::metadata(&written.sheet).unwrap().len();
        let thumb = std::fs::metadata(written.thumb.as_ref().unwrap()).unwrap().len();
        let sheet_raw = u64::from(bb.sheet.unwrap().cols * fw)
            * u64::from(bb.sheet.unwrap().cell_h * 5)
            * 4;
        println!("sheet {sheet} bytes (raw {sheet_raw}), strip {thumb} bytes (raw 1048576)");
        assert!(
            thumb < 120 * 1024,
            "preview strip must be compressed, got {thumb} bytes"
        );
        assert!(
            sheet < sheet_raw / 4,
            "sheet must be compressed, got {sheet} of {sheet_raw} raw"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wide_frames_narrow_the_columns_instead_of_overflowing() {
        assert_eq!(sheet_cols(40, 60).unwrap(), SHEET_COLS);
        assert_eq!(sheet_cols(3, 60).unwrap(), 3);
        assert_eq!(sheet_cols(40, 3000).unwrap(), 2);
        assert_eq!(sheet_cols(40, 8192).unwrap(), 1);
        assert!(sheet_cols(2, 9000).is_err());
    }
}
