## Description

The `TabBar` widget displays a horizontal row of tabs that can be used to navigate between different views or sections in an application. It supports features such as draggable tabs, closeable tabs, and scrollable tab lists when there are more tabs than can fit in the available space.

## Layouting

Support for [Walk](Walk.md) layout features which define how the widget positions itself in its parent container.

No Support for layouting child elements via the feature subset defined in [Layout](Layout.md).

## DrawShaders

### draw_drag ([DrawColor](DrawColor.md))
Determines the appearance of tabs during dragging operations.

### draw_fill ([DrawColor](DrawColor.md))
Determines the background appearance of the `TabBar`.

## Subwidgets

### scroll_bars ([ScrollBars](ScrollBars.md))
Reference to the `ScrollBars` that appear during tab overflow.

## Examples

### Typical Usage

```Rust
<TabBar> {
	scroll_bars: <ScrollBars> {}
	CloseableTab = <Tab> {closeable:true}
	PermanentTab = <Tab> {closeable:false}

	// LAYOUT PROPERTIES
	height: 30.0,
	// Element is 30.0 high.

	width: Fill,
	// Element expands to use all available horizontal space.
}
```
### Advanced
```Rust
MyTabBar = <TabBar> {
	draw_drag: { color: #6 }
	draw_fill: { color: #8 }
	scroll_bars: <ScrollBars> {}

	CloseableTab = <Tab> {closeable:true}
	PermanentTab = <Tab> {closeable:false}

	// LAYOUT PROPERTIES
	height: Fit,
	width: Fill,
	margin: 0.0
}

<MyTabBar> {}
```