# Camera architecture (AV1-ready stage)

## Scope

This document describes the camera pipeline architecture implemented up to the AV1 encoder-ready stage (before SVT-AV1 vendoring).

Implemented goals:

1. Low-latency preview path remains first-class.
2. Encoder-ready frame path is explicit and platform-agnostic.
3. Android and Linux/V4L2 are wired to the shared transport.
4. Existing `Video` widget behavior remains functional.

## Dual-path model

The camera pipeline has two explicit paths:

- **Preview path (low latency)**
  - `Video` widget renders frames as soon as available.
  - For camera input, preview is driven through YUV plane textures.

- **Encoder-ready path (structured frame transport)**
  - Shared API in `makepad_platform::video`:
    - `CameraFrameRef`
    - `CameraFramePlaneRef`
    - `CameraFrameLayout`
    - `CameraColorMatrix`
    - `CameraFrameInputFn`
  - Frame metadata includes:
    - `timestamp_ns`
    - `width`, `height`
    - `layout` and `matrix`
    - per-plane bytes + `row_stride` + `pixel_stride`

The transport is platform-agnostic and can be used by future encoder integration without changing public widget APIs.

## Ownership and buffering

`CameraFramePool` provides reusable frame ownership:

- checkout reusable owned frame (`CameraFrameOwned`)
- publish latest frame (drops stale frame for latency)
- take latest frame in consumer
- recycle frame back to free pool

This keeps queue depth short and reduces allocation churn.

Current strategy is single-latest semantics (latency-oriented), not deep buffering.

## Source-mode architecture in `Video` widget

`Video` now uses explicit source modes:

- `ExternalTexture`
- `YuvPlanes`

Rules:

- Camera source selects `YuvPlanes` mode.
- Non-camera source selects `ExternalTexture` mode.
- Primary texture setup is mode-aware (`ensure_primary_texture`) so camera mode does not require external texture setup.
- Android `TextureHandleReady` gating only applies in external texture mode.

This removes implicit coupling between widget defaults and backend assumptions.

## Android backend notes

- `AImageReader_acquireLatestImage` is used.
- Reader queue is kept small (`maxImages = 3`).
- Camera callbacks now publish `CameraFrameRef` with plane metadata.
- `AImage_getTimestamp` is captured into `timestamp_ns`.
- Legacy `VideoInputFn` compatibility remains available, but the camera player path uses structured frame transport.

## Linux / V4L2 backend notes

- Capture loop publishes `CameraFrameRef` for:
  - I420
  - NV12
  - YUY2
  - MJPEG
- Timestamp is derived from V4L2 buffer timestamp.
- Camera player consumes transport and normalizes to I420 for GL upload.

## Compatibility and non-breaking extension hooks

`CxMediaApi` now has explicit adapter hooks:

- `camera_frame_input(...)`
- `camera_frame_input_box(...)`

Default implementation is no-op, so unsupported platforms remain non-breaking.

Android and Linux override these hooks.

## Follow-up for Apple/Windows

Apple and Windows implementations can be added without API break by wiring their native camera frames into `CameraFrameRef`:

1. Add platform camera capture adapters that emit `CameraFrameRef` through `camera_frame_input_box`.
2. Map platform-native pixel formats to `CameraFrameLayout` + per-plane strides.
3. Populate `timestamp_ns` from native monotonic capture timestamps.
4. Reuse `CameraFramePool` strategy for latency-first latest-frame handoff.
5. Keep legacy `video_input_box` compatibility until all internal camera users are migrated.

No API signature changes are required for this extension.

## Camera -> SVT-AV1 integration (current)

The platform now includes a shared camera AV1 encoder worker:

- module: `platform/src/video_encode/camera_av1_encoder.rs`
- SVT wrapper: `platform/src/video_encode/svt_av1_wrapper.c`
- FFI: `platform/src/video_encode/svt_av1_ffi.rs`

### API surface

`makepad_platform::video` now exposes:

- `CameraAv1EncoderConfig`
- `EncodedAv1PacketRef`
- `EncodedAv1PacketOwned`
- `CameraAv1OutputFn`

`CxMediaApi` now exposes:

- `camera_av1_output(index, config, callback)`
- `camera_av1_output_box(index, config, callback)`

### Backpressure / latency policy

The encoder worker uses one bounded frame queue per stream.

- queue capacity: `config.queue_capacity` (default `2`)
- policy on overflow: **drop oldest** frame, keep newest
- capture callback never blocks on encoder

This keeps capture latency bounded under encoder overload.

### Platform wiring

- Linux (V4L2): native `CameraFrameRef` -> shared encoder worker -> AV1 packets
- Android (NDK camera): native `CameraFrameRef` -> shared encoder worker -> AV1 packets
- Apple (AVFoundation): now emits `CameraFrameRef` with timestamp + layout + stride (NV12/YUY2) and can feed shared encoder worker
- Windows (Media Foundation): now emits `CameraFrameRef` with timestamp + layout + stride (NV12/YUY2/MJPEG) and can feed shared encoder worker

All platforms use the same I420 normalization + queue/backpressure + SVT packet output path.

### Build/link behavior

- Linux: links vendored `libs/svt-av1/Bin/Release/libSvtAv1Enc.a` and builds a thin wrapper C object.
- Android: uses prebuilt library when available; otherwise build script attempts CMake Android build of vendored SVT and links resulting static archive.
- Other platforms: camera frame transport and API are available; AV1 encoder registration falls back cleanly when `has_svt_av1` is not enabled.
