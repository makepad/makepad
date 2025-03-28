# box

```glsl
fn box(inout self, x: float, y: float, w: float, h: float, r: float)
```

The `box` function draws a rectangle with rounded corners within the `Sdf2d` drawing context. The rectangle is defined by its position, dimensions, and a uniform corner radius applied to all corners.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance. The function modifies the internal state of `self` to include the box shape.
- **x** (`float`): The x-coordinate of the lower-left corner of the box.
- **y** (`float`): The y-coordinate of the lower-left corner of the box.
- **w** (`float`): The width of the box.
- **h** (`float`): The height of the box.
- **r** (`float`): The radius of the rounded corners.

## Returns

- **void**: This function does not return a value but updates the internal state of `self` to represent the box with rounded corners.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Draw a rectangle at position (10.0, 10.0) with width 100.0, height 100.0,
    // and corner radius of 5.0.
    sdf.box(10.0, 10.0, 100.0, 100.0, 5.0);

    // Fill the shape with a solid red color.
    sdf.fill(#f00);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: Initialize the `Sdf2d` context using the current position (`self.pos`) and size (`self.rect_size`) of the viewport.
- **Draw Box with Rounded Corners**: Use the `box` function to draw a rectangle starting at position `(10.0, 10.0)` with a width and height of `100.0` units and a corner radius of `5.0`. This creates a square with uniformly rounded corners.
- **Apply Fill**: Fill the shape with red color using `sdf.fill(#f00)`.
- **Return Result**: Return `sdf.result`, which contains the final rendered color.

### Notes

- **Positioning**: The box is positioned using the lower-left corner at `(x, y)`. The `w` and `h` parameters define the width and height, extending the box to the right and upwards.
- **Corner Radius**: The `r` parameter sets the radius for all four corners equally. Adjusting this value changes the rounding of the corners, with `0.0` resulting in sharp corners.
- **Drawing Order**: After defining the shape with `box`, use a fill function like `fill` or `fill_keep` to render it.
- **Transformations**: Transformations such as `translate`, `rotate`, or `scale` can be applied to the `Sdf2d` context to modify the position and orientation of the box.
