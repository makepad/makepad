//! DrawSceneSplat: a generated terrain tile's paint, blended per pixel in
//! one opaque draw.

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    // A terrain tile (gen level terrain_mesh: one part of triplanar
    // triangles, one of flat ones) draws here: DrawScenePbr's vertex stage,
    // binding and lighting (`shade_mr`), with a pixel that paints the
    // tile's layers itself. Before, every triangle drew again, alpha-cut,
    // in the overlay part of each other layer weighing at a corner (about
    // seven deep): each overlay fragment ran the triplanar read and the
    // cover rule and then mostly discarded, and a pipeline that can discard
    // costs early-Z and a tile GPU its hidden-surface removal.
    //
    // The rule is the overlays' rule, so the picture is theirs: a layer
    // covers where w + (height - 0.5) * 0.45 + (noise - 0.5) * 0.5 clears
    // 0.36 (w its weight, height its texture's alpha, noise world value
    // noise at ~2 m and ~0.5 m) and w >= 0.02; the last covering layer in
    // paint order wins, else the heaviest. Walking the layers from the last
    // down, the first that covers is the one; the dominant one needs no
    // test. Layer reads sit in branches that differ between pixels, so they
    // take explicit gradients (sample_grad_repeat). What the pixel shows (its
    // colour, normal and far anti-tiling read) is filtered anisotropically:
    // a ground seen at a grazing angle keeps its detail along the view.
    //
    // Bindings (the PBR lane's seventeen texture slots, retyped where the
    // lane does not read them): `tex` is the layers' albedo array (height
    // in alpha), `normal_map` their normals with relative roughness in alpha,
    // `occlusion_map` the tile's weight map (no occlusion: occlusion_strength stays 0)
    // or a worn road part's band map (`splat_band_texel`; renderer
    // draw_items.rs SPLAT_MAP_SLOT), and the ORM slot the grass field's
    // cover raster (`grass_cover_at`). Per layer, splat_l<i> = (UV scale
    // 1/m, roughness, metallic, normal strength).
    //
    // A worn road part (gen level road.rs: a carriageway and the paint on
    // it) carries its band coordinates (metres right of the centre line,
    // metres of arc) in the vertex's AO lane, which DrawScenePbr's vertex
    // decodes into v_ao_uv (model.rs MODEL_VERTEX_FLOATS: only such a part
    // carries them there; `splat_band_coords`), and the masks of its wear
    // follow them: the line cars wear into it, sealant seams, repair
    // patches and dirty edges (`splat_band_wear`). A function of the
    // coordinates alone: any camera sees the same road.
    mod.draw.DrawSceneSplat = mod.std.set_type_default() do #(DrawSceneSplat::script_shader(vm)){
        ..mod.draw.DrawScenePbr,
        tex: texture_2d_array(float)
        normal_map: texture_2d_array(float)
        // Weight map: world x, z of texel 0; texels per metre.
        splat_map: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        // Weight map: texels per row, rows per page, pages; layers.
        splat_size: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        // 1 blends the layers, 0 paints the heaviest only (a far tile,
        // TierBudget::terrain_blend_reach).
        splat_blend: uniform(1.0)
        splat_l0: uniform(vec4(1.0, 0.9, 0.0, 0.0))
        splat_l1: uniform(vec4(1.0, 0.9, 0.0, 0.0))
        splat_l2: uniform(vec4(1.0, 0.9, 0.0, 0.0))
        splat_l3: uniform(vec4(1.0, 0.9, 0.0, 0.0))
        splat_l4: uniform(vec4(1.0, 0.9, 0.0, 0.0))
        splat_l5: uniform(vec4(1.0, 0.9, 0.0, 0.0))
        splat_l6: uniform(vec4(1.0, 0.9, 0.0, 0.0))
        splat_l7: uniform(vec4(1.0, 0.9, 0.0, 0.0))

        // x: relief metres, y: wet, z: puddle share, w: height quantile.
        splat_relief_on: uniform(0.0)
        splat_s0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_s1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_s2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_s3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_s4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_s5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_s6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_s7: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        // World macro variation per layer: x brightness, y warm/cool hue,
        // z roughness amplitudes, w the middle octave's size in metres (0:
        // none; a terrain layer then keeps the brightness drift).
        splat_m0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_m1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_m2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_m3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_m4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_m5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_m6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_m7: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        // Hex tiling per layer (`tiling: "stochastic"`), 1 or 0: layers 0-3
        // in a, 4-7 in b (splat_hex_texel).
        splat_hex_a: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        splat_hex_b: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        // A worn road part's band (material_surface.rs SplatBand): x the arc
        // its band coordinates count from, y their arc span (0: no band, no
        // masks), z their lateral span, w the carriageway's half width.
        splat_band: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // Its band map: metres of arc per texel, texels per row, rows, a
        // closed road's arc (0: open; the masks repeat over a lap).
        splat_band_map: uniform(vec4(1.0, 2.0, 1.0, 0.0))
        // Wear amounts: the worn line, sealant seams, repairs, dirty edges.
        splat_wear: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        // The grass's far carpet (renderer/grass_draw.rs set_grass_carpet):
        // the field's origin x, z and 1 / its width and depth; the sRGB lush
        // and dry blade (lush.w: the blades' wind); the screen's density
        // (grass.rs GrassLod: full, reach; w 1 while a field grows).
        grass_map: uniform(vec4(0.0, 0.0, 1.0, 1.0))
        grass_lush: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        grass_dry: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        grass_lod: uniform(vec4(1.0, 1.0, 0.0, 0.0))
        // The trampled field (crate::trample) as the blades read it: its
        // corner x, z and 1 / side (0: nothing pressed, no read). It rides
        // the PBR lane's emissive slot (renderer/grass_draw.rs
        // TRAMPLE_SLOT): this lane reads no emissive map.
        grass_trample: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wind_state: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        wind_scroll: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        // The grass field's cover raster at u (0..1 over the field):
        // density in x, dryness in y. It is bound on the PBR lane's ORM
        // slot (renderer/grass_draw.rs GRASS_COVER_SLOT): this lane reads no
        // ORM map and the model shaders have no free texture slot, so the
        // slot is read here and nowhere else.
        grass_cover_at: fn(u: vec2) -> vec4 {
            return self.orm_map.sample_lod(u, 0.0)
        }

        // Where the grass's blades end, the meadow carries on as a carpet
        // on the ground: its cover texel at wp (density x, dryness y), in z
        // how flat it was pressed (the trampled field, faded out over its
        // last 8 % as the blades fade it), and in w how much of the pixel
        // it takes: none inside 40% of the blades' reach, all of the
        // density from 80%, where the last blades start to sink
        // (shaders/grass.rs), so the ground between them is already the
        // meadow's colour when they go, and a trail runs on past them.
        grass_carpet_at: fn(wp: vec3) -> vec4 {
            let u = (vec2(wp.x, wp.z) - self.grass_map.xy) * self.grass_map.zw
            if self.grass_lod.w < 0.5 || u.x <= 0.0 || u.y <= 0.0 || u.x >= 1.0 || u.y >= 1.0 { return vec4(0.0, 0.0, 0.0, 0.0) }
            let cover = self.grass_cover_at(u)
            let d = length(self.eye.xyz - wp)
            let w = cover.x * smoothstep(self.grass_lod.y * 0.4, self.grass_lod.y * 0.8, d)
            var lay = 0.0
            let tr = self.grass_trample
            if tr.z > 0.0 && w > 0.0 {
                let tuv = (vec2(wp.x, wp.z) - tr.xy) * tr.z
                let edge = min(min(tuv.x, 1.0 - tuv.x), min(tuv.y, 1.0 - tuv.y))
                if edge > 0.0 {
                    lay = self.emissive_map.sample_lod(tuv, 0.0).z * smoothstep(0.0, 0.08, edge)
                }
            }
            return vec4(cover.x, cover.y, lay, w)
        }

        // The carpet: the blades' colour and light (shaders/grass.rs) as a
        // far meadow shows them, on the ground. The meadow's drier and
        // greener patches and their hue, the clumps' tufts (fading out
        // where a pixel spans a clump: `span`, metres), the paint's own
        // fine detail (`detail`: its texel's brightness over its
        // neighbourhood's, about 1 on average), lit as blades that face the
        // eye half-upright, and the light of the gusts passing. Under the
        // blades it is the meadow's depth, the lower blades (t 0.3 up a
        // blade: darker, more occluded), so the blades in front keep their
        // contrast; where they thin and sink it turns into what a low view
        // sees of them from afar, the upper blade (t 0.65). Where it was
        // pressed (cover.z) it is paler and turned up to the sky, as the
        // laid blades are.
        grass_carpet: fn(wp: vec3, cover: vec4, span: float, detail: float) -> vec4 {
            let xz = vec2(wp.x, wp.z)
            let patch = self.wind_noise(xz * 0.083 + vec2(31.0, 17.0))
            let tufts = (self.wind_noise(xz * 1.43 + vec2(7.3, 2.1)) - 0.5) * (1.0 - smoothstep(0.17, 0.35, span))
            let t = mix(0.3, 0.65, smoothstep(self.grass_lod.y * 0.55, self.grass_lod.y * 0.95, length(self.eye.xyz - wp)))
            let dry = clamp(cover.y + (patch - 0.5) * 0.7 + tufts * 0.3 + t * 0.125 + cover.z * 0.25, 0.0, 1.0)
            let hue = (patch - 0.5) * 0.08 + tufts * 0.1
            let gust = self.scene_wind(xz, self.wind_state, self.wind_scroll)
            let lift = 1.0 + (gust.z - 0.5) * 0.12 * self.grass_lush.w
            let shade = mix(mix(0.55, 0.3, cover.x * 0.5), 1.12, t) * (1.0 + tufts * 0.32) * (1.0 + 0.12 * cover.z)
            let col = mix(self.grass_lush.xyz, self.grass_dry.xyz, dry) * vec3(1.0 + hue, 1.0, 1.0 - hue * 1.5)
            let albedo = self.to_scene(col * (shade * lift * detail))
            let v = normalize(self.eye.xyz - wp)
            let n = normalize(vec3(v.x, 0.0, v.z) * 0.64 + vec3(0.0, 0.9 + 1.5 * cover.z, 0.0))
            let l = normalize(self.light_dir)
            let ndl = max(dot(n, l), 0.0)
            let vis = self.csm_vis_fast(wp, n, ndl)
            let occ = mix(0.35, 1.0, t)
            let ambient = self.gi_ambient(wp, n, mix(self.sun_ground, self.sun_sky, clamp(n.y * 0.5 + 0.5, 0.0, 1.0))) * occ
            let back = max(dot(v * (0.0 - 1.0), l), 0.0)
            let through = back * back * back * 0.45 * t
            let sheen = pow(max(dot(n, normalize(l + v)), 0.0), 24.0) * 0.16 * t
            let lit = albedo * (ambient + self.sun_color * ((ndl * 0.85 + 0.15) * vis + through * vis)) + self.sun_color * (sheen * vis)
            return self.csm_debug_view(self.gi_display(vec4(self.scene_fogged(self.to_display(lit), self.v_fog, wp, self.fog_density), 1.0), wp, n), wp, n)
        }

        // The band coordinates: metres right of the centre line, metres of
        // arc (the AO lane as DrawScenePbr's vertex decodes it, unorm16 x 2
        // over the part's spans).
        splat_band_coords: fn() -> vec2 {
            return vec2((self.v_ao_uv.x - 0.5) * self.splat_band.z, self.splat_band.x + self.v_ao_uv.y * self.splat_band.y)
        }

        // The band map at arc a: r the racing line (0.5 + 0.5 * metres right
        // / half width), g the rubber laid there, b the braking. Rows start
        // on the previous row's last texel: the filter never spans two rows.
        splat_band_texel: fn(a: float) -> vec4 {
            let m = self.splat_band_map
            let t = clamp(a / m.x, 0.0, (m.y - 1.0) * m.z)
            let r = min(floor(t / (m.y - 1.0)), m.z - 1.0)
            let x = t - r * (m.y - 1.0)
            return self.occlusion_map.sample_lod(vec2((x + 0.5) / m.y, (r + 0.5) / m.z), 0.0)
        }

        // A sine-free hash of a lattice point (exact for any arc a road
        // reaches; a sine of a large argument differs between GPUs).
        band_hash: fn(p: vec2) -> float {
            var p3 = fract(vec3(p.x, p.y, p.x) * vec3(0.1031, 0.103, 0.0973))
            let d = dot(p3, vec3(p3.y, p3.z, p3.x) + vec3(33.33, 33.33, 33.33))
            p3 = p3 + vec3(d, d, d)
            return fract((p3.x + p3.y) * p3.z)
        }

        // Value noise at x (lattice units, row k), 0..1; the lattice wraps
        // every n points (0: never).
        band_vnoise: fn(x: float, k: float, n: float) -> float {
            let i = floor(x)
            let u = x - i
            var i0 = i
            var i1 = i + 1.0
            if n > 0.0 {
                i0 = i0 - floor(i0 / n) * n
                i1 = i1 - floor(i1 / n) * n
            }
            return mix(self.band_hash(vec2(i0, k)), self.band_hash(vec2(i1, k)), u * u * (3.0 - 2.0 * u))
        }

        // Noise along the arc at about f cycles a metre (row k); on a closed
        // road a whole number of cycles a lap, so the line shows no seam.
        band_noise: fn(a: float, f: float, k: float) -> float {
            let len = self.splat_band_map.w
            if len > 0.0 {
                let n = max(floor(len * f + 0.5), 1.0)
                return self.band_vnoise(a * n / len, k, n)
            }
            return self.band_vnoise(a * f, k, 0.0)
        }

        // About c0 metres of arc, a whole number of them a lap.
        band_cell: fn(c0: float) -> float {
            let len = self.splat_band_map.w
            if len > 0.0 { return len / max(floor(len / c0 + 0.5), 1.0) }
            return c0
        }

        // Cell index of arc a in cells of c metres (wrapped on a lap).
        band_index: fn(a: float, c: float) -> float {
            let k = floor(a / c)
            let len = self.splat_band_map.w
            if len > 0.0 {
                let n = max(floor(len / c + 0.5), 1.0)
                return k - floor(k / n) * n
            }
            return k
        }

        // How much of a pixel `f` metres across, at distance d from the
        // middle of a line w metres either side of it, the line covers (a
        // box filter: a line thinner than the pixel fades to its share).
        band_line: fn(d: float, w: float, f: float) -> float {
            let e = max(f, 0.0001)
            let lo = max(0.0 - w, d - e * 0.5)
            let hi = min(w, d + e * 0.5)
            return clamp((hi - lo) / e, 0.0, 1.0)
        }

        // The wear at band coordinates bc (metres right, metres of arc),
        // the pixel's footprint along each bf, on a layer `wet` (0..1):
        // x the albedo factor, y the roughness shift, z standing water (the
        // edges hold it when the road is wet), w the dirt share (a dusty
        // tint). Masks:
        // - line: rubber where the cars run. A circuit's racing line (the
        //   band map's offset, rubber and braking): a broad dark band with
        //   the two tyre tracks in it. A road's wheel paths: two per lane.
        //   Darker and smoother.
        // - edges: dirt within 0.8 m of the carriageway's edge and over the
        //   edge line, broken along the arc.
        // - seams: sealant over the joints between paving passes (centre
        //   line, lane widths), wandering, in stretches; and cracks across
        //   the road, sealed, one in some 11 m cells. Dark and glossy.
        // - repairs: patches in some 17 m cells, a paving pass wide or a
        //   wheel track wide, newer (darker, smoother, less rubber) or older
        //   (lighter), with a sealed rim.
        splat_band_wear: fn(bc: vec2, bf: vec2, wet: float) -> vec4 {
            let lat = bc.x
            let a = bc.y
            let hw = self.splat_band.w
            let wear = self.splat_wear
            let f = max(bf.x, bf.y)
            var dark = 0.0
            var rough = 0.0
            var seal = 0.0
            var patch = 0.0
            let pw = hw / max(floor(hw / 3.8 + 0.5), 1.0)
            if wear.z > 0.0 && abs(lat) < hw {
                let cr = self.band_cell(17.0)
                let k = self.band_index(a, cr)
                let j = floor(lat / pw)
                let id = k * 7.0 + j
                if self.band_hash(vec2(id, 43.0)) < 0.3 * wear.z {
                    let len = 1.5 + 5.0 * self.band_hash(vec2(id, 47.0))
                    let a0 = (floor(a / cr) + 0.08) * cr + self.band_hash(vec2(id, 53.0)) * max(cr * 0.84 - len, 0.0)
                    var l0 = j * pw + 0.12
                    var l1 = (j + 1.0) * pw - 0.12
                    if self.band_hash(vec2(id, 59.0)) < 0.5 {
                        let w = 0.9 + 0.6 * self.band_hash(vec2(id, 67.0))
                        let c = mix(l0 + w * 0.5, l1 - w * 0.5, self.band_hash(vec2(id, 71.0)))
                        l0 = c - w * 0.5
                        l1 = c + w * 0.5
                    }
                    let sd = max(max(l0 - lat, lat - l1), max(a0 - a, a - a0 - len))
                    patch = clamp(0.5 - sd / max(f, 0.0001), 0.0, 1.0)
                    seal = max(seal, self.band_line(sd, 0.018, f))
                    let newer = step(self.band_hash(vec2(id, 61.0)), 0.6)
                    dark = dark + patch * mix(-0.14, 0.3, newer)
                    rough = rough + patch * mix(0.06, -0.05, newer)
                    patch = patch * newer
                }
            }
            if wear.x > 0.0 {
                var rub = 0.0
                if self.splat_band_map.w > 0.0 {
                    let t = self.splat_band_texel(a)
                    let d = lat - (t.x - 0.5) * 2.0 * hw
                    let ad = abs(d) - 0.78
                    rub = (t.y + t.z * 0.3) * (exp(0.0 - d * d / 1.1) * 0.55 + exp(0.0 - ad * ad / 0.09) * 0.45)
                } else {
                    let lane = abs(abs(lat) - hw * 0.5) - 0.85
                    rub = exp(0.0 - lane * lane / 0.12) * 0.75
                }
                rub = rub * (0.7 + 0.3 * self.band_noise(a, 0.05, 3.0)) * wear.x * (1.0 - patch * 0.7)
                dark = dark + rub * 0.6
                rough = rough - rub * 0.25
            }
            if wear.y > 0.0 && abs(lat) < hw {
                let j = floor(lat / pw + 0.5)
                if abs(j * pw) < hw - 0.25 {
                    let wander = (self.band_noise(a, 0.08, 11.0 + j) - 0.5) * 0.14 + (self.band_noise(a, 0.9, 13.0 + j) - 0.5) * 0.025
                    let on = smoothstep(0.32, 0.45, self.band_noise(a, 0.02, 17.0 + j)) * wear.y
                    let w = 0.012 + 0.01 * self.band_noise(a, 0.4, 19.0 + j)
                    seal = max(seal, self.band_line(lat - j * pw - wander, w, bf.x) * on)
                }
                let ct = self.band_cell(11.0)
                let k = self.band_index(a, ct)
                if self.band_hash(vec2(k, 23.0)) < 0.5 * wear.y {
                    let ac = (floor(a / ct) + 0.15 + 0.7 * self.band_hash(vec2(k, 29.0))) * ct
                    let la = (self.band_hash(vec2(k, 31.0)) * 2.0 - 1.0) * hw
                    let lb = (self.band_hash(vec2(k, 37.0)) * 2.0 - 1.0) * hw
                    let span = smoothstep(min(la, lb) - 0.1, min(la, lb) + 0.3, lat) * (1.0 - smoothstep(max(la, lb) - 0.3, max(la, lb) + 0.1, lat))
                    let wander = (self.band_vnoise(lat * 0.8, k * 7.0 + 41.0, 0.0) - 0.5) * 0.3
                    seal = max(seal, self.band_line(a - ac - wander, 0.01, bf.y) * span)
                }
            }
            dark = dark + seal * 0.5
            rough = rough - seal * 0.45
            var dirt = 0.0
            if wear.w > 0.0 {
                let e = hw - abs(lat)
                let edge = (1.0 - smoothstep(0.0, 0.8, e)) * smoothstep(-0.6, -0.15, e)
                dirt = edge * (0.55 + 0.45 * self.band_noise(a, 0.3, 7.0 + sign(lat))) * wear.w
                rough = rough + dirt * 0.1 * (1.0 - wet)
            }
            return vec4(clamp(1.0 - dark, 0.3, 1.3), rough, dirt * wet, dirt)
        }

        splat_macro_param: fn(l: float) -> vec4 {
            if l < 0.5 { return self.splat_m0 }
            if l < 1.5 { return self.splat_m1 }
            if l < 2.5 { return self.splat_m2 }
            if l < 3.5 { return self.splat_m3 }
            if l < 4.5 { return self.splat_m4 }
            if l < 5.5 { return self.splat_m5 }
            if l < 6.5 { return self.splat_m6 }
            return self.splat_m7
        }

        // World macro variation at wp (xz) for a layer's `mv`: x the
        // brightness field, y the roughness field (both about -1..1), z the
        // hue field (-0.5..0.5). Three octaves of world value noise at 4.7x,
        // 1x and 0.2x the size (about 70, 15 and 3 m at size 15): no tile
        // period can show through it. Roughness takes its own two finer
        // octaves and shares only the broadest one, inverted (light, worn
        // stretches are polished), so the specular sheen does not just
        // repeat the brightness. An octave fades out where one pixel spans an
        // eighth to a quarter of its cell, so nothing shimmers at grazing
        // angles; `span` is the pixel's footprint in metres.
        splat_macro: fn(mv: vec4, wp: vec3, span: float) -> vec3 {
            let q = vec2(wp.x, wp.z) / mv.w
            let s = span / mv.w
            let k2 = 1.0 - smoothstep(0.12, 0.25, s)
            let k3 = 1.0 - smoothstep(0.024, 0.05, s)
            let a1 = self.tn_noise(q * 0.21 + vec2(3.7, 1.3)) - 0.5
            let a2 = (self.tn_noise(q + vec2(7.1, 2.9)) - 0.5) * k2
            let a3 = (self.tn_noise(q * 5.0 + vec2(1.9, 5.3)) - 0.5) * k3
            let r2 = (self.tn_noise(q * 1.3 + vec2(13.7, 8.3)) - 0.5) * k2
            let r3 = (self.tn_noise(q * 4.3 + vec2(5.9, 17.1)) - 0.5) * k3
            let hue = self.tn_noise(q * 0.47 + vec2(11.3, 4.1)) - 0.5
            return vec3((a1 * 0.5 + a2 * 0.33 + a3 * 0.22) * 2.0, (a1 * -0.4 + r2 * 0.4 + r3 * 0.3) * 2.0, hue)
        }

        splat_surface_param: fn(l: float) -> vec4 {
            if l < 0.5 { return self.splat_s0 }
            if l < 1.5 { return self.splat_s1 }
            if l < 2.5 { return self.splat_s2 }
            if l < 3.5 { return self.splat_s3 }
            if l < 4.5 { return self.splat_s4 }
            if l < 5.5 { return self.splat_s5 }
            if l < 6.5 { return self.splat_s6 }
            return self.splat_s7
        }

        splat_param: fn(l: float) -> vec4 {
            if l < 0.5 { return self.splat_l0 }
            if l < 1.5 { return self.splat_l1 }
            if l < 2.5 { return self.splat_l2 }
            if l < 3.5 { return self.splat_l3 }
            if l < 4.5 { return self.splat_l4 }
            if l < 5.5 { return self.splat_l5 }
            if l < 6.5 { return self.splat_l6 }
            return self.splat_l7
        }

        // A layer read: anisotropic when `af` is 1 (what the pixel shows),
        // trilinear when 0 (the cover test, the relief trace and its shadow
        // taps: heights that need no sharpness, where sixteen taps a read
        // would be the real cost).
        splat_read: fn(p: vec3, d1: vec2, d2: vec2, af: float) -> vec4 {
            if af > 0.5 { return self.tex.sample_grad_aniso_repeat(p, d1, d2) }
            return self.tex.sample_grad_repeat(p, d1, d2)
        }

        splat_weight: fn(l: float, wa: vec4, wb: vec4) -> float {
            if l < 0.5 { return wa.x }
            if l < 1.5 { return wa.y }
            if l < 2.5 { return wa.z }
            if l < 3.5 { return wa.w }
            if l < 4.5 { return wb.x }
            if l < 5.5 { return wb.y }
            if l < 6.5 { return wb.z }
            return wb.w
        }

        // Layer l's texel at world position wp (height in w): the top
        // projection on a flat part, three projections blended by `bw`
        // otherwise. dx, dy: the screen derivatives of wp; af: splat_read's.
        splat_texel: fn(l: float, wp: vec3, bw: vec3, dx: vec3, dy: vec3, af: float) -> vec4 {
            let s = self.splat_param(l).x
            let ty = self.splat_read(vec3(wp.x * s, wp.z * s, l), vec2(dx.x, dx.z) * s, vec2(dy.x, dy.z) * s, af)
            if self.triplanar < 0.0 { return ty }
            let tx = self.splat_read(vec3(wp.z * s, (0.0 - wp.y) * s, l), vec2(dx.z, 0.0 - dx.y) * s, vec2(dy.z, 0.0 - dy.y) * s, af)
            let tz = self.splat_read(vec3(wp.x * s, (0.0 - wp.y) * s, l), vec2(dx.x, 0.0 - dx.y) * s, vec2(dy.x, 0.0 - dy.y) * s, af)
            return tx * bw.x + ty * bw.y + tz * bw.z
        }

        // Hex tiling (a layer with `tiling: "stochastic"`): the tile is read
        // three times, at the corners of the triangle-grid cell around the
        // pixel (cells about a third of a tile), each copy shifted by its
        // corner's random offset, and blended by the barycentric weights to
        // the 7th power (narrow seams) times each copy's height (the higher
        // grain shows through), so the tile's repeat never lines up. Copies
        // are not turned: a ground's grain often has a direction (grass
        // strokes, a road's wear), and turned copies break it into visible
        // cells. Gradients are the plain read's. Only the winning layer's
        // reads (never splat_try's).
        //
        // Corner k (0..2) of the cell around st (uv x 2 sqrt 3): xy its
        // lattice id, z its barycentric weight.
        splat_hex_corner: fn(st: vec2, k: float) -> vec3 {
            let skew = vec2(st.x - st.y * 0.57735027, st.y * 1.15470054)
            let base = floor(skew)
            let f = skew - base
            let z = 1.0 - f.x - f.y
            if z > 0.0 {
                if k < 0.5 { return vec3(base.x, base.y, z) }
                if k < 1.5 { return vec3(base.x, base.y + 1.0, f.y) }
                return vec3(base.x + 1.0, base.y, f.x)
            }
            if k < 0.5 { return vec3(base.x + 1.0, base.y + 1.0, 0.0 - z) }
            if k < 1.5 { return vec3(base.x + 1.0, base.y, 1.0 - f.y) }
            return vec3(base.x, base.y + 1.0, 1.0 - f.x)
        }

        // Corner c's copy of uv: shifted by a random offset in tile units. A
        // sine-free hash, exact for any lattice id a world reaches.
        splat_hex_uv: fn(c: vec3, uv: vec2) -> vec2 {
            var p3 = fract(vec3(c.x, c.y, c.x) * vec3(0.1031, 0.103, 0.0973))
            let d = dot(p3, vec3(p3.y, p3.z, p3.x) + vec3(33.33, 33.33, 33.33))
            p3 = p3 + vec3(d, d, d)
            return uv + fract(vec2(p3.x + p3.y, p3.x + p3.z) * vec2(p3.z, p3.y))
        }

        // The corners' weights from their barycentric weights b and their
        // copies' heights h (40..100 %), normalised.
        splat_hex_weights: fn(b: vec3, h: vec3) -> vec3 {
            let b2 = b * b
            let w = b2 * b2 * b2 * b * (vec3(0.4, 0.4, 0.4) + h * 0.6)
            return w / max(w.x + w.y + w.z, 0.000001)
        }

        // Layer l's hex-tiled texel (albedo, height in w) at uv, tile units.
        splat_hex_texel: fn(l: float, uv: vec2, du1: vec2, du2: vec2, af: float) -> vec4 {
            let st = uv * 3.4641016
            let c0 = self.splat_hex_corner(st, 0.0)
            let c1 = self.splat_hex_corner(st, 1.0)
            let c2 = self.splat_hex_corner(st, 2.0)
            let p0 = self.splat_hex_uv(c0, uv)
            let p1 = self.splat_hex_uv(c1, uv)
            let p2 = self.splat_hex_uv(c2, uv)
            let t0 = self.splat_read(vec3(p0.x, p0.y, l), du1, du2, af)
            let t1 = self.splat_read(vec3(p1.x, p1.y, l), du1, du2, af)
            let t2 = self.splat_read(vec3(p2.x, p2.y, l), du1, du2, af)
            let w = self.splat_hex_weights(vec3(c0.z, c1.z, c2.z), vec3(t0.w, t1.w, t2.w))
            return t0 * w.x + t1 * w.y + t2 * w.z
        }

        // Its normal-map texel (encoded normal, roughness in w), weighted as
        // the texel (the heights are the shown texel's anisotropic reads
        // again, from a warm cache).
        splat_hex_normal: fn(l: float, uv: vec2, du1: vec2, du2: vec2) -> vec4 {
            let st = uv * 3.4641016
            let c0 = self.splat_hex_corner(st, 0.0)
            let c1 = self.splat_hex_corner(st, 1.0)
            let c2 = self.splat_hex_corner(st, 2.0)
            let u0 = self.splat_hex_uv(c0, uv)
            let p0 = vec3(u0.x, u0.y, l)
            let u1 = self.splat_hex_uv(c1, uv)
            let p1 = vec3(u1.x, u1.y, l)
            let u2 = self.splat_hex_uv(c2, uv)
            let p2 = vec3(u2.x, u2.y, l)
            let w = self.splat_hex_weights(vec3(c0.z, c1.z, c2.z), vec3(
                self.tex.sample_grad_aniso_repeat(p0, du1, du2).w,
                self.tex.sample_grad_aniso_repeat(p1, du1, du2).w,
                self.tex.sample_grad_aniso_repeat(p2, du1, du2).w))
            return self.normal_map.sample_grad_aniso_repeat(p0, du1, du2) * w.x
                + self.normal_map.sample_grad_aniso_repeat(p1, du1, du2) * w.y
                + self.normal_map.sample_grad_aniso_repeat(p2, du1, du2) * w.z
        }

        // How far hex tiling is on for layer l at this pixel: fully where its
        // tile spans 16 pixels or more, off below 8, where one read stands in
        // (in those mips a ground's tile holds little but its mean, so the
        // copies agree; between the two the reads cross-fade, never a step).
        splat_hex_amount: fn(l: float, du1: vec2, du2: vec2) -> float {
            if self.splat_weight(l, self.splat_hex_a, self.splat_hex_b) < 0.5 { return 0.0 }
            return 1.0 - smoothstep(0.0625, 0.125, max(length(du1), length(du2)))
        }

        // Mesh UVs for a single surface; the existing world projections for
        // terrain. Explicit gradients stay those of the undisplaced surface.
        // hex: splat_hex_amount (terrain: on its top projection, where uv is
        // that projection's; the side projections stay single reads). af:
        // splat_read's.
        splat_relief_read: fn(l: float, wp: vec3, uv: vec2, bw: vec3, dx: vec3, dy: vec3, du1: vec2, du2: vec2, hex: float, af: float) -> vec4 {
            var t = vec4(0.0, 0.0, 0.0, 0.0)
            if hex < 1.0 {
                if self.triplanar == 0.0 {
                    t = self.splat_read(vec3(uv.x, uv.y, l), du1, du2, af)
                } else {
                    t = self.splat_texel(l, wp, bw, dx, dy, af)
                }
            }
            if hex > 0.0 {
                var tiled = self.splat_hex_texel(l, uv, du1, du2, af)
                if self.triplanar > 0.0 {
                    let s = self.splat_param(l).x
                    let tx = self.splat_read(vec3(wp.z * s, (0.0 - wp.y) * s, l), vec2(dx.z, 0.0 - dx.y) * s, vec2(dy.z, 0.0 - dy.y) * s, af)
                    let tz = self.splat_read(vec3(wp.x * s, (0.0 - wp.y) * s, l), vec2(dx.x, 0.0 - dx.y) * s, vec2(dy.x, 0.0 - dy.y) * s, af)
                    tiled = tx * bw.x + tiled * bw.y + tz * bw.z
                }
                t = mix(t, tiled, hex)
            }
            return t
        }

        // Linear POM in normalised depth (0 at the top, 1 at the bottom).
        // A fixed loop bound, 8..24 active steps, then one secant refinement
        // between the last two samples. The secant needs no texture read.
        splat_relief_trace: fn(l: float, wp: vec3, uv: vec2, ray: vec3, uv_ray: vec2, bw: vec3, dx: vec3, dy: vec3, du1: vec2, du2: vec2, height: float, nv: float, hex: float) -> float {
            let steps = ceil(mix(24.0, 8.0, clamp((nv - 0.1391731) / 0.8608269, 0.0, 1.0)))
            var depth = 0.0
            var before_depth = 0.0
            var error = 1.0 - height
            var before_error = error
            for i in 0..24 {
                if float(i) < steps && error > 0.0 {
                    before_depth = depth
                    before_error = error
                    depth = (float(i) + 1.0) / steps
                    let h = self.splat_relief_read(l, wp - ray * depth, uv - uv_ray * depth, bw, dx, dy, du1, du2, hex, 0.0).w
                    error = 1.0 - h - depth
                }
            }
            let fraction = clamp(before_error / max(before_error - error, 0.00001), 0.0, 1.0)
            return mix(before_depth, depth, fraction)
        }

        // Four height taps from the hit toward the sun; only direct sunlight
        // is attenuated. The ambient/IBL/GI paths retain their own occlusion.
        splat_relief_shadow: fn(l: float, wp: vec3, uv: vec2, ray: vec3, uv_ray: vec2, hit: float, bw: vec3, dx: vec3, dy: vec3, du1: vec2, du2: vec2, hex: float) -> float {
            var blocked = 0.0
            for i in 0..4 {
                let rise = hit * ((float(i) + 1.0) * 0.25)
                let h = self.splat_relief_read(l, wp + ray * rise, uv + uv_ray * rise, bw, dx, dy, du1, du2, hex, 0.0).w
                blocked = max(blocked, smoothstep(0.015, 0.08, h - (1.0 - hit + rise)))
            }
            return 1.0 - 0.65 * blocked
        }

        // Layer l, tested from the top of the paint down (`won` < 0: none
        // has covered yet).
        splat_try: fn(l: float, won: vec4, wa: vec4, wb: vec4, dom: float, nz: float, wp: vec3, bw: vec3, dx: vec3, dy: vec3) -> vec4 {
            if won.w >= 0.0 || l >= self.splat_size.w || l == dom { return won }
            let w = self.splat_weight(l, wa, wb)
            if w < 0.02 { return won }
            let t = self.splat_texel(l, wp, bw, dx, dy, 0.0)
            if w + (t.w - 0.5) * 0.45 + (nz - 0.5) * 0.5 < 0.36 { return won }
            return vec4(l, 0.0, 0.0, l)
        }

        pixel: fn() {
            self.host_cut()
            self.door_window()
            let wp = self.v_csm.xyz
            let dx = dFdx(wp)
            let dy = dFdy(wp)
            let carpet = self.grass_carpet_at(wp)
            let gn = abs(normalize(self.v_csm_n))
            var bw = gn * gn
            bw = bw * bw
            bw = bw / max(bw.x + bw.y + bw.z, 0.0001)
            // The weights: bilinear between the vertices, each page's rows
            // clamped so the filter never reaches the next page.
            let sz = self.splat_size
            let g = clamp((vec2(wp.x, wp.z) - self.splat_map.xy) * self.splat_map.z, vec2(0.0, 0.0), vec2(sz.x - 1.0, sz.y - 1.0)) + vec2(0.5, 0.5)
            let rows = sz.y * sz.z
            var wa = vec4(1.0, 0.0, 0.0, 0.0)
            if sz.w > 1.5 { wa = self.occlusion_map.sample_lod(vec2(g.x / sz.x, g.y / rows), 0.0) }
            var wb = vec4(0.0, 0.0, 0.0, 0.0)
            if sz.z > 1.5 { wb = self.occlusion_map.sample_lod(vec2(g.x / sz.x, (g.y + sz.y) / rows), 0.0) }
            // The heaviest layer: what shows where none covers.
            var dom = 0.0
            var best = wa.x
            if sz.w > 1.5 && wa.y > best {
                dom = 1.0
                best = wa.y
            }
            if sz.w > 2.5 && wa.z > best {
                dom = 2.0
                best = wa.z
            }
            if sz.w > 3.5 && wa.w > best {
                dom = 3.0
                best = wa.w
            }
            if sz.w > 4.5 && wb.x > best {
                dom = 4.0
                best = wb.x
            }
            if sz.w > 5.5 && wb.y > best {
                dom = 5.0
                best = wb.y
            }
            if sz.w > 6.5 && wb.z > best {
                dom = 6.0
                best = wb.z
            }
            if sz.w > 7.5 && wb.w > best {
                dom = 7.0
                best = wb.w
            }
            var won = vec4(0.0, 0.0, 0.0, 0.0 - 1.0)
            // Under a full carpet the paint does not show: no blend.
            if self.splat_blend > 0.5 && sz.w > 1.5 && carpet.w < 0.99 {
                let np = vec2(wp.x, wp.z)
                let nz = self.tn_noise(np * 0.45) * 0.6 + self.tn_noise(np * 1.9) * 0.4
                won = self.splat_try(7.0, won, wa, wb, dom, nz, wp, bw, dx, dy)
                won = self.splat_try(6.0, won, wa, wb, dom, nz, wp, bw, dx, dy)
                won = self.splat_try(5.0, won, wa, wb, dom, nz, wp, bw, dx, dy)
                won = self.splat_try(4.0, won, wa, wb, dom, nz, wp, bw, dx, dy)
                won = self.splat_try(3.0, won, wa, wb, dom, nz, wp, bw, dx, dy)
                won = self.splat_try(2.0, won, wa, wb, dom, nz, wp, bw, dx, dy)
                won = self.splat_try(1.0, won, wa, wb, dom, nz, wp, bw, dx, dy)
                won = self.splat_try(0.0, won, wa, wb, dom, nz, wp, bw, dx, dy)
            }
            var l = dom
            if won.w >= 0.0 { l = won.w }
            let p = self.splat_param(l)
            let surface_param = self.splat_surface_param(l)
            let s = p.x
            let dist = length(self.eye.xyz - wp)
            var n = normalize(self.v_csm_n)
            if self.double_sided > 0.5 && dot(n, self.toward_eye(wp)) < 0.0 { n = n * -1.0 }
            let geometric_normal = n
            // Terrain already has world projection axes. Meshes carry UVs,
            // not their generating brush axes (chamfers can inherit another
            // face's projection), so only that path uses UV derivatives.
            var axis_u = vec3(1.0, 0.0, 0.0)
            var axis_v = vec3(0.0, 0.0, 1.0)
            if n.y < 0.6 {
                if abs(n.x) > abs(n.z) {
                    axis_u = vec3(0.0, 0.0, sign(n.x))
                } else {
                    axis_u = vec3(0.0 - sign(n.z), 0.0, 0.0)
                }
                axis_v = vec3(0.0, -1.0, 0.0)
            }
            var uv = vec2(dot(wp, axis_u), dot(wp, axis_v)) * s
            var du1 = vec2(dot(dx, axis_u), dot(dx, axis_v)) * s
            var du2 = vec2(dot(dy, axis_u), dot(dy, axis_v)) * s
            if self.triplanar == 0.0 {
                uv = self.v_uv
                du1 = dFdx(self.v_uv)
                du2 = dFdy(self.v_uv)
            }
            // Hex tiling on the mesh-UV path and terrain's top projection.
            var hex = 0.0
            if self.triplanar == 0.0 || n.y >= 0.6 { hex = self.splat_hex_amount(l, du1, du2) }
            var tex = self.splat_relief_read(l, wp, uv, bw, dx, dy, du1, du2, hex, 1.0)
            var sun_visibility = 1.0
            // Uniform gate: no ray construction, march or shadow taps for
            // draws without relief. Only the visible winning layer marches.
            if self.splat_relief_on > 0.5 {
                if surface_param.x > 0.0 && dist < 20.0 {
                    var grad_u = axis_u * s
                    var grad_v = axis_v * s
                    if self.triplanar == 0.0 {
                        let area = dot(dx, cross(dy, n))
                        grad_u = vec3(0.0, 0.0, 0.0)
                        grad_v = vec3(0.0, 0.0, 0.0)
                        if abs(area) > 0.00000001 {
                            grad_u = (cross(dy, n) * du1.x + cross(n, dx) * du2.x) / area
                            grad_v = (cross(dy, n) * du1.y + cross(n, dx) * du2.y) / area
                        }
                    }
                    let view = normalize(self.toward_eye(wp))
                    let nv = dot(n, view)
                    // sin(8 degrees)..sin(16 degrees): entirely off below
                    // eight degrees, smoothly restored above it.
                    let relief_fade = (1.0 - smoothstep(12.0, 20.0, dist)) * smoothstep(0.1391731, 0.2756374, nv)
                    let tu = grad_u - n * dot(n, grad_u)
                    if relief_fade > 0.0 && dot(tu, tu) > 0.00000001 {
                        let tangent = normalize(tu)
                        let bitangent = normalize(cross(n, tangent))
                        let view_ts = vec3(dot(view, tangent), dot(view, bitangent), nv)
                        let depth_metres = surface_param.x * relief_fade
                        let ray = (tangent * view_ts.x + bitangent * view_ts.y) * (depth_metres / nv)
                        let uv_ray = vec2(dot(ray, grad_u), dot(ray, grad_v))
                        let hit = self.splat_relief_trace(l, wp, uv, ray, uv_ray, bw, dx, dy, du1, du2, tex.w, nv, hex)
                        let hit_wp = wp - ray * hit
                        uv = uv - uv_ray * hit
                        tex = self.splat_relief_read(l, hit_wp, uv, bw, dx, dy, du1, du2, hex, 1.0)
                        if dist < 8.0 {
                            let light = normalize(self.light_dir)
                            let nl = dot(n, light)
                            if nl > 0.0 {
                                let light_ts = vec3(dot(light, tangent), dot(light, bitangent), nl)
                                let sun_ray = (tangent * light_ts.x + bitangent * light_ts.y) * (depth_metres / max(nl, 0.0001))
                                let sun_uv = vec2(dot(sun_ray, grad_u), dot(sun_ray, grad_v))
                                let visibility = self.splat_relief_shadow(l, hit_wp, uv, sun_ray, sun_uv, hit, bw, dx, dy, du1, du2, hex)
                                sun_visibility = mix(1.0, visibility, relief_fade * (1.0 - smoothstep(6.0, 8.0, dist)))
                            }
                        }
                    }
                }
            }
            // The texel as painted, before the far variation: the carpet's grain.
            let grain = tex
            // Anti-tiling, as DrawScenePbr's triplanar path: with distance a
            // second top-down read at an unrelated scale and offset. Not for
            // a layer with the macro term or hex tiling: those carry the far
            // variation and the near.
            let mv = self.splat_macro_param(l)
            let far = smoothstep(25.0, 140.0, dist)
            if self.triplanar != 0.0 && far > 0.001 && mv.w <= 0.0 && self.splat_weight(l, self.splat_hex_a, self.splat_hex_b) < 0.5 {
                let s2 = s * 0.173
                let t2 = self.tex.sample_grad_aniso_repeat(vec3(wp.x * s2 + 0.31, wp.z * s2 - 0.47, l), vec2(dx.x, dx.z) * s2, vec2(dy.x, dy.z) * s2)
                tex = vec4(mix(tex.xyz, t2.xyz, 0.5 * far * bw.y), tex.w)
            }
            // World macro variation (the layer's own), else on terrain the
            // brightness drift over ~60 m.
            var macro_field = vec3(0.0, 0.0, 0.0)
            if mv.w > 0.0 {
                macro_field = self.splat_macro(mv, wp, max(length(dx), length(dy)))
            } else if self.triplanar != 0.0 {
                let drift = self.tn_noise(vec2(wp.x, wp.z) * 0.017) * 0.65 + self.tn_noise(vec2(wp.x, wp.z) * 0.061) * 0.35
                tex = vec4(tex.xyz * (0.86 + 0.28 * drift), tex.w)
            }
            // A worn road part's masks (a uniform gate: no band, none).
            var band_wear = vec4(1.0, 0.0, 0.0, 0.0)
            if self.splat_band.y > 0.0 {
                let bc = self.splat_band_coords()
                band_wear = self.splat_band_wear(bc, abs(dFdx(bc)) + abs(dFdy(bc)), surface_param.y)
            }
            var water = 0.0
            if surface_param.z > 0.0 {
                // Low-frequency, world-anchored patches, continuous on walls
                // as well as floors. No noise texture or extra height read.
                let patch = self.tn_noise(vec2(wp.x + wp.y * 0.31, wp.z + wp.y * 0.73) * 0.22)
                let water_height = tex.w + (patch - 0.5) * (0.16 * (1.0 - surface_param.z))
                water = 1.0 - smoothstep(surface_param.w - 0.03, surface_param.w + 0.03, water_height)
                // A patch mask also dries the deepest joints between pools;
                // a tiny threshold wobble alone would wet every mortar line.
                water = water * smoothstep(0.25 * (1.0 - surface_param.z), max(0.65 * (1.0 - surface_param.z), 0.0001), patch)
                if surface_param.z >= 1.0 { water = 1.0 }
            }
            water = max(water, band_wear.z)
            let wet = surface_param.y
            let hue = macro_field.z * mv.y * 2.0
            let base = self.to_scene(vec3(tex.x, tex.y, tex.z)) * ((1.0 - 0.35 * wet) * (1.0 - 0.35 * water))
                * (1.0 + macro_field.x * mv.x) * vec3(1.0 + hue, 1.0, 1.0 - hue)
                * band_wear.x * mix(vec3(1.0, 1.0, 1.0), vec3(0.86, 0.8, 0.72), band_wear.w)
            var vertex_tint = self.to_lin(self.v_tint.xyz)
            if self.triplanar == 0.0 { vertex_tint = self.to_scene(self.v_tint.xyz) }
            let surf = self.surface(vec4(self.color_adjust(
                base * vertex_tint,
                self.tint,
                self.color_adjust_ctl
            ), 1.0))
            // One read for both the normal and roughness, at the same hit.
            var normal_rough = vec4(0.5, 0.5, 1.0, 0.5)
            if hex < 1.0 { normal_rough = self.normal_map.sample_grad_aniso_repeat(vec3(uv.x, uv.y, l), du1, du2) }
            if hex > 0.0 { normal_rough = mix(normal_rough, self.splat_hex_normal(l, uv, du1, du2), hex) }
            let determinant = du1.x * du2.y - du1.y * du2.x
            if p.w > 0.00001 && abs(determinant) > 0.00000001 {
                let orientation = sign(determinant)
                let tangent = normalize(dx * du2.y - dy * du1.y) * orientation
                let bitangent = normalize(dy * du1.x - dx * du2.x) * orientation
                let xy = normal_rough.xy * 2.0 - vec2(1.0, 1.0)
                let mapped = vec3(xy.x, xy.y, sqrt(max(1.0 - dot(xy, xy), 0.0)))
                n = normalize(tangent * (mapped.x * p.w) + bitangent * (mapped.y * p.w) + n * mapped.z)
            }
            n = self.mat_normal(n)
            if water > 0.0 { n = normalize(mix(n, geometric_normal, water)) }
            let dry_rough = clamp(p.y + (normal_rough.w - 0.5) * 0.7 + macro_field.y * mv.z + band_wear.y, 0.04, 1.0)
            let wet_rough = mix(dry_rough, 0.08, wet)
            let mr = self.mat_metal_rough(vec2(clamp(p.z, 0.0, 1.0) * (1.0 - water), mix(wet_rough, 0.03, water)))
            let f0 = mix(mix(vec3(0.04, 0.04, 0.04), surf.xyz, mr.x), vec3(0.02, 0.02, 0.02), water)
            let lit = self.shade_mr_surface(surf.xyz, surf.w, mr, n, vec3(0.0, 0.0, 0.0), f0, sun_visibility)
            if carpet.w > 0.0 {
                // The paint's grain carries into the carpet: the shown texel
                // against a read of eight times its footprint (three mips up).
                let wide = self.splat_read(vec3(uv.x, uv.y, l), du1 * 8.0, du2 * 8.0, 0.0)
                let luma = vec3(0.3, 0.6, 0.1)
                let detail = clamp(dot(grain.xyz, luma) / max(dot(wide.xyz, luma), 0.02), 0.6, 1.4)
                return mix(lit, self.grass_carpet(wp, carpet, max(length(dx), length(dy)), detail), carpet.w)
            }
            return lit
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }
}
