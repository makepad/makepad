// A module implementing a trait of another crate for a type of another crate: refused (the
// orphan rule), which keeps the running program's impl and method answers valid.
pub struct Mine;
impl core::fmt::Display for Vec<u8> {
    fn fmt(&self, _f: &mut core::fmt::Formatter) -> core::fmt::Result {
        Ok(())
    }
}
pub fn frame(x: f32) -> f32 {
    x
}
