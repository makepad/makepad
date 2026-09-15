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
pub use review_pose::*;
