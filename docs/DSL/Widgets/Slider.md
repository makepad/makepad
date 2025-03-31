Graphical numeric value control.
## [Layouting](Layouting.md)
Complete layouting feature set support.
## Slider

### DrawShaders
#### draw_slider ([DrawSlider](#drawslider))
Reference to the DrawShader that determines the look of the slider.
#### draw_text ([DrawText](DrawText.md))
This allows styling of the button's text with all of the attributes supported by [DrawText](DrawText.md) including colors, font, font size and so on.
### Fields
#### bind (String)
Binds the slider to a data field.
#### default (f64)
The default value of the slider.
#### label_align ([Align](Align.md))
Controls the placement of the text label within the slider.
#### label_walk ([Walk](Walk.md))
Controls the label's inner layout properties as supported by [Walk](Walk.md).
#### max (f64)
The maximum value that the slider lets users select.
#### min (f64)
The minimum value that the slider lets users select.
#### precision (usize)
The number of decimal places displayed by the slider.
#### step (f64)
Allows for stepping the selectable number in defined increments.
For instance, if the minimum value is `0.0` and the maximum value is `1.0`, then a step size of `0.1` would lead to ten selectable options (`0.0`, `0.1`, `0.2`, ..., `1.0`).
#### text (String)
The slider's text label.
#### text_input ([TextInput](TextInput.md))
The text input widget that allows for selecting desired values by keyboard.
#### hover_actions_enabled (bool)
Indicates if the label of the slider responds to hover events. The primary use case for this kind of emitted actions is for displaying tooltips, and it is turned on by default since this component already consumes mouse events.
### DrawSlider
#### DrawShaders
##### draw_super ([DrawQuad](DrawQuad.md))
The shader that determines the look of the slider.
#### Fields
##### slide_pos (f32)
The current position of the slider handle, as a normalized value between `0.0` and `1.0`.
##### slider_type ([SliderType](#slidertype))
Selects the desired slider type.
###### SliderType
An enumeration of slider types:

* **Horizontal**: A horizontally oriented slider.
* **Vertical**: A vertical slider. *Note: Not implemented yet.*
* **Rotary**: A radial slider. *Note: Not implemented yet.*
### Subwidgets
#### text_input ([TextInput](TextInput.md))
The text input widget that allows for entering the desired value directly via the keyboard.
### States
| State       | Trigger                               |
| :---------- | :------------------------------------ |
| drag (f32)  | Element is dragged with the mouse     |
| focus (f32) | Element gets focus                    |
| hover (f32) | User moves the mouse over the element |
### Widget presets & variations
- **SliderBig**: A 3D-design slider.
### Examples
#### Basic
```rust
// Basic example: a simple slider with default settings
<Slider> {}
```
#### Typical
```rust
// Typical example: a slider configured with specific parameters
<Slider> {
    bind: "value", // Binds to the "value" data field
    default: 0.0, // Default value
    max: 5.0, // Upper bound
    min: 0.0, // Lower bound
    precision: 4, // Number of decimal places
    step: 0.25, // Stepping the available values
    text: "Amount", // Label text

    // LAYOUT PROPERTIES

    height: 15.0,
    // Element is 15 pixels high

    width: Fill,
    // Element expands to use all available horizontal space
}
```
#### Advanced
```rust
// Advanced example: a customized slider with modified appearance and behavior
MySlider = <Slider> {
    bind: "custom_value", // Binds to the "custom_value" data field
    default: 0.0, // Default value
    max: 5.0, // Upper bound
    min: 0.0, // Lower bound
    precision: 4, // Number of decimal places
    step: 0.25, // Stepping the available values
    text: "Amount", // Label text

	draw_slider: {
		instance line_color: #f00
		instance bipolar: 0.0
		fn pixel(self) -> vec4 {
			let nub_size = 3

			let sdf = Sdf2d::viewport(self.pos * self.rect_size)
			let top = 20.0;

			sdf.box(1.0, top, self.rect_size.x - 2, self.rect_size.y - top - 2, 1);
			sdf.fill_keep(
				mix(
					mix(#00000044, #00000044 * 0.1, pow(self.pos.y, 1.0)),
					mix(#00000044 * 1.75, #00000044 * 0.1, pow(self.pos.y, 1.0)),
					self.drag
				)
			) // Control backdrop gradient

			sdf.stroke(mix(mix(#x00000060, #x00000070, self.drag), #xFFFFFF10, pow(self.pos.y, 10.0)), 1.0) // Control outline
			let in_side = 5.0;
			let in_top = 5.0; // Ridge: vertical position
			sdf.rect(1.0 + in_side, top + in_top, self.rect_size.x - 2 - 2 * in_side, 3);
			sdf.fill(mix(#00000044, #00000088, self.drag)); // Ridge color
			let in_top = 7.0;
			sdf.rect(1.0 + in_side, top + in_top, self.rect_size.x - 2 - 2 * in_side, 3);
			sdf.fill(#FFFFFF18); // Ridge: Rim light catcher

			let nub_x = self.slide_pos * (self.rect_size.x - nub_size - in_side * 2 - 9);
			sdf.move_to(mix(in_side + 3.5, self.rect_size.x * 0.5, self.bipolar), top + in_top);

			sdf.line_to(nub_x + in_side + nub_size * 0.5, top + in_top);
			sdf.stroke_keep(mix(#FFFFFF00, self.line_color, self.drag), 1.5)
			sdf.stroke(
				mix(mix(self.line_color * 0.85, self.line_color, self.hover), #xFFFFFF80, self.drag),
				1
			)

			let nub_x = self.slide_pos * (self.rect_size.x - nub_size - in_side * 2 - 3) - 3;
			sdf.box(nub_x + in_side, top + 1.0, 12, 12, 1.)

			sdf.fill_keep(mix(mix(#x7, #x8, self.hover), #3, self.pos.y)); // Nub background gradient
			sdf.stroke(
				mix(
					mix(#xa, #xC, self.hover),
					#0,
					pow(self.pos.y, 1.5)
				),
				1.
			); // Nub outline gradient

			return sdf.result
		}
	}

    draw_text: {
        wrap: Word, // Wraps text between words

        text_style: {
            // Controls the appearance of text
            font: {path: dep("crate://self/resources/GoNotoKurrent-Bold.ttf")},
            // Font file dependency

            font_size: 12.0, // Font size of 12.0
        }

        fn get_color(self) -> vec4 {
            // Overwrite the shader's fill method
            return mix(
                mix(
                    self.color,
                    self.color_hover,
                    self.hover
                ),
                self.color_pressed,
                self.pressed
            );
        }
    },

    label_align: { x: 0.0, y: 0.5 }, // Placement of the label in its parent container

    label_walk: {
        width: Fit, height: Fit,
        margin: { left: 20.0 }
    },

    text_input: {
        cursor_margin_bottom: 2.0,
        cursor_margin_top: 2.0,
        select_pad_edges: 2.0,
        cursor_size: 2.0,
        empty_message: "0",
        numeric_only: true,
        draw_bg: { color: #00000000 },
    },

    animator: {
        hover = {
            default: off
            off = {
                from: {all: Forward {duration: 0.2}}
                ease: OutQuad
                apply: {
                    draw_slider: {hover: 0.0}
                }
            }
            on = {
                from: {all: Snap}
                apply: {
                    draw_slider: {hover: 1.0}
                }
            }
        }
        focus = {
            default: off
            off = {
                from: {all: Forward {duration: 0.0}}
                apply: {
                    draw_slider: {focus: 0.0}
                }
            }
            on = {
                from: {all: Snap}
                apply: {
                    draw_slider: {focus: 1.0}
                }
            }
        }
        drag = {
            default: off
            off = {
                from: {all: Forward {duration: 0.1}}
                apply: {draw_slider: {drag: 0.0}}
            }
            on = {
                cursor: Arrow,
                from: {all: Snap}
                apply: {draw_slider: {drag: 1.0}}
            }
        }
    }

    // LAYOUT PROPERTIES
    height: Fit,
    width: 150.0,
    margin: 5.0,
    padding: 0.0,
}

<MySlider> {}
```