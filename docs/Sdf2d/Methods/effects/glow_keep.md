# glow_keep

```glsl
fn glow_keep(inout self, color: vec4, width: float) -> vec4
```

The `glow_keep` function applies a glowing effect to the current shape in the `Sdf2d` drawing context by blending a specified color with the existing result based on the glow width. This function preserves the current shape and clipping settings while applying the glow effect, allowing you to layer effects without resetting the drawing state.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance where the glow effect is applied. The function modifies the `result` field of `self` in place.
- **color** (`vec4`): An RGBA color of the glow.
- **width** (`float`): The width of the glow effect. The glow width is scaled according to the current `scale_factor` of the drawing context. Larger values produce a more pronounced glow.

## Returns

- **vec4**: The updated result color after the glow effect is applied. This color reflects the addition of the glow effect to the existing content.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
    
    // Draw a circle centered at (50.0, 50.0) with a radius of 30.0.
    sdf.circle(50.0, 50.0, 30.0);
    
    // Apply a red glow effect with a width of 10.0 units, preserving the shape state.
    sdf.glow_keep(#f00, 10.0);
    
    // Optionally, fill the shape with another color without resetting the state.
    sdf.fill_keep(#00f);
    
    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: Initialize the `Sdf2d` context using the current position and size of the viewport.
- **Draw Shape**: Define a circle centered at `(50.0, 50.0)` with a radius of `30.0`.
- **Apply Glow Effect**: Use `glow_keep` to apply a red glow effect (`#f00`) with a width of `10.0` units to the current shape. The `glow_keep` function adds a glow around the shape without altering the existing drawing state.
- **Optional Fill**: Use `fill_keep` to fill the shape with blue color (`#00f`) while preserving the shape for further operations if needed.
- **Return Result**: Return `sdf.result`, which contains the final rendered color after all drawing operations.

### Notes

- **Preserving State**: The `glow_keep` function maintains the current shape and clipping settings, allowing you to apply additional effects or fills afterward.
- **Order of Operations**: The order in which you apply effects and fills affects the final rendering. Define the shape first, then apply the glow and any fills as needed.
- **Glow Width Scaling**: The `width` parameter is adjusted based on the `scale_factor` of the `Sdf2d` context, ensuring consistent glow size regardless of transformations.
- **Combining Effects**: You can apply multiple effects sequentially by using the `_keep` variants of functions, preserving the drawing state between operations.