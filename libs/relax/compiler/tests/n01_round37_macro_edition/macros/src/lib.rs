#[macro_export]
macro_rules! by_value { ($value:expr) => { $value.into_iter() }; }
