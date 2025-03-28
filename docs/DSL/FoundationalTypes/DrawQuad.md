`DrawQuad` is a versatile graphical component used for rendering 2D quads. It is essential for drawing rectangular shapes on the screen and managing their transformation, clipping, and layout within the user interface.

## geometry (`GeometryQuad2D`)
Specifies the geometric properties of the quad, including its size and shape. This defines how the quad is rendered in terms of its dimensions and form.

## Example
```rust
<Button> {
	// Allows instantiation of custom-styled elements, e.g., <MyButton> {}.
	
	// BUTTON SPECIFIC PROPERTIES
	draw_bg: { // Shader object responsible for drawing the background.
		fn get_color(self) -> vec4 { // Overrides the shader's fill method.
			return mix( // Implements state transition animations.
				mix(
					self.color,
					mix(self.color, #f, 0.5),
					self.hover
				),
				self.color_pressed,
				self.pressed
			)
		}
	},

	text: "I can be clicked", // Text label displayed on the button.

	// LAYOUT PROPERTIES
	height: Fit,
	width: Fit,
}
```

In this advanced example, the `<Button>` component customizes the `draw_bg` shader object to create interactive animations based on the button's state, such as `hover` and `pressed`. This allows for dynamic visual feedback, enhancing the user experience.
