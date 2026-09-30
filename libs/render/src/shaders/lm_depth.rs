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
            // passes use the identity tile. flip_a.yz = the cascade tile's
            // depth generation (shadow_csm::depth_generation_window):
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
        morph_delta: fn(vertex:float,lane:float)->vec3f {
            var delta=vec3(0.0,0.0,0.0)
            if self.morph_ctl.w > 0.5 {
                let index=(0.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights0.x
            }
            if self.morph_ctl.w > 1.5 {
                let index=(1.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights0.y
            }
            if self.morph_ctl.w > 2.5 {
                let index=(2.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights0.z
            }
            if self.morph_ctl.w > 3.5 {
                let index=(3.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights0.w
            }
            if self.morph_ctl.w > 4.5 {
                let index=(4.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights1.x
            }
            if self.morph_ctl.w > 5.5 {
                let index=(5.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights1.y
            }
            if self.morph_ctl.w > 6.5 {
                let index=(6.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights1.z
            }
            if self.morph_ctl.w > 7.5 {
                let index=(7.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights1.w
            }
            if self.morph_ctl.w > 8.5 {
                let index=(8.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights2.x
            }
            if self.morph_ctl.w > 9.5 {
                let index=(9.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights2.y
            }
            if self.morph_ctl.w > 10.5 {
                let index=(10.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights2.z
            }
            if self.morph_ctl.w > 11.5 {
                let index=(11.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights2.w
            }
            if self.morph_ctl.w > 12.5 {
                let index=(12.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights3.x
            }
            if self.morph_ctl.w > 13.5 {
                let index=(13.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights3.y
            }
            if self.morph_ctl.w > 14.5 {
                let index=(14.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights3.z
            }
            if self.morph_ctl.w > 15.5 {
                let index=(15.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights3.w
            }
            if self.morph_ctl.w > 16.5 {
                let index=(16.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights4.x
            }
            if self.morph_ctl.w > 17.5 {
                let index=(17.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights4.y
            }
            if self.morph_ctl.w > 18.5 {
                let index=(18.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights4.z
            }
            if self.morph_ctl.w > 19.5 {
                let index=(19.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights4.w
            }
            if self.morph_ctl.w > 20.5 {
                let index=(20.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights5.x
            }
            if self.morph_ctl.w > 21.5 {
                let index=(21.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights5.y
            }
            if self.morph_ctl.w > 22.5 {
                let index=(22.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights5.z
            }
            if self.morph_ctl.w > 23.5 {
                let index=(23.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights5.w
            }
            if self.morph_ctl.w > 24.5 {
                let index=(24.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights6.x
            }
            if self.morph_ctl.w > 25.5 {
                let index=(25.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights6.y
            }
            if self.morph_ctl.w > 26.5 {
                let index=(26.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights6.z
            }
            if self.morph_ctl.w > 27.5 {
                let index=(27.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights6.w
            }
            if self.morph_ctl.w > 28.5 {
                let index=(28.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights7.x
            }
            if self.morph_ctl.w > 29.5 {
                let index=(29.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights7.y
            }
            if self.morph_ctl.w > 30.5 {
                let index=(30.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights7.z
            }
            if self.morph_ctl.w > 31.5 {
                let index=(31.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights7.w
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
        morph_delta: fn(vertex:float,lane:float)->vec3f {
            var delta=vec3(0.0,0.0,0.0)
            if self.morph_ctl.w > 0.5 {
                let index=(0.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights0.x
            }
            if self.morph_ctl.w > 1.5 {
                let index=(1.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights0.y
            }
            if self.morph_ctl.w > 2.5 {
                let index=(2.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights0.z
            }
            if self.morph_ctl.w > 3.5 {
                let index=(3.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights0.w
            }
            if self.morph_ctl.w > 4.5 {
                let index=(4.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights1.x
            }
            if self.morph_ctl.w > 5.5 {
                let index=(5.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights1.y
            }
            if self.morph_ctl.w > 6.5 {
                let index=(6.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights1.z
            }
            if self.morph_ctl.w > 7.5 {
                let index=(7.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights1.w
            }
            if self.morph_ctl.w > 8.5 {
                let index=(8.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights2.x
            }
            if self.morph_ctl.w > 9.5 {
                let index=(9.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights2.y
            }
            if self.morph_ctl.w > 10.5 {
                let index=(10.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights2.z
            }
            if self.morph_ctl.w > 11.5 {
                let index=(11.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights2.w
            }
            if self.morph_ctl.w > 12.5 {
                let index=(12.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights3.x
            }
            if self.morph_ctl.w > 13.5 {
                let index=(13.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights3.y
            }
            if self.morph_ctl.w > 14.5 {
                let index=(14.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights3.z
            }
            if self.morph_ctl.w > 15.5 {
                let index=(15.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights3.w
            }
            if self.morph_ctl.w > 16.5 {
                let index=(16.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights4.x
            }
            if self.morph_ctl.w > 17.5 {
                let index=(17.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights4.y
            }
            if self.morph_ctl.w > 18.5 {
                let index=(18.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights4.z
            }
            if self.morph_ctl.w > 19.5 {
                let index=(19.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights4.w
            }
            if self.morph_ctl.w > 20.5 {
                let index=(20.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights5.x
            }
            if self.morph_ctl.w > 21.5 {
                let index=(21.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights5.y
            }
            if self.morph_ctl.w > 22.5 {
                let index=(22.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights5.z
            }
            if self.morph_ctl.w > 23.5 {
                let index=(23.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights5.w
            }
            if self.morph_ctl.w > 24.5 {
                let index=(24.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights6.x
            }
            if self.morph_ctl.w > 25.5 {
                let index=(25.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights6.y
            }
            if self.morph_ctl.w > 26.5 {
                let index=(26.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights6.z
            }
            if self.morph_ctl.w > 27.5 {
                let index=(27.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights6.w
            }
            if self.morph_ctl.w > 28.5 {
                let index=(28.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights7.x
            }
            if self.morph_ctl.w > 29.5 {
                let index=(29.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights7.y
            }
            if self.morph_ctl.w > 30.5 {
                let index=(30.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights7.z
            }
            if self.morph_ctl.w > 31.5 {
                let index=(31.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights7.w
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
            if self.tex.sample_as_bgra_repeat(self.v_uv).w < 0.5 {
                discard()
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
        morph_delta: fn(vertex:float,lane:float)->vec3f {
            var delta=vec3(0.0,0.0,0.0)
            if self.morph_ctl.w > 0.5 {
                let index=(0.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights0.x
            }
            if self.morph_ctl.w > 1.5 {
                let index=(1.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights0.y
            }
            if self.morph_ctl.w > 2.5 {
                let index=(2.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights0.z
            }
            if self.morph_ctl.w > 3.5 {
                let index=(3.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights0.w
            }
            if self.morph_ctl.w > 4.5 {
                let index=(4.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights1.x
            }
            if self.morph_ctl.w > 5.5 {
                let index=(5.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights1.y
            }
            if self.morph_ctl.w > 6.5 {
                let index=(6.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights1.z
            }
            if self.morph_ctl.w > 7.5 {
                let index=(7.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights1.w
            }
            if self.morph_ctl.w > 8.5 {
                let index=(8.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights2.x
            }
            if self.morph_ctl.w > 9.5 {
                let index=(9.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights2.y
            }
            if self.morph_ctl.w > 10.5 {
                let index=(10.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights2.z
            }
            if self.morph_ctl.w > 11.5 {
                let index=(11.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights2.w
            }
            if self.morph_ctl.w > 12.5 {
                let index=(12.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights3.x
            }
            if self.morph_ctl.w > 13.5 {
                let index=(13.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights3.y
            }
            if self.morph_ctl.w > 14.5 {
                let index=(14.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights3.z
            }
            if self.morph_ctl.w > 15.5 {
                let index=(15.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights3.w
            }
            if self.morph_ctl.w > 16.5 {
                let index=(16.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights4.x
            }
            if self.morph_ctl.w > 17.5 {
                let index=(17.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights4.y
            }
            if self.morph_ctl.w > 18.5 {
                let index=(18.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights4.z
            }
            if self.morph_ctl.w > 19.5 {
                let index=(19.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights4.w
            }
            if self.morph_ctl.w > 20.5 {
                let index=(20.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights5.x
            }
            if self.morph_ctl.w > 21.5 {
                let index=(21.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights5.y
            }
            if self.morph_ctl.w > 22.5 {
                let index=(22.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights5.z
            }
            if self.morph_ctl.w > 23.5 {
                let index=(23.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights5.w
            }
            if self.morph_ctl.w > 24.5 {
                let index=(24.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights6.x
            }
            if self.morph_ctl.w > 25.5 {
                let index=(25.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights6.y
            }
            if self.morph_ctl.w > 26.5 {
                let index=(26.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights6.z
            }
            if self.morph_ctl.w > 27.5 {
                let index=(27.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights6.w
            }
            if self.morph_ctl.w > 28.5 {
                let index=(28.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights7.x
            }
            if self.morph_ctl.w > 29.5 {
                let index=(29.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights7.y
            }
            if self.morph_ctl.w > 30.5 {
                let index=(30.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights7.z
            }
            if self.morph_ctl.w > 31.5 {
                let index=(31.0*self.morph_ctl.z+vertex)*2.0+lane
                let uv=vec2((modf(index,self.morph_ctl.x)+0.5)/self.morph_ctl.x,(floor(index/self.morph_ctl.x)+0.5)/self.morph_ctl.y)
                delta=delta+self.morph_map.sample_nearest(uv, 0.0).xyz*self.morph_weights7.w
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
