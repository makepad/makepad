# stroke_keep

```glsl
fn stroke_keep(inout self, color: vec4, width: float) -> vec4
```

The `stroke_keep` function applies a stroke to the current shape within the `Sdf2d` drawing context, blending a specified color along the edge of the shape based on the stroke width. This function preserves the existing shape and clipping settings, allowing you to add strokes without resetting the drawing state.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance where the stroke operation is performed. The function modifies the `result` field of `self` in place.
- **color** (`vec4`): An RGBA color for the stroke.
- **width** (`float`): The width of the stroke. The stroke width is scaled according to the current `scale_factor` of the drawing context.

## Returns

- **vec4**: The updated result color after the stroke operation, reflecting the stroke blended with the existing content.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Draw a rectangle with rounded corners.
    sdf.box(10.0, 10.0, 80.0, 80.0, 5.0);

    // Apply a stroke with a width of 2.5 units and red color.
    sdf.stroke_keep(#f00, 2.5);

    // Optionally, fill the shape with another color without resetting the state.
    // sdf.fill_keep(#0f0);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: Initialize the `Sdf2d` context using the current position and size of the viewport.
- **Draw Shape**: Use `sdf.box` to draw a rectangle starting at position `(10.0, 10.0)` with a width and height of `80.0` units and a corner radius of `5.0`.
- **Apply Stroke**: Use `stroke_keep` to apply a red stroke (`#f00`) with a width of `2.5` units to the current shape. This stroke is blended along the edges of the shape without altering the existing drawing state.
- **Optional Fill**: You can fill the shape with another color using `sdf.fill_keep`, which allows for layering fills and strokes. In this example, it's commented out.
- **Return Result**: Return `sdf.result`, which contains the final rendered color after all drawing operations.

### Notes

- **Preserving State**: The `stroke_keep` function maintains the current shape and clipping settings, so you can perform additional operations on the same shape without resetting the state.
- **Stroke Width Scaling**: The `width` parameter is adjusted based on the `scale_factor` of the `Sdf2d` context, ensuring consistent stroke width regardless of transformations.
- **Color Specification**: Colors can be specified using hexadecimal notation (e.g., `#f00` for red) or as `vec4` values.
- **Order of Operations**: The order in which you apply strokes and fills affects the final rendering. Define the shape first, then apply strokes and fills as needed.
