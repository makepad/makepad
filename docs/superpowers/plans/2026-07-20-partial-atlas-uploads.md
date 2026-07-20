# Backend-Specific Partial Atlas Uploads Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Benchmark and implement native partial uploads for stable RGBAf32 atlas textures on Linux desktop OpenGL, Metal, D3D11, and WebGL2.

**Architecture:** Keep the existing retained CPU atlas and `TextureUpdated::Partial(RectUsize)` contract. Move Vulkan's bounds-safe rectangle resolution onto `TextureUpdated`, reuse it in every backend, and force full uploads whenever native storage changes or a backend is not approved for partial float uploads. Reuse the Splash example as the paired full-versus-partial runtime probe.

**Tech Stack:** Rust, Makepad platform textures, OpenGL, Metal, D3D11/windows-rs, WebAssembly/WebGL2, Makepad Studio remote protocol.

## Global Constraints

- A backend passes only with at least 75% fewer submitted bytes, identical rendered output, and partial p95 no greater than 1.02 times the full control.
- Use a fixed 2048 by 64 RGBAf32 atlas; the measured dirty rectangle is x=1, y=63, width=63, height=1.
- The expected submissions are exactly 1,008 partial bytes and 2,097,152 full bytes.
- Run 30 paired warmups, then 500 measured pairs, across five release runs.
- Use release builds for tests, runtime checks, and benchmarks.
- Launch and validate UI programs only through a fresh Makepad Studio `RunItem` after clearing the prior build.
- Do not add dependencies, a telemetry subsystem, a benchmark crate, an atlas payload format, or a runtime feature flag.
- Preserve full uploads on unverified Android GLES and OpenHarmony paths.
- Preserve Vulkan upload mechanics and headless rendering behavior. Recreated
  nonzero Vulkan storage receives a forced full upload; zero logical extents
  only initialize physical max(1) image storage and have no upload rectangle.
- A backend without native hardware or browser validation remains pending; a cross-compile alone is not a pass.
- Do not stage or modify the user's untracked `LearningMakepad.md`.

## File Map

- `platform/src/texture.rs`: shared upload-rectangle resolution and focused unit tests.
- `platform/src/os/linux/vulkan.rs`: consume the shared rectangle method without changing Vulkan behavior.
- `examples/splash/src/main.rs`: paired full/partial atlas workload and visible correctness probe.
- `examples/splash/tests/ui.rs`: minimal benchmark wiring/runtime check.
- `platform/src/os/linux/opengl.rs`: desktop-only RGBAf32 `glTexSubImage2D` path.
- `platform/src/os/apple/metal.rs`: partial `MTLRegion` upload and actual byte accounting.
- `platform/src/os/windows/d3d11.rs`: stable-texture `UpdateSubresource` path.
- `libs/windows/windows-rs/src/Windows/mod.rs`: generated `UpdateSubresource` wrapper retained by the repository stripper.
- `platform/src/os/web/from_wasm.rs`: partial-region fields in the existing RGBAf32 texture message.
- `platform/src/os/web/web_gl.rs`: resolve and send the WebGL2 update region.
- `platform/src/os/web/web_gl.js`: `texSubImage2D` and pixel-store restoration.

---

### Task 1: Shared Upload Rectangle and Vulkan Migration

**Files:**
- Modify: `platform/src/texture.rs:376-405`
- Modify: `platform/src/os/linux/vulkan.rs:3629-3835`
- Test: `platform/src/texture.rs`

**Interfaces:**
- Consumes: `TextureUpdated::{Empty, Partial, Full}` and `RectUsize`.
- Produces: `TextureUpdated::upload_rect(self, width, height, force_full) -> Option<RectUsize>`.

- [ ] **Step 1: Add the failing shared tests**

Add this module at the end of `platform/src/texture.rs`:

```rust
#[cfg(test)]
mod texture_updated_upload_rect_tests {
    use super::*;

    fn rect(x: usize, y: usize, width: usize, height: usize) -> RectUsize {
        RectUsize::new(
            PointUsize::new(x, y),
            SizeUsize::new(width, height),
        )
    }

    #[test]
    fn texture_updated_upload_rect_clamps_partial_regions() {
        assert_eq!(
            TextureUpdated::Partial(rect(1, 63, 63, 1)).upload_rect(2048, 64, false),
            Some(rect(1, 63, 63, 1)),
        );
        assert_eq!(
            TextureUpdated::Partial(rect(6, 3, usize::MAX, usize::MAX))
                .upload_rect(8, 4, false),
            Some(rect(6, 3, 2, 1)),
        );
        assert_eq!(
            TextureUpdated::Partial(rect(8, 4, 1, 1)).upload_rect(8, 4, false),
            None,
        );
    }

    #[test]
    fn texture_updated_upload_rect_handles_full_empty_and_reallocation() {
        assert_eq!(TextureUpdated::Empty.upload_rect(8, 4, false), None);
        assert_eq!(
            TextureUpdated::Empty.upload_rect(8, 4, true),
            Some(rect(0, 0, 8, 4)),
        );
        assert_eq!(
            TextureUpdated::Full.upload_rect(8, 4, false),
            Some(rect(0, 0, 8, 4)),
        );
        assert_eq!(
            TextureUpdated::Partial(rect(1, 1, 2, 2)).upload_rect(8, 4, true),
            Some(rect(0, 0, 8, 4)),
        );
        assert_eq!(TextureUpdated::Full.upload_rect(0, 4, false), None);
    }
}
```

- [ ] **Step 2: Run the tests and confirm the red state**

Run:

```bash
rtk cargo test --release -p makepad-platform --lib texture_updated_upload_rect
```

Expected: compilation fails because `TextureUpdated::upload_rect` does not exist.

- [ ] **Step 3: Implement the shared method**

Add this method inside the existing `impl TextureUpdated`:

```rust
pub(crate) fn upload_rect(
    self,
    width: usize,
    height: usize,
    force_full: bool,
) -> Option<RectUsize> {
    if width == 0 || height == 0 {
        return None;
    }
    let full_rect = RectUsize::new(
        PointUsize::new(0, 0),
        SizeUsize::new(width, height),
    );
    if force_full {
        return Some(full_rect);
    }
    match self {
        Self::Empty => None,
        Self::Full => Some(full_rect),
        Self::Partial(rect) => {
            let x0 = rect.origin.x.min(width);
            let y0 = rect.origin.y.min(height);
            let x1 = rect.origin.x.saturating_add(rect.size.width).min(width);
            let y1 = rect.origin.y.saturating_add(rect.size.height).min(height);
            (x1 > x0 && y1 > y0).then(|| {
                RectUsize::new(
                    PointUsize::new(x0, y0),
                    SizeUsize::new(x1 - x0, y1 - y0),
                )
            })
        }
    }
}
```

The zero-dimension check remains first because it describes logical texture
content. For nonzero dimensions, `force_full` takes precedence over the update
variant so a backend can initialize recreated native storage from the retained
CPU texture, including when the update state is `Empty`.

- [ ] **Step 4: Switch Vulkan to the shared method**

Delete `CxVulkan::texture_upload_rect`. At each of the five existing call sites in `vec_texture_upload`, replace the local call with:

```rust
let rect = updated.upload_rect(*width, *height, force_full)?;
let x = rect.origin.x;
let y = rect.origin.y;
let w = rect.size.width;
let h = rect.size.height;
```

Keep `pack_texture_region_bytes` and every Vulkan staging/copy operation unchanged.
Vulkan separately allocates physical image extents with `max(1)`; when either
logical dimension is zero, `upload_rect` returns `None` and no copy is issued.

- [ ] **Step 5: Run shared and producer tests**

Run:

```bash
rtk cargo test --release -p makepad-platform --lib texture_updated_upload_rect
rtk cargo test --release -p makepad-draw appended_dirty_rect
```

Expected: two shared rectangle tests pass and the existing append-rectangle tests pass.

- [ ] **Step 6: Check the Vulkan target when installed**

Run:

```bash
MAKEPAD=vulkan rtk cargo check --release -p makepad-platform --target aarch64-linux-android
```

Expected: success. If the target is absent, record Vulkan compile validation as pending rather than installing a toolchain without approval.

- [ ] **Step 7: Commit the shared contract**

```bash
rtk git add platform/src/texture.rs platform/src/os/linux/vulkan.rs
rtk git commit -m "Share texture upload rectangle resolution"
```

---

### Task 2: Splash Full-versus-Partial Benchmark Probe

**Files:**
- Modify: `examples/splash/src/main.rs:1-40, 760-900, 2062-2190`
- Modify: `examples/splash/tests/ui.rs`

**Interfaces:**
- Consumes: `Texture::new_with_format`, `take_vec_f32`, `put_back_vec_f32`, `NextFrame`, `ImageRef::set_texture`.
- Produces: a visible `atlas_upload_bench_start` control, `atlas_upload_toggle` correctness control, `atlas_upload_bench_status` result, and paired non-gating `ATLAS_BENCH SCHEDULER_PROXY` log line. Backend acceptance comes from Studio profiler evidence.

- [ ] **Step 1: Add the failing UI test**

Append to `examples/splash/tests/ui.rs`:

```rust
#[makepad_test]
fn splash_atlas_upload_bench(app: TestApp) {
    app.locator(Selector::id("atlas_upload_toggle"))
        .wait_visible()
        .click();
    app.locator(Selector::id("atlas_upload_bench_status"))
        .wait_text("variant B");
    app.locator(Selector::id("atlas_upload_bench_start"))
        .wait_visible()
        .click();
    app.wait_for_log_contains("ATLAS_BENCH SCHEDULER_PROXY");
}
```

- [ ] **Step 2: Confirm the UI test is red through Studio**

With Studio running, run:

```bash
MAKEPAD_TEST_VISIBLE=1 MAKEPAD_NO_VSYNC=1 rtk cargo test --release -p makepad-example-splash --test ui splash_atlas_upload_bench -- --test-threads=1
```

Expected: failure because `atlas_upload_toggle` does not exist.

- [ ] **Step 3: Add the probe controls at the top of TabVarTtf**

Immediately after the two introductory labels in `TabVarTtf`, add:

```text
View{
    width: Fill height: Fit flow: Right spacing: 8
    atlas_upload_bench_start := Button{text: "Run atlas upload benchmark"}
    atlas_upload_toggle := Button{text: "Toggle atlas pixels"}
    atlas_upload_bench_status := Label{text: "idle" draw_text.color: #aaa}
}
View{
    width: Fill height: 64 flow: Right spacing: 8
    atlas_upload_full_image := Image{
        width: 256 height: 64 fit: ImageFit.Stretch
    }
    atlas_upload_partial_image := Image{
        width: 256 height: 64 fit: ImageFit.Stretch
    }
}
```

- [ ] **Step 4: Add the benchmark state**

Change the standard-library import to:

```rust
use std::{path::Path, time::Instant};
```

Add this state before `pub struct App`:

```rust
const ATLAS_BENCH_WIDTH: usize = 2048;
const ATLAS_BENCH_HEIGHT: usize = 64;
const ATLAS_BENCH_WARMUP_PAIRS: usize = 30;
const ATLAS_BENCH_MEASURED_PAIRS: usize = 500;
const ATLAS_BENCH_REQUESTED_PARTIAL_BYTES: u64 = 63 * 16;
const ATLAS_BENCH_REQUESTED_FULL_BYTES: u64 =
    (ATLAS_BENCH_WIDTH * ATLAS_BENCH_HEIGHT * 16) as u64;

#[derive(Clone, Copy)]
enum AtlasUploadKind {
    Full,
    Partial,
}

#[derive(Default)]
struct AtlasUploadBench {
    full_texture: Option<Texture>,
    partial_texture: Option<Texture>,
    next_frame: NextFrame,
    pending_started: Option<Instant>,
    pending_kind: Option<AtlasUploadKind>,
    pending_measured: bool,
    step: usize,
    running: bool,
    variant_b: bool,
    full_us: Vec<u64>,
    partial_us: Vec<u64>,
}

impl AtlasUploadBench {
    fn initial_data() -> Vec<f32> {
        let mut data = vec![0.0; ATLAS_BENCH_WIDTH * ATLAS_BENCH_HEIGHT * 4];
        for y in 0..ATLAS_BENCH_HEIGHT {
            for x in 0..ATLAS_BENCH_WIDTH {
                let offset = (y * ATLAS_BENCH_WIDTH + x) * 4;
                data[offset] = x as f32 / ATLAS_BENCH_WIDTH as f32;
                data[offset + 1] = y as f32 / ATLAS_BENCH_HEIGHT as f32;
                data[offset + 2] = if (x + y) & 1 == 0 { 0.25 } else { 0.75 };
                data[offset + 3] = 1.0;
            }
        }
        data
    }

    fn new_texture(cx: &mut Cx) -> Texture {
        Texture::new_with_format(
            cx,
            TextureFormat::VecRGBAf32 {
                width: ATLAS_BENCH_WIDTH,
                height: ATLAS_BENCH_HEIGHT,
                data: Some(Self::initial_data()),
                updated: TextureUpdated::Full,
            },
        )
    }

    fn dirty_rect() -> RectUsize {
        RectUsize::new(PointUsize::new(1, 63), SizeUsize::new(63, 1))
    }

    fn write_tail(cx: &mut Cx, texture: &Texture, variant_b: bool, full: bool) {
        let mut data = texture.take_vec_f32(cx);
        for x in 1..64 {
            let offset = (63 * ATLAS_BENCH_WIDTH + x) * 4;
            data[offset] = if variant_b { 1.0 } else { 0.0 };
            data[offset + 1] = if variant_b { 0.0 } else { 1.0 };
            data[offset + 2] = x as f32 / 63.0;
            data[offset + 3] = 1.0;
        }
        texture.put_back_vec_f32(
            cx,
            data,
            if full { None } else { Some(Self::dirty_rect()) },
        );
    }

    fn p95(samples: &[u64]) -> u64 {
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        sorted[(sorted.len() * 95 - 1) / 100]
    }

    fn start(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        let full = Self::new_texture(cx);
        let partial = Self::new_texture(cx);
        ui.image(cx, ids!(atlas_upload_full_image))
            .set_texture(cx, Some(full.clone()));
        ui.image(cx, ids!(atlas_upload_partial_image))
            .set_texture(cx, Some(partial.clone()));
        self.full_texture = Some(full);
        self.partial_texture = Some(partial);
        self.pending_started = None;
        self.pending_kind = None;
        self.pending_measured = false;
        self.step = 0;
        self.running = true;
        self.variant_b = false;
        self.full_us.clear();
        self.partial_us.clear();
        ui.label(cx, ids!(atlas_upload_bench_status))
            .set_text(cx, "running");
        ui.redraw(cx);
        self.next_frame = cx.new_next_frame();
    }

    fn toggle(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        if self.running {
            return;
        }
        if self.full_texture.is_none() || self.partial_texture.is_none() {
            let full = Self::new_texture(cx);
            let partial = Self::new_texture(cx);
            ui.image(cx, ids!(atlas_upload_full_image))
                .set_texture(cx, Some(full.clone()));
            ui.image(cx, ids!(atlas_upload_partial_image))
                .set_texture(cx, Some(partial.clone()));
            self.full_texture = Some(full);
            self.partial_texture = Some(partial);
        }
        self.variant_b = !self.variant_b;
        Self::write_tail(cx, self.full_texture.as_ref().unwrap(), self.variant_b, true);
        Self::write_tail(
            cx,
            self.partial_texture.as_ref().unwrap(),
            self.variant_b,
            false,
        );
        ui.label(cx, ids!(atlas_upload_bench_status)).set_text(
            cx,
            if self.variant_b { "variant B" } else { "variant A" },
        );
        ui.redraw(cx);
    }

    fn handle_next_frame(&mut self, cx: &mut Cx, event: &NextFrameEvent, ui: &WidgetRef) {
        if !self.running || !event.set.contains(&self.next_frame) {
            return;
        }

        if let (Some(started), Some(kind)) =
            (self.pending_started.take(), self.pending_kind.take())
        {
            if self.pending_measured {
                let elapsed = started.elapsed().as_micros() as u64;
                match kind {
                    AtlasUploadKind::Full => self.full_us.push(elapsed),
                    AtlasUploadKind::Partial => self.partial_us.push(elapsed),
                }
            }
        }

        let total_pairs = ATLAS_BENCH_WARMUP_PAIRS + ATLAS_BENCH_MEASURED_PAIRS;
        if self.step == total_pairs * 2 {
            self.running = false;
            let full_p95 = Self::p95(&self.full_us);
            let partial_p95 = Self::p95(&self.partial_us);
            ui.label(cx, ids!(atlas_upload_bench_status))
                .set_text(cx, "scheduler proxy recorded");
            log!(
                "ATLAS_BENCH SCHEDULER_PROXY requested_partial_bytes={} requested_full_bytes={} partial_scheduler_p95_us={} full_scheduler_p95_us={}",
                ATLAS_BENCH_REQUESTED_PARTIAL_BYTES,
                ATLAS_BENCH_REQUESTED_FULL_BYTES,
                partial_p95,
                full_p95,
            );
            return;
        }

        let pair = self.step / 2;
        let first_in_pair = self.step & 1 == 0;
        let full_first = pair & 1 == 0;
        let kind = if first_in_pair == full_first {
            AtlasUploadKind::Full
        } else {
            AtlasUploadKind::Partial
        };
        let variant_b = pair & 1 == 0;
        match kind {
            AtlasUploadKind::Full => Self::write_tail(
                cx,
                self.full_texture.as_ref().unwrap(),
                variant_b,
                true,
            ),
            AtlasUploadKind::Partial => Self::write_tail(
                cx,
                self.partial_texture.as_ref().unwrap(),
                variant_b,
                false,
            ),
        }
        self.pending_started = Some(Instant::now());
        self.pending_kind = Some(kind);
        self.pending_measured = pair >= ATLAS_BENCH_WARMUP_PAIRS;
        self.step += 1;
        ui.redraw(cx);
        self.next_frame = cx.new_next_frame();
    }
}
```

- [ ] **Step 5: Wire the state into App**

Add this field to `App`:

```rust
#[rust]
atlas_upload_bench: AtlasUploadBench,
```

At the beginning of `handle_next_frame`, before the lens early return, add:

```rust
self.atlas_upload_bench
    .handle_next_frame(cx, e, &self.ui);
```

At the beginning of `handle_actions`, add:

```rust
if self
    .ui
    .button(cx, ids!(atlas_upload_bench_start))
    .clicked(actions)
{
    self.atlas_upload_bench.start(cx, &self.ui);
}
if self
    .ui
    .button(cx, ids!(atlas_upload_toggle))
    .clicked(actions)
{
    self.atlas_upload_bench.toggle(cx, &self.ui);
}
```

- [ ] **Step 6: Run non-UI compilation**

Run:

```bash
rtk cargo check --release -p makepad-example-splash
```

Expected: success.

- [ ] **Step 7: Run the visible UI check**

Run the UI test command from Step 2 again.

Expected: the toggle reaches `variant B` and the log contains `ATLAS_BENCH SCHEDULER_PROXY`. These next-frame intervals are observational only; backend timing acceptance comes from Studio profiler evidence.

- [ ] **Step 8: Verify A/B/A rendering through a fresh Studio run**

Start one persistent bridge:

```bash
target/release/cargo-makepad studio --studio=127.0.0.1:8001
```

Send `ListBuilds`, clear the prior Splash build with `ClearBuild`, then send:

```json
{"RunItem":{"mount":"makepad","name":"makepad-example-splash"}}
```

Capture the two 256 by 64 image regions at variant A, variant B, and variant A again. The acceptance condition is:

```text
full_A == partial_A
full_B == partial_B
full_A_after == partial_A_after
full_A == full_A_after
full_A != full_B
```

- [ ] **Step 9: Commit the benchmark probe**

```bash
rtk git add examples/splash/src/main.rs examples/splash/tests/ui.rs
rtk git commit -m "Add partial atlas upload benchmark"
```

---

### Task 3: Linux Desktop OpenGL Partial Upload

**Files:**
- Modify: `platform/src/os/linux/opengl.rs:2315-2635`
- Test: `platform/src/os/linux/opengl.rs`

**Interfaces:**
- Consumes: `TextureUpdated::upload_rect` and `OsType`.
- Produces: RGBAf32 `glTexSubImage2D` only on `LinuxWindow` and `LinuxDirect`.

- [ ] **Step 1: Extract the current policy and add a red policy test**

Add:

```rust
fn partial_texture_updates_supported(format: &TextureFormat, os_type: &OsType) -> bool {
    cfg!(not(ohos_sim)) && !matches!(format, TextureFormat::VecRGBAf32 { .. })
}

#[cfg(test)]
mod partial_texture_update_policy_tests {
    use super::*;

    fn rgba_f32() -> TextureFormat {
        TextureFormat::VecRGBAf32 {
            width: 8,
            height: 4,
            data: None,
            updated: TextureUpdated::Empty,
        }
    }

    #[test]
    fn rgba_f32_partial_updates_are_desktop_linux_only() {
        let format = rgba_f32();
        assert!(partial_texture_updates_supported(
            &format,
            &OsType::LinuxWindow(Default::default()),
        ));
        assert!(partial_texture_updates_supported(
            &format,
            &OsType::LinuxDirect,
        ));
        assert!(!partial_texture_updates_supported(
            &format,
            &OsType::Android(Default::default()),
        ));
        assert!(!partial_texture_updates_supported(
            &format,
            &OsType::OpenHarmony(Default::default()),
        ));
    }
}
```

Run on Linux:

```bash
rtk cargo test --release -p makepad-platform rgba_f32_partial_updates_are_desktop_linux_only
```

Expected: the LinuxWindow assertion fails.

- [ ] **Step 2: Implement the approved policy**

Replace the helper body with:

```rust
if cfg!(ohos_sim) {
    return false;
}
!matches!(format, TextureFormat::VecRGBAf32 { .. })
    || matches!(os_type, OsType::LinuxWindow(_) | OsType::LinuxDirect)
```

Rename `_os_type` to `os_type` in `update_vec_texture`.

- [ ] **Step 3: Resolve and clamp the native upload rectangle**

After the format tuple is built, replace the current constants and `match updated` with:

```rust
let partial_allowed = partial_texture_updates_supported(&self.format, os_type);
let force_full = needs_realloc || !partial_allowed;
let Some(rect) = updated.upload_rect(width, height, force_full) else {
    (gl.glBindTexture)(gl_sys::TEXTURE_2D, 0);
    return;
};
let use_partial =
    partial_allowed && !needs_realloc && matches!(updated, TextureUpdated::Partial(_));
let unpack_alignment = gl_unpack_alignment(bytes_per_pixel);

if use_partial {
    (gl.glPixelStorei)(gl_sys::UNPACK_ALIGNMENT, unpack_alignment);
    (gl.glPixelStorei)(gl_sys::UNPACK_ROW_LENGTH, width as _);
    (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_PIXELS, rect.origin.x as i32);
    (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_ROWS, rect.origin.y as i32);
    (gl.glTexSubImage2D)(
        gl_sys::TEXTURE_2D,
        0,
        rect.origin.x as i32,
        rect.origin.y as i32,
        rect.size.width as i32,
        rect.size.height as i32,
        format,
        data_type,
        data,
    );
} else {
    (gl.glPixelStorei)(gl_sys::UNPACK_ALIGNMENT, unpack_alignment);
    (gl.glPixelStorei)(gl_sys::UNPACK_ROW_LENGTH, width as _);
    (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_PIXELS, 0);
    (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_ROWS, 0);
    (gl.glTexImage2D)(
        gl_sys::TEXTURE_2D,
        0,
        internal_format as i32,
        width as i32,
        height as i32,
        0,
        format,
        data_type,
        data,
    );
}
```

Keep the existing reset of alignment, row length, and skip state after this block.

- [ ] **Step 4: Run Linux tests and release check**

```bash
rtk cargo test --release -p makepad-platform rgba_f32_partial_updates_are_desktop_linux_only
rtk cargo check --release -p makepad-platform
```

Expected: success on Linux.

- [ ] **Step 5: Run five fresh release Studio benchmarks**

For each run, clear the previous Splash build and start a new `makepad-example-splash` `RunItem` with `MAKEPAD_NO_VSYNC=1` inherited by Studio. Verify:

- the native `glTexSubImage2D` arguments resolve to exactly 1,008 bytes and
  the full control resolves to 2,097,152 bytes;
- A/B/A probe crops satisfy Task 2.
- Every run reports partial p95 no greater than 1.02 times full p95.
- No GL error or black probe.

If native Linux GL is unavailable, leave this task marked pending.

- [ ] **Step 6: Commit only after the native gate passes**

```bash
rtk git add platform/src/os/linux/opengl.rs
rtk git commit -m "Use partial float atlas uploads on desktop GL"
```

---

### Task 4: Metal Partial Upload

**Files:**
- Modify: `platform/src/os/apple/metal.rs:1886-2080`

**Interfaces:**
- Consumes: `TextureUpdated::upload_rect`.
- Produces: partial `replaceRegion` calls and the actual uploaded-byte count already consumed by Metal profiling.

- [ ] **Step 1: Preserve the allocation result**

Change the start of `update_vec_texture` to:

```rust
let needs_realloc = self.alloc_vec();
if needs_realloc {
    let alloc = self.alloc.as_ref().unwrap();
```

Keep the existing descriptor and texture creation body inside that condition.

- [ ] **Step 2: Replace only the VecRGBAf32 arm**

Use:

```rust
TextureFormat::VecRGBAf32 {
    width,
    height,
    data,
    ..
} => {
    let Some(rect) = update.upload_rect(*width, *height, needs_realloc) else {
        return 0;
    };
    let data = data.as_ref().unwrap();
    let float_offset = (rect.origin.y * *width + rect.origin.x) * 4;
    let region = MTLRegion {
        origin: MTLOrigin {
            x: rect.origin.x as u64,
            y: rect.origin.y as u64,
            z: 0,
        },
        size: MTLSize {
            width: rect.size.width as u64,
            height: rect.size.height as u64,
            depth: 1,
        },
    };
    let _: () = unsafe {
        msg_send![
            self.os.texture.as_ref().unwrap().as_id(),
            replaceRegion: region
            mipmapLevel: 0u64
            withBytes: data.as_ptr().add(float_offset) as *const std::ffi::c_void
            bytesPerRow: (*width as u64) * 16u64
        ]
    };
    (rect.size.width as u64)
        .saturating_mul(rect.size.height as u64)
        .saturating_mul(16)
}
```

Leave every other texture format unchanged.

- [ ] **Step 3: Run release checks**

```bash
rtk cargo test --release -p makepad-platform --lib texture_updated_upload_rect
rtk cargo check --release -p makepad-platform --target aarch64-apple-darwin
```

Expected: success.

- [ ] **Step 4: Run five fresh Metal Studio benchmarks**

Clear and rerun `makepad-example-splash` for each measurement. Confirm the existing Metal profiler records exactly 1,008 bytes for partial frames and 2,097,152 bytes for full frames, the A/B/A crops match, and every p95 ratio is at most 1.02.

- [ ] **Step 5: Commit after the Metal gate passes**

```bash
rtk git add platform/src/os/apple/metal.rs
rtk git commit -m "Upload partial float atlas regions on Metal"
```

---

### Task 5: D3D11 Partial Upload

**Files:**
- Modify: `platform/src/os/windows/d3d11.rs:20-65, 1251-1431`
- Regenerate: `libs/windows/windows-rs/src/Windows/mod.rs`

**Interfaces:**
- Consumes: `TextureUpdated::upload_rect` and stable `ID3D11Texture2D` storage.
- Produces: `ID3D11DeviceContext::UpdateSubresource` with a bounded `D3D11_BOX`.

- [ ] **Step 1: Add imports and preserve update state**

Add `TextureUpdated` to the texture import and `D3D11_BOX` to the Direct3D11 import. Replace the beginning of `update_vec_texture` with:

```rust
let needs_realloc = self.alloc_vec();
let updated = self.take_updated();
if updated.is_empty() {
    return;
}
```

- [ ] **Step 2: Add the stable partial path before cube/full recreation**

Insert:

```rust
if !needs_realloc && matches!(updated, TextureUpdated::Partial(_)) {
    if let (
        TextureFormat::VecRGBAf32 {
            width,
            height,
            data,
            ..
        },
        Some(texture),
    ) = (&self.format, &self.os.texture)
    {
        let Some(rect) = updated.upload_rect(*width, *height, false) else {
            return;
        };
        let dst_box = D3D11_BOX {
            left: rect.origin.x as u32,
            top: rect.origin.y as u32,
            front: 0,
            right: (rect.origin.x + rect.size.width) as u32,
            bottom: (rect.origin.y + rect.size.height) as u32,
            back: 1,
        };
        let data = data.as_ref().unwrap();
        let float_offset = (rect.origin.y * *width + rect.origin.x) * 4;
        let resource: ID3D11Resource = texture.cast().unwrap();
        unsafe {
            d3d11_cx.context.UpdateSubresource(
                &resource,
                0,
                Some(&dst_box as *const D3D11_BOX),
                data.as_ptr().add(float_offset) as *const std::ffi::c_void,
                (*width * 16) as u32,
                0,
            );
        }
        return;
    }
}
```

Keep the existing `CreateTexture2D` and shader-resource-view code as the fallback for Full, resize, reallocation, or missing native storage.

- [ ] **Step 3: Confirm the stripped-wrapper red state on Windows**

Run:

```bash
rtk cargo check --release -p makepad-platform --target x86_64-pc-windows-msvc
```

Expected before regeneration: `ID3D11DeviceContext` has no method named `UpdateSubresource`.

- [ ] **Step 4: Regenerate the repository Windows bindings**

Run:

```bash
rtk cargo run --release --manifest-path tools/windows_strip/Cargo.toml --features fetch-windows-upstream
```

Expected: the generated `ID3D11DeviceContext` inherent implementation retains:

```rust
pub unsafe fn UpdateSubresource<P0>(
    &self,
    pdstresource: P0,
    dstsubresource: u32,
    pdstbox: Option<*const D3D11_BOX>,
    psrcdata: *const core::ffi::c_void,
    srcrowpitch: u32,
    srcdepthpitch: u32,
)
where
    P0: windows_core::Param<ID3D11Resource>,
```

- [ ] **Step 5: Re-run the Windows release check**

Run the Step 3 command again.

Expected: success. If the Windows target is not installed, request approval before downloading it and keep validation pending.

- [ ] **Step 6: Run five native Windows Studio benchmarks**

On Windows, clear and rerun `makepad-example-splash` for every measurement. Confirm exact box dimensions, A/B/A output equality, intact sentinel pixels, and every p95 ratio at most 1.02.

- [ ] **Step 7: Commit only after the native gate passes**

```bash
rtk git add platform/src/os/windows/d3d11.rs libs/windows/windows-rs/src/Windows/mod.rs
rtk git commit -m "Update partial float atlas regions on D3D11"
```

---

### Task 6: WebGL2 Partial Upload

**Files:**
- Modify: `platform/src/os/web/from_wasm.rs:258-264`
- Modify: `platform/src/os/web/web_gl.rs:90-166`
- Modify: `platform/src/os/web/web_gl.js:776-803`

**Interfaces:**
- Consumes: `TextureUpdated::upload_rect` and the existing RGBAf32 WASM pointer.
- Produces: region-aware `FromWasmAllocTextureImage2D_RGBAf32` and `texSubImage2D` calls that restore WebGL unpack state.

- [ ] **Step 1: Extend the existing message**

Replace the RGBAf32 message with:

```rust
#[allow(non_camel_case_types)]
#[derive(FromWasm)]
pub struct FromWasmAllocTextureImage2D_RGBAf32 {
    pub texture_id: usize,
    pub width: usize,
    pub height: usize,
    pub data: WasmPtrF32,
    pub is_partial: bool,
    pub x: usize,
    pub y: usize,
    pub update_width: usize,
    pub update_height: usize,
}
```

- [ ] **Step 2: Preserve allocation and update state in Rust**

Import `TextureUpdated` in `web_gl.rs`. Replace the two discarded values with:

```rust
let needs_realloc = cxtexture.alloc_vec();
let updated = cxtexture.take_updated();
if !updated.is_empty() {
```

In the `VecRGBAf32` arm, send:

```rust
let Some(rect) = updated.upload_rect(*width, *height, needs_realloc) else {
    continue;
};
self.os.from_wasm(FromWasmAllocTextureImage2D_RGBAf32 {
    texture_id: texture_id.0,
    width: *width,
    height: *height,
    data: WasmPtrF32::new(data.as_ref().unwrap()),
    is_partial: !needs_realloc
        && matches!(updated, TextureUpdated::Partial(_)),
    x: rect.origin.x,
    y: rect.origin.y,
    update_width: rect.size.width,
    update_height: rect.size.height,
});
```

Leave the existing `web.rs` `to_js_code()` registration unchanged.

- [ ] **Step 3: Implement the WebGL2 branch**

Replace the body of `FromWasmAllocTextureImage2D_RGBAf32` with:

```javascript
let gl = this.gl;
let old_tex = this.textures[args.texture_id];
let gl_tex = old_tex || gl.createTexture();

gl.bindTexture(gl.TEXTURE_2D, gl_tex);
gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
let data_array = new Float32Array(
  this.memory.buffer,
  args.data.ptr,
  args.width * args.height * 4,
);

if (args.is_partial && old_tex) {
  gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
  gl.pixelStorei(gl.UNPACK_ROW_LENGTH, args.width);
  gl.pixelStorei(gl.UNPACK_SKIP_PIXELS, args.x);
  gl.pixelStorei(gl.UNPACK_SKIP_ROWS, args.y);
  try {
    gl.texSubImage2D(
      gl.TEXTURE_2D,
      0,
      args.x,
      args.y,
      args.update_width,
      args.update_height,
      gl.RGBA,
      gl.FLOAT,
      data_array,
    );
  } finally {
    gl.pixelStorei(gl.UNPACK_ROW_LENGTH, 0);
    gl.pixelStorei(gl.UNPACK_SKIP_PIXELS, 0);
    gl.pixelStorei(gl.UNPACK_SKIP_ROWS, 0);
  }
} else {
  gl.texImage2D(
    gl.TEXTURE_2D,
    0,
    gl.RGBA32F,
    args.width,
    args.height,
    0,
    gl.RGBA,
    gl.FLOAT,
    data_array,
  );
}
this.textures[args.texture_id] = gl_tex;
```

- [ ] **Step 4: Run syntax, shared, and WASM checks**

```bash
rtk proxy node --check platform/src/os/web/web_gl.js
rtk cargo test --release -p makepad-platform --lib texture_updated_upload_rect
rtk cargo check --release -p makepad-platform --target wasm32-unknown-unknown
```

Expected: success. If the WASM target is absent, request approval before installing it and keep target validation pending.

- [ ] **Step 5: Run five fresh browser Studio benchmarks**

Use the Studio runnable workflow for Splash's web target. Confirm:

- the first allocation uses `texImage2D`;
- stable partial updates use `texSubImage2D` with x=1, y=63, width=63, height=1;
- unpack row length and skip values are zero after the call;
- the A/B/A crops match;
- every p95 ratio is at most 1.02.

- [ ] **Step 6: Commit only after the browser gate passes**

```bash
rtk git add platform/src/os/web/from_wasm.rs platform/src/os/web/web_gl.rs platform/src/os/web/web_gl.js
rtk git commit -m "Upload partial float atlas regions on WebGL2"
```

---

### Task 7: Final Cross-Backend Verification

**Files:**
- Verify only; no source file is added.

**Interfaces:**
- Consumes: all backend checkpoints and Splash benchmark logs/screenshots.
- Produces: a pass/pending matrix for the handoff.

- [ ] **Step 1: Run all portable release checks**

```bash
rtk cargo test --release -p makepad-platform --lib texture_updated_upload_rect
rtk cargo test --release -p makepad-draw appended_dirty_rect
rtk cargo check --release -p makepad-example-splash
rtk proxy node --check platform/src/os/web/web_gl.js
```

Expected: success.

- [ ] **Step 2: Inspect the final diff**

```bash
rtk git diff --check
rtk git status --short
```

Expected: no whitespace errors, no accidental generated files, and `LearningMakepad.md` remains untracked and unstaged.

- [ ] **Step 3: Record the backend matrix**

For Metal, Linux desktop OpenGL, D3D11, WebGL2, and Vulkan, record:

```text
compile: pass | pending
native runtime: pass | pending
partial bytes: measured value
full bytes: measured value
five p95 ratios: values
pixel digest: pass | pending
fallback/reallocation: pass | pending
```

A backend is complete only when all native entries pass and all five p95 ratios are at most 1.02. Unsupported GLES/OpenHarmony and headless are expected full/no-upload controls, not failures.

- [ ] **Step 4: Stop instead of widening scope**

If a backend misses the byte, pixel, or timing gate, retain its full-upload path and report the measurement. Do not add a feature flag, staging pool, atlas growth policy, or compact payload in this plan.
