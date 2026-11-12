fn values() -> [u32; 5] {
    [
        enum_cast_dep::State::First as u32,
        enum_cast_dep::State::Second as u32,
        enum_cast_dep::Signed::Negative as u32,
        enum_cast_dep::Signed::Positive as u32,
        enum_cast_dep::local_variant_cast(),
    ]
}

#[cfg(feature = "reject-variant")]
fn foreign_non_exhaustive_variant() -> u32 {
    enum_cast_dep::variant_only() as u32
}

fn main() {
    assert_eq!(values(), [3, 7, u32::MAX, 2, 0]);
    println!("non-exhaustive enum casts: ok");
}

#[test]
fn casts_use_discriminants() {
    assert_eq!(values(), [3, 7, u32::MAX, 2, 0]);
}
