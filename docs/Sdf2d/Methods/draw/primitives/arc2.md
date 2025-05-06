# arc2

```glsl
fn arc2(inout self, x: float, y: float, r: float, s: float, e: float)
```

The `arc2` function draws an arc segment of a circle within the `Sdf2d` drawing context. It defines a portion of a circle centered at `(x, y)` with radius `r`, starting from angle `s` and ending at angle `e`. The arc is shaped based on the current position within the drawing context.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance. The function modifies the internal state of `self` to represent the arc shape.
- **x** (`float`): The x-coordinate of the circle's center.
- **y** (`float`): The y-coordinate of the circle's center.
- **r** (`float`): The radius of the circle.
- **s** (`float`): The start angle of the arc in radians. Angles are measured from the positive x-axis.
- **e** (`float`): The end angle of the arc in radians.

## Returns

- This function does not return a value but updates the internal shape within the `Sdf2d` context to include the arc.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
    
    // Draw an arc centered at (50.0, 50.0) with a radius of 40.0,
    // starting from 0 radians (0 degrees) to PI radians (180 degrees).
    sdf.arc2(50.0, 50.0, 40.0, 0.0, PI);
    
    // Apply a solid red fill to the arc.
    sdf.fill(#f00);
    
    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: Initialize the `Sdf2d` context using the current position (`self.pos`) and size (`self.rect_size`) of the viewport.
- **Draw Arc**: Use `arc2` to draw a semicircular arc centered at `(50.0, 50.0)` with a radius of `40.0`. The arc starts from `0.0` radians (corresponding to the positive x-axis) and ends at `PI` radians (180 degrees), forming the upper half of a circle.
- **Apply Fill**: Call `fill` with the color `#f00` (solid red) to fill the arc shape.
- **Return Result**: Return `sdf.result`, which contains the final rendered color.

## Notes

- **Angle Measurements**: Angles `s` and `e` are measured in radians. A full circle is `2 * PI` radians. Use `PI / 2` for 90 degrees, `PI` for 180 degrees, etc.
- **Drawing Order**: The `arc2` function modifies the shape used by the fill functions. Ensure you call `fill`, `fill_keep`, or another drawing function after defining the arc to render it.
- **Transformations**: You can apply transformations like `translate`, `rotate`, or `scale` before drawing the arc to position and orient it as needed.

`````