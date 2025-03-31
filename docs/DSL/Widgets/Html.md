# Html Widget

A widget that renders HTML content.

## Inherits

- `text_flow`: [TextFlow](TextFlow.md)
- `link`: [LinkLabel](LinkLabel.md)

## Layout

The `Html` widget relies on its parent's layout properties.

## Fields

### `body` (`RcStringMut`)

Contains the HTML code to be rendered by the widget.

### `ul_markers` (`Vec<String>`)

A vector of markers used for unordered lists, indexed by the list's nesting level. Each marker can be an arbitrary string, such as a bullet point or a custom icon.

### `ol_markers` (`Vec<OrderedListType>`)

A vector of markers used for ordered lists, indexed by the list's nesting level. Allows customization of numbering styles, such as numbers, letters, or Roman numerals.

### `ol_separator` (`String`)

The character or string used to separate an ordered list's item number from the content. For example, using `")"` would format list items as `1)`.

## HtmlLink

### Fields

#### `href` (`String`)

The URL to which the link points.

#### `link` ([LinkLabel](LinkLabel.md))

The textual representation of the link to be displayed.

## Examples

### Typical Usage

```rust
MyHtml = <Html> {
    body: "<h1>H1 Headline</h1><h2>H2 Headline</h2><h3>H3 Headline</h3><h4>H4 Headline</h4><h5>H5 Headline</h5><h6>H6 Headline</h6>This is <b>bold</b> and <i>italic text</i>. <b><i>Bold italic</i></b>, <u>underlined</u>, and <s>strikethrough</s> text. <p>This is a paragraph.</p> <code>A code block</code>. <br/> And this is a <a href='https://www.google.com/'>link</a><br/><ul><li>lorem</li><li>ipsum</li><li>dolor</li></ul><ol><li>lorem</li><li>ipsum</li><li>dolor</li></ol><br/><blockquote>Blockquote</blockquote> <pre>preformatted text</pre><sub>subscript</sub><del>deleted text</del>",

    // Layout Properties
    height: Fill, // Fills the available vertical space
    width: Fill,  // Fills the available horizontal space
}
```

### Advanced Usage

```rust
MyHtml = <Html> {
    body: "<h1>H1 Headline</h1><p>Text</p>",

    // Layout Properties
    height: Fill,
    width: Fill,
    margin: 10.0,
    padding: 5.0,
    flow: Right,
    spacing: 0.0,
    align: { x: 0.0, y: 0.0 },
} 

<MyHtml> {
    body: "<h1>H1 Headline</h1><p>Text</p>"
}
```