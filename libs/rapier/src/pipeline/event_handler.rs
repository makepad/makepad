use crate::dynamics::RigidBodySet;
use crate::geometry::{ColliderSet, CollisionEvent, ContactForceEvent, ContactPair};
use crate::math::Real;
use std::sync::mpsc::Sender;

bitflags::bitflags! {
    #[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
    #[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
    /// Flags that control which physics events are generated for a collider.
    ///
    /// By default, colliders don't generate events (for performance). Enable specific events
    /// per-collider using these flags.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// // Enable collision start/stop events for a trigger zone
    /// let trigger = ColliderBuilder::cuboid(5.0, 5.0, 5.0)
    ///     .sensor(true)
    ///     .active_events(ActiveEvents::COLLISION_EVENTS)
    ///     .build();
    ///
    /// // Enable force events for breakable glass
    /// let glass = ColliderBuilder::cuboid(1.0, 2.0, 0.1)
    ///     .active_events(ActiveEvents::CONTACT_FORCE_EVENTS)
    ///     .contact_force_event_threshold(1000.0)
    ///     .build();
    /// ```
    pub struct ActiveEvents: u32 {
        /// Enables `Started`/`Stopped` collision events for this collider.
        ///
        /// You'll receive events when this collider starts or stops touching others.
        const COLLISION_EVENTS = 0b0001;

        /// Enables contact force events when forces exceed a threshold.
        ///
        /// You'll receive events when contact forces surpass `contact_force_event_threshold`.
        const CONTACT_FORCE_EVENTS = 0b0010;
    }
}

impl Default for ActiveEvents {
    fn default() -> Self {
        ActiveEvents::empty()
    }
}

/// A callback interface for receiving physics events (collisions starting/stopping, contact forces).
///
/// Implement this trait to get notified when:
/// - Two colliders start or stop touching ([`handle_collision_event`](Self::handle_collision_event))
/// - Contact forces exceed a threshold ([`handle_contact_force_event`](Self::handle_contact_force_event))
///
/// # Common use cases
/// - Playing sound effects when objects collide
/// - Triggering game events (damage, pickups, checkpoints)
/// - Monitoring structural stress
/// - Detecting when specific objects touch
///
/// # Built-in implementation
/// Use [`ChannelEventCollector`] to collect events into channels for processing after the physics step.
///
/// # Example
/// ```
/// # use rapier3d::prelude::*;
/// # use rapier3d::geometry::ContactPair;
/// struct MyEventHandler;
///
/// impl EventHandler for MyEventHandler {
///     fn handle_collision_event(
///         &self,
///         bodies: &RigidBodySet,
///         colliders: &ColliderSet,
///         event: CollisionEvent,
///         contact_pair: Option<&ContactPair>,
///     ) {
///         match event {
///             CollisionEvent::Started(h1, h2, _) => {
///                 println!("Collision started between {:?} and {:?}", h1, h2);
///             }
///             CollisionEvent::Stopped(h1, h2, _) => {
///                 println!("Collision ended between {:?} and {:?}", h1, h2);
///             }
///         }
///     }
/// #   fn handle_contact_force_event(&self, _dt: Real, _bodies: &RigidBodySet, _colliders: &ColliderSet, _contact_pair: &ContactPair, _total_force_magnitude: Real) {}
/// }
/// ```
pub trait EventHandler: Send + Sync {
    /// Called when two colliders start or stop touching each other.
    ///
    /// Collision events are triggered when intersection state changes (Started/Stopped).
    /// At least one collider must have [`ActiveEvents::COLLISION_EVENTS`] enabled.
    ///
    /// # Parameters
    /// * `event` - Either `Started(h1, h2, flags)` or `Stopped(h1, h2, flags)`
    /// * `bodies` - All rigid bodies (to look up body info)
    /// * `colliders` - All colliders (to look up collider info)
    /// * `contact_pair` - Detailed contact info (`None` for sensors, since they don't compute contacts)
    ///
    /// # Use cases
    /// - Play collision sound effects
    /// - Apply damage when objects hit
    /// - Trigger game events (entering zones, picking up items)
    /// - Track what's touching what
    fn handle_collision_event(
        &self,
        bodies: &RigidBodySet,
        colliders: &ColliderSet,
        event: CollisionEvent,
        contact_pair: Option<&ContactPair>,
    );

    /// Called when contact forces exceed a threshold.
    ///
    /// Triggered when the total force magnitude between two colliders exceeds the
    /// [`Collider::contact_force_event_threshold`](crate::geometry::Collider::set_contact_force_event_threshold).
    /// At least one collider must have [`ActiveEvents::CONTACT_FORCE_EVENTS`] enabled.
    ///
    /// # Use cases
    /// - Detect hard impacts (for damage, breaking objects)
    /// - Monitor structural stress
    /// - Trigger effects at certain force levels (sparks, cracks)
    ///
    /// # Parameters
    /// * `total_force_magnitude` - Sum of magnitudes of all contact forces (not vector sum!)
    ///   Example: Two forces `[0, 100, 0]` and `[0, -100, 0]` → magnitude = 200 (not 0)
    fn handle_contact_force_event(
        &self,
        dt: Real,
        bodies: &RigidBodySet,
        colliders: &ColliderSet,
        contact_pair: &ContactPair,
        total_force_magnitude: Real,
    );
}

impl EventHandler for () {
    fn handle_collision_event(
        &self,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        _event: CollisionEvent,
        _contact_pair: Option<&ContactPair>,
    ) {
    }

    fn handle_contact_force_event(
        &self,
        _dt: Real,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        _contact_pair: &ContactPair,
        _total_force_magnitude: Real,
    ) {
    }
}

/// A ready-to-use event handler that collects events into channels for later processing.
///
/// Instead of processing events immediately during physics step, this collector sends them
/// to channels that you can poll from your game loop. This is the recommended approach.
///
/// # Example
/// ```
/// # use rapier3d::prelude::*;
/// use std::sync::mpsc::channel;
///
/// let (collision_send, collision_recv) = channel();
/// let (contact_force_send, contact_force_recv) = channel();
/// let event_handler = ChannelEventCollector::new(collision_send, contact_force_send);
///
/// // After physics step:
/// while let Ok(collision_event) = collision_recv.try_recv() {
///     match collision_event {
///         CollisionEvent::Started(h1, h2, _) => println!("Collision!"),
///         CollisionEvent::Stopped(h1, h2, _) => println!("Separated"),
///     }
/// }
/// ```
pub struct ChannelEventCollector {
    collision_event_sender: Sender<CollisionEvent>,
    contact_force_event_sender: Sender<ContactForceEvent>,
}

impl ChannelEventCollector {
    /// Initialize a new collision event handler from channel senders.
    pub fn new(
        collision_event_sender: Sender<CollisionEvent>,
        contact_force_event_sender: Sender<ContactForceEvent>,
    ) -> Self {
        Self {
            collision_event_sender,
            contact_force_event_sender,
        }
    }
}

impl EventHandler for ChannelEventCollector {
    fn handle_collision_event(
        &self,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        event: CollisionEvent,
        _: Option<&ContactPair>,
    ) {
        let _ = self.collision_event_sender.send(event);
    }

    fn handle_contact_force_event(
        &self,
        dt: Real,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        contact_pair: &ContactPair,
        total_force_magnitude: Real,
    ) {
        let result = ContactForceEvent::from_contact_pair(dt, contact_pair, total_force_magnitude);
        let _ = self.contact_force_event_sender.send(result);
    }
}
