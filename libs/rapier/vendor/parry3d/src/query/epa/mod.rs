//! Expanding Polytope Algorithm (EPA) for computing penetration depth and contact information.
//!
//! # What is EPA?
//!
//! The **Expanding Polytope Algorithm (EPA)** is a computational geometry algorithm used to
//! determine how deeply two convex shapes are penetrating each other. It works as a follow-up
//! to the GJK (Gilbert-Johnson-Keerthi) algorithm.
//!
//! # How EPA relates to GJK
//!
//! **GJK and EPA work together as a two-stage process:**
//!
//! 1. **GJK** (Gilbert-Johnson-Keerthi) quickly determines if two shapes are intersecting
//!    - If shapes are **separated**: GJK computes the distance between them
//!    - If shapes are **penetrating**: GJK detects the penetration but cannot compute the depth
//!
//! 2. **EPA** takes over when shapes are penetrating
//!    - Uses GJK's final simplex (a set of points surrounding the origin) as a starting point
//!    - Iteratively expands this simplex to find the penetration depth and contact normal
//!
//! # When to use EPA
//!
//! EPA is used when you need detailed contact information for **penetrating shapes**:
//!
//! - **Physics simulation**: Resolving interpenetration between colliding objects
//! - **Contact manifold generation**: Finding contact points and normals for collision response
//! - **Penetration depth queries**: Determining how far apart to move objects to resolve overlap
//!
//! **Key requirement**: Both shapes must implement the [`SupportMap`](crate::shape::SupportMap)
//! trait, which provides support points in any direction.
//!
//! # How EPA works (simplified)
//!
//! 1. **Start with GJK simplex**: Begin with the simplex from GJK that encloses the origin
//!    in the Minkowski difference space (the CSO - Configuration Space Obstacle)
//!
//! 2. **Find closest face**: Identify which face of the polytope is closest to the origin
//!
//! 3. **Expand polytope**: Add a new support point in the direction of the closest face's normal
//!
//! 4. **Update and repeat**: Incorporate the new point and find the new closest face
//!
//! 5. **Converge**: Stop when the closest face cannot be expanded further
//!    - The distance to this face is the **penetration depth**
//!    - The face's normal is the **contact normal**
//!    - The closest points on the face map back to **contact points** on the original shapes
//!
//! # Example usage
//!
//! ```
//! # #[cfg(all(feature = "dim3", feature = "f32"))] {
//! use parry3d::query::epa::EPA;
//! use parry3d::query::gjk::VoronoiSimplex;
//! use parry3d::shape::Ball;
//! use parry3d::math::Pose;
//!
//! let ball1 = Ball::new(1.0);
//! let ball2 = Ball::new(1.0);
//! let pos12 = Pose::translation(1.5, 0.0, 0.0); // Overlapping balls
//!
//! // GJK detects penetration, EPA computes contact details
//! let simplex = VoronoiSimplex::new(); // Would be filled by GJK
//! let mut epa = EPA::new();
//!
//! // If GJK detected penetration, EPA can compute:
//! // - Contact points on each shape
//! // - Penetration depth
//! // - Contact normal
//! // if let Some((pt1, pt2, normal)) = epa.closest_points(&pos12, &ball1, &ball2, &simplex) {
//! //     println!("Contact normal: {}", normal);
//! // }
//! # }
//! ```
//!
//! # Implementation notes
//!
//! - **Dimension-specific**: EPA has separate implementations for 2D (`epa2`) and 3D (`epa3`)
//!   because the polytope expansion differs:
//!   - 2D: Expands polygons (edges)
//!   - 3D: Expands polyhedra (faces)
//!
//! - **Convergence**: EPA uses tolerances to determine when to stop expanding. In rare cases
//!   with degenerate geometry or numerical precision issues, it may return `None` if it cannot
//!   converge to a solution.
//!
//! - **Performance**: EPA is more expensive than GJK, which is why it's only used when shapes
//!   are actually penetrating. For separated shapes, GJK alone is sufficient.
//!
#[cfg(feature = "dim2")]
pub use self::epa2::EPA;
#[cfg(feature = "dim3")]
pub use self::epa3::EPA;

#[cfg(feature = "dim2")]
pub mod epa2;
#[cfg(feature = "dim3")]
pub mod epa3;
