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
            slug_atlas::SlugGlyphCacheResult,
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

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn register_draw_text_slug(vm: &mut ScriptVm) {
    let slug_shader = DrawTextSlug::script_shader(vm);
    let script_mod = script! {
        use mod.pod.*
        use mod.math.*
        use mod.shader.*
        use mod.draw
        use mod.geom
        use mod.res.*

        mod.std.set_type_default() do #(slug_shader){
            async_compile: true

            vertex_pos: vertex_position(vec4f)
            fb0: fragment_output(0, vec4f)

            draw_call: uniform_buffer(draw.DrawCallUniforms)
            draw_pass: uniform_buffer(draw.DrawPassUniforms)
            draw_list: uniform_buffer(draw.DrawListUniforms)

            geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)

            curve_texture: texture_2d(float)
            band_texture: texture_2d(float)

            color: #fff
            hover: instance(0.0)
            focus: instance(0.0)
            down: instance(0.0)
            disabled: instance(0.0)
            empty: instance(0.0)
            active: instance(0.0)
            drag: instance(0.0)
            pressed: instance(0.0)
            opened: instance(0.0)
            focussed: instance(0.0)
            is_even: instance(0.0)
            is_folder: instance(0.0)
            scale: instance(1.0)

            color_hover: uniform(#fff)
            color_focus: uniform(#fff)
            color_down: uniform(#fff)
            color_disabled: uniform(#fff)
            color_empty: uniform(#fff)
            color_empty_hover: uniform(#fff)
            color_empty_focus: uniform(#fff)
            color_active: uniform(#fff)
            color_drag: uniform(#fff)
            color_pressed: uniform(#fff)

            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            color_2_hover: uniform(#fff)
            color_2_focus: uniform(#fff)
            color_2_down: uniform(#fff)
            color_2_disabled: uniform(#fff)
            color_2_empty: uniform(#fff)
            color_2_empty_hover: uniform(#fff)
            color_2_empty_focus: uniform(#fff)
            color_2_active: uniform(#fff)
            color_2_drag: uniform(#fff)
            color_2_pressed: uniform(#fff)
            use_color_2: uniform(0.0)

            color_dither: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)
            total_chars: instance(1000000.0)
            aa_pad_px: uniform(float(1.0))
            slug_visibility: uniform(1.0)
            slug_matrix_0: uniform(vec4(1.0, 0.0, 0.0, 0.0))
            slug_matrix_1: uniform(vec4(0.0, 1.0, 0.0, 0.0))
            slug_matrix_3: uniform(vec4(0.0, 0.0, 0.0, 1.0))
            slug_viewport_px: uniform(vec2(1.0, 1.0))

            pos: varying(vec2f)
            world: varying(vec4f)

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
                let p = mix(self.rect_pos, self.rect_pos + self.rect_size, self.geom.pos)
                let p_clipped = clamp(p, self.draw_clip.xy, self.draw_clip.zw)
                let pad_lpx = self.aa_pad_px / max(self.draw_pass.dpi_factor, 0.0001)
                let content_rect_pos = self.rect_pos + vec2(pad_lpx, pad_lpx)
                let content_rect_size = vec2(
                    max(self.rect_size.x - 2.0 * pad_lpx, 0.0001),
                    max(self.rect_size.y - 2.0 * pad_lpx, 0.0001)
                )
                self.pos = vec2(
                    (p_clipped.x - content_rect_pos.x) / content_rect_size.x,
                    (p_clipped.y - content_rect_pos.y) / content_rect_size.y
                )
                self.world = self.draw_list.view_transform * vec4(
                    p_clipped.x,
                    p_clipped.y,
                    self.glyph_depth + self.draw_call.zbias,
                    1.
                )
                self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
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
                // Linux uses the shared helper shader for SLUG fallback promotion.
                // Prefer the simpler full-curve scan here to avoid correctness issues
                // in the helper's band-accelerated path for some glyph shapes.
                let x_scan = self.scan_horizontal_all(sample, px_x)
                let y_scan = self.scan_vertical_all(sample, px_y)
                return self.calc_coverage(x_scan.x, y_scan.x, x_scan.y, y_scan.y)
            }

            fragment: fn() {
                self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
            }

            get_color: fn() {
                let mut color = self.color
                let mut color_hover = self.color_hover
                let mut color_focus = self.color_focus
                let mut color_down = self.color_down
                let mut color_disabled = self.color_disabled
                let mut color_empty = self.color_empty
                let mut color_empty_hover = self.color_empty_hover
                let mut color_empty_focus = self.color_empty_focus
                let mut color_active = self.color_active
                let mut color_drag = self.color_drag
                let mut color_pressed = self.color_pressed

                if (self.use_color_2 > 0.5) {
                    let mut gradient_fill_dir = self.pos.y
                    if (self.gradient_fill_horizontal > 0.5) {
                        gradient_fill_dir = self.pos.x
                    }
                    color = mix(self.color, self.color_2, gradient_fill_dir)
                    color_hover = mix(self.color_hover, self.color_2_hover, gradient_fill_dir)
                    color_focus = mix(self.color_focus, self.color_2_focus, gradient_fill_dir)
                    color_down = mix(self.color_down, self.color_2_down, gradient_fill_dir)
                    color_disabled = mix(self.color_disabled, self.color_2_disabled, gradient_fill_dir)
                    color_empty = mix(self.color_empty, self.color_2_empty, gradient_fill_dir)
                    color_empty_hover = mix(self.color_empty_hover, self.color_2_empty_hover, gradient_fill_dir)
                    color_empty_focus = mix(self.color_empty_focus, self.color_2_empty_focus, gradient_fill_dir)
                    color_active = mix(self.color_active, self.color_2_active, gradient_fill_dir)
                    color_drag = mix(self.color_drag, self.color_2_drag, gradient_fill_dir)
                    color_pressed = mix(self.color_pressed, self.color_2_pressed, gradient_fill_dir)
                }

                return color
                    .mix(
                        color_empty
                            .mix(color_empty_hover, self.hover)
                            .mix(color_empty_focus, max(self.focus, self.focussed)),
                        self.empty
                    )
                    .mix(color_focus, max(self.focus, self.focussed) * (1.0 - self.empty))
                    .mix(color_active, max(self.active, self.opened))
                    .mix(color_hover, self.hover)
                    .mix(color_down, self.down)
                    .mix(color_drag, self.drag)
                    .mix(color_pressed, self.pressed)
                    .mix(color_disabled, self.disabled)
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

            pixel: fn() {
                return self.sample_slug_pixel() * self.slug_visibility
            }
        }
    };
    vm.eval(script_mod);
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
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
        total_chars: instance(1000000.0)

        grayscale_texture: texture_2d(float)
        color_texture: texture_2d(float)
        msdf_texture: texture_2d(float)

        vertex: fn() {
            let p = mix(self.rect_pos, self.rect_pos + self.rect_size, self.geom.pos)
            let p_clipped = clamp(p, self.draw_clip.xy, self.draw_clip.zw)
            let p_normalized = (p_clipped - self.rect_pos) / self.rect_size

            self.pos = p_normalized
            self.t = mix(self.t_min, self.t_max, p_normalized.xy)
            self.world = self.draw_list.view_transform * vec4(
                p_clipped.x,
                p_clipped.y,
                self.glyph_depth + self.draw_call.zbias,
                1.
            )
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * (self.world))
        }

        sdf: fn(scale, p, color) {
            let sampled = self.grayscale_texture.sample_as_bgra(p)
            let s = if self.atlas_plane < 0.5 {
                sampled.r
            } else if self.atlas_plane < 1.5 {
                sampled.g
            } else if self.atlas_plane < 2.5 {
                sampled.b
            } else {
                sampled.a
            }
            let safe_scale = max(scale, 0.0001)
            let luma = dot(color.rgb, vec3(0.299, 0.587, 0.114))
            var a = clamp(
                (s - (1.0 - self.cutoff)) * self.radius / safe_scale * self.sdf_sharpness + 0.5,
                0.0,
                1.0,
            )
            let bias = (0.5 - luma) * self.sdf_luma_bias
            a = clamp(a - bias, 0.0, 1.0)
            return a
        }

        msdf: fn(scale, p, color) {
            let s = self.msdf_texture.sample_as_bgra(p)
            let dist = s.a
            let safe_scale = max(scale, 0.0001)
            let luma = dot(color.rgb, vec3(0.299, 0.587, 0.114))
            var a = clamp(
                (dist - (1.0 - self.cutoff)) * self.radius / safe_scale * self.sdf_sharpness + 0.5,
                0.0,
                1.0,
            )
            let bias = (0.5 - luma) * self.sdf_luma_bias
            if a > self.sdf_luma_bias * 0.5 {
                a = clamp(a - bias, 0.0, 1.0)
            }
            return a
        }

        get_color: fn() {
            return self.color
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }

        sample_text_pixel: fn() {
            let dxt = length(dFdx(self.t))
            let dyt = length(dFdy(self.t))
            if self.texture_index < 0.5 {
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
            } else {
                let c = self.get_color()
                let scale = (dxt + dyt) * self.msdf_texture.size().x * 0.5
                let tex_size = self.msdf_texture.size()
                let half_texel = vec2(0.5 / tex_size.x, 0.5 / tex_size.y)
                let p = clamp(self.t.xy, self.t_min + half_texel, self.t_max - half_texel)
                let s = self.msdf(scale, p, c)
                return s * vec4(c.rgb * c.a, c.a)
            }
        }

        pixel: fn() {
            return self.sample_text_pixel()
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
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

#[derive(Script)]
#[repr(C)]
pub struct DrawText {
    #[rust]
    pub many_instances: Option<ManyInstances>,
    #[rust]
    pending_slug_flush_generation: u64,
    #[rust]
    slug_flush_defer_depth: u64,
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[rust]
    slug_draw: Option<DrawTextSlug>,
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[rust]
    slug_promotion: SlugPromotionState,
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[rust]
    slug_sync_plan: SlugDrawSyncPlan,
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[rust]
    slug_layout_pad: u64,
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

    /// Per-row horizontal alignment applied by the text layouter when the
    /// text does not fill the full `max_width_in_lpxs`. `x: 0.0` = left,
    /// `0.5` = center, `1.0` = right. `y` is currently unused by the
    /// layouter. Reset to `Align::default()` per draw by callers that care.
    #[rust]
    pub layout_align: Align,

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

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawTextSlug {
    #[rust]
    many_instances: Option<ManyInstances>,
    #[deref]
    draw_vars: DrawVars,
    #[live]
    rect_pos: Vec2f,
    #[live]
    rect_size: Vec2f,
    #[live]
    draw_clip: Vec4f,
    #[live(1.0)]
    depth_clip: f32,
    #[live]
    glyph_depth: f32,
    #[live]
    texture_index: f32,
    #[live]
    char_index: f32,
    #[live(vec4(1., 1., 1., 1.))]
    color: Vec4f,
    #[live(1.0)]
    sdf_sharpness: f32,
    #[live(0.03)]
    sdf_luma_bias: f32,
    #[live]
    t_min: Vec2f,
    #[live]
    t_max: Vec2f,
    #[live]
    atlas_plane: f32,
    #[live]
    pad1: f32,
    #[live(0.0)]
    aa_2x2: f32,
    #[live(0.0)]
    aa_4x4: f32,
    #[live(0.2)]
    stem_darken: f32,
    #[live(0.125)]
    stem_darken_max: f32,
}

#[derive(Clone, Copy, Debug)]
enum ResolvedGlyph {
    Raster(RasterizedGlyph),
    Slug(crate::text::slug_atlas::SlugGlyphInfo),
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
const SLUG_HELPER_BUILDS_PER_REDRAW: usize = 1;

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[derive(Default)]
struct SlugHelperWarmupState {
    redraw_id: u64,
    builds_this_redraw: usize,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[derive(Default)]
struct SlugHelperPrewarmState {
    registered: bool,
    initialized: bool,
    requested_redraw_id: u64,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[derive(Default)]
struct SlugPromotionState {
    redraw_id: u64,
    saw_slug_candidates_this_redraw: bool,
    saw_unready_this_redraw: bool,
    allow_slug_this_redraw: bool,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[derive(Default)]
struct SlugDrawSyncPlan {
    source_shader_id: Option<usize>,
    target_shader_id: Option<usize>,
    instance_ids: Vec<LiveId>,
    uniform_ids: Vec<LiveId>,
    source_has_color_2: bool,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl SlugDrawSyncPlan {
    fn ensure(&mut self, cx: &Cx, source_shader_id: usize, target_shader_id: usize) {
        if self.source_shader_id == Some(source_shader_id)
            && self.target_shader_id == Some(target_shader_id)
        {
            return;
        }

        self.source_shader_id = Some(source_shader_id);
        self.target_shader_id = Some(target_shader_id);
        self.instance_ids.clear();
        self.uniform_ids.clear();

        let source_shader = &cx.draw_shaders.shaders[source_shader_id];
        let target_shader = &cx.draw_shaders.shaders[target_shader_id];
        let source_dyn_instances = &source_shader.mapping.dyn_instances.inputs;
        let target_dyn_instances = &target_shader.mapping.dyn_instances.inputs;
        let source_dyn_uniforms = &source_shader.mapping.dyn_uniforms.inputs;
        let target_dyn_uniforms = &target_shader.mapping.dyn_uniforms.inputs;

        let source_has_instance = |id| source_dyn_instances.iter().any(|input| input.id == id);
        let target_has_instance = |id| target_dyn_instances.iter().any(|input| input.id == id);
        let source_has_uniform = |id| source_dyn_uniforms.iter().any(|input| input.id == id);
        let target_has_uniform = |id| target_dyn_uniforms.iter().any(|input| input.id == id);

        for id in [
            live_id!(hover),
            live_id!(focus),
            live_id!(down),
            live_id!(disabled),
            live_id!(empty),
            live_id!(active),
            live_id!(drag),
            live_id!(pressed),
            live_id!(opened),
            live_id!(focussed),
            live_id!(is_even),
            live_id!(is_folder),
            live_id!(scale),
            live_id!(total_chars),
        ] {
            if source_has_instance(id) && target_has_instance(id) {
                self.instance_ids.push(id);
            }
        }

        for id in [
            live_id!(aa_pad_px),
            live_id!(color_hover),
            live_id!(color_focus),
            live_id!(color_down),
            live_id!(color_disabled),
            live_id!(color_empty),
            live_id!(color_empty_hover),
            live_id!(color_empty_focus),
            live_id!(color_active),
            live_id!(color_drag),
            live_id!(color_pressed),
            live_id!(color_2),
            live_id!(color_2_hover),
            live_id!(color_2_focus),
            live_id!(color_2_down),
            live_id!(color_2_disabled),
            live_id!(color_2_empty),
            live_id!(color_2_empty_hover),
            live_id!(color_2_empty_focus),
            live_id!(color_2_active),
            live_id!(color_2_drag),
            live_id!(color_2_pressed),
            live_id!(color_dither),
            live_id!(gradient_fill_horizontal),
        ] {
            if source_has_uniform(id) && target_has_uniform(id) {
                self.uniform_ids.push(id);
            }
        }

        self.source_has_color_2 = source_has_uniform(live_id!(color_2));
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn slug_try_consume_helper_build_budget(cx: &mut Cx) -> bool {
    let redraw_id = cx.redraw_id;
    let state = cx.global::<SlugHelperWarmupState>();
    if state.redraw_id != redraw_id {
        state.redraw_id = redraw_id;
        state.builds_this_redraw = 0;
    }
    if state.builds_this_redraw >= SLUG_HELPER_BUILDS_PER_REDRAW {
        return false;
    }
    state.builds_this_redraw += 1;
    true
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn slug_register_helper_if_needed(cx: &mut Cx) {
    let should_register = {
        let state = cx.global::<SlugHelperPrewarmState>();
        if state.registered {
            false
        } else {
            state.registered = true;
            true
        }
    };
    if should_register {
        cx.with_vm(register_draw_text_slug);
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn slug_maybe_prewarm_helper(cx: &mut Cx2d) -> bool {
    enum PrewarmAction {
        Ready,
        RequestFollowupRedraw,
        TryPrewarm,
    }

    let redraw_id = cx.cx.redraw_id;
    let action = {
        let state = cx.cx.global::<SlugHelperPrewarmState>();
        if state.initialized {
            PrewarmAction::Ready
        } else if state.requested_redraw_id == 0 {
            state.requested_redraw_id = redraw_id;
            PrewarmAction::RequestFollowupRedraw
        } else if state.requested_redraw_id == redraw_id {
            PrewarmAction::RequestFollowupRedraw
        } else {
            PrewarmAction::TryPrewarm
        }
    };

    match action {
        PrewarmAction::Ready => true,
        PrewarmAction::RequestFollowupRedraw => {
            // A real SLUG glyph was requested; keep this frame on the raster
            // fallback and start warming the shared helper on the next redraw.
            cx.redraw_all();
            false
        }
        PrewarmAction::TryPrewarm => {
            if !slug_try_consume_helper_build_budget(cx.cx) {
                cx.redraw_all();
                return false;
            }
            slug_register_helper_if_needed(cx.cx);
            {
                let state = cx.cx.global::<SlugHelperPrewarmState>();
                if state.initialized {
                    return true;
                }
                state.initialized = true;
            }
            cx.cx.with_vm(|vm| {
                let _ = DrawTextSlug::script_new_with_default(vm);
            });
            cx.redraw_all();
            false
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl DrawText {
    fn slug_run_is_ready(&mut self, cx: &mut Cx2d, text: &LaidoutText) -> bool {
        let dpi_factor = cx.current_dpi_factor() as f32;
        let redraw_id = cx.cx.redraw_id;
        let mut has_slug_candidates = false;
        let mut all_slug_candidates_ready = true;
        let mut pending_slug_generation = 0;

        if self.slug_promotion.redraw_id != redraw_id {
            self.slug_promotion.allow_slug_this_redraw = self.slug_promotion.redraw_id != 0
                && self.slug_promotion.saw_slug_candidates_this_redraw
                && !self.slug_promotion.saw_unready_this_redraw;
            self.slug_promotion.redraw_id = redraw_id;
            self.slug_promotion.saw_slug_candidates_this_redraw = false;
            self.slug_promotion.saw_unready_this_redraw = false;
        }

        for row in &text.rows {
            for glyph in &row.glyphs {
                let font_size_in_dpxs = glyph.font_size_in_lpxs * dpi_factor;
                if glyph
                    .font
                    .has_glyph_raster_image(glyph.id, font_size_in_dpxs)
                {
                    continue;
                }
                if !cx.fonts.borrow().should_use_slug_glyph(font_size_in_dpxs) {
                    continue;
                }

                match cx.fonts.borrow_mut().get_or_cache_slug_glyph(
                    redraw_id,
                    glyph.font.as_ref(),
                    glyph.id,
                ) {
                    SlugGlyphCacheResult::Ready(..) => {
                        has_slug_candidates = true;
                    }
                    SlugGlyphCacheResult::NeedsUpload { generation, .. } => {
                        has_slug_candidates = true;
                        all_slug_candidates_ready = false;
                        pending_slug_generation = pending_slug_generation.max(generation);
                    }
                    SlugGlyphCacheResult::Deferred => {
                        has_slug_candidates = true;
                        all_slug_candidates_ready = false;
                    }
                    SlugGlyphCacheResult::Unavailable => {}
                }
            }
        }

        if pending_slug_generation != 0 {
            self.pending_slug_flush_generation = self
                .pending_slug_flush_generation
                .max(pending_slug_generation);
        }

        if !has_slug_candidates {
            return false;
        }

        self.slug_promotion.saw_slug_candidates_this_redraw = true;

        if !all_slug_candidates_ready {
            self.slug_promotion.saw_unready_this_redraw = true;
            cx.redraw_all();
            return false;
        }

        if !slug_maybe_prewarm_helper(cx) {
            self.slug_promotion.saw_unready_this_redraw = true;
            return false;
        }

        if !self.ensure_slug_draw(cx) {
            self.slug_promotion.saw_unready_this_redraw = true;
            cx.redraw_all();
            return false;
        }

        if !self.slug_promotion.allow_slug_this_redraw {
            cx.redraw_all();
            return false;
        }

        true
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl ScriptHook for DrawText {
    fn on_after_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_from_script() {
            self.slug_draw = None;
            self.slug_promotion = Default::default();
            self.slug_sync_plan = Default::default();
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
impl ScriptHook for DrawText {}

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

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl DrawTextSlug {
    fn has_open_batch(&self) -> bool {
        self.many_instances.is_some()
    }

    fn begin_many_instances(&mut self, cx: &mut Cx2d, visibility: f32) -> bool {
        if self.many_instances.is_some() {
            return true;
        }
        self.update_draw_vars(cx);
        self.draw_vars
            .set_uniform(cx.cx, live_id!(slug_visibility), &[visibility]);
        self.many_instances = cx.begin_many_aligned_instances(&self.draw_vars);
        self.many_instances.is_some()
    }

    fn end_many_instances(&mut self, cx: &mut Cx2d, extend_area: bool) {
        if let Some(instances) = self.many_instances.take() {
            self.finish_many_instances(cx, instances, extend_area);
        }
    }

    fn finish_many_instances(
        &mut self,
        cx: &mut Cx2d,
        instances: ManyInstances,
        extend_area: bool,
    ) {
        let new_area = cx.end_many_instances(instances);
        let old_area = self.draw_vars.area;
        if extend_area {
            let extended = old_area.extend_with(cx, new_area);
            self.draw_vars.area = cx.update_area_refs(old_area, extended);
        } else {
            self.draw_vars.area = cx.update_area_refs(old_area, new_area);
        }
    }

    fn update_draw_vars(&mut self, cx: &mut Cx2d) {
        self.draw_vars.append_group_id = cx.draw_call_group_content().0;
        let fonts = cx.fonts.borrow();
        self.draw_vars.texture_slots[0] = Some(fonts.slug_curve_texture().clone());
        self.draw_vars.texture_slots[1] = Some(fonts.slug_band_texture().clone());
        for slot in self.draw_vars.texture_slots.iter_mut().skip(2) {
            *slot = None;
        }

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

    fn draw_slug_glyph(
        &mut self,
        cx: &mut Cx2d,
        draw_text: &DrawText,
        origin_in_lpxs: Point<f32>,
        font_size_in_lpxs: f32,
        color: Option<Color>,
        glyph: crate::text::slug_atlas::SlugGlyphInfo,
    ) {
        let Some(instances) = self.many_instances.as_mut() else {
            return;
        };

        let bounds_in_lpxs = TextRect::new(
            Point::new(
                origin_in_lpxs.x + glyph.origin_in_ems.x * font_size_in_lpxs * draw_text.font_scale,
                origin_in_lpxs.y
                    + (-glyph.origin_in_ems.y - glyph.size_in_ems.height)
                        * font_size_in_lpxs
                        * draw_text.font_scale,
            ),
            Size::new(
                glyph.size_in_ems.width * font_size_in_lpxs * draw_text.font_scale,
                glyph.size_in_ems.height * font_size_in_lpxs * draw_text.font_scale,
            ),
        );

        let pad = (draw_text.get_aa_pad_px(cx.cx) / cx.current_dpi_factor() as f32).max(0.0);
        self.rect_pos = vec2(bounds_in_lpxs.origin.x - pad, bounds_in_lpxs.origin.y - pad)
            + vec2(0.0, draw_text.temp_y_shift * font_size_in_lpxs);
        self.rect_size = vec2(
            bounds_in_lpxs.size.width + pad * 2.0,
            bounds_in_lpxs.size.height + pad * 2.0,
        );
        self.draw_clip = draw_text.draw_clip;
        self.depth_clip = draw_text.depth_clip;
        self.glyph_depth = draw_text.glyph_depth;
        self.texture_index = 3.0;
        self.char_index = draw_text.char_index;
        let mut source_color = [
            draw_text.color.x,
            draw_text.color.y,
            draw_text.color.z,
            draw_text.color.w,
        ];
        if !draw_text
            .draw_vars
            .get_instance_on_area(cx.cx, live_id!(color), &mut source_color)
        {
            draw_text
                .draw_vars
                .get_instance(cx.cx, live_id!(color), &mut source_color);
        }
        self.color = vec4(
            source_color[0],
            source_color[1],
            source_color[2],
            source_color[3],
        );
        self.sdf_sharpness = draw_text.sdf_sharpness;
        self.sdf_luma_bias = draw_text.sdf_luma_bias;
        if let Some(color) = color {
            self.color = vec4(
                color.r as f32,
                color.g as f32,
                color.b as f32,
                color.a as f32,
            ) / 255.0;
        }
        self.t_min = vec2(glyph.curve_offset as f32, glyph.curve_count as f32);
        self.t_max = vec2(glyph.band_offset as f32, glyph.band_count as f32);
        self.atlas_plane = glyph.fill_flags as f32;
        self.pad1 = 0.0;
        self.aa_2x2 = draw_text.aa_2x2;
        self.aa_4x4 = draw_text.aa_4x4;
        self.stem_darken = draw_text.stem_darken;
        self.stem_darken_max = draw_text.stem_darken_max;
        self.draw_vars
            .set_dyn_instance(cx.cx, live_id!(scale), &[draw_text.font_scale]);

        instances
            .instances
            .extend_from_slice(self.draw_vars.as_slice());
    }
}

impl DrawText {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn slug_draw_is_ready(&self, cx: &mut Cx2d) -> bool {
        let Some(shader_id) = self
            .slug_draw
            .as_ref()
            .and_then(|slug_draw| slug_draw.draw_vars.draw_shader_id)
        else {
            return false;
        };
        cx.cx.is_draw_shader_window_ready(shader_id)
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn ensure_slug_draw(&mut self, cx: &mut Cx2d) -> bool {
        if self.slug_draw.is_some() {
            self.sync_slug_draw_state(cx);
            return self.slug_draw_is_ready(cx);
        }

        slug_register_helper_if_needed(cx.cx);
        let mut created = None;

        cx.cx.with_vm(|vm| {
            let slug_draw = DrawTextSlug::script_new_with_default(vm);
            created = Some(slug_draw);
        });

        self.slug_draw = created;
        if self.slug_draw.is_some() {
            if let Some(slug_draw) = self.slug_draw.as_mut() {
                slug_draw.draw_vars.options = self.draw_vars.options.clone();
            }
            self.sync_slug_draw_state(cx);
            return self.slug_draw_is_ready(cx);
        }
        false
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn sync_slug_draw_state(&mut self, cx: &mut Cx2d) {
        let Some(source_shader_id) = self.draw_vars.draw_shader_id else {
            return;
        };
        let Some(target_shader_id) = self
            .slug_draw
            .as_ref()
            .and_then(|slug_draw| slug_draw.draw_vars.draw_shader_id)
        else {
            return;
        };
        self.slug_sync_plan
            .ensure(cx.cx, source_shader_id.index, target_shader_id.index);
        let Some(slug_draw) = self.slug_draw.as_mut() else {
            return;
        };
        slug_draw.draw_vars.options = self.draw_vars.options.clone();

        for &id in &self.slug_sync_plan.instance_ids {
            let mut value = [0.0; 4];
            self.draw_vars.get_instance(cx.cx, id, &mut value);
            slug_draw.draw_vars.set_dyn_instance(cx.cx, id, &value);
        }

        for &id in &self.slug_sync_plan.uniform_ids {
            let mut value = [0.0; 4];
            self.draw_vars.get_uniform(cx.cx, id, &mut value);
            slug_draw.draw_vars.set_uniform(cx.cx, id, &value);
        }

        let mut color_2 = [-1.0; 4];
        if self.slug_sync_plan.source_has_color_2 {
            self.draw_vars
                .get_uniform(cx.cx, live_id!(color_2), &mut color_2);
        }
        slug_draw.draw_vars.set_uniform(
            cx.cx,
            live_id!(use_color_2),
            &[if color_2[0] > -0.5 { 1.0 } else { 0.0 }],
        );
    }

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
            self.flush_slug_textures_if_allowed(cx);
            self.finish_many_instances(cx, instances);
        }
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if let Some(mut slug_draw) = self.slug_draw.take() {
            slug_draw.end_many_instances(cx, self.extend_area);
            self.slug_draw = Some(slug_draw);
        }
    }

    /// Defers SLUG atlas uploads while preserving the original draw-call order.
    ///
    /// This is useful for composite text widgets like `TextFlow`, which issue
    /// many separate `DrawText` runs interleaved with non-text draw calls. Those
    /// widgets need the glyph uploads to be coalesced, but cannot safely hold a
    /// single `many_instances` batch open across the whole widget.
    pub fn begin_deferred_slug_flush(&mut self) {
        self.slug_flush_defer_depth = self.slug_flush_defer_depth.saturating_add(1);
    }

    pub fn end_deferred_slug_flush(&mut self, cx: &mut Cx2d) {
        if self.slug_flush_defer_depth == 0 {
            return;
        }
        self.slug_flush_defer_depth -= 1;
        self.flush_slug_textures_if_allowed(cx);
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
        let mut max_width_in_lpxs = self.max_layout_width_for_walk(cx, walk);

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

        let wrap = matches!(cx.turtle().layout().flow, Flow::Right { wrap: true, .. });

        let text = self.layout(cx, 0.0, 0.0, max_width_in_lpxs, wrap, align, text);
        self.draw_walk_laidout(cx, walk, &text)
    }

    fn max_layout_width_for_walk(&self, cx: &mut Cx2d, walk: Walk) -> Option<f32> {
        let width = cx.turtle().max_width(walk).or_else(|| {
            let turtle_rect = cx.turtle().inner_rect();
            (!turtle_rect.size.x.is_nan()).then_some(turtle_rect.size.x)
        })?;

        Some((width.max(0.0) as f32) / self.font_scale.max(0.0001))
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
    ///
    /// When text wraps to multiple visual rows, each row is emitted as its own
    /// `FinishedWalk` with a single-row `outer_size.y`. Between rows,
    /// `turtle_new_line_with_spacing` is called to finish the previous visual
    /// row (triggering `finish_row` → `RowAlign::Center`, etc.) and position
    /// the turtle for the next row. This lets per-row alignment work correctly
    /// when inline block widgets (pills, badges, etc.) share a visual row with
    /// text of a different height.
    pub fn draw_walk_resumable_with(
        &mut self,
        cx: &mut Cx2d,
        text_str: &str,
        mut f: impl FnMut(&mut Cx2d, Rect, f32),
    ) -> (usize, bool) {
        self.draw_walk_resumable_with_inner(cx, text_str, false, &mut f)
    }

    /// Like [`Self::draw_walk_resumable_with`], but invokes the callback before
    /// emitting glyphs. Use this for backgrounds that must sit behind text.
    pub fn draw_walk_resumable_with_background(
        &mut self,
        cx: &mut Cx2d,
        text_str: &str,
        mut f: impl FnMut(&mut Cx2d, Rect, f32),
    ) -> (usize, bool) {
        self.draw_walk_resumable_with_inner(cx, text_str, true, &mut f)
    }

    fn draw_walk_resumable_with_inner(
        &mut self,
        cx: &mut Cx2d,
        text_str: &str,
        callback_before_text: bool,
        f: &mut impl FnMut(&mut Cx2d, Rect, f32),
    ) -> (usize, bool) {
        // ── Layout parameters ──
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
        let wrap = matches!(cx.turtle().layout().flow, Flow::Right { wrap: true, .. });

        // ── Text layout ──
        let text = self.layout(
            cx,
            first_row_indent_in_lpxs,
            row_height as f32,
            max_width_in_lpxs,
            wrap,
            self.layout_align,
            text_str,
        );

        // ── Common computations ──
        let last_row = text.rows.last().unwrap();
        let new_turtle_pos = {
            let p = origin_in_lpxs
                + Size::new(
                    last_row.width_in_lpxs,
                    last_row.origin_in_lpxs.y - last_row.ascender_in_lpxs,
                ) * self.font_scale;
            dvec2(p.x as f64, p.y as f64)
        };
        let used_size_in_lpxs = text.size_in_lpxs * self.font_scale;
        let shift_y = text
            .rows
            .first()
            .and_then(|r| r.glyphs.first())
            .map(|g| g.font_size_in_lpxs * self.temp_y_shift)
            .unwrap_or(0.0);

        // ── Decide whether to use per-row glyph batching ──
        // Per-row batching gives each visual row its own AlignEntry so that
        // finish_row alignment shifts apply independently per row. This
        // requires a fresh draw (no `many_instances` reuse buffer).
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        let per_row = self.many_instances.is_none() && text.rows.len() > 1;
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        let per_row = false;

        if per_row {
            // ── PER-ROW path ──
            // Draw each visual row as a separate instance batch so its glyphs
            // get their own AlignEntry. Emit a FinishedWalk per row and call
            // turtle_new_line between rows so finish_row runs at each row
            // boundary — enabling RowAlign::Center to center text relative to
            // any inline widget (pill) on the same visual row.

            self.update_draw_vars(cx);
            self.glyph_depth = self.draw_depth;
            let saved_extend_area = self.extend_area;

            for (row_idx, row) in text.rows.iter().enumerate() {
                let row_h =
                    ((row.ascender_in_lpxs - row.descender_in_lpxs) * self.font_scale) as f64;

                // Between rows: finish the previous visual row (triggers
                // finish_row → RowAlign::Center) and position for the next.
                if row_idx > 0 {
                    let ws = cx.turtle().wrap_spacing();
                    cx.turtle_new_line_with_spacing(ws);
                    self.extend_area = true;
                }

                // Compute row_origin: for row 0 (continuation row), use the
                // layouter's position (correct indent on the existing line).
                // For wrapped rows (1+), use the TURTLE's current position so
                // glyphs land where the turtle tracks — not at the layouter's
                // position which doesn't account for taller inline widgets
                // (pills) inflating the previous row's height.
                let row_origin = if row_idx == 0 {
                    origin_in_lpxs + Size::from(row.origin_in_lpxs) * self.font_scale
                } else {
                    let tp = cx.turtle().pos();
                    Point::new(
                        tp.x as f32 + row.origin_in_lpxs.x * self.font_scale,
                        tp.y as f32 + row.ascender_in_lpxs * self.font_scale,
                    )
                };
                let row_top_y = (row_origin.y - row.ascender_in_lpxs * self.font_scale) as f64;

                let row_als = cx.align_list_len();
                let (sx, ex) =
                    row_span_x_bounds_in_lpxs(row, row_idx == 0, row_idx + 1 == text.rows.len());
                let row_rect = rect(
                    (row_origin.x + sx * self.font_scale) as f64,
                    row_top_y + shift_y as f64,
                    ((ex - sx) * self.font_scale) as f64,
                    row_h,
                );

                if callback_before_text {
                    f(cx, row_rect, row.ascender_in_lpxs);
                }

                // Draw this row's glyphs as a separate aligned-instance batch.
                if let Some(mut instances) = cx.begin_many_aligned_instances(&self.draw_vars) {
                    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
                    self.draw_row(cx, row_origin, row, &mut instances.instances);
                    #[cfg(any(target_os = "linux", target_os = "windows"))]
                    let _ = (row_origin, row, &mut instances.instances);
                    self.finish_many_instances(cx, instances);
                }

                // Update turtle allocation for this row.
                if row_idx == 0 {
                    let turtle = cx.turtle_mut();
                    turtle.move_to(dvec2(origin_in_lpxs.x as f64, origin_in_lpxs.y as f64));
                    turtle.allocate_width(used_size_in_lpxs.width as f64);
                    turtle.allocate_height(row_h);
                } else {
                    let turtle = cx.turtle_mut();
                    turtle.allocate_height(row_h);
                    turtle.allocate_width((row.width_in_lpxs * self.font_scale) as f64);
                }

                // Set wrap spacing from this row's line-spacing metrics.
                cx.turtle_mut().set_wrap_spacing(
                    (row.ascender_in_lpxs * row.line_spacing_scale - row.ascender_in_lpxs) as f64,
                );

                // Call the `f` callback for this row (draws strikethrough,
                // underline, or plain area tracking).
                if !callback_before_text {
                    f(cx, row_rect, row.ascender_in_lpxs);
                }

                // Emit a per-row FinishedWalk covering this row's glyphs +
                // callback rect-areas.
                cx.emit_turtle_walk(
                    Rect {
                        pos: dvec2(row_origin.x as f64, row_top_y),
                        size: dvec2((row.width_in_lpxs * self.font_scale) as f64, row_h),
                    },
                    row_als,
                );
            }

            // Position turtle at end of last row for subsequent inline content.
            // pos.y is already at the last row's top (from turtle_new_line).
            // Advance pos.x past the last row's text width.
            let last = text.rows.last().unwrap();
            let end_x = cx.turtle().pos().x + (last.width_in_lpxs * self.font_scale) as f64;
            let end_y = cx.turtle().pos().y;
            cx.turtle_mut().move_to(dvec2(end_x, end_y));
            self.extend_area = saved_extend_area;
            self.flush_slug_textures_if_allowed(cx);
        } else {
            // ── SINGLE-WALK path (single-row text, reuse mode, or Linux/Win) ──
            let align_list_start = cx.align_list_len();
            if callback_before_text {
                for (row_index, row) in text.rows.iter().enumerate() {
                    let (sx, ex) = row_span_x_bounds_in_lpxs(
                        row,
                        row_index == 0,
                        row_index + 1 == text.rows.len(),
                    );
                    f(
                        cx,
                        rect(
                            (origin_in_lpxs.x + (row.origin_in_lpxs.x + sx) * self.font_scale)
                                as f64,
                            (origin_in_lpxs.y
                                + (row.origin_in_lpxs.y - row.ascender_in_lpxs) * self.font_scale)
                                as f64
                                + shift_y as f64,
                            ((ex - sx) * self.font_scale) as f64,
                            ((row.ascender_in_lpxs - row.descender_in_lpxs) * self.font_scale)
                                as f64,
                        ),
                        row.ascender_in_lpxs,
                    );
                }
            }

            self.draw_text(cx, origin_in_lpxs, &text);

            let turtle = cx.turtle_mut();
            turtle.move_to(dvec2(origin_in_lpxs.x as f64, origin_in_lpxs.y as f64));
            turtle.allocate_width(used_size_in_lpxs.width as f64);
            turtle.allocate_height(used_size_in_lpxs.height as f64);
            turtle.move_to(new_turtle_pos);
            turtle.set_wrap_spacing(
                (last_row.ascender_in_lpxs * last_row.line_spacing_scale
                    - last_row.ascender_in_lpxs) as f64,
            );

            if !callback_before_text {
                for (row_index, row) in text.rows.iter().enumerate() {
                    let (sx, ex) = row_span_x_bounds_in_lpxs(
                        row,
                        row_index == 0,
                        row_index + 1 == text.rows.len(),
                    );
                    f(
                        cx,
                        rect(
                            (origin_in_lpxs.x + (row.origin_in_lpxs.x + sx) * self.font_scale)
                                as f64,
                            (origin_in_lpxs.y
                                + (row.origin_in_lpxs.y - row.ascender_in_lpxs) * self.font_scale)
                                as f64
                                + shift_y as f64,
                            ((ex - sx) * self.font_scale) as f64,
                            ((row.ascender_in_lpxs - row.descender_in_lpxs) * self.font_scale)
                                as f64,
                        ),
                        row.ascender_in_lpxs,
                    );
                }
            }

            cx.emit_turtle_walk(
                Rect {
                    pos: new_turtle_pos,
                    size: dvec2(
                        used_size_in_lpxs.width as f64,
                        used_size_in_lpxs.height as f64,
                    ),
                },
                align_list_start,
            );
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
        self.text_style.font_family.ensure_fonts_loaded(cx);
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
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        {
            self.update_draw_vars(cx);
            self.glyph_depth = self.draw_depth;
            let use_slug_this_frame = self.slug_run_is_ready(cx, text);
            let slug_area_was_empty = self
                .slug_draw
                .as_ref()
                .map(|slug_draw| slug_draw.draw_vars.area.is_empty())
                .unwrap_or(true);
            let shadow_slug_this_frame = use_slug_this_frame && slug_area_was_empty;

            // If the caller opened an outer raster batch via
            // DrawText::begin_many_instances, adopt it here. A nested
            // cx.begin_many_aligned_instances on the same draw_item would
            // otherwise panic, because the outer open already swapped the
            // draw_item's `instances` Vec into its ManyInstances.
            let mut raster_instances = self.many_instances.take();
            let mut raster_instances_is_outer = raster_instances.is_some();
            let mut drew_raster_this_frame = false;
            let mut drew_slug_this_frame = false;

            for row in &text.rows {
                let row_origin = origin_in_lpxs + Size::from(row.origin_in_lpxs) * self.font_scale;
                for glyph in &row.glyphs {
                    use crate::text::geom::Point;

                    let glyph_origin = Point::new(
                        row_origin.x
                            + glyph.origin_in_lpxs.x * self.font_scale
                            + glyph.offset_in_lpxs() * self.font_scale,
                        row_origin.y + glyph.origin_in_lpxs.y * self.font_scale,
                    );
                    if !use_slug_this_frame {
                        if raster_instances.is_none() {
                            raster_instances = cx.begin_many_aligned_instances(&self.draw_vars);
                            raster_instances_is_outer = false;
                        }
                        if let Some(instances) = raster_instances.as_mut() {
                            drew_raster_this_frame |= self.draw_slug_raster_fallback_glyph(
                                cx,
                                glyph_origin,
                                glyph,
                                &mut instances.instances,
                            );
                        }
                        continue;
                    }
                    match self.resolve_glyph(cx, glyph) {
                        Some(ResolvedGlyph::Slug(slug_glyph)) => {
                            let mut drew_slug = false;
                            let mut needs_raster_fallback = shadow_slug_this_frame;
                            if self.ensure_slug_draw(cx) {
                                if let Some(instances) = raster_instances.take() {
                                    self.finish_many_instances(cx, instances);
                                    raster_instances_is_outer = false;
                                }
                                self.sync_slug_draw_state(cx);
                                if let Some(mut slug_draw) = self.slug_draw.take() {
                                    if slug_draw.begin_many_instances(
                                        cx,
                                        if shadow_slug_this_frame { 0.0 } else { 1.0 },
                                    ) {
                                        slug_draw.draw_slug_glyph(
                                            cx,
                                            self,
                                            glyph_origin,
                                            glyph.font_size_in_lpxs,
                                            glyph.color,
                                            slug_glyph,
                                        );
                                        self.glyph_depth += 0.000001;
                                        self.char_index += 1.0;
                                        drew_slug = true;
                                        drew_slug_this_frame = true;
                                    }
                                    self.slug_draw = Some(slug_draw);
                                }
                            }
                            if !drew_slug {
                                needs_raster_fallback = true;
                                cx.redraw_all();
                            }
                            if needs_raster_fallback {
                                if raster_instances.is_none() {
                                    raster_instances =
                                        cx.begin_many_aligned_instances(&self.draw_vars);
                                    raster_instances_is_outer = false;
                                }
                                if let Some(instances) = raster_instances.as_mut() {
                                    drew_raster_this_frame |= self.draw_slug_raster_fallback_glyph(
                                        cx,
                                        glyph_origin,
                                        glyph,
                                        &mut instances.instances,
                                    );
                                }
                            }
                        }
                        Some(ResolvedGlyph::Raster(rasterized_glyph)) => {
                            if let Some(mut slug_draw) = self.slug_draw.take() {
                                if slug_draw.has_open_batch() {
                                    slug_draw.end_many_instances(cx, self.extend_area);
                                }
                                self.slug_draw = Some(slug_draw);
                            }

                            if raster_instances.is_none() {
                                raster_instances = cx.begin_many_aligned_instances(&self.draw_vars);
                                raster_instances_is_outer = false;
                            }
                            let Some(instances) = raster_instances.as_mut() else {
                                continue;
                            };
                            self.draw_rasterized_glyph(
                                glyph_origin,
                                glyph.font_size_in_lpxs,
                                glyph.color,
                                rasterized_glyph,
                                &mut instances.instances,
                            );
                            drew_raster_this_frame = true;
                        }
                        None => {}
                    }
                }

                self.draw_debug_row_guides(cx, row_origin, row);
            }

            if shadow_slug_this_frame && drew_slug_this_frame {
                cx.redraw_all();
            }

            self.flush_slug_textures_if_allowed(cx);
            if let Some(instances) = raster_instances.take() {
                if raster_instances_is_outer {
                    // Hand the outer batch back so the caller's eventual
                    // end_many_instances can finalize it.
                    self.many_instances = Some(instances);
                } else {
                    self.finish_many_instances(cx, instances);
                }
            }
            if let Some(mut slug_draw) = self.slug_draw.take() {
                if slug_draw.has_open_batch() {
                    slug_draw.end_many_instances(cx, self.extend_area);
                }
                if !drew_slug_this_frame {
                    let old_area = slug_draw.draw_vars.area;
                    if !old_area.is_empty() {
                        cx.cx.redraw_area_in_draw(old_area);
                    }
                    slug_draw.draw_vars.area = cx.update_area_refs(old_area, Area::Empty);
                }
                self.slug_draw = Some(slug_draw);
            }
            // Don't clobber the outer batch's area if we've handed it back.
            if !drew_raster_this_frame && self.many_instances.is_none() {
                let old_area = self.draw_vars.area;
                if !old_area.is_empty() {
                    cx.cx.redraw_area_in_draw(old_area);
                }
                self.draw_vars.area = cx.update_area_refs(old_area, Area::Empty);
            }
            return;
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
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
            self.flush_slug_textures_if_allowed(cx);
            self.finish_many_instances(cx, instances);
        }
    }

    fn flush_slug_textures_if_allowed(&mut self, cx: &mut Cx2d) {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        let _ = cx;
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        return;
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        if self.slug_flush_defer_depth != 0 {
            return;
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        self.flush_slug_textures_if_needed(cx);
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    fn flush_slug_textures_if_needed(&mut self, cx: &mut Cx2d) {
        if self.pending_slug_flush_generation == 0 {
            return;
        }
        let fonts = cx.fonts.clone();
        let mut fonts = fonts.borrow_mut();
        if fonts.slug_uploaded_generation() < self.pending_slug_flush_generation {
            fonts.flush_slug_textures(cx.cx);
        }
        if fonts.slug_uploaded_generation() >= self.pending_slug_flush_generation {
            self.pending_slug_flush_generation = 0;
        }
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
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
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
    }

    fn draw_debug_row_guides(
        &mut self,
        cx: &mut Cx2d,
        origin_in_lpxs: Point<f32>,
        row: &LaidoutRow,
    ) {
        if !self.debug {
            return;
        }

        let width_in_lpxs = row.width_in_lpxs * self.font_scale;
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

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
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
        self.draw_debug_row_guides(cx, origin_in_lpxs, row);
    }

    fn resolve_glyph(&mut self, cx: &mut Cx2d, glyph: &LaidoutGlyph) -> Option<ResolvedGlyph> {
        let font_size_in_dpxs = glyph.font_size_in_lpxs * cx.current_dpi_factor() as f32;
        let glyph_prefers_raster_image = glyph
            .font
            .has_glyph_raster_image(glyph.id, font_size_in_dpxs);
        if !glyph_prefers_raster_image && cx.fonts.borrow().should_use_slug_glyph(font_size_in_dpxs)
        {
            let slug_lookup = {
                let mut fonts = cx.fonts.borrow_mut();
                fonts.get_or_cache_slug_glyph(cx.cx.redraw_id, glyph.font.as_ref(), glyph.id)
            };

            match slug_lookup {
                SlugGlyphCacheResult::Ready(slug_glyph) => {
                    return Some(ResolvedGlyph::Slug(slug_glyph));
                }
                #[cfg(any(target_os = "linux", target_os = "windows"))]
                SlugGlyphCacheResult::NeedsUpload {
                    generation,
                    glyph: _,
                } => {
                    self.pending_slug_flush_generation =
                        self.pending_slug_flush_generation.max(generation);
                    cx.redraw_all();
                }
                #[cfg(not(any(target_os = "linux", target_os = "windows")))]
                SlugGlyphCacheResult::NeedsUpload {
                    generation,
                    glyph: slug_glyph,
                } => {
                    self.pending_slug_flush_generation =
                        self.pending_slug_flush_generation.max(generation);
                    return Some(ResolvedGlyph::Slug(slug_glyph));
                }
                SlugGlyphCacheResult::Deferred => {
                    cx.redraw_all();
                }
                SlugGlyphCacheResult::Unavailable => {}
            }
        }

        glyph
            .rasterize(font_size_in_dpxs)
            .map(ResolvedGlyph::Raster)
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
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

        match self.resolve_glyph(cx, glyph) {
            Some(ResolvedGlyph::Slug(slug_glyph)) => {
                self.draw_slug_glyph(
                    cx,
                    glyph_origin,
                    glyph.font_size_in_lpxs,
                    glyph.color,
                    slug_glyph,
                    output,
                );
            }
            Some(ResolvedGlyph::Raster(rasterized_glyph)) => self.draw_rasterized_glyph(
                glyph_origin,
                glyph.font_size_in_lpxs,
                glyph.color,
                rasterized_glyph,
                output,
            ),
            None => {}
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
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

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn draw_slug_raster_fallback_glyph(
        &mut self,
        cx: &mut Cx2d,
        origin_in_lpxs: Point<f32>,
        glyph: &LaidoutGlyph,
        output: &mut Vec<f32>,
    ) -> bool {
        let font_size_in_dpxs = glyph.font_size_in_lpxs * cx.current_dpi_factor() as f32;
        let should_use_slug = cx.fonts.borrow().should_use_slug_glyph(font_size_in_dpxs);
        let fallback_dpxs_per_em = if should_use_slug && font_size_in_dpxs > 0.0 {
            cx.fonts.borrow().max_rasterized_glyph_dpxs_per_em()
        } else {
            0.0
        };
        let rasterized_glyph = if should_use_slug {
            let stable_dpxs_per_em = if fallback_dpxs_per_em > 0.0 {
                fallback_dpxs_per_em.min(font_size_in_dpxs)
            } else {
                font_size_in_dpxs
            };
            glyph
                .font
                .rasterize_glyph_stable_fallback(glyph.id, stable_dpxs_per_em)
        } else {
            glyph.rasterize(font_size_in_dpxs)
        };
        let Some(rasterized_glyph) = rasterized_glyph else {
            return false;
        };
        self.draw_rasterized_glyph(
            origin_in_lpxs,
            glyph.font_size_in_lpxs,
            glyph.color,
            rasterized_glyph,
            output,
        );
        true
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
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if let Some(slug_draw) = self.slug_draw.as_mut() {
            slug_draw
                .draw_vars
                .set_instance_on_area(cx, live_id!(total_chars), &[total]);
        }
    }

    pub fn redraw_areas(&self, cx: &mut Cx) {
        self.draw_vars.area.redraw(cx);
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if let Some(slug_draw) = self.slug_draw.as_ref() {
            slug_draw.draw_vars.area.redraw(cx);
        }
    }

    pub fn redraw(&self, cx: &mut Cx) {
        self.redraw_areas(cx);
    }

    pub fn new_draw_call(&mut self, cx: &mut Cx2d) {
        self.update_draw_vars(cx);
        cx.new_draw_call(&self.draw_vars);
    }

    pub fn append_to_draw_call(&self, cx: &mut Cx2d) {
        cx.append_to_draw_call(&self.draw_vars);
    }

    pub fn get_aa_pad_px(&self, cx: &mut Cx) -> f32 {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if let Some(slug_draw) = self.slug_draw.as_ref() {
            let mut value = [0.0];
            slug_draw
                .draw_vars
                .get_uniform(cx, live_id!(aa_pad_px), &mut value);
            return value[0];
        }

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

    fn update_font_definitions(&self, cx: &mut Cx, fonts: &mut Fonts) {
        let mut font_ids = Vec::new();

        for member in &self.members {
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

    fn ensure_fonts_loaded(&self, cx: &mut Cx) {
        CxDraw::lazy_construct_fonts(cx);

        let family_id = self.to_font_family_id();
        let fonts = cx.get_global::<Rc<RefCell<Fonts>>>().clone();

        {
            let fonts_ref = fonts.borrow();
            if fonts_ref.is_font_family_complete(family_id) {
                return;
            }
        }

        for member in &self.members {
            cx.load_script_resource(member.handle);
        }
        {
            let fonts_ref = fonts.borrow();
            if fonts_ref.is_font_family_complete(family_id) {
                return;
            }
        }

        let mut fonts_ref = fonts.borrow_mut();
        self.update_font_definitions(cx, &mut fonts_ref);
    }
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
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    use super::{register_draw_text_slug, DrawTextSlug};
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    use crate::makepad_platform::{live_id, LiveId, ScriptNew, ScriptVmCx};
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    use crate::makepad_platform::{vec4, Cx};

    #[test]
    fn draw_text_size_stays_16_byte_aligned() {
        assert_eq!(std::mem::size_of::<DrawText>() % 16, 0);
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn read_instance(draw_vars: &super::DrawVars, cx: &mut Cx, id: LiveId) -> [f32; 4] {
        let mut value = [0.0; 4];
        draw_vars.get_instance(cx, id, &mut value);
        value
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[test]
    fn draw_text_color_is_visible_through_instance_slice() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let mut draw_text = DrawText::script_new_with_default(vm);
            draw_text.color = vec4(0.11, 0.22, 0.33, 0.44);

            let packed =
                vm.with_cx_mut(|cx| read_instance(&draw_text.draw_vars, cx, live_id!(color)));

            assert_eq!(packed, [0.11, 0.22, 0.33, 0.44]);
        });
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[test]
    fn slug_helper_color_is_visible_through_instance_slice() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            register_draw_text_slug(vm);

            let mut slug_draw = DrawTextSlug::script_new_with_default(vm);
            slug_draw.color = vec4(0.15, 0.25, 0.35, 0.45);

            let packed =
                vm.with_cx_mut(|cx| read_instance(&slug_draw.draw_vars, cx, live_id!(color)));

            assert_eq!(packed, [0.15, 0.25, 0.35, 0.45]);
        });
    }
}
