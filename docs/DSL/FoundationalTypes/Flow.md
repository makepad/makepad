# Flow

Controls the layout of child elements within a container by specifying the direction in which child elements are placed and how they behave when the container's dimensions are constrained.

## Flow Types

### Down

A vertical layout where children are placed one below the other from top to bottom.

#### Description

In `Down` flow, each child element is positioned below the previous one along the y-axis. The width of each child can be controlled separately, and the container's height grows to accommodate its children unless specified otherwise.

#### Example

```rust
<View> {
    width: Fill,
    height: Fill,
    flow: Down,
    spacing: 5.0,
    <Label> { text: "First Item" }
    <Label> { text: "Second Item" }
    <Label> { text: "Third Item" }
}
```

In this example, three `Label` elements are stacked vertically with a spacing of 5 units between them.

### Right

A horizontal layout where children are placed next to each other from left to right.

#### Description

In `Right` flow, child elements are positioned side by side along the x-axis. The height of each child can be controlled independently, and the container's width increases to accommodate the children unless specified otherwise.

#### Example

```rust
<View> {
    width: Fill,
    height: Fixed(50.0),
    flow: Right,
    spacing: 10.0,
    <Button> { label: "Option A" }
    <Button> { label: "Option B" }
    <Button> { label: "Option C" }
}
```

This will display three buttons in a horizontal row with 10 units of spacing between them.

### Overlay

A layout where children are stacked on top of each other, overlaying in the z-order from back to front, meaning later elements are drawn over earlier ones.

#### Description

In `Overlay` flow, all child elements occupy the same position in the container, effectively superimposing them. This is useful for creating composite views where elements need to overlap.

#### Example

```rust
<View> {
    width: Fixed(200.0),
    height: Fixed(200.0),
    flow: Overlay,
    <Image> { source: "background.png" }
    <Icon> { name: "icon.png", position: vec2(50.0, 50.0) }
    <Label> { text: "Overlay Text", align: center }
}
```

In this example, an `Image` is used as a background, an `Icon` is placed at a specific position, and a `Label` is overlaid on top, all within the same container.

### RightWrap

A horizontal layout where children are placed next to each other from left to right, wrapping to the next line when the maximum width is reached.

#### Description

In `RightWrap` flow, child elements are arranged horizontally until they exceed the container's maximum width, at which point they wrap to a new line starting from the left. This flow type is useful for layouts that need to adapt to varying widths, like responsive grids.

**Note:** `Flow::RightWrap` does not support child elements with `width: Fill`; child elements should have fixed or fit widths.

#### Example

```rust
<View> {
    width: Fixed(300.0),
    height: Fill,
    flow: RightWrap,
    spacing: 10.0,
    line_spacing: 15.0,
    <Button> { label: "Button 1", width: Fixed(100.0) }
    <Button> { label: "Button 2", width: Fixed(100.0) }
    <Button> { label: "Button 3", width: Fixed(100.0) }
    <Button> { label: "Button 4", width: Fixed(100.0) }
}
```

In this example, buttons are arranged in a horizontal row until they exceed the container's width of 300 units, after which they wrap to the next line.

## Additional Notes

- The `flow` property is defined in the `Layout` struct and determines the primary direction of the layout.
- Spacing between child elements can be adjusted using the `spacing` property for horizontal spacing and `line_spacing` for vertical spacing in wrapped layouts.
- `Flow::RightWrap` currently does not support child elements with `width: Fill`. Ensure child elements have a fixed or fit width when using this flow type.