//! DrawSceneImpostor: a far tree as one camera-facing quad that shows the
//! three baked views (impostor.rs) nearest to the camera's direction.

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    // The foliage lane's sibling for a model's stand-in layer: the same
    // binding, instance stream and uniforms (the lane switches only the
    // shader id for that layer, renderer/draw_models.rs), its own vertex
    // stage and pixel.
    //
    // The quad (impostor.rs) is centre + corner x radius in the model's XY
    // plane with uv = corner x radius, so the vertex stage recovers both.
    // It turns the quad toward the camera, finds the camera's direction in
    // the copy's own frame (yawed, leaned and scaled copies included),
    // encodes it hemi-octahedrally onto the impostor.rs GRID x GRID view grid and picks the
    // grid triangle around it: three views and their barycentric weights.
    // For each view it projects the quad's corners onto that view's image
    // plane, which is affine across the quad, so the per-view uvs
    // interpolate exactly. The pixel reads the three albedo, normal and
    // depth texels, blends them, and lights the copy with the foliage lane's
    // own terms (shaders/foliage.rs), so a tree keeps its brightness when it
    // switches: wrapped sun, light through the leaves toward the sun (a
    // crown's tinted, `crown` set from the stand-in material), the
    // hemisphere fill, the baked occlusion on the fill, the light through the
    // leaves and half the sun, the copy's own colour adjust, the sun's
    // cascades, fog. A far forest then turns with the camera and follows the
    // time of day.
    //
    // Depth: the bake's depth atlas rides in the OCCLUSION slot (impostor.rs
    // `ImpostorBake::depth`), the surface's occlusion in its blue.
    // The quad rasterises at the front of the bounding sphere and the pixel
    // pushes its depth back to the baked surface (a conservative `@greater`
    // write: the early depth test still rejects a stand-in behind a hill),
    // so a far crown meets terrain, walls and other crowns where its leaves
    // are, and takes the cascades' shadow there. Its own stand-in's shadow
    // it never takes (csm_own, draw_models.rs), as the mesh's layers never do.
    mod.draw.DrawSceneImpostor = mod.std.set_type_default() do #(DrawSceneImpostor::script_shader(vm)){
        ..mod.draw.DrawSceneFoliageLit

        // In-cell uvs of views 0 and 1, of view 2 plus weights 0 and 1, and
        // the three views' cells (i + GRID j).
        v_imp_a: varying(vec4f)
        v_imp_b: varying(vec4f)
        v_imp_c: varying(vec4f)
        depth_out: mod.shader.fragment_depth(@greater)

        // The direction toward the viewer of grid point g (impostor.rs
        // `frame_dir`).
        imp_frame_dir: fn(g: vec2) -> vec3 {
            let uv = g * (2.0 / #(crate::impostor::GRID_LAST_F)) - vec2(1.0, 1.0)
            let x = (uv.x + uv.y) * 0.5
            let z = (uv.x - uv.y) * 0.5
            return normalize(vec3(x, max(1.0 - abs(x) - abs(z), 0.0), z))
        }

        // The in-cell uv of a model-space offset from the centre in view g's
        // image (impostor.rs `frame_basis`, cells top-down).
        imp_cell_uv: fn(g: vec2, off: vec3, r: float) -> vec2 {
            let f = self.imp_frame_dir(g)
            var right = vec3(1.0, 0.0, 0.0)
            if f.y < 0.999 { right = normalize(cross(vec3(0.0, 1.0, 0.0), f)) }
            let up = cross(f, right)
            return vec2(dot(off, right) / r * (0.5 * #(crate::impostor::FILL_F)) + 0.5, 0.5 - dot(off, up) / r * (0.5 * #(crate::impostor::FILL_F)))
        }

        vertex: fn() {
            let corner_r = unpack2f16(self.geom.uv)
            let r = max(abs(corner_r.x), 0.001)
            let corner = vec2(corner_r.x / r, corner_r.y / r)
            let center = vec3(self.geom.px - corner_r.x, self.geom.py - corner_r.y, self.geom.pz)
            let m = self.draw_list.view_transform * self.transform
            let ax = (m * vec4(1.0, 0.0, 0.0, 0.0)).xyz
            let ay = (m * vec4(0.0, 1.0, 0.0, 0.0)).xyz
            let az = (m * vec4(0.0, 0.0, 1.0, 0.0)).xyz
            let s2 = max(dot(ax, ax), 0.000001)
            let cw = (m * vec4(center.x, center.y, center.z, 1.0)).xyz
            let eye = (self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)).xyz
            // unit axes: a scaled view (an XR box's stage rides in it) must not size the quad
            let cam_r = normalize((self.draw_pass.camera_inv * vec4(1.0, 0.0, 0.0, 0.0)).xyz)
            let cam_u = normalize((self.draw_pass.camera_inv * vec4(0.0, 1.0, 0.0, 0.0)).xyz)
            // World directions into the copy's frame (uniform scale).
            let to_eye = eye - cw
            let vd = normalize(vec3(dot(ax, to_eye), dot(ay, to_eye), dot(az, to_eye)))
            let rl = normalize(vec3(dot(ax, cam_r), dot(ay, cam_r), dot(az, cam_r)))
            let ul = normalize(vec3(dot(ax, cam_u), dot(ay, cam_u), dot(az, cam_u)))
            let off = (rl * corner.x + ul * corner.y) * r
            // Views from below the horizon reuse the rim (the bake covers the
            // upper hemisphere only).
            let hv = normalize(vec3(vd.x, max(vd.y, 0.0), vd.z))
            let hs = abs(hv.x) + abs(hv.y) + abs(hv.z)
            let ox = hv.x / hs
            let oz = hv.z / hs
            let g = clamp((vec2(ox + oz, ox - oz) * 0.5 + vec2(0.5, 0.5)) * #(crate::impostor::GRID_LAST_F), vec2(0.0, 0.0), vec2(#(crate::impostor::GRID_LAST_F), #(crate::impostor::GRID_LAST_F)))
            let gi = min(floor(g), vec2(#(crate::impostor::GRID_LAST_F) - 1.0, #(crate::impostor::GRID_LAST_F) - 1.0))
            let fr = g - gi
            var g0 = gi
            var g1 = gi + vec2(1.0, 0.0)
            var g2 = gi + vec2(0.0, 1.0)
            var w0 = 1.0 - fr.x - fr.y
            var w1 = fr.x
            if fr.x + fr.y > 1.0 {
                g0 = gi + vec2(1.0, 1.0)
                g1 = gi + vec2(0.0, 1.0)
                g2 = gi + vec2(1.0, 0.0)
                w0 = fr.x + fr.y - 1.0
                w1 = 1.0 - fr.x
            }
            let uv0 = self.imp_cell_uv(g0, off, r)
            let uv1 = self.imp_cell_uv(g1, off, r)
            let uv2 = self.imp_cell_uv(g2, off, r)
            self.v_imp_a = vec4(uv0.x, uv0.y, uv1.x, uv1.y)
            self.v_imp_b = vec4(uv2.x, uv2.y, w0, w1)
            let s = sqrt(s2)
            self.v_imp_c = vec4(g0.x + g0.y * #(crate::impostor::GRID_F), g1.x + g1.y * #(crate::impostor::GRID_F), g2.x + g2.y * #(crate::impostor::GRID_F), r * s)
            // The quad faces the camera through the crown's centre; world
            // position of this corner from the camera's own axes.
            let wp = cw + (cam_r * corner.x + cam_u * corner.y) * (r * s)
            self.world = vec4(wp.x, wp.y, wp.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            let l = normalize(self.light_dir)
            self.v_csm = vec4(wp.x, wp.y, wp.z, 0.0)
            self.v_csm_n = normalize(to_eye)
            self.v_csm_own = dot(self.csm_rz0.xyz, cw - wp) / max(self.csm_zw.x, 0.000001)
            self.v_tint = unpack4u8(self.geom.color)
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            let clip_out = self.draw_pass.camera_projection * view_pos
            self.v_spos = clip_out
            // Raster depth at the front of the bounding sphere along this
            // corner's own ray (x, y, w unchanged): pushed by r (1 + r / d),
            // the plane through the four corners lies in front of the sphere,
            // so every baked depth the pixel writes is at or behind it.
            let dist = max(length(view_pos.xyz), 0.0001)
            let rs = r * s
            let push = min(rs * (1.0 + rs / dist), dist * 0.9)
            let front = self.draw_pass.camera_projection * vec4(view_pos.xyz * (1.0 - push / dist), 1.0)
            self.vertex_pos = vec4(clip_out.x, clip_out.y, front.z / front.w * clip_out.w, clip_out.w)
        }

        // One view's texels: albedo (alpha = coverage) and the decoded
        // model-space normal; outside the view's cell nothing is covered.
        imp_albedo: fn(uv: vec2, cell: float) -> vec4 {
            let inside = step(0.0, uv.x) * step(uv.x, 1.0) * step(0.0, uv.y) * step(uv.y, 1.0)
            let cy = floor((cell + 0.5) / #(crate::impostor::GRID_F))
            let cx = floor(cell + 0.5 - cy * #(crate::impostor::GRID_F))
            let a = self.tex.sample((vec2(cx, cy) + clamp(uv, vec2(0.004, 0.004), vec2(0.996, 0.996))) / #(crate::impostor::GRID_F))
            return vec4(a.x, a.y, a.z, a.w * inside)
        }
        // One view's baked depth toward its viewer, -1..1 of the radius
        // (16 bits over red and green), and the surface's occlusion (blue;
        // impostor.rs `ImpostorBake::depth`).
        imp_depth_ao: fn(uv: vec2, cell: float) -> vec2 {
            let cy = floor((cell + 0.5) / #(crate::impostor::GRID_F))
            let cx = floor(cell + 0.5 - cy * #(crate::impostor::GRID_F))
            let q = self.occlusion_map.sample((vec2(cx, cy) + clamp(uv, vec2(0.004, 0.004), vec2(0.996, 0.996))) / #(crate::impostor::GRID_F))
            return vec2((q.x * (65280.0 / 65535.0) + q.y * (255.0 / 65535.0)) * 2.0 - 1.0, q.z)
        }
        imp_normal: fn(uv: vec2, cell: float) -> vec3 {
            let cy = floor((cell + 0.5) / #(crate::impostor::GRID_F))
            let cx = floor(cell + 0.5 - cy * #(crate::impostor::GRID_F))
            let e = self.normal_map.sample((vec2(cx, cy) + clamp(uv, vec2(0.004, 0.004), vec2(0.996, 0.996))) / #(crate::impostor::GRID_F)).xy * 2.0 - vec2(1.0, 1.0)
            // The view's own frame (impostor.rs view_encode): x right, y up,
            // z toward that view's viewer, rebuilt.
            let f = self.imp_frame_dir(vec2(cx, cy))
            var right = vec3(1.0, 0.0, 0.0)
            if f.y < 0.999 { right = normalize(cross(vec3(0.0, 1.0, 0.0), f)) }
            let up = cross(f, right)
            return right * e.x + up * e.y + f * sqrt(max(1.0 - dot(e, e), 0.0))
        }

        pixel: fn() {
            // The mesh <-> stand-in crossfade (draw_models.rs).
            self.door_window()
            let w0 = self.v_imp_b.z
            let w1 = self.v_imp_b.w
            let w2 = max(1.0 - w0 - w1, 0.0)
            let a0 = self.imp_albedo(self.v_imp_a.xy, self.v_imp_c.x)
            let a1 = self.imp_albedo(self.v_imp_a.zw, self.v_imp_c.y)
            let a2 = self.imp_albedo(self.v_imp_b.xy, self.v_imp_c.z)
            let cover = a0.w * w0 + a1.w * w1 + a2.w * w2
            let cut = self.cut_coverage(cover, 0.5)
            if cut <= 0.0 { self.clip() }
            // Colour and normal from the views that cover this pixel.
            let k0 = w0 * a0.w
            let k1 = w1 * a1.w
            let k2 = w2 * a2.w
            let ks = max(k0 + k1 + k2, 0.0001)
            let alb = (a0.xyz * k0 + a1.xyz * k1 + a2.xyz * k2) / ks
            let nl = self.imp_normal(self.v_imp_a.xy, self.v_imp_c.x) * k0
                + self.imp_normal(self.v_imp_a.zw, self.v_imp_c.y) * k1
                + self.imp_normal(self.v_imp_b.xy, self.v_imp_c.z) * k2
            // Model space to world through the copy's axes.
            let m = self.draw_list.view_transform * self.transform
            var n = normalize((m * vec4(nl.x, nl.y, nl.z, 0.0)).xyz)
            let wq = self.v_csm.xyz
            let v = normalize(self.eye.xyz - wq)
            // The baked surface on this pixel's ray (the views' depths with
            // the colour's weights), its depth written at or behind the
            // quad's front raster depth, and its occlusion.
            let da = (self.imp_depth_ao(self.v_imp_a.xy, self.v_imp_c.x) * k0
                + self.imp_depth_ao(self.v_imp_a.zw, self.v_imp_c.y) * k1
                + self.imp_depth_ao(self.v_imp_b.xy, self.v_imp_c.z) * k2) / ks
            let ao = clamp(da.y, 0.0, 1.0)
            // Leaf texels (alpha impostor.rs LEAF_ALPHA) let the sun through;
            // the trunk and the solid core (255) do not, as in the mesh's lane.
            let leaf = (k0 * step(a0.w, 0.9) + k1 * step(a1.w, 0.9) + k2 * step(a2.w, 0.9)) / ks
            let wp = wq + v * (da.x * self.v_imp_c.w)
            let clip = self.draw_pass.camera_projection * (self.draw_pass.camera_view * vec4(wp.x, wp.y, wp.z, 1.0))
            self.depth_out = max(self.depth_out, clip.z / clip.w)
            let l = normalize(self.light_dir)
            let ndl = dot(n, l)
            // The copy's own tint and adjust, as its mesh draws them.
            let albedo = self.color_adjust(self.to_scene(alb) * self.to_lin(self.v_tint.xyz), self.tint, self.color_adjust_ctl)
            var sao = 1.0
            if self.ssao_ctl.x > 0.001 {
                let sp = self.v_spos.xy / max(self.v_spos.w, 0.000001)
                sao = 1.0 - (1.0 - self.ssao_map.sample_nearest(vec2(sp.x * 0.5 + 0.5, 0.5 - sp.y * 0.5)).x) * self.ssao_ctl.x
            }
            // The foliage lane's terms (shaders/foliage.rs).
            let wrap = clamp(ndl * 0.7 + 0.3, 0.0, 1.0)
            let back = max(dot(v * (0.0 - 1.0), l), 0.0)
            var through = vec3(1.0, 1.0, 1.0) * (back * back * back * 0.5 * leaf)
            if self.crown > 0.5 {
                through = vec3(1.1, 1.2, 0.45) * (back * back * back * 0.8 * ao * leaf)
            }
            let ambient = mix(self.sun_ground, self.sun_sky, clamp(n.y * 0.5 + 0.5, 0.0, 1.0)) * (ao * sao)
            let vis = self.csm_vis_fast(wp, n, max(ndl, 0.0))
            let lit = albedo * (ambient + self.sun_color * ((vec3(wrap, wrap, wrap) + through) * vis * mix(1.0, ao, 0.5)))
            return vec4(self.scene_fogged(self.to_display(lit), self.v_fog, wp, self.fog_density), self.cut_alpha(cut))
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }
}
