pub mod store {
    pub struct Store { pub fine: Option<u32>, len: usize }
    impl Store {
        pub fn new() -> Self { Store { fine: None, len: 7 } }
        pub fn len(&self) -> usize { self.len }
    }
}

// E0451: a struct update outside the struct's module names its private field `len`
pub fn updated() -> usize {
    store::Store { fine: Some(1), ..store::Store::new() }.len()
}
