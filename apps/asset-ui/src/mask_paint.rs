//! Mask painter for the FLUX.1 Fill-dev inpaint/outpaint lane: shows the
//! pinned input picture letterboxed in the viewer and lets the user paint a
//! repaint mask on it (left drag = paint, ⌥/right = erase). Exports the
//! canvas + mask as PNGs in the exact size the service expects (same size,
//! white/255 = repaint). Outpaint = grow the canvas around the picture (edge
//! replicate) and mask the new border, same request shape.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawMaskPaint::script_shader(vm)){
        ..mod.draw.DrawQuad
        image_tex: texture_2d(float)
        mask_tex: texture_2d(float)
        // Letterboxed image rect inside the widget, in 0..1 of rect_size.
        fit_origin: vec2(0.0, 0.0)
        fit_size: vec2(1.0, 1.0)
        // Brush cursor (image uv + radius in uv units of the x axis).
        cursor_uv: vec2(-1.0, -1.0)
        cursor_r: 0.0
        has_image: 0.0

        pixel: fn() {
            let uv = (self.pos - self.fit_origin) / self.fit_size
            let inside = step(0.0, uv.x) * step(uv.x, 1.0) * step(0.0, uv.y) * step(uv.y, 1.0) * self.has_image
            // Checkerboard ground like the image viewer.
            let p = floor(self.pos * self.rect_size / 8.0)
            let check = modf(p.x + p.y, 2.0)
            let board = mix(#x26262b, #x3c3c42, check)
            let color = self.image_tex.sample_as_bgra(uv)
            let m = self.mask_tex.sample_as_bgra(uv).x
            let painted = mix(color.xyz, vec3(1.0, 0.25, 0.2), m * 0.55)
            let base = mix(board.xyz, painted, inside)
            // Brush ring.
            let d = length((uv - self.cursor_uv) * vec2(1.0, self.fit_size.y * self.rect_size.y / (self.fit_size.x * self.rect_size.x)))
            let ring = smoothstep(self.cursor_r * 1.08, self.cursor_r, d) * (1.0 - smoothstep(self.cursor_r, self.cursor_r * 0.92, d))
            let ringed = mix(base, vec3(1.0, 1.0, 1.0), ring * inside * step(0.0, self.cursor_r))
            return vec4(ringed, 1.0)
        }
    }

    mod.widgets.MaskPaintBase = #(MaskPaint::register_widget(vm))
    mod.widgets.MaskPaint = set_type_default() do mod.widgets.MaskPaintBase{
        width: Fill
        height: Fill
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMaskPaint {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    fit_origin: Vec2f,
    #[live]
    fit_size: Vec2f,
    #[live]
    cursor_uv: Vec2f,
    #[live]
    cursor_r: f32,
    #[live]
    has_image: f32,
}

/// Action emitted when the mask changed (so the host can relabel buttons).
#[derive(Clone, Debug, Default)]
pub enum MaskPaintAction {
    MaskChanged,
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct MaskPaint {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[visible]
    #[live(true)]
    visible: bool,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawMaskPaint,
    #[rust]
    area: Area,
    /// Canvas pixels (BGRA u32, makepad order), `width * height`.
    #[rust]
    canvas: Vec<u32>,
    #[rust]
    width: usize,
    #[rust]
    height: usize,
    /// Mask, 0 or 255 per canvas pixel.
    #[rust]
    mask: Vec<u8>,
    #[rust]
    image_texture: Option<Texture>,
    #[rust]
    mask_texture: Option<Texture>,
    #[rust]
    mask_dirty: bool,
    /// Brush radius in canvas pixels.
    #[rust(24.0)]
    brush_radius: f32,
    #[rust]
    painting: Option<bool>,
    #[rust]
    last_pos: Option<(f32, f32)>,
    #[rust]
    hover_uv: Option<(f32, f32)>,
    #[rust]
    fit: (f64, f64, f64, f64),
    /// Widget rect at the last draw (abs), for pointer → canvas mapping.
    #[rust]
    rect: Rect,
}

impl MaskPaint {
    pub fn has_image(&self) -> bool {
        self.width > 0 && self.height > 0
    }

    pub fn has_mask(&self) -> bool {
        self.mask.iter().any(|&m| m > 0)
    }

    pub fn canvas_size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    pub fn brush_radius(&self) -> f32 {
        self.brush_radius
    }

    pub fn set_brush_radius(&mut self, radius: f32) {
        self.brush_radius = radius.clamp(1.0, 512.0);
    }

    /// Paint (or erase) one disc — used by headless smoke hooks; the
    /// interactive path goes through `stroke`.
    pub fn paint_at(&mut self, x: f32, y: f32, radius: f32, paint: bool) {
        paint_disc(&mut self.mask, self.width, self.height, x, y, radius, paint);
        self.mask_dirty = true;
    }

    /// Load a decoded picture as the canvas; the mask resets.
    pub fn set_image(&mut self, cx: &mut Cx, image: &ImageBuffer) {
        self.width = image.width;
        self.height = image.height;
        self.canvas = image.data[..image.width * image.height].to_vec();
        self.mask = vec![0u8; self.width * self.height];
        self.image_texture = None;
        self.mask_texture = None;
        self.mask_dirty = true;
        self.draw_bg.redraw(cx);
    }

    pub fn clear(&mut self, cx: &mut Cx) {
        self.width = 0;
        self.height = 0;
        self.canvas.clear();
        self.mask.clear();
        self.image_texture = None;
        self.mask_texture = None;
        self.draw_bg.redraw(cx);
    }

    pub fn clear_mask(&mut self, cx: &mut Cx) {
        for m in &mut self.mask {
            *m = 0;
        }
        self.mask_dirty = true;
        self.draw_bg.redraw(cx);
    }

    pub fn invert_mask(&mut self, cx: &mut Cx) {
        for m in &mut self.mask {
            *m = 255 - *m;
        }
        self.mask_dirty = true;
        self.draw_bg.redraw(cx);
    }

    /// Outpaint: grow the canvas by `fraction` of each dimension on every
    /// side (edge-replicated fill), rounded so the result is a multiple of
    /// 16, and mask the new border. Existing mask strokes are kept.
    pub fn outpaint(&mut self, cx: &mut Cx, fraction: f32) {
        if !self.has_image() {
            return;
        }
        let pad_x = ((self.width as f32 * fraction).round() as usize).max(16);
        let pad_y = ((self.height as f32 * fraction).round() as usize).max(16);
        let new_w = (self.width + 2 * pad_x).div_ceil(16) * 16;
        let new_h = (self.height + 2 * pad_y).div_ceil(16) * 16;
        let off_x = (new_w - self.width) / 2;
        let off_y = (new_h - self.height) / 2;
        let (canvas, mask) =
            grow_canvas(&self.canvas, &self.mask, self.width, self.height, new_w, new_h, off_x, off_y);
        self.canvas = canvas;
        self.mask = mask;
        self.width = new_w;
        self.height = new_h;
        self.image_texture = None;
        self.mask_texture = None;
        self.mask_dirty = true;
        self.draw_bg.redraw(cx);
    }

    /// The canvas as RGBA8 PNG (what goes to the service as `image`).
    pub fn canvas_png(&self) -> Option<Vec<u8>> {
        if !self.has_image() {
            return None;
        }
        let rgba = bgra_to_rgba8(&self.canvas);
        makepad_ai_hub::testpattern::encode_png_rgba(&rgba, self.width, self.height).ok()
    }

    /// The mask as an opaque gray PNG (white = repaint).
    pub fn mask_png(&self) -> Option<Vec<u8>> {
        if !self.has_image() {
            return None;
        }
        let mut rgba = Vec::with_capacity(self.mask.len() * 4);
        for &m in &self.mask {
            rgba.extend_from_slice(&[m, m, m, 255]);
        }
        makepad_ai_hub::testpattern::encode_png_rgba(&rgba, self.width, self.height).ok()
    }

    fn ensure_textures(&mut self, cx: &mut Cx) {
        if !self.has_image() {
            return;
        }
        if self.image_texture.is_none() {
            self.image_texture = Some(Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: self.width,
                    height: self.height,
                    data: Some(self.canvas.clone()),
                    updated: TextureUpdated::Full,
                },
            ));
        }
        if self.mask_texture.is_none() || self.mask_dirty {
            let data: Vec<u32> = self
                .mask
                .iter()
                .map(|&m| 0xff00_0000 | ((m as u32) << 16) | ((m as u32) << 8) | m as u32)
                .collect();
            match &self.mask_texture {
                Some(texture) => {
                    texture.put_back_vec_u32(cx, data, None);
                }
                None => {
                    self.mask_texture = Some(Texture::new_with_format(
                        cx,
                        TextureFormat::VecBGRAu8_32 {
                            width: self.width,
                            height: self.height,
                            data: Some(data),
                            updated: TextureUpdated::Full,
                        },
                    ));
                }
            }
            self.mask_dirty = false;
        }
    }

    /// Widget-space point → canvas pixel (None outside the picture).
    fn canvas_point(&self, abs: Vec2d) -> Option<(f32, f32)> {
        let rect = self.rect;
        let (ox, oy, sw, sh) = self.fit;
        if sw <= 0.0 || sh <= 0.0 {
            return None;
        }
        let u = (abs.x - rect.pos.x - ox) / sw;
        let v = (abs.y - rect.pos.y - oy) / sh;
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return None;
        }
        Some(((u * self.width as f64) as f32, (v * self.height as f64) as f32))
    }

    fn stroke(&mut self, from: (f32, f32), to: (f32, f32), paint: bool) {
        let steps = (((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt() / (self.brush_radius * 0.35).max(1.0))
            .ceil()
            .max(1.0) as usize;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let x = from.0 + (to.0 - from.0) * t;
            let y = from.1 + (to.1 - from.1) * t;
            paint_disc(&mut self.mask, self.width, self.height, x, y, self.brush_radius, paint);
        }
        self.mask_dirty = true;
    }
}

/// Paint (or erase) a filled disc into `mask`.
pub fn paint_disc(mask: &mut [u8], width: usize, height: usize, cx: f32, cy: f32, radius: f32, paint: bool) {
    let r2 = radius * radius;
    let x0 = ((cx - radius).floor().max(0.0)) as usize;
    let x1 = ((cx + radius).ceil().min(width as f32 - 1.0)).max(0.0) as usize;
    let y0 = ((cy - radius).floor().max(0.0)) as usize;
    let y1 = ((cy + radius).ceil().min(height as f32 - 1.0)).max(0.0) as usize;
    if width == 0 || height == 0 || x0 > x1 || y0 > y1 {
        return;
    }
    let value = if paint { 255 } else { 0 };
    for y in y0..=y1 {
        let dy = y as f32 + 0.5 - cy;
        for x in x0..=x1 {
            let dx = x as f32 + 0.5 - cx;
            if dx * dx + dy * dy <= r2 {
                mask[y * width + x] = value;
            }
        }
    }
}

/// Edge-replicate `canvas` into a `new_w x new_h` frame at `(off_x, off_y)`;
/// the mask keeps its strokes and the whole new border becomes repaint.
pub fn grow_canvas(
    canvas: &[u32],
    mask: &[u8],
    width: usize,
    height: usize,
    new_w: usize,
    new_h: usize,
    off_x: usize,
    off_y: usize,
) -> (Vec<u32>, Vec<u8>) {
    let mut out = vec![0u32; new_w * new_h];
    let mut out_mask = vec![255u8; new_w * new_h];
    for y in 0..new_h {
        let sy = y.saturating_sub(off_y).min(height - 1);
        for x in 0..new_w {
            let sx = x.saturating_sub(off_x).min(width - 1);
            out[y * new_w + x] = canvas[sy * width + sx];
            let inside = x >= off_x && x < off_x + width && y >= off_y && y < off_y + height;
            if inside {
                out_mask[y * new_w + x] = mask[sy * width + sx];
            }
        }
    }
    (out, out_mask)
}

fn bgra_to_rgba8(bgra: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bgra.len() * 4);
    for px in bgra {
        out.extend_from_slice(&[(px >> 16) as u8, (px >> 8) as u8, *px as u8, (px >> 24) as u8]);
    }
    out
}

impl Widget for MaskPaint {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if !self.visible {
            return;
        }
        let uid = self.widget_uid();
        match event.hits(cx, self.area) {
            Hit::FingerDown(fe) => {
                if let Some(p) = self.canvas_point(fe.abs) {
                    let paint = !(fe.modifiers.alt || fe.mouse_button().is_some_and(|b| b.is_secondary()));
                    self.painting = Some(paint);
                    self.stroke(p, p, paint);
                    self.last_pos = Some(p);
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerMove(fe) => {
                let p = self.canvas_point(fe.abs);
                if let (Some(paint), Some(p)) = (self.painting, p) {
                    let from = self.last_pos.unwrap_or(p);
                    self.stroke(from, p, paint);
                    self.last_pos = Some(p);
                }
                self.hover_uv = p.map(|(x, y)| (x / self.width.max(1) as f32, y / self.height.max(1) as f32));
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                self.hover_uv = self
                    .canvas_point(fe.abs)
                    .map(|(x, y)| (x / self.width.max(1) as f32, y / self.height.max(1) as f32));
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.hover_uv = None;
                self.draw_bg.redraw(cx);
            }
            Hit::FingerUp(_) => {
                if self.painting.take().is_some() {
                    cx.widget_action(uid, MaskPaintAction::MaskChanged);
                }
                self.last_pos = None;
                self.draw_bg.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.ensure_textures(cx);
        let rect = cx.peek_walk_turtle(walk);
        self.rect = rect;
        // Letterbox the picture ("Smallest" fit).
        if self.has_image() && rect.size.x > 0.0 && rect.size.y > 0.0 {
            let scale = (rect.size.x / self.width as f64).min(rect.size.y / self.height as f64);
            let sw = self.width as f64 * scale;
            let sh = self.height as f64 * scale;
            let ox = (rect.size.x - sw) * 0.5;
            let oy = (rect.size.y - sh) * 0.5;
            self.fit = (ox, oy, sw, sh);
            self.draw_bg.fit_origin = vec2((ox / rect.size.x) as f32, (oy / rect.size.y) as f32);
            self.draw_bg.fit_size = vec2((sw / rect.size.x) as f32, (sh / rect.size.y) as f32);
            self.draw_bg.has_image = 1.0;
            self.draw_bg.cursor_r = self.brush_radius / self.width as f32;
        } else {
            self.fit = (0.0, 0.0, 0.0, 0.0);
            self.draw_bg.fit_origin = vec2(0.0, 0.0);
            self.draw_bg.fit_size = vec2(1.0, 1.0);
            self.draw_bg.has_image = 0.0;
            self.draw_bg.cursor_r = 0.0;
        }
        self.draw_bg.cursor_uv = match self.hover_uv {
            Some((u, v)) => vec2(u, v),
            None => vec2(-10.0, -10.0),
        };
        match (&self.image_texture, &self.mask_texture) {
            (Some(image), Some(mask)) => {
                self.draw_bg.draw_vars.set_texture(0, image);
                self.draw_bg.draw_vars.set_texture(1, mask);
            }
            _ => {
                self.draw_bg.draw_vars.empty_texture(0);
                self.draw_bg.draw_vars.empty_texture(1);
            }
        }
        self.draw_bg.draw_walk(cx, walk);
        self.area = self.draw_bg.area();
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disc_paint_and_erase_stay_in_bounds() {
        let mut mask = vec![0u8; 8 * 8];
        paint_disc(&mut mask, 8, 8, 4.0, 4.0, 2.0, true);
        assert_eq!(mask[4 * 8 + 4], 255);
        assert_eq!(mask[0], 0);
        // Near the corner: no panic, partial disc.
        paint_disc(&mut mask, 8, 8, 0.0, 0.0, 3.0, true);
        assert_eq!(mask[0], 255);
        paint_disc(&mut mask, 8, 8, 4.0, 4.0, 2.0, false);
        assert_eq!(mask[4 * 8 + 4], 0);
        // Degenerate sizes never index.
        paint_disc(&mut [], 0, 0, 1.0, 1.0, 5.0, true);
    }

    #[test]
    fn grow_canvas_replicates_edges_and_masks_the_border() {
        // 2x1 canvas [A B] grown to 6x3 at offset (2,1).
        let canvas = vec![0xff0000aa, 0xff0000bb];
        let mask = vec![0u8, 255];
        let (out, out_mask) = grow_canvas(&canvas, &mask, 2, 1, 6, 3, 2, 1);
        assert_eq!(out.len(), 18);
        // Left border replicates A, right border replicates B.
        assert_eq!(out[1 * 6 + 0], 0xff0000aa);
        assert_eq!(out[1 * 6 + 5], 0xff0000bb);
        // Rows above/below replicate the single source row.
        assert_eq!(out[0 * 6 + 2], 0xff0000aa);
        assert_eq!(out[2 * 6 + 3], 0xff0000bb);
        // Original mask kept inside, border all repaint.
        assert_eq!(out_mask[1 * 6 + 2], 0);
        assert_eq!(out_mask[1 * 6 + 3], 255);
        assert_eq!(out_mask[0], 255);
        assert_eq!(out_mask[17], 255);
    }
}
