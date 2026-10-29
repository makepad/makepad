// matrix-sdk-common lib.rs: Send/Sync aliases that differ on wasm, and the store super trait
#[cfg(not(target_family = "wasm"))]
pub trait SendOutsideWasm: Send {}
#[cfg(not(target_family = "wasm"))]
impl<T: Send> SendOutsideWasm for T {}
#[cfg(target_family = "wasm")]
pub trait SendOutsideWasm {}
#[cfg(target_family = "wasm")]
impl<T> SendOutsideWasm for T {}

#[cfg(not(target_family = "wasm"))]
pub trait SyncOutsideWasm: Sync {}
#[cfg(not(target_family = "wasm"))]
impl<T: Sync> SyncOutsideWasm for T {}
#[cfg(target_family = "wasm")]
pub trait SyncOutsideWasm {}
#[cfg(target_family = "wasm")]
impl<T> SyncOutsideWasm for T {}

pub trait AsyncTraitDeps: std::fmt::Debug + SendOutsideWasm + SyncOutsideWasm {}
impl<T: std::fmt::Debug + SendOutsideWasm + SyncOutsideWasm> AsyncTraitDeps for T {}
