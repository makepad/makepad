//! Working-tree sprite-pass lab test (not for commit): screenshot the row of
//! six billboard recipes and say, per recipe, whether pixels arrived.

use makepad_test::{makepad_test, Selector, TestApp};
use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
use makepad_zune_png::PngDecoder;

struct Image {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

impl Image {
    fn read(path: &std::path::Path) -> Image {
        let bytes = std::fs::read(path)
            .unwrap_or_else(|err| panic!("cannot read grab {}: {err}", path.display()));
        let mut decoder = PngDecoder::new(ZCursor::new(&bytes));
        let pixels = decoder
            .decode_raw()
            .unwrap_or_else(|err| panic!("cannot decode grab {}: {err:?}", path.display()));
        let (width, height) = decoder.dimensions().expect("grab has no dimensions");
        let components = decoder
            .colorspace()
            .expect("grab has no colorspace")
            .num_components();
        let mut rgba = vec![0u8; width * height * 4];
        for i in 0..width * height {
            let src = i * components;
            rgba[i * 4] = pixels[src];
            rgba[i * 4 + 1] = pixels[src + 1];
            rgba[i * 4 + 2] = pixels[src + 2];
            rgba[i * 4 + 3] = if components == 4 { pixels[src + 3] } else { 255 };
        }
        Image {
            width,
            height,
            rgba,
        }
    }

    fn pixel(&self, x: usize, y: usize) -> [u8; 3] {
        let p = (y.min(self.height - 1) * self.width + x.min(self.width - 1)) * 4;
        [self.rgba[p], self.rgba[p + 1], self.rgba[p + 2]]
    }
}

fn is_red(p: [u8; 3]) -> bool {
    p[0] > 150 && p[1] < 90 && p[2] < 90
}
fn is_yellow(p: [u8; 3]) -> bool {
    p[0] > 150 && p[1] > 150 && p[2] < 90
}
fn is_green(p: [u8; 3]) -> bool {
    p[1] > 120 && p[0] < 100 && p[2] < 100
}
fn is_magenta(p: [u8; 3]) -> bool {
    p[0] > 150 && p[1] < 90 && p[2] > 150
}
fn is_blue(p: [u8; 3]) -> bool {
    p[2] > 150 && p[0] < 100 && p[1] < 150
}

#[makepad_test]
fn each_billboard_recipe_puts_pixels_on_screen(app: TestApp) {
    app.locator(Selector::id("lab")).wait_visible();
    // Give the pass a couple of frames to settle, then grab.
    std::thread::sleep(std::time::Duration::from_millis(600));
    let path = app.screenshot();
    println!("[spritelab] grab: {}", path.display());
    let img = Image::read(&path);

    // Six equal column bands, one per case, in submission order.
    let band_w = img.width / 6;
    let names = [
        "0 asset-ui recipe (whole uv, zw=0)      ",
        "1 sandbox barrel (half uv, zw=46x32)    ",
        "2 sandbox troo (window uv, zw=472x434)  ",
        "3 troo mirrored (u0>u1, zw=472x434)     ",
        "4 troo window, ramp OFF (zw=0)          ",
        "5 barrel recipe facing AWAY (yaw+pi)    ",
    ];
    let mut counts = [[0usize; 5]; 6];
    for band in 0..6 {
        let x0 = band * band_w;
        let x1 = (band + 1) * band_w;
        for y in 0..img.height {
            for x in x0..x1 {
                let p = img.pixel(x, y);
                if is_red(p) {
                    counts[band][0] += 1;
                }
                if is_yellow(p) {
                    counts[band][1] += 1;
                }
                if is_green(p) {
                    counts[band][2] += 1;
                }
                if is_magenta(p) {
                    counts[band][3] += 1;
                }
                if is_blue(p) {
                    counts[band][4] += 1;
                }
            }
        }
    }
    println!("[spritelab] band                                      red  yellow   green magenta    blue");
    for band in 0..6 {
        println!(
            "[spritelab] {} {:>7} {:>7} {:>7} {:>7} {:>7}",
            names[band],
            counts[band][0],
            counts[band][1],
            counts[band][2],
            counts[band][3],
            counts[band][4]
        );
    }

    let mut failures: Vec<String> = Vec::new();
    if counts[0][0] < 100 || counts[0][1] < 100 {
        failures.push(format!(
            "case 0 (asset-ui recipe) missing: red {} yellow {}",
            counts[0][0], counts[0][1]
        ));
    }
    if counts[1][0] < 100 {
        failures.push(format!(
            "case 1 (sandbox barrel recipe) missing: red {}",
            counts[1][0]
        ));
    }
    if counts[2][2] < 100 {
        failures.push(format!(
            "case 2 (SANDBOX TROO RECIPE) missing: green {}",
            counts[2][2]
        ));
    }
    if counts[3][2] < 100 {
        failures.push(format!(
            "case 3 (troo mirrored) missing: green {}",
            counts[3][2]
        ));
    }
    if counts[4][2] < 100 {
        failures.push(format!(
            "case 4 (troo, ramp off) missing: green {}",
            counts[4][2]
        ));
    }
    // Case 5 (facing away) is report-only: culling it or showing it are both
    // defensible; the row above says which one this build does.
    println!(
        "[spritelab] case 5 (facing away) red pixels: {} -> {}",
        counts[5][0],
        if counts[5][0] > 100 {
            "drawn from behind (no backface cull)"
        } else {
            "NOT drawn (culled or degenerate when facing away)"
        }
    );
    assert!(
        failures.is_empty(),
        "invisible billboard recipes:\n{}",
        failures.join("\n")
    );
}
