# fill_keep

```glsl
fn fill_keep(inout self, color: vec4) -> vec4
```

The `fill_keep` function blends an RGBA color with the current result color of the `Sdf2d` drawing context, converting the color to pre-multiplied alpha format before blending. It preserves the existing shapes and clipping settings, allowing you to apply additional fills without resetting the drawing state.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance where the blending operation is performed. The function modifies the `result` field of `self` in place.
- **color** (`vec4`): An RGBA color to be blended with the existing content.

## Returns

- **vec4**: The updated result color after the blending operation, reflecting the blending of the input color (converted to pre-multiplied alpha) with the existing content.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Draw a rectangle with rounded corners.
    sdf.box(10.0, 10.0, 80.0, 80.0, 5.0);

    // Apply a semi-transparent red fill, converting to pre-multiplied alpha.
    sdf.fill_keep(vec4(1.0, 0.0, 0.0, 0.5));

    // Draw another shape without resetting the state.
    // For example, draw a circle inside the rectangle.
    sdf.circle(50.0, 50.0, 30.0);

    // Apply a semi-transparent green fill, blending with the previous fills.
    sdf.fill_keep(vec4(0.0, 1.0, 0.0, 0.5));

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: We initialize an `Sdf2d` drawing context using the current position (`self.pos`) and size (`self.rect_size`) of the viewport.
- **Draw Rectangle**: We draw a box starting at position `(10.0, 10.0)` with a width and height of `80.0` units and rounded corners with a radius of `5.0`.
- **First Fill**: Use `fill_keep` to apply a semi-transparent red color (`vec4(1.0, 0.0, 0.0, 0.5)`) to the rectangle. The `fill_keep` function converts the color to pre-multiplied alpha and blends it with the existing content, preserving the current shape and clipping settings.
