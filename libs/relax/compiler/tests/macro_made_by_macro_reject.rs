//! A generated macro's type mismatch points to the literal inside its definition.
// rejects: E0308
// rejects-at: 7:35 E0308
macro_rules! make {
    () => {
        macro_rules! made {
            () => {{ let v: u32 = "text"; v }};
        }
    };
}
make!();

pub fn run() -> u32 {
    made!()
}
