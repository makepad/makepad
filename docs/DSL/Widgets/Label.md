The standard text widget.

## [Layouting](Layouting.md)

Complete layouting feature set support.

## DrawShaders

### `draw_text` ([DrawText](DrawText.md))

This field allows styling of the label's text with all attributes supported by [DrawText](DrawText.md), including colors, font, font size, and more.

## Fields

### `align` (Align)

Controls where the text is placed within the parent container.

- **x**: `float` (0.0 - 1.0)
- **y**: `float` (0.0 - 1.0)

### `padding` ([Padding](../ft/ft_padding.md))

Defines the space between the label's border and its content.

### `text` (`RcStringMut`)

The text to be displayed by the label.

### `hover_actions_enabled` (`bool`)

Indicates if this label responds to hover events.

*Note:* It is not enabled by default because it will consume finger events and prevent other widgets from receiving them if not considered carefully. The primary use case for this is displaying tooltips.


## Widget Presets & Variations

The base theme includes a typographic system following the HTML convention for text formats:

- `<H1>`: Headline
- `<H1italic>`: Headline (Italic)
- `<H2>`: Headline
- `<H2italic>`: Headline (Italic)
- `<H3>`: Headline
- `<H3italic>`: Headline (Italic)
- `<H4>`: Headline
- `<H4italic>`: Headline (Italic)
- `<P>`: Paragraph
- `<Pbold>`: Bold paragraph text
- `<Pitalic>`: Italic paragraph text
- `<Pbolditalic>`: Bold italic paragraph text

## Examples

### Basic Usage

```rust
<Label> {
    text: "Hello World", // Simple label with default settings
}
```

### Typical

```rust
<Label> {
    text: "Hello world",
    align: { x: 0.0, y: 0.5 },          // Aligns text to the left and vertically centered
	// LAYOUT PROPERTIES
	width: Fill,
	// Element expands to use all available horizontal space.
	margin: { top: 10.0, right: 5.0, bottom: 10.0, left: 5.0 },
	// Individual margins outside the element for all four directions.
    padding: { left: 10.0, right: 10.0 }, // Adds horizontal padding
}
```

### Advanced

```rust
MyLabel = <Label> {
    draw_text: {
        fn get_color(self) -> vec4 {
            return mix(#00f, #f00, self.pos.x); // Gradient from blue to red based on position
        },
        color: #f00, // Sets default text color to red
        text_style: {
            font_size: 40.0, // Increases font size
        },
        wrap: Word, // Wraps text at word boundaries
    },

    text: "Hello world",

    // LAYOUT PROPERTIES
    height: Fit,               // Fits height to content
    width: Fit,                // Fits width to content
    padding: 10.0,             // Adds uniform padding
    margin: 5.0,               // Adds uniform margin
    flow: Right,               // Lays out child elements from left to right
    spacing: 10.0,             // Spacing between child elements
    align: { x: 0.0, y: 0.5 }, // Aligns text to the left and vertically centered
    line_spacing: 1.5,         // Line spacing for text content
}

<MyLabel> {
    text: "My Label",
}
```

*Note:* In the advanced example, we define a custom label `MyLabel` with advanced styling and use it to display "My Label".
