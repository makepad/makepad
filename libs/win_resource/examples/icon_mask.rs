//! Cuts an app icon rendered on an opaque white page (macOS QuickLook, see
//! tools/app_icons/render.sh) back to its rounded square: pixels outside the
//! square become transparent and edge pixels are un-blended from the white,
//! so the icon sits cleanly on any Dock or taskbar. The square is the shared
//! app icon background: x/y 64, size 896, corner radius 200 on 1024.
//!   cargo run --release -p makepad-win-resource --example icon_mask -- icon_1024.png...
use makepad_win_resource::image::Image;

fn coverage(x: u32, y: u32, scale: f32) -> f32 {
    let (lo, hi, r) = (64.0 * scale, 960.0 * scale, 200.0 * scale);
    let mut inside = 0;
    for sy in 0..4 {
        for sx in 0..4 {
            let px = x as f32 + (sx as f32 + 0.5) / 4.0;
            let py = y as f32 + (sy as f32 + 0.5) / 4.0;
            let cx = px.clamp(lo + r, hi - r);
            let cy = py.clamp(lo + r, hi - r);
            let within = px >= lo && px <= hi && py >= lo && py <= hi;
            if within && (px - cx).powi(2) + (py - cy).powi(2) <= r * r {
                inside += 1;
            }
        }
    }
    inside as f32 / 16.0
}

fn main() {
    for path in std::env::args().skip(1) {
        let png = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let mut image = Image::decode_png(&png).unwrap_or_else(|e| panic!("{path}: {e}"));
        let scale = image.width as f32 / 1024.0;
        for y in 0..image.height {
            for x in 0..image.width {
                let i = (y * image.width + x) as usize * 4;
                let a = coverage(x, y, scale);
                let pixel = &mut image.rgba[i..i + 4];
                if a == 0.0 {
                    pixel.copy_from_slice(&[0, 0, 0, 0]);
                } else if a < 1.0 {
                    for c in &mut pixel[..3] {
                        *c = ((*c as f32 - 255.0 * (1.0 - a)) / a).clamp(0.0, 255.0).round() as u8;
                    }
                    pixel[3] = (a * 255.0).round() as u8;
                }
            }
        }
        std::fs::write(&path, image.encode_png().unwrap_or_else(|e| panic!("{path}: {e}"))).unwrap();
        println!("{path}");
    }
}
