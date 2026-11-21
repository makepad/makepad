pub fn mid_value() -> u32 {
    tdk_base::base_value() + 1
}

// a dependency beside tdk-base that no edit touches
pub fn side() -> u32 {
    tdk_side::side_value()
}
