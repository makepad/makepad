use super::multibody_link::{MultibodyLink, MultibodyLinkVec};
use super::multibody_workspace::MultibodyWorkspace;
use crate::dynamics::{RigidBodyHandle, RigidBodySet, RigidBodyType, RigidBodyVelocity};
use crate::math::{
    ANG_DIM, AngDim, AngVector, DIM, DVector, Dim, Jacobian, Pose, Real, SPATIAL_DIM,
    Vector,
};
use crate::prelude::MultibodyJoint;
#[cfg(feature = "dim3")]
use crate::utils::mat_to_na;
use crate::utils::{AngularInertiaOps, CrossProduct, CrossProductMatrix, IndexMut2, vect_to_na};
use na::{
    self, DMatrix, DVectorView, DVectorViewMut, Dyn, LU, OMatrix, SMatrix, SVector, StorageMut,
};

#[cfg(doc)]
use crate::prelude::{GenericJoint, RigidBody};

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
struct Force {
    linear: Vector,
    angular: AngVector,
}

impl Force {
    fn new(linear: Vector, angular: AngVector) -> Self {
        Self { linear, angular }
    }

    fn as_vector(&self) -> &SVector<Real, SPATIAL_DIM> {
        unsafe { std::mem::transmute(self) }
    }
}

#[cfg(feature = "dim2")]
fn concat_rb_mass_matrix(mass: Vector, inertia: Real) -> SMatrix<Real, SPATIAL_DIM, SPATIAL_DIM> {
    let mut result = SMatrix::<Real, SPATIAL_DIM, SPATIAL_DIM>::zeros();
    result[(0, 0)] = mass.x;
    result[(1, 1)] = mass.y;
    result[(2, 2)] = inertia;
    result
}

#[cfg(feature = "dim3")]
fn concat_rb_mass_matrix(
    mass: Vector,
    inertia: na::Matrix3<Real>,
) -> SMatrix<Real, SPATIAL_DIM, SPATIAL_DIM> {
    let mut result = SMatrix::<Real, SPATIAL_DIM, SPATIAL_DIM>::zeros();
    result[(0, 0)] = mass.x;
    result[(1, 1)] = mass.y;
    result[(2, 2)] = mass.z;
    result
        .fixed_view_mut::<ANG_DIM, ANG_DIM>(DIM, DIM)
        .copy_from(&inertia);
    result
}

/// An articulated body simulated using the reduced-coordinates approach.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Debug)]
pub struct Multibody {
    // TODO: serialization: skip the workspace fields.
    pub(crate) links: MultibodyLinkVec,
    pub(crate) velocities: DVector,
    pub(crate) damping: DVector,
    pub(crate) accelerations: DVector,

    body_jacobians: Vec<Jacobian<Real>>,
    // NOTE: the mass matrices are dimensioned based on the non-kinematic degrees of
    //       freedoms only. The `Self::augmented_mass_permutation` sequence can be used to
    //       move dofs from/to a format that matches the augmented mass.
    // TODO: use sparse matrices?
    augmented_mass: DMatrix<Real>,
    inv_augmented_mass: LU<Real, Dyn, Dyn>,
    // The indexing sequence for moving all kinematics degrees of
    // freedoms to the end of the generalized coordinates vector.
    augmented_mass_indices: IndexSequence,

    acc_augmented_mass: DMatrix<Real>,
    acc_inv_augmented_mass: LU<Real, Dyn, Dyn>,

    ndofs: usize,
    pub(crate) root_is_dynamic: bool,
    pub(crate) solver_id: u32,
    self_contacts_enabled: bool,

    /*
     * Workspaces.
     */
    workspace: MultibodyWorkspace,
    coriolis_v: Vec<OMatrix<Real, Dim, Dyn>>,
    coriolis_w: Vec<OMatrix<Real, AngDim, Dyn>>,
    i_coriolis_dt: Jacobian<Real>,
}
impl Default for Multibody {
    fn default() -> Self {
        Multibody::new()
    }
}

impl Multibody {
    /// Creates a new multibody with no link.
    pub fn new() -> Self {
        Self::with_self_contacts(true)
    }

    pub(crate) fn with_self_contacts(self_contacts_enabled: bool) -> Self {
        Multibody {
            links: MultibodyLinkVec(Vec::new()),
            velocities: DVector::zeros(0),
            damping: DVector::zeros(0),
            accelerations: DVector::zeros(0),
            body_jacobians: Vec::new(),
            augmented_mass: DMatrix::zeros(0, 0),
            inv_augmented_mass: LU::new(DMatrix::zeros(0, 0)),
            acc_augmented_mass: DMatrix::zeros(0, 0),
            acc_inv_augmented_mass: LU::new(DMatrix::zeros(0, 0)),
            augmented_mass_indices: IndexSequence::new(),
            ndofs: 0,
            solver_id: 0,
            workspace: MultibodyWorkspace::new(),
            coriolis_v: Vec::new(),
            coriolis_w: Vec::new(),
            i_coriolis_dt: Jacobian::zeros(0),
            root_is_dynamic: false,
            self_contacts_enabled,
            // solver_workspace: Some(SolverWorkspace::new()),
        }
    }

    pub(crate) fn with_root(handle: RigidBodyHandle, self_contacts_enabled: bool) -> Self {
        let mut mb = Multibody::with_self_contacts(self_contacts_enabled);
        // NOTE: we have no way of knowing if the root in fixed at this point, so
        //       we mark it as dynamic and will fix later with `Self::update_root_type`.
        mb.root_is_dynamic = true;
        let joint = MultibodyJoint::free(Pose::IDENTITY);
        mb.add_link(None, joint, handle);
        mb
    }

    pub(crate) fn remove_link(self, to_remove: usize, joint_only: bool) -> Vec<Multibody> {
        let mut result = vec![];
        let mut link2mb = vec![usize::MAX; self.links.len()];
        let mut link_id2new_id = vec![usize::MAX; self.links.len()];

        // Split multibody and update the set of links and ndofs.
        for (i, mut link) in self.links.0.into_iter().enumerate() {
            let is_new_root = i == 0
                || !joint_only && link.parent_internal_id == to_remove
                || joint_only && i == to_remove;

            if !joint_only && i == to_remove {
                continue;
            } else if is_new_root {
                link2mb[i] = result.len();
                result.push(Multibody::with_self_contacts(self.self_contacts_enabled));
            } else {
                link2mb[i] = link2mb[link.parent_internal_id]
            }

            let curr_mb = &mut result[link2mb[i]];
            link_id2new_id[i] = curr_mb.links.len();

            if is_new_root {
                let joint = MultibodyJoint::fixed(*link.local_to_world());
                link.joint = joint;
            }

            curr_mb.ndofs += link.joint().ndofs();
            curr_mb.links.push(link);
        }

        // Adjust all the internal ids, and copy the data from the
        // previous multibody to the new one.
        for mb in &mut result {
            mb.grow_buffers(mb.ndofs, mb.links.len());
            mb.workspace.resize(mb.links.len(), mb.ndofs);

            let mut assembly_id = 0;
            for (i, link) in mb.links.iter_mut().enumerate() {
                let link_ndofs = link.joint().ndofs();
                mb.velocities
                    .rows_mut(assembly_id, link_ndofs)
                    .copy_from(&self.velocities.rows(link.assembly_id, link_ndofs));
                mb.damping
                    .rows_mut(assembly_id, link_ndofs)
                    .copy_from(&self.damping.rows(link.assembly_id, link_ndofs));
                mb.accelerations
                    .rows_mut(assembly_id, link_ndofs)
                    .copy_from(&self.accelerations.rows(link.assembly_id, link_ndofs));

                link.internal_id = i;
                link.assembly_id = assembly_id;

                // NOTE: for the root, the current`link.parent_internal_id` is invalid since that
                //       parent lies in a different multibody now.
                link.parent_internal_id = if i != 0 {
                    link_id2new_id[link.parent_internal_id]
                } else {
                    0
                };
                assembly_id += link_ndofs;
            }
        }

        result
    }

    pub(crate) fn append(&mut self, mut rhs: Multibody, parent: usize, joint: MultibodyJoint) {
        let rhs_root_ndofs = rhs.links[0].joint.ndofs();
        // Values for rhs will be copied into the buffers of `self` starting at this index.
        let rhs_copy_shift = self.ndofs + joint.ndofs();
        // Number of dofs to copy from rhs. The root’s dofs isn’t included because it will be
        // replaced by `joint.
        let rhs_copy_ndofs = rhs.ndofs - rhs_root_ndofs;

        // Adjust the ids of all the rhs links except the first one.
        let base_assembly_id = self.velocities.len() - rhs_root_ndofs + joint.ndofs();
        let base_internal_id = self.links.len() + 1;
        let base_parent_id = self.links.len();

        for link in &mut rhs.links.0[1..] {
            link.assembly_id += base_assembly_id;
            link.internal_id += base_internal_id;
            link.parent_internal_id += base_parent_id;
        }

        // Adjust the first link.
        {
            rhs.links[0].joint = joint;
            rhs.links[0].assembly_id = self.velocities.len();
            rhs.links[0].internal_id = self.links.len();
            rhs.links[0].parent_internal_id = parent;
        }

        // Grow buffers then append data from rhs.
        self.grow_buffers(rhs_copy_ndofs + rhs.links[0].joint.ndofs(), rhs.links.len());

        if rhs_copy_ndofs > 0 {
            self.velocities
                .rows_mut(rhs_copy_shift, rhs_copy_ndofs)
                .copy_from(&rhs.velocities.rows(rhs_root_ndofs, rhs_copy_ndofs));
            self.damping
                .rows_mut(rhs_copy_shift, rhs_copy_ndofs)
                .copy_from(&rhs.damping.rows(rhs_root_ndofs, rhs_copy_ndofs));
            self.accelerations
                .rows_mut(rhs_copy_shift, rhs_copy_ndofs)
                .copy_from(&rhs.accelerations.rows(rhs_root_ndofs, rhs_copy_ndofs));
        }

        rhs.links[0]
            .joint
            .default_damping(&mut self.damping.rows_mut(base_assembly_id, rhs_root_ndofs));

        self.links.append(&mut rhs.links);
        self.ndofs = self.velocities.len();
        self.workspace.resize(self.links.len(), self.ndofs);
    }

    /// Whether self-contacts are enabled on this multibody.
    ///
    /// If set to `false` no two link from this multibody can generate contacts, even
    /// if the contact is enabled on the individual joint with [`GenericJoint::contacts_enabled`].
    pub fn self_contacts_enabled(&self) -> bool {
        self.self_contacts_enabled
    }

    /// Sets whether self-contacts are enabled on this multibody.
    ///
    /// If set to `false` no two link from this multibody can generate contacts, even
    /// if the contact is enabled on the individual joint with [`GenericJoint::contacts_enabled`].
    pub fn set_self_contacts_enabled(&mut self, enabled: bool) {
        self.self_contacts_enabled = enabled;
    }

    /// The inverse augmented mass matrix of this multibody.
    pub fn inv_augmented_mass(&self) -> &LU<Real, Dyn, Dyn> {
        &self.inv_augmented_mass
    }

    /// The first link of this multibody.
    #[inline]
    pub fn root(&self) -> &MultibodyLink {
        &self.links[0]
    }

    /// Mutable reference to the first link of this multibody.
    #[inline]
    pub fn root_mut(&mut self) -> &mut MultibodyLink {
        &mut self.links[0]
    }

    /// Reference `i`-th multibody link of this multibody.
    ///
    /// Return `None` if there is less than `i + 1` multibody links.
    #[inline]
    pub fn link(&self, id: usize) -> Option<&MultibodyLink> {
        self.links.get(id)
    }

    /// Mutable reference to the multibody link with the given id.
    ///
    /// Return `None` if the given id does not identifies a multibody link part of `self`.
    #[inline]
    pub fn link_mut(&mut self, id: usize) -> Option<&mut MultibodyLink> {
        self.links.get_mut(id)
    }

    /// The number of links on this multibody.
    pub fn num_links(&self) -> usize {
        self.links.len()
    }

    /// Iterator through all the links of this multibody.
    ///
    /// All link are guaranteed to be yielded before its descendant.
    pub fn links(&self) -> impl Iterator<Item = &MultibodyLink> {
        self.links.iter()
    }

    /// Mutable iterator through all the links of this multibody.
    ///
    /// All link are guaranteed to be yielded before its descendant.
    pub fn links_mut(&mut self) -> impl Iterator<Item = &mut MultibodyLink> {
        self.links.iter_mut()
    }

    /// The vector of damping applied to this multibody.
    #[inline]
    pub fn damping(&self) -> &DVector {
        &self.damping
    }

    /// Mutable vector of damping applied to this multibody.
    #[inline]
    pub fn damping_mut(&mut self) -> &mut DVector {
        &mut self.damping
    }

    pub(crate) fn add_link(
        &mut self,
        parent: Option<usize>, // TODO: should be a RigidBodyHandle?
        dof: MultibodyJoint,
        body: RigidBodyHandle,
    ) -> &mut MultibodyLink {
        assert!(
            parent.is_none() || !self.links.is_empty(),
            "Multibody::build_body: invalid parent id."
        );

        /*
         * Compute the indices.
         */
        let assembly_id = self.velocities.len();
        let internal_id = self.links.len();

        /*
         * Grow the buffers.
         */
        let ndofs = dof.ndofs();
        self.grow_buffers(ndofs, 1);
        self.ndofs += ndofs;

        /*
         * Setup default damping.
         */
        dof.default_damping(&mut self.damping.rows_mut(assembly_id, ndofs));

        /*
         * Create the multibody.
         */
        let local_to_parent = dof.body_to_parent();
        let local_to_world;

        let parent_internal_id;
        if let Some(parent) = parent {
            parent_internal_id = parent;
            let parent_link = &mut self.links[parent_internal_id];
            local_to_world = parent_link.local_to_world * local_to_parent;
        } else {
            parent_internal_id = 0;
            local_to_world = local_to_parent;
        }

        let rb = MultibodyLink::new(
            body,
            internal_id,
            assembly_id,
            parent_internal_id,
            dof,
            local_to_world,
            local_to_parent,
        );

        self.links.push(rb);
        self.workspace.resize(self.links.len(), self.ndofs);

        &mut self.links[internal_id]
    }

    fn grow_buffers(&mut self, ndofs: usize, num_jacobians: usize) {
        let len = self.velocities.len();
        self.velocities.resize_vertically_mut(len + ndofs, 0.0);
        self.damping.resize_vertically_mut(len + ndofs, 0.0);
        self.accelerations.resize_vertically_mut(len + ndofs, 0.0);
        self.body_jacobians
            .extend((0..num_jacobians).map(|_| Jacobian::zeros(0)));
    }

    pub(crate) fn update_acceleration(&mut self, bodies: &RigidBodySet) {
        if self.ndofs == 0 {
            return; // Nothing to do.
        }

        self.accelerations.fill(0.0);

        // Eqn 42 to 45
        for i in 0..self.links.len() {
            let link = &self.links[i];
            let rb = &bodies[link.rigid_body];

            let mut acc = RigidBodyVelocity::zero();

            if i != 0 {
                let parent_id = link.parent_internal_id;
                let parent_link = &self.links[parent_id];
                let parent_rb = &bodies[parent_link.rigid_body];

                acc += self.workspace.accs[parent_id];
                // The 2.0 originates from the two identical terms of Jdot (the terms become
                // identical once they are multiplied by the generalized velocities).
                acc.linvel += 2.0 * parent_rb.vels.angvel.gcross(link.joint_velocity.linvel);
                #[cfg(feature = "dim3")]
                {
                    acc.angvel += parent_rb.vels.angvel.cross(link.joint_velocity.angvel);
                }

                acc.linvel += parent_rb
                    .vels
                    .angvel
                    .gcross(parent_rb.vels.angvel.gcross(link.shift02));
                acc.linvel += self.workspace.accs[parent_id].angvel.gcross(link.shift02);
            }

            acc.linvel += rb.vels.angvel.gcross(rb.vels.angvel.gcross(link.shift23));
            acc.linvel += acc.angvel.gcross(link.shift23);

            self.workspace.accs[i] = acc;

            // TODO: should gyroscopic forces already be computed by the rigid-body itself
            //       (at the same time that we add the gravity force)?
            let gyroscopic;
            let rb_inertia = rb.mprops.effective_angular_inertia();
            let rb_mass = rb.mprops.effective_mass();

            #[cfg(feature = "dim3")]
            {
                let angvel = rb.vels.angvel;
                let inertia_times_angvel = rb_inertia * angvel;
                gyroscopic = angvel.cross(inertia_times_angvel);
            }
            #[cfg(feature = "dim2")]
            {
                gyroscopic = 0.0;
            }

            let external_forces = Force::new(
                rb.forces.force - rb_mass * acc.linvel,
                rb.forces.torque - gyroscopic - rb_inertia * acc.angvel,
            );
            self.accelerations.gemv_tr(
                1.0,
                &self.body_jacobians[i],
                external_forces.as_vector(),
                1.0,
            );
        }

        self.accelerations
            .cmpy(-1.0, &self.damping, &self.velocities, 1.0);

        self.augmented_mass_indices
            .with_rearranged_rows_mut(&mut self.accelerations, |accs| {
                self.acc_inv_augmented_mass.solve_mut(accs);
            });
    }

    /// Computes the constant terms of the dynamics.
    pub(crate) fn update_dynamics(&mut self, dt: Real, bodies: &mut RigidBodySet) {
        /*
         * Compute velocities.
         * NOTE: this is needed for kinematic bodies too.
         */
        let link = &mut self.links[0];
        let joint_velocity = link
            .joint
            .jacobian_mul_coordinates(&self.velocities.as_slice()[link.assembly_id..]);

        link.joint_velocity = joint_velocity;
        bodies.index_mut_internal(link.rigid_body).vels = link.joint_velocity;

        for i in 1..self.links.len() {
            let (link, parent_link) = self.links.get_mut_with_parent(i);
            let rb = &bodies[link.rigid_body];
            let parent_rb = &bodies[parent_link.rigid_body];

            let joint_velocity = link
                .joint
                .jacobian_mul_coordinates(&self.velocities.as_slice()[link.assembly_id..]);
            link.joint_velocity = joint_velocity.transformed(
                &(parent_link.local_to_world.rotation * link.joint.data.local_frame1.rotation),
            );
            let mut new_rb_vels = parent_rb.vels + link.joint_velocity;
            let shift = rb.mprops.world_com - parent_rb.mprops.world_com;
            new_rb_vels.linvel += parent_rb.vels.angvel.gcross(shift);
            new_rb_vels.linvel += link.joint_velocity.angvel.gcross(link.shift23);

            bodies.index_mut_internal(link.rigid_body).vels = new_rb_vels;
        }

        /*
         * Update augmented mass matrix.
         */
        self.update_inertias(dt, bodies);
    }

    fn update_body_jacobians(&mut self) {
        for i in 0..self.links.len() {
            let link = &self.links[i];

            if self.body_jacobians[i].ncols() != self.ndofs {
                // TODO: use a resize instead.
                self.body_jacobians[i] = Jacobian::zeros(self.ndofs);
            }

            let parent_to_world;

            if i != 0 {
                let parent_id = link.parent_internal_id;
                let parent_link = &self.links[parent_id];
                parent_to_world = parent_link.local_to_world;

                let (link_j, parent_j) = self.body_jacobians.index_mut_const(i, parent_id);
                link_j.copy_from(parent_j);

                {
                    let mut link_j_v = link_j.fixed_rows_mut::<DIM>(0);
                    let parent_j_w = parent_j.fixed_rows::<ANG_DIM>(DIM);

                    let shift_tr = vect_to_na(link.shift02).gcross_matrix_tr();
                    link_j_v.gemm(1.0, &shift_tr, &parent_j_w, 1.0);
                }
            } else {
                self.body_jacobians[i].fill(0.0);
                parent_to_world = Pose::IDENTITY;
            }

            let ndofs = link.joint.ndofs();
            let mut tmp = SMatrix::<Real, SPATIAL_DIM, SPATIAL_DIM>::zeros();
            let mut link_joint_j = tmp.columns_mut(0, ndofs);
            let mut link_j_part = self.body_jacobians[i].columns_mut(link.assembly_id, ndofs);
            link.joint.jacobian(
                &(parent_to_world.rotation * link.joint.data.local_frame1.rotation),
                &mut link_joint_j,
            );
            link_j_part += link_joint_j;

            {
                let link_j = &mut self.body_jacobians[i];
                let (mut link_j_v, link_j_w) =
                    link_j.rows_range_pair_mut(0..DIM, DIM..DIM + ANG_DIM);
                let shift_tr = vect_to_na(link.shift23).gcross_matrix_tr();
                link_j_v.gemm(1.0, &shift_tr, &link_j_w, 1.0);
            }
        }
    }

    fn update_inertias(&mut self, dt: Real, bodies: &RigidBodySet) {
        if self.ndofs == 0 {
            return; // Nothing to do.
        }

        if self.augmented_mass.ncols() != self.ndofs {
            // TODO: do a resize instead of a full reallocation.
            self.augmented_mass = DMatrix::zeros(self.ndofs, self.ndofs);
            self.acc_augmented_mass = DMatrix::zeros(self.ndofs, self.ndofs);
        } else {
            self.augmented_mass.fill(0.0);
            self.acc_augmented_mass.fill(0.0);
        }

        self.augmented_mass_indices.clear();

        if self.coriolis_v.len() != self.links.len() {
            self.coriolis_v.resize(
                self.links.len(),
                OMatrix::<Real, Dim, Dyn>::zeros(self.ndofs),
            );
            self.coriolis_w.resize(
                self.links.len(),
                OMatrix::<Real, AngDim, Dyn>::zeros(self.ndofs),
            );
            self.i_coriolis_dt = Jacobian::zeros(self.ndofs);
        }

        let mut curr_assembly_id = 0;

        for i in 0..self.links.len() {
            let link = &self.links[i];
            let rb = &bodies[link.rigid_body];
            let rb_mass = rb.mprops.effective_mass();
            let rb_inertia = rb.mprops.effective_angular_inertia().into_matrix();
            let body_jacobian = &self.body_jacobians[i];

            // NOTE: the mass matrix index reordering operates on the assumption that the assembly
            //       ids are traversed in order. This assert is here to ensure the assumption always
            //       hold.
            assert_eq!(
                curr_assembly_id, link.assembly_id,
                "Internal error: contiguity assumption on assembly_id does not hold."
            );
            curr_assembly_id += link.joint.ndofs();

            if link.joint.kinematic {
                for k in link.assembly_id..link.assembly_id + link.joint.ndofs() {
                    self.augmented_mass_indices.remove(k);
                }
            } else {
                for k in link.assembly_id..link.assembly_id + link.joint.ndofs() {
                    self.augmented_mass_indices.keep(k);
                }
            }

            #[allow(unused_mut)] // mut is needed for 3D but not for 2D.
            let mut augmented_inertia = rb_inertia;

            #[cfg(feature = "dim3")]
            {
                // Derivative of gyroscopic forces.
                let gyroscopic_matrix = rb.vels.angvel.gcross_matrix() * rb_inertia
                    - (rb_inertia * rb.vels.angvel).gcross_matrix();

                augmented_inertia += gyroscopic_matrix * dt;
            }

            // TODO: optimize that (knowing the structure of the augmented inertia matrix).
            // TODO: this could be better optimized in 2D.
            #[allow(clippy::useless_conversion)] // Needed in 3D, no-op in 2D
            let rb_mass_matrix_wo_gyro = concat_rb_mass_matrix(rb_mass, mat_to_na(rb_inertia));
            #[allow(clippy::useless_conversion)] // Needed in 3D, no-op in 2D
            let rb_mass_matrix = concat_rb_mass_matrix(rb_mass, mat_to_na(augmented_inertia));
            self.augmented_mass
                .quadform(1.0, &rb_mass_matrix_wo_gyro, body_jacobian, 1.0);
            self.acc_augmented_mass
                .quadform(1.0, &rb_mass_matrix, body_jacobian, 1.0);

            /*
             *
             * Coriolis matrix.
             *
             */
            let rb_j = &self.body_jacobians[i];
            let rb_j_w = rb_j.fixed_rows::<ANG_DIM>(DIM);

            let ndofs = link.joint.ndofs();

            if i != 0 {
                let parent_id = link.parent_internal_id;
                let parent_link = &self.links[parent_id];
                let parent_rb = &bodies[parent_link.rigid_body];
                let parent_j = &self.body_jacobians[parent_id];
                let parent_j_w = parent_j.fixed_rows::<ANG_DIM>(DIM);
                let parent_w = vect_to_na(parent_rb.vels.angvel).gcross_matrix();

                let (coriolis_v, parent_coriolis_v) = self.coriolis_v.index_mut2(i, parent_id);
                let (coriolis_w, parent_coriolis_w) = self.coriolis_w.index_mut2(i, parent_id);

                coriolis_v.copy_from(parent_coriolis_v);
                coriolis_w.copy_from(parent_coriolis_w);

                // [c1 - c0].gcross() * (JDot + JDot/u * qdot)"
                let shift_cross_tr = vect_to_na(link.shift02).gcross_matrix_tr();
                coriolis_v.gemm(1.0, &shift_cross_tr, parent_coriolis_w, 1.0);

                // JDot (but the 2.0 originates from the sum of two identical terms in JDot and JDot/u * gdot)
                let dvel_cross = vect_to_na(
                    rb.vels.angvel.gcross(link.shift02) + 2.0 * link.joint_velocity.linvel,
                )
                .gcross_matrix_tr();
                coriolis_v.gemm(1.0, &dvel_cross, &parent_j_w, 1.0);

                // JDot/u * qdot
                coriolis_v.gemm(
                    1.0,
                    &vect_to_na(link.joint_velocity.linvel).gcross_matrix_tr(),
                    &parent_j_w,
                    1.0,
                );
                coriolis_v.gemm(1.0, &(parent_w * shift_cross_tr), &parent_j_w, 1.0);

                #[cfg(feature = "dim3")]
                {
                    let vel_wrt_joint_w = vect_to_na(link.joint_velocity.angvel).gcross_matrix();
                    coriolis_w.gemm(-1.0, &vel_wrt_joint_w, &parent_j_w, 1.0);
                }

                // JDot (but the 2.0 originates from the sum of two identical terms in JDot and JDot/u * gdot)
                if !link.joint.kinematic {
                    let mut coriolis_v_part = coriolis_v.columns_mut(link.assembly_id, ndofs);

                    let mut tmp1 = SMatrix::<Real, SPATIAL_DIM, SPATIAL_DIM>::zeros();
                    let mut rb_joint_j = tmp1.columns_mut(0, ndofs);
                    link.joint.jacobian(
                        &(parent_link.local_to_world.rotation
                            * link.joint.data.local_frame1.rotation),
                        &mut rb_joint_j,
                    );

                    let rb_joint_j_v = rb_joint_j.fixed_rows::<DIM>(0);
                    coriolis_v_part.gemm(2.0, &parent_w, &rb_joint_j_v, 1.0);

                    #[cfg(feature = "dim3")]
                    {
                        let rb_joint_j_w = rb_joint_j.fixed_rows::<ANG_DIM>(DIM);
                        let mut coriolis_w_part = coriolis_w.columns_mut(link.assembly_id, ndofs);
                        coriolis_w_part.gemm(1.0, &parent_w, &rb_joint_j_w, 1.0);
                    }
                }
            } else {
                self.coriolis_v[i].fill(0.0);
                self.coriolis_w[i].fill(0.0);
            }

            let coriolis_v = &mut self.coriolis_v[i];
            let coriolis_w = &mut self.coriolis_w[i];

            {
                // [c3 - c2].gcross() * (JDot + JDot/u * qdot)
                let shift_cross_tr = vect_to_na(link.shift23).gcross_matrix_tr();
                coriolis_v.gemm(1.0, &shift_cross_tr, coriolis_w, 1.0);

                // JDot
                let dvel_cross = vect_to_na(rb.vels.angvel.gcross(link.shift23)).gcross_matrix_tr();
                coriolis_v.gemm(1.0, &dvel_cross, &rb_j_w, 1.0);

                // JDot/u * qdot
                coriolis_v.gemm(
                    1.0,
                    &(vect_to_na(rb.vels.angvel).gcross_matrix() * shift_cross_tr),
                    &rb_j_w,
                    1.0,
                );
            }

            let coriolis_v = &mut self.coriolis_v[i];
            let coriolis_w = &mut self.coriolis_w[i];

            /*
             * Meld with the mass matrix.
             */
            {
                let mut i_coriolis_dt_v = self.i_coriolis_dt.fixed_rows_mut::<DIM>(0);
                i_coriolis_dt_v.copy_from(coriolis_v);
                let rb_mass_dt = vect_to_na(rb_mass * dt);
                i_coriolis_dt_v
                    .column_iter_mut()
                    .for_each(|mut c| c.component_mul_assign(&rb_mass_dt));
            }

            #[cfg(feature = "dim2")]
            {
                let mut i_coriolis_dt_w = self.i_coriolis_dt.fixed_rows_mut::<ANG_DIM>(DIM);
                // NOTE: this is just an axpy, but on row columns.
                i_coriolis_dt_w.zip_apply(coriolis_w, |o, x| *o = x * dt * rb_inertia);
            }
            #[cfg(feature = "dim3")]
            {
                let mut i_coriolis_dt_w = self.i_coriolis_dt.fixed_rows_mut::<ANG_DIM>(DIM);
                i_coriolis_dt_w.gemm(dt, &mat_to_na(rb_inertia), coriolis_w, 0.0);
            }

            self.acc_augmented_mass
                .gemm_tr(1.0, rb_j, &self.i_coriolis_dt, 1.0);
        }

        /*
         * Damping.
         */
        for i in 0..self.ndofs {
            self.acc_augmented_mass[(i, i)] += self.damping[i] * dt;
            self.augmented_mass[(i, i)] += self.damping[i] * dt;
        }

        let effective_dim = self
            .augmented_mass_indices
            .dim_after_removal(self.acc_augmented_mass.nrows());

        // PERF: since we clone the matrix anyway for LU, should be directly output
        //       a new matrix instead of applying permutations?
        self.augmented_mass_indices
            .rearrange_columns(&mut self.acc_augmented_mass, true);
        self.augmented_mass_indices
            .rearrange_columns(&mut self.augmented_mass, true);

        self.augmented_mass_indices
            .rearrange_rows(&mut self.acc_augmented_mass, true);
        self.augmented_mass_indices
            .rearrange_rows(&mut self.augmented_mass, true);

        // TODO: avoid allocation inside LU at each timestep.
        self.acc_inv_augmented_mass = LU::new(
            self.acc_augmented_mass
                .view((0, 0), (effective_dim, effective_dim))
                .into_owned(),
        );
        self.inv_augmented_mass = LU::new(
            self.augmented_mass
                .view((0, 0), (effective_dim, effective_dim))
                .into_owned(),
        );
    }

    /// The generalized velocity at the multibody_joint of the given link.
    #[inline]
    pub fn joint_velocity(&self, link: &MultibodyLink) -> DVectorView<'_, Real> {
        let ndofs = link.joint().ndofs();
        DVectorView::from_slice(
            &self.velocities.as_slice()[link.assembly_id..link.assembly_id + ndofs],
            ndofs,
        )
    }

    /// The generalized accelerations of this multibodies.
    #[inline]
    pub fn generalized_acceleration(&self) -> DVectorView<'_, Real> {
        self.accelerations.rows(0, self.ndofs)
    }

    /// The generalized velocities of this multibodies.
    #[inline]
    pub fn generalized_velocity(&self) -> DVectorView<'_, Real> {
        self.velocities.rows(0, self.ndofs)
    }

    /// The body jacobian for link `link_id` calculated by the last call to [`Multibody::forward_kinematics`].
    #[inline]
    pub fn body_jacobian(&self, link_id: usize) -> &Jacobian<Real> {
        &self.body_jacobians[link_id]
    }

    /// The mutable generalized velocities of this multibodies.
    #[inline]
    pub fn generalized_velocity_mut(&mut self) -> DVectorViewMut<'_, Real> {
        self.velocities.rows_mut(0, self.ndofs)
    }

    #[inline]
    pub(crate) fn integrate(&mut self, dt: Real) {
        for rb in self.links.iter_mut() {
            rb.joint
                .integrate(dt, &self.velocities.as_slice()[rb.assembly_id..])
        }
    }

    /// Apply displacements, in generalized coordinates, to this multibody.
    ///
    /// Note this does **not** updates the link poses, only their generalized coordinates.
    /// To update the link poses and associated rigid-bodies, call [`Self::forward_kinematics`].
    pub fn apply_displacements(&mut self, disp: &[Real]) {
        for link in self.links.iter_mut() {
            link.joint.apply_displacement(&disp[link.assembly_id..])
        }
    }

    pub(crate) fn update_root_type(&mut self, bodies: &RigidBodySet, take_body_pose: bool) {
        if let Some(rb) = bodies.get(self.links[0].rigid_body) {
            if rb.is_dynamic() != self.root_is_dynamic {
                let root_pose = if take_body_pose {
                    *rb.position()
                } else {
                    self.links[0].local_to_world
                };

                if rb.is_dynamic() {
                    let free_joint = MultibodyJoint::free(root_pose);
                    let prev_root_ndofs = self.links[0].joint().ndofs();
                    self.links[0].joint = free_joint;
                    self.links[0].assembly_id = 0;
                    self.ndofs += SPATIAL_DIM;

                    self.velocities = self.velocities.clone().insert_rows(0, SPATIAL_DIM, 0.0);
                    self.damping = self.damping.clone().insert_rows(0, SPATIAL_DIM, 0.0);
                    self.accelerations =
                        self.accelerations.clone().insert_rows(0, SPATIAL_DIM, 0.0);

                    for link in &mut self.links[1..] {
                        link.assembly_id += SPATIAL_DIM - prev_root_ndofs;
                    }
                } else {
                    assert!(self.velocities.len() >= SPATIAL_DIM);
                    assert!(self.damping.len() >= SPATIAL_DIM);
                    assert!(self.accelerations.len() >= SPATIAL_DIM);

                    let fixed_joint = MultibodyJoint::fixed(root_pose);
                    let prev_root_ndofs = self.links[0].joint().ndofs();
                    self.links[0].joint = fixed_joint;
                    self.links[0].assembly_id = 0;
                    self.ndofs -= prev_root_ndofs;

                    if self.ndofs == 0 {
                        self.velocities = DVector::zeros(0);
                        self.damping = DVector::zeros(0);
                        self.accelerations = DVector::zeros(0);
                    } else {
                        self.velocities =
                            self.velocities.index((prev_root_ndofs.., 0)).into_owned();
                        self.damping = self.damping.index((prev_root_ndofs.., 0)).into_owned();
                        self.accelerations = self
                            .accelerations
                            .index((prev_root_ndofs.., 0))
                            .into_owned();
                    }

                    for link in &mut self.links[1..] {
                        link.assembly_id -= prev_root_ndofs;
                    }
                }

                self.root_is_dynamic = rb.is_dynamic();
            }

            // Make sure the positions are properly set to match the rigid-body’s.
            if take_body_pose {
                if self.links[0].joint.data.locked_axes.is_empty() {
                    self.links[0].joint.set_free_pos(*rb.position());
                } else {
                    self.links[0].joint.data.local_frame1 = *rb.position();
                }
            }
        }
    }

    /// Update the rigid-body poses based on this multibody joint poses.
    ///
    /// This is typically called after [`Self::forward_kinematics`] to apply the new joint poses
    /// to the rigid-bodies.
    pub fn update_rigid_bodies(&self, bodies: &mut RigidBodySet, update_mass_properties: bool) {
        self.update_rigid_bodies_internal(bodies, update_mass_properties, false, true)
    }

    pub(crate) fn update_rigid_bodies_internal(
        &self,
        bodies: &mut RigidBodySet,
        update_mass_properties: bool,
        update_next_positions_only: bool,
        change_tracking: bool,
    ) {
        // Handle the children. They all have a parent within this multibody.
        for link in self.links.iter() {
            let rb = if change_tracking {
                bodies.get_mut_internal_with_modification_tracking(link.rigid_body)
            } else {
                bodies.get_mut_internal(link.rigid_body)
            };

            if let Some(rb) = rb {
                rb.pos.next_position = link.local_to_world;

                if !update_next_positions_only {
                    rb.pos.position = link.local_to_world;
                }

                if update_mass_properties {
                    rb.mprops
                        .update_world_mass_properties(rb.body_type, &link.local_to_world);
                }
            }
        }
    }

    // TODO: make a version that doesn’t write back to bodies and doesn’t update the jacobians
    //       (i.e. just something used by the velocity solver’s small steps).
    /// Apply forward-kinematics to this multibody.
    ///
    /// This will update the [`MultibodyLink`] pose information as wall as the body jacobians.
    /// This will also ensure that the multibody has the proper number of degrees of freedom if
    /// its root node changed between dynamic and non-dynamic.
    ///
    /// Note that this does **not** update the poses of the [`RigidBody`] attached to the joints.
    /// Run [`Self::update_rigid_bodies`] to trigger that update.
    ///
    /// This method updates `self` with the result of the forward-kinematics operation.
    /// For a non-mutable version running forward kinematics on a single link, see
    /// [`Self::forward_kinematics_single_link`].
    ///
    /// ## Parameters
    /// - `bodies`: the set of rigid-bodies.
    /// - `read_root_pose_from_rigid_body`: if set to `true`, the root joint (either a fixed joint,
    ///   or a free joint) will have its pose set to its associated-rigid-body pose. Set this to `true`
    ///   when the root rigid-body pose has been modified and needs to affect the multibody.
    pub fn forward_kinematics(
        &mut self,
        bodies: &RigidBodySet,
        read_root_pose_from_rigid_body: bool,
    ) {
        // Be sure the degrees of freedom match and take the root position if needed.
        self.update_root_type(bodies, read_root_pose_from_rigid_body);

        // Special case for the root, which has no parent.
        {
            let link = &mut self.links[0];
            link.local_to_parent = link.joint.body_to_parent();
            link.local_to_world = link.local_to_parent;
        }

        // Handle the children. They all have a parent within this multibody.
        for i in 1..self.links.len() {
            let (link, parent_link) = self.links.get_mut_with_parent(i);

            link.local_to_parent = link.joint.body_to_parent();
            link.local_to_world = parent_link.local_to_world * link.local_to_parent;

            {
                let parent_rb = &bodies[parent_link.rigid_body];
                let link_rb = &bodies[link.rigid_body];
                let c0 = parent_link.local_to_world * parent_rb.mprops.local_mprops.local_com;
                let c2 =
                    link.local_to_world * Vector::from(link.joint.data.local_frame2.translation);
                let c3 = link.local_to_world * link_rb.mprops.local_mprops.local_com;

                link.shift02 = c2 - c0;
                link.shift23 = c3 - c2;
            }

            assert_eq!(
                bodies[link.rigid_body].body_type,
                RigidBodyType::Dynamic,
                "A rigid-body that is not at the root of a multibody must be dynamic."
            );
        }

        /*
         * Compute body jacobians.
         */
        self.update_body_jacobians();
    }

    /// Computes the ids of all the links between the root and the link identified by `link_id`.
    pub fn kinematic_branch(&self, link_id: usize) -> Vec<usize> {
        let mut branch = vec![]; // Perf: avoid allocation.
        let mut curr_id = Some(link_id);

        while let Some(id) = curr_id {
            branch.push(id);
            curr_id = self.links[id].parent_id();
        }

        branch.reverse();
        branch
    }

    /// Apply forward-kinematics to compute the position of a single link of this multibody.
    ///
    /// If `out_jacobian` is `Some`, this will simultaneously compute the new jacobian of this link.
    /// If `displacement` is `Some`, the generalized position considered during transform propagation
    /// is the sum of the current position of `self` and this `displacement`.
    // TODO: this shares a lot of code with `forward_kinematics` and `update_body_jacobians`, except
    //       that we are only traversing a single kinematic chain. Could this be refactored?
    pub fn forward_kinematics_single_link(
        &self,
        bodies: &RigidBodySet,
        link_id: usize,
        displacement: Option<&[Real]>,
        out_jacobian: Option<&mut Jacobian<Real>>,
    ) -> Pose {
        let branch = self.kinematic_branch(link_id);
        self.forward_kinematics_single_branch(bodies, &branch, displacement, out_jacobian)
    }

    /// Apply forward-kinematics to compute the position of a single sorted branch of this multibody.
    ///
    /// The given `branch` must have the following properties:
    /// - It must be sorted, i.e., `branch[i] < branch[i + 1]`.
    /// - All the indices must be part of the same kinematic branch.
    /// - If a link is `branch[i]`, then `branch[i - 1]` must be its parent.
    ///
    /// In general, this method shouldn’t be used directly and [`Self::forward_kinematics_single_link`]
    /// should be preferred since it computes the branch indices automatically.
    ///
    /// If you want to calculate the branch indices manually, see [`Self::kinematic_branch`].
    ///
    /// If `out_jacobian` is `Some`, this will simultaneously compute the new jacobian of this branch.
    /// This represents the body jacobian for the last link in the branch.
    ///
    /// If `displacement` is `Some`, the generalized position considered during transform propagation
    /// is the sum of the current position of `self` and this `displacement`.
    // TODO: this shares a lot of code with `forward_kinematics` and `update_body_jacobians`, except
    //       that we are only traversing a single kinematic chain. Could this be refactored?
    pub fn forward_kinematics_single_branch(
        &self,
        bodies: &RigidBodySet,
        branch: &[usize],
        displacement: Option<&[Real]>,
        mut out_jacobian: Option<&mut Jacobian<Real>>,
    ) -> Pose {
        if let Some(out_jacobian) = out_jacobian.as_deref_mut() {
            if out_jacobian.ncols() != self.ndofs {
                *out_jacobian = Jacobian::zeros(self.ndofs);
            } else {
                out_jacobian.fill(0.0);
            }
        }

        let mut parent_link: Option<MultibodyLink> = None;

        for i in branch {
            let mut link = self.links[*i];

            if let Some(displacement) = displacement {
                link.joint
                    .apply_displacement(&displacement[link.assembly_id..]);
            }

            let parent_to_world;

            if let Some(parent_link) = parent_link {
                link.local_to_parent = link.joint.body_to_parent();
                link.local_to_world = parent_link.local_to_world * link.local_to_parent;

                {
                    let parent_rb = &bodies[parent_link.rigid_body];
                    let link_rb = &bodies[link.rigid_body];
                    let c0 = parent_link.local_to_world * parent_rb.mprops.local_mprops.local_com;
                    let c2 = link.local_to_world
                        * Vector::from(link.joint.data.local_frame2.translation);
                    let c3 = link.local_to_world * link_rb.mprops.local_mprops.local_com;

                    link.shift02 = c2 - c0;
                    link.shift23 = c3 - c2;
                }

                parent_to_world = parent_link.local_to_world;

                if let Some(out_jacobian) = out_jacobian.as_deref_mut() {
                    let (mut link_j_v, parent_j_w) =
                        out_jacobian.rows_range_pair_mut(0..DIM, DIM..DIM + ANG_DIM);
                    let shift_tr = vect_to_na(link.shift02).gcross_matrix_tr();
                    link_j_v.gemm(1.0, &shift_tr, &parent_j_w, 1.0);
                }
            } else {
                link.local_to_parent = link.joint.body_to_parent();
                link.local_to_world = link.local_to_parent;
                parent_to_world = Pose::IDENTITY;
            }

            if let Some(out_jacobian) = out_jacobian.as_deref_mut() {
                let ndofs = link.joint.ndofs();
                let mut tmp = SMatrix::<Real, SPATIAL_DIM, SPATIAL_DIM>::zeros();
                let mut link_joint_j = tmp.columns_mut(0, ndofs);
                let mut link_j_part = out_jacobian.columns_mut(link.assembly_id, ndofs);
                link.joint.jacobian(
                    &(parent_to_world.rotation * link.joint.data.local_frame1.rotation),
                    &mut link_joint_j,
                );
                link_j_part += link_joint_j;

                {
                    let (mut link_j_v, link_j_w) =
                        out_jacobian.rows_range_pair_mut(0..DIM, DIM..DIM + ANG_DIM);
                    let shift_tr = vect_to_na(link.shift23).gcross_matrix_tr();
                    link_j_v.gemm(1.0, &shift_tr, &link_j_w, 1.0);
                }
            }

            parent_link = Some(link);
        }

        parent_link
            .map(|link| link.local_to_world)
            .unwrap_or(Pose::IDENTITY)
    }

    /// The total number of freedoms of this multibody.
    #[inline]
    pub fn ndofs(&self) -> usize {
        self.ndofs
    }

    pub(crate) fn fill_jacobians(
        &self,
        link_id: usize,
        unit_force: Vector,
        unit_torque: AngVector,
        j_id: &mut usize,
        jacobians: &mut DVector,
    ) -> (Real, Real) {
        if self.ndofs == 0 {
            return (0.0, 0.0);
        }

        let wj_id = *j_id + self.ndofs;
        let force = Force {
            linear: unit_force,
            #[cfg(feature = "dim2")]
            angular: unit_torque,
            #[cfg(feature = "dim3")]
            angular: unit_torque,
        };

        let link = &self.links[link_id];
        let mut out_j = jacobians.rows_mut(*j_id, self.ndofs);
        self.body_jacobians[link.internal_id].tr_mul_to(force.as_vector(), &mut out_j);

        // TODO: Optimize with a copy_nonoverlapping?
        for i in 0..self.ndofs {
            jacobians[wj_id + i] = jacobians[*j_id + i];
        }

        {
            let mut out_invm_j = jacobians.rows_mut(wj_id, self.ndofs);
            self.augmented_mass_indices
                .with_rearranged_rows_mut(&mut out_invm_j, |out_invm_j| {
                    self.inv_augmented_mass.solve_mut(out_invm_j);
                });
        }

        let j = jacobians.rows(*j_id, self.ndofs);
        let invm_j = jacobians.rows(wj_id, self.ndofs);
        *j_id += self.ndofs * 2;

        (j.dot(&invm_j), j.dot(&self.generalized_velocity()))
    }

    // #[cfg(feature = "parallel")]
    // #[inline]
    // pub(crate) fn has_active_internal_constraints(&self) -> bool {
    //     self.links()
    //         .any(|link| link.joint().num_velocity_constraints() != 0)
    // }

    #[cfg(feature = "parallel")]
    #[inline]
    #[allow(dead_code)] // That will likely be useful when we re-introduce intra-island parallelism.
    pub(crate) fn num_active_internal_constraints_and_jacobian_lines(&self) -> (usize, usize) {
        let num_constraints: usize = self
            .links
            .iter()
            .map(|l| l.joint().num_velocity_constraints())
            .sum();
        (num_constraints, num_constraints)
    }
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Debug)]
struct IndexSequence {
    first_to_remove: usize,
    index_map: Vec<usize>,
}

impl IndexSequence {
    fn new() -> Self {
        Self {
            first_to_remove: usize::MAX,
            index_map: vec![],
        }
    }

    fn clear(&mut self) {
        self.first_to_remove = usize::MAX;
        self.index_map.clear();
    }

    fn keep(&mut self, i: usize) {
        if self.first_to_remove == usize::MAX {
            // Nothing got removed yet. No need to register any
            // special indexing.
            return;
        }

        self.index_map.push(i);
    }

    fn remove(&mut self, i: usize) {
        if self.first_to_remove == usize::MAX {
            self.first_to_remove = i;
        }
    }

    fn dim_after_removal(&self, original_dim: usize) -> usize {
        if self.first_to_remove == usize::MAX {
            original_dim
        } else {
            self.first_to_remove + self.index_map.len()
        }
    }

    fn rearrange_columns<R: na::Dim, C: na::Dim, S: StorageMut<Real, R, C>>(
        &self,
        mat: &mut na::Matrix<Real, R, C, S>,
        clear_removed: bool,
    ) {
        if self.first_to_remove == usize::MAX {
            // Nothing to rearrange.
            return;
        }

        for (target_shift, source) in self.index_map.iter().enumerate() {
            let target = self.first_to_remove + target_shift;
            let (mut target_col, source_col) = mat.columns_range_pair_mut(target, *source);
            target_col.copy_from(&source_col);
        }

        if clear_removed {
            mat.columns_range_mut(self.first_to_remove + self.index_map.len()..)
                .fill(0.0);
        }
    }

    fn rearrange_rows<R: na::Dim, C: na::Dim, S: StorageMut<Real, R, C>>(
        &self,
        mat: &mut na::Matrix<Real, R, C, S>,
        clear_removed: bool,
    ) {
        if self.first_to_remove == usize::MAX {
            // Nothing to rearrange.
            return;
        }

        for mut col in mat.column_iter_mut() {
            for (target_shift, source) in self.index_map.iter().enumerate() {
                let target = self.first_to_remove + target_shift;
                col[target] = col[*source];
            }

            if clear_removed {
                col.rows_range_mut(self.first_to_remove + self.index_map.len()..)
                    .fill(0.0);
            }
        }
    }

    fn inv_rearrange_rows<R: na::Dim, C: na::Dim, S: StorageMut<Real, R, C>>(
        &self,
        mat: &mut na::Matrix<Real, R, C, S>,
    ) {
        if self.first_to_remove == usize::MAX {
            // Nothing to rearrange.
            return;
        }

        for mut col in mat.column_iter_mut() {
            for (target_shift, source) in self.index_map.iter().enumerate().rev() {
                let target = self.first_to_remove + target_shift;
                col[*source] = col[target];
                col[target] = 0.0;
            }
        }
    }

    fn with_rearranged_rows_mut<C: na::Dim, S: StorageMut<Real, Dyn, C>>(
        &self,
        mat: &mut na::Matrix<Real, Dyn, C, S>,
        mut f: impl FnMut(&mut na::MatrixViewMut<Real, Dyn, C, S::RStride, S::CStride>),
    ) {
        self.rearrange_rows(mat, true);
        let effective_dim = self.dim_after_removal(mat.nrows());
        if effective_dim > 0 {
            f(&mut mat.rows_mut(0, effective_dim));
        }
        self.inv_rearrange_rows(mat);
    }
}

#[cfg(test)]
mod test {
    use super::IndexSequence;
    use crate::dynamics::{ImpulseJointSet, IslandManager};
    #[cfg(feature = "dim3")]
    use crate::math::Vector;
    use crate::math::{Real, SPATIAL_DIM};
    use crate::prelude::{
        ColliderSet, MultibodyJointHandle, MultibodyJointSet, RevoluteJoint, RigidBodyBuilder,
        RigidBodySet,
    };
    use na::{DVector, RowDVector};

    #[test]
    fn test_multibody_append() {
        let mut bodies = RigidBodySet::new();
        let mut joints = MultibodyJointSet::new();

        let a = bodies.insert(RigidBodyBuilder::dynamic());
        let b = bodies.insert(RigidBodyBuilder::dynamic());
        let c = bodies.insert(RigidBodyBuilder::dynamic());
        let d = bodies.insert(RigidBodyBuilder::dynamic());

        #[cfg(feature = "dim2")]
        let joint = RevoluteJoint::new();
        #[cfg(feature = "dim3")]
        let joint = RevoluteJoint::new(Vector::X);

        let mb_handle = joints.insert(a, b, joint, true).unwrap();
        joints.insert(c, d, joint, true).unwrap();
        joints.insert(b, c, joint, true).unwrap();

        assert_eq!(joints.get(mb_handle).unwrap().0.ndofs, SPATIAL_DIM + 3);
    }

    #[test]
    fn test_multibody_insert() {
        let mut rnd = oorandom::Rand32::new(1234);

        for k in 0..10 {
            let mut bodies = RigidBodySet::new();
            let mut multibody_joints = MultibodyJointSet::new();

            let num_links = 100;
            let mut handles = vec![];

            for _ in 0..num_links {
                handles.push(bodies.insert(RigidBodyBuilder::dynamic()));
            }

            let mut insertion_id: Vec<_> = (0..num_links - 1).collect();

            #[cfg(feature = "dim2")]
            let joint = RevoluteJoint::new();
            #[cfg(feature = "dim3")]
            let joint = RevoluteJoint::new(Vector::X);

            match k {
                0 => {} // Remove in insertion order.
                1 => {
                    // Remove from leaf to root.
                    insertion_id.reverse();
                }
                _ => {
                    // Shuffle the vector a bit.
                    // (This test checks multiple shuffle arrangements due to k > 2).
                    for l in 0..num_links - 1 {
                        insertion_id.swap(l, rnd.rand_range(0..num_links as u32 - 1) as usize);
                    }
                }
            }

            let mut mb_handle = MultibodyJointHandle::invalid();
            for i in insertion_id {
                mb_handle = multibody_joints
                    .insert(handles[i], handles[i + 1], joint, true)
                    .unwrap();
            }

            assert_eq!(
                multibody_joints.get(mb_handle).unwrap().0.ndofs,
                SPATIAL_DIM + num_links - 1
            );
        }
    }

    #[test]
    fn test_multibody_remove() {
        let mut rnd = oorandom::Rand32::new(1234);

        for k in 0..10 {
            let mut bodies = RigidBodySet::new();
            let mut multibody_joints = MultibodyJointSet::new();
            let mut colliders = ColliderSet::new();
            let mut impulse_joints = ImpulseJointSet::new();
            let mut islands = IslandManager::new();

            let num_links = 100;
            let mut handles = vec![];

            for _ in 0..num_links {
                handles.push(bodies.insert(RigidBodyBuilder::dynamic()));
            }

            #[cfg(feature = "dim2")]
            let joint = RevoluteJoint::new();
            #[cfg(feature = "dim3")]
            let joint = RevoluteJoint::new(Vector::X);

            for i in 0..num_links - 1 {
                multibody_joints
                    .insert(handles[i], handles[i + 1], joint, true)
                    .unwrap();
            }

            match k {
                0 => {} // Remove in insertion order.
                1 => {
                    // Remove from leaf to root.
                    handles.reverse();
                }
                _ => {
                    // Shuffle the vector a bit.
                    // (This test checks multiple shuffle arrangements due to k > 2).
                    for l in 0..num_links {
                        handles.swap(l, rnd.rand_range(0..num_links as u32) as usize);
                    }
                }
            }

            for handle in handles {
                bodies.remove(
                    handle,
                    &mut islands,
                    &mut colliders,
                    &mut impulse_joints,
                    &mut multibody_joints,
                    true,
                );
            }
        }
    }

    fn test_sequence() -> IndexSequence {
        let mut seq = IndexSequence::new();
        seq.remove(2);
        seq.remove(3);
        seq.remove(4);
        seq.keep(5);
        seq.keep(6);
        seq.remove(7);
        seq.keep(8);
        seq
    }

    #[test]
    fn index_sequence_rearrange_columns() {
        let seq = test_sequence();
        let mut vec = RowDVector::from_fn(10, |_, c| c as Real);
        seq.rearrange_columns(&mut vec, true);
        assert_eq!(
            vec,
            RowDVector::from(vec![0.0, 1.0, 5.0, 6.0, 8.0, 0.0, 0.0, 0.0, 0.0, 0.0])
        );
    }

    #[test]
    fn index_sequence_rearrange_rows() {
        let seq = test_sequence();
        let mut vec = DVector::from_fn(10, |r, _| r as Real);
        seq.rearrange_rows(&mut vec, true);
        assert_eq!(
            vec,
            DVector::from(vec![0.0, 1.0, 5.0, 6.0, 8.0, 0.0, 0.0, 0.0, 0.0, 0.0])
        );
        seq.inv_rearrange_rows(&mut vec);
        assert_eq!(
            vec,
            DVector::from(vec![0.0, 1.0, 0.0, 0.0, 0.0, 5.0, 6.0, 0.0, 8.0, 0.0])
        );
    }

    #[test]
    fn index_sequence_with_rearranged_rows_mut() {
        let seq = test_sequence();
        let mut vec = DVector::from_fn(10, |r, _| r as Real);
        seq.with_rearranged_rows_mut(&mut vec, |v| {
            assert_eq!(v.len(), 5);
            assert_eq!(*v, DVector::from(vec![0.0, 1.0, 5.0, 6.0, 8.0]));
            *v *= 10.0;
        });
        assert_eq!(
            vec,
            DVector::from(vec![0.0, 10.0, 0.0, 0.0, 0.0, 50.0, 60.0, 0.0, 80.0, 0.0])
        );
    }
}
