//! The document layer beyond the graph itself: structured identities, the
//! open connection-type registry and workflow packages.
//!
//! Registry snapshots supply the graph codec and pinned component modules
//! with the same type and node descriptors. Runtime executor factories are
//! separate from this serializable document metadata.

pub mod binding;
pub mod closure;
pub mod ident;
pub mod interface;
pub mod package;
pub mod types;
pub mod registry;
pub use registry::{Registry, RegistryData, RegistryError, TypeDescriptor, ParamDescriptor, ParamType, InputDescriptor, NodeCodec, WriteField, EndpointClass, BUILTIN_OWNER};
