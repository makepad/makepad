use crate::math::{Real, Vector, DIM};
use crate::query::{PointProjection, PointQuery, PointQueryWithLocation};
use crate::shape::{FeatureId, Triangle, TrianglePointLocation};

#[inline]
fn compute_result(pt: Vector, proj: Vector) -> PointProjection {
    #[cfg(feature = "dim2")]
    {
        PointProjection::new(pt == proj, proj)
    }

    #[cfg(feature = "dim3")]
    {
        // TODO: is this acceptable to assume the point is inside of the
        // triangle if it is close enough?
        PointProjection::new(relative_eq!(proj, pt), proj)
    }
}

impl PointQuery for Triangle {
    #[inline]
    fn project_local_point(&self, pt: Vector, solid: bool) -> PointProjection {
        self.project_local_point_and_get_location(pt, solid).0
    }

    #[inline]
    fn project_local_point_and_get_feature(&self, pt: Vector) -> (PointProjection, FeatureId) {
        let (proj, loc) = if DIM == 2 {
            self.project_local_point_and_get_location(pt, false)
        } else {
            self.project_local_point_and_get_location(pt, true)
        };

        let feature = match loc {
            TrianglePointLocation::OnVertex(i) => FeatureId::Vertex(i),
            #[cfg(feature = "dim3")]
            TrianglePointLocation::OnEdge(i, _) => FeatureId::Edge(i),
            #[cfg(feature = "dim2")]
            TrianglePointLocation::OnEdge(i, _) => FeatureId::Face(i),
            TrianglePointLocation::OnFace(i, _) => FeatureId::Face(i),
            TrianglePointLocation::OnSolid => FeatureId::Face(0),
        };

        (proj, feature)
    }

    // NOTE: the default implementation of `.distance_to_point(...)` will return the error that was
    // eaten by the `::approx_eq(...)` on `project_point(...)`.
}

impl PointQueryWithLocation for Triangle {
    type Location = TrianglePointLocation;

    #[inline]
    fn project_local_point_and_get_location(
        &self,
        pt: Vector,
        solid: bool,
    ) -> (PointProjection, Self::Location) {
        // To understand the ideas, consider reading the slides below
        // https://box2d.org/files/ErinCatto_GJK_GDC2010.pdf

        let a = self.a;
        let b = self.b;
        let c = self.c;

        let ab = b - a;
        let ac = c - a;
        let ap = pt - a;

        let ab_ap = ab.dot(ap);
        let ac_ap = ac.dot(ap);

        if ab_ap <= 0.0 && ac_ap <= 0.0 {
            // Voronoï region of `a`.
            return (compute_result(pt, a), TrianglePointLocation::OnVertex(0));
        }

        let bp = pt - b;
        let ab_bp = ab.dot(bp);
        let ac_bp = ac.dot(bp);

        if ab_bp >= 0.0 && ac_bp <= ab_bp {
            // Voronoï region of `b`.
            return (compute_result(pt, b), TrianglePointLocation::OnVertex(1));
        }

        let cp = pt - c;
        let ab_cp = ab.dot(cp);
        let ac_cp = ac.dot(cp);

        if ac_cp >= 0.0 && ab_cp <= ac_cp {
            // Voronoï region of `c`.
            return (compute_result(pt, c), TrianglePointLocation::OnVertex(2));
        }

        enum ProjectionInfo {
            OnAB,
            OnAC,
            OnBC,
            // The usize indicates if we are on the CW side (0) or CCW side (1) of the face.
            OnFace(usize, Real, Real, Real),
        }

        // Checks on which edge voronoï region the point is.
        // For 2D and 3D, it uses explicit cross/perp products that are
        // more numerically stable.
        fn stable_check_edges_voronoi(
            ab: Vector,
            ac: Vector,
            bc: Vector,
            ap: Vector,
            bp: Vector,
            cp: Vector,
            ab_ap: Real,
            ab_bp: Real,
            ac_ap: Real,
            ac_cp: Real,
            ac_bp: Real,
            ab_cp: Real,
        ) -> ProjectionInfo {
            #[cfg(feature = "dim2")]
            {
                let n = ab.perp_dot(ac);
                let vc = n * ab.perp_dot(ap);
                if vc < 0.0 && ab_ap >= 0.0 && ab_bp <= 0.0 {
                    return ProjectionInfo::OnAB;
                }

                let vb = -n * ac.perp_dot(cp);
                if vb < 0.0 && ac_ap >= 0.0 && ac_cp <= 0.0 {
                    return ProjectionInfo::OnAC;
                }

                let va = n * bc.perp_dot(bp);
                if va < 0.0 && ac_bp - ab_bp >= 0.0 && ab_cp - ac_cp >= 0.0 {
                    return ProjectionInfo::OnBC;
                }

                ProjectionInfo::OnFace(0, va, vb, vc)
            }
            #[cfg(feature = "dim3")]
            {
                let n;

                #[cfg(feature = "improved_fixed_point_support")]
                {
                    let scaled_n = ab.cross(ac);
                    n = scaled_n.try_normalize(0.0).unwrap_or(scaled_n);
                }

                #[cfg(not(feature = "improved_fixed_point_support"))]
                {
                    n = ab.cross(ac);
                }

                let vc = n.dot(ab.cross(ap));
                if vc < 0.0 && ab_ap >= 0.0 && ab_bp <= 0.0 {
                    return ProjectionInfo::OnAB;
                }

                let vb = -n.dot(ac.cross(cp));
                if vb < 0.0 && ac_ap >= 0.0 && ac_cp <= 0.0 {
                    return ProjectionInfo::OnAC;
                }

                let va = n.dot(bc.cross(bp));
                if va < 0.0 && ac_bp - ab_bp >= 0.0 && ab_cp - ac_cp >= 0.0 {
                    return ProjectionInfo::OnBC;
                }

                let clockwise = if n.dot(ap) >= 0.0 { 0 } else { 1 };

                ProjectionInfo::OnFace(clockwise, va, vb, vc)
            }
        }

        let bc = c - b;
        match stable_check_edges_voronoi(
            ab, ac, bc, ap, bp, cp, ab_ap, ab_bp, ac_ap, ac_cp, ac_bp, ab_cp,
        ) {
            ProjectionInfo::OnAB => {
                // Voronoï region of `ab`.
                let v = ab_ap / ab.length_squared();
                let bcoords = [1.0 - v, v];

                let res = a + ab * v;
                return (
                    compute_result(pt, res),
                    TrianglePointLocation::OnEdge(0, bcoords),
                );
            }
            ProjectionInfo::OnAC => {
                // Voronoï region of `ac`.
                let w = ac_ap / ac.length_squared();
                let bcoords = [1.0 - w, w];

                let res = a + ac * w;
                return (
                    compute_result(pt, res),
                    TrianglePointLocation::OnEdge(2, bcoords),
                );
            }
            ProjectionInfo::OnBC => {
                // Voronoï region of `bc`.
                let w = bc.dot(bp) / bc.length_squared();
                let bcoords = [1.0 - w, w];

                let res = b + bc * w;
                return (
                    compute_result(pt, res),
                    TrianglePointLocation::OnEdge(1, bcoords),
                );
            }
            ProjectionInfo::OnFace(face_side, va, vb, vc) => {
                // Voronoï region of the face.
                if DIM != 2 {
                    // NOTE: in some cases, numerical instability
                    // may result in the denominator being zero
                    // when the triangle is nearly degenerate.
                    if va + vb + vc != 0.0 {
                        let denom = 1.0 / (va + vb + vc);
                        let v = vb * denom;
                        let w = vc * denom;
                        let bcoords = [1.0 - v - w, v, w];
                        let res = a + ab * v + ac * w;

                        return (
                            compute_result(pt, res),
                            TrianglePointLocation::OnFace(face_side as u32, bcoords),
                        );
                    }
                }
            }
        }

        // Special treatment if we work in 2d because in this case we really are inside of the
        // object.
        if solid {
            (
                PointProjection::new(true, pt),
                TrianglePointLocation::OnSolid,
            )
        } else {
            // We have to project on the closest edge.

            // TODO: this might be optimizable.
            // TODO: be careful with numerical errors.
            let v = ab_ap / (ab_ap - ab_bp); // proj on ab = a + ab * v
            let w = ac_ap / (ac_ap - ac_cp); // proj on ac = a + ac * w
            let u = (ac_bp - ab_bp) / (ac_bp - ab_bp + ab_cp - ac_cp); // proj on bc = b + bc * u

            let bc = c - b;
            let d_ab = ap.length_squared() - (ab.length_squared() * v * v);
            let d_ac = ap.length_squared() - (ac.length_squared() * w * w);
            let d_bc = bp.length_squared() - (bc.length_squared() * u * u);

            let proj;
            let loc;

            if d_ab < d_ac {
                if d_ab < d_bc {
                    // ab
                    let bcoords = [1.0 - v, v];
                    proj = a + ab * v;
                    loc = TrianglePointLocation::OnEdge(0, bcoords);
                } else {
                    // bc
                    let bcoords = [1.0 - u, u];
                    proj = b + bc * u;
                    loc = TrianglePointLocation::OnEdge(1, bcoords);
                }
            } else if d_ac < d_bc {
                // ac
                let bcoords = [1.0 - w, w];
                proj = a + ac * w;
                loc = TrianglePointLocation::OnEdge(2, bcoords);
            } else {
                // bc
                let bcoords = [1.0 - u, u];
                proj = b + bc * u;
                loc = TrianglePointLocation::OnEdge(1, bcoords);
            }

            (PointProjection::new(true, proj), loc)
        }
    }
}
