The `RotatedImage` widget displays an image that can be scaled, allowing for dynamic transformations. It's useful for creating interactive and visually engaging interfaces where images need to be manipulated.

## [Layouting](Layouting.md)

Complete layouting feature set support.

## DrawShader

### draw_bg ([DrawColor](DrawColor.md))

The `DrawColor` shader responsible for rendering the image with scaling transformations.

## Fields

### source (LiveDependency)

Path to the image file. This is a `LiveDependency` that specifies the image to be displayed.

### scale (f64)

The scaling factor for the image. A value of `1.0` displays the image at its original size.

## Examples

### Scaled Image

```Rust
<RotatedImage> {
    source: dep("crate://self/resources/logo.png"),
    scale: 0.5, // Half the original size
}
```

Displays the image scaled to half its original size.

### Advanced

```Rust
MyRotatedImage = <RotatedImage> {
    source: dep("crate://self/resources/my_logo.png"),
    scale: 2.0, // Double the original size

    draw_bg: {
        // Custom shader adjustments if needed
    }

    // LAYOUT PROPERTIES

    height: 200.0,
    width: Fit,
    margin: 10.0,
    padding: 0.0,
}
```

Defines a custom `MyRotatedImage` with specific scaling and layout properties.
