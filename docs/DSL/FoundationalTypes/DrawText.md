Draws styled text.
## Fields
### color (Vec4)
Defines the desired color as RGBA.

### combine_spaces (bool)
Combines multiple spaces into a single one. Useful for Markdown and HTML processing.

### ignore_newlines (bool)
Ignores newline characters in the text.

### text_style ([TextStyle](TextStyle.md))
Allows for styling of text with the features defined in [TextStyle](TextStyle.md).

### wrap ([TextWrap](#textwrap))
Selects how to handle line breaks.

#### TextWrap
* **Ellipsis**: *Currently not supported*.
  Overflow text is cut off and ellipsis are shown to signify hidden text.
* **Word**: Line breaks happen after complete words.
* **Line**: Line breaks occur at the end of each line.

### font_scale (f64)
Scales the font size of the text.

## Example
```rust
<Button> {
	draw_text: { // Shader object that draws the icon.
		wrap: Word, // Wraps text between words.
		text_style: {
		// Controls the appearance of text.
			font: {path: dep("crate://self/resources/GoNotoKurrent-Bold.ttf")},
			// Font file dependency.

			font_size: 12.0, // Font-size of 12.0.
		}

		fn get_color(self) -> vec4 { // Overwrite the shader's fill method.
			return mix( // State transition animations.
				mix(
					self.color,
					self.color_hover,
					self.hover
				),
				self.color_pressed,
				self.pressed
			)
		}
	}

	text: "I can be clicked", // Text label.

	// LAYOUT PROPERTIES
	height: Fit,
	width: Fit,
	margin: 5.0
	padding: { top: 3.0, right: 6.0, bottom: 3.0, left: 6.0 },
	flow: Right,
	spacing: 5.0,
	align: { x: 0.5, y: 0.5 },
	line_spacing: 1.5
}
```