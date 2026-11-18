// rejects: E0308
// A placeholder of the dyn's binder tied to the fn pointer binder's own: one type is more general than the other.
pub fn tied_placeholder(
    source: for<'e> fn(&'e u8, Box<dyn for<'s> Fn(&'s u8)>),
) -> for<'r> fn(&'r u8, Box<dyn Fn(&'r u8)>) {
    source
}
