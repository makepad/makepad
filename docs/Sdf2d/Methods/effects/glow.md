# glow

```glsl
fn glow(inout self, color: vec4, width: float) -> vec4
```

The `glow` function applies a glowing effect to the current shape in the `Sdf2d` drawing context by blending a specified color around the edges of the shape based on the glow width. Unlike `glow_keep`, the `glow` function resets the internal shape and clipping state after applying the glow effect, preparing the context for new drawing operations.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance where the glow effect is applied. The function modifies the `result` and resets certain fields (`shape`, `old_shape`, `clip`, `has_clip`) of `self` in place.
- **color** (`vec4`): An RGBA color for the glow effect.
- **width** (`float`): The width of the glow effect. The glow width is scaled according to the current `scale_factor` of the drawing context.

## Returns

- **vec4**: The updated result color after the glow effect is applied, reflecting the addition of the glow effect to the existing content. The internal shape and clipping state are reset after this operation.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Draw a rectangle with rounded corners.
    sdf.box(10.0, 10.0, 80.0, 80.0, 5.0);

    // Apply a blue glow effect with a width of 10.0 units.
    sdf.glow(#88f, 10.0);

    // Since 'glow' resets the shape state, we can define a new shape.
    // Draw a circle inside the rectangle.
    sdf.circle(50.0, 50.0, 30.0);

    // Apply an orange glow effect to the new shape.
    sdf.glow(#f80, 5.0);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: Initialize the `Sdf2d` context using the current position (`self.pos`) multiplied by `self.rect_size`, which represents the size of the viewport.

- **Draw First Shape**: Use `sdf.box` to draw a rectangle starting at position `(10.0, 10.0)` with a width and height of `80.0` units and a corner radius of `5.0`.

- **Apply Glow to First Shape**: Use `sdf.glow` to apply a blue glow (`#88f`) with a width of `10.0` units to the current shape (the rectangle). After this, the internal shape and clipping state are reset.

- **Draw Second Shape**: Define a new shape—a circle centered at `(50.0, 50.0)` with a radius of `30.0`.

- **Apply Glow to Second Shape**: Apply an orange glow (`#f80`) with a width of `5.0` units to the new shape.

- **Return Result**: Return `sdf.result`, which contains the final rendered color after all drawing operations.

### Notes

- **State Reset**: Unlike `glow_keep`, the `glow` function resets the internal shape (`shape`, `old_shape`) and clipping (`clip`, `has_clip`) state after execution. This means you can start defining a new shape immediately after without manual resets.

- **Order of Operations**: Ensure that you apply `glow` after defining the shape you wish to apply the effect to. Since `glow` resets the shape state, any shape you wish to affect must be defined before calling `glow`.

- **Glow Width Scaling**: The `width` parameter is adjusted based on the `scale_factor` of the `Sdf2d` context, ensuring consistent glow size regardless of transformations.

- **Color Specification**: Colors can be specified using hexadecimal notation (e.g., `#88f` for blue, `#f80` for orange) or as `vec4` values.

- **Multiple Glows**: By using `glow` and defining new shapes between calls, you can apply multiple glow effects to different shapes sequentially.

- **Combining Effects**: Since `glow` resets the shape state, you can combine it with other drawing operations to create complex visuals. Remember to define each shape before applying the glow to it.

### Additional Information

- **Usage**: Use `glow` when you want to apply a glow effect and prepare the context for new drawing operations without preserving the current shape and clipping settings.

- **Performance Consideration**: Applying multiple glow effects can be computationally intensive. Optimize by minimizing the number of glow operations when possible.

- **Applications**: The `glow` function is useful for highlighting elements, creating neon effects, or adding emphasis to shapes in your graphics or UI designs.
