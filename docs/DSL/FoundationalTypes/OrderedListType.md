List style types. Used with the HTML and Markdown widgets.
## Numbers
Lists are enumerated with arabic numerals.
## LowerAlpha
Lists are enumerated alphabetically with lowercase letters.
## A: UpperAlpha
Lists are enumerated alphabetically with uppercase letters.
## i: LowerRoman
Lists are enumerated lowercase roman numerals. 
## I: UpperRoman
Lists are enumerated uppercase roman numerals. 
## Example
```rust
use OrderedListType::*;

let list_styles = vec![
    Numbers,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
];

// Example usage in a Html widget
MyHtml = <Html> {
    ol_markers: list_styles,
    // Other properties...
}
```
