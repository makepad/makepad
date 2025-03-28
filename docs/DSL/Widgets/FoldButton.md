The `FoldButton` widget is used to indicate and control expandable sections within the UI. It displays a button that can toggle between open and closed states, often represented with an arrow or triangle that rotates based on its state.

## Layouting

Support for [Walk](Walk.md) layout features which define how the widget positions itself in its parent container.

No support for layouting child elements via the feature subset defined in [Layout](Layout.md).

## Fields
### abs_size (`DVec2`)
The absolute size of the `FoldButton`.

### abs_offset (`DVec2`)
The absolute offset position of the `FoldButton` from its parent.


## DrawShaders

### `draw_bg` ([DrawQuad](DrawQuad.md))

Determines the appearance of the `FoldButton`, including visual representation and animations for different states like hover and open.

## States

| State         | Trigger                                        |
|---------------|------------------------------------------------|
| `hover` (f32) | When the user moves the mouse over the element |
| `open` (f32)  | When the button is clicked to open or close a section |


## Examples
### Basic
```rust
<FoldButton> {}
```
### Typical
```rust
<FoldButton> {
	// LAYOUT PROPERTIES

	height: 15.0,
	// Element is 15. high.

	width: 15.0,
	// Element expands to use all available horizontal space.
}
```

### Advanced
```Rust
MyFoldButton = <FoldButton> {
	draw_bg: {
		instance open: 0.0
		instance hover: 0.0
		uniform fade: 1.0

		fn pixel(self) -> vec4 {
			let sz = 2.5;
			let c = vec2(5.0, 0.6 * self.rect_size.y);
			let sdf = Sdf2d::viewport(self.pos * self.rect_size);
			sdf.clear(vec4(0.));

			// we have 3 points, and need to rotate around its center
			sdf.rotate(self.open * 0.5 * PI + 0.5 * PI, c.x, c.y);
			sdf.move_to(c.x - sz, c.y + sz);
			sdf.line_to(c.x, c.y - sz);
			sdf.line_to(c.x + sz, c.y + sz);
			sdf.close_path();
			sdf.fill(mix(
				#888,
				#AAA,
				self.hover
			)
			);
			return sdf.result * self.fade;
		}
	}

	animator: {
		hover = {
			default: off
			off = {
				from: {all: Forward {duration: 0.1}}
				apply: {draw_bg: {hover: 0.0}}
			}

			on = {
				from: {all: Snap}
				apply: {draw_bg: {hover: 1.0}}
			}
		}

		open = {
			default: on
			off = {
				from: {all: Forward {duration: 0.2}}
				ease: ExpDecay {d1: 0.96, d2: 0.97}
				redraw: true
				apply: {
					draw_bg: {open: [{time: 0.0, value: 1.0}, {time: 1.0, value: 0.0}]}
				}
			}
			on = {
				from: {all: Forward {duration: 0.2}}
				ease: ExpDecay {d1: 0.98, d2: 0.95}
				redraw: true
				apply: {
					draw_bg: {open: [{time: 0.0, value: 0.0}, {time: 1.0, value: 1.0}]}
				}
			}
		}
	}

	// LAYOUT PROPERTIES

	height: Fit,
	width: Fit,
	margin: { top: 10.0, right: 5.0, bottom: 10.0, left: 5.0 },
}

<MyFoldButton> {}
```

