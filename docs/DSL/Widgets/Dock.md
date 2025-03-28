The `Dock` widget provides a dockable layout that supports tabs and drag-and-drop docking. It allows users to organize content in a flexible and customizable interface.

It is the main component for creating dockable layouts. It manages tabs, splitters, and the drag-and-drop behavior necessary for docking panels.

## DrawShaders

### drag_quad ([DrawColor](../FoundationalTypes/DrawQuad.md))

The `drag_quad` `DrawShader` determines the appearance of the dock while it is being dragged.

### round_corner ([DrawRoundCorner](#drawroundcorner))

An overlay `DrawShader` that allows for rounded corners on dock panels.

### padding_fill ([DrawColor](../FoundationalTypes/DrawQuad.md))

A `DrawShader` that determines the background appearance of the dock.

## Fields

### border_size (f64)

The width of the dock's outline.

### tab_bar (Option\<LivePtr\>)

Reference to the `TabBar` widget to be used within the dock.

### splitter (Option\<LivePtr\>)

Reference to the `Splitter` widget used for dividing the dock area.

## DrawRoundCorner

The `DrawRoundCorner` `DrawShader` is responsible for drawing the rounded corners on dock panels.

### Fields

#### border_radius (f32)

The radius used for rounding the corners.

#### flip (Vec2)

A vector used to mirror the `DrawShader`'s rounded corner to populate the rectangle's remaining three corners with correctly oriented copies.

## Examples

### Typical / Advanced

```rust
MyDock = <Dock> {
	round_corner: {
		// TODO: tbd
	}
	padding_fill: { color: #f00 }
	border_size: 3.0
	tab_bar: <TabBar> {}
	splitter: <Splitter> {}

	drag_quad: {
		color: #00f
	}

	root = Splitter {
		axis: Horizontal,
		align: FromA(300.0),
		a: tab_set_1,
		b: tab_set_2
	}

	tab_set_1 = Tabs {
		tabs: [tab_a, tab_b],
		selected: 1
	}

	tab_set_2 = Tabs {
		tabs: [tab_c, tab_d],
		selected: 1
	}

	tab_a = Tab {
		name: "Tab A"
		template: PermanentTab,
		kind: Container_A
	}

	tab_b = Tab {
		name: "Tab B"
		template: PermanentTab,
		kind: Container_B
	}

	tab_c = Tab {
		name: "Tab C"
		template: CloseableTab,
		kind: Container_C
	}

	tab_d = Tab {
		name: "Tab D"
		template: CloseableTab,
		kind: Container_D
	}

	Container_A = <RectView> {
		height: Fill, width: Fill
		padding: 10.,
		<Label> {text: "Hello"}
	}

	Container_B = <RectView> {
		height: Fill, width: Fill
		padding: 10.,
		<Label> {text: "Aloah"}
	}

	Container_C = <RectView> {
		height: Fill, width: Fill
		padding: 10.,
		<Label> {text: "Ahoy"}
	}

	Container_D = <RectView> {
		height: Fill, width: Fill
		padding: 10.,
		<Label> {text: "Hi"}
	}

	height: 500.,
	width: Fill
}

<MyDock> {}
```
