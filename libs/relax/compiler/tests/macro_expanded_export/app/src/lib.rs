use defs::pick;

pub fn picks() -> (u32, u32) {
    (defs::pick!(4, 2), pick!(1, 1))
}
