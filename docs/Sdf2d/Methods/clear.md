# clear

```glsl
fn clear(inout self, color: vec4)
```

The `clear` function resets the current drawing result in the `Sdf2d` context by blending a specified color over the existing content, taking into account the alpha component for transparency. This is typically used to clear the drawing area before commencing new drawing operations.

## Parameters

- **self** (inout): A reference to the `Sdf2d` instance. The function modifies the `result` field of `self` in place.

- **color** (`vec4`): An RGBA color used to clear the drawing context.

## Returns

- **void**: This function does not return a value but updates the `result` field of `self` to reflect the new color after the blending operation.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context for the current viewport.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Clear the drawing context with a dark gray color.
    sdf.clear(#181818);

    // Draw a rectangle with rounded corners.
    sdf.box(10.0, 10.0, self.rect_size.x - 20.0, self.rect_size.y - 20.0, 5.0);

    // Fill the rectangle with red color.
    sdf.fill(#f00);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: Initialize the `Sdf2d` context using the current position scaled by `self.rect_size`.

- **Clear the Context**: Call `sdf.clear(#181818)` to clear the drawing context with a dark gray color (`#181818`). This blends the specified color over any existing content, taking the alpha component into account.

- **Draw Shape**: Draw a rectangle starting at position `(10.0, 10.0)`, with width and height adjusted to fit within the context (`self.rect_size.x - 20.0`, `self.rect_size.y - 20.0`), and with rounded corners of radius `5.0`.

- **Apply Fill**: Fill the rectangle with red color using `sdf.fill(#f00)`.

- **Return Result**: Return `sdf.result`, which contains the final rendered color.

### Notes

- **Blending Behavior**: The `clear` function blends the provided `color` with the existing content using the alpha component. If you want to completely overwrite the existing content, ensure that the alpha component is set to `1.0`.

- **Usage**: Use `sdf.clear()` at the beginning of your `pixel` function to reset the drawing context before starting new drawing operations.

- **Color Specification**: Colors can be specified using hexadecimal notation (e.g., `#181818` for dark gray) or as `vec4` values.

- **Order of Operations**: The `clear` function should be called before any drawing operations to ensure the context is reset.