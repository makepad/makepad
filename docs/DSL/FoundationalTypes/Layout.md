# Layout

The `Layout` struct controls the layout properties of elements, such as positioning, alignment, spacing, padding, and scrolling.

## Fields

### align ([Align](Align.md))

Specifies how child elements are aligned within the parent container along the horizontal (`x`) and vertical (`y`) axes.

- `x` (f64 = 0.0): Horizontal alignment. A value between `0.0` (left) and `1.0` (right).
- `y` (f64 = 0.0): Vertical alignment. A value between `0.0` (top) and `1.0` (bottom).

### clip_x (bool = true)

Enables or disables horizontal clipping. When `true`, content that extends beyond the container's horizontal bounds is clipped.

### clip_y (bool = true)

Enables or disables vertical clipping. When `true`, content that extends beyond the container's vertical bounds is clipped.

### flow ([Flow](Flow.md) = `Flow::Right`)

Determines the layout direction of child elements within the container.

Possible values:

- `Flow::Right`: Arranges child elements horizontally from left to right.
- `Flow::Down`: Arranges child elements vertically from top to bottom.
- `Flow::Overlay`: Stacks child elements on top of each other.
- `Flow::RightWrap`: Arranges child elements horizontally, wrapping to the next line when the right edge is reached.

### line_spacing (f64 = 0.0)

Specifies the spacing between lines when wrapping content or arranging elements in a vertical flow.

### padding ([Padding](Padding.md))

Sets the internal padding of the container, controlling the distance between the container's borders and its content.

#### Padding Fields

- `left` (f64 = 0.0): Padding on the left side.
- `top` (f64 = 0.0): Padding on the top side.
- `right` (f64 = 0.0): Padding on the right side.
- `bottom` (f64 = 0.0): Padding on the bottom side.

### scroll (DVec2 = vec2(0.0, 0.0))

Sets the scroll offset of the content within the container.

### spacing (f64 = 0.0)

Specifies the spacing between child elements within the container.

### Size

Defines how elements consume space within their parent container.

#### Size Variants

- `Size::Fixed(f64)`: Sets the element to a fixed size.
- `Size::Fill`: The element fills the remaining available space in the parent container, accounting for padding and margins.
- `Size::Fit`: The element sizes itself to fit its content.
- `Size::All`: The element takes up all available space in the parent container, ignoring any padding and margins.

## Example

```rust
<View> {
    // Set padding on all sides
    padding: { left: 10.0, right: 10.0, top: 10.0, bottom: 10.0 },
    // Arrange child elements vertically
    flow: Flow::Down,
    // Set spacing between child elements
    spacing: 10.0,
    // Align child elements to the left and top
    align: { x: 0.0, y: 0.0 },
    // Set line spacing between wrapped lines
    line_spacing: 1.5,
    // Scroll offset
    scroll: vec2(0.0, 300.0),
    // Child elements
    <Button> { text: "Button 1" },
    <Button> { text: "Button 2" }
}
```