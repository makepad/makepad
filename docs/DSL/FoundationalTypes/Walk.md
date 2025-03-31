Makepad’s foundational layout properties for the inner properties of elements.
## abs_pos (`Option<DVec2>`)
An optional absolute position for the element. When set, the element is positioned at the specified coordinates, bypassing the normal layout flow. Use sparingly, as it can disrupt responsive layouts.

```rust
// Positions the element at (100.0, 50.0)
abs_pos: Some(dvec2(100.0, 50.0))
```
## height ([Size](Size.md))
Determines the height of elements.

**Options**
- `Size::Fixed(f64)` - An absolute height specified in pixels.
- `Size::Fill` - Expands the element to fill the remaining height of its parent container.
- `Size::Fit` - Shrinks the element to fit the height of its child elements.
- `Size::All` - Spans the element to the full height, potentially beyond its parent.

```rust
height: Size::Fixed(200.0) // Sets the height to 200 pixels.
height: Size::Fill         // Fills the remaining height of the parent.
height: Size::Fit          // Fits the height to its content.
height: Size::All          // Spans the full height.
```
## margin (Margin)
Sets the margin area on all four sides of an element.

```rust
// A margin of 10.0 on all four sides.
margin: Margin { left: 10.0, right: 10.0, top: 10.0, bottom: 10.0 }

// A different margin for every side.
margin: Margin {
    top: 0.0,
    right: 5.0,
    bottom: 2.5,
    left: 5.0,
}
```
## width ([Size](Size.md))
Determines the width of elements.

**Options**
- `Size::Fixed(f64)` - An absolute width specified in pixels.
- `Size::Fill` - Expands the element to fill the remaining width of its parent container.
- `Size::Fit` - Shrinks the element to fit the width of its child elements.
- `Size::All` - Spans the element to the full width, potentially beyond its parent.

```rust
width: Size::Fixed(200.0) // Sets the width to 200 pixels.
width: Size::Fill         // Fills the remaining width of the parent.
width: Size::Fit          // Fits the width to its content.
width: Size::All          // Spans the full width.
```
## Example
```rust
<View> {
	flow: Flow::Down, // Arranges child elements vertically.
	width: Size::Fill, // Fills the width of the parent.
	height: Size::Fit, // Fits the height to its content.
	margin: Margin {
		top: 10.0,
		right: 5.0,
		bottom: 10.0,
		left: 5.0,
	},
	// Child elements go here.
}
```