//~ msg: cannot find macro `emit` in this scope
mod source {
    macro_rules! publish {
        () => {
            macro_rules! emit { () => { fn generated() {} } }
            use emit;
        };
    }
    publish!();
    const _: fn() = { emit!(); generated };
}

source::emit!();
fn main() { generated(); }
