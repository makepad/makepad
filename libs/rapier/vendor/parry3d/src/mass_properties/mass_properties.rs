use crate::math::{AngVector, AngularInertia, Pose, Real, Rotation, Vector};
#[cfg(feature = "dim3")]
use crate::math::{MatExt, Matrix, VectorExt};
use crate::utils;
use core::ops::{Add, AddAssign, Sub, SubAssign};
use num::Zero;

#[cfg_attr(feature = "f32", expect(clippy::unnecessary_cast))]
const EPSILON: Real = f32::EPSILON as Real;

#[derive(Copy, Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
/// The mass properties of a rigid body.
///
/// Mass properties define how an object responds to forces and torques in physics
/// simulation. They include the mass, center of mass, and angular inertia (resistance
/// to rotation).
///
/// # Fields
///
/// - **local_com**: Center of mass in the shape's local coordinate system
/// - **inv_mass**: Inverse mass (1/mass). Zero = infinite mass (immovable)
/// - **inv_principal_inertia**: Inverse angular inertia along principal axes
/// - **principal_inertia_local_frame** (3D only): Rotation to principal inertia axes
///
/// # Why Inverse Values?
///
/// Physics engines store inverse mass and inertia because:
/// - Infinite mass/inertia (immovable objects) = zero inverse
/// - Avoids division in the simulation loop (multiply by inverse instead)
/// - More numerically stable for very heavy objects
///
/// # Angular Inertia
///
/// Angular inertia (moment of inertia) describes resistance to rotation:
/// - **Higher values**: Harder to spin (like a heavy wheel)
/// - **Lower values**: Easier to spin (like a light rod)
/// - **Different per axis**: Objects resist rotation differently around different axes
///
/// # Principal Inertia
///
/// The inertia tensor is diagonalized to principal axes:
/// - Rotation around principal axes is independent
/// - Simplifies physics calculations
/// - In 2D: Only one axis (perpendicular to the plane)
/// - In 3D: Three orthogonal axes
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::mass_properties::MassProperties;
/// use parry3d::shape::{Ball, Shape};
/// use parry3d::math::Vector;
///
/// // Compute mass properties for a unit ball with density 1.0
/// let ball = Ball::new(1.0);
/// let props = ball.mass_properties(1.0);
///
/// // Mass of a unit sphere with density 1.0
/// let mass = props.mass();
/// println!("Mass: {}", mass);
///
/// // Center of mass (at origin for a ball)
/// assert_eq!(props.local_com, Vector::ZERO);
///
/// // For simulation, use inverse values
/// if props.inv_mass > 0.0 {
///     // Object has finite mass - can be moved
///     println!("Can apply forces");
/// } else {
///     // Object has infinite mass - immovable (like terrain)
///     println!("Static/kinematic object");
/// }
/// # }
/// ```
///
/// # Combining Mass Properties
///
/// Mass properties can be added to create compound objects:
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::mass_properties::MassProperties;
/// use parry3d::shape::{Ball, Cuboid, Shape};
/// use parry3d::math::Vector;
///
/// let ball = Ball::new(1.0);
/// let cuboid = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
///
/// let ball_props = ball.mass_properties(1.0);
/// let cuboid_props = cuboid.mass_properties(1.0);
///
/// // Combined properties (ball + cuboid)
/// let combined = ball_props + cuboid_props;
///
/// // Total mass is sum of individual masses
/// let total_mass = combined.mass();
/// println!("Combined mass: {}", total_mass);
/// # }
/// ```
pub struct MassProperties {
    /// The center of mass in local (shape-relative) coordinates.
    ///
    /// This is the balance point of the object. For symmetric shapes, it's typically
    /// at the geometric center. All angular inertia calculations are relative to this point.
    pub local_com: Vector,

    /// The inverse of the mass (1 / mass).
    ///
    /// - **Positive value**: Normal object with finite mass
    /// - **Zero**: Infinite mass (immovable/static object)
    ///
    /// To get the actual mass, use `mass()` method or compute `1.0 / inv_mass`.
    pub inv_mass: Real,

    /// The inverse of the principal angular inertia values.
    ///
    /// These are the angular inertia values along the principal inertia axes:
    /// - **2D**: Single scalar value (rotation around perpendicular axis)
    /// - **3D**: Vector of three values (rotation around X, Y, Z principal axes)
    ///
    /// Angular inertia relative to the center of mass (`local_com`).
    /// Zero components indicate infinite inertia (no rotation) along that axis.
    pub inv_principal_inertia: AngVector,

    #[cfg(feature = "dim3")]
    /// The rotation from local coordinates to principal inertia axes (3D only).
    ///
    /// This rotation aligns the object's coordinate system with its principal
    /// axes of inertia, where the inertia tensor is diagonal.
    pub principal_inertia_local_frame: Rotation,
}

impl MassProperties {
    /// Creates mass properties from center of mass, mass, and angular inertia (2D).
    ///
    /// # Arguments
    ///
    /// * `local_com` - Center of mass in local coordinates
    /// * `mass` - The mass (positive value, use 0.0 for infinite mass)
    /// * `principal_inertia` - Angular inertia around the perpendicular axis
    ///
    /// # Example (2D)
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::mass_properties::MassProperties;
    /// use parry2d::math::Vector;
    ///
    /// // Create mass properties for a 10kg object
    /// let props = MassProperties::new(
    ///     Vector::ZERO,     // Centered at origin
    ///     10.0,              // 10kg mass
    ///     5.0                // Angular inertia
    /// );
    ///
    /// assert_eq!(props.mass(), 10.0);
    /// assert_eq!(props.inv_mass, 0.1);  // 1/10
    /// # }
    /// ```
    #[cfg(feature = "dim2")]
    pub fn new(local_com: Vector, mass: Real, principal_inertia: Real) -> Self {
        let inv_mass = utils::inv(mass);
        let inv_principal_inertia = utils::inv(principal_inertia);
        Self {
            local_com,
            inv_mass,
            inv_principal_inertia,
        }
    }

    /// Initializes the mass properties from the given center-of-mass, mass, and principal angular inertia.
    ///
    /// The center-of-mass is specified in the local-space of the rigid-body.
    /// The principal angular inertia are the angular inertia along the coordinate axes in the local-space
    /// of the rigid-body.
    #[cfg(feature = "dim3")]
    pub fn new(local_com: Vector, mass: Real, principal_inertia: AngVector) -> Self {
        Self::with_principal_inertia_frame(local_com, mass, principal_inertia, Rotation::IDENTITY)
    }

    /// Initializes the mass properties from the given center-of-mass, mass, and principal angular inertia.
    ///
    /// The center-of-mass is specified in the local-space of the rigid-body.
    /// The principal angular inertia are the angular inertia along the coordinate axes defined by
    /// the `principal_inertia_local_frame` expressed in the local-space of the rigid-body.
    #[cfg(feature = "dim3")]
    pub fn with_principal_inertia_frame(
        local_com: Vector,
        mass: Real,
        principal_inertia: AngVector,
        principal_inertia_local_frame: Rotation,
    ) -> Self {
        let inv_mass = utils::inv(mass);
        let inv_principal_inertia = principal_inertia.map(utils::inv);
        Self {
            local_com,
            inv_mass,
            inv_principal_inertia,
            principal_inertia_local_frame,
        }
    }

    /// Initialize a new `MassProperties` from a given center-of-mass, mass, and angular inertia matrix.
    ///
    /// The angular inertia matrix will be diagonalized in order to extract the principal inertia
    /// values and principal inertia frame.
    #[cfg(feature = "dim3")]
    pub fn with_inertia_matrix(local_com: Vector, mass: Real, inertia: Matrix) -> Self {
        let mut eigen = inertia.symmetric_eigen();
        if eigen.eigenvectors.determinant() < 0.0 {
            eigen.eigenvectors.swap_cols(1, 2);
            eigen.eigenvalues.as_mut().swap(1, 2);
        }
        let eigenvectors = eigen.eigenvectors;
        let principal_inertia_local_frame = Rotation::from_mat3(&eigenvectors).normalize();

        // Drop negative eigenvalues.
        let principal_inertia = eigen.eigenvalues.map(|e| e.max(0.0));

        Self::with_principal_inertia_frame(
            local_com,
            mass,
            principal_inertia,
            principal_inertia_local_frame,
        )
    }

    /// The mass.
    pub fn mass(&self) -> Real {
        utils::inv(self.inv_mass)
    }

    /// The angular inertia along the principal inertia axes and center of mass of the rigid-body.
    pub fn principal_inertia(&self) -> AngVector {
        #[cfg(feature = "dim2")]
        return utils::inv(self.inv_principal_inertia);
        #[cfg(feature = "dim3")]
        return self.inv_principal_inertia.map(utils::inv);
    }

    /// The world-space center of mass of the rigid-body.
    pub fn world_com(&self, pos: &Pose) -> Vector {
        pos * self.local_com
    }

    #[cfg(feature = "dim2")]
    /// The world-space inverse angular inertia tensor of the rigid-body.
    pub fn world_inv_inertia(&self, _rot: &Rotation) -> AngularInertia {
        self.inv_principal_inertia
    }

    #[cfg(feature = "dim3")]
    /// The world-space inverse angular inertia tensor of the rigid-body.
    pub fn world_inv_inertia(&self, rot: &Rotation) -> AngularInertia {
        if self.inv_principal_inertia != Vector::ZERO {
            let mut lhs = Matrix::from_quat(rot * self.principal_inertia_local_frame);
            let rhs = lhs.transpose();
            lhs.x_axis *= self.inv_principal_inertia.x;
            lhs.y_axis *= self.inv_principal_inertia.y;
            lhs.z_axis *= self.inv_principal_inertia.z;
            let inertia = lhs * rhs;
            AngularInertia::from_sdp_matrix(inertia)
        } else {
            AngularInertia::zero()
        }
    }

    #[cfg(feature = "dim3")]
    /// Reconstructs the inverse angular inertia tensor of the rigid body from its principal inertia values and axes.
    pub fn reconstruct_inverse_inertia_matrix(&self) -> Matrix {
        let inv_principal_inertia = self.inv_principal_inertia;
        let rot_mat = Matrix::from_quat(self.principal_inertia_local_frame);
        let inv_rot_mat = Matrix::from_quat(self.principal_inertia_local_frame.inverse());

        rot_mat * Matrix::from_diagonal(inv_principal_inertia) * inv_rot_mat
    }

    #[cfg(feature = "dim3")]
    /// Reconstructs the angular inertia tensor of the rigid body from its principal inertia values and axes.
    pub fn reconstruct_inertia_matrix(&self) -> Matrix {
        let principal_inertia = self.inv_principal_inertia.map(utils::inv);
        let rot_mat = Matrix::from_quat(self.principal_inertia_local_frame);
        let inv_rot_mat = Matrix::from_quat(self.principal_inertia_local_frame.inverse());
        rot_mat * Matrix::from_diagonal(principal_inertia) * inv_rot_mat
    }

    #[cfg(feature = "dim2")]
    pub(crate) fn construct_shifted_inertia_matrix(&self, shift: Vector) -> Real {
        let i = utils::inv(self.inv_principal_inertia);

        if self.inv_mass != 0.0 {
            let mass = 1.0 / self.inv_mass;
            i + shift.length_squared() * mass
        } else {
            i
        }
    }

    #[cfg(feature = "dim3")]
    pub(crate) fn construct_shifted_inertia_matrix(&self, shift: Vector) -> Matrix {
        let matrix = self.reconstruct_inertia_matrix();

        if self.inv_mass != 0.0 {
            let mass = 1.0 / self.inv_mass;
            let diag = shift.length_squared();
            let diagm = Matrix::from_diagonal(Vector::splat(diag));
            matrix + (diagm - shift.kronecker(shift)) * mass
        } else {
            matrix
        }
    }

    /// Transform each element of the mass properties.
    pub fn transform_by(&self, m: &Pose) -> Self {
        // NOTE: we don't apply the parallel axis theorem here
        // because the center of mass is also transformed.
        Self {
            local_com: m * self.local_com,
            inv_mass: self.inv_mass,
            inv_principal_inertia: self.inv_principal_inertia,
            #[cfg(feature = "dim3")]
            principal_inertia_local_frame: m.rotation * self.principal_inertia_local_frame,
        }
    }

    /// Changes the mass on these mass-properties.
    ///
    /// The `adjust_angular_inertia` argument should always be `true`, unless
    /// there are some specific reasons not to do so. Setting this to `true`
    /// will automatically adjust the angular inertia of `self` to account
    /// for the mass change (i.e. it will multiply the angular inertia by
    /// `new_mass / prev_mass`). Setting it to `false` will not change the
    /// current angular inertia.
    pub fn set_mass(&mut self, new_mass: Real, adjust_angular_inertia: bool) {
        let new_inv_mass = utils::inv(new_mass);

        if adjust_angular_inertia {
            let curr_mass = utils::inv(self.inv_mass);
            self.inv_principal_inertia *= new_inv_mass * curr_mass;
        }

        self.inv_mass = new_inv_mass;
    }
}

impl Zero for MassProperties {
    fn zero() -> Self {
        Self {
            inv_mass: 0.0,
            #[cfg(feature = "dim2")]
            inv_principal_inertia: 0.0,
            #[cfg(feature = "dim3")]
            inv_principal_inertia: Vector::ZERO,
            #[cfg(feature = "dim3")]
            principal_inertia_local_frame: Rotation::IDENTITY,
            local_com: Vector::ZERO,
        }
    }

    fn is_zero(&self) -> bool {
        *self == Self::zero()
    }
}

impl Sub<MassProperties> for MassProperties {
    type Output = Self;

    #[cfg(feature = "dim2")]
    fn sub(self, other: MassProperties) -> Self {
        if self.is_zero() || other.is_zero() {
            return self;
        }

        let m1 = utils::inv(self.inv_mass);
        let m2 = utils::inv(other.inv_mass);

        let mut new_mass = m1 - m2;

        if new_mass < EPSILON {
            // Account for small numerical errors.
            new_mass = 0.0;
        }

        let inv_mass = utils::inv(new_mass);

        let local_com = (self.local_com * m1 - other.local_com * m2) * inv_mass;
        let i1 = self.construct_shifted_inertia_matrix(local_com - self.local_com);
        let i2 = other.construct_shifted_inertia_matrix(local_com - other.local_com);
        let mut inertia = i1 - i2;

        if inertia < EPSILON {
            // Account for small numerical errors.
            inertia = 0.0;
        }

        // NOTE: we drop the negative eigenvalues that may result from subtraction rounding errors.
        let inv_principal_inertia = utils::inv(inertia);

        Self {
            local_com,
            inv_mass,
            inv_principal_inertia,
        }
    }

    #[cfg(feature = "dim3")]
    fn sub(self, other: MassProperties) -> Self {
        if self.is_zero() || other.is_zero() {
            return self;
        }

        let m1 = utils::inv(self.inv_mass);
        let m2 = utils::inv(other.inv_mass);
        let mut new_mass = m1 - m2;

        if new_mass < EPSILON {
            new_mass = 0.0;
        }

        let inv_mass = utils::inv(new_mass);
        let local_com = (self.local_com * m1 - other.local_com * m2) * inv_mass;
        let i1 = self.construct_shifted_inertia_matrix(local_com - self.local_com);
        let i2 = other.construct_shifted_inertia_matrix(local_com - other.local_com);
        let inertia = i1 - i2;
        Self::with_inertia_matrix(local_com, new_mass, inertia)
    }
}

impl SubAssign<MassProperties> for MassProperties {
    fn sub_assign(&mut self, rhs: MassProperties) {
        *self = *self - rhs
    }
}

impl Add<MassProperties> for MassProperties {
    type Output = Self;

    #[cfg(feature = "dim2")]
    fn add(self, other: MassProperties) -> Self {
        if self.is_zero() {
            return other;
        } else if other.is_zero() {
            return self;
        }

        let m1 = utils::inv(self.inv_mass);
        let m2 = utils::inv(other.inv_mass);
        let inv_mass = utils::inv(m1 + m2);
        let local_com = (self.local_com * m1 + other.local_com * m2) * inv_mass;
        let i1 = self.construct_shifted_inertia_matrix(local_com - self.local_com);
        let i2 = other.construct_shifted_inertia_matrix(local_com - other.local_com);
        let inertia = i1 + i2;
        let inv_principal_inertia = utils::inv(inertia);

        Self {
            local_com,
            inv_mass,
            inv_principal_inertia,
        }
    }

    #[cfg(feature = "dim3")]
    fn add(self, other: MassProperties) -> Self {
        if self.is_zero() {
            return other;
        } else if other.is_zero() {
            return self;
        }

        let m1 = utils::inv(self.inv_mass);
        let m2 = utils::inv(other.inv_mass);
        let inv_mass = utils::inv(m1 + m2);
        let local_com = (self.local_com * m1 + other.local_com * m2) * inv_mass;
        let i1 = self.construct_shifted_inertia_matrix(local_com - self.local_com);
        let i2 = other.construct_shifted_inertia_matrix(local_com - other.local_com);
        let inertia = i1 + i2;

        Self::with_inertia_matrix(local_com, m1 + m2, inertia)
    }
}

impl AddAssign<MassProperties> for MassProperties {
    fn add_assign(&mut self, rhs: MassProperties) {
        *self = *self + rhs
    }
}

#[cfg(feature = "alloc")]
impl core::iter::Sum<MassProperties> for MassProperties {
    #[cfg(feature = "dim2")]
    fn sum<I>(iter: I) -> Self
    where
        I: Iterator<Item = Self>,
    {
        use alloc::vec::Vec;

        let mut total_mass = 0.0;
        let mut total_com = Vector::ZERO;
        let mut total_inertia = 0.0;
        // TODO: avoid this allocation.
        // This is needed because we iterate twice.
        let mut all_props = Vec::new();

        for props in iter {
            let mass = utils::inv(props.inv_mass);
            total_mass += mass;
            total_com += props.local_com * mass;
            all_props.push(props);
        }

        if total_mass > 0.0 {
            total_com /= total_mass;
        }

        for props in all_props {
            total_inertia += props.construct_shifted_inertia_matrix(total_com - props.local_com);
        }

        Self {
            local_com: total_com,
            inv_mass: utils::inv(total_mass),
            inv_principal_inertia: utils::inv(total_inertia),
        }
    }

    #[cfg(feature = "dim3")]
    fn sum<I>(iter: I) -> Self
    where
        I: Iterator<Item = Self>,
    {
        use alloc::vec::Vec;

        let mut total_mass = 0.0;
        let mut total_com = Vector::ZERO;
        let mut total_inertia = Matrix::ZERO;
        // TODO: avoid this allocation.
        // This is needed because we iterate twice.
        let mut all_props = Vec::new();

        for props in iter {
            let mass = utils::inv(props.inv_mass);
            total_mass += mass;
            total_com += props.local_com * mass;
            all_props.push(props);
        }

        if total_mass > 0.0 {
            total_com /= total_mass;
        }

        let total_com = total_com;
        for props in all_props {
            total_inertia += props.construct_shifted_inertia_matrix(total_com - props.local_com);
        }

        Self::with_inertia_matrix(total_com, total_mass, total_inertia)
    }
}

impl approx::AbsDiffEq for MassProperties {
    type Epsilon = Real;
    fn default_epsilon() -> Self::Epsilon {
        Real::default_epsilon()
    }

    fn abs_diff_eq(&self, other: &Self, epsilon: Self::Epsilon) -> bool {
        #[cfg(feature = "dim2")]
        let inertia_is_ok = self
            .inv_principal_inertia
            .abs_diff_eq(&other.inv_principal_inertia, epsilon);

        #[cfg(feature = "dim3")]
        let inertia_is_ok = self
            .reconstruct_inverse_inertia_matrix()
            .abs_diff_eq(other.reconstruct_inverse_inertia_matrix(), epsilon);

        inertia_is_ok
            && self.local_com.abs_diff_eq(other.local_com, epsilon)
            && self.inv_mass.abs_diff_eq(&other.inv_mass, epsilon)
    }
}

impl approx::RelativeEq for MassProperties {
    fn default_max_relative() -> Self::Epsilon {
        Real::default_max_relative()
    }

    fn relative_eq(
        &self,
        other: &Self,
        epsilon: Self::Epsilon,
        max_relative: Self::Epsilon,
    ) -> bool {
        #[cfg(feature = "dim2")]
        let inertia_is_ok = self.inv_principal_inertia.relative_eq(
            &other.inv_principal_inertia,
            epsilon,
            max_relative,
        );

        // Compare either the inertia matrix or its inverse, whichever is most precise.
        #[cfg(feature = "dim3")]
        let inertia_is_ok = self.reconstruct_inverse_inertia_matrix().relative_eq(
            &other.reconstruct_inverse_inertia_matrix(),
            epsilon,
            max_relative,
        ) || self.reconstruct_inertia_matrix().relative_eq(
            &other.reconstruct_inertia_matrix(),
            epsilon,
            max_relative,
        );

        inertia_is_ok
            && self
                .local_com
                .relative_eq(&other.local_com, epsilon, max_relative)
            && self
                .inv_mass
                .relative_eq(&other.inv_mass, epsilon, max_relative)
    }
}

#[cfg(test)]
mod test {
    use super::MassProperties;
    #[cfg(feature = "dim3")]
    use crate::math::Rotation;
    use crate::math::{AngVector, Vector};
    use crate::shape::{Ball, Capsule, Shape};
    use approx::assert_relative_eq;
    use num::Zero;

    #[test]
    fn mass_properties_add_partial_zero() {
        let m1 = MassProperties {
            local_com: Vector::ZERO,
            inv_mass: 2.0,
            #[cfg(feature = "dim2")]
            inv_principal_inertia: 0.0,
            #[cfg(feature = "dim3")]
            inv_principal_inertia: Vector::ZERO,
            #[cfg(feature = "dim3")]
            principal_inertia_local_frame: Rotation::IDENTITY,
        };
        let m2 = MassProperties {
            local_com: Vector::ZERO,
            inv_mass: 0.0,
            #[cfg(feature = "dim2")]
            inv_principal_inertia: 1.0,
            #[cfg(feature = "dim3")]
            inv_principal_inertia: Vector::new(3.0, 2.0, 1.0),
            #[cfg(feature = "dim3")]
            principal_inertia_local_frame: Rotation::IDENTITY,
        };
        let result = MassProperties {
            local_com: Vector::ZERO,
            inv_mass: 2.0,
            #[cfg(feature = "dim2")]
            inv_principal_inertia: 1.0,
            #[cfg(feature = "dim3")]
            inv_principal_inertia: Vector::new(3.0, 2.0, 1.0),
            #[cfg(feature = "dim3")]
            // TODO: ideally this should be IDENTITY, but glam’s conversion from matrix
            //       to quaternion returns this instead. This is OK for the cube test
            //       due to its symmetry though this needs a closer look.
            principal_inertia_local_frame: Rotation::from_xyzw(1.0, 0.0, 0.0, 0.0),
        };

        assert_eq!(m1 + m2, result);
        assert_eq!(m2 + m1, result);
    }

    #[test]
    fn mass_properties_add_sub() {
        // Check that addition and subtraction of mass properties behave as expected.
        let c1 = Capsule::new_x(1.0, 2.0);
        let c2 = Capsule::new_y(3.0, 4.0);
        let c3 = Ball::new(2.0);

        let m1 = c1.mass_properties(1.0);
        let m2 = c2.mass_properties(1.0);
        let m3 = c3.mass_properties(1.0);
        let m1m2m3 = m1 + m2 + m3;

        assert_relative_eq!(m1 + m2, m2 + m1, epsilon = 1.0e-6);
        assert_relative_eq!(m1m2m3 - m1, m2 + m3, epsilon = 1.0e-6);
        assert_relative_eq!(m1m2m3 - m2, m1 + m3, epsilon = 1.0e-6);
        assert_relative_eq!(m1m2m3 - m3, m1 + m2, epsilon = 1.0e-6);
        assert_relative_eq!(m1m2m3 - (m1 + m2), m3, epsilon = 1.0e-5);
        assert_relative_eq!(m1m2m3 - (m1 + m3), m2, epsilon = 1.0e-6);
        assert_relative_eq!(m1m2m3 - (m2 + m3), m1, epsilon = 1.0e-6);
        assert_relative_eq!(m1m2m3 - m1 - m2, m3, epsilon = 1.0e-5);
        assert_relative_eq!(m1m2m3 - m1 - m3, m2, epsilon = 1.0e-6);
        assert_relative_eq!(m1m2m3 - m2 - m3, m1, epsilon = 1.0e-6);
        assert_relative_eq!(m1m2m3 - m2 - m3, m1, epsilon = 1.0e-6);

        // NOTE: converting the inverse inertia matrices don't work well here because
        //       tiny inertia value originating from the subtraction can result in a non-zero
        //       (but large) inverse.
        assert_relative_eq!(
            (((m1m2m3 - m1) - m2) - m3).principal_inertia(),
            AngVector::default(),
            epsilon = 1.0e-2
        );
        assert_relative_eq!((((m1m2m3 - m1) - m2) - m3).mass(), 0.0, epsilon = 1.0e-6);
    }

    #[test]
    #[cfg(feature = "alloc")]
    fn mass_properties_compound() {
        use crate::{
            math::{Pose, Vector},
            shape::{Compound, Cuboid, SharedShape},
        };

        // Compute the mass properties of a compound shape made of three 1x1x1 cuboids.
        let shape = Cuboid::new(Vector::splat(0.5));
        let mp = shape.mass_properties(1.0);
        let iso2 = Pose::from_parts(Vector::Y.into(), Default::default());
        let iso3 = Pose::from_parts((-Vector::Y).into(), Default::default());

        // Test sum shifted result through `MassProperties::add`
        let sum = [mp, mp.transform_by(&iso2), mp.transform_by(&iso3)]
            .into_iter()
            .sum::<MassProperties>();

        // Test compound through `MassProperties::from_compound`
        let compound_shape = Compound::new(vec![
            (
                Pose::from_parts(Vector::default().into(), Default::default()),
                SharedShape::new(shape),
            ),
            (iso2, SharedShape::new(shape)),
            (iso3, SharedShape::new(shape)),
        ]);
        let mp_compound = compound_shape.mass_properties(1.0);

        // Check that the mass properties of the compound shape match the mass properties
        // of a single 1x3x1 cuboid.
        #[cfg(feature = "dim2")]
        let expected = Cuboid::new(Vector::new(1.5, 0.5)).mass_properties(1.0);
        #[cfg(feature = "dim3")]
        let expected = Cuboid::new(Vector::new(1.5, 0.5, 0.5)).mass_properties(1.0);

        // Sum shifted
        assert_relative_eq!(sum.local_com, expected.local_com, epsilon = 1.0e-6);
        assert_relative_eq!(sum.inv_mass, expected.inv_mass, epsilon = 1.0e-6);
        #[cfg(feature = "dim3")]
        assert!(sum
            .inv_principal_inertia
            .abs_diff_eq(expected.inv_principal_inertia, 1.0e-6));
        #[cfg(feature = "dim2")]
        assert_relative_eq!(
            sum.inv_principal_inertia,
            expected.inv_principal_inertia,
            epsilon = 1.0e-6
        );

        // Compound
        assert_relative_eq!(mp_compound.local_com, expected.local_com, epsilon = 1.0e-6);
        assert_relative_eq!(mp_compound.inv_mass, expected.inv_mass, epsilon = 1.0e-6);
        #[cfg(feature = "dim3")]
        assert!(mp_compound
            .inv_principal_inertia
            .abs_diff_eq(expected.inv_principal_inertia, 1.0e-6));
        #[cfg(feature = "dim2")]
        assert_relative_eq!(
            mp_compound.inv_principal_inertia,
            expected.inv_principal_inertia,
            epsilon = 1.0e-6
        );
    }

    #[test]
    #[cfg(feature = "alloc")]
    fn mass_properties_sum_no_nan() {
        let mp: MassProperties = [MassProperties::zero()].iter().copied().sum();
        assert!(!mp.local_com.x.is_nan() && !mp.local_com.y.is_nan());
        #[cfg(feature = "dim3")]
        assert!(!mp.local_com.z.is_nan());
    }
}
