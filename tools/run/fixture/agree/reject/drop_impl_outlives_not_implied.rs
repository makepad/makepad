// rejects: E0367
// A Drop impl may not add `T: 'a` when no field implies it: here `T` is held by value.
pub struct Labelled<'a, T> {
    pub label: &'a str,
    pub value: T,
}
impl<'a, T: 'a> Drop for Labelled<'a, T> {
    fn drop(&mut self) {}
}
