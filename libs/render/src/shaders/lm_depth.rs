//! Light-map/CSM depth passes: sun depth (static and skinned) and lamp depth.

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    // Sun-view depth: all static geometry (mesh instances + the occluder
    // boxes packed into one world-space geometry) rasterized from a
    // region's fitted ortho sun camera. Double-sided on purpose — the kits
    // are double-sided and the CPU rays hit both faces.
    mod.draw.DrawLmSunDepth = mod.std.set_type_default() do #(DrawLmSunDepth::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        color_format: @Rf32
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.GameMeshVertexAo, geom.GameMeshAoGeom)
        v_d: varying(float)
        v_clip: varying(vec2f)

        vertex: fn() {
            var pos=vec3(self.geom.px,self.geom.py,self.geom.pz)
            if self.morph_ctl.w > 0.5 { pos=pos+self.morph_delta(self.geom.ao_uv,0.0) }
            let wp = self.transform * vec4(pos, 1.0)
            let nx = dot(self.sun_rx.xyz, wp.xyz) + self.sun_rx.w
            let ny = dot(self.sun_ry.xyz, wp.xyz) + self.sun_ry.w
            let nz = dot(self.sun_rz.xyz, wp.xyz) + self.sun_rz.w
            self.v_d = nz
            self.v_clip = vec2(nx, ny)
            // flip_a.x = 1: FLIPPED depth test — LessEqual on (1 - z01)
            // keeps the LARGEST z01 = the surface nearest the GROUND along
            // the sun ray. The far map feeds the shadow-top plane, which
            // wants the CPU rays' first-hit (a slab's underside), not the
            // sun's view of its top.
            let zq = nz + self.flip_a.x * (1.0 - 2.0 * nz)
            // tile_a places the cascade pass's tiles in the grid; the atlas
            // passes use the identity tile. flip_a.yz = the cascade tiles'
            // depth window (shadow_csm::CSM_DEPTH_WINDOW):
            // z * (1 - y) + z_offset; zero = identity for the atlas passes.
            self.vertex_pos = vec4(
                nx * self.tile_a.x + self.tile_a.z,
                ny * self.tile_a.y + self.tile_a.w,
                zq * (1.0 - self.flip_a.y) + self.flip_a.z,
                1.0
            )
        }

        pixel: fn() {
            // Fragments past their own tile's edge belong to a NEIGHBOUR
            // cascade's tile — the hardware only clips at the strip's
            // border. No-op for full-target passes (nothing rasterizes
            // past ndc 1).
            if abs(self.v_clip.x) > 1.001 {
                discard()
            }
            if abs(self.v_clip.y) > 1.001 {
                discard()
            }
            return vec4(self.v_d, 0.0, 0.0, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
        morph_map: texture_2d(float)
        // The weight of morph target k (0..31): morph_weights0..7, four each.
        morph_weight: fn(k: float) -> float {
            let i = floor(k / 4.0)
            var w = self.morph_weights0
            if i > 0.5 { w = self.morph_weights1 }
            if i > 1.5 { w = self.morph_weights2 }
            if i > 2.5 { w = self.morph_weights3 }
            if i > 3.5 { w = self.morph_weights4 }
            if i > 4.5 { w = self.morph_weights5 }
            if i > 5.5 { w = self.morph_weights6 }
            if i > 6.5 { w = self.morph_weights7 }
            let c = k - i * 4.0
            if c < 0.5 { return w.x }
            if c < 1.5 { return w.y }
            if c < 2.5 { return w.z }
            return w.w
        }
        // The first morph_ctl.w targets' deltas, weighted, in target order
        // (one loop: the body is compiled once, not per target).
        morph_delta: fn(vertex:float,lane:float)->vec3f {
            var delta=vec3(0.0,0.0,0.0)
            var k=0.0
            while k < 31.5 && self.morph_ctl.w > k + 0.5 {
                let index=(k*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weight(k)
                k=k+1.0
            }
            return delta
        }
    }

    // Sun-view depth, SKINNED variant: the same projection over a rig's
    // REST mesh (geom.GameMeshVertexSkin), position-blended in the vertex
    // stage against the frame's joint-palette texture — exactly
    // DrawSceneSkinnedGpu's skinning minus the normal work a depth map never
    // reads. The Realtime cascades rasterize characters with this, so
    // their shadows land in the maps like every other caster's.
    // skin_a.x = the instance's first palette texel (joint_base).
    // DrawLmSunDepth without the tile clip, for casters the CPU proved lie
    // wholly inside their tile: no discard, so hidden-surface removal stays.
    mod.draw.DrawLmSunDepthInside = mod.std.set_type_default() do #(DrawLmSunDepthInside::script_shader(vm)){
        ..mod.draw.DrawLmSunDepth
        pixel: fn() {
            return vec4(self.v_d, 0.0, 0.0, 1.0)
        }
    }

    mod.draw.DrawLmSunDepthSkinned = mod.std.set_type_default() do #(DrawLmSunDepthSkinned::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        color_format: @Rf32
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.GameMeshVertexSkin, geom.GameMeshSkinGeom)
        joint_tex: texture_2d(float)
        v_d: varying(float)
        v_clip: varying(vec2f)

        // Palette row fetch, verbatim from DrawSceneSkinnedGpu: nearest at
        // texel centres with explicit lod (vertex stage, RGBA32F).
        jrow: fn(t: float) -> vec4f {
            let dim = self.joint_tex.size()
            let y = floor(t / dim.x)
            let x = t - y * dim.x
            return self.joint_tex.sample_nearest(
                vec2((x + 0.5) / dim.x, (y + 0.5) / dim.y),
                0.0
            )
        }

        // Position-only 4-influence blend (DrawSceneSkinnedGpu's, normals
        // dropped). MUST stay in lockstep with the visible draw or the
        // baked shadow detaches from the body that casts it.
        skinned_pos: fn() -> vec3f {
            var rest = vec4(self.geom.px, self.geom.py, self.geom.pz, 1.0)
            if self.morph_ctl.w > 0.5 { rest=vec4(rest.xyz+self.morph_delta(self.geom.source_vertex,0.0),1.0) }
            let jj = unpack4u8(self.geom.joints)
            let jw = unpack4u8(self.geom.weights)
            var pos = vec3(0.0, 0.0, 0.0)
            if jw.x > 0.0 {
                let b = self.skin_a.x + floor(jj.x * 255.0 + 0.5) * 3.0
                pos = pos + vec3(
                    dot(self.jrow(b), rest),
                    dot(self.jrow(b + 1.0), rest),
                    dot(self.jrow(b + 2.0), rest)
                ) * jw.x
            }
            if jw.y > 0.0 {
                let b = self.skin_a.x + floor(jj.y * 255.0 + 0.5) * 3.0
                pos = pos + vec3(
                    dot(self.jrow(b), rest),
                    dot(self.jrow(b + 1.0), rest),
                    dot(self.jrow(b + 2.0), rest)
                ) * jw.y
            }
            if jw.z > 0.0 {
                let b = self.skin_a.x + floor(jj.z * 255.0 + 0.5) * 3.0
                pos = pos + vec3(
                    dot(self.jrow(b), rest),
                    dot(self.jrow(b + 1.0), rest),
                    dot(self.jrow(b + 2.0), rest)
                ) * jw.z
            }
            if jw.w > 0.0 {
                let b = self.skin_a.x + floor(jj.w * 255.0 + 0.5) * 3.0
                pos = pos + vec3(
                    dot(self.jrow(b), rest),
                    dot(self.jrow(b + 1.0), rest),
                    dot(self.jrow(b + 2.0), rest)
                ) * jw.w
            }
            return pos
        }

        vertex: fn() {
            let pos = self.skinned_pos()
            let wp = self.transform * vec4(pos.x, pos.y, pos.z, 1.0)
            let nx = dot(self.sun_rx.xyz, wp.xyz) + self.sun_rx.w
            let ny = dot(self.sun_ry.xyz, wp.xyz) + self.sun_ry.w
            let nz = dot(self.sun_rz.xyz, wp.xyz) + self.sun_rz.w
            self.v_d = nz
            self.v_clip = vec2(nx, ny)
            // Same flip, tile and depth-generation contracts as DrawLmSunDepth.
            let zq = nz + self.flip_a.x * (1.0 - 2.0 * nz)
            self.vertex_pos = vec4(
                nx * self.tile_a.x + self.tile_a.z,
                ny * self.tile_a.y + self.tile_a.w,
                zq * (1.0 - self.flip_a.y) + self.flip_a.z,
                1.0
            )
        }

        pixel: fn() {
            if abs(self.v_clip.x) > 1.001 {
                discard()
            }
            if abs(self.v_clip.y) > 1.001 {
                discard()
            }
            return vec4(self.v_d, 0.0, 0.0, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
        morph_map: texture_2d(float)
        // The weight of morph target k (0..31): morph_weights0..7, four each.
        morph_weight: fn(k: float) -> float {
            let i = floor(k / 4.0)
            var w = self.morph_weights0
            if i > 0.5 { w = self.morph_weights1 }
            if i > 1.5 { w = self.morph_weights2 }
            if i > 2.5 { w = self.morph_weights3 }
            if i > 3.5 { w = self.morph_weights4 }
            if i > 4.5 { w = self.morph_weights5 }
            if i > 5.5 { w = self.morph_weights6 }
            if i > 6.5 { w = self.morph_weights7 }
            let c = k - i * 4.0
            if c < 0.5 { return w.x }
            if c < 1.5 { return w.y }
            if c < 2.5 { return w.z }
            return w.w
        }
        // The first morph_ctl.w targets' deltas, weighted, in target order
        // (one loop: the body is compiled once, not per target).
        morph_delta: fn(vertex:float,lane:float)->vec3f {
            var delta=vec3(0.0,0.0,0.0)
            var k=0.0
            while k < 31.5 && self.morph_ctl.w > k + 0.5 {
                let index=(k*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weight(k)
                k=k+1.0
            }
            return delta
        }
    }

    // Lamp-view depth: the same geometry seen from a lamp, six 90-degree
    // faces tiled 3x2 into one scratch. The face view is three row vec4s
    // like the sun camera; the tile mapping rides in clip space (linear in
    // clip components, so the hardware interpolates it correctly) and
    // fragments that rasterize past their tile's frustum are discarded.
    // Near plane = LIGHT_CLEARANCE: geometry hugging the bulb (the lamp
    // fixture itself) clips out of the map instead of eclipsing the light.
    // DrawLmSunDepth plus the layer's alpha: a leaf card casts leaves.
    mod.draw.DrawLmSunDepthCutout = mod.std.set_type_default() do #(DrawLmSunDepthCutout::script_shader(vm)){
        ..mod.draw.DrawLmSunDepth
        tex: texture_2d(float)
        v_uv: varying(vec2f)

        vertex: fn() {
            let pos = vec3(self.geom.px, self.geom.py, self.geom.pz)
            let wp = self.transform * vec4(pos, 1.0)
            let nx = dot(self.sun_rx.xyz, wp.xyz) + self.sun_rx.w
            let ny = dot(self.sun_ry.xyz, wp.xyz) + self.sun_ry.w
            let nz = dot(self.sun_rz.xyz, wp.xyz) + self.sun_rz.w
            self.v_d = nz
            self.v_clip = vec2(nx, ny)
            self.v_uv = unpack2f16(self.geom.uv)
            let zq = nz + self.flip_a.x * (1.0 - 2.0 * nz)
            self.vertex_pos = vec4(
                nx * self.tile_a.x + self.tile_a.z,
                ny * self.tile_a.y + self.tile_a.w,
                zq * (1.0 - self.flip_a.y) + self.flip_a.z,
                1.0
            )
        }

        pixel: fn() {
            if abs(self.v_clip.x) > 1.001 || abs(self.v_clip.y) > 1.001 {
                discard()
            }
            if self.tex.sample_repeat(self.v_uv).w < 0.5 {
                discard()
            }
            return vec4(self.v_d, 0.0, 0.0, 1.0)
        }
    }

    // A far stand-in (impostor.rs) casting: its quad turned to face the
    // sun, cut by the coverage of the baked view nearest to the sun's
    // direction, so the shadow of a far tree is that tree's silhouette as
    // the sun sees it (the stand-in's quad itself lies in the model's XY
    // plane and would cast a square). The quad carries centre and radius
    // (positions = centre + corner x radius, uv = corner x radius). Depth
    // is the crown centre's: at the outer cascades' texel size the crown's
    // own depth range does not matter.
    mod.draw.DrawLmSunDepthImpostor = mod.std.set_type_default() do #(DrawLmSunDepthImpostor::script_shader(vm)){
        ..mod.draw.DrawLmSunDepthCutout

        vertex: fn() {
            let corner_r = unpack2f16(self.geom.uv)
            let r = max(abs(corner_r.x), 0.001)
            let corner = vec2(corner_r.x / r, corner_r.y / r)
            let center = vec3(self.geom.px - corner_r.x, self.geom.py - corner_r.y, self.geom.pz)
            let ax = (self.transform * vec4(1.0, 0.0, 0.0, 0.0)).xyz
            let ay = (self.transform * vec4(0.0, 1.0, 0.0, 0.0)).xyz
            let az = (self.transform * vec4(0.0, 0.0, 1.0, 0.0)).xyz
            let s = length(ax)
            let cw = (self.transform * vec4(center.x, center.y, center.z, 1.0)).xyz
            let rx = normalize(self.sun_rx.xyz)
            let ry = normalize(self.sun_ry.xyz)
            // Depth grows away from the sun: the view toward the sun is -rz.
            let ts = 0.0 - normalize(self.sun_rz.xyz)
            let vd = normalize(vec3(dot(ax, ts), dot(ay, ts), dot(az, ts)))
            let rl = normalize(vec3(dot(ax, rx), dot(ay, rx), dot(az, rx)))
            let ul = normalize(vec3(dot(ax, ry), dot(ay, ry), dot(az, ry)))
            let off = (rl * corner.x + ul * corner.y) * r
            // The nearest baked view (impostor.rs frame_dir / frame_basis).
            let hv = normalize(vec3(vd.x, max(vd.y, 0.0), vd.z))
            let hs = abs(hv.x) + abs(hv.y) + abs(hv.z)
            let g = floor(clamp((vec2(hv.x / hs + hv.z / hs, hv.x / hs - hv.z / hs) * 0.5 + vec2(0.5, 0.5)) * #(crate::impostor::GRID_LAST_F), vec2(0.0, 0.0), vec2(#(crate::impostor::GRID_LAST_F), #(crate::impostor::GRID_LAST_F))) + vec2(0.5, 0.5))
            let fuv = g * (2.0 / #(crate::impostor::GRID_LAST_F)) - vec2(1.0, 1.0)
            let fx = (fuv.x + fuv.y) * 0.5
            let fz = (fuv.x - fuv.y) * 0.5
            let f = normalize(vec3(fx, max(1.0 - abs(fx) - abs(fz), 0.0), fz))
            var right = vec3(1.0, 0.0, 0.0)
            if f.y < 0.999 { right = normalize(cross(vec3(0.0, 1.0, 0.0), f)) }
            let up = cross(f, right)
            let cuv = clamp(vec2(dot(off, right) / r * (0.5 * #(crate::impostor::FILL_F)) + 0.5, 0.5 - dot(off, up) / r * (0.5 * #(crate::impostor::FILL_F))), vec2(0.0, 0.0), vec2(1.0, 1.0))
            self.v_uv = (g + cuv) / #(crate::impostor::GRID_F)
            let wp = cw + (rx * corner.x + ry * corner.y) * (r * s)
            let nx = dot(self.sun_rx.xyz, wp) + self.sun_rx.w
            let ny = dot(self.sun_ry.xyz, wp) + self.sun_ry.w
            let nz = dot(self.sun_rz.xyz, cw) + self.sun_rz.w
            self.v_d = nz
            self.v_clip = vec2(nx, ny)
            let zq = nz + self.flip_a.x * (1.0 - 2.0 * nz)
            self.vertex_pos = vec4(
                nx * self.tile_a.x + self.tile_a.z,
                ny * self.tile_a.y + self.tile_a.w,
                zq * (1.0 - self.flip_a.y) + self.flip_a.z,
                1.0
            )
        }

        pixel: fn() {
            if abs(self.v_clip.x) > 1.001 || abs(self.v_clip.y) > 1.001 {
                discard()
            }
            let a = self.tex.sample(self.v_uv).w
            if a < 0.5 {
                discard()
            }
            // Leaf texels (impostor.rs LEAF_ALPHA, below the core's 1.0) let
            // the sun through a third of the time, at a fixed per-texel hash:
            // a far crown's shadow is dappled like the near cascade's cards,
            // not a solid blob.
            if a < 0.9 {
                let tp = floor(self.v_uv * self.tex.size())
                if fract(sin(dot(tp, vec2(12.9898, 78.233))) * 43758.5453) < 0.34 {
                    discard()
                }
            }
            return vec4(self.v_d, 0.0, 0.0, 1.0)
        }
    }

    mod.draw.DrawLmLampDepth = mod.std.set_type_default() do #(DrawLmLampDepth::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        color_format: @Rf32
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.GameMeshVertexAo, geom.GameMeshAoGeom)
        v_view: varying(vec3f)

        vertex: fn() {
            var pos=vec3(self.geom.px,self.geom.py,self.geom.pz)
            if self.morph_ctl.w > 0.5 { pos=pos+self.morph_delta(self.geom.ao_uv,0.0) }
            let wp = self.transform * vec4(pos, 1.0)
            let vx = dot(self.face_rx.xyz, wp.xyz) + self.face_rx.w
            let vy = dot(self.face_ry.xyz, wp.xyz) + self.face_ry.w
            let vz = dot(self.face_rz.xyz, wp.xyz) + self.face_rz.w
            self.v_view = vec3(vx, vy, vz)
            let near = self.lamp_range.x
            let far = self.lamp_range.y
            // lamp_range.zw = the local atlas face's depth generation
            // (local_shadows.rs): clip z * (1 - z) + w * vz, identity at 0.
            self.vertex_pos = vec4(
                vx * self.tile_a.x + vz * self.tile_a.z,
                vy * self.tile_a.y + vz * self.tile_a.w,
                (vz - near) * far / max(far - near, 0.0001) * (1.0 - self.lamp_range.z) + self.lamp_range.w * vz,
                vz
            )
        }

        pixel: fn() {
            let vz = max(self.v_view.z, 0.0001)
            let nx = self.v_view.x / vz
            let ny = self.v_view.y / vz
            if abs(nx) > 0.999 {
                discard()
            }
            if abs(ny) > 0.999 {
                discard()
            }
            let d01 = (self.v_view.z - self.lamp_range.x)
                / max(self.lamp_range.y - self.lamp_range.x, 0.0001)
            return vec4(d01, 0.0, 0.0, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
        morph_map: texture_2d(float)
        // The weight of morph target k (0..31): morph_weights0..7, four each.
        morph_weight: fn(k: float) -> float {
            let i = floor(k / 4.0)
            var w = self.morph_weights0
            if i > 0.5 { w = self.morph_weights1 }
            if i > 1.5 { w = self.morph_weights2 }
            if i > 2.5 { w = self.morph_weights3 }
            if i > 3.5 { w = self.morph_weights4 }
            if i > 4.5 { w = self.morph_weights5 }
            if i > 5.5 { w = self.morph_weights6 }
            if i > 6.5 { w = self.morph_weights7 }
            let c = k - i * 4.0
            if c < 0.5 { return w.x }
            if c < 1.5 { return w.y }
            if c < 2.5 { return w.z }
            return w.w
        }
        // The first morph_ctl.w targets' deltas, weighted, in target order
        // (one loop: the body is compiled once, not per target).
        morph_delta: fn(vertex:float,lane:float)->vec3f {
            var delta=vec3(0.0,0.0,0.0)
            var k=0.0
            while k < 31.5 && self.morph_ctl.w > k + 0.5 {
                let index=(k*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weight(k)
                k=k+1.0
            }
            return delta
        }
    }

    // DrawLmLampDepth without the face clip, for backends that scissor each
    // draw to its face's tile (local_shadows.rs): nothing rasterizes past
    // the tile, so the pipeline needs no `discard` and keeps the GPU's
    // hidden-surface removal. A wall beside a lamp spans far past its face.
    mod.draw.DrawLmLampDepthInside = mod.std.set_type_default() do #(DrawLmLampDepthInside::script_shader(vm)){
        ..mod.draw.DrawLmLampDepth
        pixel: fn() {
            let d01 = (self.v_view.z - self.lamp_range.x)
                / max(self.lamp_range.y - self.lamp_range.x, 0.0001)
            return vec4(d01, 0.0, 0.0, 1.0)
        }
    }
}
