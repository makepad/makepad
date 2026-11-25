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

#[test]
fn image_based_lighting_reflects_its_environment() {
    let img = world_scene("ibl", "pixels::image_based_lighting_reflects_its_environment");
    let split = img.split();
    let all: Vec<_> = (0..3).map(|c| img.rows(c, split)).collect();
    for (r, row) in ROWS.iter().enumerate() {
        let cols: Vec<_> = all.iter().map(|rows| &rows[r]).collect();
        for c in 0..3 {
            println!("[material_lab] ibl {row} column {c}: brightest {:.1}, mean {:.1}", brightest(cols[c]), mean_luma(cols[c]));
        }
        // Both metal shapes reflect the sunset (no other light is on), so
        // both are lit; the environment is warm at the horizon.
        for c in 0..2 {
            assert!(mean_luma(cols[c]) > 25.0, "column {c}: an IBL metal {row} reflects its environment");
        }
        let warm = cols[0].iter().filter(|(_, p)| !is_background(*p)).filter(|(_, p)| p[0] > p[2] + 10).count();
        assert!(warm > 50, "the sunset's warm horizon shows in the {row}'s reflection");
    }
}

// Read the actual scene attachment (the applied HDR texture when SSR is on).
// Window decoration, DPI, compositing and resampling cannot move a boundary.
struct TraceImage {
    rgb: Vec<[f32; 3]>,
    pick: Vec<u8>,
    receiver_surface: Option<[u8; 4]>,
}

impl TraceImage {
    const W: usize = 800;
    const H: usize = 300;

    fn read(dir: &std::path::Path, revision: usize, ssr: bool) -> Self {
        let color = std::fs::read(dir.join(format!("{revision}.rgba16f"))).unwrap();
        let ids = std::fs::read(dir.join(format!("{revision}.pick"))).unwrap();
        assert_eq!(color.len(), Self::W * Self::H * 8);
        assert_eq!(ids.len(), Self::W * Self::H * 4);
        let rgb = color.chunks_exact(8).map(|p| {
            let rgb = [0, 2, 4].map(|i| makepad_draw::f16_bits_to_f32(u16::from_le_bytes([p[i], p[i+1]])));
            assert!(rgb.iter().all(|c| c.is_finite()), "non-finite scene color");
            rgb
        }).collect();
        let pick = ids.chunks_exact(4).map(|p| { assert_eq!(&p[1..3], &[0, 0]); p[0] }).collect();
        let receiver_surface = ssr.then(|| {
            let surface = std::fs::read(dir.join(format!("{revision}.surface"))).unwrap();
            assert_eq!(surface.len(), Self::W * Self::H * 4);
            let at = Self::at(545, 240) * 4;
            surface[at..at + 4].try_into().unwrap()
        });
        Self { rgb, pick, receiver_surface }
    }

    fn at(x: usize, y: usize) -> usize { y * Self::W + x }

    fn id(&self, x: usize, y: usize) -> u8 { self.pick[Self::at(x, y)] }

    // An independent RGB silhouette. Both sphere materials are copper-red,
    // and the intersecting reflective plane is blue.
    fn red(&self, x: usize, y: usize) -> bool {
        let [r, g, b] = self.rgb[Self::at(x, y)];
        r > g && r > b
    }

    fn boundary(&self, center: usize, by_pick: bool) -> Vec<bool> {
        let inside = |x, y| if by_pick { self.id(x, y) == 2 } else { self.red(x, y) };
        let mut boundary = vec![false; 201 * 201];
        for j in 1..200 {
            for i in 1..200 {
                let (x, y) = (center + i - 100, 50 + j);
                boundary[j * 201 + i] = inside(x, y) &&
                    [(x-1,y),(x+1,y),(x,y-1),(x,y+1)].iter().any(|&(u,v)| !inside(u,v));
            }
        }
        boundary
    }
}

fn within_one_pixel(a: &[bool], b: &[bool], label: &str) {
    for (from, to) in [(a, b), (b, a)] {
        assert!(from.iter().filter(|&&hit| hit).count() > 300, "{label}: missing sphere boundary");
        for (i, &hit) in from.iter().enumerate() {
            if hit {
                assert!([i, i-1, i+1, i-201, i+201].iter().any(|&j| to[j]),
                    "{label}: boundary more than one pixel away at ({}, {})", i % 201, i / 201);
            }
        }
    }
}

// Compare the change in *linear* RGB at the same receiver pixels with the
// traced sphere and with the substituted mesh. A disabled effect cannot pass
// by matching two unchanged images. Exclude silhouette quantization pixels.
fn compare_effect(label: &str, traced: (&TraceImage, &TraceImage), mesh: (&TraceImage, &TraceImage),
    region: impl Fn(usize, usize, u8) -> bool) {
    let (mut signal, mut error, mut changed, mut receivers) = (0.0, 0.0, 0, 0);
    for y in 22..289 {
        for x in 450..641 {
            let i = TraceImage::at(x, y);
            let id = mesh.0.pick[i];
            if !region(x, y, id) || [i,i-1,i+1,i-800,i+800].iter().any(|&j| {
                [traced.0,traced.1,mesh.0,mesh.1].iter().any(|p| p.pick[j] != id)
            }) { continue; }
            receivers += 1;
            let expected = (0..3).map(|c| (mesh.1.rgb[i][c] - mesh.0.rgb[i][c]).abs()).sum::<f32>();
            if expected < 0.003 { continue; }
            changed += 1;
            signal += expected;
            for c in 0..3 {
                let observed = traced.1.rgb[i][c] - traced.0.rgb[i][c];
                error += (observed - (mesh.1.rgb[i][c] - mesh.0.rgb[i][c])).abs();
            }
        }
    }
    assert!(changed > 40, "{label}: {changed} of {receivers} shared receiver pixels responded in the reference (RGB signal {signal})");
    assert!(error < 0.30 * signal, "{label}: RGB response error {error} on mesh signal {signal} ({changed} pixels)");
}

// Check the installed recipe's diagnostic and pick replay on unsupported backends.
fn traced_scene_available(app: &TestApp, dir: &std::path::Path, program: &str) -> Result<bool, TestError> {
    let first = "material lab: traced rendered 1 ";
    app.wait_for_log_contains(first);
    let log = app.log_since(0)?;
    let rendered = log.lines.iter().find_map(|line| line.split_once(first).map(|(_, state)| state)).unwrap();
    let (_, device) = rendered.split_once(" backend=").unwrap();
    let (backend, capability) = device.split_once(" volumes=").unwrap();
    let (volumes, reason) = capability.split_once(" reason=").unwrap();
    if volumes.parse::<bool>().unwrap() {
        assert!(rendered.starts_with("Traced "), "volume-capable {backend} did not render {program}: {rendered}");
        return Ok(true);
    }

    let expected_reason = match backend {
        "Vulkan" => "volume textures aren't available on Vulkan yet",
        "Direct3D 11" => "volume textures aren't available on D3D11 yet",
        "OpenGL" => "volume textures aren't available on GLES yet",
        "WebGL" => "ray-traced surfaces aren't available on WebGL2 yet",
        _ => panic!("unexpected lack of volume sampling on {backend}"),
    };
    assert!(rendered.starts_with("Failed "), "expected the traced program to fail on {backend}: {rendered}");
    assert_eq!(reason, expected_reason);
    let failed = TraceImage::read(dir, 1, false);
    app.press_key(makepad_test::KeyCode::KeyO);
    app.wait_for_log_contains("material lab: traced rendered 2 Omitted ");
    let omitted = TraceImage::read(dir, 2, false);
    assert_eq!(failed.id(255,150), 2, "failure hid the ordinary sphere");
    assert_eq!(failed.id(545,150), 1, "failed proxy covered the plane");
    assert!(failed.rgb == omitted.rgb && failed.pick == omitted.pick, "failed {program} layer changed color or picking on {backend}");
    let logs = app.log_since(0)?;
    let prefix = format!("render: traced surface {program} ");
    assert_eq!(logs.lines.iter().filter(|line| line.contains(&prefix) && line.ends_with(reason)).count(),
        1, "{program} must report its terminal reason once");
    Ok(false)
}

#[test]
fn traced_sphere_scene() {
    use makepad_test::KeyCode::*;
    let capture = |early: bool| {
        let case = if early { "early" } else { "scene" };
        let mut config = TestConfig::current_package(env!("CARGO_MANIFEST_DIR"), env!("CARGO_PKG_NAME"),
            format!("pixels::traced_sphere_{case}")).unwrap();
        config.app_args.push(format!("--scene={}", if early { "traced-early-depth" } else { "traced" }));
        let dir = config.artifacts_dir.clone();
        config.app_args.push(format!("--trace-captures={}", dir.display()));
        run_with_config(config, |app: TestApp| -> Result<(), TestError> {
            if !traced_scene_available(&app, &dir, "analytic")? { return Ok(()); }
            let (l, r) = (255, 545);
            let mut revision = 0;
            let mut view = "Traced";
            let mut ssr_on = false;
            let mut picture = |key| {
                if let Some(key) = key {
                    if key == KeyS { ssr_on = !ssr_on; }
                    view = match key {
                        KeyT => "Traced", KeyM => "Mesh", KeyW => "Waiting",
                        KeyF => "Failed", KeyO => "Omitted", _ => view,
                    };
                    app.press_key(key);
                }
                revision += 1;
                app.wait_for_log_contains(&format!("material lab: traced rendered {revision} {view} "));
                TraceImage::read(&dir, revision, ssr_on)
            };
            let base = picture(None);
            for center in [l, r] {
                assert!(base.red(center, 150) && base.id(center,150) == 2, "sphere centre must hit");
                for (dx,y) in [(85,65),(60,212)] {
                    assert!(!base.red(center+dx,y) && base.id(center+dx,y) == 1,
                        "corner miss or sphere behind the intersecting plane at ({}, {y})", center+dx);
                }
            }
            if early {
                let covered = picture(Some(KeyB));
                let costly_front = picture(Some(KeyE));
                let costly_behind = picture(Some(KeyB));
                for front in [&covered, &costly_front] {
                    assert_eq!(front.id(r,150), 3, "the depth prefill covers the centre");
                    assert_eq!(front.id(r+60,150), 2, "an uncovered sphere hit survives");
                    assert_eq!(front.id(r+85,65), 1, "an uncovered corner sees the plane");
                    let [red,green,_] = front.rgb[TraceImage::at(r,150)];
                    assert!(green > red, "the front prefill must win in color too");
                }
                for (cheap, costly) in [(&base,&costly_behind), (&covered,&costly_front)] {
                    assert_eq!(cheap.pick, costly.pick, "extra fragment work changed depth/order");
                    let max = cheap.rgb.iter().zip(&costly.rgb).flat_map(|(a,b)| (0..3).map(move |i| (a[i]-b[i]).abs()/(1.0+a[i].abs()))).fold(0.0f32,f32::max);
                    assert!(max < 0.003, "cost hook changed the picture by {max}");
                }
                return Ok(());
            }
            for by_pick in [false,true] {
                within_one_pixel(&base.boundary(l, by_pick), &base.boundary(r, by_pick),
                    if by_pick { "pick boundary" } else { "color boundary" });
            }
            let behind = picture(Some(Key1));
            let front = picture(Some(Key2));
            for center in [l,r] {
                let x = center + 40;
                assert!(behind.red(x,150) && behind.id(x,150) == 2, "sphere must win the rear depth bracket");
                assert!(!front.red(x,150) && front.id(x,150) == 3, "front bracket must win in pick and color");
            }
            let _ = picture(Some(Key0));
            let stripe = picture(Some(KeyC));
            let mesh_stripe = picture(Some(KeyM));
            let mesh = picture(Some(KeyC));
            compare_effect("cascade stripe", (&base,&stripe), (&mesh,&mesh_stripe), |_,_,id| id == 2);
            // A horizontal caster projects across the interior, with lit sphere above and below.
            let darker = |x,y| {
                let i = TraceImage::at(x,y);
                stripe.rgb[i][0] < base.rgb[i][0] * 0.85
            };
            let bands: Vec<_> = (78..203).filter(|&y| (r-26..=r+26).all(|x| darker(x,y))).collect();
            let mesh_bands: Vec<_> = (78..203).filter(|&y| (r-26..=r+26).all(|x| {
                let i = TraceImage::at(x,y);
                mesh_stripe.rgb[i][0] < mesh.rgb[i][0] * 0.85
            })).collect();
            assert!(!bands.is_empty() && bands.len() < 80 && !mesh_bands.is_empty(), "no bounded cascade stripe crosses both surfaces");
            for (&a,&b) in bands.first().zip(mesh_bands.first()).into_iter().chain(bands.last().zip(mesh_bands.last())) {
                assert!(a.abs_diff(b) <= 1, "cascade stripe moved from row {b} to {a}");
            }
            let mesh_ao = picture(Some(KeyA));
            let ao = picture(Some(KeyT));
            compare_effect("contact SSAO", (&base,&ao), (&mesh,&mesh_ao), |_,y,_| y > 173);
            let contact = (173..239).flat_map(|y| (r-60..r+60).map(move |x| TraceImage::at(x,y)));
            assert!(contact.filter(|&i| ao.rgb[i][0] < base.rgb[i][0] * 0.98).count() > 40,
                "AO must darken contact with the plane");
            let _ = picture(Some(KeyA));
            let fog = picture(Some(KeyG));
            let mesh_fog = picture(Some(KeyM));
            compare_effect("distance fog", (&base,&fog), (&mesh,&mesh_fog), |_,_,id| id == 2);
            let mesh_without_ssr = picture(Some(KeyG));
            let mesh_ssr = picture(Some(KeyS));
            let ssr = picture(Some(KeyT));
            // The ortho ray at row 240 reflects off the sloping plane toward
            // the visible face of the sphere at row 205 (about 0.36 m away).
            for (label, image) in [("mesh", &mesh_ssr), ("traced", &ssr)] {
                assert_eq!(image.id(r, 240), 1, "{label} SSR receiver pick");
                assert_eq!(image.id(r, 205), 2, "{label} SSR reflected source pick");
                let surface = image.receiver_surface.unwrap();
                // SURFACE is RGBA8: oct-encoded (0, 1.4, 1), gloss 0.88, eligible.
                assert!(surface[0].abs_diff(128) <= 1 && surface[1].abs_diff(202) <= 1
                    && surface[2].abs_diff(224) <= 1 && surface[3] == 255,
                    "{label} SSR receiver has the wrong normal, gloss or coverage: {surface:?}");
            }
            compare_effect("sphere reflection on plane", (&base,&ssr), (&mesh_without_ssr,&mesh_ssr), |x,y,id| id == 1 && x.abs_diff(r) < 65 && y > 195);
            for (x,y) in [(r+84,65),(r-84,65)] {
                assert_eq!(ssr.id(x,y), 1);
                let i = TraceImage::at(x,y);
                for k in 0..3 { assert!((ssr.rgb[i][k]-mesh_ssr.rgb[i][k]).abs() < 0.015, "proxy corner in SSR"); }
            }
            // Removing the subject with SSR live must remove its reflection on
            // plane pixels that were still the plane in the original pick map.
            let empty_ssr = picture(Some(KeyO));
            let reflected = (218..270).flat_map(|y| (r-50..r+50).map(move |x| TraceImage::at(x,y)));
            assert!(reflected.filter(|&i| mesh_ssr.pick[i] == 1 && ssr.pick[i] == 1 &&
                mesh_ssr.rgb[i][0] > empty_ssr.rgb[i][0] + 0.003 &&
                ssr.rgb[i][0] > empty_ssr.rgb[i][0] + 0.003).count() > 40,
                "the red sphere produced no attributable reflection on the receiver");
            let empty = picture(Some(KeyS));
            for (dx,y) in [(85,65),(60,212)] {
                let i = TraceImage::at(r+dx,y);
                assert_eq!(base.rgb[i], empty.rgb[i], "a miss or occluded hit overwrote the plane");
            }
            let omitted = picture(Some(KeyC));
            let waiting = picture(Some(KeyW));
            let released = picture(Some(KeyT));
            assert_eq!(released.id(r,150), 2, "releasing the held generation must draw the analytic surface");
            let failed = picture(Some(KeyF));
            for absent in [&waiting,&failed] {
                assert_eq!(absent.pick, omitted.pick, "unavailable layer appeared in pick replay");
                assert!(absent.rgb == omitted.rgb, "unavailable layer changed color or cast a shadow");
                assert_eq!(absent.id(l,150), 2, "ordinary sibling disappeared");
                assert_eq!(absent.id(r,150), 1, "proxy footprint covered the plane");
            }
            Ok(())
        }).unwrap();
    };
    capture(false);
    capture(true);
}

#[test]
fn traced_tile_hit_and_miss() {
    use makepad_test::KeyCode::*;
    let mut config = TestConfig::current_package(env!("CARGO_MANIFEST_DIR"), env!("CARGO_PKG_NAME"),
        "pixels::traced_tile_hit_and_miss").unwrap();
    let dir = config.artifacts_dir.clone();
    config.app_args.push("--scene=traced-tiles".into());
    config.app_args.push(format!("--trace-captures={}", dir.display()));
    run_with_config(config, |app: TestApp| -> Result<(), TestError> {
        if !traced_scene_available(&app, &dir, "tiles")? { return Ok(()); }
        let mut revision = 1;
        let mut picture = |key, view| {
            app.press_key(key);
            revision += 1;
            app.wait_for_log_contains(&format!("material lab: traced rendered {revision} {view} "));
            TraceImage::read(&dir, revision, false)
        };
        let at = |x| TraceImage::at(x, 150);
        let hit = |image: &TraceImage, x, emission: [u8; 3], label| {
            assert_eq!(image.id(x, 150), 2, "{label}: no tile in pick replay at {x}");
            let rgb = image.rgb[at(x)];
            for (observed, byte) in rgb.into_iter().zip(emission) {
                assert!((observed - f32::from(byte) / 255.0).abs() < 0.012,
                    "{label}: expected linear emission {emission:?}/255 at {x}, saw {rgb:?}");
            }
        };
        let base = TraceImage::read(&dir, 1, false);
        hit(&base, 464, [224,48,128], "first cell at z=0.5");
        hit(&base, 500, [32,184,80], "late hit at z=-0.25 after six empty cells");
        // Adjacent pixels straddle the box boundary. The material byte is
        // identical, but these atlas tiles select different palette pages.
        hit(&base, 544, [68,31,212], "last column of left tile");
        hit(&base, 545, [200,145,35], "first column of right tile");
        let behind = picture(Key1, "Traced");
        let front = picture(Key2, "Traced");
        let front_reference = picture(KeyO, "Omitted");
        for x in [464,500] {
            assert_eq!(behind.id(x,150), 2, "rear depth bracket covered the tile hit at {x}");
            assert_eq!(behind.rgb[at(x)], base.rgb[at(x)], "rear bracket changed hit color at {x}");
            assert_eq!(front.id(x,150), 3, "front depth bracket did not win at {x}");
            assert_eq!(front_reference.id(x,150), 3, "missing reference bracket at {x}");
            assert_eq!(front.rgb[at(x)], front_reference.rgb[at(x)], "front bracket lost color depth at {x}");
            assert_ne!(front.rgb[at(x)], base.rgb[at(x)], "front bracket did not cover the emission at {x}");
        }
        let plane = picture(Key0, "Omitted");
        let receiver = |image: &TraceImage, x, label| {
            assert_eq!(plane.id(x,150), 1, "{label}: missing reference receiver at {x}");
            assert_eq!(image.id(x,150), 1, "{label}: miss did not pick through at {x}");
            assert_eq!(image.rgb[at(x)], plane.rgb[at(x)], "{label}: miss changed receiver color at {x}");
        };
        receiver(&base, 516, "empty column inside the conservative box");
        receiver(&base, 480, "empty column before rotation");
        // The ortho camera maps pixel centres to x=(u+0.5-400)/100,
        // y=(150-v-0.5)/100. Compare each receiver to itself with both
        // casters omitted. Use the sphere fixture's 15% shadow contrast
        // and its 0.003 linear RGB noise floor.
        let shadow_factor = |image: &TraceImage, x, y, id, label: &str| -> [f32; 3] {
            assert_eq!(plane.id(x, y), id, "{label}: missing reference receiver at ({x},{y})");
            assert_eq!(image.id(x, y), id, "{label}: camera lost the receiver at ({x},{y})");
            let i = TraceImage::at(x, y);
            std::array::from_fn(|c| {
                let lit = plane.rgb[i][c];
                assert!(lit > 0.003, "{label}: reference receiver is dark at ({x},{y})");
                image.rgb[i][c] / lit
            })
        };
        let shadow = |image: &TraceImage, x, y, id, dark: bool, label: &str| {
            let i = TraceImage::at(x, y);
            for (c, factor) in shadow_factor(image, x, y, id, label).into_iter().enumerate() {
                let matches = if dark { factor < 0.85 } else { (image.rgb[i][c] - plane.rgb[i][c]).abs() < 0.003 };
                assert!(matches, "{label} at ({x},{y}): RGB {:?}, lit {:?}", image.rgb[i], plane.rgb[i]);
            }
        };

        // Along each sun ray y=-0.195+0.5*z. At x=0.895 the ray enters
        // the box at z=0.5, crosses the first pad at z=0, then hits the
        // late cell at z=-0.25 before reaching the backdrop at z=-1.5.
        shadow(&base, 489, 169, 4, false, "pad between proxy entry and late hit");
        shadow(&base, 489, 244, 1, true, "same ray behind the late hit");
        shadow(&base, 478, 229, 1, false, "sun ray through empty tile column 2");
        // At z=0 row 215 crosses material 97 at y=0.095, inside its y=[-0.375,0.375]
        // slab. Even the flank samples are >0.10 m inside its x=[1.20,1.825] shadow.
        // The near fit is 3.09 m / 1024 texels. Its 3-texel disk, 2x2 compare and
        // normal bias reach <0.02 m, so both flanks and the join see full occlusion.
        for x in [530, 560] { shadow(&base, x, 215, 1, true, "occupied flank of the tile join"); }
        let left = shadow_factor(&base, 530, 215, 1, "left flank");
        let right = shadow_factor(&base, 560, 215, 1, "right flank");
        // PBR adds (hash-0.5)*0.03 to the white receiver AO and clamps it.
        // Ambient scales by a in [0.985,1], direct light by b=0.65+0.35*a.
        // Divide both intensities by a: each is a positive sum of a constant
        // and a term scaled by b/a. Across the row their ratio can therefore
        // change by at most max(b/a)/min(b/a). Each of the two measured ratios
        // also rounds a numerator and denominator to binary16 (unit roundoff 2^-11).
        let min_ao = 1.0 - 0.03 / 2.0;
        let ao_spread = (0.65 + 0.35 * min_ao) / min_ao;
        let roundoff = 1.0 / 2048.0;
        let half_spread = (1.0 + roundoff) / (1.0 - roundoff);
        let spread = ao_spread * half_spread * half_spread;
        for x in 539..=551 {
            for (c, factor) in shadow_factor(&base, x, 215, 1, "tile join").into_iter().enumerate() {
                let lo = left[c].max(right[c]) / spread;
                let hi = left[c].min(right[c]) * spread;
                assert!((lo..=hi).contains(&factor), "tile join at ({x},215), channel {c}: factor {factor} outside [{lo}, {hi}]");
                assert!(factor < 0.85, "continuous shadow across the tile edge at ({x},215), channel {c}");
            }
        }

        let mesh_only = picture(KeyC, "Omitted");
        let both = picture(KeyT, "Traced");
        // The stock bar on the first ray lies at z=0.30, closer to the
        // sun than its pad. The second bar at z=-0.95 lies behind its pad
        // (z=-0.65) and ahead of the backdrop. Both rays cross the late slab.
        shadow(&mesh_only, 489, 169, 4, true, "near stock caster alone");
        shadow(&both, 489, 169, 4, true, "near stock depth survives the later traced draw");
        shadow(&mesh_only, 501, 201, 4, false, "far stock caster behind the second pad");
        shadow(&mesh_only, 501, 244, 1, true, "far stock caster reaches the backdrop");
        shadow(&both, 501, 201, 4, true, "near tile wins over far stock depth");
        let tile_only = picture(KeyC, "Traced");
        shadow(&tile_only, 464, 253, 1, true, "editable cell casts onto the backdrop");
        let cleared = picture(KeyX, "Traced");
        receiver(&cleared, 464, "cleared cell behind the original box");
        for (x,y) in [(451,150), (464,137), (500,150), (544,150), (545,150)] {
            let i = TraceImage::at(x, y);
            assert_eq!(cleared.pick[i], 2, "clearing one cell hid another at ({x},{y})");
            assert_eq!(cleared.rgb[i], base.rgb[i], "clearing one cell changed another at ({x},{y})");
        }
        // The edited cell spans x=[0.575,0.7], y=[-0.125,0], z=[0.375,0.5].
        // On the backdrop, y projects to y-0.5*(z+1.5). Row 253's ray
        // stays in the cleared row for the full slab; its neighbours still cast.
        shadow(&cleared, 464, 253, 1, false, "clearing one cell opens its shadow");
        for (x,y) in [(451, 253), (464, 244), (464, 264)] {
            shadow(&cleared, x, y, 1, true, "neighbour of the edited cell still casts");
        }
        let restored = picture(KeyX, "Traced");
        shadow(&restored, 464, 253, 1, true, "restored cell casts again");
        assert_eq!(restored.pick, tile_only.pick, "restoring the cell changed the visible geometry");
        for y in 0..TraceImage::H {
            for x in 0..TraceImage::W {
                let i = TraceImage::at(x, y);
                if !matches!(tile_only.pick[i], 1 | 4) { continue; }
                assert_eq!(cleared.pick[i], tile_only.pick[i], "clearing a cell hid a receiver at ({x},{y})");
                // Projected cell corners plus three pixels for the near
                // cascade's filtered edge. Newly exposed camera pixels had pick 2 above.
                let outside_edit = !((454..=473).contains(&x) && (240..=266).contains(&y));
                for c in 0..3 {
                    let before = tile_only.rgb[i][c];
                    assert!((restored.rgb[i][c] - before).abs() < 0.003, "shadow failed to restore at ({x},{y})");
                    if outside_edit {
                        assert!((cleared.rgb[i][c] - before).abs() < 0.003, "edit moved another shadow at ({x},{y})");
                    }
                }
            }
        }
        let turned = picture(KeyR, "Traced");
        // This ray now enters the left side, crosses X and Z cell planes,
        // and reaches material 6 through its X face inside depth slice 1.
        hit(&turned, 480, [32,184,80], "oblique side entry");
        // Rotation sends x to 1.45+(local_x+local_z)/sqrt(2). The late
        // slab now covers x=0.785 and the first slab leaves x=0.645.
        shadow(&restored, 464, 229, 1, true, "old silhouette before rotation");
        shadow(&turned, 464, 229, 1, false, "old silhouette clears after rotation");
        shadow(&turned, 478, 229, 1, true, "rotated late slab darkens the old gap");
        // At local z=0.125 the shared face projects to x=1.45+0.125/sqrt(2).
        // The second ray passes through the empty part of the left box
        // before its first occupied cell in the right brick.
        shadow(&turned, 553, 215, 1, true, "rotated ray enters the seam in the left brick");
        shadow(&turned, 554, 215, 1, true, "rotated ray reaches the right brick past the empty left box");
        Ok(())
    }).unwrap();
}
