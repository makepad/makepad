//! The unresolved name points to the println! argument written in the macro body.
// rejects: E0425
// rejects-at: 5:28 E0425
macro_rules! m {
    () => { println!("{}", missing) };
}

pub fn run() {
    m!();
}
