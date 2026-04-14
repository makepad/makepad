use {
    crate::{
        cx_2d::Cx2d,
        cx_draw::CxDraw,
        draw_list_2d::ManyInstances,
        makepad_platform::*,
        text::{
            color::Color,
            font::FontId,
            font_family::FontFamilyId,
            fonts::Fonts,
            geom::{Point, Rect as TextRect, Size, Transform},
            layouter::{
                BorrowedLayoutParams, LaidoutGlyph, LaidoutRow, LaidoutText, LayoutOptions, Style,
            },
            loader::{FontDefinition, FontFamilyDefinition},
            rasterizer::{AtlasKind, RasterizedGlyph},
        },
        turtle::*,
        turtle::{Align, Walk},
    },
    std::{
        cell::RefCell,
        collections::hash_map::DefaultHasher,
        hash::{Hash, Hasher},
        rc::Rc,
    },
};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom
    use mod.res.*

    mod.text = {
        let text = me
        FontFamily: mod.std.set_type_default() do #(FontFamily::script_component(vm))
        FontMember: mod.std.set_type_default() do #(FontMember::script_api(vm))
        TextOverflow: mod.std.set_type_default() do #(TextOverflow::script_api(vm)),
        ..me.TextOverflow,
        TextStyle: mod.std.set_type_default() do #(TextStyle::script_api(vm)){
            font_size: 10
            font_family: text.FontFamily{
                latin := text.FontMember{res: crate_resource("self:../../widgets/resources/IBMPlexSans-Text.ttf") asc:-0.1 desc:0.0}
            }
            line_spacing: 1.2
        }
    }

    use mod.text.*

    mod.draw.DrawText = mod.std.set_type_default() do #(DrawText::script_shader(vm)){

        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)

        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)

        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)

        color: #fff
        sdf_sharpness: 1.0
        sdf_luma_bias: 0.03

        pos: varying(vec2f)
        t: varying(vec2f)
        world: varying(vec4f)

        radius: uniform(float)
        cutoff: uniform(float)
        aa_pad_px: uniform(float(1.0))
        slug_matrix_0: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        slug_matrix_1: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        slug_matrix_3: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        slug_viewport_px: uniform(vec2(1.0, 1.0))
        total_chars: instance(1000000.0)

        grayscale_texture: texture_2d(float)
        color_texture: texture_2d(float)
        msdf_texture: texture_2d(float)
        curve_texture: texture_2d(float)
        band_texture: texture_2d(float)

        saturate: fn(v: float) -> float {
            return clamp(v, 0.0, 1.0)
        }

        slug_dilate: fn(pos: vec2, tex: vec2, jac: vec4, normal: vec2) -> vec4 {
            let n = normalize(normal)
            let s = dot(self.slug_matrix_3.xy, pos) + self.slug_matrix_3.w
            let t = dot(self.slug_matrix_3.xy, n)

            let u = (
                s * dot(self.slug_matrix_0.xy, n)
                    - t * (dot(self.slug_matrix_0.xy, pos) + self.slug_matrix_0.w)
            ) * self.slug_viewport_px.x
            let v = (
                s * dot(self.slug_matrix_1.xy, n)
                    - t * (dot(self.slug_matrix_1.xy, pos) + self.slug_matrix_1.w)
            ) * self.slug_viewport_px.y

            let s2 = s * s
            let st = s * t
            let uv = u * u + v * v
            let d = normal * (
                s2 * (st + sqrt(uv)) / max(uv - st * st, 0.0000001)
            ) * self.aa_pad_px

            let vpos = pos + d
            let vtex = vec2(tex.x + dot(d, jac.xy), tex.y + dot(d, jac.zw))
            return vec4(vtex.x, vtex.y, vpos.x, vpos.y)
        }

        vertex: fn() {
            let use_slug = if self.texture_index > 2.5 {1.0} else {0.0}

            let p_raster = mix(self.rect_pos, self.rect_pos + self.rect_size, self.geom.pos)
            let p_clipped_raster = clamp(p_raster, self.draw_clip.xy, self.draw_clip.zw)
            let p_normalized_raster = (p_clipped_raster - self.rect_pos) / self.rect_size

            let pad_lpx = self.aa_pad_px / max(self.draw_pass.dpi_factor, 0.0001)
            let content_rect_pos = self.rect_pos + vec2(pad_lpx, pad_lpx)
            let content_rect_size = vec2(
                max(self.rect_size.x - 2.0 * pad_lpx, 0.0001),
                max(self.rect_size.y - 2.0 * pad_lpx, 0.0001)
            )
            let p_slug = mix(content_rect_pos, content_rect_pos + content_rect_size, self.geom.pos)
            let jac = vec4(1.0 / content_rect_size.x, 0.0, 0.0, 1.0 / content_rect_size.y)
            let corner = self.geom.pos * 2.0 - 1.0
            let normal = if dot(corner, corner) > 0.000001 {
                corner
            } else {
                vec2(1.0, 0.0)
            }
            let dilated = self.slug_dilate(p_slug, self.geom.pos, jac, normal)
            let p_clipped_slug = clamp(dilated.zw, self.draw_clip.xy, self.draw_clip.zw)
            let pos_slug = vec2(
                dilated.x + (p_clipped_slug.x - dilated.z) * jac.x,
                dilated.y + (p_clipped_slug.y - dilated.w) * jac.w
            )

            self.pos = mix(p_normalized_raster, pos_slug, use_slug)
            self.t = mix(self.t_min, self.t_max, p_normalized_raster.xy)
            let final_pos = mix(p_clipped_raster, p_clipped_slug, use_slug)
            self.world = self.draw_list.view_transform * vec4(
                final_pos.x,
                final_pos.y,
                self.glyph_depth + self.draw_call.zbias,
                1.
            )
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }

        sdf: fn(scale, p, color) {
            let sampled = self.grayscale_texture.sample_as_bgra(p);
            let s = if self.atlas_plane < 0.5 {
                sampled.r
            } else if self.atlas_plane < 1.5 {
                sampled.g
            } else if self.atlas_plane < 2.5 {
                sampled.b
            } else {
                sampled.a
            };
            // Convert sampled SDF to coverage (0..1). scale is source texels per screen pixel.
            let safe_scale = max(scale, 0.0001);
            let luma = dot(color.rgb, vec3(0.299, 0.587, 0.114));
            var a = clamp(
                (s - (1.0 - self.cutoff)) * self.radius / safe_scale * self.sdf_sharpness + 0.5,
                0.0,
                1.0,
            );
            // Polarity compensation:
            // dark text on light backgrounds usually appears softer than the inverse,
            // so we bias coverage slightly by text luminance.
            let bias = (0.5 - luma) * self.sdf_luma_bias;
            a = clamp(a - bias, 0.0, 1.0);
            return a
        }

        msdf: fn(scale, p, color) {
            let s = self.msdf_texture.sample_as_bgra(p);
            // Use alpha as the coverage source to keep parity with SDF while RGB stores MSDF.
            let dist = s.a;
            let safe_scale = max(scale, 0.0001);
            let luma = dot(color.rgb, vec3(0.299, 0.587, 0.114));
            var a = clamp(
                (dist - (1.0 - self.cutoff)) * self.radius / safe_scale * self.sdf_sharpness + 0.5,
                0.0,
                1.0,
            );
            let bias = (0.5 - luma) * self.sdf_luma_bias;
            // Avoid lifting near-zero background alpha into visible gray quads on light text.
            if a > self.sdf_luma_bias * 0.5 {
                a = clamp(a - bias, 0.0, 1.0);
            }
            return a
        }

        fetch_curve_texel: fn(texel_idx: float) -> vec4 {
            let tex_size = self.curve_texture.size()
            let row = floor(texel_idx / tex_size.x)
            let col = texel_idx - row * tex_size.x
            let uv = vec2(
                (col + 0.5) / tex_size.x,
                (row + 0.5) / tex_size.y
            )
            return self.curve_texture.sample_nearest(uv)
        }

        fetch_band_texel: fn(texel_idx: float) -> vec4 {
            let tex_size = self.band_texture.size()
            let row = floor(texel_idx / tex_size.x)
            let col = texel_idx - row * tex_size.x
            let uv = vec2(
                (col + 0.5) / tex_size.x,
                (row + 0.5) / tex_size.y
            )
            return self.band_texture.sample_nearest(uv)
        }

        pick_channel: fn(v: vec4, channel: float) -> float {
            if channel < 0.5 {
                return v.x
            }
            if channel < 1.5 {
                return v.y
            }
            if channel < 2.5 {
                return v.z
            }
            return v.w
        }

        slug_curve_offset: fn() -> float {
            return self.t_min.x
        }

        slug_curve_count: fn() -> float {
            return self.t_min.y
        }

        slug_band_offset: fn() -> float {
            return self.t_max.x
        }

        slug_band_count: fn() -> float {
            return self.t_max.y
        }

        slug_fill_flags: fn() -> float {
            return self.atlas_plane
        }

        calc_root_code: fn(y1: float, y2: float, y3: float) -> u32 {
            let i1 = asuint(y1) >> u32(31)
            let i2 = asuint(y2) >> u32(30)
            let i3 = asuint(y3) >> u32(29)

            let shift = (i1 & u32(1)) | (i2 & u32(2)) | (i3 & u32(4))
            return (u32(11892) >> shift) & u32(257)
        }

        solve_horiz_poly: fn(p12: vec4, p3: vec2) -> vec2 {
            let a = p12.xy - p12.zw * 2.0 + p3
            let b = p12.xy - p12.zw
            let ra = 1.0 / a.y
            let rb = 0.5 / b.y

            let d = sqrt(max(b.y * b.y - a.y * p12.y, 0.0))
            let mut t1 = (b.y - d) * ra
            let mut t2 = (b.y + d) * ra
            if abs(a.y) < 1.0 / 65536.0 {
                t1 = p12.y * rb
                t2 = t1
            }
            return vec2(
                (a.x * t1 - b.x * 2.0) * t1 + p12.x,
                (a.x * t2 - b.x * 2.0) * t2 + p12.x
            )
        }

        solve_vert_poly: fn(p12: vec4, p3: vec2) -> vec2 {
            let a = p12.xy - p12.zw * 2.0 + p3
            let b = p12.xy - p12.zw
            let ra = 1.0 / a.x
            let rb = 0.5 / b.x

            let d = sqrt(max(b.x * b.x - a.x * p12.x, 0.0))
            let mut t1 = (b.x - d) * ra
            let mut t2 = (b.x + d) * ra
            if abs(a.x) < 1.0 / 65536.0 {
                t1 = p12.x * rb
                t2 = t1
            }
            return vec2(
                (a.y * t1 - b.y * 2.0) * t1 + p12.y,
                (a.y * t2 - b.y * 2.0) * t2 + p12.y
            )
        }

        scan_horizontal_list: fn(list_offset: float, list_count: float, sample: vec2, px_size: float) -> vec2 {
            let limit = floor(list_count + 0.5)
            var coverage = 0.0
            var weight = 0.0

            var j = 0.0
            loop {
                if j >= limit { break }

                let packed_idx = floor(j * 0.25)
                let channel = j - packed_idx * 4.0
                let idx_data = self.fetch_band_texel(list_offset + packed_idx)
                let curve_idx = self.pick_channel(idx_data, channel)

                let p12 = self.fetch_curve_texel(curve_idx * 2.0) - vec4(sample.x, sample.y, sample.x, sample.y)
                let p3 = self.fetch_curve_texel(curve_idx * 2.0 + 1.0).xy - sample
                if max(max(p12.x, p12.z), p3.x) / px_size < -0.5 { break }

                let code = self.calc_root_code(p12.y, p12.w, p3.y)
                if code != u32(0) {
                    let r = self.solve_horiz_poly(p12, p3) / px_size
                    if (code & u32(1)) != u32(0) {
                        coverage = coverage + self.saturate(r.x + 0.5)
                        weight = max(weight, self.saturate(1.0 - abs(r.x) * 2.0))
                    }
                    if code > u32(1) {
                        coverage = coverage - self.saturate(r.y + 0.5)
                        weight = max(weight, self.saturate(1.0 - abs(r.y) * 2.0))
                    }
                }

                j = j + 1.0
            }

            return vec2(coverage, weight)
        }

        scan_vertical_list: fn(list_offset: float, list_count: float, sample: vec2, px_size: float) -> vec2 {
            let limit = floor(list_count + 0.5)
            var coverage = 0.0
            var weight = 0.0

            var j = 0.0
            loop {
                if j >= limit { break }

                let packed_idx = floor(j * 0.25)
                let channel = j - packed_idx * 4.0
                let idx_data = self.fetch_band_texel(list_offset + packed_idx)
                let curve_idx = self.pick_channel(idx_data, channel)

                let p12 = self.fetch_curve_texel(curve_idx * 2.0) - vec4(sample.x, sample.y, sample.x, sample.y)
                let p3 = self.fetch_curve_texel(curve_idx * 2.0 + 1.0).xy - sample
                if max(max(p12.y, p12.w), p3.y) / px_size < -0.5 { break }

                let code = self.calc_root_code(p12.x, p12.z, p3.x)
                if code != u32(0) {
                    let r = self.solve_vert_poly(p12, p3) / px_size
                    if (code & u32(1)) != u32(0) {
                        coverage = coverage - self.saturate(r.x + 0.5)
                        weight = max(weight, self.saturate(1.0 - abs(r.x) * 2.0))
                    }
                    if code > u32(1) {
                        coverage = coverage + self.saturate(r.y + 0.5)
                        weight = max(weight, self.saturate(1.0 - abs(r.y) * 2.0))
                    }
                }

                j = j + 1.0
            }

            return vec2(coverage, weight)
        }

        scan_horizontal_all: fn(sample: vec2, px_size: float) -> vec2 {
            let limit = floor(self.slug_curve_count() + 0.5)
            var coverage = 0.0
            var weight = 0.0

            var i = 0.0
            loop {
                if i >= limit { break }

                let curve_idx = self.slug_curve_offset() + i
                let p12 = self.fetch_curve_texel(curve_idx * 2.0) - vec4(sample.x, sample.y, sample.x, sample.y)
                let p3 = self.fetch_curve_texel(curve_idx * 2.0 + 1.0).xy - sample
                let code = self.calc_root_code(p12.y, p12.w, p3.y)
                if code != u32(0) {
                    let r = self.solve_horiz_poly(p12, p3) / px_size
                    if (code & u32(1)) != u32(0) {
                        coverage = coverage + self.saturate(r.x + 0.5)
                        weight = max(weight, self.saturate(1.0 - abs(r.x) * 2.0))
                    }
                    if code > u32(1) {
                        coverage = coverage - self.saturate(r.y + 0.5)
                        weight = max(weight, self.saturate(1.0 - abs(r.y) * 2.0))
                    }
                }

                i = i + 1.0
            }

            return vec2(coverage, weight)
        }

        scan_vertical_all: fn(sample: vec2, px_size: float) -> vec2 {
            let limit = floor(self.slug_curve_count() + 0.5)
            var coverage = 0.0
            var weight = 0.0

            var i = 0.0
            loop {
                if i >= limit { break }

                let curve_idx = self.slug_curve_offset() + i
                let p12 = self.fetch_curve_texel(curve_idx * 2.0) - vec4(sample.x, sample.y, sample.x, sample.y)
                let p3 = self.fetch_curve_texel(curve_idx * 2.0 + 1.0).xy - sample
                let code = self.calc_root_code(p12.x, p12.z, p3.x)
                if code != u32(0) {
                    let r = self.solve_vert_poly(p12, p3) / px_size
                    if (code & u32(1)) != u32(0) {
                        coverage = coverage - self.saturate(r.x + 0.5)
                        weight = max(weight, self.saturate(1.0 - abs(r.x) * 2.0))
                    }
                    if code > u32(1) {
                        coverage = coverage + self.saturate(r.y + 0.5)
                        weight = max(weight, self.saturate(1.0 - abs(r.y) * 2.0))
                    }
                }

                i = i + 1.0
            }

            return vec2(coverage, weight)
        }

        calc_coverage: fn(xcov: float, ycov: float, xwgt: float, ywgt: float) -> float {
            let coverage = max(
                abs(xcov * xwgt + ycov * ywgt) / max(xwgt + ywgt, 1.0 / 65536.0),
                min(abs(xcov), abs(ycov))
            )
            if self.slug_fill_flags() >= 4096.0 {
                return 1.0 - abs(1.0 - fract(coverage * 0.5) * 2.0)
            }
            return self.saturate(coverage)
        }

        alpha_at: fn(sample: vec2, px_x: float, px_y: float) -> float {
            var coverage_x = 0.0
            var coverage_y = 0.0
            var weight_x = 0.0
            var weight_y = 0.0

            if self.slug_band_count() > 0.5 {
                let num_bands = max(floor(self.slug_band_count() + 0.5), 1.0)
                let h_band_idx = clamp(floor(sample.y * num_bands), 0.0, num_bands - 1.0)
                let v_band_idx = clamp(floor(sample.x * num_bands), 0.0, num_bands - 1.0)

                let h_band_info = self.fetch_band_texel(self.slug_band_offset() + h_band_idx)
                let h_band = self.scan_horizontal_list(
                    floor(h_band_info.x + 0.5),
                    h_band_info.y,
                    sample,
                    px_x,
                )
                coverage_x = h_band.x
                weight_x = h_band.y

                let v_band_info = self.fetch_band_texel(
                    self.slug_band_offset() + num_bands + v_band_idx
                )
                let v_band = self.scan_vertical_list(
                    floor(v_band_info.x + 0.5),
                    v_band_info.y,
                    sample,
                    px_y,
                )
                coverage_y = v_band.x
                weight_y = v_band.y
            } else {
                let x_scan = self.scan_horizontal_all(sample, px_x)
                coverage_x = x_scan.x
                weight_x = x_scan.y

                let y_scan = self.scan_vertical_all(sample, px_y)
                coverage_y = y_scan.x
                weight_y = y_scan.y
            }

            return self.calc_coverage(coverage_x, coverage_y, weight_x, weight_y)
        }

        sample_slug_pixel: fn() {
            if self.slug_curve_count() < 0.5 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }

            let sample = self.pos
            let px_x = max(abs(dFdx(sample.x)) + abs(dFdy(sample.x)), 0.00001)
            let px_y = max(abs(dFdx(sample.y)) + abs(dFdy(sample.y)), 0.00001)
            let alpha_base = if self.aa_4x4 > 0.5 {
                let x0 = px_x * 0.125
                let x1 = px_x * 0.375
                let y0 = px_y * 0.125
                let y1 = px_y * 0.375
                let a0 = self.alpha_at(sample + vec2(-x1, -y1), px_x, px_y)
                let a1 = self.alpha_at(sample + vec2(-x0, -y1), px_x, px_y)
                let a2 = self.alpha_at(sample + vec2( x0, -y1), px_x, px_y)
                let a3 = self.alpha_at(sample + vec2( x1, -y1), px_x, px_y)
                let a4 = self.alpha_at(sample + vec2(-x1, -y0), px_x, px_y)
                let a5 = self.alpha_at(sample + vec2(-x0, -y0), px_x, px_y)
                let a6 = self.alpha_at(sample + vec2( x0, -y0), px_x, px_y)
                let a7 = self.alpha_at(sample + vec2( x1, -y0), px_x, px_y)
                let a8 = self.alpha_at(sample + vec2(-x1,  y0), px_x, px_y)
                let a9 = self.alpha_at(sample + vec2(-x0,  y0), px_x, px_y)
                let a10 = self.alpha_at(sample + vec2( x0,  y0), px_x, px_y)
                let a11 = self.alpha_at(sample + vec2( x1,  y0), px_x, px_y)
                let a12 = self.alpha_at(sample + vec2(-x1,  y1), px_x, px_y)
                let a13 = self.alpha_at(sample + vec2(-x0,  y1), px_x, px_y)
                let a14 = self.alpha_at(sample + vec2( x0,  y1), px_x, px_y)
                let a15 = self.alpha_at(sample + vec2( x1,  y1), px_x, px_y)
                clamp(
                    (a0 + a1 + a2 + a3 + a4 + a5 + a6 + a7 + a8 + a9 + a10 + a11 + a12 + a13 + a14 + a15)
                        * 0.0625,
                    0.0,
                    1.0
                )
            } else if self.aa_2x2 > 0.5 {
                let offset = vec2(px_x * 0.25, px_y * 0.25)
                let a0 = self.alpha_at(sample + vec2(-offset.x, -offset.y), px_x, px_y)
                let a1 = self.alpha_at(sample + vec2(offset.x, -offset.y), px_x, px_y)
                let a2 = self.alpha_at(sample + vec2(-offset.x, offset.y), px_x, px_y)
                let a3 = self.alpha_at(sample + vec2(offset.x, offset.y), px_x, px_y)
                clamp((a0 + a1 + a2 + a3) * 0.25, 0.0, 1.0)
            } else {
                self.alpha_at(sample, px_x, px_y)
            }
            let darken = clamp(max(px_x, px_y) * self.stem_darken, 0.0, self.stem_darken_max)
            let edge_weight = clamp(1.0 - abs(alpha_base * 2.0 - 1.0), 0.0, 1.0)
            let alpha = clamp(alpha_base + darken * edge_weight, 0.0, 1.0)
            let color = self.get_color()
            return vec4(color.rgb * color.a * alpha, color.a * alpha)
        }

        get_color: fn() {
            return self.color
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip);
        }

        sample_text_pixel: fn() {
            if self.texture_index < 0.5 {
                let dxt = length(dFdx(self.t))
                let dyt = length(dFdy(self.t))
                let c = self.get_color()
                let scale = (dxt + dyt) * self.grayscale_texture.size().x * 0.5
                let tex_size = self.grayscale_texture.size()
                let half_texel = vec2(0.5 / tex_size.x, 0.5 / tex_size.y)
                let p = clamp(self.t.xy, self.t_min + half_texel, self.t_max - half_texel)
                let s = self.sdf(scale, p, c)
                return s * vec4(c.rgb * c.a, c.a)
            } else if self.texture_index < 1.5 {
                let tex_size = self.color_texture.size()
                let half_texel = vec2(0.5 / tex_size.x, 0.5 / tex_size.y)
                let p = clamp(self.t.xy, self.t_min + half_texel, self.t_max - half_texel)
                let c = self.color_texture.sample_as_bgra(p)
                return vec4(c.rgb * c.a, c.a)
            } else if self.texture_index < 2.5 {
                let dxt = length(dFdx(self.t))
                let dyt = length(dFdy(self.t))
                let c = self.get_color()
                let scale = (dxt + dyt) * self.msdf_texture.size().x * 0.5
                let tex_size = self.msdf_texture.size()
                let half_texel = vec2(0.5 / tex_size.x, 0.5 / tex_size.y)
                let p = clamp(self.t.xy, self.t_min + half_texel, self.t_max - half_texel)
                let s = self.msdf(scale, p, c)
                return s * vec4(c.rgb * c.a, c.a)
            } else {
                return self.sample_slug_pixel()
            }
        }

        pixel: fn() {
            return self.sample_text_pixel()
        }
    }
}

/// Controls how text overflow is handled when text exceeds its container.
///
/// Analogous to CSS `text-overflow`. Requires a width constraint to take effect.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook)]
pub enum TextOverflow {
    /// Text is clipped at the container boundary (default).
    #[pick]
    Clip,
    /// An ellipsis character (U+2026 "…") is shown where text is truncated.
    Ellipsis,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawText {
    #[rust]
    pub many_instances: Option<ManyInstances>,
    #[live]
    pub text_style: TextStyle,
    #[live(1.0)]
    pub font_scale: f32,
    #[live(0.0)]
    pub draw_depth: f32,
    #[live]
    pub debug: bool,

    #[live]
    pub temp_y_shift: f32,

    /// Maximum number of lines to display. 0 means unlimited (default).
    /// When text exceeds this many lines, excess lines are hidden.
    /// Combined with `text_overflow: Ellipsis`, an ellipsis is appended
    /// to the last visible line.
    #[live(0usize)]
    pub max_lines: usize,

    /// Controls how text overflow is handled.
    /// `Clip` (default) clips text at the boundary.
    /// `Ellipsis` appends "…" at the truncation point.
    #[live]
    pub text_overflow: TextOverflow,

    /// When true, successive draws extend the area instead of replacing it.
    /// Useful when drawing multiple text chunks that should be treated as one area.
    #[live]
    pub extend_area: bool,

    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub rect_pos: Vec2f,
    #[live]
    pub rect_size: Vec2f,
    #[live]
    pub draw_clip: Vec4f,
    #[live(1.0)]
    pub depth_clip: f32,
    #[live]
    pub glyph_depth: f32,
    #[live]
    pub texture_index: f32,
    #[live]
    pub char_index: f32,
    #[live(vec4(1., 1., 1., 1.))]
    pub color: Vec4f,
    #[live(1.0)]
    pub sdf_sharpness: f32,
    #[live(0.03)]
    pub sdf_luma_bias: f32,
    #[live]
    pub t_min: Vec2f,
    #[live]
    pub t_max: Vec2f,
    #[live]
    pub atlas_plane: f32,
    #[live]
    pub pad1: f32,
    #[live(0.0)]
    pub aa_2x2: f32,
    #[live(0.0)]
    pub aa_4x4: f32,
    #[live(0.2)]
    pub stem_darken: f32,
    #[live(0.125)]
    pub stem_darken_max: f32,
}

#[derive(Clone, Debug)]
pub struct PreparedTextGlyph {
    pub pen_x_in_lpxs: f32,
    pub offset_x_in_lpxs: f32,
    pub advance_in_lpxs: f32,
    pub font_size_in_lpxs: f32,
    pub rasterized: RasterizedGlyph,
}

#[derive(Clone, Debug)]
pub struct PreparedTextRun {
    pub width_in_lpxs: f32,
    pub ascender_in_lpxs: f32,
    pub descender_in_lpxs: f32,
    pub glyphs: Vec<PreparedTextGlyph>,
}

impl DrawText {
    pub fn draw_abs(&mut self, cx: &mut Cx2d, pos: Vec2d, text: &str) {
        let text = self.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
        self.draw_text(cx, Point::new(pos.x as f32, pos.y as f32), &text);
    }

    pub fn begin_many_instances(&mut self, cx: &mut Cx2d) {
        if self.many_instances.is_some() {
            return;
        }
        self.update_draw_vars(cx);
        self.many_instances = cx.begin_many_aligned_instances(&self.draw_vars);
    }

    pub fn end_many_instances(&mut self, cx: &mut Cx2d) {
        if let Some(instances) = self.many_instances.take() {
            self.finish_many_instances(cx, instances);
        }
    }

    pub fn draw_rasterized_glyphs_abs(
        &mut self,
        cx: &mut Cx2d,
        glyphs: &[(Point<f32>, f32, RasterizedGlyph)],
        color: Vec4f,
    ) {
        if glyphs.is_empty() {
            return;
        }
        self.update_draw_vars(cx);
        if let Some(mut instances) = self.many_instances.take() {
            self.glyph_depth = self.draw_depth;
            self.color = color;
            for (origin_in_lpxs, font_size_in_lpxs, rasterized_glyph) in glyphs {
                self.draw_rasterized_glyph(
                    *origin_in_lpxs,
                    *font_size_in_lpxs,
                    None,
                    *rasterized_glyph,
                    &mut instances.instances,
                );
            }
            self.many_instances = Some(instances);
            return;
        }

        let Some(mut instances) = cx.begin_many_aligned_instances(&self.draw_vars) else {
            return;
        };

        self.glyph_depth = self.draw_depth;
        self.color = color;
        for (origin_in_lpxs, font_size_in_lpxs, rasterized_glyph) in glyphs {
            self.draw_rasterized_glyph(
                *origin_in_lpxs,
                *font_size_in_lpxs,
                None,
                *rasterized_glyph,
                &mut instances.instances,
            );
        }

        self.finish_many_instances(cx, instances);
    }

    pub fn draw_rasterized_glyph_abs(
        &mut self,
        cx: &mut Cx2d,
        origin_in_lpxs: Point<f32>,
        font_size_in_lpxs: f32,
        rasterized_glyph: RasterizedGlyph,
        color: Vec4f,
    ) {
        self.draw_rasterized_glyphs_abs(
            cx,
            &[(origin_in_lpxs, font_size_in_lpxs, rasterized_glyph)],
            color,
        );
    }

    pub fn prepare_single_line_run(&self, cx: &mut Cx2d, text: &str) -> Option<PreparedTextRun> {
        let laidout = self.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
        let row = laidout.rows.first()?;
        if row.glyphs.is_empty() {
            return None;
        }

        let dpx_factor = cx.current_dpi_factor() as f32;
        let mut glyphs = Vec::with_capacity(row.glyphs.len());
        for glyph in &row.glyphs {
            let dpx_per_em = glyph.font_size_in_lpxs * dpx_factor;
            let Some(rasterized) = glyph.rasterize(dpx_per_em) else {
                continue;
            };

            glyphs.push(PreparedTextGlyph {
                pen_x_in_lpxs: glyph.origin_in_lpxs.x * self.font_scale,
                offset_x_in_lpxs: glyph.offset_in_lpxs() * self.font_scale,
                advance_in_lpxs: glyph.advance_in_lpxs() * self.font_scale,
                font_size_in_lpxs: glyph.font_size_in_lpxs,
                rasterized,
            });
        }
        if glyphs.is_empty() {
            return None;
        }

        Some(PreparedTextRun {
            width_in_lpxs: row.width_in_lpxs * self.font_scale,
            ascender_in_lpxs: row.ascender_in_lpxs * self.font_scale,
            descender_in_lpxs: row.descender_in_lpxs * self.font_scale,
            glyphs,
        })
    }

    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk, align: Align, text: &str) -> Rect {
        let turtle_rect = cx.turtle().inner_rect();
        let mut max_width_in_lpxs = if !turtle_rect.size.x.is_nan() {
            Some(turtle_rect.size.x as f32)
        } else {
            None
        };

        // For Fit-width containers with a max bound, resolve the bound so that
        // ellipsis truncation and max_lines clamping can work. Without this, Fit
        // layouts are unconstrained and text is laid out at full width on one line.
        if max_width_in_lpxs.is_none()
            && (self.text_overflow == TextOverflow::Ellipsis || self.max_lines > 0)
        {
            if let crate::turtle::Size::Fit {
                max: Some(max_bound),
                ..
            } = walk.width
            {
                if let Some(resolved) = max_bound.eval_width(cx) {
                    let padding = cx.turtle().padding();
                    max_width_in_lpxs =
                        Some((resolved - padding.left - padding.right).max(0.0) as f32);
                }
            }
        }

        let wrap = cx.turtle().layout().flow
            == Flow::Right {
                row_align: RowAlign::Top,
                wrap: true,
            };

        let text = self.layout(cx, 0.0, 0.0, max_width_in_lpxs, wrap, align, text);
        self.draw_walk_laidout(cx, walk, &text)
    }

    pub fn draw_walk_laidout(
        &mut self,
        cx: &mut Cx2d,
        walk: Walk,
        laidout_text: &LaidoutText,
    ) -> Rect {
        use crate::text::geom::{Point, Size};
        use crate::turtle;

        let size_in_lpxs = laidout_text.size_in_lpxs * self.font_scale;
        let max_size_in_lpxs = Size::new(
            cx.turtle()
                .max_width(walk)
                .map_or(size_in_lpxs.width, |max_width| max_width as f32),
            cx.turtle()
                .max_height(walk)
                .map_or(size_in_lpxs.height, |max_height| max_height as f32),
        );
        let turtle_rect = cx.walk_turtle(Walk {
            abs_pos: walk.abs_pos,
            margin: walk.margin,
            width: turtle::Size::Fixed(max_size_in_lpxs.width as f64),
            height: turtle::Size::Fixed(max_size_in_lpxs.height as f64),
            metrics: Metrics {
                descender: -laidout_text.rows.last().unwrap().descender_in_lpxs as f64,
                line_gap: 0.0,
                line_scale: 1.0,
            },
        });

        if self.debug {
            let mut area = Area::Empty;
            cx.add_aligned_rect_area(&mut area, turtle_rect);
            cx.cx.debug.area(area, vec4(1.0, 1.0, 1.0, 1.0));
        }

        let origin_in_lpxs = Point::new(turtle_rect.pos.x as f32, turtle_rect.pos.y as f32);
        self.draw_text(cx, origin_in_lpxs, laidout_text);

        rect(
            origin_in_lpxs.x as f64,
            origin_in_lpxs.y as f64,
            size_in_lpxs.width as f64,
            size_in_lpxs.height as f64,
        )
    }

    /// Draws text within the current turtle flow, calling `f` for each laid-out row.
    /// Returns `(row_count, is_truncated)`: the number of rows produced, and whether
    /// the text was truncated (e.g., by `max_lines` / ellipsis).
    pub fn draw_walk_resumable_with(
        &mut self,
        cx: &mut Cx2d,
        text_str: &str,
        mut f: impl FnMut(&mut Cx2d, Rect, f32),
    ) -> (usize, bool) {
        let turtle_pos = cx.turtle().pos();
        let turtle_rect = cx.turtle().inner_rect();
        let origin_in_lpxs = Point::new(turtle_rect.pos.x as f32, turtle_pos.y as f32);
        let first_row_indent_in_lpxs = turtle_pos.x as f32 - origin_in_lpxs.x;
        let row_height = cx.turtle().next_row_offset();

        let max_width_in_lpxs = if !turtle_rect.size.x.is_nan() {
            Some(turtle_rect.size.x as f32)
        } else {
            None
        };
        let wrap = cx.turtle().layout().flow
            == Flow::Right {
                row_align: RowAlign::Top,
                wrap: true,
            };

        let text = self.layout(
            cx,
            first_row_indent_in_lpxs,
            row_height as f32,
            max_width_in_lpxs,
            wrap,
            Align::default(),
            text_str,
        );
        self.draw_text(cx, origin_in_lpxs, &text);

        let last_row = text.rows.last().unwrap();
        let new_turtle_pos = origin_in_lpxs
            + Size::new(
                last_row.width_in_lpxs,
                last_row.origin_in_lpxs.y - last_row.ascender_in_lpxs,
            ) * self.font_scale;
        let used_size_in_lpxs = text.size_in_lpxs * self.font_scale;
        // Account for temp_y_shift in the allocated height so that shifted
        // glyphs (e.g., from top_drop) don't get clipped by their container.
        let shift_extra_height = if self.temp_y_shift != 0.0 {
            let fs = text
                .rows
                .first()
                .and_then(|r| r.glyphs.first())
                .map(|g| g.font_size_in_lpxs)
                .unwrap_or(0.0);
            (self.temp_y_shift * fs * self.font_scale).abs() as f64
        } else {
            0.0
        };
        let new_turtle_pos = dvec2(new_turtle_pos.x as f64, new_turtle_pos.y as f64);
        let turtle = cx.turtle_mut();

        turtle.move_to(dvec2(origin_in_lpxs.x as f64, origin_in_lpxs.y as f64));
        turtle.allocate_width(used_size_in_lpxs.width as f64);
        turtle.allocate_height(used_size_in_lpxs.height as f64 + shift_extra_height);
        turtle.move_to(new_turtle_pos);

        turtle.set_wrap_spacing(
            (last_row.ascender_in_lpxs * last_row.line_spacing_scale - last_row.ascender_in_lpxs)
                as f64,
        );

        cx.emit_turtle_walk(Rect {
            pos: new_turtle_pos,
            size: dvec2(
                used_size_in_lpxs.width as f64,
                used_size_in_lpxs.height as f64 + shift_extra_height,
            ),
        });

        let shift = if let Some(row) = text.rows.first() {
            if let Some(glyph) = row.glyphs.first() {
                glyph.font_size_in_lpxs * self.temp_y_shift
            } else {
                0.0
            }
        } else {
            0.0
        };

        for (row_index, row) in text.rows.iter().enumerate() {
            let (start_x_in_lpxs, end_x_in_lpxs) =
                row_span_x_bounds_in_lpxs(row, row_index == 0, row_index + 1 == text.rows.len());
            let rect_in_lpxs = TextRect::new(
                Point::new(
                    origin_in_lpxs.x + (row.origin_in_lpxs.x + start_x_in_lpxs) * self.font_scale,
                    origin_in_lpxs.y
                        + (row.origin_in_lpxs.y - row.ascender_in_lpxs) * self.font_scale,
                ),
                Size::new(
                    (end_x_in_lpxs - start_x_in_lpxs) * self.font_scale,
                    (row.ascender_in_lpxs - row.descender_in_lpxs) * self.font_scale,
                ),
            );
            f(
                cx,
                rect(
                    rect_in_lpxs.origin.x as f64,
                    rect_in_lpxs.origin.y as f64 + shift as f64,
                    rect_in_lpxs.size.width as f64,
                    rect_in_lpxs.size.height as f64,
                ),
                row.ascender_in_lpxs,
            )
        }
        (text.rows.len(), text.is_truncated)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn layout(
        &self,
        cx: &mut Cx,
        first_row_indent_in_lpxs: f32,
        first_row_min_line_spacing_below_in_lpxs: f32,
        max_width_in_lpxs: Option<f32>,
        wrap: bool,
        align: Align,
        text: &str,
    ) -> Rc<LaidoutText> {
        self.text_style
            .font_family
            .ensure_fonts_loaded_for_text(cx, Some(text));
        let fonts = cx.get_global::<Rc<RefCell<Fonts>>>().clone();
        let mut fonts = fonts.borrow_mut();

        fonts.get_or_layout(BorrowedLayoutParams {
            text,
            style: Style {
                font_family_id: self.text_style.font_family.to_font_family_id(),
                font_size_in_pts: self.text_style.font_size,
                color: None,
            },
            options: LayoutOptions {
                first_row_indent_in_lpxs,
                first_row_min_line_spacing_below_in_lpxs,
                max_width_in_lpxs,
                wrap,
                align: align.x as f32,
                line_spacing_scale: self.text_style.line_spacing,
                max_rows: if self.max_lines > 0 {
                    Some(self.max_lines)
                } else {
                    None
                },
                ellipsis: self.text_overflow == TextOverflow::Ellipsis,
            },
        })
    }

    fn draw_text(&mut self, cx: &mut Cx2d, origin_in_lpxs: Point<f32>, text: &LaidoutText) {
        self.update_draw_vars(cx);
        if let Some(mut instances) = self.many_instances.take() {
            self.glyph_depth = self.draw_depth;
            for row in &text.rows {
                self.draw_row(
                    cx,
                    origin_in_lpxs + Size::from(row.origin_in_lpxs) * self.font_scale,
                    row,
                    &mut instances.instances,
                );
            }
            self.flush_slug_textures(cx);
            self.many_instances = Some(instances);
            return;
        }
        let Some(mut instances) = cx.begin_many_aligned_instances(&self.draw_vars) else {
            return;
        };
        self.glyph_depth = self.draw_depth;
        for row in &text.rows {
            self.draw_row(
                cx,
                origin_in_lpxs + Size::from(row.origin_in_lpxs) * self.font_scale,
                row,
                &mut instances.instances,
            );
        }
        self.flush_slug_textures(cx);
        self.finish_many_instances(cx, instances);
    }

    fn flush_slug_textures(&mut self, cx: &mut Cx2d) {
        let fonts = cx.fonts.clone();
        fonts.borrow_mut().flush_slug_textures(cx.cx);
    }

    fn finish_many_instances(&mut self, cx: &mut Cx2d, instances: ManyInstances) {
        let new_area = cx.end_many_instances(instances);
        let old_area = self.draw_vars.area;
        if self.extend_area {
            let extended = old_area.extend_with(cx, new_area);
            self.draw_vars.area = cx.update_area_refs(old_area, extended);
        } else {
            self.draw_vars.area = cx.update_area_refs(old_area, new_area);
        }
    }

    fn update_draw_vars(&mut self, cx: &mut Cx2d) {
        self.draw_vars.append_group_id = cx.draw_call_group_content().0;
        let fonts = cx.fonts.borrow();
        let rasterizer = fonts.rasterizer().borrow();
        let sdfer_settings = rasterizer.sdfer().settings();
        self.draw_vars.dyn_uniforms[0] = sdfer_settings.radius;
        self.draw_vars.dyn_uniforms[1] = sdfer_settings.cutoff;
        self.draw_vars.texture_slots[0] = Some(fonts.grayscale_texture().clone());
        self.draw_vars.texture_slots[1] = Some(fonts.color_texture().clone());
        self.draw_vars.texture_slots[2] = Some(fonts.msdf_texture().clone());
        self.draw_vars.texture_slots[3] = Some(fonts.slug_curve_texture().clone());
        self.draw_vars.texture_slots[4] = Some(fonts.slug_band_texture().clone());

        let pass_id = cx.pass_stack.last().unwrap().pass_id;
        let draw_list_id = *cx.draw_list_stack.last().unwrap();
        let pass_uniforms = cx.passes[pass_id].pass_uniforms.clone();
        let view_transform = cx.draw_lists[draw_list_id]
            .draw_list_uniforms
            .view_transform;
        let model_view = Mat4f::mul(&pass_uniforms.camera_view, &view_transform);
        let slug_matrix = Mat4f::mul(&pass_uniforms.camera_projection, &model_view);
        let viewport = cx.current_pass_size();
        let dpi_factor = cx.current_dpi_factor() as f32;
        let viewport_px = [
            (viewport.x as f32 * dpi_factor).max(1.0),
            (viewport.y as f32 * dpi_factor).max(1.0),
        ];

        self.draw_vars
            .set_uniform(cx.cx, live_id!(slug_matrix_0), &mat4_row(&slug_matrix, 0));
        self.draw_vars
            .set_uniform(cx.cx, live_id!(slug_matrix_1), &mat4_row(&slug_matrix, 1));
        self.draw_vars
            .set_uniform(cx.cx, live_id!(slug_matrix_3), &mat4_row(&slug_matrix, 3));
        self.draw_vars
            .set_uniform(cx.cx, live_id!(slug_viewport_px), &viewport_px);
    }

    fn draw_row(
        &mut self,
        cx: &mut Cx2d,
        origin_in_lpxs: Point<f32>,
        row: &LaidoutRow,
        out_instances: &mut Vec<f32>,
    ) {
        for glyph in &row.glyphs {
            self.draw_glyph(
                cx,
                origin_in_lpxs + Size::from(glyph.origin_in_lpxs) * self.font_scale,
                glyph,
                out_instances,
            );
        }

        let width_in_lpxs = row.width_in_lpxs * self.font_scale;
        if self.debug {
            let mut area = Area::Empty;
            cx.add_aligned_rect_area(
                &mut area,
                rect(
                    origin_in_lpxs.x as f64,
                    (origin_in_lpxs.y - row.ascender_in_lpxs * self.font_scale) as f64,
                    width_in_lpxs as f64,
                    1.0,
                ),
            );
            cx.cx.debug.area(area, vec4(1.0, 0.0, 0.0, 1.0));
            let mut area = Area::Empty;
            cx.add_aligned_rect_area(
                &mut area,
                rect(
                    origin_in_lpxs.x as f64,
                    origin_in_lpxs.y as f64,
                    width_in_lpxs as f64,
                    1.0,
                ),
            );
            cx.cx.debug.area(area, vec4(0.0, 1.0, 0.0, 1.0));
            let mut area = Area::Empty;
            cx.add_aligned_rect_area(
                &mut area,
                rect(
                    origin_in_lpxs.x as f64,
                    (origin_in_lpxs.y - row.descender_in_lpxs * self.font_scale) as f64,
                    width_in_lpxs as f64,
                    1.0,
                ),
            );
            cx.cx.debug.area(area, vec4(0.0, 0.0, 1.0, 1.0));
        }
    }

    fn draw_glyph(
        &mut self,
        cx: &mut Cx2d,
        origin_in_lpxs: Point<f32>,
        glyph: &LaidoutGlyph,
        output: &mut Vec<f32>,
    ) {
        use crate::text::geom::Point;
        let glyph_origin = Point::new(
            origin_in_lpxs.x + glyph.offset_in_lpxs() * self.font_scale,
            origin_in_lpxs.y,
        );

        let slug_glyph = {
            cx.fonts
                .borrow_mut()
                .get_or_cache_slug_glyph(glyph.font.as_ref(), glyph.id)
        };
        if let Some(slug_glyph) = slug_glyph {
            self.draw_slug_glyph(
                cx,
                glyph_origin,
                glyph.font_size_in_lpxs,
                glyph.color,
                slug_glyph,
                output,
            );
            return;
        }

        let font_size_in_dpxs = glyph.font_size_in_lpxs * cx.current_dpi_factor() as f32;
        if let Some(rasterized_glyph) = glyph.rasterize(font_size_in_dpxs) {
            self.draw_rasterized_glyph(
                glyph_origin,
                glyph.font_size_in_lpxs,
                glyph.color,
                rasterized_glyph,
                output,
            );
        }
    }

    fn draw_slug_glyph(
        &mut self,
        cx: &mut Cx2d,
        origin_in_lpxs: Point<f32>,
        font_size_in_lpxs: f32,
        color: Option<Color>,
        glyph: crate::text::slug_atlas::SlugGlyphInfo,
        output: &mut Vec<f32>,
    ) {
        let bounds_in_lpxs = TextRect::new(
            Point::new(
                origin_in_lpxs.x + glyph.origin_in_ems.x * font_size_in_lpxs * self.font_scale,
                origin_in_lpxs.y
                    + (-glyph.origin_in_ems.y - glyph.size_in_ems.height)
                        * font_size_in_lpxs
                        * self.font_scale,
            ),
            Size::new(
                glyph.size_in_ems.width * font_size_in_lpxs * self.font_scale,
                glyph.size_in_ems.height * font_size_in_lpxs * self.font_scale,
            ),
        );

        let pad = (self.get_aa_pad_px(cx.cx) / cx.current_dpi_factor() as f32).max(0.0);
        self.rect_pos = vec2(bounds_in_lpxs.origin.x - pad, bounds_in_lpxs.origin.y - pad)
            + vec2(0.0, self.temp_y_shift * font_size_in_lpxs);
        self.rect_size = vec2(
            bounds_in_lpxs.size.width + pad * 2.0,
            bounds_in_lpxs.size.height + pad * 2.0,
        );
        if let Some(color) = color {
            self.color = vec4(
                color.r as f32,
                color.g as f32,
                color.b as f32,
                color.a as f32,
            ) / 255.0;
        }
        self.texture_index = 3.0;
        self.atlas_plane = glyph.fill_flags as f32;
        self.t_min = vec2(glyph.curve_offset as f32, glyph.curve_count as f32);
        self.t_max = vec2(glyph.band_offset as f32, glyph.band_count as f32);
        let slice = self.draw_vars.as_slice();

        output.extend_from_slice(slice);
        self.glyph_depth += 0.000001;
        self.char_index += 1.0;
    }

    fn draw_rasterized_glyph(
        &mut self,
        origin_in_lpxs: Point<f32>,
        font_size_in_lpxs: f32,
        color: Option<Color>,
        glyph: RasterizedGlyph,
        output: &mut Vec<f32>,
    ) {
        fn tex_coord(point: Point<usize>, size: Size<usize>) -> Point<f32> {
            Point::new(
                point.x as f32 / size.width as f32,
                point.y as f32 / size.height as f32,
            )
        }

        let texture_index = match glyph.atlas_kind {
            AtlasKind::Grayscale => 0.0,
            AtlasKind::Color => 1.0,
            AtlasKind::Msdf => 2.0,
        };

        let atlas_image_bounds = glyph.atlas_image_bounds;
        let atlas_size = glyph.atlas_size;
        let t_min = tex_coord(glyph.atlas_image_bounds.min(), atlas_size);
        let t_max = tex_coord(glyph.atlas_image_bounds.max(), atlas_size);

        let atlas_image_padding = glyph.atlas_image_padding;
        let atlas_image_size = atlas_image_bounds.size;
        let origin_in_dpxs = glyph.origin_in_dpxs;
        let bounds_in_dpxs = TextRect::new(
            Point::new(
                origin_in_dpxs.x - atlas_image_padding as f32,
                -origin_in_dpxs.y - atlas_image_size.height as f32 + (atlas_image_padding as f32),
            ),
            Size::new(
                atlas_image_size.width as f32,
                atlas_image_size.height as f32,
            ),
        );
        let bounds_in_lpxs = bounds_in_dpxs.apply_transform(
            Transform::from_scale_uniform(font_size_in_lpxs / glyph.dpxs_per_em * self.font_scale)
                .translate(origin_in_lpxs.x, origin_in_lpxs.y),
        );

        self.rect_pos = vec2(bounds_in_lpxs.origin.x, bounds_in_lpxs.origin.y)
            + vec2(0.0, self.temp_y_shift * font_size_in_lpxs);
        self.rect_size = vec2(bounds_in_lpxs.size.width, bounds_in_lpxs.size.height);
        if let Some(color) = color {
            self.color = vec4(
                color.r as f32,
                color.g as f32,
                color.b as f32,
                color.a as f32,
            ) / 255.0;
        }
        self.texture_index = texture_index;
        self.atlas_plane = glyph.atlas_plane as f32;
        self.t_min = vec2(t_min.x, t_min.y);
        self.t_max = vec2(t_max.x, t_max.y);
        let slice = self.draw_vars.as_slice();

        output.extend_from_slice(slice);
        self.glyph_depth += 0.000001;
        self.char_index += 1.0;
    }

    /// Resets the character index counter to 0. Call this before drawing text
    /// when you want to track character positions for animation effects.
    pub fn reset_char_index(&mut self) {
        self.char_index = 0.0;
    }

    /// Sets the total_chars instance value on all instances in the area after drawing is complete.
    /// This allows the shader to know how many characters are in the buffer
    /// for fade-in animation effects.
    pub fn set_total_chars(&mut self, cx: &mut Cx, total: f32) {
        self.draw_vars
            .set_instance_on_area(cx, live_id!(total_chars), &[total]);
    }

    pub fn new_draw_call(&mut self, cx: &mut Cx2d) {
        self.update_draw_vars(cx);
        cx.new_draw_call(&self.draw_vars);
    }

    pub fn append_to_draw_call(&self, cx: &mut Cx2d) {
        cx.append_to_draw_call(&self.draw_vars);
    }

    pub fn get_aa_pad_px(&self, cx: &mut Cx) -> f32 {
        let mut value = [0.0];
        self.draw_vars
            .get_uniform(cx, live_id!(aa_pad_px), &mut value);
        value[0]
    }
}

fn mat4_row(mat: &Mat4f, row: usize) -> [f32; 4] {
    [mat.v[row], mat.v[row + 4], mat.v[row + 8], mat.v[row + 12]]
}

#[derive(Debug, Clone, Script, ScriptHook)]
pub struct TextStyle {
    #[live]
    pub font_family: FontFamily,
    #[live(10.0)]
    pub font_size: f32,
    #[live(1.0)]
    pub line_spacing: f32,
    /// A vertical offset applied when drawing text, as a fraction of the font size.
    /// Positive values shift text downward, useful for aligning baselines when
    /// mixing fonts with different vertical metrics (e.g., code font with regular text).
    #[live(0.0)]
    pub top_drop: f32,
}

#[derive(Debug, Clone, Script, ScriptHook)]
pub struct FontMember {
    #[live]
    pub res: Option<ScriptHandleRef>,
    #[live]
    pub asc: f32,
    #[live]
    pub desc: f32,
    /// Positive values map to the OpenType `wght` axis. `0.0` keeps the font default.
    #[live(0.0)]
    pub weight: f32,
}

#[derive(Debug, Clone, Script, PartialEq)]
pub struct FontFamily {
    #[rust]
    id: LiveId,
    #[rust]
    members: Vec<FontMemberDef>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FontMemberDef {
    handle: ScriptHandle,
    asc: f32,
    desc: f32,
    weight: f32,
}

impl FontFamily {
    fn to_font_family_id(&self) -> FontFamilyId {
        (self.id.0).into()
    }

    fn update_font_definitions(&self, cx: &mut Cx, fonts: &mut Fonts, text: Option<&str>) {
        let mut font_ids = Vec::new();

        for member in &self.members {
            if !font_member_is_needed_for_text(cx, member.handle, text) {
                continue;
            }
            let font_id = font_member_font_id(member);

            if !fonts.is_font_known(font_id) {
                let font_data = cx.get_resource_font_bytes(member.handle);

                if let Some(data) = font_data {
                    fonts.define_font(
                        font_id,
                        FontDefinition {
                            data,
                            index: 0,
                            ascender_fudge_in_ems: member.asc,
                            descender_fudge_in_ems: member.desc,
                            weight: font_member_weight(member),
                            variations: Vec::new(),
                        },
                    );
                }
            }

            if fonts.is_font_known(font_id) {
                font_ids.push(font_id);
            }
        }

        fonts.set_font_family_definition(
            self.to_font_family_id(),
            FontFamilyDefinition {
                font_ids,
                expected_member_count: self.members.len(),
            },
        );
    }

    fn ensure_fonts_loaded_for_text(&self, cx: &mut Cx, text: Option<&str>) {
        CxDraw::lazy_construct_fonts(cx);

        let family_id = self.to_font_family_id();
        let fonts = cx.get_global::<Rc<RefCell<Fonts>>>().clone();

        {
            let fonts_ref = fonts.borrow();
            if fonts_ref.is_font_family_complete(family_id) {
                return;
            }
        }

        // Slow path: request only the resources needed by this family, then re-check.
        for member in &self.members {
            if !font_member_is_needed_for_text(cx, member.handle, text) {
                continue;
            }
            cx.load_script_resource(member.handle);
        }
        {
            let fonts_ref = fonts.borrow();
            if fonts_ref.is_font_family_complete(family_id) {
                return;
            }
        }

        let mut fonts_ref = fonts.borrow_mut();
        self.update_font_definitions(cx, &mut fonts_ref, text);
    }

    fn ensure_fonts_loaded(&self, cx: &mut Cx) {
        self.ensure_fonts_loaded_for_text(cx, None);
    }
}

fn font_member_is_needed_for_text(cx: &Cx, handle: ScriptHandle, text: Option<&str>) -> bool {
    let Some(text) = text else {
        return true;
    };
    let Some(path) = cx.get_resource_abs_path(handle) else {
        return true;
    };

    if is_cjk_fallback_font_path(&path) {
        return text_has_cjk(text);
    }
    if is_emoji_fallback_font_path(&path) {
        return text_has_emoji(text);
    }
    true
}

fn is_cjk_fallback_font_path(path: &str) -> bool {
    matches!(
        resource_basename(path),
        Some(name)
            if name.eq_ignore_ascii_case("LXGWWenKaiRegular.ttf")
                || name.eq_ignore_ascii_case("LXGWWenKaiBold.ttf")
    )
}

fn is_emoji_fallback_font_path(path: &str) -> bool {
    matches!(
        resource_basename(path),
        Some(name) if name.eq_ignore_ascii_case("NotoColorEmoji.ttf")
    )
}

fn resource_basename(path: &str) -> Option<&str> {
    path.rsplit(['/', '\\']).next()
}

fn font_member_weight(member: &FontMemberDef) -> Option<f32> {
    if member.weight.is_finite() && member.weight > 0.0 {
        Some(member.weight)
    } else {
        None
    }
}

fn font_member_font_id(member: &FontMemberDef) -> FontId {
    let mut hasher = DefaultHasher::new();
    member.handle.index().hash(&mut hasher);
    member.asc.to_bits().hash(&mut hasher);
    member.desc.to_bits().hash(&mut hasher);
    member.weight.to_bits().hash(&mut hasher);
    FontId::from(hasher.finish())
}

fn text_has_cjk(text: &str) -> bool {
    text.chars().any(is_cjk_char)
}

fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2E80..=0x2FFF
            | 0x3000..=0x303F
            | 0x3040..=0x30FF
            | 0x3100..=0x312F
            | 0x31A0..=0x31EF
            | 0x1100..=0x11FF
            | 0x3130..=0x318F
            | 0xAC00..=0xD7AF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFFEF
            | 0x20000..=0x2EE5F
            | 0x2F800..=0x2FA1F
    )
}

fn text_has_emoji(text: &str) -> bool {
    text.chars().any(is_emoji_char)
}

fn is_emoji_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2600..=0x27BF | 0x200D | 0xFE0F | 0x1F000..=0x1FAFF | 0x1FB00..=0x1FBFF
    )
}

fn row_span_x_bounds_in_lpxs(
    row: &LaidoutRow,
    is_first_row: bool,
    _is_last_row: bool,
) -> (f32, f32) {
    let start_x_in_lpxs = if is_first_row {
        row.glyphs
            .first()
            .map(|glyph| glyph.origin_in_lpxs.x)
            .unwrap_or(row.width_in_lpxs)
    } else {
        0.0
    };
    let end_x_in_lpxs = row.width_in_lpxs;
    (start_x_in_lpxs, end_x_in_lpxs.max(start_x_in_lpxs))
}

impl TextStyle {
    pub fn font_family_id(&self) -> FontFamilyId {
        self.font_family.to_font_family_id()
    }

    pub fn ensure_fonts_loaded(&self, cx: &mut Cx) {
        self.font_family.ensure_fonts_loaded(cx);
    }

    pub fn ensure_fonts_loaded_for_text(&self, cx: &mut Cx, text: &str) {
        self.font_family
            .ensure_fonts_loaded_for_text(cx, Some(text));
    }
}

impl ScriptHook for FontFamily {
    fn on_custom_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) -> bool {
        let Some(obj) = value.as_object() else {
            return false;
        };

        // Use the object index as the unique id
        self.id = LiveId(obj.index() as u64);
        self.members.clear();

        let len = vm.bx.heap.vec_len(obj);
        for i in 0..len {
            let kv = vm.bx.heap.vec_key_value(obj, i, NoTrap);
            let member = FontMember::script_from_value(vm, kv.value);
            if let Some(ref handle_ref) = member.res {
                self.members.push(FontMemberDef {
                    handle: handle_ref.as_handle(),
                    asc: member.asc,
                    desc: member.desc,
                    weight: member.weight,
                });
            }
        }

        // Don't eagerly register fonts here. Font registration is deferred
        // to ensure_fonts_loaded() which is called at draw time.
        // This avoids redundant work when the same FontFamily is applied
        // to hundreds of widgets.

        true
    }
}

#[cfg(test)]
mod tests {
    use super::DrawText;

    #[test]
    fn draw_text_size_stays_16_byte_aligned() {
        assert_eq!(std::mem::size_of::<DrawText>() % 16, 0);
    }
}
