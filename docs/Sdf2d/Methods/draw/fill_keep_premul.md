# fill_keep_premul

```glsl
fn fill_keep_premul(inout self, source: vec4) -> vec4
```

The `fill_keep_premul` function blends a pre-multiplied source color with the current result color of the SDF2D drawing context, preserving the existing content while applying the new fill.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance where the blending operation is performed. The function modifies the `result` field of `self` in place.
- **source** (`vec4`): A pre-multiplied RGBA color to be blended with the existing content.

## Returns

- **vec4**: The updated result color after the blending operation, reflecting the combination of the source color and the existing content.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Draw a rectangle with rounded corners.
    sdf.box(10.0, 10.0, 80.0, 80.0, 5.0);

    // Apply a semi-transparent red fill while keeping existing content.
    sdf.fill_keep_premul(vec4(1.0, 0.0, 0.0, 0.5));

    // Rotate the drawing by 45 degrees around the center of the rectangle.
    sdf.rotate(PI * 0.25, 50.0, 50.0);

    // Return the final color result.
    return sdf.result;
}
```

In this example:

- **Create Drawing Context**: We initialize an `Sdf2d` drawing context using the current position (`self.pos`) and size (`self.rect_size`) of the viewport.
- **Draw Shape**: We draw a box starting at position `(10.0, 10.0)` with a width and height of `80.0` units and rounded corners with a radius of `5.0`.
- **Apply Fill**: The `fill_keep_premul` function is used to blend a semi-transparent red color (`vec4(1.0, 0.0, 0.0, 0.5)`) with the existing content. This preserves any previous fills or drawings while adding the new color.
- **Rotate Drawing**: We rotate the entire drawing context by 45 degrees (`PI * 0.25` radians) around the point `(50.0, 50.0)`, which is the center of the rectangle.
- **Return Result**: The final color result is returned, which includes all the drawing operations performed.

This function is particularly useful when you want to add a new fill color to your drawing without overwriting the existing content, allowing for complex blending effects in your shaders.