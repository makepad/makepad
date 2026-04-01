mod tests {
    use super::*;

    #[test]
    fn depth_query_plane_supports_body_respects_exact_quad_edge() {
        let plane = DepthQuerySupportPlane {
            point: vec3f(0.0, 0.0, 0.0),
            normal: vec3f(0.0, 1.0, 0.0),
            tangent: vec3f(1.0, 0.0, 0.0),
            bitangent: vec3f(0.0, 0.0, 1.0),
            half_extent_tangent: 0.10,
            half_extent_bitangent: 0.08,
        };

        assert!(depth_query_plane_supports_body(
            plane,
            vec3f(0.099, 0.03, 0.0),
            0.05,
            0.0,
        ));
        assert!(!depth_query_plane_supports_body(
            plane,
            vec3f(0.101, 0.03, 0.0),
            0.05,
            0.0,
        ));
    }

    #[test]
    fn retained_support_refresh_requeries_before_body_reaches_quad_edge() {
        let plane = DepthQuerySupportPlane {
            point: vec3f(0.0, 0.0, 0.0),
            normal: vec3f(0.0, 1.0, 0.0),
            tangent: vec3f(1.0, 0.0, 0.0),
            bitangent: vec3f(0.0, 0.0, 1.0),
            half_extent_tangent: 0.10,
            half_extent_bitangent: 0.10,
        };
        let retained = RetainedDepthQueryHit {
            surfaces: std::array::from_fn(|index| {
                (index == 0).then_some(RetainedDepthQuerySurface {
                    target: DepthQuerySurfaceTarget {
                        collider: DepthQueryCollider {
                            fingerprint: 1,
                            geometry: DepthQueryColliderGeometry::HalfSpace(plane),
                            role: DepthQueryColliderRole::Support,
                            restitution: 0.0,
                        },
                    },
                    misses_left: XR_DEPTH_QUERY_HIT_MISS_GRACE_FRAMES,
                })
            }),
        };

        assert!(retained.can_skip_refresh(
            vec3f(0.00, 0.03, 0.0),
            vec3f(0.02, 0.03, 0.0),
            0.05,
            0.30,
        ));
        assert!(!retained.can_skip_refresh(
            vec3f(0.06, 0.03, 0.0),
            vec3f(0.09, 0.03, 0.0),
            0.05,
            0.30,
        ));
    }

    #[test]
    fn slow_support_refresh_margin_is_less_aggressive_than_fast_motion() {
        let plane = DepthQuerySupportPlane {
            point: vec3f(0.0, 0.0, 0.0),
            normal: vec3f(0.0, 1.0, 0.0),
            tangent: vec3f(1.0, 0.0, 0.0),
            bitangent: vec3f(0.0, 0.0, 1.0),
            half_extent_tangent: 0.10,
            half_extent_bitangent: 0.10,
        };
        let retained = RetainedDepthQueryHit {
            surfaces: std::array::from_fn(|index| {
                (index == 0).then_some(RetainedDepthQuerySurface {
                    target: DepthQuerySurfaceTarget {
                        collider: DepthQueryCollider {
                            fingerprint: 2,
                            geometry: DepthQueryColliderGeometry::HalfSpace(plane),
                            role: DepthQueryColliderRole::Support,
                            restitution: 0.0,
                        },
                    },
                    misses_left: XR_DEPTH_QUERY_HIT_MISS_GRACE_FRAMES,
                })
            }),
        };

        assert!(retained.can_skip_refresh(
            vec3f(0.082, 0.03, 0.0),
            vec3f(0.088, 0.03, 0.0),
            0.05,
            0.05,
        ));
        assert!(!retained.can_skip_refresh(
            vec3f(0.082, 0.03, 0.0),
            vec3f(0.088, 0.03, 0.0),
            0.05,
            0.30,
        ));
    }

    #[test]
    fn retained_support_does_not_hide_impact_capable_motion() {
        assert!(depth_query_should_refresh_from_tsdf(
            false, true, 0.12, true,
        ));
        assert!(!depth_query_should_refresh_from_tsdf(
            false, true, 0.12, false,
        ));
        assert!(!depth_query_should_refresh_from_tsdf(
            true, false, 0.40, true,
        ));
    }
}
