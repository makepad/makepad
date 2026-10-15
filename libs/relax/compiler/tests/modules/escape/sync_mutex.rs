pub fn frame(x: f32) -> f32 { let m = std::sync::Mutex::new(x); *m.lock().unwrap() }
