# subtract

```glsl
fn subtract(inout self)
```

The `subtract` function modifies the current shape in the `Sdf2d` drawing context by subtracting the previously stored shape from it. The result is a new shape representing the area of the current shape minus the overlapping area with the stored shape. This operation is useful when you want to cut out a portion of a shape using another shape.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance. The function modifies the internal `shape` and `old_shape` fields of `self` to reflect the subtraction of the shapes.

## Returns

- **void**: This function does not return a value but updates the internal state of `self` to represent the subtracted shape.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Draw the base shape: a circle centered at (50.0, 50.0) with a radius of 40.0.
    sdf.circle(50.0, 50.0, 40.0);

    // Store the base shape for subtraction.
    sdf.union(); // Store the current shape.

    // Draw the subtracting shape: a rectangle overlapping the circle.
    sdf.rect(40.0, 40.0, 20.0, 20.0);

    // Subtract the stored shape (circle) from the current shape (rectangle).
    sdf.subtract();

    // Fill the resulting shape with a solid red color.
    sdf.fill(#f00);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: Initialize the `Sdf2d` context using the current position and size of the viewport.

- **Draw Base Shape**: Use `sdf.circle(50.0, 50.0, 40.0)` to draw a circle centered at `(50.0, 50.0)` with a radius of `40.0`.

- **Store Shape**: Call `sdf.union()` to store the current shape (the circle) in `self.old_shape`.

- **Draw Subtracting Shape**: Draw a rectangle overlapping the circle using `sdf.rect(40.0, 40.0, 20.0, 20.0)`.

- **Subtract Shapes**: Use `sdf.subtract()` to subtract the stored shape (the circle) from the current shape (the rectangle), resulting in a shape representing the rectangle minus the overlapping area with the circle.

- **Fill Shape**: Fill the resulting shape with red color using `sdf.fill(#f00)`.

- **Return Result**: Return `sdf.result`, which contains the final rendered image showing the subtracted shape.

### Notes

- **Shape Composition**: The `subtract` function allows you to create shapes by cutting out parts of one shape using another. This is useful for creating holes, notches, or complex silhouettes.

- **Order of Operations**: Ensure that you store the initial shape using `sdf.union()` before drawing the subtracting shape and applying `sdf.subtract()`.

- **Transformations**: You can apply transformations like `translate`, `rotate`, or `scale` to manipulate the shapes before subtracting them.

- **Combining Shapes**: The `subtract` function is part of a set of Boolean operations that allows you to combine shapes in various ways. Other related functions include `sdf.union()` and `sdf.intersect()`.

- **Further Effects**: After subtracting shapes, you can apply effects like `sdf.glow()` or `sdf.stroke()` to enhance the visual appearance.