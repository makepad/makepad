// rejects: function pointers and raw pointers not derived from integers in patterns behave unpredictably and should not be relied upon
#[derive(PartialEq, Eq)]
pub enum Key {
    Number(u64),
    Ptr(*const u8),
}

const BAD: Key = Key::Ptr(&0u8 as *const u8);

pub fn check(k: Key) -> bool {
    match k {
        BAD => true,
        _ => false,
    }
}
