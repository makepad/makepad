//! The dense room, its atlas uploads, and the CPU inputs recorded with each GPU capture.
use super::*;
use makepad_render::traced_volume::{build_coarse, patch_coarse, TracedVolumeTile};

const DIMS: [u32; 3] = [24, 16, 16];
const PITCH: f32 = 0.1;
const TILE_CELLS: u32 = 16;
const EDIT: [u32; 3] = [8, 8, 8];
const PILLAR: u8 = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum VolumeCamera { Outside, Inside, Slice }

pub(super) struct VolumeFixture {
    tiles: Vec<TracedVolumeTile>,
    cells: Vec<u8>,
    coarse: Vec<u8>,
    textures: Option<[Texture; 2]>,
    cleared: bool,
    pub(super) subject: ModelInstance,
    pub(super) subject_no_inside: ModelInstance,
    cube: ModelInstance,
}

impl VolumeFixture {
    pub(super) fn install(r: &mut Renderer, cx: &mut Cx, series: bool) -> (Self, TracedSurfaceRecipe) {
        let mut cells = Vec::new();
        for z in 0..16 {
            for y in 0..16 {
                for x in 0..24 {
                    let jamb = !(2..20).contains(&x) || y >= 14;
                    let pillar = (8..12).contains(&x) && (8..12).contains(&z)
                        || (16..19).contains(&x) && (3..6).contains(&z);
                    cells.push(if y == 0 { 4 } else if y == 15 { 6 }
                        else if x == 0 || x == 23 { 5 }
                        else if z == 15 && jamb { 1 } else if z == 0 && jamb { 2 }
                        else if pillar { PILLAR } else { 0 });
                }
            }
        }
        let coarse = build_coarse(&cells, DIMS);
        let mut recipe = TracedSurfaceRecipe::volume(PITCH, TILE_CELLS);
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut tiles = Vec::new();
        let mut palette = vec![0xff00_0000; 4 * 3 * 256];
        // Draw the front half first. The capture preserves this index-buffer order for early Z.
        for z in [8, 0] {
            for x in [0, 12] {
                let page = tiles.len();
                let tile = TracedVolumeTile { origin_cells: [x - 12, -8, z - 8], extent: [12, 16, 8],
                    atlas_origin: [x as u8, 0, z as u8], palette_page: page as u8 };
                tile.append_box(&mut vertices, &mut indices, PITCH);
                tiles.push(tile);
                // Black metal isolates linear emission. Red identifies the tile, green the material.
                for mat in 1..=6 {
                    palette[(3 * page + 1) * 256 + mat] = 0xffff_ff00;
                    palette[(3 * page + 2) * 256 + mat] = 0xff00_00a0
                        | ((40 + 48 * page as u32) << 16) | ((25 + 30 * mat as u32) << 8);
                }
            }
        }
        let programs: &[&str] = if series { &["volume", "volume_no_inside"] } else { &["volume"] };
        trace_packed_asset(r, cx, "room_boxes", vertices, indices, [0.0, 0.5], 5, programs);
        r.set_csm_scene_bounds(Some((vec3f(-4.2, -1.6, -1.5), vec3f(4.2, 1.6, 1.1))));
        r.set_csm_near_bounds(Some((vec3f(0.2, -1.5, -1.5), vec3f(2.7, 0.9, 0.85))));
        let palette = Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 {
            width: 256, height: 12, data: Some(palette), updated: TextureUpdated::Full,
        });
        let mut bindings = vec![(live_id!(trace_palette), palette)];
        let textures = cx.gpu_info().volume_caps().r8_unorm.then(|| {
            let mut upload = |extent, data| Texture::new_volume(cx, VolumeDesc { extent, format: VolumeFormat::R8Unorm }, data)
                .expect("room R8 atlas");
            let material = upload(DIMS, cells.clone());
            let occupancy = upload(DIMS.map(|n| n / 4), coarse.clone());
            bindings.extend([(live_id!(trace_volume), material.clone()), (live_id!(trace_coarse), occupancy.clone())]);
            [material, occupancy]
        });
        recipe.textures = bindings.into();
        let subject = trace_instance("volume", [1.45, 0.0, 0.0], [1.0; 3], [1.0; 3]);
        // The cube sits just behind the first pillar, with one edge visible around it.
        let center = subject.transform.transform_vec4(vec4(0.15, 0.02, -0.25, 1.0));
        let cube = trace_instance("receiver", [center.x, center.y, center.z], [0.21; 3], [0.15, 0.8, 0.3]);
        let subject_no_inside = ModelInstance { model: "volume_no_inside".into(), ..subject.clone() };
        (Self { tiles, cells, coarse, textures, cleared: false, subject, subject_no_inside, cube }, recipe)
    }

    pub(super) fn camera(&self, camera: VolumeCamera) -> (Vec3f, Vec3f, f32) {
        let local_eye = match camera {
            VolumeCamera::Outside => return (vec3f(-0.8, 0.8, 8.0), Vec3f::default(), 21.4),
            VolumeCamera::Inside => vec3f(-0.82, 0.035, 0.22),
            // The oblique z_ndc=0 rectangle cuts the left wall at x=-1.2..-1.1.
            VolumeCamera::Slice => vec3f(-1.27, 0.045, 0.37),
        };
        let eye = self.subject.transform.transform_vec4(local_eye.to_vec4()).to_vec3f();
        let target = self.cube.transform.transform_vec4(vec4(0.0, 0.0, 0.0, 1.0)).to_vec3f();
        (eye, target, 55.0)
    }

    pub(super) fn interior_cube(&self, camera: VolumeCamera) -> Option<&ModelInstance> {
        (camera != VolumeCamera::Outside).then_some(&self.cube)
    }

    pub(super) fn set_cleared(&mut self, cx: &mut Cx, clear: bool) {
        if self.cleared == clear { return; }
        self.cleared = clear;
        let value = if clear { 0 } else { PILLAR };
        let [x, y, z] = EDIT;
        for dz in 0..4 {
            for dy in 0..4 {
                let start = (x + DIMS[0] * (y + dy + DIMS[1] * (z + dz))) as usize;
                self.cells[start..start + 4].fill(value);
            }
        }
        patch_coarse(&self.cells, DIMS, &mut self.coarse, EDIT, EDIT.map(|v| v + 4));
        if let Some([material, occupancy]) = &self.textures {
            material.update_volume(cx, VolumeRegion { origin: EDIT, extent: [4; 3] }, 4, 16, &[value; 64]).unwrap();
            let block = EDIT.map(|v| v / 4);
            let i = (block[0] + DIMS[0] / 4 * (block[1] + DIMS[1] / 4 * block[2])) as usize;
            occupancy.update_volume(cx, VolumeRegion { origin: block, extent: [1; 3] }, 1, 1, &self.coarse[i..=i]).unwrap();
        }
    }

    pub(super) fn metadata(&self, view: Mat4f, projection: Mat4f, camera: VolumeCamera) -> HostBuf {
        use makepad_draw::makepad_platform::makepad_micro_serde::SerBin;
        let mut bytes = Vec::new();
        (view.v, projection.v, self.subject.transform.v).ser_bin(&mut bytes);
        (PITCH, TILE_CELLS, DIMS).ser_bin(&mut bytes);
        self.tiles.len().ser_bin(&mut bytes);
        for tile in &self.tiles {
            (tile.origin_cells, tile.extent, tile.atlas_origin, tile.palette_page).ser_bin(&mut bytes);
        }
        self.cells.ser_bin(&mut bytes);
        self.coarse.ser_bin(&mut bytes);
        // The receiver mesh spans [-1, 1] on every axis before this world transform.
        self.interior_cube(camera).map(|cube| cube.transform.v).ser_bin(&mut bytes);
        bytes.into()
    }
}

const BLOCK_FRAMES: usize = 16;
const SERIES_FRAMES: usize = 12 * BLOCK_FRAMES;
const WARMUP_FRAMES: usize = 45;

pub(super) struct VolumeSeries {
    clock_hold_pass: DrawPass,
    list: DrawList,
    quad: DrawQuad,
    iters: u32,
    warmup: usize,
    hold_ms: Vec<f64>,
    camera: VolumeCamera,
    switch_discards: usize,
    inside_enabled: bool,
    alternate: Vec<ModelInstance>,
    submitted: usize,
    received: usize,
    drain: usize,
    // Each tag identifies one slot, independent of completion order and repeated paints.
    inside_draws: [Option<u64>; SERIES_FRAMES],
    seen: [bool; SERIES_FRAMES],
}

impl VolumeSeries {
    pub(super) fn new(cx: &mut Cx, iters: u32, subject_no_inside: &ModelInstance) -> Self {
        let clock_hold_pass = DrawPass::new_with_name(cx, "volume series clock hold");
        clock_hold_pass.set_background(cx, true);
        let texture = Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 {
            size: TextureSize::Fixed { width: 512, height: 512 }, initial: true,
        });
        clock_hold_pass.set_color_texture(cx, &texture, DrawPassClearColor::ClearWith(Vec4f::default()));
        let mut quad = cx.with_vm(|vm| {
            let shader = script_eval!(vm, { mod.widgets.LabClockHold });
            DrawQuad::script_from_value(vm, shader)
        });
        quad.draw_vars.set_uniform(cx, live_id!(iters), &[iters as f32]);
        Self {
            clock_hold_pass, list: DrawList::new(cx), quad, iters, warmup: 0,
            hold_ms: Vec::with_capacity(WARMUP_FRAMES), camera: VolumeCamera::Outside,
            switch_discards: 0, inside_enabled: true, alternate: vec![subject_no_inside.clone()],
            submitted: 0, received: 0, drain: 0, inside_draws: [None; SERIES_FRAMES],
            seen: [false; SERIES_FRAMES],
        }
    }

    pub(super) fn next_frame(&mut self, cx: &mut Cx, r: &mut Renderer, announced: bool) -> (VolumeCamera, u64) {
        if self.warmup == 0 {
            for name in ["volume", "volume_no_inside"] {
                match r.traced_surface_state(name) {
                    TracedSurfaceState::Ready => {}
                    TracedSurfaceState::Failed(reason) => panic!("traced-volume-series rejected {name}: {reason}"),
                    _ => return (self.camera, 0),
                }
            }
            if !announced || !r.items_ready(cx) || !r.scene_settled() { return (self.camera, 0); }
            self.clock_hold_pass.set_gpu_timing_enabled(cx, true);
        }
        if self.warmup <= WARMUP_FRAMES {
            self.hold_ms.extend(self.clock_hold_pass.gpu_times_ms(cx));
            self.warmup += 1;
            if self.warmup <= WARMUP_FRAMES { return (self.camera, 0); }
            self.clock_hold_pass.set_gpu_timing_enabled(cx, false);
            assert!(!self.hold_ms.is_empty(), "clock hold warm-up produced no GPU times");
            self.hold_ms.sort_by(f64::total_cmp);
            let n = self.hold_ms.len();
            let median = 0.5 * (self.hold_ms[(n - 1) / 2] + self.hold_ms[n / 2]);
            log!("traced-volume-series warmup={} iters={} samples={} clock_hold_ms={:.6}", WARMUP_FRAMES, self.iters, n, median);
        }
        if self.submitted == SERIES_FRAMES {
            self.drain += 1;
            return (self.camera, 0);
        }

        let block = self.submitted / BLOCK_FRAMES;
        let frame = self.submitted % BLOCK_FRAMES;
        let inside_camera = matches!(block % 4, 1 | 2);
        let camera = if inside_camera { VolumeCamera::Inside } else { VolumeCamera::Outside };
        if self.camera != camera {
            self.camera = camera;
            self.switch_discards = 4;
        }
        let enabled = matches!(frame % 4, 0 | 3);
        if self.inside_enabled != enabled {
            self.alternate = r.set_world_attachments(std::mem::take(&mut self.alternate));
            self.inside_enabled = enabled;
        }
        if self.switch_discards > 0 {
            self.switch_discards -= 1;
            return (camera, 0);
        }
        let cond = 2 * u64::from(inside_camera) + u64::from(!enabled);
        let tag = ((block as u64 + 1) << 8) | (((frame / 2) as u64) << 2) | cond;
        self.submitted += 1;
        (camera, tag)
    }

    pub(super) fn draw_clock_hold(&mut self, cx: &mut Cx2d, scene: &Texture) {
        let size = dvec2(512.0, 512.0);
        cx.make_child_pass(&self.clock_hold_pass);
        cx.begin_pass(&self.clock_hold_pass, Some(1.0));
        self.clock_hold_pass.set_size(cx, size);
        self.clock_hold_pass.set_dpi_factor(cx, 1.0);
        self.list.begin_always(cx);
        cx.begin_root_turtle(size, Layout::flow_overlay());
        self.quad.draw_vars.set_texture(0, scene);
        self.quad.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size });
        cx.end_pass_sized_turtle();
        self.list.end(cx);
        cx.end_pass(&self.clock_hold_pass);
    }

    fn slot(tag: u64) -> usize {
        let block = (tag >> 8) as usize - 1;
        let pair = ((tag & 255) >> 2) as usize;
        block * BLOCK_FRAMES + 2 * pair + (tag & 1) as usize
    }

    pub(super) fn record_submission(&mut self, tag: u64, inside: u64) {
        self.inside_draws[Self::slot(tag)] = Some(inside);
    }

    pub(super) fn finish_frame(&mut self, cx: &mut Cx2d, pass: &DrawPass) {
        for (tag, ms) in pass.take_gpu_time_samples(cx.cx) {
            if tag == 0 { continue; }
            let slot = Self::slot(tag);
            if self.seen[slot] { continue; }
            self.seen[slot] = true;
            self.received += 1;
            let cond = (tag & 3) as usize;
            let inside = self.inside_draws[slot].expect("timed frame has no submission");
            let pair = (slot % BLOCK_FRAMES) / 2;
            let pos = (pair ^ cond) & 1;
            log!("traced-volume-series backend={} cond={} sub_block={} pair={} pos={} tag={} inside={} scene_gpu_ms={:.6}",
                cx.gpu_backend().name(), ["A", "B", "C", "D"][cond], slot / BLOCK_FRAMES,
                pair, pos, tag, inside, ms);
            assert_eq!(inside, u64::from(cond == 2), "unexpected Inside draw count for tag {tag}");
        }
        if self.received == SERIES_FRAMES {
            pass.set_gpu_timing_enabled(cx.cx, false);
            log!("traced-volume-series complete frames={} outside_pairs=48 inside_pairs=48", self.received);
            cx.cx.quit();
        } else {
            assert!(self.drain < 60, "traced-volume-series incomplete after 60 drain frames: {}/{}", self.received, SERIES_FRAMES);
            cx.new_next_frame();
        }
    }
}
