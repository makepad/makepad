# DrawIcon

The `DrawIcon` component is used to draw vector icons in Makepad, supporting rendering of SVG files and inline SVG data. It allows for various customizations such as scaling, coloring, brightness adjustment, and more.

## Properties

### brightness (`f32`, default `1.0`)

Controls the brightness of the icon. A higher value increases the brightness, while a lower value decreases it.

### curve (`f32`, default `0.6`)

Adjusts the rendering curve of the icon, affecting the sharpness and contrast of edges.

### linearize (`f32`, default `0.5`)

Determines the level of linearization applied to the icon rendering, which can help smooth out details.

### color (`Vec4`)

Specifies the tint color of the icon using an RGBA color vector. This allows you to change the color of the icon to match your application's theme.

### scale (`f64`, default `1.0`)

A uniform scale factor that maintains the original aspect ratio of the icon. Increasing the scale will make the icon larger, while decreasing it will make it smaller.

### svg_file (`LiveDependency`)

Specifies the path to an SVG vector file. This should be a `LiveDependency` pointing to the desired SVG file to render.

### svg_path (`ArcStringMut`)

Allows for inline embedding of SVG data. This is useful if you need to include SVG content directly without referencing an external file.

### translate (`DVec2`)

Specifies a translation vector to move the graphic. This can be used to adjust the position of the icon within its container.

## Examples

### Basic

In this example, we draw an icon with a red tint and default brightness.

```rust
draw_icon: {
    color: #ff0000, // Red tint
    brightness: 1.0, // Default brightness
    svg_file: dep("crate://self/resources/icons/icon_image.svg"), // Path to SVG file
}
```

### Advanced

Here, we customize the `DrawIcon` within a `Button`, overriding the `get_color` function to implement state transition animations.

```rust
<Button> {
    draw_icon: { // Shader object that draws the icon
        svg_file: dep("crate://self/resources/icons/back.svg"), // Icon file dependency

        fn get_color(self) -> vec4 { // Override the shader's fill method
            return mix( // State transition animations
                mix(
                    self.color,
                    mix(self.color, #ffffff, 0.5),
                    self.hover
                ),
                self.color_pressed,
                self.pressed
            )
        }
    }

    icon_walk: {
        margin: 10.0,
        width: 16.0,
        height: Fit
    }

    text: "I can be clicked", // Text label

    // LAYOUT PROPERTIES
    height: Fit,
    width: Fit,
    margin: 5.0,
    padding: { top: 3.0, right: 6.0, bottom: 3.0, left: 6.0 },
    flow: Right,
    spacing: 5.0,
    align: { x: 0.5, y: 0.5 },
    line_spacing: 1.5
}
```

In this advanced example:

- We override the `get_color` function to animate the icon's color on hover and press states.
- `icon_walk` defines the layout properties for the icon within the button.
- The button includes a text label and various layout settings to control its appearance.

## Notes

- Ensure that the SVG files used are compatible with the icon atlas system in Makepad.
- Adjusting the `curve` and `linearize` properties can help fine-tune the rendering for different icon styles.
