// rejects: E0533
// rejects: expected unit struct, unit variant or constant, found struct variant `E::V`
// rejects-at: 11:9 E0533

pub enum E {
    V { x: u32 },
}

pub fn f<const E: usize>(x: self::E) {
    match x {
        E::V => {}
        _ => {}
    }
}
