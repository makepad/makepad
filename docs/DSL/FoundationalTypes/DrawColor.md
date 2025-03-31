# DrawColor

`DrawColor` is a drawing component that fills an area with a solid color. It is a basic drawable element that can be customized or extended by overriding its shader.

## Properties

### color (Vec4)

Controls the fill color of elements. This property can be overridden using a custom shader.
## color (Vec4)
Specifies the RGBA color value to be displayed.
## draw_super ([DrawQuad](DrawQuad.md))
An optional `DrawShader` that allows for more complex designs.

## Example

```rust
<View> {
    show_bg: true, // Enables the display of a background
    draw_bg: { 
        color: #f00 // Sets the background color to red
    },

    // LAYOUT PROPERTIES
    width: 200.0,  // Sets the width to 200 units
    height: 200.0, // Sets the height to 200 units
    flow: Down,    // Sets the layout flow direction to downward
}
```

In this example, a `View` is created with the background enabled (`show_bg: true`). The background is drawn using `DrawColor` by specifying `draw_bg` with the `color` property set to red (`#f00`). The view has dimensions of `200` units in width and height, and its layout flow is set to `Down`, meaning child elements would stack vertically.