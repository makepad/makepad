# fill

```glsl
fn fill(inout self, color: vec4) -> vec4
```

The `fill` function fills the current shape in the `Sdf2d` drawing context with an RGBA color. It converts the provided color to pre-multiplied alpha format before blending it with the existing content. This function also resets the internal shape and clipping parameters, preparing the context for subsequent drawing operations.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance where the fill operation is performed. The function modifies the `result`, `shape`, `old_shape`, `clip`, and `has_clip` fields of `self` in place.
- **color** (`vec4`): An RGBA color used to fill the shape.

## Returns

- **vec4**: The updated `result` color after the fill operation, reflecting the blended color of the input and the existing content. The shape and clipping parameters are reset after this operation.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
    
    // Draw a rectangle with rounded corners.
    sdf.box(10.0, 10.0, 100.0, 100.0, 5.0);
    
    // Apply a solid red fill to the rectangle.
    sdf.fill(#f00);
    
    // Rotate the drawing by 90 degrees around the center of the rectangle.
    sdf.rotate(PI * 0.5, 60.0, 60.0);
    
    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: We initialize an `Sdf2d` drawing context using the current position (`self.pos`) multiplied by `self.rect_size`, which represents the size of the viewport.
- **Draw Shape**: We use `sdf.box` to draw a rectangle starting at position `(10.0, 10.0)` with a width and height of `100.0` units and a corner radius of `5.0`.
- **apply fill**: we fill the rectangle with red color using `sdf.fill(#f00)`. The `fill` function converts the color to pre-multiplied alpha internally and blends it with the existing content.
- **Reset State**: After calling `fill`, the shape and clipping parameters are reset. This means we can define new shapes without manually resetting the state.
- **Rotate Drawing**: We rotate the entire drawing context by 90 degrees (`PI * 0.5` radians) around the point `(60.0, 60.0)`, which is the center of the rectangle.
- **Return Result**: We return `sdf.result`, which contains the final rendered color after all drawing operations.

### Notes

- **Color Conversion**: The `fill` function automatically converts the provided RGBA color to pre-multiplied alpha format before blending.
- **State Reset**: The function resets the internal shape and clipping state after execution, preparing the context for new drawing operations.
- **Order of Operations**: Ensure that you define the shape before calling `fill`. Any shapes defined after `fill` will require additional fill or stroke operations to render.