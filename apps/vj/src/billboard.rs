//! Stateful billboard playback — same `stateful-billboard` text format the
//! game importer / asset-ui viewer uses. Frames play like a video clip;
//! named states (walk, attack, pain, …) are switched from the UI.
//!
//! Library tiles and the mixer both play the front-facing (rot 0/1) frames
//! at each frame's authored size — never a contact sheet of every rotation.
//!
//! Two sources feed the same [`PreparedBillboard`]:
//! - a local `.billboard` manifest with one PNG per frame beside it
//!   ([`prepare`]), and
//! - a catalog `Billboard` asset, which publishes ONE packed sprite sheet
//!   (role `Texture`) plus this manifest text (role `Source`). The manifest
//!   then carries a `sheet <cols> <cell_w> <cell_h>` header and a trailing
//!   `cell <n>` on every frame line, so [`prepare_from_sheet`] can cut the
//!   frames back out at their authored sizes without a per-frame download.

use crate::media::key_sprite_alpha;
use std::path::{Path, PathBuf};

pub const MAGIC: &str = "stateful-billboard";

/// Uniform-cell layout of a packed sprite sheet: `cols` cells per row, every
/// cell `cell_w`×`cell_h`, frames top-left anchored inside their cell and
/// laid out row-major by cell index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SheetLayout {
    pub cols: usize,
    pub cell_w: usize,
    pub cell_h: usize,
}

#[derive(Clone, Debug)]
pub struct SpriteFrame {
    pub letter: char,
    pub rot: u8,
    pub w: u32,
    pub h: u32,
    pub file: String,
    /// Cell index in the packed sheet, when the manifest was published with
    /// one (grouped catalog actors).
    pub cell: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct AnimState {
    pub name: String,
    pub first: usize,
    pub last: usize,
    pub r#loop: bool,
    pub fps: u8,
}

#[derive(Clone, Debug)]
pub struct Manifest {
    pub preview: String,
    pub states: Vec<AnimState>,
    pub frames: Vec<SpriteFrame>,
    /// Present when the frames live in one packed sheet.
    pub sheet: Option<SheetLayout>,
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut lines = text.lines();
        let header = lines.next().unwrap_or("").trim();
        if !header.starts_with(MAGIC) {
            return Err("not a stateful-billboard".into());
        }
        let mut preview = String::new();
        let mut states = Vec::new();
        let mut frames = Vec::new();
        let mut sheet = None;
        for line in lines {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_whitespace();
            match parts.next() {
                Some("preview") => preview = parts.next().unwrap_or("").to_string(),
                Some("sheet") => {
                    let mut num = || parts.next().and_then(|s| s.parse::<usize>().ok());
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
                        states.push(AnimState { name, first, last, r#loop: lp, fps });
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
                    // Trailing tokens after the file are unordered flags:
                    // `flip` and `cell <n>`. A sheet-only manifest may omit
                    // the per-frame file entirely, so `cell` in the file
                    // position is read as the first flag, not a filename.
                    let mut file = parts.next().unwrap_or("").to_string();
                    let mut rest: Vec<&str> = parts.collect();
                    if file == "cell" {
                        rest.insert(0, "cell");
                        file.clear();
                    }
                    let mut cell = None;
                    for (i, token) in rest.iter().enumerate() {
                        if *token == "cell" {
                            cell = rest.get(i + 1).and_then(|s| s.parse::<usize>().ok());
                        }
                    }
                    if !file.is_empty() || cell.is_some() {
                        frames.push(SpriteFrame { letter, rot, w, h, file, cell });
                    }
                }
                _ => {}
            }
        }
        if frames.is_empty() {
            return Err("billboard has no frames".into());
        }
        if preview.is_empty() {
            preview = states
                .first()
                .map(|s| s.name.clone())
                .unwrap_or_else(|| "idle".into());
        }
        Ok(Self { preview, states, frames, sheet })
    }

    fn state_range(&self, name: &str) -> std::ops::Range<usize> {
        self.states
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.first..s.last)
            .unwrap_or(0..self.frames.len())
    }

    fn indices_in(&self, range: std::ops::Range<usize>) -> Vec<usize> {
        range.filter(|i| *i < self.frames.len()).collect()
    }

    fn is_front(&self, index: usize) -> bool {
        self.frames[index].rot <= 1
    }

    /// Frame INDICES of the preview state. Index-based because a packed-sheet
    /// manifest's frames can share (or omit) a file name — identity is the
    /// position in `frames`, never the file string.
    pub fn preview_indices(&self) -> Vec<usize> {
        let src = self.indices_in(self.state_range(&self.preview));
        let front: Vec<usize> = src.iter().copied().filter(|i| self.is_front(*i)).collect();
        if front.len() >= 2 {
            return front;
        }
        if src.len() >= 2 {
            return src;
        }
        if self.frames.is_empty() {
            Vec::new()
        } else {
            vec![0]
        }
    }

    /// Front-facing frame indices of `name`, else every frame in its range.
    pub fn indices_for_state(&self, name: &str) -> Vec<usize> {
        let src = self.indices_in(self.state_range(name));
        if src.is_empty() {
            return self.preview_indices();
        }
        let front: Vec<usize> = src.iter().copied().filter(|i| self.is_front(*i)).collect();
        if !front.is_empty() {
            return front;
        }
        src
    }

    /// Front-facing (rot 1 or 0) frames of the preview state — same set the
    /// asset-ui library card cycles. Native sizes, not a rotation sheet.
    pub fn preview_frames(&self) -> Vec<&SpriteFrame> {
        self.preview_indices().into_iter().map(|i| &self.frames[i]).collect()
    }

    pub fn preview_fps(&self) -> f32 {
        self.states
            .iter()
            .find(|s| s.name == self.preview)
            .map(|s| s.fps)
            .unwrap_or(8)
            .clamp(1, 30) as f32
    }

    /// Front-facing frames of `name`, else every frame in that state's range.
    pub fn frames_for_state(&self, name: &str) -> Vec<&SpriteFrame> {
        self.indices_for_state(name).into_iter().map(|i| &self.frames[i]).collect()
    }

    /// State names worth playing, in manifest order (poses are not clips).
    fn playable_states(&self) -> Vec<String> {
        let names: Vec<String> = if self.states.is_empty() {
            vec![self.preview.clone()]
        } else {
            self.states.iter().map(|s| s.name.clone()).collect()
        };
        names.into_iter().filter(|n| !n.starts_with("pose_")).collect()
    }

    fn state_meta(&self, name: &str) -> (f32, bool) {
        let meta = self.states.iter().find(|s| s.name == name);
        (
            meta.map(|s| s.fps).unwrap_or(8).clamp(1, 30) as f32,
            meta.map(|s| s.r#loop).unwrap_or(true),
        )
    }
}

/// Top-left pixel of `cell` in a packed sheet.
pub fn cell_origin(layout: SheetLayout, cell: usize) -> (usize, usize) {
    let cols = layout.cols.max(1);
    ((cell % cols) * layout.cell_w, (cell / cols) * layout.cell_h)
}

/// Copy one frame out of a packed sheet: the frame's authored `w`×`h`,
/// top-left anchored inside its uniform cell. Refuses anything that would
/// read outside the decoded sheet instead of returning smeared pixels.
pub fn cut_cell(
    sheet: &[u32],
    sheet_w: usize,
    sheet_h: usize,
    layout: SheetLayout,
    cell: usize,
    w: usize,
    h: usize,
) -> Result<(Vec<u32>, usize, usize), String> {
    if w == 0 || h == 0 {
        return Err("sheet frame has zero size".into());
    }
    if w > layout.cell_w || h > layout.cell_h {
        return Err(format!(
            "frame {w}x{h} does not fit cell {}x{}",
            layout.cell_w, layout.cell_h
        ));
    }
    if sheet.len() < sheet_w * sheet_h {
        return Err("sheet pixels shorter than its dimensions".into());
    }
    let (x0, y0) = cell_origin(layout, cell);
    if x0 + w > sheet_w || y0 + h > sheet_h {
        return Err(format!("cell {cell} lies outside a {sheet_w}x{sheet_h} sheet"));
    }
    let mut out = vec![0u32; w * h];
    for y in 0..h {
        let src = (y0 + y) * sheet_w + x0;
        let dst = y * w;
        out[dst..dst + w].copy_from_slice(&sheet[src..src + w]);
    }
    Ok((out, w, h))
}

#[derive(Clone, Debug)]
pub struct PreparedState {
    pub name: String,
    pub fps: f32,
    pub r#loop: bool,
    pub frames: Vec<(Vec<u32>, usize, usize)>,
}

#[derive(Clone, Debug)]
pub struct PreparedBillboard {
    pub preview: String,
    pub states: Vec<PreparedState>,
}

/// The packed sheet a sheet-backed manifest names, decoded once. Every
/// frame line of such a manifest carries the SAME file (the sheet); the
/// importer's `sheet_file()` picks it the same way.
pub fn sheet_beside(bb: &Manifest, root: &Path) -> Option<Result<(Vec<u32>, usize, usize), String>> {
    bb.sheet?;
    let file = bb
        .frames
        .iter()
        .map(|f| f.file.as_str())
        .find(|f| !f.is_empty())?;
    Some(decode_frame(&root.join(file)))
}

pub fn prepare(path: &Path) -> Result<PreparedBillboard, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let bb = Manifest::parse(&text)?;
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    // The library lands one packed sheet per actor now; only older
    // manifests still have a PNG per frame.
    if let Some(sheet) = sheet_beside(&bb, root) {
        let (pixels, w, h) = sheet?;
        return assemble_cut(&bb, &pixels, w, h);
    }
    let mut decoded: Vec<Option<(Vec<u32>, usize, usize)>> = vec![None; bb.frames.len()];
    assemble_states(&bb, |index| {
        if decoded[index].is_none() {
            decoded[index] = decode_frame(&root.join(&bb.frames[index].file)).ok();
        }
        decoded[index].clone()
    })
}

/// Same actor, one download: the catalog publishes every frame in a packed
/// sheet plus this manifest, so the frames are cut out of already-decoded
/// pixels instead of fetched one PNG at a time.
pub fn prepare_from_sheet(
    text: &str,
    sheet: &[u32],
    sheet_w: usize,
    sheet_h: usize,
) -> Result<PreparedBillboard, String> {
    let bb = Manifest::parse(text)?;
    assemble_cut(&bb, sheet, sheet_w, sheet_h)
}

/// Build the playable states by cutting each frame out of a packed sheet.
fn assemble_cut(
    bb: &Manifest,
    sheet: &[u32],
    sheet_w: usize,
    sheet_h: usize,
) -> Result<PreparedBillboard, String> {
    let layout = bb.sheet.ok_or("billboard manifest has no sheet header")?;
    let mut cut: Vec<Option<(Vec<u32>, usize, usize)>> = vec![None; bb.frames.len()];
    assemble_states(bb, |index| {
        if cut[index].is_none() {
            let frame = &bb.frames[index];
            cut[index] = frame.cell.and_then(|cell| {
                cut_cell(
                    sheet,
                    sheet_w,
                    sheet_h,
                    layout,
                    cell,
                    frame.w as usize,
                    frame.h as usize,
                )
                .ok()
            });
        }
        cut[index].clone()
    })
}

/// Shared state assembly: `pixels(frame_index)` supplies (and caches) the
/// decoded frame, whichever source it came from.
fn assemble_states(
    bb: &Manifest,
    mut pixels: impl FnMut(usize) -> Option<(Vec<u32>, usize, usize)>,
) -> Result<PreparedBillboard, String> {
    let mut states = Vec::new();
    for name in bb.playable_states() {
        let frames: Vec<(Vec<u32>, usize, usize)> = bb
            .indices_for_state(&name)
            .into_iter()
            .filter_map(&mut pixels)
            .collect();
        if frames.is_empty() {
            continue;
        }
        let (fps, r#loop) = bb.state_meta(&name);
        states.push(PreparedState { name, fps, r#loop, frames });
    }
    if states.is_empty() {
        return Err("billboard has no playable states".into());
    }
    Ok(PreparedBillboard {
        preview: bb.preview.clone(),
        states,
    })
}

pub fn decode_frame(path: &PathBuf) -> Result<(Vec<u32>, usize, usize), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let image = if bytes.starts_with(&[0xff, 0xd8]) {
        makepad_widgets::ImageBuffer::from_jpg(&bytes)
    } else {
        makepad_widgets::ImageBuffer::from_png(&bytes)
    }
    .map_err(|e| format!("billboard frame: {e:?}"))?;
    let (w, h) = (image.width, image.height);
    if w == 0 || h == 0 {
        return Err("empty frame".into());
    }
    let mut data = image.data;
    key_sprite_alpha(&mut data);
    Ok((data, w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_engine_manifest() {
        let text = "\
stateful-billboard 1
prefix troo
role character
preview walk
state walk 0 2 1 8
state attack 2 4 0 10
frame 0 A 1 32 48 a.png
frame 1 B 1 32 48 b.png
frame 2 C 1 40 48 c.png
frame 3 D 1 40 48 d.png
";
        let m = Manifest::parse(text).unwrap();
        assert_eq!(m.preview, "walk");
        assert_eq!(m.states.len(), 2);
        assert_eq!(m.frames.len(), 4);
        assert!(m.states[0].r#loop);
        assert!(!m.states[1].r#loop);
        assert_eq!(m.frames[0].letter, 'A');
        assert_eq!(m.frames[0].rot, 1);
        assert_eq!((m.frames[0].w, m.frames[0].h), (32, 48));
        let walk = m.preview_frames();
        assert_eq!(walk.len(), 2);
        assert_eq!(walk[0].file, "a.png");
        assert_eq!(walk[1].file, "b.png");
        let attack = m.frames_for_state("attack");
        assert_eq!(attack.len(), 2);
        assert_eq!(attack[0].file, "c.png");
    }

    #[test]
    fn preview_drops_side_rotations() {
        let text = "\
stateful-billboard 1
preview walk
state walk 0 6 1 8
frame 0 A 1 40 80 a1.png
frame 1 A 2 54 81 a2.png
frame 2 A 3 53 82 a3.png
frame 3 B 1 41 80 b1.png
frame 4 B 2 55 81 b2.png
frame 5 B 3 54 82 b3.png
";
        let m = Manifest::parse(text).unwrap();
        let prev = m.preview_frames();
        assert_eq!(
            prev.iter().map(|f| f.file.as_str()).collect::<Vec<_>>(),
            ["a1.png", "b1.png"]
        );
        assert_eq!((prev[0].w, prev[0].h), (40, 80));
        assert_eq!((prev[1].w, prev[1].h), (41, 80));
    }

    /// The grouped-actor manifest the catalog publishes beside a packed
    /// sheet: `sheet` header + a `cell` index on every frame.
    fn sheet_manifest() -> &'static str {
        "\
stateful-billboard 1
prefix troo
role character
preview walk
facings 8
sheet 4 8 8
state walk 0 3 1 8
state pain 3 4 0 6
frame 0 A 1 8 8 trooa1.png cell 0
frame 1 A 2 6 8 trooa2.png flip cell 1
frame 2 B 1 4 3 troob1.png cell 2
frame 3 G 1 8 8 troog1.png cell 5
"
    }

    /// 32×16 sheet, 4 columns of 8×8 cells: cell n is filled with n+1 so a
    /// mis-cut frame is visible as the wrong constant.
    fn synthetic_sheet() -> (Vec<u32>, usize, usize) {
        let (w, h) = (32usize, 16usize);
        let mut px = vec![0u32; w * h];
        for y in 0..h {
            for x in 0..w {
                let cell = (y / 8) * 4 + (x / 8);
                px[y * w + x] = 0xff00_0000 | (cell as u32 + 1);
            }
        }
        (px, w, h)
    }

    #[test]
    fn parses_sheet_header_and_cell_indices() {
        let m = Manifest::parse(sheet_manifest()).unwrap();
        assert_eq!(m.sheet, Some(SheetLayout { cols: 4, cell_w: 8, cell_h: 8 }));
        assert_eq!(
            m.frames.iter().map(|f| f.cell).collect::<Vec<_>>(),
            [Some(0), Some(1), Some(2), Some(5)]
        );
        // A `flip` token before `cell` must not swallow the index.
        assert_eq!(m.frames[1].cell, Some(1));
        // Old manifests (no sheet, no cell) still parse.
        let legacy = Manifest::parse(
            "stateful-billboard 1\npreview idle\nstate idle 0 1 1 8\nframe 0 A 1 8 8 a.png\n",
        )
        .unwrap();
        assert!(legacy.sheet.is_none());
        assert_eq!(legacy.frames[0].cell, None);
    }

    #[test]
    fn cell_geometry_is_row_major_and_top_left_anchored() {
        let layout = SheetLayout { cols: 4, cell_w: 8, cell_h: 8 };
        assert_eq!(cell_origin(layout, 0), (0, 0));
        assert_eq!(cell_origin(layout, 3), (24, 0));
        assert_eq!(cell_origin(layout, 4), (0, 8));
        assert_eq!(cell_origin(layout, 5), (8, 8));
        let (sheet, w, h) = synthetic_sheet();
        // Cell 5 is the second row's second cell: value 6 everywhere.
        let (px, fw, fh) = cut_cell(&sheet, w, h, layout, 5, 8, 8).unwrap();
        assert_eq!((fw, fh), (8, 8));
        assert!(px.iter().all(|p| *p == 0xff00_0006), "cut the wrong cell");
        // A smaller frame reads the cell's TOP-LEFT corner only.
        let (px, fw, fh) = cut_cell(&sheet, w, h, layout, 2, 4, 3).unwrap();
        assert_eq!((fw, fh, px.len()), (4, 3, 12));
        assert!(px.iter().all(|p| *p == 0xff00_0003));
        // Out-of-bounds and over-large frames refuse instead of smearing.
        assert!(cut_cell(&sheet, w, h, layout, 8, 8, 8).is_err());
        assert!(cut_cell(&sheet, w, h, layout, 0, 9, 8).is_err());
        assert!(cut_cell(&sheet, w, h, layout, 0, 0, 8).is_err());
    }

    #[test]
    fn sheet_prepare_builds_states_at_authored_sizes() {
        let (sheet, w, h) = synthetic_sheet();
        let prepared = prepare_from_sheet(sheet_manifest(), &sheet, w, h).unwrap();
        assert_eq!(prepared.preview, "walk");
        let names: Vec<&str> = prepared.states.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["walk", "pain"]);
        let walk = &prepared.states[0];
        // walk is frames 0..3, of which only rot<=1 play: cells 0 and 2.
        assert_eq!(walk.frames.len(), 2);
        assert_eq!((walk.frames[0].1, walk.frames[0].2), (8, 8));
        assert!(walk.frames[0].0.iter().all(|p| *p == 0xff00_0001));
        assert_eq!((walk.frames[1].1, walk.frames[1].2), (4, 3));
        assert!(walk.frames[1].0.iter().all(|p| *p == 0xff00_0003));
        assert_eq!(walk.fps, 8.0);
        assert!(walk.r#loop);
        let pain = &prepared.states[1];
        assert_eq!(pain.frames.len(), 1);
        assert!(pain.frames[0].0.iter().all(|p| *p == 0xff00_0006));
        assert!(!pain.r#loop);
        // A manifest without the sheet header is not a sheet source.
        assert!(prepare_from_sheet(
            "stateful-billboard 1\npreview idle\nframe 0 A 1 8 8 a.png\n",
            &sheet,
            w,
            h,
        )
        .is_err());
    }
}
