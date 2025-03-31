# circle

```glsl
fn circle(inout self, x: float, y: float, r: float)
```

The `circle` function draws a circle within the `Sdf2d` drawing context. The circle is defined by its center coordinates and radius.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance. The function modifies the internal state of `self` to include the circle shape.
- **x** (`float`): The x-coordinate of the center of the circle.
- **y** (`float`): The y-coordinate of the center of the circle.
- **r** (`float`): The radius of the circle.

## Returns

- **void**: This function does not return a value but updates the internal state of `self` to represent the circle.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Draw a circle centered at (50.0, 50.0) with a radius of 40.0.
    sdf.circle(50.0, 50.0, 40.0);

    // Fill the circle with a solid red color.
    sdf.fill(#f00);

    // Rotate the drawing by 45 degrees around the center of the circle.
    sdf.rotate(PI * 0.25, 50.0, 50.0);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: Initialize the `Sdf2d` context using the current position (`self.pos`) and the size of the rectangle (`self.rect_size`), which represents the viewport area for drawing.
- **Draw Circle**: Use the `circle` function to draw a circle centered at `(50.0, 50.0)` with a radius of `40.0`.
- **Apply Fill**: Fill the circle with red color using `sdf.fill(#f00)`. The `#f00` is a shorthand for the color red in hexadecimal notation.
- **Rotate Drawing**: Rotate the entire drawing context by 45 degrees (`PI * 0.25` radians) around the center point `(50.0, 50.0)`. This affects all subsequent drawing operations and transformations applied to `sdf`.
- **Return Result**: Return `sdf.result`, which contains the final rendered color after all drawing operations.

### Notes

- **Positioning**: The circle is centered at the coordinates `(x, y)`. Adjust these values to position the circle within your drawing area.
- **Radius**: The `r` parameter defines the size of the circle. A larger radius creates a bigger circle.
- **Transformations**: The `rotate` function is used here to rotate the drawing. Other transformations like `translate` and `scale` can also be applied to manipulate the drawing context.
- **Drawing Order**: Ensure that you define the shape (using `circle`) before applying fills or other drawing operations. Calling `fill` or `fill_keep` renders the shape onto the context.

````