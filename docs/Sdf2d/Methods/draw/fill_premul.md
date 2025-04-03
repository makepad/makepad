# fill_premul

```glsl
fn fill_premul(inout self, color: vec4) -> vec4
```

The `fill_premul` function fills the current shape in the `Sdf2d` drawing context with a **pre-multiplied** RGBA color. It blends the provided color with the existing content and resets the internal shape and clipping parameters, preparing the context for subsequent drawing operations.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance where the fill operation is performed. The function modifies the `result`, `shape`, `old_shape`, `clip`, and `has_clip` fields of `self` in place.
- **color** (`vec4`): A **pre-multiplied** RGBA color used to fill the shape.

## Returns

- **vec4**: The updated `result` color after the fill operation, reflecting the blended color of the input and the existing content. The shape and clipping parameters are reset after this operation.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Draw a rectangle with rounded corners.
    sdf.box(10.0, 10.0, 80.0, 80.0, 5.0);

    // Apply a semi-transparent red fill using pre-multiplied alpha.
    // The RGB components are multiplied by the alpha (0.5).
    sdf.fill_premul(vec4(1.0 * 0.5, 0.0 * 0.5, 0.0 * 0.5, 0.5));

    // Rotate the drawing by 45 degrees around the center of the rectangle.
    sdf.rotate(PI * 0.25, 50.0, 50.0);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: Initialize the `Sdf2d` context using the current position (`self.pos`) scaled by `self.rect_size`, which represents the size of the viewport.
- **Draw Shape**: Use `sdf.box` to draw a rectangle starting at position `(10.0, 10.0)` with a width and height of `80.0` units and corner radius of `5.0`.
- **Apply Fill**: Call `fill_premul` to fill the rectangle with a semi-transparent red color. Since `fill_premul` expects a pre-multiplied color, we multiply each RGB component by the alpha (`0.5`), resulting in `vec4(0.5, 0.0, 0.0, 0.5)`.
- **Reset State**: After `fill_premul`, the shape and clipping parameters are reset, allowing you to start drawing new shapes without manually resetting the state.
- **Rotate Drawing**: Rotate the entire drawing context by 45 degrees (`PI * 0.25` radians) around the point `(50.0, 50.0)`, which is the center of the rectangle.
- **Return Result**: Return `sdf.result`, which contains the final rendered color after all drawing operations.

### Notes

- **Pre-multiplied Alpha**: Ensure that the color provided to `fill_premul` is pre-multiplied, meaning each RGB component is multiplied by the alpha component. This is crucial for correct blending and transparency effects.
- **State Reset**: Unlike `fill_keep_premul`, the `fill_premul` function resets the internal shape and clipping state after execution. This means you can define new shapes immediately after calling `fill_premul` without manual resets.
- **Usage**: Use `fill_premul` when you want to fill a shape and prepare the context for new drawing operations without preserving the current shape and clipping settings.
- **Transformations**: You can apply transformations such as `rotate`, `translate`, or `scale` to manipulate the drawing context as needed. In this example, `rotate` is used to rotate the entire drawing.

### Important Considerations

- **Color Accuracy**: When working with pre-multiplied alpha, it's essential to pre-multiply the RGB components to ensure accurate color blending.
- **Alpha Blending**: Pre-multiplied alpha helps in avoiding artifacts in blending, especially when rendering semi-transparent textures or layers.
- **Subsequent Operations**: Since `fill_premul` resets the shape and clipping state, any shapes drawn before calling it will not affect subsequent drawing operations. Plan your drawing sequence accordingly.
