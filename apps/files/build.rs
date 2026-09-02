#[path = "../../libs/ai/models/paint/src/png.rs"]
#[allow(dead_code)]
mod png;

use std::{env, fs, path::Path};

const WIDTH: u32 = 256;
const HEIGHT: u32 = 128;

const PALETTES: [(&str, [u8; 3], [u8; 3], [u8; 3]); 6] = [
    ("aurora-vignette.png", [28, 31, 78], [69, 175, 170], [227, 151, 232]),
    ("canyon-vignette.png", [86, 35, 38], [221, 119, 70], [255, 211, 128]),
    ("lagoon-vignette.png", [12, 55, 77], [28, 154, 166], [151, 232, 207]),
    ("meadow-vignette.png", [36, 68, 45], [124, 167, 79], [234, 213, 126]),
    ("twilight-vignette.png", [36, 25, 65], [103, 71, 141], [238, 144, 116]),
    ("cinema-still.png", [19, 24, 39], [56, 75, 105], [235, 176, 91]),
];

fn mix(a: u8, b: u8, amount: u32) -> u32 {
    (u32::from(a) * (255 - amount) + u32::from(b) * amount) / 255
}

fn picture(top: [u8; 3], bottom: [u8; 3], accent: [u8; 3], seed: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    let glow_x = 36 + seed * 31;
    let glow_y = 22 + (seed * 17) % 54;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let blend = (y * 190 / (HEIGHT - 1) + x * 65 / (WIDTH - 1)).min(255);
            let edge_x = (x as i32 * 2 - (WIDTH - 1) as i32).unsigned_abs();
            let edge_y = (y as i32 * 2 - (HEIGHT - 1) as i32).unsigned_abs();
            let vignette = edge_x * edge_x * 18 / ((WIDTH - 1) * (WIDTH - 1))
                + edge_y * edge_y * 24 / ((HEIGHT - 1) * (HEIGHT - 1));
            let glow_dx = (x as i32 - glow_x as i32).unsigned_abs();
            let glow_dy = (y as i32 - glow_y as i32).unsigned_abs();
            let glow = 72u32.saturating_sub(glow_dx * glow_dx / 360 + glow_dy * glow_dy / 90);
            let texture = ((x * 17 + y * 31 + seed * 43) & 7) as i32 - 3;

            for channel in 0..3 {
                let base = mix(top[channel], bottom[channel], blend);
                let lit = (base * (72 - glow) + u32::from(accent[channel]) * glow) / 72;
                pixels.push((lit as i32 - vignette as i32 + texture).clamp(0, 255) as u8);
            }
        }
    }
    pixels
}

fn write_if_changed(path: &Path, bytes: &[u8]) {
    if fs::read(path).ok().as_deref() != Some(bytes) {
        fs::write(path, bytes).expect("write generated files demo picture");
    }
}

fn main() {
    println!("cargo:rerun-if-changed=../../libs/ai/models/paint/src/png.rs");
    println!("cargo:rerun-if-changed=build.rs");
    let out_dir = env::var_os("OUT_DIR").expect("Cargo did not set OUT_DIR");
    for (seed, (name, top, bottom, accent)) in PALETTES.iter().copied().enumerate() {
        let pixels = picture(top, bottom, accent, seed as u32);
        let encoded = png::encode_png(WIDTH, HEIGHT, png::PngColor::Rgb, &pixels);
        write_if_changed(&Path::new(&out_dir).join(name), &encoded);
    }
}
