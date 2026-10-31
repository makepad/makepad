mod tests {
    use super::*;
    use makepad_widgets::makepad_platform::xr_tsdf::mesh::XR_DEBUG_DEPTH_FLOATS_PER_VERTEX;
    use makepad_widgets::*;

    fn packed_position(vertices: &[f32], vertex_index: usize) -> Vec3f {
        let base = vertex_index * XR_DEBUG_DEPTH_FLOATS_PER_VERTEX;
        vec3f(vertices[base], vertices[base + 1], vertices[base + 2])
    }

    fn packed_barycentric(vertices: &[f32], vertex_index: usize) -> Vec3f {
        let base = vertex_index * XR_DEBUG_DEPTH_FLOATS_PER_VERTEX;
        vec3f(vertices[base + 4], vertices[base + 5], vertices[base + 6])
    }

    #[test]
    fn push_debug_depth_plane_emits_single_quad_mesh() {
        let plane = DepthQuerySupportPlane {
            point: vec3f(0.0, 0.5, 0.0),
            normal: vec3f(0.0, 1.0, 0.0),
            tangent: vec3f(1.0, 0.0, 0.0),
            bitangent: vec3f(0.0, 0.0, 1.0),
            half_extent_tangent: 0.25,
            half_extent_bitangent: 0.10,
        };
        let mut indices = Vec::new();
        let mut vertices = Vec::new();

        push_debug_depth_plane(&mut indices, &mut vertices, plane);

        assert_eq!(indices, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(vertices.len(), 6 * XR_DEBUG_DEPTH_FLOATS_PER_VERTEX);
        let first = packed_position(&vertices, 0);
        let sixth = packed_position(&vertices, 5);
        assert_eq!(first, vec3f(-0.25, 0.5, -0.10));
        assert_eq!(sixth, vec3f(-0.25, 0.5, 0.10));
        assert_eq!(packed_barycentric(&vertices, 3), vec3f(1.0, 0.0, 0.0));
        assert_eq!(packed_barycentric(&vertices, 4), vec3f(0.0, 1.0, 0.0));
        assert_eq!(packed_barycentric(&vertices, 5), vec3f(0.0, 0.0, 1.0));
    }
}
