The `Tab` widget allows you to organize content into multiple tabs, enabling users to switch between different views or sections within the same window.

[description]
## [Layouting](Layouting.md)
Complete layouting feature set support.
## DrawShaders
### draw_bg ([DrawQuad](DrawQuad.md))
The shader that determines the background appearance of tabs. It can be customized to change the tab's background color, shape, and other graphical properties.
### draw_icon ([DrawIcon](DrawIcon.md))
Determines the appearance of the tab's icon. Customizable with attributes supported by [DrawIcon](DrawIcon.md), including the SVG file, color, and size.
### draw_name ([DrawText](DrawText.md))
Allows styling of the tab's text with all of the attributes supported by [DrawText](DrawText.md), including colors, font, font size, and more.
## Fields
### close_button ([TabCloseButton](TabCloseButton.md))
Reference to the [TabCloseButton](TabCloseButton.md) widget, which is used to display a close button on the tab when `closeable` is set to `true`.
### closeable (bool)
Controls whether the tab is closeable. When set to `true`, a close button will appear on the tab.
### icon_walk ([Walk](Walk.md))
Controls the icon’s layout properties as supported by [Walk](Walk.md), such as margin, width, and height.
### min_drag_dist (f64 = 10.0)
The distance a tab needs to be dragged before it detaches from its parent container.
## Flags
* is_selected (bool) *(read-only)*
Indicates whether the tab is currently selected.
* is_dragging (bool) *(read-only)*
Indicates whether the tab is currently being dragged.
* hover (f32) *(read-only)*
Represents the hover state of the tab. Transitions between `0.0` (not hovered) and `1.0` (hovered).
* selected (f32) *(read-only)*
Represents the selection state of the tab. Transitions between `0.0` (not selected) and `1.0` (selected).
## States
| State          | Trigger                               |
| :------------- | :------------------------------------ |
| hover (f32)         | User moves the mouse over the element |
| selected (f32)      | Element is selected                   |
## Examples
### Basic
```Rust
<Tab> {}
```
### Typical
```Rust
<Tab> {
	close_button: <TabCloseButton> {}
	closeable: true,

	// LAYOUT PROPERTIES

	height: 25.,
	// Element is 25.0 high

	width: Fill,
	// Element assumes the width of its content.
}
```

### Advanced
```Rust
MyTab = <Tab> {
	close_button: <TabCloseButton> {}
	closeable: true,
	min_drag_dist: 10.0

	draw_bg: {
		instance hover: float
		instance selected: float

		fn pixel(self) -> vec4 {
			let sdf = Sdf2d::viewport(self.pos * self.rect_size);
			sdf.box(
				-1.,
				-1.,
				self.rect_size.x + 2,
				self.rect_size.y + 2,
				1.
			)
			sdf.fill_keep(
				mix(
					THEME_COLOR_D_2 * 0.75,
					THEME_COLOR_DOCK_TAB_ACTIVE,
					self.selected
				)
			)
			return sdf.result
		}
	}

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

	icon_walk: {
		margin: 10.,
		width: 16.,
		height: Fit
	},

	draw_name: {
		text_style: <THEME_FONT_REGULAR> {}
		instance hover: 0.0
		instance selected: 0.0
		fn get_color(self) -> vec4 {
			return mix(
				mix(
					THEME_COLOR_LABEL_INNER_INACTIVE,
					THEME_COLOR_LABEL_INNER_ACTIVE,
					self.selected
				),
				THEME_COLOR_LABEL_INNER_HOVER,
				self.hover
			)
		}
	}

	animator: {
		hover = {
			default: off
			off = {
				from: {all: Forward {duration: 0.2}}
				apply: {
					draw_bg: {hover: 0.0}
					draw_name: {hover: 0.0}
				}
			}

			on = {
				cursor: Hand,
				from: {all: Forward {duration: 0.1}}
				apply: {
					draw_bg: {hover: [{time: 0.0, value: 1.0}]}
					draw_name: {hover: [{time: 0.0, value: 1.0}]}
				}
			}
		}

		selected = {
			default: off
			off = {
				from: {all: Forward {duration: 0.3}}
				apply: {
					close_button: {draw_button: {selected: 0.0}}
					draw_bg: {selected: 0.0}
					draw_name: {selected: 0.0}
				}
			}

			on = {
				from: {all: Snap}
				apply: {
					close_button: {draw_button: {selected: 1.0}}
					draw_bg: {selected: 1.0}
					draw_name: {selected: 1.0}
				}
			}
		}
	}

	// LAYOUT PROPERTIES

	height: Fit,
	width: Fit,
	margin: { top: 0.0, left: 0.0, bottom: 0.0, right: 1.0 },
	padding: { top: 2.5, left: 0.0, bottom: 2.5, right: 0.0 },
	flow: Right,
	spacing: 5.0,
	align: { x: 0.0, y: 0.5 },
}

<MyTab> {}
```