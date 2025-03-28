# Align

Makepad's foundational alignment property for layout elements.

## align (Align)

Sets the alignment of child elements within their parent container along the horizontal (`x`) and vertical (`y`) axes.

The `align` property is an object with `x` and `y` fields, each accepting a floating-point value between `0.0` and `1.0`, representing the alignment along the respective axis.

**Options**

- **Horizontal Alignment (`align.x`):**
  - `0.0`: Left-aligned
  - `0.5`: Center-aligned horizontally
  - `1.0`: Right-aligned

- **Vertical Alignment (`align.y`):**
  - `0.0`: Top-aligned
  - `0.5`: Center-aligned vertically
  - `1.0`: Bottom-aligned

## Example

```rust
<Label> {
    text: "Hello world"
    align: { x: 0.0, y: 0.5 } // Positioned on the left and vertically centered
}
```

In this example, the `Label` is aligned to the left edge (`x: 0.0`) of its parent container and vertically centered (`y: 0.5`).

## Notes

- The `align` values are floating-point numbers between `0.0` and `1.0`.
- Values outside the range of `0.0` to `1.0` can be used for alignment beyond the edges of the parent container.
- The default alignment is typically `x: 0.0` and `y: 0.0`, which places the element at the top-left corner of the parent.