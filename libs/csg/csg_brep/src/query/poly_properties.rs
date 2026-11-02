//! Manifold test, volume and mass properties of polygonal B-reps: the per-face pyramid
//! sums of `SmPolyFace::ComputeProperties`, their total over a poly B-rep, and the
//! `SM_API` queries built on them.
//!
//! Upstream: `source/SMLib/src/SmPoly.cpp`, `source/SMLib/inc/SmPoly.h`,
//! `source/SMLib/inc/SmTopoTypes.h`, `source/SM_API/src/SmApiPolygons.cpp`.
//!
//! | Rust                                         | Upstream                                    |
//! |----------------------------------------------|---------------------------------------------|
//! | [`MassPropertiesType`]                       | `SmMassPropertiesType`                      |
//! | [`PolyProperties`]                           | the outputs of `ComputeProperties`          |
//! | [`PolyBrep::edge_is_manifold`]               | `SmPolyEdge::IsManifold`                    |
//! | [`PolyBrep::is_manifold_solid`]              | `SmPolyBrep::IsManifoldSolid`               |
//! | [`PolyBrep::face_compute_properties`]        | `SmPolyFace::ComputeProperties`, `SmPolyEdge::GetOtherPolyVertex` |
//! | [`PolyBrep::compute_properties`]             | `SmPolyBrep::ComputeProperties`             |
//! | [`poly_brep_is_manifold_solid`]              | `SmApiPolyBrepIsManifoldSolid`              |
//! | [`poly_brep_compute_volume`]                 | `SmApiPolyBrepComputeVolume`                |
//! | [`poly_brep_compute_mass_properties`], [`PolyMassProperties`] | `SmApiPolyBrepComputeMassProperties` |
//!
//! Deviations:
//! - **Outputs.** The out-parameters are return values ([`PolyProperties`],
//!   [`PolyMassProperties`]); a failure returns no partial sums.
//! - **`NULL` poly B-rep.** The `SM_API` functions take a reference, so their
//!   `SM_ERR_INVALID_INPUT` for a null pointer has no counterpart.
//! - **Mutable B-rep.** `SmPolyFace::GetNormal` (called with its defaults `FALSE`,
//!   `TRUE`) caches the normal and raises tolerances on a `const` face, so the
//!   property functions take `&mut PolyBrep`.
//! - **Missing objects.** A face without edges (upstream reads `edges[0]` of an empty
//!   array) is [`SolidError::InvalidInput`]; an edge without start or end vertex
//!   (a null dereference upstream) is `SM_ERR_NULL_POINTER`.

use crate::error::{Result, SolidError};
use crate::geom::vector3d::is_valid_double;
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, EFF_ZERO};

use super::poly::{PolyBrep, PolyEdgeId, PolyFaceId};

const OP_FACE: &str = "query::poly_properties::face_compute_properties";
const OP_MASS: &str = "query::poly_properties::poly_brep_compute_mass_properties";

/// Which properties `ComputeProperties` evaluates (`SmMassPropertiesType`): each one
/// also computes all earlier ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum MassPropertiesType {
    /// `SM_MPT_AREA`: area only.
    Area,
    /// `SM_MPT_VOLUME`: area and volume.
    Volume,
    /// `SM_MPT_CENTROID`: area, volume and centroid.
    Centroid,
    /// `SM_MPT_MOMENTS`: area, volume, centroid and moments.
    Moments,
    /// `SM_MPT_ALL`: all properties (as [`MassPropertiesType::Moments`]).
    #[default]
    All,
}

/// The outputs of `ComputeProperties`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PolyProperties {
    /// `rdArea` (`rdDeltaArea`).
    pub area: f64,
    /// `rdVolume` (`rdDeltaVolume`): the signed volume between the faces and the origin.
    pub volume: f64,
    /// `rBarycenter`: for one face the centroid times the delta volume; for a poly B-rep
    /// the centroid (divided by the volume unless only the area was asked for or the
    /// volume is zero).
    pub barycenter: Point3,
    /// `aMoments`: `[0]` the second moments (integral of `x^2, y^2, z^2 dV`), `[1]` the
    /// products (integral of `xy, yz, zx dV`), about the origin of computation.
    pub moments: [Vec3; 2],
}

/// The result of [`poly_brep_compute_mass_properties`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolyMassProperties {
    /// `rdArea`.
    pub area: f64,
    /// `rdVolume` (non-negative).
    pub volume: f64,
    /// `rdMass`: density times volume.
    pub mass: f64,
    /// `rCentroid`: the barycenter, a world position.
    pub centroid: Point3,
    /// `rMomentsOfInertia`: `Ixx, Iyy, Izz` about the origin, density-weighted.
    pub moments_of_inertia: Vec3,
    /// `rProductsOfInertia`: the products `yz, zx, xy` about the origin, density-weighted.
    pub products_of_inertia: Vec3,
}

impl PolyBrep {
    /// Whether `e` is shared by exactly two faces: its radial partner is another edge
    /// whose partner is `e` (`SmPolyEdge::IsManifold`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an unknown or deleted edge.
    pub fn edge_is_manifold(&self, e: PolyEdgeId) -> Result<bool> {
        // m_pNextRadialE != this && m_pNextRadialE->m_pNextRadialE == this
        Ok(match self.edge_radial(e)? {
            None => false,
            Some(next) => self.edge_radial(next)? == Some(e),
        })
    }

    /// Whether every poly edge is manifold (`SmPolyBrep::IsManifoldSolid`).
    ///
    /// # Errors
    ///
    /// Those of the edge walk.
    pub fn is_manifold_solid(&self) -> Result<bool> {
        // for every PolyEdge - check for nonManifold
        for e in self.poly_edges(None)? {
            if !self.edge_is_manifold(e)? {
                // not manifold - return FALSE
                return Ok(false);
            }
        }
        // manifold - return TRUE
        Ok(true)
    }

    /// The area of face `f` and the volume, centroid and moments of the pyramid between
    /// it and `origin` (`SmPolyFace::ComputeProperties`): every edge after the first
    /// adds the trapezoid between it and the first edge's line, as two tetrahedra with
    /// apex `origin`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a face without edges or a zero-length first
    /// edge (`Unitize`'s `SM_ERR_INVALID_INPUT`); `SM_ERR_NULL_POINTER` for an edge
    /// without vertices; those of the edge queries and of `GetNormal`.
    pub fn face_compute_properties(
        &mut self,
        k: &Kernel,
        f: PolyFaceId,
        origin: Point3,
        which: MassPropertiesType,
    ) -> Result<PolyProperties> {
        const OP: &str = OP_FACE;
        // Init outputs
        let mut area = 0.0;
        let mut volume = 0.0;
        let mut barycenter = Vec3::ZERO;
        let mut moments = [Vec3::ZERO; 2];

        // Get the base points.  We'll use the first Edge.
        let edges = self.face_poly_edges(f)?;
        let first = *edges
            .first()
            .ok_or_else(|| SolidError::invalid_input(OP, "face without poly edges"))?;
        let start = |b: &Self, e: PolyEdgeId| -> Result<Point3> {
            let v = b
                .edge_start_poly_vertex(e)?
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            b.vertex_point(v)
        };
        let end = |b: &Self, e: PolyEdgeId| -> Result<Point3> {
            let v = b
                .edge_end_poly_vertex(e)?
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            b.vertex_point(v)
        };
        let base_pt0 = start(self, first)?;
        let base_pt1 = end(self, first)?;

        // zero-length edge?
        let base_vec_unit = (base_pt1 - base_pt0).unitize()?;

        // Also get a unit vector perp to sBaseVecUnit, in the plane: the 'up' vector.
        let (norm, _) = self.face_normal(k, f, false, true)?;
        // This should already be unit, but to be safe... (status ignored)
        let up_vec_unit = norm.cross(base_vec_unit).unitize_or_z();

        // Loop on all subsequent Edgeuses.
        for &e in edges.iter().skip(1) {
            // don't use the previous sP1 as the new sP0: we might jump between loops
            // (GetOtherPolyVertex of the start vertex is the end vertex)
            let p0 = start(self, e)?;
            let p1 = end(self, e)?;

            // Area: base length times average height (sP0 - sP1: upstream switched
            // the points of SmFace::GetProperties).
            let this_edge = p0 - p1;
            let base = this_edge.dot(base_vec_unit);
            let h0 = (p0 - base_pt0).dot(up_vec_unit);
            let h1 = (p1 - base_pt0).dot(up_vec_unit);

            area += base * (h0 + h1); // We'll divide by 2 after the loop.

            if which == MassPropertiesType::Area {
                continue;
            }

            // Volume: the trapezoidal pyramid is two tetrahedra,
            //  vol = 1/6 ( VR0 cross VP1 ) dot ( R1 - P0 ).

            // Get the actual projections of sP0 and sP1 on the base line.
            let r0 = base_pt0 + base_vec_unit * (p0 - base_pt0).dot(base_vec_unit);
            let r1 = base_pt0 + base_vec_unit * (p1 - base_pt0).dot(base_vec_unit);

            let vp1 = p1 - origin;
            let vr0 = r0 - origin;

            // (sVR0 * sVP1: upstream switched the vectors of SmFace::GetProperties)
            let cross = vr0.cross(vp1);

            let this_vol = cross.dot(r1 - p0);
            volume += this_vol; // We'll divide by 6 after the loop.

            if which == MassPropertiesType::Volume {
                continue;
            }

            // The remaining properties need the two tetrahedra separately: origin to
            // Proj0, Proj1 and Pt1, and origin to Proj0, Pt1 and Pt0.
            let vp0 = p0 - origin;
            let vr1 = r1 - origin;

            let vol1 = cross.dot(vr1);
            let vol2 = -cross.dot(vp0);

            // Center of mass.
            let cent = (r0 + r1 + p1 + origin) / 4.0;
            barycenter += vol1 * cent;

            let cent = (r0 + p1 + p0 + origin) / 4.0;
            barycenter += vol2 * cent;

            if which == MassPropertiesType::Centroid {
                continue;
            }

            // Moments. Ixx, Iyy, Izz all at once; we'll divide by 10 after the loop.
            moments[0] += vol1
                * (vr0.multiply(vr0)
                    + vr1.multiply(vr1)
                    + vp1.multiply(vp1)
                    + vr0.multiply(vr1)
                    + vr0.multiply(vp1)
                    + vr1.multiply(vp1));

            moments[0] += vol2
                * (vr0.multiply(vr0)
                    + vp0.multiply(vp0)
                    + vp1.multiply(vp1)
                    + vr0.multiply(vp0)
                    + vr0.multiply(vp1)
                    + vp0.multiply(vp1));

            // Products of inertia, for this tetrahedron; we'll divide by 20 after the
            // loop.
            // Ixy:
            moments[1].x += vol1
                * (2.0 * (vr0.x * vr0.y + vr1.x * vr1.y + vp1.x * vp1.y)
                    + vr0.x * vr1.y
                    + vr1.x * vr0.y
                    + vr0.x * vp1.y
                    + vp1.x * vr0.y
                    + vr1.x * vp1.y
                    + vp1.x * vr1.y);
            moments[1].x += vol2
                * (2.0 * (vr0.x * vr0.y + vp0.x * vp0.y + vp1.x * vp1.y)
                    + vr0.x * vp0.y
                    + vp0.x * vr0.y
                    + vr0.x * vp1.y
                    + vp1.x * vr0.y
                    + vp0.x * vp1.y
                    + vp1.x * vp0.y);

            // Iyz
            moments[1].y += vol1
                * (2.0 * (vr0.z * vr0.y + vr1.z * vr1.y + vp1.z * vp1.y)
                    + vr0.z * vr1.y
                    + vr1.z * vr0.y
                    + vr0.z * vp1.y
                    + vp1.z * vr0.y
                    + vr1.z * vp1.y
                    + vp1.z * vr1.y);
            moments[1].y += vol2
                * (2.0 * (vr0.z * vr0.y + vp0.z * vp0.y + vp1.z * vp1.y)
                    + vr0.z * vp0.y
                    + vp0.z * vr0.y
                    + vr0.z * vp1.y
                    + vp1.z * vr0.y
                    + vp0.z * vp1.y
                    + vp1.z * vp0.y);

            // Izx
            moments[1].z += vol1
                * (2.0 * (vr0.z * vr0.x + vr1.z * vr1.x + vp1.z * vp1.x)
                    + vr0.z * vr1.x
                    + vr1.z * vr0.x
                    + vr0.z * vp1.x
                    + vp1.z * vr0.x
                    + vr1.z * vp1.x
                    + vp1.z * vr1.x);
            moments[1].z += vol2
                * (2.0 * (vr0.z * vr0.x + vp0.z * vp0.x + vp1.z * vp1.x)
                    + vr0.z * vp0.x
                    + vp0.z * vr0.x
                    + vr0.z * vp1.x
                    + vp1.z * vr0.x
                    + vp0.z * vp1.x
                    + vp1.z * vp0.x);
        } // end loop on all Edgeuses of this face, summing their contributions.

        // Divide by the constant factors.
        area /= 2.0;
        volume /= 6.0;
        // These were also multiplied by the volume, so do another 6 (operator/=):
        barycenter /= 6.0;
        moments[0] /= 10.0 * 6.0;
        moments[1] /= 20.0 * 6.0;

        Ok(PolyProperties {
            area,
            volume,
            barycenter,
            moments,
        })
    }

    /// The sums of [`PolyBrep::face_compute_properties`] over every poly face, the
    /// barycenter divided by the volume (`SmPolyBrep::ComputeProperties`). Closed faces
    /// give the enclosed volume for any origin.
    ///
    /// # Errors
    ///
    /// Those of [`PolyBrep::face_compute_properties`] and of the face walk.
    pub fn compute_properties(
        &mut self,
        k: &Kernel,
        origin: Point3,
        which: MassPropertiesType,
    ) -> Result<PolyProperties> {
        // Initialize outputs.
        let mut total = PolyProperties::default();

        for f in self.poly_faces(None)? {
            let delta = self.face_compute_properties(k, f, origin, which)?;
            total.area += delta.area;
            total.volume += delta.volume;
            total.barycenter += delta.barycenter;
            total.moments[0] += delta.moments[0];
            total.moments[1] += delta.moments[1];
        }

        if which != MassPropertiesType::Area && total.volume != 0.0 {
            // plain `/` as upstream (not `/=`, whose Vec3 operator checks the denominator)
            total.barycenter = core::ops::Div::div(total.barycenter, total.volume);
        }

        Ok(total)
    }
}

/// Whether a poly B-rep is a manifold solid: every edge shared by exactly two faces
/// (`SmApiPolyBrepIsManifoldSolid`).
///
/// # Errors
///
/// Those of [`PolyBrep::is_manifold_solid`].
pub fn poly_brep_is_manifold_solid(poly_brep: &PolyBrep) -> Result<bool> {
    poly_brep.is_manifold_solid()
}

/// The signed volume enclosed by a poly B-rep, summed about its bounding box's middle
/// (`SmApiPolyBrepComputeVolume`).
///
/// # Errors
///
/// Those of the bounding box and of [`PolyBrep::compute_properties`].
pub fn poly_brep_compute_volume(k: &Kernel, poly_brep: &mut PolyBrep) -> Result<f64> {
    let bbox = poly_brep
        .calculate_bounding_box(false, None, false, None)?
        .normal_box;
    let origin = bbox.mid();
    Ok(poly_brep
        .compute_properties(k, origin, MassPropertiesType::Volume)?
        .volume)
}

/// Area, volume, mass, centroid and the moments and products of inertia of a closed
/// manifold poly B-rep about `origin` (`SmApiPolyBrepComputeMassProperties`). A
/// globally reversed winding is undone (the signed integrals are negated).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for an invalid or below-`SM_EFF_ZERO` density, an
/// invalid origin, a poly B-rep that is not a manifold solid, and an invalid or
/// below-`SM_EFF_ZERO` volume; those of [`PolyBrep::compute_properties`].
pub fn poly_brep_compute_mass_properties(
    k: &Kernel,
    poly_brep: &mut PolyBrep,
    density: f64,
    origin: Point3,
) -> Result<PolyMassProperties> {
    const OP: &str = OP_MASS;
    if !is_valid_double(density) || density < EFF_ZERO {
        return Err(SolidError::invalid_input(OP, "invalid density"));
    }

    if !is_valid_double(origin.x) || !is_valid_double(origin.y) || !is_valid_double(origin.z) {
        return Err(SolidError::invalid_input(OP, "invalid origin"));
    }

    // Only defined for a closed manifold mesh; an open one gives a volume that
    // depends on the origin.
    if !poly_brep.is_manifold_solid()? {
        return Err(SolidError::invalid_input(OP, "not a manifold solid"));
    }

    let p = poly_brep.compute_properties(k, origin, MassPropertiesType::All)?;
    let mut volume = p.volume;
    let mut moments = p.moments;

    if !is_valid_double(volume) {
        return Err(SolidError::invalid_input(OP, "invalid volume"));
    }

    // A closed mesh with globally reversed winding passes IsManifoldSolid, but every
    // signed integral comes back negated: volume, second moments and products.
    if volume < 0.0 {
        volume = -volume;
        moments[0] *= -1.0;
        moments[1] *= -1.0;
    }

    if volume < EFF_ZERO {
        return Err(SolidError::invalid_input(OP, "degenerate volume"));
    }

    // Second moments about the planes through the origin, density-weighted.
    let second = moments[0] * density;

    Ok(PolyMassProperties {
        area: p.area,
        volume,
        mass: density * volume,
        // The kernel already returns the barycenter as a world position.
        centroid: p.barycenter,
        // Ixx = integral of (y^2 + z^2) dm, cyclically.
        moments_of_inertia: Vec3::new(
            second.y + second.z,
            second.x + second.z,
            second.x + second.y,
        ),
        // Reorder the kernel's (xy, yz, zx) to the Brep API's (yz, zx, xy).
        products_of_inertia: Vec3::new(
            moments[1].y * density,
            moments[1].z * density,
            moments[1].x * density,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit cube `[0, 1]^3` with outward faces and radial pairs, as the difftest
    /// builds it.
    fn cube(k: &Kernel, flip: bool) -> PolyBrep {
        let pts = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ];
        let mut faces: Vec<Vec<usize>> = vec![
            vec![0, 3, 2, 1],
            vec![4, 5, 6, 7],
            vec![0, 1, 5, 4],
            vec![1, 2, 6, 5],
            vec![2, 3, 7, 6],
            vec![3, 0, 4, 7],
        ];
        if flip {
            for f in &mut faces {
                f.reverse();
            }
        }
        let mut b = PolyBrep::new(k, 1e-5);
        let r = b.add_region().unwrap();
        let s = b.add_shell(r).unwrap();
        let vs: Vec<_> = pts
            .iter()
            .map(|p| b.add_vertex(k, Vec3::new(p[0], p[1], p[2]), 1e-5).unwrap())
            .collect();
        let mut edges = Vec::new();
        for f in &faces {
            let face = b.add_face(k, s, 1e-5, None).unwrap();
            let lp = b.start_poly_edge_loop(face).unwrap();
            for j in 0..f.len() {
                let (a, c) = (vs[f[j]], vs[f[(j + 1) % f.len()]]);
                let (pa, pc) = (b.vertex_point(a).unwrap(), b.vertex_point(c).unwrap());
                let e = b.add_poly_edge(k, lp, 1e-5, pa, pc, a).unwrap();
                edges.push((f[j], f[(j + 1) % f.len()], e));
            }
            let _ = b.finish_poly_edge_loop(lp);
        }
        for &(a, c, e) in &edges {
            if a < c {
                let mate = edges.iter().find(|x| x.0 == c && x.1 == a).unwrap().2;
                b.link_radials(&[e, mate]).unwrap();
            }
        }
        b
    }

    #[test]
    fn cube_is_a_manifold_solid_of_volume_one() {
        let k = Kernel::default();
        let mut b = cube(&k, false);
        assert!(poly_brep_is_manifold_solid(&b).unwrap());
        let v = poly_brep_compute_volume(&k, &mut b).unwrap();
        assert!((v.abs() - 1.0).abs() < 1e-12, "{v}");
    }

    #[test]
    fn mass_properties_ignore_the_winding() {
        let k = Kernel::default();
        let o = Point3::ZERO;
        let a = poly_brep_compute_mass_properties(&k, &mut cube(&k, false), 2.0, o).unwrap();
        let b = poly_brep_compute_mass_properties(&k, &mut cube(&k, true), 2.0, o).unwrap();
        assert!((a.volume - 1.0).abs() < 1e-12);
        assert!((a.mass - 2.0).abs() < 1e-12);
        assert!((a.area - 6.0).abs() < 1e-12);
        assert!((a.centroid - Vec3::new(0.5, 0.5, 0.5)).length() < 1e-12);
        // Ixx about the corner = density * (1/3 + 1/3)
        assert!((a.moments_of_inertia.x - 2.0 * 2.0 / 3.0).abs() < 1e-12);
        assert!((a.volume - b.volume).abs() < 1e-12);
    }

    #[test]
    fn open_mesh_is_not_a_manifold_solid() {
        let k = Kernel::default();
        let mut b = PolyBrep::new(&k, 1e-5);
        let r = b.add_region().unwrap();
        let s = b.add_shell(r).unwrap();
        let vs: Vec<_> = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
            .iter()
            .map(|p| b.add_vertex(&k, Vec3::new(p[0], p[1], p[2]), 1e-5).unwrap())
            .collect();
        let face = b.add_face(&k, s, 1e-5, None).unwrap();
        let lp = b.start_poly_edge_loop(face).unwrap();
        for j in 0..3 {
            let (a, c) = (vs[j], vs[(j + 1) % 3]);
            let (pa, pc) = (b.vertex_point(a).unwrap(), b.vertex_point(c).unwrap());
            b.add_poly_edge(&k, lp, 1e-5, pa, pc, a).unwrap();
        }
        let _ = b.finish_poly_edge_loop(lp);
        assert!(!poly_brep_is_manifold_solid(&b).unwrap());
        assert!(poly_brep_compute_mass_properties(&k, &mut b, 1.0, Point3::ZERO).is_err());
        let p = b
            .compute_properties(&k, Point3::ZERO, MassPropertiesType::Area)
            .unwrap();
        assert!((p.area.abs() - 0.5).abs() < 1e-12);
    }
}
