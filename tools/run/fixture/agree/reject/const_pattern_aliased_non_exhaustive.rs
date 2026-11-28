// rejects: E0004
// rejects: non-exhaustive patterns: `false` not covered
// rejects: non-exhaustive patterns: `[false]` not covered
trait Tr {
    type A;
}
impl Tr for () {
    type A = bool;
}

const T: <() as Tr>::A = true;
pub const N: usize = 1;
const ARR_T: [bool; N] = [true];

pub fn missing_bool(b: bool) -> u8 {
    match b {
        T => 0,
    }
}

pub fn missing_array(x: [bool; N]) -> u8 {
    match x {
        ARR_T => 0,
    }
}
