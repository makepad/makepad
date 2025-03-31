# intersect

```glsl
fn intersect(inout self)
```

The `intersect` function modifies the current shape in the `Sdf2d` drawing context by intersecting it with the previously stored shape. The result is a new shape that represents the area where both shapes overlap. This operation is useful when you want to create complex shapes by combining simpler ones based on their overlapping regions.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance. The function modifies the internal `shape` and `old_shape` fields of `self` to reflect the intersection of the shapes.

## Returns

- **void**: This function does not return a value but updates the internal state of `self` to represent the intersected shape.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Draw the first circle at position (40.0, 50.0) with a radius of 30.0.
    sdf.circle(40.0, 50.0, 30.0);

    // Store the current shape for intersection.
    sdf.union(); // Store the first shape.

    // Draw the second circle at position (60.0, 50.0) with a radius of 30.0.
    sdf.circle(60.0, 50.0, 30.0);

    // Perform intersection between the current shape and the stored shape.
    sdf.intersect();

    // Fill the intersected area with a solid red color.
    sdf.fill(#f00);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: We initialize an `Sdf2d` drawing context scaled to the size of the current rectangle (`self.rect_size`).

- **First Shape**: We draw a circle centered at `(40.0, 50.0)` with a radius of `30.0`.

- **Store Shape**: By calling `sdf.union()`, we save the current shape into `self.old_shape`. This prepares it for the intersection operation.

- **Second Shape**: We draw another circle centered at `(60.0, 50.0)` with the same radius.

- **Apply Intersection**: Using `sdf.intersect()`, we modify the `shape` field to represent the intersection between the current shape (second circle) and the stored shape (first circle). The result is the overlapping area of the two circles.

- **Fill Shape**: We fill the intersected area with red color using `sdf.fill(#f00)`.

- **Return Result**: The function returns `sdf.result`, which contains the final rendered image showing the intersected shape.

### Notes

- **Shape Composition**: The `intersect` function is useful when you need to create shapes based on the common area of multiple shapes, allowing for more complex designs.

- **Order of Operations**: Ensure that you store the initial shape using `sdf.union()` before drawing the second shape and applying `sdf.intersect()`.

- **Transformation**: You can apply transformations like `translate`, `rotate`, or `scale` to manipulate the shapes before intersecting them.

- **Combining Shapes**: The `intersect` function is part of a set of Boolean operations that allows you to combine shapes in various ways. Other related functions include `sdf.union()` and `sdf.subtract()`.

- **Further Effects**: After intersecting shapes, you can apply effects like `sdf.glow()` or `sdf.stroke()` to enhance the visual appearance.