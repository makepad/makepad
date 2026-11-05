// Stands in for an extern crate with a macro (`serde_json::json!`).
pub struct Value(pub u32);

#[macro_export]
macro_rules! json {
    ($v:expr) => {
        $crate::Value($v)
    };
}
