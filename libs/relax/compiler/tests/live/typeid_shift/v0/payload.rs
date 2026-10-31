pub struct Payload {
    v: u32,
}

pub fn check() -> u32 {
    let p = Payload { v: 7 };
    let now = unsafe { core::intrinsics_mem::type_id::<Payload>() };
    (crate::STORED.with(|s| s.get()) == now) as u32 * p.v
}
