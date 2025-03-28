# Start of Selection
`TextStyle` is a configuration structure that defines various properties used to control the appearance of text. These properties include font, size, spacing, and other stylistic elements. This structure is essential for customizing how text is displayed within a widget, allowing for flexible and rich text formatting options.

## Attributes

### brightness (f32 = 1.0)
Controls the overall brightness of the rendered text. Higher values make the text brighter, while lower values make it darker.

### curve (f32 = 0.5)
Adjusts the curve of the text's anti-aliasing. A higher curve value increases the sharpness at the edges of glyphs, whereas lower values soften them.

### font ([Font](Font.md))
Specifies the font used to render the text. This can be set to a specific font family or font style, allowing for customization of text appearance.

### font_size (f64 = 9.0)
Determines the size of the text in points. Larger values increase the size of the text, and smaller values reduce it.

### height_factor (f64 = 1.3)
Defines the height factor applied to the text. This scales the vertical space occupied by each line of text, often used to adjust spacing relative to the font size.

### line_spacing (f64 = 1.4)
Specifies the amount of space between lines of text. A higher value increases the gap between lines, while a lower value reduces it.

### top_drop (f64 = 1.1)
Controls the vertical adjustment of the text relative to its baseline. It is typically used to fine-tune the position of text, especially when rendering at different sizes or with varying font styles.

## Example

```rust
<TextInput> {
    width: 200,
    height: Fit,
    draw_label: {
        text_style: {
            font: { path: dep("crate://self/resources/GoNotoKurrent-Regular.ttf") },
            font_size: 12.0,
            line_spacing: 1.5,
            top_drop: 1.2,
            height_factor: 1.4,
        }
    }
    // Additional properties can be added here as needed
}
```

*Example Explanation:*
This example demonstrates how to configure a `TextInput` widget with customized text styling. The `text_style` block specifies the font path, size, line spacing, top drop, and height factor to control the appearance of the text label within the input field.

# End of Selection
```