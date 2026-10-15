// Frame-1 SDF glyph builds, the way makepad's text stack makes them (draw/src/text/sdfer.rs): ttf-parser
// outline -> ab_glyph_rasterizer coverage -> sdfer ESDT (solidify on, padding 4, radius 8, cutoff 0.25,
// layouter.rs) into a caller-owned output image with reused buffers. Per iteration (a cold atlas): every
// glyph of a calculator UI's character set at the three sizes Calculator's first frame uses
// (32/34/80 px/em), so the SDF kernel dominates like it does before the first present.
use crate::{fold, inputs};
use rustybuzz::ttf_parser::{GlyphId, OutlineBuilder};
use sdfer::{esdt, Image2d, Unorm8};

struct Builder {
    r: ab_glyph_rasterizer::Rasterizer,
    s: f32,
    ox: f32,
    oy: f32,
    last: (f32, f32),
    start: (f32, f32),
}

impl Builder {
    fn p(&self, x: f32, y: f32) -> ab_glyph_rasterizer::Point {
        ab_glyph_rasterizer::point((x - self.ox) * self.s, (y - self.oy) * self.s)
    }
}

impl OutlineBuilder for Builder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.last = (x, y);
        self.start = (x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (a, b) = (self.p(self.last.0, self.last.1), self.p(x, y));
        self.r.draw_line(a, b);
        self.last = (x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (a, b, c) = (self.p(self.last.0, self.last.1), self.p(x1, y1), self.p(x, y));
        self.r.draw_quad(a, b, c);
        self.last = (x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (a, b, c, d) = (self.p(self.last.0, self.last.1), self.p(x1, y1), self.p(x2, y2), self.p(x, y));
        self.r.draw_cubic(a, b, c, d);
        self.last = (x, y);
    }
    fn close(&mut self) {
        if self.last != self.start {
            let (a, b) = (self.p(self.last.0, self.last.1), self.p(self.start.0, self.start.1));
            self.r.draw_line(a, b);
            self.last = self.start;
        }
    }
}

const PAD: usize = 4;

pub fn setup() -> Box<dyn FnMut() -> u64> {
    let font = inputs::read("widgets/resources/IBMPlexSans-Text.ttf");
    let chars: Vec<char> = "0123456789+-*/=.,%()ACDELMRSabcdefghilmnorstuvxy".chars().collect();
    let mut bufs = esdt::ReusableBuffers::default();
    Box::new(move || {
        let face = rustybuzz::ttf_parser::Face::parse(&font, 0).unwrap();
        let upem = face.units_per_em() as f32;
        let mut h = 0u64;
        for size in [32.0f32, 34.0, 80.0] {
            for &c in &chars {
                let Some(g) = face.glyph_index(c) else { continue };
                let Some(bb) = face.glyph_bounding_box(GlyphId(g.0)) else { continue };
                let s = size / upem;
                let w = (((bb.x_max - bb.x_min) as f32 * s).ceil() as usize + 1).max(1);
                let ht = (((bb.y_max - bb.y_min) as f32 * s).ceil() as usize + 1).max(1);
                let mut b = Builder { r: ab_glyph_rasterizer::Rasterizer::new(w, ht), s, ox: bb.x_min as f32, oy: bb.y_min as f32, last: (0.0, 0.0), start: (0.0, 0.0) };
                face.outline_glyph(GlyphId(g.0), &mut b);
                let mut cov: Image2d<Unorm8> = Image2d::new(w, ht);
                b.r.for_each_pixel_2d(|x, y, a| {
                    cov[(x as usize, ht - 1 - y as usize)] = Unorm8::encode(a);
                });
                let mut out: Image2d<Unorm8> = Image2d::new(w + PAD * 2, ht + PAD * 2);
                esdt::glyph_to_sdf_into(&mut cov, esdt::Params { pad: PAD, radius: 8.0, cutoff: 0.25, ..esdt::Params::default() }, &mut bufs, &mut out);
                h = fold(h, (w as u64) << 32 | ht as u64);
                for y in 0..out.height() {
                    for x in 0..out.width() {
                        h = fold(h, out[(x, y)].to_bits() as u64);
                    }
                }
            }
        }
        h
    })
}
