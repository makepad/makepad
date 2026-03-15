use crate::dynamics::{IslandManager, JointGraphEdge, JointIndex, RigidBodySet};
use crate::geometry::{ContactManifold, ContactManifoldIndex};

#[cfg(feature = "simd-is-enabled")]
use {
    crate::math::{SIMD_LAST_INDEX, SIMD_WIDTH},
    vec_map::VecMap,
};

#[cfg(feature = "parallel")]
use crate::dynamics::{MultibodyJointSet, RigidBodyHandle};

#[cfg(feature = "parallel")]
pub(crate) trait PairInteraction {
    fn body_pair(&self) -> (Option<RigidBodyHandle>, Option<RigidBodyHandle>);
}
#[cfg(feature = "simd-is-enabled")]
use crate::dynamics::RigidBodyType;

#[cfg(feature = "parallel")]
impl PairInteraction for &mut ContactManifold {
    fn body_pair(&self) -> (Option<RigidBodyHandle>, Option<RigidBodyHandle>) {
        (self.data.rigid_body1, self.data.rigid_body2)
    }
}

#[cfg(feature = "parallel")]
impl PairInteraction for JointGraphEdge {
    fn body_pair(&self) -> (Option<RigidBodyHandle>, Option<RigidBodyHandle>) {
        (Some(self.weight.body1), Some(self.weight.body2))
    }
}

#[cfg(feature = "parallel")]
#[allow(dead_code)] // That will likely be useful when we re-introduce intra-island parallelism.
pub(crate) struct ParallelInteractionGroups {
    bodies_color: Vec<u128>,         // Workspace.
    interaction_indices: Vec<usize>, // Workspace.
    interaction_colors: Vec<usize>,  // Workspace.
    sorted_interactions: Vec<usize>,
    groups: Vec<usize>,
}

#[cfg(feature = "parallel")]
#[allow(dead_code)] // That will likely be useful when we re-introduce intra-island parallelism.
impl ParallelInteractionGroups {
    pub fn new() -> Self {
        Self {
            bodies_color: Vec::new(),
            interaction_indices: Vec::new(),
            interaction_colors: Vec::new(),
            sorted_interactions: Vec::new(),
            groups: Vec::new(),
        }
    }

    pub fn group(&self, i: usize) -> &[usize] {
        let range = self.groups[i]..self.groups[i + 1];
        &self.sorted_interactions[range]
    }

    pub fn num_groups(&self) -> usize {
        self.groups.len().saturating_sub(1)
    }

    pub fn group_interactions<Interaction: PairInteraction>(
        &mut self,
        island_id: usize,
        islands: &IslandManager,
        bodies: &RigidBodySet,
        multibodies: &MultibodyJointSet,
        interactions: &[Interaction],
        interaction_indices: &[usize],
    ) {
        let num_island_bodies = islands.island(island_id).len();
        self.bodies_color.clear();
        self.interaction_indices.clear();
        self.groups.clear();
        self.sorted_interactions.clear();
        self.interaction_colors.clear();

        let mut color_len = [0; 128];
        self.bodies_color.resize(num_island_bodies, 0u128);
        self.interaction_indices
            .extend_from_slice(interaction_indices);
        self.interaction_colors.resize(interaction_indices.len(), 0);
        let bcolors = &mut self.bodies_color;

        for (interaction_id, color) in self
            .interaction_indices
            .iter()
            .zip(self.interaction_colors.iter_mut())
        {
            let mut body_pair = interactions[*interaction_id].body_pair();
            let is_fixed1 = body_pair.0.map(|b| bodies[b].is_fixed()).unwrap_or(true);
            let is_fixed2 = body_pair.1.map(|b| bodies[b].is_fixed()).unwrap_or(true);

            let representative = |handle: RigidBodyHandle| {
                if let Some(link) = multibodies.rigid_body_link(handle).copied() {
                    let multibody = multibodies.get_multibody(link.multibody).unwrap();
                    multibody
                        .link(1) // Use the link 1 to cover the case where the multibody root is fixed.
                        .or(multibody.link(0)) // TODO: Never happens?
                        .map(|l| l.rigid_body)
                        .unwrap()
                } else {
                    handle
                }
            };

            body_pair = (
                body_pair.0.map(representative),
                body_pair.1.map(representative),
            );

            match (is_fixed1, is_fixed2) {
                (false, false) => {
                    let rb1 = &bodies[body_pair.0.unwrap()];
                    let rb2 = &bodies[body_pair.1.unwrap()];
                    let color_mask =
                        bcolors[rb1.ids.active_set_id] | bcolors[rb2.ids.active_set_id];
                    *color = (!color_mask).trailing_zeros() as usize;
                    color_len[*color] += 1;
                    bcolors[rb1.ids.active_set_id] |= 1 << *color;
                    bcolors[rb2.ids.active_set_id] |= 1 << *color;
                }
                (true, false) => {
                    let rb2 = &bodies[body_pair.1.unwrap()];
                    let color_mask = bcolors[rb2.ids.active_set_id];
                    *color = 127 - (!color_mask).leading_zeros() as usize;
                    color_len[*color] += 1;
                    bcolors[rb2.ids.active_set_id] |= 1 << *color;
                }
                (false, true) => {
                    let rb1 = &bodies[body_pair.0.unwrap()];
                    let color_mask = bcolors[rb1.ids.active_set_id];
                    *color = 127 - (!color_mask).leading_zeros() as usize;
                    color_len[*color] += 1;
                    bcolors[rb1.ids.active_set_id] |= 1 << *color;
                }
                (true, true) => unreachable!(),
            }
        }

        let mut sort_offsets = [0; 128];
        let mut last_offset = 0;

        for i in 0..128 {
            if color_len[i] != 0 {
                self.groups.push(last_offset);
                sort_offsets[i] = last_offset;
                last_offset += color_len[i];
            }
        }

        self.sorted_interactions
            .resize(interaction_indices.len(), 0);

        for (interaction_id, color) in interaction_indices
            .iter()
            .zip(self.interaction_colors.iter())
        {
            self.sorted_interactions[sort_offsets[*color]] = *interaction_id;
            sort_offsets[*color] += 1;
        }

        self.groups.push(self.sorted_interactions.len());
    }
}

pub(crate) struct InteractionGroups {
    #[cfg(feature = "simd-is-enabled")]
    buckets: VecMap<([usize; SIMD_WIDTH], usize)>,
    #[cfg(feature = "simd-is-enabled")]
    body_masks: Vec<u128>,
    pub simd_interactions: Vec<ContactManifoldIndex>,
    pub nongrouped_interactions: Vec<ContactManifoldIndex>,
}

impl InteractionGroups {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "simd-is-enabled")]
            buckets: VecMap::new(),
            #[cfg(feature = "simd-is-enabled")]
            body_masks: Vec::new(),
            simd_interactions: Vec::new(),
            nongrouped_interactions: Vec::new(),
        }
    }

    // #[cfg(not(feature = "parallel"))]
    // pub fn clear(&mut self) {
    //     #[cfg(feature = "simd-is-enabled")]
    //     {
    //         self.buckets.clear();
    //         self.body_masks.clear();
    //         self.simd_interactions.clear();
    //     }
    //     self.nongrouped_interactions.clear();
    // }

    // TODO: there is a lot of duplicated code with group_manifolds here.
    // But we don't refactor just now because we may end up with distinct
    // grouping strategies in the future.
    #[cfg(not(feature = "simd-is-enabled"))]
    pub fn group_joints(
        &mut self,
        _island_id: usize,
        _islands: &IslandManager,
        _bodies: &RigidBodySet,
        _interactions: &[JointGraphEdge],
        interaction_indices: &[JointIndex],
    ) {
        self.nongrouped_interactions
            .extend_from_slice(interaction_indices);
    }

    #[cfg(feature = "simd-is-enabled")]
    pub fn group_joints(
        &mut self,
        island_id: usize,
        islands: &IslandManager,
        bodies: &RigidBodySet,
        interactions: &[JointGraphEdge],
        interaction_indices: &[JointIndex],
    ) {
        // TODO: right now, we only sort based on the axes locked by the joint.
        // We could also take motors and limits into account in the future (most of
        // the SIMD constraints generation for motors and limits is already implemented).
        #[cfg(feature = "dim3")]
        const NUM_JOINT_TYPES: usize = 64;
        #[cfg(feature = "dim2")]
        const NUM_JOINT_TYPES: usize = 8;

        // The j-th bit of joint_type_conflicts[i] indicates that the
        // j-th bucket contains a joint with a type different than `i`.
        let mut joint_type_conflicts = [0u128; NUM_JOINT_TYPES];

        // Note: each bit of a body mask indicates what bucket already contains
        // a constraints involving this body.
        // TODO: currently, this is a bit overconservative because when a bucket
        // is full, we don't clear the corresponding body mask bit. This may result
        // in less grouped constraints.
        self.body_masks
            .resize(islands.island(island_id).len(), 0u128);

        // NOTE: each bit of the occupied mask indicates what bucket already
        // contains at least one constraint.
        let mut occupied_mask = 0u128;

        for interaction_i in interaction_indices {
            let interaction = &interactions[*interaction_i].weight;

            let rb1 = &bodies[interaction.body1];
            let rb2 = &bodies[interaction.body2];

            let is_fixed1 = !rb1.is_dynamic_or_kinematic();
            let is_fixed2 = !rb2.is_dynamic_or_kinematic();

            if is_fixed1 && is_fixed2 {
                continue;
            }

            if !interaction.data.supports_simd_constraints() {
                // This joint does not support simd constraints yet.
                self.nongrouped_interactions.push(*interaction_i);
                continue;
            }

            let ijoint = interaction.data.locked_axes.bits() as usize;
            let i1 = rb1.ids.active_set_id;
            let i2 = rb2.ids.active_set_id;
            let conflicts = self.body_masks.get(i1).copied().unwrap_or_default()
                | self.body_masks.get(i2).copied().unwrap_or_default()
                | joint_type_conflicts[ijoint];
            let conflictfree_targets = !(conflicts & occupied_mask); // The & is because we consider empty buckets as free of conflicts.
            let conflictfree_occupied_targets = conflictfree_targets & occupied_mask;

            let target_index = if conflictfree_occupied_targets != 0 {
                // Try to fill partial WContacts first.
                conflictfree_occupied_targets.trailing_zeros()
            } else {
                conflictfree_targets.trailing_zeros()
            };

            if target_index == 128 {
                // The interaction conflicts with every bucket we can manage.
                // So push it in a nongrouped interaction list that won't be combined with
                // any other interactions.
                self.nongrouped_interactions.push(*interaction_i);
                continue;
            }

            let target_mask_bit = 1 << target_index;

            let bucket = self
                .buckets
                .entry(target_index as usize)
                .or_insert_with(|| ([0; SIMD_WIDTH], 0));

            if bucket.1 == SIMD_LAST_INDEX {
                // We completed our group.
                (bucket.0)[SIMD_LAST_INDEX] = *interaction_i;
                self.simd_interactions.extend_from_slice(&bucket.0);
                bucket.1 = 0;
                occupied_mask &= !target_mask_bit;

                for k in 0..NUM_JOINT_TYPES {
                    joint_type_conflicts[k] &= !target_mask_bit;
                }
            } else {
                (bucket.0)[bucket.1] = *interaction_i;
                bucket.1 += 1;
                occupied_mask |= target_mask_bit;

                for k in 0..ijoint {
                    joint_type_conflicts[k] |= target_mask_bit;
                }
                for k in ijoint + 1..NUM_JOINT_TYPES {
                    joint_type_conflicts[k] |= target_mask_bit;
                }
            }

            // NOTE: fixed bodies don't transmit forces. Therefore they don't
            // imply any interaction conflicts.
            if !is_fixed1 {
                self.body_masks[i1] |= target_mask_bit;
            }

            if !is_fixed2 {
                self.body_masks[i2] |= target_mask_bit;
            }
        }

        self.nongrouped_interactions.extend(
            self.buckets
                .values()
                .flat_map(|e| e.0.iter().take(e.1).copied()),
        );
        self.buckets.clear();
        self.body_masks.iter_mut().for_each(|e| *e = 0);

        assert!(
            self.simd_interactions.len() % SIMD_WIDTH == 0,
            "Invalid SIMD contact grouping."
        );

        //        println!(
        //            "Num grouped interactions: {}, nongrouped: {}",
        //            self.simd_interactions.len(),
        //            self.nongrouped_interactions.len()
        //        );
    }

    pub fn clear_groups(&mut self) {
        self.simd_interactions.clear();
        self.nongrouped_interactions.clear();
    }

    #[cfg(not(feature = "simd-is-enabled"))]
    pub fn group_manifolds(
        &mut self,
        _island_id: usize,
        _islands: &IslandManager,
        _bodies: &RigidBodySet,
        _interactions: &[&mut ContactManifold],
        interaction_indices: &[ContactManifoldIndex],
    ) {
        self.nongrouped_interactions
            .extend_from_slice(interaction_indices);
    }

    #[cfg(feature = "simd-is-enabled")]
    pub fn group_manifolds(
        &mut self,
        island_id: usize,
        islands: &IslandManager,
        bodies: &RigidBodySet,
        interactions: &[&mut ContactManifold],
        interaction_indices: &[ContactManifoldIndex],
    ) {
        // Note: each bit of a body mask indicates what bucket already contains
        // a constraints involving this body.
        // TODO: currently, this is a bit overconservative because when a bucket
        // is full, we don't clear the corresponding body mask bit. This may result
        // in less grouped contacts.
        // NOTE: body_masks and buckets are already cleared/zeroed at the end of each sort loop.
        self.body_masks
            .resize(islands.island(island_id).len(), 0u128);

        // NOTE: each bit of the occupied mask indicates what bucket already
        // contains at least one constraint.
        let mut occupied_mask = 0u128;
        let max_interaction_points = interaction_indices
            .iter()
            .map(|i| interactions[*i].data.num_active_contacts())
            .max()
            .unwrap_or(1);

        // TODO: find a way to reduce the number of iteration.
        // There must be a way to iterate just once on every interaction indices
        // instead of MAX_MANIFOLD_POINTS times.
        for k in 1..=max_interaction_points {
            for interaction_i in interaction_indices {
                let interaction = &interactions[*interaction_i];

                // TODO: how could we avoid iterating
                // on each interaction at every iteration on k?
                if interaction.data.num_active_contacts() != k {
                    continue;
                }

                let (status1, active_set_id1) = if let Some(rb1) = interaction.data.rigid_body1 {
                    let rb1 = &bodies[rb1];
                    (rb1.body_type, rb1.ids.active_set_id as u32)
                } else {
                    (RigidBodyType::Fixed, u32::MAX)
                };
                let (status2, active_set_id2) = if let Some(rb2) = interaction.data.rigid_body2 {
                    let rb2 = &bodies[rb2];
                    (rb2.body_type, rb2.ids.active_set_id as u32)
                } else {
                    (RigidBodyType::Fixed, u32::MAX)
                };

                let is_fixed1 = !status1.is_dynamic_or_kinematic();
                let is_fixed2 = !status2.is_dynamic_or_kinematic();

                // TODO: don't generate interactions between fixed bodies in the first place.
                if is_fixed1 && is_fixed2 {
                    continue;
                }

                let i1 = active_set_id1;
                let i2 = active_set_id2;
                let mask1 = if !is_fixed1 {
                    self.body_masks[i1 as usize]
                } else {
                    0
                };
                let mask2 = if !is_fixed2 {
                    self.body_masks[i2 as usize]
                } else {
                    0
                };
                let conflicts = mask1 | mask2;
                let conflictfree_targets = !(conflicts & occupied_mask); // The & is because we consider empty buckets as free of conflicts.
                let conflictfree_occupied_targets = conflictfree_targets & occupied_mask;

                let target_index = if conflictfree_occupied_targets != 0 {
                    // Try to fill partial WContacts first.
                    conflictfree_occupied_targets.trailing_zeros()
                } else {
                    conflictfree_targets.trailing_zeros()
                };

                if target_index == 128 {
                    // The interaction conflicts with every bucket we can manage.
                    // So push it in an nongrouped interaction list that won't be combined with
                    // any other interactions.
                    self.nongrouped_interactions.push(*interaction_i);
                    continue;
                }

                let target_mask_bit = 1 << target_index;

                let bucket = self
                    .buckets
                    .entry(target_index as usize)
                    .or_insert_with(|| ([0; SIMD_WIDTH], 0));

                if bucket.1 == SIMD_LAST_INDEX {
                    // We completed our group.
                    (bucket.0)[SIMD_LAST_INDEX] = *interaction_i;
                    self.simd_interactions.extend_from_slice(&bucket.0);
                    bucket.1 = 0;
                    occupied_mask &= !target_mask_bit;
                } else {
                    (bucket.0)[bucket.1] = *interaction_i;
                    bucket.1 += 1;
                    occupied_mask |= target_mask_bit;
                }

                // NOTE: fixed bodies don't transmit forces. Therefore they don't
                // imply any interaction conflicts.
                if !is_fixed1 {
                    self.body_masks[i1 as usize] |= target_mask_bit;
                }

                if !is_fixed2 {
                    self.body_masks[i2 as usize] |= target_mask_bit;
                }
            }

            self.nongrouped_interactions.extend(
                self.buckets
                    .values()
                    .flat_map(|e| e.0.iter().take(e.1).copied()),
            );
            self.buckets.clear();
            self.body_masks.iter_mut().for_each(|e| *e = 0);
            occupied_mask = 0u128;
        }

        assert!(
            self.simd_interactions.len() % SIMD_WIDTH == 0,
            "Invalid SIMD contact grouping."
        );
    }
}
