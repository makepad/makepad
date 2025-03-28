A widget for displaying monochromatic single-path SVG vectors. Supports both inline SVG data and SVG files.

## [Layouting](Layouting.md)

Supports the complete layout feature set.

## Draw Shaders

### draw_bg ([DrawQuad](DrawQuad.md))

Used for non-vector icons that are drawn with shaders.

### draw_icon ([DrawIcon](DrawIcon.md))

Displays a monochrome SVG vector.

## Fields

### icon_walk ([Walk](Walk.md))
Controls the icon's inner layout properties as supported by [`Walk`](Walk.md).

## Examples

### Basic Usage

```rust
MyIcon = <Icon> {
    draw_icon: { svg_file: dep("crate://self/resources/icons/icon_text.svg") }

    // Layout properties
    height: 15.0, // Element is 15 units high.
    width: 15.0,  // Element is 15 units wide.
}
```

### Typical Usage

```rust
<Icon> {
    draw_icon: {
        color: #AA0000,
        color_active: #FF0000,
        svg_file: dep("crate://self/resources/icons/icon_text.svg"),
    }

    icon_walk: {
        width: 10.5,
        height: Fit,
        margin: 5.0,
    }

    // Layout properties
    height: 15.0, // Element is 15 units high.
    width: Fill,  // Element expands to use all available horizontal space.
    margin: { top: 10.0, right: 5.0, bottom: 10.0, left: 5.0 },
    // Individual margins outside the element for all four directions.
    padding: { top: 15.0, left: 0.0 },
    // Individual space between the element's border and its content for top and left.
}
```

### Advanced Usage

```rust
MyIcon = <Icon> {
    draw_bg: {
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.circle(5.0, 5.0, 4.0);
            sdf.fill(#888888);
            sdf.move_to(3.0, 5.0);
            sdf.line_to(3.0, 5.0);
            sdf.move_to(5.0, 5.0);
            sdf.line_to(5.0, 5.0);
            sdf.move_to(7.0, 5.0);
            sdf.line_to(7.0, 5.0);
            sdf.stroke(#000000, 0.8);
            return sdf.result;
        }
    }

    icon_walk: {
        width: 10.5,
        height: Fit,
        margin: 5.0,
    }

    // Layout properties
    height: 15.0,
    width: 15.0,
    margin: 5.0,
    padding: 2.5,
    align: { x: 0.5, y: 0.5 },
}

<MyIcon> {}
```
