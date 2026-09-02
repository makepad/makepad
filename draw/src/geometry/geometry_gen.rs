use crate::makepad_platform::*;

#[derive(Clone, Script, ScriptHook)]
pub struct QuadVertex {
    #[live]
    pub pos: Vec2f,
}

/// One vertex of a closed frame ring: a corner of the unit square, plus which
/// side of the strip it belongs to (0 = outer edge, 1 = inner edge).
///
/// Used by the exploded view's container outlines. They are strips, not quads:
/// a drawless container must submit no full-plane geometry at all — both
/// because a plane covered in alpha reads as fog over the layers behind it,
/// and because the mode doubles as an overdraw instrument, where a covered
/// pixel has to mean the app painted it.
#[derive(Clone, Script, ScriptHook)]
pub struct OutlineVertex {
    #[live]
    pub pos: Vec2f,
    #[live]
    pub inner: f32,
    /// std140 wants a 16-byte stride; the generator writes this slot too.
    #[live]
    pub pad: f32,
}

#[derive(Clone, Script, ScriptHook)]
pub struct VectorVertex {
    #[live]
    pub x: f32,
    #[live]
    pub y: f32,
    #[live]
    pub u: f32,
    #[live]
    pub v: f32,
    #[live]
    pub color_r: f32,
    #[live]
    pub color_g: f32,
    #[live]
    pub color_b: f32,
    #[live]
    pub color_a: f32,
    #[live]
    pub stroke_mult: f32,
    #[live]
    pub stroke_dist: f32,
    #[live]
    pub shape_id: f32,
    // dual-purpose params: shadow (stroke_mult == -1) or gradient (param0 > 0)
    #[live]
    pub param0: f32,
    #[live]
    pub param1: f32,
    #[live]
    pub param2: f32,
    #[live]
    pub param3: f32,
    #[live]
    pub param4: f32,
    #[live]
    pub param5: f32,
    // max distance from this vertex to any other vertex it shares a triangle with
    #[live]
    pub clip_radius: f32,
    // per-shape z-bias for painter's algorithm ordering
    #[live]
    pub zbias: f32,
}

/// Packed VectorVertex: 12 f32 slots carrying the 19 logical fields —
/// f16 pairs / unorm8x4 bitcast into single slots, unpacked in the vertex
/// shader (unpack2f16/unpack4u8). Halves vertex fetch bandwidth; the
/// precision-critical slots (positions, stroke_mult sentinels, param4
/// icon-floor composite, param5 depth ladder, zbias 1e-6 steps) stay f32.
#[derive(Clone, Script, ScriptHook)]
pub struct VectorVertexPacked {
    #[live]
    pub x: f32,
    #[live]
    pub y: f32,
    /// f16(u) | f16(v)
    #[live]
    pub uv: f32,
    /// unorm8 r|g|b|a
    #[live]
    pub color: f32,
    #[live]
    pub stroke_mult: f32,
    #[live]
    pub stroke_dist: f32,
    /// f16(param0) | f16(shape_id)
    #[live]
    pub p0s: f32,
    /// f16(param1) | f16(param2)
    #[live]
    pub p12: f32,
    /// f16(param3) | f16(clip_radius, clamped)
    #[live]
    pub p3c: f32,
    #[live]
    pub param4: f32,
    #[live]
    pub param5: f32,
    #[live]
    pub zbias: f32,
}

/// Map polygon fills need only their tile-local position, premultiplied
/// colour, fill/material variant, AA coverage and the two depth ladders.
/// The final slot bit-packs those ladders; see `pack_fill_record`.
#[derive(Clone, Script, ScriptHook)]
#[repr(C)]
pub struct FillVertexPacked {
    #[live]
    pub x: f32,
    #[live]
    pub y: f32,
    /// unorm8 r|g|b|a
    #[live]
    pub color: f32,
    /// f16(shape/material code) | f16(fill AA coverage)
    #[live]
    pub params: f32,
    /// u16 flat zbias ticks | u16 tilted param5 ticks
    #[live]
    pub zbias: f32,
}

/// Instanced symbol mesh vertex (map POI icons): screen-px offset from the
/// instance anchor, f16 uv pair, stroke distance. 16 bytes; the anchor,
/// colour, zoom floor and depth ride the instance instead of every vertex.
#[derive(Clone, Script, ScriptHook)]
pub struct IconVertexPacked {
    #[live]
    pub x: f32,
    #[live]
    pub y: f32,
    /// f16(u) | f16(v)
    #[live]
    pub uv: f32,
    #[live]
    pub stroke_dist: f32,
}

#[derive(Clone, Script, ScriptHook)]
pub struct PbrVertex {
    #[live]
    pub pos_nx: Vec4f, // xyz + nx
    #[live]
    pub ny_nz_uv: Vec4f, // ny nz u v
    #[live]
    pub color: Vec4f, // rgba
    #[live]
    pub tangent: Vec4f, // tangent xyz + handedness
}

/// Packed mesh vertex: 6 f32 slots instead of PbrVertex's 16, for streams
/// where fetch bandwidth matters more than the last bit of precision —
/// CPU-skinned characters (re-uploaded every frame), terrain, shadow meshes.
///
/// Same idea as [`VectorVertexPacked`]: position stays f32 because it is
/// precision-critical, everything else is an f16 pair or a unorm8 quad
/// bitcast into one slot and unpacked in the vertex shader
/// (`unpack2f16` / `unpack4u8`). Normals are octahedral-encoded, which is
/// what lets a 3-component unit vector fit in two f16 lanes.
#[derive(Clone, Script, ScriptHook)]
pub struct GameMeshVertex {
    // Flat f32 fields, not a Vec3f: std140 pads a vec3 to 16 bytes, which
    // would not match the Rust repr(C) size. VectorVertexPacked does the
    // same for the same reason.
    #[live]
    pub px: f32,
    #[live]
    pub py: f32,
    #[live]
    pub pz: f32,
    /// f16(oct.x) | f16(oct.y) — octahedral unit normal.
    #[live]
    pub nrm: f32,
    /// f16(u) | f16(v)
    #[live]
    pub uv: f32,
    /// unorm8 r|g|b|a
    #[live]
    pub color: f32,
}

/// [`GameMeshVertex`] plus a second uv, into a baked ambient-occlusion atlas.
///
/// A separate type rather than a seventh lane on GameMeshVertex, because the
/// shadow mesh shares that layout and has no use for an AO coordinate — it
/// would pay four bytes a vertex for nothing. Props opt in; shadows do not.
#[derive(Clone, Script, ScriptHook)]
pub struct GameMeshVertexAo {
    #[live]
    pub px: f32,
    #[live]
    pub py: f32,
    #[live]
    pub pz: f32,
    /// f16(oct.x) | f16(oct.y) — octahedral unit normal.
    #[live]
    pub nrm: f32,
    /// f16(u) | f16(v) — colormap atlas.
    #[live]
    pub uv: f32,
    /// unorm8 r|g|b|a
    #[live]
    pub color: f32,
    /// f16(u) | f16(v) — AO atlas. Per FRAGMENT occlusion: sampling this per
    /// vertex instead would carry exactly as much information as a vertex
    /// bake, which is the thing the atlas exists to escape.
    #[live]
    pub ao_uv: f32,
}

/// [`GameMeshVertex`] plus skinning influences, for GPU-skinned characters:
/// the REST mesh uploads once and the vertex shader blends it against a
/// joint-matrix texture, so the per-frame cost is a joint palette instead of
/// a full posed vertex stream.
///
/// No colour lane — character rigs carry their colour in the atlas and the
/// per-instance tint — and no AO lane, because a deforming mesh cannot carry
/// a baked occlusion atlas (see GameMeshVertexAo).
#[derive(Clone, Script, ScriptHook)]
pub struct GameMeshVertexSkin {
    #[live]
    pub px: f32,
    #[live]
    pub py: f32,
    #[live]
    pub pz: f32,
    /// f16(oct.x) | f16(oct.y) — octahedral unit normal, rest pose.
    #[live]
    pub nrm: f32,
    /// f16(u) | f16(v) — colormap atlas.
    #[live]
    pub uv: f32,
    /// unorm8x4 — four joint indices. u8 addresses 256 joints; the rigs in
    /// play carry 7-41.
    #[live]
    pub joints: f32,
    /// unorm8x4 — four blend weights, normalized on load.
    #[live]
    pub weights: f32,
    /// unorm16x2 — uv into the rig's rest-pose AO chart atlas (same packing
    /// as GameMeshVertexAo.ao_uv: f16 spacing near 1.0 is a whole texel).
    /// Baked once per rig on the rest mesh; the occlusion rides the skinned
    /// surface through every pose because topology never changes.
    #[live]
    pub ao_uv: f32,
}

#[derive(Clone, Script, ScriptHook)]
pub struct CubeVertex {
    #[live]
    pub geom_pos: Vec3f,
    #[live]
    pub geom_id: f32,
    #[live]
    pub geom_normal: Vec3f,
    #[live]
    pub geom_pad: f32,
    #[live]
    pub geom_uv: Vec2f,
    #[live]
    pub geom_tail_pad_0: f32,
    #[live]
    pub geom_tail_pad_1: f32,
}

#[derive(Clone, Script, ScriptHook)]
pub struct IcoVertex {
    #[live]
    pub pos: Vec4f,
    #[live]
    pub normal: Vec4f,
}

#[derive(Clone, Script, ScriptHook)]
pub struct DepthMeshVertex {
    #[live]
    pub pos: Vec4f,
    #[live]
    pub barycentric: Vec4f,
}

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    // Each of these standard shader geometries is a single Cx-owned slot shared by
    // every VM. A VM (main or Splash isolate) only gets a *non-owning* handle to it,
    // so tearing an isolate down never frees the slot and the Cx-global shader cache
    // (which bakes in the geometry id) can't be left pointing at a reclaimed slot.
    fn shared(vm: &mut ScriptVm, key: LiveId, make: impl FnOnce() -> GeometryGen) -> ScriptValue {
        let id = vm
            .cx_mut()
            .shared_geometry(key, |cx| make().into_geometry(cx));
        Geometry::new_borrowed(id).into_script_handle(vm)
    }

    let geom = vm.new_module(id!(geom));
    // lets make a Quad geometry here
    set_script_value_to_pod!(vm, geom.QuadVertex);
    // now lets also build a quad vertexbuffer
    let gen = shared(vm, id!(QuadGeom), || GeometryGen::from_quad_2d(0., 0., 1., 1.));
    set_script_value!(vm, geom.QuadGeom = gen);
    // Frame-ring strip for the exploded view's container outlines.
    set_script_value_to_pod!(vm, geom.OutlineVertex);
    let ogen = shared(vm, id!(OutlineGeom), GeometryGen::from_outline_ring);
    set_script_value!(vm, geom.OutlineGeom = ogen);
    // Vector geometry: vertex type + placeholder geom (overridden at draw time)
    set_script_value_to_pod!(vm, geom.VectorVertex);
    let vgen = shared(vm, id!(VectorGeom), GeometryGen::from_triangle_2d);
    set_script_value!(vm, geom.VectorGeom = vgen);
    set_script_value_to_pod!(vm, geom.VectorVertexPacked);
    let vpgen = shared(vm, id!(VectorGeomPacked), GeometryGen::from_triangle_2d_packed);
    set_script_value!(vm, geom.VectorGeomPacked = vpgen);
    set_script_value_to_pod!(vm, geom.FillVertexPacked);
    let fpgen = shared(vm, id!(FillGeomPacked), GeometryGen::from_triangle_2d_fill_packed);
    set_script_value!(vm, geom.FillGeomPacked = fpgen);
    // Instanced icon meshes: vertex type + placeholder geom (the mesh is bound at draw time)
    set_script_value_to_pod!(vm, geom.IconVertexPacked);
    let ipgen = shared(vm, id!(IconGeomPacked), GeometryGen::from_triangle_2d_icon_packed);
    set_script_value!(vm, geom.IconGeomPacked = ipgen);
    // PBR geometry: vertex type + placeholder geom (overridden at draw time)
    set_script_value_to_pod!(vm, geom.PbrVertex);
    let pgen = shared(vm, id!(PbrGeom), GeometryGen::from_triangle_pbr);
    set_script_value!(vm, geom.PbrGeom = pgen);
    // Packed game mesh geometry: 6-slot vertex for bandwidth-bound streams.
    set_script_value_to_pod!(vm, geom.GameMeshVertex);
    let gmgen = shared(vm, id!(GameMeshGeom), GeometryGen::from_triangle_game_mesh);
    set_script_value!(vm, geom.GameMeshGeom = gmgen);
    // Packed game mesh + AO atlas uv, for props that carry baked occlusion.
    set_script_value_to_pod!(vm, geom.GameMeshVertexAo);
    let gmaogen = shared(vm, id!(GameMeshAoGeom), GeometryGen::from_triangle_game_mesh_ao);
    set_script_value!(vm, geom.GameMeshAoGeom = gmaogen);
    // GPU-skinned mesh: packed rest vertices + joint indices/weights.
    set_script_value_to_pod!(vm, geom.GameMeshVertexSkin);
    let gmskin = shared(vm, id!(GameMeshSkinGeom), GeometryGen::from_triangle_game_mesh_skin);
    set_script_value!(vm, geom.GameMeshSkinGeom = gmskin);
    // Cube geometry: unit cube in the old geom_pos/geom_normal/geom_uv layout.
    set_script_value_to_pod!(vm, geom.CubeVertex);
    let cgen = shared(vm, id!(CubeGeom), || {
        GeometryGen::from_cube_3d(1.0, 1.0, 1.0, 1, 1, 1).into_cube_vertex_pod()
    });
    set_script_value!(vm, geom.CubeGeom = cgen);
    // Ico geometry: minimal pos/normal layout for faceted solids.
    set_script_value_to_pod!(vm, geom.IcoVertex);
    let igen = shared(vm, id!(IcoGeom), GeometryGen::from_triangle_ico);
    set_script_value!(vm, geom.IcoGeom = igen);
    // Depth mesh geometry: position plus per-triangle barycentrics.
    set_script_value_to_pod!(vm, geom.DepthMeshVertex);
    let dgen = shared(vm, id!(DepthMeshGeom), GeometryGen::from_triangle_depth_mesh);
    set_script_value!(vm, geom.DepthMeshGeom = dgen);
    NIL
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GeometryGen {
    pub vertices: Vec<f32>, // vec4 pos, vec3 normal, vec2 uv
    pub indices: Vec<u32>,
}

#[derive(Clone, Copy)]
pub enum GeometryAxis {
    X = 0,
    Y = 1,
    Z = 2,
}

impl GeometryGen {
    pub fn update_geometry(self, cx: &mut Cx, geometry: &Geometry) {
        geometry.update(cx, self.indices, self.vertices);
    }

    pub fn into_geometry(self, cx: &mut Cx) -> Geometry {
        let g = Geometry::new(cx);
        g.update(cx, self.indices, self.vertices);
        g
    }

    /// Placeholder single-triangle geometry for vector drawing (overridden at draw time)
    pub fn from_triangle_2d_packed() -> GeometryGen {
        let mut g = Self::default();
        for _ in 0..3 {
            g.vertices.extend_from_slice(&crate::vector::pack_vector_record(&[
                0.0, 0.0, 0.5, 1.0,
                1.0, 1.0, 1.0, 1.0,
                1e6, 0.0, 0.0,
                0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
                0.0,
                0.0,
            ]));
        }
        g.indices.extend_from_slice(&[0, 1, 2]);
        g
    }

    pub fn from_triangle_2d_icon_packed() -> GeometryGen {
        let mut g = Self::default();
        for _ in 0..3 {
            g.vertices.extend_from_slice(&[0.0, 0.0, crate::vector::pack_pair_f16(0.5, 1.0), 0.0]);
        }
        g.indices.extend_from_slice(&[0, 1, 2]);
        g
    }

    pub fn from_triangle_2d_fill_packed() -> GeometryGen {
        let mut g = Self::default();
        let logical = [
            0.0, 0.0, 0.5, 1.0,
            1.0, 1.0, 1.0, 1.0,
            1e6, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            0.0,
            0.0,
        ];
        for _ in 0..3 {
            g.vertices.extend_from_slice(
                &crate::vector::pack_fill_record(&logical).expect("placeholder is a map fill"),
            );
        }
        g.indices.extend_from_slice(&[0, 1, 2]);
        g
    }

    pub fn from_triangle_2d() -> GeometryGen {
        let mut g = Self::default();
        // 3 vertices with full VectorVertex stride (23 floats each)
        for _ in 0..3 {
            g.vertices.extend_from_slice(&[
                0.0, 0.0, 0.5, 1.0, // x, y, u, v
                1.0, 1.0, 1.0, 1.0, // color r,g,b,a
                1e6, 0.0, 0.0, // stroke_mult, stroke_dist, shape_id
                0.0, 0.0, 0.0, 0.0, 0.0, 0.0, // param0-5
                0.0, // clip_radius
                0.0, // zbias
            ]);
        }
        g.indices.extend_from_slice(&[0, 1, 2]);
        g
    }

    pub fn from_quad_2d(x1: f32, y1: f32, x2: f32, y2: f32) -> GeometryGen {
        let mut g = Self::default();
        g.add_quad_2d(x1, y1, x2, y2);
        g
    }

    /// A closed frame ring as a triangle strip: four outer corners of the unit
    /// square paired with four inner ones, eight triangles round the loop.
    ///
    /// Vertex layout is `OutlineVertex` — `(corner.x, corner.y, inner)`. The
    /// shader places the outer ring on the rect's border and pushes the inner
    /// ring in by the stroke width, so the only geometry submitted is the
    /// frame itself; the middle of the container is never rasterized.
    pub fn from_outline_ring() -> GeometryGen {
        let mut g = Self::default();
        let corners = [(0.0f32, 0.0f32), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        for (x, y) in corners {
            g.vertices.extend_from_slice(&[x, y, 0.0, 0.0]); // outer
            g.vertices.extend_from_slice(&[x, y, 1.0, 0.0]); // inner
        }
        // Two triangles per side, wrapping the last side back to corner 0.
        for c in 0..4u32 {
            let a = c * 2; // this corner, outer
            let b = a + 1; // this corner, inner
            let n = ((c + 1) % 4) * 2; // next corner, outer
            let m = n + 1; // next corner, inner
            g.indices.extend_from_slice(&[a, n, b]);
            g.indices.extend_from_slice(&[b, n, m]);
        }
        g
    }

    /// Placeholder single-triangle geometry for PBR drawing (overridden at draw time)
    pub fn from_triangle_pbr() -> GeometryGen {
        let mut g = Self::default();
        // 3 vertices with full PbrVertex stride (16 floats each)
        for _ in 0..3 {
            g.vertices.extend_from_slice(&[
                0.0, 0.0, 0.0, // x, y, z
                0.0, 0.0, 1.0, // nx, ny, nz
                0.0, 0.0, // u, v
                1.0, 1.0, 1.0, 1.0, // color r, g, b, a
                1.0, 0.0, 0.0, 1.0, // tx, ty, tz, tw
            ]);
        }
        g.indices.extend_from_slice(&[0, 1, 2]);
        g
    }

    /// Placeholder single triangle in the packed GameMeshVertex stride.
    pub fn from_triangle_game_mesh() -> GeometryGen {
        let mut g = Self::default();
        for _ in 0..3 {
            // pos, nrm(oct 0,0 = +y), uv, color(white)
            g.vertices
                .extend_from_slice(&[0.0, 0.0, 0.0, 0.0, 0.0, f32::from_bits(u32::MAX)]);
        }
        g.indices.extend_from_slice(&[0, 1, 2]);
        g
    }

    /// Placeholder single triangle in the packed GameMeshVertexAo stride.
    pub fn from_triangle_game_mesh_ao() -> GeometryGen {
        let mut g = Self::default();
        for _ in 0..3 {
            // pos, nrm, uv, color(white), ao_uv
            g.vertices
                .extend_from_slice(&[0.0, 0.0, 0.0, 0.0, 0.0, f32::from_bits(u32::MAX), 0.0]);
        }
        g.indices.extend_from_slice(&[0, 1, 2]);
        g
    }

    /// Placeholder single triangle in the packed GameMeshVertexSkin stride.
    pub fn from_triangle_game_mesh_skin() -> GeometryGen {
        let mut g = Self::default();
        for _ in 0..3 {
            // pos, nrm, uv, joints(0,0,0,0), weights(1,0,0,0), ao_uv(0,0)
            g.vertices
                .extend_from_slice(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, f32::from_bits(0xff), 0.0]);
        }
        g.indices.extend_from_slice(&[0, 1, 2]);
        g
    }

    /// Placeholder single-triangle geometry for minimal faceted-vertex drawing.
    pub fn from_triangle_ico() -> GeometryGen {
        let mut g = Self::default();
        for _ in 0..3 {
            g.vertices.extend_from_slice(&[
                0.0, 0.0, 0.0, 1.0, // pos xyz + pad
                0.0, 0.0, 1.0, 0.0, // normal xyz + pad
            ]);
        }
        g.indices.extend_from_slice(&[0, 1, 2]);
        g
    }

    /// Placeholder single-triangle geometry for depth-mesh drawing with barycentrics.
    pub fn from_triangle_depth_mesh() -> GeometryGen {
        let mut g = Self::default();
        g.vertices.extend_from_slice(&[
            0.0, 0.0, 0.0, 1.0, // pos xyz + pad
            1.0, 0.0, 0.0, 0.0, // barycentric xyz + pad
            0.0, 0.0, 0.0, 1.0, // pos xyz + pad
            0.0, 1.0, 0.0, 0.0, // barycentric xyz + pad
            0.0, 0.0, 0.0, 1.0, // pos xyz + pad
            0.0, 0.0, 1.0, 0.0, // barycentric xyz + pad
        ]);
        g.indices.extend_from_slice(&[0, 1, 2]);
        g
    }

    pub fn from_cube_3d(
        width: f32,
        height: f32,
        depth: f32,
        width_segments: usize,
        height_segments: usize,
        depth_segments: usize,
    ) -> GeometryGen {
        let mut g = Self::default();
        g.add_cube_3d(
            width,
            height,
            depth,
            width_segments,
            height_segments,
            depth_segments,
        );
        g
    }

    pub fn into_cube_vertex_pod(self) -> GeometryGen {
        let vertex_count = self.vertices.len() / 9;
        let mut out = GeometryGen {
            vertices: Vec::with_capacity(vertex_count * 12),
            indices: self.indices,
        };

        for chunk in self.vertices.chunks_exact(9) {
            // Expand the old cube layout:
            // pos3, id1, normal3, uv2
            // into the POD layout the script vertex buffer expects:
            // pos3, id1, normal3, pad1, uv2, tail_pad2
            out.vertices.extend_from_slice(&chunk[0..7]);
            out.vertices.push(0.0);
            out.vertices.extend_from_slice(&chunk[7..9]);
            out.vertices.push(0.0);
            out.vertices.push(0.0);
        }

        out
    }

    // requires pos:vec2 normalized layout
    pub fn add_quad_2d(&mut self, x1: f32, y1: f32, x2: f32, y2: f32) {
        let vertex_offset = self.vertices.len() as u32;
        self.vertices.push(x1);
        self.vertices.push(y1);
        self.vertices.push(x2);
        self.vertices.push(y1);
        self.vertices.push(x2);
        self.vertices.push(y2);
        self.vertices.push(x1);
        self.vertices.push(y2);
        self.indices.push(vertex_offset);
        self.indices.push(vertex_offset + 1);
        self.indices.push(vertex_offset + 2);
        self.indices.push(vertex_offset + 2);
        self.indices.push(vertex_offset + 3);
        self.indices.push(vertex_offset);
    }

    // requires pos:vec3, id:float, normal:vec3, uv:vec2 layout
    pub fn add_cube_3d(
        &mut self,
        width: f32,
        height: f32,
        depth: f32,
        width_segments: usize,
        height_segments: usize,
        depth_segments: usize,
    ) {
        self.add_plane_3d(
            GeometryAxis::Z,
            GeometryAxis::Y,
            GeometryAxis::X,
            -1.0,
            -1.0,
            depth,
            height,
            width,
            depth_segments,
            height_segments,
            0.0,
        );
        self.add_plane_3d(
            GeometryAxis::Z,
            GeometryAxis::Y,
            GeometryAxis::X,
            1.0,
            -1.0,
            depth,
            height,
            -width,
            depth_segments,
            height_segments,
            1.0,
        );
        self.add_plane_3d(
            GeometryAxis::X,
            GeometryAxis::Z,
            GeometryAxis::Y,
            1.0,
            1.0,
            width,
            depth,
            height,
            width_segments,
            depth_segments,
            2.0,
        );
        self.add_plane_3d(
            GeometryAxis::X,
            GeometryAxis::Z,
            GeometryAxis::Y,
            1.0,
            -1.0,
            width,
            depth,
            -height,
            width_segments,
            depth_segments,
            3.0,
        );
        self.add_plane_3d(
            GeometryAxis::X,
            GeometryAxis::Y,
            GeometryAxis::Z,
            1.0,
            -1.0,
            width,
            height,
            depth,
            width_segments,
            height_segments,
            4.0,
        );
        self.add_plane_3d(
            GeometryAxis::X,
            GeometryAxis::Y,
            GeometryAxis::Z,
            -1.0,
            -1.0,
            width,
            height,
            -depth,
            width_segments,
            height_segments,
            5.0,
        );
    }

    // requires pos:vec3, id:float, normal:vec3, uv:vec2 layout
    #[allow(clippy::too_many_arguments)]
    pub fn add_plane_3d(
        &mut self,
        u: GeometryAxis,
        v: GeometryAxis,
        w: GeometryAxis,
        udir: f32,
        vdir: f32,
        width: f32,
        height: f32,
        depth: f32,
        grid_x: usize,
        grid_y: usize,
        id: f32,
    ) {
        let segment_width = width / (grid_x as f32);
        let segment_height = height / (grid_y as f32);
        let width_half = width / 2.0;
        let height_half = height / 2.0;
        let depth_half = depth / 2.0;
        let grid_x1 = grid_x + 1;
        let grid_y1 = grid_y + 1;

        let vertex_offset = self.vertices.len() / 9;

        for iy in 0..grid_y1 {
            let y = (iy as f32) * segment_height - height_half;

            for ix in 0..grid_x1 {
                let x = (ix as f32) * segment_width - width_half;
                let off = self.vertices.len();
                self.vertices.push(0.0);
                self.vertices.push(0.0);
                self.vertices.push(0.0);

                self.vertices[off + u as usize] = x * udir;
                self.vertices[off + v as usize] = y * vdir;
                self.vertices[off + w as usize] = depth_half;

                self.vertices.push(id);
                let off = self.vertices.len();
                self.vertices.push(0.0);
                self.vertices.push(0.0);
                self.vertices.push(0.0);
                self.vertices[off + w as usize] = if depth > 0.0 { 1.0 } else { -1.0 };

                self.vertices.push((ix as f32) / (grid_x as f32));
                self.vertices.push(1.0 - (iy as f32) / (grid_y as f32));
            }
        }

        for iy in 0..grid_y {
            for ix in 0..grid_x {
                let a = vertex_offset + ix + grid_x1 * iy;
                let b = vertex_offset + ix + grid_x1 * (iy + 1);
                let c = vertex_offset + (ix + 1) + grid_x1 * (iy + 1);
                let d = vertex_offset + (ix + 1) + grid_x1 * iy;
                self.indices.push(a as u32);
                self.indices.push(b as u32);
                self.indices.push(d as u32);
                self.indices.push(b as u32);
                self.indices.push(c as u32);
                self.indices.push(d as u32);
            }
        }
    }
}
