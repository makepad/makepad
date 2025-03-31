Defines different sizing modes for widgets in the Makepad UI framework.

```rust
pub enum Size {
    Fill,
    Fixed(f64),
    Fit,
    All,
}
```

## Variants

### Fill

Expands the element to fill the available space within the parent container, respecting the parent's constraints, padding, and margins.

### Fixed(f64)

Sets the element's size to an absolute value specified by the `f64` parameter.

*Example:* `Fixed(200.0)` sets the size to 200 units.

### Fit

Sizes the element based on the combined dimensions of its children. The element adjusts to tightly enclose its content.

### All

Makes the element take up the full size specified by the parent container, ignoring any padding, margins, or constraints. Unlike `Fill`, which fills the available space considering constraints, `All` stretches the element to match the exact dimensions of the parent container.

## Example

```rust
// A view that adjusts its height to fit its children and fills the available width.
<View> {
    flow: Down,    // Sets the layout flow direction to vertical.
    height: Fit,   // The view's height adjusts to fit its children.
    width: Fill,   // The view's width fills the available space in the parent container.
    margin: {      // Adds spacing around the view.
        top: 10.0,
        right: 5.0,
        bottom: 10.0,
        left: 5.0,
    },
    // ... child elements ...
}
```

In this example, the `<View>`:

- Uses `flow: Down` to stack its child elements vertically.
- Sets `height: Fit` so its height adjusts based on its children's heights.
- Sets `width: Fill` to occupy all available horizontal space within its parent.
- Applies margins to add spacing outside the view's borders.