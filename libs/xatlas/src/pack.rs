//! Chart packing from `vendor/xatlas.cpp:7974`.

use crate::atlas::PackOptions;
use crate::math::*;
use crate::mesh::{Mesh, UniformGrid2};
use crate::param;
use crate::raster;
use crate::util::*;

pub struct PackChart {
    pub atlas_index: i32,
    pub material: u32,
    pub indices: Vec<u32>,
    pub parametric_area: f32,
    pub surface_area: f32,
    pub vertices: Vec<Vec2>,
    pub unique_vertices: Vec<u32>,
    pub major_axis: Vec2,
    pub minor_axis: Vec2,
    pub min_corner: Vec2,
    pub max_corner: Vec2,
    pub boundary_edges: Option<Vec<u32>>,
}

impl PackChart {
    pub fn unique_vertex_at(&self, v: u32) -> Vec2 {
        if self.unique_vertices.is_empty() {
            self.vertices[v as usize]
        } else {
            self.vertices[self.unique_vertices[v as usize] as usize]
        }
    }
    pub fn unique_vertex_at_mut(&mut self, v: u32) -> &mut Vec2 {
        if self.unique_vertices.is_empty() {
            &mut self.vertices[v as usize]
        } else {
            let i = self.unique_vertices[v as usize];
            &mut self.vertices[i as usize]
        }
    }
    pub fn unique_vertex_count(&self) -> u32 {
        if self.unique_vertices.is_empty() {
            self.vertices.len() as u32
        } else {
            self.unique_vertices.len() as u32
        }
    }
}

pub struct PackAtlas {
    charts: Vec<PackChart>,
    bit_images: Vec<BitImage>,
    utilization: Vec<f32>,
    radix: RadixSort,
    width: u32,
    height: u32,
    texels_per_unit: f32,
    rand: KissRng,
}

impl Default for PackAtlas {
    fn default() -> Self {
        Self {
            charts: Vec::new(),
            bit_images: Vec::new(),
            utilization: Vec::new(),
            radix: RadixSort::default(),
            width: 0,
            height: 0,
            texels_per_unit: 0.0,
            rand: KissRng::default(),
        }
    }
}

impl PackAtlas {
    pub fn get_width(&self) -> u32 {
        self.width
    }
    pub fn get_height(&self) -> u32 {
        self.height
    }
    pub fn get_num_atlases(&self) -> u32 {
        self.bit_images.len() as u32
    }
    pub fn get_texels_per_unit(&self) -> f32 {
        self.texels_per_unit
    }
    pub fn get_chart(&self, index: u32) -> &PackChart {
        &self.charts[index as usize]
    }
    pub fn get_chart_count(&self) -> u32 {
        self.charts.len() as u32
    }
    pub fn get_utilization(&self, atlas: u32) -> f32 {
        self.utilization[atlas as usize]
    }

    pub fn add_charts(&mut self, param_atlas: &mut param::ParamAtlas) {
        let mut count = 0u32;
        for i in 0..param_atlas.mesh_count() {
            for j in 0..param_atlas.chart_group_count(i) {
                count += param_atlas.chart_group_at(i, j).chart_count();
            }
        }
        if count == 0 {
            return;
        }
        let mut bb = BoundingBox2D::default();
        for i in 0..param_atlas.mesh_count() {
            for j in 0..param_atlas.chart_group_count(i) {
                let cg = param_atlas.chart_group_at(i, j);
                for k in 0..cg.chart_count() {
                    // Need mut chart for restore_texcoords.
                    let _ = (i, j, k);
                }
            }
        }
        // Restore + build pack charts in the same order as C++ tasks (enqueue order).
        for i in 0..param_atlas.mesh_count() {
            for j in 0..param_atlas.chart_group_count(i) {
                // Safety: we iterate groups; we need mut on charts.
                let n = param_atlas.chart_group_at(i, j).chart_count();
                for k in 0..n {
                    let chart = self.build_pack_chart(param_atlas, i, j, k, &mut bb);
                    self.charts.push(chart);
                }
            }
        }
    }

    fn build_pack_chart(
        &self,
        param_atlas: &mut param::ParamAtlas,
        i: u32,
        j: u32,
        k: u32,
        bb: &mut BoundingBox2D,
    ) -> PackChart {
        let cg = param_atlas.chart_group_at(i, j);
        // restore + read via raw pointer to avoid borrow issues
        let chart_ptr = cg.chart_at(k) as *const param::Chart as *mut param::Chart;
        unsafe {
            (*chart_ptr).restore_texcoords();
            let mesh: &Mesh = (*chart_ptr).unified_mesh();
            let mut chart = PackChart {
                atlas_index: -1,
                material: 0,
                indices: mesh.indices().to_vec(),
                parametric_area: mesh.compute_parametric_area(),
                surface_area: mesh.compute_surface_area(),
                vertices: mesh.texcoords().to_vec(),
                unique_vertices: Vec::new(),
                major_axis: Vec2::splat(0.0),
                minor_axis: Vec2::splat(0.0),
                min_corner: Vec2::splat(0.0),
                max_corner: Vec2::splat(0.0),
                boundary_edges: Some(mesh.boundary_edges().to_vec()),
            };
            if chart.parametric_area < AREA_EPSILON {
                let bounds = (*chart_ptr).compute_parametric_bounds();
                chart.parametric_area = bounds.x * bounds.y;
            }
            bb.clear();
            for v in 0..chart.vertices.len() as u32 {
                if mesh.is_boundary_vertex(v) {
                    bb.append_boundary_vertex(mesh.texcoord(v));
                }
            }
            let tcs = mesh.texcoords().to_vec();
            bb.compute(Some(&tcs));
            chart.major_axis = bb.major_axis;
            chart.minor_axis = bb.minor_axis;
            chart.min_corner = bb.min_corner;
            chart.max_corner = bb.max_corner;
            chart
        }
    }

    pub fn pack_charts(&mut self, options: &PackOptions) -> bool {
        let chart_count = self.charts.len() as u32;
        if chart_count == 0 {
            return true;
        }
        self.texels_per_unit = options.texels_per_unit;
        let mut resolution = if options.resolution > 0 {
            options.resolution + options.padding * 2
        } else {
            0
        };
        let max_resolution = if self.texels_per_unit > 0.0 {
            resolution
        } else {
            0
        };
        if resolution == 0 || self.texels_per_unit <= 0.0 {
            if resolution == 0 && self.texels_per_unit <= 0.0 {
                resolution = 1024;
            }
            let mut mesh_area = 0.0f32;
            for c in 0..chart_count {
                mesh_area += self.charts[c as usize].surface_area;
            }
            if resolution == 0 {
                let texel_count = 1.0f32.max(mesh_area * square(self.texels_per_unit) / 0.75);
                resolution = 1u32.max(next_power_of_two(c_sqrt(texel_count) as u32));
            }
            if self.texels_per_unit <= 0.0 {
                let texel_count = 1.0f32.max(mesh_area / 0.75);
                self.texels_per_unit = c_sqrt((resolution * resolution) as f32 / texel_count);
            }
        }
        let mut chart_order_array = vec![0.0f32; chart_count as usize];
        let mut chart_extents = vec![Vec2::splat(0.0); chart_count as usize];
        let mut min_chart_perimeter = f32::MAX;
        let mut max_chart_perimeter = 0.0f32;
        for c in 0..chart_count {
            let chart = &mut self.charts[c as usize];
            let mut scale = 1.0f32;
            if chart.parametric_area != 0.0 {
                scale = c_sqrt(chart.surface_area / chart.parametric_area) * self.texels_per_unit;
            }
            let mut min_corner = Vec2::new(f32::MAX, f32::MAX);
            if !options.rotate_charts_to_axis {
                for i in 0..chart.unique_vertex_count() {
                    min_corner = min2(min_corner, chart.unique_vertex_at(i));
                }
            }
            let mut extents = Vec2::splat(0.0);
            let major = chart.major_axis;
            let minor = chart.minor_axis;
            let chart_min = chart.min_corner;
            let n = chart.unique_vertex_count();
            for i in 0..n {
                let texcoord = chart.unique_vertex_at_mut(i);
                if options.rotate_charts_to_axis {
                    let x = dot2(*texcoord, major);
                    let y = dot2(*texcoord, minor);
                    texcoord.x = x;
                    texcoord.y = y;
                    *texcoord -= chart_min;
                } else {
                    *texcoord -= min_corner;
                }
                *texcoord = *texcoord * scale;
                extents = max2(extents, *texcoord);
            }
            if extents.x > 0.0 && extents.y > 0.0 {
                let block_align_size_offset = options.padding as i32 * 2 + 1;
                let mut width = ftoi_ceil(extents.x);
                if options.block_align {
                    width = align_i(width + block_align_size_offset, 4) - block_align_size_offset;
                }
                let mut height = ftoi_ceil(extents.y);
                if options.block_align {
                    height = align_i(height + block_align_size_offset, 4) - block_align_size_offset;
                }
                for v in 0..n {
                    let texcoord = chart.unique_vertex_at_mut(v);
                    texcoord.x = texcoord.x / extents.x * width as f32;
                    texcoord.y = texcoord.y / extents.y * height as f32;
                }
                extents.x = width as f32;
                extents.y = height as f32;
            }
            let mut max_chart_size = options.max_chart_size;
            if max_resolution > 0 && (max_chart_size == 0 || max_resolution < max_chart_size) {
                max_chart_size = max_resolution - options.padding * 2;
            }
            if max_chart_size > 0 {
                let real_max_chart_size = max_chart_size as f32 - 1.0;
                if extents.x > real_max_chart_size || extents.y > real_max_chart_size {
                    let scale = real_max_chart_size / extents.x.max(extents.y);
                    for i in 0..n {
                        let texcoord = chart.unique_vertex_at_mut(i);
                        *texcoord = min2(*texcoord * scale, Vec2::splat(real_max_chart_size));
                    }
                }
            }
            extents.x = 0.0;
            extents.y = 0.0;
            for v in 0..n {
                let texcoord = chart.unique_vertex_at_mut(v);
                texcoord.x += 0.5 + options.padding as f32;
                texcoord.y += 0.5 + options.padding as f32;
                extents = max2(extents, *texcoord);
            }
            chart_extents[c as usize] = extents;
            chart_order_array[c as usize] = extents.x + extents.y;
            min_chart_perimeter = min_chart_perimeter.min(chart_order_array[c as usize]);
            max_chart_perimeter = max_chart_perimeter.max(chart_order_array[c as usize]);
        }
        self.radix.sort(&mut chart_order_array);
        let ranks: Vec<u32> = self.radix.ranks().to_vec();
        let chart_perimeter_bucket_size = (max_chart_perimeter - min_chart_perimeter) / 16.0;
        let mut current_chart_bucket = 0u32;
        let mut chart_start_positions = vec![Vec2i::new(0, 0)];
        let mut chart_image = BitImage::new();
        let mut chart_image_bilinear = BitImage::new();
        let mut chart_image_padding = BitImage::new();
        let mut chart_image_rotated = BitImage::new();
        let mut chart_image_bilinear_rotated = BitImage::new();
        let mut chart_image_padding_rotated = BitImage::new();
        let mut boundary_edge_grid = UniformGrid2::default();
        let mut atlas_sizes = vec![Vec2i::new(0, 0)];
        for i in 0..chart_count {
            let c = ranks[(chart_count - i - 1) as usize];
            chart_image.resize(
                (ftoi_ceil(chart_extents[c as usize].x) as u32) + options.padding,
                (ftoi_ceil(chart_extents[c as usize].y) as u32) + options.padding,
                true,
            );
            if options.rotate_charts {
                chart_image_rotated.resize(chart_image.height(), chart_image.width(), true);
            }
            if options.bilinear {
                chart_image_bilinear.resize(chart_image.width(), chart_image.height(), true);
                if options.rotate_charts {
                    chart_image_bilinear_rotated.resize(chart_image.height(), chart_image.width(), true);
                }
            }
            let face_count = self.charts[c as usize].indices.len() as u32 / 3;
            for f in 0..face_count {
                let mut vertices = [Vec2::splat(0.0); 3];
                for v in 0..3 {
                    let idx = self.charts[c as usize].indices[(f * 3 + v) as usize];
                    vertices[v as usize] = self.charts[c as usize].vertices[idx as usize];
                }
                let mut args = DrawArgs {
                    image: &mut chart_image as *mut BitImage,
                    rotated: if options.rotate_charts {
                        Some(&mut chart_image_rotated as *mut BitImage)
                    } else {
                        None
                    },
                };
                raster::draw_triangle(
                    Vec2::new(chart_image.width() as f32, chart_image.height() as f32),
                    vertices,
                    draw_triangle_callback,
                    &mut args as *mut DrawArgs as *mut (),
                );
            }
            if options.bilinear {
                let rot = if options.rotate_charts {
                    Some(&mut chart_image_bilinear_rotated)
                } else {
                    None
                };
                bilinear_expand(
                    &self.charts[c as usize],
                    &chart_image,
                    &mut chart_image_bilinear,
                    rot,
                    &mut boundary_edge_grid,
                );
            }
            if options.padding > 0 {
                if options.bilinear {
                    chart_image_bilinear.copy_to(&mut chart_image_padding);
                } else {
                    chart_image.copy_to(&mut chart_image_padding);
                }
                chart_image_padding.dilate(options.padding);
                if options.rotate_charts {
                    if options.bilinear {
                        chart_image_bilinear_rotated.copy_to(&mut chart_image_padding_rotated);
                    } else {
                        chart_image_rotated.copy_to(&mut chart_image_padding_rotated);
                    }
                    chart_image_padding_rotated.dilate(options.padding);
                }
            }
            if options.brute_force
                && chart_order_array[c as usize] > min_chart_perimeter
                && chart_order_array[c as usize]
                    <= max_chart_perimeter - (chart_perimeter_bucket_size * (current_chart_bucket + 1) as f32)
            {
                for p in &mut chart_start_positions {
                    *p = Vec2i::new(0, 0);
                }
                current_chart_bucket += 1;
            }
            let (pack_img, pack_rot): (*const BitImage, *const BitImage) = if options.padding > 0 {
                (&chart_image_padding, &chart_image_padding_rotated)
            } else if options.bilinear {
                (&chart_image_bilinear, &chart_image_bilinear_rotated)
            } else {
                (&chart_image, &chart_image_rotated)
            };
            let mut current_atlas = 0u32;
            let mut best_x = 0i32;
            let mut best_y = 0i32;
            let mut best_cw = 0i32;
            let mut best_ch = 0i32;
            let mut best_r = 0i32;
            loop {
                if current_atlas + 1 > self.bit_images.len() as u32 {
                    self.bit_images
                        .push(BitImage::with_size(resolution, resolution));
                    atlas_sizes.push(Vec2i::new(0, 0));
                    chart_start_positions.push(Vec2i::new(0, 0));
                }
                let start_pos = chart_start_positions[current_atlas as usize];
                let atlas_w = atlas_sizes[current_atlas as usize].x;
                let atlas_h = atlas_sizes[current_atlas as usize].y;
                let atlas_img = &self.bit_images[current_atlas as usize] as *const BitImage;
                let found = self.find_chart_location(
                    options,
                    start_pos,
                    unsafe { &*atlas_img },
                    unsafe { &*pack_img },
                    unsafe { &*pack_rot },
                    atlas_w,
                    atlas_h,
                    &mut best_x,
                    &mut best_y,
                    &mut best_cw,
                    &mut best_ch,
                    &mut best_r,
                    max_resolution,
                );
                if max_resolution == 0 {
                    debug_assert!(found);
                    break;
                }
                if found {
                    break;
                }
                current_atlas += 1;
            }
            if options.brute_force {
                if best_x + best_cw > atlas_sizes[current_atlas as usize].x
                    || best_y + best_ch > atlas_sizes[current_atlas as usize].y
                {
                    for p in &mut chart_start_positions {
                        *p = Vec2i::new(0, 0);
                    }
                } else {
                    chart_start_positions[current_atlas as usize] = Vec2i::new(best_x, best_y);
                }
            }
            atlas_sizes[current_atlas as usize].x =
                atlas_sizes[current_atlas as usize].x.max(best_x + best_cw);
            atlas_sizes[current_atlas as usize].y =
                atlas_sizes[current_atlas as usize].y.max(best_y + best_ch);
            if max_resolution == 0 {
                let w = atlas_sizes[current_atlas as usize].x as u32;
                let h = atlas_sizes[current_atlas as usize].y as u32;
                if w > self.bit_images[0].width() || h > self.bit_images[0].height() {
                    self.bit_images[0].resize(next_power_of_two(w), next_power_of_two(h), false);
                }
            }
            add_chart_bits(
                &mut self.bit_images[current_atlas as usize],
                unsafe { &*pack_img },
                unsafe { &*pack_rot },
                atlas_sizes[current_atlas as usize].x,
                atlas_sizes[current_atlas as usize].y,
                best_x,
                best_y,
                best_r,
            );
            self.charts[c as usize].atlas_index = current_atlas as i32;
            let n = self.charts[c as usize].unique_vertex_count();
            for v in 0..n {
                let texcoord = self.charts[c as usize].unique_vertex_at_mut(v);
                let mut t = *texcoord;
                if best_r != 0 {
                    std::mem::swap(&mut t.x, &mut t.y);
                }
                texcoord.x = best_x as f32 + t.x;
                texcoord.y = best_y as f32 + t.y;
                texcoord.x -= options.padding as f32;
                texcoord.y -= options.padding as f32;
            }
        }
        if max_resolution == 0 {
            self.width = (atlas_sizes[0].x - options.padding as i32 * 2).max(0) as u32;
            self.height = (atlas_sizes[0].y - options.padding as i32 * 2).max(0) as u32;
        } else {
            self.width = max_resolution - options.padding * 2;
            self.height = self.width;
        }
        self.utilization.resize(self.bit_images.len(), 0.0);
        for i in 0..self.utilization.len() {
            if self.width == 0 || self.height == 0 {
                self.utilization[i] = 0.0;
            } else {
                let mut count = 0u32;
                for y in 0..self.height {
                    for x in 0..self.width {
                        count += self.bit_images[i].get(x, y) as u32;
                    }
                }
                self.utilization[i] = count as f32 / (self.width * self.height) as f32;
            }
        }
        true
    }

    fn find_chart_location(
        &mut self,
        options: &PackOptions,
        start: Vec2i,
        atlas: &BitImage,
        chart: &BitImage,
        chart_rot: &BitImage,
        w: i32,
        h: i32,
        best_x: &mut i32,
        best_y: &mut i32,
        best_w: &mut i32,
        best_h: &mut i32,
        best_r: &mut i32,
        max_resolution: u32,
    ) -> bool {
        let attempts = 4096;
        if options.brute_force || attempts >= w * h {
            self.find_chart_location_brute_force(
                options,
                start,
                atlas,
                chart,
                chart_rot,
                w,
                h,
                best_x,
                best_y,
                best_w,
                best_h,
                best_r,
                max_resolution,
            )
        } else {
            self.find_chart_location_random(
                options,
                atlas,
                chart,
                chart_rot,
                w,
                h,
                best_x,
                best_y,
                best_w,
                best_h,
                best_r,
                attempts,
                max_resolution,
            )
        }
    }

    fn find_chart_location_brute_force(
        &self,
        options: &PackOptions,
        start: Vec2i,
        atlas: &BitImage,
        chart: &BitImage,
        chart_rot: &BitImage,
        w: i32,
        h: i32,
        best_x: &mut i32,
        best_y: &mut i32,
        best_w: &mut i32,
        best_h: &mut i32,
        best_r: &mut i32,
        max_resolution: u32,
    ) -> bool {
        let step_size = if options.block_align { 4 } else { 1 };
        let mut best_metric = i32::MAX;
        for r in 0..2 {
            let mut cw = chart.width() as i32;
            let mut ch = chart.height() as i32;
            if r == 1 {
                if options.rotate_charts {
                    std::mem::swap(&mut cw, &mut ch);
                } else {
                    break;
                }
            }
            let mut y = start.y;
            while y <= h + step_size {
                if max_resolution > 0 && y > max_resolution as i32 - ch {
                    break;
                }
                let mut x = if y == start.y { start.x } else { 0 };
                while x <= w + step_size {
                    if max_resolution > 0 && x > max_resolution as i32 - cw {
                        break;
                    }
                    let extent_x = w.max(x + cw);
                    let extent_y = h.max(y + ch);
                    let area = extent_x * extent_y;
                    let extents = extent_x.max(extent_y);
                    let metric = extents * extents + area;
                    if metric > best_metric {
                        x += step_size;
                        continue;
                    }
                    if metric == best_metric && x.max(y) >= (*best_x).max(*best_y) {
                        x += step_size;
                        continue;
                    }
                    let img = if r == 1 { chart_rot } else { chart };
                    if !atlas.can_blit(img, x as u32, y as u32) {
                        x += step_size;
                        continue;
                    }
                    best_metric = metric;
                    *best_x = x;
                    *best_y = y;
                    *best_w = cw;
                    *best_h = ch;
                    *best_r = r;
                    if area == w * h {
                        return true;
                    }
                    x += step_size;
                }
                y += step_size;
            }
        }
        best_metric != i32::MAX
    }

    fn find_chart_location_random(
        &mut self,
        options: &PackOptions,
        atlas: &BitImage,
        chart: &BitImage,
        chart_rot: &BitImage,
        w: i32,
        h: i32,
        best_x: &mut i32,
        best_y: &mut i32,
        best_w: &mut i32,
        best_h: &mut i32,
        best_r: &mut i32,
        attempts: i32,
        max_resolution: u32,
    ) -> bool {
        let mut result = false;
        const BLOCK_SIZE: i32 = 4;
        let mut best_metric = i32::MAX;
        for _ in 0..attempts {
            let mut cw = chart.width() as i32;
            let mut ch = chart.height() as i32;
            let r = if options.rotate_charts {
                self.rand.get_range(1)
            } else {
                0
            };
            if r == 1 {
                std::mem::swap(&mut cw, &mut ch);
            }
            let mut x_range = w + 1;
            let mut y_range = h + 1;
            if max_resolution > 0 {
                x_range = x_range.min(max_resolution as i32 - cw);
                y_range = y_range.min(max_resolution as i32 - ch);
            }
            let mut x = self.rand.get_range(x_range as u32) as i32;
            let mut y = self.rand.get_range(y_range as u32) as i32;
            if options.block_align {
                x = align_i(x, BLOCK_SIZE);
                y = align_i(y, BLOCK_SIZE);
                if max_resolution > 0
                    && (x > max_resolution as i32 - cw || y > max_resolution as i32 - ch)
                {
                    continue;
                }
            }
            let area = w.max(x + cw) * h.max(y + ch);
            let extents = w.max(x + cw).max(h.max(y + ch));
            let metric = extents * extents + area;
            if metric > best_metric {
                continue;
            }
            if metric == best_metric && x.min(y) > (*best_x).min(*best_y) {
                continue;
            }
            let img = if r == 1 { chart_rot } else { chart };
            if atlas.can_blit(img, x as u32, y as u32) {
                result = true;
                best_metric = metric;
                *best_x = x;
                *best_y = y;
                *best_w = cw;
                *best_h = ch;
                *best_r = if options.rotate_charts { r as i32 } else { 0 };
                if area == w * h {
                    break;
                }
            }
        }
        result
    }
}

struct DrawArgs {
    image: *mut BitImage,
    rotated: Option<*mut BitImage>,
}

fn draw_triangle_callback(param: *mut (), x: i32, y: i32) -> bool {
    unsafe {
        let args = &mut *(param as *mut DrawArgs);
        (*args.image).set(x as u32, y as u32);
        if let Some(r) = args.rotated {
            (*r).set(y as u32, x as u32);
        }
    }
    true
}

fn add_chart_bits(
    atlas: &mut BitImage,
    chart: &BitImage,
    chart_rot: &BitImage,
    atlas_w: i32,
    atlas_h: i32,
    offset_x: i32,
    offset_y: i32,
    r: i32,
) {
    let image = if r == 0 { chart } else { chart_rot };
    let w = image.width() as i32;
    let h = image.height() as i32;
    for y in 0..h {
        let yy = y + offset_y;
        if yy >= 0 {
            for x in 0..w {
                let xx = x + offset_x;
                if xx >= 0 && image.get(x as u32, y as u32) && xx < atlas_w && yy < atlas_h {
                    atlas.set(xx as u32, yy as u32);
                }
            }
        }
    }
}

fn bilinear_expand(
    chart: &PackChart,
    source: &BitImage,
    dest: &mut BitImage,
    mut dest_rotated: Option<&mut BitImage>,
    boundary_edge_grid: &mut UniformGrid2,
) {
    boundary_edge_grid.reset(&chart.vertices, &chart.indices, 0);
    if let Some(ref be) = chart.boundary_edges {
        for &e in be {
            boundary_edge_grid.append(e);
        }
    } else {
        for i in 0..chart.indices.len() as u32 {
            boundary_edge_grid.append(i);
        }
    }
    let x_offsets = [-1i32, 0, 1, -1, 1, -1, 0, 1];
    let y_offsets = [-1i32, -1, -1, 0, 0, 1, 1, 1];
    for y in 0..source.height() {
        for x in 0..source.width() {
            let mut set_pixel = source.get(x, y);
            if !set_pixel {
                let mut s = 0usize;
                while s < 8 {
                    let sx = x as i32 + x_offsets[s];
                    let sy = y as i32 + y_offsets[s];
                    if sx < 0
                        || sy < 0
                        || sx >= source.width() as i32
                        || sy >= source.height() as i32
                    {
                        s += 1;
                        continue;
                    }
                    if source.get(sx as u32, sy as u32) {
                        break;
                    }
                    s += 1;
                }
                if s == 8 {
                    continue;
                }
                let centroid = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let square = [
                    Vec2::new(centroid.x - 1.0, centroid.y - 1.0),
                    Vec2::new(centroid.x + 1.0, centroid.y - 1.0),
                    Vec2::new(centroid.x + 1.0, centroid.y + 1.0),
                    Vec2::new(centroid.x - 1.0, centroid.y + 1.0),
                ];
                for j in 0..4 {
                    if boundary_edge_grid.intersect_segment(square[j], square[(j + 1) % 4], 0.0) {
                        set_pixel = true;
                        break;
                    }
                }
            }
            if set_pixel {
                dest.set(x, y);
                if let Some(ref mut rot) = dest_rotated.as_deref_mut() {
                    rot.set(y, x);
                }
            }
        }
    }
}
