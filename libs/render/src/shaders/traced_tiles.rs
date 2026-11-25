//! Tile payload and intersection hooks. The consuming pass owns the ray and its outputs.

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.geom

    mod.draw.TracedTileHit = {
        trace_cells_per_unit: uniform(1.0)
        traced_scale: fn() -> float { return self.trace_cells_per_unit }
        traced_point: fn() -> vec3 {
            return vec3(unpack2f16(self.geom.uv), unpack2f16(self.geom.nrm).x)
        }
        traced_max: fn() -> vec3 {
            return floor(round(unpack4u8(self.geom.ao_uv).xyz * 255.0) / 16.0)
        }
        traced_min: fn() -> vec3 {
            let bounds = round(unpack4u8(self.geom.ao_uv).xyz * 255.0)
            return bounds - floor(bounds / 16.0) * 16.0
        }

        // Entry, exit and time per cell along one axis. A parallel ray owns the
        // low side of a shared plane, so adjacent boxes cannot both claim it.
        tile_slab: fn(p: float, d: float, lo: float, hi: float) -> vec3 {
            if d == 0.0 {
                if p < lo || p >= hi { return vec3(1.0e30, -1.0e30, 0.0) }
                return vec3(-1.0e30, 1.0e30, 0.0)
            }
            let a = (lo - p) / d
            let b = (hi - p) / d
            return vec3(min(a, b), max(a, b), min(abs(1.0 / d), 1.0e30))
        }
        tile_next: fn(p: float, d: float, cell: float) -> float {
            if d > 0.0 { return (cell + 1.0 - p) / d }
            if d < 0.0 { return (cell - p) / d }
            return 1.0e30
        }
        traced_hit: fn(p: vec3, d: vec3) -> vec4 {
            let lo = round(self.v_trace_lo)
            let hi = round(self.v_trace_hi)
            let sx = self.tile_slab(p.x, d.x, lo.x, hi.x)
            let sy = self.tile_slab(p.y, d.y, lo.y, hi.y)
            let sz = self.tile_slab(p.z, d.z, lo.z, hi.z)
            let entry = max(sx.x, max(sy.x, sz.x))
            let exit_t = min(sx.y, min(sy.y, sz.y))
            var t = max(entry, 0.0)
            if t >= exit_t { return vec4(-1.0, 0.0, 0.0, 0.0) }

            let origin = round(self.v_trace_payload.xyz * 255.0) * 8.0
            let at = p + t * d
            let stride = sign(d)
            // At an integer plane, start on its forward side, even for -d.
            var cell = clamp(mix(ceil(at) - vec3(1.0), floor(at), step(vec3(0.0), d)), lo, hi - vec3(1.0))
            var crossing = vec3(self.tile_next(p.x, d.x, cell.x), self.tile_next(p.y, d.y, cell.y), self.tile_next(p.z, d.z, cell.z))
            let delta = vec3(sx.z, sy.z, sz.z)
            // Octahedral encodings of the three possible entry normals.
            let nx = vec2(-stride.x, 0.0)
            let ny = vec2(0.0, -stride.y)
            let nz = vec2(step(0.0, d.z))
            var face = nz
            if entry == sx.x { face = nx } else if entry == sy.x { face = ny }
            // At most 1 + 3*7 cells. Step every tied axis at once: cells
            // touched only along an edge have zero ray length. X then Y wins
            // the normal when entering through more than one face.
            for i in 0..24 {
                let q = origin + cell
                let m = round(self.trace_volume.load(vec3i(i32(q.x), i32(q.y), i32(q.z))).x * 255.0)
                if m > 0.0 { return vec4(t, face, m) }
                t = min(crossing.x, min(crossing.y, crossing.z))
                if t >= exit_t { break }
                let axes = step(crossing, vec3(t))
                face = nz
                if axes.x > 0.0 { face = nx } else if axes.y > 0.0 { face = ny }
                cell = cell + axes * stride
                if cell.x < lo.x || cell.y < lo.y || cell.z < lo.z || cell.x >= hi.x || cell.y >= hi.y || cell.z >= hi.z { break }
                crossing = crossing + axes * delta
            }
            return vec4(-1.0, 0.0, 0.0, 0.0)
        }
    }
    mod.draw.TracedTiles = {
        ..mod.draw.TracedTileHit
        tile_palette_row: fn(material: float, row: float) -> vec4 {
            let page = round(self.v_trace_aux.w * 255.0)
            let pixel = vec2(material + 0.5, 3.0 * page + row + 0.5)
            return self.trace_palette.sample_nearest(pixel / self.trace_palette.size(), 0.0)
        }
        traced_base: fn(material: float, p: vec3) -> vec4 { return self.tile_palette_row(material, 0.0) }
        traced_mr: fn(material: float) -> vec4 { return self.tile_palette_row(material, 1.0) }
        traced_emission: fn(material: float) -> vec3 { return self.tile_palette_row(material, 2.0).xyz }
    }

    mod.draw.DrawSceneTracedTiles = mod.draw.DrawSceneTraced{ ..mod.draw.TracedTiles }

    mod.draw.DrawTracedSunTiles = #(DrawTracedSun::script_shader(vm)){
        ..mod.draw.TracedProxy,
        ..mod.draw.TracedTileHit
        alpha_blend: false
        backface_culling: false
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
}
