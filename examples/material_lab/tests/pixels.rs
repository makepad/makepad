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

    /// The centre of column `c`. The lab projects at the window's aspect, so
    /// a metre spans as many pixels across as down.
    fn column_x(&self, c: usize) -> f32 {
        self.width as f32 * 0.5 + (c as f32 - 3.0) * 1.4 * self.per_metre()
    }

    /// Pixels of column `c` in rows `y0..y1`.
    fn column_rows(&self, c: usize, y0: usize, y1: usize) -> Vec<(usize, [i32; 3])> {
        let (cx, half) = (self.column_x(c), 1.4 * self.per_metre() * 0.45);
        let (x0, x1) = ((cx - half).max(0.0) as usize, ((cx + half) as usize).min(self.width));
        let mut out = Vec::new();
        for y in y0..y1 {
            for x in x0..x1 {
                out.push((y, self.px(x, y)));
            }
        }
        out
    }

    /// Column `c`, both rows.
    fn column(&self, c: usize) -> Vec<(usize, [i32; 3])> {
        self.column_rows(c, self.content_top(), self.height)
    }

    /// The background row between the cube row and the sphere row above it,
    /// found down the middle of column 2 (Unlit in every scene, so it shows
    /// whatever the light).
    fn split(&self) -> usize {
        let x = self.column_x(2) as usize;
        let filled = |y: usize| !is_background(self.px(x, y));
        let top = self.content_top();
        let mut y = self.height - 1;
        while y > top && !filled(y) {
            y -= 1;
        }
        while y > top && filled(y) {
            y -= 1;
        }
        let gap = y;
        while y > top && !filled(y) {
            y -= 1;
        }
        (gap + y) / 2
    }

    /// Column `c`'s cube (rows below `split`) and sphere (rows above).
    fn rows(&self, c: usize, split: usize) -> [Vec<(usize, [i32; 3])>; 2] {
        [self.column_rows(c, split, self.height), self.column_rows(c, self.content_top(), split)]
    }
}

/// The two rows of every scene, as `Image::rows` returns them.
const ROWS: [&str; 2] = ["cube", "sphere"];

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
    let split = img.split();
    let red = |p: [i32; 3]| p[0] > 150 && p[1] < 60 && p[2] < 60;
    let green = |p: [i32; 3]| p[1] > 150 && p[0] < 60 && p[2] < 60;
    let magenta = |p: [i32; 3]| p[0] > 150 && p[2] > 150 && p[1] < 80;
    let blue = |p: [i32; 3]| p[2] > 150 && p[0] < 60 && p[1] < 60;
    let yellow = |p: [i32; 3]| p[0] > 140 && p[1] > 140 && p[2] < p[0] - 50;
    let grey = |p: [i32; 3]| !is_background(p) && (p[0] - p[1]).abs() < 30 && (p[1] - p[2]).abs() < 40;
    // Every hook on both shapes; the vertex column's lifted shapes straddle
    // the split, so only its coverage is checked per row.
    let all: Vec<_> = (0..7).map(|c| img.rows(c, split)).collect();
    for (r, row) in ROWS.iter().enumerate() {
        let cols: Vec<_> = all.iter().map(|rows| &rows[r]).collect();
        let covered = |c: usize| fraction(cols[c], |p| !is_background(p));
        for c in 0..7 {
            println!("[material_lab] {row} column {c}: {:.3} covered, centroid row {:.1}", covered(c), centroid_y(cols[c]));
            assert!(covered(c) > 0.03, "{row} column {c} drew nothing");
        }
        assert!(fraction(cols[0], grey) > 0.02, "stock lane: a lit grey {row}");
        assert!(fraction(cols[0], red) + fraction(cols[0], green) + fraction(cols[0], blue) < 0.005, "stock lane {row} untouched by the hooks");
        assert!(fraction(cols[1], red) > 0.8 * covered(1), "finish: red {row}");
        assert!(fraction(cols[2], green) > 0.8 * covered(2), "unlit surface: flat green {row}");
        assert!(fraction(cols[3], magenta) > 0.2 * covered(3), "error material: magenta hatching on the {row}");
        assert!(fraction(cols[5], blue) > 0.8 * covered(5), "lighting: blue {row}");
        assert!(fraction(cols[6], yellow) > 0.5 * covered(6), "light: yellow direct light on the {row}");
    }
    // The vertex hook lifts its cube and sphere 0.9 m: their pixels sit well
    // above the stock pair's (rows count down the image).
    let cols: Vec<_> = (0..7).map(|c| img.column(c)).collect();
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
    let split = img.split();
    let all: Vec<_> = (0..4).map(|c| img.rows(c, split)).collect();
    let cyan = |p: [i32; 3]| p[1] > 150 && p[2] > 150 && p[0] < 60;
    let red = |p: [i32; 3]| p[0] > 150 && p[1] < 60 && p[2] < 60;
    for (r, row) in ROWS.iter().enumerate() {
        let cols: Vec<_> = all.iter().map(|rows| &rows[r]).collect();
        for c in 0..4 {
            println!("[material_lab] rect {row} column {c}: brightest {:.1}, mean {:.1}", brightest(cols[c]), mean_luma(cols[c]));
        }
        // Means over the shape's pixels: a band's edges can catch a
        // neighbour's highlight.
        assert!(mean_luma(cols[0]) > 60.0, "the rect light lights its {row}");
        assert!(mean_luma(cols[1]) < 15.0, "no light, no world sun: a dark {row}");
        assert!(fraction(cols[2], cyan) > 0.02, "an Unlit {row} draws its colour in the dark");
        assert!(fraction(cols[3], red) > 0.02, "a packed Instances item draws its tinted {row}s");
    }
}

/// The least mean luma of a shape the sunset lights through the IBL lookups
/// at the LANE'S scale. The lab never asks for HDR output, so on every
/// platform it draws in the legacy lane, where the lookups carry the map's
/// exposure as the rig's fill does (`ibl_ctl.y`, renderer/stock_ibl.rs; the
/// sunset meters at the ceiling, 3.2). With the scale the item pair and the
/// stock chrome pair read means of 134.8 to 145.5 (the HAL's D3D11; 131.9 to
/// 142.9 on the D3D11 backend before it); at the map's raw scale, which is
/// what a frame draws when the control is not resolved at the rig site
/// (frame.rs, `resolve_ibl_lane`) or not written on the draw (draw_models.rs,
/// `bind_ibl_lane`), 47.4 to 53.6 (measured on that older backend). No
/// headless test runs those two calls, so this is the check that fails when
/// one of them is lost: a mean near 50 is that.
const LIT_AT_THE_LANES_SCALE: f32 = 90.0;

#[test]
fn image_based_lighting_reflects_its_environment() {
    let img = world_scene("ibl", "pixels::image_based_lighting_reflects_its_environment");
    let split = img.split();
    let all: Vec<_> = (0..6).map(|c| img.rows(c, split)).collect();
    let warm = |col: &[(usize, [i32; 3])]| col.iter().filter(|(_, p)| !is_background(*p)).filter(|(_, p)| p[0] > p[2] + 10).count();
    let cool = |col: &[(usize, [i32; 3])]| col.iter().filter(|(_, p)| !is_background(*p)).filter(|(_, p)| p[2] > p[0] + 10).count();
    for (r, row) in ROWS.iter().enumerate() {
        let cols: Vec<_> = all.iter().map(|rows| &rows[r]).collect();
        for c in 0..6 {
            println!("[material_lab] ibl {row} column {c}: brightest {:.1}, mean {:.1}", brightest(cols[c]), mean_luma(cols[c]));
        }
        // Both metal item shapes reflect the sunset (no other light is on),
        // so both are lit, at the lane's scale and not the map's raw one;
        // the environment is warm at the horizon.
        for c in 0..2 {
            let mean = mean_luma(cols[c]);
            assert!(mean > LIT_AT_THE_LANES_SCALE, "column {c}: an IBL metal {row} reflects its environment at the lane's scale: mean {mean:.1}, want above {LIT_AT_THE_LANES_SCALE}");
        }
        assert!(warm(cols[0]) > 50, "the sunset's warm horizon shows in the {row}'s reflection");
        // The stock chrome pair (column 4) draws through the engine's IBL
        // program: lit and warm like the item shapes. The scene authors a
        // pale blue fog of zero density (items_world), the colour the
        // analytic lane's sky_env reflects at the horizon whatever the
        // environment is: a chrome shape left on that lane shows a cool
        // band, not the sunset.
        println!("[material_lab] ibl {row} column 4: warm {}, cool {}", warm(cols[4]), cool(cols[4]));
        assert!(fraction(cols[4], |p| !is_background(p)) > 0.03, "{row} column 4 drew nothing");
        let mean = mean_luma(cols[4]);
        assert!(mean > LIT_AT_THE_LANES_SCALE, "a stock chrome {row} reflects the environment at the lane's scale: mean {mean:.1}, want above {LIT_AT_THE_LANES_SCALE}");
        assert!(warm(cols[4]) > 50, "the stock chrome {row}'s reflection is the sunset's");
        assert!(cool(cols[4]) < warm(cols[4]), "the stock chrome {row}'s reflection is not the analytic sky_env's blue horizon");
        // The matte stock pair (column 5) keeps the diffuse lane: no light,
        // no environment term (the world's zero Sun and Sky have the last
        // word over the environment's fill, frame.rs lane_rig), a dark shape.
        assert!(fraction(cols[5], |p| !is_background(p)) > 0.03, "{row} column 5 drew nothing");
        assert!(mean_luma(cols[5]) < 15.0, "a matte stock {row} under no light stays dark");
    }
}
