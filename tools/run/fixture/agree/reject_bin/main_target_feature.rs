// rejects: `main` function is not allowed to have `#[target_feature]`
#[cfg_attr(target_arch = "x86_64", target_feature(enable = "sse2"))]
#[cfg_attr(target_arch = "aarch64", target_feature(enable = "neon"))]
fn main() {}
