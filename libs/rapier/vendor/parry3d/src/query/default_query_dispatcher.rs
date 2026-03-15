use crate::math::{Pose, Real, Vector};
use crate::query::details::ShapeCastOptions;
use crate::query::{
    self, details::NonlinearShapeCastMode, ClosestPoints, Contact, NonlinearRigidMotion,
    QueryDispatcher, ShapeCastHit, Unsupported,
};
#[cfg(feature = "alloc")]
use crate::query::{
    contact_manifolds::{ContactManifoldsWorkspace, NormalConstraints},
    query_dispatcher::PersistentQueryDispatcher,
    ContactManifold,
};
use crate::shape::{HalfSpace, Segment, Shape};
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

#[cfg(any(feature = "std", feature = "alloc"))]
use crate::shape::ShapeType;

/// The default query dispatcher implementation provided by Parry.
///
/// This dispatcher handles all the built-in shape types and automatically selects the most
/// appropriate algorithm for each shape pair combination. It is used internally by all the
/// free functions in the [`crate::query`] module.
///
/// # What It Does
///
/// `DefaultQueryDispatcher` implements efficient query dispatch logic that:
///
/// 1. **Examines shape types** using runtime type checking (`as_ball()`, `as_cuboid()`, etc.)
/// 2. **Selects specialized algorithms** for specific shape pairs (e.g., ball-ball, cuboid-cuboid)
/// 3. **Falls back to general algorithms** when specialized versions aren't available (e.g., GJK/EPA for support map shapes)
/// 4. **Handles composite shapes** by decomposing them and performing multiple sub-queries
///
/// # Supported Shape Combinations
///
/// The dispatcher provides optimized implementations for many shape pairs, including:
///
/// ## Basic Shapes
/// - **Ball-Ball**: Analytical formulas (fastest)
/// - **Ball-Convex**: Specialized algorithms
/// - **Cuboid-Cuboid**: SAT-based algorithms
/// - **Segment-Segment**: Direct geometric calculations
///
/// ## Support Map Shapes
/// For shapes implementing the `SupportMap` trait (most convex shapes):
/// - Uses **GJK algorithm** for distance and intersection queries
/// - Uses **EPA algorithm** for penetration depth when shapes overlap
///
/// ## Composite Shapes
/// Handles complex shapes by decomposing them:
/// - **TriMesh**: Queries individual triangles using BVH acceleration
/// - **Compound**: Queries component shapes
/// - **HeightField**: Efficiently queries relevant cells
/// - **Voxels**: Queries occupied voxels
///
/// ## Special Cases
/// - **HalfSpace**: Infinite planes with specialized handling
/// - **Rounded shapes**: Automatically accounts for border radius
///
/// # When to Use
///
/// You typically don't need to create `DefaultQueryDispatcher` explicitly. The free functions
/// in [`crate::query`] use it automatically:
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query;
/// use parry3d::shape::Ball;
/// use parry3d::math::Pose;
///
/// let ball1 = Ball::new(1.0);
/// let ball2 = Ball::new(1.0);
/// let pos1 = Pose::identity();
/// let pos2 = Pose::translation(5.0, 0.0, 0.0);
///
/// // This uses DefaultQueryDispatcher internally
/// let distance = query::distance(&pos1, &ball1, &pos2, &ball2);
/// # }
/// ```
///
/// However, you might use it explicitly when:
///
/// - Creating a dispatcher chain with custom dispatchers
/// - Implementing custom query logic that needs to delegate to default behavior
/// - Building a custom collision detection pipeline
///
/// # Example: Direct Usage
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::{QueryDispatcher, DefaultQueryDispatcher};
/// use parry3d::shape::{Ball, Cuboid};
/// use parry3d::math::{Pose, Vector};
///
/// let dispatcher = DefaultQueryDispatcher;
///
/// let ball = Ball::new(1.0);
/// let cuboid = Cuboid::new(Vector::splat(1.0));
///
/// let pos1 = Pose::identity();
/// let pos2 = Pose::translation(3.0, 0.0, 0.0);
/// let pos12 = pos1.inv_mul(&pos2);
///
/// // Query intersection
/// let intersects = dispatcher.intersection_test(&pos12, &ball, &cuboid)
///     .expect("This shape pair is supported");
///
/// // Query distance
/// let dist = dispatcher.distance(&pos12, &ball, &cuboid)
///     .expect("This shape pair is supported");
///
/// println!("Distance: {}, Intersecting: {}", dist, intersects);
/// # }
/// ```
///
/// # Example: Chaining with Custom Dispatcher
///
/// ```ignore
/// # {
/// use parry3d::query::{QueryDispatcher, DefaultQueryDispatcher};
///
/// struct MyCustomDispatcher;
/// // ... implement QueryDispatcher for MyCustomDispatcher ...
///
/// // Try custom dispatcher first, fall back to default
/// let dispatcher = MyCustomDispatcher.chain(DefaultQueryDispatcher);
///
/// // Now queries will use your custom logic when applicable,
/// // and Parry's default logic otherwise
/// let dist = dispatcher.distance(pos12, shape1, shape2)?;
/// # }
/// ```
///
/// # Algorithm Selection Strategy
///
/// The dispatcher follows this priority order when selecting algorithms:
///
/// 1. **Exact shape type matching**: Ball-Ball, Cuboid-Cuboid, etc.
/// 2. **Specialized asymmetric pairs**: Ball-ConvexShape, HalfSpace-SupportMap, etc.
/// 3. **Support map fallback**: Any SupportMap-SupportMap pair uses GJK/EPA
/// 4. **Composite shape decomposition**: TriMesh, Compound, HeightField, Voxels
/// 5. **Unsupported**: Returns `Err(Unsupported)` if no algorithm exists
///
/// # Performance Characteristics
///
/// - **Type checking overhead**: Minimal - uses efficient trait object downcasting
/// - **Specialized algorithms**: O(1) for ball-ball, O(log n) to O(n) for composite shapes
/// - **GJK/EPA**: Iterative algorithms that typically converge in 5-20 iterations
/// - **Composite shapes**: Use BVH for O(log n) acceleration of sub-queries
///
/// # Thread Safety
///
/// `DefaultQueryDispatcher` is `Send + Sync` and has no internal state, making it safe to
/// share across threads. You can use a single instance for all queries in a parallel
/// collision detection system.
///
/// # Limitations
///
/// Some shape pairs are not supported and will return `Err(Unsupported)`:
///
/// - Custom shapes not implementing required traits (e.g., not convex, no support map)
/// - Some asymmetric pairs that lack specialized implementations
/// - Certain combinations involving custom user shapes
///
/// When encountering `Unsupported`, you can implement a custom dispatcher to handle these cases.
///
/// # See Also
///
/// - [`QueryDispatcher`]: The trait this struct implements
/// - [`crate::query`]: High-level query functions that use this dispatcher
/// - [`PersistentQueryDispatcher`]: Extended trait for contact manifold queries
#[derive(Debug, Clone)]
pub struct DefaultQueryDispatcher;

impl QueryDispatcher for DefaultQueryDispatcher {
    fn intersection_test(
        &self,
        pos12: &Pose,
        shape1: &dyn Shape,
        shape2: &dyn Shape,
    ) -> Result<bool, Unsupported> {
        if let (Some(b1), Some(b2)) = (shape1.as_ball(), shape2.as_ball()) {
            let p12 = pos12.translation;
            Ok(query::details::intersection_test_ball_ball(p12, b1, b2))
        } else if let (Some(c1), Some(c2)) = (shape1.as_cuboid(), shape2.as_cuboid()) {
            Ok(query::details::intersection_test_cuboid_cuboid(
                pos12, c1, c2,
            ))
        } else if let (Some(t1), Some(c2)) = (shape1.as_triangle(), shape2.as_cuboid()) {
            Ok(query::details::intersection_test_triangle_cuboid(
                pos12, t1, c2,
            ))
        } else if let (Some(c1), Some(t2)) = (shape1.as_cuboid(), shape2.as_triangle()) {
            Ok(query::details::intersection_test_cuboid_triangle(
                pos12, c1, t2,
            ))
        } else if let Some(b1) = shape1.as_ball() {
            Ok(query::details::intersection_test_ball_point_query(
                pos12, b1, shape2,
            ))
        } else if let Some(b2) = shape2.as_ball() {
            Ok(query::details::intersection_test_point_query_ball(
                pos12, shape1, b2,
            ))
        } else if let (Some(p1), Some(s2)) =
            (shape1.as_shape::<HalfSpace>(), shape2.as_support_map())
        {
            Ok(query::details::intersection_test_halfspace_support_map(
                pos12, p1, s2,
            ))
        } else if let (Some(s1), Some(p2)) =
            (shape1.as_support_map(), shape2.as_shape::<HalfSpace>())
        {
            Ok(query::details::intersection_test_support_map_halfspace(
                pos12, s1, p2,
            ))
        } else if let (Some(s1), Some(s2)) = (shape1.as_support_map(), shape2.as_support_map()) {
            Ok(query::details::intersection_test_support_map_support_map(
                pos12, s1, s2,
            ))
        } else {
            #[cfg(feature = "alloc")]
            if let Some(c1) = shape1.as_composite_shape() {
                return Ok(query::details::intersection_test_composite_shape_shape(
                    self, pos12, c1, shape2,
                ));
            } else if let Some(c2) = shape2.as_composite_shape() {
                return Ok(query::details::intersection_test_shape_composite_shape(
                    self, pos12, shape1, c2,
                ));
            } else if let Some(v1) = shape1.as_voxels() {
                return Ok(query::details::intersection_test_voxels_shape(
                    self, pos12, v1, shape2,
                ));
            } else if let Some(v2) = shape2.as_voxels() {
                return Ok(query::details::intersection_test_shape_voxels(
                    self, pos12, shape1, v2,
                ));
            }

            Err(Unsupported)
        }
    }

    /// Computes the minimum distance separating two shapes.
    ///
    /// Returns `0.0` if the objects are touching or penetrating.
    fn distance(
        &self,
        pos12: &Pose,
        shape1: &dyn Shape,
        shape2: &dyn Shape,
    ) -> Result<Real, Unsupported> {
        let ball1 = shape1.as_ball();
        let ball2 = shape2.as_ball();

        if let (Some(b1), Some(b2)) = (ball1, ball2) {
            let p2 = pos12.translation;
            Ok(query::details::distance_ball_ball(b1, p2, b2))
        } else if let (Some(b1), true) = (ball1, shape2.is_convex()) {
            Ok(query::details::distance_ball_convex_polyhedron(
                pos12, b1, shape2,
            ))
        } else if let (true, Some(b2)) = (shape1.is_convex(), ball2) {
            Ok(query::details::distance_convex_polyhedron_ball(
                pos12, shape1, b2,
            ))
        } else if let (Some(c1), Some(c2)) = (shape1.as_cuboid(), shape2.as_cuboid()) {
            Ok(query::details::distance_cuboid_cuboid(pos12, c1, c2))
        } else if let (Some(s1), Some(s2)) = (shape1.as_segment(), shape2.as_segment()) {
            Ok(query::details::distance_segment_segment(pos12, s1, s2))
        } else if let (Some(p1), Some(s2)) =
            (shape1.as_shape::<HalfSpace>(), shape2.as_support_map())
        {
            Ok(query::details::distance_halfspace_support_map(
                pos12, p1, s2,
            ))
        } else if let (Some(s1), Some(p2)) =
            (shape1.as_support_map(), shape2.as_shape::<HalfSpace>())
        {
            Ok(query::details::distance_support_map_halfspace(
                pos12, s1, p2,
            ))
        } else if let (Some(s1), Some(s2)) = (shape1.as_support_map(), shape2.as_support_map()) {
            Ok(query::details::distance_support_map_support_map(
                pos12, s1, s2,
            ))
        } else {
            #[cfg(feature = "alloc")]
            if let Some(c1) = shape1.as_composite_shape() {
                return Ok(query::details::distance_composite_shape_shape(
                    self, pos12, c1, shape2,
                ));
            } else if let Some(c2) = shape2.as_composite_shape() {
                return Ok(query::details::distance_shape_composite_shape(
                    self, pos12, shape1, c2,
                ));
            }

            Err(Unsupported)
        }
    }

    fn contact(
        &self,
        pos12: &Pose,
        shape1: &dyn Shape,
        shape2: &dyn Shape,
        prediction: Real,
    ) -> Result<Option<Contact>, Unsupported> {
        let ball1 = shape1.as_ball();
        let ball2 = shape2.as_ball();

        if let (Some(b1), Some(b2)) = (ball1, ball2) {
            Ok(query::details::contact_ball_ball(pos12, b1, b2, prediction))
        // } else if let (Some(c1), Some(c2)) = (shape1.as_cuboid(), shape2.as_cuboid()) {
        //     Ok(query::details::contact_cuboid_cuboid(
        //         pos12, c1, c2, prediction,
        //     ))
        } else if let (Some(p1), Some(s2)) =
            (shape1.as_shape::<HalfSpace>(), shape2.as_support_map())
        {
            Ok(query::details::contact_halfspace_support_map(
                pos12, p1, s2, prediction,
            ))
        } else if let (Some(s1), Some(p2)) =
            (shape1.as_support_map(), shape2.as_shape::<HalfSpace>())
        {
            Ok(query::details::contact_support_map_halfspace(
                pos12, s1, p2, prediction,
            ))
        } else if let (Some(b1), true) = (ball1, shape2.is_convex()) {
            Ok(query::details::contact_ball_convex_polyhedron(
                pos12, b1, shape2, prediction,
            ))
        } else if let (true, Some(b2)) = (shape1.is_convex(), ball2) {
            Ok(query::details::contact_convex_polyhedron_ball(
                pos12, shape1, b2, prediction,
            ))
        } else {
            #[cfg(feature = "alloc")]
            if let (Some(s1), Some(s2)) = (shape1.as_support_map(), shape2.as_support_map()) {
                return Ok(query::details::contact_support_map_support_map(
                    pos12, s1, s2, prediction,
                ));
            } else if let Some(c1) = shape1.as_composite_shape() {
                return Ok(query::details::contact_composite_shape_shape(
                    self, pos12, c1, shape2, prediction,
                ));
            } else if let Some(c2) = shape2.as_composite_shape() {
                return Ok(query::details::contact_shape_composite_shape(
                    self, pos12, shape1, c2, prediction,
                ));
            }

            Err(Unsupported)
        }
    }

    fn closest_points(
        &self,
        pos12: &Pose,
        shape1: &dyn Shape,
        shape2: &dyn Shape,
        max_dist: Real,
    ) -> Result<ClosestPoints, Unsupported> {
        let ball1 = shape1.as_ball();
        let ball2 = shape2.as_ball();

        if let (Some(b1), Some(b2)) = (ball1, ball2) {
            Ok(query::details::closest_points_ball_ball(
                pos12, b1, b2, max_dist,
            ))
        } else if let (Some(b1), true) = (ball1, shape2.is_convex()) {
            Ok(query::details::closest_points_ball_convex_polyhedron(
                pos12, b1, shape2, max_dist,
            ))
        } else if let (true, Some(b2)) = (shape1.is_convex(), ball2) {
            Ok(query::details::closest_points_convex_polyhedron_ball(
                pos12, shape1, b2, max_dist,
            ))
        } else if let (Some(s1), Some(s2)) =
            (shape1.as_shape::<Segment>(), shape2.as_shape::<Segment>())
        {
            Ok(query::details::closest_points_segment_segment(
                pos12, s1, s2, max_dist,
            ))
        // } else if let (Some(c1), Some(c2)) = (shape1.as_cuboid(), shape2.as_cuboid()) {
        //     Ok(query::details::closest_points_cuboid_cuboid(
        //         pos12, c1, c2, max_dist,
        //     ))
        } else if let (Some(s1), Some(s2)) = (shape1.as_segment(), shape2.as_segment()) {
            Ok(query::details::closest_points_segment_segment(
                pos12, s1, s2, max_dist,
            ))
        // } else if let (Some(c1), Some(t2)) = (shape1.as_cuboid(), shape2.as_triangle()) {
        //     Ok(query::details::closest_points_cuboid_triangle(
        //         pos12, c1, t2, max_dist,
        //     ))
        } else if let (Some(t1), Some(c2)) = (shape1.as_triangle(), shape2.as_cuboid()) {
            Ok(query::details::closest_points_triangle_cuboid(
                pos12, t1, c2, max_dist,
            ))
        } else if let (Some(p1), Some(s2)) =
            (shape1.as_shape::<HalfSpace>(), shape2.as_support_map())
        {
            Ok(query::details::closest_points_halfspace_support_map(
                pos12, p1, s2, max_dist,
            ))
        } else if let (Some(s1), Some(p2)) =
            (shape1.as_support_map(), shape2.as_shape::<HalfSpace>())
        {
            Ok(query::details::closest_points_support_map_halfspace(
                pos12, s1, p2, max_dist,
            ))
        } else if let (Some(s1), Some(s2)) = (shape1.as_support_map(), shape2.as_support_map()) {
            Ok(query::details::closest_points_support_map_support_map(
                pos12, s1, s2, max_dist,
            ))
        } else {
            #[cfg(feature = "alloc")]
            if let Some(c1) = shape1.as_composite_shape() {
                return Ok(query::details::closest_points_composite_shape_shape(
                    self, pos12, c1, shape2, max_dist,
                ));
            } else if let Some(c2) = shape2.as_composite_shape() {
                return Ok(query::details::closest_points_shape_composite_shape(
                    self, pos12, shape1, c2, max_dist,
                ));
            }

            Err(Unsupported)
        }
    }

    fn cast_shapes(
        &self,
        pos12: &Pose,
        local_vel12: Vector,
        shape1: &dyn Shape,
        shape2: &dyn Shape,
        options: ShapeCastOptions,
    ) -> Result<Option<ShapeCastHit>, Unsupported> {
        if let (Some(b1), Some(b2)) = (shape1.as_ball(), shape2.as_ball()) {
            Ok(query::details::cast_shapes_ball_ball(
                pos12,
                local_vel12,
                b1,
                b2,
                options,
            ))
        } else if let (Some(p1), Some(s2)) =
            (shape1.as_shape::<HalfSpace>(), shape2.as_support_map())
        {
            Ok(query::details::cast_shapes_halfspace_support_map(
                pos12,
                local_vel12,
                p1,
                s2,
                options,
            ))
        } else if let (Some(s1), Some(p2)) =
            (shape1.as_support_map(), shape2.as_shape::<HalfSpace>())
        {
            Ok(query::details::cast_shapes_support_map_halfspace(
                pos12,
                local_vel12,
                s1,
                p2,
                options,
            ))
        } else {
            #[cfg(feature = "alloc")]
            if let Some(heightfield1) = shape1.as_heightfield() {
                return query::details::cast_shapes_heightfield_shape(
                    self,
                    pos12,
                    local_vel12,
                    heightfield1,
                    shape2,
                    options,
                );
            } else if let Some(heightfield2) = shape2.as_heightfield() {
                return query::details::cast_shapes_shape_heightfield(
                    self,
                    pos12,
                    local_vel12,
                    shape1,
                    heightfield2,
                    options,
                );
            } else if let (Some(s1), Some(s2)) = (shape1.as_support_map(), shape2.as_support_map())
            {
                return Ok(query::details::cast_shapes_support_map_support_map(
                    pos12,
                    local_vel12,
                    s1,
                    s2,
                    options,
                ));
            } else if let Some(c1) = shape1.as_composite_shape() {
                return Ok(query::details::cast_shapes_composite_shape_shape(
                    self,
                    pos12,
                    local_vel12,
                    c1,
                    shape2,
                    options,
                ));
            } else if let Some(c2) = shape2.as_composite_shape() {
                return Ok(query::details::cast_shapes_shape_composite_shape(
                    self,
                    pos12,
                    local_vel12,
                    shape1,
                    c2,
                    options,
                ));
            } else if let Some(v1) = shape1.as_voxels() {
                return Ok(query::details::cast_shapes_voxels_shape(
                    self,
                    pos12,
                    local_vel12,
                    v1,
                    shape2,
                    options,
                ));
            } else if let Some(v2) = shape2.as_voxels() {
                return Ok(query::details::cast_shapes_shape_voxels(
                    self,
                    pos12,
                    local_vel12,
                    shape1,
                    v2,
                    options,
                ));
            }

            Err(Unsupported)
        }
    }

    fn cast_shapes_nonlinear(
        &self,
        motion1: &NonlinearRigidMotion,
        shape1: &dyn Shape,
        motion2: &NonlinearRigidMotion,
        shape2: &dyn Shape,
        start_time: Real,
        end_time: Real,
        stop_at_penetration: bool,
    ) -> Result<Option<ShapeCastHit>, Unsupported> {
        if let (Some(sm1), Some(sm2)) = (shape1.as_support_map(), shape2.as_support_map()) {
            let mode = if stop_at_penetration {
                NonlinearShapeCastMode::StopAtPenetration
            } else {
                NonlinearShapeCastMode::directional_toi(shape1, shape2)
            };

            Ok(
                query::details::cast_shapes_nonlinear_support_map_support_map(
                    self, motion1, sm1, shape1, motion2, sm2, shape2, start_time, end_time, mode,
                ),
            )
        } else {
            #[cfg(feature = "alloc")]
            if let Some(c1) = shape1.as_composite_shape() {
                return Ok(query::details::cast_shapes_nonlinear_composite_shape_shape(
                    self,
                    motion1,
                    c1,
                    motion2,
                    shape2,
                    start_time,
                    end_time,
                    stop_at_penetration,
                ));
            } else if let Some(c2) = shape2.as_composite_shape() {
                return Ok(query::details::cast_shapes_nonlinear_shape_composite_shape(
                    self,
                    motion1,
                    shape1,
                    motion2,
                    c2,
                    start_time,
                    end_time,
                    stop_at_penetration,
                ));
            } else if let Some(c1) = shape1.as_voxels() {
                return Ok(query::details::cast_shapes_nonlinear_voxels_shape(
                    self,
                    motion1,
                    c1,
                    motion2,
                    shape2,
                    start_time,
                    end_time,
                    stop_at_penetration,
                ));
            } else if let Some(c2) = shape2.as_voxels() {
                return Ok(query::details::cast_shapes_nonlinear_shape_voxels(
                    self,
                    motion1,
                    shape1,
                    motion2,
                    c2,
                    start_time,
                    end_time,
                    stop_at_penetration,
                ));
            }
            /* } else if let (Some(p1), Some(s2)) = (shape1.as_shape::<HalfSpace>(), shape2.as_support_map()) {
            //        query::details::cast_shapes_nonlinear_halfspace_support_map(m1, vel1, p1, m2, vel2, s2)
                    unimplemented!()
                } else if let (Some(s1), Some(p2)) = (shape1.as_support_map(), shape2.as_shape::<HalfSpace>()) {
            //        query::details::cast_shapes_nonlinear_support_map_halfspace(m1, vel1, s1, m2, vel2, p2)
                    unimplemented!() */

            Err(Unsupported)
        }
    }
}

#[cfg(feature = "alloc")]
impl<ManifoldData, ContactData> PersistentQueryDispatcher<ManifoldData, ContactData>
    for DefaultQueryDispatcher
where
    ManifoldData: Default + Clone,
    ContactData: Default + Copy,
{
    fn contact_manifolds(
        &self,
        pos12: &Pose,
        shape1: &dyn Shape,
        shape2: &dyn Shape,
        prediction: Real,
        manifolds: &mut Vec<ContactManifold<ManifoldData, ContactData>>,
        workspace: &mut Option<ContactManifoldsWorkspace>,
    ) -> Result<(), Unsupported> {
        use crate::query::contact_manifolds::*;

        let composite1 = shape1.as_composite_shape();
        let composite2 = shape2.as_composite_shape();

        if let (Some(composite1), Some(composite2)) = (composite1, composite2) {
            contact_manifolds_composite_shape_composite_shape(
                self, pos12, composite1, composite2, prediction, manifolds, workspace,
            );

            return Ok(());
        }

        match (shape1.shape_type(), shape2.shape_type()) {
            (ShapeType::TriMesh, _) | (_, ShapeType::TriMesh) => {
                contact_manifolds_trimesh_shape_shapes(
                    self, pos12, shape1, shape2, prediction, manifolds, workspace,
                );
            }
            (ShapeType::HeightField, _) => {
                if let Some(composite2) = composite2 {
                    contact_manifolds_heightfield_composite_shape(
                        self,
                        pos12,
                        &pos12.inverse(),
                        shape1.as_heightfield().unwrap(),
                        composite2,
                        prediction,
                        manifolds,
                        workspace,
                        false,
                    )
                } else {
                    contact_manifolds_heightfield_shape_shapes(
                        self, pos12, shape1, shape2, prediction, manifolds, workspace,
                    );
                }
            }
            (_, ShapeType::HeightField) => {
                if let Some(composite1) = composite1 {
                    contact_manifolds_heightfield_composite_shape(
                        self,
                        &pos12.inverse(),
                        pos12,
                        shape2.as_heightfield().unwrap(),
                        composite1,
                        prediction,
                        manifolds,
                        workspace,
                        true,
                    )
                } else {
                    contact_manifolds_heightfield_shape_shapes(
                        self, pos12, shape1, shape2, prediction, manifolds, workspace,
                    );
                }
            }
            (ShapeType::Voxels, ShapeType::Voxels) => contact_manifolds_voxels_voxels_shapes(
                self, pos12, shape1, shape2, prediction, manifolds, workspace,
            ),
            (ShapeType::Voxels, ShapeType::Ball) | (ShapeType::Ball, ShapeType::Voxels) => {
                contact_manifolds_voxels_ball_shapes(pos12, shape1, shape2, prediction, manifolds)
            }
            (ShapeType::Voxels, _) | (_, ShapeType::Voxels) => {
                if composite1.is_some() || composite2.is_some() {
                    contact_manifolds_voxels_composite_shape_shapes(
                        self, pos12, shape1, shape2, prediction, manifolds, workspace,
                    )
                } else {
                    contact_manifolds_voxels_shape_shapes(
                        self, pos12, shape1, shape2, prediction, manifolds, workspace,
                    )
                }
            }
            _ => {
                if let Some(composite1) = composite1 {
                    contact_manifolds_composite_shape_shape(
                        self, pos12, composite1, shape2, prediction, manifolds, workspace, false,
                    );
                } else if let Some(composite2) = composite2 {
                    contact_manifolds_composite_shape_shape(
                        self,
                        &pos12.inverse(),
                        composite2,
                        shape1,
                        prediction,
                        manifolds,
                        workspace,
                        true,
                    );
                } else {
                    if manifolds.is_empty() {
                        manifolds.push(ContactManifold::new());
                    }

                    return self.contact_manifold_convex_convex(
                        pos12,
                        shape1,
                        shape2,
                        None,
                        None,
                        prediction,
                        &mut manifolds[0],
                    );
                }
            }
        }

        Ok(())
    }

    fn contact_manifold_convex_convex(
        &self,
        pos12: &Pose,
        shape1: &dyn Shape,
        shape2: &dyn Shape,
        normal_constraints1: Option<&dyn NormalConstraints>,
        normal_constraints2: Option<&dyn NormalConstraints>,
        prediction: Real,
        manifold: &mut ContactManifold<ManifoldData, ContactData>,
    ) -> Result<(), Unsupported> {
        use crate::query::contact_manifolds::*;

        match (shape1.shape_type(), shape2.shape_type()) {
            (ShapeType::Ball, ShapeType::Ball) => {
                contact_manifold_ball_ball_shapes(pos12, shape1, shape2, prediction, manifold)
            }
            (ShapeType::Cuboid, ShapeType::Cuboid) =>
                contact_manifold_cuboid_cuboid_shapes(pos12, shape1, shape2, prediction, manifold)
            ,
            // (ShapeType::Polygon, ShapeType::Polygon) => (
            //     PrimitiveContactGenerator {
            //         generate_contacts: super::generate_contacts_polygon_polygon,
            //         ..PrimitiveContactGenerator::default()
            //     },
            //     None,
            // ),
            (ShapeType::Capsule, ShapeType::Capsule) => {
                contact_manifold_capsule_capsule_shapes(pos12, shape1, shape2, prediction, manifold)
            }
            (_, ShapeType::Ball) | (ShapeType::Ball, _) => {
                contact_manifold_convex_ball_shapes(pos12, shape1, shape2, normal_constraints1, normal_constraints2, prediction, manifold)
            }
            // (ShapeType::Capsule, ShapeType::Cuboid) | (ShapeType::Cuboid, ShapeType::Capsule) =>
            //     contact_manifold_cuboid_capsule_shapes(pos12, shape1, shape2, prediction, manifold),
            (ShapeType::Triangle, ShapeType::Cuboid) | (ShapeType::Cuboid, ShapeType::Triangle) => {
                contact_manifold_cuboid_triangle_shapes(pos12, shape1, shape2, normal_constraints1, normal_constraints2,  prediction, manifold)
            }
            (ShapeType::HalfSpace, _) => {
                if let Some((pfm2, border_radius2)) = shape2.as_polygonal_feature_map() {
                    contact_manifold_halfspace_pfm(
                        pos12,
                        shape1.as_halfspace().unwrap(),
                        pfm2,
                        border_radius2,
                        prediction,
                        manifold,
                        false
                    )
                } else {
                    return Err(Unsupported)
                }
            }
            (_, ShapeType::HalfSpace) => {
                if let Some((pfm1, border_radius1)) = shape1.as_polygonal_feature_map() {
                    contact_manifold_halfspace_pfm(
                        &pos12.inverse(),
                        shape2.as_halfspace().unwrap(),
                        pfm1,
                        border_radius1,
                        prediction,
                        manifold,
                        true
                    )
                } else {
                    return Err(Unsupported)
                }
            }
            _ => {
                if let (Some(pfm1), Some(pfm2)) = (
                    shape1.as_polygonal_feature_map(),
                    shape2.as_polygonal_feature_map(),
                ) {
                    contact_manifold_pfm_pfm(
                        pos12, pfm1.0, pfm1.1, normal_constraints1, pfm2.0, pfm2.1, normal_constraints2, prediction, manifold,
                    )
                } else {
                    return Err(Unsupported);
                }
            }
        }

        Ok(())
    }
}
