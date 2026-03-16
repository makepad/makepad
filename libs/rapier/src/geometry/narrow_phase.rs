#[cfg(feature = "parallel")]
use rayon::prelude::*;

use crate::data::Coarena;
use crate::data::graph::EdgeIndex;
use crate::dynamics::{
    CoefficientCombineRule, ImpulseJointSet, IslandManager, RigidBodyDominance, RigidBodySet,
    RigidBodyType,
};
use crate::geometry::{
    BoundingVolume, BroadPhasePairEvent, ColliderChanges, ColliderGraphIndex, ColliderHandle,
    ColliderPair, ColliderSet, CollisionEvent, ContactData, ContactManifold, ContactManifoldData,
    ContactPair, InteractionGraph, IntersectionPair, SolverContact, SolverFlags,
    TemporaryInteractionIndex,
};
use crate::math::{MAX_MANIFOLD_POINTS, Real};
use crate::pipeline::{
    ActiveEvents, ActiveHooks, ContactModificationContext, EventHandler, PairFilterContext,
    PhysicsHooks,
};
use crate::prelude::{CollisionEventFlags, MultibodyJointSet};
use parry::query::{DefaultQueryDispatcher, PersistentQueryDispatcher};
use parry::utils::PoseOpt;
use parry::utils::hashmap::HashMap;
use std::sync::Arc;

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
struct ColliderGraphIndices {
    contact_graph_index: ColliderGraphIndex,
    intersection_graph_index: ColliderGraphIndex,
}

impl ColliderGraphIndices {
    fn invalid() -> Self {
        Self {
            contact_graph_index: InteractionGraph::<(), ()>::invalid_graph_index(),
            intersection_graph_index: InteractionGraph::<(), ()>::invalid_graph_index(),
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum PairRemovalMode {
    FromContactGraph,
    FromIntersectionGraph,
    Auto,
}

/// The narrow-phase collision detector that computes precise contact points between colliders.
///
/// After the broad-phase quickly filters out distant object pairs, the narrow-phase performs
/// detailed geometric computations to find exact:
/// - Contact points (where surfaces touch)
/// - Contact normals (which direction surfaces face)
/// - Penetration depths (how much objects overlap)
///
/// You typically don't interact with this directly - it's managed by [`PhysicsPipeline::step`](crate::pipeline::PhysicsPipeline::step).
/// However, you can access it to query contact information or intersection state between specific colliders.
///
/// **For spatial queries** (raycasts, shape casts), use [`QueryPipeline`](crate::pipeline::QueryPipeline) instead.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone)]
pub struct NarrowPhase {
    #[cfg_attr(
        feature = "serde-serialize",
        serde(skip, default = "crate::geometry::default_persistent_query_dispatcher")
    )]
    query_dispatcher: Arc<dyn PersistentQueryDispatcher<ContactManifoldData, ContactData>>,
    contact_graph: InteractionGraph<ColliderHandle, ContactPair>,
    intersection_graph: InteractionGraph<ColliderHandle, IntersectionPair>,
    graph_indices: Coarena<ColliderGraphIndices>,
}

pub(crate) type ContactManifoldIndex = usize;

impl Default for NarrowPhase {
    fn default() -> Self {
        Self::new()
    }
}

impl NarrowPhase {
    /// Creates a new empty narrow-phase.
    pub fn new() -> Self {
        Self::with_query_dispatcher(DefaultQueryDispatcher)
    }

    /// Creates a new empty narrow-phase with a custom query dispatcher.
    pub fn with_query_dispatcher<D>(d: D) -> Self
    where
        D: 'static + PersistentQueryDispatcher<ContactManifoldData, ContactData>,
    {
        Self {
            query_dispatcher: Arc::new(d),
            contact_graph: InteractionGraph::new(),
            intersection_graph: InteractionGraph::new(),
            graph_indices: Coarena::new(),
        }
    }

    /// The query dispatcher used by this narrow-phase to select the right collision-detection
    /// algorithms depending on the shape types.
    pub fn query_dispatcher(
        &self,
    ) -> &dyn PersistentQueryDispatcher<ContactManifoldData, ContactData> {
        &*self.query_dispatcher
    }

    /// The contact graph containing all contact pairs and their contact information.
    pub fn contact_graph(&self) -> &InteractionGraph<ColliderHandle, ContactPair> {
        &self.contact_graph
    }

    /// The intersection graph containing all intersection pairs and their intersection information.
    pub fn intersection_graph(&self) -> &InteractionGraph<ColliderHandle, IntersectionPair> {
        &self.intersection_graph
    }

    /// All the contacts involving the given collider.
    ///
    /// It is strongly recommended to use the [`NarrowPhase::contact_pairs_with`] method instead. This
    /// method can be used if the generation number of the collider handle isn't known.
    pub fn contact_pairs_with_unknown_gen(
        &self,
        collider: u32,
    ) -> impl Iterator<Item = &ContactPair> {
        self.graph_indices
            .get_unknown_gen(collider)
            .map(|id| id.contact_graph_index)
            .into_iter()
            .flat_map(move |id| self.contact_graph.interactions_with(id))
            .map(|pair| pair.2)
    }

    /// All the contact pairs involving the given collider.
    ///
    /// The returned contact pairs identify pairs of colliders with intersecting bounding-volumes.
    /// To check if any geometric contact happened between the collider shapes, check
    /// [`ContactPair::has_any_active_contact`].
    pub fn contact_pairs_with(
        &self,
        collider: ColliderHandle,
    ) -> impl Iterator<Item = &ContactPair> {
        self.graph_indices
            .get(collider.0)
            .map(|id| id.contact_graph_index)
            .into_iter()
            .flat_map(move |id| self.contact_graph.interactions_with(id))
            .map(|pair| pair.2)
    }

    /// All the intersection pairs involving the given collider.
    ///
    /// It is strongly recommended to use the [`NarrowPhase::intersection_pairs_with`]  method instead.
    /// This method can be used if the generation number of the collider handle isn't known.
    pub fn intersection_pairs_with_unknown_gen(
        &self,
        collider: u32,
    ) -> impl Iterator<Item = (ColliderHandle, ColliderHandle, bool)> + '_ {
        self.graph_indices
            .get_unknown_gen(collider)
            .map(|id| id.intersection_graph_index)
            .into_iter()
            .flat_map(move |id| {
                self.intersection_graph
                    .interactions_with(id)
                    .map(|e| (e.0, e.1, e.2.intersecting))
            })
    }

    /// All the intersection pairs involving the given collider, where at least one collider
    /// involved in the intersection is a sensor.
    ///
    /// The returned contact pairs identify pairs of colliders (where at least one is a sensor) with
    /// intersecting bounding-volumes. To check if any geometric overlap happened between the collider shapes, check
    /// the returned boolean.
    pub fn intersection_pairs_with(
        &self,
        collider: ColliderHandle,
    ) -> impl Iterator<Item = (ColliderHandle, ColliderHandle, bool)> + '_ {
        self.graph_indices
            .get(collider.0)
            .map(|id| id.intersection_graph_index)
            .into_iter()
            .flat_map(move |id| {
                self.intersection_graph
                    .interactions_with(id)
                    .map(|e| (e.0, e.1, e.2.intersecting))
            })
    }

    /// Returns the contact pair at the given temporary index.
    pub fn contact_pair_at_index(&self, id: TemporaryInteractionIndex) -> &ContactPair {
        &self.contact_graph.graph.edges[id.index()].weight
    }

    /// The contact pair involving two specific colliders.
    ///
    /// It is strongly recommended to use the [`NarrowPhase::contact_pair`] method instead. This
    /// method can be used if the generation number of the collider handle isn't known.
    ///
    /// If this returns `None`, there is no contact between the two colliders.
    /// If this returns `Some`, then there may be a contact between the two colliders. Check the
    /// result [`ContactPair::has_any_active_contact`] method to see if there is an actual contact.
    pub fn contact_pair_unknown_gen(&self, collider1: u32, collider2: u32) -> Option<&ContactPair> {
        let id1 = self.graph_indices.get_unknown_gen(collider1)?;
        let id2 = self.graph_indices.get_unknown_gen(collider2)?;
        self.contact_graph
            .interaction_pair(id1.contact_graph_index, id2.contact_graph_index)
            .map(|c| c.2)
    }

    /// The contact pair involving two specific colliders.
    ///
    /// If this returns `None`, there is no contact between the two colliders.
    /// If this returns `Some`, then there may be a contact between the two colliders. Check the
    /// result [`ContactPair::has_any_active_contact`] method to see if there is an actual contact.
    pub fn contact_pair(
        &self,
        collider1: ColliderHandle,
        collider2: ColliderHandle,
    ) -> Option<&ContactPair> {
        let id1 = self.graph_indices.get(collider1.0)?;
        let id2 = self.graph_indices.get(collider2.0)?;
        self.contact_graph
            .interaction_pair(id1.contact_graph_index, id2.contact_graph_index)
            .map(|c| c.2)
    }

    /// The intersection pair involving two specific colliders.
    ///
    /// It is strongly recommended to use the [`NarrowPhase::intersection_pair`] method instead. This
    /// method can be used if the generation number of the collider handle isn't known.
    ///
    /// If this returns `None` or `Some(false)`, then there is no intersection between the two colliders.
    /// If this returns `Some(true)`, then there may be an intersection between the two colliders.
    pub fn intersection_pair_unknown_gen(&self, collider1: u32, collider2: u32) -> Option<bool> {
        let id1 = self.graph_indices.get_unknown_gen(collider1)?;
        let id2 = self.graph_indices.get_unknown_gen(collider2)?;
        self.intersection_graph
            .interaction_pair(id1.intersection_graph_index, id2.intersection_graph_index)
            .map(|c| c.2.intersecting)
    }

    /// The intersection pair involving two specific colliders.
    ///
    /// If this returns `None` or `Some(false)`, then there is no intersection between the two colliders.
    /// If this returns `Some(true)`, then there may be an intersection between the two colliders.
    pub fn intersection_pair(
        &self,
        collider1: ColliderHandle,
        collider2: ColliderHandle,
    ) -> Option<bool> {
        let id1 = self.graph_indices.get(collider1.0)?;
        let id2 = self.graph_indices.get(collider2.0)?;
        self.intersection_graph
            .interaction_pair(id1.intersection_graph_index, id2.intersection_graph_index)
            .map(|c| c.2.intersecting)
    }

    /// All the contact pairs maintained by this narrow-phase.
    pub fn contact_pairs(&self) -> impl Iterator<Item = &ContactPair> {
        self.contact_graph.interactions()
    }

    /// All the intersection pairs maintained by this narrow-phase.
    pub fn intersection_pairs(
        &self,
    ) -> impl Iterator<Item = (ColliderHandle, ColliderHandle, bool)> + '_ {
        self.intersection_graph
            .interactions_with_endpoints()
            .map(|e| (e.0, e.1, e.2.intersecting))
    }

    // #[cfg(feature = "parallel")]
    // pub(crate) fn contact_pairs_vec_mut(&mut self) -> &mut Vec<ContactPair> {
    //     &mut self.contact_graph.interactions
    // }

    /// Maintain the narrow-phase internal state by taking collider removal into account.
    pub fn handle_user_changes(
        &mut self,
        mut islands: Option<&mut IslandManager>,
        modified_colliders: &[ColliderHandle],
        removed_colliders: &[ColliderHandle],
        colliders: &mut ColliderSet,
        bodies: &mut RigidBodySet,
        events: &dyn EventHandler,
    ) {
        // TODO: avoid these hash-maps.
        // They are necessary to handle the swap-remove done internally
        // by the contact/intersection graphs when a node is removed.
        let mut prox_id_remap = HashMap::default();
        let mut contact_id_remap = HashMap::default();

        for collider in removed_colliders {
            // NOTE: if the collider does not have any graph indices currently, there is nothing
            // to remove in the narrow-phase for this collider.
            if let Some(graph_idx) = self
                .graph_indices
                .remove(collider.0, ColliderGraphIndices::invalid())
            {
                let intersection_graph_id = prox_id_remap
                    .get(collider)
                    .copied()
                    .unwrap_or(graph_idx.intersection_graph_index);
                let contact_graph_id = contact_id_remap
                    .get(collider)
                    .copied()
                    .unwrap_or(graph_idx.contact_graph_index);

                self.remove_collider(
                    intersection_graph_id,
                    contact_graph_id,
                    islands.as_deref_mut(),
                    colliders,
                    bodies,
                    &mut prox_id_remap,
                    &mut contact_id_remap,
                    events,
                );
            }
        }

        self.handle_user_changes_on_colliders(
            islands,
            modified_colliders,
            colliders,
            bodies,
            events,
        );
    }
    pub(crate) fn remove_collider(
        &mut self,
        intersection_graph_id: ColliderGraphIndex,
        contact_graph_id: ColliderGraphIndex,
        islands: Option<&mut IslandManager>,
        colliders: &mut ColliderSet,
        bodies: &mut RigidBodySet,
        prox_id_remap: &mut HashMap<ColliderHandle, ColliderGraphIndex>,
        contact_id_remap: &mut HashMap<ColliderHandle, ColliderGraphIndex>,
        events: &dyn EventHandler,
    ) {
        // Wake up every body in contact with the deleted collider and generate Stopped collision events.
        if let Some(islands) = islands {
            for (a, b, pair) in self.contact_graph.interactions_with(contact_graph_id) {
                if let Some(parent) = colliders.get(a).and_then(|c| c.parent.as_ref()) {
                    islands.wake_up(bodies, parent.handle, true)
                }

                if let Some(parent) = colliders.get(b).and_then(|c| c.parent.as_ref()) {
                    islands.wake_up(bodies, parent.handle, true)
                }

                if pair.start_event_emitted {
                    events.handle_collision_event(
                        bodies,
                        colliders,
                        CollisionEvent::Stopped(a, b, CollisionEventFlags::REMOVED),
                        Some(pair),
                    );
                }
            }
        } else {
            // If there is no island, don’t wake-up bodies, but do send the Stopped collision event.
            for (a, b, pair) in self.contact_graph.interactions_with(contact_graph_id) {
                if pair.start_event_emitted {
                    events.handle_collision_event(
                        bodies,
                        colliders,
                        CollisionEvent::Stopped(a, b, CollisionEventFlags::REMOVED),
                        Some(pair),
                    );
                }
            }
        }

        // Generate Stopped collision events for intersections.
        for (a, b, pair) in self
            .intersection_graph
            .interactions_with(intersection_graph_id)
        {
            if pair.start_event_emitted {
                events.handle_collision_event(
                    bodies,
                    colliders,
                    CollisionEvent::Stopped(
                        a,
                        b,
                        CollisionEventFlags::REMOVED | CollisionEventFlags::SENSOR,
                    ),
                    None,
                );
            }
        }

        // We have to manage the fact that one other collider will
        // have its graph index changed because of the node's swap-remove.
        if let Some(replacement) = self.intersection_graph.remove_node(intersection_graph_id) {
            if let Some(replacement) = self.graph_indices.get_mut(replacement.0) {
                replacement.intersection_graph_index = intersection_graph_id;
            } else {
                prox_id_remap.insert(replacement, intersection_graph_id);
                // I feel like this should never happen now that the narrow-phase is the one owning
                // the graph_indices. Let's put an unreachable in there and see if anybody still manages
                // to reach it. If nobody does, we will remove this.
                unreachable!();
            }
        }

        if let Some(replacement) = self.contact_graph.remove_node(contact_graph_id) {
            if let Some(replacement) = self.graph_indices.get_mut(replacement.0) {
                replacement.contact_graph_index = contact_graph_id;
            } else {
                contact_id_remap.insert(replacement, contact_graph_id);
                // I feel like this should never happen now that the narrow-phase is the one owning
                // the graph_indices. Let's put an unreachable in there and see if anybody still manages
                // to reach it. If nobody does, we will remove this.
                unreachable!();
            }
        }
    }
    pub(crate) fn handle_user_changes_on_colliders(
        &mut self,
        mut islands: Option<&mut IslandManager>,
        modified_colliders: &[ColliderHandle],
        colliders: &ColliderSet,
        bodies: &mut RigidBodySet,
        events: &dyn EventHandler,
    ) {
        let mut pairs_to_remove = vec![];

        for handle in modified_colliders {
            // NOTE: we use `get` because the collider may no longer
            //       exist if it has been removed.
            if let Some(co) = colliders.get(*handle) {
                if !co.changes.needs_narrow_phase_update() {
                    // No flag relevant to the narrow-phase is enabled for this collider.
                    continue;
                }

                if let Some(gid) = self.graph_indices.get(handle.0) {
                    // For each modified colliders, we need to wake-up the bodies it is in contact with
                    // so that the narrow-phase properly takes into account the change in, e.g.,
                    // collision groups. Waking up the modified collider's parent isn't enough because
                    // it could be a fixed or kinematic body which don't propagate the wake-up state.
                    if let Some(islands) = islands.as_deref_mut() {
                        if let Some(co_parent) = &co.parent {
                            islands.wake_up(bodies, co_parent.handle, true);
                        }

                        for inter in self
                            .contact_graph
                            .interactions_with(gid.contact_graph_index)
                        {
                            let other_handle = if *handle == inter.0 { inter.1 } else { inter.0 };
                            let other_parent = colliders
                                .get(other_handle)
                                .and_then(|co| co.parent.as_ref());

                            if let Some(other_parent) = other_parent {
                                islands.wake_up(bodies, other_parent.handle, true);
                            }
                        }
                    }

                    // For each collider which had their sensor status modified, we need
                    // to transfer their contact/intersection graph edges to the intersection/contact graph.
                    // To achieve this we will remove the relevant contact/intersection pairs form the
                    // contact/intersection graphs, and then add them into the other graph.
                    if co.changes.intersects(ColliderChanges::TYPE) {
                        if co.is_sensor() {
                            // Find the contact pairs for this collider and
                            // push them to `pairs_to_remove`.
                            for inter in self
                                .contact_graph
                                .interactions_with(gid.contact_graph_index)
                            {
                                pairs_to_remove.push((
                                    ColliderPair::new(inter.0, inter.1),
                                    PairRemovalMode::FromContactGraph,
                                ));
                            }
                        } else {
                            // Find the contact pairs for this collider and
                            // push them to `pairs_to_remove` if both involved
                            // colliders are not sensors.
                            for inter in self
                                .intersection_graph
                                .interactions_with(gid.intersection_graph_index)
                                .filter(|(h1, h2, _)| {
                                    !colliders[*h1].is_sensor() && !colliders[*h2].is_sensor()
                                })
                            {
                                pairs_to_remove.push((
                                    ColliderPair::new(inter.0, inter.1),
                                    PairRemovalMode::FromIntersectionGraph,
                                ));
                            }
                        }
                    }

                    // NOTE: if a collider only changed parent, we don’t need to remove it from any
                    //       of the graphs as re-parenting doesn’t change the sensor status of a
                    //       collider. If needed, their collision/intersection data will be
                    //       updated/removed automatically in the contact or intersection update
                    //       functions.
                }
            }
        }

        // Remove the pair from the relevant graph.
        for pair in &pairs_to_remove {
            self.remove_pair(
                islands.as_deref_mut(),
                colliders,
                bodies,
                &pair.0,
                events,
                pair.1,
            );
        }

        // Add the removed pair to the relevant graph.
        for pair in pairs_to_remove {
            self.add_pair(colliders, &pair.0);
        }
    }
    fn remove_pair(
        &mut self,
        islands: Option<&mut IslandManager>,
        colliders: &ColliderSet,
        bodies: &mut RigidBodySet,
        pair: &ColliderPair,
        events: &dyn EventHandler,
        mode: PairRemovalMode,
    ) {
        if let (Some(co1), Some(co2)) =
            (colliders.get(pair.collider1), colliders.get(pair.collider2))
        {
            // TODO: could we just unwrap here?
            // Don't we have the guarantee that we will get a `AddPair` before a `DeletePair`?
            if let (Some(gid1), Some(gid2)) = (
                self.graph_indices.get(pair.collider1.0),
                self.graph_indices.get(pair.collider2.0),
            ) {
                if mode == PairRemovalMode::FromIntersectionGraph
                    || (mode == PairRemovalMode::Auto && (co1.is_sensor() || co2.is_sensor()))
                {
                    let intersection = self
                        .intersection_graph
                        .remove_edge(gid1.intersection_graph_index, gid2.intersection_graph_index);

                    // Emit an intersection lost event if we had an intersection before removing the edge.
                    if let Some(mut intersection) = intersection {
                        if intersection.intersecting
                            && (co1.flags.active_events | co2.flags.active_events)
                                .contains(ActiveEvents::COLLISION_EVENTS)
                        {
                            intersection.emit_stop_event(
                                bodies,
                                colliders,
                                pair.collider1,
                                pair.collider2,
                                events,
                            )
                        }
                    }
                } else {
                    let contact_pair = self
                        .contact_graph
                        .remove_edge(gid1.contact_graph_index, gid2.contact_graph_index);

                    // Emit a contact stopped event if we had a contact before removing the edge.
                    // Also wake up the dynamic bodies that were in contact.
                    if let Some(mut ctct) = contact_pair {
                        if ctct.has_any_active_contact() {
                            if let Some(islands) = islands {
                                if let Some(co_parent1) = &co1.parent {
                                    islands.wake_up(bodies, co_parent1.handle, true);
                                }

                                if let Some(co_parent2) = co2.parent {
                                    islands.wake_up(bodies, co_parent2.handle, true);
                                }
                            }

                            if (co1.flags.active_events | co2.flags.active_events)
                                .contains(ActiveEvents::COLLISION_EVENTS)
                            {
                                ctct.emit_stop_event(bodies, colliders, events);
                            }
                        }
                    }
                }
            }
        }
    }
    fn add_pair(&mut self, colliders: &ColliderSet, pair: &ColliderPair) {
        if let (Some(co1), Some(co2)) =
            (colliders.get(pair.collider1), colliders.get(pair.collider2))
        {
            // These colliders have no parents - continue.

            let (gid1, gid2) = self.graph_indices.ensure_pair_exists(
                pair.collider1.0,
                pair.collider2.0,
                ColliderGraphIndices::invalid(),
            );

            if co1.is_sensor() || co2.is_sensor() {
                // NOTE: the collider won't have a graph index as long
                // as it does not interact with anything.
                if !InteractionGraph::<(), ()>::is_graph_index_valid(gid1.intersection_graph_index)
                {
                    gid1.intersection_graph_index =
                        self.intersection_graph.graph.add_node(pair.collider1);
                }

                if !InteractionGraph::<(), ()>::is_graph_index_valid(gid2.intersection_graph_index)
                {
                    gid2.intersection_graph_index =
                        self.intersection_graph.graph.add_node(pair.collider2);
                }

                if self
                    .intersection_graph
                    .graph
                    .find_edge(gid1.intersection_graph_index, gid2.intersection_graph_index)
                    .is_none()
                {
                    let _ = self.intersection_graph.add_edge(
                        gid1.intersection_graph_index,
                        gid2.intersection_graph_index,
                        IntersectionPair::new(),
                    );
                }
            } else {
                // NOTE: same code as above, but for the contact graph.
                // TODO: refactor both pieces of code somehow?

                // NOTE: the collider won't have a graph index as long
                // as it does not interact with anything.
                if !InteractionGraph::<(), ()>::is_graph_index_valid(gid1.contact_graph_index) {
                    gid1.contact_graph_index = self.contact_graph.graph.add_node(pair.collider1);
                }

                if !InteractionGraph::<(), ()>::is_graph_index_valid(gid2.contact_graph_index) {
                    gid2.contact_graph_index = self.contact_graph.graph.add_node(pair.collider2);
                }

                if self
                    .contact_graph
                    .graph
                    .find_edge(gid1.contact_graph_index, gid2.contact_graph_index)
                    .is_none()
                {
                    let interaction = ContactPair::new(pair.collider1, pair.collider2);
                    let _ = self.contact_graph.add_edge(
                        gid1.contact_graph_index,
                        gid2.contact_graph_index,
                        interaction,
                    );
                }
            }
        }
    }

    pub(crate) fn register_pairs(
        &mut self,
        mut islands: Option<&mut IslandManager>,
        colliders: &ColliderSet,
        bodies: &mut RigidBodySet,
        broad_phase_events: &[BroadPhasePairEvent],
        events: &dyn EventHandler,
    ) {
        for event in broad_phase_events {
            match event {
                BroadPhasePairEvent::AddPair(pair) => {
                    self.add_pair(colliders, pair);
                }
                BroadPhasePairEvent::DeletePair(pair) => {
                    self.remove_pair(
                        islands.as_deref_mut(),
                        colliders,
                        bodies,
                        pair,
                        events,
                        PairRemovalMode::Auto,
                    );
                }
            }
        }
    }
    pub(crate) fn compute_intersections(
        &mut self,
        bodies: &RigidBodySet,
        colliders: &ColliderSet,
        hooks: &dyn PhysicsHooks,
        events: &dyn EventHandler,
    ) {
        let nodes = &self.intersection_graph.graph.nodes;
        let query_dispatcher = &*self.query_dispatcher;

        // TODO: don't iterate on all the edges.
        par_iter_mut!(&mut self.intersection_graph.graph.edges).for_each(|edge| {
            let handle1 = nodes[edge.source().index()].weight;
            let handle2 = nodes[edge.target().index()].weight;
            let had_intersection = edge.weight.intersecting;
            let co1 = &colliders[handle1];
            let co2 = &colliders[handle2];
            let rb_handle1 = co1.parent.map(|p| p.handle);
            let rb_handle2 = co2.parent.map(|p| p.handle);

            'emit_events: {
                if !co1.changes.needs_narrow_phase_update()
                    && !co2.changes.needs_narrow_phase_update()
                {
                    // No update needed for these colliders.
                    return;
                }

                if rb_handle1 == rb_handle2 && co1.parent.is_some() {
                    // Same parents. Ignore collisions.
                    edge.weight.intersecting = false;
                    break 'emit_events;
                }
                // TODO: avoid lookup into bodies.
                let mut rb_type1 = RigidBodyType::Fixed;
                let mut rb_type2 = RigidBodyType::Fixed;

                if let Some(co_parent1) = &co1.parent {
                    rb_type1 = bodies[co_parent1.handle].body_type;
                }

                if let Some(co_parent2) = &co2.parent {
                    rb_type2 = bodies[co_parent2.handle].body_type;
                }

                // Filter based on the rigid-body types.
                if !co1.flags.active_collision_types.test(rb_type1, rb_type2)
                    && !co2.flags.active_collision_types.test(rb_type1, rb_type2)
                {
                    edge.weight.intersecting = false;
                    break 'emit_events;
                }

                // Filter based on collision groups.
                if !co1.flags.collision_groups.test(co2.flags.collision_groups) {
                    edge.weight.intersecting = false;
                    break 'emit_events;
                }

                let active_hooks = co1.flags.active_hooks | co2.flags.active_hooks;

                if active_hooks.contains(ActiveHooks::FILTER_INTERSECTION_PAIR) {
                    let context = PairFilterContext {
                        bodies,
                        colliders,
                        rigid_body1: rb_handle1,
                        rigid_body2: rb_handle2,
                        collider1: handle1,
                        collider2: handle2,
                    };

                    if !hooks.filter_intersection_pair(&context) {
                        // No intersection allowed.
                        edge.weight.intersecting = false;
                        break 'emit_events;
                    }
                }

                let pos12 = co1.pos.inv_mul(&co2.pos);
                edge.weight.intersecting = query_dispatcher
                    .intersection_test(&pos12, &*co1.shape, &*co2.shape)
                    .unwrap_or(false);
            }

            let active_events = co1.flags.active_events | co2.flags.active_events;

            if active_events.contains(ActiveEvents::COLLISION_EVENTS)
                && had_intersection != edge.weight.intersecting
            {
                if edge.weight.intersecting {
                    edge.weight
                        .emit_start_event(bodies, colliders, handle1, handle2, events);
                } else {
                    edge.weight
                        .emit_stop_event(bodies, colliders, handle1, handle2, events);
                }
            }
        });
    }
    pub(crate) fn compute_contacts(
        &mut self,
        prediction_distance: Real,
        dt: Real,
        islands: &mut IslandManager,
        bodies: &mut RigidBodySet,
        colliders: &ColliderSet,
        impulse_joints: &ImpulseJointSet,
        multibody_joints: &MultibodyJointSet,
        hooks: &dyn PhysicsHooks,
        events: &dyn EventHandler,
    ) {
        let query_dispatcher = &*self.query_dispatcher;
        #[cfg(feature = "parallel")]
        let (snd, rcv) = std::sync::mpsc::channel();

        // TODO PERF: don't iterate on all the edges.
        par_iter_mut!(&mut self.contact_graph.graph.edges).for_each(|edge| {
            let pair = &mut edge.weight;
            let had_any_active_contact = pair.has_any_active_contact();
            let co1 = &colliders[pair.collider1];
            let co2 = &colliders[pair.collider2];
            let rb_handle1 = co1.parent.map(|p| p.handle);
            let rb_handle2 = co2.parent.map(|p| p.handle);

            'emit_events: {
                if !co1.changes.needs_narrow_phase_update()
                    && !co2.changes.needs_narrow_phase_update()
                {
                    // No update needed for these colliders.
                    return;
                }

                if rb_handle1 == rb_handle2 && co1.parent.is_some() {
                    // Same parents. Ignore collisions.
                    pair.clear();
                    break 'emit_events;
                }

                let rb1 = co1.parent.map(|co_parent1| &bodies[co_parent1.handle]);
                let rb2 = co2.parent.map(|co_parent2| &bodies[co_parent2.handle]);

                let rb_type1 = rb1.map(|rb| rb.body_type).unwrap_or(RigidBodyType::Fixed);
                let rb_type2 = rb2.map(|rb| rb.body_type).unwrap_or(RigidBodyType::Fixed);

                // Deal with contacts disabled between bodies attached by joints.
                if let (Some(co_parent1), Some(co_parent2)) = (&co1.parent, &co2.parent) {
                    for (_, joint) in
                        impulse_joints.joints_between(co_parent1.handle, co_parent2.handle)
                    {
                        if !joint.data.contacts_enabled {
                            pair.clear();
                            break 'emit_events;
                        }
                    }

                    let link1 = multibody_joints.rigid_body_link(co_parent1.handle);
                    let link2 = multibody_joints.rigid_body_link(co_parent2.handle);

                    if let (Some(link1), Some(link2)) = (link1, link2) {
                        // If both bodies belong to the same multibody, apply some additional built-in
                        // contact filtering rules.
                        if link1.multibody == link2.multibody {
                            // 1) check if self-contacts is enabled.
                            if let Some(mb) = multibody_joints.get_multibody(link1.multibody) {
                                if !mb.self_contacts_enabled() {
                                    pair.clear();
                                    break 'emit_events;
                                }
                            }

                            // 2) if they are attached by a joint, check if  contacts is disabled.
                            if let Some((_, _, mb_link)) =
                                multibody_joints.joint_between(co_parent1.handle, co_parent2.handle)
                            {
                                if !mb_link.joint.data.contacts_enabled {
                                    pair.clear();
                                    break 'emit_events;
                                }
                            }
                        }
                    }
                }

                // Filter based on the rigid-body types.
                if !co1.flags.active_collision_types.test(rb_type1, rb_type2)
                    && !co2.flags.active_collision_types.test(rb_type1, rb_type2)
                {
                    pair.clear();
                    break 'emit_events;
                }

                // Filter based on collision groups.
                if !co1.flags.collision_groups.test(co2.flags.collision_groups) {
                    pair.clear();
                    break 'emit_events;
                }

                let active_hooks = co1.flags.active_hooks | co2.flags.active_hooks;

                let mut solver_flags = if active_hooks.contains(ActiveHooks::FILTER_CONTACT_PAIRS) {
                    let context = PairFilterContext {
                        bodies,
                        colliders,
                        rigid_body1: rb_handle1,
                        rigid_body2: rb_handle2,
                        collider1: pair.collider1,
                        collider2: pair.collider2,
                    };

                    if let Some(solver_flags) = hooks.filter_contact_pair(&context) {
                        solver_flags
                    } else {
                        // No contact allowed.
                        pair.clear();
                        break 'emit_events;
                    }
                } else {
                    SolverFlags::default()
                };

                if !co1.flags.solver_groups.test(co2.flags.solver_groups) {
                    solver_flags.remove(SolverFlags::COMPUTE_IMPULSES);
                }

                if co1.changes.contains(ColliderChanges::SHAPE)
                    || co2.changes.contains(ColliderChanges::SHAPE)
                {
                    // The shape changed so the workspace is no longer valid.
                    pair.workspace = None;
                }

                let pos12 = co1.pos.inv_mul(&co2.pos);

                let contact_skin_sum = co1.contact_skin() + co2.contact_skin();
                let soft_ccd_prediction1 = rb1.map(|rb| rb.soft_ccd_prediction()).unwrap_or(0.0);
                let soft_ccd_prediction2 = rb2.map(|rb| rb.soft_ccd_prediction()).unwrap_or(0.0);
                let effective_prediction_distance = if soft_ccd_prediction1 > 0.0
                    || soft_ccd_prediction2 > 0.0
                {
                    let aabb1 = co1.compute_collision_aabb(0.0);
                    let aabb2 = co2.compute_collision_aabb(0.0);
                    let inv_dt = crate::utils::inv(dt);

                    let linvel1 = rb1
                        .map(|rb| rb.linvel().clamp_length_max(soft_ccd_prediction1 * inv_dt))
                        .unwrap_or_default();
                    let linvel2 = rb2
                        .map(|rb| rb.linvel().clamp_length_max(soft_ccd_prediction2 * inv_dt))
                        .unwrap_or_default();

                    if !aabb1.intersects(&aabb2)
                        && !aabb1.intersects_moving_aabb(&aabb2, linvel2 - linvel1)
                    {
                        pair.clear();
                        break 'emit_events;
                    }

                    prediction_distance.max(dt * (linvel1 - linvel2).length()) + contact_skin_sum
                } else {
                    prediction_distance + contact_skin_sum
                };

                let _ = query_dispatcher.contact_manifolds(
                    &pos12,
                    &*co1.shape,
                    &*co2.shape,
                    effective_prediction_distance,
                    &mut pair.manifolds,
                    &mut pair.workspace,
                );

                let friction = CoefficientCombineRule::combine(
                    co1.material.friction,
                    co2.material.friction,
                    co1.material.friction_combine_rule,
                    co2.material.friction_combine_rule,
                );
                let restitution = CoefficientCombineRule::combine(
                    co1.material.restitution,
                    co2.material.restitution,
                    co1.material.restitution_combine_rule,
                    co2.material.restitution_combine_rule,
                );

                let zero = RigidBodyDominance(0); // The value doesn't matter, it will be MAX because of the effective groups.
                let dominance1 = rb1.map(|rb| rb.dominance).unwrap_or(zero);
                let dominance2 = rb2.map(|rb| rb.dominance).unwrap_or(zero);

                for manifold in &mut pair.manifolds {
                    let world_pos1 = manifold.subshape_pos1.prepend_to(&co1.pos);
                    let world_pos2 = manifold.subshape_pos2.prepend_to(&co2.pos);
                    manifold.data.solver_contacts.clear();
                    manifold.data.rigid_body1 = rb_handle1;
                    manifold.data.rigid_body2 = rb_handle2;
                    manifold.data.solver_flags = solver_flags;
                    manifold.data.relative_dominance = dominance1.effective_group(&rb_type1)
                        - dominance2.effective_group(&rb_type2);
                    manifold.data.normal = world_pos1.rotation * manifold.local_n1;

                    // Generate solver contacts.
                    #[allow(unused_mut)] // Mut not needed in 2D.
                    let mut selected = [0, 1, 2, 3];
                    #[allow(unused_mut)] // Mut not needed in 2D.
                    let mut num_selected = MAX_MANIFOLD_POINTS.min(manifold.points.len());

                    #[cfg(feature = "dim3")]
                    // super::manifold_reduction::reduce_manifold_bepu_like(
                    //     manifold,
                    //     &mut selected,
                    //     &mut num_selected,
                    // );
                    #[cfg(feature = "dim3")]
                    super::manifold_reduction::reduce_manifold_naive(
                        manifold,
                        &mut selected,
                        &mut num_selected,
                        prediction_distance,
                    );

                    for contact_id in &selected[..num_selected] {
                        //     // manifold.points.iter().enumerate() {
                        let contact = &manifold.points[*contact_id];
                        let effective_contact_dist =
                            contact.dist - co1.contact_skin() - co2.contact_skin();

                        let keep_solver_contact = effective_contact_dist < prediction_distance || {
                            let world_pt1 = world_pos1 * contact.local_p1;
                            let world_pt2 = world_pos2 * contact.local_p2;
                            let vel1 = rb1
                                .map(|rb| rb.velocity_at_point(world_pt1))
                                .unwrap_or_default();
                            let vel2 = rb2
                                .map(|rb| rb.velocity_at_point(world_pt2))
                                .unwrap_or_default();
                            effective_contact_dist + (vel2 - vel1).dot(manifold.data.normal) * dt
                                < prediction_distance
                        };

                        if keep_solver_contact {
                            // Generate the solver contact.
                            let world_pt1 = world_pos1 * contact.local_p1;
                            let world_pt2 = world_pos2 * contact.local_p2;

                            let effective_point = world_pt1.midpoint(world_pt2);

                            let solver_contact = SolverContact {
                                contact_id: [*contact_id as u32],
                                point: effective_point,
                                dist: effective_contact_dist,
                                friction,
                                restitution,
                                tangent_velocity: Default::default(),
                                is_new: (contact.data.impulse == 0.0) as u32 as Real,
                                warmstart_impulse: contact.data.warmstart_impulse,
                                warmstart_tangent_impulse: contact.data.warmstart_tangent_impulse,
                                #[cfg(feature = "dim2")]
                                warmstart_twist_impulse: na::zero(),
                                #[cfg(feature = "dim3")]
                                warmstart_twist_impulse: contact.data.warmstart_twist_impulse,
                                #[cfg(feature = "dim3")]
                                padding: Default::default(),
                            };

                            manifold.data.solver_contacts.push(solver_contact);
                        }
                    }

                    // Apply the user-defined contact modification.
                    if active_hooks.contains(ActiveHooks::MODIFY_SOLVER_CONTACTS) {
                        let mut modifiable_solver_contacts =
                            std::mem::take(&mut manifold.data.solver_contacts);
                        let mut modifiable_user_data = manifold.data.user_data;
                        let mut modifiable_normal = manifold.data.normal;

                        let mut context = ContactModificationContext {
                            bodies,
                            colliders,
                            rigid_body1: rb_handle1,
                            rigid_body2: rb_handle2,
                            collider1: pair.collider1,
                            collider2: pair.collider2,
                            manifold,
                            solver_contacts: &mut modifiable_solver_contacts,
                            normal: &mut modifiable_normal,
                            user_data: &mut modifiable_user_data,
                        };

                        hooks.modify_solver_contacts(&mut context);

                        manifold.data.solver_contacts = modifiable_solver_contacts;
                        manifold.data.normal = modifiable_normal;
                        manifold.data.user_data = modifiable_user_data;
                    }
                }
            }

            /*
             * Handle actions on contact start/stop:
             *  - Emit event (if applicable).
             *  - Notify the island manager to potentially wake up the bodies.
             */
            let has_any_active_contact = pair.has_any_active_contact();
            if has_any_active_contact != had_any_active_contact {
                let active_events = co1.flags.active_events | co2.flags.active_events;
                if active_events.contains(ActiveEvents::COLLISION_EVENTS) {
                    if has_any_active_contact {
                        pair.emit_start_event(bodies, colliders, events);
                    } else {
                        pair.emit_stop_event(bodies, colliders, events);
                    }
                }

                #[cfg(not(feature = "parallel"))]
                islands.interaction_started_or_stopped(
                    bodies,
                    rb_handle1,
                    rb_handle2,
                    has_any_active_contact,
                    true,
                );
                #[cfg(feature = "parallel")]
                {
                    // When running in parallel mode, defer the islands call after the loop.
                    let _ = snd.send((rb_handle1, rb_handle2, has_any_active_contact));
                }
            }
        });

        #[cfg(feature = "parallel")]
        {
            drop(snd);
            for (parent1, parent2, any_active_contact) in rcv.iter() {
                islands.interaction_started_or_stopped(
                    bodies,
                    parent1,
                    parent2,
                    any_active_contact,
                    true,
                );
            }
        }
    }

    /// Retrieve all the interactions with at least one contact point, happening between two active bodies.
    // NOTE: this is very similar to the code from ImpulseJointSet::select_active_interactions.
    pub(crate) fn select_active_contacts<'a>(
        &'a mut self,
        islands: &IslandManager,
        bodies: &RigidBodySet,
        out_contact_pairs: &mut Vec<TemporaryInteractionIndex>,
        out_manifolds: &mut Vec<&'a mut ContactManifold>,
        out: &mut [Vec<ContactManifoldIndex>],
    ) {
        for out_island in &mut out[..islands.active_islands().len()] {
            out_island.clear();
        }

        // TODO: don't iterate through all the interactions.
        for (pair_id, inter) in self.contact_graph.graph.edges.iter_mut().enumerate() {
            let mut push_pair = false;

            for manifold in &mut inter.weight.manifolds {
                if manifold
                    .data
                    .solver_flags
                    .contains(SolverFlags::COMPUTE_IMPULSES)
                    && manifold.data.num_active_contacts() != 0
                {
                    let (active_island_id1, rb_type1, sleeping1) =
                        if let Some(handle1) = manifold.data.rigid_body1 {
                            let rb1 = &bodies[handle1];
                            (
                                rb1.ids.active_island_id,
                                rb1.body_type,
                                rb1.activation.sleeping,
                            )
                        } else {
                            (0, RigidBodyType::Fixed, true)
                        };

                    let (active_island_id2, rb_type2, sleeping2) =
                        if let Some(handle2) = manifold.data.rigid_body2 {
                            let rb2 = &bodies[handle2];
                            (
                                rb2.ids.active_island_id,
                                rb2.body_type,
                                rb2.activation.sleeping,
                            )
                        } else {
                            (0, RigidBodyType::Fixed, true)
                        };

                    if (rb_type1.is_dynamic() || rb_type2.is_dynamic())
                        && (!rb_type1.is_dynamic() || !sleeping1)
                        && (!rb_type2.is_dynamic() || !sleeping2)
                    {
                        let island_awake_index = if !rb_type1.is_dynamic() {
                            islands.islands[active_island_id2]
                                .id_in_awake_list()
                                .expect("Internal error: island should be awake.")
                        } else {
                            islands.islands[active_island_id1]
                                .id_in_awake_list()
                                .expect("Internal error: island should be awake.")
                        };

                        out[island_awake_index].push(out_manifolds.len());
                        out_manifolds.push(manifold);
                        push_pair = true;
                    }
                }
            }

            if push_pair {
                out_contact_pairs.push(EdgeIndex::new(pair_id as u32));
            }
        }
    }
}

#[cfg(test)]
#[cfg(feature = "f32")]
#[cfg(feature = "dim3")]
mod test {
    use crate::math::Vector;
    use crate::prelude::{
        CCDSolver, ColliderBuilder, DefaultBroadPhase, IntegrationParameters, PhysicsPipeline,
        RigidBodyBuilder,
    };

    use super::*;

    /// Test for https://github.com/dimforge/rapier/issues/734.
    #[test]
    pub fn collider_set_parent_depenetration() {
        // This tests the scenario:
        // 1. Body A has two colliders attached (and overlapping), Body B has none.
        // 2. One of the colliders from Body A gets re-parented to Body B.
        //    -> Collision is properly detected between the colliders of A and B.
        let mut rigid_body_set = RigidBodySet::new();
        let mut collider_set = ColliderSet::new();

        /* Create the ground. */
        let collider = ColliderBuilder::ball(0.5);

        /* Create body 1, which will contain both colliders at first. */
        let rigid_body_1 = RigidBodyBuilder::dynamic()
            .translation(Vector::new(0.0, 0.0, 0.0))
            .build();
        let body_1_handle = rigid_body_set.insert(rigid_body_1);

        /* Create collider 1. Parent it to rigid body 1. */
        let collider_1_handle =
            collider_set.insert_with_parent(collider.build(), body_1_handle, &mut rigid_body_set);

        /* Create collider 2. Parent it to rigid body 1. */
        let collider_2_handle =
            collider_set.insert_with_parent(collider.build(), body_1_handle, &mut rigid_body_set);

        /* Create body 2. No attached colliders yet. */
        let rigid_body_2 = RigidBodyBuilder::dynamic()
            .translation(Vector::new(0.0, 0.0, 0.0))
            .build();
        let body_2_handle = rigid_body_set.insert(rigid_body_2);

        /* Create other structures necessary for the simulation. */
        let gravity = Vector::ZERO;
        let integration_parameters = IntegrationParameters::default();
        let mut physics_pipeline = PhysicsPipeline::new();
        let mut island_manager = IslandManager::new();
        let mut broad_phase = DefaultBroadPhase::new();
        let mut narrow_phase = NarrowPhase::new();
        let mut impulse_joint_set = ImpulseJointSet::new();
        let mut multibody_joint_set = MultibodyJointSet::new();
        let mut ccd_solver = CCDSolver::new();
        let physics_hooks = ();
        let event_handler = ();

        physics_pipeline.step(
            gravity,
            &integration_parameters,
            &mut island_manager,
            &mut broad_phase,
            &mut narrow_phase,
            &mut rigid_body_set,
            &mut collider_set,
            &mut impulse_joint_set,
            &mut multibody_joint_set,
            &mut ccd_solver,
            &physics_hooks,
            &event_handler,
        );
        let collider_1_position = collider_set.get(collider_1_handle).unwrap().pos;
        let collider_2_position = collider_set.get(collider_2_handle).unwrap().pos;
        assert!(
            (collider_1_position.translation - collider_2_position.translation).length() < 0.5f32
        );

        let contact_pair = narrow_phase
            .contact_pair(collider_1_handle, collider_2_handle)
            .expect("The contact pair should exist.");
        assert_eq!(contact_pair.manifolds.len(), 0);
        assert!(
            narrow_phase
                .intersection_pair(collider_1_handle, collider_2_handle)
                .is_none(),
            "Interaction pair is for sensors"
        );
        /* Parent collider 2 to body 2. */
        collider_set.set_parent(collider_2_handle, Some(body_2_handle), &mut rigid_body_set);

        physics_pipeline.step(
            gravity,
            &integration_parameters,
            &mut island_manager,
            &mut broad_phase,
            &mut narrow_phase,
            &mut rigid_body_set,
            &mut collider_set,
            &mut impulse_joint_set,
            &mut multibody_joint_set,
            &mut ccd_solver,
            &physics_hooks,
            &event_handler,
        );

        let contact_pair = narrow_phase
            .contact_pair(collider_1_handle, collider_2_handle)
            .expect("The contact pair should exist.");
        assert_eq!(contact_pair.manifolds.len(), 1);
        assert!(
            narrow_phase
                .intersection_pair(collider_1_handle, collider_2_handle)
                .is_none(),
            "Interaction pair is for sensors"
        );

        /* Run the game loop, stepping the simulation once per frame. */
        for _ in 0..200 {
            physics_pipeline.step(
                gravity,
                &integration_parameters,
                &mut island_manager,
                &mut broad_phase,
                &mut narrow_phase,
                &mut rigid_body_set,
                &mut collider_set,
                &mut impulse_joint_set,
                &mut multibody_joint_set,
                &mut ccd_solver,
                &physics_hooks,
                &event_handler,
            );

            let collider_1_position = collider_set.get(collider_1_handle).unwrap().pos;
            let collider_2_position = collider_set.get(collider_2_handle).unwrap().pos;
            println!("collider 1 position: {}", collider_1_position.translation);
            println!("collider 2 position: {}", collider_2_position.translation);
        }

        let collider_1_position = collider_set.get(collider_1_handle).unwrap().pos;
        let collider_2_position = collider_set.get(collider_2_handle).unwrap().pos;
        println!("collider 2 position: {}", collider_2_position.translation);
        assert!(
            (collider_1_position.translation - collider_2_position.translation).length() >= 0.5f32,
            "colliders should no longer be penetrating."
        );
    }

    /// Test for https://github.com/dimforge/rapier/issues/734.
    #[test]
    pub fn collider_set_parent_no_self_intersection() {
        // This tests the scenario:
        // 1. Body A and Body B each have one collider attached.
        //    -> There should be a collision detected between A and B.
        // 2. The collider from Body B gets attached to Body A.
        //    -> There should no longer be any collision between A and B.
        // 3. Re-parent one of the collider from Body A to Body B again.
        //    -> There should a collision again.
        let mut rigid_body_set = RigidBodySet::new();
        let mut collider_set = ColliderSet::new();

        /* Create the ground. */
        let collider = ColliderBuilder::ball(0.5);

        /* Create body 1, which will contain collider 1. */
        let rigid_body_1 = RigidBodyBuilder::dynamic()
            .translation(Vector::new(0.0, 0.0, 0.0))
            .build();
        let body_1_handle = rigid_body_set.insert(rigid_body_1);

        /* Create collider 1. Parent it to rigid body 1. */
        let collider_1_handle =
            collider_set.insert_with_parent(collider.build(), body_1_handle, &mut rigid_body_set);

        /* Create body 2, which will contain collider 2 at first. */
        let rigid_body_2 = RigidBodyBuilder::dynamic()
            .translation(Vector::new(0.0, 0.0, 0.0))
            .build();
        let body_2_handle = rigid_body_set.insert(rigid_body_2);

        /* Create collider 2. Parent it to rigid body 2. */
        let collider_2_handle =
            collider_set.insert_with_parent(collider.build(), body_2_handle, &mut rigid_body_set);

        /* Create other structures necessary for the simulation. */
        let gravity = Vector::ZERO;
        let integration_parameters = IntegrationParameters::default();
        let mut physics_pipeline = PhysicsPipeline::new();
        let mut island_manager = IslandManager::new();
        let mut broad_phase = DefaultBroadPhase::new();
        let mut narrow_phase = NarrowPhase::new();
        let mut impulse_joint_set = ImpulseJointSet::new();
        let mut multibody_joint_set = MultibodyJointSet::new();
        let mut ccd_solver = CCDSolver::new();
        let physics_hooks = ();
        let event_handler = ();

        physics_pipeline.step(
            gravity,
            &integration_parameters,
            &mut island_manager,
            &mut broad_phase,
            &mut narrow_phase,
            &mut rigid_body_set,
            &mut collider_set,
            &mut impulse_joint_set,
            &mut multibody_joint_set,
            &mut ccd_solver,
            &physics_hooks,
            &event_handler,
        );

        let contact_pair = narrow_phase
            .contact_pair(collider_1_handle, collider_2_handle)
            .expect("The contact pair should exist.");
        assert_eq!(
            contact_pair.manifolds.len(),
            1,
            "There should be a contact manifold."
        );

        let collider_1_position = collider_set.get(collider_1_handle).unwrap().pos;
        let collider_2_position = collider_set.get(collider_2_handle).unwrap().pos;
        assert!(
            (collider_1_position.translation - collider_2_position.translation).length() < 0.5f32
        );

        /* Parent collider 2 to body 1. */
        collider_set.set_parent(collider_2_handle, Some(body_1_handle), &mut rigid_body_set);
        physics_pipeline.step(
            gravity,
            &integration_parameters,
            &mut island_manager,
            &mut broad_phase,
            &mut narrow_phase,
            &mut rigid_body_set,
            &mut collider_set,
            &mut impulse_joint_set,
            &mut multibody_joint_set,
            &mut ccd_solver,
            &physics_hooks,
            &event_handler,
        );

        let contact_pair = narrow_phase
            .contact_pair(collider_1_handle, collider_2_handle)
            .expect("The contact pair should no longer exist.");
        assert_eq!(
            contact_pair.manifolds.len(),
            0,
            "Colliders with same parent should not be in contact together."
        );

        /* Parent collider 2 back to body 1. */
        collider_set.set_parent(collider_2_handle, Some(body_2_handle), &mut rigid_body_set);
        physics_pipeline.step(
            gravity,
            &integration_parameters,
            &mut island_manager,
            &mut broad_phase,
            &mut narrow_phase,
            &mut rigid_body_set,
            &mut collider_set,
            &mut impulse_joint_set,
            &mut multibody_joint_set,
            &mut ccd_solver,
            &physics_hooks,
            &event_handler,
        );

        let contact_pair = narrow_phase
            .contact_pair(collider_1_handle, collider_2_handle)
            .expect("The contact pair should exist.");
        assert_eq!(
            contact_pair.manifolds.len(),
            1,
            "There should be a contact manifold."
        );
    }
}
