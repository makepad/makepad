The button control.

## [Layouting](Layouting.md)
Complete layouting feature set support.

## DrawShaders
### draw_bg ([DrawQuad](DrawQuad.md))
Draws the background.
### draw_icon ([DrawIcon](DrawIcon.md))
Displays a monochrome svg vector next to the button's text or instead of it.
### draw_text ([DrawText](DrawText.md))
This allows styling of the button's text with all of the attributes supported by [DrawText](DrawText.md) including colors, font, font size and so on.
## Fields
### grab_key_focus (bool = true)
Determines whether the button should capture the keyboard focus when it is interacted with. 
### icon_walk ([Walk](Walk.md))
Controls the icon’s inner layout properties as supported by [Walk](Walk.md).
### label_walk ([Walk](Walk.md))
Controls the label’s inner layout properties as supported by [Walk](Walk.md).
### text (RcStringMut)
The text to be shown on the button.

## Widget presets & variations

### ButtonFlat
A flat design preset.
```rust
<ButtonFlat> {
	draw_icon: {
		color: #f00,
		svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
	}
	text: "I can have a lovely icon!"
}
```

### ButtonFlatter
A flat design preset that does not show a backdrop on mouse hover.
```rust
<ButtonFlatter> {
	draw_icon: {
		color: #f00,
		svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
	}
	text: "I can have a lovely icon!"
}
```

## States
| State         | Trigger                                             |
| :------------ | :-------------------------------------------------- |
| hover (f32)   | User moves the mouse over the element               |
| pressed (f32) | Mouse button is pushed down during click operations |

# Widget presets & variations
- **ButtonIcon**: A button that is prepared to show a monochrome svg graphic
	-  draw_icon
		- color
		- svg_file
- **ButtonFlat**: A button with a flat design that shows an embossed background on hover
- **ButtonFlatter**: A fully flat design button

# Examples
## Basic
```rust
<Button> { text: "I can be clicked" } // Default button with a custom label.
```

## Typical
```Rust
<Button> {
	text: "I can be clicked", // Text label.
	draw_bg: {
		color: #800, // Set the default color.
		bodytop: #f00, // Set the hover-state color.
		bodybottom: #400, // Set the pressed-state color.
	},
	height: Fit, // Element assumes height of its children.
	width: Fill, // Element expands to use all available horizontal space.
	margin: 10.0, // Homogenous spacing of 10.0 around the element.
	padding: 7.5  // Homogenous spacing of 7.5 between all the element's
				  // borders and its content.
}
```

## Advanced
```rust
MyButton = <Button> {
// Allows instantiation of customly styled elements as i.e. <MyButton> {}.

	// BUTTON SPECIFIC PROPERTIES

	draw_bg: { // Shader object that draws the bg.
		fn get_color(self) -> vec4 { // Overwrite the shader's fill method.
			return mix( // State transition animations.
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

	draw_icon: { // Shader object that draws the icon.
		svg_file: dep("crate://self/resources/icons/back.svg"),
		// Icon file dependency.

		fn get_color(self) -> vec4 { // Overwrite the shader's fill method.
			return mix( // State transition animations.
				mix(
					self.color,
					mix(self.color, #f, 0.5),
					self.hover
				),
				self.color_pressed,
				self.pressed
			)
		}
	}

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

	grab_key_focus: true, // Keyboard gets focus when clicked.

	icon_walk: {
		margin: 10.,
		width: 16.,
		height: Fit
	}

	label_walk: {
		margin: 0.,
		width: Fit,
		height: Fit,
	}

	text: "I can be clicked", // Text label.

    animator: { // State change triggered animations.
        hover = { // State
            default: off // The state's starting point.
			off = { // Behavior when the animation is started to the off-state
			  	from: { // Behavior depending on the prior states
					all: Forward {duration: 0.1}, // Default animation direction and speed in secs.
					pressed: Forward {duration: 0.25} // Direction and speed for 'pressed' in secs.
				}
				apply: { // Shader methods to animate
					draw_bg: { pressed: 0.0, hover: 0.0 } // Timeline target positions for the given states.
					draw_icon: { pressed: 0.0, hover: 0.0 }
					draw_text: { pressed: 0.0, hover: 0.0 }
				}
			}

			on = { // Behavior when the animation is started to the on-state
				from: {
					all: Forward {duration: 0.1},
					pressed: Forward {duration: 0.5}
				}
				apply: {
					draw_bg: { pressed: 0.0, hover: [{time: 0.0, value: 1.0}] },
					// pressed: 'pressed' timeline target position
					// hover, time: Normalized timeline from 0.0 - 1.0. 'duration' then determines the actual playback duration of this animation in seconds.
					// hover, value: target timeline position
					draw_icon: { pressed: 0.0, hover: [{time: 0.0, value: 1.0}] },
					draw_text: { pressed: 0.0, hover: [{time: 0.0, value: 1.0}] }
				}
			}
 
			pressed = { // Behavior when the animation is started to the pressed-state
				from: {all: Forward {duration: 0.2}}
				apply: {
					draw_bg: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0}, 
					draw_icon: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0},
					draw_text: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0}
				}
			}
		}
	}

	// LAYOUT PROPERTIES

	height: Fit,
	// Element assumes the height of its children.

	width: Fit,
	// Element assumes the width of its children.

	margin: 5.0
	padding: { top: 3.0, right: 6.0, bottom: 3.0, left: 6.0 },
	// Individual space between the element's border and its content
	// for top and left.

	flow: Right,
	// Stacks children from left to right.

	spacing: 5.0,
	// A spacing of 10.0 between children.

	align: { x: 0.5, y: 0.5 },
	// Positions children at the left (x) bottom (y) corner of the parent.

	line_spacing: 1.5
}

<MyButton> { // An instance of the styled button.
	text: "My Button Label" // Overwrites the text label property.
}
```
