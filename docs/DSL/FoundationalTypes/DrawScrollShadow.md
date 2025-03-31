`DrawScrollShadow` is a graphical component in the Makepad Rust UI framework used to render shadow effects along the edges of scrollable areas. It enhances visual feedback by drawing shadows at the boundaries of scrollable containers, simulating depth and indicating scrollable content. This is particularly useful for highlighting content boundaries when scrolling horizontally or vertically.

## Attributes

### [DrawQuad](DrawQuad.md)
Inherits from `DrawQuad`, leveraging its core rendering functionality for drawing 2D quadrilaterals, including vertex transformations, clipping, and instance-based rendering.

### `shadow_size` (f32)
Specifies the thickness of the shadow in logical pixels. It controls how prominent the shadow appears at the edges of the scrollable content.

### `shadow_is_top` (f32)
A flag that indicates the orientation of the shadow. A value of `0.0` means the shadow is drawn on the left or bottom edge, while a value of `1.0` means the shadow is drawn on the top or right edge. This attribute is dynamically adjusted based on the scrolling direction during rendering.

### `scroll` (f32)
Represents the current scroll offset of the container along the relevant axis (horizontal or vertical). This value is used to calculate the shadow's position relative to the scrolling content.

## Notes

- Ensure that the `DrawScrollShadow` component is integrated within a scrollable container to function correctly.
- The shadows automatically adjust based on the scroll positions and the dimensions of the scrollable area.
