//! Editable model documents. All evaluation and serialization belongs on a worker.
//!
//! The authoritative state is a checkpoint and a typed transaction log. Render
//! meshes are derived products. No UI, network, locks or thread creation live here.

mod canon;
mod atlas;
mod document;
mod export;
mod ops;
mod texture;
mod service;
mod rig;
mod soft_body;
mod soft_review;
pub use soft_body::{SoftBodyBind, SoftBodyAttachmentRequest};
mod primitives;
pub mod transform;
pub use transform::Transform;

pub use document::*;
pub use export::*;
pub use ops::*;
pub use texture::*;
pub use service::*;
pub use rig::*;
pub use makepad_strict_json as json;

pub const MODELING_API_DOC: &str = include_str!("../API.md");
/// Version of what the model engine builds from a given program. Hosts that
/// cache built products by program hash mix it into the key, so a change to
/// generated geometry rebuilds them once. 2: portable transcendentals (the
/// same bits on every platform, so a product built on one machine is the
/// one every other machine would build).
pub const MODEL_GENERATOR_VERSION: u32 = 2;
pub use makepad_mesh_edit as mesh;

mod schema;
mod authoring_export;
mod scene;
pub use scene::*;

mod surface;
pub use surface::*;

mod mesh_editing;
pub use mesh_editing::*;

mod rig_control;
pub use rig_control::*;

mod construction;
pub use construction::*;

mod morph_export;

mod delivery;

mod selection;
pub use selection::*;

mod review;
mod review_pose;
pub mod templates;
mod character_mesh;
pub mod character;
pub mod stencil;
pub mod import;
mod program;
pub use program::*;
pub use review_pose::*;
