pub fn unsigned(x: u32) -> u32 { -x }
pub fn inferred() -> u8 { let x = -1; x }
pub fn boolean() -> bool { -true }
pub fn float() -> f32 { !1.0f32 }
pub fn pointer(x: *const u8) -> *const u8 { !x }
pub fn unconstrained<T>(x: T) -> T { -x }
pub fn nested_ref(x: &&i32) -> i32 { -x }
