pub struct Wide(pub u64);

#[cfg(feature = "std")]
impl Wide {
    pub fn label(&self) -> String {
        format!("wide {}", self.0)
    }
}
