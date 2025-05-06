The `PortalList` widget efficiently handles large lists of items by only rendering the items currently visible in the viewport. It supports features like scrolling, flick scrolling, and alignment of items within the list. This is especially useful for implementing lists with a large number of items without compromising performance.

## [Layouting](Layouting.md)

Complete layouting feature set support.

## Fields

### align_top_when_empty (bool = true)

If `true`, content will be displayed at the top when the widget is not fully filled. This means that if there are not enough items to fill the viewport, they will be aligned to the top.

### auto_tail (bool = false)

If `true` and the list is scrolled to the bottom, any new items added to the list will cause the scroll position to stick to the last item. This is useful for applications like chat windows or logs where new items are continuously appended.

### capture_overload (bool = false)

If `true`, the widget will capture mouse events even if they are already captured by another widget. This ensures that scroll events are handled by the `PortalList` even when other interactive elements are present.

### drag_scrolling (bool = true)

If `true`, dragging the content area scrolls it. This is especially useful for touch interfaces and mobile devices where users expect to scroll by dragging.

### grab_key_focus (bool = false)

Determines whether the widget should capture the keyboard focus when it is interacted with. If `true`, interacting with the `PortalList` will grant it keyboard focus.

### keep_invisible (bool = false)

If `true`, items that are scrolled out of view are kept in memory instead of being removed. This can improve performance if recreating items is expensive, at the cost of higher memory usage.

### max_pull_down (f64 = 100.0)

Defines the maximum distance the content can be pulled down beyond the top of the list when overscrolling. It creates a "pull-to-refresh" or bounce-back effect when users scroll past the start of the list.

## Subwidget

### scroll_bar ([ScrollBar](ScrollBar.md))

Reference to the `ScrollBar` subwidget used for scrolling within the `PortalList`.

## States

| State         | Trigger                                             |
|---------------|-----------------------------------------------------|
| hover (f32)   | User moves the mouse over the element               |
| pressed (f32) | Mouse button is pushed down during click operations |

## Examples

### Typical

```Rust
MyPortalList = <PortalList> {
	Location = <LogItem> {
		icon = <LogIcon> {},
		binary = <Label> {draw_text: {color: #5}, width: Fit, margin: {right: 4, top:0, bottom:0}, padding: 0, draw_text: {wrap: Word}}
		location = <LinkLabel> {padding:0, margin: 0, text: ""}
		body = <P> {width: Fill, margin: {left: 5, top:0, bottom:0}, padding: 0, draw_text: {wrap: Word}}
	}

	Bare = <LogItem> {
		icon = <LogIcon> {},
		binary = <P> { margin: 0, draw_text: {color: #888888 }, width: Fit }
		body = <P> {  margin: 0 }
	}

	Empty = <LogItem> {
		cursor: Default
		width: Fill
		height: 25,
		body = <P> {  margin: 0, text: "" }
	}

	// LAYOUT PROPERTIES

	width: Fill,
	// Element expands to use all available vertical space.

	width: Fill,
	// Element expands to use all available horizontal space.
}
```

### Advanced
```Rust
MyPortalList = <PortalList> {
	align_top_when_empty: true,
	auto_tail: false,
	capture_overload: true,
	drag_scrolling: true,
	grab_key_focus: false,
	keep_invisible: false,
	max_pull_down: 100.0,

	Location = <LogItem> {
		icon = <LogIcon> {},
		binary = <Label> {draw_text: {color: #5}, width: Fit, margin: {right: 4, top:0, bottom:0}, padding: 0, draw_text: {wrap: Word}}
		location = <LinkLabel> {padding:0, margin: 0, text: ""}
		body = <P> {width: Fill, margin: {left: 5, top:0, bottom:0}, padding: 0, draw_text: {wrap: Word}}
	}

	Bare = <LogItem> {
		icon = <LogIcon> {},
		binary = <P> { margin: 0, draw_text: {color: #888888 }, width: Fit }
		body = <P> {  margin: 0 }
	}

	Empty = <LogItem> {
		cursor: Default
		width: Fill
		height: 25,
		body = <P> {  margin: 0, text: "" }
	}

	// LAYOUT PROPERTIES

	height: Fill,
	width: Fill,
	margin: 5.0
	padding: 5.0
	flow: Down,
	spacing: 5.0,
	align: { x: 0.0, y: 0.0 },
	line_spacing: 1.5,
	scroll: vec2(0.0, 300.0),
}

<MyPortalList> {}
```
