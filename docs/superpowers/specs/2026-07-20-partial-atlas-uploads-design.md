# Backend-Specific Partial Atlas Uploads

Date: 2026-07-20

Status: Approved design

## Goal

Reduce the cost of append-only RGBAf32 atlas updates by sending only the dirty
rectangle to each capable graphics backend. A backend change ships only when
it cuts submitted texture bytes by at least 75%, produces identical output,
and keeps p95 frame or submission time within 2% of the full-upload control.

## Scope

Keep the existing atlas producers, retained CPU texture data, and
TextureUpdated::Partial(RectUsize) contract. Reuse the rectangle resolution
already implemented by Vulkan. Implement native partial uploads independently
for Linux desktop OpenGL, Metal, D3D11, and WebGL2.

Vulkan remains the reference implementation. Headless rendering remains
unchanged because it reads the retained CPU texture directly.

## Chosen Approach

Use a phased backend rollout. The shared rectangle logic and benchmark land
first, followed by small backend-specific changes. Each backend is measured
and accepted independently.

This avoids a new atlas payload format, a second telemetry system, or a
cross-platform upload abstraction. Those additions are unnecessary because
the existing update state and CPU buffer already contain everything the native
APIs need.

## Upload Data Flow

1. An atlas producer appends data to its retained VecRGBAf32 texture and marks
   TextureUpdated::Partial with a bounding dirty rectangle.
2. The backend allocates or reuses native texture storage.
3. The existing Vulkan texture_upload_rect logic, moved to the shared texture
   module, resolves the update:
   - Zero logical width or height produces no upload rectangle.
   - Otherwise, reallocated storage forces the complete texture rectangle,
     even when the update state is Empty, so recreated native storage is
     initialized from the retained CPU texture.
   - Without a forced full upload, Empty produces no upload and Full produces
     the complete texture rectangle.
   - Without a forced full upload, Partial is clipped to the texture bounds;
     a fully clipped or zero-area rectangle produces no upload.
4. The backend converts the resolved rectangle directly into its native upload
   call. The source pointer is offset to the rectangle origin while the source
   row pitch remains the full CPU texture width.
5. The update state is consumed with the existing take_updated behavior.

No retry mechanism is added. Metal, D3D11, and the existing GL paths do not
provide a useful recoverable result at this call site, so current failure
handling remains unchanged.

## Backend Behavior

### Vulkan

Keep the normal upload mechanics. Vulkan clips dirty rectangles, packs partial
rows, preserves stable images, and forces a full upload after image recreation.
For zero logical dimensions, the shared resolver returns no logical upload
rectangle, but Vulkan still creates physical width.max(1) by height.max(1) by
layers storage, copies zero-filled data across those physical extents, and
performs the normal shader-read layout transition before descriptor binding.
Its local rectangle helper becomes the shared implementation.

Staging-buffer reuse is explicitly separate work and requires its own
benchmark.

### Linux Desktop OpenGL

Allow VecRGBAf32 to use the existing glTexSubImage2D branch on X11, Wayland,
and linux-direct. Reuse the existing UNPACK_ROW_LENGTH,
UNPACK_SKIP_PIXELS, and UNPACK_SKIP_ROWS handling.

Retain full uploads on conservative GLES and OHOS paths. Restore all modified
pixel-store state after every partial upload.

### Metal

When storage is stable and the update is partial, call replaceRegion with the
clipped MTLRegion. Offset the VecRGBAf32 source pointer by
(y * full_width + x) * 4 floats and keep bytesPerRow at full_width * 16.

Reallocation, resize, or full update uses the existing full-region path. The
existing Metal upload-byte accounting reports rectangle width * height * 16.

### D3D11

When storage is stable and the update is partial, preserve the existing
texture and shader-resource view and call UpdateSubresource with a D3D11_BOX.
Use the offset source pointer and full_width * 16 row pitch.

Missing storage, resize, format change, or full update uses the existing
CreateTexture2D and shader-resource-view path. Regenerate the repository's
stripped Windows bindings to expose UpdateSubresource.

### WebGL2

Extend the existing RGBAf32 WASM texture message with the resolved update
rectangle and whether storage must be allocated. Existing or stable textures
use texSubImage2D.

Set UNPACK_ROW_LENGTH to the full texture width and set the skip values to the
rectangle origin. Reset all three values after the call. Missing storage,
resize, or full update retains texImage2D.

WebGL2 is already required, so no new capability probe or extension is added.

### Headless

Do nothing. The headless raster path reads the current CPU VecRGBAf32 data and
does not upload textures.

## Benchmark

Reuse makepad-example-splash; do not add a crate or benchmark framework. Add a
Start Atlas Upload Benchmark button beside the existing DrawGlyph probe. It
remains idle unless activated and then runs an atlas probe that owns two
identical RGBAf32 textures:

- Full control: marks the update as Full.
- Partial candidate: marks the same changed texels as Partial.

Both paths render the same deterministic probe image. The benchmark alternates
their execution order to reduce drift.

The primary workload uses a fixed 2048 by 64 atlas. It appends 63 texels within
one existing row without resizing:

- Partial submission: 63 * 1 * 16 = 1,008 bytes.
- Full submission: 2048 * 64 * 16 = 2,097,152 bytes.
- Expected reduction: 99.9519%.

Run 30 warm-up iterations, then 500 paired iterations, repeated across five
release runs.

Submitted bytes are defined from the exact rectangle supplied to the native
upload call, not DrawList's current full-texture estimate. The same resolved
rectangle drives the API parameters and the byte calculation, so no permanent
parallel telemetry subsystem is needed.

Use existing profiler timing when it represents GPU completion. Otherwise the
probe measures CPU submission duration, labels it as such, and applies the same
p95 no-regression gate. CPU timing must not be reported as a GPU-completion
improvement.

## Correctness and Safety Checks

One focused shared test covers:

- Empty and Full updates.
- In-bounds partial updates.
- Clipped and overflow-adjacent rectangles.
- Zero-area and fully outside rectangles.
- Reallocation forcing a full upload, including from Empty update state.

Backend validation covers:

- Same-row partial update.
- Row-crossing partial update.
- Resize and first allocation falling back to full upload.
- Untouched sentinel texels remaining unchanged.
- Full and partial rendered-output digests matching.
- GL and WebGL pixel-store state being restored.
- No backend API error or black output in the probe.

UI and rendering validation uses fresh release Studio runs. Clear the previous
build before every rerun and validate only the new build id. A backend without
available native hardware or browser coverage remains pending rather than
being treated as passing.

## Acceptance Gate

A backend implementation can land only when all three conditions pass:

1. Submitted bytes are no more than 25% of the full-upload control.
2. The rendered output is identical and sentinel texels are intact.
3. p95 frame or clearly labelled submission time is no more than 1.02 times
   the full-upload control.

If a backend misses a gate, retain its current full-upload behavior and report
the measurement. Do not add a runtime feature flag for a failed path.

## Rollout

1. Shared rectangle helper, focused tests, and the Splash benchmark probe.
2. Linux desktop OpenGL and Metal, measured independently.
3. D3D11 plus the minimal stripped-binding update.
4. WebGL2 WASM message and JavaScript upload path.

Each backend is a separate review and validation checkpoint. Vulkan preserves
its normal upload mechanics and explicitly initializes zero-size recreations
with the deterministic zero-fill copy and layout transition described above;
headless receives no behavior change.

## Non-Goals

- Changing atlas growth or allocation policy.
- Packing dirty rows into a new producer-owned payload.
- Vulkan staging-buffer pooling.
- A general texture-upload abstraction.
- A new public telemetry or benchmark framework.
- Enabling partial RGBAf32 uploads on unverified GLES or OHOS paths.
