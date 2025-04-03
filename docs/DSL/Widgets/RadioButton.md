The `RadioButton` control allows users to select a single option from a set of choices. It can display text, icons, or images, and supports different styles like round buttons or tabs.

## Inherits

- **frame**: [[View]]

## [Layouting](Layouting.md)

Complete layouting feature set support.

## RadioButton

### DrawShaders

#### draw_radio ([DrawRadioButton](#drawradiobutton))

Determines the appearance of the `RadioButton`.

#### draw_icon ([DrawIcon](DrawIcon.md))

Displays a monochrome SVG vector next to the button's text or instead of it.

#### draw_text ([DrawText](DrawText.md))

Styles the button's text with various attributes like colors, font, and font size.

### Fields

#### bind (String)

Specifies the binding key for the RadioButton's value. This key is used to synchronize the RadioButton's selection state with the application data.
#### icon_walk ([Walk](Walk.md))
Controls the icon's inner layout properties as supported by [Walk](Walk.md).
#### image ([Image](DSL/FoundationalTypes/Image.md))
Image to be shown if [media](#media) is "Image"
#### label_align ([Align](Align.md))
Controls the text-label's alignment.
#### label_walk ([Walk](Walk.md))
Controls the label's inner layout properties as supported by [Walk](Walk.md).
#### media ([MediaType](../ft/ft_mediatype.md))
Select the type of visual representation that is shown next to the individual options.

* **Image**: Pixel data files like jpg and png.
* **Icon**: Vector data files like svg.
* **None**: No media.
#### text (RcStringMut)
The radio button's text label.
#### value (LiveValue)
The value that is assigned to the radio button.

## DrawShaders

### DrawRadioButton ([DrawQuad](DrawQuad.md))
#### Fields
##### RadioType

RadioButtons can operate in different modes.

- **Round**: The default mode.
    * Shader enum value: `shader_enum(1)`
- **Tab**: Tab-mode to be used with the TabBar and Dock widget.
    * Shader enum value: `shader_enum(2)`

### States
| State          | Trigger                               |
| :------------- | :------------------------------------ |
| focus (f32)    | Element gets focus                    |
| hover (f32)    | User moves the mouse over the element |
| selected (f32) | Element is picked                     |
## Widget presets & variations

- **RadioButtonCustom**: Allows showing a custom icon that can be toggled.
	- draw_icon
		- svg_file
- **RadioButtonTextual**: A text-only RadioButton
- **RadioButtonImage**: A RadioButton that supports showing a picture
- **RadioButtonTab**: A Tab-Mode RadioButton.

## Examples
### Basic
```rust
<RadioButton> { text: "Option" }
```
### Typical
```rust
<RadioButton> {
	text: "Option"
	bind: "selected_option"
	value: "option_value"
}
```
### Advanced
```rust
MyRadioButton = <RadioButton> {
	image: <Image> { source: dep("crate://self/resources/ducky.png" ) }
	bind: "selected_option"
	media: Image,
	text: "Option"
	value: "option_value"

	draw_icon: {
		instance hover: 0.0
		instance selected: 0.0
		uniform color: #333333
		uniform color_active: #999999
		fn get_color(self) -> vec4 {
			return mix(
				mix(
					self.color,
					mix(self.color, #f, 0.4),
					self.hover
				),
				mix(
					self.color_active,
					mix(self.color_active, #f, 0.75),
					self.hover
				),
				self.selected
			)
		}
	}

	icon_walk: { margin: { left: 20. } }

	draw_radio: {
		radio_type: Round,

		uniform size: 7.0;
		uniform border_radius: 4.0
		instance bodytop: #AAAAAA
		instance bodybottom: #666666

		fn pixel(self) -> vec4 {
			let sdf = Sdf2d::viewport(self.pos * self.rect_size)
			match self.radio_type {
				RadioType::Round => {
					let sz = self.size;
					let left = sz + 1.;
					let c = vec2(left + sz, self.rect_size.y * 0.5);
					sdf.circle(left, c.y, sz);
					sdf.fill_keep(mix(#333333, #181818, pow(self.pos.y, 1.)))
					sdf.stroke(mix(#333333, #CCCCCC, self.pos.y), 3.0)
					let isz = sz * 0.5;
					sdf.circle(left, c.y, isz);
					sdf.fill(
						mix(
							mix(
								#FFFFFF00,
								#AAAAAA,
								self.hover
							),
							#999999,
							self.selected
						)
					);
				}
				RadioType::Tab => {
					let grad_top = 5.0;
					let grad_bot = 1.0;
					let body = mix(
						mix(self.bodytop, #AAAAAA, self.hover),
						self.bodybottom,
						self.selected
					);
					let body_transp = vec4(body.xyz, 0.0);
					let top_gradient = mix(body_transp, mix(#CCCCCC, #333333, self.selected), max(0.0, grad_top - sdf.pos.y) / grad_top);
					let bot_gradient = mix(
						mix(body_transp, #CCCCCC, self.selected),
						top_gradient,
						clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
					);

					// the little drop shadow at the bottom
					let shift_inward = 0. * 1.75;
					sdf.move_to(shift_inward, self.rect_size.y);
					sdf.line_to(self.rect_size.x - shift_inward, self.rect_size.y);
					sdf.stroke(
						mix(
							#333333,
							#FFFFFF00,
							self.selected
						), 3.0 * 2.)

					sdf.box(
						1.,
						1.,
						self.rect_size.x - 2.0,
						self.rect_size.y - 2.0,
						1.
					)
					sdf.fill_keep(body)

					sdf.stroke(bot_gradient, 3.0 * 1.5)
				}
			}
			return sdf.result
		}
	}

	draw_text: {
		instance hover: 0.0
		instance selected: 0.0

		uniform color_unselected: #888888
		uniform color_unselected_hover: #AAAAAA
		uniform color_selected: #999999

		text_style: {
		// Controls the appearance of text.
			font: {path: dep("crate://self/resources/GoNotoKurrent-Bold.ttf")},
			// Font file dependency.

			font_size: 12.0, // Font-size of 12.0.
		}
		fn get_color(self) -> vec4 {
			return mix(
				mix(
					self.color_unselected,
					self.color_unselected,
					// self.color_unselected_hover,
					self.hover
				),
				self.color_unselected,
				// self.color_selected,
				self.selected
			)
		}
	}

	label_align: { y: 0.0 }

	label_walk: {
		width: Fit, height: Fit,
		margin: { left: 20. }
	}

	animator: {
		hover = {
			default: off
			off = {
				from: {all: Forward {duration: 0.15}}
				apply: {
					draw_radio: {hover: 0.0}
					draw_text: {hover: 0.0}
					draw_icon: {hover: 0.0}
				}
			}
			on = {
				from: {all: Snap}
				apply: {
					draw_radio: {hover: 1.0}
					draw_text: {hover: 1.0}
					draw_icon: {hover: 1.0}
				}
			}
		}
		selected = {
			default: off
			off = {
				from: {all: Forward {duration: 0.2}}
				apply: {
					draw_radio: {selected: 0.0}
					draw_icon: {selected: 0.0}
					draw_text: {selected: 0.0}
					draw_icon: {selected: 0.0}
				}
			}
			on = {
				cursor: Arrow,
				from: {all: Forward {duration: 0.0}}
				apply: {
					draw_radio: {selected: 1.0}
					draw_icon: {selected: 1.0}
					draw_text: {selected: 1.0}
					draw_icon: {selected: 1.0}
				}
			}
		}
	}

	// LAYOUT PROPERTIES

	height: 15.0,
	// Element is 15. high.

	width: Fill,
	// Element expands to use all available horizontal space.

	margin: { top: 10.0, right: 5.0, bottom: 10.0, left: 5.0 },
	// Individual margins outside the element for all four directions.

	padding: { top: 15.0, left: 0.0 },
	// Individual space between the element's border and its content
	// for top and left.

	flow: Right,
	// Stacks children from left to right.

	spacing: 10.0,
	// A spacing of 10.0 between children.

	align: { x: 0.0, y: 1.0 },
	// Positions children at the left (x) bottom (y) corner of the parent.

	clip_x: true,
	// Hides horizontal overflow.

	clip_y: false
	// Hides vertical overflow.

	line_spacing: 1.5

	scroll: vec2(0.0, 300.0)
}

<MyRadioButton> {}