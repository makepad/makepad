use crate::dynamics::RigidBody;
use crate::math::{Real, SPATIAL_DIM, Vector};
use crate::utils::{RotationOps, ScalarType};
use na::{DVectorView, DVectorViewMut};
use parry::math::{Pose, Rotation, SIMD_WIDTH, SimdReal};
use std::ops::{AddAssign, Sub, SubAssign};

#[cfg(feature = "simd-is-enabled")]
use crate::utils::transmute_to_wide;

#[cfg(feature = "simd-is-enabled")]
macro_rules! aos(
    ($data_repr: ident [ $idx: ident ] . $data_n: ident, $fallback: ident) => {
        [
            if ($idx[0] as usize) < $data_repr.len() {
                $data_repr[$idx[0] as usize].$data_n.0
            } else {
                $fallback.$data_n.0
            },
            if ($idx[1] as usize) < $data_repr.len() {
                $data_repr[$idx[1] as usize].$data_n.0
            } else {
                $fallback.$data_n.0
            },
            if ($idx[2] as usize) < $data_repr.len() {
                $data_repr[$idx[2] as usize].$data_n.0
            } else {
                $fallback.$data_n.0
            },
            if ($idx[3] as usize) < $data_repr.len() {
                $data_repr[$idx[3] as usize].$data_n.0
            } else {
                $fallback.$data_n.0
            },
        ]
    }
);

#[cfg(feature = "simd-is-enabled")]
macro_rules! aos_unchecked(
    ($data_repr: ident [ $idx: ident ] . $data_n: ident) => {
        [
            unsafe { $data_repr.get_unchecked($idx[0] as usize).$data_n.0 },
            unsafe { $data_repr.get_unchecked($idx[1] as usize).$data_n.0 },
            unsafe { $data_repr.get_unchecked($idx[2] as usize).$data_n.0 },
            unsafe { $data_repr.get_unchecked($idx[3] as usize).$data_n.0 },
        ]
    }
);

#[cfg(feature = "simd-is-enabled")]
macro_rules! scatter(
    ($data: ident [ $idx: ident [ $i: expr ] ] = [$($aos: ident),*]) => {
       unsafe {
            #[allow(clippy::missing_transmute_annotations)] // Different macro calls transmute to different types
            if ($idx[$i] as usize) < $data.len() {
                $data[$idx[$i] as usize] = std::mem::transmute([$($aos[$i]),*]);
            }
        }
    }
);

#[cfg(feature = "simd-is-enabled")]
macro_rules! scatter_unchecked(
    ($data: ident [ $idx: ident [ $i: expr ] ] = [$($aos: ident),*]) => {
       #[allow(clippy::missing_transmute_annotations)] // Different macro calls transmute to different types
       unsafe {
           *$data.get_unchecked_mut($idx[$i] as usize) = std::mem::transmute([$($aos[$i]),*]);
       }
    }
);

#[derive(Default)]
pub struct SolverBodies {
    pub vels: Vec<SolverVel<Real>>,
    pub poses: Vec<SolverPose<Real>>,
}

impl SolverBodies {
    pub fn clear(&mut self) {
        self.vels.clear();
        self.poses.clear();
    }

    pub fn resize(&mut self, sz: usize) {
        self.vels.resize(sz, Default::default());
        self.poses.resize(sz, Default::default());
    }

    pub fn len(&self) -> usize {
        self.vels.len()
    }

    // TODO: add a SIMD version?
    pub fn copy_from(&mut self, _dt: Real, i: usize, rb: &RigidBody) {
        let poses = &mut self.poses[i];
        let vels = &mut self.vels[i];

        #[cfg(feature = "dim2")]
        {
            vels.angular = rb.vels.angvel;
        }

        #[cfg(feature = "dim3")]
        {
            if rb.forces.gyroscopic_forces_enabled {
                vels.angular = rb.angvel_with_gyroscopic_forces(_dt);
            } else {
                vels.angular = rb.angvel();
            }
        }
        vels.linear = rb.vels.linvel;

        let pose = rb
            .pos
            .position
            .prepend_translation(rb.mprops.local_mprops.local_com);
        poses.rotation = pose.rotation;
        poses.translation = pose.translation;

        if rb.is_dynamic_or_kinematic() {
            poses.ii = rb.mprops.effective_world_inv_inertia;
            poses.im = rb.mprops.effective_inv_mass;
        } else {
            poses.ii = Default::default();
            poses.im = Default::default();
        }
    }

    #[inline]
    pub unsafe fn gather_vels_unchecked(&self, idx: [u32; SIMD_WIDTH]) -> SolverVel<SimdReal> {
        #[cfg(not(feature = "simd-is-enabled"))]
        unsafe {
            *self.vels.get_unchecked(idx[0] as usize)
        }
        #[cfg(feature = "simd-is-enabled")]
        unsafe {
            SolverVel::gather_unchecked(&self.vels, idx)
        }
    }

    #[inline]
    pub fn gather_vels(&self, idx: [u32; SIMD_WIDTH]) -> SolverVel<SimdReal> {
        #[cfg(not(feature = "simd-is-enabled"))]
        return self.vels.get(idx[0] as usize).copied().unwrap_or_default();
        #[cfg(feature = "simd-is-enabled")]
        return SolverVel::gather(&self.vels, idx);
    }

    #[inline]
    pub fn get_vel(&self, i: u32) -> SolverVel<Real> {
        self.vels.get(i as usize).copied().unwrap_or_default()
    }

    #[inline]
    pub fn scatter_vels(&mut self, idx: [u32; SIMD_WIDTH], vels: SolverVel<SimdReal>) {
        #[cfg(not(feature = "simd-is-enabled"))]
        if (idx[0] as usize) < self.vels.len() {
            self.vels[idx[0] as usize] = vels
        }

        #[cfg(feature = "simd-is-enabled")]
        vels.scatter(&mut self.vels, idx);
    }

    #[inline]
    pub fn set_vel(&mut self, i: u32, vel: SolverVel<Real>) {
        if (i as usize) < self.vels.len() {
            self.vels[i as usize] = vel;
        }
    }

    #[inline]
    pub fn get_pose(&self, i: u32) -> SolverPose<Real> {
        self.poses.get(i as usize).copied().unwrap_or_default()
    }

    #[inline]
    pub unsafe fn gather_poses_unchecked(&self, idx: [u32; SIMD_WIDTH]) -> SolverPose<SimdReal> {
        #[cfg(not(feature = "simd-is-enabled"))]
        unsafe {
            *self.poses.get_unchecked(idx[0] as usize)
        }

        #[cfg(feature = "simd-is-enabled")]
        unsafe {
            SolverPose::gather_unchecked(&self.poses, idx)
        }
    }

    #[inline]
    pub fn gather_poses(&self, idx: [u32; SIMD_WIDTH]) -> SolverPose<SimdReal> {
        #[cfg(not(feature = "simd-is-enabled"))]
        return self.poses.get(idx[0] as usize).copied().unwrap_or_default();

        #[cfg(feature = "simd-is-enabled")]
        return SolverPose::gather(&self.poses, idx);
    }

    #[inline]
    pub fn scatter_poses(&mut self, idx: [u32; SIMD_WIDTH], poses: SolverPose<SimdReal>) {
        #[cfg(not(feature = "simd-is-enabled"))]
        if (idx[0] as usize) < self.poses.len() {
            self.poses[idx[0] as usize] = poses;
        }

        #[cfg(feature = "simd-is-enabled")]
        poses.scatter(&mut self.poses, idx);
    }

    #[inline]
    pub fn scatter_poses_unchecked(&mut self, idx: [u32; SIMD_WIDTH], poses: SolverPose<SimdReal>) {
        #[cfg(not(feature = "simd-is-enabled"))]
        unsafe {
            *self.poses.get_unchecked_mut(idx[0] as usize) = poses
        }

        #[cfg(feature = "simd-is-enabled")]
        poses.scatter_unchecked(&mut self.poses, idx);
    }
}

// Total 7/13
#[repr(C)]
#[cfg_attr(feature = "simd-is-enabled", repr(align(16)))]
#[derive(Copy, Clone, Default)]
pub struct SolverVel<T: ScalarType> {
    pub linear: T::Vector,     // 2/3
    pub angular: T::AngVector, // 1/3
    // TODO: explicit padding are useful for static assertions.
    //       But might be wasteful for the SolverVel<SimdReal>
    //       specialization.
    #[cfg(feature = "simd-is-enabled")]
    #[cfg(feature = "dim2")]
    padding: [T; 1],
    #[cfg(feature = "simd-is-enabled")]
    #[cfg(feature = "dim3")]
    padding: [T; 2],
}

#[cfg(feature = "simd-is-enabled")]
#[repr(C)]
struct SolverVelRepr {
    data0: SimdReal,
    #[cfg(feature = "dim3")]
    data1: SimdReal,
}

#[cfg(feature = "simd-is-enabled")]
impl SolverVelRepr {
    pub fn zero() -> Self {
        Self {
            data0: na::zero(),
            #[cfg(feature = "dim3")]
            data1: na::zero(),
        }
    }
}

#[cfg(feature = "simd-is-enabled")]
impl SolverVel<SimdReal> {
    #[inline]
    pub unsafe fn gather_unchecked(data: &[SolverVel<Real>], idx: [u32; SIMD_WIDTH]) -> Self {
        // TODO: double-check that the compiler is using simd loads and
        //       isn’t generating useless copies.

        let data_repr: &[SolverVelRepr] = unsafe { std::mem::transmute(data) };

        #[cfg(feature = "dim2")]
        {
            let aos = aos_unchecked!(data_repr[idx].data0);
            let soa = wide::f32x4::transpose(transmute_to_wide(aos));
            unsafe { std::mem::transmute(soa) }
        }

        #[cfg(feature = "dim3")]
        {
            let aos0 = aos_unchecked!(data_repr[idx].data0);
            let soa0 = wide::f32x4::transpose(transmute_to_wide(aos0));
            let aos1 = aos_unchecked!(data_repr[idx].data1);
            let soa1 = wide::f32x4::transpose(transmute_to_wide(aos1));
            unsafe { std::mem::transmute((soa0, soa1)) }
        }
    }

    #[inline]
    pub fn gather(data: &[SolverVel<Real>], idx: [u32; SIMD_WIDTH]) -> Self {
        // TODO: double-check that the compiler is using simd loads and
        //       isn’t generating useless copies.

        let zero = SolverVelRepr::zero();
        let data_repr: &[SolverVelRepr] = unsafe { std::mem::transmute(data) };

        #[cfg(feature = "dim2")]
        {
            let aos = aos!(data_repr[idx].data0, zero);
            let soa = wide::f32x4::transpose(transmute_to_wide(aos));
            unsafe { std::mem::transmute(soa) }
        }

        #[cfg(feature = "dim3")]
        {
            let aos0 = aos!(data_repr[idx].data0, zero);
            let soa0 = wide::f32x4::transpose(transmute_to_wide(aos0));
            let aos1 = aos!(data_repr[idx].data1, zero);
            let soa1 = wide::f32x4::transpose(transmute_to_wide(aos1));
            unsafe { std::mem::transmute((soa0, soa1)) }
        }
    }

    #[inline]
    #[cfg(feature = "dim2")]
    pub fn scatter(self, data: &mut [SolverVel<Real>], idx: [u32; SIMD_WIDTH]) {
        // TODO: double-check that the compiler is using simd loads and no useless copies.
        let soa: [wide::f32x4; 4] = unsafe { std::mem::transmute(self) };
        let aos = wide::f32x4::transpose(soa);
        scatter!(data[idx[0]] = [aos]);
        scatter!(data[idx[1]] = [aos]);
        scatter!(data[idx[2]] = [aos]);
        scatter!(data[idx[3]] = [aos]);
    }

    #[inline]
    #[cfg(feature = "dim3")]
    pub fn scatter(self, data: &mut [SolverVel<Real>], idx: [u32; SIMD_WIDTH]) {
        let soa: [[wide::f32x4; 4]; 2] = unsafe { std::mem::transmute(self) };
        // TODO: double-check that the compiler is using simd loads and no useless copies.
        let aos0 = wide::f32x4::transpose(soa[0]);
        let aos1 = wide::f32x4::transpose(soa[1]);
        scatter!(data[idx[0]] = [aos0, aos1]);
        scatter!(data[idx[1]] = [aos0, aos1]);
        scatter!(data[idx[2]] = [aos0, aos1]);
        scatter!(data[idx[3]] = [aos0, aos1]);
    }
}

// Total: 7/16
#[repr(C)]
#[cfg_attr(feature = "simd-is-enabled", repr(align(16)))]
#[derive(Copy, Clone)]
pub struct SolverPose<N: ScalarType> {
    /// Positional change of the rigid-body’s center of mass.
    pub rotation: N::Rotation, // 2/4
    pub translation: N::Vector, // 2/3
    pub ii: N::AngInertia,      // 1/6
    pub im: N::Vector,          // 2/3
    #[cfg(feature = "dim2")]
    pub padding: [N; 1],
}

impl SolverPose<Real> {
    pub fn pose(&self) -> Pose {
        Pose::from_parts(self.translation, self.rotation)
    }
}

#[cfg(feature = "simd-is-enabled")]
impl SolverPose<SimdReal> {
    pub fn pose(&self) -> <SimdReal as ScalarType>::Pose {
        <SimdReal as ScalarType>::Pose::from_parts(self.translation.into(), self.rotation)
    }
}

impl<N: ScalarType> SolverPose<N> {
    #[inline]
    pub fn transform_point(&self, pt: N::Vector) -> N::Vector {
        self.rotation * pt + self.translation
    }

    #[inline]
    pub fn inverse_transform_point(&self, pt: N::Vector) -> N::Vector {
        self.rotation.inverse() * (pt - self.translation)
    }
}

impl Default for SolverPose<Real> {
    #[inline]
    fn default() -> Self {
        Self {
            rotation: Rotation::IDENTITY,
            translation: Vector::ZERO,
            ii: Default::default(),
            im: Default::default(),
            #[cfg(feature = "dim2")]
            padding: Default::default(),
        }
    }
}

#[cfg(feature = "simd-is-enabled")]
#[repr(C)]
struct SolverPoseRepr {
    data0: SimdReal,
    data1: SimdReal,
    #[cfg(feature = "dim3")]
    data2: SimdReal,
    #[cfg(feature = "dim3")]
    data3: SimdReal,
}

#[cfg(feature = "simd-is-enabled")]
impl SolverPoseRepr {
    pub fn identity() -> Self {
        // TODO PERF: will the compiler handle this efficiently and generate
        //            everything at compile-time?
        unsafe { std::mem::transmute(SolverPose::default()) }
    }
}

#[cfg(feature = "simd-is-enabled")]
impl SolverPose<SimdReal> {
    #[inline]
    pub unsafe fn gather_unchecked(data: &[SolverPose<Real>], idx: [u32; SIMD_WIDTH]) -> Self {
        // TODO: double-check that the compiler is using simd loads and
        //       isn’t generating useless copies.

        let data_repr: &[SolverPoseRepr] = unsafe { std::mem::transmute(data) };

        #[cfg(feature = "dim2")]
        {
            let aos0 = aos_unchecked!(data_repr[idx].data0);
            let aos1 = aos_unchecked!(data_repr[idx].data1);
            let soa0 = wide::f32x4::transpose(transmute_to_wide(aos0));
            let soa1 = wide::f32x4::transpose(transmute_to_wide(aos1));
            unsafe { std::mem::transmute([soa0, soa1]) }
        }

        #[cfg(feature = "dim3")]
        {
            let aos0 = aos_unchecked!(data_repr[idx].data0);
            let aos1 = aos_unchecked!(data_repr[idx].data1);
            let aos2 = aos_unchecked!(data_repr[idx].data2);
            let aos3 = aos_unchecked!(data_repr[idx].data3);
            let soa0 = wide::f32x4::transpose(transmute_to_wide(aos0));
            let soa1 = wide::f32x4::transpose(transmute_to_wide(aos1));
            let soa2 = wide::f32x4::transpose(transmute_to_wide(aos2));
            let soa3 = wide::f32x4::transpose(transmute_to_wide(aos3));
            unsafe { std::mem::transmute([soa0, soa1, soa2, soa3]) }
        }
    }

    #[inline]
    pub fn gather(data: &[SolverPose<Real>], idx: [u32; SIMD_WIDTH]) -> Self {
        // TODO: double-check that the compiler is using simd loads and
        //       isn’t generating useless copies.

        let identity = SolverPoseRepr::identity();
        let data_repr: &[SolverPoseRepr] = unsafe { std::mem::transmute(data) };

        #[cfg(feature = "dim2")]
        {
            let aos0 = aos!(data_repr[idx].data0, identity);
            let aos1 = aos!(data_repr[idx].data1, identity);
            let soa0 = wide::f32x4::transpose(transmute_to_wide(aos0));
            let soa1 = wide::f32x4::transpose(transmute_to_wide(aos1));
            unsafe { std::mem::transmute([soa0, soa1]) }
        }

        #[cfg(feature = "dim3")]
        {
            let aos0 = aos!(data_repr[idx].data0, identity);
            let aos1 = aos!(data_repr[idx].data1, identity);
            let aos2 = aos!(data_repr[idx].data2, identity);
            let aos3 = aos!(data_repr[idx].data3, identity);
            let soa0 = wide::f32x4::transpose(transmute_to_wide(aos0));
            let soa1 = wide::f32x4::transpose(transmute_to_wide(aos1));
            let soa2 = wide::f32x4::transpose(transmute_to_wide(aos2));
            let soa3 = wide::f32x4::transpose(transmute_to_wide(aos3));
            unsafe { std::mem::transmute([soa0, soa1, soa2, soa3]) }
        }
    }

    #[inline]
    #[cfg(feature = "dim2")]
    pub fn scatter_unchecked(self, data: &mut [SolverPose<Real>], idx: [u32; SIMD_WIDTH]) {
        // TODO: double-check that the compiler is using simd loads and no useless copies.
        let soa: [[wide::f32x4; 4]; 2] = unsafe { std::mem::transmute(self) };
        let aos0 = wide::f32x4::transpose(soa[0]);
        let aos1 = wide::f32x4::transpose(soa[1]);
        scatter_unchecked!(data[idx[0]] = [aos0, aos1]);
        scatter_unchecked!(data[idx[1]] = [aos0, aos1]);
        scatter_unchecked!(data[idx[2]] = [aos0, aos1]);
        scatter_unchecked!(data[idx[3]] = [aos0, aos1]);
    }

    #[inline]
    #[cfg(feature = "dim3")]
    pub fn scatter_unchecked(self, data: &mut [SolverPose<Real>], idx: [u32; SIMD_WIDTH]) {
        let soa: [[wide::f32x4; 4]; 4] = unsafe { std::mem::transmute(self) };
        // TODO: double-check that the compiler is using simd loads and no useless copies.
        let aos0 = wide::f32x4::transpose(soa[0]);
        let aos1 = wide::f32x4::transpose(soa[1]);
        let aos2 = wide::f32x4::transpose(soa[2]);
        let aos3 = wide::f32x4::transpose(soa[3]);
        scatter_unchecked!(data[idx[0]] = [aos0, aos1, aos2, aos3]);
        scatter_unchecked!(data[idx[1]] = [aos0, aos1, aos2, aos3]);
        scatter_unchecked!(data[idx[2]] = [aos0, aos1, aos2, aos3]);
        scatter_unchecked!(data[idx[3]] = [aos0, aos1, aos2, aos3]);
    }

    #[inline]
    #[cfg(feature = "dim2")]
    pub fn scatter(self, data: &mut [SolverPose<Real>], idx: [u32; SIMD_WIDTH]) {
        // TODO: double-check that the compiler is using simd loads and no useless copies.
        let soa: [[wide::f32x4; 4]; 2] = unsafe { std::mem::transmute(self) };
        let aos0 = wide::f32x4::transpose(soa[0]);
        let aos1 = wide::f32x4::transpose(soa[1]);
        scatter!(data[idx[0]] = [aos0, aos1]);
        scatter!(data[idx[1]] = [aos0, aos1]);
        scatter!(data[idx[2]] = [aos0, aos1]);
        scatter!(data[idx[3]] = [aos0, aos1]);
    }

    #[inline]
    #[cfg(feature = "dim3")]
    pub fn scatter(self, data: &mut [SolverPose<Real>], idx: [u32; SIMD_WIDTH]) {
        let soa: [[wide::f32x4; 4]; 4] = unsafe { std::mem::transmute(self) };
        // TODO: double-check that the compiler is using simd loads and no useless copies.
        let aos0 = wide::f32x4::transpose(soa[0]);
        let aos1 = wide::f32x4::transpose(soa[1]);
        let aos2 = wide::f32x4::transpose(soa[2]);
        let aos3 = wide::f32x4::transpose(soa[3]);
        scatter!(data[idx[0]] = [aos0, aos1, aos2, aos3]);
        scatter!(data[idx[1]] = [aos0, aos1, aos2, aos3]);
        scatter!(data[idx[2]] = [aos0, aos1, aos2, aos3]);
        scatter!(data[idx[3]] = [aos0, aos1, aos2, aos3]);
    }
}

impl<N: ScalarType> SolverVel<N> {
    pub fn as_slice(&self) -> &[N; SPATIAL_DIM] {
        unsafe { std::mem::transmute(self) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [N; SPATIAL_DIM] {
        unsafe { std::mem::transmute(self) }
    }

    pub fn as_vector_slice(&self) -> DVectorView<'_, N> {
        DVectorView::from_slice(&self.as_slice()[..], SPATIAL_DIM)
    }

    pub fn as_vector_slice_mut(&mut self) -> DVectorViewMut<'_, N> {
        DVectorViewMut::from_slice(&mut self.as_mut_slice()[..], SPATIAL_DIM)
    }
}

impl<N: ScalarType> SolverVel<N> {
    pub fn zero() -> Self {
        Self {
            linear: Default::default(),
            angular: Default::default(),
            #[cfg(feature = "simd-is-enabled")]
            #[cfg(feature = "dim2")]
            padding: [na::zero(); 1],
            #[cfg(feature = "simd-is-enabled")]
            #[cfg(feature = "dim3")]
            padding: [na::zero(); 2],
        }
    }
}

impl<N: ScalarType> AddAssign for SolverVel<N> {
    fn add_assign(&mut self, rhs: Self) {
        self.linear += rhs.linear;
        self.angular += rhs.angular;
    }
}

impl<N: ScalarType> SubAssign for SolverVel<N> {
    fn sub_assign(&mut self, rhs: Self) {
        self.linear -= rhs.linear;
        self.angular -= rhs.angular;
    }
}

impl<N: ScalarType> Sub for SolverVel<N> {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        SolverVel {
            linear: self.linear - rhs.linear,
            angular: self.angular - rhs.angular,
            #[cfg(feature = "simd-is-enabled")]
            padding: self.padding,
        }
    }
}
