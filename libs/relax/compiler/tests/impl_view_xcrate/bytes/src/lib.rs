// winnow 1.0 stream/bytes.rs:357 shape: a second `AsRef` impl for `[u8]`, in a crate the cipher
// crate does not depend on.
pub struct Bytes(pub u8);

impl AsRef<Bytes> for [u8] {
    fn as_ref(&self) -> &Bytes {
        &Bytes(7)
    }
}
