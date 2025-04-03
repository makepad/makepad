# scale

```glsl
fn scale(inout self, f: float, x: float, y: float)
```

The `scale` function applies a scaling transformation to the coordinate system of the `Sdf2d` drawing context around a specified pivot point. This transformation scales all subsequent drawing operations, allowing you to zoom in or out relative to a specific point.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance. The function modifies `self.pos` and `self.scale_factor` in place, updating the coordinate system of the drawing context.
- **f** (`float`): The scaling factor. Values greater than `1.0` scale up (zoom in), values between `0.0` and `1.0` scale down (zoom out).
- **x**, **y** (`float`): The x and y coordinates of the pivot point around which the scaling is performed.

## Returns

- **void**: This function does not return a value but updates the `self.pos` and `self.scale_factor` fields of the `Sdf2d` context.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context relative to the viewport size.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Apply scaling by a factor of 0.5 around point (50.0, 50.0).
    sdf.scale(0.5, 50.0, 50.0);

    // Draw a rectangle with rounded corners after scaling.
    sdf.box(20.0, 20.0, 60.0, 60.0, 5.0);

    // Fill the rectangle with red color.
    sdf.fill(#f00);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: We initialize an `Sdf2d` drawing context scaled to the size of the viewport (`self.rect_size`).

- **Apply Scaling**: We apply a scaling transformation by a factor of `0.5` using `sdf.scale(0.5, 50.0, 50.0)`. The scaling is performed around the pivot point `(50.0, 50.0)`, effectively zooming out, as the scaling factor is less than `1.0`.

- **Draw Shape**: After applying the scaling, we draw a rectangle using `sdf.box(20.0, 20.0, 60.0, 60.0, 5.0)`. Due to the scaling, the rectangle is drawn at half its original size relative to the pivot point.

- **Apply Fill**: We fill the rectangle with red color `#f00` by calling `sdf.fill(#f00)`.

- **Return Result**: We return `sdf.result`, which contains the final rendered color after all drawing operations.

### Notes

- **Scaling Factor (`f`)**: The value of `f` determines the scaling. A value greater than `1.0` enlarges the drawing (zoom in), while a value between `0.0` and `1.0` reduces it (zoom out).

- **Pivot Point**: The scaling occurs relative to the pivot point `(x, y)`. Changing the pivot point allows you to zoom relative to different locations in your drawing.

- **Order of Operations**: The `scale` function affects the coordinate system from the point it is called onward. Any drawing commands issued after the scaling will be scaled accordingly. To scale existing shapes, apply the scaling before the drawing commands.

- **Cumulative Transformations**: Transformations such as `scale`, `rotate`, and `translate` are cumulative and affect subsequent drawing operations in the order they are applied.

- **Effect on Strokes and Line Widths**: Scaling affects all aspects of subsequent drawings, including stroke widths and line sizes, which are scaled accordingly.

### Important Considerations

- **Coordinate Transformation**: The `scale` function modifies the position `self.pos` and updates `self.scale_factor` using the formula:
  ```glsl
  self.scale_factor *= f;
  self.pos = (self.pos - vec2(x, y)) * f + vec2(x, y);
  ```
  This scales the distance from the current position to the pivot point by the scaling factor `f`.

- **Resetting Scale**: If you need to reset the scale for further drawing operations, you may need to apply an inverse scaling or reinitialize the `Sdf2d` context.

### Alternative Example with Zoom In

To zoom in instead of zooming out, use a scaling factor greater than `1.0`:

```glsl
fn pixel(self) -> vec4 {
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Apply scaling by a factor of 2.0 around point (50.0, 50.0).
    sdf.scale(2.0, 50.0, 50.0);

    // Draw a rectangle with rounded corners after scaling.
    sdf.box(10.0, 10.0, 80.0, 80.0, 5.0);

    // Fill the rectangle with blue color.
    sdf.fill(#00f);

    return sdf.result;
}
```

In this example:

- The scaling factor `2.0` zooms in, making the rectangle appear larger relative to the pivot point.

### Summary

The `scale` function is particularly useful when you want to zoom in or out of your drawing relative to a specific point. By scaling the drawing context before issuing drawing commands, all subsequent shapes are transformed accordingly, simplifying the process of rendering scaled elements in your graphics.
