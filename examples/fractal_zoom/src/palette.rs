//! The built-in gradients, baked into one palette atlas (256 x N texels, one
//! row per palette) that the tile shader and the swatches both sample, and
//! the swatch grid that picks one.

use makepad_widgets::*;

pub const PALETTE_WIDTH: usize = 256;

pub struct Palette {
    pub name: &'static str,
    /// (position 0..1, 0xRRGGBB); the gradient wraps from the last stop back
    /// to the first, so every palette cycles seamlessly.
    pub stops: &'static [(f32, u32)],
    /// A cosine palette a + b cos(2 pi (c t + d)) instead of stops (c whole,
    /// so it cycles too). The original zoomer coloured with one of these.
    pub cosine: Option<[[f32; 3]; 4]>,
}

pub const PALETTES: &[Palette] = &[
    // the original fractal zoomer's palette (its shader's Pal::iq2)
    Palette { name: "Classic", stops: &[], cosine: Some([[0.5, 0.5, 0.5], [0.5, 0.5, 0.5], [1.0, 1.0, 1.0], [0.0, 0.1, 0.2]]) },
    Palette { name: "Prism", stops: &[], cosine: Some([[0.5, 0.5, 0.5], [0.5, 0.5, 0.5], [1.0, 1.0, 1.0], [0.0, 0.33, 0.67]]) },
    Palette { name: "Dusk", stops: &[], cosine: Some([[0.8, 0.5, 0.4], [0.2, 0.4, 0.2], [2.0, 1.0, 1.0], [0.0, 0.25, 0.25]]) },
    Palette { name: "Azure Gold", stops: &[(0.0, 0x000764), (0.16, 0x206bcb), (0.42, 0xedffff), (0.6425, 0xffaa00), (0.8575, 0x000200)], cosine: None },
    Palette { name: "Ember", stops: &[(0.0, 0x050000), (0.22, 0x4a0c08), (0.45, 0xb5300b), (0.68, 0xff9d1e), (0.86, 0xfff3c4)], cosine: None },
    Palette { name: "Ocean", stops: &[(0.0, 0x001018), (0.25, 0x00507a), (0.5, 0x2fb4c8), (0.7, 0xd8fbff), (0.85, 0x0a3550)], cosine: None },
    Palette { name: "Aurora", stops: &[(0.0, 0x050014), (0.25, 0x1b7f5a), (0.45, 0x6ef0a0), (0.7, 0x3a2a8c), (0.85, 0xc060ff)], cosine: None },
    Palette { name: "Sunset", stops: &[(0.0, 0x12002a), (0.25, 0x6a0d5c), (0.5, 0xe0446a), (0.72, 0xffb056), (0.88, 0xfff1c2)], cosine: None },
    Palette { name: "Glacier", stops: &[(0.0, 0x000814), (0.3, 0x0a2a5a), (0.55, 0x6aa6e0), (0.75, 0xffffff), (0.9, 0x8ab8e8)], cosine: None },
    Palette { name: "Neon", stops: &[(0.0, 0x0b0014), (0.25, 0xff00a8), (0.5, 0x00e5ff), (0.75, 0xf6ff00), (0.9, 0x7a00ff)], cosine: None },
    Palette { name: "Moss", stops: &[(0.0, 0x440154), (0.25, 0x3b528b), (0.5, 0x21918c), (0.75, 0x5ec962), (0.9, 0xfde725)], cosine: None },
    Palette { name: "Spectrum", stops: &[(0.0, 0xff2020), (0.166, 0xffd020), (0.333, 0x30e040), (0.5, 0x20d0ff), (0.666, 0x3040ff), (0.833, 0xe020ff)], cosine: None },
    Palette { name: "Rose", stops: &[(0.0, 0x1a0010), (0.3, 0x7a1f3d), (0.55, 0xe8879c), (0.75, 0xfde2e4), (0.9, 0xa03050)], cosine: None },
    Palette { name: "Copper", stops: &[(0.0, 0x0a0503), (0.3, 0x5c2c14), (0.55, 0xc87533), (0.75, 0xf5c99b), (0.9, 0x3a1a0c)], cosine: None },
    Palette { name: "Ink", stops: &[(0.0, 0x000000), (0.5, 0xf4f1ea)], cosine: None },
];

fn unpack(c: u32) -> [f32; 3] {
    [((c >> 16) & 255) as f32, ((c >> 8) & 255) as f32, (c & 255) as f32]
}

/// One palette row: the cosine, or smoothstep blends between the stops,
/// wrapping around.
pub fn bake_row(p: &Palette, out: &mut [u32]) {
    if let Some([a, b, c, d]) = p.cosine {
        for (i, texel) in out.iter_mut().enumerate().take(PALETTE_WIDTH) {
            let t = i as f32 / PALETTE_WIDTH as f32;
            let mut rgb = [0u32; 3];
            for ch in 0..3 {
                let v = a[ch] + b[ch] * (std::f32::consts::TAU * (c[ch] * t + d[ch])).cos();
                rgb[ch] = (v.clamp(0.0, 1.0) * 255.0).round() as u32;
            }
            *texel = 0xff00_0000 | (rgb[0] << 16) | (rgb[1] << 8) | rgb[2];
        }
        return;
    }
    let n = p.stops.len();
    for (i, texel) in out.iter_mut().enumerate().take(PALETTE_WIDTH) {
        let t = i as f32 / PALETTE_WIDTH as f32;
        // the stop at or before t (wrapping to the last stop before the first)
        let mut k = n - 1;
        for (j, s) in p.stops.iter().enumerate() {
            if s.0 <= t {
                k = j;
            }
        }
        let (p0, c0) = p.stops[k];
        let (p1, c1) = p.stops[(k + 1) % n];
        let p1 = if p1 <= p0 { p1 + 1.0 } else { p1 };
        let tt = if t < p0 { t + 1.0 } else { t };
        let f = ((tt - p0) / (p1 - p0)).clamp(0.0, 1.0);
        let f = f * f * (3.0 - 2.0 * f);
        let (a, b) = (unpack(c0), unpack(c1));
        let mut rgb = [0u32; 3];
        for ch in 0..3 {
            rgb[ch] = (a[ch] + (b[ch] - a[ch]) * f).round().clamp(0.0, 255.0) as u32;
        }
        *texel = 0xff00_0000 | (rgb[0] << 16) | (rgb[1] << 8) | rgb[2];
    }
}

/// All palettes, one row each.
pub fn palette_atlas(cx: &mut Cx) -> Texture {
    let mut data = vec![0u32; PALETTE_WIDTH * PALETTES.len()];
    for (row, p) in PALETTES.iter().enumerate() {
        bake_row(p, &mut data[row * PALETTE_WIDTH..(row + 1) * PALETTE_WIDTH]);
    }
    Texture::new_with_format(
        cx,
        TextureFormat::VecBGRAu8_32 {
            width: PALETTE_WIDTH,
            height: PALETTES.len(),
            data: Some(data),
            updated: TextureUpdated::Full,
        },
    )
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawSwatch::script_shader(vm)){
        ..mod.draw.DrawQuad
        atlas: texture_2d(float)
        pixel: fn() {
            let p = self.pos * self.rect_size
            let sdf = Sdf2d.viewport(p)
            sdf.box(1.5, 1.5, self.rect_size.x - 3.0, self.rect_size.y - 3.0, 5.0)
            let u = (floor(clamp(self.pos.x, 0.0, 0.999) * 256.0) + 0.5) / 256.0
            let v = (self.row + 0.5) / self.rows
            let col = self.atlas.sample_nearest(vec2(u, v))
            sdf.fill_keep(vec4(col.xyz, 1.0))
            let ring = mix(vec4(1.0, 1.0, 1.0, 0.10 + 0.25 * self.hover), vec4(1.0, 1.0, 1.0, 0.95), self.selected)
            sdf.stroke(ring, 1.0 + self.selected * 1.0)
            return sdf.result
        }
    }

    mod.widgets.PaletteGridBase = #(PaletteGrid::register_widget(vm))
    mod.widgets.PaletteGrid = set_type_default() do mod.widgets.PaletteGridBase{
        width: Fill
        height: Fit
        draw_text +: {
            color: #xffffffcc
            text_style: theme.font_regular{ font_size: 8 }
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSwatch {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    row: f32,
    #[live]
    rows: f32,
    #[live]
    selected: f32,
    #[live]
    hover: f32,
}

#[derive(Clone, Debug, Default)]
pub enum PaletteGridAction {
    Selected(usize),
    #[default]
    None,
}

/// A grid of palette swatches with their names; click to pick one.
#[derive(Script, ScriptHook, Widget)]
pub struct PaletteGrid {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[live]
    draw_swatch: DrawSwatch,
    #[live]
    draw_text: DrawText,
    /// The whole grid: the hit area (the swatches are separate draws).
    #[redraw]
    #[rust]
    area: Area,
    #[rust]
    atlas: Option<Texture>,
    #[rust]
    pub selected: usize,
    #[rust]
    hover: Option<usize>,
    /// Swatch cells relative to the grid's top-left.
    #[rust]
    cells: Vec<Rect>,
}

const COLUMNS: usize = 3;
const CELL_H: f64 = 38.0;
const SWATCH_H: f64 = 22.0;
const GAP: f64 = 6.0;

impl PaletteGrid {
    pub fn set_atlas(&mut self, cx: &mut Cx, atlas: Texture) {
        self.atlas = Some(atlas);
        self.area.redraw(cx);
    }

    /// The swatch under a point relative to the grid's top-left (events
    /// come in window coordinates, the grid may draw in a scrolled list).
    fn cell_at(&self, at: Vec2d) -> Option<usize> {
        self.cells.iter().position(|r| r.contains(at))
    }
}

impl Widget for PaletteGrid {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                cx.set_cursor(MouseCursor::Hand);
                let h = self.cell_at(fe.abs - fe.rect.pos);
                if h != self.hover {
                    self.hover = h;
                    self.area.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.hover = None;
                self.area.redraw(cx);
            }
            Hit::FingerDown(fe) => {
                if let Some(i) = self.cell_at(fe.abs - fe.rect.pos) {
                    self.selected = i;
                    cx.widget_action(self.uid, PaletteGridAction::Selected(i));
                    self.area.redraw(cx);
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rows = PALETTES.len().div_ceil(COLUMNS);
        let height = rows as f64 * CELL_H;
        let walk = Walk { height: Size::Fixed(height), ..walk };
        cx.begin_turtle(walk, Layout::default());
        let rect = cx.turtle().rect();
        let Some(atlas) = self.atlas.clone() else {
            cx.end_turtle_with_area(&mut self.area);
            return DrawStep::done();
        };
        self.draw_swatch.draw_vars.set_texture(0, &atlas);
        self.draw_swatch.rows = PALETTES.len() as f32;
        let cell_w = (rect.size.x - GAP * (COLUMNS as f64 - 1.0)) / COLUMNS as f64;
        self.cells.clear();
        for (i, p) in PALETTES.iter().enumerate() {
            let (col, row) = (i % COLUMNS, i / COLUMNS);
            let pos = rect.pos + dvec2(col as f64 * (cell_w + GAP), row as f64 * CELL_H);
            let cell = Rect { pos: pos - rect.pos, size: dvec2(cell_w, CELL_H) };
            self.cells.push(cell);
            self.draw_swatch.row = i as f32;
            self.draw_swatch.selected = if i == self.selected { 1.0 } else { 0.0 };
            self.draw_swatch.hover = if Some(i) == self.hover { 1.0 } else { 0.0 };
            self.draw_swatch.draw_abs(cx, Rect { pos, size: dvec2(cell_w, SWATCH_H) });
            self.draw_text.draw_abs(cx, pos + dvec2(2.0, SWATCH_H + 3.0), p.name);
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}
