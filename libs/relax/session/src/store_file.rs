//! A store file's header fields as the tools that share the JIT's store read them without the
//! compiler (devhub's disk guard retires one build's stores by them, lease/disk.rs): relax-compiler
//! store.rs writes its headers with these, so they are said once.

/// Every store file starts with the magic ...
pub const MAGIC: &[u8; 4] = b"RCS1";
/// ... then its store format (u32, little-endian): a build of another format cannot read it.
pub const FORMAT: u32 = 3;
/// The header's bytes.
pub const HEADER: usize = 128;
/// Where the header names the Relax build that wrote the store (u64, little-endian).
pub const BUILD_AT: usize = 112;
