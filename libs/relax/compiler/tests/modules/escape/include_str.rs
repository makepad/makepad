const S: &str = include_str!("/etc/hosts");
pub fn frame(x: f32) -> f32 { x + S.len() as f32 }
