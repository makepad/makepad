// Text shaping + glyph rasterisation of a fixed multi-paragraph document, the way makepad's text
// stack does it (draw/src/text: rustybuzz shaping, ttf-parser outlines, ab_glyph_rasterizer
// coverage; glyph_outline.rs `rasterize` is mirrored below). makepad-draw itself needs the
// platform layer (windows, GPU), so it cannot run headless; the libraries and the call sequence are
// the same. Per iteration (a "document open" with a cold glyph cache): shape every paragraph,
// break lines greedily at a fixed width, rasterise every distinct (glyph, size) once.
use crate::{fold, inputs};
use rustybuzz::ttf_parser::{GlyphId, OutlineBuilder};
use std::collections::HashMap;

#[derive(Clone, Copy)]
struct Pt {
    x: f32,
    y: f32,
}

enum Cmd {
    Move(Pt),
    Line(Pt),
    Quad(Pt, Pt),
    Cubic(Pt, Pt, Pt),
    Close,
}

#[derive(Default)]
struct Builder {
    cmds: Vec<Cmd>,
}

impl OutlineBuilder for Builder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.cmds.push(Cmd::Move(Pt { x, y }));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.cmds.push(Cmd::Line(Pt { x, y }));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.cmds.push(Cmd::Quad(Pt { x: x1, y: y1 }, Pt { x, y }));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.cmds.push(Cmd::Cubic(Pt { x: x1, y: y1 }, Pt { x: x2, y: y2 }, Pt { x, y }));
    }
    fn close(&mut self) {
        self.cmds.push(Cmd::Close);
    }
}

/// glyph_outline.rs `GlyphOutline::rasterize`: the outline scaled to `px_per_em`, origin at the
/// bounds corner, into a coverage image (rows top-down); returns the image digest and size
fn rasterize(face: &rustybuzz::Face, glyph: u16, px_per_em: f32) -> (u64, u32, u32) {
    use ab_glyph_rasterizer::{point, Rasterizer};
    let mut b = Builder::default();
    let bounds = match face.outline_glyph(GlyphId(glyph), &mut b) {
        Some(r) => r,
        None => return (0, 0, 0),
    };
    let upem = face.units_per_em() as f32;
    let s = px_per_em / upem;
    let (ox, oy) = (bounds.x_min as f32, bounds.y_min as f32);
    let w = (((bounds.x_max - bounds.x_min) as f32 * s).ceil() as u32 + 1).max(1);
    let h = (((bounds.y_max - bounds.y_min) as f32 * s).ceil() as u32 + 1).max(1);
    let t = |p: Pt| point((p.x - ox) * s, (p.y - oy) * s);
    let mut r = Rasterizer::new(w as usize, h as usize);
    let mut last = Pt { x: 0.0, y: 0.0 };
    let mut last_move: Option<Pt> = None;
    for c in &b.cmds {
        match *c {
            Cmd::Move(p) => {
                last = p;
                last_move = Some(p);
            }
            Cmd::Line(p) => {
                r.draw_line(t(last), t(p));
                last = p;
            }
            Cmd::Quad(p1, p) => {
                r.draw_quad(t(last), t(p1), t(p));
                last = p;
            }
            Cmd::Cubic(p1, p2, p) => {
                r.draw_cubic(t(last), t(p1), t(p2), t(p));
                last = p;
            }
            Cmd::Close => {
                if let Some(m) = last_move.take() {
                    r.draw_line(t(last), t(m));
                    last = m;
                }
            }
        }
    }
    let mut img = vec![0u8; (w * h) as usize];
    r.for_each_pixel_2d(|x, y, a| {
        img[((h - 1 - y) * w + x) as usize] = (a * 255.0) as u8;
    });
    let mut d = fold(0, (w as u64) << 32 | h as u64);
    for px in img.iter() {
        d = fold(d, *px as u64);
    }
    (d, w, h)
}

fn document() -> Vec<(String, f32)> {
    // paragraphs of the repo's own markdown; '#' headings set larger
    let mut out = Vec::new();
    for f in ["README.md", "AGENTS.md"] {
        let text = String::from_utf8_lossy(&inputs::read(f)).to_string();
        for p in text.split("\n\n") {
            let p = p.trim();
            if p.is_empty() || p.starts_with("```") {
                continue;
            }
            let (txt, size) = match p.strip_prefix('#') {
                Some(h) => (h.trim_start_matches('#').trim().replace('\n', " "), 24.0),
                None => (p.replace('\n', " "), 16.0),
            };
            out.push((txt, size));
        }
    }
    out
}

pub fn setup() -> Box<dyn FnMut() -> u64> {
    let font = inputs::read("widgets/resources/IBMPlexSans-Text.ttf");
    let doc = document();
    Box::new(move || {
        let face = rustybuzz::Face::from_slice(&font, 0).unwrap();
        let upem = face.units_per_em() as f32;
        let feats: Vec<rustybuzz::Feature> = Vec::new();
        let width = 640.0f32;
        let mut cache: HashMap<(u16, u32), u64> = HashMap::new();
        let mut h = 0u64;
        let mut y = 0.0f32;
        for (para, size) in &doc {
            let mut buf = rustybuzz::UnicodeBuffer::new();
            buf.push_str(para);
            let out = rustybuzz::shape(&face, &feats, buf);
            let scale = *size / upem;
            let mut x = 0.0f32;
            let mut lines = 1u64;
            let infos = out.glyph_infos();
            let pos = out.glyph_positions();
            let bytes = para.as_bytes();
            let mut i = 0;
            while i < infos.len() {
                // a word: glyphs up to and including the next space
                let mut j = i;
                let mut wadv = 0.0f32;
                while j < infos.len() {
                    wadv += pos[j].x_advance as f32 * scale;
                    let is_space = bytes[infos[j].cluster as usize] == b' ';
                    j += 1;
                    if is_space {
                        break;
                    }
                }
                if x + wadv > width && x > 0.0 {
                    x = 0.0;
                    y += *size * 1.3;
                    lines += 1;
                }
                for k in i..j {
                    let g = infos[k].glyph_id as u16;
                    let key = (g, size.to_bits());
                    let d = *cache.entry(key).or_insert_with(|| rasterize(&face, g, *size).0);
                    let gx = x + pos[k].x_offset as f32 * scale;
                    h = fold(h, d ^ ((gx * 64.0) as i32 as u32 as u64) << 20 ^ ((y * 64.0) as i32 as u32 as u64) << 40);
                    x += pos[k].x_advance as f32 * scale;
                }
                i = j;
            }
            h = fold(h, lines);
            y += *size * 0.8;
        }
        // cache contents folded in key order (HashMap iteration order is random)
        let mut keys: Vec<_> = cache.iter().collect();
        keys.sort();
        for (k, v) in keys {
            h = fold(h, (k.0 as u64) << 32 ^ k.1 as u64 ^ *v);
        }
        h
    })
}
