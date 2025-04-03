The dropdown menu widget.
## [Layouting](Layouting.md)
Complete layouting feature set support.

## DropDown
### DrawShaders
#### draw_bg ([DrawQuad](DrawQuad.md))
The DrawShader that determines the apperance of the DropDown.

#### draw_text ([DrawLabelText](#drawlabeltext))
This allows styling of the button's text with all of the attributes supported by [DrawText](DrawText.md) including colors, font, font size and so on.

### Fields
#### bind (String)
The data binding key used to bind the DropDown's selected value to a data model.

#### bind_enum (String)
Specifies the enum type name when binding to an enum value.

#### labels (Vec\<String\>)
The text labels shown for the given menu options.

#### popup_menu (Option\<LivePtr\>)
Reference to the popup menu widget.

#### popup_menu_position ([PopupMenuPosition](#popupmenuposition))
Popup menus can optionally be shown below the DropDown widget or overlay it.
##### PopupMenuPosition
- **OnSelected**: The Popup overlays the DropDown so that the selected menu item is in the same spot as the Dropdown
- **BelowInput**: The Popup is shown under the DropDown

#### selected_item (usize)
The index of the selected menu item.

#### values ([Vec\<LiveValue\>](#veclivevalue))
Values of the given menu options.

## DrawLabelText
### DrawShaders
#### draw_super ([DrawText](DrawText.md))
This allows styling of the button's text with all of the attributes supported by [DrawText](DrawText.md) including colors, font, font size and so on.

### States
| State         | Trigger                                             |
| :------------ | :-------------------------------------------------- |
| focus (f32)   | Element gets focus                                  |
| hover (f32)   | User moves the mouse over the element               |
| pressed (f32) | Mouse button is pushed down during click operations |
## Examples
### Typical
```Rust
<DropDown> {
	// TODO: utilize the existing focus state
	popup_menu: <PopupMenu> {}
	bind: , // TODO: tbd
	bind_enum: , // TODO: tbd
	labels: ["Value One", "Value Two", "Thrice", "Fourth Value", "Option E", "Hexagons"], // The shown options
	values: [ValueOne, ValueTwo, Thrice, FourthValue, OptionE, Hexagons], // The values for the shown 'label' options.


	// LAYOUT PROPERTIES

	height: Fill,
	// Element expands to use all available vertical space.

	width: Fill,
	// Element expands to use all available horizontal space.
}
```

### Advanced
```Rust
MyDropDown = <DropDown> {
	// TODO: utilize the existing focus state
	popup_menu: <PopupMenu> {}

	bind: , // TODO: tbd
	bind_enum: , // TODO: tbd
	labels: ["Value One", "Value Two", "Thrice", "Fourth Value", "Option E", "Hexagons"], // The shown options
	selected_item: 0, // The selection option
	values: [ValueOne, ValueTwo, Thrice, FourthValue, OptionE, Hexagons], // The values for the shown 'label' options.

	popup_menu: {
		menu_item: {
			width: Fill, height: Fit,
			padding: 10.0,
			indent_width: 10.0
			
			draw_bg: {
				color: #x48,
				color_selected: #x6
			}
		}
	}
	popup_menu_position: BelowInput, // The popup is shown under the control.

	draw_bg: {
		instance hover: 0.0,
		instance focus: 0.0,
		instance pressed: 0.0,
		instance open: 0.0,
		
		uniform border_radius: 5.0,
		uniform color: #888,
		uniform color_pressed: #666,
		uniform color_focussed: #777,

		uniform color_bevel_light: #CCC,
		instance color_bevel_shadow: #333,
		instance bevel: 0.75,

		uniform triangle_color: #DDD,
		uniform triangle_color_hover: #FFF,

		fn pixel(self) -> vec4 {
			let sdf = Sdf2d::viewport(self.pos * self.rect_size);
			let grad_top = 5.0;
			let grad_bot = 1.0;
			let body = mix(mix(self.color, self.color_pressed, self.hover), self.color_focussed, self.focus);
			let body_transp = vec4(body.xyz, 0.0);

			let top_gradient = mix(
				body_transp,
				mix(
					mix(
						#FFFFFF00,
						self.color_bevel_light,
						self.hover
					),
					self.color_bevel_light,
					self.focus
				),
				max(0.0, grad_top - sdf.pos.y) / grad_top);

			let bot_gradient = mix(
				mix(body_transp, self.color_bevel_shadow, self.pressed),
				top_gradient,
				clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
			);

			// the little drop shadow at the bottom
			let shift_inward = self.border_radius * 1.75;
			sdf.move_to(shift_inward, self.rect_size.y);
			sdf.line_to(self.rect_size.x - shift_inward, self.rect_size.y);
			sdf.stroke(mix(
				mix(
					#00000000,
					self.color_bevel_shadow,
					self.hover
				),
				self.color_bevel_shadow,
				self.focus
				), self.bevel
			)

			sdf.box(
				1.,
				1.,
				self.rect_size.x - 2.0,
				self.rect_size.y - 2.0,
				self.border_radius
			)
			sdf.fill_keep(body)

			sdf.stroke(
				bot_gradient,
				self.bevel * 1.5
			)

			// lets draw a little triangle in the corner
			let c = vec2(self.rect_size.x - 10.0, self.rect_size.y * 0.5)
			let sz = 3.;
			let offset = 1.;

			sdf.move_to(c.x - sz, c.y - sz + offset);
			sdf.line_to(c.x + sz, c.y - sz + offset);
			sdf.line_to(c.x, c.y + sz * 0.25 + offset);
			sdf.close_path();

			sdf.fill(mix(self.triangle_color, self.triangle_color_hover, self.hover));

			return sdf.result
		}
	}

	draw_text: {
		uniform color: #000
		uniform color_pressed: #333

		text_style: {
		// Controls the appearance of text.
			font: {path: dep("crate://self/resources/GoNotoKurrent-Bold.ttf")},
			// Font file dependency.

			font_size: 12.0, // Font-size of 12.0.
		}

		fn get_color(self) -> vec4 {
			return mix(
				self.color,
				self.color_pressed,
				self.pressed
			)
		}
	}

	animator: {
		hover = {
			default: off,
			off = {
				from: {all: Forward {duration: 0.1}}
				apply: {
					draw_bg: {pressed: 0.0, hover: 0.0}
					draw_text: {pressed: 0.0, hover: 0.0}
				}
			}

			on = {
				from: {
					all: Forward {duration: 0.1}
					pressed: Forward {duration: 0.01}
				}
				apply: {
					draw_bg: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
					draw_text: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
				}
			}

			pressed = {
				from: {all: Forward {duration: 0.2}}
				apply: {
					draw_bg: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
					draw_text: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
				}
			}
		}
		focus = {
			default: off
			off = {
				from: {all: Forward {duration: 0.2}}
				apply: {
					draw_bg: {focus: 0.0},
					draw_text: {focus: 0.0}
				}
			}
			on = {
				from: {all: Snap}
				apply: {
					draw_bg: {focus: 1.0},
					draw_text: {focus: 1.0}
				}
			}
		}
	}


	// LAYOUT PROPERTIES
	height: Fit,
	width: Fit,
	margin: 5.0,
	padding: 2.5,
	flow: Down,
	spacing: 1.25,
	align: { x: 0.0, y: 0.0 },
	line_spacing: 1.5
}

<MyDropDown> {
	labels: ["One", "Two", "Three", "Fourth", "Five", "Six"], // The shown options
	values: [One, Two, Three, Four, Five, Six], // The values for the shown 'label' options.
}
```