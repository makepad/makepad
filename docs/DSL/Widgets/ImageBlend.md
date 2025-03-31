# ImageBlend

This widget allows blending between multiple images.

## [Layouting](Layouting.md)

Complete layouting feature set support.

## DrawShaders

### draw_bg ([DrawQuad](DrawQuad.md))
Draws the background, including the images to be blended.

## Fields

### walk ([Walk](Walk.md))
Controls the widget's layout properties as supported by [Walk](Walk.md).

### min_width (`i64`)
Sets a minimal width for the image.

### min_height (`i64`)
Sets a minimal height for the image.

### width_scale (`f64` = `1.0`)
Scaling factor for the width of the images.

### fit ([ImageFit](ImageFit.md))
Determines how the image should fit within its bounds, as defined in [ImageFit](ImageFit.md).

### breathe (`bool` = `false`)
Determines whether the image should have a breathing animation.

### source (`LiveDependency`)
A `LiveDependency` path to the image file.

## States

| State       | Trigger                                           |
|-------------|---------------------------------------------------|
| `breathe.on`  | Widget is initialized and `breathe` is `true`     |

## Examples

### Basic
```rust
<ImageBlend> {
    source: dep("crate://self/resources/image.png"), // Path to the image file
}
```
A basic `ImageBlend` widget displaying a single image.

### With Breathing Animation
```rust
<ImageBlend> {
    source: dep("crate://self/resources/image.png"), // Path to the image file
    breathe: true,                                   // Enable breathing animation
}
```
An `ImageBlend` widget that plays a breathing animation when initialized.

### Advanced
```rust
<ImageBlend> {
    source: dep("crate://self/resources/image.png"), // Path to the image file
    min_width: 200,                                  // Minimum width for the image
    min_height: 150,                                 // Minimum height for the image
    width_scale: 1.5,                                // Scale the image width by 1.5
    fit: Contain,                                    // Image fit mode
    walk: {
        width: Fill,                                 // Fill available horizontal space
        height: Fit,                                 // Fit content vertically
    },
    draw_bg: {
        // Additional background styling
    },
}
```
An advanced `ImageBlend` widget with customized scaling, dimensions, fit mode, and layout properties.