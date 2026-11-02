//! The transformation functions of the API (`SmApiTransform`, `SmApiScale`,
//! `SmApiScaleByPt`, `SmApiTranslate`, `SmApiRotate`) and their common worker
//! `sm_TransformIndependentObject`, which moves a stand-alone B-rep, poly B-rep, curve
//! or surface in place.

use crate::curve::composite_curve::p1::TYPE as COMPOSITE_CURVE_TYPE;
use crate::curve::crv_in_volume::CrvInVolume;
use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::p2::Curve;
use crate::curve::line::Line;
use crate::curve::projected_curve::ProjectedCurve;
use crate::error::{Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::tol::are_same;
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, EFF_ZERO, PI};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::BSplineSurface;
use crate::query::poly::{PolyBrep, PolyNotifyEnv};
use crate::surface::surface::p2::SRF_IN_VOLUME_TYPE;
use crate::surface::surface::Surface;
use crate::topology::topology::TopologyRecords;

const OP: &str = "topology::proto_topology::p1::transform_independent_object";
const OP_ROTATE: &str = "topology::proto_topology::p1::api_rotate";

/// The object an API transformation acts on (the `SmObject*` of
/// `sm_TransformIndependentObject`, by kind).
pub enum TransformObject<'a> {
    /// An `SmBrep`.
    Brep(&'a mut crate::topology::Brep),
    /// An `SmPolyBrep` (with what its notifications need; see [`PolyBrepTarget`]).
    PolyBrep(&'a mut dyn PolyBrepTransform),
    /// An `SmCurve`.
    Curve(&'a mut Curve),
    /// An `SmSurface`.
    Surface(&'a mut Surface),
    /// Any other object.
    Other,
}

impl core::fmt::Debug for TransformObject<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Brep(_) => f.write_str("Brep"),
            Self::PolyBrep(_) => f.write_str("PolyBrep"),
            Self::Curve(c) => f.debug_tuple("Curve").field(c).finish(),
            Self::Surface(s) => f.debug_tuple("Surface").field(s).finish(),
            Self::Other => f.write_str("Other"),
        }
    }
}

/// A poly B-rep as [`transform_independent_object`] moves it (`SmPolyBrep::Transform`).
pub trait PolyBrepTransform {
    /// `SmPolyBrep::Transform(crRotateNMove, cpOptScale)`.
    ///
    /// # Errors
    ///
    /// Those of [`PolyBrep::transform`].
    fn transform(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
    ) -> Result<()>;
}

/// A [`PolyBrep`] with the environment its edit notifications go to.
pub struct PolyBrepTarget<'a, 'e, R: ?Sized, T> {
    /// The poly B-rep.
    pub brep: &'a mut PolyBrep,
    /// Its notification environment.
    pub env: &'a mut PolyNotifyEnv<'e, R, T>,
}

impl<R: TopologyRecords + ?Sized, T> PolyBrepTransform for PolyBrepTarget<'_, '_, R, T> {
    fn transform(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
    ) -> Result<()> {
        self.brep.transform(k, self.env, rotate_n_move, scale)
    }
}

/// `SM_ERR_INVALID_INPUT` as returned by the API functions.
const fn invalid(reason: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation: OP,
        reason,
    }
}

/// `sm_IsUniformPositiveScale`: equal components (to the scaled zero), positive.
fn is_uniform_positive_scale(scale: Vec3) -> bool {
    are_same(scale.x, scale.y) && are_same(scale.x, scale.z) && scale.x > EFF_ZERO
}

/// `sm_TransformIndependentObject`: scales `obj` about the origin by `scale` (when
/// given), then rotates and moves it by `transform`. Only stand-alone curves and
/// surfaces are accepted, and specialised curve or surface kinds only take
/// positive, uniform scales.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] (`SM_ERR_INVALID_INPUT`) for no object, an undefined
/// or near-zero scale, a non-positive poly B-rep scale, an owned curve or surface, a
/// curve in a volume, on a surface, projected or composite, a surface in a volume,
/// a scale a specialised kind does not take, or any other object; those of
/// [`Curve::transform`], [`Surface::transform`] and [`PolyBrep::transform`];
/// those of `SmBrep::Transform` (`topology::brep::p5::transform`) for B-reps.
pub fn transform_independent_object(
    k: &mut Kernel,
    obj: Option<TransformObject<'_>>,
    transform: &Axis2Placement,
    scale: Option<Vec3>,
) -> Result<()> {
    let Some(obj) = obj else {
        return Err(invalid("no object"));
    };

    if let Some(s) = scale {
        if s.is_undef() || s.min_dimension() <= EFF_ZERO {
            return Err(invalid("scale is undefined or near zero"));
        }
    }

    match obj {
        TransformObject::Brep(brep) => {
            crate::topology::brep::p5::transform(k, brep, transform, scale, true)
        }
        TransformObject::PolyBrep(poly) => {
            // The stable API publishes only positive, non-degenerate PolyBrep scales.
            if let Some(s) = scale {
                if s.x <= EFF_ZERO || s.y <= EFF_ZERO || s.z <= EFF_ZERO {
                    return Err(invalid("poly B-rep scale is not positive"));
                }
            }
            poly.transform(k, transform, scale)
        }
        TransformObject::Curve(curve) => {
            let curve_type = curve.type_code();
            if curve.owner().is_some() {
                return Err(invalid(
                    "Cannot transform a Brep-owned Curve independently; transform the owning \
                     Brep instead.",
                ));
            }

            if curve_type == CrvInVolume::TYPE
                || curve_type == CrvOnSurf::TYPE
                || curve_type == ProjectedCurve::TYPE
                || curve_type == COMPOSITE_CURVE_TYPE
            {
                return Err(invalid("curve kind cannot be transformed independently"));
            }

            // Specialized curve representations receive only positive, uniform
            // scaling through this API.
            if let Some(s) = scale {
                if curve_type != BSplineCurve::TYPE
                    && curve_type != Line::TYPE
                    && !is_uniform_positive_scale(s)
                {
                    return Err(invalid(
                        "Cannot transform this standalone Curve in place with these scale \
                         factors",
                    ));
                }
            }

            curve.transform(k, transform, scale)
        }
        TransformObject::Surface(surface) => {
            if surface.owner().is_some() {
                return Err(invalid(
                    "Cannot transform a Brep-owned Surface independently; transform the \
                     owning Brep instead.",
                ));
            }

            if surface.type_code() == SRF_IN_VOLUME_TYPE {
                return Err(invalid("surface kind cannot be transformed independently"));
            }

            // Specialized surface representations receive only positive, uniform
            // scaling through this API.
            if let Some(s) = scale {
                if surface.type_code() != BSplineSurface::TYPE && !is_uniform_positive_scale(s) {
                    return Err(invalid(
                        "Cannot transform this standalone Surface in place with these scale \
                         factors",
                    ));
                }
            }

            surface.transform(k, transform, scale)
        }
        TransformObject::Other => Err(invalid("object cannot be transformed")),
    }
}

/// Rotates and moves `obj` (`SmApiTransform`).
///
/// # Errors
///
/// Those of [`transform_independent_object`].
pub fn api_transform(
    k: &mut Kernel,
    obj: Option<TransformObject<'_>>,
    rotate_move: &Axis2Placement,
) -> Result<()> {
    transform_independent_object(k, obj, rotate_move, None)
}

/// Scales `obj` about the origin (`SmApiScale`).
///
/// # Errors
///
/// Those of [`transform_independent_object`].
pub fn api_scale(k: &mut Kernel, obj: Option<TransformObject<'_>>, scale: Vec3) -> Result<()> {
    let matrix = Axis2Placement::default();
    transform_independent_object(k, obj, &matrix, Some(scale))
}

/// Scales `obj` about `ref_pt` (`SmApiScaleByPt`): one transformation
/// `x' = scale * x + ref * (1 - scale)`.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for an undefined or near-zero scale; those of
/// [`transform_independent_object`].
pub fn api_scale_by_pt(
    k: &mut Kernel,
    obj: Option<TransformObject<'_>>,
    ref_pt: Point3,
    scale: Vec3,
) -> Result<()> {
    if scale.is_undef() || scale.min_dimension() <= EFF_ZERO {
        return Err(invalid("scale is undefined or near zero"));
    }

    // Transform applies scale before placement. Compose x' = scale*x + ref*(1-scale)
    // into one operation so a rejected scale cannot leave the object partially
    // translated.
    let translate = Vec3::new(
        ref_pt.x * (1.0 - scale.x),
        ref_pt.y * (1.0 - scale.y),
        ref_pt.z * (1.0 - scale.z),
    );
    let mut transform = Axis2Placement::default();
    transform.translate(translate);
    transform_independent_object(k, obj, &transform, Some(scale))
}

/// Moves `obj` by `translate` (`SmApiTranslate`).
///
/// # Errors
///
/// Those of [`transform_independent_object`].
pub fn api_translate(
    k: &mut Kernel,
    obj: Option<TransformObject<'_>>,
    translate: Vec3,
) -> Result<()> {
    let mut transform = Axis2Placement::default();
    transform.translate(translate);
    transform_independent_object(k, obj, &transform, None)
}

/// Rotates `obj` by `angle_deg` degrees about the line through `ref_pt` along `axis`
/// (`SmApiRotate`); the axis is unitized first.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for no object or a zero-length axis; those of
/// [`transform_independent_object`].
pub fn api_rotate(
    k: &mut Kernel,
    obj: Option<TransformObject<'_>>,
    ref_pt: Point3,
    axis: Vec3,
    angle_deg: f64,
) -> Result<()> {
    if obj.is_none() {
        return Err(SolidError::InvalidInput {
            operation: OP_ROTATE,
            reason: "no object",
        });
    }

    // Normalize the rotation axis, failing gracefully on a zero vector.
    let axis = axis.unitize().map_err(|_| SolidError::InvalidInput {
        operation: OP_ROTATE,
        reason: "rotation axis has zero length",
    })?;

    let mut transform = Axis2Placement::default();
    transform.rotate_about_axis_at_point(angle_deg * (PI / 180.0), ref_pt, axis);

    transform_independent_object(k, obj, &transform, None)
}
