// matrix-sdk Client -> Arc<CryptoStoreWrapper> -> Arc<dyn CryptoStore>: Send + Sync through
// supertraits defined in another crate (rustc elaborates the object type's supertraits)
use dax_store::{CryptoStoreWrapper, Mem};
use std::sync::Arc;
fn is_send_sync<T: Send + Sync>(_: &T) {}
async fn tick() {}
async fn uses(w: Arc<CryptoStoreWrapper>) -> u32 { tick().await; w.id() }
fn main() {
    let w = Arc::new(CryptoStoreWrapper::new(Arc::new(Mem(7))));
    is_send_sync(&w);
    let f = uses(w.clone());
    let h = std::thread::spawn(move || {
        let _f = f;
        w.id()
    });
    println!("dyn_auto_xcrate {}", h.join().unwrap());
}
