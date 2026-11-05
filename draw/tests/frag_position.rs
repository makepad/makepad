//! `fragment_position()` on the native HAL GPU backend (Metal on macOS, Vulkan on Linux, D3D11
//! on Windows): every pixel of the probe quad reads its own centre, depth and 1 / clip w.
#![cfg(not(target_arch = "wasm32"))]

mod frag_position_probe;

#[test]
fn fragment_position_is_the_pixel_its_depth_and_reciprocal_w() {
    frag_position_probe::check("offscreen");
}
