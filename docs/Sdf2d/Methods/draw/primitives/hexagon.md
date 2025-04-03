# hexagon

```glsl
fn hexagon(inout self, x: float, y: float, r: float)
```

The `hexagon` function draws a regular hexagon within the `Sdf2d` drawing context. The hexagon is defined by its center coordinates and radius.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance. The function modifies the internal state of `self` to include the hexagon shape.
- **x** (`float`): The x-coordinate of the center of the hexagon.
- **y** (`float`): The y-coordinate of the center of the hexagon.
- **r** (`float`): The radius of the hexagon. This is the distance from the center to any vertex of the hexagon.

## Returns

- **void**: This function does not return a value but updates the internal state of `self` to represent the hexagon shape.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Draw a hexagon centered at (50.0, 50.0) with a radius of 40.0.
    sdf.hexagon(50.0, 50.0, 40.0);

    // Fill the hexagon with a solid red color.
    sdf.fill(#f00);

    // Rotate the drawing by 30 degrees around the center of the hexagon.
    sdf.rotate(PI / 6.0, 50.0, 50.0);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: We initialize an `Sdf2d` drawing context using the current position (`self.pos`) and the size of the rectangle (`self.rect_size`), representing the viewport area for drawing.
- **Draw Hexagon**: Use the `hexagon` function to draw a regular hexagon centered at `(50.0, 50.0)` with a radius of `40.0`.
- **Apply Fill**: Fill the hexagon with red color using `sdf.fill(#f00)`.
- **Rotate Drawing**: Rotate the entire drawing context by 30 degrees (`PI / 6.0` radians) around the center point `(50.0, 50.0)`. This affects all subsequent drawing operations and transformations.
- **Return Result**: Return `sdf.result`, which contains the final rendered color after all drawing operations.

### Notes

- **Regular Hexagon**: The hexagon drawn is a regular hexagon, meaning all sides and angles are equal.
- **Positioning**: The hexagon is centered at the coordinates `(x, y)`.
- **Radius**: The `r` parameter defines the size of the hexagon. It represents the distance from the center to any vertex.
- **Transformations**: Additional transformations like `translate`, `rotate`, or `scale` can be applied to the `Sdf2d` context to manipulate the drawing.
- **Angle Units**: Rotation angles are specified in radians. To convert degrees to radians, use the formula `radians = degrees * (PI / 180)`. In the example, 30 degrees is converted to radians by `PI / 6.0`.

### Additional Information

- **Drawing Order**: Ensure that you define the shape using `hexagon` before applying fills or other drawing operations. The `fill` function renders the shape onto the context.
- **Color Format**: The `#f00` color code is a shorthand hexadecimal notation for red (`#ff0000`). You can use other color codes or `vec4` values to specify different colors.
- **Sdf2d Context**: The `Sdf2d` drawing context allows for vector graphics rendering using signed distance fields, enabling smooth and scalable shapes with anti-aliasing.
