Determines how an image is fitted within its container, controlling aspect ratio preservation and sizing behavior.

![Image Fit Modes](image_modes.png)

## Variants

### Size

Displays the image at its original size based on its intrinsic dimensions, adjusted by any specified scaling factors. The image maintains its aspect ratio and does not stretch to fill the container.

### Stretch

Stretches the image to fill the entire container, ignoring its original aspect ratio. Both width and height are scaled independently to match the container's dimensions.

### Horizontal

Scales the image to fit the width of the container while maintaining its aspect ratio. The height is adjusted proportionally based on the image's aspect ratio.

### Vertical

Scales the image to fit the height of the container while maintaining its aspect ratio. The width is adjusted proportionally based on the image's aspect ratio.

### Smallest

Scales the image to the smallest possible size that fits entirely within the container while maintaining its aspect ratio. Both width and height are adjusted to ensure the image does not overflow the container.

### Biggest

Scales the image to the largest possible size that covers the entire container while maintaining its aspect ratio. Portions of the image may overflow the container if the aspect ratios do not match.

## Example

```rust
// Display an image using different fit strategies
<Image> {
    // Set the fitting strategy
    fit: Stretch,
    // Minimum dimensions in device-independent pixels
    min_width: 100.0,
    min_height: 100.0,
    // Optionally scale the width
    width_scale: 1.0,
    // Source of the image
    source: dep("crate://self/resources/logo.png"),

    // Layout properties
    width: Fill,
    height: Fixed(150.0),
    margin: { left: 5.0, right: 5.0, top: 5.0, bottom: 5.0 },
}
```

In this example:

- `fit: ImageFit::Stretch` stretches the image to fill the available space.
- `min_width` and `min_height` ensure the image does not scale below these dimensions.
- `width_scale` allows you to adjust the scaling of the image's width.
- `source` specifies the path to the image resource.
- The `width` and `height` define how the image occupies space within its parent container.
- `margin` adds space around the image.