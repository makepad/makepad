//! Stateful billboard playback — same `stateful-billboard` text format the
//! game importer / asset-ui viewer uses. Frames play like a video clip;
//! named states (walk, attack, pain, …) are switched from the UI.
//!
//! Library tiles and the mixer both play the front-facing (rot 0/1) frames
//! at each frame's authored size — never a contact sheet of every rotation.

use crate::media::key_sprite_alpha;
use std::path::{Path, PathBuf};

pub const MAGIC: &str = "stateful-billboard";

#[derive(Clone, Debug)]
pub struct SpriteFrame {
    pub letter: char,
    pub rot: u8,
    pub w: u32,
    pub h: u32,
    pub file: String,
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
        for line in lines {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_whitespace();
            match parts.next() {
                Some("preview") => preview = parts.next().unwrap_or("").to_string(),
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
                    let file = parts.next().unwrap_or("").to_string();
                    if !file.is_empty() {
                        frames.push(SpriteFrame { letter, rot, w, h, file });
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
        Ok(Self { preview, states, frames })
    }

    fn state_range(&self, name: &str) -> std::ops::Range<usize> {
        self.states
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.first..s.last)
            .unwrap_or(0..self.frames.len())
    }

    fn frames_in(&self, range: std::ops::Range<usize>) -> Vec<&SpriteFrame> {
        range.filter_map(|i| self.frames.get(i)).collect()
    }

    /// Front-facing (rot 1 or 0) frames of the preview state — same set the
    /// asset-ui library card cycles. Native sizes, not a rotation sheet.
    pub fn preview_frames(&self) -> Vec<&SpriteFrame> {
        let src = self.frames_in(self.state_range(&self.preview));
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
        let src = self.frames_in(self.state_range(name));
        if src.is_empty() {
            return self.preview_frames();
        }
        let front: Vec<&SpriteFrame> = src
            .iter()
            .copied()
            .filter(|f| f.rot == 1 || f.rot == 0)
            .collect();
        if !front.is_empty() {
            return front;
        }
        src
    }
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

pub fn prepare(path: &Path) -> Result<PreparedBillboard, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let bb = Manifest::parse(&text)?;
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    let mut decoded: Vec<Option<(Vec<u32>, usize, usize)>> = vec![None; bb.frames.len()];
    let names: Vec<String> = if bb.states.is_empty() {
        vec![bb.preview.clone()]
    } else {
        bb.states.iter().map(|s| s.name.clone()).collect()
    };
    let mut states = Vec::new();
    for name in names {
        if name.starts_with("pose_") {
            continue;
        }
        let mut frames = Vec::new();
        for frame in bb.frames_for_state(&name) {
            let Some(index) = bb.frames.iter().position(|f| {
                f.file == frame.file && f.letter == frame.letter && f.rot == frame.rot
            }) else {
                continue;
            };
            if decoded[index].is_none() {
                decoded[index] = decode_frame(&root.join(&frame.file)).ok();
            }
            if let Some(pix) = decoded[index].clone() {
                frames.push(pix);
            }
        }
        if frames.is_empty() {
            continue;
        }
        let meta = bb.states.iter().find(|s| s.name == name);
        states.push(PreparedState {
            fps: meta.map(|s| s.fps).unwrap_or(8).clamp(1, 30) as f32,
            r#loop: meta.map(|s| s.r#loop).unwrap_or(true),
            name,
            frames,
        });
    }
    if states.is_empty() {
        return Err("billboard has no playable states".into());
    }
    Ok(PreparedBillboard {
        preview: bb.preview,
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
}
