# box_y

```glsl
fn box_y(inout self, x: float, y: float, w: float, h: float, r_top: float, r_bottom: float)
```

The `box_y` function draws a rectangle within the `Sdf2d` drawing context, allowing you to specify different corner radii for the top and bottom sides. This enables the creation of rectangles with asymmetric rounded corners along the vertical axis.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance. The function modifies the internal state of `self` to include the box shape.
- **x** (`float`): The x-coordinate of the lower-left corner of the box.
- **y** (`float`): The y-coordinate of the lower-left corner of the box.
- **w** (`float`): The width of the box.
- **h** (`float`): The height of the box.
- **r_top** (`float`): The radius of the top corners (top-left and top-right).
- **r_bottom** (`float`): The radius of the bottom corners (bottom-left and bottom-right).

## Returns

- **void**: This function does not return a value but updates the internal state of `self` to represent the box with the specified corner radii.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Draw a rectangle at position (10.0, 10.0) with width 80.0 and height 100.0.
    // Top corners have a radius of 15.0, bottom corners have a radius of 5.0.
    sdf.box_y(
        10.0,   // x position
        10.0,   // y position
        80.0,   // width
        100.0,  // height
        15.0,   // radius of top corners
        5.0     // radius of bottom corners
    );

    // Apply a solid red fill to the box.
    sdf.fill(#f00);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: We initialize an `Sdf2d` drawing context using the current position (`self.pos`) and size (`self.rect_size`) of the viewport.
- **Draw Box with Asymmetric Corners**: We use `box_y` to draw a rectangle starting at position `(10.0, 10.0)` with a width of `80.0` units and a height of `100.0` units. The top corners (`r_top`) have a radius of `15.0`, and the bottom corners (`r_bottom`) have a radius of `5.0`.
- **Apply Fill**: We fill the shape with red color using `sdf.fill(#f00)`.
- **Return Result**: Return `sdf.result`, which contains the final rendered color.

### Notes

- **Positioning**: The box is positioned using the lower-left corner at `(x, y)`. Width (`w`) and height (`h`) define the size of the box.
- **Corner Radii**: Specifying different radii for the top and bottom sides allows for asymmetrical designs, useful for creating UI elements like tabs, tooltips, or headers with distinctive styles.
- **Drawing Order**: After defining the shape with `box_y`, use a fill function like `fill` or `fill_keep` to render it.
- **Transformations**: You can apply transformations such as `translate`, `rotate`, or `scale` to the `Sdf2d` context to adjust the position and orientation of the box as needed.
