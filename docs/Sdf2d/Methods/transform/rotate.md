# rotate

```glsl
fn rotate(inout self, a: float, x: float, y: float)
```

The `rotate` function applies a rotation transformation to the coordinate system of the `Sdf2d` drawing context around a specified pivot point. This transformation affects all subsequent drawing operations, allowing you to rotate shapes and paths within the context.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance. The function modifies `self.pos` in place, updating the coordinate system of the drawing context.
- **a** (`float`): The angle of rotation in radians. Positive values rotate **clockwise**, negative values rotate **counter-clockwise**.
- **x**, **y** (`float`): The x and y coordinates of the pivot point around which the rotation is performed.

## Returns

- **void**: This function does not return a value but updates the `self.pos` field of the `Sdf2d` context.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context relative to the viewport size.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Apply rotation by 90 degrees (PI * 0.5 radians) clockwise around point (50.0, 50.0).
    sdf.rotate(PI * 0.5, 50.0, 50.0);

    // Draw a rectangle with rounded corners after rotation.
    sdf.box(1.0, 1.0, 100.0, 100.0, 3.0);

    // Fill the rectangle with red color.
    sdf.fill(#f00);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: We initialize an `Sdf2d` drawing context scaled to the size of the rectangle (`self.rect_size`), which represents the viewport area for drawing.

- **Apply Rotation**: We apply a rotation of 90 degrees clockwise by calling `sdf.rotate(PI * 0.5, 50.0, 50.0)`. The rotation is performed around the pivot point `(50.0, 50.0)`, which is the center of the rectangle we will draw.

- **Draw Shape**: After applying the rotation, we draw a rectangle using `sdf.box(1.0, 1.0, 100.0, 100.0, 3.0)`. The rectangle starts at position `(1.0, 1.0)`, has a width and height of `100.0` units, and corners rounded with a radius of `3.0`.

- **Apply Fill**: We fill the rectangle with red color `#f00` by calling `sdf.fill(#f00)`.

- **Return Result**: We return `sdf.result`, which contains the final rendered color after all drawing operations.

### Notes

- **Rotation Direction**: The rotation angle `a` is in radians. Due to the implementation using `cos(-a)` and `sin(-a)`, positive values of `a` result in a clockwise rotation, which may differ from common mathematical conventions.

- **Pivot Point**: The rotation occurs around the specified pivot point `(x, y)`. Changing the pivot point adjusts the center of rotation, allowing for rotations around different points in your drawing.

- **Order of Operations**: The `rotate` function affects the coordinate system from the point it is called onward. Any drawing commands issued after the rotation will be rotated accordingly. To rotate existing shapes, apply the rotation before the drawing commands.

- **Transformations Stack**: Transformations such as `rotate`, `translate`, and `scale` are cumulative and affect subsequent drawing operations in the order they are applied.

### Alternative Example without Rotation

If you apply the rotation after drawing the shape, the rotation won't affect the shape:

```glsl
fn pixel(self) -> vec4 {
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Draw a rectangle without rotation.
    sdf.box(1.0, 1.0, 100.0, 100.0, 3.0);

    // Fill the rectangle with red color.
    sdf.fill(#f00);

    // Apply rotation (this won't affect the already drawn rectangle).
    sdf.rotate(PI * 0.5, 50.0, 50.0);

    return sdf.result;
}
```

In this case, the rectangle will not appear rotated because the rotation was applied after the drawing commands.

### Summary

The `rotate` function is particularly useful when you want to rotate shapes or patterns without manually calculating the rotated coordinates for each point. By rotating the drawing context before issuing drawing commands, all subsequent shapes are transformed accordingly, simplifying the process of rendering rotated elements in your graphics.
