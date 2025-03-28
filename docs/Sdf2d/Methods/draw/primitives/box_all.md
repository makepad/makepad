# box_all

```glsl
fn box_all(
    inout self,
    x: float,
    y: float,
    w: float,
    h: float,
    r_left_top: float,
    r_right_top: float,
    r_right_bottom: float,
    r_left_bottom: float
)
```

The `box_all` function draws a rectangle within the `Sdf2d` drawing context, allowing each corner to have an individual radius. This enables the creation of rectangles with asymmetrical rounded corners.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance. The function modifies the internal state of `self` to include the box shape.
- **x** (`float`): The x-coordinate of the lower-left corner of the box.
- **y** (`float`): The y-coordinate of the lower-left corner of the box.
- **w** (`float`): The width of the box.
- **h** (`float`): The height of the box.
- **r_left_top** (`float`): The radius of the top-left corner.
- **r_right_top** (`float`): The radius of the top-right corner.
- **r_right_bottom** (`float`): The radius of the bottom-right corner.
- **r_left_bottom** (`float`): The radius of the bottom-left corner.

## Returns

- **void**: This function does not return a value but updates the internal state of `self` to represent the box with the specified rounded corners.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Draw a rectangle at position (10.0, 10.0) with width 80.0 and height 80.0.
    // Each corner has a different radius.
    sdf.box_all(
        10.0, // x position
        10.0, // y position
        80.0, // width
        80.0, // height
        0.0,  // top-left corner radius (sharp corner)
        10.0, // top-right corner radius
        20.0, // bottom-right corner radius
        5.0   // bottom-left corner radius
    );

    // Fill the shape with a solid red color.
    sdf.fill(#f00);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: We initialize an `Sdf2d` drawing context using the current position (`self.pos`) and size (`self.rect_size`) of the viewport.
- **Draw Box with Custom Corners**: We use `box_all` to draw a rectangle starting at position `(10.0, 10.0)` with a width and height of `80.0`. Each corner has a different corner radius:
  - **Top-left corner** (`r_left_top`): `0.0` (sharp corner)
  - **Top-right corner** (`r_right_top`): `10.0`
  - **Bottom-right corner** (`r_right_bottom`): `20.0`
  - **Bottom-left corner** (`r_left_bottom`): `5.0`
- **Apply Fill**: We fill the shape with red color using `sdf.fill(#f00)`.
- **Return Result**: Return `sdf.result`, which contains the final rendered color.

### Notes

- **Positioning**: The box is positioned using the lower-left corner at `(x, y)`. The width (`w`) extends the box to the right, and the height (`h`) extends it upwards.
- **Corner Radii**: By specifying different radii for each corner, you can create boxes with various rounded corner configurations, enabling more complex and custom shapes.
- **Drawing Order**: Ensure that you call a fill function like `fill` or `fill_keep` after defining the shape to render it.
- **Transformations**: You can apply transformations such as `translate`, `rotate`, or `scale` to the `Sdf2d` context before or after drawing the shape to position and orient it as needed.
