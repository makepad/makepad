//~ none
mod source {
    macro_rules! publish {
        () => {
            macro_rules! emit { () => { fn generated() {} } }
            pub(crate) use emit;
        };
    }
    publish!();
    const _: fn() = { emit!(); generated };
}

source::emit!();
fn main() { generated(); }
