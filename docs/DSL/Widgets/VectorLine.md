A widget that displays a customizable line within its parent container. The line's appearance and position can be adjusted using various fields.

## Layouting

Support for [Walk](Walk.md) layout features which define how the widget positions itself in its parent container.

No support for layouting child elements via the feature subset defined in [Layout](Layout.md).

## VectorLine

### DrawShaders

#### draw_ls (DrawLine)

The `DrawShader` that determines the visual rendering of the `VectorLine` widget.

### Fields

#### color (Vec4)

The color of the line.

#### contained (bool = true)

Determines whether the line is contained within the bounds of its parent container. If `true`, the line is clipped to the parent's dimensions; if `false`, the line may extend beyond the parent's bounds.

#### line_align ([LineAlign](#linealign) = LineAlign::Top)

Controls the line's alignment and orientation within its parent container.

##### LineAlign

- **Top**: Line is positioned at the top.
- **Bottom**: Line is positioned at the bottom.
- **Left**: Line is positioned on the left.
- **Right**: Line is positioned on the right.
- **HorizontalCenter**: Line is horizontally centered within the parent.
- **VerticalCenter**: Line is vertically centered within the parent.
- **DiagonalTopLeftBottomRight**: Line goes from the top-left corner to the bottom-right corner.
- **DiagonalBottomLeftTopRight**: Line goes from the bottom-left corner to the top-right corner.
- **Free**: Line position is determined freely (requires manual specification of start and end points).

#### line_width (f64 = 15.0)

The width of the line in pixels.

## Examples

### Typical
```Rust
<VectorLine> {
	color: #ff0,
	line_align: Top,
	line_width: 5.0,

	// LAYOUT PROPERTIES

	height: 2.0,
	// Element is 15. high.

	width: 100.0,
	// Element expands to use all available horizontal space.

	margin: { top: 10.0, right: 5.0, bottom: 10.0, left: 5.0 },
	// Individual margins outside the element for all four directions.
}
```
### Advanced
```Rust
MyVectorLine = <VectorLine> {
	color: #ff0,
	contained: true,
	line_align: Top,
	line_width: 20.0,

	draw_ls: {
		// TODO: tbd
	}

	// LAYOUT PROPERTIES
	height: 3.0,
	width: Fill,
	margin: { top: 10.0, right: 0.0, bottom: 10.0, left: 0.0 },
}

<MyVectorLine> {}
```