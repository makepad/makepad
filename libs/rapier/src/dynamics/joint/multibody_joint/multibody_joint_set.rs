use parry::utils::hashset::HashSet;

use crate::data::{Arena, Coarena, Index};
use crate::dynamics::joint::MultibodyLink;
use crate::dynamics::{GenericJoint, Multibody, MultibodyJoint, RigidBodyHandle};
use crate::geometry::{InteractionGraph, RigidBodyGraphIndex};

/// The unique handle of an multibody_joint added to a `MultibodyJointSet`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[repr(transparent)]
pub struct MultibodyJointHandle(pub Index);

/// The temporary index of a multibody added to a `MultibodyJointSet`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[repr(transparent)]
pub struct MultibodyIndex(pub Index);

impl MultibodyJointHandle {
    /// Converts this handle into its (index, generation) components.
    pub fn into_raw_parts(self) -> (u32, u32) {
        self.0.into_raw_parts()
    }

    /// Reconstructs an handle from its (index, generation) components.
    pub fn from_raw_parts(id: u32, generation: u32) -> Self {
        Self(Index::from_raw_parts(id, generation))
    }

    /// An always-invalid rigid-body handle.
    pub fn invalid() -> Self {
        Self(Index::from_raw_parts(
            crate::INVALID_U32,
            crate::INVALID_U32,
        ))
    }
}

impl Default for MultibodyJointHandle {
    fn default() -> Self {
        Self::invalid()
    }
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
/// Indexes usable to get a multibody link from a `MultibodyJointSet`.
///
/// ```
/// # use rapier3d::prelude::*;
/// # let mut bodies = RigidBodySet::new();
/// # let mut multibody_joint_set = MultibodyJointSet::new();
/// # let body1 = bodies.insert(RigidBodyBuilder::dynamic());
/// # let body2 = bodies.insert(RigidBodyBuilder::dynamic());
/// # let joint = RevoluteJointBuilder::new(Vector::Y);
/// # multibody_joint_set.insert(body1, body2, joint, true);
/// # let multibody_link_id = multibody_joint_set.rigid_body_link(body2).unwrap();
/// // With:
/// //     multibody_joint_set: MultibodyJointSet
/// //     multibody_link_id: MultibodyLinkId
/// let multibody = &multibody_joint_set[multibody_link_id.multibody];
/// let link = multibody.link(multibody_link_id.id).expect("Link not found.");
/// ```
pub struct MultibodyLinkId {
    pub(crate) graph_id: RigidBodyGraphIndex,
    /// The multibody index to be used as `&multibody_joint_set[multibody]` to
    /// retrieve the multibody reference.
    pub multibody: MultibodyIndex,
    /// The multibody link index to be given to [`Multibody::link`].
    pub id: usize,
}

impl Default for MultibodyLinkId {
    fn default() -> Self {
        Self {
            graph_id: RigidBodyGraphIndex::new(crate::INVALID_U32),
            multibody: MultibodyIndex(Index::from_raw_parts(
                crate::INVALID_U32,
                crate::INVALID_U32,
            )),
            id: 0,
        }
    }
}

#[derive(Default)]
/// A set of rigid bodies that can be handled by a physics pipeline.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Debug)]
pub struct MultibodyJointSet {
    pub(crate) multibodies: Arena<Multibody>, // NOTE: a Slab would be sufficient.
    pub(crate) rb2mb: Coarena<MultibodyLinkId>,
    // NOTE: this is mostly for the island extraction. So perhaps we won’t need
    //       that any more in the future when we improve our island builder.
    pub(crate) connectivity_graph: InteractionGraph<RigidBodyHandle, ()>,
    pub(crate) to_wake_up: HashSet<RigidBodyHandle>,
    /// A set of rigid-body pairs to join in the island manager during the next timestep.
    pub(crate) to_join: HashSet<(RigidBodyHandle, RigidBodyHandle)>,
}

impl MultibodyJointSet {
    /// Create a new empty set of multibodies.
    pub fn new() -> Self {
        Self {
            multibodies: Arena::new(),
            rb2mb: Coarena::new(),
            connectivity_graph: InteractionGraph::new(),
            to_wake_up: HashSet::default(),
            to_join: HashSet::default(),
        }
    }

    /// Iterates through all the multibody joints from this set.
    pub fn iter(
        &self,
    ) -> impl Iterator<
        Item = (
            MultibodyJointHandle,
            &MultibodyLinkId,
            &Multibody,
            &MultibodyLink,
        ),
    > {
        self.rb2mb
            .iter()
            .filter(|(_, link)| link.id > 0) // The first link of a rigid-body hasn’t been added by the user.
            .map(|(h, link)| {
                let mb = &self.multibodies[link.multibody.0];
                (MultibodyJointHandle(h), link, mb, mb.link(link.id).unwrap())
            })
    }

    /// Inserts a new kinematic multibody joint into this set.
    pub fn insert_kinematic(
        &mut self,
        body1: RigidBodyHandle,
        body2: RigidBodyHandle,
        data: impl Into<GenericJoint>,
        wake_up: bool,
    ) -> Option<MultibodyJointHandle> {
        self.do_insert(body1, body2, data, true, wake_up)
    }

    /// Inserts a new multibody joint into this set.
    pub fn insert(
        &mut self,
        body1: RigidBodyHandle,
        body2: RigidBodyHandle,
        data: impl Into<GenericJoint>,
        wake_up: bool,
    ) -> Option<MultibodyJointHandle> {
        self.do_insert(body1, body2, data, false, wake_up)
    }

    /// Inserts a new multibody_joint into this set.
    fn do_insert(
        &mut self,
        body1: RigidBodyHandle,
        body2: RigidBodyHandle,
        data: impl Into<GenericJoint>,
        kinematic: bool,
        wake_up: bool,
    ) -> Option<MultibodyJointHandle> {
        let link1 = self.rb2mb.get(body1.0).copied().unwrap_or_else(|| {
            let mb_handle = self.multibodies.insert(Multibody::with_root(body1, true));
            MultibodyLinkId {
                graph_id: self.connectivity_graph.graph.add_node(body1),
                multibody: MultibodyIndex(mb_handle),
                id: 0,
            }
        });

        let link2 = self.rb2mb.get(body2.0).copied().unwrap_or_else(|| {
            let mb_handle = self.multibodies.insert(Multibody::with_root(body2, true));
            MultibodyLinkId {
                graph_id: self.connectivity_graph.graph.add_node(body2),
                multibody: MultibodyIndex(mb_handle),
                id: 0,
            }
        });

        if link1.multibody == link2.multibody || link2.id != 0 {
            // This would introduce an invalid configuration.
            return None;
        }

        self.connectivity_graph
            .graph
            .add_edge(link1.graph_id, link2.graph_id, ());
        self.rb2mb.insert(body1.0, link1);
        self.rb2mb.insert(body2.0, link2);

        let mb2 = self.multibodies.remove(link2.multibody.0).unwrap();
        let multibody1 = &mut self.multibodies[link1.multibody.0];

        for mb_link2 in mb2.links() {
            let link = self.rb2mb.get_mut(mb_link2.rigid_body.0).unwrap();
            link.multibody = link1.multibody;
            link.id += multibody1.num_links();
        }

        multibody1.append(mb2, link1.id, MultibodyJoint::new(data.into(), kinematic));

        if wake_up {
            self.to_wake_up.insert(body1);
            self.to_wake_up.insert(body2);
        }

        self.to_join.insert((body1, body2));

        // Because each rigid-body can only have one parent link,
        // we can use the second rigid-body’s handle as the multibody_joint’s
        // handle.
        Some(MultibodyJointHandle(body2.0))
    }

    /// Removes a multibody_joint from this set.
    pub fn remove(&mut self, handle: MultibodyJointHandle, wake_up: bool) {
        if let Some(removed) = self.rb2mb.get(handle.0).copied() {
            let multibody = self.multibodies.remove(removed.multibody.0).unwrap();

            // Remove the edge from the connectivity graph.
            if let Some(parent_link) = multibody.link(removed.id).unwrap().parent_id() {
                let parent_rb = multibody.link(parent_link).unwrap().rigid_body;
                let parent_graph_id = self.rb2mb.get(parent_rb.0).unwrap().graph_id;
                self.connectivity_graph
                    .remove_edge(parent_graph_id, removed.graph_id);

                if wake_up {
                    self.to_wake_up.insert(RigidBodyHandle(handle.0));
                    self.to_wake_up.insert(parent_rb);
                }

                // TODO: remove the node if it no longer has any attached edges?

                // Extract the individual sub-trees generated by this removal.
                let multibodies = multibody.remove_link(removed.id, true);

                // Update the rb2mb mapping.
                for multibody in multibodies {
                    if multibody.num_links() == 1 {
                        // We don’t have any multibody_joint attached to this body, remove it.
                        let isolated_link = multibody.link(0).unwrap();
                        let isolated_graph_id =
                            self.rb2mb.get(isolated_link.rigid_body.0).unwrap().graph_id;
                        if let Some(other) = self.connectivity_graph.remove_node(isolated_graph_id)
                        {
                            self.rb2mb.get_mut(other.0).unwrap().graph_id = isolated_graph_id;
                        }
                    } else {
                        let mb_id = self.multibodies.insert(multibody);
                        for link in self.multibodies[mb_id].links() {
                            let ids = self.rb2mb.get_mut(link.rigid_body.0).unwrap();
                            ids.multibody = MultibodyIndex(mb_id);
                            ids.id = link.internal_id;
                        }
                    }
                }
            }
        }
    }

    /// Removes all the multibody_joints from the multibody the given rigid-body is part of.
    pub fn remove_multibody_articulations(&mut self, handle: RigidBodyHandle, wake_up: bool) {
        if let Some(removed) = self.rb2mb.get(handle.0).copied() {
            // Remove the multibody.
            let multibody = self.multibodies.remove(removed.multibody.0).unwrap();
            for link in multibody.links() {
                let rb_handle = link.rigid_body;

                if wake_up {
                    self.to_wake_up.insert(rb_handle);
                }

                // Remove the rigid-body <-> multibody mapping for this link.
                let removed = self.rb2mb.remove(rb_handle.0, Default::default()).unwrap();
                // Remove the node (and all it’s edges) from the connectivity graph.
                if let Some(other) = self.connectivity_graph.remove_node(removed.graph_id) {
                    self.rb2mb.get_mut(other.0).unwrap().graph_id = removed.graph_id;
                }
            }
        }
    }

    /// Removes all the multibody joints attached to a rigid-body.
    pub fn remove_joints_attached_to_rigid_body(&mut self, rb_to_remove: RigidBodyHandle) {
        // TODO: optimize this.
        if let Some(link_to_remove) = self.rb2mb.get(rb_to_remove.0).copied() {
            let mut articulations_to_remove = vec![];
            for (rb1, rb2, _) in self
                .connectivity_graph
                .interactions_with(link_to_remove.graph_id)
            {
                // There is a multibody_joint handle is equal to the second rigid-body’s handle.
                articulations_to_remove.push(MultibodyJointHandle(rb2.0));

                self.to_wake_up.insert(rb1);
                self.to_wake_up.insert(rb2);
            }

            for articulation_handle in articulations_to_remove {
                self.remove(articulation_handle, true);
            }
        }
    }

    /// Returns the link of this multibody attached to the given rigid-body.
    ///
    /// Returns `None` if `rb` isn’t part of any rigid-body.
    pub fn rigid_body_link(&self, rb: RigidBodyHandle) -> Option<&MultibodyLinkId> {
        self.rb2mb.get(rb.0)
    }

    /// Gets a reference to a multibody, based on its temporary index.
    pub fn get_multibody(&self, index: MultibodyIndex) -> Option<&Multibody> {
        self.multibodies.get(index.0)
    }

    /// Gets a mutable reference to a multibody, based on its temporary index.
    /// `MultibodyJointSet`.
    pub fn get_multibody_mut(&mut self, index: MultibodyIndex) -> Option<&mut Multibody> {
        // TODO: modification tracking.
        self.multibodies.get_mut(index.0)
    }

    /// Gets a mutable reference to a multibody, based on its temporary index.
    ///
    /// This method will bypass any modification-detection automatically done by the
    /// `MultibodyJointSet`.
    pub fn get_multibody_mut_internal(&mut self, index: MultibodyIndex) -> Option<&mut Multibody> {
        self.multibodies.get_mut(index.0)
    }

    /// Gets a reference to the multibody identified by its `handle`.
    pub fn get(&self, handle: MultibodyJointHandle) -> Option<(&Multibody, usize)> {
        let link = self.rb2mb.get(handle.0)?;
        let multibody = self.multibodies.get(link.multibody.0)?;
        Some((multibody, link.id))
    }

    /// Gets a mutable reference to the multibody identified by its `handle`.
    pub fn get_mut(&mut self, handle: MultibodyJointHandle) -> Option<(&mut Multibody, usize)> {
        let link = self.rb2mb.get(handle.0)?;
        let multibody = self.multibodies.get_mut(link.multibody.0)?;
        Some((multibody, link.id))
    }

    /// Gets a mutable reference to the multibody identified by its `handle`.
    ///
    /// This method will bypass any modification-detection automatically done by the MultibodyJointSet.
    pub fn get_mut_internal(
        &mut self,
        handle: MultibodyJointHandle,
    ) -> Option<(&mut Multibody, usize)> {
        // TODO: modification tracking?
        let link = self.rb2mb.get(handle.0)?;
        let multibody = self.multibodies.get_mut(link.multibody.0)?;
        Some((multibody, link.id))
    }

    /// Gets the joint with the given handle without a known generation.
    ///
    /// This is useful when you know you want the joint at index `i` but
    /// don't know what is its current generation number. Generation numbers are
    /// used to protect from the ABA problem because the joint position `i`
    /// are recycled between two insertion and a removal.
    ///
    /// Using this is discouraged in favor of `self.get(handle)` which does not
    /// suffer form the ABA problem.
    pub fn get_unknown_gen(&self, i: u32) -> Option<(&Multibody, usize, MultibodyJointHandle)> {
        let link = self.rb2mb.get_unknown_gen(i)?;
        let generation = self.rb2mb.get_gen(i)?;
        let multibody = self.multibodies.get(link.multibody.0)?;
        Some((
            multibody,
            link.id,
            MultibodyJointHandle(Index::from_raw_parts(i, generation)),
        ))
    }

    /// Returns the joint between two rigid-bodies (if it exists).
    pub fn joint_between(
        &self,
        rb1: RigidBodyHandle,
        rb2: RigidBodyHandle,
    ) -> Option<(MultibodyJointHandle, &Multibody, &MultibodyLink)> {
        let id1 = self.rb2mb.get(rb1.0)?;
        let id2 = self.rb2mb.get(rb2.0)?;

        // Both bodies must be part of the same multibody.
        if id1.multibody != id2.multibody {
            return None;
        }

        let mb = self.multibodies.get(id1.multibody.0)?;

        // NOTE: if there is a joint between these two bodies, then
        //       one of the bodies must be the parent of the other.
        let link1 = mb.link(id1.id)?;
        let parent1 = link1.parent_id();

        if parent1 == Some(id2.id) {
            Some((MultibodyJointHandle(rb1.0), mb, link1))
        } else {
            let link2 = mb.link(id2.id)?;
            let parent2 = link2.parent_id();

            if parent2 == Some(id1.id) {
                Some((MultibodyJointHandle(rb2.0), mb, link2))
            } else {
                None
            }
        }
    }

    /// Iterates through all the joints attached to the given rigid-body.
    pub fn attached_joints(
        &self,
        rb: RigidBodyHandle,
    ) -> impl Iterator<Item = (RigidBodyHandle, RigidBodyHandle, MultibodyJointHandle)> + '_ {
        self.rb2mb
            .get(rb.0)
            .into_iter()
            .flat_map(move |link| self.connectivity_graph.interactions_with(link.graph_id))
            .map(|inter| {
                // NOTE: the joint handle is always equal to the handle of the second rigid-body.
                (inter.0, inter.1, MultibodyJointHandle(inter.1.0))
            })
    }

    /// Iterate through the handles of all the rigid-bodies attached to this rigid-body
    /// by a multibody_joint.
    pub fn attached_bodies(
        &self,
        body: RigidBodyHandle,
    ) -> impl Iterator<Item = RigidBodyHandle> + '_ {
        self.rb2mb
            .get(body.0)
            .into_iter()
            .flat_map(move |id| self.connectivity_graph.interactions_with(id.graph_id))
            .map(move |inter| crate::utils::select_other((inter.0, inter.1), body))
    }

    /// Iterate through the handles of all the rigid-bodies attached to this rigid-body
    /// by an enabled multibody_joint.
    pub fn bodies_attached_with_enabled_joint(
        &self,
        body: RigidBodyHandle,
    ) -> impl Iterator<Item = RigidBodyHandle> + '_ {
        self.attached_bodies(body).filter(move |other| {
            if let Some((_, _, link)) = self.joint_between(body, *other) {
                link.joint.data.is_enabled()
            } else {
                false
            }
        })
    }

    /// Iterates through all the multibodies on this set.
    pub fn multibodies(&self) -> impl Iterator<Item = &Multibody> {
        self.multibodies.iter().map(|e| e.1)
    }
}

impl std::ops::Index<MultibodyIndex> for MultibodyJointSet {
    type Output = Multibody;

    fn index(&self, index: MultibodyIndex) -> &Multibody {
        &self.multibodies[index.0]
    }
}

// impl Index<MultibodyJointHandle> for MultibodyJointSet {
//     type Output = Multibody;
//
//     fn index(&self, index: MultibodyJointHandle) -> &Multibody {
//         &self.multibodies[index.0]
//     }
// }
