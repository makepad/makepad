//! `fragment_position()` on the HAL's GLES backend (Linux): every pixel of the probe quad reads
//! its own centre, depth and 1 / clip w, its rows counted from the top as on the others.
#![cfg(target_os = "linux")]

mod frag_position_probe;

#[test]
fn fragment_position_is_the_pixel_its_depth_and_reciprocal_w_on_gl() {
    frag_position_probe::check("gl");
}
