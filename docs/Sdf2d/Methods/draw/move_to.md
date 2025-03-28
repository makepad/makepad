# move_to

```glsl
fn move_to(inout self, x: float, y: float)
```

The `move_to` function sets the starting point of a new path in the `Sdf2d` drawing context. It updates the current position to the specified coordinates `(x, y)` without drawing a line from the previous position. This is commonly used when beginning a new shape or repositioning the drawing cursor without connecting it to the previous path.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance. The function modifies the `start_pos` and `last_pos` fields of `self` to the new position `(x, y)`.

- **x** (`float`): The x-coordinate of the new starting point.

- **y** (`float`): The y-coordinate of the new starting point.

## Returns

- **void**: This function does not return a value but updates the internal state of the `Sdf2d` context to reflect the new starting position.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Initialize the Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Move to the starting point of the path at (50.0, 20.0).
    sdf.move_to(50.0, 20.0);

    // Draw lines to create a triangle.
    sdf.line_to(80.0, 70.0);   // Line to (80.0, 70.0).
    sdf.line_to(20.0, 70.0);   // Line to (20.0, 70.0).
    sdf.close_path();          // Close the path back to (50.0, 20.0).

    // Fill the triangle with a solid red color.
    sdf.fill(#f00);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Initialize Drawing Context**: We create an `Sdf2d` instance for drawing, scaled to the current rectangle size.

- **Start New Path**: Use `sdf.move_to(50.0, 20.0)` to set the starting point of the path at `(50.0, 20.0)`.

- **Draw Lines**: Draw lines to two other points using `sdf.line_to`, forming the sides of a triangle.

- **Close Path**: Call `sdf.close_path()` to draw a line back to the starting point, completing the triangle.

- **Fill Shape**: Fill the shape with red color using `sdf.fill(#f00)`.

- **Result**: Return `sdf.result`, which contains the final rendered image.

### Notes

- **Starting Point**: The `move_to` function moves the drawing cursor without creating a line, essentially lifting the pen up and moving to a new location.

- **Path Construction**: Combining `move_to`, `line_to`, and `close_path` allows you to define complex shapes by specifying their vertices.

- **State Update**: After calling `move_to`, both `start_pos` and `last_pos` are updated to the new coordinates, preparing for subsequent drawing commands.
