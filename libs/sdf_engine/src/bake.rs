//! The GPU brick bake: one quad per brick into the truth atlas (R32F) with the
//! material slot atlas (R8) as a second attachment, each texel one lattice
//! sample evaluated by the program interpreter below. The shader is the twin
//! of [`crate::twin::eval`] written op for op; with `precise_math` the atlas
//! equals the CPU bake bit for bit.
//!
//! The pass keeps its texels between frames. Each frame takes at most
//! `budget` bricks from the queue and records them once; the next batch waits
//! until the GPU painted that one whole (`DrawPass::painted_whole_since`), so
//! a draw left out while a pipeline compiles is painted from the same
//! recording and nothing is lost. A recording only writes its bricks from the
//! program data, so running it again (every pipeline that becomes ready
//! repaints all passes) writes the same texels.
//!
//! SHADER LAW: unary minus has the lowest precedence in the script shader
//! parser; write `(0.0 - a) - b`, never `-a - b`.

use crate::volume::{Brick, Volume, ATLAS_SLOTS_X, ATLAS_W, BS};
use std::collections::BTreeSet;

/// Render-copy slots per row: a slot is 81 x 9 texels (9 z slices of 9 x 9).
pub const COPY_SLOTS_X: usize = 50;
pub const COPY_W: usize = COPY_SLOTS_X * 81;
/// Index texture width (texels).
pub const INDEX_W: usize = 1024;
use makepad_draw::*;

/// Program texture width (texels).
pub const PROG_W: usize = 1024;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // The program interpreter (`twin::eval` op for op), shared by the bake
    // and the march: `eval_program(base, p)` = (distance, slot) of the
    // program whose header texel is `base` at the object-frame point p.
    mod.draw.SdfInterp = {
        prog_tex: texture_2d(float)
        prog_inv: uniform(vec2(1.0, 1.0))
        fetch: fn(i: f32) -> vec4 {
            let row = floor(i * 0.0009765625)
            let col = i - row * 1024.0
            return self.prog_tex.sample_nearest((vec2(col, row) + vec2(0.5, 0.5)) * self.prog_inv)
        }
        len2: fn(x: f32, y: f32) -> f32 {
            return sqrt(x * x + y * y)
        }
        len3: fn(x: f32, y: f32, z: f32) -> f32 {
            return sqrt((x * x + y * y) + z * z)
        }
        clamp01: fn(x: f32) -> f32 {
            return min(max(x, 0.0), 1.0)
        }
        // sin and cos (twin::psin_pcos)
        psc: fn(x: f32) -> vec2 {
            let q = floor(x * 0.6366198 + 0.5)
            let r = (x - q * 1.5707964) - q * (0.0 - 0.00000004371139)
            let r2 = r * r
            let s = r + r * r2 * ((0.0 - 0.16666667) + r2 * (0.008333331 + r2 * ((0.0 - 0.00019840874) + r2 * 0.0000027525562)))
            let c = 1.0 + r2 * ((0.0 - 0.5) + r2 * (0.041666638 + r2 * ((0.0 - 0.0013888378) + r2 * 0.000024760495)))
            let n = q - floor(q * 0.25) * 4.0
            if n < 0.5 { return vec2(s, c) }
            if n < 1.5 { return vec2(c, 0.0 - s) }
            if n < 2.5 { return vec2(0.0 - s, 0.0 - c) }
            return vec2(0.0 - c, s)
        }
        smin: fn(a: f32, b: f32, k: f32) -> f32 {
            if k <= 0.0 { return min(a, b) }
            let h = self.clamp01(0.5 + (0.5 * (b - a)) / k)
            return (b + (a - b) * h) - (k * h) * (1.0 - h)
        }
        prim: fn(pc: f32, px: f32, py: f32, pz: f32) -> f32 {
            let t0 = self.fetch(pc)
            let t1 = self.fetch(pc + 1.0)
            let t2 = self.fetch(pc + 2.0)
            let kind = t0.y
            let a = t0.z
            let b = t0.w
            let c = t1.x
            let d = t1.y
            if kind < 0.5 {
                return self.len3(px, py, pz) - a
            }
            if kind < 1.5 {
                let qx = abs(px) - a
                let qy = abs(py) - b
                let qz = abs(pz) - c
                return self.len3(max(qx, 0.0), max(qy, 0.0), max(qz, 0.0)) + min(max(qx, max(qy, qz)), 0.0) - d
            }
            if kind < 2.5 {
                let qx = self.len2(px, pz)
                let dx = qx - a
                let dy = abs(py) - b
                let mx = max(dx, 0.0)
                let my = max(dy, 0.0)
                return sqrt(mx * mx + my * my) + min(max(dx, dy), 0.0)
            }
            if kind < 3.5 {
                let qx = self.len2(px, pz)
                let k2x = b - a
                let k2y = c * 2.0
                var rr = b
                if py < 0.0 { rr = a }
                let cax = qx - min(qx, rr)
                let cay = abs(py) - c
                let t = self.clamp01(((b - qx) * k2x + (c - py) * k2y) / (k2x * k2x + k2y * k2y))
                let cbx = (qx - b) + k2x * t
                let cby = (py - c) + k2y * t
                var s = 1.0
                if cbx < 0.0 && cay < 0.0 { s = 0.0 - 1.0 }
                return s * sqrt(min(cax * cax + cay * cay, cbx * cbx + cby * cby))
            }
            if kind < 4.5 {
                let q = self.len2(px, pz) - a
                return self.len2(q, py) - b
            }
            if kind < 5.5 {
                let pax = px - a
                let pay = py - b
                let paz = pz - c
                let bx = d
                let by = t1.z
                let bz = t1.w
                let h = self.clamp01(((pax * bx + pay * by) + paz * bz) / ((bx * bx + by * by) + bz * bz))
                return self.len3(pax - bx * h, pay - by * h, paz - bz * h) - t2.x
            }
            if kind < 6.5 {
                let k0 = self.len3(px / a, py / b, pz / c)
                let k1 = self.len3(px / (a * a), py / (b * b), pz / (c * c))
                return (k0 * (k0 - 1.0)) / max(k1, 0.000000000001)
            }
            let ex = self.psc(px * a)
            let ey = self.psc(py * a)
            let ez = self.psc(pz * a)
            let gv = (ex.x * ey.y + ey.x * ez.y) + ez.x * ex.y
            return abs(gv) / a - b
        }

        eval_program: fn(base: f32, x: f32, y: f32, z: f32) -> vec2 {
            var px = x
            var py = y
            var pz = z
            var vs = array(0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f)
            var ss = array(0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f)
            var sx8 = array(0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f)
            var sy8 = array(0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f)
            var sz8 = array(0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f)
            var ms = array(0f, 0f, 0f, 0f, 0f, 0f, 0f, 0f)
            var vn = 0i
            var pn = 0i
            var mn = 0i
            let len = self.fetch(base).x
            var pc = 1.0
            loop {
                if pc >= len { break }
                let h = self.fetch(base + pc)
                let code = h.x
                if code < 0.5 {
                    vs[vn] = self.prim(base + pc, px, py, pz)
                    if mn > 0i { ss[vn] = ms[mn - 1i] } else { ss[vn] = 0.0 }
                    vn = vn + 1i
                    pc = pc + 3.0
                } else if code < 1.5 {
                    sx8[pn] = px
                    sy8[pn] = py
                    sz8[pn] = pz
                    pn = pn + 1i
                    let r0 = self.fetch(base + pc + 1.0)
                    let r1 = self.fetch(base + pc + 2.0)
                    let r2 = self.fetch(base + pc + 3.0)
                    let nx = ((r0.x * px + r0.y * py) + r0.z * pz) + r0.w
                    let ny = ((r1.x * px + r1.y * py) + r1.z * pz) + r1.w
                    let nz = ((r2.x * px + r2.y * py) + r2.z * pz) + r2.w
                    px = nx
                    py = ny
                    pz = nz
                    pc = pc + 4.0
                } else if code < 2.5 {
                    pn = pn - 1i
                    px = sx8[pn]
                    py = sy8[pn]
                    pz = sz8[pn]
                    vs[vn - 1i] = vs[vn - 1i] * h.y
                    pc = pc + 1.0
                } else if code < 3.5 {
                    sx8[pn] = px
                    sy8[pn] = py
                    sz8[pn] = pz
                    pn = pn + 1i
                    let x = px
                    let y = py
                    let z = pz
                    if h.y < 0.5 {
                        let e = self.psc(h.z * y)
                        px = e.y * x + e.x * z
                        pz = e.y * z - e.x * x
                    } else if h.y < 1.5 {
                        let e = self.psc(h.z * x)
                        px = e.y * x + e.x * y
                        py = e.y * y - e.x * x
                    } else if h.y < 2.5 {
                        let k = 1.0 / max(1.0 + h.z * y, 0.001)
                        px = x * k
                        pz = z * k
                    } else if h.w < 0.5 {
                        px = abs(x)
                    } else if h.w < 1.5 {
                        py = abs(y)
                    } else {
                        pz = abs(z)
                    }
                    pc = pc + 1.0
                } else if code < 4.5 {
                    pn = pn - 1i
                    px = sx8[pn]
                    py = sy8[pn]
                    pz = sz8[pn]
                    vs[vn - 1i] = vs[vn - 1i] / h.y
                    pc = pc + 1.0
                } else if code < 5.5 {
                    let e = vs[vn - 1i]
                    let se = ss[vn - 1i]
                    let d = vs[vn - 2i]
                    let sd = ss[vn - 2i]
                    vn = vn - 1i
                    var v = 0.0
                    var s = sd
                    if h.y < 0.5 {
                        v = self.smin(d, e, h.z)
                        if e < d { s = se }
                    } else if h.y < 1.5 {
                        v = 0.0 - self.smin(0.0 - d, e, h.z)
                    } else {
                        v = 0.0 - self.smin(0.0 - d, 0.0 - e, h.z)
                        if e > d { s = se }
                    }
                    vs[vn - 1i] = v
                    ss[vn - 1i] = s
                    pc = pc + 1.0
                } else if code < 6.5 {
                    vs[vn - 1i] = abs(vs[vn - 1i]) - h.y
                    pc = pc + 1.0
                } else if code < 7.5 {
                    vs[vn - 1i] = vs[vn - 1i] - h.y
                    pc = pc + 1.0
                } else if code < 8.5 {
                    ms[mn] = h.y
                    mn = mn + 1i
                    pc = pc + 1.0
                } else {
                    mn = mn - 1i
                    pc = pc + 1.0
                }
            }
            // + 0.0: a zero leaves as +0 (min/max of signed zeros differ between GPUs)
            return vec2(vs[0], ss[0])
        }
    }

    mod.draw.SdfBake = mod.std.set_type_default() do #(DrawSdfBake::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SdfInterp,
        alpha_blend: false
        precise_math: true
        color_format: @Rf32
        fb1: fragment_output(1, vec4f)
        cell: uniform(1.0)
        band: uniform(1.0)

        fragment: fn() {
            let pix = floor(self.world.xy)
            let lx = pix.x - self.rect_pos.x
            let ly = pix.y - self.rect_pos.y
            let sz = floor(lx * 0.125)
            let sx = lx - sz * 8.0
            let e = self.eval_program(self.brick.w, (self.brick.x * 8.0 + sx) * self.cell, (self.brick.y * 8.0 + ly) * self.cell, (self.brick.z * 8.0 + sz) * self.cell)
            // + 0.0: a zero leaves as +0 (min/max of signed zeros differ between GPUs)
            let d = min(max(e.x, 0.0 - self.band), self.band) + 0.0
            self.fb0 = vec4(d, 0.0, 0.0, 1.0)
            self.fb1 = vec4(e.y / 255.0, 0.0, 0.0, 1.0)
        }
    }

    // The render copy: a brick's 9^3 samples (its own 8^3 and the first of
    // the next brick on each axis, the apron) into the R16F copy atlas (9 z
    // slices of 9 x 9), with the slots into the R8 copy; bilinear filtering
    // then needs no neighbour lookups. A sample of an empty brick is the band
    // with the brick's sign.
    mod.draw.SdfCopy = mod.std.set_type_default() do #(DrawSdfCopy::script_shader(vm)){
        ..mod.draw.DrawQuad
        alpha_blend: false
        color_format: @Rf16
        fb1: fragment_output(1, vec4f)
        truth: texture_2d(float)
        truth_slots: texture_2d(float)
        index_tex: texture_2d(float)
        truth_inv: uniform(vec2(1.0, 1.0))
        index_inv: uniform(vec2(1.0, 1.0))
        vol_min: uniform(vec3(0.0, 0.0, 0.0))
        vol_dim: uniform(vec3(1.0, 1.0, 1.0))
        band: uniform(1.0)

        // `v` rounded to the nearest f16 (ties to even), as an f32: the
        // R16F write then stores it exactly. Render-target conversion modes
        // differ between GPUs (an RTX 5090 truncates).
        to_half: fn(v: f32) -> f32 {
            let a = abs(v)
            if a < 0.00006103515625 {
                // the f16 subnormals: multiples of 2^-24
                let q = a * 16777216.0
                var r = floor(q)
                let f = q - r
                if f > 0.5 || (f == 0.5 && r - floor(r * 0.5) * 2.0 > 0.5) { r = r + 1.0 }
                let h = r * 0.000000059604644775390625
                if v < 0.0 { return 0.0 - h }
                return h
            }
            let b = asuint(v)
            let odd = (b >> 13u) & 1u
            return asfloat((b + 4095u + odd) & 4294959104u)
        }

        fragment: fn() {
            let pix = floor(self.world.xy)
            let lx = pix.x - self.rect_pos.x
            let ly = pix.y - self.rect_pos.y
            let sz = floor((lx + 0.5) / 9.0)
            let sx = lx - sz * 9.0
            let g = self.brick.xyz * 8.0 + vec3(sx, ly, sz)
            let nb = floor(g * 0.125)
            let local = g - nb * 8.0
            let ib = nb - self.vol_min
            var v = self.band
            var slot_v = 0.0
            if ib.x >= 0.0 && ib.y >= 0.0 && ib.z >= 0.0 && ib.x < self.vol_dim.x && ib.y < self.vol_dim.y && ib.z < self.vol_dim.z {
                let i = (ib.z * self.vol_dim.y + ib.y) * self.vol_dim.x + ib.x
                let row = floor(i * 0.0009765625)
                let e = self.index_tex.sample_nearest((vec2(i - row * 1024.0, row) + vec2(0.5, 0.5)) * self.index_inv)
                if e.x >= 0.0 {
                    let srow = floor(e.x * 0.015625)
                    let scol = e.x - srow * 64.0
                    let t = vec2(scol * 64.0 + local.z * 8.0 + local.x, srow * 8.0 + local.y) + vec2(0.5, 0.5)
                    v = self.truth.sample_nearest(t * self.truth_inv).x
                    slot_v = self.truth_slots.sample_nearest(t * self.truth_inv).x
                } else if e.y < 0.0 {
                    v = 0.0 - self.band
                }
            }
            self.fb0 = vec4(self.to_half(v), 0.0, 0.0, 1.0)
            self.fb1 = vec4(slot_v, 0.0, 0.0, 1.0)
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSdfCopy {
    #[deref]
    pub draw_super: DrawQuad,
    /// The brick (x y z).
    #[live]
    pub brick: Vec4f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSdfBake {
    #[deref]
    pub draw_super: DrawQuad,
    /// The brick (x y z) and its program's first texel (w).
    #[live]
    pub brick: Vec4f,
}

/// The atlases one volume bakes into and the pass that writes them.
pub struct Baker {
    pass: DrawPass,
    list: DrawList,
    copy_pass: DrawPass,
    copy_list: DrawList,
    /// The render copy: R16F distances and R8 slots with the apron, `COPY_W`
    /// wide, `copy_rows` high.
    pub copy: Texture,
    pub copy_slots: Texture,
    pub copy_rows: usize,
    index_tex: Texture,
    index_rows: usize,
    /// Baked bricks whose render copy must be redone.
    copy_queue: BTreeSet<usize>,
    /// Truth (R32F) and slots (R8), `ATLAS_W` wide, `rows` high.
    pub atlas: Texture,
    pub slots: Texture,
    pub rows: usize,
    prog_tex: Texture,
    prog_rows: usize,
    /// The first texel of each program in `prog_tex`.
    prog_at: Vec<u32>,
    /// Bricks still to bake, and the batch in flight with the repaint it was
    /// recorded at.
    queue: Vec<usize>,
    inflight: Option<(u64, Vec<usize>)>,
    /// Bricks recorded per frame at most.
    pub budget: usize,
    /// Batches the GPU has finished (bricks), for the caller's progress.
    pub baked: usize,
    /// The passes' current recording draws nothing.
    recorded_empty: bool,
}

impl Baker {
    pub fn new(cx: &mut Cx) -> Baker {
        let rows = 8;
        Baker {
            pass: DrawPass::new_with_name(cx, "sdf_bake"),
            list: DrawList::new(cx),
            copy_pass: DrawPass::new_with_name(cx, "sdf_copy"),
            copy_list: DrawList::new(cx),
            copy: Texture::new_with_format(cx, TextureFormat::RenderRf16 { size: TextureSize::Fixed { width: COPY_W, height: 9 }, initial: true }),
            copy_slots: Texture::new_with_format(cx, TextureFormat::RenderRu8 { size: TextureSize::Fixed { width: COPY_W, height: 9 }, initial: true }),
            copy_rows: 9,
            index_tex: Texture::new_with_format(cx, TextureFormat::VecRGBAf32 { width: INDEX_W, height: 1, data: Some(vec![0.0; INDEX_W * 4]), updated: TextureUpdated::Full }),
            index_rows: 1,
            copy_queue: BTreeSet::new(),
            atlas: Texture::new_with_format(cx, TextureFormat::RenderRf32 { size: TextureSize::Fixed { width: ATLAS_W, height: rows }, initial: true }),
            slots: Texture::new_with_format(cx, TextureFormat::RenderRu8 { size: TextureSize::Fixed { width: ATLAS_W, height: rows }, initial: true }),
            rows,
            prog_tex: Texture::new_with_format(cx, TextureFormat::VecRGBAf32 { width: PROG_W, height: 1, data: Some(vec![0.0; PROG_W * 4]), updated: TextureUpdated::Full }),
            prog_rows: 1,
            prog_at: Vec::new(),
            queue: Vec::new(),
            inflight: None,
            budget: 8192,
            baked: 0,
            recorded_empty: false,
        }
    }

    /// Uploads the volume's programs and queues its dirty bricks; grows the
    /// atlases (and re-queues every baked brick) when its slots outgrow them.
    pub fn sync(&mut self, cx: &mut Cx, vol: &mut Volume) {
        // programs: one texture, rebuilt when the volume has new ones
        if self.prog_at.len() != vol.programs.len() {
            let mut data: Vec<f32> = Vec::new();
            self.prog_at.clear();
            for p in &vol.programs {
                self.prog_at.push((data.len() / 4) as u32);
                data.extend(p.texels.iter().flatten());
            }
            let rows = (data.len() / 4).div_ceil(PROG_W).max(1);
            data.resize(rows * PROG_W * 4, 0.0);
            self.prog_rows = rows;
            self.prog_tex = Texture::new_with_format(cx, TextureFormat::VecRGBAf32 { width: PROG_W, height: rows, data: Some(data), updated: TextureUpdated::Full });
        }
        let need_rows = (vol.slot_count as usize).div_ceil(ATLAS_SLOTS_X).max(1) * BS as usize;
        if need_rows > self.rows {
            let rows = need_rows.next_power_of_two();
            self.rows = rows;
            self.atlas = Texture::new_with_format(cx, TextureFormat::RenderRf32 { size: TextureSize::Fixed { width: ATLAS_W, height: rows }, initial: true });
            self.slots = Texture::new_with_format(cx, TextureFormat::RenderRu8 { size: TextureSize::Fixed { width: ATLAS_W, height: rows }, initial: true });
            self.queue.clear();
            self.inflight = None;
            self.recorded_empty = false;
            self.queue.extend((0..vol.bricks.len()).filter(|i| matches!(vol.bricks[*i], Brick::Baked { .. }) && !vol.cpu_only.contains(i)));
            vol.dirty.clear();
        }
        self.queue.append(&mut vol.dirty);
        let need_copy = (vol.slot_count as usize).div_ceil(COPY_SLOTS_X).max(1) * 9;
        if need_copy > self.copy_rows {
            self.copy_rows = need_copy.next_power_of_two();
            self.recorded_empty = false;
            self.copy = Texture::new_with_format(cx, TextureFormat::RenderRf16 { size: TextureSize::Fixed { width: COPY_W, height: self.copy_rows }, initial: true });
            self.copy_slots = Texture::new_with_format(cx, TextureFormat::RenderRu8 { size: TextureSize::Fixed { width: COPY_W, height: self.copy_rows }, initial: true });
            self.copy_queue.extend((0..vol.bricks.len()).filter(|i| matches!(vol.bricks[*i], Brick::Baked { .. })));
        }
        // the neighbours of everything re-classified re-copy their aprons
        for i in std::mem::take(&mut vol.touched) {
            self.copy_queue.extend(vol.around(i));
        }
        // the index, whole (a texel per brick)
        // (slot, skip, the brick's program's first texel, 0)
        let mut texels = vol.index_texels();
        // (-1: no program, the brick's texels are data: sculpted or uploaded)
        for (t, b) in texels.iter_mut().zip(&vol.bricks) {
            t[2] = match b {
                Brick::Baked { program, .. } => self.prog_at.get(*program as usize).copied().unwrap_or(0) as f32,
                _ => -1.0,
            };
        }
        let mut data: Vec<f32> = texels.into_iter().flatten().collect();
        let rows = (data.len() / 4).div_ceil(INDEX_W).max(1);
        data.resize(rows * INDEX_W * 4, 0.0);
        self.index_rows = rows;
        self.index_tex = Texture::new_with_format(cx, TextureFormat::VecRGBAf32 { width: INDEX_W, height: rows, data: Some(data), updated: TextureUpdated::Full });
    }

    /// Forgets the queue and the uploaded programs (a new volume into the same atlases).
    pub fn reset(&mut self) {
        self.queue.clear();
        self.copy_queue.clear();
        self.inflight = None;
        self.recorded_empty = false;
        self.prog_at.clear();
    }

    pub fn prog_texture(&self) -> &Texture {
        &self.prog_tex
    }
    pub fn prog_rows(&self) -> usize {
        self.prog_rows
    }
    pub fn index_texture(&self) -> &Texture {
        &self.index_tex
    }
    pub fn index_rows(&self) -> usize {
        self.index_rows
    }

    /// Whether everything queued has been baked.
    pub fn idle(&self) -> bool {
        self.queue.is_empty() && self.copy_queue.is_empty() && self.inflight.is_none()
    }

    /// The passes for this frame: wait on the batch in flight, else record
    /// the next one: its bricks into the truth atlases, then (a parent pass,
    /// painted after) the render copies of those bricks and their neighbours.
    /// The caller samples `copy` (or `atlas`), so the passes are painted.
    pub fn draw(&mut self, cx: &mut Cx2d, vol: &Volume, bake: &mut DrawSdfBake, copy: &mut DrawSdfCopy) {
        let copy_id = self.copy_pass.draw_pass_id();
        if let Some((serial, batch)) = &self.inflight {
            cx.make_child_pass(&self.copy_pass);
            self.pass.set_pass_parent(cx.cx, copy_id);
            if !(self.pass.painted_whole_since(cx.cx, *serial) && self.copy_pass.painted_whole_since(cx.cx, *serial)) {
                return;
            }
            self.baked += batch.len();
            self.inflight = None;
        }
        let n = self.queue.len().min(self.budget);
        let batch: Vec<usize> = self.queue.drain(..n).collect();
        let mut copies: BTreeSet<usize> = std::mem::take(&mut self.copy_queue);
        for &i in &batch {
            copies.extend(vol.around(i));
        }
        copies.retain(|i| matches!(vol.bricks[*i], Brick::Baked { .. }));
        // Nothing to do: the passes keep their last recording, which is empty
        // (recorded once after the last batch), so repaints write nothing and
        // whatever samples the atlases does not repaint every frame.
        if batch.is_empty() && copies.is_empty() && self.recorded_empty {
            cx.make_child_pass(&self.copy_pass);
            self.pass.set_pass_parent(cx.cx, copy_id);
            return;
        }
        self.recorded_empty = batch.is_empty() && copies.is_empty();

        // the bake
        let size = dvec2(ATLAS_W as f64, self.rows as f64);
        self.pass.set_size(cx, size);
        self.pass.clear_color_textures(cx.cx);
        self.pass.set_color_texture(cx, &self.atlas, DrawPassClearColor::InitWith(vec4(vol.band, 0.0, 0.0, 1.0)));
        self.pass.add_color_texture(cx, &self.slots, DrawPassClearColor::InitWith(vec4(0.0, 0.0, 0.0, 0.0)));
        self.pass.clear_depth_texture(cx.cx);
        self.pass.set_pass_parent(cx.cx, copy_id);
        cx.begin_pass(&self.pass, Some(1.0));
        self.pass.set_size(cx, size);
        self.pass.set_dpi_factor(cx, 1.0);
        self.list.begin_always(cx);
        cx.begin_root_turtle(size, Layout::flow_overlay());
        {
            let c = &*cx.cx;
            let dv = &mut bake.draw_super.draw_vars;
            dv.set_texture(0, &self.prog_tex);
            dv.set_uniform(c, id!(prog_inv), &[1.0 / PROG_W as f32, 1.0 / self.prog_rows as f32]);
            dv.set_uniform(c, id!(cell), &[vol.cell]);
            dv.set_uniform(c, id!(band), &[vol.band]);
        }
        for &i in &batch {
            let Brick::Baked { slot, program } = vol.bricks[i] else { continue };
            let b = vol.brick_of(i);
            bake.brick = vec4(b[0] as f32, b[1] as f32, b[2] as f32, self.prog_at[program as usize] as f32);
            bake.draw_super.draw_abs(cx, slot_rect(slot));
        }
        cx.end_pass_sized_turtle();
        self.list.end(cx);
        cx.end_pass(&self.pass);

        // the render copy (samples the truth atlases the bake just wrote)
        let size = dvec2(COPY_W as f64, self.copy_rows as f64);
        self.copy_pass.set_size(cx, size);
        self.copy_pass.clear_color_textures(cx.cx);
        self.copy_pass.set_color_texture(cx, &self.copy, DrawPassClearColor::InitWith(vec4(vol.band, 0.0, 0.0, 1.0)));
        self.copy_pass.add_color_texture(cx, &self.copy_slots, DrawPassClearColor::InitWith(vec4(0.0, 0.0, 0.0, 0.0)));
        self.copy_pass.clear_depth_texture(cx.cx);
        cx.make_child_pass(&self.copy_pass);
        cx.begin_pass(&self.copy_pass, Some(1.0));
        self.copy_pass.set_size(cx, size);
        self.copy_pass.set_dpi_factor(cx, 1.0);
        self.copy_list.begin_always(cx);
        cx.begin_root_turtle(size, Layout::flow_overlay());
        {
            let c = &*cx.cx;
            let dv = &mut copy.draw_super.draw_vars;
            dv.set_texture(0, &self.atlas);
            dv.set_texture(1, &self.slots);
            dv.set_texture(2, &self.index_tex);
            dv.set_uniform(c, id!(truth_inv), &[1.0 / ATLAS_W as f32, 1.0 / self.rows as f32]);
            dv.set_uniform(c, id!(index_inv), &[1.0 / INDEX_W as f32, 1.0 / self.index_rows as f32]);
            dv.set_uniform(c, id!(vol_min), &vol.min.map(|v| v as f32));
            dv.set_uniform(c, id!(vol_dim), &vol.dim.map(|v| v as f32));
            dv.set_uniform(c, id!(band), &[vol.band]);
        }
        for &i in &copies {
            let Brick::Baked { slot, .. } = vol.bricks[i] else { continue };
            let b = vol.brick_of(i);
            copy.brick = vec4(b[0] as f32, b[1] as f32, b[2] as f32, 0.0);
            copy.draw_super.draw_abs(cx, copy_rect(slot));
        }
        cx.end_pass_sized_turtle();
        self.copy_list.end(cx);
        cx.end_pass(&self.copy_pass);
        if !batch.is_empty() || !copies.is_empty() {
            self.inflight = Some((cx.cx.repaint_id(), batch));
        }
    }
}

/// A slot's texel rect in the render copy.
pub fn copy_rect(slot: u32) -> Rect {
    let s = slot as usize;
    Rect { pos: dvec2(((s % COPY_SLOTS_X) * 81) as f64, ((s / COPY_SLOTS_X) * 9) as f64), size: dvec2(81.0, 9.0) }
}

/// A slot's texel rect in the atlas.
pub fn slot_rect(slot: u32) -> Rect {
    let s = slot as usize;
    Rect { pos: dvec2(((s % ATLAS_SLOTS_X) * 64) as f64, ((s / ATLAS_SLOTS_X) * 8) as f64), size: dvec2(64.0, 8.0) }
}
