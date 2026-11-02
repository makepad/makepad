//! Owned-geometry adapter for offsetting a source face use.
//!
//! Upstream: `source/SMLib/src/SmOffsetExecutive.cpp` (533–567, 1276–1312) and
//! `source/SMLib/src/SmOffsetGeometryCreation.cpp` (`FaceuseOffset`).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `build` (private) | `DoSolidOffset` / `DoExtendedOffset` calls to `FaceuseOffset` |
//! | `build_local` (private) | the same call of a local operation (`SmLocalOperationGeometryCreation::FaceuseOffset`) |
//! | `FaceBrep` (private) | source lookup and owned geometry from `FaceuseOffset` |
//!
//! Deviations: tagged handles separate source topology/geometry from owned results;
//! geometry transfers once into the caller as values. The shared face algorithm,
//! concrete geometry kinds, SSI optional slots, types and deviations are unchanged.
//! No topology is cloned or edited, and no new recursion is introduced.

use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::kernel::Kernel;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::offset::offset_geometry_creation::{
    FaceOffsetBrep, LocalOperationGeometryCreation, OffsetGeometryCreation, SsiData,
};
use crate::surface::surface::Surface;
use crate::topology::{edgeuse::p1 as faceuse_ops, face::p2 as face_ops, Brep, FaceId, FaceuseId};

const OP: &str = "offset::offset_executive::face";

#[derive(Clone, Copy, PartialEq)]
enum Handle {
    SourceFaceuse(FaceuseId),
    SourceFace(FaceId),
    SourceSurface(FaceId),
    NewSurface(usize),
    NewCurve(usize),
}

fn invalid_handle() -> SolidError {
    SolidError::invalid_input(
        OP,
        "face creator handle does not resolve to the requested kind",
    )
}

struct FaceBrep<'a> {
    original: &'a Brep,
    surfaces: Vec<Option<Surface>>,
    curves: Vec<Option<Curve>>,
}

impl FaceBrep<'_> {
    fn take_output(
        mut self,
        made: (Option<Handle>, SsiData<Handle>),
    ) -> Result<(Option<Surface>, SsiData<Curve>)> {
        let surface = match made.0 {
            Some(Handle::NewSurface(i)) => Some(
                self.surfaces
                    .get_mut(i)
                    .and_then(Option::take)
                    .ok_or_else(invalid_handle)?,
            ),
            None => None,
            Some(_) => return Err(invalid_handle()),
        };
        let mut data = SsiData::default();
        for handle in made.1.curves_3d {
            let Handle::NewCurve(i) = handle else {
                return Err(invalid_handle());
            };
            data.curves_3d.push(
                self.curves
                    .get_mut(i)
                    .and_then(Option::take)
                    .ok_or_else(invalid_handle)?,
            );
        }
        for slot in made.1.uv_curves1 {
            data.uv_curves1.push(match slot {
                None => None,
                Some(Handle::NewCurve(i)) => Some(
                    self.curves
                        .get_mut(i)
                        .and_then(Option::take)
                        .ok_or_else(invalid_handle)?,
                ),
                Some(_) => return Err(invalid_handle()),
            });
        }
        for slot in made.1.uv_curves2 {
            data.uv_curves2.push(match slot {
                None => None,
                Some(Handle::NewCurve(i)) => Some(
                    self.curves
                        .get_mut(i)
                        .and_then(Option::take)
                        .ok_or_else(invalid_handle)?,
                ),
                Some(_) => return Err(invalid_handle()),
            });
        }
        data.curve_types = made.1.curve_types;
        data.deviations = made.1.deviations;
        Ok((surface, data))
    }
}

impl FaceOffsetBrep<Handle> for FaceBrep<'_> {
    fn faceuse_orientation(&self, handle: Handle) -> Result<OrientType> {
        let Handle::SourceFaceuse(id) = handle else {
            return Err(invalid_handle());
        };
        faceuse_ops::faceuse_orientation(self.original, id)
    }
    fn faceuse_face(&self, handle: Handle) -> Result<Option<Handle>> {
        let Handle::SourceFaceuse(id) = handle else {
            return Err(invalid_handle());
        };
        Ok(faceuse_ops::faceuse_face(self.original, id)?.map(Handle::SourceFace))
    }
    fn face_surface(&self, handle: Handle) -> Result<Option<Handle>> {
        let Handle::SourceFace(id) = handle else {
            return Err(invalid_handle());
        };
        Ok(face_ops::surface(self.original, id)?.map(|_| Handle::SourceSurface(id)))
    }
    fn surface(&self, handle: Handle) -> Result<&Surface> {
        match handle {
            Handle::SourceSurface(face) => {
                face_ops::surface(self.original, face)?.ok_or_else(invalid_handle)
            }
            Handle::NewSurface(i) => self
                .surfaces
                .get(i)
                .and_then(Option::as_ref)
                .ok_or_else(invalid_handle),
            _ => Err(invalid_handle()),
        }
    }
    fn insert_surface(&mut self, surface: Surface) -> Result<Handle> {
        let i = self.surfaces.len();
        self.surfaces.push(Some(surface));
        Ok(Handle::NewSurface(i))
    }
    fn insert_curve(&mut self, curve: Curve) -> Result<Handle> {
        let i = self.curves.len();
        self.curves.push(Some(curve));
        Ok(Handle::NewCurve(i))
    }
}

/// Builds the offset surface and optional self-intersection curves for a source face use.
pub(super) fn build(
    kernel: &Kernel,
    creator: &OffsetGeometryCreation,
    original: &Brep,
    faceuse: FaceuseId,
    skip_self_intersection: bool,
) -> Result<(Option<Surface>, SsiData<Curve>)> {
    let mut backend = FaceBrep {
        original,
        surfaces: Vec::new(),
        curves: Vec::new(),
    };
    let made = creator.faceuse_offset_with_backend(
        kernel,
        &mut backend,
        Handle::SourceFaceuse(faceuse),
        skip_self_intersection,
    )?;
    backend.take_output(made)
}

/// The surface `SmLocalOperationGeometryCreation::FaceuseOffset` lists for a source face
/// use's face: `surfaces[i]` replaces the surface of `faces[i]`. No self-intersections.
pub(super) fn build_local(
    original: &Brep,
    faces: &[FaceId],
    surfaces: &[Surface],
    faceuse: FaceuseId,
) -> Result<(Option<Surface>, SsiData<Curve>)> {
    let mut backend = FaceBrep {
        original,
        surfaces: surfaces.iter().cloned().map(Some).collect(),
        curves: Vec::new(),
    };
    let creator = LocalOperationGeometryCreation::from_handles(
        0.0,
        0.0,
        faces.iter().map(|&f| Handle::SourceFace(f)).collect(),
        (0..surfaces.len()).map(Handle::NewSurface).collect(),
    );
    let made = creator.faceuse_offset_with_backend(&mut backend, Handle::SourceFaceuse(faceuse))?;
    backend.take_output(made)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::curve::CurveKind;
    use crate::curve::line::Line;
    use crate::geom::attribute::ObjectKey;
    use crate::geom::object::ObjectRef;
    use crate::geom::vector2d::Point2;
    use crate::math::Vec3;
    use crate::nurbs::b_spline_curve::BSplineCurve;
    use crate::nurbs::b_spline_surface::BSplineSurface;
    use crate::surface::plane::Plane;
    use crate::surface::surface::p2::{SurfaceKind, TsectCurveType};
    use crate::topology::Faceuse;

    fn source(surface: Surface, orientation: OrientType) -> (Brep, FaceId, FaceuseId) {
        let mut original = Brep::new();
        let shell = original.add_shell().unwrap();
        let face = original.add_face(shell).unwrap();
        original.face_mut(face).unwrap().surface = Some(surface);
        let faceuse = original
            .add_faceuse_record(Faceuse {
                face: Some(face),
                shell: Some(shell),
                orientation,
                ..Faceuse::default()
            })
            .unwrap();
        (original, face, faceuse)
    }

    fn backend(original: &Brep) -> FaceBrep<'_> {
        FaceBrep {
            original,
            surfaces: Vec::new(),
            curves: Vec::new(),
        }
    }

    fn plane(height: f64) -> Surface {
        Surface::from(Plane::from_normal(Vec3::new(0.0, 0.0, height), Vec3::Z))
    }

    fn line() -> Curve {
        Curve::from(Line::from_points(Vec3::ZERO, Vec3::X, 3))
    }

    fn spline() -> Curve {
        Curve::from(BSplineCurve::create_line_segment(3, Vec3::Y, Vec3::Z).unwrap())
    }

    #[test]
    fn signed_face_offsets_keep_source_geometry_kind_and_owner() {
        let kernel = Kernel::default();
        for orientation in [OrientType::Same, OrientType::Opposite] {
            for distance in [-0.25, 0.0, 0.25] {
                let (mut original, face, faceuse) = source(plane(0.5), orientation);
                *original
                    .face_mut(face)
                    .unwrap()
                    .surface
                    .as_mut()
                    .unwrap()
                    .owner_mut() = Some(ObjectRef {
                    key: ObjectKey(71),
                    type_code: crate::geom::assert_array::labels::FACE_TYPE,
                });
                // the offset reads the source plane's NURB, which upstream makes and
                // keeps on it (GetOrCreateGwNurbPointer): make it before the snapshot
                original
                    .face(face)
                    .unwrap()
                    .surface
                    .as_ref()
                    .unwrap()
                    .natural_uv_domain_with_nurb(&kernel)
                    .unwrap();
                let before = format!("{original:?}");
                let (made, ssi) = build(
                    &kernel,
                    &OffsetGeometryCreation::new(distance, 1e-6),
                    &original,
                    faceuse,
                    true,
                )
                .unwrap();
                let made = made.unwrap();
                let SurfaceKind::Plane(made) = made.kind() else {
                    panic!("an offset plane must retain its concrete kind");
                };
                let sign = if orientation == OrientType::Opposite {
                    -1.0
                } else {
                    1.0
                };
                assert_eq!(
                    made.evaluate_point(Point2::new(0.0, 0.0)).z,
                    0.5 + sign * distance
                );
                assert_eq!(before, format!("{original:?}"));
                assert!(ssi.curves_3d.is_empty());
                assert!(ssi.uv_curves1.is_empty() && ssi.uv_curves2.is_empty());
                assert!(ssi.curve_types.is_empty() && ssi.deviations.is_empty());
            }
        }
    }

    #[test]
    fn analytic_replacement_returns_the_last_owned_surface_not_the_first() {
        let kernel = Kernel::default();
        let warped = BSplineSurface::create_bilinear_surface(
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            Vec3::new(1.0, 1.0, 0.5),
        )
        .unwrap();
        let (original, _, faceuse) = source(Surface::from(warped), OrientType::Same);
        let before = format!("{original:?}");
        let mut backend = backend(&original);
        let made = OffsetGeometryCreation::new(0.125, 1e-6)
            .faceuse_offset_with_backend(
                &kernel,
                &mut backend,
                Handle::SourceFaceuse(faceuse),
                true,
            )
            .unwrap();
        assert_eq!(backend.surfaces.len(), 2);
        assert!(matches!(made.0, Some(Handle::NewSurface(1))));
        let (surface, _) = backend.take_output(made).unwrap();
        let surface = surface.unwrap();
        let SurfaceKind::OffsetSurface(offset) = surface.kind() else {
            panic!("the warped patch must retain its offset-surface representation");
        };
        assert!(!offset.allow_extension());
        assert_eq!(before, format!("{original:?}"));
    }

    #[test]
    fn owned_ssi_transfer_preserves_optional_slots_kinds_and_metadata() {
        // Synthetic store transport evidence, not native self-intersection parity.
        let original = Brep::new();
        let mut backend = backend(&original);
        let _unused = backend.insert_surface(plane(-7.0)).unwrap();
        let surface = backend.insert_surface(plane(9.0)).unwrap();
        let curve_3d = backend.insert_curve(line()).unwrap();
        let other_3d = backend.insert_curve(spline()).unwrap();
        let uv1 = backend.insert_curve(spline()).unwrap();
        let uv2 = backend.insert_curve(line()).unwrap();
        let ssi = SsiData {
            curves_3d: vec![curve_3d, other_3d],
            uv_curves1: vec![Some(uv1), None],
            uv_curves2: vec![None, Some(uv2)],
            curve_types: vec![TsectCurveType::Crossing, TsectCurveType::Touching],
            deviations: vec![-0.0, 1e-12],
        };
        let (surface, ssi) = backend.take_output((Some(surface), ssi)).unwrap();
        let surface = surface.unwrap();
        let SurfaceKind::Plane(surface) = surface.kind() else {
            panic!("stored surface kind changed");
        };
        assert_eq!(surface.evaluate_point(Point2::new(0.0, 0.0)).z, 9.0);
        assert!(matches!(ssi.curves_3d[0].kind(), CurveKind::Line(_)));
        assert!(matches!(ssi.curves_3d[1].kind(), CurveKind::BSpline(_)));
        assert!(matches!(
            ssi.uv_curves1[0].as_ref().unwrap().kind(),
            CurveKind::BSpline(_)
        ));
        assert!(ssi.uv_curves1[1].is_none() && ssi.uv_curves2[0].is_none());
        assert!(matches!(
            ssi.uv_curves2[1].as_ref().unwrap().kind(),
            CurveKind::Line(_)
        ));
        assert_eq!(
            ssi.curve_types,
            [TsectCurveType::Crossing, TsectCurveType::Touching]
        );
        assert_eq!(
            ssi.deviations
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            [(-0.0_f64).to_bits(), 1e-12_f64.to_bits()]
        );
    }

    #[test]
    fn rejects_repeated_owned_curve_handles_even_across_ssi_arrays() {
        for across_arrays in [false, true] {
            let original = Brep::new();
            let mut backend = backend(&original);
            let curve = backend.insert_curve(line()).unwrap();
            let ssi = if across_arrays {
                SsiData {
                    curves_3d: vec![curve],
                    uv_curves1: vec![Some(curve)],
                    ..SsiData::default()
                }
            } else {
                SsiData {
                    curves_3d: vec![curve, curve],
                    ..SsiData::default()
                }
            };
            assert!(backend.take_output((None, ssi)).is_err());
        }
    }

    #[test]
    fn rejects_invalid_owned_slots_and_borrowed_source_outputs() {
        let (original, face, faceuse) = source(plane(0.0), OrientType::Same);
        for surface in [Handle::NewSurface(usize::MAX), Handle::SourceSurface(face)] {
            assert!(backend(&original)
                .take_output((Some(surface), SsiData::default()))
                .is_err());
        }
        let ssi = SsiData {
            curves_3d: vec![Handle::NewCurve(usize::MAX)],
            ..SsiData::default()
        };
        assert!(backend(&original).take_output((None, ssi)).is_err());
        assert!(backend(&original)
            .surface(Handle::SourceFaceuse(faceuse))
            .is_err());
    }

    #[test]
    fn null_surface_and_empty_ssi_transfer_without_inventing_geometry() {
        let original = Brep::new();
        let (surface, ssi) = backend(&original)
            .take_output((None, SsiData::default()))
            .unwrap();
        assert!(surface.is_none() && ssi.curves_3d.is_empty());
    }

    /// source issue #1072 (WR-464): the collapse threshold of the cylinder,
    /// cone, sphere and torus. Past it the offset has no surface and the flag does not
    /// turn that into a failure; short of it the offset is a surface.
    #[test]
    fn collapse_thresholds_of_the_analytic_surfaces() {
        use crate::geom::axis2_placement::Axis2Placement;
        use crate::surface::cone::Cone;
        use crate::surface::sphere::Sphere;
        use crate::surface::torus::{Torus, TorusOptions};

        let k0 = Kernel::default();
        let frame = Axis2Placement::IDENTITY;
        let cone = Cone::create_canonical(&k0, &frame, 2.0, 1.0, 3.0, false, false).unwrap();
        let cone_cos = (cone.semi_angle_deg() * std::f64::consts::PI / 180.0).cos();
        let cylinder = Cone::create_canonical_cylinder(&k0, &frame, 1.5).unwrap();
        let sphere = Sphere::create_canonical(&k0, &frame, 2.0).unwrap();
        let torus =
            Torus::create_canonical(&k0, &frame, 4.0, 0.5, TorusOptions::default(), None).unwrap();
        // (surface, distance just short of the collapse, distance past it)
        let cases = [
            (Surface::from(cylinder), -1.4, -1.6),
            (Surface::from(cone), -0.9 / cone_cos, -2.1 / cone_cos),
            (Surface::from(sphere), -1.9, -2.1),
            (Surface::from(torus), -0.4, -0.6),
        ];
        for fixes in [false, true] {
            let mut kernel = Kernel::default();
            kernel.fixes.offset_missing_surface_fails = fixes;
            for (surface, short, past) in &cases {
                let (original, _, faceuse) = source(surface.clone(), OrientType::Same);
                for (d, collapses) in [(*short, false), (*past, true)] {
                    assert_eq!(
                        surface.offset_collapses(&kernel, d, 1e-7).unwrap(),
                        collapses
                    );
                    let mut backend = backend(&original);
                    let made = OffsetGeometryCreation::new(d, 1e-6)
                        .faceuse_offset_with_backend(
                            &kernel,
                            &mut backend,
                            Handle::SourceFaceuse(faceuse),
                            true,
                        )
                        .unwrap();
                    assert_eq!(made.0.is_none(), collapses, "{d}");
                }
            }
        }
    }

    /// A surface whose offset is always one never collapses.
    #[test]
    fn offset_collapses_is_false_for_a_plane() {
        let kernel = Kernel::default();
        assert!(!plane(0.0).offset_collapses(&kernel, -1e9, 1e-7).unwrap());
    }
}
