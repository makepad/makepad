# hline

```glsl
fn hline(inout self, y: float, h: float)
```

The `hline` function draws a horizontal line within the `Sdf2d` drawing context at a specified y-coordinate and with a specified half-thickness. This function is useful for adding horizontal lines or dividers in your graphics or UI elements.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance. The function modifies the internal state of `self` to include the horizontal line shape.
- **y** (`float`): The y-coordinate of the center of the horizontal line.
- **h** (`float`): The half-thickness of the line. The total thickness of the line will be `2 * h`.

## Returns

- **void**: This function does not return a value but updates the internal state of `self` to represent the horizontal line.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Draw a horizontal line at y = 50.0 with half-thickness 2.0.
    sdf.hline(50.0, 2.0);

    // Fill the horizontal line with a solid red color.
    sdf.fill(#f00);

    // Rotate the drawing by 90 degrees (PI * 0.5 radians) around point (50.0, 50.0).
    sdf.rotate(PI * 0.5, 50.0, 50.0);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: Initialize the `Sdf2d` context using the current position (`self.pos`) and size (`self.rect_size`) of the viewport.
- **Draw Horizontal Line**: Use `sdf.hline` to draw a horizontal line centered at `y = 50.0` with a half-thickness of `2.0`. This results in a line that is `4.0` units thick.
- **Apply Fill**: Fill the horizontal line with red color using `sdf.fill(#f00)`. The color `#f00` is shorthand for red in hexadecimal notation.
- **Rotate Drawing**: Rotate the entire drawing context by 90 degrees (`PI * 0.5` radians) around the point `(50.0, 50.0)`. This demonstrates how transformations can be applied to the drawing.
- **Return Result**: Return `sdf.result`, which contains the final rendered color after all drawing operations.

### Notes

- **Thickness Control**: Adjusting the `h` parameter changes the thickness of the line. A larger `h` results in a thicker line.
- **Positioning**: The line spans horizontally across the drawing area at the specified `y` coordinate.
- **Transformations**: Additional transformations like `translate`, `rotate`, or `scale` can be applied to the `Sdf2d` context to manipulate the position and orientation of the line.
- **Drawing Order**: Ensure that you call a fill function like `fill` or `fill_keep` after defining the shape to render it.e
