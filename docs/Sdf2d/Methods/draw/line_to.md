# line_to

```glsl
fn line_to(inout self, x: float, y: float)
```

The `line_to` function draws a line segment from the current position (`self.last_pos`) to a specified point (`x`, `y`). This updates the path in the `Sdf2d` drawing context, allowing you to create complex shapes by connecting multiple points.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance. The function modifies internal fields (`dist`, `shape`, `old_shape`, `clip`, `has_clip`, `last_pos`) to reflect the new line segment.
- **x** (`float`): The x-coordinate of the line's endpoint.
- **y** (`float`): The y-coordinate of the line's endpoint.

## Returns

- **void**: This function does not return a value but updates the internal state of `self` to include the new line segment in the current path.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Start a new path at point (30.0, 30.0).
    sdf.move_to(30.0, 30.0);

    // Draw lines to create a star shape.
    sdf.line_to(50.0, 70.0);   // Line to point (50.0, 70.0).
    sdf.line_to(70.0, 30.0);   // Line to point (70.0, 30.0).
    sdf.line_to(10.0, 50.0);   // Line to point (10.0, 50.0).
    sdf.line_to(90.0, 50.0);   // Line to point (90.0, 50.0).

    // Close the path to complete the shape.
    sdf.close_path();

    // Fill the shape with a solid red color.
    sdf.fill(#f00);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: Initialize the `Sdf2d` context using the current position (`self.pos`) and size (`self.rect_size`) of the viewport.

- **Start Path**: Set the starting point of the path at `(30.0, 30.0)` using `sdf.move_to`.

- **Draw Lines**: Use `sdf.line_to` to draw lines connecting to various points, constructing the outline of a star shape.

- **Close Path**: Use `sdf.close_path` to draw a line back to the starting point, ensuring that the shape is closed.

- **Apply Fill**: Fill the closed path with red color using `sdf.fill(#f00)`.

- **Return Result**: Return `sdf.result`, which contains the final rendered color after all drawing operations.

### Notes

- **Path Construction**: The combination of `move_to`, `line_to`, and `close_path` allows you to create complex vector shapes by defining their outlines point by point.

- **State Management**: Each call to `line_to` updates the current position (`self.last_pos`). The `close_path` function uses this position to connect back to the starting point (`self.start_pos`).

- **Drawing Order**: Ensure that you define your entire path and close it before applying fills or strokes to render the shape correctly.

- **Transformations**: You can apply transformations like `translate`, `rotate`, or `scale` to the `Sdf2d` context to manipulate the shape as needed.