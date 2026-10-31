pub fn frame(x: f32) -> f32 { let p = &x as *const f32; unsafe { *p } }
