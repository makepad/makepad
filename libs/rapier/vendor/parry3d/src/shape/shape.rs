#[cfg(feature = "alloc")]
use alloc::{boxed::Box, vec::Vec};
use core::fmt::Debug;

use crate::bounding_volume::{Aabb, BoundingSphere, BoundingVolume};
use crate::mass_properties::MassProperties;
use crate::math::{Pose, Real, RealField, Vector};
use crate::query::{PointQuery, RayCast};
#[cfg(feature = "serde-serialize")]
use crate::shape::SharedShape;
#[cfg(feature = "alloc")]
use crate::shape::{composite_shape::CompositeShape, Compound, HeightField, Polyline, TriMesh};
use crate::shape::{
    Ball, Capsule, Cuboid, FeatureId, HalfSpace, PolygonalFeatureMap, RoundCuboid, RoundShape,
    RoundTriangle, Segment, SupportMap, Triangle,
};
#[cfg(feature = "dim3")]
use crate::shape::{Cone, Cylinder, RoundCone, RoundCylinder};

#[cfg(feature = "dim3")]
#[cfg(feature = "alloc")]
use crate::shape::{ConvexPolyhedron, RoundConvexPolyhedron, Voxels};

#[cfg(feature = "dim2")]
#[cfg(feature = "alloc")]
use crate::shape::{ConvexPolygon, RoundConvexPolygon, Voxels};
use downcast_rs::{impl_downcast, DowncastSync};
use num::Zero;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
/// Enum representing the type of a shape.
pub enum ShapeType {
    /// A ball shape.
    Ball = 0,
    /// A cuboid shape.
    Cuboid,
    /// A capsule shape.
    Capsule,
    /// A segment shape.
    Segment,
    /// A triangle shape.
    Triangle,
    /// A shape defined as a voxel grid.
    Voxels,
    /// A triangle mesh shape.
    TriMesh,
    /// A set of segments.
    Polyline,
    /// A shape representing a full half-space.
    HalfSpace,
    /// A heightfield shape.
    HeightField,
    /// A Compound shape.
    Compound,
    #[cfg(feature = "dim2")]
    ConvexPolygon,
    #[cfg(feature = "dim3")]
    /// A convex polyhedron.
    ConvexPolyhedron,
    #[cfg(feature = "dim3")]
    /// A cylindrical shape.
    Cylinder,
    #[cfg(feature = "dim3")]
    /// A cone shape.
    Cone,
    // /// A custom shape type.
    // Custom(u8),
    /// A cuboid with rounded corners.
    RoundCuboid,
    /// A triangle with rounded corners.
    RoundTriangle,
    // /// A triangle-mesh with rounded corners.
    // RoundedTriMesh,
    // /// An heightfield with rounded corners.
    // RoundedHeightField,
    /// A cylinder with rounded corners.
    #[cfg(feature = "dim3")]
    RoundCylinder,
    /// A cone with rounded corners.
    #[cfg(feature = "dim3")]
    RoundCone,
    /// A convex polyhedron with rounded corners.
    #[cfg(feature = "dim3")]
    RoundConvexPolyhedron,
    /// A convex polygon with rounded corners.
    #[cfg(feature = "dim2")]
    RoundConvexPolygon,
    /// A custom user-defined shape.
    Custom,
}

#[derive(Copy, Clone)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize))]
/// Enum representing the shape with its actual type
pub enum TypedShape<'a> {
    /// A ball shape.
    Ball(&'a Ball),
    /// A cuboid shape.
    Cuboid(&'a Cuboid),
    /// A capsule shape.
    Capsule(&'a Capsule),
    /// A segment shape.
    Segment(&'a Segment),
    /// A triangle shape.
    Triangle(&'a Triangle),
    #[cfg(feature = "alloc")]
    /// A shape defined as a voxel grid.
    Voxels(&'a Voxels),
    /// A triangle mesh shape.
    #[cfg(feature = "alloc")]
    TriMesh(&'a TriMesh),
    /// A set of segments.
    #[cfg(feature = "alloc")]
    Polyline(&'a Polyline),
    /// A shape representing a full half-space.
    HalfSpace(&'a HalfSpace),
    /// A heightfield shape.
    #[cfg(feature = "alloc")]
    HeightField(&'a HeightField),
    /// A Compound shape.
    #[cfg(feature = "alloc")]
    Compound(&'a Compound),
    #[cfg(feature = "dim2")]
    #[cfg(feature = "alloc")]
    ConvexPolygon(&'a ConvexPolygon),
    #[cfg(feature = "dim3")]
    #[cfg(feature = "alloc")]
    /// A convex polyhedron.
    ConvexPolyhedron(&'a ConvexPolyhedron),
    #[cfg(feature = "dim3")]
    /// A cylindrical shape.
    Cylinder(&'a Cylinder),
    #[cfg(feature = "dim3")]
    /// A cone shape.
    Cone(&'a Cone),
    /// A cuboid with rounded corners.
    RoundCuboid(&'a RoundCuboid),
    /// A triangle with rounded corners.
    RoundTriangle(&'a RoundTriangle),
    // /// A triangle-mesh with rounded corners.
    // RoundedTriMesh,
    // /// An heightfield with rounded corners.
    // RoundedHeightField,
    /// A cylinder with rounded corners.
    #[cfg(feature = "dim3")]
    RoundCylinder(&'a RoundCylinder),
    /// A cone with rounded corners.
    #[cfg(feature = "dim3")]
    RoundCone(&'a RoundCone),
    /// A convex polyhedron with rounded corners.
    #[cfg(feature = "dim3")]
    #[cfg(feature = "alloc")]
    RoundConvexPolyhedron(&'a RoundConvexPolyhedron),
    /// A convex polygon with rounded corners.
    #[cfg(feature = "dim2")]
    #[cfg(feature = "alloc")]
    RoundConvexPolygon(&'a RoundConvexPolygon),
    /// A custom user-defined shape.
    #[cfg_attr(feature = "serde-serialize", serde(skip))]
    Custom(&'a dyn Shape),
}
impl Debug for TypedShape<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Ball(arg0) => f.debug_tuple("Ball").field(arg0).finish(),
            Self::Cuboid(arg0) => f.debug_tuple("Cuboid").field(arg0).finish(),
            Self::Capsule(arg0) => f.debug_tuple("Capsule").field(arg0).finish(),
            Self::Segment(arg0) => f.debug_tuple("Segment").field(arg0).finish(),
            Self::Triangle(arg0) => f.debug_tuple("Triangle").field(arg0).finish(),
            #[cfg(feature = "alloc")]
            Self::Voxels(arg0) => f.debug_tuple("Voxels").field(arg0).finish(),
            #[cfg(feature = "alloc")]
            Self::TriMesh(arg0) => f.debug_tuple("TriMesh").field(arg0).finish(),
            #[cfg(feature = "alloc")]
            Self::Polyline(arg0) => f.debug_tuple("Polyline").field(arg0).finish(),
            Self::HalfSpace(arg0) => f.debug_tuple("HalfSpace").field(arg0).finish(),
            #[cfg(feature = "alloc")]
            Self::HeightField(arg0) => f.debug_tuple("HeightField").field(arg0).finish(),
            #[cfg(feature = "alloc")]
            Self::Compound(arg0) => f.debug_tuple("Compound").field(arg0).finish(),
            #[cfg(feature = "dim2")]
            #[cfg(feature = "alloc")]
            Self::ConvexPolygon(arg0) => f.debug_tuple("ConvexPolygon").field(arg0).finish(),
            #[cfg(feature = "dim3")]
            #[cfg(feature = "alloc")]
            Self::ConvexPolyhedron(arg0) => f.debug_tuple("ConvexPolyhedron").field(arg0).finish(),
            #[cfg(feature = "dim3")]
            Self::Cylinder(arg0) => f.debug_tuple("Cylinder").field(arg0).finish(),
            #[cfg(feature = "dim3")]
            Self::Cone(arg0) => f.debug_tuple("Cone").field(arg0).finish(),
            Self::RoundCuboid(arg0) => f.debug_tuple("RoundCuboid").field(arg0).finish(),
            Self::RoundTriangle(arg0) => f.debug_tuple("RoundTriangle").field(arg0).finish(),
            #[cfg(feature = "dim3")]
            Self::RoundCylinder(arg0) => f.debug_tuple("RoundCylinder").field(arg0).finish(),
            #[cfg(feature = "dim3")]
            Self::RoundCone(arg0) => f.debug_tuple("RoundCone").field(arg0).finish(),
            #[cfg(feature = "dim3")]
            #[cfg(feature = "alloc")]
            Self::RoundConvexPolyhedron(arg0) => {
                f.debug_tuple("RoundConvexPolyhedron").field(arg0).finish()
            }
            #[cfg(feature = "dim2")]
            #[cfg(feature = "alloc")]
            Self::RoundConvexPolygon(arg0) => {
                f.debug_tuple("RoundConvexPolygon").field(arg0).finish()
            }
            Self::Custom(_) => f.debug_tuple("Custom").finish(),
        }
    }
}

#[cfg(feature = "serde-serialize")]
#[derive(Deserialize)]
// NOTE: This enum MUST match the `TypedShape` enum.
/// Enum representing the shape with its actual type
pub(crate) enum DeserializableTypedShape {
    /// A ball shape.
    Ball(Ball),
    /// A cuboid shape.
    Cuboid(Cuboid),
    /// A capsule shape.
    Capsule(Capsule),
    /// A segment shape.
    Segment(Segment),
    /// A triangle shape.
    Triangle(Triangle),
    /// A shape defined as a voxel grid.
    #[cfg(feature = "alloc")]
    Voxels(Voxels),
    /// A triangle mesh shape.
    #[cfg(feature = "alloc")]
    TriMesh(TriMesh),
    /// A set of segments.
    #[cfg(feature = "alloc")]
    Polyline(Polyline),
    /// A shape representing a full half-space.
    HalfSpace(HalfSpace),
    /// A heightfield shape.
    #[cfg(feature = "alloc")]
    HeightField(HeightField),
    /// A Compound shape.
    #[cfg(feature = "alloc")]
    Compound(Compound),
    #[cfg(feature = "dim2")]
    #[cfg(feature = "alloc")]
    ConvexPolygon(ConvexPolygon),
    #[cfg(feature = "dim3")]
    #[cfg(feature = "alloc")]
    /// A convex polyhedron.
    ConvexPolyhedron(ConvexPolyhedron),
    #[cfg(feature = "dim3")]
    /// A cylindrical shape.
    Cylinder(Cylinder),
    #[cfg(feature = "dim3")]
    /// A cone shape.
    Cone(Cone),
    // /// A custom shape type.
    // Custom(u8),
    /// A cuboid with rounded corners.
    RoundCuboid(RoundCuboid),
    /// A triangle with rounded corners.
    RoundTriangle(RoundTriangle),
    // /// A triangle-mesh with rounded corners.
    // RoundedTriMesh,
    // /// An heightfield with rounded corners.
    // RoundedHeightField,
    /// A cylinder with rounded corners.
    #[cfg(feature = "dim3")]
    RoundCylinder(RoundCylinder),
    /// A cone with rounded corners.
    #[cfg(feature = "dim3")]
    RoundCone(RoundCone),
    /// A convex polyhedron with rounded corners.
    #[cfg(feature = "dim3")]
    #[cfg(feature = "alloc")]
    RoundConvexPolyhedron(RoundConvexPolyhedron),
    /// A convex polygon with rounded corners.
    #[cfg(feature = "dim2")]
    #[cfg(feature = "alloc")]
    RoundConvexPolygon(RoundConvexPolygon),
    /// A custom user-defined shape.
    #[allow(dead_code)]
    Custom,
}

#[cfg(feature = "serde-serialize")]
impl DeserializableTypedShape {
    /// Converts `self` to a `SharedShape` if `self` isn't `Custom`.
    pub fn into_shared_shape(self) -> Option<SharedShape> {
        match self {
            DeserializableTypedShape::Ball(s) => Some(SharedShape::new(s)),
            DeserializableTypedShape::Cuboid(s) => Some(SharedShape::new(s)),
            DeserializableTypedShape::Capsule(s) => Some(SharedShape::new(s)),
            DeserializableTypedShape::Segment(s) => Some(SharedShape::new(s)),
            DeserializableTypedShape::Triangle(s) => Some(SharedShape::new(s)),
            #[cfg(feature = "alloc")]
            DeserializableTypedShape::Voxels(s) => Some(SharedShape::new(s)),
            #[cfg(feature = "alloc")]
            DeserializableTypedShape::TriMesh(s) => Some(SharedShape::new(s)),
            #[cfg(feature = "alloc")]
            DeserializableTypedShape::Polyline(s) => Some(SharedShape::new(s)),
            DeserializableTypedShape::HalfSpace(s) => Some(SharedShape::new(s)),
            #[cfg(feature = "alloc")]
            DeserializableTypedShape::HeightField(s) => Some(SharedShape::new(s)),
            #[cfg(feature = "alloc")]
            DeserializableTypedShape::Compound(s) => Some(SharedShape::new(s)),
            #[cfg(feature = "dim2")]
            #[cfg(feature = "alloc")]
            DeserializableTypedShape::ConvexPolygon(s) => Some(SharedShape::new(s)),
            #[cfg(feature = "dim3")]
            #[cfg(feature = "alloc")]
            DeserializableTypedShape::ConvexPolyhedron(s) => Some(SharedShape::new(s)),
            #[cfg(feature = "dim3")]
            DeserializableTypedShape::Cylinder(s) => Some(SharedShape::new(s)),
            #[cfg(feature = "dim3")]
            DeserializableTypedShape::Cone(s) => Some(SharedShape::new(s)),
            DeserializableTypedShape::RoundCuboid(s) => Some(SharedShape::new(s)),
            DeserializableTypedShape::RoundTriangle(s) => Some(SharedShape::new(s)),
            #[cfg(feature = "dim3")]
            DeserializableTypedShape::RoundCylinder(s) => Some(SharedShape::new(s)),
            #[cfg(feature = "dim3")]
            DeserializableTypedShape::RoundCone(s) => Some(SharedShape::new(s)),
            #[cfg(feature = "dim3")]
            #[cfg(feature = "alloc")]
            DeserializableTypedShape::RoundConvexPolyhedron(s) => Some(SharedShape::new(s)),
            #[cfg(feature = "dim2")]
            #[cfg(feature = "alloc")]
            DeserializableTypedShape::RoundConvexPolygon(s) => Some(SharedShape::new(s)),
            DeserializableTypedShape::Custom => None,
        }
    }
}

/// Trait implemented by shapes usable by Rapier.
pub trait Shape: RayCast + PointQuery + DowncastSync {
    /// Computes the [`Aabb`] of this shape.
    fn compute_local_aabb(&self) -> Aabb;
    /// Computes the bounding-sphere of this shape.
    fn compute_local_bounding_sphere(&self) -> BoundingSphere;

    /// Clones this shape into a boxed trait-object.
    ///
    /// The boxed trait-object has the same concrete type as `Self`.
    #[cfg(feature = "alloc")]
    #[deprecated = "renamed to `clone_dyn`"]
    fn clone_box(&self) -> Box<dyn Shape> {
        self.clone_dyn()
    }

    /// Clones this shape into a boxed trait-object.
    ///
    /// The boxed trait-object has the same concrete type as `Self`.
    #[cfg(feature = "alloc")]
    fn clone_dyn(&self) -> Box<dyn Shape>;

    /// Scales this shape by `scale` into a boxed trait-object.
    ///
    /// In some cases, the resulting shape doesn’t have the same type as Self. For example,
    /// if a non-uniform scale is provided and Self as a [`Ball`], then the result will be discretized
    /// (based on the `num_subdivisions` parameter) as a `ConvexPolyhedron` (in 3D) or `ConvexPolygon` (in 2D).
    #[cfg(feature = "alloc")]
    fn scale_dyn(&self, scale: Vector, num_subdivisions: u32) -> Option<Box<dyn Shape>>;

    /// Computes the [`Aabb`] of this shape with the given position.
    fn compute_aabb(&self, position: &Pose) -> Aabb {
        self.compute_local_aabb().transform_by(position)
    }
    /// Computes the bounding-sphere of this shape with the given position.
    fn compute_bounding_sphere(&self, position: &Pose) -> BoundingSphere {
        self.compute_local_bounding_sphere().transform_by(position)
    }

    /// Compute the mass-properties of this shape given its uniform density.
    fn mass_properties(&self, density: Real) -> MassProperties;

    /// Gets the type tag of this shape.
    fn shape_type(&self) -> ShapeType;

    /// Gets the underlying shape as an enum.
    fn as_typed_shape(&self) -> TypedShape<'_>;

    fn ccd_thickness(&self) -> Real;

    // TODO: document this.
    // This should probably be the largest sharp edge angle (in radians) in [0; PI].
    // Though this isn't a very good description considering this is PI / 2
    // for capsule (which doesn't have any sharp angle). I guess a better way
    // to phrase this is: "the smallest angle such that rotating the shape by
    // that angle may result in different contact points".
    fn ccd_angular_thickness(&self) -> Real;

    /// Is this shape known to be convex?
    ///
    /// If this returns `true` then `self` is known to be convex.
    /// If this returns `false` then it is not known whether or
    /// not `self` is convex.
    fn is_convex(&self) -> bool {
        false
    }

    /// Converts this shape into its support mapping, if it has one.
    fn as_support_map(&self) -> Option<&dyn SupportMap> {
        None
    }

    #[cfg(feature = "alloc")]
    fn as_composite_shape(&self) -> Option<&dyn CompositeShape> {
        None
    }

    /// Converts this shape to a polygonal feature-map, if it is one.
    fn as_polygonal_feature_map(&self) -> Option<(&dyn PolygonalFeatureMap, Real)> {
        None
    }

    // fn as_rounded(&self) -> Option<&Rounded<Box<AnyShape>>> {
    //     None
    // }

    /// The shape's normal at the given point located on a specific feature.
    fn feature_normal_at_point(&self, _feature: FeatureId, _point: Vector) -> Option<Vector> {
        None
    }

    /// Computes the swept [`Aabb`] of this shape, i.e., the space it would occupy by moving from
    /// the given start position to the given end position.
    fn compute_swept_aabb(&self, start_pos: &Pose, end_pos: &Pose) -> Aabb {
        let aabb1 = self.compute_aabb(start_pos);
        let aabb2 = self.compute_aabb(end_pos);
        aabb1.merged(&aabb2)
    }
}

impl_downcast!(sync Shape);

impl dyn Shape {
    /// Converts this abstract shape to the given shape, if it is one.
    pub fn as_shape<T: Shape>(&self) -> Option<&T> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to the given mutable shape, if it is one.
    pub fn as_shape_mut<T: Shape>(&mut self) -> Option<&mut T> {
        self.downcast_mut()
    }

    /// Converts this abstract shape to a ball, if it is one.
    pub fn as_ball(&self) -> Option<&Ball> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to a mutable ball, if it is one.
    pub fn as_ball_mut(&mut self) -> Option<&mut Ball> {
        self.downcast_mut()
    }

    /// Converts this abstract shape to a cuboid, if it is one.
    pub fn as_cuboid(&self) -> Option<&Cuboid> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to a mutable cuboid, if it is one.
    pub fn as_cuboid_mut(&mut self) -> Option<&mut Cuboid> {
        self.downcast_mut()
    }

    /// Converts this abstract shape to a halfspace, if it is one.
    pub fn as_halfspace(&self) -> Option<&HalfSpace> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to a halfspace, if it is one.
    pub fn as_halfspace_mut(&mut self) -> Option<&mut HalfSpace> {
        self.downcast_mut()
    }

    /// Converts this abstract shape to a segment, if it is one.
    pub fn as_segment(&self) -> Option<&Segment> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to a mutable segment, if it is one.
    pub fn as_segment_mut(&mut self) -> Option<&mut Segment> {
        self.downcast_mut()
    }

    /// Converts this abstract shape to a capsule, if it is one.
    pub fn as_capsule(&self) -> Option<&Capsule> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to a mutable capsule, if it is one.
    pub fn as_capsule_mut(&mut self) -> Option<&mut Capsule> {
        self.downcast_mut()
    }

    /// Converts this abstract shape to a triangle, if it is one.
    pub fn as_triangle(&self) -> Option<&Triangle> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to a mutable triangle, if it is one.
    pub fn as_triangle_mut(&mut self) -> Option<&mut Triangle> {
        self.downcast_mut()
    }

    /// Converts this abstract shape to voxels, if it is one.
    #[cfg(feature = "alloc")]
    pub fn as_voxels(&self) -> Option<&Voxels> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to mutable voxels, if it is one.
    #[cfg(feature = "alloc")]
    pub fn as_voxels_mut(&mut self) -> Option<&mut Voxels> {
        self.downcast_mut()
    }

    /// Converts this abstract shape to a compound shape, if it is one.
    #[cfg(feature = "alloc")]
    pub fn as_compound(&self) -> Option<&Compound> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to a mutable compound shape, if it is one.
    #[cfg(feature = "alloc")]
    pub fn as_compound_mut(&mut self) -> Option<&mut Compound> {
        self.downcast_mut()
    }

    /// Converts this abstract shape to a triangle mesh, if it is one.
    #[cfg(feature = "alloc")]
    pub fn as_trimesh(&self) -> Option<&TriMesh> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to a mutable triangle mesh, if it is one.
    #[cfg(feature = "alloc")]
    pub fn as_trimesh_mut(&mut self) -> Option<&mut TriMesh> {
        self.downcast_mut()
    }

    /// Converts this abstract shape to a polyline, if it is one.
    #[cfg(feature = "alloc")]
    pub fn as_polyline(&self) -> Option<&Polyline> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to a mutable polyline, if it is one.
    #[cfg(feature = "alloc")]
    pub fn as_polyline_mut(&mut self) -> Option<&mut Polyline> {
        self.downcast_mut()
    }

    /// Converts this abstract shape to a heightfield, if it is one.
    #[cfg(feature = "alloc")]
    pub fn as_heightfield(&self) -> Option<&HeightField> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to a mutable heightfield, if it is one.
    #[cfg(feature = "alloc")]
    pub fn as_heightfield_mut(&mut self) -> Option<&mut HeightField> {
        self.downcast_mut()
    }

    /// Converts this abstract shape to a round cuboid, if it is one.
    pub fn as_round_cuboid(&self) -> Option<&RoundCuboid> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to a mutable round cuboid, if it is one.
    pub fn as_round_cuboid_mut(&mut self) -> Option<&mut RoundCuboid> {
        self.downcast_mut()
    }

    /// Converts this abstract shape to a round triangle, if it is one.
    pub fn as_round_triangle(&self) -> Option<&RoundTriangle> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to a round triangle, if it is one.
    pub fn as_round_triangle_mut(&mut self) -> Option<&mut RoundTriangle> {
        self.downcast_mut()
    }

    /// Converts this abstract shape to a convex polygon, if it is one.
    #[cfg(feature = "dim2")]
    #[cfg(feature = "alloc")]
    pub fn as_convex_polygon(&self) -> Option<&ConvexPolygon> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to a mutable convex polygon, if it is one.
    #[cfg(feature = "dim2")]
    #[cfg(feature = "alloc")]
    pub fn as_convex_polygon_mut(&mut self) -> Option<&mut ConvexPolygon> {
        self.downcast_mut()
    }

    /// Converts this abstract shape to a round convex polygon, if it is one.
    #[cfg(feature = "dim2")]
    #[cfg(feature = "alloc")]
    pub fn as_round_convex_polygon(&self) -> Option<&RoundConvexPolygon> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to a mutable round convex polygon, if it is one.
    #[cfg(feature = "dim2")]
    #[cfg(feature = "alloc")]
    pub fn as_round_convex_polygon_mut(&mut self) -> Option<&mut RoundConvexPolygon> {
        self.downcast_mut()
    }

    #[cfg(feature = "dim3")]
    #[cfg(feature = "alloc")]
    pub fn as_convex_polyhedron(&self) -> Option<&ConvexPolyhedron> {
        self.downcast_ref()
    }
    #[cfg(feature = "dim3")]
    #[cfg(feature = "alloc")]
    pub fn as_convex_polyhedron_mut(&mut self) -> Option<&mut ConvexPolyhedron> {
        self.downcast_mut()
    }

    /// Converts this abstract shape to a cylinder, if it is one.
    #[cfg(feature = "dim3")]
    pub fn as_cylinder(&self) -> Option<&Cylinder> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to a mutable cylinder, if it is one.
    #[cfg(feature = "dim3")]
    pub fn as_cylinder_mut(&mut self) -> Option<&mut Cylinder> {
        self.downcast_mut()
    }

    /// Converts this abstract shape to a cone, if it is one.
    #[cfg(feature = "dim3")]
    pub fn as_cone(&self) -> Option<&Cone> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to a mutable cone, if it is one.
    #[cfg(feature = "dim3")]
    pub fn as_cone_mut(&mut self) -> Option<&mut Cone> {
        self.downcast_mut()
    }

    /// Converts this abstract shape to a round cylinder, if it is one.
    #[cfg(feature = "dim3")]
    pub fn as_round_cylinder(&self) -> Option<&RoundCylinder> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to a mutable round cylinder, if it is one.
    #[cfg(feature = "dim3")]
    pub fn as_round_cylinder_mut(&mut self) -> Option<&mut RoundCylinder> {
        self.downcast_mut()
    }

    /// Converts this abstract shape to a round cone, if it is one.
    #[cfg(feature = "dim3")]
    pub fn as_round_cone(&self) -> Option<&RoundCone> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to a mutable round cone, if it is one.
    #[cfg(feature = "dim3")]
    pub fn as_round_cone_mut(&mut self) -> Option<&mut RoundCone> {
        self.downcast_mut()
    }

    /// Converts this abstract shape to a round convex polyhedron, if it is one.
    #[cfg(feature = "dim3")]
    #[cfg(feature = "alloc")]
    pub fn as_round_convex_polyhedron(&self) -> Option<&RoundConvexPolyhedron> {
        self.downcast_ref()
    }
    /// Converts this abstract shape to a mutable round convex polyhedron, if it is one.
    #[cfg(feature = "dim3")]
    #[cfg(feature = "alloc")]
    pub fn as_round_convex_polyhedron_mut(&mut self) -> Option<&mut RoundConvexPolyhedron> {
        self.downcast_mut()
    }
}

impl Shape for Ball {
    #[cfg(feature = "alloc")]
    fn clone_dyn(&self) -> Box<dyn Shape> {
        Box::new(*self)
    }

    #[cfg(feature = "alloc")]
    fn scale_dyn(&self, scale: Vector, num_subdivisions: u32) -> Option<Box<dyn Shape>> {
        let scaled = self.scaled(scale, num_subdivisions)?;
        Some(scaled.either::<_, _, Box<dyn Shape>>(|x| Box::new(x), |x| Box::new(x)))
    }

    fn compute_local_aabb(&self) -> Aabb {
        self.local_aabb()
    }

    fn compute_local_bounding_sphere(&self) -> BoundingSphere {
        self.local_bounding_sphere()
    }

    fn compute_aabb(&self, position: &Pose) -> Aabb {
        self.aabb(position)
    }

    fn mass_properties(&self, density: Real) -> MassProperties {
        MassProperties::from_ball(density, self.radius)
    }

    fn ccd_thickness(&self) -> Real {
        self.radius
    }

    fn ccd_angular_thickness(&self) -> Real {
        Real::pi()
    }

    fn is_convex(&self) -> bool {
        true
    }

    fn shape_type(&self) -> ShapeType {
        ShapeType::Ball
    }

    fn as_typed_shape(&self) -> TypedShape<'_> {
        TypedShape::Ball(self)
    }

    fn as_support_map(&self) -> Option<&dyn SupportMap> {
        Some(self as &dyn SupportMap)
    }

    /// The shape's normal at the given point located on a specific feature.
    #[inline]
    fn feature_normal_at_point(&self, _: FeatureId, point: Vector) -> Option<Vector> {
        (point).try_normalize()
    }
}

impl Shape for Cuboid {
    #[cfg(feature = "alloc")]
    fn clone_dyn(&self) -> Box<dyn Shape> {
        Box::new(*self)
    }

    #[cfg(feature = "alloc")]
    fn scale_dyn(&self, scale: Vector, _num_subdivisions: u32) -> Option<Box<dyn Shape>> {
        Some(Box::new(self.scaled(scale)))
    }

    fn compute_local_aabb(&self) -> Aabb {
        self.local_aabb()
    }

    fn compute_local_bounding_sphere(&self) -> BoundingSphere {
        self.local_bounding_sphere()
    }

    fn compute_aabb(&self, position: &Pose) -> Aabb {
        self.aabb(position)
    }

    fn mass_properties(&self, density: Real) -> MassProperties {
        MassProperties::from_cuboid(density, self.half_extents)
    }

    fn is_convex(&self) -> bool {
        true
    }

    fn shape_type(&self) -> ShapeType {
        ShapeType::Cuboid
    }

    fn as_typed_shape(&self) -> TypedShape<'_> {
        TypedShape::Cuboid(self)
    }

    fn ccd_thickness(&self) -> Real {
        self.half_extents.min_element()
    }

    fn ccd_angular_thickness(&self) -> Real {
        Real::frac_pi_2()
    }

    fn as_support_map(&self) -> Option<&dyn SupportMap> {
        Some(self as &dyn SupportMap)
    }

    fn as_polygonal_feature_map(&self) -> Option<(&dyn PolygonalFeatureMap, Real)> {
        Some((self as &dyn PolygonalFeatureMap, 0.0))
    }

    fn feature_normal_at_point(&self, feature: FeatureId, _point: Vector) -> Option<Vector> {
        self.feature_normal(feature)
    }
}

impl Shape for Capsule {
    #[cfg(feature = "alloc")]
    fn clone_dyn(&self) -> Box<dyn Shape> {
        Box::new(*self)
    }

    #[cfg(feature = "alloc")]
    fn scale_dyn(&self, scale: Vector, num_subdivisions: u32) -> Option<Box<dyn Shape>> {
        let scaled = self.scaled(scale, num_subdivisions)?;
        Some(scaled.either::<_, _, Box<dyn Shape>>(|x| Box::new(x), |x| Box::new(x)))
    }

    fn compute_local_aabb(&self) -> Aabb {
        self.local_aabb()
    }

    fn compute_local_bounding_sphere(&self) -> BoundingSphere {
        self.local_bounding_sphere()
    }

    fn compute_aabb(&self, position: &Pose) -> Aabb {
        self.aabb(position)
    }

    fn mass_properties(&self, density: Real) -> MassProperties {
        MassProperties::from_capsule(density, self.segment.a, self.segment.b, self.radius)
    }

    fn is_convex(&self) -> bool {
        true
    }

    fn shape_type(&self) -> ShapeType {
        ShapeType::Capsule
    }

    fn as_typed_shape(&self) -> TypedShape<'_> {
        TypedShape::Capsule(self)
    }

    fn ccd_thickness(&self) -> Real {
        self.radius
    }

    fn ccd_angular_thickness(&self) -> Real {
        Real::frac_pi_2()
    }

    fn as_support_map(&self) -> Option<&dyn SupportMap> {
        Some(self as &dyn SupportMap)
    }

    fn as_polygonal_feature_map(&self) -> Option<(&dyn PolygonalFeatureMap, Real)> {
        Some((&self.segment as &dyn PolygonalFeatureMap, self.radius))
    }
}

impl Shape for Triangle {
    #[cfg(feature = "alloc")]
    fn clone_dyn(&self) -> Box<dyn Shape> {
        Box::new(*self)
    }

    #[cfg(feature = "alloc")]
    fn scale_dyn(&self, scale: Vector, _num_subdivisions: u32) -> Option<Box<dyn Shape>> {
        Some(Box::new(self.scaled(scale)))
    }

    fn compute_local_aabb(&self) -> Aabb {
        self.local_aabb()
    }

    fn compute_local_bounding_sphere(&self) -> BoundingSphere {
        self.local_bounding_sphere()
    }

    fn compute_aabb(&self, position: &Pose) -> Aabb {
        self.aabb(position)
    }

    fn mass_properties(&self, _density: Real) -> MassProperties {
        #[cfg(feature = "dim2")]
        return MassProperties::from_triangle(_density, self.a, self.b, self.c);
        #[cfg(feature = "dim3")]
        return MassProperties::zero();
    }

    fn is_convex(&self) -> bool {
        true
    }

    fn shape_type(&self) -> ShapeType {
        ShapeType::Triangle
    }

    fn as_typed_shape(&self) -> TypedShape<'_> {
        TypedShape::Triangle(self)
    }

    fn ccd_thickness(&self) -> Real {
        // TODO: in 2D use the smallest height of the triangle.
        0.0
    }

    fn ccd_angular_thickness(&self) -> Real {
        Real::frac_pi_2()
    }

    fn as_support_map(&self) -> Option<&dyn SupportMap> {
        Some(self as &dyn SupportMap)
    }

    fn as_polygonal_feature_map(&self) -> Option<(&dyn PolygonalFeatureMap, Real)> {
        Some((self as &dyn PolygonalFeatureMap, 0.0))
    }

    fn feature_normal_at_point(&self, _feature: FeatureId, _point: Vector) -> Option<Vector> {
        #[cfg(feature = "dim2")]
        return None;
        #[cfg(feature = "dim3")]
        return self.feature_normal(_feature);
    }
}

impl Shape for Segment {
    #[cfg(feature = "alloc")]
    fn clone_dyn(&self) -> Box<dyn Shape> {
        Box::new(*self)
    }

    #[cfg(feature = "alloc")]
    fn scale_dyn(&self, scale: Vector, _num_subdivisions: u32) -> Option<Box<dyn Shape>> {
        Some(Box::new(self.scaled(scale)))
    }

    fn compute_local_aabb(&self) -> Aabb {
        self.local_aabb()
    }

    fn compute_local_bounding_sphere(&self) -> BoundingSphere {
        self.local_bounding_sphere()
    }

    fn compute_aabb(&self, position: &Pose) -> Aabb {
        self.aabb(position)
    }

    fn mass_properties(&self, _density: Real) -> MassProperties {
        MassProperties::zero()
    }

    fn is_convex(&self) -> bool {
        true
    }

    fn ccd_thickness(&self) -> Real {
        0.0
    }

    fn ccd_angular_thickness(&self) -> Real {
        Real::frac_pi_2()
    }

    fn shape_type(&self) -> ShapeType {
        ShapeType::Segment
    }

    fn as_typed_shape(&self) -> TypedShape<'_> {
        TypedShape::Segment(self)
    }

    fn as_support_map(&self) -> Option<&dyn SupportMap> {
        Some(self as &dyn SupportMap)
    }

    fn as_polygonal_feature_map(&self) -> Option<(&dyn PolygonalFeatureMap, Real)> {
        Some((self as &dyn PolygonalFeatureMap, 0.0))
    }

    fn feature_normal_at_point(&self, feature: FeatureId, _point: Vector) -> Option<Vector> {
        self.feature_normal(feature)
    }
}

#[cfg(feature = "alloc")]
impl Shape for Compound {
    fn clone_dyn(&self) -> Box<dyn Shape> {
        Box::new(self.clone())
    }

    fn scale_dyn(&self, scale: Vector, num_subdivisions: u32) -> Option<Box<dyn Shape>> {
        use super::SharedShape;

        let scaled: Vec<_> = self
            .shapes()
            .iter()
            .map(|(pos, shape)| {
                let scaled_shape = shape.scale_dyn(scale, num_subdivisions)?;
                Some((
                    Pose::from_parts(pos.translation * scale, pos.rotation),
                    SharedShape(scaled_shape.into()),
                ))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Box::new(Compound::new(scaled)))
    }

    fn compute_local_aabb(&self) -> Aabb {
        *self.local_aabb()
    }

    fn compute_local_bounding_sphere(&self) -> BoundingSphere {
        self.local_bounding_sphere()
    }

    fn compute_aabb(&self, position: &Pose) -> Aabb {
        self.local_aabb().transform_by(position)
    }

    fn mass_properties(&self, density: Real) -> MassProperties {
        MassProperties::from_compound(density, self.shapes())
    }

    fn shape_type(&self) -> ShapeType {
        ShapeType::Compound
    }

    fn as_typed_shape(&self) -> TypedShape<'_> {
        TypedShape::Compound(self)
    }

    fn ccd_thickness(&self) -> Real {
        self.shapes()
            .iter()
            .fold(Real::MAX, |curr, (_, s)| curr.min(s.ccd_thickness()))
    }

    fn ccd_angular_thickness(&self) -> Real {
        self.shapes().iter().fold(Real::MAX, |curr, (_, s)| {
            curr.max(s.ccd_angular_thickness())
        })
    }

    #[cfg(feature = "alloc")]
    fn as_composite_shape(&self) -> Option<&dyn CompositeShape> {
        Some(self as &dyn CompositeShape)
    }
}

#[cfg(feature = "alloc")]
impl Shape for Polyline {
    fn clone_dyn(&self) -> Box<dyn Shape> {
        Box::new(self.clone())
    }

    fn scale_dyn(&self, scale: Vector, _num_subdivisions: u32) -> Option<Box<dyn Shape>> {
        Some(Box::new(self.clone().scaled(scale)))
    }

    fn compute_local_aabb(&self) -> Aabb {
        self.local_aabb()
    }

    fn compute_local_bounding_sphere(&self) -> BoundingSphere {
        self.local_bounding_sphere()
    }

    fn compute_aabb(&self, position: &Pose) -> Aabb {
        self.aabb(position)
    }

    fn mass_properties(&self, _density: Real) -> MassProperties {
        MassProperties::zero()
    }

    fn shape_type(&self) -> ShapeType {
        ShapeType::Polyline
    }

    fn as_typed_shape(&self) -> TypedShape<'_> {
        TypedShape::Polyline(self)
    }

    fn ccd_thickness(&self) -> Real {
        0.0
    }

    fn ccd_angular_thickness(&self) -> Real {
        // TODO: the value should depend on the angles between
        // adjacent segments of the polyline.
        Real::frac_pi_4()
    }

    #[cfg(feature = "alloc")]
    fn as_composite_shape(&self) -> Option<&dyn CompositeShape> {
        Some(self as &dyn CompositeShape)
    }
}

#[cfg(feature = "alloc")]
impl Shape for TriMesh {
    fn clone_dyn(&self) -> Box<dyn Shape> {
        Box::new(self.clone())
    }

    fn scale_dyn(&self, scale: Vector, _num_subdivisions: u32) -> Option<Box<dyn Shape>> {
        Some(Box::new(self.clone().scaled(scale)))
    }

    fn compute_local_aabb(&self) -> Aabb {
        self.local_aabb()
    }

    fn compute_local_bounding_sphere(&self) -> BoundingSphere {
        self.local_bounding_sphere()
    }

    fn compute_aabb(&self, position: &Pose) -> Aabb {
        self.aabb(position)
    }

    fn mass_properties(&self, density: Real) -> MassProperties {
        MassProperties::from_trimesh(density, self.vertices(), self.indices())
    }

    fn shape_type(&self) -> ShapeType {
        ShapeType::TriMesh
    }

    fn as_typed_shape(&self) -> TypedShape<'_> {
        TypedShape::TriMesh(self)
    }

    fn ccd_thickness(&self) -> Real {
        // TODO: in 2D, return the smallest CCD thickness among triangles?
        0.0
    }

    fn ccd_angular_thickness(&self) -> Real {
        // TODO: the value should depend on the angles between
        // adjacent triangles of the trimesh.
        Real::frac_pi_4()
    }

    /// Gets the normal of the triangle represented by `feature`.
    fn feature_normal_at_point(&self, _feature: FeatureId, _point: Vector) -> Option<Vector> {
        #[cfg(feature = "dim2")]
        return None;
        #[cfg(feature = "dim3")]
        return self.feature_normal(_feature);
    }

    #[cfg(feature = "alloc")]
    fn as_composite_shape(&self) -> Option<&dyn CompositeShape> {
        Some(self as &dyn CompositeShape)
    }
}

#[cfg(feature = "alloc")]
impl Shape for HeightField {
    fn clone_dyn(&self) -> Box<dyn Shape> {
        Box::new(self.clone())
    }

    fn scale_dyn(&self, scale: Vector, _num_subdivisions: u32) -> Option<Box<dyn Shape>> {
        Some(Box::new(self.clone().scaled(scale)))
    }

    fn compute_local_aabb(&self) -> Aabb {
        self.local_aabb()
    }

    fn compute_local_bounding_sphere(&self) -> BoundingSphere {
        self.local_bounding_sphere()
    }

    fn compute_aabb(&self, position: &Pose) -> Aabb {
        self.aabb(position)
    }

    fn mass_properties(&self, _density: Real) -> MassProperties {
        MassProperties::zero()
    }

    fn shape_type(&self) -> ShapeType {
        ShapeType::HeightField
    }

    fn as_typed_shape(&self) -> TypedShape<'_> {
        TypedShape::HeightField(self)
    }

    fn ccd_thickness(&self) -> Real {
        0.0
    }

    fn ccd_angular_thickness(&self) -> Real {
        // TODO: the value should depend on the angles between
        // adjacent triangles of the heightfield.
        Real::frac_pi_4()
    }
}

#[cfg(feature = "dim2")]
#[cfg(feature = "alloc")]
impl Shape for ConvexPolygon {
    fn clone_dyn(&self) -> Box<dyn Shape> {
        Box::new(self.clone())
    }

    fn scale_dyn(&self, scale: Vector, _num_subdivisions: u32) -> Option<Box<dyn Shape>> {
        Some(Box::new(self.clone().scaled(scale)?))
    }

    fn compute_local_aabb(&self) -> Aabb {
        self.local_aabb()
    }

    fn compute_local_bounding_sphere(&self) -> BoundingSphere {
        self.local_bounding_sphere()
    }

    fn compute_aabb(&self, position: &Pose) -> Aabb {
        self.aabb(position)
    }

    fn mass_properties(&self, density: Real) -> MassProperties {
        MassProperties::from_convex_polygon(density, self.points())
    }

    fn is_convex(&self) -> bool {
        true
    }

    fn shape_type(&self) -> ShapeType {
        ShapeType::ConvexPolygon
    }

    fn as_typed_shape(&self) -> TypedShape<'_> {
        TypedShape::ConvexPolygon(self)
    }

    fn ccd_thickness(&self) -> Real {
        // TODO: we should use the OBB instead.
        self.compute_local_aabb().half_extents().min_element()
    }

    fn ccd_angular_thickness(&self) -> Real {
        // TODO: the value should depend on the angles between
        // adjacent segments of the convex polygon.
        Real::frac_pi_4()
    }

    fn as_support_map(&self) -> Option<&dyn SupportMap> {
        Some(self as &dyn SupportMap)
    }

    fn as_polygonal_feature_map(&self) -> Option<(&dyn PolygonalFeatureMap, Real)> {
        Some((self as &dyn PolygonalFeatureMap, 0.0))
    }

    fn feature_normal_at_point(&self, feature: FeatureId, _point: Vector) -> Option<Vector> {
        self.feature_normal(feature)
    }
}

#[cfg(feature = "dim3")]
#[cfg(feature = "alloc")]
impl Shape for ConvexPolyhedron {
    fn clone_dyn(&self) -> Box<dyn Shape> {
        Box::new(self.clone())
    }

    fn scale_dyn(&self, scale: Vector, _num_subdivisions: u32) -> Option<Box<dyn Shape>> {
        Some(Box::new(self.clone().scaled(scale)?))
    }

    fn compute_local_aabb(&self) -> Aabb {
        self.local_aabb()
    }

    fn compute_local_bounding_sphere(&self) -> BoundingSphere {
        self.local_bounding_sphere()
    }

    fn compute_aabb(&self, position: &Pose) -> Aabb {
        self.aabb(position)
    }

    fn mass_properties(&self, density: Real) -> MassProperties {
        let (vertices, indices) = self.to_trimesh();
        MassProperties::from_convex_polyhedron(density, &vertices, &indices)
    }

    fn is_convex(&self) -> bool {
        true
    }

    fn shape_type(&self) -> ShapeType {
        ShapeType::ConvexPolyhedron
    }

    fn as_typed_shape(&self) -> TypedShape<'_> {
        TypedShape::ConvexPolyhedron(self)
    }

    fn ccd_thickness(&self) -> Real {
        // TODO: we should use the OBB instead.
        self.compute_local_aabb().half_extents().min_element()
    }

    fn ccd_angular_thickness(&self) -> Real {
        // TODO: the value should depend on the angles between
        // adjacent segments of the convex polyhedron.
        Real::frac_pi_4()
    }

    fn as_support_map(&self) -> Option<&dyn SupportMap> {
        Some(self as &dyn SupportMap)
    }

    fn as_polygonal_feature_map(&self) -> Option<(&dyn PolygonalFeatureMap, Real)> {
        Some((self as &dyn PolygonalFeatureMap, 0.0))
    }

    fn feature_normal_at_point(&self, feature: FeatureId, _point: Vector) -> Option<Vector> {
        self.feature_normal(feature)
    }
}

#[cfg(feature = "dim3")]
impl Shape for Cylinder {
    #[cfg(feature = "alloc")]
    fn clone_dyn(&self) -> Box<dyn Shape> {
        Box::new(*self)
    }

    #[cfg(feature = "alloc")]
    fn scale_dyn(&self, scale: Vector, num_subdivisions: u32) -> Option<Box<dyn Shape>> {
        let scaled = self.scaled(scale, num_subdivisions)?;
        Some(scaled.either::<_, _, Box<dyn Shape>>(|x| Box::new(x), |x| Box::new(x)))
    }

    fn compute_local_aabb(&self) -> Aabb {
        self.local_aabb()
    }

    fn compute_local_bounding_sphere(&self) -> BoundingSphere {
        self.local_bounding_sphere()
    }

    fn compute_aabb(&self, position: &Pose) -> Aabb {
        self.aabb(position)
    }

    fn mass_properties(&self, density: Real) -> MassProperties {
        MassProperties::from_cylinder(density, self.half_height, self.radius)
    }

    fn is_convex(&self) -> bool {
        true
    }

    fn shape_type(&self) -> ShapeType {
        ShapeType::Cylinder
    }

    fn as_typed_shape(&self) -> TypedShape<'_> {
        TypedShape::Cylinder(self)
    }

    fn ccd_thickness(&self) -> Real {
        self.radius
    }

    fn ccd_angular_thickness(&self) -> Real {
        Real::frac_pi_2()
    }

    fn as_support_map(&self) -> Option<&dyn SupportMap> {
        Some(self as &dyn SupportMap)
    }

    fn as_polygonal_feature_map(&self) -> Option<(&dyn PolygonalFeatureMap, Real)> {
        Some((self as &dyn PolygonalFeatureMap, 0.0))
    }
}

#[cfg(feature = "dim3")]
impl Shape for Cone {
    #[cfg(feature = "alloc")]
    fn clone_dyn(&self) -> Box<dyn Shape> {
        Box::new(*self)
    }

    #[cfg(feature = "alloc")]
    fn scale_dyn(&self, scale: Vector, num_subdivisions: u32) -> Option<Box<dyn Shape>> {
        let scaled = self.scaled(scale, num_subdivisions)?;
        Some(scaled.either::<_, _, Box<dyn Shape>>(|x| Box::new(x), |x| Box::new(x)))
    }

    fn compute_local_aabb(&self) -> Aabb {
        self.local_aabb()
    }

    fn compute_local_bounding_sphere(&self) -> BoundingSphere {
        self.local_bounding_sphere()
    }

    fn compute_aabb(&self, position: &Pose) -> Aabb {
        self.aabb(position)
    }

    fn mass_properties(&self, density: Real) -> MassProperties {
        MassProperties::from_cone(density, self.half_height, self.radius)
    }

    fn is_convex(&self) -> bool {
        true
    }

    fn shape_type(&self) -> ShapeType {
        ShapeType::Cone
    }

    fn as_typed_shape(&self) -> TypedShape<'_> {
        TypedShape::Cone(self)
    }

    fn ccd_thickness(&self) -> Real {
        self.radius
    }

    fn ccd_angular_thickness(&self) -> Real {
        let apex_half_angle = RealField::atan2(self.radius, self.half_height);
        assert!(apex_half_angle >= 0.0);
        let basis_angle = Real::frac_pi_2() - apex_half_angle;
        basis_angle.min(apex_half_angle * 2.0)
    }

    fn as_support_map(&self) -> Option<&dyn SupportMap> {
        Some(self as &dyn SupportMap)
    }

    fn as_polygonal_feature_map(&self) -> Option<(&dyn PolygonalFeatureMap, Real)> {
        Some((self as &dyn PolygonalFeatureMap, 0.0))
    }
}

impl Shape for HalfSpace {
    #[cfg(feature = "alloc")]
    fn clone_dyn(&self) -> Box<dyn Shape> {
        Box::new(*self)
    }

    #[cfg(feature = "alloc")]
    fn scale_dyn(&self, scale: Vector, _num_subdivisions: u32) -> Option<Box<dyn Shape>> {
        Some(Box::new(self.scaled(scale)?))
    }

    fn compute_local_aabb(&self) -> Aabb {
        self.local_aabb()
    }

    fn compute_local_bounding_sphere(&self) -> BoundingSphere {
        self.local_bounding_sphere()
    }

    fn compute_aabb(&self, position: &Pose) -> Aabb {
        self.aabb(position)
    }

    fn is_convex(&self) -> bool {
        true
    }

    fn ccd_thickness(&self) -> Real {
        #[cfg_attr(feature = "f32", expect(clippy::unnecessary_cast))]
        let result = f32::MAX as Real;
        result
    }

    fn ccd_angular_thickness(&self) -> Real {
        Real::pi()
    }

    fn mass_properties(&self, _: Real) -> MassProperties {
        MassProperties::zero()
    }

    fn shape_type(&self) -> ShapeType {
        ShapeType::HalfSpace
    }

    fn as_typed_shape(&self) -> TypedShape<'_> {
        TypedShape::HalfSpace(self)
    }
}

#[cfg(feature = "alloc")]
impl Shape for Voxels {
    fn compute_local_aabb(&self) -> Aabb {
        self.local_aabb()
    }

    fn compute_local_bounding_sphere(&self) -> BoundingSphere {
        self.local_bounding_sphere()
    }

    fn clone_dyn(&self) -> Box<dyn Shape> {
        Box::new(self.clone())
    }

    fn scale_dyn(&self, scale: Vector, _num_subdivisions: u32) -> Option<Box<dyn Shape>> {
        Some(Box::new(self.clone().scaled(scale)))
    }

    fn mass_properties(&self, density: Real) -> MassProperties {
        MassProperties::from_voxels(density, self)
    }

    fn shape_type(&self) -> ShapeType {
        ShapeType::Voxels
    }

    fn as_typed_shape(&self) -> TypedShape<'_> {
        TypedShape::Voxels(self)
    }

    fn ccd_thickness(&self) -> Real {
        self.voxel_size().min_element()
    }

    fn ccd_angular_thickness(&self) -> Real {
        Real::frac_pi_2()
    }
}

macro_rules! impl_shape_for_round_shape(
    ($S: ty, $Tag: ident, $t: tt) => {
        impl Shape for RoundShape<$S> {
            #[cfg(feature = "alloc")]
            fn clone_dyn(&self) -> Box<dyn Shape> {
                Box::new(self.clone())
            }

            #[cfg(feature = "alloc")]
            fn scale_dyn(&self, scale: Vector, num_subdivisions: u32) -> Option<Box<dyn Shape>> {
                $t(self, scale, num_subdivisions)
            }

            fn compute_local_aabb(&self) -> Aabb {
                self.inner_shape.local_aabb().loosened(self.border_radius)
            }

            fn compute_local_bounding_sphere(&self) -> BoundingSphere {
                self.inner_shape.local_bounding_sphere().loosened(self.border_radius)
            }

            fn compute_aabb(&self, position: &Pose) -> Aabb {
                self.inner_shape.aabb(position).loosened(self.border_radius)
            }

            fn mass_properties(&self, density: Real) -> MassProperties {
                self.inner_shape.mass_properties(density)
            }

            fn is_convex(&self) -> bool {
                self.inner_shape.is_convex()
            }

            fn shape_type(&self) -> ShapeType {
                ShapeType::$Tag
            }

            fn as_typed_shape(&self) -> TypedShape<'_> {
                TypedShape::$Tag(self)
            }

            fn ccd_thickness(&self) -> Real {
                self.inner_shape.ccd_thickness() + self.border_radius
            }

            fn ccd_angular_thickness(&self) -> Real {
                // The fact that the shape is round doesn't change anything
                // to the CCD angular thickness.
                self.inner_shape.ccd_angular_thickness()
            }

            fn as_support_map(&self) -> Option<&dyn SupportMap> {
                Some(self as &dyn SupportMap)
            }

            fn as_polygonal_feature_map(&self) -> Option<(&dyn PolygonalFeatureMap, Real)> {
                Some((&self.inner_shape as &dyn PolygonalFeatureMap, self.border_radius))
            }
        }
    }
);

impl_shape_for_round_shape!(
    Cuboid,
    RoundCuboid,
    (|this: &Self, scale: Vector, _num_subdivisions: u32| {
        let shape = RoundShape {
            border_radius: this.border_radius,
            inner_shape: this.inner_shape.scaled(scale),
        };
        Some(Box::new(shape) as Box<dyn Shape>)
    })
);

impl_shape_for_round_shape!(
    Triangle,
    RoundTriangle,
    (|this: &Self, scale: Vector, _num_subdivisions: u32| {
        let shape = RoundShape {
            border_radius: this.border_radius,
            inner_shape: this.inner_shape.scaled(scale),
        };
        Some(Box::new(shape) as Box<dyn Shape>)
    })
);

#[cfg(feature = "dim2")]
#[cfg(feature = "alloc")]
impl_shape_for_round_shape!(
    ConvexPolygon,
    RoundConvexPolygon,
    (|this: &Self, scale: Vector, _num_subdivisions: u32| {
        let shape = RoundShape {
            border_radius: this.border_radius,
            inner_shape: this.inner_shape.clone().scaled(scale)?,
        };
        Some(Box::new(shape) as Box<dyn Shape>)
    })
);

#[cfg(feature = "dim3")]
impl_shape_for_round_shape!(
    Cylinder,
    RoundCylinder,
    (|this: &Self, scale: Vector, num_subdivisions: u32| {
        Some(
            this.inner_shape
                .scaled(scale, num_subdivisions)?
                .either::<_, _, Box<dyn Shape>>(
                    |inner_shape| {
                        Box::new(RoundShape {
                            border_radius: this.border_radius,
                            inner_shape,
                        })
                    },
                    |inner_shape| {
                        Box::new(RoundShape {
                            border_radius: this.border_radius,
                            inner_shape,
                        })
                    },
                ),
        )
    })
);
#[cfg(feature = "dim3")]
impl_shape_for_round_shape!(
    Cone,
    RoundCone,
    (|this: &Self, scale: Vector, num_subdivisions: u32| {
        Some(
            this.inner_shape
                .scaled(scale, num_subdivisions)?
                .either::<_, _, Box<dyn Shape>>(
                    |inner_shape| {
                        Box::new(RoundShape {
                            border_radius: this.border_radius,
                            inner_shape,
                        })
                    },
                    |inner_shape| {
                        Box::new(RoundShape {
                            border_radius: this.border_radius,
                            inner_shape,
                        })
                    },
                ),
        )
    })
);

#[cfg(feature = "dim3")]
#[cfg(feature = "alloc")]
impl_shape_for_round_shape!(
    ConvexPolyhedron,
    RoundConvexPolyhedron,
    (|this: &Self, scale: Vector, _num_subdivisions: u32| {
        let shape = RoundShape {
            border_radius: this.border_radius,
            inner_shape: this.inner_shape.clone().scaled(scale)?,
        };
        Some(Box::new(shape) as Box<dyn Shape>)
    })
);
