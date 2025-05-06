A widget for displaying and managing a list of items. It supports scrolling, flick scrolling, and drag scrolling.
## [Layouting](Layouting.md)
Complete layouting feature set support.

## Fields
### align_top_when_empty (bool = true)
If true content will be displayed at the top when the widget is not fully filled.

### capture_overload (bool = false)
Allows for multiple events to occur simultaneously. For instance, touch scrolling will still work while buttons are actively used by the user.

### drag_scrolling (bool = true)
If true dragging the content area scrolls it, which is mostly useful for mobile interfaces.

### grab_key_focus (bool = false)
Determines whether the list should capture the keyboard focus when it is interacted with.

### max_pull_down (f64 = 100.0)
The maximum distance one can pull down the list from the top before it bounces back.

## Subwidgets
### scroll_bars ([ScrollBars](ScrollBars.md))
References to the `ScrollBars` widget. To display scroll bars, one needs to reference them here. This allows for referencing specifically styled and configured `ScrollBars`.

## Examples
### Basic
```Rust
<FlatList> {
	scroll_bars: <ScrollBars> {}

	Target = <BuildItem> {
		padding: 0,
		check = <RunButton> { margin: {left: 23} }
	}

	Binary = <BuildItem> {
		flow: Right

		fold = <FoldButton> {
			height: 25, width: 15,
			margin: { left: 8.0 }
			animator: { open = { default: off } },
		}
		check = <RunButton> {}
	}

	Empty = <BuildItem> {
		height: Fit, width: Fill,
		cursor: Default
	}

	// LAYOUT PROPERTIES

	height: 500.0,
	// Element is 500. high.

	width: Fill,
	// Element expands to use all available horizontal space.
}
```

### Typical
```Rust
<FlatList> {
	align_top_when_empty: true,
	capture_overload: true,
	drag_scrolling: true,
	flick_scroll_decay: 0.98,
	flick_scroll_maximum: 80.0,
	flick_scroll_minimum: 0.2,
	flick_scroll_scaling: 0.005,
	grab_key_focus: false,
	max_pull_down: 100.0,
	swipe_drag_duration: 0.2,

	scroll_bars: <ScrollBars> {}

	Target = <BuildItem> {
		padding: 0,
		check = <RunButton> { margin: {left: 23} }
	}

	Binary = <BuildItem> {
		flow: Right

		fold = <FoldButton> {
			height: 25, width: 15,
			margin: { left: 8.0 }
			animator: { open = { default: off } },
		}
		check = <RunButton> {}
	}

	Empty = <BuildItem> {
		height: Fit, width: Fill,
		cursor: Default
	}

	// LAYOUT PROPERTIES

	height: 500.0,
	// Element is 500. high.

	width: Fill,
	// Element expands to use all available horizontal space.

}
```

### Advanced
```Rust
MyFlatList = <FlatList> {
	align_top_when_empty: true,
	capture_overload: true,
	drag_scrolling: true,
	flick_scroll_decay: 0.98,
	flick_scroll_maximum: 80.0,
	flick_scroll_minimum: 0.2,
	flick_scroll_scaling: 0.005,
	grab_key_focus: false,
	max_pull_down: 100.0,
	swipe_drag_duration: 0.2,

	scroll_bars: <ScrollBars> {}

	Target = <BuildItem> {
		padding: 0,
		check = <RunButton> { margin: {left: 23} }
	}

	Binary = <BuildItem> {
		flow: Right

		fold = <FoldButton> {
			height: 25, width: 15,
			margin: { left: 8.0 }
			animator: { open = { default: off } },

			draw_bg: {
				uniform size: 3.75;
				instance open: 0.0
				
				fn pixel(self) -> vec4 {
					let sdf = Sdf2d::viewport(self.pos * self.rect_size)
					let left = 2;
					let sz = self.size;
					let c = vec2(left + sz, self.rect_size.y * 0.5);
					
					// PLUS
					sdf.box(0.5, sz * 3.0, sz * 2.5, sz * 0.7, 1.0); // rounding = 3rd value
					// vertical
					sdf.fill_keep(mix((#6), #8, self.hover));
					sdf.box(sz * 1.0, sz * 2.125, sz * 0.7, sz * 2.5, 1.0); // rounding = 3rd value

					sdf.fill_keep(mix(mix((#6), #8, self.hover), #fff0, self.open))

					return sdf.result
				}
			}
		}
		check = <RunButton> {}
	}

	Empty = <BuildItem> {
		height: Fit, width: Fill,
		cursor: Default
	}

	// LAYOUT PROPERTIES
	height: Fill,
	width: 250.,
	margin: 5.0,
	padding: 5.0,
	flow: Down,
	spacing: 2.5,
	align: { x: 0.0, y: 0.0 },
}

<MyFlatList> {}
```
