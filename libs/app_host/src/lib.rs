//! Hosting makepad apps that run as child processes with `--stdin-loop`.
//!
//! - [`hub`]: the localhost websocket the children connect to.
//! - [`hosted`]: [`HostedApp`], the protocol state with one child: swapchain,
//!   bootstrap, presented frames, tick pacing, input forwarding, snapshots.
//! - [`launch`]: the launch environment and process-group kill.
//! - [`children`]: [`Children`], the lifecycle of a host's child apps: hub,
//!   build/launch through the registry, beat, quit, message routing.

pub mod audio_ring;
pub mod children;
// Frames through shared GPU textures (the HAL's exports): native only.
#[cfg(not(target_arch = "wasm32"))]
pub mod frame_ring;
pub mod hosted;
pub mod hub;
pub mod input;
pub mod launch;
pub mod shm;
pub mod status;

pub use children::{ChildEvent, ChildSpec, Children, Home, Life, Slot};
pub use input::{forward_hit, Forwarded};
pub use audio_ring::{AudioRecord, AudioRingPacket, AudioRingSetup, HostAudioRing, ProducerAudioRing};
#[cfg(not(target_arch = "wasm32"))]
pub use frame_ring::{HostRing, ProducerRing, RingFrameInfo, RingLease, RingSetup, RingShown, RingSpec};
pub use hosted::{Clock, HostedApp, Presented, RunTarget};
pub use hub::{send_to_app, AppHub, ClientId, HubEvent};
pub use status::AppStatus;
pub use makepad_app_registry::BuildPolicy;
