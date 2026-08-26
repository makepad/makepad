//! A z-buffered triangle rasteriser, so a human can look at the shots.
//!
//! Deliberately minimal: perspective projection, per-face flat shading from a
//! single sun plus a sky term, one colour per element class. It is not trying
//! to be the viewer's renderer — it is trying to answer "is the camera inside
//! a wall, is it pointing at anything, does this read as a building" from a
//! PNG, without a GPU, in a test.
//!
//! Returns RGB bytes; encoding is the caller's problem (the `qa` binary uses
//! `makepad-zune-png`).

use crate::scene::{TourClass, TourScene};
use crate::track::TourKey;
use makepad_math::{vec3, Vec3f};

pub struct Image {
    pub width: usize,
    pub height: usize,
    /// RGB8, row major, top row first.
    pub rgb: Vec<u8>,
}

impl Image {
    pub fn new(width: usize, height: usize) -> Image {
        Image {
            width,
            height,
            rgb: vec![0; width * height * 3],
        }
    }

    #[inline]
    pub fn put(&mut self, x: usize, y: usize, c: [u8; 3]) {
        let i = (y * self.width + x) * 3;
        self.rgb[i] = c[0];
        self.rgb[i + 1] = c[1];
        self.rgb[i + 2] = c[2];
    }

    /// Paste `src` at `(ox, oy)`, clipped.
    pub fn blit(&mut self, src: &Image, ox: usize, oy: usize) {
        for y in 0..src.height {
            let dy = oy + y;
            if dy >= self.height {
                break;
            }
            for x in 0..src.width {
                let dx = ox + x;
                if dx >= self.width {
                    break;
                }
                let s = (y * src.width + x) * 3;
                self.put(dx, dy, [src.rgb[s], src.rgb[s + 1], src.rgb[s + 2]]);
            }
        }
    }

    pub fn fill(&mut self, c: [u8; 3]) {
        for p in self.rgb.chunks_exact_mut(3) {
            p.copy_from_slice(&c);
        }
    }

    /// A 3×5 bitmap font, enough for labels on a contact sheet.
    pub fn text(&mut self, x: usize, y: usize, s: &str, c: [u8; 3], scale: usize) {
        let mut cx = x;
        for ch in s.chars() {
            let g = glyph(ch);
            for (row, bits) in g.iter().enumerate() {
                for col in 0..3 {
                    if bits & (1 << (2 - col)) == 0 {
                        continue;
                    }
                    for sy in 0..scale {
                        for sx in 0..scale {
                            let px = cx + col * scale + sx;
                            let py = y + row * scale + sy;
                            if px < self.width && py < self.height {
                                self.put(px, py, c);
                            }
                        }
                    }
                }
            }
            cx += 4 * scale;
            if cx + 4 * scale >= self.width {
                return;
            }
        }
    }
}

fn glyph(ch: char) -> [u8; 5] {
    match ch.to_ascii_uppercase() {
        'A' => [0b010, 0b101, 0b111, 0b101, 0b101],
        'B' => [0b110, 0b101, 0b110, 0b101, 0b110],
        'C' => [0b011, 0b100, 0b100, 0b100, 0b011],
        'D' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'E' => [0b111, 0b100, 0b110, 0b100, 0b111],
        'F' => [0b111, 0b100, 0b110, 0b100, 0b100],
        'G' => [0b011, 0b100, 0b101, 0b101, 0b011],
        'H' => [0b101, 0b101, 0b111, 0b101, 0b101],
        'I' => [0b111, 0b010, 0b010, 0b010, 0b111],
        'J' => [0b001, 0b001, 0b001, 0b101, 0b010],
        'K' => [0b101, 0b110, 0b100, 0b110, 0b101],
        'L' => [0b100, 0b100, 0b100, 0b100, 0b111],
        'M' => [0b101, 0b111, 0b111, 0b101, 0b101],
        'N' => [0b101, 0b111, 0b111, 0b111, 0b101],
        'O' => [0b010, 0b101, 0b101, 0b101, 0b010],
        'P' => [0b110, 0b101, 0b110, 0b100, 0b100],
        'Q' => [0b010, 0b101, 0b101, 0b110, 0b011],
        'R' => [0b110, 0b101, 0b110, 0b101, 0b101],
        'S' => [0b011, 0b100, 0b010, 0b001, 0b110],
        'T' => [0b111, 0b010, 0b010, 0b010, 0b010],
        'U' => [0b101, 0b101, 0b101, 0b101, 0b011],
        'V' => [0b101, 0b101, 0b101, 0b101, 0b010],
        'W' => [0b101, 0b101, 0b111, 0b111, 0b101],
        'X' => [0b101, 0b101, 0b010, 0b101, 0b101],
        'Y' => [0b101, 0b101, 0b010, 0b010, 0b010],
        'Z' => [0b111, 0b001, 0b010, 0b100, 0b111],
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b011, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        '.' => [0b000, 0b000, 0b000, 0b000, 0b010],
        ',' => [0b000, 0b000, 0b000, 0b010, 0b100],
        '-' => [0b000, 0b000, 0b111, 0b000, 0b000],
        ':' => [0b000, 0b010, 0b000, 0b010, 0b000],
        '/' => [0b001, 0b001, 0b010, 0b100, 0b100],
        '%' => [0b101, 0b001, 0b010, 0b100, 0b101],
        '#' => [0b101, 0b111, 0b101, 0b111, 0b101],
        '(' => [0b001, 0b010, 0b010, 0b010, 0b001],
        ')' => [0b100, 0b010, 0b010, 0b010, 0b100],
        '_' => [0b000, 0b000, 0b000, 0b000, 0b111],
        '+' => [0b000, 0b010, 0b111, 0b010, 0b000],
        _ => [0b000, 0b000, 0b000, 0b000, 0b000],
    }
}

fn class_color(c: TourClass) -> [f32; 3] {
    match c {
        TourClass::Wall => [0.86, 0.84, 0.80],
        TourClass::Slab => [0.62, 0.60, 0.58],
        TourClass::Roof => [0.55, 0.28, 0.22],
        TourClass::Column | TourClass::Beam => [0.72, 0.70, 0.66],
        TourClass::Door => [0.48, 0.34, 0.20],
        TourClass::Window | TourClass::CurtainWall | TourClass::Skylight => [0.55, 0.72, 0.82],
        TourClass::Stair => [0.70, 0.66, 0.58],
        TourClass::Railing => [0.40, 0.40, 0.44],
        TourClass::Furniture => [0.50, 0.42, 0.36],
        TourClass::Lamp => [0.90, 0.86, 0.60],
        TourClass::Site => [0.36, 0.46, 0.30],
        _ => [0.70, 0.70, 0.72],
    }
}

/// Render one camera key. Returns an image of `width` × `height`.
pub fn render(scene: &TourScene, key: &TourKey, width: usize, height: usize) -> Image {
    let mut img = Image::new(width, height);
    let mut depth = vec![f32::INFINITY; width * height];

    // Sky gradient, so up is obvious even in an empty frame.
    for y in 0..height {
        let f = y as f32 / height as f32;
        let c = [
            (150.0 + 60.0 * (1.0 - f)) as u8,
            (175.0 + 50.0 * (1.0 - f)) as u8,
            (205.0 + 40.0 * (1.0 - f)) as u8,
        ];
        for x in 0..width {
            img.put(x, y, c);
        }
    }

    let eye = key.pos;
    let fwd = key.dir();
    let world_up = key.up.normalize();
    let right = Vec3f::cross(fwd, world_up).normalize();
    let up = Vec3f::cross(right, fwd).normalize();
    let aspect = width as f32 / height as f32;
    let tan_half = (key.fov_y_deg.to_radians() * 0.5).tan();
    let near = 0.03f32;
    let sun = vec3(0.35, -0.55, 0.76).normalize();

    let project = |p: Vec3f| -> Option<(f32, f32, f32)> {
        let v = p - eye;
        let z = v.dot(fwd);
        if z <= near {
            return None;
        }
        let x = v.dot(right) / (z * tan_half * aspect);
        let y = v.dot(up) / (z * tan_half);
        Some((
            (x * 0.5 + 0.5) * width as f32,
            (0.5 - y * 0.5) * height as f32,
            z,
        ))
    };

    for tri in 0..scene.triangle_count() {
        let Some(elem) = scene.element_of_triangle(tri) else {
            continue;
        };
        if elem.class == TourClass::Zone {
            continue;
        }
        let [a, b, c] = scene.triangle(tri);
        // Clip-by-rejection: any vertex behind the eye drops the triangle.
        // Good enough for a QA sheet and it keeps this function short.
        let (Some(pa), Some(pb), Some(pc)) = (project(a), project(b), project(c)) else {
            continue;
        };
        let minx = pa.0.min(pb.0).min(pc.0).floor().max(0.0) as usize;
        let maxx = (pa.0.max(pb.0).max(pc.0).ceil() as isize).clamp(0, width as isize) as usize;
        let miny = pa.1.min(pb.1).min(pc.1).floor().max(0.0) as usize;
        let maxy = (pa.1.max(pb.1).max(pc.1).ceil() as isize).clamp(0, height as isize) as usize;
        if minx >= maxx || miny >= maxy {
            continue;
        }
        let area = (pb.0 - pa.0) * (pc.1 - pa.1) - (pc.0 - pa.0) * (pb.1 - pa.1);
        if area.abs() < 1e-6 {
            continue;
        }
        let inv_area = 1.0 / area;

        let n = Vec3f::cross(b - a, c - a);
        let nl = n.length();
        let n = if nl < 1e-9 { vec3(0.0, 0.0, 1.0) } else { n * (1.0 / nl) };
        let facing = n.dot(sun).abs();
        let base = class_color(elem.class);
        let lit = 0.32 + 0.68 * facing;
        let col = [
            ((base[0] * lit).clamp(0.0, 1.0) * 255.0) as u8,
            ((base[1] * lit).clamp(0.0, 1.0) * 255.0) as u8,
            ((base[2] * lit).clamp(0.0, 1.0) * 255.0) as u8,
        ];

        for y in miny..maxy {
            for x in minx..maxx {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let w0 = ((pb.0 - pa.0) * (py - pa.1) - (px - pa.0) * (pb.1 - pa.1)) * inv_area;
                let w1 = ((px - pa.0) * (pc.1 - pa.1) - (pc.0 - pa.0) * (py - pa.1)) * inv_area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                // Perspective-correct depth.
                let z = 1.0 / (w2 / pa.2 + w1 / pb.2 + w0 / pc.2);
                let di = y * width + x;
                if z < depth[di] {
                    depth[di] = z;
                    img.put(x, y, col);
                }
            }
        }
    }
    img
}

/// Lay images out in a grid with captions under each.
pub fn contact_sheet(tiles: &[(Image, String)], cols: usize, title: &str) -> Image {
    if tiles.is_empty() {
        let mut i = Image::new(640, 80);
        i.fill([24, 24, 28]);
        i.text(8, 8, title, [230, 230, 235], 2);
        return i;
    }
    let tw = tiles[0].0.width;
    let th = tiles[0].0.height;
    let cap = 18usize;
    let pad = 6usize;
    let head = 30usize;
    let cols = cols.max(1);
    let rows = tiles.len().div_ceil(cols);
    let w = cols * (tw + pad) + pad;
    let h = head + rows * (th + cap + pad) + pad;
    let mut sheet = Image::new(w, h);
    sheet.fill([22, 22, 26]);
    sheet.text(pad, 9, title, [235, 235, 240], 2);
    for (i, (img, caption)) in tiles.iter().enumerate() {
        let cx = pad + (i % cols) * (tw + pad);
        let cy = head + (i / cols) * (th + cap + pad);
        sheet.blit(img, cx, cy);
        sheet.text(cx + 2, cy + th + 5, caption, [190, 195, 205], 1);
    }
    sheet
}
