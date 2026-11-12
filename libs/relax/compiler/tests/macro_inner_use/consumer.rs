macro_rules! nested_value { () => { file_value!() }; }
pub fn value() -> u32 { nested_value!() }
