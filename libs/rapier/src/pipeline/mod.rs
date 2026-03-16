//! Structure for combining the various physics components to perform an actual simulation.

pub use collision_pipeline::CollisionPipeline;
pub use event_handler::{ActiveEvents, ChannelEventCollector, EventHandler};
pub use physics_hooks::{ActiveHooks, ContactModificationContext, PairFilterContext, PhysicsHooks};
pub use physics_pipeline::PhysicsPipeline;
pub use query_pipeline::{QueryFilter, QueryFilterFlags, QueryPipeline, QueryPipelineMut};

#[cfg(feature = "debug-render")]
pub use self::debug_render_pipeline::{
    DebugColor, DebugRenderBackend, DebugRenderMode, DebugRenderObject, DebugRenderPipeline,
    DebugRenderStyle,
};

mod collision_pipeline;
mod event_handler;
mod physics_hooks;
mod physics_pipeline;
mod query_pipeline;
mod user_changes;

#[cfg(feature = "debug-render")]
mod debug_render_pipeline;
