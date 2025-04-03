A widget that renders Markdown-formatted text.

## Layouting
Complete layouting feature set support.

## Fields

### body (ArcStringMut)
This field holds the Markdown content that will be rendered by the widget.

### paragraph_spacing (f64)
The amount of space between displayed paragraphs.

### text_flow ([TextFlow](TextFlow.md))
Selects the desired [TextFlow](TextFlow.md) mode.
## Examples

### Typical
```rust
<Markdown> {
    body: "# Headline 1\n## Headline 2\n### Headline 3\n#### Headline 4\nThis is standard text with a\n\nline break, a short ~~strike through~~ demo.\n\n*Italic text*\n\n**Bold text**\n\n- Bullet\n- Another bullet\n\n- Third bullet\n\n1. Numbered list Bullet\n2. Another list entry\n\n3. Third list entry\n\n`Monospaced text`\n\n> This is a quote.\n\nThis is `inline code`.\n\n```code block```"

    // LAYOUT PROPERTIES
    height: Fill, // Element expands to use all available vertical space.
    width: Fill,  // Element expands to use all available horizontal space.
}
```

**Explanation:** This example creates a `Markdown` widget that renders a sample Markdown text showcasing various formatting options such as headers, bold and italic text, lists, code blocks, and more.

### Advanced 
```rust
MyMarkdown = <Markdown> {
    body: "# Headline 1\n## Headline 2\n### Headline 3\n#### Headline 4\nThis is standard text with a\n\nline break, a short ~~strike through~~ demo.\n\n*Italic text*\n\n**Bold text**\n\n- Bullet\n- Another bullet\n\n- Third bullet\n\n1. Numbered list Bullet\n2. Another list entry\n\n3. Third list entry\n\n`Monospaced text`\n\n> This is a quote.\n\nThis is `inline code`.\n\n```code block```"

    paragraph_spacing: 20.0,   // Sets the spacing between paragraphs.

    // LAYOUT PROPERTIES
    height: 300.,              // Sets the widget height to 300 units.
    width: Fill,               // Element expands to use all available horizontal space.
    margin: 10.0,              // Adds a margin of 10 units around the widget.
    padding: 10.0,             // Adds padding of 10 units inside the widget.
    flow: Right,               // Sets the flow direction to right.
    align: { x: 0.0, y: 0.0 }, // Aligns the content to the top-left.
    line_spacing: 1.5          // Sets the spacing between lines.
}

<MyMarkdown> {
    body: "# Headline 1\n\nMy Text"
}
```

**Explanation:** This advanced example defines a custom widget `MyMarkdown` based on `Markdown` with additional styling properties such as paragraph spacing, margin, padding, and line spacing. The custom widget is then instantiated with custom Markdown content.
