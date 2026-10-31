// matrix-sdk-crypto store/traits.rs + crypto_store_wrapper.rs
use dax_common::AsyncTraitDeps;
use std::sync::Arc;
pub trait CryptoStore: AsyncTraitDeps {
    type Error: std::fmt::Debug + Into<StoreError>;
    fn id(&self) -> Result<u32, Self::Error>;
}
#[derive(Debug)]
pub struct StoreError;
/// matrix-sdk-crypto store/traits.rs: the object type names its associated type
pub type DynCryptoStore = dyn CryptoStore<Error = StoreError>;
#[derive(Debug)]
pub struct Mem(pub u32);
impl CryptoStore for Mem {
    type Error = StoreError;
    fn id(&self) -> Result<u32, StoreError> { Ok(self.0) }
}
#[derive(Debug)]
pub struct CryptoStoreWrapper {
    store: Arc<DynCryptoStore>,
}
impl CryptoStoreWrapper {
    pub fn new(store: Arc<DynCryptoStore>) -> Self { Self { store } }
    pub fn id(&self) -> u32 { self.store.id().unwrap() }
}
