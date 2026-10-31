enum E { A, B(u8) }

impl E {
    fn valid(&self) -> bool {
        match self { Self::A => true, Self::B(_) => false }
    }
}

#[test]
fn self_variant_is_a_value_path() {
    assert!(E::A.valid());
    assert!(!E::B(1).valid());
}
