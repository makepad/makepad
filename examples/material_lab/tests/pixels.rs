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

use makepad_draw::{vec4f, Mat4f};
use makepad_render::traced_volume::{TracedVolumeTile, VolumePage};

#[derive(Debug, PartialEq)]
struct VolumeCapture {
    view: Mat4f,
    projection: Mat4f,
    transform: Mat4f,
    pitch: f32,
    tile_cells: u32,
    dims: [u32; 3],
    tiles: Vec<TracedVolumeTile>,
    cells: Vec<u8>,
    coarse: Vec<u8>,
    cube: Option<Mat4f>,
}

impl VolumeCapture {
    fn read(dir: &std::path::Path, revision: usize) -> Self {
        use makepad_draw::makepad_platform::makepad_micro_serde::DeBin;
        let bytes = std::fs::read(dir.join(format!("{revision}.volume"))).unwrap();
        let mut pos = 0;
        let (view, projection, transform) = DeBin::de_bin(&mut pos, &bytes).unwrap();
        let (pitch, tile_cells, dims) = DeBin::de_bin(&mut pos, &bytes).unwrap();
        let count = usize::de_bin(&mut pos, &bytes).unwrap();
        let tiles = (0..count).map(|_| {
            let (origin_cells, extent, atlas_origin, palette_page) = DeBin::de_bin(&mut pos, &bytes).unwrap();
            TracedVolumeTile { origin_cells, extent, atlas_origin, palette_page }
        }).collect();
        let cells = Vec::de_bin(&mut pos, &bytes).unwrap();
        let coarse = Vec::de_bin(&mut pos, &bytes).unwrap();
        let cube = Option::<[f32; 16]>::de_bin(&mut pos, &bytes).unwrap().map(|v| Mat4f { v });
        assert_eq!(pos, bytes.len(), "all capture metadata consumed");
        Self { view: Mat4f { v: view }, projection: Mat4f { v: projection }, transform: Mat4f { v: transform },
            pitch, tile_cells, dims, tiles, cells, coarse, cube }
    }

    fn clip_from_model(&self) -> Mat4f {
        Mat4f::mul(&self.projection, &Mat4f::mul(&self.view, &self.transform))
    }

    fn pixel(&self, p: [f32; 3]) -> usize {
        let q = self.clip_from_model().transform_vec4(vec4f(p[0], p[1], p[2], 1.0));
        let x = ((1.0 + q.x / q.w) * 0.5 * TraceImage::W as f32).floor() as usize;
        let y = ((1.0 - q.y / q.w) * 0.5 * TraceImage::H as f32).floor() as usize;
        assert!(x < TraceImage::W && y < TraceImage::H, "probe outside capture");
        TraceImage::at(x, y)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct VolumeFace { tile: usize, axis: u8, positive: bool, material: u8 }

// Perspective projection of the corners bounds every ray through this outside proxy.
fn proxy_bounds(matrix: &Mat4f, tile: &TracedVolumeTile, pitch: f32) -> [f32; 4] {
    let mut bounds = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
    for corner in 0..8 {
        let [x,y,z] = [0,1,2].map(|a| pitch * (tile.origin_cells[a] +
            if corner & (1 << a) == 0 { 0 } else { i32::from(tile.extent[a]) }) as f32);
        let q = matrix.transform_vec4(vec4f(x,y,z,1.0));
        assert!(q.w > 0.0, "outside proxy spans the eye");
        let x = (1.0 + q.x / q.w) * (TraceImage::W as f32 * 0.5);
        let y = (1.0 - q.y / q.w) * (TraceImage::H as f32 * 0.5);
        bounds = [bounds[0].min(x), bounds[1].min(y), bounds[2].max(x), bounds[3].max(y)];
    }
    bounds
}

fn proxy_entry(p: [f32; 3], d: [f32; 3], extent: [u8; 3]) -> Option<f32> {
    let (mut enter, mut exit) = (0.0f32, f32::INFINITY);
    for a in 0..3 {
        let hi = f32::from(extent[a]);
        if d[a] == 0.0 {
            if !(0.0..hi).contains(&p[a]) { return None; }
        } else {
            let t0 = -p[a] / d[a];
            let t1 = (hi - p[a]) / d[a];
            enter = enter.max(t0.min(t1));
            exit = exit.min(t0.max(t1));
        }
    }
    (enter < exit).then_some(enter)
}

fn replay_volume(capture: &VolumeCapture, label: &str) -> Vec<Option<VolumeFace>> {
    let (w, h) = (TraceImage::W, TraceImage::H);
    let page = VolumePage { dims: capture.dims, cells: &capture.cells, coarse: &capture.coarse };
    let clip = capture.clip_from_model();
    let inverse = clip.invert();
    let eye = Mat4f::mul(&capture.view, &capture.transform).invert().transform_vec4(vec4f(0.0, 0.0, 0.0, 1.0));
    let eye_cells = [eye.x, eye.y, eye.z].map(|v| v / capture.pitch);
    let bounds: Vec<_> = capture.tiles.iter().map(|t| proxy_bounds(&clip, t, capture.pitch)).collect();
    let mut faces = vec![None; w*h];
    let mut samples: [Vec<u32>; 3] = Default::default();
    let (mut raw, mut early_z) = (0usize, 0usize);
    let mut steps = Vec::with_capacity(capture.tiles.len());
    for y in 0..h {
        for x in 0..w {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let covers = |b: &[f32; 4]| b[0] <= px && px <= b[2] && b[1] <= py && py <= b[3];
            if !bounds.iter().any(covers) { continue; }
            let q = inverse.transform_vec4(vec4f(2.0*px / w as f32 - 1.0, 1.0 - 2.0*py / h as f32, 0.0, 1.0));
            let ray = (q.to_vec3f() / q.w - eye.to_vec3f()).normalize();
            let d = [ray.x, ray.y, ray.z];
            let mut depth = f32::INFINITY;
            let mut face = None;
            steps.clear();
            for (i, tile) in capture.tiles.iter().enumerate() {
                if !covers(&bounds[i]) { continue; }
                let p = [0,1,2].map(|a| eye_cells[a] - tile.origin_cells[a] as f32);
                let Some(entry) = proxy_entry(p, d, tile.extent) else { continue };
                raw += 1;
                // Ray distance orders depth exactly as window Z on this pixel. The
                // backdrop is behind every box and the stock sphere is beside them.
                if entry > depth { continue; }
                early_z += 1;
                match tile.march_counted(&page, p, d, capture.tile_cells, true) {
                    Ok(hit) => {
                        steps.push(hit.steps);
                        if hit.t <= depth {
                            depth = hit.t;
                            face = Some(VolumeFace { tile: i, axis: hit.axis, positive: d[hit.axis as usize] < 0.0, material: hit.material });
                        }
                    }
                    Err(count) => steps.push(count),
                }
            }
            if steps.is_empty() { continue; }
            faces[y*w+x] = face;
            // Group all marches for a pixel by its final view: shallow incidence
            // on a visible face, through the front opening, or the exterior shell.
            let front_t = (8.0 - eye_cells[2]) / d[2];
            let front_x = eye_cells[0] + d[0]*front_t;
            let front_y = eye_cells[1] + d[1]*front_t;
            let category = if face.is_some_and(|f| d[f.axis as usize].abs() < 0.35) { 2 }
                else if (-10.0..8.0).contains(&front_x) && (-7.0..6.0).contains(&front_y) { 1 } else { 0 };
            samples[category].extend_from_slice(&steps);
        }
    }
    let summary = |mut v: Vec<u32>| {
        v.sort_unstable();
        assert!(!v.is_empty(), "missing M1 population");
        [v[(v.len()-1)/2], v[(v.len()-1)*95/100], *v.last().unwrap()]
    };
    let all: Vec<_> = samples.iter().flatten().copied().collect();
    assert_eq!(all.len(), early_z);
    let [exterior, interior, grazing] = samples.map(summary);
    println!("M1 {label} steps p50/p95/max overall={:?} exterior={exterior:?} interior/aperture={interior:?} grazing={grazing:?}", summary(all));
    println!("M2 {label} pixels={} raw={} ({:.5}/pixel) earlyz={} ({:.5}/pixel)", w*h, raw, raw as f64/(w*h) as f64, early_z, early_z as f64/(w*h) as f64);
    assert!(raw > early_z && early_z > 0, "overlapping room proxies exercise early Z");

    faces
}

fn volume_parity(capture: &VolumeCapture, image: &TraceImage, omitted: &TraceImage, surface: &[u8], label: &str) {
    use std::collections::BTreeMap;
    let start = std::time::Instant::now();
    let faces = replay_volume(capture, label);
    let (w,h) = (TraceImage::W, TraceImage::H);
    assert_eq!(surface.len(), w*h*4);
    let mut classes = BTreeMap::<VolumeFace, (usize, usize)>::new();
    let (mut interior, mut correct, mut hits, mut matched_hits) = (0,0,0,0);
    for y in 1..h-1 {
        for x in 1..w-1 {
            let i = y*w+x;
            let signature = |f: Option<VolumeFace>| f.map(|f| (f.axis, f.positive, f.material));
            if (y-1..=y+1).any(|v| (x-1..=x+1).any(|u| signature(faces[v*w+u]) != signature(faces[i]))) { continue; }
            interior += 1;
            let expected_id = if faces[i].is_some() { 5 } else { omitted.pick[i] };
            correct += usize::from(image.pick[i] == expected_id);
            if let Some(f) = faces[i] {
                hits += 1;
                let count = classes.entry(f).or_default();
                count.0 += 1;
                let tile = capture.tiles[f.tile];
                let color = [(40 + 48*u32::from(tile.palette_page)) as f32 / 255.0,
                    (25 + 30*u32::from(f.material)) as f32 / 255.0];
                let mut normal = [0.0; 3];
                normal[f.axis as usize] = if f.positive { 1.0 } else { -1.0 };
                let n = capture.transform.transform_vec4(vec4f(normal[0],normal[1],normal[2],0.0)).to_vec3f().normalize();
                let (nx,ny) = makepad_render::skin::oct_encode(n);
                let oct = [nx,ny].map(|v| ((v*0.5+0.5)*255.0).round() as u8);
                if image.pick[i] == 5 && (0..2).all(|c| (image.rgb[i][c]-color[c]).abs() < 0.012
                    && surface[4*i+c].abs_diff(oct[c]) <= 1) && surface[4*i+3] == 255 {
                    count.1 += 1;
                    matched_hits += 1;
                }
            }
        }
    }
    let qualifying = classes.values().filter(|(pixels, _)| *pixels >= 4).count();
    println!("parity {label}: {correct}/{interior} interior picks, {matched_hits}/{hits} interior tile/material/normal matches, {qualifying} qualifying faces, replay {:?}", start.elapsed());
    assert!(correct * 1000 > interior * 995, "{label}: {correct}/{interior} interior picks");
    for (face,(expected,found)) in &classes {
        assert!(*expected < 4 || *found > 0, "{label}: missing {face:?}, {expected} CPU interior pixels");
    }
    assert!(hits > 0 && matched_hits * 1000 > hits * 995, "{label}: {matched_hits}/{hits} interior face matches");
}

#[test]
fn material_lab_traced_volume_cpu_gpu_parity() {
    use makepad_test::KeyCode::*;
    let mut config = TestConfig::current_package(env!("CARGO_MANIFEST_DIR"), env!("CARGO_PKG_NAME"),
        "pixels::material_lab_traced_volume_cpu_gpu_parity").unwrap();
    let dir = config.artifacts_dir.clone();
    config.app_args.push("--scene=traced-volume".into());
    config.app_args.push(format!("--trace-captures={}", dir.display()));
    run_with_config(config, |app: TestApp| -> Result<(), TestError> {
        if !traced_scene_available(&app, &dir, "volume")? {
            println!("volume unavailable: verified the diagnostic, stock geometry, and identical omitted capture");
            return Ok(());
        }
        let picture = |key, rev, view| {
            app.press_key(key);
            app.wait_for_log_contains(&format!("material lab: traced rendered {rev} {view} "));
            TraceImage::read(&dir, rev, false)
        };
        let base = TraceImage::read(&dir, 1, false);
        let full = VolumeCapture::read(&dir, 1);
        let edited = picture(KeyX, 2, "Traced");
        let clear = VolumeCapture::read(&dir, 2);
        assert_eq!(full.cells.iter().zip(&clear.cells).filter(|(a,b)| a != b).count(), 64);
        assert_eq!(full.coarse.iter().zip(&clear.coarse).filter(|(a,b)| a != b).count(), 1);
        let restored = picture(KeyX, 3, "Traced");
        let omitted = picture(KeyO, 4, "Omitted");
        // Projection of the edited pillar centre, its sun ray on z=-1.5,
        // and a neighbouring parallel ray outside the pillar's footprint.
        let hole = full.pixel([-0.2, 0.2, 0.2]);
        assert_eq!((base.pick[hole], edited.pick[hole]), (5,1), "X opens the pillar to the backdrop");
        let shadow = full.pixel([-0.2,-0.65,-1.5]);
        assert_eq!(restored.pick[hole], 5, "restoring the coarse block restores the visible pillar");
        let neighbour = full.pixel([-0.52,-0.65,-1.5]);
        for (i, blocked) in [(shadow,true),(neighbour,false)] {
            assert_eq!((base.pick[i], edited.pick[i], omitted.pick[i]), (1,1,1), "backdrop probe ({},{})", i%TraceImage::W, i/TraceImage::W);
            for c in 0..3 {
                let lit = omitted.rgb[i][c];
                let before = base.rgb[i][c];
                let after = edited.rgb[i][c];
                assert!(lit > 0.003 && if blocked { before < 0.85*lit } else { (before-lit).abs() < 0.003 },
                    "pillar shadow at ({},{}): {before}, omitted {lit}", i%TraceImage::W, i/TraceImage::W);
                assert!((restored.rgb[i][c]-before).abs() < 0.003, "restoring the block restores its shadow");
                assert!((after-lit).abs() < 0.003, "edited shadow at ({},{}): {after}, omitted {lit}", i%TraceImage::W, i/TraceImage::W);
            }
        }
        // S captures the renderer's world-normal output independently of RGB/pick.
        // The two shadow captures above differ only in uploaded atlas bytes.
        let mesh = picture(KeyM, 5, "Mesh");
        assert_eq!(mesh.pick[full.pixel([0.0; 3])], 2, "M substitutes the reference mesh");
        let failed = picture(KeyF, 6, "Failed");
        assert!(failed.pick == omitted.pick && failed.rgb == omitted.rgb, "failed volume draws only the stock scene");
        picture(KeyT, 7, "Traced");
        let normal_full = picture(KeyS, 8, "Traced");
        let normal_clear = picture(KeyX, 9, "Traced");
        for (label, input, image, normals, revision) in [
            ("full", &full, &base, normal_full, 8), ("edited", &clear, &edited, normal_clear, 9),
        ] {
            assert_eq!(*input, VolumeCapture::read(&dir, revision), "normal replay must use identical CPU data");
            assert!(image.pick == normals.pick, "normal replay must show the same geometry");
            volume_parity(input, image, &omitted, &std::fs::read(dir.join(format!("{revision}.surface"))).unwrap(), label);
        }
        Ok(())
    }).unwrap();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NearSurface {
    Background(u8),
    Cube,
    Voxel(VolumeFace),
}

impl NearSurface {
    // A coplanar same-material seam is interior even where the palette page changes.
    // Keep those pixels in the crack check, as in the exterior replay.
    fn same_interior(self, other: Self) -> bool {
        match (self, other) {
            (Self::Voxel(a), Self::Voxel(b)) => (a.axis, a.positive, a.material) == (b.axis, b.positive, b.material),
            (Self::Cube, Self::Cube) | (Self::Background(_), Self::Background(_)) => true,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct NearPixel {
    surface: NearSurface,
    voxel_t: f32,
    cube_t: f32,
}

// The vertex cull uses the model-space near plane and a margin of 0.05 trace cells.
// Evaluate clip.z at all eight corners, independently of the shader's extreme-corner selection.
fn straddling_tiles(capture: &VolumeCapture) -> usize {
    let clip = capture.clip_from_model();
    let normal = makepad_draw::vec3f(clip.v[2], clip.v[6], clip.v[10]);
    let margin = normal.length() * (0.05 * capture.pitch);
    capture.tiles.iter().filter(|tile| {
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for corner in 0..8 {
            let [x, y, z] = [0, 1, 2].map(|a| {
                let offset = if corner & (1 << a) == 0 { 0 } else { i32::from(tile.extent[a]) };
                (tile.origin_cells[a] + offset) as f32 * capture.pitch
            });
            let z = clip.transform_vec4(vec4f(x, y, z, 1.0)).z;
            min = min.min(z);
            max = max.max(z);
        }
        min <= margin && max >= -margin
    }).count()
}

fn replay_from_near(capture: &VolumeCapture, omitted: &TraceImage) -> Vec<NearPixel> {
    let (w, h) = (TraceImage::W, TraceImage::H);
    let inverse = capture.clip_from_model().invert();
    let eye = Mat4f::mul(&capture.view, &capture.transform).invert().transform_vec4(vec4f(0.0, 0.0, 0.0, 1.0));
    let cube_local = capture.cube.map(|cube| Mat4f::mul(&cube.invert(), &capture.transform));
    let page = VolumePage { dims: capture.dims, cells: &capture.cells, coarse: &capture.coarse };
    let mut result = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let q = inverse.transform_vec4(vec4f(2.0 * (x as f32 + 0.5) / w as f32 - 1.0,
                1.0 - 2.0 * (y as f32 + 0.5) / h as f32, 0.0, 1.0));
            let near = q.to_vec3f() / q.w;
            let ray = (near - eye.to_vec3f() / eye.w).normalize();
            // Both intersections return distance along the ray in model metres.
            let cube_t = cube_local.and_then(|matrix| {
                let p = matrix.transform_vec4(near.to_vec4());
                let d = matrix.transform_vec4(vec4f(ray.x, ray.y, ray.z, 0.0));
                proxy_entry([p.x + 1.0, p.y + 1.0, p.z + 1.0], [d.x, d.y, d.z], [2; 3])
            }).unwrap_or(f32::INFINITY);
            let cells = near / capture.pitch;
            let d = [ray.x, ray.y, ray.z];
            let mut voxel_t = f32::INFINITY;
            let mut face = None;
            for (i, tile) in capture.tiles.iter().enumerate() {
                let origin = tile.origin_cells;
                let p = [cells.x - origin[0] as f32, cells.y - origin[1] as f32, cells.z - origin[2] as f32];
                if let Some(hit) = tile.march(&page, p, d, capture.tile_cells, true) {
                    let t = hit.t * capture.pitch;
                    if t < voxel_t {
                        voxel_t = t;
                        face = Some(VolumeFace { tile: i, axis: hit.axis, positive: d[hit.axis as usize] < 0.0, material: hit.material });
                    }
                }
            }
            let surface = if cube_t < voxel_t { NearSurface::Cube }
                else if let Some(face) = face { NearSurface::Voxel(face) }
                else { NearSurface::Background(omitted.pick[y*w+x]) };
            result.push(NearPixel { surface, voxel_t, cube_t });
        }
    }
    result
}

fn near_parity(label: &str, inside: u64, capture: &VolumeCapture, image: &TraceImage, omitted: &TraceImage) {
    let (w, h) = (TraceImage::W, TraceImage::H);
    let rays = replay_from_near(capture, omitted);
    let straddling = straddling_tiles(capture);
    assert_eq!(capture.tiles.len(), 4);
    assert_eq!(capture.cube.is_some(), label != "outside");
    assert_eq!(inside, u64::from(label != "outside"), "{label}: recorded Inside draws");
    assert!(!omitted.pick.contains(&5), "{label}: O must omit the volume");
    let mut cpu_tiles = vec![false; capture.tiles.len()];
    let mut gpu_tiles = cpu_tiles.clone();
    for p in &rays {
        if let NearSurface::Voxel(f) = p.surface { cpu_tiles[f.tile] = true; }
    }
    let (mut interior, mut correct, mut sliced_solid_hits) = (0, 0, 0);
    let (mut cube_in_front_of_voxel, mut voxel_in_front_of_cube) = (0, 0);
    let mut cracks = 0;
    let mut first_crack = None;
    let mut first_mismatch = None;
    for y in 1..h-1 {
        for x in 1..w-1 {
            let i = y*w+x;
            let p = rays[i];
            if (y-1..=y+1).any(|v| (x-1..=x+1).any(|u| !rays[v*w+u].surface.same_interior(p.surface))) { continue; }
            interior += 1;
            let pick = image.pick[i];
            let matched = match p.surface {
                NearSurface::Background(id) => pick == id && id != 4,
                NearSurface::Cube => {
                    if pick == 4 && omitted.pick[i] == 4 && p.voxel_t.is_finite() { cube_in_front_of_voxel += 1; }
                    pick == 4
                }
                NearSurface::Voxel(f) => {
                    let red = f32::from(40 + 48*capture.tiles[f.tile].palette_page) / 255.0;
                    let green = f32::from(25 + 30*f.material) / 255.0;
                    let matched = pick == 5 && (image.rgb[i][0]-red).abs() < 0.012 && (image.rgb[i][1]-green).abs() < 0.012;
                    if matched {
                        gpu_tiles[f.tile] = true;
                        if p.voxel_t == 0.0 { sliced_solid_hits += 1; }
                        if p.voxel_t < p.cube_t && p.cube_t.is_finite() && omitted.pick[i] == 4 { voxel_in_front_of_cube += 1; }
                    }
                    if pick == omitted.pick[i] {
                        cracks += 1;
                        first_crack.get_or_insert((x, y, p, pick, image.rgb[i]));
                    }
                    matched
                }
            };
            if !matched { first_mismatch.get_or_insert((x, y, p, pick, image.rgb[i], omitted.pick[i])); }
            correct += usize::from(matched);
        }
    }
    println!("{label} inside={inside} interior={interior} agreement={correct}/{interior} straddling={straddling} sliced_solid_hits={sliced_solid_hits} cube_in_front_of_voxel={cube_in_front_of_voxel} voxel_in_front_of_cube={voxel_in_front_of_cube} cracks={cracks} visible_tiles={cpu_tiles:?} matched_tiles={gpu_tiles:?}");
    assert_eq!(cracks, 0, "{label}: first crack (x,y,CPU,GPU pick,RGB) {first_crack:?}");
    assert!(correct * 1000 > interior * 995, "{label}: first mismatch (x,y,CPU,GPU pick,RGB,omitted pick) {first_mismatch:?}");
    assert_eq!(gpu_tiles, cpu_tiles, "{label}: missing visible tile");
    if label == "outside" {
        assert_eq!(straddling, 0);
    } else {
        assert!(straddling > 0);
        if label == "inside" { assert!(straddling < 4, "Inside vertex cull must reject a tile"); }
        if label == "slice" { assert!(sliced_solid_hits > 0, "the near plane must cut visible solid cells"); }
        assert!(cube_in_front_of_voxel >= 50 && voxel_in_front_of_cube >= 50, "{label}: both depth orders must be visible");
    }
}

#[test]
fn material_lab_inside_and_near_plane_cut_parity() {
    use makepad_test::KeyCode::*;
    let mut config = TestConfig::current_package(env!("CARGO_MANIFEST_DIR"), env!("CARGO_PKG_NAME"),
        "pixels::material_lab_inside_and_near_plane_cut_parity").unwrap();
    let dir = config.artifacts_dir.clone();
    config.app_args.push("--scene=traced-volume".into());
    config.app_args.push(format!("--trace-captures={}", dir.display()));
    run_with_config(config, |app: TestApp| -> Result<(), TestError> {
        if !traced_scene_available(&app, &dir, "volume")? { return Ok(()); }
        let picture = |key: Option<makepad_test::KeyCode>, revision, view| -> Result<_, TestError> {
            if let Some(key) = key { app.press_key(key); }
            let prefix = format!("material lab: traced rendered {revision} {view} ");
            app.wait_for_log_contains(&prefix);
            let log = app.log_since(0)?;
            let line = log.lines.iter().find(|line| line.contains(&prefix)).unwrap();
            let inside: u64 = line.split_once(" inside=").unwrap().1.split_once(" backend=").unwrap().0.parse().unwrap();
            Ok((TraceImage::read(&dir, revision, false), VolumeCapture::read(&dir, revision), inside))
        };
        type Picture = (TraceImage, VolumeCapture, u64);
        let check = |label, (image, capture, inside): Picture, (omitted, baseline, absent): Picture| {
            assert_eq!(capture, baseline, "{label}: identical matrices, cube and volume data across T/O");
            assert_eq!(absent, 0, "{label}: omitted pass submitted an Inside draw");
            near_parity(label, inside, &capture, &image, &omitted);
        };
        check("outside", picture(None, 1, "Traced")?, picture(Some(KeyO), 2, "Omitted")?);
        let omitted = picture(Some(KeyI), 3, "Omitted")?;
        check("inside", picture(Some(KeyT), 4, "Traced")?, omitted);
        check("slice", picture(Some(KeyN), 5, "Traced")?, picture(Some(KeyO), 6, "Omitted")?);
        Ok(())
    }).unwrap();
}
