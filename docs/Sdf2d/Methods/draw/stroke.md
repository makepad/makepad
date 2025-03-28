# stroke

```glsl
fn stroke(inout self, color: vec4, width: float) -> vec4
```

The `stroke` function applies a stroke to the current shape within the `Sdf2d` drawing context, blending a specified color along the edge of the shape based on the stroke width. Unlike `stroke_keep`, the `stroke` function resets the internal shape and clipping state after performing the stroke operation, making it ready for a new shape definition.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance where the stroke operation is performed. The function modifies the `result` and resets certain fields (`shape`, `old_shape`, `clip`, `has_clip`) of `self` in place.
- **color** (`vec4`): An RGBA color for the stroke.
- **width** (`float`): The width of the stroke. The stroke width is scaled according to the current `scale_factor` of the drawing context.

## Returns

- **vec4**: The updated result color after the stroke operation, reflecting the stroke blended with the existing content. The internal shape and clipping state are reset after this operation.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Draw a rectangle with rounded corners.
    sdf.box(10.0, 10.0, 80.0, 80.0, 5.0);

    // Apply a stroke with a width of 2.5 units and red color.
    sdf.stroke(#f00, 2.5);

    // Since 'stroke' resets the shape state, we can define a new shape.
    // For example, draw a circle.
    sdf.circle(50.0, 50.0, 30.0);

    // Apply a different stroke to the new shape.
    sdf.stroke(#00f, 1.5);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: Initialize the `Sdf2d` context using the current position and size of the viewport.
- **Draw First Shape**: Use `sdf.box` to draw a rectangle starting at position `(10.0, 10.0)` with a width and height of `80.0` units and a corner radius of `5.0`.
- **Apply Stroke to First Shape**: Use `stroke` to apply a red stroke (`#f00`) with a width of `2.5` units to the current shape (the rectangle). After this, the internal shape and clipping state are reset.
- **Draw Second Shape**: Define a new shape, such as a circle centered at `(50.0, 50.0)` with a radius of `30.0`.
- **Apply Stroke to Second Shape**: Apply a blue stroke (`#00f`) with a width of `1.5` units to the new shape.
- **Return Result**: Return `sdf.result`, which contains the final rendered color after all drawing operations.

### Notes

- **State Reset**: Unlike `stroke_keep`, the `stroke` function resets the internal shape (`shape`, `old_shape`) and clipping (`clip`, `has_clip`) state after execution. This means you can start defining a new shape immediately after without manual resets.
- **Order of Operations**: The order in which you apply strokes and define shapes affects the final rendering. Since `stroke` resets the shape state, ensure that you apply it after defining the shape you wish to stroke.
- **Stroke Width Scaling**: The `width` parameter is adjusted based on the `scale_factor` of the `Sdf2d` context, ensuring consistent stroke width regardless of transformations.
- **Color Specification**: Colors can be specified using hexadecimal notation (e.g., `#f00` for red, `#00f` for blue) or as `vec4` values.
- **Multiple Strokes**: By using `stroke` and defining new shapes between calls, you can apply multiple strokes to different shapes sequentially.
