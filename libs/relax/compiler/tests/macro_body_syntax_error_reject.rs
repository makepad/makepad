//! The missing initializer is reported at the semicolon in the macro body, not at the call.
// rejects: expected expression
// rejects-at: 5:21 expected expression
macro_rules! m {
    () => { let x = ; };
}

pub fn run() {
    m!();
}
