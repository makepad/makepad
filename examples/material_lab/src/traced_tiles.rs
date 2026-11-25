//! The two adjacent boxes used by `--scene=traced-tiles`. R turns the
//! subject 45 degrees, X clears/restores one cell, 1/2 bracket camera depth,
//! and C adds ordinary casters along the two sun-depth probes.
use super::*;
use makepad_render::custom_material::TracedTile;

pub(super) struct TileFixture {
    atlas: Option<Texture>,
    cleared: bool,
}

impl TileFixture {
    pub(super) fn install(r: &mut Renderer, cx: &mut Cx) -> (Self, TracedSurfaceRecipe) {
        // Keep the same depth window with and without the subject. Fit the
        // detail tile around the cell shadows and their receivers.
        r.set_csm_scene_bounds(Some((vec3f(-4.2, -1.6, -1.5), vec3f(4.2, 1.6, 1.1))));
        r.set_csm_near_bounds(Some((vec3f(0.4, -1.25, -1.5), vec3f(1.9, 0.4, 0.55))));
        let pitch = 0.125;
        let left = TracedTile {
            origin_cells: [-8, -4, -4], lo: [0, 1, 0], hi: [8, 7, 8],
            atlas_tile: [1, 1, 1], dominant_material: 97, palette_page: 1,
        };
        let right = TracedTile {
            origin_cells: [0, -4, -4], lo: [0, 1, 4], hi: [3, 7, 5],
            atlas_tile: [0, 1, 0], dominant_material: 97, palette_page: 0,
        };
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        left.append_box(&mut vertices, &mut indices, pitch);
        right.append_box(&mut vertices, &mut indices, pitch);
        trace_packed_asset(r, cx, "tile_boxes", vertices, indices, [0.0, 0.5], 2, &["tiles"]);

        let atlas = trace_volumes_available(cx).then(|| {
            let mut data = vec![0; 16 * 16 * 16];
            let mut write = |tile: &TracedTile, x: usize, y: usize, z: usize, material| {
                let [tx, ty, tz] = tile.atlas_tile.map(usize::from);
                data[(tz * 8 + z) * 256 + (ty * 8 + y) * 16 + tx * 8 + x] = material;
            };
            for y in 1..7 {
                for x in 0..2 { write(&left, x, y, 7, 193); }  // first cell
                for x in 3..5 { write(&left, x, y, 1, 6); }    // six empty cells first
                for x in 6..8 { write(&left, x, y, 4, 97); }
                for x in 0..3 { write(&right, x, y, 4, 97); }
            }
            write(&left, 7, 1, 0, 97);
            Texture::new_volume(cx, VolumeDesc { extent: [16; 3], format: VolumeFormat::R8Unorm }, data)
                .expect("fixture R8 atlas")
        });
        let mut palette = vec![0xff00_0000; 6 * 256];
        for (page, material, rgb) in [(1, 193, 0xe03080), (1, 6, 0x20b850), (1, 97, 0x441fd4), (0, 97, 0xc89123)] {
            // Black metal at roughness one isolates the linear RGB emission.
            palette[(3 * page + 1) * 256 + material] = 0xffff_ff00;
            palette[(3 * page + 2) * 256 + material] = 0xff00_0000 | rgb;
        }
        let palette = Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 {
            width: 256, height: 6, data: Some(palette), updated: TextureUpdated::Full,
        });
        let mut recipe = TracedSurfaceRecipe::tiles(pitch);
        let mut bindings = vec![(live_id!(trace_palette), palette)];
        if let Some(volume) = &atlas { bindings.push((live_id!(trace_volume), volume.clone())); }
        recipe.textures = bindings.into();
        (Self { atlas, cleared: false }, recipe)
    }

    pub(super) fn instance(turned: bool) -> ModelInstance {
        let mut subject = trace_instance("tiles", [1.45, 0.0, 0.0], [1.0; 3], [1.0; 3]);
        if turned {
            let c = std::f32::consts::FRAC_1_SQRT_2;
            subject.transform.v[0] = c;
            subject.transform.v[2] = -c;
            subject.transform.v[8] = c;
            subject.transform.v[10] = c;
        }
        subject
    }

    pub(super) fn depth_probes(placed: &mut Vec<ModelInstance>, side: i8) {
        if side == 0 { return; }
        for (x, z) in [(0.645, 0.5), (1.005, -0.25)] {
            placed.push(trace_instance("bar", [x, -0.005, z + 0.008 * f32::from(side)],
                [0.031, 0.031, 0.001], [0.16, 0.8, 0.12]));
        }
    }

    pub(super) fn shadow_probes(placed: &mut Vec<ModelInstance>, caster: bool) {
        // Both rays cross the late cell at y=-0.32, z=-0.25. The sun has
        // dy/dz=0.5. Keep each pad's camera-facing surface on its ray.
        for (x, pad_z, bar_z) in [(0.895, 0.0, 0.30), (1.015, -0.65, -0.95)] {
            let ray_y = |z| -0.195 + 0.5 * z;
            placed.push(trace_instance("receiver", [x, ray_y(pad_z), pad_z - 0.005],
                [0.04, 0.04, 0.005], [0.7; 3]));
            if caster {
                placed.push(trace_instance("bar", [x, ray_y(bar_z), bar_z], [0.04, 0.04, 0.015], [0.3; 3]));
            }
        }
    }

    pub(super) fn set_cleared(&mut self, cx: &mut Cx, clear: bool) {
        if self.cleared == clear { return; }
        self.cleared = clear;
        if let Some(atlas) = &self.atlas {
            let region = VolumeRegion { origin: [9, 11, 15], extent: [1; 3] };
            atlas.update_volume(cx, region, 1, 1, &[if clear { 0 } else { 193 }])
                .expect("edit one tile cell in place");
        }
    }
}
