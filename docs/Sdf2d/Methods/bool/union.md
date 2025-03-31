# union

```glsl
fn union(inout self)
```

The `union` function combines the current shape in the `Sdf2d` drawing context with the previously stored shape, resulting in a new shape that encompasses both the current and the stored shapes. This operation is useful for creating complex shapes by combining simpler ones, effectively merging them into a single shape.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance. The function updates the internal `shape` and `old_shape` fields of `self` to represent the union of the shapes.

## Returns

- **void**: This function does not return a value but modifies the internal state of `self` to represent the combined shape.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Draw the first circle at position (40.0, 50.0) with a radius of 30.0.
    sdf.circle(40.0, 50.0, 30.0);

    // Store the current shape for the union operation.
    sdf.union();

    // Draw the second circle at position (70.0, 50.0) with a radius of 30.0.
    sdf.circle(70.0, 50.0, 30.0);

    // Combine the stored shape with the current shape using union.
    sdf.union();

    // Fill the combined shape with a solid red color.
    sdf.fill(#f00);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: We initialize an `Sdf2d` drawing context scaled to the size of the current rectangle (`self.rect_size`).

- **First Shape**: Draw a circle centered at `(40.0, 50.0)` with a radius of `30.0`.

- **Store Shape**: Use `sdf.union()` to store the current shape in `self.old_shape`, preparing to combine it with another shape.

- **Second Shape**: Draw another circle centered at `(70.0, 50.0)` with the same radius.

- **Combine Shapes**: Call `sdf.union()` again to combine the stored shape (`self.old_shape`) with the current shape (`self.shape`). The result is a new shape that encompasses both circles.

- **Apply Fill**: Fill the combined shape with red color using `sdf.fill(#f00)`.

- **Return Result**: Return `sdf.result`, which contains the final rendered image showing the union of the two circles.

### Notes

- **Shape Composition**: The `union` function allows you to combine shapes by merging their areas, effectively uniting them into a single shape.

- **Order of Operations**: When combining multiple shapes, you should call `sdf.union()` after drawing each shape to accumulate them. Each call to `sdf.union()` updates the internal state to include the new shape.

- **Transformation**: You can apply transformations like `translate`, `rotate`, or `scale` to manipulate the shapes before combining them.

- **Additional Shapes**: To combine more shapes, continue the pattern:
  - Draw a new shape.
  - Call `sdf.union()` to include it in the combined shape.

- **Further Effects**: After combining shapes, you can apply effects like `sdf.glow()` or `sdf.stroke()` to enhance the visual appearance.

- **Preserving State**: Since `sdf.union()` modifies the internal state but does not reset it, you can continue to add shapes or apply effects as needed.

### Important Considerations

- **Combining Multiple Shapes**: Ensure you call `sdf.union()` after each new shape you want to include in the union.

- **Understanding Internal State**: The `union` function updates `self.old_shape` and `self.shape` to represent the minimum of the current and stored shapes' signed distance fields, effectively merging them.

- **Performance**: Be mindful of the number of shapes combined, as complex unions may impact rendering performance.

### Alternative Example with More Shapes

```glsl
fn pixel(self) -> vec4 {
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Draw and combine multiple circles.
    sdf.circle(40.0, 50.0, 30.0);
    sdf.union(); // Store first circle.

    sdf.circle(70.0, 50.0, 30.0);
    sdf.union(); // Combine second circle.

    sdf.circle(55.0, 80.0, 30.0);
    sdf.union(); // Combine third circle.

    // Fill the combined shape.
    sdf.fill(#f00);

    return sdf.result;
}
```

In this alternative example:

- We draw three circles and call `sdf.union()` after each one to include all of them in the final combined shape.
