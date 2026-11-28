//! The shared scene and Sun intersection for dense R8 volume tiles.

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.geom

    mod.draw.TracedVolumeHit = {
        trace_cells_per_unit: uniform(1.0)
        trace_tile_steps: uniform(192.0)
        trace_coarse_only: uniform(0.0)
        trace_coarse: texture_3d(float)
        traced_scale: fn() -> float { return self.trace_cells_per_unit }
        traced_point: fn() -> vec3 {
            return vec3(unpack2f16(self.geom.uv), unpack2f16(self.geom.nrm).x)
        }
        traced_min: fn() -> vec3 { return vec3(0.0) }
        traced_max: fn() -> vec3 { return round(unpack4u8(self.geom.ao_uv).xyz * 255.0) }

        volume_child_occupied: fn(byte: u32, cell: vec3i) -> bool {
            let cx = (u32(cell.x) >> 1u) & 1u
            let cy = (u32(cell.y) >> 1u) & 1u
            let cz = (u32(cell.z) >> 1u) & 1u
            return (byte & (1u << (cx + 2u * cy + 4u * cz))) != 0u
        }
        volume_exit_plane: fn(lo: i32, size: i32, d: float) -> i32 {
            if d > 0.0 { return lo + size }
            return lo
        }
        volume_exit_t: fn(lo: i32, size: i32, pa: float, d: float) -> float {
            if d == 0.0 { return 1.0e30 }
            return (f32(self.volume_exit_plane(lo, size, d)) - pa) / d
        }
        volume_ray_cell: fn(at: float, d: float) -> i32 {
            if d >= 0.0 { return i32(floor(at)) }
            return i32(ceil(at)) - 1i
        }
        volume_step_axis: fn(cell: i32, lo: i32, size: i32, pa: float, d: float,
            crossing: float, t_next: float) -> i32 {
            if crossing <= t_next {
                if d > 0.0 { return lo + size }
                return lo - 1i
            }
            if d == 0.0 { return cell }
            let next = clamp(self.volume_ray_cell(pa + t_next * d, d), lo, lo + size - 1i)
            if d > 0.0 { return max(cell, next) }
            return min(cell, next)
        }

        // A block exit can coincide with a different unit face.
        volume_entry_face: fn(cell: vec3i, pa: vec3, d: vec3) -> vec2 {
            let x = self.trace_axis_entry(pa.x, d.x, f32(cell.x), f32(cell.x + 1i))
            let y = self.trace_axis_entry(pa.y, d.y, f32(cell.y), f32(cell.y + 1i))
            let z = self.trace_axis_entry(pa.z, d.z, f32(cell.z), f32(cell.z + 1i))
            if x >= y && x >= z { return vec2(-sign(d.x), 0.0) }
            if y >= z { return vec2(0.0, -sign(d.y)) }
            return vec2(step(0.0, d.z))
        }
        traced_hit: fn(p: vec3, d: vec3) -> vec4 {
            let extent = round(self.v_trace_hi)
            let sx = self.trace_slab(p.x, d.x, 0.0, extent.x)
            let sy = self.trace_slab(p.y, d.y, 0.0, extent.y)
            let sz = self.trace_slab(p.z, d.z, 0.0, extent.z)
            let t_enter = max(max(sx.x, sy.x), sz.x)
            let exit_t = min(min(sx.y, sy.y), sz.y)
            return self.traced_hit_interval(p, d, t_enter, exit_t)
        }
        traced_hit_interval: fn(p: vec3, d: vec3, t_enter: float, exit_t: float) -> vec4 {
            var t = max(t_enter, 0.0)
            let miss = vec4(-1.0, 0.0, 0.0, 0.0)
            if t >= exit_t { return miss }

            let extent = round(self.v_trace_hi)
            let origin = round(self.v_trace_payload.xyz * 255.0)
            let pa = p + origin
            let lo = vec3i(i32(origin.x), i32(origin.y), i32(origin.z))
            let hi = lo + vec3i(i32(extent.x), i32(extent.y), i32(extent.z))
            let at = pa + t * d
            var cell = clamp(vec3i(self.volume_ray_cell(at.x, d.x), self.volume_ray_cell(at.y, d.y),
                self.volume_ray_cell(at.z, d.z)), lo, hi - vec3i(1i))
            var cached = vec3i(-1i)
            var byte = 0u
            let max_steps = u32(self.trace_tile_steps)
            for i in 0..384 {
                if i >= max_steps { break }
                let block = cell / vec3i(4i)
                if block.x != cached.x || block.y != cached.y || block.z != cached.z {
                    byte = u32(round(self.trace_coarse.load(block).x * 255.0))
                    cached = block
                }
                var size = 1i
                if byte == 0u {
                    size = 4i
                } else if !self.volume_child_occupied(byte, cell) {
                    size = 2i
                } else if self.trace_coarse_only > 0.5 {
                    return vec4(t, self.volume_entry_face(cell, pa, d), 1.0)
                }
                if size == 1i {
                    let m = round(self.trace_volume.load(cell).x * 255.0)
                    if m > 0.0 {
                        return vec4(t, self.volume_entry_face(cell, pa, d), m)
                    }
                }
                let block_lo = (cell / vec3i(size)) * vec3i(size)
                let tx = self.volume_exit_t(block_lo.x, size, pa.x, d.x)
                let ty = self.volume_exit_t(block_lo.y, size, pa.y, d.y)
                let tz = self.volume_exit_t(block_lo.z, size, pa.z, d.z)
                let t_next = min(min(tx, ty), tz)
                if t_next >= exit_t { break }
                cell = vec3i(self.volume_step_axis(cell.x, block_lo.x, size, pa.x, d.x, tx, t_next),
                    self.volume_step_axis(cell.y, block_lo.y, size, pa.y, d.y, ty, t_next),
                    self.volume_step_axis(cell.z, block_lo.z, size, pa.z, d.z, tz, t_next))
                if cell.x < lo.x || cell.y < lo.y || cell.z < lo.z || cell.x >= hi.x || cell.y >= hi.y || cell.z >= hi.z { break }
                t = t_next
            }
            return miss
        }
    }

    mod.draw.TracedVolumePalette = {
        volume_palette_row: fn(material: float, row: float) -> vec4 {
            let page = round(self.v_trace_aux.w * 255.0)
            let pixel = vec2(material + 0.5, 3.0 * page + row + 0.5)
            return self.trace_palette.sample_nearest(pixel / self.trace_palette.size(), 0.0)
        }
        traced_base: fn(material: float, p: vec3) -> vec4 { return self.volume_palette_row(material, 0.0) }
        traced_mr: fn(material: float) -> vec4 { return self.volume_palette_row(material, 1.0) }
        traced_emission: fn(material: float) -> vec3 { return self.volume_palette_row(material, 2.0).xyz }
    }

    // Only dense scene volumes store material IDs at these atlas coordinates.
    mod.draw.TracedVolumeAo = {
        volume_ao_occ: fn(q: vec3i, lo: vec3i, hi: vec3i) -> float {
            if q.x < lo.x || q.y < lo.y || q.z < lo.z || q.x >= hi.x || q.y >= hi.y || q.z >= hi.z {
                return 0.0
            }
            return step(0.5, round(self.trace_volume.load(q).x * 255.0))
        }
        traced_ao: fn(hit: vec4, p: vec3, d: vec3) -> float {
            let strength = clamp(self.voxel_ao_strength, 0.0, 1.0)
            if strength <= 0.0001 { return 1.0 }
            let extent = round(self.v_trace_hi)
            let origin = round(self.v_trace_payload.xyz * 255.0)
            let lo = vec3i(i32(origin.x), i32(origin.y), i32(origin.z))
            let hi = lo + vec3i(i32(extent.x), i32(extent.y), i32(extent.z))
            let n = round(self.oct_decode(hit.yz))
            let p_hit = p + d * hit.x + origin
            let pf = floor(p_hit)
            let cell_t = clamp(vec3i(i32(pf.x), i32(pf.y), i32(pf.z)), lo, hi - vec3i(1i))
            var base = cell_t
            var t1 = vec3i(0i)
            var t2 = vec3i(0i)
            var u = 0.0
            var v = 0.0
            if abs(n.x) > 0.5 {
                base = vec3i(i32(floor(p_hit.x + n.x * 0.5)), cell_t.y, cell_t.z)
                t1 = vec3i(0i, 1i, 0i)
                t2 = vec3i(0i, 0i, 1i)
                u = clamp(p_hit.y - f32(cell_t.y), 0.0, 1.0)
                v = clamp(p_hit.z - f32(cell_t.z), 0.0, 1.0)
            } else if abs(n.y) > 0.5 {
                base = vec3i(cell_t.x, i32(floor(p_hit.y + n.y * 0.5)), cell_t.z)
                t1 = vec3i(1i, 0i, 0i)
                t2 = vec3i(0i, 0i, 1i)
                u = clamp(p_hit.x - f32(cell_t.x), 0.0, 1.0)
                v = clamp(p_hit.z - f32(cell_t.z), 0.0, 1.0)
            } else {
                base = vec3i(cell_t.x, cell_t.y, i32(floor(p_hit.z + n.z * 0.5)))
                t1 = vec3i(1i, 0i, 0i)
                t2 = vec3i(0i, 1i, 0i)
                u = clamp(p_hit.x - f32(cell_t.x), 0.0, 1.0)
                v = clamp(p_hit.y - f32(cell_t.y), 0.0, 1.0)
            }
            let s_u0 = self.volume_ao_occ(base - t1, lo, hi)
            let s_u1 = self.volume_ao_occ(base + t1, lo, hi)
            let s_v0 = self.volume_ao_occ(base - t2, lo, hi)
            let s_v1 = self.volume_ao_occ(base + t2, lo, hi)
            let occ_u = mix(s_u0, s_u1, u)
            let occ_v = mix(s_v0, s_v1, v)
            let occ = clamp(0.5 * (occ_u + occ_v) + 0.25 * occ_u * occ_v, 0.0, 1.0)
            return mix(1.0, mix(1.0, 0.45, occ), strength)
        }
    }

    mod.draw.DrawTracedSun = #(DrawTracedSun::script_shader(vm)){
        ..mod.draw.TracedProxy
        alpha_blend: false
        // shadow_csm.rs:311 builds right = up_hint × fwd, winding Sun-facing triangles clockwise.
        cull_mode: @None
        color_format: @Rf32
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        depth_out: mod.shader.fragment_depth(@greater)
        geom: vertex_buffer(geom.GameMeshVertexAo, geom.GameMeshAoGeom)
        trace_volume: texture_3d(float)
        // Cascade XY, depth at the proxy face, and depth gained per cell along the ray.
        v_sun: varying(vec4f)

        cascade_depth: fn(z01: float, window: vec2) -> float {
            return z01 * (1.0 - window.x) + window.y
        }
        vertex: fn() {
            self.trace_proxy_vertex()
            let d = normalize(self.trace_local_ray(self.transform, normalize(self.sun_rz.xyz)))
            self.v_trace_ray = d
            let cell_step = (self.transform * vec4(d / self.traced_scale(), 0.0)).xyz
            let wp = self.transform * vec4(self.geom.px, self.geom.py, self.geom.pz, 1.0)
            let xy = vec2(dot(self.sun_rx.xyz, wp.xyz) + self.sun_rx.w, dot(self.sun_ry.xyz, wp.xyz) + self.sun_ry.w)
            let z01 = dot(self.sun_rz.xyz, wp.xyz) + self.sun_rz.w
            self.v_sun = vec4(xy, z01, dot(self.sun_rz.xyz, cell_step))
            self.vertex_pos = vec4(xy * self.tile_a.xy + self.tile_a.zw, self.cascade_depth(z01, self.flip_a.yz), 1.0)
        }
        fragment: fn() {
            if abs(self.v_sun.x) > 1.001 || abs(self.v_sun.y) > 1.001 { discard() }
            let p = self.v_trace_p
            let d = self.v_trace_ray
            if !self.trace_entry_face(p, d) { discard() }
            let hit = self.traced_hit(p, d)
            if hit.x < 0.0 { discard() }
            let z01 = self.v_sun.z + hit.x * self.v_sun.w
            self.depth_out = max(self.depth_out, self.cascade_depth(z01, self.flip_a.yz))
            self.fb0 = vec4(z01, 0.0, 0.0, 1.0)
        }
    }

    mod.draw.DrawSceneTracedVolume = mod.draw.DrawSceneTraced{
        ..mod.draw.TracedVolumePalette,
        ..mod.draw.TracedVolumeAo,
        ..mod.draw.TracedVolumeHit
        cull_mode: @Back
    }
    mod.draw.DrawSceneTracedVolumeInside = mod.draw.DrawSceneTracedInside{
        ..mod.draw.TracedVolumePalette,
        ..mod.draw.TracedVolumeAo,
        ..mod.draw.TracedVolumeHit
        cull_mode: @Front
    }
    mod.draw.DrawTracedSunVolume = mod.draw.DrawTracedSun{ ..mod.draw.TracedVolumeHit }
}
