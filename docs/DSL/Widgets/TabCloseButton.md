The `TabCloseButton` widget provides a button used to close tabs in the UI. It displays an "X" icon that changes appearance when hovered over, offering visual feedback to the user.

## Layouting

Support for [Walk](Walk.md) layout features which define how the widget positions itself in its parent container.

No Support for layouting child elements via the feature subset defined in [Layout](Layout.md).

## DrawShaders

### `draw_button` ([DrawQuad](DrawQuad.md))

This `DrawShader` determines the appearance of the `TabCloseButton`. It draws the "X" icon and handles hover effects.

## States

| State     | Trigger                               |
| :-------- | :------------------------------------ |
| `hover` (f32) | User moves the mouse over the element |

## Examples
### Basic
```Rust
<TabCloseButton> {}
```
### Typical
```Rust
<TabCloseButton> {
	// LAYOUT PROPERTIES

	height: 15.0,
	// Element is 15. high.

	width: 15.,
	// Element is 15. wide.
}
```
### Advanced
```Rust
MyTabCloseButton = <TabCloseButton> {
	draw_button: {
		instance hover: float;
		instance selected: float;

		fn pixel(self) -> vec4 {
			let sdf = Sdf2d::viewport(self.pos * self.rect_size);
			let mid = self.rect_size / 2.0;
			let size = (self.hover * 0.25 + 0.5) * 0.25 * length(self.rect_size);
			let min = mid - vec2(size);
			let max = mid + vec2(size);
			sdf.move_to(min.x, min.y);
			sdf.line_to(max.x, max.y);
			sdf.move_to(min.x, max.y);
			sdf.line_to(max.x, min.y);
			return sdf.stroke(mix(
				#A00,
				#F00,
				self.hover
			), 1.0);
		}
	}

	animator: {
		hover = {
			default: off
			off = {
				from: {all: Forward {duration: 0.1}}
				apply: {
					draw_button: {hover: 0.0}
				}
			}

			on = {
				cursor: Hand,
				from: {all: Snap}
				apply: {
					draw_button: {hover: 1.0}
				}
			}
		}
	}


	// LAYOUT PROPERTIES
	height: 5.0,
	width: 5.0,
	margin: 5.0
}

<MyTabCloseButton> {}
```