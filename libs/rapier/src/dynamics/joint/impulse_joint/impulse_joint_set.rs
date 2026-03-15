use parry::utils::hashset::HashSet;

use super::ImpulseJoint;
use crate::geometry::{InteractionGraph, RigidBodyGraphIndex, TemporaryInteractionIndex};

use crate::data::Coarena;
use crate::data::arena::Arena;
use crate::dynamics::{GenericJoint, IslandManager, RigidBodyHandle, RigidBodySet};

/// The unique identifier of a joint added to the joint set.
/// The unique identifier of a collider added to a collider set.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[repr(transparent)]
pub struct ImpulseJointHandle(pub crate::data::arena::Index);

impl ImpulseJointHandle {
    /// Converts this handle into its (index, generation) components.
    pub fn into_raw_parts(self) -> (u32, u32) {
        self.0.into_raw_parts()
    }

    /// Reconstructs an handle from its (index, generation) components.
    pub fn from_raw_parts(id: u32, generation: u32) -> Self {
        Self(crate::data::arena::Index::from_raw_parts(id, generation))
    }

    /// An always-invalid joint handle.
    pub fn invalid() -> Self {
        Self(crate::data::arena::Index::from_raw_parts(
            crate::INVALID_U32,
            crate::INVALID_U32,
        ))
    }
}

pub(crate) type JointIndex = usize;
pub(crate) type JointGraphEdge = crate::data::graph::Edge<ImpulseJoint>;

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Default, Debug)]
/// The collection that stores all joints connecting rigid bodies in your physics world.
///
/// Joints constrain how two bodies can move relative to each other. This set manages
/// all joint instances (hinges, sliders, springs, etc.) using handles for safe access.
///
/// # Common joint types
/// - [`FixedJoint`](crate::dynamics::FixedJoint): Weld two bodies together
/// - [`RevoluteJoint`](crate::dynamics::RevoluteJoint): Hinge (rotation around axis)
/// - [`PrismaticJoint`](crate::dynamics::PrismaticJoint): Slider (translation along axis)
/// - [`SpringJoint`](crate::dynamics::SpringJoint): Elastic connection
/// - [`RopeJoint`](crate::dynamics::RopeJoint): Maximum distance limit
///
/// # Example
/// ```
/// # use rapier3d::prelude::*;
/// # let mut bodies = RigidBodySet::new();
/// # let body1 = bodies.insert(RigidBodyBuilder::dynamic());
/// # let body2 = bodies.insert(RigidBodyBuilder::dynamic());
/// let mut joints = ImpulseJointSet::new();
///
/// // Create a hinge connecting two bodies
/// let joint = RevoluteJointBuilder::new(Vector::Y)
///     .local_anchor1(Vector::new(1.0, 0.0, 0.0))
///     .local_anchor2(Vector::new(-1.0, 0.0, 0.0))
///     .build();
/// let handle = joints.insert(body1, body2, joint, true);
/// ```
pub struct ImpulseJointSet {
    rb_graph_ids: Coarena<RigidBodyGraphIndex>,
    /// Map joint handles to edge ids on the graph.
    joint_ids: Arena<TemporaryInteractionIndex>,
    joint_graph: InteractionGraph<RigidBodyHandle, ImpulseJoint>,
    /// A set of rigid-body handles to wake-up during the next timestep.
    pub(crate) to_wake_up: HashSet<RigidBodyHandle>,
    /// A set of rigid-body pairs to join in the island manager during the next timestep.
    pub(crate) to_join: HashSet<(RigidBodyHandle, RigidBodyHandle)>,
}

impl ImpulseJointSet {
    /// Creates a new empty set of impulse_joints.
    pub fn new() -> Self {
        Self {
            rb_graph_ids: Coarena::new(),
            joint_ids: Arena::new(),
            joint_graph: InteractionGraph::new(),
            to_wake_up: HashSet::default(),
            to_join: HashSet::default(),
        }
    }

    /// Returns how many joints are currently in this collection.
    pub fn len(&self) -> usize {
        self.joint_graph.graph.edges.len()
    }

    /// Returns `true` if there are no joints in this collection.
    pub fn is_empty(&self) -> bool {
        self.joint_graph.graph.edges.is_empty()
    }

    /// Returns the internal graph structure (nodes=bodies, edges=joints).
    ///
    /// Advanced usage - most users should use `attached_joints()` instead.
    pub fn joint_graph(&self) -> &InteractionGraph<RigidBodyHandle, ImpulseJoint> {
        &self.joint_graph
    }

    /// Returns all joints connecting two specific bodies.
    ///
    /// Usually returns 0 or 1 joint, but multiple joints can connect the same pair.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let mut joints = ImpulseJointSet::new();
    /// # let body1 = bodies.insert(RigidBodyBuilder::dynamic());
    /// # let body2 = bodies.insert(RigidBodyBuilder::dynamic());
    /// # let joint = RevoluteJointBuilder::new(Vector::Y);
    /// # joints.insert(body1, body2, joint, true);
    /// for (handle, joint) in joints.joints_between(body1, body2) {
    ///     println!("Found joint {:?}", handle);
    /// }
    /// ```
    pub fn joints_between(
        &self,
        body1: RigidBodyHandle,
        body2: RigidBodyHandle,
    ) -> impl Iterator<Item = (ImpulseJointHandle, &ImpulseJoint)> {
        self.rb_graph_ids
            .get(body1.0)
            .zip(self.rb_graph_ids.get(body2.0))
            .into_iter()
            .flat_map(move |(id1, id2)| self.joint_graph.interaction_pair(*id1, *id2).into_iter())
            .map(|inter| (inter.2.handle, inter.2))
    }

    /// Returns all joints attached to a specific body.
    ///
    /// Each result is `(body1, body2, joint_handle, joint)` where one of the bodies
    /// matches the queried body.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let mut joints = ImpulseJointSet::new();
    /// # let body_handle = bodies.insert(RigidBodyBuilder::dynamic());
    /// # let other_body = bodies.insert(RigidBodyBuilder::dynamic());
    /// # let joint = RevoluteJointBuilder::new(Vector::Y);
    /// # joints.insert(body_handle, other_body, joint, true);
    /// for (b1, b2, j_handle, joint) in joints.attached_joints(body_handle) {
    ///     println!("Body connected to {:?} via {:?}", b2, j_handle);
    /// }
    /// ```
    pub fn attached_joints(
        &self,
        body: RigidBodyHandle,
    ) -> impl Iterator<
        Item = (
            RigidBodyHandle,
            RigidBodyHandle,
            ImpulseJointHandle,
            &ImpulseJoint,
        ),
    > {
        self.rb_graph_ids
            .get(body.0)
            .into_iter()
            .flat_map(move |id| self.joint_graph.interactions_with(*id))
            .map(|inter| (inter.0, inter.1, inter.2.handle, inter.2))
    }

    /// Iterates through all the impulse joints attached to the given rigid-body.
    pub fn map_attached_joints_mut(
        &mut self,
        body: RigidBodyHandle,
        mut f: impl FnMut(RigidBodyHandle, RigidBodyHandle, ImpulseJointHandle, &mut ImpulseJoint),
    ) {
        self.rb_graph_ids.get(body.0).into_iter().for_each(|id| {
            for inter in self.joint_graph.interactions_with_mut(*id) {
                (f)(inter.0, inter.1, inter.3.handle, inter.3)
            }
        })
    }

    /// Returns only the enabled joints attached to a body.
    ///
    /// Same as `attached_joints()` but filters out disabled joints.
    pub fn attached_enabled_joints(
        &self,
        body: RigidBodyHandle,
    ) -> impl Iterator<
        Item = (
            RigidBodyHandle,
            RigidBodyHandle,
            ImpulseJointHandle,
            &ImpulseJoint,
        ),
    > {
        self.attached_joints(body)
            .filter(|inter| inter.3.data.is_enabled())
    }

    /// Checks if the given joint handle is valid (joint still exists).
    pub fn contains(&self, handle: ImpulseJointHandle) -> bool {
        self.joint_ids.contains(handle.0)
    }

    /// Returns a read-only reference to the joint with the given handle.
    pub fn get(&self, handle: ImpulseJointHandle) -> Option<&ImpulseJoint> {
        let id = self.joint_ids.get(handle.0)?;
        self.joint_graph.graph.edge_weight(*id)
    }

    /// Returns a mutable reference to the joint with the given handle.
    ///
    /// # Parameters
    /// * `wake_up_connected_bodies` - If `true`, wakes up both bodies connected by this joint
    pub fn get_mut(
        &mut self,
        handle: ImpulseJointHandle,
        wake_up_connected_bodies: bool,
    ) -> Option<&mut ImpulseJoint> {
        let id = self.joint_ids.get(handle.0)?;
        let joint = self.joint_graph.graph.edge_weight_mut(*id);
        if wake_up_connected_bodies {
            if let Some(joint) = &joint {
                self.to_wake_up.insert(joint.body1);
                self.to_wake_up.insert(joint.body2);
            }
        }
        joint
    }

    /// Gets a joint by index without knowing the generation (advanced/unsafe).
    ///
    /// ⚠️ **Prefer `get()` instead!** This bypasses generation checks.
    /// See [`RigidBodySet::get_unknown_gen`] for details on the ABA problem.
    pub fn get_unknown_gen(&self, i: u32) -> Option<(&ImpulseJoint, ImpulseJointHandle)> {
        let (id, handle) = self.joint_ids.get_unknown_gen(i)?;
        Some((
            self.joint_graph.graph.edge_weight(*id)?,
            ImpulseJointHandle(handle),
        ))
    }

    /// Gets a mutable joint by index without knowing the generation (advanced/unsafe).
    ///
    /// ⚠️ **Prefer `get_mut()` instead!** This bypasses generation checks.
    pub fn get_unknown_gen_mut(
        &mut self,
        i: u32,
    ) -> Option<(&mut ImpulseJoint, ImpulseJointHandle)> {
        let (id, handle) = self.joint_ids.get_unknown_gen(i)?;
        Some((
            self.joint_graph.graph.edge_weight_mut(*id)?,
            ImpulseJointHandle(handle),
        ))
    }

    /// Iterates over all joints in this collection.
    ///
    /// Each iteration yields `(joint_handle, &joint)`.
    pub fn iter(&self) -> impl Iterator<Item = (ImpulseJointHandle, &ImpulseJoint)> {
        self.joint_graph
            .graph
            .edges
            .iter()
            .map(|e| (e.weight.handle, &e.weight))
    }

    /// Iterates over all joints with mutable access.
    ///
    /// Each iteration yields `(joint_handle, &mut joint)`.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (ImpulseJointHandle, &mut ImpulseJoint)> {
        self.joint_graph
            .graph
            .edges
            .iter_mut()
            .map(|e| (e.weight.handle, &mut e.weight))
    }

    // /// The set of impulse_joints as an array.
    // pub(crate) fn impulse_joints(&self) -> &[JointGraphEdge] {
    //     // self.joint_graph
    //     //     .graph
    //     //     .edges
    //     //     .iter_mut()
    //     //     .map(|e| &mut e.weight)
    // }

    // #[cfg(not(feature = "parallel"))]
    #[allow(dead_code)] // That will likely be useful when we re-introduce intra-island parallelism.
    pub(crate) fn joints_mut(&mut self) -> &mut [JointGraphEdge] {
        &mut self.joint_graph.graph.edges[..]
    }

    #[cfg(feature = "parallel")]
    pub(crate) fn joints_vec_mut(&mut self) -> &mut Vec<JointGraphEdge> {
        &mut self.joint_graph.graph.edges
    }

    /// Adds a joint connecting two bodies and returns its handle.
    ///
    /// The joint constrains how the two bodies can move relative to each other.
    ///
    /// # Parameters
    /// * `body1`, `body2` - The two bodies to connect
    /// * `data` - The joint configuration (FixedJoint, RevoluteJoint, etc.)
    /// * `wake_up` - If `true`, wakes up both bodies
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let mut joints = ImpulseJointSet::new();
    /// # let body1 = bodies.insert(RigidBodyBuilder::dynamic());
    /// # let body2 = bodies.insert(RigidBodyBuilder::dynamic());
    /// let joint = RevoluteJointBuilder::new(Vector::Y)
    ///     .local_anchor1(Vector::new(1.0, 0.0, 0.0))
    ///     .local_anchor2(Vector::new(-1.0, 0.0, 0.0))
    ///     .build();
    /// let handle = joints.insert(body1, body2, joint, true);
    /// ```
    pub fn insert(
        &mut self,
        body1: RigidBodyHandle,
        body2: RigidBodyHandle,
        data: impl Into<GenericJoint>,
        wake_up: bool,
    ) -> ImpulseJointHandle {
        let data = data.into();
        let handle = self.joint_ids.insert(0.into());
        let joint = ImpulseJoint {
            body1,
            body2,
            data,
            impulses: Default::default(),
            handle: ImpulseJointHandle(handle),
        };

        let default_id = InteractionGraph::<(), ()>::invalid_graph_index();
        let mut graph_index1 = *self
            .rb_graph_ids
            .ensure_element_exist(joint.body1.0, default_id);
        let mut graph_index2 = *self
            .rb_graph_ids
            .ensure_element_exist(joint.body2.0, default_id);

        // NOTE: the body won't have a graph index if it does not
        // have any joint attached.
        if !InteractionGraph::<RigidBodyHandle, ImpulseJoint>::is_graph_index_valid(graph_index1) {
            graph_index1 = self.joint_graph.graph.add_node(joint.body1);
            self.rb_graph_ids.insert(joint.body1.0, graph_index1);
        }

        if !InteractionGraph::<RigidBodyHandle, ImpulseJoint>::is_graph_index_valid(graph_index2) {
            graph_index2 = self.joint_graph.graph.add_node(joint.body2);
            self.rb_graph_ids.insert(joint.body2.0, graph_index2);
        }

        self.joint_ids[handle] = self.joint_graph.add_edge(graph_index1, graph_index2, joint);

        if wake_up {
            self.to_wake_up.insert(body1);
            self.to_wake_up.insert(body2);
        }

        self.to_join.insert((body1, body2));

        ImpulseJointHandle(handle)
    }

    /// Retrieve all the enabled impulse joints happening between two active bodies.
    // NOTE: this is very similar to the code from NarrowPhase::select_active_interactions.
    pub(crate) fn select_active_interactions(
        &self,
        islands: &IslandManager,
        bodies: &RigidBodySet,
        out: &mut [Vec<JointIndex>],
    ) {
        for out_island in &mut out[..islands.active_islands().len()] {
            out_island.clear();
        }

        // FIXME: don't iterate through all the interactions.
        for (i, edge) in self.joint_graph.graph.edges.iter().enumerate() {
            let joint = &edge.weight;
            let rb1 = &bodies[joint.body1];
            let rb2 = &bodies[joint.body2];

            if joint.data.is_enabled()
                && (rb1.is_dynamic_or_kinematic() || rb2.is_dynamic_or_kinematic())
                && (!rb1.is_dynamic_or_kinematic() || !rb1.is_sleeping())
                && (!rb2.is_dynamic_or_kinematic() || !rb2.is_sleeping())
            {
                let island_awake_index = if !rb1.is_dynamic_or_kinematic() {
                    islands.islands[rb2.ids.active_island_id]
                        .id_in_awake_list()
                        .expect("Internal error: island should be awake.")
                } else {
                    islands.islands[rb1.ids.active_island_id]
                        .id_in_awake_list()
                        .expect("Internal error: island should be awake.")
                };

                out[island_awake_index].push(i);
            }
        }
    }

    /// Removes a joint from the world.
    ///
    /// Returns the removed joint if it existed, or `None` if the handle was invalid.
    ///
    /// # Parameters
    /// * `wake_up` - If `true`, wakes up both bodies that were connected by this joint
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let mut joints = ImpulseJointSet::new();
    /// # let body1 = bodies.insert(RigidBodyBuilder::dynamic());
    /// # let body2 = bodies.insert(RigidBodyBuilder::dynamic());
    /// # let joint = RevoluteJointBuilder::new(Vector::Y).build();
    /// # let joint_handle = joints.insert(body1, body2, joint, true);
    /// if let Some(joint) = joints.remove(joint_handle, true) {
    ///     println!("Removed joint between {:?} and {:?}", joint.body1, joint.body2);
    /// }
    /// ```
    pub fn remove(&mut self, handle: ImpulseJointHandle, wake_up: bool) -> Option<ImpulseJoint> {
        let id = self.joint_ids.remove(handle.0)?;
        let endpoints = self.joint_graph.graph.edge_endpoints(id)?;

        if wake_up {
            if let Some(rb_handle) = self.joint_graph.graph.node_weight(endpoints.0) {
                self.to_wake_up.insert(*rb_handle);
            }
            if let Some(rb_handle) = self.joint_graph.graph.node_weight(endpoints.1) {
                self.to_wake_up.insert(*rb_handle);
            }
        }

        let removed_joint = self.joint_graph.graph.remove_edge(id);

        if let Some(edge) = self.joint_graph.graph.edge_weight(id) {
            self.joint_ids[edge.handle.0] = id;
        }

        removed_joint
    }

    /// Deletes all the impulse_joints attached to the given rigid-body.
    ///
    /// The provided rigid-body handle is not required to identify a rigid-body that
    /// is still contained by the `bodies` component set.
    /// Returns the (now invalid) handles of the removed impulse_joints.
    pub fn remove_joints_attached_to_rigid_body(
        &mut self,
        handle: RigidBodyHandle,
    ) -> Vec<ImpulseJointHandle> {
        let mut deleted = vec![];

        if let Some(deleted_id) = self
            .rb_graph_ids
            .remove(handle.0, InteractionGraph::<(), ()>::invalid_graph_index())
        {
            if InteractionGraph::<(), ()>::is_graph_index_valid(deleted_id) {
                // We have to delete each joint one by one in order to:
                // - Wake-up the attached bodies.
                // - Update our Handle -> graph edge mapping.
                // Delete the node.
                let to_delete: Vec<_> = self
                    .joint_graph
                    .interactions_with(deleted_id)
                    .map(|e| (e.0, e.1, e.2.handle))
                    .collect();
                for (h1, h2, to_delete_handle) in to_delete {
                    deleted.push(to_delete_handle);
                    let to_delete_edge_id = self.joint_ids.remove(to_delete_handle.0).unwrap();
                    self.joint_graph.graph.remove_edge(to_delete_edge_id);

                    // Update the id of the edge which took the place of the deleted one.
                    if let Some(j) = self.joint_graph.graph.edge_weight_mut(to_delete_edge_id) {
                        self.joint_ids[j.handle.0] = to_delete_edge_id;
                    }

                    // Wake up the attached bodies.
                    self.to_wake_up.insert(h1);
                    self.to_wake_up.insert(h2);
                }

                if let Some(other) = self.joint_graph.remove_node(deleted_id) {
                    // One rigid-body joint graph index may have been invalidated
                    // so we need to update it.
                    self.rb_graph_ids.insert(other.0, deleted_id);
                }
            }
        }

        deleted
    }
}
