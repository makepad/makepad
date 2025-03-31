# viewport

```glsl
fn viewport(pos: vec2) -> Self
```

The `viewport` function creates and returns a new instance of the `Sdf2d` drawing context, initialized with default values for rendering operations. This function sets up a new drawing context with predefined settings such as position, anti-aliasing, clipping, and other rendering-related parameters. It is commonly used to initialize the drawing context within the `pixel` shader function.

## Parameters

- **pos** (`vec2`): A vector representing the position within the viewport or the normalized device coordinates (NDC). The components are:
  - **x**: The x-coordinate, typically ranging from `0.0` to `1.0` across the viewport's width.
  - **y**: The y-coordinate, typically ranging from `0.0` to `1.0` across the viewport's height.

## Returns

- **Self**: A new instance of the `Sdf2d` drawing context, initialized with default settings. The returned object contains various fields related to the context's state, such as position, anti-aliasing factor, scale factor, and initial shapes.

## Example

```glsl
fn pixel(self) -> vec4 {
    // Create an Sdf2d drawing context relative to the viewport size.
    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

    // Draw a rectangle with rounded corners.
    sdf.box(1.0, 1.0, 100.0, 100.0, 3.0);

    // Fill the rectangle with red color.
    sdf.fill(#f00);

    // Rotate the drawing by 90 degrees (PI * 0.5 radians) around point (50.0, 50.0).
    sdf.rotate(PI * 0.5, 50.0, 50.0);

    // Return the final color result.
    return sdf.result;
}
```

### Explanation

In this example:

- **Create Drawing Context**: We initialize an `Sdf2d` drawing context by scaling `self.pos` (the normalized position in the viewport) by `self.rect_size` (the size of the rectangle or viewport). This maps the position to pixel coordinates.

- **Draw Shape**: We use `sdf.box` to draw a rectangle starting at position `(1.0, 1.0)` with a width and height of `100.0` units, and a corner radius of `3.0`.

- **Apply Fill**: We fill the rectangle with red color using `sdf.fill(#f00)`.

- **Rotate Drawing**: We rotate the entire drawing context by 90 degrees (`PI * 0.5` radians) around the point `(50.0, 50.0)`.

- **Return Result**: We return `sdf.result`, which contains the final rendered color after all drawing operations.

### Notes

- **Initialization**: The `viewport` function initializes the `Sdf2d` context with default values:
  - `pos`: Set to the input `pos` value.
  - `result`: Initialized to `vec4(0.0)`, representing a transparent black color.
  - `scale_factor`: Initialized to `1.0`, affecting scaling transformations.
  - Other internal fields like `shape`, `old_shape`, `clip`, and `has_clip` are set to their default values to prepare for drawing operations.

- **Context Usage**: After creating the `Sdf2d` context with `viewport`, you can perform various drawing operations like drawing shapes, applying transformations, and rendering effects.

- **Coordinate Mapping**: Multiplying `self.pos` by `self.rect_size` maps the normalized device coordinates to the actual pixel coordinates of the viewport, which is necessary for accurate positioning of shapes.

- **Transformation Order**: Remember that transformations like `rotate`, `scale`, and `translate` affect all subsequent drawing commands. Apply them in the correct order to achieve the desired effect.

- **Anti-Aliasing**: The `aa` (anti-aliasing) factor is automatically initialized based on the input `pos`, helping to smooth edges and improve visual quality.

### Additional Information

- **Custom Initialization**: You can modify the initialization by directly setting or modifying the fields of the `Sdf2d` context after creating it with `viewport`, if needed.

- **Multiple Viewports**: If you need to render multiple separate drawing contexts, you can create multiple `Sdf2d` instances using `viewport` with different positions or contexts.

- **Integration with Shader Code**: The `viewport` function is typically used within the `pixel` shader function to initialize the drawing context for per-pixel operations.