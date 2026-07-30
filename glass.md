# Apple Glass UI Shader Plan

## Goal

Add first-class glass UI surfaces that can tint, refract, highlight, and blur the scene behind them without each widget inventing its own offscreen path. The first implementation lives in `Window`: each window owns a `GaussStack`, and overlay widgets request access to that stack through a generic API. Longer term, the same request/capture model can become a backend-level hookable renderflow.

## Current State

- `widgets/src/glass_panel.rs` already defines a `GlassPanel` style with tint, border, specular, noise, and a `use_scene_blur` knob, but it does not sample a real blurred scene texture yet.
- UI rendering is pass-based. `platform/src/os/cx_shared.rs::compute_pass_repaint_order` builds `passes_todo`, and each backend renders that ordered list to a window or texture. It now logs the pass count when the count changes.
- Views can create child texture passes through `ViewOptimize::Texture`, but that is a widget-local cache, not a frame-level background blur stack.
- `Window` can split a frame into an offscreen scene capture plus root-pass overlays when an overlay widget requests glass.
- The immediate problem for real glass is that a shader cannot reliably sample the current framebuffer underneath itself while drawing in the same render pass. We need renderflow-owned intermediate textures.

## Window-Owned Architecture

The first slice keeps the renderflow local to `Window` so we can validate behavior before changing every backend:

1. `Window` owns a `GaussStack`.
2. A generic `request_window_gauss(cx)` API records that the current window has an overlay needing the gauss mipchain.
3. The next redraw captures normal window contents into the stack's high-resolution scene pass.
4. Overlay drawlists are still collected by the window overlay, but are assigned to the root window pass while the scene is captured.
5. `Window::end()` builds the mip chain, draws the sharp scene back into the root pass, then appends overlays.
6. Glass widgets bind the scene texture plus mip textures if the stack is active; otherwise they draw a fallback material.

This keeps `Window` free of specific widget types. It only asks whether the window has a gauss request, and it exposes textures through a generic snapshot.

## Future Renderflow Hook

Initial hook points:

- `before_pass(draw_pass_id)`: allocate or update resources before a UI pass is rendered.
- `after_pass(draw_pass_id)`: capture pass output or schedule dependent passes.
- `before_present(window_pass_id)`: composite final effects before presenting the window.
- `resize_or_dpi_changed(window_pass_id, size, dpi)`: invalidate per-window blur resources.

The default renderflow hook would do nothing, so existing apps keep the same behavior. The current `Window` implementation is the prototype for the hook contract.

## Glass Blur Stack

The `GaussStack` owns per-window blur resources:

- `scene_texture`: the resolved color output before glass overlays.
- `mip_textures`: a chain of downsampled textures for larger blur radii.
- `scene_pass`: the high-resolution drawpass that captures normal content.
- `mip_passes`: drawpasses that build the pyramid.
- `uniforms`: blur radius, saturation, tint, refraction, noise seed, and scale.

The first implementation uses a conservative full-window mip stack:

1. Render normal content into an offscreen scene texture.
2. Downsample scene texture into a mip pyramid.
3. Draw the unblurred scene texture back into the root window pass.
4. Draw overlay widgets that sample the scene texture and mip textures in screen coordinates.
5. Present the composed final pass.

Once stable, optimize by scissoring or tiling around glass rectangles.

## Widget Integration

Glass widgets should use the request/snapshot API:

- Register demand by calling `request_window_gauss(cx)` while drawing inside an overlay drawlist.
- Expose script properties for `blur_radius`, `saturation`, `tint_color`, `tint_alpha`, `border_alpha`, `specular_strength`, `noise_strength`, and `refraction_strength`.
- Bind window-provided scene/mip textures into the glass draw shader.
- Keep a fallback path when no blur stack exists: current tint/noise/highlight shader behavior.

The widget should not allocate render targets directly. It only declares intent: "this overlay draw needs backdrop glass with these parameters."

## Shader Work

Implement shaders in layers:

1. `DrawGlassPanel` / `GaussRoundedView`: final panel shader that samples blurred backdrop texture, optional sharp scene texture, and procedural noise.
2. `DrawBlurDownsample`: box or tent downsample.
3. Mip-level interpolation and a small multi-tap gather for smoother large kernels.
4. Optional `DrawGlassMask`: writes rounded-rect mask for region-limited blur.

The first pass should favor stable visuals over perfect physical accuracy:

- Background blur sampled in screen/window coordinates.
- Tint and saturation correction after blur.
- Rounded SDF clipping in the glass panel shader.
- Border/highlight generated in the panel shader.
- Noise dither kept subtle and time-stable unless animation is explicitly requested.

## Backend Path

Implementation order:

1. Validate the window-owned `GaussStack` on the existing backend paths.
2. Clean up the request/snapshot API so multiple glass widgets can share one stack.
3. Add backend-neutral renderflow structs and hook APIs in `platform/src` if the window-local model proves too limiting.
4. Convert the Metal repaint loop to execute renderflow ops.
5. Keep other backends on the default no-hook flow until the API is stable.

## Invalidation And Performance

Blur resources must be rebuilt when:

- Window size changes.
- DPI changes.
- Glass parameters require a different stack resolution or radius class.
- The source pass repaints.
- The overlay request state changes from no glass to glass, or glass to no glass.

Performance controls:

- Downsample before blur.
- Bucket blur radii into a small number of stack variants.
- Skip blur work when no visible glass regions exist.
- Reuse textures across frames.
- Add counters for renderflow op count, blur texture size, and pass count.

## Milestones

1. Log current pass count from the shared pass planner.
2. Add `Window`-owned `GaussStack` with scene and mip drawpasses.
3. Add generic overlay request/snapshot API with no widget-type leakage into `Window`.
4. Add a `GaussRoundedView` proof widget and splash popup demo.
5. Add screenshots and pass-count checks through Studio remote runs.
6. Add smoother multi-tap upsample/selection for large blur kernels.
7. Optimize region-limited blur after the full-window path is correct.

## Open Questions

- Should glass split a window into "content before glass" and "glass overlay" draw lists automatically, or should glass widgets force a renderflow capture at their depth?
- Do we want one blur stack per window or one per source pass?
- How much refraction should be supported in the first shader without requiring normal maps or distance fields from surrounding UI?
- Should renderflow hooks be internal-only at first, or exposed to app/widget authors once stabilized?
