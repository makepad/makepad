//! Static assert-report label tables (`SmAssertReportLabel` arrays) and the object
//! type codes [`assert_label_list`](super::assert_label_list) dispatches on.
//!
//! Upstream: the `extern SmAssertReportLabel s*_list[]` declarations of
//! `source/SMLib/inc/SmAssertArray.h`, defined next to each `AssertValid` method in
//! `source/SMLib/src/Sm*.cpp` (file named on each table), and the `Sm*_TYPE` codes of
//! `SmTypes.h`, `SmCurveTypes.h`, `SmSurfaceTypes.h`, `SmTopoTypes.h` and friends.
//!
//! Generated mechanically from upstream (entry order, categories and strings verbatim;
//! upstream lists no fourth initializer, so every `message_for_file` is `None`).

use super::{AssertReportLabel, AssertType};

/// Shorthand for a table entry.
const fn l(
    assert_type: AssertType,
    name: &'static str,
    message: Option<&'static str>,
) -> AssertReportLabel {
    AssertReportLabel {
        assert_type,
        name,
        message,
        message_for_file: None,
    }
}

// ---- object type codes (`SM_TYPE`) -------------------------------------------

/// `SmAxis2Placement_TYPE`.
pub const AXIS2_PLACEMENT_TYPE: i64 = 12082;
/// `SmPolarConversion_TYPE`.
pub const POLAR_CONVERSION_TYPE: i64 = 12084;
/// `SmCircle_TYPE`.
pub const CIRCLE_TYPE: i64 = 14021;
/// `SmEllipse_TYPE`.
pub const ELLIPSE_TYPE: i64 = 14022;
/// `SmAttribute_TYPE`.
pub const ATTRIBUTE_TYPE: i64 = 12070;
/// `SmBSplineCurve_TYPE`.
pub const B_SPLINE_CURVE_TYPE: i64 = 14033;
/// `SmCurve_TYPE`.
pub const CURVE_TYPE: i64 = 14001;
/// `SmAObject_TYPE`.
pub const A_OBJECT_TYPE: i64 = 12060;
/// `SmLine_TYPE`.
pub const LINE_TYPE: i64 = 14010;
/// `SmTree_TYPE`.
pub const TREE_TYPE: i64 = 12150;
/// `SmHashTable_TYPE`.
pub const HASH_TABLE_TYPE: i64 = 12152;
/// `SmObjsInVoxels_TYPE`.
pub const OBJS_IN_VOXELS_TYPE: i64 = 12153;
/// `SmSolutionArray_TYPE`.
pub const SOLUTION_ARRAY_TYPE: i64 = 12020;
/// `SmBSplineSurface_TYPE`.
pub const B_SPLINE_SURFACE_TYPE: i64 = 15002;
/// `SmPlane_TYPE`.
pub const PLANE_TYPE: i64 = 15011;
/// `SmCone_TYPE`.
pub const CONE_TYPE: i64 = 15012;
/// `SmSphere_TYPE`.
pub const SPHERE_TYPE: i64 = 15014;
/// `SmCylinder_TYPE`.
pub const CYLINDER_TYPE: i64 = 15013;
/// `SmTorus_TYPE`.
pub const TORUS_TYPE: i64 = 15015;
/// `SmSurfOfRevolution_TYPE`.
pub const SURF_OF_REVOLUTION_TYPE: i64 = 15020;
/// `SmExtent1d_TYPE`.
pub const EXTENT1D_TYPE: i64 = 12088;
/// `SmExtent2d_TYPE`.
pub const EXTENT2D_TYPE: i64 = 12090;
/// `SmExtent3d_TYPE`.
pub const EXTENT3D_TYPE: i64 = 12092;
/// `SmPeriodicExtent1d_TYPE`.
pub const PERIODIC_EXTENT1D_TYPE: i64 = 12096;
/// `SmPolarBox_TYPE`.
pub const POLAR_BOX_TYPE: i64 = 12098;
/// `SmPseudoBox_TYPE`.
pub const PSEUDO_BOX_TYPE: i64 = 12102;
/// `SmDerivSurfDefinition_TYPE`.
pub const DERIV_SURF_DEFINITION_TYPE: i64 = 12104;
/// `SmSurfaceCache_TYPE`.
pub const SURFACE_CACHE_TYPE: i64 = 15100;
/// `SmSurfOfExtrusion_TYPE`.
pub const SURF_OF_EXTRUSION_TYPE: i64 = 15030;
/// `SmSurface_TYPE`.
pub const SURFACE_TYPE: i64 = 15001;
/// `SmCrvOnSurf_TYPE`.
pub const CRV_ON_SURF_TYPE: i64 = 14048;
/// `SmProjectedCurve_TYPE`.
pub const PROJECTED_CURVE_TYPE: i64 = 14037;
/// `SmFilletEdge_TYPE`.
pub const FILLET_EDGE_TYPE: i64 = 16033;
/// `SmFilletBrep_TYPE`.
pub const FILLET_BREP_TYPE: i64 = 12094;
/// `SmFilletVertex_TYPE`.
pub const FILLET_VERTEX_TYPE: i64 = 16034;
/// `SmFilletVertexuse_TYPE`.
pub const FILLET_VERTEXUSE_TYPE: i64 = 16032;
/// `SmFilletEdgeuse_TYPE`.
pub const FILLET_EDGEUSE_TYPE: i64 = 16031;
/// `SmSpaceBend_TYPE`.
pub const SPACE_BEND_TYPE: i64 = 12316;
/// `SmSpaceUnbend_TYPE`.
pub const SPACE_UNBEND_TYPE: i64 = 12318;
/// `SmOffsetSurface_TYPE`.
pub const OFFSET_SURFACE_TYPE: i64 = 15400;
/// `SmOffsetCurve_TYPE`.
pub const OFFSET_CURVE_TYPE: i64 = 14036;
/// `SmAssembly_TYPE`.
pub const ASSEMBLY_TYPE: i64 = 16055;
/// `SmAssemblyInstance_TYPE`.
pub const ASSEMBLY_INSTANCE_TYPE: i64 = 16056;
/// `SmSAGObject_TYPE`.
pub const SAG_OBJECT_TYPE: i64 = 16050;
/// `SmBrep_TYPE`.
pub const BREP_TYPE: i64 = 16011;
/// `SmEdge_TYPE`.
pub const EDGE_TYPE: i64 = 16016;
/// `SmFace_TYPE`.
pub const FACE_TYPE: i64 = 16002;
/// `SmCurveClassification_TYPE`.
pub const CURVE_CLASSIFICATION_TYPE: i64 = 12312;
/// `SmCurveInterval_TYPE`.
pub const CURVE_INTERVAL_TYPE: i64 = 12313;
/// `SmPointClassification_TYPE`.
pub const POINT_CLASSIFICATION_TYPE: i64 = 12308;
/// `SmTopology_TYPE`.
pub const TOPOLOGY_TYPE: i64 = 16001;
/// `SmOwningTopology_TYPE`.
pub const OWNING_TOPOLOGY_TYPE: i64 = 16010;
/// `SmEdgeuse_TYPE`.
pub const EDGEUSE_TYPE: i64 = 16005;
/// `SmFaceuse_TYPE`.
pub const FACEUSE_TYPE: i64 = 16022;
/// `SmVertexuse_TYPE`.
pub const VERTEXUSE_TYPE: i64 = 16006;
/// `SmVertex_TYPE`.
pub const VERTEX_TYPE: i64 = 16021;
/// `SmLoop_TYPE`.
pub const LOOP_TYPE: i64 = 16003;
/// `SmLoopuse_TYPE`.
pub const LOOPUSE_TYPE: i64 = 16004;
/// `SmRegion_TYPE`.
pub const REGION_TYPE: i64 = 16012;
/// `SmShell_TYPE`.
pub const SHELL_TYPE: i64 = 16013;
/// `SmPolyFace_TYPE`.
pub const POLY_FACE_TYPE: i64 = 18005;
/// `SmCPolyFace_TYPE`.
pub const C_POLY_FACE_TYPE: i64 = 18004;
/// `SmPolyLoop_TYPE`.
pub const POLY_LOOP_TYPE: i64 = 18006;
/// `SmPolyVertex_TYPE`.
pub const POLY_VERTEX_TYPE: i64 = 18008;
/// `SmPolyEdge_TYPE`.
pub const POLY_EDGE_TYPE: i64 = 18007;
/// `SmPolyRegion_TYPE`.
pub const POLY_REGION_TYPE: i64 = 18002;
/// `SmPolyShell_TYPE`.
pub const POLY_SHELL_TYPE: i64 = 18003;
/// `SmPolyBrep_TYPE`.
pub const POLY_BREP_TYPE: i64 = 18001;
/// `SmTrimmingTools_TYPE`.
pub const TRIMMING_TOOLS_TYPE: i64 = 12225;
/// `SmCrvInVolume_TYPE`.
pub const CRV_IN_VOLUME_TYPE: i64 = 14053;
/// `SmSrfInVolume_TYPE`.
pub const SRF_IN_VOLUME_TYPE: i64 = 15218;
/// `SmVolume_TYPE`.
pub const VOLUME_TYPE: i64 = 22001;
/// `SmBSplineVolume_TYPE`.
pub const B_SPLINE_VOLUME_TYPE: i64 = 22004;
/// `SmTransform_TYPE`.
pub const TRANSFORM_TYPE: i64 = 22008;
/// `SmBendVolume_TYPE`.
pub const BEND_VOLUME_TYPE: i64 = 22006;
/// `SmTwistVolume_TYPE`.
pub const TWIST_VOLUME_TYPE: i64 = 22011;
/// `SmUnbendVolume_TYPE`.
pub const UNBEND_VOLUME_TYPE: i64 = 22010;

// ---- label tables ------------------------------------------------------------

/// `sAssertDefault_list` (`source/SMLib/src/SmAssertArray.cpp`).
pub const DEFAULT: &[AssertReportLabel] = &[l(
    AssertType::Unknown,
    "Unknown_Assert",
    Some("Accessed an Unknown AssertReport_list - error"),
)];

/// `sAssertAxis2Placement_list` (`source/SMLib/src/SmAxis2Placement.cpp`).
pub const AXIS2_PLACEMENT: &[AssertReportLabel] = &[
    l(
        AssertType::Angle,
        "Ortho Vectors",
        Some("m_vXAxis and m_vYAxis are orthogonal"),
    ),
    l(
        AssertType::UnitVector,
        "Unit X Vector",
        Some("m_vXAxis is a unit vector"),
    ),
    l(
        AssertType::UnitVector,
        "Unit Y Vector",
        Some("m_vYAxis is a unit vector"),
    ),
];

/// `sAssertPolarConversion_list` (`source/SMLib/src/SmEllipse.cpp`).
pub const POLAR_CONVERSION: &[AssertReportLabel] = &[
    l(
        AssertType::Flag,
        "Flags",
        Some("m_bPolarConversionPossible must be false when m_cpCurve is NULL"),
    ),
    l(
        AssertType::Cache,
        "Nested Call",
        Some("SmPolarConversion::IsCurrent() failed"),
    ),
];

/// `sAssertPolarIsCurrent_list` (`source/SMLib/src/SmEllipse.cpp`).
pub const POLAR_IS_CURRENT: &[AssertReportLabel] = &[
    l(
        AssertType::Knots,
        "Knots",
        Some("Number of sNaturalKnots != number of m_vNaturalKnots"),
    ),
    l(
        AssertType::Knots,
        "Angles",
        Some("sNaturalKnot values != m_vNaturalKnot values"),
    ),
    l(
        AssertType::Angle,
        "Angles",
        Some("Computed Angles must match Anlges in storage"),
    ),
    l(
        AssertType::Angle,
        "Angles",
        Some("Stored Angles must increase or decrease monotonically"),
    ),
];

/// `sAssertCircle_list` (`source/SMLib/src/SmCircle.cpp`).
pub const CIRCLE: &[AssertReportLabel] = &[l(
    AssertType::Radius,
    "Radius",
    Some("m_dRadiusAtXAxis and m_dRadiusAtYAxis are equal"),
)];

/// `sAssertEllipse_list` (`source/SMLib/src/SmEllipse.cpp`).
pub const ELLIPSE: &[AssertReportLabel] = &[
    l(
        AssertType::Flag,
        "bad m_bInsideOut flag value",
        Some("m_bInsideOut can only be TRUE for circles"),
    ),
    l(
        AssertType::Points,
        "bad STEP to Nurb Approx",
        Some("Dist(EvalSTEP(STEPT), EvalNurb(Convert(STEPT)) greater than tol"),
    ),
    l(
        AssertType::Points,
        "Bad Nurb to STEP Approx",
        Some("Dist(NurbPt, EvalSTEP(STEPInversion(NurbPt)) greater than tol"),
    ),
];

/// `sAssertAttribute_list` (`source/SMLib/src/SmAttribute.cpp`).
pub const ATTRIBUTE: &[AssertReportLabel] = &[l(
    AssertType::Pointer,
    "Context",
    Some("m_vUsers share the same context"),
)];

/// `sAssertBSplineCurve_list` (`source/SMLib/src/SmBSplineCurve.cpp`).
pub const B_SPLINE_CURVE: &[AssertReportLabel] = &[
    l(AssertType::Pointer, "Bad Null Ptr", Some("BSplineCurve m_pNurb ptr is NULL")),
    l(AssertType::Knots, "Bad Knot Count", Some("BSplineCurve Knot Vector Start and End knots do not have degree+1 multiplicity")),
    l(AssertType::Knots, "Bad Knot Count", Some("BSplineCurve Knot Vector Internal knot has multiplicity greater than degree")),
    l(AssertType::Geometric, "Warn Coincident CPts", Some("BSplineCurve's 1st two control points are coincident - not an error by itself, but commonly part of a problem.")),
    l(AssertType::Geometric, "Warn Coincident CPts", Some("BSplineCurve's last two control points are coincident - not an error by itself, but copmmonly part of a problem.")),
    l(AssertType::Geometric, "Bad SpeedPt", Some("2D BSplineCurve Warning: zero-length derivative at interior multiple knot")),
    l(AssertType::Geometric, "Bad SpeedPt", Some("3D BSplineCurve Warning: zero-length derivative at interior multiple knot")),
    l(AssertType::Geometric, "Bad CurveKink", Some("2D BSplineCurve Warning: Kink at interior knot")),
    l(AssertType::Geometric, "Bad CurveKink", Some("3D BSplineCurve Warning: Kink at interior knot")),
    l(AssertType::Geometric, "Bad CurveCusp", Some("2D BSplineCurve ERROR: Curve kinks back on itself at interior knot")),
    l(AssertType::Geometric, "Bad CurveCusp", Some("3D BSplineCurve ERROR: Curve kinks back on itself at interior knot")),
    l(AssertType::Geometric, "Bad SpeedPt", Some("2D BSplineCurve Warning: Interior 1st Deriv too slow, less than AvgSpeed/10000 (often an almost kink or almost singular point - never good practice)")),
    l(AssertType::Geometric, "Bad SpeedPt", Some("3D BSplineCurve Warning: Interior 1st Deriv too slow, less than AvgSpeed/10000 (often an almost kink or almost singular point - never good practice)")),
    l(AssertType::Geometric, "Bad CurveCusp", Some("2D BSplineCurve Warning: Extremely small radius of curvature (often an almost kink point - sometimes what's desired)")),
    l(AssertType::Geometric, "Bad CurveCusp", Some("3D BSplineCurve Warning: Extremely small radius of curvature (often an almost kink point - sometimes what's desired)")),
    l(AssertType::Geometric, "Bad Repeated CPts", Some("BSplineCurve has Internal Coincident ControlPoints (causes internal point singularity)")),
    l(AssertType::Knots, "Bad BSpline Rep", Some("Number of knots inconsistent with number of ControlPoints and Curve degree")),
];

/// `sAssertCurve_list` (`source/SMLib/src/SmCurve.cpp`).
pub const CURVE: &[AssertReportLabel] = &[l(
    AssertType::Type,
    "Type",
    Some("m_pOwner is correct type"),
)];

/// `sAssertAObject_list` (`source/SMLib/src/SmAObject.cpp`).
pub const A_OBJECT: &[AssertReportLabel] = &[
    l(
        AssertType::Type,
        "Data Type",
        Some("m_pAttributes is NULL or not the correct type"),
    ),
    l(
        AssertType::Pointer,
        "Context",
        Some("pAttrib shares the same context"),
    ),
    l(
        AssertType::Pointer,
        "Context",
        Some("pAttrib shares the same context"),
    ),
];

/// `sAssertLine_list` (`source/SMLib/src/SmLine.cpp`).
pub const LINE: &[AssertReportLabel] = &[
    l(
        AssertType::Points,
        "End Points",
        Some("sSTEPStartPoint closer than sNurbEndPoint"),
    ),
    l(
        AssertType::Points,
        "End Points",
        Some("sSTEPEndPoint closer than sNurbStartPoint"),
    ),
    l(
        AssertType::Points,
        "End Points",
        Some("sSTEPStartPoint closer than sNurbStartPoint"),
    ),
    l(
        AssertType::Points,
        "End Points",
        Some("sSTEPEndPoint closer than sNurbEndPoint"),
    ),
];

/// `sAssertTree_list` (`source/SMLib/src/SmTree.cpp`).
pub const TREE: &[AssertReportLabel] = &[
    l(
        AssertType::Box,
        "Bad TreeNode BBox3d size",
        Some("TreeNode BBox3d is zero or negative for NonDegenerate Curve/Surface"),
    ),
    l(
        AssertType::Box,
        "Bad TreeNode BBox3d containment",
        Some("TreeNode BBox3d not contained by Parent BBox3d"),
    ),
    l(
        AssertType::Box,
        "Bad TreeNode Child1 BBox3d",
        Some("Tree ParentNode BBox3d does not contain pChild1 BBox3d"),
    ),
    l(
        AssertType::Box,
        "Bad TreeNode Child2 BBox3d",
        Some("Tree ParentNode BBox3d does not contain pChild2 BBox3d"),
    ),
    l(
        AssertType::Domain,
        "Bad TreeNode UVDomain containment",
        Some("TreeNode UVDomain not contained by Parent UVdomain"),
    ),
    l(
        AssertType::Domain,
        "Bad TreeNode Child1 UVDomain",
        Some("TreeNode UVDomain does not contain pChild1 UVDomain"),
    ),
    l(
        AssertType::Domain,
        "Bad TreeNode Child2 UVDomain",
        Some("TreeNode UVDomain does not contain pChild2 UVDomain"),
    ),
    l(
        AssertType::Domain,
        "Bad TreeNode Bezier UVDomain containment",
        Some("TreeNode Bezier UVDomain not contained by Parent Bezier UVdomain"),
    ),
    l(
        AssertType::Domain,
        "Bad TreeNode Child1 Bezier UVDomain",
        Some("TreeNode Bezier UVDomain does not contain pChild1 Bezier UVDomain"),
    ),
    l(
        AssertType::Box,
        "Bad TreeNode Child2 Bezier UVDomain",
        Some("TreeNode Bezier UVDomain does not contain pChild2 Bezier UVDomain"),
    ),
    l(
        AssertType::Box,
        "Bad TreeNode Pseudo Box Size",
        Some("TreeNode PseudoBox3d is zero or negative for NonDegenerate Curve/Surface"),
    ),
    l(
        AssertType::Box,
        "Bad TreeNode Pseudo Box containment",
        Some("TreeNode PseudoBox3d Volume is greater than Parent PseudoBox3d Volume"),
    ),
    l(
        AssertType::Box,
        "Bad TreeNode Child1 Pseudo Box",
        Some("TreeNode PseudoBox3d Volume is less than pChild1 PseudoBox3d Volume"),
    ),
    l(
        AssertType::Box,
        "Bad TreeNode Child2 Pseudo Box",
        Some("TreeNode PseudoBox3d Volume is less than pChild2 PseudoBox3d Volume"),
    ),
    l(
        AssertType::Topological,
        "Bad TreeVertices",
        Some("Bad SubdivisionTree Vertex Connectivity Graph"),
    ),
    l(
        AssertType::Topological,
        "Bad Tree LeafNode Start Index",
        Some("Tree LeafNode has bad StartIndex Value"),
    ),
];

/// `sAssertHashTable_list` (`source/SMLib/src/SmHashTable.cpp`).
pub const HASH_TABLE: &[AssertReportLabel] = &[
    l(AssertType::Cache, "Bad HashTable BucketItem[ii]->BucketIndex", Some("HashTable BucketItem BucketIndex points to an unused Bucket - should be in bucket and bucket should be used")),
    l(AssertType::Cache, "Bad HashTable Total BucketItem count", Some("HashTable Total BucketItem Count is less than Removed BucketItem Count - Should be Total = Used + Removed BucketItem counts")),
    l(AssertType::Cache, "Bad HashTable Total, Removed, Used BucketItem counts", Some("HashTable Total, Used, and Removed BucketItem Counts are bad - Should be Total = Used + Removed BucketItem counts")),
    l(AssertType::Cache, "Bad HashTable Used BucketItem counts", Some("HashTable Counted-Used != StoredUsed BucketItem counts - Should be equal")),
];

/// `sAssertObjsInVoxels_list` (`source/SMLib/src/SmHashTable.cpp`).
pub const OBJS_IN_VOXELS: &[AssertReportLabel] = &[
    l(AssertType::Cache, "Bad ObjsInVoxels contained m_sHashTable", Some("ObjsInVoxels contained m_sHashTable failed its m_sHashTable->AssertValid() call")),
    l(AssertType::Cache, "Bad conflicting ObjsInVoxels Voxel and bucket counts", Some("ObjsInVoxels voxel count not equal to HashTable bucket count - should be equal")),
    l(AssertType::Cache, "Bad conflicting ObjsInVoxels PointItems and BucketItems counts", Some("ObjsInVoxels point items count not == HashTable total BucketItems count - should be less than or equal")),
    l(AssertType::Cache, "Bad conflicting BucketItem and PointItem iith elements", Some("ObjsInVoxels m_sHashTable.BucketItem[ii]->m_sItem != associated m_sHashTable.m_sPointItems[ii] - should be equal")),
    l(AssertType::Cache, "Bad ObjsInVoxels m_sPointItems[ii] Bad VoxelIndx", Some("ObjsInVoxels m_sHashTable->BucketItem[ii]->BucketIndex is not within 1 voxel of the VoxelIndex computed from m_sPointItems[ii]->m_sTgtPoint3d")),
    l(AssertType::Cache, "Bad ObjsInVoxels m_sHashTable.m_sBucketItems[ii] Bad VoxelIndx", Some("ObjsInVoxels m_sHashTable.m_sBuckets[m_sHashTable.m_sBucketItems[ii].BucketIndex] does not contain BucketItem[ii]")),
    l(AssertType::Cache, "Bad Conflicting BucketItem back m_sItem and m_sItemId values", Some("ObjsInVoxels &m_sPointItems[m_sHashTable.m_sBucektItems[ii].m_sItemId] != m_sHashTable.m_sBucketItems[ii].m_sItem - they should be the same.")),
    l(AssertType::Cache, "Bad ObjsInVoxels m_sHashTable.m_sBucketItems[ii].m_sItemId value", Some("ObjsInVoxels m_sHashTable.m_sBucketItems[ii].m_sItemId index value is not contained in range:[0, m_sPointItems.GetSize()]")),
];

/// `sAssertSolutionArray_list` (`source/SMLib/src/SmSolutionArray.cpp`).
pub const SOLUTION_ARRAY: &[AssertReportLabel] = &[
    l(
        AssertType::Size,
        "Bad SIZE",
        Some("m_pData is NULL and m_lSize is zero - should be greater than zero"),
    ),
    l(
        AssertType::Size,
        "Bad SIZE",
        Some("m_pData is NULL and m_lMaxSize is zero - should be greater than zero"),
    ),
    l(
        AssertType::Size,
        "Bad SIZE",
        Some(
            "m_pData is NotNULL and m_lSize greater than m_lMaxSize - should be less than or equal",
        ),
    ),
];

/// `sAssertBSplineSurface_list` (`source/SMLib/src/SmBSplineSurface.cpp`).
pub const B_SPLINE_SURFACE: &[AssertReportLabel] = &[
    l(AssertType::Pointer, "Bad NULL Ptr", Some("SmBSplineSurface m_pNurb ptr is NULL")),
    l(AssertType::Parameterization, "Bad Knot Multiplicity", Some("SmBSplineSurface Knot multiplicity greater than surface degree")),
    l(AssertType::Degenerate, "Bad Internal Pole", Some("SmBSplineSurface has an Internal pole caused by combinations of coincident CPts")),
    l(AssertType::Degenerate, "Bad Degnerate CPt Row", Some("SmBSplineSurface has an internal row or column of all coincident control points")),
    l(AssertType::Degenerate, "Warn: Repeated End Cpts", Some("SmBSplineSurface has a repeated end CPt row. Causes bdry cross-tangents to degenerate to zero - warning")),
    l(AssertType::Geometric, "Bad NonMonotonic CPts", Some("SmBSplineSurface Control Net doubles back on itself probably making a self intersecting surface")),
];

/// `sAssertPlane_list` (`source/SMLib/src/SmPlane.cpp`).
pub const PLANE: &[AssertReportLabel] = &[
    l(
        AssertType::Parameterization,
        "Domain X",
        Some("m_vAnalUVDomain should have length in X"),
    ),
    l(
        AssertType::Parameterization,
        "Domain Y",
        Some("m_vAnalUVDomain should have length in Y"),
    ),
    l(
        AssertType::Parameterization,
        "Equal Domain",
        Some("sNaturalUVDomain should be equal to m_vAnalUVDomain"),
    ),
    l(
        AssertType::Scale,
        "Scale",
        Some("m_vUVScale should be positive in X"),
    ),
    l(
        AssertType::Scale,
        "Scale",
        Some("m_vUVScale should be positive in Y"),
    ),
    l(
        AssertType::Geometric,
        "Equal Shapes",
        Some("Analytic and Nurb shapes should be equal"),
    ),
];

/// `sAssertCone_list` (`source/SMLib/src/SmCone.cpp`).
pub const CONE: &[AssertReportLabel] = &[
    l(AssertType::Radius, "Bad Cone Radius", Some("TopRadius or BotRadius is negative. Both must be >= 0.0 to ensure the apex is not within the domain)")),
    l(AssertType::Parameterization, "Bad Cone Parameterization", Some("GenCurve not the same as the Surface Parameterization in GenCurve direction")),
    l(AssertType::Parameterization, "Bad Cone Parameterization", Some("make sure cone is parameterized in height of cone axis ")),
    l(AssertType::Angle, "Bad Cone Sweep Direction", Some("Nurb sweep direction is not CCW about STEP Z axis ")),
    l(AssertType::Type, "Bad Cone GenCurve type", Some("GenCurve is not a SmLine type object. It must be. ")),
    l(AssertType::Distance, "Bad Cone GenCurve LineScale", Some("GenCurve Line Scale != 1/Cos(m_dSemiAngleRad). It must, to parameterize GenCurve to the cone's height.")),
    l(AssertType::Geometric, "Bad Cone Pole", Some("Cone apex (where radius == 0.0) is within the Cone's AnalIvlV placing a pole on the surface interior.")),
];

/// `sAssertSphere_list` (`source/SMLib/src/SmSphere.cpp`).
pub const SPHERE: &[AssertReportLabel] = &[
    l(AssertType::Radius, "Positive Radius", Some("m_dRadius is positive")),
    l(AssertType::Parameterization, "Parameterization", Some("GenCurve has same parameterization as surface")),
    l(AssertType::Geometric, "Overlapping Domain", Some("Analytic U Direction Domain too large (more than 360 deg): spheres can't overlap themselves")),
    l(AssertType::Geometric, "Overlapping Domain", Some("Analytic V Direction Domain too large (more than 180 deg): spheres can't run through poles")),
    l(AssertType::Geometric, "Included Singularity", Some("Analytic V Direction Domain places v=-90 pole within domain interior")),
    l(AssertType::Geometric, "Included Singularity", Some("Analytic V Direction Domain places v=+90 pole within domain interior")),
];

/// `sAssertCylinder_list` (`source/SMLib/src/SmCylinder.cpp`).
pub const CYLINDER: &[AssertReportLabel] = &[
    l(
        AssertType::Radius,
        "Zero SemiAngle",
        Some("Cones must have zero SemiAngles"),
    ),
    l(
        AssertType::Geometric,
        "Overlapping Domain",
        Some("Analytic U Direction Domain too large: cylinders can't overlap themselves"),
    ),
    l(
        AssertType::Geometric,
        "Domain Max",
        Some("Analytic U Direction Max Domain larger than +360"),
    ),
    l(
        AssertType::Geometric,
        "Domain Min",
        Some("Analytic U Direction Min Domain less than -360"),
    ),
];

/// `sAssertTorus_list` (`source/SMLib/src/SmTorus.cpp`).
pub const TORUS: &[AssertReportLabel] = &[
    l(
        AssertType::Radius,
        "Major Radius",
        Some("m_dMajorRadius is greater than zero"),
    ),
    l(
        AssertType::Radius,
        "Minor Radius",
        Some("m_dMinorRadius is greater than zero"),
    ),
    l(
        AssertType::Parameterization,
        "Same Parameterization",
        Some("Curve and Surface have same paramterization"),
    ),
    l(
        AssertType::Geometric,
        "Overlapping Domain",
        Some("Analytic U Direction Domain too large: Tori can't overlap themselves"),
    ),
    l(
        AssertType::Geometric,
        "Overlapping Domain",
        Some("Analytic V Direction Domain too large: Tori can't overlap themselves"),
    ),
];

/// Not upstream (WR-539, the fork's source fork@daba678): [`TORUS`] with the
/// fork's test 5, "Self Intersection". Only the torus check with
/// `Kernel::fixes.torus_check_spindle_apex` reports from it; `SM_GetAssertLabelList`
/// ([`label_list`]) keeps upstream's [`TORUS`].
pub const TORUS_WITH_SPINDLE_APEX: &[AssertReportLabel] = &[
    l(
        AssertType::Radius,
        "Major Radius",
        Some("m_dMajorRadius is greater than zero"),
    ),
    l(
        AssertType::Radius,
        "Minor Radius",
        Some("m_dMinorRadius is greater than zero"),
    ),
    l(
        AssertType::Parameterization,
        "Same Parameterization",
        Some("Curve and Surface have same paramterization"),
    ),
    l(
        AssertType::Geometric,
        "Overlapping Domain",
        Some("Analytic U Direction Domain too large: Tori can't overlap themselves"),
    ),
    l(
        AssertType::Geometric,
        "Overlapping Domain",
        Some("Analytic V Direction Domain too large: Tori can't overlap themselves"),
    ),
    l(
        AssertType::Geometric,
        "Self Intersection",
        Some("Spindle torus: Analytic V Direction Domain runs through an apex on the axis"),
    ),
];

/// `sAssertSurfOfRevolution_list` (`source/SMLib/src/SmSurfOfRevolution.cpp`).
pub const SURF_OF_REVOLUTION: &[AssertReportLabel] = &[
    l(AssertType::Pointer, "Context", Some("Bad m_pGenCurve does not share same context")),
    l(AssertType::Pointer, "Context", Some("Bad m_vPolarConverter does not share same context")),
    l(AssertType::Parameterization, "Parameterization", Some("Bad GenCurve Anal and NURB intervals don't match appropriate Surface Domain Intervals")),
    l(AssertType::Geometric, "Midpoint", Some("Bad Analytic and NURB GenCurve midPoint values are not the same")),
    l(AssertType::Size, "Size", Some("Bad PolarConverter lAngCount is not greater than one")),
    l(AssertType::Angle, "Start Angle", Some("Bad PolarConverter First polar angle does not equal start SweepInterval bounds")),
    l(AssertType::Angle, "End Angle", Some("Bad PolarConverter Last polar angle does not equal end SweepInterval bounds")),
    l(AssertType::Parameterization, "Parameterization", Some("Bad PolarCurve Anal and NURB intervals does not match appropriate Surface Domain Intervals")),
    l(AssertType::Geometric, "Sweep midpoint", Some("Bad Analytic and NURB Evaluate Sweep near midpoint values not the same")),
    l(AssertType::Distance, "Dist V", Some("Bad GenCurve eval does not map to Surface eval within dScaledZero")),
    l(AssertType::Distance, "Dist U", Some("Bad GenCurve eval does not map to Surface eval within dScaledZero")),
    l(AssertType::Domain, "InsideOut GenCurve", Some("Bad GenCurve and Surface bInsideOut bit values don't match")),
    l(AssertType::Domain, "InsideOut PolarCurve", Some("Bad PolarCurve bInsideOut bit values is not FALSE")),
    l(AssertType::Geometric, "Overlapping Domain", Some("Bad Analytic U Direction Domain too large: SurfOfRevolution can't overlap themselves")),
    l(AssertType::Pointer, "Bad GenCurve Owner", Some("Bad GenCurve owner is not this SmSurfOfRevolution object")),
    l(AssertType::Size, "Size", Some("Bad GenCurve length degenerates SurfOfRevolution to an approximate circular curve")),
];

/// `sAssertExtent1d_list` (`source/SMLib/src/SmExtent1d.cpp`).
pub const EXTENT1D: &[AssertReportLabel] =
    &[l(AssertType::Minmax, "Min Max", Some("m_dMin <= m_dMax"))];

/// `sAssertExtent2d_list` (`source/SMLib/src/SmExtent2d.cpp`).
pub const EXTENT2D: &[AssertReportLabel] =
    &[l(AssertType::Minmax, "Min Max", Some("m_vMin <= m_vMax"))];

/// `sAssertExtent3d_list` (`source/SMLib/src/SmExtent3d.cpp`).
pub const EXTENT3D: &[AssertReportLabel] =
    &[l(AssertType::Minmax, "Min Max", Some("m_vMin <= m_vMax"))];

/// `sAssertPeriodicExtent1d_list` (`source/SMLib/src/SmPeriodicExtent1d.cpp`).
pub const PERIODIC_EXTENT1D: &[AssertReportLabel] = &[
    l(
        AssertType::Values,
        "Bad Period",
        Some("m_dPeriod must be greater 0.0 and it's not"),
    ),
    l(
        AssertType::Values,
        "Bad Min Boundary",
        Some("m_dMin must be in range:[0 m_dPeriod] and it's not"),
    ),
    l(
        AssertType::Values,
        "Bad Max Boundary",
        Some("m_dMax must be in range:[0 m_dPeriod] and it's not"),
    ),
];

/// `sAssertPolarBox_list` (`source/SMLib/src/SmPolarBox.cpp`).
pub const POLAR_BOX: &[AssertReportLabel] = &[
    l(AssertType::Box, "Transform", Some("Transform is ok")),
    l(
        AssertType::Box,
        "Box State",
        Some("m_ePolarBoxState initialized"),
    ),
];

/// `sAssertPseudoBox_list` (`source/SMLib/src/SmPseudoBox.cpp`).
pub const PSEUDO_BOX: &[AssertReportLabel] = &[
    l(
        AssertType::Minmax,
        "Min Max",
        Some("m_aIntervals Min <= Max"),
    ),
    l(
        AssertType::UnitVector,
        "UnitVector",
        Some("m_aBasis vectors must be unit length"),
    ),
];

/// `sAssertDerivSurfDefinition_list` (`source/SMLib/src/SmSurface.cpp`).
pub const DERIV_SURF_DEFINITION: &[AssertReportLabel] = &[
    l(AssertType::Size, "Sample Points", Some("m_lNumSamples is greater than zero")),
    l(AssertType::Angle, "Sweep", Some("End values are between -360 and 360")),
    l(AssertType::Pointer, "NULL", Some("m_pTangentReference is nonNull when m_eTangencyType == SM_TT_G1_FROM_CURVE or SM_TT_DIR_TO_REFERENCE_CURVE")),
    l(AssertType::Size, "Cross-Tangent", Some("m_dStartTanLength and m_dEndTanScale are zero or positive")),
    l(AssertType::Angle, "Rotation", Some("m_dStartAngleDeg and m_dEndAngleDeg are between -360 and 360")),
    l(AssertType::Vector, "Vector", Some("m_vVector is initialized")),
    l(AssertType::Type, "Curvature Type", Some("Requested CurvatureType matches BaseCurve or SurfaceBoundaryCurve construction choice")),
    l(AssertType::Size, "Radius of Curvature", Some("m_dStartRadius and m_dEndRadScale are zero or positive")),
    l(AssertType::Pointer, "NULL", Some("m_pCurvatureReference is non-NULL when m_eCurvatureType == SM_CT_G2_FROM_CURVE")),
];

/// `sAssertSurfaceCache_list` (`source/SMLib/src/SmSurfaceCache.cpp`).
pub const SURFACE_CACHE: &[AssertReportLabel] = &[
    l(AssertType::Size, "Bad InsideTreeNode", Some("INSIDE classified SurfaceCacheNode has NonEmpty Vertex or Curve list. Should be empty.")),
    l(AssertType::Size, "Bad OutsideTreeNode", Some("OUTSIDE classified SurfaceCacheNode has NonEmpty Vertex or Curve list. Should be empty.")),
    l(AssertType::Size, "Bad BoundaryTreeNode", Some("BOUNDARY classified SurfaceCacheNode has empty Vertex or Curve list. Should be NonEmpty.")),
    l(AssertType::Flag, "Bad TreeNode", Some("Node remains unclassified (in/on/out) after FaceContainment is Done")),
    l(AssertType::Size, "Bad Assoc Arrays sizes", Some("m_pUContinuities list size not equal to Surface Unique u Knot list size")),
    l(AssertType::Size, "Bad Assoc Arrays sizes", Some("m_pVContinuities list size not equal to Surface Unique u Knot list size")),
];

/// `sAssertSurfOfExtrusion_list` (`source/SMLib/src/SmSurfOfExtrusion.cpp`).
pub const SURF_OF_EXTRUSION: &[AssertReportLabel] = &[
    l(AssertType::Vector, "Vector", Some("m_vExtrusionVec must be initialized")),
    l(AssertType::Vector, "Vector Length", Some("m_vExtrusionVec must be non-zero")),
    l(AssertType::Domain, "Analytic X", Some("m_vAnalUVDomain XLength must be non-zero")),
    l(AssertType::Domain, "Analytic Y", Some("m_vAnalUVDomain YLength must be non-zero")),
    l(AssertType::Domain, "Domain Range", Some("Analytic and Nurb domains must share same range")),
    l(AssertType::Distance, "Distance between", Some("STEP domain point converted to Nurb and back to STEP domain must be the same domain point")),
    l(AssertType::Distance, "Distance between", Some("equivalent STEP and Nurb domain point evaluations must yield same image point values")),
    l(AssertType::Domain, "Analytic Interval", Some("m_vAnalUVDomain->UInterval must be the same as the m_pGenCurve->STEPInterval")),
    l(AssertType::Geometric, "Origin", Some("m_vOrigin is a point on m_pGenCurve")),
    l(AssertType::Distance, "Planar", Some("m_vCurvePoint is on plane")),
    l(AssertType::Angle, "Planar", Some("m_vCurveVec is plane normal")),
    l(AssertType::Distance, "Linear", Some("m_vCurvePoint is on line")),
    l(AssertType::Angle, "Linear", Some("m_vCurveVec is tangent")),
    l(AssertType::Geometric, "Linear", Some("bLinear && bPlanar")),
    l(AssertType::Geometric, "Linear", Some("Unique sweep")),
    l(AssertType::Geometric, "Perpendicular", Some("!bLinear && bPlanar")),
    l(AssertType::Geometric, "Perpendicular", Some("Orthogonal sweep")),
    l(AssertType::Geometric, "Independent", Some("!bLinear && bPlanar")),
    l(AssertType::Geometric, "Independent", Some("NonPlanar sweep")),
    l(AssertType::Geometric, "Coplanar", Some("!bLinear && bPlanar")),
    l(AssertType::Geometric, "Coplanar", Some("Is coplanar")),
    l(AssertType::Geometric, "Coplanar", Some("GenCurve points of tangency with the extrusion direction")),
    l(AssertType::Geometric, "NOT_Planar", Some("!bLinear && !bPlanar")),
    l(AssertType::Geometric, "NOT_Planar", Some("GenCurve points of tangency with the extrusion direction")),
    l(AssertType::Domain, "InsideOut GenCurve", Some("GenCurve bInsideOut bit value should be FALSE")),
    l(AssertType::Domain, "GenCurveDomain", Some("GenCurve Anal and NURB intervals must match appropriate Surface Domain Intervals")),
    l(AssertType::Pointer, "Bad GenCurve Owner", Some("GenCurve owner must be this SmSurfOfExtrusion object")),
    l(AssertType::Pointer, "Bad GenCurve Context", Some("GenCurve context must be this SmSurfOfExtrusion context")),
];

/// `sAssertSurface_list` (`source/SMLib/src/SmSurface.cpp`).
pub const SURFACE: &[AssertReportLabel] = &[
    l(
        AssertType::Type,
        "Bad Surf Owner",
        Some("Surface->Owner not oneof: NULL, Face, or Geometry with a GenSurface"),
    ),
    l(
        AssertType::Geometric,
        "Bad Surf FlatCorner",
        Some("Surface Su and Sv Tangents in at least one corner are parallel"),
    ),
    l(
        AssertType::Geometric,
        "Bad Surf Degenerated to Point",
        Some("Surface has degenerated into a single point"),
    ),
    l(
        AssertType::Geometric,
        "Bad Surf Degenerates to Curve",
        Some("Surface has degenerated into a single curve"),
    ),
    l(
        AssertType::Geometric,
        "Bad Surf Discontinuity",
        Some("Surface sampled IsoLine contains a G1 or C0 discontinuity across a Knot Value"),
    ),
    l(
        AssertType::Geometric,
        "Bad Surf Pole Approximation",
        Some("Surface is close to singular at boundary - should be tightened to exactly singular"),
    ),
    l(
        AssertType::Coincidence,
        "Bad Surf Coincidence at Seam",
        Some("Open surface has partially coincident boundaries"),
    ),
];

/// `sAssertCrvOnSurf_list` (`source/SMLib/src/SmCrvOnSurf.cpp`).
pub const CRV_ON_SURF: &[AssertReportLabel] = &[
    l(
        AssertType::NestedTest,
        "Base Curve",
        Some("Base Curve must pass AssertValid"),
    ),
    l(
        AssertType::NestedTest,
        "Compounding Surface",
        Some("Compounding Surface must pass AssertValid"),
    ),
    l(
        AssertType::Geometric,
        "Contained Base Curve",
        Some("Base Curve must map to domain of the surface"),
    ),
    l(
        AssertType::Pointer,
        "Bad UVCurve Owner",
        Some("When owned by CrvOnSurf, ContainedUVCurve->owner must be this SmCrvOnSurf object"),
    ),
    l(
        AssertType::Pointer,
        "Bad Surface Owner",
        Some("When owned by CrvOnSurf, ContainedSurface->owner must be this SmCrvOnSurf object"),
    ),
    l(
        AssertType::Pointer,
        "Bad UVCurve Context",
        Some(
            "When owned by CrvOnSurf, ContainedUVCurve->context must be this SmCrvOnSurf->context",
        ),
    ),
    l(
        AssertType::Pointer,
        "Bad Surface Context",
        Some(
            "When owned by CrvOnSurf, ContainedSurface->context must be this SmCrvOnSurf->context",
        ),
    ),
];

/// `sAssertProjectedCurve_list` (`source/SMLib/src/SmProjectedCurve.cpp`).
pub const PROJECTED_CURVE: &[AssertReportLabel] = &[
    l(
        AssertType::Values,
        "Uninitialized Object",
        Some("Projection Type is uninitialized to unknown"),
    ),
    l(
        AssertType::NestedTest,
        "Base Curve",
        Some("Base Curve must pass AssertValid"),
    ),
    l(
        AssertType::Vector,
        "uninit vector",
        Some("vectors defining projection must be initialized"),
    ),
    l(
        AssertType::Vector,
        "zero vector",
        Some("vectors defining projection must be nonZero"),
    ),
    l(
        AssertType::Vector,
        "unit vector",
        Some("vectors must be unit-length"),
    ),
    l(
        AssertType::Pointer,
        "Base Curve Context",
        Some("Base Curve context must be this->m_pContext"),
    ),
];

/// `sAssertFilletEdge_list` (`source/SMLib/src/SmFilletGeom.cpp`).
pub const FILLET_EDGE: &[AssertReportLabel] = &[l(
    AssertType::Unknown,
    "Unknown",
    Some("Not yet implemented"),
)];

/// `sAssertFilletBrep_list` (`source/SMLib/src/SmFilletGeom.cpp`).
pub const FILLET_BREP: &[AssertReportLabel] = &[l(
    AssertType::Unknown,
    "UNKNOWN",
    Some("Not yet implemented"),
)];

/// `sAssertFilletVertex_list` (`source/SMLib/src/SmFilletGeom.cpp`).
pub const FILLET_VERTEX: &[AssertReportLabel] = &[
    l(
        AssertType::Pointer,
        "Context",
        Some("m_pMate[0] shares the same context"),
    ),
    l(
        AssertType::Pointer,
        "Context",
        Some("m_pMate[1] shares the same context"),
    ),
    l(
        AssertType::Pointer,
        "Context",
        Some("m_cpCorner shares the same context"),
    ),
];

/// `sAssertFilletVertexuse_list` (`source/SMLib/src/SmFilletGeom.cpp`).
pub const FILLET_VERTEXUSE: &[AssertReportLabel] = &[l(
    AssertType::Unknown,
    "UNKNOWN",
    Some("Not yet implemented"),
)];

/// `sAssertFilletEdgeuse_list` (`source/SMLib/src/SmFilletGeom.cpp`).
pub const FILLET_EDGEUSE: &[AssertReportLabel] = &[l(
    AssertType::Unknown,
    "UNKNOWN",
    Some("Not yet implemented"),
)];

/// `sAssertBend_list` (`source/SMLib/src/SmSpaceBend.cpp`).
pub const BEND: &[AssertReportLabel] = &[
    l(
        AssertType::Pointer,
        "NULL Pointer",
        Some("Target Brep must be nonNull"),
    ),
    l(
        AssertType::Values,
        "BendAngle Range",
        Some("BendAngle must be in the range -180 to + 180 degrees"),
    ),
    l(
        AssertType::Values,
        "Neg BendRadius",
        Some("The rule: NeutralRadius >= KFactor * SheetThickness must be TRUE"),
    ),
    l(
        AssertType::UnitVector,
        "Unit Vector",
        Some("BendAxis vector must be unit length"),
    ),
    l(
        AssertType::UnitVector,
        "Unit Vector",
        Some("BendDirection vector must be unit length"),
    ),
    l(
        AssertType::Vector,
        "Parallel Vectors",
        Some("BendAxis and BendDirection can't be parallel"),
    ),
    l(
        AssertType::Values,
        "KFactor Range",
        Some("KFactor must be in the range 0.0 to 1.0"),
    ),
];

/// `sAssertUnbend_list` (`source/SMLib/src/SmSpaceUnbend.cpp`).
pub const UNBEND: &[AssertReportLabel] = &[
    l(
        AssertType::Pointer,
        "NULL Pointer",
        Some("Target Brep must be nonNull"),
    ),
    l(
        AssertType::Values,
        "UnbendAngle Range",
        Some("UnbendAngle must be in the range -180 to + 180 degrees"),
    ),
    l(
        AssertType::Values,
        "Neg UnbendRadius",
        Some("The rule: NeutralRadius >= KFactor * SheetThickness must be TRUE"),
    ),
    l(
        AssertType::UnitVector,
        "Unit Vector",
        Some("UnbendAxis vector must be unit length"),
    ),
    l(
        AssertType::UnitVector,
        "Unit Vector",
        Some("UnbendDirection vector must be unit length"),
    ),
    l(
        AssertType::Vector,
        "Parallel Vectors",
        Some("UnbendAxis and UnbendDirection can't be parallel"),
    ),
    l(
        AssertType::Values,
        "KFactor Range",
        Some("KFactor must be in the range 0.0 to 1.0"),
    ),
];

/// `sAssertOffsetSurface_list` (`source/SMLib/src/SmOffsetSurface.cpp`).
pub const OFFSET_SURFACE: &[AssertReportLabel] = &[
    l(
        AssertType::Pointer,
        "Context",
        Some("m_pSurface shares the same context"),
    ),
    l(
        AssertType::Pointer,
        "Bad Surface Owner",
        Some("Base Surface owner must be this SmOffsetSurface object"),
    ),
    l(
        AssertType::Degenerate,
        "Intersecting Surface",
        Some("Offset Surface has self-intersecting interior"),
    ),
];

/// `sAssertOffsetCurve_list` (`source/SMLib/src/SmOffsetCurve.cpp`).
pub const OFFSET_CURVE: &[AssertReportLabel] = &[
    l(
        AssertType::Pointer,
        "Context",
        Some("m_pSurface shares the same context"),
    ),
    l(
        AssertType::Pointer,
        "Bad Curve Owner",
        Some("Base Curve owner must be this SmOffsetCurve object"),
    ),
];

/// `sAssertAssembly_list` (`source/SMLib/src/SmAssembly.cpp`).
pub const ASSEMBLY: &[AssertReportLabel] = &[l(
    AssertType::Pointer,
    "Context",
    Some("m_vInstances share the same context"),
)];

/// `sAssertAssemblyInstance_list` (`source/SMLib/src/SmAssemblyInstance.cpp`).
pub const ASSEMBLY_INSTANCE: &[AssertReportLabel] = &[l(
    AssertType::Pointer,
    "Context",
    Some("m_pAssembly shares the same context"),
)];

/// `sAssertSAGObject_list` (`source/SMLib/src/SmSAGObject.cpp`).
pub const SAG_OBJECT: &[AssertReportLabel] = &[l(
    AssertType::Unknown,
    "UNKNOWN",
    Some("Not Implemented Yet"),
)];

/// `sAssertBrep_list` (`source/SMLib/src/SmBrep.cpp`).
pub const BREP: &[AssertReportLabel] = &[
    l(AssertType::Pointer, "Bad Context", Some("m_pInfiniteRegion Context not the same as Brep Context")),
    l(AssertType::Pointer, "Bad Context", Some("m_pFaceListHead Context not the same as Brep Context")),
    l(AssertType::Pointer, "Bad Context", Some("m_pEdgeListHead Context not the same as Brep Context")),
    l(AssertType::Pointer, "Bad Context", Some("m_pCFaceListHead Context not the same as Brep Contextt")),
    l(AssertType::Pointer, "Bad Context", Some("m_pCEdgeListHead Context not the same as Brep Context")),
    l(AssertType::Pointer, "Bad Context", Some("m_pVertexListHead Context not the same as Brep Context")),
    l(AssertType::Pointer, "Bad Context", Some("m_pDeletedSurfaces Context not the same as Brep Context")),
    l(AssertType::Size, "Bad Face Count", Some("MultiRegion Breps must have some faces")),
    l(AssertType::Inside, "Bad Infinite Region", Some("m_pInfiniteRegion is incorrect")),
    l(AssertType::Pointer, "Bad Infinite Region", Some("m_pInfiniteRegion must be first in the region list")),
    l(AssertType::Type, "Bad Outer Shell Type", Some("OuterShell not a faceuse shell, must be")),
    l(AssertType::Closed, "Bad Outer Shell closed", Some("OuterShell not closed, must be ")),
    l(AssertType::Inside, "Bad Inner shells", Some("NonInfiniteRegion Inner shell is not geometrically inside its containing outerShell")),
    l(AssertType::Pointer, "Bad Context", Some("Brep Shell Context not equal to Brep Context")),
    l(AssertType::Pointer, "Bad Context", Some("Brep Face Context not equal to Brep Context")),
    l(AssertType::Coincidence, "Bad Coincidence", Some("Brep contains Coincident Geometry")),
    l(AssertType::Size, "Bad Region/Shell Counts", Some("multiRegion Brep has an empty region with no shells, must have at least 1")),
    l(AssertType::Size, "Bad InfiniteRegion/Shell Counts", Some("multiRegion Brep Infinite region has no closed inner shell, must have at least 1")),
    l(AssertType::Size, "Region Count", Some("Brep region count not equal a topology traversal region count, must be")),
    l(AssertType::Pointer, "Region Connectivity", Some("Regions found by topology traversal should be the same as the Brep's region list")),
    l(AssertType::NestedTest, "Bad SubTopology", Some("A Brep SubTopology Region, Shell, Face, Loop, Edge, or Vertex object failed its AssertValid checks")),
    l(AssertType::Topological, "Suspicious Shell Count", Some("Warning MultiFace Brep has one shell for every face - it's likely this Brep should be sent to Stitch()")),
    l(AssertType::Topological, "Bad Shell Count", Some("Brep with only lamina edges does not have as many shells as faces")),
    l(AssertType::Geometric, "Bad Brep Tol", Some("stored Brep ZoneTol not equal the SmTol::Consistent Tolerance Model value")),
];

/// `sAssertValidatePointers_list` (`source/SMLib/src/SmBrep.cpp`).
pub const VALIDATE_POINTERS: &[AssertReportLabel] = &[
    l(
        AssertType::Values,
        "Tolerance",
        Some("Tolerance less than effective zero"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Wire edge pointer is Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Wire edgeuse pointer is Null"),
    ),
    l(
        AssertType::Closed,
        "Closed",
        Some("SmTrimmingTools::CheckFace reported some error"),
    ),
    l(
        AssertType::Pointer,
        "Context",
        Some("Face and brep share the same context"),
    ),
    l(
        AssertType::Values,
        "Tolerance",
        Some("Face tolerance relative to brep tolerance"),
    ),
    l(
        AssertType::Values,
        "Tolerance",
        Some("Face tolerance relative to brep tolerance"),
    ),
    l(
        AssertType::Pointer,
        "Surface",
        Some("Face has underlying surface"),
    ),
    l(
        AssertType::Pointer,
        "Context",
        Some("Surface and brep share the same context"),
    ),
    l(AssertType::Pointer, "Owner", Some("Surface has owner")),
    l(
        AssertType::Pointer,
        "Valid",
        Some("Surface passes validity check"),
    ),
    l(
        AssertType::Domain,
        "Domain",
        Some("Face conatined within surface domain"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Face has a primary faceuse"),
    ),
    l(
        AssertType::Pointer,
        "Context",
        Some("Faceuse and brep share same context"),
    ),
    l(
        AssertType::Direction,
        "Orient",
        Some("Faceuse is SM_OT_SAME "),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Faceuse mate is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Context",
        Some("Faceuse mate and brep share same context"),
    ),
    l(
        AssertType::Direction,
        "Orient",
        Some("Mated faceuses share same orientation"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Faceuse backpointer to brep"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Faceuse points to face"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Faceuse mate points to face"),
    ),
    l(AssertType::Pointer, "Pointer", Some("Faceuse mates agree")),
    l(AssertType::Pointer, "Pointer", Some("Faceuse mates agree")),
    l(AssertType::Pointer, "Null", Some("Shell is non-Null")),
    l(AssertType::Pointer, "Null", Some("Shell is non-Null")),
    l(
        AssertType::Pointer,
        "Context",
        Some("Shell and brep share same context"),
    ),
    l(
        AssertType::Pointer,
        "Context",
        Some("Shell and brep share same context"),
    ),
    l(AssertType::Type, "Type", Some("Shell is faceuse shell")),
    l(AssertType::Type, "Type", Some("Shell is faceuse shell")),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Loop points back to face"),
    ),
    l(AssertType::Pointer, "Null", Some("Loopuse is non-Null")),
    l(AssertType::Pointer, "Null", Some("Loopuse is non-Null")),
    l(
        AssertType::Pointer,
        "Context",
        Some("Loop and brep share same context"),
    ),
    l(
        AssertType::Pointer,
        "Context",
        Some("Loopuse and brep share same context"),
    ),
    l(
        AssertType::Pointer,
        "Context",
        Some("Loopuse and brep share same context"),
    ),
    l(
        AssertType::Direction,
        "Orient",
        Some("Outer loopuse is SM_OT_SAME"),
    ),
    l(
        AssertType::Direction,
        "Orient",
        Some("Outer loopuse is SM_OT_SAME"),
    ),
    l(
        AssertType::Type,
        "Type",
        Some("Outer loopuse is vertex loopuse"),
    ),
    l(
        AssertType::Type,
        "Type",
        Some("Outer loopuse is vertex loopuse"),
    ),
    l(
        AssertType::Direction,
        "Orient",
        Some("Inner loopuse is SM_OT_OPPOSITE"),
    ),
    l(
        AssertType::Direction,
        "Orient",
        Some("Inner loopuse is SM_OT_OPPOSITE"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Loopuse points back to loop"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Loopuse points back to loop"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Loopuse mate points back to loop"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Loopuse mate points back to loop"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Starting edgeuse is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Context",
        Some("Edgeuse and brep share same context"),
    ),
    l(
        AssertType::Points,
        "Point",
        Some("Edgeuse and edge share same end point"),
    ),
    l(
        AssertType::Type,
        "Type",
        Some("EdgeuseType is a SmLoopuse_type"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("m_pSorLU points back to Loopuse"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Edgeuse CW points back to Edgeuse"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Starting edgeuse is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Context",
        Some("Edgeuse and brep share same context"),
    ),
    l(
        AssertType::Points,
        "Point",
        Some("Edgeuse and edge share same end point"),
    ),
    l(
        AssertType::Type,
        "Type",
        Some("EdgeuseType is a SmLoopuse_type"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("m_pSorLU points back to Loopuse"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Edgeuse CW points back to Edgeuse"),
    ),
    l(
        AssertType::Pointer,
        "Context",
        Some("Edge and brep share same context"),
    ),
    l(
        AssertType::Direction,
        "Orient",
        Some("Edge orientations are correct"),
    ),
    l(
        AssertType::Values,
        "Tolerance",
        Some("Edge tolerance relative to brep tolerance"),
    ),
    l(
        AssertType::Values,
        "Tolerance",
        Some("Edge tolerance relative to brep tolerance"),
    ),
    l(
        AssertType::Pointer,
        "Valid",
        Some("Bad Edge fails pE->ValidateGeometry(FALSE) check"),
    ),
    l(
        AssertType::Size,
        "Size",
        Some("Edgeuse has even number of mates"),
    ),
    l(
        AssertType::Direction,
        "Orient",
        Some("Edgeuse orientation opposite to mate"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Edgeuse points back to edge"),
    ),
    l(AssertType::Type, "Type", Some("Shell is vertex shell")),
    l(AssertType::Pointer, "Null", Some("Edgeuse is non-Null")),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Edgeuse points back to edgeuse"),
    ),
    l(AssertType::Pointer, "Null", Some("Edgeuse is non-Null")),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Edgeuse points back to edgeuse"),
    ),
    l(
        AssertType::Points,
        "Point",
        Some("Edge endpoints within face domain"),
    ),
    l(
        AssertType::Points,
        "Point",
        Some("Edge endpoints within face domain"),
    ),
    l(
        AssertType::Points,
        "Point",
        Some("Edge endpoints within face domain"),
    ),
    l(AssertType::Pointer, "Null", Some("Vertexuse is non-Null")),
    l(
        AssertType::Type,
        "Type",
        Some("Vertexuse is SmEdgeuse_TYPE) "),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Vertexuse points back to edgeuse) "),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Radial partner points back to edgeuse) "),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Mate partner points back to edgeuse) "),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Loopuse and mate share loop"),
    ),
    l(AssertType::Pointer, "Null", Some("Faceuse is non-Null")),
    l(
        AssertType::Pointer,
        "Null",
        Some("Faceuse mate is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("faceuse and mate share same face"),
    ),
    l(
        AssertType::Values,
        "Tolerance",
        Some("Vertex tolerance relative to brep tolerance"),
    ),
    l(
        AssertType::Values,
        "Tolerance",
        Some("Vertex tolerance relative to brep tolerance"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Vertexuse points back to vertex) "),
    ),
    l(AssertType::Pointer, "Null", Some("Shell is non-Null")),
    l(
        AssertType::Type,
        "Type",
        Some("Shell type is SmVertexuse_TYPE) ) "),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Shell points back to vertexuse"),
    ),
    l(AssertType::Pointer, "Null", Some("Loopuse is non-Null")),
    l(
        AssertType::Type,
        "Type",
        Some("Loopuse type is SmVertexuse_TYPE) ) "),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Loopuse points back to vertexuse"),
    ),
    l(AssertType::Pointer, "Null", Some("Edgeuse is non-Null")),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Edgeuse points back to vertexuse"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Edgeuse edge is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Shell region points back to region"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Face shell points back to shell"),
    ),
    l(
        AssertType::Size,
        "Size",
        Some("Composite edges has at least two edges"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Radial edgeuse is next edgeuse"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Edgeuses share same region"),
    ),
    l(
        AssertType::Direction,
        "Orient",
        Some("Edgeuse orientation opposite to pair"),
    ),
    l(
        AssertType::Direction,
        "Orient",
        Some("Edgeuse orientation same to pair"),
    ),
    l(
        AssertType::Direction,
        "Orient",
        Some("Edgeuse orientation the same"),
    ),
    l(
        AssertType::Direction,
        "Orient",
        Some("Loopuse orientation the same"),
    ),
    l(
        AssertType::Direction,
        "Orient",
        Some("Loopuse orientation opposite"),
    ),
    l(
        AssertType::Direction,
        "Orient",
        Some("Loopuse orientation the same"),
    ),
    l(
        AssertType::Direction,
        "Orient",
        Some("Loopuse orientation opposite"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Primary loopuse on primary faceuse"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Faceuse shell contains faceuse"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Faceuse shell contains faceuse"),
    ),
    l(AssertType::Pointer, "Null", Some("Loopuse is non-Null")),
    l(AssertType::Pointer, "Null", Some("Loopuse is non-Null")),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Primary loopuse edgeuses are mated to the mate loopuse edgeuses"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Brep context is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Brep infinite region is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Brep face list head is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Brep edge list head is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Brep vertex list head is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Region owner is non-Null"),
    ),
    l(AssertType::Pointer, "Null", Some("Shell owner is non-Null")),
    l(
        AssertType::Pointer,
        "Null",
        Some("Shell owned-use list is non-Null"),
    ),
    l(AssertType::Pointer, "Null", Some("Face owner is non-Null")),
    l(
        AssertType::Pointer,
        "Null",
        Some("Face surface is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Face primary faceuse is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Faceuse face is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Faceuse mate is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Faceuse shell owner is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Faceuse first loopuse is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Loop primary loopuse is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Loopuse loop is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Loopuse mate is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Loopuse faceuse owner is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Loopuse start edgeuse or vertexuse is non-Null"),
    ),
    l(AssertType::Pointer, "Null", Some("Edge owner is non-Null")),
    l(AssertType::Pointer, "Null", Some("Edge curve is non-Null")),
    l(
        AssertType::Pointer,
        "Null",
        Some("Edge primary edgeuse is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Edgeuse owning edge is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Edgeuse vertexuse is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Edgeuse shell or loopuse is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Loop edgeuse CCW neighbor is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Loop edgeuse CW neighbor is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Edgeuse next radial-list neighbor is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Edgeuse last radial-list neighbor is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Vertex owner is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Vertex first vertexuse is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Vertexuse owning vertex is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("Vertexuse shell, loopuse, or edgeuse is non-Null"),
    ),
    l(
        AssertType::Pointer,
        "Pointer",
        Some("Loopuse edgeuse CCW walk closed"),
    ),
];

/// `sAssertValidateTolerances_list` (`source/SMLib/src/SmBrep.cpp`).
pub const VALIDATE_TOLERANCES: &[AssertReportLabel] = &[
    l(
        AssertType::Geometric,
        "Bad Gap",
        Some("Edge/Vertex gap across Edgeuse start larger than tol"),
    ),
    l(
        AssertType::Geometric,
        "Bad Gap",
        Some("Vertex/Face gap across Edgeuse start larger than tol"),
    ),
    l(
        AssertType::Geometric,
        "Bad Gap",
        Some("Edge/Vertex gap across Edgeuse end larger than tol"),
    ),
    l(
        AssertType::Geometric,
        "Bad Gap",
        Some("Vertex/Face gap across Edgeuse end larger than tol"),
    ),
    l(
        AssertType::Geometric,
        "Bad Gap",
        Some("Edge/Surface gap across Edgeuse larger than tol"),
    ),
    l(
        AssertType::Geometric,
        "Bad Gap",
        Some("Edge/UVTrimCurve gap across Edgeuse larger than tol"),
    ),
    l(
        AssertType::Geometric,
        "Bad Gap",
        Some("UVTrimCurve/Vertex gap across Edgeuse start larger than tol"),
    ),
    l(
        AssertType::Geometric,
        "Bad Gap",
        Some("UVTrimCurve/Vertex gap across Edgeuse end larger than tol"),
    ),
];

/// `sAssertCoincidentTopology_list` (`source/SMLib/src/SmBrep.cpp`).
pub const COINCIDENT_TOPOLOGY: &[AssertReportLabel] = &[
    l(
        AssertType::Geometric,
        "Bad Coincident Vertex",
        Some("Vertex pair is coincident, it's likely this Brep should be sent to Stitch()"),
    ),
    l(
        AssertType::Geometric,
        "Bad Coincident   Edge",
        Some("Edge   pair is coincident, it's likely this Brep should be sent to Stitch()"),
    ),
    l(
        AssertType::Geometric,
        "Bad Coincident   Face",
        Some("Face   pair is coincident, it's likely this Brep should be sent to Stitch()"),
    ),
    l(
        AssertType::Geometric,
        "Bad Vertex/Edge Connection",
        Some(
            "Vertex not connected to nearby Edge, it's likely this Brep should be sent to Stitch()",
        ),
    ),
    l(
        AssertType::Geometric,
        "Bad Vertex/Face Connection",
        Some(
            "Vertex not connected to nearby Face, it's likely this Brep should be sent to Stitch()",
        ),
    ),
    l(
        AssertType::Geometric,
        "Bad Edge/Face   Connection",
        Some(
            "Edge   not connected to nearby Face, it's likely this Brep should be sent to Stitch()",
        ),
    ),
];

/// `sAssertSubTopology_list` (`source/SMLib/src/SmBrep.cpp`).
pub const SUB_TOPOLOGY: &[AssertReportLabel] = &[
    l(
        AssertType::Pointer,
        "Bad Region",
        Some("Region failed AssertValid during AssertSubTopology call"),
    ),
    l(
        AssertType::Pointer,
        "Bad Shell",
        Some("Shell  failed AssertValid during AssertSubTopology call"),
    ),
    l(
        AssertType::Pointer,
        "Bad Face",
        Some("Face   failed AssertValid during AssertSubTopology call"),
    ),
    l(
        AssertType::Pointer,
        "Bad CFace",
        Some("CFace  failed AssertValid during AssertSubTopology call"),
    ),
    l(
        AssertType::Pointer,
        "Bad Loop",
        Some("Loop   failed AssertValid during AssertSubTopology call"),
    ),
    l(
        AssertType::Pointer,
        "Bad Edge",
        Some("Edge   failed AssertValid during AssertSubTopology call"),
    ),
    l(
        AssertType::Pointer,
        "Bad CEdge",
        Some("CEdge  failed AssertValid during AssertSubTopology call"),
    ),
    l(
        AssertType::Pointer,
        "Bad Vertex",
        Some("Vertex failed AssertValid during AssertSubTopology call"),
    ),
];

/// `sAssertEdge_list` (`source/SMLib/src/SmEdge.cpp`).
pub const EDGE: &[AssertReportLabel] = &[
    l(AssertType::Pointer, "Bad Curve", Some("m_pCurve is NULL")),
    l(
        AssertType::Pointer,
        "Bad Context",
        Some("Edge and m_pCurve have different contexts"),
    ),
    l(
        AssertType::Domain,
        "Bad Domain",
        Some("Edge m_vInterval not contained in m_pCurve->NaturalInterval"),
    ),
    l(
        AssertType::Pointer,
        "Bad Primary EU",
        Some("Edge m_pList (its primary EU) is NULL"),
    ),
    l(
        AssertType::NestedTest,
        "Bad SubTopology",
        Some("An Edge SubTopology Vertex failed its AssertValid check"),
    ),
    l(
        AssertType::Geometric,
        "Bad Edge Tol",
        Some("Edge Tolerance less than Brep Tol"),
    ),
    l(
        AssertType::Geometric,
        "Bad Edge Tol",
        Some("Edge Tolerance greater than 1000 times Brep Tol"),
    ),
    l(
        AssertType::Geometric,
        "Bad Edge Length",
        Some("Edge Length less than Edge tolerance"),
    ),
    l(
        AssertType::Topological,
        "Bad Edgeuse TYPE",
        Some("Edgeuse of Manifold or Spine Edge is not TYPE SmLoopuse_TYPE"),
    ),
    l(
        AssertType::Angle,
        "Bad Sector Ang Sequence",
        Some("An Edge->Edgeuse/RadialEdgeuse pair of radial angles do not increase monotonically"),
    ),
    l(
        AssertType::Angle,
        "Bad Radial AngleDeg",
        Some("An Edge->Edgeuse/PrimaryEdgeuse sector AngleDeg not computed from 0.0 to 360.0"),
    ),
    l(
        AssertType::Topological,
        "Bad Vertex Ptrs",
        Some("Not all Edge->Edgeuse->Vertexuse->Vertices map to the same two vertices"),
    ),
    l(
        AssertType::Geometric,
        "Bad Edge Tol",
        Some("stored Vertex ZoneTol not equal the SmTol::Consistent Tolerance Model value"),
    ),
    l(
        AssertType::Geometric,
        "Bad Edge loc",
        Some("Edge not within XSectTol3d of all Face->Surfaces"),
    ),
    l(
        AssertType::Direction,
        "Bad Edgeuse Orientations",
        Some("Edgeuses do not follow same-opposite-same-opposite ordering"),
    ),
    l(
        AssertType::Pointer,
        "Edge Vertex Cnt",
        Some("Closed edges must have one vertex, open edges 2"),
    ),
    l(
        AssertType::Geometric,
        "Edge/Vertex Gap",
        Some("start edge/vertex gap must be less than edge + vertex tolerance"),
    ),
    l(
        AssertType::Geometric,
        "Edge/Vertex Gap",
        Some("end edge/vertex gap must be less than edge + vertex tolerance"),
    ),
];

/// `sAssertFace_list` (`source/SMLib/src/SmFace.cpp`).
pub const FACE: &[AssertReportLabel] = &[
    l(AssertType::Pointer, "Context", Some("Face and its m_pFU must share the same context")),
    l(AssertType::Pointer, "Context", Some("Face and its m_pSurface must share the same context")),
    l(AssertType::Domain, "UV Domain", Some("Face UVDomain must be contained in m_pSurface->NaturalUVDomain")),
    l(AssertType::Pointer, "m_pSurface", Some("Face->Surface pointer must be nonNULL")),
    l(AssertType::Pointer, "Face->Surface->Face", Some("Face->Surface->Face must be this Face")),
    l(AssertType::Pointer, "Composite Face", Some("When Surface->Face is composite, this face must be the SurfaceFace or one its members")),
    l(AssertType::Pointer, "Composite Face", Some("When Surface->Face is not composite, the SurfaceFace must be this face")),
    l(AssertType::NestedTest, "CheckFaceFailure", Some("SmTrimmingTools::CheckFace reported problems")),
    l(AssertType::Pointer, "Edge Vertex Cnt", Some("Closed edges must have one vertex, open edges 2")),
    l(AssertType::Geometric, "Edge/Vertex Gap", Some("start edge/vertex gap must be less than edge + vertex tolerance")),
    l(AssertType::Geometric, "Edge/Vertex Gap", Some("end edge/vertex gap must be less than edge + vertex tolerance")),
    l(AssertType::Pointer, "vertexuse/faceuse", Some("Vertexuse must connect to Faceuse for LoopuseTypes and an EdgeuseTypes not connected to wires")),
    l(AssertType::Geometric, "Face->Vertex", Some("Every Face->Vertex must classify back to the face")),
    l(AssertType::Pointer, "Face->Vertex", Some("Every Face->Vertex must connect back to the face at least once")),
    l(AssertType::Pointer, "Face->Loop", Some("Face->EdgeLoopuse must have at least 1 edgeuse and Face->VertexLoopuses must have no edgesuses")),
    l(AssertType::Pointer, "Face->Loop", Some("Every loop->Loopuse->Edge must by used by both loop->loopuses")),
    l(AssertType::Pointer, "Face->Loop", Some("both loop->loopuses must connect to the same number of loop->edges")),
    l(AssertType::Geometric, "Face->ClosedEdge", Some("Gap between ends of a single edge loop must be less than tol")),
    l(AssertType::Pointer, "Face->Loop->Edgeuse lists", Some("The order of edgeuses in paired face->loopuses must match")),
    l(AssertType::Geometric, "Face->Edge/Edge Gap", Some("EdgeEndPoint/NextEdgeStartPoint Loop Gap must be less than EdgeTol+EdgeTol")),
    l(AssertType::Geometric, "Face->Edge/Edge Gap", Some("StartLoopPoint/EndLoopPoint Loop Gap must be less than EdgeTol+EdgeTol")),
    l(AssertType::NestedTest, "Bad SubTopology", Some("A Face SubTopology Loop, Edge, or Vertex object failed its AssertValid checks")),
    l(AssertType::Domain, "UV Domain", Some("Face UVDomain must be a NonZero area")),
    l(AssertType::Domain, "UV Domain", Some("Face UVDomain must be contained within Surface UVDomain")),
    l(AssertType::Pole, "Face missing PoleVertex", Some("Face Surface Pole in Brep must be marked by a Vertex")),
    l(AssertType::Geometric, "Too Small Face Tol", Some("Face Tolerance less than Brep Tol")),
    l(AssertType::Geometric, "Too Large Face Tol", Some("Face Tolerance greater than 1000 times Brep Tol")),
    l(AssertType::Geometric, "Degenerate Face", Some("Zero Area Face degenerates to a point location")),
    l(AssertType::Geometric, "Degenerate Face", Some("Zero Area Face degenerates to a curve shape")),
    l(AssertType::Geometric, "Face crosses Seam", Some("Face boundary illegally crosses a surface seam.")),
    l(AssertType::Geometric, "Bad Face Tol", Some("stored Face ZoneTol not equal the SmTol::Consistent Tolerance Model value")),
    l(AssertType::Geometric, "Face missing SeamEdge", Some("Face Surface Seam in Brep must be marked by an Edge")),
    l(AssertType::Coincidence, "Face has Coincident Vertices", Some("Face has Coincient Vertices.")),
    l(AssertType::Coincidence, "Face has Coincident Edges", Some("Face has Coincident Edges.")),
    l(AssertType::Coincidence, "Face has partially Coincident Edges", Some("Face has partially Coincident Edges.")),
    l(AssertType::Pointer, "m_pSurface", Some("Face->Surface owner is NULL")),
    l(AssertType::Geometric, "Face Near missing Seam", Some("Face Surface Seam has suspicious Edge near seam")),
    l(AssertType::Inside, "Bad Loop Nesting", Some("Face InnerLoop must lie inside the Face OuterLoop and outside every other InnerLoop")),
    l(AssertType::Geometric, "Intersecting Face Loops", Some("Face Loops must be disjoint in the Face UVSpace, sharing no Vertex, Edge or crossing")),
    l(AssertType::Topological, "Connected Face Loops", Some("Face Loops must not share a Vertex")),
];

/// `sAssertCurveClassification_list` (`source/SMLib/src/SmCurveClass.cpp`).
pub const CURVE_CLASSIFICATION: &[AssertReportLabel] = &[
    l(
        AssertType::Pointer,
        "Bad m_cpCurve",
        Some("m_cpCurve is NULL"),
    ),
    l(
        AssertType::Flag,
        "Bad CurrentCrvFlag Value",
        Some("m_bIsCurveCurrent must equal 0 or 1"),
    ),
    l(
        AssertType::Size,
        "Bad IvlArray Size",
        Some("m_lSize Not <= m_lMaxSize"),
    ),
    l(
        AssertType::Size,
        "Bad IvlArray Ptr",
        Some("When m_lSize > 0, m_pData must be nonNULL"),
    ),
    l(
        AssertType::Flag,
        "Bad TangencyFlag Value",
        Some("m_bIgnoreTangency must equal 0 or 1"),
    ),
    l(
        AssertType::Values,
        "Bad 1st Ivl Min Value",
        Some("CrvClassif->Ivl->Min Value != 1st CrvIvl->Ivl->Min Value"),
    ),
    l(
        AssertType::Values,
        "Bad last Ivl Max Value",
        Some("CrvClassif->Ivl->Max Value != 1st CrvIvl->Ivl->Max Value"),
    ),
    l(
        AssertType::Values,
        "MisMatched Ivl bound Values",
        Some("some CrvIvl[i]->Ivl->Max Value != CrvIvl[i+1]->Ivl->Min Value"),
    ),
    l(
        AssertType::Pointer,
        "Missing DoingFillet Data",
        Some("When DoingFillet, m_cpOtherClassification must be nonNULL"),
    ),
    l(
        AssertType::Flag,
        "Bad m_bTgtLoopsOnly Value",
        Some("m_bTgtLoopsOnly must be 0 or 1"),
    ),
    l(
        AssertType::Pointer,
        "Bad m_pTgtLoop value",
        Some("a Loop of m_sTgtLoops list is not a Loop of m_cpFace"),
    ),
    l(
        AssertType::Flag,
        "Bad m_bOKToSetTolerance Val",
        Some("m_bOKToSetTolerance must be 0 or 1"),
    ),
];

/// `sAssertCurveInterval_list` (`source/SMLib/src/SmCurveClass.cpp`).
pub const CURVE_INTERVAL: &[AssertReportLabel] = &[l(
    AssertType::Pointer,
    "Bad CurveClass BackPtr",
    Some("m_pCurveClassification back pointer must be nonNULL"),
)];

/// `sAssertPointClassification_list` (`source/SMLib/src/SmPointClass.cpp`).
pub const POINT_CLASSIFICATION: &[AssertReportLabel] = &[
    l(
        AssertType::Values,
        "Bad m_ePointClass",
        Some("m_ePointClass == SM_PC_UNKNOWN when m_pObject != NULL"),
    ),
    l(
        AssertType::Values,
        "Uninit SrcZoneTol3d",
        Some("m_sSrcZoneTol3d is Uninit"),
    ),
    l(
        AssertType::Values,
        "Bad SrcZoneTol3d",
        Some("m_sSrcZoneTol3d is less than SM_EFF_ZERO"),
    ),
];

/// `sAssertTopology_list` (`source/SMLib/src/SmTopology.cpp`).
pub const TOPOLOGY: &[AssertReportLabel] = &[
    l(
        AssertType::Pointer,
        "Owner",
        Some("m_pListOwner is in list"),
    ),
    l(
        AssertType::Pointer,
        "Context",
        Some("m_pListOwner shares the same context"),
    ),
    l(
        AssertType::Mark,
        "Mark",
        Some("m_lMark is less than context mark"),
    ),
    l(
        AssertType::Mark,
        "Mark2",
        Some("m_lMark2 is less than context mark2"),
    ),
    l(
        AssertType::Mark,
        "Mark3",
        Some("m_lMark3 is less than context mark3"),
    ),
    l(
        AssertType::Pointer,
        "Equal Domain",
        Some("m_pNext is non-NULL"),
    ),
    l(
        AssertType::Pointer,
        "Equal Domain",
        Some("m_pLast is non-NULL"),
    ),
    l(
        AssertType::Pointer,
        "Linked List",
        Some("Circularly linked list corruption"),
    ),
    l(
        AssertType::Pointer,
        "Counts",
        Some("Incorrect m_pList count"),
    ),
];

/// `sAssertOwningTopology_list` (`source/SMLib/src/SmOwningTopology.cpp`).
pub const OWNING_TOPOLOGY: &[AssertReportLabel] = &[
    l(
        AssertType::Pointer,
        "Null",
        Some("pTgt linked list is Non-NULL"),
    ),
    l(
        AssertType::Pointer,
        "Back Pointer",
        Some("pTgt back pointer is valid"),
    ),
    l(AssertType::Pointer, "Owner", Some("pTgt owner is valid")),
    l(
        AssertType::Pointer,
        "List Length",
        Some("pTgt->m_pNext == pStart"),
    ),
    l(
        AssertType::Pointer,
        "Context",
        Some("pTgt shares the same context"),
    ),
];

/// `sAssertEdgeuse_list` (`source/SMLib/src/SmEdgeuse.cpp`).
pub const EDGEUSE: &[AssertReportLabel] = &[
    l(AssertType::Pointer, "Bad Context", Some("Edgeuse and m_pSorLU have different contexts")),
    l(AssertType::Pointer, "Bad Context", Some("Edgeuse and m_pCCW have different contexts")),
    l(AssertType::Pointer, "Bad Context", Some("Edgeuse and m_pCW have different contexts")),
    l(AssertType::Pointer, "Bad Context", Some("Edgeuse and m_pVU have different contexts")),
    l(AssertType::Pointer, "Bad Context", Some("Edgeuse and m_pUVTrimCurve have different contexts")),
    l(AssertType::Type, "Bad TYPE", Some("SmEdgeuse->m_tEdgeuseType and m_pSorLU have inconsistent type values")),
    l(AssertType::Pointer, "Bad UVTrimCurve Owner", Some("EdgeuseUVTrimCurve->Owner != Edgeuse")),
    l(AssertType::Geometric, "Bad UVTrimCurve", Some("Edgeuse->UVTrimCurve jumps across a periodic surface boundary")),
    l(AssertType::Pointer, "Bad RadialPair Shell", Some("An Edgeuse/RadialEdgeuse pair not attached to same Shell")),
    l(AssertType::Direction, "Warning 0.0 Angle Sector", Some("A TangentSector Edgeuse/RadialEdgeuse pair of SurfaceNormals point in opposite directions")),
    l(AssertType::Direction, "Bad EdgeSector Normals", Some("An Edge->Edgeuse/RadialEdgeuse pair of SurfaceNormals don't consistently point into the sector")),
    l(AssertType::Geometric, "Bad UVTrimCurve", Some("Edgeuse->UVTrimCurve has zero length")),
    l(AssertType::Topological, "Bad Loop  Connection", Some("Edgeuse->GetLoop() obj doesn't contain this Edgeuse")),
    l(AssertType::Topological, "Bad Shell Connection", Some("Edgeuse->GetShell() obj doesn't contain this Edgeuse")),
    l(AssertType::Topological, "Bad LoopNeighbor Connection", Some("Edgeuse->m_pCCW->m_pCW Ptr != Edgeuse")),
    l(AssertType::Topological, "Bad LoopNeighbor Connection", Some("Edgeuse->m_pCW->m_pCCW Ptr != Edgeuse")),
    l(AssertType::Topological, "Bad Vertex Connection", Some("Edgeuse->m_pVU->GetLoopuse() Ptr != Edgeuse")),
    l(AssertType::Topological, "Bad Loopuse Pointer", Some("Edgeuse m_tEdgeuseType is SmLoopuse_TYPE when m_pSorLU does not point to a Loopuse")),
    l(AssertType::Topological, "Bad Shell Pointer", Some("Edgeuse m_tEdgeuseType is SmShell_TYPE when m_pSorLU does not point to a Shell")),
    l(AssertType::Coincidence, "Bad Loop Connection Vertex", Some("Edgeuse/Edgeuse LoopNeighbor connection not made through a single Vertex (probably a coincident vertex pair)")),
    l(AssertType::Pole, "Bad UVTrimCurve Near Pole", Some("UVTrimCurve ControlPolygon turns sharply to be tangent to nearby surface pole UV boundary.  The bad turn causes tessellation and other problems")),
    l(AssertType::Cache, "Bad Edge/Face Cache", Some("Edgeuse cached Edge/Face Gap value is stale")),
    l(AssertType::Pointer, "Bad MatedPair Face", Some("An Edgeuse/MateEdgeuse pair not attached to same Face")),
    l(AssertType::Direction, "Bad RadialPair Orientations", Some("An Edgeuse/RadialEdgeuse pair do not have different Orientations")),
    l(AssertType::Direction, "Bad MatedPair Orientations", Some("An Edgeuse/MateEdgeuse pair do not have different Orientations")),
    l(AssertType::Parameterization, "Bad UVTrimCurve Parameterization", Some("An Edgeuse UVTrimCurve->Ivl != Edge->Ivl")),
    l(AssertType::Parameterization, "Bad UVTrimCurve Parameterization", Some("An Edgeuse UVTrimCurve->Ivl NotContainedBy Edge->Curve->Ivl")),
    l(AssertType::Geometric, "Bad Binormal Location", Some("An Edgeuse/RadialEdgeuse pair do not evaluate to the same binormal location")),
];

/// `sAssertFaceuse_list` (`source/SMLib/src/SmFaceuse.cpp`).
pub const FACEUSE: &[AssertReportLabel] = &[
    l(
        AssertType::Pointer,
        "Context",
        Some("m_pF shares the same context"),
    ),
    l(
        AssertType::Pointer,
        "Context",
        Some("m_pFUMate shares the same context"),
    ),
];

/// `sAssertVertexuse_list` (`source/SMLib/src/SmVertexuse.cpp`).
pub const VERTEXUSE: &[AssertReportLabel] = &[
    l(AssertType::Pointer, "Bad Context Pointers", Some("this Vertexuse and its m_pSorLUorEU don't share the same context")),
    l(AssertType::Type, "Bad Type Value", Some("this Vertexuse->m_tVertexuseType does not match the m_pSorLUorEU->m_tObjType value")),
    l(AssertType::Cache, "Bad Vertex/Edge Cache", Some("Vertexuse cached Vertex/Edge Gap value is stale")),
    l(AssertType::Cache, "Bad Vertex/Face Cache", Some("Vertexuse cached Vertex/Face Gap value is stale")),
    l(AssertType::Geometric, "Bad Gap or Orientation", Some("Vertexuse->Sector has UV and 3d gaps exceeding XSectTol or Sector nearby interior not inside FaceSrf->UVDomain")),
    l(AssertType::Topological, "Bad Missing Seam", Some("Vertexuse->Sector contains a FaceSrf->Seam not marked by SeamEdge")),
    l(AssertType::Pointer, "vertexuse/faceuse", Some("Vertexuse must connect to Faceuse for LoopuseTypes and an EdgeuseTypes not connected to wires")),
];

/// `sAssertVertex_list` (`source/SMLib/src/SmVertex.cpp`).
pub const VERTEX: &[AssertReportLabel] = &[
    l(
        AssertType::Geometric,
        "Bad Vertex Tol",
        Some("stored Vertex ZoneTol not equal the SmTol::Consistent Tolerance Model value"),
    ),
    l(
        AssertType::Geometric,
        "Bad Vertex Gap",
        Some("Vertex-Face Gap exceeds XSectTol3d for one (or more) Faces"),
    ),
    l(
        AssertType::Topological,
        "Bad Topology",
        Some("Related TopologyGraph Ptrs are not consistent for all Vertex->Vertexuses."),
    ),
];

/// `sAssertLoop_list` (`source/SMLib/src/SmLoop.cpp`).
pub const LOOP: &[AssertReportLabel] = &[
    l(AssertType::Pointer, "Bad Context", Some("Loop and m_pLU have different contexts")),
    l(AssertType::NestedTest, "Bad SubTopology", Some("A Loop SubTopology Edge or Vertex object failed its AssertValid checks")),
    l(AssertType::Geometric, "Bad Loop Orient", Some("A UpperLoopuse->Edgeuse's 'Inside' is Outside Surface Natural Boundary - loop may have wrong orientation")),
    l(AssertType::Pointer, "Face->Loop", Some("Face->EdgeLoopuse must have at least 1 edgeuse and Face->VertexLoopuses must have no edgesuses")),
    l(AssertType::Pointer, "Face->Loop", Some("Every loop->Loopuse->Edge must by used by both loop->loopuses")),
    l(AssertType::Pointer, "Face->Loop", Some("both loop->loopuses must connect to the same number of loop->edges")),
    l(AssertType::Pointer, "Face->Loop->Edgeuse lists", Some("The order of edgeuses in paired face->loopuses must match")),
];

/// `sAssertLoopuse_list` (`source/SMLib/src/SmLoopuse.cpp`).
pub const LOOPUSE: &[AssertReportLabel] = &[
    l(
        AssertType::Pointer,
        "MisMatched Context ptrs",
        Some("Loopuse and member m_pL contexts differ"),
    ),
    l(
        AssertType::Pointer,
        "MisMatched Context ptrs",
        Some("Loopuse and member m_pLUMate contexts differ"),
    ),
    l(
        AssertType::Pointer,
        "MisMatched Context ptrs",
        Some("Loopuse and member m_pEUorVU contexts differ"),
    ),
    l(
        AssertType::Topological,
        "No Edgeuse or Vertexuse",
        Some("When Loopuse is used in a Faceuse, it must have a Vertexuse or Edgeuse"),
    ),
    l(
        AssertType::Topological,
        "Bad Missing Seam",
        Some("a Loop->Vertexuse->Sector contains a FaceSrf->Seam not marked by SeamEdge"),
    ),
    l(
        AssertType::Geometric,
        "Edgeuses out of tolerance",
        Some("Gaps between Loopuse Edgeuses are greater than XSectTol3d"),
    ),
];

/// `sAssertRegion_list` (`source/SMLib/src/SmRegion.cpp`).
pub const REGION: &[AssertReportLabel] = &[
    l(AssertType::NestedTest, "Bad SubTopology", Some("A Region SubTopology Shell, Face, Loop, Edge, or Vertex object failed its AssertValid checks")),
];

/// `sAssertShell_list` (`source/SMLib/src/SmShell.cpp`).
pub const SHELL: &[AssertReportLabel] = &[
    l(AssertType::Pointer, "Pointer", Some("m_pList is non-NULL")),
    l(
        AssertType::Size,
        "SmVertexuse_TYPE",
        Some("sShellVertices has one vertex"),
    ),
    l(
        AssertType::Size,
        "SmVertexuse_TYPE",
        Some("sWireEdges is empty "),
    ),
    l(
        AssertType::Size,
        "SmVertexuse_TYPE",
        Some("sShellFaceuses is empty "),
    ),
    l(
        AssertType::Pointer,
        "SmVertexuse_TYPE",
        Some("pVu share same context"),
    ),
    l(
        AssertType::Type,
        "SmVertexuse_TYPE",
        Some("pVu is vertexuse type"),
    ),
    l(
        AssertType::Pointer,
        "SmVertexuse_TYPE",
        Some("pVU->m_pSorLUorEU points back to this"),
    ),
    l(
        AssertType::Pointer,
        "SmEdgeuse_TYPE",
        Some("pEu1 shares the same context"),
    ),
    l(
        AssertType::Type,
        "SmEdgeuse_TYPE",
        Some("pEu1 is edgeuse type"),
    ),
    l(
        AssertType::Pointer,
        "SmEdgeuse_TYPE",
        Some("pEu1->m_pSorLU points back to this"),
    ),
    l(
        AssertType::Size,
        "SmEdgeuse_TYPE",
        Some("sShellVertices is empty"),
    ),
    l(
        AssertType::Size,
        "SmEdgeuse_TYPE",
        Some("sShellFaceuses is empty"),
    ),
    l(
        AssertType::Size,
        "SmEdgeuse_TYPE",
        Some("sEdgeuses1 and sEdgeuses2 share the same size"),
    ),
    l(
        AssertType::Pointer,
        "SmEdgeuse_TYPE",
        Some("pEu1 shares same context"),
    ),
    l(
        AssertType::Pointer,
        "SmEdgeuse_TYPE",
        Some("pEu2 shares same context"),
    ),
    l(AssertType::Type, "SmEdgeuse_TYPE", Some("pEu1 is edgeuse")),
    l(
        AssertType::Pointer,
        "SmEdgeuse_TYPE",
        Some("pEu1->m_pSorLU points back to this"),
    ),
    l(AssertType::Type, "SmEdgeuse_TYPE", Some("pEu2 is edgeuse ")),
    l(
        AssertType::Pointer,
        "SmEdgeuse_TYPE",
        Some("pEu2->m_pSorLU points back to this "),
    ),
    l(
        AssertType::Pointer,
        "SmFaceuse_TYPE",
        Some("pFu shares the same context"),
    ),
    l(
        AssertType::Pointer,
        "SmFaceuse_TYPE",
        Some("pFu->m_pListOwner points back to this"),
    ),
    l(
        AssertType::Size,
        "SmFaceuse_TYPE",
        Some("sFaceuses1 and sFaceuses2 are equal"),
    ),
    l(
        AssertType::Size,
        "SmFaceuse_TYPE",
        Some("sShellFaceuses and sFaceuses2 are equal "),
    ),
    l(
        AssertType::Size,
        "SmFaceuse_TYPE",
        Some("sShellVertices is empty "),
    ),
    l(
        AssertType::Pointer,
        "SmFaceuse_TYPE",
        Some("sFaceuses1 shares the same context"),
    ),
    l(
        AssertType::Pointer,
        "SmFaceuse_TYPE",
        Some("sFaceuses1->GetShell() points back to this"),
    ),
    l(
        AssertType::Pointer,
        "SmFaceuse_TYPE",
        Some("sFaceuses2[ii]->GetShell() points back to this"),
    ),
    l(
        AssertType::Pointer,
        "SmFaceuse_TYPE",
        Some("pEu shares the same context"),
    ),
    l(
        AssertType::Pointer,
        "SmFaceuse_TYPE",
        Some("pShellRadial points back to this"),
    ),
    l(
        AssertType::Pointer,
        "SmFaceuse_TYPE",
        Some("pEu and pEuRadial share the same edge"),
    ),
    l(AssertType::Distance, "SmFaceuse_TYPE", Some("dDist < dTol")),
    l(
        AssertType::Direction,
        "SmFaceuse_TYPE",
        Some("sThisEuTangent == -sRadialTangent"),
    ),
    l(
        AssertType::Angle,
        "SmFaceuse_TYPE",
        Some("dNormAngle = dBiAngle - Pi"),
    ),
    l(
        AssertType::NestedTest,
        "Bad SubTopology",
        Some(
            "A Shell SubTopology Face, Loop, Edge, or Vertex object failed its AssertValid checks",
        ),
    ),
    l(
        AssertType::Pointer,
        "SmFaceuse_TYPE",
        Some("Faceuse->Loopuse is NULL"),
    ),
];

/// `sAssertPolyFace_list` (`source/SMLib/src/SmPoly.cpp`).
pub const POLY_FACE: &[AssertReportLabel] = &[
    l(AssertType::Pointer, "Bad CPolyFace Context", Some("this PolyFace->context not the same as its owner m_pCPolyFace->context")),
    l(AssertType::Pointer, "Bad CPolyFace Back Pointer", Some("this PolyFace->m_pCPolyFace value equals this - it should point to a Composite PolyFace")),
    l(AssertType::List, "Bad CPolyFace List", Some("this PolyFace not in its m_pCPolyFace->PolyFace list")),
    l(AssertType::Pointer, "Bad Loop", Some("PolyFace->m_pList of PolyLoops is NULL - PolyFace should have at least 1 PolyLoop")),
    l(AssertType::Pointer, "Bad Sibling PolyFace BackPtrs", Some("PolyFace->pNext->pLast != pPolyFace")),
    l(AssertType::Pointer, "Bad Sibling PolyFace BackPtrs", Some("PolyFace->pLast->pNext != pPolyFace")),
    l(AssertType::Topological, "Bad Loop Order", Some("PolyFace->1st PolyLoop orientation not SM_OT_SAME")),
    l(AssertType::Topological, "Bad Loop Order", Some("PolyFace->After 1st PolyLoop orientation not SM_OT_OPPOSITE")),
    l(AssertType::Topological, "Bad Loop Orienation", Some("PolyFace has more than 1 SM_OT_SAME oriented PolyLoops, must have just 1.")),
];

/// `sAssertCPolyFace_list` (`source/SMLib/src/SmPoly.cpp`).
pub const C_POLY_FACE: &[AssertReportLabel] = &[
    l(
        AssertType::Pointer,
        "SmCPolyFace",
        Some("this m_pCPolyFace shares this pointer"),
    ),
    l(
        AssertType::Pointer,
        "Member->m_pCPolyFace",
        Some("CPolyFace member must point to this CPolyFace"),
    ),
];

/// `sAssertPolyLoop_list` (`source/SMLib/src/SmPoly.cpp`).
pub const POLY_LOOP: &[AssertReportLabel] = &[
    l(AssertType::Distance, "Bad Gap", Some("Gap Dist > dDistTol3d")),
    l(AssertType::Pointer, "Bad Context", Some("m_pLastEndPolyVertex context != PolyLoop context")),
    l(AssertType::Pointer, "Bad List member", Some("PolyLoop member Obj is not a PolyEdge or is NULL")),
    l(AssertType::Closed, "Bad Closed PolyLoop", Some("PolyLoop->m_bClosed == FALSE, not TRUE")),
    l(AssertType::Pointer, "Bad Back Ptrs", Some("pPolyLoop->pNext->pLast != this pPolyLoop")),
    l(AssertType::Pointer, "Bad Back Ptrs", Some("pPolyLoop->pLast->pNext != this pPolyLoop")),
    l(AssertType::Degenerate, "Some Coincident Vertices", Some("Some pPolyLoop->Vertices are within tol of one another")),
    l(AssertType::Degenerate, "All Coincident Vertices", Some("All pPolyLoop->Vertices are within tol of one another")),
    l(AssertType::Pointer, "Bad NULL Owner Pointer", Some("pPolyLoop->m_pListOwner SmPolyFace Ptr is NULL - PolyLoop is not part of a PolyFace")),
    l(AssertType::Pointer, "Bad NULL Next Pointer", Some("pPolyLoop->m_pNext SmPolyLoop Ptr is NULL - probably PolyLoop is not part of a PolyFace")),
    l(AssertType::Pointer, "Bad NULL Last Pointer", Some("pPolyLoop->m_pLast SmPolyLoop Ptr is NULL - probably PolyLoop is not part of a PolyFace")),
    l(AssertType::Degenerate, "Warn Degenerate PolyFacePolyLoop with less than 3 PolyEdges", None),
];

/// `sAssertPolyVertex_list` (`source/SMLib/src/SmPoly.cpp`).
pub const POLY_VERTEX: &[AssertReportLabel] = &[
    l(
        AssertType::Pointer,
        "Empty PolyEdge List",
        Some("PolyVertex not attached to a PolyEdge"),
    ),
    l(
        AssertType::Pointer,
        "Context",
        Some("m_pPolyEdgeList has different context than PolyVertex"),
    ),
    l(
        AssertType::Geometric,
        "Uninit Point",
        Some("Uninitialized PolyVertex Point"),
    ),
    l(
        AssertType::Pointer,
        "Broken BackPtrs",
        Some("pPolyVertex->pNext->pLast != pPolyVertex"),
    ),
    l(
        AssertType::Pointer,
        "Broken BackPtrs",
        Some("pPolyVertex->pLast->pNext != pPolyVertex"),
    ),
    l(
        AssertType::Pointer,
        "Broken BackPtrs",
        Some("pPolyVertex->Edges->m_pPolyVertex != pPolyVertex"),
    ),
];

/// `sAssertPolyEdge_list` (`source/SMLib/src/SmPoly.cpp`).
pub const POLY_EDGE: &[AssertReportLabel] = &[
    l(AssertType::Pointer, "Bad Back Pointer", Some("PolyEdge->m_pNextRadialE->m_pLastEdgeAtV != this pPolyEdge")),
    l(AssertType::Pointer, "Bad Back Pointer", Some("PolyEdge->m_pLastEdgeAtV->m_pNextRadialE != this pPolyEdge")),
    l(AssertType::Pointer, "Bad Back Pointer", Some("PolyEdge->m_pNextEdgeAtV->m_pLastEdgeAtV != this pPolyEdge")),
    l(AssertType::Pointer, "Bad Back Pointer", Some("PolyEdge->m_pLastEdgeAtV->m_pNextEdgeAtV != this pPolyEdge")),
    l(AssertType::Pointer, "Bad Linked List", Some("Radial Edge circular linked list corruption")),
    l(AssertType::Pointer, "Bad Context", Some("PolyEdge->m_pNextEdgeAtV different context than this PolyEdge")),
    l(AssertType::Pointer, "Bad Context", Some("PolyEdge->m_pLastRadialE different context than this PolyEdge")),
    l(AssertType::Pointer, "Bad Context", Some("PolyEdge->m_pNextEdgeAtV different context than this PolyEdge")),
    l(AssertType::Pointer, "Bad Context", Some("PolyEdge->m_pPrevEdgeAtV different context than this PolyEdge")),
    l(AssertType::Pointer, "Bad Context", Some("PolyEdge->m_pPolyVertex shares the same context than this PolyEdge")),
    l(AssertType::Pointer, "Bad Context", Some("PolyEdge->m_pNext (CCW) LoopNeighbor different context than this PolyEdge")),
    l(AssertType::Pointer, "Bad Context", Some("PolyEdge->m_pLast (CW) LoopNeighbor different context than this PolyEdge")),
    l(AssertType::Pointer, "Bad Back Pointer", Some("PolyEdge->m_pNext->m_pLast (PolyEdge->CCW->CW Loop Neighbors) != this SmPolyEdge")),
    l(AssertType::Pointer, "Bad Back Pointer", Some("PolyEdge->m_pLast->m_pNext (PolyEdge->CW->CCW Loop Neighbors) != this SmPolyEdge")),
    l(AssertType::Coincidence, "Bad Coincidence", Some("Zero Dihedral Angle between 2 PolyFaces across this PolyEdge and one of its Radial neighbors")),
    l(AssertType::Pointer, "Bad NULL Pointer", Some("PolyEdge->m_pNextRadialE == NULL, should be part of closed PtrLoop")),
    l(AssertType::Pointer, "Bad NULL Pointer", Some("PolyEdge->m_pLastRadialE == NULL, should be part of closed PtrLoop")),
    l(AssertType::Pointer, "Bad NULL Pointer", Some("PolyEdge->m_pNextEdgeAtV == NULL, should be part of closed PtrLoop")),
    l(AssertType::Pointer, "Bad NULL Pointer", Some("PolyEdge->m_pPrevEdgeAtV == NULL, should be part of closed PtrLoop")),
    l(AssertType::Pointer, "Bad NULL Pointer", Some("PolyEdge->m_pNext (CCW) LoopNeighbor == NULL, should be part of closed PtrLoop")),
    l(AssertType::Pointer, "Bad NULL Pointer", Some("PolyEdge->m_pLast (CW) LoopNeighbor  == NULL, should be part of closed PtrLoop")),
    l(AssertType::Pointer, "Bad NULL Pointer", Some("PolyEdge->m_pStartPolyVertex == NULL, PolyEdge has no Start position")),
    l(AssertType::Degenerate, "Bad Degenerate Line", Some("PolyEdge length is zero")),
    l(AssertType::Coincidence, "Bad Coincidence", Some("Two PolyEdges share PolyVertices but are not Radial Partners")),
    l(AssertType::Degenerate, "Warn Short Line", Some("PolyEdge length too short - EndPolyVertices are within tolerance to one another")),
    l(AssertType::Pointer, "Bad NULL Pointer", Some("PolyEdge->m_pListOwner PolyLoop Ptr == NULL")),
];

/// `sAssertPolyRegion_list` (`source/SMLib/src/SmPoly.cpp`).
pub const POLY_REGION: &[AssertReportLabel] = &[
    l(AssertType::Pointer, "Null", Some("pShell is non-NULL")),
    l(
        AssertType::Pointer,
        "Broken BackPtrs",
        Some("pPolyFace->pNext->pLast != pPolyFace"),
    ),
    l(
        AssertType::Pointer,
        "Broken BackPtrs",
        Some("pPolyFace->pLast->pNext != pPolyFace"),
    ),
];

/// `sAssertPolyShell_list` (`source/SMLib/src/SmPoly.cpp`).
pub const POLY_SHELL: &[AssertReportLabel] = &[
    l(AssertType::Pointer, "Null", Some("pFace is non-NULL")),
    l(
        AssertType::Pointer,
        "Broken BackPtrs",
        Some("pPolyFace->pNext->pLast != pPolyFace"),
    ),
    l(
        AssertType::Pointer,
        "Broken BackPtrs",
        Some("pPolyFace->pLast->pNext != pPolyFace"),
    ),
];

/// `sAssertPolyBrep_list` (`source/SMLib/src/SmPoly.cpp`).
pub const POLY_BREP: &[AssertReportLabel] = &[
    l(
        AssertType::Pointer,
        "Context",
        Some("m_pPolyVertexListHead shares the same context"),
    ),
    l(
        AssertType::Pointer,
        "Context",
        Some("m_pCPolyFaceListHead shares the same context"),
    ),
    l(AssertType::Pointer, "Null", Some("pRegion is non-NULL")),
    l(
        AssertType::Pointer,
        "Null",
        Some("NULL PolyVertexPtr in PolyVertex list"),
    ),
    l(
        AssertType::Pointer,
        "Null",
        Some("NULL CPolyFace in CPolyFace list"),
    ),
    l(
        AssertType::Coincidence,
        "Coincidence",
        Some("Coincident Topology"),
    ),
    l(
        AssertType::Geometric,
        "Uniform 2d/3d",
        Some("Mixed 2d and 3d PolyVertex position spaces"),
    ),
    l(
        AssertType::Coincidence,
        "Coincident PolyVerts",
        Some("Coincidence PolyVertices"),
    ),
    l(
        AssertType::Geometric,
        "Intersecting PolyEdges",
        Some("PolyEdges intersect one another"),
    ),
    l(
        AssertType::Topological,
        "Vertex List duplicate entry",
        Some("PolyVertex listed multiple times on m_pVertexListHead"),
    ),
];

/// `sTrimmingCheckLoopForMiniHourglass_list` (`source/SMLib/src/SmTrimmingTools.cpp`).
pub const TRIMMING_CHECK_LOOP_FOR_MINI_HOURGLASS: &[AssertReportLabel] = &[l(
    AssertType::Geometric,
    "Small Closed Edge",
    Some("Signal as a problem any closed edge whose length is less than tol"),
)];

/// `sTrimmingCheckTessellation_list` (`source/SMLib/src/SmTrimmingTools.cpp`).
pub const TRIMMING_CHECK_TESSELLATION: &[AssertReportLabel] = &[l(
    AssertType::Geometric,
    "tessellation",
    Some("This face could not be tessellated by NLib "),
)];

/// `sTrimmingCheckFace_list` (`source/SMLib/src/SmTrimmingTools.cpp`).
pub const TRIMMING_CHECK_FACE: &[AssertReportLabel] = &[
    l(AssertType::Pointer, "Loop", Some("Face has no Loops")),
    l(
        AssertType::Geometric,
        "UVTrimCurve",
        Some("Info only: Face has edges without UVTrimCurves"),
    ),
    l(
        AssertType::Geometric,
        "Loop Orientation",
        Some("unable to compute loop orientation for Face Loop"),
    ),
    l(
        AssertType::Geometric,
        "Loop Orientation",
        Some("Face Loop has bad orientation"),
    ),
    l(
        AssertType::Geometric,
        "Loop Orientation",
        Some("Face Loop has zero area"),
    ),
];

/// `sTrimmingCheckLoop_list` (`source/SMLib/src/SmTrimmingTools.cpp`).
pub const TRIMMING_CHECK_LOOP: &[AssertReportLabel] = &[
    l(
        AssertType::Geometric,
        "Edge/Edge Gap",
        Some("Gap between edge endPoints exceeds tolerance"),
    ),
    l(
        AssertType::Domain,
        "UVEdge/UVEdge Gap",
        Some("too large U Dir gap between loop Edge endPoints that are not on a singularity"),
    ),
    l(
        AssertType::Domain,
        "UVEdge/UVEdge Gap",
        Some("too large V Dir gap between loop Edge endPoints that are not on a singularity"),
    ),
];

/// `sAssertCrvInVolume_list` (`source/SMLib/src/SmCrvInVolume.cpp`).
pub const CRV_IN_VOLUME: &[AssertReportLabel] = &[
    l(
        AssertType::NestedTest,
        "Base Curve",
        Some("BaseCurve must pass AssertValid"),
    ),
    l(
        AssertType::NestedTest,
        "Compounding Volume",
        Some("Compounding Volume must pass AssertValid"),
    ),
    l(
        AssertType::Cache,
        "Inconsistent Cache",
        Some("Cached breaks and associated continuities must be consistent"),
    ),
    l(
        AssertType::Pointer,
        "Bad BaseCurve Owner",
        Some("When owned, BaseCurve owner must be this SmCrvInVolume object"),
    ),
    l(
        AssertType::Pointer,
        "Bad BaseVolume Owner",
        Some("When owned, BaseVolume owner must be this SmCrvInVolume object"),
    ),
    l(
        AssertType::Pointer,
        "Bad BaseCurve Context",
        Some("When owned, BaseCurve->context must be this SmCrvInVolume->context"),
    ),
    l(
        AssertType::Pointer,
        "Bad BaseVolume Context",
        Some("When owned, BaseVolume->context must be this SmCrvInVolume->context"),
    ),
];

/// `sAssertSrfInVolume_list` (`source/SMLib/src/SmSrfInVolume.cpp`).
pub const SRF_IN_VOLUME: &[AssertReportLabel] = &[
    l(
        AssertType::NestedTest,
        "Base Surface",
        Some("Base Surface must pass AssertValid"),
    ),
    l(
        AssertType::NestedTest,
        "Compounding Volume",
        Some("Compounding Volume must pass AssertValid"),
    ),
    l(
        AssertType::Pointer,
        "Bad BaseSurface Owner",
        Some("When owned, BaseSurface owner must be this SmSrfInVolume object"),
    ),
    l(
        AssertType::Pointer,
        "Bad BaseVolume Owner",
        Some("When owned, BaseVolume owner must be this SmSrtInVolume object"),
    ),
    l(
        AssertType::Pointer,
        "Bad BaseSurface Context",
        Some("When owned, BaseSurface context must be this SmSrfInVolume context"),
    ),
    l(
        AssertType::Pointer,
        "Bad BaseVolume Context",
        Some("When owned, BaseVolume context must be this SmSrtInVolume context"),
    ),
];

/// `sAssertVolume_list` (`source/SMLib/src/SmVolume.cpp`).
pub const VOLUME: &[AssertReportLabel] = &[
    l(
        AssertType::NestedTest,
        "Contained NextMap",
        Some("Contained NextMap does not pass AssertValid"),
    ),
    l(
        AssertType::NestedTest,
        "Contained OrientMap",
        Some("Contained OrientMap does not pass AssertValid"),
    ),
    l(
        AssertType::NestedTest,
        "Contained InvOrientMap",
        Some("Contained InvOrientMap does not pass AssertValid"),
    ),
    l(
        AssertType::NestedProperty,
        "Simple OrientMap",
        Some("Contained OrientMap->m_pNextMap is not NULL"),
    ),
    l(
        AssertType::NestedProperty,
        "Simple OrientMap",
        Some("Contained OrientMap->m_pOrientMap is not NULL"),
    ),
    l(
        AssertType::NestedProperty,
        "Simple OrientMap",
        Some("Contained OrientMap->m_pInvOrientMap is not NULL"),
    ),
    l(
        AssertType::NestedProperty,
        "Simple InvOrientMap",
        Some("Contained InvOrientMap->m_pNextMap is not NULL"),
    ),
    l(
        AssertType::NestedProperty,
        "Simple InvOrientMap",
        Some("Contained InvOrientMap->m_pOrientMap is not NULL"),
    ),
    l(
        AssertType::NestedProperty,
        "Simple InvOrientMap",
        Some("Contained InvOrientMap->m_pInvOrientMap is not NULL"),
    ),
];

/// `sAssertBSplineVolume_list` (`source/SMLib/src/SmBSplineVolume.cpp`).
pub const B_SPLINE_VOLUME: &[AssertReportLabel] = &[
    l(AssertType::Pointer, "Null", Some("m_pNurb is non-NULL")),
    l(
        AssertType::NestedTest,
        "Validity Tests",
        Some("Volume failed PassesValidityCheck() call"),
    ),
];

/// `sAssertTransform_list` (`source/SMLib/src/SmTransform.cpp`).
pub const TRANSFORM: &[AssertReportLabel] = &[l(
    AssertType::Unknown,
    "UNKNOWN",
    Some("No constraints on m_adT to check for yet"),
)];

/// `sAssertBendVolume_list` (`source/SMLib/src/SmBendVolume.cpp`).
pub const BEND_VOLUME: &[AssertReportLabel] = &[
    l(
        AssertType::UnitVector,
        "unit-vector",
        Some("BendAxis has to be unit lenghth"),
    ),
    l(
        AssertType::UnitVector,
        "unit-vector",
        Some("BendMidDir has to be unit lenghth"),
    ),
    l(
        AssertType::UnitVector,
        "unit-vector",
        Some("BendBinormal has to be unit lenghth"),
    ),
    l(
        AssertType::Geometric,
        "orthogonal",
        Some("Bend Axis/StartDir/Binormal must be mutually orthoganal"),
    ),
    l(
        AssertType::Geometric,
        "orthogonal",
        Some("Bend Axis/StartDir/Binormal must be right handed"),
    ),
    l(
        AssertType::Geometric,
        "Positive-Distance",
        Some("Bend NeutralDistance must be PositiveDefinite"),
    ),
    l(
        AssertType::Geometric,
        "Positive-Tolerance",
        Some("Bend SingularityTol must be PositiveDefinite"),
    ),
];

/// `sAssertTwistVolume_list` (`source/SMLib/src/SmTwistVolume.cpp`).
pub const TWIST_VOLUME: &[AssertReportLabel] = &[
    l(
        AssertType::UnitVector,
        "unit-vector",
        Some("TwistAxis has to be unit lenghth"),
    ),
    l(
        AssertType::UnitVector,
        "unit-vector",
        Some("TwistMidDir has to be unit lenghth"),
    ),
    l(
        AssertType::UnitVector,
        "unit-vector",
        Some("TwistBinormal has to be unit lenghth"),
    ),
    l(
        AssertType::Geometric,
        "orthogonal",
        Some("Twist Axis/StartDir/Binormal must be mutually orthoganal"),
    ),
    l(
        AssertType::Geometric,
        "orthogonal",
        Some("Twist Axis/StartDir/Binormal must be right handed"),
    ),
    l(
        AssertType::Geometric,
        "NonZero",
        Some("Twist PeriodDistance abs value has to be greater than PeriodMinDist"),
    ),
];

/// `sAssertUnbendVolume_list` (`source/SMLib/src/SmUnbendVolume.cpp`).
pub const UNBEND_VOLUME: &[AssertReportLabel] = &[
    l(
        AssertType::UnitVector,
        "unit-vector",
        Some("UnbendAxis has to be unit lenghth"),
    ),
    l(
        AssertType::UnitVector,
        "unit-vector",
        Some("UnbendMidDir has to be unit lenghth"),
    ),
    l(
        AssertType::UnitVector,
        "unit-vector",
        Some("UnbendBinormal has to be unit lenghth"),
    ),
    l(
        AssertType::Geometric,
        "orthogonal",
        Some("Unbend Axis/StartDir/Binormal must be mutually orthoganal"),
    ),
    l(
        AssertType::Geometric,
        "orthogonal",
        Some("Unbend Axis/StartDir/Binormal must be right handed"),
    ),
    l(
        AssertType::Geometric,
        "Positive-Distance",
        Some("Unbend NeutralDistance must be PositiveDefinite"),
    ),
    l(
        AssertType::Geometric,
        "Positive-Tolerance",
        Some("Unbend SingularityTol must be PositiveDefinite"),
    ),
];

/// `SM_GetAssertLabelList`: the label table of `type_code` (list `list_index` when the
/// type has several), or [`DEFAULT`] for an unknown type or list index.
#[must_use]
pub fn label_list(type_code: i64, list_index: u64) -> &'static [AssertReportLabel] {
    match type_code {
        AXIS2_PLACEMENT_TYPE => AXIS2_PLACEMENT,
        POLAR_CONVERSION_TYPE => match list_index {
            0 => POLAR_CONVERSION,
            1 => POLAR_IS_CURRENT,
            _ => DEFAULT,
        },
        CIRCLE_TYPE => CIRCLE,
        ELLIPSE_TYPE => ELLIPSE,
        ATTRIBUTE_TYPE => ATTRIBUTE,
        B_SPLINE_CURVE_TYPE => B_SPLINE_CURVE,
        CURVE_TYPE => CURVE,
        A_OBJECT_TYPE => A_OBJECT,
        LINE_TYPE => LINE,
        TREE_TYPE => TREE,
        HASH_TABLE_TYPE => HASH_TABLE,
        OBJS_IN_VOXELS_TYPE => OBJS_IN_VOXELS,
        SOLUTION_ARRAY_TYPE => SOLUTION_ARRAY,
        B_SPLINE_SURFACE_TYPE => B_SPLINE_SURFACE,
        PLANE_TYPE => PLANE,
        CONE_TYPE => CONE,
        SPHERE_TYPE => SPHERE,
        CYLINDER_TYPE => CYLINDER,
        TORUS_TYPE => TORUS,
        SURF_OF_REVOLUTION_TYPE => SURF_OF_REVOLUTION,
        EXTENT1D_TYPE => EXTENT1D,
        EXTENT2D_TYPE => EXTENT2D,
        EXTENT3D_TYPE => EXTENT3D,
        PERIODIC_EXTENT1D_TYPE => PERIODIC_EXTENT1D,
        POLAR_BOX_TYPE => POLAR_BOX,
        PSEUDO_BOX_TYPE => PSEUDO_BOX,
        DERIV_SURF_DEFINITION_TYPE => DERIV_SURF_DEFINITION,
        SURFACE_CACHE_TYPE => SURFACE_CACHE,
        SURF_OF_EXTRUSION_TYPE => SURF_OF_EXTRUSION,
        SURFACE_TYPE => SURFACE,
        CRV_ON_SURF_TYPE => CRV_ON_SURF,
        PROJECTED_CURVE_TYPE => PROJECTED_CURVE,
        FILLET_EDGE_TYPE => FILLET_EDGE,
        FILLET_BREP_TYPE => FILLET_BREP,
        FILLET_VERTEX_TYPE => FILLET_VERTEX,
        FILLET_VERTEXUSE_TYPE => FILLET_VERTEXUSE,
        FILLET_EDGEUSE_TYPE => FILLET_EDGEUSE,
        SPACE_BEND_TYPE => BEND,
        SPACE_UNBEND_TYPE => UNBEND,
        OFFSET_SURFACE_TYPE => OFFSET_SURFACE,
        OFFSET_CURVE_TYPE => OFFSET_CURVE,
        ASSEMBLY_TYPE => ASSEMBLY,
        ASSEMBLY_INSTANCE_TYPE => ASSEMBLY_INSTANCE,
        SAG_OBJECT_TYPE => SAG_OBJECT,
        BREP_TYPE => match list_index {
            0 => BREP,
            1 => VALIDATE_POINTERS,
            2 => VALIDATE_TOLERANCES,
            3 => COINCIDENT_TOPOLOGY,
            4 => SUB_TOPOLOGY,
            _ => DEFAULT,
        },
        EDGE_TYPE => EDGE,
        FACE_TYPE => FACE,
        CURVE_CLASSIFICATION_TYPE => CURVE_CLASSIFICATION,
        CURVE_INTERVAL_TYPE => CURVE_INTERVAL,
        POINT_CLASSIFICATION_TYPE => POINT_CLASSIFICATION,
        TOPOLOGY_TYPE => TOPOLOGY,
        OWNING_TOPOLOGY_TYPE => OWNING_TOPOLOGY,
        EDGEUSE_TYPE => EDGEUSE,
        FACEUSE_TYPE => FACEUSE,
        VERTEXUSE_TYPE => VERTEXUSE,
        VERTEX_TYPE => VERTEX,
        LOOP_TYPE => LOOP,
        LOOPUSE_TYPE => LOOPUSE,
        REGION_TYPE => REGION,
        SHELL_TYPE => SHELL,
        POLY_FACE_TYPE => POLY_FACE,
        C_POLY_FACE_TYPE => C_POLY_FACE,
        POLY_LOOP_TYPE => POLY_LOOP,
        POLY_VERTEX_TYPE => POLY_VERTEX,
        POLY_EDGE_TYPE => POLY_EDGE,
        POLY_REGION_TYPE => POLY_REGION,
        POLY_SHELL_TYPE => POLY_SHELL,
        POLY_BREP_TYPE => POLY_BREP,
        TRIMMING_TOOLS_TYPE => match list_index {
            0 => TRIMMING_CHECK_LOOP_FOR_MINI_HOURGLASS,
            1 => TRIMMING_CHECK_TESSELLATION,
            2 => TRIMMING_CHECK_FACE,
            3 => TRIMMING_CHECK_LOOP,
            _ => DEFAULT,
        },
        CRV_IN_VOLUME_TYPE => CRV_IN_VOLUME,
        SRF_IN_VOLUME_TYPE => SRF_IN_VOLUME,
        VOLUME_TYPE => VOLUME,
        B_SPLINE_VOLUME_TYPE => B_SPLINE_VOLUME,
        TRANSFORM_TYPE => TRANSFORM,
        BEND_VOLUME_TYPE => BEND_VOLUME,
        TWIST_VOLUME_TYPE => TWIST_VOLUME,
        UNBEND_VOLUME_TYPE => UNBEND_VOLUME,
        _ => DEFAULT,
    }
}

/// Every `(type code, list index)` pair with its own table in [`label_list`], in
/// upstream `switch` order.
pub const LABELLED: &[(i64, u64)] = &[
    (AXIS2_PLACEMENT_TYPE, 0),
    (POLAR_CONVERSION_TYPE, 0),
    (POLAR_CONVERSION_TYPE, 1),
    (CIRCLE_TYPE, 0),
    (ELLIPSE_TYPE, 0),
    (ATTRIBUTE_TYPE, 0),
    (B_SPLINE_CURVE_TYPE, 0),
    (CURVE_TYPE, 0),
    (A_OBJECT_TYPE, 0),
    (LINE_TYPE, 0),
    (TREE_TYPE, 0),
    (HASH_TABLE_TYPE, 0),
    (OBJS_IN_VOXELS_TYPE, 0),
    (SOLUTION_ARRAY_TYPE, 0),
    (B_SPLINE_SURFACE_TYPE, 0),
    (PLANE_TYPE, 0),
    (CONE_TYPE, 0),
    (SPHERE_TYPE, 0),
    (CYLINDER_TYPE, 0),
    (TORUS_TYPE, 0),
    (SURF_OF_REVOLUTION_TYPE, 0),
    (EXTENT1D_TYPE, 0),
    (EXTENT2D_TYPE, 0),
    (EXTENT3D_TYPE, 0),
    (PERIODIC_EXTENT1D_TYPE, 0),
    (POLAR_BOX_TYPE, 0),
    (PSEUDO_BOX_TYPE, 0),
    (DERIV_SURF_DEFINITION_TYPE, 0),
    (SURFACE_CACHE_TYPE, 0),
    (SURF_OF_EXTRUSION_TYPE, 0),
    (SURFACE_TYPE, 0),
    (CRV_ON_SURF_TYPE, 0),
    (PROJECTED_CURVE_TYPE, 0),
    (FILLET_EDGE_TYPE, 0),
    (FILLET_BREP_TYPE, 0),
    (FILLET_VERTEX_TYPE, 0),
    (FILLET_VERTEXUSE_TYPE, 0),
    (FILLET_EDGEUSE_TYPE, 0),
    (SPACE_BEND_TYPE, 0),
    (SPACE_UNBEND_TYPE, 0),
    (OFFSET_SURFACE_TYPE, 0),
    (OFFSET_CURVE_TYPE, 0),
    (ASSEMBLY_TYPE, 0),
    (ASSEMBLY_INSTANCE_TYPE, 0),
    (SAG_OBJECT_TYPE, 0),
    (BREP_TYPE, 0),
    (BREP_TYPE, 1),
    (BREP_TYPE, 2),
    (BREP_TYPE, 3),
    (BREP_TYPE, 4),
    (EDGE_TYPE, 0),
    (FACE_TYPE, 0),
    (CURVE_CLASSIFICATION_TYPE, 0),
    (CURVE_INTERVAL_TYPE, 0),
    (POINT_CLASSIFICATION_TYPE, 0),
    (TOPOLOGY_TYPE, 0),
    (OWNING_TOPOLOGY_TYPE, 0),
    (EDGEUSE_TYPE, 0),
    (FACEUSE_TYPE, 0),
    (VERTEXUSE_TYPE, 0),
    (VERTEX_TYPE, 0),
    (LOOP_TYPE, 0),
    (LOOPUSE_TYPE, 0),
    (REGION_TYPE, 0),
    (SHELL_TYPE, 0),
    (POLY_FACE_TYPE, 0),
    (C_POLY_FACE_TYPE, 0),
    (POLY_LOOP_TYPE, 0),
    (POLY_VERTEX_TYPE, 0),
    (POLY_EDGE_TYPE, 0),
    (POLY_REGION_TYPE, 0),
    (POLY_SHELL_TYPE, 0),
    (POLY_BREP_TYPE, 0),
    (TRIMMING_TOOLS_TYPE, 0),
    (TRIMMING_TOOLS_TYPE, 1),
    (TRIMMING_TOOLS_TYPE, 2),
    (TRIMMING_TOOLS_TYPE, 3),
    (CRV_IN_VOLUME_TYPE, 0),
    (SRF_IN_VOLUME_TYPE, 0),
    (VOLUME_TYPE, 0),
    (B_SPLINE_VOLUME_TYPE, 0),
    (TRANSFORM_TYPE, 0),
    (BEND_VOLUME_TYPE, 0),
    (TWIST_VOLUME_TYPE, 0),
    (UNBEND_VOLUME_TYPE, 0),
];
