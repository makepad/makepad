# rect

```glsl
fn rect(inout self, x: float, y: float, w: float, h: float)
```

The `rect` function draws a rectangle within the `Sdf2d` drawing context. The rectangle is defined by its position and dimensions, with the origin at the lower-left corner.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance. The function modifies the internal state of `self` to include the rectangle shape.
- **x** (`float`): The x-coordinate of the lower-left corner of the rectangle.
- **y** (`float`): The y-coordinate of the lower-left corner of the rectangle.
- **w** (`float`): The width of the rectangle.
- **h** (`float`): The height of the rectangle.

## Returns

- **void**: This function does not return a value but updates the internal state of `self` to represent the rectangle.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Draw a rectangle at position (10.0, 10.0) with width 80.0 and height 60.0.
    sdf.rect(10.0, 10.0, 80.0, 60.0);

    // Fill the rectangle with a solid red color.
    sdf.fill(#f00);

    // Rotate the drawing by 45 degrees (PI * 0.25 radians) around the center of the rectangle.
    sdf.rotate(PI * 0.25, 50.0, 40.0);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: Initialize the `Sdf2d` context using the current position (`self.pos`) and size (`self.rect_size`) of the viewport.
- **Draw Rectangle**: Use the `rect` function to draw a rectangle starting at position `(10.0, 10.0)` with a width of `80.0` units and a height of `60.0` units.
- **Apply Fill**: Fill the rectangle with red color using `sdf.fill(#f00)`.
- **Rotate Drawing**: Rotate the entire drawing context by 45 degrees (`PI * 0.25` radians) around the point `(50.0, 40.0)`, which is the center of the rectangle. This demonstrates how transformations can be applied to shapes.
- **Return Result**: Return `sdf.result`, which contains the final rendered color after all drawing operations.

### Notes

- **Positioning**: The rectangle is positioned with its lower-left corner at `(x, y)`. The width (`w`) and height (`h`) extend the rectangle to the right and upwards.
- **Transformations**: You can apply transformations such as `translate`, `rotate`, or `scale` to the `Sdf2d` context to adjust the position and orientation of the rectangle.
- **Drawing Order**: Ensure that you define the shape using `rect` before applying fills or other drawing operations. The `fill` function renders the shape onto the context.
