This widget manages keyboard navigation, specifically handling focus traversal using the **Tab** key. It facilitates navigating through focusable elements (nav stops) in a structured and orderly manner.
## Layouting
No layouting support.

## DrawShaders

### draw_focus ([DrawQuad](DrawQuad.md))

This shader styles the focus indicator when a navigation stop gains focus. It allows customization of the focus appearance, such as color and size.

### draw_text ([DrawText](DrawText.md))

This shader allows styling of any text associated with the `NavControl`, supporting all attributes from [DrawText](DrawText.md), including colors, font, and font size.


## Examples
### Typical
```Rust
<NavControl> {}
```
### Advanced
```Rust
MyNavControl = <NavControl> {
  draw_text: {
    text_style: {
      font_size: 6
    },
    color: #f00
  }

}

<MyNavControl> {}
```