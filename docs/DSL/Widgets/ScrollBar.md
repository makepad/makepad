The `ScrollBar` widget allows users to navigate through content that overflows the visible area, either horizontally or vertically.

## Layouting
No layouting support.

## Structure
The `ScrollBar` consists of a track and a handle. The track represents the total scrollable area, while the handle represents the visible portion of the content.

## Fields

### axis  ([ScrollAxis](#scrollaxis) = `ScrollAxis::Horizontal`)
Selects the scrolling direction.

#### ScrollAxis
- **Horizontal** - Scrolls content horizontally.
- **Vertical** - Scrolls content vertically.

### bar_side_margin (f64)
The amount of margin between the `ScrollBar` and the outer container.

### bar_size (f64)
A scaling factor for the size of the `ScrollBar`.

### min_handle_size (f64)
A minimum handle size to ensure usability when there's an excessive amount of content to scroll.

### smoothing (Option<f64>)
Controls the smoothness of scrolling motions. If `Some(value)`, scrolling is animated over time. If `None`, scrolling is immediate.

### use_vertical_finger_scroll (bool)
Determines whether vertical finger scrolling is enabled when the `ScrollBar` is horizontal.

## DrawShaders

### draw_bar ([DrawScrollBar](#drawscrollbar))
Reference to the `DrawShader` that determines the appearance of the `ScrollBar`.

## DrawScrollBar
 `DrawScrollBar` is responsible for rendering the visual elements of the `ScrollBar`.

### Flags

* is_vertical (f32)
Indicates the scrolling orientation. `1.0` for vertical, `0.0` for horizontal.

### Fields

#### norm_handle (f32)
The normalized size of the handle.

#### norm_scroll (f32)
The normalized scroll position.

### DrawShaders

#### draw_super ([DrawQuad](DrawQuad.md))
The base `DrawShader` for the `ScrollBar`.

## States

| State    | Trigger                                             |
| :------- | :-------------------------------------------------- |
| `hover`  | User moves the mouse over the element               |
| `pressed`| Mouse button is pushed down during click operations |

## Examples
### Basic
```Rust
<ScrollBar> {}
```

### Typical
```Rust
<ScrollBar> {
		bar_size: 10.0,
		bar_side_margin: 3.0
		min_handle_size: 30.0
		axis: Vertical
		smoothing: 10.0
		use_vertical_finger_scroll: false
}
```

### Advanced 
```Rust
MyScrollBar = <ScrollBar> {
		bar_size: 10.0,
		bar_side_margin: 3.0
		min_handle_size: 30.0
		axis: Vertical
		smoothing: 10.0
		use_vertical_finger_scroll: false

		draw_bar: {
			instance pressed: 0.0
			instance hover: 0.0
			
			instance color: #888,
			instance color_hover: #999
			instance color_pressed: #666
			
			uniform size: 6.0
			uniform border_radius: 1.5

			fn pixel(self) -> vec4 {
				let sdf = Sdf2d::viewport(self.pos * self.rect_size);
				if self.is_vertical > 0.5 {
					sdf.box(
						1.,
						self.rect_size.y * self.norm_scroll,
						self.size,
						self.rect_size.y * self.norm_handle,
						self.border_radius
					);
				}
				else {
					sdf.box(
						self.rect_size.x * self.norm_scroll,
						1.,
						self.rect_size.x * self.norm_handle,
						self.size,
						self.border_radius
					);
				}
				return sdf.fill(mix(
					self.color, 
					mix(
						self.color_hover,
						self.color_pressed,
						self.pressed
					),
					self.hover
				));
			}
		}

		animator: {
			hover = {
				default: off
				off = {
					from: {all: Forward {duration: 0.1}}
					apply: {
						draw_bar: {pressed: 0.0, hover: 0.0}
					}
				}

				on = {
					cursor: Default,
					from: {
						all: Forward {duration: 0.1}
						pressed: Forward {duration: 0.01}
					}
					apply: {
						draw_bar: {
							pressed: 0.0,
							hover: [{time: 0.0, value: 1.0}],
						}
					}
				}

				pressed = {
					cursor: Default,
					from: {all: Snap}
					apply: {
						draw_bar: {
							pressed: 1.0,
							hover: 1.0,
						}
					}
				}
			}
		}

}

<MyScrollBar> {}
```