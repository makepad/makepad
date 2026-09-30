//! Material pixel tests: grab the lab's hidden window once every material's
//! pipeline is ready and check each cube's column by what it must show.

use makepad_test::{makepad_test, run_with_config, TestApp, TestConfig, TestError};
use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
use makepad_zune_png::PngDecoder;

struct Image {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

impl Image {
    fn read(path: &std::path::Path) -> Image {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("cannot read grab {}: {e}", path.display()));
        let mut decoder = PngDecoder::new(ZCursor::new(&bytes));
        let pixels = decoder.decode_raw().unwrap_or_else(|e| panic!("cannot decode grab: {e:?}"));
        let (width, height) = decoder.dimensions().expect("grab has no dimensions");
        let components = decoder.colorspace().expect("grab has no colorspace").num_components();
        let mut rgba = vec![0u8; width * height * 4];
        for i in 0..width * height {
            let src = i * components;
            rgba[i * 4..i * 4 + 3].copy_from_slice(&pixels[src..src + 3]);
            rgba[i * 4 + 3] = if components == 4 { pixels[src + 3] } else { 255 };
        }
        Image { width, height, rgba }
    }

    fn px(&self, x: usize, y: usize) -> [i32; 3] {
        let p = (y * self.width + x) * 4;
        [self.rgba[p] as i32, self.rgba[p + 1] as i32, self.rgba[p + 2] as i32]
    }

    /// The first row below the window's title bar (the grab includes it).
    fn content_top(&self) -> usize {
        (0..self.height).find(|&y| { let p = self.px(4, y); (p[0] - 80).abs() > 12 || (p[2] - 80).abs() > 12 }).unwrap_or(0)
    }

    /// Pixels per metre at the row's distance, on screen.
    fn per_metre(&self) -> f32 {
        (self.height - self.content_top()) as f32 / (2.0 * 9.0 * 15f32.to_radians().tan())
    }

    /// Pixels of column `c`, centred on its cube. The 800x300 pass is drawn
    /// stretched to the window, so horizontal places are fractions of the
    /// pass's own width.
    fn column(&self, c: usize) -> Vec<(usize, [i32; 3])> {
        let per_frac = 1.4 * (300.0 / (2.0 * 9.0 * 15f32.to_radians().tan())) / 800.0;
        let cx = self.width as f32 * (0.5 + (c as f32 - 3.0) * per_frac);
        let half = self.width as f32 * per_frac * 0.45;
        let (x0, x1) = ((cx - half).max(0.0) as usize, ((cx + half) as usize).min(self.width));
        let mut out = Vec::new();
        for y in self.content_top()..self.height {
            for x in x0..x1 {
                out.push((y, self.px(x, y)));
            }
        }
        out
    }
}

/// The pass clear colour (0.05, 0.06, 0.09): anything else is a cube.
fn is_background(p: [i32; 3]) -> bool {
    (p[0] - 13).abs() <= 6 && (p[1] - 15).abs() <= 6 && (p[2] - 23).abs() <= 6
}

fn fraction(col: &[(usize, [i32; 3])], pred: impl Fn([i32; 3]) -> bool) -> f32 {
    col.iter().filter(|(_, p)| pred(*p)).count() as f32 / col.len().max(1) as f32
}

/// The mean row of a column's non-background pixels.
fn centroid_y(col: &[(usize, [i32; 3])]) -> f32 {
    let hits: Vec<usize> = col.iter().filter(|(_, p)| !is_background(*p)).map(|(y, _)| *y).collect();
    hits.iter().sum::<usize>() as f32 / hits.len().max(1) as f32
}

#[makepad_test]
fn each_material_hook_shows_in_its_column(app: TestApp) {
    app.wait_for_log_contains("material lab: ready");
    std::thread::sleep(std::time::Duration::from_millis(800));
    let path = app.screenshot();
    println!("[material_lab] grab: {}", path.display());
    let img = Image::read(&path);
    let cols: Vec<_> = (0..7).map(|c| img.column(c)).collect();
    let cube = |c: usize| fraction(&cols[c], |p| !is_background(p));
    for c in 0..7 {
        println!("[material_lab] column {c}: {:.3} covered, centroid row {:.1}", cube(c), centroid_y(&cols[c]));
        assert!(cube(c) > 0.03, "column {c} drew nothing");
    }
    let red = |p: [i32; 3]| p[0] > 150 && p[1] < 60 && p[2] < 60;
    let green = |p: [i32; 3]| p[1] > 150 && p[0] < 60 && p[2] < 60;
    let magenta = |p: [i32; 3]| p[0] > 150 && p[2] > 150 && p[1] < 80;
    let blue = |p: [i32; 3]| p[2] > 150 && p[0] < 60 && p[1] < 60;
    let yellow = |p: [i32; 3]| p[0] > 140 && p[1] > 140 && p[2] < p[0] - 50;
    let grey = |p: [i32; 3]| !is_background(p) && (p[0] - p[1]).abs() < 30 && (p[1] - p[2]).abs() < 40;
    assert!(fraction(&cols[0], grey) > 0.02, "stock lane: a lit grey cube");
    assert!(fraction(&cols[0], red) + fraction(&cols[0], green) + fraction(&cols[0], blue) < 0.005, "stock lane untouched by the hooks");
    assert!(fraction(&cols[1], red) > 0.8 * cube(1), "finish: red");
    assert!(fraction(&cols[2], green) > 0.8 * cube(2), "unlit surface: flat green");
    assert!(fraction(&cols[3], magenta) > 0.2 * cube(3), "error material: magenta hatching");
    assert!(fraction(&cols[5], blue) > 0.8 * cube(5), "lighting: blue");
    assert!(fraction(&cols[6], yellow) > 0.5 * cube(6), "light: yellow direct light");
    // The vertex hook lifts its cube 0.9 m: its pixels sit well above the
    // stock cube's (rows count down the image).
    let per_metre = img.per_metre();
    let lift = centroid_y(&cols[0]) - centroid_y(&cols[4]);
    assert!(lift > 0.6 * per_metre, "vertex: lifted {lift:.1} px, want about {:.1}", 0.9 * per_metre);
}

/// Grab the lab drawing a `World` scene (`--scene=<name>`) once ready.
fn world_scene(name: &str, test_name: &str) -> Image {
    let mut config = TestConfig::current_package(env!("CARGO_MANIFEST_DIR"), env!("CARGO_PKG_NAME"), test_name).unwrap();
    config.app_args.push(format!("--scene={name}"));
    let mut out = None;
    run_with_config(config, |app: TestApp| -> Result<(), TestError> {
        app.wait_for_log_contains("material lab: ready, world scene");
        std::thread::sleep(std::time::Duration::from_millis(800));
        let path = app.screenshot();
        println!("[material_lab] {name} grab: {}", path.display());
        out = Some(Image::read(&path));
        Ok(())
    })
    .unwrap();
    out.unwrap()
}

fn luma(p: [i32; 3]) -> f32 {
    0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32
}

/// The mean luma of a column's non-background pixels.
fn mean_luma(col: &[(usize, [i32; 3])]) -> f32 {
    let lit: Vec<f32> = col.iter().filter(|(_, p)| !is_background(*p)).map(|(_, p)| luma(*p)).collect();
    lit.iter().sum::<f32>() / lit.len().max(1) as f32
}

fn brightest(col: &[(usize, [i32; 3])]) -> f32 {
    col.iter().map(|(_, p)| luma(*p)).fold(0.0, f32::max)
}

#[test]
fn world_items_and_a_rect_area_light_draw_in_the_dark() {
    let img = world_scene("rect", "pixels::world_items_and_a_rect_area_light_draw_in_the_dark");
    let cols: Vec<_> = (0..4).map(|c| img.column(c)).collect();
    for c in 0..4 {
        println!("[material_lab] rect column {c}: brightest {:.1}, mean {:.1}", brightest(&cols[c]), mean_luma(&cols[c]));
    }
    let cyan = |p: [i32; 3]| p[1] > 150 && p[2] > 150 && p[0] < 60;
    let red = |p: [i32; 3]| p[0] > 150 && p[1] < 60 && p[2] < 60;
    // Means over the cube pixels: a band's edges can catch a neighbour's
    // highlight.
    assert!(mean_luma(&cols[0]) > 60.0, "the rect light lights its cube");
    assert!(mean_luma(&cols[1]) < 15.0, "no light, no world sun: a dark cube");
    assert!(fraction(&cols[2], cyan) > 0.02, "an Unlit item draws its colour in the dark");
    assert!(fraction(&cols[3], red) > 0.02, "a packed Instances item draws its tinted copies");
}

#[test]
fn image_based_lighting_reflects_its_environment() {
    let img = world_scene("ibl", "pixels::image_based_lighting_reflects_its_environment");
    let cols: Vec<_> = (0..3).map(|c| img.column(c)).collect();
    for c in 0..3 {
        println!("[material_lab] ibl column {c}: brightest {:.1}, mean {:.1}", brightest(&cols[c]), mean_luma(&cols[c]));
    }
    // Both metal cubes reflect the sunset (no other light is on), so both
    // are lit; the environment is warm at the horizon.
    for c in 0..2 {
        assert!(mean_luma(&cols[c]) > 25.0, "column {c}: an IBL metal cube reflects its environment");
    }
    let warm = cols[0].iter().filter(|(_, p)| !is_background(*p)).filter(|(_, p)| p[0] > p[2] + 10).count();
    assert!(warm > 50, "the sunset's warm horizon shows in the reflection");
}
