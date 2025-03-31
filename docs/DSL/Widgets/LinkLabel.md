A clickable label widget that opens a URL when clicked.

# Inherits
- [[Button]]

## [Layouting](Layouting.md)
Complete layouting feature set support.

## Fields
### url (String)
The URL that will be opened when the link is clicked.

### open_in_place (bool)
If `true`, opens the URL in place; otherwise, opens in a new window or tab.


## Examples

### Typical
```Rust
<LinkLabel> {
    text: "Click me",
    url: "https://example.com"
}
```
This example creates a simple link label that opens `https://example.com` when clicked.

### Advanced
```Rust
MyLinkLabel = <LinkLabel> {
    text: "Click me!",
    url: "https://example.com",
    open_in_place: true,

    draw_text: {
        fn get_color(self) -> vec4 {
            return mix(#00f, #f00, self.pos.x)
        },
        color: #f00,
        text_style: {
            font_size: 40.0
        },
        wrap: Word
    },

    // LAYOUT PROPERTIES

    height: Fit,
    width: Fit,
    margin: 2.5,
    padding: 2.5,
    align: { x: 0.0, y: 0.5 },
    line_spacing: 1.5
}

<MyLinkLabel> {
    text: "Click me!"
}
```
In this advanced example, we define a custom `MyLinkLabel` that:

- Changes the text color dynamically based on the x-position.
- Sets a larger font size and text wrapping.
- Includes additional layout properties like `height`, `width`, `margin`, and `padding`.