pub mod oid;
mod sha1;

pub use oid::{hash_object, ObjectHasher, ObjectId, OidError};
pub use sha1::Sha1;
